//! Questions an agent asks in its reply text, not through `tb question`: the turn's end is screened
//! against the owner's rules, answered back to the agent when they cover it, and otherwise put to the
//! owner with an alert. A follow-up prompt doesn't settle it: asking again alerts again.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, dispatch, midna, p, reports};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

/// A board whose question screener is a script printing `verdict` (the JSON claude would return).
fn board_screening(verdict: Value) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let script = dir.path().join("claude");
    std::fs::write(&script, format!("#!/bin/sh\necho '{}'\n", json!({"structured_output": verdict}))).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.claude = script.to_string_lossy().to_string();
    cfg.questions.screen = true;
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[])
        .unwrap();
    let b = Board { app, _dir: dir };
    b.post("/tasks", json!({"title": "Cap the retries", "detail": "Do the thing.", "project": "webapp"}));
    b.report("tb.take", json!({"task": "T1"}));
    b
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn report(&self, event: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {"branch": "feature/x", "sha": "abc123"}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message))
    }
    fn stop(&self, reply: &str) -> Value {
        self.report("hook.stop", json!({"last_message": reply, "last_message_end": reply}))
    }
    fn task(&self) -> taskboardd::util::Row {
        board::get_task(&self.app, 1).unwrap()
    }
    fn alerts(&self) -> Vec<String> {
        dispatch::alerts(&self.app).iter().filter_map(|a| a["text"].as_str().map(|s| s.to_string())).collect()
    }
    fn answers(&self) -> Vec<String> {
        let jobs = self.app.db.q("SELECT * FROM jobs WHERE kind = 'deliver' AND purpose = 'answer' ORDER BY id", p![]).unwrap();
        jobs.iter().map(|j| board::job_args(j).st("text")).collect()
    }
}

fn ask() -> Value {
    json!({"verdict": "ask"})
}

#[test]
fn a_turn_ending_on_a_question_in_prose_is_screened_and_put_to_the_owner() {
    let b = board_screening(ask());
    b.stop("I looked at the retry loop.\n\nShould the cap be 3 or 5 retries?");
    let t = b.task();
    assert_eq!((t.s("status"), t.s("needs_reason")), (Some("needs"), Some("question")));
    assert_eq!(t.s("question"), Some("Should the cap be 3 or 5 retries?"));
    assert!(b.alerts().iter().any(|a| a == "T1: Should the cap be 3 or 5 retries?"), "{:?}", b.alerts());
    assert!(b.answers().is_empty());
}

#[test]
fn a_turn_that_doesn_t_ask_isn_t_put_to_the_owner() {
    let b = board_screening(ask());
    b.stop("I capped the retries at 3. Next I test it.");
    assert_eq!(b.task().s("status"), Some("working"));
    assert!(b.alerts().is_empty());
}

#[test]
fn a_reply_the_screener_finds_asks_nothing_isn_t_put_to_the_owner() {
    let b = board_screening(json!({"verdict": "none"}));
    b.stop("Nothing needs you right now. When #52 merges, or if a check fails, let me know and I'll finish the merge then.");
    assert_eq!(b.task().s("status"), Some("working"));
    assert!(b.alerts().is_empty());
    assert!(b.answers().is_empty());
}

#[test]
fn a_question_the_rules_answer_is_typed_back_to_the_agent() {
    let b = board_screening(json!({"verdict": "answered", "answer": "Cap it at 3.", "source": "CLAUDE.md"}));
    b.stop("Tell me which cap you want.");
    assert_eq!(b.task().s("status"), Some("working"), "the owner isn't asked");
    assert!(b.alerts().is_empty());
    let answers = b.answers();
    assert_eq!(answers.len(), 1);
    assert!(answers[0].starts_with("[task-board:T1] Answered from CLAUDE.md without asking"), "{}", answers[0]);
    assert!(answers[0].contains("Cap it at 3."));
}

#[test]
fn a_follow_up_prompt_then_a_re_ask_in_prose_alerts_the_owner_again() {
    let b = board_screening(ask());
    b.report("tb.question", json!({"text": "Should the cap be 3 or 5 retries?"}));
    assert_eq!(b.task().s("status"), Some("needs"));

    // The owner asks something back instead of answering; the agent replies and asks again in prose.
    b.report("hook.prompt", json!({"prompt": "What does the cap guard against?"}));
    assert_eq!(b.task().s("status"), Some("working"));
    b.stop("It stops a dead host being retried forever.\n\nTell me which cap you want, or answer the board question.");
    let t = b.task();
    assert_eq!((t.s("status"), t.s("needs_reason")), (Some("needs"), Some("question")));
    assert_eq!(t.s("answered_at"), None);
    assert!(b.alerts().iter().any(|a| a.starts_with("T1: Tell me which cap you want")), "{:?}", b.alerts());
}
