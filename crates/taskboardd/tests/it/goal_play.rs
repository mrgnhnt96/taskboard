//! Play on a goal (`POST /goals/:id/run`, `tb start G<n>`): it resumes the goal's tasks parked for Start,
//! the ones whose terminal closed among them, and says why each task of the current wave that still can't
//! start waits.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query, FROM};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, midna, reports, runner};

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
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn play(&self, gid: i64) -> Value {
        let q: Query = [(FROM.to_string(), "app".to_string())].into_iter().collect();
        api::dispatch(&self.app, "POST", &format!("/goals/G{gid}/run"), &q, &json!({})).unwrap_or_else(|e| panic!("run: {}", e.message))
    }
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
    fn task(&self, title: &str, extra: Value) -> i64 {
        let mut body = json!({"title": title, "detail": "Do it.", "project": "webapp", "pickup": "new"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", body)["id"].as_i64().unwrap()
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    /// Midna says the owner closed terminal `sid`, and Claude's session there ended.
    fn owner_closes(&self, sid: &str) {
        let e = json!({"seq": 1, "at": taskboardd::util::now_iso(), "kind": "session.closed", "actor": {"kind": "human"}, "session_id": sid, "data": {"force": true}});
        midna::closed_events(&self.app, &[e]).unwrap();
        self.report("hook.session_end", sid, json!({"reason": "other"}));
    }
}

fn goal(b: &Board, in_order: bool) -> i64 {
    b.post("/goals", json!({"name": "Move to Oracle", "project": "webapp", "outcome": "it runs on Oracle", "run_in_order": in_order, "max_terminals": 3}))["id"].as_i64().unwrap()
}

#[test]
fn play_resumes_a_task_whose_terminal_closed() {
    let b = new_board();
    let gid = goal(&b, true);
    let id = b.task("Move the database", json!({"goal_id": gid}));
    b.session("s1");
    b.report("tb.take", "s1", json!({"task": format!("T{id}")}));
    b.owner_closes("s1");
    let t = b.row(id);
    assert_eq!((t.s("status"), t.b("lost")), (Some("needs"), true), "the owner closed it, so it waits for Start");
    assert!(runner::start_queued(&b.app).unwrap().is_empty());

    let run = b.play(gid);
    assert_eq!(run["resumed_now"], 1, "{run}");
    let t = b.row(id);
    assert_eq!((t.s("status"), t.b("lost"), t.s("session_id")), (Some("queued"), false, None));
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![id], "Play starts it again");
}

#[test]
fn play_starts_a_task_that_waits_for_start() {
    let b = new_board();
    let gid = goal(&b, true);
    let id = b.task("Move the database", json!({"goal_id": gid, "pickup": "manual"}));
    assert!(runner::start_queued(&b.app).unwrap().is_empty(), "it waits for Start");

    let run = b.play(gid);
    assert_eq!(run["resumed_now"], 1, "{run}");
    assert_eq!(b.row(id).s("pickup"), Some("new"));
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![id]);
}

#[test]
fn play_says_which_task_waits_and_why() {
    let b = new_board();
    let gid = goal(&b, false);
    b.session("s1");
    let h = b.report("tb.new_task", "s1", json!({"title": "Create an Oracle Cloud account", "detail": "Sign up.", "project": "webapp", "kind": "human", "goal": format!("G{gid}")}))
        ["created"][0]
        .as_str()
        .unwrap()
        .to_string();
    let id = b.task("Deploy to Oracle Cloud", json!({"goal_id": gid}));
    b.post(&format!("/tasks/T{id}"), json!({"waits_for": format!("T{}", &h[1..])}));
    let ready = b.task("Write the runbook", json!({"goal_id": gid}));

    let run = b.play(gid);
    assert_eq!(run["waiting"], json!([format!("T{id} waits for you to do {h}")]), "{run}");
    assert!(run["hours_wait"].is_null());
    assert_eq!(run["resumed_now"], 0);
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![ready], "the ready task starts; the one waiting on the owner doesn't");
}

