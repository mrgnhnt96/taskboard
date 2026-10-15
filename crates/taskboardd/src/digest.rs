//! One notification for everything waiting on the owner. With `[alerts] digest_mins` above 0, an alert that isn't
//! urgent doesn't notify on its own: new ones gather for `digest_mins`, then go out together as one notification
//! (Midna's `taskboard-digest`, replaced each time) that opens Home's Needs you list. It repeats every
//! `alert_every_mins` inside the work hours while anything is left, snoozes as one, and is taken away once
//! nothing is. An alert snoozed on its own sits out of the digest until its snooze runs out.

use serde_json::{json, Value};

use crate::app::App;
use crate::dispatch::{self, Waiter};
use crate::util::*;
use crate::{hours, midna};

/// The key the digest's waiter and its notification responses go by, apart from alerts' ids.
pub const KEY: &str = "digest";

/// Midna's id for the digest's notification.
pub const NOTIFICATION_ID: &str = "taskboard-digest";

/// The most alerts the notification lists by name; the rest are counted.
const SHOWN: usize = 5;

/// Whether the digest is on.
pub fn on(app: &App) -> bool {
    app.cfg.alerts.digest_mins > 0
}

/// Whether alert `a` goes out in the digest rather than on its own.
pub fn holds(app: &App, a: &Value) -> bool {
    on(app) && a["urgent"] != true
}

/// The digest's last notification: `sent` (the alerts it listed), `notified_at`, `snoozed_until` and
/// `answered` (the `notified_at` whose snooze was already taken).
pub fn state(app: &App) -> Value {
    let s = app.db.get_setting("alert_digest").ok().flatten();
    s.and_then(|s| serde_json::from_str(&s).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}))
}

fn save(app: &App, s: &Value) -> Result<()> {
    app.db.set_setting("alert_digest", Some(&jdumps(s)))
}

fn snoozed(a: &Value, now: f64) -> bool {
    a["snoozed_until"].as_str().and_then(parse_iso).is_some_and(|s| s > now)
}

/// The alerts the digest lists now, in the app's order.
pub fn members(app: &App) -> Vec<Value> {
    let now = now_ts();
    dispatch::listing(app).into_iter().filter(|a| holds(app, a) && !snoozed(a, now)).collect()
}

fn ids(rows: &[Value]) -> Vec<String> {
    rows.iter().filter_map(|a| a["id"].as_str().map(str::to_string)).collect()
}

/// "T12 needs you", "3 things need you".
pub fn title(rows: &[Value]) -> String {
    match rows {
        [one] => match one["task"].as_str() {
            Some(t) => format!("{t} needs you"),
            None => "1 thing needs you".into(),
        },
        _ => format!("{} things need you", rows.len()),
    }
}

/// One line per alert, the first `SHOWN` of them, then how many more.
pub fn body(rows: &[Value]) -> String {
    let mut lines: Vec<String> = rows.iter().take(SHOWN).map(|a| one_line(a["text"].as_str().unwrap_or(""), 120)).collect();
    if rows.len() > SHOWN {
        lines.push(format!("and {} more", rows.len() - SHOWN));
    }
    lines.join("\n")
}

/// `notify.send`'s params for the digest of `rows`.
pub fn params(app: &App, rows: &[Value]) -> Value {
    let mut p = json!({"title": title(rows), "body": body(rows), "category": "board", "sound": true,
                       "open": dispatch::alert_url(app, None, None, None), "id": NOTIFICATION_ID});
    let actions = dispatch::snooze_actions(app);
    if !actions.is_empty() {
        p["actions"] = json!(actions);
    }
    p
}

