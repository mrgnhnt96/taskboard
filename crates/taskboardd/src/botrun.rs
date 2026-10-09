//! Reviewers who run their own AI review bot every few hours (`tb reviewers bot <who> --every <h>
//! --mark <text>`). The board spots each run from the marker anywhere in that person's comments on
//! the repo's `bot_scan_prs` most recently updated PRs (whoever opened them; read at most every
//! `bot_scan_mins`, `PrHost::recent_comments`), from the last `bot_window_hours`, at each comment's
//! own time (comments within `bot_run_gap_mins` of a run are the same run), and keeps them in
//! `reviewer_bot_runs`.
//!
//! Once a run has been seen, the bot is timed: its next run is the last one plus the interval, rolled
//! forward by the interval until it's in the future; the picker asks that person only when the next run
//! is at most `bot_due_mins` away, and their pace is the fastest the speed table gives. A reviewer
//! whose bot hasn't been seen yet isn't asked until it has.

use serde_json::{json, Value};

use crate::app::App;
use crate::reviewers;
use crate::util::*;
use crate::{fields, p};

/// When each repo's comments were last read for bot runs: `{"<host>:<repo>": iso}`.
const SCANS_SETTING: &str = "bot_scans";

/// The last run seen, as Unix seconds.
pub fn last_run(app: &App, r: &Row) -> Result<Option<f64>> {
    Ok(app.db.val("SELECT MAX(at) FROM reviewer_bot_runs WHERE reviewer_id = ?", p![r.id()])?.as_str().and_then(parse_iso))
}

/// The reviewer runs a review bot (`tb reviewers bot`).
pub fn has_bot(r: &Row) -> bool {
    r.f("bot_every_h").is_some_and(|h| h > 0.0) && r.s("bot_mark").is_some_and(|m| !m.is_empty())
}

/// The first run after `now`: the last one plus the interval, rolled forward by it until it's later.
pub fn roll_forward(last: f64, every_secs: f64, now: f64) -> f64 {
    if every_secs <= 0.0 {
        return last;
    }
    let mut next = last + every_secs;
    if next <= now {
        next += ((now - next) / every_secs).floor() * every_secs;
        if next <= now {
            next += every_secs;
        }
    }
    next
}

/// When their bot runs next, once a run has been seen.
pub fn next_run(app: &App, r: &Row) -> Result<Option<f64>> {
    if !has_bot(r) {
        return Ok(None);
    }
    let Some(last) = last_run(app, r)? else { return Ok(None) };
    Ok(Some(roll_forward(last, r.f("bot_every_h").unwrap_or(0.0) * 3600.0, now_ts())))
}

/// Their bot runs on a known schedule.
pub fn timed(app: &App, r: &Row) -> Result<bool> {
    Ok(next_run(app, r)?.is_some())
}

/// Whether the picker may ask them now: they have no bot, or their bot has been seen and runs within
/// `bot_due_mins`.
pub fn may_ask(app: &App, r: &Row) -> Result<bool> {
    if !has_bot(r) {
        return Ok(true);
    }
    Ok(match next_run(app, r)? {
        None => false,
        Some(next) => next - now_ts() <= app.cfg.reviewers.bot_due_mins * 60.0,
    })
}

/// The repos to read for a project's bots: every watched repo its tasks' PRs are in.
fn repos(app: &App, project: &str) -> Result<Vec<(String, String)>> {
    Ok(app
        .db
        .q(
            "SELECT pr_host, pr_repo, MAX(id) AS last FROM tasks WHERE project = ? AND pr_repo IS NOT NULL AND pr_repo != '' \
             AND pr_host IN ('github', 'bitbucket') GROUP BY pr_host, pr_repo ORDER BY last DESC",
            p![project],
        )?
        .iter()
        .map(|r| (r.st("pr_host"), r.st("pr_repo")))
        .collect())
}

/// Whether this repo's comments are due to be read again (`bot_scan_mins`), noting the read when
/// they are.
fn scan_due(app: &App, key: &str) -> Result<bool> {
    let mut scans = jloads_obj(app.db.get_setting(SCANS_SETTING)?.as_deref());
    let last = scans.get(key).and_then(|v| v.as_str()).and_then(parse_iso);
    if last.is_some_and(|l| now_ts() - l < app.cfg.reviewers.bot_scan_mins * 60.0) {
        return Ok(false);
    }
    scans.insert(key.to_string(), json!(now_iso()));
    app.db.set_setting(SCANS_SETTING, Some(&jdumps(&Value::Object(scans))))?;
    Ok(true)
}

/// Notes the bot runs shown by marked comments on the repos' recent PRs. Reads the hosts outside any
/// database transaction.
pub fn note_runs(app: &App) -> Result<i64> {
    let bots: Vec<Row> = app.db.q("SELECT * FROM reviewers WHERE bot_every_h IS NOT NULL AND bot_mark IS NOT NULL AND bot_mark != ''", p![])?.into_iter().filter(has_bot).collect();
    let mut projects: Vec<String> = bots.iter().map(|r| r.st("project")).collect();
    projects.sort();
    projects.dedup();
    let cfg = &app.cfg.reviewers;
    let since = now_ts() - cfg.bot_window_hours * 3600.0;
    let mut noted = 0;
    for project in projects {
        let mine: Vec<&Row> = bots.iter().filter(|r| r.st("project") == project).collect();
        for (host, repo) in repos(app, &project)? {
            if !scan_due(app, &format!("{host}:{repo}"))? {
                continue;
            }
            let comments = match crate::prhost::host_for(app, &host).and_then(|h| h.recent_comments(&repo, cfg.bot_scan_prs)) {
                Ok(c) => c,
                Err(e) => {
                    app.info(format!("reviewers: couldn't read {repo}'s comments for bot runs: {e}"));
                    continue;
                }
            };
            noted += app.db.tx(|| {
                let mut n = 0;
                for c in &comments {
                    let Some(at) = parse_iso(&c.at) else { continue };
                    if at < since {
                        continue;
                    }
                    let text = c.text.to_lowercase();
                    for r in mine.iter().filter(|r| reviewers::names(r, &c.author) && text.contains(&r.st("bot_mark").to_lowercase())) {
                        if note_run(app, r, at, &format!("{repo}#{}:{}", c.pr, c.id))? {
                            n += 1;
                        }
                    }
                }
                Ok(n)
            })?;
        }
    }
    Ok(noted)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_overdue_run_rolls_forward_into_the_future() {
        let h = 3600.0;
        assert_eq!(roll_forward(0.0, 4.0 * h, 1.0 * h), 4.0 * h, "not due yet: last + interval");
        assert_eq!(roll_forward(0.0, 4.0 * h, 9.0 * h), 12.0 * h, "two runs missed: the next one after now");
        assert_eq!(roll_forward(0.0, 4.0 * h, 8.0 * h), 12.0 * h, "exactly at a run: the one after");
    }
}
