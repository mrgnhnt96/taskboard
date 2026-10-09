//! Tasks that need another task's work first (`tb wait-for T<n>`).

use serde_json::{json, Value};

use crate::app::App;
use crate::board;
use crate::util::*;
use crate::{deliver, fields, hours, p, prflow};

pub const FOLLOW: &str = "follow";

pub fn ids(t: &Row) -> Vec<i64> {
    jloads_arr(t.s("waits_for"))
        .iter()
        .filter_map(|x| x.as_i64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
        .collect()
}

/// Every task this one needs first: what it waits for, and the task it stacks on (`stack.rs`).
pub fn deps(t: &Row) -> Vec<i64> {
    let mut all = ids(t);
    if let Some(p) = crate::stack::parent_id(t) {
        if !all.contains(&p) {
            all.push(p);
        }
    }
    all
}

pub fn clean(app: &App, value: &Value, t: Option<&Row>) -> Result<Option<String>> {
    let items: Vec<String> = match value {
        Value::Null => return Ok(None),
        Value::String(s) if s.trim().is_empty() || s.trim().eq_ignore_ascii_case("none") => return Ok(None),
        Value::String(s) => s.replace(',', " ").split_whitespace().map(|x| x.to_string()).collect(),
        Value::Array(a) if a.is_empty() => return Ok(None),
        Value::Array(a) => a.iter().map(|x| x.as_str().map(|s| s.to_string()).unwrap_or_else(|| x.to_string())).collect(),
        other => vec![other.to_string()],
    };
    let mut out: Vec<i64> = vec![];
    for v in items {
        if v.eq_ignore_ascii_case("none") {
            continue;
        }
        let Some(n) = parse_ref_str(&v, "task")? else { continue };
        if let Some(t) = t {
            if n == t.id() {
                return err(400, "A task can't wait for itself.");
            }
        }
        let other = board::get_task(app, n)?;
        if let Some(t) = t {
            if other.s("project") != t.s("project") {
                return err(
                    409,
                    format!(
                        "{} is in {}, not {}, so its work can't be brought into this task.",
                        rf("task", n),
                        other.st("project"),
                        t.st("project")
                    ),
                );
            }
            if reaches(app, n, t.id(), &mut vec![])? {
                return err(409, format!("{} already waits for {}, so this would wait forever.", rf("task", n), rf("task", t.id())));
            }
        }
        if !out.contains(&n) {
            out.push(n);
        }
    }
    Ok(if out.is_empty() { None } else { Some(jdumps(&json!(out))) })
}

pub fn reaches(app: &App, start: i64, target: i64, seen: &mut Vec<i64>) -> Result<bool> {
    if start == target {
        return Ok(true);
    }
    if seen.contains(&start) {
        return Ok(false);
    }
    seen.push(start);
    let Some(t) = board::find_task(app, Some(start))? else { return Ok(false) };
    for n in deps(&t) {
        if reaches(app, n, target, seen)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn label(app: &App, other: &Row) -> Result<String> {
    let g = board::find_goal(app, other.i("goal_id"))?;
    Ok(rf("task", other.id()) + &g.map(|g| format!(" ({})", rf("goal", g.id()))).unwrap_or_default())
}

pub fn ready(other: Option<&Row>) -> bool {
    other.map(|o| o.s("status") == Some("done") && !o.b("failed")).unwrap_or(false)
}

/// The tasks this one waits for until their PR merges, not just until they're done (`tb wait-for --merged`).
pub fn merged_ids(t: &Row) -> Vec<i64> {
    board::task_context(t)
        .get("waits_merged")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
        .unwrap_or_default()
}

/// Adds tasks to the ones this one waits for until their PR merges.
pub fn set_merged(app: &App, t: &Row, wanted: &[i64]) -> Result<()> {
    let mut ctx = board::task_context(t);
    let mut all = merged_ids(t);
    for n in wanted {
        if !all.contains(n) {
            all.push(*n);
        }
    }
    ctx.insert("waits_merged".into(), json!(all));
    board::save_context(app, t.id(), &ctx, false)
}

/// Forgets every merge wait (`tb wait-for none`).
pub fn clear_merged(app: &App, t: &Row) -> Result<()> {
    let mut ctx = board::task_context(t);
    if ctx.remove("waits_merged").is_some() {
        board::save_context(app, t.id(), &ctx, false)?;
    }
    Ok(())
}

fn pr_merged(o: &Row) -> bool {
    o.st("pr_state").eq_ignore_ascii_case("MERGED") || o.s("pr_phase") == Some("merged")
}

fn pr_declined(o: &Row) -> bool {
    matches!(o.st("pr_state").to_uppercase().as_str(), "CLOSED" | "DECLINED" | "SUPERSEDED") || o.s("pr_phase") == Some("declined")
}

/// Done and not failed, and its work is on the base branch: its PR merged, or it ships no PR.
pub fn merged(app: &App, other: Option<&Row>) -> Result<bool> {
    let Some(o) = other.filter(|o| ready(Some(o))) else { return Ok(false) };
    if pr_merged(o) {
        return Ok(true);
    }
    if o.i("pr_num").is_some() {
        return Ok(false);
    }
    Ok(has(o.s("no_pr")) || !crate::projects::task_ships_pr(app, o)?)
}

/// Whether `t` can go on with task `n`'s work: done, or merged when `t` waits for its PR to merge.
pub fn ready_for(app: &App, t: &Row, n: i64, other: Option<&Row>) -> Result<bool> {
    if merged_ids(t).contains(&n) {
        merged(app, other)
    } else {
        Ok(ready(other))
    }
}

pub fn blocker(app: &App, t: &Row) -> Result<Option<String>> {
    for n in deps(t) {
        let other = board::find_task(app, Some(n))?;
        let Some(other) = other else { continue };
        if ready_for(app, t, n, Some(&other))? {
            continue;
        }
        let who = label(app, &other)?;
        let pr = other.i("pr_num").map(|p| format!("PR #{p}"));
        return Ok(Some(match other.s("status") {
            Some("done") if !other.b("failed") => match pr {
                Some(pr) if pr_declined(&other) => format!("Blocked by {who}, whose {pr} was declined, so its work isn't merged"),
                Some(pr) => format!("Blocked by {who} until {pr} merges"),
                None => format!("Blocked by {who} until its PR merges"),
            },
            Some("done") =>format!("Blocked by {who}, which failed. It starts once {} is done", rf("task", n)),
            Some("planned") => format!("Blocked by {who}, which isn't queued yet"),
            _ => format!("Blocked by {who}"),
        }));
    }
    Ok(None)
}

/// The line a queued card shows when something holds it back.
pub fn waiting_line(app: &App, t: &Row) -> Result<Value> {
    if t.i("start_job").is_some() {
        return Ok(Value::Null);
    }
    if let Some(b) = blocker(app, t)? {
        return Ok(json!(b));
    }
    if let Some(why) = crate::jira::ticket_wait(app, t)? {
        return Ok(json!(why));
    }
    if has(t.s("line_session")) && crate::lines::held_by_clock(app, t)? {
        return Ok(json!(hours::blocker(app)));
    }
    if t.s("pickup") == Some("manual") {
        return Ok(json!("Waits for you to press Start"));
    }
    if let Some(r) = t.s("retry_at").filter(|r| r > &now_iso().as_str()) {
        return Ok(json!(format!("Trying again at {}", local_clock(Some(r)))));
    }
    if let Some(g) = board::find_goal(app, t.i("goal_id"))? {
        if !hours::goal_open(app, Some(&g)) {
            return Ok(json!(hours::blocker(app)));
        }
        if let Some(why) = crate::runner::goal_blocker(app, t, &g)? {
            return Ok(json!(why));
        }
    } else if let Some(why) = hours::blocker(app) {
        return Ok(json!(why));
    }
    if let Some(why) = crate::locks::blocker(app, t)? {
        return Ok(json!(why));
    }
    Ok(crate::devices::blocker(app, t)?.map(Value::String).unwrap_or(Value::Null))
}

pub fn blocked_by(app: &App, t: &Row) -> Result<Vec<Value>> {
    if !board::is_blocked(app, t)? {
        return Ok(vec![]);
    }
    let mut out = vec![];
    for n in deps(t) {
        if let Some(o) = board::find_task(app, Some(n))? {
            if !ready_for(app, t, n, Some(&o))? {
                out.push(board::task_card(app, &o)?);
            }
        }
    }
    Ok(out)
}

pub fn waiting_on(app: &App, t: &Row) -> Result<Vec<Row>> {
    app.db.q(
        "SELECT * FROM tasks WHERE status != 'done' AND id != ? AND (pr_after = ? OR (waits_for IS NOT NULL \
         AND EXISTS (SELECT 1 FROM json_each(tasks.waits_for) WHERE value = ?))) ORDER BY id",
        p![t.id(), t.id(), t.id()],
    )
}

pub fn branch_of(t: &Row) -> Option<String> {
    board::task_context(t)
        .get("where")
        .and_then(|w| w.get("branch"))
        .and_then(|b| b.as_str())
        .filter(|b| !b.is_empty())
        .map(|b| b.to_string())
}

fn git_head(path: Option<&str>, branch: Option<&str>) -> Option<String> {
    let (path, branch) = (path?, branch?);
    if !std::path::Path::new(path).is_dir() {
        return None;
    }
    let git = crate::proc::which("git")?;
    let args: Vec<String> = vec!["-C".into(), path.into(), "rev-parse".into(), "--verify".into(), "-q".into(), format!("refs/heads/{branch}^{{commit}}")];
    let out = crate::proc::run(&git, &args, None, 5.0).ok()?;
    if out.code != Some(0) {
        return None;
    }
    Some(out.stdout.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn head_of(t: &Row) -> Option<String> {
    let flow = jloads_obj(t.s("pr_flow"));
    if let Some(h) = flow.get("head").and_then(|v| v.as_str()) {
        if t.s("pr_state").unwrap_or("OPEN").eq_ignore_ascii_case("OPEN") {
            return Some(h.to_string());
        }
    }
    let ctx = board::task_context(t);
    let worktree = ctx.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str()).map(|s| s.to_string());
    git_head(worktree.as_deref().or(t.s("repo_path")), branch_of(t).as_deref())
        .or_else(|| flow.get("head").and_then(|v| v.as_str()).map(|s| s.to_string()))
}

fn where_it_is(app: &App, other: &Row) -> Result<String> {
    let branch = branch_of(other);
    let mut bits = vec![format!("{} “{}”", label(app, other)?, short(&other.st("title"), 60))];
    let merged = other.st("pr_state").eq_ignore_ascii_case("MERGED");
    if merged {
        bits.push(format!("is merged (PR #{}), so its work is on the main branch", other.i0("pr_num")));
    } else if other.i("pr_num").is_some() {
        bits.push(format!(
            "is in PR #{} ({}) on branch {}",
            other.i0("pr_num"),
            other.st("pr_url"),
            branch.as_deref().unwrap_or("its branch")
        ));
    } else if let Some(b) = &branch {
        bits.push(format!("is on branch {b}"));
    } else {
        bits.push("is done; its branch isn't known, so look for it with git branch -a or its log".into());
    }
    if let Some(h) = head_of(other) {
        if !merged {
            bits.push(format!("at {}", &h[..h.len().min(12)]));
        }
    }
    Ok(bits.join(" "))
}

pub fn bring_in_text(app: &App, t: &Row) -> Result<String> {
    let mut lines = vec![];
    for n in deps(t) {
        let other = board::find_task(app, Some(n))?;
        if ready_for(app, t, n, other.as_ref())? {
            lines.push(format!("- {}", where_it_is(app, other.as_ref().unwrap())?));
        }
    }
    if lines.is_empty() {
        return Ok(String::new());
    }
    let mut out = vec!["This task needs work from another task, and it's ready:".to_string()];
    out.extend(lines);
    out.push(format!(
        "Bring it in with git, not by asking {}: git fetch origin, then rebase this task's branch so it sits on top \
         of that work. When it isn't merged yet, rebase onto its branch (git rebase origin/<its branch>), or with a \
         base of your own already, replay just your commits on top: git rebase --onto origin/<its branch> <your base> \
         <your branch>. Rebase, never merge. Say in the PR description that it needs that PR first.",
        app.cfg.owner
    ));
    Ok(out.join("\n"))
}

pub fn park(app: &App, t: &Row, wanted: &str, who: &str, why: &str) -> Result<()> {
    let mut ctx = board::task_context(t);
    ctx.insert("parked".into(), json!({"at": now_iso(), "session": t.v("session_id"), "why": why}));
    app.db.x(
        "UPDATE jobs SET state = 'expired', result = ?, updated_at = ? WHERE task_id = ? AND kind = 'agent' \
         AND purpose = 'start' AND state IN ('pending', 'running')",
        p![jdumps(&json!({"cancelled": "waits"})), now_iso(), t.id()],
    )?;
    board::save_context(app, t.id(), &ctx, false)?;
    let names = jloads_arr(Some(wanted)).iter().filter_map(|v| v.as_i64()).map(|n| rf("task", n)).collect::<Vec<_>>().join(", ");
    let pickup = if matches!(t.s("pickup"), Some("manual") | Some("attach")) { "new".to_string() } else { t.st("pickup") };
    board::update_task(
        app,
        t.id(),
        fields!["waits_for" => wanted, "status" => "queued", "needs_reason" => null, "question" => null,
                "session_id" => null, "start_job" => null, "lost" => 0, "pickup_session" => null, "pickup" => pickup,
                "latest" => format!("Waiting for {names}; it starts again by itself.")],
    )?;
    board::log_event(
        app,
        t.id(),
        who,
        "status",
        &format!(
            "Waits for {names}{}. Back in the queue; it carries on in this conversation once that's done",
            if why.is_empty() { String::new() } else { format!(": {why}") }
        ),
    )?;
    Ok(())
}

pub fn parked(t: &Row) -> Option<Row> {
    board::task_context(t).get("parked").and_then(|v| v.as_object()).cloned()
}

pub fn resume_prompt(app: &App, t: &Row) -> Result<String> {
    let tb = board::tb_cmd(app);
    let r = rf("task", t.id());
    let text = bring_in_text(app, t)?;
    Ok([
        format!("[task-board:{r}] What this task was waiting for is ready, so it carries on here."),
        text,
        format!("Then carry on with the task from where you stopped, and finish with {tb} done as usual."),
    ]
    .into_iter()
    .filter(|x| !x.is_empty())
    .collect::<Vec<_>>()
    .join("\n"))
}

pub fn started(app: &App, t: &Row) -> Result<()> {
    let mut ctx = board::task_context(t);
    if ctx.remove("parked").is_some() {
        board::save_context(app, t.id(), &ctx, false)?;
    }
    Ok(())
}

/// Parked tasks whose old terminal is still open and idle, to close.
pub fn to_close(app: &App) -> Result<Vec<(Row, Row, String)>> {
    let mut out = vec![];
    for t in app.db.q(
        "SELECT * FROM tasks WHERE status = 'queued' AND session_id IS NULL AND line_session IS NULL \
         AND json_extract(context, '$.parked.session') IS NOT NULL",
        p![],
    )? {
        let Some(p) = parked(&t) else { continue };
        let Some(s) = board::get_session(app, p.s("session"))? else { continue };
        if s.s("status") == Some("gone") || crate::lines::busy(app, &s.st("id"))? {
            continue;
        }
        let at = p.st("at");
        out.push((t, s, at));
    }
    Ok(out)
}

/// Tells a live task (or wakes a done task's PR) when work it builds on moves or merges.
pub fn follow_ups(app: &App) -> Result<i64> {
    let mut sent = 0;
    for t in app.db.q("SELECT * FROM tasks WHERE waits_for IS NOT NULL OR pr_after IS NOT NULL", p![])? {
        let live = matches!(t.s("status"), Some("working") | Some("needs")) && has(t.s("session_id"));
        let pr_open = t.s("status") == Some("done") && board::pr_still_open(&t);
        if !(live || pr_open) {
            continue;
        }
        let flow = jloads_obj(t.s("pr_flow"));
        let seen = flow.get(FOLLOW).and_then(|v| v.as_object()).cloned().unwrap_or_default();
        let mut changes = Row::new();
        let mut notes = vec![];
        for n in deps(&t) {
            let other = board::find_task(app, Some(n))?;
            if !ready(other.as_ref()) {
                continue;
            }
            let other = other.unwrap();
            let now = json!({"head": head_of(&other), "merged": other.st("pr_state").eq_ignore_ascii_case("MERGED")});
            let key = n.to_string();
            let Some(before) = seen.get(&key) else {
                changes.insert(key, now);
                continue;
            };
            if *before == now || (now["head"].is_null() && now["merged"] == false) {
                continue;
            }
            notes.push(follow_line(app, &other, before, &now)?);
            changes.insert(key, now);
        }
        if changes.is_empty() {
            continue;
        }
        if !notes.is_empty() {
            let mut lines = vec![format!("[task-board:{}] Work this task builds on has moved:", rf("task", t.id()))];
            lines.extend(notes.iter().cloned());
            lines.push(format!(
                "Rebase, run the tests, and push with git push --force-with-lease if the branch was pushed. Don't ask {}; this is routine.",
                app.cfg.owner
            ));
            let text = lines.join("\n");
            if live {
                deliver::add(app, "follow", &text, t.id(), None, None)?;
            } else if prflow::waking(app, &t)? || !hours::goal_open(app, board::find_goal(app, t.i("goal_id"))?.as_ref()) {
                continue;
            } else {
                prflow::follow_wake(app, &t, &text)?;
            }
            board::log_event(
                app,
                t.id(),
                board::BOARD,
                "handoff",
                &format!("Told it to rebase: {}", notes.iter().map(|x| short(x, 120)).collect::<Vec<_>>().join("; ")),
            )?;
            sent += 1;
        }
        let mut merged = seen.clone();
        for (k, v) in changes {
            merged.insert(k, v);
        }
        prflow::merge_flow(app, t.id(), vec![(FOLLOW, Value::Object(merged))])?;
    }
    Ok(sent)
}

fn follow_line(app: &App, other: &Row, before: &Value, now: &Value) -> Result<String> {
    let branch = branch_of(other).unwrap_or_else(|| "its branch".into());
    let old: String = before["head"].as_str().unwrap_or("").chars().take(12).collect();
    let old = if old.is_empty() { "<its old last commit>".to_string() } else { old };
    let lbl = label(app, other)?;
    if now["merged"] == true && before["merged"] != true {
        return Ok(format!(
            "- {lbl} was merged. Drop its commits from your branch and keep only yours: git fetch origin && \
             git rebase --onto origin/<your base, usually main> {old} <your branch>."
        ));
    }
    let new: String = now["head"].as_str().unwrap_or("").chars().take(12).collect();
    Ok(format!(
        "- {lbl}'s branch {branch} moved from {old} to {new}. Replay your commits on the new tip: \
         git fetch origin && git rebase --onto origin/{branch} {old} <your branch>."
    ))
}
