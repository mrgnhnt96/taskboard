//! The runner: starts queued tasks, closes finished terminals, delivers messages, reads the spool.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::app::App;
use crate::util::*;
use crate::{board, deliver, dispatch, fields, gitattrs, handoff, hooks, hours, human, jira, jobs, limits, locks, midna, p, prflow, projects, reports, transcript, usage, waitsfor, worktrees};

pub fn task_cwd(app: &App, t: &Row) -> Result<Option<String>> {
    if let Some(r) = t.s("repo_path").filter(|r| !r.is_empty()) {
        return Ok(Some(r.to_string()));
    }
    projects::project_path(app, t.s("project"))
}

pub fn offer_text(app: &App, t: &Row) -> String {
    let tb = board::tb_cmd(app);
    let r = rf("task", t.id());
    format!("[task-board:{r}] New task for you: {}. Run {tb} take {r} when you're free.", t.st("title"))
}

pub fn start_task(
    app: &App,
    t: &Row,
    mode: &str,
    session_id: Option<&str>,
    prompt: Option<String>,
    flags: Option<String>,
    cwd: Option<String>,
) -> Result<i64> {
    human::refuse_terminal(app, t)?;
    if mode == "attach" && crate::jira_desk::is_desk(app, session_id)? {
        return err(409, "Tasks can't go to the Jira desk.");
    }
    let cwd = match cwd {
        Some(c) => Some(c),
        None if mode != "attach" => match worktrees::made(app, t)? {
            Some(w) => Some(w),
            None => task_cwd(app, t)?,
        },
        None => task_cwd(app, t)?,
    };
    let title = short(&t.st("title"), 40);
    // Lent before the handoff is built, so it names them.
    crate::devices::lend(app, t)?;
    let prompt = match prompt {
        Some(p) => p,
        None => handoff::build_starting(app, t.id())?,
    };
    let jid = if mode == "attach" {
        let s = board::get_session(app, session_id)?;
        let Some(s) = s.filter(|s| s.s("status") != Some("gone")) else {
            return err(409, "That Midna terminal isn't open any more.");
        };
        if s.s("status") == Some("idle") && !board::waiting_on_background(&s) && board::runs_claude(&s) {
            board::create_job(
                app,
                "agent",
                json!({"cwd": cwd.clone().or(s.s("project_path").map(|x| x.to_string())), "title": title,
                       "prompt": prompt, "queue": false, "session_id": s.v("id")}),
                Some(t.id()),
                "start",
                None,
            )?
        } else {
            if has(s.s("agent")) && !s.st("agent").to_lowercase().contains("claude") {
                return err(409, "That terminal isn't running Claude, so the board can't offer it a task.");
            }
            let jid = deliver::add(app, "offer", &offer_text(app, t), t.id(), s.s("id"), None)?;
            board::log_event(
                app,
                t.id(),
                board::BOARD,
                "midna",
                &format!("Offering it to {} with that terminal's next prompt or turn", board::session_name(app, s.s("id"), None)),
            )?;
            jid
        }
    } else {
        let Some(cwd) = cwd else {
            return err(409, format!("The board doesn't know the folder for the project “{}”.", t.st("project")));
        };
        let mut args = json!({"cwd": cwd, "title": title, "prompt": prompt, "queue": mode == "queue"});
        let m = &app.cfg.models;
        let flags = with_effort(with_model(flags, m.resolve(t.s("model")).as_deref()), m.resolve_effort(t.s("model"), t.s("effort")).as_deref());
        if let Some(f) = flags {
            args["flags"] = json!(f);
        }
        board::create_job(app, "agent", args, Some(t.id()), "start", None)?
    };
    let mut f = fields!["start_job" => jid, "latest" => "Starting in Midna…"];
    if matches!(t.s("status"), Some("planned") | Some("needs")) {
        f.extend(fields!["status" => "queued", "needs_reason" => null, "question" => null]);
    }
    board::update_task(app, t.id(), f)?;
    app.info(format!("runner: {} start job {} ({mode})", rf("task", t.id()), rf("job", jid)));
    Ok(jid)
}

