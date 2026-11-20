//! Expected checks read from Azure DevOps (`azure_checks.rs`): a build still queued in Azure, which
//! the PR host doesn't show yet, holds the merge past `expected_wait_mins`; a finished one reads as
//! passed or failed; with none in Azure the expected wait runs out as before, sooner for a stacked PR
//! when the project says so.

use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::{Config, PrProject};
use taskboardd::prhost::bitbucket::Http;
use taskboardd::prhost::{self, Check, FakeHost, HostResult, Record, Reviewer};
use taskboardd::util::{now_ts, RowExt};
use taskboardd::{azure_checks, board, midna, prflow, reports};

const BB: &str = "https://bitbucket.org/acme/webapp/pull-requests/9";
const HEAD: &str = "abc123def456";

/// Azure's Build API for tests: answers every builds query with `builds` and counts the calls.
#[derive(Default)]
struct FakeAzure {
    builds: Mutex<Vec<Value>>,
    calls: Mutex<Vec<String>>,
}

impl Http for FakeAzure {
    fn call(&self, _m: &str, url: &str, _b: Option<&Value>) -> HostResult<Value> {
        self.calls.lock().push(url.to_string());
        Ok(json!({"value": self.builds.lock().clone()}))
    }
}

impl FakeAzure {
    fn set(&self, status: &str, result: Option<&str>) {
        *self.builds.lock() = vec![json!({
            "id": 42, "status": status, "result": result, "sourceVersion": format!("{HEAD}0000"),
            "queueTime": "2026-11-20T19:10:00Z",
            "_links": {"web": {"href": "https://dev.azure.com/roonlabs/proj/_build/results?buildId=42"}}
        })];
    }
}

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
    azure: Arc<FakeAzure>,
}

fn board_with(f: impl FnOnce(&mut PrProject)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    cfg.pr.agents_merge = Some(true);
    let mut rules = PrProject {
        approvals: Some(1),
        expected: Some(vec!["roon_mobile".into()]),
        expected_wait_mins: Some(30.0),
        azure_project: Some("https://dev.azure.com/roonlabs/proj".into()),
        azure_pipelines: Some([("roon_mobile".to_string(), 16)].into_iter().collect()),
        ..Default::default()
    };
    f(&mut rules);
    cfg.pr.projects.insert("webapp".into(), rules);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    let azure = Arc::new(FakeAzure::default());
    azure_checks::install(&app, azure.clone());
    Board { app, dir, azure }
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| e.message)
    }
    fn pr_task(&self) -> i64 {
        let id = self.post("/tasks", json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
        for (event, extra) in [("tb.take", json!({"task": format!("T{id}")})), ("tb.done", json!({"summary": "Done", "pr": BB}))] {
            let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
            for (k, v) in extra.as_object().unwrap() {
                b[k] = v.clone();
            }
            reports::handle(&self.app, b, false).unwrap();
        }
        id
    }
    fn phase(&self, id: i64) -> String {
        board::get_task(&self.app, id).unwrap().st("pr_phase")
    }
    fn flow(&self, id: i64) -> Value {
        serde_json::from_str(&board::get_task(&self.app, id).unwrap().st("pr_flow")).unwrap()
    }
    fn poll(&self) {
        prflow::refresh(&self.app).unwrap();
    }
    /// The push was first seen `mins` ago, and Azure was last asked long enough ago to ask again.
    fn age(&self, id: i64, mins: f64) {
        let mut runs = self.flow(id)["azure_runs"].clone();
        if let Some(e) = runs.get_mut(HEAD) {
            e["at"] = json!("2000-01-01T00:00:00Z");
        }
        prflow::merge_flow(&self.app, id, vec![("head_at", json!({HEAD: now_ts() - mins * 60.0})), ("azure_runs", runs)]).unwrap();
    }
    fn merge(&self, id: i64) -> Result<Value, String> {
        self.post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true, "who": "The agent"}))
    }
}

/// An approved PR with no checks on the host yet.
fn approved(base: &str) -> Record {
    Record {
        state: "OPEN".into(),
        title: "Hide the spinner".into(),
        author: "me".into(),
        head: HEAD.into(),
        branch: "fix/RRD-6420-hidden-spinner-ticker".into(),
        base: base.into(),
        base_head: "b1".into(),
        reviewers: vec![Reviewer { user: "rev".into(), name: "Rev".into(), state: "approved".into(), requested: true, ..Default::default() }],
        approvals: 1,
        ..Default::default()
    }
}

fn host(b: &Board, rec: Record) -> Arc<FakeHost> {
    let h = FakeHost::new("bitbucket", rec);
    prhost::install(&b.app, h.clone());
    h
}

fn checks(b: &Board, id: i64) -> Vec<Value> {
    b.flow(id)["rec"]["checks"].as_array().cloned().unwrap_or_default()
}

