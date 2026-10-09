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
        CREATE TABLE reviewers(id INTEGER PRIMARY KEY, project TEXT, email TEXT, name TEXT, aliases TEXT,
          commits INTEGER DEFAULT 0, removed INTEGER DEFAULT 0, pinned INTEGER DEFAULT 0,
          last_asked TEXT, asks INTEGER DEFAULT 0, swaps INTEGER DEFAULT 0);
        CREATE TABLE review_asks(id INTEGER PRIMARY KEY, task_id INTEGER, email TEXT, asked_at TEXT,
          state TEXT DEFAULT 'open', answered_at TEXT);
        CREATE TABLE master_breaks(id INTEGER PRIMARY KEY, sha TEXT, url TEXT, build_id INT, pipeline TEXT, title TEXT,
          state TEXT DEFAULT 'open', verdict TEXT, reason TEXT, opened_at TEXT, closed_at TEXT);
        -- The Python board's migrations.
        ALTER TABLE reviewers ADD COLUMN bb_name TEXT;
        ALTER TABLE reviewers ADD COLUMN bot_every INTEGER;
        ALTER TABLE reviewers ADD COLUMN bot_ran_at TEXT;
        ALTER TABLE reviewers ADD COLUMN automated TEXT;
        ALTER TABLE reviewers ADD COLUMN slack_id TEXT;
        ALTER TABLE review_asks ADD COLUMN replaces INT;
        ALTER TABLE review_asks ADD COLUMN filled_at TEXT;
        ALTER TABLE review_asks ADD COLUMN tries INTEGER DEFAULT 0;
        ALTER TABLE review_asks ADD COLUMN busy TEXT;
        ALTER TABLE review_asks ADD COLUMN swapped_at TEXT;
        ALTER TABLE review_asks ADD COLUMN reply TEXT;
        ALTER TABLE review_asks ADD COLUMN reply_said TEXT;
        ALTER TABLE master_breaks ADD COLUMN proof TEXT;
        ALTER TABLE master_breaks ADD COLUMN fix TEXT;
        ALTER TABLE master_breaks ADD COLUMN error TEXT;
        ALTER TABLE master_breaks ADD COLUMN base TEXT;
        CREATE TABLE photos(id INTEGER PRIMARY KEY, url TEXT);
        CREATE TABLE devices(id TEXT PRIMARY KEY, name TEXT, tags TEXT, blocked TEXT, removed_at TEXT);
        CREATE TABLE device_loans(id INTEGER PRIMARY KEY, device_id TEXT, task_id INT, lent_at TEXT, returned_at TEXT);
        CREATE TABLE goal_devices(goal_id INT, device_id TEXT, purpose TEXT, reserved INT);
        CREATE TABLE bits(id INTEGER PRIMARY KEY, name TEXT, kind TEXT, project TEXT, made_at TEXT, goal_id INT);

        INSERT INTO goals VALUES (3, 'Checkout v2', 'Pay with one click', 'web', '2026-09-01T10:00:00Z', 0, 'Run make seed first');
        INSERT INTO goal_notes VALUES (1, 3, 'decision', 'Stripe, not Adyen', 'you', '2026-09-01T11:00:00Z');
        INSERT INTO tasks VALUES (7, 'Card form', 'Build it', 'web', '{web}', 'needs', 3, 1, 'midna-1', 'conv-7',
          'https://github.com/acme/web/pull/41', NULL, 41, 'review', '2026-09-02T09:00:00Z', '2026-09-03T09:00:00Z', '2026-09-02T09:05:00Z', NULL, NULL, 'x',
          'android:2 ios', 1, NULL, NULL, '["checkout.v2","card_form_local"]');
        INSERT INTO tasks VALUES (12, 'Receipts', 'Email them', 'web', '{web}', 'done', 3, 2, NULL, 'conv-12',
          NULL, 'acme/web', 50, NULL, '2026-09-02T09:00:00Z', '2026-09-04T09:00:00Z', '2026-09-03T09:00:00Z', '2026-09-04T09:00:00Z', 'Sent', NULL,
          'android:99', 0, 'Emails ship with the API change', 7, NULL);
        INSERT INTO tasks VALUES (13, 'Totals', 'Round them', 'web', '{web}', 'done', 3, 5, NULL, NULL,
          NULL, 'acme/web', 51, NULL, '2026-09-02T09:00:00Z', '2026-09-05T09:00:00Z', '2026-09-03T09:00:00Z', '2026-09-04T09:00:00Z', NULL, NULL,
          NULL, NULL, NULL, NULL, NULL);
        INSERT INTO tasks VALUES (14, 'Receipt API', 'Serve them', 'api', '{api}', 'needs', NULL, 3, NULL, NULL,
          NULL, 'api', 9, 'checks', '2026-09-02T09:00:00Z', '2026-09-04T09:00:00Z', NULL, NULL, NULL, NULL, 'device:pixel tag:usb', NULL, NULL, NULL, NULL);
        INSERT INTO tasks VALUES (15, 'Mirror', 'Elsewhere', 'mirror', '/nowhere', 'needs', 3, 4, NULL, NULL,
          NULL, 'mirror', 3, NULL, '2026-09-02T09:00:00Z', '2026-09-04T09:00:00Z', NULL, NULL, NULL, NULL, 'none', NULL, NULL, NULL, NULL);
        ALTER TABLE tasks ADD COLUMN pr_state TEXT;
        UPDATE tasks SET pr_state = 'OPEN' WHERE id IN (7, 12);
        UPDATE tasks SET pr_state = 'MERGED' WHERE id = 13;
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
        INSERT INTO settings VALUES ('saggar_projects', '[{"name":"web"}]');
        INSERT INTO jobs VALUES (1, 'agent', 'pending');
        INSERT INTO reviewers VALUES (1, 'web', 'ana.old@acme.dev', 'Ana B', '["ana@acme.dev","anab"]', 6, 1, 0, '2026-09-01T09:00:00Z', 3, 1, NULL, NULL, NULL, NULL, NULL);
        INSERT INTO reviewers VALUES (2, 'web', 'ana@acme.dev', 'Ana', NULL, 579, 0, 1, '2026-09-03T09:00:00Z', 12, 2, 'Ana Lima', NULL, NULL, 'yes', NULL);
        INSERT INTO reviewers VALUES (3, 'web', 'bo@acme.dev', 'Bo', NULL, 40, 1, 0, NULL, 0, 0, NULL, NULL, NULL, NULL, NULL);
        INSERT INTO reviewers VALUES (4, 'web', 'bot@acme.dev', 'Reviewbot', NULL, 10, 0, 0, NULL, 0, 0, 'Review Bot', 180, '2026-09-03T08:00:00Z', 'sometimes', NULL);
        INSERT INTO reviewers VALUES (5, NULL, 'cy@acme.dev', 'Cy', NULL, 3, 0, 0, NULL, 0, 0, NULL, NULL, NULL, 'no', NULL);
        INSERT INTO reviewers VALUES (6, NULL, 'nobody@acme.dev', 'Nobody', NULL, 0, 0, 0, NULL, 0, 0, NULL, NULL, NULL, NULL, NULL);
        -- Dee: an active row with fewer commits than her removed one; she stays active.
        INSERT INTO reviewers VALUES (7, 'web', 'dee.old@acme.dev', 'Dee O', '["dee@acme.dev"]', 500, 1, 0, NULL, 0, 0, NULL, NULL, NULL, NULL, NULL);
        INSERT INTO reviewers VALUES (8, 'web', 'dee@acme.dev', 'Dee', NULL, 50, 0, 0, NULL, 0, 0, NULL, NULL, NULL, 'some', NULL);
        -- Eve: two rows that share only a Slack id.
        INSERT INTO reviewers VALUES (9, 'web', 'eve@acme.dev', 'Eve', NULL, 20, 0, 0, NULL, 0, 0, NULL, NULL, NULL, NULL, 'U0EVE');
        INSERT INTO reviewers VALUES (10, 'web', 'eve@home.dev', 'Eve H', NULL, 2, 0, 0, NULL, 0, 0, NULL, NULL, NULL, NULL, 'U0EVE');
        INSERT INTO review_asks VALUES (1, 7, 'ana@acme.dev', '2026-09-03T09:00:00Z', 'open', NULL, NULL, NULL, 0, NULL, NULL, NULL, NULL);
        INSERT INTO review_asks VALUES (2, 7, 'bo@acme.dev', '2026-09-03T09:00:00Z', 'open', NULL, NULL, '2026-09-03T13:00:00Z', 2, 'in a meeting', '2026-09-03T11:00:00Z', NULL, NULL);
        INSERT INTO review_asks VALUES (3, 7, 'bot@acme.dev', '2026-09-03T11:00:00Z', 'answered', '2026-09-03T12:00:00Z', 2, NULL, 1, NULL, NULL, 'approved', NULL);
        INSERT INTO review_asks VALUES (4, 12, 'ana.old@acme.dev', '2026-09-03T09:00:00Z', 'open', NULL, NULL, NULL, 0, NULL, NULL, NULL, NULL);
        INSERT INTO review_asks VALUES (5, 14, 'cy@acme.dev', '2026-09-03T09:00:00Z', 'open', NULL, NULL, NULL, 0, NULL, NULL, NULL, NULL);
        INSERT INTO review_asks VALUES (6, 99, 'zed@acme.dev', '2026-09-03T09:00:00Z', 'open', NULL, NULL, NULL, 0, NULL, NULL, NULL, NULL);
        INSERT INTO review_asks VALUES (7, 13, 'anab', '2026-09-03T09:00:00Z', 'open', NULL, NULL, NULL, 0, NULL, NULL, NULL, NULL);
        -- Answered, then swapped off: not a reviewer on the PR any more.
        INSERT INTO review_asks VALUES (8, 12, 'bo@acme.dev', '2026-09-03T09:00:00Z', 'answered', '2026-09-03T10:00:00Z', NULL, NULL, 0, NULL, '2026-09-03T11:00:00Z', NULL, 'Needs changes to the totals');
        -- Swapped off, then answered anyway, in words of its own.
        INSERT INTO review_asks VALUES (9, 12, 'bot@acme.dev', '2026-09-03T09:00:00Z', 'open', '2026-09-03T12:00:00Z', NULL, NULL, 0, NULL, '2026-09-03T11:00:00Z', 'Left two notes', NULL);
        INSERT INTO master_breaks VALUES (1, 'abc1', 'https://bitbucket.org/acme/api/pipelines/results/41', '41', 'Pipeline', 'Build fails on master',
          'fixed', 'yours', 'My commit broke the build', '2026-09-03T09:00:00Z', '2026-09-03T10:00:00Z', NULL, 'T12', 'error: x
          at y', 'master');
        INSERT INTO master_breaks VALUES (2, 'def2', 'https://bitbucket.org/acme/api/pipelines/results/42', '42', 'Pipeline', 'Lint fails',
          'open', 'not_yours', 'Not our files', '2026-09-04T09:00:00Z', NULL, 'https://bitbucket.org/acme/api/pipelines/results/40 failed the same', NULL, NULL, 'master');
        INSERT INTO master_breaks VALUES (4, 'fed4', NULL, NULL, NULL, 'Flaky?', 'open', 'unsure', 'Flaky?', '2026-09-04T10:00:00Z', NULL, NULL, NULL, NULL, 'release');
        INSERT INTO photos VALUES (1, 'x.png');
        INSERT INTO goals VALUES (4, 'Wallet', 'Pay later', 'web', '2026-09-01T10:00:00Z', 0, NULL);
        INSERT INTO devices VALUES ('pixel', 'Pixel 9', 'android,phone', NULL, NULL);
        INSERT INTO devices VALUES ('emu', 'emu-1', 'android', NULL, NULL);
        INSERT INTO devices VALUES ('iphone', 'iPhone 15', '["ios","phone","usb"]', 'the demo', NULL);
        INSERT INTO devices VALUES ('dup', 'pixel 9', 'android', NULL, NULL);
        INSERT INTO device_loans VALUES (1, 'pixel', 7, '2026-09-03T09:00:00Z', NULL);
        INSERT INTO device_loans VALUES (2, 'gone-device', 7, '2026-09-03T09:00:00Z', NULL);
        INSERT INTO device_loans VALUES (3, 'emu', 12, '2026-09-03T09:00:00Z', NULL);
        -- Purposes as a comma list; two goals reserve the pixel.
        INSERT INTO goal_devices VALUES (3, 'pixel', 'measure,Demo day', 1);
        INSERT INTO goal_devices VALUES (4, 'pixel', NULL, 1);
        INSERT INTO goal_devices VALUES (3, 'iphone', NULL, 0);
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
    let devs = app.db.q("SELECT name, tags, note, off FROM devices ORDER BY name", vec![]).unwrap();
    let names: Vec<String> = devs.iter().map(|d| d.st("name")).collect();
    assert_eq!(names, vec!["emu-1", "iphone-15", "pixel-9"]);
    let pixel = &devs[2];
    assert_eq!(serde_json::from_str::<Value>(&pixel.st("tags")).unwrap(), json!(["android", "phone"]));
    assert_eq!((devs[1].i("off"), devs[1].s("note")), (Some(1), Some("Kept for the demo")), "a blocked device comes over off");
    assert!(rep.skipped.iter().any(|s| s.contains("devices pixel-9")), "two devices with one name: the second is listed");
    assert_eq!(taskboardd::devices::lent(&app, 7).unwrap(), vec!["pixel-9"]);
    assert!(rep.skipped.iter().any(|s| s.starts_with("device_loans 2")));
    assert!(taskboardd::devices::lent(&app, 12).unwrap().is_empty(), "a finished task's loan comes back");
    assert_eq!(app.db.val("SELECT released_at FROM device_loans WHERE task_id = 12", vec![]).unwrap(), json!("2026-09-04T09:00:00Z"));
    let needs = |owner: &str| app.db.val("SELECT needs FROM device_needs WHERE owner = ?", vec![json!(owner)]).unwrap();
    assert_eq!(serde_json::from_str::<Value>(needs("T7").as_str().unwrap()).unwrap(), json!([{"tag": "android", "n": 2}, {"tag": "ios", "n": 1}]));
    assert!(needs("G3").is_null(), "the goal's devices are its pool, not needs");
    assert!(needs("T12").is_null());
    // The goals' pools: a comma list of purposes, each a tag; both goals' reservations of the pixel.
    let pool = |g: &str| get(&app, &format!("goals/{g}/devices"))["devices"].as_array().unwrap().iter().map(|d| (d["name"].as_str().unwrap().to_string(), d["purpose"].clone(), d["reserved"] == true)).collect::<Vec<_>>();
    assert_eq!(pool("G3"), vec![("iphone-15".into(), Value::Null, false), ("pixel-9".into(), json!("measure,demo-day"), true)]);
    assert_eq!(pool("G4"), vec![("pixel-9".into(), Value::Null, true)]);
    assert_eq!(get(&app, "devices/pixel-9")["reserved_for"], "G3 and G4");
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
    for k in ["bridge_pid", "bridge:last_sync", "dispatch_seen:T7", "usage_guard_handled:2026-09-03", "review_round:T7", "nudge_sent:T7", "jira_desk_session", "review_log_health", "saggar_projects"] {
        assert!(app.db.get_setting(k).unwrap().is_none(), "{k} is the old board's running state");
        assert!(rep.left_settings.iter().any(|s| s == k));
    }
    assert_eq!(app.db.count("SELECT COUNT(*) FROM jobs WHERE purpose IS NOT 'jira_desk'", vec![]).unwrap(), 0);

    // The reviewer roster: one row per person and project, with what the old board knew of them.
    let rv = |name: &str| app.db.q1("SELECT * FROM reviewers WHERE name = ?", vec![json!(name)]).unwrap().unwrap();
    assert_eq!(app.db.count("SELECT COUNT(*) FROM reviewers", vec![]).unwrap(), 6, "Ana B is Ana (an alias is her email), Dee O is Dee, Eve H is Eve; Nobody has no project");
    let ana = rv("Ana");
    assert_eq!((ana.st("project"), ana.st("source")), ("web".into(), "import".into()));
    assert!(ana.s("removed_at").is_none(), "folding in a removed alias row leaves Ana as she was");
    assert_eq!(ana.i("commits"), Some(585), "the folded row's commits add up");
    assert_eq!(serde_json::from_str::<Value>(&ana.st("emails")).unwrap(), json!(["ana@acme.dev", "ana.old@acme.dev"]));
    let aliases: Vec<String> = serde_json::from_str(&ana.st("aliases")).unwrap();
    for a in ["Ana Lima", "Ana B", "anab"] {
        assert!(aliases.iter().any(|x| x == a), "{a} names Ana: {aliases:?}");
    }
    assert_eq!((ana.i("pinned"), ana.f("automation")), (Some(1), Some(6.67)), "automated = yes: 1.0 against no's 0.15");
    let ana_d = taskboardd::reviewers::dict(&app, &ana).unwrap();
    assert_eq!((ana_d["asks"].as_i64(), ana_d["swaps"].as_i64(), ana_d["last_asked"].as_str()), (Some(15), Some(3), Some("2026-09-03T09:00:00Z")), "the old counts carry on");
    assert_eq!(taskboardd::reviewers::ask_count(&app, &ana).unwrap(), 15);
    let bo = rv("Bo");
    assert!(bo.s("removed_at").is_some(), "removed: never asked again");
    let bot = rv("Reviewbot");
    assert_eq!((bot.f("bot_every_h"), bot.f("automation")), (Some(3.0), Some(2.33)), "bot_every is minutes; automated = sometimes");
    assert!(bot.st("aliases").contains("Review Bot"), "bb_name is an alias");
    assert_eq!(app.db.val("SELECT at FROM reviewer_bot_runs WHERE reviewer_id = ?", vec![json!(bot.id())]).unwrap(), json!("2026-09-03T08:00:00Z"), "bot_ran_at is its last run");
    assert!(rep.copied.iter().any(|(t, n)| t == "reviewer_bot_runs" && *n == 1));
    assert_eq!(rv("Cy").st("project"), "api", "no project of its own: the one it was asked on");
    assert_eq!(rv("Cy").f("automation"), Some(1.0), "automated = no is the board's normal");
    assert!(rep.skipped.iter().any(|s| s == "reviewers Nobody: no project"));
    assert!(rep.copied.iter().any(|(t, n)| t == "reviewers" && *n == 6));
    let dee = rv("Dee");
    assert!(dee.s("removed_at").is_none(), "her active row stays, though the removed one has more commits");
    assert_eq!((dee.i("commits"), dee.f("automation")), (Some(550), Some(4.0)), "automated = some");
    let eve = rv("Eve");
    assert_eq!(serde_json::from_str::<Value>(&eve.st("emails")).unwrap(), json!(["eve@acme.dev", "eve@home.dev"]), "one Slack id, one person");
    assert_eq!((eve.st("slack"), eve.i("commits")), ("U0EVE".into(), Some(22)));

    // Each ask, matched by email or alias, with its state and stand-in; an open ask closes only
    // when its PR is merged or closed, not because its task is done.
    let ask = |id: i64| app.db.q1("SELECT * FROM review_asks WHERE id = ?", vec![json!(id)]).unwrap().unwrap();
    let a1 = ask(1);
    assert_eq!((a1.i("reviewer_id"), a1.st("state"), a1.st("why"), a1.st("name")), (Some(ana.id()), "open".into(), "pick".into(), "Ana".into()));
    assert_eq!((a1.st("pr_host"), a1.st("pr_repo"), a1.i("pr_num")), ("github".into(), "acme/web".into(), Some(41)), "the PR comes from the task");
    let a2 = ask(2);
    assert_eq!((a2.st("state"), a2.i("reviewer_id"), a2.i("filled")), ("swapped".into(), Some(bo.id()), Some(1)), "swapped_at: swapped; filled_at: its fill-in was asked");
    assert_eq!(a2.s("closed_at"), Some("2026-09-03T11:00:00Z"));
    let a3 = ask(3);
    assert_eq!((a3.st("state"), a3.i("reviewer_id"), a3.i("replaces"), a3.st("why")), ("answered".into(), Some(bot.id()), Some(2), "swap".into()), "replaces is Bo's ask");
    assert_eq!(a3.s("answer"), Some("approved"), "reply is the answer");
    let a8 = ask(8);
    assert_eq!((a8.st("state"), a8.s("answer"), a8.s("answered_at")), ("swapped".into(), Some("changes"), Some("2026-09-03T10:00:00Z")), "answered, then swapped off");
    let a9 = ask(9);
    assert_eq!((a9.st("state"), a9.s("answer")), ("came_back".into(), Some("Left two notes")), "swapped off, then answered: the old words kept");
    let live: Vec<String> = taskboardd::asks::pill_info(&app, 12).unwrap().into_keys().collect();
    assert!(!live.iter().any(|u| u.contains("bo")), "Bo was swapped off T12's PR: {live:?}");
    assert_eq!((ask(4).st("state"), ask(4).i("reviewer_id")), ("open".into(), Some(ana.id())), "T12 is done but its PR is still open; Ana B's email is Ana's");
    assert_eq!((ask(7).st("state"), ask(7).i("reviewer_id")), ("closed".into(), Some(ana.id())), "T13's PR merged; anab is Ana");
    assert_eq!(ask(5).st("name"), "Cy");
    assert!(rep.skipped.iter().any(|s| s.starts_with("review_asks 6")));
    assert_eq!(rep.copied.iter().find(|(t, _)| t == "review_asks").map(|(_, n)| *n), Some(8));
    let extra: Value = serde_json::from_str(&app.db.get_setting("import.review_asks.unmapped").unwrap().unwrap()).unwrap();
    assert_eq!(extra["columns"], json!(["tries", "busy"]), "what the board has no place for is kept");
    assert!(extra["rows"].as_array().unwrap().contains(&json!({"id": 2, "tries": 2, "busy": "in a meeting"})));

    // Master breaks keep their numbers and verdicts, on the one project the old board watched.
    let br = |id: i64| app.db.q1("SELECT * FROM breaks WHERE id = ?", vec![json!(id)]).unwrap().unwrap();
    let m1 = br(1);
    assert_eq!((m1.st("project"), m1.st("state"), m1.st("verdict"), m1.i("task_id"), m1.st("head")), ("api".into(), "closed".into(), "ours".into(), Some(12), "abc1".into()), "fix T12 is the fix task");
    assert_eq!(serde_json::from_str::<Value>(&m1.st("checks")).unwrap(), json!(["Pipeline"]));
    assert_eq!(m1.s("verdict_why"), Some("My commit broke the build"));
    let ev: Value = serde_json::from_str(&m1.st("evidence")).unwrap();
    assert_eq!(ev["checks"][0]["url"], "https://bitbucket.org/acme/api/pipelines/results/41");
    assert_eq!(ev["checks"][0]["steps"], json!(["error: x", "at y"]));
    assert_eq!((ev["title"].as_str(), ev["build_id"].as_str(), ev["fix"].as_str()), (Some("Build fails on master"), Some("41"), Some("T12")));
    assert_eq!(m1.st("branch"), "master", "base is the branch");
    let m2 = br(2);
    assert_eq!((m2.st("state"), m2.st("verdict")), ("open".into(), "not_ours".into()));
    assert_eq!(serde_json::from_str::<Value>(&m2.st("proof")).unwrap(), json!(["https://bitbucket.org/acme/api/pipelines/results/40"]));
    assert_eq!((br(4).st("state"), br(4).st("verdict")), ("open".into(), "unsure".into()), "api is watched");
    assert!(taskboardd::breaks::banner(&app).unwrap().is_empty(), "the banner is only for the owner's breaks");
    assert_eq!(app.db.get_setting("master_watch").unwrap().map(|w| serde_json::from_str::<Value>(&w).unwrap()["api"]["branch"].clone()), Some(json!("release")), "the newest break's branch carries over");
    assert!(app.db.get_setting("import.master_breaks.unmapped").unwrap().is_none(), "every break column has a place");

    // Mapped tables aren't parked; a table the board has no place for is kept whole.
    for t in ["devices", "device_loans", "goal_devices", "bits", "reviewers", "review_asks", "master_breaks"] {
        assert!(!rep.kept.iter().any(|(k, _)| k == t), "{t} is mapped, not parked");
        assert!(app.db.get_setting(&format!("import.{t}")).unwrap().is_none());
    }
    let kept: Value = serde_json::from_str(&app.db.get_setting("import.photos").unwrap().unwrap()).unwrap();
    assert_eq!(kept["rows"][0]["url"], "x.png");
    assert!(rep.kept.iter().any(|(t, n)| t == "photos" && *n == 1));
    assert!(rep.lines().iter().any(|l| l.contains("T15") && l.contains("G4") && l.contains("B5")));

    // New work carries on after the old numbers.
    let new = api::dispatch(&app, "POST", "tasks", &Default::default(), &json!({"title": "Next", "project": "web", "detail": "x"})).unwrap();
    assert_eq!(new["ref"], "T16");

    // A board with work in it is refused: the numbers would clash.
    let again = import::import(&app, &old_path).unwrap_err();
    assert_eq!(again.status, 409);
}

