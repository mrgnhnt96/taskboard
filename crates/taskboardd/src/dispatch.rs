//! Alerts and notifications, and when a new terminal may open.

use std::path::Path;

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

fn keep(app: &App, rows: &[Value]) -> Result<()> {
    app.db.set_setting("alerts", Some(&jdumps(&Value::Array(rows.to_vec()))))
}

fn new_alert_id() -> String {
    use rand::Rng;
    let n: u64 = rand::thread_rng().gen();
    format!("{n:016x}")[..10].to_string()
}

pub fn add_alert(app: &App, text: &str, task_id: Option<i64>, goal_id: Option<i64>, session_id: Option<&str>, extra: Option<&str>) -> Result<()> {
    let mut rows: Vec<Value> =
        alerts(app).into_iter().filter(|a| !(task_id.is_some() && a["task_id"].as_i64() == task_id)).collect();
    let now = now_iso();
    let mut new = json!({"id": new_alert_id(), "at": now, "text": text, "task_id": task_id, "session_id": session_id,
                         "task": rf_opt("task", task_id), "goal": rf_opt("goal", goal_id), "notified_at": now});
    if let Some(flag) = extra {
        new[flag] = json!(true);
    }
    rows.push(new);
    let n = rows.len();
    if n > 20 {
        rows.drain(..n - 20);
    }
    keep(app, &rows)?;
    notify(app, text, &alert_url(app, task_id, goal_id, session_id), None);
    Ok(())
}

pub fn clear_alerts(app: &App, task_id: Option<i64>, alert_id: Option<&str>) -> Result<()> {
    let rows: Vec<Value> = alerts(app)
        .into_iter()
        .filter(|a| {
            !((task_id.is_some() && a["task_id"].as_i64() == task_id) || (alert_id.is_some() && a["id"].as_str() == alert_id))
        })
        .collect();
    keep(app, &rows)
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
    if every <= 0.0 || !hours::is_open(app) {
        return Ok(());
    }
    let mut rows = alerts(app);
    let now = now_ts();
    let mut sent = false;
    for a in rows.iter_mut() {
        // A snooze runs to a clock time; a repeat waits for `every` of the time the Mac was awake.
        let due = match a["snoozed_until"].as_str().and_then(parse_iso) {
            Some(s) => now >= s,
            None => age_secs(a["notified_at"].as_str().or(a["at"].as_str())).unwrap_or(0.0) >= every,
        };
        if due {
            a["notified_at"] = json!(now_iso());
            a["snoozed_until"] = Value::Null;
            sent = true;
            let goal = a["goal"].as_str().and_then(|g| parse_ref_str(g, "goal").ok().flatten());
            notify(app, a["text"].as_str().unwrap_or(""), &alert_url(app, a["task_id"].as_i64(), goal, a["session_id"].as_str()), None);
        }
    }
    if sent {
        keep(app, &rows)?;
    }
    Ok(())
}

fn resolved(app: &App, a: &Value) -> Result<bool> {
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

fn prune_alerts(app: &App) -> Result<()> {
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
    prune_alerts(app)?;
    repeat_alerts(app)
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