/// A task's `model` from a body: `small`, `large` or a model name; null, empty or `default` clears it.
pub fn clean_model(v: Option<&serde_json::Value>) -> Result<Option<String>> {
    let m = match v {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(serde_json::Value::String(s)) => s.trim().to_string(),
        Some(_) => return err(400, "The model is small, large or a model name."),
    };
    if m.is_empty() || m == "default" {
        return Ok(None);
    }
    if m.len() > 80 || !m.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '[' | ']')) {
        return err(400, format!("“{m}” isn't a model: use small, large or a model name like sonnet or claude-opus-5-5."));
    }
    Ok(Some(m))
}

/// A task's `effort` from a body: low, medium, high, xhigh or max; null, empty or `default` clears it.
pub fn clean_effort(v: Option<&serde_json::Value>) -> Result<Option<String>> {
    let e = match v {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(serde_json::Value::String(s)) => s.trim().to_lowercase(),
        Some(_) => return err(400, "The effort is low, medium, high, xhigh or max."),
    };
    if e.is_empty() || e == "default" {
        return Ok(None);
    }
    if !EFFORTS.contains(&e.as_str()) {
        return err(400, format!("“{e}” isn't an effort: use low, medium, high, xhigh or max."));
    }
    Ok(Some(e))
}

pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// The terminal's Claude flags with `--model` added, unless they already name one.
pub(crate) fn with_model(flags: Option<String>, model: Option<&str>) -> Option<String> {
    with_flag(flags, "--model", model)
}

/// The terminal's Claude flags with `--effort` added, unless they already name one.
pub(crate) fn with_effort(flags: Option<String>, effort: Option<&str>) -> Option<String> {
    with_flag(flags, "--effort", effort)
}

fn with_flag(flags: Option<String>, flag: &str, value: Option<&str>) -> Option<String> {
    let Some(v) = value else { return flags };
    let eq = format!("{flag}=");
    if flags.as_deref().is_some_and(|f| f.split_whitespace().any(|w| w == flag || w.starts_with(&eq))) {
        return flags;
    }
    let v = shlex::try_quote(v).map(|q| q.into_owned()).unwrap_or_else(|_| v.to_string());
    Some(match flags.filter(|f| !f.trim().is_empty()) {
        Some(f) => format!("{f} {flag} {v}"),
        None => format!("{flag} {v}"),
    })
}

fn start_parked(app: &App, t: &Row) -> Result<()> {
    let ctx = board::task_context(t);
    let wt = ctx.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str()).filter(|w| Path::new(w).is_dir()).map(|s| s.to_string());
    let cwd = match &wt {
        Some(w) => Some(w.clone()),
        None => task_cwd(app, t)?,
    };
    let cid = t.s("claude_session_id").filter(|c| !c.is_empty() && transcript::conversation_saved(&app.cfg.claude_projects, cwd.as_deref().unwrap_or(""), c));
    let too_big = match cid {
        Some(cid) => {
            let last = board::get_session(app, t.s("session_id"))?;
            let c = limits::conversation(app, cwd.as_deref().unwrap_or(""), cid, last.as_ref().and_then(|s| s.s("last_activity")));
            limits::fresh_start_why(app, &c)
        }
        None => None,
    };
    if let (Some(cid), None) = (cid, &too_big) {
        start_task(app, t, "new", None, Some(waitsfor::resume_prompt(app, t)?), Some(format!("--resume {cid}")), wt)?;
        board::log_event(app, t.id(), board::BOARD, "handoff", "What it waited for is done, so its conversation carries on in a new terminal")?;
    } else if let Some(why) = too_big {
        start_task(app, t, "new", None, None, None, wt)?;
        board::log_event(app, t.id(), board::BOARD, "handoff", &format!("What it waited for is done. It starts again with the handoff: {why}"))?;
    } else {
        start_task(app, t, "new", None, None, None, wt)?;
        board::log_event(app, t.id(), board::BOARD, "handoff", "What it waited for is done, so it starts again with the handoff")?;
    }
    waitsfor::started(app, &board::get_task(app, t.id())?)
}

/// A `task.starting` hook skipped the task: it's done without running, and says why.
pub fn skip_task(app: &App, t: &Row, d: &hooks::Decision) -> Result<()> {
    let line = d.said("Skipped");
    hooks::note(app, t.id(), &line)?;
    reports::finish_task(app, &board::get_task(app, t.id())?, "Hook", &line, false, None)?;
    Ok(())
}

