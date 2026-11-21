//! A task moved to another project takes its history with it: its merged PR and the kept days it
//! shows on count under the new project, so nothing grouped by project keeps a stray one (such as a
//! Midna project id a task was first filed under). Tasks moved before that carry it on the next start.
//! One test per file: it moves the board's clock.

use chrono::{Duration, Local, TimeZone};
use serde_json::{json, Value};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{advance_clock, now_ts, reset_clock, RowExt};
use taskboardd::{api, board, days, fields, p, reproject};

fn get(app: &App, path: &str, query: &[(&str, &str)]) -> Value {
    let q = query.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    api::dispatch(app, "GET", path, &q, &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
}

fn post(app: &App, path: &str, body: Value) -> Value {
    api::dispatch(app, "POST", path, &Default::default(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
}

fn at(day: chrono::NaiveDate, hh: u32, mm: u32) {
    let want = Local.from_local_datetime(&day.and_hms_opt(hh, mm, 0).unwrap()).earliest().unwrap().timestamp() as f64;
    advance_clock(want - now_ts());
}

/// A task in `project` that works an hour on `day` and finishes with PR `pr` merged.
fn merged_task(app: &App, day: chrono::NaiveDate, project: &str, pr: i64) -> i64 {
    at(day, 9, 0);
    let id = post(app, "/tasks", json!({"title": format!("Ship PR {pr}"), "detail": "Do it.", "project": project, "status": "planned"}))["id"]
        .as_i64()
        .unwrap();
    at(day, 10, 0);
    board::update_task(app, id, fields!["status" => "working"]).unwrap();
    at(day, 11, 0);
    board::update_task(app, id, fields!["status" => "done", "pr_num" => pr]).unwrap();
    board::update_task(app, id, fields!["pr_phase" => "merged"]).unwrap();
    id
}

fn merges_by_project(app: &App) -> Vec<(String, i64)> {
    app.db.q("SELECT project, pr_num FROM pr_merges ORDER BY pr_num", p![]).unwrap().iter().map(|r| (r.st("project"), r.i0("pr_num"))).collect()
}

fn by_project(app: &App, day: chrono::NaiveDate) -> Value {
    get(app, "days", &[("date", &day.format("%Y-%m-%d").to_string())])["day"]["by_project"].clone()
}

#[test]
fn a_moved_task_takes_its_merge_and_days_with_it() {
    reset_clock();
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let projects: Vec<Value> = ["da_hunts", "raindrop"].iter().map(|n| json!({"name": n, "path": dir.path().join(n).to_string_lossy()})).collect();
    app.db.set_setting("midna_projects", Some(&json!(projects).to_string())).unwrap();
    let recent = days::today() - Duration::days(10);
    let old = days::today() - Duration::days(days::detail_days(&app) + 5);

    // Moved on the board: the merge and the kept day follow it.
    let moved = merged_task(&app, recent, "p_c1de7f", 1);
    reset_clock();
    assert_eq!(by_project(&app, recent), json!({"p_c1de7f": 60.0}), "the day is kept under the project it had");
    post(&app, &format!("tasks/T{moved}"), json!({"project": "da_hunts"}));
    assert_eq!(merges_by_project(&app), vec![("da_hunts".to_string(), 1)]);
    assert_eq!(by_project(&app, recent), json!({"da_hunts": 60.0}));
    assert_eq!(app.db.count("SELECT COUNT(*) FROM task_states WHERE task_id = ? AND project != 'da_hunts'", p![moved]).unwrap(), 0);

    // Moved before moves carried history: the next start carries it.
    let earlier = merged_task(&app, recent, "p_9985fa", 2);
    reset_clock();
    by_project(&app, recent);
    app.db.x("UPDATE tasks SET project = 'raindrop' WHERE id = ?", p![earlier]).unwrap();
    assert_eq!(reproject::carry_earlier_moves(&app).unwrap(), 1);
    assert_eq!(merges_by_project(&app), vec![("da_hunts".to_string(), 1), ("raindrop".to_string(), 2)]);
    assert_eq!(by_project(&app, recent), json!({"da_hunts": 60.0, "raindrop": 60.0}));
    assert_eq!(reproject::carry_earlier_moves(&app).unwrap(), 0, "once carried, it's done");

    // A day older than the kept log keeps its summary: there's nothing left to work it out again from.
    let gone = merged_task(&app, old, "p_aaaaaa", 3);
    reset_clock();
    by_project(&app, old);
    post(&app, &format!("tasks/T{gone}"), json!({"project": "da_hunts"}));
    assert_eq!(by_project(&app, old), json!({"p_aaaaaa": 60.0}));
    assert_eq!(merges_by_project(&app)[2], ("da_hunts".to_string(), 3), "its merge still moves");
}
