//! The owner's standing word for terminals a Midna trigger opens (`tb start-grant add --trigger "Nightly
//! triage"`): such a terminal has no one typing in it, so `tb start` there has no prompt to read. With a
//! grant, a terminal that trigger opened may start the tasks its own conversation made.
//!
//! Which trigger opened a terminal comes from Midna's `session.opened` event (`actor: {kind: "trigger",
//! name}`), which an agent can't write, never from a prompt or a terminal's name. A grant names one
//! trigger by its Midna id too: one with the same name that an agent added later isn't it. Only the
//! owner sets a grant (the app, or their own `!` run of the command); anyone may take one away.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::midna::{call_timeout, MidnaError};
use crate::p;
use crate::util::*;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS start_grants(
  id INTEGER PRIMARY KEY, trigger TEXT NOT NULL, trigger_id TEXT NOT NULL, project TEXT, by TEXT, at TEXT, removed_at TEXT);
CREATE TABLE IF NOT EXISTS trigger_opened(session_id TEXT PRIMARY KEY, trigger TEXT NOT NULL, at TEXT);
"#;

/// The owner's own `! tb start-grant add …` run.
pub static ADD_CMD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bstart-grant\s+add\b").unwrap());

/// Notes that Midna's trigger `trigger` opened terminal `sid`.
pub fn note_opened(app: &App, sid: &str, trigger: &str) -> Result<()> {
    app.db.x("INSERT OR REPLACE INTO trigger_opened(session_id, trigger, at) VALUES (?, ?, ?)", p![sid, trigger, now_iso()])?;
    Ok(())
}

/// The trigger that opened terminal `sid`, if one did.
pub fn opened_by(app: &App, sid: &str) -> Result<Option<String>> {
    Ok(app.db.q1("SELECT trigger FROM trigger_opened WHERE session_id = ?", p![sid])?.map(|r| r.st("trigger")))
}

fn active(app: &App) -> Result<Vec<Row>> {
    app.db.q("SELECT * FROM start_grants WHERE removed_at IS NULL ORDER BY id", p![])
}

fn line(g: &Row) -> String {
    let scope = g.s("project").filter(|p| !p.is_empty()).map(|p| format!(" in {p}")).unwrap_or_default();
    format!("S{} trigger '{}'{scope}", g.id(), g.st("trigger"))
}

pub fn list(app: &App) -> Result<Value> {
    let rows = active(app)?;
    let lines: Vec<String> = rows.iter().map(line).collect();
    let grants: Vec<Value> = rows
        .iter()
        .map(|g| json!({"id": g.id(), "trigger": g.st("trigger"), "trigger_id": g.st("trigger_id"), "project": g.s("project"), "by": g.st("by"), "at": g.st("at")}))
        .collect();
    let text = if lines.is_empty() { "No standing start grants.".to_string() } else { lines.join("\n") };
    Ok(json!({"grants": grants, "line": text}))
}

/// Midna's triggers named `name`: the one a grant pins, or why there isn't exactly one.
fn the_trigger(app: &App, name: &str) -> std::result::Result<Value, String> {
    let all = match call_timeout(app, "trigger.list", json!({}), 5.0) {
        Ok(v) => v.as_array().cloned().unwrap_or_default(),
        Err(MidnaError::Down(m) | MidnaError::Refused(m)) => return Err(format!("couldn't read Midna's triggers: {m}")),
    };
    let named: Vec<Value> = all.into_iter().filter(|t| t["name"].as_str() == Some(name)).collect();
    match named.len() {
        0 => Err(format!("Midna has no trigger named '{name}'")),
        1 => Ok(named[0].clone()),
        n => Err(format!("{n} of Midna's triggers are named '{name}'")),
    }
}

/// Adds a grant; the caller has checked it's the owner's.
pub fn add(app: &App, by: &str, body: &Value) -> Result<Value> {
    let name = body_str(body, "trigger");
    if name.trim().is_empty() {
        return err(400, "Name the Midna trigger whose terminals may start the tasks they make (--trigger).");
    }
    let t = the_trigger(app, &name).map_err(|m| ApiError::new(400, format!("No grant: {m}.")))?;
    let tid = t["id"].as_str().unwrap_or("").to_string();
    let project = Some(body_str(body, "project")).filter(|p| !p.is_empty());
    if active(app)?.iter().any(|g| g.st("trigger_id") == tid && g.s("project").map(str::to_string) == project) {
        return err(409, format!("Trigger '{name}' already has that grant."));
    }
    let id = app.db.insert(
        "start_grants",
        vec![("trigger", json!(name)), ("trigger_id", json!(tid)), ("project", json!(project)), ("by", json!(by)), ("at", json!(now_iso()))],
    )?;
    let g = app.db.q1("SELECT * FROM start_grants WHERE id = ?", p![id])?.ok_or_else(|| ApiError::new(500, "The grant wasn't saved."))?;
    app.info(format!("start grant added: {} ({by})", line(&g)));
    Ok(json!({"id": id, "line": format!("Granted S{id}: terminals trigger '{name}' opens may start the tasks they make{}.", g.s("project").map(|p| format!(" in {p}")).unwrap_or_default())}))
}

/// Takes grant `id` away.
pub fn remove(app: &App, id: i64) -> Result<Value> {
    let Some(g) = app.db.q1("SELECT * FROM start_grants WHERE id = ? AND removed_at IS NULL", p![id])? else {
        return err(404, format!("There's no standing start grant S{id}."));
    };
    app.db.x("UPDATE start_grants SET removed_at = ? WHERE id = ?", p![now_iso(), id])?;
    app.info(format!("start grant removed: {}", line(&g)));
    Ok(json!({"line": format!("Removed {}.", line(&g))}))
}

/// The owner's grant that lets terminal `sid` start task `t` with no word in its conversation, as the log's
/// "Started on …" line: a trigger opened `sid`, a grant names that trigger (and the task's project, when
/// it names one), the trigger is still the one granted, and `sid`'s own conversation made the task.
pub fn honored(app: &App, sid: &str, t: &Row) -> Result<Option<String>> {
    let Some(trigger) = opened_by(app, sid)? else { return Ok(None) };
    let project = t.st("project");
    let grants: Vec<Row> = active(app)?
        .into_iter()
        .filter(|g| g.st("trigger") == trigger && g.s("project").is_none_or(|p| p.is_empty() || p == project))
        .collect();
    if grants.is_empty() {
        return Ok(None);
    }
    let made = app
        .db
        .q1("SELECT 1 FROM session_events WHERE session_id = ? AND kind = ? AND data = ? LIMIT 1", p![sid, crate::startword::MADE, t.id().to_string()])?;
    if made.is_none() {
        return Ok(None);
    }
    let now = match the_trigger(app, &trigger) {
        Ok(v) => v["id"].as_str().unwrap_or("").to_string(),
        Err(m) => {
            app.info(format!("start grant for trigger '{trigger}' not used: {m}"));
            return Ok(None);
        }
    };
    let Some(g) = grants.iter().find(|g| g.st("trigger_id") == now) else {
        app.info(format!("start grant for trigger '{trigger}' not used: it's a different trigger from the one granted"));
        return Ok(None);
    };
    Ok(Some(format!("Started on {}'s standing grant S{} for trigger '{trigger}'", app.cfg.owner, g.id())))
}
