//! A merge refused because checks are still running on the host isn't a stall: the board watches the
//! build and wakes for the merge once it finishes (GitHub #187). One test in this file: it moves the
//! board's clock.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, Check, FakeHost, Record};
use taskboardd::util::{advance_clock, reset_clock, RowExt};
use taskboardd::{board, dispatch, midna, p, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    cfg.pr.wake = true;
    cfg.pr.agents_merge = Some(true);
    cfg.pr.no_checks_after_mins = 0.0;
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| e.message)
    }
    fn report(&self, event: &str, session: &str, extra: Value) {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap();
    }
    fn pr_task(&self, num: i64) -> i64 {
        let id = self.post("/tasks", json!({"title": format!("Ship {num}"), "detail": "Do it.", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
        let sid = format!("s{num}");
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[])
            .unwrap();
        self.report("tb.take", &sid, json!({"task": format!("T{id}")}));
        self.report("tb.done", &sid, json!({"summary": "Done", "pr": format!("https://bitbucket.org/acme/webapp/pull-requests/{num}")}));
        id
    }
    fn task(&self, id: i64) -> taskboardd::util::Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn flow(&self, id: i64) -> Value {
        serde_json::from_str(self.task(id).s("pr_flow").unwrap_or("{}")).unwrap()
    }
    fn wakes(&self, id: i64) -> i64 {
        self.app.db.count("SELECT COUNT(*) FROM jobs WHERE purpose = 'pr' AND task_id = ?", p![id]).unwrap()
    }
    fn step(&self, id: i64, rec: &Value) {
        self.app.db.tx(|| prflow::step(&self.app, &self.task(id), rec)).unwrap();
    }
    fn visit_done(&self, id: i64) {
        self.app.db.x("UPDATE jobs SET state = 'done' WHERE purpose = 'pr' AND task_id = ?", p![id]).unwrap();
        self.post(&format!("tasks/T{id}/pr/wait"), json!({})).unwrap();
    }
    fn log(&self, id: i64) -> Vec<String> {
        self.app.db.q("SELECT text FROM events WHERE task_id = ?", p![id]).unwrap().iter().filter_map(|r| r.s("text").map(|s| s.to_string())).collect()
    }
}

fn record(head: &str, checks: &[(&str, &str)]) -> Record {
    Record {
        state: "OPEN".into(),
        author: "me".into(),
        head: head.into(),
        base: "main".into(),
        checks: checks.iter().map(|(n, s)| Check { name: n.to_string(), state: s.to_string(), url: None, at: None }).collect(),
        review_decision: "APPROVED".into(),
        approvals: 2,
        ..Default::default()
    }
}

fn rec(head: &str, checks: &[(&str, &str)]) -> Value {
    record(head, checks).to_value()
}

const HEAD: &str = "abcdef1234567890";

#[test]
fn a_merge_refused_for_running_checks_watches_the_build() {
    reset_clock();
    let b = board();
    let host = FakeHost::new("bitbucket", record(HEAD, &[("build", "running"), ("e2e", "running")]));
    prhost::install(&b.app, host.clone());

    // tb pr merge sees the running checks, refuses and notes them; the feed's record has no builds.
    let id = b.pr_task(9);
    let e = b.post(&format!("tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains("checks are still running: build, e2e"), "{e}");
    let noted = &b.flow(id)["checks_running"];
    assert_eq!(noted["head"], "abcdef123456");
    assert_eq!(noted["checks"], json!(["build", "e2e"]));
    assert!(b.log(id).iter().any(|l| l == "PR #9 can't merge yet: build, e2e still running on Bitbucket, so the board watches the build"));
    let before = b.wakes(id);
    for wait in [61.0, 301.0, 901.0] {
        b.step(id, &rec(HEAD, &[]));
        assert_eq!(b.task(id).s("pr_phase"), Some("checks"), "held while the build runs");
        advance_clock(wait);
    }
    b.step(id, &rec(HEAD, &[("build", "passed")]));
    assert_eq!(b.task(id).s("pr_phase"), Some("checks"), "e2e is still running");
    assert_eq!(b.wakes(id), before, "no merge wake while the build runs");
    assert!(!dispatch::alerts(&b.app).iter().any(|a| a["text"].as_str().unwrap().contains("still open after")));
    b.step(id, &rec(HEAD, &[("build", "passed"), ("e2e", "passed")]));
    assert_eq!(b.task(id).s("pr_phase"), Some("merge"));
    assert_eq!(b.wakes(id), before + 1, "the merge wakes once the build finishes");
    reset_clock();

    // A failed check is no running build: nothing is noted.
    let failed = b.pr_task(10);
    *host.rec.lock() = record(HEAD, &[("build", "failed")]);
    b.post(&format!("tasks/T{failed}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(b.flow(failed).get("checks_running").is_none());

    // A running check the feed never reports is re-checked with a fresh merge after 30 minutes.
    let silent = b.pr_task(11);
    b.step(silent, &rec(HEAD, &[]));
    b.post(&format!("tasks/T{silent}/pr/checks-running"), json!({"head": HEAD, "checks": ["e2e"]})).unwrap();
    assert_eq!(b.task(silent).s("pr_phase"), Some("checks"));
    b.visit_done(silent);
    let before = b.wakes(silent);
    advance_clock(1801.0);
    b.step(silent, &rec(HEAD, &[]));
    assert_eq!(b.task(silent).s("pr_phase"), Some("merge"));
    assert_eq!(b.wakes(silent), before + 1);
    assert!(!b.log(silent).iter().any(|l| l.contains("try 1 of")), "a fresh merge, not a retry");
    reset_clock();

    // A running check noted on an older head doesn't hold the merge.
    let moved = b.pr_task(12);
    b.step(moved, &rec("h2", &[]));
    let v = b.post(&format!("tasks/T{moved}/pr/checks-running"), json!({"head": HEAD, "checks": ["e2e"]})).unwrap();
    assert_eq!(v["noted"], false);
    b.step(moved, &rec("h2", &[]));
    assert_eq!(b.task(moved).s("pr_phase"), Some("merge"));

    // A build change after the merge gave up brings it back with fresh tries.
    let gave_up = b.pr_task(13);
    let green = rec(HEAD, &[("build", "passed")]);
    b.step(gave_up, &green);
    for wait in [61.0, 301.0, 901.0] {
        b.visit_done(gave_up);
        advance_clock(wait);
        b.step(gave_up, &green);
    }
    b.visit_done(gave_up);
    advance_clock(901.0);
    b.step(gave_up, &green);
    assert_eq!(b.wakes(gave_up), 4);
    assert!(prflow::wake_stuck(&b.task(gave_up)), "it gave up");
    b.step(gave_up, &rec(HEAD, &[("build", "passed"), ("lint", "passed")]));
    assert_eq!(b.wakes(gave_up), 5, "the build change wakes the merge again");
    assert!(b.flow(gave_up).get("merge_tries").is_none(), "with fresh tries");
    reset_clock();
}
