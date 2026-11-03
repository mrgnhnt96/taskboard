//! Tasks sharing one branch and PR (`tb task new|set --same-pr T<n>`, issue #880): the order they run
//! in, the refusals, the waves, the PR state members follow, the anchor's hold, the handoff and the plan
//! check.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{PrLink, Row, RowExt};
use taskboardd::{board, midna, prflow, reports, samepr, stack, waitsfor};

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
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn post_err(&self, path: &str, body: Value) -> (u16, String) {
        let e = api::dispatch(&self.app, "POST", path, &Query::new(), &body).expect_err("should fail");
        (e.status, e.message)
    }
    fn task(&self, title: &str, extra: Value) -> i64 {
        let mut body = json!({"title": title, "detail": "Do it.", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", body)["id"].as_i64().unwrap()
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn goal(&self) -> i64 {
        self.post("/goals", json!({"name": "Checkout", "project": "webapp"}))["id"].as_i64().unwrap()
    }
    fn propose(&self, g: i64, tasks: Value) -> Vec<String> {
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": self.dir.path().join("webapp").to_string_lossy(), "status": {"state": "working"}})], &[])
            .unwrap();
        let b = json!({"event": "tb.propose", "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}, "goal": format!("G{g}"), "tasks": tasks});
        let r = reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("propose: {}", e.message));
        r["warnings"].as_array().cloned().unwrap_or_default().iter().map(|w| w.as_str().unwrap().to_string()).collect()
    }
    fn link(&self, id: i64, num: i64, branch: &str) {
        let pr = PrLink { host: "github".into(), repo: "acme/webapp".into(), num, url: format!("https://github.com/acme/webapp/pull/{num}") };
        let t = self.row(id);
        self.app.db.tx(|| prflow::link_pr(&self.app, &t, &pr, "test")).unwrap();
        let rec = json!({"state": "OPEN", "head": format!("{num:0>40}"), "branch": branch, "base": "main", "checks": [], "failed": [], "running": 0});
        prflow::merge_flow(&self.app, id, vec![("rec", rec)]).unwrap();
        board::update_task(&self.app, id, vec![("status", json!("done"))]).unwrap();
    }
    fn close(&self, id: i64, state: &str, phase: &str) {
        board::update_task(&self.app, id, vec![("pr_state", json!(state)), ("pr_phase", json!(phase))]).unwrap();
    }
}

#[test]
fn members_queue_behind_the_group_s_last_task_and_none_takes_one_out() {
    let b = new_board();
    let a = b.task("Add the endpoint", json!({}));
    let m1 = b.task("Use it", json!({"same_pr": format!("T{a}")}));
    let m2 = b.task("Test it", json!({"same_pr": format!("T{m1}")}));
    assert_eq!(samepr::anchor_id(&b.row(m1)), Some(a));
    assert_eq!(samepr::anchor_id(&b.row(m2)), Some(a), "a member named stands for its anchor");
    assert_eq!(waitsfor::ids(&b.row(m1)), vec![a]);
    assert_eq!(waitsfor::ids(&b.row(m2)), vec![m1], "it comes after the group's last task, not the anchor");
    assert_eq!(samepr::shared_ids(&b.app, &b.row(m2)).unwrap(), vec![a, m1]);

    b.post(&format!("/tasks/{m2}"), json!({"same_pr": "none"}));
    let row = b.row(m2);
    assert_eq!((samepr::anchor_id(&row), waitsfor::ids(&row)), (None, vec![]), "none drops the group's wait too");
}

#[test]
fn sharing_a_pr_is_refused_where_it_can_t_work() {
    let b = new_board();
    let a = b.task("Add the endpoint", json!({}));
    let (code, msg) = b.post_err(&format!("/tasks/{a}"), json!({"same_pr": format!("T{a}")}));
    assert_eq!((code, msg.as_str()), (400, "A task can't share its own PR."));

    let s = b.task("Stacked", json!({"stack_on": format!("T{a}")}));
    let (code, msg) = b.post_err(&format!("/tasks/{s}"), json!({"same_pr": format!("T{a}")}));
    assert_eq!(code, 409);
    assert!(msg.contains("shares a PR or stacks on one, not both"), "{msg}");

    let quiet = b.task("No PR", json!({"ships_pr": "no"}));
    let (code, msg) = b.post_err(&format!("/tasks/{quiet}"), json!({"same_pr": format!("T{a}")}));
    assert_eq!((code, msg), (409, format!("T{quiet} doesn't end in a PR, so there's none to share.")));

    let m = b.task("Use it", json!({"same_pr": format!("T{a}")}));
    let (code, msg) = b.post_err(&format!("/tasks/{a}"), json!({"same_pr": format!("T{quiet}")}));
    assert_eq!(code, 409);
    assert!(msg.starts_with(&format!("Other tasks share T{a}'s PR")), "{msg}");

    b.link(a, 41, "feat/endpoint");
    b.close(a, "MERGED", "merged");
    let late = b.task("Late", json!({}));
    let (code, msg) = b.post_err(&format!("/tasks/{late}"), json!({"same_pr": format!("T{m}")}));
    assert_eq!((code, msg), (409, format!("T{a}'s PR is closed, so there's nothing left to add to.")));
}

#[test]
fn group_tasks_in_a_goal_never_share_a_wave() {
    let b = new_board();
    let g = b.goal();
    let a = b.task("Add the endpoint", json!({"goal_id": g, "wave": 1}));
    let (code, msg) = b.post_err("/tasks", json!({"title": "Use it", "detail": "x", "project": "webapp", "goal_id": g, "wave": 1, "same_pr": format!("T{a}")}));
    assert_eq!(code, 409);
    assert!(msg.contains("has to be in an earlier wave"), "{msg}");

    let m = b.task("Use it", json!({"goal_id": g, "wave": 2, "same_pr": format!("T{a}")}));
    let (code, msg) = b.post_err(&format!("/tasks/{m}"), json!({"wave": 1}));
    assert_eq!(code, 409);
    assert!(msg.starts_with(&format!("T{m}: T{a} has to be in an earlier wave")), "{msg}");
    assert_eq!(b.row(m).i("wave"), Some(2), "the refused move changed nothing");
}

#[test]
fn members_that_linked_the_pr_follow_the_anchor_s_state() {
    let b = new_board();
    let a = b.task("Add the endpoint", json!({}));
    let linked = b.task("Use it", json!({"same_pr": format!("T{a}")}));
    let waiting = b.task("Test it", json!({"same_pr": format!("T{a}")}));
    b.link(a, 42, "feat/endpoint");
    let url = b.row(a).st("pr_url");
    board::update_task(&b.app, linked, vec![("pr_url", json!(url)), ("pr_repo", json!("acme/webapp")), ("pr_host", json!("github"))]).unwrap();

    b.close(a, "MERGED", "merged");
    let row = b.row(linked);
    assert!(stack::merged(&row), "{row:?}");
    assert_eq!(row.i("pr_num"), None, "a member never owns the PR, so the PR flow follows the anchor alone");
    assert_eq!(b.row(waiting).s("pr_phase"), None, "one that hasn't added its work yet doesn't count as merged");

    b.close(a, "CLOSED", "declined");
    assert!(waitsfor::pr_declined(&b.row(linked)), "it goes back with a declined PR");
}

#[test]
fn the_anchor_s_pr_holds_until_every_member_is_done() {
    let b = new_board();
    let a = b.task("Add the endpoint", json!({}));
    let m = b.task("Use it", json!({"same_pr": format!("T{a}")}));
    assert_eq!(stack::stack_hold(&b.app, &b.row(a)).unwrap(), Some(format!("T{m} shares this PR and hasn't added its work yet")));
    assert_eq!(stack::stack_hold(&b.app, &b.row(m)).unwrap(), None);
    board::update_task(&b.app, m, vec![("status", json!("done"))]).unwrap();
    assert_eq!(stack::stack_hold(&b.app, &b.row(a)).unwrap(), None);
}

#[test]
fn the_handoff_says_where_a_member_s_work_goes() {
    let b = new_board();
    let a = b.task("Add the endpoint", json!({}));
    let m = b.task("Use it", json!({"same_pr": format!("T{a}")}));
    let line = samepr::handoff_line(&b.app, &b.row(a), "origin").unwrap().unwrap();
    assert!(line.starts_with(&format!("T{m} shares this task's branch and PR and runs after it")), "{line}");

    let line = samepr::handoff_line(&b.app, &b.row(m), "origin").unwrap().unwrap();
    assert!(line.contains("once its branch is known"), "{line}");

    b.link(a, 43, "feat/endpoint");
    let line = samepr::handoff_line(&b.app, &b.row(m), "origin").unwrap().unwrap();
    assert!(line.starts_with(&format!("This task shares T{a}'s branch and PR (feat/endpoint, PR #43)")), "{line}");
    assert!(line.contains(&format!("git switch -c feat/endpoint-t{m} origin/feat/endpoint")), "{line}");
    assert!(line.contains("origin HEAD:feat/endpoint"), "{line}");
    assert_eq!(samepr::start_branch(&b.app, &b.row(m)).unwrap().as_deref(), Some("feat/endpoint"));

    b.close(a, "MERGED", "merged");
    let line = samepr::handoff_line(&b.app, &b.row(m), "origin").unwrap().unwrap();
    assert!(line.contains("merged now: build on the main branch"), "{line}");
    assert_eq!(samepr::start_branch(&b.app, &b.row(m)).unwrap(), None);
}

#[test]
fn a_member_s_review_starts_where_the_task_before_it_submitted() {
    let b = new_board();
    let a = b.task("Add the endpoint", json!({}));
    let m = b.task("Use it", json!({"same_pr": format!("T{a}")}));
    assert_eq!(samepr::review_base(&b.app, &b.row(m)), None);
    samepr::note_head(&b.app, a, Some("abc123")).unwrap();
    assert_eq!(samepr::review_base(&b.app, &b.row(m)).as_deref(), Some("abc123"));
    assert_eq!(samepr::review_base(&b.app, &b.row(a)), None, "the anchor reviews against its base as usual");
}

#[test]
fn the_plan_check_warns_when_a_shared_pr_gets_too_big_to_read() {
    let b = new_board();
    let g = b.goal();
    let a = b.task("Head", json!({"goal_id": g, "wave": 1}));
    for w in 2..=4 {
        b.task(&format!("Part {w}"), json!({"goal_id": g, "wave": w, "same_pr": format!("T{a}")}));
    }
    let w = b.propose(g, json!(["Extra::x::9"]));
    assert!(!w.iter().any(|w| w.contains("one reader")), "four is fine: {w:?}");

    let w = b.propose(g, json!([{"title": "Part 5", "detail": "x", "wave": 5, "same_pr": format!("T{a}")}]));
    let fifth = board::goal_tasks(&b.app, g).unwrap().iter().map(|t| t.id()).max().unwrap();
    assert_eq!(samepr::anchor_id(&b.row(fifth)), Some(a), "a proposed task can share a PR too");
    let want = format!("5 tasks share T{a}'s PR: more than 4 is a lot for one reader. Give some their own PR (tb task set T{fifth} --same-pr none).");
    assert!(w.contains(&want), "{w:?}");
}
