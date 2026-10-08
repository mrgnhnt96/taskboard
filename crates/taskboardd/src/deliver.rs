//! Messages for a terminal's task: handed over by its hooks, or typed in through Midna once it's idle.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, dispatch, hours, p, prflow, runner};

pub const KIND: &str = "deliver";
pub const SEND: &str = "deliver_send";
const BOARD_PURPOSES: &[&str] = &["follow"];
pub const DELIVER_EVENTS: &[&str] = &["hook.session_start", "hook.prompt", "hook.stop"];
const RESEND_AFTER_SECS: f64 = 30.0;
const DELIVERY_WAIT_SECS: f64 = 90.0;
const SEND_SETTLE_SECS: f64 = 5.0;
const SEND_RETRY_SECS: f64 = 30.0;

fn what(purpose: &str) -> &'static str {
    match purpose {
        "answer" => "your answer",
        "offer" => "the offer",
        "checkpoint" => "the checkpoint request",
        "carry_on" => "the nudge to carry on",
        "follow" => "the rebase note",
        _ => "the board's message",
    }
}

fn done_text(purpose: &str, to: &str) -> String {
    match purpose {
        "answer" => format!("Delivered your answer to {to}"),
        "offer" => format!("Offered it to {to}; it starts when that terminal runs take"),
        "checkpoint" => format!("Asked {to} for a checkpoint"),
        "carry_on" => format!("{to} stopped with nothing to wait for, so the board told it to carry on"),
        "follow" => format!("Told {to} to rebase onto the work it builds on"),
        _ => format!("Delivered a message to {to}"),
    }
}

pub fn add(app: &App, purpose: &str, text: &str, task_id: i64, to: Option<&str>, not_before: Option<&str>) -> Result<i64> {
    let target = match not_before {
        Some(nb) => json!({"not_before": nb}),
        None => json!({}),
    };
    board::create_job(app, KIND, json!({"to": to, "text": text}), Some(task_id), purpose, Some(target))
}

fn held(target: &Row) -> bool {
    !hours::reached(target.s("not_before"))
}

fn open_jobs(app: &App, task_id: Option<i64>) -> Result<Vec<Row>> {
    match task_id {
        None => app.db.q("SELECT * FROM jobs WHERE kind = ? AND state IN ('pending','running') ORDER BY id", p![KIND]),
        Some(t) => app.db.q(
            "SELECT * FROM jobs WHERE kind = ? AND state IN ('pending','running') AND task_id = ? ORDER BY id",
            p![KIND, t],
        ),
    }
}

fn unwanted(app: &App, j: &Row, t: Option<&Row>) -> Result<Option<&'static str>> {
    let Some(t) = t else { return Ok(Some("its task was deleted")) };
    if t.s("status") == Some("done")
        && !(j.s("purpose") == Some("answer") && (prflow::visitor(t).is_some() || prflow::waking(app, t)?))
    {
        return Ok(Some("the task is done"));
    }
    if j.s("purpose") == Some("offer") && (t.i("start_job") != Some(j.id()) || has(t.s("session_id"))) {
        return Ok(Some("the task went elsewhere"));
    }
    Ok(None)
}

fn to(app: &App, j: &Row, t: Option<&Row>) -> Result<Option<Row>> {
    if j.s("purpose") == Some("offer") {
        return board::get_session(app, board::job_args(j).s("to"));
    }
    match t {
        Some(t) if t.s("status") == Some("done") => board::get_session(app, prflow::visitor(t).as_deref()),
        Some(t) => board::get_session(app, t.s("session_id")),
        None => Ok(None),
    }
}

pub fn drop(app: &App, task_id: i64, why: &str) -> Result<()> {
    for j in open_jobs(app, Some(task_id))? {
        app.db.x(
            "UPDATE jobs SET state = 'expired', result = ?, updated_at = ? WHERE id = ?",
            p![jdumps(&json!({"cancelled": why})), now_iso(), j.id()],
        )?;
    }
    Ok(())
}

fn hours_open(app: &App, t: &Row) -> Result<bool> {
    Ok(hours::goal_open(app, board::find_goal(app, t.i("goal_id"))?.as_ref()))
}

