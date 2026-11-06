//! A worktree folder isn't a project of its own: Midna's listing of one, and a terminal working in one,
//! fold into the repo's project; a listed folder that's gone drops out. A task whose saved folder is
//! gone follows its project to the folder it moved to.

use serde_json::json;
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::Row;
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
