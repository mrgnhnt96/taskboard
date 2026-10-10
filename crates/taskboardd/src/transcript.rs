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
/// The neutral prompts that are a slash command the owner sent (its UserPromptSubmit still fires).
const SLASH_PREFIXES: &[&str] = &["<command-name>", "<command-message>"];
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
    let text = prompt_text(e)?;
    let lead = text.trim_start();
    if NEUTRAL_PREFIXES.iter().any(|p| lead.starts_with(p)) {
        return None;
    }
    Some(text)
}

/// A slash command the owner sent: neutral to the turn, but its UserPromptSubmit starts the hooks'
/// windows (and the prompt's stamp) over.
fn slash_command(e: &Value) -> bool {
    prompt_text(e).is_some_and(|t| SLASH_PREFIXES.iter().any(|p| t.trim_start().starts_with(p)))
}

/// The text of a user entry that isn't a tool's result, meta, a subagent's or a compaction's summary
/// (an auto-compact mid-turn writes one, and the turn goes on past it).
fn prompt_text(e: &Value) -> Option<String> {
    if e["type"] != "user" || ["isMeta", "isSidechain", "isCompactSummary", "isVisibleInTranscriptOnly"].iter().any(|k| e[*k] == true) {
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
    (!text.trim().is_empty()).then_some(text)
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
    /// The turn's tool calls in order, as (`tool_use_id`, `call_key`, tool): the key is what a hook
    /// that got no id calls one.
    pub calls: Vec<(String, String, String)>,
    /// Paths the turn's shell commands run or build from (`python3 ../tools/gen.py`, `--manifest-path
    /// ../Cargo.toml`), as written: what they write can be anywhere in the file's folder.
    pub shell_runs: Vec<String>,
    /// How many of `calls` came before the turn's last slash command: the hooks' windows (and the
    /// prompt's stamp) start over at its UserPromptSubmit, so only the calls after it are theirs.
    pub since_prompt: usize,
}

impl Turn {
    /// The calls made since the last prompt the hooks saw: the ones this turn's windows cover.
    pub fn current_calls(&self) -> &[(String, String, String)] {
        &self.calls[self.since_prompt.min(self.calls.len())..]
    }

    /// Whether call `id` came back without being refused: `None` when the transcript shows no result.
    pub fn ran(&self, id: &str) -> Option<bool> {
        self.results.iter().rev().find(|(i, _)| i == id).map(|(_, refused)| !refused)
    }
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
/// approval no one could give), the auto mode classifier or a safety check denied it, or a PreToolUse
/// hook blocked it. Matched after an `Error: `, `Hook ` or `<tool_use_error>` in front.
const REFUSALS: &[&str] = &[
    "The user doesn't want to proceed",
    "The user doesn't want to take this action",
    "Permission to use ",
    "Permission for this ",
    "This command requires approval",
    "This command uses shell operators that require approval",
    "This Bash command contains multiple operations",
    "Contains expansion",
    "PreToolUse:",
    "The server-side auto mode classifier",
    "This command changes directory before running a version-control command",
    "This session is isolated in the worktree",
    "This agent is isolated in the worktree",
    "Parser skipped input between top-level statements",
];

/// How an error from a call that started reads, whatever deny word it has: the API's or a subagent's.
const RAN_ERRORS: &[&str] = &["API Error", "Agent stopped"];
/// Words a bare deny reason (a hook's own `permissionDecisionReason`, sent back as its text) says the
/// call was stopped with.
const DENY_WORDS: &[&str] = &["refuse", "denied", "deny", "blocked", "not allowed", "disallowed", "forbidden", "rejected", "policy"];
/// How a program's own message starts (`fatal: ...`, `! [rejected] ...`, `403 Forbidden`): a command
/// that ran and failed, not a hook's reason.
static PROGRAM_MESSAGE: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"^(?i:[^a-z]|(error|fatal|warning|hint|remote):|npm err!)").unwrap());
/// The longest a bare deny reason runs.
const DENY_REASON_MAX: usize = 300;
/// What a command that ran prints when the OS or a server turns it down: its own failure, not a refusal.
const RAN_DENIALS: &[&str] = &["permission denied", "connection refused", "operation not permitted"];

