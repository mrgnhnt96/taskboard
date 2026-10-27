//! PRs on a pluggable host (#2, #10, #11, #12, #14, #15, #16): a Bitbucket PR is watched like a GitHub
//! one, open threads drive the stage, `tb pr reply` / `ack` / `addressed` / `merge` / `not-ours` act
//! through the host, and a project's rules (approvals, expected checks, failures_cmd) hold.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::{Config, PrProject, RebasePolicy};
use taskboardd::prhost::{self, Check, FakeHost, Record, Reviewer, Thread};
use taskboardd::util::{iso, now_ts, RowExt};
use taskboardd::{board, midna, p, prcmds, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.watch = true;
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
    fn post(&self, path: &str, body: Value) -> Value {
        self.try_post(path, body).unwrap_or_else(|e| panic!("POST {path}: {e}"))
    }
    fn try_post(&self, path: &str, body: Value) -> Result<Value, String> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| e.message)
    }
    fn get(&self, path: &str, query: &[(&str, &str)]) -> Value {
        let q: Query = query.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        api::dispatch(&self.app, "GET", path, &q, &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn report(&self, event: &str, session: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message))
    }
    fn pr_task(&self, url: &str) -> i64 {
        let id = self.post("/tasks", json!({"title": "Ship it", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
        let repo = self.dir.path().join("webapp").to_string_lossy().to_string();
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo, "status": {"state": "working"}})], &[]).unwrap();
        self.report("tb.take", "s1", json!({"task": format!("T{id}")}));
        self.report("tb.done", "s1", json!({"summary": "Done", "pr": url}));
        id
    }
    fn task(&self, id: i64) -> serde_json::Map<String, Value> {
        board::get_task(&self.app, id).unwrap()
    }
    fn phase(&self, id: i64) -> String {
        self.task(id).st("pr_phase")
    }
    fn flow(&self, id: i64) -> Value {
        serde_json::from_str(&self.task(id).st("pr_flow")).unwrap()
    }
}

const BB: &str = "https://bitbucket.org/acme/webapp/pull-requests/9";

fn check(name: &str, state: &str) -> Check {
    Check { name: name.into(), state: state.into(), url: None, at: None }
}

fn reviewer(user: &str, state: &str) -> Reviewer {
    Reviewer { user: user.into(), name: user.to_uppercase(), state: state.into(), requested: true, changes_at: None, ..Default::default() }
}

fn thread(id: &str, last: &str) -> Thread {
    Thread {
        id: id.into(),
        kind: "review".into(),
        resolvable: true,
        author: "rev".into(),
        author_name: "Rev".into(),
        last_author: last.into(),
        last_id: format!("{id}-c"),
        text: format!("Thread {id}"),
        ..Default::default()
    }
}

fn green() -> Record {
    Record {
        state: "OPEN".into(),
        title: "Add x".into(),
        author: "me".into(),
        head: "h1".into(),
        branch: "feat".into(),
        base: "main".into(),
        base_head: "b1".into(),
        checks: vec![check("build", "passed")],
        ..Default::default()
    }
}

fn fake(b: &Board, rec: Record) -> Arc<FakeHost> {
    let h = FakeHost::new("bitbucket", rec);
    prhost::install(&b.app, h.clone());
    h
}

fn poll(b: &Board) {
    prflow::refresh(&b.app).unwrap();
}

#[test]
fn a_bitbucket_pr_is_watched_and_its_open_threads_drive_the_stage() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    assert_eq!(b.phase(id), "checks", "a Bitbucket PR gets a stage too");
    let mut rec = green();
    rec.threads = vec![thread("1", "rev"), thread("2", "me")];
    let h = fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "comments", "thread 1 waits on the author; thread 2 had their last word");
    let card = board::pr_card(&b.app, &b.task(id));
    assert_eq!(card["stage"]["open_threads"], 1);

    let v = b.post(&format!("/tasks/T{id}/pr/reply"), json!({"thread": "1", "text": "Renamed it", "resolve": true, "who": "The agent"}));
    assert_eq!(v["resolved"], true);
    assert_eq!(h.calls(), vec!["reply 1 Renamed it", "resolve 1"]);
    assert_eq!(b.phase(id), "review", "nothing is open; nobody approved yet");
    assert!(b.try_post(&format!("/tasks/T{id}/pr/reply"), json!({"thread": "9", "text": "x"})).unwrap_err().contains("no thread 9"));
}

#[test]
fn pr_thread_prints_a_threads_whole_text_and_replies() {
    // #609: status clips each thread; the agent can't read the host itself.
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let long = format!("src/a.rs\n{}", "Rename this. ".repeat(30));
    let mut rec = green();
    let mut th = thread("1", "rev");
    th.body = long.clone();
    th.replies = vec![taskboardd::prhost::Reply { author: "me".into(), author_name: "Me".into(), text: "Why?".into(), body: "Why?\nIt reads fine.".into(), ..Default::default() }];
    rec.threads = vec![th, thread("2", "me")];
    fake(&b, rec);
    poll(&b);
    let v = b.post(&format!("/tasks/T{id}/pr/thread"), json!({"thread": "1"}));
    assert_eq!(v["thread"]["body"], long.as_str());
    assert_eq!(v["thread"]["replies"][0]["body"], "Why?\nIt reads fine.");
    assert_eq!(v["open"], true);
    assert_eq!(b.post(&format!("/tasks/T{id}/pr/thread"), json!({"thread": "2"}))["open"], false, "the author had the last word");
    assert!(b.try_post(&format!("/tasks/T{id}/pr/thread"), json!({"thread": "9"})).unwrap_err().contains("no thread 9"));
}

fn reads(h: &FakeHost) -> usize {
    *h.reads.lock()
}

/// Ages the board's copy of the PR past `pr thread`'s TTL.
fn age_copy(b: &Board, id: i64) {
    prflow::merge_flow(&b.app, id, vec![("checked_at", json!(iso(now_ts() - prcmds::HELD_SECS - 5.0)))]).unwrap();
}

#[test]
fn pr_thread_serves_the_boards_copy_until_it_ages_out_or_is_refreshed() {
    // #679: each pr/thread read the host live, 2-9 s a time, and added to the rate limit.
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.threads = vec![thread("1", "rev")];
    let h = fake(&b, rec);
    poll(&b);
    let polled = reads(&h);
    let v = b.post(&format!("/tasks/T{id}/pr/thread"), json!({"thread": "1"}));
    assert_eq!(reads(&h), polled, "the board's copy from the poll answers");
    assert_eq!(v["as_of"], b.flow(id)["checked_at"]);
    assert!(v["stale"].is_null());
    assert_eq!(v["open"], true);

    h.rec.lock().threads[0].replies = vec![taskboardd::prhost::Reply { author: "rev".into(), text: "Ping".into(), ..Default::default() }];
    let v = b.post(&format!("/tasks/T{id}/pr/thread"), json!({"thread": "1"}));
    assert!(v["thread"]["replies"].as_array().map_or(true, |r| r.is_empty()), "still the copy");
    let v = b.post(&format!("/tasks/T{id}/pr/thread"), json!({"thread": "1", "refresh": true}));
    assert_eq!(reads(&h), polled + 1, "refresh reads the host");
    assert_eq!(v["thread"]["replies"][0]["text"], "Ping");

    age_copy(&b, id);
    b.post(&format!("/tasks/T{id}/pr/thread"), json!({"thread": "1"}));
    assert_eq!(reads(&h), polled + 2, "an old copy is read again");
    b.post(&format!("/tasks/T{id}/pr/thread"), json!({"thread": "1"}));
    assert_eq!(reads(&h), polled + 2, "and that read is the copy now");

    h.rec.lock().threads.push(thread("2", "rev"));
    assert_eq!(b.post(&format!("/tasks/T{id}/pr/thread"), json!({"thread": "2"}))["thread"]["id"], "2", "a thread the copy doesn't have yet is read");
    assert_eq!(reads(&h), polled + 3);
}

#[test]
fn pr_thread_reads_asked_at_once_share_one_host_call() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.threads = vec![thread("1", "rev")];
    let h = fake(&b, rec);
    poll(&b);
    let polled = reads(&h);
    *h.read_delay.lock() = std::time::Duration::from_millis(400);
    let path = format!("/tasks/T{id}/pr/thread");
    std::thread::scope(|sc| {
        let first = sc.spawn(|| b.post(&path, json!({"thread": "1", "refresh": true})));
        std::thread::sleep(std::time::Duration::from_millis(100));
        let second = sc.spawn(|| b.post(&path, json!({"thread": "1", "refresh": true})));
        assert_eq!(first.join().unwrap()["thread"]["id"], "1");
        assert_eq!(second.join().unwrap()["thread"]["id"], "1");
    });
    assert_eq!(reads(&h), polled + 1, "the second waited on the first's read");
}

#[test]
fn pr_thread_answers_from_the_copy_while_the_host_is_rate_limited() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.threads = vec![thread("1", "rev")];
    let h = fake(&b, rec);
    poll(&b);
    age_copy(&b, id);
    *h.read_error.lock() = Some("Bitbucket rate-limited until 3:40 PM".into());
    let v = b.post(&format!("/tasks/T{id}/pr/thread"), json!({"thread": "1"}));
    assert_eq!(v["thread"]["text"], "Thread 1");
    assert_eq!(v["stale"], "Bitbucket rate-limited until 3:40 PM", "it says the copy may be stale");
    assert_eq!(v["as_of"], b.flow(id)["checked_at"], "as of the copy's own read");
    let e = b.try_post(&format!("/tasks/T{id}/pr/thread"), json!({"thread": "2"})).unwrap_err();
    assert!(e.contains("Couldn't read PR #9: Bitbucket rate-limited"), "{e}");
}

