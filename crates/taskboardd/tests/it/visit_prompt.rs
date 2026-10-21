//! #592: a terminal a PR visit opened isn't closed as finished before its prompt reaches it, and a
//! visit whose prompt never lands is tried again instead of leaving the PR unvisited.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, Check, FakeHost, Record};
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, midna, p, prflow, reports};

struct Board {
    app: Arc<App>,
    host: Arc<FakeHost>,
    repo: String,
    _dir: tempfile::TempDir,
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    cfg.pr.wake = true;
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    let host = FakeHost::new("bitbucket", rec("h1", &[]));
    prhost::install(&app, host.clone());
    let b = Board { app, host, repo: repo.to_string_lossy().to_string(), _dir: dir };
    b.terminals(&["s1"]);
    b
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
    fn task(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn flow(&self, id: i64) -> Value {
        serde_json::from_str(self.task(id).s("pr_flow").unwrap_or("{}")).unwrap()
    }
    fn closing(&self) -> Vec<String> {
        prflow::pr_tabs_to_close(&self.app).unwrap().into_iter().map(|(s, _, _)| s.st("id")).collect()
    }
    /// A task done in s1 whose PR's `ci` check fails, and the board's wake opened s2 for the fix, idle
    /// since long ago with no prompt yet.
    fn woke_into_s2(&self) -> i64 {
        let id = api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &json!({"title": "Add login", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
        self.report("s1", "tb.take", json!({"task": format!("T{id}")}));
        self.report("s1", "tb.done", json!({"summary": "Done", "pr": "https://bitbucket.org/acme/webapp/pull-requests/9"}));
        self.app.db.x("UPDATE tasks SET auto_close = 1 WHERE id = ?", p![id]).unwrap();
        *self.host.rec.lock() = rec("h1", &["ci"]);
        prflow::refresh(&self.app).unwrap();
        assert!(self.flow(id)["woke"].is_string(), "{}", self.flow(id));
        self.terminals(&["s1", "s2"]);
        self.app.db.x("UPDATE jobs SET state = 'done', target = '{\"session\": \"s2\"}' WHERE task_id = ? AND purpose = 'pr'", p![id]).unwrap();
        self.app.db.x("UPDATE sessions SET status = 'idle', status_at = '2000-01-01T00:00:00Z' WHERE id = 's2'", p![]).unwrap();
        id
    }
}

fn rec(head: &str, failed: &[&str]) -> Record {
    let checks = ["ci"].iter().map(|n| Check { name: n.to_string(), state: if failed.contains(n) { "failed" } else { "passed" }.into(), url: None, at: None }).collect();
    Record { state: "OPEN".into(), title: "Add login".into(), author: "me".into(), head: head.into(), branch: "feat".into(), base: "main".into(), base_head: "b1".into(), checks, ..Default::default() }
}

#[test]
fn an_idle_tab_the_visit_opened_stays_open_until_its_prompt_reaches_it() {
    let b = board();
    let id = b.woke_into_s2();
    assert!(!b.closing().contains(&"s2".to_string()), "idle before its prompt isn't a finished turn");

    b.report("s2", "hook.prompt", json!({"prompt": format!("[task-board:T{id}] fix the checks")}));
    assert_eq!(b.flow(id)["session"], "s2", "it picked up the visit");
    prflow::prompts_missed(&b.app).unwrap();
    assert!(b.flow(id)["woke"].is_string(), "a visit whose prompt landed is left to run: {}", b.flow(id));
}

#[test]
fn a_prompt_that_never_lands_is_a_failed_wake_the_board_tries_again() {
    for gone in [false, true] {
        let b = board();
        let id = b.woke_into_s2();
        if gone {
            b.terminals(&["s1"]);
            b.terminals(&["s1"]);
            assert_eq!(board::get_session(&b.app, Some("s2")).unwrap().unwrap().st("status"), "gone");
        }
        prflow::prompts_missed(&b.app).unwrap();
        let f = b.flow(id);
        assert!(f["woke"].is_null(), "gone = {gone}: {f}");
        assert_eq!((f["wake_tries"].as_i64(), f["reread_at"].is_string()), (Some(1), true), "it reads the PR again to retry: {f}");
        let log = b.app.db.q("SELECT text FROM events WHERE task_id = ? ORDER BY id DESC LIMIT 1", p![id]).unwrap();
        assert!(log[0].st("text").starts_with("Couldn't reopen it for PR #9"), "{:?}", log);
        if !gone {
            assert!(b.closing().contains(&"s2".to_string()), "the stale tab closes once the wake is given up");
        }
    }
}
