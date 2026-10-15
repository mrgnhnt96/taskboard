//! `tb hook <Event>`: the Claude Code hooks. They report to the board and never get in Claude's way:
//! outside a Midna terminal they do nothing, they always exit 0, and they print only valid hook JSON.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::client::{self, CallError};
use taskboardd::comments;
use taskboardd::config::Config;
use taskboardd::steps::{from_listing, Before, Step};

const CONTEXT_EVENTS: &[&str] = &["SessionStart", "UserPromptSubmit"];
const WAITING_NOTIFICATIONS: &[&str] = &["permission_prompt", "idle_prompt"];
const CONTINUING_END_REASONS: &[&str] = &["clear", "resume"];
const GIT_TIMEOUT: f64 = 0.3;
const SLACK: f64 = 0.35;
const SESSION_END_POST_CAP: f64 = 0.8;
const MAX_CONTEXT: usize = 9900;
/// A prompt is sent whole up to what the board keeps of one; a longer one keeps its start and its end.
#[cfg(test)]
const MAX_PROMPT: usize = taskboardd::board::PROMPT_FULL_KEEP;
const MAX_LAST_MESSAGE: usize = 2000;
const MAX_OUTPUT: usize = 1000;

/// A command that opens a pull request: `gh pr create`, `glab mr create`, or a POST to GitHub's
/// `…/pulls` or Bitbucket's `…/pullrequests` (through `gh api`, `tb api` or curl).
static PR_CLI_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b(?:gh\s+pr\s+create|glab\s+mr\s+create)\b").unwrap());
static PR_API_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"/(?:pulls|pullrequests)/?["']?(?:\s|$)"#).unwrap());
static POST_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:^|\s)(?:-d|--data\S*|-f|-F|--field|--raw-field|--input|-X\s*POST|--method\s+POST)(?:\s|=|$)").unwrap());
/// GitLab's: the board can't open a merge request there, so the agent still does.
static GITLAB_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bglab\s+mr\s+create\b").unwrap());
/// An MCP tool that opens a pull request (GitHub's, Bitbucket's, …).
static PR_TOOL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^mcp__.*(?:create_?pull_?request|create_pr$|pull_?request_?create)").unwrap());

fn opens_pr(tool: &str, input: &Value) -> bool {
    if tool != "Bash" {
        return PR_TOOL_RE.is_match(tool);
    }
    let cmd = input["command"].as_str().unwrap_or("");
    PR_CLI_RE.is_match(cmd) || (PR_API_RE.is_match(cmd) && POST_RE.is_match(cmd))
}

static PUSH_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bgit\b(?:\s+(?:-[Cc]\s+\S+|--?[\w-]+(?:=\S+)?))*\s+push\b").unwrap());

/// A git command that changes a repository: a commit, a push, a new worktree, or moving its checkout
/// to another branch or commit.
static GIT_WORK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\bgit\b(?:\s+(?:-[Cc]\s+\S+|--?[\w-]+(?:=\S+)?))*\s+(?:commit|push|worktree\s+add|checkout|switch|merge|rebase|reset|cherry-pick|revert)\b").unwrap()
});
static GIT_DIR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s-C\s+(\S+)").unwrap());
static CD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:^|&&|;|\|\||\()\s*(?:cd|pushd)\s+([^\s;&|)]+)").unwrap());

/// Where a tool call changes a repository: the file an edit names; for a git command that changes
/// one (`GIT_WORK_RE`) or a PR opening, the folder it runs in (`git -C <dir>`, else the last `cd <dir>`
/// or `pushd <dir>` before it, else the cwd). None for any other call.
fn work_place(tool: &str, input: &Value, cwd: &str) -> Option<PathBuf> {
    let at = |p: &str| {
        let p = p.trim_matches(['"', '\'']);
        let p = match (p.strip_prefix("~/"), std::env::var("HOME")) {
            (Some(rest), Ok(home)) => format!("{home}/{rest}"),
            _ => p.to_string(),
        };
        let p = Path::new(&p);
        if p.is_absolute() { p.to_path_buf() } else { Path::new(cwd).join(p) }
    };
    if EDIT_TOOLS.contains(&tool) {
        return input["file_path"].as_str().or(input["notebook_path"].as_str()).filter(|f| !f.is_empty()).map(at);
    }
    if tool != "Bash" {
        return None;
    }
    let cmd = input["command"].as_str().unwrap_or("");
    let git = GIT_WORK_RE.find(cmd);
    if git.is_none() && !opens_pr(tool, input) {
        return None;
    }
    if let Some(dir) = git.and_then(|m| GIT_DIR_RE.captures(m.as_str())) {
        return Some(at(&dir[1]));
    }
    let before = &cmd[..git.or_else(|| PR_CLI_RE.find(cmd)).map_or(cmd.len(), |m| m.start())];
    Some(CD_RE.captures_iter(before).last().map_or_else(|| PathBuf::from(cwd), |c| at(&c[1])))
}

