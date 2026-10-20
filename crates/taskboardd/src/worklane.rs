//! A task's lane: where it works while others run. `shared` works in the project's own checkout, `slot`
//! borrows a worktree from the pool (`slots.rs`), `alone` runs with nothing else in its goal beside it
//! (`locks.rs`), and `auto`, the default, leaves the pick to the board: a slot for a task in a wave whose
//! mates' planned files overlap its own, or that plans no files; else the shared checkout. A goal's lane
//! is what its tasks on `auto` take (a goal with a `worktree_base` is on `slot`). An auto pick is kept in
//! the task's context once it starts, so the task never moves folders part way through.

use std::path::Path;

use serde_json::{json, Value};

use crate::app::App;
use crate::board;
use crate::util::*;

pub const LANES: [&str; 4] = ["shared", "slot", "alone", "auto"];

/// A lane from a body: shared, slot, alone or auto (`default` and blank are auto); None when not given.
pub fn clean(value: Option<&Value>) -> Result<Option<String>> {
    let v = match value {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) => s.trim().to_lowercase(),
        Some(v) => v.to_string(),
    };
    match v.as_str() {
        "" | "default" => Ok(Some("auto".into())),
        l if LANES.contains(&l) => Ok(Some(l.to_string())),
        _ => err(400, format!("“{v}” isn't a lane. Give shared, slot, alone or auto.")),
    }
}

/// A lane that's set: anything but auto.
fn chosen(lane: Option<&str>) -> Option<&str> {
    lane.filter(|l| LANES.contains(l) && *l != "auto")
}

/// A goal's lane: its own, else `slot` when it has a worktree base, else auto.
pub fn goal_lane(g: &Row) -> &str {
    match chosen(g.s("lane")) {
        Some(l) => l,
        None if g.s("worktree_base").is_some_and(|b| !b.is_empty()) => "slot",
        None => "auto",
    }
}

/// The task's own lane: what it was given, else `alone` when it runs alone, else auto.
pub fn own_lane(t: &Row) -> &str {
    match chosen(t.s("lane")) {
        Some(l) => l,
        None if t.s("alone").is_some() => "alone",
        None => "auto",
    }
}

/// Where the task works: shared, slot or alone.
pub fn pick(app: &App, t: &Row) -> Result<String> {
    Ok(picked(app, t)?.0)
}

/// The lane and how it came: `task` (its own), `goal` (its goal's), or `auto` (the board's pick).
pub fn picked(app: &App, t: &Row) -> Result<(String, &'static str)> {
    let own = own_lane(t);
    if own != "auto" {
        return Ok((own.to_string(), "task"));
    }
    let g = board::find_goal(app, t.i("goal_id"))?;
    if let Some(l) = g.as_ref().map(goal_lane).filter(|l| *l != "auto") {
        return Ok((l.to_string(), "goal"));
    }
    if let Some(l) = chosen(board::task_context(t).s("lane")) {
        return Ok((l.to_string(), "auto"));
    }
    let slot = match &g {
        Some(g) => auto_slot(app, t, g)?,
        None => false,
    };
    Ok((if slot { "slot" } else { "shared" }.to_string(), "auto"))
}

/// The files a wave plans for the task (`--file`).
fn plan_files(t: &Row) -> Vec<String> {
    str_list(board::task_context(t).get("plan_files"))
}

