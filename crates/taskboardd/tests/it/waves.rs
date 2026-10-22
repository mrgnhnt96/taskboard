//! Waves on a goal (`docs/parity/oct8-changes.md` #5): states, review stops, a failed task holding the
//! next wave, "Continue to wave N", and the runner waiting on them. A later wave starts once the ones
//! before it have nothing left that could start (#636).

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, fields, human, midna, p, reports, runner, waitsfor};

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
    fn set(&self, id: i64, status: &str, failed: bool) {
        board::update_task(&self.app, id, fields!["status" => status, "failed" => failed as i64]).unwrap();
    }
    fn waves(&self, g: i64) -> Vec<Value> {
        self.get(&format!("goals/G{g}"))["waves"].as_array().cloned().unwrap()
    }
    fn waiting(&self, id: i64) -> Value {
        waitsfor::waiting_line(&self.app, &board::get_task(&self.app, id).unwrap()).unwrap()
    }
}

/// A goal planned through `tb propose` with two tasks in wave 1, one in wave 2 and one with no wave.
fn planned(b: &Board) -> (i64, Vec<i64>) {
    plan(b, &["Model::add it::1", "Form::build it::1", "Wire::connect them::2", "Docs::write them"], 2)
}

/// A goal planned through `tb propose` ("title::detail::wave") and run, `max` terminals at a time.
fn plan(b: &Board, tasks: &[&str], max: i64) -> (i64, Vec<i64>) {
    midna::sync(&b.app, &[json!({"id": "s1", "name": "Planner", "agent": "claude", "cwd": b.dir.path().join("webapp").to_string_lossy(), "status": {"state": "idle"}})], &[]).unwrap();
    let g = b.post("/goals", json!({"name": "Login", "project": "webapp", "run_in_order": true, "max_terminals": max}))["id"].as_i64().unwrap();
    let r = reports::handle(&b.app, json!({"event": "tb.propose", "session": "s1", "goal": format!("G{g}"), "tasks": tasks}), false).unwrap();
    let ids: Vec<i64> = r["created"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()[1..].parse().unwrap()).collect();
    api::dispatch(&b.app, "POST", &format!("goals/G{g}/run"), &[(api::FROM.to_string(), "app".to_string())].into_iter().collect::<Query>(), &json!({})).unwrap();
    (g, ids)
}

impl Board {
    /// Done, with its PR open in review.
    fn in_review(&self, id: i64, pr: i64) {
        board::update_task(&self.app, id, fields!["status" => "done", "pr_num" => pr, "pr_state" => "OPEN"]).unwrap();
    }
    fn merge(&self, id: i64) {
        board::update_task(&self.app, id, fields!["pr_state" => "MERGED", "pr_phase" => "merged"]).unwrap();
    }
    fn wait_on(&self, id: i64, on: i64) {
        self.app.db.x("UPDATE tasks SET waits_for = ? WHERE id = ?", p![json!([on]).to_string(), id]).unwrap();
    }
    /// Waits for `on`'s PR to merge, as a task stacked on it does.
    fn wait_for_merge(&self, id: i64, on: i64) {
        self.wait_on(id, on);
        waitsfor::set_merged(&self.app, &board::get_task(&self.app, id).unwrap(), &[on]).unwrap();
    }
    /// Why its goal holds it back, if it does.
    fn goal_hold(&self, id: i64) -> Option<String> {
        let t = board::get_task(&self.app, id).unwrap();
        runner::goal_blocker(&self.app, &t, &board::get_goal(&self.app, t.i0("goal_id")).unwrap()).unwrap()
    }
}

#[test]
fn a_wave_waits_for_the_one_before_it() {
    let b = new_board();
    let (g, t) = planned(&b);
    assert_eq!(board::get_task(&b.app, t[2]).unwrap().i("wave"), Some(2), "title::detail::2 sets the wave");
    assert_eq!(board::get_task(&b.app, t[3]).unwrap().i("wave"), None);
    let w = b.waves(g);
    assert_eq!(w.iter().map(|w| w["state"].as_str().unwrap()).collect::<Vec<_>>(), vec!["ready", "waiting"]);
    assert_eq!(w[0]["to_start"], json!([format!("T{}", t[0]), format!("T{}", t[1])]));
    assert_eq!(w[1]["hold"], format!("Waits for wave 1 to start T{}, T{}", t[0], t[1]));
    assert!(b.waiting(t[1]).is_null() || !b.waiting(t[1]).as_str().unwrap().contains("Waits for"), "waves replace running in order: {}", b.waiting(t[1]));
    assert_eq!(b.waiting(t[2]), format!("Waits for wave 1 to start T{}, T{}", t[0], t[1]));

    b.set(t[0], "working", false);
    assert_eq!(b.waves(g)[0]["state"], "running");
    assert_eq!(b.waiting(t[2]), format!("Waits for wave 1 to start T{}", t[1]));
    b.set(t[0], "done", false);
    b.set(t[1], "done", false);
    let w = b.waves(g);
    assert_eq!(w[0]["state"], "done");
    assert_eq!(w[1]["state"], "ready");
    assert!(b.waiting(t[2]).is_null() || !b.waiting(t[2]).as_str().unwrap().contains("wave"));
}

