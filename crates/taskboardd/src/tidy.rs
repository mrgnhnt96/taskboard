//! Tidying the backlog. On a timer (`[backlog] tidy_every_hours`, a day unless changed), a headless
//! `claude -p` reads each project's open issues that aren't planned yet and closes the ones that
//! repeat another issue, or that aren't real: already fixed, or wrong about the code. It runs in the
//! project's checkout with Claude's read-only tools, so it can look before it calls a finding false;
//! without a checkout it only closes repeats.
//!
//! A project with no issue added since its last tidy is skipped. Each issue it closes is closed as
//! won't do with the reason in its log (`tb backlog reopen` undoes it), and the issue a repeat
//! repeats gets a note. `tb backlog tidy` turns it on or off, changes how often, or runs it now.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, p, projects, triage};

/// The owner's changes to `[backlog] tidy` and `tidy_every_hours`: `{"on", "every_hours"}`.
const KEY: &str = "backlog_tidy";
/// What the tidy has done: `checked_at`, `seen` (each project's newest issue id it read) and `last`.
const STATE: &str = "backlog_tidy_state";
/// A tidy reads code, so it may take longer than grouping.
const TIMEOUT_SECS: u64 = 900;
/// The most issues of one project read in one tidy.
const MOST: usize = 150;
pub const MAX_EVERY_HOURS: f64 = 24.0 * 30.0;

fn saved(app: &App, key: &str) -> Value {
    app.db.get_setting(key).ok().flatten().and_then(|s| serde_json::from_str(&s).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}))
}

/// Whether the tidy is on (config.toml, unless `tb backlog tidy` said otherwise).
pub fn on(app: &App) -> bool {
    saved(app, KEY)["on"].as_bool().unwrap_or(app.cfg.backlog.tidy)
}

/// Hours between tidies.
pub fn every_hours(app: &App) -> f64 {
    saved(app, KEY)["every_hours"].as_f64().unwrap_or(app.cfg.backlog.tidy_every_hours).clamp(1.0, MAX_EVERY_HOURS)
}

/// `POST /backlog/tidy {on?, every_hours?, now?, reset?}`: change the tidy, or start one now.
pub fn set(app: &App, body: &Value) -> Result<Value> {
    let mut s = if as_bool(body.get("reset"), false) { json!({}) } else { saved(app, KEY) };
    if let Some(v) = body.get("on").filter(|v| !v.is_null()) {
        let Some(b) = v.as_bool() else { return err(400, "on is true or false.") };
        s["on"] = json!(b);
    }
    if let Some(v) = body.get("every_hours").filter(|v| !v.is_null()) {
        let h = v.as_f64().or_else(|| v.as_str().and_then(|x| x.trim().parse().ok()));
        let Some(h) = h.filter(|h| (1.0..=MAX_EVERY_HOURS).contains(h)) else {
            return err(400, "every_hours is a number of hours from 1 (hourly) to 720 (30 days).");
        };
        s["every_hours"] = json!(h);
    }
    app.db.set_setting(KEY, Some(&jdumps(&s)))?;
    if as_bool(body.get("now"), false) {
        if !triage::ai_on(app) {
            return err(409, "Claude is off for the backlog ([backlog] ai), or claude isn't installed.");
        }
        start(app, true);
    }
    Ok(state(app))
}

/// `GET /backlog/tidy`: whether it's on, how often, when it last ran and what it closed, and a `line` saying so.
pub fn state(app: &App) -> Value {
    let st = saved(app, STATE);
    let (on, every, running) = (on(app), every_hours(app), app.shared.lock().tidying);
    let last = st["last"].clone();
    let mut text = if on { format!("Backlog tidy: on, every {}.", hours_text(every)) } else { "Backlog tidy: off.".to_string() };
    if on && !triage::ai_on(app) {
        text.push_str(" It waits for Claude: [backlog] ai is off, or claude isn't installed.");
    }
    if running {
        text.push_str(" Tidying now.");
    }
    if let Some(at) = last["at"].as_str() {
        let closed = last["closed"].as_array().cloned().unwrap_or_default();
        let what = if closed.is_empty() {
            "nothing to close".to_string()
        } else {
            format!("closed {}", closed.iter().map(closed_text).collect::<Vec<_>>().join("; "))
        };
        text.push_str(&format!("\nLast tidy {}: {what}.", when(at)));
        if let Some(e) = last["error"].as_str() {
            text.push_str(&format!(" It failed for {e}"));
        }
    }
    json!({"on": on, "every_hours": every, "running": running, "checked_at": st["checked_at"], "last": last, "line": text})
}

