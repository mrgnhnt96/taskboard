//! Plan quality per finished goal, lessons that keep or retire themselves after retrospectives, and
//! kept planning lessons written into a project's planning file once the owner accepts.

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, fields, retro};

fn new_board() -> (tempfile::TempDir, std::sync::Arc<App>) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
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

fn call(app: &App, method: &str, path: &str, body: Value) -> Value {
    api::dispatch(app, method, path, &Query::new(), &body).unwrap()
}

fn get(app: &App, path: &str, q: &[(&str, &str)]) -> Value {
    let q: Query = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    api::dispatch(app, "GET", path, &q, &json!({})).unwrap()
}

fn goal(app: &App, project: &str, name: &str) -> i64 {
    call(app, "POST", "/goals", json!({"name": name, "project": project}))["id"].as_i64().unwrap()
}

fn task(app: &App, g: i64, project: &str, title: &str) -> i64 {
    call(app, "POST", "/tasks", json!({"title": title, "detail": "Do it.", "project": project, "goal_id": g}))["id"].as_i64().unwrap()
}

fn finish(app: &App, t: i64, at: &str) {
    app.db.update("tasks", &json!(t), fields!["status" => "done", "finished_at" => at]).unwrap();
}

fn state(app: &App, t: i64, at: &str, status: &str, failed: bool) {
    app.db.insert("task_states", fields!["task_id" => t, "at" => at, "status" => status, "failed" => failed as i64]).unwrap();
}

/// G1 in webapp: T1 planned (started three times, two hours estimated, one hour worked, a failed
/// review step, a PR visit to fix its checks and a question; a passed step and its rebase and merge
/// visits found nothing to fix), T2 added after the work started.
fn finished_goal(app: &App) -> i64 {
    let g = goal(app, "webapp", "Ship login");
    app.db.update("goals", &json!(g), fields!["created_at" => "2026-09-30T08:00:00Z"]).unwrap();
    let t1 = task(app, g, "webapp", "Login form");
    app.db.update("tasks", &json!(t1), fields!["created_at" => "2026-09-30T09:00:00Z", "started_at" => "2026-10-01T09:00:00Z", "start_tries" => 3, "human_min" => 120]).unwrap();
    let t2 = task(app, g, "webapp", "Session cookie");
    app.db.update("tasks", &json!(t2), fields!["created_at" => "2026-10-02T09:00:00Z", "started_at" => "2026-10-02T09:30:00Z"]).unwrap();
    state(app, t1, "2026-10-01T09:00:00Z", "working", false);
    state(app, t1, "2026-10-01T10:00:00Z", "done", false);
    board::log_event(app, t1, "agent", "question", "Asked: Should the form remember the email?").unwrap();
    for text in [
        "Brought the conversation back for PR #7: fixing checks",
        "Picked up again after a restart",
        "Brought the conversation back for PR #7: rebasing",
        "Starting a fresh conversation on sonnet for PR #7: ready to merge",
        "Brought the conversation back for PR #7: ready to merge",
    ] {
        board::log_event(app, t1, "board", "handoff", text).unwrap();
    }
    for data in [json!({"name": "Review", "ok": false}), json!({"name": "Review", "ok": true})] {
        app.db.insert("events", fields!["task_id" => t1, "at" => "2026-10-01T10:00:00Z", "who" => "board", "kind" => "step", "data" => data.to_string()]).unwrap();
    }
    finish(app, t1, "2026-10-01T10:00:00Z");
    finish(app, t2, "2026-10-02T11:00:00Z");
    g
}

