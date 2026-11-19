//! The Days page's Waiting on you is the time at least one task waited on the owner's answer that
//! day: a wait carried past midnight counts only its part of each day, tasks waiting side by side
//! count once, and human tasks and lost terminals don't count. One test per file: it moves the
//! board's clock.

use chrono::{Duration, Local, TimeZone};
use serde_json::{json, Value};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{advance_clock, now_ts, reset_clock, now_iso};
use taskboardd::{api, board, days, fields, p};

fn at(day: chrono::NaiveDate, hh: u32, mm: u32) {
    let want = Local.from_local_datetime(&day.and_hms_opt(hh, mm, 0).unwrap()).earliest().unwrap().timestamp() as f64;
    advance_clock(want - now_ts());
}

fn task(app: &App, title: &str, project: &str) -> i64 {
    let q = Default::default();
    api::dispatch(app, "POST", "/tasks", &q, &json!({"title": title, "detail": "Do it.", "project": project, "status": "planned"})).unwrap()["id"].as_i64().unwrap()
}

fn set(app: &App, id: i64, status: &str, extra: Vec<(&str, Value)>) {
    let mut f = fields!["status" => status];
    f.extend(extra);
    board::update_task(app, id, f).unwrap();
}

fn day_of(app: &App, d: chrono::NaiveDate) -> Value {
    let q = [("date".to_string(), d.format("%Y-%m-%d").to_string())].into_iter().collect();
    api::dispatch(app, "GET", "days", &q, &json!({})).unwrap()["day"].clone()
}

#[test]
fn waiting_on_you_counts_each_day_once_and_only_answers() {
    reset_clock();
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let day = days::today() - Duration::days(10);
    let next = day + Duration::days(1);

    at(day, 8, 0);
    let a = task(&app, "Pick a colour", "webapp");
    let b = task(&app, "Name the flag", "api");
    let c = task(&app, "Renew the cert", "webapp");
    let e = task(&app, "Tidy logs", "api");
    at(day, 9, 0);
    set(&app, c, "needs", vec![("needs_reason", json!("human"))]);
    set(&app, e, "needs", vec![("needs_reason", json!("lost"))]);
    at(day, 10, 0);
    set(&app, a, "needs", vec![("needs_reason", json!("question"))]);
    at(day, 10, 30);
    set(&app, b, "needs", vec![("needs_reason", json!("question"))]);
    at(day, 11, 0);
    set(&app, b, "working", vec![("needs_reason", Value::Null)]);
    at(next, 10, 0);
    set(&app, a, "working", vec![("needs_reason", Value::Null)]);
    reset_clock();

    // A waits 10:00 to midnight; B's half hour sits inside it; the human task and lost terminal don't count.
    let d = day_of(&app, day);
    assert_eq!(d["wait_min"], 840.0);
    assert_eq!(d["questions"], 2.0);
    // The next day counts only A's last ten hours, and A isn't a new wait there.
    let n = day_of(&app, next);
    assert_eq!(n["wait_min"], 600.0);
    assert_eq!(n["questions"], 0.0);
    assert!(n["waits"].as_array().unwrap().is_empty());

    // A day kept before waits were kept as stretches is worked out again, not read back.
    let old = day - Duration::days(1);
    let os = old.format("%Y-%m-%d").to_string();
    app.db.x("DELETE FROM day_stats WHERE date = ?", p![os]).unwrap();
    app.db
        .x(
            "INSERT INTO day_stats(date, project, data, at) VALUES(?, 'webapp', ?, ?)",
            p![os, json!({"agent_min": 0.0, "waits": [{"task": a, "min": 44832.0}]}).to_string(), now_iso()],
        )
        .unwrap();
    assert_eq!(day_of(&app, old)["wait_min"], 0.0);
}
