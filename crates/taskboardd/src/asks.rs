//! Asking for reviews: `tb pr reviewers`, which sets a PR's reviewers through its host
//! (`prhost.rs`) and records each ask in the ledger (`review_asks`, see `reviewers.rs`).
//!
//! Like `prcmds.rs`, it reads the PR fresh first and never calls the host inside a database
//! transaction.

use serde_json::{json, Value};

use crate::app::App;
use crate::prhost::{self, PrRef};
use crate::reviewers;
use crate::util::*;
use crate::{board, fields, prflow};

fn flow(t: &Row) -> Row {
    jloads_obj(t.s("pr_flow"))
}

fn pr_of(t: &Row) -> Result<PrRef> {
    match PrRef::of(t) {
        Some(p) => Ok(p),
        None => err(404, format!("{} has no PR linked.", rf("task", t.id()))),
    }
}

fn host(app: &App, pr: &PrRef) -> Result<std::sync::Arc<dyn prhost::PrHost>> {
    prhost::host_for(app, &pr.host).map_err(|e| ApiError::new(409, format!("Can't work with PR #{}: {e}.", pr.num)))
}

fn who_of(body: &Value) -> String {
    let w = body_str(body, "who");
    if w.is_empty() { "the owner".to_string() } else { w }
}

/// Someone to ask or take off: (host id, name).
#[derive(Debug, Clone, PartialEq)]
pub struct Who {
    pub user: String,
    pub name: String,
}

/// Who `who` is on this PR: a roster reviewer (by any of their names), else someone on the PR's
/// reviewer list, else a host id as given.
pub fn resolve(app: &App, t: &Row, rec: &Value, who: &str) -> Result<Who> {
    let who = who.trim();
    if let Some(r) = reviewers::find(app, &t.st("project"), who)? {
        let Some(user) = r.s("host_user").filter(|u| !u.is_empty()) else {
            return err(409, format!("The board doesn't know {}'s account on the PR host yet. Add it: tb reviewers add \"{}\" --user <their id>.", r.st("name"), r.st("name")));
        };
        return Ok(Who { user: user.to_string(), name: r.st("name") });
    }
    let w = who.trim_start_matches('@').to_lowercase();
    for r in rec["reviewers"].as_array().cloned().unwrap_or_default() {
        if r["user"].as_str().map(|u| u.to_lowercase()) == Some(w.clone()) || r["name"].as_str().map(|u| u.to_lowercase()) == Some(w.clone()) {
            return Ok(Who { user: r["user"].as_str().unwrap_or("").to_string(), name: r["name"].as_str().unwrap_or("").to_string() });
        }
    }
    Ok(Who { user: who.trim_start_matches('@').to_string(), name: who.trim_start_matches('@').to_string() })
}

/// Refuses asking the PR's author or someone removed from the roster.
fn may_ask(app: &App, t: &Row, rec: &Value, w: &Who) -> Result<()> {
    let author = rec["author"].as_str().unwrap_or("");
    if !author.is_empty() && author.eq_ignore_ascii_case(&w.user) {
        return err(409, format!("{} wrote this PR, so they can't review it.", w.name));
    }
    if let Some(r) = reviewers::by_host_user(app, &t.st("project"), &w.user)? {
        if r.s("removed_at").is_some() {
            let why = r.s("removed_why").map(|y| format!(" ({y})")).unwrap_or_default();
            return err(409, format!("{} is off {}'s reviewers{why}: the board never asks them. tb reviewers back \"{}\" puts them back.", r.st("name"), t.st("project"), r.st("name")));
        }
    }
    Ok(())
}

