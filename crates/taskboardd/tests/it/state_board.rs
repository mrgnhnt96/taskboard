//! `GET /state`'s Done column (picked from the rows, then carded), the project list it and
//! every card read, which the board keeps until the database changes, and the stamp a poll asks
//! it and `GET /goals` `since`.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{iso, now_iso, now_ts};
use taskboardd::{fields, projects};

fn new_board() -> (Arc<App>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    (app, dir)
}

fn state(app: &App, done: &str) -> Value {
    let q: Query = [("done".to_string(), done.to_string())].into_iter().collect();
    api::dispatch(app, "GET", "state", &q, &json!({})).unwrap_or_else(|e| panic!("GET state: {}", e.message))
}

/// A done task that finished `hours` ago (None: no finished_at, so its updated_at counts).
fn done_task(app: &App, title: &str, hours: Option<f64>) -> i64 {
    let at = iso(now_ts() - hours.unwrap_or(0.0) * 3600.0);
    let finished = if hours.is_some() { json!(at) } else { Value::Null };
    app.db
        .insert("tasks", fields!["title" => title, "project" => "webapp", "status" => "done", "created_at" => now_iso(),
                                 "updated_at" => at, "finished_at" => finished])
        .unwrap()
}

fn titles(st: &Value) -> Vec<String> {
    st["columns"]["done"].as_array().unwrap().iter().map(|c| c["title"].as_str().unwrap().to_string()).collect()
}

#[test]
fn done_shows_the_window_newest_first_and_counts_the_rest() {
    let (app, _dir) = new_board();
    done_task(&app, "ten days", Some(240.0));
    done_task(&app, "an hour", Some(1.0));
    done_task(&app, "three days", Some(72.0));
    done_task(&app, "just now, no finish time", None);
    done_task(&app, "five hours", Some(5.0));

    let day = state(&app, "24h");
    assert_eq!(titles(&day), ["just now, no finish time", "an hour", "five hours"]);
    assert_eq!(day["counts"]["done_hidden"], json!(2));

    let week = state(&app, "7d");
    assert_eq!(titles(&week), ["just now, no finish time", "an hour", "five hours", "three days"]);
    assert_eq!(week["counts"]["done_hidden"], json!(1));

    let all = state(&app, "all");
    assert_eq!(titles(&all), ["just now, no finish time", "an hour", "five hours", "three days", "ten days"]);
    assert_eq!(all["counts"]["done_hidden"], json!(0));
    // Done isn't one of the counted columns.
    assert!(all["counts"].get("done").is_none());
}

#[test]
fn done_shows_at_most_200_the_newest() {
    let (app, _dir) = new_board();
    for i in 0..205 {
        done_task(&app, &format!("t{i}"), Some(i as f64 * 0.01));
    }
    let st = state(&app, "all");
    let shown = titles(&st);
    assert_eq!(shown.len(), 200);
    assert_eq!((shown[0].as_str(), shown[199].as_str()), ("t0", "t199"));
    assert_eq!(st["counts"]["done_hidden"], json!(5));
}

#[test]
fn goals_ride_along_unless_left_out() {
    let (app, _dir) = new_board();
    api::dispatch(&app, "POST", "/goals", &Query::new(), &json!({"name": "Sign in", "project": "webapp"})).unwrap();
    let with = |q: &[(&str, &str)]| -> Value {
        let q: Query = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        api::dispatch(&app, "GET", "state", &q, &json!({})).unwrap()["goals"].clone()
    };
    assert_eq!(with(&[])[0]["name"], "Sign in");
    assert_eq!(with(&[("goals", "0")]), json!([]), "the app keeps its own list from GET /goals");
}

fn names(app: &App) -> Vec<(String, Value)> {
    projects::list_projects(app).unwrap().iter().map(|p| (p["name"].as_str().unwrap().to_string(), p["path"].clone())).collect()
}

