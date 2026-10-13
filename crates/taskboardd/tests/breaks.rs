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
    // The commit fallback decides unless a test turns the fault check on with a scripted claude.
    cfg.master.fault_check = false;
    f(&mut cfg, dir.path());
    let app = App::for_tests(cfg);
    let ci = Arc::new(FakeBranch::default());
    breaks::install(&app, "github", ci.clone());
    (Board { app, ci }, dir)
}

/// A verdict reason long enough for the board.
const WHY: &str = "Sam's change broke the build; checked the log.";

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
            checks: vec![Check { name: "build".into(), state: state.into(), url: None, at: None }],
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

    let m = b.post("/master/M1", json!({"verdict": "ours", "who": "Sam", "why": WHY}));
    assert_eq!((m["verdict"].as_str(), m["verdict_by"].as_str()), (Some("ours"), Some("Sam")));
    assert_eq!(b.tasks(), 1);
    assert!(dispatch::alerts(&b.app).iter().any(|a| a["key"] == "master:M1" && a["urgent"] == true));

    b.branch("failed", vec![commit("c3", "bob@acme.dev"), commit("c2", "sam@acme.dev"), commit("c1", "bob@acme.dev")]);
    assert_eq!(b.get("/master/M1")["verdict_by"], "Sam", "the owner's word isn't decided again");
    let e = api::dispatch(&b.app, "POST", "/master/M1", &Query::new(), &json!({"verdict": "not-ours", "why": WHY, "title": "Flaky test"})).unwrap_err();
    assert!(e.message.contains("--proof"), "{}", e.message);
    assert!(api::dispatch(&b.app, "POST", "/master/M1", &Query::new(), &json!({"verdict": "not-ours", "why": WHY, "title": "Flaky test", "proof": ["flaky"]})).is_err());
    let m = b.post("/master/M1", json!({"verdict": "not-ours", "why": WHY, "title": "Flaky test", "proof": ["https://github.com/acme/webapp/actions/runs/77"]}));
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
        std::fs::write(
            &script,
            "#!/bin/sh\necho '{\"structured_output\": {\"verdict\": \"not_ours\", \"why\": \"The test is flaky on main.\", \"proof\": [\"https://ci/7\", \"flaky\"]}}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        c.claude = script.to_string_lossy().to_string();
        c.master.fault_check = true;
    });
    b.branch("failed", vec![commit("c2", "sam@acme.dev")]);
    let m = b.get("/master/M1");
    assert_eq!((m["verdict"].as_str(), m["verdict_by"].as_str(), m["verdict_why"].as_str()), (Some("not_ours"), Some("claude"), Some("The test is flaky on main.")));
    assert_eq!(m["proof"], json!(["https://ci/7"]), "only the http(s) links");
    assert_eq!(b.tasks(), 0);
}

/// A board whose fault check is the script `body` (a shell command line run as claude).
fn with_check(body: &str) -> (Board, tempfile::TempDir) {
    let body = body.to_string();
    board_with(move |c, dir| {
        let script = dir.join("claude");
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        c.claude = script.to_string_lossy().to_string();
        c.master.fault_check = true;
    })
}

#[test]
fn a_fault_check_that_can_t_decide_is_unsure_and_starts_no_fix() {
    for (body, want) in [
        ("echo 'boom' >&2; exit 3", "its answer couldn't be read (exit 3: boom)"),
        ("echo '{\"structured_output\": {\"verdict\": \"maybe\"}}'", "it gave no verdict"),
    ] {
        let (b, _d) = with_check(body);
        b.branch("failed", vec![commit("c2", "sam@acme.dev")]);
        let m = b.get("/master/M1");
        assert_eq!((m["verdict"].as_str(), m["verdict_by"].as_str()), (Some("unsure"), Some("claude")), "{body}");
        assert!(m["check_error"].as_str().unwrap().contains(want), "{}", m["check_error"]);
        assert!(m["verdict_why"].as_str().unwrap().starts_with("The fault check couldn't decide: "), "{}", m["verdict_why"]);
        assert_eq!(b.tasks(), 0, "only a verdict the check reached starts a fix");
        assert!(dispatch::alerts(&b.app).is_empty());
    }

    let (b, _d) = board_with(|c, _| {
        c.claude = "/nowhere/claude".into();
        c.master.fault_check = true;
    });
    b.branch("failed", vec![commit("c2", "sam@acme.dev")]);
    let m = b.get("/master/M1");
    assert_eq!(m["verdict"], "unsure", "a missing claude can't decide either");
    assert!(m["check_error"].as_str().unwrap().contains("isn't installed"), "{}", m["check_error"]);
    assert_eq!(b.tasks(), 0);
}

