//! A `blocks: pr` item holds its group's PR until it's fixed: the first becomes a pre-merge fix task that
//! shares the PR, later ones join it until it starts, past `[backlog] critical_max` they go to the owner,
//! and after the merge they block the goal instead.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{jloads_obj, now_iso, PrLink, Row, RowExt};
use taskboardd::{board, dispatch, midna, prcmds, prflow, reports, samepr};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
    goal: i64,
}

fn new_board_with(tweak: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    tweak(&mut cfg);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    let goal = api::dispatch(&app, "POST", "/goals", &Query::new(), &json!({"name": "Login", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    Board { app, dir, goal }
}

fn new_board() -> Board {
    new_board_with(|_| {})
}

fn approved(num: i64, branch: &str) -> Value {
    json!({"state": "OPEN", "head": format!("{num:0>40}"), "branch": branch, "base": "main", "checks": [{"name": "build", "state": "passed"}],
           "failed": [], "running": 0, "comments": 0, "approvals": 1, "review_decision": "APPROVED", "requested": 1})
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn task(&self, title: &str, extra: Value) -> i64 {
        let mut body = json!({"title": title, "detail": "Do it.", "project": "webapp", "goal_id": self.goal});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", body)["id"].as_i64().unwrap()
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn rec(&self, id: i64) -> Value {
        jloads_obj(self.row(id).s("pr_flow")).get("rec").cloned().unwrap()
    }
    /// Gives task `id` an open, green, approved PR and steps it, so it's ready to merge.
    fn link(&self, id: i64, num: i64, branch: &str) {
        let pr = PrLink { host: "github".into(), repo: "acme/webapp".into(), num, url: format!("https://github.com/acme/webapp/pull/{num}") };
        let t = self.row(id);
        self.app.db.tx(|| prflow::link_pr(&self.app, &t, &pr, "test")).unwrap();
        prflow::merge_flow(&self.app, id, vec![("rec", approved(num, branch))]).unwrap();
        board::update_task(&self.app, id, vec![("status", json!("done"))]).unwrap();
        prflow::step(&self.app, &self.row(id), &approved(num, branch)).unwrap();
    }
    /// Puts task `id` to work in terminal s1, the terminal `found` files from.
    fn work(&self, id: i64) {
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": self.dir.path().join("webapp").to_string_lossy(), "status": {"state": "working"}})], &[])
            .unwrap();
        self.app.db.x("UPDATE tasks SET session_id = NULL WHERE session_id = 's1'", taskboardd::p![]).unwrap();
        board::update_task(&self.app, id, vec![("status", json!("working")), ("session_id", json!("s1"))]).unwrap();
    }
    fn found(&self, title: &str, why: &str) -> Value {
        let body = json!({"event": "tb.found", "session": "s1", "title": title, "kind": "bug", "blocks": "pr", "blocks_why": why});
        reports::handle(&self.app, body, false).unwrap_or_else(|e| panic!("found: {}", e.message))
    }
    fn issue(&self, v: &Value) -> Row {
        let id: i64 = v["issue"].as_str().unwrap()[1..].parse().unwrap();
        board::get_issue(&self.app, id).unwrap()
    }
    fn alerts(&self) -> Vec<String> {
        dispatch::alerts(&self.app).iter().filter_map(|a| a["text"].as_str().map(str::to_string)).collect()
    }
    fn blockers(&self, id: i64) -> Vec<String> {
        prcmds::merge_blockers(&self.app, &self.row(id), &self.rec(id), false).unwrap()
    }
    fn finish(&self, id: i64) {
        board::update_task(&self.app, id, vec![("status", json!("done"))]).unwrap();
    }
}

#[test]
fn the_first_pr_item_makes_a_fix_task_that_shares_the_pr() {
    let b = new_board();
    let a = b.task("Sign in", json!({"wave": 1}));
    b.link(a, 12, "feat/sign-in");
    assert_eq!(b.row(a).st("pr_phase"), "merge");
    b.work(a);

    let v = b.found("Tokens leak to the log", "src/auth.rs logs the bearer token");
    let i = b.issue(&v);
    let fix = i.i("task_id").expect("made into a task");
    assert_eq!((i.st("state").as_str(), i.i("holds_pr")), ("task", Some(a)));
    let held = v["held"].as_str().unwrap();
    assert!(held.contains(&format!("holds T{a}'s PR until T{fix}")), "{held}");

    let f = b.row(fix);
    assert_eq!(f.st("title"), format!("Pre-merge fixes for T{a}"));
    assert_eq!((samepr::anchor_id(&f), f.i("goal_id"), f.i("wave"), f.i("from_issue_id")), (Some(a), Some(b.goal), Some(2), Some(i.id())));
    assert!(f.st("detail").contains(&format!("- B{} Tokens leak to the log: src/auth.rs logs the bearer token", i.id())), "{}", f.st("detail"));
    let hold = samepr::hold(&b.app, &b.row(a)).unwrap().unwrap();
    assert_eq!(hold, format!("B{} blocks this PR: src/auth.rs logs the bearer token", i.id()));

    b.finish(fix);
    assert!(samepr::hold(&b.app, &b.row(a)).unwrap().is_none(), "the fix task is done, so nothing holds it");
}

#[test]
fn later_items_join_the_fix_task_until_it_starts_then_make_another() {
    let b = new_board();
    let a = b.task("Sign in", json!({"wave": 1}));
    b.link(a, 12, "feat/sign-in");
    b.work(a);

    let first = b.issue(&b.found("Tokens leak to the log", "src/auth.rs logs the bearer token"));
    let fix = first.i("task_id").unwrap();
    let v = b.found("Logout wipes saved drafts", "drafts.rs deletes every draft");
    let second = b.issue(&v);
    assert_eq!((second.st("state").as_str(), second.i("task_id"), second.i("holds_pr")), ("task", Some(fix), Some(a)));
    assert!(v["held"].as_str().unwrap().contains(&format!("was added to T{fix}")), "{v}");
    let detail = b.row(fix).st("detail");
    assert!(detail.contains(&format!("- B{} Tokens leak", first.id())) && detail.contains(&format!("- B{} Logout wipes saved drafts: drafts.rs deletes every draft", second.id())), "{detail}");
    let fixes = b.app.db.count("SELECT COUNT(*) FROM tasks WHERE same_pr = ?", taskboardd::p![a]).unwrap();
    assert_eq!(fixes, 1, "one fix task for both");

    board::update_task(&b.app, fix, vec![("status", json!("working")), ("started_at", json!(now_iso()))]).unwrap();
    let third = b.issue(&b.found("Reset link never expires", "reset.rs skips the expiry check"));
    let fix2 = third.i("task_id").unwrap();
    assert_ne!(fix2, fix, "the first one started, so a new member takes it");
    let f2 = b.row(fix2);
    assert_eq!((samepr::anchor_id(&f2), f2.i("wave")), (Some(a), Some(3)), "one wave after the group's last task");
    assert!(!b.row(fix).st("detail").contains("Reset link"));

    b.finish(fix);
    let hold = samepr::hold(&b.app, &b.row(a)).unwrap().unwrap();
    assert!(hold.starts_with(&format!("B{} blocks this PR", third.id())), "{hold}");
}

#[test]
fn a_member_s_item_holds_the_anchor_s_pr() {
    let b = new_board();
    let a = b.task("Sign in", json!({"wave": 1}));
    let m = b.task("Remember me", json!({"wave": 2, "same_pr": format!("T{a}")}));
    b.link(a, 12, "feat/sign-in");
    b.work(m);

    let i = b.issue(&b.found("Cookie never expires", "remember.rs sets no max-age"));
    assert_eq!(i.i("holds_pr"), Some(a));
    let f = b.row(i.i("task_id").unwrap());
    assert_eq!((samepr::anchor_id(&f), f.i("wave")), (Some(a), Some(3)), "it comes after the member, the group's last task");
    assert!(samepr::hold(&b.app, &b.row(a)).unwrap().unwrap().starts_with(&format!("B{}", i.id())));
}

#[test]
fn past_the_cap_an_item_goes_to_the_owner_and_still_holds() {
    let b = new_board_with(|c| c.backlog.critical_max = 1);
    let a = b.task("Sign in", json!({"wave": 1}));
    b.link(a, 12, "feat/sign-in");
    b.work(a);

    let first = b.issue(&b.found("Tokens leak to the log", "src/auth.rs logs the bearer token"));
    assert_eq!(first.st("state"), "task");
    let v = b.found("Logout wipes saved drafts", "drafts.rs deletes every draft");
    let second = b.issue(&v);
    assert_eq!((second.st("state").as_str(), second.i("task_id"), second.i("holds_pr")), ("open", None, Some(a)));
    assert!(v["held"].as_str().unwrap().contains("past the 1 pre-merge fixes"), "{v}");
    let alert = format!("B{} holds T{a}'s PR", second.id());
    assert!(b.alerts().iter().any(|t| t.starts_with(&alert)), "{:?}", b.alerts());

    b.finish(first.i("task_id").unwrap());
    let hold = samepr::hold(&b.app, &b.row(a)).unwrap().unwrap();
    assert!(hold.starts_with(&format!("B{} blocks this PR", second.id())), "the capped item holds on its own: {hold}");

    b.post(&format!("/backlog/{}/drop", second.id()), json!({"reason": "not a real crash"}));
    assert!(!b.alerts().iter().any(|t| t.starts_with(&alert)), "{:?}", b.alerts());
    assert!(samepr::hold(&b.app, &b.row(a)).unwrap().is_none());
}

#[test]
fn the_cap_holds_before_any_fix_task_exists() {
    let b = new_board_with(|c| c.backlog.critical_max = 0);
    let a = b.task("Sign in", json!({"wave": 1}));
    b.link(a, 12, "feat/sign-in");
    b.work(a);

    let i = b.issue(&b.found("Tokens leak to the log", "src/auth.rs logs the bearer token"));
    assert_eq!((i.st("state").as_str(), i.i("task_id")), ("open", None));
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM tasks WHERE same_pr = ?", taskboardd::p![a]).unwrap(), 0);
    assert_eq!(b.row(a).st("pr_phase"), "stack", "it holds with no task made yet");
    assert!(b.blockers(a).iter().any(|w| w.contains(&format!("B{} blocks this PR", i.id()))), "{:?}", b.blockers(a));
}

#[test]
fn after_the_merge_it_blocks_the_goal_as_an_ordinary_task() {
    let b = new_board();
    let a = b.task("Sign in", json!({"wave": 1}));
    b.link(a, 12, "feat/sign-in");
    board::update_task(&b.app, a, vec![("pr_state", json!("MERGED")), ("pr_phase", json!("merged"))]).unwrap();
    b.work(a);

    let v = b.found("Tokens leak to the log", "src/auth.rs logs the bearer token");
    let i = b.issue(&v);
    assert_eq!((i.st("blocks").as_str(), i.i("misses_goal"), i.i("holds_pr"), i.i("goal_id"), i.st("state").as_str()), ("goal", Some(1), None, Some(b.goal), "task"));
    let t = b.row(i.i("task_id").unwrap());
    assert_eq!((samepr::anchor_id(&t), t.i("goal_id")), (None, Some(b.goal)), "an ordinary task in the goal");
    assert!(v["held"].as_str().unwrap().contains("already merged, so it blocks the goal instead"), "{v}");

    // Outside a goal there's nothing for it to block.
    let solo = b.post("/tasks", json!({"title": "Profile", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
    b.link(solo, 13, "feat/profile");
    board::update_task(&b.app, solo, vec![("pr_state", json!("CLOSED")), ("pr_phase", json!("declined"))]).unwrap();
    b.work(solo);
    let i = b.issue(&b.found("Avatar upload crashes", "avatar.rs unwraps the decode"));
    assert_eq!((i.st("blocks").as_str(), i.i("holds_pr"), i.st("state").as_str()), ("none", None, "open"));
}

#[test]
fn an_item_filed_while_the_pr_waits_to_merge_holds_it_at_once() {
    // The race: a thread is resolved and the PR reaches merge just before the item that comes out of it
    // is filed. Filing steps the PR again, so it leaves merge before anything wakes to merge it.
    let b = new_board_with(|c| c.backlog.critical_max = 0);
    let a = b.task("Sign in", json!({"wave": 1}));
    b.link(a, 12, "feat/sign-in");
    assert_eq!(b.row(a).st("pr_phase"), "merge");
    assert!(b.blockers(a).is_empty(), "{:?}", b.blockers(a));
    b.work(a);

    let i = b.issue(&b.found("Tokens leak to the log", "src/auth.rs logs the bearer token"));
    assert_eq!(b.row(a).st("pr_phase"), "stack");
    let why = b.blockers(a);
    assert!(why.iter().any(|w| w == &format!("B{} blocks this PR: src/auth.rs logs the bearer token", i.id())), "{why:?}");

    // Settling it lets the PR go back to merge on its own.
    b.post(&format!("/backlog/{}/drop", i.id()), json!({"reason": "already fixed"}));
    assert_eq!(b.row(a).st("pr_phase"), "merge");
    assert!(b.blockers(a).is_empty(), "{:?}", b.blockers(a));
}

#[test]
fn a_fix_task_at_merge_holds_until_it_is_done() {
    let b = new_board();
    let a = b.task("Sign in", json!({"wave": 1}));
    b.link(a, 12, "feat/sign-in");
    b.work(a);

    let i = b.issue(&b.found("Tokens leak to the log", "src/auth.rs logs the bearer token"));
    assert_eq!(b.row(a).st("pr_phase"), "stack", "made into a fix task, it holds the merge");
    b.finish(i.i("task_id").unwrap());
    assert_eq!(prflow::phase_of(&b.app, &b.row(a), &b.rec(a)), "merge");
}
