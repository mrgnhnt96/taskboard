//! Idle task terminals give their memory back. A terminal the board opened for a task that has sat
//! idle at its prompt for `[limits] idle_close_mins` (waiting on a review, a CI run or the owner) is
//! asked to save a checkpoint, then closed once that turn is over. The task keeps its status and its
//! worktree slot; when the board next has a message for it (`deliver.rs`), or the owner answers its
//! question, its conversation carries on in a new terminal (fresh from the handoff when it's too big or
//! too old to resume).

use serde_json::json;

use crate::app::App;
use crate::util::*;
use crate::{board, deliver, fields, limits, runner};

/// The board's ask for a checkpoint before it closes the idle terminal.
pub const CHECKPOINT: &str = "idle_checkpoint";
/// The context key for the ask, while the terminal is still open: `{session, asked_at}`.
const ASKED: &str = "idle_close";
/// The context key for a task whose terminal was closed this way: `{session, at}`.
const CLOSED: &str = "idle_closed";

fn ask_text(app: &App, t: &Row, mins: i64) -> String {
    let tb = board::tb_cmd(app);
    format!(
        "[task-board:{}] This terminal has sat idle for {}, so the board will close it to free its memory. \
         Save a checkpoint now ({tb} checkpoint --done \"…\" --next \"…\" --decision \"…\") with what you're waiting \
         on and what comes next, then end your turn. The task keeps its slot and carries on in a new terminal \
         when the board has something for it.",
        rf("task", t.id()),
        limits::mins_text(mins)
    )
}

/// A task the board may close the idle terminal of: on it now, not waiting on a set time, and either
/// working or waiting on the owner's answer (which reopens it).
fn may_close(t: &Row, s: &Row) -> bool {
    let asked = t.s("status") == Some("needs") && t.s("needs_reason") == Some("question");
    (t.s("status") == Some("working") || asked)
        && !t.b("lost")
        && t.s("session_id") == s.s("id")
        && !crate::wait_until::waiting(t)
}

fn open_deliveries(app: &App, id: i64) -> Result<i64> {
    app.db.count("SELECT COUNT(*) FROM jobs WHERE kind = ? AND task_id = ? AND state IN ('pending','running')", crate::p![deliver::KIND, id])
}

/// The runner's sweep: asks idle task terminals for a checkpoint, then closes them.
pub fn sweep(app: &App) -> Result<()> {
    let mins = limits::get(app).idle_close_mins;
    if mins <= 0 {
        return Ok(());
    }
    for (s, t) in runner::task_terminals(app)? {
        let sid = s.st("id");
        if !may_close(&t, &s) || !runner::settled(&s) || board::limited(&s) || board::compacting_since(&s).is_string() {
            continue;
        }
        if !crate::lines::of(app, &sid)?.is_empty() || !board::opened_by_board(app, Some(&sid))? || open_deliveries(app, t.id())? > 0 {
            continue;
        }
        let idle = age_secs(s.s("status_at")).unwrap_or(0.0);
        let ctx = board::task_context(&t);
        let asked = ctx.get(ASKED).filter(|a| a.get("session").and_then(|v| v.as_str()) == Some(sid.as_str()));
        let asked_at = asked.and_then(|a| a.get("asked_at")).and_then(|v| v.as_str()).map(|s| s.to_string());
        // Asked and its turn since is over (or it never took the ask in, and sat idle long enough again).
        let ready = match &asked_at {
            Some(at) => {
                let turned = s.s("status_at").and_then(parse_iso).zip(parse_iso(at)).is_some_and(|(idle_at, asked)| idle_at > asked);
                turned || age_secs(Some(at)).unwrap_or(0.0) >= mins as f64 * 60.0
            }
            None => false,
        };
        if !ready && idle < mins as f64 * 60.0 {
            continue;
        }
        // A task waiting on the owner's answer isn't asked: checkpoint asks wait out a question, and its
        // reply could read as the answer. Its question and answers are in the handoff already.
        if !ready && t.s("status") == Some("working") {
            app.db.tx(|| {
                let mut ctx = board::task_context(&board::get_task(app, t.id())?);
                ctx.insert(ASKED.into(), json!({"session": sid, "asked_at": now_iso()}));
                board::save_context(app, t.id(), &ctx, false)?;
                deliver::add(app, CHECKPOINT, &ask_text(app, &t, mins), t.id(), None, None).map(|_| ())
            })?;
            continue;
        }
        if !runner::close_worth_trying(app, &sid, board::IDLE_CLOSE)? {
            continue;
        }
        let why = format!("It sat idle for over {}", limits::mins_text(mins));
        app.db.tx(|| runner::close_for(app, &s, &t, board::IDLE_CLOSE, &why).map(|_| ()))?;
    }
    Ok(())
}

