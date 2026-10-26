//! `[jira] merged_note`: the agent's note for testers. A task that ships a PR on a ticket finishes with one
//! (`tb submit --merged-note`), and once its PR merges the board posts it on the ticket, then the "PR #n was
//! merged" line, and only then moves the ticket to `[jira] merged`. A merge with no note leaves the ticket
//! where it is, raises an alert and brings the conversation back to write one. A post or move that fails
//! raises an alert and is tried again every `RETRY_SECS` until the ticket has moved.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, dispatch, fields, p, projects};

/// How long after a failed post or move the board tries again.
const RETRY_SECS: f64 = 600.0;
/// How long a note may be.
pub const MAX: usize = 20_000;

pub fn on(app: &App) -> bool {
    app.cfg.jira_on() && app.cfg.jira.merged_note
}

/// Whether this task's merge needs a note: the setting's on and its PR's ticket is kept in step.
pub fn needed(app: &App, t: &Row) -> Result<bool> {
    Ok(on(app) && has(t.s("jira_key")) && t.b("jira_sync") && projects::task_ships_pr(app, t)?)
}

pub fn has_note(t: &Row) -> bool {
    t.s("merged_note").is_some_and(|n| !n.trim().is_empty())
}

/// What `tb submit` says to a task that needs a note and has none.
pub fn refusal(app: &App, t: &Row) -> String {
    let tb = board::tb_cmd(app);
    let me = rf("task", t.id());
    format!(
        "{me}'s ticket {} gets a note for testers before it moves to {} at merge. Read what it covers in {tb} jira, then finish with \
         {tb} submit \"<summary>\" --merged-note \"<markdown>\" (or set it first with {tb} task set {me} --merged-note …). \
         Work that isn't testable still gets a note that says so.",
        t.st("jira_key"),
        merged_status(app)
    )
}

/// The handoff's line about the note, when this task needs one.
pub fn handoff_line(app: &App, t: &Row) -> Result<Option<String>> {
    if !on(app) || !projects::task_ships_pr(app, t)? || !t.b("jira_sync") {
        return Ok(None);
    }
    let tb = board::tb_cmd(app);
    Ok(Some(format!(
        "Jira: once the PR merges, the board posts your note for testers on the ticket before it moves to {}. Write it to the guide in \
         {tb} jira and finish with {tb} submit \"<summary>\" --merged-note \"<markdown>\"; tb submit refuses a ticketed PR task without one.",
        merged_status(app)
    )))
}

fn merged_status(app: &App) -> String {
    match app.cfg.jira.merged.trim() {
        "" => "its merged status".to_string(),
        s => s.to_string(),
    }
}

fn alert_key(t: &Row) -> String {
    format!("merged-note:{}", t.id())
}

/// At merge: posts the note and moves the ticket, or holds the ticket and asks for the note.
/// `comment` is the "PR #n was merged" line. A head merge (`stack.rs`) lands in its parent's branch, not
/// the main branch, so its ticket waits for the stack's bottom PR; when that merges, every head-merged
/// task in its stack goes with it.
pub fn ticket_merged(app: &App, t: &Row, comment: &str) -> Result<()> {
    if !crate::waitsfor::stack_landed(app, t)? {
        let into = crate::stack::parent_id(t).map(|n| rf("task", n)).unwrap_or_default();
        let text = format!("PR #{} merged into {into}'s branch; the ticket moves once its stack reaches the main branch", t.i0("pr_num"));
        board::log_event(app, t.id(), board::BOARD, "jira", &text)?;
        return Ok(());
    }
    landed(app, t, comment)?;
    for k in crate::waitsfor::head_merged_children(app, t.id())? {
        board::log_event(app, k.id(), board::BOARD, "jira", &format!("Its stack reached the main branch with {}'s PR", rf("task", t.id())))?;
        stack_landed(app, &k)?;
    }
    Ok(())
}

/// A head-merged task whose stack just reached the main branch, and the head-merged tasks above it.
fn stack_landed(app: &App, t: &Row) -> Result<()> {
    landed(app, t, &crate::prflow::merged_comment(t))?;
    for k in crate::waitsfor::head_merged_children(app, t.id())? {
        stack_landed(app, &k)?;
    }
    Ok(())
}

fn landed(app: &App, t: &Row, comment: &str) -> Result<()> {
    if !on(app) || !has(t.s("jira_key")) || !t.b("jira_sync") {
        let st = app.cfg.jira.merged.clone();
        board::jira_keep_in_step(app, t, Some(&st), Some(comment))?;
        return Ok(());
    }
    board::update_task(app, t.id(), fields!["merged_note_wait" => now_iso(), "merged_note_posted" => null])?;
    let t = board::get_task(app, t.id())?;
    if has_note(&t) {
        return post(app, &t);
    }
    missing(app, &t)
}

