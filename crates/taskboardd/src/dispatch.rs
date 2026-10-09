//! Alerts and notifications, and when a new terminal may open.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, hours, midna, p, prflow};

fn real(p: &str) -> String {
    if p.is_empty() {
        return String::new();
    }
    let e = expand_home(p);
    std::fs::canonicalize(&e).unwrap_or(e).to_string_lossy().to_string()
}

fn under(path: &str, root: &str) -> bool {
    !path.is_empty() && !root.is_empty() && (path == root || path.starts_with(&format!("{root}/")))
}

/// Whether an agent job opens a new terminal (rather than typing into an open one).
pub fn new_terminal(app: &App, j: &Row) -> bool {
    if j.s("kind") != Some("agent") {
        return false;
    }
    let a = board::job_args(j);
    has(a.s("cwd")) && midna::send_target(app, &a).is_none()
}

fn project_roots(app: &App) -> Vec<String> {
    let mut v: Vec<String> = jloads_arr(app.db.get_setting("midna_projects").ok().flatten().as_deref())
        .iter()
        .filter_map(|p| p.get("path").and_then(|x| x.as_str()))
        .map(real)
        .collect();
    v.sort();
    v.dedup();
    v
}

fn root_for(app: &App, cwd: &str) -> String {
    let cwd = real(cwd);
    project_roots(app).into_iter().filter(|r| under(&cwd, r)).max_by_key(|r| r.len()).unwrap_or(cwd)
}

fn project_busy(app: &App, root: &str) -> Result<bool> {
    let rows = app.db.q("SELECT project_path FROM sessions WHERE status IN ('working','needs')", p![])?;
    Ok(rows.iter().any(|r| under(&real(&r.st("project_path")), root)))
}

/// A queued agent job (pickup "queue") waits while another terminal in its project is busy.
pub fn launch_ready(app: &App, j: &Row) -> Result<bool> {
    if !new_terminal(app, j) {
        return Ok(true);
    }
    let a = board::job_args(j);
    if as_bool(a.get("queue"), false) {
        return Ok(!project_busy(app, &root_for(app, &a.st("cwd")))?);
    }
    Ok(true)
}

pub fn alerts(app: &App) -> Vec<Value> {
    jloads_arr(app.db.get_setting("alerts").ok().flatten().as_deref())
}

/// The alerts in the order the app shows them: urgent ones first, then as they came.
pub fn listing(app: &App) -> Vec<Value> {
    let mut rows = alerts(app);
    rows.sort_by_key(|a| a["urgent"] != true);
    rows
}

/// Saves the alerts and withdraws the notifications of any that are gone.
fn keep(app: &App, rows: &[Value]) -> Result<()> {
    let gone: Vec<String> = alerts(app)
        .iter()
        .filter_map(|a| a["id"].as_str())
        .filter(|id| !rows.iter().any(|r| r["id"].as_str() == Some(id)))
        .map(str::to_string)
        .collect();
    app.db.set_setting("alerts", Some(&jdumps(&Value::Array(rows.to_vec()))))?;
    for id in gone {
        withdraw(app, &id);
    }
    Ok(())
}

fn new_alert_id() -> String {
    use rand::Rng;
    let n: u64 = rand::thread_rng().gen();
    format!("{n:016x}")[..10].to_string()
}

/// An alert that can't be dismissed (it still snoozes), isn't replaced by other alerts for its task and isn't
/// pushed out by the 20-alert cap: a PR waiting on the owner's review (it clears once the PR is reviewed,
/// `resolved`) or an urgent alert (it clears when what raised it clears it, or its task moves on).
pub fn stays(a: &Value) -> bool {
    a["review"] == true || a["urgent"] == true
}

pub fn add_alert(app: &App, text: &str, task_id: Option<i64>, goal_id: Option<i64>, session_id: Option<&str>, extra: Option<&str>) -> Result<()> {
    push_alert(app, text, task_id, goal_id, session_id, extra).map(|_| ())
}