/// The reviewers asked on the PR right now (requested, not swapped off).
pub fn on_pr(f: &Row, rec: &Value) -> Vec<Who> {
    let off = prflow::swapped_off(f);
    rec["reviewers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|r| r["requested"] == true || r["state"] != "pending")
        .filter(|r| !off.iter().any(|o| r["user"].as_str() == Some(o.as_str())))
        .map(|r| Who { user: r["user"].as_str().unwrap_or("").to_string(), name: r["name"].as_str().unwrap_or("").to_string() })
        .collect()
}

/// Adds `user` to the PR flow's `swapped_off` (their request for changes no longer holds).
pub fn swap_off(app: &App, id: i64, user: &str) -> Result<()> {
    let t = board::get_task(app, id)?;
    let mut off = prflow::swapped_off(&flow(&t));
    if !off.iter().any(|o| o == user) {
        off.push(user.to_string());
    }
    prflow::merge_flow(app, id, fields!["swapped_off" => off]).map(|_| ())
}

/// Takes `user` out of `swapped_off` (they came back: their review counts again).
pub fn swap_on(app: &App, id: i64, user: &str) -> Result<()> {
    let t = board::get_task(app, id)?;
    let off: Vec<String> = prflow::swapped_off(&flow(&t)).into_iter().filter(|o| o != user).collect();
    prflow::merge_flow(app, id, fields!["swapped_off" => off]).map(|_| ())
}

/// Notes on the PR flow who was asked last (`asked`: when, who, by whom), which the app shows and
/// the `ask` stage waits for.
pub fn note_asked(app: &App, id: i64, names: &[String], by: &str) -> Result<()> {
    prflow::merge_flow(app, id, fields!["asked" => json!({"at": now_iso(), "names": names, "by": by}), "ask_tries" => null, "ask_retry_at" => null])
        .map(|_| ())
}

/// Host ids the picker must skip on this PR: everyone on it now, everyone swapped off, everyone
/// who already reviewed it.
pub fn skip_list(f: &Row, rec: &Value) -> Vec<String> {
    let mut out: Vec<String> = rec["reviewers"].as_array().cloned().unwrap_or_default().iter().filter_map(|r| r["user"].as_str().map(|s| s.to_string())).collect();
    out.extend(prflow::swapped_off(f));
    out
}

/// Picks `n` reviewers for the PR (`picker.rs`), skipping `skip` too.
pub fn pick(app: &App, t: &Row, rec: &Value, n: usize, skip: &[String]) -> Result<Vec<(Who, String)>> {
    let mut all = skip_list(&flow(t), rec);
    all.extend(skip.iter().cloned());
    Ok(crate::picker::pick(app, t, rec, n, &all)?.into_iter().map(|p| (Who { user: p.user, name: p.name }, p.why)).collect())
}

/// How many more reviewers the PR needs to have `count` asked (not counting anyone swapped off).
pub fn wanted(app: &App, t: &Row, rec: &Value, count: Option<usize>) -> usize {
    let n = count.unwrap_or(app.cfg.reviewers.count);
    if count.is_some() {
        return n;
    }
    n.saturating_sub(on_pr(&flow(t), rec).len())
}

/// Why asking this PR's reviewers waits for the PR feed (`feed::holding`), if it does: the PR's
/// first ask goes out when the feed is merely quiet outside the work hours, later ones don't.
pub fn held(app: &App, t: &Row) -> Result<Option<String>> {
    let first = app.db.count("SELECT COUNT(*) FROM review_asks WHERE task_id = ?", crate::p![t.id()])? == 0;
    Ok(crate::feed::holding(app, first))
}

/// Whether the owner's review gates this ask (the `ask` stage): it's on for the project, the owner
/// hasn't reviewed the PR, and nobody has been asked on it yet. Whatever the task's status, so a
/// reopened task waits too; a later swap or drop doesn't.
pub fn needs_owner_review(app: &App, t: &Row) -> Result<bool> {
    if !reviewers::ask_stage_on(app, t.s("project")) {
        return Ok(false);
    }
    let f = flow(t);
    if f.contains_key("reviewed") || f.contains_key("asked") {
        return Ok(false);
    }
    Ok(app.db.count("SELECT COUNT(*) FROM review_asks WHERE task_id = ?", crate::p![t.id()])? == 0)
}

/// Whether this PR's reviewers may be asked now (`tb pr reviewers`, besides `--dry-run`): not while
/// the PR feed is holding (`feed::holding`; a PR's first ask goes out outside the feed's hours), and
/// with the `ask` stage on, a PR's first ask (`adding` someone) not before the owner has reviewed it.
fn may_ask_now(app: &App, t: &Row, adding: bool) -> Result<()> {
    if let Some(why) = held(app, t)? {
        return err(409, format!("{why} Try again once tb feed says it's healthy."));
    }
    if adding && needs_owner_review(app, t)? {
        return err(409, format!("{} hasn't reviewed PR #{} yet: reviewers are asked after that (the ask stage).", app.cfg.owner, t.i0("pr_num")));
    }
    Ok(())
}

/// `tb pr reviewers [--ask WHO…] [--replace X [--with Y]] [--drop X] [--count N] [--dry-run]`. With
/// none of `ask`, `replace` and `drop`, the picker chooses (`count`, else enough for `[reviewers]
/// count`).
pub fn pr_reviewers(app: &App, id: i64, body: &Value) -> Result<Value> {
    let t = board::get_task(app, id)?;
    let pr = pr_of(&t)?;
    let rec = match prflow::refresh_task(app, id)? {
        Ok(r) => r,
        Err(e) => return err(502, format!("Couldn't read PR #{}: {e}.", pr.num)),
    };
    let t = board::get_task(app, id)?;
    let by = who_of(body);
    let fail = |what: &str, e: String| ApiError::new(502, format!("Couldn't {what} on PR #{}: {e}.", pr.num));
    let drop = body_str(body, "drop");
    let replace = body_str(body, "replace");
    let ask = str_list(body.get("ask"));
    let count = body["count"].as_u64().map(|n| n as usize).or_else(|| body_str(body, "count").parse().ok());
    let dry = as_bool(body.get("dry_run"), false);
    let why = Some(body_str(body, "why")).filter(|w| !w.is_empty());
    let picking = drop.is_empty() && replace.is_empty() && ask.is_empty();
    if picking && dry {
        let picks = pick(app, &t, &rec, wanted(app, &t, &rec, count), &[])?;
        return Ok(json!({"task": rf("task", id), "dry_run": true,
                         "picks": picks.iter().map(|(w, why)| json!({"user": w.user, "name": w.name, "why": why})).collect::<Vec<_>>()}));
    }
    may_ask_now(app, &t, picking || !ask.is_empty())?;
    let h = host(app, &pr)?;
    let mut asked: Vec<(Who, String)> = vec![];
    let mut dropped: Vec<Who> = vec![];
    let mut replaced: Option<(Who, Who)> = None;
    if !drop.is_empty() {
        let w = resolve(app, &t, &rec, &drop)?;
        h.remove_reviewers(&pr, std::slice::from_ref(&w.user)).map_err(|e| fail("take a reviewer off", e))?;
        dropped.push(w);
    }
    if !replace.is_empty() {
        let old = resolve(app, &t, &rec, &replace)?;
        let with = body_str(body, "with");
        let new = if with.is_empty() {
            match pick(app, &t, &rec, 1, std::slice::from_ref(&old.user))?.into_iter().next() {
                Some((w, _)) => w,
                None => return err(409, format!("Nobody else on {}'s roster can stand in for {}. Name someone with --with.", t.st("project"), old.name)),
            }
        } else {
            resolve(app, &t, &rec, &with)?
        };
        may_ask(app, &t, &rec, &new)?;
        h.replace_reviewer(&pr, &old.user, &new.user).map_err(|e| fail("swap the reviewer", e))?;
        replaced = Some((old, new));
    }
    let mut want: Vec<(Who, String)> = vec![];
    for a in &ask {
        let w = resolve(app, &t, &rec, a)?;
        may_ask(app, &t, &rec, &w)?;
        if !want.iter().any(|(x, _)| x.user == w.user) {
            want.push((w, why.clone().unwrap_or_else(|| "ask".into())));
        }
    }
    if picking {
        let n = wanted(app, &t, &rec, count);
        want = pick(app, &t, &rec, n, &[])?.into_iter().map(|(w, _)| (w, why.clone().unwrap_or_else(|| "pick".into()))).collect();
        if want.is_empty() && n > 0 && on_pr(&flow(&t), &rec).is_empty() {
            return err(409, format!("Nobody on {}'s roster can review this PR: add reviewers with tb reviewers add, or name one with --ask.", t.st("project")));
        }
    }
    if !want.is_empty() {
        let users: Vec<String> = want.iter().map(|(w, _)| w.user.clone()).collect();
        h.request_reviews(&pr, &users).map_err(|e| fail("ask for reviews", e))?;
        asked.extend(want);
    }
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        let num = pr.num;
        for w in &dropped {
            if let Some(a) = reviewers::open_ask(app, id, &w.user)? {
                reviewers::close_ask(app, a.id(), "dropped")?;
            }
            swap_off(app, id, &w.user)?;
            reviewers::note(app, &t, &by, &format!("Took {} off PR #{num}'s reviewers", w.name))?;
        }
        if let Some((old, new)) = &replaced {
            let prior = reviewers::open_ask(app, id, &old.user)?;
            if let Some(a) = &prior {
                reviewers::close_ask(app, a.id(), "swapped")?;
            }
            swap_off(app, id, &old.user)?;
            reviewers::record_ask(app, &t, &new.user, &new.name, "replace", &by, prior.map(|a| a.id()))?;
            reviewers::note(app, &t, &by, &format!("Asked {} to review PR #{num} in place of {}", new.name, old.name))?;
        }
        for (w, why) in &asked {
            if reviewers::open_ask(app, id, &w.user)?.is_none() {
                reviewers::record_ask(app, &t, &w.user, &w.name, why, &by, None)?;
            }
        }
        if !asked.is_empty() {
            reviewers::note(app, &t, &by, &format!("Asked {} to review PR #{num}", asked.iter().map(|(w, _)| w.name.clone()).collect::<Vec<_>>().join(", ")))?;
        }
        let mut names: Vec<String> = asked.iter().map(|(w, _)| w.name.clone()).collect();
        if let Some((_, new)) = &replaced {
            names.push(new.name.clone());
        }
        if !names.is_empty() || picking {
            note_asked(app, id, &names, &by)?;
        }
        // At the `ask` stage, asking finishes the agent's visit.
        if t.s("pr_phase") == Some("ask") {
            prflow::waited(app, &t)?;
        }
        Ok(())
    })?;
    let _ = prflow::refresh_task(app, id)?;
    let t = board::get_task(app, id)?;
    Ok(json!({
        "task": rf("task", id),
        "asked": asked.iter().map(|(w, why)| json!({"user": w.user, "name": w.name, "why": why})).collect::<Vec<_>>(),
        "dropped": dropped.iter().map(|w| json!({"user": w.user, "name": w.name})).collect::<Vec<_>>(),
        "replaced": replaced.as_ref().map(|(o, n)| json!({"old": o.name, "new": n.name})),
        "asks": reviewers::asks_of(app, id)?.iter().map(reviewers::ask_dict).collect::<Vec<_>>(),
        "pr": board::pr_card(&t),
    }))
}

