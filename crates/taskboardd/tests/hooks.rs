//! The owner's hooks at each step of the flow: announced after a step, or run before a stoppable one,
//! which they can stop or (for PR checks) skip.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, fields, hooks, midna, p, prflow, reports, runner};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

/// A board whose hooks.json is `file` (with `RECORD` replaced by a command that saves each input).
fn board(file: Value) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    cfg.pr.wake = true;
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let seen = dir.path().join("seen.jsonl");
    let record = format!("cat >> {} && echo >> {}", seen.display(), seen.display());
    std::fs::write(hooks::path(&cfg), file.to_string().replace("RECORD", &record)).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    Board { app, dir }
}

fn cmd(c: &str) -> Value {
    json!([{"hooks": [{"type": "command", "command": c}]}])
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": ""});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn task(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn new_task(&self, extra: Value) -> i64 {
        let mut body = json!({"title": "Add login", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", body).unwrap()["id"].as_i64().unwrap()
    }
    fn seen(&self) -> Vec<Value> {
        let p: PathBuf = self.dir.path().join("seen.jsonl");
        std::fs::read_to_string(p).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect()
    }
    fn events(&self) -> Vec<String> {
        self.seen().iter().map(|r| r["event"].as_str().unwrap().to_string()).collect()
    }
    fn history(&self, id: i64) -> Vec<String> {
        self.app.db.q("SELECT text FROM events WHERE task_id = ? AND kind = 'hook' ORDER BY id", p![id]).unwrap().iter().map(|r| r.st("text")).collect()
    }
    fn alerts(&self) -> Vec<String> {
        taskboardd::dispatch::alerts(&self.app).iter().map(|a| a["text"].as_str().unwrap().to_string()).collect()
    }
    /// Takes the task in s1 and finishes it with a PR.
    fn done_with_pr(&self, id: i64) {
        self.report("tb.take", json!({"task": rf(id)})).unwrap();
        self.report("tb.done", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9"})).unwrap();
        assert_eq!(self.task(id).s("pr_phase"), Some("checks"));
    }
    /// What `prflow::refresh` does with one read of the PR.
    fn refresh(&self, id: i64, rec: &Value) {
        prflow::gate(&self.app, &self.task(id), rec).unwrap();
        self.app.db.tx(|| prflow::step(&self.app, &self.task(id), rec)).unwrap();
    }
    fn wakes(&self) -> i64 {
        self.app.db.count("SELECT COUNT(*) FROM jobs WHERE kind = 'agent' AND purpose = 'pr'", p![]).unwrap()
    }
}

fn rf(id: i64) -> String {
    format!("T{id}")
}

fn rec(head: &str, failed: &[&str], decision: &str) -> Value {
    let checks: Vec<Value> = ["ci"].iter().map(|n| json!({"name": n, "state": if failed.contains(n) { "failed" } else { "passed" }})).collect();
    json!({"state": "OPEN", "head": head, "checks": checks, "failed": failed, "running": 0, "comments": 0, "approvals": 0, "review_decision": decision})
}

#[test]
fn hooks_announce_steps_for_matching_projects() {
    let b = board(json!({"hooks": {
        "task.created": cmd("RECORD"),
        "task.working": [{"matcher": "webapp", "hooks": [{"type": "command", "command": "RECORD"}]}],
        "task.done": [{"matcher": "other", "hooks": [{"type": "command", "command": "RECORD"}]}],
        "pr.opened": [{"matcher": "web.*", "hooks": [{"type": "command", "command": "RECORD"},
                                                      {"type": "command", "command": "echo nope >&2; exit 3"}]}],
    }}));
    let id = b.new_task(json!({}));
    b.done_with_pr(id);

    let runs = b.seen();
    assert_eq!(b.events(), vec!["task.created", "task.working", "pr.opened"], "task.done only runs for project `other`");
    assert_eq!(runs[1]["from"]["status"], "queued");
    assert_eq!(runs[2]["task"]["pr"]["num"], 9);
    assert_eq!(runs[2]["task"]["ref"], "T1");
    assert_eq!(runs[2]["can"], json!({"block": false, "skip": false}));
    assert_eq!(b.history(id).len(), 1, "a failing hook is noted on the task");
    let log = std::fs::read_to_string(b.dir.path().join("hooks.log")).unwrap();
    assert_eq!(log.lines().count(), 4);
    assert!(log.contains("nope"));
}

#[test]
fn a_hook_can_stop_a_task_from_finishing() {
    let b = board(json!({"hooks": {
        "task.finishing": cmd("RECORD; echo 'the tests are red' >&2; exit 2"),
        "task.done": cmd("RECORD"),
    }}));
    let id = b.new_task(json!({"ships_pr": false}));
    b.report("tb.take", json!({"task": "T1"})).unwrap();
    let (code, why) = b.report("tb.done", json!({"summary": "All good"})).unwrap_err();
    assert_eq!(code, 409);
    assert!(why.contains("Stopped from finishing by hook"), "{why}");
    assert!(why.contains("the tests are red"), "{why}");
    assert_eq!(b.task(id).s("status"), Some("working"));
    assert!(b.history(id)[0].contains("the tests are red"));
    assert_eq!(b.events(), vec!["task.finishing"]);
    assert_eq!(b.seen()[0]["done"]["summary"], "All good");
    assert_eq!(b.seen()[0]["can"]["block"], true);

    let (code, _) = b.post("/tasks/T1/done", json!({"summary": "Done by hand"})).unwrap_err();
    assert_eq!(code, 409, "the owner's Done goes through the same hooks");
    b.post("/tasks/T1/fail", json!({"reason": "Giving up"})).unwrap();
    assert_eq!(b.task(id).s("status"), Some("done"), "failing isn't a finish a hook can stop");
}

#[test]
fn a_hook_can_stop_a_task_from_starting() {
    let b = board(json!({"hooks": {"task.starting": cmd(r#"echo '{"decision": "block", "reason": "the build farm is down"}'"#)}}));
    let id = b.new_task(json!({}));
    assert!(runner::start_queued(&b.app).unwrap().is_empty());
    let t = b.task(id);
    assert_eq!(t.s("status"), Some("needs"));
    assert!(t.st("latest").contains("the build farm is down"));
    assert!(b.alerts().iter().any(|a| a.contains("the build farm is down")), "{:?}", b.alerts());
    let (code, why) = b.post("/tasks/T1/start", json!({"mode": "new"})).unwrap_err();
    assert_eq!(code, 409);
    assert!(why.contains("the build farm is down"));
}

#[test]
fn a_hook_can_skip_the_checks_it_cancelled() {
    let b = board(json!({"hooks": {
        "pr.checks": cmd(r#"RECORD; echo '{"decision": "skip", "reason": "cancelled the builds"}'"#),
        "pr.fix": cmd(r#"RECORD; echo '{"decision": "skip", "reason": "cancelled the builds"}'"#),
        "pr.review": cmd("RECORD"),
    }}));
    let id = b.new_task(json!({}));
    b.done_with_pr(id);

    // The cancelled builds show up as failed checks: the hook skips them, so the PR goes to review.
    b.refresh(id, &rec("h1", &["ci"], "REVIEW_REQUIRED"));
    let t = b.task(id);
    assert_eq!(t.s("pr_phase"), Some("review"));
    assert_eq!(t.s("pr_build"), Some("Checks skipped"));
    assert_eq!(b.wakes(), 0, "the agent isn't brought back to fix cancelled builds");
    assert_eq!(b.events(), vec!["pr.fix", "pr.review"]);
    assert_eq!(b.seen()[0]["failed_checks"], json!(["ci"]));
    assert!(b.history(id)[0].contains("cancelled the builds"));

    b.refresh(id, &rec("h1", &["ci"], "REVIEW_REQUIRED"));
    assert_eq!(b.events().len(), 2, "the same push isn't asked about twice");

    // A new push starts the checks again; the hook runs (and skips) once more.
    let mut running = rec("h2", &[], "REVIEW_REQUIRED");
    running["running"] = json!(1);
    b.refresh(id, &running);
    assert_eq!(b.task(id).s("pr_phase"), Some("review"));
    assert_eq!(b.events(), vec!["pr.fix", "pr.review", "pr.checks", "pr.review"]);
}

#[test]
fn a_hook_can_skip_a_task_so_it_never_runs() {
    let b = board(json!({"hooks": {"task.starting": cmd(r#"echo '{"decision": "skip", "reason": "APP-41 is already fixed"}'"#), "task.done": cmd("RECORD")}}));
    let id = b.new_task(json!({}));
    assert!(runner::start_queued(&b.app).unwrap().is_empty());
    let t = b.task(id);
    assert_eq!(t.s("status"), Some("done"));
    assert!(!t.b("failed"));
    assert!(t.st("summary").contains("Skipped by hook") && t.st("summary").contains("APP-41 is already fixed"), "{}", t.st("summary"));
    assert_eq!(b.events(), vec!["task.done"]);
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM jobs WHERE kind = 'agent'", p![]).unwrap(), 0, "no terminal was opened");
}

#[test]
fn a_hook_can_skip_review_and_comments() {
    let b = board(json!({"hooks": {
        "pr.comments": cmd(r#"echo '{"decision": "skip", "reason": "bot comments"}'"#),
        "pr.review": cmd(r#"echo '{"decision": "skip", "reason": "no review needed here"}'"#),
        "pr.merge": cmd("RECORD"),
    }}));
    let id = b.new_task(json!({}));
    b.done_with_pr(id);
    let mut r = rec("h1", &[], "");
    r["comments"] = json!(2);
    b.refresh(id, &r);
    assert_eq!(b.task(id).s("pr_phase"), Some("merge"), "comments answered, review approved: ready to merge");
    assert_eq!(b.wakes(), 0);
    assert_eq!(b.events(), vec!["pr.merge"]);
    let h = b.history(id);
    assert!(h.iter().any(|l| l.contains("“Addressing comments” skipped by hook") && l.contains("bot comments")), "{h:?}");
    assert!(h.iter().any(|l| l.contains("“Awaiting review” skipped by hook")), "{h:?}");
}

#[test]
fn a_skip_a_step_cant_take_is_noted_and_ignored() {
    let b = board(json!({"hooks": {"task.finishing": cmd(r#"echo '{"decision": "skip", "reason": "why not"}'"#)}}));
    let id = b.new_task(json!({"ships_pr": false}));
    b.report("tb.take", json!({"task": "T1"})).unwrap();
    b.report("tb.done", json!({"summary": "Done"})).unwrap();
    assert_eq!(b.task(id).s("status"), Some("done"));
    assert!(b.history(id).iter().any(|l| l.contains("can't be skipped: finishing is the last step")), "{:?}", b.history(id));
}

#[test]
fn a_hook_can_stop_the_agent_being_brought_back() {
    let b = board(json!({"hooks": {"pr.fix": cmd("echo 'CI is flaky today; I will look' >&2; exit 2")}}));
    let id = b.new_task(json!({}));
    b.done_with_pr(id);
    b.refresh(id, &rec("h1", &["ci"], ""));
    assert_eq!(b.task(id).s("pr_phase"), Some("fix"), "a stop doesn't pretend the checks passed");
    assert_eq!(b.wakes(), 0);
    assert!(b.alerts().iter().any(|a| a.contains("stopped at “Fixing checks”") && a.contains("CI is flaky today")), "{:?}", b.alerts());
}

#[test]
fn tb_pr_skip_checks_moves_the_pr_on_but_never_past_a_failure() {
    let b = board(json!({"hooks": {}}));
    let id = b.new_task(json!({}));
    b.done_with_pr(id);
    b.app.db.tx(|| prflow::step(&b.app, &b.task(id), &rec("h1", &["ci"], ""))).unwrap();
    assert_eq!(b.task(id).s("pr_phase"), Some("fix"));
    let (code, e) = b.post("/tasks/T1/pr/skip-checks", json!({"reason": "builds cancelled", "all": true})).unwrap_err();
    assert!(code == 409 && e.contains("not failures") && e.contains("tb pr not-ours"), "{code} {e}");
    let (code, _) = b.post("/tasks/T1/pr/skip-checks", json!({"all": true})).unwrap_err();
    assert_eq!(code, 400, "a reason is required");
    // The builds were stopped: those can be skipped.
    let mut stopped = rec("h2", &[], "");
    stopped["checks"] = json!([{"name": "ci", "state": "stopped"}]);
    b.app.db.tx(|| prflow::step(&b.app, &b.task(id), &stopped)).unwrap();
    b.post("/tasks/T1/pr/skip-checks", json!({"reason": "builds cancelled", "all": true})).unwrap();
    assert_eq!(b.task(id).s("pr_phase"), Some("review"));
    assert_eq!(b.task(id).s("pr_build"), Some("Checks skipped"));
    let bar = taskboardd::prbar::bar(&b.app, &b.task(id)).unwrap();
    assert_eq!(bar["checks"], "skipped", "skipped, not “not this PR's”");
    b.app.db.tx(|| prflow::step(&b.app, &b.task(id), &rec("h3", &[], ""))).unwrap();
    assert_eq!(b.task(id).s("pr_phase"), Some("review"), "--all skips later pushes too");
    b.app.db.tx(|| prflow::step(&b.app, &b.task(id), &rec("h4", &["ci"], ""))).unwrap();
    assert_eq!(b.task(id).s("pr_phase"), Some("fix"), "but a later push's failure still counts");
}

#[test]
fn goals_announce_created_paused_and_finished() {
    let b = board(json!({"hooks": {
        "goal.created": cmd("RECORD"), "goal.paused": cmd("RECORD"), "goal.resumed": cmd("RECORD"),
        "goal.finished": cmd("RECORD"),
    }}));
    let g = b.post("/goals", json!({"name": "Login", "project": "webapp"})).unwrap();
    let gid = g["id"].as_i64().unwrap();
    b.post(&format!("/goals/G{gid}"), json!({"paused": true})).unwrap();
    b.post(&format!("/goals/G{gid}"), json!({"paused": false})).unwrap();
    let id = b.new_task(json!({"goal_id": gid}));
    b.report("tb.take", json!({"task": "T1"})).unwrap();
    b.report("tb.done", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9"})).unwrap();
    assert_eq!(b.events(), vec!["goal.created", "goal.paused", "goal.resumed"], "not finished while its PR is open");
    board::update_task(&b.app, id, fields!["pr_state" => "MERGED", "pr_phase" => "merged"]).unwrap();
    assert_eq!(b.events().last().unwrap(), "goal.finished");
    let fin = b.seen().pop().unwrap();
    assert_eq!(fin["goal"]["ref"], format!("G{gid}"));
    assert_eq!(fin["task"]["ref"], "T1");
}
