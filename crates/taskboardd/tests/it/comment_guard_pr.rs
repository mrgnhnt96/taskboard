//! #795: the comment guard on `tb pr wait` judges the PR's own diff, merge-base(base, head)..head,
//! whatever branch the task's old checkout is on now. #920, #921: with the base at its current tip.
//! #962: still so when the board can't fetch the base and the host's base SHA predates the move.

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, FakeHost, Record};
use taskboardd::util::ApiError;
use taskboardd::{midna, p, prflow, reports};

const PR: &str = "https://bitbucket.org/acme/webapp/pull-requests/9";

struct Board {
    app: Arc<App>,
    host: Arc<FakeHost>,
    dir: tempfile::TempDir,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().expect("git runs");
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, text: &str) -> String {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", file]);
    git(dir, &["rev-parse", "HEAD"])
}

fn clone(origin: &Path, to: &Path) {
    git(origin.parent().unwrap(), &["clone", "-q", &origin.to_string_lossy(), &to.to_string_lossy()]);
    git(to, &["config", "user.email", "t@example.com"]);
    git(to, &["config", "user.name", "T"]);
}

impl Board {
    fn repo(&self) -> std::path::PathBuf {
        self.dir.path().join("webapp")
    }
    fn report(&self, event: &str, extra: Value) {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": ""});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message));
    }
    fn pr_wait(&self, id: i64) -> Result<Value, ApiError> {
        api::dispatch(&self.app, "POST", &format!("/tasks/{id}/pr/wait"), &Query::new(), &json!({"by_session": "s1"}))
    }
    fn pr_head(&self, head: &str, base_head: &str) {
        self.pr_into("main", head, base_head);
    }
    fn pr_into(&self, base: &str, head: &str, base_head: &str) {
        *self.host.rec.lock() =
            Record { state: "OPEN".into(), title: "Add login".into(), author: "me".into(), head: head.into(), branch: "feat".into(), base: base.into(), base_head: base_head.into(), ..Default::default() };
        prflow::refresh(&self.app).unwrap();
    }
}

fn board(dir: tempfile::TempDir) -> Board {
    let repo = dir.path().join("webapp");
    let mut cfg = Config::for_tests(dir.path());
    cfg.comments.guard = true;
    cfg.pr.watch = true;
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    let host = FakeHost::new("bitbucket", Record::default());
    prhost::install(&app, host.clone());
    midna::sync(&app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "idle"}})], &[]).unwrap();
    Board { app, host, dir }
}

/// `main` gets a comment before `feat` is cut and another after; `feat` adds none. The PR is handed in.
fn handed_in() -> (Board, i64, String) {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    git(dir.path(), &["init", "-q", "--bare", "-b", "main", &origin.to_string_lossy()]);
    let repo = dir.path().join("webapp");
    clone(&origin, &repo);
    commit(&repo, "a.rs", "fn a() {}\n");
    commit(&repo, "old.rs", "// an old base note\nfn old() {}\n");
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(&repo, &["switch", "-q", "-c", "feat"]);
    let head = commit(&repo, "b.rs", "fn b() {}\n");
    git(&repo, &["push", "-q", "-u", "origin", "feat"]);
    git(&repo, &["switch", "-q", "main"]);
    let base_head = commit(&repo, "new.rs", "// a new base note\nfn new() {}\n");
    git(&repo, &["push", "-q", "origin", "main"]);
    git(&repo, &["switch", "-q", "feat"]);

    let b = board(dir);
    let id = b.hand_in();
    b.pr_head(&head, &base_head);
    (b, id, base_head)
}

