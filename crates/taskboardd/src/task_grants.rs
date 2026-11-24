//! Midna permissions a task carries (`tb task set T5 --allow 'tool:mcp__slack__chat_post*'`): each grant is
//! a Midna rule kind and glob. Once the owner confirms one, every terminal the board opens for the task gets
//! a Midna `allow` rule for it, scoped to that terminal and expiring after `RULE_SECS`, so the task's work
//! runs that action with no approval prompt while other terminals are still asked.
//!
//! Only the owner confirms a grant: from the app, or with Grant on the notification the board sends when a
//! terminal adds one (theirs or an agent's, which can't be told apart). What a prompt says never counts.
//! Agents can't remove Midna rules, so a rule ends with its terminal or its expiry; dropping a grant adds an
//! `ask` rule over each live one, which outranks the `allow` in the same scope.

use serde_json::{json, Value};

use crate::app::App;
use crate::board;
use crate::midna::{call_timeout, MidnaError};
use crate::p;
use crate::util::*;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS task_grants(
  id INTEGER PRIMARY KEY, task_id INTEGER NOT NULL, kind TEXT NOT NULL, pattern TEXT NOT NULL, state TEXT NOT NULL,
  asked_by TEXT, at TEXT, granted_by TEXT, granted_at TEXT, notified_at TEXT, removed_at TEXT);
CREATE INDEX IF NOT EXISTS task_grants_task ON task_grants(task_id);
CREATE TABLE IF NOT EXISTS task_grant_rules(
  rule_id TEXT PRIMARY KEY, grant_id INTEGER NOT NULL, session_id TEXT NOT NULL, at TEXT, expires_at TEXT,
  fired INTEGER NOT NULL DEFAULT 0, last_fired_at TEXT, gone INTEGER NOT NULL DEFAULT 0);
CREATE INDEX IF NOT EXISTS task_grant_rules_grant ON task_grant_rules(grant_id);
"#;

pub const KINDS: &[&str] = &["command", "tool", "path", "cli", "window"];

/// How long a terminal's rule lasts; the board adds it again on the terminal's next open, and renews it
/// while the terminal is still up.
pub const RULE_SECS: u64 = 24 * 3600;

/// How often `tend` reads Midna's rules for fired counts and renewals.
const TEND_SECS: f64 = 120.0;

/// Midna's methods only the human may call (`midna schema --list`, human_only): an `allow` rule can't be
/// let reach them.
pub const HUMAN_ONLY: &[&str] = &[
    "daemon.upgrade", "daemon.stop", "daemon.reset", "project.remove", "session.close_hold", "session.send_image",
    "links.patterns.approve", "links.actions.approve", "notify.respond", "session.pause", "session.stop_processes",
    "rule.remove", "rule.restore", "trigger.set_secret", "secret.replace", "secret.remove", "webhooks.configure",
    "keep_awake.wake_setup", "peers.confirm", "peers.forget", "peers.set_tier", "peers.set_drive", "drive.open",
    "drive.close", "peers.set_auto_accept", "peers.set_agent_send", "share.accept", "share.decline", "updates.install",
    "updates.report", "themes.report", "hooks.install", "hooks.uninstall", "script.click",
];

/// Midna CLI verbs that answer for the human without a method of their own above.
const HUMAN_ONLY_VERBS: &[&str] = &["approve", "needs respond", "needs dismiss", "settings set", "settings reset"];

/// Matches Midna's rule globs: `*` any run, `?` one character, `\` escapes the next.
pub fn glob_match(pattern: &str, value: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let v: Vec<char> = value.chars().collect();
    fn go(p: &[char], v: &[char]) -> bool {
        match p.first() {
            None => v.is_empty(),
            Some('*') => (0..=v.len()).any(|i| go(&p[1..], &v[i..])),
            Some('?') => !v.is_empty() && go(&p[1..], &v[1..]),
            Some('\\') if p.len() > 1 => v.first() == Some(&p[1]) && go(&p[2..], &v[1..]),
            Some(c) => v.first() == Some(c) && go(&p[1..], &v[1..]),
        }
    }
    go(&p, &v)
}

