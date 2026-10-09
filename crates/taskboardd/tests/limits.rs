//! Context limits (`tb limits`), generated files in `.git/info/attributes`, the compacting flag,
//! refused attachments, terminals in Midna's Background group and the comment guard.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, gitattrs, handoff, midna, p, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    std::fs::create_dir_all(dir.path().join("webapp")).unwrap();
    f(&mut cfg);
    let app = App::for_tests(cfg);
    let b = Board { app, dir };
    b.app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": b.repo().to_string_lossy()}]).to_string())).unwrap();
    b
}

impl Board {
    fn repo(&self) -> PathBuf {
        self.dir.path().join("webapp")
    }
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": self.repo().to_string_lossy()});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn add_session(&self, status: &str) {
        let s = json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": self.repo().to_string_lossy(), "status": {"state": status}});
        midna::sync(&self.app, &[s], &[]).unwrap();
    }
    fn task(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn new_task(&self) -> i64 {
        let id = self.post("/tasks", json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
        self.add_session("working");
        self.report("tb.take", json!({"task": format!("T{id}")})).unwrap();
        id
    }
    /// A transcript for the conversation `c-s1`: its last reply's context size, written `mins_ago`.
    fn transcript(&self, tokens: i64, mins_ago: i64) {
        let at = taskboardd::util::iso(taskboardd::util::now_ts() - mins_ago as f64 * 60.0);
        let dir = self.app.cfg.claude_projects.join("-somewhere");
        std::fs::create_dir_all(&dir).unwrap();
        let line = json!({"type": "assistant", "timestamp": at, "message": {"usage": {"input_tokens": 10, "cache_read_input_tokens": tokens - 10}}});
        std::fs::write(dir.join("c-s1.jsonl"), format!("{line}\n")).unwrap();
    }
}

fn git(dir: &Path, args: &[&str]) {
    let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().expect("git runs");
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
}

fn git_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "T"]);
    std::fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "first"]);
    git(dir, &["checkout", "-q", "-b", "feature"]);
}

