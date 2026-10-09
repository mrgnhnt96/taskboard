//! Pull requests after a task is done: watch PRs on their host (GitHub or Bitbucket Cloud, see
//! `prhost.rs`), work out their stage, and bring the task's conversation back when the PR needs work
//! (a failed check, open review threads, ready to merge).
//!
//! Besides the latest read (`rec`), a task's `pr_flow` keeps the board's own state of the PR:
//! `threads_acked` (thread id → the last comment it was acknowledged at, by `tb pr ack` or a reply to a
//! comment the host can't resolve), `not_ours` (head → the failed checks cleared for that push, with
//! the reason and proof), `head_base` (head → the base's commit when that push was first seen),
//! `swapped_off` (reviewers taken off the PR, whose requests for changes no longer hold) and
//! `addressed` (the last `tb pr addressed`).

use std::path::Path;

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, dispatch, fields, handoff, hooks, hours, limits, p, runner};

pub const WAKE: &[&str] = &["fix", "comments", "merge"];
pub const IN_REVIEW: &[&str] = &["review", "rereview", "comments", "merge", "waits"];
const FINISHED: &[&str] = &["merged", "declined"];
const WAKE_RETRY_WAITS: [i64; 3] = [60, 300, 900];
/// A merge the agent said it finished (`tb pr wait`) that's still open is brought back after each of these.
const MERGE_RETRY_WAITS: [i64; 3] = [60, 300, 900];

pub fn label(phase: &str) -> &str {
    match phase {
        "checks" => "Watching checks",
        "fix" => "Fixing checks",
        "review" => "Awaiting review",
        "rereview" => "Awaiting re-review",
        "comments" => "Addressing comments",
        "merge" => "Ready to merge",
        "waits" => "Waits on base",
        "merged" => "Merged",
        "declined" => "Closed",
        other => other,
    }
}

fn flow(t: &Row) -> Row {
    jloads_obj(t.s("pr_flow"))
}

pub fn merge_flow(app: &App, task_id: i64, changes: Vec<(&str, Value)>) -> Result<Row> {
    let row = app.db.q1("SELECT pr_flow FROM tasks WHERE id = ?", p![task_id])?;
    let mut f = jloads_obj(row.as_ref().and_then(|r| r.s("pr_flow")));
    for (k, v) in changes {
        if v.is_null() {
            f.remove(k);
        } else {
            f.insert(k.to_string(), v);
        }
    }
    board::update_task(app, task_id, fields!["pr_flow" => jdumps(&Value::Object(f.clone()))])?;
    Ok(f)
}

/// Links a PR found in an agent's report to its task (only the first one sticks).
pub fn link_pr(app: &App, t: &Row, pr: &PrLink, who: &str) -> Result<bool> {
    if t.s("pr_url") == Some(pr.url.as_str()) || has(t.s("pr_repo")) {
        return Ok(false);
    }
    if app.db.q1("SELECT id FROM tasks WHERE pr_repo = ? AND pr_num = ? AND id != ?", p![pr.repo, pr.num, t.id()])?.is_some() {
        return Ok(false);
    }
    let watched = crate::prhost::watched(Some(&pr.host)) && app.cfg.pr.watch;
    board::update_task(
        app,
        t.id(),
        fields!["pr_host" => pr.host, "pr_repo" => pr.repo, "pr_num" => pr.num, "pr_url" => pr.url,
                "pr_state" => "OPEN", "pr_build" => "Unknown", "pr_review" => "Not reviewed yet",
                "pr_phase" => if watched { Some("checks") } else { None }],
    )?;
    board::log_event(app, t.id(), who, "status", &format!("Linked PR #{} ({})", pr.num, pr.repo))?;
    Ok(true)
}

pub fn card(t: &Row) -> Value {
    let Some(phase) = t.s("pr_phase").filter(|p| !p.is_empty()) else { return Value::Null };
    if t.i("pr_num").is_none() {
        return Value::Null;
    }
    let f = flow(t);
    let waking = WAKE.contains(&phase);
    let rec = f.get("rec").cloned().unwrap_or(Value::Null);
    json!({
        "phase": phase, "label": label(phase),
        "session": if waking { f.v("session") } else { Value::Null },
        "stopped": if waking { f.v("stopped") } else { Value::Null },
        "failed_checks": rec.get("failed").cloned().unwrap_or(json!([])),
        "comments": rec.get("comments").cloned().unwrap_or(json!(0)),
        "open_threads": open_threads(&f, &rec).len(),
        "not_ours": not_ours(&f, &rec),
        "review_decision": rec.get("review_decision").cloned().unwrap_or(Value::Null),
        "checked_at": f.v("checked_at"),
        "awaiting_you": t.s("status") == Some("done") && awaiting_owner(t),
    })
}

/// A green PR nobody has reviewed yet: the owner may want to look before peers do.
pub fn awaiting_owner(t: &Row) -> bool {
    t.s("pr_phase") == Some("review") && !flow(t).contains_key("reviewed")
}

pub fn wake_stuck(t: &Row) -> bool {
    let f = flow(t);
    WAKE.contains(&t.s("pr_phase").unwrap_or(""))
        && (f.contains_key("wake_tries") || (t.s("pr_phase") == Some("merge") && f.contains_key("merge_gave_up")))
}

pub fn live_job(app: &App, t: &Row) -> Result<Option<Row>> {
    app.db.q1(
        "SELECT id FROM jobs WHERE task_id = ? AND kind = 'agent' AND purpose = 'pr' AND state IN ('pending', 'running')",
        p![t.id()],
    )
}

pub fn waking(app: &App, t: &Row) -> Result<bool> {
    Ok(live_job(app, t)?.is_some() || (WAKE.contains(&t.s("pr_phase").unwrap_or("")) && flow(t).contains_key("woke")))
}

