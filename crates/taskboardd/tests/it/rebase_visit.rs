//! A rebase follow-up on a done task with an open PR (#870): the terminal the board opens for it is the
//! task's PR visit, so the task stays done and that terminal's tb status, tb pr wait and tb submit find it.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, p, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    cfg.pr.wake = true;
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    fn report(&self, event: &str, session: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message))
    }
    fn add_session(&self, sid: &str, state: &str) {
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo, "status": {"state": state}})], &[])
            .unwrap();
    }
    fn t(&self, id: i64) -> taskboardd::util::Row {
        board::get_task(&self.app, id).unwrap()
    }
    /// A done task with PR #9 open, then the board's rebase follow-up opened terminal `s9` for it.
    fn rebase_visit(&self) -> i64 {
        let body = json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"});
        let id = api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &body).unwrap()["id"].as_i64().unwrap();
        self.add_session("s1", "working");
        self.report("tb.take", "s1", json!({"task": format!("T{id}")}));
        self.report("tb.done", "s1", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9"}));
        self.add_session("s1", "idle");
        let t = self.t(id);
        assert!(self.app.db.tx(|| prflow::follow_wake(&self.app, &t, "Rebase onto main.")).unwrap().is_some());
        self.add_session("s9", "working");
        self.app
            .db
            .x("UPDATE jobs SET state = 'done', target = json_set(coalesce(target, '{}'), '$.session', 's9') WHERE purpose = 'pr'", p![])
            .unwrap();
        id
    }
    fn prompt(&self, id: i64) -> Value {
        let text = format!("[task-board:T{id}] Work this task builds on has moved:\n- T1's branch moved. Rebase onto it.");
        self.report("hook.prompt", "s9", json!({"prompt": text}))
    }
}

#[test]
fn the_rebase_prompt_binds_its_terminal_and_leaves_the_task_done() {
    let b = board();
    let id = b.rebase_visit();
    let out = b.prompt(id);
    assert_eq!(out["task"], json!(format!("T{id}")));
    assert!(out.get("context").is_none(), "no 'already done, ask the owner' on the board's own prompt: {out}");
    let t = b.t(id);
    assert_eq!(t.s("status"), Some("done"));
    assert_eq!(session_of(&t).as_deref(), Some("s9"));
    assert_eq!(prflow::visited_by(&b.app, "s9").unwrap().map(|v| v.id()), Some(id));

    let status = b.report("tb.status", "s9", json!({}));
    assert_eq!(status["task"], json!(format!("T{id}")), "tb status names the task: {status}");

    let done = b.report("tb.done", "s9", json!({"summary": "Rebased"}));
    assert_eq!(done["task"], json!(format!("T{id}")), "a bare tb submit finds the task: {done}");
    assert_eq!(b.t(id).s("status"), Some("done"));

    api::dispatch(&b.app, "POST", &format!("/tasks/{id}/pr/wait"), &Query::new(), &json!({"by_session": "s9"}))
        .unwrap_or_else(|e| panic!("tb pr wait: {}", e.message));
}

#[test]
fn tb_take_in_the_rebase_terminal_doesnt_take_the_task_back() {
    let b = board();
    let id = b.rebase_visit();
    let out = b.report("tb.take", "s9", json!({"task": format!("T{id}")}));
    assert_eq!(out["task"], json!(format!("T{id}")));
    assert!(out["context"].as_str().unwrap_or("").contains(&format!("tb pr wait T{id}")), "{out}");
    let t = b.t(id);
    assert_eq!(t.s("status"), Some("done"), "it stays handed in");
    assert_eq!(session_of(&t).as_deref(), Some("s9"));
    let taken_back = b.app.db.count("SELECT COUNT(*) FROM events WHERE task_id = ? AND text LIKE 'Taken back%'", p![id]).unwrap();
    assert_eq!(taken_back, 0);
}

#[test]
fn tb_take_elsewhere_still_takes_a_done_task_back_for_its_pr() {
    let b = board();
    let id = b.rebase_visit();
    b.add_session("s5", "working");
    b.report("tb.take", "s5", json!({"task": format!("T{id}")}));
    assert_eq!(b.t(id).s("status"), Some("working"));
}

fn session_of(t: &taskboardd::util::Row) -> Option<String> {
    serde_json::from_str::<Value>(t.s("pr_flow").unwrap_or("{}")).ok()?["session"].as_str().map(|s| s.to_string())
}
