//! Alerts raised as free text get a short title and suggested fixes from Haiku, off the hot path; when
//! Haiku can't help, or the switch is off, they stay plain text.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{dispatch, suggest};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

/// A board whose `claude` is a script that prints `out`, with suggestions on or off.
fn board(out: &str, on: bool) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    let script = dir.path().join("claude");
    std::fs::write(&script, format!("#!/bin/sh\necho '{out}'\n")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.claude = script.to_string_lossy().to_string();
    cfg.alerts.suggest = on;
    let app = App::for_tests(cfg);
    let b = Board { app, _dir: dir };
    b.post("/tasks", json!({"title": "Cap the retries", "detail": "Do the thing.", "project": "webapp"}));
    b
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn alert(&self) -> Value {
        dispatch::alerts(&self.app).into_iter().next().unwrap()
    }
}

fn good() -> String {
    json!({"structured_output": {"title": "T1 asks for a retry cap", "detail": "The retry loop needs a cap before it ships.", "actions": [
        {"label": "Cap at 3 retries", "prompt": "Answer T1: cap the retries at 3."},
        {"label": "Cap at 5 retries", "prompt": "Answer T1: cap the retries at 5.", "command": "tb answer T1 5"}]}})
    .to_string()
}

#[test]
fn a_raised_alert_gets_a_title_and_fixes_that_are_prompts_not_commands() {
    let b = board(&good(), true);
    let raised = b.post("/alerts", json!({"text": "T1: should the retries cap at 3 or 5?", "task": "T1"}));
    assert_eq!(b.alert()["suggest"], "pending", "{raised}");
    suggest::tick(&b.app);
    let a = b.alert();
    assert_eq!(a["suggest"], "done");
    assert_eq!(a["title"], "T1 asks for a retry cap");
    assert_eq!(a["detail"], "The retry loop needs a cap before it ships.");
    assert_eq!(a["text"], "T1: should the retries cap at 3 or 5?", "the text still says it all");
    assert_eq!(a["project"], "webapp");
    let actions = a["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0], json!({"label": "Cap at 3 retries", "prompt": "Answer T1: cap the retries at 3.", "command": ""}));
    assert_eq!(actions[1]["command"], "", "Haiku's commands are never kept");
    // Asked once: another tick leaves it alone.
    suggest::tick(&b.app);
    assert_eq!(b.alert()["actions"], a["actions"]);
}

#[test]
fn an_agents_question_asks_for_fixes_and_the_boards_own_alerts_dont() {
    let b = board(&good(), true);
    dispatch::add_alert(&b.app, "T1 couldn't get the board's message.", Some(1), None, None, None).unwrap();
    assert!(b.alert()["suggest"].is_null());
    dispatch::add_free_alert(&b.app, "T1: should the retries cap at 3 or 5?", Some(1), None).unwrap();
    assert_eq!(b.alert()["suggest"], "pending");
}

#[test]
fn when_haiku_fails_or_answers_out_of_shape_the_alert_stays_plain_text() {
    for out in ["not json", r#"{"structured_output": {"title": "x", "detail": "", "actions": []}}"#] {
        let b = board(out, true);
        b.post("/alerts", json!({"text": "Something odd happened", "task": "T1"}));
        suggest::tick(&b.app);
        let a = b.alert();
        assert_eq!(a["suggest"], "failed", "{out}");
        assert!(a["title"].is_null() && a["actions"].is_null(), "{a}");
        assert_eq!(a["text"], "Something odd happened");
    }
}

#[test]
fn a_suggested_fix_opens_in_its_project_or_the_home_folder_when_it_has_none() {
    let b = board(&good(), true);
    b.post("/alerts", json!({"text": "T1: should the retries cap at 3 or 5?", "task": "T1"}));
    b.post("/alerts", json!({"text": "The disk is nearly full"}));
    suggest::tick(&b.app);
    let repo = b._dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    b.app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    b.app.shared.lock().midna_seen_at = Some(taskboardd::util::now_iso());
    let cwd = |text: &str| -> Value {
        let a = dispatch::alerts(&b.app).into_iter().find(|a| a["text"] == text).unwrap();
        assert_eq!(a["suggest"], "done", "{a}");
        let r = dispatch::act(&b.app, a["id"].as_str().unwrap(), 0).unwrap();
        let jid = r["job"].as_str().unwrap().trim_start_matches(|c: char| !c.is_ascii_digit()).parse::<i64>().unwrap();
        let j = b.app.db.q1("SELECT args FROM jobs WHERE id = ?", taskboardd::p![jid]).unwrap().unwrap();
        serde_json::from_str::<Value>(j["args"].as_str().unwrap()).unwrap()["cwd"].clone()
    };
    assert_eq!(cwd("T1: should the retries cap at 3 or 5?"), b._dir.path().join("webapp").to_string_lossy().as_ref());
    assert_eq!(cwd("The disk is nearly full"), dirs::home_dir().unwrap().to_string_lossy().as_ref());
}

#[test]
fn with_suggestions_off_nothing_is_asked() {
    let b = board(&good(), false);
    b.post("/alerts", json!({"text": "T1: should the retries cap at 3 or 5?", "task": "T1"}));
    suggest::tick(&b.app);
    let a = b.alert();
    assert!(a["suggest"].is_null() && a["actions"].is_null(), "{a}");
}
