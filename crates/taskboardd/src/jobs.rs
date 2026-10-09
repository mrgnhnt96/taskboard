//! Midna jobs: which runs next, what its result means for its task, and when it expires.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, deliver, dispatch, fields, p, prflow};

pub const MIDNA_KINDS: &str = "('agent','message','close','focus','rename')";

fn not_claude(app: &App, j: &Row) -> Result<bool> {
    if j.s("kind") != Some("message") {
        return Ok(false);
    }
    let s = board::get_session(app, board::job_args(j).s("to"))?;
    Ok(s.map(|s| has(s.s("agent")) && !s.st("agent").to_lowercase().contains("claude")).unwrap_or(false))
}

/// The next pending Midna job that may run, marked running.
pub fn take(app: &App) -> Result<Option<Row>> {
    loop {
        let picked = app.db.tx(|| {
            let rows = app.db.q(&format!("SELECT * FROM jobs WHERE state = 'pending' AND kind IN {MIDNA_KINDS} ORDER BY id"), p![])?;
            let mut j = None;
            for r in rows {
                if dispatch::launch_ready(app, &r)? {
                    j = Some(r);
                    break;
                }
            }
            let Some(j) = j else { return Ok(None) };
            if not_claude(app, &j)? {
                let res = json!({"ok": false, "exit_code": -1, "stdout": "",
                                 "stderr": "That terminal isn't running Claude, so the board didn't send it anything."});
                app.db.x("UPDATE jobs SET state = 'failed', result = ?, updated_at = ? WHERE id = ?", p![jdumps(&res), now_iso(), j.id()])?;
                after_result(app, &j, false, &res)?;
                return Ok(Some(None));
            }
            app.db.x("UPDATE jobs SET state = 'running', attempts = attempts + 1, updated_at = ? WHERE id = ?", p![now_iso(), j.id()])?;
            Ok(Some(Some(j)))
        })?;
        match picked {
            None => return Ok(None),
            Some(None) => continue,
            Some(Some(j)) => {
                app.info(format!("job {} {} handed to Midna", rf("job", j.id()), j.st("kind")));
                return Ok(Some(j));
            }
        }
    }
}

fn first_line(s: &str) -> String {
    s.lines().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("").to_string()
}

pub fn result(app: &App, job_id: i64, ok: bool, stdout: &str, stderr: &str, exit_code: i64) -> Result<()> {
    let Some(j) = app.db.q1("SELECT * FROM jobs WHERE id = ?", p![job_id])? else {
        return err(404, format!("There's no job {}.", rf("job", job_id)));
    };
    let res = json!({"ok": ok, "exit_code": exit_code, "stdout": clip(stdout, 4000), "stderr": clip(stderr, 4000)});
    let late = matches!(j.s("state"), Some("expired") | Some("done") | Some("failed"));
    app.db.x(
        "UPDATE jobs SET state = ?, result = ?, updated_at = ? WHERE id = ?",
        p![if ok { "done" } else { "failed" }, jdumps(&res), now_iso(), j.id()],
    )?;
    let why = if ok { String::new() } else { format!(": {}", first_line(stderr)) };
    app.info(format!("job {} {} {}{why}", rf("job", j.id()), j.st("kind"), if ok { "ok" } else { "failed" }));
    if late && j.s("state") == Some("expired") && !ok {
        return Ok(());
    }
    after_result(app, &j, ok, &res)?;
    if ok {
        if let Some(t) = j.i("task_id") {
            dispatch::clear_alerts(app, Some(t), None)?;
        }
    }
    Ok(())
}

pub fn you_said_no(why: &str) -> bool {
    why.contains("denied by user")
}

