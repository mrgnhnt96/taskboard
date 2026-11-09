//! A start's terminal between Midna opening it and its agent checking in (`startwatch.rs`): one that
//! exits or closes first puts the task back in the queue, and one waiting on Claude's folder-trust
//! prompt says so on the task.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{iso, now_iso, now_ts, RowExt};
use taskboardd::{board, midna, p, startwatch};

struct Board {
    app: Arc<App>,
    repo: String,
    _dir: tempfile::TempDir,
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    Board { app, repo: repo.to_string_lossy().into_owned(), _dir: dir }
}

fn from_app() -> Query {
    [(api::FROM.to_string(), "app".to_string())].into_iter().collect()
}

impl Board {
    fn task(&self) -> i64 {
        let body = json!({"title": "Add logout", "detail": "Do it.", "project": "webapp"});
        api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &body).unwrap()["id"].as_i64().unwrap()
    }
    fn start(&self, id: i64) -> Value {
        api::dispatch(&self.app, "POST", &format!("/tasks/T{id}/start"), &from_app(), &json!({"mode": "new"}))
            .unwrap_or_else(|e| panic!("Start T{id}: {} {}", e.status, e.message))
    }
    /// Starts the task and has Midna open terminal `sid` for it (the start job done, as `run_agent` leaves it).
    fn opened(&self, sid: &str) -> i64 {
        let id = self.task();
        self.start(id);
        let job = board::get_task(&self.app, id).unwrap().i("start_job").expect("a start job");
        self.app
            .db
            .x("UPDATE jobs SET state = 'done', target = json_set(COALESCE(target, '{}'), '$.session', ?) WHERE id = ?", p![sid, job])
            .unwrap();
        id
    }
    fn listed(&self, sid: &str, status: Value) -> Value {
        json!({"id": sid, "name": "Add logout", "agent": "claude", "cwd": self.repo, "status": status})
    }
    fn sync(&self, sessions: &[Value]) {
        midna::sync(&self.app, sessions, &[]).unwrap();
    }
    fn event(&self, kind: &str, sid: &str, by: &str, data: Value) {
        let e = json!({"seq": 1, "at": now_iso(), "kind": kind, "actor": {"kind": by}, "session_id": sid, "data": data});
        midna::closed_events(&self.app, &[e]).unwrap();
    }
    fn card(&self, id: i64) -> Value {
        board::task_card(&self.app, &board::get_task(&self.app, id).unwrap()).unwrap()
    }
    fn log(&self, id: i64) -> Vec<String> {
        self.app.db.q("SELECT text FROM events WHERE task_id = ? ORDER BY id", p![id]).unwrap().iter().map(|r| r.st("text")).collect()
    }
    fn closes(&self, sid: &str, purpose: &str) -> i64 {
        self.app
            .db
            .count("SELECT COUNT(*) FROM jobs WHERE kind = 'close' AND purpose = ? AND json_extract(args, '$.session') = ?", p![purpose, sid])
            .unwrap()
    }
}

/// Back in the queue as a failed start: no start job, one try counted and a retry set.
fn requeued(b: &Board, id: i64) -> taskboardd::util::Row {
    let t = board::get_task(&b.app, id).unwrap();
    assert_eq!(t.s("status"), Some("queued"));
    assert_eq!(t.i("start_job"), None, "the start is over, so Start shows again");
    assert_eq!(t.i0("start_tries"), 1);
    assert!(t.s("retry_at").is_some(), "it starts again by itself");
    assert_eq!(b.card(id)["start_terminal"], Value::Null);
    t
}

#[test]
fn a_start_terminal_closed_before_its_agent_checks_in_goes_back_in_the_queue() {
    let b = board();
    let id = b.opened("s9");
    b.sync(&[b.listed("s9", json!({"state": "working"}))]);
    assert_eq!(b.card(id)["start_terminal"]["session_id"], "s9");
    assert_eq!(b.card(id)["starting"], true);

    b.event("session.closed", "s9", "human", json!({"force": true}));
    let t = requeued(&b, id);
    assert_eq!(t.st("latest"), "Add logout closed before its agent checked in (you closed it)");
    assert!(b.log(id).iter().any(|l| l.starts_with("Add logout closed before its agent checked in")), "{:?}", b.log(id));
}

#[test]
fn a_start_terminal_that_exits_before_its_agent_checks_in_is_closed_and_requeued() {
    let b = board();
    let id = b.opened("s9");
    b.sync(&[b.listed("s9", json!({"state": "working"}))]);
    b.event("session.exited", "s9", "system", json!({"exit_code": 1}));
    let t = requeued(&b, id);
    assert!(t.st("latest").starts_with("Add logout closed before its agent checked in"), "{}", t.st("latest"));
    assert_eq!(b.closes("s9", "start_ended"), 1, "what's left of it is closed so it can't pick the task up later");
}