#[test]
fn reply_and_ack_write_once_and_note_it_on_the_copy_without_reading_the_pr() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.threads = vec![thread("1", "rev"), thread("2", "rev")];
    let h = fake(&b, rec);
    poll(&b);
    let polled = reads(&h);
    let as_of = b.flow(id)["checked_at"].clone();

    let v = b.post(&format!("/tasks/T{id}/pr/reply"), json!({"thread": "1", "text": "Renamed it"}));
    assert_eq!(v["open_threads"], 1);
    let v = b.post(&format!("/tasks/T{id}/pr/ack"), json!({"thread": "2"}));
    assert_eq!(v["resolved"], true);
    assert_eq!(v["open_threads"], 0);
    assert_eq!(h.calls(), vec!["reply 1 Renamed it", "resolve 2"]);
    assert_eq!(reads(&h), polled, "neither read the PR");

    let th = b.post(&format!("/tasks/T{id}/pr/thread"), json!({"thread": "1"}));
    assert_eq!(th["thread"]["replies"][0]["text"], "Renamed it", "the copy has the reply");
    assert_eq!(th["thread"]["last_author"], "me");
    assert_eq!(th["open"], false);
    assert_eq!(b.post(&format!("/tasks/T{id}/pr/thread"), json!({"thread": "2"}))["thread"]["resolved"], true);
    assert_eq!(reads(&h), polled);
    assert_eq!(b.flow(id)["checked_at"], as_of, "the copy is still as of its last read");
}

#[test]
fn ack_resolves_without_a_reply_and_holds_until_someone_speaks_again() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    let mut plain = thread("c1", "rev");
    plain.kind = "comment".into();
    plain.resolvable = false;
    rec.threads = vec![plain.clone()];
    rec.reviewers = vec![reviewer("ok", "approved")];
    rec.approvals = 1;
    let h = fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "comments");
    b.post(&format!("/tasks/T{id}/pr/ack"), json!({"thread": "c1"}));
    assert_eq!(h.calls(), Vec::<String>::new(), "a comment the host can't resolve is only noted on the board");
    assert_eq!(b.flow(id)["threads_acked"]["c1"], "c1-c");
    assert_eq!(b.phase(id), "merge");
    // The reviewer writes again on it.
    h.rec.lock().threads[0].last_id = "c1-d".into();
    poll(&b);
    assert_eq!(b.phase(id), "comments", "a new word reopens it");
}

#[test]
fn addressed_refuses_while_threads_are_open_then_asks_the_reviewers_again() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.review_decision = "CHANGES_REQUESTED".into();
    rec.changes_at = Some("t1".into());
    rec.reviewers = vec![reviewer("rev", "changes"), reviewer("gone", "changes"), reviewer("ok", "approved")];
    rec.threads = vec![thread("1", "rev")];
    let h = fake(&b, rec);
    prflow::merge_flow(&b.app, id, vec![("swapped_off", json!(["gone"]))]).unwrap();
    poll(&b);
    assert_eq!(b.phase(id), "comments");
    let e = b.try_post(&format!("/tasks/T{id}/pr/addressed"), json!({})).unwrap_err();
    assert!(e.contains("still has 1 thread open: 1"), "{e}");
    b.post(&format!("/tasks/T{id}/pr/ack"), json!({"thread": "1"}));
    let v = b.post(&format!("/tasks/T{id}/pr/addressed"), json!({"who": "The agent"}));
    assert_eq!(v["asked"], json!(["REV"]), "not the reviewer swapped off");
    assert!(h.calls().contains(&"re-request rev".to_string()));
    assert_eq!(b.phase(id), "rereview");
    assert_eq!(b.flow(id)["addressed"]["asked"], json!(["REV"]));
}

#[test]
fn addressed_while_the_host_account_is_limited_ends_the_visit_until_it_lifts() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.review_decision = "CHANGES_REQUESTED".into();
    rec.changes_at = Some("t1".into());
    rec.reviewers = vec![reviewer("rev", "changes")];
    rec.threads = vec![thread("1", "rev")];
    let h = fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "comments");
    *h.held.lock() = Some(taskboardd::util::now_ts() + 600.0);
    *h.read_error.lock() = Some("Bitbucket rate-limited until 3:40 PM".into());
    let e = b.try_post(&format!("/tasks/T{id}/pr/addressed"), json!({"who": "The agent"})).unwrap_err();
    assert!(e.contains("Couldn't read PR #9") && e.contains("brings you back after"), "{e}");
    assert_eq!(b.phase(id), "comments", "nobody was asked again yet");
    assert!(b.flow(id)["retry_at"].is_string());
    assert!(b.flow(id).get("handled").is_none(), "the visit comes back for the same step");
}

/// Rate-limits the fake host's account: `held_until` says so and writes fail, as `Limits::admit` would.
fn limit(h: &FakeHost) {
    *h.held.lock() = Some(now_ts() + 600.0);
    *h.write_error.lock() = Some("Bitbucket rate-limited until 3:40 PM".into());
}

fn lift(h: &FakeHost) {
    *h.held.lock() = None;
    *h.write_error.lock() = None;
}

fn logged(b: &Board, id: i64, has: &str) -> bool {
    b.app.db.q("SELECT text FROM events WHERE task_id = ?", vec![json!(id)]).unwrap().iter().any(|r| r.st("text").contains(has))
}

/// The reviewer row the task card shows for `user`.
fn reviewer_row(b: &Board, id: i64, user: &str) -> Value {
    let card = board::task_card(&b.app, &b.task(id)).unwrap();
    card["pr"]["bar"]["reviewer_rows"].as_array().unwrap().iter().find(|r| r["user"] == user).cloned().unwrap_or(Value::Null)
}

/// A PR whose reviewer `rev` asked for changes at `t1`, with their one thread answered.
fn changes_asked(b: &Board) -> (i64, Arc<FakeHost>) {
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.review_decision = "CHANGES_REQUESTED".into();
    rec.changes_at = Some("2026-10-26T15:00:00Z".into());
    rec.reviewers = vec![Reviewer { changes_at: Some("2026-10-26T15:00:00Z".into()), ..reviewer("rev", "changes") }];
    rec.threads = vec![thread("1", "rev")];
    let h = fake(b, rec);
    poll(b);
    b.post(&format!("/tasks/T{id}/pr/ack"), json!({"thread": "1"}));
    (id, h)
}

#[test]
fn addressed_says_it_asked_through_the_host_and_the_row_shows_when() {
    let b = board_with(|_| {});
    let (id, h) = changes_asked(&b);
    let v = b.post(&format!("/tasks/T{id}/pr/addressed"), json!({"who": "The agent"}));
    assert_eq!((v["sent"].as_str(), v["asked"].clone()), (Some("host"), json!(["REV"])));
    assert!(h.calls().contains(&"re-request rev".to_string()));
    let row = reviewer_row(&b, id, "rev");
    assert_eq!((row["state"].as_str(), row["changes"].as_bool(), row["rereview_queued"].as_bool()), (Some("rereview"), Some(true), Some(false)), "{row}");
    assert!(row["rereview_at"].is_string() && row["asked_at"] == row["rereview_at"], "{row}");
}

#[test]
fn addressed_while_writes_are_limited_queues_the_ask_and_the_board_sends_it_once_the_limit_lifts() {
    let b = board_with(|_| {});
    let (id, h) = changes_asked(&b);
    limit(&h);
    let v = b.post(&format!("/tasks/T{id}/pr/addressed"), json!({"who": "The agent"}));
    assert_eq!(v["sent"], "queued", "{v}");
    assert!(v["until"].is_string() && v["site"] == "Bitbucket", "{v}");
    assert_eq!(b.phase(id), "rereview");
    let row = reviewer_row(&b, id, "rev");
    assert_eq!((row["state"].as_str(), row["rereview_queued"].as_bool()), (Some("rereview"), Some(true)), "{row}");
    assert!(row["rereview_at"].is_null(), "not asked yet: {row}");
    prcmds::ask_held_rereviews(&b.app).unwrap();
    assert_eq!(b.flow(id)["rereview_asks"]["rev"]["queued"], true, "still held: the ask waits");
    lift(&h);
    let before = h.calls().iter().filter(|c| *c == "re-request rev").count();
    prcmds::ask_held_rereviews(&b.app).unwrap();
    assert_eq!(h.calls().iter().filter(|c| *c == "re-request rev").count(), before + 1);
    let row = reviewer_row(&b, id, "rev");
    assert_eq!(row["rereview_queued"], false, "{row}");
    assert!(row["rereview_at"].is_string(), "{row}");
    assert!(logged(&b, id, "Asked REV to look again on PR #9, held over the rate limit"));
    prcmds::ask_held_rereviews(&b.app).unwrap();
    assert_eq!(h.calls().iter().filter(|c| *c == "re-request rev").count(), before + 1, "sent once");
}

#[test]
fn a_queued_ask_the_host_refuses_is_tried_again_not_dropped() {
    let b = board_with(|_| {});
    let (id, h) = changes_asked(&b);
    limit(&h);
    b.post(&format!("/tasks/T{id}/pr/addressed"), json!({"who": "The agent"}));
    *h.held.lock() = None;
    *h.write_error.lock() = Some("Bitbucket said 500".into());
    prcmds::ask_held_rereviews(&b.app).unwrap();
    let f = b.flow(id);
    assert_eq!(f["rereview_asks"]["rev"]["queued"], true, "kept for the next try");
    assert!(f["rereview_retry_at"].is_string(), "{f}");
    assert!(logged(&b, id, "Couldn't ask REV to look again on PR #9 yet"));
}

/// The reviewer `user`'s entry in what `tb pr status` reads.
fn status_reviewer(b: &Board, id: i64, user: &str) -> Value {
    let v = prcmds::status(&b.app, id).unwrap();
    v["reviewers"].as_array().unwrap().iter().find(|r| r["user"] == user).cloned().unwrap_or(Value::Null)
}

#[test]
fn asking_a_reviewer_who_asked_for_changes_shows_a_re_review_until_they_review_again() {
    let b = board_with(|_| {});
    let (id, h) = changes_asked(&b);
    let v = b.post(&format!("/tasks/T{id}/pr/reviewers"), json!({"ask": ["rev"], "who": "The agent"}));
    assert_eq!(v["asked"][0]["queued"], false, "{v}");
    assert!(h.calls().contains(&"re-request rev".to_string()), "{:?}", h.calls());
    let r = status_reviewer(&b, id, "rev");
    assert_eq!((r["state"].as_str(), r["rereview"]["queued"].as_bool()), (Some("changes"), Some(false)), "{r}");
    assert!(r["rereview"]["at"].is_string(), "{r}");
    let row = reviewer_row(&b, id, "rev");
    assert_eq!((row["state"].as_str(), row["changes"].as_bool(), row["rereview_queued"].as_bool()), (Some("rereview"), Some(true), Some(false)), "{row}");
    assert!(row["rereview_at"].is_string(), "{row}");
    let mut rec = h.rec.lock().clone();
    rec.reviewers = vec![reviewer("rev", "approved")];
    fake(&b, rec);
    poll(&b);
    assert_eq!(reviewer_row(&b, id, "rev")["state"], "approved");
    assert!(status_reviewer(&b, id, "rev")["rereview"].is_null());
}

