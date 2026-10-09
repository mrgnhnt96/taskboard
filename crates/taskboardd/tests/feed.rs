//! The PR feed (#3): an event refreshes its one PR, the feed's health (stuck, silent) holds the review
//! loop, the board restarts it at 1/5/15 minutes and then alerts, and a request for changes without
//! comments doesn't move the stage.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, Check, FakeHost, Record, Reviewer};
use taskboardd::util::{iso, now_ts, RowExt};
use taskboardd::{board, dispatch, feed, midna, prflow, reports};

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
    fn report(&self, event: &str, session: &str, extra: Value) {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap();
    }
    fn pr_task(&self, url: &str) -> i64 {
        let id = self.post("/tasks", json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", "s1", json!({"task": format!("T{id}")}));
        self.report("tb.done", "s1", json!({"summary": "Done", "pr": url}));
        id
    }
    fn phase(&self, id: i64) -> String {
        board::get_task(&self.app, id).unwrap().st("pr_phase")
    }
    /// Rewrites the feed's state (times in seconds before now).
    fn feed_state(&self, st: Value) {
        self.app.db.set_setting("pr_feed", Some(&st.to_string())).unwrap();
    }
    fn feed(&self) -> Value {
        serde_json::from_str(&self.app.db.get_setting("pr_feed").unwrap().unwrap_or("{}".into())).unwrap()
    }
}

const BB: &str = "https://bitbucket.org/acme/webapp/pull-requests/9";

fn ago(secs: f64) -> String {
    iso(now_ts() - secs)
}

fn green() -> Record {
    Record {
        state: "OPEN".into(),
        author: "me".into(),
        head: "h1".into(),
        base: "main".into(),
        checks: vec![Check { name: "build".into(), state: "passed".into(), url: None }],
        threads: vec![],
        ..Default::default()
    }
}

fn fake(b: &Board, rec: Record) -> Arc<FakeHost> {
    let h = FakeHost::new("bitbucket", rec);
    prhost::install(&b.app, h.clone());
    h
}

#[test]
fn an_event_reads_its_one_pr_again() {
    let b = board_with(|_, _| {});
    let id = b.pr_task(BB);
    assert_eq!(b.phase(id), "checks");
    fake(&b, green());
    let v = b.post("/prs/event", json!({"url": BB, "source": "slack"}));
    assert_eq!(v["task"], format!("T{id}"));
    assert_eq!(v["refreshed"], true);
    assert_eq!(v["phase"], "review");
    assert_eq!(b.phase(id), "review");
    let h = b.get("/prs/feed");
    assert_eq!(h["last_event"]["task"], format!("T{id}"));
    assert!(h["last_event_at"].is_string());

    let v = b.post("/prs/event", json!({"repo": "acme/other", "num": 3}));
    assert_eq!(v["task"], Value::Null, "a PR the board didn't make is only noted");
    let v = b.post("/prs/event", json!({"repo": "acme/webapp", "num": 9, "kind": "pr"}));
    assert_eq!(v["task"], format!("T{id}"), "repo and num find it too");
    assert!(api::dispatch(&b.app, "POST", "/prs/event", &Query::new(), &json!({"kind": "nope"})).is_err());
}

#[test]
fn without_the_feed_it_is_always_healthy_and_the_poll_runs() {
    let b = board_with(|_, _| {});
    b.feed_state(json!({"last_heartbeat_at": ago(9999.0)}));
    assert!(feed::feed_healthy(&b.app));
    assert!(feed::poll_due(&b.app));
    assert_eq!(feed::holding(&b.app, false), None);
}

#[test]
fn a_stuck_or_silent_feed_holds_the_review_loop_and_the_poll_takes_over() {
    let b = board_with(|c, _| c.feed.on = true);
    b.feed_state(json!({"since": ago(10.0)}));
    assert!(feed::feed_healthy(&b.app));
    assert!(!feed::poll_due(&b.app), "a healthy feed drives the PRs; the poll rests");

    b.feed_state(json!({"since": ago(4000.0), "last_heartbeat_at": ago(200.0)}));
    let h = b.get("/prs/feed");
    assert_eq!(h["problem"], "stuck");
    assert!(h["why"].as_str().unwrap().contains("heartbeat for 3 minutes"), "{h}");
    assert!(!feed::feed_healthy(&b.app));
    assert!(feed::holding(&b.app, false).unwrap().starts_with("Holding reviewer asks"));
    assert!(feed::poll_due(&b.app));

    b.post("/prs/heartbeat", json!({}));
    assert!(feed::feed_healthy(&b.app));

    b.feed_state(json!({"since": ago(4000.0)}));
    assert_eq!(b.get("/prs/feed")["problem"], "silent", "nothing at all for over an hour (the hours are off: always open)");
    b.post("/prs/event", json!({"url": BB}));
    assert!(feed::feed_healthy(&b.app), "an event counts");
    assert_eq!(b.get("/state")["pr_feed"]["healthy"], true);
}

