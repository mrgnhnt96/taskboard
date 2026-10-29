//! Reviewers who run their own AI review bot every few hours (`tb reviewers bot <who> --every <h>
//! --mark <text>`). The board spots each run from the marker anywhere in that person's comments on
//! the repo's `bot_scan_prs` most recently updated PRs (whoever opened them; read at most every
//! `bot_scan_mins`, `PrHost::recent_comments`; a read that fails is tried again on the next sweep),
//! from the last `bot_window_hours`, and keeps them in `reviewer_bot_runs`. A comment within
//! `bot_run_gap_mins` of another comment of a run is the same run (gap to gap), which starts at its
//! earliest comment, whatever order the host lists them in.
//!
//! A repo a healthy stream relay covers (`feed::relay_back_at`) isn't read on that timer: the relay's
//! comment and review events note the runs as they come ([`note_event`]), and the repo is read once to
//! catch up when the relay connects or comes back from being stuck, then only every `[reviewers]
//! bot_reconcile_secs` (never at 0, the default). A read of a covered repo that fails waits
//! `bot_scan_mins` before it's tried again, so a limited account isn't asked every sweep. While the
//! relay is stuck or silent the repo is read on the `bot_scan_mins` timer again.
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
/// When a covered repo's read last failed, cleared by a good read: `{"<host>:<repo>": iso}`.
const FAILS_SETTING: &str = "bot_scan_fails";

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

/// When this repo was noted in `setting` (`{"<host>:<repo>": iso}`).
fn stamp(app: &App, setting: &str, key: &str) -> Result<Option<f64>> {
    Ok(jloads_obj(app.db.get_setting(setting)?.as_deref()).get(key).and_then(|v| v.as_str()).and_then(parse_iso))
}

/// Notes this repo in `setting` at `at`, or takes it off.
fn set_stamp(app: &App, setting: &str, key: &str, at: Option<&str>) -> Result<()> {
    let mut stamps = jloads_obj(app.db.get_setting(setting)?.as_deref());
    match at {
        Some(at) => {
            stamps.insert(key.to_string(), json!(at));
        }
        None if stamps.remove(key).is_none() => return Ok(()),
        None => {}
    }
    app.db.set_setting(setting, Some(&jdumps(&Value::Object(stamps))))
}

