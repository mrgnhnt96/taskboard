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

/// Why Midna's socket turned a call down: its JSON-RPC error code (`midna_proto::error`) and message.
#[derive(Debug)]
pub enum RpcError {
    /// Nothing listens on the socket (Midna isn't running).
    Down(String),
    Refused { code: i64, message: String },
}

pub const RPC_UNKNOWN_METHOD: i64 = -32601;
pub const RPC_HUMAN_ONLY: i64 = 2;
pub const RPC_NOT_FOUND: i64 = 3;
pub const RPC_CONFLICT: i64 = 4;

/// Calls Midna over its JSON-RPC socket (newline-delimited JSON-RPC 2.0), as this process. Midna checks
/// a trusted client (`agents.trusted_clients`) on the socket's peer, so a call that needs that trust
/// can't go through the CLI.
pub fn rpc(sock: &Path, method: &str, params: &Value, timeout: f64) -> std::result::Result<Value, RpcError> {
    use std::io::{BufRead, BufReader, Write};
    let mut s = std::os::unix::net::UnixStream::connect(sock).map_err(|e| RpcError::Down(format!("Midna isn't running ({}: {e})", sock.display())))?;
    let t = Some(Duration::from_secs_f64(timeout));
    let refused = |m: String| RpcError::Refused { code: 0, message: m };
    let _ = s.set_read_timeout(t);
    let _ = s.set_write_timeout(t);
    let req = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    s.write_all(format!("{req}\n").as_bytes()).map_err(|e| refused(format!("Couldn't send {method} to Midna: {e}")))?;
    let mut lines = BufReader::new(s);
    let mut line = String::new();
    loop {
        line.clear();
        match lines.read_line(&mut line) {
            Ok(0) => return Err(refused(format!("Midna closed the connection before answering {method}"))),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                return Err(refused(format!("Midna didn't answer {method} within {} seconds", timeout as i64)));
            }
            Err(e) => return Err(refused(format!("Couldn't read Midna's answer to {method}: {e}"))),
            Ok(_) => {}
        }
        // Notifications (no id) and anything else share the connection; only our id answers.
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
        if v["id"] != json!(1) {
            continue;
        }
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            return Err(RpcError::Refused { code: e["code"].as_i64().unwrap_or(0), message: e["message"].as_str().unwrap_or("Midna refused").to_string() });
        }
        return Ok(v["result"].clone());
    }
}

/// The notification kind the board's notifications go as (their `category`).
pub const NOTIFY_KIND: &str = "board";

