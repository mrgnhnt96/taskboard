//! A queued start waits only while its project has as many busy terminals as it may
//! (`[terminals] project_max`, or `tb project set --max-terminals`), and its card says so.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{jobs, midna, runner};

struct Board {
    app: Arc<App>,
    repo: String,
    _dir: tempfile::TempDir,
}

fn new_board(project_max: i64) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.terminals.project_max = project_max;
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let repo = repo.to_string_lossy().to_string();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo}]).to_string())).unwrap();
    Board { app, repo, _dir: dir }
}

impl Board {
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    /// Two terminals open in the project, `n` of them working and the rest idle.
    fn busy(&self, n: usize) {
        let sessions: Vec<Value> = (0..2)
            .map(|i| {
                let state = if i < n { "working" } else { "idle" };
                json!({"id": format!("s{i}"), "name": format!("Term {i}"), "agent": "claude", "cwd": self.repo, "status": {"state": state}})
            })
            .collect();
        midna::sync(&self.app, &sessions, &[]).unwrap();
    }
    fn queued_task(&self) -> i64 {
        let goal = self.post("/goals", json!({"name": "Work", "project": "webapp", "max_terminals": 4}))["id"].as_i64().unwrap();
        let id = self.post("/tasks", json!({"title": "Fix it", "detail": "Do it.", "project": "webapp", "ships_pr": false, "goal_id": goal}))["id"]
            .as_i64()
            .unwrap();
        runner::start_queued(&self.app).unwrap();
        id
    }
    fn takes_start(&self) -> bool {
        jobs::take(&self.app).unwrap().is_some_and(|j| j.s("purpose") == Some("start"))
    }
}

#[test]
fn a_queued_start_opens_while_the_project_is_under_its_cap() {
    let b = new_board(3);
    b.busy(2);
    let t = b.queued_task();
    assert!(b.get(&format!("tasks/T{t}"))["waiting"].is_null());
    assert!(b.takes_start(), "two of three busy: one more may open");
}

#[test]
fn a_queued_start_waits_at_the_cap_and_says_why() {
    let b = new_board(2);
    b.busy(2);
    let t = b.queued_task();
    let card = b.get(&format!("tasks/T{t}"));
    assert_eq!(card["starting"], true);
    assert_eq!(card["waiting"], "Waits for a free terminal: 2 of 2 busy in webapp");
    assert!(!b.takes_start());

    b.busy(1);
    assert!(b.get(&format!("tasks/T{t}"))["waiting"].is_null());
    assert!(b.takes_start(), "a terminal freed up");
}

#[test]
fn a_project_sets_its_own_cap() {
    let b = new_board(5);
    b.busy(2);
    let out = b.post("projects/webapp", json!({"max_terminals": 2}));
    assert_eq!(out["max_terminals"], 2);
    assert_eq!(out["max_terminals_set"], 2);
    let t = b.queued_task();
    assert_eq!(b.get(&format!("tasks/T{t}"))["waiting"], "Waits for a free terminal: 2 of 2 busy in webapp");
    assert!(!b.takes_start());

    let out = b.post("projects/webapp", json!({"max_terminals": "default"}));
    assert_eq!(out["max_terminals"], 5);
    assert!(out["max_terminals_set"].is_null());
    assert!(b.takes_start());
}

#[test]
fn a_cap_is_a_count_from_one() {
    let b = new_board(5);
    let e = api::dispatch(&b.app, "POST", "projects/webapp", &Query::new(), &json!({"max_terminals": 0})).unwrap_err();
    assert_eq!(e.status, 400);
}
