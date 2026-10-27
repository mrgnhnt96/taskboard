//! The lens review stage (`[pr.lens]`, `lens_pr.rs`): a new PR gets lens's PR pass (`lens review --pr`),
//! and the `lens` stage while it runs; it never holds a merge.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{jloads_obj, now_iso, RowExt};
use taskboardd::prhost::{self, Check, FakeHost, Record, Thread};
use taskboardd::{board, lens_pr, midna, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

/// A board whose `lens` records its arguments and folder, and prints a summary (exit 0) or, when `fail`
/// exists, exits 1; when `slow` exists, it first sleeps 30 seconds.
fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    cfg.pr.wake = true;
    let lens = dir.path().join("fake-lens");
    std::fs::write(
        &lens,
        "#!/bin/sh\nd=$(dirname \"$0\")\necho \"$PWD|$*\" >> \"$d/calls.txt\"\nif [ -f \"$d/slow\" ]; then sleep 30; fi\nif [ -f \"$d/fail\" ]; then echo 'Error: no gh login' >&2; exit 1; fi\necho 'Posted 2 findings.'\n",
    )
    .unwrap();
    std::fs::set_permissions(&lens, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.lens.bin = lens.to_string_lossy().to_string();
    f(&mut cfg);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    // The read after lens posts goes to this host, not the real one.
    prhost::install(&app, FakeHost::new("github", green_record()));
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

fn lens_on() -> Board {
    board_with(|c| c.pr.lens.on = true)
}

fn green_record() -> Record {
    Record { state: "OPEN".into(), head: "h1".into(), checks: vec![Check { name: "ci".into(), state: "passed".into(), url: None, at: None }], ..Default::default() }
}

fn green() -> Value {
    json!({"state": "OPEN", "head": "h1", "checks": [{"name": "ci", "state": "passed"}], "failed": [], "running": 0,
           "comments": 0, "approvals": 0, "review_decision": "", "changes_at": ""})
}

impl Board {
    fn call(&self, method: &str, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, method, path, &Query::new(), &body).unwrap_or_else(|e| panic!("{method} {path}: {}", e.message))
    }
    fn report(&self, event: &str, extra: Value) {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message));
    }
    fn pr_task(&self) -> i64 {
        let id = self.call("POST", "/tasks", json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")}));
        self.report("tb.done", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9"}));
        id
    }
    fn task(&self, id: i64) -> taskboardd::util::Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn lens(&self, id: i64) -> Value {
        jloads_obj(self.task(id).s("pr_flow")).get("lens").cloned().unwrap_or(Value::Null)
    }
    /// The visits the board woke the PR's agent for, oldest first.
    fn visits(&self, id: i64) -> Vec<Value> {
        let jobs = self.app.db.q("SELECT args FROM jobs WHERE purpose = 'pr' AND task_id = ? ORDER BY id", taskboardd::p![id]).unwrap();
        jobs.iter().map(|j| Value::Object(board::job_args(j).into_iter().collect())).collect()
    }
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("calls.txt")).unwrap_or_default().lines().map(|l| l.to_string()).collect()
    }
    /// Steps a record with the lens run as given, as if it were still going.
    fn step_with_lens(&self, id: i64, lens: Value, rec: &Value) -> taskboardd::util::Row {
        prflow::merge_flow(&self.app, id, vec![("lens", lens)]).unwrap();
        self.app.db.tx(|| prflow::step(&self.app, &self.task(id), rec)).unwrap();
        self.task(id)
    }
}

#[test]
fn off_by_default_a_new_pr_gets_no_lens_review() {
    let b = board_with(|_| {});
    let id = b.pr_task();
    prflow::apply(&b.app, id, &green()).unwrap();
    assert!(b.calls().is_empty(), "lens wasn't run");
    assert!(b.lens(id).is_null());
    assert_eq!(b.task(id).s("pr_phase"), Some("review"));
}

