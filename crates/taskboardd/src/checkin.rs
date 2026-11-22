//! Check-ins on long turns: once a task's terminal has been working `after_mins`, and every
//! `every_mins` after that, the board reads the terminal's last lines and the processes under it and
//! asks a headless `claude -p` (Haiku) whether the turn is going at a fair pace for its work, is far
//! slower than its work warrants (and what it waits on: a lock, a queue, Midna's busy hold, a hung
//! command), or is stuck. A slow or stuck verdict shows on the task's card and raises an alert for the
//! owner until the turn ends or a later check-in says it's working again. It also leaves the agent a
//! message for its next prompt or stop (once per turn, `nudge`); the agent isn't interrupted mid-command.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, deliver, dispatch, midna, p, proc};

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
    /// Whether a slow or stuck verdict also leaves the agent a message for its next prompt or stop.
    pub nudge: bool,
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
            nudge: true,
        }
    }
}

/// How often the loop looks for a turn that's due.
const TICK_SECS: f64 = 60.0;
/// How much of the terminal's tail the card keeps.
const TAIL_SHOWN: usize = 4000;
/// Processes under the terminal that the check names, at most.
const PROCESSES: usize = 15;

/// The verdicts the owner is told about: a turn far slower than its work warrants, or one going nowhere.
const FLAGGED: &[&str] = &["slow", "stuck"];

fn verdict_schema() -> Value {
    json!({"type": "object", "required": ["verdict", "why", "waiting_on"], "properties": {
        "verdict": {"enum": ["working", "slow", "stuck", "unsure"]}, "why": {"type": "string"},
        "waiting_on": {"type": "string"}}})
}

/// The last check-in on a session's current turn: one made since the terminal last started working.
fn current(s: &Row) -> Option<Value> {
    let c: Value = serde_json::from_str(s.s("stuck_check")?).ok()?;
    (s.s("status") == Some("working") && c["turn"].as_str().is_some() && c["turn"].as_str() == s.s("status_at")).then_some(c)
}

/// The slow or stuck verdict a check-in left on the turn: its own, or an earlier one an unsure check-in
/// didn't overturn.
fn flagged(c: &Value) -> Option<&str> {
    let v = c["flagged"].as_str().or(c["verdict"].as_str())?;
    FLAGGED.contains(&v).then_some(v)
}

/// What a task's card shows while its terminal's turn looks slow or stuck:
/// `{at, why, tail, quiet_secs, working_secs, slow, waiting_on}`.
pub fn stuck_view(s: &Row) -> Value {
    let Some(c) = current(s) else { return Value::Null };
    match flagged(&c) {
        Some(v) => json!({"at": c["at"], "why": card_why(v, &c), "tail": c["tail"], "quiet_secs": c["quiet_secs"],
                          "working_secs": c["working_secs"], "slow": v == "slow", "waiting_on": c["waiting_on"]}),
        None => Value::Null,
    }
}

/// "Slow, waiting on scripts/test.sh's lock: …" on a slow turn's card; a stuck one's plain reason.
fn card_why(verdict: &str, c: &Value) -> String {
    let why = c["why"].as_str().unwrap_or("");
    if verdict != "slow" {
        return why.to_string();
    }
    let head = match c["waiting_on"].as_str().filter(|w| !w.is_empty()) {
        Some(w) => format!("Slow, waiting on {w}"),
        None => "Slow".to_string(),
    };
    if why.is_empty() { head } else { format!("{head}: {why}") }
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
    clear_ended(app, &rows);
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

/// The key of the alert a stuck turn on session `sid` raises.
fn alert_key(sid: &str) -> String {
    format!("stuck:{sid}")
}

/// Whether a session's slow or stuck alert stays up: its turn is still going and no check-in on it has
/// found it working since (an unsure one leaves it up).
fn still_stuck(s: &Row) -> bool {
    current(s).is_some_and(|c| c["verdict"] != "working")
}

/// Takes down the stuck alerts of turns that have ended, or that a later check-in found working:
/// `working` is every session working on a task right now.
fn clear_ended(app: &App, working: &[Row]) {
    for a in dispatch::alerts(app) {
        let Some(sid) = a["key"].as_str().and_then(|k| k.strip_prefix("stuck:")) else { continue };
        if !working.iter().any(|s| s.s("id") == Some(sid) && still_stuck(s)) {
            let _ = dispatch::clear_alert_key(app, &alert_key(sid));
        }
    }
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
         Decide from its terminal whether that time fits the work, is far longer than the work warrants, or is going nowhere. \
         A spinner and a running command are not progress by themselves: a command can sit queued behind a lock or a queue for an hour.\n\
         - working: output is still changing and the time fits the work, like a big build or test run that is visibly moving.\n\
         - slow: it gets somewhere, or will, but far slower than the work warrants, mostly waiting its turn: a machine-wide lock \
         (scripts/test.sh's lock, cargo's \"Blocking waiting for file lock\"), a build slot or other queue, Midna holding a command \
         because the Mac is busy, a long sleep or poll loop, or one build or test that runs on without output.\n\
         - stuck: it repeats the same failing step, waits on something that won't come (an interactive prompt, a hung command, a server that never answers), or loops without progress.\n\
         - unsure: you can't tell.\n\
         In waiting_on, name what the turn waits on in a few words (\"scripts/test.sh's lock\", \"a Midna build slot\", \"a hung cargo test\"), \
         or leave it empty when it waits on nothing. Give the reason in why, in one plain sentence a person reads at a glance.\n\n\
         The terminal has been quiet for: {quiet}\n\nProcesses running under it:\n{procs}\n\nIts last lines:\n{tail}",
        (working / 60.0).round() as i64
    ))
}

