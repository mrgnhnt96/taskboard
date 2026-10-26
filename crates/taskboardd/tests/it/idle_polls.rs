//! How often the timed poll reads each open PR (#677): less often the longer it stays the same, from
//! the start again on a change or a stream event, hourly off hours unless it can move at night, and
//! hourly once its checks look stuck, with one alert per push.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::{Config, PrProject};
use taskboardd::prhost::{self, Check, FakeHost, Record, Reviewer};
use taskboardd::util::{iso, now_ts, RowExt};
use taskboardd::{board, dispatch, midna, prflow, prpace, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    cfg.pr.pace_polls = true;
    f(&mut cfg);
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
    /// Sets the PR's last read and last change (seconds ago).
    fn still(&self, id: i64, read: f64, moved: f64) {
        prflow::merge_flow(&self.app, id, vec![("read_at", json!(ago(read))), ("moved_at", json!(ago(moved)))]).unwrap();
    }
    fn every(&self, id: i64) -> f64 {
        prpace::every(&self.app, &board::get_task(&self.app, id).unwrap()).unwrap()
    }
    fn flow(&self, id: i64) -> Value {
        serde_json::from_str(&board::get_task(&self.app, id).unwrap().st("pr_flow")).unwrap()
    }
    fn set_phase(&self, id: i64, phase: &str) {
        self.app.db.x("UPDATE tasks SET pr_phase = ? WHERE id = ?", vec![json!(phase), json!(id)]).unwrap();
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
        checks: vec![Check { name: "build".into(), state: "passed".into(), url: None, at: None }],
        threads: vec![],
        ..Default::default()
    }
}

fn fake(b: &Board, rec: Record) -> Arc<FakeHost> {
    let h = FakeHost::new("bitbucket", rec);
    prhost::install(&b.app, h.clone());
    h
}

fn reads(h: &FakeHost) -> usize {
    *h.reads.lock()
}

fn closed_hours(b: &Board) {
    let tomorrow = chrono::Local::now().date_naive().succ_opt().unwrap().format("%a").to_string().to_lowercase();
    b.post("/hours", json!({"on": true, "start": "00:00", "end": "23:59", "days": tomorrow}));
}

#[test]
fn a_still_pr_is_read_less_often_the_longer_it_stays_the_same() {
    let b = board_with(|_| {});
    let h = fake(&b, green());
    let id = b.pr_task(BB);
    prflow::poll(&b.app).unwrap();
    let n = reads(&h);
    assert!(n >= 1, "a PR never read is read");
    prflow::poll(&b.app).unwrap();
    assert_eq!(reads(&h), n, "read a moment ago: not yet");

    for (still, every) in [(60.0, 180.0), (1800.0, 600.0), (7200.0, 1800.0), (30000.0, 3600.0)] {
        b.still(id, 0.0, still);
        assert_eq!(b.every(id), every, "still for {still}s");
    }

    b.still(id, 700.0, 8000.0);
    prflow::poll(&b.app).unwrap();
    assert_eq!(reads(&h), n, "still for two hours: every 30 minutes");
    b.still(id, 1800.0, 8000.0);
    prflow::poll(&b.app).unwrap();
    assert_eq!(reads(&h), n + 1, "30 minutes since the last read");
    assert!(b.every(id) >= 1800.0, "an unchanged read doesn't start it over");
}

#[test]
fn without_pace_polls_every_pr_is_read_on_every_poll() {
    let b = board_with(|c| c.pr.pace_polls = false);
    let h = fake(&b, green());
    b.pr_task(BB);
    prflow::poll(&b.app).unwrap();
    let n = reads(&h);
    prflow::poll(&b.app).unwrap();
    assert_eq!(reads(&h), n + 1);
}

#[test]
fn a_change_or_a_stream_event_starts_the_pace_over() {
    let b = board_with(|_| {});
    let h = fake(&b, green());
    let id = b.pr_task(BB);
    prflow::poll(&b.app).unwrap();

    b.still(id, 3700.0, 30000.0);
    prflow::poll(&b.app).unwrap();
    assert_eq!(b.every(id), 3600.0, "read again, the same: still hourly");

    b.still(id, 3700.0, 30000.0);
    h.rec.lock().title = "Ship it, renamed".into();
    prflow::poll(&b.app).unwrap();
    assert_eq!(b.every(id), 180.0, "the read found a change");

    b.still(id, 3700.0, 30000.0);
    h.rec.lock().base_head = "b2".into();
    prflow::poll(&b.app).unwrap();
    assert_eq!(b.every(id), 3600.0, "a new commit on the base isn't the PR moving");

    b.still(id, 0.0, 30000.0);
    b.post("/prs/event", json!({"url": BB, "source": "stream"}));
    assert_eq!(b.every(id), 180.0, "a stream event for it");
}

