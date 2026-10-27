//! `tb pr merge` refuses a PR that carries another open PR's head commit: the host would mark that PR
//! merged with it, out from under whoever still works on it (lens#22 carried #21's 8609e36 and merged it).

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, Check, FakeHost, Record};
use taskboardd::util::RowExt;
use taskboardd::{board, midna, prflow, reports};

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
    /// A done task with GitHub PR `num` in acme/webapp, its record's head `head`.
    fn pr_task(&self, num: i64, head: &str) -> i64 {
        let id = self.post("/tasks", json!({"title": format!("Ship {num}"), "detail": "Do it.", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
        let sid = format!("s{num}");
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[])
            .unwrap();
        self.report("tb.take", &sid, json!({"task": format!("T{id}")}));
        self.report("tb.done", &sid, json!({"summary": "Done", "pr": format!("https://github.com/acme/webapp/pull/{num}")}));
        let t = board::get_task(&self.app, id).unwrap();
        self.app.db.tx(|| prflow::step(&self.app, &t, &record(head).to_value())).unwrap();
        id
    }
    fn phase(&self, id: i64) -> Option<String> {
        board::get_task(&self.app, id).unwrap().s("pr_phase").map(str::to_string)
    }
}

fn record(head: &str) -> Record {
    Record {
        state: "OPEN".into(),
        author: "me".into(),
        head: head.into(),
        base: "main".into(),
        checks: vec![Check { name: "ci".into(), state: "passed".into(), url: None, at: None }],
        review_decision: "APPROVED".into(),
        approvals: 2,
        ..Default::default()
    }
}

const HEAD_21: &str = "8609e36c0ffee0000000000000000000000000aa";
const HEAD_22: &str = "22222220000000000000000000000000000000bb";

#[test]
fn a_pr_carrying_another_open_prs_head_is_refused_for_owner_and_agent() {
    let b = board();
    let host = FakeHost::new("github", record(HEAD_22));
    prhost::install(&b.app, host.clone());
    let lens21 = b.pr_task(21, HEAD_21);
    let lens22 = b.pr_task(22, HEAD_22);
    *host.commits.lock() = vec!["1111111000000000000000000000000000000000".into(), HEAD_21.into(), HEAD_22.into()];

    for body in [json!({}), json!({"agent": true})] {
        let e = b.post(&format!("tasks/T{lens22}/pr/merge"), body).unwrap_err();
        assert!(e.starts_with("PR #22 can't merge yet: it carries "), "{e}");
        assert!(e.contains(&format!("T{lens21}'s PR #21 (its head 8609e36), which hasn't merged")), "{e}");
        assert!(e.contains(&format!("tb wait-for T{lens21} --merged")), "{e}");
        assert!(e.contains(&format!("tb task set T{lens22} --stack-on T{lens21}")), "{e}");
    }
    assert!(host.calls().iter().any(|c| c == "commits 22"), "{:?}", host.calls());
    assert!(!host.calls().iter().any(|c| c.starts_with("merge")), "nothing merged: {:?}", host.calls());
    assert_ne!(b.phase(lens21).as_deref(), Some("merged"));
    assert_ne!(b.phase(lens22).as_deref(), Some("merged"));
}

#[test]
fn a_pr_with_only_its_own_commits_merges() {
    let b = board();
    let host = FakeHost::new("github", record(HEAD_22));
    prhost::install(&b.app, host.clone());
    b.pr_task(21, HEAD_21);
    let lens22 = b.pr_task(22, HEAD_22);
    *host.commits.lock() = vec!["1111111000000000000000000000000000000000".into(), HEAD_22.into()];

    let out = b.post(&format!("tasks/T{lens22}/pr/merge"), json!({})).unwrap();
    assert_eq!(out["merged"], true);
    assert!(host.calls().iter().any(|c| c == "commits 22"), "{:?}", host.calls());
    assert!(host.calls().iter().any(|c| c.starts_with("merge")), "{:?}", host.calls());
}

#[test]
fn a_merged_pr_s_head_doesnt_hold_and_alone_the_commits_arent_read() {
    let b = board();
    let host = FakeHost::new("github", record(HEAD_22));
    prhost::install(&b.app, host.clone());
    let lens21 = b.pr_task(21, HEAD_21);
    b.app.db.x("UPDATE tasks SET pr_phase = 'merged' WHERE id = ?", taskboardd::p![lens21]).unwrap();
    let lens22 = b.pr_task(22, HEAD_22);
    *host.commits.lock() = vec![HEAD_21.into(), HEAD_22.into()];

    let out = b.post(&format!("tasks/T{lens22}/pr/merge"), json!({})).unwrap();
    assert_eq!(out["merged"], true);
    assert!(!host.calls().iter().any(|c| c.starts_with("commits")), "no other open PR to look for: {:?}", host.calls());
}
