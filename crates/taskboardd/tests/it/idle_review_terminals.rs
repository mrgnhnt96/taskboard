//! #800: idle terminals of In review tasks close once their visit ends, however it ends, and the owner
//! can sweep the ones left over.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, Check, FakeHost, PrRef, Record};
use taskboardd::util::{now_ts, ApiError, Row, RowExt};
use taskboardd::{board, midna, p, prflow, reports, runner};

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
    let host = FakeHost::new("bitbucket", rec(&[]));
    prhost::install(&app, host.clone());
    let b = Board { app, host, repo: repo.to_string_lossy().to_string(), _dir: dir };
    b.terminals(&["s1"]);
    b
}

const PR: &str = "https://bitbucket.org/acme/webapp/pull-requests/9";

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
    fn task(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn new_task(&self) -> i64 {
        self.post("/tasks", json!({"title": "Add login", "project": "webapp"})).unwrap()["id"].as_i64().unwrap()
    }
    /// The board's own pass over the PR terminals it can close.
    fn closing(&self) -> Vec<String> {
        prflow::pr_tabs_to_close(&self.app).unwrap().into_iter().map(|(s, _, _)| s.st("id")).collect()
    }
    fn closes(&self) -> Vec<String> {
        let jobs = self.app.db.q("SELECT args FROM jobs WHERE kind = 'close' ORDER BY id", p![]).unwrap();
        jobs.iter().filter_map(|j| board::job_args(j).s("session").map(str::to_string)).collect()
    }
    /// Settled: idle since long ago. A change of status stamps `status_at`, so it's set back after.
    fn idle(&self, sid: &str) {
        self.app.db.x("UPDATE sessions SET status = 'idle' WHERE id = ?", p![sid]).unwrap();
        self.app.db.x("UPDATE sessions SET status_at = '2000-01-01T00:00:00Z' WHERE id = ?", p![sid]).unwrap();
    }
    /// A task done in s1 whose PR's `ci` check fails: the board's wake opened s2 for the fix, and its
    /// prompt landed there.
    fn visiting_in_s2(&self) -> i64 {
        let id = self.new_task();
        self.report("s1", "tb.take", json!({"task": format!("T{id}")}));
        self.report("s1", "tb.done", json!({"summary": "Done", "pr": PR}));
        self.app.db.x("UPDATE tasks SET auto_close = 1 WHERE id = ?", p![id]).unwrap();
        *self.host.rec.lock() = rec(&["ci"]);
        prflow::refresh(&self.app).unwrap();
        self.terminals(&["s1", "s2"]);
        self.app.db.x("UPDATE jobs SET state = 'done', target = '{\"session\": \"s2\"}' WHERE task_id = ? AND purpose = 'pr'", p![id]).unwrap();
        self.report("s2", "hook.prompt", json!({"prompt": format!("[task-board:T{id}] fix the checks")}));
        id
    }
}

fn rec(failed: &[&str]) -> Record {
    let checks = ["ci"].iter().map(|n| Check { name: n.to_string(), state: if failed.contains(n) { "failed" } else { "passed" }.into(), url: None, at: None }).collect();
    Record { state: "OPEN".into(), title: "Add login".into(), author: "me".into(), head: "h1".into(), branch: "feat".into(), base: "main".into(), base_head: "b1".into(), checks, ..Default::default() }
}

#[test]
fn a_visit_the_board_ends_under_a_rate_limit_closes_its_terminal() {
    let b = board();
    let id = b.visiting_in_s2();
    b.idle("s2");
    assert!(!b.closing().contains(&"s2".to_string()), "the visit is still on");

    *b.host.held.lock() = Some(now_ts() + 600.0);
    let t = b.task(id);
    let pr = PrRef { host: "bitbucket".into(), repo: t.st("pr_repo"), num: 9, url: t.st("pr_url") };
    let e = prflow::wait_out_limit(&b.app, id, &pr, &json!({"by_session": "s2"}), &t.st("pr_phase"), ApiError::new(502, "Couldn't read PR #9."));
    assert!(e.message.contains("The board ended this visit"), "{}", e.message);
    b.idle("s2");
    assert!(b.closing().contains(&"s2".to_string()), "nothing brings the agent back to this terminal");
}