/// Whether the agent asks at this PR's `ask` stage: it's being (or will be) brought back in hours.
fn agent_asks(app: &App, t: &Row) -> Result<bool> {
    if prflow::waking(app, t)? {
        return Ok(true);
    }
    Ok(app.cfg.pr.wake && crate::hours::goal_open(app, board::find_goal(app, t.i("goal_id"))?.as_ref()))
}

/// The `ask` stage without an agent: outside work hours (or with `pr.wake` off) the board picks
/// and asks the reviewers itself, retrying after each of `ask_retry_waits` when it can't.
pub fn stage(app: &App) -> Result<()> {
    for t in app.db.q("SELECT * FROM tasks WHERE status = 'done' AND pr_phase = 'ask' AND pr_num IS NOT NULL", vec![])? {
        if let Err(e) = stage_one(app, &t) {
            app.info(format!("reviewers: asking for {}: {}", rf("task", t.id()), e.message));
        }
    }
    Ok(())
}

fn stage_one(app: &App, t: &Row) -> Result<bool> {
    if prflow::waking(app, t)? {
        return Ok(false);
    }
    if let Some(why) = held(app, t)? {
        app.info(format!("reviewers: not asking for {} yet: {why}", rf("task", t.id())));
        return Ok(false);
    }
    let f = flow(t);
    if agent_asks(app, t)? {
        // The agent asks: bring it back now if the feed held its wake before.
        if let Some(rec) = f.get("rec").filter(|r| r.is_object()) {
            app.db.tx(|| prflow::step(app, t, rec).map(|_| ()))?;
        }
        return Ok(false);
    }
    if f.s("ask_retry_at").is_some_and(|r| r > now_iso().as_str()) {
        return Ok(false);
    }
    let Some(rec) = f.get("rec").filter(|r| r.is_object()).cloned() else { return Ok(false) };
    let pr = pr_of(t)?;
    let n = wanted(app, t, &rec, None);
    let tried: std::result::Result<Vec<(Who, String)>, String> = (|| {
        let picks = pick(app, t, &rec, n, &[]).map_err(|e| e.message)?;
        if picks.is_empty() && n > 0 && on_pr(&f, &rec).is_empty() {
            return Err(format!("nobody on {}'s roster can review it", t.st("project")));
        }
        if !picks.is_empty() {
            let users: Vec<String> = picks.iter().map(|(w, _)| w.user.clone()).collect();
            prhost::host_for(app, &pr.host)?.request_reviews(&pr, &users)?;
        }
        Ok(picks)
    })();
    match tried {
        Ok(picks) => {
            app.db.tx(|| {
                let names: Vec<String> = picks.iter().map(|(w, _)| w.name.clone()).collect();
                for (w, _) in &picks {
                    if reviewers::open_ask(app, t.id(), &w.user)?.is_none() {
                        reviewers::record_ask(app, t, &w.user, &w.name, "stage", board::BOARD, None)?;
                    }
                }
                note_asked(app, t.id(), &names, board::BOARD)?;
                let text = if names.is_empty() {
                    format!("PR #{} already has its reviewers", pr.num)
                } else {
                    format!("Asked {} to review PR #{}", names.join(", "), pr.num)
                };
                reviewers::note(app, t, board::BOARD, &text)
            })?;
            let _ = prflow::refresh_task(app, t.id())?;
            Ok(true)
        }
        Err(e) => {
            let waits = &app.cfg.reviewers.ask_retry_waits;
            let tries = f.i0("ask_tries") + 1;
            let wait = waits.get((tries as usize).min(waits.len().max(1)) - 1).copied().unwrap_or(900);
            app.db.tx(|| {
                prflow::merge_flow(app, t.id(), fields!["ask_tries" => tries, "ask_retry_at" => iso(now_ts() + wait as f64)])?;
                let e = e.trim_end_matches(['.', ' ']);
                reviewers::note(app, t, board::BOARD, &format!("Couldn't ask for reviews on PR #{}: {e}. Trying again in {}", pr.num, plural((wait / 60).max(1), "minute")))?;
                if tries == waits.len() as i64 {
                    crate::dispatch::add_alert(
                        app,
                        &format!("The board couldn't ask for reviews on PR #{} for {} after {tries} tries: {e}.", pr.num, rf("task", t.id())),
                        Some(t.id()),
                        t.i("goal_id"),
                        None,
                        Some("pr"),
                    )?;
                }
                Ok(())
            })?;
            Ok(false)
        }
    }
}

