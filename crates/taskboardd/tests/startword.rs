//! `tb start T4`: an agent starts a task only on the word a human typed in its terminal.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, p, reports};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    Board { app, _dir: dir }
}

impl Board {
    fn report(&self, event: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": ""});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap()
    }
    fn said(&self, prompt: &str) {
        self.report("hook.prompt", json!({"prompt": prompt}));
    }
    /// What the agent's `tb task new` adds: a task that waits for Start.
    fn new_task(&self) -> i64 {
        let v = self.report("tb.new_task", json!({"title": "Add login"}));
        v["created"][0].as_str().unwrap().trim_start_matches('T').parse().unwrap()
    }
    /// What `tb start T<id>` sends from terminal s1.
    fn tb_start(&self, id: i64) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", &format!("/tasks/T{id}/start"), &Query::new(), &json!({"mode": "queue", "via_session": "s1"}))
            .map_err(|e| (e.status, e.message))
    }
    fn status(&self, id: i64) -> String {
        board::get_task(&self.app, id).unwrap().st("status")
    }
    fn log(&self, id: i64) -> Vec<String> {
        self.app.db.q("SELECT text FROM events WHERE task_id = ? ORDER BY id", p![id]).unwrap().iter().map(|r| r.st("text")).collect()
    }
}

#[test]
fn an_agent_starts_a_task_on_the_owners_word() {
    let b = board();
    b.said("create a new task for the login page and queue it");
    let id = b.new_task();
    assert_eq!(b.status(id), "queued");
    assert!(b.tb_start(id).unwrap()["starting"] == true);
    let by = format!("Started by {} via Term", b.app.cfg.owner);
    assert!(b.log(id).contains(&by), "{:?}", b.log(id));
}

#[test]
fn without_the_owners_word_the_start_is_refused() {
    let b = board();
    let id = b.new_task();
    let refused = |b: &Board| {
        let (code, why) = b.tb_start(id).unwrap_err();
        assert_eq!(code, 403);
        assert!(why.contains("Only a human can start"), "{why}");
        assert_eq!(b.status(id), "queued");
    };
    refused(&b);
    b.said("make a task for the login page but don't start it");
    refused(&b);
    // The board's own prompt (here handing this terminal another task) is no one's word.
    let other = b.new_task();
    b.said(&format!("[task-board:T{other}] You are picking up “Add login”. Start T{id} when you're ready."));
    refused(&b);
    b.said("start T99");
    refused(&b);
    b.said("thanks, looks good");
    refused(&b);

    // The word names the task, so a later prompt doesn't take it back.
    b.said(&format!("ok, start T{id}"));
    b.said("and tell me when it's going");
    assert!(b.tb_start(id).unwrap()["starting"] == true);
}

#[test]
fn the_word_is_for_this_conversation_only() {
    let b = board();
    let id = b.new_task();
    b.said(&format!("start T{id}"));
    b.report("hook.session_start", json!({"source": "clear"}));
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);
}

#[test]
fn the_boards_start_says_it_came_from_the_ui() {
    let b = board();
    let id = b.new_task();
    api::dispatch(&b.app, "POST", &format!("/tasks/T{id}/start"), &Query::new(), &json!({"mode": "queue"})).unwrap();
    assert!(b.log(id).contains(&"Started in the UI".to_string()), "{:?}", b.log(id));
}