#[test]
fn tb_limits_shows_changes_and_resets() {
    let b = board_with(|_| {});
    let v = b.get("/limits");
    assert_eq!(v["compact_window"], 150_000);
    assert_eq!(v["warm_tokens"], 60_000);
    assert!(v["line"].as_str().unwrap().contains("Compact window: 150k tokens"));

    let v = b.post("/limits", json!({"compact_window": 120000, "cold_idle_mins": 0, "generated": "*.g.dart, Cargo.lock"})).unwrap();
    assert_eq!(v["compact_window"], 120_000);
    assert!(v["line"].as_str().unwrap().contains("Compact before resuming after: off"));
    assert_eq!(v["generated"], json!(["*.g.dart", "Cargo.lock"]));
    let v = b.post("/limits", json!({"generated": ["*.pb.go"], "project": "api"})).unwrap();
    assert_eq!(v["project_generated"]["api"], json!(["*.pb.go"]));
    assert_eq!(v["defaults"]["compact_window"], 150_000);

    assert_eq!(b.post("/limits", json!({"warm_tokens": -1})).unwrap_err().0, 400);
    let v = b.post("/limits", json!({"reset": true})).unwrap();
    assert_eq!(v["compact_window"], 150_000);
    assert_eq!(v["generated"], json!([]));
    assert_eq!(taskboardd::builds::settings_arg(&b.app, "/nonexistent").as_deref(), Some(r#"{"autoCompactWindow":150000}"#));
}

#[test]
fn a_pr_wake_resumes_only_a_warm_small_conversation() {
    let b = board_with(|c| {
        c.pr.watch = true;
        c.pr.wake = true;
    });
    let id = b.new_task();
    b.report("tb.done", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9"})).unwrap();
    // Its terminal is gone, so a wake opens a new one.
    b.app.db.x("UPDATE sessions SET status = 'gone'", p![]).unwrap();
    let rec = json!({"state": "OPEN", "head": "h1", "checks": [{"name": "ci", "state": "failed"}], "failed": ["ci"], "running": 0,
                     "comments": 0, "approvals": 0, "review_decision": ""});
    let wake = |b: &Board| {
        b.app.db.x("DELETE FROM jobs WHERE purpose = 'pr'", p![]).unwrap();
        let t = b.task(id);
        b.app.db.tx(|| prflow::wake(&b.app, &t, "fix", &rec, None)).unwrap();
        board::job_args(&b.app.db.q1("SELECT * FROM jobs WHERE purpose = 'pr' ORDER BY id DESC", p![]).unwrap().unwrap())
    };

    b.transcript(20_000, 5);
    assert_eq!(wake(&b).st("flags"), "--resume c-s1", "warm and small: resumed");

    b.transcript(90_000, 5);
    let a = wake(&b);
    assert!(a.s("flags").is_none(), "too big: fresh");
    assert!(a.st("prompt").contains("This is a fresh conversation"));
    let log = b.app.db.q1("SELECT text FROM events WHERE task_id = ? AND kind = 'handoff' AND text LIKE 'Starting a fresh%' ORDER BY id DESC", p![id]).unwrap().unwrap();
    assert!(log.st("text").contains("90k tokens, over the 60k"), "{}", log.st("text"));

    b.transcript(20_000, 180);
    assert!(wake(&b).s("flags").is_none(), "idle 3h: fresh");

    b.post("/limits", json!({"warm_tokens": 0, "warm_idle_mins": 0})).unwrap();
    assert_eq!(wake(&b).st("flags"), "--resume c-s1", "with the caps off it resumes");
}

#[test]
fn cold_conversations_and_background_purposes() {
    let b = board_with(|c| c.terminals.background = vec!["plan".into()]);
    b.transcript(20_000, 90);
    let c = taskboardd::limits::conversation(&b.app, "", "c-s1", None);
    assert!(taskboardd::limits::is_cold(&b.app, &c), "idle 90m is over the hour");
    b.transcript(20_000, 10);
    assert!(!taskboardd::limits::is_cold(&b.app, &taskboardd::limits::conversation(&b.app, "", "c-s1", None)));

    let job = |purpose: &str| {
        let mut j = Row::new();
        j.insert("purpose".into(), json!(purpose));
        j
    };
    assert!(midna::opens_in_background(&b.app, &job("plan")));
    assert!(!midna::opens_in_background(&b.app, &job("start")));
}

#[test]
fn generated_files_go_in_a_managed_attributes_block() {
    let b = board_with(|c| c.limits.generated = vec!["*.g.dart".into()]);
    git_repo(&b.repo());
    let attrs = b.repo().join(".git").join("info").join("attributes");
    std::fs::create_dir_all(attrs.parent().unwrap()).unwrap();
    std::fs::write(&attrs, "*.png binary\n").unwrap();
    assert_eq!(gitattrs::sync(&b.app).unwrap(), 1);
    let text = std::fs::read_to_string(&attrs).unwrap();
    assert!(text.starts_with("*.png binary\n") && text.contains("*.g.dart -diff"), "{text}");
    assert_eq!(gitattrs::sync(&b.app).unwrap(), 0, "nothing changes the second time");

    b.post("/limits", json!({"generated": "Cargo.lock", "project": "webapp"})).unwrap();
    gitattrs::sync(&b.app).unwrap();
    let text = std::fs::read_to_string(&attrs).unwrap();
    assert!(text.contains("*.g.dart -diff\nCargo.lock -diff"), "{text}");

    b.post("/limits", json!({"generated": "none"})).unwrap();
    b.post("/limits", json!({"generated": [], "project": "webapp"})).unwrap();
    gitattrs::sync(&b.app).unwrap();
    assert_eq!(std::fs::read_to_string(&attrs).unwrap(), "*.png binary\n", "the block goes with no globs");
}

#[test]
fn a_repo_no_longer_looked_after_loses_its_block() {
    let b = board_with(|c| c.limits.generated = vec!["*.g.dart".into()]);
    git_repo(&b.repo());
    let attrs = b.repo().join(".git").join("info").join("attributes");
    std::fs::create_dir_all(attrs.parent().unwrap()).unwrap();
    std::fs::write(&attrs, "*.png binary\n").unwrap();
    gitattrs::sync(&b.app).unwrap();
    assert!(std::fs::read_to_string(&attrs).unwrap().contains("*.g.dart -diff"));

    b.app.db.set_setting("midna_projects", Some("[]")).unwrap();
    assert_eq!(gitattrs::sync(&b.app).unwrap(), 1);
    assert_eq!(std::fs::read_to_string(&attrs).unwrap(), "*.png binary\n", "the project went, so its block did");
    assert_eq!(gitattrs::sync(&b.app).unwrap(), 0, "and it's forgotten after that");
}

#[test]
fn a_removed_project_with_finished_work_loses_its_block() {
    let b = board_with(|c| c.limits.generated = vec!["*.g.dart".into()]);
    git_repo(&b.repo());
    let attrs = b.repo().join(".git").join("info").join("attributes");
    let id = b.new_task();
    gitattrs::sync(&b.app).unwrap();
    assert!(std::fs::read_to_string(&attrs).unwrap().contains("*.g.dart -diff"));

    b.app.db.set_setting("midna_projects", Some("[]")).unwrap();
    gitattrs::sync(&b.app).unwrap();
    assert!(std::fs::read_to_string(&attrs).unwrap().contains("*.g.dart -diff"), "its task is still open");
    b.app.db.x("UPDATE tasks SET status = 'done' WHERE id = ?", p![id]).unwrap();
    b.app.db.x("UPDATE sessions SET status = 'gone'", p![]).unwrap();
    gitattrs::sync(&b.app).unwrap();
    assert!(!std::fs::read_to_string(&attrs).unwrap().contains("-diff"), "a done task doesn't keep it a target");
}

#[test]
fn blocks_from_before_they_were_tracked_and_the_python_board_s_are_cleaned_up() {
    let b = board_with(|c| c.limits.generated = vec!["*.g.dart".into()]);
    git_repo(&b.repo());
    let gone = b.dir.path().join("old");
    std::fs::create_dir_all(&gone).unwrap();
    git_repo(&gone);
    let id = b.post("/tasks", json!({"title": "Old", "detail": "Old.", "project": "old"})).unwrap()["id"].as_i64().unwrap();
    b.app.db.x("UPDATE tasks SET status = 'done', repo_path = ? WHERE id = ?", p![gone.to_string_lossy(), id]).unwrap();
    let old_attrs = gone.join(".git").join("info").join("attributes");
    std::fs::create_dir_all(old_attrs.parent().unwrap()).unwrap();
    std::fs::write(&old_attrs, format!("*.png binary\n\n{}\n*.g.dart -diff\n{}\n", gitattrs::BEGIN, gitattrs::END)).unwrap();
    let attrs = b.repo().join(".git").join("info").join("attributes");
    std::fs::create_dir_all(attrs.parent().unwrap()).unwrap();
    std::fs::write(&attrs, "# task-board: generated files (diff-skipped)\n*.lock -diff\n# task-board: end\n").unwrap();

    gitattrs::sync(&b.app).unwrap();
    assert_eq!(std::fs::read_to_string(&old_attrs).unwrap(), "*.png binary\n", "a repo that stopped being a target before the upgrade");
    let text = std::fs::read_to_string(&attrs).unwrap();
    assert_eq!(text, format!("{}\n*.g.dart -diff\n{}\n", gitattrs::BEGIN, gitattrs::END), "the Python board's block is replaced, not doubled");
}

#[test]
fn a_compacting_terminal_is_flagged_until_it_speaks_again() {
    let b = board_with(|_| {});
    let id = b.new_task();
    b.report("hook.pre_compact", json!({"trigger": "auto"})).unwrap();
    let card = board::task_card(&b.app, &b.task(id)).unwrap();
    assert!(card["compacting"].is_string(), "{card}");
    let sessions = b.get("/sessions");
    let s = sessions["sessions"].as_array().unwrap().iter().find(|s| s["id"] == "s1").unwrap();
    assert!(s["compacting"].is_string());

    b.report("hook.session_start", json!({"source": "compact"})).unwrap();
    assert!(board::task_card(&b.app, &b.task(id)).unwrap()["compacting"].is_null());
}

#[test]
fn writing_is_refused_as_an_attachment() {
    let b = board_with(|_| {});
    b.new_task();
    let (code, msg) = b.report("tb.attach", json!({"url": "/tmp/report.MD", "title": "Report", "kind": "results"})).unwrap_err();
    assert_eq!(code, 400);
    assert!(msg.contains(".md") && msg.contains("brief artifact"), "{msg}");
    b.report("tb.attach", json!({"url": "https://example.com/report.html", "title": "Report", "kind": "results"})).expect("a link is fine");
    b.report("tb.attach", json!({"url": "/tmp/shot.png", "title": "Shot", "kind": "evidence"})).expect("an image is fine");

    let open = board_with(|c| c.attachments.refuse = vec![]);
    open.new_task();
    open.report("tb.attach", json!({"url": "/tmp/report.md", "title": "Report", "kind": "results"})).expect("nothing refused");
}

#[test]
fn the_comment_guard_holds_tb_done_while_the_branch_adds_comments() {
    let b = board_with(|c| c.comments.guard = true);
    git_repo(&b.repo());
    let id = b.new_task();
    assert!(handoff::build(&b.app, id).unwrap().contains("No code comments"));

    std::fs::write(b.repo().join("a.rs"), "// explains a\nfn a() {}\n").unwrap();
    let (code, msg) = b.report("tb.done", json!({"summary": "Done"})).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("a.rs:1") && msg.contains("explains a"), "{msg}");

    std::fs::write(b.repo().join("a.rs"), "#[allow(dead_code)]\nfn a() {}\n").unwrap();
    b.report("tb.done", json!({"summary": "Done"})).expect("no comments left");
    assert_eq!(b.task(id).s("status"), Some("done"));

    let off = board_with(|_| {});
    let id = off.new_task();
    assert!(!handoff::build(&off.app, id).unwrap().contains("No code comments"));
}

/// A `midna` that logs each call's method and params and answers `{}`.
fn fake_midna(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let exe = dir.join("midna");
    std::fs::write(&exe, format!("#!/bin/sh\necho \"$2 $3\" >> '{}'\necho '{{}}'\n", dir.join("calls.log").display())).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    exe
}

impl Board {
    /// The text of each `queue.add` the fake Midna got, in order; clears its log.
    fn typed(&self) -> Vec<String> {
        let log = self.dir.path().join("calls.log");
        let calls = std::fs::read_to_string(&log).unwrap_or_default();
        let _ = std::fs::remove_file(&log);
        calls
            .lines()
            .filter_map(|l| l.strip_prefix("queue.add "))
            .map(|p| serde_json::from_str::<Value>(p).unwrap()["text"].as_str().unwrap().to_string())
            .collect()
    }
    fn run(&self, jid: i64) {
        let j = self.app.db.q1("SELECT * FROM jobs WHERE id = ?", p![jid]).unwrap().unwrap();
        midna::run_job(&self.app, &j);
    }
    fn compact_events(&self, id: i64) -> Vec<String> {
        let rows = self.app.db.q("SELECT text FROM events WHERE task_id = ? AND text LIKE '%compacts it before sending%'", p![id]).unwrap();
        rows.iter().map(|r| r.st("text")).collect()
    }
}

#[test]
fn a_message_job_compacts_a_cold_open_terminal_first() {
    let b = board_with(|c| c.midna = fake_midna(&c.data.clone()));
    let id = b.new_task();
    let message = || board::create_job(&b.app, "message", json!({"to": "s1", "text": "Hello"}), Some(id), "deliver", None).unwrap();

    b.transcript(10_000, 10);
    b.run(message());
    assert_eq!(b.typed(), ["Hello"], "a warm terminal gets only the text");
    assert!(b.compact_events(id).is_empty());

    b.transcript(61_500, 90);
    let jid = message();
    b.run(jid);
    assert_eq!(b.typed(), ["/compact", "Hello"], "a cold one is compacted first");
    let events = b.compact_events(id);
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(events[0].starts_with("Its conversation has been idle 90 min (61.5k tokens)"), "{}", events[0]);

    b.app.db.x("UPDATE jobs SET state = 'running' WHERE id = ?", p![jid]).unwrap();
    b.run(jid);
    assert_eq!(b.typed(), ["Hello"], "a retried job doesn't compact again");
    assert_eq!(b.compact_events(id).len(), 1, "and logs it once");
}

#[test]
fn compacting_an_open_terminal_says_how_cold_it_was() {
    let c = taskboardd::limits::Conversation { tokens: Some(150_000), idle_mins: Some(72.4) };
    assert_eq!(
        midna::compact_open_text(&c),
        "Its conversation has been idle 72 min (150k tokens), so the board compacts it before sending it anything."
    );
    let c = taskboardd::limits::Conversation { tokens: None, idle_mins: Some(61.0) };
    assert_eq!(midna::compact_open_text(&c), "Its conversation has been idle 61 min, so the board compacts it before sending it anything.");
}
