//! #1162: while a healthy stream relay covers a PR's repo, the board's copy of the PR is what an
//! agent's `tb pr status`, `tb pr merge` pre-checks and `tb pr wait` go by, so they don't read the host
//! each run. The host is read when an event didn't land, when the copy is older than the reconcile
//! window, and when no relay covers the repo. A merge the copy shows held refuses without a read; one
//! it lets through reads the host once before merging.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, Check, FakeHost, Record};
use taskboardd::util::{iso, now_ts, RowExt};
use taskboardd::{board, fields, midna, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    cfg.pr.agents_merge = Some(true);
    cfg.pr.no_checks_after_mins = 0.0;
    cfg.pr_body.require_ticket = Some(false);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    Board { app, dir }
}

impl Board {
    fn call(&self, method: &str, path: &str, body: Value) -> Result<Value, String> {
        api::dispatch(&self.app, method, path, &Query::new(), &body).map_err(|e| e.message)
    }
    fn report(&self, event: &str, session: &str, extra: Value) {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap();
    }
    fn pr_task(&self, num: i64) -> i64 {
        let id = self.call("POST", "/tasks", json!({"title": format!("Ship {num}"), "detail": "Do it.", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
        let sid = format!("s{num}");
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[])
            .unwrap();
        self.report("tb.take", &sid, json!({"task": format!("T{id}")}));
        self.report("tb.done", &sid, json!({"summary": "Done", "pr": format!("https://bitbucket.org/acme/webapp/pull-requests/{num}")}));
        id
    }
    fn status(&self, id: i64) -> Value {
        let q: Query = [("full".to_string(), "1".to_string())].into_iter().collect();
        api::dispatch(&self.app, "GET", &format!("tasks/T{id}/pr"), &q, &json!({})).map_err(|e| e.message).unwrap()
    }
    fn merge(&self, id: i64) -> Result<Value, String> {
        self.call("POST", &format!("tasks/T{id}/pr/merge"), json!({"agent": true}))
    }
    fn flow(&self, id: i64) -> Value {
        serde_json::from_str(board::get_task(&self.app, id).unwrap().s("pr_flow").unwrap_or("{}")).unwrap()
    }
    fn set_flow(&self, id: i64, k: &str, v: Value) {
        prflow::merge_flow(&self.app, id, fields![k => v]).unwrap();
    }
    fn send(&self, e: Value) -> Value {
        self.call("POST", "/stream/events", json!({"v": 1, "events": [e]})).unwrap()
    }
    /// The relay fi, healthy now and covering Bitbucket.
    fn relay(&self, id: &str) {
        self.send(json!({"id": id, "kind": "heartbeat", "at": iso(now_ts()), "source": "fi", "v": 1, "sources": ["prs", "builds"], "covers": [{"provider": "bitbucket"}]}));
    }
    /// A review on PR #9: a hint, which reads the PR from its host.
    fn review(&self, id: &str) -> Value {
        let pr = json!({"provider": "bitbucket", "repo": "acme/webapp", "num": 9});
        self.send(json!({"id": id, "kind": "review", "at": iso(now_ts()), "source": "fi", "v": 1, "pr": pr, "who": "pat", "state": "approved"}))
    }
}

fn record(checks: &[(&str, &str)]) -> Record {
    Record {
        state: "OPEN".into(),
        author: "me".into(),
        head: "abcdef1234567890".into(),
        base: "main".into(),
        branch: "login".into(),
        checks: checks.iter().map(|(n, s)| Check { name: n.to_string(), state: s.to_string(), url: None, at: None }).collect(),
        review_decision: "APPROVED".into(),
        approvals: 2,
        ..Default::default()
    }
}

fn ago(secs: f64) -> Value {
    json!(iso(now_ts() - secs))
}

#[test]
fn an_agents_pr_status_answers_from_the_streamed_copy_until_the_host_must_be_read() {
    let b = board();
    let host = FakeHost::new("bitbucket", record(&[("build", "passed")]));
    prhost::install(&b.app, host.clone());
    let id = b.pr_task(9);
    b.relay("h1");
    let reads = || *host.reads.lock();

    let n = reads();
    b.status(id);
    assert_eq!(reads(), n + 1, "never read since fi connected: read once");
    for _ in 0..3 {
        b.status(id);
    }
    assert_eq!(reads(), n + 1, "fi covers it and it was just read: the board's copy answers");
    b.call("POST", &format!("tasks/T{id}/pr/wait"), json!({})).unwrap();
    assert_eq!(reads(), n + 1, "tb pr wait doesn't read the host");

    // An event the board couldn't read leaves the copy behind it: the next status reads.
    *host.read_error.lock() = Some("Bitbucket answered 500".into());
    let r = b.review("r1");
    assert!(r.to_string().contains("Bitbucket answered 500"), "{r}");
    assert!(b.flow(id).get("event_unread_at").is_some());
    *host.read_error.lock() = None;
    let n = reads();
    b.status(id);
    assert_eq!(reads(), n + 1, "an event didn't land: read");
    assert!(b.flow(id).get("event_unread_at").is_none(), "the read caught up");
    b.status(id);
    assert_eq!(reads(), n + 1);

    // A copy older than the reconcile window is read again.
    b.set_flow(id, "read_at", ago(4000.0));
    b.status(id);
    assert_eq!(reads(), n + 2, "an hour on: read");
    b.status(id);
    assert_eq!(reads(), n + 2);

    // A hint's settle read that's due reads too.
    b.set_flow(id, "reread_at", ago(1.0));
    b.status(id);
    assert_eq!(reads(), n + 3, "a settle read is due");

    // fi goes stuck: nothing covers the repo, so every status reads.
    let mut st: Value = serde_json::from_str(&b.app.db.get_setting("pr_feed").unwrap().unwrap()).unwrap();
    st["relays"]["fi"]["last_heartbeat_at"] = ago(200.0);
    b.app.db.set_setting("pr_feed", Some(&st.to_string())).unwrap();
    b.status(id);
    b.status(id);
    assert_eq!(reads(), n + 5, "no healthy relay covers it");
}

#[test]
fn an_agents_merge_judges_the_boards_copy_before_reading_the_host() {
    let b = board();
    let host = FakeHost::new("bitbucket", record(&[("build", "running")]));
    prhost::install(&b.app, host.clone());
    let reads = || *host.reads.lock();

    // No relay: a copy checked just now with a check running refuses without a read.
    let id = b.pr_task(9);
    b.status(id);
    let n = reads();
    let e = b.merge(id).unwrap_err();
    assert!(e.starts_with("PR #9 can't merge yet: checks still running: tb pr wait."), "{e}");
    assert!(e.contains("checks are still running: build"), "{e}");
    assert!(e.ends_with("run tb pr wait and stop here."), "{e}");
    assert_eq!(reads(), n, "refused on the board's copy");
    assert_eq!(b.flow(id)["checks_running"]["checks"], json!(["build"]));
    assert_eq!(b.flow(id)["merge_refused"]["why"], "checks are still running: build");

    // The same copy checked over an hour ago is read first.
    b.set_flow(id, "checked_at", ago(4000.0));
    let e = b.merge(id).unwrap_err();
    assert!(e.starts_with("PR #9 can't merge yet: checks are still running: build."), "{e}");
    assert_eq!(reads(), n + 1, "an old copy: read");

    // Covered by fi, the copy answers for every hold, not only running checks.
    b.relay("h1");
    *host.rec.lock() = Record { approvals: 0, review_decision: String::new(), ..record(&[("build", "passed")]) };
    b.review("r1");
    let n = reads();
    let e = b.merge(id).unwrap_err();
    assert!(e.contains("approv"), "{e}");
    assert!(!e.contains("tb pr wait"), "{e}");
    assert_eq!(reads(), n, "fi keeps the copy: refused without a read");

    // Nothing holds it on the copy: one read before merging.
    *host.rec.lock() = record(&[("build", "passed")]);
    b.review("r2");
    let n = reads();
    let v = b.merge(id).unwrap();
    assert_eq!(v["merged"], true, "{v}");
    assert_eq!(reads(), n + 1, "merging reads the host once");
}