#[test]
fn a_finished_goal_has_plan_quality_numbers() {
    let (_d, app) = new_board();
    let g = finished_goal(&app);
    let open = goal(&app, "webapp", "Still going");
    task(&app, open, "webapp", "Not done");
    let v = get(&app, "/quality", &[("project", "webapp")]);
    let p = &v["projects"][0];
    assert_eq!(p["project"], "webapp");
    assert_eq!(p["goals"].as_array().unwrap().len(), 1, "an unfinished goal has no numbers yet: {v}");
    let gv = &p["goals"][0];
    assert_eq!(gv["goal"], format!("G{g}"));
    assert_eq!((gv["tasks"].as_i64(), gv["added_late"].as_i64(), gv["failed"].as_i64()), (Some(2), Some(1), Some(1)), "{gv}");
    assert_eq!((gv["review_rounds"].as_i64(), gv["questions"].as_i64()), (Some(2), Some(1)), "the checks fix and the failed step: {gv}");
    assert_eq!((gv["est_min"].as_f64(), gv["work_min"].as_f64()), (Some(120.0), Some(60.0)), "{gv}");
    let t = &p["total"];
    assert_eq!((t["late_share"].as_f64(), t["rounds_per_task"].as_f64(), t["actual_vs_estimate"].as_f64()), (Some(0.5), Some(1.0), Some(0.5)), "{t}");
    assert_eq!(p["months"][0]["month"], "2026-10");
    assert!(app.db.get_setting(&format!("quality:G{g}")).unwrap().is_some(), "kept so it outlasts the events");
    assert!(get(&app, "/quality", &[("project", "api")])["projects"].as_array().unwrap().is_empty());
}

#[test]
fn tasks_made_with_the_goal_or_planned_by_its_design_are_not_late() {
    let (_d, app) = new_board();
    let g = goal(&app, "webapp", "Ship billing");
    app.db.update("goals", &json!(g), fields!["created_at" => "2026-10-05T09:00:00Z"]).unwrap();
    let mk = |title: &str, made: &str, started: &str, done: &str| {
        let t = task(&app, g, "webapp", title);
        app.db.update("tasks", &json!(t), fields!["created_at" => made, "started_at" => started]).unwrap();
        finish(&app, t, done);
        t
    };
    mk("Planned", "2026-10-05T09:00:00Z", "2026-10-05T09:01:00Z", "2026-10-05T09:20:00Z");
    mk("Made with the goal", "2026-10-05T09:03:00Z", "2026-10-05T09:30:00Z", "2026-10-05T09:40:00Z");
    let design = mk("Design", "2026-10-05T09:00:00Z", "2026-10-05T09:10:00Z", "2026-10-05T10:00:00Z");
    app.db.update("tasks", &json!(design), fields!["kind" => "design"]).unwrap();
    mk("While designing", "2026-10-05T09:30:00Z", "2026-10-05T10:30:00Z", "2026-10-05T11:00:00Z");
    mk("Right after the design", "2026-10-05T10:20:00Z", "2026-10-05T10:40:00Z", "2026-10-05T11:00:00Z");
    let late = mk("After the build began", "2026-10-05T11:00:00Z", "2026-10-05T11:10:00Z", "2026-10-05T11:30:00Z");
    let v = get(&app, "/quality", &[("project", "webapp")]);
    assert_eq!(v["projects"][0]["goals"][0]["added_late"], 1, "{v}");
    let d = retro::dossier(&app, g).unwrap();
    let marked: Vec<&str> = d.lines().filter(|l| l.ends_with("added after the goal started")).collect();
    assert_eq!(marked.len(), 1, "{d}");
    assert!(marked[0].starts_with(&format!("T{late} ")), "{d}");
}

#[test]
fn numbers_kept_the_old_way_are_worked_out_again() {
    let (_d, app) = new_board();
    let g = finished_goal(&app);
    let key = format!("quality:G{g}");
    let old = json!({"goal": format!("G{g}"), "finished_at": "2026-10-02T11:00:00Z", "fingerprint": "2:2026-10-02T11:00:00Z", "tasks": 2, "added_late": 2, "review_rounds": 9});
    app.db.set_setting(&key, Some(&old.to_string())).unwrap();
    let gv = &get(&app, "/quality", &[("project", "webapp")])["projects"][0]["goals"][0];
    assert_eq!((gv["review_rounds"].as_i64(), gv["added_late"].as_i64()), (Some(2), Some(1)), "counted again from the events: {gv}");

    // Once the cleanup took the events, only what the tasks still show is counted again.
    app.db.set_setting(&key, Some(&old.to_string())).unwrap();
    app.db.changed("DELETE FROM events", taskboardd::p![]).unwrap();
    let gv = &get(&app, "/quality", &[("project", "webapp")])["projects"][0]["goals"][0];
    assert_eq!((gv["review_rounds"].as_i64(), gv["added_late"].as_i64()), (Some(9), Some(1)), "{gv}");
    assert!(gv["fingerprint"].as_str().unwrap().starts_with("v4:"), "{gv}");
}

