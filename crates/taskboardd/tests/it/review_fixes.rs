//! A kept review's suggested fixes: Claude's answer filed as untriaged backlog issues tagged with the
//! review, fixes the backlog already has linked instead of written twice, at most five.

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{now_ts, RowExt};
use taskboardd::{fields, review, review_fixes};

fn new_board() -> (tempfile::TempDir, std::sync::Arc<App>) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": "/tmp/webapp"}]).to_string())).unwrap();
    (dir, app)
}

fn call(app: &App, method: &str, path: &str) -> Result<Value, u16> {
    api::dispatch(app, method, path, &Query::new(), &json!({})).map_err(|e| e.status)
}

fn fix(title: &str) -> Value {
    json!({"action": "new", "project": "webapp", "kind": "gap", "title": title, "change": format!("{title}."),
           "evidence": "cargo test ran 40 times at a 3 min median.", "saves_minutes": 90})
}

#[test]
fn fixes_are_filed_untriaged_and_tagged_with_their_review() {
    let (_d, app) = new_board();
    let id = review::save(&app, now_ts() - 7.0 * 86400.0, now_ts(), true).unwrap();
    let answer = json!({"fixes": [fix("Run the integration tests as one binary"), fix("Share a build cache across worktrees")]});
    let v = review_fixes::apply(&app, id, &answer).unwrap();
    assert_eq!(v["filed"].as_array().unwrap().len(), 2);
    let rows = app.db.q("SELECT * FROM issues WHERE review_id = ? AND state = 'open' AND goal_id IS NULL", vec![json!(id)]).unwrap();
    assert_eq!(rows.len(), 2);
    let b = &rows[0];
    let detail = b.st("detail");
    assert!(detail.contains("Evidence: cargo test ran 40 times"), "{detail}");
    assert!(detail.contains("Estimated time saved: about 1.5 h a week."), "{detail}");
    assert_eq!(b.st("source"), "review");
}

#[test]
fn an_open_fix_is_linked_not_written_twice() {
    let (_d, app) = new_board();
    let old = app.db.insert("issues", fields!["project" => "webapp", "kind" => "gap", "title" => "Share a build cache across worktrees", "state" => "open"]).unwrap();
    let id = review::save(&app, now_ts() - 7.0 * 86400.0, now_ts(), true).unwrap();
    let answer = json!({"fixes": [
        {"action": "link", "issue": old, "project": "webapp", "kind": "gap", "title": "Cache builds", "change": "Use sccache.", "evidence": "", "saves_minutes": 60},
        fix("Share a build cache across worktrees"),
        {"action": "link", "issue": 999, "project": "webapp", "kind": "gap", "title": "Gone", "change": "Nothing.", "evidence": "", "saves_minutes": 0},
    ]});
    let v = review_fixes::apply(&app, id, &answer).unwrap();
    assert_eq!(v["linked"], json!([format!("B{old}")]));
    assert_eq!(v["filed"], json!([]));
    assert_eq!(v["skipped"], 2);
    assert_eq!(app.db.count("SELECT COUNT(*) FROM issues", vec![]).unwrap(), 1);
    let note = app.db.q1("SELECT text FROM issue_events WHERE issue_id = ? ORDER BY id DESC", vec![json!(old)]).unwrap().unwrap();
    assert!(note.st("text").starts_with(&format!("Suggested again by review {id}")), "{}", note.st("text"));
}