/// Why a goal holds this queued task back: paused, an earlier task not done, or too many terminals.
pub fn goal_blocker(app: &App, t: &Row, g: &Row) -> Result<Option<String>> {
    if let Some(why) = goal_order_blocker(app, t, g)? {
        return Ok(Some(why));
    }
    let tasks = board::goal_tasks(app, g.id())?;
    let max = g.i("max_terminals").unwrap_or(2).max(1);
    let active = tasks
        .iter()
        .filter(|x| x.id() != t.id())
        .filter(|x| {
            (matches!(x.s("status"), Some("working") | Some("needs")) && !matches!(x.s("needs_reason"), Some("start_failed") | Some(human::NEEDS)))
                || (x.s("status") == Some("queued") && x.i("start_job").is_some())
        })
        .count() as i64;
    if active >= max {
        return Ok(Some(format!("Waits for a free slot: its goal runs {} at a time", plural(max, "terminal"))));
    }
    Ok(None)
}

/// `goal_blocker` without the terminal limit: paused, a goal it runs after not finished, or an earlier
/// wave or task not done.
pub fn goal_order_blocker(app: &App, t: &Row, g: &Row) -> Result<Option<String>> {
    if g.b("deprioritized") {
        return Ok(Some("Its goal is deprioritized".into()));
    }
    if g.b("paused") {
        return Ok(Some("Its goal is paused".into()));
    }
    if let Some(why) = crate::chain::blocker(app, g)? {
        return Ok(Some(why));
    }
    let tasks = board::goal_tasks(app, g.id())?;
    // Waves take the place of running in order.
    let waved = crate::waves::uses_waves(&tasks);
    if waved {
        if let Some(why) = crate::waves::wave_block(app, t, g)? {
            return Ok(Some(why));
        }
    }
    if g.b("run_in_order") && !waved {
        for x in &tasks {
            if x.id() == t.id() {
                break;
            }
            if !matches!(x.s("status"), Some("done") | Some("planned")) {
                return Ok(Some(format!("Waits for {} to finish first", rf("task", x.id()))));
            }
        }
    }
    Ok(None)
}

/// Each goal's queued tasks in wave order, in the places its tasks hold in `queued`: a later wave's
/// task never takes a terminal, lock, device or worktree slot ahead of an earlier wave's.
fn in_wave_order(queued: Vec<Row>) -> Vec<Row> {
    let mut by_goal: std::collections::HashMap<i64, std::collections::VecDeque<Row>> = Default::default();
    let mut waved: Vec<&Row> = queued.iter().filter(|t| t.i("wave").is_some()).collect();
    waved.sort_by_key(|t| t.i("wave"));
    for t in waved {
        by_goal.entry(t.i0("goal_id")).or_default().push_back(t.clone());
    }
    queued
        .into_iter()
        .map(|t| match t.i("wave") {
            Some(_) => by_goal.get_mut(&t.i0("goal_id")).and_then(|q| q.pop_front()).unwrap_or(t),
            None => t,
        })
        .collect()
}

/// Why `t` doesn't claim a worktree slot yet: it would take a new one, and its start would only wait
/// on a terminal cap holding it. A slot is claimed only by a start that can open its terminal.
pub fn slot_hold(app: &App, t: &Row) -> Result<Option<String>> {
    if t.s("pickup") == Some("attach") || t.i("start_job").is_some() || worktrees::made(app, t)?.is_some() {
        return Ok(None);
    }
    let Some(repo) = worktrees::slot_repo(app, t)? else { return Ok(None) };
    let queue = waitsfor::parked(t).is_none() && t.s("pickup") != Some("new");
    dispatch::hold_before_start(app, t, &repo, queue)
}

