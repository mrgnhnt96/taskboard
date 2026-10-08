//! The owner's own hooks: commands in `hooks.json` that run at each step of the board's flow, shaped like
//! Claude Code's hooks. Every hook whose matcher fits the project runs with the event as JSON on stdin.
//!
//! A step's hooks run just before it, and a hook can stop it (exit 2, or print `{"decision": "block",
//! "reason": "…"}`) or skip it (`{"decision": "skip", …}`), unless the step says why it can't. Events that
//! only report what happened (a task was added, a PR merged) are announced afterwards, off the board's
//! threads, and can't change anything. A stop or skip is always said out loud: in the task's history, and
//! to whoever asked for the step.

use std::path::{Path, PathBuf};
use std::time::Instant;

use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::config::Config;
use crate::util::*;
use crate::{board, proc, waitsfor};

const DEFAULT_TIMEOUT_SECS: f64 = 60.0;
const OUTPUT_LIMIT: usize = 4000;

pub struct Event {
    pub name: &'static str,
    pub when: &'static str,
    /// Its hooks run before the step, so they can stop or skip it. Otherwise it's announced afterwards.
    pub before: bool,
    /// Why a hook can't stop this step (empty: it can).
    pub no_stop: &'static str,
    /// Why a hook can't skip this step (empty: it can).
    pub no_skip: &'static str,
}

pub const ANNOUNCED: &str = "it's announced after it happened";

impl Event {
    pub fn can_stop(&self) -> bool {
        self.before && self.no_stop.is_empty()
    }
    pub fn can_skip(&self) -> bool {
        self.before && self.no_skip.is_empty()
    }
    /// Why a hook can't take `decision` ("block" or "skip") here, if it can't.
    pub fn refuses(&self, decision: &str) -> Option<&'static str> {
        if !self.before {
            return Some(ANNOUNCED);
        }
        let why = if decision == "skip" { self.no_skip } else { self.no_stop };
        if why.is_empty() { None } else { Some(why) }
    }
}

/// A step announced after it happens: its hooks can't change it.
const fn after(name: &'static str, when: &'static str) -> Event {
    Event { name, when, before: false, no_stop: "", no_skip: "" }
}

/// A step whose hooks run first and may stop or skip it, unless it says why not.
const fn step(name: &'static str, when: &'static str) -> Event {
    Event { name, when, before: true, no_stop: "", no_skip: "" }
}

const fn no_stop(e: Event, why: &'static str) -> Event {
    Event { no_stop: why, ..e }
}

const fn no_skip(e: Event, why: &'static str) -> Event {
    Event { no_skip: why, ..e }
}

/// The flow, in order: every event a hook can be added to and when it fires. Steps can be stopped and
/// skipped unless they say why not.
pub const EVENTS: &[Event] = &[
    after("task.created", "A task was added (queued or planned)"),
    after("task.queued", "A task went (back) into the queue: planned → queued, put back after its terminal closed, retried"),
    step("task.starting", "The board is about to open a terminal for the task. Skip: the task is marked done without running"),
    after("task.working", "An agent is working on the task: it started, was picked up, or carried on after an answer"),
    after("task.needs", "The task needs the owner: a question, a lost terminal, an API error (see task.needs_reason)"),
    no_skip(
        step("task.finishing", "The task is about to be marked done (by the agent's tb done or by the owner)"),
        "finishing is the last step before done; let it carry on instead",
    ),
    after("task.done", "The task was marked done"),
    after("task.failed", "The agent gave up on the task"),
    after("pr.opened", "A PR was linked to the task"),
    no_stop(
        step("pr.checks", "The PR's checks are running (on each new push). Skip: they count as passed"),
        "the checks run on the PR's host, not on the board",
    ),
    step("pr.fix", "A check failed; the board is about to bring the agent back to fix it. Skip: the checks count as passed"),
    step("pr.review", "The PR waits for review; the board is about to tell the owner. Skip: it counts as approved"),
    step("pr.comments", "The PR has review comments; the board is about to bring the agent back for them. Skip: they count as answered"),
    no_skip(
        step("pr.merge", "The PR is approved and green; the board is about to bring the agent back to merge it, or tell the owner"),
        "the board can't merge by skipping; merge it (a hook may) and the board sees it merged",
    ),
    after("pr.merged", "The PR was merged"),
    after("pr.declined", "The PR was closed without merging"),
    after("goal.created", "A goal was made"),
    after("goal.paused", "A goal was paused"),
    after("goal.resumed", "A paused goal was resumed"),
    after("goal.archived", "A goal was archived"),
    after("goal.finished", "Every task in the goal is done and every PR is merged or closed"),
];

