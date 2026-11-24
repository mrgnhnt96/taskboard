//! A queued start waits only while its project has as many busy terminals as it may
//! (`[terminals] project_max`, or `tb project set --max-terminals`), and any task start waits while the
//! board runs as many task terminals as it may (`[terminals] global_max`, or `tb terminals --max`) or the Mac's load or memory
//! pressure is too high (`[terminals] hold_load_per_core`, `hold_memory`); its card says so.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, fields, jobs, midna, pressure, runner};

struct Board {
    app: Arc<App>,
    repo: String,
    _dir: tempfile::TempDir,
}

fn new_board(project_max: i64) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.terminals.project_max = project_max;
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let repo = repo.to_string_lossy().to_string();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo}]).to_string())).unwrap();
    Board { app, repo, _dir: dir }
}

impl Board {
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    /// Two terminals open in the project, `n` of them working and the rest idle.
    fn busy(&self, n: usize) {
        let sessions: Vec<Value> = (0..2)
            .map(|i| {
                let state = if i < n { "working" } else { "idle" };
                json!({"id": format!("s{i}"), "name": format!("Term {i}"), "agent": "claude", "cwd": self.repo, "status": {"state": state}})
            })
            .collect();
        midna::sync(&self.app, &sessions, &[]).unwrap();
    }
    fn queued_task(&self) -> i64 {
        self.queued_task_in("webapp")
    }
    fn queued_task_in(&self, project: &str) -> i64 {
        let goal = self.post("/goals", json!({"name": "Work", "project": project, "max_terminals": 4}))["id"].as_i64().unwrap();
        let id = self.post("/tasks", json!({"title": "Fix it", "detail": "Do it.", "project": project, "ships_pr": false, "goal_id": goal}))["id"]
            .as_i64()
            .unwrap();
        runner::start_queued(&self.app).unwrap();
        id
    }
    fn takes_start(&self) -> bool {
        jobs::take(&self.app).unwrap().is_some_and(|j| j.s("purpose") == Some("start"))
    }
}

#[test]
fn a_queued_start_opens_while_the_project_is_under_its_cap() {
    let b = new_board(3);
    b.busy(2);
    let t = b.queued_task();
    assert!(b.get(&format!("tasks/T{t}"))["waiting"].is_null());
    assert!(b.takes_start(), "two of three busy: one more may open");
}

#[test]
fn a_queued_start_waits_at_the_cap_and_says_why() {
    let b = new_board(2);
    b.busy(2);
    let t = b.queued_task();
    let card = b.get(&format!("tasks/T{t}"));
    assert_eq!(card["starting"], true);
    assert_eq!(card["waiting"], "Waits for a free terminal: 2 of 2 busy in webapp");
    assert!(!b.takes_start());

    b.busy(1);
    assert!(b.get(&format!("tasks/T{t}"))["waiting"].is_null());
    assert!(b.takes_start(), "a terminal freed up");
}

#[test]
fn a_project_sets_its_own_cap() {
    let b = new_board(5);
    b.busy(2);
    let out = b.post("projects/webapp", json!({"max_terminals": 2}));
    assert_eq!(out["max_terminals"], 2);
    assert_eq!(out["max_terminals_set"], 2);
    let t = b.queued_task();
    assert_eq!(b.get(&format!("tasks/T{t}"))["waiting"], "Waits for a free terminal: 2 of 2 busy in webapp");
    assert!(!b.takes_start());

    let out = b.post("projects/webapp", json!({"max_terminals": "default"}));
    assert_eq!(out["max_terminals"], 5);
    assert!(out["max_terminals_set"].is_null());
    assert!(b.takes_start());
}

#[test]
fn a_cap_is_a_count_from_one_to_a_hundred() {
    let b = new_board(5);
    let e = api::dispatch(&b.app, "POST", "projects/webapp", &Query::new(), &json!({"max_terminals": 0})).unwrap_err();
    assert_eq!(e.status, 400);
    assert_eq!(b.post("projects/webapp", json!({"max_terminals": 100}))["max_terminals"], 100);
    let e = api::dispatch(&b.app, "POST", "projects/webapp", &Query::new(), &json!({"max_terminals": 101})).unwrap_err();
    assert_eq!((e.status, e.message.as_str()), (400, "max_terminals is a count from 1 to 100, or default."));
}