#[test]
fn the_fault_check_s_not_ours_without_proof_is_unsure() {
    let (b, _d) = with_check("echo '{\"structured_output\": {\"verdict\": \"not_ours\", \"why\": \"Looks like a flaky runner.\", \"title\": \"Flaky runner\", \"proof\": []}}'");
    b.branch("failed", vec![commit("c2", "sam@acme.dev")]);
    let m = b.get("/master/M1");
    assert_eq!(m["verdict"], "unsure");
    assert_eq!(m["verdict_why"], "It looked like someone else's, but nothing proved it: Looks like a flaky runner.");
    assert_eq!(m["check_error"], Value::Null, "the check did decide");
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

#[test]
fn the_old_listener_s_routes_read_the_branch_and_answer_as_before() {
    let (b, _d) = board_with(|_, _| {});
    b.branch("passed", vec![commit("c1", "bob@acme.dev")]);
    *b.ci.read.lock() = BranchRead {
        head: "c2".into(),
        checks: vec![Check { name: "build".into(), state: "failed".into(), url: None, at: None }],
        commits: vec![commit("c2", "bob@acme.dev"), commit("c1", "bob@acme.dev")],
        queued_at: None,
    };
    let url = "https://github.com/acme/webapp/actions/runs/41";
    let v = b.post("/master-builds", json!({"pipeline": "Pipeline", "build_id": 41, "url": url, "sha": "c2", "state": "finished", "ts": 1700000000}));
    assert_eq!((v["project"].as_str(), v["checked"].as_bool()), (Some("webapp"), Some(true)), "{v}");
    assert_eq!(v["break"]["ref"], "M1", "the branch is read at once");
    let ev = &b.get("/master/M1")["evidence"];
    assert_eq!((ev["build"]["build_id"].as_i64(), ev["build"]["state"].as_str()), (Some(41), Some("finished")), "{ev}");
    assert_eq!(ev["checks"][0]["url"], url, "the failing check gets the build's link");
    let feed: Value = serde_json::from_str(&b.app.db.get_setting("pr_feed").unwrap().unwrap()).unwrap();
    assert!(feed["last_event_at"].is_string(), "the feed is alive: {feed}");
    assert_eq!(feed["last_event"]["kind"], "master-build");

    let v = b.post("/master-builds", json!({"pipeline": "Other", "url": "https://github.com/acme/api/actions/runs/2", "project": "nope"}));
    assert_eq!(v["checked"], true, "the only watched project when nothing names one");

    let list = b.get("/master-breaks");
    assert_eq!(list["breaks"][0]["ref"], "M1");
    assert_eq!(b.get("/master-breaks/M1")["ref"], "M1");
    let mut q = Query::new();
    q.insert("limit".into(), "0".into());
    assert_eq!(api::dispatch(&b.app, "GET", "/master-breaks", &q, &json!({})).unwrap()["breaks"].as_array().unwrap().len(), 1, "at least one");

    let m = b.post("/master-breaks/M1/verdict", json!({"verdict": "yours", "who": "Sam", "why": WHY}));
    assert_eq!(m["verdict"], "ours");
    assert!(api::dispatch(&b.app, "POST", "/master-breaks/M1/verdict", &Query::new(), &json!({"verdict": "not_yours", "why": WHY, "title": "Flaky test"})).is_err(), "still needs a proof link");
    let m = b.post("/master-breaks/M1/verdict", json!({"verdict": "not_yours", "why": WHY, "title": "Flaky test", "proof": [url]}));
    assert_eq!(m["verdict"], "not_ours");
    assert!(api::dispatch(&b.app, "DELETE", "/master-breaks/M1", &Query::new(), &json!({})).is_err());
}

fn stream_build(b: &Board, id: &str, repo: &str, branch: &str, head: &str, state: &str) -> Value {
    let url = "https://github.com/acme/webapp/actions/runs/41";
    let ev = json!({"id": id, "kind": "build", "at": "2026-10-12T15:04:00Z", "source": "relay", "repo": repo, "branch": branch, "head": head,
                    "pipeline": "ci", "state": state, "url": url});
    b.post("/stream/events", json!({"v": 1, "events": [ev]}))["results"][0].clone()
}

#[test]
fn a_v1_build_on_the_watched_branch_reaches_the_break_watch() {
    let (b, _d) = board_with(|c, _| c.master.projects.get_mut("webapp").unwrap().branch = Some("master".into()));
    b.branch("passed", vec![commit("c1", "bob@acme.dev")]);
    *b.ci.read.lock() = BranchRead {
        head: "c2".into(),
        checks: vec![Check { name: "build".into(), state: "failed".into(), url: None, at: None }],
        commits: vec![commit("c2", "bob@acme.dev"), commit("c1", "bob@acme.dev")],
        queued_at: None,
    };

    let r = stream_build(&b, "e1", "acme/webapp", "feature/x", "c2", "failed");
    assert_eq!(r["status"], "accepted", "{r}");
    assert!(b.get("/master")["open"].as_array().unwrap().is_empty(), "another branch doesn't touch breaks");
    let r = stream_build(&b, "e2", "acme/api", "master", "c2", "failed");
    assert_eq!(r["status"], "accepted", "{r}");
    assert!(b.get("/master")["open"].as_array().unwrap().is_empty(), "a repo not under [master.projects] doesn't either");

    let r = stream_build(&b, "e3", "acme/webapp", "master", "c2", "failed");
    assert_eq!(r["result"]["master"]["break"]["ref"], "M1", "{r}");
    let ev = &b.get("/master/M1")["evidence"];
    let build = &ev["build"];
    assert_eq!((build["build_id"].as_i64(), build["sha"].as_str(), build["state"].as_str(), build["pipeline"].as_str()), (Some(41), Some("c2"), Some("failed"), Some("ci")), "{ev}");
    assert_eq!(build["ts"], "2026-10-12T15:04:00Z");
    assert_eq!(ev["checks"][0]["url"], "https://github.com/acme/webapp/actions/runs/41", "the failing check gets the build's link");
    assert_eq!(b.get("/master/M1")["green_head"], "c1", "the watch keeps the last green itself");
    let builds = b.get("/stream")["sources"].as_array().unwrap().iter().find(|s| s["source"] == "builds").cloned().unwrap();
    assert_eq!(builds["last_event"]["kind"], "build", "{builds}");

    b.ci.read.lock().checks[0].state = "passed".into();
    let r = stream_build(&b, "e4", "acme/webapp", "master", "c2", "passed");
    assert_eq!(r["result"]["master"]["break"], Value::Null, "{r}");
    assert!(b.get("/master")["open"].as_array().unwrap().is_empty(), "a passed build closes it");
}

#[test]
fn a_verdict_s_title_names_the_cause_on_the_card() {
    let (b, _d) = board_with(|_, _| {});
    b.branch("passed", vec![commit("c0", "bob@acme.dev")]);
    b.branch("failed", vec![commit("c2", "sam@acme.dev"), commit("c1", "bob@acme.dev"), commit("c0", "bob@acme.dev")]);
    let long = "x".repeat(81);
    let e = api::dispatch(&b.app, "POST", "/master/M1", &Query::new(), &json!({"verdict": "unsure", "why": WHY, "title": long})).unwrap_err();
    assert!(e.message.contains("80 at most"), "{}", e.message);
    assert_eq!(b.get("/master/M1")["verdict_by"], "fallback", "a refused title changes nothing");
    let m = b.post("/master/M1", json!({"verdict": "not-ours", "why": "The NDK mirror was down for every build.", "title": "Android NDK download failed",
                                        "proof": ["https://github.com/acme/webapp/actions/runs/77"]}));
    assert_eq!(m["verdict_title"], "Android NDK download failed");
    assert_eq!(b.get("/master/M1")["verdict_title"], "Android NDK download failed");
    let m = b.post("/master/M1", json!({"verdict": "ours", "why": WHY}));
    assert_eq!(m["verdict_title"], Value::Null, "a new word without a title drops the old one");
}

#[test]
fn a_verdict_needs_a_reason_and_not_ours_a_cause() {
    let (b, _d) = board_with(|_, _| {});
    b.branch("passed", vec![commit("c0", "bob@acme.dev")]);
    b.branch("failed", vec![commit("c2", "sam@acme.dev"), commit("c1", "bob@acme.dev"), commit("c0", "bob@acme.dev")]);
    let post = |body: Value| api::dispatch(&b.app, "POST", "/master/M1", &Query::new(), &body);
    let proof = json!(["https://github.com/acme/webapp/actions/runs/77"]);
    for body in [json!({"verdict": "ours"}), json!({"verdict": "unsure"}), json!({"verdict": "unsure", "why": "too short"})] {
        let e = post(body.clone()).unwrap_err();
        assert_eq!(e.status, 400, "{body}");
        assert!(e.message.contains("--why"), "{}", e.message);
    }
    assert_eq!(b.get("/master/M1")["verdict_by"], "fallback", "a refused verdict changes nothing");
    let e = post(json!({"verdict": "not-ours", "why": WHY, "proof": proof})).unwrap_err();
    assert!(e.status == 400 && e.message.contains("--title"), "{}", e.message);
    let m = post(json!({"verdict": "not-ours", "why": WHY, "proof": proof, "title": "Android NDK download failed"})).unwrap();
    assert_eq!(m["verdict_title"], "Android NDK download failed");
    let m = post(json!({"verdict": "unsure", "why": WHY})).unwrap();
    assert_eq!((m["verdict_title"].clone(), m["verdict_label"].as_str()), (Value::Null, Some("unsure, so not yours to fix")), "the card falls back to the label");
}

#[test]
fn a_verdict_from_a_terminal_is_credited_to_it() {
    let (b, _d) = board_with(|_, _| {});
    taskboardd::midna::sync(&b.app, &[json!({"id": "s1", "name": "Fixer", "agent": "claude", "cwd": "/tmp", "status": {"state": "working"}})], &[]).unwrap();
    b.branch("passed", vec![commit("c0", "bob@acme.dev")]);
    b.branch("failed", vec![commit("c2", "sam@acme.dev"), commit("c1", "bob@acme.dev"), commit("c0", "bob@acme.dev")]);
    let m = b.post("/master/M1", json!({"verdict": "unsure", "why": WHY, "by_session": "s1"}));
    assert_eq!(m["verdict_by"], "Fixer");
    let m = b.post("/master/M1", json!({"verdict": "unsure", "why": WHY}));
    assert_eq!(m["verdict_by"], b.app.cfg.owner.as_str(), "from the app it's the owner's");
    let m = b.post("/master/M1", json!({"verdict": "unsure", "why": WHY, "by_session": "s1", "who": "Sam"}));
    assert_eq!(m["verdict_by"], "Sam", "--who wins over the terminal");
}

#[test]
fn the_fault_check_s_title_names_the_fix_task() {
    let (b, _d) = board_with(|c, dir| {
        let script = dir.join("claude");
        std::fs::write(
            &script,
            "#!/bin/sh\necho '{\"structured_output\": {\"verdict\": \"ours\", \"why\": \"The new Gradle plugin pins an NDK that no longer downloads.\", \"title\": \"Android NDK download failed\"}}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        c.claude = script.to_string_lossy().to_string();
        c.master.fault_check = true;
    });
    b.branch("passed", vec![commit("c1", "bob@acme.dev")]);
    *b.ci.read.lock() = BranchRead {
        head: "c2".into(),
        checks: vec![Check { name: "build".into(), state: "failed".into(), url: None, at: None }],
        commits: vec![commit("c2", "sam@acme.dev"), commit("c1", "bob@acme.dev")],
        queued_at: None,
    };
    b.post("/master-builds", json!({"pipeline": "android", "build_id": 41, "url": "https://github.com/acme/webapp/actions/runs/41", "sha": "c2", "state": "failed"}));
    let m = b.get("/master/M1");
    assert_eq!((m["verdict"].as_str(), m["verdict_by"].as_str(), m["verdict_title"].as_str()), (Some("ours"), Some("claude"), Some("Android NDK download failed")));
    let t = board::get_task(&b.app, b.fix_of("M1")).unwrap();
    assert_eq!(t.st("title"), "Fix master android: Android NDK download failed");
    let alert = dispatch::alerts(&b.app).into_iter().find(|a| a["key"] == "master:M1").unwrap();
    assert!(alert["text"].as_str().unwrap().contains("build fails (Android NDK download failed)"), "{alert}");
}

#[test]
fn tb_master_lists_only_the_red_ones_or_the_newest() {
    let (b, _d) = board_with(|_, _| {});
    b.branch("failed", vec![commit("c1", "bob@acme.dev")]);
    b.branch("passed", vec![commit("c2", "bob@acme.dev"), commit("c1", "bob@acme.dev")]);
    b.branch("failed", vec![commit("c3", "bob@acme.dev"), commit("c2", "bob@acme.dev")]);
    let v = b.get("/master");
    assert_eq!((v["open"][0]["ref"].as_str(), v["closed"][0]["ref"].as_str()), (Some("M2"), Some("M1")));

    let mut q = Query::new();
    q.insert("open".into(), "1".into());
    let v = api::dispatch(&b.app, "GET", "/master", &q, &json!({})).unwrap();
    assert_eq!(v["open"].as_array().unwrap().len(), 1);
    assert_eq!(v["open"][0]["ref"], "M2");
    assert!(v["closed"].as_array().unwrap().is_empty(), "only the red one");

    b.branch("passed", vec![commit("c4", "bob@acme.dev"), commit("c3", "bob@acme.dev")]);
    let mut q = Query::new();
    q.insert("limit".into(), "1".into());
    let v = api::dispatch(&b.app, "GET", "/master", &q, &json!({})).unwrap();
    let listed: Vec<&Value> = v["open"].as_array().unwrap().iter().chain(v["closed"].as_array().unwrap()).collect();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["ref"], "M2", "the newest");
}

#[test]
fn the_fix_task_says_how_to_finish_and_its_agent_may_rule_the_break_out() {
    let (b, _d) = board_with(|_, _| {});
    taskboardd::midna::sync(&b.app, &[json!({"id": "s1", "name": "Fixer", "agent": "claude", "cwd": "/tmp", "status": {"state": "working"}})], &[]).unwrap();
    b.branch("failed", vec![commit("c2", "sam@acme.dev")]);
    let t = board::get_task(&b.app, b.fix_of("M1")).unwrap();
    let detail = t.st("detail");
    for want in [
        "Branch from origin/main, make the smallest change that turns it green, and open the PR.",
        "If it goes green on its own, check tb master M1 before doing more, then finish with tb done \"<summary>\" --no-pr \"<why>\".",
        "run tb master M1 not-ours --title \"<cause>\" --why \"<how you checked>\" --proof <build or artifact link>",
        "tb master M1 unsure --why",
    ] {
        assert!(detail.contains(want), "{want} in {detail}");
    }

    let not_ours = json!({"verdict": "not-ours", "title": "Runner image lost its SDK", "why": "The same step fails on main before Sam's commit.", "proof": "https://ci/1", "by_session": "s1"});
    let m = b.post("/master/M1", not_ours.clone());
    assert_eq!((m["verdict"].as_str(), m["verdict_by"].as_str()), (Some("not_ours"), Some("Fixer")), "the fix task's agent rules it out, credited to its terminal");

    b.post("/master/M1", json!({"verdict": "ours", "why": WHY}));
    let e = api::dispatch(&b.app, "POST", "/master/M1", &Query::new(), &not_ours).unwrap_err();
    assert_eq!(e.status, 409, "{}", e.message);
    assert!(e.message.contains("their word stands"), "{}", e.message);
    assert_eq!(b.get("/master/M1")["verdict"], "ours", "the owner's word wins");
}

#[test]
fn the_master_alert_follows_its_fix_and_notifies_on_each_change() {
    let (b, _d) = board_with(|_, _| {});
    b.branch("failed", vec![commit("c2", "sam@acme.dev")]);
    let tid = b.fix_of("M1");
    let t = format!("T{tid}");
    let master = || dispatch::alerts(&b.app).into_iter().find(|a| a["key"] == "master:M1").expect("the master alert is up");
    let first = master();
    assert!(first["text"].as_str().unwrap().contains(&format!("It's yours to fix: {t} is on it.")), "{first}");
    breaks::tick(&b.app).unwrap();
    assert_eq!(master()["id"], first["id"], "nothing changed, so it isn't raised again");

    let set = |sql: &str| b.app.db.x(&format!("UPDATE tasks SET {sql} WHERE id = ?"), p![tid]).unwrap();
    let mut last = first["id"].clone();
    let mut expect = |want: String| {
        breaks::tick(&b.app).unwrap();
        let a = master();
        assert_eq!(a["text"], want.as_str());
        assert_eq!(a["urgent"], true);
        assert_ne!(a["id"], last, "a new state raises it again, so it notifies");
        last = a["id"].clone();
    };

    set(&format!("created_at = '{}'", iso(now_ts() - 600.0)));
    expect(format!("{t} hasn't started after 5 minutes: Couldn't start it at once: The board doesn't know the folder for the project “webapp”.. It waits in the queue."));

    set("status = 'needs', needs_reason = 'question', question = 'Which NDK version should the build pin?'");
    dispatch::add_alert(&b.app, "the fix's own question", Some(tid), None, None, None).unwrap();
    expect(format!("{t}, the fix for master webapp, needs you: Which NDK version should the build pin?"));

    set("status = 'done', failed = 1, summary = 'The build machine is out of disk.'");
    expect(format!("{t}, the fix for master webapp, has stopped: The build machine is out of disk."));

    set(&format!("failed = 0, finished_at = '{}', pr_num = 5, pr_state = 'OPEN', pr_phase = 'fix', pr_flow = '{{}}'", iso(now_ts())));
    expect(format!("The fix PR #5's checks are failing; {t} is fixing them."));

    set("pr_phase = 'review'");
    expect("The fix PR #5 for master webapp is waiting for your review.".into());

    set("pr_phase = 'comments', pr_flow = '{\"wake_tries\": 3}'");
    expect("The fix PR #5 for master webapp needs its agent, but it didn't wake.".into());

    set("pr_num = NULL, pr_state = NULL, pr_phase = NULL, pr_flow = NULL");
    expect(format!("{t}, the fix for master webapp, finished without a fix PR, and main is still red."));

    set("status = 'working'");
    b.app.db.x("UPDATE breaks SET opened_at = ? WHERE id = 1", p![iso(now_ts() - 3.0 * 3600.0 - 60.0)]).unwrap();
    expect(format!("Master webapp is still red 3 hours after your change broke it; {t} is on it."));

    b.app.db.x("UPDATE breaks SET task_id = NULL WHERE id = 1", p![]).unwrap();
    expect("Master webapp broke on your change and no fix is running.".into());

    b.branch("passed", vec![commit("c3", "sam@acme.dev")]);
    breaks::tick(&b.app).unwrap();
    assert!(dispatch::alerts(&b.app).iter().all(|a| a["key"] != "master:M1"), "green takes it down for good");
}

#[test]
fn the_master_alert_and_a_task_alert_outlive_the_board_s_own_lines() {
    let (b, _d) = board_with(|_, _| {});
    b.branch("failed", vec![commit("c2", "sam@acme.dev")]);
    let fix = b.fix_of("M1");
    let other = b.post("/tasks", json!({"title": "Probe", "detail": "d", "project": "webapp"}))["id"].as_i64().unwrap();
    dispatch::add_alert(&b.app, "Probe needs you", Some(other), None, None, None).unwrap();
    let keys = || dispatch::alerts(&b.app).into_iter().map(|a| a["task_id"].as_i64().unwrap_or(0)).collect::<Vec<_>>();
    let later = iso(now_ts() + 5.0);
    for (tid, who, text) in [(fix, "PR", "PR #5: checks passed"), (other, "PR", "PR #6: checks passed"), (other, "Hook", "pr.checks hook failed")] {
        b.app.db.x("INSERT INTO events(task_id, at, who, kind, text) VALUES (?, ?, ?, 'status', ?)", p![tid, later.clone(), who, text]).unwrap();
    }
    dispatch::prune_alerts(&b.app).unwrap();
    assert!(dispatch::alerts(&b.app).iter().any(|a| a["key"] == "master:M1"), "a PR stage doesn't take the break's alert down");
    assert!(keys().contains(&other), "the board's own lines aren't the agent moving on");

    b.app.db.x("INSERT INTO events(task_id, at, who, kind, text) VALUES (?, ?, 'Fix agent', 'turn', 'Pinned the NDK')", p![fix, later.clone()]).unwrap();
    dispatch::prune_alerts(&b.app).unwrap();
    assert!(dispatch::alerts(&b.app).iter().any(|a| a["key"] == "master:M1"), "nor does the fix's own work while main is red");

    b.app.db.x("UPDATE breaks SET state = 'closed' WHERE id = 1", p![]).unwrap();
    dispatch::prune_alerts(&b.app).unwrap();
    assert!(dispatch::alerts(&b.app).iter().all(|a| a["key"] != "master:M1"), "a closed break's alert goes");
}
