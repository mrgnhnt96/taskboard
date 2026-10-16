//! A task's terminal tab in Midna follows the task: In review, Done, Question, Alert, Task failed and
//! Waiting, set through local triggers the board keeps in Midna.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, fields, midna, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

/// A `midna` that logs each call's method and params to `calls`, lists the triggers in `triggers`
/// and answers `{}` to anything else.
fn fake_midna(dir: &Path) -> std::path::PathBuf {
    let exe = dir.join("midna");
    let script = format!(
        "#!/bin/sh\necho \"$2 $3\" >> {log}\ncase \"$2\" in\n  trigger.list) cat {list} 2>/dev/null || echo '[]' ;;\n  session.list) echo '[]' ;;\n  project.list) echo '[]' ;;\n  events.list) echo '[]' ;;\n  *) echo '{{}}' ;;\nesac\n",
        log = dir.join("calls").display(),
        list = dir.join("triggers").display(),
    );
    std::fs::write(&exe, script).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    exe
}

fn new_board(runner: bool) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.midna = fake_midna(dir.path());
    cfg.runner = runner;
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn report(&self, event: &str, session: &str, extra: Value) -> Result<Value, String> {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {"branch": "feature/x"}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| e.message)
    }
    /// A task taken by terminal `sid`.
    fn task_on(&self, sid: &str, title: &str) -> i64 {
        let repo = self.dir.path().join("webapp");
        midna::sync(
            &self.app,
            &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})],
            &[],
        )
        .unwrap();
        let id = self.post("/tasks", json!({"title": title, "detail": "Do it.", "project": "webapp", "pickup": "new"}))["id"].as_i64().unwrap();
        self.report("tb.take", sid, json!({"task": format!("T{id}")})).unwrap();
        self.tabs();
        id
    }
    /// The tab statuses queued for Midna since the last look.
    fn tabs(&self) -> Vec<(String, &'static str)> {
        self.app.shared.lock().midna_tabs.drain(..).collect()
    }
    fn set(&self, id: i64, f: Vec<(&str, Value)>) {
        board::update_task(&self.app, id, f).unwrap();
    }
}

fn tab(sid: &str, t: &'static str) -> Vec<(String, &'static str)> {
    vec![(sid.to_string(), t)]
}

#[test]
fn a_question_sets_the_tab_and_its_withdraw_clears_it() {
    let b = new_board(true);
    b.task_on("s1", "Login");
    b.report("tb.question", "s1", json!({"text": "Which login page?"})).unwrap();
    assert_eq!(b.tabs(), tab("s1", "question"));
    b.report("tb.withdraw", "s1", json!({"why": "Found it in the config"})).unwrap();
    assert_eq!(b.tabs(), tab("s1", "clear"));
}

#[test]
fn a_wait_sets_the_tab() {
    let b = new_board(true);
    let other = b.task_on("s2", "Schema");
    b.task_on("s1", "Login");
    b.report("tb.wait_for", "s1", json!({"tasks": [format!("T{other}")], "why": "needs the schema"})).unwrap();
    assert_eq!(b.tabs(), tab("s1", "waiting"));
}

#[test]
fn a_fail_sets_the_tab() {
    let b = new_board(true);
    b.task_on("s1", "Login");
    b.report("tb.fail", "s1", json!({"reason": "The API is gone"})).unwrap();
    assert_eq!(b.tabs(), tab("s1", "failed"));
}

#[test]
fn a_refused_submit_marks_the_tab_failed() {
    let b = new_board(true);
    b.task_on("s1", "Login");
    assert!(b.report("tb.done", "s1", json!({"summary": "Login works"})).is_err());
    assert_eq!(b.tabs(), tab("s1", "failed"));
}

