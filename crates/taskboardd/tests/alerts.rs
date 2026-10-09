//! Alerts: urgent ones (first, can't be dismissed, repeat outside the work hours), keyed alerts raised
//! through `POST /alerts`, and the notification's id and snooze buttons and what a pick does.

use chrono::Datelike;
use serde_json::{json, Value};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{iso, local_now, now_ts};
use taskboardd::{api, dispatch, hours};

fn get(app: &App, path: &str) -> Value {
    api::dispatch(app, "GET", path, &Default::default(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
}

fn post(app: &App, path: &str, body: Value) -> Result<Value, (u16, String)> {
    api::dispatch(app, "POST", path, &Default::default(), &body).map_err(|e| (e.status, e.message))
}

/// Makes every alert look notified `mins` minutes ago.
fn age_alerts(app: &App, mins: f64) {
    let mut rows = dispatch::alerts(app);
    for a in rows.iter_mut() {
        a["notified_at"] = json!(iso(now_ts() - mins * 60.0));
    }
    app.db.set_setting("alerts", Some(&Value::Array(rows).to_string())).unwrap();
}

#[test]
fn urgent_alerts_stay_sort_first_and_repeat_outside_hours() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));

    post(&app, "alerts", json!({"text": "Couldn't reach the PR host"})).unwrap();
    let urgent = post(&app, "alerts", json!({"text": "main is red on web", "urgent": true, "key": "main:web"})).unwrap()["alert"].clone();
    assert_eq!(urgent["urgent"], true);
    assert_eq!(urgent["key"], "main:web");
    let again = post(&app, "alerts", json!({"text": "main is red on web", "urgent": true, "key": "main:web"})).unwrap();
    assert_eq!(again["alert"]["id"], urgent["id"], "a key is raised once");

    let state = get(&app, "state");
    let alerts = state["alerts"].as_array().unwrap();
    assert_eq!(alerts.len(), 2);
    assert_eq!(alerts[0]["id"], urgent["id"], "urgent alerts come first");

    let id = urgent["id"].as_str().unwrap();
    let (status, msg) = post(&app, &format!("alerts/{id}/dismiss"), json!({})).unwrap_err();
    assert_eq!(status, 409);
    assert!(msg.contains("urgent"), "{msg}");

    // Outside the work hours only the urgent alert repeats.
    let today = hours::DAYS[local_now().weekday().num_days_from_monday() as usize];
    let others: Vec<&str> = hours::DAYS.iter().copied().filter(|d| *d != today).collect();
    post(&app, "hours", json!({"on": true, "start": "09:00", "end": "17:00", "days": others})).unwrap();
    assert!(!hours::is_open(&app));
    age_alerts(&app, 30.0);
    let before: Vec<Value> = dispatch::alerts(&app);
    dispatch::tick(&app).unwrap();
    let after = dispatch::alerts(&app);
    let notified = |rows: &[Value], urgent: bool| rows.iter().find(|a| (a["urgent"] == true) == urgent).unwrap()["notified_at"].clone();
    assert_ne!(notified(&after, true), notified(&before, true), "the urgent alert repeated");
    assert_eq!(notified(&after, false), notified(&before, false), "the other waits for the work hours");

    // Whoever raised it clears it, by key.
    post(&app, "alerts/main:web/clear", json!({})).unwrap();
    assert!(dispatch::alerts(&app).iter().all(|a| a["urgent"] != true));
    assert_eq!(post(&app, "alerts/main:web/clear", json!({})).unwrap_err().0, 404);
}

#[test]
fn notifications_carry_the_alert_id_and_snooze_buttons() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.alerts.snooze_mins = vec![5, 30, 120];
    let app = App::for_tests(cfg);
    let a = post(&app, "alerts", json!({"text": "T1 didn't start"})).unwrap()["alert"].clone();
    let id = a["id"].as_str().unwrap();

    let p = dispatch::alert_params(&app, &a);
    assert_eq!(p["id"], dispatch::notification_id(id));
    assert_eq!(p["actions"], json!(["Snooze 5 min", "Snooze 30 min", "Snooze 2 hours"]));
    assert_eq!(dispatch::snooze_label(60), "Snooze 1 hour");

    // A click or a dismissal leaves it alone; a snooze button snoozes it.
    assert!(!dispatch::handle_response(&app, id, &json!({"response": {"kind": "clicked"}})).unwrap());
    assert!(!dispatch::handle_response(&app, id, &json!({"response": {"kind": "action", "action": "Snooze 15 min"}})).unwrap(), "not a configured button");
    assert!(dispatch::handle_response(&app, id, &json!({"response": {"kind": "action", "action": "Snooze 2 hours"}})).unwrap());
    let snoozed = dispatch::alerts(&app)[0]["snoozed_until"].as_str().map(str::to_string).expect("snoozed");
    let until = taskboardd::util::parse_iso(&snoozed).unwrap();
    assert!((until - now_ts() - 7200.0).abs() < 5.0);
    assert!(post(&app, &format!("alerts/{id}/snooze"), json!({"mins": 120})).is_ok(), "the API takes the configured snoozes");

    // No buttons when there are no snoozes.
    let dir2 = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir2.path());
    cfg.alerts.snooze_mins = vec![];
    let quiet = App::for_tests(cfg);
    assert!(dispatch::alert_params(&quiet, &a).get("actions").is_none());
}

