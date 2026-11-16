//! #1014: `tb pr wait` answers on the progress stream as it goes, and the PR's status says the board
//! watches it, and since when, once the visit is handed back.

use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, Check, FakeHost, Record};
use taskboardd::util::{parse_iso, now_ts, ApiError};
use taskboardd::{midna, p, prflow, progress, reports};

struct Board {
    app: Arc<App>,
    host: Arc<FakeHost>,
    repo: String,
    _dir: tempfile::TempDir,
}

const PR: &str = "https://bitbucket.org/acme/webapp/pull-requests/9";

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    cfg.pr.wake = true;
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    let host = FakeHost::new("bitbucket", rec(&[]));
    prhost::install(&app, host.clone());
    let b = Board { app, host, repo: repo.to_string_lossy().to_string(), _dir: dir };
    b.terminals(&["s1"]);
    b
}

fn rec(failed: &[&str]) -> Record {
    let checks = ["ci"].iter().map(|n| Check { name: n.to_string(), state: if failed.contains(n) { "failed" } else { "passed" }.into(), url: None, at: None }).collect();
    Record { state: "OPEN".into(), title: "Add login".into(), author: "me".into(), head: "h1".into(), branch: "feat".into(), base: "main".into(), base_head: "b1".into(), checks, ..Default::default() }
}

impl Board {
    fn terminals(&self, ids: &[&str]) {
        let open: Vec<Value> =
            ids.iter().map(|id| json!({"id": id, "name": format!("Term {id}"), "agent": "claude", "cwd": self.repo, "status": {"state": "idle"}})).collect();
        midna::sync(&self.app, &open, &[]).unwrap();
    }
    fn report(&self, sid: &str, event: &str, extra: Value) {
        let mut b = json!({"event": event, "session": sid, "claude_session": format!("c-{sid}"), "cwd": ""});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message));
    }
    fn post(&self, path: &str, body: Value) -> Result<Value, ApiError> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body)
    }
    fn pr(&self, id: i64) -> Value {
        api::dispatch(&self.app, "GET", &format!("/tasks/{id}/pr"), &Query::new(), &Value::Null).unwrap()
    }
    /// A task handed in from s1 whose PR's `ci` check fails: the board's wake opened s2 for the fix.
    fn visiting_in_s2(&self) -> i64 {
        let id = self.post("/tasks", json!({"title": "Add login", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
        self.report("s1", "tb.take", json!({"task": format!("T{id}")}));
        self.report("s1", "tb.done", json!({"summary": "Done", "pr": PR}));
        *self.host.rec.lock() = rec(&["ci"]);
        prflow::refresh(&self.app).unwrap();
        self.terminals(&["s1", "s2"]);
        self.app.db.x("UPDATE jobs SET state = 'done', target = '{\"session\": \"s2\"}' WHERE task_id = ? AND purpose = 'pr'", p![id]).unwrap();
        self.report("s2", "hook.prompt", json!({"prompt": format!("[task-board:T{id}] fix the checks")}));
        id
    }
}

#[test]
fn the_status_says_the_board_watches_the_pr_once_the_visit_is_handed_back() {
    let b = board();
    let id = b.visiting_in_s2();
    assert!(b.pr(id)["watching"].is_null(), "a visit is on: the agent has it, not the board");

    let before = now_ts();
    let v = b.post(&format!("/tasks/{id}/pr/wait"), json!({"by_session": "s2"})).unwrap();
    let since = v["watching"]["since"].as_str().unwrap_or_else(|| panic!("{v}"));
    assert!(parse_iso(since).unwrap() >= before.floor(), "{since}");
    assert_eq!(b.pr(id)["watching"], v["watching"], "tb pr status reads the same");
}

#[test]
fn a_merged_pr_isnt_watched_any_more() {
    let b = board();
    let id = b.visiting_in_s2();
    b.post(&format!("/tasks/{id}/pr/wait"), json!({"by_session": "s2"})).unwrap();
    assert!(b.pr(id)["watching"].is_object());
    b.app.db.x("UPDATE tasks SET pr_state = 'MERGED', pr_phase = 'merged' WHERE id = ?", p![id]).unwrap();
    assert!(b.pr(id)["watching"].is_null(), "a merged PR isn't watched any more");
}

#[test]
fn tb_pr_wait_says_each_stage_on_the_progress_stream() {
    let b = board();
    let id = b.visiting_in_s2();
    let lines = Arc::new(Mutex::new(vec![]));
    let l = lines.clone();
    let sink: progress::Sink = Arc::new(move |s: &str| l.lock().push(s.to_string()));
    progress::with(sink, || b.post(&format!("/tasks/{id}/pr/wait"), json!({"by_session": "s2"}))).unwrap();
    assert!(lines.lock().iter().any(|l| l == "Handed the PR back to the board to watch"), "{:?}", lines.lock());
}
