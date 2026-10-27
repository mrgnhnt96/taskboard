//! The home page (`GET /home`): a goal shows the wave it's on and what's still open from earlier
//! waves; planned and finished work stays off it; the alerts come with their task.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, dispatch, midna, reports};

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
    Board { app, dir }
}

impl Board {
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn set(&self, id: i64, f: Vec<(&str, Value)>) {
        board::update_task(&self.app, id, f).unwrap();
    }
    fn home(&self) -> Value {
        self.get("home")
    }
}

fn refs(cards: &Value) -> Vec<String> {
    cards.as_array().unwrap().iter().map(|c| c["ref"].as_str().unwrap().to_string()).collect()
}

/// A goal with tasks in waves 1, 1, 2, 2 and 3, started.
fn goal(b: &Board) -> (i64, Vec<i64>) {
    midna::sync(&b.app, &[json!({"id": "s1", "name": "Planner", "agent": "claude", "cwd": b.dir.path().join("webapp").to_string_lossy(), "status": {"state": "idle"}})], &[]).unwrap();
    let g = b.post("/goals", json!({"name": "Login", "project": "webapp"}))["id"].as_i64().unwrap();
    let r = reports::handle(&b.app, json!({"event": "tb.propose", "session": "s1", "goal": format!("G{g}"),
        "tasks": ["Model::add it::1", "Form::build it::1", "Wire::connect::2", "Errors::show them::2", "Docs::write::3"]}), false).unwrap();
    let ids: Vec<i64> = r["created"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()[1..].parse().unwrap()).collect();
    (g, ids)
}

#[test]
fn a_planned_goal_stays_off_home() {
    let b = new_board();
    goal(&b);
    assert_eq!(b.home()["goals"], json!([]));
}

#[test]
fn a_goal_shows_its_wave_and_what_earlier_waves_left_open() {
    let b = new_board();
    let (g, t) = goal(&b);
    api::dispatch(&b.app, "POST", &format!("goals/G{g}/run"), &[(api::FROM.to_string(), "app".to_string())].into_iter().collect::<Query>(), &json!({})).unwrap();
    let h = b.home();
    let e = &h["goals"][0];
    assert_eq!(e["goal"]["ref"], format!("G{g}"));
    assert_eq!(e["wave"], 1, "nothing in flight: the first wave with a task queued");
    assert_eq!(e["waves"], 3);
    assert_eq!(e["now"], json!([]));
    assert_eq!(e["queued"], 2);

    // Wave 1: one task done and merged, one done with its PR still open; wave 2 working and queued.
    b.set(t[0], vec![("status", json!("done"))]);
    b.set(t[1], vec![("status", json!("done")), ("pr_num", json!(12)), ("pr_state", json!("OPEN"))]);
    b.set(t[2], vec![("status", json!("working"))]);
    b.set(t[4], vec![("status", json!("queued"))]);
    let e = &b.home()["goals"][0];
    assert_eq!(e["wave"], 2);
    assert_eq!(refs(&e["now"]), vec![format!("T{}", t[2])]);
    assert_eq!(e["queued"], 1, "wave 2's queued task; wave 3's isn't counted");
    assert_eq!(refs(&e["left"]), vec![format!("T{}", t[1])], "the open PR from wave 1");
    assert_eq!(e["left_waves"], json!([1]));

    // Its PR merges: wave 1 has nothing left.
    b.set(t[1], vec![("pr_state", json!("MERGED"))]);
    assert_eq!(b.home()["goals"][0]["left"], json!([]));
}

#[test]
fn a_task_whose_pr_is_open_isnt_done_and_keeps_its_goal_in_progress() {
    let b = new_board();
    let (g, t) = goal(&b);
    api::dispatch(&b.app, "POST", &format!("goals/G{g}/run"), &[(api::FROM.to_string(), "app".to_string())].into_iter().collect::<Query>(), &json!({})).unwrap();
    for id in &t {
        b.set(*id, vec![("status", json!("done"))]);
    }
    b.set(t[4], vec![("pr_num", json!(12)), ("pr_state", json!("OPEN"))]);
    let c = b.get(&format!("goals/G{g}"));
    assert_eq!((&c["done"], &c["review"], &c["total"]), (&json!(4), &json!(1), &json!(5)));
    assert_eq!(c["state"], "Waits for PR #12 to merge");
    assert!(c["finished_at"].is_null());
    assert_eq!(refs(&b.home()["goals"].as_array().unwrap().iter().map(|e| e["goal"].clone()).collect()), vec![format!("G{g}")]);
    assert_eq!(b.get("summary")["goals"][0]["done"], 4, "it's still listed with the goals in progress");

    b.set(t[4], vec![("pr_state", json!("MERGED"))]);
    let c = b.get(&format!("goals/G{g}"));
    assert_eq!((&c["done"], &c["review"], &c["state"]), (&json!(5), &json!(0), &json!("Done")));
    assert_eq!(b.get("summary")["goals"], json!([]));
}

