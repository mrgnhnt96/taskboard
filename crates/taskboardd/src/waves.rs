//! Waves: a goal's tasks grouped into steps that run side by side. A wave starts once every wave
//! before it has passed: all its tasks done (none failed), or the owner let the goal go on past it
//! ("Continue to wave N"). A wave can stop the goal after it for the owner's review (`stop_after`,
//! set only from the app: it's the owner's word). Anyone can hold a wave (`held_at`): its tasks
//! don't start, nor anything after it, until it's continued.
//! Names, stop points and holds live in `goal_waves`; the wave of each task is `tasks.wave`.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, p};

fn settings(app: &App, goal_id: i64) -> Result<HashMap<i64, Row>> {
    Ok(app.db.q("SELECT * FROM goal_waves WHERE goal_id = ?", p![goal_id])?.into_iter().filter_map(|w| w.i("wave").map(|n| (n, w))).collect())
}

pub fn uses_waves(tasks: &[Row]) -> bool {
    tasks.iter().any(|t| t.i("wave").is_some())
}

/// The goal's waves in order, each with its state: done, stopped (done, waiting for the owner's
/// review), failed, running, held (held before it started), ready (may start) or waiting (an
/// earlier wave holds it).
pub fn waves(app: &App, g: &Row, tasks: &[Row]) -> Result<Vec<Value>> {
    let set = settings(app, g.id())?;
    let mut nums: Vec<i64> = tasks.iter().filter_map(|t| t.i("wave")).collect();
    nums.sort();
    nums.dedup();
    let mut out: Vec<Value> = vec![];
    let mut holding: Option<Value> = None;
    for n in nums {
        let ts: Vec<&Row> = tasks.iter().filter(|t| t.i("wave") == Some(n)).collect();
        let w = set.get(&n).cloned().unwrap_or_default();
        let done: Vec<&&Row> = ts.iter().filter(|t| t.s("status") == Some("done")).collect();
        let failed: Vec<String> = done.iter().filter(|t| t.b("failed")).map(|t| rf("task", t.id())).collect();
        let starting = ts.iter().filter(|t| t.s("status") == Some("queued") && t.i("start_job").is_some()).count();
        let active = ts.iter().filter(|t| matches!(t.s("status"), Some("working") | Some("needs"))).count() + starting;
        let all_done = done.len() == ts.len();
        let stop = w.b("stop_after");
        let released = w.s("released_at").is_some();
        let held = w.s("held_at").is_some() && !all_done;
        let passed = !held && (released || (all_done && failed.is_empty() && !stop));
        let state = if all_done && failed.is_empty() {
            if stop && !released { "stopped" } else { "done" }
        } else if all_done {
            "failed"
        } else if active > 0 {
            "running"
        } else if held {
            "held"
        } else if holding.is_some() {
            "waiting"
        } else {
            "ready"
        };
        let wv = json!({
            "wave": n, "name": w.s("name").unwrap_or(""), "stop_after": stop, "released_at": w.v("released_at"),
            "held": held, "held_at": if held { w.v("held_at") } else { Value::Null },
            "passed": passed, "state": state, "blocked": holding.is_some(),
            "held_by": holding.as_ref().map(|h| h["state"].clone()).unwrap_or(Value::Null),
            "hold": holding.as_ref().map(|h| json!(hold_reason(h))).unwrap_or(Value::Null),
            "tasks": ts.iter().map(|t| rf("task", t.id())).collect::<Vec<_>>(), "total": ts.len(), "done": done.len(),
            "failed": failed, "active": active, "starting": starting,
            "planned": ts.iter().filter(|t| t.s("status") == Some("planned")).count(),
        });
        if !passed && holding.is_none() {
            holding = Some(wv.clone());
        }
        out.push(wv);
    }
    Ok(out)
}

/// "wave 2 (API)".
pub fn wave_label(w: &Value) -> String {
    let name = w["name"].as_str().unwrap_or("");
    format!("wave {}{}", w["wave"], if name.is_empty() { String::new() } else { format!(" ({name})") })
}

