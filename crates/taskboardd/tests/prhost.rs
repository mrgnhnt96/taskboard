//! PRs on a pluggable host (#2, #10, #11, #12, #14, #15, #16): a Bitbucket PR is watched like a GitHub
//! one, open threads drive the stage, `tb pr reply` / `ack` / `addressed` / `merge` / `not-ours` act
//! through the host, and a project's rules (approvals, expected checks, failures_cmd) hold.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::{Config, PrProject};
use taskboardd::prhost::{self, Check, FakeHost, Record, Reviewer, Thread};
use taskboardd::util::{now_ts, RowExt};
use taskboardd::{board, midna, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board_with(f: impl FnOnce(&mut Config)) -> Board {
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
    fn post(&self, path: &str, body: Value) -> Value {
        self.try_post(path, body).unwrap_or_else(|e| panic!("POST {path}: {e}"))
    }
    fn try_post(&self, path: &str, body: Value) -> Result<Value, String> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| e.message)
    }
    fn get(&self, path: &str, query: &[(&str, &str)]) -> Value {
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
    fn pr_task(&self, url: &str) -> i64 {
        let id = self.post("/tasks", json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", "s1", json!({"task": format!("T{id}")}));
        self.report("tb.done", "s1", json!({"summary": "Done", "pr": url}));
        id
    }
    fn task(&self, id: i64) -> serde_json::Map<String, Value> {
        board::get_task(&self.app, id).unwrap()
    }
    fn phase(&self, id: i64) -> String {
        self.task(id).st("pr_phase")
    }
    fn flow(&self, id: i64) -> Value {
        serde_json::from_str(&self.task(id).st("pr_flow")).unwrap()
    }
}

const BB: &str = "https://bitbucket.org/acme/webapp/pull-requests/9";

fn check(name: &str, state: &str) -> Check {
    Check { name: name.into(), state: state.into(), url: None }
}

fn reviewer(user: &str, state: &str) -> Reviewer {
    Reviewer { user: user.into(), name: user.to_uppercase(), state: state.into(), requested: true, changes_at: None }
}

fn thread(id: &str, last: &str) -> Thread {
    Thread {
        id: id.into(),
        kind: "review".into(),
        resolvable: true,
        author: "rev".into(),
        author_name: "Rev".into(),
        last_author: last.into(),
        last_id: format!("{id}-c"),
        text: format!("Thread {id}"),
        ..Default::default()
    }
}

fn green() -> Record {
    Record {
        state: "OPEN".into(),
        title: "Add x".into(),
        author: "me".into(),
        head: "h1".into(),
        branch: "feat".into(),
        base: "main".into(),
        base_head: "b1".into(),
        checks: vec![check("build", "passed")],
        ..Default::default()
    }
}

fn fake(b: &Board, rec: Record) -> Arc<FakeHost> {
    let h = FakeHost::new("bitbucket", rec);
    prhost::install(&b.app, h.clone());
    h
}

fn poll(b: &Board) {
    prflow::refresh(&b.app).unwrap();
}

#[test]
fn a_bitbucket_pr_is_watched_and_its_open_threads_drive_the_stage() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    assert_eq!(b.phase(id), "checks", "a Bitbucket PR gets a stage too");
    let mut rec = green();
    rec.threads = vec![thread("1", "rev"), thread("2", "me")];
    let h = fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "comments", "thread 1 waits on the author; thread 2 had their last word");
    let card = board::pr_card(&b.task(id));
    assert_eq!(card["stage"]["open_threads"], 1);

    let v = b.post(&format!("/tasks/T{id}/pr/reply"), json!({"thread": "1", "text": "Renamed it", "resolve": true, "who": "The agent"}));
    assert_eq!(v["resolved"], true);
    assert_eq!(h.calls(), vec!["reply 1 Renamed it", "resolve 1"]);
    assert_eq!(b.phase(id), "review", "nothing is open; nobody approved yet");
    assert!(b.try_post(&format!("/tasks/T{id}/pr/reply"), json!({"thread": "9", "text": "x"})).unwrap_err().contains("no thread 9"));
}