fn ask(app: &App, prompt: String) -> Option<Value> {
    let v = ask_haiku(app, prompt, &verdict_schema())?;
    matches!(v["verdict"].as_str(), Some("working" | "slow" | "stuck" | "unsure")).then_some(v)
}

/// A headless `claude -p` on the check-in's model, budget and timeout, with no tools, settings or MCP,
/// giving back its answer in `schema`'s shape. None when claude is missing, fails, times out or doesn't
/// answer in JSON.
pub fn ask_haiku(app: &App, prompt: String, schema: &Value) -> Option<Value> {
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
        schema.to_string(),
        "--output-format".into(),
        "json".into(),
    ];
    let out = proc::run(&claude, &args, Some(&app.cfg.data), c.timeout_secs as f64).ok()?;
    let d: Value = serde_json::from_str(out.stdout.trim()).ok()?;
    d.get("structured_output").cloned().or_else(|| d.get("result").and_then(|r| r.as_str()).and_then(|r| serde_json::from_str(r).ok()))
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
    let waiting_on = one_line(v["waiting_on"].as_str().unwrap_or(""), 80);
    let before = current(s);
    let was = before.as_ref().and_then(flagged).map(str::to_string);
    let now = match verdict {
        "unsure" => was.clone(),
        v if FLAGGED.contains(&v) => Some(v.to_string()),
        _ => None,
    };
    let nudged = before.as_ref().is_some_and(|c| c["nudged"] == true);
    let task = s.i("task_id");
    let nudge = app.cfg.checkin.nudge && FLAGGED.contains(&verdict) && !nudged && task.is_some();
    let mut check = json!({
        "turn": s.s("status_at"), "at": now_iso(), "verdict": verdict, "why": why, "waiting_on": waiting_on,
        "flagged": now, "nudged": nudged || nudge,
        "tail": last_chars(&tail, TAIL_SHOWN), "quiet_secs": quiet.map(|q| q.round() as i64),
        "working_secs": working.round() as i64,
    });
    // The turn may have ended while Haiku read it: the check-in belongs to the turn it read.
    let saved = app.db.x("UPDATE sessions SET stuck_check = ? WHERE id = ? AND status_at = ?", p![jdumps(&check), sid, s.s("status_at")]);
    let Some(task) = task else { return };
    if !matches!(saved, Ok(1)) {
        return;
    }
    let mins = (working / 60.0).round() as i64;
    let raised = now.as_deref().filter(|n| was.as_deref() != Some(*n));
    let log = match raised {
        Some(n) => Some(format!("Looks {} after {mins} minutes: {why}", said(n, &waiting_on))),
        None if verdict == "working" && was.is_some() => Some(format!("Getting somewhere again: {why}")),
        None => None,
    };
    if let Some(text) = log {
        let _ = board::log_event(app, task, "", "checkin", &text);
    }
    if let Some(n) = raised {
        // A slow turn that turns stuck raises its alert afresh.
        let _ = dispatch::clear_alert_key(app, &alert_key(&sid));
        let goal = board::find_task(app, Some(task)).ok().flatten().and_then(|t| t.i("goal_id"));
        let text = format!("{} looks {} after {mins} minutes: {why}", rf("task", task), said(n, &waiting_on));
        let fields = json!({"title": alert_title(n, &waiting_on, mins), "detail": why});
        let _ = dispatch::add_alert_keyed_with(app, &text, Some(task), goal, &alert_key(&sid), fields);
    } else if now.is_none() {
        let _ = dispatch::clear_alert_key(app, &alert_key(&sid));
    }
    if nudge {
        let text = nudge_text(task, &board::tb_cmd(app), verdict, &waiting_on, &why, mins);
        if deliver::add(app, "checkin", &text, task, None, None).is_err() {
            check["nudged"] = json!(false);
            let _ = app.db.x("UPDATE sessions SET stuck_check = ? WHERE id = ? AND status_at = ?", p![jdumps(&check), sid, s.s("status_at")]);
        }
    }
}

