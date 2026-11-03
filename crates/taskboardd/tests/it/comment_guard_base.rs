//! #865: with no PR on record yet, the comment guard judges a task's branch against the branch it
//! started from or goes into (its PR base, else its slot's or goal's base), not the default branch.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().expect("git runs");
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, text: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", file]);
}

fn clone(origin: &Path, to: &Path) {
    git(origin.parent().unwrap(), &["clone", "-q", &origin.to_string_lossy(), &to.to_string_lossy()]);
    git(to, &["config", "user.email", "t@example.com"]);
    git(to, &["config", "user.name", "T"]);
}

impl Board {
    fn new(dir: tempfile::TempDir) -> Board {
        let mut cfg = Config::for_tests(dir.path());
        cfg.comments.guard = true;
        let b = Board { app: App::for_tests(cfg), dir };
        b.app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": b.repo().to_string_lossy()}]).to_string())).unwrap();
        let s = json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": b.repo().to_string_lossy(), "status": {"state": "working"}});
        midna::sync(&b.app, &[s], &[]).unwrap();
        b
    }
    fn repo(&self) -> PathBuf {
        self.dir.path().join("webapp")
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": self.repo().to_string_lossy()});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn take(&self, extra: Value) -> i64 {
        let mut body = json!({"title": "Backport the fix", "detail": "Do it.", "project": "webapp", "ships_pr": false});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        let id = self.post("/tasks", body)["id"].as_i64().unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")})).unwrap();
        id
    }
    fn done(&self) -> Result<Value, (u16, String)> {
        self.report("tb.done", json!({"summary": "Done"}))
    }
}

/// `release/x` forks from `main` and adds a commented file; `main` moves on after. `webapp` is cloned
/// before the release branch is pushed when `late` is set, so git there doesn't have it yet.
fn diverged(late: bool) -> (Board, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    git(dir.path(), &["init", "-q", "--bare", "-b", "main", &origin.to_string_lossy()]);
    let up = dir.path().join("upstream");
    clone(&origin, &up);
    commit(&up, "a.rs", "fn a() {}\n");
    git(&up, &["push", "-q", "-u", "origin", "main"]);
    let repo = dir.path().join("webapp");
    if late {
        clone(&origin, &repo);
    }
    git(&up, &["switch", "-q", "-c", "release/x"]);
    commit(&up, "release.rs", "// a release note\nfn release() {}\n");
    git(&up, &["push", "-q", "-u", "origin", "release/x"]);
    git(&up, &["switch", "-q", "main"]);
    commit(&up, "main.rs", "fn main() {}\n");
    git(&up, &["push", "-q", "origin", "main"]);
    if !late {
        clone(&origin, &repo);
    }
    (Board::new(dir), repo)
}

#[test]
fn a_task_detached_at_its_goals_base_with_nothing_past_it_passes_the_guard() {
    let (b, repo) = diverged(false);
    git(&repo, &["checkout", "-q", "--detach", "origin/release/x"]);
    let g = b.post("/goals", json!({"name": "Release fixes", "project": "webapp"}))["id"].as_i64().unwrap();
    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/release/x"}));
    let id = b.take(json!({"goal_id": g}));

    std::fs::write(repo.join("a.rs"), "// mine\nfn a() {}\n").unwrap();
    let (code, msg) = b.done().unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("a.rs:1") && !msg.contains("release.rs"), "only the task's own comment counts: {msg}");

    git(&repo, &["checkout", "-q", "--", "a.rs"]);
    assert_eq!(git(&repo, &["log", "--oneline", "origin/release/x..HEAD"]), "");
    b.done().expect("nothing past the goal's base");
    assert_eq!(board::get_task(&b.app, id).unwrap().s("status"), Some("done"));
}

#[test]
fn a_branch_off_its_pr_base_is_judged_against_it_fetching_it_when_git_lacks_it() {
    let (b, repo) = diverged(true);
    git(&repo, &["fetch", "-q", "origin", "release/x"]);
    git(&repo, &["switch", "-q", "-c", "fix", "FETCH_HEAD"]);
    git(&repo, &["update-ref", "-d", "refs/remotes/origin/release/x"]);
    commit(&repo, "fix.rs", "// why\nfn fix() {}\n");
    let id = b.take(json!({"pr_base": "release/x"}));

    let (code, msg) = b.done().unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("adds a code comment") && msg.contains("fix.rs:1") && !msg.contains("release.rs"), "{msg}");

    commit(&repo, "fix.rs", "fn fix() {}\n");
    b.done().expect("the branch adds no comments over release/x");
    assert_eq!(board::get_task(&b.app, id).unwrap().s("status"), Some("done"));
}
