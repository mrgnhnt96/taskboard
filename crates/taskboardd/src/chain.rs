//! Goals that run after other goals (`tb goal set G2 --after G1`): a chained goal's tasks wait until
//! every goal it follows is finished, and once the last one finishes the board runs it.

use serde_json::{json, Value};

use crate::app::App;
use crate::board;
use crate::fields;
use crate::util::*;

pub fn ids(g: &Row) -> Vec<i64> {
    jloads_arr(g.s("after_goals")).iter().filter_map(|x| x.as_i64()).collect()
}

/// The goals `value` names for `g` to follow, as stored: none clears them. Refuses itself and a loop.
pub fn clean(app: &App, value: &Value, g: Option<&Row>) -> Result<Option<String>> {
    let items: Vec<String> = match value {
        Value::Null => return Ok(None),
        Value::String(s) => s.replace(',', " ").split_whitespace().map(|x| x.to_string()).collect(),
        Value::Array(a) => a.iter().map(|x| x.as_str().map(|s| s.to_string()).unwrap_or_else(|| x.to_string())).collect(),
        other => vec![other.to_string()],
    };
    let mut out: Vec<i64> = vec![];
    for v in items {
        if v.eq_ignore_ascii_case("none") {
            continue;
        }
        let Some(n) = parse_ref_str(&v, "goal")? else { continue };
        board::get_goal(app, n)?;
        if let Some(g) = g {
            if n == g.id() {
                return err(400, "A goal can't run after itself.");
            }
            if reaches(app, n, g.id(), &mut vec![])? {
                return err(409, format!("{} already runs after {}, so this would wait forever.", rf("goal", n), rf("goal", g.id())));
            }
        }
        if !out.contains(&n) {
            out.push(n);
        }
    }
    Ok(if out.is_empty() { None } else { Some(jdumps(&json!(out))) })
}

fn reaches(app: &App, start: i64, target: i64, seen: &mut Vec<i64>) -> Result<bool> {
    if start == target {
        return Ok(true);
    }
    if seen.contains(&start) {
        return Ok(false);
    }
    seen.push(start);
    let Some(g) = board::find_goal(app, Some(start))? else { return Ok(false) };
    for n in ids(&g) {
        if reaches(app, n, target, seen)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Why the goals `g` follows hold its tasks back: the first one not finished yet.
pub fn blocker(app: &App, g: &Row) -> Result<Option<String>> {
    for n in ids(g) {
        let Some(o) = board::find_goal(app, Some(n))? else { continue };
        if !board::goal_finished(app, n)? {
            return Ok(Some(format!("Its goal runs after {} ({}), which isn't finished yet", rf("goal", n), o.st("name"))));
        }
    }
    Ok(None)
}

/// `g` just finished: runs each goal that follows it and now follows only finished goals.
pub fn finished(app: &App, g: &Row) -> Result<()> {
    let pattern = format!("%{}%", g.id());
    for next in app.db.q("SELECT * FROM goals WHERE after_goals LIKE ? AND COALESCE(archived, 0) = 0", crate::p![pattern])? {
        if !ids(&next).contains(&g.id()) || blocker(app, &next)?.is_some() {
            continue;
        }
        let why = format!("Queued: {} ({}) finished, and this goal runs after it", rf("goal", g.id()), g.st("name"));
        queue_planned(app, next.id(), board::BOARD, &why, &why)?;
        if next.b("paused") || next.b("deprioritized") {
            board::update_goal(app, next.id(), fields!["paused" => 0, "deprioritized" => 0, "updated_at" => now_iso()])?;
        }
    }
    Ok(())
}

/// Queues the goal's planned tasks, and those whose start failed again; returns how many were planned.
/// A planned task that waited for Start before it joined the goal runs with the goal now.
pub fn queue_planned(app: &App, id: i64, who: &str, queued: &str, again: &str) -> Result<i64> {
    let mut n = 0;
    app.db.update("goals", &json!(id), fields!["ran_at" => now_iso()])?;
    for t in board::goal_tasks(app, id)? {
        if t.s("status") == Some("planned") {
            let mut f = fields!["status" => "queued"];
            if t.s("pickup") == Some("manual") && t.s("line_session").is_none() {
                f.extend(fields!["pickup" => "queue"]);
            }
            board::update_task(app, t.id(), f)?;
            board::log_event(app, t.id(), who, "status", queued)?;
            n += 1;
        } else if t.s("status") == Some("needs") && t.s("needs_reason") == Some("start_failed") {
            board::update_task(app, t.id(), fields!["status" => "queued", "needs_reason" => null, "start_job" => null, "start_tries" => 0, "retry_at" => null])?;
            board::log_event(app, t.id(), who, "status", again)?;
        }
    }
    Ok(n)
}
