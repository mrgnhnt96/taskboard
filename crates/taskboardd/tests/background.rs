//! A terminal whose turn ended with background commands or agents still running is waiting on them,
//! not idle: it shows "waiting", isn't offered work, and closes only by force.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{iso, now_ts};
use taskboardd::{midna, reports, transcript};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    fn session(&self) -> Value {
        let s = api::dispatch(&self.app, "GET", "/sessions", &Query::new(), &json!({})).unwrap();
        s["sessions"][0].clone()
    }
    fn report(&self, event: &str, extra: Value) {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": ""});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message));
    }
    fn midna_says(&self, state: &str) {
        self.midna_lists(state, json!({}));
    }
    /// Midna's sync, with this `agent_info` on the terminal.
    fn midna_lists(&self, state: &str, agent_info: Value) {
        let repo = self.dir.path().join("webapp");
        midna::sync(
            &self.app,
            &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": state}, "agent_info": agent_info})],
            &[],
        )
        .unwrap();
    }
    fn transcript(&self, lines: &[Value]) -> String {
        let dir = self.app.cfg.claude_projects.join("-webapp");
        std::fs::create_dir_all(&dir).unwrap();
        let p: PathBuf = dir.join("c-s1.jsonl");
        std::fs::write(&p, lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n")).unwrap();
        p.to_string_lossy().to_string()
    }
}

fn shell_started(id: &str, at: &str) -> Value {
    json!({"type": "user", "timestamp": at, "message": {"content": [{"type": "tool_result", "content": format!("Command running in background with ID: {id}.")}]},
           "toolUseResult": {"stdout": "", "backgroundTaskId": id}})
}

fn agent_started(id: &str, at: &str) -> Value {
    json!({"type": "user", "timestamp": at, "message": {"content": [{"type": "tool_result", "content": "Async agent launched."}]},
           "toolUseResult": {"isAsync": true, "status": "async_launched", "agentId": id, "description": "Review it"}})
}

fn finished(id: &str, at: &str) -> Value {
    json!({"type": "user", "timestamp": at, "message": {"content": format!("<task-notification>\n<task-id>{id}</task-id>\n<status>completed</status>\n</task-notification>")}})
}

#[test]
fn counts_background_commands_and_agents_until_each_reports_back() {
    let b = new_board();
    let now = iso(now_ts());
    let long_ago = iso(now_ts() - 3.0 * 3600.0);
    let path = b.transcript(&[
        shell_started("b1", &now),
        shell_started("b2", &now),
        agent_started("a1", &now),
        agent_started("a2", &now),
        finished("b2", &now),
        finished("a2", &now),
        shell_started("b-old", &long_ago),
    ]);
    assert_eq!(transcript::background_running(&b.app.cfg.claude_projects, Some(&path)), Some(transcript::Background { commands: 1, agents: 1 }), "b1 and a1; b-old is past Claude's limit");
}

#[test]
fn a_turn_that_leaves_background_work_running_waits_on_it() {
    let b = new_board();
    b.midna_says("idle");
    let now = iso(now_ts());
    let path = b.transcript(&[shell_started("b1", &now), agent_started("a1", &now)]);

    b.report("hook.stop", json!({"last_message": "Both are running. I'll wait for them.", "transcript_path": path}));
    let s = b.session();
    assert_eq!(s["status"], "waiting");
    assert_eq!(s["background"], json!({"agents": 1, "commands": 1}));
    assert_eq!(s["close"], "force", "closing it would stop its background work");
    assert_eq!(s["can_take"], false);

    b.midna_says("idle");
    assert_eq!(b.session()["status"], "waiting", "Midna sees its prompt, but the work is still running");
    b.midna_says("working");
    assert_eq!(b.session()["status"], "working", "a background task reported back and woke it");

    let path = b.transcript(&[shell_started("b1", &now), agent_started("a1", &now), finished("b1", &now), finished("a1", &now)]);
    b.report("hook.stop", json!({"last_message": "Both finished.", "transcript_path": path}));
    let s = b.session();
    assert_eq!(s["status"], "idle");
    assert_eq!(s["background"], Value::Null);
    assert_eq!(s["close"], "close");
}

#[test]
fn a_new_claude_process_drops_the_old_ones_background_work() {
    let b = new_board();
    b.midna_says("idle");
    let path = b.transcript(&[shell_started("b1", &iso(now_ts()))]);
    b.report("hook.stop", json!({"last_message": "Waiting on the build.", "transcript_path": path}));
    assert_eq!(b.session()["status"], "waiting");

    b.report("hook.session_start", json!({"source": "compact"}));
    assert_eq!(b.session()["status"], "waiting", "compacting keeps the same process and its background work");

    b.report("hook.session_start", json!({"source": "startup"}));
    assert_eq!(b.session()["status"], "idle");
}

fn running_shell(id: &str) -> Value {
    json!({"id": id, "kind": "shell", "status": "running", "description": "build", "command": "make"})
}

#[test]
fn an_interrupted_wake_up_doesnt_leave_it_waiting() {
    // #125's repro: the background shell finishes and wakes it, and that turn ends with no Stop.
    let b = new_board();
    b.midna_says("idle");
    let now = iso(now_ts());
    let path = b.transcript(&[shell_started("b1", &now)]);
    b.report("hook.stop", json!({"last_message": "Waiting on the build.", "transcript_path": path}));
    assert_eq!(b.session()["status"], "waiting");

    b.midna_says("working");
    b.transcript(&[shell_started("b1", &now), finished("b1", &now)]);
    b.report("hook.prompt", json!({"prompt": "<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>\n</task-notification>"}));
    b.midna_says("idle");
    let s = b.session();
    assert_eq!(s["status"], "idle", "the turn that started cleared the saved count");
    assert_eq!(s["background"], Value::Null);
    assert_eq!(s["close"], "close");
}

