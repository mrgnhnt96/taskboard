//! The board's lens sweep: it runs `lens sweep` for each project lens is on for, keeps the latest
//! run's PR and digest on the project, alerts on a new sweep PR and turns an escalated blocker into
//! one new task.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{dispatch, lens_sweep, p};

struct Board {
    app: Arc<App>,
    repo: PathBuf,
    calls: PathBuf,
    _dir: tempfile::TempDir,
}

fn git(repo: &Path, args: &[&str]) {
    let o = Command::new("git").arg("-C").arg(repo).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
}

/// A board with one project, `webapp`, and a fake lens whose `sweep` prints `out` (with `$REPO` for
/// the folder it was given) and exits with `code`.
fn board(out: &str, code: i32, remote: bool) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    if remote {
        git(&repo, &["remote", "add", "origin", "https://github.com/o/webapp.git"]);
    }
    let calls = dir.path().join("calls");
    let bin = dir.path().join("lens");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\nREPO=\"$3\"\ncat <<EOF\n{out}\nEOF\necho 'Error: lens broke' >&2\nexit {code}\n",
            calls.display()
        ),
    )
    .unwrap();
    Command::new("chmod").arg("+x").arg(&bin).status().unwrap();
    cfg.lens.bin = bin.to_string_lossy().to_string();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    Board { app, repo, calls, _dir: dir }
}

impl Board {
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.calls).unwrap_or_default().lines().map(String::from).collect()
    }
    fn project(&self) -> Value {
        let projects = api::dispatch(&self.app, "GET", "/projects", &Query::new(), &json!({})).unwrap();
        projects["projects"].as_array().unwrap().iter().find(|p| p["name"] == "webapp").unwrap().clone()
    }
    fn tasks(&self) -> Vec<taskboardd::util::Row> {
        self.app.db.q("SELECT * FROM tasks WHERE project = ?", p!["webapp"]).unwrap()
    }
    /// A sweep report with one fixed and one escalated blocker, and the escalated one's ledger.
    fn report(&self) {
        let review = self.repo.join(".review");
        std::fs::create_dir_all(review.join("sweep-1/fix")).unwrap();
        std::fs::write(
            review.join("sweep-1/fix/F2.json"),
            json!({"status": "escalated", "escalation": {"reason": "the fix needs a schema change", "needs": "a migration"}}).to_string(),
        )
        .unwrap();
        let report = json!({
            "pass": "sweep",
            "pr": "https://github.com/o/webapp/pull/9",
            "findings": [
                {"id": "F1", "severity": "blocker", "file": "a.rs", "line": 3, "title": "off by one", "fix": {"status": "resolved"}},
                {"id": "F2", "severity": "blocker", "file": "db.rs", "line": 40, "title": "lost write",
                 "failureScenario": "two saves at once drop one", "fix": {"status": "escalated", "ledger": ".review/sweep-1/fix/F2.json"}},
            ],
        });
        std::fs::write(review.join("sweep-1.json"), report.to_string()).unwrap();
    }
}

const SWEPT: &str = "lens sweep: 3 commit(s), 80 changed line(s) in 2 area(s) on origin/main.\nBlockers: 2 (1 fixed), majors: 0, minors: 0; 0 skipped as already reported.\nReport: .review/sweep-1.json\nDigest: .review/sweep-1.md\nPR: https://github.com/o/webapp/pull/9\nWatermark: abc";

#[test]
fn a_sweep_links_its_pr_and_digest_and_files_an_escalated_blocker_once() {
    let b = board(SWEPT, 0, true);
    b.report();
    lens_sweep::tick(&b.app).unwrap();
    assert_eq!(b.calls(), [format!("sweep --repo {}", b.repo.display())], "no --now: lens decides when it's due");
    let s = b.project()["lens_sweep"].clone();
    assert_eq!(s["state"], "done", "{s}");
    assert_eq!(s["pr"], "https://github.com/o/webapp/pull/9");
    assert_eq!(s["digest"], b.repo.join(".review/sweep-1.md").to_string_lossy().as_ref());
    assert!(s.get("filed").is_none(), "the dedup keys stay inside: {s}");
    let alerts = dispatch::alerts(&b.app);
    assert!(alerts.iter().any(|a| a["text"].as_str().unwrap().contains("pull/9") && a["text"].as_str().unwrap().contains("sweep-1.md")), "{alerts:?}");
    let tasks = b.tasks();
    assert_eq!(tasks.len(), 1, "only the escalated blocker becomes a task: {tasks:?}");
    let t = &tasks[0];
    assert_eq!(t.st("status"), "queued");
    assert!(t.st("title").contains("db.rs:40") && t.st("title").contains("lost write"), "{}", t.st("title"));
    let detail = t.st("detail");
    for want in ["two saves at once drop one", "the fix needs a schema change", "a migration", "pull/9", "sweep-1.md"] {
        assert!(detail.contains(want), "{want} missing from: {detail}");
    }
    lens_sweep::tick(&b.app).unwrap();
    assert_eq!(b.tasks().len(), 1, "the next sweep that still escalates it doesn't file it again");
    assert_eq!(dispatch::alerts(&b.app).iter().filter(|a| a["text"].as_str().unwrap().contains("pull/9")).count(), 1);
}

#[test]
fn a_sweep_that_isnt_due_says_why_and_keeps_the_last_links() {
    let b = board(SWEPT, 0, true);
    b.report();
    lens_sweep::tick(&b.app).unwrap();
    let mut rec = lens_sweep::of(&b.app, "webapp").unwrap();
    lens_sweep::outcome(&b.app, "webapp", &b.repo, "lens: sweep deferred: next at 3:00 PM\n", &mut rec).unwrap();
    assert_eq!(rec["said"], "lens: sweep deferred: next at 3:00 PM");
    assert_eq!(rec["pr"], "https://github.com/o/webapp/pull/9", "the last sweep's PR stays on show");
}

#[test]
fn a_failed_sweep_shows_lens_s_error() {
    let b = board("", 1, true);
    lens_sweep::tick(&b.app).unwrap();
    let s = b.project()["lens_sweep"].clone();
    assert_eq!((s["state"].clone(), s["error"].clone()), (json!("failed"), json!("lens broke")), "{s}");
    assert!(b.tasks().is_empty());
}

#[test]
fn no_sweep_without_a_remote_or_with_lens_off() {
    let b = board(SWEPT, 0, false);
    lens_sweep::tick(&b.app).unwrap();
    assert!(b.calls().is_empty(), "nothing pushed to sweep without a remote");
    let b = board(SWEPT, 0, true);
    api::dispatch(&b.app, "POST", "/projects/webapp", &Query::new(), &json!({"lens": "off"})).unwrap();
    lens_sweep::tick(&b.app).unwrap();
    assert!(b.calls().is_empty(), "lens off for the project");
    assert!(b.project()["lens_sweep"].is_null());
}