fn sending(app: &App, target: &Row) -> Result<bool> {
    let Some(jid) = target.i("send_job") else { return Ok(false) };
    let st = app.db.val("SELECT state FROM jobs WHERE id = ?", p![jid])?;
    Ok(matches!(st.as_str(), Some("pending") | Some("running")))
}

fn save_target(app: &App, j: &Row, target: &Row) -> Result<()> {
    app.db.x("UPDATE jobs SET target = ? WHERE id = ?", p![jdumps(&Value::Object(target.clone())), j.id()])?;
    Ok(())
}

fn send(app: &App, j: &Row, t: &Row, s: &Row, target: &mut Row) -> Result<Option<i64>> {
    let purpose = j.st("purpose");
    let failed_ago = age_secs(target.s("send_failed_at")).unwrap_or(SEND_RETRY_SECS);
    if !board::runs_claude(s)
        || s.s("status") != Some("idle")
        || board::offline(s)
        || (purpose == "checkpoint" && t.s("status") == Some("needs"))
        || (BOARD_PURPOSES.contains(&purpose.as_str()) && !hours_open(app, t)?)
        || age_secs(s.s("status_at")).unwrap_or(0.0) < SEND_SETTLE_SECS
        || sending(app, target)?
        || failed_ago < SEND_RETRY_SECS
    {
        return Ok(None);
    }
    let text = board::job_args(j).st("text");
    let jid = board::create_job(app, "message", json!({"to": s.v("id"), "text": text, "deliver": j.id()}), Some(t.id()), SEND, None)?;
    target.insert("send_job".into(), json!(jid));
    target.insert("sent_at".into(), json!(now_iso()));
    target.insert("sent_to".into(), s.v("id"));
    target.insert("sent_on".into(), json!("midna"));
    app.db.x(
        "UPDATE jobs SET state = 'running', attempts = attempts + 1, target = ?, updated_at = ? WHERE id = ?",
        p![jdumps(&Value::Object(target.clone())), now_iso(), j.id()],
    )?;
    board::log_event(
        app,
        t.id(),
        board::BOARD,
        "midna",
        &format!(
            "{} is idle at its prompt, so the board sent it {} through Midna",
            board::session_name(app, s.s("id"), None),
            what(&purpose)
        ),
    )?;
    Ok(Some(jid))
}

pub fn send_failed(app: &App, deliver_id: i64, why: &str) -> Result<()> {
    let Some(j) = app.db.q1("SELECT * FROM jobs WHERE id = ? AND kind = ?", p![deliver_id, KIND])? else { return Ok(()) };
    if !matches!(j.s("state"), Some("pending") | Some("running")) {
        return Ok(());
    }
    let mut target = board::job_target(&j);
    target.remove("send_job");
    target.insert("send_failed".into(), json!(why));
    target.insert("send_failed_at".into(), json!(now_iso()));
    app.db.x(
        "UPDATE jobs SET state = 'pending', target = ?, updated_at = ? WHERE id = ?",
        p![jdumps(&Value::Object(target)), now_iso(), j.id()],
    )?;
    Ok(())
}

/// Hands this terminal's waiting messages to a live hook report.
pub fn take(app: &App, sid: Option<&str>, event: &str) -> Result<Option<Value>> {
    let Some(sid) = sid.filter(|s| !s.is_empty()) else { return Ok(None) };
    if !DELIVER_EVENTS.contains(&event) {
        return Ok(None);
    }
    let mine = match board::task_for_session(app, Some(sid))? {
        Some(t) => Some(t),
        None => prflow::visited_by(app, sid)?,
    };
    let mut handed = vec![];
    for j in open_jobs(app, None)? {
        let mut target = board::job_target(&j);
        if held(&target) || sending(app, &target)? {
            continue;
        }
        if j.s("state") == Some("running") && age_secs(target.s("sent_at")).unwrap_or(0.0) < RESEND_AFTER_SECS {
            continue;
        }
        if j.s("purpose") == Some("offer") {
            if board::job_args(&j).s("to") != Some(sid) {
                continue;
            }
        } else if mine.as_ref().map(|m| Some(m.id()) != j.i("task_id")).unwrap_or(true) {
            continue;
        }
        let t = board::find_task(app, j.i("task_id"))?;
        if unwanted(app, &j, t.as_ref())?.is_some() {
            continue;
        }
        target.insert("sent_at".into(), json!(now_iso()));
        target.insert("sent_to".into(), json!(sid));
        target.insert("sent_on".into(), json!(event));
        app.db.x(
            "UPDATE jobs SET state = 'running', attempts = attempts + 1, target = ?, updated_at = ? WHERE id = ?",
            p![jdumps(&Value::Object(target)), now_iso(), j.id()],
        )?;
        handed.push(j);
    }
    if handed.is_empty() {
        return Ok(None);
    }
    let ids: Vec<i64> = handed.iter().map(|j| j.id()).collect();
    let text = handed.iter().map(|j| board::job_args(j).st("text")).collect::<Vec<_>>().join("\n\n");
    Ok(Some(json!({"ids": ids, "text": text})))
}

