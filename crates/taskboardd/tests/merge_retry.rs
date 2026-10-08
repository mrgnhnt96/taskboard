//! A merge the agent finished that's still open is brought back at 1, 5 and 15 minutes, then alerts once
//! (`docs/parity/oct8-changes.md` #3). One test in this file: it moves the board's clock.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{advance_clock, reset_clock, RowExt};
use taskboardd::{board, dispatch, midna, p, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    f(&mut cfg);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn report(&self, event: &str, session: &str, extra: Value) -> Value {
        self.try_report(event, session, extra).unwrap_or_else(|e| panic!("{event}: {e}"))
    }
    fn try_report(&self, event: &str, session: &str, extra: Value) -> Result<Value, String> {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| e.message)
    }
    fn repo(&self) -> String {
        self.dir.path().join("webapp").to_string_lossy().to_string()
    }
    fn add_session(&self, sid: &str) {
        midna::sync(
            &self.app,
            &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": self.repo(), "status": {"state": "working"}})],
            &[],
        )
        .unwrap();
    }
    fn task(&self, title: &str, extra: Value) -> i64 {
        let mut body = json!({"title": title, "detail": "Do it.", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", body)["id"].as_i64().unwrap()
    }
}

fn pr_task(b: &Board) -> i64 {
    let id = b.task("Ship it", json!({}));
    b.add_session("s1");
    b.report("tb.take", "s1", json!({"task": format!("T{id}")}));
    b.report("tb.done", "s1", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9"}));
    id
}

fn rec(decision: &str, comments: i64, changes_at: &str) -> Value {
    json!({"state": "OPEN", "head": "h1", "checks": [{"name": "ci", "state": "passed"}], "failed": [], "running": 0,
           "comments": comments, "approvals": if decision == "APPROVED" { 1 } else { 0 }, "review_decision": decision, "changes_at": changes_at})
}

#[test]
fn a_merge_left_open_is_retried_then_alerts_once() {
    reset_clock();
    let b = board_with(|c| {
        c.pr.watch = true;
        c.pr.wake = true;
        c.pr.agents_merge = true;
    });
    let id = pr_task(&b);
    let t = || board::get_task(&b.app, id).unwrap();
    let wakes = || b.app.db.count("SELECT COUNT(*) FROM jobs WHERE purpose = 'pr'", p![]).unwrap();
    let step = || b.app.db.tx(|| prflow::step(&b.app, &t(), &rec("APPROVED", 0, ""))).unwrap();
    step();
    assert_eq!(t().s("pr_phase"), Some("merge"));
    assert_eq!(wakes(), 1);
    for (n, wait) in [60.0, 300.0, 900.0].iter().enumerate() {
        b.app.db.x("UPDATE jobs SET state = 'done' WHERE purpose = 'pr'", p![]).unwrap();
        b.post(&format!("tasks/T{id}/pr/wait"), json!({}));
        step();
        assert_eq!(wakes(), n as i64 + 1, "not before its wait");
        advance_clock(*wait + 1.0);
        step();
        assert_eq!(wakes(), n as i64 + 2, "try {}", n + 1);
    }
    b.app.db.x("UPDATE jobs SET state = 'done' WHERE purpose = 'pr'", p![]).unwrap();
    b.post(&format!("tasks/T{id}/pr/wait"), json!({}));
    advance_clock(901.0);
    step();
    step();
    assert_eq!(wakes(), 4, "three retries, then no more");
    let alerts: Vec<Value> = dispatch::alerts(&b.app).into_iter().filter(|a| a["text"].as_str().unwrap().contains("still open after 3 tries")).collect();
    assert_eq!(alerts.len(), 1);
    assert!(prflow::wake_stuck(&t()), "the alert holds until it merges");
    reset_clock();
}