/// The urgent alert up for a task, if any.
fn urgent_for(app: &App, task_id: Option<i64>) -> Option<Value> {
    task_id.and_then(|t| alerts(app).into_iter().find(|a| a["urgent"] == true && a["task_id"].as_i64() == Some(t)))
}

/// `add_alert`, giving back the alert it kept. A new alert for a task replaces the task's other alerts except
/// the ones that stay; an urgent one replaces nothing. While a task has an urgent alert up, a new plain alert
/// for it isn't raised (None); its PR-review alert still is.
fn push_alert(app: &App, text: &str, task_id: Option<i64>, goal_id: Option<i64>, session_id: Option<&str>, extra: Option<&str>) -> Result<Option<Value>> {
    let (review, urgent) = (extra == Some("review"), extra == Some("urgent"));
    if !review && !urgent && urgent_for(app, task_id).is_some() {
        return Ok(None);
    }
    let mut rows: Vec<Value> = alerts(app)
        .into_iter()
        .filter(|a| urgent || !(task_id.is_some() && a["task_id"].as_i64() == task_id) || (stays(a) && !review) || a["urgent"] == true)
        .collect();
    let now = now_iso();
    let mut new = json!({"id": new_alert_id(), "at": now, "text": text, "task_id": task_id, "session_id": session_id,
                         "task": rf_opt("task", task_id), "goal": rf_opt("goal", goal_id), "notified_at": now});
    if let Some(flag) = extra {
        new[flag] = json!(true);
    }
    let new_id = new["id"].clone();
    rows.push(new);
    // The newest 20 alerts, plus every one that stays, however many others came after it.
    let (staying, mut rest): (Vec<Value>, Vec<Value>) = rows.into_iter().partition(stays);
    let n = rest.len();
    if n > 20 {
        rest.drain(..n - 20);
    }
    let rows: Vec<Value> = staying.into_iter().chain(rest).collect();
    keep(app, &rows)?;
    let kept = rows.into_iter().find(|a| a["id"] == new_id);
    if let Some(a) = &kept {
        notify_alert(app, a);
    }
    Ok(kept)
}

/// An alert raised through `POST /alerts` (`tb alert raise`): with a `key`, raising it again while it's up
/// changes nothing, and `clear_alert_key` takes it away.
pub fn raise(app: &App, text: &str, task_id: Option<i64>, goal_id: Option<i64>, key: Option<&str>, urgent: bool) -> Result<Value> {
    if let Some(k) = key {
        if let Some(a) = alerts(app).into_iter().find(|a| a["key"] == k) {
            return Ok(a);
        }
    }
    let Some(new) = push_alert(app, text, task_id, goal_id, None, urgent.then_some("urgent"))? else {
        let held = urgent_for(app, task_id).map(|a| a["id"].as_str().unwrap_or("").to_string()).unwrap_or_default();
        return err(409, format!("{} has an urgent alert up ({held}); its other alerts wait until that clears.", task_id.map(|t| rf("task", t)).unwrap_or_default()));
    };
    let mut rows = alerts(app);
    let Some(a) = rows.iter_mut().find(|a| a["id"] == new["id"]) else {
        return err(500, "The alert wasn't kept.");
    };
    if let Some(k) = key {
        a["key"] = json!(k);
    }
    let out = a.clone();
    keep(app, &rows)?;
    Ok(out)
}

/// An alert that goes with a thing rather than a task (a QA comment, `qa:3`): one per key, cleared by
/// its key, and resolved by its owner (`qa::alert_resolved`).
pub fn add_alert_keyed(app: &App, text: &str, task_id: Option<i64>, goal_id: Option<i64>, key: &str) -> Result<()> {
    if alerts(app).iter().any(|a| a["key"] == key) {
        return Ok(());
    }
    let Some(new) = push_alert(app, text, task_id, goal_id, None, None)? else { return Ok(()) };
    let mut rows = alerts(app);
    if let Some(a) = rows.iter_mut().find(|a| a["id"] == new["id"]) {
        a["key"] = json!(key);
    }
    keep(app, &rows)
}

