//! What a task waiting on the owner asks: `tb question --option` answers in a click, Claude's picker
//! (`AskUserQuestion`) shows its questions and options until it closes, and an idle prompt says what
//! the agent's last reply ended on rather than Claude's "waiting for your input".

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, reports};

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
    fn card(&self) -> Value {
        board::task_card(&self.app, &board::get_task(&self.app, 1).unwrap()).unwrap()
    }
}

#[test]
fn question_options_go_on_the_card_and_drop_with_the_question() {
    let b = new_board();
    b.report("tb.question", json!({"text": "10 or 30 retries?", "options": ["10: The old limit", "30"]}));
    let c = b.card();
    assert_eq!(c["status"], "needs");
    assert_eq!(c["ask"]["picker"], false);
    assert_eq!(c["ask"]["questions"][0]["options"], json!([{"label": "10", "description": "The old limit"}, {"label": "30", "description": ""}]));

    b.post("/tasks/T1/answer", json!({"text": "10", "when": "now"}));
    b.report("hook.prompt", json!({"prompt": "[task-board:T1] answer: 10"}));
    let c = b.card();
    assert_eq!(c["status"], "working");
    assert_eq!(c["ask"], Value::Null);

    b.report("tb.question", json!({"text": "Ship it?"}));
    assert_eq!(b.card()["ask"], Value::Null, "a question without options has none");
}

#[test]
fn a_picker_waits_on_the_owner_until_it_closes() {
    let b = new_board();
    let input = json!({"questions": [{"question": "Pick a fruit", "header": "Fruit", "multiSelect": false,
                                       "options": [{"label": "Apple", "description": "Red"}, {"label": "Pear", "description": "Green"}]}]});
    b.report("hook.ask", json!({"tool_input": input}));
    let c = b.card();
    assert_eq!((c["status"].as_str(), c["needs_reason"].as_str(), c["question"].as_str()), (Some("needs"), Some("attention"), Some("Pick a fruit")));
    assert_eq!(c["ask"]["picker"], true);
    assert_eq!(c["ask"]["questions"][0]["options"][1], json!({"label": "Pear", "description": "Green"}));

    // Claude's own notification about the picker says less than the picker.
    b.report("hook.attention", json!({"notification_type": "permission_prompt", "message": "Claude needs your input"}));
    assert_eq!(b.card()["question"], "Pick a fruit");

    b.report("hook.ask_done", json!({}));
    let c = b.card();
    assert_eq!(c["status"], "working");
    assert_eq!((c["question"].clone(), c["ask"].clone()), (Value::Null, Value::Null));
    assert!(board::get_task(&b.app, 1).unwrap().s("answered_at").is_some());
}

#[test]
fn an_idle_prompt_says_what_the_last_reply_ended_on() {
    let b = new_board();
    let dir = b.app.cfg.claude_projects.join("webapp");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("c-s1.jsonl");
    let lines = [
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Removed the subscription.\n\nShould I also drop the old topic?"}]}}),
        json!({"type": "assistant", "isSidechain": true, "message": {"content": [{"type": "text", "text": "a subagent's"}]}}),
    ];
    std::fs::write(&path, lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n")).unwrap();
    b.report("hook.attention", json!({"notification_type": "idle_prompt", "message": "Claude is waiting for your input", "transcript_path": path.to_string_lossy()}));
    let c = b.card();
    assert_eq!((c["status"].as_str(), c["question"].as_str()), (Some("needs"), Some("Should I also drop the old topic?")));
}

#[test]
fn a_question_answered_another_way_is_withdrawn() {
    let b = new_board();
    let none = reports::handle(&b.app, json!({"event": "tb.withdraw", "session": "s1", "why": "Found it"}), false).unwrap_err();
    assert_eq!(none.status, 409, "nothing to withdraw before a question");

    b.report("tb.question", json!({"text": "10 or 30 retries?", "options": ["10", "30"]}));
    assert_eq!(b.card()["status"], "needs");
    let empty = reports::handle(&b.app, json!({"event": "tb.withdraw", "session": "s1", "why": " "}), false).unwrap_err();
    assert_eq!(empty.status, 400, "a withdrawal says how it got answered");

    let v = b.report("tb.withdraw", json!({"why": "The config already says 10"}));
    assert_eq!(v["task"], "T1");
    let c = b.card();
    assert_eq!((c["status"].as_str(), c["question"].as_str(), c["ask"].as_object()), (Some("working"), None, None));
    let t = board::get_task(&b.app, 1).unwrap();
    assert_eq!(t.s("needs_reason"), None);
    let logged = b.app.db.q1("SELECT text FROM events WHERE task_id = 1 ORDER BY id DESC LIMIT 1", taskboardd::p![]).unwrap().unwrap().st("text");
    assert_eq!(logged, "Withdrew the question: The config already says 10");
}
