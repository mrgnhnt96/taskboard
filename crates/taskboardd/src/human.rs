//! Human tasks (`tb task new --human`, shown as H<n>): work only the owner can do, such as making an
//! account or signing an agreement. Its detail holds the steps and links. No terminal ever starts one:
//! once nothing holds it back, the runner puts it in Needs you and raises an alert, and it's done when
//! the owner presses Done in the app (with an optional note). Tasks that wait for it, by `waits_for` or
//! by their goal's order, then go on, and a parked one is told what the owner did.

use serde_json::{json, Value};

use crate::app::App;
use crate::board::{self, OWNER};
use crate::util::*;
use crate::{dig, dispatch, fields, midna, reports, waitsfor};

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

/// `POST /tasks/:id/talk`: a Claude terminal to ask about a human task in, opened in its project's
/// checkout (the home folder when it has none) with the task's steps in its context. Claude helps the
/// owner through them; it never does them or marks the task done. Refused for a task that isn't a
/// human one, one that's done, or while Midna is down.
pub fn talk(app: &App, id: i64) -> Result<Value> {
    let t = board::get_task(app, id)?;
    let r = task_ref(&t);
    if !is(&t) {
        return err(409, format!("{r} isn't a human task."));
    }
    if t.s("status") == Some("done") {
        return err(409, format!("{r} is done already."));
    }
    if !midna::up(app) {
        return err(409, "Midna isn't running, so the board can't open a terminal.");
    }
    let cwd = match dig::checkout(app, &t.st("project"))? {
        Some(p) => p,
        None => dirs::home_dir().unwrap_or_default(),
    };
    let args = json!({"cwd": cwd, "title": short(&format!("Ask: {}", t.st("title")), 40), "prompt": "", "draft": "",
                      "context": talk_context(app, &t)?, "focus": true, "queue": false});
    let j = app.db.tx(|| board::create_job(app, "agent", args, None, "human_talk", Some(json!({"task": id}))))?;
    Ok(json!({"ok": true, "job": rf("job", j)}))
}

fn talk_context(app: &App, t: &Row) -> Result<String> {
    let owner = &app.cfg.owner;
    let r = task_ref(t);
    let mut parts = vec![format!(
        "{owner} opened this terminal from the task board to ask about {r}, a human task: something only they can do, \
         following its steps. Help them with it: explain a step, find what a step needs (in the code, the board or on \
         the web), or draft text they'll paste. Don't do the steps for them and don't change code. Wait for {owner}; \
         don't start on anything by yourself. You aren't on a task: don't take one. You can't mark {r} done; {owner} \
         presses Done in the app when it is."
    )];
    let mut task = format!("{r}: {}", t.st("title"));
    if !t.st("project").is_empty() {
        task.push_str(&format!(" (project {})", t.st("project")));
    }
    task.push_str(&format!("\nIts steps:\n{}", t.st("detail").trim()));
    parts.push(task);
    let waiting: Vec<String> =
        waitsfor::waiting_on(app, t)?.iter().map(|o| format!("- {} {}", task_ref(o), short(&o.st("title"), 80))).collect();
    if !waiting.is_empty() {
        parts.push(format!("Waiting for it to be done:\n{}", waiting.join("\n")));
    }
    parts.push(format!("For all of it as it stands now: tb task show {r}"));
    Ok(parts.join("\n\n"))
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
