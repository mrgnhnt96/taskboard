//! A task's reply that asks the owner something above its last paragraph, where the board doesn't
//! read it: the Stop is sent back once to ask it with `tb question` or a final Questions block. A
//! board setting (on by default) turns the check off.

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
    b.call("POST", "/tasks", json!({"title": "Cap the retries", "detail": "Do the thing.", "project": "webapp"}));
    b.report("tb.take", json!({"task": "T1"}));
    b
}

impl Board {
    fn call(&self, method: &str, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, method, path, &Query::new(), &body).unwrap_or_else(|e| panic!("{method} {path}: {}", e.message))
    }
    fn report(&self, event: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {"branch": "feature/x", "sha": "abc123"}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message))
    }
    fn stop(&self, reply: &str, again: bool) -> Value {
        self.report("hook.stop", json!({"last_message": reply, "last_message_end": reply, "reply": reply, "stop_hook_active": again}))
    }
}

const BURIED: &str = "I read the retry loop.\n\nShould I keep the old endpoint?\n\nI capped the retries at 3.";

#[test]
fn a_question_asked_mid_reply_is_sent_back_once() {
    let b = new_board();
    let out = b.stop(BURIED, false);
    let block = out["block"].as_str().expect("the stop is sent back");
    assert!(block.contains("above its last paragraph") && block.contains("Should I keep the old endpoint?"), "{block}");
    assert!(block.contains("question \"<the question>\" --option") && block.contains("Questions block"), "{block}");
    let t = board::get_task(&b.app, 1).unwrap();
    assert_eq!(t.s("status"), Some("working"), "it isn't put to the owner from the reply");
    assert_eq!(board::get_session(&b.app, Some("s1")).unwrap().unwrap().s("status"), Some("working"));

    let again = b.stop(BURIED, true);
    assert!(again.get("block").is_none(), "once per turn: {again}");
}

#[test]
fn the_whole_reply_is_read_past_the_hook_s_short_copy() {
    let b = new_board();
    let long = format!("Should I keep the old endpoint?\n\n{}\n\nDone.", "Words. ".repeat(500));
    let out = b.report("hook.stop", json!({"last_message": long.chars().take(20).collect::<String>(), "last_message_end": "Done.", "reply": long}));
    assert!(out["block"].is_string(), "{out}");
}

#[test]
fn a_reply_ending_on_its_question_or_asking_only_in_code_or_a_quote_is_not_sent_back() {
    let b = new_board();
    for reply in [
        "I capped the retries.\n\nI capped them at 3.",
        "```\nlet x = a?;\n```\n\nThe flag `--retry?` stays.\n\n> Should I keep it?\n\nI kept it.",
    ] {
        let out = b.stop(reply, false);
        assert!(out.get("block").is_none(), "{reply}: {out}");
    }
    let out = b.stop("I read the loop.\n\nShould the cap be 3 or 5 retries?", false);
    assert!(out.get("block").is_none(), "{out}");
}

#[test]
fn the_setting_turns_the_check_off() {
    let b = new_board();
    assert_eq!(b.call("GET", "/buried-questions", json!({}))["on"], true, "on by default");
    assert_eq!(b.call("POST", "/buried-questions", json!({"on": false}))["on"], false);
    assert!(b.stop(BURIED, false).get("block").is_none());
    assert_eq!(b.call("POST", "/buried-questions", json!({"on": "on"}))["on"], true);
    assert!(b.stop(BURIED, false)["block"].is_string());
}

#[test]
fn a_task_that_isn_t_working_is_not_sent_back() {
    let b = new_board();
    b.app.db.update("tasks", &json!(1), fields!["status" => "needs", "needs_reason" => "question", "question" => "Keep it?"]).unwrap();
    assert!(b.stop(BURIED, false).get("block").is_none());
}
