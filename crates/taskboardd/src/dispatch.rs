//! Alerts and notifications, and when a new terminal may open.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, digest, hours, midna, p, prflow};

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
    let mut v: Vec<String> = jloads_arr(crate::reqcache::setting(app, "midna_projects").ok().flatten().as_deref())
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

/// Busy terminals under `root`.
fn busy_terminals(app: &App, root: &str) -> Result<i64> {
    let rows = app.db.q("SELECT project_path, status, background, background_at, live_background FROM sessions WHERE status IN ('working','needs','idle')", p![])?;
    Ok(rows
        .iter()
        .filter(|r| r.s("status") != Some("idle") || board::waiting_on_background(r))
        .filter(|r| under(&real(&r.st("project_path")), root))
        .count() as i64)
}

/// Task terminals running across the board: a task's own terminal while the task works or the terminal is
/// busy, and a start the board is opening right now (so starts taken back to back don't overshoot the cap).
pub fn running_task_terminals(app: &App) -> Result<i64> {
    let rows = app.db.q(
        "SELECT s.status, s.background, s.background_at, s.live_background, MAX(t.status = 'working') AS working
         FROM sessions s JOIN tasks t ON t.session_id = s.id
         WHERE s.status IN ('working','needs','idle') GROUP BY s.id",
        p![],
    )?;
    let open = rows.iter().filter(|r| r.i("working") == Some(1) || r.s("status") != Some("idle") || board::waiting_on_background(r)).count();
    let launching = app.db.q(
        "SELECT j.id FROM jobs j JOIN tasks t ON t.start_job = j.id WHERE j.state = 'running' AND j.purpose = 'start' AND t.status = 'queued'",
        p![],
    )?;
    Ok((open + launching.len()) as i64)
}

/// How many task terminals may run at once across the board: `tb terminals --max`, else `[terminals] global_max`.
pub fn board_max(app: &App) -> i64 {
    board_max_set(app).unwrap_or(app.cfg.terminals.global_max).max(1)
}

/// What `tb terminals --max` put on the board, if anything.
pub fn board_max_set(app: &App) -> Option<i64> {
    app.db.get_setting(BOARD_MAX_SETTING).ok().flatten().and_then(|s| s.trim().parse().ok())
}

const BOARD_MAX_SETTING: &str = "board_max_terminals";

/// Sets the board's task terminal cap from a body's `max_terminals` (null or "default": back to config.toml's).
pub fn set_board_max(app: &App, body: &Value) -> Result<()> {
    let Some(v) = body.get("max_terminals") else { return err(400, "Say what to change: max_terminals.") };
    let n = match v {
        Value::Null => None,
        Value::String(s) if s.trim().eq_ignore_ascii_case("default") => None,
        _ => match v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())) {
            Some(n) if (1..=50).contains(&n) => Some(n),
            _ => return err(400, "max_terminals is a count from 1 to 50, or default."),
        },
    };
    app.db.set_setting(BOARD_MAX_SETTING, n.map(|n| n.to_string()).as_deref())
}

/// The board's task terminal cap for `GET /terminals`: the cap, what set it, and how many run now.
pub fn terminals_state(app: &App) -> Result<Value> {
    let (max, running) = (board_max(app), running_task_terminals(app)?);
    let from = if board_max_set(app).is_some() { "tb terminals --max" } else { "config.toml" };
    Ok(json!({"max_terminals": max, "max_terminals_set": board_max_set(app), "running": running,
              "line": format!("{running} of {max} task terminals running (cap from {from})")}))
}

/// Why a task's start job can't open its terminal yet: the board already runs as many task terminals as it
/// may (`tb terminals --max`, or `[terminals] global_max`), or, for a queued start (pickup "queue"), its
/// project already has as many busy terminals as it may (`[terminals] project_max`, or the project's own
/// `max_terminals`). A goal's own limit holds its tasks back before they get a start job.
pub fn start_hold(app: &App, j: &Row) -> Result<Option<String>> {
    if !new_terminal(app, j) {
        return Ok(None);
    }
    let a = board::job_args(j);
    if j.s("purpose") == Some("start") && j.i("task_id").is_some() {
        let (cap, running) = (board_max(app), running_task_terminals(app)?);
        if running >= cap {
            return Ok(Some(format!("Waits for a free slot: {running} of {cap} task terminals running across the board")));
        }
    }
    if !as_bool(a.get("queue"), false) {
        return Ok(None);
    }
    let root = root_for(app, &a.st("cwd"));
    let name = match board::find_task(app, j.i("task_id"))?.and_then(|t| t.s("project").filter(|p| !p.is_empty()).map(str::to_string)) {
        Some(n) => Some(n),
        None => crate::projects::project_for_path(app, Some(&root))?,
    };
    let cap = crate::projects::max_terminals(app, name.as_deref());
    let busy = busy_terminals(app, &root)?;
    if busy >= cap {
        let name = name.unwrap_or_else(|| "its project".into());
        return Ok(Some(format!("Waits for a free terminal: {busy} of {cap} busy in {name}")));
    }
    Ok(None)
}

