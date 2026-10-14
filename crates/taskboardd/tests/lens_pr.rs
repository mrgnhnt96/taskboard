//! The lens review stage (`[pr.lens]`, `lens_pr.rs`): a new PR gets lens's PR pass, run headless, and
//! the `lens` stage while it runs; it never holds a merge.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{jloads_obj, now_iso, RowExt};
use taskboardd::{board, midna, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

/// A board whose `claude` records its arguments and folder, and answers with `answer.json` (exit 0) or,
/// when `fail` exists, exits 1.
fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    cfg.pr.wake = true;
    let claude = dir.path().join("fake-claude");
    std::fs::write(
        &claude,
        "#!/bin/sh\nd=$(dirname \"$0\")\necho \"$PWD|$*\" >> \"$d/calls.txt\"\nif [ -f \"$d/fail\" ]; then echo 'no gh login' >&2; exit 1; fi\ncat \"$d/answer.json\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(dir.path().join("answer.json"), r#"{"type":"result","is_error":false,"result":"Posted 2 findings."}"#).unwrap();
    cfg.claude = claude.to_string_lossy().to_string();
    let plugins = cfg.claude_projects.parent().unwrap().join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let installed = json!({"version": 2, "plugins": {"lens@lens": [{"scope": "user", "installPath": "/x", "version": "0.1.0"}]}});
    std::fs::write(plugins.join("installed_plugins.json"), installed.to_string()).unwrap();
    f(&mut cfg);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

fn lens_on() -> Board {
    board_with(|c| c.pr.lens.on = true)
}

fn green() -> Value {
    json!({"state": "OPEN", "head": "h1", "checks": [{"name": "ci", "state": "passed"}], "failed": [], "running": 0,
           "comments": 0, "approvals": 0, "review_decision": "", "changes_at": ""})
}

impl Board {
    fn call(&self, method: &str, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, method, path, &Query::new(), &body).unwrap_or_else(|e| panic!("{method} {path}: {}", e.message))
    }
    fn report(&self, event: &str, extra: Value) {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message));
    }
    fn pr_task(&self) -> i64 {
        let id = self.call("POST", "/tasks", json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")}));
        self.report("tb.done", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9"}));
        id
    }
    fn task(&self, id: i64) -> taskboardd::util::Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn lens(&self, id: i64) -> Value {
        jloads_obj(self.task(id).s("pr_flow")).get("lens").cloned().unwrap_or(Value::Null)
    }
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("calls.txt")).unwrap_or_default().lines().map(|l| l.to_string()).collect()
    }
    /// Steps a record with the lens run as given, as if it were still going.
    fn step_with_lens(&self, id: i64, lens: Value, rec: &Value) -> taskboardd::util::Row {
        prflow::merge_flow(&self.app, id, vec![("lens", lens)]).unwrap();
        self.app.db.tx(|| prflow::step(&self.app, &self.task(id), rec)).unwrap();
        self.task(id)
    }
}

#[test]
fn off_by_default_a_new_pr_gets_no_lens_review() {
    let b = board_with(|_| {});
    let id = b.pr_task();
    prflow::apply(&b.app, id, &green()).unwrap();
    assert!(b.calls().is_empty(), "claude wasn't run");
    assert!(b.lens(id).is_null());
    assert_eq!(b.task(id).s("pr_phase"), Some("review"));
}

#[test]
fn a_new_pr_gets_the_lens_pr_pass_once() {
    let b = lens_on();
    let id = b.pr_task();
    prflow::apply(&b.app, id, &green()).unwrap();
    let calls = b.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let repo = b.dir.path().join("webapp").canonicalize().unwrap();
    let (cwd, args) = calls[0].split_once('|').unwrap();
    assert_eq!(std::path::Path::new(cwd).canonicalize().unwrap(), repo, "run from the PR's repo");
    assert!(args.starts_with("-p /lens:review --pr 9 --permission-mode default --allowedTools Bash,"), "{args}");
    let lens = b.lens(id);
    assert_eq!(lens["state"], "done", "{lens}");
    assert_eq!(lens["head"], "h1");
    assert!(std::fs::read_to_string(lens["log"].as_str().unwrap()).unwrap().contains("Posted 2 findings."));
    assert_eq!(b.task(id).s("pr_phase"), Some("review"), "done, the PR moves on");
    let events = b.call("GET", &format!("/tasks/T{id}"), json!({})).to_string();
    assert!(events.contains("lens reviewed it"), "{events}");
    prflow::apply(&b.app, id, &green()).unwrap();
    let mut pushed = green();
    pushed["head"] = json!("h2");
    prflow::apply(&b.app, id, &pushed).unwrap();
    assert_eq!(b.calls().len(), 1, "a later read or push isn't reviewed again");
}

