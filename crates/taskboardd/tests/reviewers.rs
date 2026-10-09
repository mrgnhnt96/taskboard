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

/// One poll of the PRs, then the review sweep (which runs on its own timer, `runner::reviews`).
pub fn poll(b: &Board) {
    prflow::refresh(&b.app).unwrap();
    taskboardd::runner::reviews(&b.app).unwrap();
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

pub fn sh_git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").arg("-C").arg(repo).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repo where Ana and Bo wrote a.rs, Bo and Cy wrote b.rs, Dee has too few commits, and the
/// owner (me@acme.com) has plenty; `feat` changes a.rs. (base sha, head sha).
pub fn history(b: &Board) -> (String, String) {
    let repo = b.repo();
    sh_git(&repo, &["init", "-q", "-b", "main"]);
    sh_git(&repo, &["config", "user.email", "me@acme.com"]);
    sh_git(&repo, &["config", "user.name", "Me"]);
    let mut n = 0;
    let mut commit = |file: &str, who: &str, email: &str| {
        n += 1;
        std::fs::write(repo.join(file), format!("{n}\n")).unwrap();
        sh_git(&repo, &["add", "."]);
        sh_git(&repo, &["commit", "-q", "-m", &format!("c{n}"), "--author", &format!("{who} <{email}>")]);
    };
    for _ in 0..6 {
        commit("a.rs", "Ana Lima", "ana@acme.com");
    }
    commit("a.rs", "Bo Park", "bo@acme.com");
    for _ in 0..4 {
        commit("b.rs", "Bo Park", "bo@acme.com");
    }
    for _ in 0..5 {
        commit("b.rs", "Cy Ng", "cy@acme.com");
    }
    for _ in 0..2 {
        commit("c.rs", "Dee", "dee@acme.com");
    }
    for _ in 0..5 {
        commit("c.rs", "Me", "me@acme.com");
    }
    let base = sh_git(&repo, &["rev-parse", "HEAD"]);
    sh_git(&repo, &["checkout", "-q", "-b", "feat"]);
    commit("a.rs", "Me", "me@acme.com");
    let head = sh_git(&repo, &["rev-parse", "HEAD"]);
    sh_git(&repo, &["checkout", "-q", "main"]);
    (base, head)
}

pub fn members(h: &FakeHost) {
    *h.members.lock() = ["Ana Lima", "Bo Park", "Cy Ng", "Dee", "Me"]
        .iter()
        .map(|n| Reviewer { user: format!("{{{}}}", n.split(' ').next().unwrap().to_lowercase()), name: n.to_string(), ..Default::default() })
        .collect();
}

pub fn rec_on(base: &str, head: &str) -> Record {
    Record { base_head: base.into(), head: head.into(), author: "{me}".into(), ..green() }
}

fn picks(v: &Value) -> Vec<(String, String)> {
    v["picks"].as_array().unwrap().iter().map(|p| (p["name"].as_str().unwrap().to_string(), p["why"].as_str().unwrap().to_string())).collect()
}

fn pair(a: &str, b: &str) -> (String, String) {
    (a.to_string(), b.to_string())
}

#[test]
fn the_picker_asks_a_main_contributor_and_rotates_the_rest() {
    let b = board_with(|_| {});
    let (base, head) = history(&b);
    let h = fake(&b, rec_on(&base, &head));
    members(&h);
    let id = b.pr_task(BB);

    let v = b.post(&format!("/tasks/{id}/pr/reviewers"), json!({"dry_run": true}));
    assert_eq!(picks(&v), vec![pair("Ana Lima", "main"), pair("Bo Park", "turn")]);
    // The commit history joined the roster: Dee has too few commits.
    let names: Vec<String> = b.roster().iter().map(|r| r["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(names, vec!["Ana Lima", "Bo Park", "Cy Ng", "Me"]);
    assert_eq!(b.roster()[0]["user"], "{ana}", "the host's members gave them their account");
    assert!(h.calls().iter().all(|c| !c.starts_with("request")), "a dry run asks nobody");

    b.post(&format!("/tasks/{id}/pr/reviewers"), json!({"who": "The agent"}));
    assert!(h.calls().contains(&"request {ana},{bo}".to_string()));
    assert_eq!(b.asks(id).iter().map(|a| a.st("why")).collect::<Vec<_>>(), vec!["pick", "pick"]);
    // Asked again with two on the PR, there's nobody more to ask.
    b.post(&format!("/tasks/{id}/pr/reviewers"), json!({}));
    assert_eq!(b.asks(id).len(), 2);

    // The next PR: Ana is still the main contributor; Cy's turn comes before Bo's, who has an open ask.
    *h.rec.lock() = rec_on(&base, &head);
    let id2 = b.pr_task("https://bitbucket.org/acme/webapp/pull-requests/10");
    let v = b.post(&format!("/tasks/{id2}/pr/reviewers"), json!({"dry_run": true}));
    assert_eq!(picks(&v), vec![pair("Ana Lima", "main"), pair("Cy Ng", "turn")]);

    // A removed reviewer is never picked; a pinned one always is.
    b.act("remove", "cy@acme.com", json!({})).unwrap();
    b.act("pin", "Bo Park", json!({})).unwrap();
    let v = b.post(&format!("/tasks/{id2}/pr/reviewers"), json!({"dry_run": true}));
    assert_eq!(picks(&v), vec![pair("Bo Park", "pinned"), pair("Ana Lima", "turn")], "Bo is a main contributor too");
}

#[test]
fn a_review_is_timed_in_work_minutes() {
    let b = board_with(|_| {});
    let h = fake(&b, green());
    let id = b.pr_task(BB);
    b.add("Ana", json!({"user": "{ana}"}));
    b.post(&format!("/tasks/{id}/pr/reviewers"), json!({"ask": ["Ana"]}));
    let asked = taskboardd::util::parse_iso(&b.asks(id)[0].st("asked_at")).unwrap();
    b.app.db.x("UPDATE review_asks SET asked_at = ?", vec![json!(taskboardd::util::iso(asked - 20.0 * 60.0))]).unwrap();
    set_state(&h, "{ana}", "approved");
    poll(&b);
    let a = &b.asks(id)[0];
    assert_eq!((a.st("state"), a.st("answer")), ("answered".to_string(), "approved".to_string()));
    let mins = a.f("work_mins").unwrap();
    assert!((19.0..=21.0).contains(&mins), "{mins}");
    assert_eq!(b.roster()[0]["median_work_mins"].as_f64().map(|m| m.round()), Some(mins.round()));
}

struct Around(Vec<(&'static str, taskboardd::presence::Tier)>);
impl taskboardd::presence::Availability for Around {
    fn check(&self, r: &serde_json::Map<String, Value>) -> Result<taskboardd::presence::Presence, String> {
        let tier = self.0.iter().find(|(n, _)| r.st("name").starts_with(n)).map(|(_, t)| *t).unwrap_or(taskboardd::presence::Tier::Quiet);
        Ok(taskboardd::presence::Presence { tier, why: "canned".into() })
    }
}

#[test]
fn availability_tiers_steer_the_picker_and_the_out_are_never_picked() {
    use taskboardd::presence::Tier;
    let b = board_with(|c| c.reviewers.availability = "slack".into());
    let (base, head) = history(&b);
    let h = fake(&b, rec_on(&base, &head));
    members(&h);
    taskboardd::presence::install(&b.app, Arc::new(Around(vec![("Ana", Tier::Out), ("Bo", Tier::Quiet), ("Cy", Tier::Online), ("Me", Tier::Online)])));
    let id = b.pr_task(BB);
    let v = b.post(&format!("/tasks/{id}/pr/reviewers"), json!({"dry_run": true}));
    // Ana is out, so the main pick is Bo (quiet, the only other main contributor); Cy is online.
    assert_eq!(picks(&v), vec![pair("Bo Park", "main"), pair("Cy Ng", "turn")]);

    taskboardd::presence::install(&b.app, Arc::new(Around(vec![("Ana", Tier::Online), ("Bo", Tier::Missing), ("Cy", Tier::Off)])));
    let id2 = b.pr_task("https://bitbucket.org/acme/webapp/pull-requests/10");
    let _ = id;
    // Cached checks hold for a while; a fresh board-wide wait isn't needed for the next ones.
    taskboardd::util::advance_clock(11.0 * 60.0);
    let v = b.post(&format!("/tasks/{id2}/pr/reviewers"), json!({"dry_run": true}));
    taskboardd::util::reset_clock();
    assert_eq!(picks(&v), vec![pair("Ana Lima", "main"), pair("Cy Ng", "turn")], "off beats nobody");
    let bo = b.roster().into_iter().find(|r| r["name"] == "Bo Park").unwrap();
    assert_eq!((bo["removed"].clone(), bo["removed_why"].clone()), (json!(true), json!("canned")), "not on Slack: off the roster");
}

/// A comment of Ana's on PR `pr` of the repo (anyone's PR), `mins_ago` minutes ago, its marker in a footer.
fn bot_comment(pr: i64, id: &str, mins_ago: f64) -> taskboardd::prhost::Comment {
    taskboardd::prhost::Comment {
        pr,
        id: id.into(),
        author: "{ana}".into(),
        author_name: "Ana".into(),
        at: taskboardd::util::iso(taskboardd::util::now_ts() - mins_ago * 60.0),
        text: "2 findings: see below.\n\nRename `x`.\n\n---\n🤖 AI Review".into(),
    }
}

#[test]
fn a_reviewer_with_a_timed_bot_is_asked_just_before_it_runs() {
    let b = board_with(|c| c.reviewers.bot_scan_mins = 0.0);
    let h = fake(&b, green());
    let id = b.pr_task(BB);
    for (n, u) in [("Ana", "{ana}"), ("Bo", "{bo}"), ("Cy", "{cy}")] {
        b.add(n, json!({"user": u}));
    }
    b.act("bot", "Ana", json!({"every_h": 4, "mark": "ai review"})).unwrap();
    let names = |v: &Value| -> Vec<String> { picks(v).into_iter().map(|(n, _)| n).collect() };
    let dry = || b.post(&format!("/tasks/{id}/pr/reviewers"), json!({"dry_run": true, "count": 3}));
    // No run seen yet: she isn't asked until one is.
    assert!(!names(&dry()).contains(&"Ana".to_string()));

    // Her bot ran an hour ago on someone else's PRs (two comments of one run, the marker in a
    // footer): the next run is 3 hours off, so she waits.
    *h.comments.lock() = vec![bot_comment(31, "c1", 60.0), bot_comment(32, "c2", 50.0)];
    poll(&b);
    assert!(h.calls().contains(&"comments acme/webapp 20".to_string()), "the repo's recent PRs: {:?}", h.calls());
    assert_eq!(b.app.db.count("SELECT COUNT(DISTINCT at) FROM reviewer_bot_runs", vec![]).unwrap(), 1, "one run");
    let at = b.app.db.val("SELECT at FROM reviewer_bot_runs", vec![]).unwrap().as_str().and_then(taskboardd::util::parse_iso).unwrap();
    assert!((taskboardd::util::now_ts() - at - 3600.0).abs() < 60.0, "the bot comment's own time");
    assert!(!names(&dry()).contains(&"Ana".to_string()));
    assert!(b.roster()[0]["bot"]["last_run"].is_string());
    assert!(b.roster()[0]["bot"]["next_run"].is_string());

    // A run 3h40m ago: the next is 20 minutes off, so she's asked, and first (the fastest pace).
    b.app.db.x("DELETE FROM reviewer_bot_runs", vec![]).unwrap();
    *h.comments.lock() = vec![bot_comment(31, "c3", 220.0)];
    poll(&b);
    let v = dry();
    assert_eq!(names(&v).len(), 3);
    assert!(names(&v).contains(&"Ana".to_string()));

    // Overdue (a run 7h50m ago, the one at 4h missed): the next is rolled forward to 10 minutes off.
    b.app.db.x("DELETE FROM reviewer_bot_runs", vec![]).unwrap();
    *h.comments.lock() = vec![bot_comment(31, "c5", 470.0)];
    poll(&b);
    assert!(names(&dry()).contains(&"Ana".to_string()));
    // Overdue by a little more than an hour: the rolled-forward run is hours off, so she waits.
    b.app.db.x("DELETE FROM reviewer_bot_runs", vec![]).unwrap();
    *h.comments.lock() = vec![bot_comment(31, "c6", 5.0 * 60.0 + 10.0)];
    poll(&b);
    assert!(!names(&dry()).contains(&"Ana".to_string()), "not asked at any time just because a run is overdue");

    // An old comment outside the window isn't a run.
    b.app.db.x("DELETE FROM reviewer_bot_runs", vec![]).unwrap();
    *h.comments.lock() = vec![bot_comment(31, "c4", 13.0 * 60.0)];
    poll(&b);
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM reviewer_bot_runs", vec![]).unwrap(), 0);
}

fn crew(b: &Board) {
    for (n, u) in [("Ana", "{ana}"), ("Bo", "{bo}"), ("Cy", "{cy}"), ("Dee", "{dee}")] {
        b.add(n, json!({"user": u}));
    }
}

/// Makes every open ask on the board look `mins` minutes old.
fn age_asks(b: &Board, mins: f64) {
    let at = taskboardd::util::iso(taskboardd::util::now_ts() - mins * 60.0);
    b.app.db.x("UPDATE review_asks SET asked_at = ? WHERE state = 'open'", vec![json!(at)]).unwrap();
}

fn states(b: &Board, id: i64) -> Vec<(String, String)> {
    b.asks(id).iter().map(|a| (a.st("name"), a.st("state"))).collect()
}

#[test]
fn a_slow_reviewer_is_swapped_and_comes_back() {
    let b = board_with(|c| c.reviewers.swap = true);
    let h = fake(&b, green());
    let id = b.pr_task(BB);
    crew(&b);
    b.post(&format!("/tasks/{id}/pr/reviewers"), json!({"ask": ["Ana"]}));
    age_asks(&b, 30.0);
    poll(&b);
    assert_eq!(states(&b, id), vec![pair("Ana", "open")], "not timed out yet");

    age_asks(&b, 100.0);
    poll(&b);
    assert_eq!(states(&b, id), vec![pair("Ana", "swapped"), pair("Bo", "open")]);
    assert!(h.calls().contains(&"remove {ana}".to_string()), "{:?}", h.calls());
    assert_eq!(b.flow(id)["swapped_off"], json!(["{ana}"]));
    assert_eq!(b.asks(id)[1].st("why"), "swap");
    let rows = &taskboardd::prbar::bar(&b.app, &b.task(id)).unwrap()["reviewer_rows"];
    let bo = rows.as_array().unwrap().iter().find(|r| r["user"] == "{bo}").unwrap();
    assert_eq!(bo["swaps"], 1, "{rows}");
    assert!(bo["asked_at"].is_string());

    // Ana approves anyway: she counts again and Bo, who hasn't looked, is taken off.
    set_state(&h, "{ana}", "approved");
    poll(&b);
    assert_eq!(states(&b, id), vec![pair("Ana", "came_back"), pair("Bo", "dropped")]);
    assert!(h.calls().contains(&"remove {bo}".to_string()));
    assert_eq!(b.flow(id)["swapped_off"], json!(["{bo}"]));
}

#[test]
fn a_swapped_off_request_for_changes_is_waived_and_someone_else_asked() {
    let b = board_with(|c| c.reviewers.swap = true);
    let h = fake(&b, green());
    let id = b.pr_task(BB);
    crew(&b);
    b.post(&format!("/tasks/{id}/pr/reviewers"), json!({"ask": ["Ana"]}));
    age_asks(&b, 100.0);
    poll(&b);
    set_state(&h, "{ana}", "changes");
    h.rec.lock().review_decision = "CHANGES_REQUESTED".into();
    poll(&b);
    assert_eq!(b.phase(id), "review", "her request doesn't hold");
    assert_eq!(states(&b, id), vec![pair("Ana", "swapped"), pair("Bo", "open"), pair("Cy", "open")]);
    assert_eq!(b.asks(id)[2].st("why"), "fill_in");
    poll(&b);
    assert_eq!(b.asks(id).len(), 3, "once");
}

#[test]
fn the_sweep_swaps_while_a_healthy_feed_drives_the_prs() {
    let b = board_with(|c| {
        c.reviewers.swap = true;
        c.feed.on = true;
    });
    let h = fake(&b, green());
    let id = b.pr_task(BB);
    crew(&b);
    b.post("/prs/heartbeat", json!({}));
    b.post(&format!("/tasks/{id}/pr/reviewers"), json!({"ask": ["Ana"]}));
    assert!(!taskboardd::feed::poll_due(&b.app), "a healthy feed: no poll");
    age_asks(&b, 100.0);
    // Only the sweep's own timer runs: no poll of the PRs.
    taskboardd::runner::reviews(&b.app).unwrap();
    assert_eq!(states(&b, id), vec![pair("Ana", "swapped"), pair("Bo", "open")]);
    assert!(h.calls().contains(&"remove {ana}".to_string()), "{:?}", h.calls());
}

#[test]
fn nobody_is_swapped_unless_swapping_is_on_and_the_hours_are_open() {
    let b = board_with(|_| {});
    let _h = fake(&b, green());
    let id = b.pr_task(BB);
    crew(&b);
    b.post(&format!("/tasks/{id}/pr/reviewers"), json!({"ask": ["Ana"]}));
    age_asks(&b, 500.0);
    poll(&b);
    assert_eq!(states(&b, id), vec![pair("Ana", "open")], "[reviewers] swap is off");

    let b = board_with(|c| c.reviewers.swap = true);
    let _h = fake(&b, green());
    let id = b.pr_task(BB);
    crew(&b);
    b.post(&format!("/tasks/{id}/pr/reviewers"), json!({"ask": ["Ana"]}));
    // Work hours that are never open now: a day of the week that isn't today.
    let tomorrow = chrono::Local::now().date_naive().succ_opt().unwrap().format("%a").to_string().to_lowercase();
    b.post("/hours", json!({"on": true, "start": "00:00", "end": "23:59", "days": tomorrow}));
    age_asks(&b, 3.0 * 24.0 * 60.0);
    poll(&b);
    assert_eq!(states(&b, id), vec![pair("Ana", "open")], "outside work hours");
}

fn pr_jobs(b: &Board, id: i64) -> Vec<String> {
    b.app.db.q("SELECT args FROM jobs WHERE task_id = ? AND purpose = 'pr' ORDER BY id", vec![json!(id)]).unwrap().iter().map(|j| j.st("args")).collect()
}

/// Work hours that aren't open now: only tomorrow.
fn closed_hours(b: &Board) {
    let tomorrow = chrono::Local::now().date_naive().succ_opt().unwrap().format("%a").to_string().to_lowercase();
    b.post("/hours", json!({"on": true, "start": "00:00", "end": "23:59", "days": tomorrow}));
}

#[test]
fn after_the_owner_s_review_the_agent_is_woken_to_ask_for_reviews() {
    let b = board_with(|c| c.reviewers.ask_stage = true);
    let h = fake(&b, green());
    let id = b.pr_task(BB);
    crew(&b);
    poll(&b);
    assert_eq!(b.phase(id), "review");
    assert!(pr_jobs(&b, id).is_empty());
    b.post(&format!("/tasks/{id}/pr/reviewed"), json!({}));
    assert_eq!(b.phase(id), "ask");
    assert_eq!(prflow::label("ask"), "Asking for reviews");
    let jobs = pr_jobs(&b, id);
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert!(jobs[0].contains("pr reviewers T"), "{}", jobs[0]);
    assert!(h.calls().iter().all(|c| !c.starts_with("request")), "the agent asks, not the board");

    b.post(&format!("/tasks/{id}/pr/reviewers"), json!({"who": "The agent"}));
    assert_eq!(b.phase(id), "review");
    assert_eq!(b.flow(id)["asked"]["names"].as_array().unwrap().len(), 2);
    assert!(b.flow(id).get("woke").is_none(), "asking finished the visit");
    let card = board::task_card(&b.app, &b.task(id)).unwrap();
    assert_eq!(card["pr"]["stage"]["asked"]["by"], "The agent");
}

#[test]
fn outside_work_hours_the_board_asks_for_reviews_itself_and_retries() {
    let b = board_with(|c| c.reviewers.ask_stage = true);
    let h = fake(&b, green());
    let id = b.pr_task(BB);
    poll(&b);
    closed_hours(&b);
    // Nobody on the roster yet: it can't ask, and tries again later.
    b.post(&format!("/tasks/{id}/pr/reviewed"), json!({}));
    assert_eq!(b.phase(id), "ask");
    assert_eq!(b.flow(id)["ask_tries"], 1);
    assert!(pr_jobs(&b, id).is_empty(), "no agent outside work hours");
    crew(&b);
    poll(&b);
    assert_eq!(b.flow(id)["ask_tries"], 1, "it waits a minute first");

    b.app.db.x("UPDATE tasks SET pr_flow = json_set(pr_flow, '$.ask_retry_at', '2000-01-01T00:00:00Z')", vec![]).unwrap();
    poll(&b);
    assert!(h.calls().contains(&"request {ana},{bo}".to_string()), "{:?}", h.calls());
    assert_eq!(b.asks(id).iter().map(|a| (a.st("why"), a.st("asked_by"))).collect::<Vec<_>>(),
               vec![pair("stage", "Task board"), pair("stage", "Task board")]);
    assert_eq!(b.phase(id), "review");
    assert_eq!(b.flow(id)["asked"]["names"], json!(["Ana", "Bo"]));
    assert!(b.flow(id).get("ask_tries").is_none());
}

#[test]
fn without_the_ask_stage_the_review_goes_straight_to_reviewers() {
    let b = board_with(|_| {});
    let _h = fake(&b, green());
    let id = b.pr_task(BB);
    poll(&b);
    b.post(&format!("/tasks/{id}/pr/reviewed"), json!({}));
    poll(&b);
    assert_eq!(b.phase(id), "review");
}

pub fn local_ts(y: i32, m: u32, d: u32, h: u32, min: u32) -> f64 {
    use chrono::TimeZone;
    chrono::Local.with_ymd_and_hms(y, m, d, h, min, 0).single().unwrap().timestamp() as f64
}

#[test]
fn work_minutes_count_only_the_work_hours() {
    let b = board_with(|_| {});
    let (a, z) = (local_ts(2026, 10, 7, 16, 0), local_ts(2026, 10, 8, 10, 0));
    assert_eq!(taskboardd::picker::work_minutes(&b.app, a, z), 18.0 * 60.0, "hours off: every minute");
    b.post("/hours", json!({"on": true, "start": "09:00", "end": "17:00", "days": "all"}));
    assert_eq!(taskboardd::picker::work_minutes(&b.app, a, z), 120.0);
}

fn ago(secs: f64) -> String {
    taskboardd::util::iso(taskboardd::util::now_ts() - secs)
}

/// The feed's state as `feed.rs` keeps it (times in seconds before now).
fn feed_state(b: &Board, st: Value) {
    b.app.db.set_setting("pr_feed", Some(&st.to_string())).unwrap();
}

#[test]
fn tb_pr_reviewers_waits_for_a_held_feed_and_its_settle_window() {
    let b = board_with(|c| c.feed.on = true);
    let _h = fake(&b, green());
    let id = b.pr_task(BB);
    crew(&b);
    // Stuck: no heartbeat for over 3 minutes.
    feed_state(&b, json!({"since": ago(4000.0), "last_heartbeat_at": ago(400.0)}));
    let e = b.try_post(&format!("/tasks/{id}/pr/reviewers"), json!({"ask": ["Ana"]})).unwrap_err();
    assert!(e.contains("Holding reviewer asks") && e.contains("heartbeat"), "{e}");
    assert!(b.try_post(&format!("/tasks/{id}/pr/reviewers"), json!({"dry_run": true})).is_ok(), "a dry run asks nobody");

    // Back, but only just: the missed events catch up first.
    taskboardd::feed::check(&b.app).unwrap();
    b.post("/prs/heartbeat", json!({}));
    taskboardd::feed::check(&b.app).unwrap();
    assert!(taskboardd::feed::feed_healthy(&b.app));
    let e = b.try_post(&format!("/tasks/{id}/pr/reviewers"), json!({"ask": ["Ana"]})).unwrap_err();
    assert!(e.contains("just came back"), "{e}");
    assert!(b.get("/prs/feed", &[])["settling_secs"].as_f64().unwrap() > 290.0);

    feed_state(&b, json!({"since": ago(4000.0), "last_heartbeat_at": ago(1.0), "healthy_at": ago(301.0)}));
    b.post(&format!("/tasks/{id}/pr/reviewers"), json!({"ask": ["Ana"]}));
    assert_eq!(states(&b, id), vec![pair("Ana", "open")]);
}

#[test]
fn a_stuck_feed_holds_even_the_first_ask_outside_work_hours() {
    let b = board_with(|c| c.feed.on = true);
    feed_state(&b, json!({"since": ago(4000.0), "last_heartbeat_at": ago(400.0)}));
    closed_hours(&b);
    assert!(taskboardd::feed::holding(&b.app, true).is_some(), "stuck is down or stale, not a quiet night");
    feed_state(&b, json!({"since": ago(9000.0)}));
    assert_eq!(taskboardd::feed::holding(&b.app, true), None, "merely quiet outside the hours: the first ask goes");
}

#[test]
fn with_the_ask_stage_reviewers_wait_for_the_owner_s_review() {
    let b = board_with(|_| {});
    let _h = fake(&b, green());
    let id = b.pr_task(BB);
    crew(&b);
    poll(&b);
    // The project's own switch, as tb project set --ask-stage on sets it.
    let v = b.post("/projects/webapp", json!({"ask_stage": true}));
    assert_eq!(v["pr_rules"]["ask_stage"], true);
    let e = b.try_post(&format!("/tasks/{id}/pr/reviewers"), json!({"ask": ["Ana"]})).unwrap_err();
    assert!(e.contains("hasn't reviewed PR #9 yet"), "{e}");
    b.post(&format!("/tasks/{id}/pr/reviewed"), json!({}));
    assert_eq!(b.phase(id), "ask", "the project's switch turns the stage on");
    b.post(&format!("/tasks/{id}/pr/reviewers"), json!({"who": "The agent"}));
    assert_eq!(b.phase(id), "review");
    let v = b.post("/projects/webapp", json!({"ask_stage": "off"}));
    assert_eq!(v["pr_rules"]["ask_stage"], false);
    assert!(b.try_post("/projects/webapp", json!({"ask_stage": "maybe"})).unwrap_err().contains("on or off"));
}

#[test]
fn the_agent_isn_t_woken_to_ask_while_the_feed_holds_and_is_once_it_s_back() {
    let b = board_with(|c| {
        c.reviewers.ask_stage = true;
        c.feed.on = true;
    });
    let _h = fake(&b, green());
    let id = b.pr_task(BB);
    crew(&b);
    poll(&b);
    feed_state(&b, json!({"since": ago(4000.0), "last_heartbeat_at": ago(400.0)}));
    b.post(&format!("/tasks/{id}/pr/reviewed"), json!({}));
    assert_eq!(b.phase(id), "ask");
    assert!(pr_jobs(&b, id).is_empty(), "held: no wake");
    taskboardd::runner::reviews(&b.app).unwrap();
    assert!(pr_jobs(&b, id).is_empty());

    feed_state(&b, json!({"since": ago(4000.0), "last_heartbeat_at": ago(1.0), "healthy_at": ago(400.0)}));
    taskboardd::runner::reviews(&b.app).unwrap();
    let jobs = pr_jobs(&b, id);
    assert_eq!(jobs.len(), 1, "the sweep brings it back once the feed has settled");
    assert!(jobs[0].contains("pr reviewers T"), "{}", jobs[0]);
}
