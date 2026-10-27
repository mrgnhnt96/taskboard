//! What `tb pr …` does to a task's PR through its host (`prhost.rs`): the full status, replying to and
//! acknowledging threads, `addressed` (re-request review), the guarded merge, and clearing a failure
//! that isn't the PR's (`not-ours`).
//!
//! Each reads the PR fresh first (`prflow::refresh_task`), so it judges what the host says now, and
//! never calls the host inside a database transaction. Reading and answering a thread don't (#679):
//! `tb pr thread` serves the board's own copy of the PR (its last poll or stream event) while it's
//! younger than [`HELD_SECS`], and `tb pr reply` / `tb pr ack` write to the host once and note the
//! write on that copy. When they do read, callers asking at once share the one read ([`shared_read`]).
//! A reply or resolve the host refuses while the PR's account is rate-limited is kept and posted by
//! the board once the limit is over ([`post_held`]), so the agent doesn't wait for it. So is the ask
//! `addressed` makes for another review ([`ask_held_rereviews`]).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::App;
use crate::prhost::{self, MergeOpts, PrRef, Thread};
use crate::util::*;
use crate::{board, fields, p, prflow, progress};

/// How many of the base branch's latest commits `tb pr status` compares a failure against.
pub const BASE_COMMITS: usize = 5;
pub const REASON_MIN: usize = 20;
pub const REASON_MAX: usize = 300;
pub const TITLE_MAX: usize = 80;
/// How long `tb pr thread` serves the board's copy of a PR before it reads the host again.
pub const HELD_SECS: f64 = 60.0;

fn pr_of(t: &Row) -> Result<PrRef> {
    match PrRef::of(t) {
        Some(p) => Ok(p),
        None => err(404, format!("{} has no PR linked.", rf("task", t.id()))),
    }
}

fn host(app: &App, pr: &PrRef) -> Result<Arc<dyn prhost::PrHost>> {
    prhost::host_for(app, &pr.host).map_err(|e| ApiError::new(409, format!("Can't work with PR #{}: {e}.", pr.num)))
}

/// Runs `f`, one call to `pr`'s host, saying `still waiting on <host> (<doing> PR #n)` on the
/// progress stream while it runs.
fn on_host<T>(pr: &PrRef, doing: &str, f: impl FnOnce() -> T) -> T {
    progress::waiting(&format!("{} ({doing} PR #{})", prhost::limit::label(&pr.host), pr.num), progress::WAITING_EVERY, f)
}

/// Reads the PR now and steps it; the task and the record.
fn fresh(app: &App, id: i64) -> Result<(Row, Value)> {
    let t = board::get_task(app, id)?;
    let pr = pr_of(&t)?;
    match on_host(&pr, "reading", || prflow::refresh_task(app, id))? {
        Ok(rec) => Ok((board::get_task(app, id)?, rec)),
        Err(e) => err(502, format!("Couldn't read PR #{}: {e}.", pr.num)),
    }
}

/// The last read of each task's PR made here, and how many have ended: a caller that asked while one
/// ran takes its answer instead of reading again.
#[derive(Default)]
pub struct Flight {
    reads: AtomicU64,
    last: Mutex<Option<std::result::Result<Value, String>>>,
}

/// Reads the PR now and steps it, as [`fresh`] does, but one read at a time per task: a caller that
/// asked while another's read ran gets that read's answer, so concurrent asks cost the host one call.
fn shared_read(app: &App, id: i64, pr: &PrRef) -> Result<std::result::Result<Value, String>> {
    let f = app.shared.lock().pr_reads.entry(id).or_default().clone();
    let asked = f.reads.load(Ordering::SeqCst);
    let mut last = f.last.lock();
    if let Some(r) = last.as_ref().filter(|_| f.reads.load(Ordering::SeqCst) > asked) {
        return Ok(r.clone());
    }
    let r = on_host(pr, "reading", || prflow::refresh_task(app, id))?;
    *last = Some(r.clone());
    f.reads.fetch_add(1, Ordering::SeqCst);
    Ok(r)
}

/// The record to find thread `want` in, and why it may be stale: the board's copy while it's younger
/// than `max_age` seconds and has the thread, else a read of the host. When that read fails, the
/// board's copy still answers if it has the thread, marked stale with why.
fn held_or_read(app: &App, id: i64, want: &str, max_age: f64) -> Result<(Row, Value, Option<String>)> {
    let t = board::get_task(app, id)?;
    let pr = pr_of(&t)?;
    let f = flow(&t);
    let held = f.get("rec").filter(|r| r["threads"].is_array()).cloned();
    let young = wall_age_secs(f.s("checked_at")).is_some_and(|a| a < max_age);
    if let Some(rec) = held.as_ref().filter(|r| young && find_thread(r, pr.num, want).is_ok()) {
        return Ok((t, rec.clone(), None));
    }
    match shared_read(app, id, &pr)? {
        Ok(rec) => Ok((board::get_task(app, id)?, rec, None)),
        Err(e) => match held.filter(|r| find_thread(r, pr.num, want).is_ok()) {
            Some(rec) => Ok((t, rec, Some(e))),
            None => err(502, format!("Couldn't read PR #{}: {e}.", pr.num)),
        },
    }
}

fn flow(t: &Row) -> Row {
    jloads_obj(t.s("pr_flow"))
}

fn names(v: &[Value], key: &str) -> Vec<String> {
    v.iter().filter_map(|x| x[key].as_str().map(|s| s.to_string())).collect()
}

fn find_thread(rec: &Value, num: i64, id: &str) -> Result<Thread> {
    let want = id.trim().trim_start_matches('#');
    rec["threads"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| serde_json::from_value::<Thread>(t).ok())
        .find(|t| t.id == want)
        .ok_or_else(|| ApiError::new(404, format!("PR #{num} has no thread {want}. tb pr status lists the open ones with their ids.")))
}

/// Notes on the board that a thread was answered at its last comment (`threads_acked`).
fn ack_locally(app: &App, id: i64, th: &Thread) -> Result<()> {
    let t = board::get_task(app, id)?;
    let mut acks = flow(&t).get("threads_acked").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    acks.insert(th.id.clone(), json!(th.last_id));
    prflow::merge_flow(app, id, fields!["threads_acked" => Value::Object(acks)]).map(|_| ())
}

fn who_of(app: &App, body: &Value) -> String {
    board::caller(app, body, "the owner")
}

/// `tb pr reply <thread> "<text>" [--resolve]`.
pub fn reply(app: &App, id: i64, body: &Value) -> Result<Value> {
    let text = body_str(body, "text").trim().to_string();
    if text.is_empty() {
        return err(400, "Say what you did: the reply can't be empty.");
    }
    let want = body_str(body, "thread");
    let (t, rec, _) = held_or_read(app, id, &want, f64::INFINITY)?;
    let pr = pr_of(&t)?;
    let th = find_thread(&rec, pr.num, &want)?;
    let h = host(app, &pr)?;
    let resolve = as_bool(body.get("resolve"), false);
    if let Err(e) = on_host(&pr, "replying on", || h.reply(&pr, &th, &text)) {
        return match hold(app, id, &pr, &th, &text, resolve, &who_of(app, body))? {
            Some(said) => answered(app, id, json!({"thread": th.id, "resolved": false, "held": said})),
            None => err(502, format!("Couldn't reply on PR #{}: {e}.", pr.num)),
        };
    }
    let resolved = if resolve {
        match on_host(&pr, "resolving a thread on", || h.resolve(&pr, &th)) {
            Ok(r) => r,
            Err(e) => match hold(app, id, &pr, &th, "", true, &who_of(app, body))? {
                Some(said) => {
                    posted(app, id, &pr, &th, &text, true, false, &who_of(app, body))?;
                    return answered(app, id, json!({"thread": th.id, "replied": true, "resolved": false, "held": said}));
                }
                None => return err(502, format!("Replied, but couldn't resolve the thread: {e}.")),
            },
        }
    } else {
        false
    };
    posted(app, id, &pr, &th, &text, resolve, resolved, &who_of(app, body))?;
    answered(app, id, json!({"thread": th.id, "resolved": resolved}))
}

