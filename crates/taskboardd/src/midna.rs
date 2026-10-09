//! Midna, the terminal app: the board calls its CLI (`midna call <method> '<json>'`).

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, jobs, keep_awake, limits, p, proc};

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

/// A conversation resumed in a new terminal after going cold is compacted first, headless.
fn compact_before_resume(app: &App, j: &Row, cwd: &str, cid: &str) {
    if !limits::is_cold(app, &limits::conversation(app, cwd, cid, None)) {
        return;
    }
    let what = match limits::compact(app, cwd, cid) {
        Ok(()) => "Compacted its cold conversation before resuming it".to_string(),
        Err(e) => format!("Couldn't compact its cold conversation before resuming it ({e}); resuming it as it is"),
    };
    app.info(format!("job {}: {what}", rf("job", j.id())));
    if let Some(tid) = j.i("task_id") {
        let _ = app.db.tx(|| board::log_event(app, tid, board::BOARD, "handoff", &what).map(|_| ()));
    }
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
        if let Some(s) = limits::settings_arg(app) {
            a.insert("settings".into(), json!(s));
        }
    }
    let (agent_args, resume) = claude_args(&a)?;
    if let Some(cid) = &resume {
        compact_before_resume(app, j, &cwd, cid);
    }
    let mut params = json!({"kind": "agent", "agent": "claude", "cwd": cwd,
                            "name": tab_title(a.s("title").unwrap_or("Claude"), j.i("task_id")),
                            "background": as_bool(a.get("background"), false) || opens_in_background(app, j), "close_on_exit": true, "agent_args": agent_args});
    if let Some(r) = resume {
        params["resume"] = json!(r);
    }
    if let Some(pr) = a.s("prompt").filter(|p| !p.is_empty()) {
        params["prompt"] = json!(pr);
    }
    let s = call(app, "session.open", params)?;
    let Some(sid) = s.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()) else {
        return Err(MidnaError::Refused("Midna opened no terminal".into()));
    };
    set_target(app, j.id(), "session", json!(sid))?;
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

pub fn sync_once(app: &App) -> MResult<()> {
    let sessions = call_timeout(app, "session.list", json!({}), 10.0)?;
    let projects = call_timeout(app, "project.list", json!({}), 10.0)?;
    let empty = vec![];
    let (_, to_close) = sync(app, sessions.as_array().unwrap_or(&empty), projects.as_array().unwrap_or(&empty))?;
    let usage = call_timeout(app, "usage.get", json!({}), 10.0).ok();
    app.shared.lock().midna_usage = usage.and_then(|u| u.get("claude").cloned()).filter(|v| v.is_object());
    keep_awake::sync(app);
    read_settings(app);
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
