//! Shared tasks: one task's work can finish more than one goal. Its home goal (`tasks.goal_id`) runs it;
//! other goals point to it (`task_goals`, `tb task set T<n> --also G<n>`) and count it toward their own.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, p};

/// The other goals this task also finishes, oldest link first.
pub fn goal_ids(app: &App, task_id: i64) -> Result<Vec<i64>> {
    Ok(app
        .db
        .q("SELECT goal_id FROM task_goals WHERE task_id = ? ORDER BY at, goal_id", p![task_id])?
        .iter()
        .filter_map(|r| r.i("goal_id"))
        .collect())
}

/// `[{id, ref, name}]` for a card's `also`.
pub fn goals_of(app: &App, task_id: i64) -> Result<Vec<Value>> {
    let mut out = vec![];
    for gid in goal_ids(app, task_id)? {
        if let Some(g) = board::find_goal(app, Some(gid))? {
            out.push(json!({"id": g.id(), "ref": rf("goal", g.id()), "name": g.v("name")}));
        }
    }
    Ok(out)
}

/// Tasks from other goals that also finish this one.
pub fn tasks_for(app: &App, goal_id: i64) -> Result<Vec<Row>> {
    app.db.q(
        "SELECT t.* FROM task_goals r JOIN tasks t ON t.id = r.task_id \
         WHERE r.goal_id = ? AND t.goal_id IS NOT ? ORDER BY r.at, t.id",
        p![goal_id, goal_id],
    )
}

/// Every task that counts toward the goal: its own, then the shared ones.
pub fn counted_tasks(app: &App, goal_id: i64) -> Result<Vec<Row>> {
    let mut rows = board::goal_tasks(app, goal_id)?;
    rows.extend(tasks_for(app, goal_id)?);
    Ok(rows)
}

/// Goal ids from `--also` (a list or "G3, G4"), each checked to be in the task's project.
pub fn clean(app: &App, value: Option<&Value>, t: Option<&Row>) -> Result<Vec<i64>> {
    let items: Vec<Value> = match value {
        None | Some(Value::Null) => return Ok(vec![]),
        Some(Value::Array(a)) => a.clone(),
        Some(Value::String(s)) if s.trim().is_empty() || s.trim() == "none" => return Ok(vec![]),
        Some(Value::String(s)) => s.replace(',', " ").split_whitespace().map(|x| json!(x)).collect(),
        Some(v) => vec![v.clone()],
    };
    let mut out = vec![];
    for v in &items {
        let Some(gid) = parse_ref(v, "goal")? else { continue };
        let g = board::get_goal(app, gid)?;
        if let Some(t) = t {
            if g.s("project") != t.s("project") {
                return err(
                    409,
                    format!(
                        "{} is in {}, not {}, so this task's work can't finish it.",
                        rf("goal", gid),
                        g.st("project"),
                        t.st("project")
                    ),
                );
            }
        }
        if !out.contains(&gid) {
            out.push(gid);
        }
    }
    Ok(out)
}

/// Links the task to more goals; returns the ones that are new.
pub fn add(app: &App, t: &Row, gids: &[i64], who: &str) -> Result<Vec<i64>> {
    if gids.is_empty() {
        return Ok(vec![]);
    }
    let Some(home) = t.i("goal_id") else {
        let tr = rf("task", t.id());
        return err(409, format!("{tr} has no goal of its own yet. Give it its home goal first (tb task set {tr} --goal G<n>), then --also the others."));
    };
    let have = goal_ids(app, t.id())?;
    let added: Vec<i64> = gids.iter().copied().filter(|g| *g != home && !have.contains(g)).collect();
    let now = now_iso();
    for gid in &added {
        app.db.x("INSERT INTO task_goals(task_id, goal_id, at) VALUES (?, ?, ?)", p![t.id(), *gid, now.clone()])?;
    }
    if !added.is_empty() {
        board::log_event(app, t.id(), who, "note", &format!("Also finishes {}", names(app, &added)?))?;
    }
    Ok(added)
}

/// Unlinks the task from goals; returns the ones it was linked to.
pub fn drop(app: &App, t: &Row, gids: &[i64], who: &str) -> Result<Vec<i64>> {
    let have = goal_ids(app, t.id())?;
    let gone: Vec<i64> = gids.iter().copied().filter(|g| have.contains(g)).collect();
    for gid in &gone {
        app.db.x("DELETE FROM task_goals WHERE task_id = ? AND goal_id = ?", p![t.id(), *gid])?;
    }
    if !gone.is_empty() {
        board::log_event(app, t.id(), who, "note", &format!("No longer for {}", names(app, &gone)?))?;
    }
    Ok(gone)
}

/// A task that moves into a goal it also finished now runs there instead.
pub fn moved_home(app: &App, task_id: i64, gid: Option<i64>) -> Result<()> {
    if let Some(g) = gid {
        app.db.x("DELETE FROM task_goals WHERE task_id = ? AND goal_id = ?", p![task_id, g])?;
    }
    Ok(())
}

/// Where a task goes when its home goal is gone: the newest other goal it finishes.
pub fn next_home(app: &App, task_id: i64, gone_goal: Option<i64>) -> Result<Option<i64>> {
    Ok(app
        .db
        .q1(
            "SELECT goal_id FROM task_goals WHERE task_id = ? AND goal_id IS NOT ? ORDER BY at DESC, goal_id DESC LIMIT 1",
            p![task_id, gone_goal],
        )?
        .and_then(|r| r.i("goal_id")))
}

pub fn forget_goal(app: &App, gid: i64) -> Result<()> {
    app.db.x("DELETE FROM task_goals WHERE goal_id = ?", p![gid])?;
    Ok(())
}

pub fn forget_tasks(app: &App, ids: &[i64]) -> Result<()> {
    for id in ids {
        app.db.x("DELETE FROM task_goals WHERE task_id = ?", p![*id])?;
    }
    Ok(())
}

/// "G3 “Name”, G4 “Other”".
pub fn names(app: &App, gids: &[i64]) -> Result<String> {
    let mut out = vec![];
    for gid in gids {
        out.push(match board::find_goal(app, Some(*gid))? {
            Some(g) => format!("{} “{}”", rf("goal", *gid), g.st("name")),
            None => rf("goal", *gid),
        });
    }
    Ok(out.join(", "))
}

/// The plan prompt's and goal context's list of the shared tasks.
pub fn lines(app: &App, goal_id: i64, indent: &str) -> Result<Vec<String>> {
    let rows = tasks_for(app, goal_id)?;
    if rows.is_empty() {
        return Ok(vec![]);
    }
    let mut out = vec![String::new(), "Tasks from other goals that also finish this one (their home goal runs them):".to_string()];
    for t in rows {
        let st = crate::api::status_label(&t);
        let home = t.i("goal_id").map(|g| rf("goal", g)).unwrap_or_default();
        out.push(format!("{indent}- {} [{st}] {} (home {home})", rf("task", t.id()), t.st("title")));
    }
    Ok(out)
}