pub fn delivered(app: &App, sid: Option<&str>, ids: &[Value]) -> Result<()> {
    for jid in ids {
        let Some(jid) = jid.as_i64().or_else(|| jid.as_str().and_then(|s| s.parse().ok())) else { continue };
        let Some(j) = app.db.q1("SELECT * FROM jobs WHERE id = ? AND kind = ?", p![jid, KIND])? else { continue };
        if j.s("state") == Some("done") {
            continue;
        }
        let mut target = board::job_target(&j);
        target.insert("delivered_at".into(), json!(now_iso()));
        app.db.x(
            "UPDATE jobs SET state = 'done', target = ?, updated_at = ? WHERE id = ?",
            p![jdumps(&Value::Object(target.clone())), now_iso(), j.id()],
        )?;
        let Some(tid) = j.i("task_id") else { continue };
        board::log_event(app, tid, board::BOARD, "midna", &done_text(&j.st("purpose"), &board::session_name(app, sid, None)))?;
        if target.contains_key("alerted") {
            dispatch::clear_alerts(app, Some(tid), None)?;
        }
    }
    Ok(())
}

fn expire(app: &App, j: &Row, t: Option<&Row>, why: &str) -> Result<()> {
    app.db.x("UPDATE jobs SET state = 'expired', updated_at = ? WHERE id = ?", p![now_iso(), j.id()])?;
    app.info(format!("job {} {} not delivered: {why}", rf("job", j.id()), j.st("purpose")));
    if let Some(t) = t {
        if j.s("purpose") == Some("answer") && (t.s("status") != Some("done") || prflow::visitor(t).is_some()) {
            board::log_event(
                app,
                t.id(),
                board::BOARD,
                "midna",
                &format!("Your answer didn't reach the terminal before {why}. The next terminal gets it with the handoff"),
            )?;
        }
    }
    Ok(())
}

/// An answer for a task whose terminal is closed: start it again with the answer in its handoff.
pub fn answer_in_new_tab(app: &App, t: &Row, text: &str) -> Result<bool> {
    if t.s("status") == Some("done") {
        if prflow::live_job(app, t)?.is_some() {
            add(app, "answer", text, t.id(), None, None)?;
            return Ok(true);
        }
        if prflow::answer_in_new_tab(app, t, text)?.is_none() {
            return Ok(false);
        }
        board::log_event(
            app,
            t.id(),
            board::BOARD,
            "midna",
            &format!("Its terminal is closed, so the board reopened its conversation for PR #{} with your answer", t.i0("pr_num")),
        )?;
        return Ok(true);
    }
    if !has(t.s("started_at")) || crate::waitsfor::parked(t).is_some() {
        return Ok(false);
    }
    if board::live_start_job(app, t)?.is_some() {
        return Ok(true);
    }
    let pickup = if matches!(t.s("pickup"), Some("manual") | Some("attach")) { "new".to_string() } else { t.st("pickup") };
    board::update_task(
        app,
        t.id(),
        crate::fields!["session_id" => null, "lost" => 0, "start_job" => null, "pickup_session" => null, "pickup" => pickup],
    )?;
    if let Err(e) = runner::start_task(app, &board::get_task(app, t.id())?, "new", None, None, None, None) {
        board::log_event(app, t.id(), board::BOARD, "midna", &format!("Couldn't hand over the board's message. {}", e.message))?;
        dispatch::add_alert(app, &format!("{} couldn't get the board's message.", rf("task", t.id())), Some(t.id()), t.i("goal_id"), None, None)?;
        return Ok(false);
    }
    board::log_event(
        app,
        t.id(),
        board::BOARD,
        "midna",
        "Its terminal is closed, so the board started it in a new terminal with your answer in its handoff",
    )?;
    Ok(true)
}