/// The ways an action can name Midna method `m` (`notify.respond`): as a CLI verb, a raw call, an MCP tool.
fn spellings(m: &str) -> Vec<String> {
    let words: Vec<&str> = m.split('.').collect();
    let mut out = vec![m.to_string(), m.replace('.', "_")];
    for sep in [" ", "-"] {
        let mut ws: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        out.push(ws.join(" "));
        out.push(ws.iter().map(|w| w.replace('_', sep)).collect::<Vec<_>>().join(" "));
        ws[0] = format!("{}s", ws[0]);
        out.push(ws.join(" "));
        out.push(ws.iter().map(|w| w.replace('_', sep)).collect::<Vec<_>>().join(" "));
    }
    out.sort();
    out.dedup();
    out
}

/// The human-only Midna method or verb a `kind` grant of `pattern` would reach, if any.
pub fn reaches_human_only(kind: &str, pattern: &str) -> Option<String> {
    if !matches!(kind, "command" | "tool" | "cli") {
        return None;
    }
    let low = pattern.to_lowercase();
    let names = HUMAN_ONLY.iter().map(|m| (m.to_string(), spellings(m))).chain(HUMAN_ONLY_VERBS.iter().map(|v| (v.to_string(), vec![v.to_string()])));
    for (name, forms) in names {
        for f in &forms {
            let probes = [
                f.clone(),
                format!("{f} x"),
                format!("midna {f} x"),
                format!("midna call {name} {{}}"),
                format!("mcp__midna__{}", name.replace(['.', ' '], "_")),
                format!("Bash(midna {f} x)"),
            ];
            let mentions = (kind == "cli" || low.contains("midna")) && low.contains(f.to_lowercase().as_str());
            if mentions || probes.iter().any(|p| glob_match(pattern, p)) {
                return Some(name);
            }
        }
    }
    None
}

/// Reads `<kind>:<pattern>`: a Midna rule kind and a glob that can't reach a human-only method.
pub fn parse(s: &str) -> std::result::Result<(String, String), String> {
    let Some((kind, pattern)) = s.split_once(':') else {
        return Err(format!("“{s}” isn't a grant: write it as <kind>:<pattern>, the kind one of {}.", KINDS.join(", ")));
    };
    let kind = kind.trim().to_lowercase();
    if !KINDS.contains(&kind.as_str()) {
        return Err(format!("“{kind}” isn't a Midna rule kind: use one of {}.", KINDS.join(", ")));
    }
    let pattern = pattern.trim().to_string();
    if pattern.is_empty() {
        return Err(format!("The {kind} grant needs a pattern after the colon."));
    }
    if let Some(m) = reaches_human_only(&kind, &pattern) {
        return Err(format!("“{kind}:{pattern}” can't be granted: it reaches Midna's {m}, which only the human may do."));
    }
    Ok((kind, pattern))
}

fn gref(id: i64) -> String {
    format!("A{id}")
}

pub fn parse_gref(s: &str) -> Result<i64> {
    s.trim().trim_start_matches(['A', 'a']).parse::<i64>().map_err(|_| ApiError::new(404, format!("“{s}” isn't a grant (A3).")))
}

fn get(app: &App, id: i64) -> Result<Row> {
    app.db.q1("SELECT * FROM task_grants WHERE id = ? AND removed_at IS NULL", p![id])?.ok_or_else(|| ApiError::new(404, format!("There's no grant {}.", gref(id))))
}

fn line(g: &Row) -> String {
    let state = if g.st("state") == "granted" { "" } else { " (waiting for the owner's Grant)" };
    format!("{} {}:{}{state}", gref(g.id()), g.st("kind"), g.st("pattern"))
}

/// The task's grants, for its detail and `tb task grant list`: each with its terminals' rules and fired counts.
pub fn view(app: &App, task_id: i64) -> Result<Vec<Value>> {
    let rows = app.db.q("SELECT * FROM task_grants WHERE task_id = ? AND removed_at IS NULL ORDER BY id", p![task_id])?;
    let mut out = vec![];
    for g in rows {
        let rules = app.db.q("SELECT * FROM task_grant_rules WHERE grant_id = ? ORDER BY at", p![g.id()])?;
        let fired: i64 = rules.iter().map(|r| r.i("fired").unwrap_or(0)).sum();
        let last = rules.iter().filter_map(|r| r.s("last_fired_at").map(str::to_string)).max();
        out.push(json!({
            "id": g.id(), "ref": gref(g.id()), "kind": g.st("kind"), "pattern": g.st("pattern"), "state": g.st("state"),
            "asked_by": g.v("asked_by"), "at": g.v("at"), "granted_by": g.v("granted_by"), "granted_at": g.v("granted_at"),
            "fired": fired, "last_fired_at": last,
            "rules": rules.iter().map(|r| json!({"rule_id": r.st("rule_id"), "session_id": r.st("session_id"), "at": r.v("at"),
                "expires_at": r.v("expires_at"), "fired": r.i("fired").unwrap_or(0), "last_fired_at": r.v("last_fired_at"), "live": !r.b("gone")})).collect::<Vec<_>>(),
        }));
    }
    Ok(out)
}

