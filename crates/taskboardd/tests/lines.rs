//! A terminal's line: asks split off into their own tasks, queued in the terminal or switched to, and the
//! terminal moving on to the next one by itself once its task is done.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, fields, lines, midna, p, reports, runner};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    fn get(&self, path: &str) -> Value {
        let (p, q) = path.split_once('?').unwrap_or((path, ""));
        let query: Query = q.split('&').filter(|x| !x.is_empty()).filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
        api::dispatch(&self.app, "GET", p, &query, &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn report(&self, event: &str, session: &str, extra: Value) -> Value {
        self.try_report(event, session, extra).unwrap_or_else(|e| panic!("{event}: {e}"))
    }
    fn try_report(&self, event: &str, session: &str, extra: Value) -> Result<Value, String> {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| e.message)
    }
    fn add_session(&self, sid: &str) {
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[])
            .unwrap();
    }
    /// A terminal on a fresh `--here` task; its ref.
    fn on_task(&self, sid: &str, title: &str) -> String {
        self.add_session(sid);
        let r = self.report("tb.new_task", sid, json!({"title": title, "detail": "Asked for.", "here": true}));
        r["created"][0].as_str().unwrap().to_string()
    }
    fn split(&self, sid: &str, title: &str, line: &str) -> Value {
        self.report("tb.new_task", sid, json!({"title": title, "detail": "A separate ask.", "here": true, "line": line}))
    }
    fn task(&self, r: &str) -> Value {
        self.get(&format!("tasks/{r}"))
    }
    fn on(&self, sid: &str) -> Option<String> {
        self.get(&format!("whoami?session={sid}"))["task"]["ref"].as_str().map(|s| s.to_string())
    }
}

fn id(r: &str) -> i64 {
    r.trim_start_matches('T').parse().unwrap()
}

#[test]
fn the_owners_prompts_on_a_task_ask_whether_the_ask_is_part_of_it() {
    let b = new_board();
    let t1 = b.on_task("s1", "Fix the header");
    let out = b.report("hook.prompt", "s1", json!({"prompt": "Also make the footer sticky"}));
    let c = out["context"].as_str().unwrap();
    assert!(c.contains(&format!("You're on {t1}")) && c.contains("--here --next") && c.contains("--here --now"), "{c}");

    for not_asks in [format!("[task-board:{t1}] continue"), "/compact".into(), "<command-name>x</command-name>".into()] {
        let out = b.report("hook.prompt", "s1", json!({"prompt": not_asks}));
        assert!(!out["context"].as_str().unwrap_or("").contains("--here --next"), "{not_asks}");
    }

    b.add_session("s2");
    let out = b.report("hook.prompt", "s2", json!({"prompt": "Make the header blue"}));
    assert!(!out["context"].as_str().unwrap_or("").contains("--here --next"), "no task, nothing to split from");
}

#[test]
fn next_queues_the_ask_in_this_terminal_and_nothing_else_starts_it() {
    let b = new_board();
    let t1 = b.on_task("s1", "Fix the header");
    let r = b.split("s1", "Make the footer sticky", "next");
    let t2 = r["created"][0].as_str().unwrap().to_string();
    assert_eq!(r["status"], "queued");
    assert!(r["context"].as_str().unwrap().contains(&format!("Carry on with {t1}")));
    assert_eq!(b.on("s1").as_deref(), Some(t1.as_str()), "the terminal stays on its task");

    let card = b.task(&t2);
    assert_eq!(card["status"], "queued");
    assert!(card["session_id"].is_null());
    assert_eq!(card["line"]["kind"], "queued");
    assert_eq!(card["line"]["label"], "Queued in Term s1");
    assert_eq!(card["line"]["after"], t1);
    assert_eq!(b.get("whoami?session=s1")["line"][0]["ref"], t2);

    board::update_task(&b.app, id(&t2), fields!["pickup" => "queue"]).unwrap();
    assert!(runner::start_queued(&b.app).unwrap().is_empty(), "a task in a line starts only there");

    let plain = b.try_report("tb.new_task", "s1", json!({"title": "Another", "here": true}));
    assert!(plain.unwrap_err().contains("--here --next"), "a plain --here says how to split");
}

