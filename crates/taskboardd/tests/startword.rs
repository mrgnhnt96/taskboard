//! `tb start T4`: an agent starts a task only on the word a human typed in its terminal.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, p, reports};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    Board { app, _dir: dir }
}

impl Board {
    fn report(&self, event: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": ""});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap()
    }
    fn said(&self, prompt: &str) {
        self.report("hook.prompt", json!({"prompt": prompt}));
    }
    /// What the agent's `tb task new` adds: a task that waits for Start.
    fn new_task(&self) -> i64 {
        let v = self.report("tb.new_task", json!({"title": "Add login"}));
        v["created"][0].as_str().unwrap().trim_start_matches('T').parse().unwrap()
    }
    /// What `tb start T<id>` sends from terminal s1.
    fn tb_start(&self, id: i64) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", &format!("/tasks/T{id}/start"), &Query::new(), &json!({"mode": "queue", "via_session": "s1"}))
            .map_err(|e| (e.status, e.message))
    }
    fn status(&self, id: i64) -> String {
        board::get_task(&self.app, id).unwrap().st("status")
    }
    fn log(&self, id: i64) -> Vec<String> {
        self.app.db.q("SELECT text FROM events WHERE task_id = ? ORDER BY id", p![id]).unwrap().iter().map(|r| r.st("text")).collect()
    }
}

#[test]
fn an_agent_starts_a_task_on_the_owners_word() {
    let b = board();
    b.said("create a new task for the login page and queue it");
    let id = b.new_task();
    assert_eq!(b.status(id), "queued");
    assert!(b.tb_start(id).unwrap()["starting"] == true);
    let by = format!("Started by {} via Term", b.app.cfg.owner);
    assert!(b.log(id).contains(&by), "{:?}", b.log(id));
}

#[test]
fn without_the_owners_word_the_start_is_refused() {
    let b = board();
    let id = b.new_task();
    let refused = |b: &Board| {
        let (code, why) = b.tb_start(id).unwrap_err();
        assert_eq!(code, 403);
        assert!(why.contains("Only a human can start"), "{why}");
        assert_eq!(b.status(id), "queued");
    };
    refused(&b);
    b.said("make a task for the login page but don't start it");
    refused(&b);
    // The board's own prompt (here handing this terminal another task) is no one's word.
    let other = b.new_task();
    b.said(&format!("[task-board:T{other}] You are picking up “Add login”. Start T{id} when you're ready."));
    refused(&b);
    b.said("start T99");
    refused(&b);
    b.said("thanks, looks good");
    refused(&b);

    // The word names the task, so a later prompt doesn't take it back.
    b.said(&format!("ok, start T{id}"));
    b.said("and tell me when it's going");
    assert!(b.tb_start(id).unwrap()["starting"] == true);
}

#[test]
fn the_word_is_for_this_conversation_only() {
    let b = board();
    let id = b.new_task();
    b.said(&format!("start T{id}"));
    b.report("hook.session_start", json!({"source": "clear"}));
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);
}

#[test]
fn the_boards_start_says_it_came_from_the_ui() {
    let b = board();
    let id = b.new_task();
    let from_app: Query = [(api::FROM.to_string(), "app".to_string())].into_iter().collect();
    api::dispatch(&b.app, "POST", &format!("/tasks/T{id}/start"), &from_app, &json!({"mode": "queue"})).unwrap();
    assert!(b.log(id).contains(&"Started in the UI".to_string()), "{:?}", b.log(id));
}

#[test]
fn a_start_with_no_terminal_is_only_the_apps() {
    let b = board();
    let id = b.new_task();
    let e = api::dispatch(&b.app, "POST", &format!("/tasks/T{id}/start"), &Query::new(), &json!({"mode": "queue"})).unwrap_err();
    assert_eq!(e.status, 403);
    assert!(e.message.contains("Only a human can start"), "{}", e.message);
    assert_eq!(b.status(id), "queued");
    assert!(!b.log(id).iter().any(|l| l.starts_with("Started")), "{:?}", b.log(id));
    // An empty via_session is no terminal either.
    let e = api::dispatch(&b.app, "POST", &format!("/tasks/T{id}/start"), &Query::new(), &json!({"mode": "queue", "via_session": ""}))
        .unwrap_err();
    assert_eq!(e.status, 403);
}