/// A board over four projects with `n` working tasks in `alpha`, each in its own idle terminal, and one
/// more busy terminal in each project that isn't a task's.
fn board_with_tasks(n: usize) -> (Board, Vec<i64>) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let names = ["alpha", "bravo", "charlie", "delta"];
    let roots: Vec<String> = names
        .iter()
        .map(|n| {
            let r = dir.path().join(n);
            std::fs::create_dir_all(&r).unwrap();
            r.to_string_lossy().to_string()
        })
        .collect();
    let projects: Vec<Value> = names.iter().zip(&roots).map(|(n, r)| json!({"name": n, "path": r})).collect();
    app.db.set_setting("midna_projects", Some(&Value::Array(projects).to_string())).unwrap();
    let b = Board { app, repo: roots[3].clone(), _dir: dir };
    let mut sessions: Vec<Value> = roots
        .iter()
        .map(|r| json!({"id": format!("{r}-own"), "name": "Mine", "agent": "claude", "cwd": r, "status": {"state": "working"}}))
        .collect();
    sessions.extend((0..n).map(|i| json!({"id": format!("t{i}"), "name": "Task", "agent": "claude", "cwd": roots[0], "status": {"state": "idle"}})));
    midna::sync(&b.app, &sessions, &[]).unwrap();
    let tasks = (0..n)
        .map(|i| {
            let id = b.post("/tasks", json!({"title": format!("Work {i}"), "detail": "Do it.", "project": "alpha", "ships_pr": false}))["id"].as_i64().unwrap();
            board::update_task(&b.app, id, fields!["status" => "working", "session_id" => format!("t{i}")]).unwrap();
            id
        })
        .collect();
    (b, tasks)
}

#[test]
fn the_board_runs_eight_task_terminals_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    assert_eq!(app.cfg.terminals.global_max, 8);
}

#[test]
fn a_start_over_the_board_cap_waits_for_a_free_slot() {
    let (b, tasks) = board_with_tasks(2);
    let out = b.post("terminals", json!({"max_terminals": 2}));
    assert_eq!((out["max_terminals"].as_i64(), out["running"].as_i64()), (Some(2), Some(2)), "the busy terminals that aren't tasks' don't count");
    let t = b.queued_task_in("delta");
    assert_eq!(b.get(&format!("tasks/T{t}"))["waiting"], "Waits for a free slot: 2 of 2 task terminals running across the board");
    assert!(!b.takes_start(), "delta has room, the board doesn't");

    board::update_task(&b.app, tasks[0], fields!["status" => "done"]).unwrap();
    assert!(b.get(&format!("tasks/T{t}"))["waiting"].is_null());
    assert!(b.takes_start(), "a slot freed up");
    assert_eq!(b.get("terminals")["running"], 2, "the start being opened holds its slot");
}

#[test]
fn the_board_cap_holds_a_start_in_a_new_terminal_too() {
    let (b, _) = board_with_tasks(2);
    b.post("terminals", json!({"max_terminals": "2"}));
    let id = b.post("/tasks", json!({"title": "Now", "detail": "Do it.", "project": "delta", "ships_pr": false}))["id"].as_i64().unwrap();
    let from_app: Query = [(api::FROM.to_string(), "app".to_string())].into_iter().collect();
    api::dispatch(&b.app, "POST", &format!("/tasks/T{id}/start"), &from_app, &json!({"mode": "new"})).unwrap();
    assert!(!b.takes_start());

    let out = b.post("terminals", json!({"max_terminals": "default"}));
    assert_eq!((out["max_terminals"].as_i64(), out["max_terminals_set"].is_null()), (Some(8), true));
    assert!(b.takes_start());
}

#[test]
fn the_board_cap_is_a_count_from_one_to_a_hundred() {
    let (b, _) = board_with_tasks(0);
    let e = api::dispatch(&b.app, "POST", "terminals", &Query::new(), &json!({"max_terminals": 0})).unwrap_err();
    assert_eq!(e.status, 400);
    assert_eq!(b.post("terminals", json!({"max_terminals": 100}))["max_terminals"], 100);
    let e = api::dispatch(&b.app, "POST", "terminals", &Query::new(), &json!({"max_terminals": 101})).unwrap_err();
    assert_eq!((e.status, e.message.as_str()), (400, "max_terminals is a count from 1 to 100, or default."));
}

/// A board that holds task starts while the Mac reads `load1` on 12 cores with this memory pressure.
fn board_on_a_mac(load1: f64, memory: &str) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.terminals.hold_load_per_core = 1.5;
    cfg.terminals.hold_memory = "warning".into();
    cfg.terminals.hold_free_memory_gb = 2.0;
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let repo = repo.to_string_lossy().to_string();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo}]).to_string())).unwrap();
    let b = Board { app, repo, _dir: dir };
    b.mac(load1, memory);
    b
}

impl Board {
    fn mac(&self, load1: f64, memory: &str) {
        pressure::fix_for_tests(&self.app, Some(pressure::Reading { load1, cpus: 12, memory: memory.into(), from: "mac", ..Default::default() }));
    }
}

