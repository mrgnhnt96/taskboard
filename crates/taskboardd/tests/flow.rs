//! End-to-end flows through the board's own functions, on an in-memory database.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, jobs, midna, p, prflow, reports, runner};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    f(&mut cfg);
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, _dir: dir }
}

fn new_board() -> Board {
    board_with(|_| {})
}

impl Board {
    fn get(&self, path: &str) -> Value {
        let (p, q) = path.split_once('?').unwrap_or((path, ""));
        let query: Query = q.split('&').filter(|x| !x.is_empty()).filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
        api::dispatch(&self.app, "GET", p, &query, &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn post_err(&self, path: &str, body: Value) -> (u16, String) {
        let e = api::dispatch(&self.app, "POST", path, &Query::new(), &body).expect_err("should fail");
        (e.status, e.message)
    }
    fn report(&self, event: &str, session: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {"branch": "feature/x", "commit": "Add x", "sha": "abc123", "uncommitted": 1}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message))
    }
    fn task(&self, id: i64) -> taskboardd::util::Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn jobs(&self, kind: &str) -> Vec<taskboardd::util::Row> {
        self.app.db.q("SELECT * FROM jobs WHERE kind = ? ORDER BY id", p![kind]).unwrap()
    }
    fn add_session(&self, sid: &str, status: &str) {
        let repo = self._dir.path().join("webapp");
        midna::sync(
            &self.app,
            &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": status}})],
            &[],
        )
        .unwrap();
    }
}

fn new_task(b: &Board, title: &str, extra: Value) -> i64 {
    let mut body = json!({"title": title, "detail": "Do the thing.", "project": "webapp"});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    b.post("/tasks", body)["id"].as_i64().unwrap()
}

#[test]
fn a_task_starts_is_claimed_reports_and_finishes() {
    let b = new_board();
    let id = new_task(&b, "Add login", json!({}));
    assert_eq!(b.task(id).s("status"), Some("queued"));

    let started = runner::start_queued(&b.app).unwrap();
    assert_eq!(started, vec![id]);
    let job = &b.jobs("agent")[0];
    let args = board::job_args(job);
    assert!(args.st("prompt").starts_with("[task-board:T1] You are picking up “Add login” in webapp"));
    assert!(args.st("prompt").contains("What to do:\nDo the thing."));
    assert_eq!(b.task(id).i("start_job"), Some(job.id()));
    assert!(runner::start_queued(&b.app).unwrap().is_empty(), "a task with a live start job isn't started twice");

    b.add_session("s1", "working");
    let prompt = args.st("prompt");
    let out = b.report("hook.prompt", "s1", json!({"prompt": prompt}));
    assert_eq!(out["task"], "T1");
    assert!(out.get("context").is_none(), "the full handoff was already in the prompt");
    let t = b.task(id);
    assert_eq!(t.s("status"), Some("working"));
    assert_eq!(t.s("session_id"), Some("s1"));
    assert_eq!(t.s("claude_session_id"), Some("c-s1"));

    b.report("tb.checkpoint", "s1", json!({"done": ["Wrote the form"], "next": ["Wire the API"], "decisions": ["Use fetch"]}));
    let ctx = board::task_context(&b.task(id));
    assert_eq!(ctx["done"], json!(["Wrote the form"]));
    assert_eq!(ctx["where"]["branch"], "feature/x");
    assert_eq!(b.task(id).s("latest"), Some("Wrote the form. Next: Wire the API."));

    let found = b.report("tb.found", "s1", json!({"title": "Flaky date test", "kind": "gap"}));
    assert_eq!(found["issue"], "B1");
    let again = b.report("tb.found", "s1", json!({"title": "flaky  date test!", "kind": "gap"}));
    assert_eq!(again["seen"], true, "the same title merges into the open issue");

    b.report("hook.stop", "s1", json!({"last_message": "I wired the API. Next I test it."}));
    assert_eq!(b.task(id).s("latest"), Some("I wired the API."));

    let handoff = b.get("/tasks/T1/handoff")["text"].as_str().unwrap().to_string();
    assert!(handoff.contains("Done: Wrote the form."));
    assert!(handoff.contains("Keep: Use fetch."));
    assert!(handoff.contains("Branch: feature/x"));

    let done = b.report("tb.done", "s1", json!({"summary": "Login works. PR: https://github.com/acme/webapp/pull/7"}));
    assert_eq!(done["task"], "T1");
    let t = b.task(id);
    assert_eq!(t.s("status"), Some("done"));
    assert_eq!(t.s("pr_url"), Some("https://github.com/acme/webapp/pull/7"));
    assert_eq!(t.s("pr_repo"), Some("acme/webapp"));
    let card = b.get("/tasks/T1");
    assert_eq!(card["pr"]["num"], 7);
    assert_eq!(card["pr"]["url"], "https://github.com/acme/webapp/pull/7");
    let notes = board::goal_notes(&b.app, 0).unwrap();
    assert!(notes.is_empty());
}