pub fn visited_by(app: &App, sid: &str) -> Result<Option<Row>> {
    if sid.is_empty() {
        return Ok(None);
    }
    app.db.q1(
        "SELECT * FROM tasks WHERE status = 'done' AND pr_num IS NOT NULL AND json_extract(pr_flow, '$.session') = ? \
         ORDER BY updated_at DESC LIMIT 1",
        p![sid],
    )
}

pub fn visitor(t: &Row) -> Option<String> {
    if t.s("status") != Some("done") || !WAKE.contains(&t.s("pr_phase").unwrap_or("")) {
        return None;
    }
    flow(t).s("session").map(|s| s.to_string())
}

pub fn picked_up(app: &App, t: &Row, sid: &str, name: &str) -> Result<()> {
    if sid.is_empty() || flow(t).s("session") == Some(sid) {
        return Ok(());
    }
    let phase = t.st("pr_phase");
    board::add_terminal(app, t.id(), sid, label(&phase))?;
    board::upsert_session(app, sid, fields!["last_task" => t.id()])?;
    merge_flow(app, t.id(), fields!["session" => sid, "wake_tries" => null, "retry_at" => null])?;
    board::log_event(app, t.id(), name, "status", &format!("Picked up PR #{}: {}", t.i0("pr_num"), label(&phase).to_lowercase()))?;
    Ok(())
}

pub fn resumed(app: &App, t: &Row) -> Result<()> {
    if flow(t).contains_key("stopped") {
        merge_flow(app, t.id(), fields!["stopped" => null])?;
    }
    Ok(())
}

pub fn stopped(app: &App, t: &Row, name: &str, message: &str, asked: bool, wrote: Option<&str>) -> Result<bool> {
    let f = flow(t);
    if !WAKE.contains(&t.s("pr_phase").unwrap_or("")) || !f.contains_key("woke") {
        return Ok(false);
    }
    let message = if message.is_empty() { "It stopped without saying why." } else { message };
    let before = f.get("stopped").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    if before.s("message") == Some(message) || (before.b("asked") && !asked) {
        return Ok(true);
    }
    let mut st = json!({"at": now_iso(), "message": message});
    if asked {
        st["asked"] = json!(true);
    }
    merge_flow(app, t.id(), vec![("stopped", st)])?;
    let num = t.i0("pr_num");
    let mut text = if asked {
        format!("Asked you on PR #{num}: {message}")
    } else {
        format!("Stopped on PR #{num} before it was finished: {message}")
    };
    if let Some(w) = wrote {
        text += &format!(" (The agent wrote: “{w}”)");
    }
    board::log_event(app, t.id(), name, "question", &text)?;
    Ok(true)
}

pub fn wake_failed(app: &App, t: &Row, text: &str) -> Result<bool> {
    let t = board::get_task(app, t.id())?;
    let f = flow(&t);
    if t.s("status") != Some("done") || !WAKE.contains(&t.s("pr_phase").unwrap_or("")) || !f.contains_key("woke") {
        return Ok(false);
    }
    let tries = f.i0("wake_tries") + 1;
    let wait = WAKE_RETRY_WAITS[(tries.min(3) - 1) as usize];
    merge_flow(app, t.id(), fields!["woke" => null, "wake_tries" => tries, "retry_at" => iso(now_ts() + wait as f64)])?;
    let mins = wait / 60;
    let text = text.trim_end_matches(['.', ' ']);
    board::log_event(
        app,
        t.id(),
        board::MIDNA,
        "midna",
        &format!("Couldn't reopen it for PR #{}: {text}. Trying again in {}", t.i0("pr_num"), plural(mins, "minute")),
    )?;
    if tries == WAKE_RETRY_WAITS.len() as i64 + 1 {
        dispatch::add_alert(
            app,
            &format!(
                "{} couldn't be reopened for PR #{} after {tries} tries: {text}. The board keeps trying every {mins} minutes.",
                rf("task", t.id()),
                t.i0("pr_num")
            ),
            Some(t.id()),
            t.i("goal_id"),
            None,
            Some("pr"),
        )?;
    }
    Ok(true)
}

fn wake_key(phase: &str, rec: &Value) -> String {
    let key = format!("{phase}:{}:{}", rec["head"].as_str().unwrap_or(""), rec["comments"].as_i64().unwrap_or(0));
    if phase != "merge" {
        return key;
    }
    // A change in the reviews wakes the merge again.
    format!("{key}:{}:{}", rec["review_decision"].as_str().unwrap_or(""), rec["approvals"].as_i64().unwrap_or(0))
}

fn since_handled(f: &Row) -> f64 {
    now_ts() - f.s("handled_at").and_then(parse_iso).unwrap_or(0.0)
}

/// The agent finished its merge visit but the PR is still open: time to bring it back (1, 5, then 15 minutes on).
fn merge_stalled(f: &Row, key: &str) -> bool {
    let tries = f.i0("merge_tries") as usize;
    f.s("handled") == Some(key) && !f.contains_key("woke") && tries < MERGE_RETRY_WAITS.len() && since_handled(f) >= MERGE_RETRY_WAITS[tries] as f64
}

/// Every retry is spent and it's still open: alert once.
fn merge_given_up(f: &Row, key: &str) -> bool {
    f.s("handled") == Some(key)
        && !f.contains_key("woke")
        && !f.contains_key("merge_gave_up")
        && f.i0("merge_tries") as usize >= MERGE_RETRY_WAITS.len()
        && since_handled(f) >= MERGE_RETRY_WAITS[MERGE_RETRY_WAITS.len() - 1] as f64
}

fn retry_later(f: &Row) -> bool {
    f.s("retry_at").map(|r| r > now_iso().as_str()).unwrap_or(false)
}

