//! Check-ins on long turns: once a task's terminal has been working `after_mins`, and every
//! `every_mins` after that, the board reads the terminal's last lines and the processes under it and
//! asks a headless `claude -p` (Haiku) whether the agent is still getting somewhere or stuck. A stuck
//! verdict shows on the task's card until the turn ends or a later check-in says it's working again.
//! The agent isn't interrupted and nobody is notified.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, midna, p, proc};

/// `[checkin]` in config.toml.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CheckinConfig {
    pub on: bool,
    /// How long a turn runs before its first check-in.
    pub after_mins: u64,
    /// How long between check-ins on the same turn.
    pub every_mins: u64,
    pub model: String,
    pub budget_usd: String,
    pub timeout_secs: u64,
    /// How many of the terminal's last lines the check reads.
    pub lines: u32,
}

impl Default for CheckinConfig {
    fn default() -> Self {
        CheckinConfig {
            on: true,
            after_mins: 20,
            every_mins: 20,
            model: "claude-haiku-5-5".into(),
            budget_usd: "0.10".into(),
            timeout_secs: 90,
            lines: 60,
        }
    }
}

/// How often the loop looks for a turn that's due.
const TICK_SECS: f64 = 60.0;
/// How much of the terminal's tail the card keeps.
const TAIL_SHOWN: usize = 4000;
/// Processes under the terminal that the check names, at most.
const PROCESSES: usize = 15;

fn verdict_schema() -> Value {
    json!({"type": "object", "required": ["verdict", "why"], "properties": {
        "verdict": {"enum": ["working", "stuck", "unsure"]}, "why": {"type": "string"}}})
}

/// The last check-in on a session's current turn: one made since the terminal last started working.
fn current(s: &Row) -> Option<Value> {
    let c: Value = serde_json::from_str(s.s("stuck_check")?).ok()?;
    (s.s("status") == Some("working") && c["turn"].as_str().is_some() && c["turn"].as_str() == s.s("status_at")).then_some(c)
}

/// What a task's card shows while its terminal's turn looks stuck: `{at, why, tail, quiet_secs, working_secs}`.
pub fn stuck_view(s: &Row) -> Value {
    match current(s).filter(|c| c["verdict"] == "stuck") {
        Some(c) => json!({"at": c["at"], "why": c["why"], "tail": c["tail"], "quiet_secs": c["quiet_secs"], "working_secs": c["working_secs"]}),
        None => Value::Null,
    }
}

/// Whether a session's turn is due a check-in, given how long it has worked and its last check-in.
pub fn due(cfg: &CheckinConfig, working_secs: f64, last_age_secs: Option<f64>) -> bool {
    working_secs >= cfg.after_mins as f64 * 60.0 && last_age_secs.is_none_or(|a| a >= cfg.every_mins as f64 * 60.0)
}

pub fn run(app: std::sync::Arc<App>) {
    while !app.stopping() {
        if app.cfg.checkin.on {
            if let Err(e) = tick(&app) {
                app.info(format!("checkin: {e}"));
            }
        }
        app.sleep(TICK_SECS);
    }
}

fn tick(app: &App) -> Result<()> {
    let rows = app.db.q(
        "SELECT s.*, t.id AS task_id FROM sessions s JOIN tasks t ON t.session_id = s.id \
         WHERE s.status = 'working' AND t.status = 'working'",
        p![],
    )?;
    for s in rows {
        if app.stopping() {
            break;
        }
        if board::compacting_since(&s).is_string() {
            continue;
        }
        let working = age_secs(s.s("status_at")).unwrap_or(0.0);
        let last = current(&s).and_then(|c| age_secs(c["at"].as_str()));
        if due(&app.cfg.checkin, working, last) {
            check(app, &s, working);
        }
    }
    Ok(())
}

/// The end of `text`, at most `limit` characters.
fn last_chars(text: &str, limit: usize) -> String {
    let n = text.chars().count();
    text.chars().skip(n.saturating_sub(limit)).collect()
}

/// The commands running under the terminal, below Claude itself.
fn processes(app: &App, sid: &str) -> Vec<String> {
    let list = midna::call(app, "session.processes", json!({"id": sid})).ok();
    let mut out: Vec<String> = vec![];
    for p in list.as_ref().and_then(|l| l.as_array()).into_iter().flatten() {
        let cmd = one_line(p["command"].as_str().unwrap_or(""), 160);
        if p["depth"].as_i64().unwrap_or(0) > 0 && !cmd.is_empty() && !out.contains(&cmd) {
            out.push(cmd);
        }
    }
    out.truncate(PROCESSES);
    out
}

fn prompt(app: &App, s: &Row, working: f64, quiet: Option<f64>, tail: &str, procs: &[String]) -> Result<String> {
    let t = board::find_task(app, s.i("task_id"))?;
    let title = t.as_ref().map(|t| t.st("title")).unwrap_or_default();
    let quiet = quiet.map(|q| format!("{} minutes", (q / 60.0).round() as i64)).unwrap_or_else(|| "unknown".into());
    let procs = if procs.is_empty() { "(none)".to_string() } else { procs.join("\n") };
    Ok(format!(
        "A Claude Code agent has been working on the task \"{title}\" for {} minutes without finishing its turn. \
         Decide from its terminal whether it is still making progress or is stuck.\n\
         - working: output is still changing, a long build or test is running, or it is plainly waiting on something that ends by itself.\n\
         - stuck: it repeats the same failing step, waits on something that won't come (an interactive prompt, a hung command, a server that never answers), or loops without progress.\n\
         - unsure: you can't tell.\n\
         Give the reason in one plain sentence a person reads at a glance.\n\n\
         The terminal has been quiet for: {quiet}\n\nProcesses running under it:\n{procs}\n\nIts last lines:\n{tail}",
        (working / 60.0).round() as i64
    ))
}