#[test]
fn a_prompt_about_a_start_is_no_word() {
    let b = board();
    let id = b.new_task();
    for said in [
        format!("why did T{id} start failing yesterday?"),
        format!("T{id} started an hour ago, right?"),
        format!("is T{id} starting?"),
        format!("did you start T{id}?"),
        format!("don't start T{id}"),
        format!("restart T{id}"),
        format!("the log says start T{id} failed"),
    ] {
        b.said(&said);
        assert_eq!(b.tb_start(id).unwrap_err().0, 403, "{said}");
    }
}

#[test]
fn an_unnamed_ask_is_only_for_a_task_this_conversation_made_after_it() {
    let b = board();
    // Made before the prompt: "it" isn't this task.
    let id = b.new_task();
    b.said("start the dev server and check the login page");
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);
    b.said("queue it");
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);

    // A task made elsewhere (the board's API, not this terminal) after the prompt isn't it either.
    b.said("make a task for the login page and queue it");
    let other = api::dispatch(&b.app, "POST", "/tasks", &Query::new(), &json!({"title": "Other", "project": "webapp"})).unwrap();
    let other = other["id"].as_i64().unwrap();
    assert_eq!(b.tb_start(other).unwrap_err().0, 403);

    // Made by this conversation after the prompt: that's the word.
    let made = b.new_task();
    assert!(b.tb_start(made).unwrap()["starting"] == true);

    // Once a later prompt comes in, the unnamed one no longer counts.
    b.said("make a task for the signup page and queue it");
    let next = b.new_task();
    b.said("thanks");
    assert_eq!(b.tb_start(next).unwrap_err().0, 403);
}

#[test]
fn a_later_prompt_that_takes_the_start_back_wins() {
    let b = board();
    let id = b.new_task();
    for back in [format!("wait, don't start T{id}"), "actually, don't start it".to_string(), format!("hold off on T{id}"), "wait".into(), format!("start T{id} tomorrow instead")] {
        b.said(&format!("start T{id}"));
        b.said(&back);
        assert_eq!(b.tb_start(id).unwrap_err().0, 403, "{back}");
    }
    // Asked again after taking it back: that's the word.
    b.said(&format!("ok, start T{id}"));
    assert!(b.tb_start(id).unwrap()["starting"] == true);
}

#[test]
fn a_task_waiting_on_unfinished_work_does_not_start_even_on_the_word() {
    let b = board();
    let first = b.new_task();
    let v = b.report("tb.new_task", json!({"title": "Add logout", "waits_for": [format!("T{first}")]}));
    let id: i64 = v["created"][0].as_str().unwrap().trim_start_matches('T').parse().unwrap();
    b.said(&format!("start T{id}"));
    let (code, why) = b.tb_start(id).unwrap_err();
    assert_eq!(code, 409);
    assert!(why.contains(&format!("T{id} can't start yet. Blocked by T{first}")), "{why}");
    assert_eq!(b.status(id), "queued");

    // Once the work it needs is done, the same word starts it.
    board::update_task(&b.app, first, vec![("status", json!("done"))]).unwrap();
    assert!(b.tb_start(id).unwrap()["starting"] == true);
}

#[test]
fn a_no_a_later_time_a_condition_a_question_or_a_paste_is_no_word() {
    let b = board();
    let id = b.new_task();
    for said in [
        format!("never, ever, start T{id}"),
        format!("do not, under any circumstances, start T{id}"),
        format!("wait until 6am, then start T{id}"),
        format!("start T{id} on Monday"),
        format!("start T{id} in two weeks"),
        format!("start T{id} when T7 lands"),
        format!("start T{id} once I say so"),
        format!("start T{id}?"),
        format!("so, start T{id}?"),
        format!("the log says: start T{id}."),
        format!("here's the log\n  start T{id}"),
        format!("look at this:\nstart T{id}\nstart T{id}"),
        format!("> start T{id}"),
        format!("```\nstart T{id}\n```"),
    ] {
        b.said(&said);
        assert_eq!(b.tb_start(id).unwrap_err().0, 403, "{said:?}");
        assert_eq!(b.status(id), "queued");
    }
}

