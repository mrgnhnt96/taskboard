//! The home page (`GET /home`): a goal shows the wave it's on and what's still open from earlier
//! waves; planned and finished work stays off it; the alerts come with their task.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, dispatch, midna, reports};

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
    Board { app, dir }
}

impl Board {
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn set(&self, id: i64, f: Vec<(&str, Value)>) {
        board::update_task(&self.app, id, f).unwrap();
    }
    fn home(&self) -> Value {
        self.get("home")
    }
}

fn refs(cards: &Value) -> Vec<String> {
    cards.as_array().unwrap().iter().map(|c| c["ref"].as_str().unwrap().to_string()).collect()
}

/// A goal with tasks in waves 1, 1, 2, 2 and 3, started.
fn goal(b: &Board) -> (i64, Vec<i64>) {
    midna::sync(&b.app, &[json!({"id": "s1", "name": "Planner", "agent": "claude", "cwd": b.dir.path().join("webapp").to_string_lossy(), "status": {"state": "idle"}})], &[]).unwrap();
    let g = b.post("/goals", json!({"name": "Login", "project": "webapp"}))["id"].as_i64().unwrap();
    let r = reports::handle(&b.app, json!({"event": "tb.propose", "session": "s1", "goal": format!("G{g}"),
        "tasks": ["Model::add it::1", "Form::build it::1", "Wire::connect::2", "Errors::show them::2", "Docs::write::3"]}), false).unwrap();
    let ids: Vec<i64> = r["created"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()[1..].parse().unwrap()).collect();
    (g, ids)
}

#[test]
fn a_planned_goal_stays_off_home() {
    let b = new_board();
    goal(&b);
    assert_eq!(b.home()["goals"], json!([]));
}

#[test]
fn a_goal_shows_its_wave_and_what_earlier_waves_left_open() {
    let b = new_board();
    let (g, t) = goal(&b);
    b.post(&format!("goals/G{g}/run"), json!({}));
    let h = b.home();
    let e = &h["goals"][0];
    assert_eq!(e["goal"]["ref"], format!("G{g}"));
    assert_eq!(e["wave"], 1, "nothing in flight: the first wave with a task queued");
    assert_eq!(e["waves"], 3);
    assert_eq!(e["now"], json!([]));
    assert_eq!(e["queued"], 2);

    // Wave 1: one task done and merged, one done with its PR still open; wave 2 working and queued.
    b.set(t[0], vec![("status", json!("done"))]);
    b.set(t[1], vec![("status", json!("done")), ("pr_num", json!(12)), ("pr_state", json!("OPEN"))]);
    b.set(t[2], vec![("status", json!("working"))]);
    b.set(t[4], vec![("status", json!("queued"))]);
    let e = &b.home()["goals"][0];
    assert_eq!(e["wave"], 2);
    assert_eq!(refs(&e["now"]), vec![format!("T{}", t[2])]);
    assert_eq!(e["queued"], 1, "wave 2's queued task; wave 3's isn't counted");
    assert_eq!(refs(&e["left"]), vec![format!("T{}", t[1])], "the open PR from wave 1");
    assert_eq!(e["left_waves"], json!([1]));

    // Its PR merges: wave 1 has nothing left.
    b.set(t[1], vec![("pr_state", json!("MERGED"))]);
    assert_eq!(b.home()["goals"][0]["left"], json!([]));
}

#[test]
fn a_finished_goal_leaves_home() {
    let b = new_board();
    let (g, t) = goal(&b);
    b.post(&format!("goals/G{g}/run"), json!({}));
    for id in &t {
        b.set(*id, vec![("status", json!("done"))]);
    }
    assert_eq!(b.home()["goals"], json!([]));
}

#[test]
fn tasks_outside_a_goal_get_their_projects_entry() {
    let b = new_board();
    let id = b.post("/tasks", json!({"title": "Fix the logo", "project": "webapp"}))["id"].as_i64().unwrap();
    b.set(id, vec![("status", json!("working"))]);
    let e = &b.home()["goals"][0];
    assert_eq!(e["goal"], Value::Null);
    assert_eq!(e["project"], "webapp");
    assert_eq!(refs(&e["now"]), vec![format!("T{id}")]);
}

#[test]
fn needs_carry_the_task_and_its_goal() {
    let b = new_board();
    let (g, t) = goal(&b);
    b.set(t[0], vec![("status", json!("needs")), ("question", json!("Keep the old form?"))]);
    dispatch::add_alert(&b.app, &format!("T{}: Keep the old form?", t[0]), Some(t[0]), Some(g), None, None).unwrap();
    let n = &b.home()["needs"][0];
    assert_eq!(n["task"], format!("T{}", t[0]));
    assert_eq!(n["card"]["title"], "Model");
    assert_eq!(n["card"]["status"], "needs");
    assert_eq!(n["goal_name"], "Login");
}

#[test]
fn a_task_that_needs_you_without_an_alert_still_shows() {
    let b = new_board();
    let (_, t) = goal(&b);
    b.set(t[1], vec![("status", json!("needs")), ("needs_reason", json!("question")), ("question", json!("Which icon?"))]);
    let n = &b.home()["needs"][0];
    assert_eq!(n["id"], Value::Null, "nothing to dismiss");
    assert_eq!(n["task"], format!("T{}", t[1]));
    assert_eq!(n["text"], "Which icon?");
    assert_eq!(n["goal_name"], "Login");
}