/// "I reviewed it": with the `ask` stage on, the PR moves to it now, and the board asks at once
/// when no agent will.
pub fn after_reviewed(app: &App, id: i64) -> Result<()> {
    let t = board::get_task(app, id)?;
    if !reviewers::ask_stage_on(app, t.s("project")) {
        return Ok(());
    }
    let Some(rec) = flow(&t).get("rec").filter(|r| r.is_object()).cloned() else { return Ok(()) };
    app.db.tx(|| prflow::step(app, &t, &rec).map(|_| ()))?;
    let t = board::get_task(app, id)?;
    if t.s("pr_phase") == Some("ask") && t.s("status") == Some("done") {
        stage_one(app, &t)?;
    }
    Ok(())
}

fn reviewer_of<'a>(rec: &'a Value, user: &str) -> Option<&'a Value> {
    rec["reviewers"].as_array()?.iter().find(|r| r["user"].as_str().is_some_and(|u| u.eq_ignore_ascii_case(user)))
}

/// What a reviewer's host state says about an ask: their answer, if they've reviewed. GitHub keeps
/// someone's last review while they're asked again (`requested`), so there an answer counts only once
/// the request is gone; Bitbucket clears it when they're asked again.
fn answer_of(t: &Row, rec: &Value, user: &str) -> Option<String> {
    let r = reviewer_of(rec, user)?;
    if r["requested"] == true && t.s("pr_host") == Some("github") {
        return None;
    }
    let st = r["state"].as_str().unwrap_or("");
    matches!(st, "approved" | "changes" | "commented").then(|| st.to_string())
}