fn hours_text(h: f64) -> String {
    if h == 1.0 {
        "hour".into()
    } else if h == 24.0 {
        "day".into()
    } else if h % 24.0 == 0.0 {
        format!("{} days", h / 24.0)
    } else {
        format!("{} hours", (h * 10.0).round() / 10.0)
    }
}

fn when(at: &str) -> String {
    match age_secs(Some(at)) {
        Some(a) if a < 86_400.0 => format!("at {}", local_clock(Some(at))),
        Some(a) => format!("{} ago", plural((a / 86_400.0) as i64, "day")),
        None => String::new(),
    }
}

fn closed_text(c: &Value) -> String {
    let r = c["ref"].as_str().unwrap_or("");
    match c["why"].as_str() {
        Some("duplicate") => format!("{r} (repeats {})", c["of"].as_str().unwrap_or("another")),
        Some("fixed") => format!("{r} (already fixed)"),
        _ => format!("{r} (not a real problem)"),
    }
}

// ------------------------------------------------------------------ the timer

/// Starts a tidy when one is due: on, Claude on, none running, and `every_hours` since the last check.
pub fn tick(app: &App) -> Result<()> {
    if !on(app) || !triage::ai_on(app) {
        return Ok(());
    }
    let checked = saved(app, STATE)["checked_at"].as_str().and_then(parse_iso);
    if checked.is_some_and(|c| now_ts() - c < every_hours(app) * 3600.0) {
        return Ok(());
    }
    start(app, false);
    Ok(())
}

/// Tidies on the Claude thread. `all` reads every project, new issues or not (`tb backlog tidy --now`).
fn start(app: &App, all: bool) {
    {
        let mut sh = app.shared.lock();
        if sh.tidying {
            return;
        }
        sh.tidying = true;
    }
    let mut st = saved(app, STATE);
    st["checked_at"] = json!(now_iso());
    let _ = app.db.set_setting(STATE, Some(&jdumps(&st)));
    app.queue_ai(Box::new(move |a: &App| {
        let r = run(a, all);
        a.shared.lock().tidying = false;
        if let Err(e) = r {
            a.info(format!("backlog: tidy failed: {}", e.message));
        }
    }));
}

/// The projects with an issue added since their last tidy, each with its newest issue's id.
pub fn due(app: &App, all: bool) -> Result<Vec<(String, i64)>> {
    let seen = saved(app, STATE)["seen"].clone();
    let mut out = vec![];
    for r in app.db.q("SELECT project, MAX(id) AS newest FROM issues WHERE project IS NOT NULL AND project != '' GROUP BY project ORDER BY project", p![])? {
        let (name, newest) = (r.st("project"), r.i0("newest"));
        if all || seen[&name].as_i64().is_none_or(|s| newest > s) {
            out.push((name, newest));
        }
    }
    Ok(out)
}

fn run(app: &App, all: bool) -> Result<()> {
    let mut closed: Vec<Value> = vec![];
    let mut errors: Vec<String> = vec![];
    for (project, newest) in due(app, all)? {
        match tidy_project(app, &project) {
            Ok(c) => {
                closed.extend(c);
                let mut st = saved(app, STATE);
                st["seen"][&project] = json!(newest);
                app.db.set_setting(STATE, Some(&jdumps(&st)))?;
            }
            Err(e) => {
                app.info(format!("backlog: tidying {project} failed: {e}"));
                errors.push(format!("{project}: {e}"));
            }
        }
    }
    let mut st = saved(app, STATE);
    st["last"] = json!({"at": now_iso(), "closed": closed, "error": if errors.is_empty() { Value::Null } else { json!(errors.join("; ")) }});
    app.db.set_setting(STATE, Some(&jdumps(&st)))?;
    if !closed.is_empty() {
        app.info(format!("backlog: tidy closed {}", closed.iter().map(closed_text).collect::<Vec<_>>().join(", ")));
    }
    Ok(())
}

// ------------------------------------------------------------------ asking Claude