fn prompt(app: &App, t: &Row, phase: &str, rec: &Value) -> String {
    let tb = board::tb_cmd(app);
    let r = rf("task", t.id());
    let pr = format!("PR #{} ({})", t.i0("pr_num"), t.st("pr_url"));
    let head = format!("[task-board:{r}] {pr} for “{}” needs you.", t.st("title"));
    let failed: Vec<String> = rec["failed"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    let body = match phase {
        "fix" => format!(
            "A check failed on its head ({}). Run {tb} pr status {r}: it lists each failed check's failed steps and tests \
             where it can read them, and marks a check the base branch fails too. Read the failing check's log when that \
             isn't enough (for GitHub: gh pr checks {num} and gh run view --log-failed). Work from that error, not a guess. \
             If the PR's change causes it, fix exactly that in this task's worktree, run the tests, and push; the checks \
             run again. If the base branch has moved, rebase onto it before you push (git push --force-with-lease). If the \
             failure isn't this PR's (the base branch fails it too), don't change unrelated code: clear it with {tb} pr \
             not-ours {r} --check \"<check>\" --title \"<what fails>\" --reason \"<why it isn't this PR>\" --proof <link to \
             the same failure without this change>.",
            if failed.is_empty() { "see the PR".to_string() } else { failed.join(", ") },
            num = t.i0("pr_num")
        ),
        "comments" => format!(
            "There are open review threads{}. {tb} pr status {r} lists them with their ids. Address each in the worktree; \
             one that can fairly wait for a later PR goes to the backlog instead ({tb} backlog add \"<title>\" --kind \
             follow). Push the fixes, then answer each thread that asked for something with {tb} pr reply {r} <thread> \
             \"<what you did>\" --resolve. A comment that asks for nothing gets {tb} pr ack {r} <thread>: it's resolved \
             without a reply.{}",
            if rec["review_decision"] == "CHANGES_REQUESTED" { " and a reviewer asked for changes" } else { "" },
            if rec["review_decision"] == "CHANGES_REQUESTED" {
                format!(" When every thread is answered, run {tb} pr addressed {r}: it asks the reviewers who wanted changes to look again.")
            } else {
                String::new()
            }
        ),
        "merge" => format!(
            "It's approved and every check is green. Merge it with {tb} pr merge {r}: it checks the PR once more, then \
             merges it with the repository's default strategy and deletes its branch."
        ),
        _ => String::new(),
    };
    [
        head,
        body,
        format!(
            "When you're finished, run {tb} pr wait {r} (tb pr addressed and tb pr merge finish the visit too): the board \
             watches the PR and brings you back when it needs you again. Don't take another task here, and don't run tb \
             done: the task is already done."
        ),
    ]
    .join("\n")
}

fn resume_or_fresh(app: &App, t: &Row, mut args: Value) -> Result<Value> {
    let last = board::get_session(app, flow(t).s("session"))?;
    let cid = last
        .as_ref()
        .and_then(|l| l.s("claude_session_id").filter(|c| !c.is_empty()).map(|s| s.to_string()))
        .or_else(|| t.s("claude_session_id").filter(|c| !c.is_empty()).map(|s| s.to_string()));
    let live = last.as_ref().is_some_and(|l| l.s("status") != Some("gone") && board::runs_claude(l));
    let too_big = match (&cid, live) {
        (Some(cid), false) => {
            let cwd = args["cwd"].as_str().unwrap_or("").to_string();
            limits::fresh_start_why(app, &limits::conversation(app, &cwd, cid, last.as_ref().and_then(|l| l.s("last_activity"))))
        }
        _ => None,
    };
    match cid {
        Some(_) if too_big.is_some() => {
            let why = too_big.unwrap_or_default();
            board::log_event(app, t.id(), board::BOARD, "handoff", &format!("Starting a fresh conversation for PR #{}: {why}", t.i0("pr_num")))?;
            let p = format!("{}\n\n{}", args["prompt"].as_str().unwrap_or(""), handoff::pr_context(app, t));
            args["prompt"] = json!(p);
        }
        None => {
            board::log_event(app, t.id(), board::BOARD, "handoff", &format!("Starting a fresh conversation for PR #{}: it has no conversation to resume", t.i0("pr_num")))?;
            let p = format!("{}\n\n{}", args["prompt"].as_str().unwrap_or(""), handoff::pr_context(app, t));
            args["prompt"] = json!(p);
        }
        Some(cid) => {
            args["flags"] = json!(format!("--resume {cid}"));
            if let Some(l) = last.filter(|l| l.s("status") != Some("gone")) {
                args["replace"] = l.v("id");
            }
        }
    }
    Ok(args)
}

fn pr_cwd(app: &App, t: &Row) -> Result<Option<String>> {
    let ctx = board::task_context(t);
    let wt = ctx.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str()).filter(|w| Path::new(w).is_dir()).map(|s| s.to_string());
    Ok(match wt {
        Some(w) => Some(w),
        None => runner::task_cwd(app, t)?,
    })
}

pub fn wake(app: &App, t: &Row, phase: &str, rec: &Value, extra: Option<&str>) -> Result<Option<i64>> {
    let Some(cwd) = pr_cwd(app, t)? else { return Ok(None) };
    let mut text = prompt(app, t, phase, rec);
    if let Some(e) = extra {
        text += &format!("\n{e}");
    }
    let args = json!({"cwd": cwd, "title": short(&format!("PR #{}: {}", t.i0("pr_num"), t.st("title")), 40), "prompt": text, "queue": false});
    let args = resume_or_fresh(app, t, args)?;
    let jid = board::create_job(app, "agent", args, Some(t.id()), "pr", None)?;
    board::log_event(app, t.id(), board::BOARD, "handoff", &format!("Brought the conversation back for PR #{}: {}", t.i0("pr_num"), label(phase).to_lowercase()))?;
    Ok(Some(jid))
}

pub fn follow_wake(app: &App, t: &Row, text: &str) -> Result<Option<i64>> {
    let Some(cwd) = pr_cwd(app, t)? else { return Ok(None) };
    let tb = board::tb_cmd(app);
    let r = rf("task", t.id());
    let args = json!({"cwd": cwd, "title": short(&format!("PR #{}: {}", t.i0("pr_num"), t.st("title")), 40), "queue": false,
                      "prompt": format!("{text}\nPush with git push --force-with-lease, then run {tb} pr wait {r}. Don't take another task here, and don't run tb done: the task is already done.")});
    let args = resume_or_fresh(app, t, args)?;
    Ok(Some(board::create_job(app, "agent", args, Some(t.id()), "pr", None)?))
}

