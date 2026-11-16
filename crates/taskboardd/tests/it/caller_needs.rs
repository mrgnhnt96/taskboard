//! A terminal on a task makes or starts tasks whose detail names it ("Blocks T12"): `tb task new` and `tb start`
//! hand back the exact `tb wait-for` line, so its agent records the wait instead of only saying it will wait.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::midna;
use taskboardd::reports;

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
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
    fn report(&self, event: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": ""});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap()
    }
    fn task(&self, project: &str, title: &str, detail: &str) -> i64 {
        self.post("/tasks", json!({"title": title, "detail": detail, "project": project, "pickup": "manual"})).unwrap()["id"].as_i64().unwrap()
    }
    /// Terminal s1 in `project`, on task `id`.
    fn on(&self, project: &str, id: i64) {
        let repo = self.dir.path().join(project);
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")}));
    }
    /// What `tb task new` sends from s1.
    fn new_task(&self, extra: Value) -> Value {
        self.report("tb.new_task", extra)
    }
    /// What `tb start T<id>` sends from s1, after the owner asked for it there.
    fn start(&self, id: i64) -> Value {
        self.report("hook.prompt", json!({"prompt": format!("start T{id}")}));
        self.post(&format!("/tasks/T{id}/start"), json!({"mode": "queue", "via_session": "s1"})).unwrap()
    }
}

fn made(v: &Value) -> String {
    v["created"][0].as_str().unwrap().to_string()
}

#[test]
fn a_new_task_that_names_the_terminals_task_prints_the_wait_for_line() {
    let b = new_board();
    let mine = b.task("webapp", "Ship the form", "Do it.");
    b.on("webapp", mine);

    let v = b.new_task(json!({"title": "Fix the validator", "detail": format!("Blocks T{mine}: report the SHA on T{mine}.")}));
    let line = v["wait_line"].as_str().unwrap_or_else(|| panic!("{v}"));
    assert!(line.contains(&format!("tb wait-for {} --why", made(&v))), "{line}");
    assert!(line.contains("Only saying you'll wait records nothing"), "{line}");

    let other = b.new_task(json!({"title": "Tidy the readme", "detail": format!("Found while on T{mine}0.")}));
    assert!(other["wait_line"].is_null(), "{other}");
}

#[test]
fn starting_tasks_that_name_it_prints_one_line_for_them_all() {
    let b = new_board();
    let mine = b.task("webapp", "Ship the form", "Do it.");
    let one = b.task("webapp", "Fix the validator", &format!("Blocks T{mine}."));
    let two = b.task("webapp", "Bump the client", &format!("T{mine} needs the new client."));
    let unrelated = b.task("webapp", "Tidy the readme", "Nothing to do with it.");
    b.on("webapp", mine);

    let v = b.start(one);
    assert!(v["wait_line"].as_str().unwrap_or_default().contains(&format!("tb wait-for T{one} --why")), "{v}");
    assert!(b.start(unrelated)["wait_line"].is_null());

    b.report("hook.prompt", json!({"prompt": format!("start T{two} and T{unrelated}")}));
    let v = b.post("/tasks/start", json!({"tasks": [format!("T{two}")], "mode": "queue", "via_session": "s1"})).unwrap();
    assert!(v["wait_line"].as_str().unwrap_or_default().contains(&format!("tb wait-for T{two} --why")), "{v}");
}

#[test]
fn no_line_once_it_waits_or_when_the_new_task_waits_for_it() {
    let b = new_board();
    let mine = b.task("webapp", "Ship the form", "Do it.");
    b.on("webapp", mine);
    let v = b.new_task(json!({"title": "Follow up", "detail": format!("After T{mine} lands."), "waits_for": [format!("T{mine}")]}));
    assert!(v["wait_line"].is_null(), "{v}");
}

#[test]
fn another_projects_task_gets_the_refusal_until_the_project_may_wait_on_it() {
    let b = new_board();
    let mine = b.task("webapp", "Use the endpoint", "Do it.");
    b.on("webapp", mine);

    let v = b.new_task(json!({"title": "Add the endpoint", "detail": format!("Blocks T{mine}."), "project": "api"}));
    let line = v["wait_line"].as_str().unwrap_or_else(|| panic!("{v}"));
    assert!(line.contains("tb project set webapp --waits-on api"), "{line}");
    assert!(!line.contains("tb wait-for"), "{line}");

    b.post("/projects/webapp", json!({"waits_on": ["api"]})).unwrap();
    let v = b.new_task(json!({"title": "Add the other endpoint", "detail": format!("Blocks T{mine}."), "project": "api"}));
    assert!(v["wait_line"].as_str().unwrap_or_default().contains(&format!("tb wait-for {} --why", made(&v))), "{v}");
}
