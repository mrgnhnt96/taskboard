//! A hook that shows a task's work in another repository marks the task, logs it and alerts the owner
//! once; work back in the project (a worktree of its repo counts) or the task ending clears it.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, dispatch, midna, p, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn git(at: &Path, args: &[&str]) {
    let ok = Command::new("git").arg("-C").arg(at).args(args).output().unwrap().status.success();
    assert!(ok, "git {args:?}");
}

fn repo(at: &Path) {
    std::fs::create_dir_all(at).unwrap();
    git(at, &["init", "-q"]);
    git(at, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "first"]);
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let home = dir.path().join("webapp");
    repo(&home);
    repo(&dir.path().join("other"));
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": home.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    fn path(&self, p: &str) -> String {
        self.dir.path().join(p).to_string_lossy().into_owned()
    }
    fn report(&self, event: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{}", e.message))
    }
    fn taken(&self) -> i64 {
        let body = json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"});
        let id = api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &body).unwrap_or_else(|e| panic!("{}", e.message))["id"].as_i64().unwrap();
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": self.path("webapp"), "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")}));
        id
    }
    fn outside(&self, id: i64) -> Value {
        board::task_context(&board::get_task(&self.app, id).unwrap()).get("outside").cloned().unwrap_or(Value::Null)
    }
    fn alerts(&self, id: i64) -> usize {
        dispatch::alerts(&self.app).iter().filter(|a| a["key"] == format!("outside:T{id}")).count()
    }
    fn logged(&self, id: i64) -> Vec<String> {
        self.app.db.q("SELECT text FROM events WHERE task_id = ? AND kind = 'outside'", p![id]).unwrap().iter().map(|e| e.st("text")).collect()
    }
}

#[test]
fn work_in_another_repo_marks_the_task_once_and_clears_back_home() {
    let b = board();
    let id = b.taken();
    b.report("hook.stop", json!({"cwd": b.path("other")}));
    let top = std::fs::canonicalize(b.path("other")).unwrap().to_string_lossy().into_owned();
    assert_eq!(b.outside(id)["path"], json!(top));
    assert_eq!(b.alerts(id), 1);
    b.report("hook.stop", json!({"cwd": b.path("other")}));
    assert_eq!(b.logged(id).len(), 1, "{:?}", b.logged(id));
    assert_eq!(b.alerts(id), 1);

    b.report("hook.stop", json!({"cwd": b.path("webapp")}));
    assert!(b.outside(id).is_null());
    assert_eq!(b.alerts(id), 0);
    assert_eq!(b.logged(id).last().map(String::as_str), Some("Back in webapp"));
}

#[test]
fn a_git_dash_c_commit_into_another_repo_counts_and_done_clears_it() {
    let b = board();
    let id = b.taken();
    let cmd = format!("git -C {} commit -m x", b.path("other"));
    b.report("hook.commit", json!({"cwd": b.path("webapp"), "command": cmd}));
    assert!(b.outside(id).is_object());
    assert_eq!(b.alerts(id), 1);

    let t = board::get_task(&b.app, id).unwrap();
    reports::finish_task(&b.app, &t, "test", "Done", false, None).unwrap();
    assert!(b.outside(id).is_null());
    assert_eq!(b.alerts(id), 0);
}

#[test]
fn a_worktree_of_the_projects_repo_is_home() {
    let b = board();
    let id = b.taken();
    git(&b.dir.path().join("webapp"), &["worktree", "add", "-q", &b.path("webapp-t1")]);
    b.report("hook.stop", json!({"cwd": b.path("webapp-t1")}));
    assert!(b.outside(id).is_null());
    assert_eq!(b.alerts(id), 0);
}
