//! `tb wait-for T<n> --merged`: a task that waits for another task's PR to merge, not just for it to finish (#99).

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, midna, p, reports, runner, waitsfor};

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
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
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
    fn task(&self, title: &str) -> i64 {
        self.post("/tasks", json!({"title": title, "detail": "Do it.", "project": "webapp", "pickup": "new"}))["id"].as_i64().unwrap()
    }
    fn on(&self, sid: &str, id: i64) {
        self.session(sid);
        self.report("tb.take", sid, json!({"task": format!("T{id}")}));
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn card(&self, id: i64) -> Value {
        self.get(&format!("tasks/T{id}"))
    }
    fn starts(&self, id: i64) -> Vec<Row> {
        self.app.db.q("SELECT * FROM jobs WHERE kind = 'agent' AND purpose = 'start' AND task_id = ? ORDER BY id", p![id]).unwrap()
    }
}

/// T1 is done with PR #9 open; T2 is on s2.
fn done_with_open_pr(b: &Board) -> (i64, i64) {
    let one = b.task("Add the endpoint");
    let two = b.task("Use the endpoint");
    b.on("s1", one);
    b.report("tb.done", "s1", json!({"summary": "Done.", "pr": "https://github.com/acme/webapp/pull/9"}));
    assert_eq!(b.row(one).i("pr_num"), Some(9));
    b.on("s2", two);
    (one, two)
}

#[test]
fn it_parks_while_the_pr_is_open_and_resumes_once_it_merges() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);

    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")], "merged": true, "why": "Needs it on main"}));
    assert_eq!(out["parked"], true, "{out}");
    assert!(out["context"].as_str().unwrap().contains(&format!("Blocked by T{one} until PR #9 merges")), "{out}");
    assert_eq!(waitsfor::merged_ids(&b.row(two)), vec![one]);
    let card = b.card(two);
    assert_eq!(card["status"], "queued");
    assert_eq!(card["blocked"], true);
    assert_eq!(card["waits_for_state"], json!([{"ref": format!("T{one}"), "done": false}]));
    assert!(card["waiting"].as_str().unwrap().contains("until PR #9 merges"), "{card}");
    assert!(runner::start_queued(&b.app).unwrap().is_empty(), "it waits while the PR is open");
    assert!(b.starts(two).is_empty());

    board::update_task(&b.app, one, vec![("pr_state", json!("MERGED")), ("pr_phase", json!("merged"))]).unwrap();
    assert_eq!(b.card(two)["waits_for_state"][0]["done"], true);
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![two]);
    let jobs = b.starts(two);
    assert_eq!(jobs.len(), 1);
    let args = board::job_args(&jobs[0]);
    assert_eq!(args.st("flags"), "--resume c-s2", "it carries on in the parked conversation");
    assert!(args.st("prompt").contains("is merged (PR #9)"), "{}", args.st("prompt"));
}

#[test]
fn a_declined_pr_says_so_and_keeps_it_waiting() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")], "merged": true}));
    board::update_task(&b.app, one, vec![("pr_state", json!("CLOSED")), ("pr_phase", json!("declined"))]).unwrap();
    let w = b.card(two)["waiting"].as_str().unwrap().to_string();
    assert!(w.contains(&format!("T{one}")) && w.contains("whose PR #9 was declined"), "{w}");
    assert!(runner::start_queued(&b.app).unwrap().is_empty());
}

#[test]
fn a_plain_wait_on_done_work_is_ready_and_hints_at_merged() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")]}));
    assert_eq!(out["parked"], false, "{out}");
    let text = out["context"].as_str().unwrap();
    assert!(text.contains(&format!("run tb wait-for T{one} --merged instead")), "{text}");
    assert!(waitsfor::merged_ids(&b.row(two)).is_empty());
    assert_eq!(b.row(two).s("status"), Some("working"));
}

#[test]
fn work_with_no_pr_counts_as_merged() {
    let b = new_board();
    let one = b.task("Add the endpoint");
    let two = b.task("Use the endpoint");
    b.on("s1", one);
    b.report("tb.done", "s1", json!({"summary": "Done.", "no_pr": "Config only"}));
    b.on("s2", two);
    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")], "merged": true}));
    assert_eq!(out["parked"], false, "{out}");
    assert!(!out["context"].as_str().unwrap_or("").contains("--merged"), "{out}");
    assert_eq!(b.card(two)["waits_for_state"][0]["done"], true);
}

#[test]
fn a_task_that_ships_no_pr_counts_as_merged() {
    let b = new_board();
    let one = b.task("Add the endpoint");
    let two = b.task("Use the endpoint");
    b.post(&format!("/tasks/T{one}"), json!({"ships_pr": "no"}));
    b.on("s1", one);
    b.report("tb.done", "s1", json!({"summary": "Done."}));
    b.on("s2", two);
    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")], "merged": true}));
    assert_eq!(out["parked"], false, "{out}");
}

#[test]
fn waiting_for_none_forgets_the_merge_waits() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")], "merged": true}));
    assert_eq!(waitsfor::merged_ids(&b.row(two)), vec![one]);
    b.on("s2", two);
    b.report("tb.wait_for", "s2", json!({"tasks": "none"}));
    assert!(waitsfor::merged_ids(&b.row(two)).is_empty());
}
