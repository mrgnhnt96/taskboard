//! Reviewers who run their own AI review bot every few hours (`tb reviewers bot <who> --every <h>
//! --mark <text>`). The board spots each run from the marker in that person's comments on the PRs
//! it watches (`bot_window_hours` back; comments within `bot_run_gap_mins` of a run are the same
//! run) and keeps them in `reviewer_bot_runs`. Once a run has been seen recently (within two
//! intervals), the bot is timed: its next run is the last one plus the interval, the picker asks
//! that person only when the next run is at most `bot_due_mins` away, and their pace is the
//! fastest the speed table gives. A bot not seen yet is a reviewer like any other.

use serde_json::Value;

use crate::app::App;
use crate::reviewers;
use crate::util::*;
use crate::{fields, p};

/// The last run seen, as Unix seconds.
pub fn last_run(app: &App, r: &Row) -> Result<Option<f64>> {
    Ok(app.db.val("SELECT MAX(at) FROM reviewer_bot_runs WHERE reviewer_id = ?", p![r.id()])?.as_str().and_then(parse_iso))
}

/// When their bot runs next, if its schedule is known.
pub fn next_run(app: &App, r: &Row) -> Result<Option<f64>> {
    let Some(every) = r.f("bot_every_h").filter(|h| *h > 0.0) else { return Ok(None) };
    let Some(last) = last_run(app, r)? else { return Ok(None) };
    let gap = every * 3600.0;
    Ok((now_ts() - last < 2.0 * gap).then_some(last + gap))
}

/// Their bot runs on a known schedule.
pub fn timed(app: &App, r: &Row) -> Result<bool> {
    Ok(next_run(app, r)?.is_some())
}

/// Whether the picker may ask them now: not timed, or their bot runs within `bot_due_mins`.
pub fn may_ask(app: &App, r: &Row) -> Result<bool> {
    Ok(match next_run(app, r)? {
        None => true,
        Some(next) => next - now_ts() <= app.cfg.reviewers.bot_due_mins * 60.0,
    })
}

/// Notes the bot runs shown by marked comments on the board's PRs.
pub fn note_runs(app: &App) -> Result<i64> {
    let bots = app.db.q("SELECT * FROM reviewers WHERE bot_every_h IS NOT NULL AND bot_mark IS NOT NULL AND bot_mark != ''", p![])?;
    if bots.is_empty() {
        return Ok(0);
    }
    let cfg = &app.cfg.reviewers;
    let since = now_ts() - cfg.bot_window_hours * 3600.0;
    let tasks = app.db.q("SELECT id, project, pr_repo, pr_num, pr_flow FROM tasks WHERE pr_num IS NOT NULL AND pr_flow IS NOT NULL", p![])?;
    let mut noted = 0;
    for t in tasks {
        let f = jloads_obj(t.s("pr_flow"));
        let threads = f.get("rec").and_then(|r| r.get("threads")).and_then(|v| v.as_array()).cloned().unwrap_or_default();
        for r in bots.iter().filter(|r| r.s("project") == t.s("project")) {
            let mark = r.st("bot_mark").to_lowercase();
            for th in &threads {
                let author = th["author"].as_str().unwrap_or("");
                if author.is_empty() || !reviewers::names(r, author) || !th["text"].as_str().unwrap_or("").to_lowercase().contains(&mark) {
                    continue;
                }
                let at = th["last_at"].as_str().and_then(parse_iso).unwrap_or_else(now_ts);
                if at < since {
                    continue;
                }
                if note_run(app, r, at, &format!("{}#{}:{}", t.st("pr_repo"), t.i0("pr_num"), thread_id(th)))? {
                    noted += 1;
                }
            }
        }
    }
    Ok(noted)
}

fn thread_id(th: &Value) -> String {
    th["id"].as_str().map(|s| s.to_string()).unwrap_or_else(|| th["id"].to_string())
}

/// Records one run at `at` unless a run within `bot_run_gap_mins` already covers it, or this comment
/// was counted before. True when it's a new run.
pub fn note_run(app: &App, r: &Row, at: f64, comment: &str) -> Result<bool> {
    if app.db.count("SELECT COUNT(*) FROM reviewer_bot_runs WHERE reviewer_id = ? AND ref = ?", p![r.id(), comment])? > 0 {
        return Ok(false);
    }
    let gap = app.cfg.reviewers.bot_run_gap_mins * 60.0;
    let near = app
        .db
        .q("SELECT at FROM reviewer_bot_runs WHERE reviewer_id = ?", p![r.id()])?
        .iter()
        .filter_map(|x| x.s("at").and_then(parse_iso))
        .find(|x| (x - at).abs() < gap);
    // A comment of a run already seen is kept at that run's time, so it isn't counted again.
    app.db.insert("reviewer_bot_runs", fields!["reviewer_id" => r.id(), "at" => iso(near.unwrap_or(at)), "ref" => comment])?;
    Ok(near.is_none())
}
