//! `tb submit` on a goal's last open task checks the goal's done-when first (GitHub #334): it refuses while a
//! miss on the done-when is open, and asks once to check the done-when on each platform it names.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, reports};

const OUTCOME: &str = "Sign-in works on iOS and Android";

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
    goal: i64,
    task: i64,
}

/// A goal with a done-when and one task in it that needs no PR, working in terminal s1.
fn new_board(outcome: &str) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    let goal = api::dispatch(&app, "POST", "/goals", &Query::new(), &json!({"name": "Login", "project": "webapp", "outcome": outcome})).unwrap()["id"].as_i64().unwrap();
    let b = Board { app, _dir: dir, goal, task: 0 };
    let task = b.add_task("QA sign-in", "working");
    board::update_task(&b.app, task, vec![("session_id", json!("s1"))]).unwrap();
    Board { task, ..b }
}

impl Board {
    fn add_task(&self, title: &str, status: &str) -> i64 {
        let id = api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &json!({"title": title, "project": "webapp", "ships_pr": false})).unwrap()["id"].as_i64().unwrap();
        board::update_task(&self.app, id, vec![("goal_id", json!(self.goal)), ("status", json!(status))]).unwrap();
        id
    }
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut body = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        reports::handle(&self.app, body, false).map_err(|e| (e.status, e.message))
    }
    fn submit(&self, extra: Value) -> Result<Value, (u16, String)> {
        let mut body = json!({"summary": "Checked sign-in"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.report("tb.done", body)
    }
    fn miss(&self, title: &str) -> i64 {
        let v = self.report("tb.found", json!({"title": title, "kind": "gap", "misses_goal": true})).unwrap();
        v["issue"].as_str().unwrap()[1..].parse().unwrap()
    }
    fn status(&self, id: i64) -> String {
        board::get_task(&self.app, id).unwrap().st("status").to_string()
    }
}

#[test]
fn the_last_task_is_asked_once_to_check_the_done_when_per_platform() {
    let b = new_board(OUTCOME);
    b.add_task("Build sign-in", "done");

    let (code, why) = b.submit(json!({})).unwrap_err();
    assert_eq!(code, 409, "{why}");
    assert!(why.contains("last open task") && why.contains(OUTCOME) && why.contains("each platform") && why.contains("--misses-goal"), "{why}");
    assert_eq!(b.status(b.task), "working");

    b.submit(json!({})).unwrap();
    assert_eq!(b.status(b.task), "done");
}

#[test]
fn a_task_with_others_still_open_or_a_goal_with_no_done_when_isnt_asked() {
    let b = new_board(OUTCOME);
    b.add_task("Build sign-in", "queued");
    b.submit(json!({})).unwrap();
    assert_eq!(b.status(b.task), "done");

    let b = new_board("");
    b.submit(json!({})).unwrap();
    assert_eq!(b.status(b.task), "done");
}

#[test]
fn an_open_miss_holds_the_last_task_until_it_is_settled() {
    let b = new_board(OUTCOME);
    let miss = b.miss("Sign-in fails on Android");

    for _ in 0..2 {
        let (code, why) = b.submit(json!({})).unwrap_err();
        assert_eq!(code, 409, "{why}");
        assert!(why.contains(&format!("B{miss}")) && why.contains("Sign-in fails on Android") && why.contains("with the owner"), "{why}");
    }
    assert_eq!(b.status(b.task), "working");

    api::dispatch(&b.app, "POST", &format!("backlog/B{miss}/drop"), &Query::new(), &json!({})).unwrap();
    let (_, why) = b.submit(json!({})).unwrap_err();
    assert!(why.contains(OUTCOME), "once the miss is settled it's asked to check the done-when: {why}");
    b.submit(json!({})).unwrap();
    assert_eq!(b.status(b.task), "done");
}

#[test]
fn a_miss_made_into_another_task_names_it_and_one_made_into_this_task_doesnt_hold_it() {
    let b = new_board(OUTCOME);
    let miss = b.miss("Sign-in fails on Android");
    let fix = b.add_task("Fix Android sign-in", "done");
    b.app.db.update("issues", &json!(miss), taskboardd::fields!["state" => "task", "task_id" => fix]).unwrap();
    board::update_task(&b.app, fix, vec![("failed", json!(1))]).unwrap();

    let (code, why) = b.submit(json!({})).unwrap_err();
    assert_eq!(code, 409, "{why}");
    assert!(why.contains(&format!("made into T{fix}")) && why.contains("wait-for"), "{why}");

    b.app.db.update("issues", &json!(miss), taskboardd::fields!["task_id" => b.task]).unwrap();
    board::update_task(&b.app, fix, vec![("failed", json!(0))]).unwrap();
    let (_, why) = b.submit(json!({})).unwrap_err();
    assert!(why.contains(OUTCOME) && !why.contains(&format!("B{miss}")), "{why}");
}

#[test]
fn a_last_task_that_fixes_the_miss_can_finish() {
    let b = new_board(OUTCOME);
    let miss = b.miss("Sign-in fails on Android");
    let fixes = json!({"fixes": [format!("B{miss}")]});
    let (_, why) = b.submit(fixes.clone()).unwrap_err();
    assert!(why.contains(OUTCOME) && !why.contains("still open"), "{why}");
    b.submit(fixes).unwrap();
    assert_eq!(b.status(b.task), "done");
}