pub fn list(app: &App, task_id: i64) -> Result<Value> {
    let grants = view(app, task_id)?;
    let lines: Vec<String> = grants
        .iter()
        .map(|g| {
            let state = if g["state"] == "granted" { format!(" · fired {}", g["fired"]) } else { " · waiting for the owner's Grant".to_string() };
            format!("{} {}:{}{state}", g["ref"].as_str().unwrap_or(""), g["kind"].as_str().unwrap_or(""), g["pattern"].as_str().unwrap_or(""))
        })
        .collect();
    let text = if lines.is_empty() { format!("{} has no grants.", rf("task", task_id)) } else { lines.join("\n") };
    Ok(json!({"grants": grants, "line": text}))
}

/// Adds grants (`allow`: `<kind>:<pattern>` each) to task `task_id`. The owner's (`owner`, from the app) are
/// granted at once; a terminal's (`via`) wait for the owner's Grant on a notification.
pub fn add(app: &App, task_id: i64, allow: &[String], owner: bool, via: &str) -> Result<Value> {
    if allow.is_empty() {
        return err(400, "Say what to grant: --allow '<kind>:<pattern>', e.g. --allow 'tool:mcp__slack__chat_post*'.");
    }
    let parsed = allow.iter().map(|s| parse(s)).collect::<std::result::Result<Vec<_>, _>>().map_err(|m| ApiError::new(400, m))?;
    let t = board::get_task(app, task_id)?;
    let by = if owner { "the app".to_string() } else { board::session_name(app, Some(via).filter(|v| !v.is_empty()), None) };
    let (mut lines, mut granted, mut asked) = (vec![], vec![], vec![]);
    app.db.tx(|| {
        for (kind, pattern) in &parsed {
            let had = app.db.q1(
                "SELECT * FROM task_grants WHERE task_id = ? AND kind = ? AND pattern = ? AND removed_at IS NULL",
                p![task_id, kind.as_str(), pattern.as_str()],
            )?;
            match had {
                Some(g) if g.st("state") == "granted" || !owner => lines.push(format!("{} is already on {}.", line(&g), rf("task", task_id))),
                Some(g) => {
                    confirm_row(app, &g, &format!("{} in the app", app.cfg.owner))?;
                    granted.push(g.id());
                }
                None => {
                    let state = if owner { "granted" } else { "pending" };
                    let now = now_iso();
                    let mut row = vec![("task_id", json!(task_id)), ("kind", json!(kind)), ("pattern", json!(pattern)), ("state", json!(state)), ("asked_by", json!(by)), ("at", json!(now))];
                    if owner {
                        row.extend([("granted_by", json!(format!("{} in the app", app.cfg.owner))), ("granted_at", json!(now))]);
                    }
                    let id = app.db.insert("task_grants", row)?;
                    let g = get(app, id)?;
                    board::log_event(app, task_id, board::BOARD, "grant", &format!("Grant {} added by {by}", line(&g)))?;
                    if owner {
                        granted.push(id);
                    } else {
                        asked.push(id);
                    }
                }
            }
        }
        Ok(())
    })?;
    for id in &granted {
        let g = get(app, *id)?;
        lines.push(format!("Granted {}: terminals the board opens for {} run it without asking.", line(&g), rf("task", task_id)));
    }
    if !asked.is_empty() {
        let owner_name = &app.cfg.owner;
        let refs: Vec<String> = asked.iter().map(|id| gref(*id)).collect();
        if app.cfg.notify && app.cfg.runner {
            for id in &asked {
                ask(app, *id, &t)?;
            }
            lines.push(format!(
                "Asked {owner_name} in a notification to grant {}; it isn't granted until they tap Grant. Don't run this again or ask them.",
                refs.join(", ")
            ));
        } else {
            lines.push(format!("{} waits for {owner_name} to confirm it in the app; the board's notifications are off, so it couldn't ask them.", refs.join(", ")));
        }
    }
    if !granted.is_empty() {
        apply_live(app, task_id);
    }
    Ok(json!({"granted": granted.iter().map(|i| gref(*i)).collect::<Vec<_>>(), "asked": asked.iter().map(|i| gref(*i)).collect::<Vec<_>>(), "line": lines.join("\n")}))
}

