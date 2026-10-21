//! Design tasks (`tb task new --kind design`): a task whose work is a design. It ends with its
//! design attached instead of a PR, and in a goal its build tasks wait for it.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, midna, p, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn task(&self, title: &str, extra: Value) -> i64 {
        let mut body = json!({"title": title, "detail": "Do it.", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", body).unwrap()["id"].as_i64().unwrap()
    }
    fn goal(&self) -> i64 {
        self.post("/goals", json!({"name": "Onboarding", "project": "webapp"})).unwrap()["id"].as_i64().unwrap()
    }
    fn take(&self, id: i64) {
        let repo = self.dir.path().join("webapp");
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[])
            .unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")})).unwrap();
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn kind(&self, id: i64) -> Value {
        self.get(&format!("tasks/T{id}"))["kind"].clone()
    }
    fn waits(&self, id: i64) -> Vec<i64> {
        self.row(id).s("waits_for").map(|s| serde_json::from_str(s).unwrap()).unwrap_or_default()
    }
}

#[test]
fn kind_is_set_on_create_and_with_set_and_shows_in_the_list() {
    let b = new_board();
    let d = b.task("Sketch the onboarding", json!({"kind": "design"}));
    let n = b.task("Build the onboarding", json!({}));
    assert_eq!((b.kind(d), b.kind(n)), (json!("design"), json!("agent")));
    assert_eq!(b.row(d).i("ships_pr"), Some(0), "a design ends without a PR");

    let list = b.get("tasks");
    let kinds: Vec<(i64, Value)> = list["tasks"].as_array().unwrap().iter().map(|t| (t["id"].as_i64().unwrap(), t["kind"].clone())).collect();
    assert!(kinds.contains(&(d, json!("design"))) && kinds.contains(&(n, json!("agent"))), "{kinds:?}");

    b.post(&format!("tasks/{n}"), json!({"kind": "design"})).unwrap();
    assert_eq!((b.kind(n), b.row(n).i("ships_pr")), (json!("design"), Some(0)));
    b.post(&format!("tasks/{n}"), json!({"kind": "none"})).unwrap();
    assert_eq!((b.kind(n), b.row(n).i("ships_pr")), (json!("agent"), None), "back to none, it ends as its project does");

    let (code, why) = b.post(&format!("tasks/{n}"), json!({"kind": "human"})).unwrap_err();
    assert_eq!(code, 400, "{why}");
    let (code, why) = b.post("/tasks", json!({"title": "X", "project": "webapp", "kind": "mockup"})).unwrap_err();
    assert_eq!(code, 400, "{why}");
}

#[test]
fn a_design_task_submits_with_its_design_attached_and_no_pr() {
    let b = new_board();
    let d = b.task("Sketch the onboarding", json!({"kind": "design"}));
    b.take(d);
    let (code, why) = b.report("tb.done", json!({"summary": "Three concepts"})).unwrap_err();
    assert_eq!(code, 409, "{why}");
    assert!(why.contains("tb attach <link> --kind design"), "says how to attach it: {why}");
    assert_eq!(b.row(d).st("status"), "working");

    // A doc isn't the design.
    b.report("tb.attach", json!({"url": "https://example.com/notes", "title": "Notes", "kind": "doc"})).unwrap();
    assert_eq!(b.report("tb.done", json!({"summary": "Three concepts"})).unwrap_err().0, 409);

    b.report("tb.attach", json!({"url": "https://claude.ai/design/abc", "title": "Onboarding canvas", "kind": "design"})).unwrap();
    b.report("tb.done", json!({"summary": "Three concepts"})).unwrap();
    assert_eq!(b.row(d).st("status"), "done");
    let texts: Vec<Row> = b.app.db.q("SELECT text FROM events WHERE task_id = ? ORDER BY id", p![d]).unwrap();
    assert!(texts.iter().any(|e| e.s("text") == Some("Design: Onboarding canvas https://claude.ai/design/abc")), "the design shows on the task");
}

#[test]
fn build_tasks_in_a_goal_wait_for_its_design_unless_told_otherwise() {
    let b = new_board();
    let g = b.goal();
    let d = b.task("Sketch the onboarding", json!({"kind": "design", "goal_id": g}));
    assert!(b.waits(d).is_empty(), "the design itself waits for nothing");

    let build = b.task("Build the onboarding", json!({"goal_id": g}));
    assert_eq!(b.waits(build), [d]);
    let free = b.task("Fix the footer", json!({"goal_id": g, "waits_for": "none"}));
    assert!(b.waits(free).is_empty(), "waits_for none opts out");

    // Moved into the goal, a task waits for the design too.
    let moved = b.task("Copy for the onboarding", json!({}));
    b.post(&format!("tasks/{moved}"), json!({"goal_id": g})).unwrap();
    assert_eq!(b.waits(moved), [d]);

    // Once the design is done, new build tasks don't wait for it.
    b.app.db.x("UPDATE tasks SET status = 'done' WHERE id = ?", p![d]).unwrap();
    let later = b.task("Polish the onboarding", json!({"goal_id": g}));
    assert!(b.waits(later).is_empty());
}
