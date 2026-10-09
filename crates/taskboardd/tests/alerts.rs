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