#[test]
fn off_hours_only_a_pr_that_can_move_at_night_keeps_its_pace() {
    let b = board_with(|c| c.pr.agents_merge = Some(false));
    let h = fake(&b, green());
    let id = b.pr_task(BB);
    b.post("/reviewers", json!({"project": "webapp", "reviewer": "Ana", "user": "{ana}"}));
    b.post("/reviewers/bot", json!({"project": "webapp", "reviewer": "Ana", "every_h": 4, "mark": "ai review"}));
    h.rec.lock().reviewers = vec![Reviewer { user: "{ana}".into(), name: "Ana".into(), state: "pending".into(), requested: true, ..Default::default() }];
    prflow::poll(&b.app).unwrap();
    b.set_phase(id, "review");
    b.still(id, 0.0, 60.0);
    assert_eq!(b.every(id), 180.0, "inside the hours");

    closed_hours(&b);
    assert_eq!(b.every(id), 3600.0, "Ana's bot hasn't been seen to run: nothing moves at night");
    let ana = b.app.db.val("SELECT id FROM reviewers WHERE name = 'Ana'", vec![]).unwrap();
    b.app.db.x("INSERT INTO reviewer_bot_runs(reviewer_id, at, ref) VALUES (?, ?, 'r1')", vec![ana, json!(ago(3600.0))]).unwrap();
    assert_eq!(b.every(id), 180.0, "her bot runs every 4 hours: it can review at night");

    b.set_phase(id, "merge");
    assert_eq!(b.every(id), 3600.0, "approved and green, but the owner merges");
    taskboardd::projects::set_agents_merge(&b.app, Some(true)).unwrap();
    assert_eq!(b.every(id), 180.0, "the agents merge it at night");

    b.set_phase(id, "comments");
    assert_eq!(b.every(id), 3600.0, "comments wait for the morning, or the stream");
}

#[test]
fn checks_far_past_their_wait_are_read_hourly_and_raised_once_per_push() {
    let b = board_with(|c| {
        c.pr.projects.insert("webapp".into(), PrProject { expected_wait_mins: Some(30.0), ..Default::default() });
    });
    let mut rec = green();
    rec.checks = vec![Check { name: "build".into(), state: "running".into(), url: None, at: None }];
    let h = fake(&b, rec);
    let id = b.pr_task(BB);
    prflow::poll(&b.app).unwrap();
    assert_eq!(board::get_task(&b.app, id).unwrap().st("pr_phase"), "checks");
    assert_eq!(b.every(id), 180.0, "running within its wait");

    let stuck = |b: &Board| dispatch::alerts(&b.app).into_iter().filter(|a| a["text"].as_str().unwrap_or("").contains("looks stuck")).count();
    // 100 minutes on this push: past three times its 30-minute wait.
    prflow::merge_flow(&b.app, id, vec![("head_at", json!({"h1": now_ts() - 6000.0}))]).unwrap();
    b.still(id, 200.0, 200.0);
    assert_eq!(b.every(id), 3600.0, "stuck checks are read hourly");
    prflow::poll(&b.app).unwrap();
    assert_eq!(stuck(&b), 0, "not read yet");
    b.still(id, 3600.0, 3600.0);
    prflow::poll(&b.app).unwrap();
    assert_eq!(stuck(&b), 1);
    assert_eq!(b.flow(id)["checks_stuck"], "h1");

    let n = reads(&h);
    b.still(id, 600.0, 60.0);
    prflow::poll(&b.app).unwrap();
    assert_eq!(reads(&h), n, "not at the checks rate");
    b.still(id, 3600.0, 60.0);
    prflow::poll(&b.app).unwrap();
    assert_eq!(reads(&h), n + 1);
    assert_eq!(stuck(&b), 1, "raised once per push");

    h.rec.lock().head = "h2".into();
    b.still(id, 3600.0, 60.0);
    prflow::poll(&b.app).unwrap();
    assert_eq!(b.every(id), 180.0, "a new push starts its wait over");
}
