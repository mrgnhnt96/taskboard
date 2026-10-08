//! Pull requests after a task is done: watch GitHub PRs with `gh`, work out their stage, and bring the
//! task's conversation back when the PR needs work (a failed check, review comments, ready to merge).

use std::path::Path;

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, dispatch, fields, handoff, hours, p, proc, runner};

pub const WAKE: &[&str] = &["fix", "comments", "merge"];
pub const IN_REVIEW: &[&str] = &["review", "comments", "merge"];
const FINISHED: &[&str] = &["merged", "declined"];
const WAKE_RETRY_WAITS: [i64; 3] = [60, 300, 900];

pub fn label(phase: &str) -> &str {
    match phase {
        "checks" => "Watching checks",
        "fix" => "Fixing checks",
        "review" => "Awaiting review",
        "comments" => "Addressing comments",
        "merge" => "Ready to merge",
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
    let watched = pr.host == "github" && app.cfg.pr.watch;
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
    WAKE.contains(&t.s("pr_phase").unwrap_or("")) && flow(t).contains_key("wake_tries")
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
    format!("{phase}:{}:{}", rec["head"].as_str().unwrap_or(""), rec["comments"].as_i64().unwrap_or(0))
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
            "A check failed on its head ({}). Run {tb} pr status {r}, then read the failing check's log (for GitHub: \
             gh pr checks {num} and gh run view --log-failed). Work from that error, not a guess. If the PR's change causes \
             it, fix exactly that in this task's worktree, run the tests, and push; the checks run again. If the base \
             branch has moved, rebase onto it before you push (git push --force-with-lease). If the failure isn't this \
             PR's (the base branch fails it too), say so with {tb} note and don't change unrelated code.",
            if failed.is_empty() { "see the PR".to_string() } else { failed.join(", ") },
            num = t.i0("pr_num")
        ),
        "comments" => format!(
            "There are new review comments{}. Read them (for GitHub: gh pr view {num} --comments, and the review threads \
             on the PR). Address each in the worktree; a comment that can fairly wait for a later PR goes to the backlog \
             instead ({tb} backlog add \"<title>\" --kind follow). Reply to every comment that asked for something with \
             what you did, push the fixes, and resolve what you addressed.",
            if rec["review_decision"] == "CHANGES_REQUESTED" { " and a reviewer asked for changes" } else { "" },
            num = t.i0("pr_num")
        ),
        "merge" => format!(
            "It's approved and every check is green. Merge it with the repository's default strategy (for GitHub: \
             gh pr merge {num}), then run {tb} pr merged {r}.",
            num = t.i0("pr_num")
        ),
        _ => String::new(),
    };
    [
        head,
        body,
        format!(
            "When you're finished, run {tb} pr wait {r}: the board watches the PR and brings you back when it needs you \
             again. Don't take another task here, and don't run tb done: the task is already done."
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
    match cid {
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
        if !t.b("auto_close") || board::task_for_session(app, s.s("id"))?.is_some() || board::close_rule(Some(&s)).is_none() {
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

/// One GitHub PR as `gh` reports it, boiled down to what the stage needs.
pub fn read_github(app: &App, t: &Row) -> std::result::Result<Value, String> {
    let gh = proc::which(&app.cfg.pr.gh).ok_or_else(|| "the gh command isn't installed".to_string())?;
    let args: Vec<String> = [
        "pr",
        "view",
        &t.st("pr_url"),
        "--json",
        "number,state,title,author,headRefOid,headRefName,baseRefName,reviewDecision,statusCheckRollup,comments,reviews,mergeable",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let o = proc::run(&gh, &args, None, 30.0).map_err(|_| "gh didn't answer in 30 seconds".to_string())?;
    if o.code != Some(0) {
        return Err(o.stderr.lines().next().unwrap_or("gh failed").to_string());
    }
    let d: Value = serde_json::from_str(&o.stdout).map_err(|e| format!("gh's answer doesn't parse: {e}"))?;
    Ok(summarize_github(&d))
}

pub fn summarize_github(d: &Value) -> Value {
    let author = d["author"]["login"].as_str().unwrap_or("");
    let mut failed = vec![];
    let mut running = 0;
    let mut checks = vec![];
    for c in d["statusCheckRollup"].as_array().cloned().unwrap_or_default() {
        let name = c["name"].as_str().or(c["context"].as_str()).unwrap_or("check").to_string();
        let state = match (c["status"].as_str(), c["conclusion"].as_str(), c["state"].as_str()) {
            (_, _, Some(s)) => match s {
                "SUCCESS" => "passed",
                "FAILURE" | "ERROR" => "failed",
                _ => "running",
            },
            (Some("COMPLETED"), Some(con), _) => match con {
                "SUCCESS" | "NEUTRAL" | "SKIPPED" => "passed",
                _ => "failed",
            },
            _ => "running",
        };
        if state == "failed" {
            failed.push(name.clone());
        }
        if state == "running" {
            running += 1;
        }
        checks.push(json!({"name": name, "state": state}));
    }
    let comments = d["comments"].as_array().map(|a| a.iter().filter(|c| c["author"]["login"].as_str() != Some(author)).count()).unwrap_or(0)
        + d["reviews"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|r| r["author"]["login"].as_str() != Some(author))
                    .filter(|r| r["state"] != "APPROVED")
                    .filter(|r| r["state"] == "CHANGES_REQUESTED" || r["body"].as_str().map(|b| !b.trim().is_empty()).unwrap_or(false))
                    .count()
            })
            .unwrap_or(0);
    let changes_at = d["reviews"]
        .as_array()
        .and_then(|a| {
            a.iter()
                .filter(|r| r["state"] == "CHANGES_REQUESTED" && r["author"]["login"].as_str() != Some(author))
                .filter_map(|r| r["submittedAt"].as_str())
                .max()
                .map(|s| s.to_string())
        });
    let approvals = d["reviews"]
        .as_array()
        .map(|a| a.iter().filter(|r| r["state"] == "APPROVED" && r["author"]["login"].as_str() != Some(author)).count())
        .unwrap_or(0);
    json!({
        "state": d["state"].as_str().unwrap_or("OPEN"), "title": d["title"], "head": d["headRefOid"],
        "branch": d["headRefName"], "base": d["baseRefName"], "review_decision": d["reviewDecision"].as_str().unwrap_or(""),
        "checks": checks, "failed": failed, "running": running, "comments": comments, "approvals": approvals,
        "mergeable": d["mergeable"], "changes_at": changes_at,
    })
}

pub fn phase_of(app: &App, t: &Row, rec: &Value) -> String {
    match rec["state"].as_str().unwrap_or("OPEN").to_uppercase().as_str() {
        "MERGED" => return "merged".into(),
        "CLOSED" | "DECLINED" => return "declined".into(),
        _ => {}
    }
    if rec["failed"].as_array().map(|a| !a.is_empty()).unwrap_or(false) {
        return "fix".into();
    }
    let checks = rec["checks"].as_array().map(|a| a.len()).unwrap_or(0);
    let running = rec["running"].as_i64().unwrap_or(0) > 0;
    let f = flow(t);
    let first = f.get("head_at").and_then(|h| h.get(rec["head"].as_str().unwrap_or(""))).and_then(|v| v.as_f64());
    let waited = first.map(crate::clock::awake_since).unwrap_or(0.0);
    if running || (checks == 0 && waited < app.cfg.pr.no_checks_after_mins * 60.0) {
        return "checks".into();
    }
    let seen = f.i0("comments_seen");
    let decision = rec["review_decision"].as_str().unwrap_or("");
    let answered = f.get("answered_changes").map(|a| *a == rec["changes_at"]).unwrap_or(false);
    if (decision == "CHANGES_REQUESTED" && !answered) || rec["comments"].as_i64().unwrap_or(0) > seen {
        return "comments".into();
    }
    if decision == "APPROVED" || (decision.is_empty() && rec["approvals"].as_i64().unwrap_or(0) > 0) {
        return "merge".into();
    }
    "review".into()
}

/// Applies a fresh PR record to its task: stage, Jira, wakes.
pub fn step(app: &App, t: &Row, rec: &Value) -> Result<bool> {
    let mut f = flow(t);
    let head = rec["head"].as_str().unwrap_or("").to_string();
    let mut changes: Vec<(&str, Value)> = vec![("rec", rec.clone()), ("checked_at", json!(now_iso()))];
    if !head.is_empty() {
        let mut heads = f.get("head_at").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        if !heads.contains_key(&head) {
            heads.insert(head.clone(), json!(now_ts()));
            f.insert("head_at".into(), Value::Object(heads.clone()));
            changes.push(("head_at", Value::Object(heads)));
        }
        changes.push(("head", json!(head)));
    }
    let mut probe = t.clone();
    probe.insert("pr_flow".into(), json!(jdumps(&Value::Object(f.clone()))));
    let phase = phase_of(app, &probe, rec);
    let old = t.st("pr_phase");
    let phase_changed = phase != old;
    let mut task_fields = fields!["pr_state" => rec["state"].as_str().unwrap_or("OPEN").to_uppercase(),
                                  "pr_build" => build_label(rec), "pr_review" => review_label(rec)];
    if let Some(title) = rec["title"].as_str() {
        task_fields.push(("pr_title", json!(title)));
    }
    if phase_changed {
        changes.extend(fields!["stopped" => null, "handled" => null]);
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
    if t.s("status") == Some("done") && phase == "review" && !f.contains_key("reviewed") && !f.contains_key("review_alerted") {
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
    if may_wake
        && WAKE.contains(&phase.as_str())
        && t.s("status") == Some("done")
        && f.s("woke") != Some(key.as_str())
        && f.s("handled") != Some(key.as_str())
        && !retry_later(&f)
        && live_job(app, t)?.is_none()
        && hours::goal_open(app, board::find_goal(app, t.i("goal_id"))?.as_ref())
    {
        let mut probe = t.clone();
        probe.insert("pr_phase".into(), json!(phase));
        if wake(app, &probe, &phase, rec, None)?.is_some() {
            changes.extend(fields!["woke" => key, "stopped" => null]);
        }
    }
    if phase == "merge" && !app.cfg.pr.agents_merge && t.s("status") == Some("done") && f.s("merge_alerted") != Some(head.as_str()) {
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

fn review_label(rec: &Value) -> String {
    match rec["review_decision"].as_str().unwrap_or("") {
        "APPROVED" => "Approved".into(),
        "CHANGES_REQUESTED" => "Changes requested".into(),
        _ if rec["approvals"].as_i64().unwrap_or(0) > 0 => format!("{} approved", plural(rec["approvals"].as_i64().unwrap_or(0), "reviewer")),
        _ => "Not reviewed yet".into(),
    }
}

/// Reads every open GitHub PR the board's tasks made and steps each one.
pub fn refresh(app: &App) -> Result<i64> {
    if !app.cfg.pr.watch {
        return Ok(0);
    }
    let tasks = app.db.q(
        "SELECT * FROM tasks WHERE pr_host = 'github' AND pr_num IS NOT NULL AND (pr_phase IS NULL OR pr_phase NOT IN ('merged', 'declined'))",
        p![],
    )?;
    let mut changed = 0;
    for t in tasks {
        match read_github(app, &t) {
            Ok(rec) => {
                let t = board::get_task(app, t.id())?;
                if app.db.tx(|| step(app, &t, &rec))? {
                    changed += 1;
                }
            }
            Err(e) => app.info(format!("prs: couldn't read {}: {e}", t.st("pr_url"))),
        }
    }
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
    fn summarizes_gh_output() {
        let d = json!({
            "state": "OPEN", "title": "Add x", "author": {"login": "me"}, "headRefOid": "abc",
            "reviewDecision": "CHANGES_REQUESTED",
            "statusCheckRollup": [
                {"name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"},
                {"name": "test", "status": "COMPLETED", "conclusion": "FAILURE"},
                {"context": "ci/legacy", "state": "PENDING"}
            ],
            "comments": [{"author": {"login": "me"}}, {"author": {"login": "rev"}}],
            "reviews": [{"author": {"login": "rev"}, "state": "CHANGES_REQUESTED", "body": ""}]
        });
        let r = summarize_github(&d);
        assert_eq!(r["failed"], json!(["test"]));
        assert_eq!(r["running"], 1);
        assert_eq!(r["comments"], 2);
        assert_eq!(build_label(&r), "Failed");
    }
}
