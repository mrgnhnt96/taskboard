//! The work event stream, protocol v1 (#76): one endpoint takes one event or a batch, answers each
//! event on its own (accepted, duplicate, rejected with why), keeps each payload once by its id and
//! routes pr, build and heartbeat to the PR feed and jira.comment to QA comments. Each source (#77)
//! has a mode, board or stream, and its own health; in stream mode a complete pr event is the PR's
//! state and the source's own poll rests while its stream is healthy.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::prhost::{self, Check, FakeHost, Record};
use taskboardd::stream::SourceConfig;
use taskboardd::util::{iso, now_ts, Row, RowExt};
use taskboardd::{board, feed, midna, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board() -> Board {
    board_with(|_| {})
}

fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
    f(&mut cfg);
    cfg.jira.site = "acme.atlassian.net".into();
    cfg.jira.project = "PROJ".into();
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
    fn mode_err(&self, body: Value) -> String {
        api::dispatch(&self.app, "POST", "/stream/mode", &Query::new(), &body).expect_err("should fail").message
    }
    fn source(&self, name: &str) -> Value {
        self.get("/stream")["sources"].as_array().unwrap().iter().find(|s| s["source"] == name).cloned().unwrap()
    }
    fn feed_state(&self, name: &str, st: Value) {
        let key = if name == "prs" { "pr_feed".to_string() } else { format!("stream_feed_{name}") };
        self.app.db.set_setting(&key, Some(&st.to_string())).unwrap();
    }
    fn rec(&self, id: i64) -> Value {
        let f: Value = serde_json::from_str(&board::get_task(&self.app, id).unwrap().st("pr_flow")).unwrap();
        f["rec"].clone()
    }
    fn send(&self, body: Value) -> Value {
        self.post("/stream/events", body)
    }
    fn send_err(&self, body: Value) -> (u16, String) {
        let e = api::dispatch(&self.app, "POST", "/stream/events", &Query::new(), &body).expect_err("should fail");
        (e.status, e.message)
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
    fn kept(&self, event_id: &str) -> Option<Row> {
        self.app.db.q1("SELECT * FROM stream_events WHERE event_id = ?", taskboardd::p![event_id]).unwrap()
    }
}

const BB: &str = "https://bitbucket.org/acme/webapp/pull-requests/9";

fn ev(id: &str, kind: &str, extra: Value) -> Value {
    let mut e = json!({"id": id, "kind": kind, "at": "2026-10-12T15:04:00Z", "source": "relay"});
    for (k, v) in extra.as_object().unwrap() {
        e[k] = v.clone();
    }
    e
}

fn ago(secs: f64) -> String {
    iso(now_ts() - secs)
}

fn stream_mode(name: &str) -> impl FnOnce(&mut Config) + '_ {
    move |c| {
        c.stream.sources.insert(name.into(), SourceConfig { mode: "stream".into(), ..Default::default() });
    }
}

fn host_rec(head: &str) -> Record {
    Record {
        state: "OPEN".into(),
        author: "me".into(),
        head: head.into(),
        base: "main".into(),
        checks: vec![Check { name: "build".into(), state: "passed".into(), url: None, at: None }],
        ..Default::default()
    }
}

fn batch(events: Vec<Value>) -> Value {
    json!({"v": 1, "events": events})
}

fn full_pr(id: &str) -> Value {
    ev(id, "pr", json!({"provider": "bitbucket", "repo": "acme/webapp", "num": 9, "url": BB, "title": "Login", "author": "me",
        "state": "open", "draft": false, "branch": "login", "base": "main", "head": "h1", "mergeable": true,
        "checks": [{"name": "build", "state": "passed", "url": null, "started_at": "2026-10-12T15:00:00Z", "finished_at": "2026-10-12T15:03:00Z"}],
        "reviewers": [], "threads": []}))
}

#[test]
fn the_envelope_names_its_version_and_holds_events() {
    let b = board();
    assert!(b.send_err(json!({"events": [ev("e1", "heartbeat", json!({}))]})).1.contains("protocol version"));
    let (code, msg) = b.send_err(json!({"v": 2, "events": [ev("e1", "heartbeat", json!({}))]}));
    assert_eq!((code, msg.as_str()), (400, "This board speaks stream protocol v1, not v2."));
    assert_eq!(b.send_err(batch(vec![])).1, "events is empty.");
    assert_eq!(b.send_err(json!({"v": 1, "events": {}})).1, "events is a list of events.");
    assert!(b.send_err(json!({"v": 1})).1.starts_with("Send events"));

    let mut one = ev("e1", "heartbeat", json!({}));
    one["v"] = json!(1);
    let r = b.send(one);
    assert_eq!(r["accepted"], 1, "one event on its own needs no list: {r}");
    assert_eq!(r["results"][0], json!({"id": "e1", "kind": "heartbeat", "status": "accepted", "result": r["results"][0]["result"]}));
}