/// Sends a notification (`notify.send`). Midna refuses a category it has no kind for, so when the board's
/// kind isn't there (a Midna that never had it, or the owner removed it) it adds the kind and sends again.
pub fn notify_send(exe: &Path, params: &Value) -> MResult<Value> {
    match call_exe(exe, "notify.send", params, CALL_TIMEOUT) {
        Err(MidnaError::Refused(m)) if m.contains(&format!("no kind `{NOTIFY_KIND}`")) => {
            // Another send may have added it meanwhile; the second send says whether it's there.
            let _ = call_exe(
                exe,
                "notify.kinds.add",
                &json!({"key": NOTIFY_KIND, "label": "Task board", "description": "Starts to approve, alerts, digests and reviews from the task board"}),
                CALL_TIMEOUT,
            );
            call_exe(exe, "notify.send", params, CALL_TIMEOUT)
        }
        r => r,
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

/// Whether this job's terminal opens in Midna's Background group (config.toml's `[terminals]`): its
/// purpose is listed, or `<purpose>:<visit>` for the kind of PR visit it is (`pr:merge`).
pub fn opens_in_background(app: &App, j: &Row) -> bool {
    let Some(p) = j.s("purpose") else { return false };
    let visit = board::job_args(j).s("visit").map(|v| format!("{p}:{v}"));
    app.cfg.terminals.background.iter().any(|b| b.trim() == p || Some(b.trim()) == visit.as_deref())
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
    // A slot's terminal is left for the board to `close`: closing itself on exit runs Midna's cleanup.
    let mut params = json!({"kind": "agent", "agent": "claude", "cwd": cwd,
                            "name": tab_title(a.s("title").unwrap_or("Claude"), j.i("task_id")),
                            "background": as_bool(a.get("background"), false) || opens_in_background(app, j),
                            "close_on_exit": !crate::slots::is_slot(&cwd), "agent_args": agent_args});
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

/// Closes a terminal. One in a pool slot skips Midna's close cleanup, which would remove the slot's
/// worktree and branch from under the pool.
fn close(app: &App, sid: &str, cwd: Option<&str>, force: bool) -> MResult<Value> {
    let mut params = json!({"id": sid, "force": force});
    if cwd.is_some_and(crate::slots::is_slot) {
        params["cleanup"] = json!(false);
    }
    call(app, "session.close", params)
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
            let cwd = call(app, "session.get", json!({"id": a.st("session")})).ok().and_then(|s| s.get("cwd").and_then(|c| c.as_str()).map(|c| c.to_string()));
            close(app, &a.st("session"), cwd.as_deref(), as_bool(a.get("force"), false))?;
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
            send_panels(&app);
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

/// How stale a terminal's `seen_at` may get while nothing else about it changes. Stamping it on every
/// sync would move the board's mark each tick, and nothing reads it closer than this.
const SEEN_EVERY_SECS: f64 = 60.0;

/// Applies Midna's terminal and project lists to the board.
pub fn sync(app: &App, sessions: &[Value], projects: &[Value]) -> Result<(Vec<String>, Vec<String>)> {
    let mut seen_ids: Vec<String> = vec![];
    let mut gone: Vec<String> = vec![];
    let mut to_close: Vec<String> = vec![];
    app.db.tx(|| {
        if !projects.is_empty() {
            // A worktree folder Midna lists is its repo's project, not one of its own; a folder that's gone isn't one.
            let list: Vec<Value> = projects
                .iter()
                .filter(|p| {
                    let path = p.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    !path.is_empty() && std::path::Path::new(path).exists() && crate::projects::main_checkout(path).is_none()
                })
                .map(|p| json!({"name": p.get("name"), "path": p.get("path")}))
                .collect();
            let list = jdumps(&Value::Array(list));
            if app.db.get_setting("midna_projects")?.as_deref() != Some(list.as_str()) {
                crate::reqcache::forget_setting("midna_projects");
                app.db.set_setting("midna_projects", Some(&list))?;
            }
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
                    board::session_gone_with(app, &sid, "Claude exited", describe_exit(Some(s), None, now_ts()).as_deref())?;
                    gone.push(sid.clone());
                }
                continue;
            }
            let project = match path.as_deref().and_then(crate::projects::main_checkout) {
                Some(main) => crate::projects::midna_project_at(app, Some(&main))?.or_else(|| base_name(&main)),
                None => proj.s("name").map(|x| x.to_string()).or_else(|| path.as_deref().and_then(base_name)),
            };
            let mut f = crate::fields![
                "name" => name, "project" => project, "project_path" => path, "branch" => branch, "dirty" => dirty,
                "agent" => agent, "status" => state_of(&state), "prompt_waiting" => (state == "needs_you") as i64,
                "last_activity" => s.get("last_activity_at"), "source" => "midna", "missed" => 0, "gone_at" => null
            ];
            if let Some(c) = info.s("conversation_id").filter(|c| !c.is_empty()) {
                f.push(("claude_session_id", json!(c)));
            }
            let (live, live_agents) = live_background(&info).map_or((Value::Null, Value::Null), |(n, a)| (json!(n), json!(a)));
            f.push(("live_background", live));
            f.push(("live_background_agents", live_agents));
            if let Some(cur) = &cur {
                board::note_rename(app, Some(cur), Some(&name))?;
                let moved = app.db.update_changed("sessions", &json!(sid), f)?;
                if moved || !wall_age_secs(cur.s("seen_at")).is_some_and(|a| a < SEEN_EVERY_SECS) {
                    app.db.update("sessions", &json!(sid), crate::fields!["seen_at" => now])?;
                }
            } else {
                let mut row = vec![("id", json!(sid)), ("seen_at", json!(now))];
                row.extend(f);
                app.db.insert("sessions", row)?;
            }
            if let Some(t) = board::task_for_session(app, Some(&sid))? {
                if !name.is_empty() && t.s("session_name") != Some(name.as_str()) {
                    board::update_task(app, t.id(), crate::fields!["session_name" => name])?;
                }
                carried_on(app, &board::get_task(app, t.id())?, &sid, &state, st.s("since"))?;
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
        app.debug(format!("midna: {} gone", plural(gone.len() as i64, "terminal")));
    }
    Ok((gone, to_close))
}

/// A task waiting on its terminal (a permission prompt approved there, an API error it got past)
/// whose terminal Midna has seen working since it last asked: it's back at work, with no prompt
/// or turn end to say so. Working since before then is a reading from before the ask, so it stays.
fn carried_on(app: &App, t: &Row, sid: &str, state: &str, since: Option<&str>) -> Result<()> {
    let waiting = t.s("status") == Some("needs") && !t.b("lost") && t.s("session_id") == Some(sid);
    if !waiting || state != "working" || !crate::reports::UNTIL_IT_CARRIES_ON.contains(&t.st("needs_reason").as_str()) {
        return Ok(());
    }
    // When it last asked: each prompt or error logs a question, a second one while it still waits too.
    let asked = app.db.val("SELECT MAX(at) FROM events WHERE task_id = ? AND kind = 'question'", p![t.id()])?;
    if !since.zip(asked.as_str()).is_some_and(|(since, asked)| since > asked) {
        return Ok(());
    }
    board::set_working(app, t, board::MIDNA, Some("Back to work: its terminal carried on"))
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

/// The refs that become links in agent terminals, each opening this build's own page for it. A human
/// task's H<n> opens as its T<n>. J, L, Q and M refs have no page in the app, so they stay text.
pub fn link_patterns(app: &App) -> Vec<String> {
    [(r"(T\d+)", "task/$1"), (r"H(\d+)", "task/T$1"), (r"(G\d+)", "goal/$1"), (r"(B\d+)", "issue/$1"), (r"(R\d+)", "release/$1"), (r"(I\d+)", "idea/$1")]
        .iter()
        .map(|(re, page)| format!(r"\b{re}\b = {} agents", dispatch::page_link(app, page)))
        .collect()
}

/// Midna's caps on one owner's menus.
const MAX_MENUS: usize = 32;
const DETAIL_CHARS: usize = 40;

/// The `tb` beside this daemon, so a menu's run item names the same program as the `tb preview`
/// Midna runs (its `actions` only pass when the programs match).
fn menu_tb(app: &App) -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("tb")))
        .and_then(|tb| std::fs::canonicalize(tb).ok())
        .map(|tb| tb.display().to_string())
        .unwrap_or_else(|| board::tb_program(app))
}

/// The board's menus for its own links (T, G and B pages) and a "Taskboard" section on each open
/// task PR's link. The task menu's items are what Midna shows when `tb preview` gives no actions.
pub fn link_menus(app: &App, tb: &str) -> Vec<Value> {
    let own = |page: &str, letter: &str, icon: &str, more: Vec<Value>| {
        let links = format!(r"^{}({letter}\d+)(?:[/?#]|$)", regex::escape(&dispatch::page_link(app, page)));
        let items: Vec<Value> = [json!({"label": "Copy $1", "copy": "$1"})].into_iter().chain(more).collect();
        json!({"links": links, "own": true, "icon": icon, "title": "$1", "subtitle": links_owner(app), "items": items})
    };
    let focus = json!({"label": "Go to its terminal", "run": [tb, "focus", "$1", "--port", app.cfg.port.to_string()]});
    let backlog = json!({"label": "Open its backlog", "open": format!("{}#/goal/$1?view=backlog", dispatch::page_link(app, ""))});
    let mut menus = vec![
        own("task/", "T", "checklist", vec![json!({"divider": true}), focus]),
        own("goal/", "G", "tag", vec![json!({"divider": true}), backlog]),
        own("issue/", "B", "issue", vec![]),
    ];
    let rows = app
        .db
        .q(
            "SELECT id, title, pr_url FROM tasks WHERE COALESCE(pr_url, '') != '' AND COALESCE(pr_state, '') NOT IN ('MERGED', 'DECLINED') ORDER BY id DESC LIMIT ?",
            p![(MAX_MENUS - menus.len()) as i64],
        )
        .unwrap_or_default();
    let mut prs: Vec<(String, Vec<Value>)> = Vec::new();
    for r in &rows {
        let item = json!({"label": format!("Open T{}", r.id()), "detail": one_line(&r.st("title"), DETAIL_CHARS), "open": dispatch::page_link(app, &format!("task/T{}", r.id()))});
        match prs.iter_mut().find(|(url, _)| *url == r.st("pr_url")) {
            Some((_, items)) => items.push(item),
            None => prs.push((r.st("pr_url"), vec![item])),
        }
    }
    menus.extend(prs.into_iter().map(|(url, items)| {
        json!({"links": format!(r"^{}(?:[/?#]|$)", regex::escape(url.trim_end_matches('/'))), "section": links_owner(app), "items": items})
    }));
    menus
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
                app.info("midna: it doesn't take link patterns yet (links.patterns.register); T, H, G, B, R and I refs stay plain text there");
                true
            }
            Err(e) => {
                app.info(format!("midna: couldn't register link patterns: {e}"));
                false
            }
        }
    };
    register_menus(app, &run);
    if app.cfg.runner {
        remove_old_triggers(app, &run);
    }
    app.shared.lock().midna_links = Some((Instant::now(), if has { Some(run) } else { last }));
}

