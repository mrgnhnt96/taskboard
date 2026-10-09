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
        CREATE TABLE reviewers(id INTEGER PRIMARY KEY, project TEXT, name TEXT, github TEXT, email TEXT, aliases TEXT,
          removed INT DEFAULT 0, removed_reason TEXT, pinned INT DEFAULT 0, auto TEXT, bot_hours REAL, bot_mark TEXT);
        CREATE TABLE review_asks(id INTEGER PRIMARY KEY, task_id INT, reviewer_id TEXT, at TEXT, status TEXT, verdict TEXT,
          replaced_by INT, reason TEXT, reviewed_at TEXT);
        CREATE TABLE master_breaks(id INTEGER PRIMARY KEY, project TEXT, status TEXT, sha TEXT, fault TEXT, reason TEXT,
          task_id INT, failed_checks TEXT, at TEXT, resolved_at TEXT);
        CREATE TABLE photos(id INTEGER PRIMARY KEY, url TEXT);
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
          NULL, 'api', 9, 'checks', '2026-09-02T09:00:00Z', '2026-09-04T09:00:00Z', NULL, NULL, NULL, NULL, 'device:pixel tag:usb', NULL, NULL, NULL, NULL);
        INSERT INTO tasks VALUES (15, 'Mirror', 'Elsewhere', 'mirror', '/nowhere', 'needs', 3, 4, NULL, NULL,
          NULL, 'mirror', 3, NULL, '2026-09-02T09:00:00Z', '2026-09-04T09:00:00Z', NULL, NULL, NULL, NULL, 'none', NULL, NULL, NULL, NULL);
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
        INSERT INTO settings VALUES ('jira_desk_session', 'midna-9');
        INSERT INTO settings VALUES ('review_log_health', '{"ok":true}');
        INSERT INTO jobs VALUES (1, 'agent', 'pending');
        INSERT INTO reviewers VALUES (1, 'web', 'Ana', 'ana-gh', 'ana@acme.dev', NULL, 0, NULL, 1, 'high', NULL, NULL);
        INSERT INTO reviewers VALUES (2, 'web', 'Ana B', NULL, 'ana@acme.dev', '["anab"]', 0, NULL, 0, NULL, NULL, NULL);
        INSERT INTO reviewers VALUES (3, 'web', 'Bo', 'bo-gh', 'bo@acme.dev', NULL, 1, 'Left the team', 0, NULL, NULL, NULL);
        INSERT INTO reviewers VALUES (4, 'web', 'Reviewbot', 'rb-gh', NULL, NULL, 0, NULL, 0, '0.5', 6, '[bot]');
        INSERT INTO reviewers VALUES (5, NULL, 'Cy', 'cy-gh', NULL, NULL, 0, NULL, 0, NULL, NULL, NULL);
        INSERT INTO reviewers VALUES (6, NULL, 'Nobody', NULL, NULL, NULL, 0, NULL, 0, NULL, NULL, NULL);
        INSERT INTO review_asks VALUES (1, 7, '1', '2026-09-03T09:00:00Z', 'pending', NULL, NULL, 'auto', NULL);
        INSERT INTO review_asks VALUES (2, 7, '3', '2026-09-03T09:00:00Z', 'replaced', NULL, 3, 'auto', NULL);
        INSERT INTO review_asks VALUES (3, 7, '4', '2026-09-03T11:00:00Z', 'reviewed', 'changes_requested', NULL, 'nudge', '2026-09-03T12:00:00Z');
        INSERT INTO review_asks VALUES (4, 12, '2', '2026-09-03T09:00:00Z', 'pending', NULL, NULL, NULL, NULL);
        INSERT INTO review_asks VALUES (5, 14, '5', '2026-09-03T09:00:00Z', 'asked', NULL, NULL, NULL, NULL);
        INSERT INTO review_asks VALUES (6, 99, 'zed', '2026-09-03T09:00:00Z', 'asked', NULL, NULL, NULL, NULL);
        INSERT INTO master_breaks VALUES (1, 'web', 'fixed', 'abc1', 'yours', 'My commit broke the build', 12, 'build, test', '2026-09-03T09:00:00Z', '2026-09-03T10:00:00Z');
        INSERT INTO master_breaks VALUES (2, 'web', 'red', 'def2', 'not_yours', NULL, NULL, '["lint"]', '2026-09-04T09:00:00Z', NULL);
        INSERT INTO master_breaks VALUES (4, 'api', 'red', 'fed4', 'unsure', 'Flaky?', NULL, NULL, '2026-09-04T09:00:00Z', NULL);
        INSERT INTO photos VALUES (1, 'x.png');
        INSERT INTO devices VALUES ('pixel', 'Pixel 9', 'android', 'phone', 'open -a Pixel', 0);
        INSERT INTO devices VALUES ('emu', 'emu-1', 'android', NULL, NULL, 0);
        INSERT INTO devices VALUES ('iphone', 'iPhone 15', 'ios', '["phone","usb"]', NULL, 1);
        INSERT INTO devices VALUES ('dup', 'pixel 9', 'android', NULL, NULL, 0);
        INSERT INTO device_loans VALUES (1, 'pixel', 7, '2026-09-03T09:00:00Z', NULL);
        INSERT INTO device_loans VALUES (2, 'gone-device', 7, '2026-09-03T09:00:00Z', NULL);
        INSERT INTO device_loans VALUES (3, 'emu', 12, '2026-09-03T09:00:00Z', NULL);
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
    let mut cfg = Config::for_tests(dir.path());
    cfg.master.projects.insert("api".into(), Default::default());
    let app = App::for_tests(cfg);
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
    assert!(taskboardd::devices::lent(&app, 12).unwrap().is_empty(), "a finished task's loan comes back");
    assert_eq!(app.db.val("SELECT released_at FROM device_loans WHERE task_id = 12", vec![]).unwrap(), json!("2026-09-04T09:00:00Z"));
    let needs = |owner: &str| app.db.val("SELECT needs FROM device_needs WHERE owner = ?", vec![json!(owner)]).unwrap();
    assert_eq!(serde_json::from_str::<Value>(needs("T7").as_str().unwrap()).unwrap(), json!([{"tag": "android", "n": 2}, {"tag": "ios", "n": 1}]));
    assert_eq!(serde_json::from_str::<Value>(needs("G3").as_str().unwrap()).unwrap(), json!([{"tag": "android", "n": 1}, {"tag": "iphone-15", "n": 1}]));
    assert!(needs("T12").is_null());
    assert_eq!(
        serde_json::from_str::<Value>(needs("T14").as_str().unwrap()).unwrap(),
        json!([{"tag": "pixel-9", "n": 1}, {"tag": "usb", "n": 1}]),
        "old device:<id> is that device, tag:x is tag x"
    );
    let t15 = app.db.q1("SELECT * FROM tasks WHERE id = 15", vec![]).unwrap().unwrap();
    assert!(taskboardd::devices::needs(&app, &t15).unwrap().is_empty(), "an explicit none holds over the goal's needs");
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
    for k in ["bridge_pid", "bridge:last_sync", "dispatch_seen:T7", "usage_guard_handled:2026-09-03", "review_round:T7", "nudge_sent:T7", "jira_desk_session", "review_log_health"] {
        assert!(app.db.get_setting(k).unwrap().is_none(), "{k} is the old board's running state");
        assert!(rep.left_settings.iter().any(|s| s == k));
    }
    assert_eq!(app.db.count("SELECT COUNT(*) FROM jobs WHERE purpose IS NOT 'jira_desk'", vec![]).unwrap(), 0);

    // The reviewer roster: one row per person and project, with what the old board knew of them.
    let rv = |name: &str| app.db.q1("SELECT * FROM reviewers WHERE name = ?", vec![json!(name)]).unwrap().unwrap();
    assert_eq!(app.db.count("SELECT COUNT(*) FROM reviewers", vec![]).unwrap(), 4, "Ana B is Ana (one email); Nobody has no project");
    let ana = rv("Ana");
    assert_eq!((ana.st("project"), ana.st("host_user"), ana.st("source")), ("web".into(), "ana-gh".into(), "import".into()));
    assert_eq!(serde_json::from_str::<Value>(&ana.st("emails")).unwrap(), json!(["ana@acme.dev"]));
    assert_eq!(serde_json::from_str::<Value>(&ana.st("aliases")).unwrap(), json!(["Ana B", "anab"]));
    assert_eq!((ana.i("pinned"), ana.f("automation")), (Some(1), Some(2.0)));
    let bo = rv("Bo");
    assert!(bo.s("removed_at").is_some(), "removed: never asked again");
    assert_eq!(bo.s("removed_why"), Some("Left the team"));
    let bot = rv("Reviewbot");
    assert_eq!((bot.f("bot_every_h"), bot.s("bot_mark"), bot.f("automation")), (Some(6.0), Some("[bot]"), Some(0.5)));
    assert_eq!(rv("Cy").st("project"), "api", "no project of its own: the one it was asked on");
    assert!(rep.skipped.iter().any(|s| s == "reviewers Nobody: no project"));
    assert!(rep.copied.iter().any(|(t, n)| t == "reviewers" && *n == 4));

    // Each ask, with its state, answer and stand-in; an open ask on a finished task is closed.
    let ask = |id: i64| app.db.q1("SELECT * FROM review_asks WHERE id = ?", vec![json!(id)]).unwrap().unwrap();
    let a1 = ask(1);
    assert_eq!((a1.i("reviewer_id"), a1.st("state"), a1.st("why"), a1.st("host_user")), (Some(ana.id()), "open".into(), "pick".into(), "ana-gh".into()));
    assert_eq!((a1.st("pr_host"), a1.st("pr_repo"), a1.i("pr_num")), ("github".into(), "acme/web".into(), Some(41)), "the PR comes from the task");
    assert_eq!((ask(2).st("state"), ask(2).i("reviewer_id")), ("swapped".into(), Some(bo.id())));
    let a3 = ask(3);
    assert_eq!((a3.st("state"), a3.s("answer"), a3.i("replaces"), a3.st("why")), ("answered".into(), Some("changes"), Some(2), "swap".into()));
    assert_eq!((ask(4).st("state"), ask(4).i("reviewer_id")), ("closed".into(), Some(ana.id())), "T12 is done; Ana B's ask is Ana's");
    assert_eq!(ask(5).st("name"), "Cy");
    assert!(rep.skipped.iter().any(|s| s.starts_with("review_asks 6")));

    // Master breaks keep their numbers and verdicts; one open on a project nobody watches is closed.
    let br = |id: i64| app.db.q1("SELECT * FROM breaks WHERE id = ?", vec![json!(id)]).unwrap().unwrap();
    let m1 = br(1);
    assert_eq!((m1.st("state"), m1.st("verdict"), m1.i("task_id"), m1.st("head")), ("closed".into(), "ours".into(), Some(12), "abc1".into()));
    assert_eq!(serde_json::from_str::<Value>(&m1.st("checks")).unwrap(), json!(["build", "test"]));
    assert_eq!(m1.s("verdict_why"), Some("My commit broke the build"));
    assert_eq!((br(2).st("state"), br(2).st("verdict")), ("closed".into(), "not_ours".into()));
    assert!(br(2).s("closed_at").is_some());
    assert!(rep.skipped.iter().any(|s| s.starts_with("M2: open on the old board")));
    assert_eq!((br(4).st("state"), br(4).st("verdict")), ("open".into(), "unsure".into()), "api is watched");
    assert_eq!(taskboardd::breaks::banner(&app).unwrap().len(), 1);

    // Mapped tables aren't parked; a table the board has no place for is kept whole.
    for t in ["devices", "device_loans", "goal_devices", "bits", "reviewers", "review_asks", "master_breaks"] {
        assert!(!rep.kept.iter().any(|(k, _)| k == t), "{t} is mapped, not parked");
        assert!(app.db.get_setting(&format!("import.{t}")).unwrap().is_none());
    }
    let kept: Value = serde_json::from_str(&app.db.get_setting("import.photos").unwrap().unwrap()).unwrap();
    assert_eq!(kept["rows"][0]["url"], "x.png");
    assert!(rep.kept.iter().any(|(t, n)| t == "photos" && *n == 1));
    assert!(rep.lines().iter().any(|l| l.contains("T15") && l.contains("G3") && l.contains("B5")));

    // New work carries on after the old numbers.
    let new = api::dispatch(&app, "POST", "tasks", &Default::default(), &json!({"title": "Next", "project": "web", "detail": "x"})).unwrap();
    assert_eq!(new["ref"], "T16");

    // A board with work in it is refused: the numbers would clash.
    let again = import::import(&app, &old_path).unwrap_err();
    assert_eq!(again.status, 409);
}

#[test]
fn a_goals_device_pool_is_kept_whole() {
    let old_dir = tempfile::tempdir().unwrap();
    let old_path = old_dir.path().join("tasks.db");
    Connection::open(&old_path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE goals(id INTEGER PRIMARY KEY, name TEXT, project TEXT);
             CREATE TABLE goal_devices(goal_id INT, device_id TEXT, purpose TEXT, reserved INT);
             INSERT INTO goals VALUES (3, 'Checkout v2', 'web');
             INSERT INTO goal_devices VALUES (3, 'pixel', 'Payments on Android', 1);
             INSERT INTO goal_devices VALUES (3, 'iphone', 'Payments on iOS', 0);",
        )
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let rep = import::import(&app, &old_path).unwrap();
    assert_eq!(app.db.count("SELECT COUNT(*) FROM device_needs", vec![]).unwrap(), 0, "a pool isn't a need for one of each");
    let kept: Value = serde_json::from_str(&app.db.get_setting("import.goal_devices").unwrap().unwrap()).unwrap();
    assert_eq!(kept["rows"][0]["purpose"], "Payments on Android");
    assert!(rep.kept.iter().any(|(t, n)| t == "goal_devices" && *n == 2));
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
