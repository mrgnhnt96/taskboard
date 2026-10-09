//! `tb step aim` (issue #78): the owner saves what a task's rounds look at, and later rounds and the gates
//! follow it, as the Python board's `tb review point` did.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, midna, reports, steps};

const REVIEW: &str = r#"
[[steps]]
name = "Author review"
prompt = "Run the review"
check = "true"
per_head = true
"#;

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn git(path: &Path, args: &[&str]) -> String {
    let o = Command::new("git").arg("-C").arg(path).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    std::fs::write(&cfg.config_path, REVIEW).unwrap();
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("README"), "x").unwrap();
    git(&repo, &["add", "README"]);
    git(&repo, &["commit", "-q", "-m", "First"]);
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    Board { app, dir }
}

impl Board {
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn repo(&self) -> std::path::PathBuf {
        self.dir.path().join("webapp")
    }
    fn new_task(&self) -> i64 {
        let body = json!({"title": "Add login", "project": "webapp"});
        let id = api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &body).unwrap()["id"].as_i64().unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")})).unwrap();
        id
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn steps(&self, head: &str) -> Value {
        let mut q = Query::new();
        q.insert("session".into(), "s1".into());
        q.insert("head".into(), head.into());
        api::dispatch(&self.app, "GET", "/steps", &q, &Value::Null).unwrap()
    }
}

#[test]
fn a_pinned_aim_is_what_rounds_and_gates_judge() {
    let b = new_board();
    let id = b.new_task();
    let (aimed, here) = ("a".repeat(40), "c".repeat(40));
    let v = b.report("tb.step_aim", json!({"branch": "main", "sha": aimed})).unwrap();
    let main = git(&b.repo(), &["rev-parse", "main"]);
    assert_eq!(v["aim"], json!({"branch": "main", "sha": aimed, "tip": main}), "the pin keeps its branch's tip");
    assert_eq!(v["head"], json!(aimed));
    // `GET /steps` hands the aim to `tb`, and judges the step on it, not on the caller's checkout.
    let s = b.steps(&here);
    assert_eq!(s["aim"]["sha"], json!(aimed));
    assert_eq!(s["head"], json!(aimed));
    b.report("tb.step", json!({"name": "Author review", "via": "done", "ok": true, "head": aimed})).unwrap();
    assert_eq!(b.steps(&here)["steps"][0]["done"], true);
    // The gates (tb done, opening the PR) judge the aimed commit too.
    assert!(steps::missing_at(&b.app, &b.row(id), &[steps::Before::Pr], Some(&here)).unwrap().is_empty());
    assert!(b.row(id).st("context").contains("step_aim"));
    // --clear drops it: the caller's checkout counts again.
    let v = b.report("tb.step_aim", json!({"clear": true})).unwrap();
    assert!(v["aim"].is_null());
    assert_eq!(b.steps(&here)["steps"][0]["done"], false);
    assert!(b.steps(&here)["aim"].is_null());
}

#[test]
fn a_branch_or_worktree_aim_follows_its_latest_commit() {
    let b = new_board();
    let id = b.new_task();
    let repo = b.repo();
    git(&repo, &["branch", "feat"]);
    b.report("tb.step_aim", json!({"branch": "feat"})).unwrap();
    let tip = git(&repo, &["rev-parse", "feat"]);
    assert_eq!(steps::aim_head(&b.row(id)), Some(tip.clone()));
    let wt = b.dir.path().join("feat-wt");
    git(&repo, &["worktree", "add", "-q", &wt.to_string_lossy(), "feat"]);
    std::fs::write(wt.join("more"), "x").unwrap();
    git(&wt, &["add", "more"]);
    git(&wt, &["commit", "-q", "-m", "More"]);
    let moved = git(&wt, &["rev-parse", "HEAD"]);
    assert_eq!(steps::aim_head(&b.row(id)), Some(moved.clone()), "a branch aim follows the branch as it moves");
    b.report("tb.step_aim", json!({"worktree": wt.to_string_lossy()})).unwrap();
    assert_eq!(steps::aim_head(&b.row(id)), Some(moved));
    assert_eq!(steps::saved_aim(&b.row(id)), json!({"worktree": wt.to_string_lossy()}), "a new aim replaces the old");
}