/// Gives Midna the board's link menus when it's a new Midna run or the menus changed (a task's PR
/// opened or merged). A Midna too old to know `links.actions.register` is logged once a run.
fn register_menus(app: &App, run: &str) {
    let menus = link_menus(app, &menu_tb(app));
    let sent = Value::Array(menus.clone()).to_string();
    if app.shared.lock().midna_menus.as_ref().is_some_and(|(r, m)| r == run && (m.is_empty() || *m == sent)) {
        return;
    }
    let sent = match call_timeout(app, "links.actions.register", json!({"owner": links_owner(app), "menus": menus}), 5.0) {
        Ok(_) => sent,
        Err(MidnaError::Refused(e)) if e.contains("unknown method") => {
            app.info("midna: it doesn't take link menus yet (links.actions.register); its own menu shows on T, G and B links");
            String::new()
        }
        Err(e) => return app.info(format!("midna: couldn't register link menus: {e}")),
    };
    app.shared.lock().midna_menus = Some((run.to_string(), sent));
}

/// The tab statuses the board gives a task's terminal: what it sends, the label, its color and the
/// state Midna sorts and notifies by. Each comes off with the terminal's next prompt.
pub const TAB_STATUSES: [(&str, &str, &str, &str); 7] = [
    ("in_review", "In review", "purple", "done"),
    ("done", "Done", "green", "done"),
    ("question", "Question", "amber", "needs_you"),
    ("alert", "Alert", "red", "needs_you"),
    ("urgent", "Alert", "red", "needs_you"),
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
/// A `held` one goes on with an idle base, so Midna raises no needs-you item for it (`held_tab`).
pub fn tab_call(sid: &str, tab: &str, detail: Option<&str>, held: bool) -> (&'static str, Value) {
    match TAB_STATUSES.iter().find(|(t, ..)| *t == tab) {
        Some((_, label, color, base)) => {
            let base = if held { "idle" } else { base };
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
        return if t.b("failed") {
            Some("failed")
        } else if pr_merged(t) || (t.i("pr_num").is_none() && !has(t.s("pr_url"))) {
            Some("done")
        } else if board::pr_still_open(t) {
            Some(if pr_asked(t) { "question" } else { "in_review" })
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

fn pr_merged(t: &Row) -> bool {
    t.s("pr_phase") == Some("merged") || t.s("pr_state").is_some_and(|s| s.eq_ignore_ascii_case("MERGED"))
}

/// Whether the task's PR visit stopped to ask Morgan something.
fn pr_asked(t: &Row) -> bool {
    jloads_obj(t.s("pr_flow")).get("stopped").is_some_and(|s| s["asked"] == true)
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
    Ok(still_on(app, t, &sid)?.then_some(sid))
}

/// The task's own terminal when a PR visit holds its tab: it still shows what the task was when the
/// visit began, so it follows the task too. None when it's the visit's, or has moved on.
fn own_terminal(app: &App, t: &Row, visit: &str) -> Result<Option<String>> {
    let Some(own) = t.s("session_id").filter(|s| !s.is_empty() && *s != visit) else { return Ok(None) };
    Ok(still_on(app, t, own)?.then(|| own.to_string()))
}

/// Whether terminal `sid` is still on task `t`: no other task there open, or finished since.
fn still_on(app: &App, t: &Row, sid: &str) -> Result<bool> {
    let other = app.db.count(
        "SELECT COUNT(*) FROM tasks WHERE session_id = ? AND id != ? AND (status != 'done' OR (? != '' AND COALESCE(finished_at, '') > ?))",
        p![sid, t.id(), t.st("finished_at"), t.st("finished_at")],
    )?;
    Ok(other == 0)
}

/// Sets a task's terminal tab when its state moved from `before` to `after` changes what it shows.
pub fn task_changed(app: &App, before: &Row, after: &Row) -> Result<()> {
    let now = task_tab(after);
    if now == task_tab(before) || !app.cfg.runner {
        return Ok(());
    }
    let Some(sid) = tab_terminal(app, after)? else { return Ok(()) };
    set_tab(app, &sid, now.unwrap_or(TAB_CLEAR), now.and_then(|n| task_tab_detail(after, n)));
    // The visit asks its questions itself; the task's own tab only follows what isn't one.
    if now != Some("question") {
        if let Some(own) = own_terminal(app, after, &sid)? {
            set_tab(app, &own, now.unwrap_or(TAB_CLEAR), None);
        }
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

/// Whether tab status `tab` waits for the work hours to open before it raises a needs-you item in
/// Midna: a needs_you one outside them, except an urgent alert's.
fn held_tab(app: &App, tab: &str) -> bool {
    tab != "urgent" && TAB_STATUSES.iter().any(|(t, .., base)| *t == tab && *base == "needs_you") && !crate::hours::is_open(app)
}

/// Puts the needs_you base back on the tab statuses held outside the work hours (`held_tab`) once
/// they open, on the terminals still showing them; one taken down meanwhile (a prompt there) or
/// with a newer status queued stays as it is.
fn release_tabs(app: &App) {
    if app.shared.lock().midna_held_tabs.is_empty() || !crate::hours::is_open(app) {
        return;
    }
    let held: Vec<_> = app.shared.lock().midna_held_tabs.drain().collect();
    for (sid, (tab, detail)) in held {
        if app.shared.lock().midna_tabs.iter().any(|(q, ..)| *q == sid) {
            continue;
        }
        let label = TAB_STATUSES.iter().find(|(t, ..)| *t == tab).map(|(_, label, ..)| *label);
        match call_timeout(app, "session.get", json!({"id": sid}), 5.0) {
            Ok(s) if s["custom_status"]["label"].as_str() == label => set_tab(app, &sid, tab, detail),
            Ok(_) | Err(MidnaError::Refused(_)) => {}
            Err(MidnaError::Down(_)) => {
                app.shared.lock().midna_held_tabs.insert(sid, (tab, detail));
            }
        }
    }
}

/// Sends the queued tab statuses, in order. One Midna won't take is logged and dropped; with Midna
/// down they wait. A Midna too old for `session.set_status` is logged once and its tabs show none.
pub fn send_tabs(app: &App) {
    static TOO_OLD: AtomicBool = AtomicBool::new(false);
    release_tabs(app);
    loop {
        let Some((sid, tab, detail)) = app.shared.lock().midna_tabs.pop_front() else { return };
        let held = held_tab(app, tab);
        let (method, params) = tab_call(&sid, tab, detail.as_deref(), held);
        let sent = call_timeout(app, method, params, 5.0);
        let mut s = app.shared.lock();
        match held && sent.is_ok() {
            true => s.midna_held_tabs.insert(sid.clone(), (tab, detail.clone())),
            false => s.midna_held_tabs.remove(&sid),
        };
        drop(s);
        match sent {
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

/// The most tasks kept waiting to send their panel while Midna is away.
const PANELS_KEPT: usize = 200;
/// The most terminals whose last panel is kept, so an unchanged one isn't sent again.
const PANELS_REMEMBERED: usize = 500;

/// The task fields a terminal's panel is read from (`task_panel`), beyond the tab status's.
pub fn panel_field(field: &str) -> bool {
    tab_field(field) || matches!(field, "session_id" | "title" | "goal_id")
}

/// The badge and color a task's panel shows for its state.
pub fn panel_state(t: &Row) -> (&'static str, &'static str) {
    match t.s("status") {
        Some("done") if t.b("failed") => ("Failed", "red"),
        Some("done") if pr_merged(t) => ("Merged", "green"),
        Some("done") if board::pr_still_open(t) => match pr_asked(t) {
            true => ("Question", "amber"),
            false => ("In review", "purple"),
        },
        Some("done") if t.i("pr_num").is_some() || has(t.s("pr_url")) => ("PR closed", "gray"),
        Some("done") => ("Done", "green"),
        Some("needs") => match t.s("needs_reason") {
            Some("question") => ("Question", "amber"),
            Some("lost") => ("Lost", "red"),
            _ => ("Needs you", "amber"),
        },
        Some("working") => ("Working", "teal"),
        Some("queued") if crate::waitsfor::parked(t).is_some() => ("Waiting", "blue"),
        Some("queued") => ("Queued", "gray"),
        _ => ("Planned", "gray"),
    }
}

/// The panel a task puts on its terminals' entries in Midna's Summary: the task and its state, its
/// goal (`goal`, with how many of its tasks are done) and buttons to the task, its PR and the goal's
/// next task (`next`).
pub fn task_panel(app: &App, t: &Row, goal: Option<(&Row, i64, i64)>, next: Option<&Row>) -> Value {
    let tref = rf("task", t.id());
    let (badge, color) = panel_state(t);
    let open = dispatch::page_link(app, &format!("task/{tref}"));
    let mut buttons = vec![json!({"label": "Open task", "open": open})];
    if let Some(pr) = t.s("pr_url").filter(|u| u.starts_with("https://") || u.starts_with("http://")) {
        let label = t.i("pr_num").map(|n| format!("PR #{n}")).unwrap_or_else(|| "Open PR".into());
        buttons.push(json!({"label": label, "open": pr}));
    }
    if let Some(n) = next {
        let nref = rf("task", n.id());
        buttons.push(json!({"label": format!("Next: {nref}"), "open": dispatch::page_link(app, &format!("task/{nref}"))}));
    }
    let mut panel = json!({
        "title": one_line(&format!("{tref} {}", t.st("title")), 120),
        "url": open,
        "badge": badge,
        "color": color,
        "line": format!("{tref} {badge}"),
        "buttons": buttons,
        "app": links_owner(app),
    });
    if let Some((g, done, total)) = goal {
        let gref = rf("goal", g.id());
        panel["subtitle"] = json!(one_line(&format!("{gref} {} · {done}/{total}", g.st("name")), 120));
        panel["subtitle_url"] = json!(dispatch::page_link(app, &format!("goal/{gref}")));
    }
    panel
}

/// The task's panel and the terminals it goes on (none when no terminal shows it).
fn panel_of(app: &App, t: &Row) -> Result<Option<(Vec<String>, Value)>> {
    let sids = panel_terminals(app, t)?;
    if sids.is_empty() {
        return Ok(None);
    }
    let goal = board::find_goal(app, t.i("goal_id"))?;
    let (counts, next) = match &goal {
        Some(g) => (Some(board::goal_counts(app, g.id())?), next_in_goal(&board::goal_tasks(app, g.id())?, t)),
        None => (None, None),
    };
    let goal = goal.as_ref().zip(counts).map(|(g, c)| (g, c["done"].as_i64().unwrap_or(0), c["total"].as_i64().unwrap_or(0)));
    Ok(Some((sids, task_panel(app, t, goal, next.as_ref()))))
}

/// The goal's next task for `t`: the first not done after it in the goal's order, else before it.
fn next_in_goal(tasks: &[Row], t: &Row) -> Option<Row> {
    let at = tasks.iter().position(|r| r.id() == t.id()).unwrap_or(0);
    let open = |r: &&Row| r.id() != t.id() && r.s("status") != Some("done");
    tasks[at..].iter().find(open).or_else(|| tasks[..at].iter().find(open)).cloned()
}

/// The terminals showing task `t` in Midna (running or closed): its own and the ones it worked in,
/// each while it's the task that terminal shows (`terminal_task`).
fn panel_terminals(app: &App, t: &Row) -> Result<Vec<String>> {
    let mut sids: Vec<String> = t.s("session_id").filter(|s| !s.is_empty()).map(|s| vec![s.to_string()]).unwrap_or_default();
    for r in app.db.q("SELECT session_id FROM task_terminals WHERE task_id = ? ORDER BY at DESC LIMIT 5", p![t.id()])? {
        if let Some(s) = r.s("session_id").filter(|s| !s.is_empty() && !sids.iter().any(|x| x == s)) {
            sids.push(s.to_string());
        }
    }
    let mut out = vec![];
    for sid in sids {
        if terminal_task(app, &sid)? == Some(t.id()) {
            out.push(sid);
        }
    }
    Ok(out)
}

/// The task a terminal's panel shows: the open one on it, else the last one it worked on.
fn terminal_task(app: &App, sid: &str) -> Result<Option<i64>> {
    let open = app.db.val("SELECT id FROM tasks WHERE session_id = ? AND status != 'done' ORDER BY updated_at DESC, id DESC LIMIT 1", p![sid])?;
    if let Some(id) = open.as_i64() {
        return Ok(Some(id));
    }
    if let Some(id) = board::get_session(app, Some(sid))?.and_then(|s| s.i("last_task")) {
        return Ok(Some(id));
    }
    Ok(app.db.val("SELECT task_id FROM task_terminals WHERE session_id = ? ORDER BY at DESC LIMIT 1", p![sid])?.as_i64())
}

/// Queues task `id`'s panel for its terminals; the jobs thread sends it.
pub fn queue_panel(app: &App, id: i64) {
    if !app.cfg.runner {
        return;
    }
    let mut s = app.shared.lock();
    if s.midna_panels.contains(&id) {
        return;
    }
    if s.midna_panels.len() >= PANELS_KEPT {
        s.midna_panels.pop_front();
    }
    s.midna_panels.push_back(id);
    drop(s);
    app.notify_jobs();
}

/// Queues a task's panel when its state moved from `before` to `after` changes it, and its goal's
/// other tasks' panels when that changes the goal's progress or next task.
pub fn panel_changed(app: &App, before: &Row, after: &Row) -> Result<()> {
    if !app.cfg.runner {
        return Ok(());
    }
    let shown = |t: &Row| (panel_state(t), t.v("title"), t.v("session_id"), t.v("goal_id"), t.v("pr_url"), t.v("pr_num"));
    if shown(before) == shown(after) {
        return Ok(());
    }
    queue_panel(app, after.id());
    let goals = [before.i("goal_id"), after.i("goal_id")];
    if panel_state(before) == panel_state(after) && goals[0] == goals[1] {
        return Ok(());
    }
    for g in goals.into_iter().flatten() {
        let others = app.db.q(
            "SELECT DISTINCT tt.task_id FROM task_terminals tt JOIN tasks t ON t.id = tt.task_id WHERE t.goal_id = ? AND t.id != ?",
            p![g, after.id()],
        )?;
        for o in others {
            queue_panel(app, o.i0("task_id"));
        }
    }
    Ok(())
}

/// Sends the queued tasks' panels to the Summary entries of their terminals. One Midna won't take is
/// logged (one for a terminal it doesn't know, quietly) and dropped; with Midna down they wait. A
/// Midna too old for `summary.annotate` is logged once.
pub fn send_panels(app: &App) {
    static TOO_OLD: AtomicBool = AtomicBool::new(false);
    loop {
        let Some(id) = app.shared.lock().midna_panels.pop_front() else { return };
        let (sids, panel) = match board::find_task(app, Some(id)).and_then(|t| t.map(|t| panel_of(app, &t)).transpose()) {
            Ok(Some(Some(p))) => p,
            Ok(_) => continue,
            Err(e) => {
                app.info(format!("midna: couldn't build {}'s panel: {e}", rf("task", id)));
                continue;
            }
        };
        let owner = links_owner(app).to_lowercase().replace(' ', "-");
        for sid in sids {
            if app.shared.lock().midna_panels_sent.get(&sid) == Some(&panel) {
                continue;
            }
            match call_timeout(app, "summary.annotate", json!({"id": sid, "owner": owner, "panel": panel}), 5.0) {
                Ok(_) => {
                    let mut s = app.shared.lock();
                    if s.midna_panels_sent.len() >= PANELS_REMEMBERED {
                        s.midna_panels_sent.clear();
                    }
                    s.midna_panels_sent.insert(sid, panel.clone());
                }
                Err(MidnaError::Down(_)) => {
                    app.shared.lock().midna_panels.push_front(id);
                    return;
                }
                Err(MidnaError::Refused(e)) if e.contains("unknown method") => {
                    if !TOO_OLD.swap(true, Ordering::Relaxed) {
                        app.info("midna: it doesn't take Summary panels yet (summary.annotate); closed terminals show no task until it's updated".to_string());
                    }
                }
                // A terminal Midna has dropped from its Summary.
                Err(MidnaError::Refused(e)) if e.starts_with("no terminal") => {}
                Err(MidnaError::Refused(e)) => app.info(format!("midna: couldn't put {}'s panel on {sid}: {e}", rf("task", id))),
            }
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
    crate::slot_keep::sync(app);
    for sid in to_close.into_iter().filter(|_| app.cfg.runner) {
        let cwd = sessions.as_array().and_then(|all| all.iter().find(|s| s.get("id").and_then(|v| v.as_str()) == Some(sid.as_str()))).and_then(|s| s.get("cwd")).and_then(|c| c.as_str());
        if let Err(e) = close(app, &sid, cwd, false) {
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
        if let Err(e) = crate::usage::record(&app) {
            app.info(format!("usage: couldn't keep the reading: {}", e.message));
        }
        app.sleep(app.cfg.intervals.midna_sync);
    }
}

/// How long each look at a terminal that just ended may take: the hook that reports it waits.
const EXIT_DETAIL_TIMEOUT: f64 = 0.3;
const EXIT_LINES: usize = 3;

/// What Midna can tell of a terminal that ended with no reason Claude named: how it exited, how
/// long it ran and its last lines. Straight to its CLI, so a Midna that's down marks nothing.
pub fn exit_detail(app: &App, sid: &str) -> Option<String> {
    let s = call_exe(&app.cfg.midna, "session.get", &json!({"id": sid}), EXIT_DETAIL_TIMEOUT).ok();
    let read = call_exe(&app.cfg.midna, "session.read", &json!({"id": sid, "lines": 20}), EXIT_DETAIL_TIMEOUT).ok();
    describe_exit(s.as_ref(), read.as_ref().and_then(|r| r["text"].as_str()), now_ts())
}

/// `exit 1` or `signal 9 (SIGKILL)`, `ran 12 minutes`, `last lines: a | b`; parts Midna didn't give
/// are left out.
pub fn describe_exit(s: Option<&Value>, text: Option<&str>, now: f64) -> Option<String> {
    let mut parts = vec![];
    if let Some(code) = s.and_then(|s| s["status"]["exit_code"].as_i64()) {
        parts.push(match code {
            129..=192 => format!("signal {}{}", code - 128, signal_name(code - 128).map(|n| format!(" ({n})")).unwrap_or_default()),
            _ => format!("exit {code}"),
        });
    }
    if let Some(at) = s.and_then(|s| s["created_at"].as_str()).and_then(parse_iso) {
        parts.push(format!("ran {}", crate::feed::span((now - at).max(0.0))));
    }
    // The terminal's own words, not the rules and prompt marks drawn around them.
    let lines: Vec<String> = text.unwrap_or("").lines().map(|l| one_line(l, 120)).filter(|l| l.chars().any(|c| c.is_alphanumeric())).collect();
    if !lines.is_empty() {
        parts.push(format!("last lines: {}", lines[lines.len().saturating_sub(EXIT_LINES)..].join(" | ")));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn signal_name(n: i64) -> Option<&'static str> {
    Some(match n {
        1 => "SIGHUP",
        2 => "SIGINT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => return None,
    })
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
                r"\bH(\d+)\b = taskboard://task/T$1 agents",
                r"\b(G\d+)\b = taskboard://goal/$1 agents",
                r"\b(B\d+)\b = taskboard://issue/$1 agents",
                r"\b(R\d+)\b = taskboard://release/$1 agents",
                r"\b(I\d+)\b = taskboard://idea/$1 agents",
            ]
        );
        let mut cfg = crate::config::Config::for_tests(dir.path());
        cfg.page_url = "taskboard-dev://".into();
        let app = App::for_tests(cfg);
        assert_eq!(links_owner(&app), "Taskboard Dev");
        assert_eq!(link_patterns(&app)[3], r"\b(B\d+)\b = taskboard-dev://issue/$1 agents");
    }

    #[test]
    fn needs_you_tabs_go_on_idle_outside_hours_except_urgent_ones() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::for_tests(crate::config::Config::for_tests(dir.path()));
        crate::hours::shut_for_tests(&app);
        assert!(held_tab(&app, "question"));
        assert!(held_tab(&app, "alert"));
        assert!(!held_tab(&app, "urgent"));
        assert!(!held_tab(&app, "done"));
        let (_, held) = tab_call("s1", "question", Some("T1 · Fix: which port?"), true);
        assert_eq!((held["base"].as_str(), held["label"].as_str()), (Some("idle"), Some("Question")));
        let (_, urgent) = tab_call("s1", "urgent", None, false);
        assert_eq!((urgent["base"].as_str(), urgent["label"].as_str()), (Some("needs_you"), Some("Alert")));
        crate::hours::set(&app, &json!({"on": false})).unwrap();
        assert!(!held_tab(&app, "question"));
    }

    #[test]
    fn link_menus_own_pages_and_open_prs() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = crate::config::Config::for_tests(dir.path());
        cfg.page_url = "taskboard-dev://".into();
        let app = App::for_tests(cfg);
        let pr = "https://github.com/o/r/pull/7";
        let a = app.db.insert("tasks", vec![("title", json!("Fix the login page when the token expires overnight")), ("pr_url", json!(pr)), ("pr_state", json!("OPEN"))]).unwrap();
        let b = app.db.insert("tasks", vec![("title", json!("Stacked")), ("pr_url", json!(pr))]).unwrap();
        app.db.insert("tasks", vec![("title", json!("Merged")), ("pr_url", json!("https://github.com/o/r/pull/3")), ("pr_state", json!("MERGED"))]).unwrap();
        app.db.insert("tasks", vec![("title", json!("No PR"))]).unwrap();

        let menus = link_menus(&app, "/Apps/tb");
        assert_eq!(menus.len(), 4);
        let task = &menus[0];
        assert_eq!(task["own"], true);
        assert_eq!(task["icon"], "checklist");
        assert_eq!(task["subtitle"], "Taskboard Dev");
        let links = Regex::new(task["links"].as_str().unwrap()).unwrap();
        assert_eq!(&links.captures("taskboard-dev://task/T42").unwrap()[1], "T42");
        assert!(links.is_match("taskboard-dev://task/T42?tab=notes"));
        assert!(!links.is_match("taskboard://task/T42"));
        assert!(!links.is_match("taskboard-dev://task/T42x"));
        assert_eq!(task["items"][0], json!({"label": "Copy $1", "copy": "$1"}));
        assert_eq!(task["items"][2]["run"], json!(["/Apps/tb", "focus", "$1", "--port", app.cfg.port.to_string()]));
        assert_eq!(menus[1]["icon"], "tag");
        assert_eq!(menus[1]["items"][2]["open"], "taskboard-dev://#/goal/$1?view=backlog");
        assert!(Regex::new(menus[1]["links"].as_str().unwrap()).unwrap().is_match("taskboard-dev://goal/G5"));
        assert_eq!(menus[2]["icon"], "issue");
        assert!(Regex::new(menus[2]["links"].as_str().unwrap()).unwrap().is_match("taskboard-dev://issue/B4"));

        let section = &menus[3];
        assert_eq!(section["section"], "Taskboard Dev");
        assert!(section.get("own").is_none());
        let links = Regex::new(section["links"].as_str().unwrap()).unwrap();
        assert!(links.is_match(pr));
        assert!(links.is_match(&format!("{pr}/files")));
        assert!(!links.is_match(&format!("{pr}1")));
        let items = section["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["label"], format!("Open T{b}"));
        assert_eq!(items[1]["label"], format!("Open T{a}"));
        assert_eq!(items[1]["open"], format!("taskboard-dev://task/T{a}"));
        assert!(items[1]["detail"].as_str().unwrap().chars().count() <= DETAIL_CHARS);
    }

    #[test]
    fn link_menus_stay_under_midnas_cap() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::for_tests(crate::config::Config::for_tests(dir.path()));
        for n in 0..40 {
            app.db.insert("tasks", vec![("title", json!("t")), ("pr_url", json!(format!("https://github.com/o/r/pull/{n}")))]).unwrap();
        }
        assert_eq!(link_menus(&app, "tb").len(), MAX_MENUS);
    }

    fn row(fields: Value) -> Row {
        fields.as_object().unwrap().clone()
    }

    #[test]
    fn panel_states_have_a_badge_and_color() {
        let pr = |extra: Value| {
            let mut t = json!({"status": "done", "pr_num": 9, "pr_url": "https://github.com/o/r/pull/9", "pr_state": "OPEN", "pr_phase": "checks"});
            t.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            row(t)
        };
        let cases = [
            (row(json!({"status": "working"})), ("Working", "teal")),
            (row(json!({"status": "queued"})), ("Queued", "gray")),
            (row(json!({"status": "queued", "context": "{\"parked\": {\"session\": \"s1\"}}"})), ("Waiting", "blue")),
            (row(json!({"status": "needs", "needs_reason": "question"})), ("Question", "amber")),
            (row(json!({"status": "needs", "needs_reason": "lost"})), ("Lost", "red")),
            (row(json!({"status": "needs", "needs_reason": "attention"})), ("Needs you", "amber")),
            (row(json!({"status": "done"})), ("Done", "green")),
            (row(json!({"status": "done", "failed": 1})), ("Failed", "red")),
            (pr(json!({})), ("In review", "purple")),
            (pr(json!({"pr_flow": "{\"stopped\": {\"asked\": true}}"})), ("Question", "amber")),
            (pr(json!({"pr_phase": "merged", "pr_state": "MERGED"})), ("Merged", "green")),
            (pr(json!({"pr_state": "CLOSED"})), ("PR closed", "gray")),
            (pr(json!({"failed": 1})), ("Failed", "red")),
            (row(json!({})), ("Planned", "gray")),
        ];
        for (t, want) in cases {
            assert_eq!(panel_state(&t), want, "{t:?}");
        }
    }

    #[test]
    fn a_task_panel_shows_the_task_its_goal_and_where_to_go_next() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::for_tests(crate::config::Config::for_tests(dir.path()));
        let t = row(json!({"id": 731, "title": "Dropped files  type their path", "status": "done", "pr_num": 88,
                           "pr_url": "https://github.com/o/r/pull/88", "pr_state": "OPEN", "pr_phase": "review"}));
        let g = row(json!({"id": 12, "name": "Drag and drop"}));
        let next = row(json!({"id": 732, "title": "Drop folders"}));
        assert_eq!(
            task_panel(&app, &t, Some((&g, 3, 5)), Some(&next)),
            json!({
                "title": "T731 Dropped files type their path",
                "url": "taskboard://task/T731",
                "badge": "In review",
                "color": "purple",
                "line": "T731 In review",
                "subtitle": "G12 Drag and drop · 3/5",
                "subtitle_url": "taskboard://goal/G12",
                "buttons": [
                    {"label": "Open task", "open": "taskboard://task/T731"},
                    {"label": "PR #88", "open": "https://github.com/o/r/pull/88"},
                    {"label": "Next: T732", "open": "taskboard://task/T732"},
                ],
                "app": "Taskboard",
            })
        );
        let alone = row(json!({"id": 5, "title": "Fix", "status": "working", "pr_url": "file:///tmp/x"}));
        let p = task_panel(&app, &alone, None, None);
        assert_eq!(p["buttons"], json!([{"label": "Open task", "open": "taskboard://task/T5"}]), "only web PR links");
        assert!(p.get("subtitle").is_none() && p.get("subtitle_url").is_none());
    }

    #[test]
    fn the_next_task_is_the_goals_next_open_one() {
        let tasks: Vec<Row> = [(1, "done"), (2, "queued"), (3, "working"), (4, "done"), (5, "queued")]
            .iter()
            .map(|(id, s)| row(json!({"id": id, "status": s})))
            .collect();
        let next = |id: i64| next_in_goal(&tasks, &tasks[(id - 1) as usize]).map(|r| r.id());
        assert_eq!(next(3), Some(5));
        assert_eq!(next(5), Some(2), "wraps to the earliest open one");
        assert_eq!(next(1), Some(2));
        let alone = vec![row(json!({"id": 1, "status": "working"}))];
        assert_eq!(next_in_goal(&alone, &alone[0]), None);
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

    #[test]
    fn an_ended_terminal_says_how_it_exited() {
        let now = parse_iso("2026-10-26T17:12:00Z").unwrap();
        let s = json!({"created_at": "2026-10-26T17:00:00Z", "status": {"state": "exited", "exit_code": 137}});
        let text = "Error: out of memory\n\n  at worker\n────────\n❯\n  Opus 5.5 · $0.63\n";
        assert_eq!(describe_exit(Some(&s), Some(text), now).as_deref(),
                   Some("signal 9 (SIGKILL), ran 12 minutes, last lines: Error: out of memory | at worker | Opus 5.5 · $0.63"));
        let s = json!({"status": {"state": "exited", "exit_code": 1}});
        assert_eq!(describe_exit(Some(&s), None, now).as_deref(), Some("exit 1"));
        assert_eq!(describe_exit(None, Some("───\n❯"), now), None, "nothing Midna knows");
    }
}