/// A folder's repository: its shared git folder (one for all of the repository's worktrees) and its
/// top folder. None outside a repository.
fn repo_of(place: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut dir = place;
    while !dir.is_dir() {
        dir = dir.parent()?;
    }
    let o = std::process::Command::new("git").arg("-C").arg(dir).args(["rev-parse", "--path-format=absolute", "--git-common-dir", "--show-toplevel"]).output().ok().filter(|o| o.status.success())?;
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    let mut lines = out.lines().map(|l| PathBuf::from(l.trim())).map(|p| p.canonicalize().unwrap_or(p));
    Some((lines.next()?, lines.next()?))
}

/// Why the board refuses work at `place` for this task, whose repository is `home`: the work is in
/// another repository, so the board would show the task as tracked while it happens beside it. A
/// repository that holds the task's (a home-folder dotfiles one) isn't another project's.
fn elsewhere(place: &Path, home: &Path, task: &str, project: &str) -> Option<(String, PathBuf)> {
    let (common, top) = repo_of(place)?;
    let (home_common, home_top) = repo_of(home)?;
    if common == home_common || home_top.starts_with(&top) {
        return None;
    }
    let shown = top.display();
    Some((
        format!(
            "{task} is {project}'s work, but this changes {shown}, another repository. The board doesn't track {task}'s work there, so it \
             refuses it: the task would show as in progress while its work happens beside the board. Keep {task}'s work in {project}. \
             Work in {shown} needs its own task in that repository's project: tb found it, or tb task new \"<title>\" --project <its project> \
             (no project for it yet? midna project add {shown}). If {task} can't be done without it, tb fail \"<why>\"."
        ),
        top,
    ))
}

/// `PreToolUse`: refuses an edit, a git commit, push or new worktree, or a PR opening in a repository
/// this terminal's task doesn't own, and tells the owner. No task, no board, or no repository: carry on.
fn elsewhere_refused(payload: &Value, session: &str, cfg: &Config, cwd: &str) -> bool {
    let tool = payload["tool_name"].as_str().unwrap_or("");
    let Some(place) = work_place(tool, &payload["tool_input"], cwd) else { return false };
    if repo_of(&place).is_none() {
        return false;
    }
    let Ok(v) = client::request(cfg, "GET", &format!("/steps?session={session}"), None, hook_timeout()) else { return false };
    let Some(task) = v["task"].as_str() else { return false };
    let vars = &v["vars"];
    let Some(home) = ["repo", "worktree"].iter().filter_map(|k| vars[*k].as_str()).find(|p| !p.is_empty()) else { return false };
    let project = vars["project"].as_str().unwrap_or("its project");
    let Some((why, top)) = elsewhere(&place, Path::new(home), task, project) else { return false };
    deny(&why);
    let text = format!("{task} tried to work in {}, a repository it doesn't own ({task} is {project}'s). The board refused it.", top.display());
    let _ = client::request(cfg, "POST", "/alerts", Some(&json!({"text": text, "key": format!("elsewhere:{task}"), "task": task})), hook_timeout());
    true
}

/// The head of the task's own checkout (`{worktree}` and `{branch}` in a `GET /steps` answer): its
/// branch's tip there, else the checkout's head.
fn own_head(vars: &Value) -> Option<String> {
    let dir = vars["worktree"].as_str().filter(|d| !d.is_empty() && Path::new(d).is_dir())?;
    let rev = |r: &str| {
        let o = std::process::Command::new("git").args(["-C", dir, "rev-parse", "--verify", "-q", &format!("{r}^{{commit}}")]).output().ok().filter(|o| o.status.success())?;
        Some(String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|h| !h.is_empty())
    };
    vars["branch"].as_str().filter(|b| !b.is_empty()).and_then(|b| rev(&format!("refs/heads/{b}"))).or_else(|| rev("HEAD"))
}

/// The refusal for opening a PR from the terminal of a task (its `GET /steps` listing) that ends in one:
/// the board opens that PR itself. None when the task ends without a PR.
fn board_opens_pr(steps: &Value, tb: &str) -> Option<String> {
    if steps["ships_pr"] != true {
        return None;
    }
    let task = steps["task"].as_str().unwrap_or("This task");
    Some(format!(
        "{task} ends in a PR, and the board opens it: write the description to a file and finish with \
         {tb} submit \"<summary>\" --pr-body <file>. It checks the description and that the branch is pushed and \
         rebased, then opens the PR. ({tb} submit --pr <link> is only for a PR that was already open before the task.)"
    ))
}

