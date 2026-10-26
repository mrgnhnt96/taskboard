//! Human tasks (`tb task new --human`, H<n>): only the owner does them, the runner never starts a
//! terminal on one, and the tasks waiting for one go on once the owner presses Done in the app.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query, FROM};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, dispatch, midna, p, reports, runner};

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

const STEPS: &str = "1. Sign up at https://signup.cloud.oracle.com.\n2. Pick the home region.\n3. Note the tenancy name.";

impl Board {
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
    fn from_app(&self, path: &str, body: Value) -> Result<Value, (u16, String)> {
        let q: Query = [(FROM.to_string(), "app".to_string())].into_iter().collect();
        api::dispatch(&self.app, "POST", path, &q, &body).map_err(|e| (e.status, e.message))
    }
    fn report(&self, event: &str, session: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {"branch": "feature/x"}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn session(&self, sid: &str) {
        let repo = self.dir.path().join("webapp");
        midna::sync(
            &self.app,
            &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})],
            &[],
        )
        .unwrap();
    }
    fn task(&self, title: &str) -> i64 {
        self.post("/tasks", json!({"title": title, "detail": "Do it.", "project": "webapp", "pickup": "new"})).unwrap()["id"].as_i64().unwrap()
    }
    /// An agent on `sid` adds a human task; its ref.
    fn human(&self, sid: &str, extra: Value) -> String {
        let mut body = json!({"title": "Create an Oracle Cloud account", "detail": STEPS, "project": "webapp", "kind": "human"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        let out = self.report("tb.new_task", sid, body).unwrap();
        out["created"][0].as_str().unwrap().to_string()
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn starts(&self, id: i64) -> Vec<Row> {
        self.app.db.q("SELECT * FROM jobs WHERE kind = 'agent' AND purpose = 'start' AND task_id = ? ORDER BY id", p![id]).unwrap()
    }
}

fn id_of(r: &str) -> i64 {
    r[1..].parse().unwrap()
}

#[test]
fn a_human_task_goes_to_the_owner_not_to_a_terminal() {
    let b = new_board();
    b.session("s1");
    let h = b.human("s1", json!({}));
    assert!(h.starts_with('H'), "{h}");
    let id = id_of(&h);
    let card = b.get(&format!("tasks/{h}"));
    assert_eq!(card["kind"], "human");
    assert_eq!(card["ref"], h);
    assert_eq!(card["ships_pr"], false);
    assert_eq!(b.get(&format!("tasks/T{id}"))["ref"], h, "T<n> still finds it");

    assert!(runner::start_queued(&b.app).unwrap().is_empty());
    assert!(b.starts(id).is_empty(), "no terminal starts on it");
    let t = b.row(id);
    assert_eq!(t.st("status"), "needs");
    assert_eq!(t.st("needs_reason"), "human");
    assert!(dispatch::alerts(&b.app).iter().any(|a| a["task_id"] == id || a["task"] == format!("T{id}")), "the owner is told");

    let (status, msg) = b.from_app(&format!("tasks/{h}/start"), json!({"mode": "new"})).unwrap_err();
    assert_eq!(status, 409, "{msg}");
    assert!(msg.contains("human task"), "{msg}");
    b.session("s2");
    let (_, msg) = b.report("tb.take", "s2", json!({"task": h})).unwrap_err();
    assert!(msg.contains("human task"), "{msg}");
}

#[test]
fn only_the_app_marks_it_done() {
    let b = new_board();
    b.session("s1");
    let h = b.human("s1", json!({}));
    runner::start_queued(&b.app).unwrap();

    let (status, _) = b.post(&format!("tasks/{h}/did"), json!({})).unwrap_err();
    assert_eq!(status, 403);
    let (status, _) = b.post(&format!("tasks/{h}/done"), json!({})).unwrap_err();
    assert_eq!(status, 409);
    let (status, _) = b.report("tb.done", "s1", json!({"task": h, "summary": "Did it."})).unwrap_err();
    assert_eq!(status, 409);
    let (status, _) = b.report("tb.fail", "s1", json!({"task": h, "reason": "Couldn't."})).unwrap_err();
    assert_eq!(status, 409);
    let (status, _) = b.post(&format!("tasks/{h}/fail"), json!({})).unwrap_err();
    assert_eq!(status, 409);
    assert_eq!(b.row(id_of(&h)).st("status"), "needs", "nothing an agent sends finishes it");

    let card = b.from_app(&format!("tasks/{h}/did"), json!({"note": "Tenancy acme-dev, region us-ashburn-1"})).unwrap();
    assert_eq!(card["status"], "done");
    assert_eq!(card["owner_note"], "Tenancy acme-dev, region us-ashburn-1");
    assert!(dispatch::alerts(&b.app).iter().all(|a| a["key"] != format!("human-{}", id_of(&h))), "its alert is cleared");
}

#[test]
fn a_parked_task_resumes_with_the_owners_note() {
    let b = new_board();
    let two = b.task("Deploy to Oracle Cloud");
    b.session("s2");
    b.report("tb.take", "s2", json!({"task": format!("T{two}")})).unwrap();
    let dir = b.app.cfg.claude_projects.join("-webapp");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("c-s2.jsonl"), "{\"type\":\"user\"}\n").unwrap();

    let h = b.human("s2", json!({}));
    let out = b.report("tb.wait_for", "s2", json!({"tasks": [h.clone()], "why": "Needs the account"})).unwrap();
    assert_eq!(out["parked"], true, "{out}");
    let card = b.get(&format!("tasks/T{two}"));
    assert_eq!(card["waits_for_state"], json!([{"ref": h, "done": false}]));
    assert!(card["waiting"].as_str().unwrap().contains(&format!("Waits for you to do {h}")), "{card}");

    assert!(runner::start_queued(&b.app).unwrap().is_empty());
    assert!(b.starts(two).is_empty());

    b.from_app(&format!("tasks/{h}/did"), json!({"note": "Tenancy acme-dev"})).unwrap();
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![two]);
    let args = board::job_args(&b.starts(two)[0]);
    assert_eq!(args.st("flags"), "--resume c-s2");
    let prompt = args.st("prompt");
    assert!(prompt.contains(&format!("{h} “Create an Oracle Cloud account” is done")), "{prompt}");
    assert!(prompt.contains("Tenancy acme-dev"), "{prompt}");
}

