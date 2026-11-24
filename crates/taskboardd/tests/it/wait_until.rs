//! A working task that parks itself until a set time (`tb wait-until`): the board records when it
//! resumes and what it does then, keeps it off the owner while it waits, clears the wait on the next
//! turn, and puts it on the owner only when that time passes by the grace period with no turn.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{advance_clock, reset_clock, RowExt};
use taskboardd::{board, midna, reports, wait_until};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn new_board() -> Board {
    reset_clock();
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let web = dir.path().join("webapp");
    std::fs::create_dir_all(&web).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": web.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": web.to_string_lossy(), "status": {"state": "working"}})], &[])
        .unwrap();
    let b = Board { app, _dir: dir };
    b.call("POST", "/tasks", json!({"title": "Fix the nightly build", "detail": "Do the thing.", "project": "webapp"}));
    b.report("tb.take", json!({"task": "T1"}));
    b
}

impl Board {
    fn call(&self, method: &str, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, method, path, &Query::new(), &body).unwrap_or_else(|e| panic!("{method} {path}: {}", e.message))
    }
    fn body(event: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {"branch": "feature/x", "sha": "abc123"}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        b
    }
    fn report(&self, event: &str, extra: Value) -> Value {
        reports::handle(&self.app, Self::body(event, extra), false).unwrap_or_else(|e| panic!("{event}: {}", e.message))
    }
    fn refused(&self, event: &str, extra: Value) -> String {
        match reports::handle(&self.app, Self::body(event, extra), false) {
            Ok(v) => panic!("{event} went through: {v}"),
            Err(e) => e.message,
        }
    }
    fn wait(&self, at: &str, then: &str) -> Value {
        self.report("tb.wait_until", json!({"at": at, "then": then}))
    }
    fn stop(&self, reply: &str) -> Value {
        self.report("hook.stop", json!({"last_message": reply, "last_message_end": reply, "reply": reply}))
    }
    fn task(&self) -> taskboardd::util::Row {
        board::get_task(&self.app, 1).unwrap()
    }
    fn card(&self) -> Value {
        self.call("GET", "/tasks/T1", json!({}))
    }
}

const PLAN: &str = "read the nightly build's result and fix what failed";

#[test]
fn parking_records_when_it_resumes_and_what_it_does_then() {
    let b = new_board();
    let out = b.wait("in 40m", PLAN);
    let text = out["context"].as_str().unwrap();
    assert!(text.contains("T1 waits until") && text.contains("End your turn"), "{text}");
    assert_eq!(out["resume_plan"], PLAN);
    let t = b.task();
    assert_eq!(t.s("status"), Some("working"), "it's still the agent's");
    assert!(wait_until::waiting(&t));
    let card = b.card();
    assert_eq!(card["waiting_until"], out["waiting_until"], "{card}");
    assert_eq!(card["resume_plan"], PLAN);
    assert!(card["latest"].as_str().unwrap().starts_with("Waiting · resumes "), "{}", card["latest"]);
}

#[test]
fn a_parked_turn_end_and_idle_prompt_stay_off_the_owner() {
    let b = new_board();
    // The board sent the turn's prompt: a plain stop would get the carry-on nudge.
    b.report("hook.prompt", json!({"prompt": "[task-board:T1] continue"}));
    b.wait("06:30 UTC", PLAN);
    let out = b.stop("The nightly build runs at 06:30 UTC. Should I check back then?");
    assert!(out.get("deliver").is_none() && out.get("block").is_none(), "no nudge and not sent back: {out}");
    b.report("hook.attention", json!({"notification_type": "idle_prompt", "message": "Claude is waiting for your input"}));
    let t = b.task();
    assert_eq!(t.s("status"), Some("working"), "not Needs you: {:?}", t.s("question"));
    assert!(wait_until::waiting(&t), "still waiting");
    assert!(t.s("question").is_none());
}

#[test]
fn the_next_turn_clears_the_wait() {
    let b = new_board();
    b.wait("in 2h", PLAN);
    b.stop("Waiting for the nightly build.");
    b.report("hook.prompt", json!({"prompt": "The build finished, check it."}));
    let t = b.task();
    assert_eq!(t.s("status"), Some("working"));
    assert!(!wait_until::waiting(&t) && t.s("waiting_until").is_none() && t.s("resume_plan").is_none());
    let card = b.card();
    assert!(card["waiting_until"].is_null() && card["resume_plan"].is_null(), "{card}");
}

#[test]
fn missing_the_time_by_the_grace_period_needs_the_owner() {
    let b = new_board();
    b.wait("in 40m", PLAN);
    b.stop("Waiting for the nightly build.");
    advance_clock(40.0 * 60.0 + 10.0 * 60.0);
    b.app.db.tx(|| wait_until::sweep(&b.app)).unwrap();
    assert_eq!(b.task().s("status"), Some("working"), "still within the grace period");
    advance_clock(6.0 * 60.0);
    b.app.db.tx(|| wait_until::sweep(&b.app)).unwrap();
    let t = b.task();
    assert_eq!(t.s("status"), Some("needs"));
    assert_eq!(t.s("needs_reason"), Some(wait_until::OVERDUE));
    let q = t.st("question");
    assert!(q.starts_with("T1 was due back at ") && q.contains("hasn't resumed") && q.contains(PLAN), "{q}");
    assert!(t.s("waiting_until").is_none(), "the wait is over");
    // A turn on its terminal carries it on.
    b.report("hook.prompt", json!({"prompt": "carry on"}));
    assert_eq!(b.task().s("status"), Some("working"));
    reset_clock();
}

#[test]
fn none_drops_the_wait() {
    let b = new_board();
    b.wait("in 40m", PLAN);
    let out = b.report("tb.wait_until", json!({"at": "none"}));
    assert!(out["context"].as_str().unwrap().contains("no longer waits"), "{out}");
    assert!(!wait_until::waiting(&b.task()));
}

#[test]
fn a_bad_time_a_missing_plan_or_a_task_not_working_is_refused() {
    let b = new_board();
    assert!(b.refused("tb.wait_until", json!({"at": "soonish", "then": PLAN})).contains("6:30am"));
    assert!(b.refused("tb.wait_until", json!({"at": "in 40m", "then": ""})).contains("--then"));
    b.app.db.x("UPDATE tasks SET status = 'queued' WHERE id = 1", taskboardd::p![]).unwrap();
    let why = b.refused("tb.wait_until", json!({"at": "in 40m", "then": PLAN, "task": "T1"}));
    assert!(why.contains("isn't working"), "{why}");
}
