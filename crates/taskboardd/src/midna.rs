//! Midna, the terminal app: the board calls its CLI (`midna call <method> '<json>'`).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, dispatch, jobs, keep_awake, limits, p, proc};

const DOWN_EXIT: i32 = 3;
const OPEN_EVERY: Duration = Duration::from_secs(30);
const CALL_TIMEOUT: f64 = 30.0;
const TAB_PREFIX: &str = "TB ";
static BOARD_TAB: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(TB|T\d+) ").unwrap());
const CLAUDE_EXITED: &str = "claude exited";

#[derive(Debug)]
pub enum MidnaError {
    /// Midna said no (or didn't answer in time).
    Refused(String),
    /// Midna isn't running; the job waits.
    Down(String),
}

impl std::fmt::Display for MidnaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MidnaError::Refused(s) | MidnaError::Down(s) => f.write_str(s),
        }
    }
}

type MResult<T> = std::result::Result<T, MidnaError>;

impl From<ApiError> for MidnaError {
    fn from(e: ApiError) -> Self {
        MidnaError::Refused(e.message)
    }
}

pub fn tab_title(title: &str, task_id: Option<i64>) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = if let Some(rest) = title.strip_prefix(TAB_PREFIX) {
        rest.to_string()
    } else if BOARD_TAB.is_match(&title) {
        return title;
    } else {
        title
    };
    match task_id {
        Some(t) => format!("{} {title}", rf("task", t)),
        None => format!("{TAB_PREFIX}{title}"),
    }
}

pub fn board_tab(name: &str) -> bool {
    BOARD_TAB.is_match(name)
}

fn why(stderr: &str) -> String {
    let first = stderr.lines().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("");
    first.strip_prefix("midna: ").unwrap_or(first).to_string()
}

fn seen(app: &App) {
    let mut s = app.shared.lock();
    s.midna_seen_at = Some(now_iso());
    s.midna_down = false;
}

pub fn up(app: &App) -> bool {
    let seen = app.shared.lock().midna_seen_at.clone();
    age_secs(seen.as_deref()).map(|a| a <= app.cfg.intervals.midna_up).unwrap_or(false)
}

pub fn status(app: &App) -> Value {
    json!({"up": up(app), "seen_at": app.shared.lock().midna_seen_at})
}

fn open_app(app: &App) {
    if !app.cfg.open_midna {
        return;
    }
    {
        let mut s = app.shared.lock();
        if s.midna_opened.map(|t| t.elapsed() < OPEN_EVERY).unwrap_or(false) {
            return;
        }
        s.midna_opened = Some(Instant::now());
    }
    if std::process::Command::new("open").args(["-g", "-b", &app.cfg.midna_bundle]).spawn().is_ok() {
        app.info("midna: it wasn't running; opened it");
    }
}

/// Calls Midna's CLI directly (for notifications, which don't touch the board's state).
pub fn call_exe(exe: &Path, method: &str, params: &Value, timeout: f64) -> MResult<Value> {
    let args = vec!["call".to_string(), method.to_string(), params.to_string()];
    match proc::run(exe, &args, None, timeout) {
        Err(proc::RunError::Spawn(e)) => Err(MidnaError::Down(format!("Couldn't run Midna's CLI at {}: {e}", exe.display()))),
        Err(proc::RunError::TimedOut) => Err(MidnaError::Refused(format!("Midna didn't answer {method} within {} seconds", timeout as i64))),
        Ok(o) if o.code == Some(DOWN_EXIT) => Err(MidnaError::Down("Midna isn't running".into())),
        Ok(o) if o.code != Some(0) => {
            let w = why(&o.stderr);
            Err(MidnaError::Refused(if w.is_empty() { format!("midna exited {}", o.code.unwrap_or(-1)) } else { w }))
        }
        Ok(o) => {
            let t = o.stdout.trim();
            Ok(if t.is_empty() { Value::Null } else { serde_json::from_str(t).unwrap_or_else(|_| json!(t)) })
        }
    }
}

pub fn call(app: &App, method: &str, params: Value) -> MResult<Value> {
    call_timeout(app, method, params, CALL_TIMEOUT)
}

pub fn call_timeout(app: &App, method: &str, params: Value, timeout: f64) -> MResult<Value> {
    let r = call_exe(&app.cfg.midna, method, &params, timeout);
    match &r {
        Err(MidnaError::Down(m)) => {
            app.shared.lock().midna_down = true;
            if m == "Midna isn't running" {
                open_app(app);
            }
        }
        Err(MidnaError::Refused(m)) if m.contains("didn't answer") => {}
        _ => seen(app),
    }
    r
}

fn set_target(app: &App, jid: i64, k: &str, v: Value) -> Result<()> {
    app.db.tx(|| {
        let j = app.db.q1("SELECT target FROM jobs WHERE id = ?", p![jid])?;
        let mut t = j.map(|j| jloads_obj(j.s("target"))).unwrap_or_default();
        t.insert(k.into(), v);
        app.db.x("UPDATE jobs SET target = ? WHERE id = ?", p![jdumps(&Value::Object(t)), jid])?;
        Ok(())
    })
}