pub fn clear_alert_key(app: &App, key: &str) -> Result<()> {
    let rows: Vec<Value> = alerts(app).into_iter().filter(|a| a["key"] != key).collect();
    keep(app, &rows)
}

/// Clears every alert whose key starts with `prefix` (`pr-builds:`).
pub fn clear_alert_prefix(app: &App, prefix: &str) -> Result<()> {
    let rows: Vec<Value> = alerts(app).into_iter().filter(|a| !a["key"].as_str().is_some_and(|k| k.starts_with(prefix))).collect();
    keep(app, &rows)
}

/// Clears an alert by id, or a task's alerts: all but the ones that stay (a PR waiting on review, an urgent
/// alert), which clear on their own condition or by their id or key.
pub fn clear_alerts(app: &App, task_id: Option<i64>, alert_id: Option<&str>) -> Result<()> {
    let rows: Vec<Value> = alerts(app)
        .into_iter()
        .filter(|a| {
            !((task_id.is_some() && a["task_id"].as_i64() == task_id && !stays(a)) || (alert_id.is_some() && a["id"].as_str() == alert_id))
        })
        .collect();
    keep(app, &rows)
}

/// The snooze buttons on a notification, from config.toml's `[alerts] snooze_mins` (at most four).
pub fn snooze_actions(app: &App) -> Vec<String> {
    app.cfg.alerts.snooze_mins.iter().filter(|m| **m > 0).take(4).map(|m| snooze_label(*m)).collect()
}

/// "Snooze 15 min", "Snooze 1 hour", "Snooze 2 hours".
pub fn snooze_label(mins: i64) -> String {
    match mins {
        60 => "Snooze 1 hour".into(),
        m if m % 60 == 0 => format!("Snooze {} hours", m / 60),
        m => format!("Snooze {m} min"),
    }
}

/// The minutes a snooze button stands for, when it's one of the configured ones.
pub fn snooze_mins(app: &App, label: &str) -> Option<i64> {
    app.cfg.alerts.snooze_mins.iter().copied().filter(|m| *m > 0).take(4).find(|m| snooze_label(*m) == label)
}

/// What the owner did with an alert's notification (Midna's `notify.response`): a snooze button snoozes the
/// alert; a click opens the app on it (Midna does that) and a dismissal leaves it to repeat. Whether it snoozed.
/// `sent_at` (the alert's `notified_at` when that notification went out) makes each notification's pick count
/// once, across restarts: Midna keeps answering with it until the alert is notified again.
pub fn handle_response(app: &App, alert_id: &str, response: &Value) -> Result<bool> {
    let r = response.get("response").filter(|r| r.is_object()).unwrap_or(response);
    if r["kind"] != "action" {
        return Ok(false);
    }
    let Some(mins) = r["action"].as_str().and_then(|l| snooze_mins(app, l)) else { return Ok(false) };
    let Some(a) = alerts(app).into_iter().find(|a| a["id"] == alert_id) else { return Ok(false) };
    let sent_at = response.get("sent_at").and_then(|x| x.as_str());
    if sent_at.is_some() && a["answered"].as_str() == sent_at {
        return Ok(false);
    }
    snooze_alert(app, alert_id, mins)?;
    if let Some(at) = sent_at {
        let mut rows = alerts(app);
        for a in rows.iter_mut().filter(|a| a["id"] == alert_id) {
            a["answered"] = json!(at);
        }
        keep(app, &rows)?;
    }
    Ok(true)
}

