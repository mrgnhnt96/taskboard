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
