//! Finishing a goal on the owner's word with failed tasks in it (`POST /goals/:id/accept`, `tb goal done`):
//! the failed tasks stay failed and count as failed, but those ones stop holding the goal open.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, fields};

fn new_board() -> (tempfile::TempDir, Arc<App>) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    (dir, app)
}

fn post(app: &App, path: &str, body: Value) -> Result<Value, String> {
    api::dispatch(app, "POST", path, &Query::new(), &body).map_err(|e| e.message)
}

fn get(app: &App, path: &str) -> Value {
    api::dispatch(app, "GET", path, &Query::new(), &json!({})).unwrap()
}

fn task(app: &App, g: i64, title: &str) -> i64 {
    post(app, "/tasks", json!({"title": title, "detail": "Do it.", "project": "webapp", "goal_id": g})).unwrap()["id"].as_i64().unwrap()
}

fn end(app: &App, t: i64, failed: bool) {
    app.db.update("tasks", &json!(t), fields!["status" => "done", "failed" => failed as i64, "finished_at" => "2026-10-20T10:00:00Z"]).unwrap();
}

/// G<n> with a measurement run that failed and the one that replaced it.
fn teardown(app: &App) -> (i64, i64, i64) {
    let g = post(app, "/goals", json!({"name": "Core teardown", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    (g, task(app, g, "Measure the teardown"), task(app, g, "Measure it again"))
}

#[test]
fn the_owner_finishes_a_goal_whose_failed_task_was_replaced() {
    let (_d, app) = new_board();
    let (g, miss, redo) = teardown(&app);
    end(&app, miss, true);
    let path = format!("/goals/{g}/accept");
    let e = post(&app, &path, json!({"why": "The rerun replaced it"})).unwrap_err();
    assert!(e.contains(&format!("T{redo}")), "a task still to do: {e}");
    end(&app, redo, false);
    assert!(!board::goal_finished(&app, g).unwrap());
    assert!(get(&app, &format!("/goals/{g}"))["state"].as_str().unwrap().starts_with("Outcome not met"));
    assert!(post(&app, &path, json!({})).is_err(), "it takes a why");

    let v = post(&app, &path, json!({"why": "The rerun replaced it"})).unwrap();
    assert!(board::goal_finished(&app, g).unwrap());
    assert_eq!(v["state"], "Done · 1 failed (accepted)");
    assert_eq!((v["done"].as_i64(), v["failed"].as_i64(), v["total"].as_i64()), (Some(1), Some(1), Some(2)), "the failed task never counts as done");
    assert!(v["finished_at"].is_string());
    assert_eq!(v["accepted"]["tasks"], json!([format!("T{miss}")]));
    assert_eq!(v["accepted"]["why"], "The rerun replaced it");
    let note = app.db.q1("SELECT text FROM goal_notes WHERE goal_id = ? AND kind = 'decision'", taskboardd::p![g]).unwrap().unwrap();
    assert!(note.get("text").unwrap().as_str().unwrap().contains(&format!("T{miss}")));
    assert!(post(&app, &path, json!({"why": "again"})).unwrap_err().contains("finished already"));
}

#[test]
fn a_task_that_fails_after_the_owners_word_holds_the_goal_open_again() {
    let (_d, app) = new_board();
    let (g, miss, redo) = teardown(&app);
    end(&app, miss, true);
    end(&app, redo, false);
    post(&app, &format!("/goals/{g}/accept"), json!({"why": "The rerun replaced it"})).unwrap();
    let late = task(&app, g, "One more run");
    end(&app, late, true);
    assert!(!board::goal_finished(&app, g).unwrap());
    assert!(get(&app, &format!("/goals/{g}"))["state"].as_str().unwrap().starts_with("Outcome not met: 1 failed"));
}

#[test]
fn a_pr_still_open_or_a_goal_already_finished_is_refused() {
    let (_d, app) = new_board();
    let (g, miss, redo) = teardown(&app);
    end(&app, miss, true);
    end(&app, redo, false);
    app.db.update("tasks", &json!(redo), fields!["pr_num" => 7, "pr_state" => "OPEN"]).unwrap();
    let path = format!("/goals/{g}/accept");
    assert!(post(&app, &path, json!({"why": "x"})).unwrap_err().contains("PRs still open"));

    let (g2, a, b) = teardown(&app);
    end(&app, a, false);
    end(&app, b, false);
    assert!(post(&app, &format!("/goals/{g2}/accept"), json!({"why": "x"})).unwrap_err().contains("finished already"));
}