#[test]
fn every_board_marker_is_the_boards_prompt() {
    let b = board();
    let id = b.new_task();
    for marker in ["G1", "J3", "T99", "P2", "anything at all"] {
        b.said(&format!("[task-board:{marker}] Plan the goal. Start T{id} now."));
        assert_eq!(b.tb_start(id).unwrap_err().0, 403, "{marker}");
        let data = b.app.db.q("SELECT data FROM session_events WHERE kind = 'prompt' ORDER BY id DESC LIMIT 1", p![]).unwrap();
        assert_eq!(data[0].st("data"), "board", "{marker}");
    }
}

#[test]
fn a_singular_unnamed_ask_covers_only_the_first_task_made_after_it() {
    let b = board();
    b.said("make two tasks for the login page and queue it");
    let first = b.new_task();
    let second = b.new_task();
    assert_eq!(b.tb_start(second).unwrap_err().0, 403);
    assert!(b.tb_start(first).unwrap()["starting"] == true);

    // An unnamed "start it" that asks for no new task is about something else.
    b.said("the dev server won't come up; start it");
    let later = b.new_task();
    assert_eq!(b.tb_start(later).unwrap_err().0, 403);
}

#[test]
fn a_plural_unnamed_ask_covers_every_task_made_in_reply_to_it() {
    let b = board();
    b.said("make tasks for A and B and queue them");
    let a = b.new_task();
    let c = b.new_task();
    assert!(b.tb_start(a).unwrap()["starting"] == true);
    assert!(b.tb_start(c).unwrap()["starting"] == true);

    // Made before the ask, or by a later prompt's reply: not "them".
    let before = b.new_task();
    b.said("make two tasks for the signup page and queue both");
    let x = b.new_task();
    let y = b.new_task();
    assert_eq!(b.tb_start(before).unwrap_err().0, 403);
    b.said("thanks");
    let after = b.new_task();
    for id in [x, y, after] {
        assert_eq!(b.tb_start(id).unwrap_err().0, 403, "T{id}");
    }

    // A task made in reply to the board's prompt isn't made in reply to the owner's.
    b.said("make tasks for C and D and queue these");
    b.said("[task-board:G2] Plan the goal.");
    let planned = b.new_task();
    assert_eq!(b.tb_start(planned).unwrap_err().0, 403);
}

#[test]
fn queue_it_after_the_task_was_made_is_the_word() {
    let b = board();
    b.said("make a task for the footer");
    let id = b.new_task();
    b.said("queue it");
    assert!(b.tb_start(id).unwrap()["starting"] == true);

    // Plural, for every task the prompt before made.
    b.said("make tasks for the header and the sidebar");
    let h = b.new_task();
    let s = b.new_task();
    b.said("ok, queue them");
    assert!(b.tb_start(h).unwrap()["starting"] == true);
    assert!(b.tb_start(s).unwrap()["starting"] == true);
}

#[test]
fn queue_it_after_the_task_was_made_is_no_word_when_it_may_mean_something_else() {
    let b = board();
    // Something newer was said in between.
    b.said("make a task for the footer");
    let id = b.new_task();
    b.said("thanks");
    b.said("queue it");
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);

    // The prompt says more than the ask: "it" may be the dev server.
    b.said("make a task for the footer");
    let id = b.new_task();
    b.said("the dev server won't come up; start it");
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);

    // Two tasks were made: which is "it"?
    b.said("make tasks for the header and the sidebar");
    let h = b.new_task();
    let s = b.new_task();
    b.said("queue it");
    assert_eq!(b.tb_start(h).unwrap_err().0, 403);
    assert_eq!(b.tb_start(s).unwrap_err().0, 403);

    // The agent made it without being asked for a task.
    b.said("fix the footer");
    let id = b.new_task();
    b.said("queue it");
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);

    // Taken back.
    b.said("make a task for the footer");
    let id = b.new_task();
    b.said("queue it");
    b.said("nope");
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);
}

