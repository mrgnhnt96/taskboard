//! A cold conversation resumed in a new terminal opens at once and compacts in the tab: `/compact` is
//! queued ahead of the prompt, with no headless compact holding the terminal back.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, p};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
    repo: String,
}

/// A board whose Midna logs each call and opens S9, and whose Claude marks any headless run. The
/// conversation `c1` last spoke at `last_at`.
fn new_board(last_at: &str) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let midna_exe = dir.path().join("fake-midna");
    std::fs::write(
        &midna_exe,
        "#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s %s\\n' \"$2\" \"$3\" >> \"$d/calls\"\nif [ \"$2\" = session.open ]; then echo '{\"id\": \"S9\"}'; exit 0; fi\necho '{}'\n",
    )
    .unwrap();
    std::fs::set_permissions(&midna_exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.midna = midna_exe;
    let claude_exe = dir.path().join("fake-claude");
    std::fs::write(&claude_exe, "#!/bin/sh\ntouch \"$(dirname \"$0\")/headless\"\n").unwrap();
    std::fs::set_permissions(&claude_exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.claude = claude_exe.to_string_lossy().to_string();
    let file = taskboardd::transcript::transcript_file(&cfg.claude_projects, &repo.to_string_lossy(), "c1");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, json!({"type": "user", "message": {"content": "hi"}, "timestamp": last_at}).to_string()).unwrap();
    let app = App::for_tests(cfg);
    let repo = repo.to_string_lossy().to_string();
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo}]).to_string())).unwrap();
    Board { app, dir, repo }
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    /// Runs a resume of `c1` for a new task and returns Midna's calls: method and params.
    fn resume(&self) -> (i64, Vec<(String, Value)>) {
        let t = self.post("/tasks", json!({"title": "Fix the tab bar", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
        let args = json!({"flags": "--resume c1", "cwd": self.repo, "prompt": "[T1] continue"});
        let jid = board::create_job(&self.app, "agent", args, Some(t), "start", None).unwrap();
        midna::run_job(&self.app, &self.app.db.q1("SELECT * FROM jobs WHERE id = ?", p![jid]).unwrap().unwrap());
        let calls = std::fs::read_to_string(self.dir.path().join("calls")).unwrap_or_default();
        let calls = calls
            .lines()
            .map(|l| {
                let (m, p) = l.split_once(' ').unwrap();
                (m.to_string(), serde_json::from_str(p).unwrap())
            })
            .collect();
        (t, calls)
    }
}

fn queued(calls: &[(String, Value)]) -> Vec<String> {
    calls.iter().filter(|(m, _)| m == "queue.add").map(|(_, p)| p["text"].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_cold_resume_opens_at_once_and_compacts_in_the_tab_ahead_of_the_prompt() {
    let b = new_board("2000-01-01T00:00:00Z");
    let (t, calls) = b.resume();

    let opens: Vec<&Value> = calls.iter().filter(|(m, _)| m == "session.open").map(|(_, p)| p).collect();
    assert_eq!(opens.len(), 1, "{calls:?}");
    assert_eq!(opens[0]["resume"], "c1");
    assert!(opens[0].get("prompt").is_none(), "the prompt waits behind the compact: {}", opens[0]);
    assert_eq!(calls[0].0, "session.open", "the terminal opens before anything is queued: {calls:?}");
    assert_eq!(queued(&calls), ["/compact", "[T1] continue"]);
    assert!(calls.iter().filter(|(m, _)| m == "queue.add").all(|(_, p)| p["session"] == "S9" && p["when"]["kind"] == "idle"), "{calls:?}");
    assert!(!b.dir.path().join("headless").exists(), "no headless compact runs before the terminal opens");

    let events: Vec<String> = b.app.db.q("SELECT text FROM events WHERE task_id = ?", p![t]).unwrap().iter().map(|e| e.st("text")).collect();
    assert!(events.iter().any(|e| e.contains("so the new terminal compacts it before carrying on")), "{events:?}");
}

#[test]
fn a_warm_resume_opens_with_its_prompt_and_no_compact() {
    let b = new_board(&taskboardd::util::now_iso());
    let (_, calls) = b.resume();

    let open = calls.iter().find(|(m, _)| m == "session.open").map(|(_, p)| p).unwrap();
    assert_eq!(open["prompt"], "[T1] continue");
    assert!(queued(&calls).is_empty(), "{calls:?}");
}