#[test]
fn a_question_is_answered_and_handed_over_by_the_hook() {
    let b = new_board();
    let id = new_task(&b, "Pick a colour", json!({}));
    b.add_session("s1", "working");
    b.report("tb.take", "s1", json!({"task": "T1"}));
    assert_eq!(b.task(id).s("status"), Some("working"));

    b.report("tb.question", "s1", json!({"text": "Blue or green?"}));
    let t = b.task(id);
    assert_eq!(t.s("status"), Some("needs"));
    assert_eq!(t.s("question"), Some("Blue or green?"));
    let state = b.get("/state");
    assert_eq!(state["columns"]["needs"][0]["ref"], "T1");

    b.post("/tasks/T1/answer", json!({"text": "Green"}));
    let d = b.jobs("deliver");
    assert_eq!(d.len(), 1);
    assert!(board::job_args(&d[0]).st("text").contains("answer: Green"));

    let out = b.report("hook.stop", "s1", json!({"last_message": "Waiting."}));
    let ids = out["deliver"]["ids"].as_array().unwrap().clone();
    assert_eq!(ids.len(), 1);
    assert!(out["deliver"]["text"].as_str().unwrap().contains("Green"));
    assert_eq!(b.task(id).s("status"), Some("working"), "the answer was sent, so the next stop goes back to work");

    b.report("hook.delivered", "s1", json!({"ids": ids}));
    assert_eq!(b.jobs("deliver")[0].s("state"), Some("done"));
    let log = b.get("/tasks/T1/log")["log"].as_array().unwrap().clone();
    assert!(log.iter().any(|e| e["text"].as_str().unwrap().starts_with("Delivered your answer to Term s1")));
    let ctx = board::task_context(&b.task(id));
    assert!(ctx["answers"][0].as_str().unwrap().contains("Blue or green? → “Green”"));
}

#[test]
fn a_closed_terminal_requeues_the_task_then_waits_for_the_owner() {
    let b = new_board();
    let id = new_task(&b, "Long job", json!({}));
    b.add_session("s1", "working");
    b.report("tb.take", "s1", json!({"task": "T1"}));
    b.report("hook.session_end", "s1", json!({"reason": "other"}));
    let t = b.task(id);
    assert_eq!(t.s("status"), Some("queued"), "the first loss restarts from the handoff");
    assert_eq!(t.s("pickup"), Some("queue"));

    b.add_session("s2", "working");
    b.report("tb.take", "s2", json!({"task": "T1"}));
    b.report("hook.session_end", "s2", json!({"reason": "other"}));
    b.add_session("s3", "working");
    b.report("tb.take", "s3", json!({"task": "T1"}));
    b.report("hook.session_end", "s3", json!({"reason": "other"}));
    let t = b.task(id);
    assert_eq!(t.s("status"), Some("needs"), "after two restarts in an hour it waits for the owner");
    assert_eq!(t.s("needs_reason"), Some("lost"));
    assert!(t.b("lost"));
}

#[test]
fn midna_sync_tracks_terminals_and_marks_missing_ones_gone() {
    let b = new_board();
    b.add_session("s1", "idle");
    let s = board::get_session(&b.app, Some("s1")).unwrap().unwrap();
    assert_eq!(s.s("project"), Some("webapp"));
    assert_eq!(s.s("status"), Some("idle"));
    let rows = b.get("/sessions")["sessions"].as_array().unwrap().clone();
    assert_eq!(rows[0]["can_take"], true);
    midna::sync(&b.app, &[], &[]).unwrap();
    midna::sync(&b.app, &[], &[]).unwrap();
    let s = board::get_session(&b.app, Some("s1")).unwrap().unwrap();
    assert_eq!(s.s("status"), Some("gone"));
    let closed = b.get("/sessions/closed");
    assert_eq!(closed["total"], 1);
}