fn send(app: &App, sid: &str, text: &str) -> MResult<()> {
    call(app, "queue.add", json!({"session": sid, "text": text, "enter": true, "when": {"kind": "idle"}}))?;
    Ok(())
}

fn live_claude(app: &App, sid: &str) -> bool {
    matches!(board::get_session(app, Some(sid)), Ok(Some(s)) if s.s("status") != Some("gone") && board::runs_claude(&s))
}

/// The open terminal an agent job types into, if it has one.
pub fn send_target(app: &App, a: &Row) -> Option<String> {
    if let Some(s) = a.s("session_id").filter(|s| !s.is_empty()) {
        return Some(s.to_string());
    }
    a.s("replace").filter(|r| !r.is_empty() && live_claude(app, r)).map(|r| r.to_string())
}

pub fn claude_args(a: &Row) -> MResult<(Vec<String>, Option<String>)> {
    let flags = a.st("flags");
    let mut parts = shlex::split(&flags).ok_or_else(|| MidnaError::Refused("its Claude flags don't parse".into()))?;
    let mut resume = None;
    if let Some(i) = parts.iter().position(|p| p == "--resume") {
        if i + 1 < parts.len() {
            resume = Some(parts[i + 1].clone());
            parts.drain(i..i + 2);
        }
    }
    if let Some(s) = a.s("settings").filter(|s| !s.is_empty()) {
        parts.extend(["--settings".to_string(), s.to_string()]);
    }
    if let Some(c) = a.s("context").filter(|s| !s.is_empty()) {
        parts.extend(["--append-system-prompt".to_string(), c.to_string()]);
    }
    Ok((parts, resume))
}

/// Whether this job's terminal opens in Midna's Background group (config.toml's `[terminals]`).
pub fn opens_in_background(app: &App, j: &Row) -> bool {
    j.s("purpose").is_some_and(|p| app.cfg.terminals.background.iter().any(|b| b.trim() == p))
}

/// A live terminal whose conversation has gone cold, with its size and idle minutes.
fn cold_open(app: &App, sid: &str) -> Option<limits::Conversation> {
    let Ok(Some(s)) = board::get_session(app, Some(sid)) else { return None };
    let cid = s.s("claude_session_id").filter(|c| !c.is_empty())?;
    let c = limits::conversation(app, s.s("project_path").unwrap_or(""), cid, s.s("last_activity"));
    limits::is_cold(app, &c).then_some(c)
}

/// The task event for compacting an open terminal before sending it anything.
pub fn compact_open_text(c: &limits::Conversation) -> String {
    let mins = c.idle_mins.unwrap_or(0.0) as i64;
    let size = c.tokens.map(|n| format!(" ({} tokens)", limits::tokens_text(n))).unwrap_or_default();
    format!("Its conversation has been idle {mins} min{size}, so the board compacts it before sending it anything.")
}

/// A cold open terminal gets `/compact` ahead of whatever the job sends it, logged on the job's task.
/// The job's target remembers it, so a job retried after Midna went down doesn't compact twice.
fn compact_first(app: &App, j: &Row, sid: &str) -> MResult<()> {
    if board::job_target(j).get("compacted").is_some() {
        return Ok(());
    }
    let Some(c) = cold_open(app, sid) else { return Ok(()) };
    send(app, sid, "/compact")?;
    set_target(app, j.id(), "compacted", json!(sid))?;
    let what = compact_open_text(&c);
    app.info(format!("job {}: {what}", rf("job", j.id())));
    if let Some(tid) = j.i("task_id") {
        let _ = app.db.tx(|| board::log_event(app, tid, board::BOARD, "midna", &what).map(|_| ()));
    }
    Ok(())
}

/// A conversation resumed in a new terminal after going cold: its size and idle minutes, logged on the
/// job's task. The terminal opens at once and compacts it in the tab, ahead of the prompt.
fn cold_resume(app: &App, j: &Row, cwd: &str, cid: &str) -> Option<limits::Conversation> {
    let c = limits::conversation(app, cwd, cid, None);
    if !limits::is_cold(app, &c) {
        return None;
    }
    let mins = c.idle_mins.unwrap_or(0.0) as i64;
    let size = c.tokens.map(|n| format!(" ({} tokens)", limits::tokens_text(n))).unwrap_or_default();
    let what = format!("Its conversation has been idle {mins} min{size}, so the new terminal compacts it before carrying on.");
    app.info(format!("job {}: {what}", rf("job", j.id())));
    if let Some(tid) = j.i("task_id") {
        let _ = app.db.tx(|| board::log_event(app, tid, board::BOARD, "handoff", &what).map(|_| ()));
    }
    Some(c)
}

