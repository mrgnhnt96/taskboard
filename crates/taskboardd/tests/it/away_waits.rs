//! A wait on the owner counts only while they could answer: not while the Mac slept, the owner was
//! away from it (no input), or outside the work hours. Those stretches draw as `away` bars.

use chrono::{Duration, Local, TimeZone};
use serde_json::{json, Value};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{advance_clock, now_ts, reset_clock};
use taskboardd::{api, board, clock, days, fields, hours};

fn at(day: chrono::NaiveDate, hh: u32, mm: u32) -> f64 {
    let want = Local.from_local_datetime(&day.and_hms_opt(hh, mm, 0).unwrap()).earliest().unwrap().timestamp() as f64;
    advance_clock(want - now_ts());
    want
}

fn set(app: &App, id: i64, status: &str, extra: Vec<(&str, Value)>) {
    let mut f = fields!["status" => status];
    f.extend(extra);
    board::update_task(app, id, f).unwrap();
}

#[test]
fn a_wait_leaves_out_time_away_and_outside_work_hours() {
    reset_clock();
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    hours::set(&app, &json!({"on": true, "start": "09:00", "end": "17:00", "days": "all"})).unwrap();
    let day = days::today() - Duration::days(61);
    let ds = day.format("%Y-%m-%d").to_string();

    at(day, 7, 50);
    let q = Default::default();
    let id = api::dispatch(&app, "POST", "/tasks", &q, &json!({"title": "Pick a colour", "detail": "Do it.", "project": "webapp", "status": "planned"})).unwrap()["id"]
        .as_i64()
        .unwrap();
    at(day, 8, 0);
    set(&app, id, "needs", vec![("needs_reason", json!("question"))]);
    let (gone, back) = (at(day, 10, 0), at(day, 10, 30));
    clock::note_away(gone, back);
    at(day, 12, 0);
    set(&app, id, "working", vec![("needs_reason", Value::Null)]);
    at(day, 12, 30);
    set(&app, id, "done", vec![]);
    reset_clock();

    let page = api::dispatch(&app, "GET", "days", &[("date".to_string(), ds)].into_iter().collect(), &json!({})).unwrap();
    clock::forget_aways();
    let lane = page["day"]["lanes"].as_array().unwrap().iter().find(|l| l["project"] == "webapp").unwrap().clone();
    let kinds: Vec<&str> = lane["bars"].as_array().unwrap().iter().map(|b| b["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, vec!["away", "needs", "away", "needs", "done"], "8 to 9 is before the hours, 10 to 10:30 away");
    assert_eq!(page["day"]["wait_min"], 150.0, "9 to 10, then 10:30 to 12");
}
