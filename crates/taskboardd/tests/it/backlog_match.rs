//! A new task that isn't made from a backlog item is refused when an open item in its goal (or its project,
//! outside any goal) names the same work, word for word or reworded; the refusal names each item and the
//! command that takes it, and `not_backlog` (`--not-backlog`) makes it anyway.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::ops::titles_match;
use taskboardd::{midna, reports};

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
    let b = Board { app, _dir: dir };
    let repo = b._dir.path().join("webapp").to_string_lossy().to_string();
    midna::sync(&b.app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
    b
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn issue(&self, title: &str, goal: Option<i64>) -> i64 {
        self.post("/backlog", json!({"title": title, "project": "webapp", "kind": "bug", "goal_id": goal}))["id"].as_i64().unwrap()
    }
    fn task_count(&self) -> usize {
        api::dispatch(&self.app, "GET", "tasks", &Query::new(), &json!({})).unwrap()["tasks"].as_array().map(|a| a.len()).unwrap_or(0)
    }
    fn new_task(&self, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": "tb.new_task", "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
}

#[test]
fn titles_match_the_same_or_reworded_work_only() {
    assert!(titles_match("Fix the login crash", "fix the  Login crash!"));
    assert!(titles_match("Refuse a new task that matches an open backlog item", "Board refuses new tasks matching backlog items"));
    assert!(!titles_match("Fix the login crash", "Fix the login timeout"), "two shared words aren't enough");
    assert!(!titles_match("Add dark mode to settings", "Refuse a new task that matches an open backlog item"));
    assert!(!titles_match("", ""));
}

#[test]
fn an_exact_match_is_refused_with_the_item_and_the_command() {
    let b = new_board();
    let i = b.issue("Header clips on narrow windows", None);
    let before = b.task_count();
    let (status, msg) = b.new_task(json!({"title": "Header clips on narrow windows"})).unwrap_err();
    assert_eq!(status, 409);
    assert!(msg.contains(&format!("B{i} “Header clips on narrow windows”")), "{msg}");
    assert!(msg.contains(&format!("tb task new --issue B{i}")), "{msg}");
    assert!(msg.contains("--not-backlog"), "{msg}");
    assert!(msg.contains("tb search \"<words>\""), "{msg}");
    assert_eq!(b.task_count(), before, "nothing was made");
    let (status, _) = b.new_task(json!({"title": "Header clips on narrow windows", "here": true})).unwrap_err();
    assert_eq!(status, 409, "--here is checked too");
}

#[test]
fn a_reworded_title_in_the_goal_is_refused_and_names_the_goal() {
    let b = new_board();
    let g = b.post("/goals", json!({"name": "Search", "project": "webapp"}))["id"].as_i64().unwrap();
    let i = b.issue("Search results load slowly on large projects", Some(g));
    let (status, msg) = b.new_task(json!({"title": "Slow search results loading for large projects", "goal": format!("G{g}")})).unwrap_err();
    assert_eq!(status, 409);
    assert!(msg.contains(&format!("tb task new --issue B{i} --goal G{g}")), "{msg}");
    let proposed = b.new_task(json!({"event": "tb.propose", "goal": format!("G{g}"), "tasks": ["Search results load slowly on large projects::x"]}));
    assert_eq!(proposed.unwrap_err().0, 409, "a proposed task list is checked too");
}

#[test]
fn unrelated_work_and_other_places_are_not_refused() {
    let b = new_board();
    let g = b.post("/goals", json!({"name": "Search", "project": "webapp"}))["id"].as_i64().unwrap();
    b.issue("Search results load slowly on large projects", Some(g));
    let closed = b.issue("Footer overlaps the sidebar", None);
    b.post(&format!("backlog/B{closed}/drop"), json!({"reason": "dupe"}));
    assert!(b.new_task(json!({"title": "Add dark mode to settings"})).is_ok());
    assert!(b.new_task(json!({"title": "Search results load slowly on large projects"})).is_ok(), "an item in a goal doesn't block a task outside it");
    assert!(b.new_task(json!({"title": "Footer overlaps the sidebar"})).is_ok(), "a closed item doesn't block");
}

#[test]
fn not_backlog_or_naming_the_item_makes_it() {
    let b = new_board();
    let i = b.issue("Header clips on narrow windows", None);
    let r = b.new_task(json!({"title": "Header clips on narrow windows", "not_backlog": true})).unwrap();
    assert_eq!(r["created"].as_array().unwrap().len(), 1);
    let r = b.new_task(json!({"title": "Header clips on narrow windows", "issue": format!("B{i}")})).unwrap();
    assert_eq!(r["created"].as_array().unwrap().len(), 1, "taking the item is never refused");
    let j = b.issue("Sidebar flickers on resize", None);
    let made = b.post("/tasks", json!({"title": "Sidebar flickers on resize", "project": "webapp", "not_backlog": true}));
    assert!(made["id"].as_i64().is_some(), "the API takes not_backlog too");
    let e = api::dispatch(&b.app, "POST", "tasks", &Query::new(), &json!({"title": "Sidebar flickers on resize", "project": "webapp"})).unwrap_err();
    assert!(e.status == 409 && e.message.contains(&format!("B{j}")), "{}", e.message);
}
