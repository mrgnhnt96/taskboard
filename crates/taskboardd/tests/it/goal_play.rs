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