#[test]
fn a_task_waits_for_another_and_a_goal_runs_in_order() {
    let b = new_board();
    let g = b.post("/goals", json!({"name": "Checkout", "project": "webapp", "outcome": "people can pay", "max_terminals": 2}));
    let gid = g["id"].as_i64().unwrap();
    let t1 = new_task(&b, "Cart", json!({"goal_id": gid, "status": "planned"}));
    let t2 = new_task(&b, "Pay", json!({"goal_id": gid, "status": "planned"}));
    let other = new_task(&b, "Tax table", json!({}));
    b.post(&format!("/tasks/T{other}"), json!({"pickup": "manual"}));
    b.post(&format!("/tasks/T{t1}"), json!({"waits_for": format!("T{other}")}));
    let (code, msg) = b.post_err(&format!("/tasks/T{other}"), json!({"waits_for": format!("T{t1}")}));
    assert_eq!(code, 409);
    assert!(msg.contains("wait forever"));

    let run = b.post(&format!("/goals/G{gid}/run"), json!({}));
    assert_eq!(run["queued_now"], 2);
    assert!(runner::start_queued(&b.app).unwrap().is_empty(), "T1 is blocked by the tax table and T2 waits for T1");
    let card = b.get(&format!("/tasks/T{t1}"));
    assert_eq!(card["blocked"], true);
    assert!(card["waiting"].as_str().unwrap().starts_with("Blocked by T"));
    assert!(b.get(&format!("/tasks/T{t2}"))["waiting"].as_str().unwrap().contains("to finish first"));

    b.post(&format!("/tasks/T{other}/done"), json!({"summary": "Table in"}));
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![t1]);
    let prompt = board::job_args(&b.jobs("agent")[0]).st("prompt");
    assert!(prompt.contains("This task needs work from another task, and it's ready"));
    assert!(prompt.contains("It is part of the goal “Checkout”, done when people can pay."));
}

#[test]
fn spooled_reports_are_read_in_order_and_deduplicated() {
    let b = new_board();
    new_task(&b, "Spooled", json!({}));
    b.add_session("s1", "working");
    b.report("tb.take", "s1", json!({"task": "T1"}));
    let dir = b.app.cfg.spool_dir();
    let body = json!({"event": "tb.note", "session": "s1", "text": "offline note", "at": "2026-10-01T10:00:00Z"});
    std::fs::write(dir.join("0000000000001-1-000001.json"), body.to_string()).unwrap();
    std::fs::write(dir.join("0000000000002-1-000002.json"), body.to_string()).unwrap();
    std::fs::write(dir.join("0000000000003-1-000003.json"), "not json").unwrap();
    assert_eq!(reports::ingest_spool(&b.app).unwrap(), 2, "both readable files are handled; the bad one is set aside");
    let notes = b.app.db.count("SELECT COUNT(*) FROM events WHERE kind = 'note' AND text = 'offline note'", p![]).unwrap();
    assert_eq!(notes, 1);
    assert!(dir.join("0000000000003-1-000003.json.bad").exists());
}

#[test]
fn a_failed_start_retries_then_waits_for_the_owner() {
    let b = new_board();
    let id = new_task(&b, "Flaky start", json!({}));
    for _ in 0..4 {
        runner::start_queued(&b.app).unwrap();
        let j = b.app.db.q1("SELECT * FROM jobs WHERE kind = 'agent' AND state = 'pending'", p![]).unwrap().unwrap();
        b.app.db.x("UPDATE jobs SET state = 'running' WHERE id = ?", p![j.id()]).unwrap();
        b.app.db.tx(|| jobs::result(&b.app, j.id(), false, "", "midna: the folder is gone", 1)).unwrap();
        b.app.db.x("UPDATE tasks SET retry_at = NULL WHERE id = ?", p![id]).unwrap();
        if b.task(id).s("status") == Some("needs") {
            break;
        }
        assert_eq!(b.task(id).s("status"), Some("queued"));
    }
    let t = b.task(id);
    assert_eq!(t.s("status"), Some("needs"));
    assert_eq!(t.s("needs_reason"), Some("start_failed"));
    assert_eq!(b.get("/state")["alerts"].as_array().unwrap().len(), 1);
}

