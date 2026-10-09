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

fn git(dir: &std::path::Path, args: &[&str]) {
    let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().expect("git runs");
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
}

/// A checkout whose origin is `remote`.
fn checkout(dir: &std::path::Path, remote: &str) -> String {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    git(dir, &["remote", "add", "origin", remote]);
    dir.to_string_lossy().into_owned()
}

/// The old board's file: its own (older) columns, a WAL left open, and its extra tables. `web` and
/// `api` are checkouts whose remotes are on GitHub and Bitbucket.
fn old_board(path: &std::path::Path, web: &str, api: &str) -> Connection {
    let c = Connection::open(path).unwrap();
    c.pragma_update(None, "journal_mode", "WAL").unwrap();
    c.execute_batch(
        &r#"
        CREATE TABLE tasks(id INTEGER PRIMARY KEY, title TEXT, detail TEXT, project TEXT, repo_path TEXT, status TEXT,
          goal_id INT, position REAL, session_id TEXT, conversation_id TEXT, pr_url TEXT, pr_repo TEXT, pr_num INT, pr_phase TEXT,
          created_at TEXT, updated_at TEXT, started_at TEXT, finished_at TEXT, summary TEXT, extra_old_column TEXT,
          device_need TEXT, ships_pr INT, no_pr TEXT, pr_after INT, bits TEXT);
        CREATE TABLE events(id INTEGER PRIMARY KEY, task_id INT, at TEXT, who TEXT, kind TEXT, text TEXT, data TEXT);
        CREATE TABLE goals(id INTEGER PRIMARY KEY, name TEXT, outcome TEXT, project TEXT, created_at TEXT, archived INT DEFAULT 0, task_setup TEXT);
        CREATE TABLE goal_notes(id INTEGER PRIMARY KEY, goal_id INT, kind TEXT, text TEXT, source TEXT, at TEXT);
        CREATE TABLE task_goals(task_id INT, goal_id INT, at TEXT);
        CREATE TABLE backlog(id INTEGER PRIMARY KEY, goal INT, project TEXT, kind TEXT, title TEXT, state TEXT, source TEXT, created_at TEXT);
        CREATE TABLE sessions(id TEXT PRIMARY KEY, name TEXT, project TEXT, status TEXT, claude_session_id TEXT, status_at TEXT, jira_desk INT DEFAULT 0);
        CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT);
        CREATE TABLE jobs(id INTEGER PRIMARY KEY, kind TEXT, state TEXT);
        CREATE TABLE reviewers(id INTEGER PRIMARY KEY, name TEXT, github TEXT);
        CREATE TABLE review_asks(id INTEGER PRIMARY KEY, task_id INT, reviewer_id INT, at TEXT);
        CREATE TABLE master_breaks(id INTEGER PRIMARY KEY, project TEXT, at TEXT);
        CREATE TABLE devices(id TEXT PRIMARY KEY, name TEXT, kind TEXT, tags TEXT, focus_cmd TEXT, disabled INT DEFAULT 0);
        CREATE TABLE device_loans(id INTEGER PRIMARY KEY, device_id TEXT, task_id INT, lent_at TEXT, returned_at TEXT);
        CREATE TABLE goal_devices(goal_id INT, tag TEXT, count INT);
        CREATE TABLE bits(id INTEGER PRIMARY KEY, name TEXT, kind TEXT, project TEXT, made_at TEXT, goal_id INT);

        INSERT INTO goals VALUES (3, 'Checkout v2', 'Pay with one click', 'web', '2026-09-01T10:00:00Z', 0, 'Run make seed first');
        INSERT INTO goal_notes VALUES (1, 3, 'decision', 'Stripe, not Adyen', 'you', '2026-09-01T11:00:00Z');
        INSERT INTO tasks VALUES (7, 'Card form', 'Build it', 'web', '{web}', 'needs', 3, 1, 'midna-1', 'conv-7',
          'https://github.com/acme/web/pull/41', NULL, 41, 'review', '2026-09-02T09:00:00Z', '2026-09-03T09:00:00Z', '2026-09-02T09:05:00Z', NULL, NULL, 'x',
          'android:2 ios', 1, NULL, NULL, '["checkout.v2","card_form_local"]');
        INSERT INTO tasks VALUES (12, 'Receipts', 'Email them', 'web', '{web}', 'done', 3, 2, NULL, 'conv-12',
          NULL, 'acme/web', 50, NULL, '2026-09-02T09:00:00Z', '2026-09-04T09:00:00Z', '2026-09-03T09:00:00Z', '2026-09-04T09:00:00Z', 'Sent', NULL,
          'android:99', 0, 'Emails ship with the API change', 7, NULL);
        INSERT INTO tasks VALUES (14, 'Receipt API', 'Serve them', 'api', '{api}', 'needs', NULL, 3, NULL, NULL,
          NULL, NULL, 9, 'checks', '2026-09-02T09:00:00Z', '2026-09-04T09:00:00Z', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);
        INSERT INTO tasks VALUES (15, 'Mirror', 'Elsewhere', 'mirror', '/nowhere', 'needs', NULL, 4, NULL, NULL,
          NULL, 'mirror', 3, NULL, '2026-09-02T09:00:00Z', '2026-09-04T09:00:00Z', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);
        INSERT INTO task_goals VALUES (7, 3, '2026-09-02T09:00:00Z');
        INSERT INTO task_goals VALUES (7, 3, '2026-09-02T10:00:00Z');
        INSERT INTO events VALUES (1, 7, '2026-09-02T10:00:00Z', 'tb', 'checkpoint', 'Form half done', '{"next":["validation"]}');
        INSERT INTO events VALUES (2, 7, '2026-09-02T11:00:00Z', 'tb', 'note', 'Uses the shared input', NULL);
        INSERT INTO backlog VALUES (5, 3, 'web', 'bug', 'Total rounds wrong', 'open', 'terminal', '2026-09-02T12:00:00Z');
        INSERT INTO sessions VALUES ('midna-1', 'web 1', 'web', 'needs', 'conv-7', '2026-09-03T09:00:00Z', 0);
        INSERT INTO sessions VALUES ('midna-9', 'Jira desk', 'board', 'idle', 'conv-9', '2026-09-03T09:00:00Z', 1);
        INSERT INTO settings VALUES ('work_hours', '{"on":true,"start":"07:00","end":"15:00","days":["mon","tue"],"alert_every_mins":5}');
        INSERT INTO settings VALUES ('project_pr_flow', '{"web":"stack"}');
        INSERT INTO settings VALUES ('alerts', '[{"id":"old","text":"stale"}]');
        INSERT INTO settings VALUES ('bridge_pid', '4242');
        INSERT INTO settings VALUES ('bridge:last_sync', '2026-09-03T09:00:00Z');
        INSERT INTO settings VALUES ('dispatch_seen:T7', '1');
        INSERT INTO settings VALUES ('usage_guard_handled:2026-09-03', '1');
        INSERT INTO settings VALUES ('review_round:T7', '2');
        INSERT INTO settings VALUES ('nudge_sent:T7', '1');
        INSERT INTO jobs VALUES (1, 'agent', 'pending');
        INSERT INTO reviewers VALUES (1, 'Ana', 'ana-gh');
        INSERT INTO review_asks VALUES (1, 7, 1, '2026-09-03T09:00:00Z');
        INSERT INTO master_breaks VALUES (1, 'web', '2026-09-03T09:00:00Z');
        INSERT INTO devices VALUES ('pixel', 'Pixel 9', 'android', 'phone', 'open -a Pixel', 0);
        INSERT INTO devices VALUES ('emu', 'emu-1', 'android', NULL, NULL, 0);
        INSERT INTO devices VALUES ('iphone', 'iPhone 15', 'ios', '["phone","usb"]', NULL, 1);
        INSERT INTO devices VALUES ('dup', 'pixel 9', 'android', NULL, NULL, 0);
        INSERT INTO device_loans VALUES (1, 'pixel', 7, '2026-09-03T09:00:00Z', NULL);
        INSERT INTO device_loans VALUES (2, 'gone-device', 7, '2026-09-03T09:00:00Z', NULL);
        INSERT INTO goal_devices VALUES (3, 'android', 1);
        INSERT INTO goal_devices VALUES (3, 'iphone', 1);
        INSERT INTO bits VALUES (2, 'checkout.v2', 'backend', 'web', NULL, 3);
        INSERT INTO bits VALUES (4, 'receipts.email', 'local', 'web', NULL, NULL);
        "#
        .replace("{web}", web)
        .replace("{api}", api),
    )
    .unwrap();
    c
}

