//! A task's own commits record the files they changed, so a task that ends with no PR still has code
//! areas for lesson picking; its PR's files replace them once the PR is read.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{midna, relevance, reports};

fn git(at: &Path, args: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(at).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

impl Board {
    fn new() -> Board {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("webapp");
        std::fs::create_dir_all(&home).unwrap();
        git(&home, &["init", "-q"]);
        git(&home, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let app = App::for_tests(Config::for_tests(dir.path()));
        app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": home.to_string_lossy()}]).to_string())).unwrap();
        Board { app, dir }
    }
    fn home(&self) -> std::path::PathBuf {
        self.dir.path().join("webapp")
    }
    fn report(&self, event: &str, extra: Value) {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": self.home().to_string_lossy(), "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{}", e.message));
    }
    fn taken(&self) -> i64 {
        let body = json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"});
        let id = api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &body).unwrap()["id"].as_i64().unwrap();
        let home = self.home().to_string_lossy().into_owned();
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": home, "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")}));
        id
    }
    /// Writes the files and commits them, as the agent would; the commit's output.
    fn commit(&self, files: &[&str], args: &[&str]) -> String {
        for f in files {
            let p = self.home().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, format!("{f}\n")).unwrap();
            git(&self.home(), &["add", f]);
        }
        git(&self.home(), &[&["commit"], args].concat())
    }
}

#[test]
fn a_tasks_commits_record_the_files_they_changed() {
    let b = Board::new();
    let id = b.taken();
    let out = b.commit(&["src/db.rs", "src/api.rs"], &["-m", "fix: one"]);
    b.report("hook.commit", json!({"command": "git commit -m 'fix: one'", "output": out}));
    assert_eq!(relevance::paths_of(&b.app, id).unwrap(), ["src/api.rs", "src/db.rs"]);

    // A quiet commit prints nothing: the checkout's head is the commit.
    b.commit(&["docs/x.md"], &["-q", "-m", "docs"]);
    let head = git(&b.home(), &["rev-parse", "HEAD"]).trim().to_string();
    b.report("hook.commit", json!({"command": "git commit -q -m docs", "output": "", "git": {"sha": head}}));
    assert_eq!(relevance::paths_of(&b.app, id).unwrap(), ["docs/x.md", "src/api.rs", "src/db.rs"]);

    // A commit that didn't happen adds nothing, even with the head another commit made.
    std::fs::write(b.home().join("other.rs"), "x").unwrap();
    git(&b.home(), &["add", "other.rs"]);
    git(&b.home(), &["commit", "-q", "-m", "someone else's"]);
    let head = git(&b.home(), &["rev-parse", "HEAD"]).trim().to_string();
    b.report("hook.commit", json!({"command": "git commit -m again", "output": "nothing to commit, working tree clean", "git": {"sha": head}}));
    assert_eq!(relevance::paths_of(&b.app, id).unwrap().len(), 3);

    // The PR's files, once read, are the task's.
    relevance::record(&b.app, id, "abc123", &["src/db.rs".into()]).unwrap();
    assert_eq!(relevance::paths_of(&b.app, id).unwrap(), ["src/db.rs"]);
}