pub fn hold_reason(w: &Value) -> String {
    let failed: Vec<&str> = w["failed"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    if w["state"] == "stopped" {
        format!("Stopped for your review after {}", wave_label(w))
    } else if w["held"] == true {
        format!("{} is held until it's continued", capital(&wave_label(w)))
    } else if !failed.is_empty() {
        format!("{} in {} failed. Try again, or continue past it", failed.join(", "), wave_label(w))
    } else {
        format!("Waits for {}", wave_label(w))
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// The earlier wave that holds this task back, or its own wave when that's held.
pub fn holding_wave(t: &Row, wl: &[Value]) -> Option<Value> {
    let n = t.i("wave")?;
    wl.iter()
        .take_while(|w| w["wave"].as_i64().unwrap_or(0) <= n)
        .find(|w| (w["wave"].as_i64().unwrap_or(0) < n && w["passed"] != true) || (w["wave"].as_i64() == Some(n) && w["held"] == true))
        .cloned()
}

/// Why a goal's waves hold this queued task back.
pub fn wave_block(app: &App, t: &Row, g: &Row) -> Result<Option<String>> {
    if t.i("wave").is_none() {
        return Ok(None);
    }
    let tasks = board::goal_tasks(app, g.id())?;
    Ok(holding_wave(t, &waves(app, g, &tasks)?).map(|w| hold_reason(&w)))
}

/// The goal is stopped at a wave for the owner (review stop or a failed task): one line, or None.
pub fn stopped_line(wl: &[Value]) -> Option<String> {
    let w = wl.iter().find(|w| w["passed"] != true)?;
    match w["state"].as_str() {
        Some("stopped") => Some(format!("W{} is done. Review it, then continue", &wave_label(w)[1..])),
        Some("failed") => Some(format!(
            "{} in {} failed",
            w["failed"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
            wave_label(w)
        )),
        _ => None,
    }
}

/// A wave's name and stop point (`POST /goals/:id/waves/:n`, `tb goal wave`).
pub fn set_wave(app: &App, goal_id: i64, n: i64, f: Vec<(&str, Value)>) -> Result<()> {
    app.db.x("INSERT INTO goal_waves(goal_id, wave) VALUES(?, ?) ON CONFLICT(goal_id, wave) DO NOTHING", p![goal_id, n])?;
    for (k, v) in f {
        app.db.x(&format!("UPDATE goal_waves SET {k} = ? WHERE goal_id = ? AND wave = ?"), vec![v, json!(goal_id), json!(n)])?;
    }
    Ok(())
}

/// "Continue to wave N": the goal goes on past this wave (a review stop or a failed task). A held
/// wave that isn't done is let start instead.
pub fn release(app: &App, goal_id: i64, n: i64, who: &str) -> Result<()> {
    let g = board::get_goal(app, goal_id)?;
    let ts: Vec<Row> = board::goal_tasks(app, goal_id)?.into_iter().filter(|t| t.i("wave") == Some(n)).collect();
    if ts.is_empty() {
        return err(404, format!("Wave {n} has no tasks."));
    }
    let held = settings(app, goal_id)?.get(&n).map(|w| w.s("held_at").is_some()).unwrap_or(false);
    if held && !ts.iter().all(|t| t.s("status") == Some("done")) {
        app.db.x("UPDATE goal_waves SET held_at = NULL WHERE goal_id = ? AND wave = ?", p![goal_id, n])?;
        for t in &ts {
            board::log_event(app, t.id(), who, "status", &format!("{who} let wave {n} start"))?;
        }
        return Ok(());
    }
    set_wave(app, goal_id, n, vec![("released_at", json!(now_iso()))])?;
    for t in &ts {
        board::log_event(app, t.id(), who, "status", &format!("{who} let the goal go on past wave {n}"))?;
    }
    if g.b("paused") {
        board::update_goal(app, goal_id, crate::fields!["paused" => 0, "updated_at" => now_iso()])?;
    }
    Ok(())
}

/// Holds a wave (`POST /goals/:id/waves/:n/hold`, `tb goal wave --hold`): none of its tasks that
/// haven't started start, nor any later wave, until it's continued. `on: false` lifts the hold.
pub fn hold(app: &App, goal_id: i64, n: i64, on: bool, who: &str) -> Result<()> {
    board::get_goal(app, goal_id)?;
    let ts: Vec<Row> = board::goal_tasks(app, goal_id)?.into_iter().filter(|t| t.i("wave") == Some(n)).collect();
    if ts.is_empty() {
        return err(404, format!("Wave {n} has no tasks."));
    }
    if on && ts.iter().all(|t| t.s("status") == Some("done")) {
        return err(409, format!("Wave {n} is done, so there's nothing left in it to hold."));
    }
    set_wave(app, goal_id, n, vec![("held_at", if on { json!(now_iso()) } else { Value::Null })])?;
    for t in &ts {
        board::log_event(app, t.id(), who, "status", &if on { format!("{who} held wave {n}") } else { format!("{who} let wave {n} start") })?;
    }
    Ok(())
}
