//! `tb backlog task|drop|reopen`: only open issues can be made into tasks or dropped, several issues change
//! together or not at all, and the history credits the terminal that ran it. `tb task new --issue B<n>` (and
//! `tb backlog task B<n> --here`) builds a task from an open issue the same way, on this terminal, in a goal or on the board.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, reports};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, _dir: dir }
}

impl Board {
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn post_err(&self, path: &str, body: Value) -> (u16, String) {
        let e = api::dispatch(&self.app, "POST", path, &Query::new(), &body).expect_err("should fail");
        (e.status, e.message)
    }
    fn issue(&self, title: &str) -> i64 {
        self.post("/backlog", json!({"title": title, "project": "webapp", "kind": "bug"}))["id"].as_i64().unwrap()
    }
    fn state(&self, id: i64) -> String {
        self.get(&format!("backlog/B{id}"))["state"].as_str().unwrap_or("").to_string()
    }
    fn task_count(&self) -> usize {
        self.get("tasks")["tasks"].as_array().map(|a| a.len()).unwrap_or(0)
    }
    fn report(&self, event: &str, session: &str, extra: Value) -> Value {
        self.try_report(event, session, extra).unwrap_or_else(|e| panic!("{event}: {e}"))
    }
    fn try_report(&self, event: &str, session: &str, extra: Value) -> Result<Value, String> {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| e.message)
    }
    fn add_session(&self, sid: &str) {
        let repo = self._dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[])
            .unwrap();
    }
    /// The task `r` (T12) was made from, and the issue's task: (from_issue_id, issue's task_id).
    fn link(&self, r: &str, issue: i64) -> (Option<i64>, Option<i64>) {
        let t = board::get_task(&self.app, r.trim_start_matches('T').parse().unwrap()).unwrap();
        (t.i("from_issue_id"), board::get_issue(&self.app, issue).unwrap().i("task_id"))
    }
}

#[test]
fn a_dropped_or_promoted_issue_cant_be_promoted_or_dropped_again() {
    let b = new_board();
    let i = b.issue("Flaky test");
    b.post(&format!("backlog/B{i}/promote"), json!({"where": "board"}));
    let (code, msg) = b.post_err(&format!("backlog/B{i}/promote"), json!({"where": "board"}));
    assert_eq!(code, 409);
    assert!(msg.contains("already T"), "{msg}");
    assert_eq!(b.post_err(&format!("backlog/B{i}/drop"), json!({})).0, 409);

    let j = b.issue("Slow page");
    b.post(&format!("backlog/B{j}/drop"), json!({"reason": "dupe"}));
    let before = b.task_count();
    let (code, msg) = b.post_err(&format!("backlog/B{j}/promote"), json!({"where": "board"}));
    assert_eq!(code, 409);
    assert!(msg.contains("isn't open any more"), "{msg}");
    assert_eq!(b.post_err(&format!("backlog/B{j}/drop"), json!({})).0, 409);
    assert_eq!(b.task_count(), before);

    b.post(&format!("backlog/B{j}/reopen"), json!({}));
    b.post(&format!("backlog/B{j}/promote"), json!({"where": "board"}));
    assert_eq!(b.state(j), "task");
}

#[test]
fn several_issues_change_together_or_not_at_all() {
    let b = new_board();
    let (x, y, z) = (b.issue("One"), b.issue("Two"), b.issue("Three"));
    b.post(&format!("backlog/B{z}/drop"), json!({}));
    let before = b.task_count();
    let (code, _) = b.post_err("backlog/bulk", json!({"action": "task", "ids": [format!("B{x}"), format!("B{y}"), format!("B{z}")], "where": "board"}));
    assert_eq!(code, 409);
    assert_eq!((b.state(x), b.state(y)), ("open".to_string(), "open".to_string()));
    assert_eq!(b.task_count(), before);

    let r = b.post("backlog/bulk", json!({"action": "task", "ids": [format!("B{x}"), format!("B{y}")], "where": "board", "who": "terminal abcd1234"}));
    assert_eq!(r["count"], 2);
    assert_eq!(r["tasks"].as_array().unwrap().len(), 2);

    let (code, _) = b.post_err("backlog/bulk", json!({"action": "reopen", "ids": [format!("B{z}"), format!("B{x}"), "B999"]}));
    assert_eq!(code, 404);
    assert_eq!(b.state(z), "drop");

    // Reopening one that's already open names it.
    let w = b.issue("Four");
    let (code, msg) = b.post_err("backlog/bulk", json!({"action": "reopen", "ids": [format!("B{z}"), format!("B{w}")]}));
    assert_eq!((code, msg), (409, format!("B{w} is already open, so nothing changed.")));
    assert_eq!(b.state(z), "drop");

    // Several move together.
    let g = b.post("goals", json!({"name": "Later", "project": "web"}))["id"].as_i64().unwrap();
    let r = b.post("backlog/bulk", json!({"action": "move", "ids": [format!("B{z}"), format!("B{w}")], "goal_id": format!("G{g}")}));
    assert_eq!(r["count"], 2);
}

