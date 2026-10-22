//! A task's terminal tab in Midna follows the task: In review, Done, Question, Alert, Task failed and
//! Waiting, set with Midna's `session.set_status`.

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
        self.queued().into_iter().map(|(sid, t, _)| (sid, t)).collect()
    }
    /// The tab statuses queued since the last look, with what each is waiting on.
    fn queued(&self) -> Vec<(String, &'static str, Option<String>)> {
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
fn a_refused_submit_leaves_the_tab_alone() {
    let b = new_board(true);
    b.task_on("s1", "Login");
    assert!(b.report("tb.done", "s1", json!({"summary": "Login works"})).is_err());
    assert_eq!(b.tabs(), vec![]);
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

/// The calls the fake Midna got with `method`, as their params.
fn calls(b: &Board, method: &str) -> Vec<Value> {
    let calls = std::fs::read_to_string(b.dir.path().join("calls")).unwrap_or_default();
    calls.lines().filter_map(|l| l.strip_prefix(&format!("{method} "))).map(|p| serde_json::from_str(p).unwrap()).collect()
}

#[test]
fn queued_tabs_go_to_midna_as_status_labels() {
    let b = new_board(true);
    midna::set_tab(&b.app, "s1", "question", Some("T1 · Login: Which login page?".into()));
    midna::set_tab(&b.app, "s3", "in_review", None);
    midna::set_tab(&b.app, "s2", "clear", None);
    midna::send_tabs(&b.app);
    assert_eq!(
        calls(&b, "session.set_status"),
        vec![
            json!({"id": "s1", "label": "Question", "color": "amber", "base": "needs_you", "clear_on": "prompt",
                   "detail": "T1 · Login: Which login page?"}),
            json!({"id": "s3", "label": "In review", "color": "purple", "base": "done", "clear_on": "prompt"}),
        ]
    );
    assert_eq!(calls(&b, "session.clear_status"), vec![json!({"id": "s2"})]);
    assert!(calls(&b, "agent.hook").is_empty());
    assert!(b.tabs().is_empty());
}

#[test]
fn the_old_tab_triggers_come_out_once_per_midna_run() {
    let b = new_board(true);
    let trigger = |id: &str, name: &str, event: &str, enabled: bool| {
        json!({"id": id, "name": name, "source": "local", "event": event, "enabled": enabled})
    };
    let have = vec![
        trigger("t_1", "Taskboard: In review", "hook.Taskboard", true),
        trigger("t_2", "Taskboard: Clear status", "hook.Taskboard", false),
        trigger("t_3", "Taskboard: Done", "hook.Stop", true),
        trigger("t_4", "Keep awake", "hook.Taskboard", true),
    ];
    std::fs::write(b.dir.path().join("triggers"), serde_json::to_string(&have).unwrap()).unwrap();
    midna::sync_once(&b.app).unwrap();
    assert_eq!(calls(&b, "trigger.set_enabled"), vec![json!({"id": "t_1", "enabled": false})], "an enabled one is paused first");
    assert_eq!(calls(&b, "trigger.remove"), vec![json!({"id": "t_1"}), json!({"id": "t_2"})], "only the board's own");
    assert!(calls(&b, "trigger.add").is_empty());
    // The same Midna run isn't looked at again.
    std::fs::write(b.dir.path().join("calls"), "").unwrap();
    b.app.shared.lock().midna_links = None;
    midna::sync_once(&b.app).unwrap();
    assert!(calls(&b, "trigger.list").is_empty());
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
    assert_eq!(b.tabs(), [tab("v1", "in_review"), tab("s1", "in_review")].concat());
    b.set(id, fields!["pr_flow" => json!({"session": "v1", "stopped": {"asked": true}}).to_string()]);
    assert_eq!(b.tabs(), tab("v1", "question"), "the question is the visit's alone");
}

#[test]
fn a_merge_during_a_visit_sets_the_task_s_own_terminal_too() {
    let b = new_board(true);
    let id = b.task_on("s1", "Login");
    b.set(id, fields!["status" => "done", "finished_at" => "2026-10-16T10:00:00Z", "pr_num" => 9, "pr_state" => "OPEN", "pr_phase" => "checks"]);
    assert_eq!(b.tabs(), tab("s1", "in_review"));
    b.set(id, fields!["pr_flow" => json!({"session": "v1"}).to_string()]);
    b.tabs();
    b.set(id, fields!["pr_phase" => "merged", "pr_state" => "MERGED"]);
    assert_eq!(b.tabs(), [tab("v1", "done"), tab("s1", "done")].concat());
}

#[test]
fn a_merge_during_a_visit_leaves_an_own_terminal_that_moved_on() {
    let b = new_board(true);
    let id = b.task_on("s1", "Login");
    b.set(id, fields!["status" => "done", "finished_at" => "2026-10-16T10:00:00Z", "pr_num" => 9, "pr_state" => "OPEN", "pr_phase" => "checks",
                      "pr_flow" => json!({"session": "v1"}).to_string()]);
    b.tabs();
    b.task_on("s1", "Signup");
    b.set(id, fields!["pr_phase" => "merged", "pr_state" => "MERGED"]);
    assert_eq!(b.tabs(), tab("v1", "done"));
}

/// What the queued tab statuses are waiting on, in order.
fn details(b: &Board) -> Vec<Option<String>> {
    b.queued().into_iter().map(|(_, _, d)| d).collect()
}

#[test]
fn a_needs_you_tab_says_what_it_waits_on() {
    let b = new_board(true);
    let id = b.task_on("s1", "Login");
    b.report("tb.question", "s1", json!({"text": "Which login page?"})).unwrap();
    assert_eq!(details(&b), vec![Some(format!("T{id} · Login: Which login page?"))]);
    b.report("tb.withdraw", "s1", json!({"why": "Found it"})).unwrap();
    assert_eq!(details(&b), vec![None], "a clear has nothing to say");

    b.post("/alerts", json!({"text": "The build server is down", "task": format!("T{id}")}));
    assert_eq!(details(&b), vec![Some(format!("T{id} · Login: The build server is down"))]);
    b.post("/alerts", json!({"text": "Disk is nearly full", "session": "s1"}));
    assert_eq!(details(&b), vec![Some("Disk is nearly full".into())], "an alert with no task is its own text");

    b.set(id, fields!["status" => "done", "pr_num" => 9, "pr_state" => "OPEN", "pr_phase" => "checks",
                      "pr_flow" => json!({"session": "v1"}).to_string()]);
    assert_eq!(details(&b), vec![None, None], "in review needs nothing from you, on the visit or the task's own tab");
    b.set(id, fields!["pr_flow" => json!({"session": "v1", "stopped": {"asked": true, "message": "Squash the commits?"}}).to_string()]);
    assert_eq!(details(&b), vec![Some(format!("T{id} · Login: asked on PR #9: Squash the commits?"))]);
}

#[test]
fn a_question_after_the_pr_visit_ended_reaches_the_owner_instead_of_being_refused() {
    // #609: the visit's wake was spent but the agent is still on the PR.
    let b = new_board(true);
    let id = b.task_on("s1", "Login");
    b.set(id, fields!["status" => "done", "pr_num" => 9, "pr_state" => "OPEN", "pr_phase" => "comments",
                      "pr_flow" => json!({"session": "s1"}).to_string()]);
    b.report("tb.question", "s1", json!({"text": "Who can read the thread?"})).unwrap();
    assert_eq!(details(&b).last(), Some(&Some(format!("T{id} · Login: asked on PR #9: Who can read the thread?"))));
}