/// "slow, waiting on scripts/test.sh's lock" or "stuck".
fn said(verdict: &str, waiting_on: &str) -> String {
    match verdict {
        "slow" if !waiting_on.is_empty() => format!("slow, waiting on {waiting_on}"),
        v => v.to_string(),
    }
}

/// The alert's title, which Midna's collapsed card shows alone.
fn alert_title(verdict: &str, waiting_on: &str, mins: i64) -> String {
    match verdict {
        "slow" if !waiting_on.is_empty() => format!("Slow: waiting on {waiting_on}"),
        "slow" => format!("Slow for {mins} minutes"),
        _ => format!("Stuck for {mins} minutes"),
    }
}

/// The message a slow or stuck turn leaves its agent for its next prompt or stop.
fn nudge_text(task: i64, tb: &str, verdict: &str, waiting_on: &str, why: &str, mins: i64) -> String {
    let why = why.trim_end_matches('.');
    let advice = if verdict == "slow" {
        "If a command waits on a lock or queue other terminals hold, run less while it's busy (only the tests your change \
         touches, a smaller build) or do other work meanwhile, and stop a command that has hung."
    } else {
        "Stop a command that has hung or waits on input that won't come, and take another way through instead of \
         repeating the same step."
    };
    format!(
        "{} Board check-in: this turn ran {mins} minutes and looked {}: {why}. {advice} If that's already behind you, \
         carry on. If you can't get past it, say what holds you up with {tb} note.",
        crate::handoff::marker(task),
        said(verdict, waiting_on)
    )
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
    fn a_stuck_alert_stays_until_the_turn_ends_or_works_again() {
        let check = |verdict: &str| json!({"turn": "2026-10-14T20:00:00Z", "at": "2026-10-14T20:25:00Z", "verdict": verdict});
        assert!(still_stuck(&row("working", "2026-10-14T20:00:00Z", check("stuck"))));
        assert!(still_stuck(&row("working", "2026-10-14T20:00:00Z", check("unsure"))), "unsure leaves it up");
        assert!(!still_stuck(&row("working", "2026-10-14T20:00:00Z", check("working"))));
        assert!(!still_stuck(&row("working", "2026-10-14T21:00:00Z", check("stuck"))), "a later turn");
        assert!(!still_stuck(&row("idle", "2026-10-14T20:00:00Z", check("stuck"))), "the turn ended");
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

    #[test]
    fn a_slow_turn_shows_what_it_waits_on() {
        let turn = "2026-11-21T10:00:00Z";
        let slow = json!({"turn": turn, "at": "2026-11-21T10:40:00Z", "verdict": "slow", "why": "cargo test sits in line",
                          "waiting_on": "scripts/test.sh's lock", "flagged": "slow"});
        let v = stuck_view(&row("working", turn, slow.clone()));
        assert_eq!(v["slow"], true);
        assert_eq!(v["waiting_on"], "scripts/test.sh's lock");
        assert_eq!(v["why"], "Slow, waiting on scripts/test.sh's lock: cargo test sits in line");
        let mut unsure = slow.clone();
        unsure["verdict"] = json!("unsure");
        assert_eq!(stuck_view(&row("working", turn, unsure))["slow"], true, "an unsure check-in keeps the flag");
        let mut bare = slow;
        bare["waiting_on"] = json!("");
        bare["why"] = json!("");
        assert_eq!(stuck_view(&row("working", turn, bare))["why"], "Slow");
    }

    #[test]
    fn the_flag_comes_from_the_turns_last_slow_or_stuck_verdict() {
        assert_eq!(flagged(&json!({"verdict": "stuck"})), Some("stuck"));
        assert_eq!(flagged(&json!({"verdict": "unsure", "flagged": "slow"})), Some("slow"));
        assert_eq!(flagged(&json!({"verdict": "unsure", "flagged": null})), None);
        assert_eq!(flagged(&json!({"verdict": "working"})), None);
    }

    #[test]
    fn alerts_and_nudges_say_what_the_turn_waits_on() {
        assert_eq!(alert_title("slow", "a Midna build slot", 45), "Slow: waiting on a Midna build slot");
        assert_eq!(alert_title("slow", "", 45), "Slow for 45 minutes");
        assert_eq!(alert_title("stuck", "a prompt", 45), "Stuck for 45 minutes");
        assert_eq!(said("slow", "a hung cargo test"), "slow, waiting on a hung cargo test");
        assert_eq!(said("stuck", "a prompt"), "stuck");
        let n = nudge_text(7, "tb", "slow", "scripts/test.sh's lock", "cargo test waits on the lock.", 45);
        assert!(n.starts_with("[task-board:T7] "), "{n}");
        assert!(n.contains("ran 45 minutes and looked slow, waiting on scripts/test.sh's lock: cargo test waits on the lock. "), "{n}");
        assert!(n.contains("tb note"), "{n}");
        assert!(nudge_text(7, "tb", "stuck", "", "repeats a failing step", 30).contains("instead of repeating"));
    }
}
