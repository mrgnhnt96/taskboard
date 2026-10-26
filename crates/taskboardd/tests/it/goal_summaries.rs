//! Goal summaries are kept between polls: asking again with nothing written gives the same answer,
//! and a changed goal row gives a fresh one. A finished goal's summary holds through writes that can't
//! change it; any other goal's is made again after any write.

use std::sync::Arc;

use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, fields};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    // On disk, so `add_unseen` can write it from another connection.
    let app = App::new(Config::for_tests(dir.path()), false).unwrap();
    Board { app, _dir: dir }
}

impl Board {
    fn goal(&self) -> i64 {
        self.app.db.insert("goals", fields!["name" => "Tablet mode", "project" => "webapp"]).unwrap()
    }
    fn task(&self, goal: i64, status: &str) -> i64 {
        self.app.db.insert("tasks", fields!["title" => "x", "project" => "webapp", "goal_id" => goal, "status" => status]).unwrap()
    }
    fn total(&self, goal: i64) -> serde_json::Value {
        let row = board::get_goal(&self.app, goal).unwrap();
        board::goal_dict(&self.app, &row).unwrap()["total"].clone()
    }
    /// Adds a done task to the goal from another connection, which the board can't tell apart from any
    /// other outside write: a kept summary still counts the tasks before it.
    fn add_unseen(&self, goal: i64) {
        let c = rusqlite::Connection::open(self.app.cfg.db_path()).unwrap();
        c.execute("INSERT INTO tasks(title, project, goal_id, status) VALUES('y', 'webapp', ?, 'done')", [goal]).unwrap();
    }
}

#[test]
fn a_write_refreshes_the_summary() {
    let b = new_board();
    let g = b.goal();
    b.task(g, "done");
    let row = board::get_goal(&b.app, g).unwrap();
    assert_eq!(board::goal_dict(&b.app, &row).unwrap()["total"], 1);
    assert_eq!(board::goal_dict(&b.app, &row).unwrap()["total"], 1);
    b.task(g, "queued");
    let d = board::goal_dict(&b.app, &row).unwrap();
    assert_eq!(d["total"], 2);
    assert_eq!(d["queued"], 1);
}

#[test]
fn a_changed_goal_row_isnt_answered_from_the_old_one() {
    let b = new_board();
    let g = b.goal();
    let mut row = board::get_goal(&b.app, g).unwrap();
    assert_eq!(board::goal_dict(&b.app, &row).unwrap()["name"], "Tablet mode");
    row.insert("name".into(), "Phone mode".into());
    assert_eq!(board::goal_dict(&b.app, &row).unwrap()["name"], "Phone mode");
}

#[test]
fn a_finished_goal_is_kept_through_another_goals_writes() {
    let b = new_board();
    let (done, busy) = (b.goal(), b.goal());
    b.task(done, "done");
    let other = b.task(busy, "queued");
    assert_eq!(b.total(done), 1);
    b.add_unseen(done);
    b.app.db.update("tasks", &other.into(), fields!["status" => "working"]).unwrap();
    b.app.db.insert("events", fields!["kind" => "x"]).unwrap();
    assert_eq!(b.total(done), 1);
}

#[test]
fn a_finished_goal_is_made_again_when_its_own_task_changes() {
    let b = new_board();
    let g = b.goal();
    let t = b.task(g, "done");
    assert_eq!(b.total(g), 1);
    b.add_unseen(g);
    b.app.db.update("tasks", &t.into(), fields!["failed" => true]).unwrap();
    assert_eq!(b.total(g), 2);
}

#[test]
fn a_finished_goal_is_made_again_when_a_task_moves_into_it() {
    let b = new_board();
    let (g, other) = (b.goal(), b.goal());
    b.task(g, "done");
    let t = b.task(other, "done");
    assert_eq!(b.total(g), 1);
    b.app.db.update("tasks", &t.into(), fields!["goal_id" => g]).unwrap();
    assert_eq!(b.total(g), 2);
}

#[test]
fn a_finished_goal_is_made_again_after_a_write_it_reads() {
    let b = new_board();
    let g = b.goal();
    b.task(g, "done");
    b.app.db.set_setting("pr_flow", Some("pr")).unwrap();
    assert_eq!(b.total(g), 1);
    b.add_unseen(g);
    b.app.db.set_setting("pr_flow", Some("pr")).unwrap();
    assert_eq!(b.total(g), 1, "a setting rewritten as it was changes nothing");
    b.app.db.set_setting("pr_flow", Some("direct")).unwrap();
    assert_eq!(b.total(g), 2);
}

#[test]
fn an_outside_write_alone_makes_it_again() {
    let b = new_board();
    let g = b.goal();
    b.task(g, "done");
    assert_eq!(b.total(g), 1);
    b.add_unseen(g);
    assert_eq!(b.total(g), 2);
}

#[test]
fn an_unfinished_goal_is_made_again_after_any_write() {
    let b = new_board();
    let (g, other) = (b.goal(), b.goal());
    b.task(g, "queued");
    let t = b.task(other, "queued");
    assert_eq!(b.total(g), 1);
    b.add_unseen(g);
    b.app.db.update("tasks", &t.into(), fields!["status" => "working"]).unwrap();
    assert_eq!(b.total(g), 2);
}
