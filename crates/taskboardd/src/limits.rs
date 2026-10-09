//! Context limits: how big a board agent's conversation may get before Claude compacts it, when a
//! conversation is too cold to resume without compacting first, and when a task or PR conversation is
//! too big or too old to resume at all (it starts fresh from the handoff instead). Also the
//! generated-file globs kept in each project's `.git/info/attributes` (`gitattrs.rs`).
//!
//! `[limits]` in config.toml is where they start; `tb limits` changes them on the board.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{proc, transcript};

const KEY: &str = "limits";
pub const NUMBERS: [&str; 4] = ["compact_window", "cold_idle_mins", "warm_tokens", "warm_idle_mins"];
/// How long a headless `/compact` may take before the resume goes ahead without it.
const COMPACT_TIMEOUT: f64 = 600.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Limits {
    pub compact_window: i64,
    pub cold_idle_mins: i64,
    pub warm_tokens: i64,
    pub warm_idle_mins: i64,
    pub generated: Vec<String>,
    pub project_generated: BTreeMap<String, Vec<String>>,
}

impl Limits {
    fn set_number(&mut self, k: &str, v: i64) {
        match k {
            "compact_window" => self.compact_window = v,
            "cold_idle_mins" => self.cold_idle_mins = v,
            "warm_tokens" => self.warm_tokens = v,
            _ => self.warm_idle_mins = v,
        }
    }
    /// The globs for one project: everyone's, then its own.
    pub fn globs_for(&self, project: Option<&str>) -> Vec<String> {
        let mut out = self.generated.clone();
        for g in project.and_then(|p| self.project_generated.get(p)).into_iter().flatten() {
            if !out.contains(g) {
                out.push(g.clone());
            }
        }
        out
    }
    pub fn to_json(&self) -> Value {
        json!({"compact_window": self.compact_window, "cold_idle_mins": self.cold_idle_mins,
               "warm_tokens": self.warm_tokens, "warm_idle_mins": self.warm_idle_mins,
               "generated": self.generated, "project_generated": self.project_generated})
    }
}

fn defaults(app: &App) -> Limits {
    let c = &app.cfg.limits;
    Limits {
        compact_window: c.compact_window.max(0),
        cold_idle_mins: c.cold_idle_mins.max(0),
        warm_tokens: c.warm_tokens.max(0),
        warm_idle_mins: c.warm_idle_mins.max(0),
        generated: clean_globs(&c.generated),
        project_generated: c.project_generated.iter().map(|(k, v)| (k.clone(), clean_globs(v))).filter(|(_, v)| !v.is_empty()).collect(),
    }
}

fn clean_globs(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for g in list.iter().map(|g| g.trim()).filter(|g| !g.is_empty()) {
        if !out.iter().any(|x| x == g) {
            out.push(g.to_string());
        }
    }
    out
}

fn globs_of(v: &Value) -> Result<Vec<String>> {
    match v {
        Value::Null => Ok(vec![]),
        Value::String(s) if s.trim().eq_ignore_ascii_case("none") => Ok(vec![]),
        Value::String(s) => Ok(clean_globs(&s.split(',').map(|x| x.to_string()).collect::<Vec<_>>())),
        Value::Array(a) => Ok(clean_globs(&a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>())),
        _ => err(400, "Give the generated files as globs: a list, or one string split by commas."),
    }
}

fn str_globs(v: Option<&Value>) -> Option<Vec<String>> {
    v.and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
}

/// The board's limits: config.toml's, with what `tb limits` changed on top.
pub fn get(app: &App) -> Limits {
    let mut l = defaults(app);
    let saved = jloads_obj(app.db.get_setting(KEY).ok().flatten().as_deref());
    for k in NUMBERS {
        if let Some(n) = saved.get(k).and_then(|v| v.as_i64()) {
            l.set_number(k, n.max(0));
        }
    }
    if let Some(g) = str_globs(saved.get("generated")) {
        l.generated = clean_globs(&g);
    }
    if let Some(m) = saved.get("project_generated").and_then(|v| v.as_object()) {
        for (p, v) in m {
            match str_globs(Some(v)) {
                Some(g) if !g.is_empty() => {
                    l.project_generated.insert(p.clone(), clean_globs(&g));
                }
                _ => {
                    l.project_generated.remove(p);
                }
            }
        }
    }
    l
}

