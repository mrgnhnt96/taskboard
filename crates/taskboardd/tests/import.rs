//! `taskboardd import`: an old board's tasks.db (an older schema, with tables this board has no
//! place for) carried into a fresh board, numbers kept.

use rusqlite::Connection;
use serde_json::{json, Value};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{api, import};

fn get(app: &App, path: &str) -> Value {
    api::dispatch(app, "GET", path, &Default::default(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
}

/// The old board's file: its own (older) columns, a WAL left open, and its extra tables.
fn old_board(path: &std::path::Path) -> Connection {
    let c = Connection::open(path).unwrap();
    c.pragma_update(None, "journal_mode", "WAL").unwrap();
    c.execute_batch(
        r#"
        CREATE TABLE tasks(id INTEGER PRIMARY KEY, title TEXT, detail TEXT, project TEXT, repo_path TEXT, status TEXT,
          goal_id INT, position REAL, session_id TEXT, conversation_id TEXT, pr_url TEXT, pr_num INT, pr_phase TEXT,
          created_at TEXT, updated_at TEXT, started_at TEXT, finished_at TEXT, summary TEXT, extra_old_column TEXT);
        CREATE TABLE events(id INTEGER PRIMARY KEY, task_id INT, at TEXT, who TEXT, kind TEXT, text TEXT, data TEXT);
        CREATE TABLE goals(id INTEGER PRIMARY KEY, name TEXT, outcome TEXT, project TEXT, created_at TEXT, archived INT DEFAULT 0);
        CREATE TABLE goal_notes(id INTEGER PRIMARY KEY, goal_id INT, kind TEXT, text TEXT, source TEXT, at TEXT);
        CREATE TABLE backlog(id INTEGER PRIMARY KEY, goal INT, project TEXT, kind TEXT, title TEXT, state TEXT, source TEXT, created_at TEXT);
        CREATE TABLE sessions(id TEXT PRIMARY KEY, name TEXT, project TEXT, status TEXT, claude_session_id TEXT, status_at TEXT);
        CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT);
        CREATE TABLE jobs(id INTEGER PRIMARY KEY, kind TEXT, state TEXT);
        CREATE TABLE reviewers(id INTEGER PRIMARY KEY, name TEXT, github TEXT);
        CREATE TABLE review_asks(id INTEGER PRIMARY KEY, task_id INT, reviewer_id INT, at TEXT);
        CREATE TABLE devices(id TEXT PRIMARY KEY, name TEXT);

        INSERT INTO goals VALUES (3, 'Checkout v2', 'Pay with one click', 'web', '2026-09-01T10:00:00Z', 0);
        INSERT INTO goal_notes VALUES (1, 3, 'decision', 'Stripe, not Adyen', 'you', '2026-09-01T11:00:00Z');
        INSERT INTO tasks VALUES (7, 'Card form', 'Build it', 'web', '/tmp/web', 'needs', 3, 1, 'midna-1', 'conv-7',
          'https://github.com/acme/web/pull/41', 41, 'review', '2026-09-02T09:00:00Z', '2026-09-03T09:00:00Z', '2026-09-02T09:05:00Z', NULL, NULL, 'x');
        INSERT INTO tasks VALUES (12, 'Receipts', 'Email them', 'web', '/tmp/web', 'done', 3, 2, NULL, 'conv-12',
          NULL, NULL, NULL, '2026-09-02T09:00:00Z', '2026-09-04T09:00:00Z', '2026-09-03T09:00:00Z', '2026-09-04T09:00:00Z', 'Sent', NULL);
        INSERT INTO events VALUES (1, 7, '2026-09-02T10:00:00Z', 'tb', 'checkpoint', 'Form half done', '{"next":["validation"]}');
        INSERT INTO events VALUES (2, 7, '2026-09-02T11:00:00Z', 'tb', 'note', 'Uses the shared input', NULL);
        INSERT INTO backlog VALUES (5, 3, 'web', 'bug', 'Total rounds wrong', 'open', 'terminal', '2026-09-02T12:00:00Z');
        INSERT INTO sessions VALUES ('midna-1', 'web 1', 'web', 'needs', 'conv-7', '2026-09-03T09:00:00Z');
        INSERT INTO settings VALUES ('work_hours', '{"on":true,"start":"07:00","end":"15:00","days":["mon","tue"],"alert_every_mins":5}');
        INSERT INTO settings VALUES ('alerts', '[{"id":"old","text":"stale"}]');
        INSERT INTO jobs VALUES (1, 'agent', 'pending');
        INSERT INTO reviewers VALUES (1, 'Ana', 'ana-gh');
        INSERT INTO review_asks VALUES (1, 7, 1, '2026-09-03T09:00:00Z');
        INSERT INTO devices VALUES ('pixel', 'Pixel 9');
        "#,
    )
    .unwrap();
    c
}

#[test]
fn an_old_board_comes_over_with_its_numbers() {
    let old_dir = tempfile::tempdir().unwrap();
    let old_path = old_dir.path().join("tasks.db");
    // Left open, so some of it is still only in the -wal file.
    let _old = old_board(&old_path);
    let before = std::fs::read(&old_path).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let rep = import::import(&app, &old_path).unwrap();
    assert_eq!(std::fs::read(&old_path).unwrap(), before, "the old file is only read");

    // Tasks, goals and issues keep their numbers, with their log and conversations.
    let t = get(&app, "tasks/T7");
    assert_eq!(t["title"], "Card form");
    assert_eq!(t["goal"]["ref"], "G3");
    let row = app.db.q1("SELECT * FROM tasks WHERE id = 7", vec![]).unwrap().unwrap();
    assert_eq!(row.s("claude_session_id"), Some("conv-7"), "the conversation id comes over from its older name");
    assert_eq!(row.s("pr_url"), Some("https://github.com/acme/web/pull/41"));
    assert_eq!(row.s("pr_phase"), Some("review"));
    let log = app.db.q("SELECT kind, text FROM events WHERE task_id = 7 ORDER BY id", vec![]).unwrap();
    assert_eq!(log.iter().map(|e| e.st("kind")).collect::<Vec<_>>(), vec!["checkpoint", "note"]);
    assert_eq!(get(&app, "goals/G3")["name"], "Checkout v2");
    let issue = get(&app, "backlog/B5");
    let issue = issue.get("issue").filter(|i| i.is_object()).unwrap_or(&issue);
    assert_eq!(issue["title"], "Total rounds wrong");
    assert_eq!(app.db.val("SELECT goal_id FROM issues WHERE id = 5", vec![]).unwrap(), json!(3), "`goal` is goal_id");
    assert_eq!(app.db.val("SELECT status_at FROM sessions WHERE id = 'midna-1'", vec![]).unwrap(), json!("2026-09-03T09:00:00Z"));

    // The owner's settings come over; the old board's running state and its jobs don't.
    assert_eq!(taskboardd::hours::get(&app).start, "07:00");
    assert!(taskboardd::dispatch::alerts(&app).is_empty());
    assert_eq!(app.db.count("SELECT COUNT(*) FROM jobs", vec![]).unwrap(), 0);

    // Tables the board has no place for are kept whole.
    let kept: Value = serde_json::from_str(&app.db.get_setting("import.reviewers").unwrap().unwrap()).unwrap();
    assert_eq!(kept["rows"][0]["github"], "ana-gh");
    assert!(rep.kept.iter().any(|(t, n)| t == "review_asks" && *n == 1));
    assert!(rep.kept.iter().any(|(t, _)| t == "devices"));
    assert!(rep.lines().iter().any(|l| l.contains("T12") && l.contains("G3") && l.contains("B5")));

    // New work carries on after the old numbers.
    let new = api::dispatch(&app, "POST", "tasks", &Default::default(), &json!({"title": "Next", "project": "web", "detail": "x"})).unwrap();
    assert_eq!(new["ref"], "T13");

    // A board with work in it is refused: the numbers would clash.
    let again = import::import(&app, &old_path).unwrap_err();
    assert_eq!(again.status, 409);
}

#[test]
fn a_file_that_isnt_a_board_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("other.db");
    Connection::open(&p).unwrap().execute_batch("CREATE TABLE photos(id INTEGER PRIMARY KEY)").unwrap();
    let app = App::for_tests(Config::for_tests(&dir.path().join("board")));
    assert_eq!(import::import(&app, &p).unwrap_err().status, 400);
    assert_eq!(import::import(&app, &dir.path().join("missing.db")).unwrap_err().status, 404);
}