#[test]
fn the_board_restarts_a_bad_feed_at_1_5_15_minutes_then_alerts() {
    let b = board_with(|c, dir| {
        c.feed.on = true;
        c.feed.restart_cmd = format!("echo restarted >> '{}'", dir.join("restarts.log").display());
    });
    let log = b.dir.path().join("restarts.log");
    let restarts = || std::fs::read_to_string(&log).map(|s| s.lines().count()).unwrap_or(0);
    let stuck = |bad_for: f64, done: usize| {
        let mut st = json!({"since": ago(9000.0), "last_heartbeat_at": ago(9000.0), "unhealthy_since": ago(bad_for)});
        st["restarts"] = json!(vec![json!({"ok": true}); done]);
        st
    };
    b.feed_state(json!({"since": ago(9000.0), "last_heartbeat_at": ago(9000.0)}));
    feed::check(&b.app).unwrap();
    assert!(b.feed()["unhealthy_since"].is_string(), "it notes when the feed went bad");
    assert_eq!(restarts(), 0, "and waits a minute");

    b.feed_state(stuck(61.0, 0));
    feed::check(&b.app).unwrap();
    assert_eq!(restarts(), 1);
    feed::check(&b.app).unwrap();
    assert_eq!(restarts(), 1, "the next restart waits for 5 minutes");
    b.feed_state(stuck(301.0, 1));
    feed::check(&b.app).unwrap();
    b.feed_state(stuck(901.0, 2));
    feed::check(&b.app).unwrap();
    assert_eq!(restarts(), 3);
    assert!(dispatch::alerts(&b.app).is_empty(), "no alert while restarting might still help");

    b.feed_state(stuck(901.0 + 300.0, 3));
    feed::check(&b.app).unwrap();
    assert_eq!(restarts(), 3, "three restarts and no more");
    let alerts = dispatch::alerts(&b.app);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0]["key"], feed::ALERT_KEY);
    assert!(alerts[0]["text"].as_str().unwrap().contains("still stuck after 3 restarts"), "{}", alerts[0]);

    b.post("/prs/heartbeat", json!({}));
    feed::check(&b.app).unwrap();
    assert!(dispatch::alerts(&b.app).is_empty(), "healthy again: the alert clears");
    assert!(b.feed().get("unhealthy_since").is_none());
}

#[test]
fn a_failed_restart_alerts_at_once() {
    let b = board_with(|c, _| {
        c.feed.on = true;
        c.feed.restart_cmd = "echo 'no such agent' >&2; exit 3".into();
    });
    b.feed_state(json!({"since": ago(9000.0), "last_heartbeat_at": ago(9000.0), "unhealthy_since": ago(61.0)}));
    feed::check(&b.app).unwrap();
    let alerts = dispatch::alerts(&b.app);
    assert_eq!(alerts.len(), 1);
    assert!(alerts[0]["text"].as_str().unwrap().contains("Restarting it failed: restart_cmd: no such agent"), "{}", alerts[0]);
    assert_eq!(b.feed()["restarts"][0]["ok"], false);
}

