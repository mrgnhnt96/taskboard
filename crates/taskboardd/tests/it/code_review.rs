//! The code review stage: with a reviewer set, `tb submit` waits while the board runs it on the task's
//! worktree (docs/REVIEW-PROTOCOL.md), records the round, has a triage agent sort any findings, then carries
//! the submit on to its PR.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, code_review, dispatch, midna, p, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
    repo: PathBuf,
}

const BODY: &str = "## Summary\nAdds passkey sign-in.\n\n## Changes\n- New sign-in screen\n\n## Testing\n- Unit tests\n";

/// A reviewer that saves what it was given under `seen/` in the test folder, then writes `result`.
const PASS: &str = r#"{"protocol":1,"reviewer":"fake","verdict":"pass","headline":"Looks fine","findings":[],"cost":{"usd":0.25}}"#;
const FAIL: &str = r#"{"protocol":1,"reviewer":"fake","verdict":"fail","headline":"One thing","findings":[{"id":"F1","title":"Off by one","severity":"major","file":"a.txt","line":1,"detail":"why","url":null}]}"#;

fn reviewer(dir: &Path, result: &str) -> String {
    let seen = dir.join("seen");
    std::fs::create_dir_all(&seen).unwrap();
    let script = dir.join("reviewer.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nset -e\nS='{seen}'\nenv | grep '^TASKBOARD_' | sort > \"$S/env-$TASKBOARD_ROUND\"\ncp \"$TASKBOARD_INTENT\" \"$S/intent-$TASKBOARD_ROUND\"\n\
             [ -n \"$TASKBOARD_PREVIOUS\" ] && cp \"$TASKBOARD_PREVIOUS\" \"$S/previous-$TASKBOARD_ROUND\"\n\
             echo \"round $TASKBOARD_ROUND\" >> \"$TASKBOARD_STATE_DIR/kept\"\ncp \"$TASKBOARD_STATE_DIR/kept\" \"$S/kept-$TASKBOARD_ROUND\"\n\
             echo reading the diff >&2\ncat > \"$TASKBOARD_RESULT\" <<'EOF'\n{result}\nEOF\n",
            seen = seen.display()
        ),
    )
    .unwrap();
    format!("sh '{}'", script.display())
}

/// A board whose reviewer is `command`, given the test folder.
fn new_board(command: impl FnOnce(&Path) -> Option<String>) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.gh = fake_gh(dir.path()).to_string_lossy().to_string();
    cfg.pr_body.require_ticket = Some(false);
    cfg.code_review.command = command(dir.path());
    let repo = git_repo(dir.path());
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    Board { app, dir, repo }
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap()
    }
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": self.repo.to_string_lossy(), "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    /// A task taken in terminal s1, on a branch with one commit.
    fn task(&self) -> i64 {
        let id = self.post("/tasks", json!({"title": "Add passkeys", "detail": "Sign in with a passkey.", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": self.repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")})).unwrap();
        git(&self.repo, &["switch", "-q", "-c", "feat/passkeys"]);
        self.commit("a.txt", "a\n");
        id
    }
    fn commit(&self, file: &str, text: &str) {
        std::fs::write(self.repo.join(file), text).unwrap();
        git(&self.repo, &["add", file]);
        git(&self.repo, &["commit", "-q", "-m", file]);
    }
    fn push(&self) {
        git(&self.repo, &["push", "-q", "-u", "origin", "feat/passkeys"]);
    }
    fn done(&self) -> Result<Value, (u16, String)> {
        self.report("tb.done", json!({"summary": "Passkeys", "pr_body": BODY}))
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn seen(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.path().join("seen").join(name)).unwrap_or_default()
    }
    fn alerts(&self) -> Vec<String> {
        dispatch::alerts(&self.app).iter().filter_map(|a| a["text"].as_str().map(str::to_string)).collect()
    }
    /// Marks a finding of the task's round in triage, as the triage agent's terminal.
    fn triage(&self, id: i64, finding: &str, outcome: &str, reason: &str) -> Result<Value, (u16, String)> {
        self.post(&format!("/tasks/T{id}/code-review/triage"), json!({"finding": finding, "outcome": outcome, "reason": reason, "session": "s-triage"}))
    }
    fn stage(&self, id: i64) -> Value {
        self.get(&format!("tasks/T{id}"))["code_review"].clone()
    }
    fn pr_opened(&self) -> bool {
        std::fs::read_to_string(self.dir.path().join("gh.log")).is_ok_and(|l| l.contains("pr create"))
    }
}