impl Board {
    fn hand_in(&self) -> i64 {
        let id = api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &json!({"title": "Add login", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")}));
        self.report("tb.done", json!({"summary": "Done", "pr": PR}));
        id
    }
}

/// #920, #921: `feat`'s PR goes into `parent`, which is then rebased onto a `main` that gained a
/// comment. `feat` follows with `rebase --onto` and pushes; the host still has both old SHAs.
fn stacked_then_base_moved() -> (Board, i64, String) {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    git(dir.path(), &["init", "-q", "--bare", "-b", "main", &origin.to_string_lossy()]);
    let repo = dir.path().join("webapp");
    clone(&origin, &repo);
    commit(&repo, "a.rs", "fn a() {}\n");
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(&repo, &["switch", "-q", "-c", "parent"]);
    let old_parent = commit(&repo, "parent.rs", "fn parent() {}\n");
    git(&repo, &["push", "-q", "-u", "origin", "parent"]);
    git(&repo, &["switch", "-q", "-c", "feat"]);
    let old_head = commit(&repo, "b.rs", "fn b() {}\n");
    git(&repo, &["push", "-q", "-u", "origin", "feat"]);

    let b = board(dir);
    let id = b.hand_in();
    b.pr_into("parent", &old_head, &old_parent);

    git(&repo, &["switch", "-q", "main"]);
    commit(&repo, "new.rs", "// a new base note\nfn new() {}\n");
    git(&repo, &["push", "-q", "origin", "main"]);
    git(&repo, &["switch", "-q", "parent"]);
    git(&repo, &["rebase", "-q", "main"]);
    git(&repo, &["push", "-q", "-f", "origin", "parent"]);
    git(&repo, &["switch", "-q", "feat"]);
    git(&repo, &["rebase", "-q", "--onto", "origin/parent", &old_parent, "feat"]);
    git(&repo, &["push", "-q", "-f", "origin", "feat"]);
    (b, id, old_parent)
}

#[test]
fn tb_pr_wait_judges_the_pr_not_the_branch_its_old_checkout_is_on_now() {
    let (b, id, _) = handed_in();
    // Another task took the checkout: an older branch with a comment of its own, left uncommitted too.
    git(&b.repo(), &["switch", "-q", "-c", "other", "HEAD~2"]);
    commit(&b.repo(), "o.rs", "// someone else's note\nfn o() {}\n");
    std::fs::write(b.repo().join("a.rs"), "// an uncommitted note\nfn a() {}\n").unwrap();

    b.pr_wait(id).unwrap_or_else(|e| panic!("the PR adds no comments: {}", e.message));
}

#[test]
fn tb_pr_wait_refuses_a_comment_the_pr_adds_even_when_pushed_from_another_checkout() {
    let (b, id, base_head) = handed_in();
    let elsewhere = b.dir.path().join("elsewhere");
    clone(&b.dir.path().join("origin.git"), &elsewhere);
    git(&elsewhere, &["switch", "-q", "feat"]);
    let head = commit(&elsewhere, "b.rs", "// explains b\nfn b() {}\n");
    git(&elsewhere, &["push", "-q", "origin", "feat"]);
    git(&b.repo(), &["switch", "-q", "--detach", "main"]);
    b.pr_head(&head, &base_head);

    let e = b.pr_wait(id).unwrap_err();
    assert_eq!(e.status, 409);
    assert!(e.message.contains("this PR adds a code comment") && e.message.contains("b.rs:1") && e.message.contains("explains b"), "{}", e.message);
    assert!(!e.message.contains("base note"), "the base branch's own comments aren't the PR's: {}", e.message);
}

#[test]
fn tb_pr_wait_judges_a_stacked_pr_against_its_bases_current_tip_after_the_base_moves() {
    let (b, id, _) = stacked_then_base_moved();
    b.pr_wait(id).unwrap_or_else(|e| panic!("the PR's own lines add no comments: {}", e.message));

    git(&b.repo(), &["switch", "-q", "--detach", "main"]);
    b.pr_wait(id).unwrap_or_else(|e| panic!("judged from another checkout too: {}", e.message));
}

#[test]
fn tb_pr_wait_still_refuses_a_stacked_prs_own_comment_after_the_base_moves() {
    let (b, id, _) = stacked_then_base_moved();
    commit(&b.repo(), "b.rs", "// explains b\nfn b() {}\n");
    git(&b.repo(), &["push", "-q", "origin", "feat"]);
    git(&b.repo(), &["switch", "-q", "--detach", "main"]);

    let e = b.pr_wait(id).unwrap_err();
    assert_eq!(e.status, 409);
    assert!(e.message.contains("this PR adds a code comment") && e.message.contains("b.rs:1"), "{}", e.message);
    assert!(!e.message.contains("base note"), "main's comment came in with the base, not the PR: {}", e.message);
}

/// The board's daemon can't reach origin: the fetch fails, leaving what the checkout already has.
fn origin_unreachable(b: &Board) {
    let gone = b.dir.path().join("gone.git");
    git(&b.repo(), &["remote", "set-url", "origin", &gone.to_string_lossy()]);
}

#[test]
fn tb_pr_wait_judges_a_moved_base_at_the_tip_the_checkout_has_when_the_fetch_fails() {
    let (b, id, _) = stacked_then_base_moved();
    origin_unreachable(&b);

    b.pr_wait(id).unwrap_or_else(|e| panic!("the host's base SHA predates the move, so main's comment isn't the PR's: {}", e.message));
}

#[test]
fn tb_pr_wait_judges_a_moved_base_at_the_head_the_board_saw_on_its_parent() {
    let (b, id, old_parent) = stacked_then_base_moved();
    let new_parent = git(&b.repo(), &["rev-parse", "origin/parent"]);
    git(&b.repo(), &["update-ref", "refs/remotes/origin/parent", &old_parent]);
    origin_unreachable(&b);
    let parent = api::dispatch(&b.app, "POST", "/tasks", &Query::new(), &json!({"title": "Parent", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    let flow = json!({"head": new_parent, "rec": {"branch": "parent", "head": new_parent}}).to_string();
    b.app.db.x("UPDATE tasks SET status = 'done', pr_state = 'OPEN', pr_flow = ? WHERE id = ?", p![flow, parent]).unwrap();
    b.app.db.x("UPDATE tasks SET waits_for = ? WHERE id = ?", p![json!([parent]).to_string(), id]).unwrap();

    b.pr_wait(id).unwrap_or_else(|e| panic!("the board saw parent move, so main's comment isn't the PR's: {}", e.message));
}
