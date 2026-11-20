//! The merge dialog's Ask the agent to rebase (`POST tasks/<id>/pr/rebase`): the ask goes to the task's
//! open terminal, or brings its PR conversation back when that terminal is closed.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{midna, p, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    cfg.pr.wake = true;
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    fn report(&self, event: &str, session: &str, extra: Value) {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message));
    }
    fn add_session(&self, sid: &str, state: &str) {
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo, "status": {"state": state}})], &[])
            .unwrap();
    }
    /// A done task with PR #9 open, handed in from terminal `s1`.
    fn pr_task(&self) -> i64 {
        let body = json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"});
        let id = api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &body).unwrap()["id"].as_i64().unwrap();
        self.add_session("s1", "working");
        self.report("tb.take", "s1", json!({"task": format!("T{id}")}));
        self.report("tb.done", "s1", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9"}));
        self.add_session("s1", "idle");
        id
    }
    fn ask(&self, id: i64) -> taskboardd::util::Result<Value> {
        api::dispatch(&self.app, "POST", &format!("/tasks/T{id}/pr/rebase"), &Query::new(), &json!({"why": "The branch is 3 commits behind main."}))
    }
    fn jobs(&self, id: i64) -> Vec<(String, String, String)> {
        self.app
            .db
            .q("SELECT kind, purpose, args FROM jobs WHERE task_id = ? ORDER BY id", p![id])
            .unwrap()
            .iter()
            .map(|j| (j.st("kind"), j.st("purpose"), j.st("args")))
            .collect()
    }
}

#[test]
fn the_ask_goes_to_the_open_terminal() {
    let b = board();
    let id = b.pr_task();
    let out = b.ask(id).unwrap();
    assert_eq!(out["asked"], json!(true));
    let jobs = b.jobs(id);
    let sent = jobs.iter().find(|(k, p, _)| k == "deliver" && p == "rebase").unwrap_or_else(|| panic!("no rebase delivery: {jobs:?}"));
    assert!(sent.2.contains("rebase PR #9") && sent.2.contains("behind main") && sent.2.contains("--force-with-lease"), "{}", sent.2);
    assert!(!jobs.iter().any(|(k, _, _)| k == "agent"), "no new terminal while its own is open: {jobs:?}");
}

#[test]
fn a_closed_terminal_brings_the_pr_conversation_back() {
    let b = board();
    let id = b.pr_task();
    b.app.db.x("UPDATE sessions SET status = 'gone' WHERE id = 's1'", p![]).unwrap();
    b.ask(id).unwrap();
    let jobs = b.jobs(id);
    let visit = jobs.iter().find(|(k, p, _)| k == "agent" && p == "pr").unwrap_or_else(|| panic!("no PR visit: {jobs:?}"));
    assert!(visit.2.contains("rebase PR #9") && visit.2.contains("pr wait"), "{}", visit.2);
    b.ask(id).unwrap();
    let jobs = b.jobs(id);
    assert_eq!(jobs.iter().filter(|(k, _, _)| k == "agent").count(), 1, "one visit at a time: {jobs:?}");
    assert!(jobs.iter().any(|(k, p, _)| k == "deliver" && p == "rebase"), "a second ask goes to the visit on its way: {jobs:?}");
}

#[test]
fn a_task_without_a_pr_has_nothing_to_rebase() {
    let b = board();
    let body = json!({"title": "No PR", "detail": "Do it.", "project": "webapp"});
    let id = api::dispatch(&b.app, "POST", "/tasks", &Query::new(), &body).unwrap()["id"].as_i64().unwrap();
    assert_eq!(b.ask(id).unwrap_err().status, 404);
}