#[test]
fn a_new_pr_gets_the_lens_pr_pass_once() {
    let b = lens_on();
    let id = b.pr_task();
    prflow::apply(&b.app, id, &green()).unwrap();
    let calls = b.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let repo = b.dir.path().join("webapp").canonicalize().unwrap();
    let (cwd, args) = calls[0].split_once('|').unwrap();
    assert_eq!(std::path::Path::new(cwd).canonicalize().unwrap(), repo, "run from the PR's repo");
    assert_eq!(args, "review --pr 9");
    let lens = b.lens(id);
    assert_eq!(lens["state"], "done", "{lens}");
    assert_eq!(lens["head"], "h1");
    assert!(std::fs::read_to_string(lens["log"].as_str().unwrap()).unwrap().contains("Posted 2 findings."));
    assert_eq!(b.task(id).s("pr_phase"), Some("review"), "done, the PR moves on");
    let events = b.call("GET", &format!("/tasks/T{id}"), json!({})).to_string();
    assert!(events.contains("lens reviewed it"), "{events}");
    prflow::apply(&b.app, id, &green()).unwrap();
    let mut pushed = green();
    pushed["head"] = json!("h2");
    prflow::apply(&b.app, id, &pushed).unwrap();
    assert_eq!(b.calls().len(), 1, "a later read or push isn't reviewed again");
}

#[test]
fn a_project_can_turn_it_on_or_off() {
    let b = board_with(|c| {
        c.pr.projects.insert("webapp".into(), taskboardd::config::PrProject { lens: Some(true), ..Default::default() });
    });
    let id = b.pr_task();
    prflow::apply(&b.app, id, &green()).unwrap();
    assert_eq!(b.calls().len(), 1);
    let b = board_with(|c| {
        c.pr.lens.on = true;
        c.pr.projects.insert("webapp".into(), taskboardd::config::PrProject { lens: Some(false), ..Default::default() });
    });
    let id = b.pr_task();
    prflow::apply(&b.app, id, &green()).unwrap();
    assert!(b.calls().is_empty());
}

#[test]
fn a_failed_run_is_tried_again_then_holds_the_pr_and_asks_the_owner() {
    let b = lens_on();
    std::fs::write(b.dir.path().join("fail"), "").unwrap();
    let id = b.pr_task();
    prflow::apply(&b.app, id, &green()).unwrap();
    assert_eq!(b.calls().len(), 2, "tried twice");
    let lens = b.lens(id);
    assert_eq!(lens["state"], "failed", "{lens}");
    assert_eq!(lens["error"], "no gh login");
    assert_eq!(b.task(id).s("pr_phase"), Some("lens"), "it still holds the PR");
    let card = b.call("GET", &format!("/tasks/T{id}"), json!({})).to_string();
    assert!(card.contains("so it runs again"), "{card}");
    assert!(card.contains("the lens review failed: no gh login"), "{card}");
    let alerts = taskboardd::dispatch::alerts(&b.app);
    assert!(alerts.iter().any(|a| a.to_string().contains("--retry")), "the owner is asked: {alerts:?}");
}

#[test]
fn the_owner_runs_a_failed_review_again_or_lets_the_pr_go_without_it() {
    let b = lens_on();
    std::fs::write(b.dir.path().join("fail"), "").unwrap();
    let id = b.pr_task();
    prflow::apply(&b.app, id, &green()).unwrap();
    let refused = api::dispatch(&b.app, "POST", &format!("/tasks/T{id}/pr/lens"), &Query::new(), &json!({"skip": "lens is down", "agent": true}));
    assert_eq!(refused.unwrap_err().status, 403, "an agent can't skip it");
    std::fs::remove_file(b.dir.path().join("fail")).unwrap();
    b.call("POST", &format!("/tasks/T{id}/pr/lens"), json!({"retry": true}));
    assert_eq!(b.calls().len(), 3);
    assert_eq!(b.lens(id)["state"], "done");
    assert_eq!(b.task(id).s("pr_phase"), Some("review"), "done, the PR moves on");

    let id = b.pr_task();
    std::fs::write(b.dir.path().join("fail"), "").unwrap();
    prflow::apply(&b.app, id, &green()).unwrap();
    b.call("POST", &format!("/tasks/T{id}/pr/lens"), json!({"skip": "lens is down today"}));
    let lens = b.lens(id);
    assert_eq!(lens["state"], "skipped", "{lens}");
    assert_eq!(lens["reason"], "lens is down today");
    assert_eq!(b.task(id).s("pr_phase"), Some("review"));
}

#[test]
fn without_lens_installed_the_run_fails() {
    let b = lens_on();
    std::fs::remove_file(&b.app.cfg.lens.bin).unwrap();
    let id = b.pr_task();
    prflow::apply(&b.app, id, &green()).unwrap();
    assert!(b.calls().is_empty());
    assert_eq!(b.lens(id)["error"], "lens isn't installed");
    assert_eq!(b.task(id).s("pr_phase"), Some("lens"));
}