/// Notes on the board a reply (or, with no `text`, a resolve) the host has taken on `th`.
fn posted(app: &App, id: i64, pr: &PrRef, th: &Thread, text: &str, resolve: bool, resolved: bool, who: &str) -> Result<()> {
    app.db.tx(|| {
        // A plain comment has no thread on the host to carry the reply: the board remembers it was answered.
        if !th.resolvable || resolve {
            ack_locally(app, id, th)?;
        }
        let what = if resolved { "Replied to and resolved" } else { "Replied to" };
        board::log_event(app, id, who, "status", &format!("{what} {} on PR #{}: {}", thread_name(th), pr.num, one_line(text, 200))).map(|_| ())
    })?;
    note_write(app, id, th, text, resolved)
}

/// Notes a reply (none when `text` is empty) and a resolve on `th` in the board's copy of the PR.
fn note_write(app: &App, id: i64, th: &Thread, text: &str, resolved: bool) -> Result<()> {
    noted(app, id, &th.id, |rec, held| {
        if !text.is_empty() {
            let us = prflow::us(rec).to_string();
            let at = now_iso();
            held.replies.push(prhost::Reply { author: us.clone(), author_name: us.clone(), text: one_line(text, prhost::REPLY_MAX), body: prhost::whole(text), at: at.clone() });
            held.last_author = us;
            held.last_at = at;
        }
        held.resolved |= resolved;
    })
}

/// `tb pr ack <thread>`: resolved without a reply (on the board only, where the host can't resolve it).
pub fn ack(app: &App, id: i64, body: &Value) -> Result<Value> {
    let want = body_str(body, "thread");
    let (t, rec, _) = held_or_read(app, id, &want, f64::INFINITY)?;
    let pr = pr_of(&t)?;
    let th = find_thread(&rec, pr.num, &want)?;
    let resolved = if th.resolvable && !th.resolved {
        let h = host(app, &pr)?;
        match on_host(&pr, "resolving a thread on", || h.resolve(&pr, &th)) {
            Ok(r) => r,
            Err(e) => match hold(app, id, &pr, &th, "", true, &who_of(app, body))? {
                Some(said) => return answered(app, id, json!({"thread": th.id, "resolved": false, "held": said})),
                None => return err(502, format!("Couldn't resolve the thread: {e}.")),
            },
        }
    } else {
        false
    };
    app.db.tx(|| {
        ack_locally(app, id, &th)?;
        board::log_event(app, id, &who_of(app, body), "status", &format!("Acknowledged {} on PR #{} without a reply", thread_name(&th), pr.num)).map(|_| ())
    })?;
    noted(app, id, &th.id, |_, held| held.resolved |= resolved)?;
    answered(app, id, json!({"thread": th.id, "resolved": resolved}))
}

/// A reply (or, with no `text`, a resolve) the host refused while the PR's account was rate-limited,
/// kept in the PR's flow (`held_replies`) until [`post_held`] posts it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HeldReply {
    thread: Thread,
    #[serde(default)]
    text: String,
    #[serde(default)]
    resolve: bool,
}

