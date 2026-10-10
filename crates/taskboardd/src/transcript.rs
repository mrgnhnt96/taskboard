//! Reading Claude Code transcripts (`~/.claude/projects/<dashed path>/<conversation>.jsonl`).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use regex::bytes::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;

const ENDED: &[&str] = &["turn_duration", "stop_hook_summary"];
const NEUTRAL_PREFIXES: &[&str] = &["<command-name>", "<command-message>", "<local-command", "<system-reminder>", "[Request interrupted by user"];
const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];
/// Tools that can change files the transcript doesn't name: a shell command, or a subagent.
const SHELL_TOOLS: &[&str] = &["Bash"];
const SUBAGENT_TOOLS: &[&str] = &["Task", "Agent"];
const TAIL_BYTES: u64 = 2 * 1024 * 1024;
pub const CLAUDE_STOPS_BACKGROUND_AFTER: f64 = 2.0 * 3600.0;
const AGENT_LAUNCHED: &[u8] = b"async_launched";

static DASH: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"[^A-Za-z0-9]").unwrap());
static BG_STARTED: Lazy<Regex> = Lazy::new(|| Regex::new(r#""backgroundTaskId":\s*"([^"]+)""#).unwrap());
static BG_ENDED: Lazy<Regex> = Lazy::new(|| Regex::new(r"<task-id>([^<]+)</task-id>").unwrap());
/// A directory a shell command works in: `cd <dir>`, `pushd <dir>` or `git -C <dir>`.
static SHELL_DIR: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r#"(?:^|[;&|(]|\s)(?:cd|pushd|git\s+-C)\s+("[^"]+"|'[^']+'|[^\s;&|()]+)"#).unwrap());

pub fn project_dir(root: &Path, project_path: &str) -> PathBuf {
    root.join(DASH.replace_all(project_path.trim_end_matches('/'), "-").as_ref())
}

pub fn transcript_file(root: &Path, project_path: &str, claude_id: &str) -> PathBuf {
    let p = project_dir(root, project_path).join(format!("{claude_id}.jsonl"));
    if p.is_file() {
        return p;
    }
    if let Ok(dirs) = std::fs::read_dir(root) {
        for d in dirs.flatten() {
            let c = d.path().join(format!("{claude_id}.jsonl"));
            if c.is_file() {
                return c;
            }
        }
    }
    p
}

/// Background work a transcript started that hasn't reported back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Background {
    pub commands: usize,
    pub agents: usize,
}

impl Background {
    pub fn total(&self) -> usize {
        self.commands + self.agents
    }
}

/// The background commands and background agents in this transcript that started (under two hours
/// ago) and haven't reported back.
pub fn background_running(root: &Path, path: Option<&str>) -> Option<Background> {
    let p = std::fs::canonicalize(expand_home(path?)).ok()?;
    let root = std::fs::canonicalize(root).ok()?;
    if p.extension().map(|e| e != "jsonl").unwrap_or(true) || !p.starts_with(&root) {
        return None;
    }
    let data = std::fs::read(&p).ok()?;
    let ended: Vec<&[u8]> = BG_ENDED.captures_iter(&data).filter_map(|c| c.get(1).map(|m| m.as_bytes())).collect();
    let mut running = Background::default();
    for line in data.split(|b| *b == b'\n') {
        if !BG_STARTED.is_match(line) && !line.windows(AGENT_LAUNCHED.len()).any(|w| w == AGENT_LAUNCHED) {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<Value>(line) else { continue };
        let Some((id, agent)) = started_in_background(line, &v) else { continue };
        if ended.contains(&id.as_bytes()) {
            continue;
        }
        let Some(at) = v["timestamp"].as_str().and_then(parse_iso) else { continue };
        if crate::clock::awake_since(at) >= CLAUDE_STOPS_BACKGROUND_AFTER {
            continue;
        }
        if agent {
            running.agents += 1;
        } else {
            running.commands += 1;
        }
    }
    Some(running)
}

/// The id a background command (`backgroundTaskId`) or background agent (an `async_launched` agent's
/// `agentId`) started on this line goes by, and whether it's an agent; its `<task-notification>`
/// carries the same id.
fn started_in_background(line: &[u8], v: &Value) -> Option<(String, bool)> {
    if let Some(c) = BG_STARTED.captures(line) {
        return Some((String::from_utf8_lossy(c.get(1)?.as_bytes()).into_owned(), false));
    }
    let r = &v["toolUseResult"];
    if r["status"] != "async_launched" {
        return None;
    }
    r["agentId"].as_str().filter(|id| !id.is_empty()).map(|id| (id.to_string(), true))
}

fn session_path(app: &App, s: &Row) -> Option<PathBuf> {
    let cid = s.s("claude_session_id").filter(|c| !c.is_empty())?;
    let p = transcript_file(&app.cfg.claude_projects, s.s("project_path").unwrap_or(""), cid);
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

fn lines(path: &Path) -> Vec<Value> {
    let Ok(mut f) = std::fs::File::open(path) else { return vec![] };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut buf = Vec::new();
    if size > TAIL_BYTES {
        let _ = f.seek(SeekFrom::Start(size - TAIL_BYTES));
    }
    let _ = f.read_to_end(&mut buf);
    let mut it = buf.split(|b| *b == b'\n');
    if size > TAIL_BYTES {
        it.next();
    }
    it.filter_map(|l| serde_json::from_slice::<Value>(l).ok()).filter(|v| v.is_object()).collect()
}

fn prompt_of(e: &Value) -> Option<String> {
    if e["type"] != "user" || e["isMeta"] == true || e["isSidechain"] == true {
        return None;
    }
    let content = &e["message"]["content"];
    let mut texts = vec![];
    match content {
        Value::String(s) => texts.push(s.clone()),
        Value::Array(a) => {
            for b in a {
                if b["type"] == "tool_result" {
                    return None;
                }
                if b["type"] == "text" {
                    texts.push(b["text"].as_str().unwrap_or("").to_string());
                }
            }
        }
        _ => return None,
    }
    let text = texts.into_iter().filter(|t| !t.trim().is_empty()).collect::<Vec<_>>().join("\n");
    let lead = text.trim_start();
    if text.trim().is_empty() || NEUTRAL_PREFIXES.iter().any(|p| lead.starts_with(p)) {
        return None;
    }
    Some(text)
}

/// The main conversation's tool calls in one event, as (name, input).
fn tool_uses(e: &Value) -> Vec<(&str, &Value)> {
    if e["type"] != "assistant" || e["isSidechain"] == true {
        return vec![];
    }
    e["message"]["content"]
        .as_array()
        .map(|a| a.iter().filter(|b| b["type"] == "tool_use").map(|b| (b["name"].as_str().unwrap_or(""), &b["input"])).collect())
        .unwrap_or_default()
}

fn edits(e: &Value) -> Vec<String> {
    if e["type"] != "assistant" || e["isSidechain"] == true {
        return vec![];
    }
    e["message"]["content"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|b| b["type"] == "tool_use" && EDIT_TOOLS.contains(&b["name"].as_str().unwrap_or("")))
                .filter_map(|b| b["input"]["file_path"].as_str().or(b["input"]["notebook_path"].as_str()).map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Default)]
pub struct Turn {
    pub at: Value,
    pub prompt: String,
    pub files: Vec<String>,
    pub closed: bool,
    /// The turn ran a shell command, which can change files without naming them.
    pub ran_shell: bool,
    /// The turn ran a subagent, whose edits aren't in this transcript.
    pub ran_subagent: bool,
    /// Directories the turn's shell commands moved into (`cd`, `pushd`, `git -C`), as written.
    pub shell_dirs: Vec<String>,
    /// Paths the turn's shell commands name (`../lib/x.rs`, `/abs/dir`), as written.
    pub shell_paths: Vec<String>,
    /// The turn's tool calls that came back, by `tool_use_id`, and whether each was refused (denied
    /// at the permission prompt or blocked by a hook) rather than run; a call that failed or was cut
    /// off after it started ran.
    pub results: Vec<(String, bool)>,
    /// The turn's tool calls, as (`tool_use_id`, `call_key`): what a hook that got no id calls one.
    pub calls: Vec<(String, String)>,
}

/// The id a hook gives a tool call Claude sent no `tool_use_id` for: made from its tool and input,
/// keys sorted, so the transcript's copy of the call gets the same one.
pub fn call_key(tool: &str, input: &Value) -> String {
    fn sorted(v: &Value) -> Value {
        match v {
            Value::Object(o) => {
                let mut keys: Vec<&String> = o.keys().collect();
                keys.sort();
                Value::Object(keys.into_iter().map(|k| (k.clone(), sorted(&o[k]))).collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
            x => x.clone(),
        }
    }
    // FNV-1a: the same in `tb` and the daemon, whatever either was built with.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in tool.bytes().chain([0]).chain(sorted(input).to_string().into_bytes()) {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    format!("call-{h:016x}")
}

/// How Claude Code words the result of a call it never ran: the owner denied it (or it needed an
/// approval no one could give), or a PreToolUse hook blocked it.
const REFUSALS: &[&str] = &[
    "The user doesn't want to proceed",
    "The user doesn't want to take this action",
    "Permission to use ",
    "Permission for this command was denied",
    "This command requires approval",
    "This command uses shell operators that require approval",
    "This Bash command contains multiple operations",
    "Contains expansion",
    "PreToolUse:",
];

/// A tool result's text, whether a string or a list of text blocks.
fn result_text(b: &Value) -> String {
    match &b["content"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|x| x["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// An error result for a call that never ran: refused, not failed or cut off after it started.
fn refused(b: &Value) -> bool {
    b["is_error"] == true && {
        let text = result_text(b);
        REFUSALS.iter().any(|r| text.trim_start().starts_with(r))
    }
}

/// The tool calls in one of the main conversation's events, as (tool_use_id, call_key).
fn tool_calls(e: &Value) -> Vec<(String, String)> {
    if e["type"] != "assistant" || e["isSidechain"] == true {
        return vec![];
    }
    e["message"]["content"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|b| b["type"] == "tool_use")
                .filter_map(|b| b["id"].as_str().filter(|id| !id.is_empty()).map(|id| (id.to_string(), call_key(b["name"].as_str().unwrap_or(""), &b["input"]))))
                .collect()
        })
        .unwrap_or_default()
}

/// The paths a shell command names: its words with a `/` in them (a flag's `--x=` value included),
/// quotes taken off. URLs and bare words aren't paths; a bare name sits in the folder the command runs in.
fn named_paths(command: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for word in command.split(|c: char| c.is_whitespace() || ";&|()<>".contains(c)) {
        let w = word.trim_matches(|c| c == '"' || c == '\'' || c == '`');
        let w = if w.starts_with('-') { w.split_once('=').map(|(_, v)| v.trim_matches(|c| c == '"' || c == '\'')).unwrap_or("") } else { w };
        if w.contains('/') && !w.contains("://") && !w.contains('$') && !out.iter().any(|x| x == w) {
            out.push(w.to_string());
        }
    }
    out
}

/// The tool results in one of the main conversation's events, as (tool_use_id, refused).
fn tool_results(e: &Value) -> Vec<(String, bool)> {
    if e["type"] != "user" || e["isSidechain"] == true {
        return vec![];
    }
    e["message"]["content"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|b| b["type"] == "tool_result")
                .filter_map(|b| b["tool_use_id"].as_str().filter(|id| !id.is_empty()).map(|id| (id.to_string(), refused(b))))
                .collect()
        })
        .unwrap_or_default()
}

fn turns_of(events: &[Value]) -> Vec<Turn> {
    let mut out: Vec<Turn> = vec![];
    for e in events {
        if let Some(p) = prompt_of(e) {
            out.push(Turn { at: e["timestamp"].clone(), prompt: p, ..Turn::default() });
            continue;
        }
        let Some(cur) = out.last_mut() else { continue };
        if e["type"] == "system" && ENDED.contains(&e["subtype"].as_str().unwrap_or("")) {
            cur.closed = true;
        } else {
            for p in edits(e) {
                if !cur.files.contains(&p) {
                    cur.files.push(p);
                }
            }
            cur.results.extend(tool_results(e));
            cur.calls.extend(tool_calls(e));
            for (name, input) in tool_uses(e) {
                if SUBAGENT_TOOLS.contains(&name) {
                    cur.ran_subagent = true;
                }
                if SHELL_TOOLS.contains(&name) {
                    cur.ran_shell = true;
                    let command = input["command"].as_str().unwrap_or("");
                    for p in named_paths(command) {
                        if !cur.shell_paths.contains(&p) {
                            cur.shell_paths.push(p);
                        }
                    }
                    for c in SHELL_DIR.captures_iter(command) {
                        let d = c[1].trim_matches(|ch| ch == '"' || ch == '\'').to_string();
                        if !d.is_empty() && d != "-" && !cur.shell_dirs.contains(&d) {
                            cur.shell_dirs.push(d);
                        }
                    }
                }
            }
        }
    }
    out
}

pub fn last_turn(app: &App, s: &Row) -> Option<Turn> {
    let p = session_path(app, s)?;
    turns_of(&lines(&p)).pop()
}

/// The last turn of the transcript a hook named (its `transcript_path`), when it's one of Claude's.
pub fn last_turn_at(root: &Path, path: Option<&str>) -> Option<Turn> {
    let p = std::fs::canonicalize(expand_home(path.filter(|p| !p.is_empty())?)).ok()?;
    let root = std::fs::canonicalize(root).ok()?;
    if p.extension().map(|e| e != "jsonl").unwrap_or(true) || !p.starts_with(&root) {
        return None;
    }
    turns_of(&lines(&p)).pop()
}

pub fn turns(app: &App, s: &Row, limit: usize) -> Value {
    let found = session_path(app, s).map(|p| turns_of(&lines(&p))).unwrap_or_default();
    let mut files: Vec<&String> = found.iter().flat_map(|t| t.files.iter()).collect();
    files.sort();
    files.dedup();
    let list: Vec<Value> = found
        .iter()
        .rev()
        .take(limit)
        .map(|t| json!({"at": t.at, "prompt": t.prompt, "files": t.files, "closed": t.closed}))
        .collect();
    json!({"turns": list, "count": found.len(), "files": files.len()})
}

/// How big a conversation's context was at its last reply, and when it last wrote anything.
pub struct Size {
    pub tokens: Option<i64>,
    pub last_at: Option<String>,
}

fn size_of(events: &[Value]) -> Size {
    let last_at = events.iter().rev().find_map(|e| e["timestamp"].as_str().map(|s| s.to_string()));
    // The newest of the last reply's usage and the last compact boundary's postTokens wins.
    let tokens = events.iter().rev().filter(|e| e["isSidechain"] != true).find_map(|e| {
        if e["type"] == "system" && e["subtype"] == "compact_boundary" {
            return e["compactMetadata"]["postTokens"].as_i64();
        }
        if e["type"] != "assistant" {
            return None;
        }
        let u = e["message"].get("usage")?;
        let n = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens"].iter().map(|k| u[*k].as_i64().unwrap_or(0)).sum::<i64>();
        (n > 0).then_some(n)
    });
    Size { tokens, last_at }
}

/// The size of the conversation `claude_id` (run in `project_path`), if its transcript is found.
pub fn conversation_size(root: &Path, project_path: &str, claude_id: &str) -> Option<Size> {
    let p = transcript_file(root, project_path, claude_id);
    p.is_file().then(|| size_of(&lines(&p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashes_project_dirs() {
        assert_eq!(project_dir(Path::new("/r"), "/Users/a/my.proj/"), PathBuf::from("/r/-Users-a-my-proj"));
    }

    #[test]
    fn reads_turns_and_edits() {
        let ev = vec![
            json!({"type": "user", "message": {"content": "Do it"}, "timestamp": "2026-10-01T10:00:00Z"}),
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Edit", "input": {"file_path": "/a.rs"}}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result"}]}}),
            json!({"type": "system", "subtype": "turn_duration"}),
            json!({"type": "user", "message": {"content": "<system-reminder>x"}}),
        ];
        let t = turns_of(&ev);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].files, vec!["/a.rs"]);
        assert!(t[0].closed);
        assert!(!t[0].ran_shell && !t[0].ran_subagent);
    }

    #[test]
    fn reads_the_shell_commands_and_subagents_a_turn_ran() {
        let bash = |c: &str| json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Bash", "input": {"command": c}}]}});
        let ev = vec![
            json!({"type": "user", "message": {"content": "Do it"}, "timestamp": "2026-10-01T10:00:00Z"}),
            bash("cd /repo/app && cargo fmt; git -C '/repo/lib' status"),
            bash("(cd sub && make) | tail; cd -"),
            json!({"type": "assistant", "isSidechain": true, "message": {"content": [{"type": "tool_use", "name": "Agent", "input": {}}]}}),
        ];
        let t = turns_of(&ev);
        assert!(t[0].ran_shell && !t[0].ran_subagent, "a sidechain's own calls aren't the turn's");
        assert_eq!(t[0].shell_dirs, vec!["/repo/app", "/repo/lib", "sub"]);
        let mut ev = ev;
        ev.push(json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Task", "input": {"prompt": "x"}}]}}));
        assert!(turns_of(&ev)[0].ran_subagent);
    }

    #[test]
    fn reads_the_paths_a_command_names_and_each_calls_result() {
        assert_eq!(named_paths(r#"sed -i '' s/a/b/ ../lib/x.rs && cat "/abs/dir/y.rs" --out=build/z curl https://x.io/a $HOME/q ls"#), vec!["s/a/b/", "../lib/x.rs", "/abs/dir/y.rs", "build/z"]);
        let ev = vec![
            json!({"type": "user", "message": {"content": "Go"}, "timestamp": "2026-10-01T10:00:00Z"}),
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "is_error": true, "content": "The user doesn't want to proceed with this tool use. The tool use was rejected."}]}}),
            json!({"type": "user", "isSidechain": true, "message": {"content": [{"type": "tool_result", "tool_use_id": "t2", "content": "ok"}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t3", "content": "ok"}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t4", "is_error": true, "content": [{"type": "text", "text": "PreToolUse:Bash hook error: midna: denied by the human"}]}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t5", "is_error": true, "content": "Exit code 1\nerror: test failed"}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t6", "is_error": true, "content": "[Tool call interrupted: the session ended before this call's result was recorded]"}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t7", "is_error": true, "content": "This command requires approval"}]}}),
        ];
        let t = turns_of(&ev);
        assert_eq!(t.len(), 1, "a tool result isn't a prompt");
        let r = |id: &str, refused: bool| (id.to_string(), refused);
        assert_eq!(t[0].results, vec![r("t1", true), r("t3", false), r("t4", true), r("t5", false), r("t6", false), r("t7", true)], "only a refusal never ran");
        assert_eq!(t[0].calls, vec![("t1".to_string(), call_key("Bash", &json!({"command": "ls"})))]);
    }

    #[test]
    fn a_calls_key_is_its_tool_and_input_whatever_the_key_order() {
        let a = call_key("Bash", &serde_json::from_str(r#"{"command": "ls", "description": "List", "x": {"b": 1, "a": [2]}}"#).unwrap());
        let b = call_key("Bash", &serde_json::from_str(r#"{"x": {"a": [2], "b": 1}, "description": "List", "command": "ls"}"#).unwrap());
        assert_eq!(a, b);
        assert!(a.starts_with("call-") && a.len() == 21, "{a}");
        assert_ne!(a, call_key("Agent", &serde_json::from_str(r#"{"command": "ls", "description": "List", "x": {"b": 1, "a": [2]}}"#).unwrap()));
        assert_ne!(a, call_key("Bash", &json!({"command": "ls -a"})));
    }

    #[test]
    fn reads_the_last_context_size() {
        let ev = vec![
            json!({"type": "assistant", "message": {"usage": {"input_tokens": 5, "cache_read_input_tokens": 1000}}, "timestamp": "2026-10-01T10:00:00Z"}),
            json!({"type": "assistant", "isSidechain": true, "message": {"usage": {"input_tokens": 99999}}, "timestamp": "2026-10-01T10:01:00Z"}),
            json!({"type": "assistant", "message": {"usage": {"input_tokens": 10, "cache_creation_input_tokens": 200, "cache_read_input_tokens": 3000, "output_tokens": 40}}, "timestamp": "2026-10-01T10:02:00Z"}),
            json!({"type": "system", "subtype": "turn_duration", "timestamp": "2026-10-01T10:03:00Z"}),
        ];
        let s = size_of(&ev);
        assert_eq!(s.tokens, Some(3250));
        assert_eq!(s.last_at.as_deref(), Some("2026-10-01T10:03:00Z"));
    }

    #[test]
    fn reads_post_tokens_after_compacting() {
        let reply = json!({"type": "assistant", "message": {"usage": {"input_tokens": 10, "cache_read_input_tokens": 150000}}, "timestamp": "2026-10-01T10:00:00Z"});
        let boundary = json!({"type": "system", "subtype": "compact_boundary", "compactMetadata": {"trigger": "manual", "preTokens": 150010, "postTokens": 12000}, "timestamp": "2026-10-01T10:01:00Z"});
        assert_eq!(size_of(&[reply.clone(), boundary.clone()]).tokens, Some(12000));
        let later = json!({"type": "assistant", "message": {"usage": {"input_tokens": 5, "cache_read_input_tokens": 14000}}, "timestamp": "2026-10-01T10:02:00Z"});
        assert_eq!(size_of(&[reply.clone(), boundary, later]).tokens, Some(14005));
        let bare = json!({"type": "system", "subtype": "compact_boundary", "compactMetadata": {"trigger": "auto"}});
        assert_eq!(size_of(&[reply, bare]).tokens, Some(150010));
    }
}
