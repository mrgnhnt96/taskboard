//! PR builds stopped (#17): the owner's switch recording who, the cancels it triggers (the host's own
//! or a provider's command, retried, then an alert), checks counting as passed, the owner's push
//! builds, and build hooks for PRs on hosts the board doesn't watch.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, Check, FakeHost, Record};
use taskboardd::util::RowExt;
use taskboardd::{board, dispatch, hooks, midna, prbuilds, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board_with(f: impl FnOnce(&mut Config, &std::path::Path)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    f(&mut cfg, dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    Board { app, dir }
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn report(&self, event: &str, extra: Value) {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap();
    }
    fn pr_task(&self, url: &str) -> i64 {
        let id = self.post("/tasks", json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")}));
        self.report("tb.done", json!({"summary": "Done", "pr": url}));
        id
    }
    fn task(&self, id: i64) -> serde_json::Map<String, Value> {
        board::get_task(&self.app, id).unwrap()
    }
    fn flow(&self, id: i64) -> Value {
        serde_json::from_str(&self.task(id).st("pr_flow")).unwrap()
    }
    fn queue(&self) -> Vec<Value> {
        serde_json::from_str(&self.app.db.get_setting("pr_build_cancels").unwrap().unwrap_or("[]".into())).unwrap()
    }
    /// Makes every waiting cancel due now.
    fn due_now(&self) {
        let q: Vec<Value> = self.queue().into_iter().map(|mut x| {
            x["next_at"] = json!("2000-01-01T00:00:00Z");
            x
        }).collect();
        self.app.db.set_setting("pr_build_cancels", Some(&Value::Array(q).to_string())).unwrap();
    }
}

const GH: &str = "https://github.com/acme/webapp/pull/9";

fn running() -> Record {
    Record {
        state: "OPEN".into(),
        author: "me".into(),
        head: "h1".into(),
        base: "main".into(),
        checks: vec![Check { name: "build".into(), state: "running".into(), url: Some("https://github.com/acme/webapp/actions/runs/55/job/1".into()) }],
        threads: vec![],
        ..Default::default()
    }
}

#[test]
fn stopping_records_who_and_the_checks_count_as_passed_while_builds_are_cancelled() {
    let b = board_with(|_, _| {});
    let id = b.pr_task(GH);
    let h = FakeHost::new("github", running());
    prhost::install(&b.app, h.clone());
    prflow::refresh(&b.app).unwrap();
    assert_eq!(b.task(id).st("pr_phase"), "checks");

    let v = b.post("/pr-builds", json!({"stopped": true, "who": "Sam", "reason": "CI minutes ran out"}));
    assert_eq!((v["stopped"].as_bool(), v["by"].as_str(), v["reason"].as_str()), (Some(true), Some("Sam"), Some("CI minutes ran out")));
    assert_eq!(b.get("/state")["pr_builds"]["stopped"], true);
    assert_eq!(b.queue().len(), 1, "stopping cancels what's already running");

    taskboardd::runner::prs(&b.app).unwrap();
    assert_eq!(b.task(id).st("pr_phase"), "review", "the checks count as passed");
    assert_eq!(b.task(id).st("pr_build"), "Builds stopped");
    prbuilds::tick(&b.app).unwrap();
    assert!(h.calls().contains(&"cancel h1".to_string()), "{:?}", h.calls());
    assert!(b.queue().is_empty());
    assert!(b.flow(id)["builds_cancelled"]["h1"].is_string());
    taskboardd::runner::prs(&b.app).unwrap();
    assert!(b.queue().is_empty(), "a push's builds are cancelled once");

    let v = b.post("/pr-builds", json!({"stopped": false, "who": "Sam"}));
    assert_eq!((v["stopped"].as_bool(), v["resumed_by"].as_str()), (Some(false), Some("Sam")));
    prflow::refresh(&b.app).unwrap();
    assert_eq!(b.task(id).st("pr_phase"), "checks", "resumed: the checks count again");
    assert!(api::dispatch(&b.app, "POST", "/pr-builds", &Query::new(), &json!({})).is_err());
}

#[test]
fn a_build_event_runs_the_provider_s_cancel_command_and_retries_then_alerts() {
    let b = board_with(|c, dir| {
        c.pr_builds.cancel.insert("azure".into(), format!("echo \"$TB_PROVIDER $TB_HEAD $TB_PR_NUM\" >> '{}'; exit 1", dir.join("cancels.log").display()));
    });
    let id = b.pr_task(GH);
    b.post("/pr-builds", json!({"stopped": true, "who": "Sam"}));
    let v = b.post("/prs/event", json!({"url": GH, "kind": "build", "state": "started", "head": "h2", "provider": "azure"}));
    assert_eq!(v["builds"]["cancel"], "queued");
    b.post("/prs/event", json!({"url": GH, "kind": "build", "state": "running", "head": "h2", "provider": "azure"}));
    assert_eq!(b.queue().len(), 1, "the same push is cancelled once");

    prbuilds::tick(&b.app).unwrap();
    let log = std::fs::read_to_string(b.dir.path().join("cancels.log")).unwrap();
    assert_eq!(log.trim(), "azure h2 9");
    assert_eq!(b.queue()[0]["tries"], 1);
    prbuilds::tick(&b.app).unwrap();
    assert_eq!(b.queue()[0]["tries"], 1, "the next try waits 10 seconds");
    for _ in 0..4 {
        b.due_now();
        prbuilds::tick(&b.app).unwrap();
    }
    assert!(b.queue().is_empty(), "five tries, then it gives up");
    let alerts = dispatch::alerts(&b.app);
    assert_eq!(alerts.len(), 1);
    let text = alerts[0]["text"].as_str().unwrap();
    assert!(text.contains("couldn't cancel the builds of PR #9 at h2 after 5 tries"), "{text}");
    assert_eq!(alerts[0]["task"], format!("T{id}"));
}

#[test]
fn a_ci_without_a_cancel_command_alerts_at_once() {
    let b = board_with(|_, _| {});
    b.pr_task(GH);
    b.post("/pr-builds", json!({"stopped": true}));
    assert_eq!(b.get("/pr-builds")["by"], "Sam", "the owner, when nobody's named");
    b.post("/prs/event", json!({"url": GH, "kind": "build", "state": "started", "head": "h3", "provider": "jenkins"}));
    prbuilds::tick(&b.app).unwrap();
    let alerts = dispatch::alerts(&b.app);
    assert!(alerts[0]["text"].as_str().unwrap().contains("there's no cancel command for jenkins in [pr_builds.cancel]"), "{}", alerts[0]);
}

#[test]
fn the_owner_s_own_push_builds_are_cancelled_too() {
    let b = board_with(|c, dir| {
        c.owner_emails = vec!["sam@acme.dev".into()];
        c.pr_builds.cancel.insert("github".into(), format!("echo \"$TB_BRANCH $TB_HEAD\" >> '{}'", dir.join("cancels.log").display()));
    });
    b.post("/pr-builds", json!({"stopped": true}));
    let v = b.post("/prs/event", json!({"kind": "build", "state": "started", "repo": "acme/webapp", "host": "github", "branch": "wip", "head": "p1",
                                        "author": "someone@acme.dev"}));
    assert_eq!(v["builds"]["owners"], false);
    assert!(b.queue().is_empty(), "not the owner's push");
    let v = b.post("/prs/event", json!({"kind": "build", "state": "started", "repo": "acme/webapp", "host": "github", "branch": "wip", "head": "p1",
                                        "author": "Sam@Acme.dev"}));
    assert_eq!(v["builds"]["cancel"], "queued");
    prbuilds::tick(&b.app).unwrap();
    assert_eq!(std::fs::read_to_string(b.dir.path().join("cancels.log")).unwrap().trim(), "wip p1");
    assert!(dispatch::alerts(&b.app).is_empty());
}

#[test]
fn build_hooks_fire_for_a_pr_on_a_host_the_board_does_not_watch() {
    let b = board_with(|c, _| {
        std::fs::write(
            hooks::path(c),
            json!({"hooks": {"pr.checks": [{"hooks": [{"type": "command", "command": "echo '{\"decision\": \"skip\", \"reason\": \"cancelled them\"}'"}]}]}}).to_string(),
        )
        .unwrap();
    });
    let id = b.pr_task("https://gitlab.com/acme/webapp/-/merge_requests/4");
    assert_eq!(b.task(id).st("pr_host"), "gitlab");
    let v = b.post("/prs/event", json!({"url": "https://gitlab.com/acme/webapp/-/merge_requests/4", "kind": "build", "state": "started", "head": "g1"}));
    assert_eq!(v["task"], format!("T{id}"));
    assert_eq!(v["builds"]["hook"], "skip");
    assert_eq!(b.flow(id)["skip_checks"]["g1"], "cancelled them");
    let v = b.post("/prs/event", json!({"url": "https://gitlab.com/acme/webapp/-/merge_requests/4", "kind": "build", "state": "running", "head": "g1"}));
    assert!(v["builds"].get("hook").is_none(), "asked once per push");
}