#[test]
fn a_pr_goes_in_review_then_done_once_merged() {
    let b = new_board(true);
    let id = b.task_on("s1", "Login");
    b.set(id, fields!["status" => "done", "pr_num" => 9, "pr_url" => "https://github.com/acme/webapp/pull/9", "pr_state" => "OPEN", "pr_phase" => "checks"]);
    assert_eq!(b.tabs(), tab("s1", "in_review"));
    b.set(id, fields!["pr_phase" => "review"]);
    assert_eq!(b.tabs(), vec![], "the same status isn't sent again");
    b.set(id, fields!["pr_phase" => "merged", "pr_state" => "MERGED"]);
    assert_eq!(b.tabs(), tab("s1", "done"));
}

#[test]
fn done_without_a_pr_is_done() {
    let b = new_board(true);
    let id = b.task_on("s1", "Login");
    b.set(id, fields!["status" => "done", "no_pr" => "nothing to change"]);
    assert_eq!(b.tabs(), tab("s1", "done"));
}

#[test]
fn a_terminal_on_another_task_keeps_its_tab() {
    let b = new_board(true);
    let id = b.task_on("s1", "Login");
    b.set(id, fields!["status" => "done", "pr_num" => 9, "pr_state" => "OPEN", "pr_phase" => "checks"]);
    b.tabs();
    b.task_on("s1", "Signup");
    b.set(id, fields!["pr_phase" => "merged"]);
    assert_eq!(b.tabs(), vec![]);
}

#[test]
fn an_alert_sets_the_tab_of_the_terminal_that_raised_it() {
    let b = new_board(true);
    b.task_on("s1", "Login");
    b.post("/alerts", json!({"text": "The build server is down", "session": "s1"}));
    assert_eq!(b.tabs(), tab("s1", "alert"));
}

#[test]
fn a_board_without_the_runner_sends_no_tabs() {
    let b = new_board(false);
    let id = b.task_on("s1", "Login");
    b.set(id, fields!["status" => "done", "no_pr" => "nothing to change"]);
    assert_eq!(b.tabs(), vec![]);
}

#[test]
fn queued_tabs_go_to_midna_as_hook_events() {
    let b = new_board(true);
    midna::set_tab(&b.app, "s1", "question");
    midna::send_tabs(&b.app);
    let calls = std::fs::read_to_string(b.dir.path().join("calls")).unwrap();
    let line = calls.lines().find(|l| l.starts_with("agent.hook ")).expect("an agent.hook call");
    let params: Value = serde_json::from_str(line.trim_start_matches("agent.hook ")).unwrap();
    assert_eq!(params, json!({"agent": "claude", "event": "Taskboard", "session": "s1", "payload": {"tab": "question"}}));
    assert!(b.tabs().is_empty());
}

#[test]
fn the_triggers_go_in_once_per_midna_run() {
    let b = new_board(true);
    let adds = || {
        std::fs::read_to_string(b.dir.path().join("calls")).unwrap_or_default().lines().filter(|l| l.starts_with("trigger.add ")).count()
    };
    midna::sync_once(&b.app).unwrap();
    assert_eq!(adds(), 7);
    let want = midna::tab_triggers(&b.app);
    assert_eq!(want[0]["name"], "Taskboard: In review");
    assert_eq!(want[0]["event"], "hook.Taskboard");
    assert_eq!(want[0]["action"], json!({"kind": "set_status", "label": "In review", "color": "purple", "base": "done", "clear_on": "prompt"}));
    assert_eq!(want[6]["action"], json!({"kind": "clear_status"}));
    // A Midna that already has them (a restarted board, the same Midna run or not) gets none again.
    std::fs::write(b.dir.path().join("calls"), "").unwrap();
    let have: Vec<Value> = want.iter().enumerate().map(|(i, t)| {
        let mut t = t.clone();
        t["id"] = json!(format!("t_{i}"));
        t
    }).collect();
    std::fs::write(b.dir.path().join("triggers"), serde_json::to_string(&have).unwrap()).unwrap();
    b.app.shared.lock().midna_tab_triggers = None;
    b.app.shared.lock().midna_links = None;
    midna::sync_once(&b.app).unwrap();
    assert_eq!(adds(), 0);
}