#[test]
fn each_event_is_answered_on_its_own_and_kept_once() {
    let b = board();
    let push = ev("p1", "push", json!({"repo": "acme/webapp", "branch": "main", "before": "a", "after": "b", "forced": false,
        "commits": [{"sha": "b", "message": "Fix", "author": "sam"}]}));
    let r = b.send(batch(vec![push.clone(), ev("bad", "push", json!({"repo": "acme/webapp"})), push.clone(), json!("x")]));
    assert_eq!((r["accepted"].as_i64(), r["duplicate"].as_i64(), r["rejected"].as_i64()), (Some(1), Some(1), Some(2)), "{r}");
    let res = r["results"].as_array().unwrap();
    assert_eq!(res[0]["status"], "accepted");
    assert_eq!(res[1]["why"], "branch is missing.");
    assert_eq!(res[2]["status"], "duplicate", "the same id twice in one batch is kept once");
    assert_eq!(res[3]["why"], "An event is an object.");

    assert_eq!(b.send(batch(vec![push]))["results"][0]["status"], "duplicate", "a resend later is a duplicate too");
    let kept = b.kept("p1").unwrap();
    assert_eq!(kept.st("kind"), "push");
    assert_eq!(kept.st("source"), "relay");
    let data: Value = serde_json::from_str(&kept.st("data")).unwrap();
    assert_eq!(data["commits"][0]["message"], "Fix", "the whole payload is kept for later");
    assert!(b.kept("bad").is_none());
}

#[test]
fn review_comment_and_jira_issue_events_are_kept_for_later() {
    let b = board();
    let pr = json!({"provider": "bitbucket", "repo": "acme/webapp", "num": 9});
    let r = b.send(batch(vec![
        ev("r1", "review", json!({"pr": pr, "who": "pat", "state": "changes", "body": "Rename it"})),
        ev("c1", "comment", json!({"pr": pr, "thread_id": "t1", "comment_id": "c1", "author": "pat", "body": "Why?", "path": "a.rs", "line": 4})),
        ev("j1", "jira.issue", json!({"key": "PROJ-7", "summary": "Login", "type": "Story", "status": "In Review", "status_category": "indeterminate",
            "from_status": "In Progress", "assignee": "me", "reporter": "sam", "changed_by": "sam", "epic": "PROJ-1"})),
    ]));
    assert_eq!(r["accepted"], 3, "{r}");
    for id in ["r1", "c1", "j1"] {
        let kept = b.kept(id).unwrap();
        assert_eq!(kept.i("complete"), Some(1));
        assert_eq!(kept.st("result"), "null");
    }
}

#[test]
fn a_pr_event_refreshes_its_pr_and_says_whether_it_was_complete() {
    let b = board();
    let id = b.pr_task(BB);
    let h = FakeHost::new("bitbucket", Record {
        state: "OPEN".into(),
        author: "me".into(),
        head: "h1".into(),
        base: "main".into(),
        checks: vec![Check { name: "build".into(), state: "passed".into(), url: None, at: None }],
        ..Default::default()
    });
    prhost::install(&b.app, h);
    let r = b.send(batch(vec![full_pr("pr1"), ev("pr2", "pr", json!({"repo": "acme/webapp", "num": 9, "provider": "bitbucket"}))]));
    let res = r["results"].as_array().unwrap();
    assert_eq!(res[0]["complete"], true);
    assert_eq!(res[0]["result"]["task"], format!("T{id}"), "{r}");
    assert_eq!(res[0]["result"]["refreshed"], true);
    assert_eq!(res[1]["complete"], false, "missing fields: kept, but the board reads that PR from its host");
    assert_eq!(b.kept("pr1").unwrap().i("complete"), Some(1));
    assert_eq!(b.kept("pr2").unwrap().i("complete"), Some(0));
    assert_eq!(board::get_task(&b.app, id).unwrap().st("pr_phase"), "review");
}

