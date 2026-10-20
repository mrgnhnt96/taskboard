//! The PR plan and PR flow from the Python board: stacked PRs (`--stack-on`), a per-task ships-PR flag,
//! canceled PRs (`tb submit --no-pr`), per-head author-review steps with findings, and `tb submit --pr-body`
//! opening the PR itself (issues #13, #19, #20, #21, #22).

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{parse_iso, PrLink, Row, RowExt};
use taskboardd::{board, handoff, midna, p, prflow, progress, reports, stack, steps};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn new_board_with(config: &str, tweak: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.gh = fake_gh(dir.path()).to_string_lossy().to_string();
    tweak(&mut cfg);
    std::fs::write(&cfg.config_path, config).unwrap();
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

fn new_board() -> Board {
    new_board_with("", |_| {})
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
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn repo(&self) -> String {
        self.dir.path().join("webapp").to_string_lossy().to_string()
    }
    fn session(&self) {
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": self.repo(), "status": {"state": "working"}})], &[]).unwrap();
    }
    fn task(&self, title: &str, extra: Value) -> i64 {
        let mut body = json!({"title": title, "detail": "Do it.", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", body)["id"].as_i64().unwrap()
    }
    fn take(&self, id: i64) {
        self.session();
        self.report("tb.take", json!({"task": format!("T{id}")})).unwrap();
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn card(&self, id: i64) -> Value {
        self.get(&format!("tasks/T{id}"))
    }
    fn link(&self, id: i64, num: i64, branch: &str) {
        let pr = PrLink { host: "github".into(), repo: "acme/webapp".into(), num, url: format!("https://github.com/acme/webapp/pull/{num}") };
        let t = self.row(id);
        self.app.db.tx(|| prflow::link_pr(&self.app, &t, &pr, "test")).unwrap();
        let rec = json!({"state": "OPEN", "head": format!("{num:0>40}"), "branch": branch, "base": "main", "checks": [{"name": "build", "state": "passed", "url": "https://ci.example/1"}], "failed": [], "running": 0,
                         "comments": 0, "approvals": 1, "review_decision": "APPROVED", "requested": 1});
        prflow::merge_flow(&self.app, id, vec![("rec", rec)]).unwrap();
        board::update_task(&self.app, id, vec![("status", json!("done"))]).unwrap();
    }
}

fn git(path: &Path, args: &[&str]) -> String {
    let o = Command::new("git").arg("-C").arg(path).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

/// The board's project folder as a clone of a bare origin with one commit on main.
fn git_repo(b: &Board) -> std::path::PathBuf {
    let origin = b.dir.path().join("origin.git");
    let repo = b.dir.path().join("webapp");
    std::fs::remove_dir_all(&repo).unwrap();
    git(b.dir.path(), &["init", "-q", "--bare", "-b", "main", &origin.to_string_lossy()]);
    git(b.dir.path(), &["clone", "-q", &origin.to_string_lossy(), &repo.to_string_lossy()]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["switch", "-q", "-c", "main"]);
    std::fs::write(repo.join("README"), "webapp\n").unwrap();
    git(&repo, &["add", "README"]);
    git(&repo, &["commit", "-q", "-m", "First"]);
    git(&repo, &["push", "-q", "origin", "main"]);
    git(&repo, &["remote", "set-head", "origin", "main"]);
    repo
}

/// A `gh` that records its arguments and stdin, and answers like `gh pr create` / `gh pr edit`.
fn fake_gh(dir: &Path) -> std::path::PathBuf {
    let gh = dir.join("gh");
    let log = dir.join("gh.log");
    std::fs::write(
        &gh,
        format!("#!/bin/sh\necho \"$@\" >> '{}'\ncat >> '{}.stdin' 2>/dev/null\necho https://github.com/acme/webapp/pull/77\n", log.display(), log.display()),
    )
    .unwrap();
    Command::new("chmod").arg("+x").arg(&gh).status().unwrap();
    gh
}

// ------------------------------------------------------------------ #13 stacked PRs

#[test]
fn a_stack_in_one_goal_needs_its_parent_in_an_earlier_wave() {
    let b = new_board();
    let g = b.post("/goals", json!({"name": "Checkout", "project": "webapp"}))["id"].as_i64().unwrap();
    let parent = b.task("Parent later", json!({"goal_id": g, "wave": 2}));
    let child = b.task("Child earlier", json!({"goal_id": g, "wave": 1}));
    let earlier = format!("T{parent} has to be in an earlier wave, so its PR is up before this one starts.");
    for wave in [1, 2] {
        b.post(&format!("/tasks/T{child}"), json!({"wave": wave}));
        let (code, msg) = b.post_err(&format!("/tasks/T{child}"), json!({"stack_on": format!("T{parent}")}));
        assert_eq!((code, msg.as_str()), (409, earlier.as_str()), "child in wave {wave}");
        assert_eq!(b.row(child).i("pr_after"), None);
    }
    let (code, msg) = b.post_err("/tasks", json!({"title": "New child", "detail": "Do it.", "project": "webapp", "goal_id": g, "wave": 1, "stack_on": format!("T{parent}")}));
    assert_eq!((code, msg.as_str()), (409, earlier.as_str()), "tb task new --stack-on");

    // Moving the child to a later wave in the same call stacks it.
    b.post(&format!("/tasks/T{child}"), json!({"wave": 3, "stack_on": format!("T{parent}")}));
    assert_eq!(b.row(child).i("pr_after"), Some(parent));

    // Moving either one back so the parent isn't earlier is refused, and nothing moves.
    let (code, _) = b.post_err(&format!("/tasks/T{child}"), json!({"wave": 2}));
    assert_eq!(code, 409);
    let (code, msg) = b.post_err(&format!("/tasks/T{parent}"), json!({"wave": 3}));
    assert_eq!(code, 409);
    assert!(msg.contains(&format!("T{child} stacks on T{parent}")), "{msg}");
    assert_eq!((b.row(child).i("wave"), b.row(parent).i("wave")), (Some(3), Some(2)));

    // A task with no wave isn't held by the waves, so it can stack on any of them.
    let free = b.task("No wave", json!({"goal_id": g}));
    b.post(&format!("/tasks/T{free}"), json!({"stack_on": format!("T{child}")}));
    assert_eq!(b.row(free).i("pr_after"), Some(child));
}

#[test]
fn a_stacked_task_waits_for_its_parent_in_any_goal() {
    let b = new_board();
    let g1 = b.post("/goals", json!({"name": "API", "project": "webapp"}))["id"].as_i64().unwrap();
    let g2 = b.post("/goals", json!({"name": "UI", "project": "webapp"}))["id"].as_i64().unwrap();
    let parent = b.task("Add the endpoint", json!({"goal_id": g1}));
    let child = b.task("Use the endpoint", json!({"goal_id": g2, "stack_on": format!("T{parent}")}));
    assert_eq!(b.row(child).i("pr_after"), Some(parent));
    let c = b.card(child);
    assert_eq!(c["blocked"], true, "{c}");
    assert!(c["waiting"].as_str().unwrap().contains(&format!("T{parent}")), "{c}");
    assert_eq!(c["stack_on"]["ref"], format!("T{parent}"));

    // A cycle and another project are refused.
    let (code, _) = b.post_err(&format!("/tasks/T{parent}"), json!({"stack_on": format!("T{child}")}));
    assert_eq!(code, 409);
    let (code, _) = b.post_err(&format!("/tasks/T{child}"), json!({"stack_on": format!("T{child}")}));
    assert_eq!(code, 400);

    // Once the parent is done with its PR open, the child starts from its branch.
    b.link(parent, 12, "feat/endpoint");
    let c = b.card(child);
    assert_eq!(c["blocked"], false, "{c}");
    assert_eq!(c["stack_on"]["line"], format!("Stacks on T{parent}'s PR #12 · tail merge"));
    assert_eq!(c["stack_on"]["merge"], "tail");
    let h = handoff::build(&b.app, child).unwrap();
    assert!(h.contains("cut this task's branch from origin/feat/endpoint") && h.contains("open the PR into feat/endpoint"), "{h}");
    let v = steps::vars_for(&b.app, &b.row(child));
    assert_eq!(v["base"], "feat/endpoint");
    assert_eq!(v["base_ref"], "origin/feat/endpoint");

    // tb task set --stack-on none.
    b.post(&format!("/tasks/T{child}"), json!({"stack_on": "none"}));
    assert_eq!(b.row(child).i("pr_after"), None);
}

#[test]
fn a_stacked_pr_waits_on_its_base_then_is_pointed_at_main() {
    let b = new_board();
    let parent = b.task("Add the endpoint", json!({}));
    let child = b.task("Use the endpoint", json!({"stack_on": format!("T{parent}")}));
    b.link(parent, 12, "feat/endpoint");
    b.link(child, 13, "feat/use");
    let t = b.row(child);
    let rec = jrec(&t);
    assert_eq!(prflow::phase_of(&b.app, &t, &rec), "waits");
    assert_eq!(prflow::label("waits"), "Waits on base");

    // The parent merges: the child's PR goes into main and may merge.
    b.post(&format!("/tasks/T{parent}/pr/merged"), json!({}));
    let log = std::fs::read_to_string(b.dir.path().join("gh.log")).unwrap();
    assert!(log.contains("pr edit 13 -R acme/webapp --base main"), "{log}");
    let t = b.row(child);
    assert_eq!(prflow::phase_of(&b.app, &t, &jrec(&t)), "merge");
    let c = b.card(child);
    assert_eq!(c["pr"]["bar"]["retargeted"], "main", "{c}");
    // Only once.
    assert_eq!(stack::retarget(&b.app).unwrap(), 0);
}

#[test]
fn a_stack_merge_needs_a_stack_parent() {
    let b = new_board();
    let lone = b.task("Add the endpoint", json!({}));
    let (code, why) = b.post_err(&format!("/tasks/T{lone}"), json!({"stack_merge": "head"}));
    assert_eq!(code, 400, "{why}");
    assert!(why.contains("--stack-on"), "{why}");
    let (code, why) = b.post_err("/tasks", json!({"title": "Use it", "detail": "Do it.", "project": "webapp", "stack_merge": "head"}));
    assert_eq!(code, 400, "{why}");

    let child = b.task("Use the endpoint", json!({"stack_on": format!("T{lone}"), "stack_merge": "head"}));
    assert_eq!(b.row(child).s("stack_merge"), Some("head"));
    let (code, _) = b.post_err(&format!("/tasks/T{child}"), json!({"stack_merge": "sideways"}));
    assert_eq!(code, 400);
    b.post(&format!("/tasks/T{child}"), json!({"stack_merge": "tail"}));
    assert_eq!(b.row(child).s("stack_merge"), None);
    b.post(&format!("/tasks/T{child}"), json!({"stack_merge": "head"}));
    assert_eq!(b.card(child)["stack_on"]["merge"], "head");

    // Moving off the stack drops its merge mode with it.
    b.post(&format!("/tasks/T{child}"), json!({"stack_on": "none"}));
    assert_eq!(b.row(child).s("stack_merge"), None);
}

#[test]
fn a_head_merge_pr_merges_into_its_parent_and_holds_the_parent() {
    let b = new_board();
    let parent = b.task("Add the endpoint", json!({}));
    let child = b.task("Use the endpoint", json!({"stack_on": format!("T{parent}"), "stack_merge": "head"}));
    b.link(parent, 12, "feat/endpoint");
    let c = b.card(child);
    assert_eq!(c["stack_on"]["line"], format!("Stacks on T{parent}'s PR #12 · head merge"), "{c}");
    let h = handoff::build(&b.app, child).unwrap();
    assert!(h.contains(&format!("head-merges into T{parent}")) && h.contains("open the PR into feat/endpoint"), "{h}");
    assert!(h.contains("never moved to the main branch"), "{h}");

    // The parent holds while the child is open, before it has a PR.
    let pt = b.row(parent);
    assert_eq!(prflow::phase_of(&b.app, &pt, &jrec(&pt)), "stack");
    assert_eq!(prflow::label("stack"), "Waits on stack");
    let why = taskboardd::prcmds::merge_blockers(&b.app, &pt, &jrec(&pt), false).unwrap();
    assert!(why.iter().any(|w| w.contains(&format!("T{child} head-merges into it"))), "{why:?}");

    // The child's PR goes into the parent's branch and may merge there: no wait on its base.
    b.link(child, 13, "feat/use");
    prflow::merge_flow(&b.app, child, vec![("rec", json!({"state": "OPEN", "head": "1".repeat(40), "branch": "feat/use", "base": "feat/endpoint",
        "checks": [{"name": "build", "state": "passed", "url": "https://ci.example/2"}], "failed": [], "running": 0, "comments": 0, "approvals": 1,
        "review_decision": "APPROVED", "requested": 1}))]).unwrap();
    let t = b.row(child);
    assert_eq!(prflow::phase_of(&b.app, &t, &jrec(&t)), "merge");
    let why = taskboardd::prcmds::merge_blockers(&b.app, &t, &jrec(&t), false).unwrap();
    assert!(why.is_empty(), "{why:?}");
    let pt = b.row(parent);
    let why = taskboardd::prcmds::merge_blockers(&b.app, &pt, &jrec(&pt), false).unwrap();
    assert!(why.iter().any(|w| w.contains(&format!("T{child}'s PR #13 head-merges into it"))), "{why:?}");

    // Once it merges into the parent's branch, the parent may merge; nothing is ever retargeted.
    b.post(&format!("/tasks/T{child}/pr/merged"), json!({}));
    let pt = b.row(parent);
    assert_eq!(prflow::phase_of(&b.app, &pt, &jrec(&pt)), "merge");
    b.post(&format!("/tasks/T{parent}/pr/merged"), json!({}));
    assert_eq!(stack::retarget(&b.app).unwrap(), 0);
    let log = std::fs::read_to_string(b.dir.path().join("gh.log")).unwrap_or_default();
    assert!(!log.contains("--base main"), "{log}");
}

#[test]
fn a_tail_merge_child_does_not_hold_its_parent() {
    let b = new_board();
    let parent = b.task("Add the endpoint", json!({}));
    let child = b.task("Use the endpoint", json!({"stack_on": format!("T{parent}"), "stack_merge": "tail"}));
    b.link(parent, 12, "feat/endpoint");
    b.link(child, 13, "feat/use");
    let pt = b.row(parent);
    assert_eq!(prflow::phase_of(&b.app, &pt, &jrec(&pt)), "merge");
    assert!(stack::stack_hold(&b.app, &pt).unwrap().is_none());
}

fn jrec(t: &Row) -> Value {
    taskboardd::util::jloads_obj(t.s("pr_flow")).get("rec").cloned().unwrap()
}

// ------------------------------------------------------------------ #22 the PR plan

#[test]
fn a_task_can_end_in_a_pr_or_not_whatever_its_project_does() {
    let b = new_board();
    let g = b.post("/goals", json!({"name": "Plan", "project": "webapp"}))["id"].as_i64().unwrap();
    let a = b.task("Code", json!({"goal_id": g}));
    let n = b.task("Write it up", json!({"goal_id": g, "ships_pr": false}));
    assert_eq!(b.card(a)["ships_pr"], true);
    assert_eq!(b.card(a)["ships_pr_set"], Value::Null);
    assert_eq!(b.card(n)["ships_pr"], false);
    assert_eq!(b.card(n)["ships_pr_set"], false);
    let h = handoff::build(&b.app, n).unwrap();
    assert!(
        h.contains("This task ends without a PR, so it has no Jira ticket. Don't make or link one; a branch it needs is named <type>/<short-slug>."),
        "{h}"
    );
    board::update_task(&b.app, n, vec![("jira_key", json!("ZZZ-11"))]).unwrap();
    let h = handoff::build(&b.app, n).unwrap();
    assert!(!h.contains("ZZZ-11"), "a key it kept from before isn't shown: {h}");
    let gd = b.get(&format!("goals/G{g}"));
    assert_eq!(gd["prs_planned"], 1, "{gd}");

    b.post(&format!("/tasks/T{n}"), json!({"ships_pr": "auto"}));
    assert_eq!(b.card(n)["ships_pr_set"], Value::Null);
    let (code, _) = b.post_err(&format!("/tasks/T{n}"), json!({"ships_pr": "maybe"}));
    assert_eq!(code, 400);

    // tb task new --no-pr, from an agent.
    b.session();
    let v = b.report("tb.new_task", json!({"title": "Notes", "project": "webapp", "ships_pr": false})).unwrap();
    let id: i64 = v["created"][0].as_str().unwrap()[1..].parse().unwrap();
    assert_eq!(b.row(id).i("ships_pr"), Some(0));
}

// ------------------------------------------------------------------ #21 canceled PRs

#[test]
fn done_without_the_pr_keeps_why_and_withdraws_the_ticket() {
    let b = new_board_with("", |c| {
        c.jira.site = "acme.atlassian.net".into();
        c.jira.project = "ABC".into();
        c.jira.canceled = "Withdrawn".into();
    });
    let id = b.task("Fix the flicker", json!({}));
    board::update_task(&b.app, id, vec![("jira_key", json!("ABC-9"))]).unwrap();
    b.take(id);
    b.report("tb.done", json!({"summary": "Already fixed", "no_pr": "T3 fixed it in ABC-7", "no_evidence": "no UI change"})).unwrap();
    let c = b.card(id);
    assert_eq!(c["status"], "done");
    assert_eq!(c["no_pr"], "T3 fixed it in ABC-7");
    assert_eq!(c["no_evidence"], "no UI change");
    let log: Vec<String> = c["log"].as_array().unwrap().iter().map(|e| e["text"].as_str().unwrap_or("").to_string()).collect();
    assert!(log.iter().any(|l| l == "PR canceled: T3 fixed it in ABC-7"), "{log:?}");
    let j = b.app.db.q1("SELECT args FROM jobs WHERE kind = 'jira' AND task_id = ? ORDER BY id DESC", p![id]).unwrap().unwrap();
    let args: Value = serde_json::from_str(j.s("args").unwrap()).unwrap();
    assert_eq!(args["status"], "Withdrawn", "{args}");
    assert_eq!(args["comment"], "PR canceled: T3 fixed it in ABC-7\n\nAlready fixed", "the summary follows the reason");
}

#[test]
fn no_pr_is_for_a_task_that_would_have_one() {
    let b = new_board();
    let id = b.task("Write it up", json!({"ships_pr": false}));
    b.take(id);
    let (code, why) = b.report("tb.done", json!({"summary": "Done", "no_pr": "nothing to ship"})).unwrap_err();
    assert_eq!(code, 409, "{why}");
    let id2 = b.task("Code", json!({}));
    board::update_task(&b.app, id, vec![("status", json!("done"))]).unwrap();
    b.take(id2);
    let (code, _) = b.report("tb.done", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/3", "no_pr": "x"})).unwrap_err();
    assert_eq!(code, 400);
}

// ------------------------------------------------------------------ #19 the author-side review gate

const REVIEW: &str = r#"
[[steps]]
name = "Author review"
prompt = "Run the review"
check = "true"
per_head = true
min_gap_mins = 10
bar = "WD"
"#;

fn round(b: &Board, head: &str, ok: bool, result: Value) -> Value {
    b.report("tb.step", json!({"name": "Author review", "via": "done", "ok": ok, "head": head, "result": result})).unwrap()
}

#[test]
fn a_per_head_step_passes_once_per_commit() {
    let b = new_board_with(REVIEW, |_| {});
    let id = b.task("Add login", json!({}));
    b.take(id);
    let a = "a".repeat(40);
    let c2 = "c".repeat(40);
    round(&b, &a, true, json!(null));
    let t = b.row(id);
    assert!(steps::missing_at(&b.app, &t, &[steps::Before::Pr], Some(&a[..7])).unwrap().is_empty(), "a short sha names the same commit");
    assert_eq!(steps::missing_at(&b.app, &t, &[steps::Before::Pr], Some(&c2)).unwrap().len(), 1, "a new push needs another round");
    let mut q = Query::new();
    q.insert("session".into(), "s1".into());
    q.insert("head".into(), c2.clone());
    let s = api::dispatch(&b.app, "GET", "/steps", &q, &Value::Null).unwrap();
    assert_eq!(s["steps"][0]["done"], false);
    assert!(s["steps"][0]["next_round_at"].is_string(), "rounds are 10 minutes apart: {s}");
    assert_eq!(s["steps"][0]["per_head"], true);
}

const TWO_GAPS: &str = r#"
[[steps]]
name = "Author review"
prompt = "Run the review"
check = "true"
per_head = true
min_gap_mins = 10

[[steps]]
name = "Lint"
run = "true"
min_gap_mins = 5
"#;

#[test]
fn tb_limits_review_gap_changes_the_step_s_gap() {
    let b = new_board_with(REVIEW, |_| {});
    let id = b.task("Add login", json!({}));
    b.take(id);
    let head = "a".repeat(40);
    round(&b, &head, true, json!(null));
    let next = |b: &Board| {
        let mut q = Query::new();
        q.insert("session".into(), "s1".into());
        q.insert("head".into(), head.clone());
        let s = api::dispatch(&b.app, "GET", "/steps", &q, &Value::Null).unwrap();
        s["steps"][0]["next_round_at"].as_str().and_then(parse_iso)
    };
    let at_10 = next(&b).expect("rounds are 10 minutes apart");

    let v = b.post("limits", json!({"review_gap": 25}));
    assert!(v["line"].as_str().unwrap().contains("Review rounds, Author review: one round per branch every 25 min"), "{v}");
    assert_eq!(v["review_gaps"][0]["mins"], 25.0);
    let at_25 = next(&b).expect("still apart");
    assert!((at_25 - at_10 - 15.0 * 60.0).abs() < 2.0, "the next round's earliest start moves with the gap");

    let v = b.post("limits", json!({"review_gap": 0}));
    assert!(v["line"].as_str().unwrap().contains("Author review: no gap between rounds"), "{v}");
    assert!(next(&b).is_none(), "0 means no gap");

    let (code, why) = b.post_err("limits", json!({"review_gap": 300}));
    assert_eq!(code, 400);
    assert!(why.contains("0 to 240"), "{why}");
    let (code, why) = b.post_err("limits", json!({"review_gap": 10, "step": "Nope"}));
    assert_eq!((code, why.contains("“Nope” isn't a step")), (400, true), "{why}");
}

#[test]
fn a_review_gap_names_its_step_when_several_have_one_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    std::fs::write(&cfg.config_path, TWO_GAPS).unwrap();
    let app = App::new(cfg.clone(), false).unwrap();
    let e = taskboardd::limits::set(&app, &json!({"review_gap": 15})).unwrap_err();
    assert!(e.message.contains("More than one step has a review gap: “Author review”, “Lint”"), "{}", e.message);
    taskboardd::limits::set(&app, &json!({"review_gap": 15, "step": "author review"})).unwrap();
    drop(app);

    let app = App::new(cfg.clone(), false).unwrap();
    let gaps: Vec<(String, Option<f64>)> = steps::load(&app).into_iter().map(|s| (s.name, s.min_gap_mins)).collect();
    assert_eq!(gaps, vec![("Author review".to_string(), Some(15.0)), ("Lint".to_string(), Some(5.0))], "the other step is untouched");
    assert_eq!(std::fs::read_to_string(&cfg.config_path).unwrap(), TWO_GAPS, "config.toml isn't rewritten");
    let line = taskboardd::limits::state(&app)["line"].as_str().unwrap().to_string();
    assert!(line.contains("Review rounds, Author review: one round per branch every 15 min"), "{line}");
    assert!(line.contains("Review rounds, Lint: one round per branch every 5 min"), "{line}");
}

#[test]
fn a_rounds_findings_show_on_the_task_and_can_be_triaged() {
    let b = new_board_with(REVIEW, |_| {});
    let id = b.task("Add login", json!({}));
    b.take(id);
    let head = "b".repeat(40);
    round(&b, &head, false, json!({"verdict": "fail", "findings": [
        {"id": "F1", "title": "Typo", "severity": "low"},
        {"id": "F2", "title": "SQL injection", "severity": "high", "file": "api.rs", "line": 12},
        {"id": "F3", "title": "Old", "severity": "high", "state": "fixed"}]}));
    let c = b.card(id);
    let r = &c["step_results"][0];
    assert_eq!(r["headline"], "2 open findings", "{r}");
    assert_eq!(r["open"], 2);
    let ids: Vec<&str> = r["findings"].as_array().unwrap().iter().map(|f| f["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["F2", "F1", "F3"], "open first, then by severity");

    let v = b.report("tb.step_triage", json!({"name": "author review", "finding": "F2", "state": "fixed", "commit": "abc1234", "note": "bound the query"})).unwrap();
    assert_eq!(v["open"], 1);
    let (code, why) = b.report("tb.step_triage", json!({"name": "Author review", "finding": "F9", "state": "fixed"})).unwrap_err();
    assert_eq!(code, 400);
    assert!(why.contains("F1, F2, F3"), "{why}");
    b.report("tb.step_triage", json!({"name": "Author review", "finding": "F1", "state": "dismissed"})).unwrap();
    let r = b.card(id)["step_results"][0].clone();
    assert_eq!(r["headline"], "Answered, not approved", "{r}");

    // An unreviewable round doesn't block.
    round(&b, &head, true, json!({"verdict": "skip"}));
    let r = b.card(id)["step_results"][0].clone();
    assert_eq!(r["headline"], "Couldn't review this round");
    assert!(steps::missing_at(&b.app, &b.row(id), &[steps::Before::Pr], Some(&head)).unwrap().is_empty());

    // The PR bar names it.
    b.link(id, 21, "feat/login");
    let c = b.card(id);
    assert_eq!(c["pr"]["bar"]["wd"]["bar"], "WD", "{}", c["pr"]["bar"]);
    assert_eq!(c["pr"]["bar"]["reviewers"], 2);
    assert_eq!(c["pr"]["bar"]["build_url"], "https://ci.example/1", "with nothing failed, Checks opens the latest build");
}

#[test]
fn the_pr_bar_links_failed_checks_and_says_when_review_needs_setup_or_is_off() {
    let b = new_board();
    let id = b.task("Add login", json!({}));
    b.take(id);
    b.link(id, 22, "feat/login");
    let set_rec = |rec: Value| prflow::merge_flow(&b.app, id, vec![("rec", rec)]).unwrap();
    let head = format!("{:0>40}", 22);
    set_rec(json!({"state": "OPEN", "head": head, "branch": "feat/login", "base": "main", "failed": ["lint"], "running": 0, "comments": 0,
                   "checks": [{"name": "GitGuardian", "state": "passed", "url": "https://dashboard.gitguardian.com/x"},
                              {"name": "lint", "state": "failed", "url": "https://ci.example/lint"}],
                   "approvals": 0, "review_decision": "", "requested": 0}));
    let bar = b.card(id)["pr"]["bar"].clone();
    assert_eq!(bar["build_url"], "https://ci.example/lint", "a failed check opens itself: {bar}");
    assert_eq!(bar["review"], "setup", "nobody on the PR and nobody to ask: {bar}");

    // Someone asked on the host: it isn't a setup question any more.
    set_rec(json!({"state": "OPEN", "head": head, "branch": "feat/login", "base": "main", "failed": [], "running": 0, "comments": 0,
                   "checks": [{"name": "build", "state": "passed"}], "approvals": 0, "review_decision": "", "requested": 1}));
    assert!(b.card(id)["pr"]["bar"]["review"].is_null());

    // A project without a Review step goes to merge once the checks pass.
    let rec = json!({"state": "OPEN", "head": head, "branch": "feat/login", "base": "main", "failed": [], "running": 0, "comments": 0,
                     "checks": [{"name": "build", "state": "passed"}], "approvals": 0, "review_decision": "", "requested": 0});
    set_rec(rec.clone());
    let t = b.row(id);
    assert_eq!(prflow::phase_of(&b.app, &t, &rec), "review");
    let project = b.row(id).st("project");
    let v = b.post(&format!("/projects/{project}"), json!({"review": "off"}));
    assert_eq!(v["pr_rules"]["review"], false, "{v}");
    assert_eq!(prflow::phase_of(&b.app, &b.row(id), &rec), "merge");
    assert_eq!(b.card(id)["pr"]["bar"]["review"], "off");
}

// ------------------------------------------------------------------ #20 tb submit --pr-body

const BODY: &str = "## Summary\nAdds passkey sign-in.\n\n## Changes\n- New sign-in screen\n\n## Testing\n- Unit tests\n";

#[test]
fn done_with_a_pr_body_checks_it_and_opens_the_pr() {
    let b = new_board_with("", |c| {
        c.jira.site = "acme.atlassian.net".into();
        c.jira.project = "ABC".into();
    });
    let repo = git_repo(&b);
    b.app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    let id = b.task("Add passkeys", json!({}));
    board::update_task(&b.app, id, vec![("jira_key", json!("ABC-12"))]).unwrap();
    b.take(id);
    let cwd = repo.to_string_lossy().to_string();
    let done = |body: &str| b.report("tb.done", json!({"summary": "Passkeys", "pr_body": body, "cwd": cwd}));

    // A bad description, then a branch that isn't pushed, then a merge commit.
    let (code, why) = done("Just some text.").unwrap_err();
    assert_eq!(code, 400);
    assert!(why.contains("## Summary"), "{why}");
    git(&repo, &["switch", "-q", "-c", "feat/passkeys"]);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    git(&repo, &["add", "a.txt"]);
    git(&repo, &["commit", "-q", "-m", "Add a"]);
    let (code, why) = done(BODY).unwrap_err();
    assert_eq!(code, 409);
    assert!(why.contains("isn't pushed"), "{why}");
    git(&repo, &["switch", "-q", "main"]);
    std::fs::write(repo.join("m.txt"), "m\n").unwrap();
    git(&repo, &["add", "m.txt"]);
    git(&repo, &["commit", "-q", "-m", "Main moves"]);
    git(&repo, &["push", "-q", "origin", "main"]);
    git(&repo, &["switch", "-q", "feat/passkeys"]);
    git(&repo, &["push", "-q", "-u", "origin", "feat/passkeys"]);
    let (_, why) = done(BODY).unwrap_err();
    assert!(why.contains("isn't rebased on origin/main"), "{why}");
    git(&repo, &["merge", "-q", "--no-edit", "main"]);
    git(&repo, &["push", "-q", "origin", "feat/passkeys"]);
    let (_, why) = done(BODY).unwrap_err();
    assert!(why.contains("merge commit"), "{why}");
    git(&repo, &["reset", "-q", "--hard", "HEAD~1"]);
    git(&repo, &["rebase", "-q", "main"]);
    git(&repo, &["push", "-q", "--force", "origin", "feat/passkeys"]);

    // On the progress stream, tb hears each stage of opening the PR.
    let lines = Arc::new(parking_lot::Mutex::new(vec![]));
    let l = lines.clone();
    let sink: progress::Sink = Arc::new(move |s: &str| l.lock().push(s.to_string()));
    let v = progress::with(sink, || done(BODY)).unwrap();
    assert_eq!(v["pr_url"], "https://github.com/acme/webapp/pull/77", "{v}");
    assert_eq!(*lines.lock(), ["Checked the PR description", "Fetched origin", "feat/passkeys is pushed and up to date with main", "Opened PR #77"]);
    let log = std::fs::read_to_string(b.dir.path().join("gh.log")).unwrap();
    assert!(log.contains("pr create --base main --head feat/passkeys --title ABC-12 Add passkeys --body-file -"), "{log}");
    let sent = std::fs::read_to_string(b.dir.path().join("gh.log.stdin")).unwrap();
    assert!(sent.starts_with("## Summary") && sent.contains("## Context\n- Ticket: [ABC-12](https://acme.atlassian.net/browse/ABC-12)"), "{sent}");
    let t = b.row(id);
    assert_eq!(t.st("status"), "done");
    assert_eq!(t.i("pr_num"), Some(77));
}

#[test]
fn a_failed_task_handed_in_with_a_pr_body_opens_its_pr() {
    let b = new_board_with("", |c| {
        c.jira.site = "acme.atlassian.net".into();
        c.jira.project = "ABC".into();
    });
    let repo = git_repo(&b);
    b.app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    let id = b.task("Add passkeys", json!({}));
    board::update_task(&b.app, id, vec![("jira_key", json!("ABC-12"))]).unwrap();
    b.take(id);
    git(&repo, &["switch", "-q", "-c", "feat/passkeys"]);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    git(&repo, &["add", "a.txt"]);
    git(&repo, &["commit", "-q", "-m", "Add a"]);
    git(&repo, &["push", "-q", "-u", "origin", "feat/passkeys"]);
    b.report("tb.fail", json!({"reason": "Stuck."})).unwrap();
    assert!(b.row(id).b("failed"));

    let cwd = repo.to_string_lossy().to_string();
    let v = b.report("tb.done", json!({"task": format!("T{id}"), "summary": "Passkeys", "pr_body": BODY, "cwd": cwd})).unwrap();
    assert_eq!(v["pr_url"], "https://github.com/acme/webapp/pull/77", "{v}");
    let t = b.row(id);
    assert_eq!(t.st("status"), "done");
    assert!(!t.b("failed"));
    assert_eq!(t.i("pr_num"), Some(77));
}

/// Two tasks with terminals in one shared checkout: A works in its own worktree, B switches the shared
/// checkout to its branch. Each PR opens from its own task's branch, a task with no commits can't take
/// B's, and a second PR for B's branch names B's PR instead of a bare 422.
#[test]
fn tasks_sharing_a_checkout_open_prs_from_their_own_branches() {
    let b = new_board();
    let repo = git_repo(&b);
    let cwd = repo.to_string_lossy().to_string();
    let terms: Vec<Value> = ["s1", "s2", "s3", "s4"].iter().map(|s| json!({"id": s, "name": s, "agent": "claude", "cwd": cwd, "status": {"state": "working"}})).collect();
    midna::sync(&b.app, &terms, &[]).unwrap();
    let report = |sid: &str, event: &str, extra: Value| {
        let mut body = extra;
        body["session"] = json!(sid);
        body["claude_session"] = json!(format!("c-{sid}"));
        b.report(event, body)
    };
    let commit = |dir: &Path, file: &str| {
        std::fs::write(dir.join(file), "x\n").unwrap();
        git(dir, &["add", file]);
        git(dir, &["commit", "-q", "-m", file]);
        git(dir, &["rev-parse", "HEAD"])
    };
    let opened = || std::fs::read_to_string(b.dir.path().join("gh.log")).unwrap_or_default();
    let a = b.task("Accent contrast", json!({}));
    let bb = b.task("Axe findings", json!({}));
    report("s1", "tb.take", json!({"task": format!("T{a}")})).unwrap();
    report("s2", "tb.take", json!({"task": format!("T{bb}")})).unwrap();

    // A commits in its worktree from the shared checkout's terminal.
    let wt = b.dir.path().join("webapp-a");
    git(&repo, &["worktree", "add", "-q", "-b", "fix/accent", &wt.to_string_lossy()]);
    let sha = commit(&wt, "accent.txt");
    git(&wt, &["push", "-q", "-u", "origin", "fix/accent"]);
    let cmd = format!("git -C {} commit -m accent", wt.display());
    report("s1", "hook.commit", json!({"cwd": cwd, "command": cmd, "git": {"branch": "main"}, "work_git": {"branch": "fix/accent", "sha": sha}})).unwrap();

    // B switches the shared checkout to its branch and pushes it.
    git(&repo, &["switch", "-q", "-c", "fix/axe"]);
    let sha = commit(&repo, "axe.txt");
    git(&repo, &["push", "-q", "-u", "origin", "fix/axe"]);
    report("s2", "hook.commit", json!({"cwd": cwd, "command": "git commit -m axe", "git": {"branch": "fix/axe", "sha": sha}})).unwrap();

    // A's terminal sits on B's branch, but A's PR comes from A's.
    report("s1", "tb.done", json!({"summary": "Accent", "pr_body": BODY, "cwd": cwd, "git": {"branch": "fix/axe"}})).unwrap();
    assert!(opened().contains("--head fix/accent"), "{}", opened());
    assert!(!opened().contains("--head fix/axe"), "{}", opened());
    let gh = std::fs::read_to_string(&b.app.cfg.pr.gh).unwrap().replace("pull/77", "pull/78");
    std::fs::write(&b.app.cfg.pr.gh, gh).unwrap();
    report("s2", "tb.done", json!({"summary": "Axe", "pr_body": BODY, "cwd": cwd, "git": {"branch": "fix/axe"}})).unwrap();
    assert!(opened().contains("--head fix/axe"), "{}", opened());
    assert_eq!((b.row(a).i("pr_num"), b.row(bb).i("pr_num")), (Some(77), Some(78)));

    // A task that committed nothing can't open a PR from B's branch.
    let c = b.task("Stray", json!({}));
    report("s3", "tb.take", json!({"task": format!("T{c}")})).unwrap();
    let (code, why) = report("s3", "tb.done", json!({"summary": "Stray", "pr_body": BODY, "cwd": cwd})).unwrap_err();
    assert_eq!(code, 409);
    assert!(why.contains(&format!("fix/axe is T{bb}'s branch")), "{why}");

    // The host refuses a second PR for B's branch: the refusal names B's PR.
    std::fs::write(&b.app.cfg.pr.gh, "#!/bin/sh\necho 'a pull request for branch \"fix/axe\" into branch \"main\" already exists:' >&2\nexit 1\n").unwrap();
    let d = b.task("Also axe", json!({}));
    report("s4", "tb.take", json!({"task": format!("T{d}")})).unwrap();
    let sha = commit(&repo, "axe2.txt");
    git(&repo, &["push", "-q", "origin", "fix/axe"]);
    report("s4", "hook.commit", json!({"cwd": cwd, "command": "git commit -m axe2", "git": {"branch": "fix/axe", "sha": sha}})).unwrap();
    let (code, why) = report("s4", "tb.done", json!({"summary": "Also", "pr_body": BODY, "cwd": cwd})).unwrap_err();
    assert_eq!(code, 502);
    assert!(why.contains(&format!("fix/axe already has an open PR #78 (https://github.com/acme/webapp/pull/78), T{bb}'s")), "{why}");
}

#[test]
fn a_change_from_a_terminal_is_credited_to_that_terminal() {
    let b = new_board();
    b.session();
    let id = b.task("Write it up", json!({}));
    let log = |text: &str| -> Vec<String> {
        b.app.db.q("SELECT who FROM events WHERE task_id = ? AND text = ?", p![id, text]).unwrap().iter().map(|r| r.st("who")).collect()
    };
    b.post(&format!("tasks/T{id}"), json!({"ships_pr": false, "by_session": "s1"}));
    assert_eq!(log("Ends without a PR"), vec!["Term"]);
    b.post(&format!("tasks/T{id}"), json!({"locks": ["db"]}));
    assert_eq!(log("Holds the lock db while it runs"), vec![board::OWNER], "the app's change stays the owner's");
    b.post(&format!("tasks/T{id}"), json!({"locks": ["cache"], "by_session": "s1", "who": "Sam"}));
    assert_eq!(log("Holds the lock cache while it runs"), vec!["Sam"], "--who wins over the terminal");
}

// ------------------------------------------------------------------ Jira moves on a shipped ticket (#233) and code review (#236)

fn jira_board() -> Board {
    new_board_with("", |c| {
        c.jira.site = "acme.atlassian.net".into();
        c.jira.project = "ABC".into();
        c.jira.canceled = "Withdrawn".into();
        c.jira.merged = "Ready for QA".into();
        c.jira.in_review = "In Code Review".into();
        c.reviewers.ask_stage = true;
    })
}

fn jira_jobs(b: &Board, id: i64) -> Vec<Value> {
    b.app.db.q("SELECT args FROM jobs WHERE kind = 'jira' AND task_id = ? ORDER BY id", p![id]).unwrap().iter().map(|r| serde_json::from_str(r.s("args").unwrap()).unwrap()).collect()
}

fn statuses(b: &Board, id: i64) -> Vec<String> {
    jira_jobs(b, id).iter().filter_map(|a| a["status"].as_str().map(|s| s.to_string())).collect()
}

fn on_ticket(b: &Board, title: &str, extra: Value) -> i64 {
    let id = b.task(title, extra);
    board::update_task(&b.app, id, vec![("jira_key", json!("ABC-12"))]).unwrap();
    id
}

#[test]
fn a_follow_up_on_a_shipped_ticket_leaves_it_in_qa() {
    let b = jira_board();
    let shipped = on_ticket(&b, "Fix the tab bar", json!({}));
    board::update_task(&b.app, shipped, vec![("pr_state", json!("MERGED")), ("status", json!("done"))]).unwrap();

    let look = on_ticket(&b, "Answer QA", json!({"ships_pr": false}));
    b.take(look);
    assert!(statuses(&b, look).is_empty(), "a no-PR follow-up doesn't pull the ticket back");
    b.report("tb.done", json!({"summary": "Answered QA's question"})).unwrap();
    let jobs = jira_jobs(&b, look);
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!((jobs[0]["op"].as_str(), jobs[0]["status"].is_null()), (Some("comment"), true), "a --pr no task only comments");

    let fix = on_ticket(&b, "Fix it again", json!({"ships_pr": true}));
    b.take(fix);
    assert_eq!(statuses(&b, fix), vec!["In Progress"], "a follow-up that ships a PR moves it");
    b.report("tb.done", json!({"summary": "Nothing to change", "no_pr": "nothing to change", "no_evidence": "no UI change"})).unwrap();
    assert_eq!(statuses(&b, fix).last().map(String::as_str), Some("Ready for QA"), "a shipped ticket goes back to QA, not Withdrawn");
}

#[test]
fn no_pr_still_withdraws_a_ticket_that_never_shipped() {
    let b = jira_board();
    let id = on_ticket(&b, "Fix the flicker", json!({"ships_pr": true}));
    b.take(id);
    b.report("tb.done", json!({"summary": "Already fixed", "no_pr": "T3 fixed it", "no_evidence": "no UI change"})).unwrap();
    assert_eq!(statuses(&b, id).last().map(String::as_str), Some("Withdrawn"));
}

#[test]
fn the_ticket_moves_to_code_review_once_reviewers_are_asked() {
    let b = jira_board();
    let id = on_ticket(&b, "Fix the tab bar", json!({}));
    b.take(id);
    b.link(id, 7, "fix/tabs");
    let before = statuses(&b, id).len();
    let green = json!({"state": "OPEN", "head": "h1", "checks": [{"name": "build", "state": "passed"}], "failed": [], "running": 0,
                       "comments": 0, "approvals": 0, "review_decision": "REVIEW_REQUIRED"});
    let step = |phase: &str, flow: Vec<(&str, Value)>| {
        board::update_task(&b.app, id, vec![("pr_phase", json!(phase))]).unwrap();
        prflow::merge_flow(&b.app, id, flow).unwrap();
        b.app.db.tx(|| prflow::step(&b.app, &b.row(id), &green)).unwrap();
        b.row(id).st("pr_phase")
    };
    assert_eq!(step("checks", vec![("reviewed", json!("2026-10-12T00:00:00Z")), ("asked", Value::Null)]), "ask");
    assert_eq!(statuses(&b, id).len(), before, "checks → ask queues nothing");
    assert_eq!(step("ask", vec![("asked", json!({"at": "2026-10-12T00:00:00Z"}))]), "review");
    assert_eq!(statuses(&b, id)[before..], ["In Code Review"], "ask → review queues In Code Review");
    assert_eq!(step("review", vec![("asked", Value::Null)]), "ask");
    assert_eq!(statuses(&b, id).len(), before + 1, "review → ask doesn't move it again");
}

#[test]
fn done_at_the_ask_stage_only_comments() {
    let b = jira_board();
    let id = on_ticket(&b, "Fix the tab bar", json!({}));
    b.take(id);
    let pr = PrLink { host: "github".into(), repo: "acme/webapp".into(), num: 8, url: "https://github.com/acme/webapp/pull/8".into() };
    b.app.db.tx(|| prflow::link_pr(&b.app, &b.row(id), &pr, "test")).unwrap();
    board::update_task(&b.app, id, vec![("pr_phase", json!("ask")), ("pr_state", json!("OPEN"))]).unwrap();
    let before = statuses(&b, id).len();
    b.report("tb.done", json!({"summary": "Fixed it", "pr": "https://github.com/acme/webapp/pull/8"})).unwrap();
    assert_eq!(statuses(&b, id).len(), before, "at ask the ticket only gets the summary");

    let id2 = on_ticket(&b, "Fix the menu", json!({}));
    b.take(id2);
    let pr = PrLink { host: "github".into(), repo: "acme/webapp".into(), num: 9, url: "https://github.com/acme/webapp/pull/9".into() };
    b.app.db.tx(|| prflow::link_pr(&b.app, &b.row(id2), &pr, "test")).unwrap();
    board::update_task(&b.app, id2, vec![("pr_phase", json!("review")), ("pr_state", json!("OPEN"))]).unwrap();
    b.report("tb.done", json!({"summary": "Fixed it", "pr": "https://github.com/acme/webapp/pull/9"})).unwrap();
    assert_eq!(statuses(&b, id2).last().map(String::as_str), Some("In Code Review"));
}