#[test]
fn a_build_queued_in_azure_holds_the_merge_past_the_expected_wait() {
    let b = board_with(|_| {});
    let id = b.pr_task();
    let h = host(&b, approved("main"));
    b.azure.set("notStarted", None);
    b.poll();
    assert_eq!(b.phase(id), "checks");
    let c = checks(&b, id);
    assert_eq!(c.len(), 1);
    assert_eq!((c[0]["name"].as_str(), c[0]["state"].as_str(), c[0]["via"].as_str()), (Some("roon_mobile"), Some("running"), Some("azure")));
    assert!(b.azure.calls.lock()[0].contains("definitions=16&branchName=refs/heads/fix/RRD-6420-hidden-spinner-ticker"));

    // Past expected_wait_mins the build is still queued: it still holds the stage and the merge.
    b.age(id, 45.0);
    b.poll();
    assert_eq!(b.phase(id), "checks");
    let e = b.merge(id).unwrap_err();
    assert!(e.contains("checks are still running: roon_mobile"), "{e}");
    assert!(!h.calls().iter().any(|c| c.starts_with("merge")), "{:?}", h.calls());

    // It starts and the host shows it: the host's check is the one that counts.
    h.rec.lock().checks = vec![Check { name: "roon_mobile".into(), state: "running".into(), url: None, at: None }];
    b.azure.set("inProgress", None);
    b.age(id, 50.0);
    b.poll();
    let c = checks(&b, id);
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(c[0]["via"].is_null());
    let asked = b.azure.calls.lock().len();
    b.age(id, 55.0);
    b.poll();
    assert_eq!(b.azure.calls.lock().len(), asked, "Azure isn't asked about a check the host shows");

    h.rec.lock().checks[0].state = "passed".into();
    b.poll();
    assert_eq!(b.phase(id), "merge");
    b.merge(id).unwrap();
}

#[test]
fn a_finished_azure_build_reads_as_passed_or_failed() {
    let b = board_with(|_| {});
    let id = b.pr_task();
    host(&b, approved("main"));
    b.azure.set("completed", Some("failed"));
    b.poll();
    let c = checks(&b, id);
    assert_eq!(c[0]["state"], "failed");
    assert_eq!(b.flow(id)["rec"]["failed"], json!(["roon_mobile"]));
    let e = b.merge(id).unwrap_err();
    assert!(e.contains("checks failed: roon_mobile"), "{e}");

    // A finished build isn't asked about again on this head.
    let asked = b.azure.calls.lock().len();
    b.age(id, 5.0);
    b.poll();
    assert_eq!(b.azure.calls.lock().len(), asked);

    let b = board_with(|_| {});
    let id = b.pr_task();
    let h = host(&b, approved("main"));
    b.azure.set("completed", Some("succeeded"));
    b.poll();
    assert_eq!(checks(&b, id)[0]["state"], "passed");
    assert_eq!(b.phase(id), "merge", "a passed build is the expected check, so nothing waits");
    b.merge(id).unwrap();
    assert!(h.calls().iter().any(|c| c.starts_with("merge")));
}

#[test]
fn with_no_build_in_azure_the_expected_wait_runs_out_as_before() {
    let b = board_with(|_| {});
    let id = b.pr_task();
    host(&b, approved("main"));
    b.poll();
    assert_eq!(b.phase(id), "checks");
    assert!(checks(&b, id).is_empty());
    assert_eq!(b.flow(id)["azure_runs"][HEAD]["runs"]["roon_mobile"], Value::Null);
    let e = b.merge(id).unwrap_err();
    assert!(e.contains("expected checks haven't posted: roon_mobile (it waits up to 30 min"), "{e}");
    b.age(id, 31.0);
    b.poll();
    assert_eq!(b.phase(id), "merge");
    b.merge(id).unwrap();
}

#[test]
fn a_stacked_pr_with_no_build_in_azure_waits_only_its_own_wait() {
    let b = board_with(|r| r.azure_stacked_wait_mins = Some(5.0));
    let repo = b.dir.path().join("webapp");
    let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap().status.success(), "git {args:?}");
    git(&["init", "-q"]);
    git(&["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main"]);
    let id = b.pr_task();
    let h = host(&b, approved("feature/tickers"));
    b.poll();
    assert_eq!(b.flow(id)["azure_runs"][HEAD]["stacked"], true);
    b.age(id, 6.0);
    b.poll();
    assert_eq!(b.phase(id), "merge", "a stacked PR with no build in Azure stops waiting after 5 minutes");

    // Into the default branch the full wait holds.
    h.rec.lock().base = "main".into();
    b.age(id, 6.0);
    b.poll();
    assert_eq!(b.flow(id)["azure_runs"][HEAD]["stacked"], false);
    assert_eq!(b.phase(id), "checks");

    // A stacked PR whose build is queued still waits for it.
    h.rec.lock().base = "feature/tickers".into();
    b.azure.set("notStarted", None);
    b.age(id, 45.0);
    b.poll();
    assert_eq!(b.phase(id), "checks");
}
