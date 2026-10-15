//! Command timings: every tool call in the Claude transcripts of the sessions the board knows, read
//! incrementally by file offset and kept as a span (task, category, seconds, exit, load at start).

use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{midna, p, transcript};

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS span_files(
  path TEXT PRIMARY KEY, claude_id TEXT, session_id TEXT, project TEXT, task_id INT,
  parent TEXT, offset INT DEFAULT 0, pending TEXT, updated_at TEXT);
CREATE TABLE IF NOT EXISTS spans(
  id INTEGER PRIMARY KEY, tool_use_id TEXT UNIQUE, claude_id TEXT, session_id TEXT, task_id INT,
  project TEXT, tool TEXT, category TEXT, fingerprint TEXT, command TEXT, started_at TEXT,
  seconds REAL, exit_code INT, timed_out INT DEFAULT 0, background INT DEFAULT 0,
  refused INT DEFAULT 0, load REAL, subagent INT DEFAULT 0);
CREATE INDEX IF NOT EXISTS spans_started ON spans(started_at);
CREATE INDEX IF NOT EXISTS spans_task ON spans(task_id);
"#;

/// `[spans]` in config.toml. `rules` come before the built-in ones: `{ pattern = "...", category = "..." }`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SpansConfig {
    pub on: bool,
    pub every_secs: u64,
    pub rules: Vec<Rule>,
}