fn run_agent(app: &App, j: &Row, a: &Row) -> MResult<String> {
    if let Some(sid) = send_target(app, a) {
        compact_first(app, j, &sid)?;
        send(app, &sid, &a.st("prompt"))?;
        return Ok(format!("Queued in {sid}"));
    }
    let cwd = a.st("cwd");
    if cwd.is_empty() {
        return Err(MidnaError::Refused("an agent job needs a folder".into()));
    }
    if !Path::new(&cwd).is_dir() {
        return Err(MidnaError::Refused(format!("the folder {cwd} doesn't exist")));
    }
    let mut a = a.clone();
    if !has(a.s("settings")) {
        if let Some(s) = crate::builds::settings_arg(app, &cwd) {
            a.insert("settings".into(), json!(s));
        }
    }
    let (agent_args, resume) = claude_args(&a)?;
    let cold = resume.as_deref().and_then(|cid| cold_resume(app, j, &cwd, cid)).is_some();
    let prompt = a.s("prompt").filter(|p| !p.is_empty());
    let mut params = json!({"kind": "agent", "agent": "claude", "cwd": cwd,
                            "name": tab_title(a.s("title").unwrap_or("Claude"), j.i("task_id")),
                            "background": as_bool(a.get("background"), false) || opens_in_background(app, j), "close_on_exit": true, "agent_args": agent_args});
    if let Some(r) = &resume {
        params["resume"] = json!(r);
    }
    // A cold conversation's prompt waits behind `/compact` in the terminal's queue instead.
    if let Some(pr) = prompt.filter(|_| !cold) {
        params["prompt"] = json!(pr);
    }
    let s = call(app, "session.open", params)?;
    let Some(sid) = s.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()) else {
        return Err(MidnaError::Refused("Midna opened no terminal".into()));
    };
    set_target(app, j.id(), "session", json!(sid))?;
    // The prompt goes in even when its compact couldn't: a full-size conversation beats one with no prompt.
    if cold {
        let compacted = send(app, &sid, "/compact");
        if let Some(Err(e)) = prompt.map(|pr| send(app, &sid, pr)) {
            return Ok(format!("Opened {sid}, but couldn't queue its prompt: {e}"));
        }
        if let Err(e) = compacted {
            return Ok(format!("Opened {sid}, but couldn't queue its compact: {e}"));
        }
    }
    // A `draft` is typed in once Claude is ready, without Enter: the owner reads it and sends it.
    if let Some(d) = a.s("draft").filter(|d| !d.is_empty()) {
        if let Err(e) = call(app, "queue.add", json!({"session": sid, "text": d, "enter": false})) {
            return Ok(format!("Opened {sid}, but couldn't type its request in: {e}"));
        }
    }
    // Midna opens a terminal without switching to it; one the owner asked for is shown.
    if as_bool(a.get("focus"), false) {
        if let Err(e) = call(app, "session.focus", json!({"id": sid})) {
            return Ok(format!("Opened {sid}, but couldn't show it: {e}"));
        }
    }
    Ok(format!("Opened {sid}"))
}

fn run_handler(app: &App, j: &Row) -> MResult<String> {
    let a = board::job_args(j);
    match j.st("kind").as_str() {
        "agent" => run_agent(app, j, &a),
        "message" => {
            if !has(a.s("to")) || !has(a.s("text")) {
                return Err(MidnaError::Refused("a message job needs to and text".into()));
            }
            compact_first(app, j, &a.st("to"))?;
            send(app, &a.st("to"), &a.st("text"))?;
            Ok(format!("Queued in {}", a.st("to")))
        }
        "close" => {
            if !has(a.s("session")) {
                return Err(MidnaError::Refused("a close job needs a terminal".into()));
            }
            call(app, "session.close", json!({"id": a.st("session"), "force": as_bool(a.get("force"), false)}))?;
            Ok(format!("Closed {}", a.st("session")))
        }
        "focus" => {
            call(app, "session.focus", json!({"id": a.st("session")}))?;
            Ok(format!("Focused {}", a.st("session")))
        }
        "rename" => {
            call(app, "session.rename", json!({"id": a.st("session"), "name": a.st("name")}))?;
            Ok(format!("Renamed {}", a.st("session")))
        }
        other => Err(MidnaError::Refused(format!("the board doesn't know a {other} job"))),
    }
}

pub fn run_job(app: &App, j: &Row) {
    let (ok, stdout, stderr, code) = match run_handler(app, j) {
        Err(MidnaError::Down(e)) => {
            let _ = app.db.tx(|| {
                app.db.x("UPDATE jobs SET state = 'pending', updated_at = ? WHERE id = ? AND state = 'running'", p![now_iso(), j.id()])
            });
            app.info(format!("job {} {} waits: {e}", rf("job", j.id()), j.st("kind")));
            return;
        }
        Err(MidnaError::Refused(e)) => (false, String::new(), e, 1),
        Ok(out) => (true, out, String::new(), 0),
    };
    if let Err(e) = app.db.tx(|| jobs::result(app, j.id(), ok, &stdout, &stderr, code)) {
        app.info(format!("job {}: recording its result failed: {e}", rf("job", j.id())));
    }
    app.wake_runner();
}

