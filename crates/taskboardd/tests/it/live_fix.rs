//! #489: the PR card says its checks are being fixed only while a terminal is on them, and says the
//! owner has to act when only they can fix the failure (`tb pr needs-owner`).

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, Check, FakeHost, Record};
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, midna, p, prflow, reports};

struct Board {
    app: Arc<App>,
    host: Arc<FakeHost>,
    repo: String,
    _dir: tempfile::TempDir,
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    cfg.pr.wake = true;
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    let host = FakeHost::new("bitbucket", rec("h1", &[]));
    prhost::install(&app, host.clone());
    let b = Board { app, host, repo: repo.to_string_lossy().to_string(), _dir: dir };
    b.terminals(&["s1"]);
    b
}

impl Board {
    fn terminals(&self, ids: &[&str]) {
        let open: Vec<Value> =
            ids.iter().map(|id| json!({"id": id, "name": format!("Term {id}"), "agent": "claude", "cwd": self.repo, "status": {"state": "working"}})).collect();
        midna::sync(&self.app, &open, &[]).unwrap();
    }
    fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| e.message)
    }
    fn report(&self, event: &str, extra: Value) {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": ""});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message));
    }
    fn task(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    /// A task done in s1 with a PR whose `ci` check fails on `h1`, and s1 picked up the board's wake to fix it.
    fn failing(&self) -> i64 {
        let id = self.post("/tasks", json!({"title": "Add login", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")}));
        self.report("tb.done", json!({"summary": "Done", "pr": "https://bitbucket.org/acme/webapp/pull-requests/9"}));
        self.push("h1", &["ci"]);
        assert_eq!(self.task(id).s("pr_phase"), Some("fix"));
        self.app.db.x("UPDATE jobs SET state = 'done' WHERE task_id = ? AND purpose = 'pr'", p![id]).unwrap();
        prflow::picked_up(&self.app, &self.task(id), "s1", "Term s1").unwrap();
        id
    }
    fn push(&self, head: &str, failed: &[&str]) {
        *self.host.rec.lock() = rec(head, failed);
        prflow::refresh(&self.app).unwrap();
    }
    fn stage(&self, id: i64) -> Value {
        board::pr_card(&self.app, &self.task(id))["stage"].clone()
    }
    fn alerts(&self) -> Vec<String> {
        taskboardd::dispatch::alerts(&self.app).iter().map(|a| a["text"].as_str().unwrap().to_string()).collect()
    }
}

fn rec(head: &str, failed: &[&str]) -> Record {
    let checks = ["ci", "lint"].iter().map(|n| Check { name: n.to_string(), state: if failed.contains(n) { "failed" } else { "passed" }.into(), url: None, at: None }).collect();
    Record { state: "OPEN".into(), title: "Add login".into(), author: "me".into(), head: head.into(), branch: "feat".into(), base: "main".into(), base_head: "b1".into(), checks, ..Default::default() }
}

#[test]
fn the_fix_shows_its_terminal_while_the_visit_is_on_and_reads_failed_once_it_ends() {
    let b = board();
    let id = b.failing();
    let s = b.stage(id);
    assert_eq!((s["label"].as_str(), s["session"].as_str()), (Some("Fixing checks"), Some("s1")), "{s}");
    assert!(s["idle"].is_null());
    // The visit ends without a fix (tb pr wait): nobody is on the checks any more.
    prflow::waited(&b.app, &b.task(id)).unwrap();
    let s = b.stage(id);
    assert_eq!(s["label"], "Checks failed", "{s}");
    assert!(s["session"].is_null(), "{s}");
    assert_eq!(s["idle"], true);
}

#[test]
fn a_closed_terminal_or_one_on_another_task_isn_t_fixing_the_checks() {
    let b = board();
    let id = b.failing();
    assert_eq!(b.stage(id)["session"], "s1");
    // s1 takes another task: it moved on.
    let other = b.post("/tasks", json!({"title": "Other", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    b.report("tb.take", json!({"task": format!("T{other}")}));
    assert!(b.stage(id)["session"].is_null(), "{}", b.stage(id));

    let b = board();
    let id = b.failing();
    b.terminals(&[]);
    b.terminals(&[]);
    assert_eq!(b.app.db.q("SELECT status FROM sessions WHERE id = 's1'", p![]).unwrap()[0].st("status"), "gone");
    assert!(b.stage(id)["session"].is_null(), "{}", b.stage(id));
}

#[test]
fn needs_owner_tells_the_owner_finishes_the_visit_and_holds_for_this_push_only() {
    let b = board();
    let id = b.failing();
    let url = format!("/tasks/T{id}/pr/needs-owner");
    assert!(b.post(&url, json!({"reason": "secret"})).unwrap_err().contains("20 to 300"));
    assert!(b.post(&url, json!({"reason": "Add the NPM_TOKEN repository secret.", "checks": ["lint"]})).unwrap_err().contains("lint didn't fail"));
    let v = b.post(&url, json!({"reason": "Add the NPM_TOKEN repository secret."})).unwrap();
    assert_eq!(v["checks"], json!(["ci"]));
    assert_eq!(v["head"], "h1");
    let f: Value = serde_json::from_str(&b.task(id).st("pr_flow")).unwrap();
    assert!(f["woke"].is_null(), "the visit is over: {f}");
    let s = b.stage(id);
    assert_eq!(s["label"], "Needs you to fix checks", "{s}");
    assert_eq!(s["owner"]["reason"], "Add the NPM_TOKEN repository secret.");
    assert!(s["session"].is_null(), "{s}");
    assert!(b.alerts().iter().any(|a| a.contains("needs you") && a.contains("NPM_TOKEN")), "{:?}", b.alerts());
    // A new push that fails again is the agent's to fix.
    b.push("h2", &["ci"]);
    let s = b.stage(id);
    assert!(s["owner"].is_null(), "{s}");
    assert_ne!(s["label"], "Needs you to fix checks");
}

#[test]
fn needs_owner_refuses_when_nothing_fails() {
    let b = board();
    let id = b.failing();
    b.push("h2", &[]);
    let e = b.post(&format!("/tasks/T{id}/pr/needs-owner"), json!({"reason": "Add the NPM_TOKEN repository secret."})).unwrap_err();
    assert!(e.contains("Nothing fails"), "{e}");
}

#[test]
fn a_fix_waiting_to_retry_its_wake_isn_t_idle() {
    let b = board();
    let id = b.failing();
    // The terminal couldn't be reopened: the board tries again in a minute, so the fix isn't dropped.
    assert!(prflow::wake_failed(&b.app, &b.task(id), "no window").unwrap());
    let s = b.stage(id);
    assert!(s["idle"].is_null(), "{s}");
    assert_ne!(s["label"], "Checks failed", "{s}");
    // Once the retry is due and nothing picked it up, nobody is on the checks.
    b.app.db.x("UPDATE tasks SET pr_flow = json_set(pr_flow, '$.retry_at', '2000-01-01T00:00:00Z') WHERE id = ?", p![id]).unwrap();
    let s = b.stage(id);
    assert_eq!(s["label"], "Checks failed", "{s}");
    assert_eq!(s["idle"], true);
}
