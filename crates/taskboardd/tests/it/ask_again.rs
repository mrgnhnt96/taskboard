//! A task waiting on the owner's answer whose terminal is gone: Focus opens a new terminal that sums up
//! the task and asks the question again, instead of refusing with no terminal to show.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, fields, p};

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
    Board { app, _dir: dir }
}

impl Board {
    fn post(&self, path: &str) -> taskboardd::util::Result<Value> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &json!({}))
    }
    fn task(&self, f: Vec<(&'static str, Value)>) -> i64 {
        let body = json!({"title": "Save the sign-in token", "detail": "Do it.", "project": "webapp"});
        let t = api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &body).unwrap()["id"].as_i64().unwrap();
        board::update_task(&self.app, t, f).unwrap();
        t
    }
    fn card(&self, t: i64) -> Value {
        board::task_card(&self.app, &board::get_task(&self.app, t).unwrap()).unwrap()
    }
}

#[test]
fn focus_on_a_question_with_no_terminal_opens_one_that_asks_again() {
    let b = board();
    let t = b.task(fields!["status" => "needs", "needs_reason" => "question", "question" => "Which option do you want?",
                           "ask" => json!({"picker": false, "questions": [{"question": "Which option do you want?",
                               "options": [{"label": "Allow the agent", "description": ""}, {"label": "Drop it", "description": ""}]}]}).to_string()]);
    assert_eq!(b.card(t)["terminal_open"], false);

    let v = b.post(&format!("/tasks/T{t}/focus")).unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(v["opened"], true);
    let job = b.app.db.q1("SELECT * FROM jobs WHERE task_id = ? AND kind = 'agent'", p![t]).unwrap().expect("a new terminal");
    let args: Value = serde_json::from_str(job.s("args").unwrap()).unwrap();
    let prompt = args["prompt"].as_str().unwrap();
    assert!(prompt.starts_with(&format!("[task-board:T{t}] ")), "{prompt}");
    assert!(prompt.contains("Sum up") && prompt.contains("Which option do you want?\n- Allow the agent\n- Drop it"), "{prompt}");
    assert!(prompt.contains("Save the sign-in token"), "the handoff follows: {prompt}");
    assert_eq!(args["focus"], true, "the owner asked for it, so it's shown once open");
    let log: Vec<String> = b.app.db.q("SELECT text FROM events WHERE task_id = ?", p![t]).unwrap().iter().map(|r| r.st("text")).collect();
    assert!(log.iter().any(|l| l == "Opened a new terminal to answer the question"), "{log:?}");
}

#[test]
fn the_terminal_opened_to_ask_again_is_focused() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let midna_exe = dir.path().join("fake-midna");
    std::fs::write(
        &midna_exe,
        "#!/bin/sh\nprintf '%s %s\\n' \"$2\" \"$3\" >> \"$(dirname \"$0\")/calls\"\nif [ \"$2\" = session.open ]; then echo '{\"id\": \"S9\"}'; exit 0; fi\necho '{}'\n",
    )
    .unwrap();
    std::fs::set_permissions(&midna_exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    cfg.midna = midna_exe;
    let app = App::for_tests(cfg);
    let path = repo.to_string_lossy().to_string();
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": path}]).to_string())).unwrap();
    let job = |args: Value| {
        let jid = board::create_job(&app, "agent", args, None, "start", None).unwrap();
        taskboardd::midna::run_job(&app, &app.db.q1("SELECT * FROM jobs WHERE id = ?", p![jid]).unwrap().unwrap());
        let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap_or_default();
        std::fs::remove_file(dir.path().join("calls")).ok();
        calls.lines().map(|l| l.split_once(' ').unwrap().0.to_string()).collect::<Vec<_>>()
    };

    assert_eq!(job(json!({"cwd": path, "prompt": "hi", "focus": true})), ["session.open", "session.focus"]);
    assert_eq!(job(json!({"cwd": path, "prompt": "hi"})), ["session.open"], "other terminals open where they are");
}

#[test]
fn focus_with_no_terminal_and_no_question_still_refuses() {
    let b = board();
    let t = b.task(fields!["status" => "working"]);
    let e = b.post(&format!("/tasks/T{t}/focus")).unwrap_err();
    assert_eq!((e.status, e.message.as_str()), (409, "This task has no open Midna terminal."));
}