#[test]
fn a_closed_pr_isn_t_reviewed() {
    let b = lens_on();
    let id = b.pr_task();
    let mut rec = green();
    rec["state"] = json!("MERGED");
    prflow::apply(&b.app, id, &rec).unwrap();
    assert!(b.calls().is_empty());
}

#[test]
fn while_lens_runs_the_pr_shows_the_lens_stage_before_review() {
    let b = lens_on();
    let id = b.pr_task();
    let t = b.step_with_lens(id, json!({"state": "running", "head": "h1", "at": now_iso(), "started_at": now_iso()}), &green());
    assert_eq!(t.s("pr_phase"), Some("lens"));
    assert_eq!(prflow::label("lens"), "Lens review");
    assert!(!prflow::awaiting_owner(&t), "the owner is asked to look once lens has posted");
    let card = b.call("GET", &format!("/tasks/T{id}"), json!({}));
    assert_eq!(card["pr"]["bar"]["lens"]["state"], "running", "{}", card["pr"]);
}

#[test]
fn lens_holds_the_merge_but_not_a_failed_check() {
    let b = board_with(|c| {
        c.pr.lens.on = true;
        c.pr.approvals = 1;
        c.pr.agents_merge = Some(false);
    });
    let id = b.pr_task();
    let running = json!({"state": "running", "head": "h1", "at": now_iso(), "started_at": now_iso()});
    let mut failed = green();
    failed["failed"] = json!(["ci"]);
    failed["checks"] = json!([{"name": "ci", "state": "failed"}]);
    assert_eq!(b.step_with_lens(id, running.clone(), &failed).s("pr_phase"), Some("fix"));
    let mut approved = green();
    approved["approvals"] = json!(1);
    approved["review_decision"] = json!("APPROVED");
    let t = b.step_with_lens(id, running, &approved);
    assert_eq!(t.s("pr_phase"), Some("lens"), "an approved PR waits for lens");
    let blockers = taskboardd::prcmds::merge_blockers(&b.app, &t, &approved, true).unwrap();
    assert_eq!(blockers, ["lens is still reviewing it"]);
    let done = json!({"state": "done", "head": "h1", "at": now_iso()});
    assert_eq!(b.step_with_lens(id, done, &approved).s("pr_phase"), Some("merge"));
}

#[test]
fn its_findings_hold_the_merge_until_resolved_though_lens_posts_as_us() {
    let b = board_with(|c| {
        c.pr.lens.on = true;
        c.pr.approvals = 1;
    });
    let id = b.pr_task();
    let mut rec = green();
    rec["approvals"] = json!(1);
    rec["review_decision"] = json!("APPROVED");
    rec["viewer"] = json!("morgan");
    let finding = |resolved: bool| {
        json!({"id": "th1", "kind": "review", "resolvable": true, "resolved": resolved, "author": "morgan", "author_name": "morgan",
               "last_author": "morgan", "last_id": "c1", "last_at": now_iso(), "text": "Off by one",
               "body": "Off by one.\n\n<!-- lens:finding key=abc -->"})
    };
    let done = json!({"state": "done", "head": "h1", "at": now_iso()});
    rec["threads"] = json!([finding(false)]);
    assert_eq!(b.step_with_lens(id, done.clone(), &rec).s("pr_phase"), Some("comments"));
    rec["threads"] = json!([finding(true)]);
    assert_eq!(b.step_with_lens(id, done, &rec).s("pr_phase"), Some("merge"));
}

#[test]
fn a_run_the_board_lost_is_run_again() {
    let b = lens_on();
    let id = b.pr_task();
    let t = b.step_with_lens(id, json!({"state": "running", "head": "h1", "at": "2020-01-01T00:00:00Z", "started_at": "2020-01-01T00:00:00Z", "tries": 1}), &green());
    assert_eq!(t.s("pr_phase"), Some("lens"), "it still holds");
    let card = b.call("GET", &format!("/tasks/T{id}"), json!({}));
    assert_eq!(card["pr"]["bar"]["lens"]["state"], "failed", "{}", card["pr"]);
    prflow::apply(&b.app, id, &green()).unwrap();
    assert_eq!(b.calls().len(), 1, "the next read runs it again");
    let lens = b.lens(id);
    assert_eq!(lens["state"], "done", "{lens}");
    assert_eq!(lens["tries"], 2);
}