pub fn after_result(app: &App, j: &Row, ok: bool, res: &Value) -> Result<()> {
    let a = board::job_args(j);
    let t = board::find_task(app, j.i("task_id"))?;
    let stderr = res["stderr"].as_str().unwrap_or("");
    let stdout = res["stdout"].as_str().unwrap_or("");
    let why = {
        let w = first_line(stderr);
        if !w.is_empty() {
            w
        } else {
            let w = first_line(stdout);
            if w.is_empty() {
                format!("exit code {}", res["exit_code"])
            } else {
                w
            }
        }
    };
    let purpose = j.st("purpose");
    match j.st("kind").as_str() {
        "agent" => {
            if purpose == "plan" || purpose == "reopen" {
                return Ok(());
            }
            let Some(t) = t else { return Ok(()) };
            if ok {
                let text = if let Some(to) = stdout.strip_prefix("Queued in ") {
                    format!("Sent to {} in Midna", board::session_name(app, Some(to.trim()), None))
                } else if a.st("flags").contains("--resume") {
                    "Reopened the old conversation in a new Midna terminal".into()
                } else if as_bool(a.get("queue"), false) {
                    "Started in a new Midna terminal once its project had a free terminal".into()
                } else {
                    "Started in a new Midna terminal".into()
                };
                board::log_event(app, t.id(), board::MIDNA, "midna", &text)?;
            } else if !(purpose == "pr" && prflow::wake_failed(app, &t, &format!("Midna refused: {why}"))?) {
                start_failed(app, &t, &format!("Midna refused: {why}"), true, !you_said_no(&why))?;
            }
        }
        "message" => {
            if let Some(did) = a.i("deliver") {
                if ok {
                    deliver::delivered(app, a.s("to"), &[json!(did)])?;
                } else {
                    deliver::send_failed(app, did, &why)?;
                    if let Some(t) = &t {
                        board::log_event(app, t.id(), board::MIDNA, "midna", &format!("Midna refused: {why}"))?;
                    }
                }
                return Ok(());
            }
            let Some(t) = t else { return Ok(()) };
            let to = board::session_name(app, a.s("to"), None);
            if ok {
                let text = match purpose.as_str() {
                    "answer" => format!("Sent your answer to {to}"),
                    "checkpoint" => format!("Asked {to} for a checkpoint"),
                    _ => format!("Sent a message to {to}"),
                };
                board::log_event(app, t.id(), board::MIDNA, "midna", &text)?;
            } else {
                board::log_event(app, t.id(), board::MIDNA, "midna", &format!("Midna refused: {why}"))?;
            }
        }
        "close" => {
            let sid = a.st("session");
            board::session_event(
                app,
                &sid,
                if ok { "closed" } else { "close_failed" },
                &if ok { "Closed in Midna".to_string() } else { format!("Midna didn't close it: {why}") },
                None,
            )?;
            if ok {
                let on = app.db.q("SELECT * FROM tasks WHERE session_id = ?", p![sid])?;
                if !sid.is_empty() {
                    board::upsert_session(
                        app,
                        &sid,
                        fields!["status" => "gone", "gone_at" => now_iso(), "last_task" => on.last().map(|t| t.id())],
                    )?;
                }
                for tt in on {
                    board::detach_closed(app, &tt)?;
                }
            } else if let Some(t) = t {
                board::log_event(app, t.id(), board::MIDNA, "midna", &format!("Midna refused to close the terminal: {why}"))?;
            }
        }
        "rename" => {
            let (sid, name) = (a.st("session"), a.st("name"));
            let old = board::session_name(app, Some(&sid), None);
            if ok && !sid.is_empty() && !name.is_empty() {
                app.db.x("UPDATE sessions SET name = ? WHERE id = ?", p![name, sid])?;
            }
            if !(ok && old == name) {
                board::session_event(
                    app,
                    &sid,
                    "rename",
                    &if ok { format!("Renamed from “{old}” to “{name}”") } else { format!("Midna didn't rename it: {why}") },
                    None,
                )?;
            }
            for tt in app.db.q("SELECT * FROM tasks WHERE session_id = ? AND status != 'done'", p![sid])? {
                board::log_event(
                    app,
                    tt.id(),
                    board::MIDNA,
                    "midna",
                    &if ok {
                        format!("Renamed its terminal from “{old}” to “{name}”")
                    } else {
                        format!("Midna didn't rename its terminal: {why}")
                    },
                )?;
            }
        }
        "focus" => {
            if let Some(t) = t {
                board::log_event(
                    app,
                    t.id(),
                    board::MIDNA,
                    "midna",
                    &if ok { "Brought its terminal to the front".to_string() } else { format!("Midna refused: {why}") },
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn closing(app: &App, sid: &str) -> Result<bool> {
    Ok(app
        .db
        .q1(
            "SELECT 1 FROM jobs WHERE kind = 'close' AND state IN ('pending','running') AND json_extract(args, '$.session') = ? LIMIT 1",
            p![sid],
        )?
        .is_some())
}

const RENAME_ERROR_SHOWN_SECS: f64 = 600.0;

pub fn rename_state(app: &App, sid: &str) -> Result<Row> {
    let mut out = Row::new();
    let Some(j) = app.db.q1(
        "SELECT * FROM jobs WHERE kind = 'rename' AND json_extract(args, '$.session') = ? ORDER BY id DESC LIMIT 1",
        p![sid],
    )?
    else {
        return Ok(out);
    };
    match j.s("state") {
        Some("pending") | Some("running") => {
            out.insert("renaming".into(), board::job_args(&j).v("name"));
        }
        Some("failed") | Some("expired") if age_secs(j.s("updated_at")).unwrap_or(0.0) <= RENAME_ERROR_SHOWN_SECS => {
            let res = jloads_obj(j.s("result"));
            let mut why = first_line(res.s("stderr").unwrap_or(""));
            if why.is_empty() {
                why = first_line(res.s("stdout").unwrap_or(""));
            }
            if why.is_empty() {
                why = "Midna didn't answer in time".into();
            }
            out.insert("rename_error".into(), json!(why));
        }
        _ => {}
    }
    Ok(out)
}

const START_RETRY_WAITS: [i64; 3] = [60, 300, 900];

/// A start that didn't happen: retry after 1, 5 and 15 minutes, then wait for the owner.
pub fn start_failed(app: &App, t: &Row, text: &str, counts: bool, retry: bool) -> Result<bool> {
    let t = board::get_task(app, t.id())?;
    if t.s("status") == Some("done") {
        return Ok(false);
    }
    let tries = t.i0("start_tries") + if counts { 1 } else { 0 };
    if !retry || tries > START_RETRY_WAITS.len() as i64 {
        board::update_task(
            app,
            t.id(),
            fields!["status" => "needs", "needs_reason" => "start_failed", "start_job" => null, "latest" => text,
                    "start_tries" => 0, "retry_at" => null],
        )?;
        let msg = if retry {
            format!("{text}. It didn't start after {} tries, so it's waiting for you", START_RETRY_WAITS.len() + 1)
        } else {
            text.to_string()
        };
        board::log_event(app, t.id(), board::MIDNA, "midna", &msg)?;
        dispatch::add_alert(app, &format!("{} didn't start: {}", rf("task", t.id()), one_line(text, 200)), Some(t.id()), t.i("goal_id"), None, None)?;
        return Ok(false);
    }
    let wait = if counts { START_RETRY_WAITS[(tries - 1).max(0) as usize] } else { 0 };
    let pickup = if t.s("pickup") == Some("manual") { "queue".to_string() } else { t.st("pickup") };
    board::update_task(
        app,
        t.id(),
        fields!["status" => "queued", "needs_reason" => null, "start_job" => null, "latest" => text,
                "start_tries" => tries, "retry_at" => if wait > 0 { Some(iso(now_ts() + wait as f64)) } else { None },
                "pickup" => pickup],
    )?;
    let msg = if wait > 0 {
        let m = wait / 60;
        format!("{text}. Trying again in {m} minute{}", if m != 1 { "s" } else { "" })
    } else {
        format!("{text}. Trying again once Midna is back")
    };
    board::log_event(app, t.id(), board::MIDNA, "midna", &msg)?;
    Ok(true)
}

pub fn expire(app: &App) -> Result<()> {
    let running_max = app.cfg.intervals.running_job_expiry;
    let pending_max = app.cfg.intervals.pending_job_expiry;
    for j in app.db.q(
        "SELECT * FROM jobs WHERE state IN ('pending','running') AND kind NOT IN ('jira', 'deliver')",
        p![],
    )? {
        let state = j.st("state");
        if state == "pending" && j.s("kind") == Some("agent") && !dispatch::launch_ready(app, &j)? {
            continue;
        }
        let age = age_secs(if state == "running" { j.s("updated_at") } else { j.s("created_at") }).unwrap_or(0.0);
        if !((state == "running" && age > running_max) || (state == "pending" && age > pending_max)) {
            continue;
        }
        app.db.x("UPDATE jobs SET state = 'expired', updated_at = ? WHERE id = ?", p![now_iso(), j.id()])?;
        let why = if state == "running" { "Midna didn't answer in time" } else { "Midna wasn't running for an hour" };
        app.info(format!("job {} {} expired: {why}", rf("job", j.id()), j.st("kind")));
        let Some(t) = board::find_task(app, j.i("task_id"))? else { continue };
        if j.s("kind") == Some("agent") && j.s("purpose") == Some("pr") && prflow::wake_failed(app, &t, why)? {
            continue;
        }
        if t.i("start_job") == Some(j.id()) {
            start_failed(app, &t, &format!("Couldn't start: {why}"), state == "running", true)?;
        } else {
            board::log_event(app, t.id(), board::BOARD, "midna", &format!("Gave up on a Midna {}: {why}", j.st("kind")))?;
        }
    }
    Ok(())
}
