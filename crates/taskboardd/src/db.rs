//! SQLite storage. Rows come back as JSON objects so the board logic can read them by name.

use std::cell::{Cell, RefCell};
use std::path::Path;

use parking_lot::ReentrantMutex;
use rusqlite::types::ValueRef;
use rusqlite::{params_from_iter, Connection};
use serde_json::{Number, Value};

use crate::util::{Result, Row};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS tasks(
  id INTEGER PRIMARY KEY, title TEXT, detail TEXT,
  project TEXT, repo_path TEXT,
  priority TEXT DEFAULT 'normal',
  status TEXT,
  failed INT DEFAULT 0, lost INT DEFAULT 0,
  needs_reason TEXT,
  question TEXT, latest TEXT, summary TEXT,
  goal_id INT, position REAL,
  pickup TEXT DEFAULT 'queue', pickup_session TEXT,
  session_id TEXT, session_name TEXT,
  claude_session_id TEXT,
  auto_close INT DEFAULT 1,
  jira_key TEXT, jira_status TEXT, jira_sync INT DEFAULT 1,
  pr_host TEXT, pr_repo TEXT, pr_num INT, pr_url TEXT, pr_title TEXT,
  pr_state TEXT, pr_build TEXT, pr_review TEXT, pr_phase TEXT, pr_flow TEXT,
  from_issue_id INT,
  context TEXT DEFAULT '{}',
  meta TEXT DEFAULT '[]',
  ctx_version INT DEFAULT 1,
  created_at TEXT, updated_at TEXT, started_at TEXT, finished_at TEXT,
  start_job INT,
  answered_at TEXT,
  waits_for TEXT,
  start_tries INT DEFAULT 0, retry_at TEXT
);
CREATE INDEX IF NOT EXISTS tasks_status ON tasks(status);
CREATE INDEX IF NOT EXISTS tasks_session ON tasks(session_id);
CREATE INDEX IF NOT EXISTS tasks_goal ON tasks(goal_id);

CREATE TABLE IF NOT EXISTS events(
  id INTEGER PRIMARY KEY, task_id INT, at TEXT, who TEXT, kind TEXT, text TEXT, data TEXT);
CREATE INDEX IF NOT EXISTS events_task ON events(task_id);

CREATE TABLE IF NOT EXISTS goals(
  id INTEGER PRIMARY KEY, name TEXT, outcome TEXT, tldr TEXT, project TEXT, repo_path TEXT,
  epic_key TEXT, epic_status TEXT, product TEXT,
  run_in_order INT DEFAULT 1, max_terminals INT DEFAULT 2, auto_close INT DEFAULT 1,
  paused INT DEFAULT 0, deprioritized INT DEFAULT 0, hours_until TEXT,
  created_at TEXT, updated_at TEXT, archived INT DEFAULT 0);

CREATE TABLE IF NOT EXISTS goal_notes(
  id INTEGER PRIMARY KEY, goal_id INT, kind TEXT, text TEXT, source TEXT, pinned INT DEFAULT 0, at TEXT);
CREATE INDEX IF NOT EXISTS goal_notes_goal ON goal_notes(goal_id);

CREATE TABLE IF NOT EXISTS issues(
  id INTEGER PRIMARY KEY, goal_id INT, project TEXT, kind TEXT, title TEXT, detail TEXT,
  said TEXT, how TEXT, source TEXT,
  found_by_task INT, found_by_session TEXT, found_by_name TEXT,
  state TEXT DEFAULT 'open',
  task_id INT, jira_key TEXT, snapshot TEXT DEFAULT '{}', created_at TEXT, updated_at TEXT);
CREATE INDEX IF NOT EXISTS issues_goal ON issues(goal_id);

CREATE TABLE IF NOT EXISTS issue_events(
  id INTEGER PRIMARY KEY, issue_id INT, at TEXT, who TEXT, kind TEXT, text TEXT, snapshot TEXT);
