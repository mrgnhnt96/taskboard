//! The runner: starts queued tasks, closes finished terminals, delivers messages, reads the spool.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::app::App;
use crate::util::*;
use crate::{board, deliver, dispatch, fields, gitattrs, handoff, hooks, hours, jira, jobs, limits, locks, midna, p, prflow, projects, reports, usage, waitsfor, worktrees};

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

fn start_parked(app: &App, t: &Row) -> Result<()> {
    let ctx = board::task_context(t);
    let wt = ctx.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str()).filter(|w| Path::new(w).is_dir()).map(|s| s.to_string());
    let cid = t.s("claude_session_id").filter(|c| !c.is_empty());
    let too_big = match cid {
        Some(cid) => {
            let cwd = match &wt {
                Some(w) => Some(w.clone()),
                None => task_cwd(app, t)?,
            };
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
            (matches!(x.s("status"), Some("working") | Some("needs")) && x.s("needs_reason") != Some("start_failed"))
                || (x.s("status") == Some("queued") && x.i("start_job").is_some())
        })
        .count() as i64;
    if active >= max {
        return Ok(Some(format!("Waits for a free slot: its goal runs {} at a time", plural(max, "terminal"))));
    }
    Ok(None)
}

/// `goal_blocker` without the terminal limit: paused, or an earlier wave or task not done.
pub fn goal_order_blocker(app: &App, t: &Row, g: &Row) -> Result<Option<String>> {
    if g.b("deprioritized") {
        return Ok(Some("Its goal is deprioritized".into()));
    }
    if g.b("paused") {
        return Ok(Some("Its goal is paused".into()));
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

pub fn start_queued(app: &App) -> Result<Vec<i64>> {
    let mut started = vec![];
    let in_hours = hours::may_start(app);
    let queued = app.db.q(
        "SELECT * FROM tasks WHERE status = 'queued' AND session_id IS NULL AND line_session IS NULL ORDER BY priority = 'high' DESC, created_at, id",
        p![],
    )?;
    for t in queued {
        if t.s("pickup") == Some("manual") {
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
        json!({"session": s.v("id"), "reason": format!("Task board: {}", why.to_lowercase())}),
        Some(t.id()),
        purpose,
        None,
    )
}

pub fn close_for_usage(app: &App) -> Result<()> {
    let Some(out) = usage::out_until(app) else { return Ok(()) };
    let mut made = 0;
    for (s, t) in task_terminals(app)? {
        if s.s("status") != Some("idle") {
            continue;
        }
        app.db.tx(|| close_for(app, &s, &t, board::USAGE_CLOSE, "The 5-hour usage ran out"))?;
        made += 1;
    }
    if made > 0 && app.db.get_setting("usage_closed_for")?.as_deref() != Some(out.as_str()) {
        app.db.set_setting("usage_closed_for", Some(&out))?;
        app.info(format!("usage: 5-hour window ran out; closing {made} terminals until {out}"));
        dispatch::notify(
            app,
            &format!(
                "Out of 5-hour usage. Closed {}; their tasks start again from their handoffs at {}.",
                plural(made, "task terminal"),
                hours::clock(&local_hhmm(Some(&out)))
            ),
            &app.cfg.page_url,
            None,
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
        if s.s("status") != Some("idle") || board::offline(&s) || board::waiting_on_background(&s) || age_secs(s.s("status_at")).unwrap_or(0.0) < IDLE_CLOSE_SECS {
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
    jira::run_pending(app)?;
    crate::qa::tick(app)?;
    app.db.tx(|| crate::lines::tick(app))?;
    auto_close_done(app)?;
    app.db.tx(|| offline_too_long(app))?;
    app.db.tx(|| crate::devices::release_idle(app).map(|_| ()))?;
    close_for_usage(app)?;
    close_idle_after_hours(app)?;
    close_pr_tabs(app)?;
    // Outside a transaction: a restart command may take a while.
    crate::feed::check(app)?;
    crate::prbuilds::tick(app)?;
    crate::breaks::tick(app)?;
    let made = start_queued(app)?;
    app.db.tx(|| {
        deliver::tick(app)?;
        dispatch::tick(app)
    })?;
    Ok(made)
}

pub fn prs(app: &App) -> Result<()> {
    // With a healthy PR feed, events refresh each PR as it changes (`feed.rs`).
    if crate::feed::poll_due(app) {
        prflow::refresh(app)?;
    }
    app.db.tx(|| crate::prbuilds::sweep(app))?;
    app.db.tx(|| waitsfor::follow_ups(app).map(|_| ()))?;
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
        }
        if due(Some(last_spool), iv.spool) {
            last_spool = Instant::now();
            safe("spool", reports::ingest_spool(&app).map(|_| ()));
        }
        if due(last_prs, iv.prs) {
            last_prs = Some(Instant::now());
            safe("prs", prs(&app));
        }
        if due(last_reviews, iv.reviews) {
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