/// `PreToolUse`: refuses a command that opens a PR on a task that ends in one (the board opens it, at
/// `tb submit --pr-body`; a GitLab merge request is still the agent's to open), holds one on any other
/// task while its steps for before the PR
/// (config.toml's `[[steps]]`) aren't recorded, and a push to an open PR while a per-head step hasn't
/// passed on the commit being pushed, and work in a repository the task doesn't own (`elsewhere_refused`).
/// Anything else, no task, or no board: carry on. True when it refused the call.
fn pre_tool_use(payload: &Value, session: &str) -> bool {
    let tool = payload["tool_name"].as_str().unwrap_or("");
    let cfg = client::config();
    let cwd = payload["cwd"].as_str().filter(|c| !c.is_empty()).map(|c| c.to_string()).or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string()));
    if EDIT_TOOLS.contains(&tool) {
        // The comment guard (config.toml's [comments]): read locally, no board needed.
        if let Some(reason) = comments::edit_refusal(&cfg, tool, &payload["tool_input"]) {
            deny(&reason);
            return true;
        }
        return elsewhere_refused(payload, session, &cfg, cwd.as_deref().unwrap_or(""));
    }
    if elsewhere_refused(payload, session, &cfg, cwd.as_deref().unwrap_or("")) {
        return true;
    }
    let opening = opens_pr(tool, &payload["tool_input"]);
    let pushing = tool == "Bash" && PUSH_RE.is_match(payload["tool_input"]["command"].as_str().unwrap_or(""));
    if !opening && !pushing {
        return false;
    }
    if opening {
        if let Some(reason) = cwd.as_deref().and_then(|d| comments::branch_refusal(&cfg, d, "opening the PR")) {
            deny(&reason);
            return true;
        }
    }
    let what = if opening { "opening the PR" } else { "pushing to the open PR" };
    let Ok(mut v) = client::request(&cfg, "GET", &format!("/steps?session={session}"), None, hook_timeout()) else { return false };
    if opening && !GITLAB_RE.is_match(payload["tool_input"]["command"].as_str().unwrap_or("")) {
        if let Some(reason) = board_opens_pr(&v, &tb_path()) {
            deny(&reason);
            return true;
        }
    }
    if let Some(why) = v["aim_outside"].as_str() {
        deny(&format!("Not yet: {what} waits for the task's steps, and they can't be judged. {why}"));
        return true;
    }
    // Per-head steps are judged on the task's own checkout, not on whichever folder this call runs in.
    if let Some(head) = own_head(&v["vars"]).filter(|h| v["head"].as_str() != Some(h.as_str())) {
        match client::request(&cfg, "GET", &format!("/steps?session={session}&head={head}"), None, hook_timeout()) {
            Ok(at) => v = at,
            Err(_) => return false,
        }
    }
    let vars = serde_json::from_value(v["vars"].clone()).unwrap_or_default();
    let left: Vec<Step> = from_listing(&v)
        .into_iter()
        .filter(|(s, done)| s.before == Before::Pr && !done && (opening || (s.per_head && v["pr_open"] == true)))
        .map(|(s, _)| s.filled(&vars))
        .collect();
    if left.is_empty() {
        return false;
    }
    deny(&taskboardd::steps::refusal(&tb_path(), what, &left));
    true
}

/// `PreToolUse`: refuses a tool call that reads the daemon's app token (`taskboardd::apptoken`), which
/// is what makes a request the owner's click in the app.
fn token_refused(payload: &Value) -> bool {
    let tool = payload["tool_name"].as_str().unwrap_or("");
    let cwd = payload["cwd"].as_str().filter(|c| !c.is_empty()).map(std::path::Path::new);
    if !taskboardd::apptoken::tool_reaches(tool, &payload["tool_input"], &client::config().data, cwd) {
        return false;
    }
    deny(&taskboardd::apptoken::refusal());
    true
}

const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];
const SUBAGENT_TOOLS: &[&str] = &["Agent", "Task"];
/// Tool calls that can change files without naming them: the board stamps the checkout around each
/// one, so a no-task Stop counts only what changed while the agent's own calls ran.
const WINDOW_TOOLS: &[&str] = &["Bash", "Agent", "Task"];

/// The id Claude gives a tool call, which its PreToolUse and PostToolUse both carry; failing that, one
/// made from the call itself, which the board can match to the transcript's copy of the call.
fn tool_call_id(payload: &Value) -> String {
    if let Some(id) = payload["tool_use_id"].as_str().filter(|id| !id.is_empty()) {
        return id.to_string();
    }
    taskboardd::transcript::call_key(payload["tool_name"].as_str().unwrap_or(""), &payload["tool_input"])
}

/// Whether the hooks.json Claude loaded for this plugin sends `ToolEnd`, so each window this hook
/// opens is closed: the plugin's folder is its installed version's, the one Claude read.
fn hooks_send_tool_end(root: Option<&std::path::Path>) -> bool {
    let Some(text) = root.and_then(|r| std::fs::read_to_string(r.join("hooks").join("hooks.json")).ok()) else { return false };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return false };
    v["hooks"]["PostToolUse"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|m| m["hooks"].as_array().into_iter().flatten())
        .any(|h| h["args"].as_array().is_some_and(|a| a.iter().any(|x| x == "ToolEnd")) || h["command"].as_str().is_some_and(|c| c.contains("ToolEnd")))
}

/// The report that opens (`hook.tool_start`) or closes (`hook.tool_end`) a tool call's window on the
/// board, with the checkout's stamp at that moment. None for a call that can't change files unnamed.
fn tool_window_body(payload: &Value, session: &str, event: &str) -> Option<Value> {
    let tool = payload["tool_name"].as_str().unwrap_or("");
    if !WINDOW_TOOLS.contains(&tool) {
        return None;
    }
    let cwd = payload["cwd"].as_str().filter(|c| !c.is_empty()).map(|c| c.to_string()).or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string())).unwrap_or_default();
    let mut body = client::base_body(event, session, payload["session_id"].as_str().unwrap_or(""), &cwd, Value::Null);
    body["tool"] = json!(tool);
    body["tool_use_id"] = json!(tool_call_id(payload));
    body["background"] = json!(payload["tool_input"]["run_in_background"] == true);
    if event == "hook.tool_start" {
        body["ends"] = json!(hooks_send_tool_end(plugin_root().as_deref()));
    }
    body["tree"] = client::tree_stamp(&cwd, GIT_TIMEOUT);
    Some(body)
}

