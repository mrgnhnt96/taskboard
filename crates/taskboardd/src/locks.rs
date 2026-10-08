//! Named locks and tasks that run alone. Tasks that name the same lock never run together, across the
//! board; a task that runs alone waits until its goal (or the board) is quiet, then nothing else in that
//! scope starts until it's done. A task holds its locks while it's active: working or needing the owner
//! (not a failed start), or queued with a start job.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, hours, jira, p, waitsfor};

static NAME: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[a-z0-9][a-z0-9._:-]{0,63}$").unwrap());

const ACTIVE_SQL: &str = "((t.status IN ('working', 'needs') AND COALESCE(t.needs_reason, '') != 'start_failed') \
                          OR (t.status = 'queued' AND t.start_job IS NOT NULL))";

/// "Runs alone in its goal" / "Runs alone on the board".
pub fn alone_text(scope: &str) -> &'static str {
    match scope {
        "board" => "Runs alone on the board",
        "goal" => "Runs alone in its goal",
        _ => "Runs alongside other tasks",
    }
}

pub fn names(t: &Row) -> Vec<String> {
    jloads_arr(t.s("locks")).into_iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
}

/// A lock list from a body (a list, or names split by commas or spaces), stored as JSON; none clears.
pub fn clean(value: Option<&Value>) -> Result<Option<String>> {
    let items: Vec<String> = match value {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(a)) => a.iter().map(|v| v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string())).collect(),
        Some(Value::String(s)) => s.replace(',', " ").split_whitespace().map(|s| s.to_string()).collect(),
        Some(v) => vec![v.to_string()],
    };
    let mut out: Vec<String> = vec![];
    for v in items {
        let v = v.trim().to_lowercase();
        if v.is_empty() || v == "none" {
            continue;
        }
        if !NAME.is_match(&v) {
            return err(400, format!("“{v}” can't be a lock name. Use lowercase letters, numbers, dots, dashes, colons or underscores, like local-core."));
        }
        if !out.contains(&v) {
            out.push(v);
        }
    }
    Ok(if out.is_empty() { None } else { Some(jdumps(&json!(out))) })
}

/// `goal`, `board` or none.
pub fn clean_alone(value: Option<&Value>) -> Result<Option<String>> {
    let v = match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => return Ok(None),
        Some(Value::Bool(true)) => return Ok(Some("goal".into())),
        Some(Value::String(s)) => s.trim().to_lowercase(),
        Some(v) => v.to_string(),
    };
    match v.as_str() {
        "" | "none" | "off" | "no" | "false" => Ok(None),
        "goal" | "yes" | "on" | "true" => Ok(Some("goal".into())),
        "board" => Ok(Some("board".into())),
        _ => err(400, "Alone takes goal, board or none."),
    }
}

/// Lock names no other task uses yet: probably a spelling slip when the lock is meant to be shared.
pub fn unseen_warning(app: &App, value: Option<&str>, tid: Option<i64>) -> Result<Option<String>> {
    let mut fresh: Vec<String> = jloads_arr(value).into_iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
    if fresh.is_empty() {
        return Ok(None);
    }
    for t in app.db.q("SELECT locks FROM tasks WHERE locks IS NOT NULL AND id != ?", p![tid.unwrap_or(0)])? {
        let held = names(&t);
        fresh.retain(|n| !held.contains(n));
    }
    if fresh.is_empty() {
        return Ok(None);
    }
    fresh.sort();
    Ok(Some(format!(
        "No other task uses the lock{} {} yet. Check the spelling if it's meant to be shared (tb locks lists them all).",
        if fresh.len() > 1 { "s" } else { "" },
        fresh.join(", ")
    )))
}

/// Does `alone_task`'s scope take in `other`?
fn covers(alone_task: &Row, other: &Row) -> bool {
    if alone_task.s("alone") == Some("board") {
        return true;
    }
    match alone_task.i("goal_id") {
        Some(g) => other.i("goal_id") == Some(g),
        None => other.i("goal_id").is_none() && alone_task.s("project") == other.s("project"),
    }
}

fn active(app: &App, but: i64) -> Result<Vec<Row>> {
    app.db.q(&format!("SELECT t.* FROM tasks t WHERE {ACTIVE_SQL} AND t.id != ? ORDER BY t.id"), p![but])
}