/// Gives back the worktree slots that starts waiting on a terminal cap sit on: the start job goes, and
/// the task claims a slot again once its start can open (`slot_hold`). A slot with work in it, or one
/// a parked conversation resumes in, stays.
fn free_waiting_slots(app: &App) -> Result<()> {
    let held = app.db.q(
        "SELECT t.* FROM tasks t JOIN jobs j ON t.start_job = j.id JOIN worktree_slots s ON s.task_id = t.id \
         WHERE t.status = 'queued' AND j.state = 'pending' AND j.purpose = 'start'",
        p![],
    )?;
    for t in held {
        if waitsfor::parked(&t).is_some() {
            continue;
        }
        let Some(j) = board::live_start_job(app, &t)?.filter(|j| j.s("state") == Some("pending")) else { continue };
        let Some(why) = dispatch::start_hold(app, &j)? else { continue };
        if crate::slots::release_refusal(app, &t)?.is_some() {
            continue;
        }
        let dropped = app.db.tx(|| {
            if app.db.q1("SELECT id FROM jobs WHERE id = ? AND state = 'pending'", p![j.id()])?.is_none() {
                return Ok(false);
            }
            app.db.x("UPDATE jobs SET state = 'cancelled', updated_at = ? WHERE id = ?", p![now_iso(), j.id()])?;
            board::update_task(app, t.id(), fields!["start_job" => null, "latest" => null])?;
            board::log_event(app, t.id(), board::BOARD, "midna", &format!("{why}, so it gives back its worktree slot meanwhile"))?;
            Ok(true)
        })?;
        // Without its start job it starts in the slot it still holds, should this fail.
        if dropped {
            crate::slots::give_back(app, &t)?;
        }
    }
    Ok(())
}