#[test]
fn play_says_what_a_task_waits_on_even_in_the_goals_order() {
    let b = new_board();
    let gid = goal(&b, true);
    b.session("s1");
    let h = b.report("tb.new_task", "s1", json!({"title": "Create an Oracle Cloud account", "detail": "Sign up.", "project": "webapp", "kind": "human", "goal": format!("G{gid}")}))
        ["created"][0]
        .as_str()
        .unwrap()
        .to_string();
    let id = b.task("Deploy to Oracle Cloud", json!({"goal_id": gid}));
    b.post(&format!("/tasks/T{id}"), json!({"waits_for": format!("T{}", &h[1..])}));
    b.task("Write the runbook", json!({"goal_id": gid}));

    let run = b.play(gid);
    assert_eq!(run["waiting"], json!([format!("T{id} waits for you to do {h}")]), "the runbook only waits its turn: {run}");
}

#[test]
fn open_in_claude_shows_its_terminal_once_open() {
    let b = new_board();
    let gid = goal(&b, true);
    let job = |mode: &str| {
        let v = b.post(&format!("/goals/G{gid}/plan"), json!({"mode": mode}));
        let jid = v["job"].as_str().unwrap().trim_start_matches('J').parse::<i64>().unwrap();
        board::job_args(&b.app.db.q1("SELECT * FROM jobs WHERE id = ?", taskboardd::p![jid]).unwrap().unwrap())
    };
    assert_eq!(job("edit").v("focus"), json!(true), "the owner pressed it, so Midna switches to it");
    assert_eq!(job("suggest").v("focus"), Value::Null, "a planner opened for suggestions stays where it is");
}

fn needs(b: &Board) -> Vec<Value> {
    api::dispatch(&b.app, "GET", "home", &Query::new(), &json!({})).unwrap()["needs"].as_array().unwrap().clone()
}

/// A task of goal `gid` taken in terminal `sid`, which the owner then closed.
fn closed_task(b: &Board, gid: i64, title: &str, sid: &str) -> i64 {
    let id = b.task(title, json!({"goal_id": gid}));
    b.session(sid);
    b.report("tb.take", sid, json!({"task": format!("T{id}")}));
    b.owner_closes(sid);
    id
}

#[test]
fn terminals_closed_by_hand_are_one_quiet_line_for_their_goal() {
    let b = new_board();
    let gid = goal(&b, false);
    let one = closed_task(&b, gid, "Move the database", "s1");
    let two = closed_task(&b, gid, "Move the jobs", "s2");

    let rows = needs(&b);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let row = &rows[0];
    assert_eq!(row["id"], format!("stopped:G{gid}"));
    assert_eq!(row["title"], "Move to Oracle · 2 closed");
    assert_eq!(row["project"], "webapp");
    assert!(row["card"].is_null(), "it's the goal's line, not a task's");
    assert!(row["task"].is_null(), "so Dismiss and Snooze don't reach it");
    let mut tasks: Vec<String> = row["stopped"]["tasks"].as_array().unwrap().iter().map(|t| t.as_str().unwrap().to_string()).collect();
    tasks.sort();
    assert_eq!(tasks, vec![format!("T{one}"), format!("T{two}")]);
    assert_eq!(row["waiting"], false, "no agent is waiting on a closed terminal");
    assert_eq!(row["holds"]["goal"], format!("G{gid}"));
}

#[test]
fn a_terminal_that_ended_on_its_own_stays_a_row_of_its_own() {
    let b = new_board();
    let gid = goal(&b, false);
    let id = b.task("Move the database", json!({"goal_id": gid}));
    b.session("s1");
    b.report("tb.take", "s1", json!({"task": format!("T{id}")}));
    board::update_task(&b.app, id, vec![("status", json!("needs")), ("needs_reason", json!("lost")), ("lost", json!(1))]).unwrap();

    let rows = needs(&b);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0]["id"], format!("task:T{id}"));
    assert_eq!(rows[0]["card"]["closed_by_hand"], false);
}

#[test]
fn a_hand_closed_task_outside_any_goal_keeps_its_own_row() {
    let b = new_board();
    let id = b.task("Tidy the logs", json!({}));
    b.session("s1");
    b.report("tb.take", "s1", json!({"task": format!("T{id}")}));
    b.owner_closes("s1");

    let rows = needs(&b);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0]["id"], format!("task:T{id}"));
    assert_eq!(rows[0]["card"]["closed_by_hand"], true);
}

