//! Picking who reviews a PR: `[reviewers] count` people from the project's roster (`reviewers.rs`),
//! one of them a main contributor of the files the PR changes, the rest in turn.
//!
//! - Candidates are the roster's reviewers with a host account, minus the PR's author (and
//!   `[reviewers] me`), anyone removed, and anyone already on the PR. Commit authors from the last
//!   `history_months` with at least `min_commits` commits join the roster on a sync (at most every
//!   `sync_every_hours`, or `tb reviewers sync`), and the host's members give them their account.
//! - Pinned reviewers are always asked.
//! - One pick comes from the `main_contributors` people with the most commits to the changed files.
//! - It's turn-based: a reviewer is due at their last ask + open asks × `turn_gap_hours` / weight, and
//!   the earliest due goes first. Weight is automation level × speed, where speed comes from the
//!   median time they took to review, in work minutes (`speed_by_minutes`, `slow_speed` past the
//!   last step, `no_speed_yet` before their first review).

use std::path::Path;

use chrono::Duration;
use serde_json::{json, Value};

use crate::app::App;
use crate::reviewers::{self, Person};
use crate::util::*;
use crate::{hours, p};

/// Work minutes from `a` to `b` (Unix seconds): minutes inside the work hours, or every minute with
/// the hours off.
pub fn work_minutes(app: &App, a: f64, b: f64) -> f64 {
    if b <= a {
        return 0.0;
    }
    let h = hours::get(app);
    if !h.on {
        return (b - a) / 60.0;
    }
    // Minute by minute, capped at 30 days: plenty for a review.
    let start = local_dt(a).naive_local();
    let mins = (((b - a) / 60.0).floor() as i64).min(30 * 24 * 60);
    (0..mins).filter(|m| hours::within(&h, &(start + Duration::minutes(*m)))).count() as f64
}

/// Speed from a median review time in work minutes.
pub fn speed(cfg: &crate::reviewers::ReviewersConfig, median: Option<f64>) -> f64 {
    let Some(m) = median else { return cfg.no_speed_yet };
    for (upto, s) in &cfg.speed_by_minutes {
        if m <= *upto {
            return *s;
        }
    }
    cfg.slow_speed
}

/// The fastest pace the table gives (a timed bot's).
pub fn fastest(cfg: &crate::reviewers::ReviewersConfig) -> f64 {
    cfg.speed_by_minutes.iter().map(|(_, s)| *s).fold(cfg.no_speed_yet, f64::max)
}

fn git(repo: &str, args: &[&str]) -> Option<String> {
    let git = crate::proc::which("git")?;
    let mut a: Vec<String> = vec!["-C".into(), repo.into()];
    a.extend(args.iter().map(|s| s.to_string()));
    let out = crate::proc::run(&git, &a, None, 20.0).ok()?;
    (out.code == Some(0)).then_some(out.stdout)
}

/// A GitHub login from a `123+login@users.noreply.github.com` commit email.
fn noreply_login(email: &str) -> Option<String> {
    let local = email.strip_suffix("@users.noreply.github.com")?;
    Some(local.split_once('+').map(|(_, l)| l).unwrap_or(local).to_string())
}

/// `git shortlog -sne`: (commits, name, email) per author since `since`.
pub fn shortlog(repo: &str, since: &str) -> Vec<(i64, String, String)> {
    let Some(out) = git(repo, &["shortlog", "-sne", &format!("--since={since}"), "HEAD"]) else { return vec![] };
    out.lines()
        .filter_map(|l| {
            let (n, rest) = l.trim().split_once('\t')?;
            let (name, email) = rest.rsplit_once(" <")?;
            Some((n.trim().parse().ok()?, name.trim().to_string(), email.trim_end_matches('>').to_string()))
        })
        .collect()
}