fn start_order(t: &Row) -> (i64, String, i64) {
    (if t.s("priority") == Some("high") { 0 } else { 1 }, t.st("created_at"), t.id())
}

fn refs_text(ts: &[&Row]) -> String {
    let rs: Vec<String> = ts.iter().map(|x| rf("task", x.id())).collect();
    match rs.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => rs.join(""),
    }
}

/// Why a lock or a task running alone holds this task back.
pub fn blocker(app: &App, t: &Row) -> Result<Option<String>> {
    let others = active(app, t.id())?;
    let mine = names(t);
    for x in &others {
        if let Some(held) = names(x).into_iter().find(|n| mine.contains(n)) {
            return Ok(Some(format!("Waits for {held} ({} has it)", rf("task", x.id()))));
        }
    }
    for x in &others {
        if x.s("alone").is_some() && covers(x, t) {
            return Ok(Some(format!("Waits while {} runs alone", rf("task", x.id()))));
        }
    }
    if t.s("alone").is_some() {
        let busy: Vec<&Row> = others.iter().filter(|x| covers(t, x)).collect();
        if !busy.is_empty() {
            return Ok(Some(format!("Waits to run alone ({} {} working)", refs_text(&busy), if busy.len() == 1 { "is" } else { "are" })));
        }
    }
    if let Some(first) = alone_ahead(app, t)? {
        return Ok(Some(format!("Waits for {} to run alone first", rf("task", first.id()))));
    }
    Ok(None)
}

/// A queued task that runs alone, ahead of this one in start order and ready but for the others.
fn alone_ahead(app: &App, t: &Row) -> Result<Option<Row>> {
    let mine = start_order(t);
    for x in app.db.q(
        "SELECT * FROM tasks WHERE status = 'queued' AND alone IS NOT NULL AND id != ? AND session_id IS NULL AND start_job IS NULL",
        p![t.id()],
    )? {
        if start_order(&x) < mine && covers(&x, t) && ready_but_for_others(app, &x)? {
            return Ok(Some(x));
        }
    }
    Ok(None)
}

fn ready_but_for_others(app: &App, t: &Row) -> Result<bool> {
    if t.s("pickup") == Some("manual") {
        return Ok(false);
    }
    let g = board::find_goal(app, t.i("goal_id"))?;
    if !hours::may_start(app) && !hours::goal_open(app, g.as_ref()) {
        return Ok(false);
    }
    if jira::ticket_blocker(app, t)? || waitsfor::blocker(app, t)?.is_some() {
        return Ok(false);
    }
    match g {
        Some(g) => Ok(crate::runner::goal_order_blocker(app, t, &g)?.is_none()),
        None => Ok(true),
    }
}

/// `GET /locks`: each lock, the task holding it and those waiting or planned; tasks that run alone.
pub fn overview(app: &App) -> Result<Value> {
    let rows = app.db.q(
        "SELECT * FROM tasks WHERE status IN ('planned', 'queued', 'working', 'needs') \
         AND ((locks IS NOT NULL AND locks != '') OR alone IS NOT NULL) ORDER BY id",
        p![],
    )?;
    let holding: Vec<i64> = active(app, 0)?.iter().map(|x| x.id()).collect();
    let mut by_name: Vec<Value> = vec![];
    for t in &rows {
        for n in names(t) {
            let i = match by_name.iter().position(|e| e["name"] == n.as_str()) {
                Some(i) => i,
                None => {
                    by_name.push(json!({"name": n, "held_by": null, "tasks": []}));
                    by_name.len() - 1
                }
            };
            if holding.contains(&t.id()) {
                by_name[i]["held_by"] = json!(rf("task", t.id()));
            } else if let Some(a) = by_name[i]["tasks"].as_array_mut() {
                a.push(json!(rf("task", t.id())));
            }
        }
    }
    by_name.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let alone: Vec<Value> = rows
        .iter()
        .filter(|t| t.s("alone").is_some())
        .map(|t| {
            json!({"ref": rf("task", t.id()), "title": t.v("title"), "scope": t.v("alone"),
                   "goal": rf_opt("goal", t.i("goal_id")), "running": holding.contains(&t.id())})
        })
        .collect();
    Ok(json!({"locks": by_name, "alone": alone}))
}
