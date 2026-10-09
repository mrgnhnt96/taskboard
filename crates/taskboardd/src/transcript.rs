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
const TAIL_BYTES: u64 = 2 * 1024 * 1024;
const CLAUDE_STOPS_BACKGROUND_AFTER: f64 = 2.0 * 3600.0;

static DASH: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"[^A-Za-z0-9]").unwrap());
static BG_STARTED: Lazy<Regex> = Lazy::new(|| Regex::new(r#""backgroundTaskId":\s*"([^"]+)""#).unwrap());
static BG_ENDED: Lazy<Regex> = Lazy::new(|| Regex::new(r"<task-id>([^<]+)</task-id>").unwrap());

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

/// How many background commands in this transcript started (under two hours ago) and haven't reported back.
pub fn background_running(root: &Path, path: Option<&str>) -> Option<usize> {
    let p = std::fs::canonicalize(expand_home(path?)).ok()?;
    let root = std::fs::canonicalize(root).ok()?;
    if p.extension().map(|e| e != "jsonl").unwrap_or(true) || !p.starts_with(&root) {
        return None;
    }
    let data = std::fs::read(&p).ok()?;
    let ended: Vec<&[u8]> = BG_ENDED.captures_iter(&data).filter_map(|c| c.get(1).map(|m| m.as_bytes())).collect();
    let mut running = 0;
    for line in data.split(|b| *b == b'\n') {
        let Some(c) = BG_STARTED.captures(line) else { continue };
        if ended.contains(&c.get(1).unwrap().as_bytes()) {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<Value>(line) else { continue };
        let Some(at) = v["timestamp"].as_str().and_then(parse_iso) else { continue };
        if crate::clock::awake_since(at) < CLAUDE_STOPS_BACKGROUND_AFTER {
            running += 1;
        }
    }
    Some(running)
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

pub struct Turn {
    pub at: Value,
    pub prompt: String,
    pub files: Vec<String>,
    pub closed: bool,
}

fn turns_of(events: &[Value]) -> Vec<Turn> {
    let mut out: Vec<Turn> = vec![];
    for e in events {
        if let Some(p) = prompt_of(e) {
            out.push(Turn { at: e["timestamp"].clone(), prompt: p, files: vec![], closed: false });
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
        }
    }
    out
}

pub fn last_turn(app: &App, s: &Row) -> Option<Turn> {
    let p = session_path(app, s)?;
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