#[test]
fn ack_resolves_without_a_reply_and_holds_until_someone_speaks_again() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    let mut plain = thread("c1", "rev");
    plain.kind = "comment".into();
    plain.resolvable = false;
    rec.threads = vec![plain.clone()];
    rec.reviewers = vec![reviewer("ok", "approved")];
    rec.approvals = 1;
    let h = fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "comments");
    b.post(&format!("/tasks/T{id}/pr/ack"), json!({"thread": "c1"}));
    assert_eq!(h.calls(), Vec::<String>::new(), "a comment the host can't resolve is only noted on the board");
    assert_eq!(b.flow(id)["threads_acked"]["c1"], "c1-c");
    assert_eq!(b.phase(id), "merge");
    // The reviewer writes again on it.
    h.rec.lock().threads[0].last_id = "c1-d".into();
    poll(&b);
    assert_eq!(b.phase(id), "comments", "a new word reopens it");
}

#[test]
fn addressed_refuses_while_threads_are_open_then_asks_the_reviewers_again() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.review_decision = "CHANGES_REQUESTED".into();
    rec.changes_at = Some("t1".into());
    rec.reviewers = vec![reviewer("rev", "changes"), reviewer("gone", "changes"), reviewer("ok", "approved")];
    rec.threads = vec![thread("1", "rev")];
    let h = fake(&b, rec);
    prflow::merge_flow(&b.app, id, vec![("swapped_off", json!(["gone"]))]).unwrap();
    poll(&b);
    assert_eq!(b.phase(id), "comments");
    let e = b.try_post(&format!("/tasks/T{id}/pr/addressed"), json!({})).unwrap_err();
    assert!(e.contains("still has 1 thread open: 1"), "{e}");
    b.post(&format!("/tasks/T{id}/pr/ack"), json!({"thread": "1"}));
    let v = b.post(&format!("/tasks/T{id}/pr/addressed"), json!({"who": "The agent"}));
    assert_eq!(v["asked"], json!(["REV"]), "not the reviewer swapped off");
    assert!(h.calls().contains(&"re-request rev".to_string()));
    assert_eq!(b.phase(id), "rereview");
    assert_eq!(b.flow(id)["addressed"]["asked"], json!(["REV"]));
}

#[test]
fn merge_checks_everything_first_then_merges_and_closes_the_branch() {
    let b = board_with(|c| {
        c.pr.agents_merge = true;
        c.pr.projects.insert("webapp".into(), PrProject { approvals: Some(2), merge_strategy: Some("squash".into()), ..Default::default() });
    });
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.checks.push(check("e2e", "running"));
    rec.reviewers = vec![reviewer("a", "approved")];
    rec.approvals = 1;
    rec.threads = vec![thread("1", "rev"), Thread { kind: "task".into(), ..thread("task-5", "rev") }];
    rec.tasks_open = 1;
    let h = fake(&b, rec);
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    for want in ["checks are still running: e2e", "it has 1 of the 2 approvals it needs", "1 thread open", "1 PR task open"] {
        assert!(e.contains(want), "{want} in {e}");
    }
    {
        let mut r = h.rec.lock();
        r.checks.pop();
        r.reviewers.push(reviewer("b", "approved"));
        r.threads.clear();
        r.tasks_open = 0;
    }
    poll(&b);
    assert_eq!(b.phase(id), "merge", "two approvals is enough for this project");
    b.post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true, "who": "The agent"}));
    assert!(h.calls().contains(&"merge close=true strategy=squash".to_string()));
    assert_eq!(b.phase(id), "merged");
}

#[test]
fn an_agent_can_t_merge_when_the_owner_merges() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let h = fake(&b, green());
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains("merges PRs on this board"), "{e}");
    assert!(h.calls().is_empty());
}

#[test]
fn a_stacked_base_must_merge_first() {
    let b = board_with(|_| {});
    let below = b.pr_task("https://bitbucket.org/acme/webapp/pull-requests/8");
    let mut low = green();
    low.branch = "base-work".into();
    fake(&b, low);
    poll(&b);
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.base = "base-work".into();
    rec.reviewers = vec![reviewer("a", "approved")];
    fake(&b, rec);
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({})).unwrap_err();
    assert!(e.contains(&format!("its base branch base-work is T{below}'s PR #8, which hasn't merged yet")), "{e}");
}