fn answered_elsewhere(app: &App, j: &Row, t: Option<&Row>) -> Result<bool> {
    let Some(t) = t else { return Ok(false) };
    if j.s("purpose") != Some("answer") {
        return Ok(false);
    }
    let text = board::job_args(j).st("text");
    if !answer_in_new_tab(app, t, &text)? {
        return Ok(false);
    }
    app.db.x(
        "UPDATE jobs SET state = 'done', result = ?, updated_at = ? WHERE id = ?",
        p![jdumps(&json!({"new_tab": true})), now_iso(), j.id()],
    )?;
    Ok(true)
}

pub fn tick(app: &App) -> Result<()> {
    for j in open_jobs(app, None)? {
        let t = board::find_task(app, j.i("task_id"))?;
        let mut target = board::job_target(&j);
        if held(&target) {
            continue;
        }
        if has(target.s("not_before")) && !target.contains_key("released_at") {
            target.insert("released_at".into(), json!(now_iso()));
            save_target(app, &j, &target)?;
        }
        let s = to(app, &j, t.as_ref())?;
        let mut why = unwanted(app, &j, t.as_ref())?;
        let gone = s.as_ref().map(|s| s.s("status") == Some("gone")).unwrap_or(true);
        let live_pr = match &t {
            Some(t) => prflow::live_job(app, t)?.is_some(),
            None => false,
        };
        if why.is_none() && gone && !live_pr {
            if answered_elsewhere(app, &j, t.as_ref())? {
                continue;
            }
            why = Some("its terminal closed");
        }
        if let Some(why) = why {
            expire(app, &j, t.as_ref(), why)?;
            continue;
        }
        let (Some(t), purpose) = (t, j.st("purpose")) else { continue };
        let sent_recently = j.s("state") == Some("running") && age_secs(target.s("sent_at")).unwrap_or(0.0) < RESEND_AFTER_SECS;
        if purpose != "offer" && !sent_recently {
            if let Some(s) = &s {
                send(app, &j, &t, s, &mut target)?;
            }
        }
        if sending(app, &target)? {
            continue;
        }
        let waits_for_hours = BOARD_PURPOSES.contains(&purpose.as_str()) && !hours_open(app, &t)?;
        // Offline counts as mid-turn: Midna carries it on when the network is back, so it isn't idle.
        let mid_turn = s.as_ref().map(|s| s.s("status") == Some("working") || board::offline(s)).unwrap_or(false);
        if mid_turn && target.contains_key("alerted") {
            target.remove("alerted");
            save_target(app, &j, &target)?;
            dispatch::clear_alerts(app, Some(t.id()), None)?;
        }
        let since_release = age_secs(target.s("released_at").or(j.s("created_at"))).unwrap_or(0.0);
        let since_status = s.as_ref().and_then(|s| age_secs(s.s("status_at"))).unwrap_or(f64::INFINITY);
        let waited = since_release.min(since_status);
        if !target.contains_key("alerted") && !mid_turn && !waits_for_hours && waited >= DELIVERY_WAIT_SECS {
            target.insert("alerted".into(), json!(now_iso()));
            save_target(app, &j, &target)?;
            let w = what(&purpose);
            let name = s.as_ref().map(|s| board::session_name(app, s.s("id"), None)).unwrap_or_else(|| "Its terminal".into());
            board::log_event(
                app,
                t.id(),
                board::BOARD,
                "midna",
                &format!("{name} is idle at its prompt and hasn't picked up {w}. Type anything in it to hand it over"),
            )?;
            dispatch::add_alert(
                app,
                &format!("{} is idle and hasn't picked up {w}. Type anything in its terminal to hand it over.", rf("task", t.id())),
                Some(t.id()),
                t.i("goal_id"),
                None,
                None,
            )?;
        }
    }
    Ok(())
}
