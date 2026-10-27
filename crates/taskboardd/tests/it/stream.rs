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
use taskboardd::{board, dispatch, feed, midna, prflow, reports};

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
    // These PR tasks are about the stream, not their tickets.
    cfg.pr_body.require_ticket = Some(false);
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
fn review_comment_and_jira_issue_events_are_kept() {
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
        assert_eq!(b.kept(id).unwrap().i("complete"), Some(1));
    }
    for id in ["r1", "c1"] {
        assert_eq!(b.kept(id).unwrap().st("result"), "null");
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
fn a_review_or_comment_on_a_board_pr_reads_it_like_a_pr_hint() {
    let b = board();
    let id = b.pr_task(BB);
    let h = FakeHost::new("bitbucket", host_rec("h7"));
    prhost::install(&b.app, h.clone());
    let pr = json!({"provider": "bitbucket", "repo": "acme/webapp", "num": 9});
    let r = b.send(batch(vec![ev("r1", "review", json!({"pr": pr, "who": "pat", "state": "changes"}))]));
    let res = &r["results"][0]["result"];
    assert_eq!(res["task"], format!("T{id}"), "{r}");
    assert_eq!(res["kind"], "review");
    assert_eq!(res["refreshed"], true);
    assert_eq!(b.rec(id)["head"], "h7", "the PR was read from its host");

    h.rec.lock().head = "h8".into();
    let r = b.send(batch(vec![ev("c1", "comment", json!({"pr": {"url": BB}, "comment_id": "c1", "author": "pat", "body": "Why?"}))]));
    assert_eq!(r["results"][0]["result"]["task"], format!("T{id}"), "{r}");
    assert_eq!(b.rec(id)["head"], "h8", "a comment reads it again too");

    h.rec.lock().head = "h9".into();
    let other = json!({"provider": "bitbucket", "repo": "acme/other", "num": 3});
    let r = b.send(batch(vec![ev("r2", "review", json!({"pr": other, "who": "pat", "state": "approved"}))]));
    assert_eq!(r["results"][0]["status"], "accepted");
    assert!(r["results"][0].get("result").is_none(), "a PR off the board isn't read: {r}");
    assert_eq!(b.rec(id)["head"], "h8");
    assert_eq!(b.source("prs")["last_event"]["kind"], "review", "it still counts for health");
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
fn a_pr_event_with_no_checks_keeps_the_saved_checks_for_the_same_head() {
    let b = board_with(stream_mode("prs"));
    let id = b.pr_task(BB);
    let mut rec = host_rec("h1");
    rec.checks.push(Check { name: "lint".into(), state: "failed".into(), url: None, at: None });
    prhost::install(&b.app, FakeHost::new("bitbucket", rec));
    prflow::refresh_task(&b.app, id).unwrap().unwrap();

    let mut e = full_pr("pr1");
    e["checks"] = json!([]);
    let r = b.send(batch(vec![e]));
    assert_eq!(r["results"][0]["result"]["applied"], true, "{r}");
    let rec = b.rec(id);
    assert_eq!(rec["checks"].as_array().map(Vec::len), Some(2), "same head h1, the host's checks stay: {rec}");
    assert_eq!(rec["failed"], json!(["lint"]));

    let mut e = full_pr("pr2");
    e["head"] = json!("h2");
    e["checks"] = json!([]);
    b.send(batch(vec![e]));
    let rec = b.rec(id);
    assert_eq!(rec["checks"], json!([]), "a new head's checks start over: {rec}");
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
    assert_eq!(b.get("/state")["stream"], b.get("/stream"), "the app reads the same overview from /state");
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

// --- relays (#205) ---

fn from(relay: &str, mut e: Value) -> Value {
    e["source"] = json!(relay);
    e["v"] = json!(1);
    e
}

#[test]
fn a_relay_covering_only_bitbucket_leaves_github_prs_polled() {
    let b = board_with(stream_mode("prs"));
    let gh = FakeHost::new("github", Record { title: "read from GitHub".into(), ..host_rec("g1") });
    let bb = FakeHost::new("bitbucket", Record { title: "read from Bitbucket".into(), ..host_rec("b1") });
    prhost::install(&b.app, gh.clone());
    prhost::install(&b.app, bb.clone());
    let gh_task = b.pr_task("https://github.com/acme/webapp/pull/7");
    let bb_task = b.pr_task(BB);
    b.send(from("bb-relay", ev("h1", "heartbeat", json!({"sources": ["prs", "builds"], "covers": [{"provider": "bitbucket"}]}))));
    b.send(from("gh-relay", ev("r1", "review", json!({"pr": {"provider": "github", "repo": "acme/other", "num": 3}, "who": "rev", "state": "approved"}))));

    assert!(feed::poll_due(&b.app), "no healthy relay covers every PR");
    assert!(feed::pr_poll_rests(&b.app, "bitbucket", "acme/webapp"));
    assert!(!feed::pr_poll_rests(&b.app, "github", "acme/webapp"), "gh-relay never sent a heartbeat, so it covers nothing");
    prflow::poll(&b.app).unwrap();
    let title = |id: i64| board::get_task(&b.app, id).unwrap().s("pr_title").map(str::to_string);
    assert_eq!(title(gh_task).as_deref(), Some("read from GitHub"), "the GitHub PR is polled");
    assert_eq!(title(bb_task).as_deref(), Some("read from Bitbucket"), "never read yet, so the Bitbucket PR is read once");

    gh.rec.lock().title = "GitHub again".into();
    bb.rec.lock().title = "Bitbucket again".into();
    prflow::poll(&b.app).unwrap();
    assert_eq!(title(gh_task).as_deref(), Some("GitHub again"), "the GitHub PR is polled every round");
    assert_eq!(title(bb_task).as_deref(), Some("read from Bitbucket"), "bb-relay drives the Bitbucket PR until its reconcile");

    let relays = b.source("prs")["relays"].as_array().unwrap().clone();
    let names: Vec<&str> = relays.iter().map(|r| r["relay"].as_str().unwrap()).collect();
    assert_eq!(names, ["bb-relay", "gh-relay"]);
    assert_eq!(relays[0]["covers"], json!([{"provider": "bitbucket"}]));
    assert!(relays[1]["last_heartbeat_at"].is_null() && relays[1]["last_event_at"].is_string());
    assert_eq!(relays[1]["covers"], json!([]), "gh-relay never sent a heartbeat, so it covers nothing");
    assert!(b.source("builds")["relays"][0]["relay"] == "bb-relay");
    assert!(b.send(from("r", ev("h2", "heartbeat", json!({"covers": [{"repo": "x"}]}))))["results"][0]["why"].as_str().unwrap().contains("provider"));
}

#[test]
fn one_relay_s_idle_doesn_t_hide_another_s_silence() {
    let b = board_with(stream_mode("prs"));
    let idle_until = iso(now_ts() + 3600.0);
    b.feed_state(
        "prs",
        json!({"since": ago(5000.0), "last_heartbeat_at": ago(10.0), "active": false, "idle_until": idle_until,
               "relays": {"a": {"since": ago(5000.0), "last_heartbeat_at": ago(10.0), "active": false, "idle_until": idle_until},
                          "b": {"since": ago(5000.0), "last_heartbeat_at": ago(4000.0), "active": true}}}),
    );
    let prs = b.source("prs");
    assert!(prs["problem"].is_string());
    assert!(prs["why"].as_str().unwrap().contains("relay b"), "{}", prs["why"]);
    assert_eq!(prs["relays"][0]["idle"], true);
    assert_eq!(prs["relays"][0]["healthy"], true);
    assert_eq!(prs["relays"][1]["healthy"], false);
    assert!(!feed::poll_rests(&b.app, "prs"));
}

// --- event-only relays (#220) ---

fn hours_off(b: &Board) {
    b.post("/hours", json!({"on": false}));
}

#[test]
fn a_webhook_relay_s_quiet_doesn_t_make_the_source_unhealthy() {
    let marker = tempfile::tempdir().unwrap();
    let ran = marker.path().join("restarted");
    let cmd = format!("touch {}", ran.display());
    let b = board_with(|c| {
        c.feed.on = true;
        c.feed.silent_secs = 20.0;
        c.feed.restart_cmd = cmd.clone();
        c.feed.restart_mins = vec![0.0];
    });
    hours_off(&b);
    b.feed_state(
        "prs",
        json!({"since": ago(600.0), "last_heartbeat_at": ago(5.0),
               "relays": {"fi": {"since": ago(600.0), "last_heartbeat_at": ago(5.0), "covers": [{"provider": "bitbucket"}]},
                          "github": {"since": ago(600.0), "last_event_at": ago(300.0)}}}),
    );
    let prs = b.source("prs");
    assert_eq!(prs["healthy"], true, "{}", prs["why"]);
    let gh = prs["relays"].as_array().unwrap().iter().find(|r| r["relay"] == "github").cloned().unwrap();
    assert_eq!((gh["event_only"].clone(), gh["healthy"].clone(), gh["problem"].clone()), (json!(true), json!(true), Value::Null));
    assert!(gh["last_event_at"].is_string());
    feed::check(&b.app).unwrap();
    feed::check(&b.app).unwrap();
    assert!(!ran.exists(), "nothing is restarted");
    assert!(!dispatch::alerts(&b.app).iter().any(|a| a["key"] == "pr-feed"));
    assert_eq!(feed::holding(&b.app, true), None, "the review loop isn't held");
    assert!(feed::feed_healthy(&b.app));
}

#[test]
fn a_silent_listener_relay_still_restarts_the_source() {
    let marker = tempfile::tempdir().unwrap();
    let ran = marker.path().join("restarted");
    let cmd = format!("touch {}", ran.display());
    let b = board_with(|c| {
        c.feed.on = true;
        c.feed.silent_secs = 20.0;
        c.feed.restart_cmd = cmd.clone();
        c.feed.restart_mins = vec![0.0];
    });
    hours_off(&b);
    b.feed_state(
        "prs",
        json!({"since": ago(600.0), "last_heartbeat_at": ago(300.0),
               "relays": {"fi": {"since": ago(600.0), "last_heartbeat_at": ago(300.0), "covers": [{"provider": "bitbucket"}]},
                          "github": {"since": ago(600.0), "last_event_at": ago(5.0)}}}),
    );
    let prs = b.source("prs");
    assert_eq!(prs["healthy"], false);
    assert!(prs["why"].as_str().unwrap().contains("relay fi"), "{}", prs["why"]);
    feed::check(&b.app).unwrap();
    feed::check(&b.app).unwrap();
    assert!(ran.exists(), "the listener's restart_cmd runs");
}

// --- tb stream (#221) ---

#[test]
fn get_stream_says_what_each_relay_covers() {
    let b = board_with(stream_mode("prs"));
    b.send(from("all", ev("h1", "heartbeat", json!({"sources": ["prs"]}))));
    b.send(from("hook", ev("r1", "review", json!({"pr": {"provider": "github", "repo": "acme/other", "num": 3}, "who": "rev", "state": "approved"}))));
    let relays = b.source("prs")["relays"].as_array().unwrap().clone();
    let by = |n: &str| relays.iter().find(|r| r["relay"] == n).cloned().unwrap();
    assert_eq!(by("all")["covers"], "all");
    assert_eq!(by("hook")["covers"], json!([]));
    assert_eq!(by("hook")["event_only"], true);
}

#[test]
fn a_relay_covering_some_prs_polls_some() {
    let b = board_with(stream_mode("prs"));
    b.send(from("bb", ev("h1", "heartbeat", json!({"sources": ["prs"], "covers": [{"provider": "bitbucket", "repo": "acme/webapp"}]}))));
    let prs = b.source("prs");
    assert_eq!((prs["polling"].clone(), prs["polling_some"].clone()), (json!(true), json!(true)));
    assert_eq!(prs["relays"][0]["covers"], json!([{"provider": "bitbucket", "repo": "acme/webapp"}]));

    let b = board_with(stream_mode("prs"));
    b.send(from("all", ev("h1", "heartbeat", json!({"sources": ["prs"]}))));
    let prs = b.source("prs");
    assert_eq!((prs["polling"].clone(), prs["polling_some"].clone()), (json!(false), json!(false)));
    assert_eq!(prs["polled"], json!([]));
}

#[test]
fn polling_some_names_the_repos_it_polls_and_why() {
    // #727
    let b = board_with(stream_mode("prs"));
    prhost::install(&b.app, FakeHost::new("github", host_rec("g1")));
    let bb = FakeHost::new("bitbucket", host_rec("b1"));
    prhost::install(&b.app, bb.clone());
    b.pr_task("https://github.com/acme/quill/pull/7");
    let covered = b.pr_task(BB);
    b.send(from("bb", ev("h1", "heartbeat", json!({"sources": ["prs"], "covers": [{"provider": "bitbucket", "repo": "acme/webapp"}]}))));
    assert_eq!(
        b.source("prs")["polled"],
        json!([{"host": "bitbucket", "repo": "acme/webapp", "prs": 1, "why": "reconcile"}, {"host": "github", "repo": "acme/quill", "prs": 1, "why": "not covered"}]),
        "the Bitbucket PR hasn't been read since the relay connected"
    );
    prflow::poll(&b.app).unwrap();
    assert_eq!(b.source("prs")["polled"], json!([{"host": "github", "repo": "acme/quill", "prs": 1, "why": "not covered"}]));

    // The host can't say what changed: the covered PR waits for a later round instead of a full read.
    prflow::merge_flow(&b.app, covered, vec![("read_at", json!(ago(7200.0)))]).unwrap();
    *bb.stamps_error.lock() = Some("rate-limited".into());
    let before = *bb.reads.lock();
    prflow::poll(&b.app).unwrap();
    assert_eq!(*bb.reads.lock(), before, "a covered PR isn't read in full when the stamps fail");
    prflow::refresh(&b.app).unwrap();
    assert_eq!(*bb.reads.lock(), before + 1, "a refresh still reads it");
    *bb.stamps_error.lock() = None;
    prflow::merge_flow(&b.app, covered, vec![("read_at", json!(ago(7200.0)))]).unwrap();
    prflow::poll(&b.app).unwrap();
    assert_eq!(*bb.reads.lock(), before + 2, "its reconcile reads it once the host answers");
}

#[test]
fn one_relay_covering_every_provider_works_as_before() {
    let b = board_with(stream_mode("prs"));
    b.send(from("relay", ev("h1", "heartbeat", json!({"sources": ["prs"]}))));
    let prs = b.source("prs");
    assert_eq!(prs["healthy"], true);
    assert_eq!(prs["polling"], false);
    assert!(feed::poll_rests(&b.app, "prs") && feed::poll_due(&b.app), "the poll runs, for the relay's hourly reconcile");
    assert!(feed::pr_poll_rests(&b.app, "github", "acme/webapp"));
    assert!(prs["last_heartbeat_at"].is_string(), "the source's own fields still show the last heartbeat");

    b.feed_state("prs", json!({"since": ago(4000.0), "last_heartbeat_at": ago(200.0), "relays": {"relay": {"since": ago(4000.0), "last_heartbeat_at": ago(200.0)}}}));
    assert_eq!(b.source("prs")["problem"], "stuck");
    assert!(feed::poll_due(&b.app), "stuck, so the poll takes over");
}

// --- a relay-covered PR's slow reconcile (#675) ---

fn bb_relay(id: &str, extra: Value) -> Value {
    let mut e = ev(id, "heartbeat", json!({"sources": ["prs", "builds"], "covers": [{"provider": "bitbucket"}]}));
    for (k, v) in extra.as_object().unwrap() {
        e[k] = v.clone();
    }
    from("fi", e)
}

#[test]
fn in_board_mode_a_relay_covered_pr_is_read_hourly_or_when_its_relay_comes_back() {
    let b = board();
    let h = FakeHost::new("bitbucket", Record { title: "one".into(), ..host_rec("h1") });
    prhost::install(&b.app, h.clone());
    let id = b.pr_task(BB);
    b.send(bb_relay("h1", json!({})));
    let title = |b: &Board| board::get_task(&b.app, id).unwrap().s("pr_title").map(str::to_string).unwrap_or_default();
    let read_again = |b: &Board, t: &str| {
        h.rec.lock().title = t.into();
        prflow::poll(&b.app).unwrap();
        title(b) == t
    };
    assert!(read_again(&b, "one"), "never read since the relay connected: read once");
    assert!(!read_again(&b, "two"), "fi covers it and it was just read");

    prflow::merge_flow(&b.app, id, taskboardd::fields!["read_at" => ago(4000.0)]).unwrap();
    assert!(read_again(&b, "two"), "an hour on, the reconcile reads it");
    assert!(!read_again(&b, "three"));

    prflow::merge_flow(&b.app, id, taskboardd::fields!["read_at" => ago(60.0)]).unwrap();
    b.send(bb_relay("h2", json!({"connected_at": ago(10.0)})));
    assert!(read_again(&b, "three"), "fi connected again since the last read");
    assert!(!read_again(&b, "four"));

    let mut st: Value = serde_json::from_str(&b.app.db.get_setting("pr_feed").unwrap().unwrap()).unwrap();
    st["relays"]["fi"]["last_heartbeat_at"] = json!(ago(200.0));
    b.feed_state("prs", st);
    assert!(read_again(&b, "four"), "fi is stuck, so its PRs are polled");
    assert!(read_again(&b, "five"));

    prflow::merge_flow(&b.app, id, taskboardd::fields!["read_at" => ago(30.0)]).unwrap();
    b.send(bb_relay("h3", json!({})));
    assert!(read_again(&b, "six"), "fi came back since the last read");
    assert!(!read_again(&b, "seven"));
}

#[test]
fn a_pr_with_no_covering_relay_or_poll_while_healthy_is_polled_every_round() {
    let b = board_with(|c| c.feed.poll_while_healthy = true);
    let h = FakeHost::new("bitbucket", Record { title: "one".into(), ..host_rec("h1") });
    prhost::install(&b.app, h.clone());
    let id = b.pr_task(BB);
    b.send(bb_relay("h1", json!({})));
    prflow::poll(&b.app).unwrap();
    h.rec.lock().title = "two".into();
    prflow::poll(&b.app).unwrap();
    assert_eq!(board::get_task(&b.app, id).unwrap().s("pr_title"), Some("two"), "poll_while_healthy");

    let b = board();
    let h = FakeHost::new("bitbucket", Record { title: "one".into(), ..host_rec("h1") });
    prhost::install(&b.app, h.clone());
    let id = b.pr_task(BB);
    b.send(from("fi", ev("h1", "heartbeat", json!({"sources": ["prs"], "covers": [{"provider": "github"}]}))));
    prflow::poll(&b.app).unwrap();
    h.rec.lock().title = "two".into();
    prflow::poll(&b.app).unwrap();
    assert_eq!(board::get_task(&b.app, id).unwrap().s("pr_title"), Some("two"), "fi covers GitHub only");
}

#[test]
fn board_mode_says_what_a_covering_relay_could_stop() {
    let b = board();
    assert_eq!(b.source("prs")["relay_hint"], Value::Null, "no relay");
    b.send(bb_relay("h1", json!({})));
    let hint = b.source("prs")["relay_hint"].as_str().unwrap().to_string();
    assert!(hint.starts_with("prs: relay fi covers bitbucket every repo, so the poll reads them again every 60 minutes"), "{hint}");
    assert!(hint.ends_with("`tb stream mode prs stream` would stop that"), "{hint}");
    let builds = b.source("builds")["relay_hint"].as_str().unwrap().to_string();
    assert_eq!(builds, "builds: polling although relay fi covers bitbucket every repo; `tb stream mode builds stream` would stop that");
    assert_eq!(b.get("/state")["pr_feed"]["relay_hint"], json!(hint), "/state says it too");
    assert_eq!(b.get("/prs/feed")["relay_hint"], json!(hint));
    assert_eq!(b.post("/prs/refresh", json!({}))["relay_hints"], json!([hint, builds]));

    b.post("/stream/mode", json!({"source": "prs", "mode": "stream"}));
    assert_eq!(b.source("prs")["relay_hint"], Value::Null, "stream mode: nothing to switch");
    assert_eq!(b.post("/prs/refresh", json!({}))["relay_hints"], json!([builds]));

    let mut st: Value = serde_json::from_str(&b.app.db.get_setting("stream_feed_builds").unwrap().unwrap()).unwrap();
    st["relays"]["fi"]["last_heartbeat_at"] = json!(ago(4000.0));
    b.feed_state("builds", st);
    assert_eq!(b.source("builds")["relay_hint"], Value::Null, "a stuck relay covers nothing");
}

#[test]
fn in_stream_mode_a_pr_event_with_its_state_is_applied_and_only_new_checks_are_read() {
    let b = board_with(stream_mode("prs"));
    let id = b.pr_task(BB);
    let h = FakeHost::new("bitbucket", Record { title: "from the host".into(), ..host_rec("h0") });
    prhost::install(&b.app, h.clone());
    prflow::refresh_task(&b.app, id).unwrap().unwrap();
    let mut e = full_pr("pr1");
    e["reviewers"] = json!([{"who": "sam", "state": "approved"}]);
    e["threads"] = json!([{"id": "t1", "path": "src/a.rs", "line": 3, "resolved": false,
        "comments": [{"id": "c1", "author": "kim", "body": "Rename this", "at": "2026-10-12T15:01:00Z"}]}]);
    b.send(batch(vec![e]));
    let calls = h.calls().len();

    let partial = |id: &str, head: &str, title: &str| {
        ev(id, "pr", json!({"provider": "bitbucket", "repo": "acme/webapp", "num": 9, "url": BB, "state": "open",
            "title": title, "branch": "login", "base": "main", "head": head}))
    };
    let r = b.send(batch(vec![partial("pr2", "h1", "Login, renamed")]));
    assert_eq!(r["results"][0]["complete"], false, "{r}");
    assert_eq!(r["results"][0]["result"]["applied"], true, "{r}");
    let rec = b.rec(id);
    assert_eq!(rec["title"], "Login, renamed", "{rec}");
    assert_eq!((rec["approvals"].clone(), rec["comments"].clone()), (json!(1), json!(1)), "what it doesn't carry stays: {rec}");
    assert_eq!(rec["checks"][0]["name"], "build");
    assert_eq!(h.calls().len(), calls, "same head: nothing read from the host");

    h.rec.lock().checks = vec![Check { name: "lint".into(), state: "failed".into(), url: None, at: None }];
    let r = b.send(batch(vec![partial("pr3", "h2", "Login, again")]));
    assert_eq!(r["results"][0]["result"]["applied"], true, "{r}");
    let rec = b.rec(id);
    assert_eq!((rec["head"].clone(), rec["title"].clone()), (json!("h2"), json!("Login, again")), "{rec}");
    assert_eq!(rec["failed"], json!(["lint"]), "a new head's checks are read: {rec}");
    assert_eq!(rec["approvals"], 1);
    assert_eq!(h.calls()[calls..], ["checks h2".to_string()], "only its checks");
}

// --- jira.issue (#78) ---

fn jira_ev(id: &str, key: &str, status: &str, category: &str, by: &str) -> Value {
    ev(id, "jira.issue", json!({"key": key, "status": status, "status_category": category, "assignee": "me", "changed_by": by}))
}

impl Board {
    fn ticket_task(&self, key: &str, jira_status: &str) -> i64 {
        let id = self.post("/tasks", json!({"title": "Ticketed", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
        board::update_task(&self.app, id, taskboardd::fields!["jira_key" => key, "jira_status" => jira_status]).unwrap();
        id
    }
    fn log(&self, id: i64) -> String {
        self.get(&format!("/tasks/T{id}/log")).to_string()
    }
    fn alerts(&self) -> Vec<Value> {
        taskboardd::dispatch::alerts(&self.app)
    }
}

#[test]
fn a_jira_issue_event_updates_the_task_epic_or_issue_with_its_key() {
    let b = board();
    let t = b.ticket_task("PROJ-7", "In Progress");
    let g = b.post("/goals", json!({"name": "Login", "project": "webapp"}))["id"].as_i64().unwrap();
    b.app.db.x("UPDATE goals SET epic_key = 'PROJ-1', epic_status = 'To Do' WHERE id = ?", taskboardd::p![g]).unwrap();
    let i = b.app.db.insert("issues", taskboardd::fields!["kind" => "bug", "title" => "Grey button", "project" => "webapp", "jira_key" => "PROJ-9"]).unwrap();

    let r = b.send(batch(vec![
        jira_ev("j1", "proj-7", "In Review", "indeterminate", "Sam Lee"),
        jira_ev("j2", "PROJ-1", "In Progress", "indeterminate", "Pat"),
        jira_ev("j3", "PROJ-9", "Selected", "new", ""),
        jira_ev("j4", "PROJ-404", "Done", "done", "Sam Lee"),
    ]));
    assert_eq!(r["accepted"], 4, "{r}");
    assert_eq!(board::get_task(&b.app, t).unwrap().st("jira_status"), "In Review");
    assert!(b.log(t).contains("Sam Lee moved PROJ-7 to In Review"), "{}", b.log(t));
    let goal = b.app.db.q1("SELECT epic_status FROM goals WHERE id = ?", taskboardd::p![g]).unwrap().unwrap();
    assert_eq!(goal.st("epic_status"), "In Progress");
    let issue = b.app.db.q1("SELECT jira_status FROM issues WHERE id = ?", taskboardd::p![i]).unwrap().unwrap();
    assert_eq!(issue.st("jira_status"), "Selected");
    let e = b.app.db.q1("SELECT who, text FROM issue_events WHERE issue_id = ? AND kind = 'jira'", taskboardd::p![i]).unwrap().unwrap();
    assert_eq!((e.st("who").as_str(), e.st("text").as_str()), ("Jira", "PROJ-9 moved to Selected"));
    assert!(b.kept("j4").unwrap().st("result").contains("\"tasks\":[]"), "an unknown key is kept and matches nothing");
    assert!(b.alerts().is_empty(), "no task changed its done-ness");

    b.send(batch(vec![jira_ev("j5", "PROJ-7", "In Review", "indeterminate", "Sam Lee")]));
    assert_eq!(b.log(t).matches("moved PROJ-7").count(), 1, "the same status again isn't logged twice");
}

#[test]
fn a_status_the_board_just_moved_the_ticket_to_is_skipped() {
    let b = board();
    let t = b.ticket_task("PROJ-7", "In Progress");
    taskboardd::jira::request(&b.app, "transition", t, "PROJ-7", Some("In Review"), None).unwrap();
    let r = b.send(batch(vec![jira_ev("j1", "PROJ-7", "In Review", "indeterminate", "Morgan")]));
    assert_eq!(r["accepted"], 1, "{r}");
    assert!(b.kept("j1").unwrap().st("result").contains("the board moved it"));
    assert!(!b.log(t).contains("moved PROJ-7"), "{}", b.log(t));

    b.send(batch(vec![jira_ev("j2", "PROJ-7", "Blocked", "indeterminate", "Sam")]));
    assert_eq!(board::get_task(&b.app, t).unwrap().st("jira_status"), "Blocked", "another status is someone else's move");

    b.app.db.x("UPDATE jobs SET updated_at = ?", taskboardd::p![ago(3600.0)]).unwrap();
    b.send(batch(vec![jira_ev("j3", "PROJ-7", "In Review", "indeterminate", "Sam")]));
    assert!(b.log(t).contains("Sam moved PROJ-7 to In Review"), "an old board move doesn't hide a new one");
}

#[test]
fn a_ticket_done_under_an_open_task_or_reopened_under_a_done_one_raises_an_alert() {
    let b = board();
    let open = b.ticket_task("PROJ-7", "In Progress");
    b.send(batch(vec![jira_ev("j1", "PROJ-7", "Done", "done", "Sam")]));
    let task = board::get_task(&b.app, open).unwrap();
    assert_ne!(task.st("status"), "done", "the task isn't changed");
    assert_eq!(task.st("jira_status"), "Done");
    let alerts = b.alerts();
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert!(alerts[0]["text"].as_str().unwrap().contains(&format!("Sam moved PROJ-7 to Done, but T{open} is still open")), "{alerts:?}");

    let done = b.ticket_task("PROJ-8", "Done");
    b.app.db.x("UPDATE tasks SET status = 'done' WHERE id = ?", taskboardd::p![done]).unwrap();
    b.send(batch(vec![jira_ev("j2", "PROJ-8", "In Progress", "indeterminate", "Pat")]));
    assert_eq!(board::get_task(&b.app, done).unwrap().st("status"), "done");
    let alerts = b.alerts();
    assert_eq!(alerts.len(), 2, "{alerts:?}");
    assert!(alerts.iter().any(|a| a["text"].as_str().unwrap().contains(&format!("reopening it, but T{done} is done"))), "{alerts:?}");

    b.send(batch(vec![jira_ev("j3", "PROJ-8", "In Review", "indeterminate", "Pat")]));
    assert_eq!(b.alerts().len(), 2, "moving between open statuses isn't a reopen");
}

fn qa_board() -> Board {
    board_with(|c| {
        c.jira.merged = "Ready for QA".into();
        c.jira.done = "Done".into();
        c.jira.canceled = "Withdrawn".into();
    })
}

fn moved(id: &str, key: &str, from: &str, status: &str, category: &str) -> Value {
    let mut e = jira_ev(id, key, status, category, "Quinn");
    e["from_status"] = json!(from);
    e
}

impl Board {
    fn done_ticket_task(&self, key: &str, jira_status: &str) -> i64 {
        let id = self.ticket_task(key, jira_status);
        self.app.db.x("UPDATE tasks SET status = 'done' WHERE id = ?", taskboardd::p![id]).unwrap();
        id
    }
    fn reopen_alerts(&self) -> usize {
        self.alerts().iter().filter(|a| a["text"].as_str().unwrap().contains("reopening it")).count()
    }
    fn still_open_alerts(&self) -> usize {
        self.alerts().iter().filter(|a| a["text"].as_str().unwrap().contains("is still open")).count()
    }
}

#[test]
fn qa_moving_a_ticket_on_from_the_merged_status_isn_t_a_reopen() {
    let b = qa_board();
    b.done_ticket_task("ABC-12", "Ready for QA");
    b.send(batch(vec![moved("j1", "ABC-12", "Ready for QA", "In QA", "")]));
    assert_eq!(b.reopen_alerts(), 0, "{:?}", b.alerts());

    b.done_ticket_task("ABC-13", "Done");
    b.send(batch(vec![moved("j2", "ABC-13", "Done", "In Progress", "")]));
    assert_eq!(b.reopen_alerts(), 1, "a real done status moved back reopens it: {:?}", b.alerts());

    b.done_ticket_task("ABC-14", "Ready for QA");
    b.send(batch(vec![moved("j3", "ABC-14", "Ready for QA", "To Do", "")]));
    assert_eq!(b.reopen_alerts(), 2, "back to a new status reopens it: {:?}", b.alerts());
    b.done_ticket_task("ABC-15", "Ready for QA");
    b.send(batch(vec![moved("j4", "ABC-15", "Ready for QA", "Selected", "new")]));
    assert_eq!(b.reopen_alerts(), 3, "category new reopens it: {:?}", b.alerts());

    b.ticket_task("ABC-16", "In Code Review");
    b.send(batch(vec![moved("j5", "ABC-16", "In Code Review", "Ready for QA", "")]));
    assert_eq!(b.still_open_alerts(), 1, "{:?}", b.alerts());
}

#[test]
fn a_reopened_ticket_raises_one_alert_for_all_its_done_tasks() {
    let b = qa_board();
    let tasks: Vec<i64> = (0..3).map(|_| b.done_ticket_task("ABC-21", "Done")).collect();
    for (n, t) in tasks.iter().enumerate() {
        b.app.db.x("UPDATE tasks SET finished_at = ? WHERE id = ?", taskboardd::p![ago(3600.0 - n as f64 * 60.0), *t]).unwrap();
    }
    b.send(batch(vec![moved("j1", "ABC-21", "Done", "To Do", "")]));
    let alerts: Vec<Value> = b.alerts().into_iter().filter(|a| a["text"].as_str().unwrap().contains("reopening it")).collect();
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    let want = format!("Quinn moved ABC-21 to To Do, reopening it, but T{}, T{} and T{} are done.", tasks[0], tasks[1], tasks[2]);
    assert_eq!(alerts[0]["text"], want.as_str());
    assert_eq!(alerts[0]["task_id"], tasks[2], "it opens the newest done task");

    b.send(batch(vec![moved("j2", "ABC-21", "To Do", "Done", "")]));
    taskboardd::dispatch::prune_alerts(&b.app).unwrap();
    assert_eq!(b.reopen_alerts(), 0, "it clears once the ticket is back in a done status: {:?}", b.alerts());

    let open: Vec<i64> = (0..2).map(|_| b.ticket_task("ABC-22", "In Progress")).collect();
    b.send(batch(vec![moved("j3", "ABC-22", "In Progress", "Closed", "")]));
    let still: Vec<Value> = b.alerts().into_iter().filter(|a| a["text"].as_str().unwrap().contains("still open")).collect();
    assert_eq!(still.len(), 1, "{still:?}");
    assert_eq!(still[0]["text"], format!("Quinn moved ABC-22 to Closed, but T{} and T{} are still open.", open[0], open[1]).as_str());
}

#[test]
fn a_reopened_ticket_alert_on_a_done_task_survives_the_prune() {
    let b = qa_board();
    b.done_ticket_task("ABC-12", "Done");
    b.send(batch(vec![moved("j1", "ABC-12", "Done", "In QA", "indeterminate")]));
    assert_eq!(b.reopen_alerts(), 1, "{:?}", b.alerts());
    taskboardd::dispatch::prune_alerts(&b.app).unwrap();
    assert_eq!(b.reopen_alerts(), 1, "a done task doesn't clear its own reopen alert");
}

#[test]
fn a_still_open_alert_survives_a_second_jira_move() {
    let b = qa_board();
    b.ticket_task("ABC-11", "In Code Review");
    b.send(batch(vec![jira_ev("j1", "ABC-11", "Ready for QA", "", "Pat")]));
    b.send(batch(vec![jira_ev("j2", "ABC-11", "Done", "done", "Pat")]));
    taskboardd::dispatch::prune_alerts(&b.app).unwrap();
    assert_eq!(b.still_open_alerts(), 1, "{:?}", b.alerts());
    assert_eq!(b.alerts().len(), 1, "{:?}", b.alerts());
}

#[test]
fn a_jira_alert_clears_once_task_and_ticket_agree() {
    let b = qa_board();
    let finishes = b.ticket_task("ABC-11", "In Code Review");
    b.send(batch(vec![jira_ev("j1", "ABC-11", "Done", "done", "Pat")]));
    let moves_back = b.ticket_task("ABC-12", "In Code Review");
    b.send(batch(vec![jira_ev("j2", "ABC-12", "Done", "done", "Pat")]));
    let reopened = b.done_ticket_task("ABC-13", "Done");
    b.send(batch(vec![jira_ev("j3", "ABC-13", "In Progress", "indeterminate", "Quinn")]));
    let redone = b.done_ticket_task("ABC-14", "Done");
    b.send(batch(vec![jira_ev("j4", "ABC-14", "In Progress", "indeterminate", "Quinn")]));
    assert_eq!(b.alerts().len(), 4, "{:?}", b.alerts());

    b.app.db.x("UPDATE tasks SET status = 'done' WHERE id = ?", taskboardd::p![finishes]).unwrap();
    b.send(batch(vec![jira_ev("j5", "ABC-12", "In Progress", "indeterminate", "Pat")]));
    b.app.db.x("UPDATE tasks SET status = 'queued' WHERE id = ?", taskboardd::p![reopened]).unwrap();
    b.send(batch(vec![jira_ev("j6", "ABC-14", "Done", "done", "Quinn")]));
    taskboardd::dispatch::prune_alerts(&b.app).unwrap();
    let left: Vec<i64> = b.alerts().iter().filter_map(|a| a["task_id"].as_i64()).collect();
    for id in [finishes, moves_back, reopened, redone] {
        assert!(!left.contains(&id), "T{id}'s alert clears once task and ticket agree: {:?}", b.alerts());
    }
}

fn jira_at(id: &str, key: &str, status: &str, at: &str) -> Value {
    let mut e = jira_ev(id, key, status, "indeterminate", "Sam");
    e["at"] = json!(at);
    e
}

#[test]
fn an_older_jira_issue_event_doesn_t_overwrite_a_newer_status() {
    let b = board();
    let t = b.ticket_task("PROJ-1", "To Do");
    let g = b.post("/goals", json!({"name": "Login", "project": "webapp"}))["id"].as_i64().unwrap();
    b.app.db.x("UPDATE goals SET epic_key = 'PROJ-1', epic_status = 'To Do' WHERE id = ?", taskboardd::p![g]).unwrap();
    let i = b.app.db.insert("issues", taskboardd::fields!["kind" => "bug", "title" => "Grey button", "project" => "webapp", "jira_key" => "PROJ-1"]).unwrap();

    b.send(batch(vec![jira_at("e-new", "PROJ-1", "Ready for QA", "2026-10-12T23:00:00Z")]));
    b.send(batch(vec![jira_at("e-old", "PROJ-1", "In Progress", "2026-10-12T20:00:00Z")]));
    let task = board::get_task(&b.app, t).unwrap();
    assert_eq!(task.st("jira_status"), "Ready for QA");
    assert_eq!(task.st("jira_status_at"), "2026-10-12T23:00:00Z");
    assert_eq!(b.log(t).matches("moved PROJ-1").count(), 1, "{}", b.log(t));
    assert!(b.kept("e-old").unwrap().st("result").contains("older than the stored status"));
    let goal = b.app.db.q1("SELECT epic_status FROM goals WHERE id = ?", taskboardd::p![g]).unwrap().unwrap();
    assert_eq!(goal.st("epic_status"), "Ready for QA");
    let issue = b.app.db.q1("SELECT jira_status FROM issues WHERE id = ?", taskboardd::p![i]).unwrap().unwrap();
    assert_eq!(issue.st("jira_status"), "Ready for QA");

    let other = b.ticket_task("PROJ-2", "To Do");
    b.send(batch(vec![jira_at("o-old", "PROJ-2", "In Progress", "2026-10-12T20:00:00Z")]));
    b.send(batch(vec![jira_at("o-new", "PROJ-2", "Ready for QA", "2026-10-12T23:00:00Z")]));
    assert_eq!(board::get_task(&b.app, other).unwrap().st("jira_status"), "Ready for QA");
    assert_eq!(b.log(other).matches("moved PROJ-2").count(), 2, "{}", b.log(other));
}

#[test]
fn the_qa_poll_s_read_and_the_board_s_own_move_still_apply_over_a_stream_status() {
    let b = board();
    let t = b.ticket_task("PROJ-1", "To Do");
    b.send(batch(vec![jira_at("e1", "PROJ-1", "In Progress", &ago(60.0))]));
    taskboardd::jira_status::apply(&b.app, &json!({"key": "PROJ-1", "status": "Blocked"})).unwrap();
    assert_eq!(board::get_task(&b.app, t).unwrap().st("jira_status"), "Blocked", "the poll's read is as of now");

    let job = taskboardd::jira::request(&b.app, "transition", t, "PROJ-1", Some("In Review"), None).unwrap();
    taskboardd::jira::finish(&b.app, job, true, Some("PROJ-1"), Some("In Review"), None).unwrap();
    assert_eq!(board::get_task(&b.app, t).unwrap().st("jira_status"), "In Review");
    b.send(batch(vec![jira_at("e2", "PROJ-1", "In Progress", &ago(30.0))]));
    assert_eq!(board::get_task(&b.app, t).unwrap().st("jira_status"), "In Review", "an event from before the board's move doesn't undo it");
}

#[test]
fn the_owner_s_pr_opened_outside_the_board_is_linked_to_its_task_by_branch() {
    let b = board();
    prhost::install(&b.app, FakeHost::new("bitbucket", host_rec("h1")));
    let task_on = |branch: &str, ships: bool| {
        let id = b.post("/tasks", json!({"title": format!("On {branch}"), "detail": "Do it.", "project": "webapp", "ships_pr": ships}))["id"].as_i64().unwrap();
        let ctx = json!({"where": {"branch": branch}}).to_string();
        b.app.db.x("UPDATE tasks SET context = ?, created_at = ? WHERE id = ?", taskboardd::p![ctx, ago(3600.0), id]).unwrap();
        id
    };
    let pr_ev = |id: &str, num: i64, branch: &str, extra: Value| {
        let mut e = ev(id, "pr", json!({"provider": "bitbucket", "repo": "acme/webapp", "num": num,
            "url": format!("https://bitbucket.org/acme/webapp/pull-requests/{num}"), "branch": branch, "author": "pat", "state": "open"}));
        for (k, v) in extra.as_object().unwrap() {
            e[k] = v.clone();
        }
        e
    };
    let pr_of = |id: i64| board::get_task(&b.app, id).unwrap().i("pr_num");

    let login = task_on("feature/login", true);
    let r = b.send(batch(vec![pr_ev("p1", 12, "feature/login", json!({"mine": true, "created_at": ago(60.0)}))]));
    assert_eq!(r["results"][0]["result"]["task"], format!("T{login}"), "{r}");
    assert_eq!(pr_of(login), Some(12));
    assert!(b.get(&format!("tasks/T{login}"))["log"].to_string().contains("Linked PR #12 (acme/webapp)"));

    let theirs = task_on("feature/theirs", true);
    b.send(batch(vec![pr_ev("p2", 13, "feature/theirs", json!({}))]));
    assert_eq!(pr_of(theirs), None, "only the owner's PRs count");

    let (dup1, dup2) = (task_on("feature/dup", true), task_on("feature/dup", true));
    b.send(batch(vec![pr_ev("p3", 14, "feature/dup", json!({"mine": true}))]));
    assert_eq!((pr_of(dup1), pr_of(dup2)), (None, None), "a branch two tasks work on links neither");

    let older = task_on("feature/older", true);
    b.send(batch(vec![pr_ev("p4", 15, "feature/older", json!({"mine": true, "created_at": ago(7200.0)}))]));
    assert_eq!(pr_of(older), None, "a PR opened before the task was made isn't its");

    let no_pr = task_on("feature/research", false);
    b.send(batch(vec![pr_ev("p5", 16, "feature/research", json!({"mine": true}))]));
    assert_eq!(pr_of(no_pr), None, "a task that ships no PR gets none");

    let shipped = task_on("feature/shipped", true);
    b.send(batch(vec![pr_ev("p6", 17, "feature/shipped", json!({"mine": true, "state": "merged"}))]));
    assert_eq!(pr_of(shipped), Some(17), "a merged one counts too");
}

#[test]
fn a_review_hint_reads_its_pr_once_more_after_the_settle_window() {
    let b = board_with(|c| c.pr.comment_settle_secs = 90.0);
    let id = b.pr_task(BB);
    let h = FakeHost::new("bitbucket", host_rec("h1"));
    prhost::install(&b.app, h.clone());
    let reread_at = || -> Option<String> {
        let f: Value = serde_json::from_str(&board::get_task(&b.app, id).unwrap().st("pr_flow")).unwrap();
        f["reread_at"].as_str().map(|s| s.to_string())
    };
    let pr = json!({"provider": "bitbucket", "repo": "acme/webapp", "num": 9});
    b.send(batch(vec![ev("r1", "review", json!({"pr": pr, "who": "pat", "state": "commented"}))]));
    assert_eq!(b.rec(id)["head"], "h1", "read at once");
    let first = reread_at().expect("a second read is due");
    b.send(batch(vec![ev("c1", "comment", json!({"pr": pr, "comment_id": "c1", "author": "pat", "body": "Why?"}))]));
    assert!(reread_at().unwrap() >= first, "hints in the window fold into one later read");

    h.rec.lock().head = "h2".into();
    prflow::late_reads(&b.app).unwrap();
    assert_eq!(b.rec(id)["head"], "h1", "not before the window is over");
    b.app.db.x("UPDATE tasks SET pr_flow = json_set(pr_flow, '$.reread_at', ?) WHERE id = ?", taskboardd::p![ago(1.0), id]).unwrap();
    prflow::late_reads(&b.app).unwrap();
    assert_eq!(b.rec(id)["head"], "h2", "the settled read sees what came after the comments");
    assert_eq!(reread_at(), None, "once");
}
