//! A project's setup gaps (`setup.rs`): a Review step nobody can be asked for, or a PR flow with no
//! git remote. A green PR held at review with nobody who could review it reads as stuck, not as
//! waiting, and the owner is alerted once.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, dispatch, midna, prflow, reports, setup};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    cfg.pr.wake = true;
    f(&mut cfg);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    fn call(&self, method: &str, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, method, path, &Query::new(), &body).unwrap_or_else(|e| panic!("{method} {path}: {}", e.message))
    }
    fn report(&self, event: &str, extra: Value) {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message));
    }
    fn green_pr(&self) -> i64 {
        let id = self.call("POST", "/tasks", json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")}));
        self.report("tb.done", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9"}));
        let rec = json!({"state": "OPEN", "head": "h1", "checks": [{"name": "ci", "state": "passed"}], "failed": [], "running": 0,
                         "comments": 0, "approvals": 0, "review_decision": "", "changes_at": "", "reviewers": []});
        self.app.db.tx(|| prflow::step(&self.app, &board::get_task(&self.app, id)?, &rec)).unwrap();
        id
    }
    fn stage(&self, id: i64) -> Value {
        board::task_card(&self.app, &board::get_task(&self.app, id).unwrap()).unwrap()["pr"]["stage"].clone()
    }
    fn sweep(&self) {
        self.app.db.tx(|| setup::sweep(&self.app)).unwrap();
    }
    fn setup_alerts(&self) -> Vec<String> {
        dispatch::alerts(&self.app).iter().filter(|a| a["key"] == "setup:webapp").map(|a| a["text"].as_str().unwrap_or("").to_string()).collect()
    }
    fn gap_keys(&self) -> Vec<String> {
        setup::gaps(&self.app, "webapp").unwrap().iter().map(|g| g.key.to_string()).collect()
    }
}

#[test]
fn a_review_step_with_no_reviewers_is_a_gap_until_someone_is_added_or_the_step_is_off() {
    let b = board_with(|_| {});
    assert_eq!(b.gap_keys(), vec!["no_reviewers"]);
    let shown = b.call("GET", "/projects", json!({}))["projects"].as_array().unwrap().iter().find(|p| p["name"] == "webapp").cloned().unwrap();
    assert_eq!(shown["setup_gaps"][0]["key"], "no_reviewers", "{shown}");
    b.call("POST", "/reviewers", json!({"project": "webapp", "reviewer": "Sam", "user": "sam-gh", "emails": ["sam@acme.dev"]}));
    assert!(b.gap_keys().is_empty(), "Sam can be asked");

    let off = board_with(|_| {});
    off.call("POST", "/projects/webapp", json!({"review": false}));
    assert!(off.gap_keys().is_empty(), "no Review step, nobody needed");
}

#[test]
fn a_green_pr_nobody_can_review_reads_stuck_and_alerts_the_owner_once() {
    let b = board_with(|c| c.pr.owner_review = Some(false));
    let id = b.green_pr();
    let stage = b.stage(id);
    assert_eq!(stage["phase"], "review");
    assert_eq!(stage["label"], "Stuck: no reviewers", "{stage}");
    assert_eq!(stage["setup"], true);

    b.sweep();
    let alerts = b.setup_alerts();
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert!(alerts[0].contains(&format!("T{id}'s PR is stuck")) && alerts[0].contains("no reviewers"), "{}", alerts[0]);
    // The app's card: a short title, one line, and a button per fix.
    let a = dispatch::alerts(&b.app).into_iter().find(|a| a["key"] == "setup:webapp").unwrap();
    assert_eq!(a["title"], "webapp PRs can't merge", "{a}");
    let detail = a["detail"].as_str().unwrap();
    assert!(detail.starts_with(&format!("T{id} is waiting for ")) && detail.ends_with(", and webapp has no reviewers"), "{detail}");
    let labels: Vec<&str> = a["actions"].as_array().unwrap().iter().map(|x| x["label"].as_str().unwrap()).collect();
    assert_eq!(labels, vec!["Turn off review for webapp", "Add a reviewer"]);
    assert_eq!(a["actions"][0]["command"], "tb project set webapp --review off");
    // With Midna away the board can't open a terminal; the app copies the command instead.
    let id_s = a["id"].as_str().unwrap();
    assert!(api::dispatch(&b.app, "POST", &format!("/alerts/{id_s}/act"), &Query::new(), &json!({"n": 0})).is_err());
    assert!(api::dispatch(&b.app, "POST", &format!("/alerts/{id_s}/act"), &Query::new(), &json!({"n": 5})).is_err(), "no such fix");

    let alert_id = dispatch::alerts(&b.app).iter().find(|a| a["key"] == "setup:webapp").unwrap()["id"].as_str().unwrap().to_string();
    dispatch::clear_alerts(&b.app, None, Some(&alert_id)).unwrap();
    b.sweep();
    assert!(b.setup_alerts().is_empty(), "dismissed stays dismissed while nothing changes");

    b.call("POST", "/projects/webapp", json!({"review": false}));
    b.sweep();
    assert!(b.setup_alerts().is_empty());
    assert_ne!(b.stage(id)["label"], "Stuck: no reviewers", "fixed: it moves on");
}

/// A `midna` that logs each call's method and params and opens terminal s9.
fn fake_midna(dir: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let exe = dir.join("midna");
    std::fs::write(&exe, format!("#!/bin/sh\necho \"$2 $3\" >> '{}'\necho '{{\"id\": \"s9\"}}'\n", dir.join("calls.log").display())).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    exe
}

#[test]
fn a_fix_button_opens_claude_in_the_project_with_the_request_typed_in_not_sent() {
    let b = board_with(|c| {
        c.pr.owner_review = Some(false);
        c.midna = fake_midna(&c.data.clone());
    });
    b.green_pr();
    b.sweep();
    let a = dispatch::alerts(&b.app).into_iter().find(|a| a["key"] == "setup:webapp").unwrap();
    let id_s = a["id"].as_str().unwrap().to_string();
    let _ = std::fs::remove_file(b.dir.path().join("calls.log"));
    b.app.shared.lock().midna_seen_at = Some(taskboardd::util::now_iso());

    let r = b.call("POST", &format!("/alerts/{id_s}/act"), json!({"n": 0}));
    let jid = r["job"].as_str().unwrap().trim_start_matches(|c: char| !c.is_ascii_digit()).parse::<i64>().unwrap();
    let j = b.app.db.q1("SELECT * FROM jobs WHERE id = ?", taskboardd::p![jid]).unwrap().unwrap();
    midna::run_job(&b.app, &j);

    let log = std::fs::read_to_string(b.dir.path().join("calls.log")).unwrap();
    let call = |m: &str| -> Value {
        let line = log.lines().find_map(|l| l.strip_prefix(&format!("{m} "))).unwrap_or_else(|| panic!("no {m} in {log}"));
        serde_json::from_str(line).unwrap()
    };
    let open = call("session.open");
    assert_eq!(open["cwd"], b.dir.path().join("webapp").to_string_lossy().as_ref(), "{open}");
    assert!(open.get("prompt").is_none(), "nothing is sent for the owner: {open}");
    let typed = call("queue.add");
    assert_eq!(typed["session"], "s9");
    assert_eq!(typed["enter"], false, "typed in, not sent");
    assert_eq!(typed["text"], a["actions"][0]["prompt"]);

    // A project the board has no folder for: refused, so the app copies the command.
    let fix = json!({"title": "nowhere can't open PRs", "project": "nowhere", "actions": [a["actions"][0].clone()]});
    b.app.db.tx(|| dispatch::add_alert_keyed_with(&b.app, "nowhere can't open PRs", None, None, "setup:nowhere", fix.clone())).unwrap();
    let other = dispatch::alerts(&b.app).into_iter().find(|a| a["key"] == "setup:nowhere").unwrap();
    let path = format!("/alerts/{}/act", other["id"].as_str().unwrap());
    let e = api::dispatch(&b.app, "POST", &path, &Query::new(), &json!({"n": 0})).unwrap_err();
    assert_eq!(e.status, 409, "{}", e.message);
}

#[test]
fn a_project_with_gaps_but_no_work_raises_nothing_and_fixing_it_clears_the_alert() {
    let b = board_with(|_| {});
    b.sweep();
    assert!(b.setup_alerts().is_empty(), "nothing on it yet");
    b.call("POST", "/tasks", json!({"title": "Queued", "detail": "Do it.", "project": "webapp"}));
    b.app.db.x("UPDATE tasks SET status = 'queued'", taskboardd::p![]).unwrap();
    b.sweep();
    assert_eq!(b.setup_alerts().len(), 1, "work queued on a project that can't finish it");
    b.call("POST", "/reviewers", json!({"project": "webapp", "reviewer": "Sam", "user": "sam-gh", "emails": ["sam@acme.dev"]}));
    b.sweep();
    assert!(b.setup_alerts().is_empty(), "cleared once it's set up");
}

#[test]
fn a_pr_flow_on_without_a_git_remote_is_a_gap_and_the_handoff_says_so() {
    let b = board_with(|_| {});
    b.call("POST", "/reviewers", json!({"project": "webapp", "reviewer": "Sam", "user": "sam-gh", "emails": ["sam@acme.dev"]}));
    let repo = b.dir.path().join("webapp");
    assert!(std::process::Command::new("git").arg("init").arg("-q").arg(&repo).status().unwrap().success());
    b.call("POST", "/projects/webapp", json!({"pr_flow": "on"}));
    assert_eq!(b.gap_keys(), vec!["no_remote"]);
    let line = setup::handoff_line(&b.app, "webapp").unwrap().unwrap();
    assert!(line.contains("no git remote"), "{line}");
}

#[test]
fn a_project_with_no_reviewers_syncs_its_roster_once_on_its_own() {
    let b = board_with(|c| c.reviewers.min_commits = 1);
    let repo = b.dir.path().join("webapp");
    let git = |args: &[&str]| crate::reviewers::sh_git(&repo, args);
    git(&["init", "-q", "-b", "main"]);
    git(&["remote", "add", "origin", "https://github.com/acme/webapp.git"]);
    std::fs::write(repo.join("a.rs"), "fn a() {}").unwrap();
    git(&["add", "."]);
    git(&["-c", "user.name=Me", "-c", "user.email=me@acme.dev", "commit", "-q", "-m", "a", "--author", "Sam Lee <7+sam-gh@users.noreply.github.com>"]);
    assert_eq!(b.gap_keys(), vec!["no_reviewers"]);

    setup::auto_sync(&b.app).unwrap();
    assert!(b.gap_keys().is_empty(), "Sam joined from the history with their host id");

    // Once per project: after the owner takes Sam off, the board doesn't sync again on its own.
    b.call("POST", "/reviewers/remove", json!({"project": "webapp", "reviewer": "Sam Lee"}));
    assert_eq!(b.gap_keys(), vec!["no_reviewers"]);
    setup::auto_sync(&b.app).unwrap();
    assert_eq!(b.gap_keys(), vec!["no_reviewers"], "synced already");
    // With no PR to match members against, the first ask's sync isn't held off.
    assert_eq!(b.app.db.get_setting("reviewers_synced:webapp").unwrap(), None);
}

fn auto_synced(b: &Board) -> Option<String> {
    b.app.db.get_setting("reviewers_auto_synced").unwrap()
}

#[test]
fn auto_sync_leaves_projects_without_the_gap_or_a_folder_alone() {
    // Already staffed: nothing to sync.
    let staffed = board_with(|_| {});
    staffed.call("POST", "/reviewers", json!({"project": "webapp", "reviewer": "Sam", "user": "sam-gh"}));
    setup::auto_sync(&staffed.app).unwrap();
    assert_eq!(auto_synced(&staffed), None);

    // No PRs (a folder with no remote): no Review step to staff.
    let no_prs = board_with(|_| {});
    crate::reviewers::sh_git(&no_prs.dir.path().join("webapp"), &["init", "-q"]);
    setup::auto_sync(&no_prs.app).unwrap();
    assert_eq!(auto_synced(&no_prs), None);

    // No folder yet: it waits for one instead of spending its one sync.
    let no_folder = board_with(|_| {});
    std::fs::remove_dir_all(no_folder.dir.path().join("webapp")).unwrap();
    setup::auto_sync(&no_folder.app).unwrap();
    assert_eq!(auto_synced(&no_folder), None);
}
