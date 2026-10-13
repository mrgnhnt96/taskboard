//! A cold conversation is compacted headless before a new terminal resumes it. A failed compact holds the
//! task for the owner instead of resuming it at full size, and the card shows the compact while it runs.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, dispatch, fields, jobs, midna, p};

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
    Board { app, _dir: dir }
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn start_job(&self, t: i64, state: &str, target: Option<Value>) -> i64 {
        let jid = board::create_job(&self.app, "agent", json!({"flags": "--resume c1", "cwd": "/tmp"}), Some(t), "start", target).unwrap();
        self.app.db.x("UPDATE jobs SET state = ? WHERE id = ?", p![state, jid]).unwrap();
        board::update_task(&self.app, t, fields!["status" => "queued", "start_job" => jid, "claude_session_id" => "c1"]).unwrap();
        jid
    }
}

#[test]
fn a_failed_compact_holds_the_task_for_the_owner_and_reopening_resumes_it_as_it_is() {
    let b = new_board();
    let t = b.post("/tasks", json!({"title": "Fix the tab bar", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
    let jid = b.start_job(t, "running", None);
    let why = format!("{}: claude exited 1", midna::COMPACT_FAILED);
    jobs::result(&b.app, jid, false, "", &why, 1).unwrap();

    let task = board::get_task(&b.app, t).unwrap();
    assert_eq!((task.s("status"), task.s("needs_reason")), (Some("needs"), Some("compact_failed")));
    assert!(task.s("retry_at").is_none(), "no retry: resuming at full size is what the compact avoids");
    let alerts: Vec<String> = dispatch::alerts(&b.app).iter().map(|a| a["text"].as_str().unwrap().to_string()).collect();
    assert!(alerts.iter().any(|a| a.contains("claude exited 1") && a.contains("Resume it as it is, or start fresh with the handoff")), "{alerts:?}");

    b.post(&format!("tasks/T{t}/resume"), json!({"mode": "reopen"}));
    let ctx: Value = serde_json::from_str(board::get_task(&b.app, t).unwrap().s("context").unwrap()).unwrap();
    assert_eq!(ctx["resume_as_is"], "c1", "the reopen skips the compact for this conversation");
}

#[test]
fn the_card_shows_the_headless_compact_while_it_runs() {
    let b = new_board();
    let t = b.post("/tasks", json!({"title": "Fix the tab bar", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
    b.start_job(t, "running", Some(json!({"compacting_at": "2026-10-12T10:00:00Z"})));
    assert_eq!(b.get(&format!("tasks/T{t}"))["compacting"], "2026-10-12T10:00:00Z");
    b.start_job(t, "running", Some(json!({"compacting_at": null})));
    assert_eq!(b.get(&format!("tasks/T{t}"))["compacting"], Value::Null);
}