fn schema() -> Value {
    json!({"type": "object", "required": ["close"], "properties": {"close": {"type": "array", "items": {
        "type": "object", "required": ["id", "why", "reason"],
        "properties": {"id": {"type": "integer"}, "why": {"enum": ["duplicate", "fixed", "false"]},
                       "of": {"type": "integer"}, "reason": {"type": "string"}}}}}})
}

/// Open issues nobody has planned yet: the ones a tidy may close.
fn loose(app: &App, project: &str) -> Result<Vec<Row>> {
    app.db.q(
        "SELECT * FROM issues WHERE project = ? AND state = 'open' AND goal_id IS NULL AND task_id IS NULL ORDER BY id DESC LIMIT ?",
        p![project, MOST as i64],
    )
}

/// Issues already planned (in a goal, or a task): a loose one may repeat them, but they stay.
fn planned(app: &App, project: &str) -> Result<Vec<Row>> {
    app.db.q(
        "SELECT * FROM issues WHERE project = ? AND ((state = 'open' AND goal_id IS NOT NULL) OR state IN ('task', 'ticket')) ORDER BY id DESC LIMIT ?",
        p![project, MOST as i64],
    )
}

fn tidy_project(app: &App, project: &str) -> std::result::Result<Vec<Value>, String> {
    let loose = loose(app, project).map_err(|e| e.message)?;
    if loose.is_empty() {
        return Ok(vec![]);
    }
    let planned = planned(app, project).map_err(|e| e.message)?;
    let repo = projects::project_path(app, Some(project)).ok().flatten().map(std::path::PathBuf::from).filter(|p| p.is_dir());
    let owner = &app.cfg.owner;
    let mut parts = vec![format!(
        "You tidy {owner}'s backlog for {project}: issues coding agents reported while working, not yet planned. \
         Close only what is clearly not worth keeping:\n\
         - duplicate: it reports the same problem as another issue (open or planned). Give that issue's number in \
         `of`. Of two open repeats, keep the one that says more, or the older one when they say the same.\n\
         - fixed: the code already does what the issue asks for.\n\
         - false: the issue is wrong about the code, or isn't a problem at all.\n\
         Two issues about the same area that ask for different changes are not duplicates. When in doubt, leave it \
         open: an empty list is a fine answer. Give each issue you close by its number (B12 → 12) with a short reason \
         {owner} can check."
    )];
    match &repo {
        Some(_) => parts.push(
            "You're in the project's checkout and may read its code (Read, Grep, Glob). Close an issue as fixed or false \
             only after reading the code it's about."
                .into(),
        ),
        None => parts.push("You can't see the project's code, so close duplicates only.".into()),
    }
    parts.push(format!("Open issues (you may close these):\n{}", loose.iter().map(|b| triage::issue_block(b, false)).collect::<Vec<_>>().join("\n")));
    if !planned.is_empty() {
        parts.push(format!(
            "Already planned (an open issue may repeat these; never close them):\n{}",
            planned.iter().map(|b| triage::issue_block(b, false)).collect::<Vec<_>>().join("\n")
        ));
    }
    let v = triage::ask_in(app, parts.join("\n\n"), schema(), repo.as_deref(), TIMEOUT_SECS)?;
    let loose_ids: HashSet<i64> = loose.iter().map(|b| b.id()).collect();
    let known: HashSet<i64> = loose_ids.iter().copied().chain(planned.iter().map(|b| b.id())).collect();
    let picks = checked(&v, &loose_ids, &known, repo.is_some());
    app.db.tx(|| close(app, &picks)).map_err(|e| e.message)
}

/// One issue Claude would close: why (`duplicate`, `fixed`, `false`), the issue it repeats, and its reason.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub id: i64,
    pub why: String,
    pub of: Option<i64>,
    pub reason: String,
}

/// Claude's answer made safe: only loose issues, each once; a duplicate names a known issue other than
/// itself that stays open; without the code, duplicates only.
pub fn checked(v: &Value, loose: &HashSet<i64>, known: &HashSet<i64>, read_code: bool) -> Vec<Pick> {
    let mut picks: Vec<Pick> = vec![];
    for x in v["close"].as_array().cloned().unwrap_or_default() {
        let Some(id) = x["id"].as_i64().filter(|i| loose.contains(i) && !picks.iter().any(|p| p.id == *i)) else { continue };
        let why = x["why"].as_str().unwrap_or("").to_string();
        let of = x["of"].as_i64();
        let ok = match why.as_str() {
            "duplicate" => of.is_some_and(|o| o != id && known.contains(&o)),
            "fixed" | "false" => read_code,
            _ => false,
        };
        if ok {
            let of = if why == "duplicate" { of } else { None };
            picks.push(Pick { id, why, of, reason: one_line(x["reason"].as_str().unwrap_or(""), 300) });
        }
    }
    // A duplicate of an issue closed here too would leave neither open: keep the one it repeats.
    let mut closing: HashSet<i64> = picks.iter().map(|p| p.id).collect();
    let mut kept: Vec<Pick> = vec![];
    for p in picks {
        if p.of.is_some_and(|o| closing.contains(&o)) {
            closing.remove(&p.id);
            continue;
        }
        kept.push(p);
    }
    kept
}