#[test]
fn changes_requested_without_comments_does_not_move_the_stage() {
    let b = board_with(|_, _| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.review_decision = "CHANGES_REQUESTED".into();
    rec.changes_at = Some("t1".into());
    rec.reviewers = vec![Reviewer { user: "rev".into(), name: "Rev".into(), state: "changes".into(), requested: true }];
    let h = fake(&b, rec);
    prflow::refresh(&b.app).unwrap();
    assert_eq!(b.phase(id), "review", "a bare request for changes is no work to do");
    assert_eq!(board::get_task(&b.app, id).unwrap().st("pr_review"), "Not reviewed yet");

    h.rec.lock().threads = vec![prhost::Thread {
        id: "1".into(),
        kind: "summary".into(),
        author: "rev".into(),
        last_author: "rev".into(),
        last_id: "1".into(),
        text: "Rename this".into(),
        ..Default::default()
    }];
    prflow::refresh(&b.app).unwrap();
    assert_eq!(b.phase(id), "comments", "with a comment it counts");
}

/// Work hours that aren't open now: only tomorrow.
fn closed_hours(b: &Board) {
    let tomorrow = chrono::Local::now().date_naive().succ_opt().unwrap().format("%a").to_string().to_lowercase();
    b.post("/hours", json!({"on": true, "start": "00:00", "end": "23:59", "days": tomorrow}));
}

#[test]
fn outside_work_hours_only_a_pr_s_first_ask_goes_out() {
    let b = board_with(|c, _| c.feed.on = true);
    b.feed_state(json!({"since": ago(9000.0), "last_heartbeat_at": ago(5.0), "connected_at": ago(9000.0)}));
    assert_eq!(feed::holding(&b.app, false), None, "inside the hours: nothing held");
    closed_hours(&b);
    assert!(feed::feed_healthy(&b.app), "a quiet feed outside the hours isn't a problem");
    let why = feed::holding(&b.app, false).unwrap();
    assert!(why.contains("outside the work hours"), "{why}");
    assert_eq!(feed::holding(&b.app, true), None, "a PR's first ask still goes out");
    assert!(b.get("/prs/feed")["off_hours"].is_string());
}

#[test]
fn a_listener_s_own_hours_keep_it_from_looking_stuck() {
    let b = board_with(|c, _| c.feed.on = true);
    b.feed_state(json!({"since": ago(9000.0), "last_heartbeat_at": ago(9000.0), "connected_at": ago(9000.0)}));
    b.post("/prs/heartbeat", json!({"active": false, "idle_until": iso(now_ts() + 3600.0)}));
    // It stops heartbeating at night.
    let mut st = b.feed();
    st["last_heartbeat_at"] = json!(ago(4000.0));
    b.feed_state(st.clone());
    assert!(feed::feed_healthy(&b.app), "idle: not stuck, not silent");
    feed::check(&b.app).unwrap();
    assert!(b.feed().get("unhealthy_since").is_none(), "no restarts, no alert");
    assert!(feed::holding(&b.app, false).unwrap().contains("listener is idle until"));
    assert_eq!(feed::holding(&b.app, true), None, "the first ask goes");

    // Its idle_until came and went with no heartbeat: timed from then, so stuck.
    st["idle_until"] = json!(ago(200.0));
    b.feed_state(st.clone());
    assert_eq!(b.get("/prs/feed")["problem"], "stuck");
    st["idle_until"] = json!(ago(60.0));
    b.feed_state(st);
    assert!(feed::feed_healthy(&b.app), "not yet stuck_secs after it was due back");

    // Back: `active` after idle is a connect, so it settles first; then nothing is held, whatever the work hours.
    b.post("/prs/heartbeat", json!({"activeNow": true}));
    assert!(feed::holding(&b.app, true).unwrap().contains("just came back"));
    let mut st = b.feed();
    st["connected_at"] = json!(ago(400.0));
    b.feed_state(st);
    closed_hours(&b);
    assert_eq!(feed::holding(&b.app, false), None, "the listener says it's in its hours");
    assert!(api::dispatch(&b.app, "POST", "/prs/heartbeat", &Query::new(), &json!({"idle_until": "soon"})).is_err());
}

#[test]
fn every_connect_starts_the_settle_window() {
    let b = board_with(|c, _| c.feed.on = true);
    b.feed_state(json!({"since": ago(10.0)}));
    assert_eq!(feed::holding(&b.app, true), None);
    b.post("/prs/heartbeat", json!({}));
    assert!(feed::holding(&b.app, true).unwrap().contains("just came back"), "the first connect");
    assert!(b.get("/prs/feed")["settling_secs"].as_f64().unwrap() > 290.0);

    b.feed_state(json!({"since": ago(9000.0), "last_heartbeat_at": ago(5.0), "connected_at": ago(400.0)}));
    b.post("/prs/heartbeat", json!({}));
    assert_eq!(feed::holding(&b.app, true), None, "a plain heartbeat isn't a connect");
    b.post("/prs/heartbeat", json!({"connected_at": ago(20.0)}));
    let left = b.get("/prs/feed")["settling_secs"].as_f64().unwrap();
    assert!(left > 270.0 && left < 290.0, "a quick reconnect, timed from when it connected: {left}");
    b.post("/prs/heartbeat", json!({"connected_at": ago(20.0)}));
    assert!(b.get("/prs/feed")["settling_secs"].as_f64().unwrap() < 290.0, "the same connect again doesn't restart it");
}
