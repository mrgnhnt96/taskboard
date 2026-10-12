//! The Tasks page (`GET /lanes`): tasks outside goals in queued, running, needs-you and done lanes.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::board;
use taskboardd::config::Config;
use taskboardd::util::{iso, now_iso, now_ts};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    Board { app, _dir: dir }
}

impl Board {
    fn lanes(&self, goals: bool) -> Value {
        let q: Query = [("goals".to_string(), if goals { "1" } else { "0" }.to_string())].into_iter().collect();
        api::dispatch(&self.app, "GET", "lanes", &q, &json!({})).unwrap()
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap()
    }
    fn task(&self, title: &str, project: &str, f: Vec<(&str, Value)>) -> i64 {
        let id = self.post("/tasks", json!({"title": title, "project": project}))["id"].as_i64().unwrap();
        board::update_task(&self.app, id, f).unwrap();
        id
    }
}

fn refs(cards: &Value) -> Vec<String> {
    cards.as_array().unwrap().iter().map(|c| c["ref"].as_str().unwrap().to_string()).collect()
}

fn r(id: i64) -> String {
    format!("T{id}")
}

#[test]
fn each_status_gets_its_lane_and_planned_stays_off() {
    let b = new_board();
    let planned = b.task("Plan it", "webapp", vec![("status", json!("planned"))]);
    let queued = b.task("Queue it", "webapp", vec![("status", json!("queued"))]);
    let working = b.task("Work it", "api", vec![("status", json!("working"))]);
    let needs = b.task("Ask it", "webapp", vec![("status", json!("needs"))]);
    let done = b.task("Ship it", "api", vec![("status", json!("done")), ("finished_at", json!(now_iso()))]);
    let l = b.lanes(false);
    assert_eq!(refs(&l["queued"]), vec![r(queued)]);
    assert_eq!(refs(&l["running"]), vec![r(working)]);
    assert_eq!(refs(&l["needs"]), vec![r(needs)]);
    assert_eq!(refs(&l["done"]), vec![r(done)]);
    let all: Vec<String> = ["queued", "running", "needs", "done"].iter().flat_map(|k| refs(&l[*k])).collect();
    assert!(!all.contains(&r(planned)));
    assert_eq!(l["projects"], json!([{"name": "api", "count": 2}, {"name": "webapp", "count": 2}]));
}

#[test]
fn queued_runs_in_the_runners_order() {
    let b = new_board();
    let first = b.task("Older", "webapp", vec![("status", json!("queued")), ("created_at", json!(iso(now_ts() - 300.0)))]);
    let second = b.task("Newer", "webapp", vec![("status", json!("queued")), ("created_at", json!(iso(now_ts() - 100.0)))]);
    let urgent = b.task("Urgent", "webapp", vec![("status", json!("queued")), ("priority", json!("high")), ("created_at", json!(now_iso()))]);
    assert_eq!(refs(&b.lanes(false)["queued"]), vec![r(urgent), r(first), r(second)]);
}

#[test]
fn done_is_today_plus_open_prs() {
    let b = new_board();
    let old = iso(now_ts() - 3.0 * 86400.0);
    let today = b.task("Today", "webapp", vec![("status", json!("done")), ("finished_at", json!(now_iso()))]);
    b.task("Last week", "webapp", vec![("status", json!("done")), ("finished_at", json!(old.clone()))]);
    let open = b.task("Open PR", "webapp", vec![("status", json!("done")), ("finished_at", json!(old)), ("pr_num", json!(7)), ("pr_state", json!("OPEN"))]);
    assert_eq!(refs(&b.lanes(false)["done"]), vec![r(today), r(open)]);
}

#[test]
fn goal_tasks_show_only_when_asked() {
    let b = new_board();
    let g = b.post("/goals", json!({"name": "Login", "project": "webapp"}))["id"].as_i64().unwrap();
    let loose = b.task("Loose", "webapp", vec![("status", json!("queued"))]);
    let inside = b.task("Inside", "webapp", vec![("status", json!("queued")), ("goal_id", json!(g))]);
    assert_eq!(refs(&b.lanes(false)["queued"]), vec![r(loose)]);
    assert_eq!(refs(&b.lanes(true)["queued"]), vec![r(loose), r(inside)]);
}
