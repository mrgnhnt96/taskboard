//! A start job's terminal, from the moment Midna opens it until its agent checks in (`board::claim`
//! clears `start_job`). While it sits on Claude's folder-trust prompt the task says so; once it exits
//! or closes before the agent checks in, the task goes back in the queue to start again, counted
//! like any other failed start.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, jobs, p};

/// Midna's status reason while Claude asks whether to trust a new folder.
pub const FOLDER_TRUST: &str = "folder trust";
/// How long a start terminal Midna doesn't list, and the board never saw, may take to show up.
const UNSEEN_GRACE_SECS: f64 = 120.0;
const STARTING: &str = "Starting in Midna…";

/// A task whose start job opened a terminal no agent has checked in from yet.
pub struct Opened {
    pub task: Row,
    pub job: Row,
    pub sid: String,
}

fn opened_rows(app: &App, sid: Option<&str>) -> Result<Vec<Opened>> {
    let rows = app.db.q(
        "SELECT t.id AS tid, j.id AS jid, json_extract(j.target, '$.session') AS sid FROM tasks t JOIN jobs j ON j.id = t.start_job \
         WHERE t.status = 'queued' AND COALESCE(t.session_id, '') = '' AND j.kind = 'agent' AND j.state = 'done' \
         AND COALESCE(json_extract(j.target, '$.session'), '') != '' AND (? IS NULL OR json_extract(j.target, '$.session') = ?)",
        p![sid, sid],
    )?;
    let mut out = vec![];
    for r in rows {
        let (Some(task), Some(job)) = (board::find_task(app, r.i("tid"))?, app.db.q1("SELECT * FROM jobs WHERE id = ?", p![r.i("jid")])?) else {
            continue;
        };
        out.push(Opened { task, job, sid: r.st("sid") });
    }
    Ok(out)
}

/// Every task waiting on its start terminal's agent to check in.
pub fn opened(app: &App) -> Result<Vec<Opened>> {
    opened_rows(app, None)
}

/// The task waiting on terminal `sid`'s agent to check in, if one is.
pub fn opened_by(app: &App, sid: &str) -> Result<Option<Opened>> {
    Ok(opened_rows(app, Some(sid).filter(|s| !s.is_empty()))?.into_iter().find(|o| o.sid == sid))
}

/// Task `t`'s start terminal, while its agent hasn't checked in.
pub fn of_task(app: &App, t: &Row) -> Result<Option<Opened>> {
    if t.s("status") != Some("queued") || has(t.s("session_id")) {
        return Ok(None);
    }
    let Some(job) = app.db.q1("SELECT * FROM jobs WHERE id = ? AND kind = 'agent' AND state = 'done'", p![t.i("start_job")])? else {
        return Ok(None);
    };
    let sid = board::job_target(&job).st("session");
    Ok((!sid.is_empty()).then(|| Opened { task: t.clone(), job, sid }))
}

/// A card's `start_terminal`: the terminal its start opened, while no agent has checked in from it,
/// and whether it waits on the folder-trust prompt.
pub fn card(app: &App, t: &Row) -> Result<Value> {
    let Some(o) = of_task(app, t)? else { return Ok(Value::Null) };
    let name = name_of(app, &o.sid, None);
    Ok(json!({"session_id": o.sid, "name": name, "folder_trust": t.st("latest") == trust_line(&name)}))
}

/// The line a queued card shows while its start terminal is open with no agent checked in.
pub fn waiting_line(app: &App, t: &Row) -> Result<Option<String>> {
    let Some(o) = of_task(app, t)? else { return Ok(None) };
    let name = name_of(app, &o.sid, None);
    let trust = trust_line(&name);
    Ok(Some(if t.st("latest") == trust { trust } else { format!("Opened {name}; waiting for its agent to check in") }))
}

/// The task line while its start terminal waits on the folder-trust prompt.
pub fn trust_line(name: &str) -> String {
    format!("Waiting on the folder-trust prompt in {name}")
}

fn name_of(app: &App, sid: &str, listed: Option<&Value>) -> String {
    let named = listed.and_then(|s| s["name"].as_str()).filter(|n| !n.is_empty()).map(str::to_string);
    named.unwrap_or_else(|| board::session_name(app, Some(sid), Some("its terminal")))
}

/// Whether a listed terminal shows Claude's folder-trust prompt.
pub fn on_folder_trust(listed: &Value) -> bool {
    let st = &listed["status"];
    st["state"] == "needs_you" && st["reason"].as_str().is_some_and(|r| r.trim().eq_ignore_ascii_case(FOLDER_TRUST))
}