#[test]
fn history_credits_the_terminal_that_ran_it() {
    let b = new_board();
    let i = b.issue("Flaky test");
    let who = json!("terminal abcd1234");
    b.post("backlog/bulk", json!({"action": "drop", "ids": [format!("B{i}")], "reason": "dupe", "who": who}));
    b.post("backlog/bulk", json!({"action": "reopen", "ids": [format!("B{i}")], "who": who}));
    let r = b.post("backlog/bulk", json!({"action": "task", "ids": [format!("B{i}")], "where": "board", "who": who}));
    let hist = board::issue_history(&b.app, i).unwrap();
    let by: Vec<(&str, &str)> = hist.iter().map(|e| (e["kind"].as_str().unwrap(), e["who"].as_str().unwrap())).collect();
    assert!(by.contains(&("drop", "terminal abcd1234")), "{by:?}");
    assert!(by.contains(&("note", "terminal abcd1234")), "{by:?}");
    assert!(by.contains(&("task", "terminal abcd1234")), "{by:?}");
    let t = &r["tasks"][0];
    assert_eq!(t["origin"]["by"], "terminal abcd1234");

    let j = b.issue("Owner drops this");
    b.post(&format!("backlog/B{j}/drop"), json!({}));
    let hist = board::issue_history(&b.app, j).unwrap();
    assert_eq!(hist.last().unwrap()["who"], board::OWNER);
}

#[test]
fn a_backlog_note_lands_in_the_log_and_keeps_the_detail() {
    let b = new_board();
    let i = b.post("/backlog", json!({"title": "Tab bar flickers", "project": "webapp", "kind": "bug", "detail": "On the phone build."}))["id"].as_i64().unwrap();
    b.post(&format!("backlog/B{i}/note"), json!({"text": "seen again on the tablet build"}));
    assert_eq!(b.get(&format!("backlog/B{i}"))["detail"], "On the phone build.");
    let hist = board::issue_history(&b.app, i).unwrap();
    let last = hist.last().unwrap();
    assert_eq!((last["kind"].as_str(), last["text"].as_str()), (Some("note"), Some("Added a note: “seen again on the tablet build”")));
}

#[test]
fn here_takes_an_issue_as_a_task_on_this_terminal() {
    let b = new_board();
    b.add_session("s1");
    let i = b.post("/backlog", json!({"title": "Header clips", "detail": "The header clips on narrow windows.", "project": "webapp", "kind": "bug"}))["id"]
        .as_i64()
        .unwrap();
    let r = b.report("tb.new_task", "s1", json!({"title": "", "issue": format!("B{i}"), "here": true}));
    let tr = r["created"][0].as_str().unwrap().to_string();
    assert_eq!(r["status"], "working");
    assert_eq!(r["title"], "Header clips", "the item's title when none is given");
    let t = b.get(&format!("tasks/{tr}"));
    assert_eq!(t["session_id"], "s1");
    assert_eq!(t["title"], "Header clips");
    assert!(t["detail"].as_str().unwrap().starts_with("The header clips on narrow windows."), "{}", t["detail"]);
    assert_eq!(t["origin"]["from"], format!("Backlog B{i}: Header clips"));
    assert_eq!(t["latest"], "Tracked in Term s1.", "claiming it here replaces the item's latest, as --here does");
    let tid: i64 = tr.trim_start_matches('T').parse().unwrap();
    assert_eq!(b.link(&tr, i), (Some(i), Some(tid)));
    assert_eq!(b.state(i), "task");
    let log = json!(board::issue_history(&b.app, i).unwrap()).to_string();
    assert!(log.contains(&format!("Taken as {tr} on Term s1")), "{log}");
}