#[test]
fn not_ours_clears_only_this_push_s_failed_checks_with_proof() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let h = fake(&b, green());
    let url = format!("/tasks/T{id}/pr/not-ours");
    let good = json!({"reason": "The base branch fails the same e2e test on its last builds.", "title": "Flaky login e2e", "proof": ["https://ci.example.com/b/1"]});
    assert!(b.try_post(&url, good.clone()).unwrap_err().contains("Nothing failed"));
    {
        let mut r = h.rec.lock();
        r.checks = vec![check("build", "passed"), check("e2e", "failed"), check("lint", "failed")];
    }
    poll(&b);
    assert_eq!(b.phase(id), "fix");
    let short = json!({"reason": "flaky", "title": "x", "proof": ["https://ci.example.com/b/1"]});
    assert!(b.try_post(&url, short).unwrap_err().contains("20 to 300"));
    let mut no_proof = good.clone();
    no_proof["proof"] = json!(["see CI"]);
    assert!(b.try_post(&url, no_proof).unwrap_err().contains("isn't a link"));
    let mut wrong = good.clone();
    wrong["checks"] = json!(["build"]);
    assert!(b.try_post(&url, wrong).unwrap_err().contains("build didn't fail"));
    let mut e2e = good.clone();
    e2e["checks"] = json!(["e2e"]);
    let v = b.post(&url, e2e);
    assert_eq!(v["cleared"], json!(["e2e"]));
    assert_eq!(b.phase(id), "fix", "lint still fails");
    let mut lint = good.clone();
    lint["checks"] = json!(["lint"]);
    b.post(&url, lint);
    assert_eq!(b.phase(id), "review");
    let card = board::pr_card(&b.task(id));
    assert_eq!(card["checks"], "pass");
    assert_eq!(card["stage"]["not_ours"]["title"], "Flaky login e2e");
    assert_eq!(card["stage"]["not_ours"]["checks"], json!(["e2e", "lint"]));
    // A new push has to pass on its own.
    h.rec.lock().head = "h2".into();
    poll(&b);
    assert_eq!(b.phase(id), "fix");
    assert!(board::pr_card(&b.task(id))["stage"]["not_ours"].is_null());
}

#[test]
fn expected_checks_hold_the_stage_until_they_post_or_their_wait_runs_out() {
    let b = board_with(|c| {
        c.pr.projects.insert("webapp".into(), PrProject { expected: Some(vec!["build".into(), "E2E".into()]), expected_wait_mins: Some(30.0), ..Default::default() });
    });
    let id = b.pr_task(BB);
    let h = fake(&b, green());
    poll(&b);
    assert_eq!(b.phase(id), "checks", "e2e hasn't posted");
    h.rec.lock().checks.push(check("e2e", "passed"));
    poll(&b);
    assert_eq!(b.phase(id), "review");
    // Another push where e2e never posts: the wait runs out.
    {
        let mut r = h.rec.lock();
        r.head = "h2".into();
        r.checks.pop();
    }
    poll(&b);
    assert_eq!(b.phase(id), "checks");
    prflow::merge_flow(&b.app, id, vec![("head_at", json!({"h1": now_ts() - 3600.0, "h2": now_ts() - 31.0 * 60.0}))]).unwrap();
    poll(&b);
    assert_eq!(b.phase(id), "review");
}

#[test]
fn an_empty_expected_list_doesn_t_wait() {
    let b = board_with(|c| {
        c.pr.projects.insert("webapp".into(), PrProject { expected: Some(vec![]), ..Default::default() });
    });
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.checks.clear();
    fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "review", "no checks and none expected: no grace wait");
}

