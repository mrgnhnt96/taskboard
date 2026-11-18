//! `tb submit --pr <link>` for a PR that merged before the branch's last commits were pushed: the
//! board refuses it, so those commits don't go unshipped behind a done task.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, FakeHost, Record};
use taskboardd::util::{ApiError, RowExt};
use taskboardd::{board, midna, reports};

const PR: &str = "https://github.com/acme/webapp/pull/9";

fn git(dir: &Path, args: &[&str]) -> String {
    let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().expect("git runs");
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str) -> String {
    std::fs::write(dir.join(file), format!("// {file}\n")).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", file]);
    git(dir, &["rev-parse", "HEAD"])
}

struct Board {
    app: Arc<App>,
    host: Arc<FakeHost>,
    _dir: tempfile::TempDir,
    repo: PathBuf,
    id: i64,
}

impl Board {
    fn report(&self, event: &str, extra: Value) -> Result<Value, ApiError> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": self.repo.to_string_lossy()});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false)
    }
    fn pr(&self, state: &str, head: &str) {
        *self.host.rec.lock() = Record { state: state.into(), head: head.into(), branch: "feat".into(), base: "main".into(), ..Default::default() };
    }
    fn submit(&self) -> Result<Value, ApiError> {
        self.report("tb.done", json!({"summary": "Done", "pr": PR}))
    }
    fn task(&self) -> taskboardd::util::Row {
        board::get_task(&self.app, self.id).unwrap()
    }
}

/// `main` with one commit, and `feat` on top of it with `a.rs` (the PR's head), pushed; the task is
/// taken in that checkout.
fn on_feat() -> (Board, String) {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    git(dir.path(), &["init", "-q", "--bare", "-b", "main", &origin.to_string_lossy()]);
    let repo = dir.path().join("webapp");
    git(dir.path(), &["clone", "-q", &origin.to_string_lossy(), &repo.to_string_lossy()]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "T"]);
    commit(&repo, "base.rs");
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(&repo, &["switch", "-q", "-c", "feat"]);
    let head = commit(&repo, "a.rs");
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
    b.report("tb.take", json!({"task": format!("T{id}")})).unwrap();
    (b, head)
}

#[test]
fn a_pr_merged_before_the_branchs_last_commit_is_refused() {
    let (b, head) = on_feat();
    let late = commit(&b.repo, "late.rs");
    git(&b.repo, &["push", "-q", "origin", "feat"]);
    b.pr("MERGED", &head);

    let e = b.submit().expect_err("the late commit would go unshipped");
    assert_eq!(e.status, 409);
    assert!(e.message.contains("PR #9 merged at"), "{}", e.message);
    assert!(e.message.contains(&late[..7]), "names the commit left out: {}", e.message);
    assert!(e.message.contains("--pr-body"), "says how to ship it: {}", e.message);
    let t = b.task();
    assert_ne!(t.s("status"), Some("done"));
    assert!(t.s("pr_url").is_none_or(|u| u.is_empty()), "the PR isn't linked");
}

#[test]
fn an_unpushed_commit_after_the_merged_head_is_refused_too() {
    let (b, head) = on_feat();
    commit(&b.repo, "late.rs");
    b.pr("MERGED", &head);

    assert_eq!(b.submit().expect_err("the commit would go unshipped").status, 409);
}

#[test]
fn a_pr_merged_at_the_branchs_head_finishes_the_task() {
    let (b, head) = on_feat();
    b.pr("MERGED", &head);

    b.submit().unwrap();
    let t = b.task();
    assert_eq!(t.s("status"), Some("done"));
    assert_eq!(t.s("pr_url"), Some(PR));
}

#[test]
fn an_open_pr_with_later_commits_finishes_the_task() {
    let (b, head) = on_feat();
    commit(&b.repo, "late.rs");
    b.pr("OPEN", &head);

    b.submit().unwrap();
    assert_eq!(b.task().s("status"), Some("done"));
}

#[test]
fn a_checkout_moved_on_to_another_branch_finishes_the_task() {
    let (b, head) = on_feat();
    git(&b.repo, &["switch", "-q", "-c", "next"]);
    commit(&b.repo, "next.rs");
    b.pr("MERGED", &head);

    b.submit().unwrap();
    assert_eq!(b.task().s("status"), Some("done"));
}

#[test]
fn a_branch_that_pulled_the_merged_base_finishes_the_task() {
    let (b, head) = on_feat();
    git(&b.repo, &["switch", "-q", "main"]);
    git(&b.repo, &["merge", "-q", "--no-ff", "--no-edit", "feat"]);
    commit(&b.repo, "later_on_main.rs");
    git(&b.repo, &["push", "-q", "origin", "main"]);
    git(&b.repo, &["switch", "-q", "feat"]);
    git(&b.repo, &["merge", "-q", "--ff-only", "main"]);
    b.pr("MERGED", &head);

    b.submit().unwrap();
    assert_eq!(b.task().s("status"), Some("done"));
}