/// Sends a tool call's window. One the board doesn't hear about only leaves the Stop to its older
/// guess, so nothing is spooled.
fn tool_window(payload: &Value, session: &str, event: &str) {
    let start = Instant::now();
    let Some(body) = tool_window_body(payload, session, event) else { return };
    let left = (hook_timeout() + SLACK - start.elapsed().as_secs_f64()).min(hook_timeout());
    if left > 0.05 {
        let _ = client::request(&client::config(), "POST", "/report", Some(&body), left);
    }
}

fn deny(reason: &str) {
    let o = json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": reason}});
    let mut so = std::io::stdout();
    let _ = so.write_all(o.to_string().as_bytes());
    let _ = so.flush();
}

static GIT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bgit\b(?:\s+(?:-[Cc]\s+\S+|--?[\w-]+(?:=\S+)?))*\s+(commit|push)\b").unwrap());

fn board_event(hook: &str) -> Option<&'static str> {
    Some(match hook {
        "SessionStart" => "hook.session_start",
        "UserPromptSubmit" => "hook.prompt",
        "Stop" => "hook.stop",
        "StopFailure" => "hook.api_error",
        "PreCompact" => "hook.pre_compact",
        "PostToolUse" => "hook.commit",
        "SessionEnd" => "hook.session_end",
        "Notification" => "hook.attention",
        _ => return None,
    })
}

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// The last `n` characters of `s`.
fn clip_end(s: &str, n: usize) -> String {
    let c: Vec<char> = s.chars().collect();
    c[c.len().saturating_sub(n)..].iter().collect()
}

fn clip_middle(s: &str, n: usize) -> String {
    let c: Vec<char> = s.chars().collect();
    if c.len() <= n {
        return s.to_string();
    }
    let head = n / 2;
    let tail = n - head - 3;
    format!("{}\n…\n{}", c[..head].iter().collect::<String>(), c[c.len() - tail..].iter().collect::<String>())
}

fn tool_output(resp: &Value) -> String {
    match resp {
        Value::Object(o) => [o.get("stdout"), o.get("stderr")]
            .iter()
            .filter_map(|x| x.and_then(|v| v.as_str()))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

pub fn hook_timeout() -> f64 {
    std::env::var("TASKBOARD_HOOK_TIMEOUT").ok().and_then(|v| v.parse::<f64>().ok()).filter(|v| *v > 0.0).unwrap_or(1.0)
}

fn plugin_root() -> Option<std::path::PathBuf> {
    std::env::var("CLAUDE_PLUGIN_ROOT").ok().map(std::path::PathBuf::from).filter(|p| p.is_dir())
}

fn tb_path() -> String {
    if let Some(r) = plugin_root() {
        return r.join("bin").join("tb").to_string_lossy().to_string();
    }
    std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "tb".into())
}

/// When Taskboard has a GitHub or Bitbucket account, has this session's git ask `tb git-credential`
/// first (through Claude Code's `CLAUDE_ENV_FILE`), so an agent on a task pushes with that account.
/// Only this session's environment changes; `~/.gitconfig` doesn't.
fn git_env(cfg: &Config, tb: &str) {
    let Some(file) = std::env::var_os("CLAUDE_ENV_FILE").filter(|f| !f.is_empty()) else { return };
    if !taskboardd::accounts::has_git_account(cfg) {
        return;
    }
    let Some(env) = crate::gitcred::session_env(tb) else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = f.write_all(env.as_bytes());
    }
}

fn plugin_version() -> String {
    plugin_root()
        .and_then(|r| std::fs::read_to_string(r.join(".claude-plugin").join("plugin.json")).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["version"].as_str().map(|s| s.to_string()))
        .unwrap_or_default()
}

fn hello_marker(cfg: &Config) -> std::path::PathBuf {
    cfg.data.join("plugin-hello")
}

fn say_hello(cfg: &Config, base: &Value, version: &str, path: &str, left: f64) {
    let want = format!("{version}\n{path}");
    if left <= 0.05 || std::fs::read_to_string(hello_marker(cfg)).map(|t| t == want).unwrap_or(false) {
        return;
    }
    let mut hello = base.clone();
    hello["event"] = json!("tb.hello");
    hello["tb_path"] = json!(path);
    hello["plugin_version"] = json!(version);
    if client::request(cfg, "POST", "/report", Some(&hello), left).is_ok() {
        let _ = std::fs::create_dir_all(&cfg.data);
        let tmp = hello_marker(cfg).with_extension(format!("{}.tmp", std::process::id()));
        if std::fs::write(&tmp, want).is_ok() {
            let _ = std::fs::rename(&tmp, hello_marker(cfg));
        }
    }
}