/// A tool result's text, whether a string or a list of text blocks.
fn result_text(b: &Value) -> String {
    match &b["content"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|x| x["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// An error result for a call that never ran: refused, not failed or cut off after it started.
///
/// A call ran unless its error reads as a refusal: Claude Code's own wording for one (`REFUSALS`,
/// whatever else the text says), or a bare deny reason: one short line with a `DENY_WORDS` word and
/// nothing that says the call started (the tool's own output, as Claude keeps a shell's stdout and
/// stderr in `output`; a shell's exit code; an interruption; an API error; a subagent stopped; a
/// program's own message). Any other error (an API error, max turns, a command aborted, killed or
/// failing on stderr) is from a call that ran, and may have edited first.
fn refused(b: &Value, output: bool) -> bool {
    if b["is_error"] != true {
        return false;
    }
    let text = result_text(b);
    let mut t = text.trim_start();
    for front in ["<tool_use_error>", "Error: ", "Hook "] {
        t = t.strip_prefix(front).unwrap_or(t).trim_start();
    }
    if REFUSALS.iter().any(|r| t.starts_with(r)) {
        return true;
    }
    if output || t.starts_with("Exit code ") || t.starts_with("[Tool call interrupted") || RAN_ERRORS.iter().any(|r| t.starts_with(r)) {
        return false;
    }
    let reason = t.trim_end();
    if reason.contains('\n') || reason.len() > DENY_REASON_MAX || PROGRAM_MESSAGE.is_match(reason) {
        return false;
    }
    let mut reason = reason.to_lowercase();
    for ran in RAN_DENIALS {
        reason = reason.replace(ran, "");
    }
    DENY_WORDS.iter().any(|w| reason.contains(w))
}

/// The tool calls in one of the main conversation's events, as (tool_use_id, call_key, tool).
fn tool_calls(e: &Value) -> Vec<(String, String, String)> {
    if e["type"] != "assistant" || e["isSidechain"] == true {
        return vec![];
    }
    e["message"]["content"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|b| b["type"] == "tool_use")
                .filter_map(|b| b["id"].as_str().filter(|id| !id.is_empty()).map(|id| {
                    let tool = b["name"].as_str().unwrap_or("");
                    (id.to_string(), call_key(tool, &b["input"]), tool.to_string())
                }))
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

/// Programs that run the file named after them (`python3 ../tools/gen.py`).
/// `source` and `.` aren't among them: a file sourced sets up the shell, and adds no folder.
const INTERPRETERS: &[&str] = &["python", "python3", "node", "bash", "sh", "zsh", "fish", "ruby", "perl", "php", "lua", "deno", "bun", "tsx", "ts-node", "dart", "swift", "Rscript", "osascript"];
/// What runs the program after it (`sudo`, `env`, `time`, `npx`).
const WRAPPERS: &[&str] = &["sudo", "env", "time", "nice", "nohup", "exec", "command", "xargs", "npx", "bunx"];
/// Two words that run the program after them (`uv run`, `pnpm exec`).
const WRAPPER_PAIRS: &[(&str, &str)] = &[("uv", "run"), ("pnpm", "exec"), ("pnpm", "dlx"), ("yarn", "exec"), ("yarn", "dlx"), ("poetry", "run"), ("pipenv", "run"), ("bundle", "exec")];

/// A program's name without its folder (`/usr/bin/env` is `env`).
fn base_name(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

/// Where the program in `words` is: past the wrappers (by name, wherever they live), their flags,
/// and the `NAME=value` settings in front.
fn program_at(words: &[String]) -> usize {
    let mut i = 0;
    let mut wrapped = false;
    while i < words.len() {
        let w = words[i].as_str();
        let name = base_name(w);
        if WRAPPERS.contains(&name) {
            (i, wrapped) = (i + 1, true);
        } else if WRAPPER_PAIRS.iter().any(|(a, b)| name == *a && words.get(i + 1).map(|x| x.as_str()) == Some(b)) {
            (i, wrapped) = (i + 2, true);
        } else if (w.contains('=') && !w.starts_with('-')) || (wrapped && w.starts_with('-')) {
            i += 1;
        } else {
            break;
        }
    }
    i
}
/// A long flag that names the file a command builds or runs from (`--manifest-path`, `--config`).
static RUN_FLAG: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"^--[\w-]*(manifest|config|project|makefile|file|settings|rcfile)[\w-]*$").unwrap());
/// The words of a command that rewrites the sources its manifest names (`cargo fmt`, `eslint --fix`),
/// where building or testing from one (`cargo test --manifest-path ../Cargo.toml`) only reads them.
const REWRITES: &[&str] = &["fmt", "format", "fix", "--fix", "--write", "--apply", "generate", "codegen"];

/// The paths a shell command runs or builds from, rather than reads: the program itself
/// (`../tools/gen.sh`), the script an interpreter runs, and the manifest or config a long flag names
/// for a command that rewrites what it covers. What one of those writes lands anywhere near it, as
/// `cargo fmt --manifest-path ../Cargo.toml` rewrites `../lib/x.rs`; a `cat ../a.rs` only reads.
fn run_paths(command: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut add = |w: &str| {
        if w.contains('/') && !w.contains("://") && !w.contains('$') && !out.iter().any(|x| x == w) {
            out.push(w.to_string());
        }
    };
    let unquote = |w: &str| w.trim_matches(|c| c == '"' || c == '\'' || c == '`').to_string();
    for segment in command.split(|c: char| c == '\n' || ";&|()".contains(c)) {
        let words: Vec<String> = segment.split_whitespace().map(unquote).filter(|w| !w.is_empty()).collect();
        let i = program_at(&words);
        let Some(program) = words.get(i) else { continue };
        let name = base_name(program);
        if INTERPRETERS.contains(&name) || name.starts_with("python") {
            if let Some(script) = words[i + 1..].iter().find(|w| !w.starts_with('-')) {
                add(script);
            }
        } else if program.contains('/') {
            add(program);
        }
        if !words[i + 1..].iter().any(|w| REWRITES.contains(&w.as_str())) {
            continue;
        }
        for (j, w) in words.iter().enumerate().skip(i + 1) {
            let (flag, value) = match w.split_once('=') {
                Some((f, v)) => (f, Some(unquote(v))),
                None => (w.as_str(), words.get(j + 1).cloned()),
            };
            if RUN_FLAG.is_match(flag) {
                if let Some(v) = value {
                    add(&v);
                }
            }
        }
    }
    out
}

/// The tool results in one of the main conversation's events, as (tool_use_id, refused).
fn tool_results(e: &Value) -> Vec<(String, bool)> {
    if e["type"] != "user" || e["isSidechain"] == true {
        return vec![];
    }
    // The tool's own output, which Claude keeps beside a lone result; a refusal has only its words.
    let lone = e["message"]["content"].as_array().is_some_and(|a| a.iter().filter(|b| b["type"] == "tool_result").count() == 1);
    let output = lone && e["toolUseResult"].is_object();
    e["message"]["content"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|b| b["type"] == "tool_result")
                .filter_map(|b| b["tool_use_id"].as_str().filter(|id| !id.is_empty()).map(|id| (id.to_string(), refused(b, output))))
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
        if slash_command(e) {
            cur.since_prompt = cur.calls.len();
            continue;
        }
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
                    for p in run_paths(command) {
                        if !cur.shell_runs.contains(&p) {
                            cur.shell_runs.push(p);
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
        assert_eq!(t[0].calls, vec![("t1".to_string(), call_key("Bash", &json!({"command": "ls"})), "Bash".to_string())]);
    }

    #[test]
    fn an_error_for_a_call_is_a_refusal_only_when_it_reads_as_one() {
        let result = |content: &str| json!({"type": "tool_result", "tool_use_id": "t", "is_error": true, "content": content});
        for text in [
            "Hook PreToolUse:Bash denied this tool",
            "Error: Hook PreToolUse:Bash denied this tool",
            "Permission for this action was denied by the Claude Code auto mode classifier. Reason: it deletes files outside the project",
            "Permission for this action has been denied. Reason: the owner said no",
            "Permission for this tool use was denied. The tool use was rejected",
            "Error: Permission for this command was denied by a built-in Claude Code safety check, not by the user.",
            "<tool_use_error>Blocked: sleep 100 followed by: tail -15 x</tool_use_error>",
            "sprout: sprout refuses this write: \"/dev/null\" is outside this node's declared work",
            "BLOCKED: mutating command targets a path outside this repo",
            "Permission for this action was denied. Reason: the command timed out last time",
            "sprout: blocked, the last run timed out",
            "Hook said no: interrupted work isn't allowed here, it's not allowed",
        ] {
            assert!(refused(&result(text), false), "{text}");
        }
        for text in [
            "Exit code 1\nerror: test failed",
            "Error: Exit code 137",
            "[Tool call interrupted: the session ended]",
            "Command timed out after 2m 0s",
            "Interrupted by user",
            "the stream was cut off",
            "API Error: 529 {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\"}}",
            "Prompt is too long",
            "Agent stopped: max turns reached",
            "Command was aborted before completion",
            "Command was killed with SIGKILL",
            "Error: Command failed",
            "error: could not compile `app`",
            "bash: ./x.sh: Permission denied",
            "curl: (7) Failed to connect to localhost port 3000: Connection refused",
            "Exit code 1\nthe hook blocked it",
        ] {
            assert!(!refused(&result(text), false), "{text}");
        }
        assert!(!refused(&result("blocked: whatever it printed"), true), "the tool's own output: it ran");
        assert!(!refused(&json!({"type": "tool_result", "tool_use_id": "t", "content": "Permission for this action"}), false), "not an error");
        let ev = |result: Value, output: Value| json!({"type": "user", "message": {"content": [result]}, "toolUseResult": output});
        assert_eq!(tool_results(&ev(result("odd failure"), json!({"stdout": "", "stderr": "odd failure"}))), vec![("t".to_string(), false)]);
        assert_eq!(tool_results(&ev(result("odd failure"), json!("Error: odd failure"))), vec![("t".to_string(), false)], "a string result is no refusal by itself");
        assert_eq!(tool_results(&ev(result("Blocked: outside the repo"), json!("Error: Blocked: outside the repo"))), vec![("t".to_string(), true)]);
    }

    #[test]
    fn every_refusal_wording_reads_as_one_and_a_deny_word_in_a_failure_doesnt() {
        let result = |content: &str| json!({"type": "tool_result", "tool_use_id": "t", "is_error": true, "content": content});
        let worded = [
            "The user doesn't want to proceed with this tool use. The tool use was rejected.",
            "The user doesn't want to take this action right now. STOP what you are doing.",
            "Permission to use Bash has been denied.",
            "Permission for this action has been denied. Reason: not now",
            "This command requires approval",
            "This command uses shell operators that require approval for safety",
            "This Bash command contains multiple operations. The following parts require approval: rm x",
            "Contains expansion",
            "PreToolUse:Bash hook error: no",
            "The server-side auto mode classifier gave no verdict for this action. Try again.",
            "This command changes directory before running a version-control command. Run git with -C instead.",
            "This session is isolated in the worktree /tmp/wt but this command points git at a directory outside it.",
            "This agent is isolated in the worktree /tmp/wt, but this command is too complex to verify that it stays inside the worktree.",
            "Parser skipped input between top-level statements",
        ];
        assert_eq!(worded.len(), REFUSALS.len(), "a row for every wording");
        for r in REFUSALS {
            assert!(worded.iter().any(|w| w.starts_with(r)), "{r}");
        }
        for text in worded.iter().copied().chain(["Error: Parser skipped input between top-level statements", "policy: network is cut off for this session"]) {
            assert!(refused(&result(text), false), "{text}");
        }
        for text in [
            "! [rejected]        main -> main (fetch first)",
            "To github.com:acme/webapp.git\n ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs",
            "403 Forbidden",
            "remote: Pushing to main is not allowed",
            "fatal: push to main is not allowed",
            "API Error: 403 {\"type\":\"error\",\"error\":{\"type\":\"permission_error\",\"message\":\"Request not allowed\"}}",
            "API Error: 400 {\"type\":\"error\",\"error\":{\"message\":\"Output blocked by content filtering policy\"}}",
            "Agent stopped: a tool use was rejected",
        ] {
            assert!(!refused(&result(text), false), "{text}");
        }
    }

    #[test]
    fn an_auto_compact_mid_turn_keeps_the_turns_calls() {
        let call = |id: &str| json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": id, "name": "Bash", "input": {"command": "ls"}}]}});
        let ev = vec![
            json!({"type": "user", "message": {"content": "Go"}, "timestamp": "2026-10-01T10:00:00Z"}),
            call("t1"),
            json!({"type": "system", "subtype": "compact_boundary"}),
            json!({"type": "user", "isCompactSummary": true, "isVisibleInTranscriptOnly": true, "message": {"content": "This session is being continued from a previous conversation."}}),
            call("t2"),
        ];
        let t = turns_of(&ev);
        assert_eq!(t.len(), 1, "the summary isn't a prompt");
        assert_eq!(t[0].calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), vec!["t1", "t2"]);
    }

    #[test]
    fn a_slash_command_starts_the_hooks_calls_over_but_not_the_turn() {
        let ev = vec![
            json!({"type": "user", "message": {"content": "Go"}, "timestamp": "2026-10-01T10:00:00Z"}),
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}}]}}),
            json!({"type": "user", "message": {"content": "<command-name>/review</command-name>\n<command-message>review</command-message>"}}),
            json!({"type": "user", "message": {"content": "<system-reminder>x</system-reminder>"}}),
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t2", "name": "Bash", "input": {"command": "pwd"}}]}}),
        ];
        let t = turns_of(&ev);
        assert_eq!(t.len(), 1, "a slash command is neutral to the turn");
        assert_eq!(t[0].calls.len(), 2);
        assert_eq!(t[0].current_calls().iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), vec!["t2"]);
    }

    #[test]
    fn reads_the_files_a_command_runs_or_builds_from() {
        assert_eq!(run_paths("python3 ../tools/gen.py --out x && FOO=1 sudo ./build.sh; cat ../a.rs | head ../b.rs"), vec!["../tools/gen.py", "./build.sh"]);
        assert_eq!(run_paths("cargo fmt --manifest-path ../Cargo.toml; npx prettier --write --config=../web/.prettierrc src"), vec!["../Cargo.toml", "../web/.prettierrc"]);
        assert!(run_paths("cargo test --manifest-path ../Cargo.toml").is_empty(), "a build or a test only reads what its manifest names");
        assert!(run_paths("ls ../tools/ && grep -r x ../lib && python3 -m pytest").is_empty());
        assert_eq!(run_paths("node 'scripts/gen.js'\nbash -x ../ci/run.sh"), vec!["scripts/gen.js", "../ci/run.sh"]);
        for (command, script) in [
            ("/usr/bin/env python3 ../tools/gen.py", "../tools/gen.py"),
            ("uv run python ../tools/gen.py", "../tools/gen.py"),
            ("npx tsx ../tools/gen.ts", "../tools/gen.ts"),
            ("pnpm exec tsx ../tools/gen.ts", "../tools/gen.ts"),
            ("bunx tsx ../tools/gen.ts", "../tools/gen.ts"),
            ("/opt/homebrew/bin/python3 ../tools/gen.py", "../tools/gen.py"),
            ("env -i FOO=1 node ../x.js", "../x.js"),
        ] {
            assert_eq!(run_paths(command), vec![script], "{command}");
        }
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