pub fn event(name: &str) -> Option<&'static Event> {
    EVENTS.iter().find(|e| e.name == name)
}

pub fn known(name: &str) -> bool {
    event(name).is_some()
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HooksFile {
    #[serde(default)]
    pub hooks: std::collections::BTreeMap<String, Vec<Group>>,
}

/// Like Claude Code: a matcher (a regex on the project; empty or `*` for every project) and its hooks.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Group {
    #[serde(default)]
    pub matcher: String,
    #[serde(default)]
    pub hooks: Vec<Hook>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Hook {
    #[serde(rename = "type", default = "command_type")]
    pub kind: String,
    pub command: String,
    /// With args the command runs directly; without, through `sh -c`.
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// Seconds before the hook is stopped.
    #[serde(default)]
    pub timeout: Option<f64>,
}

fn command_type() -> String {
    "command".into()
}

/// `hooks.json` next to `config.toml` (or `TASKBOARD_HOOKS`).
pub fn path(cfg: &Config) -> PathBuf {
    if let Ok(p) = std::env::var("TASKBOARD_HOOKS") {
        if !p.trim().is_empty() {
            return expand_home(&p);
        }
    }
    cfg.config_path.parent().map(Path::to_path_buf).unwrap_or_else(|| cfg.data.clone()).join("hooks.json")
}

pub fn log_path(cfg: &Config) -> PathBuf {
    cfg.data.join("hooks.log")
}

/// Reads `hooks.json` fresh each time, so edits take effect without restarting the board.
pub fn load(cfg: &Config) -> std::result::Result<HooksFile, String> {
    let p = path(cfg);
    if !p.is_file() {
        return Ok(HooksFile::default());
    }
    let text = std::fs::read_to_string(&p).map_err(|e| format!("couldn't read {}: {e}", p.display()))?;
    let file: HooksFile = serde_json::from_str(&text).map_err(|e| format!("{} doesn't parse: {e}", p.display()))?;
    for name in file.hooks.keys() {
        if !known(name) {
            return Err(format!("{} has hooks for \"{name}\", which isn't an event (run `tb hooks` for the list)", p.display()));
        }
    }
    Ok(file)
}

fn matches(matcher: &str, project: &str) -> bool {
    let m = matcher.trim();
    if m.is_empty() || m == "*" {
        return true;
    }
    Regex::new(&format!("^(?:{m})$")).map(|re| re.is_match(project)).unwrap_or(false)
}

/// The hooks for an event on a project, in file order.
pub fn matching(file: &HooksFile, event: &str, project: &str) -> Vec<Hook> {
    file.hooks
        .get(event)
        .into_iter()
        .flatten()
        .filter(|g| matches(&g.matcher, project))
        .flat_map(|g| g.hooks.iter().filter(|h| h.kind == "command").cloned())
        .collect()
}

fn task_json(t: &Row) -> Value {
    let pr = if has(t.s("pr_url")) || t.i("pr_num").is_some() {
        json!({"host": t.v("pr_host"), "repo": t.v("pr_repo"), "num": t.v("pr_num"), "url": t.v("pr_url"),
               "phase": t.v("pr_phase"), "state": t.v("pr_state")})
    } else {
        Value::Null
    };
    json!({
        "id": t.id(), "ref": rf("task", t.id()), "title": t.v("title"), "detail": t.v("detail"),
        "project": t.v("project"), "repo_path": t.v("repo_path"), "branch": waitsfor::branch_of(t),
        "status": t.v("status"), "needs_reason": t.v("needs_reason"), "question": t.v("question"),
        "failed": t.b("failed"), "summary": t.v("summary"), "priority": t.v("priority"),
        "goal": t.i("goal_id").map(|g| rf("goal", g)), "session_id": t.v("session_id"),
        "session_name": t.v("session_name"), "jira_key": t.v("jira_key"), "pr": pr,
    })
}

fn base(app: &App, event: &str) -> Value {
    let e = event_or_panic(event);
    json!({
        "event": event, "at": now_iso(), "board_url": app.cfg.url(), "tb": board::tb_cmd(app),
        "can": {"block": e.can_stop(), "skip": e.can_skip()},
    })
}

fn event_or_panic(name: &str) -> &'static Event {
    event(name).unwrap_or_else(|| panic!("hooks: {name} isn't in EVENTS"))
}

