//! A project's setup gaps (`setup.rs`): a Review step nobody can be asked for, or a PR flow with no
//! git remote. A green PR held at review with nobody who could review it reads as stuck, not as
//! waiting, and the owner is alerted once.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, dispatch, midna, prflow, reports, setup};

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
    fn green_pr(&self) -> i64 {
        let id = self.call("POST", "/tasks", json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")}));
        self.report("tb.done", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9"}));
        let rec = json!({"state": "OPEN", "head": "h1", "checks": [{"name": "ci", "state": "passed"}], "failed": [], "running": 0,
                         "comments": 0, "approvals": 0, "review_decision": "", "changes_at": "", "reviewers": []});
        self.app.db.tx(|| prflow::step(&self.app, &board::get_task(&self.app, id)?, &rec)).unwrap();
        id
    }
    fn stage(&self, id: i64) -> Value {
        board::task_card(&self.app, &board::get_task(&self.app, id).unwrap()).unwrap()["pr"]["stage"].clone()
    }
    fn sweep(&self) {
        self.app.db.tx(|| setup::sweep(&self.app)).unwrap();
    }
    fn setup_alerts(&self) -> Vec<String> {
        dispatch::alerts(&self.app).iter().filter(|a| a["key"] == "setup:webapp").map(|a| a["text"].as_str().unwrap_or("").to_string()).collect()
    }
    fn gap_keys(&self) -> Vec<String> {
        setup::gaps(&self.app, "webapp").unwrap().iter().map(|g| g.key.to_string()).collect()
    }
}

#[test]
fn a_review_step_with_no_reviewers_is_a_gap_until_someone_is_added_or_the_step_is_off() {
    let b = board_with(|_| {});
    assert_eq!(b.gap_keys(), vec!["no_reviewers"]);
    let shown = b.call("GET", "/projects", json!({}))["projects"].as_array().unwrap().iter().find(|p| p["name"] == "webapp").cloned().unwrap();
    assert_eq!(shown["setup_gaps"][0]["key"], "no_reviewers", "{shown}");
    b.call("POST", "/reviewers", json!({"project": "webapp", "reviewer": "Sam", "user": "sam-gh", "emails": ["sam@acme.dev"]}));
    assert!(b.gap_keys().is_empty(), "Sam can be asked");

    let off = board_with(|_| {});
    off.call("POST", "/projects/webapp", json!({"review": false}));
    assert!(off.gap_keys().is_empty(), "no Review step, nobody needed");
}

#[test]
fn a_green_pr_nobody_can_review_reads_stuck_and_alerts_the_owner_once() {
    let b = board_with(|c| c.pr.owner_review = Some(false));
    let id = b.green_pr();
    let stage = b.stage(id);
    assert_eq!(stage["phase"], "review");
    assert_eq!(stage["label"], "Stuck: no reviewers", "{stage}");
    assert_eq!(stage["setup"], true);

    b.sweep();
    let alerts = b.setup_alerts();
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert!(alerts[0].contains(&format!("T{id}'s PR is stuck")) && alerts[0].contains("no reviewers"), "{}", alerts[0]);

    let alert_id = dispatch::alerts(&b.app).iter().find(|a| a["key"] == "setup:webapp").unwrap()["id"].as_str().unwrap().to_string();
    dispatch::clear_alerts(&b.app, None, Some(&alert_id)).unwrap();
    b.sweep();
    assert!(b.setup_alerts().is_empty(), "dismissed stays dismissed while nothing changes");

    b.call("POST", "/projects/webapp", json!({"review": false}));
    b.sweep();
    assert!(b.setup_alerts().is_empty());
    assert_ne!(b.stage(id)["label"], "Stuck: no reviewers", "fixed: it moves on");
}

#[test]
fn a_project_with_gaps_but_no_work_raises_nothing_and_fixing_it_clears_the_alert() {
    let b = board_with(|_| {});
    b.sweep();
    assert!(b.setup_alerts().is_empty(), "nothing on it yet");
    b.call("POST", "/tasks", json!({"title": "Queued", "detail": "Do it.", "project": "webapp"}));
    b.app.db.x("UPDATE tasks SET status = 'queued'", taskboardd::p![]).unwrap();
    b.sweep();
    assert_eq!(b.setup_alerts().len(), 1, "work queued on a project that can't finish it");
    b.call("POST", "/reviewers", json!({"project": "webapp", "reviewer": "Sam", "user": "sam-gh", "emails": ["sam@acme.dev"]}));
    b.sweep();
    assert!(b.setup_alerts().is_empty(), "cleared once it's set up");
}

#[test]
fn a_pr_flow_on_without_a_git_remote_is_a_gap_and_the_handoff_says_so() {
    let b = board_with(|_| {});
    b.call("POST", "/reviewers", json!({"project": "webapp", "reviewer": "Sam", "user": "sam-gh", "emails": ["sam@acme.dev"]}));
    let repo = b.dir.path().join("webapp");
    assert!(std::process::Command::new("git").arg("init").arg("-q").arg(&repo).status().unwrap().success());
    b.call("POST", "/projects/webapp", json!({"pr_flow": "on"}));
    assert_eq!(b.gap_keys(), vec!["no_remote"]);
    let line = setup::handoff_line(&b.app, "webapp").unwrap().unwrap();
    assert!(line.contains("no git remote"), "{line}");
}
