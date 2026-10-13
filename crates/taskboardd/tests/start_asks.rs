//! A refused `tb start` asks the owner with a notification (Start / Not now) instead of leaving a dead session.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::dispatch::{answer_start, start_asks, start_notification_id, start_params};
use taskboardd::util::RowExt;
use taskboardd::{board, midna, p, reports};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

/// A board that notifies (the runner is off, so nothing reaches Midna).
fn board() -> Board {
    board_with(true)
}

fn board_with(notify: bool) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.notify = notify;
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

fn tap(action: &str) -> Value {
    json!({"response": {"kind": "action", "action": action}})
}

impl Board {
    fn report(&self, event: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": ""});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap()
    }
    fn said(&self, prompt: &str) {
        self.report("hook.prompt", json!({"prompt": prompt}));
    }
    fn task(&self, title: &str) -> i64 {
        api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &json!({"title": title, "detail": "Do it.", "project": "webapp"})).unwrap()["id"]
            .as_i64()
            .unwrap()
    }
    fn tb_start(&self, id: i64) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", &format!("/tasks/T{id}/start"), &Query::new(), &json!({"mode": "new", "via_session": "s1"}))
            .map_err(|e| (e.status, e.message))
    }
    fn tb_run(&self, g: i64) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", &format!("/goals/G{g}/run"), &Query::new(), &json!({"via_session": "s1"})).map_err(|e| (e.status, e.message))
    }
    fn status(&self, id: i64) -> String {
        board::get_task(&self.app, id).unwrap().st("status")
    }
    fn log(&self, id: i64) -> Vec<String> {
        self.app.db.q("SELECT text FROM events WHERE task_id = ? ORDER BY id", p![id]).unwrap().iter().map(|r| r.st("text")).collect()
    }
    /// Whether `id` was started from the notification (with the runner off, a start is its log line).
    fn started(&self, id: i64) -> bool {
        self.log(id).iter().any(|l| l == "Started by Sam from a notification")
    }
    fn refs(&self) -> Vec<(String, String)> {
        start_asks(&self.app).iter().map(|a| (a["ref"].as_str().unwrap_or("").to_string(), a["group"].as_str().unwrap_or("").to_string())).collect()
    }
}

#[test]
fn a_refused_start_asks_the_owner_and_tells_the_agent_not_to_retry() {
    let b = board();
    let id = b.task("Add login");
    b.said("what do you think of the login page?");
    let (code, why) = b.tb_start(id).unwrap_err();
    assert_eq!(code, 403);
    assert!(why.contains("Sam was asked in a notification"), "{why}");
    assert!(why.contains("don't run it again"), "{why}");
    let asks = start_asks(&b.app);
    assert_eq!(asks.len(), 1);
    assert_eq!(asks[0]["ref"].as_str().unwrap_or("").to_string(), format!("T{id}"));
    assert_eq!(asks[0]["session_id"].as_str().unwrap_or("").to_string(), "s1");
    assert_eq!(asks[0]["prompt"].as_str().unwrap_or("").to_string(), "what do you think of the login page?");
    assert_eq!(asks[0]["run"]["mode"], "new");

    let p = start_params(&b.app, &asks[0]["group"].as_str().unwrap_or("").to_string(), &asks);
    assert_eq!(p["title"], format!("Start T{id}?"));
    assert_eq!(p["id"], start_notification_id(&format!("T{id}")));
    assert_eq!(p["id"], format!("taskboard-start-T{id}"));
    assert_eq!(p["actions"], json!(["Start", "Not now"]));
    assert_eq!(p["category"], "board");
    let body = p["body"].as_str().unwrap();
    assert!(body.contains("Add login"), "{body}");
    assert!(body.contains("“what do you think of the login page?”"), "{body}");
}

#[test]
fn without_notifications_the_agent_is_told_how_the_owner_starts_it() {
    let b = board_with(false);
    let id = b.task("Add login");
    let (code, why) = b.tb_start(id).unwrap_err();
    assert_eq!(code, 403);
    assert!(why.contains("Sam can press Start"), "{why}");
}

#[test]
fn start_on_the_notification_starts_it_as_the_owner() {
    let b = board();
    let id = b.task("Add login");
    b.said("hmm");
    b.tb_start(id).unwrap_err();
    answer_start(&b.app, &format!("T{id}"), &tap("Start")).unwrap();
    assert!(start_asks(&b.app).is_empty());
    assert!(b.started(id), "{:?}", b.log(id));
}

#[test]
fn not_now_or_a_dismissal_drops_it_and_a_click_leaves_it() {
    let b = board();
    let id = b.task("Add login");
    b.said("hmm");
    b.tb_start(id).unwrap_err();
    let g = format!("T{id}");
    answer_start(&b.app, &g, &json!({"response": {"kind": "clicked"}})).unwrap();
    assert_eq!(start_asks(&b.app).len(), 1);
    answer_start(&b.app, &g, &tap("Not now")).unwrap();
    assert!(start_asks(&b.app).is_empty());
    assert!(!b.started(id));

    b.tb_start(id).unwrap_err();
    answer_start(&b.app, &g, &json!({"response": {"kind": "dismissed"}})).unwrap();
    assert!(start_asks(&b.app).is_empty());
    assert!(!b.started(id));
}