fn confirm_row(app: &App, g: &Row, by: &str) -> Result<()> {
    app.db.x("UPDATE task_grants SET state = 'granted', granted_by = ?, granted_at = ? WHERE id = ?", p![by, now_iso(), g.id()])?;
    board::log_event(app, g.i("task_id").unwrap_or(0), board::BOARD, "grant", &format!("Grant {}:{} confirmed by {by}", g.st("kind"), g.st("pattern")))?;
    crate::dispatch::withdraw_id(app, &notification_id(g.id()));
    Ok(())
}

/// The owner's confirm of pending grant `id` (from the app).
pub fn confirm(app: &App, id: i64, by: &str) -> Result<Value> {
    let g = get(app, id)?;
    if g.st("state") == "granted" {
        return Ok(json!({"line": format!("{} was already granted.", line(&g))}));
    }
    app.db.tx(|| confirm_row(app, &g, by))?;
    let task_id = g.i("task_id").unwrap_or(0);
    apply_live(app, task_id);
    Ok(json!({"line": format!("Granted {}.", line(&get(app, id)?))}))
}

/// Drops grant `id`: terminals opened later don't get it, and each live rule gets an `ask` over it.
pub fn drop_grant(app: &App, id: i64, by: &str) -> Result<Value> {
    let g = get(app, id)?;
    let task_id = g.i("task_id").unwrap_or(0);
    app.db.tx(|| {
        app.db.x("UPDATE task_grants SET removed_at = ?, state = 'dropped' WHERE id = ?", p![now_iso(), id])?;
        board::log_event(app, task_id, board::BOARD, "grant", &format!("Grant {} dropped by {by}", line(&g))).map(|_| ())
    })?;
    crate::dispatch::withdraw_id(app, &notification_id(id));
    let live = app.db.q("SELECT * FROM task_grant_rules WHERE grant_id = ? AND gone = 0", p![id])?;
    let mut held = vec![];
    for r in &live {
        let sid = r.st("session_id");
        let params = rule_params("ask", &g, &sid);
        match call_timeout(app, "rule.add", params, 5.0) {
            Ok(_) => {
                app.db.x("UPDATE task_grant_rules SET gone = 1 WHERE rule_id = ?", p![r.st("rule_id")])?;
            }
            Err(MidnaError::Down(m) | MidnaError::Refused(m)) => {
                app.info(format!("grant {} drop: couldn't ask over rule {} in {sid}: {m}", gref(id), r.st("rule_id")));
                held.push(sid);
            }
        }
    }
    let tail = if held.is_empty() { String::new() } else { format!(" Its rule in {} stays until that terminal closes or the rule expires.", held.join(", ")) };
    Ok(json!({"line": format!("Dropped {}.{tail}", line(&g))}))
}

fn rule_params(effect: &str, g: &Row, sid: &str) -> Value {
    json!({"effect": effect, "matcher": {"kind": g.st("kind"), "pattern": g.st("pattern")},
           "scope": {"kind": "session", "id": sid}, "expires_in_secs": RULE_SECS})
}

/// Adds terminal `sid`'s rule for granted grant `g`.
fn add_rule(app: &App, g: &Row, sid: &str) -> Result<()> {
    match call_timeout(app, "rule.add", rule_params("allow", g, sid), 5.0) {
        Ok(v) => {
            let rule_id = v["id"].as_str().map(str::to_string).unwrap_or_else(|| format!("unknown-{}-{sid}-{}", g.id(), now_ts()));
            let expires = v["expires_at"].as_str().map(str::to_string).unwrap_or_else(|| iso_in(RULE_SECS as f64));
            app.db.x(
                "INSERT OR REPLACE INTO task_grant_rules(rule_id, grant_id, session_id, at, expires_at) VALUES (?, ?, ?, ?, ?)",
                p![rule_id, g.id(), sid, now_iso(), expires],
            )?;
            app.info(format!("grant {} on {}: rule {rule_id} in {sid}", gref(g.id()), rf("task", g.i("task_id").unwrap_or(0))));
        }
        Err(MidnaError::Down(m) | MidnaError::Refused(m)) => {
            let text = format!("Grant {} couldn't be set in terminal {sid}: {m}", line(g));
            app.info(text.clone());
            board::log_event(app, g.i("task_id").unwrap_or(0), board::BOARD, "grant", &text)?;
        }
    }
    Ok(())
}

