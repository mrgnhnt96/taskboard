//! A PR closed without merging says where its work went (#1098): `tb pr close` takes a reason or what
//! replaced it and posts it; a close outside the board takes the comment made with it, else the board
//! checks the branch against its base with `git cherry`, comments when the work is there and tells the
//! owner when it isn't.

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, Check, FakeHost, Record, Thread};
use taskboardd::util::RowExt;
use taskboardd::{board, dispatch, midna, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let o = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, text: &str, msg: &str) -> String {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"])
}

/// A board with project webapp: a clone of a bare origin with `main` and a pushed branch `feat` one
/// commit ahead; `feat`'s head.
fn board() -> (Board, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    let origin = dir.path().join("origin.git");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q", "--bare"]);
    git(dir.path(), &["clone", "-q", origin.to_str().unwrap(), "webapp"]);
    let repo = dir.path().join("webapp");
    commit(&repo, "base.txt", "base\n", "Base");
    git(&repo, &["push", "-q", "origin", "HEAD:main"]);
    git(&repo, &["switch", "-q", "-c", "feat"]);
    let head = commit(&repo, "a.txt", "a\n", "Add a");
    git(&repo, &["push", "-q", "origin", "feat"]);
    git(&repo, &["switch", "-q", "main"]);
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    (Board { app, dir }, head)
}

fn record(state: &str, head: &str) -> Record {
    Record {
        state: state.into(),
        author: "me".into(),
        head: head.into(),
        branch: "feat".into(),
        base: "main".into(),
        checks: vec![Check { name: "ci".into(), state: "passed".into(), url: None, at: None }],
        ..Default::default()
    }
}

impl Board {
    fn repo(&self) -> std::path::PathBuf {
        self.dir.path().join("webapp")
    }
    fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| e.message)
    }
    fn report(&self, event: &str, session: &str, extra: Value) {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap();
    }
    /// A done task with GitHub PR #9 from `feat`, read open once.
    fn pr_task(&self, head: &str) -> i64 {
        let id = self.post("/tasks", json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
        let repo = self.repo().to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": "s9", "name": "Term s9", "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", "s9", json!({"task": format!("T{id}")}));
        self.report("tb.done", "s9", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9"}));
        self.read(id, &record("OPEN", head));
        id
    }
    fn read(&self, id: i64, rec: &Record) {
        let t = board::get_task(&self.app, id).unwrap();
        self.app.db.tx(|| prflow::step(&self.app, &t, &rec.to_value())).unwrap();
    }
    fn card(&self, id: i64) -> Value {
        board::pr_card(&self.app, &board::get_task(&self.app, id).unwrap())
    }
    fn alert(&self, id: i64) -> Option<String> {
        dispatch::alerts(&self.app).into_iter().find(|a| a["task_id"] == json!(id) && a["text"].as_str().is_some_and(|t| t.contains("closed unmerged"))).map(|a| a["text"].as_str().unwrap().to_string())
    }
}

#[test]
fn tb_pr_close_needs_a_reason_or_what_replaced_it_and_posts_it() {
    let (b, head) = board();
    let host = FakeHost::new("github", record("OPEN", &head));
    prhost::install(&b.app, host.clone());
    let id = b.pr_task(&head);
    let e = b.post(&format!("/tasks/T{id}/pr/close"), json!({})).unwrap_err();
    assert!(e.contains("--reason") && e.contains("--superseded-by"), "{e}");
    let e = b.post(&format!("/tasks/T{id}/pr/close"), json!({"superseded_by": "the other one"})).unwrap_err();
    assert!(e.contains("--superseded-by takes"), "{e}");
    assert!(!host.calls().iter().any(|c| c.starts_with("close")), "{:?}", host.calls());

    let v = b.post(&format!("/tasks/T{id}/pr/close"), json!({"superseded_by": "https://github.com/acme/webapp/pull/14", "reason": "Smaller"})).unwrap();
    assert!(host.calls().contains(&"close: Closed without merging: superseded by #14. Smaller".to_string()), "{:?}", host.calls());
    assert_eq!(v["pr"]["closed"]["line"], "Superseded by #14: Smaller");
    assert_eq!(v["pr"]["closed"]["superseded_by"], "#14");

    b.read(id, &record("CLOSED", &head));
    assert_eq!(b.card(id)["closed"]["superseded_by"], "#14", "the poll keeps what tb pr close said");
    assert!(!host.calls().iter().any(|c| c.starts_with("comment")), "{:?}", host.calls());
    assert!(b.alert(id).is_none());
}

#[test]
fn a_close_on_the_host_with_a_comment_keeps_that_comment() {
    let (b, head) = board();
    let host = FakeHost::new("github", record("OPEN", &head));
    prhost::install(&b.app, host.clone());
    let id = b.pr_task(&head);
    let mut closed = record("CLOSED", &head);
    closed.threads = vec![Thread {
        id: "c1".into(),
        kind: "comment".into(),
        author_name: "Morgan".into(),
        last_at: "2099-01-01T00:00:00Z".into(),
        text: "Done in #20 instead".into(),
        ..Default::default()
    }];
    b.read(id, &closed);
    assert_eq!(b.card(id)["closed"]["line"], "Closed: Done in #20 instead");
    assert!(!host.calls().iter().any(|c| c.starts_with("comment")), "{:?}", host.calls());
    assert!(b.alert(id).is_none());
}

#[test]
fn a_silent_close_whose_work_is_on_the_base_gets_a_comment_naming_the_commits() {
    let (b, head) = board();
    let host = FakeHost::new("github", record("OPEN", &head));
    prhost::install(&b.app, host.clone());
    let id = b.pr_task(&head);
    let repo = b.repo();
    git(&repo, &["cherry-pick", &head]);
    let landed = git(&repo, &["rev-parse", "--short=7", "HEAD"]);
    assert_ne!(landed, head[..7], "patch-equivalent, not the same commit");
    git(&repo, &["push", "-q", "origin", "main"]);
    git(&repo, &["reset", "-q", "--hard", "HEAD~1"]);

    b.read(id, &record("CLOSED", &head));
    let comment = host.calls().into_iter().find(|c| c.starts_with("comment")).expect("the board comments");
    assert!(comment.contains("already on `main`") && comment.contains(&format!("- {landed} Add a")), "{comment}");
    let card = b.card(id);
    assert_eq!(card["closed"]["line"], format!("Its work is on main in {landed}"));
    assert_eq!(card["closed"]["on_base"], true);
    assert!(b.alert(id).is_none(), "nothing for the owner");
}

#[test]
fn a_silent_close_whose_work_is_not_on_the_base_tells_the_owner() {
    let (b, head) = board();
    let host = FakeHost::new("github", record("OPEN", &head));
    prhost::install(&b.app, host.clone());
    let id = b.pr_task(&head);

    b.read(id, &record("CLOSED", &head));
    let alert = b.alert(id).expect("the owner hears");
    assert!(alert.contains(&format!("T{id}'s PR #9 closed unmerged; its work is not on main")), "{alert}");
    assert!(!host.calls().iter().any(|c| c.starts_with("comment")), "{:?}", host.calls());
    assert_eq!(b.card(id)["closed"]["line"], "Closed unmerged; its work is not on main");
    let t = board::get_task(&b.app, id).unwrap();
    assert_eq!(t.s("pr_phase"), Some("declined"));
}
