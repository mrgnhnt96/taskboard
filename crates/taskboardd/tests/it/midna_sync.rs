//! One pass of the Midna sync against a fake `midna` CLI: a close-event read that fails doesn't
//! stop the terminal list from being read.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde_json::json;
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna};

/// A `midna` that answers by method: `events` for `events.list` (a shell snippet), one Claude
/// terminal for `session.list`, and `{}` for anything else.
fn fake_midna(dir: &Path, events: &str) -> std::path::PathBuf {
    let exe = dir.join("midna");
    let sessions = json!([{"id": "s1", "name": "Term s1", "agent": "claude", "cwd": dir.to_string_lossy(), "status": {"state": "working"}}]);
    let script = format!(
        "#!/bin/sh\ncase \"$2\" in\n  events.list) {events} ;;\n  session.list) echo '{sessions}' ;;\n  project.list) echo '[]' ;;\n  *) echo '{{}}' ;;\nesac\n"
    );
    std::fs::write(&exe, script).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    exe
}

fn board_with(events: &str) -> (std::sync::Arc<App>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.midna = fake_midna(dir.path(), events);
    cfg.runner = false;
    (App::for_tests(cfg), dir)
}

#[test]
fn a_failed_close_event_read_still_reads_the_terminals() {
    for events in ["echo 'midna: unknown method events.list' >&2; exit 2", "echo 'midna: events.list refused' >&2; exit 1"] {
        let (app, _d) = board_with(events);
        midna::sync_once(&app).unwrap_or_else(|e| panic!("{events}: {e}"));
        let s = board::get_session(&app, Some("s1")).unwrap().expect("the terminal list was read");
        assert_eq!(s.s("status"), Some("working"));
    }
}

#[test]
fn a_close_event_read_that_works_keeps_its_place() {
    let (app, _d) = board_with(r#"echo '[{"seq": 7, "kind": "session.opened"}]'"#);
    midna::sync_once(&app).unwrap();
    assert_eq!(app.db.get_setting("midna_event_seq").unwrap().as_deref(), Some("7"));
    assert!(board::get_session(&app, Some("s1")).unwrap().is_some());
}

/// A task waiting on its terminal for `reason` since 10:00:00, as Midna's sync finds it.
fn waiting_task(app: &App, reason: &str) -> i64 {
    let id = app.db.insert("tasks", taskboardd::fields!["title" => "Fix", "project" => "webapp", "status" => "working", "session_id" => "s1"]).unwrap();
    app.db.x("UPDATE tasks SET status = 'needs', needs_reason = ?, question = 'Allow Bash?' WHERE id = ?", taskboardd::p![reason, id]).unwrap();
    board::log_event_full(app, id, "Term s1", "question", "Waiting for you: Allow Bash?", None, Some("2026-10-23T10:00:00Z")).unwrap();
    id
}

fn sync_state(app: &App, state: &str, since: &str) {
    let s = json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": "/tmp", "status": {"state": state, "since": since}});
    midna::sync(app, &[s], &[]).unwrap();
}

#[test]
fn a_terminal_that_carries_on_by_itself_takes_its_task_out_of_needs_you() {
    let (app, _d) = board_with("echo '[]'");
    let t = waiting_task(&app, "attention");
    sync_state(&app, "working", "2026-10-23T09:59:58Z");
    assert_eq!(board::get_task(&app, t).unwrap().s("status"), Some("needs"), "working since before the prompt is an old reading");
    sync_state(&app, "needs_you", "2026-10-23T10:00:00Z");
    assert_eq!(board::get_task(&app, t).unwrap().s("status"), Some("needs"));
    sync_state(&app, "working", "2026-10-23T10:00:40Z");
    let row = board::get_task(&app, t).unwrap();
    assert_eq!((row.s("status"), row.s("question")), (Some("working"), None), "approved in the terminal");
    let said = app.db.val("SELECT text FROM events WHERE task_id = ? ORDER BY id DESC LIMIT 1", taskboardd::p![t]).unwrap();
    assert_eq!(said, json!("Back to work: its terminal carried on"));

    let api = waiting_task(&app, "api_error");
    app.db.x("UPDATE tasks SET status = 'done' WHERE id = ?", taskboardd::p![t]).unwrap();
    sync_state(&app, "working", "2026-10-23T10:05:00Z");
    assert_eq!(board::get_task(&app, api).unwrap().s("status"), Some("working"), "past its API error by itself");
}

#[test]
fn a_question_for_the_owner_stays_while_the_terminal_works() {
    let (app, _d) = board_with("echo '[]'");
    let t = waiting_task(&app, "question");
    sync_state(&app, "working", "2026-10-23T10:05:00Z");
    assert_eq!(board::get_task(&app, t).unwrap().s("status"), Some("needs"));
}

#[test]
fn a_sync_that_finds_nothing_new_writes_nothing() {
    let (app, _d) = board_with("echo '[]'");
    sync_state(&app, "working", "2026-10-23T10:00:00Z");
    let mark = app.db.mark().unwrap();
    sync_state(&app, "working", "2026-10-23T10:00:00Z");
    assert_eq!(app.db.mark().unwrap(), mark, "a heartbeat with nothing new leaves the board as it was");

    sync_state(&app, "needs_you", "2026-10-23T10:01:00Z");
    assert_ne!(app.db.mark().unwrap(), mark);
    assert_eq!(board::get_session(&app, Some("s1")).unwrap().unwrap().s("status"), Some("needs"));
}
