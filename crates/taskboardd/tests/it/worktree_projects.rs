//! A worktree folder isn't a project of its own: Midna's listing of one, and a terminal working in one,
//! fold into the repo's project; a listed folder that's gone drops out. A task whose saved folder is
//! gone follows its project to the folder it moved to, and so does a goal's, for planning it and for the
//! tasks made or moved into it.

use serde_json::json;
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, midna, projects, runner};

#[test]
fn worktree_folders_fold_into_their_repo() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("webapp");
    let wt = dir.path().join("webapp-t12");
    std::fs::create_dir_all(repo.join(".git/worktrees/webapp-t12")).unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(wt.join(".git"), format!("gitdir: {}/.git/worktrees/webapp-t12\n", repo.display())).unwrap();
    let gone = dir.path().join("webapp-b276");
    let app = App::for_tests(Config::for_tests(dir.path()));
    let path = |p: &std::path::Path| p.to_string_lossy().to_string();
    midna::sync(
        &app,
        &[json!({"id": "s1", "name": "Term", "agent": "claude", "project_id": 2, "cwd": path(&wt), "status": {"state": "working"}})],
        &[
            json!({"id": 1, "name": "webapp", "path": path(&repo)}),
            json!({"id": 2, "name": "webapp-t12", "path": path(&wt)}),
            json!({"id": 3, "name": "webapp-b276", "path": path(&gone)}),
        ],
    )
    .unwrap();
    let names: Vec<String> = projects::list_projects(&app).unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(names, ["webapp"]);
    assert_eq!(projects::main_checkout(&path(&wt)), Some(path(&repo)));
    assert_eq!(projects::main_checkout(&path(&repo)), None);
}

fn task_saved_at(app: &App, saved: &str) -> Row {
    let made = api::dispatch(app, "POST", "/tasks", &Query::new(), &json!({"title": "Port it", "project": "webapp"})).unwrap();
    let id = made["ref"].as_str().unwrap().trim_start_matches('T').parse().unwrap();
    board::update_task(app, id, vec![("repo_path", json!(saved))]).unwrap();
    board::get_task(app, id).unwrap()
}

fn set_project_folder(app: &App, projects: serde_json::Value) {
    app.db.set_setting("midna_projects", Some(&projects.to_string())).unwrap();
}

#[test]
fn a_task_follows_its_project_when_its_saved_folder_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let moved = dir.path().join("moved").to_string_lossy().to_string();
    std::fs::create_dir_all(&moved).unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    set_project_folder(&app, json!([{"name": "webapp", "path": moved}]));
    let gone = dir.path().join("gone").to_string_lossy().to_string();
    let t = task_saved_at(&app, &gone);
    assert_eq!(runner::task_cwd(&app, &t).unwrap(), Some(moved));
}

#[test]
fn a_saved_folder_that_exists_beats_the_projects_folder() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home").to_string_lossy().to_string();
    let saved = dir.path().join("saved").to_string_lossy().to_string();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&saved).unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    set_project_folder(&app, json!([{"name": "webapp", "path": home}]));
    let t = task_saved_at(&app, &saved);
    assert_eq!(runner::task_cwd(&app, &t).unwrap(), Some(saved));
}

#[test]
fn a_gone_folder_stays_when_the_project_has_no_other() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let gone = dir.path().join("gone").to_string_lossy().to_string();
    let t = task_saved_at(&app, &gone);
    assert_eq!(runner::task_cwd(&app, &t).unwrap(), Some(gone));
}

struct MovedGoal {
    app: std::sync::Arc<App>,
    goal: String,
    moved: String,
    _dir: tempfile::TempDir,
}

fn goal_left_behind() -> MovedGoal {
    let dir = tempfile::tempdir().unwrap();
    let moved = dir.path().join("moved").to_string_lossy().to_string();
    let other = dir.path().join("other").to_string_lossy().to_string();
    std::fs::create_dir_all(&moved).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    set_project_folder(&app, json!([{"name": "webapp", "path": moved}, {"name": "notes", "path": other}]));
    let made = api::dispatch(&app, "POST", "/goals", &Query::new(), &json!({"name": "Port it", "project": "webapp"})).unwrap();
    let goal = made["ref"].as_str().unwrap().to_string();
    let gone = dir.path().join("gone").to_string_lossy().to_string();
    app.db.x("UPDATE goals SET repo_path = ?", taskboardd::p![gone]).unwrap();
    MovedGoal { app, goal, moved, _dir: dir }
}

impl MovedGoal {
    fn post(&self, path: &str, body: serde_json::Value) -> serde_json::Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn task_repo(&self, made: &serde_json::Value) -> String {
        let id = made["ref"].as_str().unwrap().trim_start_matches('T').parse().unwrap();
        board::get_task(&self.app, id).unwrap().st("repo_path").to_string()
    }
}

#[test]
fn planning_a_goal_follows_its_project_when_its_saved_folder_is_gone() {
    let g = goal_left_behind();
    let plan = g.post(&format!("/goals/{}/plan", g.goal), json!({}));
    let job = plan["job"].as_str().unwrap().trim_start_matches('J').parse::<i64>().unwrap();
    let row = g.app.db.q1("SELECT * FROM jobs WHERE id = ?", taskboardd::p![job]).unwrap().unwrap();
    assert_eq!(board::job_args(&row).st("cwd"), g.moved);
}

#[test]
fn a_new_task_in_a_goal_left_behind_saves_the_projects_folder() {
    let g = goal_left_behind();
    let made = g.post("/tasks", json!({"title": "Port the page", "project": "webapp", "goal_id": g.goal}));
    assert_eq!(g.task_repo(&made), g.moved);
}

#[test]
fn a_task_moved_into_a_goal_left_behind_saves_the_projects_folder() {
    let g = goal_left_behind();
    let made = g.post("/tasks", json!({"title": "Port the page", "project": "notes"}));
    g.post(&format!("/tasks/{}", made["ref"].as_str().unwrap()), json!({"goal_id": g.goal, "project": "webapp"}));
    assert_eq!(g.task_repo(&made), g.moved);
}