fn held_replies(t: &Row) -> Vec<HeldReply> {
    flow(t).get("held_replies").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

/// Keeps a write on `th` that the host refused, when the PR's account is rate-limited, for
/// [`post_held`] to make once the limit is over, and counts the thread answered on the board now.
/// What to tell the caller; None (nothing kept) when the account isn't held: it failed for another reason.
fn hold(app: &App, id: i64, pr: &PrRef, th: &Thread, text: &str, resolve: bool, who: &str) -> Result<Option<String>> {
    let Some(until) = prflow::host_held(app, pr) else { return Ok(None) };
    let at = local_clock(Some(&iso(until)));
    let site = prhost::limit::label(&pr.host);
    let what = match (text.is_empty(), resolve) {
        (true, _) => "resolves the thread",
        (false, true) => "posts the reply and resolves the thread",
        (false, false) => "posts the reply",
    };
    app.db.tx(|| {
        let mut held = held_replies(&board::get_task(app, id)?);
        held.push(HeldReply { thread: th.clone(), text: text.to_string(), resolve });
        prflow::merge_flow(app, id, fields!["held_replies" => json!(held)])?;
        ack_locally(app, id, th)?;
        board::log_event(app, id, who, "status", &format!("{site} has the board rate-limited: it {what} on {} of PR #{} once the limit is over, by {at}", thread_name(th), pr.num)).map(|_| ())
    })?;
    Ok(Some(format!("{site} has the board rate-limited, so the board {what} once the limit is over, by {at}. The thread counts as answered now: carry on, and don't wait for it or try again.")))
}

/// Takes the local answer off `th` again, unless someone spoke on it since.
fn unack_locally(app: &App, id: i64, th: &Thread) -> Result<()> {
    let t = board::get_task(app, id)?;
    let mut acks = flow(&t).get("threads_acked").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    if acks.get(&th.id).and_then(|v| v.as_str()) == Some(th.last_id.as_str()) {
        acks.remove(&th.id);
    }
    prflow::merge_flow(app, id, fields!["threads_acked" => Value::Object(acks)]).map(|_| ())
}

/// The PR sweep: posts each PR's held replies and resolves, oldest first, once its host takes them.
/// One the host still refuses for the limit waits with those after it (a reply that went through
/// waits as its resolve alone); one it refuses for another reason is dropped, said on the task's log,
/// and its thread counts as open again.
pub fn post_held(app: &App) -> Result<()> {
    for t in app.db.q("SELECT * FROM tasks WHERE pr_num IS NOT NULL AND json_extract(pr_flow, '$.held_replies[0]') IS NOT NULL", p![])? {
        let (id, Some(pr)) = (t.id(), PrRef::of(&t)) else { continue };
        let Ok(h) = prhost::host_for(app, &pr.host) else { continue };
        let mut held = held_replies(&t);
        while let Some(mut r) = held.first().cloned() {
            let name = thread_name(&r.thread);
            let wrote = if r.text.is_empty() { Ok(()) } else { h.reply(&pr, &r.thread, &r.text) };
            if wrote.is_ok() && !r.text.is_empty() {
                let text = std::mem::take(&mut r.text);
                held[0] = r.clone();
                if !r.resolve {
                    held.remove(0);
                }
                app.db.tx(|| {
                    prflow::merge_flow(app, id, fields!["held_replies" => json!(held)])?;
                    board::log_event(app, id, board::BOARD, "status", &format!("Posted the held reply to {name} on PR #{}: {}", pr.num, one_line(&text, 200))).map(|_| ())
                })?;
                note_write(app, id, &r.thread, &text, false)?;
                continue;
            }
            let wrote = wrote.and_then(|_| h.resolve(&pr, &r.thread));
            match wrote {
                Ok(resolved) => {
                    held.remove(0);
                    app.db.tx(|| {
                        prflow::merge_flow(app, id, fields!["held_replies" => json!(held)])?;
                        board::log_event(app, id, board::BOARD, "status", &format!("Resolved {name} on PR #{}, held over the rate limit", pr.num)).map(|_| ())
                    })?;
                    note_write(app, id, &r.thread, "", resolved)?;
                }
                Err(_) if prflow::host_held(app, &pr).is_some() => break,
                Err(e) => {
                    held.remove(0);
                    let what = if r.text.is_empty() { "resolve" } else { "reply to" };
                    app.db.tx(|| {
                        prflow::merge_flow(app, id, fields!["held_replies" => json!(held)])?;
                        unack_locally(app, id, &r.thread)?;
                        board::log_event(app, id, board::BOARD, "status", &format!("Couldn't post the held {what} {name} on PR #{}, so it's open again: {}", pr.num, one_line(&e, 200))).map(|_| ())
                    })?;
                }
            }
        }
    }
    Ok(())
}

/// `tb pr thread <thread> [--refresh]`: one thread with its whole text and replies, as the board's
/// account reads it (the agent has none of its own). From the board's copy of the PR while it's
/// younger than [`HELD_SECS`] (`refresh` reads the host anyway); `as_of` is when that copy was read,
/// and `stale` says why it couldn't be read again.
pub fn thread(app: &App, id: i64, body: &Value) -> Result<Value> {
    let want = body_str(body, "thread");
    let max_age = if as_bool(body.get("refresh"), false) { 0.0 } else { HELD_SECS };
    let (t, rec, stale) = held_or_read(app, id, &want, max_age)?;
    let pr = pr_of(&t)?;
    let th = find_thread(&rec, pr.num, &want)?;
    let open = prflow::open_threads(&flow(&t), &rec).iter().any(|o| o["id"].as_str() == Some(th.id.as_str()));
    Ok(json!({"thread": th, "open": open, "as_of": flow(&t).v("checked_at"), "stale": stale}))
}

/// Notes a write on thread `id` in the board's copy of the PR, as the host now has it, and steps the
/// PR with it, without reading the host. The copy's read time stays its last read's, so it ages out
/// as before.
fn noted(app: &App, id: i64, thread: &str, change: impl FnOnce(&Value, &mut Thread)) -> Result<()> {
    let f = flow(&board::get_task(app, id)?);
    let Some(mut rec) = f.get("rec").filter(|r| r["threads"].is_array()).cloned() else { return Ok(()) };
    let at = rec["threads"].as_array().and_then(|a| a.iter().position(|t| t["id"].as_str() == Some(thread)));
    let Some(held) = at.and_then(|i| serde_json::from_value::<Thread>(rec["threads"][i].clone()).ok().map(|th| (i, th))) else { return Ok(()) };
    let (i, mut th) = held;
    change(&rec, &mut th);
    rec["threads"][i] = json!(th);
    prflow::apply(app, id, &rec)?;
    prflow::merge_flow(app, id, fields!["checked_at" => f.v("checked_at")]).map(|_| ())
}

/// Answers a write on a thread with the PR's card, from the board's copy.
fn answered(app: &App, id: i64, mut out: Value) -> Result<Value> {
    let t = board::get_task(app, id)?;
    out["task"] = json!(rf("task", id));
    out["pr"] = board::pr_card(app, &t);
    out["open_threads"] = json!(prflow::open_threads(&flow(&t), &flow(&t).get("rec").cloned().unwrap_or(Value::Null)).len());
    Ok(out)
}

fn thread_name(th: &Thread) -> String {
    let who = if th.author_name.is_empty() { &th.author } else { &th.author_name };
    let kind = if th.kind == "task" { "task" } else { "thread" };
    if who.is_empty() {
        format!("{kind} {}", th.id)
    } else {
        format!("{who}'s {kind} {}", th.id)
    }
}

/// Steps the PR again after a change and answers with its card.
fn after(app: &App, id: i64, out: Value) -> Result<Value> {
    let _ = match PrRef::of(&board::get_task(app, id)?) {
        Some(pr) => on_host(&pr, "reading", || prflow::refresh_task(app, id))?,
        None => prflow::refresh_task(app, id)?,
    };
    answered(app, id, out)
}

/// `tb pr addressed`: refuses while threads are open; then asks each reviewer with a standing request
/// for changes (not one swapped off) to review again, and ends the visit. Each ask is kept per reviewer
/// in the PR's flow (`rereview_asks`: when, the request it answers, and whether it's `queued`), so the
/// bar shows it until that reviewer reviews again. One the host refuses while the PR's account is
/// rate-limited is queued and sent by the board once the limit is over ([`ask_held_rereviews`]).
/// `sent` says which: `host` (asked now), `queued`, or `local` (nobody to ask; recorded on the board).
pub fn addressed(app: &App, id: i64, body: &Value) -> Result<Value> {
    if let Some(why) = crate::slots::visit_end_refusal(app, &board::get_task(app, id)?, body)? {
        return err(409, why);
    }
    let pr = pr_of(&board::get_task(app, id)?)?;
    let (t, rec) = fresh(app, id).map_err(|e| prflow::wait_out_limit(app, id, &pr, body, "comments", e))?;
    let f = flow(&t);
    let open = prflow::open_threads(&f, &rec);
    if !open.is_empty() {
        let ids = names(&open, "id").join(", ");
        return err(
            409,
            format!(
                "PR #{} still has {} open: {ids}. Answer each with tb pr reply <thread> \"<what you did>\" --resolve, or tb pr ack <thread> when it asks for nothing.",
                pr.num,
                plural(open.len() as i64, "thread")
            ),
        );
    }
    let review = prflow::review_of(&f, &rec);
    let users: Vec<String> = review.requesters.iter().map(|(u, _)| u.clone()).filter(|u| !u.is_empty()).collect();
    // When the host has the board rate-limited, the ask waits for the limit to end.
    let mut held = None;
    if !users.is_empty() {
        if let Err(e) = on_host(&pr, "asking for another review on", || host(app, &pr)?.re_request_reviews(&pr, &users).map_err(|e| ApiError::new(502, e))) {
            match prflow::host_held(app, &pr) {
                Some(until) => held = Some(until),
                None => return err(502, format!("Couldn't ask for another review on PR #{}: {}.", pr.num, e.message)),
            }
        }
    }
    let asked: Vec<String> = review.requesters.iter().map(|(u, n)| if n.is_empty() { u.clone() } else { n.clone() }).collect();
    let sent = match (users.is_empty(), held) {
        (true, _) => "local",
        (false, None) => "host",
        (false, Some(_)) => "queued",
    };
    let site = prhost::limit::label(&pr.host);
    let until = held.map(|u| local_clock(Some(&iso(u))));
    let at = now_iso();
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        prflow::waited(app, &t)?;
        crate::asks::asked_again(app, &t, &review.requesters, &who_of(app, body))?;
        let mut asks = flow(&t).get("rereview_asks").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        for user in &users {
            let r = rec["reviewers"].as_array().into_iter().flatten().find(|r| r["user"].as_str() == Some(user.as_str())).cloned().unwrap_or(Value::Null);
            let name = r["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(user);
            asks.insert(user.to_lowercase(), json!({"user": user, "name": name, "at": at, "changes_at": r["changes_at"], "queued": held.is_some()}));
        }
        prflow::merge_flow(
            app,
            id,
            fields!["answered_changes" => rec["changes_at"].clone(),
                    "answered_changes_by" => crate::asks::changes_by(&rec),
                    "rereview_asks" => Value::Object(asks),
                    "addressed" => json!({"at": at, "head": rec["head"], "asked": asked, "asked_ids": users, "sent": sent})],
        )?;
        let text = match &until {
            _ if asked.is_empty() => format!("Addressed the review on PR #{}", pr.num),
            None => format!("Addressed the review on PR #{} and asked {} to look again", pr.num, asked.join(", ")),
            Some(by) => format!("Addressed the review on PR #{}; {site} has the board rate-limited, so it asks {} to look again once the limit is over, by {by}", pr.num, asked.join(", ")),
        };
        board::log_event(app, id, &who_of(app, body), "status", &text).map(|_| ())
    })?;
    crate::reports::give_back_slot(app, id);
    let out = json!({"asked": asked, "sent": sent, "site": site, "until": until});
    // Reading the PR again would only fail while the account is held: step it on the copy just read.
    if held.is_none() {
        return after(app, id, out);
    }
    app.db.tx(|| prflow::step(app, &board::get_task(app, id)?, &rec).map(|_| ()))?;
    answered(app, id, out)
}

/// How long the board waits before asking for a queued re-review again after the host refused it for
/// a reason other than its rate limit.
const REREVIEW_RETRY_SECS: f64 = 900.0;

