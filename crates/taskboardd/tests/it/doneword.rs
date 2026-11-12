//! `tb needs done H4`: an agent marks the owner's to-dos done only on the word the owner typed in its terminal.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::doneword::{as_asks, word_in, Said};
use taskboardd::startword::Prompt;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, reports, runner};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    let b = Board { app, dir };
    b.session("s1");
    b.session("s2");
    b
}

impl Board {
    fn session(&self, sid: &str) {
        let repo = self.dir.path().join("webapp");
        midna::sync(
            &self.app,
            &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})],
            &[],
        )
        .unwrap();
    }
    fn report(&self, sid: &str, event: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": sid, "claude_session": format!("c-{sid}"), "cwd": ""});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap()
    }
    fn said(&self, prompt: &str) {
        self.report("s1", "hook.prompt", json!({"prompt": prompt}));
    }
    /// What the hook tells the agent about `prompt`.
    fn told(&self, prompt: &str) -> String {
        self.report("s1", "hook.prompt", json!({"prompt": prompt}))["context"].as_str().unwrap_or("").to_string()
    }
    /// The agent's message ends with `text`.
    fn replied(&self, text: &str) {
        self.report("s1", "hook.stop", json!({"last_message": text}));
    }
    /// A human task the agent on `sid` adds; its id.
    fn human(&self, sid: &str, title: &str) -> i64 {
        let out = self.report(sid, "tb.new_task", json!({"title": title, "detail": "1. Sign up.\n2. Send back the name.", "project": "webapp", "kind": "human"}));
        runner::start_queued(&self.app).unwrap();
        out["created"][0].as_str().unwrap()[1..].parse().unwrap()
    }
    /// What `tb needs done` sends from terminal s1.
    fn needs_done(&self, ids: &[i64]) -> Result<Value, (u16, String)> {
        let tasks: Vec<String> = ids.iter().map(|id| format!("H{id}")).collect();
        api::dispatch(&self.app, "POST", "/humans/done", &Query::new(), &json!({"tasks": tasks, "via_session": "s1"})).map_err(|e| (e.status, e.message))
    }
    fn refused(&self, ids: &[i64], why: &str) {
        let (code, msg) = self.needs_done(ids).unwrap_err();
        assert_eq!(code, 403, "{why}: {msg}");
        assert!(ids.iter().all(|&id| self.status(id) != "done"), "{why}: none is marked");
    }
    fn status(&self, id: i64) -> String {
        board::get_task(&self.app, id).unwrap().st("status")
    }
}

#[test]
fn the_owners_word_naming_the_tasks_marks_them_done() {
    for word in ["mark H{a} and H{b} done", "H{a} and H{b} are done.", "I did H{a} and H{b}", "H{a}, H{b}: done", "Mark as done H{a} and H{b}", "done with H{a} and H{b}"] {
        let b = board();
        let (a, z) = (b.human("s1", "Make an account"), b.human("s1", "Sign the agreement"));
        b.refused(&[a, z], "nobody said so yet");
        b.said(&word.replace("{a}", &a.to_string()).replace("{b}", &z.to_string()));
        assert_eq!(b.needs_done(&[a, z]).unwrap()["done"], json!([format!("H{a}"), format!("H{z}")]), "{word}");
        assert_eq!(b.status(a), "done");
    }
}

#[test]
fn them_means_the_human_tasks_talked_over_before_it() {
    let b = board();
    let (a, z) = (b.human("s1", "Make an account"), b.human("s1", "Sign the agreement"));
    let other = b.human("s2", "Buy the domain");
    b.replied(&format!("I've added H{a} and H{z} for you: the account and the agreement."));
    b.said("those are done, mark them as done");
    b.refused(&[a, z, other], "another terminal's to-do was never talked over here");
    assert!(b.needs_done(&[a, z]).is_ok());

    // A to-do first talked over after the word isn't covered by it.
    let later = b.human("s1", "Renew the cert");
    b.refused(&[later], "made after the word");
}