pub fn jobs_loop(app: Arc<App>) {
    while !app.stopping() {
        let down = app.shared.lock().midna_down;
        if !down {
            send_tabs(&app);
        }
        let j = if down {
            None
        } else {
            match jobs::take(&app) {
                Ok(j) => j,
                Err(e) => {
                    app.info(format!("midna: taking a job failed: {e}"));
                    None
                }
            }
        };
        if let Some(j) = j {
            let a = app.clone();
            std::thread::spawn(move || run_job(&a, &j));
            continue;
        }
        app.wait_jobs(Duration::from_secs(2));
    }
}

fn git_of(s: &Value) -> (Value, Value) {
    let g = s.get("git").and_then(|g| g.as_object());
    match g {
        Some(g) => {
            let dirty = ["files", "added", "removed"].iter().any(|k| g.get(*k).map(|v| v.as_i64().unwrap_or(0) > 0 || v == &json!(true)).unwrap_or(false));
            (g.get("branch").cloned().unwrap_or(Value::Null), json!(dirty as i64))
        }
        None => (Value::Null, Value::Null),
    }
}

fn state_of(s: &str) -> &'static str {
    match s {
        "working" => "working",
        "needs_you" => "needs",
        _ => "idle",
    }
}

/// Background work states Midna lists that are over.
const BACKGROUND_DONE: &[&str] = &["completed", "failed", "killed", "stopped", "cancelled"];

/// Background kinds Midna lists that aren't work the terminal waits on: a monitor (Claude arms one
/// on every artifact it publishes) watches until it's stopped or expires, it never finishes by itself.
const NOT_BACKGROUND_WORK: &[&str] = &["monitor"];

/// How much background work Midna lists running in a terminal's `agent_info`, and how much of it is
/// agents: None when Midna doesn't say (no `background` list and no `background_at`). Midna leaves
/// an empty list out, so `background_at` alone means nothing's running. Monitors don't count.
pub fn live_background(info: &Row) -> Option<(i64, i64)> {
    let list = info.get("background").and_then(|v| v.as_array());
    if list.is_none() && info.get("background_at").is_none_or(|v| v.is_null()) {
        return None;
    }
    let running: Vec<&Value> = list
        .into_iter()
        .flatten()
        .filter(|t| !BACKGROUND_DONE.contains(&t["status"].as_str().unwrap_or("")))
        .filter(|t| !NOT_BACKGROUND_WORK.contains(&t["kind"].as_str().unwrap_or("")))
        .collect();
    let agents = running.iter().filter(|t| t["kind"] == "subagent").count();
    Some((running.len() as i64, agents as i64))
}

/// Applies Midna's terminal and project lists to the board.
pub fn sync(app: &App, sessions: &[Value], projects: &[Value]) -> Result<(Vec<String>, Vec<String>)> {
    let mut seen_ids: Vec<String> = vec![];
    let mut gone: Vec<String> = vec![];
    let mut to_close: Vec<String> = vec![];
    app.db.tx(|| {
        if !projects.is_empty() {
            let list: Vec<Value> = projects
                .iter()
                .filter(|p| p.get("path").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false))
                .map(|p| json!({"name": p.get("name"), "path": p.get("path")}))
                .collect();
            app.db.set_setting("midna_projects", Some(&jdumps(&Value::Array(list))))?;
        }
        let now = now_iso();
        for s in sessions {
            let Some(sid) = s.get("id").map(|v| v.as_str().map(|x| x.to_string()).unwrap_or_else(|| v.to_string())) else { continue };
            let cur = board::get_session(app, Some(&sid))?;
            let st = s.get("status").and_then(|v| v.as_object()).cloned().unwrap_or_default();
            let agent = s.get("agent").and_then(|v| v.as_str()).unwrap_or("");
            let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if agent.is_empty() {
                let was_claude = cur.as_ref().map(|c| c.s("status") != Some("gone") && c.st("agent").to_lowercase().contains("claude")).unwrap_or(false);
                if was_claude && st.s("reason") == Some(CLAUDE_EXITED) {
                    board::session_gone(app, &sid, "Claude exited")?;
                    gone.push(sid.clone());
                    if board_tab(&name) {
                        to_close.push(sid.clone());
                    }
                }
                continue;
            }
            seen_ids.push(sid.clone());
            let proj = projects
                .iter()
                .find(|p| p.get("id").is_some() && p.get("id") == s.get("project_id"))
                .and_then(|p| p.as_object())
                .cloned()
                .unwrap_or_default();
            let path = proj.s("path").map(|x| x.to_string()).or_else(|| s.get("cwd").and_then(|v| v.as_str()).map(|x| x.to_string()));
            let info = s.get("agent_info").and_then(|v| v.as_object()).cloned().unwrap_or_default();
            let (branch, dirty) = git_of(s);
            let state = st.st("state");
            if state == "exited" {
                if cur.as_ref().map(|c| c.s("status") != Some("gone")).unwrap_or(false) {
                    board::session_gone(app, &sid, "Claude exited")?;
                    gone.push(sid.clone());
                }
                continue;
            }
            let project = proj.s("name").map(|x| x.to_string()).or_else(|| path.as_deref().and_then(base_name));
            let mut f = crate::fields![
                "name" => name, "project" => project, "project_path" => path, "branch" => branch, "dirty" => dirty,
                "agent" => agent, "status" => state_of(&state), "prompt_waiting" => (state == "needs_you") as i64,
                "last_activity" => s.get("last_activity_at"), "source" => "midna", "seen_at" => now, "missed" => 0,
                "gone_at" => null
            ];
            if let Some(c) = info.s("conversation_id").filter(|c| !c.is_empty()) {
                f.push(("claude_session_id", json!(c)));
            }
            let (live, live_agents) = live_background(&info).map_or((Value::Null, Value::Null), |(n, a)| (json!(n), json!(a)));
            f.push(("live_background", live));
            f.push(("live_background_agents", live_agents));
            if let Some(cur) = &cur {
                board::note_rename(app, Some(cur), Some(&name))?;
                app.db.update("sessions", &json!(sid), f)?;
            } else {
                let mut row = vec![("id", json!(sid))];
                row.extend(f);
                app.db.insert("sessions", row)?;
            }
            if let Some(t) = board::task_for_session(app, Some(&sid))? {
                if !name.is_empty() && t.s("session_name") != Some(name.as_str()) {
                    board::update_task(app, t.id(), crate::fields!["session_name" => name])?;
                }
            }
        }
        for s in app.db.q("SELECT * FROM sessions WHERE status != 'gone' AND source = 'midna'", p![])? {
            let sid = s.st("id");
            if seen_ids.contains(&sid) || gone.contains(&sid) {
                continue;
            }
            let missed = s.i0("missed") + 1;
            app.db.x("UPDATE sessions SET missed = ? WHERE id = ?", p![missed, sid])?;
            if missed >= app.cfg.intervals.gone_after_missed {
                board::session_gone(app, &sid, "gone from Midna")?;
                gone.push(sid);
            }
        }
        Ok(())
    })?;
    if !gone.is_empty() {
        app.info(format!("midna: {} gone", plural(gone.len() as i64, "terminal")));
    }
    Ok((gone, to_close))
}

