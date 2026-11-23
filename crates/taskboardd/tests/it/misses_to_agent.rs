//! A miss on a goal's done-when that an agent can fix becomes a queued task in its goal instead of waiting
//! on the owner's Needs you list, and the hand-off is recorded for the Handled for you list. One marked as the
//! owner's call (`--needs-owner`) still goes to the owner.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, dispatch, midna, misses, p, reports};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
    goal: i64,
}

/// A goal with one working task in terminal s1, on a board that hands misses to agents.
fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.backlog.misses_to_agent = true;
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    let post = |path: &str, body: Value| api::dispatch(&app, "POST", path, &Query::new(), &body).unwrap();
    let goal = post("/goals", json!({"name": "Login", "project": "webapp", "outcome": "Signs in on every platform"}))["id"].as_i64().unwrap();
    let task = post("/tasks", json!({"title": "Sign in", "project": "webapp"}))["id"].as_i64().unwrap();
    board::update_task(&app, task, vec![("goal_id", json!(goal)), ("status", json!("working")), ("session_id", json!("s1"))]).unwrap();
    Board { app, _dir: dir, goal }
}

impl Board {
    fn found(&self, extra: Value) -> Result<Value, (u16, String)> {
        let mut body = json!({"event": "tb.found", "session": "s1", "title": "Sign-in fails on Android", "kind": "gap", "misses_goal": true});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        reports::handle(&self.app, body, false).map_err(|e| (e.status, e.message))
    }
    fn issue(&self, v: &Value) -> i64 {
        v["issue"].as_str().map(|s| s[1..].parse().unwrap()).unwrap_or_else(|| panic!("no issue in {v}"))
    }
    fn miss_alerts(&self) -> Vec<String> {
        dispatch::alerts(&self.app).iter().filter(|a| a["key"].as_str().is_some_and(board::is_miss_key)).filter_map(|a| a["text"].as_str().map(str::to_string)).collect()
    }
    fn handled(&self, task: i64) -> Vec<Value> {
        self.app.db.q("SELECT data FROM events WHERE task_id = ? AND kind = 'handled'", p![task]).unwrap().iter().map(|r| serde_json::from_str(&r.st("data")).unwrap()).collect()
    }
}

#[test]
fn a_miss_an_agent_can_fix_becomes_a_queued_task_in_its_goal() {
    let b = new_board();
    let v = b.found(json!({})).unwrap();
    assert!(v["held"].as_str().unwrap_or("").contains("An agent fixes it"), "{v}");
    let iid = b.issue(&v);
    let i = board::get_issue(&b.app, iid).unwrap();
    assert_eq!(i.s("state"), Some("task"), "{i:?}");
    let t = board::get_task(&b.app, i.i("task_id").unwrap()).unwrap();
    assert_eq!((t.s("status"), t.i("goal_id"), t.st("title").as_str()), (Some("queued"), Some(b.goal), "Sign-in fails on Android"));
    assert!(b.miss_alerts().is_empty(), "nothing on Needs you: {:?}", b.miss_alerts());
    assert_eq!(board::open_misses(&b.app, b.goal).unwrap(), 1, "the goal stays open until the task is done");
    let h = b.handled(t.id());
    assert_eq!(h.len(), 1, "{h:?}");
    assert_eq!((h[0]["by"].as_str(), h[0]["item"].as_str(), h[0]["issue"].as_i64()), (Some("misses"), Some("miss"), Some(iid)), "{h:?}");
}

#[test]
fn a_miss_that_needs_the_owners_call_stays_on_needs_you() {
    let b = new_board();
    let v = b.found(json!({"needs_owner": true})).unwrap();
    assert!(v["held"].is_null(), "{v}");
    let i = board::get_issue(&b.app, b.issue(&v)).unwrap();
    assert_eq!((i.s("state"), i.i("needs_owner")), (Some("open"), Some(1)));
    assert!(b.miss_alerts().iter().any(|a| a.contains("Sign-in fails on Android")), "{:?}", b.miss_alerts());
    assert!(misses::sweep(&b.app).unwrap().is_empty(), "the sweep leaves it with the owner");

    let (code, why) = b.found(json!({"title": "Slow login", "misses_goal": false, "needs_owner": true})).unwrap_err();
    assert_eq!(code, 400, "{why}");
    assert!(why.contains("--misses-goal"), "{why}");
}

#[test]
fn the_sweep_hands_off_a_miss_left_on_needs_you_once_its_goal_runs_again() {
    let b = new_board();
    board::update_goal(&b.app, b.goal, vec![("paused", json!(1))]).unwrap();
    let iid = b.issue(&b.found(json!({})).unwrap());
    assert_eq!(board::get_issue(&b.app, iid).unwrap().s("state"), Some("open"), "a paused goal starts nothing");
    assert_eq!(b.miss_alerts().len(), 1);
    assert!(misses::sweep(&b.app).unwrap().is_empty(), "still paused");

    board::update_goal(&b.app, b.goal, vec![("paused", json!(0))]).unwrap();
    let made = misses::sweep(&b.app).unwrap();
    assert_eq!(made.len(), 1);
    assert_eq!(board::get_issue(&b.app, iid).unwrap().i("task_id"), Some(made[0]));
    assert!(b.miss_alerts().is_empty(), "{:?}", b.miss_alerts());
}

#[test]
fn a_miss_that_came_back_from_an_agent_stays_with_the_owner() {
    let b = new_board();
    let iid = b.issue(&b.found(json!({})).unwrap());
    api::dispatch(&b.app, "POST", &format!("backlog/B{iid}/reopen"), &Query::new(), &json!({})).unwrap();
    assert_eq!(b.miss_alerts().len(), 1, "reopening puts it back on Needs you");
    assert!(misses::sweep(&b.app).unwrap().is_empty(), "the board hands a miss to an agent once");
    assert_eq!(board::get_issue(&b.app, iid).unwrap().s("state"), Some("open"));
}
