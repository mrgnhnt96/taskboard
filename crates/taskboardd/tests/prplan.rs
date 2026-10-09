//! The PR plan and PR flow from the Python board: stacked PRs (`--stack-on`), a per-task ships-PR flag,
//! canceled PRs (`tb done --no-pr`), per-head author-review steps with findings, and `tb done --pr-body`
//! opening the PR itself (issues #13, #19, #20, #21, #22).

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{PrLink, Row, RowExt};
use taskboardd::{board, handoff, midna, p, prflow, reports, stack, steps};

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
    assert_eq!(c["stack_on"]["line"], format!("Stacks on T{parent}'s PR #12"));
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
    assert!(handoff::build(&b.app, n).unwrap().contains("This task ends without a pull request."));
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
    assert!(args["comment"].as_str().unwrap().contains("PR canceled: T3 fixed it"), "{args}");
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
    assert_eq!(c["pr"]["bar"]["build_url"], "https://ci.example/1");
}

// ------------------------------------------------------------------ #20 tb done --pr-body

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

    let v = done(BODY).unwrap();
    assert_eq!(v["pr_url"], "https://github.com/acme/webapp/pull/77", "{v}");
    let log = std::fs::read_to_string(b.dir.path().join("gh.log")).unwrap();
    assert!(log.contains("pr create --base main --head feat/passkeys --title ABC-12 Add passkeys --body-file -"), "{log}");
    let sent = std::fs::read_to_string(b.dir.path().join("gh.log.stdin")).unwrap();
    assert!(sent.starts_with("## Summary") && sent.contains("## Context\n- Ticket: [ABC-12](https://acme.atlassian.net/browse/ABC-12)"), "{sent}");
    let t = b.row(id);
    assert_eq!(t.st("status"), "done");
    assert_eq!(t.i("pr_num"), Some(77));
}