fn ask(app: &App, prompt: String) -> Option<Value> {
    let claude = proc::which(&app.cfg.claude)?;
    let c = &app.cfg.checkin;
    let args: Vec<String> = vec![
        "-p".into(),
        prompt,
        "--model".into(),
        c.model.clone(),
        "--setting-sources".into(),
        "".into(),
        "--no-session-persistence".into(),
        "--tools".into(),
        "".into(),
        "--strict-mcp-config".into(),
        "--max-budget-usd".into(),
        c.budget_usd.clone(),
        "--json-schema".into(),
        verdict_schema().to_string(),
        "--output-format".into(),
        "json".into(),
    ];
    let out = proc::run(&claude, &args, Some(&app.cfg.data), c.timeout_secs as f64).ok()?;
    let d: Value = serde_json::from_str(out.stdout.trim()).ok()?;
    let v = d.get("structured_output").cloned().or_else(|| d.get("result").and_then(|r| r.as_str()).and_then(|r| serde_json::from_str(r).ok()))?;
    matches!(v["verdict"].as_str(), Some("working" | "stuck" | "unsure")).then_some(v)
}

fn check(app: &App, s: &Row, working: f64) {
    let sid = s.st("id");
    let read = midna::call(app, "session.read", json!({"id": sid, "lines": app.cfg.checkin.lines}));
    let Some(tail) = read.ok().and_then(|r| r["text"].as_str().map(|t| t.trim_end().to_string())) else { return };
    let quiet = age_secs(s.s("last_activity"));
    let procs = processes(app, &sid);
    let Some(v) = prompt(app, s, working, quiet, &tail, &procs).ok().and_then(|p| ask(app, p)) else {
        app.info(format!("checkin: no verdict for {sid}"));
        return;
    };
    let verdict = v["verdict"].as_str().unwrap_or("unsure");
    let why = one_line(v["why"].as_str().unwrap_or(""), 300);
    let was_stuck = current(s).is_some_and(|c| c["verdict"] == "stuck");
    let check = json!({
        "turn": s.s("status_at"), "at": now_iso(), "verdict": verdict, "why": why,
        "tail": last_chars(&tail, TAIL_SHOWN), "quiet_secs": quiet.map(|q| q.round() as i64),
        "working_secs": working.round() as i64,
    });
    // The turn may have ended while Haiku read it: the check-in belongs to the turn it read.
    let _ = app.db.x("UPDATE sessions SET stuck_check = ? WHERE id = ? AND status_at = ?", p![jdumps(&check), sid, s.s("status_at")]);
    let Some(task) = s.i("task_id") else { return };
    let log = match verdict {
        "stuck" if !was_stuck => Some(format!("Looks stuck after {} minutes: {why}", (working / 60.0).round() as i64)),
        "working" if was_stuck => Some(format!("Getting somewhere again: {why}")),
        _ => None,
    };
    if let Some(text) = log {
        let _ = board::log_event(app, task, "", "checkin", &text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str, status_at: &str, check: Value) -> Row {
        let mut r = Row::new();
        r.insert("status".into(), json!(status));
        r.insert("status_at".into(), json!(status_at));
        r.insert("stuck_check".into(), json!(check.to_string()));
        r
    }

    #[test]
    fn first_check_in_waits_for_after_mins_then_repeats_every_mins() {
        let c = CheckinConfig::default();
        assert!(!due(&c, 19.0 * 60.0, None));
        assert!(due(&c, 20.0 * 60.0, None));
        assert!(!due(&c, 45.0 * 60.0, Some(5.0 * 60.0)));
        assert!(due(&c, 45.0 * 60.0, Some(20.0 * 60.0)));
    }

    #[test]
    fn stuck_shows_only_for_the_turn_it_read() {
        let stuck = json!({"turn": "2026-10-14T20:00:00Z", "at": "2026-10-14T20:25:00Z", "verdict": "stuck", "why": "waits on a prompt", "tail": "? Continue (y/n)"});
        let v = stuck_view(&row("working", "2026-10-14T20:00:00Z", stuck.clone()));
        assert_eq!(v["why"], "waits on a prompt");
        assert_eq!(v["tail"], "? Continue (y/n)");
        assert!(stuck_view(&row("working", "2026-10-14T21:00:00Z", stuck.clone())).is_null(), "a later turn");
        assert!(stuck_view(&row("idle", "2026-10-14T20:00:00Z", stuck.clone())).is_null(), "the turn ended");
        let mut working = stuck;
        working["verdict"] = json!("working");
        assert!(stuck_view(&row("working", "2026-10-14T20:00:00Z", working)).is_null());
    }
}