#[test]
fn the_project_list_follows_every_write() {
    let (app, _dir) = new_board();
    assert_eq!(names(&app), []);
    let id = app.db.insert("tasks", fields!["title" => "a", "project" => "webapp", "status" => "queued"]).unwrap();
    assert_eq!(names(&app), [("webapp".to_string(), Value::Null)]);
    app.db.x("UPDATE tasks SET repo_path = ? WHERE id = ?", taskboardd::p!["/src/webapp", id]).unwrap();
    assert_eq!(names(&app), [("webapp".to_string(), json!("/src/webapp"))]);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "api", "path": "/src/api"}]).to_string())).unwrap();
    assert_eq!(names(&app).len(), 2);
    app.db.x("DELETE FROM tasks WHERE id = ?", taskboardd::p![id]).unwrap();
    assert_eq!(names(&app), [("api".to_string(), json!("/src/api"))]);
}

#[test]
fn a_rolled_back_project_isnt_remembered() {
    let (app, _dir) = new_board();
    assert_eq!(names(&app), []);
    let r: taskboardd::util::Result<()> = app.db.tx(|| {
        app.db.insert("tasks", fields!["title" => "a", "project" => "ghost", "status" => "queued"])?;
        // Read inside the transaction, as a card built mid-request would.
        assert_eq!(names(&app).len(), 1);
        taskboardd::util::err(409, "no")
    });
    assert!(r.is_err());
    assert_eq!(names(&app), []);
}

#[test]
fn another_connections_commit_is_seen() {
    let (_, dir) = new_board();
    let app = App::new(Config::for_tests(dir.path()), false).unwrap();
    assert_eq!(names(&app), []);
    let other = rusqlite::Connection::open(dir.path().join("tasks.db")).unwrap();
    other.execute("INSERT INTO tasks (title, project, status) VALUES ('a', 'cli', 'queued')", []).unwrap();
    assert_eq!(names(&app), [("cli".to_string(), Value::Null)]);
}

fn get(app: &App, path: &str, since: Option<&Value>) -> Value {
    let q: Query = since.map(|s| ("since".to_string(), s.as_str().unwrap().to_string())).into_iter().collect();
    api::dispatch(app, "GET", path, &q, &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
}

#[test]
fn a_poll_since_the_stamp_it_holds_hears_unchanged_until_the_board_moves() {
    let (app, _dir) = new_board();
    app.db.insert("goals", fields!["name" => "Ship", "project" => "webapp", "created_at" => now_iso()]).unwrap();
    for path in ["state", "goals"] {
        let full = get(&app, path, None);
        let stamp = full["stamp"].clone();
        assert!(stamp.is_string(), "{path} comes stamped");
        let again = get(&app, path, Some(&stamp));
        assert_eq!(again["unchanged"], true, "{path}: nothing moved");
        assert_eq!(again["stamp"], stamp);
        assert!(again["now"].is_string(), "{path}: the clock still comes");
        assert!(again.get("columns").is_none() && again.get("goals").is_none(), "{path}: and nothing else");
        assert!(get(&app, path, Some(&json!("other"))).get("unchanged").is_none(), "{path}: another stamp gets it all");
    }
    let (state, goals) = (get(&app, "state", None)["stamp"].clone(), get(&app, "goals", None)["stamp"].clone());
    app.db.insert("goals", fields!["name" => "Polish", "project" => "webapp", "created_at" => now_iso()]).unwrap();
    app.db.insert("tasks", fields!["title" => "a", "project" => "webapp", "status" => "queued", "created_at" => now_iso()]).unwrap();
    let moved = get(&app, "state", Some(&state));
    assert!(moved.get("unchanged").is_none() && moved["stamp"] != state, "a new task moves the board");
    let moved = get(&app, "goals", Some(&goals));
    assert_eq!(moved["goals"].as_array().unwrap().len(), 2, "a new goal moves the list");
}
