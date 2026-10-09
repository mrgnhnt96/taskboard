//! Picking who reviews a PR: `[reviewers] count` people from the project's roster (`reviewers.rs`),
//! one of them a main contributor of the files the PR changes, the rest in turn.
//!
//! - Candidates are the roster's reviewers with a host account, minus the PR's author (and
//!   `[reviewers] me`), anyone removed, and anyone already on the PR. Commit authors from the last
//!   `history_months` with at least `min_commits` commits join the roster on a sync (at most every
//!   `sync_every_hours`, or `tb reviewers sync`), and the host's members give them their account.
//! - One pick is the main contributor: pinned reviewers first, then the `main_contributors` people
//!   with the most commits to the changed files. It's skipped when one of them is already on the PR.
//! - It's turn-based: a reviewer is due at their last ask + (1 + open asks) × `turn_gap_hours` /
//!   weight, and the earliest due goes first (ties: the fewest asks, then pinned, then the most
//!   commits to the changed files, then to the project). Weight is automation level × speed, where
//!   speed comes from the median time they took to review, in work minutes (`speed_by_minutes`,
//!   `slow_speed` past the last step, `no_speed_yet` before their first review; see
//!   `reviewers::median_work_mins`). A reviewer whose bot runs on a known schedule (`botrun.rs`) is
//!   asked only shortly before its next run, at the fastest pace.

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

/// One `git shortlog -sne` line: (commits, name, email).
pub type Line = (i64, String, String);

/// `git shortlog -sne`: (commits, name, email) per author since `since`.
pub fn shortlog(repo: &str, since: &str) -> Vec<Line> {
    let Some(out) = git(repo, &["shortlog", "-sne", &format!("--since={since}"), "HEAD"]) else { return vec![] };
    out.lines()
        .filter_map(|l| {
            let (n, rest) = l.trim().split_once('\t')?;
            let (name, email) = rest.rsplit_once(" <")?;
            Some((n.trim().parse().ok()?, name.trim().to_string(), email.trim_end_matches('>').to_string()))
        })
        .collect()
}

/// The people behind `git shortlog` lines, their commits added up: lines whose email (or noreply
/// login) names the same reviewer are one person, and so are lines with the same author name,
/// except that two work emails under one name stay two people (a noreply email joins the work one
/// when there's just one).
fn authors(app: &App, project: &str, lines: Vec<Line>) -> Result<Vec<Person>> {
    let roster = reviewers::roster(app, project)?;
    let row_of = |email: &str| roster.iter().find(|r| reviewers::names(r, email) || noreply_login(email).is_some_and(|l| reviewers::names(r, &l))).map(|r| r.id());
    Ok(group_authors(lines, row_of))
}

