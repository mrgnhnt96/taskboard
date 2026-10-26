//! Pull requests after a task is done: watch PRs on their host (GitHub or Bitbucket Cloud, see
//! `prhost.rs`), work out their stage, and bring the task's conversation back when the PR needs work
//! (a failed check, open review threads, ready to merge).
//!
//! Besides the latest read (`rec`), a task's `pr_flow` keeps the board's own state of the PR:
//! `threads_acked` (thread id → the last comment it was acknowledged at, by `tb pr ack` or a reply to a
//! comment the host can't resolve), `not_ours` (head → the failed checks cleared for that push, with
//! the reason and proof), `head_base` (head → the base's commit when that push was first seen),
//! `swapped_off` (reviewers taken off the PR, whose requests for changes no longer hold),
//! `addressed` (the last `tb pr addressed`), `asked` (who was last asked to review, when and by
//! whom: `asks.rs`; the `ask` stage waits for it), `waiting_since` (reviewer → when the board first saw
//! the PR waiting on them, for reviewers it didn't ask itself: `prbar.rs`) and `lens` (the lens review of a new PR: `lens_pr.rs`;
//! the `lens` stage shows it running).
//!
//! The `ask` stage (`[reviewers] ask_stage`): once the owner has looked at a green PR ("I reviewed
//! it"), the PR waits for reviewers to be asked. The agent is brought back to run `tb pr reviewers`;
//! when it can't be (outside work hours, or `pr.wake` off), the board asks them itself
//! (`asks::stage`).

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{json, Value};

use crate::app::App;
use crate::config::RebasePolicy;
use crate::util::*;
use crate::{board, dispatch, fields, handoff, hooks, hours, limits, p, runner, transcript};

pub const WAKE: &[&str] = &["fix", "rebase", "comments", "steps", "merge", "ask"];
/// The visits that don't change code: they start fresh on `[models] pr_visit` (`fresh_visit`).
const MECHANICAL: &[&str] = &["merge", "ask"];
pub const IN_REVIEW: &[&str] = &["lens", "ask", "review", "rereview", "comments", "steps", "merge", "waits", "stack"];
/// The stages that move the ticket to In Code Review: reviewers have been asked (`ask` hasn't asked anyone yet).
pub const IN_CODE_REVIEW: &[&str] = &["review", "rereview", "comments", "steps", "merge", "waits", "stack"];
const FINISHED: &[&str] = &["merged", "declined"];
const WAKE_RETRY_WAITS: [i64; 3] = [60, 300, 900];
/// How long a terminal a visit opened may sit idle before its prompt reaches it (#592).
const PROMPT_WAIT_SECS: f64 = 120.0;
/// A merge the agent said it finished (`tb pr wait`) that's still open is brought back after each of these.
const MERGE_RETRY_WAITS: [i64; 3] = [60, 300, 900];
/// A merge refused for checks still running on the host holds the stage at `checks` for at most this
/// long while the feed doesn't show them finished; then the merge is tried afresh.
const RUNNING_ON_HOST_RECHECK: f64 = 1800.0;

pub fn label(phase: &str) -> &str {
    match phase {
        "checks" => "Watching checks",
        "fix" => "Fixing checks",
        "rebase" => "Rebasing",
        "lens" => "Lens review",
        "ask" => "Asking for reviews",
        "review" => "Awaiting review",
        "rereview" => "Awaiting re-review",
        "comments" => "Addressing comments",
        "steps" => "Waits on steps",
        "merge" => "Ready to merge",
        "waits" => "Waits on base",
        "stack" => "Waits on stack",
        "merged" => "Merged",
        "declined" => "Closed",
        other => other,
    }
}

