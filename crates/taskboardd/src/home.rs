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

/// The alerts, each with its task's card (or null) and its goal's name. A snoozed alert is left out,
/// and so is its task until the snooze ends.
fn needs(app: &App) -> Result<Vec<Value>> {
    let mut out = vec![];
    let (snoozed, up): (Vec<Value>, Vec<Value>) = crate::dispatch::listing(app).into_iter().partition(crate::dispatch::snoozed);
    for mut a in up {
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
    // A task that needs you with no alert up (dismissed, or never raised) still does, under a
    // `task:T<n>` id that Dismiss and Snooze put away (`put_away`).
    let away = put_away_list(app);
    for t in app.db.q("SELECT * FROM tasks WHERE status = 'needs' ORDER BY updated_at DESC", p![])? {
        let r = rf("task", t.id());
        if out.iter().chain(&snoozed).any(|a| a["task"].as_str() == Some(r.as_str())) {
            continue;
        }
        if away.get(&r).is_some_and(|p| p["asking"].as_str() == Some(asking(&t).as_str()) || crate::dispatch::snoozed(p)) {
            continue;
        }
        let card = board::task_card(app, &t)?;
        let text = card["question"].as_str().or(card["latest"].as_str()).unwrap_or("").to_string();
        out.push(json!({"id": format!("{TASK_ID}{r}"), "at": t.v("updated_at"), "text": text, "task": r, "goal": card["goal"]["ref"],
                        "goal_name": card["goal"]["name"], "card": card}));
    }
    Ok(out)
}

/// The id prefix of a Needs you item that's a task with no alert up: `task:T12`.
pub const TASK_ID: &str = "task:";

/// The tasks put away from Needs you while no alert is up, by ref: dismissed (`asking`, what it
/// asked then) or snoozed (`snoozed_until` / `snoozed_back`, as an alert's).
const PUT_AWAY: &str = "needs_put_away";

fn put_away_list(app: &App) -> Row {
    jloads_obj(crate::reqcache::setting(app, PUT_AWAY).ok().flatten().as_deref())
}

/// What a task that needs you is asking: a new question or a new reason brings a dismissed one back.
fn asking(t: &Row) -> String {
    format!("{}|{}|{}", t.st("needs_reason"), t.b("lost"), t.st("question"))
}

/// Dismiss (`until` None) or Snooze (`Some(until)`, as `dispatch::snooze_to`) on Needs you item
/// `id`, a `task:T<n>` with no alert up. Returns what was kept, for the app's toast.
pub fn put_away(app: &App, id: &str, until: Option<Option<f64>>) -> Result<Value> {
    let t = id.strip_prefix(TASK_ID).and_then(|r| r.trim_start_matches('T').parse::<i64>().ok()).map(|n| board::get_task(app, n));
    let Some(Ok(t)) = t.filter(|t| t.as_ref().is_ok_and(|t| t.s("status") == Some("needs"))) else {
        return err(404, "That task doesn't need you now.");
    };
    let entry = match until {
        None => json!({"asking": asking(&t)}),
        Some(until) => json!({"snoozed_until": until.map(iso), "snoozed_back": until.is_none().then(now_iso)}),
    };
    let needs: Vec<String> = app.db.q("SELECT id FROM tasks WHERE status = 'needs'", p![])?.iter().map(|t| rf("task", t.id())).collect();
    let mut away = put_away_list(app);
    away.retain(|r, _| needs.contains(r));
    away.insert(rf("task", t.id()), entry.clone());
    crate::reqcache::forget(&format!("setting:{PUT_AWAY}"));
    app.db.set_setting(PUT_AWAY, Some(&jdumps(&Value::Object(away))))?;
    Ok(entry)
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
