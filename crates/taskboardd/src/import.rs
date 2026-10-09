//! `taskboardd import <tasks.db>`: carry an old board's SQLite (the Python board this one replaces,
//! or an older taskboardd) into a fresh board, keeping every T/G/B number so refs stay valid.
//!
//! The old schema isn't fixed, so nothing here assumes it: the tables and columns are read from the
//! file itself. Each board table is filled from the old table of the same name (or a known older
//! name, like `backlog` for `issues`), column by column where the names match (or a known older
//! name does). Old tables and columns whose shape changed (devices and their loans and needs, bits
//! and their links, the Jira desk's terminal, PR links kept as repo and number, reviewers and their
//! asks, master breaks) are mapped into the board's own, reading their columns by any of the names
//! they went by. Old tables the board has no place for are kept whole in `settings` as
//! `import.<table>`, so nothing is lost. Jobs aren't carried over (the old board's pending work
//! would run again), nor are its alerts or its other running state.
//!
//! The old file is never opened in place: it's copied (with its `-wal` and `-shm`) to a scratch
//! folder first, so a running old board's database is only ever read.


use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::types::ValueRef;
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;

/// Board tables in the order they're filled, each with the older names its rows may sit under.
const TABLES: &[(&str, &[&str])] = &[
    ("goals", &[]),
    ("goal_notes", &["notes"]),
    ("goal_waves", &["waves"]),
    ("tasks", &[]),
    ("task_goals", &[]),
    ("events", &["task_events", "log"]),
    ("issues", &["backlog", "backlog_issues"]),
    ("issue_events", &["backlog_events"]),
    ("attachments", &[]),
    ("sessions", &["terminals"]),
    ("session_events", &["terminal_events"]),
    ("task_terminals", &[]),
    ("qa_comments", &[]),
    ("task_states", &[]),
    ("day_stats", &[]),
];

/// Old tables left behind on purpose (and SQLite's own).
const LEFT: &[&str] = &["jobs", "settings", "sqlite_sequence", "sqlite_stat1"];

/// Settings that describe the old board's running state rather than the owner's choices.
const LEFT_SETTINGS: &[&str] = &[
    "alerts",
    "tb_path",
    "plugin_version",
    "midna_projects",
    "usage_closed_for",
    "work_hours_today",
    "history_last_cleanup",
    "qa_checked_at",
    "sleeps",
];

/// Prefixes of running-state settings: the Midna bridge's, and the per-thing marks the old board
/// left so it wouldn't do a thing twice.
const LEFT_SETTING_PREFIXES: &[&str] = &["bridge_", "bridge:", "dispatch_seen:", "usage_guard_handled:", "review_round:", "last_"];

/// Whether an old setting is running state, not the owner's choice: a known key or prefix, a
/// per-thing mark (`<what>_seen:<id>`, `<what>_handled:<id>`, `<what>_round:<id>`), a health
/// reading (`review_log_health`) or a live session or process (`jira_desk_session`, `…_pid`).
pub fn runtime_setting(key: &str) -> bool {
    let k = key.to_lowercase();
    if LEFT_SETTINGS.contains(&k.as_str()) || LEFT_SETTING_PREFIXES.iter().any(|p| k.starts_with(p)) {
        return true;
    }
    let head = k.split(':').next().unwrap_or("");
    if ["_health", "_session", "_session_id", "_pid"].iter().any(|s| k.ends_with(s)) {
        return true;
    }
    k.contains(':') && ["_seen", "_handled", "_round", "_sent", "_done", "_at", "_lock", "_cursor"].iter().any(|s| head.ends_with(s))
}

/// Older names a board column may go by in an old table.
const COLUMNS: &[(&str, &[&str])] = &[
    ("claude_session_id", &["conversation_id", "claude_session", "conversation"]),
    ("goal_id", &["goal"]),
    ("task_id", &["task"]),
    ("issue_id", &["backlog_id", "issue"]),
    ("session_id", &["terminal_id", "terminal"]),
    ("created_at", &["created"]),
    ("updated_at", &["updated"]),
    ("detail", &["description", "body"]),
    ("setup", &["task_setup"]),
];

/// Old tables mapped by hand below, not by name.
const MAPPED: &[&str] = &[
    "devices", "device_loans", "goal_devices", "device_needs", "bits", "bit_links", "task_bits", "goal_bits",
    "reviewers", "reviewer_roster", "roster", "review_asks", "reviewer_asks", "asks", "reviewer_bot_runs", "bot_runs",
    "master_breaks", "breaks",
];

#[derive(Debug, Default)]
pub struct Report {
    /// (board table, rows written) for every table filled.
    pub copied: Vec<(String, usize)>,
    /// Old rows (or values) that didn't come over, each with why.
    pub skipped: Vec<String>,
    /// Old settings left behind as the old board's running state.
    pub left_settings: Vec<String>,
    /// (old table, rows) kept in settings as `import.<table>`.
    pub kept: Vec<(String, usize)>,
    /// Settings carried over.
    pub settings: Vec<String>,
    /// The highest T, G and B numbers carried over.
    pub last: Vec<(&'static str, i64)>,
}

impl Report {
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![];
        for (t, n) in &self.copied {
            out.push(format!("{t}: {n}"));
        }
        if !self.settings.is_empty() {
            out.push(format!("settings: {}", self.settings.join(", ")));
        }
        if !self.left_settings.is_empty() {
            out.push(format!("settings left behind (the old board's running state): {}", self.left_settings.len()));
        }
        for (t, n) in &self.kept {
            out.push(format!("{t}: {n} rows kept as settings import.{t} (the board has no table for them yet)"));
        }
        if !self.skipped.is_empty() {
            out.push(format!("skipped: {}", self.skipped.len()));
            out.extend(self.skipped.iter().map(|s| format!("  {s}")));
        }
        let last: Vec<String> = self.last.iter().filter(|(_, n)| *n > 0).map(|(k, n)| format!("{k}{n}")).collect();
        if !last.is_empty() {
            out.push(format!("Numbering carries on after {}.", last.join(", ")));
        }
        out
    }
}

fn sql_err(e: rusqlite::Error) -> ApiError {
    ApiError::new(500, format!("the old database: {e}"))
}

fn to_json(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => json!(f),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
    }
}

/// Copies the old file (and its `-wal`/`-shm`) into a fresh scratch folder.
fn scratch_copy(src: &Path) -> Result<(PathBuf, PathBuf)> {
    if !src.is_file() {
        return err(404, format!("There's no file at {}.", src.display()));
    }
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("taskboard-import-{}-{}-{n}", std::process::id(), now_ts() as i64));
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::new(500, format!("couldn't make {}: {e}", dir.display())))?;
    let copy = dir.join("old.db");
    std::fs::copy(src, &copy).map_err(|e| ApiError::new(500, format!("couldn't copy {}: {e}", src.display())))?;
    for ext in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{ext}", src.display()));
        if side.is_file() {
            let _ = std::fs::copy(&side, dir.join(format!("old.db{ext}")));
        }
    }
    Ok((dir, copy))
}

fn old_tables(c: &Connection) -> Result<Vec<String>> {
    let mut st = c.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").map_err(sql_err)?;
    let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(sql_err)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(sql_err)
}


fn old_rows(c: &Connection, table: &str) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    let mut st = c.prepare(&format!("SELECT * FROM \"{}\"", table.replace('"', ""))).map_err(sql_err)?;
    let names: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
    let n = names.len();
    let mut rows = st.query([]).map_err(sql_err)?;
    let mut out = vec![];
    while let Some(r) = rows.next().map_err(sql_err)? {
        out.push((0..n).map(|i| r.get_ref(i).map(to_json).unwrap_or(Value::Null)).collect());
    }
    Ok((names, out))
}

