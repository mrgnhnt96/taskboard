//! The Python board's changes of 2026-10-08 (`docs/parity/oct8-changes.md`): shared tasks, task origin,
//! `tb task new --here` and its Stop backstop, review alerts that stay, and the re-review and merge-retry stages.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, dispatch, midna, p, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn new_board() -> Board {
    board_with(|_| {})
}

fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    f(&mut cfg);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    fn get(&self, path: &str) -> Value {
        let (p, q) = path.split_once('?').unwrap_or((path, ""));
        let query: Query = q.split('&').filter(|x| !x.is_empty()).filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
        api::dispatch(&self.app, "GET", p, &query, &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn post_err(&self, path: &str, body: Value) -> (u16, String) {
        let e = api::dispatch(&self.app, "POST", path, &Query::new(), &body).expect_err("should fail");
        (e.status, e.message)
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
    fn repo(&self) -> String {
        self.dir.path().join("webapp").to_string_lossy().to_string()
    }
    fn add_session(&self, sid: &str) {
        midna::sync(
            &self.app,
            &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": self.repo(), "status": {"state": "working"}})],
            &[],
        )
        .unwrap();
    }
    fn goal(&self, name: &str) -> i64 {
        self.post("/goals", json!({"name": name, "project": "webapp"}))["id"].as_i64().unwrap()
    }
    fn task(&self, title: &str, extra: Value) -> i64 {
        let mut body = json!({"title": title, "detail": "Do it.", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", body)["id"].as_i64().unwrap()
    }
}

fn refs(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|x| x["ref"].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_shared_task_counts_toward_every_goal_it_finishes() {
    let b = new_board();
    let (home, other) = (b.goal("Login"), b.goal("Signup"));
    let t = b.task("Add the auth client", json!({"goal_id": home, "also": [format!("G{other}")]}));

    let card = b.get(&format!("tasks/T{t}"));
    assert_eq!(refs(&card["also"]), vec![format!("G{other}")]);
    assert_eq!(card["goal"]["id"], home);

    let g = b.get(&format!("goals/G{other}"));
    assert_eq!(g["tasks"].as_array().unwrap().len(), 0, "its home goal lists it, not this one");
    assert_eq!(refs(&g["shared"]), vec![format!("T{t}")]);
    assert_eq!(g["total"], 1, "it counts toward this goal");

    let filtered = b.get(&format!("tasks?goal=G{other}"));
    assert_eq!(refs(&filtered["tasks"]), vec![format!("T{t}")], "a goal filter finds it through --also");

    let handoff = board::get_task(&b.app, t).map(|_| taskboardd::handoff::build(&b.app, t).unwrap()).unwrap();
    assert!(handoff.contains("also finishes another goal"), "{handoff}");
    assert!(handoff.contains(&format!("G{other} “Signup”")), "{handoff}");

    b.post(&format!("tasks/T{t}"), json!({"not_also": [format!("G{other}")]}));
    assert_eq!(b.get(&format!("goals/G{other}"))["total"], 0);
}

#[test]
fn also_is_checked() {
    let b = new_board();
    let home = b.goal("Login");
    let elsewhere = b.post("/goals", json!({"name": "API", "project": "api"}))["id"].as_i64().unwrap();
    let (code, msg) = b.post_err("/tasks", json!({"title": "X", "project": "webapp", "goal_id": home, "also": [format!("G{elsewhere}")]}));
    assert_eq!(code, 409);
    assert!(msg.contains("can't finish it"), "{msg}");

    let loose = b.task("Loose", json!({}));
    let (code, msg) = b.post_err(&format!("tasks/T{loose}"), json!({"also": [format!("G{home}")]}));
    assert_eq!(code, 409);
    assert!(msg.contains("no goal of its own"), "{msg}");

    b.add_session("s1");
    let out = b.try_report("tb.new_task", "s1", json!({"title": "No home", "also": [format!("G{home}")]}));
    assert!(out.unwrap_err().contains("Only a task in a goal"));
}

#[test]
fn deleting_the_home_goal_moves_a_shared_task_to_its_newest_other_goal() {
    let b = new_board();
    let (home, g2, g3) = (b.goal("Home"), b.goal("Two"), b.goal("Three"));
    let t = b.task("Shared", json!({"goal_id": home, "also": [format!("G{g2}")]}));
    b.post(&format!("tasks/T{t}"), json!({"also": [format!("G{g3}")]}));
    let gone = b.task("Only home", json!({"goal_id": home}));

    let r = b.post(&format!("goals/G{home}/delete"), json!({"tasks": true}));
    assert_eq!(r["moved"], json!([format!("T{t}")]));
    assert_eq!(r["tasks"], 1);
    let task = board::get_task(&b.app, t).unwrap();
    assert_eq!(task.i("goal_id"), Some(g3), "the newest goal that references it");
    assert!(board::find_task(&b.app, Some(gone)).unwrap().is_none());
    let g3d = b.get(&format!("goals/G{g3}"));
    assert_eq!(refs(&g3d["tasks"]), vec![format!("T{t}")]);
    assert_eq!(refs(&g3d["shared"]), Vec::<String>::new(), "it runs here now, so it isn't shared into here");
    assert_eq!(refs(&b.get(&format!("goals/G{g2}"))["shared"]), vec![format!("T{t}")]);
}

#[test]
fn every_task_says_where_it_came_from() {
    let b = new_board();
    let g = b.goal("Login");
    let t = b.task("By hand", json!({}));
    assert_eq!(b.get(&format!("tasks/T{t}"))["origin"], json!({"from": "Added on the board", "by": "You"}));

    b.add_session("s1");
    let r = b.report("tb.propose", "s1", json!({"goal": format!("G{g}"), "tasks": ["Plan one::do it"]}));
    let planned = r["created"][0].as_str().unwrap().to_string();
    assert_eq!(b.get(&format!("tasks/{planned}"))["origin"], json!({"from": format!("Planned in G{g}"), "by": "Term s1"}));

    let i = b.post("/backlog", json!({"title": "Flaky test", "project": "webapp", "kind": "bug"}))["id"].as_i64().unwrap();
    let made = b.post(&format!("backlog/B{i}/promote"), json!({}));
    let tid = made["task"]["id"].as_i64().unwrap();
    assert_eq!(b.get(&format!("tasks/T{tid}"))["origin"], json!({"from": format!("Backlog B{i}: Flaky test"), "by": "You"}));

    let linked = b.task("Linked", json!({"origin": {"from": "A report", "by": "Sam", "url": "https://example.com/r/1"}}));
    assert_eq!(b.get(&format!("tasks/T{linked}"))["origin"]["url"], "https://example.com/r/1");
    let bad = b.task("Not a link", json!({"origin": {"from": "A report", "url": "file:///etc/passwd"}}));
    assert!(b.get(&format!("tasks/T{bad}"))["origin"].get("url").is_none());
}

#[test]
fn here_makes_a_standalone_task_this_terminal_is_on() {
    let b = new_board();
    b.add_session("s1");
    let r = b.report("tb.new_task", "s1", json!({"title": "Fix the header", "detail": "The owner asked for it.", "here": true}));
    let tr = r["created"][0].as_str().unwrap().to_string();
    assert_eq!(r["status"], "working");
    assert!(r["context"].as_str().unwrap().contains(&format!("you're on {tr} now")));
    let t = b.get(&format!("tasks/{tr}"));
    assert_eq!(t["status"], "working");
    assert_eq!(t["session_id"], "s1");
    assert!(t["goal"].is_null());
    assert_eq!(t["origin"]["from"], format!("Code changed in Term s1 while {} worked there", b.app.cfg.owner));

    let again = b.try_report("tb.new_task", "s1", json!({"title": "Another", "here": true}));
    assert!(again.unwrap_err().contains("already on"), "a terminal on a task tracks its work there");
    b.add_session("s2");
    let g = b.goal("Login");
    let with_goal = b.try_report("tb.new_task", "s2", json!({"title": "X", "here": true, "goal": format!("G{g}")}));
    assert!(with_goal.unwrap_err().contains("leave out --goal"));
}

#[test]
fn a_turn_that_changed_code_with_no_task_is_asked_to_track_it() {
    let b = new_board();
    b.add_session("s1");
    b.report("hook.prompt", "s1", json!({"prompt": "Make the header blue"}));
    let s = board::get_session(&b.app, Some("s1")).unwrap().unwrap();
    let file = taskboardd::transcript::transcript_file(&b.app.cfg.claude_projects, &s.st("project_path"), &s.st("claude_session_id"));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let inside = format!("{}/src/header.rs", b.repo());
    let lines = [
        json!({"type": "user", "message": {"content": "Make the header blue"}, "timestamp": "2026-10-08T10:00:00Z"}),
        json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Edit", "input": {"file_path": inside}}]}}),
        json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Write", "input": {"file_path": "/tmp/scratch.txt"}}]}}),
    ];
    std::fs::write(&file, lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n")).unwrap();

    let out = b.report("hook.stop", "s1", json!({"last_message": "Done."}));
    let block = out["block"].as_str().expect("the stop is blocked");
    assert!(block.contains("header.rs") && !block.contains("scratch"), "{block}");
    assert!(block.contains("--here"), "{block}");

    let again = b.report("hook.stop", "s1", json!({"last_message": "Done.", "stop_hook_active": true}));
    assert!(again.get("block").is_none(), "it asks once per turn");
}

fn tree(b: &Board, head: &str, files: &[(&str, &str)]) -> Value {
    let files: serde_json::Map<String, Value> = files.iter().map(|(f, s)| (format!("{}/{f}", b.repo()), json!(s))).collect();
    json!({"root": b.repo(), "head": head, "files": files})
}

#[test]
fn a_turn_that_changed_code_through_bash_is_asked_to_track_it() {
    let b = new_board();
    b.add_session("s1");
    b.report("hook.prompt", "s1", json!({"prompt": "This is clipping", "tree": tree(&b, "aaa", &[("src/old.rs", "1:10")])}));
    let after = tree(&b, "aaa", &[("src/old.rs", "1:10"), ("src/charts.rs", "2:40")]);
    let out = b.report("hook.stop", "s1", json!({"last_message": "Fixed.", "tree": after}));
    let block = out["block"].as_str().expect("an edit made by a script still blocks the stop");
    assert!(block.contains("charts.rs") && !block.contains("old.rs"), "{block}");

    b.report("hook.prompt", "s1", json!({"prompt": "What does this do?", "tree": tree(&b, "aaa", &[("src/charts.rs", "2:40")])}));
    let read_only = b.report("hook.stop", "s1", json!({"last_message": "It draws bars.", "tree": tree(&b, "aaa", &[("src/charts.rs", "2:40")])}));
    assert!(read_only.get("block").is_none(), "a turn that changed nothing isn't asked: {read_only}");
}

#[test]
fn a_turn_that_only_committed_is_asked_to_track_it() {
    let b = new_board();
    b.add_session("s1");
    b.report("hook.prompt", "s1", json!({"prompt": "Commit", "tree": tree(&b, "aaa", &[])}));
    let out = b.report("hook.stop", "s1", json!({"last_message": "Committed.", "git": {"commit": "fix(app): rows scroll"}, "tree": tree(&b, "bbb", &[])}));
    let block = out["block"].as_str().expect("a commit with no task blocks the stop");
    assert!(block.contains("a commit: fix(app): rows scroll"), "{block}");
}

#[test]
fn a_review_alert_stays_until_the_pr_is_reviewed() {
    let b = new_board();
    let t = b.task("Ship it", json!({}));
    b.app.db.tx(|| dispatch::add_alert(&b.app, "PR #1 for T1 is green and ready for review.", Some(t), None, None, Some("review"))).unwrap();
    let id = dispatch::alerts(&b.app)[0]["id"].as_str().unwrap().to_string();
    let ids = || dispatch::alerts(&b.app).iter().map(|a| a["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();

    let (code, _) = b.post_err(&format!("alerts/{id}/dismiss"), json!({}));
    assert_eq!(code, 409, "it can't be dismissed");
    b.post(&format!("alerts/{id}/snooze"), json!({"mins": 15}));
    assert_eq!(ids(), vec![id.clone()], "but it snoozes");

    b.app.db.tx(|| {
        dispatch::add_alert(&b.app, "T1 didn't start.", Some(t), None, None, None)?;
        dispatch::clear_alerts(&b.app, Some(t), None)
    }).unwrap();
    assert_eq!(ids(), vec![id.clone()], "other alerts for the task don't replace it");

    b.app.db.tx(|| {
        for n in 0..25 {
            dispatch::add_alert(&b.app, &format!("Alert {n}"), None, None, Some(&format!("s{n}")), None)?;
        }
        Ok(())
    }).unwrap();
    assert!(ids().contains(&id), "a flood of other alerts doesn't push it out");
    assert_eq!(ids().len(), 21);
}

fn pr_task(b: &Board) -> i64 {
    let id = b.task("Ship it", json!({}));
    b.add_session("s1");
    b.report("tb.take", "s1", json!({"task": format!("T{id}")}));
    b.report("tb.done", "s1", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9"}));
    id
}

fn rec(decision: &str, comments: i64, changes_at: &str) -> Value {
    json!({"state": "OPEN", "head": "h1", "checks": [{"name": "ci", "state": "passed"}], "failed": [], "running": 0,
           "comments": comments, "approvals": if decision == "APPROVED" { 1 } else { 0 }, "review_decision": decision, "changes_at": changes_at})
}

#[test]
fn addressed_changes_wait_in_rereview() {
    let b = board_with(|c| {
        c.pr.watch = true;
        c.pr.wake = true;
    });
    let id = pr_task(&b);
    let t = || board::get_task(&b.app, id).unwrap();
    b.app.db.tx(|| prflow::step(&b.app, &t(), &rec("CHANGES_REQUESTED", 1, "c1"))).unwrap();
    assert_eq!(t().s("pr_phase"), Some("comments"));
    b.app.db.x("UPDATE jobs SET state = 'done' WHERE purpose = 'pr'", p![]).unwrap();
    b.post(&format!("tasks/T{id}/pr/wait"), json!({}));
    b.app.db.tx(|| prflow::step(&b.app, &t(), &rec("CHANGES_REQUESTED", 1, "c1"))).unwrap();
    assert_eq!(t().s("pr_phase"), Some("rereview"));
    assert_eq!(prflow::label("rereview"), "Awaiting re-review");
    assert!(!prflow::awaiting_owner(&t()), "it waits on the reviewer, not the owner");
    b.app.db.tx(|| prflow::step(&b.app, &t(), &rec("CHANGES_REQUESTED", 1, "c2"))).unwrap();
    assert_eq!(t().s("pr_phase"), Some("comments"), "new changes asked: back to work");
}
