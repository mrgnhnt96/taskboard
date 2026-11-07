//! Research tasks (`tb task new --kind research`): a task whose work is answering a question. It ends
//! with what it found attached instead of a PR, and nothing in its goal waits for it.

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
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn task(&self, title: &str, extra: Value) -> i64 {
        let mut body = json!({"title": title, "detail": "Do it.", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", body).unwrap()["id"].as_i64().unwrap()
    }
    fn take(&self, id: i64) {
        let repo = self.dir.path().join("webapp");
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[])
            .unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")})).unwrap();
    }
}

#[test]
fn a_research_task_ends_with_its_findings_attached_not_a_pr() {
    let b = new_board();
    let t = b.task("Compare queue libraries", json!({"kind": "research"}));
    let row = board::get_task(&b.app, t).unwrap();
    assert_eq!((row.s("kind"), row.i("ships_pr")), (Some("research"), Some(0)));
    b.take(t);

    let (status, why) = b.report("tb.done", json!({"summary": "Picked one"})).unwrap_err();
    assert_eq!(status, 409);
    assert!(why.contains("research task") && why.contains("tb attach"), "{why}");

    b.report("tb.attach", json!({"url": "https://claude.ai/artifact/abc", "title": "Queue comparison", "kind": "results"})).unwrap();
    b.report("tb.done", json!({"summary": "Picked one"})).unwrap();
    assert_eq!(board::get_task(&b.app, t).unwrap().s("status"), Some("done"));
}

#[test]
fn a_research_task_ships_no_pr_even_when_asked() {
    let b = new_board();
    let body = json!({"title": "Compare", "detail": "Do it.", "project": "webapp", "kind": "research", "ships_pr": "yes"});
    let (status, why) = b.post("/tasks", body).unwrap_err();
    assert_eq!(status, 400);
    assert!(why.contains("research task"), "{why}");
}

#[test]
fn a_task_can_become_research_and_back() {
    let b = new_board();
    let t = b.task("Look at queues", json!({}));
    b.post(&format!("tasks/T{t}"), json!({"kind": "research"})).unwrap();
    assert_eq!(board::get_task(&b.app, t).unwrap().i("ships_pr"), Some(0));
    b.post(&format!("tasks/T{t}"), json!({"kind": "none"})).unwrap();
    let row = board::get_task(&b.app, t).unwrap();
    assert_eq!((row.s("kind"), row.i("ships_pr")), (None, None));
}

#[test]
fn build_tasks_in_a_goal_do_not_wait_for_its_research() {
    let b = new_board();
    let g = b.post("/goals", json!({"name": "Queues", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    b.task("Compare queue libraries", json!({"kind": "research", "goal_id": g}));
    let build = b.task("Build it", json!({"goal_id": g}));
    assert_eq!(board::get_task(&b.app, build).unwrap().s("waits_for"), None);
}

#[test]
fn research_tasks_can_be_listed_on_their_own() {
    let b = new_board();
    let r = b.task("Compare queue libraries", json!({"kind": "research"}));
    b.task("Build it", json!({}));
    let mut q = Query::new();
    q.insert("kind".to_string(), "research".to_string());
    let v = api::dispatch(&b.app, "GET", "/tasks", &q, &json!({})).unwrap();
    let ids: Vec<i64> = v["tasks"].as_array().unwrap().iter().filter_map(|t| t["id"].as_i64()).collect();
    assert_eq!(ids, vec![r]);
}

#[test]
fn the_owners_ask_for_research_gets_the_note_and_a_board_prompt_does_not() {
    let b = new_board();
    let repo = b.dir.path().join("webapp");
    midna::sync(&b.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    let said = |prompt: &str| b.report("hook.prompt", json!({"prompt": prompt})).unwrap()["context"].as_str().unwrap_or("").to_string();
    assert!(said("Can you research queue libraries?").contains("--kind research"));
    assert!(!said("Fix the login button").contains("--kind research"));
}
