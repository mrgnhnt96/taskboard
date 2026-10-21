//! Design tasks (`tb task new --kind design`): a task whose work is a design (concepts, a canvas,
//! mockups) rather than code. It stays a task (T<n>) with kind `design`. It ends without a PR, but
//! `tb submit` needs its design attached (`tb attach <link> --kind design`). In a goal it runs first:
//! a build task added to the goal waits for the goal's open design tasks unless it says otherwise.

use serde_json::{json, Value};

use crate::app::App;
use crate::board;
use crate::util::*;
use crate::{fields, human};

pub const KIND: &str = "design";

pub fn is(t: &Row) -> bool {
    t.s("kind") == Some(KIND)
}

/// `kind` from a body: `human`, `design`, or none for an agent's task.
pub fn clean_kind(v: Option<&Value>) -> Result<Option<&'static str>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(b.then_some(human::KIND)),
        Some(Value::String(s)) => match s.trim().to_lowercase().as_str() {
            "" | "agent" | "none" => Ok(None),
            k if k == human::KIND => Ok(Some(human::KIND)),
            k if k == KIND => Ok(Some(KIND)),
            _ => err(400, "A task's kind is design, human, or none for an agent's task."),
        },
        _ => err(400, "A task's kind is design, human, or none for an agent's task."),
    }
}

/// Sets a task's kind from `tb task set --kind design|none`, logging the change. False when it didn't change.
pub fn set(app: &App, t: &Row, v: &Value, who: &str) -> Result<bool> {
    let want = clean_kind(Some(v))?;
    if human::is(t) || want == Some(human::KIND) {
        return err(400, "A human task is made with tb task new --human and stays one; --kind is design or none.");
    }
    if want == t.s("kind") {
        return Ok(false);
    }
    // A design ends without a PR; back to none, it ends in one as its project does.
    let ships_pr = if want == Some(KIND) { Some(0) } else { None };
    board::update_task(app, t.id(), fields!["kind" => want, "ships_pr" => ships_pr])?;
    board::log_event(app, t.id(), who, "note", if want == Some(KIND) { "Made a design task: it ends with its design attached, not a PR" } else { "No longer a design task" })?;
    Ok(true)
}

fn design_attachment(app: &App, t: &Row) -> Result<Option<Value>> {
    Ok(board::attachments(app, Some(t.id()), None)?.into_iter().rev().find(|a| a["kind"] == "design"))
}

/// The refusal for `tb submit` on a design task with no design attached.
pub fn refuse_bare_submit(app: &App, t: &Row) -> Result<()> {
    if !is(t) || design_attachment(app, t)?.is_some() {
        return Ok(());
    }
    err(
        409,
        format!(
            "{} is a design task: attach its design (the canvas or mockup link) before you submit: \
             tb attach <link> --kind design --title \"<what it shows>\"",
            rf("task", t.id())
        ),
    )
}

/// The line `tb submit` logs on a design task: the design it ends with.
pub fn done_text(app: &App, t: &Row) -> Result<Option<String>> {
    if !is(t) {
        return Ok(None);
    }
    Ok(design_attachment(app, t)?.map(|a| {
        let title = a["title"].as_str().filter(|s| !s.is_empty()).unwrap_or("Design");
        format!("Design: {title} {}", a["url"].as_str().unwrap_or("")).trim().to_string()
    }))
}

/// The open design tasks in a goal, which its other tasks wait for.
pub fn open_in_goal(app: &App, goal_id: i64, except: i64) -> Result<Vec<i64>> {
    Ok(board::goal_tasks(app, goal_id)?.iter().filter(|o| is(o) && o.id() != except && o.s("status") != Some("done")).map(|o| o.id()).collect())
}

/// What a build task joining a goal waits for: what it waited for already, plus the goal's open design tasks.
pub fn waits_with_goal(app: &App, t: &Row, goal_id: i64) -> Result<Option<String>> {
    if is(t) || human::is(t) {
        return Ok(t.s("waits_for").map(|s| s.to_string()));
    }
    let mut ids = crate::waitsfor::ids(t);
    for d in open_in_goal(app, goal_id, t.id())? {
        if !ids.contains(&d) && !crate::waitsfor::reaches(app, d, t.id(), &mut vec![])? {
            ids.push(d);
        }
    }
    Ok(if ids.is_empty() { None } else { Some(jdumps(&json!(ids))) })
}