/// `start_hold` for `t`'s start not made yet, in a new terminal at `cwd`: the starts already waiting count
/// as running, since they open first. A task checks it before it claims a worktree slot, so a start
/// waiting on a cap doesn't sit on a slot.
pub fn hold_before_start(app: &App, t: &Row, cwd: &str, queue: bool) -> Result<Option<String>> {
    let waiting: Vec<Row> = app
        .db
        .q("SELECT j.* FROM jobs j JOIN tasks t ON t.start_job = j.id WHERE j.state = 'pending' AND j.purpose = 'start' AND t.status = 'queued'", p![])?
        .into_iter()
        .filter(|j| new_terminal(app, j))
        .collect();
    let (cap, running) = (board_max(app), running_task_terminals(app)?);
    if running >= cap {
        return Ok(Some(format!("Waits for a free slot: {running} of {cap} task terminals running across the board")));
    }
    if running + waiting.len() as i64 >= cap {
        return Ok(Some(format!("Waits for a free slot: {} ahead of it for the board's {}", plural(waiting.len() as i64, "start"), plural(cap, "task terminal"))));
    }
    if !queue {
        return Ok(None);
    }
    let root = root_for(app, cwd);
    let name = match t.s("project").filter(|p| !p.is_empty()) {
        Some(n) => Some(n.to_string()),
        None => crate::projects::project_for_path(app, Some(&root))?,
    };
    let cap = crate::projects::max_terminals(app, name.as_deref());
    let ahead = waiting.iter().filter(|j| as_bool(board::job_args(j).get("queue"), false) && root_for(app, &board::job_args(j).st("cwd")) == root).count() as i64;
    let busy = busy_terminals(app, &root)?;
    if busy + ahead >= cap {
        let name = name.unwrap_or_else(|| "its project".into());
        return Ok(Some(format!("Waits for a free terminal: {busy} of {cap} busy in {name}")));
    }
    Ok(None)
}

pub fn launch_ready(app: &App, j: &Row) -> Result<bool> {
    Ok(start_hold(app, j)?.is_none())
}

pub fn alerts(app: &App) -> Vec<Value> {
    jloads_arr(crate::reqcache::setting(app, "alerts").ok().flatten().as_deref())
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
    crate::reqcache::forget_setting("alerts");
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
    a["review"] == true || a["urgent"] == true || quiet(a)
}

/// An alert that only shows in Needs you: it never notifies or joins the digest, since what raised it
/// sent its own notification (a weekly review).
pub fn quiet(a: &Value) -> bool {
    a["quiet"] == true
}

pub fn add_alert(app: &App, text: &str, task_id: Option<i64>, goal_id: Option<i64>, session_id: Option<&str>, extra: Option<&str>) -> Result<()> {
    push_alert(app, text, task_id, goal_id, session_id, extra).map(|_| ())
}

/// `add_alert` for free text the board has no fixes for (an agent's question): the alert asks for
/// suggested fixes (`want_suggestions`).
pub fn add_free_alert(app: &App, text: &str, task_id: Option<i64>, goal_id: Option<i64>) -> Result<()> {
    match push_alert(app, text, task_id, goal_id, None, None)? {
        Some(a) => want_suggestions(app, a["id"].as_str().unwrap_or("")),
        None => Ok(()),
    }
}

/// Marks a free-text alert for `suggest.rs` to ask Haiku for its title and fixes, off the hot path.
/// An alert that already has fixes, or was asked about before, is left as it is.
pub fn want_suggestions(app: &App, id: &str) -> Result<()> {
    if !app.cfg.alerts.suggest {
        return Ok(());
    }
    let mut rows = alerts(app);
    let Some(a) = rows.iter_mut().find(|a| a["id"] == id && a["actions"].is_null() && a["suggest"].is_null()) else { return Ok(()) };
    a["suggest"] = json!("pending");
    keep(app, &rows)
}

