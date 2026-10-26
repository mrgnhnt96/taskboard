//! `[jira] merged_note` (#327): the agent's note for testers, posted on the ticket before it moves to merged.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, dispatch, fields, jira, merged_note, midna, p, prflow, projects, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.jira.site = "acme.atlassian.net".into();
    cfg.jira.project = "PROJ".into();
    cfg.jira.merged = "Ready for QA".into();
    cfg.jira.merged_note = true;
    cfg.jira.via = "claude".into();
    let claude = dir.path().join("fake-claude");
    std::fs::write(&claude, "#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s\\n' \"$@\" >> \"$d/all-args.txt\"\ncat \"$d/answer.json\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.claude = claude.to_string_lossy().to_string();
    f(&mut cfg);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    projects::set_pr_flow(&app, "webapp", "on").unwrap();
    let b = Board { app, dir };
    b.answer(json!({"ok": true, "status": "Ready for QA"}));
    b
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn task(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    /// A task on ticket PROJ-4, taken in terminal s1.
    fn ticketed(&self) -> i64 {
        let t = self.post("/tasks", json!({"title": "Add login", "project": "webapp", "jira": {"mode": "link", "key": "PROJ-4"}}))["id"].as_i64().unwrap();
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", json!({"task": format!("T{t}")})).unwrap();
        self.app.db.x("DELETE FROM jobs WHERE kind = 'jira'", vec![]).unwrap();
        t
    }
    fn merge(&self, t: i64) {
        self.app.db.x("UPDATE tasks SET pr_num = 9, pr_url = 'https://github.com/acme/webapp/pull/9' WHERE id = ?", p![t]).unwrap();
        self.app.db.tx(|| prflow::mark_merged(&self.app, &self.task(t), "Pat")).unwrap();
    }
    fn note_jobs(&self) -> Vec<Row> {
        self.app.db.q("SELECT * FROM jobs WHERE kind = 'jira' AND json_extract(args, '$.op') = 'merged_note' ORDER BY id", vec![]).unwrap()
    }
    fn answer(&self, v: Value) {
        std::fs::write(self.dir.path().join("answer.json"), json!({"structured_output": v}).to_string()).unwrap();
    }
    fn claude_calls(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("all-args.txt")).unwrap_or_default()
    }
    fn run_jira(&self) {
        self.app.db.tx(|| merged_note::tick(&self.app)).unwrap();
        jira::run_pending(&self.app).unwrap();
    }
    fn alert_for(&self, t: i64) -> Option<String> {
        dispatch::alerts(&self.app).into_iter().find(|a| a["key"] == format!("merged-note:{t}")).and_then(|a| a["text"].as_str().map(|s| s.to_string()))
    }
}

#[test]
fn tb_done_refuses_a_ticketed_pr_task_without_a_note() {
    let b = board_with(|_| {});
    let t = b.ticketed();
    let pr = "https://github.com/acme/webapp/pull/9";
    let (code, why) = b.report("tb.done", json!({"summary": "Added login", "pr": pr})).unwrap_err();
    assert_eq!(code, 409, "{why}");
    assert!(why.contains("note for testers") && why.contains("tb jira") && why.contains("--merged-note"), "{why}");

    b.report("tb.done", json!({"summary": "Added login", "pr": pr, "merged_note": "## What changed\nLogin."})).unwrap();
    assert_eq!(b.task(t).st("status"), "done");
    assert_eq!(b.task(t).st("merged_note"), "## What changed\nLogin.");
}

#[test]
fn no_pr_and_tasks_without_a_ticket_aren_t_asked_for_a_note() {
    let b = board_with(|_| {});
    b.ticketed();
    b.report("tb.done", json!({"summary": "Not needed", "no_pr": "T3 did it"})).unwrap();

    let b = board_with(|c| c.pr_body.require_ticket = Some(false));
    let t = b.ticketed();
    b.post(&format!("/tasks/T{t}"), json!({"jira_key": "none"}));
    b.report("tb.done", json!({"summary": "Added login", "pr": "https://github.com/acme/webapp/pull/9"})).unwrap();

    let b = board_with(|c| c.jira.merged_note = false);
    b.ticketed();
    b.report("tb.done", json!({"summary": "Added login", "pr": "https://github.com/acme/webapp/pull/9"})).unwrap();
}

