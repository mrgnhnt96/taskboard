//! A turn that ends saying its task waits on other board tasks, without `tb wait-for`: the Stop is sent
//! back once with the exact line, the board records the wait when it ends that way again, and it never
//! sits on the owner.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, fields, midna, reports, waitsfor};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let (web, api_dir) = (dir.path().join("webapp"), dir.path().join("api"));
    std::fs::create_dir_all(&web).unwrap();
    std::fs::create_dir_all(&api_dir).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting(
            "midna_projects",
            Some(&json!([{"name": "webapp", "path": web.to_string_lossy()}, {"name": "api", "path": api_dir.to_string_lossy()}]).to_string()),
        )
        .unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": web.to_string_lossy(), "status": {"state": "working"}})], &[])
        .unwrap();
    let b = Board { app, _dir: dir };
    b.call("POST", "/tasks", json!({"title": "Cap the retries", "detail": "Do the thing.", "project": "webapp"}));
    b.call("POST", "/tasks", json!({"title": "Add the endpoint", "detail": "Blocks T1.", "project": "webapp"}));
    b.call("POST", "/tasks", json!({"title": "Report the SHA", "detail": "Report it on T1.", "project": "webapp"}));
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
    fn task(&self, n: i64) -> taskboardd::util::Row {
        board::get_task(&self.app, n).unwrap()
    }
}

const WAITING: &str = "I started both.\n\nT1 is now waiting for two results: T2 (the endpoint) and T3 (the SHA). Once both are in, I'll wire them up.";

#[test]
fn a_reply_that_says_it_waits_is_sent_back_with_the_wait_for_line() {
    let b = new_board();
    let out = b.stop(WAITING, false);
    let block = out["block"].as_str().expect("the stop is sent back");
    assert!(block.contains("tb wait-for T2 T3 --why"), "{block}");
    assert!(!block.contains("--merged"), "{block}");
    let t = b.task(1);
    assert_eq!(t.s("status"), Some("working"), "it isn't put to the owner");
    assert_eq!(board::get_session(&b.app, Some("s1")).unwrap().unwrap().s("status"), Some("working"));
}

#[test]
fn ending_again_still_waiting_records_the_wait() {
    let b = new_board();
    b.stop(WAITING, false);
    let out = b.stop(WAITING, true);
    assert!(out.get("block").is_none(), "{out}");
    let t = b.task(1);
    assert_eq!(t.s("status"), Some("queued"), "parked on the tasks it waits for");
    assert_eq!(waitsfor::ids(&t), vec![2, 3]);
    assert!(waitsfor::parked(&t).is_some());
}

#[test]
fn a_reply_that_runs_the_line_is_left_alone() {
    let b = new_board();
    b.stop(WAITING, false);
    b.report("tb.wait_for", json!({"tasks": "T2 T3", "why": "the endpoint and the SHA"}));
    assert_eq!(b.task(1).s("status"), Some("queued"));
    let out = b.stop(WAITING, true);
    assert!(out.get("block").is_none(), "{out}");
}

#[test]
fn needing_it_merged_adds_merged_and_counts_a_done_unmerged_task() {
    let b = new_board();
    board::update_task(&b.app, 2, fields!["status" => "done", "pr_num" => 7, "pr_state" => "OPEN"]).unwrap();
    let out = b.stop("Waiting for T2's PR to be merged before I rebase onto it.", false);
    let block = out["block"].as_str().expect("sent back");
    assert!(block.contains("tb wait-for T2 --merged --why"), "{block}");

    board::update_task(&b.app, 2, fields!["pr_state" => "MERGED"]).unwrap();
    let out = b.stop("Waiting for T2's PR to be merged before I rebase onto it.", false);
    assert!(out.get("block").is_none(), "merged already: {out}");
}

#[test]
fn done_tasks_and_plain_mentions_are_not_waits() {
    let b = new_board();
    board::update_task(&b.app, 2, fields!["status" => "done"]).unwrap();
    for reply in ["Waiting for T2 to finish, then I'll rebase.", "I fixed what T3 reported.\n\nThe retries are capped at 3.", "Waiting on T99 first."] {
        let out = b.stop(reply, false);
        assert!(out.get("block").is_none(), "{reply}: {out}");
    }
    assert_eq!(b.task(1).s("status"), Some("working"));
}

#[test]
fn a_task_in_a_project_it_may_not_wait_on_gets_the_refusal() {
    let b = new_board();
    b.call("POST", "/tasks", json!({"title": "Other side", "detail": "x", "project": "api"}));
    let out = b.stop("Blocked on T4 until it lands.", false);
    let block = out["block"].as_str().expect("sent back");
    assert!(block.contains("T4") && block.contains("--waits-on api"), "{block}");
    assert!(!block.contains("wait-for T4"), "{block}");
}

#[test]
fn an_idle_prompt_on_a_waiting_reply_records_the_wait_not_an_owner_question() {
    let b = new_board();
    let dir = b.app.cfg.claude_projects.join("webapp");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("c-s1.jsonl");
    std::fs::write(&path, json!({"type": "assistant", "message": {"content": [{"type": "text", "text": WAITING}]}}).to_string()).unwrap();
    b.report("hook.attention", json!({"notification_type": "idle_prompt", "message": "Claude is waiting for your input", "transcript_path": path.to_string_lossy()}));
    let t = b.task(1);
    assert_eq!(t.s("status"), Some("queued"), "{:?}", t.s("question"));
    assert_eq!(waitsfor::ids(&t), vec![2, 3]);
}