#[test]
fn in_a_goal_it_holds_later_tasks_but_takes_no_terminal_slot() {
    let b = new_board();
    let g = b.post("/goals", json!({"name": "Move to Oracle", "project": "webapp", "run_in_order": true, "max_terminals": 1})).unwrap();
    let gref = g["ref"].as_str().unwrap().to_string();
    b.session("s1");
    let h = b.human("s1", json!({"goal": gref}));
    let next = b.report("tb.new_task", "s1", json!({"title": "Deploy to Oracle Cloud", "detail": "Ship it.", "goal": gref})).unwrap()["created"][0]
        .as_str()
        .unwrap()
        .to_string();
    b.from_app(&format!("goals/{gref}/run"), json!({})).unwrap();

    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.row(id_of(&h)).st("needs_reason"), "human");
    assert!(b.starts(id_of(&next)).is_empty(), "it waits for the human task before it");
    let card = b.get(&format!("tasks/{next}"));
    assert!(card["waiting"].as_str().unwrap().contains(&format!("T{} to finish first", id_of(&h))) || card["waiting"].as_str().unwrap().contains(&h), "{card}");

    b.from_app(&format!("tasks/{h}/did"), json!({"note": "Tenancy acme-dev"})).unwrap();
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![id_of(&next)], "a done human task frees the goal");
    let prompt = board::job_args(&b.starts(id_of(&next))[0]).st("prompt");
    assert!(prompt.contains("Tenancy acme-dev"), "the handoff names what the owner did: {prompt}");
}

#[test]
fn waiting_on_the_owner_it_leaves_the_goals_terminal_free() {
    let b = new_board();
    let g = b.post("/goals", json!({"name": "Move to Oracle", "project": "webapp", "run_in_order": false, "max_terminals": 1})).unwrap();
    let gref = g["ref"].as_str().unwrap().to_string();
    b.session("s1");
    let h = b.human("s1", json!({"goal": gref}));
    let other = b.report("tb.new_task", "s1", json!({"title": "Write the deploy script", "detail": "Ship it.", "goal": gref})).unwrap()["created"][0]
        .as_str()
        .unwrap()
        .to_string();
    b.from_app(&format!("goals/{gref}/run"), json!({})).unwrap();

    let started = runner::start_queued(&b.app).unwrap();
    assert_eq!(b.row(id_of(&h)).st("needs_reason"), "human");
    if !started.contains(&id_of(&other)) {
        assert_eq!(runner::start_queued(&b.app).unwrap(), vec![id_of(&other)], "the human task holds no slot");
    }
    assert_eq!(b.starts(id_of(&other)).len(), 1);
}

#[test]
fn it_needs_its_steps() {
    let b = new_board();
    b.session("s1");
    let (status, msg) = b.report("tb.new_task", "s1", json!({"title": "Sign the contract", "project": "webapp", "kind": "human"})).unwrap_err();
    assert_eq!(status, 400);
    assert!(msg.contains("steps"), "{msg}");
}

#[test]
fn talking_it_over_opens_claude_in_its_checkout_with_its_steps() {
    let b = new_board();
    b.session("s1");
    let h = b.human("s1", json!({}));
    let id = id_of(&h);
    let waiter = b.task("Deploy to Oracle Cloud");
    b.post(&format!("/tasks/T{waiter}"), json!({"waits_for": [id]})).unwrap();

    b.app.shared.lock().midna_seen_at = None;
    let (code, msg) = b.post(&format!("/tasks/{h}/talk"), json!({})).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("Midna isn't running"), "{msg}");
    b.app.shared.lock().midna_seen_at = Some(taskboardd::util::now_iso());

    let out = b.post(&format!("/tasks/{h}/talk"), json!({})).unwrap();
    let jid: i64 = out["job"].as_str().unwrap()[1..].parse().unwrap();
    let j = b.app.db.q1("SELECT purpose, task_id, args FROM jobs WHERE id = ?", p![jid]).unwrap().unwrap();
    assert_eq!(j.st("purpose"), "human_talk");
    assert!(j.i("task_id").is_none(), "the talk isn't the task's terminal");
    let args: Value = serde_json::from_str(&j.st("args")).unwrap();
    assert_eq!(args["cwd"], json!(b.dir.path().join("webapp")));
    let ctx = args["context"].as_str().unwrap();
    for want in [h.as_str(), "Create an Oracle Cloud account", "Pick the home region.", "Deploy to Oracle Cloud", "presses Done in the app"] {
        assert!(ctx.contains(want), "{want} in {ctx}");
    }

    // Not for an agent's task, nor once it's done.
    let (code, _) = b.post(&format!("/tasks/T{waiter}/talk"), json!({})).unwrap_err();
    assert_eq!(code, 409);
    b.from_app(&format!("/tasks/{h}/did"), json!({})).unwrap();
    let (code, msg) = b.post(&format!("/tasks/{h}/talk"), json!({})).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("done already"), "{msg}");
}