#[test]
fn a_start_waits_while_the_mac_is_loaded_and_says_why() {
    let b = board_on_a_mac(30.0, "normal");
    let t = b.queued_task();
    assert_eq!(b.get(&format!("tasks/T{t}"))["waiting"], "Waits for the Mac to calm down: load 30.0 on 12 cores (starts wait from 18)");
    assert!(!b.takes_start(), "terminals are free, the Mac isn't");
    assert_eq!(b.get("terminals")["line"], "0 of 8 task terminals running (cap from config.toml)\nNew starts wait: load 30.0 on 12 cores (starts wait from 18)");

    b.mac(12.0, "normal");
    assert!(b.get(&format!("tasks/T{t}"))["waiting"].is_null());
    assert!(b.takes_start(), "the load calmed down");
}

#[test]
fn a_start_waits_while_memory_runs_low() {
    let b = board_on_a_mac(2.0, "warning");
    let t = b.queued_task();
    assert_eq!(b.get(&format!("tasks/T{t}"))["waiting"], "Waits for free memory: macOS memory pressure is warning");
    assert!(!b.takes_start());
    let out = b.get("terminals");
    assert_eq!((out["held"].clone(), out["pressure"]["memory"].clone()), (json!("Waits for free memory: macOS memory pressure is warning"), json!("warning")));

    b.mac(2.0, "normal");
    assert!(b.takes_start());
}

#[test]
fn held_starts_open_one_per_fresh_reading_once_the_load_calms() {
    let b = board_on_a_mac(30.0, "normal");
    let held: Vec<i64> = (0..3).map(|_| b.queued_task()).collect();
    assert!(!b.takes_start());

    b.mac(12.0, "normal");
    runner::start_queued(&b.app).unwrap();
    assert!(b.takes_start(), "the first held start opens");
    assert!(!b.takes_start(), "the rest wait for a fresh reading");
    let waiting: Vec<Value> = held.iter().map(|t| b.get(&format!("tasks/T{t}"))["waiting"].clone()).collect();
    assert!(waiting.contains(&json!(pressure::PACED)), "{waiting:?}");

    b.mac(12.0, "normal");
    assert!(b.takes_start(), "one more opens on the next reading");
    assert!(!b.takes_start());
    b.mac(12.0, "normal");
    assert!(b.takes_start());

    // A reading on which no start opened ends the pacing.
    b.mac(12.0, "normal");
    b.mac(12.0, "normal");
    b.queued_task();
    b.queued_task();
    assert!(b.takes_start() && b.takes_start(), "no hold cleared just now: starts open together");
}

#[test]
fn a_hold_coming_back_while_pacing_holds_again() {
    let b = board_on_a_mac(30.0, "normal");
    b.queued_task();
    b.queued_task();
    b.mac(12.0, "normal");
    runner::start_queued(&b.app).unwrap();
    assert!(b.takes_start());
    b.mac(30.0, "normal");
    runner::start_queued(&b.app).unwrap();
    assert!(!b.takes_start(), "held on the load again");
    b.mac(12.0, "normal");
    runner::start_queued(&b.app).unwrap();
    assert!(b.takes_start(), "the hold cleared again: the next opens");
}

#[test]
fn a_start_waits_while_free_memory_is_under_the_floor() {
    let b = board_on_a_mac(2.0, "normal");
    let low = pressure::Reading { load1: 2.0, cpus: 12, memory: "normal".into(), memory_free: 1 << 30, midnad_cpu: 150, midnad_memory: 2 << 30, from: "midna" };
    pressure::fix_for_tests(&b.app, Some(low.clone()));
    let t = b.queued_task();
    assert_eq!(b.get(&format!("tasks/T{t}"))["waiting"], "Waits for free memory: 1 GB free (starts wait under 2 GB); midnad itself runs hot (150% CPU, 2 GB)");
    assert!(!b.takes_start());
    assert_eq!(b.get("terminals")["pressure"]["memory_free_bytes"], json!(1u64 << 30));

    // An older Midna sends no free memory: unknown, never a hold.
    pressure::fix_for_tests(&b.app, Some(pressure::Reading { memory_free: 0, ..low }));
    assert!(b.takes_start());
}

#[test]
fn the_mac_s_load_holds_no_start_when_the_check_is_off() {
    let b = new_board(5);
    pressure::fix_for_tests(&b.app, Some(pressure::Reading { load1: 280.0, cpus: 12, memory: "critical".into(), from: "mac", ..Default::default() }));
    b.queued_task();
    assert!(b.takes_start(), "tests' boards turn the check off");
}