/// Joins commit authors with enough commits to the project's roster, and matches the host's members
/// to reviewers without an account. Runs at most every `sync_every_hours` unless `force`.
pub fn sync(app: &App, project: &str, repo: Option<&str>, pr: Option<&crate::prhost::PrRef>, force: bool) -> Result<Value> {
    let cfg = &app.cfg.reviewers;
    let key = format!("reviewers_synced:{project}");
    let last = app.db.get_setting(&key)?.and_then(|s| parse_iso(&s));
    if !force && last.is_some_and(|l| now_ts() - l < cfg.sync_every_hours * 3600.0) {
        return Ok(json!({"skipped": true}));
    }
    let mut joined = 0;
    if let Some(repo) = repo.filter(|r| Path::new(r).is_dir()) {
        for (n, name, email) in shortlog(repo, &format!("{} months ago", cfg.history_months)) {
            if n < cfg.min_commits {
                continue;
            }
            let before = reviewers::find(app, project, &email)?.or(reviewers::find(app, project, &name)?);
            let p = Person { name: name.clone(), host_user: noreply_login(&email), emails: vec![email], source: "git".into(), commits: Some(n), ..Default::default() };
            app.db.tx(|| reviewers::fold(app, project, &p))?;
            if before.is_none() {
                joined += 1;
            }
        }
    }
    let mut matched = 0;
    if let Some(pr) = pr {
        if let Ok(host) = crate::prhost::host_for(app, &pr.host) {
            match host.members(&pr.repo) {
                Ok(members) => {
                    for m in members {
                        for r in reviewers::roster(app, project)? {
                            if has(r.s("host_user")) {
                                continue;
                            }
                            if reviewers::names(&r, &m.name) || reviewers::names(&r, &m.user) {
                                app.db.update("reviewers", &json!(r.id()), crate::fields!["host_user" => m.user.clone(), "updated_at" => now_iso()])?;
                                matched += 1;
                            }
                        }
                    }
                }
                Err(e) => app.info(format!("reviewers: couldn't list {}'s members: {e}", pr.repo)),
            }
        }
    }
    app.db.set_setting(&key, Some(&now_iso()))?;
    Ok(json!({"joined": joined, "matched": matched}))
}

/// The files the PR changes (empty when git can't tell).
fn changed_files(repo: &str, rec: &Value) -> Vec<String> {
    let (base, head) = (rec["base_head"].as_str().unwrap_or(""), rec["head"].as_str().unwrap_or(""));
    let mut out = None;
    if !base.is_empty() && !head.is_empty() {
        out = git(repo, &["diff", "--name-only", &format!("{base}...{head}")]);
    }
    if out.is_none() {
        let (b, br) = (rec["base"].as_str().unwrap_or(""), rec["branch"].as_str().unwrap_or(""));
        if !b.is_empty() && !br.is_empty() {
            out = git(repo, &["diff", "--name-only", &format!("origin/{b}...origin/{br}")]).or_else(|| git(repo, &["diff", "--name-only", &format!("{b}...{br}")]));
        }
    }
    out.unwrap_or_default().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).take(200).collect()
}

