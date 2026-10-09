//! `taskboardd import <tasks.db>`: carry an old board's SQLite (the Python board this one replaces,
//! or an older taskboardd) into a fresh board, keeping every T/G/B number so refs stay valid.
//!
//! The old schema isn't fixed, so nothing here assumes it: the tables and columns are read from the
//! file itself. Each board table is filled from the old table of the same name (or a known older
//! name, like `backlog` for `issues`), column by column where the names match (or a known older
//! name does). Old tables the board has no place for (reviewers, devices, master breaks, …) are
//! kept whole in `settings` as `import.<table>`, so nothing is lost. Jobs aren't carried over (the
//! old board's pending work would run again), nor are its alerts.
//!
//! The old file is never opened in place: it's copied (with its `-wal` and `-shm`) to a scratch
//! folder first, so a running old board's database is only ever read.


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
const LEFT_SETTINGS: &[&str] = &["alerts", "tb_path", "plugin_version", "midna_projects"];

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
];

#[derive(Debug, Default)]
pub struct Report {
    /// (board table, rows) for every table filled.
    pub copied: Vec<(String, usize)>,
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
        for (t, n) in &self.kept {
            out.push(format!("{t}: {n} rows kept as settings import.{t} (the board has no table for them yet)"));
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
            n += 1;
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
                if LEFT_SETTINGS.contains(&key.as_str()) || app.db.get_setting(&key)?.is_some() {
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
    // Everything else, kept whole for whatever needs it later.
    for t in old {
        if used.contains(t) || LEFT.contains(&t.as_str()) || t.starts_with("sqlite_") {
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
}