/// Changes the limits. Numbers are tokens or minutes (0 turns one off, null puts config.toml's back);
/// `generated` replaces the globs, for `project` when it's given; `reset` forgets every change.
pub fn set(app: &App, body: &Value) -> Result<Limits> {
    let mut saved = if as_bool(body.get("reset"), false) { Row::new() } else { jloads_obj(app.db.get_setting(KEY)?.as_deref()) };
    for k in NUMBERS {
        if !body.as_object().is_some_and(|o| o.contains_key(k)) {
            continue;
        }
        match &body[k] {
            Value::Null => {
                saved.remove(k);
            }
            v => {
                let n = v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().replace(['_', ','], "").parse().ok()));
                match n {
                    Some(n) if n >= 0 => {
                        saved.insert(k.into(), json!(n));
                    }
                    _ => return err(400, format!("{k} must be a whole number, 0 or more (0 turns it off).")),
                }
            }
        }
    }
    if body.as_object().is_some_and(|o| o.contains_key("generated")) {
        let globs = globs_of(&body["generated"])?;
        let project = body_str(body, "project");
        if project.is_empty() {
            saved.insert("generated".into(), json!(globs));
        } else {
            let mut m = saved.get("project_generated").and_then(|v| v.as_object()).cloned().unwrap_or_default();
            m.insert(project, json!(globs));
            saved.insert("project_generated".into(), Value::Object(m));
        }
    }
    app.db.set_setting(KEY, Some(&jdumps(&Value::Object(saved))))?;
    Ok(get(app))
}