#[test]
fn build_and_heartbeat_events_reach_their_sources() {
    let b = board();
    let r = b.send(batch(vec![
        ev("b1", "build", json!({"repo": "acme/webapp", "branch": "main", "head": "abc", "state": "failed", "pipeline": "ci",
            "url": "https://ci.example/1", "author": "sam", "provider": "bitbucket-pipelines"})),
        ev("h1", "heartbeat", json!({"sources": ["prs", "builds"], "active": true, "connected_at": "2026-10-12T15:00:00Z"})),
    ]));
    assert_eq!(r["accepted"], 2, "{r}");
    assert_eq!(r["results"][0]["result"]["kind"], "build");
    assert_eq!(r["results"][1]["result"]["sources"], json!(["prs", "builds"]));
    let builds = b.source("builds");
    assert_eq!(builds["last_event"]["kind"], "build", "{builds}");
    assert!(builds["last_heartbeat_at"].is_string());
    let prs = b.source("prs");
    assert_eq!(prs["last_event"], Value::Null, "a build is the builds source's: {prs}");
    assert_eq!(prs["active"], true);
    assert!(prs["last_heartbeat_at"].is_string());
    assert_eq!(b.source("qa")["last_heartbeat_at"], Value::Null, "a heartbeat covers only the sources it names");
}

#[test]
fn a_jira_comment_becomes_a_qa_comment() {
    let b = board();
    let g = b.post("/goals", json!({"name": "Login", "project": "webapp"}))["id"].as_i64().unwrap();
    let t = b.post("/tasks", json!({"title": "Add login", "project": "webapp", "goal_id": g}))["id"].as_i64().unwrap();
    board::update_task(&b.app, t, taskboardd::fields!["status" => "done", "jira_key" => "PROJ-7", "pr_num" => 12]).unwrap();
    b.post("/qa", json!({"on": true}));
    let c = ev("jc1", "jira.comment", json!({"key": "PROJ-7", "comment_id": "100", "author": "Sam QA", "body": "Logo overlaps", "visibility": null}));
    let r = b.send(batch(vec![c.clone()]));
    assert_eq!(r["results"][0]["result"]["new"], true, "{r}");
    let q = b.app.db.q1("SELECT * FROM qa_comments WHERE comment_id = '100'", taskboardd::p![]).unwrap().unwrap();
    assert_eq!(q.st("text"), "Logo overlaps");
    assert_eq!(q.st("author"), "Sam QA");
}

#[test]
fn the_old_endpoints_still_work() {
    let b = board();
    b.post("/prs/heartbeat", json!({}));
    assert_eq!(b.post("/prs/event", json!({"repo": "acme/other", "num": 3}))["task"], Value::Null);
    assert!(b.post("/jira/comment", json!({"key": "PROJ-9", "comment_id": "101"}))["ignored"].is_string());
}

#[test]
fn a_source_s_mode_comes_from_the_board_then_config_toml_then_board() {
    let b = board_with(stream_mode("qa"));
    let modes = |b: &Board| -> Vec<(String, String)> {
        b.get("/stream")["sources"].as_array().unwrap().iter().map(|s| (s["mode"].as_str().unwrap().into(), s["mode_from"].as_str().unwrap().into())).collect()
    };
    let m = modes(&b);
    assert_eq!(m[0], ("board".into(), "default".into()), "prs: today's behaviour");
    assert_eq!(m[3], ("stream".into(), "config".into()), "qa: config.toml");

    let r = b.post("/stream/mode", json!({"source": "prs", "mode": "stream"}));
    assert_eq!(r["sources"][0]["mode_from"], "override", "{r}");
    b.post("/stream/mode", json!({"source": "qa", "mode": "Board"}));
    assert_eq!(modes(&b)[3], ("board".into(), "override".into()));
    b.post("/stream/mode", json!({"source": "qa", "mode": "default"}));
    assert_eq!(modes(&b)[3], ("stream".into(), "config".into()), "default drops the override");
    assert_eq!(modes(&b)[0].0, "stream");

    assert_eq!(b.mode_err(json!({"source": "ci", "mode": "stream"})), "source is one of prs, builds, jira, qa.");
    assert_eq!(b.mode_err(json!({"source": "prs", "mode": "push"})), "mode is board, stream or default.");
}