#[test]
fn status_shows_failed_steps_base_failures_reviewers_threads_and_what_blocks_the_merge() {
    let b = board_with(|c| {
        c.pr.projects.insert(
            "webapp".into(),
            PrProject { approvals: Some(2), failures_cmd: Some("echo \"Run $TB_CHECK\"; echo test: login_works".into()), ..Default::default() },
        );
    });
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.checks = vec![check("build", "passed"), check("e2e", "failed")];
    rec.reviewers = vec![reviewer("a", "approved"), reviewer("r", "changes")];
    rec.threads = vec![Thread { author: "r".into(), ..thread("1", "rev") }];
    rec.review_decision = "CHANGES_REQUESTED".into();
    let h = fake(&b, rec);
    *h.base_failing.lock() = vec!["e2e".into()];
    poll(&b);
    h.rec.lock().base_head = "b2".into();
    let v = b.get(&format!("/tasks/T{id}/pr"), &[("full", "1")]);
    let live = &v["live"];
    assert_eq!(live["base_moved"], true);
    let rebase: Vec<&str> = live["rebase"].as_array().unwrap().iter().filter_map(|c| c.as_str()).collect();
    assert_eq!(rebase[rebase.len() - 2..], ["git push --force-with-lease origin feat", "Don't push only to rebase."], "{rebase:?}");
    let f =&live["failures"][0];
    assert_eq!(f["check"], "e2e");
    assert_eq!(f["base_fails"], true);
    assert_eq!(f["steps"], json!(["Run e2e"]));
    assert_eq!(f["tests"], json!(["login_works"]));
    assert_eq!(f["source"], "the project's failures_cmd");
    assert!(h.calls().contains(&"base main 5".to_string()));
    assert_eq!(live["reviewers"].as_array().unwrap().len(), 2);
    assert_eq!(live["approvals"], json!({"have": 1, "need": 2}));
    assert_eq!(live["open_threads"][0]["id"], "1");
    let blockers: Vec<&str> = live["blockers"].as_array().unwrap().iter().filter_map(|x| x.as_str()).collect();
    assert_eq!(blockers, vec!["checks failed: e2e", "R asked for changes", "1 thread open"]);
    assert_eq!(v["watched"], true);
}

#[test]
fn the_pr_bar_has_a_pill_per_reviewer_from_the_host() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.review_decision = "CHANGES_REQUESTED".into();
    rec.changes_at = Some("t1".into());
    rec.reviewers = vec![reviewer("ok", "approved"), reviewer("rev", "changes"), reviewer("new", "pending"), reviewer("gone", "changes")];
    rec.threads = vec![Thread { resolved: true, ..thread("1", "rev") }];
    fake(&b, rec);
    prflow::merge_flow(&b.app, id, vec![("swapped_off", json!(["gone"]))]).unwrap();
    poll(&b);
    let pills = |b: &Board| -> Vec<(String, String)> {
        let card = board::task_card(&b.app, &b.task(id)).unwrap();
        card["pr"]["bar"]["reviewer_rows"].as_array().unwrap().iter().map(|r| (r["user"].as_str().unwrap().to_string(), r["state"].as_str().unwrap().to_string())).collect()
    };
    let s = |v: &[(&str, &str)]| v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>();
    assert_eq!(pills(&b), s(&[("ok", "approved"), ("rev", "changes"), ("new", "waiting")]), "the swapped-off reviewer has no pill");
    let card = board::task_card(&b.app, &b.task(id)).unwrap();
    assert_eq!((card["pr"]["bar"]["approvals"].as_i64(), card["pr"]["bar"]["reviewers"].as_i64()), (Some(1), Some(3)));
    b.post(&format!("/tasks/T{id}/pr/addressed"), json!({}));
    assert_eq!(pills(&b), s(&[("ok", "approved"), ("rev", "rereview"), ("new", "waiting")]));
}

#[test]
fn a_stacked_pr_is_retargeted_through_its_host() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let h = fake(&b, green());
    taskboardd::propen::host::retarget(&b.app, &b.task(id), "develop").unwrap();
    assert!(h.calls().contains(&"retarget develop".to_string()), "a Bitbucket PR moves too");
}

