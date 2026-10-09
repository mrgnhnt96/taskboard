//! The home page (`GET /home`): the goals with work in flight, each with the tasks of the wave it's
//! on and the ones still open from earlier waves, the tasks outside any goal, and the alerts as the
//! "Needs you" list, each with its task's card.
//!
//! In flight: working, needs you, starting, or done with its PR still open. A goal's wave is the
//! latest one with a task in flight, else the first with a task queued; a goal without waves is one
//! wave. Planned tasks never show: nothing runs them until the owner starts the goal.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, p};

fn in_flight(t: &Row) -> bool {
    match t.s("status") {
        Some("working") | Some("needs") => true,
        Some("queued") => t.i("start_job").is_some(),
        Some("done") => !t.b("failed") && board::pr_still_open(t),
        _ => false,
    }
}

fn waiting(t: &Row) -> bool {
    t.s("status") == Some("queued") && t.i("start_job").is_none()
}

/// The wave a goal is on: the latest with a task in flight, else the first with one queued.
fn current_wave(tasks: &[Row]) -> Option<i64> {
    tasks.iter().filter(|t| in_flight(t)).filter_map(|t| t.i("wave")).max().or_else(|| tasks.iter().filter(|t| waiting(t)).filter_map(|t| t.i("wave")).min())
}

/// A task without a wave in a goal with waves runs with whichever wave is on.
fn in_wave(t: &Row, wave: Option<i64>) -> bool {
    wave.is_none() || t.i("wave").is_none_or(|w| w == wave.unwrap_or(w))
}

fn cards(app: &App, rows: &[&Row]) -> Result<Vec<Value>> {
    rows.iter().map(|t| board::task_card(app, t)).collect()
}

/// One goal's entry, or `None` when nothing in it is in flight or queued.
fn goal_entry(app: &App, g: &Row) -> Result<Option<Value>> {
    let tasks = board::goal_tasks(app, g.id())?;
    let wave = if crate::waves::uses_waves(&tasks) { current_wave(&tasks) } else { None };
    let now: Vec<&Row> = tasks.iter().filter(|t| in_flight(t) && in_wave(t, wave)).collect();
    let earlier = |t: &Row| matches!((t.i("wave"), wave), (Some(w), Some(cur)) if w < cur);
    let left: Vec<&Row> = tasks.iter().filter(|t| earlier(t) && (in_flight(t) || waiting(t))).collect();
    let queued = tasks.iter().filter(|t| waiting(t) && in_wave(t, wave)).count();
    if now.is_empty() && left.is_empty() && queued == 0 {
        return Ok(None);
    }
    let mut left_waves: Vec<i64> = left.iter().filter_map(|t| t.i("wave")).collect();
    left_waves.sort();
    left_waves.dedup();
    let mut waves: Vec<i64> = tasks.iter().filter_map(|t| t.i("wave")).collect();
    waves.sort();
    waves.dedup();
    Ok(Some(json!({
        "goal": board::goal_dict(app, g)?,
        "project": g.v("project"),
        "wave": wave,
        "waves": waves.len(),
        "now": cards(app, &now)?,
        "left": cards(app, &left)?,
        "left_waves": left_waves,
        "queued": queued,
    })))
}

/// Tasks outside any goal, one entry per project.
fn loose_entries(app: &App, project: &str) -> Result<Vec<Value>> {
    let rows = app.db.q("SELECT * FROM tasks WHERE goal_id IS NULL ORDER BY project, id", p![])?;
    let mut out: Vec<Value> = vec![];
    let mut projects: Vec<String> = rows.iter().filter_map(|t| t.s("project").map(str::to_string)).collect();
    projects.dedup();
    for name in projects.into_iter().filter(|n| project == "all" || n == project) {
        let mine: Vec<&Row> = rows.iter().filter(|t| t.s("project") == Some(name.as_str())).collect();
        let now: Vec<&Row> = mine.iter().copied().filter(|t| in_flight(t)).collect();
        let queued = mine.iter().filter(|t| waiting(t)).count();
        if now.is_empty() && queued == 0 {
            continue;
        }
        out.push(json!({"goal": null, "project": name, "wave": null, "waves": 0, "now": cards(app, &now)?, "left": [], "left_waves": [], "queued": queued}));
    }
    Ok(out)
}

/// Needs you before working before the rest; newest goal first within each.
fn rank(e: &Value) -> u8 {
    let now = e["now"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    if now.iter().any(|c| c["status"] == "needs") {
        0
    } else if now.iter().any(|c| c["status"] == "working") {
        1
    } else {
        2
    }
}

/// The alerts, each with its task's card (or null) and its goal's name.
fn needs(app: &App) -> Result<Vec<Value>> {
    let mut out = vec![];
    for mut a in crate::dispatch::listing(app) {
        let task = a["task"].as_str().and_then(|r| r.trim_start_matches('T').parse::<i64>().ok());
        let card = match task.map(|id| board::get_task(app, id)) {
            Some(Ok(t)) => board::task_card(app, &t)?,
            _ => Value::Null,
        };
        let goal_name = match a["goal"].as_str().and_then(|r| r.trim_start_matches('G').parse::<i64>().ok()) {
            Some(id) => app.db.val("SELECT name FROM goals WHERE id = ?", p![id])?,
            None => card["goal"]["name"].clone(),
        };
        a["card"] = card;
        a["goal_name"] = goal_name;
        out.push(a);
    }
    // A task that needs you with no alert up (dismissed, or never raised) still does.
    for t in app.db.q("SELECT * FROM tasks WHERE status = 'needs' ORDER BY updated_at DESC", p![])? {
        let r = rf("task", t.id());
        if out.iter().any(|a| a["task"].as_str() == Some(r.as_str())) {
            continue;
        }
        let card = board::task_card(app, &t)?;
        let text = card["question"].as_str().or(card["latest"].as_str()).unwrap_or("").to_string();
        out.push(json!({"id": null, "at": t.v("updated_at"), "text": text, "task": r, "goal": card["goal"]["ref"],
                        "goal_name": card["goal"]["name"], "card": card}));
    }
    Ok(out)
}

pub fn page(app: &App, project: &str) -> Result<Value> {
    let mut entries = vec![];
    for g in app.db.q("SELECT * FROM goals ORDER BY id DESC", p![])? {
        if g.b("archived") || g.b("deprioritized") || (project != "all" && g.s("project") != Some(project)) {
            continue;
        }
        if let Some(e) = goal_entry(app, &g)? {
            entries.push(e);
        }
    }
    entries.sort_by_key(rank);
    entries.extend(loose_entries(app, project)?);
    entries.sort_by(|a, b| a["project"].as_str().unwrap_or("").cmp(b["project"].as_str().unwrap_or("")));
    Ok(json!({"goals": entries, "needs": needs(app)?}))
}
