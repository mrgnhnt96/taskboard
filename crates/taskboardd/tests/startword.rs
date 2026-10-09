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
    let v = api::dispatch(&b.app, "POST", &format!("/goals/G{g}/run"), &Query::new(), &json!({})).unwrap();
    assert_eq!(v["queued_now"], 1);
    assert_eq!(b.status(id), "queued");
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
