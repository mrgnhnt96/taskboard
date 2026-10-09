//! Master breaks (#18): a watched default branch going red opens M<n>, the owner's commits decide
//! whose it is (with a headless fault check), only theirs gets a fix task and an urgent alert, `tb
//! master` overrides it, a finished fix that left it red alerts again, and green closes it.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::breaks::{self, BranchRead, Commit, FakeBranch, MasterProject};
use taskboardd::config::Config;
use taskboardd::prhost::Check;
use taskboardd::util::{iso, now_ts, RowExt};
use taskboardd::{board, dispatch, p};

struct Board {
    app: Arc<App>,
    ci: Arc<FakeBranch>,
}

fn board_with(f: impl FnOnce(&mut Config, &std::path::Path)) -> (Board, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.owner_emails = vec!["sam@acme.dev".into()];
    cfg.master.projects.insert("webapp".into(), MasterProject { host: Some("github".into()), repo: Some("acme/webapp".into()), branch: None });
    f(&mut cfg, dir.path());
    let app = App::for_tests(cfg);
    let ci = Arc::new(FakeBranch { read: parking_lot::Mutex::new(BranchRead::default()) });
    breaks::install(&app, "github", ci.clone());
    (Board { app, ci }, dir)
}

fn commit(sha: &str, email: &str) -> Commit {
    Commit { sha: sha.into(), name: email.split('@').next().unwrap().into(), email: email.into(), message: format!("Change {sha}") }
}

impl Board {
    /// The branch's head is the first commit, with this check state.
    fn branch(&self, state: &str, commits: Vec<Commit>) {
        *self.ci.read.lock() = BranchRead {
            head: commits[0].sha.clone(),
            checks: vec![Check { name: "build".into(), state: state.into(), url: None }],
            commits,
        };
        breaks::check_project(&self.app, "webapp", &self.app.cfg.master.projects["webapp"]).unwrap();
    }
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn tasks(&self) -> usize {
        self.app.db.count("SELECT COUNT(*) FROM tasks", p![]).unwrap() as usize
    }
}

#[test]
fn someone_else_s_break_is_recorded_and_closes_when_green() {
    let (b, _d) = board_with(|_, _| {});
    b.branch("passed", vec![commit("c1", "sam@acme.dev")]);
    assert!(b.get("/master")["open"].as_array().unwrap().is_empty());
    b.branch("failed", vec![commit("c2", "bob@acme.dev"), commit("c1", "sam@acme.dev")]);
    let m = &b.get("/master")["open"][0];
    assert_eq!(m["ref"], "M1");
    assert_eq!(m["checks"], json!(["build"]));
    assert_eq!(m["green_head"], "c1");
    assert_eq!(m["suspects"].as_array().unwrap().len(), 1, "only the commits since the last green head");
    assert_eq!((m["verdict"].as_str(), m["verdict_by"].as_str()), (Some("not_ours"), Some("commits")));
    assert_eq!(b.tasks(), 0, "not the owner's to fix");
    assert!(dispatch::alerts(&b.app).is_empty());
    assert_eq!(b.get("/state")["master"][0]["ref"], "M1", "the app's banner");

    b.branch("running", vec![commit("c3", "bob@acme.dev"), commit("c2", "bob@acme.dev")]);
    assert_eq!(b.get("/master")["open"].as_array().unwrap().len(), 1, "a running build doesn't close it");
    b.branch("passed", vec![commit("c3", "bob@acme.dev"), commit("c2", "bob@acme.dev")]);
    let v = b.get("/master");
    assert!(v["open"].as_array().unwrap().is_empty());
    assert_eq!((v["closed"][0]["state"].as_str(), v["closed"][0]["fixed_head"].as_str()), (Some("closed"), Some("c3")));
    assert!(b.get("/state")["master"].as_array().unwrap().is_empty());
}

