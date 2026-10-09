//! What `tb pr …` does to a task's PR through its host (`prhost.rs`): the full status, replying to and
//! acknowledging threads, `addressed` (re-request review), the guarded merge, and clearing a failure
//! that isn't the PR's (`not-ours`).
//!
//! Each reads the PR fresh first (`prflow::refresh_task`), so it judges what the host says now, and
//! never calls the host inside a database transaction.

use serde_json::{json, Value};

use crate::app::App;
use crate::prhost::{self, MergeOpts, PrRef, Thread};
use crate::util::*;
use crate::{board, fields, p, prflow};

/// How many of the base branch's latest commits `tb pr status` compares a failure against.
pub const BASE_COMMITS: usize = 5;
pub const REASON_MIN: usize = 20;
pub const REASON_MAX: usize = 300;
pub const TITLE_MAX: usize = 80;

fn pr_of(t: &Row) -> Result<PrRef> {
    match PrRef::of(t) {
        Some(p) => Ok(p),
        None => err(404, format!("{} has no PR linked.", rf("task", t.id()))),
    }
}

fn host(app: &App, pr: &PrRef) -> Result<std::sync::Arc<dyn prhost::PrHost>> {
    prhost::host_for(app, &pr.host).map_err(|e| ApiError::new(409, format!("Can't work with PR #{}: {e}.", pr.num)))
}

