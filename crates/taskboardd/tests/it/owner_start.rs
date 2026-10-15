//! The owner's Start from the app runs a task now anyway, whatever it waits for or stacks on (#120).
//! Only an agent's `tb start` is held back by unfinished work (`startword.rs`).

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, p};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    Board { app, _dir: dir }
}

fn from_app() -> Query {
    [(api::FROM.to_string(), "app".to_string())].into_iter().collect()
}

impl Board {
    fn task(&self, title: &str, extra: Value) -> i64 {
        let mut body = json!({"title": title, "detail": "Do it.", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &body).unwrap()["id"].as_i64().unwrap()
    }
    /// The board's Start button (and drag to Working).
    fn owner_start(&self, id: i64) -> Value {
        api::dispatch(&self.app, "POST", &format!("/tasks/T{id}/start"), &from_app(), &json!({"mode": "new"}))
            .unwrap_or_else(|e| panic!("Start T{id}: {} {}", e.status, e.message))
    }
    fn started(&self, id: i64) {
        let card = self.owner_start(id);
        assert_eq!(card["starting"], true, "{card}");
        let log: Vec<String> =
            self.app.db.q("SELECT text FROM events WHERE task_id = ? ORDER BY id", p![id]).unwrap().iter().map(|r| r.st("text")).collect();
        assert!(log.iter().any(|l| l == "Started in the UI"), "{log:?}");
    }
}

#[test]
fn the_owners_start_runs_a_task_that_waits_for_unfinished_work() {
    let b = board();
    let first = b.task("Schema", json!({}));
    let id = b.task("Add logout", json!({"waits_for": format!("T{first}")}));
    b.started(id);
}

#[test]
fn the_owners_start_runs_a_task_whose_wait_failed() {
    let b = board();
    let first = b.task("Schema", json!({}));
    let id = b.task("Add logout", json!({"waits_for": format!("T{first}")}));
    board::update_task(&b.app, first, vec![("status", json!("done")), ("failed", json!(1))]).unwrap();
    b.started(id);
}

#[test]
fn the_owners_start_runs_a_stacked_child_ahead_of_its_parent() {
    let b = board();
    let parent = b.task("Add the endpoint", json!({}));
    let child = b.task("Use the endpoint", json!({"stack_on": format!("T{parent}")}));
    b.started(child);
}