/// What a hook reads on stdin for a task's event.
pub fn payload(app: &App, event: &str, t: &Row, before: Option<&Row>) -> Value {
    let mut v = base(app, event);
    v["from"] = before.map(|b| json!({"status": b.v("status"), "pr_phase": b.v("pr_phase")})).unwrap_or(Value::Null);
    v["task"] = task_json(t);
    v
}

/// What a hook reads on stdin for a goal's event (`task` is the task whose change reached it, if any).
pub fn goal_payload(app: &App, event: &str, g: &Row, t: Option<&Row>) -> Value {
    let mut v = base(app, event);
    v["goal"] = board::goal_dict(app, g).unwrap_or_else(|_| json!({"id": g.id(), "ref": rf("goal", g.id()), "project": g.v("project")}));
    v["task"] = t.map(task_json).unwrap_or(Value::Null);
    v
}

/// The events one change to a task announces, by comparing it before and after. Steps aren't
/// here: their hooks run before the step, where the board takes it.
pub fn events_between(before: &Row, after: &Row) -> Vec<&'static str> {
    let mut out = vec![];
    if before.s("status") != after.s("status") {
        match after.s("status") {
            Some("queued") => out.push("task.queued"),
            Some("working") => out.push("task.working"),
            Some("needs") => out.push("task.needs"),
            Some("done") => out.push(if after.b("failed") { "task.failed" } else { "task.done" }),
            _ => {}
        }
    }
    if before.i("pr_num").is_none() && !has(before.s("pr_url")) && (after.i("pr_num").is_some() || has(after.s("pr_url"))) {
        out.push("pr.opened");
    }
    if before.s("pr_phase") != after.s("pr_phase") {
        if let Some(e) = after.s("pr_phase").and_then(|p| EVENTS.iter().find(|e| e.name.strip_prefix("pr.") == Some(p))) {
            if !e.before {
                out.push(e.name);
            }
        }
    }
    out
}

/// Fields whose change can reach an event; `board::update_task` only looks before and after when one is set.
pub fn watched(field: &str) -> bool {
    matches!(field, "status" | "failed" | "pr_num" | "pr_url" | "pr_phase" | "pr_state")
}

/// Announces a task's step: its hooks run on the hooks thread (inline in tests).
pub fn fire(app: &App, event: &str, t: &Row, before: Option<&Row>) {
    let input = payload(app, event, t, before);
    queue(app, event, &t.st("project"), input, t.s("repo_path"), Some(t.id()));
}

/// Announces a goal's step.
pub fn fire_goal(app: &App, event: &str, g: &Row, t: Option<&Row>) {
    let input = goal_payload(app, event, g, t);
    queue(app, event, &g.st("project"), input, g.s("repo_path"), t.map(|t| t.id()));
}

fn hooks_for(app: &App, event: &str, project: &str) -> Vec<Hook> {
    match load(&app.cfg) {
        Ok(f) => matching(&f, event, project),
        Err(e) => {
            app.info(format!("hooks: {e}"));
            vec![]
        }
    }
}

fn cwd_of(repo: Option<&str>) -> Option<PathBuf> {
    repo.map(PathBuf::from).filter(|p| p.is_dir())
}

fn queue(app: &App, event: &str, project: &str, input: Value, repo: Option<&str>, task_id: Option<i64>) {
    let hooks = hooks_for(app, event, project);
    if hooks.is_empty() {
        return;
    }
    let cwd = cwd_of(repo);
    let event = event.to_string();
    let log = log_path(&app.cfg);
    app.queue_hook(Box::new(move |app: &App| {
        for h in hooks {
            let r = run(&h, &event, &input, cwd.as_deref(), &log);
            report(app, &event, task_id, &h, &r);
        }
    }));
}

fn report(app: &App, event: &str, task_id: Option<i64>, h: &Hook, r: &RunResult) {
    let on = task_id.map(|id| format!(" {}", rf("task", id))).unwrap_or_default();
    app.info(format!("hook {event}{on} {}: {}", h.command, r.line()));
    if let (false, Some(id)) = (r.ok() || r.decision().is_some(), task_id) {
        let _ = note(app, id, &format!("{event} hook `{}` {}", short(&h.command), r.line()));
    }
}

/// A line in the task's history, from "Hook".
pub fn note(app: &App, task_id: i64, text: &str) -> Result<i64> {
    board::log_event(app, task_id, "Hook", "hook", text)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Go,
    Block { reason: String, hook: String },
    Skip { reason: String, hook: String },
}

