//! A backlog item says whether it blocks its task's PR, its goal or nothing, in the command that files it
//! (`tb found`, `tb backlog add`); while the filer's PR is open it can't be left out.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, dispatch, midna, p, reports};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
    goal: i64,
    task: i64,
}

/// A goal with one task, working in terminal s1.
fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    let post = |path: &str, body: Value| api::dispatch(&app, "POST", path, &Query::new(), &body).unwrap();
    let goal = post("/goals", json!({"name": "Login", "project": "webapp"}))["id"].as_i64().unwrap();
    let task = post("/tasks", json!({"title": "Sign in", "project": "webapp"}))["id"].as_i64().unwrap();
    board::update_task(&app, task, vec![("goal_id", json!(goal)), ("status", json!("working")), ("session_id", json!("s1"))]).unwrap();
    Board { app, _dir: dir, goal, task }
}

impl Board {
    fn found(&self, extra: Value) -> Result<Value, (u16, String)> {
        let mut body = json!({"event": "tb.found", "session": "s1", "title": "Sign-in drops the session", "kind": "bug"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        reports::handle(&self.app, body, false).map_err(|e| (e.status, e.message))
    }
    fn add(&self, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", "backlog", &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
    fn open_pr(&self) {
        board::update_task(&self.app, self.task, vec![("pr_num", json!(12)), ("pr_state", json!("OPEN"))]).unwrap();
    }
    fn issue(&self, v: &Value) -> i64 {
        v["issue"].as_str().or(v["ref"].as_str()).map(|s| s[1..].parse().unwrap()).or(v["id"].as_i64()).unwrap_or_else(|| panic!("no issue in {v}"))
    }
    fn events(&self, iid: i64) -> Vec<String> {
        self.app.db.q("SELECT text FROM issue_events WHERE issue_id = ? AND kind = 'blocks' ORDER BY id", p![iid]).unwrap().iter().map(|r| r.st("text").to_string()).collect()
    }
    fn alerts(&self) -> Vec<String> {
        dispatch::alerts(&self.app).iter().filter_map(|a| a["text"].as_str().map(str::to_string)).collect()
    }
}

#[test]
fn found_while_the_pr_is_open_must_say_what_it_blocks() {
    let b = new_board();
    let v = b.found(json!({})).unwrap();
    let iid = b.issue(&v);
    assert_eq!(board::get_issue(&b.app, iid).unwrap().st("blocks"), "none", "no open PR: it defaults to none");

    b.open_pr();
    let (code, why) = b.found(json!({"title": "Tokens leak to the log"})).unwrap_err();
    assert_eq!(code, 400);
    assert!(why.contains("--blocks") && why.contains("PR is open"), "{why}");
    let (code, why) = b.found(json!({"title": "Tokens leak to the log", "blocks": "pr"})).unwrap_err();
    assert_eq!(code, 400);
    assert!(why.contains("--blocks-why"), "{why}");
    let (code, _) = b.found(json!({"title": "Tokens leak to the log", "blocks": "maybe"})).unwrap_err();
    assert_eq!(code, 400);
    let (code, why) = b.found(json!({"title": "Tokens leak to the log", "blocks": "pr", "misses_goal": true})).unwrap_err();
    assert_eq!(code, 400, "{why}");

    let v = b.found(json!({"title": "Tokens leak to the log", "blocks": "pr", "blocks_why": "src/auth.rs logs the bearer token"})).unwrap();
    assert_eq!(v["blocks"], "pr", "{v}");
    let iid = b.issue(&v);
    let i = board::get_issue(&b.app, iid).unwrap();
    assert_eq!((i.st("blocks").as_str(), i.st("blocks_why").as_str(), i.i("misses_goal")), ("pr", "src/auth.rs logs the bearer token", Some(0)));
    assert_eq!(b.events(iid), vec!["Blocks its task's PR: src/auth.rs logs the bearer token".to_string()]);
    let d = api::dispatch(&b.app, "GET", &format!("backlog/B{iid}"), &Query::new(), &json!({})).unwrap();
    assert_eq!((d["blocks"].as_str(), d["blocks_why"].as_str()), (Some("pr"), Some("src/auth.rs logs the bearer token")), "{d}");

    let v = b.found(json!({"title": "Old flaky test", "blocks": "none"})).unwrap();
    assert_eq!(board::get_issue(&b.app, b.issue(&v)).unwrap().st("blocks"), "none");
}

#[test]
fn misses_goal_is_blocks_goal_and_holds_the_goal_with_an_alert() {
    let b = new_board();
    let v = b.found(json!({"title": "Sign-in fails on Android", "kind": "gap", "misses_goal": true})).unwrap();
    assert_eq!(v["misses_goal"], true, "{v}");
    let iid = b.issue(&v);
    let i = board::get_issue(&b.app, iid).unwrap();
    assert_eq!((i.st("blocks").as_str(), i.st("blocks_why").as_str(), i.i("misses_goal"), i.i("goal_id")), ("goal", "Sign-in fails on Android", Some(1), Some(b.goal)));
    assert!(b.alerts().iter().any(|a| a.contains("misses its done-when") && a.contains("Sign-in fails on Android")), "{:?}", b.alerts());

    let v = b.found(json!({"title": "Sign-in fails on iOS", "blocks": "goal", "blocks_why": "Done-when: signs in on every platform"})).unwrap();
    let i = board::get_issue(&b.app, b.issue(&v)).unwrap();
    assert_eq!((i.st("blocks").as_str(), i.i("misses_goal"), i.i("goal_id")), ("goal", Some(1), Some(b.goal)));
    assert!(b.alerts().iter().any(|a| a.contains("Sign-in fails on iOS")), "{:?}", b.alerts());
}

#[test]
fn backlog_add_from_a_task_terminal_says_what_it_blocks() {
    let b = new_board();
    b.open_pr();
    let (code, why) = b.add(json!({"title": "Retry storms on 500", "kind": "follow", "project": "webapp", "session": "s1"})).unwrap_err();
    assert_eq!(code, 400);
    assert!(why.contains("--blocks"), "{why}");

    let v = b.add(json!({"title": "Retry storms on 500", "kind": "follow", "project": "webapp", "session": "s1", "blocks": "goal", "blocks_why": "Done-when: errors show once"})).unwrap();
    let iid = b.issue(&v);
    let i = board::get_issue(&b.app, iid).unwrap();
    assert_eq!((i.st("blocks").as_str(), i.st("blocks_why").as_str(), i.i("goal_id"), i.i("misses_goal")), ("goal", "Done-when: errors show once", Some(b.goal), Some(1)));
    assert_eq!(b.events(iid), vec!["Blocks its goal: Done-when: errors show once".to_string()]);
    assert_eq!(v["blocks"], "goal", "{v}");

    let v = b.add(json!({"title": "Crash on empty form", "project": "webapp", "session": "s1", "blocks": "pr", "blocks_why": "form.rs unwraps None"})).unwrap();
    assert_eq!(board::get_issue(&b.app, b.issue(&v)).unwrap().st("blocks"), "pr");

    // Filed from no task (the owner's own terminal): no PR to hold, and nothing required.
    let (code, why) = b.add(json!({"title": "Crash on reload", "project": "webapp", "blocks": "pr", "blocks_why": "x"})).unwrap_err();
    assert_eq!(code, 400, "{why}");
    let v = b.add(json!({"title": "Crash on reload", "project": "webapp"})).unwrap();
    assert_eq!(board::get_issue(&b.app, b.issue(&v)).unwrap().st("blocks"), "none");
}

#[test]
fn a_settled_item_stops_saying_it_blocks_and_reopening_brings_it_back() {
    let b = new_board();
    let iid = b.issue(&b.found(json!({"title": "Sign-in fails on iOS", "blocks": "goal", "blocks_why": "Done-when: signs in on every platform"})).unwrap());
    let card = || board::issue_dict(&b.app, &board::get_issue(&b.app, iid).unwrap()).unwrap();
    let post = |path: String| api::dispatch(&b.app, "POST", &path, &Query::new(), &json!({})).unwrap();
    assert_eq!((card()["blocks"].as_str(), card()["blocks_why"].as_str()), (Some("goal"), Some("Done-when: signs in on every platform")));

    post(format!("backlog/B{iid}/drop"));
    assert_eq!((card()["blocks"].as_str(), card()["blocks_why"].as_str()), (Some("none"), None), "closed as won't do");
    assert_eq!(board::open_misses(&b.app, b.goal).unwrap(), 0);

    post(format!("backlog/B{iid}/reopen"));
    assert_eq!((card()["blocks"].as_str(), card()["blocks_why"].as_str()), (Some("goal"), Some("Done-when: signs in on every platform")));
    assert_eq!(board::open_misses(&b.app, b.goal).unwrap(), 1);
}

#[test]
fn an_item_made_into_a_task_blocks_until_that_task_is_done() {
    let b = new_board();
    let iid = b.issue(&b.found(json!({"title": "Sign-in fails on iOS", "blocks": "goal", "blocks_why": "Done-when: signs in on every platform"})).unwrap());
    b.app.db.update("issues", &json!(iid), taskboardd::fields!["state" => "task", "task_id" => b.task]).unwrap();
    let blocks = || board::issue_card(&b.app, &board::get_issue(&b.app, iid).unwrap()).unwrap()["blocks"].clone();
    assert_eq!(blocks(), "goal");

    board::update_task(&b.app, b.task, vec![("status", json!("done")), ("failed", json!(1))]).unwrap();
    assert_eq!(blocks(), "goal", "a failed task settles nothing");

    board::update_task(&b.app, b.task, vec![("failed", json!(0))]).unwrap();
    assert_eq!(blocks(), "none");
}
