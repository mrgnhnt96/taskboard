//! A per-head step's pass across a rebase (#1145): a clean rebase that leaves the task's change as it was
//! (the same `git patch-id`) keeps the pass; new work, or a rebase that had to resolve a conflict in the
//! change's own lines, sets the step to do again.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::Row;
use taskboardd::{board, midna, reports, steps};

const PARITY: &str = r#"
[[steps]]
name = "Design parity"
prompt = "Check each screen against its design"
check = "true"
per_head = true
"#;

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
    repo: PathBuf,
}

fn git(path: &Path, args: &[&str]) -> String {
    let o = Command::new("git").arg("-C").arg(path).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

/// A board with the step, and a clone of a bare origin whose main has a README.
fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    std::fs::write(&cfg.config_path, PARITY).unwrap();
    let origin = dir.path().join("origin.git");
    let repo = dir.path().join("webapp");
    git(dir.path(), &["init", "-q", "--bare", "-b", "main", &origin.to_string_lossy()]);
    git(dir.path(), &["clone", "-q", &origin.to_string_lossy(), &repo.to_string_lossy()]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["switch", "-q", "-c", "main"]);
    std::fs::write(repo.join("README"), "webapp\n\none\ntwo\nthree\nfour\nfive\nsix\n").unwrap();
    git(&repo, &["add", "README"]);
    git(&repo, &["commit", "-q", "-m", "First"]);
    git(&repo, &["push", "-q", "origin", "main"]);
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    Board { app, dir, repo }
}

impl Board {
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": self.repo.to_string_lossy(), "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    /// A task taken in s1, on a branch whose one commit adds a line at the end of the README.
    fn task(&self) -> i64 {
        let body = json!({"title": "Add login", "project": "webapp"});
        let id = api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &body).unwrap()["id"].as_i64().unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")})).unwrap();
        git(&self.repo, &["switch", "-q", "-c", "feat/login"]);
        self.write("README", "webapp\n\none\ntwo\nthree\nfour\nfive\nsix\nlogin\n");
        git(&self.repo, &["commit", "-q", "-am", "Login"]);
        id
    }
    fn write(&self, file: &str, text: &str) {
        std::fs::write(self.repo.join(file), text).unwrap();
        git(&self.repo, &["add", file]);
    }
    fn head(&self) -> String {
        git(&self.repo, &["rev-parse", "HEAD"])
    }
    fn pass(&self, head: &str) {
        self.report("tb.step", json!({"name": "Design parity", "via": "done", "ok": true, "head": head})).unwrap();
    }
    /// Lands `text` in `file` on origin's main from another clone, as another PR merging would.
    fn land(&self, file: &str, text: &str) {
        let other = self.dir.path().join(format!("other-{}", rand_name()));
        let origin = self.dir.path().join("origin.git");
        git(self.dir.path(), &["clone", "-q", "-b", "main", &origin.to_string_lossy(), &other.to_string_lossy()]);
        git(&other, &["config", "user.email", "test@example.com"]);
        git(&other, &["config", "user.name", "Test"]);
        std::fs::write(other.join(file), text).unwrap();
        git(&other, &["add", file]);
        git(&other, &["commit", "-q", "-m", file]);
        git(&other, &["push", "-q", "origin", "main"]);
    }
    fn rebase(&self) {
        git(&self.repo, &["fetch", "-q", "origin"]);
        git(&self.repo, &["rebase", "-q", "origin/main"]);
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn missing(&self, id: i64, head: &str) -> Vec<String> {
        steps::missing_at(&self.app, &self.row(id), &[steps::Before::Pr], Some(head)).unwrap().into_iter().map(|s| s.name).collect()
    }
    fn steps(&self, head: &str) -> Value {
        let mut q = Query::new();
        q.insert("session".into(), "s1".into());
        q.insert("head".into(), head.into());
        api::dispatch(&self.app, "GET", "/steps", &q, &Value::Null).unwrap()
    }
}

fn rand_name() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64
}

#[test]
fn a_clean_rebase_that_leaves_the_change_as_it_was_keeps_the_pass() {
    let b = new_board();
    let id = b.task();
    let passed = b.head();
    b.pass(&passed);
    b.land("other.txt", "landed\n");
    b.land("more.txt", "landed too\n");
    b.rebase();
    let now = b.head();
    assert_ne!(now, passed);

    assert!(b.missing(id, &now).is_empty(), "{:?}", b.missing(id, &now));
    let s = b.steps(&now);
    assert_eq!(s["steps"][0]["done"], true, "{s}");
    let r = &s["passed_rounds"]["Design parity"];
    assert_eq!((r["head"].as_str(), r["kept_from"].as_str(), r["stale"].as_bool()), (Some(now.as_str()), Some(passed.as_str()), Some(false)), "{r}");
}

#[test]
fn new_work_after_the_pass_sets_the_step_to_do_again() {
    let b = new_board();
    let id = b.task();
    b.pass(&b.head());
    b.land("other.txt", "landed\n");
    b.rebase();
    b.write("login.txt", "more\n");
    git(&b.repo, &["commit", "-q", "-m", "More login"]);

    let now = b.head();
    assert_eq!(b.missing(id, &now), vec!["Design parity".to_string()]);
    assert_eq!(b.steps(&now)["steps"][0]["done"], false);
    assert!(b.steps(&now)["passed_rounds"]["Design parity"].is_null());
}

#[test]
fn a_rebase_that_resolves_a_conflict_in_the_change_sets_the_step_to_do_again() {
    let b = new_board();
    let id = b.task();
    b.pass(&b.head());
    // Main's last line changes under the task's added line: the rebase stops on a conflict.
    b.land("README", "webapp\n\none\ntwo\nthree\nfour\nfive\nsix (moved)\n");
    git(&b.repo, &["fetch", "-q", "origin"]);
    let o = Command::new("git").arg("-C").arg(&b.repo).args(["rebase", "-q", "origin/main"]).output().unwrap();
    assert!(!o.status.success(), "expected a conflict");
    b.write("README", "webapp\n\none\ntwo\nthree\nfour\nfive\nsix (moved)\nlogin\n");
    let o = Command::new("git").arg("-C").arg(&b.repo).args(["rebase", "--continue"]).env("GIT_EDITOR", "true").output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

    let now = b.head();
    assert_eq!(b.missing(id, &now), vec!["Design parity".to_string()]);
    assert_eq!(b.steps(&now)["steps"][0]["done"], false);
}
