//! Command timings read from transcripts: each tool call is a span joined to its task, or untracked.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{midna, reports, spans};

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
    let b = Board { app, dir };
    midna::sync(
        &b.app,
        &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})],
        &[],
    )
    .unwrap();
    b
}

impl Board {
    fn claude(&self, cid: &str) {
        let b = json!({"event": "hook.session_start", "session": "s1", "claude_session": cid, "cwd": self.dir.path().join("webapp").to_string_lossy(), "source": "startup"});
        reports::handle(&self.app, b, false).unwrap();
    }
    fn file(&self, name: &str) -> PathBuf {
        let dir = taskboardd::transcript::project_dir(&self.app.cfg.claude_projects, &self.dir.path().join("webapp").to_string_lossy());
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }
    fn append(&self, path: &PathBuf, lines: &[Value]) {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }
    fn spans(&self) -> Vec<taskboardd::util::Row> {
        self.app.db.q("SELECT * FROM spans ORDER BY started_at, id", vec![]).unwrap()
    }
}

fn prompt(at: &str, text: &str) -> Value {
    json!({"type": "user", "timestamp": at, "message": {"role": "user", "content": text}})
}

fn call(at: &str, id: &str, tool: &str, input: Value) -> Value {
    json!({"type": "assistant", "timestamp": at, "message": {"content": [{"type": "tool_use", "id": id, "name": tool, "input": input}]}})
}

fn result(at: &str, id: &str, text: &str, error: bool, extra: Value) -> Value {
    json!({"type": "user", "timestamp": at, "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": text, "is_error": error}]}, "toolUseResult": extra})
}

#[test]
fn tool_calls_become_spans_joined_to_the_tagged_task() {
    let b = new_board();
    let t = api::dispatch(&b.app, "POST", "/tasks", &Query::new(), &json!({"title": "Speed it up", "detail": "Do it.", "project": "webapp", "ships_pr": false})).unwrap();
    let tid = t["task"]["id"].as_i64().or_else(|| t["id"].as_i64()).unwrap();
    b.claude("c1");
    let main = b.file("c1.jsonl");
    b.append(&main, &[
        prompt("2026-10-13T10:00:00Z", "just looking around"),
        call("2026-10-13T10:00:01Z", "u0", "Bash", json!({"command": "ls"})),
        result("2026-10-13T10:00:02Z", "u0", "a\nb", false, json!({})),
        prompt("2026-10-13T10:01:00Z", &format!("[task-board:T{tid}] You are picking up “Speed it up”")),
        call("2026-10-13T10:01:05Z", "u1", "Bash", json!({"command": "cd /x && cargo test -p app"})),
        result("2026-10-13T10:03:05Z", "u1", "Exit code 101\nfailed", true, json!({"stdout": ""})),
        call("2026-10-13T10:04:00Z", "u2", "Bash", json!({"command": "cargo build"})),
    ]);
    assert_eq!(spans::scan(&b.app).unwrap(), 2, "u2 has no result yet");

    b.append(&main, &[
        result("2026-10-13T10:14:00Z", "u2", "Command did not complete within its 600s timeout and was moved to the background (ID: b1)", false, json!({"timedOutAfterMs": 600000, "backgroundTaskId": "b1"})),
        call("2026-10-13T10:15:00Z", "u3", "Bash", json!({"command": "cargo clippy"})),
        result("2026-10-13T10:15:01Z", "u3", "PreToolUse:Bash hook error: midna: the Mac is busy (load 92.97 on 12 cores, 775%; busy from 600%).", true, json!({})),
    ]);
    let sub = main.with_extension("").join("subagents");
    std::fs::create_dir_all(&sub).unwrap();
    b.append(&sub.join("agent-a1.jsonl"), &[
        call("2026-10-13T10:16:00Z", "u4", "Bash", json!({"command": "git status"})),
        result("2026-10-13T10:16:01Z", "u4", "clean", false, json!({})),
    ]);
    std::fs::OpenOptions::new().append(true).open(&main).unwrap().write_all(br#"{"type":"assistant","timestamp":"#).unwrap();
    assert_eq!(spans::scan(&b.app).unwrap(), 3, "a half-written line waits for the next scan");
    assert_eq!(spans::scan(&b.app).unwrap(), 0, "nothing is read twice");

    let s = b.spans();
    let by = |id: &str| s.iter().find(|r| r.s("tool_use_id") == Some(id)).unwrap().clone();
    let ls = by("u0");
    assert_eq!((ls.i("task_id"), ls.s("category"), ls.i("exit_code")), (None, Some("other"), Some(0)), "before the tag: untracked");
    let test = by("u1");
    assert_eq!((test.i("task_id"), test.s("category"), test.i("exit_code")), (Some(tid), Some("test"), Some(101)));
    assert_eq!(test["seconds"], json!(120.0));
    assert_eq!(test.s("project"), Some("webapp"));
    let build = by("u2");
    assert_eq!((build.s("category"), build.i("timed_out"), build.i("background"), build["seconds"].clone()), (Some("build"), Some(1), Some(1), json!(600.0)));
    let held = by("u3");
    assert_eq!((held.s("category"), held.i("refused"), held.i("exit_code"), held["load"].clone()), (Some("load wait"), Some(1), None, json!(775.0)));
    let git = by("u4");
    assert_eq!((git.i("task_id"), git.s("category"), git.i("subagent")), (Some(tid), Some("git"), Some(1)), "a subagent works on its conversation's task");
}

#[test]
fn a_conversation_with_no_task_stays_untracked() {
    let b = new_board();
    b.claude("c2");
    let main = b.file("c2.jsonl");
    b.append(&main, &[
        call("2026-10-13T11:00:00Z", "v1", "Bash", json!({"command": "gh pr checks 4 --watch"})),
        result("2026-10-13T11:05:00Z", "v1", "all green", false, json!({})),
    ]);
    assert_eq!(spans::scan(&b.app).unwrap(), 1);
    let s = &b.spans()[0];
    assert_eq!((s.i("task_id"), s.s("category"), s.s("session_id")), (None, Some("ci wait"), Some("s1")));
}