#[test]
fn numbers_kept_by_an_older_version_stay_once_the_events_are_gone() {
    let (_d, app) = new_board();
    let g = finished_goal(&app);
    let key = format!("quality:G{g}");
    let old = json!({"goal": format!("G{g}"), "finished_at": "2026-10-02T11:00:00Z", "fingerprint": "v2:2:2026-10-02T11:00:00Z", "tasks": 2, "added_late": 2, "review_rounds": 9, "questions": 4});
    app.db.set_setting(&key, Some(&old.to_string())).unwrap();
    app.db.changed("DELETE FROM events", taskboardd::p![]).unwrap();
    let gv = &get(&app, "/quality", &[("project", "webapp")])["projects"][0]["goals"][0];
    assert_eq!((gv["review_rounds"].as_i64(), gv["questions"].as_i64(), gv["added_late"].as_i64()), (Some(9), Some(4), Some(1)), "{gv}");
    assert!(gv["fingerprint"].as_str().unwrap().starts_with("v4:"), "{gv}");
}

/// Only what a task asked the owner counts: usage limits, API errors, "Waiting for you" notices,
/// a lost network and PR visits that stopped early are logged as questions too, but aren't ones.
#[test]
fn questions_count_only_what_a_task_asked() {
    let (_d, app) = new_board();
    let g = goal(&app, "webapp", "Ship search");
    let t = task(&app, g, "webapp", "Search box");
    app.db.update("tasks", &json!(t), fields!["started_at" => "2026-10-01T09:00:00Z"]).unwrap();
    for text in [
        "Asked: Index titles too?",
        "Asked in its reply: Should the box keep the last search?",
        "Asked you in its terminal: Which font?",
        "Asked you on PR #12: Keep the old endpoint?",
        "Hit a usage limit (You've hit your limit · resets 3pm). Claude Code carries on by itself once it resets",
        "Waiting for you: Claude needs your permission to use Bash",
        "Stopped on PR #12 before it was finished: rebase conflict",
        "The Claude API failed (API Error: 500). Its turn stopped; type anything in the terminal to carry on",
        "It lost its network connection over 6 hours ago and Midna has stopped waiting; type anything in the terminal to carry on",
    ] {
        board::log_event(&app, t, "agent", "question", text).unwrap();
    }
    finish(&app, t, "2026-10-01T10:00:00Z");
    let gv = &get(&app, "/quality", &[("project", "webapp")])["projects"][0]["goals"][0];
    assert_eq!(gv["questions"].as_i64(), Some(4), "{gv}");
}

fn event(app: &App, t: Option<i64>, at: &str, kind: &str, text: &str) {
    app.db.insert("events", fields!["task_id" => t, "at" => at, "who" => "board", "kind" => kind, "text" => text]).unwrap();
}