pub fn answer_in_new_tab(app: &App, t: &Row, text: &str) -> Result<Option<i64>> {
    let phase = t.st("pr_phase");
    if t.s("status") != Some("done") || !WAKE.contains(&phase.as_str()) || live_job(app, t)?.is_some() {
        return Ok(None);
    }
    let rec = flow(t).get("rec").cloned().unwrap_or(json!({}));
    let jid = wake(app, t, &phase, &rec, Some(&format!("{} answered: {text}", app.cfg.owner)))?;
    if jid.is_some() {
        merge_flow(app, t.id(), fields!["woke" => wake_key(&phase, &rec), "stopped" => null, "handled" => null])?;
    }
    Ok(jid)
}

/// Terminals that worked on a done task's PR and can close now.
pub fn pr_tabs_to_close(app: &App) -> Result<Vec<(Row, Row, bool)>> {
    let rows = app.db.q(
        "SELECT DISTINCT s.*, t.id AS pr_task FROM sessions s JOIN task_terminals tt ON tt.session_id = s.id \
         JOIN tasks t ON t.id = tt.task_id WHERE s.status IS NOT 'gone' AND t.status = 'done' AND t.pr_num IS NOT NULL",
        p![],
    )?;
    let mut out = vec![];
    for s in rows {
        let t = board::get_task(app, s.i0("pr_task"))?;
        if !t.b("auto_close")
            || board::task_for_session(app, s.s("id"))?.is_some()
            || board::close_rule(Some(&s)).is_none()
            || !board::opened_by_board(app, s.s("id"))?
        {
            continue;
        }
        let finished = FINISHED.contains(&t.s("pr_phase").unwrap_or(""));
        let f = flow(&t);
        let still_needed = f.s("session") == s.s("id") && (f.contains_key("woke") || live_job(app, &t)?.is_some());
        if !finished && still_needed {
            continue;
        }
        out.push((s, t, finished));
    }
    Ok(out)
}

/// One read of a task's PR from its host, as the flow keeps it (`prhost::Record::to_value`).
pub fn read(app: &App, t: &Row) -> std::result::Result<Value, String> {
    let pr = crate::prhost::PrRef::of(t).ok_or_else(|| "the task has no PR".to_string())?;
    let host = crate::prhost::host_for(app, &pr.host)?;
    Ok(host.read(&pr)?.to_value())
}

/// Reads one task's PR now and steps it, as the poll does. The record, or why the host couldn't be read.
pub fn refresh_task(app: &App, id: i64) -> Result<std::result::Result<Value, String>> {
    let t = board::get_task(app, id)?;
    let rec = match read(app, &t) {
        Ok(r) => r,
        Err(e) => return Ok(Err(e)),
    };
    gate(app, &t, &rec)?;
    let t = board::get_task(app, id)?;
    app.db.tx(|| step(app, &t, &rec))?;
    Ok(Ok(rec))
}

fn str_list(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default()
}

/// The threads waiting on the PR's author: unresolved, someone else spoke last, and not acknowledged on
/// the board since. Empty for a record without threads (older reads).
pub fn open_threads(f: &Row, rec: &Value) -> Vec<Value> {
    let author = rec["author"].as_str().unwrap_or("");
    let acks = f.get("threads_acked").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    rec["threads"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| {
            let Ok(th) = serde_json::from_value::<crate::prhost::Thread>(t.clone()) else { return false };
            th.waiting_on(author) && acks.get(&th.id).and_then(|v| v.as_str()) != Some(th.last_id.as_str())
        })
        .collect()
}

/// Whether this record lists threads (both hosts do now); older records only count comments.
fn has_threads(rec: &Value) -> bool {
    rec["threads"].is_array()
}

/// This push's `tb pr not-ours` clearance, if it has one.
pub fn not_ours(f: &Row, rec: &Value) -> Value {
    let head = rec["head"].as_str().unwrap_or("");
    f.get("not_ours").and_then(|m| m.get(head)).cloned().unwrap_or(Value::Null)
}

/// The failed checks that still count: those `tb pr not-ours` didn't clear for this push.
pub fn failing(f: &Row, rec: &Value) -> Vec<String> {
    let cleared = str_list(&not_ours(f, rec)["checks"]);
    str_list(&rec["failed"]).into_iter().filter(|c| !cleared.iter().any(|x| x.eq_ignore_ascii_case(c))).collect()
}

/// Expected checks (`[pr.projects.<name>] expected`) that haven't posted on this push.
pub fn expected_missing(app: &App, t: &Row, rec: &Value) -> Option<Vec<String>> {
    let want = app.cfg.pr.project(t.s("project")).expected?;
    let posted: Vec<String> = rec["checks"].as_array().cloned().unwrap_or_default().iter().filter_map(|c| c["name"].as_str().map(|s| s.to_lowercase())).collect();
    Some(want.into_iter().filter(|w| !posted.contains(&w.to_lowercase())).collect())
}

/// The base branch moved since this push was first seen.
pub fn base_moved(f: &Row, rec: &Value) -> bool {
    let now = rec["base_head"].as_str().unwrap_or("");
    let then = f.get("head_base").and_then(|m| m.get(rec["head"].as_str().unwrap_or(""))).and_then(|v| v.as_str()).unwrap_or("");
    !now.is_empty() && !then.is_empty() && now != then
}

/// Reviewers taken off the PR: a request for changes from one of them no longer holds.
pub fn swapped_off(f: &Row) -> Vec<String> {
    str_list(f.get("swapped_off").unwrap_or(&Value::Null))
}

