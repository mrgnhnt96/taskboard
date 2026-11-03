//! With `[backlog] critical = "ask"`, a `blocks: pr` item's fix task waits planned for the owner, who
//! approves it (the task runs) or rejects it (the item goes back to the plain backlog and stops holding).

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::util::{now_iso, RowExt};
use taskboardd::{board, chain, dispatch, samepr};

use crate::critical_hold::{new_board_with, Board};

fn asking() -> Board {
    new_board_with(|c| c.backlog.critical = "ask".into())
}

fn ask(b: &Board, issue: i64) -> Option<Value> {
    dispatch::alerts(&b.app).into_iter().find(|a| a["key"] == samepr::ask_key(issue))
}

fn answer(b: &Board, issue: i64, approve: bool) -> Value {
    b.post(&format!("/backlog/{issue}/critical"), json!({"approve": approve}))
}

/// A task in the goal with an open PR (#12), working in s1 so `found` files from it.
fn held(b: &Board) -> i64 {
    let a = b.task("Sign in", json!({"wave": 1}));
    b.link(a, 12, "feat/sign-in");
    b.work(a);
    a
}

#[test]
fn an_item_waits_planned_for_the_owner_and_still_holds_the_pr() {
    let b = asking();
    let a = held(&b);

    let v = b.found("Tokens leak to the log", "src/auth.rs logs the bearer token");
    let i = b.issue(&v);
    let fix = i.i("task_id").expect("made into a task");
    assert!(i.b("asks_owner"));
    assert_eq!(b.row(fix).st("status"), "planned");
    assert!(v["held"].as_str().unwrap().contains("waits for"), "{v}");
    assert_eq!(b.row(a).st("pr_phase"), "stack", "it holds the PR while it waits");

    let alert = ask(&b, i.id()).expect("a Needs you item");
    let text = alert["text"].as_str().unwrap();
    assert!(text.starts_with(&format!("B{} holds T{a}'s PR #12: Tokens leak to the log.", i.id())), "{text}");
    assert!(text.contains("Why: src/auth.rs logs the bearer token"), "{text}");
    let actions: Vec<(&str, &str)> =
        alert["actions"].as_array().unwrap().iter().map(|x| (x["label"].as_str().unwrap(), x["command"].as_str().unwrap())).collect();
    let (pick1, pick2) = (format!("tb needs answer B{} --pick 1", i.id()), format!("tb needs answer B{} --pick 2", i.id()));
    assert_eq!(actions, vec![("Approve", pick1.as_str()), ("Reject", pick2.as_str())]);

    chain::queue_planned(&b.app, b.goal, "test", "queued", "again").unwrap();
    assert_eq!(b.row(fix).st("status"), "planned", "running the goal doesn't start it before the owner approves");

    let id = alert["id"].as_str().unwrap();
    let e = api::dispatch(&b.app, "POST", &format!("/alerts/{id}/dismiss"), &Query::new(), &json!({})).unwrap_err();
    assert_eq!(e.status, 409, "{}", e.message);
    assert!(ask(&b, i.id()).is_some());
}

#[test]
fn approving_in_a_running_goal_queues_the_fix_and_keeps_the_hold() {
    let b = asking();
    let a = held(&b);
    b.app.db.x("UPDATE goals SET ran_at = ? WHERE id = ?", taskboardd::p![now_iso(), b.goal]).unwrap();
    let i = b.issue(&b.found("Tokens leak to the log", "src/auth.rs logs the bearer token"));
    let fix = i.i("task_id").unwrap();

    let v = answer(&b, i.id(), true);
    assert!(v["text"].as_str().unwrap().contains("is queued"), "{v}");
    assert_eq!(b.row(fix).st("status"), "queued");
    assert!(!board::get_issue(&b.app, i.id()).unwrap().b("asks_owner"));
    assert!(ask(&b, i.id()).is_none());
    assert!(samepr::hold(&b.app, &b.row(a)).unwrap().is_some(), "it holds the PR until the fix is done");
    assert_eq!(b.row(a).st("pr_phase"), "stack");
}

#[test]
fn approving_before_the_goal_runs_leaves_it_for_the_run() {
    let b = asking();
    held(&b);
    let i = b.issue(&b.found("Tokens leak to the log", "src/auth.rs logs the bearer token"));
    let fix = i.i("task_id").unwrap();

    let v = answer(&b, i.id(), true);
    assert!(v["text"].as_str().unwrap().contains("runs with its goal"), "{v}");
    assert_eq!(b.row(fix).st("status"), "planned");
    chain::queue_planned(&b.app, b.goal, "test", "queued", "again").unwrap();
    assert_eq!(b.row(fix).st("status"), "queued");
}