/// Reads the PR now and steps it; the task and the record.
fn fresh(app: &App, id: i64) -> Result<(Row, Value)> {
    let t = board::get_task(app, id)?;
    let pr = pr_of(&t)?;
    match prflow::refresh_task(app, id)? {
        Ok(rec) => Ok((board::get_task(app, id)?, rec)),
        Err(e) => err(502, format!("Couldn't read PR #{}: {e}.", pr.num)),
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

fn who_of(body: &Value) -> String {
    let w = body_str(body, "who");
    if w.is_empty() {
        "the owner".to_string()
    } else {
        w
    }
}

/// `tb pr reply <thread> "<text>" [--resolve]`.
pub fn reply(app: &App, id: i64, body: &Value) -> Result<Value> {
    let text = body_str(body, "text").trim().to_string();
    if text.is_empty() {
        return err(400, "Say what you did: the reply can't be empty.");
    }
    let (t, rec) = fresh(app, id)?;
    let pr = pr_of(&t)?;
    let th = find_thread(&rec, pr.num, &body_str(body, "thread"))?;
    let h = host(app, &pr)?;
    h.reply(&pr, &th, &text).map_err(|e| ApiError::new(502, format!("Couldn't reply on PR #{}: {e}.", pr.num)))?;
    let resolve = as_bool(body.get("resolve"), false);
    let resolved = if resolve { h.resolve(&pr, &th).map_err(|e| ApiError::new(502, format!("Replied, but couldn't resolve the thread: {e}.")))? } else { false };
    app.db.tx(|| {
        // A plain comment has no thread on the host to carry the reply: the board remembers it was answered.
        if !th.resolvable || resolve {
            ack_locally(app, id, &th)?;
        }
        let what = if resolved { "Replied to and resolved" } else { "Replied to" };
        board::log_event(app, id, &who_of(body), "status", &format!("{what} {} on PR #{}: {}", thread_name(&th), pr.num, one_line(&text, 200))).map(|_| ())
    })?;
    after(app, id, json!({"thread": th.id, "resolved": resolved}))
}

/// `tb pr ack <thread>`: resolved without a reply (on the board only, where the host can't resolve it).
pub fn ack(app: &App, id: i64, body: &Value) -> Result<Value> {
    let (t, rec) = fresh(app, id)?;
    let pr = pr_of(&t)?;
    let th = find_thread(&rec, pr.num, &body_str(body, "thread"))?;
    let resolved = if th.resolvable && !th.resolved {
        host(app, &pr)?.resolve(&pr, &th).map_err(|e| ApiError::new(502, format!("Couldn't resolve the thread: {e}.")))?
    } else {
        false
    };
    app.db.tx(|| {
        ack_locally(app, id, &th)?;
        board::log_event(app, id, &who_of(body), "status", &format!("Acknowledged {} on PR #{} without a reply", thread_name(&th), pr.num)).map(|_| ())
    })?;
    after(app, id, json!({"thread": th.id, "resolved": resolved}))
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
fn after(app: &App, id: i64, mut out: Value) -> Result<Value> {
    let _ = prflow::refresh_task(app, id)?;
    let t = board::get_task(app, id)?;
    out["task"] = json!(rf("task", id));
    out["pr"] = board::pr_card(&t);
    out["open_threads"] = json!(prflow::open_threads(&flow(&t), &flow(&t).get("rec").cloned().unwrap_or(Value::Null)).len());
    Ok(out)
}

/// `tb pr addressed`: refuses while threads are open; then asks each reviewer with a standing request
/// for changes (not one swapped off) to review again, and ends the visit.
pub fn addressed(app: &App, id: i64, body: &Value) -> Result<Value> {
    let (t, rec) = fresh(app, id)?;
    let pr = pr_of(&t)?;
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
    if !users.is_empty() {
        host(app, &pr)?.re_request_reviews(&pr, &users).map_err(|e| ApiError::new(502, format!("Couldn't ask for another review on PR #{}: {e}.", pr.num)))?;
    }
    let asked: Vec<String> = review.requesters.iter().map(|(u, n)| if n.is_empty() { u.clone() } else { n.clone() }).collect();
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        prflow::waited(app, &t)?;
        prflow::merge_flow(
            app,
            id,
            fields!["answered_changes" => rec["changes_at"].clone(),
                    "addressed" => json!({"at": now_iso(), "head": rec["head"], "asked": asked, "asked_ids": users})],
        )?;
        let text = if asked.is_empty() {
            format!("Addressed the review on PR #{}", pr.num)
        } else {
            format!("Addressed the review on PR #{} and asked {} to look again", pr.num, asked.join(", "))
        };
        board::log_event(app, id, &who_of(body), "status", &text).map(|_| ())
    })?;
    after(app, id, json!({"asked": asked}))
}

/// Why the PR can't merge right now (empty: it can). Checks the record as just read.
pub fn merge_blockers(app: &App, t: &Row, rec: &Value) -> Result<Vec<String>> {
    let f = flow(t);
    let mut out = vec![];
    let state = rec["state"].as_str().unwrap_or("OPEN").to_uppercase();
    if state != "OPEN" {
        out.push(format!("it's {}", state.to_lowercase()));
        return Ok(out);
    }
    if prflow::checks_skipped(&f, rec).is_none() {
        let failing = prflow::failing(&f, rec);
        if !failing.is_empty() {
            out.push(format!("checks failed: {}", failing.join(", ")));
        }
        let checks = rec["checks"].as_array().cloned().unwrap_or_default();
        let running: Vec<String> = checks.iter().filter(|c| c["state"] == "running").filter_map(|c| c["name"].as_str().map(|s| s.to_string())).collect();
        if !running.is_empty() {
            out.push(format!("checks are still running: {}", running.join(", ")));
        }
        let stopped: Vec<String> = checks.iter().filter(|c| c["state"] == "stopped").filter_map(|c| c["name"].as_str().map(|s| s.to_string())).collect();
        if !stopped.is_empty() {
            out.push(format!("builds were stopped: {} (run them again, or tb pr skip-checks)", stopped.join(", ")));
        }
        if let Some(missing) = prflow::expected_missing(app, t, rec).filter(|m| !m.is_empty()) {
            out.push(format!("expected checks haven't posted: {}", missing.join(", ")));
        }
    }
    if !prflow::review_skipped(&f, rec) {
        let review = prflow::review_of(&f, rec);
        if review.changes {
            let who: Vec<String> = review.requesters.iter().map(|(u, n)| if n.is_empty() { u.clone() } else { n.clone() }).collect();
            out.push(if who.is_empty() { "a reviewer asked for changes".into() } else { format!("{} asked for changes", who.join(", ")) });
        } else if !prflow::approved(app, t, &review) {
            out.push(match prflow::approvals_needed(app, t) {
                Some(n) => format!("it has {} of the {} approvals it needs", review.approvals, n),
                None => "it isn't approved".into(),
            });
        }
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
    if let Some(b) = stacked_base(app, t, rec)? {
        out.push(b);
    }
    Ok(out)
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
    if as_bool(body.get("agent"), false) && !app.cfg.pr.agents_merge {
        return err(403, format!("{} merges PRs on this board (pr.agents_merge is off). Run tb pr wait and leave it to them.", app.cfg.owner));
    }
    let (t, rec) = fresh(app, id)?;
    let pr = pr_of(&t)?;
    let blockers = merge_blockers(app, &t, &rec)?;
    if !blockers.is_empty() {
        return err(409, format!("PR #{} can't merge yet: {}.", pr.num, blockers.join("; ")));
    }
    let rules = app.cfg.pr.project(t.s("project"));
    let opts = MergeOpts { strategy: rules.merge_strategy.clone().filter(|s| !s.trim().is_empty()), close_source_branch: true };
    host(app, &pr)?.merge(&pr, &opts).map_err(|e| ApiError::new(502, format!("Couldn't merge PR #{}: {e}.", pr.num)))?;
    app.db.tx(|| prflow::mark_merged(app, &board::get_task(app, id)?, &who_of(body)))?;
    let t = board::get_task(app, id)?;
    Ok(json!({"task": rf("task", id), "merged": true, "pr": board::pr_card(&t)}))
}

fn is_link(s: &str) -> bool {
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
    let asked: Vec<String> = body["checks"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    let mut checks = vec![];
    for c in &asked {
        match failed.iter().find(|f| f.eq_ignore_ascii_case(c)) {
            Some(f) => checks.push(f.clone()),
            None => return err(409, format!("{c} didn't fail on this push. Failed: {}.", failed.join(", "))),
        }
    }
    if checks.is_empty() {
        checks = failed.clone();
    }
    let who = who_of(body);
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
    Ok(json!({"task": rf("task", id), "cleared": checks, "head": head, "pr": board::pr_card(&t)}))
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
    let read = if prhost::watched(t.s("pr_host")) { prflow::refresh_task(app, id)? } else { Err("the board doesn't watch this PR's host".into()) };
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
    let base_failing: std::result::Result<Vec<String>, String> = match (read.is_ok(), failed.is_empty(), rec["base"].as_str()) {
        (true, false, Some(base)) if !base.is_empty() => host(app, &pr).map_err(|e| e.message).and_then(|h| h.base_failures(&pr, base, BASE_COMMITS)),
        _ => Ok(vec![]),
    };
    let rules = app.cfg.pr.project(t.s("project"));
    let mut failures = vec![];
    for c in rec["checks"].as_array().cloned().unwrap_or_default().into_iter().filter(|c| c["state"] == "failed") {
        let Ok(check) = serde_json::from_value::<prhost::Check>(c.clone()) else { continue };
        let mut row = json!({"check": check.name, "url": check.url, "cleared": cleared.iter().any(|x| x.eq_ignore_ascii_case(&check.name)),
                             "base_fails": base_failing.as_ref().map(|b| b.iter().any(|x| x.eq_ignore_ascii_case(&check.name))).unwrap_or(false)});
        if read.is_ok() {
            match prhost::ci::provider_for(app, &rules, &check) {
                Ok(Some(ci)) => {
                    row["source"] = json!(ci.name());
                    match ci.failures(&pr, rec["head"].as_str().unwrap_or(""), &check) {
                        Ok(fl) => {
                            row["steps"] = json!(fl.steps);
                            row["tests"] = json!(fl.tests);
                        }
                        Err(e) => row["error"] = json!(e),
                    }
                }
                Ok(None) => {}
                Err(e) => row["error"] = json!(e),
            }
        }
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
    live["base_moved"] = json!(prflow::base_moved(&f, &rec));
    live["builds_note"] = json!(builds_note);
    live["failures"] = json!(failures);
    live["base_error"] = json!(base_failing.err());
    live["not_ours"] = prflow::not_ours(&f, &rec);
    live["expected_missing"] = json!(prflow::expected_missing(app, &t, &rec));
    live["reviewers"] = json!(reviewers);
    live["approvals"] = json!({"have": review.approvals, "need": prflow::approvals_needed(app, &t)});
    live["open_threads"] = json!(prflow::open_threads(&f, &rec));
    live["tasks_open"] = rec["tasks_open"].clone();
    live["blockers"] = json!(merge_blockers(app, &t, &rec)?);
    Ok(live)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_must_be_links() {
        assert!(is_link("https://ci.example.com/build/1"));
        assert!(!is_link("see build 1"));
        assert!(!is_link("https://a b"));
    }
}
