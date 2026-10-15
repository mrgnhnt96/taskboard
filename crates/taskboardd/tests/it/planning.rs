//! `[planning]`: a project's planning file wins over the board's, the board's over the built-in rules,
//! and planners get whichever is in force.

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, planning};

fn board_with(f: impl FnOnce(&mut Config, &std::path::Path)) -> (tempfile::TempDir, std::sync::Arc<App>) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    f(&mut cfg, dir.path());
    let app = App::for_tests(cfg);
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

fn planning_for(app: &App, k: &str, v: &str) -> Value {
    let q: Query = [(k.to_string(), v.to_string())].into();
    api::dispatch(app, "GET", "/planning", &q, &json!({})).unwrap()
}

fn planner_prompt(app: &App, project: &str) -> String {
    let id = api::dispatch(app, "POST", "/goals", &Query::new(), &json!({"name": "Ship login", "project": project})).unwrap()["id"]
        .as_i64()
        .unwrap();
    api::goal_context(app, &board::get_goal(app, id).unwrap(), true).unwrap()
}

#[test]
fn with_no_file_planners_get_the_built_in_rules() {
    let (_d, app) = board_with(|_, _| {});
    let v = planning_for(&app, "project", "webapp");
    assert_eq!(v["from"], "built-in");
    assert_eq!(v["text"], planning::BUILT_IN);
    let prompt = planner_prompt(&app, "webapp");
    assert!(prompt.contains(&format!("How to slice the work:\n{}", planning::BUILT_IN)), "{prompt}");
}

#[test]
fn the_built_in_rules_say_when_to_stack_and_how_to_merge_it() {
    for bit in ["needs that task's unmerged code", "Tail merge a stack (the default)", "safe to ship alone", "Head merge a stack", "only make sense together", "--stack-merge"] {
        assert!(planning::BUILT_IN.contains(bit), "missing {bit:?}");
    }
}

#[test]
fn with_models_on_the_built_in_rules_say_which_model_and_tb_planning_names_them() {
    let (_d, app) = board_with(|_, _| {});
    assert!(planning_for(&app, "project", "webapp")["models"].is_null(), "off: no models");
    for bit in ["--model small", "mechanical, single-file or copy work", "--model large", "design, cross-crate or risky work", "Unsure: large"] {
        assert!(planning::MODEL_RULE.contains(bit), "missing {bit:?}");
    }

    let (_d, app) = board_with(|c, _| c.models.enabled = true);
    let v = planning_for(&app, "project", "webapp");
    assert_eq!(v["text"], format!("{}\n{}", planning::BUILT_IN, planning::MODEL_RULE));
    assert_eq!(v["models"], json!({"small": "sonnet", "large": "opus", "default": "opus"}));
    assert!(planner_prompt(&app, "webapp").contains(planning::MODEL_RULE));

    let (_d, app) = board_with(|c, d| {
        std::fs::write(d.join("board.md"), "Opus for everything.").unwrap();
        c.planning.guide = d.join("board.md").to_string_lossy().into();
        c.models.enabled = true;
    });
    assert_eq!(planning_for(&app, "project", "webapp")["text"], "Opus for everything.", "the owner's file wins");
    assert!(!planner_prompt(&app, "webapp").contains(planning::MODEL_RULE));
}

#[test]
fn a_project_file_wins_over_the_board_file_and_the_board_file_over_the_built_in_rules() {
    let (d, app) = board_with(|c, d| {
        std::fs::write(d.join("board.md"), "One task per screen.\n").unwrap();
        std::fs::write(d.join("api.md"), "One task per endpoint.").unwrap();
        c.planning.guide = d.join("board.md").to_string_lossy().into();
        c.planning.projects.insert("api".into(), d.join("api.md").to_string_lossy().into());
    });
    let web = planning_for(&app, "project", "webapp");
    assert_eq!((web["from"].as_str(), web["text"].as_str()), (Some("board"), Some("One task per screen.")));
    let api = planning_for(&app, "cwd", &d.path().join("api/src").to_string_lossy());
    assert_eq!((api["project"].as_str(), api["from"].as_str(), api["text"].as_str()), (Some("api"), Some("project"), Some("One task per endpoint.")));

    let prompt = planner_prompt(&app, "api");
    assert!(prompt.contains("How this project slices its work (the owner's rules, in place of the board's):\nOne task per endpoint."), "{prompt}");
    assert!(!prompt.contains(planning::BUILT_IN) && !prompt.contains("One task per screen."), "{prompt}");
    assert!(planner_prompt(&app, "webapp").contains("How this board slices its work (the owner's rules, in place of the built-in ones):\nOne task per screen."));
}

#[test]
fn an_empty_or_unreadable_file_falls_back_and_says_why() {
    let (_d, app) = board_with(|c, d| {
        std::fs::write(d.join("api.md"), "  \n").unwrap();
        c.planning.guide = "/nowhere/planning.md".into();
        c.planning.projects.insert("api".into(), d.join("api.md").to_string_lossy().into());
    });
    let v = planning_for(&app, "project", "api");
    assert_eq!(v["from"], "built-in");
    let skipped = v["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 2);
    assert_eq!((skipped[0]["from"].as_str(), skipped[0]["error"].as_str()), (Some("project"), Some("it's empty")));
    assert!(skipped[1]["error"].as_str().unwrap().contains("can't read /nowhere/planning.md"));
}

#[test]
fn files_in_the_boards_folder_need_no_config() {
    let (d, app) = board_with(|_, d| {
        std::fs::create_dir_all(d.join("planning")).unwrap();
        std::fs::write(d.join("planning.md"), "One task per screen.").unwrap();
        std::fs::write(d.join("planning/api.md"), "One task per endpoint.").unwrap();
        std::fs::write(d.join("planning/webapp.md"), "").unwrap();
    });
    let api = planning_for(&app, "project", "api");
    assert_eq!((api["from"].as_str(), api["text"].as_str()), (Some("project"), Some("One task per endpoint.")));
    assert_eq!(api["path"].as_str().unwrap(), d.path().join("planning/api.md").to_string_lossy());
    let web = planning_for(&app, "project", "webapp");
    assert_eq!((web["from"].as_str(), web["text"].as_str()), (Some("board"), Some("One task per screen.")));
    assert_eq!(web["skipped"], json!([]), "an empty file in its usual place is a placeholder, not a mistake");

    std::fs::remove_file(d.path().join("planning.md")).unwrap();
    let web = planning_for(&app, "project", "webapp");
    assert_eq!((web["from"].as_str(), web["skipped"].clone()), (Some("built-in"), json!([])));
}
