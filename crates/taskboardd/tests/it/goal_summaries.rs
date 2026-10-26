//! Goal summaries are kept between polls: asking again with nothing written gives the same answer,
//! and any write or a changed goal row gives a fresh one.

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
    let app = App::for_tests(Config::for_tests(dir.path()));
    Board { app, _dir: dir }
}

impl Board {
    fn goal(&self) -> i64 {
        self.app.db.insert("goals", fields!["name" => "Tablet mode", "project" => "webapp"]).unwrap()
    }
    fn task(&self, goal: i64, status: &str) {
        self.app.db.insert("tasks", fields!["title" => "x", "project" => "webapp", "goal_id" => goal, "status" => status]).unwrap();
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