/// Handles the notification responses Midna has reported since the last tick.
fn take_responses(app: &App) -> Result<()> {
    let inbox = app.shared.lock().notify_inbox.clone();
    let got: Vec<(String, Value)> = std::mem::take(&mut *inbox.lock());
    for (id, r) in got {
        handle_response(app, &id, &r)?;
    }
    Ok(())
}

pub fn snooze_alert(app: &App, alert_id: &str, mins: i64) -> Result<Option<Value>> {
    let mut rows = alerts(app);
    for a in rows.iter_mut() {
        if a["id"] == alert_id {
            a["snoozed_until"] = json!(iso(now_ts() + mins as f64 * 60.0));
        }
    }
    keep(app, &rows)?;
    Ok(rows.into_iter().find(|a| a["id"] == alert_id))
}

fn repeat_alerts(app: &App) -> Result<()> {
    let every = hours::get(app).alert_every_mins as f64 * 60.0;
    let open = hours::is_open(app);
    if every <= 0.0 {
        return Ok(());
    }
    let mut rows = alerts(app);
    let now = now_ts();
    let mut sent = false;
    for a in rows.iter_mut() {
        // Outside the work hours only urgent alerts keep repeating.
        if !open && a["urgent"] != true {
            continue;
        }
        // A snooze runs to a clock time; a repeat waits for `every` of the time the Mac was awake.
        let due = match a["snoozed_until"].as_str().and_then(parse_iso) {
            Some(s) => now >= s,
            None => age_secs(a["notified_at"].as_str().or(a["at"].as_str())).unwrap_or(0.0) >= every,
        };
        if due {
            a["notified_at"] = json!(now_iso());
            a["snoozed_until"] = Value::Null;
            sent = true;
            notify_alert(app, a);
        }
    }
    if sent {
        keep(app, &rows)?;
    }
    Ok(())
}

fn resolved(app: &App, a: &Value) -> Result<bool> {
    if a["key"].as_str().map(|k| k.starts_with("qa:")).unwrap_or(false) {
        return crate::qa::alert_resolved(app, a);
    }
    if a["key"].as_str().map(|k| k.starts_with("retarget:")).unwrap_or(false) {
        return crate::stack::retarget_resolved(app, a);
    }
    let at = a["at"].as_str().unwrap_or("");
    let sid = if let Some(tid) = a["task_id"].as_i64() {
        let Some(t) = board::find_task(app, Some(tid))? else { return Ok(true) };
        if a["review"] == true {
            return Ok(t.s("status") != Some("done") || !prflow::awaiting_owner(&t));
        }
        if t.s("status") == Some("done") && !(a["pr"] == true && prflow::wake_stuck(&t)) {
            return Ok(true);
        }
        let moved = app.db.count(
            "SELECT COUNT(*) FROM events WHERE task_id = ? AND at > ? AND who NOT IN (?, ?, ?)",
            p![t.id(), at, board::BOARD, board::MIDNA, board::OWNER],
        )?;
        if moved > 0 {
            return Ok(true);
        }
        a["session_id"].as_str().map(|s| s.to_string()).or_else(|| t.s("session_id").map(|s| s.to_string()))
    } else {
        a["session_id"].as_str().map(|s| s.to_string())
    };
    let s = board::get_session(app, sid.as_deref())?;
    Ok(s.map(|s| s.s("status") == Some("working") && s.st("status_at").as_str() > at).unwrap_or(false))
}

pub fn prune_alerts(app: &App) -> Result<()> {
    let rows = alerts(app);
    let mut keep_rows = vec![];
    for a in &rows {
        if !resolved(app, a)? {
            keep_rows.push(a.clone());
        }
    }
    if keep_rows.len() != rows.len() {
        keep(app, &keep_rows)?;
    }
    Ok(())
}