/// Where the review stands, with requests for changes from swapped-off reviewers waived.
pub struct Review {
    /// Someone (still on the PR) asked for changes.
    pub changes: bool,
    pub approvals: i64,
    pub decision: String,
    /// Who asked for changes: (user id, name).
    pub requesters: Vec<(String, String)>,
}

pub fn review_of(f: &Row, rec: &Value) -> Review {
    let decision = rec["review_decision"].as_str().unwrap_or("").to_string();
    let Some(list) = rec["reviewers"].as_array() else {
        return Review { changes: decision == "CHANGES_REQUESTED", approvals: rec["approvals"].as_i64().unwrap_or(0), decision, requesters: vec![] };
    };
    let off = swapped_off(f);
    let on: Vec<&Value> = list.iter().filter(|r| !off.iter().any(|o| r["user"].as_str() == Some(o.as_str()))).collect();
    let requesters: Vec<(String, String)> =
        on.iter().filter(|r| r["state"] == "changes").map(|r| (r["user"].as_str().unwrap_or("").to_string(), r["name"].as_str().unwrap_or("").to_string())).collect();
    let changes = !requesters.is_empty() || (decision == "CHANGES_REQUESTED" && list.is_empty());
    let approvals = on.iter().filter(|r| r["state"] == "approved").count() as i64;
    let decision = match decision.as_str() {
        "CHANGES_REQUESTED" if !changes => String::new(),
        _ if changes => "CHANGES_REQUESTED".to_string(),
        d => d.to_string(),
    };
    Review { changes, approvals, decision, requesters }
}

/// Approvals this task's project needs, if it says.
pub fn approvals_needed(app: &App, t: &Row) -> Option<i64> {
    app.cfg.pr.project(t.s("project")).approvals
}

/// Approved enough to merge: the project's count, or (unset) the host's verdict or any approval.
pub fn approved(app: &App, t: &Row, r: &Review) -> bool {
    if r.changes {
        return false;
    }
    match approvals_needed(app, t) {
        Some(n) => r.approvals >= n,
        None => r.decision == "APPROVED" || (r.decision.is_empty() && r.approvals > 0),
    }
}

/// Why this head's checks are skipped (a hook or `tb pr skip-checks`), if they are. `*` skips every push.
pub fn checks_skipped(f: &Row, rec: &Value) -> Option<String> {
    let skips = f.get("skip_checks")?.as_object()?;
    let head = rec["head"].as_str().unwrap_or("");
    skips.get(head).or_else(|| skips.get("*")).map(|v| v.as_str().unwrap_or("").to_string())
}

pub fn phase_of(app: &App, t: &Row, rec: &Value) -> String {
    match rec["state"].as_str().unwrap_or("OPEN").to_uppercase().as_str() {
        "MERGED" => return "merged".into(),
        "CLOSED" | "DECLINED" | "SUPERSEDED" => return "declined".into(),
        _ => {}
    }
    let f = flow(t);
    let skipped = checks_skipped(&f, rec).is_some();
    if !skipped && !failing(&f, rec).is_empty() {
        return "fix".into();
    }
    if !skipped && checks_waiting(app, t, &f, rec) {
        return "checks".into();
    }
    let seen = f.i0("comments_seen");
    let review = review_of(&f, rec);
    let answered = f.get("answered_changes").map(|a| *a == rec["changes_at"]).unwrap_or(false);
    let new_comments = if has_threads(rec) { !open_threads(&f, rec).is_empty() } else { rec["comments"].as_i64().unwrap_or(0) > seen };
    if (review.changes && !answered) || new_comments {
        return "comments".into();
    }
    let review_skipped = review_skipped(&f, rec);
    if review.changes && answered && !review_skipped {
        // The changes are pushed; the reviewer who asked for them hasn't looked again yet.
        return "rereview".into();
    }
    if review_skipped || approved(app, t, &review) {
        // A stacked PR waits for the PR it builds on to merge first (`stack.rs`).
        return if crate::stack::holds(app, t).unwrap_or(false) { "waits" } else { "merge" }.into();
    }
    "review".into()
}

pub fn review_skipped(f: &Row, rec: &Value) -> bool {
    f.get("skip_review").and_then(|v| v.as_object()).map(|m| m.contains_key(rec["head"].as_str().unwrap_or(""))).unwrap_or(false)
}

/// The checks are still going: one is running, or (since this push was first seen) the expected
/// checks haven't all posted within their wait, or none has posted within the `no_checks_after` grace.
pub fn checks_waiting(app: &App, t: &Row, f: &Row, rec: &Value) -> bool {
    if rec["running"].as_i64().unwrap_or(0) > 0 {
        return true;
    }
    let first = f.get("head_at").and_then(|h| h.get(rec["head"].as_str().unwrap_or(""))).and_then(|v| v.as_f64());
    let waited = first.map(crate::clock::awake_since).unwrap_or(0.0);
    match expected_missing(app, t, rec) {
        Some(missing) => {
            let wait = app.cfg.pr.project(t.s("project")).expected_wait_mins.unwrap_or(crate::config::EXPECTED_WAIT_MINS);
            !missing.is_empty() && waited < wait * 60.0
        }
        None => rec["checks"].as_array().map(|a| a.is_empty()).unwrap_or(true) && waited < app.cfg.pr.no_checks_after_mins * 60.0,
    }
}

/// The task's flow with this record's head noted (when it was first seen), as `step` will save it.
fn with_head(t: &Row, rec: &Value) -> (Row, Option<Value>) {
    let mut f = flow(t);
    let head = rec["head"].as_str().unwrap_or("");
    let mut heads = f.get("head_at").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    if head.is_empty() || heads.contains_key(head) {
        return (f, None);
    }
    heads.insert(head.to_string(), json!(now_ts()));
    f.insert("head_at".into(), Value::Object(heads.clone()));
    (f, Some(Value::Object(heads)))
}

fn probe(t: &Row, f: &Row) -> Row {
    let mut p = t.clone();
    p.insert("pr_flow".into(), json!(jdumps(&Value::Object(f.clone()))));
    p
}