/// Still on the PR: asked (on its reviewer list) or reviewed.
fn still_on(rec: &Value, user: &str) -> bool {
    reviewer_of(rec, user).is_some_and(|r| r["requested"] == true || r["state"] != "pending")
}

/// The review sweep (`runner::reviews`, every `[intervals] reviews` seconds whatever the feed's
/// state): brings the ledger up to date with what the PRs say, then applies the stand-in
/// rules and swaps reviewers who took too long.
///
/// - An ask is answered when its reviewer has reviewed (their speed is the work minutes it took),
///   and closed when the PR merged or closed first.
/// - Came back: someone swapped off who reviews anyway counts again, and their stand-in, if they
///   haven't reviewed yet, is taken off.
/// - Fill-in: someone swapped off who asks for changes doesn't block (they stay in `swapped_off`),
///   and one more reviewer is asked, once, while the PR has fewer than `[reviewers] count`.
/// - An ask whose reviewer was taken off the PR on the host is `dropped` (once a read after the ask
///   shows it), so nobody swaps someone who isn't on it.
/// - Swap (`[reviewers] swap`, per project `tb project set --swap`): an ask still unanswered after `swap_after_mins` work minutes is
///   replaced through the host by the picker's choice. Only inside work hours, and never while the
///   event feed is holding (`feed::holding`; the stand-in rules and the board's own later asks wait
///   for it too, while the first ask of a PR still goes out outside work hours).
pub fn sweep(app: &App) -> Result<()> {
    crate::botrun::note_runs(app)?;
    stage(app)?;
    let tasks = app.db.q("SELECT DISTINCT t.* FROM tasks t JOIN review_asks a ON a.task_id = t.id WHERE a.state IN ('open', 'swapped')", vec![])?;
    for t in tasks {
        let f = flow(&t);
        let Some(rec) = f.get("rec").filter(|r| r.is_object()).cloned() else { continue };
        let finished = matches!(t.s("pr_phase"), Some("merged") | Some("declined"));
        let read_at = f.s("checked_at").unwrap_or("").to_string();
        app.db.tx(|| {
            // The flow as it is now: `tb pr addressed` may have asked again since `t` was read.
            let now_f = flow(&board::get_task(app, t.id())?);
            for a in app.db.q("SELECT * FROM review_asks WHERE task_id = ? AND state = 'open'", crate::p![t.id()])? {
                let user = a.st("host_user");
                // A read from before this ask shows their old review, not an answer to it.
                let ans = answer_of(&t, &rec, &user).filter(|ans| !read_before(&now_f, &rec, &read_at, &a, ans));
                if let Some(ans) = ans {
                    answered(app, &a, "answered", &ans)?;
                } else if finished {
                    reviewers::close_ask(app, a.id(), "closed")?;
                } else if !still_on(&rec, &user) && read_at.as_str() > a.s("asked_at").unwrap_or("") {
                    // Taken off the PR on the host: the ask is over, and nobody swaps them.
                    reviewers::close_ask(app, a.id(), "dropped")?;
                    reviewers::note(app, &t, board::BOARD, &format!("{} isn't on PR #{}'s reviewers any more", a.st("name"), t.i0("pr_num")))?;
                }
            }
            Ok(())
        })?;
        if finished || t.s("status") != Some("done") {
            continue;
        }
        let mut changed = false;
        match stand_ins(app, &t, &rec) {
            Ok(c) => changed |= c,
            Err(e) => app.info(format!("reviewers: stand-in rules on {}: {}", rf("task", t.id()), e.message)),
        }
        match swap_slow(app, &t, &rec) {
            Ok(c) => changed |= c,
            Err(e) => app.info(format!("reviewers: swapping on {}: {}", rf("task", t.id()), e.message)),
        }
        if changed {
            // The reviewers changed on the host: read the PR again so the board shows them.
            let _ = prflow::refresh_task(app, t.id())?;
        }
    }
    Ok(())
}