fn git(path: &Path, args: &[&str]) -> String {
    let o = Command::new("git").arg("-C").arg(path).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn git_repo(dir: &Path) -> PathBuf {
    let origin = dir.join("origin.git");
    let repo = dir.join("webapp");
    git(dir, &["init", "-q", "--bare", "-b", "main", &origin.to_string_lossy()]);
    git(dir, &["clone", "-q", &origin.to_string_lossy(), &repo.to_string_lossy()]);
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

fn fake_gh(dir: &Path) -> PathBuf {
    let gh = dir.join("gh");
    let log = dir.join("gh.log");
    std::fs::write(&gh, format!("#!/bin/sh\necho \"$@\" >> '{}'\ncat > /dev/null\necho https://github.com/acme/webapp/pull/77\n", log.display())).unwrap();
    Command::new("chmod").arg("+x").arg(&gh).status().unwrap();
    gh
}

#[test]
fn a_submit_waits_in_code_review_then_opens_its_pr() {
    let b = new_board(|d| Some(reviewer(d, PASS)));
    let id = b.task();
    b.push();

    let v = b.done().unwrap();
    assert_eq!(v["code_review"], true, "{v}");
    assert!(v["message"].as_str().unwrap().contains("opens the PR"), "{v}");
    let t = b.row(id);
    assert_ne!(t.st("status"), "done");
    assert!(code_review::in_review(&t));
    assert!(!b.pr_opened());
    assert_eq!(b.get(&format!("tasks/T{id}"))["code_review"]["state"], "queued");
    let (code, why) = b.done().unwrap_err();
    assert_eq!(code, 409);
    assert!(why.contains("already in code review"), "{why}");

    code_review::sweep(&b.app).unwrap();
    let rounds = code_review::rounds(&b.app, id).unwrap();
    assert_eq!(rounds.len(), 1, "{rounds:?}");
    let r = &rounds[0];
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["round"], 1);
    assert_eq!(r["head"], git(&b.repo, &["rev-parse", "HEAD"]));
    assert_eq!(r["tree"], git(&b.repo, &["rev-parse", "HEAD^{tree}"]));
    assert_eq!(r["result"]["verdict"], "pass");
    assert_eq!(r["reviewer"], "fake");
    assert_eq!(r["cost"]["usd"], 0.25);
    assert!(r["output"].as_str().unwrap().contains("reading the diff"), "{r}");

    let env = b.seen("env-1");
    for want in [
        "TASKBOARD_REVIEW_PROTOCOL=1".to_string(),
        "TASKBOARD_ROUND=1".into(),
        format!("TASKBOARD_TASK=T{id}"),
        "TASKBOARD_BRANCH=feat/passkeys".into(),
        "TASKBOARD_BASE=origin/main".into(),
        format!("TASKBOARD_HEAD={}", r["head"].as_str().unwrap()),
    ] {
        assert!(env.lines().any(|l| l == want), "{want} in\n{env}");
    }
    assert!(env.contains("TASKBOARD_REPO=") && env.contains("TASKBOARD_STATE_DIR=") && env.contains("TASKBOARD_RESULT="), "{env}");
    assert!(!env.contains("TASKBOARD_PREVIOUS="), "{env}");
    let intent = b.seen("intent-1");
    assert!(intent.contains("Add passkeys") && intent.contains("Sign in with a passkey."), "{intent}");

    let t = b.row(id);
    assert_eq!(t.st("status"), "done");
    assert_eq!(t.i("pr_num"), Some(77));
    assert!(!code_review::in_review(&t));
    let card = b.get(&format!("tasks/T{id}"))["code_review"].clone();
    assert_eq!(card["state"], Value::Null, "{card}");
    assert_eq!(card["last"]["verdict"], "pass", "{card}");
    // The task is done, so its review folder goes.
    assert!(!code_review::task_dir(&b.app, id).exists());
}

#[test]
fn a_submit_turned_back_after_its_review_tells_the_agent_and_skips_review_when_unchanged() {
    let b = new_board(|d| Some(reviewer(d, FAIL)));
    let id = b.task();

    assert_eq!(b.done().unwrap()["code_review"], true);
    code_review::sweep(&b.app).unwrap();
    assert_eq!(b.stage(id)["state"], "triaging");
    b.triage(id, "F1", "dismiss", "Not from this change").unwrap();
    code_review::sweep(&b.app).unwrap();
    let t = b.row(id);
    assert_ne!(t.st("status"), "done");
    assert!(!code_review::in_review(&t));
    assert!(b.alerts().iter().any(|a| a.contains(&format!("T{id}: tb submit was turned back")) && a.contains("isn't pushed")), "{:?}", b.alerts());
    let job = b.app.db.q1("SELECT args FROM jobs WHERE task_id = ? AND purpose = 'code_review'", p![id]).unwrap().expect("a message for the agent");
    assert!(job.st("args").contains("isn't pushed"), "{}", job.st("args"));

    // Pushed as it was reviewed: straight on to the PR, no second round.
    b.push();
    let v = b.done().unwrap();
    assert_ne!(v["code_review"], true, "{v}");
    assert_eq!(code_review::rounds(&b.app, id).unwrap().len(), 1);
    assert_eq!(b.row(id).i("pr_num"), Some(77));
}

#[test]
fn changed_code_gets_another_round_with_the_last_one_and_the_kept_state() {
    let b = new_board(|d| Some(reviewer(d, FAIL)));
    let id = b.task();

    b.done().unwrap();
    code_review::sweep(&b.app).unwrap();
    b.triage(id, "F1", "fix", "The change counts pages wrong").unwrap();
    code_review::sweep(&b.app).unwrap();
    b.commit("b.txt", "b\n");
    b.push();
    assert_eq!(b.done().unwrap()["code_review"], true);
    code_review::sweep(&b.app).unwrap();
    b.triage(id, "F1", "dismiss", "Fixed in the last round").unwrap();
    code_review::sweep(&b.app).unwrap();

    let rounds = code_review::rounds(&b.app, id).unwrap();
    assert_eq!(rounds.iter().map(|r| r["round"].as_u64().unwrap()).collect::<Vec<_>>(), [1, 2]);
    assert!(b.seen("env-2").lines().any(|l| l == "TASKBOARD_ROUND=2"));
    let prev: Value = serde_json::from_str(&b.seen("previous-2")).unwrap();
    assert_eq!(prev["protocol"], 1);
    assert_eq!(prev["verdict"], "fail");
    assert_eq!(prev["findings"][0]["id"], "F1");
    assert_eq!(prev["findings"][0]["outcome"], "fix");
    assert_eq!(prev["findings"][0]["reason"], "The change counts pages wrong");
    assert_eq!(b.seen("kept-2"), "round 1\nround 2\n");
    assert_eq!(b.row(id).i("pr_num"), Some(77));
}

#[test]
fn a_reviewer_that_fails_twice_lets_the_task_carry_on_and_alerts_the_owner() {
    let b = new_board(|_| Some("echo boom >&2; exit 3".into()));
    let id = b.task();
    b.push();

    b.done().unwrap();
    code_review::sweep(&b.app).unwrap();
    let rounds = code_review::rounds(&b.app, id).unwrap();
    assert_eq!(rounds.len(), 2, "{rounds:?}");
    for (i, r) in rounds.iter().enumerate() {
        assert_eq!(r["ok"], false);
        assert_eq!(r["try"], i as u64 + 1);
        assert!(r["error"].as_str().unwrap().contains('3'), "{r}");
        assert!(r["output"].as_str().unwrap().contains("boom"), "{r}");
    }
    assert!(b.alerts().iter().any(|a| a.contains(&format!("T{id}'s code review failed twice"))), "{:?}", b.alerts());
    assert_eq!(b.row(id).st("status"), "done");
    assert_eq!(b.row(id).i("pr_num"), Some(77));
}

#[test]
fn a_reviewer_without_a_result_is_an_error() {
    let b = new_board(|_| Some("true".into()));
    let id = b.task();
    b.push();
    b.done().unwrap();
    code_review::sweep(&b.app).unwrap();
    let rounds = code_review::rounds(&b.app, id).unwrap();
    assert_eq!(rounds.len(), 2);
    assert!(rounds[0]["error"].as_str().unwrap().contains("no result"), "{}", rounds[0]);
    assert_eq!(b.row(id).i("pr_num"), Some(77));
}

#[test]
fn without_a_reviewer_a_submit_opens_its_pr_at_once() {
    let b = new_board(|_| None);
    let id = b.task();
    b.push();
    let v = b.done().unwrap();
    assert_ne!(v["code_review"], true, "{v}");
    assert_eq!(b.row(id).i("pr_num"), Some(77));
    assert!(code_review::rounds(&b.app, id).unwrap().is_empty());
    assert_eq!(b.get(&format!("tasks/T{id}"))["code_review"], Value::Null);
}

#[test]
fn a_project_can_turn_its_reviewer_off_or_name_its_own() {
    let b = new_board(|_| Some("exit 1".into()));
    let project = |b: &Board| b.get("projects")["projects"].as_array().unwrap().iter().find(|p| p["name"] == "webapp").unwrap().clone();
    assert_eq!(project(&b)["code_review"], "exit 1");
    assert_eq!(project(&b)["code_review_set"], Value::Null);

    let (code, _) = b.post("/projects/webapp", json!({"code_review": "a\nb"})).unwrap_err();
    assert_eq!(code, 400);
    b.post("/projects/webapp", json!({"code_review": "my-reviewer --strict"})).unwrap();
    assert_eq!(project(&b)["code_review"], "my-reviewer --strict");
    b.post("/projects/webapp", json!({"code_review": "default"})).unwrap();
    assert_eq!(project(&b)["code_review"], "exit 1");
    b.post("/projects/webapp", json!({"code_review": "none"})).unwrap();
    assert_eq!(project(&b)["code_review"], Value::Null);
    assert_eq!(project(&b)["code_review_set"], "");

    let id = b.task();
    b.push();
    assert_ne!(b.done().unwrap()["code_review"], true);
    assert_eq!(b.row(id).i("pr_num"), Some(77));
}

const TWO: &str = r#"{"protocol":1,"reviewer":"fake","verdict":"fail","headline":"Two things","findings":[{"id":"F1","title":"Off by one","severity":"major","file":"a.txt","line":1,"detail":"why"},{"id":"F2","title":"Old helper leaks","severity":"minor","file":"README","detail":"It was like this before"}]}"#;

#[test]
fn findings_go_to_a_triage_agent_that_marks_each_then_the_task_carries_on() {
    let b = new_board(|d| Some(reviewer(d, TWO)));
    let g = b.post("/goals", json!({"name": "Passkeys", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    let id = b.task();
    b.app.db.x("UPDATE tasks SET goal_id = ? WHERE id = ?", p![g, id]).unwrap();
    b.push();
    b.done().unwrap();
    let (code, _) = b.triage(id, "F1", "fix", "Too soon").unwrap_err();
    assert_eq!(code, 409, "not triaging yet");

    code_review::sweep(&b.app).unwrap();
    let stage = b.stage(id);
    assert_eq!(stage["state"], "triaging", "{stage}");
    assert_eq!(stage["triage"], json!({"findings": 2, "triaged": 0}), "{stage}");
    let job = b.app.db.q1("SELECT * FROM jobs WHERE purpose = 'code_review_triage'", p![]).unwrap().expect("a triage agent");
    assert_eq!(job.st("kind"), "agent");
    assert_eq!(job.i("task_id"), None, "the triage agent isn't the task's terminal");
    let target: Value = serde_json::from_str(&job.st("target")).unwrap();
    assert_eq!((target["task"].as_i64(), target["round"].as_u64()), (Some(id), Some(1)));
    let args: Value = serde_json::from_str(&job.st("args")).unwrap();
    let prompt = args["prompt"].as_str().unwrap();
    for want in ["Add passkeys", "Sign in with a passkey.", "F1", "Off by one", "F2", "a.txt", &format!("tb finding T{id}")] {
        assert!(prompt.contains(want), "{want} in {prompt}");
    }
    assert!(args["flags"].as_str().unwrap().contains("--disallowedTools"), "{args}");
    // Still waiting while the agent works.
    code_review::sweep(&b.app).unwrap();
    assert_eq!(b.stage(id)["state"], "triaging");
    assert!(!b.pr_opened());

    let state = b.get(&format!("tasks/T{id}/code-review/triage"));
    assert_eq!(state["triaging"], true, "{state}");
    assert_eq!(state["left"], json!(["F1", "F2"]), "{state}");

    let (code, why) = b.post(&format!("/tasks/T{id}/code-review/triage"), json!({"finding": "F1", "outcome": "fix", "reason": "Mine", "session": "s1"})).unwrap_err();
    assert_eq!(code, 403, "{why}");
    assert_eq!(b.triage(id, "F1", "fix", " ").unwrap_err().0, 400);
    assert_eq!(b.triage(id, "F1", "fix", "a\nb").unwrap_err().0, 400);
    assert_eq!(b.triage(id, "F1", "maybe", "Unsure").unwrap_err().0, 400);
    assert_eq!(b.triage(id, "F9", "fix", "Mine").unwrap_err().0, 404);

    let v = b.triage(id, "f1", "fix", "The change counts one page too many").unwrap();
    assert!(v["message"].as_str().unwrap().contains("Left: F2"), "{v}");
    assert_eq!(b.triage(id, "F1", "dismiss", "Changed my mind").unwrap_err().0, 409);
    assert_eq!(b.stage(id)["triage"]["triaged"], 1);

    let v = b.triage(id, "F2", "backlog", "The helper leaked before this change").unwrap();
    assert!(v["message"].as_str().unwrap().contains("last one"), "{v}");
    let issue = b.app.db.q1("SELECT * FROM issues WHERE title = 'Old helper leaks'", p![]).unwrap().expect("a backlog item");
    assert_eq!(issue.i("goal_id"), Some(g));
    assert_eq!(issue.st("project"), "webapp");
    assert_eq!(issue.st("kind"), "follow");
    assert_eq!(issue.i("found_by_task"), Some(id));
    assert!(issue.st("detail").contains("The helper leaked before this change"), "{}", issue.st("detail"));
    let state = b.get(&format!("tasks/T{id}/code-review/triage"));
    assert_eq!(state["findings"][1]["issue"], format!("B{}", issue.id()), "{state}");

    code_review::sweep(&b.app).unwrap();
    let t = b.row(id);
    assert!(!code_review::in_review(&t));
    assert_eq!(t.i("pr_num"), Some(77));
    let r = &code_review::rounds(&b.app, id).unwrap()[0];
    assert_eq!(r["triage"]["F1"]["outcome"], "fix", "{r}");
    assert_eq!(r["triage"]["F2"]["outcome"], "backlog", "{r}");
    let log = b.app.db.q1("SELECT text FROM events WHERE task_id = ? AND kind = 'status' AND text LIKE '%triaged%'", p![id]).unwrap().unwrap();
    assert_eq!(log.st("text"), "Code review round 1 triaged: 1 fix, 1 backlog");
    assert_eq!(b.get(&format!("tasks/T{id}/code-review/triage"))["triaging"], false);
}

#[test]
fn a_triage_agent_that_cannot_open_lets_the_task_carry_on_and_alerts_the_owner() {
    let b = new_board(|d| Some(reviewer(d, FAIL)));
    let id = b.task();
    b.push();
    b.done().unwrap();
    code_review::sweep(&b.app).unwrap();
    b.app.db.x("UPDATE jobs SET state = 'failed', result = ? WHERE purpose = 'code_review_triage'", p![json!({"stderr": "no claude"}).to_string()]).unwrap();

    code_review::sweep(&b.app).unwrap();
    assert_eq!(b.row(id).i("pr_num"), Some(77));
    assert!(
        b.alerts().iter().any(|a| a.contains(&format!("T{id}'s code review round 1: its triage agent couldn't open (no claude) with 1 of its findings left (F1)"))),
        "{:?}",
        b.alerts()
    );
}
