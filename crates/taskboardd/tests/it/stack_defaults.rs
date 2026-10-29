//! A project's stack-merge default (`tb project set --stack-merge-default`), the plan check's stack and
//! PR-count warnings, and the alert for a stacked PR the host has going somewhere else (issues #792, #793).

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{PrLink, Row, RowExt};
use taskboardd::{board, dispatch, midna, prflow, reports, stack};

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
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn post_err(&self, path: &str, body: Value) -> (u16, String) {
        let e = api::dispatch(&self.app, "POST", path, &Query::new(), &body).expect_err("should fail");
        (e.status, e.message)
    }
    fn project(&self) -> Value {
        self.get("/projects")["projects"].as_array().unwrap().iter().find(|p| p["name"] == "webapp").cloned().unwrap()
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
    fn link(&self, id: i64, num: i64, branch: &str, base: &str) {
        let pr = PrLink { host: "github".into(), repo: "acme/webapp".into(), num, url: format!("https://github.com/acme/webapp/pull/{num}") };
        let t = self.row(id);
        self.app.db.tx(|| prflow::link_pr(&self.app, &t, &pr, "test")).unwrap();
        let rec = json!({"state": "OPEN", "head": format!("{num:0>40}"), "branch": branch, "base": base, "checks": [], "failed": [], "running": 0});
        prflow::merge_flow(&self.app, id, vec![("rec", rec)]).unwrap();
        board::update_task(&self.app, id, vec![("status", json!("done"))]).unwrap();
    }
}

#[test]
fn a_project_s_stack_merge_default_is_set_shown_and_used_by_stack_on() {
    let b = new_board();
    let p = b.project();
    assert_eq!((&p["stack_merge_default"], &p["stack_merge_set"]), (&json!("tail"), &Value::Null), "{p}");
    let (code, msg) = b.post_err("/projects/webapp", json!({"stack_merge_default": "sideways"}));
    assert_eq!((code, msg.as_str()), (400, "stack_merge_default is head, tail or default."));

    b.post("/projects/webapp", json!({"stack_merge_default": "head"}));
    let p = b.project();
    assert_eq!((&p["stack_merge_default"], &p["stack_merge_set"]), (&json!("head"), &json!("head")), "{p}");

    let parent = b.task("Add the endpoint", json!({}));
    let child = b.task("Use the endpoint", json!({"stack_on": format!("T{parent}")}));
    assert_eq!(b.row(child).s("stack_merge"), Some("head"), "tb task new --stack-on takes the default");
    let tail = b.task("Ship the endpoint first", json!({"stack_on": format!("T{parent}"), "stack_merge": "tail"}));
    assert_eq!(b.row(tail).s("stack_merge"), None, "--stack-merge wins over the default");

    let later = b.task("Docs for the endpoint", json!({}));
    b.post(&format!("/tasks/T{later}"), json!({"stack_on": format!("T{parent}")}));
    assert_eq!(b.row(later).s("stack_merge"), Some("head"), "tb task set --stack-on takes it too");
    b.post(&format!("/tasks/T{tail}"), json!({"stack_on": format!("T{later}")}));
    assert_eq!(b.row(tail).s("stack_merge"), None, "a new parent keeps the task's mode");

    b.post("/projects/webapp", json!({"stack_merge_default": "default"}));
    let p = b.project();
    assert_eq!((&p["stack_merge_default"], &p["stack_merge_set"]), (&json!("tail"), &Value::Null), "{p}");
    let fresh = b.task("Tail by default", json!({"stack_on": format!("T{parent}")}));
    assert_eq!(b.row(fresh).s("stack_merge"), None);
}

#[test]
fn tb_planning_and_the_planner_name_the_project_s_default() {
    let b = new_board();
    let q: Query = [("project".to_string(), "webapp".to_string())].into();
    let planning = |b: &Board| api::dispatch(&b.app, "GET", "/planning", &q, &json!({})).unwrap();
    assert_eq!(planning(&b)["stack_merge"], json!({"default": "tail", "set": false}));
    let prompt = |b: &Board| {
        let g = b.goal();
        api::goal_context(&b.app, &board::get_goal(&b.app, g).unwrap(), true).unwrap()
    };
    assert!(!prompt(&b).contains("merges its stacks by default"));

    b.post("/projects/webapp", json!({"stack_merge_default": "head"}));
    assert_eq!(planning(&b)["stack_merge"], json!({"default": "head", "set": true}));
    let line = "- webapp head-merges its stacks by default: --stack-on without --stack-merge is --stack-merge head, and a task that tail-merges says why in its detail.";
    assert!(prompt(&b).contains(line), "{}", prompt(&b));
}