/// The PR sweep: sends the re-review asks `tb pr addressed` queued while the PR's host had the board
/// rate-limited, once the limit is over. One the host still refuses for the limit waits; one it refuses
/// for another reason stays queued and is tried again every [`REREVIEW_RETRY_SECS`] (said once on the
/// task's log), so it's never dropped while it stands. A read that sees the reviewer review again
/// drops it (`prflow::step`), and a merged or declined PR drops its queue.
pub fn ask_held_rereviews(app: &App) -> Result<()> {
    let sql = "SELECT * FROM tasks WHERE pr_num IS NOT NULL AND EXISTS \
               (SELECT 1 FROM json_each(pr_flow, '$.rereview_asks') WHERE json_extract(value, '$.queued') = 1)";
    for t in app.db.q(sql, p![])? {
        let (id, Some(pr)) = (t.id(), PrRef::of(&t)) else { continue };
        let f = flow(&t);
        if matches!(t.s("pr_phase"), Some("merged" | "declined")) {
            prflow::merge_flow(app, id, fields!["rereview_asks" => null, "rereview_retry_at" => null, "rereview_error" => null])?;
            continue;
        }
        if f.get("rereview_retry_at").and_then(|v| v.as_str()).and_then(parse_iso).is_some_and(|at| at > now_ts()) {
            continue;
        }
        let Ok(h) = prhost::host_for(app, &pr.host) else { continue };
        if h.held_until().is_some() {
            continue;
        }
        let queued: Vec<Value> = f.get("rereview_asks").and_then(|v| v.as_object()).into_iter().flat_map(|m| m.values()).filter(|a| a["queued"] == true).cloned().collect();
        let users: Vec<String> = queued.iter().filter_map(|a| a["user"].as_str().map(str::to_string)).collect();
        let names = queued.iter().filter_map(|a| a["name"].as_str().or(a["user"].as_str())).collect::<Vec<_>>().join(", ");
        match h.re_request_reviews(&pr, &users) {
            Ok(()) => app.db.tx(|| {
                let mut asks = flow(&board::get_task(app, id)?).get("rereview_asks").and_then(|v| v.as_object()).cloned().unwrap_or_default();
                let at = now_iso();
                for a in asks.values_mut().filter(|a| a["queued"] == true && a["user"].as_str().is_some_and(|u| users.iter().any(|x| x == u))) {
                    a["queued"] = json!(false);
                    a["at"] = json!(at);
                }
                prflow::merge_flow(app, id, fields!["rereview_asks" => Value::Object(asks), "rereview_retry_at" => null, "rereview_error" => null])?;
                board::log_event(app, id, board::BOARD, "status", &format!("Asked {names} to look again on PR #{}, held over the rate limit", pr.num)).map(|_| ())
            })?,
            Err(_) if prflow::host_held(app, &pr).is_some() => {}
            Err(e) => {
                let e = one_line(&e, 200);
                let again = iso(now_ts() + REREVIEW_RETRY_SECS);
                let first = f.get("rereview_error").and_then(|v| v.as_str()) != Some(e.as_str());
                app.db.tx(|| {
                    prflow::merge_flow(app, id, fields!["rereview_retry_at" => again, "rereview_error" => e.clone()])?;
                    if first {
                        let text = format!("Couldn't ask {names} to look again on PR #{} yet; the board tries again every {} minutes: {e}", pr.num, (REREVIEW_RETRY_SECS / 60.0) as i64);
                        board::log_event(app, id, board::BOARD, "status", &text)?;
                    }
                    Ok(())
                })?;
            }
        }
    }
    Ok(())
}

/// Why the PR can't merge right now (empty: it can). Checks the record as just read. `owner_approved`: the
/// owner's word to merge it approves it too, which stands in for the approvals it needs.
pub fn merge_blockers(app: &App, t: &Row, rec: &Value, owner_approved: bool) -> Result<Vec<String>> {
    let f = flow(t);
    let mut out = vec![];
    let state = rec["state"].as_str().unwrap_or("OPEN").to_uppercase();
    if state != "OPEN" {
        out.push(format!("it's {}", state.to_lowercase()));
        return Ok(out);
    }
    if prflow::conflicting(&f, rec) {
        out.push(format!("it conflicts with its base branch {}: rebase onto it and push first", rec["base"].as_str().unwrap_or("main")));
    }
    // A failure holds the merge unless the owner's hook skipped it; `tb pr skip-checks` doesn't cover one.
    let failing = prflow::failing(&f, rec);
    if !failing.is_empty() && !prflow::skip_hides_failures(&f, rec) {
        out.push(format!("checks failed: {}", failing.join(", ")));
    }
    if prflow::checks_skipped(&f, rec).is_none() {
        let checks = rec["checks"].as_array().cloned().unwrap_or_default();
        let running = running_checks(t, rec);
        if !running.is_empty() {
            out.push(format!("checks are still running: {}", running.join(", ")));
        }
        let stopped: Vec<String> = checks.iter().filter(|c| c["state"] == "stopped").filter_map(|c| c["name"].as_str().map(|s| s.to_string())).collect();
        if !stopped.is_empty() {
            out.push(format!("builds were stopped: {} (run them again, or tb pr skip-checks)", stopped.join(", ")));
        }
        // The same rule as the stage (`prflow::checks_waiting`): once the wait is over, an expected check
        // that never posted doesn't hold the merge; only failed ones do.
        match prflow::expected_missing(app, t, rec) {
            Some(missing) if !missing.is_empty() && !prflow::expected_wait_over(app, t, &f, rec) => {
                let what = if prflow::expected_for(app, t, rec).source == Some("earlier") {
                    "checks that posted on the last push haven't posted on this one"
                } else {
                    "expected checks haven't posted"
                };
                out.push(format!("{what}: {} (it waits up to {} min for them)", missing.join(", "), prflow::missing_wait_mins(app, t, rec)));
            }
            None if checks.is_empty() && !prflow::no_checks_wait_over(app, &f, rec) => {
                out.push(format!("no checks have posted on this push yet (waits up to {} min)", app.cfg.pr.no_checks_after_mins));
            }
            _ => {}
        }
    }
    if !prflow::review_skipped(app, t, &f, rec) {
        let review = prflow::review_of(&f, rec);
        if review.changes {
            let who: Vec<String> = review.requesters.iter().map(|(u, n)| if n.is_empty() { u.clone() } else { n.clone() }).collect();
            out.push(if who.is_empty() { "a reviewer asked for changes".into() } else { format!("{} asked for changes", who.join(", ")) });
        } else if !owner_approved && !prflow::approved(app, t, &review) {
            out.push(match prflow::approvals_needed(app, t) {
                Some(n) => format!("it has {} of the {n} approval{} it needs", review.approvals, if n == 1 { "" } else { "s" }),
                None => "it isn't approved".into(),
            });
        }
    }
    if let Some(why) = crate::lens_pr::blocker(app, &f) {
        out.push(why);
    }
    let open = prflow::open_threads(&f, rec);
    let tasks = open.iter().filter(|x| x["kind"] == "task").count() as i64;
    let threads = open.len() as i64 - tasks;
    if threads > 0 {
        out.push(format!("{} open", plural(threads, "thread")));
    }
    if tasks > 0 {
        out.push(format!("{} open", plural(tasks, "PR task")));
    }
    if let Some(e) = rec["tasks_error"].as_str() {
        out.push(format!("couldn't read its PR tasks ({e})"));
    }
    // A head-merge PR merges into its parent's unmerged branch: that's where it goes, not a hold.
    if !crate::stack::head(t) {
        if let Some(b) = stacked_base(app, t, rec)? {
            out.push(b);
        }
    }
    if let Some(h) = crate::stack::stack_hold(app, t)? {
        out.push(h);
    }
    // Merge steps (`before = "merge"`) hold only the merge, judged on the PR's head.
    let steps = prflow::merge_steps_left(app, t, rec);
    if !steps.is_empty() {
        out.push(format!("its merge steps haven't passed on its head: {}", crate::steps::names(&steps)));
    }
    Ok(out)
}

/// The checks still running on this push, unless its checks are skipped.
fn running_checks(t: &Row, rec: &Value) -> Vec<String> {
    if prflow::checks_skipped(&flow(t), rec).is_some() {
        return vec![];
    }
    let checks = rec["checks"].as_array().cloned().unwrap_or_default();
    checks.iter().filter(|c| c["state"] == "running").filter_map(|c| c["name"].as_str().map(|s| s.to_string())).collect()
}