#[test]
fn now_switches_and_the_old_task_waits_here_to_resume() {
    let b = new_board();
    let t1 = b.on_task("s1", "Fix the header");
    let r = b.split("s1", "Fix the login crash", "now");
    let t2 = r["created"][0].as_str().unwrap().to_string();
    assert_eq!(r["status"], "working");
    assert!(r["context"].as_str().unwrap().contains(&format!("{t1} waits here to resume")));
    assert_eq!(b.on("s1").as_deref(), Some(t2.as_str()));
    let old = b.task(&t1);
    assert_eq!(old["status"], "queued");
    assert_eq!(old["line"]["kind"], "resume");
    assert_eq!(old["line"]["label"], "To resume in Term s1");

    // A task waiting on the owner's answer keeps its terminal.
    board::update_task(&b.app, id(&t2), fields!["status" => "needs", "needs_reason" => "question", "question" => "Which?"]).unwrap();
    let held = b.try_report("tb.new_task", "s1", json!({"title": "X", "here": true, "line": "now"}));
    assert!(held.unwrap_err().contains("answer"));
}

#[test]
fn done_moves_the_terminal_on_to_the_next_task_in_its_line() {
    let b = new_board();
    let t1 = b.on_task("s1", "Fix the header");
    let t2 = b.split("s1", "Make the footer sticky", "next")["created"][0].as_str().unwrap().to_string();
    let t3 = b.split("s1", "Tidy the nav", "next")["created"][0].as_str().unwrap().to_string();

    let out = b.report("tb.done", "s1", json!({"summary": "Fixed it.", "no_pr": "Test"}));
    assert_eq!(out["task"], t1);
    let c = out["context"].as_str().unwrap();
    assert!(c.contains(&format!("[task-board:{t2}] {t1} is done")) && c.contains("moves on to"), "{c}");
    assert_eq!(b.on("s1").as_deref(), Some(t2.as_str()));
    let card = b.task(&t2);
    assert_eq!(card["status"], "working");
    assert!(card["line"].is_null());
    assert_eq!(b.task(&t3)["line"]["pos"], 1);

    // The handoff went with the move, so the next prompt doesn't send it again.
    let next = b.report("hook.prompt", "s1", json!({"prompt": "looks good"}));
    assert!(!next["context"].as_str().unwrap_or("").contains(&format!("[task-board:{t2}]")));

    let out = b.report("tb.fail", "s1", json!({"reason": "Can't."}));
    assert!(out["context"].as_str().unwrap().contains(&format!("[task-board:{t3}]")));
    let out = b.report("tb.done", "s1", json!({"summary": "Done.", "no_pr": "Test"}));
    assert!(out["context"].is_null(), "an empty line: nothing to move on to");
}

#[test]
fn the_owner_marking_it_done_sends_the_next_task_to_the_terminal() {
    let b = new_board();
    let t1 = b.on_task("s1", "Fix the header");
    let t2 = b.split("s1", "Make the footer sticky", "next")["created"][0].as_str().unwrap().to_string();
    b.post(&format!("tasks/{t1}/done"), json!({"summary": "Done by hand."}));
    b.app.db.tx(|| lines::tick(&b.app)).unwrap();
    assert_eq!(b.on("s1").as_deref(), Some(t2.as_str()));
    let out = b.report("hook.stop", "s1", json!({"last_message": "ok"}));
    assert!(out["deliver"]["text"].as_str().unwrap().contains(&format!("[task-board:{t2}]")));
}

#[test]
fn switch_and_drop_work_on_this_terminals_line_only() {
    let b = new_board();
    let t1 = b.on_task("s1", "Fix the header");
    let t2 = b.split("s1", "Make the footer sticky", "next")["created"][0].as_str().unwrap().to_string();
    let t3 = b.split("s1", "Tidy the nav", "next")["created"][0].as_str().unwrap().to_string();

    let out = b.report("tb.switch", "s1", json!({"to": t3}));
    assert!(out["context"].as_str().unwrap().contains(&format!("{t1} waits here to resume")));
    assert_eq!(b.on("s1").as_deref(), Some(t3.as_str()));
    let line: Vec<String> = b.get("whoami?session=s1")["line"].as_array().unwrap().iter().map(|x| x["ref"].as_str().unwrap().to_string()).collect();
    assert_eq!(line, vec![t1.clone(), t2.clone()], "the task switched away from goes first");

    let t4 = b.on_task("s2", "Elsewhere");
    assert!(b.try_report("tb.switch", "s1", json!({"to": t4})).unwrap_err().contains("isn't in this terminal's line"));

    b.report("tb.line", "s1", json!({"drop": t2}));
    let card = b.task(&t2);
    assert!(card["line"].is_null());
    assert_eq!(card["status"], "queued");
    let listed = b.report("tb.line", "s1", json!({}));
    assert!(listed["context"].as_str().unwrap().contains(&format!("{t1} “Fix the header” to resume")));
}