/// Auto's pick in a goal: a slot when its wave runs others beside it and either side plans no files or
/// their files overlap. Only in a git checkout, where there's a pool to borrow from.
fn auto_slot(app: &App, t: &Row, g: &Row) -> Result<bool> {
    let Some(n) = t.i("wave") else { return Ok(false) };
    if g.i("max_terminals").unwrap_or(2) <= 1 {
        return Ok(false);
    }
    let repo = crate::worktrees::repo_of(app, t)?;
    if !repo.is_some_and(|r| Path::new(&r).join(".git").exists()) {
        return Ok(false);
    }
    let own = plan_files(t);
    for m in board::goal_tasks(app, g.id())? {
        if m.id() == t.id() || m.i("wave") != Some(n) || m.s("status") == Some("done") {
            continue;
        }
        let theirs = plan_files(&m);
        if own.is_empty() || theirs.is_empty() || theirs.iter().any(|f| own.contains(f)) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Keeps auto's pick in the task's context as it starts.
pub fn pin(app: &App, t: &Row, lane: &str) -> Result<()> {
    let mut ctx = board::task_context(&board::get_task(app, t.id())?);
    if ctx.s("lane") == Some(lane) {
        return Ok(());
    }
    ctx.insert("lane".into(), json!(lane));
    board::save_context(app, t.id(), &ctx, false)
}

/// Forgets a kept pick, when the task's or goal's lane is changed.
pub fn unpin(app: &App, tid: i64) -> Result<()> {
    let mut ctx = board::task_context(&board::get_task(app, tid)?);
    if ctx.remove("lane").is_some() {
        board::save_context(app, tid, &ctx, false)?;
    }
    Ok(())
}

/// The scope the task runs alone in: its own `alone`, else `goal` when it takes its goal's `alone` lane.
pub fn alone_scope(app: &App, t: &Row) -> Result<Option<String>> {
    if let Some(a) = t.s("alone") {
        return Ok(Some(a.to_string()));
    }
    if own_lane(t) != "auto" {
        return Ok(None);
    }
    Ok(board::find_goal(app, t.i("goal_id"))?.filter(|g| goal_lane(g) == "alone").map(|_| "goal".to_string()))
}

/// "Works in the shared checkout", "Works in a worktree slot" or "Runs alone in its goal".
pub fn text(lane: &str) -> &'static str {
    match lane {
        "slot" => "Works in a worktree slot",
        "alone" => "Runs alone in its goal",
        "shared" => "Works in the shared checkout",
        _ => "The board picks its lane",
    }
}

/// A task's lane for its card and `tb task show`: what it's set to, where it works, and why.
pub fn card(app: &App, t: &Row) -> Result<Value> {
    let (lane, from) = picked(app, t)?;
    Ok(json!({"set": own_lane(t), "lane": lane, "from": from}))
}

/// What a body's `lane` and `alone` come to for a task: (lane, alone). `--alone` puts the task on the
/// alone lane; a lane other than alone takes it off. Unchanged parts are the task's (`was`) own.
pub fn with_alone(body: &Value, was: Option<&Row>) -> Result<(Option<String>, Option<String>)> {
    let lane = clean(body.get("lane"))?;
    let had_alone = was.and_then(|t| t.s("alone")).map(str::to_string);
    let alone = if body.get("alone").is_some() { crate::locks::clean_alone(body.get("alone"))? } else { had_alone.clone() };
    let was_lane = was.map(own_lane).unwrap_or("auto");
    match lane.as_deref() {
        Some("alone") => Ok((Some("alone".into()), Some(alone.unwrap_or_else(|| "goal".into())))),
        Some(l) if body.get("alone").is_some() && alone.is_some() => {
            err(400, format!("--lane {l} and --alone don't go together: a task on the alone lane runs alone. Give one."))
        }
        Some(l) => Ok((Some(l.to_string()), None)),
        None if alone.is_some() => Ok((Some("alone".into()), alone)),
        None if was_lane == "alone" && body.get("alone").is_some() => Ok((Some("auto".into()), None)),
        None => Ok((was.and_then(|t| t.s("lane")).map(str::to_string), alone)),
    }
}

/// The plan check's warnings: each wave that runs tasks side by side where some plan no files, so the
/// board can't tell whether they'd touch the same ones. Nothing when the goal's lane is set, or for
/// tasks given a lane of their own.
pub fn plan_warnings(app: &App, g: &Row) -> Result<Vec<String>> {
    if goal_lane(g) != "auto" || g.i("max_terminals").unwrap_or(2) <= 1 {
        return Ok(vec![]);
    }
    let mut waves: std::collections::BTreeMap<i64, Vec<Row>> = Default::default();
    for t in board::goal_tasks(app, g.id())? {
        if let (Some(n), false) = (t.i("wave"), t.s("status") == Some("done")) {
            waves.entry(n).or_default().push(t);
        }
    }
    let mut out = vec![];
    for (n, tasks) in waves.into_iter().filter(|(_, ts)| ts.len() > 1) {
        let bare: Vec<String> =
            tasks.iter().filter(|t| own_lane(t) == "auto" && plan_files(t).is_empty()).map(|t| rf("task", t.id())).collect();
        if !bare.is_empty() {
            out.push(format!(
                "Wave {n} runs tasks side by side, and {} plan{} no files: each takes a worktree slot. Name each task's files (\"title::detail::{n}::::src/a.rs\"), or give it a lane (tb task set T<n> --lane shared).",
                bare.join(", "),
                if bare.len() == 1 { "s" } else { "" }
            ));
        }
    }
    Ok(out)
}
