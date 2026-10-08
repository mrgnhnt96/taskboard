//! Waves on a goal (`docs/parity/oct8-changes.md` #5): states, review stops, a failed task holding the
//! next wave, "Continue to wave N", and the runner waiting on them.

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
    Board { app, dir }
}

impl Board {
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn set(&self, id: i64, status: &str, failed: bool) {
        board::update_task(&self.app, id, fields!["status" => status, "failed" => failed as i64]).unwrap();
    }
    fn waves(&self, g: i64) -> Vec<Value> {
        self.get(&format!("goals/G{g}"))["waves"].as_array().cloned().unwrap()
    }
    fn waiting(&self, id: i64) -> Value {
        waitsfor::waiting_line(&self.app, &board::get_task(&self.app, id).unwrap()).unwrap()
    }
}

/// A goal planned through `tb propose` with two tasks in wave 1, one in wave 2 and one with no wave.
fn planned(b: &Board) -> (i64, Vec<i64>) {
    let _ = &b.dir;
    midna::sync(&b.app, &[json!({"id": "s1", "name": "Planner", "agent": "claude", "cwd": b.dir.path().join("webapp").to_string_lossy(), "status": {"state": "idle"}})], &[]).unwrap();
    let g = b.post("/goals", json!({"name": "Login", "project": "webapp", "run_in_order": true}))["id"].as_i64().unwrap();
    let r = reports::handle(&b.app, json!({"event": "tb.propose", "session": "s1", "goal": format!("G{g}"),
        "tasks": ["Model::add it::1", "Form::build it::1", "Wire::connect them::2", "Docs::write them"]}), false).unwrap();
    let ids: Vec<i64> = r["created"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()[1..].parse().unwrap()).collect();
    b.post(&format!("goals/G{g}/run"), json!({}));
    (g, ids)
}

#[test]
fn a_wave_waits_for_the_one_before_it() {
    let b = new_board();
    let (g, t) = planned(&b);
    assert_eq!(board::get_task(&b.app, t[2]).unwrap().i("wave"), Some(2), "title::detail::2 sets the wave");
    assert_eq!(board::get_task(&b.app, t[3]).unwrap().i("wave"), None);
    let w = b.waves(g);
    assert_eq!(w.iter().map(|w| w["state"].as_str().unwrap()).collect::<Vec<_>>(), vec!["ready", "waiting"]);
    assert_eq!(w[1]["hold"], "Waits for wave 1");
    assert!(b.waiting(t[1]).is_null() || !b.waiting(t[1]).as_str().unwrap().contains("Waits for"), "waves replace running in order: {}", b.waiting(t[1]));
    assert_eq!(b.waiting(t[2]), "Waits for wave 1");

    b.set(t[0], "working", false);
    assert_eq!(b.waves(g)[0]["state"], "running");
    b.set(t[0], "done", false);
    b.set(t[1], "done", false);
    let w = b.waves(g);
    assert_eq!(w[0]["state"], "done");
    assert_eq!(w[1]["state"], "ready");
    assert!(b.waiting(t[2]).is_null() || !b.waiting(t[2]).as_str().unwrap().contains("wave"));
}

#[test]
fn a_review_stop_holds_the_goal_until_the_owner_continues() {
    let b = new_board();
    let (g, t) = planned(&b);
    b.post(&format!("goals/G{g}/waves/1"), json!({"name": "Basics", "stop_after": true}));
    b.set(t[0], "done", false);
    b.set(t[1], "done", false);
    let w = b.waves(g);
    assert_eq!(w[0]["state"], "stopped");
    assert_eq!(w[0]["name"], "Basics");
    assert_eq!(b.waiting(t[2]), "Stopped for your review after wave 1 (Basics)");
    assert_eq!(b.get(&format!("goals/G{g}"))["stopped"], "Wave 1 (Basics) is done. Review it, then continue");

    let d = b.post(&format!("goals/G{g}/waves/1/continue"), json!({}));
    assert_eq!(d["waves"][0]["state"], "done");
    assert!(d["waves"][0]["released_at"].is_string());
    assert!(d["stopped"].is_null());
    assert_eq!(d["waves"][1]["state"], "ready");
}

#[test]
fn a_failed_task_holds_the_next_wave_until_the_owner_goes_on() {
    let b = new_board();
    let (g, t) = planned(&b);
    b.set(t[0], "done", true);
    b.set(t[1], "done", false);
    let w = b.waves(g);
    assert_eq!(w[0]["state"], "failed");
    assert_eq!(w[0]["failed"], json!([format!("T{}", t[0])]));
    assert_eq!(b.waiting(t[2]), format!("T{} in wave 1 failed. Try again, or continue past it", t[0]));
    b.post(&format!("goals/G{g}/waves/1/continue"), json!({"who": "Planner"}));
    assert_eq!(b.waves(g)[1]["state"], "ready");
}

#[test]
fn a_task_moves_between_waves() {
    let b = new_board();
    let (g, t) = planned(&b);
    b.post(&format!("tasks/T{}", t[3]), json!({"wave": 2}));
    assert_eq!(b.waves(g)[1]["total"], 2);
    b.post(&format!("tasks/T{}", t[2]), json!({"wave": "none"}));
    assert_eq!(b.get(&format!("tasks/T{}", t[2]))["wave"], Value::Null);
    let loose = b.post("/tasks", json!({"title": "Loose", "project": "webapp"}))["id"].as_i64().unwrap();
    let e = api::dispatch(&b.app, "POST", &format!("tasks/T{loose}"), &Query::new(), &json!({"wave": 1})).expect_err("no goal");
    assert!(e.message.contains("Only a task in a goal"));
}
