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
        // QA moving the ticket on from the merged status is the usual flow, not a reopen: only a real
        // done status, or a move back to a new one, reopens it.
        let was_finished = (has(Some(&was)) && finished(app, &was)) || (has(Some(&from)) && finished(app, &from));
        board::update_task(app, t.id(), fields!["jira_status" => status.clone(), "jira_status_at" => at_iso.clone()])?;
        board::log_event(app, t.id(), who, "jira", &text)?;
        let open = t.s("status") != Some("done");
        let alert = if open && done && !was_done {
            Some(format!("{text}, but {} is still open.", rf("task", t.id())))
        } else if !open && !done && (was_finished || is_new(&status, &category)) {
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

/// A Jira alert (`jira:<task>:<status>`) holds until the task and its ticket agree again: a reopen alert
/// until the task is reopened or the ticket is back in a done status, a "still open" one until the task
/// finishes or the ticket leaves the done statuses. Neither clears because the task is done or Jira
/// logged another move.
pub fn alert_resolved(app: &App, a: &Value) -> Result<bool> {
    let Some(t) = board::find_task(app, a["task_id"].as_i64())? else { return Ok(true) };
    let task_done = t.s("status") == Some("done");
    let ticket_done = is_done(app, &t.st("jira_status"), "");
    let reopen = a["text"].as_str().is_some_and(|x| x.contains(", reopening it, but "));
    Ok(if reopen { !task_done || ticket_done } else { task_done || !ticket_done })
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

/// Jira's usual statuses for work not started, for a status without its category.
const NEW_NAMES: &[&str] = &["to do", "todo", "open", "new", "backlog", "selected for development", "reopened"];

/// A status that starts the work over: category `new`, or, without a category, Jira's usual names for one.
fn is_new(status: &str, category: &str) -> bool {
    if !category.is_empty() {
        return category == "new";
    }
    NEW_NAMES.iter().any(|s| s.eq_ignore_ascii_case(status.trim()))
}

/// A really finished status: `done`, `canceled` or Jira's usual done names, but not `merged`, which
/// hands the ticket on to QA.
fn finished(app: &App, status: &str) -> bool {
    let j = &app.cfg.jira;
    let status = status.trim();
    let named = |s: &str| !s.trim().is_empty() && s.trim().eq_ignore_ascii_case(status);
    if named(&j.done) || named(&j.canceled) {
        return true;
    }
    !named(&j.merged) && DONE_NAMES.iter().any(|s| s.eq_ignore_ascii_case(status))
}

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