/// Where a notification click goes: the app's URL scheme (`taskboard://task/T12`,
/// `taskboard://goal/G3`, `taskboard://session/<id>`), which Taskboard.app opens on that page.
pub fn alert_url(app: &App, task_id: Option<i64>, goal_id: Option<i64>, session_id: Option<&str>) -> String {
    let page = if let Some(s) = session_id {
        format!("session/{s}")
    } else if let Some(t) = task_id {
        format!("task/{}", rf("task", t))
    } else if let Some(g) = goal_id {
        format!("goal/{}", rf("goal", g))
    } else {
        String::new()
    };
    let base = &app.cfg.page_url;
    // `taskboard://` keeps its two slashes; `http://host/tasks/` loses only the trailing one.
    let base = if base.ends_with("://") { base.as_str() } else { base.trim_end_matches('/') };
    if base.ends_with("://") { format!("{base}{page}") } else { format!("{base}/{page}") }
}

/// The id an alert's notification goes by in Midna: sending it again replaces the one showing, and
/// `notify.withdraw` takes it away.
pub fn notification_id(alert_id: &str) -> String {
    format!("taskboard-alert-{alert_id}")
}

/// `notify.send`'s params for an alert: its id, its text, the link to it and the snooze buttons.
pub fn alert_params(app: &App, a: &Value) -> Value {
    let goal = a["goal"].as_str().and_then(|g| parse_ref_str(g, "goal").ok().flatten());
    let mut p = json!({"title": if a["urgent"] == true { "Task board · urgent" } else { "Task board" },
                       "body": a["text"], "category": "board", "sound": true,
                       "open": alert_url(app, a["task_id"].as_i64(), goal, a["session_id"].as_str()),
                       "id": notification_id(a["id"].as_str().unwrap_or(""))});
    let actions = snooze_actions(app);
    if !actions.is_empty() {
        p["actions"] = json!(actions);
    }
    p
}

/// An alert's desktop notification. With snooze buttons, the alert's one waiter (`Waiter::listen`) hears the
/// owner's pick and leaves it for the next tick to act on.
fn notify_alert(app: &App, a: &Value) {
    if !app.cfg.notify || !app.cfg.runner {
        return;
    }
    let params = alert_params(app, a);
    let wait = params.get("actions").is_some();
    let (midna, inbox, waiters) = {
        let sh = app.shared.lock();
        (app.cfg.midna.clone(), sh.notify_inbox.clone(), sh.alert_waiters.clone())
    };
    let alert_id = a["id"].as_str().unwrap_or("").to_string();
    let sent_at = a["notified_at"].as_str().or(a["at"].as_str()).unwrap_or("").to_string();
    std::thread::spawn(move || {
        if midna::call_exe(&midna, "notify.send", &params, 30.0).is_err() || !wait {
            return;
        }
        if let Some(w) = Waiter::start(waiters, &alert_id, &sent_at) {
            w.listen(&midna, &inbox);
        }
    });
}

/// The one thread per alert that waits on its notification's response for as long as the alert is up
/// (`Shared::alert_waiters` maps the alert's id to the `notified_at` of its latest notification).
pub struct Waiter {
    waiters: Arc<Mutex<HashMap<String, String>>>,
    alert_id: String,
}

impl Waiter {
    /// Records that the alert was notified at `sent_at`; a Waiter only when the alert had none yet.
    pub fn start(waiters: Arc<Mutex<HashMap<String, String>>>, alert_id: &str, sent_at: &str) -> Option<Waiter> {
        let fresh = waiters.lock().insert(alert_id.to_string(), sent_at.to_string()).is_none();
        fresh.then(|| Waiter { waiters, alert_id: alert_id.to_string() })
    }

