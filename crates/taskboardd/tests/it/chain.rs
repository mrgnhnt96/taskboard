//! Goals that run after other goals (`tb goal set G2 --after G1`): held until the goal before is
//! finished, then run by the board on their own.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, fields, midna, reports, waitsfor};

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
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Planner", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "idle"}})], &[]).unwrap();
    Board { app, dir }
}

impl Board {
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| e.message)
    }
    /// A goal with two planned tasks.
    fn goal(&self, name: &str) -> (i64, Vec<i64>) {
        let _ = &self.dir;
        let g = self.post("/goals", json!({"name": name, "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
        let r = reports::handle(&self.app, json!({"event": "tb.propose", "session": "s1", "goal": format!("G{g}"), "tasks": ["One::do it", "Two::do it too"]}), false).unwrap();
        (g, r["created"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()[1..].parse().unwrap()).collect())
    }
    fn run(&self, g: i64) {
        api::dispatch(&self.app, "POST", &format!("goals/G{g}/run"), &[(api::FROM.to_string(), "app".to_string())].into_iter().collect::<Query>(), &json!({})).unwrap();
    }
    fn status(&self, id: i64) -> String {
        board::get_task(&self.app, id).unwrap().st("status")
    }
    fn done(&self, id: i64) {
        board::update_task(&self.app, id, fields!["status" => "done"]).unwrap();
    }
}

#[test]
fn a_chained_goal_runs_once_the_goal_before_it_finishes() {
    let b = new_board();
    let (g1, t1) = b.goal("First");
    let (g2, t2) = b.goal("Second");
    b.post(&format!("goals/G{g2}"), json!({"after": format!("G{g1}")})).unwrap();
    assert_eq!(b.get(&format!("goals/G{g2}"))["after"], json!([format!("G{g1}")]));

    b.run(g1);
    assert_eq!(b.status(t1[0]), "queued");
    assert_eq!(b.status(t2[0]), "planned", "the chained goal waits for the first to finish");

    b.done(t1[0]);
    assert_eq!(b.status(t2[0]), "planned", "one task done isn't the goal finished");
    b.done(t1[1]);
    assert_eq!(b.status(t2[0]), "queued", "the first goal finished, so the board ran the second");
    assert_eq!(b.status(t2[1]), "queued");
    assert!(b.get(&format!("goals/G{g2}"))["after_blocker"].is_null());
}

#[test]
fn a_chained_goal_run_early_holds_its_tasks() {
    let b = new_board();
    let (g1, t1) = b.goal("First");
    let (g2, t2) = b.goal("Second");
    b.post(&format!("goals/G{g2}"), json!({"after": [format!("G{g1}")]})).unwrap();
    b.run(g2);
    assert_eq!(b.status(t2[0]), "queued");
    let why = waitsfor::waiting_line(&b.app, &board::get_task(&b.app, t2[0]).unwrap()).unwrap();
    assert!(why.as_str().unwrap_or("").contains(&format!("runs after G{g1}")), "{why}");
    b.done(t1[0]);
    b.done(t1[1]);
    let why = waitsfor::waiting_line(&b.app, &board::get_task(&b.app, t2[0]).unwrap()).unwrap();
    assert!(!why.as_str().unwrap_or("").contains("runs after"), "{why}");
}

#[test]
fn a_chain_refuses_itself_and_a_loop_and_clears_with_none() {
    let b = new_board();
    let (g1, _) = b.goal("First");
    let (g2, _) = b.goal("Second");
    assert!(b.post(&format!("goals/G{g1}"), json!({"after": format!("G{g1}")})).unwrap_err().contains("itself"));
    b.post(&format!("goals/G{g2}"), json!({"after": format!("G{g1}")})).unwrap();
    assert!(b.post(&format!("goals/G{g1}"), json!({"after": format!("G{g2}")})).unwrap_err().contains("wait forever"));
    b.post(&format!("goals/G{g2}"), json!({"after": "none"})).unwrap();
    assert_eq!(b.get(&format!("goals/G{g2}"))["after"], json!([]));
}

#[test]
fn a_task_that_waited_for_start_runs_with_the_chained_goal_it_joined() {
    let b = new_board();
    let (g1, t1) = b.goal("First");
    let (g2, _) = b.goal("Second");
    b.post(&format!("goals/G{g2}"), json!({"after": format!("G{g1}")})).unwrap();
    let id = b.post("/tasks", json!({"title": "Turn reviews back on", "detail": "Do it.", "project": "webapp", "pickup": "manual"})).unwrap()["id"].as_i64().unwrap();
    b.post(&format!("tasks/T{id}"), json!({"goal_id": g2})).unwrap();
    assert_eq!(b.status(id), "planned");

    b.run(g1);
    b.done(t1[0]);
    b.done(t1[1]);
    let t = board::get_task(&b.app, id).unwrap();
    assert_eq!((t.s("status"), t.s("pickup")), (Some("queued"), Some("queue")), "it runs with its goal, not on a Start");
    let why = waitsfor::waiting_line(&b.app, &t).unwrap();
    assert!(!why.as_str().unwrap_or("").contains("press Start"), "{why}");
}