CREATE INDEX IF NOT EXISTS issue_events_issue ON issue_events(issue_id);

CREATE TABLE IF NOT EXISTS sessions(
  id TEXT PRIMARY KEY,
  name TEXT, project TEXT, project_path TEXT, agent TEXT, status TEXT,
  prompt_waiting INT, last_activity TEXT, claude_session_id TEXT,
  source TEXT, seen_at TEXT, gone_at TEXT,
  missed INT DEFAULT 0,
  ctx_task INT, ctx_version INT,
  last_task INT,
  board_prompt INT,
  branch TEXT, dirty INT, status_at TEXT,
  api_error TEXT, api_error_kind TEXT, api_error_at TEXT, api_error_tries INT DEFAULT 0
);

CREATE TABLE IF NOT EXISTS jobs(
  id INTEGER PRIMARY KEY, kind TEXT, args TEXT, state TEXT DEFAULT 'pending', result TEXT,
  task_id INT, created_at TEXT, updated_at TEXT, attempts INT DEFAULT 0,
  purpose TEXT,
  target TEXT
);
CREATE INDEX IF NOT EXISTS jobs_state ON jobs(state);

CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT);

CREATE TABLE IF NOT EXISTS session_events(
  id INTEGER PRIMARY KEY, session_id TEXT, at TEXT, kind TEXT, text TEXT, data TEXT);
CREATE INDEX IF NOT EXISTS session_events_session ON session_events(session_id, id);

CREATE TABLE IF NOT EXISTS attachments(
  id INTEGER PRIMARY KEY, task_id INT, goal_id INT, kind TEXT, title TEXT, url TEXT,
  added_by TEXT, at TEXT, removed_at TEXT);
CREATE INDEX IF NOT EXISTS attachments_task ON attachments(task_id);
CREATE INDEX IF NOT EXISTS attachments_goal ON attachments(goal_id);

CREATE TABLE IF NOT EXISTS task_terminals(
  task_id INT NOT NULL, session_id TEXT NOT NULL, why TEXT, at TEXT, PRIMARY KEY(task_id, session_id));

CREATE TABLE IF NOT EXISTS goal_waves(
  goal_id INT NOT NULL, wave INT NOT NULL, name TEXT, stop_after INT DEFAULT 0, released_at TEXT,
  PRIMARY KEY(goal_id, wave));

CREATE TABLE IF NOT EXISTS task_goals(
  task_id INT NOT NULL, goal_id INT NOT NULL, at TEXT, PRIMARY KEY(task_id, goal_id));
CREATE INDEX IF NOT EXISTS task_goals_goal ON task_goals(goal_id);

-- QA testers' Jira comments on board tickets (`qa.rs`), each read once and turned into a task, a flag or nothing.
CREATE TABLE IF NOT EXISTS qa_comments(
  id INTEGER PRIMARY KEY, jira_key TEXT NOT NULL, comment_id TEXT UNIQUE NOT NULL, source_task_id INT,
  author TEXT, verdict TEXT, pr INT, title TEXT, ask TEXT, text TEXT, task_id INT, tries INT DEFAULT 0, error TEXT,
  decided_at TEXT, handled_at TEXT, handled_by TEXT, created_at TEXT, updated_at TEXT);

CREATE TABLE IF NOT EXISTS task_states(
  id INTEGER PRIMARY KEY, task_id INT, at TEXT, status TEXT, needs_reason TEXT, failed INT, project TEXT);
CREATE INDEX IF NOT EXISTS task_states_at ON task_states(at);
CREATE INDEX IF NOT EXISTS task_states_task ON task_states(task_id, at);

CREATE TABLE IF NOT EXISTS day_stats(
  date TEXT NOT NULL, project TEXT NOT NULL, data TEXT, at TEXT, PRIMARY KEY(date, project));

CREATE INDEX IF NOT EXISTS events_at ON events(at);

