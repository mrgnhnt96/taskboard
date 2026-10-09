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

/// `tb pr reviewers [--ask WHO…] [--replace X [--with Y]] [--drop X]`.
pub fn pr_reviewers(app: &App, id: i64, body: &Value) -> Result<Value> {
    let t = board::get_task(app, id)?;
    let pr = pr_of(&t)?;
    let rec = match prflow::refresh_task(app, id)? {
        Ok(r) => r,
        Err(e) => return err(502, format!("Couldn't read PR #{}: {e}.", pr.num)),
    };
    let t = board::get_task(app, id)?;
    let by = who_of(body);
    let h = host(app, &pr)?;
    let fail = |what: &str, e: String| ApiError::new(502, format!("Couldn't {what} on PR #{}: {e}.", pr.num));
    let mut asked: Vec<Who> = vec![];
    let mut dropped: Vec<Who> = vec![];
    let mut replaced: Option<(Who, Who)> = None;
    let drop = body_str(body, "drop");
    let replace = body_str(body, "replace");
    let ask = str_list(body.get("ask"));
    if drop.is_empty() && replace.is_empty() && ask.is_empty() {
        return err(400, "Say who: --ask <name> (repeat for more), --replace <name> --with <name>, or --drop <name>.");
    }
    if !drop.is_empty() {
        let w = resolve(app, &t, &rec, &drop)?;
        h.remove_reviewers(&pr, std::slice::from_ref(&w.user)).map_err(|e| fail("take a reviewer off", e))?;
        dropped.push(w);
    }
    if !replace.is_empty() {
        let old = resolve(app, &t, &rec, &replace)?;
        let with = body_str(body, "with");
        if with.is_empty() {
            return err(400, format!("Who stands in for {}? Add --with <name>.", old.name));
        }
        let new = resolve(app, &t, &rec, &with)?;
        may_ask(app, &t, &rec, &new)?;
        h.replace_reviewer(&pr, &old.user, &new.user).map_err(|e| fail("swap the reviewer", e))?;
        replaced = Some((old, new));
    }
    let mut want = vec![];
    for a in &ask {
        let w = resolve(app, &t, &rec, a)?;
        may_ask(app, &t, &rec, &w)?;
        if !want.iter().any(|x: &Who| x.user == w.user) {
            want.push(w);
        }
    }
    if !want.is_empty() {
        let users: Vec<String> = want.iter().map(|w| w.user.clone()).collect();
        h.request_reviews(&pr, &users).map_err(|e| fail("ask for reviews", e))?;
        asked.extend(want);
    }
    let why = if body_str(body, "why").is_empty() { "ask".to_string() } else { body_str(body, "why") };
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
        for w in &asked {
            if reviewers::open_ask(app, id, &w.user)?.is_none() {
                reviewers::record_ask(app, &t, &w.user, &w.name, &why, &by, None)?;
            }
        }
        if !asked.is_empty() {
            reviewers::note(app, &t, &by, &format!("Asked {} to review PR #{num}", asked.iter().map(|w| w.name.clone()).collect::<Vec<_>>().join(", ")))?;
        }
        let mut names: Vec<String> = asked.iter().map(|w| w.name.clone()).collect();
        if let Some((_, new)) = &replaced {
            names.push(new.name.clone());
        }
        if !names.is_empty() {
            note_asked(app, id, &names, &by)?;
        }
        Ok(())
    })?;
    let _ = prflow::refresh_task(app, id)?;
    let t = board::get_task(app, id)?;
    Ok(json!({
        "task": rf("task", id),
        "asked": asked.iter().map(|w| json!({"user": w.user, "name": w.name})).collect::<Vec<_>>(),
        "dropped": dropped.iter().map(|w| json!({"user": w.user, "name": w.name})).collect::<Vec<_>>(),
        "replaced": replaced.as_ref().map(|(o, n)| json!({"old": o.name, "new": n.name})),
        "asks": reviewers::asks_of(app, id)?.iter().map(reviewers::ask_dict).collect::<Vec<_>>(),
        "pr": board::pr_card(&t),
    }))
}
