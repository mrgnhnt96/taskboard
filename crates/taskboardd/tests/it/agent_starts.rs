//! `[agent_starts]`: the owner can let agents start tasks and run goals without their word, everywhere
//! or only in the terminal's own project.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::agent_starts::Mode;
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, p};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

/// A board with projects webapp and mobile, and terminal s1 ("Term") in webapp.
fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    f(&mut cfg);
    let (web, mobile) = (dir.path().join("webapp"), dir.path().join("mobile"));
    std::fs::create_dir_all(&web).unwrap();
    std::fs::create_dir_all(&mobile).unwrap();
    let app = App::for_tests(cfg);
    let projects = json!([{"name": "webapp", "path": web.to_string_lossy()}, {"name": "mobile", "path": mobile.to_string_lossy()}]);
    app.db.set_setting("midna_projects", Some(&projects.to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": web.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    Board { app, _dir: dir }
}

impl Board {
    fn task(&self, project: &str) -> i64 {
        api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &json!({"title": "Add login", "project": project})).unwrap()["id"].as_i64().unwrap()
    }
    fn goal(&self, project: &str) -> i64 {
        let g = api::dispatch(&self.app, "POST", "/goals", &Query::new(), &json!({"name": "Login", "project": project})).unwrap()["id"].as_i64().unwrap();
        api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &json!({"title": "Form", "project": project, "goal_id": g})).unwrap();
        g
    }
    fn tb_start(&self, id: i64) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", &format!("/tasks/T{id}/start"), &Query::new(), &json!({"mode": "queue", "via_session": "s1"}))
            .map_err(|e| (e.status, e.message))
    }
    fn tb_start_many(&self, ids: &[i64]) -> Result<Value, (u16, String)> {
        let tasks: Vec<String> = ids.iter().map(|i| format!("T{i}")).collect();
        api::dispatch(&self.app, "POST", "/tasks/start", &Query::new(), &json!({"tasks": tasks, "mode": "queue", "via_session": "s1"}))
            .map_err(|e| (e.status, e.message))
    }
    fn tb_run(&self, g: i64) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", &format!("/goals/G{g}/run"), &Query::new(), &json!({"via_session": "s1"})).map_err(|e| (e.status, e.message))
    }
    fn log(&self, id: i64) -> Vec<String> {
        self.app.db.q("SELECT text FROM events WHERE task_id = ? ORDER BY id", p![id]).unwrap().iter().map(|r| r.st("text")).collect()
    }
}

#[test]
fn by_default_an_agent_still_needs_the_word() {
    let b = board_with(|_| {});
    let id = b.task("webapp");
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);
    assert_eq!(b.tb_run(b.goal("webapp")).unwrap_err().0, 403);
}

#[test]
fn free_lets_an_agent_start_any_task_and_run_any_goal() {
    let b = board_with(|c| c.agent_starts.mode = Mode::Free);
    let id = b.task("mobile");
    assert!(b.tb_start(id).unwrap()["starting"] == true);
    assert!(b.log(id).contains(&"Started by Term".to_string()), "{:?}", b.log(id));
    let (one, two) = (b.task("webapp"), b.task("mobile"));
    assert_eq!(b.tb_start_many(&[one, two]).unwrap()["tasks"].as_array().unwrap().len(), 2);
    assert!(b.tb_run(b.goal("mobile")).is_ok());
}

#[test]
fn own_project_needs_the_word_only_for_other_projects() {
    let b = board_with(|c| c.agent_starts.mode = Mode::OwnProject);
    let mine = b.task("webapp");
    assert!(b.tb_start(mine).unwrap()["starting"] == true);
    let theirs = b.task("mobile");
    assert_eq!(b.tb_start(theirs).unwrap_err().0, 403);
    // One task it may not start holds the rest.
    let (a, c) = (b.task("webapp"), b.task("mobile"));
    assert_eq!(b.tb_start_many(&[a, c]).unwrap_err().0, 403);
    assert!(!b.log(a).iter().any(|l| l.starts_with("Started")), "{:?}", b.log(a));
    assert!(b.tb_run(b.goal("webapp")).is_ok());
    assert_eq!(b.tb_run(b.goal("mobile")).unwrap_err().0, 403);
}

#[test]
fn a_projects_own_mode_overrides_the_default() {
    let b = board_with(|c| {
        c.agent_starts.mode = Mode::Free;
        c.agent_starts.projects.insert("mobile".into(), Mode::Ask);
    });
    assert!(b.tb_start(b.task("webapp")).is_ok());
    assert_eq!(b.tb_start(b.task("mobile")).unwrap_err().0, 403);
    // A goal holding work in a project that asks needs the word.
    let g = b.goal("webapp");
    api::dispatch(&b.app, "POST", "/tasks", &Query::new(), &json!({"title": "App", "project": "mobile", "goal_id": g})).unwrap();
    assert_eq!(b.tb_run(g).unwrap_err().0, 403);
}

#[test]
fn a_caller_with_no_terminal_is_still_refused() {
    let b = board_with(|c| c.agent_starts.mode = Mode::Free);
    let id = b.task("webapp");
    let e = api::dispatch(&b.app, "POST", &format!("/tasks/T{id}/start"), &Query::new(), &json!({"mode": "queue"})).unwrap_err();
    assert_eq!(e.status, 403);
    assert_eq!(board::get_task(&b.app, id).unwrap().st("status"), "queued");
}