/// Whether the PR record `rec` (read at `read_at`) is older than the ask `a`, so its reviewer's
/// answer there (`ans`) can't answer it: it was read before the ask (by the second), or, for a
/// `rereview` ask, it still shows the request for changes `tb pr addressed` answered
/// (`answered_changes` in `f`, the flow as it is now).
fn read_before(f: &Row, rec: &Value, read_at: &str, a: &Row, ans: &str) -> bool {
    if read_at < a.s("asked_at").unwrap_or("") {
        return true;
    }
    if a.s("why") != Some("rereview") || ans != "changes" {
        return false;
    }
    let changes_at = rec["changes_at"].as_str().filter(|c| !c.is_empty());
    changes_at.is_some() && f.get("answered_changes").and_then(|v| v.as_str()) == changes_at
}

fn answered(app: &App, a: &Row, state: &str, ans: &str) -> Result<()> {
    let at = a.s("asked_at").and_then(parse_iso).unwrap_or_else(now_ts);
    let mins = crate::picker::work_minutes(app, at, now_ts());
    app.db.update("review_asks", &json!(a.id()), fields!["state" => state, "answer" => ans, "answered_at" => now_iso(), "work_mins" => (mins * 10.0).round() / 10.0])
}

/// Came back and fill-in, for the asks swapped off this task's PR.
fn stand_ins(app: &App, t: &Row, rec: &Value) -> Result<bool> {
    if crate::feed::holding(app, false).is_some() {
        return Ok(false);
    }
    let pr = pr_of(t)?;
    let num = pr.num;
    let mut changed = false;
    for old in app.db.q("SELECT * FROM review_asks WHERE task_id = ? AND state = 'swapped'", crate::p![t.id()])? {
        let user = old.st("host_user");
        let Some(ans) = answer_of(t, rec, &user) else { continue };
        let stand: Vec<Row> = app.db.q("SELECT * FROM review_asks WHERE task_id = ? AND replaces = ? AND state = 'open'", crate::p![t.id(), old.id()])?;
        if ans == "changes" {
            if old.b("filled") {
                continue;
            }
            let t = board::get_task(app, t.id())?;
            // One more reviewer only while the PR has fewer than it needs.
            let pick = if wanted(app, &t, rec, None) > 0 { pick(app, &t, rec, 1, &[])?.into_iter().next() } else { None };
            let enough = pick.is_none() && wanted(app, &t, rec, None) == 0;
            if let Some((w, _)) = &pick {
                host(app, &pr)?.request_reviews(&pr, std::slice::from_ref(&w.user)).map_err(|e| ApiError::new(502, e))?;
                changed = true;
            }
            app.db.tx(|| {
                app.db.update("review_asks", &json!(old.id()), fields!["filled" => 1])?;
                let text = match &pick {
                    Some((w, _)) => {
                        reviewers::record_ask(app, &t, &w.user, &w.name, "fill_in", board::BOARD, Some(old.id()))?;
                        format!("{} asked for changes on PR #{num} after being swapped off, so it doesn't hold; asked {} to look too", old.st("name"), w.name)
                    }
                    None if enough => format!("{} asked for changes on PR #{num} after being swapped off, so it doesn't hold; it has its reviewers", old.st("name")),
                    None => format!("{} asked for changes on PR #{num} after being swapped off, so it doesn't hold; nobody else could be asked", old.st("name")),
                };
                reviewers::note(app, &t, board::BOARD, &text)
            })?;
            continue;
        }
        // They reviewed after all: their review counts, and a stand-in who hasn't looked yet goes.
        let gone: Vec<String> = stand.iter().map(|s| s.st("host_user")).filter(|u| answer_of(t, rec, u).is_none()).collect();
        if !gone.is_empty() {
            host(app, &pr)?.remove_reviewers(&pr, &gone).map_err(|e| ApiError::new(502, e))?;
        }
        changed = true;
        app.db.tx(|| {
            answered(app, &old, "came_back", &ans)?;
            swap_on(app, t.id(), &user)?;
            for s in stand.iter().filter(|s| gone.contains(&s.st("host_user"))) {
                reviewers::close_ask(app, s.id(), "dropped")?;
                swap_off(app, t.id(), &s.st("host_user"))?;
            }
            let names: Vec<String> = stand.iter().filter(|s| gone.contains(&s.st("host_user"))).map(|s| s.st("name")).collect();
            let tail = if names.is_empty() { String::new() } else { format!("; took {} off", names.join(", ")) };
            reviewers::note(app, t, board::BOARD, &format!("{} reviewed PR #{num} after being swapped off{tail}", old.st("name")))
        })?;
    }
    Ok(changed)
}

