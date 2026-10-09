//! `GET/POST /keep-awake` pass straight through to Midna's `keep_awake.status|set`, and the last
//! answer rides along on `/state`. A fake `midna` script stands in for the CLI.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;

const STATUS: &str = r#"{"held":true,"reason":"work","line":"Keeping awake until 6 PM: 1 agent working","settings":{"enabled":true,"start":"09:00","end":"18:00","days":["mon","tue","wed","thu","fri"],"min_battery":20}}"#;

/// A `midna` that logs its args and answers `keep_awake.*` (or exits like the given code).
fn fake_midna(dir: &Path, body: &str) -> std::path::PathBuf {
    let exe = dir.join("midna");
    let log = dir.join("calls.log");
    std::fs::write(&exe, format!("#!/bin/sh\necho \"$2 $3\" >> '{}'\n{body}\n", log.display())).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    exe
}

fn board(body: &str) -> (std::sync::Arc<App>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.midna = fake_midna(dir.path(), body);
    (App::for_tests(cfg), dir)
}

fn call(app: &App, method: &str, body: Value) -> Result<Value, (u16, String)> {
    api::dispatch(app, method, "/keep-awake", &Query::new(), &body).map_err(|e| (e.status, e.message))
}

#[test]
fn reads_and_sets_through_midna() {
    let (app, dir) = board(&format!("echo '{STATUS}'"));
    let v = call(&app, "GET", json!({})).unwrap();
    assert_eq!(v["held"], json!(true));
    call(&app, "POST", json!({"days": "weekdays", "today": "until 5pm", "min_battery": 30})).unwrap();
    let log = std::fs::read_to_string(dir.path().join("calls.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines[0], "keep_awake.status {}");
    assert!(lines[1].starts_with("keep_awake.set "), "{log}");
    let sent: Value = serde_json::from_str(lines[1].trim_start_matches("keep_awake.set ")).unwrap();
    assert_eq!(sent, json!({"days": "weekdays", "today": "until 5pm", "min_battery": 30}));
    let state = api::dispatch(&app, "GET", "/state", &Query::new(), &json!({})).unwrap();
    assert_eq!(state["keep_awake"]["line"], json!("Keeping awake until 6 PM: 1 agent working"));
}

#[test]
fn midna_refusals_come_back_as_errors() {
    let (app, _d) = board("echo 'midna: keep_awake.set: min_battery should be 0-100' >&2; exit 2");
    let (code, msg) = call(&app, "POST", json!({"min_battery": 300})).unwrap_err();
    assert_eq!(code, 400);
    assert!(msg.contains("min_battery"), "{msg}");
}

#[test]
fn an_older_midna_says_to_update() {
    let (app, _d) = board("echo 'midna: unknown method `keep_awake.status`; call rpc.discover for the catalog' >&2; exit 2");
    let (code, msg) = call(&app, "GET", json!({})).unwrap_err();
    assert_eq!(code, 501);
    assert!(msg.contains("Update Midna"), "{msg}");
}

#[test]
fn midna_down() {
    let (app, _d) = board("exit 3");
    let (code, _) = call(&app, "GET", json!({})).unwrap_err();
    assert_eq!(code, 503);
}
