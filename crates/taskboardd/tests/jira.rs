//! Jira without a token (`via = "claude"`), tickets for every PR task (`auto_ticket`), the Jira desk,
//! and what the handoff says about the goal's setup, its waves, the branch and the owner's footer.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::{Config, JiraProduct};
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, fields, handoff, jira, jira_desk, midna, ops, projects};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board_with(f: impl FnOnce(&mut Config, &std::path::Path)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.jira.site = "acme.atlassian.net".into();
    cfg.jira.project = "PROJ".into();
    f(&mut cfg, dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    projects::set_pr_flow(&app, "webapp", "on").unwrap();
    Board { app, dir }
}

/// A `claude` that answers whatever `answer.json` holds and keeps its arguments in `args.txt` (every call's in `all-args.txt`).
fn fake_claude(cfg: &mut Config, dir: &std::path::Path) {
    let path = dir.join("fake-claude");
    std::fs::write(&path, "#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s\\n' \"$@\" > \"$d/args.txt\"\ncat \"$d/args.txt\" >> \"$d/all-args.txt\"\ncat \"$d/answer.json\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.claude = path.to_string_lossy().to_string();
}

impl Board {
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn post_err(&self, path: &str, body: Value) -> (u16, String) {
        let e = api::dispatch(&self.app, "POST", path, &Query::new(), &body).expect_err("should fail");
        (e.status, e.message)
    }
    fn task(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn card(&self, id: i64) -> Value {
        board::task_card(&self.app, &self.task(id)).unwrap()
    }
    fn new_task(&self, body: Value) -> i64 {
        let mut b = json!({"title": "Add login", "project": "webapp"});
        for (k, v) in body.as_object().unwrap() {
            b[k] = v.clone();
        }
        self.post("/tasks", b)["id"].as_i64().unwrap()
    }
    fn jobs(&self, sql_where: &str) -> Vec<Row> {
        self.app.db.q(&format!("SELECT * FROM jobs WHERE {sql_where} ORDER BY id"), vec![]).unwrap()
    }
    fn answer(&self, v: Value) {
        std::fs::write(self.dir.path().join("answer.json"), json!({"structured_output": v}).to_string()).unwrap();
    }
    fn claude_args(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("args.txt")).unwrap_or_default()
    }
    fn tick(&self) {
        self.app.db.tx(|| jira::ensure_tickets(&self.app)).unwrap();
        jira::run_pending(&self.app).unwrap();
    }
}

// --- via = "claude" ---

#[test]
fn claude_runs_jira_ops_with_the_connector_tools() {
    let b = board_with(|c, d| {
        c.jira.via = "claude".into();
        fake_claude(c, d);
    });
    let t = b.new_task(json!({"jira": {"mode": "link", "key": "PROJ-7"}}));
    b.answer(json!({"ok": true, "status": "In Review"}));
    b.tick();
    assert_eq!(b.task(t).st("jira_status"), "In Review");
    let args = b.claude_args();
    assert!(args.contains("Read the status of PROJ-7"), "{args}");
    assert!(args.contains("--allowedTools\nmcp__claude_ai_Atlassian_MCP,mcp__atlassian"), "{args}");
    assert!(args.contains("--model\nhaiku"), "{args}");

    // A move it can't make fails the job with Claude's reason.
    jira::request(&b.app, "transition", t, "PROJ-7", Some("Done"), None).unwrap();
    b.answer(json!({"ok": false, "error": "PROJ-7 can't go to Done from In Review"}));
    b.tick();
    let j = b.jobs("kind = 'jira' AND json_extract(args, '$.op') = 'transition'");
    assert_eq!(j[0].st("state"), "failed");
    assert!(j[0].st("result").contains("can't go to Done"), "{}", j[0].st("result"));
}

#[test]
fn claude_finds_or_makes_a_ticket_and_picks_the_product() {
    let b = board_with(|c, d| {
        c.jira.via = "claude".into();
        c.jira.products.insert("web".into(), JiraProduct { what: "the customer web app".into(), labels: vec!["web".into()], ..Default::default() });
        c.jira.products.insert("ios".into(), JiraProduct { what: "the iPhone app".into(), ..Default::default() });
        fake_claude(c, d);
    });
    let g = b.post("/goals", json!({"name": "Login", "project": "webapp"}))["id"].as_i64().unwrap();
    b.app.db.update("goals", &json!(g), fields!["epic_key" => "PROJ-1"]).unwrap();
    let t = b.new_task(json!({"goal_id": g, "jira": "create", "title": "Sign in screen"}));
    b.answer(json!({"ok": true, "key": "PROJ-9", "status": "To Do", "found": true, "product": "ios"}));
    b.tick();
    assert_eq!(b.task(t).st("jira_key"), "PROJ-9");
    let args = b.claude_args();
    assert!(args.contains("First search PROJ for an open ticket"), "{args}");
    assert!(args.contains("\"parent\":{\"key\":\"PROJ-1\"}"), "{args}");
    assert!(args.contains("- web: the customer web app"), "{args}");
    assert_eq!(board::get_goal(&b.app, g).unwrap().st("product"), "ios", "the goal takes the product picked for it");
    let log = b.get(&format!("/tasks/T{t}/log"));
    assert!(log.to_string().contains("Linked PROJ-9 (To Do), which already covers it"), "{log}");
}

#[test]
fn qa_checks_jira_through_claude_with_one_batched_search() {
    let b = board_with(|c, d| {
        c.jira.via = "claude".into();
        fake_claude(c, d);
    });
    assert_eq!(b.app.cfg.qa_poll_secs(), 30.0 * 60.0, "through Claude, every 30 minutes");
    let t = b.new_task(json!({}));
    board::update_task(&b.app, t, fields!["jira_key" => "PROJ-7"]).unwrap();
    b.app.db.set_setting("qa_on", Some("1")).unwrap();
    b.app.db.set_setting("qa_since", Some(&taskboardd::util::iso(taskboardd::util::now_ts() - 3600.0))).unwrap();
    let recent = taskboardd::util::iso(taskboardd::util::now_ts() - 60.0);
    let old = taskboardd::util::iso(taskboardd::util::now_ts() - 5.0 * 3600.0);
    b.answer(json!({"ok": true, "comments": [
        {"key": "PROJ-7", "id": "501", "created": recent, "author": "Sam QA", "text": "The button is still grey"},
        {"key": "PROJ-7", "id": "502", "created": old, "author": "Sam QA", "text": "Old"},
        {"key": "PROJ-99", "id": "503", "created": recent, "author": "Sam QA", "text": "Not ours"},
    ]}));
    assert_eq!(taskboardd::qa::poll(&b.app).unwrap(), 1);
    let args = std::fs::read_to_string(b.dir.path().join("all-args.txt")).unwrap();
    assert_eq!(args.matches("Make exactly one JQL search").count(), 1, "one Jira call per check: {args}");
    assert!(args.contains("Make exactly one JQL search, with this query as it is: key in (PROJ-7) AND updated >= -65m"), "{args}");
    assert!(args.contains("UTC now"), "{args}");
    assert!(args.contains("--model\nhaiku"), "{args}");

    let rest = board_with(|_, _| {});
    assert_eq!(rest.app.cfg.qa_poll_secs(), 5.0 * 60.0);
    let mins = board_with(|c, _| c.jira.qa_poll_mins = 12);
    assert_eq!(mins.app.cfg.qa_poll_secs(), 12.0 * 60.0);
}

#[test]
fn rest_without_a_token_says_how_to_fix_it() {
    let b = board_with(|c, _| c.jira.keychain_item = String::new());
    let t = b.new_task(json!({"jira": {"mode": "link", "key": "PROJ-7"}}));
    b.tick();
    let log = b.get(&format!("/tasks/T{t}/log")).to_string();
    assert!(log.contains("via = \\\"claude\\\""), "{log}");
}

#[test]
fn products_from_their_descriptions() {
    let mut ps = std::collections::BTreeMap::new();
    ps.insert("web".to_string(), JiraProduct { what: "the customer-facing web app and its checkout".into(), ..Default::default() });
    ps.insert("ios".to_string(), JiraProduct { what: "the iPhone app".into(), ..Default::default() });
    assert_eq!(jira::pick_product(&ps, "Checkout button does nothing on the web").as_deref(), Some("web"));
    assert_eq!(jira::pick_product(&ps, "Crash when the iPhone rotates").as_deref(), Some("ios"));
    assert_eq!(jira::pick_product(&ps, "Rename a variable"), None);
    assert_eq!(jira::comment_time("2026-10-08T14:03:11.000+0000"), jira::comment_time("2026-10-08T14:03:11Z"));
}

// --- auto_ticket ---

#[test]
fn pr_tasks_wait_for_their_ticket() {
    let b = board_with(|c, _| {
        c.jira.auto_ticket = true;
        c.jira.keychain_item = String::new();
    });
    let t = b.new_task(json!({}));
    assert!(jira::ticket_blocker(&b.app, &b.task(t)).unwrap(), "it waits before it's asked for");
    assert_eq!(b.card(t)["jira"]["status"], "No ticket yet");

    b.app.db.tx(|| jira::ensure_tickets(&b.app)).unwrap();
    assert_eq!(b.jobs("kind = 'jira'").len(), 1);
    assert_eq!(b.card(t)["jira"]["status"], "Ticket asked for");
    b.app.db.tx(|| jira::ensure_tickets(&b.app)).unwrap();
    assert_eq!(b.jobs("kind = 'jira'").len(), 1, "asked once");

    // No token: it fails, and the task keeps waiting with the reason.
    jira::run_pending(&b.app).unwrap();
    let card = b.card(t);
    assert_eq!(card["jira"]["failed"], true);
    assert!(card["jira"]["status"].as_str().unwrap().starts_with("Couldn't make the ticket"), "{card}");
    assert!(card["waiting"].as_str().unwrap().starts_with("Waits for its Jira ticket. Couldn't make it"), "{card}");
    b.app.db.tx(|| jira::ensure_tickets(&b.app)).unwrap();
    assert_eq!(b.jobs("kind = 'jira'").len(), 1, "a failure waits for a retry");

    // Retry, then link one by hand.
    b.post(&format!("/tasks/T{t}"), json!({"jira_key": "new"}));
    assert_eq!(b.jobs("kind = 'jira' AND state = 'pending'").len(), 1);
    b.post(&format!("/tasks/T{t}"), json!({"jira_key": "PROJ-12"}));
    assert!(!jira::ticket_blocker(&b.app, &b.task(t)).unwrap());

    // A task that wants no ticket doesn't wait.
    let n = b.new_task(json!({"jira": "none"}));
    assert!(!jira::ticket_blocker(&b.app, &b.task(n)).unwrap());
    b.post(&format!("/tasks/T{t}"), json!({"jira_key": "none"}));
    assert!(!jira::ticket_blocker(&b.app, &b.task(t)).unwrap());
    assert!(b.task(t).b("jira_none"));
}

#[test]
fn no_ticket_for_projects_without_prs_or_with_the_switch_off() {
    let b = board_with(|c, _| c.jira.auto_ticket = true);
    projects::set_pr_flow(&b.app, "webapp", "off").unwrap();
    let t = b.new_task(json!({}));
    b.app.db.tx(|| jira::ensure_tickets(&b.app)).unwrap();
    assert!(b.jobs("kind = 'jira'").is_empty());
    assert!(!jira::ticket_blocker(&b.app, &b.task(t)).unwrap());

    let off = board_with(|_, _| {});
    let t = off.new_task(json!({}));
    off.app.db.tx(|| jira::ensure_tickets(&off.app)).unwrap();
    assert!(off.jobs("kind = 'jira'").is_empty());
    assert!(off.card(t)["jira"].is_null());
}

// --- the desk ---

#[test]
fn the_desk_finds_or_makes_tickets_and_stays_open() {
    let b = board_with(|c, _| {
        c.jira.auto_ticket = true;
        c.jira.desk = true;
    });
    let t = b.new_task(json!({}));
    assert_eq!(b.card(t)["jira"]["status"], "No ticket yet. The Jira desk finds or makes one…");
    b.tick();
    assert_eq!(b.card(t)["jira"]["status"], "Ticket asked for · Jira desk");

    // No desk yet: the board opens one, in the background ([terminals] background has jira_desk).
    let opened = b.jobs("kind = 'agent' AND purpose = 'jira_desk'");
    assert_eq!(opened.len(), 1);
    let a = board::job_args(&opened[0]);
    assert!(a.get("background").is_none(), "the purpose list decides, not the job");
    assert!(midna::opens_in_background(&b.app, &opened[0]));
    let mut fg = b.app.cfg.clone();
    fg.terminals.background.clear();
    assert!(!midna::opens_in_background(&App::for_tests(fg), &opened[0]), "config can move it out");
    assert!(a.st("flags").contains("Bash(tb jira:*)"), "{}", a.st("flags"));
    b.tick();
    assert_eq!(b.jobs("kind = 'agent' AND purpose = 'jira_desk'").len(), 1, "opened once");

    // Midna opened it: the desk gets the job.
    b.app.db.x("UPDATE jobs SET state = 'done', target = ? WHERE id = ?", taskboardd::p![json!({"session": "s9"}).to_string(), opened[0].id()]).unwrap();
    midna::sync(&b.app, &[json!({"id": "s9", "name": "TB Jira desk", "agent": "claude", "cwd": b.dir.path().to_string_lossy(), "status": {"state": "idle"}})], &[]).unwrap();
    b.tick();
    let sent = b.jobs("kind = 'message' AND purpose = 'jira_desk'");
    assert_eq!(sent.len(), 1);
    let text = board::job_args(&sent[0]).st("text");
    let jid = b.jobs("kind = 'jira'")[0].id();
    assert!(text.starts_with(&format!("[task-board:J{jid}] Jira desk job J{jid}.")), "{text}");
    assert!(text.contains(&format!("tb jira J{jid} ok key=<KEY>")), "{text}");
    b.tick();
    assert_eq!(b.jobs("kind = 'message' AND purpose = 'jira_desk'").len(), 1, "one job at a time");

    // It reports back with tb jira.
    assert_eq!(b.post_err(&format!("/jira/jobs/J{jid}"), json!({"ok": true})).0, 400);
    b.post(&format!("/jira/jobs/J{jid}"), json!({"ok": true, "key": "proj-31", "status": "To Do", "found": true}));
    assert_eq!(b.task(t).st("jira_key"), "PROJ-31");
    assert!(!jira::ticket_blocker(&b.app, &b.task(t)).unwrap());

    // Its terminal says what it's for, and stays open.
    let list = ops::session_list(&b.app, "all").unwrap();
    let desk = list.iter().find(|s| s["id"] == "s9").unwrap();
    assert_eq!(desk["role"], jira_desk::ROLE);
    assert!(desk["close"].is_null());
    assert_eq!(b.post_err("/sessions/s9/close", json!({})).0, 409);
    assert_eq!(b.get("/jira")["desk_session"], "s9");
}

#[test]
fn the_desk_fails_a_job_with_its_reason() {
    let b = board_with(|c, _| {
        c.jira.desk = true;
        c.jira.auto_ticket = true;
    });
    let t = b.new_task(json!({"jira": "create"}));
    let jid = b.jobs("kind = 'jira'")[0].id();
    assert_eq!(b.post_err(&format!("/jira/jobs/J{jid}"), json!({"ok": false})).0, 400);
    b.post(&format!("/jira/jobs/J{jid}"), json!({"ok": false, "message": "PROJ needs a team"}));
    let log = b.get(&format!("/tasks/T{t}/log")).to_string();
    assert!(log.contains("Couldn't make the ticket: PROJ needs a team"), "{log}");

    // What waits says how to go on, and the failure raises an alert that says it too.
    let wait = jira::ticket_wait(&b.app, &b.task(t)).unwrap().unwrap();
    assert_eq!(
        wait,
        format!("Waits for its Jira ticket. Couldn't make it: PROJ needs a team. Try again with tb task set T{t} --jira new, or link one with tb task set T{t} --jira KEY.")
    );
    let alerts = taskboardd::dispatch::alerts(&b.app);
    let a = alerts.iter().find(|a| a["task_id"] == t).expect("an alert");
    let text = a["text"].as_str().unwrap();
    assert!(text.starts_with(&format!("Couldn't make the Jira ticket for T{t} “Add login”: PROJ needs a team.")), "{text}");
    assert!(text.contains(&format!("tb task set T{t} --jira new")) && text.contains("--jira KEY"), "{text}");
}

#[test]
fn the_desk_may_run_tb_by_its_path_without_a_prompt() {
    let tb = "/Applications/Taskboard.app/Contents/MacOS/tb";
    let b = board_with(|c, _| c.jira.desk = true);
    b.app.db.set_setting("tb_path", Some(tb)).unwrap();
    b.new_task(json!({"jira": "create"}));
    b.tick();
    let opened = b.jobs("kind = 'agent' AND purpose = 'jira_desk'");
    let a = board::job_args(&opened[0]);
    let (args, _) = midna::claude_args(&a).unwrap();
    assert_eq!(args[0], "--allowedTools");
    let rules: Vec<&str> = args[1].split(',').collect();
    assert_eq!(rules, vec!["mcp__claude_ai_Atlassian_MCP", "mcp__atlassian", "Bash(tb jira:*)", &format!("Bash({tb} jira:*)")]);

    // Every report it's told to run starts with a command an allow rule covers.
    let prefixes: Vec<&str> = rules.iter().filter_map(|r| r.strip_prefix("Bash(")?.strip_suffix(":*)")).collect();
    let intro = a.st("prompt");
    b.app.db.x("UPDATE jobs SET state = 'done', target = ? WHERE id = ?", taskboardd::p![json!({"session": "s9"}).to_string(), opened[0].id()]).unwrap();
    midna::sync(&b.app, &[json!({"id": "s9", "name": "TB Jira desk", "agent": "claude", "cwd": b.dir.path().to_string_lossy(), "status": {"state": "idle"}})], &[]).unwrap();
    b.tick();
    let job = board::job_args(&b.jobs("kind = 'message' AND purpose = 'jira_desk'")[0]).st("text");
    for text in [intro.as_str(), job.as_str()] {
        let runs: Vec<&str> = text.match_indices(tb).map(|(i, _)| &text[i..]).collect();
        assert!(!runs.is_empty(), "{text}");
        for r in runs {
            assert!(prefixes.iter().any(|p| r.starts_with(p)), "{r}");
        }
    }

    // A path the rule can't hold falls back to plain tb.
    b.app.db.set_setting("tb_path", Some("/odd(path)/tb")).unwrap();
    assert_eq!(jira_desk::allowed_tools(&b.app).last().unwrap(), "Bash(tb jira:*)");
}

#[test]
fn a_goal_with_no_epic_picks_an_open_one_that_covers_it() {
    let b = board_with(|c, d| {
        c.jira.via = "claude".into();
        fake_claude(c, d);
    });
    let g = b.post("/goals", json!({"name": "Checkout redesign", "outcome": "a faster checkout", "project": "webapp"}))["id"].as_i64().unwrap();
    b.new_task(json!({"goal_id": g, "jira": "create", "title": "New pay button"}));
    b.answer(json!({"ok": true, "key": "PROJ-2", "status": "In Progress", "found": true}));
    b.tick();
    let args = b.claude_args();
    assert!(args.contains("First pick an open epic (not done) in PROJ that already covers this work"), "{args}");
    assert!(args.contains("Only if none fits, make a Epic"), "{args}");
    assert_eq!(board::get_goal(&b.app, g).unwrap().st("epic_key"), "PROJ-2");

    let epics = |v: &[(&str, &str)]| v.iter().map(|(k, s)| (k.to_string(), s.to_string())).collect::<Vec<_>>();
    let open = epics(&[("PROJ-9", "Search filters"), ("PROJ-5", "Redesign of the checkout flow"), ("PROJ-3", "Checkout")]);
    assert_eq!(jira::covering_epic("Checkout redesign", &open).as_deref(), Some("PROJ-5"));
    assert_eq!(jira::covering_epic("checkout", &open).as_deref(), Some("PROJ-3"), "the same summary first");
    assert_eq!(jira::covering_epic("Login with SSO", &open), None);
    assert_eq!(jira::covering_epic("The new and the old", &epics(&[("PROJ-1", "The and the new")])), None, "stopwords match nothing");
}

#[test]
fn product_picking_ignores_stopwords() {
    let mut ps = std::collections::BTreeMap::new();
    ps.insert("web".to_string(), JiraProduct { what: "the customer-facing web app and the checkout".into(), ..Default::default() });
    ps.insert("ios".to_string(), JiraProduct { what: "iPhone".into(), ..Default::default() });
    assert_eq!(jira::pick_product(&ps, "Make the thing and the other thing faster"), None);
    assert_eq!(jira::pick_product(&ps, "The checkout and the cart"), Some("web".into()));
}

// --- the handoff ---

#[test]
fn handoff_has_the_setup_waves_branch_and_footer() {
    let b = board_with(|c, d| {
        c.handoff.branch = "{type}/{key}-{slug}".into();
        c.handoff.footer = "Code style: small functions.".into();
        std::fs::write(d.join("footer.md"), "Run cargo fmt before each commit.").unwrap();
        c.handoff.footer_file = d.join("footer.md").to_string_lossy().to_string();
    });
    let g = b.post("/goals", json!({"name": "Login", "project": "webapp"}))["id"].as_i64().unwrap();
    let v = b.post(&format!("/goals/G{g}"), json!({"setup": "Run make bootstrap for {task} ({n}) in wave {wave} of {goal}."}));
    assert_eq!(v["setup"], "Run make bootstrap for {task} ({n}) in wave {wave} of {goal}.");
    let t1 = b.new_task(json!({"goal_id": g, "wave": 1, "title": "Sign-in form", "jira": {"mode": "link", "key": "PROJ-5"}}));
    let t2 = b.new_task(json!({"goal_id": g, "wave": 1, "title": "Session cookie"}));
    let t3 = b.new_task(json!({"goal_id": g, "wave": 2, "title": "Logout"}));
    // The review stop is the owner's own checkbox in the app.
    let mut q = Query::new();
    q.insert(api::FROM.into(), "app".into());
    api::dispatch(&b.app, "POST", &format!("/goals/G{g}/waves/1"), &q, &json!({"name": "Basics", "stop_after": true})).unwrap();
    let mut ctx = board::task_context(&b.task(t2));
    ctx.insert("files".into(), json!(["src/cookie.rs"]));
    board::save_context(&b.app, t2, &ctx, false).unwrap();

    let h = handoff::build(&b.app, t1).unwrap();
    assert!(h.contains("This task is in wave 1 of 2 (Basics)."), "{h}");
    assert!(h.contains(&format!("- T{t2} “Session cookie” (queued, has touched src/cookie.rs)")), "{h}");
    assert!(h.contains("The goal stops after this wave for Sam's review."), "{h}");
    assert!(h.contains(&format!("Next is wave 2: T{t3}.")), "{h}");
    assert!(h.contains(&format!("Set up (every task in this goal does this):\nRun make bootstrap for T{t1} ({t1}) in wave 1 of G{g}.")), "{h}");
    assert!(h.contains("Name its branch feat/PROJ-5-sign-in-form unless the goal's setup says otherwise."), "{h}");
    assert!(h.contains("Code style: small functions.\n\nRun cargo fmt before each commit."), "{h}");
    assert!(handoff::is_full_handoff(&h, t1));
    assert!(h.ends_with(handoff::CLOSING));

    let h3 = handoff::build(&b.app, t3).unwrap();
    assert!(h3.contains("Nothing else runs in this wave."), "{h3}");
    assert!(h3.contains("This is the goal's last wave."), "{h3}");
    assert!(h3.contains("Name its branch feat/logout"), "{h3}");

    b.post(&format!("/goals/G{g}"), json!({"setup": "none"}));
    assert!(!handoff::build(&b.app, t1).unwrap().contains("Set up (every task"));
}

#[test]
fn wave_mates_are_the_ones_still_to_run_with_their_planned_files() {
    let b = board_with(|_, _| {});
    let g = b.post("/goals", json!({"name": "Login", "project": "webapp"}))["id"].as_i64().unwrap();
    let repo = b.dir.path().join("webapp").to_string_lossy().to_string();
    midna::sync(&b.app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
    let r = taskboardd::reports::handle(
        &b.app,
        json!({"event": "tb.propose", "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}, "goal": format!("G{g}"),
               "tasks": ["Sign-in form::Build it::1::::src/form.rs, src/form.css", "Session cookie::Bake it::1", "Old work::Done already::1"]}),
        false,
    )
    .unwrap();
    let ids: Vec<i64> = r["created"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()[1..].parse().unwrap()).collect();
    let (t1, t2, t3) = (ids[0], ids[1], ids[2]);
    assert_eq!(b.task(t1).st("detail"), "Build it");
    for t in [t1, t2] {
        board::update_task(&b.app, t, fields!["status" => "queued"]).unwrap();
    }
    board::update_task(&b.app, t3, fields!["status" => "done"]).unwrap();
    let mut ctx = board::task_context(&b.task(t2));
    ctx.insert("files".into(), json!(["src/cookie.rs"]));
    board::save_context(&b.app, t2, &ctx, false).unwrap();

    let h = handoff::build(&b.app, t2).unwrap();
    assert!(h.contains(&format!("- T{t1} “Sign-in form” (queued, owns src/form.rs, src/form.css)")), "{h}");
    assert!(!h.contains(&format!("T{t3} “Old work”")), "a done mate isn't listed: {h}");
    let h1 = handoff::build(&b.app, t1).unwrap();
    assert!(h1.contains("The wave plans these files for this task: src/form.rs, src/form.css."), "{h1}");
    assert!(h1.contains(&format!("- T{t2} “Session cookie” (queued, has touched src/cookie.rs)")), "{h1}");

    // With every mate done, nothing else runs beside it.
    board::update_task(&b.app, t2, fields!["status" => "done"]).unwrap();
    board::update_task(&b.app, t1, fields!["status" => "done"]).unwrap();
    board::update_task(&b.app, t3, fields!["status" => "queued"]).unwrap();
    assert!(handoff::build(&b.app, t3).unwrap().contains("Nothing else runs in this wave."));
}

#[test]
fn handoff_jira_lines_when_jira_is_on() {
    let b = board_with(|c, _| c.jira.auto_ticket = true);
    let t = b.new_task(json!({}));
    let h = handoff::build(&b.app, t).unwrap();
    assert!(h.contains("Jira: this task's PR needs a ticket, and the board is getting one"), "{h}");
    assert!(h.contains(&format!("--jira KEY. For a new one: tb task set T{t} --jira new")), "{h}");
    let off = board_with(|c, _| c.jira.site = String::new());
    let t = off.new_task(json!({}));
    assert!(!handoff::build(&off.app, t).unwrap().contains("Jira:"));
}

#[test]
fn branch_templates() {
    assert_eq!(handoff::fill_branch("{type}/{key}-{slug}", "fix", "PROJ-3", "Fix the login flicker!", 4), "fix/PROJ-3-fix-the-login-flicker");
    assert_eq!(handoff::fill_branch("{type}/{key}-{slug}", "feat", "", "Add dark mode", 4), "feat/add-dark-mode");
    assert_eq!(handoff::fill_branch("feature/{key}_{slug}", "feat", "", "Add dark mode", 4), "feature/add-dark-mode");
    assert_eq!(handoff::fill_branch("{task}-{slug}", "feat", "", "A", 12), "T12-a");
    assert!(handoff::slug("a very long title that keeps going and going past forty chars").len() <= 40);
}