impl Decision {
    /// "Stopped by hook `cancel-builds.sh`: builds are frozen".
    pub fn said(&self, verb: &str) -> String {
        match self {
            Decision::Go => String::new(),
            Decision::Block { reason, hook } | Decision::Skip { reason, hook } => format!("{verb} by hook `{}`: {reason}", short(hook)),
        }
    }
}

/// Runs a step's hooks now, in order, and returns the first stop or skip. Call it outside a
/// database transaction: a hook may call `tb`, which needs the board.
pub fn gate(app: &App, event: &str, t: &Row, extra: Value) -> Decision {
    let e = event_or_panic(event);
    let hooks = hooks_for(app, event, &t.st("project"));
    if hooks.is_empty() {
        return Decision::Go;
    }
    let mut input = payload(app, event, t, None);
    if let (Some(dst), Some(src)) = (input.as_object_mut(), extra.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    let cwd = cwd_of(t.s("repo_path"));
    let log = log_path(&app.cfg);
    for h in hooks {
        let r = run(&h, event, &input, cwd.as_deref(), &log);
        report(app, event, Some(t.id()), &h, &r);
        let Some((d, reason)) = r.decision() else { continue };
        if let Some(why) = e.refuses(d) {
            let verb = if d == "skip" { "skipped" } else { "stopped" };
            let _ = note(app, t.id(), &format!("{event} hook `{}` asked to {d}, but {event} can't be {verb}: {why}. Carrying on", short(&h.command)));
            continue;
        }
        return if d == "skip" { Decision::Skip { reason, hook: h.command } } else { Decision::Block { reason, hook: h.command } };
    }
    Decision::Go
}

fn short(command: &str) -> String {
    clip(command.lines().next().unwrap_or(""), 80)
}

pub struct RunResult {
    pub code: Option<i32>,
    pub error: Option<String>,
    pub stdout: String,
    pub stderr: String,
    pub ms: u128,
}

impl RunResult {
    pub fn ok(&self) -> bool {
        self.error.is_none() && self.code == Some(0)
    }

    /// Like Claude Code: exit 2 stops the step with stderr as the reason; exit 0 with
    /// `{"decision": "block"|"skip", "reason": …}` on stdout stops or skips it.
    pub fn decision(&self) -> Option<(&'static str, String)> {
        if self.error.is_some() {
            return None;
        }
        let why = |s: &str| {
            let s = one_line(s, 500);
            if s.is_empty() { "no reason given".to_string() } else { s }
        };
        match self.code {
            Some(2) => Some(("block", why(&self.stderr))),
            Some(0) => {
                let v: Value = serde_json::from_str(self.stdout.trim()).ok()?;
                let d = match v["decision"].as_str()? {
                    "block" => "block",
                    "skip" => "skip",
                    _ => return None,
                };
                Some((d, why(v["reason"].as_str().unwrap_or(""))))
            }
            _ => None,
        }
    }

    pub fn line(&self) -> String {
        if let Some((d, reason)) = self.decision() {
            return format!("asked to {d}: {reason}");
        }
        match (&self.error, self.code) {
            (Some(e), _) => e.clone(),
            (None, Some(0)) => format!("ok in {} ms", self.ms),
            (None, code) => {
                let why = self.stderr.lines().rev().find(|l| !l.trim().is_empty()).map(|l| format!(": {}", clip(l.trim(), 200))).unwrap_or_default();
                format!("exited {}{why}", code.map(|c| c.to_string()).unwrap_or_else(|| "on a signal".into()))
            }
        }
    }
}

fn env_str(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Runs one hook now and appends the result to `hooks.log`.
pub fn run(h: &Hook, event: &str, input: &Value, cwd: Option<&Path>, log: &Path) -> RunResult {
    let (program, args) = match &h.args {
        Some(a) => (expand_home(&h.command), a.clone()),
        None => (PathBuf::from("/bin/sh"), vec!["-c".to_string(), h.command.clone()]),
    };
    let (t, g) = (&input["task"], &input["goal"]);
    let project = if t.is_object() { &t["project"] } else { &g["project"] };
    let goal = if g.is_object() { &g["ref"] } else { &t["goal"] };
    let env: Vec<(String, String)> = [
        ("TASKBOARD_EVENT", &json!(event)),
        ("TASKBOARD_TASK", &t["ref"]),
        ("TASKBOARD_GOAL", goal),
        ("TASKBOARD_PROJECT", project),
        ("TASKBOARD_REPO", &t["repo_path"]),
        ("TASKBOARD_BRANCH", &t["branch"]),
        ("TASKBOARD_PR_URL", &t["pr"]["url"]),
        ("TASKBOARD_URL", &input["board_url"]),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), env_str(v)))
    .collect();
    let started = Instant::now();
    let body = serde_json::to_vec(input).unwrap_or_default();
    let timeout = h.timeout.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let res = proc::run_with(&program, &args, cwd, timeout, &env, Some(&body));
    let ms = started.elapsed().as_millis();
    let failed = |e: String| RunResult { code: None, error: Some(e), stdout: String::new(), stderr: String::new(), ms };
    let r = match res {
        Ok(o) => RunResult { code: o.code, error: None, stdout: o.stdout, stderr: o.stderr, ms },
        Err(proc::RunError::TimedOut) => failed(format!("timed out after {timeout}s")),
        Err(proc::RunError::Spawn(e)) => failed(format!("couldn't start: {e}")),
    };
    let entry = json!({
        "at": now_iso(), "event": event, "task": t["ref"], "goal": goal, "project": project, "command": h.command,
        "code": r.code, "error": r.error, "decision": r.decision().map(|(d, why)| json!({"decision": d, "reason": why})),
        "ms": r.ms as u64, "stdout": clip(&r.stdout, OUTPUT_LIMIT), "stderr": clip(&r.stderr, OUTPUT_LIMIT),
    });
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
        use std::io::Write;
        let _ = writeln!(f, "{}", jdumps(&entry));
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(v: Value) -> Row {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn status_and_pr_changes_become_events() {
        let before = row(json!({"status": "working"}));
        let after = row(json!({"status": "done", "failed": 0, "pr_num": 9, "pr_url": "u", "pr_phase": "review"}));
        assert_eq!(events_between(&before, &after), vec!["task.done", "pr.opened"], "pr.review is asked before, at its step");
        let failed = row(json!({"status": "done", "failed": 1}));
        assert_eq!(events_between(&before, &failed), vec!["task.failed"]);
        assert!(events_between(&after, &after).is_empty());
    }

    #[test]
    fn pr_steps_are_left_to_their_gate() {
        let before = row(json!({"pr_phase": "review"}));
        assert!(events_between(&before, &row(json!({"pr_phase": "merge"}))).is_empty());
        assert_eq!(events_between(&row(json!({"pr_phase": "merge"})), &row(json!({"pr_phase": "merged"}))), vec!["pr.merged"]);
    }

    fn result(code: i32, stdout: &str, stderr: &str) -> RunResult {
        RunResult { code: Some(code), error: None, stdout: stdout.into(), stderr: stderr.into(), ms: 1 }
    }

    #[test]
    fn decisions_follow_claude_code() {
        assert_eq!(result(0, "", "").decision(), None);
        assert_eq!(result(2, "", "builds are frozen\n").decision(), Some(("block", "builds are frozen".into())));
        assert_eq!(result(2, "", "").decision(), Some(("block", "no reason given".into())));
        assert_eq!(result(0, r#"{"decision":"skip","reason":"cancelled"}"#, "").decision(), Some(("skip", "cancelled".into())));
        assert_eq!(result(0, "not json", "").decision(), None);
        assert_eq!(result(1, r#"{"decision":"block"}"#, "").decision(), None, "only exit 0 output counts");
    }

    #[test]
    fn steps_can_stop_and_skip_unless_they_say_why_not() {
        for e in EVENTS {
            if e.before {
                assert!(e.can_stop() || e.can_skip(), "{} runs before but can't change anything", e.name);
            } else {
                assert_eq!(e.refuses("block"), Some(ANNOUNCED));
                assert!(e.no_stop.is_empty() && e.no_skip.is_empty(), "{}: announced events need no reasons", e.name);
            }
        }
        assert!(event("pr.merge").unwrap().refuses("skip").is_some());
        assert!(event("pr.merge").unwrap().refuses("block").is_none());
    }

    #[test]
    fn matcher_is_a_whole_project_regex() {
        assert!(matches("", "web"));
        assert!(matches("*", "web"));
        assert!(matches("web|api", "api"));
        assert!(!matches("web", "webapp"));
        assert!(matches("web.*", "webapp"));
    }

    #[test]
    fn unknown_events_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::for_tests(dir.path());
        std::fs::write(path(&cfg), r#"{"hooks": {"task.finished": []}}"#).unwrap();
        assert!(load(&cfg).unwrap_err().contains("task.finished"));
    }
}
