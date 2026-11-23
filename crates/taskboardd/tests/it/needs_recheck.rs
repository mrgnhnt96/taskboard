//! The Needs you re-check (`recheck::run`): questions and human tasks the screener never read under
//! today's rules are read again; what needn't wait on the owner comes off the list and is recorded.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, handled, midna, p, recheck, reports, runner};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

const OLD: &str = "2026-11-01T00:00:00Z";

/// A board whose screener is a script printing what `say` last set; `s1` is on T1. The rule file
/// `rules.md` is written now, after anything the tests backdate.
fn board(first: Value) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let rules = dir.path().join("rules.md");
    std::fs::write(&rules, "Amplitude questions: read it with the Amplitude MCP.").unwrap();
    cfg.claude = dir.path().join("claude").to_string_lossy().to_string();
    cfg.questions.screen = true;
    cfg.questions.rules = vec![rules.to_string_lossy().to_string()];
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    let b = Board { app, dir };
    b.say(first);
    midna::sync(
        &b.app,
        &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})],
        &[],
    )
    .unwrap();
    taskboardd::api::dispatch(&b.app, "POST", "/tasks", &Default::default(), &json!({"title": "Review the marketing site", "detail": "Do it.", "project": "webapp", "pickup": "new"}))
        .unwrap();
    b.report("tb.take", json!({"task": "T1"})).unwrap();
    b
}

impl Board {
    /// What the screener prints from now on; every call it gets is counted in `calls`.
    fn say(&self, verdict: Value) {
        let script = self.dir.path().join("claude");
        let out = json!({"structured_output": verdict}).to_string();
        std::fs::write(&script, format!("#!/bin/sh\necho x >> '{}'\necho '{out}'\n", self.calls_file().display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn calls_file(&self) -> PathBuf {
        self.dir.path().join("calls")
    }
    fn calls(&self) -> usize {
        std::fs::read_to_string(self.calls_file()).map(|s| s.lines().count()).unwrap_or(0)
    }
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {"branch": "feature/x"}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    /// Moves when everything on task `id` happened back to before the rule file was written.
    fn backdate(&self, id: i64) {
        self.app.db.x("UPDATE events SET at = ? WHERE task_id = ?", p![OLD, id]).unwrap();
        self.app.db.x("UPDATE tasks SET created_at = ? WHERE id = ?", p![OLD, id]).unwrap();
    }
    fn human(&self) -> i64 {
        let out = self
            .report("tb.new_task", json!({"title": "Find how long Amplitude keeps events", "detail": "1. Open Amplitude settings.", "project": "webapp", "kind": "human"}))
            .unwrap();
        runner::start_queued(&self.app).unwrap();
        out["created"][0].as_str().unwrap()[1..].parse().unwrap()
    }
    fn handled(&self) -> Vec<Row> {
        self.app.db.q("SELECT * FROM events WHERE kind = ? ORDER BY id", p![handled::KIND]).unwrap()
    }
    fn sent_to(&self, task: i64) -> Vec<String> {
        self.app
            .db
            .q("SELECT args FROM jobs WHERE kind = 'deliver' AND task_id = ? ORDER BY id", p![task])
            .unwrap()
            .iter()
            .map(|j| serde_json::from_str::<Value>(&j.st("args")).unwrap()["text"].as_str().unwrap_or("").to_string())
            .collect()
    }
}

#[test]
fn an_old_human_task_an_agent_can_do_goes_back_to_the_agent_that_made_it() {
    let b = board(json!({"verdict": "owner"}));
    let h = b.human();
    assert_eq!(b.row(h).st("status"), "needs", "the screen kept it when it was raised");
    assert!(recheck::picks(&b.app).unwrap().is_empty(), "read under today's rules when it was raised");
    b.backdate(h);
    assert_eq!(recheck::picks(&b.app).unwrap().len(), 1, "raised before the rules changed");

    b.say(json!({"verdict": "agent", "why": "The Amplitude MCP reads the retention."}));
    recheck::run(&b.app).unwrap();

    let t = b.row(h);
    assert_eq!((t.st("status"), t.st("owner_answer")), ("done".to_string(), "board_agent".to_string()));
    let sent = b.sent_to(1);
    assert!(sent.iter().any(|m| m.contains(&format!("off {}'s list", b.app.cfg.owner)) && m.contains("Amplitude MCP") && m.contains("--no-pr")), "{sent:?}");
    let rec = b.handled();
    assert_eq!(rec.len(), 1);
    let data: Value = serde_json::from_str(&rec[0].st("data")).unwrap();
    assert_eq!((data["by"].as_str(), data["item"].as_str(), data["verdict"].as_str()), (Some("recheck"), Some("human"), Some("agent")));
    assert_eq!(data["to"], json!(["T1"]));
}

#[test]
fn an_old_human_task_with_no_live_maker_becomes_an_agent_task_with_no_pr() {
    let b = board(json!({"verdict": "owner"}));
    let h = b.human();
    b.app.db.x("UPDATE tasks SET status = 'done' WHERE id = 1", p![]).unwrap();
    b.backdate(h);
    b.say(json!({"verdict": "agent", "why": "The Amplitude MCP reads the retention."}));
    recheck::run(&b.app).unwrap();

    assert_eq!(b.row(h).st("owner_answer"), "board_agent");
    let made = b.app.db.q("SELECT * FROM tasks WHERE id > ? AND COALESCE(kind, '') != 'human'", p![h]).unwrap();
    assert_eq!(made.len(), 1, "one agent task does it");
    assert_eq!(made[0].st("title"), "Find how long Amplitude keeps events");
    assert_eq!(made[0].i("ships_pr"), Some(0));
    assert!(made[0].st("detail").contains("Amplitude MCP"));
}

#[test]
fn haiku_unsure_goes_to_sonnet_and_a_kept_item_isn_t_read_again_until_it_changes() {
    let b = board(json!({"verdict": "owner"}));
    let h = b.human();
    b.backdate(h);
    let before = b.calls();
    recheck::run(&b.app).unwrap();
    assert_eq!(b.calls() - before, 2, "the first model kept it, so the second read it too");
    assert_eq!(b.row(h).st("status"), "needs", "both kept it");
    assert!(b.handled().is_empty());

    recheck::run(&b.app).unwrap();
    assert_eq!(b.calls() - before, 2, "unchanged since its last read");
    b.app.db.x("UPDATE tasks SET detail = 'Open Amplitude, then the org settings.' WHERE id = ?", p![h]).unwrap();
    assert_eq!(recheck::picks(&b.app).unwrap().len(), 1, "its steps changed");
}

#[test]
fn an_old_question_the_rules_answer_is_answered_like_the_owner_would() {
    let b = board(json!({"verdict": "ask", "question": "How long does Amplitude keep events?"}));
    b.report("tb.question", json!({"text": "How long does Amplitude keep events?"})).unwrap();
    assert_eq!(b.row(1).st("needs_reason"), "question");
    b.backdate(1);

    b.say(json!({"verdict": "answered", "answer": "Read it with the Amplitude MCP.", "source": "rules.md"}));
    recheck::run(&b.app).unwrap();

    let t = b.row(1);
    assert!(t.s("answered_at").is_some(), "it's off the list");
    let sent = b.sent_to(1);
    assert!(sent.iter().any(|m| m.starts_with("[task-board:T1] Answered from rules.md without asking") && m.contains("Amplitude MCP")), "{sent:?}");
    let data: Value = serde_json::from_str(&b.handled()[0].st("data")).unwrap();
    assert_eq!((data["item"].as_str(), data["was"].as_str()), (Some("question"), Some("How long does Amplitude keep events?")));
}
