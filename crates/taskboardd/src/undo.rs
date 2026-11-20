//! Taking back a backlog act from the app's toast: closing issues (Defer, Won't do), Move to goal and
//! finishing a goal without a miss. Each act that can be taken back keeps a short-lived record here and
//! answers with its path (`backlog/undo/<n>`), which the toast's Undo posts. Records live in memory only:
//! they outlast the toast, not the board.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{blocks, board, fields, p};

/// How long an act can still be taken back.
const KEEP: Duration = Duration::from_secs(10 * 60);
/// The most records kept; the oldest goes first.
const MOST: usize = 50;

/// Keeps `rec` and returns the path that takes the act back.
pub fn keep(app: &App, rec: Value) -> String {
    let mut s = app.shared.lock();
    s.undo.retain(|(_, at, _)| at.elapsed() < KEEP);
    while s.undo.len() >= MOST {
        s.undo.remove(0);
    }
    s.undo_next += 1;
    let n = s.undo_next;
    s.undo.push((n, Instant::now(), rec));
    format!("backlog/undo/{n}")
}

/// Closing open issues (`state` is `defer` or `drop`): undone, each opens again.
pub fn closed(ids: &[i64], state: &str) -> Value {
    json!({"kind": "close", "ids": ids, "state": state})
}

/// An issue's goal, project and area before Move to goal, and the task the move made.
pub fn moved(issue: &Row, task: i64) -> Value {
    json!({"id": issue.id(), "task": task, "goal_id": issue.v("goal_id"), "project": issue.v("project"), "area": issue.v("area")})
}

/// A miss that stopped holding its goal (read before), so undone it holds the goal again.
pub fn without(issue: &Row) -> Value {
    json!({"kind": "without", "id": issue.id(), "goal_id": issue.v("goal_id"), "blocks": issue.v("blocks"), "blocks_why": issue.v("blocks_why")})
}

/// `POST backlog/undo/<n>`: takes the act back, all of it or nothing. A record is used once.
pub fn route(app: &App, n: &str, body: &Value) -> Result<Value> {
    let n: i64 = n.parse().map_err(|_| ApiError::new(404, "There's nothing at that address."))?;
    let rec = {
        let s = app.shared.lock();
        s.undo.iter().find(|(k, at, _)| *k == n && at.elapsed() < KEEP).map(|(_, _, r)| r.clone())
    };
    let Some(rec) = rec else { return err(410, "It's too late to undo that.") };
    let who = {
        let w = one_line(&body_str(body, "who"), 80);
        if w.is_empty() { board::OWNER.to_string() } else { w }
    };
    let (out, gone) = app.db.tx(|| match rec["kind"].as_str() {
        Some("close") => close(app, &rec, &who).map(|v| (v, vec![])),
        Some("goal") => goal(app, &rec, &who),
        Some("without") => finish(app, &rec, &who).map(|v| (v, vec![])),
        _ => err(410, "It's too late to undo that."),
    })?;
    crate::api::remove_md(app, &gone);
    app.shared.lock().undo.retain(|(k, _, _)| *k != n);
    Ok(out)
}

fn ids(rec: &Value) -> Vec<i64> {
    rec["ids"].as_array().into_iter().flatten().filter_map(Value::as_i64).collect()
}

fn close(app: &App, rec: &Value, who: &str) -> Result<Value> {
    let state = rec["state"].as_str().unwrap_or("");
    let ids = ids(rec);
    for id in &ids {
        if board::get_issue(app, *id)?.s("state") != Some(state) {
            return err(409, format!("{} changed since, so nothing was undone.", rf("issue", *id)));
        }
    }
    for id in &ids {
        crate::api::reopen_issue(app, *id, &json!({"who": who}))?;
    }
    Ok(json!({"ok": true, "undone": "close", "count": ids.len()}))
}

/// Move to goal, taken back: the task each issue became is removed (only while nothing has started it) and
/// the issue goes back to its old goal, project and area, open again.
fn goal(app: &App, rec: &Value, who: &str) -> Result<(Value, Vec<i64>)> {
    let to = rec["goal_id"].as_i64().unwrap_or(0);
    let items = rec["items"].as_array().cloned().unwrap_or_default();
    let mut tasks = vec![];
    for it in &items {
        let (id, t) = (it["id"].as_i64().unwrap_or(0), it["task"].as_i64().unwrap_or(0));
        let task = board::find_task(app, Some(t))?;
        let started = task.as_ref().is_some_and(|x| !matches!(x.s("status"), Some("planned" | "queued")) || x.s("session_id").is_some_and(|s| !s.is_empty()));
        if task.is_none() || started || board::get_issue(app, id)?.i("task_id") != Some(t) {
            return err(409, format!("{} has started or changed since, so nothing was undone.", rf("task", t)));
        }
        tasks.push(t);
    }
    crate::api::purge_tasks(app, &tasks)?;
    board::renumber_goal(app, to)?;
    let name = board::find_goal(app, Some(to))?.map(|g| g.st("name")).unwrap_or_else(|| rf("goal", to));
    let mut goals = vec![to];
    for it in &items {
        let id = it["id"].as_i64().unwrap_or(0);
        app.db.update("issues", &json!(id), fields!["state" => "open", "task_id" => null, "goal_id" => it["goal_id"].clone(),
            "project" => it["project"].clone(), "area" => it["area"].clone(), "updated_at" => now_iso()])?;
        board::add_issue_event(app, id, who, "move", &format!("Move to {name} undone"), None)?;
        if let Some(g) = it["goal_id"].as_i64() {
            goals.push(g);
        }
    }
    goals.sort();
    goals.dedup();
    for g in goals {
        app.db.x("UPDATE tasks SET ctx_version = COALESCE(ctx_version, 1) + 1 WHERE goal_id = ? AND status != 'done'", p![g])?;
    }
    Ok((json!({"ok": true, "undone": "goal", "count": items.len()}), tasks))
}

/// Finishing a goal without a miss, taken back: the miss holds its goal open again and its alert returns.
/// What ran when the goal finished (its hook, chained goals, retro) isn't taken back.
fn finish(app: &App, rec: &Value, who: &str) -> Result<Value> {
    let id = rec["id"].as_i64().unwrap_or(0);
    let b = board::get_issue(app, id)?;
    if b.s("state") != Some("open") || b.b("misses_goal") || b.v("goal_id") != rec["goal_id"] {
        return err(409, format!("{} changed since, so nothing was undone.", rf("issue", id)));
    }
    app.db.update("issues", &json!(id), fields!["misses_goal" => 1, "blocks" => rec["blocks"].clone(),
        "blocks_why" => rec["blocks_why"].clone(), "updated_at" => now_iso()])?;
    let goal = b.i("goal_id").map(|g| rf("goal", g)).unwrap_or_else(|| "the goal".into());
    board::add_issue_event(app, id, who, "note", &format!("Holds {goal} open again: finishing without it was undone"), None)?;
    if let Some(g) = b.i("goal_id") {
        board::add_goal_note(app, g, "decision", &format!("Not finishing without {} after all: {}", rf("issue", id), b.st("title")), Some(&rf("issue", id)), false, None)?;
        blocks::miss_alert(app, id, g, &b.st("title"))?;
    }
    Ok(json!({"ok": true, "undone": "without", "issue": crate::ops::issue_detail(app, id)?}))
}
