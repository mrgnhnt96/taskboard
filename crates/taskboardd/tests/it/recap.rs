//! Weekly and monthly recaps: their periods, the facts kept for each, and the routes the app reads.

use chrono::NaiveDate;
use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::iso;
use taskboardd::{days, fields, recap};

fn new_board() -> (tempfile::TempDir, std::sync::Arc<App>) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.first_weekday = chrono::Weekday::Sun;
    let app = App::for_tests(cfg);
    let projects: Vec<Value> = ["webapp", "api"]
        .iter()
        .map(|n| {
            let repo = dir.path().join(n);
            std::fs::create_dir_all(&repo).unwrap();
            json!({"name": n, "path": repo.to_string_lossy()})
        })
        .collect();
    app.db.set_setting("midna_projects", Some(&Value::Array(projects).to_string())).unwrap();
    (dir, app)
}

fn call(app: &App, method: &str, path: &str) -> Result<Value, (u16, String)> {
    api::dispatch(app, method, path, &Query::new(), &json!({})).map_err(|e| (e.status, e.message))
}

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// Local `h:m` on `d`, stored as UTC.
fn at(d: NaiveDate, h: u32, m: u32) -> String {
    iso(days::start_of(d) + (h * 3600 + m * 60) as f64)
}

fn goal(app: &App, project: &str, name: &str) -> i64 {
    api::dispatch(app, "POST", "/goals", &Query::new(), &json!({"name": name, "project": project})).unwrap()["id"].as_i64().unwrap()
}

fn task(app: &App, g: Option<i64>, project: &str, title: &str) -> i64 {
    let mut body = json!({"title": title, "detail": "Do it.", "project": project});
    if let Some(g) = g {
        body["goal_id"] = json!(g);
    }
    api::dispatch(app, "POST", "/tasks", &Query::new(), &body).unwrap()["id"].as_i64().unwrap()
}

fn finish(app: &App, t: i64, when: &str, extra: Vec<(&str, Value)>) {
    let mut f = fields!["status" => "done", "finished_at" => when, "summary" => format!("Summary of {t}.")];
    f.extend(extra);
    app.db.update("tasks", &json!(t), f).unwrap();
}

fn state(app: &App, t: i64, when: &str, status: &str, reason: Option<&str>) {
    app.db.insert("task_states", fields!["task_id" => t, "at" => when, "status" => status, "needs_reason" => reason, "failed" => 0]).unwrap();
}

#[test]
fn a_week_runs_monday_to_monday_whatever_first_weekday_says_and_a_month_is_the_calendar_month() {
    assert_eq!(recap::period("weekly", day(2026, 10, 8)).unwrap(), (day(2026, 10, 5), day(2026, 10, 12)), "a Thursday");
    assert_eq!(recap::period("weekly", day(2026, 10, 5)).unwrap(), (day(2026, 10, 5), day(2026, 10, 12)), "the Monday itself");
    assert_eq!(recap::period("weekly", day(2026, 10, 11)).unwrap(), (day(2026, 10, 5), day(2026, 10, 12)), "a Sunday ends the week");
    assert_eq!(recap::period("monthly", day(2026, 12, 15)).unwrap(), (day(2026, 12, 1), day(2027, 1, 1)));
    assert_eq!(recap::previous("monthly", day(2026, 3, 1)).unwrap(), (day(2026, 2, 1), day(2026, 3, 1)));
    assert_eq!(recap::last_complete("weekly", day(2026, 10, 12)).unwrap(), (day(2026, 10, 5), day(2026, 10, 12)), "on Monday, the week before");
    assert_eq!(recap::last_complete("monthly", day(2026, 11, 14)).unwrap(), (day(2026, 10, 1), day(2026, 11, 1)));
    assert_eq!(recap::period("daily", day(2026, 10, 5)).unwrap_err().status, 400);

    let (_dir, app) = new_board();
    let f = recap::facts(&app, "weekly", day(2026, 10, 8)).unwrap();
    assert_eq!(f["period"]["first_day"], "2026-10-05", "first_weekday is Sunday on this board");
    assert_eq!(f["period"]["last_day"], "2026-10-11");
    assert_eq!(f["period"]["start"], json!(iso(days::start_of(day(2026, 10, 5)))), "local midnight, stored as UTC");
    assert_eq!(f["days"].as_array().unwrap().len(), 7);
}