/// The owner's hooks on the PR step this record is about to reach (`pr.checks`, `pr.fix`, `pr.comments`,
/// `pr.merge`), once per step and push. Runs before `step` and outside its transaction. A skip marks this
/// push's checks as skipped, so the PR moves on to review; a stop keeps the agent from being brought back,
/// and the owner is told why.
pub fn gate(app: &App, t: &Row, rec: &Value) -> Result<()> {
    // A skip moves the PR on, and the step it lands on asks its own hooks: at most one round per step.
    for _ in 0..5 {
        let t = board::get_task(app, t.id())?;
        if !gate_once(app, &t, rec)? {
            break;
        }
    }
    Ok(())
}

/// Asks the hooks of the step this record reaches; true when they skipped it.
fn gate_once(app: &App, t: &Row, rec: &Value) -> Result<bool> {
    let (f, _) = with_head(t, rec);
    let phase = phase_of(app, &probe(t, &f), rec);
    if !matches!(phase.as_str(), "checks" | "fix" | "review" | "comments" | "merge") {
        return Ok(false);
    }
    let key = wake_key(&phase, rec);
    if f.s("gated") == Some(key.as_str()) {
        return Ok(false);
    }
    let event = format!("pr.{phase}");
    let extra = json!({"checks": rec["checks"], "failed_checks": rec["failed"], "head": rec["head"], "comments": rec["comments"]});
    let d = hooks::gate(app, &event, t, extra);
    let pr = format!("PR #{} for {}", t.i0("pr_num"), rf("task", t.id()));
    let head = rec["head"].as_str().unwrap_or("*").to_string();
    app.db.tx(|| {
        let mut changes = fields!["gated" => key.clone()];
        match &d {
            hooks::Decision::Go => {}
            hooks::Decision::Skip { reason, .. } => {
                match phase.as_str() {
                    "comments" => {
                        changes.push(("comments_seen", json!(rec["comments"].as_i64().unwrap_or(0))));
                        if !rec["changes_at"].is_null() {
                            changes.push(("answered_changes", rec["changes_at"].clone()));
                        }
                        // The open threads count as answered too, until someone writes on them again.
                        let mut acks = f.get("threads_acked").and_then(|v| v.as_object()).cloned().unwrap_or_default();
                        for th in open_threads(&f, rec) {
                            acks.insert(th["id"].as_str().unwrap_or("").to_string(), th["last_id"].clone());
                        }
                        changes.push(("threads_acked", Value::Object(acks)));
                    }
                    "review" => {
                        let mut skips = f.get("skip_review").and_then(|v| v.as_object()).cloned().unwrap_or_default();
                        skips.insert(head.clone(), json!(reason));
                        changes.push(("skip_review", Value::Object(skips)));
                    }
                    _ => {
                        let mut skips = f.get("skip_checks").and_then(|v| v.as_object()).cloned().unwrap_or_default();
                        skips.insert(head.clone(), json!(reason));
                        changes.push(("skip_checks", Value::Object(skips)));
                    }
                }
                hooks::note(app, t.id(), &d.said(&format!("{pr}: “{}” skipped", label(&phase))))?;
            }
            hooks::Decision::Block { .. } => {
                changes.push(("held", json!(key)));
                let line = d.said(&format!("{} stopped at “{}”", pr, label(&phase)));
                hooks::note(app, t.id(), &line)?;
                dispatch::add_alert(app, &line, Some(t.id()), t.i("goal_id"), None, Some("pr"))?;
            }
        }
        merge_flow(app, t.id(), changes).map(|_| ())
    })?;
    Ok(matches!(d, hooks::Decision::Skip { .. }))
}

/// `tb pr skip-checks`: this push's checks (or, with `all`, every push's) count as passed.
pub fn skip_checks(app: &App, t: &Row, reason: &str, all: bool, who: &str) -> Result<()> {
    let f = flow(t);
    let head = f.get("rec").and_then(|r| r["head"].as_str()).filter(|h| !h.is_empty());
    let key = if all { "*" } else { head.unwrap_or("*") };
    let mut skips = f.get("skip_checks").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    skips.insert(key.to_string(), json!(reason));
    merge_flow(app, t.id(), fields!["skip_checks" => Value::Object(skips)])?;
    let which = if key == "*" { "every push's checks" } else { "this push's checks" };
    board::log_event(app, t.id(), who, "status", &format!("PR #{}: skipped {which}: {reason}", t.i0("pr_num")))?;
    if let Some(rec) = f.get("rec").filter(|r| r.is_object()) {
        step(app, &board::get_task(app, t.id())?, rec)?;
    }
    Ok(())
}

