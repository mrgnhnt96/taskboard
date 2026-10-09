//! The owner's word to start a task from a terminal (`tb start T4`). Only a human starts a task: by
//! pressing Start on the board, or by telling an agent to. The board doesn't take the agent's say-so:
//! it reads the prompts the human typed in that terminal (the UserPromptSubmit hook reports each one)
//! and starts the task only when one of them asks for it.

use once_cell::sync::Lazy;
use regex::Regex;

use crate::app::App;
use crate::p;
use crate::util::*;

/// `session_events.data` on a prompt the board sent (`[task-board:T4] …`), which is no one's word.
pub const BOARD_PROMPT: &str = "board";

static START_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:start|queue|kick\s+(?:\w+\s+)?off|begin|launch)").unwrap());
/// "don't start it", "without queueing", "no need to start T4": the word is there, but it says no.
static NOT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(?:don[’']?t|do\s+not|never|not|no\s+need\s+to|without|shouldn[’']?t|won[’']?t)\b(?:\s+\w+){0,2}\s*$").unwrap());
static REF_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b[Tt](\d+)\b").unwrap());

/// Whether `text` asks for something to start or be queued (and doesn't say not to).
pub fn says_start(text: &str) -> bool {
    START_RE.find_iter(text).any(|m| !NOT_RE.is_match(&text[..m.start()]))
}

/// The tasks `text` names (T4 → 4).
pub fn named_tasks(text: &str) -> Vec<i64> {
    REF_RE.captures_iter(text).filter_map(|c| c[1].parse().ok()).collect()
}

/// Whether one prompt, newest first at `index`, is the word to start task `id`: it asks for a start and
/// names the task, or it's the latest prompt and names no task ("make a task for it and queue it").
pub fn is_word(text: &str, index: usize, id: i64) -> bool {
    if !says_start(text) {
        return false;
    }
    let named = named_tasks(text);
    named.contains(&id) || (index == 0 && named.is_empty())
}

/// The prompt a human typed in terminal `sid`, since its Claude conversation began, that asks for task
/// `id` to start, if there's one.
pub fn owners_word(app: &App, sid: &str, id: i64) -> Result<Option<String>> {
    if sid.is_empty() {
        return Ok(None);
    }
    let prompts = app.db.q(
        "SELECT text FROM session_events WHERE session_id = ? AND kind = 'prompt' AND COALESCE(data, '') != ?
           AND id > COALESCE((SELECT MAX(id) FROM session_events WHERE session_id = ? AND kind = 'start'), 0)
         ORDER BY id DESC",
        p![sid, BOARD_PROMPT, sid],
    )?;
    Ok(prompts.iter().enumerate().map(|(i, r)| (i, r.st("text"))).find(|(i, t)| is_word(t, *i, id)).map(|(_, t)| t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_is_asked_for_unless_it_says_not_to() {
        assert!(says_start("create a new task and queue it"));
        assert!(says_start("ok, start T4"));
        assert!(says_start("kick it off"));
        assert!(says_start("Go ahead and launch T7 now"));
        assert!(!says_start("make a task for it but don't start it"));
        assert!(!says_start("add T4, do not queue it yet"));
        assert!(!says_start("fix the restart bug"));
        assert!(!says_start("what's on the board?"));
    }

    #[test]
    fn the_word_names_the_task_or_is_the_latest_prompt() {
        assert!(is_word("start T4 and T5", 3, 5));
        assert!(!is_word("start T4", 0, 5), "it names another task");
        assert!(is_word("create a new task and queue it", 0, 9));
        assert!(!is_word("create a new task and queue it", 1, 9), "an older prompt that names no task is no word for a later one");
    }
}
