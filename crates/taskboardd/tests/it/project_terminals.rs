//! A queued start waits only while its project has as many busy terminals as it may
//! (`[terminals] project_max`, or `tb project set --max-terminals`), and any task start waits while the
//! board runs as many task terminals as it may (`[terminals] global_max`, or `tb terminals --max`); its card says so.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, fields, jobs, midna, runner};

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
        self.queued_task_in("webapp")
    }
    fn queued_task_in(&self, project: &str) -> i64 {
        let goal = self.post("/goals", json!({"name": "Work", "project": project, "max_terminals": 4}))["id"].as_i64().unwrap();
        let id = self.post("/tasks", json!({"title": "Fix it", "detail": "Do it.", "project": project, "ships_pr": false, "goal_id": goal}))["id"]
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

/// A board over four projects with `n` working tasks in `alpha`, each in its own idle terminal, and one
/// more busy terminal in each project that isn't a task's.
fn board_with_tasks(n: usize) -> (Board, Vec<i64>) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let names = ["alpha", "bravo", "charlie", "delta"];
    let roots: Vec<String> = names
        .iter()
        .map(|n| {
            let r = dir.path().join(n);
            std::fs::create_dir_all(&r).unwrap();
            r.to_string_lossy().to_string()
        })
        .collect();
    let projects: Vec<Value> = names.iter().zip(&roots).map(|(n, r)| json!({"name": n, "path": r})).collect();
    app.db.set_setting("midna_projects", Some(&Value::Array(projects).to_string())).unwrap();
    let b = Board { app, repo: roots[3].clone(), _dir: dir };
    let mut sessions: Vec<Value> = roots
        .iter()
        .map(|r| json!({"id": format!("{r}-own"), "name": "Mine", "agent": "claude", "cwd": r, "status": {"state": "working"}}))
        .collect();
    sessions.extend((0..n).map(|i| json!({"id": format!("t{i}"), "name": "Task", "agent": "claude", "cwd": roots[0], "status": {"state": "idle"}})));
    midna::sync(&b.app, &sessions, &[]).unwrap();
    let tasks = (0..n)
        .map(|i| {
            let id = b.post("/tasks", json!({"title": format!("Work {i}"), "detail": "Do it.", "project": "alpha", "ships_pr": false}))["id"].as_i64().unwrap();
            board::update_task(&b.app, id, fields!["status" => "working", "session_id" => format!("t{i}")]).unwrap();
            id
        })
        .collect();
    (b, tasks)
}

#[test]
fn the_board_runs_eight_task_terminals_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    assert_eq!(app.cfg.terminals.global_max, 8);
}

#[test]
fn a_start_over_the_board_cap_waits_for_a_free_slot() {
    let (b, tasks) = board_with_tasks(2);
    let out = b.post("terminals", json!({"max_terminals": 2}));
    assert_eq!((out["max_terminals"].as_i64(), out["running"].as_i64()), (Some(2), Some(2)), "the busy terminals that aren't tasks' don't count");
    let t = b.queued_task_in("delta");
    assert_eq!(b.get(&format!("tasks/T{t}"))["waiting"], "Waits for a free slot: 2 of 2 task terminals running across the board");
    assert!(!b.takes_start(), "delta has room, the board doesn't");

    board::update_task(&b.app, tasks[0], fields!["status" => "done"]).unwrap();
    assert!(b.get(&format!("tasks/T{t}"))["waiting"].is_null());
    assert!(b.takes_start(), "a slot freed up");
    assert_eq!(b.get("terminals")["running"], 2, "the start being opened holds its slot");
}

#[test]
fn the_board_cap_holds_a_start_in_a_new_terminal_too() {
    let (b, _) = board_with_tasks(2);
    b.post("terminals", json!({"max_terminals": "2"}));
    let id = b.post("/tasks", json!({"title": "Now", "detail": "Do it.", "project": "delta", "ships_pr": false}))["id"].as_i64().unwrap();
    let from_app: Query = [(api::FROM.to_string(), "app".to_string())].into_iter().collect();
    api::dispatch(&b.app, "POST", &format!("/tasks/T{id}/start"), &from_app, &json!({"mode": "new"})).unwrap();
    assert!(!b.takes_start());

    let out = b.post("terminals", json!({"max_terminals": "default"}));
    assert_eq!((out["max_terminals"].as_i64(), out["max_terminals_set"].is_null()), (Some(8), true));
    assert!(b.takes_start());
}

#[test]
fn the_board_cap_is_a_count_from_one() {
    let (b, _) = board_with_tasks(0);
    let e = api::dispatch(&b.app, "POST", "terminals", &Query::new(), &json!({"max_terminals": 0})).unwrap_err();
    assert_eq!(e.status, 400);
}
