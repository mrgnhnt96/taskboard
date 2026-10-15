//! tb pr merge on the progress stream: the board says each stage as it finishes it, so tb's time
//! limit is per stage and a slow host doesn't make it give up while the merge goes on (PR #342).

use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::json;
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, Check, FakeHost, Record};
use taskboardd::{midna, progress, reports};

fn board() -> (Arc<App>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.agents_merge = Some(true);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    (app, dir)
}

fn pr_task(app: &Arc<App>, dir: &std::path::Path, num: i64) -> i64 {
    let t = api::dispatch(app, "POST", "/tasks", &Query::new(), &json!({"title": format!("Ship {num}"), "detail": "Do it.", "project": "webapp"})).unwrap();
    let id = t["id"].as_i64().unwrap();
    let sid = format!("s{num}");
    let repo = dir.join("webapp").to_string_lossy().to_string();
    midna::sync(app, &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
    for (event, extra) in [
        ("tb.take", json!({"task": format!("T{id}")})),
        ("tb.done", json!({"summary": "Done", "pr": format!("https://bitbucket.org/acme/webapp/pull-requests/{num}")})),
    ] {
        let mut b = json!({"event": event, "session": sid, "claude_session": format!("c-{sid}"), "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(app, b, false).unwrap();
    }
    id
}

fn record(checks: &[(&str, &str)]) -> Record {
    Record {
        state: "OPEN".into(),
        author: "me".into(),
        head: "abcdef1234567890".into(),
        base: "main".into(),
        checks: checks.iter().map(|(n, s)| Check { name: n.to_string(), state: s.to_string(), url: None, at: None }).collect(),
        review_decision: "APPROVED".into(),
        approvals: 2,
        ..Default::default()
    }
}

/// Merges T<id> as an agent, with its progress lines collected.
fn merge(app: &Arc<App>, id: i64) -> (Result<serde_json::Value, String>, Vec<String>) {
    let lines = Arc::new(Mutex::new(vec![]));
    let l = lines.clone();
    let sink: progress::Sink = Arc::new(move |s: &str| l.lock().push(s.to_string()));
    let res = progress::with(sink, || api::dispatch(app, "POST", &format!("tasks/T{id}/pr/merge"), &Query::new(), &json!({"agent": true})));
    let out = lines.lock().clone();
    (res.map_err(|e| e.message), out)
}

#[test]
fn a_merge_says_each_stage_as_it_goes() {
    let (app, dir) = board();
    let host = FakeHost::new("bitbucket", record(&[("build", "passed")]));
    prhost::install(&app, host.clone());
    let id = pr_task(&app, dir.path(), 9);
    let (res, lines) = merge(&app, id);
    res.unwrap();
    assert_eq!(lines, ["Read PR #9 from Bitbucket", "Nothing blocks the merge", "Merged PR #9 on Bitbucket"]);
    assert!(host.calls().iter().any(|c| c.starts_with("merge")), "{:?}", host.calls());
}

#[test]
fn a_refused_merge_stops_after_the_read() {
    let (app, dir) = board();
    let host = FakeHost::new("bitbucket", record(&[("build", "failed")]));
    prhost::install(&app, host.clone());
    let id = pr_task(&app, dir.path(), 10);
    let (res, lines) = merge(&app, id);
    assert!(res.is_err());
    assert_eq!(lines, ["Read PR #10 from Bitbucket"]);
    assert!(!host.calls().iter().any(|c| c.starts_with("merge")));
}