const SETTINGS_EVERY: Duration = Duration::from_secs(60);
/// What Midna does after a lost connection, and how long it keeps trying (from its setting's text).
pub const RESUME_TRIES: i64 = 3;
pub const RESUME_GIVES_UP_SECS: f64 = 6.0 * 3600.0;

/// Whether Midna sends a lost-network turn `continue` once the network is back. On unless Midna
/// says it's off: that's its default.
pub fn resumes_after_network(app: &App) -> bool {
    app.shared.lock().midna_resumes_network.map(|(_, on)| on).unwrap_or(true)
}

fn read_settings(app: &App) {
    if app.shared.lock().midna_resumes_network.is_some_and(|(at, _)| at.elapsed() < SETTINGS_EVERY) {
        return;
    }
    if let Ok(v) = call_timeout(app, "settings.get", json!({"key": "agents.resume_after_network"}), 5.0) {
        let on = v["value"].as_bool().unwrap_or(true);
        app.shared.lock().midna_resumes_network = Some((Instant::now(), on));
    }
}

/// How often the board checks that Midna (maybe restarted) has its link patterns.
const LINKS_EVERY: Duration = Duration::from_secs(30);
/// The name Midna files the board's link patterns under (and shows in its Settings).
fn links_owner(app: &App) -> &'static str {
    if app.cfg.page_url.starts_with("taskboard-dev:") { "Taskboard Dev" } else { "Taskboard" }
}

/// The refs that become links in agent terminals, each opening this build's own page for it.
pub fn link_patterns(app: &App) -> Vec<String> {
    [("T", "task"), ("G", "goal"), ("B", "issue"), ("R", "release")]
        .iter()
        .map(|(letter, page)| format!(r"\b({letter}\d+)\b = {} agents", dispatch::page_link(app, &format!("{page}/$1"))))
        .collect()
}

/// Registers the board's link patterns with each Midna it meets: once per Midna run, checked every
/// `LINKS_EVERY`. A Midna too old to know `links.patterns.register` is logged once and left be.
fn register_links(app: &App) {
    if app.shared.lock().midna_links.as_ref().is_some_and(|(at, _)| at.elapsed() < LINKS_EVERY) {
        return;
    }
    let last = app.shared.lock().midna_links.as_ref().and_then(|(_, r)| r.clone());
    let Ok(info) = call_timeout(app, "daemon.info", json!({}), 5.0) else {
        app.shared.lock().midna_links = Some((Instant::now(), last));
        return;
    };
    let run = format!("{} {}", info["pid"], info["started_at"]);
    let has = if last.as_deref() == Some(run.as_str()) {
        true
    } else {
        match call_timeout(app, "links.patterns.register", json!({"owner": links_owner(app), "patterns": link_patterns(app)}), 5.0) {
            Ok(_) => true,
            Err(MidnaError::Refused(e)) if e.contains("unknown method") => {
                app.info("midna: it doesn't take link patterns yet (links.patterns.register); T, G, B and R refs stay plain text there");
                true
            }
            Err(e) => {
                app.info(format!("midna: couldn't register link patterns: {e}"));
                false
            }
        }
    };
    if app.cfg.runner {
        remove_old_triggers(app, &run);
    }
    app.shared.lock().midna_links = Some((Instant::now(), if has { Some(run) } else { last }));
}

