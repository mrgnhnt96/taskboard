//! Ideas: kept per project with notes and report rounds, shelved, dropped or reopened, and found by search.

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;

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

fn try_call(app: &App, method: &str, path: &str, query: &[(&str, &str)], body: Value) -> Result<Value, (u16, String)> {
    let q: Query = query.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    api::dispatch(app, method, path, &q, &body).map_err(|e| (e.status, e.message))
}

fn call(app: &App, method: &str, path: &str, body: Value) -> Value {
    try_call(app, method, path, &[], body).unwrap()
}

fn idea(app: &App, project: &str, title: &str, detail: &str) -> String {
    let v = call(app, "POST", "/ideas", json!({"title": title, "detail": detail, "project": project}));
    v["ref"].as_str().unwrap().to_string()
}

fn listed(app: &App, query: &[(&str, &str)]) -> Vec<String> {
    try_call(app, "GET", "/ideas", query, json!({})).unwrap()["ideas"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["ref"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn ideas_are_kept_per_project_and_listed_newest_change_first() {
    let (dir, app) = new_board();
    let v = call(&app, "POST", "/ideas", json!({"title": "  Offline   mode ", "detail": "Queue writes.", "project": "webapp"}));
    assert_eq!(v["ref"], "I1");
    assert_eq!(v["title"], "Offline mode");
    assert_eq!(v["state"], "new");
    assert_eq!(v["goal"], Value::Null);
    assert_eq!(v["notes"], json!([]));
    assert_eq!(v["rounds"], json!([]));

    let cwd = dir.path().join("api").to_string_lossy().to_string();
    let a = call(&app, "POST", "/ideas", json!({"title": "Rate limits", "cwd": cwd}));
    assert_eq!(a["project"], "api", "the folder tb ran in names the project");
    let (code, msg) = try_call(&app, "POST", "/ideas", &[], json!({"title": "Nowhere"})).unwrap_err();
    assert_eq!((code, msg.as_str()), (400, "Say which project the idea is for (--project)."));
    assert_eq!(try_call(&app, "POST", "/ideas", &[], json!({"title": " ", "project": "webapp"})).unwrap_err().0, 400);

    let second = idea(&app, "webapp", "Dark theme", "");
    assert_eq!(listed(&app, &[("project", "webapp")]), [second.clone(), "I1".to_string()]);
    assert_eq!(listed(&app, &[("cwd", &cwd)]), ["I2"]);
    assert_eq!(listed(&app, &[("project", "all")]).len(), 3);

    app.db.x("UPDATE ideas SET updated_at = '2026-01-01T00:00:00Z'", taskboardd::p![]).unwrap();
    call(&app, "POST", "/ideas/I1/note", json!({"text": "Sync on reconnect."}));
    assert_eq!(listed(&app, &[("project", "webapp")]), ["I1".to_string(), second], "a note bumps it");
    let i1 = &try_call(&app, "GET", "/ideas", &[("project", "webapp")], json!({})).unwrap()["ideas"][0];
    assert_eq!((i1["notes"].as_i64(), i1["rounds"].as_i64()), (Some(1), Some(0)), "the list counts notes and rounds");
}

#[test]
fn show_set_and_note() {
    let (_d, app) = new_board();
    let r = idea(&app, "webapp", "Offline mode", "Queue writes.");
    call(&app, "POST", &format!("/ideas/{r}/note"), json!({"text": "First."}));
    call(&app, "POST", &format!("/ideas/{r}/note"), json!({"text": "Second."}));
    assert_eq!(try_call(&app, "POST", &format!("/ideas/{r}/note"), &[], json!({"text": "  "})).unwrap_err().0, 400);

    let v = call(&app, "POST", &format!("/ideas/{r}"), json!({"title": "Offline first", "project": "api"}));
    assert_eq!((v["title"].as_str(), v["project"].as_str(), v["detail"].as_str()), (Some("Offline first"), Some("api"), Some("Queue writes.")));
    assert_eq!(try_call(&app, "POST", &format!("/ideas/{r}"), &[], json!({})).unwrap_err().0, 400);
    assert_eq!(try_call(&app, "POST", &format!("/ideas/{r}"), &[], json!({"project": ""})).unwrap_err().0, 400);

    let v = call(&app, "GET", &format!("/ideas/{r}"), json!({}));
    let notes: Vec<&str> = v["notes"].as_array().unwrap().iter().map(|n| n["text"].as_str().unwrap()).collect();
    assert_eq!(notes, ["First.", "Second."]);

    assert_eq!(try_call(&app, "GET", "/ideas/I99", &[], json!({})).unwrap_err(), (404, "There's no idea I99.".to_string()));
    assert_eq!(try_call(&app, "GET", "/ideas/T1", &[], json!({})).unwrap_err().0, 400);
}

#[test]
fn shelve_drop_and_reopen() {
    let (_d, app) = new_board();
    let r = idea(&app, "webapp", "Offline mode", "");
    let id: i64 = r[1..].parse().unwrap();

    let v = call(&app, "POST", &format!("/ideas/{r}/shelve"), json!({"reason": "Not this quarter."}));
    assert_eq!(v["state"], "shelved");
    assert_eq!(v["notes"][0]["text"], "Shelved: Not this quarter.");
    assert!(listed(&app, &[("project", "webapp")]).is_empty(), "shelved isn't open");
    assert_eq!(listed(&app, &[("project", "webapp"), ("state", "shelved")]), [r.clone()]);
    assert_eq!(listed(&app, &[("project", "webapp"), ("state", "all")]), [r.clone()]);
    assert_eq!(try_call(&app, "GET", "/ideas", &[("state", "later")], json!({})).unwrap_err().0, 400);

    assert_eq!(call(&app, "POST", &format!("/ideas/{r}/reopen"), json!({}))["state"], "new");
    assert_eq!(call(&app, "POST", &format!("/ideas/{r}/drop"), json!({}))["state"], "dropped");
    app.db
        .insert("idea_rounds", taskboardd::fields!["idea_id" => id, "focus" => "", "report" => r#"{"fit":"good"}"#, "sha" => "abc1234def", "web" => 1])
        .unwrap();
    let v = call(&app, "POST", &format!("/ideas/{r}/reopen"), json!({}));
    assert_eq!(v["state"], "reported", "an idea with a report comes back reported");
    assert_eq!(v["rounds"][0]["report"]["fit"], "good");
    assert_eq!(v["rounds"][0]["web"], true);

    app.shared.lock().digging.insert(id);
    app.db.update("ideas", &json!(id), taskboardd::fields!["state" => "digging"]).unwrap();
    assert_eq!(try_call(&app, "POST", &format!("/ideas/{r}/shelve"), &[], json!({})).unwrap_err().0, 409);
    app.shared.lock().digging.remove(&id);
    app.db.update("ideas", &json!(id), taskboardd::fields!["state" => "planned", "goal_id" => 7]).unwrap();
    let (code, msg) = try_call(&app, "POST", &format!("/ideas/{r}/drop"), &[], json!({})).unwrap_err();
    assert_eq!((code, msg.as_str()), (409, "I1 became G7 already; work on the goal instead."));
}

#[test]
fn search_finds_ideas_by_their_words() {
    let (_d, app) = new_board();
    let r = idea(&app, "webapp", "Offline mode", "Queue writes while the network is down.");
    let q: Query = [("q", "network down"), ("kind", "idea")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let hits = api::dispatch(&app, "GET", "/search", &q, &json!({})).unwrap()["hits"].as_array().unwrap().clone();
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0]["kind"].as_str(), hits[0]["ref"].as_str(), hits[0]["state"].as_str()), (Some("idea"), Some(r.as_str()), Some("new")));
}
