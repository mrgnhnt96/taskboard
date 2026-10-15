//! Areas (`.taskboard.toml` at a project's repo root): tasks, goals and backlog issues carry one of
//! the project's areas, matched any case, cleared with `none`, refused when the project has no such area.

use rusqlite::Connection;
use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::import;
use taskboardd::util::RowExt;

struct Board {
    app: std::sync::Arc<App>,
    repo: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("rune");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        repo.join(".taskboard.toml"),
        "[[areas]]\nname = \"Quill\"\npaths = [\"apps/quill\", \"packages/quill_*\"]\n\n[[areas]]\nname = \"Art\"\npaths = [\"apps/art\"]\n",
    )
    .unwrap();
    let plain = dir.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    app.db
        .set_setting(
            "midna_projects",
            Some(&json!([{"name": "rune", "path": repo.to_string_lossy()}, {"name": "plain", "path": plain.to_string_lossy()}]).to_string()),
        )
        .unwrap();
    Board { app, repo, _dir: dir }
}

impl Board {
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn refused(&self, path: &str, body: Value) -> String {
        let e = api::dispatch(&self.app, "POST", path, &Query::new(), &body).expect_err("should be refused");
        assert_eq!(e.status, 400, "{}", e.message);
        e.message
    }
}

#[test]
fn tasks_goals_and_issues_take_one_of_the_projects_areas() {
    let b = new_board();
    let t = b.post("tasks", json!({"title": "Pen pressure", "project": "rune", "area": "quill"}));
    assert_eq!(t["area"], "Quill", "matched any case, kept in the area's own spelling");
    let tid = t["id"].as_i64().unwrap();
    assert_eq!(b.get(&format!("tasks/T{tid}"))["area"], "Quill");

    let msg = b.refused(&format!("tasks/T{tid}"), json!({"area": "Remote"}));
    assert!(msg.contains("Quill, Art"), "{msg}");
    assert_eq!(b.post(&format!("tasks/T{tid}"), json!({"area": "ART"}))["area"], "Art");
    assert_eq!(b.post(&format!("tasks/T{tid}"), json!({"area": "none"}))["area"], Value::Null);

    let g = b.post("goals", json!({"name": "Brushes", "project": "rune", "area": "art"}));
    assert_eq!(g["area"], "Art");
    let gid = g["id"].as_i64().unwrap();
    assert_eq!(b.post(&format!("goals/G{gid}"), json!({"area": "Quill"}))["area"], "Quill");
    assert_eq!(b.post(&format!("goals/G{gid}"), json!({"project": "plain"}))["area"], Value::Null, "plain has no Quill");

    let i = b.post("backlog", json!({"title": "Eraser lags", "project": "rune", "area": "QUILL"}));
    assert_eq!(i["area"], "Quill");
    let iid = i["id"].as_i64().unwrap();
    assert_eq!(b.post(&format!("backlog/B{iid}"), json!({"area": "none"}))["area"], Value::Null);
    b.post(&format!("backlog/B{iid}"), json!({"area": "art"}));
    let made = b.post(&format!("backlog/B{iid}/promote"), json!({"where": "board"}));
    let task = made["task"]["id"].as_i64().or_else(|| b.get(&format!("backlog/B{iid}"))["task_id"].as_i64()).unwrap();
    assert_eq!(taskboardd::board::get_task(&b.app, task).unwrap().st("area"), "Art", "the task keeps the issue's area");

    let msg = b.refused("tasks", json!({"title": "x", "project": "plain", "area": "Quill"}));
    assert!(msg.contains("plain has no areas"), "{msg}");
    assert_eq!(b.post("tasks", json!({"title": "y", "project": "plain", "area": "none"}))["area"], Value::Null);
}

#[test]
fn the_projects_list_shows_its_areas_and_follows_the_file() {
    let b = new_board();
    let rune = |b: &Board| b.get("projects")["projects"].as_array().unwrap().iter().find(|p| p["name"] == "rune").unwrap().clone();
    let p = rune(&b);
    assert_eq!(p["areas"], json!([{"name": "Quill", "paths": ["apps/quill", "packages/quill_*"]}, {"name": "Art", "paths": ["apps/art"]}]));
    assert_eq!(p["area_problems"], json!([]));
    std::fs::write(b.repo.join(".taskboard.toml"), "[[areas]]\nname = \"Remote\"\npaths = [\"apps/remote\", \"apps/remote_server\"]\n").unwrap();
    assert_eq!(rune(&b)["areas"][0]["name"], "Remote");
    assert_eq!(b.post("tasks", json!({"title": "Pair", "project": "rune", "area": "remote"}))["area"], "Remote");
}

#[test]
fn the_tasks_page_counts_a_projects_cards_by_area() {
    let b = new_board();
    for (title, area) in [("Brush", "Art"), ("Pen", "quill"), ("Ink", "Quill"), ("Plain", "none")] {
        let id = b.post("tasks", json!({"title": title, "project": "rune", "area": area}))["id"].as_i64().unwrap();
        taskboardd::board::update_task(&b.app, id, vec![("status", json!("queued"))]).unwrap();
    }
    let lanes = b.get("lanes");
    let rune = lanes["projects"].as_array().unwrap().iter().find(|p| p["name"] == "rune").unwrap();
    assert_eq!(rune["count"], 4);
    // In the file's order (Quill, then Art); the one with no area counts only toward the project.
    assert_eq!(rune["areas"], json!([{"name": "Quill", "count": 2}, {"name": "Art", "count": 1}]));
    assert_eq!(lanes["queued"][0]["area"], "Art", "each card carries its area");
}

#[test]
fn an_old_boards_areas_come_over() {
    let old_dir = tempfile::tempdir().unwrap();
    let old_path = old_dir.path().join("tasks.db");
    Connection::open(&old_path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE goals(id INTEGER PRIMARY KEY, name TEXT, project TEXT, area TEXT);
             CREATE TABLE tasks(id INTEGER PRIMARY KEY, title TEXT, project TEXT, status TEXT, goal_id INT, area TEXT);
             INSERT INTO goals VALUES (3, 'Brushes', 'rune', 'Art');
             INSERT INTO tasks VALUES (7, 'Pen', 'rune', 'done', 3, 'Quill');
             INSERT INTO tasks VALUES (8, 'Plain', 'rune', 'done', NULL, NULL);",
        )
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    import::import(&app, &old_path).unwrap();
    assert_eq!(taskboardd::board::get_goal(&app, 3).unwrap().st("area"), "Art");
    assert_eq!(taskboardd::board::get_task(&app, 7).unwrap().st("area"), "Quill");
    assert!(taskboardd::board::get_task(&app, 8).unwrap().s("area").is_none());
}
