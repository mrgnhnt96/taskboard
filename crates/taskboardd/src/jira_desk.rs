//! The Jira desk (`[jira] desk = true`): one Claude terminal in Midna's Background group that finds
//! or makes the board's tickets. The board sends it one job at a time ("[task-board:J12] …"); it
//! searches Jira for an open ticket that already covers the work, makes one only when none does, and
//! reports back with `tb jira J12 ok key=PROJ-1 status="To Do"` (or `fail "<why>"`). The board never
//! closes it; when it's gone, the next job opens a new one.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, jira, p};

/// The desk's agent and message jobs.
pub const PURPOSE: &str = "jira_desk";
/// A job the desk took and hasn't reported on fails after this long.
pub const EXPIRY_SECS: f64 = 1800.0;
/// What the desk's terminal is for, wherever the board shows it.
pub const ROLE: &str = "Handles Jira for the board";

pub fn on(app: &App) -> bool {
    app.cfg.jira_on() && app.cfg.jira.desk
}

/// The desk's terminal, while it's open.
pub fn session(app: &App) -> Result<Option<String>> {
    for j in app.db.q(
        "SELECT target FROM jobs WHERE kind = 'agent' AND purpose = ? AND state = 'done' ORDER BY id DESC LIMIT 5",
        p![PURPOSE],
    )? {
        let Some(sid) = jloads_obj(j.s("target")).s("session").map(|s| s.to_string()) else { continue };
        if let Some(s) = board::get_session(app, Some(&sid))? {
            if s.s("status") != Some("gone") {
                return Ok(Some(sid));
            }
        }
    }
    Ok(None)
}

/// This terminal is the desk's (it ever was: an old desk is still the desk).
pub fn is_desk(app: &App, sid: Option<&str>) -> Result<bool> {
    let Some(sid) = sid.filter(|s| !s.is_empty()) else { return Ok(false) };
    Ok(!app
        .db
        .val("SELECT 1 FROM jobs WHERE kind = 'agent' AND purpose = ? AND json_extract(target, '$.session') = ? LIMIT 1", p![PURPOSE, sid])?
        .is_null())
}

fn opening(app: &App) -> Result<bool> {
    Ok(app.db.q1("SELECT id FROM jobs WHERE kind = 'agent' AND purpose = ? AND state IN ('pending', 'running')", p![PURPOSE])?.is_some())
}

/// What the desk is told when it opens (and again at each session start).
pub fn intro(app: &App) -> String {
    let tb = board::tb_cmd(app);
    [
        "You are the task board's Jira desk: you handle Jira for the board, one job at a time, with the Atlassian connector's tools.".to_string(),
        "The board sends each job here as a message starting with [task-board:J<n>]. For each one:".into(),
        "1. Search Jira for an open ticket that already covers the work (its summary, its description, the goal's epic). If one does, use it; never make a duplicate.".into(),
        "2. Only if none does, make the ticket with exactly the fields the job gives.".into(),
        format!(
            "3. Report right away: {tb} jira J<n> ok key=PROJ-123 status=\"To Do\" (add found=yes when it was already there, and \
             product=<name> when the job asked you to pick one), or {tb} jira J<n> fail \"<why>\" when it can't be done."
        ),
        format!(
            "Never ask {} anything, never change code, and don't take board tasks. Keep this terminal open: the next job comes here. \
             Wait for the first one now.",
            app.cfg.owner
        ),
    ]
    .join("\n")
}

/// One job's message to the desk.
pub fn job_text(app: &App, j: &Row) -> Result<String> {
    let a = board::job_args(j);
    let target = board::job_target(j);
    let g = jira::job_goal(app, &target)?;
    let n = rf("job", j.id());
    let tb = board::tb_cmd(app);
    let mut what = jira::ticket_brief(app, &a, g.as_ref());
    if let Some(t) = board::find_task(app, target.i("task"))? {
        what = format!("For the board task {} “{}”.\n{what}", rf("task", t.id()), t.st("title"));
    }
    Ok(format!(
        "[task-board:{n}] Jira desk job {n}.\n{what}\n\nWhen it's done: {tb} jira {n} ok key=<KEY> status=\"<status>\" (found=yes if it was \
         already there{}), or {tb} jira {n} fail \"<why>\".",
        if g.as_ref().is_some_and(|g| has(g.s("product"))) || app.cfg.jira.products.is_empty() { "" } else { ", product=<name> for the product you picked" }
    ))
}

