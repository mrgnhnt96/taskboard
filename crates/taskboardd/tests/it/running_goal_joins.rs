//! A task that joins a running goal (`tb propose`, `tb task new --goal`, `tb backlog task`,
//! `tb task set --goal`) comes in queued so the run picks it up; planned only when asked, or
//! when the goal isn't running.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::Row;
use taskboardd::util::RowExt;
use taskboardd::{board, fields, midna, reports, waitsfor};

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
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Planner", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "idle"}})], &[]).unwrap();
    Board { app, dir }
}

fn ids(r: &Value) -> Vec<i64> {
    r["created"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()[1..].parse().unwrap()).collect()
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn report(&self, event: &str, extra: Value) -> Value {
        let mut body = json!({"event": event, "session": "s1"});
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        reports::handle(&self.app, body, false).unwrap()
    }
    /// A goal with tasks in waves 1 and 2, run or not.
    fn goal(&self, name: &str, run: bool) -> (i64, Vec<i64>) {
        let _ = &self.dir;
        let g = self.post("/goals", json!({"name": name, "project": "webapp"}))["id"].as_i64().unwrap();
        let r = self.report("tb.propose", json!({"goal": format!("G{g}"), "tasks": ["One::do it::1", "Two::do it too::2"]}));
        if run {
            api::dispatch(&self.app, "POST", &format!("goals/G{g}/run"), &[(api::FROM.to_string(), "app".to_string())].into_iter().collect::<Query>(), &json!({})).unwrap();
        }
        (g, ids(&r))
    }
    fn task(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn status(&self, id: i64) -> String {
        self.task(id).st("status")
    }
}

#[test]
fn proposed_tasks_join_a_running_goal_queued_and_wait_for_their_wave() {
    let b = new_board();
    let (g, t) = b.goal("Login", true);
    let r = b.report("tb.propose", json!({"goal": format!("G{g}"), "tasks": ["Late::found later::2"]}));
    assert_eq!(r["status"], "queued");
    assert_eq!(r["why"], format!("queued: G{g} is running"));
    let late = ids(&r)[0];
    assert_eq!(b.status(late), "queued");
    assert_eq!(b.task(late).st("pickup"), b.task(t[0]).st("pickup"), "picked up the way the run's own tasks are");
    assert_eq!(waitsfor::waiting_line(&b.app, &b.task(late)).unwrap(), "Waits for wave 1", "queued, it still waits for its wave");

    let r = b.report("tb.propose", json!({"goal": format!("G{g}"), "tasks": ["Later::next run"], "planned": true}));
    assert_eq!(r["why"], "planned (asked)");
    assert_eq!(b.status(ids(&r)[0]), "planned");
}

#[test]
fn a_goal_that_isnt_running_still_gets_planned_tasks() {
    let b = new_board();
    let (g, _) = b.goal("Login", false);
    let r = b.report("tb.propose", json!({"goal": format!("G{g}"), "tasks": ["Late::found later"]}));
    assert_eq!(r["why"], format!("planned: G{g} isn't running"));
    assert_eq!(b.status(ids(&r)[0]), "planned");

    // A run that finished is no longer running.
    let (g2, t2) = b.goal("Done", true);
    for id in t2 {
        board::update_task(&b.app, id, fields!["status" => "done"]).unwrap();
    }
    let r = b.report("tb.new_task", json!({"goal": format!("G{g2}"), "title": "Afterthought"}));
    assert_eq!(r["status"], "planned");
}

#[test]
fn a_new_task_in_a_running_goal_is_queued_unless_asked_planned() {
    let b = new_board();
    let (g, _) = b.goal("Login", true);
    let r = b.report("tb.new_task", json!({"goal": format!("G{g}"), "title": "Fix the form"}));
    assert_eq!(r["status"], "queued");
    assert_eq!(r["why"], format!("queued: G{g} is running"));
    assert_eq!(b.status(ids(&r)[0]), "queued");

    let r = b.report("tb.new_task", json!({"goal": format!("G{g}"), "title": "Polish", "planned": true}));
    assert_eq!(r["status"], "planned");
    assert_eq!(r["why"], "planned (asked)");
}

#[test]
fn a_backlog_issue_made_a_task_in_a_running_goal_is_queued() {
    let b = new_board();
    let (g, _) = b.goal("Login", true);
    let issue = |title: &str| b.post("/backlog", json!({"title": title, "goal_id": format!("G{g}"), "kind": "bug"}))["id"].as_i64().unwrap();

    let r = b.post(&format!("backlog/B{}/promote", issue("Crash on save")), json!({"where": "goal"}));
    assert_eq!(r["why"], format!("queued: G{g} is running"));
    let t: i64 = r["task"]["id"].as_i64().unwrap();
    assert_eq!(b.status(t), "queued");

    let r = b.post(&format!("backlog/B{}/promote", issue("Slow list")), json!({"where": "goal", "planned": true}));
    let t: i64 = r["task"]["id"].as_i64().unwrap();
    assert_eq!(b.status(t), "planned");
}

#[test]
fn moving_a_task_into_a_running_goal_queues_it() {
    let b = new_board();
    let (g, _) = b.goal("Login", true);
    let (idle, t) = b.goal("Later", false);
    let d = b.post(&format!("tasks/T{}", t[0]), json!({"goal_id": format!("G{g}")}));
    assert_eq!(d["why"], format!("queued: G{g} is running"));
    assert_eq!(b.status(t[0]), "queued");
    assert_eq!(b.task(t[0]).st("pickup"), b.task(t[1]).st("pickup"));

    let d = b.post(&format!("tasks/T{}", t[1]), json!({"goal_id": format!("G{g}"), "status": "planned"}));
    assert_eq!(d["why"], "planned (asked)");
    assert_eq!(b.status(t[1]), "planned");

    // Back into a goal that isn't running: planned again.
    let d = b.post(&format!("tasks/T{}", t[0]), json!({"goal_id": format!("G{idle}")}));
    assert_eq!(d["why"], format!("planned: G{idle} isn't running"));
    assert_eq!(b.status(t[0]), "planned");
}

#[test]
fn one_task_started_on_its_own_doesnt_make_the_goal_running() {
    let b = new_board();
    let (g, t) = b.goal("Login", false);
    board::update_task(&b.app, t[0], fields!["status" => "working"]).unwrap();
    let r = b.report("tb.propose", json!({"goal": format!("G{g}"), "tasks": ["Late::found later"]}));
    assert_eq!(r["why"], format!("planned: G{g} isn't running"), "the rest of the goal is still planned, so it wasn't run");
    assert_eq!(b.status(ids(&r)[0]), "planned");
}

#[test]
fn a_task_waiting_for_a_start_moved_into_a_running_goal_is_picked_up_by_the_run() {
    let b = new_board();
    let (g, _) = b.goal("Login", true);
    let lone = |title: &str| ids(&b.report("tb.new_task", json!({"title": title, "project": "webapp"})))[0];

    let t = lone("Loose end");
    assert_eq!((b.status(t), b.task(t).st("pickup")), ("queued".to_string(), "manual".to_string()));
    b.post(&format!("tasks/T{t}"), json!({"goal_id": format!("G{g}")}));
    assert_eq!((b.status(t), b.task(t).st("pickup")), ("queued".to_string(), "queue".to_string()));

    let t = lone("Hands on");
    b.post(&format!("tasks/T{t}"), json!({"goal_id": format!("G{g}"), "pickup": "manual"}));
    assert_eq!(b.task(t).st("pickup"), "manual", "a pickup asked for with the move is kept");
}
