//! Reviewers (#4–#9): each project's roster through `tb reviewers`, `tb pr reviewers` setting a PR's
//! reviewers through its host and recording each ask, the picker, availability, bot schedules, the
//! ask ledger's swaps, and the `ask` stage after the owner's review.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, Check, FakeHost, Record, Reviewer};
use taskboardd::util::RowExt;
use taskboardd::{board, midna, prflow, reports};

pub struct Board {
    pub app: Arc<App>,
    pub dir: tempfile::TempDir,
}

pub fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    f(&mut cfg);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    pub fn post(&self, path: &str, body: Value) -> Value {
        self.try_post(path, body).unwrap_or_else(|e| panic!("POST {path}: {e}"))
    }
    pub fn try_post(&self, path: &str, body: Value) -> Result<Value, String> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| e.message)
    }
    pub fn get(&self, path: &str, query: &[(&str, &str)]) -> Value {
        let q: Query = query.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        api::dispatch(&self.app, "GET", path, &q, &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn report(&self, event: &str, session: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message))
    }
    pub fn repo(&self) -> std::path::PathBuf {
        self.dir.path().join("webapp")
    }
    pub fn pr_task(&self, url: &str) -> i64 {
        let id = self.post("/tasks", json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
        let repo = self.repo().to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", "s1", json!({"task": format!("T{id}")}));
        self.report("tb.done", "s1", json!({"summary": "Done", "pr": url}));
        id
    }
    pub fn task(&self, id: i64) -> serde_json::Map<String, Value> {
        board::get_task(&self.app, id).unwrap()
    }
    pub fn phase(&self, id: i64) -> String {
        self.task(id).st("pr_phase")
    }
    pub fn flow(&self, id: i64) -> Value {
        serde_json::from_str(&self.task(id).st("pr_flow")).unwrap()
    }
    pub fn add(&self, name: &str, extra: Value) -> Value {
        let mut b = json!({"project": "webapp", "reviewer": name});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        self.post("/reviewers", b)
    }
    pub fn act(&self, action: &str, who: &str, extra: Value) -> Result<Value, String> {
        let mut b = json!({"project": "webapp", "reviewer": who});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        self.try_post(&format!("/reviewers/{action}"), b)
    }
    pub fn roster(&self) -> Vec<Value> {
        self.get("/reviewers", &[("project", "webapp")])["projects"][0]["reviewers"].as_array().cloned().unwrap_or_default()
    }
    pub fn asks(&self, id: i64) -> Vec<serde_json::Map<String, Value>> {
        self.app.db.q("SELECT * FROM review_asks WHERE task_id = ? ORDER BY id", vec![json!(id)]).unwrap()
    }
}

pub const BB: &str = "https://bitbucket.org/acme/webapp/pull-requests/9";

pub fn green() -> Record {
    Record {
        state: "OPEN".into(),
        title: "Add x".into(),
        author: "me".into(),
        head: "h1".into(),
        branch: "feat".into(),
        base: "main".into(),
        base_head: "b1".into(),
        checks: vec![Check { name: "build".into(), state: "passed".into(), url: None }],
        ..Default::default()
    }
}

pub fn fake(b: &Board, rec: Record) -> Arc<FakeHost> {
    let h = FakeHost::new("bitbucket", rec);
    prhost::install(&b.app, h.clone());
    h
}

pub fn poll(b: &Board) {
    prflow::refresh(&b.app).unwrap();
}

pub fn set_state(h: &FakeHost, user: &str, state: &str) {
    let mut r = h.rec.lock();
    match r.reviewers.iter_mut().find(|x| x.user == user) {
        Some(x) => x.state = state.into(),
        None => r.reviewers.push(Reviewer { user: user.into(), name: user.into(), state: state.into(), requested: false }),
    }
}

