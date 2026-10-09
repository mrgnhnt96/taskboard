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
        let repo = self.dir.path().join("webapp");
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": state}})], &[])
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
    assert_eq!(transcript::background_running(&b.app.cfg.claude_projects, Some(&path)), Some(2), "b1 and a1; b-old is past Claude's limit");
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
    assert_eq!(s["background"], 2);
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
    assert_eq!(s["background"], 0);
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