/// The tab statuses the board gives a task's terminal: what it sends, the label, its color and the
/// state Midna sorts and notifies by. Each comes off with the terminal's next prompt.
pub const TAB_STATUSES: [(&str, &str, &str, &str); 6] = [
    ("in_review", "In review", "purple", "done"),
    ("done", "Done", "green", "done"),
    ("question", "Question", "amber", "needs_you"),
    ("alert", "Alert", "red", "needs_you"),
    ("failed", "Task failed", "red", "failed"),
    ("waiting", "Waiting", "blue", "idle"),
];
/// What the board sends to take its tab status down.
pub const TAB_CLEAR: &str = "clear";
/// The most tab statuses kept while Midna is away; older ones are stale by the time it's back.
const TABS_KEPT: usize = 50;
/// The task fields a tab status is read from (`task_tab`).
pub fn tab_field(field: &str) -> bool {
    matches!(field, "status" | "failed" | "needs_reason" | "waits_for" | "pr_num" | "pr_url" | "pr_phase" | "pr_state" | "pr_flow")
}

/// The Midna call that puts a tab status (one of `TAB_STATUSES`, or `TAB_CLEAR`) on terminal `sid`.
/// `detail` says what it's waiting on; Midna shows it on the needs-you card a needs_you status raises.
pub fn tab_call(sid: &str, tab: &str, detail: Option<&str>) -> (&'static str, Value) {
    match TAB_STATUSES.iter().find(|(t, ..)| *t == tab) {
        Some((_, label, color, base)) => {
            let mut params = json!({"id": sid, "label": label, "color": color, "base": base, "clear_on": "prompt"});
            if let Some(d) = detail.filter(|d| !d.is_empty()) {
                params["detail"] = json!(d);
            }
            ("session.set_status", params)
        }
        None => ("session.clear_status", json!({"id": sid})),
    }
}

/// The most of a tab's detail sent; Midna's card shows the start of it.
const TAB_DETAIL_CHARS: usize = 300;

/// What a task's tab status `tab` (from `task_tab`) is waiting on, for a needs_you one: the task and
/// its question, or the question its PR visit stopped to ask.
pub fn task_tab_detail(t: &Row, tab: &str) -> Option<String> {
    if tab != "question" {
        return None;
    }
    let ask = match t.s("status") {
        Some("done") => {
            let msg = jloads_obj(t.s("pr_flow")).get("stopped").and_then(|s| s["message"].as_str().map(|m| m.to_string()));
            format!("asked on PR #{}: {}", t.i0("pr_num"), msg.unwrap_or_default())
        }
        _ => t.s("question").filter(|q| !q.trim().is_empty()).unwrap_or("waiting for your answer").to_string(),
    };
    Some(task_detail(t, &ask))
}

/// `T<n> · <title>: <what>`, on one line.
pub fn task_detail(t: &Row, what: &str) -> String {
    one_line(&format!("{} · {}: {}", rf("task", t.id()), t.st("title"), what.trim()), TAB_DETAIL_CHARS)
}

/// Takes out the local triggers older boards kept in Midna for tab statuses (hook events the board
/// no longer sends), once per Midna run (`run`, its pid and start). An enabled one is paused first,
/// since Midna asks Morgan before removing an enabled trigger.
fn remove_old_triggers(app: &App, run: &str) {
    if app.shared.lock().midna_old_triggers.as_deref() == Some(run) {
        return;
    }
    let have = match call_timeout(app, "trigger.list", json!({}), 5.0) {
        Ok(v) => v.as_array().cloned().unwrap_or_default(),
        Err(MidnaError::Down(_)) => return,
        Err(e) => {
            app.info(format!("midna: couldn't read its triggers to take out the old tab-status ones: {e}"));
            app.shared.lock().midna_old_triggers = Some(run.to_string());
            return;
        }
    };
    let owner = links_owner(app);
    let event = format!("hook.{}", owner.replace(' ', ""));
    let names: Vec<String> =
        TAB_STATUSES.iter().map(|(_, label, ..)| *label).chain(["Clear status"]).map(|n| format!("{owner}: {n}")).collect();
    let old = have.iter().filter(|t| t["source"] == "local" && t["event"] == event.as_str() && names.iter().any(|n| t["name"] == n.as_str()));
    for t in old {
        let id = &t["id"];
        let paused = match t["enabled"] == true {
            true => call_timeout(app, "trigger.set_enabled", json!({"id": id, "enabled": false}), 5.0),
            false => Ok(Value::Null),
        };
        if let Err(e) = paused.and_then(|_| call_timeout(app, "trigger.remove", json!({"id": id}), 5.0)) {
            app.info(format!("midna: couldn't remove the old {} trigger: {e}", t["name"].as_str().unwrap_or("")));
        }
    }
    app.shared.lock().midna_old_triggers = Some(run.to_string());
}

