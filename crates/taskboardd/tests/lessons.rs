//! Lessons outlast their goal: suggested ones reach nobody, kept ones reach later handoffs in their
//! project (and every project, when global), and planning lessons reach planners instead.

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, handoff};

fn new_board() -> (tempfile::TempDir, std::sync::Arc<App>) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let projects: Vec<Value> = ["webapp", "api"]
        .iter()
        .map(|n| {
            let repo = dir.path().join(n);
            std::fs::create_dir_all(&repo).unwrap();
            json!({"name": n, "path": repo.to_string_lossy()})
        })
        .collect();
    app.db.set_setting("midna_projects", Some(&Value::Array(projects).to_string())).unwrap();
    (dir, app)
}

fn call(app: &App, method: &str, path: &str, body: Value) -> Value {
    api::dispatch(app, method, path, &Query::new(), &body).unwrap()
}

fn lesson(app: &App, body: Value) -> String {
    call(app, "POST", "/lessons", body)["ref"].as_str().unwrap().to_string()
}

fn task(app: &App, project: &str) -> i64 {
    call(app, "POST", "/tasks", json!({"title": "Fix it", "detail": "Do it.", "project": project}))["id"].as_i64().unwrap()
}

fn planner_prompt(app: &App, project: &str) -> String {
    let id = call(app, "POST", "/goals", json!({"name": "Ship login", "project": project}))["id"].as_i64().unwrap();
    api::goal_context(app, &board::get_goal(app, id).unwrap(), true).unwrap()
}

#[test]
fn only_kept_lessons_reach_handoffs_in_their_project() {
    let (_d, app) = new_board();
    let l = lesson(&app, json!({"text": "Build worktrees in their own target dir.", "kind": "tooling", "project": "webapp", "evidence": ["t12", "G3"]}));
    assert_eq!(l, "L1");
    let w = task(&app, "webapp");
    assert!(!handoff::build(&app, w).unwrap().contains("own target dir"), "a suggested lesson reaches nobody");

    let v = call(&app, "POST", &format!("/lessons/{l}/keep"), json!({}));
    assert_eq!(v["state"], "kept");
    assert_eq!(v["evidence"], json!(["T12", "G3"]));
    let h = handoff::build(&app, w).unwrap();
    assert!(h.contains("Lessons from earlier work here:\n- Build worktrees in their own target dir. (L1)"), "{h}");
    assert!(!handoff::build(&app, task(&app, "api")).unwrap().contains("own target dir"), "another project's lesson");

    call(&app, "POST", &format!("/lessons/{l}/retire"), json!({}));
    assert!(!handoff::build(&app, w).unwrap().contains("own target dir"), "a retired lesson");
}

#[test]
fn global_lessons_reach_every_project_and_planning_ones_reach_planners() {
    let (_d, app) = new_board();
    let g = lesson(&app, json!({"text": "Say the PR's risk in its first line.", "kind": "process", "scope": "global", "kept": true}));
    let p = lesson(&app, json!({"text": "Put migrations in their own task.", "kind": "planning", "project": "api", "kept": true}));
    for project in ["webapp", "api"] {
        let h = handoff::build(&app, task(&app, project)).unwrap();
        assert!(h.contains(&format!("Say the PR's risk in its first line. ({g})")), "{h}");
        assert!(!h.contains("Put migrations"), "planning lessons are for planners: {h}");
    }
    let prompt = planner_prompt(&app, "api");
    assert!(prompt.contains(&format!("What earlier goals taught about planning here:\n- Put migrations in their own task. ({p})")), "{prompt}");
    assert!(!planner_prompt(&app, "webapp").contains("Put migrations"));
    let q: Query = [("project".to_string(), "api".to_string())].into();
    let v = api::dispatch(&app, "GET", "/planning", &q, &json!({})).unwrap();
    assert_eq!(v["lessons"], json!([format!("- Put migrations in their own task. ({p})")]));
}

#[test]
fn lessons_are_confirmed_edited_and_listed() {
    let (_d, app) = new_board();
    let l = lesson(&app, json!({"text": "Run only the touched tests.", "kind": "process", "project": "webapp"}));
    let v = call(&app, "POST", &format!("/lessons/{l}/confirm"), json!({"evidence": ["T7", "T12"]}));
    assert_eq!(v["confirmations"], 1);
    assert_eq!(v["evidence"], json!(["T7", "T12"]));
    let v = call(&app, "POST", &format!("/lessons/{l}"), json!({"scope": "global", "evidence": ["T7", "G2"]}));
    assert_eq!(v["scope"], "global");
    assert_eq!(v["project"], Value::Null);
    assert_eq!(v["evidence"], json!(["T7", "T12", "G2"]));

    lesson(&app, json!({"text": "An api-only lesson.", "kind": "code", "project": "api"}));
    let q: Query = [("project".to_string(), "webapp".to_string())].into();
    let listed = api::dispatch(&app, "GET", "/lessons", &q, &json!({})).unwrap();
    assert_eq!(listed["lessons"].as_array().unwrap().len(), 1, "{listed}");

    for bad in [json!({"text": "", "kind": "code", "project": "webapp"}), json!({"text": "x", "kind": "vibes", "project": "webapp"}), json!({"text": "x", "kind": "code"})] {
        assert!(api::dispatch(&app, "POST", "/lessons", &Query::new(), &bad).is_err(), "{bad}");
    }
}