/// `POST /tasks/:id/pr/checks-running`: a merge was refused while these checks ran on the host (the
/// old board's `tb pr merge` posts it). A note for an older head is ignored.
pub fn checks_running(app: &App, id: i64, body: &Value) -> Result<Value> {
    let head = body_str(body, "head");
    let checks: Vec<String> = body["checks"].as_array().map(|a| a.iter().filter_map(|c| c.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    if head.is_empty() || checks.is_empty() {
        return err(400, "checks-running needs a head and the running checks' names.");
    }
    let t = board::get_task(app, id)?;
    let rec = flow(&t).get("rec").cloned().unwrap_or(Value::Null);
    let current = rec["head"].as_str().unwrap_or("");
    let short: String = head.chars().take(12).collect();
    if !current.is_empty() && !current.starts_with(&short) {
        return Ok(json!({"task": rf("task", id), "noted": false}));
    }
    app.db.tx(|| {
        prflow::note_checks_running(app, &t, &head, &checks, &who_of(app, body))?;
        if rec.is_object() {
            prflow::step(app, &board::get_task(app, id)?, &rec)?;
        }
        Ok(())
    })?;
    Ok(json!({"task": rf("task", id), "noted": true}))
}

/// A base branch that is another task's PR, still unmerged.
fn stacked_base(app: &App, t: &Row, rec: &Value) -> Result<Option<String>> {
    let Some(base) = rec["base"].as_str().filter(|b| !b.is_empty()) else { return Ok(None) };
    let row = app.db.q1(
        "SELECT id, pr_num, pr_phase FROM tasks WHERE pr_repo = ? AND id != ? AND json_extract(pr_flow, '$.rec.branch') = ? \
         ORDER BY id DESC LIMIT 1",
        p![t.st("pr_repo"), t.id(), base],
    )?;
    Ok(row.filter(|r| r.s("pr_phase") != Some("merged")).map(|r| {
        format!("its base branch {base} is {}'s PR #{}, which hasn't merged yet", rf("task", r.id()), r.i0("pr_num"))
    }))
}

/// `tb pr merge`: checks the PR once more through its host, then merges it and deletes its branch.
pub fn merge(app: &App, id: i64, body: &Value) -> Result<Value> {
    let agent = as_bool(body.get("agent"), false);
    let via = body_str(body, "via_session");
    // The owner telling the agent in its terminal to merge this PR: a one-off merge where agents don't
    // merge on their own, and an "approved" in it stands in for the approvals.
    let before = board::get_task(app, id)?;
    if let Some(why) = crate::slots::visit_end_refusal(app, &before, body)? {
        return err(409, why);
    }
    let linked = PrRef::of(&before);
    let word = match &linked {
        Some(pr) if agent => crate::startword::owners_merge_word(app, &via, id, pr.num)?,
        _ => None,
    };
    if agent && word.is_none() && !prflow::agents_merge_on(app, before.s("project")) {
        return err(
            403,
            format!("{} merges this project's PRs (agents_merge is off), and hasn't told you to merge this one in this terminal. Run tb pr wait and leave it to them.", app.cfg.owner),
        );
    }
    let pr = pr_of(&before)?;
    let (t, rec) = fresh(app, id).map_err(|e| prflow::wait_out_limit(app, id, &pr, body, "merge", e))?;
    let site = prhost::limit::label(&pr.host);
    progress::stage(&format!("Read PR #{} from {site}", pr.num));
    let blockers = merge_blockers(app, &t, &rec, word.as_ref().is_some_and(|w| w.approved))?;
    if !blockers.is_empty() {
        let running = running_checks(&t, &rec);
        if !running.is_empty() {
            app.db.tx(|| prflow::note_checks_running(app, &t, rec["head"].as_str().unwrap_or(""), &running, board::BOARD))?;
        }
        return err(409, format!("PR #{} can't merge yet: {}.", pr.num, blockers.join("; ")));
    }
    progress::stage("Nothing blocks the merge");
    // The merge deletes this PR's branch, and a host may close a PR that goes into a deleted branch: move
    // the PRs stacked on it onto its base first.
    move_stacked_off(app, &t, &rec, pr.num)?;
    let rules = crate::projects::pr_rules(app, t.s("project"));
    let opts = MergeOpts { strategy: rules.merge_strategy.clone().filter(|s| !s.trim().is_empty()), close_source_branch: true };
    let h = host(app, &pr)?;
    if let Err(e) = on_host(&pr, "merging", || h.merge(&pr, &opts)) {
        // The host found a conflict the record didn't show yet: send the PR to the rebase stage, not to
        // merge retries.
        if merge_conflict_error(&e.to_string()) {
            app.db.tx(|| {
                prflow::note_merge_conflict(app, &t, &rec, board::BOARD)?;
                prflow::step(app, &board::get_task(app, id)?, &rec).map(|_| ())
            })?;
            return err(409, format!("PR #{} can't merge: it conflicts with its base branch {}. Rebase onto it, resolve the conflicts, push with git push --force-with-lease, then run tb pr wait.", pr.num, rec["base"].as_str().unwrap_or("main")));
        }
        return Err(prflow::wait_out_limit(app, id, &pr, body, "merge", ApiError::new(502, format!("Couldn't merge PR #{}: {e}.", pr.num))));
    }
    progress::stage(&format!("Merged PR #{} on {site}", pr.num));
    let who = match &word {
        Some(_) => format!("{} via {}", app.cfg.owner, board::session_name(app, Some(&via), None)),
        None => who_of(app, body),
    };
    app.db.tx(|| prflow::mark_merged(app, &board::get_task(app, id)?, &who))?;
    crate::reports::give_back_slot(app, id);
    let t = board::get_task(app, id)?;
    Ok(json!({"task": rf("task", id), "merged": true, "pr": board::pr_card(app, &t)}))
}

/// `tb pr close [--reason]`: closes the task's PR without merging it (Bitbucket declines it), keeping
/// its branch. An agent closes one only on the owner's word in its terminal.
pub fn close(app: &App, id: i64, body: &Value) -> Result<Value> {
    let agent = as_bool(body.get("agent"), false);
    let via = body_str(body, "via_session");
    let reason = one_line(&body_str(body, "reason"), 1000);
    let before = board::get_task(app, id)?;
    let linked = PrRef::of(&before);
    let word = match &linked {
        Some(pr) if agent => crate::startword::owners_close_word(app, &via, id, pr.num)?,
        _ => None,
    };
    if agent && word.is_none() {
        return err(403, format!("Only {} closes a PR, and they haven't told you to close this one in this terminal. Ask them.", app.cfg.owner));
    }
    let (t, rec) = fresh(app, id)?;
    let pr = pr_of(&t)?;
    match rec["state"].as_str().unwrap_or("OPEN").to_uppercase().as_str() {
        "MERGED" => return err(409, format!("PR #{} is already merged.", pr.num)),
        "CLOSED" | "DECLINED" | "SUPERSEDED" => {
            app.db.tx(|| prflow::step(app, &t, &rec).map(|_| ()))?;
            return err(409, format!("PR #{} is already closed.", pr.num));
        }
        _ => {}
    }
    let site = prhost::limit::label(&pr.host);
    let h = host(app, &pr)?;
    let comment = (!reason.is_empty()).then_some(reason.as_str());
    if let Err(e) = on_host(&pr, "closing", || h.close(&pr, comment)) {
        return err(502, format!("Couldn't close PR #{}: {e}.", pr.num));
    }
    progress::stage(&format!("Closed PR #{} on {site}", pr.num));
    let who = match &word {
        Some(_) => format!("{} via {}", app.cfg.owner, board::session_name(app, Some(&via), None)),
        None => who_of(app, body),
    };
    app.db.tx(|| prflow::mark_declined(app, &board::get_task(app, id)?, &who, &reason))?;
    let t = board::get_task(app, id)?;
    Ok(json!({"task": rf("task", id), "closed": true, "pr": board::pr_card(app, &t)}))
}

/// Who marks task `id`'s PR reviewed with `tb pr reviewed`: the owner, or an agent on the owner's word
/// in its terminal ("mark PR #9 reviewed"). An agent without that word is refused.
pub fn reviewed_by(app: &App, id: i64, body: &Value) -> Result<String> {
    let t = board::get_task(app, id)?;
    let pr = pr_of(&t)?;
    if !as_bool(body.get("agent"), false) {
        return Ok(app.cfg.owner.clone());
    }
    let via = body_str(body, "via_session");
    if crate::startword::owners_reviewed_word(app, &via, id, pr.num)?.is_none() {
        return err(403, format!("Only {} marks a PR reviewed, and they haven't told you in this terminal that they've reviewed PR #{}. Ask them, or they can use Mark reviewed on its task page.", app.cfg.owner, pr.num));
    }
    Ok(format!("{} via {}", app.cfg.owner, board::session_name(app, Some(&via), None)))
}

/// The host refused a merge because the PR conflicts with its base (GitHub: "the merge commit cannot be
/// cleanly created"; Bitbucket: "resolve all merge conflicts").
fn merge_conflict_error(e: &str) -> bool {
    let e = e.to_lowercase();
    e.contains("cleanly created") || e.contains("merge conflict")
}

/// Open PRs that go into this PR's branch: other tasks' PRs in its repo whose base is that branch, and
/// tasks stacked on it (`tb task set --stack-on`) whose PR hasn't been read yet.
fn stacked_children(app: &App, t: &Row, branch: &str) -> Result<Vec<Row>> {
    let rows = app.db.q(
        "SELECT * FROM tasks WHERE id != ? AND pr_num IS NOT NULL AND (pr_phase IS NULL OR pr_phase NOT IN ('merged', 'declined')) \
         AND ((pr_repo = ? AND json_extract(pr_flow, '$.rec.base') = ?) OR (pr_after = ? AND json_extract(pr_flow, '$.rec.base') IS NULL)) \
         ORDER BY id",
        p![t.id(), t.st("pr_repo"), branch, t.id()],
    )?;
    Ok(rows.into_iter().filter(board::pr_still_open).collect())
}

/// Points every open PR stacked on this one at this PR's base, before the merge deletes its branch. A
/// PR that can't be moved stops the merge, so it isn't closed with the branch.
fn move_stacked_off(app: &App, t: &Row, rec: &Value, num: i64) -> Result<()> {
    let (Some(branch), Some(base)) = (rec["branch"].as_str().filter(|b| !b.is_empty()), rec["base"].as_str().filter(|b| !b.is_empty())) else {
        return Ok(());
    };
    for child in stacked_children(app, t, branch)? {
        let what = format!("{} (moving PR #{} onto {base})", prhost::limit::label(&t.st("pr_host")), child.i0("pr_num"));
        let res = progress::waiting(&what, progress::WAITING_EVERY, || crate::propen::host::retarget(app, &child, base));
        app.db.tx(|| match &res {
            Ok(()) => {
                prflow::merge_flow(app, child.id(), fields!["retargeted" => base])?;
                board::log_event(app, child.id(), board::BOARD, "status", &format!("PR #{} now goes into {base}: {}'s PR #{num} is merging and its branch {branch} goes away", child.i0("pr_num"), rf("task", t.id())))
                    .map(|_| ())
            }
            Err(_) => Ok(()),
        })?;
        if let Err(e) = res {
            return err(
                502,
                format!(
                    "PR #{num} didn't merge: {}'s PR #{} goes into its branch {branch} and couldn't be moved onto {base} first ({e}). Point it at {base}, then merge again.",
                    rf("task", child.id()),
                    child.i0("pr_num")
                ),
            );
        }
        progress::stage(&format!("Moved stacked PR #{} onto {base}", child.i0("pr_num")));
    }
    Ok(())
}

/// A proof link's name for people: "GitHub Actions run 123", "Pipelines build #41", "Issue #12", a Jira
/// key, else the site it's on.
pub fn proof_label(url: &str) -> String {
    let cap = |re: &str| regex::Regex::new(re).ok().and_then(|r| r.captures(url)).map(|c| c[1].to_string());
    if let Some(n) = prhost::github::run_id(url) {
        return format!("GitHub Actions run {n}");
    }
    if let Some(n) = prhost::bitbucket::pipeline_build(url) {
        return format!("Pipelines build #{n}");
    }
    if let Some((_, n)) = prhost::ci::azure_build(url) {
        return format!("Azure build #{n}");
    }
    if let Some(n) = cap(r"github\.com/[^/]+/[^/]+/issues/(\d+)") {
        return format!("Issue #{n}");
    }
    if let Some(n) = cap(r"(?:github\.com/[^/]+/[^/]+/pull|bitbucket\.org/[^/]+/[^/]+/pull-requests)/(\d+)") {
        return format!("PR #{n}");
    }
    if let Some(k) = cap(r"/browse/([A-Z][A-Z0-9]+-\d+)") {
        return k;
    }
    if let Some(sha) = cap(r"/commits?/([0-9a-fA-F]{7,40})(?:[/?#]|$)") {
        return format!("Commit {}", &sha[..7]);
    }
    if cap(r"^https?://[^/?#]+(/(?:[^?#]*/)?artifacts?/)").is_some() {
        return "Evidence".into();
    }
    cap(r"^https?://(?:www\.)?([^/?#]+)").unwrap_or_else(|| url.to_string())
}

pub(crate) fn is_link(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    (l.starts_with("http://") || l.starts_with("https://")) && !s.contains(char::is_whitespace) && s.len() > 10
}

/// `tb pr not-ours`: clears failed checks of this push that aren't the PR's fault, with a reason and
/// links that prove it. Only failed checks of the current head can be cleared.
pub fn not_ours(app: &App, id: i64, body: &Value) -> Result<Value> {
    let reason = one_line(&body_str(body, "reason"), 1000);
    let n = reason.chars().count();
    if !(REASON_MIN..=REASON_MAX).contains(&n) {
        return err(400, format!("The reason is {n} characters: say why it isn't this PR in {REASON_MIN} to {REASON_MAX}."));
    }
    let title = one_line(&body_str(body, "title"), 1000);
    if title.is_empty() || title.chars().count() > TITLE_MAX {
        return err(400, format!("Give a title of 1 to {TITLE_MAX} characters naming what fails."));
    }
    let proof: Vec<String> = body["proof"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).collect()).unwrap_or_default();
    if proof.is_empty() {
        return err(400, "Add at least one --proof link: the same failure without this PR (a base-branch build, an issue).");
    }
    if let Some(bad) = proof.iter().find(|p| !is_link(p)) {
        return err(400, format!("{bad} isn't a link: proof is http(s) links."));
    }
    let (t, rec) = fresh(app, id)?;
    let pr = pr_of(&t)?;
    let head = rec["head"].as_str().unwrap_or("").to_string();
    let failed: Vec<String> = rec["failed"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    if failed.is_empty() || head.is_empty() {
        return err(409, format!("Nothing failed on PR #{}'s current push, so there's nothing to clear.", pr.num));
    }
    let checks = named_checks(body, &failed)?;
    let who = who_of(app, body);
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        let f = flow(&t);
        let mut all = f.get("not_ours").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        let mut cleared: Vec<String> = all.get(&head).map(|v| names_of(&v["checks"])).unwrap_or_default();
        for c in &checks {
            if !cleared.contains(c) {
                cleared.push(c.clone());
            }
        }
        all.insert(head.clone(), json!({"checks": cleared, "title": title, "reason": reason, "proof": proof, "who": who, "at": now_iso()}));
        prflow::merge_flow(app, id, fields!["not_ours" => Value::Object(all)])?;
        board::log_event(app, id, &who, "status", &format!("PR #{}: {} failed, but not because of this PR: {title}. {reason}", pr.num, checks.join(", ")))?;
        prflow::step(app, &board::get_task(app, id)?, &rec).map(|_| ())
    })?;
    let t = board::get_task(app, id)?;
    Ok(json!({"task": rf("task", id), "cleared": checks, "head": head, "pr": board::pr_card(app, &t)}))
}