/// Swaps each reviewer who hasn't answered within `swap_after_mins` work minutes.
fn swap_slow(app: &App, t: &Row, rec: &Value) -> Result<bool> {
    let cfg = &app.cfg.reviewers;
    if !reviewers::swap_on(app, t.s("project")) || crate::feed::holding(app, false).is_some() || !crate::hours::is_open(app) || !matches!(t.s("pr_phase"), Some("review") | Some("rereview")) {
        return Ok(false);
    }
    let pr = pr_of(t)?;
    let mut changed = false;
    for a in app.db.q("SELECT * FROM review_asks WHERE task_id = ? AND state = 'open'", crate::p![t.id()])? {
        let at = a.s("asked_at").and_then(parse_iso).unwrap_or_else(now_ts);
        if crate::picker::work_minutes(app, at, now_ts()) < cfg.swap_after_mins {
            continue;
        }
        let t = board::get_task(app, t.id())?;
        let old = a.st("host_user");
        let Some((new, _)) = pick(app, &t, rec, 1, std::slice::from_ref(&old))?.into_iter().next() else { continue };
        host(app, &pr)?.replace_reviewer(&pr, &old, &new.user).map_err(|e| ApiError::new(502, e))?;
        changed = true;
        app.db.tx(|| {
            reviewers::close_ask(app, a.id(), "swapped")?;
            swap_off(app, t.id(), &old)?;
            reviewers::record_ask(app, &t, &new.user, &new.name, "swap", board::BOARD, Some(a.id()))?;
            let mins = cfg.swap_after_mins.round() as i64;
            reviewers::note(app, &t, board::BOARD, &format!("{} hadn't reviewed PR #{} after {} work minutes; asked {} instead", a.st("name"), pr.num, mins, new.name))
        })?;
    }
    Ok(changed)
}