#[test]
fn a_queued_run_is_skipped_once_the_pr_has_merged() {
    let b = lens_on();
    let id = b.pr_task();
    prflow::merge_flow(&b.app, id, vec![("lens", json!({"state": "queued", "head": "h1", "at": now_iso()}))]).unwrap();
    prflow::mark_merged(&b.app, &b.task(id), "Morgan").unwrap();
    lens_pr::run(&b.app, id).unwrap();
    assert!(b.calls().is_empty(), "lens wasn't run");
    let lens = b.lens(id);
    assert_eq!(lens["state"], "skipped", "{lens}");
    assert_eq!(lens["reason"], "the PR merged");
}

#[test]
fn a_running_review_is_killed_when_the_pr_merges() {
    let b = lens_on();
    std::fs::write(b.dir.path().join("slow"), "").unwrap();
    let id = b.pr_task();
    prflow::merge_flow(&b.app, id, vec![("lens", json!({"state": "queued", "head": "h1", "at": now_iso()}))]).unwrap();
    let app = b.app.clone();
    let merger = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        prflow::mark_merged(&app, &board::get_task(&app, id).unwrap(), "Morgan").unwrap();
    });
    let started = std::time::Instant::now();
    lens_pr::run(&b.app, id).unwrap();
    merger.join().unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "killed, not waited out: {:?}", started.elapsed());
    assert_eq!(b.calls().len(), 1, "it had started");
    let lens = b.lens(id);
    assert_eq!(lens["state"], "stopped", "{lens}");
    assert_eq!(lens["reason"], "the PR merged");
    let events = b.call("GET", &format!("/tasks/T{id}"), json!({})).to_string();
    assert!(events.contains("stopped the lens review, the PR merged"), "{events}");
}

#[test]
fn once_lens_posts_the_pr_is_read_again_so_its_findings_hold_it_at_comments() {
    let b = board_with(|c| {
        c.pr.lens.on = true;
        c.pr.approvals = 1;
    });
    let id = b.pr_task();
    let mut approved = green();
    approved["approvals"] = json!(1);
    approved["review_decision"] = json!("APPROVED");
    let queued = json!({"state": "queued", "head": "h1", "at": now_iso(), "tries": 1});
    assert_eq!(b.step_with_lens(id, queued, &approved).s("pr_phase"), Some("lens"), "the last read, before lens ran");
    let mut posted = green_record();
    posted.approvals = 1;
    posted.review_decision = "APPROVED".into();
    posted.viewer = "morgan".into();
    posted.threads = vec![Thread {
        id: "th1".into(),
        kind: "review".into(),
        resolvable: true,
        author: "morgan".into(),
        last_author: "morgan".into(),
        last_at: now_iso(),
        text: "Off by one".into(),
        body: "Off by one.\n\n<!-- lens:finding key=abc -->".into(),
        ..Default::default()
    }];
    let host = FakeHost::new("github", posted);
    prhost::install(&b.app, host.clone());
    lens_pr::run(&b.app, id).unwrap();
    assert_eq!(b.lens(id)["state"], "done");
    assert_eq!(*host.reads.lock(), 1, "read fresh once lens posted");
    assert_eq!(b.task(id).s("pr_phase"), Some("comments"), "its findings hold it, not the read before lens ran");
    let visits: Vec<Value> = b.visits(id).iter().map(|v| v["visit"].clone()).collect();
    assert_eq!(visits, [json!("comments")], "never woken to merge");
}

#[test]
fn when_the_host_can_t_be_read_after_lens_posts_a_late_read_steps_it() {
    let b = lens_on();
    let id = b.pr_task();
    let queued = json!({"state": "queued", "head": "h1", "at": now_iso(), "tries": 1});
    b.step_with_lens(id, queued, &green());
    let host = FakeHost::new("github", green_record());
    *host.read_error.lock() = Some("the host is down".into());
    prhost::install(&b.app, host);
    lens_pr::run(&b.app, id).unwrap();
    assert_eq!(b.lens(id)["state"], "done");
    assert_eq!(b.task(id).s("pr_phase"), Some("lens"), "not stepped with the read before lens ran");
    assert!(jloads_obj(b.task(id).s("pr_flow")).get("reread_at").is_some_and(|r| r.is_string()), "a late read is due");
}

