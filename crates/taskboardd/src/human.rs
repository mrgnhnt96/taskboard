//! Human tasks (`tb task new --human`, shown as H<n>): work only the owner can do, such as making an
//! account or signing an agreement. Its detail holds the steps and links. No terminal ever starts one:
//! once nothing holds it back, the runner puts it in Needs you and raises an alert, and it's done when
//! the owner presses Done in the app (with an optional note). Tasks that wait for it, by `waits_for` or
//! by their goal's order, then go on, and a parked one is told what the owner did.

use serde_json::{json, Value};

use crate::app::App;
use crate::board::{self, OWNER};
use crate::util::*;
use crate::{dispatch, fields, reports};

pub const KIND: &str = "human";

/// The `needs_reason` of a human task that's ready for the owner.
pub const NEEDS: &str = "human";

pub fn is(t: &Row) -> bool {
    t.s("kind") == Some(KIND)
}

/// The task's ref as people see it: H<n> for a human task, else T<n>. T<n> still finds either.
pub fn task_ref(t: &Row) -> String {
    if is(t) {
        format!("H{}", t.id())
    } else {
        rf("task", t.id())
    }
}

/// `kind` from a new task's body: `human`, or none for an agent's task.
pub fn clean_kind(body: &Value) -> Result<Option<&'static str>> {
    match body.get("kind") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(b.then_some(KIND)),
        Some(Value::String(s)) if s.trim().is_empty() || s.trim() == "agent" => Ok(None),
        Some(Value::String(s)) if s.trim() == KIND => Ok(Some(KIND)),
        _ => err(400, "A task's kind is human, or none for an agent's task."),
    }
}

/// What a new human task must have: the steps to follow, since nobody else can do them.
pub fn refuse_bare(body: &Value) -> Result<()> {
    if body_str(body, "detail").trim().is_empty() {
        return err(400, "A human task needs its steps in --detail: what to do, in order, with the links to open.");
    }
    for k in ["stack_on", "locks", "alone"] {
        if body_has(body, k) && !body[k].is_null() {
            return err(400, format!("A human task has no terminal, so it takes no {}.", k.replace('_', "-")));
        }
    }
    Ok(())
}

/// The refusal for anything that would start a terminal on a human task.
pub fn refuse_terminal(app: &App, t: &Row) -> Result<()> {
    if is(t) {
        return err(409, format!("{} is a human task: {} does it and presses Done in the app; no terminal starts on it.", task_ref(t), app.cfg.owner));
    }
    Ok(())
}

/// The refusal for an agent's `tb submit` or `tb fail` (or the API's done and fail) on a human task.
pub fn refuse_agent_finish(app: &App, t: &Row) -> Result<()> {
    if is(t) {
        return err(409, format!("{} is a human task: only {} marks it done, with Done in the app.", task_ref(t), app.cfg.owner));
    }
    Ok(())
}

/// The runner's turn on a queued human task nothing holds back: it waits for the owner now.
pub fn ready(app: &App, t: &Row) -> Result<()> {
    let r = task_ref(t);
    let ask = format!("Your task: {}. Follow its steps, then press Done.", t.st("title"));
    board::update_task(app, t.id(), fields!["status" => "needs", "needs_reason" => NEEDS, "question" => ask.clone(), "answered_at" => null, "latest" => ask])?;
    board::log_event(app, t.id(), "Board", "status", "Ready for you")?;
    dispatch::add_alert_keyed(app, &format!("{r}: {}", t.st("title")), Some(t.id()), t.i("goal_id"), &alert_key(t.id()))
}

fn alert_key(id: i64) -> String {
    format!("human-{id}")
}

/// The owner pressed Done (only from the app): the task is finished with their note, if any.
pub fn done(app: &App, id: i64, body: &Value) -> Result<()> {
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        if !is(&t) {
            return err(409, format!("{} isn't a human task.", rf("task", id)));
        }
        if t.s("status") == Some("done") {
            return err(409, "It's already done.");
        }
        let note = clip(body_str(body, "note").trim(), 2000);
        let mut ctx = board::task_context(&t);
        if !note.is_empty() {
            ctx.insert("owner_note".into(), json!(note));
            board::save_context(app, id, &ctx, false)?;
        }
        let summary = if note.is_empty() { "Done by you".to_string() } else { format!("Done by you: {note}") };
        reports::finish_task(app, &t, OWNER, &summary, false, None)?;
        dispatch::clear_alert_key(app, &alert_key(id))?;
        dispatch::clear_alerts(app, Some(id), None)
    })
}

/// What the owner said when they pressed Done.
pub fn note(t: &Row) -> Option<String> {
    board::task_context(t).get("owner_note").and_then(|v| v.as_str()).map(|s| s.to_string()).filter(|s| !s.is_empty())
}

/// The resume prompt's line for a human task this one waited for.
pub fn done_line(app: &App, t: &Row) -> String {
    let mut line = format!("- {} “{}” is done: {} did it", task_ref(t), short(&t.st("title"), 80), app.cfg.owner);
    match note(t) {
        Some(n) => line += &format!(" and noted: {n}"),
        None => line += ".",
    }
    line
}

/// The handoff's lines for the human tasks the owner finished in this task's goal, besides the ones
/// it waits for (`waitsfor::bring_in_text` names those).
pub fn goal_text(app: &App, t: &Row) -> Result<Option<String>> {
    let Some(gid) = t.i("goal_id") else { return Ok(None) };
    let deps = crate::waitsfor::deps(t);
    let lines: Vec<String> = board::goal_tasks(app, gid)?
        .iter()
        .filter(|o| is(o) && o.s("status") == Some("done") && !o.b("failed") && !deps.contains(&o.id()))
        .map(|o| done_line(app, o))
        .collect();
    if lines.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("Done by {} in this goal, which this task may build on:\n{}", app.cfg.owner, lines.join("\n"))))
}
