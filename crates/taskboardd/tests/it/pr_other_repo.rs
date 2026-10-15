//! A task's PR has to be in its project's repo: `tb done` refuses a PR from another repo, and one an
//! agent mentions or pushes there isn't linked. A project with no remote links any PR, as before.

use std::process::Command;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board(remote: Option<&str>) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    if let Some(url) = remote {
        let git = |args: &[&str]| assert!(Command::new("git").arg("-C").arg(&repo).args(args).status().unwrap().success());
        git(&["init", "-q"]);
        git(&["remote", "add", "origin", url]);
    }
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    fn report(&self, event: &str, session: &str, extra: Value) -> Result<Value, String> {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| e.message)
    }
    fn taken(&self) -> i64 {
        let body = json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"});
        let id = api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &body).unwrap_or_else(|e| panic!("{}", e.message))["id"].as_i64().unwrap();
        let cwd = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": cwd, "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", "s1", json!({"task": format!("T{id}")})).unwrap();
        id
    }
}

#[test]
fn done_refuses_a_pr_from_another_repo_and_names_the_tasks_repo() {
    let b = board(Some("git@github.com:acme/webapp.git"));
    let id = b.taken();
    let why = b.report("tb.done", "s1", json!({"summary": "Done", "pr": "https://github.com/acme/other/pull/4"})).unwrap_err();
    assert!(why.contains("acme/other") && why.contains("acme/webapp"), "{why}");
    let t = board::get_task(&b.app, id).unwrap();
    assert_ne!(t.s("status"), Some("done"));
    assert_eq!(t.s("pr_url"), None);
}

#[test]
fn done_refuses_a_pr_from_another_repo_named_in_the_summary() {
    let b = board(Some("https://github.com/acme/webapp"));
    let id = b.taken();
    let why = b.report("tb.done", "s1", json!({"summary": "Opened https://github.com/acme/other/pull/4"})).unwrap_err();
    assert!(why.contains("acme/webapp"), "{why}");
    assert_ne!(board::get_task(&b.app, id).unwrap().s("status"), Some("done"));
}

#[test]
fn done_links_a_pr_from_the_projects_repo() {
    let b = board(Some("git@github.com:Acme/WebApp.git"));
    let id = b.taken();
    b.report("tb.done", "s1", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9"})).unwrap();
    let t = board::get_task(&b.app, id).unwrap();
    assert_eq!(t.s("pr_url"), Some("https://github.com/acme/webapp/pull/9"));
}

#[test]
fn a_pr_from_another_repo_in_a_turn_is_not_linked() {
    let b = board(Some("git@github.com:acme/webapp.git"));
    let id = b.taken();
    b.report("hook.stop", "s1", json!({"last_message": "Opened https://github.com/acme/other/pull/4 for it."})).unwrap();
    assert_eq!(board::get_task(&b.app, id).unwrap().s("pr_url"), None);
}

#[test]
fn a_project_with_no_remote_links_any_pr() {
    let b = board(None);
    let id = b.taken();
    b.report("tb.done", "s1", json!({"summary": "Done", "pr": "https://github.com/acme/other/pull/4"})).unwrap();
    assert_eq!(board::get_task(&b.app, id).unwrap().s("pr_url"), Some("https://github.com/acme/other/pull/4"));
}