#[test]
fn the_merge_visit_says_approved_only_when_it_was() {
    let b = board_with(|c| {
        c.pr.lens.on = true;
        c.pr.agents_merge = Some(true);
        c.pr.projects.insert("webapp".into(), taskboardd::config::PrProject { review: Some(false), ..Default::default() });
    });
    let id = b.pr_task();
    let done = json!({"state": "done", "head": "h1", "at": now_iso()});
    assert_eq!(b.step_with_lens(id, done.clone(), &green()).s("pr_phase"), Some("merge"));
    let prompt = b.visits(id).last().unwrap()["prompt"].as_str().unwrap().to_string();
    assert!(prompt.contains("Every check is green and nothing holds it."), "{prompt}");
    assert!(!prompt.contains("approved"), "the review is off: {prompt}");

    let b = board_with(|c| {
        c.pr.lens.on = true;
        c.pr.agents_merge = Some(true);
    });
    let id = b.pr_task();
    let mut approved = green();
    approved["review_decision"] = json!("APPROVED");
    approved["approvals"] = json!(1);
    assert_eq!(b.step_with_lens(id, done, &approved).s("pr_phase"), Some("merge"));
    let prompt = b.visits(id).last().unwrap()["prompt"].as_str().unwrap().to_string();
    assert!(prompt.contains("It's approved and every check is green."), "{prompt}");
}

const MERGE_STEP: &str = r#"
[[steps]]
name = "Staging check"
before = "merge"
prompt = "Check it on staging."
"#;

#[test]
fn the_steps_visit_says_approved_only_when_it_was() {
    let done = json!({"state": "done", "head": "h1", "at": now_iso()});
    let say = |review: bool, approved: bool| {
        let b = board_with(|c| {
            c.pr.lens.on = true;
            c.pr.agents_merge = Some(true);
            c.pr.projects.insert("webapp".into(), taskboardd::config::PrProject { review: Some(review), ..Default::default() });
        });
        std::fs::write(&b.app.cfg.config_path, MERGE_STEP).unwrap();
        let id = b.pr_task();
        let mut rec = green();
        if approved {
            rec["review_decision"] = json!("APPROVED");
            rec["approvals"] = json!(1);
        }
        assert_eq!(b.step_with_lens(id, done.clone(), &rec).s("pr_phase"), Some("steps"));
        b.visits(id).last().unwrap()["prompt"].as_str().unwrap().to_string()
    };
    let off = say(false, false);
    assert!(off.contains("Every check is green, but its merge waits"), "{off}");
    let on = say(true, true);
    assert!(on.contains("It's approved and every check is green, but its merge waits"), "{on}");
}

#[test]
fn lens_finishing_steps_with_the_last_read_when_there_is_nothing_fresh_to_read() {
    // A host the board doesn't watch is never read, so lens finishing steps the stored record.
    let b = board_with(|c| c.pr.lens.on = true);
    let id = b.pr_task();
    let queued = json!({"state": "queued", "head": "h1", "at": now_iso(), "tries": 1});
    b.step_with_lens(id, queued, &green());
    let host = FakeHost::new("github", green_record());
    prhost::install(&b.app, host.clone());
    b.app.db.x("UPDATE tasks SET pr_host = 'unwatched.example' WHERE id = ?", taskboardd::p![id]).unwrap();
    lens_pr::run(&b.app, id).unwrap();
    assert_eq!(*host.reads.lock(), 0, "an unwatched host is never read");
    assert_ne!(b.task(id).s("pr_phase"), Some("lens"), "stepped with the last read");

    // No stored read yet: the PR's first read steps it, so lens finishing reads nothing.
    let b = lens_on();
    let id = b.pr_task();
    let host = FakeHost::new("github", green_record());
    prhost::install(&b.app, host.clone());
    prflow::merge_flow(&b.app, id, vec![("lens", json!({"state": "queued", "head": "h1", "at": now_iso(), "tries": 1}))]).unwrap();
    b.app.db.x("UPDATE tasks SET pr_flow = json_remove(pr_flow, '$.rec') WHERE id = ?", taskboardd::p![id]).unwrap();
    lens_pr::run(&b.app, id).unwrap();
    assert_eq!(*host.reads.lock(), 0, "no record to re-read");
}