#[test]
fn only_a_stored_ask_and_its_latest_notification_count() {
    let b = board();
    let id = b.task("Add login");
    let other = b.task("Add logout");
    b.said("hmm");
    b.tb_start(id).unwrap_err();
    // Nobody asked to start `other`.
    answer_start(&b.app, &format!("T{other}"), &tap("Start")).unwrap();
    assert!(!b.started(other));
    // An answer to an older notification of the same ask.
    let stale = json!({"response": {"kind": "action", "action": "Start"}, "sent_at": "2020-01-01T00:00:00Z"});
    answer_start(&b.app, &format!("T{id}"), &stale).unwrap();
    assert!(!b.started(id));
    assert_eq!(start_asks(&b.app).len(), 1);
}

#[test]
fn one_ask_per_ref_and_one_notification_per_turn() {
    let b = board();
    let (t1, t2, t3) = (b.task("One"), b.task("Two"), b.task("Three"));
    b.said("hmm");
    b.tb_start(t1).unwrap_err();
    b.tb_start(t2).unwrap_err();
    b.tb_start(t1).unwrap_err();
    // The resend replaced the first ask, so T2 heads the turn's group now.
    let g = format!("T{t2}");
    assert_eq!(b.refs(), vec![(format!("T{t2}"), g.clone()), (format!("T{t1}"), g.clone())]);
    let p = start_params(&b.app, &g, &start_asks(&b.app));
    assert_eq!(p["title"], format!("Start T{t2} and T{t1}?"));
    assert_eq!(p["id"], start_notification_id(&g));

    // A new turn is a new notification.
    b.said("ok and another");
    b.tb_start(t3).unwrap_err();
    assert_eq!(b.refs().last().unwrap(), &(format!("T{t3}"), format!("T{t3}")));

    answer_start(&b.app, &g, &tap("Start")).unwrap();
    assert_eq!(b.refs(), vec![(format!("T{t3}"), format!("T{t3}"))]);
    assert!(b.started(t1) && b.started(t2));
    assert!(!b.started(t3));
}

#[test]
fn the_ask_goes_when_the_owner_starts_it_or_the_task_goes() {
    let b = board();
    let (t1, t2) = (b.task("One"), b.task("Two"));
    b.said("hmm");
    b.tb_start(t1).unwrap_err();
    b.said("hmm again");
    b.tb_start(t2).unwrap_err();
    api::dispatch(&b.app, "POST", &format!("/tasks/T{t1}/start"), &from_app(), &json!({"mode": "new"})).unwrap();
    assert_eq!(b.refs(), vec![(format!("T{t2}"), format!("T{t2}"))]);

    board::update_task(&b.app, t2, vec![("status", json!("done"))]).unwrap();
    taskboardd::dispatch::tick(&b.app).unwrap();
    assert!(start_asks(&b.app).is_empty());
}

#[test]
fn a_refused_goal_run_asks_too() {
    let b = board();
    let g = api::dispatch(&b.app, "POST", "/goals", &Query::new(), &json!({"name": "Settings", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    let id = b.task("Chips");
    board::update_task(&b.app, id, vec![("status", json!("planned")), ("goal_id", json!(g))]).unwrap();
    b.said("hmm");
    let (code, why) = b.tb_run(g).unwrap_err();
    assert_eq!(code, 403);
    assert!(why.contains("Sam was asked in a notification"), "{why}");
    assert_eq!(b.refs(), vec![(format!("G{g}"), format!("G{g}"))]);
    answer_start(&b.app, &format!("G{g}"), &tap("Start")).unwrap();
    assert!(start_asks(&b.app).is_empty());
    assert_ne!(b.status(id), "planned");
}

#[test]
fn outside_work_hours_start_queues_it_and_the_notification_says_so() {
    use chrono::Datelike;
    let b = board();
    let days = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    let today = days[chrono::Local::now().weekday().num_days_from_monday() as usize];
    let others: Vec<&str> = days.into_iter().filter(|d| *d != today).collect();
    b.app.db.set_setting("work_hours", Some(&json!({"on": true, "start": "09:00", "end": "17:00", "days": others}).to_string())).unwrap();
    let id = b.task("Add login");
    b.said("hmm");
    b.tb_start(id).unwrap_err();
    let body = start_params(&b.app, &format!("T{id}"), &start_asks(&b.app))["body"].as_str().unwrap().to_string();
    assert!(body.contains("Outside work hours: Start queues it for"), "{body}");

    answer_start(&b.app, &format!("T{id}"), &tap("Start")).unwrap();
    assert!(start_asks(&b.app).is_empty());
    let t = board::get_task(&b.app, id).unwrap();
    assert_eq!(t.st("status"), "queued");
    assert_eq!(t.st("pickup"), "new");
    let log = b.log(id);
    assert!(log.iter().any(|l| l.starts_with("Started by Sam from a notification; it starts when work hours open")), "{log:?}");
}