impl Default for SpansConfig {
    fn default() -> Self {
        SpansConfig { on: true, every_secs: 30, rules: Vec::new() }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub pattern: String,
    pub category: String,
}

/// Bytes read from one transcript per scan, so a first pass over a long history doesn't stall the loop.
const READ_PER_SCAN: u64 = 8 * 1024 * 1024;
/// A call this recent when the scan first sees it gets the load midna reports now.
const FRESH_SECS: f64 = 120.0;
/// A call with no result this long before the transcript's newest line never gets one.
const PENDING_SECS: f64 = 24.0 * 3600.0;
const COMMAND_CHARS: usize = 500;

const DEFAULT_RULES: &[(&str, &str)] = &[
    ("load wait", r"\bmidna\s+system\b|\buptime\b|\bsysctl\s+vm\.loadavg"),
    ("ci wait", r"\bgh\s+pr\s+checks\b|\bgh\s+run\s+(watch|view)\b|\btb\s+pr\s+wait\b"),
    ("background poll", r"^(BashOutput|TaskOutput|KillShell)\b|\btail\b.*\.output\b|^\s*sleep\s+\d+\s*(;|&&|$)|\bwhile\b.*\bsleep\b"),
    ("lint", r"\bclippy\b|\beslint\b|\b(flutter|dart)\s+analyze\b|\bswiftlint\b|\bruff\b"),
    ("test", r"\bcargo\s+(test|nextest)\b|\b(flutter|dart|go|swift|npm|pnpm|yarn|bun)\s+(run\s+)?test\b|\bpytest\b|\bxcodebuild\b.*\btest\b|\bvitest\b|\bjest\b"),
    ("build", r"\bcargo\s+(build|run|check|install)\b|\bxcodebuild\b|\bflutter\s+build\b|\bswift\s+build\b|\b(npm|pnpm|yarn)\s+run\s+build\b|\bmake\b|\btsc\b"),
    ("git", r"(^|[;&|(]\s*)(git|gh)\s"),
];

static TASK_TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[task-board:T(\d+)\]").unwrap());
static EXIT_CODE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^Exit code (-?\d+)").unwrap());
static BUSY_LOAD: Lazy<Regex> = Lazy::new(|| Regex::new(r"the Mac is busy \(load [\d.]+ on \d+ cores, (\d+)%").unwrap());
static CD_PREFIX: Lazy<Regex> = Lazy::new(|| Regex::new(r#"^\s*cd\s+("[^"]*"|'[^']*'|\S+)\s*&&\s*"#).unwrap());
static ENV_PREFIX: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*([A-Z_][A-Z0-9_]*=\S*\s+)+").unwrap());
static PATHS: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(~|\.{0,2})/[^\s;&|()'"]+"#).unwrap());
static NUMBERS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b\d+\b").unwrap());
static SPACES: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());

/// The category the owner's rules, then the built-in ones, give a call.
pub fn category(rules: &[Rule], tool: &str, command: &str) -> String {
    let text = if tool == "Bash" { command.to_string() } else { format!("{tool} {command}") };
    for r in rules {
        if Regex::new(&r.pattern).is_ok_and(|re| re.is_match(&text)) {
            return r.category.clone();
        }
    }
    static BUILT_IN: Lazy<Vec<(&str, Regex)>> = Lazy::new(|| DEFAULT_RULES.iter().map(|(c, p)| (*c, Regex::new(p).unwrap())).collect());
    BUILT_IN.iter().find(|(_, re)| re.is_match(&text)).map(|(c, _)| c.to_string()).unwrap_or_else(|| "other".into())
}

/// The command with what changes between runs of the same thing (its directory, env, paths and numbers) taken out.
pub fn normalized(tool: &str, command: &str) -> String {
    let c = CD_PREFIX.replace(command, "");
    let c = ENV_PREFIX.replace(&c, "");
    let c = PATHS.replace_all(&c, "<path>");
    let c = NUMBERS.replace_all(&c, "N");
    format!("{tool} {}", SPACES.replace_all(c.trim(), " "))
}

pub fn fingerprint(tool: &str, command: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in normalized(tool, command).bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn command_of(tool: &str, input: &Value) -> String {
    let s = |k: &str| input[k].as_str().map(str::to_string);
    let c = match tool {
        "Bash" => s("command"),
        "BashOutput" | "TaskOutput" | "KillShell" => s("bash_id").or_else(|| s("task_id")).or_else(|| s("shell_id")),
        _ => s("command").or_else(|| s("file_path")).or_else(|| s("pattern")).or_else(|| s("description")).or_else(|| s("url")),
    };
    c.unwrap_or_default().chars().take(COMMAND_CHARS).collect()
}

fn result_text(b: &Value) -> String {
    match &b["content"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn prompt_text(e: &Value) -> Option<String> {
    match &e["message"]["content"] {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) if !a.iter().any(|b| b["type"] == "tool_result") => {
            Some(a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"))
        }
        _ => None,
    }
}

/// How one call ended: `(exit_code, timed_out, background, refused, load from a busy refusal)`.
fn outcome(block: &Value, result: &Value) -> (Option<i64>, bool, bool, bool, Option<f64>) {
    let text = result_text(block);
    let refused = text.starts_with("PreToolUse:") || text.contains("hook error:") || text.contains("the Mac is busy");
    let busy = BUSY_LOAD.captures(&text).and_then(|c| c[1].parse().ok());
    let timed_out = result["timedOutAfterMs"].is_number() || text.contains("did not complete within its");
    let background = result["backgroundTaskId"].is_string();
    let exit = if refused || result["interrupted"] == true {
        None
    } else if block["is_error"] == true {
        EXIT_CODE.captures(&text).and_then(|c| c[1].parse().ok()).or(Some(1))
    } else {
        Some(0)
    };
    (exit, timed_out, background, refused, busy)
}

struct File {
    path: PathBuf,
    claude_id: String,
    session_id: Option<String>,
    project: Option<String>,
    task_id: Option<i64>,
    subagent: bool,
}

/// Midna's load (percent of all cores), asked once per scan and only when a fresh call needs it.
struct Load<'a> {
    app: &'a App,
    got: Option<Option<f64>>,
}

impl Load<'_> {
    fn now(&mut self) -> Option<f64> {
        *self.got.get_or_insert_with(|| {
            midna::call_exe(&self.app.cfg.midna, "system.load", &json!({}), 5.0).ok().and_then(|v| v["load_percent"].as_f64())
        })
    }
}

/// The transcripts to read: each conversation the board has a claude id for, every file read before,
/// and the subagent files beside them.
fn known(app: &App) -> Result<Vec<File>> {
    let root = &app.cfg.claude_projects;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for r in app.db.q("SELECT path, claude_id, session_id, project, task_id, parent FROM span_files", vec![])? {
        let path = PathBuf::from(r.s("path").unwrap_or_default());
        seen.insert(path.clone());
        out.push(File {
            path,
            claude_id: r.s("claude_id").unwrap_or_default().to_string(),
            session_id: r.s("session_id").map(str::to_string),
            project: r.s("project").map(str::to_string),
            task_id: r.i("task_id"),
            subagent: r.s("parent").is_some(),
        });
    }
    let ids = app.db.q(
        "SELECT s.claude_session_id AS cid, s.id AS sid, s.project, s.project_path FROM sessions s WHERE s.claude_session_id IS NOT NULL
         UNION SELECT t.claude_session_id, t.session_id, t.project, s.project_path FROM tasks t LEFT JOIN sessions s ON s.id = t.session_id
         WHERE t.claude_session_id IS NOT NULL",
        vec![],
    )?;
    for r in ids {
        let Some(cid) = r.s("cid").filter(|c| !c.is_empty()) else { continue };
        let path = transcript::transcript_file(root, r.s("project_path").unwrap_or(""), cid);
        if path.is_file() && seen.insert(path.clone()) {
            out.push(File {
                path,
                claude_id: cid.to_string(),
                session_id: r.s("sid").map(str::to_string),
                project: r.s("project").map(str::to_string),
                task_id: None,
                subagent: false,
            });
        }
    }
    let mains: Vec<(PathBuf, String, Option<String>, Option<String>)> =
        out.iter().filter(|f| !f.subagent).map(|f| (f.path.clone(), f.claude_id.clone(), f.session_id.clone(), f.project.clone())).collect();
    for (main, cid, sid, project) in mains {
        let dir = main.with_extension("").join("subagents");
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = e.path();
            if path.extension().is_some_and(|x| x == "jsonl") && seen.insert(path.clone()) {
                app.db.x(
                    "INSERT OR IGNORE INTO span_files(path, claude_id, session_id, project, parent, offset) VALUES(?, ?, ?, ?, ?, 0)",
                    p![path.to_string_lossy(), cid, sid, project, main.to_string_lossy()],
                )?;
                out.push(File { path, claude_id: cid.clone(), session_id: sid.clone(), project: project.clone(), task_id: None, subagent: true });
            }
        }
    }
    Ok(out)
}

/// The task a conversation with no tag of its own works on: one that names its claude id, else its terminal's.
fn fallback_task(app: &App, f: &File) -> Result<Option<i64>> {
    if let Some(t) = app.db.q1("SELECT id FROM tasks WHERE claude_session_id = ? ORDER BY updated_at DESC LIMIT 1", p![f.claude_id])? {
        return Ok(t.i("id"));
    }
    let current = app.db.count("SELECT COUNT(*) FROM sessions WHERE id = ? AND claude_session_id = ?", p![f.session_id, f.claude_id])? > 0;
    if !current {
        return Ok(None);
    }
    Ok(crate::board::task_for_session(app, f.session_id.as_deref())?.and_then(|t| t.i("id")))
}

fn task_project(app: &App, task: Option<i64>) -> Result<Option<String>> {
    Ok(match task {
        Some(t) => app.db.q1("SELECT project FROM tasks WHERE id = ?", p![t])?.and_then(|r| r.s("project").map(str::to_string)),
        None => None,
    })
}

/// Reads what `f` gained since the last scan and stores its finished calls. Returns how many.
fn read_file(app: &App, f: &File, load: &mut Load) -> Result<usize> {
    let key = f.path.to_string_lossy().to_string();
    let row = app.db.q1("SELECT offset, pending, task_id, parent FROM span_files WHERE path = ?", p![key])?;
    let offset = row.as_ref().and_then(|r| r.i("offset")).unwrap_or(0) as u64;
    let Ok(meta) = std::fs::metadata(&f.path) else { return Ok(0) };
    if meta.len() <= offset && row.is_some() {
        return Ok(0);
    }
    let mut pending: BTreeMap<String, Value> =
        row.as_ref().and_then(|r| r.s("pending")).and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
    let mut task = row.as_ref().and_then(|r| r.i("task_id")).or(f.task_id);
    let parent = row.as_ref().and_then(|r| r.s("parent").map(str::to_string));
    if let Some(p) = &parent {
        task = app.db.q1("SELECT task_id FROM span_files WHERE path = ?", p![p])?.and_then(|r| r.i("task_id")).or(task);
    }
    if task.is_none() {
        task = fallback_task(app, f)?;
    }

    let Ok(file) = std::fs::File::open(&f.path) else { return Ok(0) };
    let mut reader = BufReader::new(file);
    if reader.seek(SeekFrom::Start(offset)).is_err() {
        return Ok(0);
    }
    let mut consumed = 0u64;
    let mut buf = Vec::new();
    let mut spans = Vec::new();
    let now = now_ts();
    let mut newest: Option<f64> = None;
    while consumed < READ_PER_SCAN {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf).unwrap_or(0);
        if n == 0 || buf.last() != Some(&b'\n') {
            break;
        }
        consumed += n as u64;
        let Ok(e) = serde_json::from_slice::<Value>(&buf) else { continue };
        let at = e["timestamp"].as_str().unwrap_or("").to_string();
        newest = parse_iso(&at).or(newest);
        match e["type"].as_str() {
            Some("assistant") => {
                for b in e["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_use") {
                    let (Some(id), Some(tool)) = (b["id"].as_str(), b["name"].as_str()) else { continue };
                    let fresh = parse_iso(&at).is_some_and(|t| now - t < FRESH_SECS);
                    let l = if fresh { load.now() } else { None };
                    pending.insert(id.into(), json!({"tool": tool, "command": command_of(tool, &b["input"]), "at": at, "task": task, "load": l}));
                }
            }
            Some("user") => {
                if !f.subagent {
                    if let Some(t) = prompt_text(&e).and_then(|p| TASK_TAG.captures(&p).and_then(|c| c[1].parse::<i64>().ok())) {
                        task = Some(t);
                    }
                }
                for b in e["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    let Some(start) = b["tool_use_id"].as_str().and_then(|id| pending.remove(id).map(|s| (id.to_string(), s))) else { continue };
                    spans.push((start, b.clone(), e["toolUseResult"].clone(), at.clone()));
                }
            }
            _ => {}
        }
    }
    if let Some(newest) = newest {
        pending.retain(|_, s| s["at"].as_str().and_then(parse_iso).is_none_or(|t| newest - t < PENDING_SECS));
    }

    let fallback_project = f.project.clone();
    let stored = spans.len();
    app.db.tx(|| {
        for ((id, s), block, result, ended) in &spans {
            let tool = s["tool"].as_str().unwrap_or("");
            let command = s["command"].as_str().unwrap_or("");
            let started = s["at"].as_str().unwrap_or("");
            let seconds = match (parse_iso(started), parse_iso(ended)) {
                (Some(a), Some(b)) => Some((b - a).max(0.0)),
                _ => None,
            };
            let (exit, timed_out, background, refused, busy) = outcome(block, result);
            let task_id = s["task"].as_i64();
            let project = task_project(app, task_id)?.or_else(|| fallback_project.clone());
            app.db.x(
                "INSERT OR IGNORE INTO spans(tool_use_id, claude_id, session_id, task_id, project, tool, category, fingerprint, command,
                 started_at, seconds, exit_code, timed_out, background, refused, load, subagent) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                p![
                    id,
                    f.claude_id,
                    f.session_id,
                    task_id,
                    project,
                    tool,
                    if busy.is_some() { "load wait".to_string() } else { category(&app.cfg.spans.rules, tool, command) },
                    fingerprint(tool, command),
                    command,
                    started,
                    seconds,
                    exit,
                    timed_out,
                    background,
                    refused,
                    s["load"].as_f64().or(busy),
                    f.subagent
                ],
            )?;
        }
        app.db.x(
            "INSERT INTO span_files(path, claude_id, session_id, project, task_id, parent, offset, pending, updated_at) VALUES(?,?,?,?,?,?,?,?,?)
             ON CONFLICT(path) DO UPDATE SET task_id = excluded.task_id, offset = excluded.offset, pending = excluded.pending, updated_at = excluded.updated_at",
            p![key, f.claude_id, f.session_id, f.project, task, parent, (offset + consumed) as i64, serde_json::to_string(&pending).unwrap_or_default(), now_iso()],
        )?;
        Ok(())
    })?;
    Ok(stored)
}

/// One pass over every known transcript. Returns how many spans it stored.
pub fn scan(app: &App) -> Result<usize> {
    let mut load = Load { app, got: None };
    let mut n = 0;
    for f in known(app)? {
        n += read_file(app, &f, &mut load)?;
    }
    Ok(n)
}

pub fn run(app: std::sync::Arc<App>) {
    while !app.stopping() {
        if app.cfg.spans.on {
            if let Err(e) = scan(&app) {
                app.info(format!("spans: {}", e.message));
            }
        }
        app.sleep(app.cfg.spans.every_secs.max(5) as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat(c: &str) -> String {
        category(&[], "Bash", c)
    }

    #[test]
    fn categories_follow_the_time_review() {
        assert_eq!(cat("cd /x && CARGO_TARGET_DIR=t cargo test -p taskboardd --test it spans"), "test");
        assert_eq!(cat("cargo build --release"), "build");
        assert_eq!(cat("cargo clippy --all-targets -- -D warnings"), "lint");
        assert_eq!(cat("gh pr checks 12 --watch"), "ci wait");
        assert_eq!(cat("midna system --json"), "load wait");
        assert_eq!(cat("sleep 60"), "background poll");
        assert_eq!(category(&[], "BashOutput", "b1"), "background poll");
        assert_eq!(cat("git push -u origin feat/x"), "git");
        assert_eq!(cat("ls -la"), "other");
        assert_eq!(category(&[], "Read", "/x/src/main.rs"), "other");
        let rules = vec![Rule { pattern: r"qa\.sh".into(), category: "qa".into() }];
        assert_eq!(category(&rules, "Bash", "./scripts/qa.sh --device 2"), "qa");
    }

    #[test]
    fn fingerprints_ignore_directories_paths_and_numbers() {
        let a = fingerprint("Bash", "cd /a/wt-1 && cargo test -p x --test it 2>&1 | tail -40");
        let b = fingerprint("Bash", "cd /b/wt-2 && cargo test -p x --test it 2>&1 | tail -80");
        assert_eq!(a, b);
        assert_ne!(a, fingerprint("Bash", "cargo build -p x"));
        assert_eq!(normalized("Bash", "FOO=1 BAR=2 tail -5 /tmp/x.log"), "Bash tail -N <path>");
    }

    #[test]
    fn outcomes_read_exit_codes_timeouts_and_refusals() {
        let ok = outcome(&json!({"content": "done"}), &json!({}));
        assert_eq!(ok, (Some(0), false, false, false, None));
        let failed = outcome(&json!({"content": "Exit code 101\nerror", "is_error": true}), &json!({}));
        assert_eq!(failed.0, Some(101));
        let timed = outcome(&json!({"content": "Command did not complete within its 600s timeout and was moved to the background (ID: b1)"}), &json!({"timedOutAfterMs": 600000, "backgroundTaskId": "b1"}));
        assert_eq!((timed.1, timed.2), (true, true));
        let busy = outcome(
            &json!({"content": "PreToolUse:Bash hook error: midna: the Mac is busy (load 92.97 on 12 cores, 775%; busy from 600%).", "is_error": true}),
            &json!({}),
        );
        assert_eq!(busy, (None, false, false, true, Some(775.0)));
    }
}
