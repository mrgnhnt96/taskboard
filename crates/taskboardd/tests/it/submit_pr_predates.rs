//! `tb submit --pr <link>` only links a PR that was open before the task started: one the host says
//! was opened later is refused from the agent's terminal (the board opens the task's own PR).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, FakeHost, Record};
use taskboardd::util::{ApiError, RowExt};
use taskboardd::{board, fields, midna, reports};

const PR: &str = "https://github.com/acme/webapp/pull/9";
const BEFORE: &str = "2000-01-01T00:00:00Z";
const AFTER: &str = "2999-01-01T00:00:00Z";

fn git(dir: &Path, args: &[&str]) {
    let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().expect("git runs");
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
}

struct Board {
    app: Arc<App>,
    host: Arc<FakeHost>,
    _dir: tempfile::TempDir,
    repo: PathBuf,
    id: i64,
}

impl Board {
    fn report(&self, extra: Value) -> Result<Value, ApiError> {
        let mut b = json!({"session": "s1", "claude_session": "c-s1", "cwd": self.repo.to_string_lossy()});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false)
    }
    fn pr_opened(&self, at: &str) {
        *self.host.rec.lock() = Record { state: "OPEN".into(), branch: "feat".into(), base: "main".into(), created_at: at.into(), ..Default::default() };
    }
    fn submit(&self) -> Result<Value, ApiError> {
        self.report(json!({"event": "tb.done", "summary": "Done", "pr": PR}))
    }
    fn task(&self) -> taskboardd::util::Row {
        board::get_task(&self.app, self.id).unwrap()
    }
}

/// A pushed `feat` branch in `webapp`, and a task taken in that checkout from terminal `s1`.
fn taken() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    git(dir.path(), &["init", "-q", "--bare", "-b", "main", &origin.to_string_lossy()]);
    let repo = dir.path().join("webapp");
    git(dir.path(), &["clone", "-q", &origin.to_string_lossy(), &repo.to_string_lossy()]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("a.rs"), "// a\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "a"]);
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(&repo, &["switch", "-q", "-c", "feat"]);
    git(&repo, &["push", "-q", "-u", "origin", "feat"]);

    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    let host = FakeHost::new("github", Record::default());
    prhost::install(&app, host.clone());
    midna::sync(&app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "idle"}})], &[]).unwrap();
    let id = api::dispatch(&app, "POST", "/tasks", &Query::new(), &json!({"title": "Add login", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    let b = Board { app, host, _dir: dir, repo, id };
    b.report(json!({"event": "tb.take", "task": format!("T{id}")})).unwrap();
    b
}

#[test]
fn a_pr_opened_after_the_task_started_is_refused() {
    let b = taken();
    b.pr_opened(AFTER);

    let e = b.submit().expect_err("the PR was opened during the task");
    assert_eq!(e.status, 409);
    assert!(e.message.contains("PR #9 was opened"), "{}", e.message);
    assert!(e.message.contains("gh pr close 9 -R acme/webapp"), "says how to close it: {}", e.message);
    assert!(e.message.contains("--pr-body"), "says how the board opens it: {}", e.message);
    let t = b.task();
    assert_ne!(t.s("status"), Some("done"));
    assert!(t.s("pr_url").is_none_or(|u| u.is_empty()), "the PR isn't linked");
}

#[test]
fn a_pr_link_in_the_summary_opened_after_the_task_started_is_refused() {
    let b = taken();
    b.pr_opened(AFTER);

    let e = b.report(json!({"event": "tb.done", "summary": format!("Done in {PR}")})).expect_err("the PR was opened during the task");
    assert_eq!(e.status, 409);
}

#[test]
fn a_pr_open_before_the_task_started_is_linked() {
    let b = taken();
    b.pr_opened(BEFORE);

    b.submit().unwrap();
    let t = b.task();
    assert_eq!(t.s("status"), Some("done"));
    assert_eq!(t.s("pr_url"), Some(PR));
}

#[test]
fn a_host_that_doesnt_say_when_the_pr_opened_links_it() {
    let b = taken();
    b.pr_opened("");

    b.submit().unwrap();
    assert_eq!(b.task().s("pr_url"), Some(PR));
}

#[test]
fn a_pr_another_task_holds_is_not_checked() {
    let b = taken();
    b.pr_opened(AFTER);
    let other = api::dispatch(&b.app, "POST", "/tasks", &Query::new(), &json!({"title": "Add signup", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    board::update_task(&b.app, other, fields!["pr_host" => "github", "pr_repo" => "acme/webapp", "pr_num" => 9, "pr_url" => PR, "pr_state" => "OPEN"]).unwrap();

    let got = b.submit();
    assert!(got.as_ref().err().is_none_or(|e| !e.message.contains("was opened")), "{got:?}");
}

#[test]
fn the_owner_linking_from_elsewhere_is_not_refused() {
    let b = taken();
    b.pr_opened(AFTER);

    let body = json!({"event": "tb.done", "task": format!("T{}", b.id), "summary": "Done", "pr": PR, "cwd": b.repo.to_string_lossy()});
    reports::handle(&b.app, body, false).unwrap();
    let t = b.task();
    assert_eq!(t.s("status"), Some("done"));
    assert_eq!(t.s("pr_url"), Some(PR));
}