#[test]
fn at_merge_the_note_then_the_merged_line_post_before_the_move() {
    let b = board_with(|_| {});
    let t = b.ticketed();
    b.post(&format!("/tasks/T{t}"), json!({"merged_note": "1. What changed: login"}));
    b.merge(t);
    assert!(b.app.db.q("SELECT * FROM jobs WHERE kind = 'jira' AND json_extract(args, '$.op') = 'transition'", vec![]).unwrap().is_empty());
    assert_eq!(b.note_jobs().len(), 1);
    b.run_jira();

    let calls = b.claude_calls();
    let note = calls.find("1. What changed: login").expect("the note was posted");
    let line = calls.find("PR #9 was merged: https://github.com/acme/webapp/pull/9").expect("the merged line was posted");
    let mv = calls.find("Move PROJ-4 to the status “Ready for QA”").expect("the ticket moved");
    assert!(note < line && line < mv, "{calls}");
    let tk = b.task(t);
    assert_eq!(tk.st("jira_status"), "Ready for QA");
    assert!(tk.s("merged_note_wait").is_none());
    assert_eq!(b.note_jobs()[0].st("state"), "done");
}

#[test]
fn a_failed_post_holds_the_ticket_alerts_and_retries_without_posting_twice() {
    let b = board_with(|_| {});
    let t = b.ticketed();
    b.post(&format!("/tasks/T{t}"), json!({"merged_note": "1. What changed: login"}));
    b.merge(t);
    b.answer(json!({"ok": false, "error": "Jira is down"}));
    b.run_jira();
    assert_eq!(b.note_jobs()[0].st("state"), "failed");
    assert_ne!(b.task(t).st("jira_status"), "Ready for QA");
    let alert = b.alert_for(t).expect("an alert");
    assert!(alert.contains(&format!("T{t}")) && alert.contains("Jira is down"), "{alert}");

    b.run_jira();
    assert_eq!(b.note_jobs().len(), 1, "not again right away");

    b.app.db.x("UPDATE jobs SET updated_at = '2020-01-01T00:00:00Z' WHERE kind = 'jira'", vec![]).unwrap();
    board::update_task(&b.app, t, fields!["merged_note_posted" => "note"]).unwrap();
    b.answer(json!({"ok": true, "status": "Ready for QA"}));
    std::fs::remove_file(b.dir.path().join("all-args.txt")).ok();
    b.run_jira();
    assert_eq!(b.note_jobs().len(), 2);
    let calls = b.claude_calls();
    assert!(!calls.contains("1. What changed: login"), "the posted note isn't posted again: {calls}");
    assert!(calls.contains("PR #9 was merged"), "{calls}");
    assert_eq!(b.task(t).st("jira_status"), "Ready for QA");
    assert!(b.alert_for(t).is_none(), "the alert clears once it moved");
}

#[test]
fn a_merge_without_a_note_holds_the_ticket_until_one_is_written() {
    let b = board_with(|_| {});
    let t = b.ticketed();
    b.merge(t);
    assert!(b.note_jobs().is_empty());
    assert!(b.app.db.q("SELECT * FROM jobs WHERE kind = 'jira'", vec![]).unwrap().is_empty(), "no move");
    let alert = b.alert_for(t).expect("an alert");
    assert!(alert.contains(&format!("T{t}")) && alert.contains("--merged-note"), "{alert}");
    let card = board::task_card(&b.app, &b.task(t)).unwrap();
    assert_eq!(card["merged_note"]["waiting"], true);

    b.post(&format!("/tasks/T{t}"), json!({"merged_note": "Nothing to test: a refactor."}));
    assert_eq!(b.note_jobs().len(), 1, "the note sends it on");
    assert!(b.alert_for(t).is_none());
    b.run_jira();
    assert_eq!(b.task(t).st("jira_status"), "Ready for QA");
    assert!(b.claude_calls().contains("Nothing to test: a refactor."));
}

#[test]
fn a_note_ask_that_cant_wake_yet_is_sent_once_it_can() {
    let b = board_with(|_| {});
    let t = b.ticketed();
    let asks = || b.app.db.count("SELECT COUNT(*) FROM jobs WHERE purpose = 'pr' AND json_extract(args, '$.title') LIKE 'Note for testers%'", p![]).unwrap();
    // A PR visit still under way when the PR merges: the ask can't come in yet.
    b.app.db.insert("jobs", fields!["kind" => "agent", "purpose" => "pr", "task_id" => t, "state" => "pending", "args" => "{}"]).unwrap();
    b.merge(t);
    assert_eq!(asks(), 0);
    b.run_jira();
    assert_eq!(asks(), 0);
    b.app.db.x("UPDATE jobs SET state = 'done' WHERE purpose = 'pr'", vec![]).unwrap();
    b.run_jira();
    assert_eq!(asks(), 1, "the sweep asks once the visit ends");
    b.app.db.x("UPDATE jobs SET state = 'done' WHERE purpose = 'pr'", vec![]).unwrap();
    b.run_jira();
    assert_eq!(asks(), 1, "and only once");
}