#[test]
fn a_review_stop_holds_the_goal_until_the_owner_continues() {
    let b = new_board();
    let (g, t) = planned(&b);
    // The review stop is the owner's checkbox in the app (`_from=app`).
    let mut from_app = Query::new();
    from_app.insert(api::FROM.into(), "app".into());
    api::dispatch(&b.app, "POST", &format!("goals/G{g}/waves/1"), &from_app, &json!({"name": "Basics", "stop_after": true})).unwrap();
    b.set(t[0], "done", false);
    b.set(t[1], "done", false);
    let w = b.waves(g);
    assert_eq!(w[0]["state"], "stopped");
    assert_eq!(w[0]["name"], "Basics");
    assert_eq!(b.waiting(t[2]), "Stopped for your review after wave 1 (Basics)");
    assert_eq!(b.get(&format!("goals/G{g}"))["stopped"], "Wave 1 (Basics) is done. Review it, then continue");

    let d = b.post(&format!("goals/G{g}/waves/1/continue"), json!({}));
    assert_eq!(d["waves"][0]["state"], "done");
    assert!(d["waves"][0]["released_at"].is_string());
    assert!(d["stopped"].is_null());
    assert_eq!(d["waves"][1]["state"], "ready");
}

#[test]
fn a_failed_task_holds_the_next_wave_until_the_owner_goes_on() {
    let b = new_board();
    let (g, t) = planned(&b);
    b.set(t[0], "done", true);
    b.set(t[1], "done", false);
    let w = b.waves(g);
    assert_eq!(w[0]["state"], "failed");
    assert_eq!(w[0]["failed"], json!([format!("T{}", t[0])]));
    assert_eq!(b.waiting(t[2]), format!("T{} in wave 1 failed. Try again, or continue past it", t[0]));
    b.post(&format!("goals/G{g}/waves/1/continue"), json!({"who": "Planner"}));
    assert_eq!(b.waves(g)[1]["state"], "ready");
}

#[test]
fn a_task_moves_between_waves() {
    let b = new_board();
    let (g, t) = planned(&b);
    b.post(&format!("tasks/T{}", t[3]), json!({"wave": 2}));
    assert_eq!(b.waves(g)[1]["total"], 2);
    b.post(&format!("tasks/T{}", t[2]), json!({"wave": "none"}));
    assert_eq!(b.get(&format!("tasks/T{}", t[2]))["wave"], Value::Null);
    let loose = b.post("/tasks", json!({"title": "Loose", "project": "webapp"}))["id"].as_i64().unwrap();
    let e = api::dispatch(&b.app, "POST", &format!("tasks/T{loose}"), &Query::new(), &json!({"wave": 1})).expect_err("no goal");
    assert!(e.message.contains("Only a task in a goal"));
}

#[test]
fn the_next_wave_starts_while_a_task_waits_on_its_stack_parents_open_pr() {
    let b = new_board();
    let (g, t) = planned(&b);
    b.in_review(t[0], 12);
    b.wait_for_merge(t[1], t[0]);
    assert_eq!(b.waiting(t[1]), format!("Blocked by T{} (G{g}) until PR #12 merges", t[0]));
    let w = b.waves(g);
    assert_eq!(w[0]["passed"], true, "nothing left in wave 1 could start: {}", w[0]);
    assert_eq!(w[0]["state"], "running");
    assert_eq!(w[0]["to_start"], json!([]));
    assert_eq!(w[1]["state"], "ready");
    assert_eq!(b.goal_hold(t[2]), None);
    assert!(runner::start_queued(&b.app).unwrap().contains(&t[2]), "wave 2 starts");
    assert_eq!(b.waves(g)[1]["state"], "running");
}