fn board_with_task() -> (tempfile::TempDir, std::sync::Arc<App>, i64) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    let t = post(&app, "tasks", json!({"title": "Fix the header", "detail": "Do it.", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    (dir, app, t)
}

fn texts(app: &App) -> Vec<String> {
    let mut v: Vec<String> = dispatch::alerts(app).iter().map(|a| a["text"].as_str().unwrap().to_string()).collect();
    v.sort();
    v
}

#[test]
fn an_urgent_alert_keeps_the_tasks_alerts_and_holds_back_new_ones() {
    let (_dir, app, t) = board_with_task();
    let task = format!("T{t}");
    app.db.tx(|| dispatch::add_alert(&app, "T1 didn't start.", Some(t), None, None, None)).unwrap();
    post(&app, "alerts", json!({"text": "main is red", "urgent": true, "key": "main:web", "task": task})).unwrap();
    assert_eq!(texts(&app), vec!["T1 didn't start.", "main is red"], "raising an urgent alert keeps the task's alerts");

    app.db.tx(|| dispatch::add_alert(&app, "T1 has a question.", Some(t), None, None, None)).unwrap();
    assert_eq!(texts(&app), vec!["T1 didn't start.", "main is red"], "no new plain alert while the urgent one is up");
    let (code, msg) = post(&app, "alerts", json!({"text": "Another", "task": task})).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("urgent"), "{msg}");
    app.db.tx(|| dispatch::add_alert(&app, "PR #1 is ready for review.", Some(t), None, None, Some("review"))).unwrap();
    assert!(texts(&app).contains(&"PR #1 is ready for review.".to_string()), "a review alert still comes in");

    // Clearing the task's alerts leaves the urgent one (and the review one) up.
    app.db.tx(|| dispatch::clear_alerts(&app, Some(t), None)).unwrap();
    assert_eq!(texts(&app), vec!["PR #1 is ready for review.", "main is red"]);

    // Cleared by its key, the task's alerts come in again.
    post(&app, "alerts/main:web/clear", json!({})).unwrap();
    app.db.tx(|| dispatch::add_alert(&app, "T1 has a question.", Some(t), None, None, None)).unwrap();
    assert!(texts(&app).contains(&"T1 has a question.".to_string()));
}

#[test]
fn a_notifications_snooze_counts_once_however_late() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let a = post(&app, "alerts", json!({"text": "T1 didn't start"})).unwrap()["alert"].clone();
    let id = a["id"].as_str().unwrap();
    let mins = app.cfg.alerts.snooze_mins.iter().copied().find(|m| *m > 0).unwrap();
    let pick = json!({"response": {"kind": "action", "action": dispatch::snooze_label(mins)}, "sent_at": a["notified_at"]});
    assert!(dispatch::handle_response(&app, id, &pick).unwrap());
    assert!(!dispatch::handle_response(&app, id, &pick).unwrap(), "the same notification's pick counts once");
    let mut again = pick.clone();
    again["sent_at"] = json!("2099-01-01T00:00:00Z");
    assert!(dispatch::handle_response(&app, id, &again).unwrap(), "the next notification's pick counts");

    // One waiter per alert: a repeat only moves it on to the newer notification.
    let waiters = app.shared.lock().alert_waiters.clone();
    assert!(dispatch::Waiter::start(waiters.clone(), id, "a").is_some());
    assert!(dispatch::Waiter::start(waiters.clone(), id, "b").is_none());
    assert_eq!(waiters.lock().get(id).map(String::as_str), Some("b"));
    post(&app, &format!("alerts/{id}/dismiss"), json!({})).unwrap();
    dispatch::tick(&app).unwrap();
    assert!(!waiters.lock().contains_key(id), "its waiter stops once the alert is gone");
}