#[test]
fn restarting_a_goal_queues_only_its_closed_tasks() {
    let b = new_board();
    let gid = goal(&b, false);
    let closed = closed_task(&b, gid, "Move the database", "s1");
    let asking = b.task("Move the jobs", json!({"goal_id": gid}));
    board::update_task(&b.app, asking, vec![("status", json!("needs")), ("needs_reason", json!("question")), ("question", json!("Which schema?"))]).unwrap();
    let planned = b.task("Move the docs", json!({"goal_id": gid}));
    let was = b.row(planned).st("status");

    let r = b.post(&format!("/goals/G{gid}/restart"), json!({}));
    assert_eq!(r["restarted_now"], 1, "{r}");
    let t = b.row(closed);
    assert_eq!((t.s("status"), t.b("lost"), t.s("session_id")), (Some("queued"), false, None));
    assert_eq!(b.row(asking).s("status"), Some("needs"), "a question isn't a closed terminal");
    assert_eq!(b.row(planned).st("status"), was, "planned work stays planned");
    assert!(!needs(&b).iter().any(|n| n["id"] == format!("stopped:G{gid}")), "the line goes with them");

    let again = api::dispatch(&b.app, "POST", &format!("/goals/G{gid}/restart"), &Query::new(), &json!({})).unwrap_err();
    assert_eq!(again.status, 409, "{}", again.message);
}

#[test]
fn a_paused_or_set_aside_goal_leaves_the_list_until_it_is_resumed() {
    for field in ["paused", "deprioritized"] {
        let b = new_board();
        let gid = goal(&b, false);
        let closed = closed_task(&b, gid, "Move the database", "s1");
        let asking = b.task("Move the jobs", json!({"goal_id": gid}));
        board::update_task(&b.app, asking, vec![("status", json!("needs")), ("needs_reason", json!("question")), ("question", json!("Which schema?"))]).unwrap();
        board::update_goal(&b.app, gid, vec![(field, json!(1))]).unwrap();
        assert_eq!(needs(&b), Vec::<Value>::new(), "{field}");

        let rest = api::dispatch(&b.app, "POST", &format!("/goals/G{gid}/restart"), &Query::new(), &json!({})).unwrap_err();
        assert_eq!(rest.status, 409, "a goal at rest has no Stopped line: {}", rest.message);

        let run = b.play(gid);
        assert_eq!(run["resumed_now"], 1, "{run}");
        assert_eq!(b.row(closed).s("status"), Some("queued"), "Play already restarts its closed tasks");
    }
}

#[test]
fn a_put_off_item_in_a_resting_goal_leaves_later_too() {
    let b = new_board();
    let gid = goal(&b, false);
    let id = b.task("Move the database", json!({"goal_id": gid}));
    board::update_task(&b.app, id, vec![("status", json!("needs")), ("needs_reason", json!("question")), ("question", json!("Which schema?"))]).unwrap();
    b.post(&format!("/alerts/task:T{id}/snooze"), json!({"mins": 60}));
    let later = |b: &Board| api::dispatch(&b.app, "GET", "home", &Query::new(), &json!({})).unwrap()["later_count"].clone();
    assert_eq!(later(&b), 1);

    board::update_goal(&b.app, gid, vec![("paused", json!(1))]).unwrap();
    assert_eq!(later(&b), 0);
}

#[test]
fn an_urgent_alert_in_a_resting_goal_stays_listed() {
    let b = new_board();
    let gid = goal(&b, false);
    taskboardd::dispatch::raise(&b.app, "The database is down", None, Some(gid), Some("db"), true).unwrap();
    taskboardd::dispatch::raise(&b.app, "Have a look at this", None, Some(gid), Some("look"), false).unwrap();
    board::update_goal(&b.app, gid, vec![("paused", json!(1))]).unwrap();

    let texts: Vec<String> = needs(&b).iter().map(|n| n["text"].as_str().unwrap().to_string()).collect();
    assert_eq!(texts, vec!["The database is down".to_string()]);
}

#[test]
fn a_resting_goals_alert_stays_out_of_the_digest() {
    let b = new_board();
    let gid = goal(&b, false);
    let asked = b.app.db.insert("tasks", taskboardd::fields!["title" => "x", "goal_id" => gid, "status" => "needs", "needs_reason" => "question"]).unwrap();
    taskboardd::dispatch::add_alert(&b.app, "Which port?", Some(asked), Some(gid), None, None).unwrap();
    assert_eq!(taskboardd::digest::members(&b.app).len(), 1);
    board::update_goal(&b.app, gid, vec![("deprioritized", json!(1))]).unwrap();
    assert_eq!(taskboardd::digest::members(&b.app), Vec::<Value>::new());
}