/// What a task's terminal tab shows for its state, if anything.
pub fn task_tab(t: &Row) -> Option<&'static str> {
    if t.s("status") == Some("done") {
        let merged = t.s("pr_phase") == Some("merged") || t.s("pr_state").is_some_and(|s| s.eq_ignore_ascii_case("MERGED"));
        let asked = jloads_obj(t.s("pr_flow")).get("stopped").is_some_and(|s| s["asked"] == true);
        return if t.b("failed") {
            Some("failed")
        } else if merged || (t.i("pr_num").is_none() && !has(t.s("pr_url"))) {
            Some("done")
        } else if board::pr_still_open(t) {
            Some(if asked { "question" } else { "in_review" })
        } else {
            None
        };
    }
    if t.s("status") == Some("needs") && t.s("needs_reason") == Some("question") {
        return Some("question");
    }
    // waits_for outlives the wait, so only a task parked on it is waiting.
    (t.s("status") == Some("queued") && crate::waitsfor::parked(t).is_some()).then_some("waiting")
}

/// The terminal a task's tab status goes on: the one visiting its PR, else its own, else the one it
/// was parked from. None when that terminal has moved on to another task, open or finished since.
pub fn tab_terminal(app: &App, t: &Row) -> Result<Option<String>> {
    let visit = jloads_obj(t.s("pr_flow")).get("session").and_then(|s| s.as_str()).map(|s| s.to_string());
    let parked = crate::waitsfor::parked(t).and_then(|p| p.get("session").and_then(|s| s.as_str()).map(|s| s.to_string()));
    let own = t.s("session_id").filter(|s| !s.is_empty()).map(|s| s.to_string());
    let Some(sid) = visit.filter(|_| t.s("status") == Some("done")).or(own).or(parked).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let other = app.db.count(
        "SELECT COUNT(*) FROM tasks WHERE session_id = ? AND id != ? AND (status != 'done' OR (? != '' AND COALESCE(finished_at, '') > ?))",
        p![sid, t.id(), t.st("finished_at"), t.st("finished_at")],
    )?;
    Ok((other == 0).then_some(sid))
}

/// Sets a task's terminal tab when its state moved from `before` to `after` changes what it shows.
pub fn task_changed(app: &App, before: &Row, after: &Row) -> Result<()> {
    let now = task_tab(after);
    if now == task_tab(before) || !app.cfg.runner {
        return Ok(());
    }
    if let Some(sid) = tab_terminal(app, after)? {
        set_tab(app, &sid, now.unwrap_or(TAB_CLEAR), now.and_then(|n| task_tab_detail(after, n)));
    }
    Ok(())
}

/// Queues a tab status (one of `TAB_STATUSES`, or `TAB_CLEAR`) for a terminal, with what it's waiting
/// on (`tab_call`); the jobs thread sends it.
pub fn set_tab(app: &App, sid: &str, tab: &'static str, detail: Option<String>) {
    if !app.cfg.runner || sid.is_empty() {
        return;
    }
    let mut s = app.shared.lock();
    if s.midna_tabs.len() >= TABS_KEPT {
        s.midna_tabs.pop_front();
    }
    s.midna_tabs.push_back((sid.to_string(), tab, detail));
    drop(s);
    app.notify_jobs();
}

/// Sends the queued tab statuses, in order. One Midna won't take is logged and dropped; with Midna
/// down they wait. A Midna too old for `session.set_status` is logged once and its tabs show none.
pub fn send_tabs(app: &App) {
    static TOO_OLD: AtomicBool = AtomicBool::new(false);
    loop {
        let Some((sid, tab, detail)) = app.shared.lock().midna_tabs.pop_front() else { return };
        let (method, params) = tab_call(&sid, tab, detail.as_deref());
        match call_timeout(app, method, params, 5.0) {
            Ok(_) => {}
            Err(MidnaError::Down(_)) => {
                app.shared.lock().midna_tabs.push_front((sid, tab, detail));
                return;
            }
            Err(MidnaError::Refused(e)) if e.contains("unknown method") => {
                if !TOO_OLD.swap(true, Ordering::Relaxed) {
                    app.info(format!("midna: it doesn't take tab statuses yet ({method}); terminal tabs show no task status until it's updated"));
                }
            }
            Err(MidnaError::Refused(e)) => app.info(format!("midna: couldn't set {sid}'s tab to {tab}: {e}")),
        }
    }
}

/// How far back the first look at Midna's event log reaches (the board keeps its place after that).
const CLOSED_LOOKBACK_SECS: f64 = 60.0;