fn from_app() -> Query {
    let mut q = Query::new();
    q.insert(api::FROM.into(), "app".into());
    q
}

#[test]
fn the_owners_start_takes_a_task_out_of_its_line_and_runs_it() {
    let b = new_board();
    let t1 = b.on_task("s1", "Fix the header");
    let t2 = b.on_task("s2", "Tidy the nav");
    board::update_task(&b.app, id(&t2), fields!["status" => "queued", "session_id" => null]).unwrap();
    // `tb take` on a busy terminal shelves its task into that terminal's line.
    b.report("tb.take", "s1", json!({"task": t2}));
    assert_eq!(b.on("s1").as_deref(), Some(t2.as_str()));
    assert_eq!(b.task(&t1)["line"]["session"], "s1");

    let card = api::dispatch(&b.app, "POST", &format!("tasks/{t1}/start"), &from_app(), &json!({"mode": "new"})).unwrap();
    assert!(card["line"].is_null(), "Start takes it out of the line");
    assert_eq!(card["starting"], true, "and runs it");
    assert!(b.get("whoami?session=s1")["line"].as_array().unwrap().is_empty());
    let said = b.app.db.count("SELECT COUNT(*) FROM events WHERE task_id = ? AND text = 'Taken out of Term s1''s line'", p![id(&t1)]).unwrap();
    assert_eq!(said, 1);

    // It isn't pulled back into s1 once that terminal is free.
    b.report("tb.done", "s1", json!({"summary": "Done.", "no_pr": "Test"}));
    b.app.db.tx(|| lines::tick(&b.app)).unwrap();
    assert_eq!(b.on("s1"), None);
}

#[test]
fn a_closed_terminals_line_goes_back_to_the_board() {
    let b = new_board();
    let t1 = b.on_task("s1", "Fix the header");
    let t2 = b.split("s1", "Make the footer sticky", "next")["created"][0].as_str().unwrap().to_string();
    b.post(&format!("tasks/{t1}/done"), json!({"summary": "Done."}));
    b.app.db.x("UPDATE tasks SET line_session = 's1' WHERE id = ?", p![id(&t2)]).unwrap();
    board::session_gone(&b.app, "s1", "prompt_input_exit").unwrap();
    b.app.db.tx(|| lines::tick(&b.app)).unwrap();
    let card = b.task(&t2);
    assert!(card["line"].is_null());
    assert_eq!(card["status"], "queued");
    assert!(card["latest"].as_str().unwrap().contains("press Start"));
    let s = board::get_session(&b.app, Some("s1")).unwrap().unwrap();
    assert_eq!(s.s("status"), Some("gone"));
}

#[test]
fn a_parked_task_waits_at_the_back_of_its_terminals_line() {
    let b = new_board();
    let t1 = b.on_task("s1", "Fix the header");
    let t2 = b.split("s1", "Make the footer sticky", "next")["created"][0].as_str().unwrap().to_string();
    let other = b.on_task("s2", "The API it needs");

    let out = b.report("tb.wait_for", "s1", json!({"tasks": [other], "why": "Needs the API"}));
    assert_eq!(out["parked"], true);
    assert_eq!(out["moved_on"], true);
    assert!(out["context"].as_str().unwrap().contains(&format!("[task-board:{t2}]")));
    assert_eq!(b.on("s1").as_deref(), Some(t2.as_str()));
    assert_eq!(b.task(&t1)["line"]["session"], "s1", "it carries on in this terminal, not a new one");
    board::update_task(&b.app, id(&t1), fields!["pickup" => "new"]).unwrap();
    assert!(runner::start_queued(&b.app).unwrap().is_empty());

    let out = b.report("tb.done", "s1", json!({"summary": "Done.", "no_pr": "Test"}));
    assert!(out["context"].is_null(), "what it waits for isn't done, so nothing is ready");
    b.report("tb.done", "s2", json!({"summary": "Done.", "no_pr": "Test"}));
    b.app.db.tx(|| lines::tick(&b.app)).unwrap();
    assert_eq!(b.on("s1").as_deref(), Some(t1.as_str()));
}