/// The failed checks `--check` names (all of `failed` when it names none).
fn named_checks(body: &Value, failed: &[String]) -> Result<Vec<String>> {
    let mut checks = vec![];
    for c in &names_of(&body["checks"]) {
        match failed.iter().find(|f| f.eq_ignore_ascii_case(c)) {
            Some(f) => checks.push(f.clone()),
            None => return err(409, format!("{c} didn't fail on this push. Failed: {}.", failed.join(", "))),
        }
    }
    Ok(if checks.is_empty() { failed.to_vec() } else { checks })
}

/// `tb pr needs-owner`: this push's failed checks are ones only the owner can fix (a missing secret
/// or variable, a permissions error). Records what they have to do (`owner_act`), finishes the visit
/// and tells them; the card says the PR needs them instead of being fixed.
pub fn needs_owner(app: &App, id: i64, body: &Value) -> Result<Value> {
    let reason = one_line(&body_str(body, "reason"), 1000);
    let n = reason.chars().count();
    if !(REASON_MIN..=REASON_MAX).contains(&n) {
        return err(400, format!("The reason is {n} characters: say what the owner has to do in {REASON_MIN} to {REASON_MAX}."));
    }
    if let Some(why) = crate::slots::visit_end_refusal(app, &board::get_task(app, id)?, body)? {
        return err(409, why);
    }
    let (t, rec) = fresh(app, id)?;
    let pr = pr_of(&t)?;
    let head = rec["head"].as_str().unwrap_or("").to_string();
    let failed = prflow::failing(&flow(&t), &rec);
    if failed.is_empty() || head.is_empty() {
        return err(409, format!("Nothing fails on PR #{}'s current push, so there's nothing for the owner to fix.", pr.num));
    }
    let checks = named_checks(body, &failed)?;
    let who = who_of(app, body);
    app.db.tx(|| {
        prflow::merge_flow(app, id, fields!["owner_act" => json!({"head": head, "checks": checks, "reason": reason, "who": who, "at": now_iso()})])?;
        let t = board::get_task(app, id)?;
        prflow::waited(app, &t)?;
        board::log_event(app, id, &who, "status", &format!("PR #{}: {} failed, and only the owner can fix it: {reason}", pr.num, checks.join(", ")))?;
        crate::dispatch::add_alert(
            app,
            &format!("PR #{} for {} needs you: {reason}", pr.num, rf("task", id)),
            Some(id),
            t.i("goal_id"),
            None,
            Some("pr"),
        )
    })?;
    crate::reports::give_back_slot(app, id);
    let t = board::get_task(app, id)?;
    Ok(json!({"task": rf("task", id), "checks": checks, "head": head, "pr": board::pr_card(app, &t)}))
}