#[test]
fn a_pin_is_dropped_once_its_branch_moves_on() {
    let b = new_board();
    let id = b.new_task();
    let repo = b.repo();
    std::fs::write(repo.join("two"), "x").unwrap();
    git(&repo, &["add", "two"]);
    git(&repo, &["commit", "-q", "-m", "Two"]);
    git(&repo, &["checkout", "-q", "-b", "feat/a"]);
    let pinned = git(&repo, &["rev-parse", "HEAD~1"]);
    let tip = git(&repo, &["rev-parse", "HEAD"]);
    b.report("tb.step_aim", json!({"branch": "feat/a", "sha": pinned, "tip": tip})).unwrap();
    b.report("tb.step", json!({"name": "Author review", "via": "done", "ok": true, "head": pinned})).unwrap();
    // While the branch stays where it was pinned, the pin holds, and `{branch}` is the aimed branch.
    assert_eq!(steps::aim_head(&b.row(id)), Some(pinned.clone()));
    assert!(steps::missing(&b.app, &b.row(id), &[steps::Before::Pr]).unwrap().is_empty());
    assert_eq!(steps::vars(&b.row(id))["branch"], "feat/a", "prompts follow the aim");
    // New work on the branch: the pin no longer counts, the gate judges the new tip, and rounds follow it.
    std::fs::write(repo.join("three"), "x").unwrap();
    git(&repo, &["add", "three"]);
    git(&repo, &["commit", "-q", "-m", "Three"]);
    let moved = git(&repo, &["rev-parse", "HEAD"]);
    let s = b.steps(&pinned);
    assert_eq!(s["aim"], json!({"branch": "feat/a", "dropped": pinned}));
    assert_eq!(s["head"], json!(moved));
    assert_eq!(s["steps"][0]["done"], false, "tb done can't pass on a commit that's no longer the branch's tip");
    assert_eq!(steps::missing(&b.app, &b.row(id), &[steps::Before::Pr]).unwrap().len(), 1);
    // The next round's report drops it from the task, with a line saying so.
    b.report("tb.step", json!({"name": "Author review", "via": "done", "ok": true, "head": moved})).unwrap();
    assert_eq!(steps::saved_aim(&b.row(id)), json!({"branch": "feat/a"}));
    let logged = b.app.db.q("SELECT text FROM events WHERE task_id = ? AND text LIKE 'Rounds no longer pinned%'", taskboardd::p![id]).unwrap();
    assert_eq!(logged.len(), 1);
    assert!(steps::missing(&b.app, &b.row(id), &[steps::Before::Pr]).unwrap().is_empty());
}

#[test]
fn the_board_keeps_the_tip_when_tb_leaves_it_out() {
    let b = new_board();
    let id = b.new_task();
    let main = git(&b.repo(), &["rev-parse", "main"]);
    b.report("tb.step_aim", json!({"branch": "main", "sha": main, "tip": ""})).unwrap();
    assert_eq!(steps::saved_aim(&b.row(id))["tip"], json!(main));
    b.report("tb.step_aim", json!({"branch": "main", "tip": main})).unwrap();
    assert_eq!(steps::saved_aim(&b.row(id)), json!({"branch": "main"}), "a tip without a pin isn't kept");
}

#[test]
fn a_publish_is_recorded_on_the_task() {
    let b = new_board();
    let id = b.new_task();
    let (code, why) = b.report("tb.step_publish", json!({"name": "Author review", "ok": true, "head": "a".repeat(40)})).unwrap_err();
    assert_eq!(code, 400, "{why}");
    assert!(why.contains("nothing to publish"), "{why}");
    std::fs::write(&b.app.cfg.config_path, format!("{REVIEW}publish = \"wd review publish {{repo}} {{pr}}\"\n")).unwrap();
    let v = b.report("tb.step_publish", json!({"name": "Author review", "ok": true, "head": "a".repeat(40)})).unwrap();
    assert_eq!(v["passed"], true);
    let logged = b.app.db.q("SELECT text FROM events WHERE task_id = ? AND text LIKE 'Published%'", taskboardd::p![id]).unwrap();
    assert_eq!(logged[0].st("text"), format!("Published Author review for {}", "a".repeat(12)));
}
