//! `tb done` and PR bar leftovers from the Python board (issue #79): the `--no-pr` length, the ends-in-a-PR
//! guard when the remote can't be read, design links and local files in the PR body, and the PR bar's
//! review step before its first round and on tasks without a PR.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{PrLink, Row};
use taskboardd::{board, midna, prflow, propen, reports, steps};

const REVIEW: &str = r#"
[[steps]]
name = "Author review"
prompt = "Run the review"
check = "true"
bar = "WD"
"#;

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn new_board(config: &str) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    std::fs::write(&cfg.config_path, config).unwrap();
    // A plain folder, not a git checkout: the board can't tell whether it has a remote.
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    Board { app, _dir: dir }
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn task(&self, extra: Value) -> i64 {
        let mut body = json!({"title": "Sign-in screen", "detail": "Do it.", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", body)["id"].as_i64().unwrap()
    }
    fn take(&self, id: i64) {
        self.report("tb.take", json!({"task": format!("T{id}")})).unwrap();
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
}

#[test]
fn no_pr_keeps_to_a_hundred_characters() {
    let b = new_board("");
    let id = b.task(json!({}));
    b.take(id);
    let (code, why) = b.report("tb.done", json!({"summary": "Done", "no_pr": "x".repeat(101)})).unwrap_err();
    assert_eq!(code, 400, "{why}");
    assert!(why.contains("100 characters"), "{why}");
    b.report("tb.done", json!({"summary": "Done", "no_pr": "x".repeat(100)})).unwrap();
}

#[test]
fn a_task_that_ends_in_a_pr_needs_one_even_when_the_remote_cant_be_read() {
    let b = new_board("");
    let id = b.task(json!({"ships_pr": true}));
    b.take(id);
    let (code, why) = b.report("tb.done", json!({"summary": "Done"})).unwrap_err();
    assert_eq!(code, 409, "{why}");
    assert!(why.contains("ends in a PR"), "{why}");
}

#[test]
fn the_pr_body_lists_design_links_and_local_evidence() {
    let b = new_board("");
    let id = b.task(json!({}));
    b.post(&format!("/tasks/T{id}/attachments"), json!({"url": "https://figma.com/file/abc", "title": "Sign-in mock", "kind": "design"}));
    b.post(&format!("/tasks/T{id}/attachments"), json!({"url": "https://cdn.example/shot.png", "title": "Screenshot", "kind": "evidence"}));
    b.post(&format!("/tasks/T{id}/attachments"), json!({"url": "/Users/me/shots/after.png", "title": "After", "kind": "evidence"}));
    let ctx = propen::context_block(&b.app, &b.row(id)).unwrap();
    assert_eq!(
        ctx,
        "## Context\n- Evidence: [Screenshot](https://cdn.example/shot.png)\n- Evidence: after.png\n\n**Design**\n- [Sign-in mock](https://figma.com/file/abc)",
    );
}

#[test]
fn the_review_step_shows_before_its_first_round_on_tasks_that_end_in_a_pr() {
    let b = new_board(REVIEW);
    let id = b.task(json!({"ships_pr": true}));
    let wd = steps::bar_card(&b.app, &b.row(id)).unwrap();
    assert_eq!(wd, json!({"name": "Author review", "bar": "WD", "pending": true}));
    assert_eq!(board::task_card(&b.app, &b.row(id)).unwrap()["wd"]["pending"], true);
    // A task that doesn't end in a PR has no review step in its bar.
    let other = b.task(json!({"ships_pr": false}));
    assert_eq!(steps::bar_card(&b.app, &b.row(other)).unwrap(), Value::Null);
    // Once its PR is merged the step goes.
    let pr = PrLink { host: "github".into(), repo: "acme/webapp".into(), num: 5, url: "https://github.com/acme/webapp/pull/5".into() };
    let t = b.row(id);
    b.app.db.tx(|| prflow::link_pr(&b.app, &t, &pr, "test")).unwrap();
    assert_eq!(steps::bar_card(&b.app, &b.row(id)).unwrap()["pending"], true, "open: still shows");
    board::update_task(&b.app, id, vec![("pr_state", json!("MERGED")), ("pr_phase", json!("merged"))]).unwrap();
    assert_eq!(steps::bar_card(&b.app, &b.row(id)).unwrap(), Value::Null);
}