pub fn tokens_text(n: i64) -> String {
    if n >= 1000 && n % 1000 == 0 {
        format!("{}k", n / 1000)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

pub fn mins_text(m: i64) -> String {
    match (m / 60, m % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

pub fn line(l: &Limits) -> String {
    let off = |n: i64, f: &dyn Fn(i64) -> String| if n > 0 { f(n) } else { "off".into() };
    let mut parts = vec![
        format!("Compact window: {}", off(l.compact_window, &|n| format!("{} tokens", tokens_text(n)))),
        format!("Compact before resuming after: {}", off(l.cold_idle_mins, &|n| format!("{} idle", mins_text(n)))),
        format!("Resume only under: {}", off(l.warm_tokens, &|n| format!("{} tokens", tokens_text(n)))),
        format!("Resume only when idle under: {}", off(l.warm_idle_mins, &mins_text)),
    ];
    parts.push(format!("Generated files: {}", if l.generated.is_empty() { "none".into() } else { l.generated.join(", ") }));
    for (p, g) in &l.project_generated {
        parts.push(format!("Generated files in {p}: {}", g.join(", ")));
    }
    parts.join("\n")
}

/// `GET /limits`.
pub fn state(app: &App) -> Value {
    let l = get(app);
    let mut out = l.to_json();
    out["defaults"] = defaults(app).to_json();
    out["line"] = json!(line(&l));
    out
}

/// The context-limit part of a board terminal's `--settings` (`builds::settings_arg` adds its build env).
pub fn settings(app: &App) -> serde_json::Map<String, Value> {
    let n = get(app).compact_window;
    let mut s = serde_json::Map::new();
    if n > 0 {
        s.insert("autoCompactWindow".into(), json!(n));
    }
    s
}

/// A conversation's size and how long it's been idle (wall-clock minutes), as far as the board can tell.
pub struct Conversation {
    pub tokens: Option<i64>,
    pub idle_mins: Option<f64>,
}

pub fn conversation(app: &App, cwd: &str, cid: &str, last_activity: Option<&str>) -> Conversation {
    let size = transcript::conversation_size(&app.cfg.claude_projects, cwd, cid);
    let tokens = size.as_ref().and_then(|s| s.tokens);
    let at = size.and_then(|s| s.last_at).or(last_activity.map(|s| s.to_string()));
    Conversation { tokens, idle_mins: wall_age_secs(at.as_deref()).map(|s| s.max(0.0) / 60.0) }
}

/// Why a task or PR conversation shouldn't be resumed, but started fresh from the handoff: it's
/// bigger or has been idle longer than a resume may carry. None: resume it.
pub fn fresh_start_why(app: &App, c: &Conversation) -> Option<String> {
    let l = get(app);
    if let Some(n) = c.tokens.filter(|n| l.warm_tokens > 0 && *n > l.warm_tokens) {
        return Some(format!("its conversation is {} tokens, over the {} a resume may carry", tokens_text(n), tokens_text(l.warm_tokens)));
    }
    if let Some(m) = c.idle_mins.filter(|m| l.warm_idle_mins > 0 && *m > l.warm_idle_mins as f64) {
        return Some(format!("its conversation has been idle {}, over the {} a resume may wait", mins_text(m as i64), mins_text(l.warm_idle_mins)));
    }
    None
}

/// A conversation idle long enough that it's compacted before it carries on.
pub fn is_cold(app: &App, c: &Conversation) -> bool {
    let cold = get(app).cold_idle_mins;
    cold > 0 && c.idle_mins.is_some_and(|m| m > cold as f64)
}

/// Compacts a conversation that isn't open anywhere with a headless `claude -p /compact --resume`.
/// No user settings or hooks load into the run, and its JSON result says whether it worked.
pub fn compact(app: &App, cwd: &str, cid: &str) -> std::result::Result<(), String> {
    let Some(claude) = proc::which(&app.cfg.claude) else { return Err(format!("couldn't find {}", app.cfg.claude)) };
    let args: Vec<String> = ["-p", "/compact", "--resume", cid, "--output-format", "json", "--setting-sources", ""].iter().map(|s| s.to_string()).collect();
    match proc::run(&claude, &args, Some(Path::new(cwd)), COMPACT_TIMEOUT) {
        Ok(o) => compact_outcome(o.code, &o.stdout, &o.stderr),
        Err(proc::RunError::TimedOut) => Err(format!("it took over {} minutes", COMPACT_TIMEOUT as i64 / 60)),
        Err(proc::RunError::Spawn(e)) => Err(e.to_string()),
    }
}

/// Reads a headless compact's exit code and `--output-format json` result.
fn compact_outcome(code: Option<i32>, stdout: &str, stderr: &str) -> std::result::Result<(), String> {
    if code != Some(0) {
        return Err(one_line(if stderr.trim().is_empty() { stdout } else { stderr }, 300));
    }
    let Ok(v) = serde_json::from_str::<Value>(stdout.trim()) else { return Err(format!("its result wasn't JSON: {}", one_line(stdout, 300))) };
    if v["is_error"] != false {
        let why = v["result"].as_str().filter(|s| !s.trim().is_empty()).or(v["subtype"].as_str()).unwrap_or("it reported an error");
        return Err(one_line(why, 300));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts() {
        assert_eq!(tokens_text(150_000), "150k");
        assert_eq!(tokens_text(61_500), "61.5k");
        assert_eq!(mins_text(90), "1h 30m");
        assert_eq!(mins_text(45), "45m");
        assert_eq!(mins_text(120), "2h");
    }

    #[test]
    fn reads_the_compact_result() {
        assert!(compact_outcome(Some(0), r#"{"type":"result","subtype":"success","is_error":false,"result":""}"#, "").is_ok());
        assert_eq!(compact_outcome(Some(0), r#"{"type":"result","is_error":true,"result":"Not enough messages to compact."}"#, "").unwrap_err(), "Not enough messages to compact.");
        assert_eq!(compact_outcome(Some(0), r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#, "").unwrap_err(), "error_during_execution");
        assert!(compact_outcome(Some(0), "Compacted.", "").unwrap_err().starts_with("its result wasn't JSON"));
        assert_eq!(compact_outcome(Some(1), "", "No conversation found").unwrap_err(), "No conversation found");
    }

    #[test]
    fn globs_merge_per_project() {
        let mut l = Limits { compact_window: 0, cold_idle_mins: 0, warm_tokens: 0, warm_idle_mins: 0, generated: vec!["*.lock".into()], project_generated: BTreeMap::new() };
        l.project_generated.insert("web".into(), vec!["*.g.dart".into(), "*.lock".into()]);
        assert_eq!(l.globs_for(Some("web")), vec!["*.lock", "*.g.dart"]);
        assert_eq!(l.globs_for(Some("api")), vec!["*.lock"]);
    }
}