#[test]
fn the_owner_s_break_gets_a_fix_task_and_an_urgent_alert_until_it_is_green() {
    let (b, _d) = board_with(|_, _| {});
    b.branch("failed", vec![commit("c2", "Sam@Acme.dev")]);
    let m = b.get("/master/M1");
    assert_eq!((m["verdict"].as_str(), m["verdict_by"].as_str()), (Some("ours"), Some("fallback")), "no claude here: every suspect is theirs");
    let tref = m["task"]["ref"].as_str().unwrap().to_string();
    let t = board::get_task(&b.app, tref[1..].parse().unwrap()).unwrap();
    assert_eq!((t.st("status").as_str(), t.st("priority").as_str(), t.st("project").as_str()), ("queued", "high", "webapp"));
    assert!(t.st("detail").contains("build fails"), "{}", t.st("detail"));
    let alerts = dispatch::listing(&b.app);
    assert_eq!(alerts[0]["urgent"], true);
    assert_eq!(alerts[0]["key"], "master:M1");
    assert_eq!(alerts[0]["task"], tref);
    b.branch("failed", vec![commit("c2", "Sam@Acme.dev")]);
    assert_eq!(b.tasks(), 1, "one fix task per break");

    b.branch("passed", vec![commit("c3", "sam@acme.dev"), commit("c2", "sam@acme.dev")]);
    assert!(dispatch::alerts(&b.app).iter().all(|a| a["key"] != "master:M1"), "green clears it");
    let log = b.app.db.q("SELECT text FROM events WHERE task_id = ?", p![t.id()]).unwrap();
    assert!(log.iter().any(|e| e.st("text").contains("green again")));
}

#[test]
fn an_unsure_break_is_not_the_owner_s_until_they_say_so() {
    let (b, _d) = board_with(|_, _| {});
    b.branch("passed", vec![commit("c0", "bob@acme.dev")]);
    b.branch("failed", vec![commit("c2", "sam@acme.dev"), commit("c1", "bob@acme.dev"), commit("c0", "bob@acme.dev")]);
    let m = b.get("/master/M1");
    assert_eq!(m["verdict"], "unsure");
    assert_eq!(b.tasks(), 0);

    let m = b.post("/master/M1", json!({"verdict": "ours", "who": "Sam"}));
    assert_eq!((m["verdict"].as_str(), m["verdict_by"].as_str()), (Some("ours"), Some("Sam")));
    assert_eq!(b.tasks(), 1);
    assert!(dispatch::alerts(&b.app).iter().any(|a| a["key"] == "master:M1" && a["urgent"] == true));

    b.branch("failed", vec![commit("c3", "bob@acme.dev"), commit("c2", "sam@acme.dev"), commit("c1", "bob@acme.dev")]);
    assert_eq!(b.get("/master/M1")["verdict_by"], "Sam", "the owner's word isn't decided again");
    let m = b.post("/master/M1", json!({"verdict": "not-ours"}));
    assert_eq!(m["verdict"], "not_ours");
    assert!(dispatch::alerts(&b.app).iter().all(|a| a["key"] != "master:M1"));
    assert!(api::dispatch(&b.app, "POST", "/master/M1", &Query::new(), &json!({"verdict": "maybe"})).is_err());
    assert!(api::dispatch(&b.app, "GET", "/master/M9", &Query::new(), &json!({})).is_err());
}

#[test]
fn the_fault_check_reads_the_evidence() {
    let (b, _d) = board_with(|c, dir| {
        let script = dir.join("claude");
        std::fs::write(&script, "#!/bin/sh\necho '{\"structured_output\": {\"verdict\": \"not_ours\", \"why\": \"The test is flaky on main.\"}}'\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        c.claude = script.to_string_lossy().to_string();
    });
    b.branch("failed", vec![commit("c2", "sam@acme.dev")]);
    let m = b.get("/master/M1");
    assert_eq!((m["verdict"].as_str(), m["verdict_by"].as_str(), m["verdict_why"].as_str()), (Some("not_ours"), Some("claude"), Some("The test is flaky on main.")));
    assert_eq!(b.tasks(), 0);
}

#[test]
fn a_fix_that_leaves_it_red_alerts_again() {
    let (b, _d) = board_with(|c, _| c.master.recheck_mins = 30.0);
    b.branch("failed", vec![commit("c2", "sam@acme.dev")]);
    let tid: i64 = b.get("/master/M1")["task"]["ref"].as_str().unwrap()[1..].parse().unwrap();
    dispatch::clear_alert_key(&b.app, "master:M1").unwrap();
    b.app.db.x("UPDATE tasks SET status = 'done', finished_at = ? WHERE id = ?", p![iso(now_ts() - 60.0), tid]).unwrap();
    b.branch("failed", vec![commit("c2", "sam@acme.dev")]);
    assert!(dispatch::alerts(&b.app).is_empty(), "it waits recheck_mins after the task finished");
    b.app.db.x("UPDATE tasks SET finished_at = ? WHERE id = ?", p![iso(now_ts() - 31.0 * 60.0), tid]).unwrap();
    b.branch("failed", vec![commit("c2", "sam@acme.dev")]);
    let a = dispatch::alerts(&b.app);
    assert_eq!(a.len(), 1);
    assert!(a[0]["text"].as_str().unwrap().contains("still"), "{}", a[0]);
}