fn iso_in(secs: f64) -> String {
    chrono::DateTime::from_timestamp((now_ts() + secs) as i64, 0).map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string()).unwrap_or_default()
}

fn granted(app: &App, task_id: i64) -> Result<Vec<Row>> {
    app.db.q("SELECT * FROM task_grants WHERE task_id = ? AND state = 'granted' AND removed_at IS NULL ORDER BY id", p![task_id])
}

/// The board just opened terminal `sid` for task `task_id`: each confirmed grant gets its rule there.
/// A rule Midna refuses is logged on the task and doesn't hold the terminal back.
pub fn on_open(app: &App, task_id: i64, sid: &str) {
    let gs = match granted(app, task_id) {
        Ok(g) => g,
        Err(e) => return app.info(format!("grants for {}: {}", rf("task", task_id), e.message)),
    };
    for g in gs {
        if let Err(e) = add_rule(app, &g, sid) {
            app.info(format!("grant {} in {sid}: {}", gref(g.id()), e.message));
        }
    }
}

/// The live terminals the board opened for task `task_id`.
fn opened_terminals(app: &App, task_id: i64) -> Result<Vec<String>> {
    let rows = app.db.q(
        "SELECT DISTINCT json_extract(j.target, '$.session') AS sid FROM jobs j JOIN sessions s ON s.id = json_extract(j.target, '$.session')
         WHERE j.task_id = ? AND j.kind = 'agent' AND s.status != 'gone'",
        p![task_id],
    )?;
    Ok(rows.iter().map(|r| r.st("sid")).filter(|s| !s.is_empty()).collect())
}

/// Gives a newly granted grant its rule in the task's terminals that are up now, and renews any that
/// expired while the terminal stayed open.
fn apply_live(app: &App, task_id: i64) {
    let r = (|| -> Result<()> {
        let sids = opened_terminals(app, task_id)?;
        for g in granted(app, task_id)? {
            for sid in &sids {
                let now = now_iso();
                let has = app.db.q1(
                    "SELECT 1 FROM task_grant_rules WHERE grant_id = ? AND session_id = ? AND gone = 0 AND (expires_at IS NULL OR expires_at > ?)",
                    p![g.id(), sid.as_str(), now],
                )?;
                if has.is_none() {
                    add_rule(app, &g, sid)?;
                }
            }
        }
        Ok(())
    })();
    if let Err(e) = r {
        app.info(format!("grants for {}: {}", rf("task", task_id), e.message));
    }
}

/// Reads Midna's rules for each terminal rule's fired count, marks the ones gone (expired or the terminal
/// closed), and renews rules in terminals still up. At most every `TEND_SECS` unless `force`.
pub fn tend(app: &App, force: bool) -> Result<()> {
    let last = app.db.get_setting("task_grants_tended_at")?.and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    if !force && now_ts() - last < TEND_SECS {
        return Ok(());
    }
    app.db.set_setting("task_grants_tended_at", Some(&now_ts().to_string()))?;
    let live = app.db.q("SELECT * FROM task_grant_rules WHERE gone = 0", p![])?;
    if !live.is_empty() {
        let rules = match call_timeout(app, "rule.list", json!({}), 10.0) {
            Ok(v) => v.as_array().cloned().unwrap_or_default(),
            Err(MidnaError::Down(_) | MidnaError::Refused(_)) => return Ok(()),
        };
        for r in &live {
            let id = r.st("rule_id");
            match rules.iter().find(|m| m["id"].as_str() == Some(id.as_str())) {
                Some(m) => app.db.x(
                    "UPDATE task_grant_rules SET fired = ?, last_fired_at = ? WHERE rule_id = ?",
                    p![m["fired"].as_i64().unwrap_or(0), m["last_fired_at"].as_str(), id],
                )?,
                None => app.db.x("UPDATE task_grant_rules SET gone = 1 WHERE rule_id = ?", p![id])?,
            };
        }
    }
    let tasks = app.db.q("SELECT DISTINCT task_id FROM task_grants WHERE state = 'granted' AND removed_at IS NULL", p![])?;
    for t in tasks {
        apply_live(app, t.i("task_id").unwrap_or(0));
    }
    Ok(())
}

/// What a grant ask goes by among the alert waiters and the notification inbox: `task-grant:3`.
pub const KEY_PREFIX: &str = "task-grant:";

pub fn key(id: i64) -> String {
    format!("{KEY_PREFIX}{id}")
}

pub fn notification_id(id: i64) -> String {
    format!("taskboard-task-grant-{id}")
}