/// A `claude -p` (or Agent SDK) run: Claude Code marks it with an `sdk-*` entrypoint. One an agent
/// starts in its terminal inherits `MIDNA_SESSION`, but it isn't that terminal's conversation.
fn headless(entrypoint: Option<&str>) -> bool {
    entrypoint.is_some_and(|e| e.starts_with("sdk"))
}

pub fn run(event_arg: Option<&str>) -> i32 {
    let start = Instant::now();
    // A headless run reports nothing for the terminal it ran in; it only meets the token guard.
    let session = if headless(std::env::var("CLAUDE_CODE_ENTRYPOINT").ok().as_deref()) { String::new() } else { std::env::var("MIDNA_SESSION").unwrap_or_default() };
    // The app token guard runs in every terminal, Midna's or not; everything else only in Midna's.
    if session.is_empty() && event_arg != Some("PreToolUse") {
        return 0;
    }
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let payload: Value = serde_json::from_str(&raw).ok().filter(|v: &Value| v.is_object()).unwrap_or(json!({}));
    let hook = event_arg.map(|s| s.to_string()).filter(|s| !s.is_empty()).or_else(|| payload["hook_event_name"].as_str().map(|s| s.to_string())).unwrap_or_default();
    if hook == "PreToolUse" {
        let tool = payload["tool_name"].as_str().unwrap_or("");
        // A subagent's prompt isn't a read: its own tool calls meet the guard.
        if !SUBAGENT_TOOLS.contains(&tool) && token_refused(&payload) {
            return 0;
        }
        if session.is_empty() {
            return 0;
        }
        if !pre_tool_use(&payload, &session) {
            tool_window(&payload, &session, "hook.tool_start");
        }
        return 0;
    }
    if session.is_empty() {
        return 0;
    }
    if hook == "ToolEnd" {
        // `tb hook ToolEnd`, from PostToolUse and PostToolUseFailure: the call's window closes.
        tool_window(&payload, &session, "hook.tool_end");
        return 0;
    }
    let Some(event) = board_event(&hook) else { return 0 };
    let s = |k: &str| payload[k].as_str().unwrap_or("").to_string();
    let mut extra = serde_json::Map::new();
    match hook.as_str() {
        "PostToolUse" => {
            if payload["tool_name"] != "Bash" {
                return 0;
            }
            let command = payload["tool_input"]["command"].as_str().unwrap_or("");
            if !GIT_RE.is_match(command) {
                return 0;
            }
            extra.insert("command".into(), json!(clip(command, 2000)));
            extra.insert("output".into(), json!(clip_middle(&tool_output(&payload["tool_response"]), MAX_OUTPUT)));
        }
        "Notification" => {
            let kind = s("notification_type");
            if !WAITING_NOTIFICATIONS.contains(&kind.as_str()) {
                return 0;
            }
            extra.insert("message".into(), json!(s("message")));
            extra.insert("notification_type".into(), json!(kind));
            extra.insert("transcript_path".into(), json!(s("transcript_path")));
            if !s("title").is_empty() {
                extra.insert("title".into(), json!(s("title")));
            }
        }
        "SessionEnd" => {
            let reason = if s("reason").is_empty() { "other".to_string() } else { s("reason") };
            if CONTINUING_END_REASONS.contains(&reason.as_str()) {
                return 0;
            }
            extra.insert("reason".into(), json!(reason));
        }
        "UserPromptSubmit" => {
            // The prompt, and that its only cut is the board's own: a "…" at its end is the owner's.
            if let Value::Object(m) = taskboardd::startword::hook_prompt(&s("prompt")) {
                extra.extend(m);
            }
        }
        "Stop" => {
            extra.insert("last_message".into(), json!(clip(&s("last_assistant_message"), MAX_LAST_MESSAGE)));
            // Its end too, where the agent asks whether to go on ("Want me to run G4?").
            extra.insert("last_message_end".into(), json!(clip_end(&s("last_assistant_message"), taskboardd::startword::REPLY_END_KEEP)));
            extra.insert("transcript_path".into(), json!(s("transcript_path")));
            extra.insert("stop_hook_active".into(), json!(payload["stop_hook_active"] == true));
        }
        "StopFailure" => {
            extra.insert("error".into(), json!(s("error")));
            extra.insert("error_details".into(), json!(clip(&s("error_details"), MAX_OUTPUT)));
            extra.insert("last_message".into(), json!(clip(&s("last_assistant_message"), MAX_OUTPUT)));
        }
        "PreCompact" => {
            extra.insert("trigger".into(), json!(s("trigger")));
        }
        _ => {}
    }
    let cfg = client::config();
    let timeout = hook_timeout();
    let deadline = start + Duration::from_secs_f64(timeout + SLACK);
    let left = || deadline.saturating_duration_since(Instant::now()).as_secs_f64().min(timeout);
    let cwd = if s("cwd").is_empty() { std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default() } else { s("cwd") };
    let (version, path) = (plugin_version(), tb_path());
    if hook == "SessionStart" {
        extra.insert("source".into(), json!(s("source")));
        extra.insert("tb_path".into(), json!(path));
        extra.insert("plugin_version".into(), json!(version));
    }
    if hook == "UserPromptSubmit" || hook == "Stop" {
        extra.insert("tree".into(), client::tree_stamp(&cwd, GIT_TIMEOUT));
    }
    if hook == "UserPromptSubmit" {
        // This hook stamps each Bash and subagent call (`tool_window`), so the board needn't guess.
        extra.insert("tool_windows".into(), json!(true));
    }
    let git = client::git_info(&cwd, GIT_TIMEOUT);
    let base = client::base_body(event, &session, &s("session_id"), &cwd, git);
    let mut body = base.clone();
    for (k, v) in extra {
        body[k] = v;
    }
    let mut post_timeout = left();
    if hook == "SessionEnd" {
        post_timeout = post_timeout.min(SESSION_END_POST_CAP);
    }
    let resp = if post_timeout > 0.05 {
        match client::request(&cfg, "POST", "/report", Some(&body), post_timeout) {
            Ok(v) => Some(v),
            Err(CallError::Refused(..)) => Some(json!({})),
            Err(CallError::Unreachable(_) | CallError::TimedOut(_)) => None,
        }
    } else {
        None
    };
    let Some(resp) = resp else {
        let _ = client::spool_write(&cfg, &body);
        return 0;
    };
    if hook == "SessionStart" {
        say_hello(&cfg, &base, &version, &path, left());
        git_env(&cfg, &path);
    }
    let context = resp["context"].as_str().filter(|c| !c.trim().is_empty()).unwrap_or("").to_string();
    let deliver = &resp["deliver"];
    let ids: Vec<Value> = if CONTEXT_EVENTS.contains(&hook.as_str()) || hook == "Stop" {
        deliver["ids"].as_array().cloned().unwrap_or_default()
    } else {
        vec![]
    };
    let mut message = if ids.is_empty() { String::new() } else { deliver["text"].as_str().unwrap_or("").to_string() };
    if hook == "Stop" && message.is_empty() {
        // The board's own reason to keep the turn going (code changed with no task to track it).
        message = resp["block"].as_str().map(|b| b.trim().to_string()).unwrap_or_default();
    }
    let out = if hook == "Stop" && !message.is_empty() {
        Some(json!({"decision": "block", "reason": clip(&message, MAX_CONTEXT)}))
    } else if CONTEXT_EVENTS.contains(&hook.as_str()) && (!context.is_empty() || !message.is_empty()) {
        let both = match (message.is_empty(), context.is_empty()) {
            (false, false) => format!("{message}\n\n{context}"),
            (false, true) => message.clone(),
            _ => context.clone(),
        };
        Some(json!({"hookSpecificOutput": {"hookEventName": hook, "additionalContext": clip(&both, MAX_CONTEXT)}}))
    } else {
        None
    };
    if let Some(o) = out {
        let mut so = std::io::stdout();
        let _ = so.write_all(o.to_string().as_bytes());
        let _ = so.flush();
    }
    if !ids.is_empty() {
        let mut ack = base.clone();
        ack["event"] = json!("hook.delivered");
        ack["ids"] = Value::Array(ids);
        let l = left();
        let sent = l > 0.05 && !matches!(client::request(&cfg, "POST", "/report", Some(&ack), l), Err(CallError::Unreachable(_) | CallError::TimedOut(_)));
        if !sent {
            let _ = client::spool_write(&cfg, &ack);
        }
    }
    0
}

