//! The weekly time review: the report built from spans and task states, its schedule, and the Needs you
//! item that stays until the review is opened.

use chrono::{Local, NaiveDate, TimeZone};
use serde_json::{json, Value};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{iso, now_ts};
use taskboardd::{api, digest, dispatch, review};

fn board() -> (std::sync::Arc<App>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.alerts.digest_mins = 30;
    (App::for_tests(cfg), dir)
}

fn get(app: &App, path: &str) -> Value {
    api::dispatch(app, "GET", path, &Default::default(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
}

fn post(app: &App, path: &str, body: Value) -> Result<Value, (u16, String)> {
    api::dispatch(app, "POST", path, &Default::default(), &body).map_err(|e| (e.status, e.message))
}

struct S<'a> {
    tool: &'a str,
    category: &'a str,
    fingerprint: &'a str,
    seconds: f64,
    exit: Option<i64>,
    timed_out: bool,
    refused: bool,
}

fn bash<'a>(category: &'a str, fingerprint: &'a str, seconds: f64, exit: i64) -> S<'a> {
    S { tool: "Bash", category, fingerprint, seconds, exit: Some(exit), timed_out: false, refused: false }
}

fn add_spans(app: &App, from: f64, spans: &[S]) {
    for (i, s) in spans.iter().enumerate() {
        app.db
            .x(
                "INSERT INTO spans(tool_use_id, claude_id, task_id, project, tool, category, fingerprint, command, started_at, seconds, exit_code, timed_out, refused)
                 VALUES(?, 'c1', 7, 'webapp', ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                vec![
                    json!(format!("tu{from}-{i}")), json!(s.tool), json!(s.category), json!(s.fingerprint), json!(s.fingerprint),
                    json!(iso(from + i as f64 * 600.0)), json!(s.seconds), json!(s.exit), json!(s.timed_out as i64), json!(s.refused as i64),
                ],
            )
            .unwrap();
    }
}

fn add_state(app: &App, task: i64, at: f64, status: &str, reason: &str) {
    app.db
        .x("INSERT INTO task_states(task_id, at, status, needs_reason, failed, project) VALUES(?, ?, ?, ?, 0, 'webapp')", vec![json!(task), json!(iso(at)), json!(status), json!(reason)])
        .unwrap();
}

fn week_of_spans(app: &App, from: f64) {
    add_spans(
        app,
        from,
        &[
            bash("build", "cargo build", 100.0, 0),
            bash("build", "cargo build", 300.0, 0),
            bash("build", "cargo build", 200.0, 0),
            bash("test", "cargo test", 60.0, 101),
            S { timed_out: true, exit: None, ..bash("test", "cargo test", 120.0, 0) },
            bash("test", "cargo test", 30.0, 0),
            S { tool: "Agent", ..bash("subagent", "explore", 1000.0, 0) },
            S { refused: true, ..bash("build", "cargo build", 900.0, 0) },
        ],
    );
}

#[test]
fn report_counts_categories_repeats_timeouts_reruns_and_owner_wait() {
    let (app, _dir) = board();
    let t0 = now_ts() - 2.0 * 86400.0;
    week_of_spans(&app, t0);
    add_state(&app, 7, t0, "needs", "question");
    add_state(&app, 7, t0 + 3600.0, "working", "");
    add_state(&app, 8, t0, "needs", "lost");

    let now = get(&app, "reviews/now");
    assert!(now["period_start"].is_string() && now["period_end"].is_string(), "{now}");
    let r = now["data"].clone();
    assert_eq!(r["seconds"], 810.0, "the subagent wrapper and refused calls aren't counted");
    assert_eq!(r["categories"], json!([{"name": "build", "seconds": 600.0}, {"name": "test", "seconds": 210.0}]));
    assert_eq!(r["projects"][0]["name"], "webapp");
    assert_eq!(r["slowest_tasks"][0]["task"], "T7");
    assert_eq!(r["slowest_tasks"][0]["top_category"], "build");

    let build = r["repeated"].as_array().unwrap().iter().find(|c| c["fingerprint"] == "cargo build").unwrap().clone();
    assert_eq!((build["runs"].as_i64(), build["median"].as_f64(), build["p90"].as_f64()), (Some(3), Some(200.0), Some(300.0)));

    assert_eq!(r["timeouts"]["count"], 1);
    assert_eq!(r["timeouts"]["commands"][0]["fingerprint"], "cargo test");
    assert_eq!(r["reruns"], json!([{"command": "cargo test", "fingerprint": "cargo test", "task": "T7", "runs": 3, "seconds": 210.0, "passed": true}]));

    assert_eq!(r["owner_wait"]["seconds"], 3600.0, "a lost terminal isn't time waiting on the owner");
    assert_eq!(r["owner_wait"]["reasons"], json!([{"name": "question", "seconds": 3600.0}]));
    assert!(r["change"].is_null(), "no earlier review to compare with");

    let spans = get(&app, "tasks/T7/spans");
    assert_eq!(spans["spans"].as_array().unwrap().len(), 8);
    assert!(get(&app, "reviews")["reviews"].as_array().unwrap().is_empty(), "reviews/now keeps nothing");
}

#[test]
fn owner_wait_counts_work_hours_once_however_many_tasks_wait() {
    let (app, _dir) = board();
    taskboardd::hours::set(&app, &json!({"on": true, "start": "09:00", "end": "17:00", "days": "all"})).unwrap();
    let day = Local::now().date_naive() - chrono::Duration::days(3);
    let at = |h: u32| Local.from_local_datetime(&day.and_hms_opt(h, 0, 0).unwrap()).earliest().unwrap().timestamp() as f64;
    // Both wait from 8 PM the night before until 11 AM: only 9 to 11 is in the work hours.
    add_state(&app, 7, at(0) - 4.0 * 3600.0, "needs", "question");
    add_state(&app, 8, at(0) - 4.0 * 3600.0, "needs", "review");
    add_state(&app, 7, at(11), "working", "");
    add_state(&app, 8, at(11), "working", "");

    let r = review::report(&app, at(0) - 86400.0, at(0) + 86400.0).unwrap();
    assert_eq!(r["owner_wait"]["seconds"], 7200.0, "9 to 11 once, not overnight and not once per task");
    assert_eq!(r["owner_wait"]["reasons"], json!([{"name": "question", "seconds": 7200.0}, {"name": "review", "seconds": 7200.0}]));
    assert_eq!(r["owner_wait"]["tasks"].as_array().unwrap().len(), 2);
}

#[test]
fn a_kept_review_stays_in_needs_you_until_it_is_opened() {
    let (app, _dir) = board();
    week_of_spans(&app, now_ts() - 86400.0);

    let r = post(&app, "reviews", json!({})).unwrap();
    let id = r["id"].as_i64().unwrap();
    assert_eq!(r["headline"], "Weekly review: 0.2 h on builds, 0 fixes suggested");
    assert!(r["seen_at"].is_null());

    let alerts = get(&app, "state")["alerts"].as_array().unwrap().clone();
    let a = alerts.iter().find(|a| a["key"] == format!("review:{id}")).expect("the review is in Needs you").clone();
    assert_eq!(a["text"], r["headline"]);
    assert_eq!(a["url"], format!("taskboard://review/{id}"));
    assert!(digest::members(&app).iter().all(|m| m["id"] != a["id"]), "the review sends its own notification, not the digest");

    let (status, msg) = post(&app, &format!("alerts/{}/dismiss", a["id"].as_str().unwrap()), json!({})).unwrap_err();
    assert_eq!(status, 409, "{msg}");

    let seen = post(&app, &format!("reviews/{id}/seen"), json!({})).unwrap();
    assert!(seen["seen_at"].is_string());
    assert!(dispatch::alerts(&app).iter().all(|x| x["key"] != format!("review:{id}")), "opening it clears Needs you");
    assert_eq!(get(&app, "reviews")["reviews"][0]["id"], id);
    assert_eq!(get(&app, &format!("reviews/{id}"))["data"]["seconds"], 810.0);
    assert_eq!(post(&app, "reviews/999/seen", json!({})).unwrap_err().0, 404);
}

#[test]
fn the_schedule_keeps_one_review_per_week_and_compares_with_the_last() {
    let (app, _dir) = board();
    let local = |d: u32, h: u32| Local.from_local_datetime(&NaiveDate::from_ymd_opt(2026, 10, d).unwrap().and_hms_opt(h, 0, 0).unwrap()).unwrap();
    let ts = |d: u32, h: u32| local(d, h).timestamp() as f64;

    // Friday Oct 9, 5:00 PM: a week of work before it.
    week_of_spans(&app, ts(7, 10));
    let first = review::tick(&app, local(9, 18).naive_local()).unwrap().expect("the week's review is made");
    assert_eq!(review::tick(&app, local(10, 9).naive_local()).unwrap(), None, "once per scheduled moment");

    // The next week has more builds and no reruns.
    add_spans(&app, ts(14, 10), &[bash("build", "cargo build", 1000.0, 0)]);
    let second = review::tick(&app, local(16, 17).naive_local()).unwrap().expect("the next week's review");
    let r = review::get(&app, second).unwrap();
    assert_eq!(r["scheduled"], true);
    assert_eq!(r["period_start"], iso(ts(9, 17)));
    let change = &r["data"]["change"];
    assert_eq!(change["review"], first);
    assert_eq!(change["seconds"], 190.0);
    assert_eq!(change["reruns"], -1);
    assert_eq!(change["timeouts"], -1);

    // A week with no agent work makes no review.
    assert_eq!(review::tick(&app, local(23, 17).naive_local()).unwrap(), None);
    assert_eq!(get(&app, "reviews")["reviews"].as_array().unwrap().len(), 2);
}

#[test]
fn a_board_seeded_before_the_sample_reviews_gets_just_those() {
    let (app, _dir) = board();
    taskboardd::seed::seed(&app).unwrap();
    app.db.x("DELETE FROM issues WHERE review_id IS NOT NULL", taskboardd::p![]).unwrap();
    app.db.x("DELETE FROM reviews", taskboardd::p![]).unwrap();
    let tasks = app.db.count("SELECT COUNT(*) FROM tasks", taskboardd::p![]).unwrap();
    taskboardd::seed::seed_reviews_only(&app).unwrap();
    assert_eq!(app.db.count("SELECT COUNT(*) FROM tasks", taskboardd::p![]).unwrap(), tasks);
    assert_eq!(get(&app, "/reviews")["reviews"].as_array().unwrap().len(), 2);
    assert!(taskboardd::seed::seed_reviews_only(&app).is_err(), "seeding them twice is refused");
}

#[test]
fn the_report_leads_with_what_the_week_shipped() {
    let (app, _d) = board();
    let start = now_ts() - 7.0 * 86400.0;
    let end = now_ts();
    let at = |days_ago: f64| iso(end - days_ago * 86400.0);
    let goal = |name: &str| app.db.insert("goals", taskboardd::fields!["name" => name, "project" => "webapp"]).unwrap();
    let task = |goal: Option<i64>, title: &str, status: &str, failed: bool, finished: Option<String>| {
        app.db
            .insert("tasks", taskboardd::fields!["goal_id" => goal, "title" => title, "status" => status, "failed" => failed as i64,
                                                 "project" => "webapp", "finished_at" => finished.clone(), "updated_at" => finished])
            .unwrap()
    };
    let done = goal("Offline mode");
    let a = task(Some(done), "Queue writes", "done", false, Some(at(3.0)));
    let b = task(Some(done), "Send them", "done", false, Some(at(1.0)));
    let open = goal("Dark mode");
    task(Some(open), "Colors", "done", false, Some(at(2.0)));
    task(Some(open), "Toggle", "queued", false, None);
    let earlier = goal("Search");
    task(Some(earlier), "Index", "done", false, Some(at(9.0)));
    task(None, "Broke", "done", true, Some(at(2.0)));
    let loose = task(None, "Typo", "done", false, Some(at(0.5)));
    let release = |name: &str, cut: String, failed: bool| {
        app.db.insert("releases", taskboardd::fields!["project" => "webapp", "name" => name, "cut_at" => cut,
                                                       "failed_at" => if failed { Some(at(0.1)) } else { None }]).unwrap()
    };
    release("v1.2.0", at(2.0), false);
    release("v1.2.1", at(1.0), true);
    release("v1.1.0", at(10.0), false);

    let s = &review::report(&app, start, end).unwrap()["shipped"];
    let refs = |k: &str| s[k].as_array().unwrap().iter().map(|v| v["ref"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(refs("tasks").len(), 4, "done in the week, not failed or earlier: {s}");
    assert_eq!(refs("tasks")[0], format!("T{loose}"), "newest first");
    assert!(refs("tasks").contains(&format!("T{a}")) && refs("tasks").contains(&format!("T{b}")));
    assert_eq!(refs("goals"), [format!("G{done}")], "only the goal that finished in the week");
    assert_eq!(s["goals"][0]["name"], "Offline mode");
    let cut: Vec<&str> = s["releases"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(cut, ["v1.2.0"], "a failed cut and an earlier one aren't shipped");

    let id = review::save(&app, start, end, false).unwrap();
    let kept = get(&app, &format!("/reviews/{id}"));
    assert_eq!(kept["data"]["shipped"], *s);
    app.db.x("UPDATE reviews SET data = json_remove(data, '$.shipped') WHERE id = ?", taskboardd::p![id]).unwrap();
    let old = get(&app, &format!("/reviews/{id}"));
    assert_eq!(old["data"]["shipped"], *s, "a review kept before this lists it too");
}