/// Puts what `suggest.rs` made of alert `id` on it (`fields`: title, detail, actions, project), or marks
/// it `failed` when `fields` is None; the card then shows its text with Dismiss. An alert that has gone
/// meanwhile is left gone.
pub fn set_suggestions(app: &App, id: &str, fields: Option<Value>) -> Result<()> {
    let mut rows = alerts(app);
    let Some(a) = rows.iter_mut().find(|a| a["id"] == id && a["suggest"] == "pending") else { return Ok(()) };
    match fields {
        Some(f) => {
            for (k, v) in f.as_object().into_iter().flatten() {
                a[k] = v.clone();
            }
            a["suggest"] = json!("done");
        }
        None => a["suggest"] = json!("failed"),
    }
    keep(app, &rows)
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
    if waits_for_hours(app, &new) {
        new["held"] = json!(true);
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
    if let Some(a) = kept.as_ref().filter(|a| !digest::holds(app, a) && a["held"] != true) {
        notify_alert(app, a);
    }
    Ok(kept)
}

/// Whether a new alert's notification waits for the work hours to open (`deliver_held`): outside them
/// only an urgent alert notifies at once.
fn waits_for_hours(app: &App, a: &Value) -> bool {
    a["urgent"] != true && !quiet(a) && !hours::is_open(app)
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
    add_alert_keyed_with(app, text, task_id, goal_id, key, Value::Null)
}

/// `add_alert_keyed`, carrying `fields` for the app's card: a short `title`, one `detail` line and the
/// `actions` it offers (`AlertAction`). `text` still says it all, for the notification and `tb alerts`.
pub fn add_alert_keyed_with(app: &App, text: &str, task_id: Option<i64>, goal_id: Option<i64>, key: &str, fields: Value) -> Result<()> {
    if alerts(app).iter().any(|a| a["key"] == key) {
        return Ok(());
    }
    let Some(new) = push_alert(app, text, task_id, goal_id, None, None)? else { return Ok(()) };
    let mut rows = alerts(app);
    if let Some(a) = rows.iter_mut().find(|a| a["id"] == new["id"]) {
        a["key"] = json!(key);
        for (k, v) in fields.as_object().into_iter().flatten() {
            a[k] = v.clone();
        }
    }
    keep(app, &rows)
}

/// A fix an alert offers: a button labelled `label` that opens a Claude terminal with `prompt` typed in
/// for the owner to send, and `command`, what the fix runs (copied when no terminal can open).
pub struct AlertAction {
    pub label: String,
    pub prompt: String,
    pub command: String,
}

impl AlertAction {
    pub fn json(&self) -> Value {
        json!({"label": self.label, "prompt": self.prompt, "command": self.command})
    }
}

/// `POST /alerts/:id/act {n}`: the alert's n-th action handed to Claude, in a new terminal in its
/// project's folder (the alert's `project`) with the action's prompt typed in, not sent: the owner
/// sends it. Refused while Midna is down or the folder is unknown; the app then copies the command.
pub fn act(app: &App, id: &str, n: usize) -> Result<Value> {
    let Some(a) = alerts(app).into_iter().find(|a| a["id"] == id) else { return err(404, "That alert is gone.") };
    let Some(action) = a["actions"].as_array().and_then(|v| v.get(n)).cloned() else { return err(404, "That alert has no such fix.") };
    if !midna::up(app) {
        return err(409, "Midna isn't running, so the board can't open a terminal.");
    }
    let project = a["project"].as_str().unwrap_or("");
    // A suggested fix on an alert with no project opens in the owner's home folder.
    let home = (project.is_empty() && a["suggest"] == "done").then(|| dirs::home_dir().map(|h| h.to_string_lossy().to_string())).flatten();
    let Some(cwd) = home.or(crate::projects::project_path(app, Some(project))?) else {
        return err(409, format!("The board doesn't know the folder for the project “{project}”."));
    };
    let label = action["label"].as_str().unwrap_or("Fix");
    let what = [a["title"].as_str(), a["detail"].as_str()].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(": ");
    let what = if what.is_empty() { a["text"].as_str().unwrap_or("").to_string() } else { what };
    let context = format!(
        "{owner} opened this terminal from an alert on the task board ({what}) by pressing “{label}”. Their request is typed into the prompt \
         for them to send. Wait for it; don't start on anything by yourself. When it comes, make the change with tb and say what changed. \
         You aren't on a task: don't take one.",
        owner = app.cfg.owner
    );
    let args = json!({"cwd": cwd, "title": short(label, 40), "prompt": "", "draft": action["prompt"], "context": context, "queue": false});
    let j = app.db.tx(|| board::create_job(app, "agent", args, None, "alert", Some(json!({"alert": id}))))?;
    Ok(json!({"ok": true, "job": rf("job", j)}))
}

/// A quiet alert (`quiet`) keyed `key`, carrying `fields` (the review it opens): one per key.
pub fn add_quiet_alert(app: &App, text: &str, key: &str, fields: Value) -> Result<()> {
    if alerts(app).iter().any(|a| a["key"] == key) {
        return Ok(());
    }
    let Some(new) = push_alert(app, text, None, None, None, Some("quiet"))? else { return Ok(()) };
    let mut rows = alerts(app);
    if let Some(a) = rows.iter_mut().find(|a| a["id"] == new["id"]) {
        a["key"] = json!(key);
        for (k, v) in fields.as_object().into_iter().flatten() {
            a[k] = v.clone();
        }
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
    // A start ask's answer waits for `start_answers`, which runs outside the tick's transaction.
    let got: Vec<(String, Value)> = {
        let mut i = inbox.lock();
        let (asks, got) = std::mem::take(&mut *i).into_iter().partition(|(id, _)| id.starts_with("start:"));
        *i = asks;
        got
    };
    for (id, r) in got {
        if id == digest::KEY {
            digest::handle_response(app, &r)?;
        } else {
            handle_response(app, &id, &r)?;
        }
    }
    Ok(())
}

pub fn snooze_alert(app: &App, alert_id: &str, mins: i64) -> Result<Option<Value>> {
    snooze_to(app, alert_id, Some(now_ts() + mins as f64 * 60.0))
}

/// Snoozes alert `alert_id` to a clock time, or (`None`) until the owner is back from their next
/// stretch away: `snoozed_back` is when it was snoozed. Snoozed, it's out of Needs you and quiet.
pub fn snooze_to(app: &App, alert_id: &str, until: Option<f64>) -> Result<Option<Value>> {
    let mut rows = alerts(app);
    for a in rows.iter_mut().filter(|a| a["id"] == alert_id) {
        a["snoozed_until"] = until.map(|t| json!(iso(t))).unwrap_or(Value::Null);
        a["snoozed_back"] = if until.is_none() { json!(now_iso()) } else { Value::Null };
    }
    keep(app, &rows)?;
    Ok(rows.into_iter().find(|a| a["id"] == alert_id))
}

/// Whether alert `a` is snoozed now: its time hasn't come, or the owner hasn't been away and back.
pub fn snoozed(a: &Value) -> bool {
    match (a["snoozed_until"].as_str().and_then(parse_iso), a["snoozed_back"].as_str().and_then(parse_iso)) {
        (Some(until), _) => until > now_ts(),
        (None, Some(since)) => !crate::clock::back_since(since),
        _ => false,
    }
}

/// Notifies each alert held outside the work hours (`waits_for_hours`) once they open, once. One that
/// cleared meanwhile was pruned before this; a snoozed one notifies when its snooze ends instead.
fn deliver_held(app: &App) -> Result<()> {
    let mut rows = alerts(app);
    if !rows.iter().any(|a| a["held"] == true) || !hours::is_open(app) {
        return Ok(());
    }
    for a in rows.iter_mut().filter(|a| a["held"] == true) {
        a["held"] = Value::Null;
        if digest::holds(app, a) || snoozed(a) {
            continue;
        }
        a["notified_at"] = json!(now_iso());
        a["snoozed_until"] = Value::Null;
        a["snoozed_back"] = Value::Null;
        notify_alert(app, a);
    }
    keep(app, &rows)
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
        // Outside the work hours only urgent alerts keep repeating; the digest repeats the rest.
        if (!open && a["urgent"] != true) || digest::holds(app, a) {
            continue;
        }
        // A snooze runs to a clock time; a repeat waits for `every` of the time the Mac was awake.
        let due = match a["snoozed_until"].as_str().and_then(parse_iso) {
            Some(s) => now >= s,
            None if a["snoozed_back"].is_string() => !snoozed(a),
            None => age_secs(a["notified_at"].as_str().or(a["at"].as_str())).unwrap_or(0.0) >= every,
        };
        if due {
            a["notified_at"] = json!(now_iso());
            a["snoozed_until"] = Value::Null;
            a["snoozed_back"] = Value::Null;
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
    if a["key"].as_str().map(|k| k.starts_with("jira:")).unwrap_or(false) {
        return crate::jira_status::alert_resolved(app, a);
    }
    if a["key"].as_str().map(|k| k.starts_with("roster:")).unwrap_or(false) {
        return crate::asks::short_resolved(app, a);
    }
    if a["key"].as_str().map(|k| k.starts_with("master:")).unwrap_or(false) {
        return crate::breaks::alert_resolved(app, a);
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
        // The board's own lines (its PR stages, the owner's hooks) aren't the agent moving on.
        let moved = app.db.count(
            "SELECT COUNT(*) FROM events WHERE task_id = ? AND at > ? AND who NOT IN (?, ?, ?, 'PR', 'Hook')",
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
    page_link(app, &page)
}

/// The app's link to `page` (`task/T12`, `review/3`) under `page_url`.
pub fn page_link(app: &App, page: &str) -> String {
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

/// `notify.send`'s params for an alert: its id, its text, the link to it and the snooze buttons. The text
/// is the title, since Midna's collapsed card shows only that.
pub fn alert_params(app: &App, a: &Value) -> Value {
    let goal = a["goal"].as_str().and_then(|g| parse_ref_str(g, "goal").ok().flatten());
    let mut p = json!({"title": a["text"], "body": if a["urgent"] == true { "Task board · urgent" } else { "Task board" },
                       "category": "board", "sound": true,
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
    if !app.cfg.notify || !app.cfg.runner || quiet(a) {
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
        if midna::notify_send(&midna, &params).is_err() || !wait {
            return;
        }
        if let Some(w) = Waiter::start(waiters, &alert_id, &sent_at, &notification_id(&alert_id)) {
            w.listen(&midna, &inbox);
        }
    });
}

/// The one thread per alert (or start ask, `start:T12`) that waits on its notification's response for as long
/// as the alert is up (`Shared::alert_waiters` maps the alert's id to the `notified_at` of its latest notification).
pub struct Waiter {
    waiters: Arc<Mutex<HashMap<String, String>>>,
    alert_id: String,
    nid: String,
}

impl Waiter {
    /// Records that the alert was notified at `sent_at` as Midna's notification `nid`; a Waiter only when the
    /// alert had none yet.
    pub fn start(waiters: Arc<Mutex<HashMap<String, String>>>, alert_id: &str, sent_at: &str, nid: &str) -> Option<Waiter> {
        let fresh = waiters.lock().insert(alert_id.to_string(), sent_at.to_string()).is_none();
        fresh.then(|| Waiter { waiters, alert_id: alert_id.to_string(), nid: nid.to_string() })
    }

    /// Waits on `notify.response` 600 s at a time, reports each notification's pick once (tagged with its
    /// `sent_at`, for `handle_response`), and stops once the alert is gone.
    pub fn listen(self, midna: &Path, inbox: &Mutex<Vec<(String, Value)>>) {
        let nid = self.nid.clone();
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
    let asks = start_asks(app);
    let digest_at = digest::showing_since(app).filter(|_| digest::on(app));
    waiters.lock().retain(|id, _| match id.strip_prefix("start:") {
        Some(g) => asks.iter().any(|a| a["group"] == g),
        None if id == digest::KEY => digest_at.is_some(),
        None => rows.iter().any(|a| a["id"].as_str() == Some(id.as_str())),
    });
    if !app.cfg.notify || !app.cfg.runner {
        return;
    }
    let (midna, inbox) = (app.cfg.midna.clone(), app.shared.lock().notify_inbox.clone());
    let mut up: Vec<(String, String, String)> = vec![];
    if !snooze_actions(app).is_empty() {
        for a in rows.iter().filter(|a| !digest::holds(app, a) && !quiet(a) && a["held"] != true) {
            let (Some(id), Some(at)) = (a["id"].as_str(), a["notified_at"].as_str().or(a["at"].as_str())) else { continue };
            up.push((id.to_string(), at.to_string(), notification_id(id)));
        }
        if let Some(at) = &digest_at {
            up.push((digest::KEY.to_string(), at.clone(), digest::NOTIFICATION_ID.to_string()));
        }
    }
    for a in &asks {
        let (Some(g), Some(at)) = (a["group"].as_str(), a["notified_at"].as_str()) else { continue };
        up.push((start_key(g), at.to_string(), start_notification_id(g)));
    }
    for (id, at, nid) in up {
        if let Some(w) = Waiter::start(waiters.clone(), &id, &at, &nid) {
            let (midna, inbox) = (midna.clone(), inbox.clone());
            std::thread::spawn(move || w.listen(&midna, &inbox));
        }
    }
}

/// Takes a cleared alert's notification away (Notification Center, Midna's card and badge).
pub fn withdraw(app: &App, alert_id: &str) {
    withdraw_id(app, &notification_id(alert_id));
}

/// Takes Midna's notification `nid` away.
pub fn withdraw_id(app: &App, nid: &str) {
    if !app.cfg.notify || !app.cfg.runner {
        return;
    }
    let midna = app.cfg.midna.clone();
    let params = json!({"id": nid});
    std::thread::spawn(move || {
        let _ = midna::call_exe(&midna, "notify.withdraw", &params, 30.0);
    });
}

/// A desktop notification through Midna, sent on its own thread. The text is the title, as for alerts.
pub fn notify(app: &App, text: &str, url: &str) {
    if !app.cfg.notify || !app.cfg.runner {
        return;
    }
    let params = json!({"title": text, "body": "Task board","category": "board", "sound": true, "open": url});
    let midna = app.cfg.midna.clone();
    std::thread::spawn(move || {
        let _ = midna::notify_send(&midna, &params);
    });
}

/// The refused starts (`tb start T12`, `tb start G3`) waiting on the owner's tap on a notification: one per
/// ref, newest last. Asks a terminal made in one turn share a `group` (the first one's ref) and one notification.
pub fn start_asks(app: &App) -> Vec<Value> {
    jloads_arr(app.db.get_setting("start_asks").ok().flatten().as_deref())
}

/// The id a group of start asks' notification goes by in Midna: `taskboard-start-T12`.
pub fn start_notification_id(group: &str) -> String {
    format!("taskboard-start-{group}")
}

/// What a group of start asks goes by among the waiters and the inbox, apart from alerts' ids.
fn start_key(group: &str) -> String {
    format!("start:{group}")
}

/// Saves the start asks and withdraws the notifications of groups that are gone.
fn keep_asks(app: &App, rows: &[Value]) -> Result<()> {
    let mut gone: Vec<String> = start_asks(app)
        .iter()
        .filter_map(|a| a["group"].as_str())
        .filter(|g| !rows.iter().any(|r| r["group"] == *g))
        .map(str::to_string)
        .collect();
    gone.dedup();
    app.db.set_setting("start_asks", Some(&jdumps(&Value::Array(rows.to_vec()))))?;
    for g in gone {
        withdraw_id(app, &start_notification_id(&g));
    }
    Ok(())
}

/// The newest prompt a human typed in terminal `sid` (its id, which marks the turn, and its text).
fn latest_prompt(app: &App, sid: &str) -> Result<(i64, String)> {
    let r = app.db.q1(
        "SELECT id, text FROM session_events WHERE session_id = ? AND kind = 'prompt' AND COALESCE(data, '') != ? ORDER BY id DESC LIMIT 1",
        p![sid, crate::startword::BOARD_PROMPT],
    )?;
    Ok(r.map(|r| (r.i("id").unwrap_or(0), r.st("text"))).unwrap_or_default())
}

/// Records that terminal `via` was refused starting `kind` `id` (`run`: how it asked to start, `{"mode",
/// "session_id"}` for a task or `{"now"}` for a goal) and asks the owner with a notification. A new ask for the
/// same ref replaces the old one; asks from one turn of one terminal go out as one notification. Whether the
/// owner is asked.
pub fn ask_start(app: &App, kind: &str, id: i64, via: &str, run: Value) -> Result<bool> {
    let r = rf(kind, id);
    let (turn, prompt) = latest_prompt(app, via)?;
    let mut rows: Vec<Value> = start_asks(app).into_iter().filter(|a| a["ref"] != r.as_str()).collect();
    // The asks its old group still holds go by the next of them now.
    let mut moved = None;
    if let Some(head) = rows.iter().find(|a| a["group"] == r.as_str()).and_then(|a| a["ref"].as_str()).map(str::to_string) {
        for a in rows.iter_mut().filter(|a| a["group"] == r.as_str()) {
            a["group"] = json!(head);
        }
        moved = Some(head);
    }
    let group = rows
        .iter()
        .find(|a| a["session_id"] == via && a["turn"] == turn)
        .and_then(|a| a["group"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| r.clone());
    let now = now_iso();
    rows.push(json!({"ref": r, "kind": kind, "id": id, "session_id": via, "prompt": clip(&prompt, 300), "turn": turn,
                     "group": group, "run": run, "at": now}));
    for a in rows.iter_mut().filter(|a| a["group"] == group.as_str()) {
        a["notified_at"] = json!(now);
    }
    keep_asks(app, &rows)?;
    for g in moved.iter().filter(|g| **g != group).chain([&group]) {
        notify_start(app, g, &rows);
    }
    Ok(app.cfg.notify)
}

/// Drops the start ask for `r` (`T12`, `G3`): it started, on the board or on a terminal's word.
pub fn drop_start_ask(app: &App, r: &str) -> Result<()> {
    let rows = start_asks(app);
    if !rows.iter().any(|a| a["ref"] == r) {
        return Ok(());
    }
    keep_asks(app, &rows.into_iter().filter(|a| a["ref"] != r).collect::<Vec<_>>())
}

/// "T12", "T12 and T13", "T12, T13 and G2".
fn say_refs(refs: &[String]) -> String {
    match refs {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `notify.send`'s params for a group of start asks: what would start, who asked and how, and Start / Not now.
pub fn start_params(app: &App, group: &str, rows: &[Value]) -> Value {
    let mine: Vec<&Value> = rows.iter().filter(|a| a["group"] == group).collect();
    let refs: Vec<String> = mine.iter().filter_map(|a| a["ref"].as_str().map(str::to_string)).collect();
    let mut lines: Vec<String> = mine
        .iter()
        .map(|a| {
            let id = a["id"].as_i64();
            let title = if a["kind"] == "goal" {
                board::find_goal(app, id).ok().flatten().map(|g| g.st("title"))
            } else {
                board::find_task(app, id).ok().flatten().map(|t| t.st("title"))
            };
            format!("{} {}", a["ref"].as_str().unwrap_or(""), title.unwrap_or_default()).trim_end().to_string()
        })
        .collect();
    let first = mine.first().copied().cloned().unwrap_or_default();
    let who = board::session_name(app, first["session_id"].as_str(), None);
    let prompt = first["prompt"].as_str().unwrap_or("");
    lines.push(if prompt.is_empty() { format!("Asked in {who}") } else { format!("{who}: “{}”", one_line(prompt, 160)) });
    if let Some(at) = hours::until_open(app) {
        let now = mine.iter().all(|a| a["kind"] == "goal" && a["run"]["now"] == true);
        lines.push(if now {
            format!("Outside work hours: Start runs it now, until they open ({}).", hours::say_when(&at))
        } else {
            format!("Outside work hours: Start queues it for {}.", hours::say_when(&at))
        });
    }
    let (task, goal) = if first["kind"] == "goal" { (None, first["id"].as_i64()) } else { (first["id"].as_i64(), None) };
    json!({"title": format!("Start {}?", say_refs(&refs)), "body": lines.join("\n"), "category": "board", "sound": true,
           "open": alert_url(app, task, goal, None), "id": start_notification_id(group), "actions": ["Start", "Not now"]})
}

/// A group of start asks' notification, and the group's one waiter for the owner's pick.
fn notify_start(app: &App, group: &str, rows: &[Value]) {
    if !app.cfg.notify || !app.cfg.runner {
        return;
    }
    let params = start_params(app, group, rows);
    let Some(sent_at) = rows.iter().find(|a| a["group"] == group).and_then(|a| a["notified_at"].as_str()).map(str::to_string) else { return };
    let (midna, inbox, waiters) = {
        let sh = app.shared.lock();
        (app.cfg.midna.clone(), sh.notify_inbox.clone(), sh.alert_waiters.clone())
    };
    let (key, nid) = (start_key(group), start_notification_id(group));
    std::thread::spawn(move || {
        if midna::notify_send(&midna, &params).is_err() {
            return;
        }
        if let Some(w) = Waiter::start(waiters, &key, &sent_at, &nid) {
            w.listen(&midna, &inbox);
        }
    });
}

/// Acts on the owner's answers to start asks' notifications, outside a transaction (a start runs hooks and
/// makes worktrees).
pub fn start_answers(app: &App) -> Result<()> {
    let inbox = app.shared.lock().notify_inbox.clone();
    let got: Vec<(String, Value)> = {
        let mut i = inbox.lock();
        let (got, rest) = std::mem::take(&mut *i).into_iter().partition(|(id, _)| id.starts_with("start:"));
        *i = rest;
        got
    };
    for (key, r) in got {
        answer_start(app, &key["start:".len()..], &r)?;
    }
    Ok(())
}

/// What the owner did with a group of start asks' notification: Start starts each as the board's Start button
/// does, Not now or a dismissal drops them, a click (it opens the page) leaves them. Only a group the board keeps,
/// and only its latest notification (`sent_at`), counts.
pub fn answer_start(app: &App, group: &str, response: &Value) -> Result<()> {
    let r = response.get("response").filter(|r| r.is_object()).unwrap_or(response);
    let rows = start_asks(app);
    let mine: Vec<Value> = rows.iter().filter(|a| a["group"] == group).cloned().collect();
    let Some(first) = mine.first() else { return Ok(()) };
    if let Some(at) = response.get("sent_at").and_then(|x| x.as_str()) {
        if first["notified_at"].as_str() != Some(at) {
            return Ok(());
        }
    }
    let start = match (r["kind"].as_str(), r["action"].as_str()) {
        (Some("action"), Some("Start")) => true,
        (Some("action"), Some("Not now")) | (Some("dismissed"), _) => false,
        _ => return Ok(()),
    };
    // Off the list first, so a start that fails isn't started again by the same tap.
    keep_asks(app, &rows.into_iter().filter(|a| a["group"] != group).collect::<Vec<_>>())?;
    if !start {
        return Ok(());
    }
    for a in &mine {
        let Some(id) = a["id"].as_i64() else { continue };
        let out = if a["kind"] == "goal" {
            crate::api::run_goal_from_notification(app, id, a["run"]["now"].as_bool().unwrap_or(false))
        } else {
            crate::api::start_from_notification(app, id, a["run"]["mode"].as_str().unwrap_or("new"), a["run"]["session_id"].as_str())
        };
        if let Err(e) = out {
            let r = a["ref"].as_str().unwrap_or("");
            let (task, goal) = if a["kind"] == "goal" { (None, Some(id)) } else { (Some(id), None) };
            notify(app, &format!("{r} didn't start: {}", e.message), &alert_url(app, task, goal, None));
        }
    }
    Ok(())
}

/// Whether a start ask still stands: its task or goal is there and hasn't started some other way.
fn ask_stands(app: &App, a: &Value) -> Result<bool> {
    let id = a["id"].as_i64();
    if a["kind"] == "goal" {
        return Ok(board::find_goal(app, id)?.is_some());
    }
    let Some(t) = board::find_task(app, id)? else { return Ok(false) };
    if matches!(t.s("status"), Some("done") | Some("working")) || (t.s("status") == Some("needs") && t.s("needs_reason") != Some("start_failed")) {
        return Ok(false);
    }
    Ok(board::live_start_job(app, &t)?.is_none())
}

fn prune_asks(app: &App) -> Result<()> {
    let rows = start_asks(app);
    let mut kept = vec![];
    for a in &rows {
        if ask_stands(app, a)? {
            kept.push(a.clone());
        }
    }
    if kept.len() != rows.len() {
        keep_asks(app, &kept)?;
    }
    Ok(())
}

pub fn tick(app: &App) -> Result<()> {
    take_responses(app)?;
    prune_asks(app)?;
    prune_alerts(app)?;
    deliver_held(app)?;
    repeat_alerts(app)?;
    digest::tick(app)?;
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

    fn quiet_app() -> (tempfile::TempDir, std::sync::Arc<App>) {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = crate::config::Config::for_tests(dir.path());
        cfg.alerts.digest_mins = 0;
        let app = App::for_tests(cfg);
        hours::shut_for_tests(&app);
        (dir, app)
    }

    fn alert_for(app: &App, task: &str) -> Value {
        alerts(app).into_iter().find(|a| a["task"] == task).unwrap()
    }

    #[test]
    fn alerts_outside_hours_wait_for_them_to_open() {
        let (_d, app) = quiet_app();
        add_alert(&app, "T1: which port?", Some(1), None, None, None).unwrap();
        raise(&app, "main is red", Some(2), None, Some("master:1"), true).unwrap();
        assert_eq!(alert_for(&app, "T1")["held"], true);
        assert!(alert_for(&app, "T2")["held"].is_null(), "an urgent alert notifies at once");

        deliver_held(&app).unwrap();
        assert_eq!(alert_for(&app, "T1")["held"], true, "still shut");

        hours::set(&app, &json!({"on": false})).unwrap();
        let before = alert_for(&app, "T1");
        deliver_held(&app).unwrap();
        let after = alert_for(&app, "T1");
        assert!(after["held"].is_null());
        assert!(after["notified_at"].as_str() >= before["notified_at"].as_str());
        add_alert(&app, "T3: which color?", Some(3), None, None, None).unwrap();
        assert!(alert_for(&app, "T3")["held"].is_null(), "inside the hours an alert notifies at once");
    }

    #[test]
    fn a_held_alert_that_clears_or_snoozes_isnt_delivered_at_open() {
        let (_d, app) = quiet_app();
        add_alert(&app, "T1: which port?", Some(1), None, None, None).unwrap();
        add_alert(&app, "T2: which color?", Some(2), None, None, None).unwrap();
        clear_alerts(&app, Some(1), None).unwrap();
        let t2 = alert_for(&app, "T2");
        snooze_alert(&app, t2["id"].as_str().unwrap(), 30).unwrap();

        hours::set(&app, &json!({"on": false})).unwrap();
        deliver_held(&app).unwrap();
        assert_eq!(alerts(&app).len(), 1);
        let t2 = alert_for(&app, "T2");
        assert!(t2["held"].is_null());
        assert!(t2["snoozed_until"].is_string(), "its snooze notifies it when it ends");
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