#[test]
fn asking_a_reviewer_who_asked_for_changes_while_writes_are_limited_queues_the_re_review() {
    let b = board_with(|_| {});
    let (id, h) = changes_asked(&b);
    limit(&h);
    let v = b.post(&format!("/tasks/T{id}/pr/reviewers"), json!({"ask": ["rev"], "who": "The agent"}));
    assert_eq!(v["asked"][0]["queued"], true, "{v}");
    assert_eq!(status_reviewer(&b, id, "rev")["rereview"]["queued"], true);
    let row = reviewer_row(&b, id, "rev");
    assert_eq!((row["state"].as_str(), row["rereview_queued"].as_bool()), (Some("rereview"), Some(true)), "{row}");
    assert!(row["rereview_at"].is_null(), "not sent yet: {row}");
    lift(&h);
    prcmds::ask_held_rereviews(&b.app).unwrap();
    assert_eq!(h.calls().iter().filter(|c| *c == "re-request rev").count(), 2, "{:?}", h.calls());
    let row = reviewer_row(&b, id, "rev");
    assert_eq!(row["rereview_queued"], false, "{row}");
    assert!(row["rereview_at"].is_string(), "{row}");
}

#[test]
fn asking_a_new_reviewer_records_no_re_review() {
    let b = board_with(|_| {});
    let (id, h) = changes_asked(&b);
    let mut rec = h.rec.lock().clone();
    rec.reviewers.push(reviewer("bo", "pending"));
    let h = fake(&b, rec);
    poll(&b);
    b.post(&format!("/tasks/T{id}/pr/reviewers"), json!({"ask": ["bo"], "who": "The agent"}));
    assert!(h.calls().contains(&"request bo".to_string()), "{:?}", h.calls());
    assert!(b.flow(id)["rereview_asks"].get("bo").is_none(), "{}", b.flow(id));
    assert_eq!(reviewer_row(&b, id, "rev")["state"], "changes");
}

#[test]
fn the_re_review_ask_clears_when_that_reviewer_asks_for_changes_again_or_approves() {
    let b = board_with(|_| {});
    let (id, h) = changes_asked(&b);
    b.post(&format!("/tasks/T{id}/pr/addressed"), json!({"who": "The agent"}));
    assert_eq!(reviewer_row(&b, id, "rev")["state"], "rereview");
    let mut rec = h.rec.lock().clone();
    rec.reviewers = vec![Reviewer { changes_at: Some("2099-01-01T00:00:00Z".into()), ..reviewer("rev", "changes") }];
    fake(&b, rec.clone());
    poll(&b);
    let row = reviewer_row(&b, id, "rev");
    assert_eq!((row["state"].as_str(), row["rereview_at"].is_null()), (Some("changes"), true), "a new round: {row}");
    assert!(b.flow(id)["rereview_asks"].get("rev").is_none(), "{}", b.flow(id));
    rec.reviewers = vec![reviewer("rev", "approved")];
    fake(&b, rec);
    poll(&b);
    assert_eq!(reviewer_row(&b, id, "rev")["state"], "approved");
}

#[test]
fn a_reply_while_the_host_account_is_limited_is_posted_by_the_board_once_it_lifts() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.threads = vec![thread("1", "rev"), thread("2", "rev")];
    let h = fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "comments");
    limit(&h);
    let v = b.post(&format!("/tasks/T{id}/pr/reply"), json!({"thread": "1", "text": "Renamed it", "resolve": true, "who": "The agent"}));
    let held = v["held"].as_str().unwrap();
    assert!(held.contains("posts the reply and resolves the thread once the limit is over") && held.contains("don't wait"), "{held}");
    assert_eq!(v["open_threads"], 1, "thread 1 counts as answered now");
    assert_eq!(b.flow(id)["held_replies"].as_array().unwrap().len(), 1);
    assert!(logged(&b, id, "has the board rate-limited"));

    prcmds::post_held(&b.app).unwrap();
    assert_eq!(b.flow(id)["held_replies"].as_array().unwrap().len(), 1, "still limited: it waits");
    b.post(&format!("/tasks/T{id}/pr/reply"), json!({"thread": "2", "text": "Done"}));
    poll(&b);
    assert_eq!(b.phase(id), "review", "nothing is open while the board holds the answers");

    lift(&h);
    h.calls.lock().clear();
    prcmds::post_held(&b.app).unwrap();
    assert_eq!(h.calls(), vec!["reply 1 Renamed it", "resolve 1", "reply 2 Done"], "posted in the order they were held");
    assert!(b.flow(id)["held_replies"].as_array().unwrap().is_empty());
    assert!(h.rec.lock().threads[0].resolved);
    assert!(logged(&b, id, "Posted the held reply to") && logged(&b, id, "Resolved"));
    h.calls.lock().clear();
    prcmds::post_held(&b.app).unwrap();
    assert_eq!(h.calls(), Vec::<String>::new(), "nothing is posted twice");
    poll(&b);
    assert_eq!(b.phase(id), "review");
}

#[test]
fn an_ack_while_the_host_account_is_limited_resolves_the_thread_once_it_lifts() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.threads = vec![thread("1", "rev")];
    let h = fake(&b, rec);
    poll(&b);
    limit(&h);
    let v = b.post(&format!("/tasks/T{id}/pr/ack"), json!({"thread": "1"}));
    assert!(v["held"].as_str().unwrap().contains("resolves the thread once the limit is over"));
    assert_eq!(v["open_threads"], 0);
    lift(&h);
    h.calls.lock().clear();
    prcmds::post_held(&b.app).unwrap();
    assert_eq!(h.calls(), vec!["resolve 1"], "no reply, only the resolve");
    assert!(h.rec.lock().threads[0].resolved);
}

#[test]
fn a_write_that_fails_without_a_limit_is_not_held() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.threads = vec![thread("1", "rev")];
    let h = fake(&b, rec);
    poll(&b);
    *h.write_error.lock() = Some("thread is locked".into());
    let e = b.try_post(&format!("/tasks/T{id}/pr/reply"), json!({"thread": "1", "text": "Renamed it"})).unwrap_err();
    assert!(e.contains("Couldn't reply on PR #9: thread is locked"), "{e}");
    assert!(b.flow(id).get("held_replies").is_none());
}

#[test]
fn a_held_reply_the_host_then_refuses_for_another_reason_opens_the_thread_again() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.threads = vec![thread("1", "rev")];
    let h = fake(&b, rec);
    poll(&b);
    limit(&h);
    b.post(&format!("/tasks/T{id}/pr/reply"), json!({"thread": "1", "text": "Renamed it"}));
    poll(&b);
    assert_eq!(b.phase(id), "review");
    *h.held.lock() = None;
    *h.write_error.lock() = Some("thread is locked".into());
    prcmds::post_held(&b.app).unwrap();
    assert!(b.flow(id)["held_replies"].as_array().unwrap().is_empty(), "dropped, not tried forever");
    assert!(logged(&b, id, "Couldn't post the held reply to"));
    poll(&b);
    assert_eq!(b.phase(id), "comments", "the thread is open again for the agent");
}

#[test]
fn merge_checks_everything_first_then_merges_and_closes_the_branch() {
    let b = board_with(|c| {
        c.pr.agents_merge = Some(true);
        c.pr.projects.insert("webapp".into(), PrProject { approvals: Some(2), merge_strategy: Some("squash".into()), ..Default::default() });
    });
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.checks.push(check("e2e", "running"));
    rec.reviewers = vec![reviewer("a", "approved")];
    rec.approvals = 1;
    rec.threads = vec![thread("1", "rev"), Thread { kind: "task".into(), ..thread("task-5", "rev") }];
    rec.tasks_open = 1;
    let h = fake(&b, rec);
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    for want in ["checks are still running: e2e", "it has 1 of the 2 approvals it needs", "1 thread open", "1 PR task open"] {
        assert!(e.contains(want), "{want} in {e}");
    }
    {
        let mut r = h.rec.lock();
        r.checks.last_mut().unwrap().state = "passed".into();
        r.reviewers.push(reviewer("b", "approved"));
        r.threads.clear();
        r.tasks_open = 0;
    }
    poll(&b);
    assert_eq!(b.phase(id), "merge", "two approvals is enough for this project");
    b.post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true, "who": "The agent"}));
    assert!(h.calls().contains(&"merge close=true strategy=squash".to_string()));
    assert_eq!(b.phase(id), "merged");
}

#[test]
fn an_agent_can_t_merge_when_the_owner_merges() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let h = fake(&b, green());
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains("merges this project's PRs"), "{e}");
    assert!(h.calls().is_empty());
}

#[test]
fn the_owners_word_in_the_terminal_merges_one_pr_and_its_approved_counts() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let h = fake(&b, green());
    poll(&b);
    let said = |p: &str| b.report("hook.prompt", "s1", json!({"prompt": p}));
    let merge = || b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true, "via_session": "s1"}));
    said("merge PR #12");
    assert!(merge().unwrap_err().contains("merges this project's PRs"), "another PR's word");
    said("merge PR #9");
    assert!(merge().unwrap_err().contains("approv"), "the word allows the merge, not a missing approval");
    said("merge PR #9, approved");
    merge().unwrap();
    assert!(h.calls().iter().any(|c| c.starts_with("merge ")), "{:?}", h.calls());
    assert_eq!(b.phase(id), "merged");
}

#[test]
fn the_owner_closes_a_pr_and_an_agent_only_on_their_word() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let h = fake(&b, green());
    poll(&b);
    let said = |p: &str| b.report("hook.prompt", "s1", json!({"prompt": p}));
    let close = || b.try_post(&format!("/tasks/T{id}/pr/close"), json!({"agent": true, "via_session": "s1", "reason": "Went another way"}));
    assert!(close().unwrap_err().contains("haven't told you to close this one"), "no word");
    said("close PR #12");
    assert!(close().unwrap_err().contains("haven't told you to close this one"), "another PR's word");
    assert!(!h.calls().iter().any(|c| c.starts_with("close")), "{:?}", h.calls());
    said("decline PR #9");
    close().unwrap();
    assert!(h.calls().contains(&"close: Went another way".to_string()), "{:?}", h.calls());
    assert_eq!(b.phase(id), "declined");
    assert!(b.try_post(&format!("/tasks/T{id}/pr/close"), json!({})).unwrap_err().contains("already closed"));
}

