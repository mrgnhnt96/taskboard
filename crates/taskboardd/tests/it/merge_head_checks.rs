//! A PR merges only on its current head's checks (GitHub #933): a push the host's read doesn't show yet
//! holds the merge, and a PR merged while its head's checks still run keeps its task open on "Merged,
//! checks still running" until they finish, telling the owner if they fail. One test here moves the clock.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, Check, FakeHost, Record};
use taskboardd::util::{advance_clock, reset_clock, RowExt};
use taskboardd::{board, dispatch, midna, p, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    cfg.pr.agents_merge = Some(true);
    cfg.pr.no_checks_after_mins = 0.0;
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
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
    fn repo(&self) -> std::path::PathBuf {
        self.dir.path().join("webapp")
    }
    fn pr_task(&self, num: i64) -> i64 {
        let id = self.post("/tasks", json!({"title": format!("Ship {num}"), "detail": "Do it.", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
        let sid = format!("s{num}");
        let repo = self.repo().to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[])
            .unwrap();
        self.report("tb.take", &sid, json!({"task": format!("T{id}")}));
        self.report("tb.done", &sid, json!({"summary": "Done", "pr": format!("https://bitbucket.org/acme/webapp/pull-requests/{num}")}));
        id
    }
    fn task(&self, id: i64) -> taskboardd::util::Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn phase(&self, id: i64) -> String {
        self.task(id).st("pr_phase")
    }
    fn step(&self, id: i64, rec: &Value) {
        self.app.db.tx(|| prflow::step(&self.app, &self.task(id), rec)).unwrap();
    }
    fn log(&self, id: i64) -> Vec<String> {
        self.app.db.q("SELECT text FROM events WHERE task_id = ?", p![id]).unwrap().iter().filter_map(|r| r.s("text").map(|s| s.to_string())).collect()
    }
    fn alerts_for(&self, num: i64) -> Vec<String> {
        dispatch::alerts(&self.app)
            .iter()
            .filter_map(|a| a["text"].as_str().map(str::to_string))
            .filter(|t| t.contains(&format!("PR #{num} ")) && t.contains("merged before its checks finished"))
            .collect()
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(repo).args(["-c", "user.name=t", "-c", "user.email=t@example.com"]).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn record(state: &str, head: &str, branch: &str, checks: &[(&str, &str)]) -> Record {
    Record {
        state: state.into(),
        author: "me".into(),
        head: head.into(),
        branch: branch.into(),
        base: "main".into(),
        checks: checks.iter().map(|(n, s)| Check { name: n.to_string(), state: s.to_string(), url: None, at: None }).collect(),
        review_decision: "APPROVED".into(),
        approvals: 2,
        ..Default::default()
    }
}

const HEAD: &str = "abcdef1234567890";

#[test]
fn a_push_the_host_doesnt_show_yet_holds_the_merge() {
    let b = board();
    let repo = b.repo();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
    let old = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "second"]);
    let new = git(&repo, &["rev-parse", "HEAD"]);

    // The agent force-pushed `new`; the host still reads `old`, whose checks are green.
    git(&repo, &["update-ref", "refs/remotes/origin/feat", &new]);
    let host = FakeHost::new("bitbucket", record("OPEN", &old, "feat", &[("build", "passed")]));
    prhost::install(&b.app, host.clone());
    let id = b.pr_task(31);
    let e = b.post(&format!("tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains(&format!("its branch was pushed at {} but Bitbucket still shows {}", &new[..10], &old[..10])), "{e}");
    assert_eq!(host.rec.lock().state, "OPEN", "nothing merged");

    // The host catches up and the new head's check is still running: still held.
    *host.rec.lock() = record("OPEN", &new, "feat", &[("build", "running")]);
    let e = b.post(&format!("tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains("checks are still running: build"), "{e}");
    assert!(!e.contains("was pushed at"), "{e}");

    // Green on the new head: it merges.
    *host.rec.lock() = record("OPEN", &new, "feat", &[("build", "passed")]);
    b.post(&format!("tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap();
    assert_eq!(b.phase(id), "merged");

    // A push from elsewhere the checkout hasn't fetched (its ref is behind the host's head) isn't a hold.
    git(&repo, &["update-ref", "refs/remotes/origin/other", &old]);
    *host.rec.lock() = record("OPEN", &new, "other", &[("build", "passed")]);
    let other = b.pr_task(32);
    b.post(&format!("tasks/T{other}/pr/merge"), json!({"agent": true})).unwrap();
    assert_eq!(b.phase(other), "merged");
}

#[test]
fn a_pr_merged_before_its_checks_finish_stays_open_until_they_do() {
    let b = board();
    let host = FakeHost::new("bitbucket", record("OPEN", HEAD, "feat", &[("build", "running"), ("lint", "passed")]));
    prhost::install(&b.app, host.clone());

    // The owner merges past the running build: the PR merges, the task stays on its checks.
    let id = b.pr_task(41);
    b.post(&format!("tasks/T{id}/pr/merge"), json!({"override": true})).unwrap();
    assert_eq!(host.rec.lock().state, "MERGED");
    let t = b.task(id);
    assert_eq!(t.s("pr_phase"), Some("landing"));
    assert_eq!(t.s("pr_state"), Some("MERGED"));
    assert_eq!(board::pr_card(&b.app, &t)["stage"]["label"], "Merged, checks still running");
    assert!(b.log(id).iter().any(|l| l == "Merged PR #41 before build finished: it stays open until they pass"), "{:?}", b.log(id));
    assert!(!b.log(id).iter().any(|l| l == "PR #41 was merged"));

    // A stale open record from before the merge doesn't pull it back.
    b.step(id, &record("OPEN", HEAD, "feat", &[("build", "running")]).to_value());
    assert_eq!(b.phase(id), "landing");
    b.step(id, &record("MERGED", HEAD, "feat", &[("build", "running")]).to_value());
    assert_eq!(b.phase(id), "landing");

    // The build passes: the task finishes as merged, with no alert.
    b.step(id, &record("MERGED", HEAD, "feat", &[("build", "passed"), ("lint", "passed")]).to_value());
    assert_eq!(b.phase(id), "merged");
    assert!(b.log(id).iter().any(|l| l == "PR #41 was merged"));
    assert!(b.alerts_for(41).is_empty());

    // Merged on the host while its build runs (the board reads it merged): the same hold.
    let failed = b.pr_task(42);
    b.step(failed, &record("OPEN", HEAD, "feat", &[("build", "running")]).to_value());
    assert_eq!(b.phase(failed), "checks");
    b.step(failed, &record("MERGED", HEAD, "feat", &[("build", "running")]).to_value());
    assert_eq!(b.phase(failed), "landing");
    assert_eq!(b.task(failed).s("pr_state"), Some("MERGED"));
    assert!(b.alerts_for(42).is_empty());

    // The build fails: the owner hears once, and the task finishes.
    let red = record("MERGED", HEAD, "feat", &[("build", "failed")]).to_value();
    b.step(failed, &red);
    let alerts = b.alerts_for(42);
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert!(alerts[0].contains("build failed on the merged commit abcdef1234: main may be broken"), "{}", alerts[0]);
    assert_eq!(b.phase(failed), "merged");
    b.step(failed, &red);
    assert_eq!(b.alerts_for(42).len(), 1, "once");

    // Merged with every check already passed: straight to merged.
    let green = b.pr_task(43);
    b.step(green, &record("MERGED", HEAD, "feat", &[("build", "passed")]).to_value());
    assert_eq!(b.phase(green), "merged");

    // `tb pr merged` while a check runs holds it too.
    let told = b.pr_task(44);
    b.step(told, &record("OPEN", HEAD, "feat", &[("build", "running")]).to_value());
    prflow::mark_merged(&b.app, &b.task(told), "owner").unwrap();
    assert_eq!(b.phase(told), "landing");
    b.step(told, &record("MERGED", HEAD, "feat", &[("build", "passed")]).to_value());
    assert_eq!(b.phase(told), "merged");
}

#[test]
fn a_force_push_merged_before_its_checks_post_waits_for_them() {
    reset_clock();
    let b = board();
    let host = FakeHost::new("bitbucket", record("OPEN", HEAD, "feat", &[("build", "failed")]));
    prhost::install(&b.app, host);

    // The last push's build failed; the force-pushed head merged before its build posted.
    let id = b.pr_task(51);
    b.step(id, &record("OPEN", HEAD, "feat", &[("build", "failed")]).to_value());
    assert_eq!(b.phase(id), "fix");
    let pushed = record("MERGED", "fedcba9876543210", "feat", &[]).to_value();
    b.step(id, &pushed);
    assert_eq!(b.phase(id), "landing", "the build that posted on the last push is expected on this one");

    // The expected build never posts: once its wait is over the task finishes.
    prflow::waits_over(&b.app).unwrap();
    assert_eq!(b.phase(id), "landing");
    advance_clock(91.0 * 60.0);
    prflow::waits_over(&b.app).unwrap();
    reset_clock();
    assert_eq!(b.phase(id), "merged");
    assert!(b.log(id).iter().any(|l| l == "PR #51 was merged"));
}