/// `board::detach_closed` for an idle close: the task keeps its status, question and slot, and waits
/// for the board's next message (or the owner's answer) to carry on in a new terminal.
pub fn detached(app: &App, t: &Row, name: &str) -> Result<()> {
    let mut ctx = board::task_context(t);
    ctx.remove(ASKED);
    ctx.insert(CLOSED.into(), json!({"session": t.s("session_id"), "at": now_iso()}));
    board::save_context(app, t.id(), &ctx, false)?;
    let asked = t.s("status") == Some("needs");
    let then = if asked { "It carries on in a new terminal once you answer." } else { "It carries on in a new terminal when the board has something for it." };
    board::update_task(
        app,
        t.id(),
        fields!["session_id" => null, "start_job" => null, "lost" => 0, "pickup_session" => null,
                "latest" => format!("Closed its idle terminal to free memory. {then}")],
    )?;
    board::log_event(app, t.id(), board::MIDNA, "status", &format!("Closed {name} after it sat idle; {}", then.to_lowercase().trim_end_matches('.')))?;
    Ok(())
}

/// Whether `t`'s terminal was closed while it sat idle and nothing has picked it up since.
pub fn closed(t: &Row) -> bool {
    board::task_context(t).contains_key(CLOSED) && !has(t.s("session_id")) && t.s("status") != Some("done") && !t.b("lost")
}

/// A new terminal took the task: it isn't waiting on a reopen any more.
pub fn picked_up(app: &App, id: i64) -> Result<()> {
    let t = board::get_task(app, id)?;
    let mut ctx = board::task_context(&t);
    if ctx.remove(CLOSED).is_some() | ctx.remove(ASKED).is_some() {
        board::save_context(app, id, &ctx, false)?;
    }
    Ok(())
}

/// Starts idle-closed task `t` in a new terminal with `text` (the board's message, `what` it is): its
/// conversation carries on when it can, else it starts fresh from the handoff with the message after it.
/// False when the start failed (the owner is told).
pub fn reopen(app: &App, t: &Row, text: &str, what: &str) -> Result<bool> {
    let pickup = if matches!(t.s("pickup"), Some("manual") | Some("attach")) { "new".to_string() } else { t.st("pickup") };
    board::update_task(app, t.id(), fields!["start_job" => null, "pickup_session" => null, "pickup" => pickup])?;
    let t = board::get_task(app, t.id())?;
    let r = rf("task", t.id());
    let resume = format!("[task-board:{r}] The board closed this terminal while it sat idle; it carries on here with {what}:\n\n{text}");
    crate::devices::lend(app, &t)?;
    let fresh = format!("{}\n\n{text}", crate::handoff::start_prompt(app, t.id())?);
    let line = match runner::start_again(app, &t, resume, Some(fresh)) {
        Ok(runner::Again::Resumed) => format!("Its idle terminal was closed, so its conversation carries on in a new terminal with {what}"),
        Ok(runner::Again::Fresh(Some(why))) => format!("Its idle terminal was closed, so it starts again from the handoff with {what}: {why}"),
        Ok(runner::Again::Fresh(None)) => format!("Its idle terminal was closed, so it starts again from the handoff with {what}"),
        Err(e) => {
            board::log_event(app, t.id(), board::BOARD, "midna", &format!("Couldn't hand over {what}. {}", e.message))?;
            crate::dispatch::add_alert(app, &format!("{r} couldn't get {what}."), Some(t.id()), t.i("goal_id"), None, None)?;
            return Ok(false);
        }
    };
    board::log_event(app, t.id(), board::BOARD, "midna", &line)?;
    Ok(true)
}