/// Applies a fresh PR record to its task: stage, Jira, wakes.
pub fn step(app: &App, t: &Row, rec: &Value) -> Result<bool> {
    let (f, heads) = with_head(t, rec);
    let head = rec["head"].as_str().unwrap_or("").to_string();
    let mut changes: Vec<(&str, Value)> = vec![("rec", rec.clone()), ("checked_at", json!(now_iso()))];
    if let Some(heads) = heads {
        changes.push(("head_at", heads));
    }
    if !head.is_empty() {
        changes.push(("head", json!(head)));
        let mut bases = f.get("head_base").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        if let Some(b) = rec["base_head"].as_str().filter(|b| !b.is_empty() && !bases.contains_key(&head)) {
            bases.insert(head.clone(), json!(b));
            changes.push(("head_base", Value::Object(bases)));
        }
    }
    let phase = phase_of(app, &probe(t, &f), rec);
    let old = t.st("pr_phase");
    let phase_changed = phase != old;
    let build = if checks_skipped(&f, rec).is_some() {
        "Checks skipped"
    } else if !str_list(&rec["failed"]).is_empty() && failing(&f, rec).is_empty() {
        "Failed, not this PR"
    } else {
        build_label(rec)
    };
    let mut task_fields = fields!["pr_state" => rec["state"].as_str().unwrap_or("OPEN").to_uppercase(),
                                  "pr_build" => build, "pr_review" => review_label(&review_of(&f, rec))];
    if let Some(title) = rec["title"].as_str() {
        task_fields.push(("pr_title", json!(title)));
    }
    if phase_changed {
        changes.extend(fields!["stopped" => null, "handled" => null, "merge_tries" => null, "merge_gave_up" => null]);
        if !WAKE.contains(&phase.as_str()) {
            changes.extend(fields!["wake_tries" => null, "retry_at" => null]);
        }
        if !FINISHED.contains(&phase.as_str()) {
            board::log_event(app, t.id(), "PR", "status", &format!("PR #{}: {}", t.i0("pr_num"), label(&phase)))?;
        }
        if IN_REVIEW.contains(&phase.as_str()) && !IN_REVIEW.contains(&old.as_str()) {
            let st = app.cfg.jira.in_review.clone();
            board::jira_keep_in_step(app, t, Some(&st), None)?;
        }
        if phase == "merged" {
            board::log_event(app, t.id(), "PR", "status", &format!("PR #{} was merged", t.i0("pr_num")))?;
            let st = app.cfg.jira.merged.clone();
            board::jira_keep_in_step(app, t, Some(&st), None)?;
        }
        if phase == "declined" {
            board::log_event(app, t.id(), "PR", "status", &format!("PR #{} was closed without merging", t.i0("pr_num")))?;
        }
        task_fields.push(("pr_phase", json!(phase)));
    }
    if t.s("status") == Some("done")
        && phase == "review"
        && !f.contains_key("reviewed")
        && !f.contains_key("review_alerted")
        && f.s("held") != Some(wake_key("review", rec).as_str())
    {
        dispatch::add_alert(
            app,
            &format!("PR #{} for {} is green and ready for review.", t.i0("pr_num"), rf("task", t.id())),
            Some(t.id()),
            t.i("goal_id"),
            None,
            Some("review"),
        )?;
        changes.push(("review_alerted", json!(now_iso())));
    }
    let key = wake_key(&phase, rec);
    let may_wake = app.cfg.pr.wake && (phase != "merge" || app.cfg.pr.agents_merge);
    let mut now_flow = f.clone();
    for (k, v) in &changes {
        if v.is_null() {
            now_flow.remove(*k);
        } else {
            now_flow.insert(k.to_string(), v.clone());
        }
    }
    let stalled = phase == "merge" && merge_stalled(&now_flow, &key);
    let fresh = f.s("woke") != Some(key.as_str()) && f.s("handled") != Some(key.as_str());
    if may_wake
        && WAKE.contains(&phase.as_str())
        && t.s("status") == Some("done")
        && (fresh || stalled)
        && f.s("held") != Some(key.as_str())
        && !retry_later(&f)
        && live_job(app, t)?.is_none()
        && hours::goal_open(app, board::find_goal(app, t.i("goal_id"))?.as_ref())
    {
        let mut probe = t.clone();
        probe.insert("pr_phase".into(), json!(phase));
        let tries = now_flow.i0("merge_tries") + 1;
        if stalled {
            board::log_event(app, t.id(), board::BOARD, "status", &format!(
                "PR #{} is ready to merge but still open, so bringing it back to merge (try {tries} of {})",
                t.i0("pr_num"), MERGE_RETRY_WAITS.len()))?;
        }
        if wake(app, &probe, &phase, rec, None)?.is_some() {
            changes.extend(fields!["woke" => key, "stopped" => null]);
            if stalled {
                changes.extend(fields!["handled" => null, "merge_tries" => tries]);
            }
        }
    } else if may_wake && phase == "merge" && t.s("status") == Some("done") && merge_given_up(&now_flow, &key) {
        dispatch::add_alert(
            app,
            &format!(
                "PR #{} for {} is ready to merge but still open after {} tries to merge it. Its log says why.",
                t.i0("pr_num"),
                rf("task", t.id()),
                MERGE_RETRY_WAITS.len()
            ),
            Some(t.id()),
            t.i("goal_id"),
            None,
            Some("pr"),
        )?;
        changes.push(("merge_gave_up", json!(now_iso())));
    }
    if phase == "merge"
        && !app.cfg.pr.agents_merge
        && t.s("status") == Some("done")
        && f.s("merge_alerted") != Some(head.as_str())
        && f.s("held") != Some(key.as_str())
    {
        dispatch::add_alert(
            app,
            &format!("PR #{} for {} is approved and green: ready to merge.", t.i0("pr_num"), rf("task", t.id())),
            Some(t.id()),
            t.i("goal_id"),
            None,
            Some("pr"),
        )?;
        changes.push(("merge_alerted", json!(head)));
    }
    board::update_task(app, t.id(), task_fields)?;
    merge_flow(app, t.id(), changes)?;
    Ok(phase_changed)
}

fn build_label(rec: &Value) -> &'static str {
    if rec["failed"].as_array().map(|a| !a.is_empty()).unwrap_or(false) {
        "Failed"
    } else if rec["running"].as_i64().unwrap_or(0) > 0 {
        "Running"
    } else if rec["checks"].as_array().map(|a| a.is_empty()).unwrap_or(true) {
        "No checks"
    } else {
        "Passed"
    }
}

fn review_label(r: &Review) -> String {
    match r.decision.as_str() {
        "APPROVED" => "Approved".into(),
        "CHANGES_REQUESTED" => "Changes requested".into(),
        _ if r.approvals > 0 => format!("{} approved", plural(r.approvals, "reviewer")),
        _ => "Not reviewed yet".into(),
    }
}

