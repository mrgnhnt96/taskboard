//! A project names the checks, or the stages inside a check's build, that must pass to merge (GitHub
//! #928): another failed or cancelled one doesn't hold an approved PR. An approved PR that failed checks
//! hold for over a day tells the owner, naming the failed stage.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::{Config, PrProject};
use taskboardd::prhost::{self, Check, FakeHost, Record};
use taskboardd::util::{iso, parse_iso, RowExt};
use taskboardd::{board, dispatch, midna, p, prflow, reports};

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
    let stages = dir.path().join("stages.json");
    cfg.pr.projects.insert("webapp".into(), PrProject { failures_cmd: Some(format!("cat '{}'", stages.display())), ..Default::default() });
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
    fn phase(&self, id: i64) -> String {
        self.task(id).st("pr_phase")
    }
    fn stages(&self, stages: Value) {
        std::fs::write(self.dir.path().join("stages.json"), json!({"steps": [], "stages": stages}).to_string()).unwrap();
    }
    fn require(&self, names: Value) -> Value {
        self.post("projects/webapp", json!({"required": names})).unwrap()
    }
    fn read(&self, id: i64, rec: &Record) {
        prflow::apply(&self.app, id, &rec.to_value()).unwrap();
    }
    /// Moves the time the PR was first seen held back by `hours`, as if that long had gone by.
    fn age_hold(&self, id: i64, hours: f64) {
        let mut f: Value = serde_json::from_str(&self.task(id).st("pr_flow")).unwrap();
        let at = parse_iso(f["held"]["at"].as_str().expect("the hold is noted")).unwrap();
        f["held"]["at"] = json!(iso(at - hours * 3600.0));
        self.app.db.x("UPDATE tasks SET pr_flow = ? WHERE id = ?", p![f.to_string(), id]).unwrap();
    }
    fn held_alerts(&self, num: i64) -> Vec<Value> {
        dispatch::alerts(&self.app).into_iter().filter(|a| a["text"].as_str().is_some_and(|t| t.starts_with(&format!("PR #{num} ")) && t.contains("approved but"))).collect()
    }
}

fn record(head: &str, checks: &[(&str, &str)], approved: bool) -> Record {
    Record {
        state: "OPEN".into(),
        author: "me".into(),
        head: head.into(),
        branch: "feat".into(),
        base: "main".into(),
        checks: checks.iter().map(|(n, s)| Check { name: n.to_string(), state: s.to_string(), url: None, at: None }).collect(),
        review_decision: if approved { "APPROVED".into() } else { String::new() },
        approvals: if approved { 2 } else { 0 },
        ..Default::default()
    }
}

fn stage(name: &str, result: &str) -> Value {
    json!({"name": name, "result": result})
}

