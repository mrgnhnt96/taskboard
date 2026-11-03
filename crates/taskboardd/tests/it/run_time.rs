//! The Working time on a task's card counts from its current pickup (`picked_up_at`), not its first
//! (`started_at`), so a night spent waiting between runs doesn't show as work (#877).

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, midna, p, reports};

const OLD: &str = "2026-01-01T00:00:00Z";

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
    fn report(&self, event: &str, session: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {"branch": "feature/x"}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message))
    }
    fn session(&self, sid: &str) {
        let repo = self.dir.path().join("webapp");
        midna::sync(
            &self.app,
            &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})],
            &[],
        )
        .unwrap();
    }
    fn task(&self, title: &str) -> i64 {
        let body = json!({"title": title, "detail": "Do it.", "project": "webapp", "pickup": "new"});
        api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &body).unwrap()["id"].as_i64().unwrap()
    }
    fn take(&self, sid: &str, id: i64) {
        self.session(sid);
        self.report("tb.take", sid, json!({"task": format!("T{id}")}));
    }
    fn card(&self, id: i64) -> Value {
        api::dispatch(&self.app, "GET", &format!("tasks/T{id}"), &Query::new(), &json!({})).unwrap()
    }
    /// The task has been running since long ago.
    fn backdate(&self, id: i64) {
        self.app.db.x("UPDATE tasks SET started_at = ?, picked_up_at = ? WHERE id = ?", p![OLD, OLD, id]).unwrap();
    }
}

fn fresh(at: &Value) -> bool {
    at.as_str().is_some_and(|a| a > "2026-01-02")
}

#[test]
fn a_pickup_after_a_wait_counts_from_the_new_pickup() {
    let b = new_board();
    let (dep, id) = (b.task("Endpoint"), b.task("Client"));
    b.take("s1", id);
    let first = b.card(id);
    assert_eq!(first["status"], "working");
    assert_eq!(first["picked_up_at"], first["started_at"], "the first pickup starts both");

    b.report("tb.wait_for", "s1", json!({"tasks": [format!("T{dep}")], "why": "Needs the endpoint"}));
    assert_eq!(b.card(id)["status"], "queued");
    b.backdate(id);

    b.take("s2", id);
    let card = b.card(id);
    assert_eq!(card["status"], "working");
    assert_eq!(card["started_at"], OLD, "started_at stays the first pickup");
    assert!(fresh(&card["picked_up_at"]), "the run counts from this pickup: {card}");
}

#[test]
fn an_answered_question_carries_the_run_on() {
    let b = new_board();
    let id = b.task("Client");
    b.take("s1", id);
    b.backdate(id);
    board::update_task(&b.app, id, vec![("status", json!("needs")), ("needs_reason", json!("question")), ("question", json!("10 or 30?"))]).unwrap();
    board::set_working(&b.app, &board::get_task(&b.app, id).unwrap(), "Term s1", Some("Answered")).unwrap();
    let card = b.card(id);
    assert_eq!(card["status"], "working");
    assert_eq!(card["picked_up_at"], OLD, "waiting on an answer is part of the run");
}

#[test]
fn a_pickup_in_a_new_terminal_after_a_lost_one_starts_a_new_run() {
    let b = new_board();
    let id = b.task("Client");
    b.take("s1", id);
    b.backdate(id);
    board::update_task(&b.app, id, vec![("status", json!("needs")), ("lost", json!(1))]).unwrap();
    b.take("s2", id);
    let card = b.card(id);
    assert_eq!(card["status"], "working");
    assert_eq!(card["started_at"], OLD);
    assert!(fresh(&card["picked_up_at"]), "{card}");
}

#[test]
fn a_task_from_before_pickups_were_kept_counts_from_its_start() {
    let b = new_board();
    let id = b.task("Client");
    b.take("s1", id);
    b.app.db.x("UPDATE tasks SET started_at = ?, picked_up_at = NULL WHERE id = ?", p![OLD, id]).unwrap();
    assert_eq!(b.card(id)["picked_up_at"], OLD);
}