/// Reads every open PR the board's tasks made, on every watched host, and steps each one.
pub fn refresh(app: &App) -> Result<i64> {
    if !app.cfg.pr.watch {
        return Ok(0);
    }
    let tasks = app.db.q(
        "SELECT * FROM tasks WHERE pr_host IN ('github', 'bitbucket') AND pr_num IS NOT NULL AND (pr_phase IS NULL OR pr_phase NOT IN ('merged', 'declined'))",
        p![],
    )?;
    let mut changed = 0;
    for t in tasks {
        match read(app, &t) {
            Ok(rec) => {
                gate(app, &board::get_task(app, t.id())?, &rec)?;
                let t = board::get_task(app, t.id())?;
                if app.db.tx(|| step(app, &t, &rec))? {
                    changed += 1;
                }
            }
            Err(e) => app.info(format!("prs: couldn't read {}: {e}", t.st("pr_url"))),
        }
    }
    crate::stack::retarget(app)?;
    crate::propen::add_evidence(app)?;
    let healed: Vec<Row> = app.db.q("SELECT * FROM tasks WHERE status = 'done' AND pr_num IS NOT NULL", p![])?;
    for t in healed {
        app.db.tx(|| heal(app, &t))?;
    }
    app.shared.lock().prs_checked_at = Some(now_iso());
    Ok(changed)
}

/// A wake whose terminal closed before it finished, or whose job failed: try again.
fn heal(app: &App, t: &Row) -> Result<()> {
    let f = flow(t);
    if !WAKE.contains(&t.s("pr_phase").unwrap_or(""))
        || !f.contains_key("woke")
        || f.get("stopped").map(|s| s["asked"] == true).unwrap_or(false)
        || live_job(app, t)?.is_some()
    {
        return Ok(());
    }
    let last = app.db.q1(
        "SELECT state, updated_at FROM jobs WHERE task_id = ? AND kind = 'agent' AND purpose = 'pr' ORDER BY id DESC LIMIT 1",
        p![t.id()],
    )?;
    if let Some(l) = &last {
        if matches!(l.s("state"), Some("failed") | Some("expired")) && wake_failed(app, t, "its last reopen didn't start")? {
            return Ok(());
        }
    }
    if let Some(s) = board::get_session(app, f.s("session"))? {
        if s.s("status") == Some("gone") && has(s.s("gone_at")) && last.as_ref().map(|l| s.st("gone_at") >= l.st("updated_at")).unwrap_or(true) {
            merge_flow(app, t.id(), fields!["woke" => null, "stopped" => null])?;
            board::log_event(
                app,
                t.id(),
                board::MIDNA,
                "midna",
                &format!("{} closed before it finished PR #{}; the board picks it up again", s.s("name").unwrap_or("Its terminal"), t.i0("pr_num")),
            )?;
        }
    }
    Ok(())
}

/// `tb pr wait`: the agent finished this visit; don't wake it again for the same state.
pub fn waited(app: &App, t: &Row) -> Result<Row> {
    let f = flow(t);
    let rec = f.get("rec").cloned().unwrap_or(json!({}));
    let mut ch = fields!["woke" => null, "stopped" => null, "comments_seen" => rec["comments"].as_i64().unwrap_or(0)];
    if t.s("pr_phase") == Some("comments") {
        ch.push(("answered_changes", rec["changes_at"].clone()));
    }
    if t.has_phase() {
        ch.push(("handled", json!(wake_key(&t.st("pr_phase"), &rec))));
        ch.push(("handled_at", json!(now_iso())));
    }
    merge_flow(app, t.id(), ch)
}

trait HasPhase {
    fn has_phase(&self) -> bool;
}
impl HasPhase for Row {
    fn has_phase(&self) -> bool {
        has(self.s("pr_phase"))
    }
}

pub fn mark_merged(app: &App, t: &Row, who: &str) -> Result<()> {
    if t.s("pr_phase") == Some("merged") {
        return Ok(());
    }
    board::update_task(app, t.id(), fields!["pr_state" => "MERGED", "pr_phase" => "merged"])?;
    merge_flow(app, t.id(), fields!["woke" => null, "stopped" => null])?;
    board::log_event(app, t.id(), who, "status", &format!("Merged PR #{}", t.i0("pr_num")))?;
    let st = app.cfg.jira.merged.clone();
    board::jira_keep_in_step(app, &board::get_task(app, t.id())?, Some(&st), None)?;
    dispatch::clear_alerts(app, Some(t.id()), None)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_the_build() {
        let r = json!({"checks": [{"name": "a", "state": "failed"}], "failed": ["a"], "running": 0});
        assert_eq!(build_label(&r), "Failed");
        assert_eq!(build_label(&json!({"checks": []})), "No checks");
    }

    #[test]
    fn open_threads_skip_acked_ones_until_someone_speaks_again() {
        let rec = json!({"author": "me", "threads": [
            {"id": "1", "kind": "comment", "resolvable": false, "resolved": false, "author": "rev", "author_name": "", "last_author": "rev", "last_id": "c1", "last_at": "", "text": ""},
            {"id": "2", "kind": "review", "resolvable": true, "resolved": true, "author": "rev", "author_name": "", "last_author": "rev", "last_id": "c2", "last_at": "", "text": ""}
        ]});
        let mut f = Row::new();
        assert_eq!(open_threads(&f, &rec).len(), 1);
        f.insert("threads_acked".into(), json!({"1": "c1"}));
        assert!(open_threads(&f, &rec).is_empty());
        f.insert("threads_acked".into(), json!({"1": "c0"}));
        assert_eq!(open_threads(&f, &rec).len(), 1, "a new comment reopens it");
    }

    #[test]
    fn a_swapped_off_reviewer_s_changes_are_waived() {
        let rec = json!({"review_decision": "CHANGES_REQUESTED", "reviewers": [
            {"user": "a", "name": "A", "state": "changes", "requested": true}, {"user": "b", "name": "B", "state": "approved", "requested": true}]});
        let mut f = Row::new();
        let r = review_of(&f, &rec);
        assert!(r.changes);
        assert_eq!(r.requesters, vec![("a".to_string(), "A".to_string())]);
        f.insert("swapped_off".into(), json!(["a"]));
        let r = review_of(&f, &rec);
        assert!(!r.changes);
        assert_eq!((r.approvals, r.decision.as_str()), (1, ""));
    }
}