pub(crate) fn flow(t: &Row) -> Row {
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

/// The host and repo of the task's project's remote (`[pr_body] remote`); None when the project has
/// no checkout, no such remote, or one on neither GitHub nor Bitbucket.
fn project_repo(app: &App, t: &Row) -> Option<(String, String)> {
    let path = crate::projects::project_path(app, t.s("project")).ok().flatten()?;
    let git = crate::proc::which("git")?;
    let args: Vec<String> = vec!["-C".into(), path, "remote".into(), "get-url".into(), app.cfg.pr_body.remote.clone()];
    let o = crate::proc::run(&git, &args, None, 5.0).ok()?;
    if o.code != Some(0) {
        return None;
    }
    crate::prhost::repo_of_remote(o.stdout.trim())
}

/// Why `pr` can't be the task's PR: it's in another repo than the task's project. None when it's in
/// that repo, or the project has no remote to judge by.
pub fn other_repo(app: &App, t: &Row, pr: &PrLink) -> Option<String> {
    let (host, repo) = project_repo(app, t)?;
    if host == pr.host && repo.eq_ignore_ascii_case(&pr.repo) {
        return None;
    }
    let project = t.s("project").unwrap_or("its project");
    Some(format!(
        "PR #{} is in {} ({}), but {} belongs to {project}, whose repo is {repo} ({host}). Open the PR from the task's checkout.",
        pr.num,
        pr.repo,
        pr.host,
        rf("task", t.id())
    ))
}

/// Refuses `pr` for the task when it's in another repo (`other_repo`).
pub fn refuse_other_repo(app: &App, t: &Row, pr: &PrLink) -> Result<()> {
    match other_repo(app, t, pr) {
        Some(why) => err(409, why),
        None => Ok(()),
    }
}

/// Links a PR found in an agent's report to its task (only the first one sticks; never one from
/// another repo than the task's project).
pub fn link_pr(app: &App, t: &Row, pr: &PrLink, who: &str) -> Result<bool> {
    if t.s("pr_url") == Some(pr.url.as_str()) || has(t.s("pr_repo")) {
        return Ok(false);
    }
    if other_repo(app, t, pr).is_some() {
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

/// After a review or comment hint: read the PR once more `comment_settle_secs` on (`late_reads`). Hints
/// within the window fold into one read, at the window after the last.
pub fn settle_after_hint(app: &App, t: &Row) -> Result<()> {
    let secs = app.cfg.pr.comment_settle_secs;
    if secs <= 0.0 {
        return Ok(());
    }
    merge_flow(app, t.id(), fields!["reread_at" => iso(now_ts() + secs)]).map(|_| ())
}

/// Reads each PR whose settle window after a hint is over, once, whether or not the poll rests.
pub fn late_reads(app: &App) -> Result<()> {
    let due = app.db.q(
        "SELECT * FROM tasks WHERE json_extract(pr_flow, '$.reread_at') IS NOT NULL AND json_extract(pr_flow, '$.reread_at') <= ?",
        p![now_iso()],
    )?;
    for t in due {
        app.db.tx(|| merge_flow(app, t.id(), fields!["reread_at" => null]).map(|_| ()))?;
        if !crate::prhost::watched(t.s("pr_host")) || !board::pr_still_open(&t) {
            continue;
        }
        if let Err(e) = refresh_task(app, t.id())? {
            app.info(format!("prs: reading PR #{} after a review hint: {e}", t.i0("pr_num")));
        }
    }
    Ok(())
}

/// PRs held at `checks` only by a wait that has run out since (the `no_checks_after` grace, or the
/// expected checks' wait), stepped again with their last record. The wait ends with time, not with a
/// read: a PR a healthy feed covers isn't polled, and one with no checks sends no event, so nothing
/// else moves it on to review. The read time (`checked_at`) stays the last read's.
pub fn waits_over(app: &App) -> Result<()> {
    for t in app.db.q("SELECT * FROM tasks WHERE pr_phase = 'checks' AND pr_num IS NOT NULL", p![])? {
        let f = flow(&t);
        let Some(rec) = f.get("rec").filter(|r| r.is_object()).cloned() else { continue };
        if phase_of(app, &t, &rec) == "checks" {
            continue;
        }
        gate(app, &t, &rec)?;
        let t = board::get_task(app, t.id())?;
        let read_at = f.get("checked_at").cloned().unwrap_or(Value::Null);
        app.db.tx(|| {
            step(app, &t, &rec)?;
            merge_flow(app, t.id(), fields!["checked_at" => read_at.clone()]).map(|_| ())
        })?;
    }
    Ok(())
}

/// A pr event for a PR no task has: the owner's open or merged PR whose source branch exactly one
/// not-done task works on, a task that ships a PR and has none yet, opened after that task was made
/// (when the event says when, `created_at`). Links it; None when no task fits, or more than one does.
pub fn link_by_branch(app: &App, ev: &Value) -> Result<Option<i64>> {
    let branch = body_str(ev, "branch");
    let state = body_str(ev, "state").to_lowercase();
    if branch.is_empty() || matches!(state.as_str(), "declined" | "closed" | "superseded") || !crate::prbuilds::owners_event(app, ev)? {
        return Ok(None);
    }
    let Some(pr) = event_link(ev) else { return Ok(None) };
    let opened = ev["created_at"].as_str().and_then(parse_iso);
    let mut fits = vec![];
    for t in app.db.q("SELECT * FROM tasks WHERE status != 'done' AND (pr_repo IS NULL OR pr_repo = '') ORDER BY id", p![])? {
        if crate::waitsfor::branch_of(&t).as_deref() != Some(branch.as_str()) || !crate::projects::task_ships_pr(app, &t)? {
            continue;
        }
        if opened.is_some_and(|o| t.s("created_at").and_then(parse_iso).is_some_and(|made| o < made)) {
            continue;
        }
        fits.push(t);
    }
    if fits.len() != 1 {
        return Ok(None);
    }
    let t = &fits[0];
    Ok(link_pr(app, t, &pr, board::BOARD)?.then(|| t.id()))
}

/// The PR an event names, as a link: its `url`, else a GitHub or Bitbucket link built from `host`,
/// `repo` and `num`.
fn event_link(ev: &Value) -> Option<PrLink> {
    if let Some(l) = find_pr(&body_str(ev, "url")) {
        return Some(l);
    }
    let (host, repo) = (body_str(ev, "host").to_lowercase(), body_str(ev, "repo"));
    let num = ev["num"].as_i64().filter(|n| *n > 0)?;
    let url = match host.as_str() {
        "github" => format!("https://github.com/{repo}/pull/{num}"),
        "bitbucket" => format!("https://bitbucket.org/{repo}/pull-requests/{num}"),
        _ => return None,
    };
    (!repo.is_empty()).then_some(PrLink { host, repo, num, url })
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
        "not_ours": not_ours_card(&f, &rec),
        "review_decision": rec.get("review_decision").cloned().unwrap_or(Value::Null),
        "checked_at": f.v("checked_at"),
        "awaiting_you": t.s("status") == Some("done") && awaiting_owner(t),
        "asked": f.v("asked"),
    })
}

/// The card's stage as it stands now (`card`): its terminal only while that terminal is still on the
/// visit (the wake is live or it stopped to ask, the terminal is open and hasn't taken another task).
/// A fix nobody is on or about to start reads "Checks failed"; one only the owner can fix (`tb pr
/// needs-owner`, this push) says it needs them, with `owner` saying what to do.
pub fn live_card(app: &App, t: &Row) -> Result<Value> {
    let mut c = card(t);
    if !c.is_object() {
        return Ok(c);
    }
    let f = flow(t);
    let on_it = waking(app, t)? || f.contains_key("stopped") || retry_later(&f);
    let live = match c["session"].as_str().filter(|s| !s.is_empty()) {
        Some(sid) if on_it => {
            board::get_session(app, Some(sid))?.is_some_and(|s| s.s("status") != Some("gone"))
                && board::task_for_session(app, Some(sid))?.is_none()
        }
        _ => false,
    };
    if !live {
        c["session"] = Value::Null;
    }
    if c["phase"] == "fix" {
        let rec = f.get("rec").cloned().unwrap_or(Value::Null);
        if let Some(o) = owner_act(&f, &rec) {
            c["label"] = json!("Needs you to fix checks");
            c["owner"] = o;
        } else if !on_it {
            c["label"] = json!("Checks failed");
            c["idle"] = json!(true);
        }
    }
    Ok(c)
}

/// This push's `tb pr needs-owner`, if it has one.
pub fn owner_act(f: &Row, rec: &Value) -> Option<Value> {
    let head = rec["head"].as_str().filter(|h| !h.is_empty())?;
    f.get("owner_act").filter(|o| o["head"] == head).cloned()
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

/// A PR visit is under way for the task: its reopen hasn't run yet, or the terminal it went to is still
/// at work and hasn't taken another task since. No other visit starts until it ends.
pub fn visit_live(app: &App, t: &Row) -> Result<bool> {
    if live_job(app, t)?.is_some() {
        return Ok(true);
    }
    let opened = app.db.val(
        "SELECT json_extract(target, '$.session') FROM jobs WHERE task_id = ? AND kind = 'agent' AND purpose = 'pr' \
         AND state = 'done' ORDER BY id DESC LIMIT 1",
        p![t.id()],
    )?;
    for sid in [flow(t).s("session"), opened.as_str()].into_iter().flatten().filter(|s| !s.is_empty()) {
        let working = board::get_session(app, Some(sid))?.is_some_and(|s| s.s("status") == Some("working"));
        if working && board::task_for_session(app, Some(sid))?.is_none() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The terminal the task's last PR visit opened, while that visit's prompt hasn't reached it: it isn't
/// the flow's terminal yet (`picked_up`) and no prompt has run in it. Idle there isn't a finished turn.
fn unprompted_tab(app: &App, t: &Row) -> Result<Option<Row>> {
    let f = flow(t);
    if !f.contains_key("woke") || live_job(app, t)?.is_some() {
        return Ok(None);
    }
    let opened = app.db.val(
        "SELECT json_extract(target, '$.session') FROM jobs WHERE task_id = ? AND kind = 'agent' AND purpose = 'pr' \
         AND state = 'done' ORDER BY id DESC LIMIT 1",
        p![t.id()],
    )?;
    let Some(sid) = opened.as_str().filter(|s| !s.is_empty() && f.s("session") != Some(*s)) else { return Ok(None) };
    Ok(board::get_session(app, Some(sid))?.filter(|s| s.v("board_prompt").is_null()))
}

/// A visit whose new terminal closed, or sat idle, before its prompt reached it: the wake failed, so
/// the board tries again rather than leaving the PR unvisited (#592).
pub fn prompts_missed(app: &App) -> Result<()> {
    let woke = app.db.q("SELECT * FROM tasks WHERE status = 'done' AND pr_num IS NOT NULL AND json_extract(pr_flow, '$.woke') IS NOT NULL", p![])?;
    for t in woke {
        if !WAKE.contains(&t.s("pr_phase").unwrap_or("")) {
            continue;
        }
        let Some(s) = unprompted_tab(app, &t)? else { continue };
        let why = match s.s("status") {
            Some("gone") => "its terminal closed before the prompt reached it",
            Some("idle") if age_secs(s.s("status_at")).unwrap_or(0.0) >= PROMPT_WAIT_SECS => "the prompt never reached its terminal",
            _ => continue,
        };
        app.db.tx(|| wake_failed(app, &t, why).map(|_| ()))?;
    }
    Ok(())
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
    note_stop(app, t, name, message, asked, wrote)?;
    Ok(true)
}

/// Records that the visit stopped (or asked the owner), whether or not the board woke it this time.
pub fn note_stop(app: &App, t: &Row, name: &str, message: &str, asked: bool, wrote: Option<&str>) -> Result<()> {
    let f = flow(t);
    let message = if message.is_empty() { "It stopped without saying why." } else { message };
    let before = f.get("stopped").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    if before.s("message") == Some(message) || (before.b("asked") && !asked) {
        return Ok(());
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
    Ok(())
}

pub fn wake_failed(app: &App, t: &Row, text: &str) -> Result<bool> {
    let t = board::get_task(app, t.id())?;
    let f = flow(&t);
    if t.s("status") != Some("done") || !WAKE.contains(&t.s("pr_phase").unwrap_or("")) || !f.contains_key("woke") {
        return Ok(false);
    }
    let tries = f.i0("wake_tries") + 1;
    let wait = WAKE_RETRY_WAITS[(tries.min(3) - 1) as usize];
    // Read the PR again once the wait is over, so the retry runs whether or not anything else steps it.
    let at = iso(now_ts() + wait as f64);
    merge_flow(app, t.id(), fields!["woke" => null, "wake_tries" => tries, "retry_at" => at.clone(), "reread_at" => at])?;
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
    if phase == "rebase" {
        // One conflict wakes once; a base that moves on after the rebase can conflict (and wake) again.
        return format!("rebase:{}:{}", rec["head"].as_str().unwrap_or(""), rec["base_head"].as_str().unwrap_or(""));
    }
    let key = format!("{phase}:{}:{}", rec["head"].as_str().unwrap_or(""), rec["comments"].as_i64().unwrap_or(0));
    if phase != "merge" {
        return key;
    }
    // A change in the reviews or the builds wakes the merge again.
    let mut builds: Vec<String> = rec["checks"]
        .as_array()
        .map(|a| a.iter().map(|c| format!("{}={}", c["name"].as_str().unwrap_or(""), c["state"].as_str().unwrap_or(""))).collect())
        .unwrap_or_default();
    builds.sort();
    format!("{key}:{}:{}:{}", rec["review_decision"].as_str().unwrap_or(""), rec["approvals"].as_i64().unwrap_or(0), builds.join(","))
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

/// The fix wake's sentence on rebasing, by policy (with its leading space).
fn rebase_advice(policy: RebasePolicy) -> &'static str {
    match policy {
        RebasePolicy::Moved => " If the base branch has moved, rebase onto it before you push (git push --force-with-lease).",
        RebasePolicy::Conflicts => {
            " Don't rebase onto the base branch unless the PR conflicts with it (a rebase starts the builds again); when it \
             does, rebase before you push (git push --force-with-lease)."
        }
        RebasePolicy::Never => " Don't rebase onto the base branch.",
    }
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
             If the PR's change causes it, fix exactly that in this task's worktree, run the failing tests and the ones covering your fix (not the whole suite), and push; the checks \
             run again.{rebase} If the \
             failure isn't this PR's (the base branch fails it too), don't change unrelated code: clear it with {tb} pr \
             not-ours {r} --check \"<check>\" --title \"<what fails>\" --reason \"<why it isn't this PR>\" --proof <link to \
             the same failure without this change>. If only the owner can fix it (a missing secret or variable, a \
             permissions error), don't work around it: run {tb} pr needs-owner {r} --reason \"<what the owner has to \
             do>\"; it tells them and finishes this visit.",
            if failed.is_empty() { "see the PR".to_string() } else { failed.join(", ") },
            num = t.i0("pr_num"),
            rebase = rebase_advice(rebase_policy(app, t))
        ),
        "rebase" => format!(
            "It conflicts with its base branch {base}, so it can't merge. In this task's worktree, rebase onto {base} \
             ({tb} pr status {r} lists the commands), resolve the conflicts, run the tests covering the files you \
             resolved (not the whole suite), and push with git push --force-with-lease. The checks run again.",
            base = rec["base"].as_str().filter(|b| !b.is_empty()).unwrap_or("the base branch")
        ),
        "comments" => format!(
            "There are open review threads{}. {tb} pr status {r} lists them with their ids, and {tb} pr thread {r} <thread> prints one's \
             whole text and replies. Address each in the worktree; \
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
        "steps" => {
            let left = merge_steps_left(app, t, rec);
            let v = crate::steps::vars_at(app, t, rec["head"].as_str());
            let each: Vec<String> = left.iter().map(|s| s.filled(&v)).map(|s| format!("- {} ({}): {}", s.name, s.what(), s.how(&tb))).collect();
            format!(
                "It's approved and every check is green, but its merge waits on {} on its head ({}):\n{}\nOnce they've \
                 passed, the board moves the PR on to its merge. Don't merge it in this visit.",
                if left.len() == 1 { "this step" } else { "these steps" },
                short(rec["head"].as_str().unwrap_or(""), 7),
                each.join("\n")
            )
        }
        "merge" => format!(
            "It's approved and every check is green. Merge it with {tb} pr merge {r}: it checks the PR once more, then \
             merges it with the repository's default strategy and deletes its branch."
        ),
        "ask" => format!(
            "{} has reviewed it. Ask for reviews with {tb} pr reviewers {r}: the board picks the reviewers (a main \
             contributor of the changed files, then whoever's turn it is) and asks them on the PR. To ask someone in \
             particular, add --ask <name>. It finishes the visit.",
            app.cfg.owner
        ),
        _ => String::new(),
    };
    [
        head,
        body,
        format!(
            "When you're finished, run {tb} pr wait {r} (tb pr addressed and tb pr merge finish the visit too): the board \
             watches the PR and brings you back when it needs you again. Don't take another task here, and don't run tb \
             submit: the task is already submitted."
        ),
    ]
    .join("\n")
}

/// The conversation a PR visit resumes: the task's own, kept aside while fresh visits run (`resume_cid`).
fn task_conversation(t: &Row, last: Option<&Row>) -> Option<String> {
    let set = |c: Option<&str>| c.filter(|c| !c.is_empty()).map(|s| s.to_string());
    set(flow(t).s("resume_cid")).or_else(|| set(last.and_then(|l| l.s("claude_session_id")))).or_else(|| set(t.s("claude_session_id")))
}

/// The old terminal a visit takes over; never the Jira desk, even if the task ever landed on it.
fn replaceable(app: &App, last: Option<Row>) -> Option<Value> {
    last.filter(|l| l.s("status") != Some("gone") && !crate::jira_desk::is_desk(app, l.s("id")).unwrap_or(false)).map(|l| l.v("id"))
}

/// A visit that doesn't change code (`MECHANICAL`): a fresh conversation on `[models] pr_visit` with
/// a short prompt, keeping the task's conversation aside for the next visit that does.
fn fresh_visit(app: &App, t: &Row, phase: &str, mut args: Value) -> Result<Value> {
    let last = board::get_session(app, flow(t).s("session"))?;
    if !flow(t).contains_key("resume_cid") {
        if let Some(cid) = task_conversation(t, last.as_ref()) {
            merge_flow(app, t.id(), fields!["resume_cid" => cid])?;
        }
    }
    let ctx = board::task_context(t);
    let branch = ctx.get("where").and_then(|w| w.get("branch")).and_then(|b| b.as_str()).filter(|b| !b.is_empty());
    let mut place = format!("Worktree: {}.", args["cwd"].as_str().unwrap_or(""));
    if let Some(b) = branch {
        place = format!("Branch: {b}. {place}");
    }
    args["prompt"] = json!(format!("{}\n\nThis is a fresh conversation for this step only. {place}", args["prompt"].as_str().unwrap_or("")));
    let model = app.cfg.models.resolve_pr_visit(t.s("model"));
    let effort = app.cfg.models.resolve_pr_visit_effort(t.s("model"), t.s("effort"));
    if let Some(f) = runner::with_effort(runner::with_model(None, model.as_deref()), effort.as_deref()) {
        args["flags"] = json!(f);
    }
    if let Some(id) = replaceable(app, last) {
        args["replace"] = id;
    }
    let on = model.map(|m| format!(" on {m}")).unwrap_or_default();
    board::log_event(app, t.id(), board::BOARD, "handoff", &format!("Starting a fresh conversation{on} for PR #{}: {}", t.i0("pr_num"), label(phase).to_lowercase()))?;
    Ok(args)
}

fn resume_or_fresh(app: &App, t: &Row, mut args: Value) -> Result<Value> {
    let last = board::get_session(app, flow(t).s("session"))?;
    let cid = task_conversation(t, last.as_ref())
        .filter(|c| transcript::conversation_saved(&app.cfg.claude_projects, args["cwd"].as_str().unwrap_or(""), c));
    let model = app.cfg.models.resolve(t.s("model"));
    let effort = app.cfg.models.resolve_effort(t.s("model"), t.s("effort"));
    if let Some(f) = runner::with_effort(runner::with_model(None, model.as_deref()), effort.as_deref()) {
        args["flags"] = json!(f);
    }
    if flow(t).contains_key("resume_cid") {
        merge_flow(app, t.id(), fields!["resume_cid" => null])?;
    }
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
            args["flags"] = json!(runner::with_effort(runner::with_model(Some(format!("--resume {cid}")), model.as_deref()), effort.as_deref()));
            if let Some(id) = replaceable(app, last) {
                args["replace"] = id;
            }
        }
    }
    Ok(args)
}

/// Where PR follow-up work runs. A task that worked in a slot claims one again, on its pushed
/// branch; while the pool is full there's nowhere yet, and the next wake tries again.
pub(crate) fn pr_cwd(app: &App, t: &Row) -> Result<Option<String>> {
    match crate::slots::reclaim(app, t) {
        Ok(Some(path)) => return Ok(Some(path)),
        Ok(None) => {}
        Err(e) => {
            app.info(format!("worktrees: {} has no slot for its PR work yet: {}", rf("task", t.id()), e.message));
            return Ok(None);
        }
    }
    let ctx = board::task_context(t);
    let wt = ctx.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str()).filter(|w| Path::new(w).is_dir()).map(|s| s.to_string());
    Ok(match wt {
        Some(w) => Some(w),
        None => runner::task_cwd(app, t)?,
    })
}

/// What a wake that had to wait for its slot does once the claim is made. A follow-up or a note ask
/// isn't sent from the worker: its caller records it only once it's sent, so the next poll sends it.
enum Again {
    Step,
    Nothing,
}

impl Again {
    fn key(&self, id: i64) -> String {
        match self {
            Again::Step => format!("{id}:step"),
            Again::Nothing => format!("{id}:claim"),
        }
    }
}

/// Wakes whose slot claim is waiting on the deferred worker, so a busy poll doesn't queue the same
/// claim again before the first one runs.
static CLAIMING: parking_lot::Mutex<BTreeSet<String>> = parking_lot::Mutex::new(BTreeSet::new());

/// `pr_cwd` for a wake, which often runs inside a transaction. Claiming a slot afresh checks it out
/// with git (seconds, more on a big repository), and every request would wait on the database
/// meanwhile: the claim goes to the deferred worker instead, which then does the wake again.
fn wake_cwd(app: &App, t: &Row, again: Again) -> Result<Option<String>> {
    if app.db.held_here() && !app.inline_deferred && crate::slots::needs_claim(app, t)? {
        claim_later(app, t.id(), again);
        return Ok(None);
    }
    pr_cwd(app, t)
}

fn claim_later(app: &App, id: i64, again: Again) {
    let key = again.key(id);
    if !CLAIMING.lock().insert(key.clone()) {
        return;
    }
    app.info(format!("worktrees: {} claims its slot outside the database first", rf("task", id)));
    app.defer(Box::new(move |app: &App| {
        if let Err(e) = claimed_then(app, id, again) {
            app.info(format!("worktrees: {} PR work after its slot claim: {}", rf("task", id), e.message));
        }
        CLAIMING.lock().remove(&key);
    }));
}

fn claimed_then(app: &App, id: i64, again: Again) -> Result<()> {
    let t = board::get_task(app, id)?;
    if let Err(e) = crate::slots::reclaim(app, &t) {
        app.info(format!("worktrees: {} has no slot for its PR work yet: {}", rf("task", id), e.message));
        return Ok(());
    }
    let t = board::get_task(app, id)?;
    if crate::slots::needs_claim(app, &t)? {
        return Ok(());
    }
    match again {
        Again::Step => {
            if let Some(rec) = flow(&t).get("rec").filter(|r| r.is_object()) {
                app.db.tx(|| step(app, &t, rec))?;
            }
        }
        Again::Nothing => {}
    }
    Ok(())
}

pub fn wake(app: &App, t: &Row, phase: &str, rec: &Value, extra: Option<&str>) -> Result<Option<i64>> {
    if visit_live(app, t)? {
        return Ok(None);
    }
    // An answer is delivered again until it lands, so only a plain wake comes back after the claim.
    let again = if extra.is_none() { Again::Step } else { Again::Nothing };
    let Some(cwd) = wake_cwd(app, t, again)? else { return Ok(None) };
    let mut text = prompt(app, t, phase, rec);
    if let Some(e) = extra {
        text += &format!("\n{e}");
    }
    let args = json!({"cwd": cwd, "title": short(&format!("PR #{}: {}", t.i0("pr_num"), t.st("title")), 40), "prompt": text, "queue": false});
    let mut args = if app.cfg.models.fresh_pr_visits && MECHANICAL.contains(&phase) { fresh_visit(app, t, phase, args)? } else { resume_or_fresh(app, t, args)? };
    // A merge needs nobody watching it: a new terminal for it opens in Midna's Background group.
    if phase == "merge" {
        args["background"] = json!(true);
    }
    let jid = board::create_job(app, "agent", args, Some(t.id()), "pr", None)?;
    board::log_event(app, t.id(), board::BOARD, "handoff", &format!("Brought the conversation back for PR #{}: {}", t.i0("pr_num"), label(phase).to_lowercase()))?;
    Ok(Some(jid))
}

/// Brings a done task's PR conversation back to rebase. None when it can't yet (a visit is live, or
/// there's no slot for it): the caller asks again later.
pub fn follow_wake(app: &App, t: &Row, text: &str) -> Result<Option<i64>> {
    if visit_live(app, t)? {
        return Ok(None);
    }
    let Some(cwd) = wake_cwd(app, t, Again::Nothing)? else { return Ok(None) };
    let tb = board::tb_cmd(app);
    let r = rf("task", t.id());
    let args = json!({"cwd": cwd, "title": short(&format!("PR #{}: {}", t.i0("pr_num"), t.st("title")), 40), "queue": false,
                      "prompt": format!("{text}\nPush with git push --force-with-lease, then run {tb} pr wait {r}. Don't take another task here, and don't run tb submit: the task is already submitted.")});
    let args = resume_or_fresh(app, t, args)?;
    Ok(Some(board::create_job(app, "agent", args, Some(t.id()), "pr", None)?))
}

/// Brings a merged task's conversation back to write its note for testers (`[jira] merged_note`).
/// None when it can't yet, as with `follow_wake`.
pub fn ask_for_note(app: &App, t: &Row, text: &str) -> Result<Option<i64>> {
    if visit_live(app, t)? {
        return Ok(None);
    }
    let Some(cwd) = wake_cwd(app, t, Again::Nothing)? else { return Ok(None) };
    let args = json!({"cwd": cwd, "title": short(&format!("Note for testers: {}", t.st("title")), 40), "prompt": text, "queue": false});
    let args = resume_or_fresh(app, t, args)?;
    let jid = board::create_job(app, "agent", args, Some(t.id()), "pr", None)?;
    board::log_event(app, t.id(), board::BOARD, "handoff", "Brought the conversation back to write the note for testers")?;
    Ok(Some(jid))
}

pub fn answer_in_new_tab(app: &App, t: &Row, text: &str) -> Result<Option<i64>> {
    let phase = t.st("pr_phase");
    if t.s("status") != Some("done") || !WAKE.contains(&phase.as_str()) || visit_live(app, t)? {
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
        // A tab the board opened for the task counts even when it never reported back as one of its terminals.
        "SELECT DISTINCT s.*, t.id AS pr_task FROM sessions s \
         JOIN (SELECT task_id, session_id FROM task_terminals \
               UNION SELECT task_id, json_extract(target, '$.session') FROM jobs WHERE kind = 'agent' AND task_id IS NOT NULL) tt \
           ON tt.session_id = s.id \
         JOIN tasks t ON t.id = tt.task_id WHERE s.status IS NOT 'gone' AND t.status = 'done' AND t.pr_num IS NOT NULL",
        p![],
    )?;
    let mut out = vec![];
    for s in rows {
        let t = board::get_task(app, s.i0("pr_task"))?;
        if !t.b("auto_close")
            || crate::lines::busy(app, &s.st("id"))?
            || board::close_rule(Some(&s)).is_none()
            || !board::opened_by_board(app, s.s("id"))?
        {
            continue;
        }
        let finished = FINISHED.contains(&t.s("pr_phase").unwrap_or(""));
        let f = flow(&t);
        let still_needed = f.s("session") == s.s("id") && (f.contains_key("woke") || live_job(app, &t)?.is_some());
        let awaiting_prompt = unprompted_tab(app, &t)?.is_some_and(|u| u.s("id") == s.s("id"));
        if !finished && (still_needed || awaiting_prompt) {
            continue;
        }
        out.push((s, t, finished));
    }
    Ok(out)
}

/// One read of a task's PR from its host, as the flow keeps it (`prhost::Record::to_value`). The read
/// time is kept as `read_at`: a PR a stream relay covers is polled again only once it's old
/// (`feed::pr_poll_skips`).
pub fn read(app: &App, t: &Row) -> std::result::Result<Value, String> {
    let pr = crate::prhost::PrRef::of(t).ok_or_else(|| "the task has no PR".to_string())?;
    let host = crate::prhost::host_for(app, &pr.host)?;
    // An open PR's stage can change: its reads still go while the account restarts slowly after a limit.
    let mut rec = crate::prhost::limit::priority(|| host.read(&pr))?;
    roster_ids(app, &t.st("project"), &mut rec).map_err(|e| e.message)?;
    merge_flow(app, t.id(), fields!["read_at" => now_iso()]).map_err(|e| e.message)?;
    Ok(rec.to_value())
}

/// Names each reviewer by the id the project's roster has for them, when the host knows them by
/// more than one (Bitbucket reads a `{uuid}`; the roster may hold the `account_id` the board asks
/// with), so asks, swaps and removals match them. The board's own account and the author keep theirs.
fn roster_ids(app: &App, project: &str, rec: &mut crate::prhost::Record) -> Result<()> {
    for r in rec.reviewers.iter_mut().filter(|r| !r.ids.is_empty() && r.user != rec.viewer && r.user != rec.author) {
        for id in r.ids.clone() {
            let user = crate::reviewers::by_host_user(app, project, &id)?.and_then(|row| row.s("host_user").map(|u| u.to_string()));
            if let Some(user) = user.filter(|u| r.ids.iter().any(|i| i.eq_ignore_ascii_case(u))) {
                r.user = user;
                break;
            }
        }
    }
    Ok(())
}

/// Reads one task's PR now and steps it, as the poll does. The record, or why the host couldn't be read.
pub fn refresh_task(app: &App, id: i64) -> Result<std::result::Result<Value, String>> {
    let t = board::get_task(app, id)?;
    let rec = match read(app, &t) {
        Ok(r) => r,
        Err(e) => return Ok(Err(e)),
    };
    apply(app, id, &rec)?;
    Ok(Ok(rec))
}

/// Steps one task's PR with a record, as a read of it would.
pub fn apply(app: &App, id: i64, rec: &Value) -> Result<()> {
    crate::lens_pr::kick(app, id, rec)?;
    let t = board::get_task(app, id)?;
    note_expected(app, &t, rec)?;
    let t = board::get_task(app, id)?;
    gate(app, &t, rec)?;
    let t = board::get_task(app, id)?;
    app.db.tx(|| step(app, &t, rec))?;
    Ok(())
}

/// A stream pr event, stepped as the PR's state without reading its host (stream mode). A partial one
/// (state, title, branch and base, without reviewers, threads or checks) keeps the rest of the last
/// read, and when it moved the head reads only that commit's checks from the host. None until the PR
/// has been read once (the board's account and the base branch's head come from that read), or when
/// the host can't read the checks apart: then the caller reads the whole PR.
pub fn apply_event(app: &App, id: i64, ev: &Value) -> Result<Option<Value>> {
    let t = board::get_task(app, id)?;
    let prior = flow(&t).get("rec").filter(|r| r.is_object()).cloned();
    let Some(prior) = prior else { return Ok(None) };
    let mut rec = crate::prhost::Record::from_event(ev, &prior);
    if crate::prhost::Record::lacks_checks(ev, &prior) {
        let Some(pr) = crate::prhost::PrRef::of(&t) else { return Ok(None) };
        let Ok(host) = crate::prhost::host_for(app, &pr.host) else { return Ok(None) };
        match host.checks(&pr, &rec.head) {
            Ok(Some(checks)) => rec.checks = checks,
            _ => return Ok(None),
        }
    }
    let rec = rec.to_value();
    apply(app, id, &rec)?;
    Ok(Some(rec))
}

fn str_list(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default()
}

/// Who a thread waits on: the board's own account on the host (`viewer`), else the PR's author.
pub fn us(rec: &Value) -> &str {
    rec["viewer"].as_str().filter(|v| !v.is_empty()).or(rec["author"].as_str()).unwrap_or("")
}

/// The threads waiting on us (`us`): unresolved, someone else spoke last, and not acknowledged on the
/// board since. Empty for a record without threads (older reads).
pub fn open_threads(f: &Row, rec: &Value) -> Vec<Value> {
    let author = us(rec);
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

/// This push's clearance for the card: with each proof link labelled (`links: [{url, label}]`).
fn not_ours_card(f: &Row, rec: &Value) -> Value {
    let mut n = not_ours(f, rec);
    if n.is_object() {
        let links: Vec<Value> = str_list(&n["proof"]).iter().map(|u| json!({"url": u, "label": crate::prcmds::proof_label(u)})).collect();
        n["links"] = json!(links);
    }
    n
}

/// The failed checks that still count: those `tb pr not-ours` didn't clear for this push.
pub fn failing(f: &Row, rec: &Value) -> Vec<String> {
    let cleared = str_list(&not_ours(f, rec)["checks"]);
    str_list(&rec["failed"]).into_iter().filter(|c| !cleared.iter().any(|x| x.eq_ignore_ascii_case(c))).collect()
}

/// The checks a push is expected to post, and where that came from: `"command"` (the project's
/// `expected_cmd`, its answer for this head), `"list"` (`expected`) or `"earlier"` (neither is set: the
/// checks that posted on the PR's last push that posted any, `earlier_checks`). No checks: no word (the
/// `no_checks_after_mins` grace), with the source still `"command"` when the command couldn't tell.
pub struct Expected {
    pub checks: Option<Vec<String>>,
    pub source: Option<&'static str>,
    /// Why the command couldn't tell, when it failed.
    pub error: Option<String>,
}

/// What this push is expected to post: `expected_cmd`'s answer for its head once `note_expected` has it,
/// else the project's `expected` list.
pub fn expected_for(app: &App, t: &Row, rec: &Value) -> Expected {
    let rules = crate::projects::pr_rules(app, t.s("project"));
    let head = rec["head"].as_str().unwrap_or("");
    let has_cmd = rules.expected_cmd.as_deref().is_some_and(|c| !c.trim().is_empty());
    let f = flow(t);
    if let Some(e) = f.get("expected_by_head").and_then(|m| m.get(head)).filter(|_| has_cmd && !head.is_empty()) {
        let checks = e["checks"].as_array().map(|_| str_list(&e["checks"]));
        return Expected { checks, source: Some("command"), error: e["error"].as_str().map(|s| s.to_string()) };
    }
    if rules.expected.is_none() && !has_cmd {
        if let Some(checks) = earlier_checks(&f, rec) {
            return Expected { checks: Some(checks), source: Some("earlier"), error: None };
        }
    }
    let source = rules.expected.is_some().then_some("list");
    Expected { checks: rules.expected, source, error: None }
}

/// The checks that posted on the PR's latest earlier push that posted any (`checks_by_head`): a new push
/// with none yet isn't one that needs none, it waits for those same checks.
fn earlier_checks(f: &Row, rec: &Value) -> Option<Vec<String>> {
    let head = rec["head"].as_str().filter(|h| !h.is_empty())?;
    let at = |h: &str| f.get("head_at").and_then(|m| m.get(h)).and_then(|v| v.as_f64()).unwrap_or(0.0);
    f.get("checks_by_head")?
        .as_object()?
        .iter()
        .filter(|(h, c)| h.as_str() != head && c.as_array().is_some_and(|a| !a.is_empty()))
        .max_by(|a, b| at(a.0).total_cmp(&at(b.0)))
        .map(|(_, c)| str_list(c))
}

/// Expected checks (`[pr.projects.<name>] expected`, or `expected_cmd`'s answer for this push) that
/// haven't posted on this push.
pub fn expected_missing(app: &App, t: &Row, rec: &Value) -> Option<Vec<String>> {
    let want = expected_for(app, t, rec).checks?;
    let posted: Vec<String> = rec["checks"].as_array().cloned().unwrap_or_default().iter().filter_map(|c| c["name"].as_str().map(|s| s.to_lowercase())).collect();
    Some(want.into_iter().filter(|w| !posted.contains(&w.to_lowercase())).collect())
}

/// Runs the project's `expected_cmd` for a head it hasn't answered for yet and keeps the answer in the
/// flow (`expected_by_head`). Outside any transaction: the command may take up to a minute.
pub fn note_expected(app: &App, t: &Row, rec: &Value) -> Result<()> {
    let Some(cmd) = crate::projects::pr_rules(app, t.s("project")).expected_cmd.filter(|c| !c.trim().is_empty()) else { return Ok(()) };
    let head = rec["head"].as_str().unwrap_or("");
    let Some(pr) = crate::prhost::PrRef::of(t) else { return Ok(()) };
    if head.is_empty() || flow(t).get("expected_by_head").and_then(|m| m.get(head)).is_some() {
        return Ok(());
    }
    let ctx = board::task_context(t);
    let wt = ctx.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str()).filter(|w| Path::new(w).is_dir()).map(|s| s.to_string());
    let mut env: Vec<(String, String)> = [
        ("TB_PR_URL", pr.url.clone()),
        ("TB_PR_REPO", pr.repo.clone()),
        ("TB_PR_NUM", pr.num.to_string()),
        ("TB_HEAD", head.to_string()),
        ("TB_BASE", rec["base"].as_str().unwrap_or("").to_string()),
        ("TB_BRANCH", rec["branch"].as_str().unwrap_or("").to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    if let Some(w) = &wt {
        env.push(("TB_WORKTREE".into(), w.clone()));
    }
    let cmd = crate::util::expand_home(cmd.trim()).to_string_lossy().to_string();
    let ran = crate::proc::run_with(Path::new("/bin/sh"), &["-c".into(), cmd], wt.as_deref().map(Path::new), 60.0, &env, None);
    let (checks, error) = match ran {
        Ok(o) if o.code == Some(0) => match parse_expected(&o.stdout) {
            Ok(c) => (c, None),
            Err(e) => (None, Some(e)),
        },
        Ok(o) => (None, Some(format!("expected_cmd failed: {}", o.stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("no output")))),
        Err(_) => (None, Some("expected_cmd didn't finish in 60 seconds".to_string())),
    };
    let entry = json!({"checks": checks, "error": error, "at": now_iso()});
    // Read the flow again: the command ran outside the transaction.
    app.db.tx(|| {
        let mut all = flow(&board::get_task(app, t.id())?).get("expected_by_head").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        all.insert(head.to_string(), entry.clone());
        merge_flow(app, t.id(), fields!["expected_by_head" => Value::Object(all)]).map(|_| ())
    })
}

/// `expected_cmd`'s output: a JSON list of check names (`[]`: none), or null or nothing (can't tell).
pub fn parse_expected(out: &str) -> std::result::Result<Option<Vec<String>>, String> {
    let out = out.trim();
    if out.is_empty() {
        return Ok(None);
    }
    match serde_json::from_str::<Value>(out) {
        Ok(Value::Null) => Ok(None),
        Ok(v @ Value::Array(_)) if v.as_array().is_some_and(|a| a.iter().all(|x| x.is_string())) => Ok(Some(str_list(&v))),
        _ => Err(format!("expected_cmd printed something other than a JSON list of check names: {}", out.lines().next().unwrap_or(""))),
    }
}

/// The base branch moved since this push was first seen.
pub fn base_moved(f: &Row, rec: &Value) -> bool {
    let now = rec["base_head"].as_str().unwrap_or("");
    let then = f.get("head_base").and_then(|m| m.get(rec["head"].as_str().unwrap_or(""))).and_then(|v| v.as_str()).unwrap_or("");
    !now.is_empty() && !then.is_empty() && now != then
}

/// The PR conflicts with its base: the host says so, or refused a merge for it on this head and base
/// (`merge_conflict`, noted by `tb pr merge`).
pub fn conflicting(f: &Row, rec: &Value) -> bool {
    let refused = f.get("merge_conflict").is_some_and(|m| {
        let head = rec["head"].as_str().unwrap_or("");
        !head.is_empty() && m["head"].as_str() == Some(head) && m["base_head"] == rec["base_head"]
    });
    conflicts(rec) == Some(true) || refused
}

/// A merge the host refused because the PR conflicts with its base: noted for this head and base, so the
/// stage moves to `rebase` instead of retrying the merge.
pub fn note_merge_conflict(app: &App, t: &Row, rec: &Value, who: &str) -> Result<()> {
    merge_flow(app, t.id(), fields!["merge_conflict" => json!({"head": rec["head"], "base_head": rec["base_head"], "at": now_iso()})])?;
    board::log_event(app, t.id(), who, "status", &format!("PR #{} can't merge: it conflicts with its base branch, so it needs a rebase", t.i0("pr_num")))?;
    Ok(())
}

/// Whether the PR conflicts with its base, as the host last said: GitHub's `mergeable`
/// ("CONFLICTING", "MERGEABLE") or a stream's bool. None: the host can't tell (yet).
pub fn conflicts(rec: &Value) -> Option<bool> {
    match &rec["mergeable"] {
        Value::Bool(m) => Some(!m),
        Value::String(s) if s == "CONFLICTING" => Some(true),
        Value::String(s) if s == "MERGEABLE" => Some(false),
        _ => None,
    }
}

/// The task's project's rebase policy says to rebase this push onto its base now.
pub fn needs_rebase(policy: RebasePolicy, f: &Row, rec: &Value) -> bool {
    match policy {
        RebasePolicy::Moved => base_moved(f, rec) || conflicts(rec) == Some(true),
        RebasePolicy::Conflicts => conflicts(rec) == Some(true),
        RebasePolicy::Never => false,
    }
}

/// The task's rebase policy (`pr.rebase`, or its project's own).
pub fn rebase_policy(app: &App, t: &Row) -> RebasePolicy {
    app.cfg.pr.rebase_for(t.s("project"))
}

/// Reviewers taken off the PR: a request for changes from one of them no longer holds.
/// `waiting_since` updated by a read: a reviewer on the PR who hasn't approved keeps the time the board
/// first saw them waiting; anyone else drops out, so asking them again starts a new wait. None when unchanged.
fn waiting_since(f: &Row, rec: &Value) -> Option<Value> {
    let had = f.get("waiting_since").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    let mut since = serde_json::Map::new();
    for r in rec["reviewers"].as_array().into_iter().flatten().filter(|r| r["requested"] == true && r["state"] != "approved") {
        let Some(user) = r["user"].as_str().filter(|u| !u.is_empty()) else { continue };
        since.insert(user.to_lowercase(), had.get(&user.to_lowercase()).cloned().unwrap_or_else(|| json!(now_iso())));
    }
    (since != had).then_some(Value::Object(since))
}

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

/// A request for changes only counts when it comes with comments: the reviewer started or spoke on
/// a thread (any, resolved too). A record without threads (older reads) takes every request.
fn commented(rec: &Value, user: &str) -> bool {
    let Some(threads) = rec["threads"].as_array() else { return true };
    !user.is_empty() && threads.iter().any(|t| t["author"].as_str() == Some(user) || t["last_author"].as_str() == Some(user))
}

pub fn review_of(f: &Row, rec: &Value) -> Review {
    let decision = rec["review_decision"].as_str().unwrap_or("").to_string();
    let Some(list) = rec["reviewers"].as_array() else {
        return Review { changes: decision == "CHANGES_REQUESTED", approvals: rec["approvals"].as_i64().unwrap_or(0), decision, requesters: vec![] };
    };
    let off = swapped_off(f);
    let on: Vec<&Value> = list.iter().filter(|r| !off.iter().any(|o| r["user"].as_str() == Some(o.as_str()))).collect();
    let requesters: Vec<(String, String)> = on
        .iter()
        .filter(|r| r["state"] == "changes" && commented(rec, r["user"].as_str().unwrap_or("")))
        .map(|r| (r["user"].as_str().unwrap_or("").to_string(), r["name"].as_str().unwrap_or("").to_string()))
        .collect();
    let changes = !requesters.is_empty() || (decision == "CHANGES_REQUESTED" && list.is_empty() && rec["comments"].as_i64().unwrap_or(1) > 0);
    // The board's own account (`viewer`) doesn't count toward approvals, as the PR's author doesn't.
    let viewer = rec["viewer"].as_str().filter(|v| !v.is_empty());
    let approvals = on.iter().filter(|r| r["state"] == "approved" && r["user"].as_str() != viewer).count() as i64;
    let decision = match decision.as_str() {
        "CHANGES_REQUESTED" if !changes => String::new(),
        _ if changes => "CHANGES_REQUESTED".to_string(),
        d => d.to_string(),
    };
    Review { changes, approvals, decision, requesters }
}

/// Whether agents may merge a project's PRs once they're approved and green: its own switch (`tb project
/// set --agents-merge`, `[pr.projects.<name>] agents_merge`), else the board's (`tb project
/// agents-merge`), else `pr.agents_merge`.
pub fn agents_merge_on(app: &App, project: Option<&str>) -> bool {
    crate::projects::pr_rules(app, project).agents_merge.unwrap_or_else(|| crate::projects::agents_merge_default(app))
}

/// Whether a green PR waits for the owner's review: the project's word (`tb project set
/// --owner-review`, `[pr.projects.<name>] owner_review`), else the board's (`tb project owner-review`),
/// else `pr.owner_review`. Off, `step` marks it reviewed for them.
pub fn owner_review_on(app: &App, project: Option<&str>) -> bool {
    crate::projects::pr_rules(app, project).owner_review.unwrap_or_else(|| crate::projects::owner_review_default(app))
}

/// Approvals this task's PR needs: its project's count, else `pr.approvals` (2). None (a count of 0):
/// the host's own verdict decides.
pub fn approvals_needed(app: &App, t: &Row) -> Option<i64> {
    Some(crate::projects::pr_rules(app, t.s("project")).approvals.unwrap_or(app.cfg.pr.approvals)).filter(|n| *n > 0)
}

/// Approved enough to merge: the count it needs, or (0) the host's verdict or any approval.
pub fn approved(app: &App, t: &Row, r: &Review) -> bool {
    if r.changes {
        return false;
    }
    match approvals_needed(app, t) {
        Some(n) => r.approvals >= n,
        None => r.decision == "APPROVED" || (r.decision.is_empty() && r.approvals > 0),
    }
}

/// This head's entry in `skip_checks` (`*` skips every push): a hook's reason (a string), or
/// `tb pr skip-checks`' `{reason, who, at}`.
fn skip_entry<'a>(f: &'a Row, rec: &Value) -> Option<&'a Value> {
    let skips = f.get("skip_checks")?.as_object()?;
    let head = rec["head"].as_str().unwrap_or("");
    skips.get(head).or_else(|| skips.get("*"))
}

/// Why this head's checks are skipped (a hook or `tb pr skip-checks`), if they are.
pub fn checks_skipped(f: &Row, rec: &Value) -> Option<String> {
    skip_entry(f, rec).map(|v| v.as_str().or(v["reason"].as_str()).unwrap_or("").to_string())
}

/// The skip covers failed checks too: only the owner's hooks can skip a failure. `tb pr skip-checks`
/// covers checks that were stopped, are still running or never posted; a failure still needs a fix or
/// `tb pr not-ours`.
pub fn skip_hides_failures(f: &Row, rec: &Value) -> bool {
    skip_entry(f, rec).is_some_and(|v| v.is_string())
}

pub fn phase_of(app: &App, t: &Row, rec: &Value) -> String {
    match rec["state"].as_str().unwrap_or("OPEN").to_uppercase().as_str() {
        "MERGED" => return "merged".into(),
        "CLOSED" | "DECLINED" | "SUPERSEDED" => return "declined".into(),
        _ => {}
    }
    let f = flow(t);
    // While the board's PR builds are stopped (`prbuilds.rs`), checks count as passed.
    let stopped = crate::prbuilds::stopped(app);
    let skipped = checks_skipped(&f, rec).is_some() || stopped;
    if !stopped && !skip_hides_failures(&f, rec) && !failing(&f, rec).is_empty() {
        return "fix".into();
    }
    if !skipped && (checks_waiting(app, t, &f, rec) || running_on_host(&f, rec)) {
        return "checks".into();
    }
    // A conflict holds everything after the checks (a failed check's wake already says to rebase): the
    // agent rebases before anyone reviews or merges it.
    if rebase_policy(app, t) != RebasePolicy::Never && conflicting(&f, rec) {
        return "rebase".into();
    }
    let seen = f.i0("comments_seen");
    let review = review_of(&f, rec);
    let answered = f.get("answered_changes").map(|a| *a == rec["changes_at"]).unwrap_or(false);
    let new_comments = if has_threads(rec) { !open_threads(&f, rec).is_empty() } else { rec["comments"].as_i64().unwrap_or(0) > seen };
    if (review.changes && !answered) || new_comments {
        return "comments".into();
    }
    let review_skipped = review_skipped(app, t, &f, rec);
    if review.changes && answered && !review_skipped {
        // The changes are pushed; the reviewer who asked for them hasn't looked again yet.
        return "rereview".into();
    }
    if (review_skipped || approved(app, t, &review)) && !rec["tasks_error"].is_string() {
        // PR tasks that couldn't be read are unknown, not none: they hold the stage as they hold
        // `tb pr merge`, so it stays awaiting review. A stacked PR waits for the PR it builds on to merge
        // first, and a PR waits for the head-merge PRs stacked on it to merge into it (`stack.rs`).
        if crate::stack::holds(app, t).unwrap_or(false) {
            return "waits".into();
        }
        if crate::stack::stack_hold(app, t).ok().flatten().is_some() {
            return "stack".into();
        }
        // The task's merge steps (`before = "merge"`) hold only the merge, judged on the PR's head.
        return if merge_steps_left(app, t, rec).is_empty() { "merge" } else { "steps" }.into();
    }
    // Lens reviewing a new PR (`lens_pr.rs`) comes before asking anyone: only stages that wait on someone
    // else make way for it, so it never holds a merge.
    if crate::lens_pr::holds(app, &f) {
        return "lens".into();
    }
    if crate::reviewers::ask_stage_on(app, t.s("project")) && f.contains_key("reviewed") && !f.contains_key("asked") {
        return "ask".into();
    }
    "review".into()
}

/// The task's merge steps that haven't passed on the PR's head (none when the PR goes into a branch they
/// don't hold, or they can't be read).
pub fn merge_steps_left(app: &App, t: &Row, rec: &Value) -> Vec<crate::steps::Step> {
    crate::steps::merge_missing(app, t, rec["head"].as_str(), rec["base"].as_str()).unwrap_or_default()
}

/// The review doesn't hold the merge: skipped for this push (`tb pr skip-review`), or the project has no
/// Review step (`tb project set --review off`).
pub fn review_skipped(app: &App, t: &Row, f: &Row, rec: &Value) -> bool {
    !crate::reviewers::review_on(app, t.s("project")) || f.get("skip_review").and_then(|v| v.as_object()).map(|m| m.contains_key(rec["head"].as_str().unwrap_or(""))).unwrap_or(false)
}

/// The checks are still going: one is running, or (since this push was first seen) the expected
/// checks haven't all posted within their wait, or none has posted within the `no_checks_after` grace.
pub fn checks_waiting(app: &App, t: &Row, f: &Row, rec: &Value) -> bool {
    if rec["running"].as_i64().unwrap_or(0) > 0 {
        return true;
    }
    match expected_missing(app, t, rec) {
        Some(missing) => !missing.is_empty() && !expected_wait_over(app, t, f, rec),
        None => rec["checks"].as_array().map(|a| a.is_empty()).unwrap_or(true) && !no_checks_wait_over(app, f, rec),
    }
}

/// This push has waited out the `no_checks_after` grace: with no expected checks, a push that posted
/// none by now needs none.
pub fn no_checks_wait_over(app: &App, f: &Row, rec: &Value) -> bool {
    head_waited(f, rec) >= app.cfg.pr.no_checks_after_mins * 60.0
}

/// `tb pr merge` was refused on this head while checks ran on the host (`checks_running`), and the record
/// doesn't show all of them finished yet: the checks are still going, for at most `RUNNING_ON_HOST_RECHECK`.
fn running_on_host(f: &Row, rec: &Value) -> bool {
    let Some(noted) = f.get("checks_running").filter(|v| v.is_object()) else { return false };
    let head = rec["head"].as_str().unwrap_or("");
    let at = noted["head"].as_str().unwrap_or("");
    if head.is_empty() || at.is_empty() || !(head.starts_with(at) || at.starts_with(head)) {
        return false;
    }
    if now_ts() - noted["at"].as_str().and_then(parse_iso).unwrap_or(0.0) >= RUNNING_ON_HOST_RECHECK {
        return false;
    }
    let checks = rec["checks"].as_array().cloned().unwrap_or_default();
    let finished = |name: &str| checks.iter().any(|c| c["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)) && c["state"] != "running");
    str_list(&noted["checks"]).iter().any(|n| !finished(n))
}

/// A merge was refused because these checks are still running on the host: the stage watches the build
/// (`running_on_host`) instead of counting the visit as a stalled merge.
pub fn note_checks_running(app: &App, t: &Row, head: &str, checks: &[String], who: &str) -> Result<()> {
    let head: String = head.chars().take(12).collect();
    merge_flow(app, t.id(), fields!["checks_running" => json!({"head": head, "checks": checks, "at": now_iso()})])?;
    let host = crate::prhost::PrRef::of(t).map(|p| crate::prhost::limit::label(&p.host)).unwrap_or_else(|| "its host".into());
    board::log_event(
        app,
        t.id(),
        who,
        "status",
        &format!("PR #{} can't merge yet: {} still running on {host}, so the board watches the build", t.i0("pr_num"), checks.join(", ")),
    )?;
    Ok(())
}

/// Seconds (awake) since this push was first seen.
pub(crate) fn head_waited(f: &Row, rec: &Value) -> f64 {
    let first = f.get("head_at").and_then(|h| h.get(rec["head"].as_str().unwrap_or(""))).and_then(|v| v.as_f64());
    first.map(crate::clock::awake_since).unwrap_or(0.0)
}

/// The project's `expected_wait_mins` (90 unless it says).
pub fn expected_wait_mins(app: &App, t: &Row) -> f64 {
    crate::projects::pr_rules(app, t.s("project")).expected_wait_mins.unwrap_or(crate::config::EXPECTED_WAIT_MINS)
}

/// How long this push waits for expected checks that haven't posted: `expected_wait_mins`, or only the
/// `no_checks_after` grace when they were learned from an earlier push (`earlier_checks`) and every check
/// this push posted has finished, as a check that no longer runs on the PR never posts.
pub fn missing_wait_mins(app: &App, t: &Row, rec: &Value) -> f64 {
    let full = expected_wait_mins(app, t);
    let checks = rec["checks"].as_array().cloned().unwrap_or_default();
    let finished = !checks.is_empty() && rec["running"].as_i64().unwrap_or(0) == 0 && checks.iter().all(|c| c["state"] != "running");
    if finished && expected_for(app, t, rec).source == Some("earlier") {
        app.cfg.pr.no_checks_after_mins.min(full)
    } else {
        full
    }
}

/// This push has waited out its expected checks: one that hasn't posted by now no longer holds the PR
/// (the stage moves on, and `tb pr merge` doesn't wait on it).
pub fn expected_wait_over(app: &App, t: &Row, f: &Row, rec: &Value) -> bool {
    head_waited(f, rec) >= missing_wait_mins(app, t, rec) * 60.0
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

/// `tb pr skip-checks`: this push's checks (or, with `all`, every push's) that were stopped, are running
/// or never posted count as passed. Never a failure: that's a fix, or `tb pr not-ours` with proof.
pub fn skip_checks(app: &App, t: &Row, reason: &str, all: bool, who: &str) -> Result<()> {
    let f = flow(t);
    let rec = f.get("rec").cloned().unwrap_or(Value::Null);
    let failed = failing(&f, &rec);
    if !failed.is_empty() {
        return err(
            409,
            format!(
                "PR #{}'s checks failed on this push: {}. tb pr skip-checks is for builds that were stopped or never ran, not failures. Fix them, or, if one fails without this PR too, clear it with tb pr not-ours --check \"<check>\" --title \"<what fails>\" --reason \"<why>\" --proof <link>.",
                t.i0("pr_num"),
                failed.join(", ")
            ),
        );
    }
    let head = rec["head"].as_str().filter(|h| !h.is_empty());
    let key = if all { "*" } else { head.unwrap_or("*") };
    let mut skips = f.get("skip_checks").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    skips.insert(key.to_string(), json!({"reason": reason, "who": who, "at": now_iso()}));
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
    let (mut f, heads) = with_head(t, rec);
    let head = rec["head"].as_str().unwrap_or("").to_string();
    let mut changes: Vec<(&str, Value)> = vec![("rec", rec.clone()), ("checked_at", json!(now_iso()))];
    // The owner's review is off: a green PR that would wait for them is marked reviewed by the board.
    if t.s("status") == Some("done")
        && !f.contains_key("reviewed")
        && !owner_review_on(app, t.s("project"))
        && phase_of(app, &probe(t, &f), rec) == "review"
    {
        let at = now_iso();
        f.insert("reviewed".into(), json!(at));
        changes.push(("reviewed", json!(at)));
        board::log_event(app, t.id(), board::BOARD, "status", &format!("PR #{}: marked reviewed for {} (owner review is off)", t.i0("pr_num"), app.cfg.owner))?;
    }
    if let Some(heads) = heads {
        changes.push(("head_at", heads));
    }
    if let Some(since) = waiting_since(&f, rec) {
        changes.push(("waiting_since", since));
    }
    if !head.is_empty() {
        changes.push(("head", json!(head)));
        // The checks this push posted, for a later push to wait on (`earlier_checks`).
        let mut by = f.get("checks_by_head").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        let mut have = by.get(&head).map(str_list).unwrap_or_default();
        let before = have.len();
        for c in rec["checks"].as_array().cloned().unwrap_or_default() {
            if let Some(n) = c["name"].as_str().filter(|n| !have.iter().any(|h| h.eq_ignore_ascii_case(n))) {
                have.push(n.to_string());
            }
        }
        if have.len() != before {
            by.insert(head.clone(), json!(have));
            changes.push(("checks_by_head", Value::Object(by)));
        }
        let mut bases = f.get("head_base").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        if let Some(b) = rec["base_head"].as_str().filter(|b| !b.is_empty() && !bases.contains_key(&head)) {
            bases.insert(head.clone(), json!(b));
            changes.push(("head_base", Value::Object(bases)));
        }
    }
    let old = t.st("pr_phase");
    // A merged PR stays merged: a stream event laid over the record from before the merge still reads open.
    let phase = if old == "merged" { old.clone() } else { phase_of(app, &probe(t, &f), rec) };
    let phase_changed = phase != old;
    changes.extend(crate::prpace::note(app, t, &f, rec, &phase, phase_changed)?);
    let build = if crate::prbuilds::stopped(app) {
        "Builds stopped"
    } else if checks_skipped(&f, rec).is_some() && (skip_hides_failures(&f, rec) || failing(&f, rec).is_empty()) {
        "Checks skipped"
    } else if !str_list(&rec["failed"]).is_empty() && failing(&f, rec).is_empty() {
        "Failed, not this PR"
    } else {
        build_label(rec)
    };
    let state = if phase == "merged" { "MERGED".to_string() } else { rec["state"].as_str().unwrap_or("OPEN").to_uppercase() };
    let mut task_fields = fields!["pr_state" => state,
                                  "pr_build" => build, "pr_review" => review_label(&review_of(&f, rec), approvals_needed(app, t))];
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
        if IN_CODE_REVIEW.contains(&phase.as_str()) && !IN_CODE_REVIEW.contains(&old.as_str()) {
            let st = app.cfg.jira.in_review.clone();
            board::jira_keep_in_step(app, t, Some(&st), None)?;
        }
        if phase == "merged" {
            board::log_event(app, t.id(), "PR", "status", &format!("PR #{} was merged", t.i0("pr_num")))?;
            crate::merged_note::ticket_merged(app, t, &merged_comment(t))?;
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
    let may_wake = app.cfg.pr.wake && (phase != "merge" || agents_merge_on(app, t.s("project")));
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
        && !visit_live(app, t)?
        && hours::goal_open(app, board::find_goal(app, t.i("goal_id"))?.as_ref())
        // The agent isn't brought back to ask for reviews while the PR feed holds asks.
        && (phase != "ask" || crate::asks::held(app, t)?.is_none())
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
            } else {
                changes.extend(fields!["merge_tries" => null, "merge_gave_up" => null]);
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
    // Merge steps the board can't bring the agent back for, or that a visit ended without passing: tell the
    // owner once per head.
    let steps_visit_over = f.s("handled") == Some(key.as_str()) || (f.s("woke") == Some(key.as_str()) && f.contains_key("stopped"));
    if phase == "steps"
        && t.s("status") == Some("done")
        && f.s("steps_alerted") != Some(head.as_str())
        && (!may_wake || steps_visit_over)
    {
        let left = crate::steps::names(&merge_steps_left(app, t, rec));
        dispatch::add_alert(
            app,
            &format!("PR #{} for {} is approved and green, but its merge waits on steps: {left}.", t.i0("pr_num"), rf("task", t.id())),
            Some(t.id()),
            t.i("goal_id"),
            None,
            Some("pr"),
        )?;
        changes.push(("steps_alerted", json!(head)));
    }
    if phase == "merge"
        && !agents_merge_on(app, t.s("project"))
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

/// `tasks.pr_review`: "Approved" once it has the approvals it needs (`need`, as `approved` counts them),
/// else "1 of 2 approved". With no count (0) the host's verdict, or any approval, decides.
fn review_label(r: &Review, need: Option<i64>) -> String {
    if r.changes || r.decision == "CHANGES_REQUESTED" {
        return "Changes requested".into();
    }
    match need {
        Some(n) if r.approvals >= n => "Approved".into(),
        Some(n) if r.approvals > 0 => format!("{} of {n} approved", r.approvals),
        None if r.decision == "APPROVED" || (r.decision.is_empty() && r.approvals > 0) => "Approved".into(),
        None if r.approvals > 0 => format!("{} approved", plural(r.approvals, "reviewer")),
        _ => "Not reviewed yet".into(),
    }
}

/// Reads every open PR the board's tasks made, on every watched host, and steps each one.
pub fn refresh(app: &App) -> Result<i64> {
    refresh_where(app, false)
}

/// The timed PR poll: [`refresh`], skipping the PRs a healthy stream relay covers until their reconcile
/// is due (`feed::pr_poll_skips`), and the PRs not due at their pace (`prpace.rs`).
pub fn poll(app: &App) -> Result<i64> {
    refresh_where(app, true)
}

fn refresh_where(app: &App, skip_covered: bool) -> Result<i64> {
    if !app.cfg.pr.watch {
        return Ok(0);
    }
    let tasks = app.db.q(
        "SELECT * FROM tasks WHERE pr_host IN ('github', 'bitbucket') AND pr_num IS NOT NULL AND (pr_phase IS NULL OR pr_phase NOT IN ('merged', 'declined'))",
        p![],
    )?;
    let mut by_host: std::collections::BTreeMap<String, Vec<Row>> = Default::default();
    for t in tasks {
        if skip_covered && (crate::feed::pr_poll_skips(app, &t) || !crate::prpace::due(app, &t)?) {
            continue;
        }
        by_host.entry(t.st("pr_host")).or_default().push(t);
    }
    let mut changed = 0;
    for (host, tasks) in by_host {
        let (polled, calls) = crate::prhost::limit::counting(|| poll_host(app, &host, &tasks));
        let (stepped, full) = polled?;
        changed += stepped;
        crate::prhost::limit::limits().note_poll(&host, tasks.len(), full, calls.get(&host).copied().unwrap_or(0), now_ts());
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

/// The poll reads a PR in full at least this often even while its stamp stays put: a stamp can miss
/// a change (a thread resolved on Bitbucket, a base the PR no longer merges into cleanly).
const FULL_READ_EVERY: f64 = 3600.0;

/// Polls one host's PRs: what changed on all of them in one batch ([`PrHost::stamps`]), then a full
/// read of each one that changed, and steps them all. How many stepped to a new stage, and how many
/// were read in full.
///
/// [`PrHost::stamps`]: crate::prhost::PrHost::stamps
fn poll_host(app: &App, host: &str, tasks: &[Row]) -> Result<(i64, usize)> {
    let stamps = stamps(app, host, tasks);
    let (mut changed, mut full) = (0, 0);
    for (t, stamp) in tasks.iter().zip(stamps) {
        let rec = match unchanged(t, stamp.as_deref()) {
            // The host said it hasn't changed: that counts as a read for the relay's reconcile (`feed::pr_poll_skips`).
            Some(rec) => {
                merge_flow(app, t.id(), fields!["read_at" => now_iso()])?;
                rec
            }
            None => match read(app, t) {
                Ok(rec) => {
                    full += 1;
                    merge_flow(app, t.id(), vec![("stamp", stamp.map_or(Value::Null, Value::from)), ("full_at", json!(now_iso()))])?;
                    rec
                }
                Err(e) => {
                    app.info(format!("prs: couldn't read {}: {e}", t.st("pr_url")));
                    continue;
                }
            },
        };
        crate::lens_pr::kick(app, t.id(), &rec)?;
        let t = board::get_task(app, t.id())?;
        note_expected(app, &t, &rec)?;
        gate(app, &board::get_task(app, t.id())?, &rec)?;
        let t = board::get_task(app, t.id())?;
        if app.db.tx(|| step(app, &t, &rec))? {
            changed += 1;
        }
    }
    Ok((changed, full))
}

/// Each task's PR stamp, in order; every one None when the host couldn't say (each is read in full).
fn stamps(app: &App, host: &str, tasks: &[Row]) -> Vec<Option<String>> {
    use crate::prhost::{Look, PrRef};
    let refs: Vec<Option<PrRef>> = tasks.iter().map(PrRef::of).collect();
    let looks: Vec<Look> = refs.iter().zip(tasks).filter_map(|(pr, t)| Some(Look { pr: pr.clone()?, checks: waits_on_checks(t) })).collect();
    // Like a read, an open PR's stamp still goes while the account restarts slowly after a limit.
    let got = crate::prhost::host_for(app, host).and_then(|h| crate::prhost::limit::priority(|| h.stamps(&looks)));
    let mut got = match got {
        Ok(got) if got.len() == looks.len() => got.into_iter(),
        Ok(_) => return vec![None; tasks.len()],
        Err(e) => {
            app.info(format!("prs: couldn't ask {host} what changed, so every PR there is read in full: {e}"));
            return vec![None; tasks.len()];
        }
    };
    refs.iter().map(|pr| pr.as_ref().and_then(|_| got.next().flatten())).collect()
}

/// The stamp reads the PR's checks too: it waits on them, or they're running or failed (a rerun moves
/// them without a push).
fn waits_on_checks(t: &Row) -> bool {
    let rec = flow(t).get("rec").cloned().unwrap_or(Value::Null);
    t.s("pr_phase") == Some("checks") || rec["running"].as_i64().unwrap_or(0) > 0 || rec["failed"].as_array().is_some_and(|f| !f.is_empty())
}

/// The PR's last record, when its stamp is the one its last full read had and that read is under
/// [`FULL_READ_EVERY`] old.
fn unchanged(t: &Row, stamp: Option<&str>) -> Option<Value> {
    let f = flow(t);
    let stamp = stamp?;
    let full_at = f.get("full_at").and_then(|a| a.as_str()).and_then(parse_iso)?;
    if f.get("stamp").and_then(|s| s.as_str()) != Some(stamp) || now_ts() - full_at >= FULL_READ_EVERY {
        return None;
    }
    f.get("rec").filter(|r| r.is_object()).cloned()
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

/// The ticket's comment when its PR merges, so whoever reads the ticket sees what merged.
pub fn merged_comment(t: &Row) -> String {
    match t.s("pr_url").filter(|u| !u.is_empty()) {
        Some(url) => format!("PR #{} was merged: {url}", t.i0("pr_num")),
        None => format!("PR #{} was merged", t.i0("pr_num")),
    }
}

pub fn mark_merged(app: &App, t: &Row, who: &str) -> Result<()> {
    if t.s("pr_phase") == Some("merged") {
        return Ok(());
    }
    board::update_task(app, t.id(), fields!["pr_state" => "MERGED", "pr_phase" => "merged"])?;
    merge_flow(app, t.id(), fields!["woke" => null, "stopped" => null])?;
    board::log_event(app, t.id(), who, "status", &format!("Merged PR #{}", t.i0("pr_num")))?;
    dispatch::clear_alerts(app, Some(t.id()), None)?;
    crate::merged_note::ticket_merged(app, &board::get_task(app, t.id())?, &merged_comment(t))?;
    Ok(())
}

/// Records the task's PR as closed without merging (`tb pr close`), as the board reads a PR closed on
/// its host.
pub fn mark_declined(app: &App, t: &Row, who: &str, reason: &str) -> Result<()> {
    if t.s("pr_phase") == Some("declined") {
        return Ok(());
    }
    let state = if t.s("pr_host") == Some("bitbucket") { "DECLINED" } else { "CLOSED" };
    board::update_task(app, t.id(), fields!["pr_state" => state, "pr_phase" => "declined"])?;
    merge_flow(app, t.id(), fields!["woke" => null, "stopped" => null])?;
    let why = if reason.is_empty() { String::new() } else { format!(": {reason}") };
    board::log_event(app, t.id(), who, "status", &format!("Closed PR #{} without merging{why}", t.i0("pr_num")))?;
    dispatch::clear_alerts(app, Some(t.id()), None)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wake_inside_a_transaction_claims_its_slot_on_the_deferred_worker_first() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::for_tests(crate::config::Config::for_tests(dir.path()));
        std::sync::Arc::get_mut(&mut app).unwrap().inline_deferred = false;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo = repo.to_str().unwrap();
        let run = |args: &[&str]| assert_eq!(crate::worktrees::git(repo, args, 30.0).code, 0, "git {args:?}");
        run(&["init", "-q"]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "one"]);
        let slot = format!("{repo}/.claude/worktrees/slot-1");
        let ctx = json!({"worktree_slot": slot}).to_string();
        let id = app.db.insert("tasks", fields!["title" => "A", "status" => "done", "pr_num" => 7, "context" => ctx]).unwrap();
        let t = board::get_task(&app, id).unwrap();
        let jobs = || app.db.q("SELECT * FROM jobs WHERE task_id = ?", p![id]).unwrap();
        for _ in 0..2 {
            assert_eq!(app.db.tx(|| follow_wake(&app, &t, "Rebase")).unwrap(), None, "no checkout while the database is held");
        }
        assert!(jobs().is_empty() && !Path::new(&slot).exists());
        let claim = app.next_deferred(std::time::Duration::ZERO).expect("the claim waits for the worker");
        assert!(app.next_deferred(std::time::Duration::ZERO).is_none(), "the same wake queues one claim");
        claim(&app);
        assert!(Path::new(&slot).is_dir(), "the worker made the slot");
        assert!(jobs().is_empty(), "the worker only claims; the caller's next poll wakes");
        assert!(app.db.tx(|| follow_wake(&app, &t, "Rebase")).unwrap().is_some());
        let made = jobs();
        assert_eq!(made.len(), 1, "then woke the task in it");
        let args: Value = serde_json::from_str(&made[0].st("args")).unwrap();
        assert_eq!(args["cwd"], json!(slot));
    }

    #[test]
    fn the_review_label_counts_approvals_against_the_ones_needed() {
        let r = |approvals: i64, decision: &str| Review { changes: decision == "CHANGES_REQUESTED", approvals, decision: decision.into(), requesters: vec![] };
        assert_eq!(review_label(&r(1, ""), Some(2)), "1 of 2 approved");
        assert_eq!(review_label(&r(2, ""), Some(2)), "Approved", "Bitbucket never says APPROVED; the count does");
        assert_eq!(review_label(&r(3, ""), Some(2)), "Approved");
        assert_eq!(review_label(&r(1, "APPROVED"), Some(2)), "1 of 2 approved", "the host's verdict below the board's count");
        assert_eq!(review_label(&r(0, "REVIEW_REQUIRED"), Some(2)), "Not reviewed yet");
        assert_eq!(review_label(&r(1, "CHANGES_REQUESTED"), Some(2)), "Changes requested");
        assert_eq!(review_label(&r(1, "APPROVED"), None), "Approved", "no count: the host decides");
        assert_eq!(review_label(&r(2, ""), None), "Approved", "no count, no verdict: any approval");
        assert_eq!(review_label(&r(1, "REVIEW_REQUIRED"), None), "1 reviewer approved");
    }

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