#[test]
fn approving_outside_a_goal_queues_the_fix() {
    let b = asking();
    let a = b.post("/tasks", json!({"title": "Sign in", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
    b.link(a, 12, "feat/sign-in");
    b.work(a);
    let i = b.issue(&b.found("Tokens leak to the log", "src/auth.rs logs the bearer token"));
    let fix = i.i("task_id").unwrap();
    assert_eq!((b.row(fix).st("status").as_str(), b.row(fix).i("goal_id")), ("planned", None));

    answer(&b, i.id(), true);
    assert_eq!(b.row(fix).st("status"), "queued");
}

#[test]
fn rejecting_the_only_item_deletes_the_unstarted_fix_and_lifts_the_hold() {
    let b = asking();
    let a = held(&b);
    let i = b.issue(&b.found("Tokens leak to the log", "src/auth.rs logs the bearer token"));
    let fix = i.i("task_id").unwrap();

    let v = answer(&b, i.id(), false);
    assert!(v["text"].as_str().unwrap().contains("back in the backlog"), "{v}");
    assert!(board::find_task(&b.app, Some(fix)).unwrap().is_none(), "it had nothing else to fix");
    let i = board::get_issue(&b.app, i.id()).unwrap();
    assert_eq!((i.st("blocks").as_str(), i.i("holds_pr"), i.st("state").as_str(), i.i("task_id")), ("none", None, "open", None));
    assert!(!i.b("asks_owner"));
    assert!(ask(&b, i.id()).is_none());
    assert!(samepr::hold(&b.app, &b.row(a)).unwrap().is_none());
    assert_eq!(b.row(a).st("pr_phase"), "merge");
}

#[test]
fn rejecting_one_of_two_items_keeps_the_fix_for_the_other() {
    let b = asking();
    let a = held(&b);
    let first = b.issue(&b.found("Tokens leak to the log", "src/auth.rs logs the bearer token"));
    let second = b.issue(&b.found("Logout wipes saved drafts", "drafts.rs deletes every draft"));
    let fix = first.i("task_id").unwrap();
    assert_eq!(second.i("task_id"), Some(fix));

    answer(&b, second.id(), false);
    let f = b.row(fix);
    assert!(!f.st("detail").contains(&format!("- B{} ", second.id())), "{}", f.st("detail"));
    assert!(f.st("detail").contains(&format!("- B{} ", first.id())), "{}", f.st("detail"));
    assert_eq!(f.st("status"), "planned", "the first item still waits");
    assert!(ask(&b, first.id()).is_some());
    assert!(ask(&b, second.id()).is_none());
    assert!(samepr::hold(&b.app, &b.row(a)).unwrap().unwrap().starts_with(&format!("B{}", first.id())));
}

#[test]
fn rejecting_the_last_waiting_item_lets_the_approved_rest_run() {
    let b = asking();
    held(&b);
    let first = b.issue(&b.found("Tokens leak to the log", "src/auth.rs logs the bearer token"));
    let second = b.issue(&b.found("Logout wipes saved drafts", "drafts.rs deletes every draft"));
    let fix = first.i("task_id").unwrap();
    b.app.db.x("UPDATE goals SET ran_at = ? WHERE id = ?", taskboardd::p![now_iso(), b.goal]).unwrap();

    let v = answer(&b, first.id(), true);
    assert!(v["text"].as_str().unwrap().contains("still waits"), "{v}");
    assert_eq!(b.row(fix).st("status"), "planned");
    answer(&b, second.id(), false);
    assert_eq!(b.row(fix).st("status"), "queued", "nothing in it waits for the owner now");
}

#[test]
fn rejecting_after_the_fix_started_takes_it_off_the_pr() {
    let b = asking();
    let a = held(&b);
    let i = b.issue(&b.found("Tokens leak to the log", "src/auth.rs logs the bearer token"));
    let fix = i.i("task_id").unwrap();
    board::update_task(&b.app, fix, vec![("status", json!("working")), ("started_at", json!(now_iso()))]).unwrap();

    let v = answer(&b, i.id(), false);
    assert!(v["text"].as_str().unwrap().contains("no longer shares the PR"), "{v}");
    let f = b.row(fix);
    assert!(f.s("same_pr").filter(|s| !s.is_empty()).is_none(), "{:?}", f.s("same_pr"));
    assert!(samepr::hold(&b.app, &b.row(a)).unwrap().is_none());
}

#[test]
fn auto_mode_asks_nothing() {
    let b = new_board_with(|_| {});
    held(&b);
    b.app.db.x("UPDATE goals SET ran_at = ? WHERE id = ?", taskboardd::p![now_iso(), b.goal]).unwrap();
    let i = b.issue(&b.found("Tokens leak to the log", "src/auth.rs logs the bearer token"));
    let fix = i.i("task_id").unwrap();
    assert!(!i.b("asks_owner"));
    assert!(ask(&b, i.id()).is_none());
    assert_eq!(b.row(fix).st("status"), "queued", "the running goal picks it up at once");
    assert!(!samepr::awaits_owner(&b.app, fix).unwrap());

    let e = api::dispatch(&b.app, "POST", &format!("/backlog/{}/critical", i.id()), &Query::new(), &json!({"approve": true})).unwrap_err();
    assert_eq!(e.status, 409);
    assert!(e.message.contains("isn't waiting for approval"), "{}", e.message);
}

#[test]
fn answering_needs_a_yes_or_no() {
    let b = asking();
    held(&b);
    let i = b.issue(&b.found("Tokens leak to the log", "src/auth.rs logs the bearer token"));
    let e = api::dispatch(&b.app, "POST", &format!("/backlog/{}/critical", i.id()), &Query::new(), &json!({})).unwrap_err();
    assert_eq!(e.status, 400);
    assert!(ask(&b, i.id()).is_some());
}
