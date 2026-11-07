//! Research tasks (`tb task new --kind research`): a task whose work is answering a question rather
//! than changing code. It stays a task (T<n>) with kind `research`. It ends without a PR, but
//! `tb submit` needs what it found attached, so the artifacts it made stay on the board. Unlike a
//! design task, nothing in its goal waits for it unless the plan says so.
//!
//! Research is caught as it starts in two places: `prompt_note` when the owner's prompt asks for it,
//! and the `PreToolUse` hook (`fan_out_refusal`) when an agent spins up a batch of subagents.

use once_cell::sync::Lazy;
use regex::Regex;

use crate::app::App;
use crate::board;
use crate::util::*;

pub const KIND: &str = "research";

/// What a research task's agent is told about finishing.
pub const HOW_IT_ENDS: &str = "It's a research task: it ends without a PR, once what it found is attached: \
                               tb attach <artifact link> --kind results --title \"<what it found>\", then tb submit.";

/// Words that only ask for research: "investigate" and "look into" are as often a bug's first step.
pub static SOUNDS_LIKE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(research(ing)?|deep[- ]dive|prior art|state of the art|(compare|evaluate|weigh) (the |our |some )?(options|alternatives|approaches|libraries|tools|vendors))\b").unwrap()
});

pub fn is(t: &Row) -> bool {
    t.s("kind") == Some(KIND)
}

/// The refusal for `tb submit` on a research task with nothing attached.
pub fn refuse_bare_submit(app: &App, t: &Row) -> Result<()> {
    if !is(t) || !board::attachments(app, Some(t.id()), None)?.is_empty() {
        return Ok(());
    }
    err(
        409,
        format!(
            "{} is a research task: attach what it found before you submit, so it isn't lost: \
             tb attach <artifact link> --kind results --title \"<what it found>\"",
            rf("task", t.id())
        ),
    )
}

/// The line `tb submit` logs on a research task: what it ends with.
pub fn done_text(app: &App, t: &Row) -> Result<Option<String>> {
    if !is(t) {
        return Ok(None);
    }
    Ok(board::attachments(app, Some(t.id()), None)?.last().map(|a| {
        let title = a["title"].as_str().filter(|s| !s.is_empty()).unwrap_or("Findings");
        format!("Research: {title} {}", a["url"].as_str().unwrap_or("")).trim().to_string()
    }))
}

/// How to put research on the board, for an agent that's about to do it.
fn how_to_track(tb: &str) -> String {
    format!(
        "Make a research task for each question: {tb} task new \"<the question>\" --kind research --detail \"<what to find out>\" --goal G<n>, \
         in the goal this work belongs to, or a new one (tb goal new \"<name>\" --tldr \"…\" --outcome \"<done when>\"). \
         One question you'll answer here yourself: --here in place of --goal. \
         Each task attaches the artifacts it makes (tb attach <link> --kind results)."
    )
}

/// The note for a prompt that asks for research, unless this terminal is already on a research task.
pub fn prompt_note(prompt: &str, task: Option<&Row>) -> Option<String> {
    if task.is_some_and(is) || !SOUNDS_LIKE.is_match(prompt) {
        return None;
    }
    Some(format!("Task board: if this prompt starts research, put it on the board before you begin. {}", how_to_track("tb")))
}

/// The refusal for a batch of subagents outside a research task. `what` names the batch.
pub fn fan_out_refusal(tb: &str, what: &str) -> String {
    format!(
        "Task board: {what} looks like research, and research goes on the board so it and its artifacts aren't lost. {} \
         Run a goal with {tb} start G<n> only if the owner said you can; otherwise tell them it's ready and ask. \
         If this isn't research (you're reading code for the work you're on), run the same call again and it goes through.",
        how_to_track(tb)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(kind: &str) -> Row {
        serde_json::from_value(json!({"id": 1, "kind": kind})).unwrap()
    }

    #[test]
    fn a_prompt_that_asks_for_research_gets_the_note() {
        for p in ["Can you research queue libraries?", "Do a deep dive on sync", "compare the options for auth"] {
            assert!(prompt_note(p, None).is_some(), "{p}");
        }
        assert!(prompt_note("Fix the login button", None).is_none());
        assert!(prompt_note("Investigate why the build fails", None).is_none());
    }

    #[test]
    fn a_research_task_gets_no_note_but_any_other_task_does() {
        assert!(prompt_note("research queues", Some(&task(KIND))).is_none());
        assert!(prompt_note("research queues", Some(&task("agent"))).is_some());
    }

    #[test]
    fn the_refusal_leaves_the_start_to_the_owner_and_says_how_to_carry_on() {
        let r = fan_out_refusal("tb", "this batch of subagents");
        assert!(r.contains("--kind research") && r.contains("only if the owner said you can") && r.contains("run the same call again"));
    }
}