-- Every change of a task's status, for the Days page's timeline (`days.rs`). A trigger, so no
-- path that moves a task can forget it. Every status change goes through `board::update_task`,
-- which stamps `updated_at`.
CREATE TRIGGER IF NOT EXISTS task_states_update AFTER UPDATE OF status, needs_reason ON tasks
  WHEN NEW.status IS NOT OLD.status OR (NEW.status = 'needs' AND NEW.needs_reason IS NOT OLD.needs_reason)
  BEGIN INSERT INTO task_states(task_id, at, status, needs_reason, failed, project)
    VALUES(NEW.id, COALESCE(NEW.updated_at, strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
      NEW.status, NEW.needs_reason, NEW.failed, NEW.project); END;
CREATE TRIGGER IF NOT EXISTS task_states_insert AFTER INSERT ON tasks
  BEGIN INSERT INTO task_states(task_id, at, status, needs_reason, failed, project)
    VALUES(NEW.id, COALESCE(NEW.created_at, strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
      NEW.status, NEW.needs_reason, NEW.failed, NEW.project); END;

CREATE TRIGGER IF NOT EXISTS sessions_status_at AFTER UPDATE OF status ON sessions
  WHEN NEW.status IS NOT OLD.status
  BEGIN UPDATE sessions SET status_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = NEW.id; END;
CREATE TRIGGER IF NOT EXISTS sessions_status_at_new AFTER INSERT ON sessions
  BEGIN UPDATE sessions SET status_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = NEW.id; END;
"#;

/// Tasks from before `task_states` existed get their start and their current status, so the Days
/// page can draw them.
const BACKFILL_STATES: &str = r#"
INSERT INTO task_states(task_id, at, status, needs_reason, failed, project)
  SELECT id, started_at, 'working', NULL, 0, project FROM tasks
  WHERE started_at IS NOT NULL AND status != 'working' AND id NOT IN (SELECT task_id FROM task_states);
INSERT INTO task_states(task_id, at, status, needs_reason, failed, project)
  SELECT id, COALESCE(CASE WHEN status = 'working' THEN started_at END, finished_at, updated_at, created_at),
    status, needs_reason, failed, project FROM tasks
  WHERE id NOT IN (SELECT task_id FROM task_states WHERE status = tasks.status);
"#;

/// Columns added since the first schema, which `CREATE TABLE IF NOT EXISTS` won't add to an older board.
const ADDED: &[(&str, &str, &str)] = &[
    ("sessions", "api_error", "TEXT"),
    ("sessions", "api_error_kind", "TEXT"),
    ("sessions", "api_error_at", "TEXT"),
    ("sessions", "api_error_tries", "INT DEFAULT 0"),
    ("tasks", "human_min", "INT"),
    ("issues", "area", "TEXT"),
    ("issues", "impact", "TEXT"),
    ("issues", "priority", "TEXT"),
    ("issues", "grp", "TEXT"),
    ("issues", "grp_about", "TEXT"),
    ("issues", "grouped_at", "TEXT"),
    ("tasks", "wave", "INT"),
    ("tasks", "origin", "TEXT"),
];

fn add_columns(conn: &Connection) -> rusqlite::Result<()> {
    for (table, col, ty) in ADDED {
        let have: Vec<String> = conn
            .prepare(&format!("PRAGMA table_info({table})"))?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<_>>()?;
        if !have.iter().any(|c| c == col) {
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {col} {ty}"))?;
        }
    }
    Ok(())
}

struct Inner {
    conn: RefCell<Connection>,
    depth: Cell<u32>,
}

pub struct Db {
    inner: ReentrantMutex<Inner>,
}

// The connection is only touched while the re-entrant lock is held.
unsafe impl Sync for Db {}
unsafe impl Send for Db {}

fn to_json(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::Number(i.into()),
        ValueRef::Real(f) => Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
    }
}

/// SQLite parameters from JSON values. Bools go in as 0/1.
fn param(v: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as S;
    match v {
        Value::Null => S::Null,
        Value::Bool(b) => S::Integer(*b as i64),
        Value::Number(n) => n.as_i64().map(S::Integer).unwrap_or_else(|| S::Real(n.as_f64().unwrap_or(0.0))),
        Value::String(s) => S::Text(s.clone()),
        other => S::Text(other.to_string()),
    }
}

#[macro_export]
macro_rules! p {
    () => { Vec::<serde_json::Value>::new() };
    ($($x:expr),+ $(,)?) => { vec![$(serde_json::json!($x)),+] };
}

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn memory() -> Result<Db> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Db> {
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        add_columns(&conn)?;
        conn.execute_batch(BACKFILL_STATES)?;
        Ok(Db { inner: ReentrantMutex::new(Inner { conn: RefCell::new(conn), depth: Cell::new(0) }) })
    }

    /// Runs `f` in one transaction. Nested calls join the outer transaction.
    pub fn tx<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let g = self.inner.lock();
        if g.depth.get() == 0 {
            g.conn.borrow().execute_batch("BEGIN IMMEDIATE")?;
        }
        g.depth.set(g.depth.get() + 1);
        // Leaves the transaction even when `f` panics, so a crash in one request can't keep it open forever.
        struct Level<'a> {
            inner: &'a Inner,
            done: bool,
        }
        impl Drop for Level<'_> {
            fn drop(&mut self) {
                if self.done {
                    return;
                }
                self.inner.depth.set(self.inner.depth.get() - 1);
                if self.inner.depth.get() == 0 {
                    let _ = self.inner.conn.borrow().execute_batch("ROLLBACK");
                }
            }
        }
        let mut level = Level { inner: &g, done: false };
        let out = f();
        level.done = true;
        g.depth.set(g.depth.get() - 1);
        if g.depth.get() == 0 {
            let c = g.conn.borrow();
            if out.is_ok() {
                if let Err(e) = c.execute_batch("COMMIT") {
                    let _ = c.execute_batch("ROLLBACK");
                    return Err(e.into());
                }
            } else {
                let _ = c.execute_batch("ROLLBACK");
            }
        }
        out
    }

    pub fn x(&self, sql: &str, params: Vec<Value>) -> Result<i64> {
        let g = self.inner.lock();
        let c = g.conn.borrow();
        c.execute(sql, params_from_iter(params.iter().map(param)))?;
        Ok(c.last_insert_rowid())
    }

    pub fn q(&self, sql: &str, params: Vec<Value>) -> Result<Vec<Row>> {
        let g = self.inner.lock();
        let c = g.conn.borrow();
        let mut st = c.prepare_cached(sql)?;
        let names: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
        let mut rows = st.query(params_from_iter(params.iter().map(param)))?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            let mut m = Row::new();
            for (i, n) in names.iter().enumerate() {
                m.insert(n.clone(), to_json(r.get_ref(i)?));
            }
            out.push(m);
        }
        Ok(out)
    }

    pub fn q1(&self, sql: &str, params: Vec<Value>) -> Result<Option<Row>> {
        Ok(self.q(sql, params)?.into_iter().next())
    }

    pub fn val(&self, sql: &str, params: Vec<Value>) -> Result<Value> {
        let g = self.inner.lock();
        let c = g.conn.borrow();
        let mut st = c.prepare_cached(sql)?;
        let mut rows = st.query(params_from_iter(params.iter().map(param)))?;
        Ok(match rows.next()? {
            Some(r) => to_json(r.get_ref(0)?),
            None => Value::Null,
        })
    }

    pub fn count(&self, sql: &str, params: Vec<Value>) -> Result<i64> {
        Ok(self.val(sql, params)?.as_i64().unwrap_or(0))
    }

    pub fn insert(&self, table: &str, row: Vec<(&str, Value)>) -> Result<i64> {
        let cols: Vec<&str> = row.iter().map(|(k, _)| *k).collect();
        let qs = vec!["?"; cols.len()].join(", ");
        let sql = format!("INSERT INTO {table} ({}) VALUES ({qs})", cols.join(", "));
        self.x(&sql, row.into_iter().map(|(_, v)| v).collect())
    }

    pub fn update(&self, table: &str, id: &Value, fields: Vec<(&str, Value)>) -> Result<()> {
        if fields.is_empty() {
            return Ok(());
        }
        let sets: Vec<String> = fields.iter().map(|(k, _)| format!("{k} = ?")).collect();
        let sql = format!("UPDATE {table} SET {} WHERE id = ?", sets.join(", "));
        let mut params: Vec<Value> = fields.into_iter().map(|(_, v)| v).collect();
        params.push(id.clone());
        self.x(&sql, params)?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self.val("SELECT value FROM settings WHERE key = ?", p![key])?.as_str().map(|s| s.to_string()))
    }

    pub fn set_setting(&self, key: &str, value: Option<&str>) -> Result<()> {
        self.x(
            "INSERT INTO settings(key, value) VALUES(?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            p![key, value],
        )?;
        Ok(())
    }

    pub fn tables(&self) -> Result<Vec<String>> {
        Ok(self
            .q("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name", vec![])?
            .into_iter()
            .filter_map(|r| r.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::RowExt;

    #[test]
    fn insert_query_update_in_a_transaction() {
        let db = Db::memory().unwrap();
        let id = db
            .tx(|| db.insert("tasks", vec![("title", "A".into()), ("status", "queued".into()), ("failed", false.into())]))
            .unwrap();
        db.update("tasks", &id.into(), vec![("title", "B".into())]).unwrap();
        let t = db.q1("SELECT * FROM tasks WHERE id = ?", p![id]).unwrap().unwrap();
        assert_eq!(t.s("title"), Some("B"));
        assert_eq!(t.i("failed"), Some(0));
        let r: Result<()> = db.tx(|| {
            db.x("UPDATE tasks SET title = 'C'", p![])?;
            Err(crate::util::ApiError::new(400, "no"))
        });
        assert!(r.is_err());
        assert_eq!(db.val("SELECT title FROM tasks", p![]).unwrap(), "B");
        db.set_setting("k", Some("v")).unwrap();
        assert_eq!(db.get_setting("k").unwrap().as_deref(), Some("v"));
    }
}

/// Field lists for inserts and updates: `fields!["status" => "done", "lost" => 0, "question" => null]`.
#[macro_export]
macro_rules! fields {
    (@acc [$($out:tt)*]) => { vec![$($out)*] };
    (@acc [$($out:tt)*] $k:expr => null $(, $($rest:tt)*)?) => {
        $crate::fields!(@acc [$($out)* ($k, serde_json::Value::Null),] $($($rest)*)?)
    };
    (@acc [$($out:tt)*] $k:expr => $v:expr $(, $($rest:tt)*)?) => {
        $crate::fields!(@acc [$($out)* ($k, serde_json::json!($v)),] $($($rest)*)?)
    };
    ($($t:tt)*) => { $crate::fields!(@acc [] $($t)*) };
}

#[cfg(test)]
mod panic_tests {
    use super::*;

    #[test]
    fn a_panic_inside_a_transaction_rolls_it_back() {
        let db = std::sync::Arc::new(Db::memory().unwrap());
        let d = db.clone();
        let r = std::thread::spawn(move || {
            let _ = d.tx(|| -> Result<()> {
                d.x("INSERT INTO settings(key, value) VALUES('a', '1')", vec![])?;
                panic!("boom");
            });
        })
        .join();
        assert!(r.is_err());
        db.tx(|| db.set_setting("b", Some("2"))).unwrap();
        assert_eq!(db.get_setting("a").unwrap(), None);
        assert_eq!(db.get_setting("b").unwrap().as_deref(), Some("2"));
    }
}
