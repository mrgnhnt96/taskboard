//! Idle task terminals: asked for a checkpoint, closed to free their memory, and reopened with the
//! board's next message for the task (`[limits] idle_close_mins`).

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{iso, now_ts, Row, RowExt};
use taskboardd::{board, deliver, idle_close, midna, p, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    std::fs::create_dir_all(dir.path().join("webapp")).unwrap();
    let app = App::for_tests(cfg);
    let b = Board { app, dir };
    b.app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": b.repo().to_string_lossy()}]).to_string())).unwrap();
    b
}

/// A board that closes task terminals idle 30 minutes.
fn idle_board() -> Board {
    let b = new_board();
    b.post("/limits", json!({"idle_close_mins": 30})).unwrap();
    b
}

fn ago(secs: f64) -> String {
    iso(now_ts() - secs)
}

impl Board {
    fn repo(&self) -> PathBuf {
        self.dir.path().join("webapp")
    }
    fn post(&self, path: &str, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": self.repo().to_string_lossy()});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn add_session(&self, status: &str) {
        let s = json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": self.repo().to_string_lossy(), "status": {"state": status}});
        midna::sync(&self.app, &[s], &[]).unwrap();
    }
    fn task(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    /// A task on `s1`, a terminal the board opened for it.
    fn new_task(&self) -> i64 {
        let id = self.post("/tasks", json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
        self.add_session("working");
        self.report("tb.take", json!({"task": format!("T{id}")})).unwrap();
        let opened = board::create_job(&self.app, "agent", json!({}), Some(id), "start", Some(json!({"session": "s1"}))).unwrap();
        self.app.db.x("UPDATE jobs SET state = 'done' WHERE id = ?", p![opened]).unwrap();
        id
    }
    /// `s1` went idle `secs` ago.
    fn idle_for(&self, secs: f64) {
        self.add_session("idle");
        self.app.db.x("UPDATE sessions SET status = 'idle', status_at = ? WHERE id = 's1'", p![ago(secs)]).unwrap();
    }
    fn sweep(&self) {
        idle_close::sweep(&self.app).unwrap();
    }
    fn jobs(&self, kind: &str, purpose: &str) -> Vec<Row> {
        self.app.db.q("SELECT * FROM jobs WHERE kind = ? AND purpose = ? ORDER BY id", p![kind, purpose]).unwrap()
    }
    fn close_jobs(&self) -> Vec<Row> {
        self.jobs("close", board::IDLE_CLOSE)
    }
    fn starts(&self) -> Vec<Row> {
        self.app.db.q("SELECT * FROM jobs WHERE kind = 'agent' AND purpose = 'start' AND state = 'pending' ORDER BY id", p![]).unwrap()
    }
    /// Its checkpoint ask was taken in and the turn it started is over.
    fn checkpoint_turn_over(&self, id: i64) {
        self.app.db.x("UPDATE jobs SET state = 'done' WHERE kind = 'deliver'", p![]).unwrap();
        let mut ctx = board::task_context(&self.task(id));
        ctx["idle_close"]["asked_at"] = json!(ago(60.0));
        board::save_context(&self.app, id, &ctx, false).unwrap();
        self.idle_for(30.0);
    }
    /// Closes task `id`'s idle terminal the whole way: asked, closed, gone.
    fn close_idle(&self, id: i64) {
        self.idle_for(31.0 * 60.0);
        self.sweep();
        self.checkpoint_turn_over(id);
        self.sweep();
        assert_eq!(self.close_jobs().len(), 1);
        board::session_gone(&self.app, "s1", "other").unwrap();
    }
    /// A transcript for the conversation `c-s1`: its last reply's context size, written `mins_ago`.
    fn transcript(&self, tokens: i64, mins_ago: i64) {
        let dir = self.app.cfg.claude_projects.join("-somewhere");
        std::fs::create_dir_all(&dir).unwrap();
        let line = json!({"type": "assistant", "timestamp": ago(mins_ago as f64 * 60.0), "message": {"usage": {"input_tokens": 10, "cache_read_input_tokens": tokens - 10}}});
        std::fs::write(dir.join("c-s1.jsonl"), format!("{line}\n")).unwrap();
    }
    fn deliver_tick(&self) {
        self.app.db.tx(|| deliver::tick(&self.app)).unwrap();
    }
}

#[test]
fn an_idle_task_terminal_saves_a_checkpoint_then_closes_and_the_task_keeps_its_place() {
    let b = idle_board();
    let id = b.new_task();
    b.idle_for(29.0 * 60.0);
    b.sweep();
    assert!(b.jobs("deliver", idle_close::CHECKPOINT).is_empty(), "under the idle limit");

    b.idle_for(31.0 * 60.0);
    b.sweep();
    let asks = b.jobs("deliver", idle_close::CHECKPOINT);
    assert_eq!(asks.len(), 1, "asked for a checkpoint first");
    let text = board::job_args(&asks[0]).st("text");
    assert!(text.starts_with(&format!("[task-board:T{id}] This terminal has sat idle for 30m")), "{text}");
    assert!(text.contains("checkpoint --done"), "{text}");
    b.sweep();
    assert_eq!(b.jobs("deliver", idle_close::CHECKPOINT).len(), 1, "asked once");
    assert!(b.close_jobs().is_empty(), "not closed while the ask is on its way");

    b.checkpoint_turn_over(id);
    b.sweep();
    let jobs = b.close_jobs();
    assert_eq!(jobs.len(), 1, "closed once its checkpoint turn is over");
    assert_eq!(board::job_args(&jobs[0])["force"], false, "never forced");
    b.sweep();
    assert_eq!(b.close_jobs().len(), 1, "once");

    board::session_gone(&b.app, "s1", "other").unwrap();
    let t = b.task(id);
    assert_eq!((t.s("status"), t.s("session_id"), t.b("lost")), (Some("working"), None, false), "it keeps its status, not queued");
    assert_eq!(t.st("latest"), "Closed its idle terminal to free memory. It carries on in a new terminal when the board has something for it.");
    let ctx = board::task_context(&t);
    assert_eq!(ctx["idle_closed"]["session"], "s1");
    assert!(ctx.get("idle_close").is_none());
    assert!(idle_close::closed(&t));
    assert!(b.starts().is_empty(), "nothing starts it until the board has something for it");
}

#[test]
fn a_message_for_an_idle_closed_task_carries_its_conversation_on_in_a_new_terminal() {
    let b = idle_board();
    let id = b.new_task();
    b.close_idle(id);
    b.transcript(20_000, 5);

    deliver::add(&b.app, "code_review_fix", "Fix these: the null check.", id, None, None).unwrap();
    b.deliver_tick();
    let starts = b.starts();
    assert_eq!(starts.len(), 1);
    let a = board::job_args(&starts[0]);
    assert_eq!(a.st("flags"), "--resume c-s1", "warm and small: resumed");
    assert!(a.st("prompt").starts_with(&format!("[task-board:T{id}] The board closed this terminal while it sat idle")), "{}", a.st("prompt"));
    assert!(a.st("prompt").ends_with("Fix these: the null check."));
    assert_eq!(b.jobs("deliver", "code_review_fix")[0].s("state"), Some("done"));

    deliver::add(&b.app, "rebase", "Rebase onto main.", id, None, None).unwrap();
    b.deliver_tick();
    assert_eq!(b.starts().len(), 1, "one terminal at a time");
    assert_eq!(b.jobs("deliver", "rebase")[0].s("state"), Some("pending"), "it waits for the new terminal");

    let t = b.task(id);
    board::claim(&b.app, &t, "s2", Some("c-s2"), None, "Started", false).unwrap().unwrap();
    let t = b.task(id);
    assert_eq!(t.s("session_id"), Some("s2"));
    assert!(board::task_context(&t).get("idle_closed").is_none(), "picked up: no longer waiting on a reopen");
    assert!(!idle_close::closed(&t));
}

#[test]
fn a_big_conversation_starts_fresh_from_the_handoff_with_the_message() {
    let b = idle_board();
    let id = b.new_task();
    b.close_idle(id);
    b.transcript(90_000, 5);

    deliver::add(&b.app, "code_review", "Your submit was turned back.", id, None, None).unwrap();
    b.deliver_tick();
    let a = board::job_args(&b.starts()[0]);
    assert!(a.s("flags").is_none(), "too big: fresh");
    assert!(a.st("prompt").contains("Ship it"), "the handoff: {}", a.st("prompt"));
    assert!(a.st("prompt").ends_with("Your submit was turned back."));
}

#[test]
fn a_checkpoint_request_for_an_idle_closed_task_doesnt_reopen_it() {
    let b = idle_board();
    let id = b.new_task();
    b.close_idle(id);
    deliver::add(&b.app, "checkpoint", "Save a checkpoint.", id, None, None).unwrap();
    b.deliver_tick();
    assert!(b.starts().is_empty());
    assert_eq!(b.jobs("deliver", "checkpoint")[0].s("state"), Some("expired"));
}

#[test]
fn a_question_closes_without_a_checkpoint_and_the_answer_reopens_it() {
    let b = idle_board();
    let id = b.new_task();
    b.report("tb.question", json!({"text": "10 or 30 retries?"})).unwrap();
    b.idle_for(31.0 * 60.0);
    b.sweep();
    assert!(b.jobs("deliver", idle_close::CHECKPOINT).is_empty(), "its reply could read as the answer");
    assert_eq!(b.close_jobs().len(), 1);

    board::session_gone(&b.app, "s1", "other").unwrap();
    let t = b.task(id);
    assert_eq!((t.s("status"), t.s("question")), (Some("needs"), Some("10 or 30 retries?")), "still waits on the owner");
    assert_eq!(t.st("latest"), "Closed its idle terminal to free memory. It carries on in a new terminal once you answer.");

    b.transcript(20_000, 5);
    b.post(&format!("/tasks/T{id}/answer"), json!({"text": "30", "when": "now"})).unwrap();
    let starts = b.starts();
    assert_eq!(starts.len(), 1);
    let a = board::job_args(&starts[0]);
    assert_eq!(a.st("flags"), "--resume c-s1", "the conversation carries on with the answer");
    assert!(a.st("prompt").contains("30"), "{}", a.st("prompt"));
}

#[test]
fn terminals_that_stay_open() {
    // The owner's own terminal that took the task.
    let b = idle_board();
    let id = b.post("/tasks", json!({"title": "Mine", "detail": "Do it.", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    b.add_session("working");
    b.report("tb.take", json!({"task": format!("T{id}")})).unwrap();
    b.idle_for(3.0 * 3600.0);
    b.sweep();
    assert!(b.jobs("deliver", idle_close::CHECKPOINT).is_empty() && b.close_jobs().is_empty(), "the owner's terminal isn't the board's to close");

    // Off (the tests' default), and a message on its way.
    let b = new_board();
    let id = b.new_task();
    b.idle_for(3.0 * 3600.0);
    b.sweep();
    assert!(b.jobs("deliver", idle_close::CHECKPOINT).is_empty(), "0 keeps them open");
    b.post("/limits", json!({"idle_close_mins": 30})).unwrap();
    deliver::add(&b.app, "slot_free", "A slot freed.", id, None, None).unwrap();
    b.sweep();
    assert!(b.jobs("deliver", idle_close::CHECKPOINT).is_empty(), "not while the board is handing it something");

    // Background work still running.
    b.app.db.x("DELETE FROM jobs WHERE kind = 'deliver'", p![]).unwrap();
    b.app.db.x("UPDATE sessions SET background = 1, background_at = ? WHERE id = 's1'", p![ago(10.0)]).unwrap();
    b.sweep();
    assert!(b.jobs("deliver", idle_close::CHECKPOINT).is_empty(), "a background build or agent counts as busy");
}

#[test]
fn tb_limits_sets_the_idle_close() {
    let b = new_board();
    let v = b.post("/limits", json!({"idle_close_mins": 45})).unwrap();
    assert_eq!(v["idle_close_mins"], 45);
    assert!(v["line"].as_str().unwrap().contains("Close idle task terminals after: 45m"), "{}", v["line"]);
    let v = b.post("/limits", json!({"idle_close_mins": 0})).unwrap();
    assert!(v["line"].as_str().unwrap().contains("Close idle task terminals after: off"));
    assert_eq!(b.post("/limits", json!({"idle_close_mins": -5})).unwrap_err().0, 400);
    assert_eq!(taskboardd::config::LimitsConfig::default().idle_close_mins, 30, "on by default outside tests");
}