#[test]
fn a_close_after_the_agent_checked_in_is_not_a_failed_start() {
    let b = board();
    let id = b.opened("s9");
    board::update_task(&b.app, id, vec![("status", json!("working")), ("session_id", json!("s9")), ("start_job", Value::Null)]).unwrap();
    b.event("session.closed", "s9", "human", json!({"force": true}));
    let t = board::get_task(&b.app, id).unwrap();
    assert_eq!(t.i0("start_tries"), 0);
    assert!(!t.st("latest").contains("before its agent checked in"), "{}", t.st("latest"));
}

#[test]
fn a_start_terminal_gone_while_the_board_was_down_is_requeued_on_the_next_sync() {
    let b = board();
    let id = b.opened("s9");
    // Midna opened it a while ago and doesn't list it now; the board never saw it.
    let job = board::get_task(&b.app, id).unwrap().i("start_job").unwrap();
    b.app.db.x("UPDATE jobs SET updated_at = ? WHERE id = ?", p![iso(now_ts() - 600.0), job]).unwrap();
    b.sync(&[]);
    requeued(&b, id);
}

#[test]
fn a_start_terminal_midna_has_yet_to_list_gets_a_moment() {
    let b = board();
    let id = b.opened("s9");
    b.sync(&[]);
    let t = board::get_task(&b.app, id).unwrap();
    assert!(t.i("start_job").is_some(), "just opened: Midna may not list it yet");
    assert_eq!(t.i0("start_tries"), 0);
}

#[test]
fn a_start_terminal_listed_as_exited_is_requeued() {
    let b = board();
    let id = b.opened("s9");
    b.sync(&[b.listed("s9", json!({"state": "exited", "exit_code": 1}))]);
    requeued(&b, id);
}

#[test]
fn the_folder_trust_prompt_shows_on_the_task_until_it_is_answered() {
    let b = board();
    let id = b.opened("s9");
    b.sync(&[b.listed("s9", json!({"state": "needs_you", "reason": startwatch::FOLDER_TRUST}))]);
    let line = startwatch::trust_line("Add logout");
    assert_eq!(board::get_task(&b.app, id).unwrap().st("latest"), line);
    let card = b.card(id);
    assert_eq!(card["start_terminal"]["folder_trust"], true, "{card}");
    assert_eq!(card["waiting"], line.as_str(), "{card}");
    assert_eq!(b.log(id).iter().filter(|l| l.starts_with(&line)).count(), 1);

    b.sync(&[b.listed("s9", json!({"state": "needs_you", "reason": startwatch::FOLDER_TRUST}))]);
    assert_eq!(b.log(id).iter().filter(|l| l.starts_with(&line)).count(), 1, "logged once, not on every sync");

    b.sync(&[b.listed("s9", json!({"state": "working"}))]);
    let card = b.card(id);
    assert_eq!(board::get_task(&b.app, id).unwrap().st("latest"), "Starting in Midna…");
    assert_eq!(card["start_terminal"]["folder_trust"], false, "{card}");
    assert_eq!(card["waiting"], "Opened Add logout; waiting for its agent to check in", "{card}");
}

#[test]
fn closing_a_terminal_still_on_the_folder_trust_prompt_says_so() {
    let b = board();
    let id = b.opened("s9");
    b.sync(&[b.listed("s9", json!({"state": "needs_you", "reason": startwatch::FOLDER_TRUST}))]);
    b.event("session.closed", "s9", "human", json!({"force": true}));
    let t = requeued(&b, id);
    assert_eq!(t.st("latest"), "Add logout closed before its agent checked in, still on the folder-trust prompt (you closed it)");
}

#[test]
fn start_over_closes_the_terminal_its_agent_never_checked_in_from() {
    let b = board();
    let id = b.opened("s9");
    b.sync(&[b.listed("s9", json!({"state": "needs_you", "reason": startwatch::FOLDER_TRUST}))]);
    let card = b.start(id);
    assert_eq!(card["starting"], true, "{card}");
    assert_eq!(b.closes("s9", "start_over"), 1);
    let starts = b.app.db.count("SELECT COUNT(*) FROM jobs WHERE task_id = ? AND kind = 'agent' AND purpose = 'start'", p![id]).unwrap();
    assert_eq!(starts, 2, "a new start");
    assert_eq!(card["start_terminal"], Value::Null, "the new start hasn't opened a terminal yet: {card}");
}