/// Where each board column comes from in the old table: its own name, any case, or an older name.
pub fn column_map(board: &[String], old: &[String]) -> Vec<(String, usize)> {
    let find = |name: &str| old.iter().position(|o| o.eq_ignore_ascii_case(name));
    board
        .iter()
        .filter_map(|b| {
            let i = find(b).or_else(|| COLUMNS.iter().find(|(k, _)| k == b).and_then(|(_, alts)| alts.iter().find_map(|a| find(a))))?;
            Some((b.clone(), i))
        })
        .collect()
}

fn find_old<'a>(old: &'a [String], name: &str, alts: &[&str]) -> Option<&'a String> {
    std::iter::once(name).chain(alts.iter().copied()).find_map(|n| old.iter().find(|o| o.eq_ignore_ascii_case(n)))
}

/// Fills a fresh board from the old database at `src`. Refuses a board that already has tasks,
/// goals or backlog issues: their numbers would clash with the old ones.
pub fn import(app: &App, src: &Path) -> Result<Report> {
    let have = app.db.count("SELECT (SELECT COUNT(*) FROM tasks) + (SELECT COUNT(*) FROM goals) + (SELECT COUNT(*) FROM issues)", vec![])?;
    if have > 0 {
        return err(409, format!("This board already has tasks, goals or backlog issues, so the old numbers would clash. Import into a fresh data folder (taskboardd import --data <empty folder> {}).", src.display()));
    }
    let (dir, copy) = scratch_copy(src)?;
    let out = (|| {
        let c = Connection::open(&copy).map_err(sql_err)?;
        let old = old_tables(&c)?;
        if !old.iter().any(|t| t == "tasks" || t == "goals") {
            return err(400, format!("{} doesn't look like a task board's database: it has no tasks or goals table.", src.display()));
        }
        app.db.tx(|| fill(app, &c, &old))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    out
}

fn fill(app: &App, c: &Connection, old: &[String]) -> Result<Report> {
    let mut rep = Report::default();
    let mut used: Vec<String> = vec![];
    for (table, alts) in TABLES {
        let Some(src) = find_old(old, table, alts) else { continue };
        used.push(src.clone());
        let board_cols: Vec<String> = app.db.q(&format!("PRAGMA table_info({table})"), vec![])?.iter().map(|r| r.st("name")).collect();
        let (names, rows) = old_rows(c, src)?;
        let map = column_map(&board_cols, &names);
        if map.is_empty() {
            continue;
        }
        if *table == "task_states" {
            // The tasks' insert trigger wrote a state for each; the old board's own history replaces them.
            app.db.x("DELETE FROM task_states", vec![])?;
        }
        let cols: Vec<&str> = map.iter().map(|(b, _)| b.as_str()).collect();
        let sql = format!("INSERT OR IGNORE INTO {table} ({}) VALUES ({})", cols.join(", "), vec!["?"; cols.len()].join(", "));
        let mut n = 0;
        for r in &rows {
            app.db.x(&sql, map.iter().map(|(_, i)| r[*i].clone()).collect())?;
            if wrote(app)? {
                n += 1;
            } else {
                rep.skipped.push(format!("{src} {}: a row with the same key came first", row_label(table, &map, r)));
            }
        }
        // The sessions' insert trigger stamps status_at with now; put the old one back.
        if *table == "sessions" {
            if let (Some((_, si)), Some((_, ii))) = (map.iter().find(|(b, _)| b == "status_at"), map.iter().find(|(b, _)| b == "id")) {
                for r in &rows {
                    app.db.x("UPDATE sessions SET status_at = ? WHERE id = ?", vec![r[*si].clone(), r[*ii].clone()])?;
                }
            }
        }
        rep.copied.push((table.to_string(), n));
    }
    // The owner's settings (work hours, PR flows, QA, …), where the new board has none yet.
    if old.iter().any(|t| t == "settings") {
        let (names, rows) = old_rows(c, "settings")?;
        if let (Some(k), Some(v)) = (names.iter().position(|n| n == "key"), names.iter().position(|n| n == "value")) {
            for r in rows {
                let Some(key) = r[k].as_str().map(str::to_string) else { continue };
                if runtime_setting(&key) {
                    rep.left_settings.push(key);
                    continue;
                }
                if key.starts_with("import.") || app.db.get_setting(&key)?.is_some() {
                    continue;
                }
                let val = match &r[v] {
                    Value::String(s) => Some(s.clone()),
                    Value::Null => None,
                    other => Some(other.to_string()),
                };
                app.db.set_setting(&key, val.as_deref())?;
                rep.settings.push(key);
            }
        }
    }
    if devices(app, c, old, &mut rep)? {
        goal_pools(app, c, old, &mut rep)?;
    }
    bits(app, c, old, &mut rep)?;
    jira_desk(app, c, old, &mut rep)?;
    pr_links(app, &mut rep)?;
    reviewers(app, c, old, &mut rep)?;
    breaks(app, c, old, &mut rep)?;
    // Everything else, kept whole for whatever needs it later.
    for t in old {
        let mapped = MAPPED.iter().any(|m| t.eq_ignore_ascii_case(m));
        if used.contains(t) || LEFT.contains(&t.as_str()) || mapped || t.starts_with("sqlite_") {
            continue;
        }
        let (names, rows) = old_rows(c, t)?;
        let n = rows.len();
        let objs: Vec<Value> = rows.into_iter().map(|r| Value::Object(names.iter().cloned().zip(r).collect())).collect();
        app.db.set_setting(&format!("import.{t}"), Some(&jdumps(&json!({"columns": names, "rows": objs, "imported_at": now_iso()}))))?;
        rep.kept.push((t.clone(), n));
    }
    for (k, table) in [("T", "tasks"), ("G", "goals"), ("B", "issues")] {
        rep.last.push((k, app.db.count(&format!("SELECT COALESCE(MAX(id), 0) FROM {table}"), vec![])?));
    }
    Ok(rep)
}

/// Whether the last insert wrote a row (an `OR IGNORE` that met a clash didn't).
fn wrote(app: &App) -> Result<bool> {
    Ok(app.db.count("SELECT changes()", vec![])? > 0)
}

/// How a skipped row is named in the report: its ref, or its key columns.
fn row_label(table: &str, map: &[(String, usize)], r: &[Value]) -> String {
    let at = |c: &str| map.iter().find(|(b, _)| b == c).map(|(_, i)| &r[*i]).filter(|v| !v.is_null());
    let kind = match table {
        "tasks" => "task",
        "goals" => "goal",
        "issues" => "issue",
        _ => "",
    };
    if let (false, Some(id)) = (kind.is_empty(), at("id").and_then(|v| v.as_i64())) {
        return rf(kind, id);
    }
    let keys: Vec<String> = ["id", "task_id", "goal_id", "session_id", "wave", "comment_id", "date", "project"]
        .iter()
        .filter_map(|c| at(c).map(|v| format!("{c}={}", text(v).unwrap_or_default())))
        .take(2)
        .collect();
    if keys.is_empty() { "a row".into() } else { keys.join(" ") }
}

/// A value as text: a string (trimmed, not empty) or a number.
fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// An old list column: a JSON list, or words split by commas or spaces.
fn words(v: &Value) -> Vec<Value> {
    match v {
        Value::Null => vec![],
        Value::Array(a) => a.clone(),
        Value::String(s) => match serde_json::from_str::<Value>(s.trim()) {
            Ok(Value::Array(a)) => a,
            Ok(o @ Value::Object(_)) => vec![o],
            _ => s.replace(',', " ").split_whitespace().map(|w| json!(w)).collect(),
        },
        other => vec![other.clone()],
    }
}

/// A device name or tag from whatever the old board called it: lowercase, odd characters as dashes.
fn slug(s: &str) -> Option<String> {
    let mut out = String::new();
    for c in s.trim().to_lowercase().chars() {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':') { c } else { '-' };
        if !(c == '-' && (out.is_empty() || out.ends_with('-'))) {
            out.push(c);
        }
    }
    let out: String = out.trim_matches(|c: char| !c.is_ascii_alphanumeric()).chars().take(64).collect();
    (!out.is_empty()).then_some(out)
}

/// One old table read whole, its columns looked up by any of the names they went by.
struct Old {
    table: String,
    names: Vec<String>,
    rows: Vec<Vec<Value>>,
}

const NULL: Value = Value::Null;

impl Old {
    fn read(c: &Connection, old: &[String], name: &str, alts: &[&str]) -> Result<Option<Old>> {
        let Some(t) = find_old(old, name, alts) else { return Ok(None) };
        let (names, rows) = old_rows(c, t)?;
        Ok(Some(Old { table: t.clone(), names, rows }))
    }

    fn has(&self, cols: &[&str]) -> bool {
        cols.iter().any(|c| self.names.iter().any(|n| n.eq_ignore_ascii_case(c)))
    }

    /// The first of `cols` the row has a value in.
    fn get<'a>(&self, r: &'a [Value], cols: &[&str]) -> &'a Value {
        cols.iter()
            .filter_map(|c| self.names.iter().position(|n| n.eq_ignore_ascii_case(c)))
            .map(|i| &r[i])
            .find(|v| !v.is_null() && v.as_str() != Some(""))
            .unwrap_or(&NULL)
    }

    fn text(&self, r: &[Value], cols: &[&str]) -> Option<String> {
        text(self.get(r, cols))
    }

    fn id(&self, r: &[Value], cols: &[&str]) -> Option<i64> {
        match self.get(r, cols) {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => parse_ref_str(s, "").ok().flatten(),
            _ => None,
        }
    }

    fn truthy(&self, r: &[Value], cols: &[&str]) -> bool {
        as_bool(Some(self.get(r, cols)), false)
    }
}