/// Whether this repo's comments are due to be read again. Not covered by a relay: `bot_scan_mins` since
/// the last good read. Covered (`back`: when its relay last connected or came back): once to catch up
/// if the last good read was before that, then every `bot_reconcile_secs` (never at 0); a failed read
/// waits `bot_scan_mins` either way.
fn scan_due(app: &App, key: &str, back: Option<f64>) -> Result<bool> {
    let (now, mins) = (now_ts(), app.cfg.reviewers.bot_scan_mins * 60.0);
    let last = stamp(app, SCANS_SETTING, key)?;
    let Some(back) = back else { return Ok(!last.is_some_and(|l| now - l < mins)) };
    if stamp(app, FAILS_SETTING, key)?.is_some_and(|f| now - f < mins) {
        return Ok(false);
    }
    let every = app.cfg.reviewers.bot_reconcile_secs;
    Ok(match last {
        Some(l) if l >= back => every > 0.0 && now - l >= every.max(mins),
        _ => true,
    })
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
            let key = format!("{host}:{repo}");
            let back = crate::feed::relay_back_at(app, &host, &repo);
            if !scan_due(app, &key, back)? {
                continue;
            }
            let started = now_iso();
            let mut comments = match crate::prhost::host_for(app, &host).and_then(|h| h.recent_comments(&repo, cfg.bot_scan_prs)) {
                Ok(c) => c,
                Err(e) => {
                    app.info(format!("reviewers: couldn't read {repo}'s comments for bot runs: {e}"));
                    if back.is_some() {
                        set_stamp(app, FAILS_SETTING, &key, Some(&started))?;
                    }
                    continue;
                }
            };
            set_stamp(app, SCANS_SETTING, &key, Some(&started))?;
            set_stamp(app, FAILS_SETTING, &key, None)?;
            // Oldest first, so each run starts at its earliest comment (Bitbucket lists the newest first).
            comments.sort_by(|a, b| parse_iso(&a.at).unwrap_or(0.0).total_cmp(&parse_iso(&b.at).unwrap_or(0.0)));
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

/// Notes the bot run a stream comment or review event shows (`pr`, `comment_id` or the event's `id`,
/// `author` or `who`, `body`, `at`): its author is a reviewer with a bot in a project whose PRs are in
/// that repo, and its body has their marker. True when it's a new run.
pub fn note_event(app: &App, ev: &Value) -> Result<bool> {
    let pr = &ev["pr"];
    let (host, repo) = (body_str(pr, "provider").to_lowercase(), body_str(pr, "repo"));
    let author = [body_str(ev, "author"), body_str(ev, "who")].into_iter().find(|a| !a.is_empty()).unwrap_or_default();
    let text = body_str(ev, "body").to_lowercase();
    let Some(at) = ev["at"].as_str().and_then(parse_iso) else { return Ok(false) };
    if repo.is_empty() || author.is_empty() || text.is_empty() || at < now_ts() - app.cfg.reviewers.bot_window_hours * 3600.0 {
        return Ok(false);
    }
    let bots = app.db.q(
        "SELECT * FROM reviewers WHERE bot_every_h IS NOT NULL AND bot_mark IS NOT NULL AND bot_mark != '' \
         AND project IN (SELECT project FROM tasks WHERE pr_host = ? AND pr_repo = ?)",
        p![host, repo],
    )?;
    let id = [body_str(ev, "comment_id"), body_str(ev, "id")].into_iter().find(|i| !i.is_empty()).unwrap_or_default();
    let comment = format!("{repo}#{}:{id}", pr["num"].as_i64().unwrap_or(0));
    let mut new = false;
    for r in bots.iter().filter(|r| has_bot(r) && reviewers::names(r, &author) && text.contains(&r.st("bot_mark").to_lowercase())) {
        new |= note_run(app, r, at, &comment)?;
    }
    Ok(new)
}

/// Records one run at `at` unless this comment was counted before, or it's within `bot_run_gap_mins`
/// of a comment of a run already seen: comments chain gap to gap, so a slow run that keeps commenting
/// stays one run. A comment earlier than the run it's part of moves the run back to it (a run starts at
/// its earliest comment), and one that bridges two runs makes them one. True when it's a new run.
pub fn note_run(app: &App, r: &Row, at: f64, comment: &str) -> Result<bool> {
    if app.db.count("SELECT COUNT(*) FROM reviewer_bot_runs WHERE reviewer_id = ? AND ref = ?", p![r.id(), comment])? > 0 {
        return Ok(false);
    }
    let gap = app.cfg.reviewers.bot_run_gap_mins * 60.0;
    // The runs (by start) with a comment this close; a row from before `comment_at` is its run's start.
    let mut near: Vec<String> = app
        .db
        .q("SELECT at, COALESCE(comment_at, at) AS c FROM reviewer_bot_runs WHERE reviewer_id = ?", p![r.id()])?
        .iter()
        .filter(|x| x.s("c").and_then(parse_iso).is_some_and(|c| (c - at).abs() < gap))
        .map(|x| x.st("at"))
        .collect();
    near.sort();
    near.dedup();
    let start = near.iter().filter_map(|n| parse_iso(n)).fold(at, f64::min);
    // A comment of a run already seen is kept at that run's time, so it isn't counted again.
    for n in &near {
        if parse_iso(n) != Some(start) {
            app.db.x("UPDATE reviewer_bot_runs SET at = ? WHERE reviewer_id = ? AND at = ?", p![iso(start), r.id(), n])?;
        }
    }
    app.db.insert("reviewer_bot_runs", fields!["reviewer_id" => r.id(), "at" => iso(start), "ref" => comment, "comment_at" => iso(at)])?;
    Ok(near.is_empty())
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