#[test]
fn a_miss_on_the_done_when_alerts_the_owner_and_keeps_the_goal_open_until_its_settled() {
    let b = new_board();
    let (g, t) = goal(&b);
    api::dispatch(&b.app, "POST", &format!("goals/G{g}/run"), &[(api::FROM.to_string(), "app".to_string())].into_iter().collect::<Query>(), &json!({})).unwrap();
    b.set(t[4], vec![("status", json!("working")), ("session_id", json!("s1"))]);
    let found = reports::handle(&b.app, json!({"event": "tb.found", "session": "s1", "title": "Sign-in fails on Android", "kind": "gap", "misses_goal": true}), false).unwrap();
    assert_eq!(found["misses_goal"], true);
    let issue: i64 = found["issue"].as_str().unwrap()[1..].parse().unwrap();
    let alerts = b.get("/state")["alerts"].clone();
    assert!(alerts.as_array().unwrap().iter().any(|a| a["text"].as_str().unwrap().contains("misses its done-when")), "{alerts}");

    for id in &t {
        b.set(*id, vec![("status", json!("done"))]);
    }
    let c = b.get(&format!("goals/G{g}"));
    assert_eq!((&c["done"], &c["misses"], &c["state"]), (&json!(5), &json!(1), &json!("Outcome not met: 1 miss")));
    assert!(c["finished_at"].is_null());
    assert_eq!(c["backlog"][0]["misses_goal"], true, "the goal page marks the miss");
    assert!(!board::goal_finished(&b.app, g).unwrap());
    assert_eq!(b.get("summary")["goals"][0]["done"], 5, "it's still listed with the goals in progress");
    assert!(b.get("/state")["alerts"].as_array().unwrap().iter().any(|a| a["text"].as_str().unwrap().contains("misses its done-when")), "finishing the task doesn't clear it");

    b.post(&format!("backlog/B{issue}/drop"), json!({"reason": "fine on Android after all"}));
    let c = b.get(&format!("goals/G{g}"));
    assert_eq!((&c["misses"], &c["state"]), (&json!(0), &json!("Done")));
    assert_eq!(b.get("summary")["goals"], json!([]));
    assert!(!b.get("/state")["alerts"].as_array().unwrap().iter().any(|a| a["text"].as_str().unwrap().contains("misses its done-when")));
}

#[test]
fn a_failed_task_isnt_done_and_its_goal_isnt_finished() {
    let b = new_board();
    let (g, t) = goal(&b);
    api::dispatch(&b.app, "POST", &format!("goals/G{g}/run"), &[(api::FROM.to_string(), "app".to_string())].into_iter().collect::<Query>(), &json!({})).unwrap();
    for id in &t {
        b.set(*id, vec![("status", json!("done"))]);
    }
    b.set(t[2], vec![("failed", json!(1))]);
    let c = b.get(&format!("goals/G{g}"));
    assert_eq!((&c["done"], &c["failed"], &c["state"]), (&json!(4), &json!(1), &json!("Outcome not met: 1 failed")));
    assert!(!board::goal_finished(&b.app, g).unwrap());
}

#[test]
fn a_finished_goal_leaves_home() {
    let b = new_board();
    let (g, t) = goal(&b);
    api::dispatch(&b.app, "POST", &format!("goals/G{g}/run"), &[(api::FROM.to_string(), "app".to_string())].into_iter().collect::<Query>(), &json!({})).unwrap();
    for id in &t {
        b.set(*id, vec![("status", json!("done"))]);
    }
    assert_eq!(b.home()["goals"], json!([]));
}

#[test]
fn tasks_outside_a_goal_get_their_projects_entry() {
    let b = new_board();
    let id = b.post("/tasks", json!({"title": "Fix the logo", "project": "webapp"}))["id"].as_i64().unwrap();
    b.set(id, vec![("status", json!("working"))]);
    let e = &b.home()["goals"][0];
    assert_eq!(e["goal"], Value::Null);
    assert_eq!(e["project"], "webapp");
    assert_eq!(refs(&e["now"]), vec![format!("T{id}")]);
}

#[test]
fn needs_carry_the_task_and_its_goal() {
    let b = new_board();
    let (g, t) = goal(&b);
    b.set(t[0], vec![("status", json!("needs")), ("question", json!("Keep the old form?"))]);
    dispatch::add_alert(&b.app, &format!("T{}: Keep the old form?", t[0]), Some(t[0]), Some(g), None, None).unwrap();
    let n = &b.home()["needs"][0];
    assert_eq!(n["task"], format!("T{}", t[0]));
    assert_eq!(n["card"]["title"], "Model");
    assert_eq!(n["card"]["status"], "needs");
    assert_eq!(n["goal_name"], "Login");
}

