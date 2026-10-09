//! `tb backlog task|drop|reopen`: only open issues can be made into tasks or dropped, several issues change
//! together or not at all, and the history credits the terminal that ran it.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::board;
use taskboardd::config::Config;

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
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
    Board { app, _dir: dir }
}

impl Board {
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
    fn issue(&self, title: &str) -> i64 {
        self.post("/backlog", json!({"title": title, "project": "webapp", "kind": "bug"}))["id"].as_i64().unwrap()
    }
    fn state(&self, id: i64) -> String {
        self.get(&format!("backlog/B{id}"))["state"].as_str().unwrap_or("").to_string()
    }
    fn task_count(&self) -> usize {
        self.get("tasks")["tasks"].as_array().map(|a| a.len()).unwrap_or(0)
    }
}

#[test]
fn a_dropped_or_promoted_issue_cant_be_promoted_or_dropped_again() {
    let b = new_board();
    let i = b.issue("Flaky test");
    b.post(&format!("backlog/B{i}/promote"), json!({"where": "board"}));
    let (code, msg) = b.post_err(&format!("backlog/B{i}/promote"), json!({"where": "board"}));
    assert_eq!(code, 409);
    assert!(msg.contains("already T"), "{msg}");
    assert_eq!(b.post_err(&format!("backlog/B{i}/drop"), json!({})).0, 409);

    let j = b.issue("Slow page");
    b.post(&format!("backlog/B{j}/drop"), json!({"reason": "dupe"}));
    let before = b.task_count();
    let (code, msg) = b.post_err(&format!("backlog/B{j}/promote"), json!({"where": "board"}));
    assert_eq!(code, 409);
    assert!(msg.contains("isn't open any more"), "{msg}");
    assert_eq!(b.post_err(&format!("backlog/B{j}/drop"), json!({})).0, 409);
    assert_eq!(b.task_count(), before);

    b.post(&format!("backlog/B{j}/reopen"), json!({}));
    b.post(&format!("backlog/B{j}/promote"), json!({"where": "board"}));
    assert_eq!(b.state(j), "task");
}

#[test]
fn several_issues_change_together_or_not_at_all() {
    let b = new_board();
    let (x, y, z) = (b.issue("One"), b.issue("Two"), b.issue("Three"));
    b.post(&format!("backlog/B{z}/drop"), json!({}));
    let before = b.task_count();
    let (code, _) = b.post_err("backlog/bulk", json!({"action": "task", "ids": [format!("B{x}"), format!("B{y}"), format!("B{z}")], "where": "board"}));
    assert_eq!(code, 409);
    assert_eq!((b.state(x), b.state(y)), ("open".to_string(), "open".to_string()));
    assert_eq!(b.task_count(), before);

    let r = b.post("backlog/bulk", json!({"action": "task", "ids": [format!("B{x}"), format!("B{y}")], "where": "board", "who": "terminal abcd1234"}));
    assert_eq!(r["count"], 2);
    assert_eq!(r["tasks"].as_array().unwrap().len(), 2);

    let (code, _) = b.post_err("backlog/bulk", json!({"action": "reopen", "ids": [format!("B{z}"), format!("B{x}"), "B999"]}));
    assert_eq!(code, 404);
    assert_eq!(b.state(z), "drop");

    // Reopening one that's already open names it.
    let w = b.issue("Four");
    let (code, msg) = b.post_err("backlog/bulk", json!({"action": "reopen", "ids": [format!("B{z}"), format!("B{w}")]}));
    assert_eq!((code, msg), (409, format!("B{w} is already open, so nothing changed.")));
    assert_eq!(b.state(z), "drop");

    // Several move together.
    let g = b.post("goals", json!({"name": "Later", "project": "web"}))["id"].as_i64().unwrap();
    let r = b.post("backlog/bulk", json!({"action": "move", "ids": [format!("B{z}"), format!("B{w}")], "goal_id": format!("G{g}")}));
    assert_eq!(r["count"], 2);
}

#[test]
fn history_credits_the_terminal_that_ran_it() {
    let b = new_board();
    let i = b.issue("Flaky test");
    let who = json!("terminal abcd1234");
    b.post("backlog/bulk", json!({"action": "drop", "ids": [format!("B{i}")], "reason": "dupe", "who": who}));
    b.post("backlog/bulk", json!({"action": "reopen", "ids": [format!("B{i}")], "who": who}));
    let r = b.post("backlog/bulk", json!({"action": "task", "ids": [format!("B{i}")], "where": "board", "who": who}));
    let hist = board::issue_history(&b.app, i).unwrap();
    let by: Vec<(&str, &str)> = hist.iter().map(|e| (e["kind"].as_str().unwrap(), e["who"].as_str().unwrap())).collect();
    assert!(by.contains(&("drop", "terminal abcd1234")), "{by:?}");
    assert!(by.contains(&("note", "terminal abcd1234")), "{by:?}");
    assert!(by.contains(&("task", "terminal abcd1234")), "{by:?}");
    let t = &r["tasks"][0];
    assert_eq!(t["origin"]["by"], "terminal abcd1234");

    let j = b.issue("Owner drops this");
    b.post(&format!("backlog/B{j}/drop"), json!({}));
    let hist = board::issue_history(&b.app, j).unwrap();
    assert_eq!(hist.last().unwrap()["who"], board::OWNER);
}