#[test]
fn the_check_reads_the_prompt_as_typed_not_the_line_it_shows() {
    let b = board();
    let id = b.new_task();
    for said in [
        format!("paste:\nstart T{id}"),
        format!("CI failed with:\n\n    start T{id}"),
        format!("my notes:\n\nstart T{id}"),
        format!("see the thread:\n\n> start T{id}"),
        format!("it printed this\n  start T{id}\n  start T{id}"),
    ] {
        b.said(&said);
        assert_eq!(b.tb_start(id).unwrap_err().0, 403, "{said:?}");
    }
    // The terminal's history still shows the prompt on one line.
    let shown = b.app.db.q1("SELECT text, full FROM session_events WHERE kind = 'prompt' ORDER BY id DESC LIMIT 1", p![]).unwrap().unwrap();
    assert_eq!(shown.st("text"), format!("it printed this start T{id} start T{id}"));
    assert_eq!(shown.st("full"), format!("it printed this\n  start T{id}\n  start T{id}"));

    // The owner's own line after the paste is the word.
    b.said(&format!("here's the log:\n  ERROR x\n\nok, start T{id}"));
    assert!(b.tb_start(id).unwrap()["starting"] == true);
}

#[test]
fn the_check_reads_past_the_first_600_characters() {
    let b = board();
    let id = b.new_task();
    let filler = "The footer overlaps the cookie banner on small screens and the links wrap badly. ".repeat(9);
    assert!(filler.len() > 650);
    b.said(&format!("start T{id}. {filler} Wait until tomorrow though."));
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);
    b.said(&format!("start T{id}. {filler} jk"));
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);
    // A long prompt that doesn't take it back is still the word.
    b.said(&format!("start T{id}. {filler}"));
    assert!(b.tb_start(id).unwrap()["starting"] == true);
}

#[test]
fn a_prompt_clipped_on_its_way_in_is_no_word() {
    let b = board();
    let id = b.new_task();
    // What the hook sends for a prompt past its 8000 characters: the start, then "…".
    let long = format!("start T{id}. {}", "x ".repeat(4100));
    let clipped: String = long.chars().take(7999).collect::<String>().trim_end().to_string() + "…";
    b.said(&clipped);
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);
    // A later prompt that names the task is read on its own.
    b.said(&format!("ok, start T{id}"));
    assert!(b.tb_start(id).unwrap()["starting"] == true);
}

#[test]
fn a_take_back_or_a_deferral_after_the_ask_in_the_same_prompt_wins() {
    let b = board();
    let id = b.new_task();
    for said in [
        format!("start T{id}. jk"),
        format!("start T{id}. On second thought, don't."),
        format!("start T{id}. no."),
        format!("start T{id}, scratch that"),
        format!("start T{id}. Not now though."),
        format!("start T{id}, first thing"),
        format!("start T{id}, at six"),
        format!("start T{id}, the moment T7 lands"),
        format!("don't (like T9), start T{id}"),
    ] {
        b.said(&said);
        assert_eq!(b.tb_start(id).unwrap_err().0, 403, "{said}");
        assert_eq!(b.status(id), "queued");
    }
}

#[test]
fn a_later_prompt_that_names_no_task_can_take_the_ask_back() {
    let b = board();
    let id = b.new_task();
    for back in ["no, don't", "nope", "never mind that", "forget I said that", "I changed my mind"] {
        b.said(&format!("start T{id}"));
        b.said(back);
        assert_eq!(b.tb_start(id).unwrap_err().0, 403, "{back}");
    }
    // "start T9 instead" takes back the start of T8, and is the word for T9.
    let other = b.new_task();
    b.said(&format!("start T{id}"));
    b.said(&format!("start T{other} instead"));
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);
    assert!(b.tb_start(other).unwrap()["starting"] == true);
}

#[test]
fn an_ask_for_the_goal_points_the_agent_at_the_goal() {
    let b = board();
    let g = api::dispatch(&b.app, "POST", "/goals", &Query::new(), &json!({"name": "Settings", "project": "webapp"})).unwrap()["id"]
        .as_i64()
        .unwrap();
    let v = b.report("tb.new_task", json!({"title": "Chips", "goal": format!("G{g}")}));
    let id: i64 = v["created"][0].as_str().unwrap().trim_start_matches('T').parse().unwrap();
    b.said("You can start the goal");
    let (code, why) = b.tb_start(id).unwrap_err();
    assert_eq!(code, 403);
    assert!(why.contains(&format!("run tb start G{g}")), "{why}");
}
