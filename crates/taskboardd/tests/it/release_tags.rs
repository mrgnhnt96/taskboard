//! Releases cut by tags: a new tag matching the project's pattern on its default branch cuts a release
//! holding the done work the tag contains.

use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::board;
use taskboardd::config::Config;
use taskboardd::release_tags;

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(repo).args(["-c", "user.name=T", "-c", "user.email=t@example.com"]).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn commit(repo: &Path, subject: &str) {
    git(repo, &["commit", "--allow-empty", "-q", "-m", subject]);
}

fn new_board() -> (tempfile::TempDir, std::sync::Arc<App>, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    commit(&repo, "First");
    let projects = json!([{"name": "webapp", "path": repo.to_string_lossy()}]);
    app.db.set_setting("midna_projects", Some(&projects.to_string())).unwrap();
    (dir, app, repo)
}

fn call(app: &App, method: &str, path: &str, body: Value) -> Value {
    api::dispatch(app, method, path, &Query::new(), &body).map_err(|e| (e.status, e.message)).unwrap()
}

fn merged_task(app: &App, title: &str, pr: i64) -> i64 {
    let id = call(app, "POST", "/tasks", json!({"title": title, "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
    board::update_task(
        app,
        id,
        vec![
            ("status", json!("done")),
            ("finished_at", json!(taskboardd::util::now_iso())),
            ("pr_num", json!(pr)),
            ("pr_url", json!(format!("https://github.com/o/webapp/pull/{pr}"))),
            ("pr_phase", json!("merged")),
        ],
    )
    .unwrap();
    id
}

fn set(app: &App, body: Value) {
    call(app, "POST", "/projects/webapp", body);
}

fn release_tasks(app: &App, r: &str) -> Vec<String> {
    let q: Query = [("project".to_string(), "webapp".to_string())].into_iter().collect();
    let v = api::dispatch(app, "GET", &format!("/releases/{r}"), &q, &json!({})).map_err(|e| e.message).unwrap();
    v["groups"].as_array().unwrap().iter().flat_map(|g| g["tasks"].as_array().unwrap().iter().map(|t| t["ref"].as_str().unwrap().to_string())).collect()
}

#[test]
fn a_new_tag_cuts_a_release_with_the_work_it_contains() {
    let (_d, app, repo) = new_board();
    set(&app, json!({"releases": "on", "release_tags": "v*"}));
    let p = call(&app, "GET", "/projects", json!({}))["projects"][0].clone();
    assert_eq!(p["release_tags"], "v*");

    // A tag there before the board looked is only noted.
    git(&repo, &["tag", "v0.9"]);
    let one = merged_task(&app, "One", 1);
    assert!(release_tags::look(&app, "webapp").unwrap().is_empty());

    commit(&repo, "Merge pull request #1 from o/feat/one");
    git(&repo, &["tag", "-a", "v1.0", "-m", "v1.0"]);
    let two = merged_task(&app, "Two", 2);
    commit(&repo, "Two (#2)");
    // Off the default branch: not cut yet.
    git(&repo, &["checkout", "-q", "-b", "side"]);
    commit(&repo, "Side");
    git(&repo, &["tag", "v1.0-side"]);
    git(&repo, &["checkout", "-q", "main"]);

    let cut = release_tags::look(&app, "webapp").unwrap();
    assert_eq!(cut.len(), 1);
    assert_eq!(release_tasks(&app, &cut[0]), vec![format!("T{one}")]);
    let list = api::dispatch(&app, "GET", "/releases", &[("project".to_string(), "webapp".to_string())].into_iter().collect(), &json!({})).unwrap();
    assert_eq!(list["releases"][0]["name"], "v1.0");
    assert_eq!(list["releases"][0]["cut_by"], board::BOARD);
    assert_eq!(release_tasks(&app, "unreleased"), vec![format!("T{two}")]);

    // Looked at again, nothing new.
    assert!(release_tags::look(&app, "webapp").unwrap().is_empty());

    git(&repo, &["tag", "v1.1"]);
    let cut = release_tags::look(&app, "webapp").unwrap();
    assert_eq!(release_tasks(&app, &cut[0]), vec![format!("T{two}")]);
    assert!(release_tasks(&app, "unreleased").is_empty());

    // The side tag lands on main once main merges it.
    git(&repo, &["merge", "-q", "--ff-only", "side"]);
    assert!(release_tags::look(&app, "webapp").unwrap().is_empty(), "nothing unreleased, so no release");
}

#[test]
fn tags_cut_nothing_while_releases_are_off() {
    let (_d, app, repo) = new_board();
    set(&app, json!({"release_tags": "v*"}));
    assert!(release_tags::look(&app, "webapp").unwrap().is_empty());
    merged_task(&app, "One", 1);
    commit(&repo, "One (#1)");
    git(&repo, &["tag", "v1.0"]);
    assert!(release_tags::look(&app, "webapp").unwrap().is_empty());

    // Turned on, the tag made while they were off is only noted; the next one cuts.
    set(&app, json!({"releases": "on"}));
    assert!(release_tags::look(&app, "webapp").unwrap().is_empty());
    git(&repo, &["tag", "v1.1"]);
    assert_eq!(release_tags::look(&app, "webapp").unwrap().len(), 1);

    set(&app, json!({"release_tags": "none"}));
    let p = call(&app, "GET", "/projects", json!({}))["projects"][0].clone();
    assert_eq!(p["release_tags"], Value::Null);
}

#[test]
fn a_tag_holds_work_merged_through_origin() {
    let (d, app, repo) = new_board();
    // Named like the PR links, so they're this repository's.
    let origin = d.path().join("o").join("webapp.git");
    git(d.path(), &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
    git(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git(&repo, &["push", "-q", "origin", "main"]);
    set(&app, json!({"releases": "on", "release_tags": "release-*"}));
    assert!(release_tags::look(&app, "webapp").unwrap().is_empty());

    // Merged and tagged on origin from another clone; the board fetches it.
    let other = d.path().join("other");
    git(d.path(), &["clone", "-q", origin.to_str().unwrap(), other.to_str().unwrap()]);
    let one = merged_task(&app, "One", 7);
    commit(&other, "Merge pull request #7 from o/feat/one");
    git(&other, &["tag", "release-1"]);
    git(&other, &["push", "-q", "origin", "main", "--tags"]);

    let cut = release_tags::look(&app, "webapp").unwrap();
    assert_eq!(release_tasks(&app, &cut[0]), vec![format!("T{one}")]);
}
