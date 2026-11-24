//! Trial-use tasks: "Use the iPhone app for a week and report back", or an agent task that comes back a
//! week later to check on its own work. The owner uses what ships and fixes what bothers them, so the
//! board refuses both: a human task is only for what only the owner can do (a sign-in, an account, a
//! purchase, a signature), and an outcome is proved by agent verification tasks before the work finishes.

use crate::app::App;
use crate::util::*;

/// Lowercased words, keeping apostrophes ("don't") and hyphens ("14-day" is an adjective, not a length).
fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .replace('’', "'")
        .split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-')
        .map(|w| w.trim_matches(|c| c == '\'' || c == '-'))
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect()
}

/// The title and each sentence of the detail, as they were written (to quote) and as words.
fn sentences(title: &str, detail: &str) -> Vec<(String, Vec<String>)> {
    std::iter::once(title)
        .chain(detail.split(['.', '!', '?', ';', '\n']))
        .map(|s| s.trim().trim_start_matches(|c: char| c.is_ascii_digit() || c == ')' || c == '-' || c == '*').trim())
        .filter(|s| !s.is_empty())
        .map(|s| (s.to_string(), words(s)))
        .collect()
}

fn is_quantity(w: &str) -> bool {
    matches!(w, "a" | "an" | "one" | "two" | "three" | "four" | "five" | "six" | "seven" | "ten" | "few" | "several" | "couple" | "some")
        || (!w.is_empty() && w.chars().all(|c| c.is_ascii_digit()))
}

/// Where `w` holds a length of time in one of `units` with a quantity before it ("a week", "3 days", "a
/// couple of days", "a few more days"), the index of its quantity. "This week" and "weekly" aren't lengths.
fn lengths(w: &[String], units: &[&str]) -> Vec<usize> {
    (0..w.len())
        .filter(|&i| units.contains(&w[i].as_str()))
        .filter_map(|i| match i {
            0 => None,
            _ if is_quantity(&w[i - 1]) => Some(i - 1),
            _ if i >= 2 && matches!(w[i - 1].as_str(), "of" | "more" | "full" | "whole" | "entire") && is_quantity(&w[i - 2]) => Some(i - 2),
            _ => None,
        })
        // "a few days", "a couple of days": the length starts at the "a".
        .map(|q| if q > 0 && matches!(w[q].as_str(), "few" | "couple") && matches!(w[q - 1].as_str(), "a" | "an") { q - 1 } else { q })
        .collect()
}

fn has(w: &[String], any: &[&str]) -> bool {
    w.iter().any(|w| any.contains(&w.as_str()))
}

fn has_pair(w: &[String], pairs: &[(&str, &str)]) -> bool {
    w.windows(2).any(|p| pairs.iter().any(|(a, b)| p[0] == *a && p[1] == *b))
}

fn dogfood(w: &[String]) -> bool {
    w.iter().any(|w| w.starts_with("dogfood"))
}

/// Days or more: what no task waits out.
const DAYS: &[&str] = &["days", "week", "weeks", "month", "months", "fortnight"];

/// A sentence where the task waits days to check on its own work: a wait of days ("wait a week", "a week
/// later", "check back in 3 days", "checked after a week") or a soak of days, or any dogfooding.
fn waits_days(w: &[String]) -> bool {
    if dogfood(w) {
        return true;
    }
    let at = lengths(w, DAYS);
    if at.is_empty() {
        return false;
    }
    let later = at.iter().any(|&q| w[q..].iter().position(|x| DAYS.contains(&x.as_str())).is_some_and(|u| w.get(q + u + 1).is_some_and(|x| x == "later")));
    let after = at.iter().any(|&q| q > 0 && matches!(w[q - 1].as_str(), "after" | "in"));
    // "Check the token expires after a week" is about a length the code keeps, not one the task waits out.
    let kept_length = has(w, &["expire", "expires", "expired", "expiry", "ago", "old", "older", "stale", "retention", "retained", "purge", "purged", "timeout", "every", "last", "past"]);
    // "Wait a week", "soak for a few days": the wait is the length's, not one for a build in the same sentence.
    let waited = at.iter().any(|&q| {
        w[q.saturating_sub(3)..q].iter().any(|x| matches!(x.as_str(), "wait" | "waits" | "waiting" | "waited" | "revisit" | "revisits" | "revisiting" | "soak" | "soaks" | "soaking"))
    });
    later
        || waited
        || has_pair(w, &[("come", "back"), ("comes", "back"), ("check", "back"), ("checks", "back"), ("check", "again"), ("checks", "again"), ("look", "again")])
        || (after && has(w, &["checked", "recheck", "rechecks", "rechecked", "re-check", "re-checks", "re-checked"]))
        || (after && !kept_length && has(w, &["check", "checks", "look", "measure", "measures", "remeasure", "compare", "review"]))
}

