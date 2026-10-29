//! A `tb question` is held on its task while claude screens it (#798): if the call never records it,
//! the board asks it as written once the call is overdue, so the task never looks idle while its agent
//! waits on an answer. `tb status` in a PR-visit terminal shows the visited task.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, fields, midna, reports};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[])
        .unwrap();
    let b = Board { app, _dir: dir };
    api::dispatch(&b.app, "POST", "/tasks", &Query::new(), &json!({"title": "Login", "detail": "Do it.", "project": "webapp"})).unwrap();
    b.report("tb.take", "s1", json!({"task": "T1"}));
    b
}

impl Board {
    fn report(&self, event: &str, session: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {"branch": "feature/x"}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message))
    }
    fn task(&self) -> taskboardd::util::Row {
        board::get_task(&self.app, 1).unwrap()
    }
    /// Holds a question on T1 the way `tb question` does before screening, made `at`.
    fn hold(&self, session: &str, text: &str, visit: bool, at: &str) {
        let mut ctx = board::task_context(&self.task());
        ctx.insert("asking".into(), json!({"id": "h1", "text": text, "options": null, "session": session, "name": "Term s1", "visit": visit, "at": at}));
        board::save_context(&self.app, 1, &ctx, false).unwrap();
    }
    fn held(&self) -> bool {
        board::task_context(&self.task()).contains_key("asking")
    }
    fn session_status(&self, sid: &str) -> Option<String> {
        board::get_session(&self.app, Some(sid)).unwrap().and_then(|s| s.s("status").map(String::from))
    }
}

const LONG_AGO: &str = "2026-01-01T10:00:00Z";

#[test]
fn a_question_whose_call_was_lost_is_asked_as_written_once_overdue() {
    let b = new_board();
    b.hold("s1", "Which login page?", false, &taskboardd::util::now_iso());
    reports::lost_questions(&b.app).unwrap();
    assert!(b.held(), "a call still within the screener's time is left to finish");
    assert_eq!(b.task().s("status"), Some("working"));

    b.hold("s1", "Which login page?", false, LONG_AGO);
    reports::lost_questions(&b.app).unwrap();
    let t = b.task();
    assert_eq!((t.s("status"), t.s("needs_reason"), t.s("question")), (Some("needs"), Some("question"), Some("Which login page?")));
    assert!(!b.held(), "it's asked once");
    assert_eq!(b.session_status("s1").as_deref(), Some("needs"));
}

#[test]
fn a_question_that_goes_through_leaves_no_hold_behind() {
    let b = new_board();
    b.report("tb.question", "s1", json!({"text": "Which login page?"}));
    assert!(!b.held());
    assert_eq!(b.task().s("status"), Some("needs"));
    assert_eq!(b.session_status("s1").as_deref(), Some("needs"));
}

#[test]
fn a_lost_question_on_a_pr_visit_is_asked_on_the_pr() {
    let b = new_board();
    board::update_task(&b.app, 1, fields!["status" => "done", "pr_num" => 9, "pr_state" => "OPEN", "pr_phase" => "comments",
                                          "pr_flow" => json!({"session": "v1"}).to_string()]).unwrap();
    b.hold("v1", "Who can read the thread?", true, LONG_AGO);
    reports::lost_questions(&b.app).unwrap();
    let stopped = &serde_json::from_str::<Value>(b.task().s("pr_flow").unwrap()).unwrap()["stopped"];
    assert_eq!((&stopped["asked"], &stopped["message"]), (&json!(true), &json!("Who can read the thread?")));
    assert!(!b.held());
}

#[test]
fn a_lost_question_from_a_visit_that_ended_is_dropped() {
    let b = new_board();
    board::update_task(&b.app, 1, fields!["status" => "done", "pr_num" => 9, "pr_state" => "OPEN", "pr_phase" => "comments",
                                          "pr_flow" => json!({"session": "v2"}).to_string()]).unwrap();
    b.hold("v1", "Who can read the thread?", true, LONG_AGO);
    reports::lost_questions(&b.app).unwrap();
    assert!(!b.held());
    assert!(serde_json::from_str::<Value>(b.task().s("pr_flow").unwrap()).unwrap().get("stopped").is_none());
}

#[test]
fn tb_status_in_a_pr_visit_terminal_shows_the_visited_task() {
    let b = new_board();
    board::update_task(&b.app, 1, fields!["status" => "done", "pr_num" => 9, "pr_state" => "OPEN", "pr_phase" => "comments",
                                          "pr_flow" => json!({"session": "v1"}).to_string()]).unwrap();
    let v = b.report("tb.status", "v1", json!({}));
    assert_eq!(v["context"], json!("T1 · Login · Done"), "{v}");
    b.report("tb.question", "v1", json!({"text": "Who can read the thread?"}));
    let v = b.report("tb.status", "v1", json!({}));
    assert_eq!(v["context"], json!("T1 · Login · Needs you"), "{v}");
}
