//! Needs you's other answers and Later choices: a to-do answered with something other than Done, an
//! item put off until next week or until a goal finishes, a finished stage put off, a miss the goal
//! finishes without, and what each item says it holds up.

use std::sync::Arc;

use chrono::{Datelike, Duration, Local};
use serde_json::{json, Value};
use taskboardd::api::{self, Query, FROM};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{parse_iso, Row, RowExt};
use taskboardd::{board, dispatch, hours, midna, p, reports, runner};

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
    fn post(&self, path: &str, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
    fn from_app(&self, path: &str, body: Value) -> Value {
        let q: Query = [(FROM.to_string(), "app".to_string())].into_iter().collect();
        api::dispatch(&self.app, "POST", path, &q, &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
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
        midna::sync(&self.app, &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    }
    fn task(&self, title: &str, extra: Value) -> i64 {
        let mut body = json!({"title": title, "detail": "Do it.", "project": "webapp", "pickup": "new"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", body).unwrap()["id"].as_i64().unwrap()
    }
    fn human(&self, sid: &str, extra: Value) -> String {
        let mut body = json!({"title": "Create an Oracle Cloud account", "detail": "1. Sign up.", "project": "webapp", "kind": "human"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.report("tb.new_task", sid, body)["created"][0].as_str().unwrap().to_string()
    }
    fn goal(&self, name: &str, extra: Value) -> i64 {
        let mut body = json!({"name": name, "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/goals", body).unwrap()["id"].as_i64().unwrap()
    }
    fn set(&self, id: i64, f: Vec<(&str, Value)>) {
        board::update_task(&self.app, id, f).unwrap();
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn starts(&self, id: i64) -> Vec<Row> {
        self.app.db.q("SELECT * FROM jobs WHERE kind = 'agent' AND purpose = 'start' AND task_id = ? ORDER BY id", p![id]).unwrap()
    }
    fn home(&self) -> Value {
        self.get("home")
    }
    fn answer(&self, h: &str, body: Value) -> Result<Value, (u16, String)> {
        self.post(&format!("tasks/{h}/owner-answer"), body)
    }
    /// A task parked on a to-do; the task and the to-do's ref.
    fn parked(&self) -> (i64, String) {
        let two = self.task("Deploy to Oracle Cloud", json!({}));
        self.session("s2");
        self.report("tb.take", "s2", json!({"task": format!("T{two}")}));
        let dir = self.app.cfg.claude_projects.join("-webapp");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("c-s2.jsonl"), "{\"type\":\"user\"}\n").unwrap();
        let h = self.human("s2", json!({}));
        assert_eq!(self.report("tb.wait_for", "s2", json!({"tasks": [h.clone()], "why": "Needs the account"}))["parked"], true);
        runner::start_queued(&self.app).unwrap();
        assert_eq!(self.row(id_of(&h)).st("status"), "needs", "the to-do is up for the owner");
        (two, h)
    }
    /// The task's item in `list` (`needs` or `later`) of the home page.
    fn item(&self, list: &str, task: &str) -> Option<Value> {
        self.home()[list].as_array().unwrap().iter().find(|i| i["task"] == task).cloned()
    }
}

fn id_of(r: &str) -> i64 {
    r[1..].parse().unwrap()
}

fn resume_prompt(b: &Board, id: i64) -> String {
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![id]);
    board::job_args(&b.starts(id)[0]).st("prompt")
}

fn days_from_now(n: i64) -> String {
    (Local::now().date_naive() + Duration::days(n)).format("%Y-%m-%d").to_string()
}

#[test]
fn cant_do_this_hands_it_back_and_the_waiting_agent_finds_another_way() {
    let b = new_board();
    let (two, h) = b.parked();
    let card = b.answer(&h, json!({"answer": "cant_do", "note": "Oracle won't take my card"})).unwrap();
    assert_eq!((&card["status"], &card["owner_answer"], &card["owner_note"]), (&json!("done"), &json!("cant_do"), &json!("Oracle won't take my card")));
    assert!(b.home()["needs"].as_array().unwrap().iter().all(|i| i["task"] != h.replace('H', "T")), "off the list");

    let prompt = resume_prompt(&b, two);
    assert!(prompt.contains("can't do it, so find another way"), "{prompt}");
    assert!(prompt.contains("Oracle won't take my card"), "{prompt}");
    assert!(!prompt.contains("is done"), "{prompt}");
}

#[test]
fn not_needed_cancels_it_and_says_so_to_what_waited() {
    let b = new_board();
    let (two, h) = b.parked();
    let card = b.answer(&h, json!({"answer": "not_needed"})).unwrap();
    assert_eq!((&card["status"], &card["failed"], &card["owner_answer"]), (&json!("done"), &json!(false), &json!("not_needed")));
    let prompt = resume_prompt(&b, two);
    assert!(prompt.contains("was cancelled"), "{prompt}");
    assert!(prompt.contains("Where that stands:"), "{prompt}");
}

#[test]
fn dont_wait_frees_what_waits_while_the_to_do_stays_listed() {
    let b = new_board();
    let (two, h) = b.parked();
    let t = format!("T{}", id_of(&h));
    assert_eq!(b.item("needs", &t).unwrap()["holds"]["tasks"], json!([format!("T{two}")]), "it holds the task that waits");

    let card = b.answer(&h, json!({"answer": "dont_wait", "note": "I'll get to it"})).unwrap();
    assert_eq!((&card["status"], &card["owner_answer"]), (&json!("needs"), &json!("dont_wait")));
    let prompt = resume_prompt(&b, two);
    assert!(prompt.contains("isn't done yet"), "{prompt}");
    assert!(prompt.contains("I'll get to it"), "{prompt}");

    let item = b.item("needs", &t).expect("still listed");
    assert_eq!(item["holds"], json!({"tasks": [], "goal": null}), "it holds nothing now");
    assert_eq!(b.answer(&h, json!({"answer": "dont_wait"})).unwrap_err().0, 409);

    let done = b.from_app(&format!("tasks/{h}/did"), json!({}));
    assert_eq!((&done["status"], &done["owner_answer"]), (&json!("done"), &Value::Null), "pressing Done later is plain Done");
}

#[test]
fn a_goal_goes_on_and_finishes_past_a_to_do_it_wont_wait_for() {
    let b = new_board();
    let g = b.goal("Move to Oracle", json!({"run_in_order": true, "max_terminals": 1}));
    b.session("s1");
    let gref = format!("G{g}");
    let h = b.human("s1", json!({"goal": gref}));
    let next = b.task("Deploy", json!({"goal_id": g}));
    b.from_app(&format!("goals/{gref}/run"), json!({}));
    runner::start_queued(&b.app).unwrap();
    assert!(b.starts(next).is_empty());
    let item = b.item("needs", &format!("T{}", id_of(&h))).unwrap();
    assert_eq!(item["holds"], json!({"tasks": [format!("T{next}")], "goal": gref}));

    b.answer(&h, json!({"answer": "dont_wait"})).unwrap();
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![next], "the goal's order lets it by");

    b.set(next, vec![("status", json!("done")), ("finished_at", json!("2026-11-06T10:00:00"))]);
    assert!(board::goal_finished(&b.app, g).unwrap());
    assert!(b.get(&format!("goals/{gref}"))["finished_at"].is_string());
}

#[test]
fn waiting_on_someone_leaves_the_list_until_the_date_and_keeps_what_waits_held() {
    let b = new_board();
    let (two, h) = b.parked();
    let t = format!("T{}", id_of(&h));
    let date = days_from_now(3);
    b.answer(&h, json!({"answer": "waiting", "until": date, "note": "Sam in finance"})).unwrap();

    assert!(b.item("needs", &t).is_none(), "off the list");
    let item = b.item("later", &t).expect("on the Later line");
    assert_eq!(item["later"]["waiting_on"], "Sam in finance");
    let back = parse_iso(item["later"]["until"].as_str().unwrap()).unwrap();
    let want = Local::now().date_naive() + Duration::days(3);
    assert_eq!(back, hours::day_start(&b.app, want));
    assert_eq!(b.home()["later_count"], 1);
    assert_eq!(b.home()["next_back"], item["later"]["until"]);
    assert_eq!(b.row(id_of(&h)).st("status"), "needs", "it stays open");
    assert!(runner::start_queued(&b.app).unwrap().is_empty(), "what waits keeps waiting");
    assert!(b.starts(two).is_empty());
}

#[test]
fn waiting_on_someone_also_puts_off_a_to_do_with_no_alert_up() {
    let b = new_board();
    let (_, h) = b.parked();
    let t = format!("T{}", id_of(&h));
    let id = b.item("needs", &t).unwrap()["id"].as_str().unwrap().to_string();
    b.post(&format!("alerts/{id}/dismiss"), json!({})).unwrap();
    b.answer(&h, json!({"answer": "waiting", "until": days_from_now(2)})).unwrap();
    assert!(b.item("needs", &t).is_none());
    assert_eq!(b.item("later", &t).unwrap()["later"]["waiting_on"], "");
}

#[test]
fn answers_are_refused_where_they_make_no_sense() {
    let b = new_board();
    let (two, h) = b.parked();
    assert_eq!(b.answer(&format!("T{two}"), json!({"answer": "cant_do"})).unwrap_err().0, 409, "not a to-do");
    assert_eq!(b.answer(&h, json!({"answer": "maybe"})).unwrap_err().0, 400);
    assert_eq!(b.answer(&h, json!({"answer": "waiting"})).unwrap_err().0, 400, "it needs a date");
    assert_eq!(b.answer(&h, json!({"answer": "waiting", "until": days_from_now(-1)})).unwrap_err().0, 400, "a date to come");
    b.answer(&h, json!({"answer": "not_needed"})).unwrap();
    assert_eq!(b.answer(&h, json!({"answer": "cant_do"})).unwrap_err().0, 409, "it's over");
}

#[test]
fn next_week_comes_back_on_monday_when_the_hours_open() {
    let b = new_board();
    let t = b.task("Sign-in", json!({}));
    b.set(t, vec![("status", json!("needs")), ("needs_reason", json!("question")), ("question", json!("Which provider?"))]);
    b.post(&format!("alerts/task:T{t}/snooze"), json!({"until": "next_week"})).unwrap();
    let until = b.item("later", &format!("T{t}")).unwrap()["later"]["until"].as_str().unwrap().to_string();
    let at = chrono::DateTime::from_timestamp(parse_iso(&until).unwrap() as i64, 0).unwrap().with_timezone(&Local);
    assert_eq!(at.weekday(), chrono::Weekday::Mon);
    let days = (at.date_naive() - Local::now().date_naive()).num_days();
    assert!((1..=7).contains(&days), "{days}");
    assert_eq!(parse_iso(&until).unwrap(), hours::next_week_start(&b.app));
}

#[test]
fn after_a_goal_comes_back_when_that_goal_finishes() {
    let b = new_board();
    let other = b.goal("Billing", json!({}));
    let last = b.task("Invoices", json!({"goal_id": other}));
    let t = b.task("Sign-in", json!({}));
    b.set(t, vec![("status", json!("needs")), ("needs_reason", json!("question")), ("question", json!("Which provider?"))]);
    dispatch::add_alert(&b.app, "An alert with no task", None, None, None, None).unwrap();
    let alerts = dispatch::alerts(&b.app);
    let alert = alerts.iter().find(|a| a["text"] == "An alert with no task").unwrap()["id"].as_str().unwrap();

    b.post(&format!("alerts/task:T{t}/snooze"), json!({"after_goal": format!("G{other}")})).unwrap();
    b.post(&format!("alerts/{alert}/snooze"), json!({"after_goal": other})).unwrap();
    let home = b.home();
    assert_eq!(home["later_count"], 2);
    assert!(home["next_back"].is_null(), "no date to say");
    assert_eq!(b.item("later", &format!("T{t}")).unwrap()["later"]["after_goal"], format!("G{other}"));
    dispatch::tick(&b.app).unwrap();
    assert_eq!(b.home()["later_count"], 2, "it stays put while the goal runs");

    b.set(last, vec![("status", json!("done"))]);
    dispatch::tick(&b.app).unwrap();
    let home = b.home();
    assert_eq!(home["later_count"], 0);
    let needs = home["needs"].as_array().unwrap();
    assert!(needs.iter().any(|i| i["task"] == format!("T{t}")), "{needs:?}");
    assert!(needs.iter().any(|i| i["id"] == alert), "{needs:?}");
}

#[test]
fn after_a_goal_needs_a_goal_that_can_finish_without_it() {
    let b = new_board();
    let g = b.goal("Sign-in", json!({}));
    let t = b.task("Provider", json!({"goal_id": g}));
    b.set(t, vec![("status", json!("needs")), ("needs_reason", json!("question")), ("question", json!("Which?"))]);
    let snooze = |goal: Value| b.post(&format!("alerts/task:T{t}/snooze"), json!({"after_goal": goal})).unwrap_err();
    let (code, why) = snooze(json!(format!("G{g}")));
    assert_eq!(code, 409);
    assert!(why.contains("never come back"), "{why}");
    assert_eq!(snooze(json!("G999")).0, 404);

    let done = b.goal("Finished", json!({}));
    let last = b.task("Last", json!({"goal_id": done}));
    b.set(last, vec![("status", json!("done"))]);
    assert_eq!(snooze(json!(format!("G{done}"))).0, 409, "it's over already");
}

/// A started goal with wave 1 done and stopped for review, and wave 2 queued.
fn stopped_goal(b: &Board) -> (i64, i64) {
    let g = b.goal("Login", json!({}));
    let one = b.task("Form", json!({"goal_id": g, "wave": 1}));
    let two = b.task("Logout", json!({"goal_id": g, "wave": 2}));
    b.from_app(&format!("goals/G{g}/waves/1"), json!({"stop_after": true}));
    b.from_app(&format!("goals/G{g}/run"), json!({}));
    b.set(one, vec![("status", json!("done"))]);
    (g, two)
}

fn entry(b: &Board, g: i64) -> Value {
    let goals = b.home()["goals"].clone();
    goals.as_array().unwrap().iter().find(|e| e["goal"]["id"] == g).cloned().expect("listed")
}

#[test]
fn a_finished_stage_can_be_put_off_but_not_dismissed() {
    let b = new_board();
    let (g, two) = stopped_goal(&b);
    let e = entry(&b, g);
    assert!(e["goal"]["stopped"].as_str().unwrap().contains("Review it"), "{e}");
    let id = e["stage"]["id"].as_str().unwrap().to_string();
    assert_eq!(id, format!("stage:G{g}:1"));
    assert_eq!(e["stage"]["holds"], json!({"tasks": [format!("T{two}")], "goal": format!("G{g}")}));

    assert_eq!(b.post(&format!("alerts/{id}/dismiss"), json!({})).unwrap_err().0, 409);
    assert_eq!(b.post(&format!("alerts/stage:G{g}:2/snooze"), json!({"mins": 60})).unwrap_err().0, 404, "not stopped there");
    assert_eq!(b.post(&format!("alerts/{id}/snooze"), json!({"after_goal": format!("G{g}")})).unwrap_err().0, 409, "its own goal");

    b.post(&format!("alerts/{id}/snooze"), json!({"mins": 60})).unwrap();
    let e = entry(&b, g);
    assert!(e["goal"]["stopped"].is_null() && e["stage"].is_null(), "{e}");
    let home = b.home();
    assert_eq!(home["later_count"], 1);
    let item = &home["later"][0];
    assert_eq!((&item["id"], &item["stage"], &item["wave"], &item["goal_name"]), (&json!(id), &json!(true), &json!(1), &json!("Login")));
    assert!(item["later"]["until"].is_string());

    let other = b.goal("Billing", json!({}));
    let last = b.task("Invoices", json!({"goal_id": other}));
    b.post(&format!("alerts/{id}/snooze"), json!({"after_goal": other})).unwrap();
    assert!(entry(&b, g)["stage"].is_null());
    b.set(last, vec![("status", json!("done"))]);
    dispatch::tick(&b.app).unwrap();
    assert_eq!(entry(&b, g)["stage"]["id"], json!(id), "back once that goal is over");
    assert_eq!(b.home()["later_count"], 0);
}

#[test]
fn a_put_off_stage_leaves_later_once_the_goal_moves_on() {
    let b = new_board();
    let (g, _) = stopped_goal(&b);
    b.post(&format!("alerts/stage:G{g}:1/snooze"), json!({"mins": 60})).unwrap();
    b.from_app(&format!("goals/G{g}/waves/1/continue"), json!({}));
    assert_eq!(b.home()["later_count"], 0);
}

#[test]
fn finish_the_goal_without_it_moves_the_miss_to_the_backlog_and_the_goal_finishes() {
    let b = new_board();
    b.session("s1");
    let g = b.goal("Login", json!({}));
    let t = b.task("Form", json!({"goal_id": g}));
    b.from_app(&format!("goals/G{g}/run"), json!({}));
    b.set(t, vec![("status", json!("working")), ("session_id", json!("s1"))]);
    let found = b.report("tb.found", "s1", json!({"title": "Sign-in fails on Android", "kind": "gap", "misses_goal": true}));
    let issue = found["issue"].as_str().unwrap().to_string();
    let miss = b.home()["needs"].as_array().unwrap().iter().find(|i| i["key"] == format!("miss:{}", id_of(&issue))).cloned().expect("listed");
    assert_eq!(miss["holds"], json!({"tasks": [], "goal": format!("G{g}")}));

    b.set(t, vec![("status", json!("done"))]);
    assert!(!board::goal_finished(&b.app, g).unwrap());

    let v = b.post(&format!("backlog/{issue}/without-goal"), json!({})).unwrap();
    assert_eq!((&v["misses_goal"], &v["state"]), (&json!(false), &json!("open")), "{v}");
    assert!(board::goal_finished(&b.app, g).unwrap());
    assert!(b.home()["needs"].as_array().unwrap().iter().all(|i| i["key"] != format!("miss:{}", id_of(&issue))), "its alert is gone");
    assert_eq!(b.post(&format!("backlog/{issue}/without-goal"), json!({})).unwrap_err().0, 409, "it holds nothing now");
}

#[test]
fn a_goal_says_when_its_set_aside() {
    let b = new_board();
    let g = b.goal("Login", json!({}));
    assert_eq!(b.post(&format!("goals/G{g}/set-aside"), json!({})).unwrap()["set_aside"], true);
    assert_eq!(b.get(&format!("goals/G{g}"))["deprioritized"], true);
    assert_eq!(b.post(&format!("goals/G{g}/bring-back"), json!({})).unwrap()["set_aside"], false);
}

#[test]
fn a_put_off_agent_question_still_counts_as_waiting() {
    let b = new_board();
    let (asked, to_do) = (b.task("Sign-in", json!({})), b.task("Lost one", json!({})));
    b.set(asked, vec![("status", json!("needs")), ("needs_reason", json!("question")), ("question", json!("Which provider?"))]);
    b.set(to_do, vec![("status", json!("needs")), ("needs_reason", json!("lost")), ("lost", json!(1))]);
    let home = b.home();
    assert_eq!(home["waiting"], 1, "a lost terminal isn't waiting on an answer");
    assert_eq!(b.item("needs", &format!("T{asked}")).unwrap()["waiting"], true);
    assert_eq!(b.item("needs", &format!("T{to_do}")).unwrap()["waiting"], false);

    b.post(&format!("alerts/task:T{asked}/snooze"), json!({"until": "tomorrow"})).unwrap();
    assert_eq!(b.home()["waiting"], 1);
    assert_eq!(b.item("later", &format!("T{asked}")).unwrap()["waiting"], true);

    let g = b.goal("Paused", json!({}));
    let held = b.task("Held", json!({"goal_id": g}));
    b.set(held, vec![("status", json!("needs")), ("needs_reason", json!("question")), ("question", json!("Which?"))]);
    assert_eq!(b.home()["waiting"], 2);
    b.post(&format!("goals/G{g}/set-aside"), json!({})).unwrap();
    assert_eq!(b.home()["waiting"], 1, "set aside, it doesn't count");
}

#[test]
fn a_wave_that_stops_holds_the_next_once_nothing_in_it_is_left_to_start() {
    let b = new_board();
    let g = b.goal("Login", json!({}));
    let (one, also, two) = (b.task("Form", json!({"goal_id": g, "wave": 1})), b.task("Cookie", json!({"goal_id": g, "wave": 1})), b.task("Logout", json!({"goal_id": g, "wave": 2})));
    b.from_app(&format!("goals/G{g}/waves/1"), json!({"stop_after": true}));
    b.from_app(&format!("goals/G{g}/run"), json!({}));
    b.set(one, vec![("status", json!("needs")), ("needs_reason", json!("question")), ("question", json!("Which?"))]);
    assert_eq!(b.item("needs", &format!("T{one}")).unwrap()["holds"]["tasks"], json!([]), "its wave mate could still start");
    b.set(also, vec![("status", json!("done"))]);
    assert_eq!(b.item("needs", &format!("T{one}")).unwrap()["holds"], json!({"tasks": [format!("T{two}")], "goal": format!("G{g}")}));
}
