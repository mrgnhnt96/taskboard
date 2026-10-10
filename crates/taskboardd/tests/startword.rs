//! `tb start T4`: an agent starts a task only on the word a human typed in its terminal.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, p, reports, startword};

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
    // Something newer was asked for in between: "it" may be that.
    b.said("make a task for the footer");
    let id = b.new_task();
    b.said("also fix the typo in the header");
    b.said("queue it");
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);
    // Only thanks in between (#128) leaves "it" the task.
    b.said("make a task for the footer");
    let id = b.new_task();
    b.said("thanks");
    b.said("queue it");
    assert!(b.tb_start(id).unwrap()["starting"] == true);

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
fn a_prompt_the_old_hook_clipped_is_no_word_but_takes_nothing_back() {
    let b = board();
    let id = b.new_task();
    // What the hook before beta.16 sent for a prompt past its 8000 characters: the start, then "…".
    let long = format!("start T{id}. {}", "x ".repeat(4100));
    let clipped: String = long.chars().take(7999).collect::<String>().trim_end().to_string() + "…";
    b.said(&clipped);
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);
    // It doesn't take back an ask before it.
    b.said(&format!("start T{id}"));
    b.said(&("x ".repeat(4000).trim_end().to_string() + "…"));
    assert!(b.tb_start(id).unwrap()["starting"] == true);
    // A later prompt that names the task is read on its own.
    let id = b.new_task();
    b.said(&clipped.replace(&format!("T{}", id - 1), &format!("T{id}")));
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

impl Board {
    fn goal(&self) -> i64 {
        api::dispatch(&self.app, "POST", "/goals", &Query::new(), &json!({"name": "Settings", "project": "webapp"})).unwrap()["id"].as_i64().unwrap()
    }
    /// A planned task in goal `g`, made elsewhere (not by this terminal's conversation).
    fn planned(&self, g: i64) -> i64 {
        let id = api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &json!({"title": "Chips", "project": "webapp"}))
            .unwrap()["id"]
            .as_i64()
            .unwrap();
        board::update_task(&self.app, id, vec![("status", json!("planned")), ("goal_id", json!(g))]).unwrap();
        id
    }
    /// What `tb start G<g>` and `tb goal set G<g> --run` send from terminal s1.
    fn tb_run(&self, g: i64) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", &format!("/goals/G{g}/run"), &Query::new(), &json!({"via_session": "s1"})).map_err(|e| (e.status, e.message))
    }
}

#[test]
fn an_agent_runs_a_goal_only_on_the_owners_word() {
    let b = board();
    let g = b.goal();
    let id = b.planned(g);
    let (code, why) = b.tb_run(g).unwrap_err();
    assert_eq!(code, 403);
    assert!(why.contains(&format!("Only a human can run G{g}")), "{why}");
    assert_eq!(b.status(id), "planned");

    for no in [format!("did you run G{g}?"), format!("run G{g} tomorrow"), format!("the log says: run G{g}")] {
        b.said(&no);
        assert_eq!(b.tb_run(g).unwrap_err().0, 403, "{no}");
    }
    // "The goal" alone means no goal this conversation has.
    b.said("start the goal");
    assert_eq!(b.tb_run(g).unwrap_err().0, 403);
    b.said(&format!("[task-board:G{g}] Plan the goal. Run G{g}."));
    b.said("thanks");
    assert_eq!(b.tb_run(g).unwrap_err().0, 403, "the board's prompt is no one's word");

    for yes in [format!("start G{g}"), format!("run G{g}"), format!("ok, kick off goal G{g}")] {
        b.said(&yes);
        assert!(b.tb_run(g).is_ok(), "{yes}");
    }
    assert_eq!(b.status(id), "queued");

    b.said(&format!("wait, don't run G{g}"));
    assert_eq!(b.tb_run(g).unwrap_err().0, 403);
}

#[test]
fn the_goal_unnamed_is_this_conversations_goal() {
    let b = board();
    let g = b.goal();
    let other = b.goal();
    b.planned(g);
    b.planned(other);
    // The conversation made a task in goal g: "the goal" is g, not the other.
    b.report("tb.new_task", json!({"title": "Chips", "goal": format!("G{g}")}));
    b.said("You can start the goal");
    assert!(b.tb_run(g).is_ok());
    assert_eq!(b.tb_run(other).unwrap_err().0, 403);

    // A conversation the board handed the goal to.
    b.report("hook.session_start", json!({"source": "clear"}));
    b.said(&format!("[task-board:G{other}] Plan the goal."));
    b.said("looks good, run the goal");
    assert!(b.tb_run(other).is_ok());
    assert_eq!(b.tb_run(g).unwrap_err().0, 403);
}

#[test]
fn the_boards_run_needs_no_word() {
    let b = board();
    let g = b.goal();
    let id = b.planned(g);
    let from_app: Query = [(api::FROM.to_string(), "app".to_string())].into_iter().collect();
    let v = api::dispatch(&b.app, "POST", &format!("/goals/G{g}/run"), &from_app, &json!({})).unwrap();
    assert_eq!(v["queued_now"], 1);
    assert_eq!(b.status(id), "queued");
}

#[test]
fn a_run_with_no_terminal_is_only_the_apps() {
    let b = board();
    let g = b.goal();
    let id = b.planned(g);
    // `tb start G1` or `tb goal set G1 --run` with MIDNA_SESSION unset, or a plain curl.
    for body in [json!({}), json!({"via_session": ""}), json!({"now": true})] {
        let e = api::dispatch(&b.app, "POST", &format!("/goals/G{g}/run"), &Query::new(), &body).unwrap_err();
        assert_eq!(e.status, 403, "{body}");
        assert!(e.message.contains(&format!("Only a human can run G{g}")), "{}", e.message);
    }
    assert_eq!(b.status(id), "planned");
}

impl Board {
    /// What the Stop hook sends at the end of the agent's turn: its message's start and its end.
    fn replied(&self, message: &str) {
        let start: String = message.chars().take(2000).collect();
        let end: String = message.chars().rev().take(startword::REPLY_END_KEEP).collect::<Vec<_>>().into_iter().rev().collect();
        self.report("hook.stop", json!({"last_message": start, "last_message_end": end}));
    }
    /// What `tb goal new … --task` reports: a goal and its planned tasks.
    fn goal_new(&self) -> i64 {
        let v = self.report("tb.goal", json!({"name": "Dark mode", "project": "webapp", "tasks": ["Colors::pick them", "Toggle::add it"]}));
        v["goal"].as_str().unwrap().trim_start_matches('G').parse().unwrap()
    }
}