/// Puts the folder-trust prompt on the task's line while its start terminal shows it, and the usual
/// line back once it's answered.
fn show_trust(app: &App, o: &Opened, listed: &Value) -> Result<()> {
    let line = trust_line(&name_of(app, &o.sid, Some(listed)));
    let latest = o.task.st("latest");
    if on_folder_trust(listed) {
        if latest != line {
            board::update_task(app, o.task.id(), fields!["latest" => line.clone()])?;
            board::log_event(app, o.task.id(), board::MIDNA, "midna", &format!("{line}. It starts once you answer it there"))?;
        }
    } else if latest == line {
        board::update_task(app, o.task.id(), fields!["latest" => STARTING])?;
    }
    Ok(())
}

/// Whether the start terminal is gone: the board marked it gone, Midna lists it as exited, or Midna
/// doesn't list one the board never saw a while after it opened.
fn gone(app: &App, o: &Opened, listed: Option<&Value>) -> Result<bool> {
    if listed.is_some_and(|s| s["status"]["state"] == "exited") {
        return Ok(true);
    }
    match board::get_session(app, Some(&o.sid))? {
        Some(s) => Ok(s.s("status") == Some("gone")),
        None => Ok(listed.is_none() && age_secs(o.job.s("updated_at")).is_some_and(|a| a > UNSEEN_GRACE_SECS)),
    }
}

/// What became of the terminal, for the task's line: how it exited, or who closed it.
fn what_happened(app: &App, o: &Opened, listed: Option<&Value>, how: Option<&str>) -> Result<String> {
    if let Some(h) = how.filter(|h| !h.is_empty()) {
        return Ok(format!(" ({h})"));
    }
    if let Some(d) = listed.filter(|s| s["status"]["state"] == "exited").and_then(|s| crate::midna::describe_exit(Some(s), None, now_ts())) {
        return Ok(format!(" ({d})"));
    }
    let by = board::get_session(app, Some(&o.sid))?.and_then(|s| s.s("closed_by").map(str::to_string));
    Ok(match by.as_deref() {
        Some("human") => " (you closed it)".into(),
        Some(b) if !b.is_empty() => format!(" (closed by {b})"),
        _ => String::new(),
    })
}

/// The start terminal ended before its agent checked in: logged, and the task back in the queue to
/// start again (its worktree slot kept for the retry), failing after the usual tries.
pub fn ended(app: &App, o: &Opened, listed: Option<&Value>, how: Option<&str>) -> Result<()> {
    let name = name_of(app, &o.sid, listed);
    let on_trust = o.task.st("latest") == trust_line(&name);
    let detail = what_happened(app, o, listed, how)?;
    let text = format!(
        "{name} closed before its agent checked in{}{detail}",
        if on_trust { ", still on the folder-trust prompt" } else { "" }
    );
    app.info(format!("start: {} {}", rf("task", o.task.id()), text));
    jobs::start_failed(app, &o.task, &text, true, true)?;
    Ok(())
}

/// One look at the start terminals against Midna's list: the folder-trust prompt shown, and the
/// ones gone (also those that went while the daemon was down) requeued.
pub fn sweep(app: &App, sessions: &[Value]) -> Result<()> {
    for o in opened(app)? {
        let listed = sessions.iter().find(|s| s["id"].as_str() == Some(o.sid.as_str()));
        if gone(app, &o, listed)? {
            ended(app, &o, listed, None)?;
        } else if let Some(s) = listed {
            show_trust(app, &o, s)?;
        }
    }
    Ok(())
}

/// Midna's `session.exited` or `session.closed` for a start terminal: the task goes back in the
/// queue at once, and what's left of an exited terminal is closed, so a restarted agent doesn't
/// start the task a second time.
pub fn midna_event(app: &App, kind: &str, sid: &str, data: &Value) -> Result<()> {
    let Some(o) = opened_by(app, sid)? else { return Ok(()) };
    let how = if kind == "session.exited" {
        let st = json!({"status": {"exit_code": data["exit_code"]}});
        crate::midna::describe_exit(Some(&st), None, now_ts())
    } else {
        None
    };
    ended(app, &o, None, how.as_deref())?;
    if kind == "session.exited" {
        board::create_job(app, "close", json!({"session": sid, "force": true}), Some(o.task.id()), "start_ended", None)?;
    } else {
        board::upsert_session(app, sid, fields!["status" => "gone", "gone_at" => now_iso()])?;
    }
    Ok(())
}

/// Starting task `t` over while its start terminal still waits for its agent: that terminal is closed,
/// so only the new one picks the task up.
pub fn start_over(app: &App, t: &Row) -> Result<()> {
    let Some(o) = of_task(app, t)? else { return Ok(()) };
    let name = name_of(app, &o.sid, None);
    board::create_job(app, "close", json!({"session": o.sid, "force": true}), Some(t.id()), "start_over", None)?;
    board::log_event(app, t.id(), board::BOARD, "midna", &format!("Closing {name}, which its agent never checked in from, to start it over"))?;
    Ok(())
}