#[test]
fn in_stream_mode_a_complete_pr_event_is_the_pr_s_state() {
    let b = board_with(stream_mode("prs"));
    let id = b.pr_task(BB);
    let h = FakeHost::new("bitbucket", host_rec("h0"));
    prhost::install(&b.app, h.clone());
    prflow::refresh_task(&b.app, id).unwrap().unwrap();
    assert_eq!(b.rec(id)["head"], "h0", "the first read gives the board's account and the base head");

    *h.rec.lock() = host_rec("h9");
    let mut e = full_pr("pr1");
    e["head"] = json!("h1");
    e["reviewers"] = json!([{"who": "sam", "state": "approved"}, {"who": "kim", "state": "changes", "at": "2026-10-12T15:02:00Z"}]);
    e["threads"] = json!([{"id": "t1", "path": "src/a.rs", "line": 3, "resolved": false,
        "comments": [{"id": "c1", "author": "kim", "body": "Rename this", "at": "2026-10-12T15:01:00Z"}]}]);
    let r = b.send(batch(vec![e]));
    assert_eq!(r["results"][0]["result"]["applied"], true, "{r}");
    let rec = b.rec(id);
    assert_eq!(rec["head"], "h1", "the event, not the host (h9): {rec}");
    assert_eq!(rec["approvals"], 1);
    assert_eq!(rec["threads"][0]["text"], "Rename this");
    assert_eq!(rec["threads"][0]["kind"], "review");
    assert_eq!(rec["comments"], 1);

    let r = b.send(batch(vec![ev("pr2", "pr", json!({"repo": "acme/webapp", "num": 9, "provider": "bitbucket", "head": "h2"}))]));
    assert_eq!(r["results"][0]["result"]["applied"], false, "missing fields: {r}");
    assert_eq!(b.rec(id)["head"], "h9", "an incomplete event falls back to one host read");
}

#[test]
fn a_pr_with_no_read_yet_or_in_board_mode_is_read_from_its_host() {
    let b = board_with(stream_mode("prs"));
    let id = b.pr_task(BB);
    prhost::install(&b.app, FakeHost::new("bitbucket", host_rec("h9")));
    let r = b.send(batch(vec![full_pr("pr1")]));
    assert_eq!(r["results"][0]["result"]["applied"], false, "no prior read: {r}");
    assert_eq!(b.rec(id)["head"], "h9");

    b.post("/stream/mode", json!({"source": "prs", "mode": "board"}));
    let mut e = full_pr("pr2");
    e["head"] = json!("h1");
    let r = b.send(batch(vec![e]));
    assert_eq!(r["results"][0]["result"]["applied"], false, "board mode: a hint");
    assert_eq!(b.rec(id)["head"], "h9");
}

#[test]
fn health_is_kept_per_source() {
    let b = board_with(stream_mode("qa"));
    let qa = b.source("qa");
    assert_eq!((qa["watched"].as_bool(), qa["healthy"].as_bool()), (Some(true), Some(true)), "{qa}");
    assert_eq!(b.source("builds")["watched"], false, "board mode: not watched");

    b.send(batch(vec![ev("h1", "heartbeat", json!({"sources": ["qa"]}))]));
    assert!(b.source("qa")["last_heartbeat_at"].is_string());
    assert_eq!(b.source("prs")["last_heartbeat_at"], Value::Null);

    b.feed_state("qa", json!({"since": ago(4000.0), "last_heartbeat_at": ago(200.0)}));
    let qa = b.source("qa");
    assert_eq!(qa["problem"], "stuck", "{qa}");
    assert!(qa["why"].as_str().unwrap().contains("heartbeat for 3 minutes"), "{qa}");
    assert_eq!(b.source("prs")["problem"], Value::Null, "one source's trouble is its own");

    b.feed_state("qa", json!({"since": ago(4000.0)}));
    assert_eq!(b.source("qa")["problem"], "silent");
}

#[test]
fn a_stream_mode_source_s_poll_rests_while_its_stream_is_healthy() {
    let b = board_with(|c| {
        for s in ["prs", "builds", "qa"] {
            stream_mode(s)(c);
        }
        c.feed.poll_while_healthy = true;
    });
    for s in ["prs", "builds", "qa"] {
        b.feed_state(s, json!({"since": ago(10.0)}));
        assert!(feed::poll_rests(&b.app, s), "{s}: healthy, so its poll rests");
        assert_eq!(b.source(s)["polling"], false);
        b.feed_state(s, json!({"since": ago(4000.0), "last_heartbeat_at": ago(200.0)}));
        assert!(!feed::poll_rests(&b.app, s), "{s}: stuck, so its poll takes over");
    }
    assert!(feed::poll_due(&b.app));
    assert!(!feed::poll_rests(&b.app, "jira"), "board mode always polls");

    b.post("/stream/mode", json!({"source": "qa", "mode": "board"}));
    b.feed_state("qa", json!({"since": ago(10.0)}));
    assert!(!feed::poll_rests(&b.app, "qa"));
    assert_eq!(b.source("qa")["polling"], true);
}