#[test]
fn the_roster_folds_people_and_keeps_who_is_never_asked() {
    let b = board_with(|_| {});
    b.add("Ana Lima", json!({"user": "ana", "emails": ["ana@acme.com"]}));
    // The same person by another email and spelling folds into one reviewer.
    let r = b.add("ana lima", json!({"emails": ["ana.l@acme.com"], "aliases": ["Ana L"]}));
    assert_eq!(r["name"], "Ana Lima");
    assert_eq!(r["emails"], json!(["ana@acme.com", "ana.l@acme.com"]));
    assert_eq!(r["aliases"], json!(["Ana L"]));
    assert_eq!(b.roster().len(), 1);

    b.add("Bo", json!({"emails": ["bo@acme.com"]}));
    b.add("Robert", json!({"user": "bob-gh"}));
    let m = b.act("merge", "Robert", json!({"other": "bo@acme.com"})).unwrap();
    assert_eq!(m["user"], "bob-gh");
    assert_eq!(m["emails"], json!(["bo@acme.com"]));
    assert!(m["aliases"].as_array().unwrap().contains(&json!("Bo")));
    assert_eq!(b.roster().len(), 2);

    let e = b.act("alias", "Robert", json!({"aliases": ["ana@acme.com"]})).unwrap_err();
    assert!(e.contains("already names Ana Lima"), "{e}");
    b.act("alias", "Robert", json!({"aliases": ["rob"]})).unwrap();
    assert_eq!(b.act("pin", "rob", json!({})).unwrap()["pinned"], true);
    assert_eq!(b.act("pin", "rob", json!({"on": false})).unwrap()["pinned"], false);
    assert_eq!(b.act("auto", "ana", json!({"level": "high"})).unwrap()["automation"], 2.0);
    assert!(b.act("auto", "ana", json!({"level": "lots"})).is_err());
    let bot = b.act("bot", "ana", json!({"every_h": 4, "mark": "AI review"})).unwrap();
    assert_eq!(bot["bot"]["every_h"], 4.0);
    assert!(b.act("bot", "ana", json!({"every_h": 4})).is_err(), "a bot needs its marker");
    assert!(b.act("bot", "ana", json!({"off": true})).unwrap()["bot"].is_null());

    let gone = b.act("remove", "Ana L", json!({"reason": "left the team"})).unwrap();
    assert_eq!((gone["removed"].clone(), gone["removed_why"].clone()), (json!(true), json!("left the team")));
    assert_eq!(b.act("back", "ana", json!({})).unwrap()["removed"], false);
    assert!(b.act("remove", "nobody", json!({})).unwrap_err().contains("no reviewer"));
}

#[test]
fn tb_pr_reviewers_sets_reviewers_through_the_host_and_records_each_ask() {
    let b = board_with(|_| {});
    let h = fake(&b, green());
    let id = b.pr_task(BB);
    b.add("Ana", json!({"user": "{ana}"}));
    b.add("Bo", json!({"user": "{bo}"}));
    b.add("Cy", json!({"user": "{cy}"}));
    b.add("Me", json!({"user": "me"}));

    let v = b.post(&format!("/tasks/{id}/pr/reviewers"), json!({"ask": ["Ana", "bo"], "who": "The agent"}));
    assert_eq!(v["asked"].as_array().unwrap().len(), 2);
    assert!(h.calls().contains(&"request {ana},{bo}".to_string()));
    let asks = b.asks(id);
    assert_eq!(asks.iter().map(|a| (a.st("name"), a.st("state"), a.st("why"))).collect::<Vec<_>>(),
               vec![("Ana".into(), "open".into(), "ask".into()), ("Bo".into(), "open".into(), "ask".into())]);
    assert_eq!(b.flow(id)["asked"]["names"], json!(["Ana", "Bo"]));

    // Never the PR's author, never someone removed.
    assert!(b.try_post(&format!("/tasks/{id}/pr/reviewers"), json!({"ask": ["Me"]})).unwrap_err().contains("wrote this PR"));
    b.act("remove", "Cy", json!({})).unwrap();
    assert!(b.try_post(&format!("/tasks/{id}/pr/reviewers"), json!({"ask": ["Cy"]})).unwrap_err().contains("never asks them"));
    b.act("back", "Cy", json!({})).unwrap();

    b.post(&format!("/tasks/{id}/pr/reviewers"), json!({"replace": "Bo", "with": "Cy"}));
    assert!(h.calls().contains(&"remove {bo}".to_string()));
    let asks = b.asks(id);
    assert_eq!(asks[1].st("state"), "swapped");
    assert_eq!((asks[2].st("name"), asks[2].st("why"), asks[2].i("replaces")), ("Cy".into(), "replace".into(), Some(asks[1].id())));
    assert_eq!(b.flow(id)["swapped_off"], json!(["{bo}"]));

    b.post(&format!("/tasks/{id}/pr/reviewers"), json!({"drop": "Ana"}));
    assert_eq!(b.asks(id)[0].st("state"), "dropped");
    let names: Vec<String> = h.rec.lock().reviewers.iter().map(|r| r.user.clone()).collect();
    assert_eq!(names, vec!["{cy}".to_string()]);
}