#[test]
fn midna_s_live_list_rules_while_it_says() {
    let b = new_board();
    b.midna_says("idle");
    let now = iso(now_ts());
    let path = b.transcript(&[shell_started("b1", &now)]);
    b.report("hook.stop", json!({"last_message": "Waiting on the build.", "transcript_path": path}));

    b.midna_lists("idle", json!({"background": [running_shell("b1")], "background_at": now}));
    assert_eq!(b.session()["status"], "waiting");
    assert_eq!(b.session()["background"], json!({"agents": 0, "commands": 1}));

    // The shell finished and the turn it woke was interrupted: no Stop, no prompt hook. Midna leaves
    // an empty list out, but still says when it last counted.
    b.midna_lists("idle", json!({"background_at": now}));
    let s = b.session();
    assert_eq!(s["status"], "idle", "Midna lists nothing running");
    assert_eq!(s["background"], Value::Null);
    assert_eq!(s["close"], "close");
}

#[test]
fn long_background_work_holds_it_as_long_as_midna_lists_it() {
    let b = new_board();
    b.midna_says("idle");
    let now = iso(now_ts());
    let path = b.transcript(&[shell_started("b1", &now)]);
    b.report("hook.stop", json!({"last_message": "Watching.", "transcript_path": path}));
    let long_ago = iso(now_ts() - 3.0 * 3600.0);
    b.app.db.x("UPDATE sessions SET background_at = ? WHERE id = 's1'", taskboardd::p![long_ago]).unwrap();
    assert_eq!(b.session()["status"], "idle", "with Midna silent, Claude would have stopped it by now");

    b.midna_lists("idle", json!({"background": [running_shell("b1")], "background_at": long_ago}));
    let s = b.session();
    assert_eq!(s["status"], "waiting", "Midna still lists it running");
    assert_eq!(s["close"], "force");
}

#[test]
fn midna_s_list_counts_agents_and_commands_still_running() {
    let b = new_board();
    b.midna_lists(
        "idle",
        json!({"background": [
            running_shell("b1"),
            {"id": "a1", "kind": "subagent", "status": "running", "description": "review"},
            {"id": "m1", "kind": "monitor", "status": "running", "description": "watch"},
            {"id": "b2", "kind": "shell", "status": "completed", "description": "done"},
        ]}),
    );
    let s = b.session();
    assert_eq!(s["status"], "waiting");
    assert_eq!(s["background"], json!({"agents": 1, "commands": 1}), "the monitor isn't work it waits on");
    assert_eq!(s["can_take"], false);
}

fn running_monitor(id: &str) -> Value {
    json!({"id": id, "kind": "monitor", "status": "running", "description": "live updates for artifact"})
}

#[test]
fn a_running_monitor_isnt_background_work() {
    // #139's repro: Claude arms a monitor on every artifact it publishes, and it runs until it expires.
    let b = new_board();
    b.midna_lists("idle", json!({"background": [running_monitor("m1")], "background_at": iso(now_ts())}));
    let s = b.session();
    assert_eq!(s["status"], "idle");
    assert_eq!(s["background"], Value::Null);
    assert_eq!(s["close"], "close");
}

#[test]
fn a_done_task_s_terminal_with_only_a_monitor_still_closes() {
    let b = new_board();
    b.midna_lists("idle", json!({"background": [running_monitor("m1")], "background_at": iso(now_ts())}));
    let t = api::dispatch(&b.app, "POST", "/tasks", &Query::new(), &json!({"title": "Zip it", "detail": "Do it.", "project": "webapp", "ships_pr": false}))
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    taskboardd::board::create_job(&b.app, "agent", json!({}), Some(t), "", Some(json!({"session": "s1"}))).unwrap();
    b.app
        .db
        .x(
            "UPDATE tasks SET status = 'done', session_id = 's1', auto_close = 1, finished_at = '2000-01-01T00:00:00Z' WHERE id = ?",
            taskboardd::p![t],
        )
        .unwrap();
    b.app.db.x("UPDATE sessions SET status_at = '2000-01-01T00:00:00Z' WHERE id = 's1'", taskboardd::p![]).unwrap();
    taskboardd::runner::auto_close_done(&b.app).unwrap();
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM jobs WHERE kind = 'close'", taskboardd::p![]).unwrap(), 1);
}

#[test]
fn every_idle_prompt_recounts_its_background_work() {
    let b = new_board();
    b.midna_says("idle");
    let now = iso(now_ts());
    let path = b.transcript(&[shell_started("b1", &now)]);
    b.report("hook.stop", json!({"last_message": "Waiting on the build.", "transcript_path": path}));
    assert_eq!(b.session()["status"], "waiting");

    let path = b.transcript(&[shell_started("b1", &now), finished("b1", &now)]);
    b.report("hook.attention", json!({"notification_type": "idle_prompt", "message": "Claude is waiting for your input", "transcript_path": path}));
    let s = b.session();
    assert_eq!(s["background"], Value::Null, "the idle prompt counted none running");
    assert_ne!(s["status"], "waiting");
}