/// One old need word as the board's: `device:<id>` is that one device (by its board name), `tag:x`
/// (or `tag:x:2`) is tag `x`; anything else is already a tag or name with an optional count.
fn old_need(w: &Value, device_of: &dyn Fn(&Value) -> Option<String>) -> Value {
    let Some(s) = w.as_str().map(str::trim) else { return w.clone() };
    let (head, rest) = match s.split_once(':') {
        Some((h, r)) => (h.to_lowercase(), r),
        None => return w.clone(),
    };
    match head.as_str() {
        "device" | "dev" => {
            let (id, n) = match rest.rsplit_once(':') {
                Some((id, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => (id, n.parse().unwrap_or(1)),
                _ => (rest, 1),
            };
            let name = device_of(&json!(id)).or_else(|| slug(id)).unwrap_or_else(|| id.to_string());
            json!({"tag": name, "n": n})
        }
        "tag" | "kind" => json!(rest),
        _ => w.clone(),
    }
}

/// Device needs from old values (`android:2 ios`, a JSON list, `{"tag": …, "n": …}`), as the board
/// stores them; None when they ask for nothing.
fn needs_value(items: Vec<Value>) -> Result<Option<String>> {
    let needs = crate::devices::clean_needs(Some(&Value::Array(items)))?;
    if needs.is_empty() {
        return Ok(None);
    }
    Ok(Some(jdumps(&json!(needs.iter().map(|x| json!({"tag": x.tag, "n": x.n})).collect::<Vec<_>>()))))
}

fn set_needs(app: &App, owner: &str, needs: &str) -> Result<()> {
    app.db.x(
        "INSERT INTO device_needs(owner, needs) VALUES(?, ?) ON CONFLICT(owner) DO UPDATE SET needs = excluded.needs",
        vec![json!(owner), json!(needs)],
    )?;
    Ok(())
}

/// The device pool, its loans, and what tasks (`tasks.device_need`) and goals (`goal_devices`) ask for.
/// True when `goal_devices` is a goal's own pool (purpose, reserved) rather than its needs.
fn devices(app: &App, c: &Connection, old: &[String], rep: &mut Report) -> Result<bool> {
    // Old device id or name → the board's name for it.
    let mut names: HashMap<String, String> = HashMap::new();
    if let Some(d) = Old::read(c, old, "devices", &[])? {
        let mut n = 0;
        for r in &d.rows {
            let raw = d.text(r, &["name", "label", "id"]);
            let Some(name) = raw.as_deref().and_then(slug) else {
                rep.skipped.push(format!("{} {}: no name", d.table, d.text(r, &["id"]).unwrap_or_default()));
                continue;
            };
            let mut tags: Vec<String> = vec![];
            for v in [d.get(r, &["tags", "tag", "kinds"]), d.get(r, &["kind", "platform", "type", "os"])] {
                for w in words(v) {
                    if let Some(t) = text(&w).as_deref().and_then(slug).map(|t| t.replace(':', "-")) {
                        if t != name && !tags.contains(&t) {
                            tags.push(t);
                        }
                    }
                }
            }
            for k in [d.text(r, &["id"]), raw.clone()].into_iter().flatten() {
                names.entry(k.to_lowercase()).or_insert_with(|| name.clone());
            }
            // Taken out of the old pool: the board deletes a removed device, so it doesn't come over.
            if !d.get(r, &["removed_at"]).is_null() || d.truthy(r, &["removed"]) {
                rep.skipped.push(format!("{} {name}: removed from the old pool", d.table));
                continue;
            }
            let (kept, kept_note) = old_blocked(&d, r);
            let off = kept || d.truthy(r, &["off", "disabled", "retired"]) || (d.has(&["enabled"]) && !d.truthy(r, &["enabled"]));
            let note = match (text(d.get(r, &["note", "notes", "description", "detail"])), kept_note) {
                (Some(n), Some(k)) => json!(format!("{n}; {k}")),
                (n, k) => json!(n.or(k)),
            };
            app.db.x(
                "INSERT OR IGNORE INTO devices(name, tags, focus, note, off, created_at, updated_at) VALUES(?, ?, ?, ?, ?, ?, ?)",
                vec![
                    json!(name),
                    json!(jdumps(&json!(tags))),
                    d.get(r, &["focus", "focus_cmd", "focus_command"]).clone(),
                    note,
                    json!(off as i64),
                    d.get(r, &["created_at", "created", "added_at"]).clone(),
                    d.get(r, &["updated_at", "updated"]).clone(),
                ],
            )?;
            if wrote(app)? {
                n += 1;
            } else {
                rep.skipped.push(format!("{} {name}: a device with that name came first", d.table));
            }
        }
        rep.copied.push(("devices".into(), n));
    }
    let device_of = |v: &Value| -> Option<String> {
        let k = text(v)?;
        names.get(&k.to_lowercase()).cloned().or_else(|| slug(&k).filter(|s| names.values().any(|n| n == s)))
    };
    if let Some(l) = Old::read(c, old, "device_loans", &["loans"])? {
        let mut n = 0;
        for r in &l.rows {
            let dev = device_of(l.get(r, &["device", "device_name", "device_id", "name"]));
            let (Some(name), Some(task)) = (dev, l.id(r, &["task_id", "task"])) else {
                rep.skipped.push(format!("{} {}: its device or task isn't known", l.table, l.text(r, &["id"]).unwrap_or_default()));
                continue;
            };
            // A loan the old board never took back from a finished task is over.
            let mut released = l.get(r, &["released_at", "returned_at", "ended_at", "released"]).clone();
            if released.is_null() {
                if let Some(t) = task_of(app, Some(task))?.filter(|t| t.s("status") == Some("done")) {
                    released = [t.v("finished_at"), t.v("updated_at")].into_iter().find(|v| !v.is_null()).unwrap_or_else(|| json!(now_iso()));
                }
            }
            app.db.insert(
                "device_loans",
                vec![
                    ("device", json!(name)),
                    ("task_id", json!(task)),
                    ("at", l.get(r, &["at", "lent_at", "created_at", "started_at"]).clone()),
                    ("released_at", released),
                ],
            )?;
            n += 1;
        }
        rep.copied.push(("device_loans".into(), n));
    }
    let mut n = 0;
    // What each task asks for, from its own column.
    const TASK_NEED: &[&str] = &["device_need", "device_needs", "devices"];
    if let Some(t) = Old::read(c, old, "tasks", &[])?.filter(|t| t.has(TASK_NEED)) {
        for r in &t.rows {
            let Some(id) = t.id(r, &["id"]) else { continue };
            let v = t.get(r, TASK_NEED);
            // An explicit "no devices" is the task's own "needs none", over its goal's needs.
            if text(v).map(|s| matches!(s.to_lowercase().as_str(), "none" | "[]" | "no" | "-")).unwrap_or(false) {
                set_needs(app, &rf("task", id), "[]")?;
                n += 1;
                continue;
            }
            match needs_value(words(v).into_iter().map(|w| old_need(&w, &device_of)).collect()) {
                Ok(Some(needs)) => {
                    set_needs(app, &rf("task", id), &needs)?;
                    n += 1;
                }
                Ok(None) => {}
                Err(e) => rep.skipped.push(format!("{} device need “{}”: {}", rf("task", id), text(v).unwrap_or_default(), e.message)),
            }
        }
    }
    // What each goal asks for: a need per row (a tag and a count, a device, or the need as text).
    // Rows with a purpose or a reserved flag are a goal's own device pool, not a need for one of
    // each: `goal_pools` maps those.
    const GOAL_NEED: &[&str] = &["need", "needs", "device_need", "spec"];
    let mut goal_pool = false;
    if let Some(g) = Old::read(c, old, "goal_devices", &[])?.filter(|g| {
        goal_pool = g.has(&["purpose", "reserved"]) && !g.has(GOAL_NEED);
        !goal_pool
    }) {
        let mut per: Vec<(i64, Vec<Value>)> = vec![];
        for r in &g.rows {
            let Some(goal) = g.id(r, &["goal_id", "goal"]) else { continue };
            let items: Vec<Value> = if !g.get(r, GOAL_NEED).is_null() {
                words(g.get(r, GOAL_NEED))
            } else {
                let tag = g.get(r, &["tag", "kind", "platform", "device", "device_id", "device_name", "name"]);
                let tag = device_of(tag).or_else(|| text(tag).as_deref().and_then(slug));
                let count = g.id(r, &["n", "count", "qty", "quantity", "num"]).unwrap_or(1);
                tag.map(|t| vec![json!({"tag": t, "n": count})]).unwrap_or_default()
            };
            match per.iter_mut().find(|(x, _)| *x == goal) {
                Some((_, v)) => v.extend(items),
                None => per.push((goal, items)),
            }
        }
        for (goal, items) in per {
            match needs_value(items) {
                Ok(Some(needs)) => {
                    set_needs(app, &rf("goal", goal), &needs)?;
                    n += 1;
                }
                Ok(None) => {}
                Err(e) => rep.skipped.push(format!("{} devices: {}", rf("goal", goal), e.message)),
            }
        }
    }
    if n > 0 {
        rep.copied.push(("device_needs".into(), n));
    }
    Ok(goal_pool)
}

/// An old device's `blocked` ("kept for …"): whether it's kept back, and the note that says what for.
fn old_blocked(d: &Old, r: &[Value]) -> (bool, Option<String>) {
    let v = d.get(r, &["blocked", "blocked_for", "kept_for", "blocked_reason"]);
    match v {
        Value::Null | Value::Bool(false) => (false, None),
        Value::Bool(true) => (true, None),
        Value::Number(n) => (n.as_f64().unwrap_or(0.0) != 0.0, None),
        Value::String(s) => match s.trim().to_lowercase().as_str() {
            "0" | "false" | "no" | "off" => (false, None),
            "1" | "true" | "yes" | "on" => (true, None),
            l if l.starts_with("kept for ") => (true, Some(format!("Kept for {}", &s.trim()[9..]))),
            _ => (true, Some(format!("Kept for {}", s.trim()))),
        },
        other => (true, text(other).map(|s| format!("Kept for {s}"))),
    }
}

/// A goal's own device pool from the old `goal_devices` (a device, its purpose, reserved), into the
/// board's `goal_devices`. The purpose counts as a tag, so it's made one (`Payments on Android` →
/// `payments-on-android`).
fn goal_pools(app: &App, c: &Connection, old: &[String], rep: &mut Report) -> Result<()> {
    let Some(g) = Old::read(c, old, "goal_devices", &[])? else { return Ok(()) };
    // Old device id or name → the board's name, as `devices` named them.
    let mut names: HashMap<String, String> = HashMap::new();
    if let Some(d) = Old::read(c, old, "devices", &[])? {
        for r in &d.rows {
            let raw = d.text(r, &["name", "label", "id"]);
            if let Some(name) = raw.as_deref().and_then(slug) {
                for k in [d.text(r, &["id"]), raw].into_iter().flatten() {
                    names.entry(k.to_lowercase()).or_insert_with(|| name.clone());
                }
            }
        }
    }
    let mut n = 0;
    for r in &g.rows {
        let dev = g.text(r, &["device", "device_id", "device_name", "name"]);
        let name = dev.as_deref().and_then(|k| names.get(&k.to_lowercase()).cloned().or_else(|| slug(k)));
        let (Some(goal), Some(name)) = (g.id(r, &["goal_id", "goal"]), name) else {
            rep.skipped.push(format!("{} {}: its goal or device isn't known", g.table, g.text(r, &["id"]).unwrap_or_default()));
            continue;
        };
        if app.db.q1("SELECT 1 FROM devices WHERE name = ?", vec![json!(name)])?.is_none() || app.db.q1("SELECT 1 FROM goals WHERE id = ?", vec![json!(goal)])?.is_none() {
            rep.skipped.push(format!("{} {} {name}: that goal or device didn't come over", g.table, rf("goal", goal)));
            continue;
        }
        let purpose = g.text(r, &["purpose", "role", "for"]).as_deref().and_then(slug).map(|p| p.replace(':', "-"));
        let mut reserved = g.truthy(r, &["reserved", "reserve"]);
        // One goal reserves a device; a later claim keeps it in that goal's pool, unreserved.
        if reserved && app.db.q1("SELECT 1 FROM goal_devices WHERE device = ? AND reserved = 1 AND goal_id != ?", vec![json!(name), json!(goal)])?.is_some() {
            rep.skipped.push(format!("{} {} {name}: reserved for another goal first, so it's in the pool unreserved", g.table, rf("goal", goal)));
            reserved = false;
        }
        app.db.x(
            "INSERT OR IGNORE INTO goal_devices(goal_id, device, purpose, reserved, at) VALUES(?, ?, ?, ?, ?)",
            vec![json!(goal), json!(name), json!(purpose), json!(reserved as i64), g.get(r, &["created_at", "added_at", "at"]).clone()],
        )?;
        if wrote(app)? {
            n += 1;
        }
    }
    rep.copied.push(("goal_devices".into(), n));
    Ok(())
}

/// A bit to link, by old id or name, to a task or a goal.
struct BitLink {
    bit: Value,
    task: Option<i64>,
    goal: Option<i64>,
    at: Option<String>,
}

/// The bits, and their links: from the old bits rows, link tables, and `tasks.bits` / `goals.bits`.
fn bits(app: &App, c: &Connection, old: &[String], rep: &mut Report) -> Result<()> {
    // Old bit id or name → the board's bit id.
    let mut ids: HashMap<String, i64> = HashMap::new();
    let mut links: Vec<BitLink> = vec![];
    if let Some(b) = Old::read(c, old, "bits", &["flags", "feature_flags"])? {
        let mut n = 0;
        for r in &b.rows {
            let Some(name) = b.text(r, &["name", "key", "flag", "bit"]) else {
                rep.skipped.push(format!("{} {}: no name", b.table, b.text(r, &["id"]).unwrap_or_default()));
                continue;
            };
            let kind = match b.text(r, &["kind", "type", "where"]).map(|k| k.to_lowercase()) {
                Some(k) if k == "local" || k == "code" => "local",
                Some(_) => "backend",
                None if b.truthy(r, &["local"]) => "local",
                None if b.has(&["backend", "remote"]) && !b.truthy(r, &["backend", "remote"]) => "local",
                None => "backend",
            };
            let mut made_at = b.get(r, &["made_at", "created_in_tool_at", "made_on", "done_at"]).clone();
            if made_at.is_null() && b.truthy(r, &["made", "created_in_tool", "done"]) {
                made_at = b.get(r, &["updated_at", "updated", "created_at"]).clone();
                if made_at.is_null() {
                    made_at = json!(now_iso());
                }
            }
            let id = b.id(r, &["id"]);
            app.db.x(
                "INSERT OR IGNORE INTO bits(id, name, kind, project, note, made_at, made_by, created_at, updated_at) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?)",
                vec![
                    json!(id),
                    json!(name),
                    json!(kind),
                    b.get(r, &["project"]).clone(),
                    b.get(r, &["note", "notes", "description", "detail"]).clone(),
                    made_at,
                    b.get(r, &["made_by"]).clone(),
                    b.get(r, &["created_at", "created"]).clone(),
                    b.get(r, &["updated_at", "updated"]).clone(),
                ],
            )?;
            if !wrote(app)? {
                rep.skipped.push(format!("{} {name}: a bit with that name or id came first", b.table));
                continue;
            }
            n += 1;
            let new = app.db.count("SELECT id FROM bits WHERE name = ?", vec![json!(name)])?;
            ids.insert(name.to_lowercase(), new);
            if let Some(id) = id {
                ids.insert(id.to_string(), new);
            }
            let at = b.text(r, &["created_at", "created"]);
            if let Some(x) = b.id(r, &["task_id", "task"]) {
                links.push(BitLink { bit: json!(new), task: Some(x), goal: None, at: at.clone() });
            }
            if let Some(x) = b.id(r, &["goal_id", "goal"]) {
                links.push(BitLink { bit: json!(new), task: None, goal: Some(x), at: at.clone() });
            }
            for w in words(b.get(r, &["tasks"])) {
                if let Ok(Some(x)) = parse_ref(&w, "task") {
                    links.push(BitLink { bit: json!(new), task: Some(x), goal: None, at: at.clone() });
                }
            }
            for w in words(b.get(r, &["goals"])) {
                if let Ok(Some(x)) = parse_ref(&w, "goal") {
                    links.push(BitLink { bit: json!(new), task: None, goal: Some(x), at: at.clone() });
                }
            }
        }
        rep.copied.push(("bits".into(), n));
    }
    // Link tables: a bit (by id or name) with a task or a goal.
    for t in ["bit_links", "task_bits", "goal_bits"] {
        let Some(l) = Old::read(c, old, t, &[])? else { continue };
        for r in &l.rows {
            links.push(BitLink {
                bit: l.get(r, &["bit_id", "bit", "name", "flag", "key"]).clone(),
                task: l.id(r, &["task_id", "task"]),
                goal: l.id(r, &["goal_id", "goal"]),
                at: l.text(r, &["at", "created_at"]),
            });
        }
    }
    // A task's or a goal's own list of bit names.
    for (table, kind) in [("tasks", "task"), ("goals", "goal")] {
        let Some(t) = Old::read(c, old, table, &[])?.filter(|t| t.has(&["bits", "flags"])) else { continue };
        for r in &t.rows {
            let Some(id) = t.id(r, &["id"]) else { continue };
            for w in words(t.get(r, &["bits", "flags"])) {
                let (task, goal) = if kind == "task" { (Some(id), None) } else { (None, Some(id)) };
                links.push(BitLink { bit: w, task, goal, at: None });
            }
        }
    }
    let mut n = 0;
    for l in links {
        let Some(key) = text(&l.bit) else { continue };
        let Some(who) = l.task.map(|x| rf("task", x)).or(l.goal.map(|x| rf("goal", x))) else {
            rep.skipped.push(format!("bit {key}: linked to no task or goal"));
            continue;
        };
        let id = match ids.get(&key.to_lowercase()) {
            Some(id) => *id,
            // A name only the task or goal knew: a bit of its own, local (nothing says it's in the tool).
            None if key.parse::<i64>().is_err() => {
                let now = now_iso();
                app.db.x("INSERT OR IGNORE INTO bits(name, kind, created_at, updated_at) VALUES(?, 'local', ?, ?)", vec![json!(key), json!(now), json!(now)])?;
                let id = app.db.count("SELECT id FROM bits WHERE name = ?", vec![json!(key)])?;
                ids.insert(key.to_lowercase(), id);
                id
            }
            None => {
                rep.skipped.push(format!("{who} bit {key}: no such bit"));
                continue;
            }
        };
        let have = app.db.count("SELECT COUNT(*) FROM bit_links WHERE bit_id = ? AND task_id IS ? AND goal_id IS ?", vec![json!(id), json!(l.task), json!(l.goal)])?;
        if have > 0 {
            continue;
        }
        let at = l.at.unwrap_or_else(now_iso);
        app.db.insert("bit_links", vec![("bit_id", json!(id)), ("task_id", json!(l.task)), ("goal_id", json!(l.goal)), ("at", json!(at))])?;
        n += 1;
    }
    if n > 0 {
        rep.copied.push(("bit_links".into(), n));
    }
    Ok(())
}

/// The old board's Jira desk terminal (`sessions.jira_desk`), marked the way the board knows its
/// desk: by the agent job that opened it.
fn jira_desk(app: &App, c: &Connection, old: &[String], rep: &mut Report) -> Result<()> {
    let Some(s) = Old::read(c, old, "sessions", &["terminals"])?.filter(|s| s.has(&["jira_desk", "desk"])) else { return Ok(()) };
    let mut n = 0;
    for r in &s.rows {
        let Some(sid) = s.text(r, &["id"]) else { continue };
        if !s.truthy(r, &["jira_desk", "desk"]) {
            continue;
        }
        let now = now_iso();
        app.db.insert(
            "jobs",
            vec![
                ("kind", json!("agent")),
                ("args", json!(jdumps(&json!({"title": "Jira desk", "imported": true})))),
                ("state", json!("done")),
                ("created_at", json!(now)),
                ("updated_at", json!(now)),
                ("attempts", json!(0)),
                ("purpose", json!(crate::jira_desk::PURPOSE)),
                ("target", json!(jdumps(&json!({"session": sid})))),
            ],
        )?;
        n += 1;
    }
    if n > 0 {
        rep.copied.push(("jira desk terminals".into(), n));
    }
    Ok(())
}

/// The host and repo of a checkout's remote (`[pr_body] remote`), looked up once per path.
fn remote_of(app: &App, path: &str, cache: &mut HashMap<String, Option<(String, String)>>) -> Option<(String, String)> {
    cache
        .entry(path.to_string())
        .or_insert_with(|| {
            let git = crate::proc::which("git")?;
            let args: Vec<String> = vec!["-C".into(), path.into(), "remote".into(), "get-url".into(), app.cfg.pr_body.remote.clone()];
            let o = crate::proc::run(&git, &args, None, 5.0).ok()?;
            if o.code != Some(0) {
                return None;
            }
            crate::prhost::repo_of_remote(o.stdout.trim())
        })
        .clone()
}

/// A PR's page on its host.
pub fn pr_page(host: &str, repo: &str, num: i64) -> Option<String> {
    match host {
        "github" => Some(format!("https://github.com/{repo}/pull/{num}")),
        "bitbucket" => Some(format!("https://bitbucket.org/{repo}/pull-requests/{num}")),
        _ => None,
    }
}

/// The old board kept a PR as `pr_repo` and `pr_num`; the board watches it by `pr_host` and links it
/// by `pr_url`, and reads it through `pr_repo` as `owner/name`. All three come from the link when
/// there is one, else the repo, else the project's remote; an old bare repo name (no owner) gives way.
fn pr_links(app: &App, rep: &mut Report) -> Result<()> {
    let rows = app.db.q(
        "SELECT id, project, repo_path, pr_host, pr_repo, pr_num, pr_url FROM tasks WHERE pr_num IS NOT NULL \
         AND (COALESCE(pr_url, '') = '' OR COALESCE(pr_host, '') = '' OR instr(COALESCE(pr_repo, ''), '/') = 0)",
        vec![],
    )?;
    let mut cache = HashMap::new();
    let mut n = 0;
    for t in rows {
        let num = t.i("pr_num").unwrap_or(0);
        let repo = t.s("pr_repo").filter(|r| r.contains('/')).map(str::to_string);
        let found = t.s("pr_url").and_then(find_pr).map(|l| (l.host, l.repo)).or_else(|| repo.as_deref().and_then(crate::prhost::repo_of_remote));
        let found = found.or_else(|| {
            let path = t.s("repo_path").filter(|p| !p.is_empty()).map(str::to_string).or_else(|| crate::projects::project_path(app, t.s("project")).ok().flatten())?;
            let (host, theirs) = remote_of(app, &path, &mut cache)?;
            // The old repo when it names one (owner/name), else the remote's.
            Some((host, repo.clone().filter(|r| r.contains('/')).unwrap_or(theirs)))
        });
        let Some((host, repo)) = found else {
            rep.skipped.push(format!("{} PR #{num}: no link, and its project's remote isn't on GitHub or Bitbucket", rf("task", t.id())));
            continue;
        };
        let host = t.s("pr_host").filter(|h| !h.is_empty()).map(str::to_string).unwrap_or(host);
        let url = t.s("pr_url").filter(|u| !u.is_empty()).map(str::to_string).or_else(|| pr_page(&host, &repo, num));
        app.db.x(
            "UPDATE tasks SET pr_host = ?, pr_repo = ?, pr_url = COALESCE(NULLIF(pr_url, ''), ?) WHERE id = ?",
            vec![json!(host), json!(repo), json!(url), json!(t.id())],
        )?;
        n += 1;
    }
    if n > 0 {
        rep.copied.push(("PR links rebuilt".into(), n));
    }
    Ok(())
}

/// The project a task was in (on the board, already imported), with its PR.
fn task_of(app: &App, id: Option<i64>) -> Result<Option<Row>> {
    match id {
        Some(id) => app.db.q1("SELECT id, project, pr_host, pr_repo, pr_num, status, finished_at, updated_at FROM tasks WHERE id = ?", vec![json!(id)]),
        None => Ok(None),
    }
}

/// An old list column as the board's JSON list of strings.
fn str_words(v: &Value) -> Vec<String> {
    words(v).iter().filter_map(text).collect()
}

/// An old JSON column (kept as text) as JSON text; plain text is kept as `{"text": …}`.
fn json_text(v: &Value) -> Value {
    match v {
        Value::Null => Value::Null,
        Value::String(s) => match serde_json::from_str::<Value>(s.trim()) {
            Ok(j) => json!(jdumps(&j)),
            Err(_) => json!(jdumps(&json!({"text": s}))),
        },
        other => json!(jdumps(other)),
    }
}

/// An ask's old state, as the board says it.
fn ask_state(raw: Option<&str>) -> Option<&'static str> {
    Some(match raw?.to_lowercase().replace(['-', ' '], "_").as_str() {
        "open" | "pending" | "asked" | "waiting" | "requested" => "open",
        "answered" | "reviewed" | "approved" | "done" | "changes" | "commented" => "answered",
        "swapped" | "replaced" | "timed_out" | "timeout" | "swapped_off" => "swapped",
        "came_back" | "back" => "came_back",
        "dropped" | "removed" | "unassigned" => "dropped",
        "closed" | "merged" | "declined" | "canceled" | "cancelled" => "closed",
        _ => return None,
    })
}

/// A review's old answer: approved, changes or commented.
fn ask_answer(raw: Option<&str>) -> Option<&'static str> {
    let r = raw?.to_lowercase();
    Some(if r.starts_with("approv") {
        "approved"
    } else if r.contains("change") || r.contains("needs_work") || r.contains("needs work") || r == "rejected" {
        "changes"
    } else if r.starts_with("comment") {
        "commented"
    } else {
        return None;
    })
}