#[test]
fn it_means_the_one_the_agent_named() {
    let b = board();
    let (a, z) = (b.human("s1", "Make an account"), b.human("s1", "Sign the agreement"));
    b.replied(&format!("H{a} and H{z} are waiting on you."));
    b.said("done");
    b.refused(&[a], "a bare done after two named means neither");
    b.refused(&[z], "a bare done after two named means neither");
    b.replied(&format!("Is H{z} finished? The agreement link was in the steps."));
    b.said("yes, done");
    b.refused(&[a], "the message before named only the other");
    assert!(b.needs_done(&[z]).is_ok());
}

#[test]
fn a_bare_done_answers_only_a_message_that_named_a_to_do() {
    let b = board();
    let a = b.human("s1", "Make an account");
    b.replied("The tests pass now. Ready for the next step?");
    b.said("done");
    b.refused(&[a], "the message before was about something else");
    b.said("it's done");
    assert!(b.needs_done(&[a]).is_ok(), "\"it\" means the only to-do talked over");
}

#[test]
fn a_prompt_that_holds_it_back_is_no_word() {
    for word in [
        "is H{a} done?",
        "H{a} isn't done yet",
        "don't mark H{a} done",
        "I haven't done H{a}",
        "mark H{a} done once the email comes",
        "mark H{a} done tomorrow",
        "I think H{a} is done",
        "should you mark H{a} done?",
    ] {
        let b = board();
        let a = b.human("s1", "Make an account");
        b.said(&word.replace("{a}", &a.to_string()));
        b.refused(&[a], word);
    }
}

#[test]
fn a_later_no_takes_it_back_and_a_but_not_leaves_one_out() {
    let b = board();
    let (a, z) = (b.human("s1", "Make an account"), b.human("s1", "Sign the agreement"));
    b.said(&format!("mark H{a} done"));
    b.said("wait, no");
    b.refused(&[a], "taken back");

    b.replied(&format!("H{a} and H{z} are still open."));
    b.said(&format!("mark them done but not H{z}"));
    b.refused(&[a, z], "one in the batch has no word");
    assert!(b.needs_done(&[a]).is_ok());
}

#[test]
fn the_boards_prompt_is_no_ones_word() {
    let b = board();
    let a = b.human("s1", "Make an account");
    b.said(&format!("[task-board:T1] You are picking up “Deploy”. H{a} is done; mark H{a} done."));
    b.refused(&[a], "the board's prompt");
}

#[test]
fn the_hook_tells_the_agent_what_the_prompt_is() {
    let b = board();
    let (a, z) = (b.human("s1", "Make an account"), b.human("s1", "Sign the agreement"));
    let said = b.told(&format!("H{a} is done"));
    assert!(said.contains(&format!("tb needs done H{a})")) && !said.contains(&format!("H{z}")), "{said}");
    b.replied(&format!("Noted. H{z} is still open."));
    let said = b.told("both are done");
    assert!(said.contains(&format!("tb needs done H{a} H{z}")), "{said}");
    let said = b.told(&format!("is H{z} done?"));
    assert!(said.contains("not the owner's word") && said.contains("don't run tb needs done"), "{said}");
    assert!(!b.told("thanks, looks good").contains("done"));
    b.refused(&[z], "the question after holds it back");
    assert!(b.needs_done(&[a]).is_ok(), "the question names only the other");
}

#[test]
fn the_word_is_read_from_the_prompts_alone() {
    let said = |text: &str, talked: &[i64], named: &[i64]| Said { prompt: Prompt::new(text), talked: talked.to_vec(), named: named.to_vec() };
    assert_eq!(word_in(&[said("mark them done", &[3, 4], &[])], 4), Some(0));
    assert_eq!(word_in(&[said("mark it done", &[3, 4], &[])], 4), None, "it, with two talked over, is neither");
    assert_eq!(word_in(&[said("mark it done", &[3, 4], &[4])], 4), Some(0), "it is the one the agent just named");
    assert_eq!(word_in(&[said("thanks", &[4], &[]), said("H4 done", &[4], &[])], 4), Some(1), "a later prompt that says nothing keeps it");
    assert_eq!(word_in(&[said("actually no, H4 isn't done", &[4], &[]), said("H4 done", &[4], &[])], 4), None);
    assert_eq!(as_asks("the to-dos are done"), "mark the tasks done");
}
