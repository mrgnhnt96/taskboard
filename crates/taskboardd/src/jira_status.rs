//! A ticket's status, as someone moved it in Jira: from a `jira.issue` stream event, or from the QA
//! poll's light status read in board mode. Finds the task, goal epic or backlog issue with that key,
//! records the status and logs who moved it. It never changes a task: when an open task's ticket is
//! done, or a done task's ticket reopened, it raises an alert instead. Moves the board made itself
//! (a recent transition job to that status) are skipped, and so is an event older than the status
//! the board keeps (`jira_status_at`): a relay catching up mustn't move a ticket back.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, dispatch, fields, p};

/// How long after the board transitions a ticket a matching status counts as its own echo.
const SELF_SECS: f64 = 900.0;

pub fn apply(app: &App, ev: &Value) -> Result<Value> {
    let key = body_str(ev, "key").trim().to_uppercase();
    let status = body_str(ev, "status").trim().to_string();
    if key.is_empty() || status.is_empty() {
        return err(400, "A jira.issue event needs key and status.");
    }
    let category = body_str(ev, "status_category").trim().to_lowercase();
    let by = body_str(ev, "changed_by").trim().to_string();
    let from = body_str(ev, "from_status").trim().to_string();
    if board_moved(app, &key, &status)? {
        return Ok(json!({"key": key, "skipped": "the board moved it"}));
    }
    // A stream event says when the status was true; the QA poll's read is as of now.
    let at = parse_iso(&body_str(ev, "at")).unwrap_or_else(now_ts);
    let at_iso = iso(at);
    let mut older = 0;
    let done = is_done(app, &status, &category);
    let text = if by.is_empty() { format!("{key} moved to {status}") } else { format!("{by} moved {key} to {status}") };
    let who = if by.is_empty() { "Jira" } else { by.as_str() };

    let mut tasks = vec![];
    let mut alerts = 0;
    for t in app.db.q("SELECT * FROM tasks WHERE UPPER(jira_key) = ?", p![key.clone()])? {
        let was = t.st("jira_status");
        if stored_newer(&t, at) {
            older += 1;
            continue;
        }
        if was.eq_ignore_ascii_case(&status) {
            app.db.update("tasks", &json!(t.id()), fields!["jira_status_at" => at_iso.clone()])?;
            continue;
        }
        let was_done = (has(Some(&was)) && is_done(app, &was, "")) || (has(Some(&from)) && is_done(app, &from, ""));
        board::update_task(app, t.id(), fields!["jira_status" => status.clone(), "jira_status_at" => at_iso.clone()])?;
        board::log_event(app, t.id(), who, "jira", &text)?;
        let open = t.s("status") != Some("done");
        let alert = if open && done && !was_done {
            Some(format!("{text}, but {} is still open.", rf("task", t.id())))
        } else if !open && !done && (was_done || category == "new") {
            Some(format!("{text}, reopening it, but {} is done.", rf("task", t.id())))
        } else {
            None
        };
        if let Some(line) = alert {
            dispatch::add_alert_keyed(app, &line, Some(t.id()), t.i("goal_id"), &format!("jira:{}:{}", t.id(), status.to_lowercase()))?;
            alerts += 1;
        }
        tasks.push(json!(rf("task", t.id())));
    }

    let mut goals = vec![];
    for g in app.db.q("SELECT id, epic_status, jira_status_at FROM goals WHERE UPPER(epic_key) = ?", p![key.clone()])? {
        if stored_newer(&g, at) {
            older += 1;
            continue;
        }
        if g.st("epic_status").eq_ignore_ascii_case(&status) {
            app.db.update("goals", &json!(g.id()), fields!["jira_status_at" => at_iso.clone()])?;
            continue;
        }
        app.db.update("goals", &json!(g.id()), fields!["epic_status" => status.clone(), "jira_status_at" => at_iso.clone(), "updated_at" => now_iso()])?;
        app.info(format!("jira: {text} ({})", rf("goal", g.id())));
        goals.push(json!(rf("goal", g.id())));
    }

    let mut issues = vec![];
    for i in app.db.q("SELECT id, jira_status, jira_status_at FROM issues WHERE UPPER(jira_key) = ?", p![key.clone()])? {
        if stored_newer(&i, at) {
            older += 1;
            continue;
        }
        if i.st("jira_status").eq_ignore_ascii_case(&status) {
            app.db.update("issues", &json!(i.id()), fields!["jira_status_at" => at_iso.clone()])?;
            continue;
        }
        app.db.update("issues", &json!(i.id()), fields!["jira_status" => status.clone(), "jira_status_at" => at_iso.clone()])?;
        board::add_issue_event(app, i.id(), who, "jira", &text, None)?;
        issues.push(json!(rf("issue", i.id())));
    }

    if older > 0 && tasks.is_empty() && goals.is_empty() && issues.is_empty() {
        return Ok(json!({"key": key, "skipped": "older than the stored status"}));
    }
    Ok(json!({"key": key, "status": status, "tasks": tasks, "goals": goals, "issues": issues, "alerts": alerts}))
}

/// The status the board keeps for this row was true after `at`.
fn stored_newer(row: &Row, at: f64) -> bool {
    row.s("jira_status_at").and_then(parse_iso).is_some_and(|kept| kept > at)
}

/// Whether the board itself just moved `key` to `status`: a Jira job, not failed, in the last few minutes.
fn board_moved(app: &App, key: &str, status: &str) -> Result<bool> {
    let since = iso(now_ts() - SELF_SECS);
    let jobs = app.db.q(
        "SELECT args, result FROM jobs WHERE kind = 'jira' AND state NOT IN ('failed', 'expired') AND updated_at >= ?",
        p![since],
    )?;
    Ok(jobs.iter().any(|j| {
        let (a, r) = (jloads_obj(j.s("args")), jloads_obj(j.s("result")));
        let same = |row: &Row, k: &str, want: &str| row.s(k).is_some_and(|v| v.trim().eq_ignore_ascii_case(want));
        (same(&a, "key", key) || same(&r, "key", key)) && (same(&a, "status", status) || same(&r, "status", status))
    }))
}

/// Jira's usual done statuses, for a status without its category.
const DONE_NAMES: &[&str] = &["done", "closed", "resolved", "canceled", "cancelled", "merged", "released"];

/// A done status: by its category when Jira gave one, else by the names the board moves tickets to
/// when done, or Jira's usual ones.
fn is_done(app: &App, status: &str, category: &str) -> bool {
    if !category.is_empty() {
        return category == "done";
    }
    let j = &app.cfg.jira;
    let status = status.trim();
    [&j.done, &j.merged, &j.canceled].iter().any(|s| !s.trim().is_empty() && s.trim().eq_ignore_ascii_case(status))
        || DONE_NAMES.iter().any(|s| s.eq_ignore_ascii_case(status))
}