/// Why an ask was made, in the board's words.
fn ask_why(raw: Option<&str>, replaces: bool) -> &'static str {
    match raw.map(|r| r.to_lowercase().replace(['-', ' '], "_")).as_deref() {
        Some("ask" | "manual" | "named" | "agent") => "ask",
        Some("replace" | "replaced") => "replace",
        Some("swap" | "swapped" | "timeout" | "timed_out" | "nudge") => "swap",
        Some("fill_in" | "fillin" | "fill") => "fill_in",
        Some("stage") => "stage",
        Some("pick" | "auto" | "picker" | "picked") => "pick",
        _ if replaces => "swap",
        _ => "pick",
    }
}

/// The roster (`reviewers`), each ask of one of them (`review_asks`), and their bots' runs. People
/// the old board kept twice on one project fold into one reviewer, as `tb reviewers` would.
fn reviewers(app: &App, c: &Connection, old: &[String], rep: &mut Report) -> Result<()> {
    // Old reviewer id → the board's reviewer row id.
    let mut ids: HashMap<String, i64> = HashMap::new();
    let asks = Old::read(c, old, "review_asks", &["reviewer_asks", "asks"])?;
    if let Some(rv) = Old::read(c, old, "reviewers", &["reviewer_roster", "roster"])? {
        let before = app.db.count("SELECT COUNT(*) FROM reviewers", vec![])?;
        let mut folded = 0;
        for r in &rv.rows {
            let old_id = rv.text(r, &["id"]);
            let host_user = rv.text(r, &["host_user", "account_id", "uuid", "github", "login", "username", "user", "bitbucket", "account"]);
            let emails = str_words(rv.get(r, &["emails", "email"]));
            let Some(name) = rv.text(r, &["name", "display_name", "display", "person"]).or_else(|| host_user.clone()).or_else(|| emails.first().cloned()) else {
                rep.skipped.push(format!("{} {}: no name", rv.table, old_id.unwrap_or_default()));
                continue;
            };
            // Its project, else the project of a task it was asked on.
            let mut project = rv.text(r, &["project", "repo", "repo_name"]);
            if project.is_none() {
                if let (Some(a), Some(oid)) = (&asks, &old_id) {
                    let task = a.rows.iter().find(|x| a.text(x, &["reviewer_id", "reviewer"]).as_deref() == Some(oid.as_str())).and_then(|x| a.id(x, &["task_id", "task"]));
                    project = task_of(app, task)?.and_then(|t| t.s("project").map(str::to_string));
                }
            }
            let Some(project) = project else {
                rep.skipped.push(format!("{} {name}: no project", rv.table));
                continue;
            };
            let person = crate::reviewers::Person {
                name: name.clone(),
                host_user: host_user.filter(|u| u != &name),
                emails,
                aliases: str_words(rv.get(r, &["aliases", "alias", "other_names", "names"])),
                slack: rv.text(r, &["slack", "slack_id", "slack_user"]),
                source: "import".into(),
                commits: rv.id(r, &["commits"]),
            };
            let had = app.db.count("SELECT COUNT(*) FROM reviewers", vec![])?;
            let row = crate::reviewers::fold(app, &project, &person)?;
            if app.db.count("SELECT COUNT(*) FROM reviewers", vec![])? == had {
                folded += 1;
            }
            let mut f: Vec<(&str, Value)> = vec![];
            let removed = rv.get(r, &["removed_at"]).clone();
            if !removed.is_null() || rv.truthy(r, &["removed", "never", "never_assign", "excluded", "blocked"]) {
                let at = if removed.is_null() || removed.is_number() { rv.get(r, &["updated_at", "updated"]).clone() } else { removed };
                f.push(("removed_at", if at.is_null() { json!(now_iso()) } else { at }));
                f.push(("removed_why", rv.get(r, &["removed_why", "removed_reason", "why", "reason"]).clone()));
            }
            if rv.truthy(r, &["pinned", "pin"]) {
                f.push(("pinned", json!(1)));
            }
            let auto = rv.get(r, &["automation", "auto", "level", "automation_level"]);
            let level = match auto {
                Value::Number(n) => n.as_f64(),
                Value::String(s) => crate::reviewers::LEVELS.iter().find(|(n, _)| n.eq_ignore_ascii_case(s.trim())).map(|(_, v)| *v).or_else(|| s.trim().parse().ok()),
                _ => None,
            };
            if let Some(v) = level.filter(|v| (0.1..=10.0).contains(v)) {
                f.push(("automation", json!(v)));
            }
            if let Some(h) = match rv.get(r, &["bot_every_h", "bot_hours", "bot_every_hours", "bot_every"]) {
                Value::Number(n) => n.as_f64(),
                Value::String(s) => s.trim().trim_end_matches('h').parse().ok(),
                _ => None,
            } {
                f.push(("bot_every_h", json!(h)));
                f.push(("bot_mark", rv.get(r, &["bot_mark", "bot_marker", "mark"]).clone()));
            }
            for (k, alts) in [("created_at", &["created_at", "created", "added_at"][..]), ("updated_at", &["updated_at", "updated"][..])] {
                let v = rv.get(r, alts);
                if !v.is_null() {
                    f.push((k, v.clone()));
                }
            }
            if !f.is_empty() {
                app.db.update("reviewers", &json!(row.id()), f)?;
            }
            if let Some(oid) = old_id {
                ids.insert(oid.to_lowercase(), row.id());
            }
            ids.entry(name.to_lowercase()).or_insert(row.id());
        }
        rep.copied.push(("reviewers".into(), (app.db.count("SELECT COUNT(*) FROM reviewers", vec![])? - before) as usize));
        if folded > 0 {
            rep.skipped.push(format!("{}: {folded} rows were the same person as another on the project, folded into one reviewer", rv.table));
        }
    }
    if let Some(a) = asks {
        let mut n = 0;
        let mut stand_ins: Vec<(i64, i64)> = vec![];
        for r in &a.rows {
            let task = task_of(app, a.id(r, &["task_id", "task"]))?;
            let who_raw = a.get(r, &["reviewer_id", "reviewer"]);
            let mut reviewer = text(who_raw).and_then(|k| ids.get(&k.to_lowercase()).copied());
            let project = a.text(r, &["project"]).or_else(|| task.as_ref().and_then(|t| t.s("project").map(str::to_string)));
            let host_user = a.text(r, &["host_user", "account_id", "uuid", "user", "login", "username", "github"]);
            if reviewer.is_none() {
                if let (Some(p), Some(k)) = (&project, host_user.clone().or_else(|| text(who_raw))) {
                    reviewer = crate::reviewers::find(app, p, &k)?.map(|x| x.id());
                }
            }
            let rrow = match reviewer {
                Some(id) => Some(crate::reviewers::get(app, id)?),
                None => None,
            };
            let name = a.text(r, &["name", "reviewer_name", "display_name"]).or_else(|| rrow.as_ref().map(|x| x.st("name"))).or_else(|| host_user.clone());
            let host_user = host_user.or_else(|| rrow.as_ref().and_then(|x| x.s("host_user").map(str::to_string)));
            if task.is_none() && project.is_none() {
                rep.skipped.push(format!("{} {}: its task isn't known", a.table, a.text(r, &["id"]).unwrap_or_default()));
                continue;
            }
            if name.is_none() && host_user.is_none() {
                rep.skipped.push(format!("{} {}: who was asked isn't known", a.table, a.text(r, &["id"]).unwrap_or_default()));
                continue;
            }
            let replaces = a.id(r, &["replaces", "replaced", "stands_in_for", "for_ask"]);
            let answered_at = a.get(r, &["answered_at", "reviewed_at"]).clone();
            let closed_at = a.get(r, &["closed_at", "swapped_at", "ended_at", "dropped_at"]).clone();
            let answer = ask_answer(a.text(r, &["answer", "verdict", "review", "result"]).as_deref());
            let state = ask_state(a.text(r, &["state", "status"]).as_deref()).unwrap_or(if !answered_at.is_null() || answer.is_some() {
                "answered"
            } else if a.truthy(r, &["swapped"]) {
                "swapped"
            } else if !closed_at.is_null() {
                "closed"
            } else {
                "open"
            });
            // An ask still open on a task that's finished: its PR is done with.
            let done = task.as_ref().map(|t| t.s("status") == Some("done")).unwrap_or(false);
            let (state, closed_at) = match state {
                "open" if done => ("closed", task.as_ref().map(|t| t.v("finished_at")).filter(|v| !v.is_null()).unwrap_or_else(|| json!(now_iso()))),
                s => (s, closed_at),
            };
            let pr = |col: &str, alts: &[&str]| -> Value {
                let mine = a.get(r, alts);
                if !mine.is_null() {
                    return mine.clone();
                }
                task.as_ref().map(|t| t.v(col)).unwrap_or(Value::Null)
            };
            let new_id = app.db.x(
                "INSERT OR IGNORE INTO review_asks(id, task_id, project, pr_host, pr_repo, pr_num, reviewer_id, host_user, name, why, asked_by, replaces, \
                 state, asked_at, answered_at, closed_at, answer, work_mins, filled) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                vec![
                    json!(a.id(r, &["id"])),
                    json!(task.as_ref().map(|t| t.id())),
                    json!(project),
                    pr("pr_host", &["pr_host", "host"]),
                    pr("pr_repo", &["pr_repo", "repo"]),
                    pr("pr_num", &["pr_num", "pr", "pr_number"]),
                    json!(reviewer),
                    json!(host_user),
                    json!(name),
                    json!(ask_why(a.text(r, &["why", "reason", "kind"]).as_deref(), replaces.is_some())),
                    json!(a.text(r, &["asked_by", "by", "who"]).unwrap_or_else(|| "import".into())),
                    json!(replaces),
                    json!(state),
                    a.get(r, &["asked_at", "at", "created_at", "created"]).clone(),
                    answered_at,
                    closed_at,
                    json!(answer),
                    a.get(r, &["work_mins", "mins", "minutes"]).clone(),
                    json!(a.truthy(r, &["filled", "filled_in"]) as i64),
                ],
            )?;
            if !wrote(app)? {
                rep.skipped.push(format!("{} {}: an ask with that id came first", a.table, a.text(r, &["id"]).unwrap_or_default()));
                continue;
            }
            n += 1;
            if let Some(by) = a.id(r, &["replaced_by", "stand_in"]) {
                stand_ins.push((new_id, by));
            }
        }
        // An ask that names the one standing in for it (which may come later).
        for (id, by) in stand_ins {
            app.db.x("UPDATE review_asks SET replaces = COALESCE(replaces, ?) WHERE id = ?", vec![json!(id), json!(by)])?;
        }
        rep.copied.push(("review_asks".into(), n));
    }
    if let Some(b) = Old::read(c, old, "reviewer_bot_runs", &["bot_runs"])? {
        let mut n = 0;
        for r in &b.rows {
            let rid = b.text(r, &["reviewer_id", "reviewer"]).and_then(|k| ids.get(&k.to_lowercase()).copied());
            let (Some(rid), Some(at), Some(rf)) = (rid, b.text(r, &["at", "seen_at", "created_at"]), b.text(r, &["ref", "comment", "comment_id", "url"])) else {
                rep.skipped.push(format!("{} {}: its reviewer, time or comment isn't known", b.table, b.text(r, &["id"]).unwrap_or_default()));
                continue;
            };
            app.db.x("INSERT OR IGNORE INTO reviewer_bot_runs(reviewer_id, at, ref) VALUES(?, ?, ?)", vec![json!(rid), json!(at), json!(rf)])?;
            if wrote(app)? {
                n += 1;
            }
        }
        rep.copied.push(("reviewer_bot_runs".into(), n));
    }
    Ok(())
}

