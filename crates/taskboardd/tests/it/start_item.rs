//! A task made from a backlog item starts on the owner's word naming the item ("start B3", "/start B3"),
//! as `tb start B<n>` asks for it; a word for another item is no word for it.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::startword::{item_as_task, ItemTask};
use taskboardd::{midna, reports};

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

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap()
    }
    fn said(&self, prompt: &str) {
        reports::handle(&self.app, json!({"event": "hook.prompt", "session": "s1", "claude_session": "c-s1", "cwd": "", "prompt": prompt}), false).unwrap();
    }
    /// A backlog item made into its task the way `tb start B<n>` makes it.
    fn item_task(&self, title: &str) -> (i64, i64) {
        let item = self.post("/backlog", json!({"title": title, "project": "webapp", "kind": "bug"}))["id"].as_i64().unwrap();
        let v = self.post("/backlog/bulk", json!({"action": "task", "ids": [format!("B{item}")], "where": "goal", "pickup": {"mode": "manual"}}));
        (item, v["tasks"][0]["id"].as_i64().unwrap())
    }
    fn tb_start(&self, id: i64) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", &format!("/tasks/T{id}/start"), &Query::new(), &json!({"mode": "new", "via_session": "s1"}))
            .map_err(|e| (e.status, e.message))
    }
}

#[test]
fn an_item_ref_reads_as_its_task_and_others_stay() {
    let i = ItemTask { item: 3, task: 40 };
    assert_eq!(item_as_task("start B3", i), "start T40");
    assert_eq!(item_as_task("/start b3", i), "/start T40");
    assert_eq!(item_as_task("start B31 and B4", i), "start B31 and B4");
    assert_eq!(item_as_task("JIRAB3 is B3x", i), "JIRAB3 is B3x");
    for dictated in ["start B-3", "start B 3", "start B #3", "start b–3"] {
        assert_eq!(item_as_task(dictated, i), "start T40", "{dictated}");
    }
    assert_eq!(item_as_task("start B-31", i), "start B-31");
}

#[test]
fn naming_the_item_starts_its_task() {
    let b = board();
    let (item, task) = b.item_task("Header clips on narrow windows");
    b.said(&format!("start B{item}"));
    assert!(b.tb_start(task).is_ok(), "the owner named the item the task was made from");
}

#[test]
fn the_start_command_with_the_item_starts_its_task() {
    let b = board();
    let (item, task) = b.item_task("Header clips on narrow windows");
    b.said(&format!("/start B{item}"));
    assert!(b.tb_start(task).is_ok());
}

#[test]
fn a_word_for_another_item_or_a_held_start_is_no_word() {
    let b = board();
    let (_, task) = b.item_task("Header clips on narrow windows");
    let (other, _) = b.item_task("Sidebar flickers on resize");
    b.said(&format!("start B{other}"));
    assert_eq!(b.tb_start(task).unwrap_err().0, 403);
    let (item, task) = b.item_task("Footer overlaps the toast");
    b.said(&format!("don't start B{item} yet"));
    assert_eq!(b.tb_start(task).unwrap_err().0, 403);
}