fn group_authors(lines: Vec<Line>, row_of: impl Fn(&str) -> Option<i64>) -> Vec<Person> {
    // (reviewer, lowercased name) → lines.
    let mut by_row: Vec<(i64, Vec<Line>)> = vec![];
    let mut by_name: Vec<(String, Vec<Line>)> = vec![];
    for l in lines {
        if let Some(id) = row_of(&l.2) {
            match by_row.iter_mut().find(|(r, _)| *r == id) {
                Some((_, v)) => v.push(l),
                None => by_row.push((id, vec![l])),
            }
            continue;
        }
        let key = l.1.trim().to_lowercase();
        match by_name.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(l),
            None => by_name.push((key, vec![l])),
        }
    }
    let mut groups: Vec<Vec<Line>> = by_row.into_iter().map(|(_, v)| v).collect();
    for (key, lines) in by_name {
        let (noreply, work): (Vec<_>, Vec<_>) = lines.into_iter().partition(|l| noreply_login(&l.2).is_some());
        // Only noreply lines, under the name of one reviewer already known by their work email.
        let known: Vec<usize> = groups.iter().enumerate().filter(|(_, g)| g.iter().any(|l| l.1.trim().to_lowercase() == key)).map(|(i, _)| i).collect();
        if work.is_empty() && known.len() == 1 {
            groups[known[0]].extend(noreply);
        } else if work.len() == 1 {
            groups.push(work.into_iter().chain(noreply).collect());
        } else {
            groups.extend(work.into_iter().chain(noreply).map(|l| vec![l]));
        }
    }
    groups
        .into_iter()
        .map(|g| {
            let name = g.iter().max_by_key(|l| l.0).map(|l| l.1.clone()).unwrap_or_default();
            Person {
                name,
                host_user: g.iter().find_map(|l| noreply_login(&l.2)),
                emails: g.iter().map(|l| l.2.clone()).collect(),
                source: "git".into(),
                commits: Some(g.iter().map(|l| l.0).sum()),
                ..Default::default()
            }
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
        for p in authors(app, project, shortlog(repo, &format!("{} months ago", cfg.history_months)))? {
            if p.commits.unwrap_or(0) < cfg.min_commits {
                continue;
            }
            let before = reviewers::roster(app, project)?.len();
            app.db.tx(|| reviewers::fold(app, project, &p))?;
            if reviewers::roster(app, project)?.len() > before {
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
    counts.sort_by_key(|c| std::cmp::Reverse(c.1));
    Ok(counts.into_iter().take(cfg.main_contributors).map(|(id, _)| id).collect())
}

/// One reviewer the picker would ask, and why.
#[derive(Debug, Clone)]
pub struct Pick {
    pub reviewer: Row,
    pub user: String,
    pub name: String,
    /// pinned (the main pick, pinned), main (a main contributor of the changed files) or turn.
    pub why: String,
    pub due: f64,
    pub weight: f64,
    /// online, quiet, off or unknown (`presence.rs`).
    pub tier: String,
    /// How many times they've been asked (the first tie-break).
    pub asked: i64,
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
    let pace = if crate::botrun::timed(app, r)? { fastest(cfg) } else { speed(cfg, reviewers::median_work_mins(app, r.id())?) };
    let weight = (r.f("automation").unwrap_or(1.0) * pace).max(0.01);
    Ok((last + (1.0 + open) * cfg.turn_gap_hours * 3600.0 / weight, weight))
}

/// The task's PR flow (`pr_flow`).
fn prflow_of(t: &Row) -> Row {
    jloads_obj(t.s("pr_flow"))
}

/// Who the PR's author is, in every form the board knows: never asked to review their own PR.
fn selves(app: &App, repo: Option<&str>, rec: &Value) -> Vec<String> {
    let mut out: Vec<String> = app.cfg.reviewers.me.iter().chain(app.cfg.owner_emails.iter()).map(|s| s.to_lowercase()).collect();
    if let Some(a) = rec["author"].as_str().filter(|a| !a.is_empty()) {
        out.push(a.to_lowercase());
    }
    if let Some(e) = repo.and_then(|r| git(r, &["config", "user.email"])) {
        out.push(e.trim().to_lowercase());
    }
    out
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
        if !crate::botrun::may_ask(app, &r)? {
            continue;
        }
        let (due, weight) = due(app, &r)?;
        let asked = app.db.count("SELECT COUNT(*) FROM review_asks WHERE reviewer_id = ?", p![r.id()])?;
        cands.push(Pick { name: r.st("name"), user, why: "turn".into(), due, weight, tier: "unknown".into(), asked, reviewer: r });
    }
    let mains = match repo.as_deref() {
        Some(r) => main_contributors(app, &project, r, &changed_files(r, rec))?,
        None => vec![],
    };
    let rank = |c: &Pick| (!c.reviewer.b("pinned"), mains.iter().position(|m| *m == c.reviewer.id()).unwrap_or(usize::MAX), -c.reviewer.i0("commits"), c.reviewer.id());
    cands.sort_by(|a, b| a.due.partial_cmp(&b.due).unwrap_or(std::cmp::Ordering::Equal).then(a.asked.cmp(&b.asked)).then(rank(a).cmp(&rank(b))));
    let mut out: Vec<Pick> = vec![];
    let take = |out: &mut Vec<Pick>, cands: &mut Vec<Pick>, i: usize, why: &str| {
        let mut c = cands.remove(i);
        c.why = why.to_string();
        out.push(c);
    };
    // The main pick: pinned reviewers first, then the changed files' main contributors, each in turn
    // order. None when one of them is already on the PR.
    let on_pr: Vec<Row> = crate::asks::on_pr(&prflow_of(t), rec)
        .iter()
        .filter_map(|w| reviewers::by_host_user(app, &project, &w.user).ok().flatten())
        .collect();
    let have_main = on_pr.iter().any(|r| r.b("pinned") || mains.contains(&r.id()));
    if out.len() < n && !have_main {
        let mut pool: Vec<usize> = cands.iter().enumerate().filter(|(_, c)| c.reviewer.b("pinned")).map(|(i, _)| i).collect();
        pool.extend(cands.iter().enumerate().filter(|(_, c)| !c.reviewer.b("pinned") && mains.contains(&c.reviewer.id())).map(|(i, _)| i));
        if let Some(i) = crate::presence::best(app, &mut cands, &pool)? {
            let why = if cands[i].reviewer.b("pinned") { "pinned" } else { "main" };
            take(&mut out, &mut cands, i, why);
        }
    }
    while out.len() < n && !cands.is_empty() {
        let pool: Vec<usize> = (0..cands.len()).collect();
        match crate::presence::best(app, &mut cands, &pool)? {
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
    fn commits_add_up_per_person() {
        let l = |n: i64, name: &str, email: &str| (n, name.to_string(), email.to_string());
        let got = group_authors(
            vec![l(6, "Ana Lima", "ana@acme.com"), l(4, "Ana Lima", "9+ana-gh@users.noreply.github.com"), l(3, "Bo", "bo@acme.com"), l(3, "Bo", "bo@other.com"), l(2, "Cy", "cy@acme.com"), l(4, "cy", "cy@home.com")],
            |e| (e == "cy@acme.com" || e == "cy@home.com").then_some(7),
        );
        let summary: Vec<(String, Option<String>, i64)> = got.iter().map(|p| (p.name.clone(), p.host_user.clone(), p.commits.unwrap())).collect();
        assert_eq!(
            summary,
            vec![("cy".into(), None, 6), ("Ana Lima".into(), Some("ana-gh".into()), 10), ("Bo".into(), None, 3), ("Bo".into(), None, 3)],
            "one reviewer's emails are one person; one name with two work emails stays two"
        );
    }

    #[test]
    fn reads_a_login_from_a_noreply_email() {
        assert_eq!(noreply_login("123+ana@users.noreply.github.com").as_deref(), Some("ana"));
        assert_eq!(noreply_login("ana@users.noreply.github.com").as_deref(), Some("ana"));
        assert_eq!(noreply_login("ana@acme.com"), None);
    }
}
