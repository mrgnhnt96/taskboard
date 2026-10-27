//! `tb wait-for T<n> --merged`: a task that waits for another task's PR to merge, not just for it to finish (#99).

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, midna, p, prflow, reports, runner, stack, waitsfor};

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
    on_branch(&b, one, "feat/endpoint");
    b.on("s2", two);
    (one, two)
}

fn on_branch(b: &Board, id: i64, branch: &str) {
    prflow::merge_flow(&b.app, id, vec![("rec", json!({"state": "OPEN", "branch": branch, "base": "main"}))]).unwrap();
}

#[test]
fn it_parks_while_the_pr_is_open_and_resumes_once_it_merges() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    let dir = b.app.cfg.claude_projects.join("-webapp");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("c-s2.jsonl"), "{\"type\":\"user\"}\n").unwrap();

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
fn a_wait_on_done_work_with_an_open_pr_stacks_on_it() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")]}));
    assert_eq!(out["parked"], false, "{out}");
    let text = out["context"].as_str().unwrap();
    assert!(text.contains(&format!("This task stacks on T{one} (branch feat/endpoint, PR #9)")), "{text}");
    assert!(text.contains("open the PR into feat/endpoint"), "{text}");
    assert!(!text.contains("--merged instead"), "a stack waits for the merge itself: {text}");
    assert!(!text.contains("This task needs work from another task"), "{text}");
    let t = b.row(two);
    assert_eq!(t.i("pr_after"), Some(one));
    assert!(waitsfor::ids(&t).is_empty(), "the wait became the stack");
    assert_eq!(t.s("stack_merge"), None, "a tail merge");
    assert_eq!(stack::base_branch(&b.app, &t).unwrap().as_deref(), Some("feat/endpoint"));
    assert!(stack::holds(&b.app, &t).unwrap(), "its PR waits for #9");
    assert_eq!(t.s("status"), Some("working"));
}

#[test]
fn a_parked_wait_stacks_once_the_work_is_done_with_its_pr_open() {
    let b = new_board();
    let one = b.task("Add the endpoint");
    let two = b.task("Use the endpoint");
    b.on("s1", one);
    b.on("s2", two);
    let dir = b.app.cfg.claude_projects.join("-webapp");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("c-s2.jsonl"), "{\"type\":\"user\"}\n").unwrap();
    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")], "why": "Needs the endpoint"}));
    assert_eq!(out["parked"], true, "{out}");

    b.report("tb.done", "s1", json!({"summary": "Done.", "pr": "https://github.com/acme/webapp/pull/9"}));
    on_branch(&b, one, "feat/endpoint");
    runner::start_queued(&b.app).unwrap();
    let t = b.row(two);
    assert_eq!(t.i("pr_after"), Some(one), "it stacks before it starts again");
    assert!(waitsfor::ids(&t).is_empty());
    let jobs = b.starts(two);
    assert_eq!(jobs.len(), 1);
    let prompt = board::job_args(&jobs[0]).st("prompt").to_string();
    assert!(prompt.contains("open the PR into feat/endpoint"), "{prompt}");
    assert!(!prompt.contains("This task needs work from another task"), "the stack line replaces the rebase advice: {prompt}");
}

#[test]
fn a_wait_on_a_merged_pr_stays_a_plain_wait_on_main() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    board::update_task(&b.app, one, vec![("pr_state", json!("MERGED")), ("pr_phase", json!("merged"))]).unwrap();
    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")]}));
    assert_eq!(out["parked"], false, "{out}");
    let text = out["context"].as_str().unwrap();
    assert!(!text.contains("stacks on"), "{text}");
    let t = b.row(two);
    assert_eq!(t.i("pr_after"), None);
    assert_eq!(waitsfor::ids(&t), vec![one]);
    assert_eq!(stack::base_branch(&b.app, &t).unwrap(), None);
}

#[test]
fn a_merged_wait_on_an_open_pr_stays_a_wait() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")], "merged": true}));
    let t = b.row(two);
    assert_eq!(t.i("pr_after"), None, "--merged asks for main, not a stack");
    assert_eq!(waitsfor::ids(&t), vec![one]);
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

/// T3 stacks on T1 with the given merge mode, is done, and its PR #10 merged into T1's branch.
fn stacked_and_merged(b: &Board, parent: i64, mode: &str) -> i64 {
    let three = b.task("Polish the endpoint");
    let mode = if mode == "head" { json!("head") } else { Value::Null };
    board::update_task(
        &b.app,
        three,
        vec![("pr_after", json!(parent)), ("stack_merge", mode), ("status", json!("done")), ("pr_num", json!(10)),
             ("pr_url", json!("https://github.com/acme/webapp/pull/10")), ("pr_state", json!("MERGED")), ("pr_phase", json!("merged"))],
    )
    .unwrap();
    three
}

#[test]
fn a_head_merged_pr_counts_as_merged_once_its_stack_reaches_main() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    let three = stacked_and_merged(&b, one, "head");
    assert!(!waitsfor::merged(&b.app, Some(&b.row(three))).unwrap());

    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{three}")], "merged": true}));
    assert_eq!(out["parked"], true, "{out}");
    let w = b.card(two)["waiting"].as_str().unwrap().to_string();
    assert!(w.contains(&format!("merged into T{one}'s branch")) && w.contains(&format!("once T{one}'s PR #9 merges")), "{w}");
    assert!(runner::start_queued(&b.app).unwrap().is_empty(), "its work isn't on main yet");

    board::update_task(&b.app, one, vec![("pr_state", json!("MERGED")), ("pr_phase", json!("merged"))]).unwrap();
    assert!(waitsfor::merged(&b.app, Some(&b.row(three))).unwrap());
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![two]);
}

#[test]
fn a_tail_merged_pr_counts_as_merged_when_it_merges() {
    let b = new_board();
    let (one, _) = done_with_open_pr(&b);
    let three = stacked_and_merged(&b, one, "tail");
    assert!(waitsfor::merged(&b.app, Some(&b.row(three))).unwrap());
}