#[test]
fn a_compacted_terminal_still_closes_once_its_visit_ends() {
    let b = board();
    let id = b.visiting_in_s2();
    b.report("s2", "hook.session_start", json!({"source": "compact"}));
    b.post(&format!("/tasks/{id}/pr/wait"), json!({"by_session": "s2"})).unwrap();
    // Midna shows a compacted terminal as `idle · session started`, not as a finished turn.
    let open = json!([{"id": "s1", "name": "Term s1", "agent": "claude", "cwd": b.repo, "status": {"state": "idle"}},
                      {"id": "s2", "name": "Term s2", "agent": "claude", "cwd": b.repo, "status": {"state": "idle", "detail": "session started"}}]);
    midna::sync(&b.app, open.as_array().unwrap(), &[]).unwrap();
    b.idle("s2");
    assert!(b.closing().contains(&"s2".to_string()), "{:?}", b.closing());
}

#[test]
fn tb_pr_wait_on_a_task_not_handed_in_says_to_hand_it_in_and_the_terminal_then_closes() {
    let b = board();
    let id = b.new_task();
    b.report("s1", "tb.take", json!({"task": format!("T{id}")}));
    // The runner started it again in s1 after its first terminal was lost; its PR was linked before.
    b.app.db.insert("jobs", taskboardd::fields!["kind" => "agent", "purpose" => "start", "state" => "done", "task_id" => id, "target" => json!({"session": "s1"}).to_string()]).unwrap();
    b.app.db.x("UPDATE tasks SET pr_host = 'bitbucket', pr_repo = 'acme/webapp', pr_num = 9, pr_url = ?, pr_state = 'OPEN', pr_phase = 'checks' WHERE id = ?", p![PR, id]).unwrap();

    let e = b.post(&format!("/tasks/{id}/pr/wait"), json!({"by_session": "s1"})).unwrap_err();
    assert_eq!(e.status, 409);
    assert!(e.message.contains(&format!("tb submit \"<summary>\" --pr {PR}")), "{}", e.message);
    assert_eq!(b.task(id).st("status"), "working", "nothing changed");

    b.report("s1", "tb.done", json!({"summary": "Done", "pr": PR}));
    b.app.db.x("UPDATE tasks SET auto_close = 1 WHERE id = ?", p![id]).unwrap();
    b.idle("s1");
    runner::auto_close_done(&b.app).unwrap();
    assert_eq!(b.closes(), ["s1"]);
}

#[test]
fn tb_close_review_closes_idle_in_review_terminals_that_wait_on_nobody() {
    let b = board();
    let id = b.visiting_in_s2();
    // The owner turned auto-close off for it: the board's own pass leaves its terminals.
    b.app.db.x("UPDATE tasks SET auto_close = 0 WHERE id = ?", p![id]).unwrap();
    b.post(&format!("/tasks/{id}/pr/wait"), json!({"by_session": "s2"})).unwrap();
    b.terminals(&["s1", "s2", "s3"]);
    b.idle("s1");
    b.idle("s2");
    assert!(b.closing().is_empty());

    // s1 asks the owner something: it waits on them, so it stays.
    b.app.db.x("UPDATE sessions SET status = 'needs' WHERE id = 's1'", p![]).unwrap();
    let v = b.post("/done/close-terminals", json!({"review": true})).unwrap();
    assert_eq!(v["jobs"], 1, "{v}");
    assert_eq!(b.closes(), ["s2"]);
    assert_eq!(b.post("/done/close-terminals", json!({"review": true})).unwrap()["jobs"], 0, "one close per terminal");

    // A merged PR's task isn't In review any more.
    b.idle("s1");
    b.app.db.x("UPDATE tasks SET pr_phase = 'merged', pr_state = 'MERGED' WHERE id = ?", p![id]).unwrap();
    assert_eq!(b.post("/done/close-terminals", json!({"review": true})).unwrap()["jobs"], 0);
}