#[test]
fn here_next_queues_the_issue_behind_this_terminals_task() {
    let b = new_board();
    b.add_session("s1");
    let first = b.report("tb.new_task", "s1", json!({"title": "Fix the footer", "detail": "Asked for.", "here": true}))["created"][0]
        .as_str()
        .unwrap()
        .to_string();
    let i = b.issue("Footer overlaps");
    let busy = b.try_report("tb.new_task", "s1", json!({"issue": format!("B{i}"), "here": true}));
    assert!(busy.unwrap_err().contains("already on"));
    assert_eq!(b.state(i), "open", "a refused take leaves the item open");
    let r = b.report("tb.new_task", "s1", json!({"issue": format!("B{i}"), "here": true, "line": "next"}));
    let tr = r["created"][0].as_str().unwrap().to_string();
    assert_eq!(r["status"], "queued");
    assert!(r["context"].as_str().unwrap().contains(&format!("{tr} is queued in this terminal after {first}")));
    assert_eq!(b.link(&tr, i).0, Some(i));
    assert_eq!(b.state(i), "task");
    let latest = b.get(&format!("tasks/{tr}"))["latest"].as_str().unwrap_or("").to_string();
    assert!(latest.starts_with("From the backlog:"), "a queued task keeps the item's latest: {latest}");
}

#[test]
fn issue_makes_a_planned_task_in_a_goal_or_a_queued_one_on_the_board() {
    let b = new_board();
    b.add_session("s1");
    let g = b.post("/goals", json!({"name": "Login", "project": "webapp"}))["id"].as_i64().unwrap();
    let i = b.issue("Password field loses focus");
    let r = b.report("tb.new_task", "s1", json!({"title": "Keep focus in the password field", "issue": format!("B{i}"), "goal": format!("G{g}"), "wave": 1}));
    let tr = r["created"][0].as_str().unwrap().to_string();
    let t = b.get(&format!("tasks/{tr}"));
    assert_eq!(t["status"], "planned");
    assert_eq!(t["title"], "Keep focus in the password field", "a title given wins");
    assert_eq!(t["origin"]["from"], format!("Backlog B{i}: Password field loses focus"));
    assert_eq!(b.link(&tr, i).0, Some(i));
    assert_eq!(b.state(i), "task");

    let j = b.issue("Slow search");
    let r = b.report("tb.new_task", "s1", json!({"issue": format!("B{j}"), "detail": "Cache the index."}));
    let tr = r["created"][0].as_str().unwrap().to_string();
    let t = b.get(&format!("tasks/{tr}"));
    assert_eq!(t["status"], "queued");
    assert_eq!(t["detail"], "Cache the index.", "a detail given wins");
    assert_eq!(b.link(&tr, j).0, Some(j));
    assert_eq!(b.state(j), "task");
}

#[test]
fn issue_refuses_a_closed_or_already_tasked_item() {
    let b = new_board();
    b.add_session("s1");
    let i = b.issue("Flaky test");
    b.post(&format!("backlog/B{i}/promote"), json!({"where": "board"}));
    let j = b.issue("Old crash");
    b.post(&format!("backlog/B{j}/drop"), json!({"reason": "dupe"}));
    let before = b.task_count();
    let tasked = b.try_report("tb.new_task", "s1", json!({"issue": format!("B{i}"), "here": true})).unwrap_err();
    assert!(tasked.contains("already T"), "{tasked}");
    let closed = b.try_report("tb.new_task", "s1", json!({"issue": format!("B{j}")})).unwrap_err();
    assert!(closed.contains("isn't open any more"), "{closed}");
    assert_eq!(b.task_count(), before);
    assert!(b.get("tasks")["tasks"].as_array().unwrap().iter().all(|t| t["session_id"] != "s1"), "the terminal took nothing");
}