/// A break's old verdict: the old board said yours / not yours / unsure.
fn break_verdict(raw: Option<&str>) -> Option<&'static str> {
    Some(match raw?.to_lowercase().replace(['-', ' '], "_").as_str() {
        "ours" | "yours" | "mine" | "owner" | "owners" => "ours",
        "not_ours" | "not_yours" | "not_mine" | "theirs" | "others" => "not_ours",
        "unsure" | "unknown" | "maybe" => "unsure",
        _ => return None,
    })
}

/// The old board's master breaks (`M<n>`), numbers kept. One still open on a project the board
/// doesn't watch (`[master.projects]`) comes over closed: nothing would ever close it.
fn breaks(app: &App, c: &Connection, old: &[String], rep: &mut Report) -> Result<()> {
    let Some(b) = Old::read(c, old, "master_breaks", &["breaks"])? else { return Ok(()) };
    let mut n = 0;
    for r in &b.rows {
        let id = b.id(r, &["id", "num", "number"]);
        let label = id.map(crate::breaks::bref).unwrap_or_else(|| "a break".into());
        let Some(project) = b.text(r, &["project"]) else {
            rep.skipped.push(format!("{} {label}: no project", b.table));
            continue;
        };
        let closed_at = b.get(r, &["closed_at", "resolved_at", "fixed_at", "green_at"]).clone();
        let mut state = match b.text(r, &["state", "status"]).map(|s| s.to_lowercase()).as_deref() {
            Some("open" | "red" | "active" | "broken" | "investigating" | "failing") => "open",
            Some(_) => "closed",
            None if closed_at.is_null() => "open",
            None => "closed",
        };
        let watched = app.cfg.master.projects.contains_key(&project);
        if state == "open" && !watched {
            state = "closed";
            rep.skipped.push(format!("{label}: open on the old board, but [master.projects] doesn't watch {project}, so it comes over closed"));
        }
        let head = b.get(r, &["head", "sha", "head_sha", "red_sha", "commit"]).clone();
        let last = b.get(r, &["last_head", "latest_sha", "last_sha"]).clone();
        let checks = b.get(r, &["checks", "failed_checks", "failing", "failing_checks"]);
        let checks = if checks.is_null() { Value::Null } else { json!(jdumps(&json!(str_words(checks)))) };
        app.db.x(
            "INSERT OR IGNORE INTO breaks(id, project, host, repo, branch, state, head, last_head, green_head, fixed_head, checks, evidence, suspects, \
             verdict, verdict_by, verdict_why, verdict_at, task_id, escalated_at, opened_at, closed_at, checked_at) \
             VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            vec![
                json!(id),
                json!(project),
                b.get(r, &["host", "pr_host"]).clone(),
                b.get(r, &["repo", "pr_repo"]).clone(),
                b.get(r, &["branch", "default_branch"]).clone(),
                json!(state),
                head.clone(),
                if last.is_null() { head } else { last },
                b.get(r, &["green_head", "green_sha", "last_green"]).clone(),
                b.get(r, &["fixed_head", "fixed_sha", "fixed_by"]).clone(),
                checks,
                json_text(b.get(r, &["evidence", "ci", "ci_evidence"])),
                json_text(b.get(r, &["suspects", "commits"])),
                json!(break_verdict(b.text(r, &["verdict", "fault", "decision", "owner"]).as_deref())),
                b.get(r, &["verdict_by", "decided_by"]).clone(),
                b.get(r, &["verdict_why", "reason", "why"]).clone(),
                b.get(r, &["verdict_at", "decided_at"]).clone(),
                json!(b.id(r, &["task_id", "fix_task", "task"])),
                b.get(r, &["escalated_at"]).clone(),
                b.get(r, &["opened_at", "at", "created_at", "started_at"]).clone(),
                if closed_at.is_null() && state == "closed" { json!(now_iso()) } else { closed_at },
                b.get(r, &["checked_at", "updated_at"]).clone(),
            ],
        )?;
        if wrote(app)? {
            n += 1;
        } else {
            rep.skipped.push(format!("{} {label}: a break with that number came first", b.table));
        }
    }
    rep.copied.push(("breaks".into(), n));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_match_by_name_case_or_older_name() {
        let board: Vec<String> = ["id", "title", "claude_session_id", "goal_id", "locks"].iter().map(|s| s.to_string()).collect();
        let old: Vec<String> = ["ID", "title", "conversation_id", "goal", "extra"].iter().map(|s| s.to_string()).collect();
        let m = column_map(&board, &old);
        assert_eq!(m, vec![("id".into(), 0), ("title".into(), 1), ("claude_session_id".into(), 2), ("goal_id".into(), 3)]);
    }

    #[test]
    fn running_state_settings_stay_behind() {
        for k in ["alerts", "bridge_pid", "bridge:cursor", "dispatch_seen:T7", "usage_guard_handled:x", "review_round:T7", "nudge_sent:T3", "last_poll"] {
            assert!(runtime_setting(k), "{k}");
        }
        for k in ["work_hours", "project_pr_flow", "qa_on", "limits", "pr_flow:web"] {
            assert!(!runtime_setting(k), "{k}");
        }
    }

    #[test]
    fn old_names_become_device_names() {
        assert_eq!(slug("iPhone 15 Pro"), Some("iphone-15-pro".into()));
        assert_eq!(slug("  emu:5554 "), Some("emu:5554".into()));
        assert_eq!(slug("--"), None);
        assert_eq!(pr_page("bitbucket", "w/r", 3).as_deref(), Some("https://bitbucket.org/w/r/pull-requests/3"));
        assert_eq!(words(&json!("a, b c")), vec![json!("a"), json!("b"), json!("c")]);
        assert_eq!(words(&json!(r#"["x"]"#)), vec![json!("x")]);
    }
}
