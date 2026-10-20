//! lens at `tb submit`: with `[lens] submit` and `on` set and the lens binary installed, a task finishes
//! only on a fresh review of its checkout; findings become notes, an open blocker holds it and an escalated one asks the owner.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, midna, p, reports};

struct Board {
    app: Arc<App>,
    repo: PathBuf,
    _dir: tempfile::TempDir,
}

fn git(repo: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "user.name=T", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn new_board(lens: bool) -> Board {
    board_with(lens, |_| {})
}

fn board_with(lens: bool, f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("app.rs"), "fn main() {}\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "init"]);
    git(&repo, &["checkout", "-qb", "feat/x"]);
    std::fs::write(repo.join("app.rs"), "fn main() { login(); }\n").unwrap();
    if lens {
        let bin = dir.path().join("lens");
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        cfg.lens.bin = bin.to_string_lossy().to_string();
        cfg.lens.submit = true;
    }
    f(&mut cfg);
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    Board { app, repo, _dir: dir }
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": self.repo.to_string_lossy(), "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn task(&self) -> i64 {
        let id = self.post("/tasks", json!({"title": "Sign-in screen", "detail": "Call login on start.", "project": "webapp"}))["id"].as_i64().unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")})).unwrap();
        id
    }
    fn done(&self) -> Result<Value, (u16, String)> {
        self.report("tb.done", json!({"summary": "Done"}))
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn notes(&self, id: i64) -> Vec<String> {
        let rows = self.app.db.q("SELECT text FROM events WHERE task_id = ? AND kind = 'note'", p![id]).unwrap();
        rows.iter().map(|e| e.st("text")).filter(|t| t.starts_with("lens ")).collect()
    }

    /// Writes the report `lens review` would for the checkout as it is now.
    fn review(&self, findings: Value) -> PathBuf {
        let merge_base = git(&self.repo, &["merge-base", "main", "HEAD"]);
        let work = self.repo.join(".review/feat/x");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("diff.patch"), format!("{}\n", git(&self.repo, &["diff", &merge_base, "--"]))).unwrap();
        let dirty = !git(&self.repo, &["status", "--porcelain", "--", "app.rs"]).is_empty();
        let report = json!({
            "version": 1, "pass": "local", "branch": "feat/x", "base": "main", "mergeBase": merge_base,
            "head": git(&self.repo, &["rev-parse", "HEAD"]), "dirty": dirty, "findings": findings,
        });
        let path = self.repo.join(".review/feat/x.json");
        std::fs::write(&path, report.to_string()).unwrap();
        path
    }

    /// Writes the fix loop's ledger for `id`, its one attempt ending at the working tree as it is now.
    fn ledger(&self, id: &str, fields: Value) -> String {
        let index = self.repo.join(".review/fix.index");
        let with_index = |args: &[&str]| {
            let o = Command::new("git").arg("-C").arg(&self.repo).args(args).env("GIT_INDEX_FILE", &index).output().unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        with_index(&["read-tree", "HEAD"]);
        with_index(&["add", "-A", "."]);
        let after = with_index(&["write-tree"]);
        let mut ledger = json!({"baseTree": "abc", "attempts": [{"n": 1, "after": after, "resolved": true}]});
        for (k, v) in fields.as_object().unwrap() {
            ledger[k] = v.clone();
        }
        let path = self.repo.join(format!(".review/feat/x/fix/{id}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, ledger.to_string()).unwrap();
        path.to_string_lossy().into_owned()
    }
}

fn finding(id: &str, severity: &str, fix: Value) -> Value {
    let mut f = json!({"id": id, "profiles": ["correctness"], "file": "app.rs", "line": 1, "severity": severity,
                       "title": format!("{id} breaks sign-in"), "failureScenario": "login() runs before the config loads"});
    if !fix.is_null() {
        f["fix"] = fix;
    }
    f
}

#[test]
fn without_lens_tb_done_is_as_before() {
    let b = new_board(false);
    let id = b.task();
    b.done().unwrap();
    assert_eq!(b.row(id).st("status"), "done");
    assert!(!b.repo.join(".review").exists());
}

#[test]
fn the_submit_gate_is_off_by_default_so_sessions_take_no_turns_for_review() {
    let b = board_with(true, |c| c.lens.submit = taskboardd::lens::SubmitConfig::default().submit);
    let id = b.task();
    b.done().unwrap();
    assert_eq!(b.row(id).st("status"), "done");
    assert!(!b.repo.join(".review").exists(), "no review intent written");
    let board = api::dispatch(&b.app, "GET", "/projects/lens", &Query::new(), &json!({})).unwrap();
    assert_eq!((board["lens"].clone(), board["submit"].clone()), (json!(true), json!(false)), "{board}");
}

#[test]
fn lens_off_for_the_board_or_the_project_skips_the_review() {
    let b = board_with(true, |c| c.lens.on = false);
    let id = b.task();
    b.done().unwrap();
    assert_eq!(b.row(id).st("status"), "done");
    let b = board_with(true, |c| {
        c.lens.projects.insert("webapp".into(), false);
    });
    let id = b.task();
    b.done().unwrap();
    assert_eq!(b.row(id).st("status"), "done");
    let b = board_with(true, |c| {
        c.lens.on = false;
        c.lens.projects.insert("webapp".into(), true);
    });
    b.task();
    assert_eq!(b.done().unwrap_err().0, 409, "a project can turn it back on");
}

#[test]
fn the_board_wide_and_project_switches_override_config() {
    let b = new_board(true);
    let off = b.post("/projects/lens", json!({"lens": "off"}));
    assert_eq!((off["lens"].clone(), off["set"].clone(), off["config"].clone()), (json!(false), json!(false), json!(true)));
    let id = b.task();
    b.done().unwrap();
    assert_eq!(b.row(id).st("status"), "done", "off on the board, no review is needed");
    let p = b.post("/projects/webapp", json!({"lens": "on"}));
    assert_eq!((p["lens"].clone(), p["lens_set"].clone()), (json!(true), json!(true)));
    b.task();
    assert_eq!(b.done().unwrap_err().0, 409, "the project's word beats the board's");
    assert_eq!(b.post("/projects/webapp", json!({"lens": "default"}))["lens"], false, "back to the board's");
    assert_eq!(b.post("/projects/lens", json!({"lens": null}))["lens"], true, "back to config.toml's (on)");
    let b = board_with(true, |c| {
        c.lens.projects.insert("webapp".into(), false);
    });
    b.post("/projects/webapp", json!({"lens": true}));
    assert!(taskboardd::lens::on(&b.app, Some("webapp")), "the board's word on a project beats [lens.projects]");
}

#[test]
fn no_lens_skips_the_review_and_notes_why() {
    let b = new_board(true);
    let id = b.task();
    let (code, why) = b.report("tb.done", json!({"summary": "Done", "no_lens": "  "})).unwrap_err();
    assert_eq!(code, 400, "{why}");
    assert!(why.contains("--no-lens \"<why>\""), "{why}");
    b.report("tb.done", json!({"summary": "Done", "no_lens": "the owner said to skip it"})).unwrap();
    assert_eq!(b.row(id).st("status"), "done");
    let log = b.app.db.q("SELECT text FROM events WHERE task_id = ?", p![id]).unwrap();
    assert!(log.iter().any(|e| e.st("text") == "Skipped the lens review: the owner said to skip it"), "{log:?}");
}

#[test]
fn with_lens_off_submit_says_nothing_about_it() {
    let b = board_with(true, |c| c.lens.on = false);
    let id = b.task();
    b.report("tb.done", json!({"summary": "Done", "no_lens": "not needed"})).unwrap();
    let log = b.app.db.q("SELECT text FROM events WHERE task_id = ?", p![id]).unwrap();
    assert!(!log.iter().any(|e| e.st("text").contains("lens")), "{log:?}");
    let b = new_board(false);
    let projects = api::dispatch(&b.app, "GET", "/projects", &Query::new(), &json!({})).unwrap();
    let webapp = projects["projects"].as_array().unwrap().iter().find(|p| p["name"] == "webapp").unwrap().clone();
    assert!(webapp.get("lens").is_none(), "no lens switch on a project while lens isn't installed: {webapp}");
}

#[test]
fn with_lens_and_no_review_it_asks_for_one_and_writes_the_intent() {
    let b = new_board(true);
    let id = b.task();
    let (code, why) = b.done().unwrap_err();
    assert_eq!(code, 409, "{why}");
    assert!(why.contains("Run `lens review --intent "), "{why}");
    let intent = std::fs::read_to_string(b.repo.join(format!(".review/T{id}-intent.md"))).unwrap();
    assert!(intent.contains("Sign-in screen") && intent.contains("Call login on start."), "{intent}");
    assert_eq!(std::fs::read_to_string(b.repo.join(".review/.gitignore")).unwrap(), "*\n");
    assert_ne!(b.row(id).st("status"), "done");
}

#[test]
fn with_lens_and_no_change_there_is_nothing_to_review() {
    let b = new_board(true);
    git(&b.repo, &["checkout", "--", "app.rs"]);
    let id = b.task();
    b.done().unwrap();
    assert_eq!(b.row(id).st("status"), "done");
}

#[test]
fn a_fresh_review_s_findings_become_notes_and_a_major_doesn_t_hold_it() {
    let b = new_board(true);
    let id = b.task();
    b.review(json!([finding("F1", "major", Value::Null)]));
    git(&b.repo, &["commit", "-qam", "login on start"]);
    b.done().unwrap();
    assert_eq!(b.row(id).st("status"), "done");
    let notes = b.notes(id);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("F1 major") && notes[0].contains("app.rs:1"), "{notes:?}");
}

#[test]
fn a_change_after_the_review_needs_a_new_one() {
    let b = new_board(true);
    let id = b.task();
    b.review(json!([]));
    std::fs::write(b.repo.join("app.rs"), "fn main() { login(); logout(); }\n").unwrap();
    let (code, why) = b.done().unwrap_err();
    assert_eq!(code, 409, "{why}");
    assert!(why.contains("older than the change"), "{why}");
    b.review(json!([]));
    b.done().unwrap();
    assert_eq!(b.row(id).st("status"), "done");
}

#[test]
fn an_open_blocker_holds_tb_done_for_the_fix_loop() {
    let b = new_board(true);
    let id = b.task();
    b.review(json!([finding("F1", "blocker", Value::Null)]));
    let (code, why) = b.done().unwrap_err();
    assert_eq!(code, 409, "{why}");
    assert!(why.contains("(F1). Run `lens review` again in ") && why.contains("/webapp (it picks the fix loop up"), "{why}");
    b.done().unwrap_err();
    assert_eq!(b.notes(id).len(), 1, "each finding is noted once");
    std::fs::write(b.repo.join("app.rs"), "fn main() { load_config(); login(); }\n").unwrap();
    let ledger = b.ledger("F1", json!({"status": "resolved"}));
    let fixed = finding("F1", "blocker", json!({"status": "resolved", "attempts": 1, "ledger": ledger}));
    let report = b.repo.join(".review/feat/x.json");
    let mut r: Value = serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    r["findings"] = json!([fixed]);
    std::fs::write(&report, r.to_string()).unwrap();
    b.done().unwrap();
    assert_eq!(b.row(id).st("status"), "done");
}

#[test]
fn an_escalated_blocker_asks_the_owner_and_waits_for_the_answer() {
    let b = new_board(true);
    let id = b.task();
    let ledger = b.repo.join(".review/feat/x/fix/F1.json");
    std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    std::fs::write(&ledger, json!({"baseTree": "abc", "attempts": [], "escalation": {"by": "judge", "needs": "decision", "reason": "two valid fixes"}}).to_string()).unwrap();
    b.review(json!([finding("F1", "blocker", json!({"status": "escalated", "attempts": 2, "ledger": ledger.to_string_lossy()}))]));

    let (code, why) = b.done().unwrap_err();
    assert_eq!(code, 409, "{why}");
    let t = b.row(id);
    assert_eq!((t.st("status").as_str(), t.st("needs_reason").as_str()), ("needs", "question"));
    assert!(t.st("question").contains("two valid fixes"), "{}", t.st("question"));

    let (_, why) = b.done().unwrap_err();
    assert!(why.contains("hasn't answered"), "{why}");

    b.post(&format!("/tasks/{id}/answer"), json!({"text": "Ship it as it is."}));
    b.done().unwrap();
    assert_eq!(b.row(id).st("status"), "done");
}