#[test]
fn a_failed_stage_that_isnt_required_doesnt_hold_an_approved_pr() {
    let b = board();
    let shown = b.require(json!(["Lint", "Unit tests"]));
    assert_eq!(shown["pr_rules"]["required"], json!(["Lint", "Unit tests"]));

    // The reported case: lint and unit tests passed, the upload stage was cancelled, so the build failed.
    b.stages(json!([stage("Lint", "succeeded"), stage("Unit tests", "succeeded"), stage("Upload", "canceled")]));
    let host = FakeHost::new("bitbucket", record("aaaa111122223333", &[("CI", "failed")], true));
    prhost::install(&b.app, host.clone());
    let id = b.pr_task(61);
    b.read(id, &record("aaaa111122223333", &[("CI", "failed")], true));
    assert_eq!(b.phase(id), "merge");
    assert_eq!(b.task(id).s("pr_build"), Some("Failed, not required"));
    b.post(&format!("tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap();
    assert_eq!(host.rec.lock().state, "MERGED");

    // A required stage failed: the check holds it.
    b.stages(json!([stage("Lint", "succeeded"), stage("Unit tests", "failed"), stage("Upload", "canceled")]));
    let red = record("bbbb111122223333", &[("CI", "failed")], true);
    *host.rec.lock() = red.clone();
    let id = b.pr_task(62);
    b.read(id, &red);
    assert_eq!(b.phase(id), "fix");
    let e = b.post(&format!("tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains("checks failed: CI"), "{e}");

    // A required stage that never finished hasn't passed.
    b.stages(json!([stage("Lint", "succeeded"), stage("Unit tests", "skipped")]));
    let id = b.pr_task(63);
    b.read(id, &record("cccc111122223333", &[("CI", "failed")], true));
    assert_eq!(b.phase(id), "fix");

    // No required name matches anything (a misspelling): every failed check still holds.
    b.require(json!(["unit-tests"]));
    b.stages(json!([stage("Lint", "succeeded"), stage("Unit tests", "succeeded"), stage("Upload", "canceled")]));
    let id = b.pr_task(64);
    b.read(id, &record("dddd111122223333", &[("CI", "failed")], true));
    assert_eq!(b.phase(id), "fix");

    // Back to every failed check holding.
    b.require(json!(["Lint", "Unit tests"]));
    assert!(b.require(Value::Null)["pr_rules"]["required"].is_null());
    let id = b.pr_task(65);
    b.read(id, &record("eeee111122223333", &[("CI", "failed")], true));
    assert_eq!(b.phase(id), "fix");
}

#[test]
fn a_failed_check_that_isnt_required_doesnt_hold_an_approved_pr() {
    let b = board();
    b.require(json!(["build"]));
    b.stages(json!([stage("Upload", "failed")]));
    prhost::install(&b.app, FakeHost::new("bitbucket", record("aaaa111122223333", &[], true)));

    let id = b.pr_task(66);
    b.read(id, &record("aaaa111122223333", &[("build", "passed"), ("deploy", "failed")], true));
    assert_eq!(b.phase(id), "merge");

    let id = b.pr_task(67);
    b.read(id, &record("bbbb111122223333", &[("build", "failed"), ("deploy", "passed")], true));
    assert_eq!(b.phase(id), "fix");
}

#[test]
fn an_approved_pr_held_by_failed_checks_for_a_day_tells_the_owner_once() {
    let b = board();
    b.stages(json!([stage("Lint", "succeeded"), stage("Upload", "canceled")]));
    prhost::install(&b.app, FakeHost::new("bitbucket", record("aaaa111122223333", &[], true)));

    let id = b.pr_task(71);
    b.read(id, &record("aaaa111122223333", &[("CI", "failed")], true));
    assert_eq!(b.phase(id), "fix");
    let waiting = b.pr_task(72);
    b.read(waiting, &record("aaaa111122223333", &[("CI", "failed")], false));
    assert_eq!(b.phase(waiting), "fix");

    prflow::held_alerts(&b.app).unwrap();
    assert!(b.task(waiting).st("pr_flow").find("\"held\"").is_none(), "a PR that isn't approved isn't held by its checks");
    b.age_hold(id, 23.0);
    prflow::held_alerts(&b.app).unwrap();
    assert!(b.held_alerts(71).is_empty(), "not a day yet");

    b.age_hold(id, 2.0);
    prflow::held_alerts(&b.app).unwrap();
    let alerts = b.held_alerts(71);
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    let text = alerts[0]["text"].as_str().unwrap();
    assert!(text.contains("is approved but failed checks have held it for 1 day: CI (stage Upload: canceled)"), "{text}");
    assert!(text.contains("Rebase it onto main and push to start a new build"), "{text}");
    assert!(text.contains(&format!("tb pr merge T{id} --override")), "{text}");
    let labels: Vec<&str> = alerts[0]["actions"].as_array().unwrap().iter().filter_map(|a| a["label"].as_str()).collect();
    assert_eq!(labels, ["Rebase and push", "Merge anyway"]);
    assert!(b.held_alerts(72).is_empty());

    prflow::held_alerts(&b.app).unwrap();
    assert_eq!(b.held_alerts(71).len(), 1, "once per push");

    // A new push starts the day again.
    dispatch::clear_alerts(&b.app, Some(id), None).unwrap();
    b.read(id, &record("ffff111122223333", &[("CI", "failed")], true));
    prflow::held_alerts(&b.app).unwrap();
    b.age_hold(id, 12.0);
    prflow::held_alerts(&b.app).unwrap();
    assert!(b.held_alerts(71).is_empty());
    b.age_hold(id, 13.0);
    prflow::held_alerts(&b.app).unwrap();
    assert_eq!(b.held_alerts(71).len(), 1);
}