#[test]
fn the_plan_check_warns_on_tail_stacks_and_on_a_mode_off_the_project_s_default() {
    let b = new_board();
    let g = b.goal();
    let parent = b.task("Add the endpoint", json!({"goal_id": g, "wave": 1}));
    let w = b.propose(g, json!([{"title": "Use it", "detail": "x", "wave": 2, "stack_on": format!("T{parent}")}]));
    let child = parent + 1;
    assert!(w.iter().any(|w| w.starts_with(&format!("T{child} tail-merges its stack: once a parent's PR merges"))), "{w:?}");

    let g2 = b.goal();
    let parent2 = b.task("Add the page", json!({"goal_id": g2, "wave": 1}));
    let w = b.propose(g2, json!([{"title": "Style it", "detail": "x", "wave": 2, "stack_on": format!("T{parent2}"), "stack_merge": "head"}]));
    assert!(!w.iter().any(|w| w.contains("merges its stack")), "a head stack is what the rules ask for: {w:?}");

    b.post("/projects/webapp", json!({"stack_merge_default": "head"}));
    let g3 = b.goal();
    let parent3 = b.task("Add the form", json!({"goal_id": g3, "wave": 1}));
    let w = b.propose(g3, json!([{"title": "Ship the form first", "detail": "x", "wave": 2, "stack_on": format!("T{parent3}"), "stack_merge": "tail"}]));
    let odd = parent3 + 1;
    let want = format!("T{odd} tail-merges its stack, but the project head-merges stacks by default: say why in the detail, or tb task set T{odd} --stack-merge head.");
    assert!(w.contains(&want), "{w:?}");
    assert!(!w.iter().any(|w| w.contains("once a parent's PR merges")), "a project with a default only hears about the odd ones: {w:?}");
}

#[test]
fn the_plan_check_warns_when_an_area_opens_many_prs_to_main() {
    let b = new_board();
    std::fs::write(b.dir.path().join("webapp").join(".taskboard.toml"), "[[areas]]\nname = \"Remote\"\npaths = [\"apps/remote\"]\n").unwrap();
    b.post("/projects/webapp", json!({"pr_flow": "on"}));
    let g = b.goal();
    let three: Vec<&str> = vec!["A::x::1::::apps/remote/a.rs", "B::x::1::::apps/remote/b.rs", "C::x::1::::apps/remote/c.rs"];
    let w = b.propose(g, json!(three));
    assert!(!w.iter().any(|w| w.contains("PR to the main branch")), "three is fine: {w:?}");
    let w = b.propose(g, json!(["D::x::1::::apps/remote/d.rs", "Elsewhere::x::1::::docs/e.md"]));
    let line = w.iter().find(|w| w.contains("each open a PR to the main branch in Remote")).unwrap_or_else(|| panic!("{w:?}"));
    assert!(line.ends_with("stack the related ones on one head with --stack-merge head."), "{line}");
    assert_eq!(line.matches('T').count(), 4, "the four in Remote, not the one elsewhere: {line}");
}

#[test]
fn a_stacked_pr_into_another_base_on_the_host_raises_an_alert_once() {
    let b = new_board();
    let parent = b.task("Add the endpoint", json!({}));
    let child = b.task("Use the endpoint", json!({"stack_on": format!("T{parent}")}));
    b.link(parent, 12, "feat/endpoint", "main");
    b.link(child, 13, "feat/use", "main");
    assert_eq!(stack::base_mismatches(&b.app).unwrap(), 1);
    let alerts: Vec<Value> = dispatch::alerts(&b.app).into_iter().filter(|a| a["task_id"].as_i64() == Some(child)).collect();
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    let want = format!("T{child} stacks on T{parent} (branch feat/endpoint), but its PR #13 goes into main on the host. Point the PR at feat/endpoint, or unstack it with tb task set T{child} --stack-on none.");
    assert_eq!(alerts[0]["text"], want);
    assert_eq!(alerts[0]["key"], format!("stackbase:T{child}"));
    assert!(!stack::base_resolved(&b.app, &alerts[0]).unwrap());
    assert_eq!(stack::base_mismatches(&b.app).unwrap(), 0, "once per base");

    let mut rec = taskboardd::util::jloads_obj(b.row(child).s("pr_flow"))["rec"].clone();
    rec["base"] = json!("feat/endpoint");
    prflow::merge_flow(&b.app, child, vec![("rec", rec)]).unwrap();
    assert!(stack::base_resolved(&b.app, &alerts[0]).unwrap(), "the PR goes into its parent's branch now");
    assert_eq!(stack::base_mismatches(&b.app).unwrap(), 0);
}
