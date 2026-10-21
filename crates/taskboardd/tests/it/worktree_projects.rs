//! A worktree folder isn't a project of its own: Midna's listing of one, and a terminal working in one,
//! fold into the repo's project; a listed folder that's gone drops out.

use serde_json::json;
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{midna, projects};

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
