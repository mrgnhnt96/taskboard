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
    let ci = Arc::new(FakeBranch::default());
    breaks::install(&app, "github", ci.clone());
    (Board { app, ci }, dir)
}

fn commit(sha: &str, email: &str) -> Commit {
    Commit { sha: sha.into(), name: email.split('@').next().unwrap().into(), email: email.into(), message: format!("Change {sha}") }
}

impl Board {
    /// The branch's head is the first commit, with this check state.
    fn branch(&self, state: &str, commits: Vec<Commit>) {
        self.branch_queued(state, commits, None);
    }
    /// The same, with the failing build queued at `queued`.
    fn branch_queued(&self, state: &str, commits: Vec<Commit>, queued: Option<f64>) {
        *self.ci.read.lock() = BranchRead {
            head: commits[0].sha.clone(),
            checks: vec![Check { name: "build".into(), state: state.into(), url: None }],
            commits,
            queued_at: queued.map(iso),
        };
        breaks::check_project(&self.app, "webapp", &self.app.cfg.master.projects["webapp"]).unwrap();
    }
    fn fix_of(&self, m: &str) -> i64 {
        self.get(&format!("/master/{m}"))["task"]["ref"].as_str().unwrap()[1..].parse().unwrap()
    }
    /// The task finished an hour ago, with PR #n in this state (no PR when `n` is None).
    fn finish(&self, tid: i64, pr: Option<(i64, &str, &str)>) {
        let (num, state, phase) = pr.map(|(n, s, ph)| (Some(n), Some(s), Some(ph))).unwrap_or((None, None, None));
        self.app
            .db
            .x("UPDATE tasks SET status = 'done', finished_at = ?, pr_num = ?, pr_state = ?, pr_phase = ? WHERE id = ?", p![iso(now_ts() - 3600.0), num, state, phase, tid])
            .unwrap();
    }
    fn log_of(&self, tid: i64) -> Vec<String> {
        self.app.db.q("SELECT text FROM events WHERE task_id = ?", p![tid]).unwrap().iter().map(|e| e.st("text")).collect()
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
    assert!(b.get("/state")["master"].as_array().unwrap().is_empty(), "the app's banner is only for the owner's breaks");

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
    let e = api::dispatch(&b.app, "POST", "/master/M1", &Query::new(), &json!({"verdict": "not-ours"})).unwrap_err();
    assert!(e.message.contains("--proof"), "{}", e.message);
    assert!(api::dispatch(&b.app, "POST", "/master/M1", &Query::new(), &json!({"verdict": "not-ours", "proof": ["flaky"]})).is_err());
    let m = b.post("/master/M1", json!({"verdict": "not-ours", "proof": ["https://github.com/acme/webapp/actions/runs/77"]}));
    assert_eq!(m["verdict"], "not_ours");
    assert_eq!(m["proof"], json!(["https://github.com/acme/webapp/actions/runs/77"]));
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

#[test]
fn suspects_reach_back_to_the_last_green_head_past_the_commits_read() {
    let (b, _d) = board_with(|c, _| c.master.commits = 2);
    b.branch("passed", vec![commit("c0", "bob@acme.dev")]);
    *b.ci.history.lock() = vec![commit("c4", "bob@acme.dev"), commit("c3", "bob@acme.dev"), commit("c2", "sam@acme.dev"), commit("c1", "bob@acme.dev"),
                                commit("c0", "bob@acme.dev")];
    b.branch("failed", vec![commit("c4", "bob@acme.dev"), commit("c3", "bob@acme.dev")]);
    let m = b.get("/master/M1");
    let shas: Vec<&str> = m["suspects"].as_array().unwrap().iter().map(|s| s["sha"].as_str().unwrap()).collect();
    assert_eq!(shas, vec!["c4", "c3", "c2", "c1"], "the whole range since c0");
    assert_eq!(m["verdict"], "unsure", "the owner's c2 is among them");
}

#[test]
fn without_a_green_head_every_commit_read_is_a_suspect() {
    let (b, _d) = board_with(|_, _| {});
    b.branch("failed", vec![commit("c3", "bob@acme.dev"), commit("c2", "sam@acme.dev")]);
    let m = b.get("/master/M1");
    assert_eq!(m["suspects"].as_array().unwrap().len(), 2);
    assert_eq!(m["verdict"], "unsure");
}

#[test]
fn the_branch_is_the_repo_s_default_unless_set_or_carried_over() {
    let (b, _d) = board_with(|_, _| {});
    *b.ci.default.lock() = Some("master".into());
    b.branch("passed", vec![commit("c1", "bob@acme.dev")]);
    assert_eq!(b.ci.branches.lock().last().map(|s| s.as_str()), Some("master"));
    assert_eq!(b.get("/master")["watched"][0]["branch"], "master");

    let (b, _d) = board_with(|_, _| {});
    breaks::carry_branch(&b.app, "webapp", "develop").unwrap();
    *b.ci.default.lock() = Some("master".into());
    b.branch("passed", vec![commit("c1", "bob@acme.dev")]);
    assert_eq!(b.ci.branches.lock().last().map(|s| s.as_str()), Some("develop"), "the import's branch");

    let (b, _d) = board_with(|c, _| c.master.projects.get_mut("webapp").unwrap().branch = Some("trunk".into()));
    breaks::carry_branch(&b.app, "webapp", "develop").unwrap();
    b.branch("passed", vec![commit("c1", "bob@acme.dev")]);
    assert_eq!(b.ci.branches.lock().last().map(|s| s.as_str()), Some("trunk"), "config.toml's word first");

    let (b, _d) = board_with(|_, _| {});
    b.branch("passed", vec![commit("c1", "bob@acme.dev")]);
    assert_eq!(b.ci.branches.lock().last().map(|s| s.as_str()), Some("main"), "main when nothing says");
}

#[test]
fn the_fix_task_starts_at_once() {
    let (b, d) = board_with(|_, _| {});
    let repo = d.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    b.app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    b.branch("failed", vec![commit("c2", "sam@acme.dev")]);
    let tid: i64 = b.get("/master/M1")["task"]["ref"].as_str().unwrap()[1..].parse().unwrap();
    let t = board::get_task(&b.app, tid).unwrap();
    assert!(t.i("start_job").is_some(), "started, not left queued");
    assert_eq!(t.st("pickup"), "new");
    assert_eq!(b.get("/state")["master"][0]["ref"], "M1", "the owner's break is on the banner");

    let (b, d) = board_with(|c, _| c.master.start_fix = false);
    let repo = d.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    b.app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    b.branch("failed", vec![commit("c2", "sam@acme.dev")]);
    let tid: i64 = b.get("/master/M1")["task"]["ref"].as_str().unwrap()[1..].parse().unwrap();
    assert!(board::get_task(&b.app, tid).unwrap().i("start_job").is_none());
}

#[test]
fn a_failure_while_the_fix_pr_is_open_joins_it() {
    let (b, _d) = board_with(|c, _| c.master.recheck_mins = 30.0);
    b.branch("failed", vec![commit("c2", "sam@acme.dev")]);
    let t1 = b.fix_of("M1");
    b.finish(t1, Some((5, "OPEN", "review")));
    dispatch::clear_alert_key(&b.app, "master:M1").unwrap();

    b.branch("failed", vec![commit("c3", "sam@acme.dev"), commit("c2", "sam@acme.dev")]);
    assert_eq!(b.tasks(), 1, "the open fix PR covers the new failure");
    assert_eq!(b.fix_of("M1"), t1);
    assert!(dispatch::alerts(&b.app).iter().all(|a| !a["text"].as_str().unwrap().contains("still")), "no 'still red' while the fix PR is open");

    b.branch("passed", vec![commit("c4", "sam@acme.dev")]);
    b.branch("failed", vec![commit("c5", "sam@acme.dev"), commit("c4", "sam@acme.dev")]);
    assert_eq!(b.tasks(), 1, "a new break joins the open fix");
    assert_eq!(b.fix_of("M2"), t1);
    assert!(b.log_of(t1).iter().any(|l| l.contains("M2 joins this fix")), "{:?}", b.log_of(t1));
}

#[test]
fn a_build_queued_before_the_fix_merged_joins_it_and_a_later_one_starts_a_new_fix() {
    let (b, _d) = board_with(|_, _| {});
    let merged = now_ts() - 600.0;
    b.branch_queued("failed", vec![commit("c2", "sam@acme.dev")], Some(merged - 1200.0));
    let t1 = b.fix_of("M1");
    b.finish(t1, Some((5, "MERGED", "merged")));
    b.app.db.x("INSERT INTO events(task_id, at, who, kind, text) VALUES (?, ?, 'PR', 'status', 'PR #5 was merged')", p![t1, iso(merged)]).unwrap();

    b.branch_queued("failed", vec![commit("c3", "sam@acme.dev"), commit("c2", "sam@acme.dev")], Some(merged - 60.0));
    assert_eq!(b.tasks(), 1, "that build couldn't hold the fix");
    assert_eq!(b.fix_of("M1"), t1);

    b.branch_queued("failed", vec![commit("c4", "sam@acme.dev"), commit("c3", "sam@acme.dev")], Some(merged + 60.0));
    assert_eq!(b.tasks(), 2, "a build queued after the fix merged that still fails gets a new fix");
    let t2 = b.fix_of("M1");
    assert_ne!(t2, t1);
    assert_eq!(board::get_task(&b.app, t2).unwrap().st("status"), "queued");
}

#[test]
fn a_fix_that_finished_without_a_pr_hands_its_break_to_the_open_fix_with_no_alert() {
    let (b, _d) = board_with(|c, _| c.master.recheck_mins = 30.0);
    b.branch("failed", vec![commit("c2", "sam@acme.dev")]);
    let t1 = b.fix_of("M1");
    b.branch("passed", vec![commit("c3", "sam@acme.dev")]);
    // M2 got its own fix, which found the open fix PR and finished without one.
    b.branch("failed", vec![commit("c4", "sam@acme.dev"), commit("c3", "sam@acme.dev")]);
    let card = taskboardd::ops::new_task(&b.app, &json!({"title": "Fix red main", "project": "webapp"}), board::BOARD, None).unwrap();
    let t2 = card["id"].as_i64().unwrap();
    b.app.db.x("UPDATE breaks SET task_id = ? WHERE id = 2", p![t2]).unwrap();
    b.finish(t1, Some((5, "OPEN", "review")));
    b.finish(t2, None);
    dispatch::clear_alert_key(&b.app, "master:M2").unwrap();

    b.branch("failed", vec![commit("c4", "sam@acme.dev"), commit("c3", "sam@acme.dev")]);
    assert_eq!(b.fix_of("M2"), t1, "handed over to the open fix");
    assert!(b.log_of(t1).iter().any(|l| l.contains("M2 handed over")), "{:?}", b.log_of(t1));
    assert!(dispatch::alerts(&b.app).iter().all(|a| a["key"] != "master:M2"), "no 'finished without a fix PR' alert");
    assert_eq!(b.tasks(), 2);
}