/// A sentence that asks the owner to use or try something for a while and report back: using or trying
/// something for a length of time ("use it for a week", "test it over a few days"), dogfooding, a soak, a
/// daily driver, a try-out, a report back, or a wait of days. "Use the code within 7 days" is a deadline.
fn trial_use(w: &[String]) -> bool {
    let for_a_while = lengths(w, &["day", "days", "week", "weeks", "month", "months", "fortnight", "while"])
        .iter()
        .any(|&q| q > 0 && matches!(w[q - 1].as_str(), "for" | "over" | "through"));
    let using = has(w, &["use", "uses", "using", "try", "tries", "trying", "switch", "run", "running", "test", "testing"]);
    (for_a_while && using)
        || has(w, &["soak", "soaking"])
        || has_pair(w, &[("report", "back"), ("reports", "back"), ("daily", "driver"), ("try", "out")])
        || w.windows(3).any(|t| t[0] == "try" && t[1] == "it" && t[2] == "out")
        || waits_days(w)
}

fn quote(s: &str) -> String {
    format!("\u{201c}{}\u{201d}", clip(s, 160))
}

/// The refusal for a human task that is a trial run rather than something only the owner can do.
pub fn human_refusal(app: &App, title: &str, detail: &str) -> Option<String> {
    let (said, _) = sentences(title, detail).into_iter().find(|(_, w)| trial_use(w))?;
    let owner = &app.cfg.owner;
    Some(format!(
        "A human task is only for what only {owner} can do (a sign-in, an account, a purchase, a signature), never for {owner} to use or try something for a while and report back: {}. \
         {owner} uses what ships and fixes what bothers them. Prove the outcome with an agent verification task instead (--device <tag> when it needs a device). \
         If part of it really is owner-only, write just those steps: the note {owner} leaves on Done says what they found.",
        quote(&said)
    ))
}

/// The refusal for an agent task that waits days to check on its own work.
pub fn agent_refusal(title: &str, detail: &str) -> Option<String> {
    let (said, _) = sentences(title, detail).into_iter().find(|(_, w)| waits_days(w))?;
    Some(format!(
        "A task never waits days to check on its own work: {}. Prove it before the task finishes, with tests or an agent verification task in the plan \
         (--device <tag> when it needs a device), never a check a week later, a soak or a trial run.",
        quote(&said)
    ))
}

/// Refuses a new task, human (`human`) or agent, that is a trial run.
pub fn refuse(app: &App, human: bool, title: &str, detail: &str) -> Result<()> {
    let why = if human { human_refusal(app, title, detail) } else { agent_refusal(title, detail) };
    match why {
        Some(why) => err(400, why),
        None => Ok(()),
    }
}

/// The plan check: refuses the whole plan before any of it is made when one of its tasks is a trial run,
/// naming the task. `items` are (title, detail, human).
pub fn refuse_plan(app: &App, items: &[(String, String, bool)]) -> Result<()> {
    for (title, detail, human) in items {
        let why = if *human { human_refusal(app, title, detail) } else { agent_refusal(title, detail) };
        if let Some(why) = why {
            return err(400, format!("{} can't be planned. {why}", quote(title)));
        }
    }
    Ok(())
}

