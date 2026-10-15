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

    assert!(midna::resume_as_is(&b.app, t, "c1").unwrap(), "the reopen's resume goes ahead as it is");
    assert!(midna::resume_as_is(&b.app, t, "c1").unwrap(), "reading the choice doesn't use it up");
    midna::used_resume_as_is(&b.app, t, "c1").unwrap();
    let ctx: Value = serde_json::from_str(board::get_task(&b.app, t).unwrap().s("context").unwrap()).unwrap();
    assert!(ctx.get("resume_as_is").is_none(), "the choice covers that one resume");
    assert!(!midna::resume_as_is(&b.app, t, "c1").unwrap(), "the next cold resume compacts again");
}

/// A cold resume as it is whose first `session.open` doesn't go through keeps the owner's choice, so the
/// retry resumes without compacting; the choice goes once a terminal opened.
#[test]
fn resume_as_is_survives_an_open_that_didn_t_go_through() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let midna_exe = dir.path().join("fake-midna");
    std::fs::write(
        &midna_exe,
        "#!/bin/sh\nd=$(dirname \"$0\")\nif [ \"$2\" = session.open ] && [ ! -f \"$d/opened-once\" ]; then touch \"$d/opened-once\"; echo 'no terminal' >&2; exit 1; fi\nif [ \"$2\" = session.open ]; then echo '{\"id\": \"S9\"}'; exit 0; fi\necho '{}'\n",
    )
    .unwrap();
    std::fs::set_permissions(&midna_exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.midna = midna_exe;
    let claude_exe = dir.path().join("fake-claude");
    std::fs::write(&claude_exe, "#!/bin/sh\ntouch \"$(dirname \"$0\")/compacted\"\nexit 1\n").unwrap();
    std::fs::set_permissions(&claude_exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.claude = claude_exe.to_string_lossy().to_string();
    let file = taskboardd::transcript::transcript_file(&cfg.claude_projects, &repo.to_string_lossy(), "c1");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, json!({"type": "user", "message": {"content": "hi"}, "timestamp": "2000-01-01T00:00:00Z"}).to_string()).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    let b = Board { app, _dir: dir };

    let t = b.post("/tasks", json!({"title": "Fix the tab bar", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
    let mut ctx = board::task_context(&board::get_task(&b.app, t).unwrap());
    ctx.insert("resume_as_is".into(), json!("c1"));
    board::save_context(&b.app, t, &ctx, false).unwrap();
    let jid = board::create_job(&b.app, "agent", json!({"flags": "--resume c1", "cwd": repo.to_string_lossy()}), Some(t), "start", None).unwrap();
    let job = || b.app.db.q1("SELECT * FROM jobs WHERE id = ?", p![jid]).unwrap().unwrap();

    midna::run_job(&b.app, &job());
    assert!(midna::resume_as_is(&b.app, t, "c1").unwrap(), "the open didn't go through, so the choice stays");
    b.app.db.x("UPDATE jobs SET state = 'pending' WHERE id = ?", p![jid]).unwrap();
    midna::run_job(&b.app, &job());
    let target: Value = serde_json::from_str(job().s("target").unwrap_or("{}")).unwrap();
    assert_eq!(target["session"], "S9", "the retry opened a terminal");
    assert!(!b._dir.path().join("compacted").exists(), "neither run compacted the conversation");
    assert!(!midna::resume_as_is(&b.app, t, "c1").unwrap(), "the choice is used up once a terminal opened");
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