#[test]
fn the_owner_closes_a_pr_from_their_own_shell_but_not_a_merged_one() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let h = fake(&b, green());
    b.post(&format!("/tasks/T{id}/pr/close"), json!({}));
    assert!(h.calls().contains(&"close".to_string()), "{:?}", h.calls());
    assert_eq!(b.phase(id), "declined");

    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.state = "MERGED".into();
    let h = fake(&b, rec);
    assert!(b.try_post(&format!("/tasks/T{id}/pr/close"), json!({})).unwrap_err().contains("already merged"));
    assert!(!h.calls().iter().any(|c| c.starts_with("close")));
}

#[test]
fn a_project_with_no_review_step_and_agents_merge_on_merges_without_anyone() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let h = fake(&b, green());
    poll(&b);
    assert_eq!(b.phase(id), "review");
    let v = b.post("/projects/webapp", json!({"review": "off", "agents_merge": "on"}));
    assert_eq!((v["pr_rules"]["review"].clone(), v["pr_rules"]["agents_merge"].clone()), (json!(false), json!(true)));
    poll(&b);
    assert_eq!(b.phase(id), "merge", "no reviewers to wait for");
    assert_eq!(b.get(&format!("/tasks/T{id}/pr"), &[])["agents_merge"], true);
    b.post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true}));
    assert!(h.calls().iter().any(|c| c.starts_with("merge ")), "{:?}", h.calls());
    assert_eq!(b.phase(id), "merged");
    let v = b.post("/projects/webapp", json!({"agents_merge": null}));
    assert_eq!(v["pr_rules"]["agents_merge"], false, "back to pr.agents_merge");
}

#[test]
fn the_board_wide_agents_merge_switch_sits_over_config_and_under_a_project() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    assert_eq!(b.get("/projects/agents-merge", &[]), json!({"agents_merge": false, "set": null, "config": false}));
    let v = b.post("/projects/agents-merge", json!({"agents_merge": "on"}));
    assert_eq!((v["agents_merge"].clone(), v["set"].clone()), (json!(true), json!(true)));
    assert_eq!(b.get(&format!("/tasks/T{id}/pr"), &[])["agents_merge"], true);
    b.post("/projects/webapp", json!({"agents_merge": "off"}));
    assert_eq!(b.get(&format!("/tasks/T{id}/pr"), &[])["agents_merge"], false, "the project's own rule wins");
    b.post("/projects/webapp", json!({"agents_merge": null}));
    let v = b.post("/projects/agents-merge", json!({"agents_merge": null}));
    assert_eq!((v["agents_merge"].clone(), v["set"].clone()), (json!(false), json!(null)));
    assert_eq!(b.get(&format!("/tasks/T{id}/pr"), &[])["agents_merge"], false, "back to config.toml's");
}

#[test]
fn a_stacked_base_must_merge_first() {
    let b = board_with(|_| {});
    let below = b.pr_task("https://bitbucket.org/acme/webapp/pull-requests/8");
    let mut low = green();
    low.branch = "base-work".into();
    fake(&b, low);
    poll(&b);
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.base = "base-work".into();
    rec.reviewers = vec![reviewer("a", "approved")];
    fake(&b, rec);
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({})).unwrap_err();
    assert!(e.contains(&format!("its base branch base-work is T{below}'s PR #8, which hasn't merged yet")), "{e}");
}

#[test]
fn not_ours_clears_only_this_push_s_failed_checks_with_proof() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let h = fake(&b, green());
    let url = format!("/tasks/T{id}/pr/not-ours");
    let good = json!({"reason": "The base branch fails the same e2e test on its last builds.", "title": "Flaky login e2e", "proof": ["https://ci.example.com/b/1"]});
    assert!(b.try_post(&url, good.clone()).unwrap_err().contains("Nothing failed"));
    {
        let mut r = h.rec.lock();
        r.checks = vec![check("build", "passed"), check("e2e", "failed"), check("lint", "failed")];
    }
    poll(&b);
    assert_eq!(b.phase(id), "fix");
    let short = json!({"reason": "flaky", "title": "x", "proof": ["https://ci.example.com/b/1"]});
    assert!(b.try_post(&url, short).unwrap_err().contains("20 to 300"));
    let mut no_proof = good.clone();
    no_proof["proof"] = json!(["see CI"]);
    assert!(b.try_post(&url, no_proof).unwrap_err().contains("isn't a link"));
    let mut wrong = good.clone();
    wrong["checks"] = json!(["build"]);
    assert!(b.try_post(&url, wrong).unwrap_err().contains("build didn't fail"));
    let mut e2e = good.clone();
    e2e["checks"] = json!(["e2e"]);
    let v = b.post(&url, e2e);
    assert_eq!(v["cleared"], json!(["e2e"]));
    assert_eq!(b.phase(id), "fix", "lint still fails");
    let mut lint = good.clone();
    lint["checks"] = json!(["lint"]);
    b.post(&url, lint);
    assert_eq!(b.phase(id), "review");
    let card = board::pr_card(&b.app, &b.task(id));
    assert_eq!(card["checks"], "pass");
    assert_eq!(card["stage"]["not_ours"]["title"], "Flaky login e2e");
    assert_eq!(card["stage"]["not_ours"]["checks"], json!(["e2e", "lint"]));
    // A new push has to pass on its own.
    h.rec.lock().head = "h2".into();
    poll(&b);
    assert_eq!(b.phase(id), "fix");
    assert!(board::pr_card(&b.app, &b.task(id))["stage"]["not_ours"].is_null());
}

#[test]
fn expected_checks_hold_the_stage_until_they_post_or_their_wait_runs_out() {
    let b = board_with(|c| {
        c.pr.projects.insert("webapp".into(), PrProject { expected: Some(vec!["build".into(), "E2E".into()]), expected_wait_mins: Some(30.0), ..Default::default() });
    });
    let id = b.pr_task(BB);
    let h = fake(&b, green());
    poll(&b);
    assert_eq!(b.phase(id), "checks", "e2e hasn't posted");
    h.rec.lock().checks.push(check("e2e", "passed"));
    poll(&b);
    assert_eq!(b.phase(id), "review");
    // Another push where e2e never posts: the wait runs out.
    {
        let mut r = h.rec.lock();
        r.head = "h2".into();
        r.checks.pop();
    }
    poll(&b);
    assert_eq!(b.phase(id), "checks");
    prflow::merge_flow(&b.app, id, vec![("head_at", json!({"h1": now_ts() - 3600.0, "h2": now_ts() - 31.0 * 60.0}))]).unwrap();
    poll(&b);
    assert_eq!(b.phase(id), "review");
}

#[test]
fn an_empty_expected_list_doesn_t_wait() {
    let b = board_with(|c| {
        c.pr.projects.insert("webapp".into(), PrProject { expected: Some(vec![]), ..Default::default() });
    });
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.checks.clear();
    fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "review", "no checks and none expected: no grace wait");
}

#[test]
fn a_pr_with_no_checks_moves_on_to_review_once_the_grace_runs_out_without_another_read() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.checks.clear();
    fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "checks", "a check may still register within the grace");
    let read_at = b.flow(id)["checked_at"].clone();
    prflow::waits_over(&b.app).unwrap();
    assert_eq!(b.phase(id), "checks", "the grace isn't over yet");
    // The grace runs out with no new read (a feed covers the PR, and nothing changes on it).
    prflow::merge_flow(&b.app, id, vec![("head_at", json!({"h1": now_ts() - 16.0 * 60.0}))]).unwrap();
    prflow::waits_over(&b.app).unwrap();
    assert_eq!(b.phase(id), "review");
    assert_eq!(b.flow(id)["checked_at"], read_at, "the last read's time stays");
}

fn bar_checks(b: &Board, id: i64) -> Value {
    board::task_card(&b.app, &b.task(id)).unwrap()["pr"]["bar"]["checks"].clone()
}

#[test]
fn expected_checks_that_haven_t_posted_aren_t_not_needed() {
    // #556: the Checks step said "Not needed" while tb pr status waited on build and e2e.
    let b = board_with(|c| {
        c.pr.projects.insert("webapp".into(), PrProject { expected: Some(vec!["build".into(), "e2e".into()]), expected_wait_mins: Some(30.0), ..Default::default() });
    });
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.checks.clear();
    fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "checks");
    assert_eq!(bar_checks(&b, id), "none_yet");
    // They waited out and never posted: still not "not needed", the stage shows its own state.
    prflow::merge_flow(&b.app, id, vec![("head_at", json!({"h1": now_ts() - 31.0 * 60.0}))]).unwrap();
    poll(&b);
    assert_eq!(b.phase(id), "review");
    assert_eq!(bar_checks(&b, id), Value::Null);
}

#[test]
fn no_checks_are_not_needed_only_once_the_grace_runs_out() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.checks.clear();
    fake(&b, rec);
    poll(&b);
    assert_eq!(bar_checks(&b, id), "none_yet", "a check may still register within the grace");
    prflow::merge_flow(&b.app, id, vec![("head_at", json!({"h1": now_ts() - 16.0 * 60.0}))]).unwrap();
    poll(&b);
    assert_eq!(bar_checks(&b, id), "not_needed");
}

#[test]
fn tb_pr_merge_waits_while_no_check_has_posted_within_the_grace() {
    // #491: with no expected checks set, an empty record held the stage but not the merge.
    let b = board_with(|c| c.pr.agents_merge = Some(true));
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.checks.clear();
    rec.reviewers = vec![reviewer("a", "approved")];
    fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "checks");
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains("no checks have posted on this push yet (waits up to 15 min)"), "{e}");
    prflow::merge_flow(&b.app, id, vec![("head_at", json!({"h1": now_ts() - 16.0 * 60.0}))]).unwrap();
    poll(&b);
    assert_eq!(b.phase(id), "merge");
    b.post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true}));
    assert_eq!(b.phase(id), "merged");
}