/// The week of October 5, 2026 in webapp: G1's T merged (a PR, an hour worked, an hour estimated,
/// in a release), T no PR, T closed PR, T PR still open, T failed, the owner's own task, and a task
/// the week before.
#[test]
fn a_week_reports_merged_released_and_human_work_apart_and_leaves_out_closed_prs() {
    let (_dir, app) = new_board();
    let wed = day(2026, 10, 7);
    let g = goal(&app, "webapp", "Ship login");
    let merged = task(&app, Some(g), "webapp", "Login form");
    let no_pr = task(&app, Some(g), "webapp", "Write the copy");
    let closed = task(&app, Some(g), "webapp", "Try another way");
    let open = task(&app, Some(g), "webapp", "Session cookie");
    let failed = task(&app, Some(g), "webapp", "Broken one");
    let mine = task(&app, Some(g), "webapp", "Make an account");
    let before = task(&app, None, "webapp", "Last week's");
    finish(&app, merged, &at(wed, 11, 0), vec![("pr_num", json!(5)), ("pr_state", json!("MERGED")), ("human_min", json!(60))]);
    finish(&app, no_pr, &at(wed, 12, 0), vec![]);
    finish(&app, closed, &at(wed, 13, 0), vec![("pr_num", json!(6)), ("pr_state", json!("CLOSED"))]);
    finish(&app, open, &at(wed, 14, 0), vec![("pr_num", json!(7)), ("pr_state", json!("OPEN"))]);
    finish(&app, failed, &at(wed, 15, 0), vec![("failed", json!(1))]);
    finish(&app, mine, &at(wed, 16, 0), vec![("kind", json!("human"))]);
    finish(&app, before, &at(day(2026, 9, 30), 9, 0), vec![]);

    state(&app, merged, &at(wed, 10, 0), "working", None);
    state(&app, merged, &at(wed, 11, 0), "done", None);
    state(&app, no_pr, &at(wed, 10, 30), "working", None);
    state(&app, no_pr, &at(wed, 11, 30), "done", None);
    state(&app, mine, &at(wed, 9, 0), "needs", Some("human"));
    state(&app, mine, &at(wed, 9, 30), "done", None);

    app.db.insert("pr_merges", fields!["task_id" => merged, "pr_num" => 5, "project" => "webapp", "merged_at" => at(wed, 11, 5), "added" => 10, "removed" => 2]).unwrap();
    app.db.insert("events", fields!["task_id" => merged, "at" => at(wed, 10, 40), "who" => "agent", "kind" => "commit", "text" => "Committed abc123: login form"]).unwrap();
    let rel = app.db.insert("releases", fields!["project" => "webapp", "name" => "v1.0.0", "cut_at" => at(wed, 17, 0)]).unwrap();
    app.db.update("tasks", &json!(merged), fields!["release_id" => rel]).unwrap();
    for (i, (tool, secs)) in [("Bash", 300.0), ("Bash", 300.0), ("Agent", 5000.0)].iter().enumerate() {
        app.db
            .insert("spans", fields!["tool_use_id" => format!("tu{i}"), "task_id" => merged, "project" => "webapp", "tool" => *tool, "category" => "build", "started_at" => at(wed, 10, 5 + i as u32), "seconds" => *secs])
            .unwrap();
    }

    let f = recap::facts(&app, "weekly", wed).unwrap();
    let t = &f["totals"];
    assert_eq!(t["tasks"], 4, "merged, no PR, PR open and the owner's own; the closed PR and the failed task are left out");
    assert_eq!((t["merged"].clone(), t["merged_pr"].clone(), t["no_pr"].clone()), (json!(2), json!(1), json!(1)));
    assert_eq!((t["pr_open"].clone(), t["closed_pr"].clone(), t["human_done"].clone()), (json!(1), json!(1), json!(1)));
    assert_eq!((t["prs"].clone(), t["added"].clone(), t["removed"].clone()), (json!(1), json!(10), json!(2)));
    assert_eq!((t["commits"].clone(), t["releases"].clone()), (json!(1), json!(1)));
    assert_eq!(t["agent_min"], 120.0, "two tasks working an hour each, side by side, both count");
    assert_eq!(t["busy_min"], 90.0, "the clock time with at least one working");
    assert_eq!(t["command_min"], 10.0, "a subagent's wrapper span isn't counted again");
    assert_eq!((t["est_human_min"].clone(), t["est_agent_min"].clone()), (json!(60), json!(60.0)));

    let b = &f["before"];
    assert_eq!((b["tasks"].clone(), b["first_day"].clone()), (json!(1), json!("2026-09-28")));

    let lists = &f["tasks"]["by_goal"][0];
    assert_eq!(lists["name"], "Ship login");
    let shipped: Vec<(String, String, bool)> = lists["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x["task"].as_str().unwrap().to_string(), x["shipped"].as_str().unwrap().to_string(), x["human"].as_bool().unwrap()))
        .collect();
    assert_eq!(
        shipped,
        vec![
            (format!("T{merged}"), "merged".into(), false),
            (format!("T{no_pr}"), "no_pr".into(), false),
            (format!("T{open}"), "open".into(), false),
            (format!("H{mine}"), "human".into(), true),
        ]
    );
    assert_eq!(lists["tasks"][0]["summary"], format!("Summary of {merged}."));

    let r = &f["releases"][0];
    assert_eq!((r["name"].clone(), r["first"].clone(), r["notes"].clone()), (json!("v1.0.0"), json!(true), Value::Null));
    assert_eq!(r["tasks"][0]["task"], format!("T{merged}"), "a release is built from its tasks");
    assert_eq!((r["count"].clone(), r["more"].clone()), (json!(1), json!(0)));

    assert_eq!(f["agent"]["projects"][0]["name"], "webapp");
    assert_eq!(f["agent"]["projects"][0]["agent_min"], 120.0);
    assert_eq!(f["agent"]["categories"][0], json!({"name": "build", "minutes": 10.0}));

    let w = &f["waiting"];
    assert_eq!(w["minutes"], 30.0);
    assert_eq!((w["tasks"][0]["task"].clone(), w["tasks"][0]["human"].clone()), (json!(format!("H{mine}")), json!(true)));
    assert_eq!(f["usage"], Value::Null, "no usage samples were taken");

    let wed_row = f["days"].as_array().unwrap().iter().find(|d| d["date"] == "2026-10-07").unwrap();
    assert_eq!((wed_row["tasks"].clone(), wed_row["merged"].clone(), wed_row["agent_min"].clone()), (json!(4), json!(2), json!(120.0)));
}

#[test]
fn loose_tasks_are_capped_per_project_and_before_is_left_out_with_nothing_to_compare() {
    let (_dir, app) = new_board();
    let wed = day(2026, 10, 7);
    for i in 0..10 {
        let t = task(&app, None, "api", &format!("Loose {i}"));
        finish(&app, t, &at(wed, 9, i), vec![]);
    }
    app.db.insert("usage_samples", fields!["at" => at(wed, 9, 0), "window" => "five_hour", "pct" => 40.0]).unwrap();
    app.db.insert("usage_samples", fields!["at" => at(wed, 10, 0), "window" => "five_hour", "pct" => 80.0]).unwrap();
    let f = recap::facts(&app, "weekly", wed).unwrap();
    let loose = &f["tasks"]["loose"][0];
    assert_eq!((loose["project"].clone(), loose["count"].clone(), loose["more"].clone()), (json!("api"), json!(10), json!(2)));
    assert_eq!(loose["tasks"].as_array().unwrap().len(), 8);
    assert_eq!(f["before"], Value::Null, "the week before had no work");
    let u = &f["usage"]["five_hour"];
    assert_eq!((u["peak"].clone(), u["avg"].clone(), u["samples"].clone()), (json!(80.0), json!(60.0), json!(2)));
    assert!(f.get("weeks").is_none(), "week-by-week trends are monthly only");
}

#[test]
fn a_month_adds_weekly_trends_plan_quality_and_lessons_kept() {
    let (_dir, app) = new_board();
    let g = goal(&app, "webapp", "Ship login");
    let t = task(&app, Some(g), "webapp", "Login form");
    finish(&app, t, &at(day(2026, 10, 14), 10, 0), vec![]);
    app.db.insert("lessons", fields!["kind" => "code", "text" => "Check the schema first.", "state" => "kept", "decided_at" => at(day(2026, 10, 20), 9, 0)]).unwrap();
    app.db.insert("lessons", fields!["kind" => "code", "text" => "Still a suggestion.", "state" => "suggested", "decided_at" => at(day(2026, 10, 20), 9, 0)]).unwrap();

    let f = recap::facts(&app, "monthly", day(2026, 10, 1)).unwrap();
    assert_eq!((f["period"]["first_day"].clone(), f["period"]["last_day"].clone()), (json!("2026-10-01"), json!("2026-10-31")));
    let weeks = f["weeks"].as_array().unwrap();
    let spans: Vec<(&str, &str)> = weeks.iter().map(|w| (w["first_day"].as_str().unwrap(), w["last_day"].as_str().unwrap())).collect();
    assert_eq!(spans, vec![("2026-10-01", "2026-10-04"), ("2026-10-05", "2026-10-11"), ("2026-10-12", "2026-10-18"), ("2026-10-19", "2026-10-25"), ("2026-10-26", "2026-10-31")]);
    assert_eq!(weeks[2]["tasks"], 1);
    assert_eq!(f["goals"][0]["name"], "Ship login", "every task done and no PR open: finished");
    assert_eq!(f["totals"]["goals_finished"], 1);
    assert_eq!(f["quality"]["this"]["goals"], 1);
    assert_eq!(f["quality"]["before"], Value::Null);
    let lessons = f["lessons"].as_array().unwrap();
    assert_eq!(lessons.len(), 1);
    assert_eq!(lessons[0]["text"], "Check the schema first.");
}

#[test]
fn the_app_lists_reads_and_marks_recaps_seen() {
    let (_dir, app) = new_board();
    let t = task(&app, None, "webapp", "Login form");
    finish(&app, t, &at(day(2026, 10, 7), 10, 0), vec![]);
    let id = recap::save(&app, "weekly", day(2026, 10, 7)).unwrap();
    assert_eq!(recap::save(&app, "weekly", day(2026, 10, 9)).unwrap(), id, "a week keeps the recap it has");
    let month = recap::save(&app, "monthly", day(2026, 10, 7)).unwrap();
    assert_ne!(month, id);

    let list = call(&app, "GET", "/recaps").unwrap();
    let cards = list["recaps"].as_array().unwrap();
    assert_eq!(cards.len(), 2);
    assert_eq!((cards[0]["id"].clone(), cards[0]["kind"].clone(), cards[0]["tasks"].clone()), (json!(id), json!("weekly"), json!(1)), "the latest period first");
    assert_eq!((cards[0]["writing_status"].clone(), cards[0]["seen_at"].clone()), (json!("none"), Value::Null));

    let one = call(&app, "GET", &format!("/recaps/{id}")).unwrap();
    assert_eq!(one["facts"]["totals"]["tasks"], 1);
    assert_eq!(one["writing"], Value::Null);

    let seen = call(&app, "POST", &format!("/recaps/{id}/seen")).unwrap();
    let at_first = seen["seen_at"].clone();
    assert!(at_first.is_string());
    assert_eq!(call(&app, "POST", &format!("/recaps/{id}/seen")).unwrap()["seen_at"], at_first, "the first time it was opened stays");

    assert_eq!(call(&app, "GET", "/recaps/999").unwrap_err().0, 404);
    assert_eq!(call(&app, "GET", "/recaps/nope").unwrap_err().0, 404);
}