/// Every phrase in #126, both ways, through the hook's path: each case is a fresh conversation, the
/// steps in order ("say:" a prompt the owner types, "agent:" the message the agent ends its turn on,
/// "goal new" / "propose" / "task new" what the agent's `tb` reports, "plan" the board opening this
/// terminal to edit the goal's plan), then `tb start G<n>` on the goal ("G" in the text is its ref).
#[test]
fn the_goal_runs_on_the_owners_word_in_the_conversation_that_made_it() {
    let runs: Vec<Vec<&str>> = vec![
        vec!["say:make a goal for dark mode with two tasks", "goal new", "say:looks good, run the goal"],
        vec!["say:plan the dark mode goal", "propose", "say:great, start the goal"],
        vec!["plan", "say:run the goal"],
        vec!["plan", "say:ok, add a task for the toggle", "task new", "say:looks right. run the goal"],
        vec!["say:make a goal for dark mode and run it", "goal new"],
        vec!["say:make a goal for dark mode", "goal new", "agent:Made G with two tasks. Want me to run it?", "say:yes"],
        vec!["say:make a goal for dark mode", "goal new", "agent:Made G with two tasks. Want me to run G?", "say:yep, go ahead"],
        vec!["say:make a goal for dark mode", "goal new", "agent:Done. Should I start the goal now?", "say:sure"],
        vec!["say:plan the dark mode goal", "propose", "agent:Proposed two tasks for G.\n\nShall I kick it off?", "say:run it"],
        vec!["say:make a goal for dark mode", "goal new", "say:go ahead and run all the tasks"],
        vec!["say:make a goal for dark mode", "goal new", "say:thanks", "say:run it"],
        // Still working.
        vec!["say:add a task to the goal", "task new", "say:start the goal"],
        // #144: "it" is the conversation's one goal.
        vec!["plan", "say:run it"],
        vec!["plan", "say:ok start it"],
        vec!["plan", "say:looks good, kick it off"],
        vec!["plan", "agent:The plan is ready. Want me to run it?", "say:yes"],
        vec!["say:make a goal for dark mode", "goal new", "agent:Done, two tasks. Want me to run it?", "say:yes"],
        vec!["say:make a goal for dark mode", "goal new", "agent:The plan is ready. Want me to run it?", "say:yes"],
        vec!["plan", "say:rename the second task", "say:ok run it"],
        vec!["say:make a goal for dark mode", "goal new", "say:rename the second task", "say:ok run it"],
        vec!["say:make a goal for dark mode", "goal new", "say:also fix the header", "say:run it"],
        // #148: right after `tb goal new`, other asks, and a question that isn't the last sentence.
        vec!["say:make a goal for dark mode", "goal new", "say:let's run it"],
        vec!["say:make a goal for dark mode", "goal new", "say:can you run it?"],
        vec!["say:make a goal for dark mode", "goal new", "say:ship it"],
        vec!["say:make a goal for dark mode", "goal new", "say:start all of them"],
        vec!["say:make a goal for dark mode", "goal new", "agent:Want me to run it? It has 2 tasks.", "say:yes"],
        vec!["plan", "agent:Want me to run it? It has 2 tasks.", "say:yes"],
        vec!["plan", "agent:Updated the plan. Want me to run it?", "say:yes"],
    ];
    let refused: Vec<Vec<&str>> = vec![
        // #157: a goal that isn't the conversation's doesn't run from its terminal, even by name.
        vec!["say:run G"],
        vec!["say:run the goal"],
        vec!["say:run it"],
        vec!["say:yes"],
        vec!["say:make a goal for dark mode", "goal new", "say:run it tomorrow"],
        vec!["say:make a goal for dark mode", "goal new", "say:run the tests"],
        vec!["say:make a goal for dark mode and run it", "goal new", "say:wait"],
        // "It" when the conversation has two goals of its own, or "run it" with more said around it.
        vec!["plan other", "say:make a goal for dark mode", "goal new", "say:rename the second task", "say:ok run it"],
        vec!["say:make a goal for dark mode", "goal new", "plan other", "agent:The plan is ready. Want me to run it?", "say:yes"],
        vec!["plan", "say:the tests fail, run it"],
        vec!["plan", "say:run it tomorrow"],
        vec!["say:make a goal for dark mode", "goal new", "say:looks good, run the goal", "say:actually, don't"],
        // "Yes" to no question, to another question, or to a question that's no longer the latest.
        vec!["say:make a goal for dark mode", "goal new", "agent:Made G with two tasks.", "say:yes"],
        vec!["say:make a goal for dark mode", "goal new", "agent:Want me to run G, or tweak the plan first?", "say:yes"],
        vec!["say:make a goal for dark mode", "goal new", "agent:Want me to run G tomorrow?", "say:yes"],
        vec!["say:make a goal for dark mode", "goal new", "agent:Want me to run G?", "say:no"],
        vec!["say:make a goal for dark mode", "goal new", "agent:Want me to run G?", "say:thanks", "say:yes"],
        vec!["say:make a goal for dark mode", "goal new", "agent:Should I open a PR?", "say:yes"],
        vec!["say:make a goal for dark mode", "goal new", "agent:I added T1 to G. Want me to start it?", "say:yes"],
        vec!["say:make a goal for dark mode", "goal new", "agent:Want me to run G?", "say:yes, but not now"],
        // A plan terminal for another goal.
        vec!["plan other", "say:run the goal"],
        // #148: "it" after a nearer noun is that noun, not the goal.
        vec!["plan", "agent:Wrote scripts/seed.sh. Want me to run it?", "say:yes"],
        vec!["plan", "say:write a script that seeds the db", "agent:Done.", "say:run it"],
        vec!["plan", "agent:I can run the linter on the plan files. Want me to run it?", "say:yes"],
        vec!["plan", "agent:I wrote a quick benchmark. Should I run it?", "say:sure"],
        vec!["plan", "agent:The docker container stopped. Should I start it?", "say:yes"],
        vec!["say:make a goal for dark mode", "goal new", "agent:Wrote scripts/seed.sh. Want me to run it?", "say:yes"],
        vec!["say:make a goal for dark mode", "goal new", "say:write a script that seeds the db", "agent:Done.", "say:run it"],
        vec!["say:make a goal for dark mode", "goal new", "agent:I wrote a quick benchmark. Should I run it?", "say:sure"],
        vec!["say:make a goal for dark mode", "goal new", "agent:The docker container stopped. Should I start it?", "say:yes"],
        vec!["plan", "say:write a script that seeds the db", "agent:Done. Want me to run it?", "say:yes"],
    ];
    let wrong = goal_cases_wrong(&runs, &refused);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Every phrase in #152, both ways, as #126's are read: "it" is the goal only when what was spoken of
/// last is the goal or its plan, or nothing but the owner's ask for the goal.
#[test]
fn it_is_the_goal_only_when_the_goal_was_spoken_of_last() {
    let mut runs: Vec<Vec<&str>> = vec![];
    let mut refused: Vec<Vec<&str>> = vec![];
    // A plan summary with other nouns in it is still about the goal.
    for (reply, yes) in [
        ("agent:Done — two tasks, each with tests. Want me to run it?", "say:yes"),
        ("agent:The goal has two tasks: add the migration and update the API. Want me to run it?", "say:yes"),
        ("agent:Each task opens its own PR. Want me to run it?", "say:sure"),
        ("agent:Two tasks: Colors (touches the theme files) and Toggle. Should I start it?", "say:yes"),
        ("agent:The plan has a build step and a deploy step. Want me to run it?", "say:yes"),
        ("agent:Made two tasks:\n- Colors: update the theme files\n- Toggle: add the switch\n\nWant me to run it?", "say:yes"),
    ] {
        runs.push(vec!["plan", reply, yes]);
        runs.push(vec!["say:make a goal for dark mode", "goal new", reply, yes]);
    }
    // The owner's prompt asked for the goal, though it names tests.
    runs.push(vec!["say:make a goal for adding tests to login", "goal new", "agent:Done. Want me to run it?", "say:yes"]);
    // Something else the agent spoke of last.
    for (reply, yes) in [
        ("agent:I set up the dev database. Should I start it?", "say:yes"),
        ("agent:I wrote a rake task to backfill the colors. Want me to run it?", "say:yes"),
        ("agent:The emulator is ready. Want me to run it?", "say:yes"),
        ("agent:I built the app. Want me to run it?", "say:yes"),
        ("agent:I wrote a small CLI to check the colors. Should I run it?", "say:yes"),
        ("agent:The codemod is ready. Want me to run it?", "say:yes"),
    ] {
        refused.push(vec!["plan", reply, yes]);
        refused.push(vec!["say:make a goal for dark mode", "goal new", reply, yes]);
    }
    // Something else the owner asked the agent to make, when the agent's message speaks of nothing.
    for (ask, reply) in [("say:set up the emulator", "agent:Done."), ("say:write a backfill", "agent:Done, it's ready."), ("say:add a storybook story for the toggle", "agent:Added.")] {
        refused.push(vec!["plan", ask, reply, "say:run it"]);
        refused.push(vec!["say:make a goal for dark mode", "goal new", ask, reply, "say:run it"]);
    }
    let wrong = goal_cases_wrong(&runs, &refused);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Each case is a fresh conversation with a planned goal G, the steps in order ("say:" a prompt the
/// owner types, "agent:" the message the agent ends its turn on, "goal new" / "propose" / "task new" what
/// the agent's `tb` reports, "plan" the board opening this terminal to edit the goal's plan), then `tb
/// start G<n>` on the goal: the cases the board gets wrong.
fn goal_cases_wrong(runs: &[Vec<&str>], refused: &[Vec<&str>]) -> Vec<String> {
    let b = board();
    let mut wrong = vec![];
    for (want, cases) in [(true, runs), (false, refused)] {
        for steps in cases {
            b.report("hook.session_start", json!({"source": "clear"}));
            let mut g = b.goal();
            b.planned(g);
            for step in steps {
                let text = |t: &str| goal_refs(t, g);
                match *step {
                    "goal new" => {
                        g = b.goal_new();
                    }
                    "propose" => {
                        b.report("tb.propose", json!({"goal": format!("G{g}"), "tasks": ["Colors::pick them"]}));
                    }
                    "task new" => {
                        b.report("tb.new_task", json!({"title": "Toggle", "goal": format!("G{g}")}));
                    }
                    "plan" | "plan other" => {
                        let other = if *step == "plan" { g } else { b.goal() };
                        api::dispatch(&b.app, "POST", &format!("/goals/G{other}/plan"), &Query::new(), &json!({"mode": "edit"})).unwrap();
                        // What Midna's job runner records once the terminal opens.
                        b.app.db.x("UPDATE jobs SET target = json_set(target, '$.session', 's1') WHERE purpose = 'plan'", p![]).unwrap();
                    }
                    s if s.starts_with("say:") => b.typed(&text(&s[4..])),
                    s if s.starts_with("agent:") => b.replied(&text(&s[6..])),
                    s => panic!("{s}"),
                }
            }
            let got = b.tb_run(g);
            if got.is_ok() != want {
                wrong.push(format!("{} G{g}: {steps:?} {:?}", if want { "refused" } else { "ran" }, got.err()));
            }
            b.app.db.x("DELETE FROM jobs WHERE purpose = 'plan'", p![]).unwrap();
        }
    }
    wrong
}

/// `t` with each "G" that stands alone (a goal's ref) made `G<g>`.
fn goal_refs(t: &str, g: i64) -> String {
    regex::Regex::new(r"\bG\b").unwrap().replace_all(t, format!("G{g}").as_str()).to_string()
}

#[test]
fn a_yes_needs_the_end_of_the_agents_message() {
    let b = board();
    b.said("make a goal for dark mode");
    let g = b.goal_new();
    // The hook before this sent only the first 2000 characters: the question at the end never came.
    let long = format!("{} Want me to run G{g}?", "Here's the plan in detail. ".repeat(100));
    let start: String = long.chars().take(2000).collect();
    b.report("hook.stop", json!({"last_message": start}));
    b.typed("yes");
    assert_eq!(b.tb_run(g).unwrap_err().0, 403);
    // Today's hook sends its end too.
    b.replied(&long);
    b.typed("yes");
    assert!(b.tb_run(g).is_ok());
}

/// A log the owner pasted, `n` characters or more of it.
fn log_paste(n: usize) -> String {
    let line = "2026-10-09 12:01:02 INFO worker 3 finished the batch in 41ms\n";
    line.repeat(n / line.len() + 1)
}

impl Board {
    /// What the UserPromptSubmit hook sends for a typed prompt: the same cut, then the report.
    fn typed(&self, prompt: &str) {
        self.report("hook.prompt", startword::hook_prompt(prompt));
    }
}

/// Every phrase in #123, both ways, through the hook's path: each case is a fresh conversation with T8
/// and T9 on the board, the prompts typed in order ("+" is the agent making a task with `tb task new`),
/// then `tb start` on one task ("T8", "T9", or "M1"/"M2", the first or second task the agent made).
#[test]
fn the_start_word_holds_only_the_start_its_no_time_or_condition_is_about() {
    let paste = log_paste(25_000);
    let log = log_paste(9000);
    let typed = "The footer overlaps the cookie banner on small screens and the links wrap badly. ".repeat(60);
    let starts: Vec<(Vec<String>, &str)> = vec![
        // A long prompt: its start and its end are read.
        (vec![format!("start T8. here's the log: {log}")], "T8"),
        (vec![format!("start T8. here's the log:\n{log}")], "T8"),
        (vec![format!("start T8. here's the log:\n{paste}\nthat's all")], "T8"),
        (vec!["start T8".into(), paste.clone(), "ok go ahead".into()], "T8"),
        (vec!["start T8".into(), format!("here's the log:\n{paste}")], "T8"),
        (vec![format!("here's the log:\n{paste}\nok, start T8")], "T8"),
        (vec![format!("start T8. {typed}…")], "T8"),
        (vec!["start T8. no need to ask".into()], "T8"),
        // A time or condition about something else.
        (vec!["start T8 and tell me when it's done".into()], "T8"),
        (vec!["start T8. let me know if it fails".into()], "T8"),
        (vec!["start T8, it only takes a few minutes".into()], "T8"),
        (vec!["start T8. it only takes a few minutes".into()], "T8"),
        (vec!["start T8. I'll review later".into()], "T8"),
        (vec!["start T8, then wait for review".into()], "T8"),
        (vec!["start T8 then wait for review".into()], "T8"),
        (vec!["start T8, the Monday report fix".into()], "T8"),
        (vec!["start T8 (the Monday report fix)".into()], "T8"),
        (vec!["start T8. once it's done, start T9".into()], "T8"),
        (vec!["make a task for the footer and queue it and let me know when it's done".into(), "+".into()], "M1"),
        (vec!["make a task for the footer".into(), "+".into(), "queue it and let me know when it's done".into()], "M1"),
        // A no about something else.
        (vec!["start T8, no rush".into()], "T8"),
        (vec!["start T8 and don't ask me again".into()], "T8"),
        (vec!["start T8 but don't merge it".into()], "T8"),
        (vec!["start T8. don't forget the tests".into()], "T8"),
        (vec!["start T8 without the migration".into()], "T8"),
        (vec!["start T8 now, not later".into()], "T8"),
        (vec!["no, start T8".into()], "T8"),
        (vec!["nope, start T8".into()], "T8"),
        // Phrasing.
        (vec!["start both T8 and T9".into()], "T8"),
        (vec!["start both T8 and T9".into()], "T9"),
        (vec!["start them: T8 and T9".into()], "T8"),
        (vec!["start them: T8 and T9".into()], "T9"),
        (vec!["start T8 — it's ready".into()], "T8"),
        (vec!["start T8 (it's ready)".into()], "T8"),
        (vec!["start T8…".into()], "T8"),
        (vec!["start T8...".into()], "T8"),
        (vec!["T8 is ready, start it".into()], "T8"),
        (vec!["T8 is ready. Start it.".into()], "T8"),
        (vec!["make a task for the footer".into(), "+".into(), "start the new task".into()], "M1"),
        (vec!["make a task for the footer and start the new task".into(), "+".into()], "M1"),
        (vec!["make two tasks for the footer".into(), "+".into(), "+".into(), "queue the first one".into()], "M1"),
        (vec!["make two tasks for the footer and queue the first one".into(), "+".into(), "+".into()], "M1"),
        (vec!["run T8".into()], "T8"),
        (vec!["pick up T8".into()], "T8"),
    ];
    let holds: Vec<(Vec<String>, &str)> = vec![
        (vec!["start T8 tomorrow".into()], "T8"),
        (vec!["start T8 once T7 lands".into()], "T8"),
        (vec!["don't start T8".into()], "T8"),
        (vec!["start T8. jk".into()], "T8"),
        (vec!["wait until 6am, then start T8".into()], "T8"),
        (vec!["start T8. Not now though.".into()], "T8"),
        (vec!["start T8. once it's done, start T9".into()], "T9"),
        (vec!["start T8. Do it tomorrow.".into()], "T8"),
        (vec![format!("start T8. here's the log:\n{paste}\njk")], "T8"),
        (vec!["start T8".into(), format!("here's the log:\n{paste}\nnever mind")], "T8"),
        (vec![format!("here's the log:\n{paste}start T8\n")], "T8"),
        (vec!["make two tasks for the footer".into(), "+".into(), "+".into(), "queue the first one".into()], "M2"),
        (vec!["T8 is ready, start it".into()], "T9"),
        (vec!["run T8's tests".into()], "T8"),
        // The lower-priority ones.
        (vec!["start T8 then".into()], "T8"),
        (vec!["ok, start T8 then.".into()], "T8"),
        (vec!["start T8 down the road".into()], "T8"),
        (vec!["start T8, down the road".into()], "T8"),
        (vec!["start T8 at your convenience".into()], "T8"),
        (vec!["start T8, at your convenience".into()], "T8"),
        (vec!["start T8 at six am".into()], "T8"),
        (vec!["start T8, six am".into()], "T8"),
        (vec!["start T8 on the 15th".into()], "T8"),
        (vec!["start T8, on the 15th".into()], "T8"),
        (vec!["start T8... psych".into()], "T8"),
        (vec!["I forbid you: start T8".into()], "T8"),
        (vec!["here's the output.\nstart T8\nexit 1".into()], "T8"),
    ];
    // #128: the real asks beta.16 still refused, and a hold on another task.
    let mut starts = starts;
    for ask in [
        "go ahead with T8",
        "go for T8",
        "do T8",
        "fire off T8",
        "put T8 in the queue",
        "start T8 now rather than later",
        "start T8, sorry for the wait",
        "ok - start T8",
        "start T8, hold off on T9",
        "start T8 now rather than tomorrow",
        "start T8. T9 can wait",
        "start T8. hold off on T9",
        "start T8 now and T9 tomorrow",
        "start T8 and T9, but T9 not until T8 merges",
    ] {
        starts.push((vec![ask.into()], "T8"));
    }
    for said in [
        vec!["make a task for the footer", "+", "thanks", "queue it"],
        vec!["make a task for the footer", "+", "looks right", "ok, queue it"],
        vec!["put the footer fix on the board", "+", "queue it"],
        vec!["track this as a task", "+", "queue it"],
    ] {
        starts.push((said.into_iter().map(String::from).collect(), "M1"));
    }
    // A typed prompt of about 8000 characters that ends in "…" is whole: the hook marks its own cut.
    let near: String = format!("start T8. {typed}{typed}").chars().take(7993).collect::<String>() + "…";
    assert_eq!(near.chars().count(), 7994);
    starts.push((vec![near], "T8"));
    let mut holds = holds;
    for said in [
        // A time or a condition after the ask.
        "start T8, over lunch",
        "start T8 over lunch",
        "start T8, but not until T9 is merged",
        "start T8 but not until T9 is merged",
        "start T8 in a while",
        "start T8, in a while",
        "start T8 at the top of the hour",
        "start T8, at the top of the hour",
        "start T8 ASAP once T9 merges",
        "start T8, ASAP once T9 merges",
        "start T8 as soon as T9 is in",
        "start T8, as soon as T9 is in",
        "start T8 during work hours",
        "start T8, during work hours",
        "start T8 in work hours",
        "start T8, in work hours",
        "start T8 during business hours",
        "start T8, during business hours",
        "start T8, once you've had lunch",
        "start T8, after I merge T9",
        "start T8, when I'm back",
        "start T8 but wait for T9",
        "start T8 over the weekend",
        "start T8 around 3",
        "start T8 after hours",
        "start T8 in a bit",
        // Quoted or pasted text.
        "the ticket reads — start T8 — weird",
        "copied from CI - start T8",
        "CI output - start T8",
        // A no.
        "I don't want you to, start T8",
    ] {
        holds.push((vec![said.into()], "T8"));
    }
    for (said, t) in [
        ("start T8. T9 can wait", "T9"),
        ("start T8. hold off on T9", "T9"),
        ("start T8 now and T9 tomorrow", "T9"),
        ("start T8 and T9, but T9 not until T8 merges", "T9"),
    ] {
        holds.push((vec![said.into()], t));
    }
    // A take-back on the last line of a paste.
    for n in [500, 2000, 9000, 25_000] {
        let log = log_paste(n);
        for back in ["jk", "never mind"] {
            holds.push((vec![format!("start T8. here's the log:\n{}\n{back}", log.trim_end())], "T8"));
        }
    }
    let b = board();
    let mut wrong = vec![];
    for (want, cases) in [(true, &starts), (false, &holds)] {
        for (said, target) in cases {
            b.report("hook.session_start", json!({"source": "clear"}));
            let (t8, t9) = (b.new_task(), b.new_task());
            let mut made = vec![];
            for p in said {
                if p == "+" {
                    made.push(b.new_task());
                } else {
                    b.typed(&p.replace("T8", "\u{1}").replace("T9", "\u{2}").replace('\u{1}', &format!("T{t8}")).replace('\u{2}', &format!("T{t9}")));
                }
            }
            let id = match *target {
                "T8" => t8,
                "T9" => t9,
                m => made[m[1..].parse::<usize>().unwrap() - 1],
            };
            let got = b.tb_start(id);
            if got.is_ok() != want {
                let shown: Vec<String> = said.iter().map(|p| p.chars().take(80).collect()).collect();
                wrong.push(format!("{} {target}: {shown:?} {:?}", if want { "refused" } else { "started" }, got.err()));            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Every phrase in #143, both ways, through the hook's path: each case is a fresh conversation with T8
/// and T9 on the board, the steps in order ("agent:" the message the agent ends its turn on, anything
/// else a prompt the owner types), then `tb start` on one task.
#[test]
fn the_start_word_reads_the_asks_143_found_refused() {
    let log = log_paste(2000);
    let pasted = |last: &str| format!("start T8. here's the log:\n{}\n{last}", log.trim_end());
    let mut starts: Vec<(Vec<String>, &str)> = vec![];
    for ask in [
        // A dash after the owner's own lead-in.
        "tests pass - start T8",
        "hey - start T8",
        "T9 is merged — start T8",
        "T9 merged - start T8",
        "T9's done - kick off T8",
        "quick one — kick off T8",
        "Good morning — start T8",
        "no — start T8 now",
        "morning - start T8",
        "not T9 — start T8",
        "ok - start T8 - it's ready",
        // A no on what came before, and "during" with no time after it.
        "don't do that, start T8 instead",
        "start T8, during the run keep an eye on CI",
        // Polite asks.
        "Can you start T8?",
        "could you kick off T8 when you get a chance",
        "Morning! start T8 when you're ready",
        // Phrasing.
        "fire up T8",
        "spin up T8",
        "T8 go",
        "T8: go",
        "go T8",
        "queue T8 + T9",
        "start T8 thx",
        "strat T8",
        "satrt T8",
    ] {
        starts.push((vec![ask.into()], "T8"));
    }
    starts.push((vec!["queue T8 + T9".into()], "T9"));
    // A line after a paste that goes on into other words is the paste's.
    for last in ["Wait timeout exceeded", "Hold on, retrying in 5s", "never mind the warnings above"] {
        starts.push((vec![pasted(last)], "T8"));
    }
    // A yes to the agent's own question.
    for (question, yes) in [("Should I start T8?", "yes"), ("Should I start T8?", "yep go ahead"), ("Want me to kick off T8 now?", "sure"), ("T8 is ready. Want me to start it?", "yes please")] {
        starts.push((vec![format!("agent:{question}"), yes.into()], "T8"));
    }
    let mut holds: Vec<(Vec<String>, &str)> = vec![];
    for said in [
        "copied from CI - start T8",
        "CI output - start T8",
        "the ticket reads — start T8 — weird",
        "the log says - start T8",
        "start T8, during work hours",
        "start T8 during the standup",
        "start T8 during lunch",
        "I don't want you to, start T8",
        "don't, start T8",
        "should I start T8?",
        // "Will you start T8?" asks for it (#151); "when will you" asks about it.
        "when will you start T8?",
        "can you start T8 tomorrow?",
        "could you start T8 once T9 lands?",
        "does T8 go first?",
        "start T8 when T9 lands",
        "not T8 — start T9",
    ] {
        holds.push((vec![said.into()], "T8"));
    }
    for back in ["jk", "never mind", "nvm", "wait", "hold on", "actually no"] {
        holds.push((vec![pasted(back)], "T8"));
    }
    for said in [
        vec!["yes"],
        vec!["agent:Should I start T8?", "yes, but not now"],
        vec!["agent:Should I start T8?", "no"],
        vec!["agent:Should I start T8 tomorrow?", "yes"],
        vec!["agent:Should I start T9?", "yes"],
        vec!["agent:Should I start T8 or T9?", "yes"],
        vec!["agent:Should I start T8?", "thanks", "yes"],
        vec!["agent:I made T8 and T9. Want me to start it?", "yes"],
        vec!["agent:Should I open a PR for T8?", "yes"],
    ] {
        holds.push((said.into_iter().map(String::from).collect(), "T8"));
    }
    let b = board();
    let mut wrong = vec![];
    for (want, cases) in [(true, &starts), (false, &holds)] {
        for (said, target) in cases {
            b.report("hook.session_start", json!({"source": "clear"}));
            let (t8, t9) = (b.new_task(), b.new_task());
            let refs = |p: &str| p.replace("T8", "\u{1}").replace("T9", "\u{2}").replace('\u{1}', &format!("T{t8}")).replace('\u{2}', &format!("T{t9}"));
            for p in said {
                match p.strip_prefix("agent:") {
                    Some(m) => b.replied(&refs(m)),
                    None => b.typed(&refs(p)),
                }
            }
            let got = b.tb_start(if *target == "T8" { t8 } else { t9 });
            if got.is_ok() != want {
                let shown: Vec<String> = said.iter().map(|p| p.chars().take(80).collect()).collect();
                wrong.push(format!("{} {target}: {shown:?} {:?}", if want { "refused" } else { "started" }, got.err()));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `tb goal set G1 … --run` asks the board first (`"check": true`): a refused run is refused before the
/// changes, and a check runs nothing.
#[test]
fn a_run_check_runs_nothing_and_says_whether_it_would() {
    let b = board();
    let g = b.goal();
    let id = b.planned(g);
    let check = |body: Value| api::dispatch(&b.app, "POST", &format!("/goals/G{g}/run"), &Query::new(), &body).map_err(|e| (e.status, e.message));
    let (code, why) = check(json!({"via_session": "s1", "check": true})).unwrap_err();
    assert_eq!(code, 403);
    assert!(why.contains(&format!("Only a human can run G{g}")), "{why}");
    let (code, why) = check(json!({"check": true})).unwrap_err();
    assert_eq!(code, 403);
    assert!(why.contains("the only way to run it from outside a Midna terminal"), "{why}");
    // #157: the goal must be the conversation's; the board hands it this one.
    b.typed(&format!("[task-board:G{g}] Plan the goal."));
    b.typed(&format!("run G{g}"));
    let v = check(json!({"via_session": "s1", "check": true})).unwrap();
    assert_eq!(v["would_run"], true);
    assert_eq!(b.status(id), "planned", "a check runs nothing");
    assert!(b.tb_run(g).is_ok());
    assert_eq!(b.status(id), "queued");
}

/// Each case is a fresh conversation with T8, T9, T10 and T11 on the board, the steps in order ("agent:"
/// the message the agent ends its turn on, anything else a prompt the owner types), then `tb start` on
/// the task named: the cases the board gets wrong.
fn start_cases_wrong(starts: &[(Vec<String>, &str)], holds: &[(Vec<String>, &str)]) -> Vec<String> {
    let b = board();
    let mut wrong = vec![];
    let named = regex::Regex::new(r"\bT(8|9|10|11)\b").unwrap();
    for (want, cases) in [(true, starts), (false, holds)] {
        for (said, target) in cases {
            b.report("hook.session_start", json!({"source": "clear"}));
            let ids: Vec<i64> = (0..4).map(|_| b.new_task()).collect();
            let id = |n: &str| ids[n.parse::<usize>().unwrap() - 8];
            let refs = |p: &str| named.replace_all(p, |c: &regex::Captures| format!("T{}", id(&c[1]))).to_string();
            for p in said {
                match p.strip_prefix("agent:") {
                    Some(m) => b.replied(&refs(m)),
                    None => b.typed(&refs(p)),
                }
            }
            let got = b.tb_start(id(&target[1..]));
            if got.is_ok() != want {
                let shown: Vec<String> = said.iter().map(|p| p.chars().take(80).collect()).collect();
                wrong.push(format!("{} {target}: {shown:?} {:?}", if want { "refused" } else { "started" }, got.err()));
            }
        }
    }
    wrong
}

/// Every phrase in #147, both ways, through the hook's path.
#[test]
fn the_start_word_reads_the_asks_147_found() {
    let log = log_paste(2000);
    let pasted = |last: &str| format!("start T8. here's the log:\n{}\n{last}", log.trim_end());
    let convo = |steps: &[&str]| steps.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    let mut starts: Vec<(Vec<String>, &str)> = vec![];
    // 1. A yes with more words, and the short ones.
    for yes in [
        "yes, start it",
        "yep, kick it off",
        "sure, queue it",
        "yes, please start it",
        "start it",
        "kick it off",
        "Yes. Also rename T9 to Footer.",
        "yes - and keep the PR small",
        "y",
        "👍",
        "go",
        "please",
        "sure thing",
    ] {
        starts.push((convo(&["agent:Should I start T8?", yes]), "T8"));
    }
    // 2. A question naming two tasks, or "them".
    for question in ["Should I start T8 and T9?", "Want me to start T8 and T9?", "I made T8 and T9. Want me to start them?"] {
        for target in ["T8", "T9"] {
            starts.push((convo(&[&format!("agent:{question}"), "yes"]), target));
        }
    }
    starts.push((convo(&["agent:I made T8 and T9. Want me to start them?", "ok, start them"]), "T9"));
    // 3. A question that isn't the last sentence.
    for question in [
        "Want me to start T8? It'll take a while.",
        "Should I start T8? Let me know.",
        "Should I start T8? (It only touches the footer.)",
        "Shall I start T8?\n\nIt only touches the footer.",
        "I can start T8 next. Want me to?",
    ] {
        starts.push((convo(&[&format!("agent:{question}"), "yes"]), "T8"));
    }
    // 4. Other asks.
    for ask in ["start T8 when you're free", "would you mind starting T8?", "can u start T8", "CI's green - start T8", "start T8 when you can"] {
        starts.push((convo(&[ask]), "T8"));
    }
    let mut holds: Vec<(Vec<String>, &str)> = vec![];
    // 5. A take-back after a paste.
    for back in [
        "lol jk",
        "haha jk",
        "lol nvm",
        "hmm actually no",
        "never mind, don't start it",
        "wait, don't start it yet",
        "hold off on that",
        "scratch that lol",
        "ugh wait",
        "actually let's wait on that",
        "wait no, I need to check something first",
        "jk lol",
        "nvm haha",
    ] {
        holds.push((vec![pasted(back)], "T8"));
    }
    // ...while a log line that only starts like one is the paste's.
    for last in ["Wait timeout exceeded", "Hold on, retrying in 5s", "never mind the warnings above"] {
        starts.push((vec![pasted(last)], "T8"));
    }
    // 6. A yes to a question about something of a task's.
    for question in ["Want me to run T8's tests?", "Should I pick up T8's comments?", "Should I go for T8's approach?"] {
        holds.push((convo(&[&format!("agent:{question}"), "yes"]), "T8"));
    }
    // 7. "When you can …" with a condition after it.
    for said in [
        "start T8 when you can confirm T9 passes",
        "start T8 when you can reproduce the bug",
        "start T8 when you get a chance to check T9",
        "start T8 when you're ready to merge T9",
    ] {
        holds.push((convo(&[said]), "T8"));
    }
    // 8. An "or" question.
    holds.push((convo(&["would you start T8 or T9 first?"]), "T8"));
    holds.push((convo(&["would you start T8 or T9 first?"]), "T9"));
    // Kept refused: a yes with a no, a time or another ask in it, and "it" with two tasks asked about.
    for said in [
        vec!["agent:Should I start T8?", "yes, but not now"],
        vec!["agent:Should I start T8?", "yes, tomorrow"],
        vec!["agent:Should I start T8?", "yes. Do it after the deploy."],
        vec!["agent:Should I start T8?", "go fix the header first"],
        vec!["agent:Should I start T8?", "yes, but first fix the header"],
        vec!["agent:Should I start T8?", "yes, start T9"],
        vec!["agent:Should I start T8 and T9?", "yes, start it"],
        vec!["agent:Should I start T8? Or should I wait for T9?", "yes"],
        vec!["agent:Should I start T8? I could also do it tomorrow.", "yes"],
        vec!["agent:Start T8?\n\nDone.", "yes"],
        vec!["agent:I can't start T8 yet. Want me to?", "yes"],
        vec!["agent:Should I start T8?", "thanks"],
    ] {
        holds.push((convo(&said), "T8"));
    }
    let wrong = start_cases_wrong(&starts, &holds);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Every phrase in #151, both ways, through the hook's path.
#[test]
fn the_start_word_reads_the_asks_151_found() {
    let log = log_paste(2000);
    let pasted = |last: &str| format!("start T8. here's the log:\n{}\n{last}", log.trim_end());
    let convo = |steps: &[&str]| steps.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    let mut starts: Vec<(Vec<String>, &str)> = vec![];
    let mut holds: Vec<(Vec<String>, &str)> = vec![];
    // 1. A log line that only starts like a take-back.
    starts.push((vec![pasted("Wait - no response from the upstream host")], "T8"));
    // 2. A blocker after the question.
    holds.push((convo(&["agent:Should I start T8? T9 needs to merge first though.", "yes"]), "T8"));
    // 3. A quote after "was".
    for said in ["the comment was - start T8", "the ticket's comment was - start T8"] {
        holds.push((convo(&[said]), "T8"));
    }
    // 4. "Them" is the tasks of the sentence the question follows.
    for question in ["T9 merged this morning. I made T10 and T11. Want me to start them?", "I made T10 and T11 (T9 covers the rest). Want me to start them?"] {
        holds.push((convo(&[&format!("agent:{question}"), "yes"]), "T9"));
        for target in ["T10", "T11"] {
            starts.push((convo(&[&format!("agent:{question}"), "yes"]), target));
        }
    }
    // 5. A yes with an aside on something else.
    for yes in ["yes, no rush", "sure, no need to hurry", "yes. Don't touch the footer though.", "yes, and don't forget the migration", "yes, and ping me when the PR is up"] {
        starts.push((convo(&["agent:Should I start T8?", yes]), "T8"));
    }
    // ...but a no on the start itself still holds it.
    for no in ["yes, but don't start yet", "yes, don't start it yet", "yes, not now"] {
        holds.push((convo(&["agent:Should I start T8?", no]), "T8"));
    }
    // 6. "k", "Will you", "just T8", and "it" after several tasks: the nearest.
    starts.push((convo(&["agent:Should I start T8?", "k"]), "T8"));
    starts.push((convo(&["Will you start T8?"]), "T8"));
    starts.push((convo(&["agent:Should I start T8 and T9?", "just T8"]), "T8"));
    holds.push((convo(&["agent:Should I start T8 and T9?", "just T8"]), "T9"));
    let follow_up = "agent:T8 is done. I made T10 for the follow-up. Want me to start it?";
    starts.push((convo(&[follow_up, "yes"]), "T10"));
    holds.push((convo(&[follow_up, "yes"]), "T8"));
    // 7. A "when" after the question on what the agent does after the start.
    starts.push((convo(&["agent:Want me to start T8? I'll open a draft PR when it's done.", "yes"]), "T8"));
    holds.push((convo(&["agent:Want me to start T8? I could also do it when T9 lands.", "yes"]), "T8"));
    // 8. A yes to a question about something of a task's, for every start word.
    for question in ["Should I start T8's review?", "Want me to start T8's preview env?", "Should I kick off T8's build?", "Should I queue T8's tests?"] {
        holds.push((convo(&[&format!("agent:{question}"), "yes"]), "T8"));
    }
    // 9. A take-back after a paste.
    for back in ["oops, not that one", "lmao no"] {
        holds.push((vec![pasted(back)], "T8"));
    }
    let wrong = start_cases_wrong(&starts, &holds);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Every phrase in #154, both ways, through the hook's path: "it" carries the ref a describing sentence
/// follows, plain words after the question leave it, and the tasks the agent made beat one it mentions.
#[test]
fn the_start_word_reads_the_asks_154_found() {
    let convo = |steps: &[&str]| steps.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    let mut starts: Vec<(Vec<String>, &str)> = vec![];
    let mut holds: Vec<(Vec<String>, &str)> = vec![];
    // 1. A sentence describing the task between its ref and "it".
    for question in [
        "T8 is ready. It only touches the header. Should I start it?",
        "I looked at T8. The fix is small. Want me to start it?",
        "T8 is ready. It touches one file. Should I start it?",
        "T8 is ready. Its PR will be small. Should I start it?",
        "T8 is ready. My plan is to keep it to the footer. Should I start it?",
        "T8 is ready. A quick look says it's one file. Should I start it?",
        "T8 is unblocked now. The migration it needed merged. Should I start it?",
        "Rebased T8 on master. The conflicts were in the theme file. Should I start it?",
        "T8 is ready.\n\nThe change:\n- the header\n- the footer\n\nShould I start it?",
    ] {
        starts.push((convo(&[&format!("agent:{question}"), "yes"]), "T8"));
    }
    let added = "agent:Added T10: rename the settings toggle. It's a small change in the settings screen. Should I start it?";
    starts.push((convo(&[added, "yes"]), "T10"));
    for question in ["I made T10 and T11. Both touch the settings screen. Want me to start them?", "T10 and T11 are ready. Each is a one-line change. Want me to start them?"] {
        for target in ["T10", "T11"] {
            starts.push((convo(&[&format!("agent:{question}"), "yes"]), target));
        }
    }
    // ...but not past something else that could be started.
    holds.push((convo(&["agent:T8 is ready. The dev server stopped. Should I start it?", "yes"]), "T8"));
    // 2. Plain words after the question.
    for question in [
        "Want me to start T8? It's the first of the two.",
        "Should I start T8? I'll write the tests first.",
        "Should I start T8? Nothing blocks it now.",
        "Should I start T8? T9 merged, so nothing is waiting on it.",
        "Should I start T8? I'll hold the PR as a draft until you look.",
    ] {
        starts.push((convo(&[&format!("agent:{question}"), "yes"]), "T8"));
    }
    // ...while ones that set something before the start still hold it.
    for question in ["Should I start T8? It's blocked on T9.", "Should I start T8? I need to check T9 first.", "Should I start T8? I'd wait until T9 merges."] {
        holds.push((convo(&[&format!("agent:{question}"), "yes"]), "T8"));
    }
    // 3. The task the agent made, not one it mentions in passing.
    for question in ["I made T10. T8 is still in review. Want me to start it?", "I made T10 for the follow-up. T8 stays as it is. Want me to start it?"] {
        starts.push((convo(&[&format!("agent:{question}"), "yes"]), "T10"));
        holds.push((convo(&[&format!("agent:{question}"), "yes"]), "T8"));
    }
    let old_one = "agent:I made T10 and T11. T8 is the old one. Want me to start them?";
    for target in ["T10", "T11"] {
        starts.push((convo(&[old_one, "yes"]), target));
    }
    holds.push((convo(&[old_one, "yes"]), "T8"));
    // A "don't start" after the yes, or typed after the ask, holds it.
    for no in ["yes, don't start until T9 merges", "yes, don't start it until T9 merges"] {
        holds.push((convo(&["agent:Should I start T8?", no]), "T8"));
    }
    holds.push((convo(&["start T8, don't start until T9 merges"]), "T8"));
    // #157: a no on another task holds only that one, and "it" can be the task the statement after the
    // question speaks of.
    for no in ["yes, but don't start T9", "yes, and don't touch T9"] {
        starts.push((convo(&["agent:Should I start T8?", no]), "T8"));
        holds.push((convo(&["agent:Should I start T8?", no]), "T9"));
    }
    starts.push((convo(&["agent:Should I start it? T8 is ready.", "yes"]), "T8"));
    let wrong = start_cases_wrong(&starts, &holds);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Every phrase in #155, both ways, as #126's are read: a goal word before the thing's name only says
/// which ("the goal's migration" is a migration), and a summary of the goal, a list of its tasks or a
/// sentence that only says more of it leaves "it" the goal.
#[test]
fn the_goal_runs_as_155_found() {
    let mut runs: Vec<Vec<&str>> = vec![];
    let mut refused: Vec<Vec<&str>> = vec![];
    // 1. A goal word as whose, or before the thing's name.
    for reply in [
        "agent:For the goal I wrote seed.sh. Want me to run it?",
        "agent:The plan's seed script is ready. Want me to run it?",
        "agent:The goal's migration is written. Want me to run it?",
        "agent:The goal's smoke test is ready. Want me to run it?",
        "agent:Its first task needs a seed script. Want me to run it?",
        "agent:The plan needs a migration. I wrote it. Want me to run it?",
        "agent:I wrote the goal's seed script. Want me to run it?",
        "agent:For the plan I drafted a migration. Want me to run it?",
        "agent:In the goal there's a codemod. Want me to run it?",
    ] {
        refused.push(vec!["plan", reply, "say:yes"]);
        refused.push(vec!["say:make a goal for dark mode", "goal new", reply, "say:yes"]);
    }
    for ask in ["say:make the plan's seed script", "say:add a goal-level smoke test", "say:add a task-level lint", "say:write the goal's migration", "say:write the plan's backfill"] {
        refused.push(vec!["plan", ask, "agent:Done.", "say:run it"]);
        refused.push(vec!["say:make a goal for dark mode", "goal new", ask, "agent:Done.", "say:run it"]);
    }
    // 2. A summary of the goal, a list of its tasks, or a sentence that only says more of it.
    for reply in [
        "agent:I split the work into two tasks. Want me to run it?",
        "agent:It has a Colors task and a Toggle task. Want me to run it?",
        "agent:The changes are small. Want me to run it?",
        "agent:Made G. Some of the work is in the settings screen. Want me to run it?",
        "agent:Made G. My guess is a day of work. Want me to run it?",
        "agent:Made G. This touches the header and the footer. Want me to run it?",
        "agent:Made G. The work is split so each PR stays small. Want me to run it?",
        "agent:Made G:\n- T10 Colors\n- T11 Toggle\n\nWant me to run it?",
        "agent:Made G:\n1. Colors: touches the theme\n2. Toggle: adds the switch\n\nWant me to run it?",
        "agent:Here's the plan:\n- Colors: update the theme files\n- Toggle: add the switch\n\nWant me to run it?",
        "agent:Here's a summary:\n- Colors: write a seed script for the theme\n- Toggle: add the switch\n\nWant me to run it?",
        "agent:Made G. Want me to run it? Colors goes first.",
        "agent:Made G. Want me to run it? Nothing blocks it now.",
        "agent:Made G. Want me to run it? I'll write the tests first in each task.",
        "agent:Made G with T10 and T11. Want me to run it?",
    ] {
        runs.push(vec!["plan", reply, "say:yes"]);
        runs.push(vec!["say:make a goal for dark mode", "goal new", reply, "say:yes"]);
    }
    runs.push(vec!["say:make a goal for the migration", "goal new", "agent:Made G. The first touches the theme.", "say:run it"]);
    runs.push(vec!["say:make a goal for the export screen", "goal new", "agent:Made G. The work is mostly UI.", "say:run it"]);
    // Two goals made: "it" is the newer.
    runs.push(vec!["say:make a goal for dark mode", "goal new", "say:make a goal for the toggle", "goal new", "say:run it"]);
    runs.push(vec!["say:make two goals, one for dark mode and one for the toggle", "goal new", "goal new", "say:run it"]);
    // Planned as a goal.
    runs.push(vec!["say:plan the login tests as a goal", "goal new", "agent:Planned. Want me to run it?", "say:yes"]);
    let wrong = goal_cases_wrong(&runs, &refused);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