/// Whether the digest goes out now: an alert it hasn't listed has waited `digest_mins`, its snooze ran out,
/// or `alert_every_mins` passed since it last went out.
fn due(app: &App, s: &Value, rows: &[Value], now: f64) -> bool {
    let sent: Vec<&str> = s["sent"].as_array().map(|v| v.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let gather = app.cfg.alerts.digest_mins as f64 * 60.0;
    let fresh = rows.iter().filter(|a| !a["id"].as_str().is_some_and(|id| sent.contains(&id)));
    if fresh.filter_map(|a| age_secs(a["at"].as_str())).any(|age| age >= gather) {
        return true;
    }
    if sent.is_empty() {
        return false;
    }
    if let Some(until) = s["snoozed_until"].as_str().and_then(parse_iso) {
        return now >= until;
    }
    let every = hours::get(app).alert_every_mins as f64 * 60.0;
    every > 0.0 && age_secs(s["notified_at"].as_str()).is_some_and(|age| age >= every)
}

/// Sends the digest when it's due inside the work hours, and takes it away once nothing is left.
pub fn tick(app: &App) -> Result<()> {
    if !on(app) {
        return Ok(());
    }
    let rows = members(app);
    let s = state(app);
    if rows.is_empty() {
        if s.get("notified_at").is_some() {
            save(app, &json!({}))?;
            dispatch::withdraw_id(app, NOTIFICATION_ID);
        }
        return Ok(());
    }
    if !hours::is_open(app) || !due(app, &s, &rows, now_ts()) {
        return Ok(());
    }
    let at = now_iso();
    save(app, &json!({"sent": ids(&rows), "notified_at": at, "answered": s["answered"]}))?;
    send(app, &rows, &at);
    Ok(())
}

fn send(app: &App, rows: &[Value], sent_at: &str) {
    if !app.cfg.notify || !app.cfg.runner {
        return;
    }
    let params = params(app, rows);
    let wait = params.get("actions").is_some();
    let (midna, inbox, waiters) = {
        let sh = app.shared.lock();
        (app.cfg.midna.clone(), sh.notify_inbox.clone(), sh.alert_waiters.clone())
    };
    let sent_at = sent_at.to_string();
    std::thread::spawn(move || {
        if midna::call_exe(&midna, "notify.send", &params, 30.0).is_err() || !wait {
            return;
        }
        if let Some(w) = Waiter::start(waiters, KEY, &sent_at, NOTIFICATION_ID) {
            w.listen(&midna, &inbox);
        }
    });
}

/// A snooze button on the digest snoozes the digest; each notification's pick counts once (`sent_at`).
pub fn handle_response(app: &App, response: &Value) -> Result<bool> {
    let r = response.get("response").filter(|r| r.is_object()).unwrap_or(response);
    if r["kind"] != "action" {
        return Ok(false);
    }
    let Some(mins) = r["action"].as_str().and_then(|l| dispatch::snooze_mins(app, l)) else { return Ok(false) };
    let mut s = state(app);
    if s.get("notified_at").is_none() {
        return Ok(false);
    }
    let sent_at = response.get("sent_at").and_then(|x| x.as_str());
    if sent_at.is_some() && s["answered"].as_str() == sent_at {
        return Ok(false);
    }
    s["snoozed_until"] = json!(iso(now_ts() + mins as f64 * 60.0));
    if let Some(at) = sent_at {
        s["answered"] = json!(at);
    }
    save(app, &s)?;
    Ok(true)
}

/// When the digest last went out, while it's showing: its waiter keeps listening until then.
pub fn showing_since(app: &App) -> Option<String> {
    state(app)["notified_at"].as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn app(digest_mins: i64) -> (tempfile::TempDir, std::sync::Arc<App>) {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::for_tests(dir.path());
        cfg.alerts.digest_mins = digest_mins;
        let app = App::for_tests(cfg);
        (dir, app)
    }

    fn age(app: &App, mins: f64) {
        let mut rows = dispatch::alerts(app);
        for a in rows.iter_mut() {
            a["at"] = json!(iso(now_ts() - mins * 60.0));
        }
        app.db.set_setting("alerts", Some(&jdumps(&Value::Array(rows)))).unwrap();
    }

    #[test]
    fn new_alerts_gather_before_one_notification() {
        let (_d, app) = app(5);
        dispatch::add_alert(&app, "T1: which port?", Some(1), None, None, None).unwrap();
        dispatch::add_alert(&app, "T2: needs attention", Some(2), None, None, None).unwrap();
        tick(&app).unwrap();
        assert!(showing_since(&app).is_none());
        age(&app, 6.0);
        tick(&app).unwrap();
        let s = state(&app);
        assert_eq!(s["sent"].as_array().unwrap().len(), 2);
        let p = params(&app, &members(&app));
        assert_eq!(p["title"], "2 things need you");
        assert_eq!(p["body"], "T1: which port?\nT2: needs attention");
        assert_eq!(p["open"], "taskboard://");
    }

    #[test]
    fn urgent_and_snoozed_alerts_stay_out() {
        let (_d, app) = app(5);
        dispatch::raise(&app, "main is red", None, None, Some("master:1"), true).unwrap();
        dispatch::add_alert(&app, "T3: review my PR", Some(3), None, None, Some("review")).unwrap();
        dispatch::add_alert(&app, "T4: which color?", Some(4), None, None, None).unwrap();
        let t4 = dispatch::alerts(&app).into_iter().find(|a| a["task"] == "T4").unwrap();
        dispatch::snooze_alert(&app, t4["id"].as_str().unwrap(), 30).unwrap();
        let rows = members(&app);
        assert_eq!(rows.len(), 1);
        assert_eq!(title(&rows), "T3 needs you");
    }

    #[test]
    fn it_goes_away_once_nothing_waits() {
        let (_d, app) = app(5);
        dispatch::add_alert(&app, "T1: which port?", Some(1), None, None, None).unwrap();
        age(&app, 6.0);
        tick(&app).unwrap();
        assert!(showing_since(&app).is_some());
        dispatch::clear_alerts(&app, Some(1), None).unwrap();
        tick(&app).unwrap();
        assert!(showing_since(&app).is_none());
    }

    #[test]
    fn a_snooze_holds_the_repeat() {
        let (_d, app) = app(5);
        dispatch::add_alert(&app, "T1: which port?", Some(1), None, None, None).unwrap();
        age(&app, 6.0);
        tick(&app).unwrap();
        let at = showing_since(&app).unwrap();
        let pick = json!({"response": {"kind": "action", "action": "Snooze 30 min"}, "sent_at": at});
        assert!(handle_response(&app, &pick).unwrap());
        assert!(!handle_response(&app, &pick).unwrap());
        assert!(!due(&app, &state(&app), &members(&app), now_ts()));
        assert!(due(&app, &state(&app), &members(&app), now_ts() + 31.0 * 60.0));
    }

    #[test]
    fn many_alerts_are_counted_past_five() {
        let rows: Vec<Value> = (1..=7).map(|n| json!({"text": format!("T{n}: q"), "task": format!("T{n}")})).collect();
        assert_eq!(body(&rows).lines().last(), Some("and 2 more"));
        assert_eq!(title(&rows), "7 things need you");
    }

    #[test]
    fn off_leaves_alerts_on_their_own() {
        let (_d, app) = app(0);
        dispatch::add_alert(&app, "T1: which port?", Some(1), None, None, None).unwrap();
        assert!(members(&app).is_empty());
    }
}
