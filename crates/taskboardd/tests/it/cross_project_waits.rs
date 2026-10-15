//! `tb project set <name> --waits-on <other>`: a project's tasks may wait for another project's, one way only.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, midna, p, reports, runner};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let app = App::for_tests(cfg);
    let mut projects = vec![];
    for name in ["webapp", "api"] {
        let repo = dir.path().join(name);
        std::fs::create_dir_all(&repo).unwrap();
        projects.push(json!({"name": name, "path": repo.to_string_lossy()}));
    }
    app.db.set_setting("midna_projects", Some(&Value::Array(projects).to_string())).unwrap();
    Board { app, dir }
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
    fn report(&self, event: &str, session: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {"branch": "feature/x"}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn task(&self, project: &str, title: &str) -> i64 {
        self.post("/tasks", json!({"title": title, "detail": "Do it.", "project": project, "pickup": "new"})).unwrap()["id"].as_i64().unwrap()
    }
    fn on(&self, sid: &str, project: &str, id: i64) {
        let repo = self.dir.path().join(project);
        midna::sync(
            &self.app,
            &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})],
            &[],
        )
        .unwrap();
        self.report("tb.take", sid, json!({"task": format!("T{id}")})).unwrap();
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn starts(&self, id: i64) -> Vec<Row> {
        self.app.db.q("SELECT * FROM jobs WHERE kind = 'agent' AND purpose = 'start' AND task_id = ? ORDER BY id", p![id]).unwrap()
    }
}

#[test]
fn a_task_cant_wait_for_another_projects_task_until_its_project_allows_it() {
    let b = new_board();
    let api_task = b.task("api", "Add the endpoint");
    let web_task = b.task("webapp", "Use the endpoint");
    b.on("s1", "webapp", web_task);

    let (status, msg) = b.report("tb.wait_for", "s1", json!({"tasks": [format!("T{api_task}")]})).unwrap_err();
    assert_eq!(status, 409);
    assert!(msg.contains("tb project set webapp --waits-on api"), "{msg}");

    let p = b.post("/projects/webapp", json!({"waits_on": ["api"]})).unwrap();
    assert_eq!(p["waits_on"], json!(["api"]));
    let out = b.report("tb.wait_for", "s1", json!({"tasks": [format!("T{api_task}")], "merged": true})).unwrap();
    assert_eq!(out["parked"], true, "{out}");
}

#[test]
fn it_only_goes_one_way() {
    let b = new_board();
    b.post("/projects/webapp", json!({"waits_on": "api"})).unwrap();
    let web_task = b.task("webapp", "Use the endpoint");
    let api_task = b.task("api", "Add the endpoint");
    b.on("s1", "api", api_task);
    let (status, msg) = b.report("tb.wait_for", "s1", json!({"tasks": [format!("T{web_task}")]})).unwrap_err();
    assert_eq!(status, 409);
    assert!(msg.contains("tb project set api --waits-on webapp"), "{msg}");
}

#[test]
fn once_the_other_projects_pr_merges_it_resumes_without_rebase_steps() {
    let b = new_board();
    b.post("/projects/webapp", json!({"waits_on": ["api"]})).unwrap();
    let api_task = b.task("api", "Add the endpoint");
    let web_task = b.task("webapp", "Use the endpoint");
    b.on("s1", "api", api_task);
    b.report("tb.done", "s1", json!({"summary": "Added GET /things.", "pr": "https://github.com/acme/api/pull/9"})).unwrap();
    b.on("s2", "webapp", web_task);

    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{api_task}")], "merged": true})).unwrap();
    assert_eq!(out["parked"], true, "{out}");
    assert!(runner::start_queued(&b.app).unwrap().is_empty(), "it waits while the api PR is open");

    board::update_task(&b.app, api_task, vec![("pr_state", json!("MERGED")), ("pr_phase", json!("merged"))]).unwrap();
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![web_task]);
    let prompt = board::job_args(&b.starts(web_task)[0]).st("prompt");
    assert!(prompt.contains("in api is merged (PR #9"), "{prompt}");
    assert!(prompt.contains("Added GET /things."), "{prompt}");
    assert!(prompt.contains("nothing to rebase"), "{prompt}");
    assert!(!prompt.contains("Bring it in with git"), "{prompt}");
    assert_eq!(b.row(web_task).s("project"), Some("webapp"));
}

#[test]
fn waits_on_names_known_projects_and_none_clears_it() {
    let b = new_board();
    let (status, _) = b.post("/projects/webapp", json!({"waits_on": ["nope"]})).unwrap_err();
    assert_eq!(status, 404);
    b.post("/projects/webapp", json!({"waits_on": ["api"]})).unwrap();
    let p = b.post("/projects/webapp", json!({"waits_on": "none"})).unwrap();
    assert_eq!(p["waits_on"], json!([]));
}
