//! Lessons outlast their goal: suggested ones reach nobody, kept ones reach later handoffs in their
//! project (and every project, when global), and planning lessons reach planners instead. Only the
//! few that fit the work do: by its words and the code it's likely to touch.

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, handoff, relevance};

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

fn task_titled(app: &App, project: &str, title: &str, detail: &str) -> i64 {
    call(app, "POST", "/tasks", json!({"title": title, "detail": detail, "project": project}))["id"].as_i64().unwrap()
}

/// The lessons a handoff gives, one line each.
fn handoff_lessons(app: &App, task: i64) -> Vec<String> {
    let h = handoff::build(app, task).unwrap();
    let Some((_, rest)) = h.split_once("Lessons from earlier work here:\n") else { return vec![] };
    rest.lines().take_while(|l| l.starts_with("- ")).map(|l| l.to_string()).collect()
}

fn kept_lesson(app: &App, text: &str, evidence: Value, confirmations: usize) -> String {
    let l = lesson(app, json!({"text": text, "kind": "code", "project": "webapp", "evidence": evidence, "kept": true}));
    for _ in 0..confirmations {
        call(app, "POST", &format!("/lessons/{l}/confirm"), json!({}));
    }
    l
}

#[test]
fn handoffs_get_the_lessons_that_fit_the_task() {
    let (_d, app) = new_board();
    let debounce = kept_lesson(&app, "Debounce the search box before fetching results.", json!([]), 0);
    let tests = kept_lesson(&app, "Run only the tests the change touched.", json!([]), 3);
    kept_lesson(&app, "Put each migration in its own file.", json!([]), 1);
    kept_lesson(&app, "Keep colour tokens in the theme file.", json!([]), 0);

    let got = handoff_lessons(&app, task_titled(&app, "webapp", "Search box fetches on every key", "Debounce it."));
    assert_eq!(got.len(), 2, "the one that fits, then the most confirmed: {got:?}");
    assert!(got[0].ends_with(&format!("({debounce})")), "{got:?}");
    assert!(got[1].ends_with(&format!("({tests})")), "{got:?}");
}

#[test]
fn handoffs_get_the_lessons_learned_in_the_same_code() {
    let (_d, app) = new_board();
    let old = task_titled(&app, "webapp", "Tidy up", "Nothing to say.");
    relevance::record(&app, old, "abc123", &["src/db.rs".into(), "src/api.rs".into()]).unwrap();
    assert_eq!(relevance::paths_of(&app, old).unwrap(), ["src/api.rs", "src/db.rs"]);
    // Nothing found on a later read keeps what was found before.
    relevance::record(&app, old, "def456", &[]).unwrap();
    assert_eq!(relevance::paths_of(&app, old).unwrap().len(), 2);

    let schema = kept_lesson(&app, "Bump the schema version with every table change.", json!([format!("T{old}")]), 0);
    for (i, text) in ["Run only the tests the change touched.", "Say the risk first.", "Name branches after the task."].iter().enumerate() {
        kept_lesson(&app, text, json!([]), i + 1);
    }
    let got = handoff_lessons(&app, task_titled(&app, "webapp", "Add a column", "In src/db.rs."));
    assert!(got[0].ends_with(&format!("({schema})")), "{got:?}");

    let q: Query = [("q".to_string(), "column in src/db.rs".to_string()), ("project".to_string(), "webapp".to_string())].into();
    let v = api::dispatch(&app, "GET", "/lessons/search", &q, &json!({})).unwrap();
    let found = v["lessons"].as_array().unwrap();
    assert_eq!(found.len(), 1, "{v}");
    assert_eq!(found[0]["ref"], schema.as_str());
    assert_eq!(found[0]["why"], json!(["touches src/db.rs"]));
}

#[test]
fn lessons_are_searched_by_their_words() {
    let (_d, app) = new_board();
    let l = kept_lesson(&app, "Debounce the search box before fetching results.", json!([]), 0);
    lesson(&app, json!({"text": "Debounce the api's retries.", "kind": "code", "project": "api"}));
    let search = |q: &str| {
        let q: Query = [("q".to_string(), q.to_string()), ("project".to_string(), "webapp".to_string())].into();
        api::dispatch(&app, "GET", "/lessons/search", &q, &json!({}))
    };
    let v = search("debouncing").unwrap();
    let found = v["lessons"].as_array().unwrap();
    assert_eq!(found.len(), 1, "only this project's: {v}");
    assert_eq!(found[0]["ref"], l.as_str());
    assert_eq!(found[0]["why"], json!(["its words"]));
    assert!(search("unrelated words").unwrap()["lessons"].as_array().unwrap().is_empty());
    assert!(search("the a").is_err(), "nothing to search for");
}