#[test]
fn a_goals_device_pool_comes_over_as_its_own_with_blocked_and_removed_devices() {
    let old_dir = tempfile::tempdir().unwrap();
    let old_path = old_dir.path().join("tasks.db");
    Connection::open(&old_path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE goals(id INTEGER PRIMARY KEY, name TEXT, project TEXT);
             CREATE TABLE devices(id TEXT PRIMARY KEY, name TEXT, tags TEXT, blocked TEXT, removed_at TEXT);
             CREATE TABLE goal_devices(goal_id INT, device_id TEXT, purpose TEXT, reserved INT);
             INSERT INTO goals VALUES (3, 'Checkout v2', 'web');
             INSERT INTO devices VALUES ('d1', 'Pixel', 'android', NULL, NULL);
             INSERT INTO devices VALUES ('d2', 'iPhone', 'ios', NULL, NULL);
             INSERT INTO devices VALUES ('d3', 'Bench rig', 'bench', 'the demo', NULL);
             INSERT INTO devices VALUES ('d4', 'Old tab', 'android', NULL, '2026-01-02T00:00:00');
             INSERT INTO goal_devices VALUES (3, 'd1', 'Payments on Android', 1);
             INSERT INTO goal_devices VALUES (3, 'd2', 'measure', 0);
             INSERT INTO goal_devices VALUES (3, 'd4', NULL, 0);",
        )
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let rep = import::import(&app, &old_path).unwrap();
    assert_eq!(app.db.count("SELECT COUNT(*) FROM device_needs", vec![]).unwrap(), 0, "a pool isn't a need for one of each");
    assert!(app.db.get_setting("import.goal_devices").unwrap().is_none(), "no longer parked");
    let pool = api::dispatch(&app, "GET", "goals/G3/devices", &Default::default(), &json!({})).unwrap();
    let rows: Vec<(String, Value, bool)> = pool["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["name"].as_str().unwrap().to_string(), d["purpose"].clone(), d["reserved"] == true))
        .collect();
    assert_eq!(rows, vec![("iphone".into(), json!("measure"), false), ("pixel".into(), json!("payments-on-android"), true)]);
    assert!(rep.copied.iter().any(|(t, n)| t == "goal_devices" && *n == 2));
    let rig = api::dispatch(&app, "GET", "devices/bench-rig", &Default::default(), &json!({})).unwrap();
    assert_eq!((rig["off"].clone(), rig["note"].clone()), (json!(true), json!("Kept for the demo")));
    assert_eq!(api::dispatch(&app, "GET", "devices/old-tab", &Default::default(), &json!({})).unwrap_err().status, 404, "a removed device stays out");
    assert!(rep.skipped.iter().any(|s| s.contains("old-tab: removed")), "{:?}", rep.skipped);
}

/// An older devices shape: a kind, a focus command, a disabled flag, and goal needs by tag and count.
#[test]
fn older_device_columns_and_goal_needs_come_over() {
    let old_dir = tempfile::tempdir().unwrap();
    let old_path = old_dir.path().join("tasks.db");
    Connection::open(&old_path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE goals(id INTEGER PRIMARY KEY, name TEXT, project TEXT);
             CREATE TABLE devices(id TEXT PRIMARY KEY, name TEXT, kind TEXT, tags TEXT, focus_cmd TEXT, disabled INT DEFAULT 0);
             CREATE TABLE goal_devices(goal_id INT, tag TEXT, count INT);
             INSERT INTO goals VALUES (3, 'Checkout v2', 'web');
             INSERT INTO devices VALUES ('pixel', 'Pixel 9', 'android', 'phone', 'open -a Pixel', 0);
             INSERT INTO devices VALUES ('iphone', 'iPhone 15', 'ios', NULL, NULL, 1);
             INSERT INTO goal_devices VALUES (3, 'android', 1);
             INSERT INTO goal_devices VALUES (3, 'iphone', 1);",
        )
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    import::import(&app, &old_path).unwrap();
    let devs = app.db.q("SELECT name, tags, focus, off FROM devices ORDER BY name", vec![]).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&devs[1].st("tags")).unwrap(), json!(["phone", "android"]));
    assert_eq!(devs[1].s("focus"), Some("open -a Pixel"));
    assert_eq!(devs[0].i("off"), Some(1), "a disabled device comes over off");
    let needs = app.db.val("SELECT needs FROM device_needs WHERE owner = 'G3'", vec![]).unwrap();
    assert_eq!(serde_json::from_str::<Value>(needs.as_str().unwrap()).unwrap(), json!([{"tag": "android", "n": 1}, {"tag": "iphone-15", "n": 1}]));
}