/// The grants waiting for the owner's answer that have a notification out: (id, notified_at).
pub fn asks(app: &App) -> Vec<(i64, String)> {
    app.db
        .q("SELECT id, notified_at FROM task_grants WHERE state = 'pending' AND removed_at IS NULL AND notified_at IS NOT NULL", p![])
        .unwrap_or_default()
        .iter()
        .map(|r| (r.id(), r.st("notified_at")))
        .collect()
}

fn ask(app: &App, id: i64, t: &Row) -> Result<()> {
    let at = now_iso();
    app.db.x("UPDATE task_grants SET notified_at = ? WHERE id = ?", p![at, id])?;
    let g = get(app, id)?;
    let params = json!({
        "title": format!("Grant {} {}:{}?", rf("task", t.id()), g.st("kind"), g.st("pattern")),
        "body": format!("Terminals the board opens for “{}” run it without a Midna prompt.\nAsked in {}", t.st("title"), g.st("asked_by")),
        "category": crate::midna::NOTIFY_ASK_KIND, "sound": true, "id": notification_id(id), "actions": ["Grant", "Not now"],
    });
    let (midna, inbox, waiters) = {
        let sh = app.shared.lock();
        (app.cfg.midna.clone(), sh.notify_inbox.clone(), sh.alert_waiters.clone())
    };
    let (k, nid) = (key(id), notification_id(id));
    std::thread::spawn(move || {
        if crate::midna::notify_send(&midna, &params).is_err() {
            return;
        }
        if let Some(w) = crate::dispatch::Waiter::start(waiters, &k, &at, &nid) {
            w.listen(&midna, &inbox);
        }
    });
    Ok(())
}

/// Acts on the owner's answers to grant asks, outside a transaction (a Grant adds Midna rules).
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

/// The owner's pick on a grant's notification: Grant confirms it, Not now or a dismissal drops it, a click
/// leaves it. Only its latest notification counts.
pub fn answer(app: &App, k: &str, response: &Value) -> Result<()> {
    let Some(id) = k.strip_prefix(KEY_PREFIX).and_then(|n| n.parse::<i64>().ok()) else { return Ok(()) };
    let r = response.get("response").filter(|r| r.is_object()).unwrap_or(response);
    let Some(g) = app.db.q1("SELECT * FROM task_grants WHERE id = ? AND state = 'pending' AND removed_at IS NULL", p![id])? else { return Ok(()) };
    if response.get("sent_at").and_then(|x| x.as_str()).is_some_and(|at| g.s("notified_at") != Some(at)) {
        return Ok(());
    }
    match (r["kind"].as_str(), r["action"].as_str()) {
        (Some("action"), Some("Grant")) => {
            confirm(app, id, &format!("{} from a notification", app.cfg.owner))?;
        }
        (Some("action"), Some("Not now")) | (Some("dismissed"), _) => {
            drop_grant(app, id, &format!("{} (Not now)", app.cfg.owner))?;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_like_midnas() {
        assert!(glob_match("mcp__slack__chat_post*", "mcp__slack__chat_postMessage"));
        assert!(!glob_match("mcp__slack__chat_post*", "mcp__slack__chat_delete"));
        assert!(glob_match("gh pr vie?", "gh pr view"));
        assert!(glob_match(r"a\*b", "a*b"));
        assert!(!glob_match(r"a\*b", "axb"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn parse_reads_kind_and_pattern() {
        assert_eq!(parse(" tool:mcp__slack__chat_post* "), Ok(("tool".to_string(), "mcp__slack__chat_post*".to_string())));
        assert_eq!(parse("command:git push origin t1:main"), Ok(("command".to_string(), "git push origin t1:main".to_string())));
        assert!(parse("slack:post").unwrap_err().contains("isn't a Midna rule kind"));
        assert!(parse("tool:").unwrap_err().contains("needs a pattern"));
        assert!(parse("nokind").unwrap_err().contains("<kind>:<pattern>"));
    }

    #[test]
    fn human_only_reaches_are_named() {
        assert!(reaches_human_only("cli", "notify respond*").unwrap().contains("notify.respond"));
        assert!(reaches_human_only("tool", "mcp__midna__share_*").unwrap().contains("share."));
        assert!(reaches_human_only("command", "midna rules remove r_1").is_some());
        assert_eq!(reaches_human_only("command", "git push*"), None);
        assert_eq!(reaches_human_only("path", "/tmp/*"), None);
        assert_eq!(reaches_human_only("cli", "send *"), None);
    }
}