/// `tb pr addressed` asked these reviewers (host id, name) to look again: each gets an ask (`rereview`)
/// unless they have one open, so the swap rules time them too.
pub fn asked_again(app: &App, t: &Row, who: &[(String, String)], by: &str) -> Result<()> {
    for (user, name) in who.iter().filter(|(u, _)| !u.is_empty()) {
        if reviewers::open_ask(app, t.id(), user)?.is_none() {
            reviewers::record_ask(app, t, user, name, "rereview", by, None)?;
        }
    }
    Ok(())
}

/// How many swaps led to each reviewer's current ask on this task (by host id, lowercased), and when
/// they were asked: for the PR bar's reviewer pills.
pub fn pill_info(app: &App, task_id: i64) -> Result<std::collections::HashMap<String, (i64, Value)>> {
    let asks = reviewers::asks_of(app, task_id)?;
    let mut out = std::collections::HashMap::new();
    for a in asks.iter().filter(|a| matches!(a.s("state"), Some("open") | Some("answered") | Some("came_back"))) {
        let mut swaps = 0;
        let mut cur = a.i("replaces");
        while let Some(id) = cur {
            swaps += 1;
            cur = asks.iter().find(|x| x.id() == id).and_then(|x| x.i("replaces"));
            if swaps > 50 {
                break;
            }
        }
        out.insert(a.st("host_user").to_lowercase(), (swaps, a.v("asked_at")));
    }
    Ok(out)
}