fn open(app: &App) -> Result<i64> {
    let dir = if app.cfg.jira.desk_dir.trim().is_empty() { app.cfg.data.clone() } else { expand_home(app.cfg.jira.desk_dir.trim()) };
    let mut tools = app.cfg.jira.claude_tools.clone();
    tools.push("Bash(tb jira:*)".into());
    let flags = format!("--allowedTools '{}'", tools.join(","));
    let id = board::create_job(
        app,
        "agent",
        json!({"cwd": dir.to_string_lossy(), "title": "Jira desk", "prompt": intro(app), "flags": flags, "background": true}),
        None,
        PURPOSE,
        None,
    )?;
    app.info(format!("jira desk: opening its terminal ({})", rf("job", id)));
    Ok(id)
}

/// Sends the desk its next job, opening its terminal first when it isn't open. One job at a time,
/// oldest first, so a goal's epic is there before its tickets.
pub fn tick(app: &App) -> Result<()> {
    if !on(app) {
        return Ok(());
    }
    let busy = app.db.q1("SELECT id FROM jobs WHERE kind = 'jira' AND state = 'running' AND json_extract(target, '$.desk') IS NOT NULL", p![])?;
    if busy.is_some() {
        return Ok(());
    }
    let pending = app.db.q("SELECT * FROM jobs WHERE kind = 'jira' AND state = 'pending' ORDER BY id", p![])?;
    let Some(j) = pending.into_iter().find(|j| jira::for_desk(app, j)) else { return Ok(()) };
    let Some(sid) = session(app)? else {
        if !opening(app)? {
            app.db.tx(|| open(app))?;
        }
        return Ok(());
    };
    app.db.tx(|| {
        let text = job_text(app, &j)?;
        board::create_job(app, "message", json!({"to": sid, "text": text}), j.i("task_id"), PURPOSE, None)?;
        let mut target = board::job_target(&j);
        target.insert("desk".into(), json!(sid));
        app.db.x(
            "UPDATE jobs SET state = 'running', attempts = attempts + 1, target = ?, updated_at = ? WHERE id = ?",
            p![jdumps(&Value::Object(target)), now_iso(), j.id()],
        )?;
        Ok(())
    })?;
    app.info(format!("jira desk: sent it {}", rf("job", j.id())));
    Ok(())
}

/// `GET /jira`: how Jira is set up, the desk, and the latest jobs.
pub fn state(app: &App) -> Result<Value> {
    let j = &app.cfg.jira;
    let jobs: Vec<Value> = app
        .db
        .q("SELECT * FROM jobs WHERE kind = 'jira' ORDER BY state IN ('pending', 'running') DESC, id DESC LIMIT 20", p![])?
        .iter()
        .map(board::job_dict)
        .collect();
    Ok(json!({
        "on": app.cfg.jira_on(), "site": j.site, "project": j.project,
        "via": if app.cfg.jira_via_claude() { "claude" } else { "rest" },
        "auto_ticket": j.auto_ticket, "desk": on(app), "desk_session": session(app)?,
        "products": j.products.iter().map(|(k, p)| json!({"name": k, "what": p.what})).collect::<Vec<_>>(),
        "jobs": jobs,
    }))
}

/// `POST /jira/jobs/:id {ok, key, status, found, product, message}`: the desk's report on a job
/// (`tb jira J12 ok key=PROJ-1 status="To Do"`).
pub fn report(app: &App, job_id: i64, body: &Value) -> Result<Value> {
    let Some(j) = app.db.q1("SELECT * FROM jobs WHERE id = ? AND kind = 'jira'", p![job_id])? else {
        return err(404, format!("There's no Jira job {}.", rf("job", job_id)));
    };
    let ok = as_bool(body.get("ok"), false);
    if matches!(j.s("state"), Some("done")) {
        return err(409, format!("{} is already done.", rf("job", job_id)));
    }
    let op = board::job_args(&j).st("op");
    let key = body_str(body, "key");
    let key = if key.is_empty() { None } else { Some(crate::ops::jira_key(&key)?) };
    if ok && op == "create" && key.is_none() {
        return err(400, "Say which ticket: key=PROJ-123.");
    }
    let status = one_line(&body_str(body, "status"), 80);
    let message = one_line(&body_str(body, "message"), 300);
    if !ok && message.is_empty() {
        return err(400, "Say why it failed.");
    }
    let product = match body_str(body, "product") {
        p if p.is_empty() => Value::Null,
        p => json!(jira::clean_product(app, &p)?),
    };
    let extra = json!({"found": as_bool(body.get("found"), false), "product": product});
    app.db.tx(|| {
        jira::finish_with(
            app,
            job_id,
            ok,
            key.as_deref(),
            if status.is_empty() { None } else { Some(&status) },
            if message.is_empty() { None } else { Some(&message) },
            &extra,
        )
    })?;
    let j = app.db.q1("SELECT * FROM jobs WHERE id = ?", p![job_id])?.unwrap_or(j);
    Ok(board::job_dict(&j))
}
