//! `tb submit --fixes B<n>`: the first submit lists the open backlog issues about what the task changed,
//! once; the issues it names close as fixed when its PR merges, or as it finishes with no PR.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, midna, prflow, reports};

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
    midna::sync(&app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    Board { app, _dir: dir }
}

impl Board {
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    /// A task in area Quill, taken by terminal s1.
    fn task(&self) -> i64 {
        let body = json!({"title": "Sign-in screen", "detail": "Do it.", "project": "webapp"});
        let id = api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &body).unwrap()["id"].as_i64().unwrap();
        self.app.db.update("tasks", &json!(id), vec![("area", json!("Quill"))]).unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")})).unwrap();
        id
    }
    fn issue(&self, project: &str, area: &str, title: &str) -> i64 {
        self.app.db.insert("issues", vec![("project", json!(project)), ("area", json!(area)), ("kind", json!("bug")), ("title", json!(title)), ("state", json!("open"))]).unwrap()
    }
    fn issue_row(&self, id: i64) -> Row {
        board::get_issue(&self.app, id).unwrap()
    }
    fn closed_line(&self, id: i64) -> Option<String> {
        self.app.db.q1("SELECT text FROM issue_events WHERE issue_id = ? AND kind = 'drop'", taskboardd::p![id]).unwrap().map(|r| r.st("text"))
    }
}

#[test]
fn the_first_submit_lists_related_issues_once_and_a_named_fix_closes_with_no_pr() {
    let b = new_board();
    let id = b.task();
    let quill = b.issue("webapp", "Quill", "Quill drops the cursor");
    let other = b.issue("webapp", "Art", "Art export is slow");
    let elsewhere = b.issue("mobile", "Quill", "Quill on the phone");

    let (code, why) = b.report("tb.done", json!({"summary": "Done", "no_pr": "nothing to ship"})).unwrap_err();
    assert_eq!(code, 409, "{why}");
    assert!(why.contains(&format!("B{quill} · Quill drops the cursor (area Quill)")), "{why}");
    assert!(!why.contains(&format!("B{other} ")) && !why.contains(&format!("B{elsewhere} ")), "only the task's own area and project: {why}");
    assert!(why.contains("--fixes B<n>"), "{why}");

    let (code, why) = b.report("tb.done", json!({"summary": "Done", "no_pr": "nothing to ship", "fixes": [format!("B{elsewhere}")]})).unwrap_err();
    assert_eq!(code, 400, "another project's issue: {why}");

    b.report("tb.done", json!({"summary": "Done", "no_pr": "nothing to ship", "fixes": [format!("B{quill}")]})).unwrap();
    assert_eq!(b.issue_row(quill).st("state"), "drop");
    assert_eq!(b.closed_line(quill).unwrap(), format!("Closed as fixed by T{id}, which finished without a PR"));
    assert_eq!(b.issue_row(other).st("state"), "open");
}

#[test]
fn asked_once_the_same_submit_goes_through_and_closes_nothing() {
    let b = new_board();
    b.task();
    let quill = b.issue("webapp", "Quill", "Quill drops the cursor");
    b.report("tb.done", json!({"summary": "Done", "no_pr": "nothing to ship"})).unwrap_err();
    b.report("tb.done", json!({"summary": "Done", "no_pr": "nothing to ship"})).unwrap();
    assert_eq!(b.issue_row(quill).st("state"), "open");
}

#[test]
fn a_named_fix_waits_for_the_pr_to_merge() {
    let b = new_board();
    let id = b.task();
    let quill = b.issue("webapp", "Quill", "Quill drops the cursor");
    b.report("tb.done", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9", "fixes": [format!("B{quill}")]})).unwrap();
    assert_eq!(b.issue_row(quill).st("state"), "open", "the PR hasn't merged yet");

    prflow::mark_merged(&b.app, &board::get_task(&b.app, id).unwrap(), "test").unwrap();
    assert_eq!(b.issue_row(quill).st("state"), "drop");
    assert_eq!(b.closed_line(quill).unwrap(), format!("Closed as fixed by T{id} (PR #9)"));
    let q = b.issue_row(quill);
    assert_eq!((q.st("closed_as"), q.i("fixed_by"), q.st("fixed_pr")), ("fixed".to_string(), Some(id), "https://github.com/acme/webapp/pull/9".to_string()));
}
