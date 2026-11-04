//! A task whose title or detail says it ends without a PR ("Measure only; no product code, no PR")
//! is set to end without one when it's made or its title or detail changes, so its handoff leaves out
//! the PR flow and its ticket. Only a task with no PR setting of its own: `tb task set T<n> --pr yes`
//! (or `--pr` on `tb task new`) wins, and undoes it.

use crate::app::App;
use crate::util::*;
use crate::{board, fields, projects, samepr, stack};

const PHRASES: &[&str] = &["no pr", "no pull request", "without a pr", "without a pull request", "measure only", "measurement only"];

/// A word after "no PR" that makes PR describe something else ("no PR yet", "no PR review").
const NOT_AFTER: &[&str] = &[
    "yet", "until", "review", "reviews", "reviewer", "reviewers", "comment", "comments", "body", "description", "title", "link", "number",
    "branch", "check", "checks", "stage", "step", "steps",
];

/// A word before the phrase that makes it a fallback, not what the task is ("if it needs nothing, finish with no PR").
const NOT_BEFORE: &[&str] = &["with", "if"];

/// The phrase in the text that says the task ends without a PR, if one does.
pub fn phrase(text: &str) -> Option<&'static str> {
    let text = text.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
    PHRASES.iter().copied().find(|p| text.match_indices(p).any(|(at, _)| stands_alone(&text, at, p.len())))
}

fn stands_alone(text: &str, at: usize, len: usize) -> bool {
    let word_char = |c: char| c.is_alphanumeric() || c == '-' || c == '_';
    let before = &text[..at];
    if before.chars().next_back().is_some_and(word_char) {
        return false;
    }
    let prev = before.split_whitespace().next_back().unwrap_or("").trim_matches(|c: char| !c.is_alphanumeric());
    if NOT_BEFORE.contains(&prev) {
        return false;
    }
    let rest = &text[at + len..];
    if rest.chars().next().is_some_and(word_char) {
        return false;
    }
    let next = rest.split_whitespace().next().unwrap_or("").trim_matches(|c: char| !c.is_alphanumeric());
    !NOT_AFTER.contains(&next)
}

/// Sets a task whose words say it ends without a PR to end without one, dropping its ticket unless one
/// is linked, and logs how to undo it. False when nothing changed.
pub fn apply(app: &App, id: i64, who: &str) -> Result<bool> {
    let t = board::get_task(app, id)?;
    let Some(said) = phrase(&format!("{}\n{}", t.st("title"), t.st("detail"))) else { return Ok(false) };
    if t.i("ships_pr").is_some()
        || t.s("kind").is_some_and(|k| !k.is_empty())
        || has(t.s("pr_url"))
        || matches!(t.s("status"), Some("done") | Some("failed"))
        || stack::parent_id(&t).is_some()
        || samepr::anchor_id(&t).is_some()
        || !samepr::members(app, id)?.is_empty()
        || !stack::children(app, id)?.is_empty()
        || !projects::ships_prs(app, &t.st("project"))?
    {
        return Ok(false);
    }
    if has(t.s("jira_key")) {
        board::update_task(app, id, fields!["ships_pr" => 0])?;
    } else {
        board::update_task(app, id, fields!["ships_pr" => 0, "jira_none" => 1])?;
    }
    let ticket = if has(t.s("jira_key")) { "" } else { " and no ticket" };
    let text = format!("Ends without a PR{ticket}: it says “{said}”. If it should end in one: tb task set {} --pr yes", rf("task", id));
    board::log_event(app, id, who, "note", &text)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::phrase;

    #[test]
    fn says_no_pr() {
        assert_eq!(phrase("Measure only; no product code, no PR"), Some("no pr"));
        assert_eq!(phrase("Measure only, then report"), Some("measure only"));
        assert_eq!(phrase("A measurement only task"), Some("measurement only"));
        assert_eq!(phrase("Ends without a PR."), Some("without a pr"));
        assert_eq!(phrase("No pull request: write it up"), Some("no pull request"));
        assert_eq!(phrase("Investigate it (no PR)"), Some("no pr"));
        assert_eq!(phrase("no\n  PR"), Some("no pr"));
    }

    #[test]
    fn keeps_pr_words_that_mean_something_else() {
        for text in [
            "No PR yet, so open one",
            "Open no duplicate PR",
            "We want no new PRs this week",
            "no PRs for the old branch",
            "There's no PR review on it",
            "Piano pr",
            "a no-PR task",
            "If it needs nothing after all, finish with no PR and say why.",
            "Open one if no PR covers it",
            "Fix the empty-state copy",
        ] {
            assert_eq!(phrase(text), None, "{text}");
        }
    }
}