pub fn start_queued(app: &App) -> Result<Vec<i64>> {
    let mut started = vec![];
    free_waiting_slots(app)?;
    let in_hours = hours::may_start(app);
    let queued = app.db.q(
        "SELECT * FROM tasks WHERE status = 'queued' AND session_id IS NULL AND line_session IS NULL ORDER BY priority = 'high' DESC, created_at, id",
        p![],
    )?;
    for t in in_wave_order(queued) {
        if crate::waitsfor::waits_for_start(app, &t)? {
            continue;
        }
        let g = board::find_goal(app, t.i("goal_id"))?;
        if !in_hours && !hours::goal_open(app, g.as_ref()) {
            continue;
        }
        if t.s("retry_at").map(|r| r > now_iso().as_str()).unwrap_or(false) {
            continue;
        }
        if board::live_start_job(app, &t)?.is_some() {
            continue;
        }
        if app.db.count(
            "SELECT COUNT(*) FROM jobs WHERE task_id = ? AND kind = 'agent' AND purpose = 'start' AND state IN ('pending','running')",
            p![t.id()],
        )? > 0
        {
            continue;
        }
        if let Some(g) = &g {
            if goal_blocker(app, &t, g)?.is_some() {
                continue;
            }
        }
        if jira::ticket_blocker(app, &t)? || waitsfor::blocker(app, &t)?.is_some() || locks::blocker(app, &t)?.is_some() {
            continue;
        }
        if human::is(&t) {
            app.db.tx(|| human::ready(app, &t))?;
            continue;
        }
        if crate::devices::blocker(app, &t)?.is_some() {
            continue;
        }
        let tid = t.id();
        match hooks::gate(app, "task.starting", &t, json!({})) {
            hooks::Decision::Go => {}
            d @ hooks::Decision::Block { .. } => {
                app.db.tx(|| jobs::start_failed(app, &t, &d.said("Stopped from starting"), false, false).map(|_| ()))?;
                continue;
            }
            d @ hooks::Decision::Skip { .. } => {
                app.db.tx(|| skip_task(app, &t, &d))?;
                continue;
            }
        }
        if slot_hold(app, &t)?.is_some() {
            continue;
        }
        if t.s("pickup") != Some("attach") {
            if let Err(e) = worktrees::ensure(app, &t) {
                app.db.tx(|| jobs::start_failed(app, &t, &format!("Couldn't start: {}", e.message), true, true).map(|_| ()))?;
                continue;
            }
        }
        let r = app.db.tx(|| {
            let t = board::get_task(app, tid)?;
            if waitsfor::parked(&t).is_some() {
                start_parked(app, &t)?;
            } else {
                match t.s("pickup") {
                    Some("queue") | Some("new") => {
                        start_task(app, &t, &t.st("pickup"), None, None, None, None)?;
                    }
                    // It waited for Start before it joined a goal that's running: the run starts it.
                    Some("manual") if !waitsfor::waits_for_start(app, &t)? => {
                        start_task(app, &t, "queue", None, None, None, None)?;
                    }
                    Some("attach") => {
                        let Some(sid) = t.s("pickup_session").filter(|s| !s.is_empty()) else { return Ok(false) };
                        start_task(app, &t, "attach", Some(sid), None, None, None)?;
                    }
                    _ => return Ok(false),
                }
            }
            Ok(true)
        });
        match r {
            Ok(true) => started.push(tid),
            Ok(false) => {}
            Err(e) if e.status < 500 => {
                app.db.tx(|| jobs::start_failed(app, &t, &format!("Couldn't start: {}", e.message), true, false).map(|_| ()))?;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(started)
}

const AUTO_CLOSE_SETTLE_SECS: f64 = 5.0;
const AUTO_CLOSE_RETRY_SECS: f64 = 20.0;
const AUTO_CLOSE_TRIES: usize = 4;
const IDLE_CLOSE_SECS: f64 = 60.0;

fn closes_since(app: &App, sid: &str, since: &str) -> Result<Vec<Row>> {
    Ok(app
        .db
        .q("SELECT args, state, updated_at FROM jobs WHERE kind = 'close' AND created_at >= ?", p![since])?
        .into_iter()
        .filter(|j| board::job_args(j).s("session") == Some(sid))
        .collect())
}

fn worth_retrying(tried: &[Row]) -> bool {
    let Some(last) = tried.last() else { return true };
    matches!(last.s("state"), Some("failed") | Some("expired"))
        && tried.len() < AUTO_CLOSE_TRIES
        && age_secs(last.s("updated_at")).unwrap_or(0.0) >= AUTO_CLOSE_RETRY_SECS
}

fn settled(s: &Row) -> bool {
    s.s("status") == Some("idle") && !board::offline(s) && !board::waiting_on_background(s) && age_secs(s.s("status_at")).unwrap_or(0.0) >= AUTO_CLOSE_SETTLE_SECS
}

pub fn auto_close_done(app: &App) -> Result<()> {
    for t in app.db.q(
        "SELECT * FROM tasks WHERE status = 'done' AND failed = 0 AND auto_close = 1 AND session_id IS NOT NULL",
        p![],
    )? {
        let Some(s) = board::get_session(app, t.s("session_id"))? else { continue };
        if !settled(&s) || crate::lines::busy(app, &s.st("id"))? || !board::opened_by_board(app, s.s("id"))? {
            continue;
        }
        let tried = closes_since(app, &s.st("id"), t.s("finished_at").unwrap_or(""))?;
        if !tried.is_empty() && !worth_retrying(&tried) {
            continue;
        }
        app.db.tx(|| board::create_job(app, "close", json!({"session": s.v("id")}), Some(t.id()), "auto_close", None))?;
    }
    for (t, s, since) in waitsfor::to_close(app)? {
        if !settled(&s) {
            continue;
        }
        let tried = closes_since(app, &s.st("id"), &since)?;
        if !tried.is_empty() && !worth_retrying(&tried) {
            continue;
        }
        app.db.tx(|| board::create_job(app, "close", json!({"session": s.v("id")}), Some(t.id()), "auto_close", None))?;
    }
    Ok(())
}

fn task_terminals(app: &App) -> Result<Vec<(Row, Row)>> {
    let mut out = vec![];
    for s in app.db.q("SELECT * FROM sessions WHERE status IS NOT 'gone'", p![])? {
        let t = match board::task_for_session(app, s.s("id"))? {
            Some(t) => Some(t),
            None => prflow::visited_by(app, &s.st("id"))?,
        };
        if let Some(t) = t {
            if board::close_rule(Some(&s)).is_some() && !board::closing_session(app, &s.st("id"))? {
                out.push((s, t));
            }
        }
    }
    Ok(out)
}

fn close_for(app: &App, s: &Row, t: &Row, purpose: &str, why: &str) -> Result<i64> {
    close_job(app, s, t, purpose, why, false)
}

fn close_job(app: &App, s: &Row, t: &Row, purpose: &str, why: &str, force: bool) -> Result<i64> {
    board::log_event(
        app,
        t.id(),
        board::BOARD,
        "midna",
        &format!("{why}, so the board is closing {}", board::session_name(app, s.s("id"), None)),
    )?;
    if t.s("status") == Some("done") {
        prflow::merge_flow(app, t.id(), vec![("woke", serde_json::Value::Null)])?;
    }
    board::create_job(
        app,
        "close",
        json!({"session": s.v("id"), "reason": format!("Task board: {}", why.to_lowercase()), "force": force}),
        Some(t.id()),
        purpose,
        None,
    )
}

/// A task terminal that has compacted `max_compactions` times carries on in a fresh one from its
/// handoff (the checkpoint saved before each compaction) instead of compacting again: closed once
/// its turn is over, or at once when it starts one more compaction (nothing runs while it compacts).
/// Only terminals the board opened: one the owner opened that took a task (`--here`, `tb take`)
/// holds their conversation, so it stays open.
pub fn close_compacted(app: &App) -> Result<()> {
    let max = limits::get(app).max_compactions;
    if max <= 0 {
        return Ok(());
    }
    for (s, t) in task_terminals(app)? {
        let n = s.i0("compactions");
        if n < max || t.s("status") != Some("working") || t.b("lost") || t.s("session_id") != s.s("id") {
            continue;
        }
        let compacting = board::compacting_since(&s).is_string() && n > max && !board::waiting_on_background(&s);
        if !compacting && (!settled(&s) || board::limited(&s)) {
            continue;
        }
        if !board::opened_by_board(app, s.s("id"))? {
            continue;
        }
        let why = format!("It has compacted its conversation {}", plural(n, "time"));
        app.db.tx(|| close_job(app, &s, &t, board::COMPACT_CLOSE, &why, compacting))?;
    }
    Ok(())
}

pub fn close_for_usage(app: &App) -> Result<()> {
    let Some(out) = usage::out_until(app) else { return Ok(()) };
    let mut made = 0;
    for (s, t) in task_terminals(app)? {
        if s.s("status") != Some("idle") || board::limited(&s) {
            continue;
        }
        app.db.tx(|| close_for(app, &s, &t, board::USAGE_CLOSE, "The usage ran out"))?;
        made += 1;
    }
    if made > 0 && app.db.get_setting("usage_closed_for")?.as_deref() != Some(out.as_str()) {
        app.db.set_setting("usage_closed_for", Some(&out))?;
        app.info(format!("usage: ran out; closing {made} terminals until {out}"));
        dispatch::notify(
            app,
            &format!(
                "Out of usage. Closed {}; their tasks start again from their handoffs {}.",
                plural(made, "task terminal"),
                hours::at_time(&hours::say_reset(&out))
            ),
            &app.cfg.page_url,
        );
    }
    Ok(())
}

/// Midna stops waiting for the network after six hours. A task still on an offline terminal by then
/// needs the owner.
pub fn offline_too_long(app: &App) -> Result<()> {
    for s in app.db.q("SELECT * FROM sessions WHERE api_error_kind = 'network' AND api_error IS NOT NULL AND status != 'gone'", p![])? {
        if age_secs(s.s("api_error_at")).unwrap_or(0.0) < midna::RESUME_GIVES_UP_SECS {
            continue;
        }
        let Some(t) = board::task_for_session(app, s.s("id"))? else { continue };
        if t.s("status") != Some("working") || t.b("lost") {
            continue;
        }
        let ask = "It lost its network connection over 6 hours ago and Midna has stopped waiting; type anything in the terminal to carry on";
        board::update_task(app, t.id(), fields!["status" => "needs", "needs_reason" => "offline", "question" => ask, "answered_at" => null])?;
        board::log_event(app, t.id(), board::MIDNA, "question", ask)?;
    }
    Ok(())
}

pub fn close_idle_after_hours(app: &App) -> Result<()> {
    if hours::is_open(app) {
        return Ok(());
    }
    for (s, t) in task_terminals(app)? {
        if s.s("status") != Some("idle") || board::offline(&s) || board::limited(&s) || board::waiting_on_background(&s) || age_secs(s.s("status_at")).unwrap_or(0.0) < IDLE_CLOSE_SECS {
            continue;
        }
        if hours::goal_open(app, board::find_goal(app, t.i("goal_id"))?.as_ref()) {
            continue;
        }
        app.db.tx(|| close_for(app, &s, &t, board::HOURS_CLOSE, "It's idle after work hours"))?;
    }
    Ok(())
}

pub const PR_TAB_CLOSE: &str = "pr_tab_close";

fn close_worth_trying(app: &App, sid: &str) -> Result<bool> {
    let failed: Vec<Row> = app
        .db
        .q(
            "SELECT state, updated_at, args FROM jobs WHERE kind = 'close' AND purpose = ? AND state IN ('failed', 'expired') ORDER BY id",
            p![PR_TAB_CLOSE],
        )?
        .into_iter()
        .filter(|j| board::job_args(j).s("session") == Some(sid))
        .collect();
    Ok(failed.len() < AUTO_CLOSE_TRIES
        && failed.last().map(|f| age_secs(f.s("updated_at")).unwrap_or(0.0) >= AUTO_CLOSE_RETRY_SECS).unwrap_or(true))
}

pub fn close_pr_tabs(app: &App) -> Result<()> {
    for (s, t, finished) in prflow::pr_tabs_to_close(app)? {
        if !settled(&s) || board::closing_session(app, &s.st("id"))? || !close_worth_trying(app, &s.st("id"))? {
            continue;
        }
        let why = if finished {
            format!("PR #{} was {}", t.i0("pr_num"), t.st("pr_phase"))
        } else {
            format!("It finished its turn on PR #{}", t.i0("pr_num"))
        };
        app.db.tx(|| {
            board::log_event(app, t.id(), board::BOARD, "midna", &format!("{why}, so the board is closing {}", board::session_name(app, s.s("id"), None)))?;
            board::create_job(
                app,
                "close",
                json!({"session": s.v("id"), "reason": format!("Task board: {}", why.to_lowercase())}),
                Some(t.id()),
                PR_TAB_CLOSE,
                None,
            )
        })?;
    }
    Ok(())
}

pub fn tick(app: &App) -> Result<Vec<i64>> {
    app.db.tx(|| jobs::expire(app))?;
    app.db.tx(|| jira::expire(app))?;
    app.db.tx(|| jira::ensure_tickets(app))?;
    app.db.tx(|| crate::merged_note::tick(app))?;
    jira::run_pending(app)?;
    crate::qa::tick(app)?;
    app.db.tx(|| crate::lines::tick(app))?;
    auto_close_done(app)?;
    app.db.tx(|| offline_too_long(app))?;
    app.db.tx(|| crate::devices::release_idle(app).map(|_| ()))?;
    close_for_usage(app)?;
    close_idle_after_hours(app)?;
    close_compacted(app)?;
    close_pr_tabs(app)?;
    // Outside a transaction: a restart command may take a while.
    crate::feed::check(app)?;
    crate::prhost::limit::check(app)?;
    crate::prbuilds::tick(app)?;
    crate::breaks::tick(app)?;
    crate::relevance::tick(app)?;
    crate::tidy::tick(app)?;
    dispatch::start_answers(app)?;
    let made = start_queued(app)?;
    app.db.tx(|| {
        deliver::tick(app)?;
        dispatch::tick(app)
    })?;
    Ok(made)
}

pub fn prs(app: &App) -> Result<()> {
    // With a healthy PR feed, events refresh each PR as it changes (`feed.rs`); with stream relays,
    // only the PRs a healthy one covers.
    if crate::feed::poll_due(app) {
        prflow::poll(app)?;
    }
    prflow::late_reads(app)?;
    prflow::waits_over(app)?;
    prflow::prompts_missed(app)?;
    app.db.tx(|| crate::prbuilds::sweep(app))?;
    app.db.tx(|| waitsfor::follow_ups(app).map(|_| ()))?;
    crate::setup::auto_sync(app)?;
    app.db.tx(|| crate::setup::sweep(app))?;
    crate::lost_done::sweep(app)?;
    crate::release_tags::sweep(app)?;
    crate::slot_ageing::sweep(app)?;
    worktrees::clean_up(app).map(|_| ())
}

/// The review sweep (`asks::sweep`), on its own timer (`[intervals] reviews`): it runs whether or not
/// the PRs are polled, so swaps, stand-ins, the board's own asks and bot runs keep going while a
/// healthy feed drives the PRs. The sweep holds what the feed's health says to hold itself.
pub fn reviews(app: &App) -> Result<()> {
    if !app.cfg.pr.watch {
        return Ok(());
    }
    crate::asks::sweep(app)
}

pub fn run(app: Arc<App>) {
    let iv = app.cfg.intervals.clone();
    let safe = |name: &str, r: Result<()>| {
        if let Err(e) = r {
            app.info(format!("runner step {name} failed: {e}"));
        }
    };
    safe("spool", reports::ingest_spool(&app).map(|_| ()));
    let (mut last_tick, mut last_spool, mut last_prs) = (None::<Instant>, Instant::now(), None::<Instant>);
    let mut last_attrs = None::<Instant>;
    let mut last_reviews = None::<Instant>;
    while !app.stopping() {
        let due = |last: Option<Instant>, every: f64| every > 0.0 && last.map(|l| l.elapsed().as_secs_f64() >= every).unwrap_or(true);
        if due(last_tick, iv.runner) {
            last_tick = Some(Instant::now());
            safe("tick", tick(&app).map(|_| ()));
            safe("pending cuts", crate::pending_cut::sweep(&app));
        }
        if due(Some(last_spool), iv.spool) {
            last_spool = Instant::now();
            safe("spool", reports::ingest_spool(&app).map(|_| ()));
        }
        // Wider while the board's calls would run past a host's `max_per_hour`.
        let stretch = crate::prhost::limit::stretch();
        if due(last_prs, iv.prs * stretch) {
            last_prs = Some(Instant::now());
            safe("prs", prs(&app));
        }
        if due(last_reviews, iv.reviews * stretch) {
            last_reviews = Some(Instant::now());
            safe("reviews", reviews(&app));
        }
        if due(last_attrs, gitattrs::EVERY_SECS) {
            last_attrs = Some(Instant::now());
            safe("attributes", gitattrs::sync(&app).map(|_| ()));
        }
        if app.wait_runner(Duration::from_secs(1)) {
            last_tick = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_model_adds_one_unless_the_flags_name_it() {
        assert_eq!(with_model(None, Some("sonnet")).as_deref(), Some("--model sonnet"));
        assert_eq!(with_model(Some("--resume c1".into()), Some("opus")).as_deref(), Some("--resume c1 --model opus"));
        assert_eq!(with_model(Some("--model=haiku".into()), Some("opus")).as_deref(), Some("--model=haiku"));
        assert_eq!(with_model(Some("--x".into()), None).as_deref(), Some("--x"));
        assert_eq!(with_model(None, None), None);
        assert_eq!(with_effort(Some("--model opus".into()), Some("high")).as_deref(), Some("--model opus --effort high"));
        assert_eq!(with_effort(Some("--effort=low".into()), Some("high")).as_deref(), Some("--effort=low"));
    }

    #[test]
    fn clean_effort_takes_levels_and_clears_on_default() {
        assert_eq!(clean_effort(Some(&json!("XHigh"))).unwrap().as_deref(), Some("xhigh"));
        assert_eq!(clean_effort(Some(&json!("default"))).unwrap(), None);
        assert_eq!(clean_effort(None).unwrap(), None);
        assert!(clean_effort(Some(&json!("huge"))).is_err());
        assert!(clean_effort(Some(&json!(3))).is_err());
    }

    #[test]
    fn clean_model_takes_names_and_clears_on_default() {
        assert_eq!(clean_model(Some(&json!("small"))).unwrap().as_deref(), Some("small"));
        assert_eq!(clean_model(Some(&json!("claude-sonnet-5-5"))).unwrap().as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(clean_model(Some(&json!("default"))).unwrap(), None);
        assert_eq!(clean_model(Some(&json!(""))).unwrap(), None);
        assert_eq!(clean_model(None).unwrap(), None);
        assert!(clean_model(Some(&json!("a b"))).is_err());
        assert!(clean_model(Some(&json!(3))).is_err());
    }
}
