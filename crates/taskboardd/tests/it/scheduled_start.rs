//! A start at a set time (`tb start T8 --at 5pm`, the app's Start at…) waits for it, then runs
//! whatever the work hours say.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::dispatch::start_asks;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, p, reports, runner, scheduled};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.notify = true;
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
    fn post(&self, path: &str, q: &Query, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, q, &body).map_err(|e| (e.status, e.message))
    }
    fn task(&self, title: &str, extra: Value) -> i64 {
        let mut body = json!({"title": title, "detail": "Do it.", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", &Query::new(), body).unwrap()["id"].as_i64().unwrap()
    }
    fn goal(&self) -> i64 {
        self.post("/goals", &Query::new(), json!({"name": "Night run", "project": "webapp", "outcome": "it ran"})).unwrap()["id"].as_i64().unwrap()
    }
    fn said(&self, prompt: &str) {
        let b = json!({"event": "hook.prompt", "session": "s1", "claude_session": "c-s1", "cwd": "", "prompt": prompt});
        reports::handle(&self.app, b, false).unwrap();
    }
    fn row(&self, id: i64) -> taskboardd::util::Row {
        board::get_task(&self.app, id).unwrap()
    }
    /// The schedule's minute moves into the past, as if the clock had reached it.
    fn come_due(&self, table: &str, id: i64) {
        self.app.db.x(&format!("UPDATE {table} SET start_at = '2020-01-01T00:00' WHERE id = ?"), p![id]).unwrap();
    }
    /// Work hours that are closed now (unless the test runs in Monday's first minute).
    fn closed_hours(&self) -> bool {
        let h = self.post("/hours", &from_app(), json!({"on": true, "start": "00:00", "end": "00:01", "days": "mon"})).unwrap();
        !h["open"].as_bool().unwrap()
    }
}

#[test]
fn a_scheduled_task_waits_for_its_time_then_starts_outside_work_hours() {
    let b = board();
    let id = b.task("Night work", json!({}));
    let card = b.post(&format!("/tasks/T{id}/start"), &from_app(), json!({"at": "tomorrow 5pm"})).unwrap();
    assert_eq!(card["status"], "queued");
    assert!(card["start_at"].as_str().unwrap().ends_with("T17:00"), "{card}");
    assert!(card["waiting"].as_str().unwrap().starts_with("Starts tomorrow"), "{card}");

    b.post("/hours", &from_app(), json!({"on": false})).unwrap();
    assert!(runner::start_queued(&b.app).unwrap().is_empty(), "it waits for its time even with no work hours");

    let closed = b.closed_hours();
    b.come_due("tasks", id);
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![id], "its time came: it starts (hours closed: {closed})");
    assert_eq!(b.row(id).s("start_at"), None, "the schedule is spent once it starts");
}

#[test]
fn a_scheduled_goal_task_starts_ahead_of_its_unrun_goal() {
    let b = board();
    let gid = b.goal();
    let id = b.task("First", json!({"goal_id": gid, "status": "planned"}));
    b.post(&format!("/tasks/T{id}/start"), &from_app(), json!({"at": "tomorrow 5pm"})).unwrap();
    b.come_due("tasks", id);
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![id]);
}

#[test]
fn cancelling_puts_the_task_back_as_it_was() {
    let b = board();
    let gid = b.goal();
    let id = b.task("First", json!({"goal_id": gid, "status": "planned"}));
    b.post(&format!("/tasks/T{id}/start"), &from_app(), json!({"at": "9am"})).unwrap();
    b.post(&format!("/tasks/T{id}/start"), &from_app(), json!({"at": "tomorrow 10am"})).unwrap();
    let card = b.post(&format!("/tasks/T{id}/start"), &from_app(), json!({"at": "off"})).unwrap();
    assert_eq!((card["status"].as_str(), card["start_at"].as_str()), (Some("planned"), None), "{card}");
    let (code, _) = b.post(&format!("/tasks/T{id}/start"), &from_app(), json!({"at": "off"})).unwrap_err();
    assert_eq!(code, 409);
}

#[test]
fn the_owners_start_runs_a_scheduled_task_now_and_drops_the_schedule() {
    let b = board();
    let id = b.task("Night work", json!({}));
    b.post(&format!("/tasks/T{id}/start"), &from_app(), json!({"at": "tomorrow 5pm"})).unwrap();
    let card = b.post(&format!("/tasks/T{id}/start"), &from_app(), json!({"mode": "new"})).unwrap();
    assert_eq!(card["starting"], true, "{card}");
    assert_eq!(b.row(id).s("start_at"), None);
}

#[test]
fn a_bad_time_is_refused() {
    let b = board();
    let id = b.task("Night work", json!({}));
    let (code, msg) = b.post(&format!("/tasks/T{id}/start"), &from_app(), json!({"at": "whenever"})).unwrap_err();
    assert_eq!(code, 400);
    assert!(msg.contains("5pm"), "{msg}");
    assert_eq!(b.row(id).s("start_at"), None);
}

#[test]
fn a_scheduled_goal_runs_at_its_time_as_start_now() {
    let b = board();
    let gid = b.goal();
    let id = b.task("First", json!({"goal_id": gid, "status": "planned"}));
    let g = b.post(&format!("/goals/G{gid}/run"), &from_app(), json!({"at": "tomorrow 9am"})).unwrap();
    assert!(g["start_at"].as_str().unwrap().ends_with("T09:00"), "{g}");
    scheduled::tick(&b.app).unwrap();
    assert_eq!(b.row(id).s("status"), Some("planned"), "not yet");

    let closed = b.closed_hours();
    b.come_due("goals", gid);
    scheduled::tick(&b.app).unwrap();
    assert_eq!(b.row(id).s("status"), Some("queued"));
    let g = board::get_goal(&b.app, gid).unwrap();
    assert_eq!(g.s("start_at"), None);
    if closed {
        assert!(g.s("hours_until").is_some(), "it runs outside the hours until they open");
        assert_eq!(runner::start_queued(&b.app).unwrap(), vec![id]);
    }
}

#[test]
fn a_goal_with_nothing_planned_cant_be_scheduled() {
    let b = board();
    let gid = b.goal();
    let (code, _) = b.post(&format!("/goals/G{gid}/run"), &from_app(), json!({"at": "5pm"})).unwrap_err();
    assert_eq!(code, 409);
}

#[test]
fn an_agent_schedules_only_on_the_owners_word_and_never_asks_to_start_now() {
    let b = board();
    let id = b.task("Night work", json!({}));
    let tb = |at: &str| b.post(&format!("/tasks/T{id}/start"), &Query::new(), json!({"at": at, "via_session": "s1"}));

    b.said("what's left on the board?");
    let (code, msg) = tb("5pm").unwrap_err();
    assert_eq!(code, 403, "{msg}");
    assert!(start_asks(&b.app).is_empty(), "a notification's Start would start it now");

    b.said(&format!("start T{id} tomorrow at 5pm"));
    let card = tb("tomorrow 5pm").unwrap();
    assert!(card["start_at"].as_str().unwrap().ends_with("T17:00"), "{card}");
    assert_eq!(b.row(id).s("status"), Some("queued"));

    let (code, _) = b.post(&format!("/tasks/T{id}/start"), &Query::new(), json!({"at": "5pm"})).unwrap_err();
    assert_eq!(code, 403, "no terminal and not the app: no one's word");
}
