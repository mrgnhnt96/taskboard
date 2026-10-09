//! The Days page: a day worked out from status changes and the task log, kept as a summary, and
//! still there after the cleanup removes its events. One test per file: it moves the board's clock.

use chrono::{Duration, Local, TimeZone};
use serde_json::{json, Value};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{advance_clock, now_ts, reset_clock, RowExt};
use taskboardd::{api, board, days, fields, midna, p, reports, runner};

fn get(app: &App, path: &str, query: &[(&str, &str)]) -> Value {
    let q = query.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    api::dispatch(app, "GET", path, &q, &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
}

fn post(app: &App, path: &str, body: Value) -> Result<Value, String> {
    api::dispatch(app, "POST", path, &Default::default(), &body).map_err(|e| e.message)
}

/// Move the board's clock to `hh:mm` local time on `day`.
fn at(day: chrono::NaiveDate, hh: u32, mm: u32) {
    let want = Local.from_local_datetime(&day.and_hms_opt(hh, mm, 0).unwrap()).earliest().unwrap().timestamp() as f64;
    advance_clock(want - now_ts());
}

fn task(app: &App, title: &str, project: &str) -> i64 {
    post(app, "/tasks", json!({"title": title, "detail": "Do it.", "project": project, "status": "planned"})).unwrap()["id"].as_i64().unwrap()
}

fn set(app: &App, id: i64, status: &str, extra: Vec<(&str, Value)>) {
    let mut f = fields!["status" => status];
    f.extend(extra);
    board::update_task(app, id, f).unwrap();
}

#[test]
fn a_day_is_worked_out_kept_and_survives_the_cleanup() {
    reset_clock();
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let day = days::today() - Duration::days(40);
    let ds = day.format("%Y-%m-%d").to_string();

    at(day, 9, 50);
    let a = task(&app, "Add login", "webapp");
    let b = task(&app, "Speed up search", "api");
    at(day, 10, 0);
    set(&app, a, "working", vec![]);
    at(day, 10, 15);
    board::log_event(&app, a, "T", "commit", "Committed: Add the form").unwrap();
    at(day, 10, 30);
    set(&app, b, "working", vec![]);
    at(day, 10, 45);
    board::log_event(&app, a, "T", "commit", "Committed: Wire the API").unwrap();
    board::log_event(&app, a, "T", "commit", "Pushed feature/x").unwrap();
    at(day, 11, 0);
    board::log_event(&app, a, "T", "question", "Asked: Which colour?").unwrap();
    set(&app, a, "needs", vec![("needs_reason", json!("question"))]);
    at(day, 11, 20);
    set(&app, a, "working", vec![("needs_reason", Value::Null)]);
    at(day, 11, 30);
    board::log_event(&app, b, board::MIDNA, "status", "Terminal ended (closed) before the task was done; queued to start again from its handoff").unwrap();
    set(&app, b, "queued", vec![]);
    at(day, 11, 40);
    set(&app, b, "working", vec![]);
    at(day, 11, 55);
    board::log_event(&app, a, "T", "status", "Linked PR #7 (acme/webapp)").unwrap();
    at(day, 12, 0);
    set(&app, a, "done", vec![("human_min", json!(240))]);
    at(day, 13, 0);
    set(&app, b, "done", vec![]);
    reset_clock();

    let page = get(&app, "days", &[("date", &ds)]);
    let d = &page["day"];
    assert_eq!(page["date"], ds.as_str());
    assert_eq!(page["is_today"], false);
    assert_eq!(d["agent_min"], 240.0, "Add login 60 + 40, Speed up search 60 + 80");
    assert_eq!(d["done"], 2.0);
    assert_eq!(d["commits"], 2.0, "a push isn't a commit");
    assert_eq!(d["prs"], 1.0);
    assert_eq!(d["questions"], 1.0);
    assert_eq!(d["wait_min"], 20.0);
    assert_eq!(d["peak"], 2);
    assert_eq!(d["human_min"], 240.0);
    assert_eq!(d["est_agent_min"], 100.0, "only the estimated task's own work");
    assert_eq!(d["by_project"], json!({"api": 140.0, "webapp": 100.0}));
    assert_eq!(page["projects"], json!(["api", "webapp"]));
    assert!(page["from_hour"].as_i64().unwrap() <= 9 && page["to_hour"].as_i64().unwrap() >= 13);

    let lanes = d["lanes"].as_array().unwrap();
    assert_eq!(lanes.len(), 2);
    let api_bars: Vec<&str> = lanes[0]["bars"].as_array().unwrap().iter().map(|b| b["kind"].as_str().unwrap()).collect();
    assert_eq!(api_bars, vec!["lost", "done"], "the closed terminal ends the first stretch");
    let web_bars: Vec<&str> = lanes[1]["bars"].as_array().unwrap().iter().map(|b| b["kind"].as_str().unwrap()).collect();
    assert_eq!(web_bars, vec!["working", "needs", "done"]);
    let kinds: Vec<&str> = lanes[1]["marks"].as_array().unwrap().iter().map(|m| m["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, vec!["commit", "commit", "question", "pr", "done"]);
    assert_eq!(d["waits"][0]["text"], "Which colour?");
    assert_eq!(d["waits"][0]["min"], 20.0);

    // The week has the day; the week before is empty; a hidden project drops out everywhere.
    let k = page["week"].as_array().unwrap().iter().position(|w| w["date"] == ds.as_str()).unwrap();
    assert_eq!(page["week"][k]["agent_min"], 240.0);
    assert_eq!(page["last_week"][k]["agent_min"], 0.0);
    assert_eq!(page["week_tasks"][0]["ref"], "T2", "the longest task first");
    // Weeks run Sunday to Saturday unless config.toml's first_weekday says otherwise.
    let first = chrono::NaiveDate::parse_from_str(page["week"][0]["date"].as_str().unwrap(), "%Y-%m-%d").unwrap();
    assert_eq!(chrono::Datelike::weekday(&first), chrono::Weekday::Sun);
    assert_eq!(page["first_weekday"], "sun");
    let hidden = get(&app, "days", &[("date", &ds), ("hide", "api")]);
    assert_eq!(hidden["day"]["agent_min"], 100.0);
    assert_eq!(hidden["week"][k]["done"], 1.0);
    assert_eq!(hidden["day"]["lanes"].as_array().unwrap().len(), 1);
    assert_eq!(hidden["projects"], json!(["api", "webapp"]), "hidden projects can still be shown again");

    // The past day is kept.
    assert!(app.db.count("SELECT COUNT(*) FROM day_stats WHERE date = ?", p![ds]).unwrap() >= 2);

    // The cleanup removes its events after 30 days; the day's totals and bars stay.
    post(&app, "/history", json!({"detail_days": 30})).unwrap();
    assert!(post(&app, "/history", json!({"detail_days": 12})).is_err());
    let out = post(&app, "/history/cleanup", json!({})).unwrap();
    assert!(out["removed"]["events"].as_i64().unwrap() >= 5);
    assert_eq!(app.db.count("SELECT COUNT(*) FROM events WHERE task_id = ?", p![a]).unwrap(), 0);
    let after = get(&app, "days", &[("date", &ds)]);
    assert_eq!(after["day"]["agent_min"], 240.0);
    assert_eq!(after["day"]["lanes"][1]["bars"].as_array().unwrap().len(), 3);
    let marks: Vec<&str> = after["day"]["lanes"][1]["marks"].as_array().unwrap().iter().map(|m| m["kind"].as_str().unwrap()).collect();
    assert_eq!(marks, vec!["done"], "only what the summary keeps");
    assert_eq!(get(&app, "history", &[])["detail_days"], 30);

    // Summaries go after `summary_days`.
    post(&app, "/history", json!({"summary_days": 180})).unwrap();
    app.db.x("UPDATE day_stats SET date = '2020-01-01' WHERE date = ?", p![ds]).unwrap();
    post(&app, "/history/cleanup", json!({})).unwrap();
    assert_eq!(app.db.count("SELECT COUNT(*) FROM day_stats WHERE date = '2020-01-01'", p![]).unwrap(), 0);

    human_estimate_from_tb_done();
}

/// `tb done --human 3h` keeps the estimate; a length the board can't read is refused.
fn human_estimate_from_tb_done() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    let id = post(&app, "/tasks", json!({"title": "Add login", "detail": "Do it.", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    runner::start_queued(&app).unwrap();
    let job = app.db.q("SELECT * FROM jobs WHERE kind = 'agent'", p![]).unwrap().remove(0);
    let prompt = board::job_args(&job).st("prompt");
    midna::sync(&app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    let report = |event: &str, extra: Value| {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": ""});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&app, b, false)
    };
    report("hook.prompt", json!({"prompt": prompt})).unwrap();
    let e = report("tb.done", json!({"summary": "Works", "human": "soon"})).unwrap_err();
    assert!(e.message.contains("like 3h"), "{}", e.message);
    assert_eq!(board::get_task(&app, id).unwrap().s("status"), Some("working"));
    report("tb.done", json!({"summary": "Works", "human": "1h30m"})).unwrap();
    let t = board::get_task(&app, id).unwrap();
    assert_eq!(t.s("status"), Some("done"));
    assert_eq!(t.i("human_min"), Some(90));
    let today = get(&app, "days", &[]);
    assert_eq!(today["is_today"], true);
    assert_eq!(today["day"]["human_min"], 90.0);
    assert_eq!(today["day"]["done"], 1.0);
}