/// PR visits after submit count as work: from the handoff that brought the task back to what ended
/// the visit, with time already counted (a working span, another visit) counted once. A visit nothing
/// ended, one logged without a task, and visits of a task with no estimate don't count.
#[test]
fn time_vs_estimate_counts_pr_visits_after_submit() {
    let (_d, app) = new_board();
    let g = goal(&app, "webapp", "Ship search");
    let t = task(&app, g, "webapp", "Search box");
    app.db.update("tasks", &json!(t), fields!["started_at" => "2026-10-01T09:00:00Z", "human_min" => 120]).unwrap();
    state(&app, t, "2026-10-01T09:00:00Z", "working", false);
    state(&app, t, "2026-10-01T10:00:00Z", "done", false);
    for (at, kind, text) in [
        ("2026-10-01T09:30:00Z", "handoff", "Brought the conversation back for PR #7: fixing checks"),
        ("2026-10-01T10:15:00Z", "midna", "It finished its turn on PR #7, so the board is closing T1"),
        ("2026-10-01T11:00:00Z", "handoff", "Brought the conversation back for PR #7: fixing checks"),
        ("2026-10-01T11:01:00Z", "status", "Picked up PR #7: fixing checks"),
        ("2026-10-01T11:30:00Z", "status", "The agent finished its visit to PR #7; the board watches it again"),
        ("2026-10-01T11:59:00Z", "handoff", "Starting a fresh conversation on sonnet for PR #7: ready to merge"),
        ("2026-10-01T12:00:00Z", "handoff", "Brought the conversation back for PR #7: ready to merge"),
        ("2026-10-01T12:20:00Z", "status", "Merged PR #7"),
        ("2026-10-01T12:21:00Z", "midna", "Gave back worktree slot-2"),
        ("2026-10-01T13:00:00Z", "handoff", "Brought the conversation back for PR #7: rebasing"),
    ] {
        event(&app, Some(t), at, kind, text);
    }
    event(&app, None, "2026-10-01T14:00:00Z", "handoff", "Brought the conversation back for PR #7: fixing checks");
    event(&app, None, "2026-10-01T15:00:00Z", "status", "Merged PR #7");
    let u = task(&app, g, "webapp", "Search results");
    state(&app, u, "2026-10-01T09:00:00Z", "working", false);
    state(&app, u, "2026-10-01T10:00:00Z", "done", false);
    event(&app, Some(u), "2026-10-01T11:00:00Z", "handoff", "Brought the conversation back for PR #8: fixing checks");
    event(&app, Some(u), "2026-10-01T12:00:00Z", "status", "Merged PR #8");
    finish(&app, t, "2026-10-01T12:20:00Z");
    finish(&app, u, "2026-10-01T12:00:00Z");
    let gv = &get(&app, "/quality", &[("project", "webapp")])["projects"][0]["goals"][0];
    assert_eq!((gv["est_min"].as_f64(), gv["work_min"].as_f64()), (Some(120.0), Some(125.0)), "60 working, 15 past it, 30 fixing, 20 merging: {gv}");
}

#[test]
fn the_days_page_compares_the_last_30_days_with_before() {
    let (_d, app) = new_board();
    finished_goal(&app);
    let v = get(&app, "/days", &[("date", "2026-10-14")]);
    let p = v["planning"].as_array().unwrap();
    assert_eq!(p.len(), 1, "{}", v["planning"]);
    assert_eq!((p[0]["project"].as_str(), p[0]["recent"]["goals"].as_i64()), (Some("webapp"), Some(1)));
    assert!(p[0]["before"].is_null());
    let earlier = get(&app, "/days", &[("date", "2026-09-20")]);
    assert!(earlier["planning"].as_array().unwrap().is_empty(), "nothing finished in those 30 days");
}

fn retro_ran(app: &App, g: i64, at: &str) {
    app.db.set_setting(&format!("retro:G{g}"), Some(&json!({"state": "done", "at": at}).to_string())).unwrap();
}

#[test]
fn a_lesson_confirmed_twice_is_kept() {
    let (_d, app) = new_board();
    let g = goal(&app, "webapp", "Ship login");
    let l = call(&app, "POST", "/lessons", json!({"text": "Plan the cookie with the form.", "kind": "planning", "project": "webapp", "evidence": ["G9"]}));
    let id = l["id"].as_i64().unwrap();
    call(&app, "POST", &format!("/lessons/L{id}/confirm"), json!({}));
    let v = retro::apply(&app, g, &json!({"lessons": [{"action": "confirm", "lesson": id, "kind": "planning", "scope": "project", "text": "x", "evidence": []}]})).unwrap();
    assert_eq!(v["kept"], json!([format!("L{id}")]), "{v}");
    let l = call(&app, "GET", &format!("/lessons/L{id}"), json!({}));
    assert_eq!(l["state"], "kept");
    assert!(l["decided_by"].as_str().unwrap().starts_with("board:"), "{l}");
}