#[test]
fn a_task_that_waits_on_the_straggler_keeps_waiting_while_its_wave_mates_start() {
    let b = new_board();
    let (g, t) = plan(&b, &["Model::add it::1", "Form::build it::1", "Wire::connect them::2", "Docs::write them::2"], 4);
    b.in_review(t[0], 12);
    b.wait_for_merge(t[1], t[0]);
    b.wait_on(t[2], t[1]);
    assert_eq!(b.goal_hold(t[2]), None, "wave 1 doesn't hold it");
    assert_eq!(b.waiting(t[2]), format!("Blocked by T{} (G{g})", t[1]));
    let started = runner::start_queued(&b.app).unwrap();
    assert!(started.contains(&t[3]), "its wave mate starts: {started:?}");
    assert!(!started.contains(&t[1]) && !started.contains(&t[2]), "{started:?}");
}

#[test]
fn a_waiting_chain_runs_across_three_waves() {
    let b = new_board();
    let (g, t) = plan(&b, &["Model::add it::1", "Form::build it::1", "Wire::connect them::2", "Polish::finish it::3", "Docs::write them::3"], 4);
    b.in_review(t[0], 12);
    b.wait_for_merge(t[1], t[0]);
    b.wait_on(t[2], t[1]);
    b.wait_on(t[3], t[2]);
    let w = b.waves(g);
    assert_eq!(w.iter().map(|w| w["passed"].as_bool().unwrap()).collect::<Vec<_>>(), vec![true, true, false]);
    assert_eq!(b.goal_hold(t[4]), None, "wave 3 starts what doesn't wait");
    assert_eq!(b.waiting(t[2]), format!("Blocked by T{} (G{g})", t[1]));
    assert_eq!(b.waiting(t[3]), format!("Blocked by T{} (G{g})", t[2]));

    // The PR merges: the straggler could start, so it holds the later waves again and goes first, and
    // once it has started wave 3 goes on.
    b.merge(t[0]);
    assert_eq!(b.waiting(t[4]), format!("Waits for wave 1 to start T{}", t[1]));
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![t[1], t[4]]);
    assert_eq!(b.waiting(t[2]), format!("Blocked by T{} (G{g})", t[1]));
}

#[test]
fn an_earlier_task_waiting_only_on_a_terminal_starts_before_later_waves() {
    let b = new_board();
    let (g, t) = plan(&b, &["Model::add it::1", "Form::build it::1", "Wire::connect them::2"], 1);
    b.set(t[0], "working", false);
    assert_eq!(b.goal_hold(t[1]).unwrap(), "Waits for a free slot: its goal runs 1 terminal at a time");
    assert_eq!(b.waves(g)[0]["passed"], false);
    assert_eq!(b.waiting(t[2]), format!("Waits for wave 1 to start T{}", t[1]));
    assert!(runner::start_queued(&b.app).unwrap().is_empty());

    b.set(t[0], "done", false);
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![t[1]], "the free terminal goes to wave 1");
    assert_eq!(b.waves(g)[0]["passed"], true);
    assert_eq!(b.goal_hold(t[2]).unwrap(), "Waits for a free slot: its goal runs 1 terminal at a time");
}

#[test]
fn a_human_task_doesnt_hold_the_next_wave() {
    let b = new_board();
    let (g, t) = planned(&b);
    b.app.db.x("UPDATE tasks SET kind = ? WHERE id = ?", p![human::KIND, t[0]]).unwrap();
    b.set(t[1], "working", false);
    assert_eq!(b.waves(g)[0]["passed"], true);
    assert_eq!(b.goal_hold(t[2]), None);
}

#[test]
fn a_review_stop_a_hold_and_a_failed_task_still_hold_the_next_wave() {
    let b = new_board();
    let (g, t) = plan(&b, &["Model::add it::1", "Form::build it::1", "Wire::connect them::2"], 4);
    b.set(t[0], "working", false);
    b.set(t[1], "working", false);
    assert_eq!(b.goal_hold(t[2]), None, "everything in wave 1 started");

    let mut from_app = Query::new();
    from_app.insert(api::FROM.into(), "app".into());
    api::dispatch(&b.app, "POST", &format!("goals/G{g}/waves/1"), &from_app, &json!({"stop_after": true})).unwrap();
    assert_eq!(b.waiting(t[2]), "Waits for wave 1 to finish, then your review");
    api::dispatch(&b.app, "POST", &format!("goals/G{g}/waves/1"), &from_app, &json!({"stop_after": false})).unwrap();

    b.post(&format!("goals/G{g}/waves/1/hold"), json!({}));
    assert_eq!(b.waiting(t[2]), "Wave 1 is held until it's continued");
    b.post(&format!("goals/G{g}/waves/1/hold"), json!({"on": false}));

    b.set(t[0], "done", true);
    assert_eq!(b.waiting(t[2]), format!("T{} in wave 1 failed. Try again, or continue past it", t[0]));
    b.post(&format!("goals/G{g}/waves/1/continue"), json!({"who": "Planner"}));
    assert_eq!(b.goal_hold(t[2]), None);
}