#[test]
fn a_new_push_waits_for_the_checks_its_last_push_posted() {
    // #491: a push with no checks yet isn't one that needs none when the last push posted some.
    let b = board_with(|c| c.pr.agents_merge = Some(true));
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.checks.push(check("e2e", "passed"));
    rec.reviewers = vec![reviewer("a", "approved")];
    let h = fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "merge");
    {
        let mut r = h.rec.lock();
        r.head = "h2".into();
        r.checks.clear();
    }
    poll(&b);
    prflow::merge_flow(&b.app, id, vec![("head_at", json!({"h1": now_ts() - 3600.0, "h2": now_ts() - 20.0 * 60.0}))]).unwrap();
    poll(&b);
    assert_eq!(b.phase(id), "checks", "past the no-checks grace, still within the expected wait");
    let live = b.get(&format!("/tasks/T{id}/pr"), &[("full", "1")])["live"].clone();
    assert_eq!(live["expected"]["source"], "earlier");
    assert_eq!(live["expected_missing"], json!(["build", "e2e"]));
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains("checks that posted on the last push haven't posted on this one: build, e2e (it waits up to 90 min for them)"), "{e}");
    h.rec.lock().checks.push(check("build", "passed"));
    poll(&b);
    assert_eq!(b.phase(id), "merge", "every check h2 posted passed, so e2e (only from the last push) gets just the short wait");
    // A third push: h2 is now the last push that posted, so only build is waited on.
    {
        let mut r = h.rec.lock();
        r.head = "h3".into();
        r.checks.clear();
    }
    poll(&b);
    let live = b.get(&format!("/tasks/T{id}/pr"), &[("full", "1")])["live"].clone();
    assert_eq!(live["expected_missing"], json!(["build"]));
    prflow::merge_flow(&b.app, id, vec![("head_at", json!({"h1": now_ts() - 3.0 * 3600.0, "h2": now_ts() - 2.0 * 3600.0, "h3": now_ts() - 91.0 * 60.0}))]).unwrap();
    poll(&b);
    assert_eq!(b.phase(id), "merge", "the expected wait ran out");
    b.post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true}));
    assert_eq!(b.phase(id), "merged");
}

/// An `expected_cmd` that answers by head (h1: build and E2E; h2: none; h3: can't tell; h4: fails) and
/// counts its runs in `runs`.
fn expected_cmd_board() -> (Board, tempfile::TempDir) {
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("runs").to_string_lossy().to_string();
    let b = board_with(|c| {
        let cmd = format!("echo \"$TB_HEAD $TB_BASE $TB_BRANCH $TB_PR_NUM\" >> '{log}'; ") + "case $TB_HEAD in h1) echo '[\"build\", \"E2E\"]';; h2) echo '[]';; h3) echo null;; *) echo nope >&2; exit 3;; esac";
        c.pr.projects.insert(
            "webapp".into(),
            PrProject { expected: Some(vec!["lint".into()]), expected_cmd: Some(cmd), expected_wait_mins: Some(30.0), ..Default::default() },
        );
    });
    (b, runs)
}

#[test]
fn expected_cmd_says_per_push_which_checks_hold_the_stage() {
    let (b, runs_dir) = expected_cmd_board();
    let id = b.pr_task(BB);
    let h = fake(&b, green());
    poll(&b);
    assert_eq!(b.phase(id), "checks", "h1 expects e2e, which hasn't posted");
    poll(&b);
    let runs = std::fs::read_to_string(runs_dir.path().join("runs")).unwrap();
    assert_eq!(runs, "h1 main feat 9\n", "run once per head, with the PR's env");
    let live = b.get(&format!("/tasks/T{id}/pr"), &[("full", "1")])["live"].clone();
    assert_eq!(live["expected"]["checks"], json!(["build", "E2E"]));
    assert_eq!(live["expected"]["source"], "command");
    assert_eq!(live["expected_missing"], json!(["E2E"]));
    h.rec.lock().checks.push(check("e2e", "passed"));
    poll(&b);
    assert_eq!(b.phase(id), "review");
    // h2 starts no pipeline: it doesn't wait, not even for the fixed list's lint.
    {
        let mut r = h.rec.lock();
        r.head = "h2".into();
        r.checks.clear();
    }
    poll(&b);
    assert_eq!(b.phase(id), "review", "[]: none expected, no grace wait");
    // h3 can't tell: the no_checks_after grace, as with no list.
    h.rec.lock().head = "h3".into();
    poll(&b);
    assert_eq!(b.phase(id), "checks", "null: nothing posted yet, within the grace");
    let live = b.get(&format!("/tasks/T{id}/pr"), &[("full", "1")])["live"].clone();
    assert!(live["expected"]["checks"].is_null());
    assert_eq!(live["expected"]["source"], "command");
    h.rec.lock().checks.push(check("build", "passed"));
    poll(&b);
    assert_eq!(b.phase(id), "review", "can't tell: whatever posted counts");
    // h4's command fails: can't tell too, with why.
    h.rec.lock().head = "h4".into();
    poll(&b);
    assert_eq!(b.phase(id), "review");
    let live = b.get(&format!("/tasks/T{id}/pr"), &[("full", "1")])["live"].clone();
    assert!(live["expected"]["checks"].is_null());
    assert!(live["expected"]["error"].as_str().unwrap().contains("nope"), "{live}");
    let runs = std::fs::read_to_string(runs_dir.path().join("runs")).unwrap();
    assert_eq!(runs.lines().count(), 4, "once per head: {runs}");
}

#[test]
fn expected_cmd_checks_wait_out_expected_wait_mins() {
    let (b, _runs) = expected_cmd_board();
    let id = b.pr_task(BB);
    fake(&b, green());
    poll(&b);
    assert_eq!(b.phase(id), "checks");
    prflow::merge_flow(&b.app, id, vec![("head_at", json!({"h1": now_ts() - 31.0 * 60.0}))]).unwrap();
    poll(&b);
    assert_eq!(b.phase(id), "review", "E2E never posted within the 30 minutes");
}

#[test]
fn expected_cmd_output_parses_as_a_list_or_can_t_tell() {
    assert_eq!(prflow::parse_expected("[\"a\", \"b\"]\n"), Ok(Some(vec!["a".into(), "b".into()])));
    assert_eq!(prflow::parse_expected("[]"), Ok(Some(vec![])));
    assert_eq!(prflow::parse_expected("null"), Ok(None));
    assert_eq!(prflow::parse_expected("  \n"), Ok(None));
    assert!(prflow::parse_expected("build").is_err());
    assert!(prflow::parse_expected("[1]").is_err());
}

#[test]
fn status_shows_failed_steps_base_failures_reviewers_threads_and_what_blocks_the_merge() {
    let b = board_with(|c| {
        c.pr.projects.insert(
            "webapp".into(),
            PrProject { approvals: Some(2), failures_cmd: Some("echo \"Run $TB_CHECK\"; echo test: login_works".into()), ..Default::default() },
        );
    });
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.checks = vec![check("build", "passed"), check("e2e", "failed")];
    rec.reviewers = vec![reviewer("a", "approved"), reviewer("r", "changes")];
    rec.threads = vec![Thread { author: "r".into(), ..thread("1", "rev") }];
    rec.review_decision = "CHANGES_REQUESTED".into();
    let h = fake(&b, rec);
    *h.base_failing.lock() = vec!["e2e".into()];
    poll(&b);
    h.rec.lock().base_head = "b2".into();
    h.rec.lock().mergeable = json!("CONFLICTING");
    let v = b.get(&format!("/tasks/T{id}/pr"), &[("full", "1")]);
    let live = &v["live"];
    assert_eq!(live["base_moved"], true);
    assert_eq!(live["conflicts"], true);
    let rebase: Vec<&str> = live["rebase"].as_array().unwrap().iter().filter_map(|c| c.as_str()).collect();
    assert_eq!(rebase[rebase.len() - 2..], ["git push --force-with-lease origin feat", "Don't push only to rebase."], "{rebase:?}");
    let f =&live["failures"][0];
    assert_eq!(f["check"], "e2e");
    assert_eq!(f["base_fails"], true);
    assert_eq!(f["steps"], json!(["Run e2e"]));
    assert_eq!(f["tests"], json!(["login_works"]));
    assert_eq!(f["source"], "the project's failures_cmd");
    assert!(h.calls().contains(&"base main 5".to_string()));
    assert_eq!(live["reviewers"].as_array().unwrap().len(), 2);
    assert_eq!(live["approvals"], json!({"have": 1, "need": 2}));
    assert_eq!(live["open_threads"][0]["id"], "1");
    let blockers: Vec<&str> = live["blockers"].as_array().unwrap().iter().filter_map(|x| x.as_str()).collect();
    assert_eq!(blockers, vec!["it conflicts with its base branch main: rebase onto it and push first", "checks failed: e2e", "R asked for changes", "1 thread open"]);
    assert_eq!(v["watched"], true);
}

#[test]
fn a_partly_approved_pr_reads_1_of_2_approved_and_approved_at_the_count_it_needs() {
    let b = board_with(|c| {
        c.pr.projects.insert("webapp".into(), PrProject { approvals: Some(2), ..Default::default() });
    });
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.reviewers = vec![reviewer("a", "approved"), reviewer("b", "pending"), reviewer("c", "pending")];
    let h = fake(&b, rec);
    poll(&b);
    assert_eq!(b.task(id).st("pr_review"), "1 of 2 approved");
    let card = board::task_card(&b.app, &b.task(id)).unwrap();
    assert_eq!((card["pr"]["bar"]["approvals"].as_i64(), card["pr"]["bar"]["need"].as_i64()), (Some(1), Some(2)));
    // Bitbucket never says APPROVED: two of three reviewers are enough.
    h.rec.lock().reviewers = vec![reviewer("a", "approved"), reviewer("b", "approved"), reviewer("c", "pending")];
    poll(&b);
    assert_eq!(b.task(id).st("pr_review"), "Approved");
}

