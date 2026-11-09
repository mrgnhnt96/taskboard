//! The owner's standing word for terminals a Midna trigger opens (`tb start-grant add --trigger "Nightly
//! triage"`): such a terminal has no one typing in it, so `tb start` there has no prompt to read. With a
//! grant, a terminal that trigger opened may start the tasks its own conversation made.
//!
//! Which trigger opened a terminal comes from Midna's `session.opened` event (`actor: {kind: "trigger",
//! name, trigger}`), which an agent can't write, never from a prompt or a terminal's name. That covers a
//! terminal the trigger's headless `--run` script opens with `midna open`: Midna marks it `via: "run"` from
//! the run's process, not from anything the script sets. A grant names one trigger by its Midna id too:
//! one with the same name that an agent added later isn't it. Only the
//! owner sets a grant: in the app, or with Grant on the notification the board sends when a terminal asks for
//! one (`tb start-grant add` there, theirs or an agent's). Anyone may take one away.
//!
//! Their own `!` run of the command can't be told from an agent's from inside it: Claude Code writes the
//! `!` input to the transcript only after the command ends, and its shell looks like one a Monitor starts.

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

/// The trigger's Midna id, when Midna's event named it (`actor.trigger`); null from a Midna that only
/// gave its name.
pub const ADDED: &[(&str, &str, &str)] = &[("trigger_opened", "trigger_id", "TEXT")];

/// Notes that Midna's trigger `trigger` (id `trigger_id`, when Midna gave it) opened terminal `sid`.
pub fn note_opened(app: &App, sid: &str, trigger: &str, trigger_id: Option<&str>) -> Result<()> {
    app.db.x(
        "INSERT OR REPLACE INTO trigger_opened(session_id, trigger, trigger_id, at) VALUES (?, ?, ?, ?)",
        p![sid, trigger, trigger_id, now_iso()],
    )?;
    Ok(())
}