#[test]
fn an_old_board_comes_over_with_its_numbers() {
    let old_dir = tempfile::tempdir().unwrap();
    let old_path = old_dir.path().join("tasks.db");
    let web = checkout(&old_dir.path().join("web"), "git@github.com:acme/web.git");
    let api_dir = checkout(&old_dir.path().join("api"), "https://bitbucket.org/acme/api.git");
    // Left open, so some of it is still only in the -wal file.
    let _old = old_board(&old_path, &web, &api_dir);
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

    // Per-goal setup and the per-task PR plan come over.
    assert_eq!(app.db.val("SELECT setup FROM goals WHERE id = 3", vec![]).unwrap(), json!("Run make seed first"));
    let t12 = app.db.q1("SELECT * FROM tasks WHERE id = 12", vec![]).unwrap().unwrap();
    assert_eq!((t12.i("ships_pr"), t12.s("no_pr"), t12.i("pr_after")), (Some(0), Some("Emails ship with the API change"), Some(7)));
    assert_eq!(row.i("ships_pr"), Some(1));

    // PR links are rebuilt from the link, the repo, or the project's remote, so they're watched.
    let pr = |id: i64| {
        let r = app.db.q1("SELECT pr_host, pr_repo, pr_url FROM tasks WHERE id = ?", vec![json!(id)]).unwrap().unwrap();
        (r.st("pr_host"), r.st("pr_repo"), r.st("pr_url"))
    };
    assert_eq!(pr(7), ("github".into(), "acme/web".into(), "https://github.com/acme/web/pull/41".into()));
    assert_eq!(pr(12), ("github".into(), "acme/web".into(), "https://github.com/acme/web/pull/50".into()));
    assert_eq!(pr(14), ("bitbucket".into(), "acme/api".into(), "https://bitbucket.org/acme/api/pull-requests/9".into()));
    assert_eq!(pr(15).0, "", "no remote on a known host: left as it was");
    assert!(rep.skipped.iter().any(|s| s.starts_with("T15 PR #3")));

    // Devices, their loans, and what tasks and goals ask for.
    let devs = app.db.q("SELECT name, tags, focus, off FROM devices ORDER BY name", vec![]).unwrap();
    let names: Vec<String> = devs.iter().map(|d| d.st("name")).collect();
    assert_eq!(names, vec!["emu-1", "iphone-15", "pixel-9"]);
    let pixel = &devs[2];
    assert_eq!(serde_json::from_str::<Value>(&pixel.st("tags")).unwrap(), json!(["phone", "android"]));
    assert_eq!(pixel.s("focus"), Some("open -a Pixel"));
    assert_eq!(devs[1].i("off"), Some(1), "a disabled device comes over off");
    assert!(rep.skipped.iter().any(|s| s.contains("devices pixel-9")), "two devices with one name: the second is listed");
    assert_eq!(taskboardd::devices::lent(&app, 7).unwrap(), vec!["pixel-9"]);
    assert!(rep.skipped.iter().any(|s| s.starts_with("device_loans 2")));
    let needs = |owner: &str| app.db.val("SELECT needs FROM device_needs WHERE owner = ?", vec![json!(owner)]).unwrap();
    assert_eq!(serde_json::from_str::<Value>(needs("T7").as_str().unwrap()).unwrap(), json!([{"tag": "android", "n": 2}, {"tag": "ios", "n": 1}]));
    assert_eq!(serde_json::from_str::<Value>(needs("G3").as_str().unwrap()).unwrap(), json!([{"tag": "android", "n": 1}, {"tag": "iphone-15", "n": 1}]));
    assert!(needs("T12").is_null());
    assert!(rep.skipped.iter().any(|s| s.starts_with("T12 device need")), "a need the board can't take is listed");

    // Bits keep their ids and kinds; links come from the bits rows and the tasks' own lists.
    let bit = |name: &str| app.db.q1("SELECT * FROM bits WHERE name = ?", vec![json!(name)]).unwrap().unwrap();
    assert_eq!((bit("checkout.v2").id(), bit("checkout.v2").st("kind")), (2, "backend".to_string()));
    assert_eq!(bit("receipts.email").st("kind"), "local");
    assert_eq!(bit("card_form_local").st("kind"), "local", "a name only the task knew becomes a local bit");
    let t7_bits: Vec<String> = taskboardd::bits::of_task(&app, 7).unwrap().iter().map(|b| b.st("name")).collect();
    assert_eq!(t7_bits, vec!["card_form_local", "checkout.v2"]);
    assert_eq!(app.db.count("SELECT COUNT(*) FROM bit_links WHERE bit_id = 2 AND goal_id = 3", vec![]).unwrap(), 1);

    // The old Jira desk terminal is still the desk.
    assert!(taskboardd::jira_desk::is_desk(&app, Some("midna-9")).unwrap());
    assert!(!taskboardd::jira_desk::is_desk(&app, Some("midna-1")).unwrap());

    // The report counts rows written and lists the ones that weren't.
    assert!(rep.copied.iter().any(|(t, n)| t == "task_goals" && *n == 1));
    assert!(rep.skipped.iter().any(|s| s.starts_with("task_goals task_id=7")));
    assert!(rep.lines().iter().any(|l| l.starts_with("skipped: ")));

    // The owner's settings come over; the old board's running state and its jobs don't.
    assert_eq!(taskboardd::hours::get(&app).start, "07:00");
    assert_eq!(app.db.get_setting("project_pr_flow").unwrap().as_deref(), Some(r#"{"web":"stack"}"#));
    assert!(taskboardd::dispatch::alerts(&app).is_empty());
    for k in ["bridge_pid", "bridge:last_sync", "dispatch_seen:T7", "usage_guard_handled:2026-09-03", "review_round:T7", "nudge_sent:T7"] {
        assert!(app.db.get_setting(k).unwrap().is_none(), "{k} is the old board's running state");
        assert!(rep.left_settings.iter().any(|s| s == k));
    }
    assert_eq!(app.db.count("SELECT COUNT(*) FROM jobs WHERE purpose IS NOT 'jira_desk'", vec![]).unwrap(), 0);

    // Tables the board has no place for yet are kept whole (reviewers and breaks: their mapping is to come).
    let kept: Value = serde_json::from_str(&app.db.get_setting("import.reviewers").unwrap().unwrap()).unwrap();
    assert_eq!(kept["rows"][0]["github"], "ana-gh");
    assert!(rep.kept.iter().any(|(t, n)| t == "review_asks" && *n == 1));
    assert!(rep.kept.iter().any(|(t, n)| t == "master_breaks" && *n == 1));
    for t in ["devices", "device_loans", "goal_devices", "bits"] {
        assert!(!rep.kept.iter().any(|(k, _)| k == t), "{t} is mapped, not parked");
        assert!(app.db.get_setting(&format!("import.{t}")).unwrap().is_none());
    }
    assert!(rep.lines().iter().any(|l| l.contains("T15") && l.contains("G3") && l.contains("B5")));

    // New work carries on after the old numbers.
    let new = api::dispatch(&app, "POST", "tasks", &Default::default(), &json!({"title": "Next", "project": "web", "detail": "x"})).unwrap();
    assert_eq!(new["ref"], "T16");

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