#[test]
fn the_pr_bar_has_a_pill_per_reviewer_from_the_host() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.review_decision = "CHANGES_REQUESTED".into();
    rec.changes_at = Some("t1".into());
    rec.reviewers = vec![reviewer("ok", "approved"), reviewer("rev", "changes"), reviewer("new", "pending"), reviewer("gone", "changes")];
    rec.threads = vec![Thread { resolved: true, ..thread("1", "rev") }];
    fake(&b, rec);
    prflow::merge_flow(&b.app, id, vec![("swapped_off", json!(["gone"]))]).unwrap();
    poll(&b);
    let pills = |b: &Board| -> Vec<(String, String)> {
        let card = board::task_card(&b.app, &b.task(id)).unwrap();
        card["pr"]["bar"]["reviewer_rows"].as_array().unwrap().iter().map(|r| (r["user"].as_str().unwrap().to_string(), r["state"].as_str().unwrap().to_string())).collect()
    };
    let s = |v: &[(&str, &str)]| v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>();
    assert_eq!(pills(&b), s(&[("ok", "approved"), ("rev", "changes"), ("new", "waiting")]), "the swapped-off reviewer has no pill");
    let card = board::task_card(&b.app, &b.task(id)).unwrap();
    assert_eq!((card["pr"]["bar"]["approvals"].as_i64(), card["pr"]["bar"]["reviewers"].as_i64()), (Some(1), Some(3)));
    b.post(&format!("/tasks/T{id}/pr/addressed"), json!({}));
    assert_eq!(pills(&b), s(&[("ok", "approved"), ("rev", "rereview"), ("new", "waiting")]));
}

#[test]
fn a_stacked_pr_is_retargeted_through_its_host() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let h = fake(&b, green());
    taskboardd::propen::host::retarget(&b.app, &b.task(id), "develop").unwrap();
    assert!(h.calls().contains(&"retarget develop".to_string()), "a Bitbucket PR moves too");
}

#[test]
fn a_thread_waits_on_the_board_s_own_account_and_unread_tasks_hold_the_merge() {
    // #49: the board posts as "bot", not as the PR's author "me".
    let b = board_with(|c| c.pr.agents_merge = Some(true));
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.viewer = "bot".into();
    rec.threads = vec![thread("1", "bot"), thread("2", "rev")];
    rec.reviewers = vec![reviewer("a", "approved"), reviewer("c", "approved")];
    rec.approvals = 2;
    rec.tasks_error = Some("Bitbucket answered 403".into());
    let h = fake(&b, rec);
    poll(&b);
    let card = board::pr_card(&b.app, &b.task(id));
    assert_eq!(card["stage"]["open_threads"], 1, "the board's own account answered 1");
    b.post(&format!("/tasks/T{id}/pr/reply"), json!({"thread": "2", "text": "Done"}));
    assert_eq!(h.rec.lock().threads[1].last_author, "bot", "a reply is the board's account's");
    // #77: green and approved, but its tasks are unknown: the stage holds as the merge does, and
    // nobody is woken for a merge that would be refused.
    poll(&b);
    assert_eq!(b.phase(id), "review", "unread tasks hold the stage");
    assert!(!b.flow(id)["woke"].as_str().unwrap_or("").starts_with("merge"), "no wake to merge: {}", b.flow(id));
    let e =b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains("couldn't read its PR tasks (Bitbucket answered 403)"), "{e}");
    assert!(!e.contains("thread"), "{e}");
    let live = b.get(&format!("/tasks/T{id}/pr"), &[("full", "1")])["live"].clone();
    assert!(live["tasks_open"].is_null(), "unknown, not 0");
    assert_eq!(live["tasks_error"], "Bitbucket answered 403");
    h.rec.lock().tasks_error = None;
    b.post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true}));
    assert_eq!(b.phase(id), "merged");
}

#[test]
fn the_board_s_own_approval_doesn_t_count() {
    // #77: like the PR's author, the board's own account doesn't approve its PRs.
    let b = board_with(|c| c.pr.approvals = 2);
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.viewer = "bot".into();
    rec.reviewers = vec![reviewer("a", "approved"), reviewer("bot", "approved")];
    rec.approvals = 2;
    let h = fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "review", "one approval of the two it needs");
    assert_eq!(board::task_card(&b.app, &b.task(id)).unwrap()["pr"]["bar"]["approvals"], 1);
    h.rec.lock().reviewers.push(reviewer("c", "approved"));
    poll(&b);
    assert_eq!(b.phase(id), "merge");
}

#[test]
fn the_card_s_review_is_approved_only_once_it_has_the_approvals_it_needs() {
    // #146: `pr.review` (what `tb pr status` prints and the panel reads without a bar) goes by the count.
    for (host, url, decision) in [("bitbucket", BB, ""), ("github", "https://github.com/acme/webapp/pull/9", "APPROVED")] {
        let b = board_with(|c| c.pr.approvals = 2);
        let id = b.pr_task(url);
        let mut rec = green();
        rec.reviewers = vec![reviewer("a", "approved"), reviewer("c", "waiting")];
        rec.approvals = 1;
        rec.review_decision = decision.into();
        let h = FakeHost::new(host, rec);
        prhost::install(&b.app, h.clone());
        poll(&b);
        let pr = &b.get(&format!("/tasks/T{id}/pr"), &[("full", "1")])["pr"];
        assert_eq!(pr["review"], "pending", "{host}: 1 of the 2 it needs");
        assert_eq!(pr["approvals"], json!({"have": 1, "need": 2}), "{host}");
        assert_eq!(board::task_card(&b.app, &b.task(id)).unwrap()["pr"]["review"], "pending", "{host}: the panel's card too");
        h.rec.lock().reviewers[1].state = "approved".into();
        h.rec.lock().approvals = 2;
        poll(&b);
        let pr = board::pr_card(&b.app, &b.task(id));
        assert_eq!((pr["review"].clone(), pr["approvals"].clone()), (json!("approved"), json!({"have": 2, "need": 2})), "{host}");
    }
}

#[test]
fn a_pr_needs_two_approvals_unless_its_project_is_set_otherwise_with_tb() {
    // #50: the board's default is 2 (tests otherwise run with the host's verdict).
    let b = board_with(|c| c.pr.approvals = taskboardd::config::DEFAULT_APPROVALS);
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.reviewers = vec![reviewer("a", "approved")];
    rec.approvals = 1;
    rec.review_decision = "APPROVED".into();
    fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "review", "one approval isn't enough, whatever the host says");
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({})).unwrap_err();
    assert!(e.contains("it has 1 of the 2 approvals it needs"), "{e}");
    let v = b.post("/projects/webapp", json!({"approvals": 1}));
    assert_eq!(v["pr_rules"]["approvals"], 1);
    assert_eq!(v["pr_rules"]["set"], json!({"approvals": 1}));
    assert_eq!(v["pr_flow"], "auto", "the PR flow is left alone");
    poll(&b);
    assert_eq!(b.phase(id), "merge");
    assert!(b.try_post("/projects/webapp", json!({"approvals": -1})).unwrap_err().contains("0 (the host's own decision) to 20"));
    let v = b.post("/projects/webapp", json!({"approvals": null}));
    assert_eq!(v["pr_rules"]["approvals"], 2, "back to the board's default");
    poll(&b);
    assert_eq!(b.phase(id), "review");
    assert!(b.try_post("/projects/webapp", json!({})).unwrap_err().contains("Say what to change"));
}

#[test]
fn an_expected_check_that_never_posts_stops_holding_the_merge_once_the_wait_is_over() {
    // #51: expected checks set with tb; after the wait only failed checks block the merge.
    let b = board_with(|c| c.pr.agents_merge = Some(true));
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.reviewers = vec![reviewer("a", "approved")];
    fake(&b, rec);
    let v = b.post("/projects/webapp", json!({"expected": ["build", "E2E"], "expected_wait_mins": 30}));
    assert_eq!(v["pr_rules"]["expected"], json!(["build", "E2E"]));
    assert_eq!(v["pr_rules"]["expected_wait_mins"], 30.0);
    poll(&b);
    assert_eq!(b.phase(id), "checks", "e2e hasn't posted");
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains("expected checks haven't posted: E2E (it waits up to 30 min for them)"), "{e}");
    prflow::merge_flow(&b.app, id, vec![("head_at", json!({"h1": now_ts() - 31.0 * 60.0}))]).unwrap();
    poll(&b);
    assert_eq!(b.phase(id), "merge");
    let live = b.get(&format!("/tasks/T{id}/pr"), &[("full", "1")])["live"].clone();
    assert_eq!((live["expected_missing"].clone(), live["expected_waited_out"].clone()), (json!(["E2E"]), json!(true)));
    assert!(live["blockers"].as_array().unwrap().is_empty(), "{live}");
    b.post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true}));
    assert_eq!(b.phase(id), "merged");
    let v = b.post("/projects/webapp", json!({"expected": null, "expected_wait_mins": null}));
    assert!(v["pr_rules"]["expected"].is_null(), "back to config.toml's (none)");
    assert!(b.try_post("/projects/webapp", json!({"expected": [""]})).unwrap_err().contains("needs a name"));
}

#[test]
fn status_blames_the_base_per_test_and_says_how_to_rebase_and_what_was_replied() {
    // #52: the PR's run fails two tests, the base's runs of the same check only one.
    let b = board_with(|c| {
        c.pr.projects.insert(
            "webapp".into(),
            PrProject { failures_cmd: Some("echo 'Run e2e'; echo test: login_works; [ -n \"$TB_HEAD\" ] && echo test: logout_works; true".into()), ..Default::default() },
        );
        c.pr.rebase = RebasePolicy::Moved;
    });
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.checks = vec![check("e2e", "failed")];
    let mut th = thread("1", "rev");
    th.replies = vec![
        taskboardd::prhost::Reply { author: "me".into(), author_name: "Me".into(), text: "Why rename it?".into(), at: "t2".into(), ..Default::default() },
        taskboardd::prhost::Reply { author: "rev".into(), author_name: "Rev".into(), text: "It clashes with the other one".into(), at: "t3".into(), ..Default::default() },
    ];
    rec.threads = vec![th];
    let h = fake(&b, rec);
    *h.base_checks.lock() = vec![Check { name: "e2e".into(), state: "failed".into(), url: Some("https://ci.example.com/base/1".into()), at: None }];
    poll(&b);
    h.rec.lock().base_head = "b2".into();
    let live = b.get(&format!("/tasks/T{id}/pr"), &[("full", "1")])["live"].clone();
    let f = &live["failures"][0];
    assert_eq!(f["tests"], json!(["login_works", "logout_works"]));
    assert_eq!(f["base_tests"], json!(["login_works"]), "only the test the base fails too");
    assert_eq!((f["base_fails"].clone(), f["base_compared"].clone()), (json!(false), json!("steps")), "logout_works is this PR's");
    assert_eq!(live["rebase"][0], "git fetch origin main && git rebase origin/main");
    assert_eq!(live["open_threads"][0]["replies"][1]["text"], "It clashes with the other one");
}