/// Applies Midna's `session.closed` events: who closed each terminal.
pub fn closed_events(app: &App, events: &[Value]) -> Result<()> {
    let mut last = app.db.get_setting("midna_event_seq")?.and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    app.db.tx(|| {
        for e in events {
            let seq = e["seq"].as_u64().unwrap_or(0);
            last = last.max(seq);
            let (Some(sid), Some(by)) = (e["session_id"].as_str(), e["actor"]["kind"].as_str()) else { continue };
            if e["kind"] != "session.closed" || age_secs(e["at"].as_str()).is_none_or(|a| a > CLOSED_LOOKBACK_SECS) {
                continue;
            }
            board::terminal_closed(app, sid, by)?;
        }
        app.db.set_setting("midna_event_seq", Some(&last.to_string()))
    })
}

/// The last `events.list` failure logged, so one that keeps failing is logged once.
static CLOSED_FAILED: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

/// Reads who closed which terminals. A failed read is logged and the sync carries on: a terminal
/// that then goes missing just ends for an unknown reason.
fn read_closed(app: &App) {
    let since = app.db.get_setting("midna_event_seq").ok().flatten().and_then(|s| s.parse::<u64>().ok());
    match call_timeout(app, "events.list", json!({"since_seq": since, "limit": 200, "filter": {"kinds": ["session.closed"]}}), 10.0) {
        Ok(events) => {
            *CLOSED_FAILED.lock().unwrap_or_else(|e| e.into_inner()) = None;
            if let Err(e) = closed_events(app, events.as_array().map(|a| a.as_slice()).unwrap_or(&[])) {
                app.info(format!("midna: couldn't apply closed terminals: {e}"));
            }
        }
        // Midna isn't running: the session list finds that out too.
        Err(MidnaError::Down(_)) => {}
        Err(MidnaError::Refused(e)) => {
            let mut last = CLOSED_FAILED.lock().unwrap_or_else(|e| e.into_inner());
            if last.as_deref() != Some(e.as_str()) {
                app.info(format!("midna: couldn't read closed terminals: {e}"));
                *last = Some(e);
            }
        }
    }
}

pub fn sync_once(app: &App) -> MResult<()> {
    // Before the list, so a terminal Morgan closed is known to be theirs when it goes missing.
    read_closed(app);
    let sessions = call_timeout(app, "session.list", json!({}), 10.0)?;
    let projects = call_timeout(app, "project.list", json!({}), 10.0)?;
    let empty = vec![];
    let (_, to_close) = sync(app, sessions.as_array().unwrap_or(&empty), projects.as_array().unwrap_or(&empty))?;
    let usage = call_timeout(app, "usage.get", json!({}), 10.0).ok();
    app.shared.lock().midna_usage = usage.and_then(|u| u.get("claude").cloned()).filter(|v| v.is_object());
    keep_awake::sync(app);
    read_settings(app);
    register_links(app);
    crate::statusbar::offer(app);
    for sid in to_close.into_iter().filter(|_| app.cfg.runner) {
        if let Err(e) = call(app, "session.close", json!({"id": sid})) {
            app.info(format!("midna: couldn't close {sid} after Claude exited: {e}"));
        }
    }
    app.wake_runner();
    Ok(())
}

pub fn sync_loop(app: Arc<App>) {
    while !app.stopping() {
        match sync_once(&app) {
            Ok(()) | Err(MidnaError::Down(_)) => {}
            Err(MidnaError::Refused(e)) => app.info(format!("midna: list failed: {e}")),
        }
        app.sleep(app.cfg.intervals.midna_sync);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_patterns_open_this_builds_pages() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = crate::config::Config::for_tests(dir.path());
        cfg.page_url = "taskboard://".into();
        let app = App::for_tests(cfg);
        assert_eq!(links_owner(&app), "Taskboard");
        assert_eq!(
            link_patterns(&app),
            [
                r"\b(T\d+)\b = taskboard://task/$1 agents",
                r"\b(G\d+)\b = taskboard://goal/$1 agents",
                r"\b(B\d+)\b = taskboard://issue/$1 agents",
                r"\b(R\d+)\b = taskboard://release/$1 agents",
            ]
        );
        let mut cfg = crate::config::Config::for_tests(dir.path());
        cfg.page_url = "taskboard-dev://".into();
        let app = App::for_tests(cfg);
        assert_eq!(links_owner(&app), "Taskboard Dev");
        assert_eq!(link_patterns(&app)[2], r"\b(B\d+)\b = taskboard-dev://issue/$1 agents");
    }

    #[test]
    fn tab_titles() {
        assert_eq!(tab_title("Fix  login", Some(12)), "T12 Fix login");
        assert_eq!(tab_title("Planner", None), "TB Planner");
        assert_eq!(tab_title("T3 Already", Some(9)), "T3 Already");
        assert_eq!(tab_title("TB Desk", Some(4)), "T4 Desk");
    }

    #[test]
    fn claude_flags() {
        let mut a = Row::new();
        a.insert("flags".into(), json!("--resume abc --model opus"));
        a.insert("context".into(), json!("be brief"));
        let (args, resume) = claude_args(&a).unwrap();
        assert_eq!(resume.as_deref(), Some("abc"));
        assert_eq!(args, vec!["--model", "opus", "--append-system-prompt", "be brief"]);
        a.insert("flags".into(), json!("--x \"unclosed"));
        assert!(claude_args(&a).is_err());
    }
}
