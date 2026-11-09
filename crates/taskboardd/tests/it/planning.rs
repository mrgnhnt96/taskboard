//! `[planning]`: a project's planning file wins over the board's, the board's over the built-in rules,
//! and planners get whichever is in force.

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, midna, planning, reports};

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
    for bit in ["needs that task's unmerged code", "Tail merge a stack (--stack-merge tail)", "safe to ship alone", "Head merge a stack", "only make sense together", "--stack-merge"] {
        assert!(planning::BUILT_IN.contains(bit), "missing {bit:?}");
    }
}

#[test]
fn the_built_in_rules_plan_stacks_for_ci_cost() {
    for bit in ["every PR to main runs the full checks", "Small, related tasks share one stack", "with --stack-merge head", "Fan out, don't chain",
                "fold it into a related stack", "Push once per review round", "only when it conflicts or right before it merges", "tb planning names it"] {
        assert!(planning::BUILT_IN.contains(bit), "missing {bit:?}");
    }
}

#[test]
fn the_built_in_rules_weigh_the_cost_of_each_extra_pr() {
    for bit in ["Each cut has a cost", "only when each side is worth its own review", "being able to name the seam isn't enough",
                "Size has two limits", "one person can read the whole diff", "folds into that neighbour",
                "limited to files the task already touches stays in the task, as its own commit", "A sweep across many files is its own task",
                "A guard nothing else protects", "is its own task, however small", "The upper limit is for the whole stack"] {
        assert!(planning::BUILT_IN.contains(bit), "missing {bit:?}");
    }
}

/// Proposes `tasks` for goal `g` from a terminal in the webapp checkout: (the tasks made, the plan check's warnings).
fn propose(app: &App, dir: &std::path::Path, g: i64, tasks: Value) -> (Vec<String>, Vec<String>) {
    midna::sync(app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": dir.join("webapp").to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    let b = json!({"event": "tb.propose", "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}, "goal": format!("G{g}"), "tasks": tasks});
    let r = reports::handle(app, b, false).unwrap_or_else(|e| panic!("propose: {}", e.message));
    let strs = |v: &Value| v.as_array().cloned().unwrap_or_default().iter().map(|w| w.as_str().unwrap().to_string()).collect();
    (strs(&r["created"]), strs(&r["warnings"]))
}

fn task(title: &str, detail: &str, files: &[&str]) -> Value {
    json!({"title": title, "detail": detail, "wave": 1, "files": files})
}

const LONG: &str = "Rework how the list loads its rows.\nPage them from the daemon and keep the scroll position across reloads.";

#[test]
fn the_plan_check_warns_when_many_of_a_goals_prs_look_tiny() {
    let (d, app) = board_with(|_, _| {});
    api::dispatch(&app, "POST", "/projects/webapp", &Query::new(), &json!({"pr_flow": "on"})).unwrap();
    let g = api::dispatch(&app, "POST", "/goals", &Query::new(), &json!({"name": "Tidy the list", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    let tiny = |w: &[String]| w.iter().find(|w| w.contains("each look like a tiny PR")).cloned();

    let (mut made, w) = propose(&app, d.path(), g, json!([task("Pin the dep", "Bump serde to 1.0.200.", &[]), task("Fix the typo", "Spell 'recieve' right in the header.", &[])]));
    assert_eq!(tiny(&w), None, "two is fine: {w:?}");
    let (big, w) = propose(&app, d.path(), g, json!([task("Rework loading", LONG, &["src/list/load.rs", "src/list/page.rs"])]));
    assert_eq!(tiny(&w), None, "a long detail over two files isn't tiny: {w:?}");

    let (beside, w) = propose(&app, d.path(), g, json!([task("Keep the scroll", LONG, &["src/list/scroll.rs"])]));
    made.extend(beside);
    let line = tiny(&w).unwrap_or_else(|| panic!("a third: one file in the folder a sibling's files are in: {w:?}"));
    assert_eq!(line, format!(
        "{} each look like a tiny PR (a one-line detail, or one file beside a sibling's), and every PR takes a human review and an agent review round: \
         fold each into the neighbour it only makes sense next to, unless it's a guard nothing else protects.",
        made.join(", ")
    ));
    assert!(!line.contains(&big[0]), "the long two-file task isn't one of them: {line}");

    let (lone, w) = propose(&app, d.path(), g, json!([task("Show the count", LONG, &["docs/count.md"])]));
    assert_eq!(tiny(&w).as_deref(), Some(line.as_str()), "one file no sibling's sits beside isn't tiny ({}): {w:?}", lone[0]);
}

#[test]
fn the_plan_check_warns_when_a_task_is_told_not_to_cut_a_release_itself() {
    let (d, app) = board_with(|_, _| {});
    let g = api::dispatch(&app, "POST", "/goals", &Query::new(), &json!({"name": "Ship the fix", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    let release = |w: &[String]| w.iter().find(|w| w.contains("not to cut a release itself")).cloned();

    let (_, w) = propose(&app, d.path(), g, json!([task("Fix the crash", "Fix it.\nRelease the lock before the await.", &["src/a.rs"])]));
    assert_eq!(release(&w), None, "releasing a lock isn't a release: {w:?}");

    let (made, w) = propose(&app, d.path(), g, json!([task("Verify the build", "Check the signed build on a Mac.\nDon\u{2019}t cut a release yourself.", &["src/b.rs"])]));
    assert_eq!(release(&w), Some(format!(
        "{0}'s detail tells its agent not to cut a release itself, but a release cut is a command an agent runs (tb release cut v<version>): \
         plan it as its own --no-pr task and have {0} wait for it (--waits-for), never as a human task.",
        made[0]
    )));
}

#[test]
fn with_models_on_the_built_in_rules_say_which_model_and_tb_planning_names_them() {
    let (_d, app) = board_with(|_, _| {});
    assert!(planning_for(&app, "project", "webapp")["models"].is_null(), "off: no models");
    for bit in ["every task a tier", "--model small", "sonnet at medium effort", "fully specified, few-file, mechanical, copy or test work", "--model large", "opus at high effort", "design, cross-crate, concurrency, daemon or GPUI rendering, release or signing work, or a vague spec", "names the files and the behaviour, it's small", "--effort xhigh for gnarly debugging", "--effort low for trivial copy edits"] {
        assert!(planning::MODEL_RULE.contains(bit), "missing {bit:?}");
    }
    assert!(!planning::MODEL_RULE.contains("Unsure: large"));

    let (_d, app) = board_with(|c, _| c.models.enabled = true);
    let v = planning_for(&app, "project", "webapp");
    assert_eq!(v["text"], format!("{}\n{}", planning::BUILT_IN, planning::MODEL_RULE));
    assert_eq!(v["models"], json!({"small": "sonnet", "large": "opus", "default": "opus", "small_effort": "medium", "large_effort": "high", "default_effort": "high"}));
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

#[test]
fn the_built_in_rules_trace_the_outcome_and_plan_prerequisites_first() {
    for bit in ["Trace the outcome", "maps to a task that proves it", "verification task with those devices", "List prerequisites first",
                "A human task for anything only the owner can do", "Known failing tests go in the goal notes", "what other systems must provide",
                "the first task writes them down", "plan up to it plus a re-plan task, not the whole goal"] {
        assert!(planning::BUILT_IN.contains(bit), "missing {bit:?}");
    }
}