fn close(app: &App, picks: &[Pick]) -> Result<Vec<Value>> {
    let mut out = vec![];
    let mut noted: HashMap<i64, Vec<String>> = HashMap::new();
    for pk in picks {
        // Still loose: it may have been planned or closed while Claude read.
        let Some(b) = board::find_issue(app, Some(pk.id))? else { continue };
        if b.s("state") != Some("open") || b.i("goal_id").is_some() || b.i("task_id").is_some() {
            continue;
        }
        let what = match (pk.why.as_str(), pk.of) {
            ("duplicate", Some(o)) => format!("a duplicate of {}", rf("issue", o)),
            ("fixed", _) => "already fixed".to_string(),
            _ => "not a real problem".to_string(),
        };
        let reason = if pk.reason.is_empty() { String::new() } else { format!(": {}", pk.reason) };
        app.db.update("issues", &json!(pk.id), crate::fields!["state" => "drop", "updated_at" => now_iso()])?;
        board::add_issue_event(app, pk.id, board::BOARD, "drop", &format!("The backlog tidy closed it as {what}{reason}"), None)?;
        board::miss_settled(app, &b)?;
        if let Some(o) = pk.of {
            noted.entry(o).or_default().push(rf("issue", pk.id));
        }
        out.push(json!({"ref": rf("issue", pk.id), "why": pk.why, "of": pk.of.map(|o| rf("issue", o)), "reason": pk.reason, "title": b.st("title")}));
    }
    for (o, dups) in noted {
        let text = format!("The backlog tidy closed {} as {}", dups.join(", "), if dups.len() == 1 { "a duplicate of this" } else { "duplicates of this" });
        board::add_issue_event(app, o, board::BOARD, "note", &text, None)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[i64]) -> HashSet<i64> {
        v.iter().copied().collect()
    }

    #[test]
    fn claudes_closes_are_checked_before_use() {
        let loose = ids(&[1, 2, 3, 4, 5, 6]);
        let known = ids(&[1, 2, 3, 4, 5, 6, 10]);
        let v = json!({"close": [
            {"id": 1, "why": "duplicate", "of": 10, "reason": "Same as the planned one"},
            {"id": 10, "why": "duplicate", "of": 1, "reason": "planned: never closed"},
            {"id": 2, "why": "duplicate", "of": 2, "reason": "itself"},
            {"id": 3, "why": "duplicate", "of": 99, "reason": "unknown"},
            {"id": 4, "why": "fixed", "reason": "Done in run.rs"},
            {"id": 4, "why": "false", "reason": "twice"},
            {"id": 5, "why": "duplicate", "of": 6, "reason": "a"},
            {"id": 6, "why": "duplicate", "of": 5, "reason": "b"},
            {"id": 7, "why": "false", "reason": "not loose"}]});
        let got: Vec<(i64, &str, Option<i64>)> = checked(&v, &loose, &known, true).iter().map(|p| (p.id, if p.why == "fixed" { "fixed" } else { "duplicate" }, p.of)).collect();
        assert_eq!(got, vec![(1, "duplicate", Some(10)), (4, "fixed", None), (6, "duplicate", Some(5))], "of two repeats of each other, one stays open");
        let blind: Vec<i64> = checked(&v, &loose, &known, false).iter().map(|p| p.id).collect();
        assert_eq!(blind, vec![1, 6],"without the code, duplicates only");
    }

    #[test]
    fn how_often_reads_plainly() {
        assert_eq!(hours_text(1.0), "hour");
        assert_eq!(hours_text(24.0), "day");
        assert_eq!(hours_text(48.0), "2 days");
        assert_eq!(hours_text(6.0), "6 hours");
    }
}
