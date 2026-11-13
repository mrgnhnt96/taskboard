//! The question a turn ends on in a terminal with no task, screened like a task's turn
//! (`reports::screen_reply`): the Talk it over and alert-fix terminals, goal planning, the Jira desk
//! and review triage. An answer from the rules is typed back into the terminal once it has settled at
//! its prompt; a question the rules don't answer goes to the owner where they'd see it.

use serde_json::json;

use crate::app::App;
use crate::util::*;
use crate::{board, dispatch, midna, p, screen};

/// A held answer, typed into its terminal once it has sat idle at its prompt (`tick`).
pub const KIND: &str = "reply_answer";
/// How long the terminal sits idle before the answer goes in: `deliver`'s settle.
const SETTLE_SECS: f64 = 5.0;

/// Where a question the rules don't answer goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Asks {
    /// The owner works in the terminal: Needs you on it.
    Terminal,
    /// Background planning: an alert on the goal.
    Goal(i64),
    /// Nowhere: the terminal's own time limits handle a stuck agent.
    Nobody,
}

/// Where terminal `sid`'s questions go, by the job that opened it; None for a terminal the board
/// doesn't screen.
pub fn asks(app: &App, sid: &str) -> Result<Option<Asks>> {
    let Some(j) = app.db.q1("SELECT * FROM jobs WHERE kind = 'agent' AND json_extract(target, '$.session') = ? ORDER BY id DESC LIMIT 1", p![sid])?
    else {
        return Ok(None);
    };
    Ok(match j.st("purpose").as_str() {
        "human_talk" | "alert_talk" | "idea_talk" | "alert" => Some(Asks::Terminal),
        // Open in Claude brings the plan's terminal to the front for the owner to work in.
        "plan" if as_bool(board::job_args(&j).get("focus"), false) => Some(Asks::Terminal),
        "plan" => board::job_target(&j).i("goal").map(Asks::Goal),
        p if p == crate::jira_desk::PURPOSE || p == crate::review_triage::PURPOSE => Some(Asks::Nobody),
        _ => None,
    })
}

/// Screens `asked`, the end of a turn's reply in terminal `sid`, off the hook's clock. `stopped_at` is
/// the terminal's status time at that stop: once it changes, the terminal has moved on and this does
/// nothing.
pub fn screen(app: &App, sid: &str, asks: Asks, asked: &str, stopped_at: &str) -> Result<()> {
    let screened = screen::reply_question(app, None, asked).unwrap_or(json!({}));
    if screened["verdict"] == "none" {
        return Ok(());
    }
    app.db.tx(|| {
        if !still_stopped(app, sid, stopped_at)? {
            return Ok(());
        }
        let name = board::session_name(app, Some(sid), None);
        if screened["verdict"] == "answered" {
            let src = screened["source"].as_str().unwrap_or("the rules");
            let ans = screened["answer"].as_str().unwrap_or("");
            let text = format!(
                "[task-board] Answered from {src} without asking {}:\n{ans}\n\nCarry on. If this answer really doesn't fit, say why.",
                app.cfg.owner
            );
            board::create_job(app, KIND, json!({"to": sid, "text": text}), None, KIND, Some(json!({"stopped_at": stopped_at})))?;
            app.info(format!("reply question: {name} asked “{}”; answered from {src}", one_line(asked, 120)));
            return Ok(());
        }
        let question = screened["question"].as_str().filter(|q| !q.trim().is_empty()).unwrap_or(asked);
        match asks {
            Asks::Terminal => {
                app.db.x("UPDATE sessions SET status = 'needs' WHERE id = ? AND status != 'gone'", p![sid])?;
                midna::set_tab(app, sid, "question", Some(one_line(question, 300)));
            }
            Asks::Goal(g) => {
                app.db.x("UPDATE sessions SET status = 'needs' WHERE id = ? AND status != 'gone'", p![sid])?;
                dispatch::add_free_alert(app, &format!("{}: {question}", rf("goal", g)), None, Some(g))?;
            }
            Asks::Nobody => {}
        }
        app.info(format!("reply question: {name} asked “{}”; left to {}", one_line(question, 120), app.cfg.owner));
        Ok(())
    })
}

/// Whether terminal `sid` still sits where its turn stopped: idle, with no status change since.
fn still_stopped(app: &App, sid: &str, stopped_at: &str) -> Result<bool> {
    Ok(board::get_session(app, Some(sid))?.is_some_and(|s| s.s("status") == Some("idle") && s.s("status_at") == Some(stopped_at)))
}

/// Types each held answer into its terminal once it has sat idle at its prompt for `SETTLE_SECS`;
/// one whose terminal moved on meanwhile (another prompt, or it closed) is dropped.
pub fn tick(app: &App) -> Result<()> {
    for j in app.db.q("SELECT * FROM jobs WHERE kind = ? AND state = 'pending' ORDER BY id", p![KIND])? {
        let (args, target) = (board::job_args(&j), board::job_target(&j));
        let sid = args.st("to");
        if !still_stopped(app, &sid, &target.st("stopped_at"))? {
            app.db.x("UPDATE jobs SET state = 'expired', updated_at = ? WHERE id = ?", p![now_iso(), j.id()])?;
            app.info(format!("job {} reply answer dropped: {sid} moved on", rf("job", j.id())));
            continue;
        }
        let Some(s) = board::get_session(app, Some(&sid))? else { continue };
        if board::offline(&s) || age_secs(s.s("status_at")).unwrap_or(0.0) < SETTLE_SECS {
            continue;
        }
        board::create_job(app, "message", json!({"to": sid, "text": args.st("text")}), None, KIND, None)?;
        app.db.x("UPDATE jobs SET state = 'done', updated_at = ? WHERE id = ?", p![now_iso(), j.id()])?;
    }
    Ok(())
}