#[test]
fn status_asks_for_a_rebase_only_as_the_rebase_policy_says() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let h = fake(&b, green());
    poll(&b);
    let rebase = |b: &Board| b.get(&format!("/tasks/T{id}/pr"), &[("full", "1")])["live"]["rebase"].as_array().unwrap().len();
    h.rec.lock().base_head = "b2".into();
    h.rec.lock().mergeable = json!("MERGEABLE");
    let live = b.get(&format!("/tasks/T{id}/pr"), &[("full", "1")])["live"].clone();
    assert_eq!((live["base_moved"].clone(), live["conflicts"].clone(), live["rebase_policy"].clone()), (json!(true), json!(false), json!("conflicts")));
    assert_eq!(rebase(&b), 0, "the base moved, but the PR merges as it is: no rebase by default");
    h.rec.lock().mergeable = json!(null);
    assert_eq!(rebase(&b), 0, "the host can't tell: no rebase");
    h.rec.lock().mergeable = json!("CONFLICTING");
    assert!(rebase(&b) > 0, "it conflicts: rebase");

    let never = board_with(|c| {
        c.pr.projects.insert("webapp".into(), PrProject { rebase: Some(RebasePolicy::Never), ..Default::default() });
    });
    let id = never.pr_task(BB);
    let h = fake(&never, green());
    poll(&never);
    h.rec.lock().base_head = "b2".into();
    h.rec.lock().mergeable = json!("CONFLICTING");
    let live = never.get(&format!("/tasks/T{id}/pr"), &[("full", "1")])["live"].clone();
    assert_eq!((live["rebase_policy"].clone(), live["rebase"].clone()), (json!("never"), json!([])), "the project's own policy wins");
}

#[test]
fn merging_moves_the_prs_stacked_on_it_onto_its_base_first() {
    // #50: the merge deletes the branch, so the PR into it is pointed at the base before.
    let b = board_with(|_| {});
    let parent = b.pr_task(BB);
    let mut rec = green();
    rec.branch = "base-work".into();
    rec.reviewers = vec![reviewer("a", "approved")];
    let h = fake(&b, rec);
    poll(&b);
    let child = b.pr_task("https://bitbucket.org/acme/webapp/pull-requests/10");
    let child_rec = Record { base: "base-work".into(), branch: "more-work".into(), head: "h9".into(), ..green() }.to_value();
    prflow::merge_flow(&b.app, child, vec![("rec", child_rec)]).unwrap();
    b.post(&format!("/tasks/T{parent}/pr/merge"), json!({}));
    let calls = h.calls();
    let at = |c: &str| calls.iter().position(|x| x.starts_with(c)).unwrap_or_else(|| panic!("{c} in {calls:?}"));
    assert!(at("retarget main") < at("merge "), "{calls:?}");
    assert_eq!(b.flow(child)["retargeted"], "main");
    assert_eq!(b.phase(parent), "merged");
    let said: Vec<String> = b.app.db.q("SELECT text FROM events WHERE task_id = ? ORDER BY id", taskboardd::p![child]).unwrap().iter().map(|r| r.st("text")).collect();
    assert!(said.iter().any(|l| l.contains("now goes into main")), "{said:?}");
}

const MERGE_STEPS: &str = r#"
[[steps]]
name = "Staging check"
before = "merge"
prompt = "Check it on staging."
skippable = true

[[steps]]
name = "Release note"
before = "merge"
prompt = "Write the release note."
default_branch_only = true
"#;

fn step_report(b: &Board, id: i64, extra: Value) -> Result<Value, (u16, String)> {
    let mut body = json!({"event": "tb.step", "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}, "task": format!("T{id}")});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    reports::handle(&b.app, body, false).map_err(|e| (e.status, e.message))
}

#[test]
fn a_merge_step_holds_only_the_merge_until_it_passes_on_the_head() {
    // #477: tb submit and the review go on; the merge waits for the steps on the PR's head.
    let b = board_with(|c| c.pr.agents_merge = Some(true));
    std::fs::write(&b.app.cfg.config_path, MERGE_STEPS).unwrap();
    let id = b.pr_task(BB);
    assert_eq!(b.task(id).st("status"), "done", "merge steps don't hold tb submit");
    let mut rec = green();
    rec.head = "abc1234".into();
    rec.reviewers = vec![reviewer("a", "approved")];
    let h = fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "steps");
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains("its merge steps haven't passed on its head: “Staging check”, “Release note”"), "{e}");
    let bar = taskboardd::prbar::bar(&b.app, &b.task(id)).unwrap();
    assert_eq!(bar["merge_steps"].as_array().unwrap().iter().map(|s| s["state"].as_str().unwrap()).collect::<Vec<_>>(), vec!["todo", "todo"]);

    // Release note isn't skippable; a skip needs a reason.
    let (code, why) = step_report(&b, id, json!({"name": "Release note", "skip": true, "why": "none", "head": "abc1234"})).unwrap_err();
    assert_eq!(code, 409);
    assert!(why.contains("isn't skippable"), "{why}");
    let (code, _) = step_report(&b, id, json!({"name": "Staging check", "skip": true, "why": "", "head": "abc1234"})).unwrap_err();
    assert_eq!(code, 400);

    step_report(&b, id, json!({"name": "Staging check", "skip": true, "why": "docs only", "head": "abc1234"})).unwrap();
    step_report(&b, id, json!({"name": "Release note", "head": "abc1234"})).unwrap();
    poll(&b);
    assert_eq!(b.phase(id), "merge");
    let bar = taskboardd::prbar::bar(&b.app, &b.task(id)).unwrap();
    assert_eq!(bar["merge_steps"][0]["state"], "skipped");
    assert_eq!(bar["merge_steps"][0]["why"], "docs only");
    assert_eq!(bar["merge_steps"][1]["state"], "passed");

    // A new push needs them again: they count per head.
    h.rec.lock().head = "def5678".into();
    poll(&b);
    assert!(b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err().contains("merge steps"));
    step_report(&b, id, json!({"name": "Staging check", "skip": true, "why": "docs only", "head": "def5678"})).unwrap();
    step_report(&b, id, json!({"name": "Release note", "head": "def5678"})).unwrap();
    b.post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true}));
    assert_eq!(b.phase(id), "merged");
}

/// The board's PR visits for a task, oldest first: each one's prompt.
fn wakes(b: &Board, id: i64) -> Vec<String> {
    let rows = b.app.db.q("SELECT * FROM jobs WHERE kind = 'agent' AND purpose = 'pr' AND task_id = ? ORDER BY id", p![id]).unwrap();
    rows.iter().map(|r| board::job_args(r).st("prompt")).collect()
}

fn visits_done(b: &Board, id: i64) {
    b.app.db.x("UPDATE jobs SET state = 'done' WHERE purpose = 'pr' AND task_id = ?", p![id]).unwrap();
}

fn conflicting(mut rec: Record) -> Record {
    rec.mergeable = json!("CONFLICTING");
    rec
}

#[test]
fn a_green_approved_pr_that_conflicts_goes_to_rebase_not_merge() {
    let b = board_with(|c| {
        c.pr.wake = true;
        c.pr.agents_merge = Some(true);
    });
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.reviewers = vec![reviewer("a", "approved")];
    let h = fake(&b, conflicting(rec));
    poll(&b);
    assert_eq!(b.phase(id), "rebase");
    let w = wakes(&b, id);
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("It conflicts with its base branch main") && w[0].contains("git push --force-with-lease") && w[0].contains("pr wait"), "{}", w[0]);
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains("it conflicts with its base branch main"), "{e}");
    assert!(!h.calls().iter().any(|c| c.starts_with("merge ")), "{:?}", h.calls());
    // One conflict wakes once.
    visits_done(&b, id);
    poll(&b);
    assert_eq!(wakes(&b, id).len(), 1, "the same conflict doesn't wake again");
    // The base moves on and still conflicts: that's a new conflict.
    h.rec.lock().base_head = "b2".into();
    poll(&b);
    assert_eq!(wakes(&b, id).len(), 2, "a new base wakes it again");
    // The rebased push merges cleanly.
    visits_done(&b, id);
    {
        let mut r = h.rec.lock();
        r.head = "h2".into();
        r.mergeable = json!("MERGEABLE");
    }
    poll(&b);
    assert_eq!(b.phase(id), "merge");
}

#[test]
fn a_conflict_goes_to_rebase_with_the_review_step_off() {
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let h = fake(&b, conflicting(green()));
    b.post("/projects/webapp", json!({"review": "off", "agents_merge": "on"}));
    poll(&b);
    assert_eq!(b.phase(id), "rebase");
    h.rec.lock().mergeable = json!("MERGEABLE");
    poll(&b);
    assert_eq!(b.phase(id), "merge");
}

#[test]
fn a_conflict_while_waiting_on_a_reviewer_wakes_the_agent() {
    let b = board_with(|c| c.pr.wake = true);
    let id = b.pr_task(BB);
    let h = fake(&b, green());
    poll(&b);
    assert_eq!(b.phase(id), "review");
    assert!(wakes(&b, id).is_empty());
    h.rec.lock().mergeable = json!("CONFLICTING");
    poll(&b);
    assert_eq!(b.phase(id), "rebase");
    assert_eq!(wakes(&b, id).len(), 1);
}

#[test]
fn a_conflict_doesnt_ask_for_a_rebase_when_the_policy_is_never() {
    let b = board_with(|c| c.pr.rebase = RebasePolicy::Never);
    let id = b.pr_task(BB);
    fake(&b, conflicting(green()));
    poll(&b);
    assert_eq!(b.phase(id), "review");
}

#[test]
fn a_merge_the_host_refuses_for_a_conflict_goes_to_rebase() {
    let b = board_with(|c| {
        c.pr.wake = true;
        c.pr.agents_merge = Some(true);
    });
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.reviewers = vec![reviewer("a", "approved")];
    let h = fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "merge");
    visits_done(&b, id);
    *h.merge_error.lock() = Some("Pull Request is not mergeable: the merge commit cannot be cleanly created".into());
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains("can't merge: it conflicts with its base branch main"), "{e}");
    assert_eq!(b.phase(id), "rebase", "not merge retries");
    assert!(b.flow(id).get("merge_tries").is_none());
    let w = wakes(&b, id);
    assert!(w.last().unwrap().contains("It conflicts with its base branch main"), "{w:?}");
    // The host's record still can't tell, but the refusal stands for this head and base.
    poll(&b);
    assert_eq!(b.phase(id), "rebase");
    *h.merge_error.lock() = None;
    h.rec.lock().head = "h2".into();
    poll(&b);
    assert_eq!(b.phase(id), "merge", "the rebased push can merge again");
}

