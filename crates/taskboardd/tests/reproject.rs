//! `tb task set --project` and `tb backlog set --project`: work filed under the wrong project moves to the right
//! one by name in any case, keeping its goal; a task that already ran lets go of its terminal and waits for Start
//! in the new repo, and finished work or a PR stays put.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let mut projects = vec![];
    for name in ["lens", "taskboard"] {
        let repo = dir.path().join(name);
        std::fs::create_dir_all(&repo).unwrap();
        projects.push(json!({"name": name, "path": repo.to_string_lossy()}));
    }
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!(projects).to_string())).unwrap();
    Board { app, dir }
}

impl Board {
    fn repo(&self, name: &str) -> String {
        self.dir.path().join(name).to_string_lossy().to_string()
    }
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn post_err(&self, path: &str, body: Value) -> (u16, String) {
        let e = api::dispatch(&self.app, "POST", path, &Query::new(), &body).expect_err("should fail");
        (e.status, e.message)
    }
    fn task(&self, body: Value) -> String {
        self.post("/tasks", body)["ref"].as_str().unwrap().to_string()
    }
    fn row(&self, r: &str) -> taskboardd::util::Row {
        board::get_task(&self.app, r.trim_start_matches('T').parse().unwrap()).unwrap()
    }
    fn log(&self, r: &str) -> String {
        self.get(&format!("tasks/{r}")).to_string()
    }
}

#[test]
fn a_planned_task_moves_with_its_repo_and_keeps_its_goal() {
    let b = new_board();
    let g = b.post("/goals", json!({"name": "Lens polish", "project": "lens"}))["ref"].as_str().unwrap().to_string();
    let t = b.task(json!({"title": "Board badge is stale", "project": "lens", "goal_id": g, "status": "planned"}));
    b.post(&format!("tasks/{t}"), json!({"project": "TaskBoard"}));
    let row = b.row(&t);
    assert_eq!(row.st("project"), "taskboard", "the board's own spelling");
    assert_eq!(row.st("repo_path"), b.repo("taskboard"));
    assert_eq!(row.st("status"), "planned", "a task that never ran keeps its place");
    assert!(row.i("goal_id").is_some(), "a goal can hold more than one project's work");
    assert!(b.log(&t).contains("Moved from lens to taskboard"), "{}", b.log(&t));
}

#[test]
fn a_task_on_a_terminal_lets_go_of_it_and_waits_for_start() {
    let b = new_board();
    midna::sync(&b.app, &[json!({"id": "s1", "name": "Lens 1", "agent": "claude", "cwd": b.repo("lens"), "status": {"state": "working"}})], &[])
        .unwrap();
    let r = reports::handle(
        &b.app,
        json!({"event": "tb.new_task", "session": "s1", "claude_session": "c-s1", "cwd": b.repo("lens"), "git": {},
               "title": "Sync drops a task", "here": true}),
        false,
    )
    .unwrap();
    let t = r["created"][0].as_str().unwrap().to_string();
    assert_eq!(b.row(&t).st("session_id"), "s1");
    b.post(&format!("tasks/{t}"), json!({"project": "taskboard"}));
    let row = b.row(&t);
    assert_eq!(row.st("project"), "taskboard");
    assert_eq!(row.s("session_id"), None, "the Lens terminal is let go");
    assert_eq!(row.s("claude_session_id"), None, "its conversation ran in the old repo");
    assert_eq!((row.st("status").as_str(), row.st("pickup").as_str()), ("queued", "manual"), "waits for Start");
    assert!(b.log(&t).contains("let go of Lens 1"), "{}", b.log(&t));
}

#[test]
fn here_refuses_another_projects_work() {
    let b = new_board();
    midna::sync(&b.app, &[json!({"id": "s1", "name": "Lens 1", "agent": "claude", "cwd": b.repo("lens"), "status": {"state": "working"}})], &[])
        .unwrap();
    let new = |project: Option<&str>| {
        reports::handle(
            &b.app,
            json!({"event": "tb.new_task", "session": "s1", "claude_session": "c-s1", "cwd": b.repo("lens"), "git": {},
                   "title": "Fix the kernel gate", "here": true, "project": project}),
            false,
        )
    };
    let e = new(Some("taskboard")).unwrap_err();
    assert_eq!(e.status, 409);
    assert!(e.message.contains("works in lens") && e.message.contains("--project taskboard (without --here)"), "{}", e.message);
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM tasks", vec![]).unwrap(), 0, "nothing filed under the wrong project");
    let r = new(Some("lens")).unwrap();
    assert_eq!(b.row(r["created"][0].as_str().unwrap()).st("project"), "lens");
}

#[test]
fn an_unknown_project_or_a_done_task_is_refused() {
    let b = new_board();
    let t = b.task(json!({"title": "Wrong home", "project": "lens", "status": "planned"}));
    let (code, msg) = b.post_err(&format!("tasks/{t}"), json!({"project": "nowhere"}));
    assert_eq!(code, 400);
    assert!(msg.contains("lens, taskboard"), "names the projects it knows: {msg}");
    b.app.db.update("tasks", &json!(b.row(&t).id()), vec![("status", json!("done"))]).unwrap();
    let (code, msg) = b.post_err(&format!("tasks/{t}"), json!({"project": "taskboard"}));
    assert_eq!(code, 409, "{msg}");
    assert_eq!(b.row(&t).st("project"), "lens");
}

#[test]
fn a_task_with_a_pr_stays_put() {
    let b = new_board();
    let t = b.task(json!({"title": "Has a PR", "project": "lens", "status": "planned"}));
    b.app.db.update("tasks", &json!(b.row(&t).id()), vec![("pr_num", json!(12))]).unwrap();
    let (code, msg) = b.post_err(&format!("tasks/{t}"), json!({"project": "taskboard"}));
    assert_eq!(code, 409);
    assert!(msg.contains("PR"), "{msg}");
}

#[test]
fn a_backlog_issue_moves_and_says_so_in_its_log() {
    let b = new_board();
    let i = b.post("/backlog", json!({"title": "Stuck badge", "project": "lens", "kind": "bug"}))["id"].as_i64().unwrap();
    b.post(&format!("backlog/B{i}"), json!({"project": "taskboard"}));
    assert_eq!(board::get_issue(&b.app, i).unwrap().st("project"), "taskboard");
    let log = json!(board::issue_history(&b.app, i).unwrap()).to_string();
    assert!(log.contains("Moved from lens to taskboard"), "{log}");
    let (code, _) = b.post_err(&format!("backlog/B{i}"), json!({"project": "nowhere"}));
    assert_eq!(code, 400);
}