    /// Waits on `notify.response` 600 s at a time, reports each notification's pick once (tagged with its
    /// `sent_at`, for `handle_response`), and stops once the alert is gone.
    fn listen(self, midna: &Path, inbox: &Mutex<Vec<(String, Value)>>) {
        let nid = notification_id(&self.alert_id);
        let mut answered: Option<String> = None;
        loop {
            let Some(sent_at) = self.waiters.lock().get(&self.alert_id).cloned() else { return };
            // Midna keeps giving this notification's pick until it's sent again; wait for that.
            if answered.as_deref() == Some(sent_at.as_str()) {
                std::thread::sleep(std::time::Duration::from_secs(5));
                continue;
            }
            match midna::call_exe(midna, "notify.response", &json!({"id": nid, "wait_secs": 600}), 620.0) {
                Ok(mut r) if r.get("response").is_some_and(|x| x.is_object()) => {
                    r["sent_at"] = json!(sent_at);
                    if self.waiters.lock().contains_key(&self.alert_id) {
                        inbox.lock().push((self.alert_id.clone(), r));
                    }
                    answered = Some(sent_at);
                }
                Ok(_) => {}
                Err(_) => std::thread::sleep(std::time::Duration::from_secs(30)),
            }
        }
    }
}

/// Keeps one waiter per alert that's up: forgets the ones whose alert is gone, and starts one for an alert
/// notified before the board restarted.
fn tend_waiters(app: &App) {
    let waiters = app.shared.lock().alert_waiters.clone();
    let rows = alerts(app);
    waiters.lock().retain(|id, _| rows.iter().any(|a| a["id"].as_str() == Some(id.as_str())));
    if !app.cfg.notify || !app.cfg.runner || snooze_actions(app).is_empty() {
        return;
    }
    let (midna, inbox) = (app.cfg.midna.clone(), app.shared.lock().notify_inbox.clone());
    for a in &rows {
        let (Some(id), Some(at)) = (a["id"].as_str(), a["notified_at"].as_str().or(a["at"].as_str())) else { continue };
        if let Some(w) = Waiter::start(waiters.clone(), id, at) {
            let (midna, inbox) = (midna.clone(), inbox.clone());
            std::thread::spawn(move || w.listen(&midna, &inbox));
        }
    }
}

/// Takes a cleared alert's notification away (Notification Center, Midna's card and badge).
fn withdraw(app: &App, alert_id: &str) {
    if !app.cfg.notify || !app.cfg.runner {
        return;
    }
    let midna = app.cfg.midna.clone();
    let params = json!({"id": notification_id(alert_id)});
    std::thread::spawn(move || {
        let _ = midna::call_exe(&midna, "notify.withdraw", &params, 30.0);
    });
}

/// A desktop notification through Midna, sent on its own thread.
pub fn notify(app: &App, text: &str, url: &str, title: Option<&str>) {
    if !app.cfg.notify || !app.cfg.runner {
        return;
    }
    let params = json!({"title": title.unwrap_or("Task board"), "body": text, "category": "board", "sound": true, "open": url});
    let midna = app.cfg.midna.clone();
    std::thread::spawn(move || {
        let _ = midna::call_exe(&midna, "notify.send", &params, 30.0);
    });
}

pub fn tick(app: &App) -> Result<()> {
    take_responses(app)?;
    prune_alerts(app)?;
    repeat_alerts(app)?;
    tend_waiters(app);
    Ok(())
}

pub fn exists(p: &str) -> bool {
    !p.is_empty() && Path::new(p).is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_links_open_the_app() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::for_tests(crate::config::Config::for_tests(dir.path()));
        assert_eq!(alert_url(&app, Some(12), Some(3), None), "taskboard://task/T12");
        assert_eq!(alert_url(&app, None, Some(3), None), "taskboard://goal/G3");
        assert_eq!(alert_url(&app, Some(12), None, Some("abc")), "taskboard://session/abc");
        assert_eq!(alert_url(&app, None, None, None), "taskboard://");
    }

    #[test]
    fn page_url_overrides_the_scheme() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = crate::config::Config::for_tests(dir.path());
        cfg.page_url = "taskboard-dev://".into();
        let app = App::for_tests(cfg);
        assert_eq!(alert_url(&app, Some(1), None, None), "taskboard-dev://task/T1");
    }
}
