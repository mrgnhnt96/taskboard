//! Agents estimate their working time up front (`tb task new --est`, `::est=` on planned items, `tb goal
//! new --est`); the board keeps the first estimate once work starts and compares it with the time spent in
//! the working status only.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{iso, now_ts, Row, RowExt};
use taskboardd::{board, estimate, fields, handoff, p, reports};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    Board { app, _dir: dir }
}

impl Board {
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn post(&self, path: &str, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap()
    }
    /// A task made the old way, with no estimate.
    fn bare_task(&self, goal: Option<i64>) -> i64 {
        let mut b = json!({"title": "Old work", "detail": "Do it.", "project": "webapp", "pickup": "manual"});
        if let Some(g) = goal {
            b["goal_id"] = json!(g);
        }
        self.post("/tasks", b).unwrap()["id"].as_i64().unwrap()
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn start(&self, id: i64) {
        board::update_task(&self.app, id, fields!["status" => "working", "started_at" => iso(now_ts())]).unwrap();
    }
    /// Replaces the task's status history with `(minutes from base, status)` steps.
    fn history(&self, id: i64, base: f64, steps: &[(f64, &str)]) {
        self.app.db.x("DELETE FROM task_states WHERE task_id = ?", p![id]).unwrap();
        for (m, status) in steps {
            self.app.db.insert("task_states", fields!["task_id" => id, "at" => iso(base + m * 60.0), "status" => *status, "failed" => 0]).unwrap();
        }
    }
}

fn id_of(v: &Value) -> i64 {
    v["created"][0].as_str().unwrap()[1..].parse().unwrap()
}

#[test]
fn agent_tasks_goals_and_planned_items_are_refused_without_an_estimate_and_say_what_to_add() {
    let b = new_board();
    let (code, why) = b.report("tb.new_task", json!({"title": "Fix the footer", "detail": "Do it."})).unwrap_err();
    assert_eq!(code, 400);
    assert!(why.contains("--est <len>") && why.contains("compares"), "{why}");
    for kind in ["design", "research"] {
        let (code, why) = b.report("tb.new_task", json!({"title": "Look into it", "detail": "Do it.", "kind": kind})).unwrap_err();
        assert_eq!(code, 400, "{kind}");
        assert!(why.contains("--est"), "{kind}: {why}");
    }
    let (code, why) = b.report("tb.new_task", json!({"title": "Fix the footer", "est": "soon"})).unwrap_err();
    assert_eq!(code, 400);
    assert!(why.contains("45m"), "{why}");

    let human = b.report("tb.new_task", json!({"title": "Sign the agreement", "detail": "1. Open the page.\n2. Sign it.", "kind": "human"})).unwrap();
    assert_eq!(b.row(id_of(&human)).i("est_min"), None, "a human task needs none");

    let t = id_of(&b.report("tb.new_task", json!({"title": "Fix the footer", "est": "1h30m"})).unwrap());
    assert_eq!(b.row(t).i("est_min"), Some(90));
    assert!(b.row(t).s("est_at").is_some());

    let (code, why) = b.report("tb.goal", json!({"name": "Dark mode", "tasks": ["Colors::pick them::est=1h"]})).unwrap_err();
    assert_eq!(code, 400);
    assert!(why.contains("running time") && why.contains("--est"), "{why}");

    let (code, why) = b.report("tb.goal", json!({"name": "Dark mode", "est": "3h", "tasks": ["Colors::pick them::est=1h", "Toggle::add it", {"title": "Docs"}]})).unwrap_err();
    assert_eq!(code, 400);
    assert!(why.contains("::est=<len>") && why.contains("“Toggle”") && why.contains("“Docs”") && !why.contains("Colors"), "{why}");
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM goals", p![]).unwrap(), 0, "nothing is made");

    let r = b.report("tb.goal", json!({"name": "Dark mode", "est": "3h", "tasks": ["Colors::pick them::1::est=45m", {"title": "Docs", "est": "20m"}]})).unwrap();
    let g: i64 = r["goal"].as_str().unwrap()[1..].parse().unwrap();
    assert_eq!(board::get_goal(&b.app, g).unwrap().i("est_min"), Some(180));
    let ests: Vec<(String, Option<i64>, Option<i64>)> = b.app.db.q("SELECT title, est_min, wave FROM tasks WHERE goal_id = ? ORDER BY id", p![g]).unwrap()
        .iter().map(|t| (t.st("title"), t.i("est_min"), t.i("wave"))).collect();
    assert_eq!(ests, vec![("Colors".into(), Some(45), Some(1)), ("Docs".into(), Some(20), None)], "::est= comes off the detail");

    let (code, why) = b.report("tb.propose", json!({"goal": format!("G{g}"), "tasks": ["Toggle::add it"]})).unwrap_err();
    assert_eq!(code, 400);
    assert!(why.contains("“Toggle”"), "{why}");
    b.report("tb.propose", json!({"goal": format!("G{g}"), "tasks": ["Toggle::add it::est=2h", {"title": "Sign", "detail": "1. Sign it.", "kind": "human"}]})).unwrap();
}

#[test]
fn the_first_estimate_is_kept_once_work_starts_and_the_owner_never_sets_one() {
    let b = new_board();
    let t = id_of(&b.report("tb.new_task", json!({"title": "Fix the footer", "est": "45m"})).unwrap());
    let by_agent = |est: &str| json!({"est": est, "by_session": "s1"});

    let (code, why) = b.post(&format!("/tasks/T{t}"), json!({"est": "1h"})).unwrap_err();
    assert_eq!(code, 403, "{why}");
    b.post(&format!("/tasks/T{t}"), by_agent("1h")).unwrap();
    assert_eq!(b.row(t).i("est_min"), Some(60), "changed before it starts");

    b.start(t);
    let (code, why) = b.post(&format!("/tasks/T{t}"), by_agent("2h")).unwrap_err();
    assert_eq!(code, 409);
    assert!(why.contains("first estimate is kept"), "{why}");
    assert_eq!(b.row(t).i("est_min"), Some(60));

    let g: i64 = b.report("tb.goal", json!({"name": "Dark mode", "est": "3h"})).unwrap()["goal"].as_str().unwrap()[1..].parse().unwrap();
    b.post(&format!("/goals/G{g}"), by_agent("4h")).unwrap();
    assert_eq!(board::get_goal(&b.app, g).unwrap().i("est_min"), Some(240));
    let gt = id_of(&b.report("tb.new_task", json!({"title": "Colors", "goal": format!("G{g}"), "est": "1h"})).unwrap());
    b.start(gt);
    let (code, why) = b.post(&format!("/goals/G{g}"), by_agent("5h")).unwrap_err();
    assert_eq!(code, 409, "{why}");
    assert_eq!(board::get_goal(&b.app, g).unwrap().i("est_min"), Some(240));
}

#[test]
fn a_task_made_before_estimates_is_asked_for_one_and_records_it_after_it_started() {
    let b = new_board();
    let t = b.bare_task(None);
    b.start(t);
    let h = handoff::build(&b.app, t).unwrap();
    assert!(h.contains("estimate <len>") && h.contains("Before any other work"), "{h}");

    let r = b.report("tb.estimate", json!({"task": format!("T{t}"), "est": "2h"})).unwrap();
    assert_eq!(r["est"], "2h");
    assert_eq!(b.row(t).i("est_min"), Some(120), "its first estimate, though it has started");
    assert!(b.row(t).s("est_at").is_some());
    assert!(!handoff::build(&b.app, t).unwrap().contains("estimate <len>"));
    assert_eq!(b.report("tb.estimate", json!({"task": format!("T{t}"), "est": "3h"})).unwrap_err().0, 409);
}

#[test]
fn actual_time_counts_only_working_stretches_and_a_goal_has_its_sum_and_running_time() {
    let b = new_board();
    let g = b.post("/goals", json!({"name": "Dark mode", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    let (one, two) = (b.bare_task(Some(g)), b.bare_task(Some(g)));
    let base = now_ts() - 6.0 * 3600.0;
    // One: 30m working, 30m waiting on the owner, 20m working again.
    b.history(one, base, &[(0.0, "queued"), (10.0, "working"), (40.0, "needs"), (70.0, "working"), (90.0, "done")]);
    // Two: 60m working, overlapping one's first stretch by 20m.
    b.history(two, base, &[(0.0, "planned"), (20.0, "working"), (80.0, "done")]);

    assert_eq!(estimate::work_by_task(&b.app, &[one, two]).unwrap().get(&one).map(|s| (s / 60.0).round()), Some(50.0), "the needs stretch is left out");
    board::update_task(&b.app, one, fields!["est_min" => 25]).unwrap();
    let d = b.get(&format!("/tasks/T{one}"));
    assert_eq!(d["estimate"]["actual_min"], 50);
    assert_eq!(d["estimate"]["est_min"], 25);
    assert_eq!(d["estimate"]["est_ratio"], 2.0);
    assert_eq!(estimate::line(&d["estimate"]).unwrap(), "Est 25m · Actual 50m (2×)");

    board::update_goal(&b.app, g, fields!["est_min" => 60]).unwrap();
    let gd = b.get(&format!("/goals/G{g}"));
    assert_eq!(gd["estimate"]["work_min"], 110, "50m and 60m");
    // Working 10–40, 20–80 and 70–90: running from 10 to 90.
    assert_eq!(gd["estimate"]["actual_min"], 80);
    assert_eq!(gd["estimate"]["est_ratio"], 1.3);
}
