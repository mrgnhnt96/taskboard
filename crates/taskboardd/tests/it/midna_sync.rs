//! One pass of the Midna sync against a fake `midna` CLI: a close-event read that fails doesn't
//! stop the terminal list from being read.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde_json::json;
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{iso, now_ts, RowExt};
use taskboardd::{board, midna, projects, reports};

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

const DAY: f64 = 86400.0;

/// A terminal only hooks reported, in the leftover folder 'w', last heard from `days` ago.
fn hook_terminal(app: &App, sid: &str, days: f64) {
    let at = iso(now_ts() - days * DAY);
    app.db
        .insert(
            "sessions",
            taskboardd::fields!["id" => sid, "status" => "idle", "source" => "hook", "agent" => "claude", "project" => "w",
                                "project_path" => "/private/tmp/mh.yqIF/w", "last_activity" => at, "seen_at" => at],
        )
        .unwrap();
}

fn status_of(app: &App, sid: &str) -> String {
    board::get_session(app, Some(sid)).unwrap().unwrap().st("status")
}

fn lists_project(app: &App, name: &str) -> bool {
    projects::list_projects(app).unwrap().iter().any(|p| p["name"] == name)
}

#[test]
fn a_stale_hook_terminal_goes_and_takes_its_project_with_it() {
    let (app, _d) = board_with("echo '[]'");
    hook_terminal(&app, "h1", 8.0);
    assert!(lists_project(&app, "w"));

    let (gone, _) = midna::sync(&app, &[], &[]).unwrap();

    assert_eq!(gone, ["h1"]);
    let s = board::get_session(&app, Some("h1")).unwrap().expect("the row is kept");
    assert_eq!(s.s("status"), Some("gone"));
    assert!(s.s("gone_at").is_some());
    assert!(!lists_project(&app, "w"));
}

#[test]
fn a_hook_terminal_heard_from_this_week_stays() {
    let (app, _d) = board_with("echo '[]'");
    hook_terminal(&app, "h1", 6.0);
    midna::sync(&app, &[], &[]).unwrap();
    assert_eq!(status_of(&app, "h1"), "idle");
    assert!(lists_project(&app, "w"));
}

#[test]
fn a_late_report_of_old_activity_counts_as_hearing_from_the_terminal() {
    let (app, _d) = board_with("echo '[]'");
    hook_terminal(&app, "h1", 8.0);
    app.db.x("UPDATE sessions SET seen_at = ? WHERE id = 'h1'", taskboardd::p![iso(now_ts() - DAY)]).unwrap();
    midna::sync(&app, &[], &[]).unwrap();
    assert_eq!(status_of(&app, "h1"), "idle");
}

#[test]
fn a_hook_terminal_with_no_time_on_it_stays() {
    let (app, _d) = board_with("echo '[]'");
    hook_terminal(&app, "h1", 8.0);
    app.db.x("UPDATE sessions SET last_activity = NULL, seen_at = NULL WHERE id = 'h1'", taskboardd::p![]).unwrap();
    midna::sync(&app, &[], &[]).unwrap();
    assert_eq!(status_of(&app, "h1"), "idle");
}

#[test]
fn a_stale_hook_terminal_midna_lists_stays() {
    let listed = [
        ("an agent terminal", json!({"id": "h1", "name": "Term h1", "agent": "claude", "cwd": "/tmp", "status": {"state": "idle"}})),
        ("a plain shell", json!({"id": "h1", "name": "Term h1", "agent": "", "cwd": "/tmp", "status": {"state": "idle"}})),
    ];
    for (what, s) in listed {
        let (app, _d) = board_with("echo '[]'");
        hook_terminal(&app, "h1", 8.0);
        let (gone, _) = midna::sync(&app, &[s], &[]).unwrap();
        assert!(gone.is_empty(), "{what}");
        assert_eq!(status_of(&app, "h1"), "idle", "{what}");
    }
}

#[test]
fn the_idle_limit_comes_from_the_config() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.intervals.hook_gone_after_idle = DAY;
    cfg.runner = false;
    let app = App::for_tests(cfg);
    hook_terminal(&app, "h1", 2.0);
    midna::sync(&app, &[], &[]).unwrap();
    assert_eq!(status_of(&app, "h1"), "gone");
}

#[test]
fn a_retired_terminal_that_fires_a_hook_comes_back() {
    let (app, _d) = board_with("echo '[]'");
    hook_terminal(&app, "h1", 8.0);
    midna::sync(&app, &[], &[]).unwrap();
    assert_eq!(status_of(&app, "h1"), "gone");

    let hook = json!({"event": "hook.stop", "session": "h1", "claude_session": "c-h1", "cwd": "", "git": {}, "last_message": "still here"});
    reports::handle(&app, hook, false).unwrap();

    let s = board::get_session(&app, Some("h1")).unwrap().unwrap();
    assert_ne!(s.s("status"), Some("gone"));
    assert_eq!(s.s("gone_at"), None);
    assert!(lists_project(&app, "w"));
    midna::sync(&app, &[], &[]).unwrap();
    assert_ne!(status_of(&app, "h1"), "gone", "its new activity keeps it");
}
