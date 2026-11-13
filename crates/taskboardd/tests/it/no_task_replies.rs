//! Questions a terminal with no task ends its turn on: a Talk it over or alert-fix terminal, goal
//! planning, the Jira desk and review triage are screened like a task's turn. An answer from the rules
//! is typed back once the terminal has settled at its prompt, and dropped if it moved on first; a
//! question the rules don't answer goes to the owner where they'd see it.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, dispatch, midna, p, reply_screen, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

/// A board whose question screener prints `verdict`, with terminal s1 opened by an agent job for
/// `purpose` (none: a terminal the owner opened), with `args` and `target`.
fn board(verdict: Value, purpose: Option<&str>, args: Value, target: Value) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let script = dir.path().join("claude");
    let saved = dir.path().join("prompt.txt");
    std::fs::write(&script, format!("#!/bin/sh\nprintf '%s' \"$2\" > '{}'\necho '{}'\n", saved.display(), json!({"structured_output": verdict}))).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.claude = script.to_string_lossy().to_string();
    cfg.questions.screen = true;
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[])
        .unwrap();
    let b = Board { app, dir };
    b.post("/goals", json!({"name": "Checkout", "project": "webapp"}));
    if let Some(purpose) = purpose {
        let mut target = target;
        target["session"] = json!("s1");
        let j = board::create_job(&b.app, "agent", args, None, purpose, Some(target)).unwrap();
        b.app.db.x("UPDATE jobs SET state = 'done' WHERE id = ?", p![j]).unwrap();
    }
    b
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn report(&self, event: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {"branch": "main", "sha": "abc123"}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message))
    }
    fn stop(&self, reply: &str) {
        self.report("hook.stop", json!({"last_message": reply, "last_message_end": reply}));
    }
    fn screened(&self) -> bool {
        self.dir.path().join("prompt.txt").exists()
    }
    fn session_status(&self) -> String {
        board::get_session(&self.app, Some("s1")).unwrap().unwrap().st("status")
    }
    fn alerts(&self) -> Vec<String> {
        dispatch::alerts(&self.app).iter().filter_map(|a| a["text"].as_str().map(|s| s.to_string())).collect()
    }
    fn jobs(&self, kind: &str) -> Vec<taskboardd::util::Row> {
        self.app.db.q("SELECT * FROM jobs WHERE kind = ? ORDER BY id", p![kind]).unwrap()
    }
    /// The terminal has sat at its prompt long enough to take a message.
    fn settle(&self) {
        self.app.db.x("UPDATE sessions SET status_at = '2020-01-01T00:00:00Z' WHERE id = 's1'", p![]).unwrap();
        let held = self.jobs(reply_screen::KIND);
        for j in held.iter().filter(|j| j.s("state") == Some("pending")) {
            let mut target = board::job_target(j);
            target.insert("stopped_at".into(), json!("2020-01-01T00:00:00Z"));
            self.app.db.x("UPDATE jobs SET target = ? WHERE id = ?", p![Value::Object(target).to_string(), j.id()]).unwrap();
        }
    }
}

fn ask() -> Value {
    json!({"verdict": "ask", "question": "Should the plan split the API work into its own goal?"})
}

fn answered() -> Value {
    json!({"verdict": "answered", "answer": "Keep it in one goal.", "source": "CLAUDE.md"})
}

#[test]
fn a_talk_it_over_question_the_rules_don_t_answer_needs_the_owner_on_that_terminal() {
    let b = board(ask(), Some("human_talk"), json!({}), json!({"task": 1}));
    b.stop("I read the steps.\n\nDo you want me to split it?");
    assert!(b.screened());
    assert_eq!(b.session_status(), "needs");
    assert!(b.alerts().is_empty(), "{:?}", b.alerts());
    assert!(b.jobs(reply_screen::KIND).is_empty());
}

#[test]
fn background_planning_s_question_is_an_alert_on_the_goal() {
    let b = board(ask(), Some("plan"), json!({"prompt": "Plan it", "queue": false}), json!({"goal": 1}));
    b.stop("The plan has three tasks.\n\nShould the API work be its own goal?");
    assert_eq!(b.session_status(), "needs");
    assert_eq!(b.alerts(), vec!["G1: Should the plan split the API work into its own goal?".to_string()]);
}

#[test]
fn planning_opened_in_claude_asks_on_its_terminal() {
    let b = board(ask(), Some("plan"), json!({"prompt": "", "focus": true}), json!({"goal": 1}));
    b.stop("Should the API work be its own goal?");
    assert_eq!(b.session_status(), "needs");
    assert!(b.alerts().is_empty(), "{:?}", b.alerts());
}

#[test]
fn the_jira_desk_s_question_goes_nowhere() {
    let b = board(ask(), Some(taskboardd::jira_desk::PURPOSE), json!({}), json!({}));
    b.stop("Which ticket should I move first?");
    assert!(b.screened());
    assert_eq!(b.session_status(), "idle");
    assert!(b.alerts().is_empty());
}

#[test]
fn a_terminal_the_owner_opened_isn_t_screened() {
    let b = board(ask(), None, json!({}), json!({}));
    b.stop("Should I split it?");
    assert!(!b.screened());
    assert_eq!(b.session_status(), "idle");
}

#[test]
fn a_reply_that_asks_nothing_is_left_alone() {
    let b = board(json!({"verdict": "none"}), Some("alert"), json!({}), json!({"alert": "a1"}));
    b.stop("Fixed it. Let me know if anything else breaks?");
    assert!(b.screened());
    assert_eq!(b.session_status(), "idle");
    assert!(b.jobs(reply_screen::KIND).is_empty());
}

#[test]
fn an_answer_from_the_rules_is_typed_in_once_the_terminal_settles() {
    let b = board(answered(), Some("idea_talk"), json!({}), json!({"idea": 1}));
    b.stop("Should this be one goal or two?");
    assert_eq!(b.session_status(), "idle", "the owner isn't asked");
    assert_eq!(b.jobs(reply_screen::KIND).len(), 1);

    reply_screen::tick(&b.app).unwrap();
    assert!(b.jobs("message").is_empty(), "it waits for the terminal to settle at its prompt");

    b.settle();
    reply_screen::tick(&b.app).unwrap();
    let sent = b.jobs("message");
    assert_eq!(sent.len(), 1);
    let args = board::job_args(&sent[0]);
    assert_eq!(args.st("to"), "s1");
    assert!(args.st("text").starts_with("[task-board] Answered from CLAUDE.md without asking"), "{}", args.st("text"));
    assert!(args.st("text").contains("Keep it in one goal."));
    assert_eq!(b.jobs(reply_screen::KIND)[0].s("state"), Some("done"));
}

#[test]
fn an_answer_is_dropped_when_the_terminal_moved_on_first() {
    let b = board(answered(), Some("alert_talk"), json!({}), json!({"alert": "a1"}));
    b.stop("Should I snooze it?");
    b.settle();
    b.report("hook.prompt", json!({"prompt": "Actually, just dismiss it."}));
    reply_screen::tick(&b.app).unwrap();
    assert!(b.jobs("message").is_empty());
    assert_eq!(b.jobs(reply_screen::KIND)[0].s("state"), Some("expired"));
}