#[test]
fn a_task_that_needs_you_without_an_alert_still_shows() {
    let b = new_board();
    let (_, t) = goal(&b);
    b.set(t[1], vec![("status", json!("needs")), ("needs_reason", json!("question")), ("question", json!("Which icon?"))]);
    let n = &b.home()["needs"][0];
    assert_eq!(n["id"], format!("task:T{}", t[1]), "an id of its own to dismiss or snooze");
    assert_eq!(n["task"], format!("T{}", t[1]));
    assert_eq!(n["text"], "Which icon?");
    assert_eq!(n["goal_name"], "Login");
}

#[test]
fn dismissing_a_task_with_no_alert_puts_it_away_until_it_asks_something_new() {
    let b = new_board();
    let (_, t) = goal(&b);
    b.set(t[1], vec![("status", json!("needs")), ("needs_reason", json!("lost")), ("lost", json!(1))]);
    let id = format!("task:T{}", t[1]);
    b.post(&format!("alerts/{id}/dismiss"), json!({}));
    assert_eq!(b.home()["needs"], json!([]));

    b.set(t[1], vec![("latest", json!("Still lost"))]);
    assert_eq!(b.home()["needs"], json!([]), "a change that asks nothing new keeps it away");

    b.set(t[1], vec![("needs_reason", json!("question")), ("lost", json!(0)), ("question", json!("Which icon?"))]);
    assert_eq!(b.home()["needs"][0]["id"], id, "a new question brings it back");
}

#[test]
fn snoozing_a_task_with_no_alert_puts_it_away_until_its_time() {
    let b = new_board();
    let (_, t) = goal(&b);
    b.set(t[1], vec![("status", json!("needs")), ("needs_reason", json!("lost")), ("lost", json!(1))]);
    let id = format!("task:T{}", t[1]);
    let a = b.post(&format!("alerts/{id}/snooze"), json!({"mins": 60}))["alert"].clone();
    assert!(a["snoozed_until"].is_string(), "the toast says until when");
    assert_eq!(b.home()["needs"], json!([]));

    let done = t[0];
    assert_eq!(api::dispatch(&b.app, "POST", &format!("alerts/task:T{done}/snooze"), &Query::new(), &json!({"mins": 15})).unwrap_err().status, 404, "a task that doesn't need you");
    assert!(api::dispatch(&b.app, "POST", &format!("alerts/{id}/snooze"), &Query::new(), &json!({})).is_err());

    b.post(&format!("alerts/{id}/snooze"), json!({"until": "back"}));
    assert_eq!(b.home()["needs"], json!([]), "until the owner is away and back");
}

#[test]
fn a_snoozed_need_leaves_home_until_its_snooze_ends() {
    let b = new_board();
    let (g, t) = goal(&b);
    b.set(t[0], vec![("status", json!("needs")), ("question", json!("Keep the old form?"))]);
    dispatch::add_alert(&b.app, &format!("T{}: Keep the old form?", t[0]), Some(t[0]), Some(g), None, None).unwrap();
    let id = b.home()["needs"][0]["id"].as_str().unwrap().to_string();

    let a = b.post(&format!("alerts/{id}/snooze"), json!({"mins": 180}))["alert"].clone();
    let until = taskboardd::util::parse_iso(a["snoozed_until"].as_str().unwrap()).unwrap();
    assert!((until - taskboardd::util::now_ts() - 3.0 * 3600.0).abs() < 5.0);
    assert_eq!(b.home()["needs"], json!([]), "its task doesn't show in its place");

    b.post(&format!("alerts/{id}/snooze"), json!({"until": "back"}));
    let a = &dispatch::alerts(&b.app)[0];
    assert!(a["snoozed_until"].is_null() && a["snoozed_back"].is_string());
    assert_eq!(b.home()["needs"], json!([]), "until the owner is away and back");

    let a = b.post(&format!("alerts/{id}/snooze"), json!({"until": "tomorrow"}))["alert"].clone();
    assert!(a["snoozed_back"].is_null());
    let until = taskboardd::util::parse_iso(a["snoozed_until"].as_str().unwrap()).unwrap();
    assert!(until > taskboardd::util::now_ts() && until < taskboardd::util::now_ts() + 2.0 * 86400.0);

    dispatch::snooze_alert(&b.app, &id, -1).unwrap();
    assert_eq!(b.home()["needs"][0]["id"], id, "back once its time comes");
}

#[test]
fn a_snooze_takes_minutes_tomorrow_or_back() {
    let b = new_board();
    let (g, t) = goal(&b);
    dispatch::add_alert(&b.app, "Look at this", Some(t[0]), Some(g), None, None).unwrap();
    let id = dispatch::alerts(&b.app)[0]["id"].as_str().unwrap().to_string();
    for body in [json!({}), json!({"mins": 0}), json!({"until": "later"})] {
        assert!(api::dispatch(&b.app, "POST", &format!("alerts/{id}/snooze"), &Query::new(), &body).is_err(), "{body}");
    }
    assert_eq!(api::dispatch(&b.app, "POST", "alerts/nope/snooze", &Query::new(), &json!({"mins": 15})).unwrap_err().status, 404);
}
