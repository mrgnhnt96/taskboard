//! The owner's review switch (`pr.owner_review`, `tb project set --owner-review`, `tb project
//! owner-review`): off, a green PR is marked reviewed by the board instead of waiting on the owner.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, dispatch, midna, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    cfg.pr.wake = true;
    f(&mut cfg);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
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
    fn green(&self, id: i64) -> taskboardd::util::Row {
        let rec = json!({"state": "OPEN", "head": "h1", "checks": [{"name": "ci", "state": "passed"}], "failed": [], "running": 0,
                         "comments": 0, "approvals": 0, "review_decision": "", "changes_at": ""});
        self.app.db.tx(|| prflow::step(&self.app, &board::get_task(&self.app, id)?, &rec)).unwrap();
        board::get_task(&self.app, id).unwrap()
    }
    fn review_alerts(&self) -> usize {
        dispatch::alerts(&self.app).iter().filter(|a| a["text"].as_str().is_some_and(|t| t.contains("ready for review"))).count()
    }
}

#[test]
fn a_green_pr_waits_on_the_owner_by_default() {
    let b = board_with(|_| {});
    let id = b.pr_task();
    let t = b.green(id);
    assert_eq!(t.s("pr_phase"), Some("review"));
    assert!(prflow::awaiting_owner(&t), "it waits for the owner's review");
    assert_eq!(b.review_alerts(), 1);
}

#[test]
fn with_owner_review_off_the_board_marks_it_reviewed() {
    let b = board_with(|c| c.pr.owner_review = Some(false));
    let id = b.pr_task();
    let t = b.green(id);
    assert!(!prflow::awaiting_owner(&t), "nobody waits on the owner");
    assert_eq!(b.review_alerts(), 0, "no “ready for review” alert");
    let card = b.call("GET", &format!("/tasks/T{id}"), json!({}));
    assert!(card.to_string().contains("marked reviewed"), "the log says the board marked it");
}

#[test]
fn with_owner_review_off_the_you_step_reads_off_until_someone_reviews() {
    let b = board_with(|c| c.pr.owner_review = Some(false));
    let id = b.pr_task();
    let you = |b: &Board| b.call("GET", &format!("/tasks/T{id}"), json!({}))["pr"]["bar"]["you"].clone();
    assert_eq!(you(&b), "off");
    b.call("POST", "/projects/webapp", json!({"owner_review": true}));
    assert_eq!(you(&b), Value::Null, "on, it's the owner's step again");
    b.call("POST", "/projects/webapp", json!({"owner_review": false}));
    b.green(id);
    assert_eq!(you(&b), "reviewed", "a review on record still says so");
}

#[test]
fn the_board_wide_and_project_switches_override_config() {
    let b = board_with(|_| {});
    assert_eq!(b.call("POST", "/projects/owner-review", json!({"owner_review": "off"}))["owner_review"], false);
    assert!(!prflow::owner_review_on(&b.app, Some("webapp")));
    b.call("POST", "/projects/webapp", json!({"owner_review": true}));
    assert!(prflow::owner_review_on(&b.app, Some("webapp")), "the project's word beats the board's");
    assert!(!prflow::owner_review_on(&b.app, Some("other")));
    assert_eq!(b.call("POST", "/projects/owner-review", json!({"owner_review": null}))["owner_review"], true, "back to config.toml's (on)");
}