/// The Python board's breaks name no project: they're on `--master-project`, else skipped and listed.
#[test]
fn breaks_without_a_project_take_the_one_named() {
    let old_dir = tempfile::tempdir().unwrap();
    let old_path = old_dir.path().join("tasks.db");
    Connection::open(&old_path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE goals(id INTEGER PRIMARY KEY, name TEXT, project TEXT);
             CREATE TABLE master_breaks(id INTEGER PRIMARY KEY, sha TEXT, state TEXT, fix TEXT, base TEXT, opened_at TEXT);
             INSERT INTO master_breaks VALUES (3, 'abc1', 'open', 'deadbeef1', 'master', '2026-09-03T09:00:00Z');
             INSERT INTO master_breaks VALUES (5, 'abc2', 'open', NULL, 'cafe123', '2026-09-04T09:00:00Z');",
        )
        .unwrap();
    // No project, none watched: listed, not guessed.
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let rep = import::import(&app, &old_path).unwrap();
    assert_eq!(app.db.count("SELECT COUNT(*) FROM breaks", vec![]).unwrap(), 0);
    assert!(rep.skipped.iter().any(|s| s.contains("M3: no project") && s.contains("--master-project")), "{:?}", rep.skipped);

    // Named: they come over on it, closed when the board doesn't watch it.
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let rep = import::import_with(&app, &old_path, &import::Options { master_project: Some("web".into()) }).unwrap();
    let m3 = app.db.q1("SELECT * FROM breaks WHERE id = 3", vec![]).unwrap().unwrap();
    assert_eq!((m3.st("project"), m3.st("state"), m3.st("fixed_head"), m3.st("branch")), ("web".into(), "closed".into(), "deadbeef1".into(), "master".into()));
    assert!(rep.skipped.iter().any(|s| s.starts_with("M3: open on the old board")));
    let m5 = app.db.q1("SELECT * FROM breaks WHERE id = 5", vec![]).unwrap().unwrap();
    assert_eq!((m5.s("branch"), m5.st("green_head")), (None, "cafe123".into()), "a sha for a base is the last green head");
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

#[test]
fn the_review_switches_the_old_board_used_come_on() {
    let old_dir = tempfile::tempdir().unwrap();
    let old_path = old_dir.path().join("tasks.db");
    Connection::open(&old_path)
        .unwrap()
        .execute_batch(
            r#"CREATE TABLE tasks(id INTEGER PRIMARY KEY, title TEXT, project TEXT, status TEXT, pr_repo TEXT, pr_num INT, pr_phase TEXT, pr_flow TEXT);
             CREATE TABLE review_asks(id INTEGER PRIMARY KEY, task_id INT, user TEXT, status TEXT, reason TEXT, at TEXT);
             INSERT INTO tasks VALUES (1, 'Ask stage', 'web', 'done', 'acme/web', 4, 'ask', NULL);
             INSERT INTO tasks VALUES (2, 'Asked', 'api', 'done', 'acme/api', 5, 'review', '{"asked": {"at": "2026-09-01T10:00:00Z"}}');
             INSERT INTO tasks VALUES (3, 'Plain', 'docs', 'done', 'acme/docs', 6, 'review', 'not json');
             INSERT INTO tasks VALUES (4, 'Notes', 'blog', 'done', NULL, NULL, NULL, NULL);
             INSERT INTO review_asks VALUES (1, 3, 'ana', 'pending', 'auto', '2026-09-01T10:00:00Z');
             INSERT INTO review_asks VALUES (2, 3, 'bo', 'pending', 'timeout', '2026-09-01T11:00:00Z');"#,
        )
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let rep = import::import(&app, &old_path).unwrap();
    let on = |p: &str| taskboardd::reviewers::ask_stage_on(&app, Some(p));
    assert!(on("web") && on("api"));
    assert!(!on("docs"), "nothing shows the stage there");
    assert!(rep.lines().iter().any(|l| l.contains("turned on") && l.contains("web: ask stage")), "{:?}", rep.lines());
    let swaps = |p: &str| taskboardd::reviewers::swap_on(&app, Some(p));
    assert!(swaps("docs"), "it swapped a reviewer who timed out there");
    assert!(swaps("web") && swaps("api"), "the old board always swapped: a project with PRs and no swap history too");
    assert!(!swaps("blog"), "a project that came over with no PR or ask");
}