#[test]
fn a_thread_waits_on_the_board_s_own_account_and_unread_tasks_hold_the_merge() {
    // #49: the board posts as "bot", not as the PR's author "me".
    let b = board_with(|c| c.pr.agents_merge = true);
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.viewer = "bot".into();
    rec.threads = vec![thread("1", "bot"), thread("2", "rev")];
    rec.reviewers = vec![reviewer("a", "approved"), reviewer("c", "approved")];
    rec.approvals = 2;
    rec.tasks_error = Some("Bitbucket answered 403".into());
    let h = fake(&b, rec);
    poll(&b);
    let card = board::pr_card(&b.task(id));
    assert_eq!(card["stage"]["open_threads"], 1, "the board's own account answered 1");
    b.post(&format!("/tasks/T{id}/pr/reply"), json!({"thread": "2", "text": "Done"}));
    assert_eq!(h.rec.lock().threads[1].last_author, "bot", "a reply is the board's account's");
    // #77: green and approved, but its tasks are unknown: the stage holds as the merge does, and
    // nobody is woken for a merge that would be refused.
    poll(&b);
    assert_eq!(b.phase(id), "review", "unread tasks hold the stage");
    assert!(!b.flow(id)["woke"].as_str().unwrap_or("").starts_with("merge"), "no wake to merge: {}", b.flow(id));
    let e =b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains("couldn't read its PR tasks (Bitbucket answered 403)"), "{e}");
    assert!(!e.contains("thread"), "{e}");
    let live = b.get(&format!("/tasks/T{id}/pr"), &[("full", "1")])["live"].clone();
    assert!(live["tasks_open"].is_null(), "unknown, not 0");
    assert_eq!(live["tasks_error"], "Bitbucket answered 403");
    h.rec.lock().tasks_error = None;
    b.post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true}));
    assert_eq!(b.phase(id), "merged");
}

#[test]
fn the_board_s_own_approval_doesn_t_count() {
    // #77: like the PR's author, the board's own account doesn't approve its PRs.
    let b = board_with(|c| c.pr.approvals = 2);
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.viewer = "bot".into();
    rec.reviewers = vec![reviewer("a", "approved"), reviewer("bot", "approved")];
    rec.approvals = 2;
    let h = fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "review", "one approval of the two it needs");
    assert_eq!(board::task_card(&b.app, &b.task(id)).unwrap()["pr"]["bar"]["approvals"], 1);
    h.rec.lock().reviewers.push(reviewer("c", "approved"));
    poll(&b);
    assert_eq!(b.phase(id), "merge");
}

#[test]
fn a_pr_needs_two_approvals_unless_its_project_is_set_otherwise_with_tb() {
    // #50: the board's default is 2 (tests otherwise run with the host's verdict).
    let b = board_with(|c| c.pr.approvals = taskboardd::config::DEFAULT_APPROVALS);
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.reviewers = vec![reviewer("a", "approved")];
    rec.approvals = 1;
    rec.review_decision = "APPROVED".into();
    fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "review", "one approval isn't enough, whatever the host says");
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({})).unwrap_err();
    assert!(e.contains("it has 1 of the 2 approvals it needs"), "{e}");
    let v = b.post("/projects/webapp", json!({"approvals": 1}));
    assert_eq!(v["pr_rules"]["approvals"], 1);
    assert_eq!(v["pr_rules"]["set"], json!({"approvals": 1}));
    assert_eq!(v["pr_flow"], "auto", "the PR flow is left alone");
    poll(&b);
    assert_eq!(b.phase(id), "merge");
    assert!(b.try_post("/projects/webapp", json!({"approvals": -1})).unwrap_err().contains("0 (the host's own decision) to 20"));
    let v = b.post("/projects/webapp", json!({"approvals": null}));
    assert_eq!(v["pr_rules"]["approvals"], 2, "back to the board's default");
    poll(&b);
    assert_eq!(b.phase(id), "review");
    assert!(b.try_post("/projects/webapp", json!({})).unwrap_err().contains("Say what to change"));
}

#[test]
fn an_expected_check_that_never_posts_stops_holding_the_merge_once_the_wait_is_over() {
    // #51: expected checks set with tb; after the wait only failed checks block the merge.
    let b = board_with(|c| c.pr.agents_merge = true);
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.reviewers = vec![reviewer("a", "approved")];
    fake(&b, rec);
    let v = b.post("/projects/webapp", json!({"expected": ["build", "E2E"], "expected_wait_mins": 30}));
    assert_eq!(v["pr_rules"]["expected"], json!(["build", "E2E"]));
    assert_eq!(v["pr_rules"]["expected_wait_mins"], 30.0);
    poll(&b);
    assert_eq!(b.phase(id), "checks", "e2e hasn't posted");
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains("expected checks haven't posted: E2E (it waits up to 30 min for them)"), "{e}");
    prflow::merge_flow(&b.app, id, vec![("head_at", json!({"h1": now_ts() - 31.0 * 60.0}))]).unwrap();
    poll(&b);
    assert_eq!(b.phase(id), "merge");
    let live = b.get(&format!("/tasks/T{id}/pr"), &[("full", "1")])["live"].clone();
    assert_eq!((live["expected_missing"].clone(), live["expected_waited_out"].clone()), (json!(["E2E"]), json!(true)));
    assert!(live["blockers"].as_array().unwrap().is_empty(), "{live}");
    b.post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true}));
    assert_eq!(b.phase(id), "merged");
    let v = b.post("/projects/webapp", json!({"expected": null, "expected_wait_mins": null}));
    assert!(v["pr_rules"]["expected"].is_null(), "back to config.toml's (none)");
    assert!(b.try_post("/projects/webapp", json!({"expected": [""]})).unwrap_err().contains("needs a name"));
}

