//! The owner's steps (config.toml's `[[steps]]`): the agent is told them, records each with `tb step
//! done`, and can't finish until they're recorded.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, handoff, midna, reports};

const STEPS: &str = r#"
[[steps]]
name = "Author-side review"
projects = "webapp|api"
prompt = "Run /author-review and fix what it finds."

[[steps]]
name = "Changelog"
before = "done"
prompt = "Add a line to CHANGELOG.md."

[[steps]]
name = "Elsewhere"
projects = "other"
prompt = "Not for webapp."
"#;

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn board(config: &str) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    std::fs::write(&cfg.config_path, config).unwrap();
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    Board { app, _dir: dir }
}

impl Board {
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": ""});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn new_task(&self) -> i64 {
        let body = json!({"title": "Add login", "project": "webapp"});
        let id = api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &body).unwrap()["id"].as_i64().unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")})).unwrap();
        id
    }
    fn steps(&self) -> Value {
        let mut q = Query::new();
        q.insert("session".into(), "s1".into());
        api::dispatch(&self.app, "GET", "/steps", &q, &Value::Null).unwrap()
    }
    fn status(&self, id: i64) -> String {
        self.task(id).st("status")
    }
    fn task(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn card(&self, id: i64) -> Value {
        board::task_card(&self.app, &self.task(id)).unwrap()
    }
    fn owner(&self, id: i64, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", &format!("/tasks/T{id}/step"), &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
}

const PR: &str = "https://github.com/acme/webapp/pull/9";

#[test]
fn the_handoff_lists_the_projects_steps() {
    let b = board(STEPS);
    let id = b.new_task();
    let h = handoff::build(&b.app, id).unwrap();
    assert!(h.contains("Before you open the PR, do this step:\n1. Author-side review: Run /author-review and fix what it finds. Then: tb step done \"Author-side review\""), "{h}");
    assert!(h.contains("Before you run tb done, do this step:\n1. Changelog"), "{h}");
    assert!(!h.contains("Elsewhere"), "{h}");
    assert!(h.contains("step fail \"<name>\""), "{h}");
}

#[test]
fn done_waits_for_the_steps() {
    let b = board(STEPS);
    let id = b.new_task();
    let (code, why) = b.report("tb.done", json!({"summary": "Done", "pr": PR})).unwrap_err();
    assert_eq!(code, 409);
    assert!(why.contains("- Author-side review (Run /author-review") && why.contains("- Changelog"), "{why}");
    assert_eq!(b.status(id), "working");

    let v = b.report("tb.step", json!({"name": "author-side REVIEW", "note": "fixed two findings"})).unwrap();
    assert_eq!(v["step"], "Author-side review");
    assert_eq!(v["left"], json!(["Changelog"]));
    let s = b.steps();
    assert_eq!(s["steps"].as_array().unwrap().len(), 2);
    assert_eq!(s["steps"][0]["done"], true);
    assert_eq!(s["steps"][1]["done"], false);

    let (_, why) = b.report("tb.done", json!({"summary": "Done", "pr": PR})).unwrap_err();
    assert!(!why.contains("Author-side review") && why.contains("Changelog"), "{why}");
    b.report("tb.step", json!({"name": "Changelog"})).unwrap();
    b.report("tb.done", json!({"summary": "Done", "pr": PR})).unwrap();
    assert_eq!(b.status(id), "done");
}

#[test]
fn without_a_pr_only_the_done_steps_count() {
    let b = board(STEPS);
    let id = b.new_task();
    b.report("tb.step", json!({"name": "Changelog"})).unwrap();
    b.report("tb.done", json!({"summary": "Nothing to change"})).unwrap();
    assert_eq!(b.status(id), "done");
}

#[test]
fn an_unknown_step_is_refused_with_the_names() {
    let b = board(STEPS);
    b.new_task();
    let (code, why) = b.report("tb.step", json!({"name": "Lint"})).unwrap_err();
    assert_eq!(code, 400);
    assert!(why.contains("“Author-side review”, “Changelog”"), "{why}");
}

#[test]
fn a_broken_steps_table_turns_them_off() {
    let b = board("[[steps]]\nname = \"x\"\nbefore = \"later\"\nprompt = \"y\"\n");
    let id = b.new_task();
    b.report("tb.done", json!({"summary": "Done", "pr": PR})).unwrap();
    assert_eq!(b.status(id), "done");
}

const KINDS: &str = r#"
[[steps]]
name = "Review"
prompt = "Review {branch} against {base}."
check = "test -f review.ok"

[[steps]]
name = "Tests"
run = "make test"

[[steps]]
name = "Design sign-off"
owner = true
open = "https://figma.com/file/abc?task={task}"
prompt = "Sign off the screens."
"#;

#[test]
fn a_failed_check_or_script_doesnt_count() {
    let b = board(KINDS);
    let id = b.new_task();
    let v = b.report("tb.step", json!({"name": "Review", "via": "done", "ok": false, "output": "no review.ok"})).unwrap();
    assert_eq!(v["passed"], false);
    assert_eq!(b.steps()["steps"][0]["done"], false);
    let (code, why) = b.report("tb.step", json!({"name": "Tests", "via": "done"})).unwrap_err();
    assert_eq!(code, 400, "a script step is run, not declared done");
    assert!(why.contains("tb step run \"Tests\""), "{why}");
    b.report("tb.step", json!({"name": "Tests", "via": "run", "ok": true, "output": "ok"})).unwrap();
    assert_eq!(b.steps()["steps"][1]["done"], true);
    assert_eq!(b.status(id), "working");
}

#[test]
fn the_owners_step_waits_for_them_and_carries_on_after_done() {
    let b = board(KINDS);
    let id = b.new_task();
    let (code, _) = b.report("tb.step", json!({"name": "Design sign-off", "via": "done"})).unwrap_err();
    assert_eq!(code, 409, "the agent can't do the owner's step");

    let v = b.report("tb.step_ask", json!({"name": "design sign-off"})).unwrap();
    assert_eq!(v["open"], format!("https://figma.com/file/abc?task=T{id}"));
    assert_eq!(b.status(id), "needs");
    let card = b.card(id);
    assert_eq!(card["step"]["name"], "Design sign-off");
    assert_eq!(card["step"]["failed"], false);
    assert!(card["question"].as_str().unwrap().contains("(at https://figma.com/file/abc?task=T"), "{card}");

    let (code, _) = b.owner(id, json!({"session": "s1"})).unwrap_err();
    assert_eq!(code, 409, "not from the agent's own terminal");
    b.owner(id, json!({})).unwrap();
    assert_eq!(b.steps()["steps"][2]["done"], true);
    assert!(b.card(id)["step"].is_null());
    let answered = b.app.db.q("SELECT text FROM events WHERE task_id = ? AND kind = 'answer'", taskboardd::p![id]).unwrap();
    assert!(answered.iter().any(|r| r.st("text").contains("Step done: Design sign-off")), "the agent is told to carry on");
}

#[test]
fn a_step_that_cant_pass_waits_for_an_answer_or_a_skip() {
    let b = board(KINDS);
    let id = b.new_task();
    b.report("tb.step_fail", json!({"name": "Tests", "why": "the suite needs a VPN"})).unwrap();
    assert_eq!(b.status(id), "needs");
    assert_eq!(b.card(id)["step"]["failed"], true);
    b.owner(id, json!({"skip": true})).unwrap();
    assert_eq!(b.steps()["steps"][1]["done"], true);
    let last = b.app.db.q1("SELECT text FROM events WHERE task_id = ? AND kind = 'step' ORDER BY id DESC", taskboardd::p![id]).unwrap().unwrap();
    assert_eq!(last.st("text"), "Step skipped: Tests");
}

#[test]
fn placeholders_fill_in_the_handoff() {
    let b = board(KINDS);
    let id = b.new_task();
    let h = handoff::build(&b.app, id).unwrap();
    assert!(h.contains(&format!("at https://figma.com/file/abc?task=T{id}")), "{h}");
    assert!(h.contains("against main"), "{h}");
    assert!(h.contains("tb step ask \"Design sign-off\", then end your turn"), "{h}");
}