#[test]
fn a_wait_on_ready_work_or_a_finished_wait_isn_t_waiting() {
    let b = new_board(true);
    let other = b.task_on("s2", "Schema");
    b.set(other, fields!["status" => "done", "no_pr" => "nothing to change", "finished_at" => "2026-10-16T10:00:00Z"]);
    b.task_on("s1", "Login");
    b.tabs();
    b.report("tb.wait_for", "s1", json!({"tasks": [format!("T{other}")]})).unwrap();
    assert_eq!(b.tabs(), vec![], "it isn't parked, so it isn't waiting");
    b.report("tb.question", "s1", json!({"text": "Which login page?"})).unwrap();
    assert_eq!(b.tabs(), tab("s1", "question"));
    b.report("tb.withdraw", "s1", json!({"why": "Found it"})).unwrap();
    assert_eq!(b.tabs(), tab("s1", "clear"), "a waits_for left from the done task doesn't show as waiting");
}

#[test]
fn an_older_task_s_merge_keeps_the_tab_of_a_task_finished_since() {
    let b = new_board(true);
    let first = b.task_on("s1", "Login");
    b.set(first, fields!["status" => "done", "finished_at" => "2026-10-16T10:00:00Z", "pr_num" => 9, "pr_state" => "OPEN", "pr_phase" => "checks"]);
    b.tabs();
    let second = b.task_on("s1", "Signup");
    b.set(second, fields!["status" => "done", "finished_at" => "2026-10-16T11:00:00Z", "pr_num" => 10, "pr_state" => "OPEN", "pr_phase" => "checks"]);
    assert_eq!(b.tabs(), tab("s1", "in_review"));
    b.set(first, fields!["pr_phase" => "merged", "pr_state" => "MERGED"]);
    assert_eq!(b.tabs(), vec![]);
    let third = b.task_on("s1", "Profile");
    b.report("tb.question", "s1", json!({"text": "Which avatar size?"})).unwrap();
    assert_eq!(b.tabs(), tab("s1", "question"), "an open task still gets the tab after earlier ones finished");
    b.set(third, fields!["status" => "done", "finished_at" => "2026-10-16T12:00:00Z", "no_pr" => "nothing to change"]);
    b.tabs();
    b.set(second, fields!["pr_phase" => "merged", "pr_state" => "MERGED"]);
    assert_eq!(b.tabs(), vec![], "the newest finished task has the tab");
}

#[test]
fn an_alert_on_a_task_from_elsewhere_sets_the_task_s_terminal() {
    let b = new_board(true);
    let id = b.task_on("s1", "Login");
    b.post("/alerts", json!({"text": "The build server is down", "task": format!("T{id}"), "session": ""}));
    assert_eq!(b.tabs(), tab("s1", "alert"));
    b.post("/alerts", json!({"text": "The tests are flaky", "task": format!("T{id}")}));
    assert_eq!(b.tabs(), tab("s1", "alert"), "a hook's alert sends no session at all");
    b.post("/alerts", json!({"text": "Disk is nearly full"}));
    assert_eq!(b.tabs(), vec![], "an alert with no terminal and no task sets no tab");
}

#[test]
fn a_pr_visit_sets_the_visiting_terminal_and_a_stop_to_ask_is_a_question() {
    let b = new_board(true);
    let id = b.task_on("s1", "Login");
    b.set(id, fields!["status" => "done", "pr_num" => 9, "pr_state" => "OPEN", "pr_phase" => "checks",
                      "pr_flow" => json!({"session": "v1"}).to_string()]);
    assert_eq!(b.tabs(), tab("v1", "in_review"));
    b.set(id, fields!["pr_flow" => json!({"session": "v1", "stopped": {"asked": true}}).to_string()]);
    assert_eq!(b.tabs(), tab("v1", "question"));
}