#[test]
fn status_blames_the_base_per_test_and_says_how_to_rebase_and_what_was_replied() {
    // #52: the PR's run fails two tests, the base's runs of the same check only one.
    let b = board_with(|c| {
        c.pr.projects.insert(
            "webapp".into(),
            PrProject { failures_cmd: Some("echo 'Run e2e'; echo test: login_works; [ -n \"$TB_HEAD\" ] && echo test: logout_works; true".into()), ..Default::default() },
        );
    });
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.checks = vec![check("e2e", "failed")];
    let mut th = thread("1", "rev");
    th.replies = vec![
        taskboardd::prhost::Reply { author: "me".into(), author_name: "Me".into(), text: "Why rename it?".into(), at: "t2".into() },
        taskboardd::prhost::Reply { author: "rev".into(), author_name: "Rev".into(), text: "It clashes with the other one".into(), at: "t3".into() },
    ];
    rec.threads = vec![th];
    let h = fake(&b, rec);
    *h.base_checks.lock() = vec![Check { name: "e2e".into(), state: "failed".into(), url: Some("https://ci.example.com/base/1".into()) }];
    poll(&b);
    h.rec.lock().base_head = "b2".into();
    let live = b.get(&format!("/tasks/T{id}/pr"), &[("full", "1")])["live"].clone();
    let f = &live["failures"][0];
    assert_eq!(f["tests"], json!(["login_works", "logout_works"]));
    assert_eq!(f["base_tests"], json!(["login_works"]), "only the test the base fails too");
    assert_eq!((f["base_fails"].clone(), f["base_compared"].clone()), (json!(false), json!("steps")), "logout_works is this PR's");
    assert_eq!(live["rebase"][0], "git fetch origin main && git rebase origin/main");
    assert_eq!(live["open_threads"][0]["replies"][1]["text"], "It clashes with the other one");
}

#[test]
fn merging_moves_the_prs_stacked_on_it_onto_its_base_first() {
    // #50: the merge deletes the branch, so the PR into it is pointed at the base before.
    let b = board_with(|_| {});
    let parent = b.pr_task(BB);
    let mut rec = green();
    rec.branch = "base-work".into();
    rec.reviewers = vec![reviewer("a", "approved")];
    let h = fake(&b, rec);
    poll(&b);
    let child = b.pr_task("https://bitbucket.org/acme/webapp/pull-requests/10");
    let child_rec = Record { base: "base-work".into(), branch: "more-work".into(), head: "h9".into(), ..green() }.to_value();
    prflow::merge_flow(&b.app, child, vec![("rec", child_rec)]).unwrap();
    b.post(&format!("/tasks/T{parent}/pr/merge"), json!({}));
    let calls = h.calls();
    let at = |c: &str| calls.iter().position(|x| x.starts_with(c)).unwrap_or_else(|| panic!("{c} in {calls:?}"));
    assert!(at("retarget main") < at("merge "), "{calls:?}");
    assert_eq!(b.flow(child)["retargeted"], "main");
    assert_eq!(b.phase(parent), "merged");
    let said: Vec<String> = b.app.db.q("SELECT text FROM events WHERE task_id = ? ORDER BY id", taskboardd::p![child]).unwrap().iter().map(|r| r.st("text")).collect();
    assert!(said.iter().any(|l| l.contains("now goes into main")), "{said:?}");
}