#[test]
fn a_github_pr_moves_through_its_stages_and_wakes_the_task() {
    let b = board_with(|c| {
        c.pr.watch = true;
        c.pr.wake = true;
    });
    let id = new_task(&b, "Ship it", json!({}));
    b.add_session("s1", "working");
    b.report("tb.take", "s1", json!({"task": "T1"}));
    b.report("tb.done", "s1", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9"}));
    let t = b.task(id);
    assert_eq!(t.s("pr_phase"), Some("checks"));

    let rec = |failed: Value, decision: &str, comments: i64| {
        json!({"state": "OPEN", "head": "h1", "checks": [{"name": "ci", "state": if failed.as_array().unwrap().is_empty() { "passed" } else { "failed" }}],
               "failed": failed, "running": 0, "comments": comments, "approvals": 0, "review_decision": decision})
    };
    b.app.db.tx(|| prflow::step(&b.app, &b.task(id), &rec(json!(["ci"]), "", 0))).unwrap();
    assert_eq!(b.task(id).s("pr_phase"), Some("fix"));
    let wakes = b.app.db.q("SELECT * FROM jobs WHERE kind = 'agent' AND purpose = 'pr'", p![]).unwrap();
    assert_eq!(wakes.len(), 1);
    let a = board::job_args(&wakes[0]);
    assert!(a.st("prompt").contains("A check failed on its head (ci)"));
    assert_eq!(a.st("flags"), "--resume c-s1");
    assert_eq!(b.get("/tasks/T1")["pr"]["checks"], "fail");

    b.app.db.tx(|| prflow::step(&b.app, &b.task(id), &rec(json!(["ci"]), "", 0))).unwrap();
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM jobs WHERE purpose = 'pr'", p![]).unwrap(), 1, "the same state doesn't wake it twice");

    b.post("/tasks/T1/pr/wait", json!({}));
    b.app.db.x("UPDATE jobs SET state = 'done' WHERE purpose = 'pr'", p![]).unwrap();
    b.app.db.tx(|| prflow::step(&b.app, &b.task(id), &rec(json!([]), "REVIEW_REQUIRED", 0))).unwrap();
    assert_eq!(b.task(id).s("pr_phase"), Some("review"));
    b.app.db.tx(|| prflow::step(&b.app, &b.task(id), &rec(json!([]), "CHANGES_REQUESTED", 2))).unwrap();
    assert_eq!(b.task(id).s("pr_phase"), Some("comments"));
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM jobs WHERE purpose = 'pr'", p![]).unwrap(), 2);
    b.app.db.x("UPDATE jobs SET state = 'done' WHERE purpose = 'pr'", p![]).unwrap();
    b.post("/tasks/T1/pr/wait", json!({}));
    b.app.db.tx(|| prflow::step(&b.app, &b.task(id), &rec(json!([]), "APPROVED", 2))).unwrap();
    assert_eq!(b.task(id).s("pr_phase"), Some("merge"));
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM jobs WHERE purpose = 'pr'", p![]).unwrap(), 2, "agents don't merge unless the config says so");
    assert!(b.get("/state")["alerts"].as_array().unwrap().iter().any(|a| a["text"].as_str().unwrap().contains("ready to merge")));
    b.post("/tasks/T1/pr/merged", json!({}));
    assert_eq!(b.task(id).s("pr_phase"), Some("merged"));
}

#[test]
fn the_api_validates_and_moves_backlog_issues() {
    let b = new_board();
    let (code, _) = b.post_err("/tasks", json!({"title": "", "project": "webapp"}));
    assert_eq!(code, 400);
    let g = b.post("/goals", json!({"name": "Docs", "project": "webapp"}));
    let issue = b.post("/backlog", json!({"title": "Typo on the home page", "kind": "clean", "project": "webapp", "said": "teh"}));
    assert_eq!(issue["said"], "“teh”");
    let moved = b.post("/backlog/B1/move", json!({"goal_id": g["id"]}));
    assert_eq!(moved["goal"]["ref"], "G1");
    let out = b.post("/backlog/B1/promote", json!({"where": "goal"}));
    assert_eq!(out["task"]["status"], "planned");
    let issue = b.get("/backlog/B1");
    assert_eq!(issue["state"], "task");
    let hist: Vec<String> = issue["history"].as_array().unwrap().iter().map(|h| h["kind"].as_str().unwrap().to_string()).collect();
    assert_eq!(hist, vec!["note", "move", "task"]);
    let handoff = b.get("/tasks/T1/handoff")["text"].as_str().unwrap().to_string();
    assert!(handoff.contains("This task came from the backlog (B1)."));
    let list = b.get("/backlog?state=all");
    assert_eq!(list["total"], 1);
    let (code, msg) = b.post_err("/backlog/B1/ticket", json!({}));
    assert_eq!(code, 409);
    assert!(msg.contains("Jira isn't set up"));
}

#[test]
fn work_hours_hold_tasks_back() {
    let b = new_board();
    new_task(&b, "Night work", json!({}));
    let h = b.post("/hours", json!({"on": true, "start": "00:00", "end": "00:01", "days": "mon"}));
    assert_eq!(h["on"], true);
    if !h["open"].as_bool().unwrap() {
        assert!(runner::start_queued(&b.app).unwrap().is_empty());
        let card = b.get("/tasks/T1");
        assert!(card["waiting"].as_str().unwrap().starts_with("Waits for work hours"));
    }
    b.post("/hours", json!({"on": false}));
    assert_eq!(runner::start_queued(&b.app).unwrap().len(), 1);
}