#[test]
fn a_lesson_no_goal_confirms_retires() {
    let (_d, app) = new_board();
    let keep = call(&app, "POST", "/lessons", json!({"text": "Split API and UI.", "kind": "planning", "project": "webapp", "kept": true}))["id"].as_i64().unwrap();
    let other = call(&app, "POST", "/lessons", json!({"text": "Pin the SDK.", "kind": "tooling", "project": "api", "kept": true}))["id"].as_i64().unwrap();
    for id in [keep, other] {
        app.db.update("lessons", &json!(id), fields!["created_at" => "2026-01-01T00:00:00Z", "decided_at" => "2026-01-01T00:00:00Z"]).unwrap();
    }
    for k in 0..3 {
        let g = goal(&app, "webapp", &format!("Goal {k}"));
        retro_ran(&app, g, &format!("2026-02-0{}T00:00:00Z", k + 1));
    }
    let g = goal(&app, "webapp", "Fourth");
    let v = retro::apply(&app, g, &json!({"lessons": []})).unwrap();
    assert_eq!(v["retired"], json!([]), "four goals aren't enough: {v}");
    retro_ran(&app, g, "2026-02-05T00:00:00Z");
    let g = goal(&app, "webapp", "Fifth");
    let v = retro::apply(&app, g, &json!({"lessons": []})).unwrap();
    assert_eq!(v["retired"], json!([format!("L{keep}")]), "only webapp's retrospectives count against webapp's lesson: {v}");
    assert_eq!(call(&app, "GET", &format!("/lessons/L{other}"), json!({}))["state"], "kept");
}

#[test]
fn a_project_lesson_confirmed_in_another_project_is_suggested_as_global() {
    let (_d, app) = new_board();
    let gw = goal(&app, "webapp", "Ship login");
    let id = call(&app, "POST", "/lessons", json!({"text": "Ask before adding a dependency.", "kind": "process", "project": "webapp", "kept": true, "evidence": [format!("G{gw}")]}))["id"]
        .as_i64()
        .unwrap();
    let ga = goal(&app, "api", "Rate limits");
    let v = retro::apply(&app, ga, &json!({"lessons": [{"action": "confirm", "lesson": id, "kind": "process", "scope": "project", "text": "x", "evidence": []}]})).unwrap();
    assert_eq!(v["confirmed"], json!([format!("L{id}")]), "another project's kept lesson can be confirmed: {v}");
    assert_eq!(v["suggest_global"], json!([format!("L{id}")]), "{v}");
    assert_eq!(call(&app, "GET", &format!("/lessons/L{id}"), json!({}))["suggest_global"], json!(["webapp", "api"]));
}

#[test]
fn kept_planning_lessons_are_suggested_for_the_planning_file_and_written_on_accept() {
    let (_d, app) = new_board();
    let id = call(&app, "POST", "/lessons", json!({"text": "Plan the cookie with the form.", "kind": "planning", "project": "webapp", "kept": true}))["id"].as_i64().unwrap();
    call(&app, "POST", "/lessons", json!({"text": "Use the test fixtures.", "kind": "code", "project": "webapp", "kept": true}));
    let v = get(&app, "/planning/lessons", &[("project", "webapp")]);
    assert_eq!(v["exists"], false);
    assert_eq!(v["add"], json!([format!("- Plan the cookie with the form. (L{id})")]), "planning lessons only: {v}");
    let text = v["text"].as_str().unwrap();
    assert!(text.contains("## Learned from finished goals"), "{text}");
    assert!(text.starts_with(get(&app, "/planning", &[("project", "webapp")])["text"].as_str().unwrap().trim_end()), "starts from the rules in force");
    assert!(get(&app, "/planning", &[("project", "webapp")])["lessons"].as_array().unwrap().len() == 1);

    let w = call(&app, "POST", "/planning/lessons", json!({"project": "webapp"}));
    assert_eq!(w["written"], true);
    let path = taskboardd::util::expand_home(w["path"].as_str().unwrap());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);

    let p = get(&app, "/planning", &[("project", "webapp")]);
    assert_eq!(p["from"], "project");
    assert!(p["lessons"].as_array().unwrap().is_empty(), "written into the file, not repeated: {p}");
    let again = get(&app, "/planning/lessons", &[("project", "webapp")]);
    assert_eq!((again["exists"].as_bool(), again["add"].as_array().unwrap().len()), (Some(true), 0));
    assert_eq!(call(&app, "POST", "/planning/lessons", json!({"project": "webapp"}))["written"], false);
}
