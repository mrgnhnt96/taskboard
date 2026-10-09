//! The owner's word to start a task from a terminal (`tb start T4`). Only a human starts a task: by
//! pressing Start on the board, or by telling an agent to. The board doesn't take the agent's say-so:
//! it reads the prompts the human typed in that terminal (the UserPromptSubmit hook reports each one)
//! and starts the task only when one of them asks for it.
//!
//! This is what keeps an agent from starting work outside work hours, so it errs on the side of no: a
//! prompt is the word only when a clause of it asks for the start ("start T8", "queue it", "kick off
//! T8"), not when it asks about one or tells of one ("why did T8 start failing?", "T8 started").

use once_cell::sync::Lazy;
use regex::Regex;

use crate::app::App;
use crate::p;
use crate::util::*;

/// `session_events.data` on a prompt the board sent (`[task-board:T4] …`), which is no one's word.
pub const BOARD_PROMPT: &str = "board";

/// `session_events.kind` for a task the terminal's conversation made (`tb task new`); `data` is its id.
pub const MADE: &str = "added";

/// Where an ask can begin: the start of the prompt or of a clause.
const EDGE: &str = r"(?:^|[.!?;:,\n(]|\b(?:and|then|but|so|also)\b)";
/// What may come before the start word in an ask ("ok,", "please", "go ahead and", "can you").
const LEAD: &str = r"(?:(?:ok(?:ay)?|alright|all\s+right|yes|yeah|yep|sure|great|cool|perfect|thanks|please|pls|now|then|so|also|just|go|go\s+ahead(?:\s+and)?|you\s+can|you\s+may|can\s+you|could\s+you|would\s+you|will\s+you|i\s+want\s+you\s+to|i[’']?d\s+like\s+you\s+to|let[’']?s|let\s+us|feel\s+free\s+to|time\s+to)\s*,?\s+)*";
/// The tasks an ask names: "T4", "task T4", "T4, T5 and T6".
const NAMED: &str = r"(?:task\s+)?[Tt]\d+(?:\s*,?\s*(?:and\s+|&\s*)?(?:task\s+)?[Tt]\d+)*";
/// The task an ask doesn't name: "it", "this", "the new task", "them".
const UNNAMED: &str = r"(?:it|this|that|them|these|those|both|(?:the|this|that|these|those|my|your|our)\s+(?:new\s+)?(?:one|ones|task|tasks))";
/// What may follow the task in an ask; anything else ("start T8 failed", "start this weekend") is no ask.
const TAIL: &str = r"(?:\s*$|\s*[.!?;:,\n)]|\s+(?:and|then|now|please|pls|too|again|up|off|asap|today|tonight|for\s+me|right\s+away|when|once|after|as\s+soon|in|on|with|from|so)\b)";

/// A start word at the start of a clause, on a task, with nothing after the task that makes it a story.
static ASK_RE: Lazy<Regex> = Lazy::new(|| {
    let verb = r"(?:start|queue|begin|launch|kick\s+off)(?:\s+up)?(?:\s+(?:work(?:ing)?\s+)?on)?";
    Regex::new(&format!(
        r"(?i){EDGE}\s*{LEAD}(?:\b{verb}\s+(?:(?P<named>{NAMED})|(?P<unnamed>{UNNAMED}))|\bkick\s+(?:(?P<named2>{NAMED})|(?P<unnamed2>{UNNAMED}))\s+off){TAIL}"
    ))
    .unwrap()
});
static REF_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b[Tt](\d+)\b").unwrap());

/// One ask for a start in a prompt: for the tasks it names, or for a task it doesn't ("queue it").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Named(Vec<i64>),
    Unnamed,
}

/// The asks for a start in `text`.
pub fn asks(text: &str) -> Vec<Ask> {
    let mut out = vec![];
    let mut at = 0;
    while let Some(c) = ASK_RE.captures_at(text, at) {
        let (named, unnamed) = (c.name("named").or(c.name("named2")), c.name("unnamed").or(c.name("unnamed2")));
        let Some(task) = named.or(unnamed) else { break };
        out.push(match named {
            Some(n) => Ask::Named(named_tasks(n.as_str())),
            None => Ask::Unnamed,
        });
        // Look again from the task on, so the next clause ("start T4 and queue T5") gets its edge back.
        at = task.end();
    }
    out
}

/// Whether `text` asks for something to start or be queued.
pub fn says_start(text: &str) -> bool {
    !asks(text).is_empty()
}

/// The tasks `text` names (T4 → 4).
pub fn named_tasks(text: &str) -> Vec<i64> {
    REF_RE.captures_iter(text).filter_map(|c| c[1].parse().ok()).collect()
}

