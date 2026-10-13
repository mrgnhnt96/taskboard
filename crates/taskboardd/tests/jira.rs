//! Jira without a token (`via = "claude"`), tickets for every PR task (`auto_ticket`), the Jira desk,
//! and what the handoff says about the goal's setup, its waves, the branch and the owner's footer.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::{Config, JiraProduct};
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, fields, handoff, jira, jira_claude, jira_desk, midna, ops, projects};

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
    assert!(args.contains("--allowedTools\nmcp__claude_ai_Atlassian_MCP__getJiraIssue,mcp__claude_ai_Atlassian_MCP__executeRead\n"), "{args}");
    assert!(args.contains("--model\nhaiku"), "{args}");
    assert!(args.contains("--setting-sources\nuser\n"), "the claude.ai connectors are the user's: {args}");
    assert!(args.contains("--permission-mode\ndefault\n"), "the owner's allow rules or mode can't widen it: {args}");
    assert!(args.contains("__createJiraIssue,"), "a read denies the writes: {args}");
    assert!(args.contains("Never create, edit or delete anything else."), "{args}");

    // A move it can't make fails the job with Claude's reason.
    jira::request(&b.app, "transition", t, "PROJ-7", Some("Done"), None).unwrap();
    b.answer(json!({"ok": false, "error": "PROJ-7 can't go to Done from In Review"}));
    b.tick();
    let args = b.claude_args();
    assert!(args.contains("operation listJiraIssueTransitions"), "{args}");
    assert!(args.contains(r#"{"issueIdOrKey": "PROJ-7"}"#), "{args}");
    assert!(args.contains("__executeRead,mcp__claude_ai_Atlassian_MCP__transitionJiraIssue\n"), "{args}");
    let j = b.jobs("kind = 'jira' AND json_extract(args, '$.op') = 'transition'");
    assert_eq!(j[0].st("state"), "failed");
    assert!(j[0].st("result").contains("can't go to Done"), "{}", j[0].st("result"));

    // The owner's own list wins.
    let b = board_with(|c, d| {
        c.jira.via = "claude".into();
        c.jira.claude_tools = vec!["mcp__atlassian".into()];
        fake_claude(c, d);
    });
    let t = b.new_task(json!({"jira": {"mode": "link", "key": "PROJ-8"}}));
    b.answer(json!({"ok": true, "status": "To Do"}));
    b.tick();
    assert_eq!(b.task(t).st("jira_status"), "To Do");
    assert!(b.claude_args().contains("--allowedTools\nmcp__atlassian\n"), "{}", b.claude_args());
}

#[test]
fn a_missing_connector_says_so_instead_of_each_ops_own_error() {
    let b = board_with(|c, d| {
        c.jira.via = "claude".into();
        fake_claude(c, d);
    });
    b.answer(json!({"ok": false, "error": "The Atlassian connector tools are not available in this session"}));
    let e = jira_claude::run(&b.app, "status", &json!({"key": "PROJ-7"})).unwrap_err();
    assert!(e.starts_with("Jira through Claude can't reach the connector: The Atlassian connector tools"), "{e}");
    let args = b.claude_args();
    assert!(args.contains("Call getAccessibleAtlassianResources once"), "the check is one read: {args}");
    assert!(args.contains("--allowedTools\nmcp__claude_ai_Atlassian_MCP__getAccessibleAtlassianResources\n"), "{args}");
    assert_eq!(jira_claude::check(&b.app).unwrap_err(), e);

    // Once a call reaches it, an op's own error is just that.
    b.answer(json!({"ok": true}));
    jira_claude::check(&b.app).unwrap();
    b.answer(json!({"ok": false, "error": "PROJ-7 can't go to Done"}));
    let calls = std::fs::read_to_string(b.dir.path().join("all-args.txt")).unwrap().matches("\n-p\n").count();
    assert_eq!(jira_claude::run(&b.app, "status", &json!({"key": "PROJ-7"})).unwrap_err(), "Jira through Claude: PROJ-7 can't go to Done");
    let after = std::fs::read_to_string(b.dir.path().join("all-args.txt")).unwrap().matches("\n-p\n").count();
    assert_eq!(after, calls + 1, "no second check");
}

/// A live read through the real `claude` and the owner's connector: `TB_JIRA_LIVE_SITE=acme.atlassian.net
/// cargo test -p taskboardd --test jira -- --ignored the_connector_is_reachable`. A Claude Code change that
/// drops the connector fails here with "Jira through Claude can't reach the connector".
#[test]
#[ignore]
fn the_connector_is_reachable() {
    let site = std::env::var("TB_JIRA_LIVE_SITE").expect("set TB_JIRA_LIVE_SITE to your Jira site");
    let b = board_with(|c, _| {
        c.jira.via = "claude".into();
        c.jira.site = site;
        c.claude = "claude".into();
    });
    jira_claude::check(&b.app).unwrap();
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
fn a_reused_ticket_gets_only_the_routing_fields_it_lacks() {
    let f = json!({
        "project": {"key": "PROJ"}, "summary": "Sign in screen", "issuetype": {"name": "Task"}, "assignee": {"accountId": "me"},
        "labels": ["web", "board"], "parent": {"key": "PROJ-1"}, "customfield_10001": "team-web",
    });
    let have = json!({"labels": ["qa", "web"], "parent": null, "customfield_10001": null});
    let (update, added) = jira::fill_in(&f, &have);
    assert_eq!(update, json!({"labels": ["qa", "web", "board"], "parent": {"key": "PROJ-1"}, "customfield_10001": "team-web"}));
    assert_eq!(added, vec!["labels board", "parent PROJ-1", "customfield_10001"]);

    let complete = json!({"labels": ["web", "board"], "parent": {"key": "PROJ-7"}, "customfield_10001": "team-ios"});
    let (update, added) = jira::fill_in(&f, &complete);
    assert_eq!(update, json!({}), "a ticket with its own parent and team keeps them");
    assert!(added.is_empty());
}

#[test]
fn claude_and_the_desk_are_told_to_fill_in_a_reused_ticket_and_say_what_they_added() {
    let b = board_with(|c, d| {
        c.jira.via = "claude".into();
        c.jira.labels = vec!["board".into()];
        fake_claude(c, d);
    });
    let g = b.post("/goals", json!({"name": "Login", "project": "webapp"}))["id"].as_i64().unwrap();
    b.app.db.update("goals", &json!(g), fields!["epic_key" => "PROJ-1"]).unwrap();
    let t = b.new_task(json!({"goal_id": g, "jira": "create", "title": "Sign in screen"}));
    b.answer(json!({"ok": true, "key": "PROJ-9", "status": "To Do", "found": true, "added": ["labels board", "parent PROJ-1"]}));
    b.tick();
    let args = b.claude_args();
    assert!(args.contains("use it: don't make another."), "{args}");
    assert!(args.contains("set only what it lacks of the fields below"), "{args}");
    assert!(args.contains("never move it from a different parent"), "{args}");
    assert!(!args.contains("don't change it"), "{args}");
    let log = b.get(&format!("/tasks/T{t}/log")).to_string();
    assert!(log.contains("Linked PROJ-9 (To Do), which already covers it, and gave it labels board; parent PROJ-1"), "{log}");

    let intro = jira_desk::intro(&b.app);
    assert!(intro.contains("Give a ticket you use only what it lacks"), "{intro}");
    assert!(intro.contains("added=\"labels web; parent PROJ-1\""), "{intro}");
}

#[test]
fn the_desk_reports_what_it_gave_a_reused_ticket() {
    let b = board_with(|c, _| {
        c.jira.auto_ticket = true;
        c.jira.desk = true;
    });
    let t = b.new_task(json!({}));
    b.tick();
    let jid = b.jobs("kind = 'jira'")[0].id();
    b.post(&format!("/jira/jobs/J{jid}"), json!({"ok": true, "key": "PROJ-31", "status": "To Do", "found": true, "added": "labels web; parent PROJ-1"}));
    let job = b.jobs(&format!("id = {jid}")).remove(0);
    let result: Value = serde_json::from_str(job.s("result").unwrap()).unwrap();
    assert_eq!(result["added"], json!(["labels web", "parent PROJ-1"]));
    let log = b.get(&format!("/tasks/T{t}/log")).to_string();
    assert!(log.contains("Linked PROJ-31 (To Do), which already covers it, and gave it labels web; parent PROJ-1"), "{log}");
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
fn in_board_mode_the_qa_poll_also_reads_open_tickets_statuses() {
    let b = board_with(|c, d| {
        c.jira.via = "claude".into();
        fake_claude(c, d);
    });
    let open = b.new_task(json!({}));
    board::update_task(&b.app, open, fields!["jira_key" => "PROJ-7", "jira_status" => "In Progress"]).unwrap();
    let done = b.new_task(json!({}));
    board::update_task(&b.app, done, fields!["jira_key" => "PROJ-8", "status" => "done"]).unwrap();
    b.answer(json!({"ok": true, "comments": [], "statuses": [
        {"key": "PROJ-7", "status": "Blocked", "status_category": "indeterminate"},
        {"key": "PROJ-99", "status": "Done", "status_category": "done"},
    ]}));
    assert_eq!(taskboardd::qa::poll(&b.app).unwrap(), 0, "QA is off: statuses only");
    let args = b.claude_args();
    assert!(args.contains("key in (PROJ-7) AND updated >= -"), "open tasks' tickets only: {args}");
    assert!(args.contains("Answer statuses") && args.contains("Don't read any ticket's comments"), "{args}");
    assert_eq!(b.task(open).st("jira_status"), "Blocked");
    assert!(b.get(&format!("/tasks/T{open}/log")).to_string().contains("PROJ-7 moved to Blocked"));

    // In stream mode with a healthy jira stream, the events carry statuses and the read rests.
    let s = board_with(|c, d| {
        c.jira.via = "claude".into();
        fake_claude(c, d);
        c.stream.sources.insert("jira".into(), taskboardd::stream::SourceConfig { mode: "stream".into(), ..Default::default() });
    });
    let t = s.new_task(json!({}));
    board::update_task(&s.app, t, fields!["jira_key" => "PROJ-7"]).unwrap();
    s.app.db.set_setting("stream_feed_jira", Some(&json!({"since": taskboardd::util::iso(taskboardd::util::now_ts() - 10.0)}).to_string())).unwrap();
    assert_eq!(taskboardd::qa::poll(&s.app).unwrap(), 0);
    assert_eq!(s.claude_args(), "", "no Jira call");
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
    assert!(b.jobs("kind = 'message' AND purpose = 'jira_desk'").is_empty(), "not before its agent answered the intro");
    board::session_event(&b.app, "s9", "reply", "Ready for the first job.", None).unwrap();
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
fn the_desk_opens_once_while_midna_syncs_and_keeps_one_desk() {
    let b = board_with(|c, _| c.jira.desk = true);
    b.new_task(json!({"jira": "create"}));
    b.tick();
    let desk_jobs = || b.jobs("kind = 'agent' AND purpose = 'jira_desk'");
    assert_eq!(desk_jobs().len(), 1);

    // Midna opened it at once, but no sync has seen the terminal yet: ticks don't open another.
    b.app.db.x("UPDATE jobs SET state = 'done', target = ? WHERE id = ?", taskboardd::p![json!({"session": "s9"}).to_string(), desk_jobs()[0].id()]).unwrap();
    for _ in 0..5 {
        b.tick();
    }
    assert_eq!(desk_jobs().len(), 1, "opened once");
    assert!(b.jobs("kind = 'message' AND purpose = 'jira_desk'").is_empty());

    // Long after, with the terminal never seen, it opens a new one.
    let old = taskboardd::util::iso(taskboardd::util::now_ts() - jira_desk::SYNC_WAIT_SECS - 5.0);
    b.app.db.x("UPDATE jobs SET updated_at = ? WHERE id = ?", taskboardd::p![old, desk_jobs()[0].id()]).unwrap();
    b.tick();
    assert_eq!(desk_jobs().len(), 2);

    // Both turn up: the board keeps the first, closes the other, and sends the job once the kept one answered.
    b.app.db.x("UPDATE jobs SET state = 'done', target = ? WHERE id = ?", taskboardd::p![json!({"session": "s10"}).to_string(), desk_jobs()[1].id()]).unwrap();
    let cwd = b.dir.path().to_string_lossy().to_string();
    midna::sync(
        &b.app,
        &[
            json!({"id": "s9", "name": "TB Jira desk", "agent": "claude", "cwd": cwd, "status": {"state": "idle"}}),
            json!({"id": "s10", "name": "TB Jira desk", "agent": "claude", "cwd": cwd, "status": {"state": "idle"}}),
        ],
        &[],
    )
    .unwrap();
    b.tick();
    let closes = b.jobs("kind = 'close'");
    assert_eq!(closes.len(), 1);
    assert_eq!(board::job_args(&closes[0]).st("session"), "s10");
    assert!(b.jobs("kind = 'message' AND purpose = 'jira_desk'").is_empty());
    b.tick();
    assert_eq!(b.jobs("kind = 'close'").len(), 1, "closed once");
    assert_eq!(desk_jobs().len(), 2, "no new desk while one is open");

    board::session_event(&b.app, "s9", "reply", "Ready.", None).unwrap();
    b.tick();
    let sent = b.jobs("kind = 'message' AND purpose = 'jira_desk'");
    assert_eq!(sent.len(), 1);
    assert_eq!(board::job_args(&sent[0]).st("to"), "s9");
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
fn a_refused_desk_fails_the_jira_job_and_isn_t_opened_again_for_it() {
    let b = board_with(|c, _| {
        c.jira.desk = true;
        c.jira.auto_ticket = true;
    });
    let t = b.new_task(json!({"jira": "create"}));
    b.tick();
    let desk = b.jobs("kind = 'agent' AND purpose = 'jira_desk'");
    assert_eq!(desk.len(), 1);
    let jid = b.jobs("kind = 'jira'")[0].id();
    assert_eq!(board::job_target(&desk[0]).i("jira_job"), Some(jid), "the desk's open carries the job that needs it");

    taskboardd::jobs::result(&b.app, desk[0].id(), false, "", "denied by policy rule r_probe", 1).unwrap();
    let job = b.jobs("kind = 'jira'").remove(0);
    assert_eq!(job.st("state"), "failed");
    let log = b.get(&format!("/tasks/T{t}/log")).to_string();
    assert!(log.contains("Midna refused: denied by policy rule r_probe"), "{log}");
    assert!(taskboardd::dispatch::alerts(&b.app).iter().any(|a| a["task_id"] == t), "the usual ticket-failure alert");
    for _ in 0..5 {
        b.tick();
    }
    assert_eq!(b.jobs("kind = 'agent' AND purpose = 'jira_desk'").len(), 1, "not opened again for that job");
}

#[test]
fn a_task_never_goes_to_the_jira_desk() {
    let b = board_with(|c, _| c.jira.desk = true);
    b.new_task(json!({"jira": "create"}));
    b.tick();
    let desk = b.jobs("kind = 'agent' AND purpose = 'jira_desk'");
    b.app.db.x("UPDATE jobs SET state = 'done', target = ? WHERE id = ?", taskboardd::p![json!({"session": "s9"}).to_string(), desk[0].id()]).unwrap();
    let cwd = b.dir.path().join("webapp").to_string_lossy().to_string();
    midna::sync(
        &b.app,
        &[
            json!({"id": "s9", "name": "TB Jira desk", "agent": "claude", "cwd": cwd, "status": {"state": "idle"}}),
            json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": cwd, "status": {"state": "idle"}}),
        ],
        &[],
    )
    .unwrap();
    let t = b.new_task(json!({"jira": "none"}));
    let attachable: Vec<String> = b.get(&format!("/tasks/T{t}"))["attachable"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap().to_string()).collect();
    assert!(attachable.contains(&"s1".to_string()), "{attachable:?}");
    assert!(!attachable.contains(&"s9".to_string()), "the desk isn't offered: {attachable:?}");

    let mut from_app = Query::new();
    from_app.insert(api::FROM.into(), "app".into());
    let e = api::dispatch(&b.app, "POST", &format!("/tasks/T{t}/start"), &from_app, &json!({"mode": "attach", "session_id": "s9"})).expect_err("refused");
    assert_eq!((e.status, e.message.as_str()), (409, "Tasks can't go to the Jira desk."));
    assert_ne!(b.task(t).st("status"), "working");
    let (code, _) = b.post_err("/tasks", json!({"title": "On the desk", "detail": "d", "project": "webapp", "jira": "none", "pickup": {"mode": "attach", "session_id": "s9"}}));
    assert_eq!(code, 409);
}

#[test]
fn a_backlog_tickets_failure_names_the_issue_and_the_retry() {
    let b = board_with(|c, _| c.jira.desk = true);
    let i = b.post("/backlog", json!({"title": "Totals round wrong", "project": "webapp"}))["id"].as_i64().unwrap();
    b.post(&format!("/backlog/B{i}/ticket"), json!({}));
    let jid = b.jobs("kind = 'jira'")[0].id();
    b.post(&format!("/jira/jobs/J{jid}"), json!({"ok": false, "message": "PROJ needs a team"}));
    let alerts = taskboardd::dispatch::alerts(&b.app);
    let text = alerts.iter().filter_map(|a| a["text"].as_str()).find(|t| t.contains("Totals round wrong")).expect("an alert").to_string();
    assert_eq!(text, format!("Couldn't make the Jira ticket for B{i} “Totals round wrong”: PROJ needs a team. Try again with tb backlog ticket B{i}."));
    // The retry it names works: the issue is open again.
    b.post(&format!("/backlog/B{i}/ticket"), json!({}));
}

#[test]
fn an_epic_is_searched_by_the_words_its_title_leads_with() {
    let jql = jira::summary_jql("Zebra checkout redesign for the mobile app with apple pay and wallet support plus coupons", true).unwrap();
    assert!(jql.starts_with(r#"(summary ~ "zebra" OR summary ~ "checkout" OR summary ~ "redesign""#), "{jql}");
    assert!(!jql.contains("coupons"), "only the first 8: {jql}");
    assert_eq!(jira::summary_jql("Fix the login", false).as_deref(), Some(r#"summary ~ "fix login""#));
    assert!(jira::summary_jql("the and", true).is_none());
}

#[test]
fn the_desk_runs_a_bare_tb_its_allow_rule_covers() {
    let path = "/x/cache/taskboard/task-board/0.1.0/bin/tb";
    let b = board_with(|c, _| c.jira.desk = true);
    b.app.db.set_setting("tb_path", Some(path)).unwrap();
    b.new_task(json!({"jira": "create"}));
    b.tick();
    let opened = b.jobs("kind = 'agent' AND purpose = 'jira_desk'");
    let a = board::job_args(&opened[0]);
    let (args, _) = midna::claude_args(&a).unwrap();
    assert_eq!(args[0], "--allowedTools");
    let rules: Vec<&str> = args[1].split(',').collect();
    let server = |n: &str| format!("mcp__claude_ai_Atlassian_MCP__{n}");
    let want = [
        server("getJiraIssue"),
        server("executeRead"),
        server("searchJiraIssuesUsingJql"),
        server("createJiraIssue"),
        server("getAccessibleAtlassianResources"),
        server("discover"),
        "Bash(tb jira:*)".to_string(),
    ];
    assert_eq!(rules, want.iter().map(String::as_str).collect::<Vec<_>>());
    assert!(
        a.st("prompt").contains("Jira: site acme.atlassian.net, project PROJ; pass cloudId the cloud id of acme.atlassian.net (the site's host name works as one)"),
        "{}",
        a.st("prompt")
    );

    // Every report it's told to run starts with a command an allow rule covers.
    let prefixes: Vec<&str> = rules.iter().filter_map(|r| r.strip_prefix("Bash(")?.strip_suffix(":*)")).collect();
    let intro = a.st("prompt");
    b.app.db.x("UPDATE jobs SET state = 'done', target = ? WHERE id = ?", taskboardd::p![json!({"session": "s9"}).to_string(), opened[0].id()]).unwrap();
    midna::sync(&b.app, &[json!({"id": "s9", "name": "TB Jira desk", "agent": "claude", "cwd": b.dir.path().to_string_lossy(), "status": {"state": "idle"}})], &[]).unwrap();
    board::session_event(&b.app, "s9", "reply", "Ready for the first job.", None).unwrap();
    b.tick();
    let job = board::job_args(&b.jobs("kind = 'message' AND purpose = 'jira_desk'")[0]).st("text");
    for text in [intro.as_str(), job.as_str()] {
        assert!(!text.contains(path), "the versioned plugin path stays out of agent text: {text}");
        let runs: Vec<&str> = text.match_indices("tb jira").map(|(i, _)| &text[i..]).collect();
        assert!(!runs.is_empty(), "{text}");
        for r in runs {
            assert!(prefixes.iter().any(|p| r.starts_with(p)), "{r}");
        }
    }

    // The owner's own list is the desk's as it is.
    let b = board_with(|c, _| {
        c.jira.desk = true;
        c.jira.claude_tools = vec!["mcp__atlassian".into()];
    });
    let tools = jira_desk::allowed_tools(&b.app);
    assert_eq!(tools[0], "mcp__atlassian");
    assert!(tools[1..].iter().all(|t| t.starts_with("Bash(")), "{tools:?}");
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
    // Before the goal runs every mate is planned, and they're still listed.
    let h0 = handoff::build(&b.app, t2).unwrap();
    assert!(h0.contains(&format!("- T{t1} “Sign-in form” (planned, owns src/form.rs, src/form.css)")), "{h0}");
    assert!(!h0.contains("Nothing else runs in this wave."), "{h0}");
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

// --- tickets and PRs ---

fn report(b: &Board, event: &str, sid: &str, extra: Value, spooled: bool) -> Result<Value, String> {
    let mut body = json!({"event": event, "session": sid, "claude_session": format!("c-{sid}"), "cwd": "", "git": {}});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    taskboardd::reports::handle(&b.app, body, spooled).map_err(|e| e.message)
}

fn on_terminal(b: &Board, sid: &str, t: i64) {
    let repo = b.dir.path().join("webapp").to_string_lossy().to_string();
    midna::sync(&b.app, &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
    report(b, "tb.take", sid, json!({"task": format!("T{t}")}), false).unwrap();
}

fn jira_args(j: &Row) -> Value {
    serde_json::from_str(j.s("args").unwrap_or("{}")).unwrap()
}

#[test]
fn a_pr_needs_a_ticket_while_jira_is_on() {
    let b = board_with(|_, _| {});
    let t = b.new_task(json!({}));
    on_terminal(&b, "s1", t);
    let pr = "https://github.com/acme/webapp/pull/9";
    let e = report(&b, "tb.done", "s1", json!({"summary": "Done", "pr": pr}), false).unwrap_err();
    assert!(e.starts_with(&format!("Every PR needs a Jira ticket, and T{t} has none.")), "{e}");
    assert!(e.contains(&format!("tb task set T{t} --jira ABC-123")), "{e}");
    assert_eq!(b.task(t).s("status"), Some("working"));

    report(&b, "tb.done", "s1", json!({"summary": "Nothing to change", "no_pr": "already fixed"}), false).unwrap();
    assert_eq!(b.task(t).s("status"), Some("done"), "no PR, no ticket needed");

    let spooled = b.new_task(json!({}));
    on_terminal(&b, "s2", spooled);
    report(&b, "tb.done", "s2", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/10"}), true).unwrap();
    assert_eq!(b.task(spooled).s("status"), Some("done"), "a spooled replay isn't refused");

    let linked = b.new_task(json!({"jira": {"mode": "link", "key": "PROJ-7"}}));
    on_terminal(&b, "s3", linked);
    report(&b, "tb.done", "s3", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/11"}), false).unwrap();
    assert_eq!(b.task(linked).s("status"), Some("done"));

    let optout = board_with(|c, _| c.pr_body.require_ticket = Some(false));
    let t = optout.new_task(json!({}));
    on_terminal(&optout, "s1", t);
    report(&optout, "tb.done", "s1", json!({"summary": "Done", "pr": pr}), false).unwrap();
}

#[test]
fn requeue_clears_a_canceled_pr_so_the_next_pr_doesn_t_withdraw_the_ticket() {
    let b = board_with(|c, _| c.jira.canceled = "Withdrawn".into());
    let t = b.new_task(json!({"jira": {"mode": "link", "key": "PROJ-3"}}));
    on_terminal(&b, "s1", t);
    report(&b, "tb.done", "s1", json!({"summary": "Not needed", "no_pr": "already fixed"}), false).unwrap();
    assert!(b.task(t).s("no_pr").is_some());
    b.post(&format!("tasks/T{t}/requeue"), json!({}));
    assert!(b.task(t).s("no_pr").is_none(), "queueing again clears the canceled PR");

    b.app.db.x("UPDATE tasks SET no_pr = 'stale', pr_num = 9, pr_url = 'https://github.com/acme/webapp/pull/9' WHERE id = ?", taskboardd::p![t]).unwrap();
    let withdrawn = || b.jobs("kind = 'jira' AND json_extract(args, '$.status') = 'Withdrawn'").len();
    let before = withdrawn();
    b.app.db.tx(|| taskboardd::reports::finish_task(&b.app, &b.task(t), "Pat", "Done", false, None).map(|_| ())).unwrap();
    assert_eq!(withdrawn(), before, "a task with a PR isn't withdrawn");
}

#[test]
fn a_project_without_prs_refuses_ticket_epic_and_pr_asks() {
    let b = board_with(|_, _| {});
    let t = b.new_task(json!({}));
    projects::set_pr_flow(&b.app, "webapp", "off").unwrap();
    let flow_on = "To change that: tb project set webapp --pr-flow on";
    let (code, e) = b.post_err(&format!("tasks/T{t}/jira"), json!({"op": "create"}));
    assert_eq!((code, e.contains(flow_on)), (409, true), "{e}");
    let (code, e) = b.post_err(&format!("tasks/T{t}"), json!({"jira_key": "new"}));
    assert_eq!((code, e.contains(flow_on)), (409, true), "{e}");
    let (code, e) = b.post_err(&format!("tasks/T{t}"), json!({"ships_pr": "yes"}));
    assert_eq!((code, e.contains(flow_on)), (409, true), "{e}");
    let (code, e) = b.post_err("/tasks", json!({"title": "Add login", "project": "webapp", "ships_pr": true}));
    assert_eq!((code, e.contains(flow_on)), (409, true), "{e}");
    let (code, e) = b.post_err("/goals", json!({"name": "Login", "project": "webapp", "epic": "create"}));
    assert_eq!((code, e.contains(flow_on)), (409, true), "{e}");

    let quiet = b.new_task(json!({"jira": "create"}));
    assert!(b.task(quiet).b("jira_none"), "a new task's ticket ask is quietly dropped");
    assert!(b.jobs("kind = 'jira'").is_empty());

    let g = b.post("/goals", json!({"name": "Login", "project": "webapp"}))["id"].as_i64().unwrap();
    jira::ensure_epic(&b.app, Some(&board::get_goal(&b.app, g).unwrap())).unwrap();
    assert!(b.jobs("kind = 'jira'").is_empty(), "no epic for a project without PRs");
}

#[test]
fn turning_the_pr_flow_off_drops_pending_ticket_asks() {
    let b = board_with(|_, _| {});
    let t = b.new_task(json!({"jira": "create"}));
    assert_eq!(b.jobs(&format!("kind = 'jira' AND state = 'pending' AND task_id = {t}")).len(), 1);
    projects::set_pr_flow(&b.app, "webapp", "off").unwrap();
    assert!(b.jobs("kind = 'jira' AND state = 'pending'").is_empty());
}

#[test]
fn a_merge_comments_which_pr_merged_on_the_ticket() {
    let b = board_with(|c, _| c.jira.merged = "Ready for QA".into());
    let t = b.new_task(json!({"jira": {"mode": "link", "key": "PROJ-4"}}));
    b.app.db.x("UPDATE tasks SET pr_num = 9, pr_url = 'https://github.com/acme/webapp/pull/9' WHERE id = ?", taskboardd::p![t]).unwrap();
    b.app.db.tx(|| taskboardd::prflow::mark_merged(&b.app, &b.task(t), "Pat")).unwrap();
    let moves = b.jobs("kind = 'jira' AND json_extract(args, '$.status') = 'Ready for QA'");
    assert_eq!(moves.len(), 1);
    assert_eq!(jira_args(&moves[0])["comment"], "PR #9 was merged: https://github.com/acme/webapp/pull/9");
}

#[test]
fn the_board_s_own_ticket_create_doesn_t_clear_a_task_s_alert() {
    let b = board_with(|c, _| c.jira.auto_ticket = true);
    let t = b.new_task(json!({}));
    b.app.db.x("DELETE FROM jobs WHERE kind = 'jira'", vec![]).unwrap();
    board::update_task(&b.app, t, fields!["status" => "needs", "question" => "Which API key?"]).unwrap();
    taskboardd::dispatch::add_alert(&b.app, "T1 needs you: Which API key?", Some(t), None, None, None).unwrap();
    let task_alerts = || taskboardd::dispatch::alerts(&b.app).into_iter().filter(|a| a["task_id"] == t).count();
    assert_eq!(task_alerts(), 1);

    jira::ensure_tickets(&b.app).unwrap();
    let create = b.jobs("kind = 'jira'").pop().expect("the board asked for a ticket");
    assert!(create.s("args").unwrap().contains("\"op\":\"create\""), "{:?}", create.s("args"));
    jira::finish(&b.app, create.id(), true, Some("PROJ-9"), None, None).unwrap();
    b.app.db.x("UPDATE events SET at = '2099-01-01T00:00:00Z' WHERE task_id = ? AND kind = 'jira'", taskboardd::p![t]).unwrap();
    taskboardd::dispatch::prune_alerts(&b.app).unwrap();
    assert_eq!(task_alerts(), 1, "the board's own ticket coming back isn't the agent moving on");
}

#[test]
fn the_board_s_own_jira_move_doesn_t_clear_a_task_s_alert() {
    let b = board_with(|_, _| {});
    let t = b.new_task(json!({}));
    board::update_task(&b.app, t, fields!["status" => "working", "jira_key" => "PROJ-1", "jira_sync" => 1, "jira_status" => "In Progress"]).unwrap();
    taskboardd::dispatch::add_alert(&b.app, "T1 looks stuck", Some(t), None, None, None).unwrap();
    let task_alerts = || taskboardd::dispatch::alerts(&b.app).into_iter().filter(|a| a["task_id"] == t).count();
    assert_eq!(task_alerts(), 1);

    let jid = board::jira_keep_in_step(&b.app, &b.task(t), Some("In Code Review"), None).unwrap().expect("a move");
    jira::finish(&b.app, jid, true, None, Some("In Code Review"), None).unwrap();
    b.app.db.x("UPDATE events SET at = '2099-01-01T00:00:00Z' WHERE task_id = ? AND kind = 'jira'", taskboardd::p![t]).unwrap();
    taskboardd::dispatch::prune_alerts(&b.app).unwrap();
    assert_eq!(task_alerts(), 1, "the board moving the ticket isn't the agent moving on");

    board::log_event(&b.app, t, "Term", "note", "Pushed the fix").unwrap();
    b.app.db.x("UPDATE events SET at = '2099-01-01T00:00:01Z' WHERE task_id = ? AND who = 'Term'", taskboardd::p![t]).unwrap();
    taskboardd::dispatch::prune_alerts(&b.app).unwrap();
    assert_eq!(task_alerts(), 0, "the agent's own line clears it");
}

// --- ticket_template and ticket_guide ---

#[test]
fn a_ticket_is_made_from_its_own_text_in_the_template() {
    let b = board_with(|c, d| {
        std::fs::write(d.join("tpl.md"), "## Why\n{description}\n\n## Board\n{task} in {project}{goal}").unwrap();
        c.jira.ticket_template = d.join("tpl.md").to_string_lossy().into();
    });
    let id = b.new_task(json!({"detail": "the detail", "jira": {"mode": "create"}, "ticket_summary": "[Web] Add login", "ticket_text": "- users sign in\n- with SSO"}));
    let job = b.jobs("kind = 'jira'").pop().unwrap();
    let a = board::job_args(&job);
    assert_eq!(a.st("summary"), "[Web] Add login");
    assert_eq!(a.st("description"), format!("## Why\n- users sign in\n- with SSO\n\n## Board\nT{id} in webapp"));
    let (f, _) = jira::ticket_fields(&b.app, &a, None);
    assert_eq!(f["summary"], "[Web] Add login");
    assert_eq!(f["description"]["content"][0]["type"], "heading");
    assert_eq!(f["description"]["content"][1]["type"], "bulletList");

    // Without ticket text, the title and detail stand in.
    let id2 = b.new_task(json!({"title": "Fix logout", "detail": "it hangs", "jira": {"mode": "create"}}));
    let a = board::job_args(&b.jobs("kind = 'jira'").pop().unwrap());
    assert_eq!(a.st("summary"), "Fix logout");
    assert_eq!(a.st("description"), format!("## Why\nit hangs\n\n## Board\nT{id2} in webapp"));
}

#[test]
fn an_unreadable_template_leaves_the_text_as_it_is() {
    let b = board_with(|c, _| c.jira.ticket_template = "/nowhere/tpl.md".into());
    b.new_task(json!({"detail": "the detail", "jira": {"mode": "create"}}));
    assert_eq!(board::job_args(&b.jobs("kind = 'jira'").pop().unwrap()).st("description"), "the detail");
    assert!(b.get("/jira")["ticket_template"]["error"].as_str().unwrap().contains("can't read /nowhere/tpl.md"));
}

#[test]
fn with_a_guide_an_agent_writes_the_ticket_text() {
    let b = board_with(|c, d| {
        std::fs::write(d.join("guide.md"), "Start with a user story.").unwrap();
        c.jira.ticket_guide = d.join("guide.md").to_string_lossy().into();
    });
    assert_eq!(b.get("/jira")["ticket_guide"]["text"], "Start with a user story.");
    let body = json!({"title": "Add login", "project": "webapp", "jira": {"mode": "create"}});
    let e = b.app.db.tx(|| ops::new_task(&b.app, &body, "agent-1", None)).unwrap_err();
    assert!(e.message.contains("tb jira"), "{}", e.message);
    assert!(b.jobs("kind = 'jira'").is_empty());

    let id = b.new_task(json!({}));
    let (code, msg) = b.post_err(&format!("/tasks/{id}"), json!({"jira_key": "new", "who": "agent-1"}));
    assert_eq!((code, msg.contains("--ticket-text")), (400, true));
    b.post(&format!("/tasks/{id}"), json!({"jira_key": "new", "who": "agent-1", "ticket_text": "As a user, I sign in."}));
    assert_eq!(board::job_args(&b.jobs("kind = 'jira'").pop().unwrap()).st("description"), "As a user, I sign in.");
    assert_eq!(b.task(id).st("ticket_text"), "As a user, I sign in.");
}