#[test]
fn at_most_five_and_only_known_projects() {
    let (_d, app) = new_board();
    let id = review::save(&app, now_ts() - 7.0 * 86400.0, now_ts(), true).unwrap();
    let mut fixes = vec![json!({"action": "new", "project": "nowhere", "kind": "gap", "title": "Elsewhere", "change": "x", "evidence": "", "saves_minutes": 5})];
    fixes.extend((1..=7).map(|n| fix(&format!("Distinct fix number {n} for {}", ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf"][n - 1]))));
    let v = review_fixes::apply(&app, id, &json!({"fixes": fixes})).unwrap();
    assert_eq!(v["filed"].as_array().unwrap().len(), review_fixes::MAX);
    assert_eq!(v["skipped"], 3);
    assert_eq!(app.db.count("SELECT COUNT(*) FROM issues WHERE project = 'nowhere'", vec![]).unwrap(), 0);
}

#[test]
fn the_review_shows_its_fixes_and_counts_them() {
    let (_d, app) = new_board();
    let id = review::save(&app, now_ts() - 7.0 * 86400.0, now_ts(), true).unwrap();
    let v = review_fixes::apply(&app, id, &json!({"fixes": [fix("Run the integration tests as one binary")]})).unwrap();
    let mut st = v.clone();
    st["state"] = json!("done");
    app.db.x("UPDATE reviews SET fixes = ? WHERE id = ?", vec![json!(st.to_string()), json!(id)]).unwrap();
    let r = call(&app, "GET", &format!("/reviews/{id}")).unwrap();
    assert_eq!(r["fix_issues"].as_array().unwrap().len(), 1);
    assert_eq!(r["fix_issues"][0]["linked"], false);
    assert!(r["headline"].as_str().unwrap().ends_with("1 fix suggested"), "{}", r["headline"]);
}

#[test]
fn running_it_by_hand_needs_claude() {
    let (_d, app) = new_board();
    let id = review::save(&app, now_ts() - 7.0 * 86400.0, now_ts(), true).unwrap();
    assert_eq!(call(&app, "POST", &format!("/reviews/{id}/fixes")).unwrap_err(), 409);
    assert_eq!(call(&app, "GET", &format!("/reviews/{id}")).unwrap()["fixes"]["state"], "none");
}

/// A board whose `claude` prints the `answer` file beside it, and fails while there's none.
fn board_with_claude() -> (tempfile::TempDir, std::sync::Arc<App>, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let answer = dir.path().join("answer");
    let script = dir.path().join("claude");
    std::fs::write(&script, format!("#!/bin/sh\ncat '{}' || exit 1\n", answer.display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.claude = script.to_string_lossy().to_string();
    cfg.backlog.ai = true;
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": "/tmp/webapp"}]).to_string())).unwrap();
    (dir, app, answer)
}

fn answers(path: &std::path::Path, fixes: Vec<Value>) {
    std::fs::write(path, json!({"structured_output": {"fixes": fixes}}).to_string()).unwrap();
}

#[test]
fn file_again_after_a_failure_files_them_once() {
    let (_d, app, answer) = board_with_claude();
    let id = review::save(&app, now_ts() - 7.0 * 86400.0, now_ts(), true).unwrap();
    let r = call(&app, "POST", &format!("/reviews/{id}/fixes")).unwrap();
    assert_eq!(r["fixes"]["state"], "failed", "{}", r["fixes"]);
    assert_eq!(r["fix_issues"], json!([]));

    answers(&answer, vec![fix("Run the integration tests as one binary"), fix("Share a build cache across worktrees")]);
    let r = call(&app, "POST", &format!("/reviews/{id}/fixes")).unwrap();
    assert_eq!(r["fixes"]["state"], "done", "{}", r["fixes"]);
    assert_eq!(r["fix_issues"].as_array().unwrap().len(), 2);

    // Filing again, even after one was closed, files nothing twice and keeps what it filed.
    let first = r["fix_issues"][0]["id"].as_i64().unwrap();
    app.db.x("UPDATE issues SET state = 'done' WHERE id = ?", vec![json!(first)]).unwrap();
    answers(&answer, vec![fix("Run the integration tests as one binary")]);
    let r = call(&app, "POST", &format!("/reviews/{id}/fixes")).unwrap();
    assert_eq!(r["fixes"]["filed"].as_array().unwrap().len(), 2, "{}", r["fixes"]);
    assert_eq!(r["fixes"]["linked"], json!([]));
    assert_eq!(app.db.count("SELECT COUNT(*) FROM issues WHERE review_id = ?", vec![json!(id)]).unwrap(), 2);

    // A failed rerun keeps what earlier runs filed.
    std::fs::remove_file(&answer).unwrap();
    let r = call(&app, "POST", &format!("/reviews/{id}/fixes")).unwrap();
    assert_eq!(r["fixes"]["state"], "failed");
    assert_eq!(r["fix_issues"].as_array().unwrap().len(), 2);
}

#[test]
fn a_run_cut_short_by_a_restart_reads_as_failed_and_files_again() {
    let (_d, app, answer) = board_with_claude();
    let id = review::save(&app, now_ts() - 7.0 * 86400.0, now_ts(), true).unwrap();
    // Left running by an earlier board process, before the owner was told.
    let stale = json!({"state": "running", "at": "2026-11-01T00:00:00Z", "run": "1-2026-11-01T00:00:00Z", "announce": true});
    app.db.x("UPDATE reviews SET fixes = ? WHERE id = ?", vec![json!(stale.to_string()), json!(id)]).unwrap();
    let st = review_fixes::status(&app, id).unwrap();
    assert_eq!(st["state"], "failed");
    assert_eq!(st["error"], "the board restarted before they were filed");
    assert!(taskboardd::dispatch::alerts(&app).is_empty(), "the owner hasn't been told yet");

    answers(&answer, vec![fix("Share a build cache across worktrees")]);
    let r = call(&app, "POST", &format!("/reviews/{id}/fixes")).unwrap();
    assert_eq!(r["fixes"]["state"], "done", "{}", r["fixes"]);
    assert_eq!(r["fix_issues"].as_array().unwrap().len(), 1);
    // The owner is told now, as the first run would have.
    assert!(taskboardd::dispatch::alerts(&app).iter().any(|a| a["review_id"] == id), "the review's Needs you item is up");
}