fn status_text(v: &Value) -> String {
    let mut parts = vec![];
    if let Some(m) = v["model"]["display_name"].as_str() {
        parts.push(m.to_string());
    }
    if let Some(d) = v["workspace"]["current_dir"].as_str().or(v["cwd"].as_str()) {
        parts.push(d.trim_end_matches('/').rsplit('/').next().unwrap_or(d).to_string());
    }
    if let Some(p) = v["rate_limits"]["five_hour"]["used_percentage"].as_f64() {
        parts.push(format!("5h {}%", p.round() as i64));
    }
    parts.join(" · ")
}

/// `tb statusline`: saves Claude Code's status-line input (with its rate limits) for the board and prints
/// a short status line; with `--pass` it prints the input unchanged, to chain into another command.
pub fn statusline(pass: bool) -> i32 {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let parsed = serde_json::from_str::<Value>(&raw);
    let mut so = std::io::stdout();
    let shown = if pass { raw.clone() } else { parsed.as_ref().map(status_text).unwrap_or_default() };
    let _ = so.write_all(shown.as_bytes());
    let _ = so.flush();
    let Ok(v) = parsed else { return 0 };
    let sid = v["session_id"].as_str().unwrap_or("").to_string();
    let name = std::env::var("MIDNA_SESSION").ok().filter(|s| !s.is_empty()).unwrap_or(sid);
    if name.is_empty() || name.contains('/') {
        return 0;
    }
    let cfg = client::config();
    let _ = save_private(&cfg.statusline_dir, &name, &raw);
    0
}