/// The trigger that opened terminal `sid`, if one did: its name, and its id when Midna gave it.
pub fn opened_by(app: &App, sid: &str) -> Result<Option<(String, Option<String>)>> {
    let row = app.db.q1("SELECT trigger, trigger_id FROM trigger_opened WHERE session_id = ?", p![sid])?;
    Ok(row.map(|r| (r.st("trigger"), r.s("trigger_id").filter(|t| !t.is_empty()).map(str::to_string))))
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
/// When Midna named the trigger's id, the grant is matched by that id alone, so a renamed trigger keeps it.
pub fn honored(app: &App, sid: &str, t: &Row) -> Result<Option<String>> {
    let Some((trigger, trigger_id)) = opened_by(app, sid)? else { return Ok(None) };
    let project = t.st("project");
    let grants: Vec<Row> = active(app)?
        .into_iter()
        .filter(|g| match &trigger_id {
            Some(id) => g.st("trigger_id") == *id,
            None => g.st("trigger") == trigger,
        })
        .filter(|g| g.s("project").is_none_or(|p| p.is_empty() || p == project))
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
    if trigger_id.is_some() {
        let g = &grants[0];
        return Ok(Some(format!("Started on {}'s standing grant S{} for trigger '{}'", app.cfg.owner, g.id(), g.st("trigger"))));
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

/// What a grant ask goes by among the alert waiters and the notification inbox: `grant:3`.
pub const KEY_PREFIX: &str = "grant:";

/// The grants terminals asked for that the owner hasn't answered: `{n, trigger, project, session_id,
/// notified_at}`.
pub fn asks(app: &App) -> Vec<Value> {
    jloads_arr(app.db.get_setting("grant_asks").ok().flatten().as_deref())
}

fn keep_asks(app: &App, rows: &[Value]) -> Result<()> {
    for gone in asks(app).iter().filter(|a| !rows.iter().any(|r| r["n"] == a["n"])) {
        crate::dispatch::withdraw_id(app, &notification_id(gone));
    }
    app.db.set_setting("grant_asks", Some(&jdumps(&Value::Array(rows.to_vec()))))
}

/// The waiter and inbox key of ask `a`.
pub fn key(a: &Value) -> String {
    format!("{KEY_PREFIX}{}", a["n"])
}

/// The id of ask `a`'s notification in Midna: `taskboard-grant-3`.
pub fn notification_id(a: &Value) -> String {
    format!("taskboard-grant-{}", a["n"])
}

/// A terminal's `tb start-grant add` (`via`: the terminal): asks the owner with a notification whose Grant
/// adds it. The trigger has to be one Midna has. A new ask for the same trigger and project replaces the old.
pub fn ask(app: &App, via: &str, body: &Value) -> Result<Value> {
    let name = body_str(body, "trigger");
    if name.trim().is_empty() {
        return err(400, "Name the Midna trigger whose terminals may start the tasks they make (--trigger).");
    }
    the_trigger(app, &name).map_err(|m| ApiError::new(400, format!("No grant: {m}.")))?;
    let project = Some(body_str(body, "project")).filter(|p| !p.is_empty());
    let mut rows = asks(app);
    let n = rows.iter().filter_map(|a| a["n"].as_i64()).max().unwrap_or(0) + 1;
    rows.retain(|a| !(a["trigger"] == name.as_str() && a["project"].as_str() == project.as_deref()));
    let a = json!({"n": n, "trigger": name, "project": project, "session_id": via, "notified_at": now_iso()});
    rows.push(a.clone());
    keep_asks(app, &rows)?;
    let owner = &app.cfg.owner;
    if !(app.cfg.notify && app.cfg.runner) {
        return err(403, format!("Only {owner} can grant a standing start, and the board's notifications are off, so it couldn't ask them."));
    }
    notify(app, &a);
    Ok(json!({"asked": true, "line": format!("Asked {owner} in a notification: trigger '{name}' gets its grant when they tap Grant. It isn't granted yet; don't run this again or ask them.")}))
}

fn notify(app: &App, a: &Value) {
    let scope = a["project"].as_str().map(|p| format!(" in {p}")).unwrap_or_default();
    let who = crate::board::session_name(app, a["session_id"].as_str(), None);
    let params = json!({
        "title": format!("Grant trigger '{}' a standing start?", a["trigger"].as_str().unwrap_or("")),
        "body": format!("Terminals it opens may start the tasks they make{scope}.\nAsked in {who}"),
        "category": crate::midna::NOTIFY_ASK_KIND, "sound": true, "id": notification_id(a), "actions": ["Grant", "Not now"],
    });
    let (midna, inbox, waiters) = {
        let sh = app.shared.lock();
        (app.cfg.midna.clone(), sh.notify_inbox.clone(), sh.alert_waiters.clone())
    };
    let (k, nid, sent_at) = (key(a), notification_id(a), a["notified_at"].as_str().unwrap_or("").to_string());
    std::thread::spawn(move || {
        if crate::midna::notify_send(&midna, &params).is_err() {
            return;
        }
        if let Some(w) = crate::dispatch::Waiter::start(waiters, &k, &sent_at, &nid) {
            w.listen(&midna, &inbox);
        }
    });
}

/// Acts on the owner's answers to grant asks, outside a transaction (adding one asks Midna for the trigger):
/// Grant adds the grant, Not now or a dismissal drops the ask, a click leaves it. Only an ask the board keeps,
/// and its latest notification, counts.
pub fn answers(app: &App) -> Result<()> {
    let inbox = app.shared.lock().notify_inbox.clone();
    let got: Vec<(String, Value)> = {
        let mut i = inbox.lock();
        let (got, rest) = std::mem::take(&mut *i).into_iter().partition(|(id, _)| id.starts_with(KEY_PREFIX));
        *i = rest;
        got
    };
    for (k, r) in got {
        answer(app, &k, &r)?;
    }
    Ok(())
}

pub fn answer(app: &App, k: &str, response: &Value) -> Result<()> {
    let r = response.get("response").filter(|r| r.is_object()).unwrap_or(response);
    let rows = asks(app);
    let Some(a) = rows.iter().find(|a| key(a) == k).cloned() else { return Ok(()) };
    if response.get("sent_at").and_then(|x| x.as_str()).is_some_and(|at| a["notified_at"].as_str() != Some(at)) {
        return Ok(());
    }
    let grant = match (r["kind"].as_str(), r["action"].as_str()) {
        (Some("action"), Some("Grant")) => true,
        (Some("action"), Some("Not now")) | (Some("dismissed"), _) => false,
        _ => return Ok(()),
    };
    keep_asks(app, &rows.into_iter().filter(|x| key(x) != k).collect::<Vec<_>>())?;
    if !grant {
        return Ok(());
    }
    let by = format!("{} from a notification", app.cfg.owner);
    if let Err(e) = add(app, &by, &json!({"trigger": a["trigger"], "project": a["project"]})) {
        crate::dispatch::notify(app, &format!("Trigger '{}' wasn't granted: {}", a["trigger"].as_str().unwrap_or(""), e.message), "");
    }
    Ok(())
}