/// Whether one prompt, newest first at `index`, is the word to start task `id`: it asks for the task by
/// name, or it's the latest prompt, asks for a start without a name ("make a task for it and queue it"),
/// and this conversation made the task after it (`made_since`).
pub fn is_word(text: &str, index: usize, id: i64, made_since: bool) -> bool {
    asks(text).iter().any(|a| match a {
        Ask::Named(ids) => ids.contains(&id),
        Ask::Unnamed => index == 0 && made_since,
    })
}

/// Notes on terminal `sid` that its conversation made task `id`: the task an unnamed "queue it" in the
/// prompt before can mean.
pub fn note_made(app: &App, sid: &str, id: i64) -> Result<()> {
    crate::board::session_event_with(app, sid, MADE, &format!("Added {}", rf("task", id)), None, Some(&id.to_string()))
}

/// The prompt a human typed in terminal `sid`, since its Claude conversation began, that asks for task
/// `id` to start, if there's one.
pub fn owners_word(app: &App, sid: &str, id: i64) -> Result<Option<String>> {
    if sid.is_empty() {
        return Ok(None);
    }
    let prompts = app.db.q(
        "SELECT id, text FROM session_events WHERE session_id = ? AND kind = 'prompt' AND COALESCE(data, '') != ?
           AND id > COALESCE((SELECT MAX(id) FROM session_events WHERE session_id = ? AND kind = 'start'), 0)
         ORDER BY id DESC",
        p![sid, BOARD_PROMPT, sid],
    )?;
    let made_since = match prompts.first() {
        Some(latest) => {
            app.db.count(
                "SELECT COUNT(*) FROM session_events WHERE session_id = ? AND kind = ? AND data = ? AND id > ?",
                p![sid, MADE, id.to_string(), latest.id()],
            )? > 0
        }
        None => false,
    };
    Ok(prompts.iter().enumerate().map(|(i, r)| (i, r.st("text"))).find(|(i, t)| is_word(t, *i, id, made_since)).map(|(_, t)| t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_is_asked_for_unless_it_says_not_to() {
        for yes in [
            "create a new task and queue it",
            "ok, start T4",
            "kick it off",
            "kick off T8",
            "kick T8 off",
            "Go ahead and launch T7 now",
            "start T8",
            "Start T8.",
            "queue it",
            "please queue T8",
            "start T8 please",
            "can you start T8?",
            "could you queue up T8 for me",
            "let's begin T8",
            "start working on T8",
            "start the new task",
            "yes, start it",
            "looks good. queue it",
            "no, start it",
            "(start T8)",
        ] {
            assert!(says_start(yes), "{yes}");
        }
        for no in [
            "make a task for it but don't start it",
            "add T4, do not queue it yet",
            "no need to start T4",
            "fix the restart bug",
            "what's on the board?",
            "why did T8 start failing yesterday?",
            "T8 started an hour ago",
            "started",
            "starting",
            "queued",
            "beginning",
            "it's starting now",
            "the dev server",
            "start the dev server and check the login page",
            "start the dev server",
            "kick off a rebuild",
            "did you start T8?",
            "should I start T8?",
            "when will you start T8?",
            "who started T8",
            "the server won't start",
            "you can't start T8 until T7 lands",
            "you cannot start T8",
            "Start T8 failed yesterday",
            "start T8 is broken",
            "start this weekend",
            "the log says \"start T8\"",
            "how do I start T8",
            "I started T8",
            "T8 is queued",
            "restart T8",
            "requeue it",
            "startup is slow",
            "launching T8 broke prod",
        ] {
            assert!(!says_start(no), "{no}");
        }
    }

    #[test]
    fn each_ask_names_its_own_tasks() {
        assert_eq!(asks("start T4 and T5"), vec![Ask::Named(vec![4, 5])]);
        assert_eq!(asks("start T4, T5 and task T6"), vec![Ask::Named(vec![4, 5, 6])]);
        assert_eq!(asks("start T4 and queue T5"), vec![Ask::Named(vec![4]), Ask::Named(vec![5])]);
        assert_eq!(asks("don't start T4, start T5"), vec![Ask::Named(vec![5])]);
        assert_eq!(asks("make a task and queue it"), vec![Ask::Unnamed]);
    }

    #[test]
    fn the_word_names_the_task_or_is_the_latest_prompt_for_a_task_made_after_it() {
        assert!(is_word("start T4 and T5", 3, 5, false));
        assert!(!is_word("start T4", 0, 5, true), "it names another task");
        assert!(!is_word("don't start T5, start T4", 0, 5, false), "the ask is for T4");
        assert!(is_word("create a new task and queue it", 0, 9, true));
        assert!(!is_word("create a new task and queue it", 0, 9, false), "this conversation didn't make the task after it");
        assert!(!is_word("create a new task and queue it", 1, 9, true), "an older prompt that names no task is no word for a later one");
        assert!(!is_word("why did T8 start failing yesterday?", 0, 8, true));
    }
}