#[test]
fn a_merge_that_hits_a_rate_limit_ends_the_visit_until_it_lifts() {
    let b = board_with(|c| {
        c.pr.wake = true;
        c.pr.agents_merge = Some(true);
    });
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.reviewers = vec![reviewer("a", "approved")];
    let h = fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "merge");
    assert_eq!(wakes(&b, id).len(), 1);
    *h.held.lock() = Some(taskboardd::util::now_ts() + 600.0);
    *h.merge_error.lock() = Some("429 Too Many Requests".into());
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains("Couldn't merge PR #9") && e.contains("brings you back after"), "{e}");
    assert!(b.flow(id).get("woke").is_none());
    assert!(b.flow(id).get("merge_tries").is_none(), "not a merge left open");
    visits_done(&b, id);
    poll(&b);
    assert_eq!(wakes(&b, id).len(), 1, "not while the limit lasts");
    *h.held.lock() = None;
    *h.merge_error.lock() = None;
    b.app.db.x("UPDATE tasks SET pr_flow = json_set(pr_flow, '$.retry_at', '2000-01-01T00:00:00Z') WHERE id = ?", p![id]).unwrap();
    poll(&b);
    assert_eq!(wakes(&b, id).len(), 2, "back to merge once it's over");
}

#[test]
fn a_check_only_an_earlier_push_posted_holds_the_pr_only_for_the_short_wait() {
    let b = board_with(|c| c.pr.agents_merge = Some(true));
    let id = b.pr_task(BB);
    let mut rec = green();
    rec.checks.push(check("pages", "passed"));
    rec.reviewers = vec![reviewer("a", "approved")];
    let h = fake(&b, rec);
    poll(&b);
    assert_eq!(b.phase(id), "merge");
    // The new push posts build but no longer runs pages.
    {
        let mut r = h.rec.lock();
        r.head = "h2".into();
        r.checks = vec![check("build", "passed")];
    }
    poll(&b);
    prflow::merge_flow(&b.app, id, vec![("head_at", json!({"h1": now_ts() - 3600.0, "h2": now_ts() - 5.0 * 60.0}))]).unwrap();
    poll(&b);
    assert_eq!(b.phase(id), "checks", "within the short wait");
    let e = b.try_post(&format!("/tasks/T{id}/pr/merge"), json!({"agent": true})).unwrap_err();
    assert!(e.contains("checks that posted on the last push haven't posted on this one: pages (it waits up to 15 min for them)"), "{e}");
    prflow::merge_flow(&b.app, id, vec![("head_at", json!({"h1": now_ts() - 3600.0, "h2": now_ts() - 16.0 * 60.0}))]).unwrap();
    poll(&b);
    assert_eq!(b.phase(id), "merge", "not the full 90 min");
}

#[test]
fn the_poll_reads_a_pr_in_full_only_when_its_stamp_moves_or_its_last_full_read_is_an_hour_old() {
    // #676: one batch call says what changed; unchanged PRs are stepped with their last record.
    let b = board_with(|_| {});
    let id = b.pr_task(BB);
    let h = fake(&b, green());
    *h.stamp.lock() = Some("s1".into());
    poll(&b);
    assert_eq!(reads(&h), 1, "the first poll has nothing to compare with");
    assert_eq!(b.flow(id)["stamp"], "s1");
    prflow::merge_flow(&b.app, id, vec![("read_at", json!(iso(now_ts() - 600.0)))]).unwrap();
    poll(&b);
    assert_eq!(reads(&h), 1, "the stamp didn't move");
    let read_at = b.flow(id)["read_at"].as_str().unwrap().to_string();
    assert!(now_ts() - taskboardd::util::parse_iso(&read_at).unwrap() < 60.0, "asking what changed is a read: {read_at}");
    assert_eq!(h.calls().iter().filter(|c| c.starts_with("stamps")).count(), 2);

    let mut rec = green();
    rec.threads = vec![thread("1", "rev")];
    *h.rec.lock() = rec;
    *h.stamp.lock() = Some("s2".into());
    poll(&b);
    assert_eq!(reads(&h), 2, "the stamp moved");
    assert_eq!(b.phase(id), "comments");
    poll(&b);
    assert_eq!(reads(&h), 2);
    assert_eq!(b.phase(id), "comments", "stepped with the last record");

    prflow::merge_flow(&b.app, id, vec![("full_at", json!(iso(now_ts() - 3601.0)))]).unwrap();
    poll(&b);
    assert_eq!(reads(&h), 3, "a stamp can miss a change, so the PR is read in full every hour");

    *h.stamp.lock() = None;
    poll(&b);
    assert_eq!(reads(&h), 4, "no stamp: read in full");
    assert!(b.flow(id).get("stamp").is_none(), "{}", b.flow(id));
}

#[test]
fn full_reads_only_due_by_age_are_spread_over_polls_oldest_first() {
    // #743: after a start every PR's hourly full read was due, and the first poll read all 17 at once.
    let b = board_with(|_| {});
    let h = fake(&b, green());
    *h.stamp.lock() = Some("s1".into());
    let ids: Vec<i64> = (1..=5).map(|n| b.pr_task(&format!("https://bitbucket.org/acme/webapp/pull-requests/{n}"))).collect();
    poll(&b);
    assert_eq!(reads(&h), 5, "a PR never read is read at once");
    for (i, id) in ids.iter().enumerate() {
        prflow::merge_flow(&b.app, *id, vec![("full_at", json!(iso(now_ts() - 7200.0 - i as f64 * 60.0))), ("read_at", json!(iso(now_ts() - 600.0)))]).unwrap();
    }
    poll(&b);
    assert_eq!(reads(&h), 8, "a few a poll");
    let read_now = |id: i64| now_ts() - taskboardd::util::parse_iso(b.flow(id)["full_at"].as_str().unwrap()).unwrap() < 60.0;
    assert_eq!(ids.iter().map(|id| read_now(*id)).collect::<Vec<_>>(), vec![false, false, true, true, true], "oldest first");
    let asked_now = |id: i64| now_ts() - taskboardd::util::parse_iso(b.flow(id)["read_at"].as_str().unwrap()).unwrap() < 60.0;
    assert!(asked_now(ids[0]) && asked_now(ids[1]), "the ones left for later still count as read: the host said they hadn't changed");
    poll(&b);
    assert_eq!(reads(&h), 10, "the rest on the next poll");
    poll(&b);
    assert_eq!(reads(&h), 10);

    *h.stamp.lock() = Some("s2".into());
    poll(&b);
    assert_eq!(reads(&h), 15, "a moved stamp is read at once, however many moved");

    *h.held.lock() = Some(now_ts() + 300.0);
    *h.stamp.lock() = Some("s3".into());
    poll(&b);
    assert_eq!(reads(&h), 20, "the host said what moved: a limited account still reads it");
}

#[test]
fn a_pr_whose_full_read_keeps_failing_doesnt_hold_its_turn() {
    // Three PRs that can't be read (gone, or the token lost the repo) took every turn, and the rest were never read.
    let b = board_with(|_| {});
    let h = fake(&b, green());
    *h.stamp.lock() = Some("s1".into());
    let ids: Vec<i64> = (1..=5).map(|n| b.pr_task(&format!("https://bitbucket.org/acme/webapp/pull-requests/{n}"))).collect();
    poll(&b);
    for (i, id) in ids.iter().enumerate() {
        prflow::merge_flow(&b.app, *id, vec![("full_at", json!(iso(now_ts() - 7200.0 - i as f64 * 60.0)))]).unwrap();
    }
    *h.read_fails.lock() = vec![3, 4, 5];
    poll(&b);
    assert_eq!(reads(&h), 8, "the three oldest are tried, and fail");
    let read_now = |id: i64| now_ts() - taskboardd::util::parse_iso(b.flow(id)["full_at"].as_str().unwrap()).unwrap() < 60.0;
    assert!(!read_now(ids[0]) && !read_now(ids[1]));
    poll(&b);
    assert_eq!(reads(&h), 11);
    assert!(read_now(ids[0]) && read_now(ids[1]), "a failed try goes to the back");
}

#[test]
fn when_the_host_cant_say_what_changed_a_limited_account_waits_and_the_rest_are_spread() {
    // #743: the read-everything fallback ran while the account was limited, and went out once it lifted.
    let b = board_with(|_| {});
    let h = fake(&b, green());
    *h.stamp.lock() = Some("s1".into());
    let ids: Vec<i64> = (1..=5).map(|n| b.pr_task(&format!("https://bitbucket.org/acme/webapp/pull-requests/{n}"))).collect();
    poll(&b);
    assert_eq!(reads(&h), 5);
    for (i, id) in ids.iter().enumerate() {
        prflow::merge_flow(&b.app, *id, vec![("full_at", json!(iso(now_ts() - 600.0 - i as f64 * 60.0))), ("read_at", json!(iso(now_ts() - 600.0)))]).unwrap();
    }
    *h.stamps_error.lock() = Some("Bitbucket answered 429".into());
    *h.held.lock() = Some(now_ts() + 300.0);
    poll(&b);
    assert_eq!(reads(&h), 5, "nothing is read while the account is limited");
    *h.held.lock() = None;
    poll(&b);
    assert_eq!(reads(&h), 8, "a few a poll once it lifts");
    let read_now = |id: i64| now_ts() - taskboardd::util::parse_iso(b.flow(id)["full_at"].as_str().unwrap()).unwrap() < 60.0;
    assert_eq!(ids.iter().map(|id| read_now(*id)).collect::<Vec<_>>(), vec![false, false, true, true, true], "oldest first");
    let asked_now = |id: i64| now_ts() - taskboardd::util::parse_iso(b.flow(id)["read_at"].as_str().unwrap()).unwrap() < 60.0;
    assert!(!asked_now(ids[0]) && !asked_now(ids[1]), "nothing said they hadn't changed: not read, so the relay's reconcile still asks");
    poll(&b);
    assert_eq!(reads(&h), 11, "still unknown: every PR stays due, so the reads go round a few a poll");
    assert!(read_now(ids[0]) && read_now(ids[1]), "the two left behind go first");
}