fn names_of(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default()
}

/// The full picture for `tb pr status`: the PR read now, each failed check's steps and tests (and
/// whether the base branch fails it too), the reviewers, open threads, whether the base moved, and
/// what stands between it and a merge.
pub fn status(app: &App, id: i64) -> Result<Value> {
    let t = board::get_task(app, id)?;
    let pr = pr_of(&t)?;
    let read = if prhost::watched(t.s("pr_host")) { on_host(&pr, "reading", || prflow::refresh_task(app, id))? } else { Err("the board doesn't watch this PR's host".into()) };
    let t = board::get_task(app, id)?;
    let f = flow(&t);
    let rec = match &read {
        Ok(r) => r.clone(),
        Err(_) => f.get("rec").cloned().unwrap_or(Value::Null),
    };
    let mut live = json!({"read_error": read.as_ref().err()});
    if !rec.is_object() {
        return Ok(live);
    }
    let cleared = names_of(&prflow::not_ours(&f, &rec)["checks"]);
    let failed = names_of(&rec["failed"]);
    let base_failing: std::result::Result<Vec<prhost::Check>, String> = match (read.is_ok(), failed.is_empty(), rec["base"].as_str()) {
        (true, false, Some(base)) if !base.is_empty() => host(app, &pr).map_err(|e| e.message).and_then(|h| on_host(&pr, &format!("reading {base}'s checks for"), || h.base_failed_checks(&pr, base, BASE_COMMITS))),
        _ => Ok(vec![]),
    };
    let rules = crate::projects::pr_rules(app, t.s("project"));
    let mut failures = vec![];
    for c in rec["checks"].as_array().cloned().unwrap_or_default().into_iter().filter(|c| c["state"] == "failed") {
        let Ok(check) = serde_json::from_value::<prhost::Check>(c.clone()) else { continue };
        let on_base: Vec<prhost::Check> = base_failing.as_ref().map(|b| b.iter().filter(|x| x.name.eq_ignore_ascii_case(&check.name)).cloned().collect()).unwrap_or_default();
        let mut row = json!({"check": check.name, "url": check.url, "cleared": cleared.iter().any(|x| x.eq_ignore_ascii_case(&check.name))});
        let mut mine: Option<prhost::ci::Failures> = None;
        if read.is_ok() {
            match prhost::ci::provider_for(app, &rules, &check) {
                Ok(Some(ci)) => {
                    row["source"] = json!(ci.name());
                    let what = format!("{} (reading {}'s failures on PR #{})", ci.name(), check.name, pr.num);
                    match progress::waiting(&what, progress::WAITING_EVERY, || ci.failures(&pr, rec["head"].as_str().unwrap_or(""), &check)) {
                        Ok(fl) => {
                            row["steps"] = json!(fl.steps);
                            row["tests"] = json!(fl.tests);
                            if let Some(e) = &fl.tests_error {
                                row["tests_error"] = json!(e);
                            }
                            mine = Some(fl);
                        }
                        Err(e) => row["error"] = json!(e),
                    }
                }
                Ok(None) => {}
                Err(e) => row["error"] = json!(e),
            }
        }
        // The same check failing on the base says little by itself: compare its failed steps and tests one
        // by one with the base's runs of it, when both can be read.
        let theirs = match &mine {
            Some(m) if !on_base.is_empty() && !(m.steps.is_empty() && m.tests.is_empty()) => {
                on_host(&pr, &format!("reading {}'s runs on the base for", check.name), || base_runs_failures(app, &rules, &pr, &on_base))
            }
            _ => None,
        };
        let b = blame(mine.as_ref(), theirs.as_ref(), !on_base.is_empty());
        row["base_fails"] = json!(b.all);
        row["base_steps"] = json!(b.steps);
        row["base_tests"] = json!(b.tests);
        row["base_compared"] = json!(b.compared);
        failures.push(row);
    }
    let review = prflow::review_of(&f, &rec);
    let off = prflow::swapped_off(&f);
    let reviewers: Vec<Value> = rec["reviewers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut r| {
            let gone = off.iter().any(|o| r["user"].as_str() == Some(o.as_str()));
            r["swapped_off"] = json!(gone);
            r["rereview"] = prflow::rereview_ask(&f, &r).map(|a| json!({"at": a["at"], "queued": a["queued"] == true})).unwrap_or(Value::Null);
            r
        })
        .collect();
    let builds_note = match prflow::checks_skipped(&f, &rec) {
        Some(why) => Some(format!("Checks skipped for this push: {why}")),
        None => {
            let stopped: Vec<String> = rec["checks"].as_array().cloned().unwrap_or_default().iter().filter(|c| c["state"] == "stopped").filter_map(|c| c["name"].as_str().map(|s| s.to_string())).collect();
            (!stopped.is_empty()).then(|| format!("Builds stopped: {}", stopped.join(", ")))
        }
    };
    let policy = prflow::rebase_policy(app, &t);
    live["base_moved"] = json!(prflow::base_moved(&f, &rec));
    live["conflicts"] = json!(prflow::conflicts(&rec));
    live["rebase_policy"] = json!(format!("{policy:?}").to_lowercase());
    live["rebase"] = json!(if prflow::needs_rebase(policy, &f, &rec) {
        rebase_commands(&app.cfg.pr_body.remote, rec["base"].as_str().unwrap_or(""), rec["branch"].as_str().unwrap_or(""), &review_gate(app, &t), &review_republish(app, &t))
    } else {
        vec![]
    });
    live["builds_note"] = json!(builds_note);
    live["failures"] = json!(failures);
    live["base_error"] = json!(base_failing.err());
    live["not_ours"] = prflow::not_ours(&f, &rec);
    let expected = prflow::expected_for(app, &t, &rec);
    live["expected"] = json!({"checks": expected.checks, "source": expected.source, "error": expected.error});
    live["expected_missing"] = json!(prflow::expected_missing(app, &t, &rec));
    live["expected_wait_mins"] = json!(prflow::expected_wait_mins(app, &t));
    live["expected_waited_out"] = json!(prflow::expected_wait_over(app, &t, &f, &rec));
    live["reviewers"] = json!(reviewers);
    live["approvals"] = json!({"have": review.approvals, "need": prflow::approvals_needed(app, &t)});
    live["open_threads"] = json!(prflow::open_threads(&f, &rec));
    live["tasks_open"] = rec["tasks_open"].clone();
    live["tasks_error"] = rec["tasks_error"].clone();
    live["blockers"] = json!(merge_blockers(app, &t, &rec, false)?);
    Ok(live)
}

/// What failed on the base too, for one failed check.
#[derive(Debug, Default, PartialEq)]
pub struct Blame {
    /// This PR's failed steps and tests that fail on the base too.
    pub steps: Vec<String>,
    pub tests: Vec<String>,
    /// Everything that failed here fails on the base too (or, compared by check, the check does).
    pub all: bool,
    /// How it was compared: "steps" (its steps and tests, one by one), "check" (only the check's name:
    /// the steps couldn't be read on one side), or null (the base doesn't fail the check).
    pub compared: Option<&'static str>,
}

/// A step's name without the error the CI added to it (Azure: "Run tests: exit 1").
fn step_key(s: &str) -> String {
    s.split(": ").next().unwrap_or(s).trim().to_lowercase()
}

/// Compares a failed check's steps and tests (`mine`) with the base's runs of the same check (`theirs`).
pub fn blame(mine: Option<&prhost::ci::Failures>, theirs: Option<&prhost::ci::Failures>, check_fails_on_base: bool) -> Blame {
    if !check_fails_on_base {
        return Blame::default();
    }
    let (Some(m), Some(b)) = (mine, theirs) else { return Blame { all: true, compared: Some("check"), ..Default::default() } };
    if m.steps.is_empty() && m.tests.is_empty() {
        return Blame { all: true, compared: Some("check"), ..Default::default() };
    }
    let base_steps: Vec<String> = b.steps.iter().map(|s| step_key(s)).collect();
    let steps: Vec<String> = m.steps.iter().filter(|s| base_steps.contains(&step_key(s))).cloned().collect();
    let tests: Vec<String> = m.tests.iter().filter(|t| b.tests.iter().any(|x| x.trim().eq_ignore_ascii_case(t.trim()))).cloned().collect();
    // With failing tests on both sides, the tests decide: the step that runs them fails either way.
    let all = if !m.tests.is_empty() { tests.len() == m.tests.len() } else { steps.len() == m.steps.len() };
    Blame { steps, tests, all, compared: Some("steps") }
}

