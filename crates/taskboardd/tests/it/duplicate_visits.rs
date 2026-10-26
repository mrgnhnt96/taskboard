//! No PR visit starts while another one is live for the task: its reopen is pending, or the terminal it
//! went to is still working on it.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, fields, midna, p, prflow, reports, waitsfor};

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
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    fn report(&self, event: &str, session: &str, extra: Value) {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message));
    }
    fn add_session(&self, sid: &str, state: &str) {
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo, "status": {"state": state}})], &[])
            .unwrap();
    }
    fn pr_task(&self) -> i64 {
        let body = json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"});
        let id = api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &body).unwrap()["id"].as_i64().unwrap();
        self.add_session("s1", "working");
        self.report("tb.take", "s1", json!({"task": format!("T{id}")}));
        self.report("tb.done", "s1", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9"}));
        id
    }
    fn t(&self, id: i64) -> taskboardd::util::Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn visits(&self) -> i64 {
        self.app.db.count("SELECT COUNT(*) FROM jobs WHERE purpose = 'pr'", p![]).unwrap()
    }
    fn step(&self, id: i64, rec: &Value) {
        self.app.db.tx(|| prflow::step(&self.app, &self.t(id), rec)).unwrap();
    }
    /// The reopen ran: its terminal (`sid`) is open, in `state`.
    fn opened(&self, sid: &str, state: &str) {
        self.add_session(sid, state);
        self.app
            .db
            .x("UPDATE jobs SET state = 'done', target = json_set(coalesce(target, '{}'), '$.session', ?) WHERE purpose = 'pr'", p![sid])
            .unwrap();
    }
}

fn failing(head: &str) -> Value {
    json!({"state": "OPEN", "head": head, "checks": [{"name": "ci", "state": "failed"}], "failed": ["ci"], "running": 0,
           "comments": 0, "approvals": 0, "review_decision": ""})
}

fn approved() -> Value {
    json!({"state": "OPEN", "head": "h1", "checks": [{"name": "ci", "state": "passed"}], "failed": [], "running": 0,
           "comments": 0, "approvals": 1, "review_decision": "APPROVED"})
}

#[test]
fn a_new_failure_waits_for_the_visit_still_at_work() {
    let b = board();
    let id = b.pr_task();
    b.add_session("s1", "idle");
    b.step(id, &failing("h1"));
    assert_eq!(b.t(id).s("pr_phase"), Some("fix"));
    assert_eq!(b.visits(), 1);
    b.opened("s9", "working");
    b.step(id, &failing("h2"));
    assert_eq!(b.visits(), 1, "the fix visit is still at work in its terminal");
    b.app.db.x("UPDATE sessions SET status = 'idle' WHERE id = 's9'", p![]).unwrap();
    b.step(id, &failing("h3"));
    assert_eq!(b.visits(), 2, "it went idle without finishing: bring it back for the new failure");
}

#[test]
fn a_pending_reopen_blocks_every_other_visit() {
    let b = board();
    let id = b.pr_task();
    b.add_session("s1", "idle");
    b.step(id, &failing("h1"));
    assert_eq!(b.visits(), 1);
    let t = b.t(id);
    assert_eq!(b.app.db.tx(|| prflow::follow_wake(&b.app, &t, "Rebase onto main.")).unwrap(), None);
    assert_eq!(b.app.db.tx(|| prflow::answer_in_new_tab(&b.app, &t, "Yes.")).unwrap(), None);
    assert_eq!(b.app.db.tx(|| prflow::wake(&b.app, &t, "fix", &failing("h1"), None)).unwrap(), None);
    assert_eq!(b.visits(), 1);
}

#[test]
fn a_follow_visit_at_work_holds_off_the_flow() {
    let b = board();
    let id = b.pr_task();
    b.add_session("s1", "idle");
    let t = b.t(id);
    assert!(b.app.db.tx(|| prflow::follow_wake(&b.app, &t, "Rebase onto main.")).unwrap().is_some());
    b.opened("s9", "working");
    b.step(id, &failing("h1"));
    assert_eq!(b.visits(), 1, "the rebase visit is still at work");
}

#[test]
fn a_merged_pr_isnt_visited_again_on_a_stale_open_record() {
    let b = board();
    let id = b.pr_task();
    b.add_session("s1", "idle");
    b.step(id, &approved());
    assert_eq!(b.t(id).s("pr_phase"), Some("merge"));
    assert_eq!(b.visits(), 1);
    b.opened("s9", "idle");
    b.app.db.tx(|| prflow::mark_merged(&b.app, &b.t(id), "agent")).unwrap();
    // A stream event laid over the record from before the merge still reads open.
    let mut stale = approved();
    stale["checks"] = json!([{"name": "ci", "state": "passed"}, {"name": "deploy", "state": "passed"}]);
    b.step(id, &stale);
    assert_eq!(b.t(id).s("pr_phase"), Some("merged"));
    assert_eq!(b.visits(), 1);
}

#[test]
fn quiet_pr_visits_open_in_the_background() {
    let b = board();
    let id = b.pr_task();
    b.add_session("s1", "idle");
    let background = |phase: &str, rec: &Value| {
        let t = b.t(id);
        let jid = b.app.db.tx(|| prflow::wake(&b.app, &t, phase, rec, None)).unwrap().expect("a visit");
        let j = b.app.db.q1("SELECT * FROM jobs WHERE id = ?", p![jid]).unwrap().unwrap();
        b.app.db.x("DELETE FROM jobs WHERE id = ?", p![jid]).unwrap();
        midna::opens_in_background(&b.app, &j)
    };
    assert!(background("merge", &approved()));
    assert!(background("rebase", &approved()));
    assert!(background("steps", &approved()));
    assert!(!background("fix", &failing("h1")), "a fix is work the owner may want to watch");
    assert!(!background("comments", &approved()), "so is answering a reviewer");
}

#[test]
fn a_follow_up_that_cant_wake_yet_is_sent_once_it_can() {
    let b = board();
    let id = b.pr_task();
    b.add_session("s1", "idle");
    let base = api::dispatch(&b.app, "POST", "/tasks", &Query::new(), &json!({"title": "Base", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    b.app.db.x("UPDATE tasks SET status = 'done', pr_flow = '{\"head\": \"h1\"}' WHERE id = ?", p![base]).unwrap();
    b.app.db.x("UPDATE tasks SET waits_for = ? WHERE id = ?", p![json!([base]).to_string(), id]).unwrap();
    let follow = || b.app.db.tx(|| waitsfor::follow_ups(&b.app)).unwrap();
    assert_eq!(follow(), 0, "the first look only notes where the base is");
    b.app.db.x("UPDATE tasks SET pr_flow = '{\"head\": \"h2\"}' WHERE id = ?", p![base]).unwrap();
    // A visit still under way: the follow-up can't come in yet, so it isn't marked as told.
    b.app.db.insert("jobs", fields!["kind" => "agent", "purpose" => "pr", "task_id" => id, "state" => "pending", "args" => "{}"]).unwrap();
    assert_eq!(follow(), 0);
    assert_eq!(follow(), 0);
    b.app.db.x("UPDATE jobs SET state = 'done' WHERE purpose = 'pr'", p![]).unwrap();
    assert_eq!(follow(), 1, "the base's move still reaches it once the visit ends");
    assert_eq!(b.visits(), 2);
    assert_eq!(follow(), 0, "and only once");
}
