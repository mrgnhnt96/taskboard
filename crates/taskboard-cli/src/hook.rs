//! `tb hook <Event>`: the Claude Code hooks. They report to the board and never get in Claude's way:
//! outside a Midna terminal they do nothing, they always exit 0, and they print only valid hook JSON.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::client::{self, CallError};
use taskboardd::config::Config;

const CONTEXT_EVENTS: &[&str] = &["SessionStart", "UserPromptSubmit"];
const WAITING_NOTIFICATIONS: &[&str] = &["permission_prompt", "idle_prompt"];
const CONTINUING_END_REASONS: &[&str] = &["clear", "resume"];
const GIT_TIMEOUT: f64 = 0.3;
const SLACK: f64 = 0.35;
const SESSION_END_POST_CAP: f64 = 0.8;
const MAX_CONTEXT: usize = 9900;
const MAX_PROMPT: usize = 8000;
const MAX_LAST_MESSAGE: usize = 2000;
const MAX_OUTPUT: usize = 1000;

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

pub fn run(event_arg: Option<&str>) -> i32 {
    let start = Instant::now();
    let Ok(session) = std::env::var("MIDNA_SESSION") else { return 0 };
    if session.is_empty() {
        return 0;
    }
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let payload: Value = serde_json::from_str(&raw).ok().filter(|v: &Value| v.is_object()).unwrap_or(json!({}));
    let hook = event_arg.map(|s| s.to_string()).filter(|s| !s.is_empty()).or_else(|| payload["hook_event_name"].as_str().map(|s| s.to_string())).unwrap_or_default();
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
            extra.insert("prompt".into(), json!(clip(&s("prompt"), MAX_PROMPT)));
        }
        "Stop" => {
            extra.insert("last_message".into(), json!(clip(&s("last_assistant_message"), MAX_LAST_MESSAGE)));
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
            Err(CallError::Unreachable(_)) => None,
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
        let sent = l > 0.05 && !matches!(client::request(&cfg, "POST", "/report", Some(&ack), l), Err(CallError::Unreachable(_)));
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
    let dir = cfg.statusline_dir.clone();
    if std::fs::create_dir_all(&dir).is_err() {
        return 0;
    }
    let path = dir.join(format!("{name}.json"));
    let tmp = dir.join(format!("{name}.json.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, raw).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_commands_are_recognised() {
        assert!(GIT_RE.is_match("git commit -m x"));
        assert!(GIT_RE.is_match("git -C /a push origin x"));
        assert!(!GIT_RE.is_match("git status"));
        assert_eq!(clip_middle("abcdefghij", 7), "abc\n…\nj");
    }
}
