//! A goal holding tasks in more than one project: planned tasks name their own project (`::project=` or
//! `tb task new --goal --project`), the goal lists every project its tasks are in, and `GET goals` filtered
//! to a project finds the goals with work there.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, reports};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut projects = vec![];
    for name in ["webapp", "api"] {
        let repo = dir.path().join(name);
        std::fs::create_dir_all(&repo).unwrap();
        projects.push(json!({"name": name, "path": repo.to_string_lossy()}));
    }
    let app = App::for_tests(Config::for_tests(dir.path()));
    app.db.set_setting("midna_projects", Some(&json!(projects).to_string())).unwrap();
    Board { app, _dir: dir }
}

impl Board {
    fn goal(&self) -> i64 {
        api::dispatch(&self.app, "POST", "/goals", &Query::new(), &json!({"name": "Sign in", "project": "webapp"})).unwrap()["id"].as_i64().unwrap()
    }
    fn report(&self, body: Value) -> Result<Value, String> {
        reports::handle(&self.app, body, false).map_err(|e| e.message)
    }
    fn projects_of(&self, created: &Value) -> Vec<String> {
        created["created"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| board::get_task(&self.app, r.as_str().unwrap()[1..].parse().unwrap()).unwrap().st("project"))
            .collect()
    }
    fn goal_refs(&self, project: &str) -> Vec<String> {
        let mut q = Query::new();
        q.insert("project".into(), project.into());
        let v = api::dispatch(&self.app, "GET", "/goals", &q, &json!({})).unwrap();
        v["goals"].as_array().unwrap().iter().map(|g| g["ref"].as_str().unwrap().to_string()).collect()
    }
}

#[test]
fn planned_tasks_go_in_the_project_they_name() {
    let b = new_board();
    let g = b.goal();
    let r = b
        .report(json!({"event": "tb.propose", "session": "s1", "goal": format!("G{g}"),
                       "tasks": ["Form::the page", "Endpoint::the route::project=API", "Tokens::sign them::model=small::project=api",
                                 "Logs::log it::project=api::model=small", {"title": "Docs", "project": "api"}]}))
        .unwrap();
    assert_eq!(b.projects_of(&r), ["webapp", "api", "api", "api", "api"]);
    let logs = board::get_task(&b.app, r["created"][3].as_str().unwrap()[1..].parse().unwrap()).unwrap();
    assert_eq!((logs.st("detail"), logs.s("model")), ("log it".to_string(), Some("small")));

    let gd = board::goal_dict(&b.app, &board::get_goal(&b.app, g).unwrap()).unwrap();
    assert_eq!(gd["project"], "webapp");
    assert_eq!(gd["projects"], json!(["webapp", "api"]));
    assert_eq!(b.goal_refs("api"), [format!("G{g}")]);
    assert_eq!(b.goal_refs("webapp"), [format!("G{g}")]);
    assert_eq!(b.goal_refs("all"), [format!("G{g}")]);
}

#[test]
fn a_task_new_in_a_goal_keeps_its_project() {
    let b = new_board();
    let g = b.goal();
    let r = b.report(json!({"event": "tb.new_task", "session": "s1", "title": "Endpoint", "goal": format!("G{g}"), "project": "api"})).unwrap();
    assert_eq!(b.projects_of(&r), ["api"]);
    let e = b.report(json!({"event": "tb.new_task", "session": "s1", "title": "Endpoint", "goal": format!("G{g}"), "project": "nope"})).unwrap_err();
    assert!(e.contains("no project “nope”"), "{e}");
}
