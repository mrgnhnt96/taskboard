//! A finished goal's retrospective: what it hands Claude, and how Claude's answer becomes lessons
//! without duplicating or double-counting the ones already on the board.

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, fields, retro};

fn new_board() -> (tempfile::TempDir, std::sync::Arc<App>) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    (dir, app)
}

fn call(app: &App, method: &str, path: &str, body: Value) -> Value {
    api::dispatch(app, method, path, &Query::new(), &body).unwrap()
}

/// A goal with two tasks: T1 planned, T2 added later after T1 failed a review and asked a question.
fn goal(app: &App) -> (i64, i64, i64) {
    let g = call(app, "POST", "/goals", json!({"name": "Ship login", "project": "webapp", "outcome": "Users can log in"}))["id"].as_i64().unwrap();
    let t1 = call(app, "POST", "/tasks", json!({"title": "Login form", "detail": "Do it.", "project": "webapp", "goal_id": g}))["id"].as_i64().unwrap();
    app.db.update("tasks", &json!(t1), fields!["started_at" => "2026-10-01T09:00:00", "start_tries" => 3, "human_min" => 120]).unwrap();
    let t2 = call(app, "POST", "/tasks", json!({"title": "Session cookie", "detail": "Do it.", "project": "webapp", "goal_id": g}))["id"].as_i64().unwrap();
    app.db.update("tasks", &json!(t2), fields!["created_at" => "2026-10-02T09:00:00"]).unwrap();
    board::log_event(app, t1, "agent", "question", "Should the form remember the email?").unwrap();
    board::log_event(app, t1, "owner", "answer", "Answered: yes").unwrap();
    board::log_event(app, t1, "board", "handoff", "Brought the conversation back for PR #7: checks failed").unwrap();
    app.db.insert(
        "events",
        fields!["task_id" => t1, "at" => "2026-10-01T10:00:00", "who" => "board", "kind" => "step",
                "data" => json!({"name": "Review", "ok": false, "result": {"findings": [{"id": "F1", "title": "Password logged in plain text", "severity": "high"}]}}).to_string()],
    )
    .unwrap();
    app.db.insert("issues", fields!["project" => "webapp", "kind" => "bug", "title" => "Logout leaves the cookie", "found_by_task" => t1, "state" => "open"]).unwrap();
    (g, t1, t2)
}

#[test]
fn the_dossier_shows_plan_against_what_happened() {
    let (_d, app) = new_board();
    let (g, _, _) = goal(&app);
    let d = retro::dossier(&app, g).unwrap();
    for want in [
        "GOAL G1 in webapp: Ship login",
        "Outcome: Users can log in",
        "T1 wave",
        "started 3 times",
        "~120 min",
        "Asked: Should the form remember the email?",
        "Answered: yes",
        "PR fix cycles: 1",
        "Password logged in plain text (high)",
        "T2 wave",
        "added after the goal started",
        "B1 [bug] found by T1",
        "(none yet)",
    ] {
        assert!(d.contains(want), "missing {want:?} in:\n{d}");
    }
}

#[test]
fn answers_become_suggested_lessons_or_confirm_existing_ones_once() {
    let (_d, app) = new_board();
    let (g, _, _) = goal(&app);
    let old = call(&app, "POST", "/lessons", json!({"text": "Never log passwords.", "kind": "code", "project": "webapp", "kept": true}));
    let answer = json!({"lessons": [
        {"action": "new", "kind": "planning", "scope": "project", "text": "Plan the session cookie with the login form.", "evidence": ["T2", "t1", "T99", "G7"]},
        {"action": "new", "kind": "planning", "scope": "project", "text": "plan the  session cookie with the login form.", "evidence": []},
        {"action": "new", "kind": "code", "scope": "project", "text": "never log  PASSWORDS.", "evidence": ["T1"]},
        {"action": "confirm", "lesson": 99, "kind": "code", "scope": "project", "text": "x", "evidence": []},
        {"action": "new", "kind": "process", "scope": "global", "text": "Ask about remembered fields before building forms.", "evidence": ["B1"]},
    ]});
    let v = retro::apply(&app, g, &answer).unwrap();
    assert_eq!(v["new"], json!(["L2", "L3"]), "{v}");
    assert_eq!(v["confirmed"], json!(["L1"]), "a repeat confirms the lesson already there");
    assert_eq!(v["skipped"], 2, "the duplicate and the unknown lesson");

    let l2 = call(&app, "GET", "/lessons/L2", json!({}));
    assert_eq!(l2["state"], "suggested");
    assert_eq!(l2["source"], "retro G1");
    assert_eq!(l2["evidence"], json!(["G1", "T2", "T1"]), "only this goal's refs");
    let l3 = call(&app, "GET", "/lessons/L3", json!({}));
    assert_eq!((l3["scope"].as_str(), l3["project"].as_str()), (Some("global"), None));
    assert_eq!(l3["evidence"], json!(["G1", "B1"]));
    let l1 = call(&app, "GET", &format!("/lessons/{}", old["ref"].as_str().unwrap()), json!({}));
    assert_eq!(l1["confirmations"], 1);
    assert_eq!(l1["evidence"], json!(["G1", "T1"]));

    let again = retro::apply(&app, g, &json!({"lessons": [{"action": "confirm", "lesson": 1, "kind": "code", "scope": "project", "text": "", "evidence": []}]})).unwrap();
    assert_eq!(again["confirmed"], json!([]), "a goal confirms a lesson once");
    assert_eq!(call(&app, "GET", "/lessons/L1", json!({}))["confirmations"], 1);
}

#[test]
fn retired_lessons_are_neither_confirmed_nor_suggested_again() {
    let (_d, app) = new_board();
    let (g, _, _) = goal(&app);
    call(&app, "POST", "/lessons", json!({"text": "Use one big task.", "kind": "planning", "project": "webapp"}));
    call(&app, "POST", "/lessons/L1/retire", json!({}));
    let v = retro::apply(&app, g, &json!({"lessons": [{"action": "new", "kind": "planning", "scope": "project", "text": "Use one big task.", "evidence": []}]})).unwrap();
    assert_eq!((v["new"].clone(), v["confirmed"].clone(), v["skipped"].clone()), (json!([]), json!([]), json!(1)));
    assert!(retro::dossier(&app, g).unwrap().contains("L1 [retired · planning · project, confirmed 0]: Use one big task."));
}

#[test]
fn running_it_by_hand_needs_claude() {
    let (_d, app) = new_board();
    let (g, _, _) = goal(&app);
    let e = api::dispatch(&app, "POST", &format!("/lessons/retro/G{g}"), &Query::new(), &json!({})).unwrap_err();
    assert_eq!(e.status, 409);
    assert_eq!(call(&app, "GET", &format!("/lessons/retro/G{g}"), json!({}))["state"], "none");
}