#[test]
fn a_project_can_turn_it_on_or_off() {
    let b = board_with(|c| {
        c.pr.projects.insert("webapp".into(), taskboardd::config::PrProject { lens: Some(true), ..Default::default() });
    });
    let id = b.pr_task();
    prflow::apply(&b.app, id, &green()).unwrap();
    assert_eq!(b.calls().len(), 1);
    let b = board_with(|c| {
        c.pr.lens.on = true;
        c.pr.projects.insert("webapp".into(), taskboardd::config::PrProject { lens: Some(false), ..Default::default() });
    });
    let id = b.pr_task();
    prflow::apply(&b.app, id, &green()).unwrap();
    assert!(b.calls().is_empty());
}

#[test]
fn a_failed_run_is_noted_and_the_pr_moves_on() {
    let b = lens_on();
    std::fs::write(b.dir.path().join("fail"), "").unwrap();
    let id = b.pr_task();
    prflow::apply(&b.app, id, &green()).unwrap();
    let lens = b.lens(id);
    assert_eq!(lens["state"], "failed", "{lens}");
    assert_eq!(lens["error"], "no gh login");
    assert_eq!(b.task(id).s("pr_phase"), Some("review"));
    let bar = b.call("GET", &format!("/tasks/T{id}"), json!({}));
    assert!(bar.to_string().contains("the lens review failed: no gh login"), "{bar}");
}

#[test]
fn without_the_plugin_installed_the_run_fails_without_calling_claude() {
    let b = lens_on();
    std::fs::remove_file(b.app.cfg.claude_projects.parent().unwrap().join("plugins/installed_plugins.json")).unwrap();
    let id = b.pr_task();
    prflow::apply(&b.app, id, &green()).unwrap();
    assert!(b.calls().is_empty());
    assert_eq!(b.lens(id)["error"], "the lens plugin isn't installed for claude");
    assert_eq!(b.task(id).s("pr_phase"), Some("review"));
}

#[test]
fn a_closed_pr_isn_t_reviewed() {
    let b = lens_on();
    let id = b.pr_task();
    let mut rec = green();
    rec["state"] = json!("MERGED");
    prflow::apply(&b.app, id, &rec).unwrap();
    assert!(b.calls().is_empty());
}

#[test]
fn while_lens_runs_the_pr_shows_the_lens_stage_before_review() {
    let b = lens_on();
    let id = b.pr_task();
    let t = b.step_with_lens(id, json!({"state": "running", "head": "h1", "at": now_iso(), "started_at": now_iso()}), &green());
    assert_eq!(t.s("pr_phase"), Some("lens"));
    assert_eq!(prflow::label("lens"), "Lens review");
    assert!(!prflow::awaiting_owner(&t), "the owner is asked to look once lens has posted");
    let card = b.call("GET", &format!("/tasks/T{id}"), json!({}));
    assert_eq!(card["pr"]["bar"]["lens"]["state"], "running", "{}", card["pr"]);
}

#[test]
fn lens_never_holds_a_failed_check_or_a_merge() {
    let b = board_with(|c| {
        c.pr.lens.on = true;
        c.pr.approvals = 1;
        c.pr.agents_merge = Some(false);
    });
    let id = b.pr_task();
    let running = json!({"state": "running", "head": "h1", "at": now_iso(), "started_at": now_iso()});
    let mut failed = green();
    failed["failed"] = json!(["ci"]);
    failed["checks"] = json!([{"name": "ci", "state": "failed"}]);
    assert_eq!(b.step_with_lens(id, running.clone(), &failed).s("pr_phase"), Some("fix"));
    let mut approved = green();
    approved["approvals"] = json!(1);
    approved["review_decision"] = json!("APPROVED");
    assert_eq!(b.step_with_lens(id, running, &approved).s("pr_phase"), Some("merge"));
}

#[test]
fn a_run_the_board_lost_stops_holding_after_its_timeout() {
    let b = lens_on();
    let id = b.pr_task();
    let t = b.step_with_lens(id, json!({"state": "running", "head": "h1", "at": "2020-01-01T00:00:00Z", "started_at": "2020-01-01T00:00:00Z"}), &green());
    assert_eq!(t.s("pr_phase"), Some("review"));
    let card = b.call("GET", &format!("/tasks/T{id}"), json!({}));
    assert_eq!(card["pr"]["bar"]["lens"]["state"], "failed", "{}", card["pr"]);
}
