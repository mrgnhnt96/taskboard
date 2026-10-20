//! A goal's `last_update`: when its started work last moved, so the sidebar can show a goal that
//! has gone quiet. Edits to planned and queued tasks don't count; a running task's terminal does.

use std::sync::Arc;

use serde_json::Value;
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::parse_iso;
use taskboardd::{board, fields};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    Board { app, _dir: dir }
}

impl Board {
    fn goal(&self) -> i64 {
        self.app.db.insert("goals", fields!["name" => "Tablet mode", "project" => "webapp"]).unwrap()
    }
    fn task(&self, goal: i64, status: &str, updated_at: &str, session: Option<&str>) {
        self.app
            .db
            .insert("tasks", fields!["title" => "x", "project" => "webapp", "goal_id" => goal, "status" => status, "updated_at" => updated_at, "session_id" => session])
            .unwrap();
    }
    fn session(&self, id: &str, last_activity: &str) {
        self.app.db.insert("sessions", fields!["id" => id, "status" => "working", "last_activity" => last_activity]).unwrap();
    }
    fn last_update(&self, goal: i64) -> Value {
        board::goal_counts(&self.app, goal).unwrap()["last_update"].clone()
    }
}

fn at(v: &Value) -> Option<f64> {
    v.as_str().and_then(parse_iso)
}

#[test]
fn edits_to_work_not_started_yet_dont_count() {
    let b = new_board();
    let g = b.goal();
    b.task(g, "done", "2026-10-20T10:00:00Z", None);
    b.task(g, "queued", "2026-10-20T12:00:00Z", None);
    b.task(g, "planned", "2026-10-20T13:00:00Z", None);
    assert_eq!(at(&b.last_update(g)), parse_iso("2026-10-20T10:00:00Z"));
}

#[test]
fn a_running_task_counts_its_terminal() {
    let b = new_board();
    let g = b.goal();
    b.task(g, "done", "2026-10-20T10:00:00Z", None);
    b.session("s1", "2026-10-20T11:30:00Z");
    b.task(g, "working", "2026-10-20T11:00:00Z", Some("s1"));
    assert_eq!(at(&b.last_update(g)), parse_iso("2026-10-20T11:30:00Z"));
}

#[test]
fn a_goal_with_nothing_started_has_no_update() {
    let b = new_board();
    let g = b.goal();
    b.task(g, "queued", "2026-10-20T12:00:00Z", None);
    assert_eq!(b.last_update(g), Value::Null);
}

#[test]
fn the_goal_list_carries_it() {
    let b = new_board();
    let g = b.goal();
    b.task(g, "done", "2026-10-20T10:00:00Z", None);
    let row = board::get_goal(&b.app, g).unwrap();
    let d = board::goal_dict(&b.app, &row).unwrap();
    assert_eq!(at(&d["last_update"]), parse_iso("2026-10-20T10:00:00Z"));
}