/// Writes one session's status JSON to `<dir>/<name>.json`. It holds the session's folder, cost and rate
/// limits, so the folder is kept 0700 and the file 0600.
fn save_private(dir: &std::path::Path, name: &str, raw: &str) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let path = dir.join(format!("{name}.json"));
    let tmp = dir.join(format!("{name}.json.{}.tmp", std::process::id()));
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(raw.as_bytes())?;
    drop(f);
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_openings_are_recognised() {
        let bash = |c: &str| opens_pr("Bash", &json!({"command": c}));
        assert!(bash("gh pr create --fill"));
        assert!(bash("cd x && gh pr create -t T -b B"));
        assert!(bash("glab mr create"));
        assert!(bash("tb api bitbucket repositories/acme/web/pullrequests -d '{\"title\":\"x\"}'"));
        assert!(bash("gh api repos/acme/web/pulls -f title=x -f head=b -f base=main"));
        assert!(!bash("gh pr view 9"));
        assert!(!bash("gh api repos/acme/web/pulls"));
        assert!(!bash("tb api bitbucket repositories/acme/web/pullrequests/9/comments -d '{}'"));
        assert!(opens_pr("mcp__github__create_pull_request", &json!({})));
        assert!(!opens_pr("mcp__github__get_pull_request", &json!({})));
    }

    #[test]
    fn the_place_a_call_works_is_its_file_or_the_folder_its_git_runs_in() {
        let bash = |c: &str| work_place("Bash", &json!({"command": c}), "/w/a");
        assert_eq!(work_place("Edit", &json!({"file_path": "/w/b/src/x.rs"}), "/w/a"), Some(PathBuf::from("/w/b/src/x.rs")));
        assert_eq!(work_place("Write", &json!({"file_path": "notes.md"}), "/w/a"), Some(PathBuf::from("/w/a/notes.md")));
        assert_eq!(bash("git commit -m x"), Some(PathBuf::from("/w/a")));
        assert_eq!(bash("git -C /w/b commit -m x"), Some(PathBuf::from("/w/b")));
        assert_eq!(bash("cd /w/b && git worktree add ../b-fix -b fix"), Some(PathBuf::from("/w/b")));
        assert_eq!(bash("cd ../b && git add . && git commit -m x"), Some(PathBuf::from("/w/a/../b")));
        assert_eq!(bash("cd \"/w/b\" && gh pr create --fill"), Some(PathBuf::from("/w/b")));
        assert_eq!(bash("git status && git log"), None, "reading changes nothing");
        assert_eq!(bash("git worktree list"), None);
        assert_eq!(bash("git -C /w/b checkout -b fix"), Some(PathBuf::from("/w/b")));
        assert_eq!(bash("pushd /w/b && git switch main"), Some(PathBuf::from("/w/b")));
        assert_eq!(bash("cd /w/b; git rebase origin/main"), Some(PathBuf::from("/w/b")));
        assert_eq!(bash("git branch --show-current && git diff"), None);
        assert_eq!(work_place("Read", &json!({"file_path": "/w/b/x"}), "/w/a"), None);
    }

    fn git_repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        let ok = std::process::Command::new("git").arg("-C").arg(dir).args(["init", "-q"]).status().unwrap().success();
        assert!(ok);
    }

    #[test]
    fn the_gate_judges_the_tip_of_the_tasks_branch_in_its_worktree() {
        let root = std::env::temp_dir().join(format!("tb-own-head-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        git_repo(&root);
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git").arg("-C").arg(&root).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        git(&["commit", "-q", "--allow-empty", "-m", "one"]);
        let one = git(&["rev-parse", "HEAD"]);
        git(&["branch", "fix"]);
        git(&["commit", "-q", "--allow-empty", "-m", "two"]);
        let two = git(&["rev-parse", "HEAD"]);
        let dir = root.to_string_lossy();
        assert_eq!(own_head(&json!({"worktree": dir, "branch": "fix"})), Some(one), "the task's branch, not the checkout's HEAD");
        assert_eq!(own_head(&json!({"worktree": dir, "branch": ""})), Some(two.clone()));
        assert_eq!(own_head(&json!({"worktree": dir, "branch": "gone"})), Some(two));
        assert_eq!(own_head(&json!({"worktree": root.join("missing").to_string_lossy(), "branch": "fix"})), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn work_in_a_repository_the_task_does_not_own_is_refused() {
        let root = std::env::temp_dir().join(format!("tb-elsewhere-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (a, b) = (root.join("a"), root.join("b"));
        git_repo(&a);
        git_repo(&b);
        let (why, top) = elsewhere(&b.join("src/x.rs"), &a, "T115", "a").expect("another repository");
        assert_eq!(top, b.canonicalize().unwrap());
        assert!(why.contains("T115 is a's work") && why.contains("midna project add"), "{why}");
        assert!(elsewhere(&a.join("src/x.rs"), &a, "T115", "a").is_none(), "the task's own repository");
        assert!(elsewhere(&root.join("loose.txt"), &a, "T115", "a").is_none(), "outside any repository");

        // The task's own worktree shares its repository.
        let commit = |d: &Path, args: &[&str]| std::process::Command::new("git").arg("-C").arg(d).args(args).output().unwrap();
        commit(&a, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "x"]);
        commit(&a, &["worktree", "add", "-q", root.join("a-fix").to_str().unwrap()]);
        assert!(elsewhere(&root.join("a-fix/src/x.rs"), &a, "T115", "a").is_none());

        // A repository that holds the task's (a dotfiles one in the home folder) isn't another project's.
        git_repo(&root);
        assert!(elsewhere(&root.join(".zshrc"), &a, "T115", "a").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn opening_a_pr_on_a_task_that_ends_in_one_points_to_pr_body() {
        let why = board_opens_pr(&json!({"task": "T7", "ships_pr": true}), "tb").unwrap();
        assert!(why.starts_with("T7 ends in a PR"), "{why}");
        assert!(why.contains("tb submit \"<summary>\" --pr-body <file>"), "{why}");
        assert!(board_opens_pr(&json!({"task": "T7", "ships_pr": false}), "tb").is_none());
        assert!(board_opens_pr(&json!({"task": "T7"}), "tb").is_none(), "an older board's listing has no flag");
        assert!(GITLAB_RE.is_match("glab mr create --fill") && !GITLAB_RE.is_match("gh pr create"), "the board can't open a GitLab MR");
    }

    #[test]
    fn bash_and_subagent_calls_open_a_window_by_their_id() {
        let call = |tool: &str, input: Value, id: Option<&str>| {
            let mut p = json!({"tool_name": tool, "tool_input": input, "cwd": "/nowhere/at/all", "session_id": "c1"});
            if let Some(id) = id {
                p["tool_use_id"] = json!(id);
            }
            tool_window_body(&p, "s1", "hook.tool_start")
        };
        let b = call("Bash", json!({"command": "ls"}), Some("toolu_1")).expect("a Bash call");
        assert_eq!((b["event"].as_str(), b["tool_use_id"].as_str(), b["background"].as_bool()), (Some("hook.tool_start"), Some("toolu_1"), Some(false)));
        assert_eq!(b["tree"], Value::Null, "no checkout, no stamp");
        let bg = call("Agent", json!({"prompt": "x", "run_in_background": true}), None).expect("a subagent");
        assert_eq!(bg["background"], json!(true));
        assert_eq!(bg["tool_use_id"], call("Agent", json!({"prompt": "x", "run_in_background": true}), None).unwrap()["tool_use_id"], "the same call, the same id");
        assert!(call("Read", json!({"file_path": "/a"}), Some("toolu_2")).is_none());
        assert_eq!(bg["tool_use_id"].as_str(), Some(taskboardd::transcript::call_key("Agent", &json!({"prompt": "x", "run_in_background": true})).as_str()), "the board can find it in the transcript");
    }

    #[test]
    fn knows_whether_the_loaded_hooks_send_tool_end() {
        let plugin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin/task-board");
        assert!(hooks_send_tool_end(Some(&plugin)), "this plugin's hooks.json");
        let old = std::env::temp_dir().join(format!("tb-old-hooks-{}", std::process::id()));
        std::fs::create_dir_all(old.join("hooks")).unwrap();
        let hooks = json!({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "tb", "args": ["hook", "PreToolUse"]}]}], "PostToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "tb", "args": ["hook", "PostToolUse"]}]}]}});
        std::fs::write(old.join("hooks").join("hooks.json"), hooks.to_string()).unwrap();
        assert!(!hooks_send_tool_end(Some(&old)), "a hooks.json from before ToolEnd");
        let _ = std::fs::remove_dir_all(&old);
        assert!(!hooks_send_tool_end(None), "no plugin folder");
    }

    #[test]
    fn git_commands_are_recognised() {
        assert!(GIT_RE.is_match("git commit -m x"));
        assert!(GIT_RE.is_match("git -C /a push origin x"));
        assert!(!GIT_RE.is_match("git status"));
        assert!(PUSH_RE.is_match("git push --force-with-lease"));
        assert!(!PUSH_RE.is_match("git commit -m push"));
        assert_eq!(clip_middle("abcdefghij", 7), "abc\n…\nj");
    }

    #[test]
    fn a_long_prompt_keeps_its_start_and_its_end() {
        let typed = format!("start T8. here's the log:\n{}\njk", "INFO all good\n".repeat(3000));
        let report = taskboardd::startword::hook_prompt(&typed);
        assert_eq!(report["prompt_cut"], taskboardd::startword::KEPT_ENDS, "it says how it cut");
        let sent = report["prompt"].as_str().unwrap();
        assert!(sent.chars().count() <= MAX_PROMPT);
        assert!(sent.starts_with("start T8.") && sent.ends_with("\njk") && sent.contains(taskboardd::startword::CUT_MARK));
        let short = format!("start T8. {}…", "word ".repeat(1000));
        assert_eq!(taskboardd::startword::keep_ends(&short, MAX_PROMPT), short, "sent whole up to the board's cap");
    }

    #[test]
    fn a_headless_run_isnt_the_terminals_conversation() {
        assert!(headless(Some("sdk-cli")), "claude -p");
        assert!(headless(Some("sdk-ts")), "the Agent SDK");
        assert!(!headless(Some("cli")), "the terminal's own claude");
        assert!(!headless(None));
    }
}

#[cfg(test)]
mod private_status_tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_status_folder_and_files_are_private_even_when_the_folder_was_open() {
        let root = std::env::temp_dir().join(format!("tb-statusline-test-{}", std::process::id()));
        let dir = root.join("statusline");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        super::save_private(&dir, "s1", "{}").unwrap();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("s1.json")), 0o600);
        assert_eq!(std::fs::read_to_string(dir.join("s1.json")).unwrap(), "{}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