/// A merge with no note: the ticket stays, the owner hears of it, and the conversation comes back for it.
fn missing(app: &App, t: &Row) -> Result<()> {
    let me = rf("task", t.id());
    let tb = board::tb_cmd(app);
    let text = format!(
        "{me}'s PR #{} merged with no note for testers, so {} stays where it is. Write it with {tb} task set {me} --merged-note \"<markdown>\"; the board then posts it and moves the ticket.",
        t.i0("pr_num"),
        t.st("jira_key")
    );
    dispatch::add_alert_keyed(app, &text, Some(t.id()), t.i("goal_id"), &alert_key(t))?;
    board::log_event(app, t.id(), board::BOARD, "jira", &format!("Kept {} where it is: the task has no note for testers", t.st("jira_key")))?;
    let ask = format!(
        "PR #{} merged, and its ticket {} waits for your note for testers before it moves. Read what the note covers in {tb} jira, \
         then run {tb} task set {me} --merged-note \"<markdown>\". Don't change code, take another task or run tb submit: the task is already submitted.",
        t.i0("pr_num"),
        t.st("jira_key")
    );
    crate::prflow::ask_for_note(app, t, &ask)?;
    Ok(())
}

fn post(app: &App, t: &Row) -> Result<()> {
    let comment = crate::prflow::merged_comment(t);
    crate::jira::request_job(
        app,
        json!({"op": "merged_note", "key": t.st("jira_key"), "status": app.cfg.jira.merged, "note": t.st("merged_note"),
               "comment": comment, "by": "board"}),
        json!({"task": t.id()}),
    )?;
    Ok(())
}

/// The note was written or changed: a ticket waiting for it goes now.
pub fn note_set(app: &App, t: &Row) -> Result<()> {
    if has(t.s("merged_note_wait")) && has_note(t) && live_job(app, t)?.is_none() {
        dispatch::clear_alert_key(app, &alert_key(t))?;
        post(app, t)?;
    }
    Ok(())
}

fn live_job(app: &App, t: &Row) -> Result<Option<Row>> {
    app.db.q1(
        "SELECT * FROM jobs WHERE kind = 'jira' AND task_id IS ? AND json_extract(args, '$.op') = 'merged_note' AND state IN ('pending', 'running') LIMIT 1",
        p![t.id()],
    )
}

fn last_failed_at(app: &App, t: &Row) -> Result<Option<f64>> {
    let j = app.db.q1(
        "SELECT state, updated_at FROM jobs WHERE kind = 'jira' AND task_id IS ? AND json_extract(args, '$.op') = 'merged_note' ORDER BY id DESC LIMIT 1",
        p![t.id()],
    )?;
    Ok(j.filter(|j| matches!(j.s("state"), Some("failed") | Some("expired"))).and_then(|j| j.s("updated_at").and_then(parse_iso)))
}

/// The runner's sweep: a ticket still waiting with its note written, whose last try failed a while ago, goes again.
pub fn tick(app: &App) -> Result<()> {
    if !on(app) {
        return Ok(());
    }
    for t in app.db.q("SELECT * FROM tasks WHERE merged_note_wait IS NOT NULL", p![])? {
        if !has_note(&t) || live_job(app, &t)?.is_some() {
            continue;
        }
        match last_failed_at(app, &t)? {
            Some(at) if now_ts() - at < RETRY_SECS => {}
            Some(_) => post(app, &t)?,
            None => {}
        }
    }
    Ok(())
}

/// How far a job's posting has got, so a retry doesn't post the same comment twice.
pub fn posted(app: &App, task_id: i64) -> String {
    app.db.val("SELECT merged_note_posted FROM tasks WHERE id = ?", p![task_id]).ok().and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default()
}

pub fn set_posted(app: &App, task_id: i64, how_far: &str) {
    if let Err(e) = app.db.x("UPDATE tasks SET merged_note_posted = ? WHERE id = ?", p![how_far, task_id]) {
        app.info(format!("merged note {}: recording what was posted failed: {e}", rf("task", task_id)));
    }
}

/// The job finished: the ticket moved, or the owner hears why it didn't.
pub fn finished(app: &App, t: &Row, ok: bool, why: &str) -> Result<()> {
    if ok {
        board::update_task(app, t.id(), fields!["merged_note_wait" => null])?;
        return dispatch::clear_alert_key(app, &alert_key(t));
    }
    let me = rf("task", t.id());
    let text = format!(
        "Couldn't post {me}'s note for testers and move {}: {}. The board tries again in {} minutes.",
        t.st("jira_key"),
        why.trim_end_matches('.'),
        (RETRY_SECS / 60.0) as i64
    );
    dispatch::clear_alert_key(app, &alert_key(t))?;
    dispatch::add_alert_keyed(app, &text, Some(t.id()), t.i("goal_id"), &alert_key(t))
}

/// The task page's view of the note.
pub fn card(t: &Row) -> Value {
    if !has_note(t) && !has(t.s("merged_note_wait")) {
        return Value::Null;
    }
    json!({"text": t.v("merged_note"), "waiting": has(t.s("merged_note_wait")), "posted": t.v("merged_note_posted")})
}