/// The failed steps and tests of the base's runs of a check (each read once by its link), merged. None
/// when none of them could be read.
fn base_runs_failures(app: &App, rules: &crate::config::PrProject, pr: &PrRef, runs: &[prhost::Check]) -> Option<prhost::ci::Failures> {
    let mut out: Option<prhost::ci::Failures> = None;
    let mut seen: Vec<Option<String>> = vec![];
    for run in runs.iter().take(BASE_COMMITS) {
        if run.url.is_none() || seen.contains(&run.url) {
            continue;
        }
        seen.push(run.url.clone());
        let Ok(Some(ci)) = prhost::ci::provider_for(app, rules, run) else { continue };
        let Ok(fl) = ci.failures(pr, "", run) else { continue };
        let o = out.get_or_insert_with(Default::default);
        o.steps.extend(fl.steps);
        o.tests.extend(fl.tests);
    }
    out
}

/// What to run when the base moved under this push: rebase onto it, test, pass the owner's review
/// gate (each `per_head` step, as `gate` says how) on the new commit, push the branch, republish the
/// owner's review for the pushed head (`republish`), and leave it alone if there's nothing else to push.
pub fn rebase_commands(remote: &str, base: &str, branch: &str, gate: &[String], republish: &[String]) -> Vec<String> {
    if base.is_empty() {
        return vec![];
    }
    let mut out = vec![format!("git fetch {remote} {base} && git rebase {remote}/{base}"), "run the tests covering your change again (not the whole suite)".into()];
    out.extend(gate.iter().cloned());
    out.push(if branch.is_empty() { "git push --force-with-lease".into() } else { format!("git push --force-with-lease {remote} {branch}") });
    out.extend(republish.iter().cloned());
    out.push("Don't push only to rebase.".into());
    out
}

/// The owner's review gate for a push: how to pass each of the task's `per_head` steps.
fn review_gate(app: &App, t: &Row) -> Vec<String> {
    let tb = board::tb_cmd(app);
    crate::steps::for_task(app, t).into_iter().filter(|s| s.per_head).map(|s| s.how(&tb)).collect()
}

/// After the push: republish the passing round for the pushed head of each `per_head` step the PR shows,
/// as the Python board said: with `tb step publish` when it has a `publish` script, else in words.
fn review_republish(app: &App, t: &Row) -> Vec<String> {
    let tb = board::tb_cmd(app);
    crate::steps::for_task(app, t).into_iter().filter_map(|s| republish_line(&tb, &s)).collect()
}

/// How to republish a step for the pushed head; none for a step nothing on the PR shows (it isn't per
/// head, or has neither a `publish` script nor a `bar` name in the PR bar).
fn republish_line(tb: &str, s: &crate::steps::Step) -> Option<String> {
    if !s.per_head {
        None
    } else if !s.publish.trim().is_empty() {
        Some(s.republish(tb))
    } else if !s.bar.trim().is_empty() {
        Some(format!("publish {}'s round for the pushed head where the PR shows it", s.name))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prhost::ci::Failures;

    fn fl(steps: &[&str], tests: &[&str]) -> Failures {
        Failures { steps: steps.iter().map(|s| s.to_string()).collect(), tests: tests.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn the_base_is_blamed_only_for_the_steps_and_tests_it_fails_too() {
        let mine = fl(&["test › cargo test"], &["login_works", "logout_works"]);
        let b = blame(Some(&mine), Some(&fl(&["test › cargo test"], &["login_works"])), true);
        assert_eq!((b.tests.clone(), b.all, b.compared), (vec!["login_works".to_string()], false, Some("steps")), "logout_works is this PR's");
        assert_eq!(b.steps, vec!["test › cargo test".to_string()]);
        let b = blame(Some(&mine), Some(&fl(&[], &["LOGIN_WORKS", "logout_works"])), true);
        assert!(b.all);
        let b = blame(Some(&fl(&["Run tests: exit 1"], &[])), Some(&fl(&["Run tests: exit 2"], &[])), true);
        assert!(b.all, "a step is its name, not the error the CI added");
        assert_eq!(blame(Some(&mine), None, true), Blame { all: true, compared: Some("check"), ..Default::default() }, "the base's steps couldn't be read");
        assert_eq!(blame(Some(&mine), Some(&mine), false), Blame::default(), "the base doesn't fail the check");
    }

    #[test]
    fn rebase_commands_name_the_remote_and_base() {
        let gate = vec!["tb step run \"Owner review\"; fix what it reports and run it again".to_string()];
        let republish = vec!["tb step publish \"Owner review\"".to_string()];
        let cmds = rebase_commands("origin", "main", "feat/x", &gate, &republish);
        assert_eq!(cmds[0], "git fetch origin main && git rebase origin/main");
        assert_eq!(cmds[2], gate[0], "the owner's review gate runs before the push");
        assert_eq!(cmds[3], "git push --force-with-lease origin feat/x");
        assert_eq!(cmds[4], republish[0], "the owner's review is republished for the pushed head");
        assert_eq!(cmds[5], "Don't push only to rebase.");
        assert_eq!(rebase_commands("origin", "main", "", &[], &[])[2], "git push --force-with-lease");
        assert_eq!(rebase_commands("origin", "main", "feat/x", &gate, &[])[4], "Don't push only to rebase.");
        assert!(rebase_commands("origin", "", "feat/x", &[], &[]).is_empty());
    }

    #[test]
    fn a_per_head_step_the_pr_shows_is_republished_with_or_without_a_publish_script() {
        let step = |extra: &str| crate::steps::parse(&format!("[[steps]]\nname = \"Owner review\"\nprompt = \"x\"\nper_head = true\n{extra}")).unwrap().remove(0);
        assert_eq!(republish_line("tb", &step("bar = \"WD\"\n")).as_deref(), Some("publish Owner review's round for the pushed head where the PR shows it"));
        assert_eq!(republish_line("tb", &step("publish = \"wd review publish\"\n")).as_deref(), Some("tb step publish \"Owner review\""));
        // Nothing on the PR shows it: no advice to republish it.
        assert_eq!(republish_line("tb", &step("")), None);
        let s = crate::steps::parse("[[steps]]\nname = \"Lint\"\nrun = \"make lint\"\nbar = \"L\"\n").unwrap();
        assert_eq!(republish_line("tb", &s[0]), None, "a step that isn't per head isn't republished");
    }

    #[test]
    fn proof_links_are_named_for_people() {
        assert_eq!(proof_label("https://github.com/a/b/actions/runs/123/job/4"), "GitHub Actions run 123");
        assert_eq!(proof_label("https://bitbucket.org/w/r/pipelines/results/41"), "Pipelines build #41");
        assert_eq!(proof_label("https://dev.azure.com/o/P/_build/results?buildId=9"), "Azure build #9");
        assert_eq!(proof_label("https://github.com/a/b/issues/12"), "Issue #12");
        assert_eq!(proof_label("https://bitbucket.org/w/r/pull-requests/7"), "PR #7");
        assert_eq!(proof_label("https://acme.atlassian.net/browse/WEB-12"), "WEB-12");
        assert_eq!(proof_label("https://github.com/a/b/commit/abc1234def5678"), "Commit abc1234");
        assert_eq!(proof_label("https://bitbucket.org/w/r/commits/abc1234def5678"), "Commit abc1234");
        assert_eq!(proof_label("https://claude.ai/code/artifact/0b1c"), "Evidence");
        assert_eq!(proof_label("https://claude.ai/artifact/0b1c"), "Evidence");
        assert_eq!(proof_label("https://ci.example.com/job/7/artifacts/log.txt"), "Evidence");
        assert_eq!(proof_label("https://www.ci.example.com/b/1"), "ci.example.com");
    }

    #[test]
    fn proof_must_be_links() {
        assert!(is_link("https://ci.example.com/build/1"));
        assert!(!is_link("see build 1"));
        assert!(!is_link("https://a b"));
    }
}
