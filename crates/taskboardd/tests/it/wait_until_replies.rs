//! A turn that ends saying its task waits until a set time (a trigger wakes it at 06:30 UTC) without
//! `tb wait-until`: the Stop is sent back once with the exact line, the board parks the task itself when
//! it ends that way again, and it never shows as a question for the owner.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{advance_clock, now_ts, reset_clock, RowExt};
use taskboardd::{api, board, midna, reports, wait_until};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

/// A board whose clock reads `now` (RFC 3339), with T1 working in terminal s1.
fn board_at(now: &str) -> Board {
    reset_clock();
    advance_clock(chrono::DateTime::parse_from_rfc3339(now).unwrap().timestamp() as f64 - now_ts());
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let web = dir.path().join("webapp");
    std::fs::create_dir_all(&web).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": web.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": web.to_string_lossy(), "status": {"state": "working"}})], &[])
        .unwrap();
    let b = Board { app, _dir: dir };
    api::dispatch(&b.app, "POST", "/tasks", &api::Query::new(), &json!({"title": "Check the first nightly build", "detail": "Do the thing.", "project": "webapp"}))
        .unwrap();
    b.report("tb.take", json!({"task": "T1"}));
    b
}

impl Board {
    fn report(&self, event: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {"branch": "feature/x", "sha": "abc123"}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message))
    }
    fn stop(&self, reply: &str, end: &str, again: bool) -> Value {
        self.report("hook.stop", json!({"last_message": reply, "last_message_end": end, "reply": reply, "stop_hook_active": again}))
    }
    fn idle_prompt(&self, reply: &str) {
        let dir = self.app.cfg.claude_projects.join("webapp");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c-s1.jsonl");
        std::fs::write(&path, json!({"type": "assistant", "message": {"content": [{"type": "text", "text": reply}]}}).to_string()).unwrap();
        self.report("hook.attention", json!({"notification_type": "idle_prompt", "message": "Claude is waiting for your input", "transcript_path": path.to_string_lossy()}));
    }
    fn task(&self) -> taskboardd::util::Row {
        board::get_task(&self.app, 1).unwrap()
    }
    /// Not put to the owner: still working, no question.
    fn not_on_the_owner(&self) {
        let t = self.task();
        assert_eq!(t.s("status"), Some("working"), "{:?}", t.s("question"));
        assert!(t.s("question").is_none() && t.s("needs_reason").is_none(), "{:?}", t.s("question"));
    }
}

/// T811's reply (another board, 5:23 PM MST on Nov 23), which the board filed as "Asks you".
const T811: &str = "Nothing more can be checked before the 05:00 UTC run. Trigger `t_b3b364` wakes this terminal at 06:30 UTC. Then I'll check pipeline 16 with `az`, attach the results, and draft the reply for Steve's thread. I'll show you the exact text and won't send it until you approve.";
const T811_AT: &str = "2026-11-24T00:23:00Z";

/// T3075's reply (this board, Nov 23): a wait until 4:15 AM that ends on an offer, filed as "Waiting for you".
const T3075: &str = "I'll wait for tonight, as the board answered. It says the scan, mood tagging and build_memories shouldn't be run by hand, so you'll get the first real results tomorrow morning.\n\n- **Photo picks:** the photos that have nothing to do with the memory are now task T3137 on the memories goal. It will run with the rest of the goal.\n- **Song check:** this terminal picks it back up at 4:15 AM Nov 24, after tonight's scan, build_memories and nightly sort. It checks how many songs the scan added, whether mood jobs were queued, and whether build_memories finished. Memories can't get mood songs, Christmas songs and beat timing before the Nov 25 build, so I'll check again then. After that I'll attach the results page with songs per mood and a sample of memories.\n\nIf you'd rather see it sooner, tell me to run it by hand and I'll do it today. It takes about 1.5 hours and writes to prod, the same writes the nightly makes.";
const T3075_AT: &str = "2026-11-23T20:42:10Z";

#[test]
fn a_reply_that_says_a_trigger_wakes_it_is_sent_back_with_the_wait_until_line() {
    let b = board_at(T811_AT);
    let out = b.stop(T811, T811, false);
    let block = out["block"].as_str().expect("the stop is sent back");
    assert!(block.contains("tb wait-until \"06:30 UTC\" --then \"check pipeline 16 with az, attach the results"), "{block}");
    assert!(!block.contains('`'), "nothing a shell would run: {block}");
    b.not_on_the_owner();
    assert!(!wait_until::waiting(&b.task()), "not parked yet");
    assert_eq!(board::get_session(&b.app, Some("s1")).unwrap().unwrap().s("status"), Some("working"));
    reset_clock();
}

#[test]
fn ending_again_still_saying_so_parks_it_and_its_idle_prompt_stays_off_the_owner() {
    let b = board_at(T811_AT);
    b.stop(T811, T811, false);
    let out = b.stop(T811, T811, true);
    assert!(out.get("block").is_none(), "{out}");
    let t = b.task();
    assert!(wait_until::waiting(&t), "parked");
    assert_eq!(t.s("waiting_until"), Some("2026-11-24T06:30:00Z"));
    assert!(t.st("resume_plan").starts_with("check pipeline 16"), "{:?}", t.s("resume_plan"));
    b.idle_prompt(T811);
    b.not_on_the_owner();
    reset_clock();
}

#[test]
fn running_the_line_after_being_sent_back_is_left_alone() {
    let b = board_at(T811_AT);
    b.stop(T811, T811, false);
    b.report("tb.wait_until", json!({"at": "06:30 UTC", "then": "check pipeline 16 and draft the reply"}));
    let out = b.stop(T811, T811, true);
    assert!(out.get("block").is_none(), "{out}");
    assert_eq!(b.task().s("resume_plan"), Some("check pipeline 16 and draft the reply"), "its own plan stands");
    reset_clock();
}

#[test]
fn an_idle_prompt_on_a_timed_wait_parks_it_instead_of_asking_the_owner() {
    let b = board_at(T811_AT);
    b.idle_prompt(T811);
    b.not_on_the_owner();
    let t = b.task();
    assert!(wait_until::waiting(&t), "parked by the board");
    assert_eq!(t.s("waiting_until"), Some("2026-11-24T06:30:00Z"));
    assert_eq!(board::get_session(&b.app, Some("s1")).unwrap().unwrap().s("status"), Some("idle"));
    reset_clock();
}

#[test]
fn a_timed_wait_that_ends_on_an_offer_is_not_screened_as_a_question() {
    let b = board_at(T3075_AT);
    let end = T3075.rsplit("\n\n").take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n\n");
    let out = b.stop(T3075, &end, false);
    let block = out["block"].as_str().expect("sent back to park, not screened as a question");
    assert!(block.contains("tb wait-until \"2026-11-24T04:15\" --then \"It checks how many songs"), "{block}");
    b.not_on_the_owner();
    b.stop(T3075, &end, true);
    b.idle_prompt(T3075);
    b.not_on_the_owner();
    assert!(wait_until::waiting(&b.task()));
    reset_clock();
}

#[test]
fn a_time_that_is_not_a_wait_is_left_to_the_usual_checks() {
    let b = board_at(T811_AT);
    let out = b.stop("The nightly build runs at 05:00 UTC and I fixed its retry step.", "The nightly build runs at 05:00 UTC and I fixed its retry step.", false);
    assert!(out.get("block").is_none(), "{out}");
    assert!(!wait_until::waiting(&b.task()));
    reset_clock();
}