#[test]
fn tb_jira_shows_the_note_guide() {
    let b = board_with(|_| {});
    let guide = b.dir.path().join("note.md");
    std::fs::write(&guide, "1. What changed").unwrap();
    let b2 = board_with(|c| c.jira.merged_note_guide = guide.to_string_lossy().into());
    let j = b2.get("/jira");
    assert_eq!(j["merged_note"], true);
    assert_eq!(j["merged_note_guide"]["text"], "1. What changed");
    assert!(b.get("/jira")["merged_note_guide"].is_null());
}

#[test]
fn the_handoff_tells_the_agent_about_the_note() {
    let b = board_with(|_| {});
    let t = b.ticketed();
    let lines = jira::handoff_block(&b.app, &b.task(t)).unwrap().join("\n");
    assert!(lines.contains("note for testers") && lines.contains("--merged-note"), "{lines}");
    let off = board_with(|c| c.jira.merged_note = false);
    let t = off.ticketed();
    assert!(!jira::handoff_block(&off.app, &off.task(t)).unwrap().join("\n").contains("note for testers"));
}

/// A ticketed task with its note written and an open PR, stacked on `parent` with `mode` when given.
fn stacked(b: &Board, key: &str, pr: i64, on: Option<(i64, &str)>) -> i64 {
    let t = b.post("/tasks", json!({"title": format!("Work for {key}"), "project": "webapp", "jira": {"mode": "link", "key": key}}))["id"].as_i64().unwrap();
    b.post(&format!("/tasks/T{t}"), json!({"merged_note": format!("Test {key}")}));
    b.app.db.x("UPDATE tasks SET status = 'done', pr_num = ?, pr_url = ? WHERE id = ?", p![pr, format!("https://github.com/acme/webapp/pull/{pr}"), t]).unwrap();
    if let Some((parent, mode)) = on {
        board::update_task(&b.app, t, fields!["pr_after" => parent, "stack_merge" => if mode == "head" { json!("head") } else { Value::Null }]).unwrap();
    }
    t
}

fn mark_merged(b: &Board, t: i64) {
    b.app.db.tx(|| prflow::mark_merged(&b.app, &b.task(t), "Pat")).unwrap();
}

fn note_keys(b: &Board) -> Vec<String> {
    b.note_jobs().iter().map(|j| board::job_args(j).st("key")).collect()
}

#[test]
fn a_head_merged_ticket_moves_once_its_stack_reaches_main() {
    let b = board_with(|_| {});
    let bottom = stacked(&b, "PROJ-4", 9, None);
    let mid = stacked(&b, "PROJ-5", 10, Some((bottom, "head")));
    let top = stacked(&b, "PROJ-6", 11, Some((mid, "head")));
    b.app.db.x("DELETE FROM jobs WHERE kind = 'jira'", vec![]).unwrap();

    mark_merged(&b, top);
    mark_merged(&b, mid);
    assert!(note_keys(&b).is_empty(), "merged into its parent's branch, not main");
    assert!(b.task(mid).s("merged_note_wait").is_none());

    mark_merged(&b, bottom);
    assert_eq!(note_keys(&b), vec!["PROJ-4", "PROJ-5", "PROJ-6"]);
    let comments: Vec<String> = b.note_jobs().iter().map(|j| board::job_args(j).st("comment")).collect();
    assert_eq!(comments[2], "PR #11 was merged: https://github.com/acme/webapp/pull/11");
    assert!(b.task(top).s("merged_note_wait").is_some());
}

#[test]
fn a_tail_merged_ticket_moves_when_its_pr_merges() {
    let b = board_with(|_| {});
    let bottom = stacked(&b, "PROJ-4", 9, None);
    let child = stacked(&b, "PROJ-5", 10, Some((bottom, "tail")));
    b.app.db.x("DELETE FROM jobs WHERE kind = 'jira'", vec![]).unwrap();
    mark_merged(&b, bottom);
    assert_eq!(note_keys(&b), vec!["PROJ-4"]);
    mark_merged(&b, child);
    assert_eq!(note_keys(&b), vec!["PROJ-4", "PROJ-5"]);
}
