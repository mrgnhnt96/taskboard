//! PR builds stopped (#17): the owner's switch recording who, the cancels it triggers (the host's own
//! or a provider's command, retried, then an alert), checks counting as passed, the owner's push
//! builds, and build hooks for PRs on hosts the board doesn't watch.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, Check, FakeHost, Record};
use taskboardd::util::RowExt;
use taskboardd::{board, dispatch, hooks, midna, prbuilds, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board_with(f: impl FnOnce(&mut Config, &std::path::Path)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    f(&mut cfg, dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    Board { app, dir }
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn report(&self, event: &str, extra: Value) {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap();
    }
    fn pr_task(&self, url: &str) -> i64 {
        let id = self.post("/tasks", json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", json!({"task": format!("T{id}")}));
        self.report("tb.done", json!({"summary": "Done", "pr": url}));
        id
    }
    fn task(&self, id: i64) -> serde_json::Map<String, Value> {
        board::get_task(&self.app, id).unwrap()
    }
    fn flow(&self, id: i64) -> Value {
        serde_json::from_str(&self.task(id).st("pr_flow")).unwrap()
    }
    fn queue(&self) -> Vec<Value> {
        serde_json::from_str(&self.app.db.get_setting("pr_build_cancels").unwrap().unwrap_or("[]".into())).unwrap()
    }
    /// Makes every waiting cancel due now.
    fn due_now(&self) {
        let q: Vec<Value> = self.queue().into_iter().map(|mut x| {
            x["next_at"] = json!("2000-01-01T00:00:00Z");
            x
        }).collect();
        self.app.db.set_setting("pr_build_cancels", Some(&Value::Array(q).to_string())).unwrap();
    }
}

const GH: &str = "https://github.com/acme/webapp/pull/9";

fn running() -> Record {
    Record {
        state: "OPEN".into(),
        author: "me".into(),
        head: "h1".into(),
        base: "main".into(),
        checks: vec![Check { name: "build".into(), state: "running".into(), url: Some("https://github.com/acme/webapp/actions/runs/55/job/1".into()) }],
        threads: vec![],
        ..Default::default()
    }
}

#[test]
fn stopping_records_who_and_the_checks_count_as_passed_while_builds_are_cancelled() {
    let b = board_with(|_, _| {});
    let id = b.pr_task(GH);
    let h = FakeHost::new("github", running());
    prhost::install(&b.app, h.clone());
    prflow::refresh(&b.app).unwrap();
    assert_eq!(b.task(id).st("pr_phase"), "checks");

    let v = b.post("/pr-builds", json!({"stopped": true, "who": "Sam", "reason": "CI minutes ran out"}));
    assert_eq!((v["stopped"].as_bool(), v["by"].as_str(), v["reason"].as_str()), (Some(true), Some("Sam"), Some("CI minutes ran out")));
    assert_eq!(b.get("/state")["pr_builds"]["stopped"], true);
    assert_eq!(b.queue().len(), 1, "stopping cancels what's already running");

    taskboardd::runner::prs(&b.app).unwrap();
    assert_eq!(b.task(id).st("pr_phase"), "review", "the checks count as passed");
    assert_eq!(b.task(id).st("pr_build"), "Builds stopped");
    prbuilds::tick(&b.app).unwrap();
    assert!(h.calls().contains(&"cancel h1".to_string()), "{:?}", h.calls());
    assert_eq!(b.queue().len(), 1, "a follow-up sweep waits");
    assert_eq!(b.queue()[0]["round"], 1);
    assert_eq!(b.get("/pr-builds")["cancelling"], 0, "follow-ups aren't counted as cancelling");
    assert!(b.flow(id)["builds_cancelled"]["h1"].is_string());
    taskboardd::runner::prs(&b.app).unwrap();
    assert_eq!(b.queue().len(), 1, "a push's builds are cancelled once, then followed up");

    let v = b.post("/pr-builds", json!({"stopped": false, "who": "Sam"}));
    assert_eq!((v["stopped"].as_bool(), v["resumed_by"].as_str()), (Some(false), Some("Sam")));
    assert!(b.queue().is_empty(), "resuming drops the follow-ups");
    assert_eq!(v["recent"][0]["what"], "stopped 0 builds");
    prflow::refresh(&b.app).unwrap();
    assert_eq!(b.task(id).st("pr_phase"), "checks", "resumed: the checks count again");
    assert!(api::dispatch(&b.app, "POST", "/pr-builds", &Query::new(), &json!({})).is_err());
}

#[test]
fn a_build_event_runs_the_provider_s_cancel_command_and_retries_then_alerts() {
    let b = board_with(|c, dir| {
        c.pr_builds.cancel.insert("azure".into(), format!("echo \"$TB_PROVIDER $TB_HEAD $TB_PR_NUM\" >> '{}'; exit 1", dir.join("cancels.log").display()));
    });
    let id = b.pr_task(GH);
    b.post("/pr-builds", json!({"stopped": true, "who": "Sam"}));
    let v = b.post("/prs/event", json!({"url": GH, "kind": "build", "state": "started", "head": "h2", "provider": "azure"}));
    assert_eq!(v["builds"]["cancel"], "queued");
    b.post("/prs/event", json!({"url": GH, "kind": "build", "state": "running", "head": "h2", "provider": "azure"}));
    assert_eq!(b.queue().len(), 1, "the same push is cancelled once");

    prbuilds::tick(&b.app).unwrap();
    let log = std::fs::read_to_string(b.dir.path().join("cancels.log")).unwrap();
    assert_eq!(log.trim(), "azure h2 9");
    assert_eq!(b.queue()[0]["tries"], 1);
    prbuilds::tick(&b.app).unwrap();
    assert_eq!(b.queue()[0]["tries"], 1, "the next try waits 10 seconds");
    for _ in 0..4 {
        b.due_now();
        prbuilds::tick(&b.app).unwrap();
    }
    assert!(b.queue().is_empty(), "five tries, then it gives up");
    let alerts = dispatch::alerts(&b.app);
    assert_eq!(alerts.len(), 1);
    let text = alerts[0]["text"].as_str().unwrap();
    assert!(text.contains("couldn't cancel the builds of PR #9 at h2 after 5 tries"), "{text}");
    assert_eq!(alerts[0]["task"], format!("T{id}"));
}

#[test]
fn a_ci_without_a_cancel_command_alerts_at_once() {
    let b = board_with(|_, _| {});
    b.pr_task(GH);
    b.post("/pr-builds", json!({"stopped": true}));
    assert_eq!(b.get("/pr-builds")["by"], "Sam", "the owner, when nobody's named");
    b.post("/prs/event", json!({"url": GH, "kind": "build", "state": "started", "head": "h3", "provider": "jenkins"}));
    prbuilds::tick(&b.app).unwrap();
    let alerts = dispatch::alerts(&b.app);
    assert!(alerts[0]["text"].as_str().unwrap().contains("there's no cancel command for jenkins in [pr_builds.cancel]"), "{}", alerts[0]);
}

#[test]
fn the_owner_s_own_push_builds_are_cancelled_too() {
    let b = board_with(|c, dir| {
        c.owner_emails = vec!["sam@acme.dev".into()];
        c.pr_builds.cancel.insert("github".into(), format!("echo \"$TB_BRANCH $TB_HEAD\" >> '{}'", dir.join("cancels.log").display()));
    });
    b.post("/pr-builds", json!({"stopped": true}));
    let v = b.post("/prs/event", json!({"kind": "build", "state": "started", "repo": "acme/webapp", "host": "github", "branch": "wip", "head": "p1",
                                        "author": "someone@acme.dev"}));
    assert_eq!(v["builds"]["owners"], false);
    assert!(b.queue().is_empty(), "not the owner's push");
    let v = b.post("/prs/event", json!({"kind": "build", "state": "started", "repo": "acme/webapp", "host": "github", "branch": "wip", "head": "p1",
                                        "author": "Sam@Acme.dev"}));
    assert_eq!(v["builds"]["cancel"], "queued");
    prbuilds::tick(&b.app).unwrap();
    assert_eq!(std::fs::read_to_string(b.dir.path().join("cancels.log")).unwrap().trim(), "wip p1");
    assert!(dispatch::alerts(&b.app).is_empty());
}

#[test]
fn a_push_is_swept_again_on_the_follow_up_schedule() {
    let b = board_with(|c, dir| {
        c.owner_emails = vec!["sam@acme.dev".into()];
        c.pr_builds.follow_up_secs = vec![10.0, 30.0];
        c.pr_builds.cancel.insert("github".into(), format!("echo \"$TB_HEAD\" >> '{}'", dir.join("cancels.log").display()));
    });
    b.post("/pr-builds", json!({"stopped": true}));
    b.post("/prs/event", json!({"kind": "build", "state": "started", "repo": "acme/webapp", "host": "github", "branch": "wip", "head": "p1",
                                "author": "sam@acme.dev"}));
    prbuilds::tick(&b.app).unwrap();
    assert_eq!(b.queue().len(), 1);
    let first_at = b.queue()[0]["first_at"].as_f64().unwrap();
    let next = taskboardd::util::iso(first_at + 10.0);
    assert_eq!(b.queue()[0]["next_at"].as_str(), Some(next.as_str()), "the first follow-up is 10 seconds after the cancel");
    prbuilds::tick(&b.app).unwrap();
    assert_eq!(b.queue()[0]["round"], 1, "not due yet");
    b.due_now();
    prbuilds::tick(&b.app).unwrap();
    assert_eq!(b.queue()[0]["round"], 2);
    b.due_now();
    prbuilds::tick(&b.app).unwrap();
    assert!(b.queue().is_empty(), "two follow-ups, then it's done");
    let log = std::fs::read_to_string(b.dir.path().join("cancels.log")).unwrap();
    assert_eq!(log.lines().count(), 3);
    let recent = b.get("/pr-builds")["recent"].as_array().cloned().unwrap();
    assert_eq!(recent.len(), 1, "the follow-ups reported nothing stopped");
    assert_eq!(recent[0]["follow_up"].as_bool(), Some(false));
}

#[test]
fn a_push_on_the_branch_of_an_owner_s_pr_is_cancelled_whoever_wrote_it() {
    let b = board_with(|c, _| c.owner_emails = vec!["sam@acme.dev".into()]);
    let id = b.pr_task(GH);
    let mut rec = running();
    rec.branch = "feature/login".into();
    let h = FakeHost::new("github", rec);
    prhost::install(&b.app, h.clone());
    prflow::refresh(&b.app).unwrap();
    b.post("/pr-builds", json!({"stopped": true}));
    b.due_now();
    prbuilds::tick(&b.app).unwrap();
    b.app.db.set_setting("pr_build_cancels", None).unwrap();

    let ev = |branch: &str| json!({"kind": "build", "state": "started", "repo": "acme/webapp", "host": "github", "branch": branch, "head": "m1",
                                   "author": "pat@acme.dev"});
    let v = b.post("/prs/event", ev("other"));
    assert_eq!(v["builds"]["owners"], false);
    assert!(b.queue().is_empty());
    let v = b.post("/prs/event", ev("feature/login"));
    assert_eq!((v["builds"]["owners"].as_bool(), v["builds"]["task"].as_str()), (Some(true), Some(format!("T{id}").as_str())));
    assert_eq!(b.queue()[0]["key"], format!("T{id}:m1"));
    prbuilds::tick(&b.app).unwrap();
    assert!(h.calls().contains(&"cancel m1".to_string()), "{:?}", h.calls());
}

#[test]
fn stopping_or_resuming_takes_down_the_give_up_alerts() {
    let b = board_with(|_, _| {});
    b.pr_task(GH);
    b.post("/pr-builds", json!({"stopped": true}));
    b.post("/prs/event", json!({"url": GH, "kind": "build", "state": "started", "head": "h3", "provider": "jenkins"}));
    prbuilds::tick(&b.app).unwrap();
    assert_eq!(dispatch::alerts(&b.app).len(), 1);
    assert_eq!(b.get("/pr-builds")["recent"][0]["ok"], false);
    b.post("/pr-builds", json!({"stopped": false}));
    assert!(dispatch::alerts(&b.app).is_empty(), "resume clears the give-up alert");
}

#[test]
fn build_hooks_fire_for_a_pr_on_a_host_the_board_does_not_watch() {
    let b = board_with(|c, _| {
        std::fs::write(
            hooks::path(c),
            json!({"hooks": {"pr.checks": [{"hooks": [{"type": "command", "command": "echo '{\"decision\": \"skip\", \"reason\": \"cancelled them\"}'"}]}]}}).to_string(),
        )
        .unwrap();
    });
    let id = b.pr_task("https://gitlab.com/acme/webapp/-/merge_requests/4");
    assert_eq!(b.task(id).st("pr_host"), "gitlab");
    let v = b.post("/prs/event", json!({"url": "https://gitlab.com/acme/webapp/-/merge_requests/4", "kind": "build", "state": "started", "head": "g1"}));
    assert_eq!(v["task"], format!("T{id}"));
    assert_eq!(v["builds"]["hook"], "skip");
    assert_eq!(b.flow(id)["skip_checks"]["g1"], "cancelled them");
    let v = b.post("/prs/event", json!({"url": "https://gitlab.com/acme/webapp/-/merge_requests/4", "kind": "build", "state": "running", "head": "g1"}));
    assert!(v["builds"].get("hook").is_none(), "asked once per push");
}

#[test]
fn builds_on_a_pr_the_owner_opened_by_hand_are_cancelled_one_recent_entry_per_build() {
    let b = board_with(|c, _| c.pr_builds.owner_prs_secs = 120.0);
    b.pr_task(GH);
    let h = FakeHost::new("github", running());
    *h.open_prs.lock() = vec![prhost::OpenPr { num: 12, branch: "by-hand".into(), url: "https://github.com/acme/webapp/pull/12".into() }];
    *h.stops.lock() = vec!["CI #3".into(), "Lint #4".into()];
    prhost::install(&b.app, h.clone());
    b.post("/pr-builds", json!({"stopped": true}));
    prbuilds::tick(&b.app).unwrap();
    assert_eq!(b.get("/pr-builds")["owner_prs"][0]["num"], 12, "read from the host on the repos the board knows");

    let v = b.post("/prs/event", json!({"kind": "build", "state": "started", "url": "https://github.com/acme/webapp/pull/12", "head": "x1"}));
    assert_eq!((v["task"].is_null(), v["builds"]["owners"].as_bool(), v["builds"]["pr"].as_i64()), (true, Some(true), Some(12)));
    assert_eq!(b.queue()[0]["key"], "acme/webapp#12:x1");
    assert_eq!((b.queue()[0]["num"].as_i64(), b.queue()[0]["branch"].as_str()), (Some(12), Some("by-hand")), "cancelled as that PR's, by its number");
    prbuilds::tick(&b.app).unwrap();
    assert!(h.calls().contains(&"cancel x1".to_string()), "{:?}", h.calls());
    let recent = b.get("/pr-builds")["recent"].as_array().cloned().unwrap();
    let builds: Vec<&str> = recent.iter().filter_map(|r| r["build"].as_str()).collect();
    assert_eq!(builds, vec!["CI #3", "Lint #4"], "one entry per build, with its pipeline's name");

    let v = b.post("/prs/event", json!({"kind": "build", "state": "started", "repo": "acme/webapp", "host": "github", "branch": "by-hand", "head": "x2"}));
    assert_eq!(v["builds"]["pr"], 12, "or by its branch");
    let v = b.post("/prs/event", json!({"kind": "build", "state": "started", "repo": "acme/webapp", "host": "github", "branch": "someone-else", "head": "x3"}));
    assert_eq!(v["builds"]["owners"], false);
}

#[test]
fn the_feed_names_the_owner_s_hand_opened_prs_until_they_close() {
    let b = board_with(|c, dir| {
        c.owner_emails = vec!["sam@acme.dev".into()];
        c.pr_builds.cancel.insert("github".into(), format!("echo \"$TB_PR_NUM $TB_BRANCH\" >> '{}'", dir.join("cancels.log").display()));
    });
    b.post("/pr-builds", json!({"stopped": true}));
    let url = "https://github.com/acme/webapp/pull/14";
    let v = b.post("/prs/event", json!({"kind": "pr", "url": url, "author": "pat@acme.dev", "branch": "theirs"}));
    assert_eq!(v["owner_pr"]["owners"], false);
    let v = b.post("/prs/event", json!({"kind": "pr", "url": url, "author": "Sam@acme.dev", "branch": "hand2", "state": "open"}));
    assert_eq!(v["owner_pr"]["open"], true);
    let build = || json!({"kind": "build", "state": "started", "repo": "acme/webapp", "host": "github", "branch": "hand2", "head": "y1", "author": "pat@acme.dev"});
    assert_eq!(b.post("/prs/event", build())["builds"]["cancel"], "queued");
    prbuilds::tick(&b.app).unwrap();
    assert_eq!(std::fs::read_to_string(b.dir.path().join("cancels.log")).unwrap().trim(), "14 hand2");

    b.post("/prs/event", json!({"kind": "pr", "url": url, "mine": true, "state": "merged"}));
    b.app.db.set_setting("pr_build_cancels", None).unwrap();
    assert_eq!(b.post("/prs/event", build())["builds"]["owners"], false, "merged: no longer an open PR of the owner's");
}

#[test]
fn a_cancel_command_s_follow_ups_are_not_logged_on_the_task() {
    let b = board_with(|c, dir| {
        c.pr_builds.cancel.insert("azure".into(), format!("echo \"$TB_HEAD\" >> '{}'", dir.join("cancels.log").display()));
    });
    let id = b.pr_task(GH);
    b.post("/pr-builds", json!({"stopped": true}));
    b.post("/prs/event", json!({"url": GH, "kind": "build", "state": "started", "head": "h5", "provider": "azure", "definition": {"name": "webapp-ci"}}));
    prbuilds::tick(&b.app).unwrap();
    for _ in 0..4 {
        b.due_now();
        prbuilds::tick(&b.app).unwrap();
    }
    assert!(b.queue().is_empty());
    assert_eq!(std::fs::read_to_string(b.dir.path().join("cancels.log")).unwrap().lines().count(), 5, "the first cancel and four follow-ups ran");
    let logged = b.app.db.count("SELECT COUNT(*) FROM events WHERE task_id = ? AND text LIKE 'PR builds are stopped: cancelled%'", vec![json!(id)]).unwrap();
    assert_eq!(logged, 1, "only the first cancel is told on the task");
    let recent = b.get("/pr-builds")["recent"].as_array().cloned().unwrap();
    assert_eq!(recent.len(), 1, "follow-ups that report nothing stay out of the recent list (#86)");
    assert_eq!(recent[0]["build"], "webapp-ci", "named by the build event's pipeline");
}

/// #86: a cancel command reports the builds it stopped in $TB_CANCELLED; a follow-up that stopped
/// some is kept, one entry per build.
#[test]
fn a_cancel_command_reports_what_it_stopped() {
    let b = board_with(|c, dir| {
        let log = dir.join("cancels.log");
        c.pr_builds.cancel.insert(
            "azure".into(),
            format!("echo x >> '{0}'; if [ $(wc -l < '{0}') -eq 2 ]; then echo 'Late #8' > \"$TB_CANCELLED\"; fi", log.display()),
        );
    });
    b.pr_task(GH);
    b.post("/pr-builds", json!({"stopped": true}));
    b.post("/prs/event", json!({"url": GH, "kind": "build", "state": "started", "head": "h6", "provider": "azure", "pipeline": "webapp-ci"}));
    prbuilds::tick(&b.app).unwrap();
    for _ in 0..4 {
        b.due_now();
        prbuilds::tick(&b.app).unwrap();
    }
    let recent = b.get("/pr-builds")["recent"].as_array().cloned().unwrap();
    let builds: Vec<(&str, bool)> = recent.iter().map(|r| (r["build"].as_str().unwrap_or(""), r["follow_up"] == true)).collect();
    assert_eq!(builds, vec![("Late #8", true), ("webapp-ci", false)]);
    assert_eq!(recent[0]["what"], "the azure cancel command stopped 1 build");
}

/// #86: a build event that names the repo `org/repo` still finds a hand-opened PR noted as `repo`.
#[test]
fn a_hand_opened_pr_matches_by_its_short_repo_name() {
    let b = board_with(|c, dir| {
        c.owner_emails = vec!["sam@acme.dev".into()];
        c.pr_builds.cancel.insert("azure".into(), format!("echo \"$TB_PR_NUM\" >> '{}'", dir.join("cancels.log").display()));
    });
    b.post("/pr-builds", json!({"stopped": true}));
    b.post("/prs/event", json!({"kind": "pr", "host": "azure", "repo": "webapp", "num": 21, "author": "sam@acme.dev", "branch": "hand3"}));
    let v = b.post("/prs/event", json!({"kind": "build", "state": "started", "repo": "acme/webapp", "branch": "hand3", "head": "z1", "provider": "azure"}));
    assert_eq!(v["builds"]["pr"], 21, "{v}");
}

/// #91: a build event finds a board task's PR by its short repo name in any case, among the board's
/// repos, so two orgs' repos with one name don't clash.
#[test]
fn a_board_task_s_pr_matches_by_its_short_repo_name_in_any_case() {
    let b = board_with(|_, _| {});
    let id = b.pr_task(GH);
    let task_of = |repo: &str| b.post("/prs/event", json!({"kind": "build", "state": "started", "repo": repo, "num": 9, "head": "h9"}))["task"].clone();
    assert_eq!(task_of("acme/webapp"), format!("T{id}"));
    assert_eq!(task_of("webapp"), format!("T{id}"));
    assert_eq!(task_of("ACME/webapp"), format!("T{id}"));
    assert_eq!(task_of("acme/other"), Value::Null);
    assert_eq!(task_of("globex/webapp"), Value::Null, "another org's webapp isn't acme's");

    // Another org's webapp #9 on the board: the short name no longer picks one, the full name does.
    let other = b.post("/tasks", json!({"title": "Other org", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
    b.app.db.x("UPDATE tasks SET pr_host = 'github', pr_repo = 'globex/webapp', pr_num = 9 WHERE id = ?", vec![json!(other)]).unwrap();
    assert_eq!(task_of("webapp"), Value::Null, "two orgs' webapp #9: the short name is ambiguous");
    assert_eq!(task_of("Acme/WebApp"), format!("T{id}"));
    assert_eq!(task_of("globex/webapp"), format!("T{other}"));
    let on = |host: &str| b.post("/prs/event", json!({"kind": "build", "state": "started", "host": host, "repo": "globex/webapp", "num": 9}))["task"].clone();
    assert_eq!(on("bitbucket"), Value::Null, "only the board's PRs on the event's host");
    assert_eq!(on("GitHub"), format!("T{other}"));
}

/// #91: the owner's hand-opened PRs in two orgs' repos of one name stay apart.
#[test]
fn hand_opened_prs_in_two_orgs_repos_of_one_name_stay_apart() {
    let b = board_with(|c, _| c.owner_emails = vec!["sam@acme.dev".into()]);
    b.post("/pr-builds", json!({"stopped": true}));
    for repo in ["acme/webapp", "globex/webapp"] {
        b.post("/prs/event", json!({"kind": "pr", "host": "github", "repo": repo, "num": 5, "mine": true, "branch": format!("{repo}-b")}));
    }
    let noted = || -> Vec<String> {
        let v: Value = serde_json::from_str(&b.app.db.get_setting("pr_builds_owner_prs").unwrap().unwrap()).unwrap();
        v["prs"].as_array().unwrap().iter().map(|p| p["repo"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!(noted(), vec!["acme/webapp", "globex/webapp"]);
    let build = |repo: &str| b.post("/prs/event", json!({"kind": "build", "state": "started", "host": "github", "repo": repo, "num": 5, "head": "q1"}))["builds"].clone();
    assert_eq!(build("Globex/webapp")["pr"], 5);
    assert_eq!(build("webapp")["owners"], false, "the short name names neither");
    b.post("/prs/event", json!({"kind": "pr", "host": "github", "repo": "GLOBEX/webapp", "num": 5, "mine": true, "state": "merged"}));
    assert_eq!(noted(), vec!["acme/webapp"], "only globex's PR closed");
}

/// #91: a cancel command that writes an empty $TB_CANCELLED stopped nothing, so even its first round
/// records nothing, as on the Python board; the push still isn't cancelled again on a poll.
#[test]
fn a_cancel_command_that_stopped_nothing_records_nothing() {
    let b = board_with(|c, _| {
        c.pr_builds.cancel.insert("azure".into(), ": > \"$TB_CANCELLED\"".into());
    });
    let id = b.pr_task(GH);
    b.post("/pr-builds", json!({"stopped": true}));
    b.post("/prs/event", json!({"url": GH, "kind": "build", "state": "started", "head": "h7", "provider": "azure"}));
    prbuilds::tick(&b.app).unwrap();
    assert_eq!(b.get("/pr-builds")["recent"], json!([]));
    let logged = b.app.db.count("SELECT COUNT(*) FROM events WHERE task_id = ? AND text LIKE 'PR builds are stopped: cancelled%'", vec![json!(id)]).unwrap();
    assert_eq!(logged, 0);
    assert!(b.flow(id)["builds_cancelled"]["h7"].is_string());
    assert_eq!(b.queue()[0]["round"], 1, "still followed up");
}