/// The roster ids of the people with the most commits to `files` (most first).
fn main_contributors(app: &App, project: &str, repo: &str, files: &[String]) -> Result<Vec<i64>> {
    let cfg = &app.cfg.reviewers;
    if files.is_empty() || cfg.main_contributors == 0 {
        return Ok(vec![]);
    }
    let mut args: Vec<String> = vec!["log".into(), format!("--since={} months ago", cfg.history_months), "--format=%ae%x09%an".into(), "--".into()];
    args.extend(files.iter().cloned());
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let Some(out) = git(repo, &refs) else { return Ok(vec![]) };
    let mut counts: Vec<(i64, usize)> = vec![];
    let roster = reviewers::roster(app, project)?;
    for l in out.lines() {
        let (email, name) = l.split_once('\t').unwrap_or((l, ""));
        let Some(r) = roster.iter().find(|r| reviewers::names(r, email) || (!name.is_empty() && reviewers::names(r, name))) else { continue };
        match counts.iter_mut().find(|(id, _)| *id == r.id()) {
            Some((_, n)) => *n += 1,
            None => counts.push((r.id(), 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(counts.into_iter().take(cfg.main_contributors).map(|(id, _)| id).collect())
}

/// One reviewer the picker would ask, and why.
#[derive(Debug, Clone)]
pub struct Pick {
    pub reviewer: Row,
    pub user: String,
    pub name: String,
    /// pinned, main (a main contributor of the changed files) or turn.
    pub why: String,
    pub due: f64,
    pub weight: f64,
    /// online, quiet, off or unknown (`presence.rs`).
    pub tier: String,
}

impl Pick {
    pub fn to_value(&self) -> Value {
        json!({"user": self.user, "name": self.name, "why": self.why, "due": iso(self.due), "weight": self.weight, "tier": self.tier})
    }
}

/// When a reviewer is next due, and their weight.
pub fn due(app: &App, r: &Row) -> Result<(f64, f64)> {
    let cfg = &app.cfg.reviewers;
    let last = app.db.val("SELECT MAX(asked_at) FROM review_asks WHERE reviewer_id = ?", p![r.id()])?;
    let last = last.as_str().and_then(parse_iso).unwrap_or(0.0);
    let open = app.db.count("SELECT COUNT(*) FROM review_asks WHERE reviewer_id = ? AND state = 'open'", p![r.id()])? as f64;
    let pace = speed(cfg, reviewers::median_work_mins(app, r.id())?);
    let weight = (r.f("automation").unwrap_or(1.0) * pace).max(0.01);
    Ok((last + open * cfg.turn_gap_hours * 3600.0 / weight, weight))
}

/// Who the PR's author is, in every form the board knows: never asked to review their own PR.
fn selves(app: &App, repo: Option<&str>, rec: &Value) -> Vec<String> {
    let mut out: Vec<String> = app.cfg.reviewers.me.iter().map(|s| s.to_lowercase()).collect();
    if let Some(a) = rec["author"].as_str().filter(|a| !a.is_empty()) {
        out.push(a.to_lowercase());
    }
    if let Some(e) = repo.and_then(|r| git(r, &["config", "user.email"])) {
        out.push(e.trim().to_lowercase());
    }
    out
}

/// The candidate to take from `pool` (indexes into the candidates, earliest due first).
fn best(pool: &[usize]) -> Option<usize> {
    pool.first().copied()
}

/// Picks up to `n` reviewers for a task's PR (fewer when the roster runs out), skipping `skip`
/// (host ids already on the PR or swapped off).
pub fn pick(app: &App, t: &Row, rec: &Value, n: usize, skip: &[String]) -> Result<Vec<Pick>> {
    let project = t.st("project");
    let repo = crate::runner::task_cwd(app, t)?;
    let pr = crate::prhost::PrRef::of(t);
    sync(app, &project, repo.as_deref(), pr.as_ref(), false)?;
    let me = selves(app, repo.as_deref(), rec);
    let skip: Vec<String> = skip.iter().map(|s| s.to_lowercase()).collect();
    let mut cands: Vec<Pick> = vec![];
    for r in reviewers::roster(app, &project)? {
        let Some(user) = r.s("host_user").filter(|u| !u.is_empty()).map(|s| s.to_string()) else { continue };
        if r.s("removed_at").is_some() || skip.contains(&user.to_lowercase()) || reviewers::idents(&r).iter().any(|i| me.contains(i)) {
            continue;
        }
        let (due, weight) = due(app, &r)?;
        cands.push(Pick { name: r.st("name"), user, why: "turn".into(), due, weight, tier: "unknown".into(), reviewer: r });
    }
    cands.sort_by(|a, b| a.due.partial_cmp(&b.due).unwrap_or(std::cmp::Ordering::Equal).then(a.reviewer.id().cmp(&b.reviewer.id())));
    let mut out: Vec<Pick> = vec![];
    let take = |out: &mut Vec<Pick>, cands: &mut Vec<Pick>, i: usize, why: &str| {
        let mut c = cands.remove(i);
        c.why = why.to_string();
        out.push(c);
    };
    while let Some(i) = cands.iter().position(|c| c.reviewer.b("pinned")) {
        take(&mut out, &mut cands, i, "pinned");
    }
    let mains = match repo.as_deref() {
        Some(r) => main_contributors(app, &project, r, &changed_files(r, rec))?,
        None => vec![],
    };
    let have_main = out.iter().any(|p| mains.contains(&p.reviewer.id()));
    if out.len() < n && !have_main {
        let pool: Vec<usize> = cands.iter().enumerate().filter(|(_, c)| mains.contains(&c.reviewer.id())).map(|(i, _)| i).collect();
        if let Some(i) = best(&pool) {
            take(&mut out, &mut cands, i, "main");
        }
    }
    while out.len() < n && !cands.is_empty() {
        let pool: Vec<usize> = (0..cands.len()).collect();
        match best(&pool) {
            Some(i) => take(&mut out, &mut cands, i, "turn"),
            None => break,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_follows_the_table() {
        let cfg = crate::reviewers::ReviewersConfig::default();
        assert_eq!(speed(&cfg, None), cfg.no_speed_yet);
        assert_eq!(speed(&cfg, Some(10.0)), cfg.speed_by_minutes[0].1);
        assert_eq!(speed(&cfg, Some(100_000.0)), cfg.slow_speed);
        assert!(fastest(&cfg) >= speed(&cfg, Some(1.0)));
    }

    #[test]
    fn reads_a_login_from_a_noreply_email() {
        assert_eq!(noreply_login("123+ana@users.noreply.github.com").as_deref(), Some("ana"));
        assert_eq!(noreply_login("ana@users.noreply.github.com").as_deref(), Some("ana"));
        assert_eq!(noreply_login("ana@acme.com"), None);
    }
}
