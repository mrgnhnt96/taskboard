//! Whether a backlog item blocks its task's PR, its goal, or nothing. Whoever files the item says so in
//! the same command (`tb found`, `tb backlog add`, `tb finding T<n> F<id> backlog`, each with `--blocks`),
//! under the rule in [`RULE`]. `goal` holds the goal open like `--misses-goal` (its old spelling): it sets
//! `misses_goal` and alerts the owner. `pr` and `goal` carry `blocks_why`, the concrete failure, path or
//! done-when line. While the filer's task has an open PR, `--blocks` can't be left out.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields};

pub const NONE: &str = "none";
pub const PR: &str = "pr";
pub const GOAL: &str = "goal";
pub const LEVELS: &[&str] = &[NONE, PR, GOAL];
const WHY_MAX: usize = 300;

/// The rule a filer classifies an item by, for the prompts that file backlog items.
pub const RULE: &str = "Each backlog item says whether it blocks, with --blocks none|pr|goal and, for pr or goal, \
--blocks-why naming the concrete failure, the path or the done-when line:\n\
- pr: merging this PR as it is leaves main worse than it is today, on code the PR changes or newly makes reachable: \
a crash; stored data lost, overwritten or counted wrong; a security hole someone could actually use (access they \
shouldn't have, leaked secrets, running commands); or something that works today stops working (a build, an install \
path, a feature removed with nothing in its place). A bug in brand-new code counts only if it does one of these. A \
human reviewer's request that was deferred is pr too.\n\
- goal: the goal's done-when isn't honestly met without it. Also a crash, data loss or security hole on main that \
this goal's work found, when no open PR caused it.\n\
- none (the default): everything else, including tests that already fail on clean main and harms that exist on \
main but aren't this goal's to fix.";

#[derive(Debug, Clone, PartialEq)]
pub struct Blocks {
    pub level: &'static str,
    pub why: Option<String>,
}

impl Blocks {
    pub fn none() -> Blocks {
        Blocks { level: NONE, why: None }
    }

    pub fn holds(&self) -> bool {
        self.level != NONE
    }
}

/// Who files the item: the task it's filed from (if any), the goal it lands in, and whether `--blocks`
/// must be given whatever the task's PR (code review triage always classifies).
pub struct Filer<'a> {
    pub task: Option<&'a Row>,
    pub goal_id: Option<i64>,
    pub always: bool,
}

impl Filer<'_> {
    fn required(&self) -> bool {
        self.always || self.task.is_some_and(board::pr_still_open)
    }
}

/// The task a terminal files from: its own, else the done task whose PR it's visiting.
pub fn session_task(app: &App, sid: Option<&str>) -> Result<Option<Row>> {
    if let Some(t) = board::task_for_session(app, sid)? {
        return Ok(Some(t));
    }
    match sid.filter(|s| !s.is_empty()) {
        Some(s) => crate::prflow::visited_by(app, s),
        None => Ok(None),
    }
}

/// Whether a filing asks for `blocks: goal` (or its `misses_goal` alias), so it lands in the filer's goal.
pub fn wants_goal(body: &Value) -> bool {
    body.get("blocks").and_then(|v| v.as_str()).is_some_and(|s| s.trim().eq_ignore_ascii_case(GOAL)) || as_bool(body.get("misses_goal"), false)
}

/// Reads `blocks` and `blocks_why` from a filing's body; `misses_goal` is `blocks: goal`, with `alias_why`
/// (the item's title) as its why. `why_default` stands in for a missing `blocks_why` (a triage reason).
pub fn read(body: &Value, filer: &Filer, why_default: Option<&str>, alias_why: &str) -> Result<Blocks> {
    let given = body.get("blocks").and_then(|v| v.as_str()).map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty());
    let misses = as_bool(body.get("misses_goal"), false);
    let level = match (given.as_deref(), misses) {
        (Some(l), true) if l != GOAL => return err(400, "--misses-goal is --blocks goal: give one or the other."),
        (Some(l), _) => match LEVELS.iter().find(|x| **x == l) {
            Some(x) => *x,
            None => return err(400, "--blocks is none, pr or goal."),
        },
        (None, true) => GOAL,
        (None, false) if filer.required() => {
            let why = match filer.task {
                Some(t) if !filer.always => format!("{}'s PR is open", rf("task", t.id())),
                _ => "code review triage files it".to_string(),
            };
            return err(400, format!("Say whether it blocks: --blocks none|pr|goal ({why}, so it can't wait to be classified)."));
        }
        (None, false) => NONE,
    };
    let given_why = body.get("blocks_why").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty());
    let why = given_why.or(why_default.map(str::trim).filter(|s| !s.is_empty())).or((given.is_none() && misses).then_some(alias_why));
    let why = why.map(|w| one_line(w, 1000));
    if level == NONE {
        return Ok(Blocks { level, why: None });
    }
    let Some(why) = why else {
        return err(400, format!("--blocks {level} needs --blocks-why \"<the concrete failure, path or done-when line>\"."));
    };
    if why.chars().count() > WHY_MAX {
        return err(400, format!("--blocks-why is one line of {WHY_MAX} characters at most."));
    }
    if level == GOAL && filer.goal_id.is_none() {
        let flag = if given.is_none() { "--misses-goal" } else { "--blocks goal" };
        return err(400, format!("{flag} is for a miss on a goal's done-when, and this isn't in a goal."));
    }
    if level == PR && filer.task.is_none() {
        return err(400, "--blocks pr holds a task's PR, and this isn't filed from a task.");
    }
    Ok(Blocks { level, why: Some(why) })
}

/// The issue fields a new item is filed with.
pub fn fields(b: &Blocks) -> Vec<(&'static str, Value)> {
    fields!["blocks" => b.level, "blocks_why" => b.why.clone(), "misses_goal" => (b.level == GOAL) as i64]
}

/// Marks a filed item (new, or the open one it was merged into) with what it blocks: `goal` holds the
/// goal open and alerts the owner; `pr` filed from a task holds that task's PR and becomes a pre-merge
/// fix (see [`crate::samepr::take_critical`]), and what became of it is returned for the filer. An item
/// merged into one that already blocks keeps the stronger mark, and one already holding a PR keeps it.
pub fn mark(app: &App, iid: i64, b: &Blocks, goal_id: Option<i64>, title: &str, who: &str, fresh: bool, filer: Option<&Row>) -> Result<Option<String>> {
    if !b.holds() {
        return Ok(None);
    }
    if !fresh {
        let was = board::get_issue(app, iid)?;
        if (was.s("blocks") == Some(GOAL) && b.level == PR) || (was.s("blocks") == Some(PR) && was.i("holds_pr").is_some()) {
            return Ok(None);
        }
        app.db.update("issues", &json!(iid), fields!["blocks" => b.level, "blocks_why" => b.why.clone(), "updated_at" => now_iso()])?;
        if b.level == GOAL {
            app.db.update("issues", &json!(iid), fields!["misses_goal" => 1])?;
        }
    }
    let what = if b.level == PR { "its task's PR" } else { "its goal" };
    board::add_issue_event(app, iid, who, "blocks", &format!("Blocks {what}: {}", b.why.as_deref().unwrap_or("")), None)?;
    match (b.level, goal_id, filer) {
        (GOAL, Some(g), _) => miss_alert(app, iid, g, title).map(|_| None),
        (PR, _, Some(t)) => crate::samepr::take_critical(app, iid, t, who),
        _ => Ok(None),
    }
}

/// A miss on the goal's own done-when goes to the owner as an alert rather than waiting quietly in the
/// backlog. It stays up until the issue is settled (made a task, ticketed or closed).
pub fn miss_alert(app: &App, issue: i64, goal_id: i64, title: &str) -> Result<()> {
    let g = board::get_goal(app, goal_id)?;
    let text = format!("{} misses its done-when: {title} ({})", g.st("name"), rf("issue", issue));
    crate::dispatch::add_alert_keyed(app, &text, None, Some(goal_id), &board::miss_key(issue))
}

/// A severity a reviewer gives a finding that isn't worth holding a PR over on its own.
pub fn low_severity(sev: &str) -> bool {
    matches!(sev.trim().to_lowercase().as_str(), "low" | "minor" | "info" | "nit" | "trivial" | "suggestion")
}

/// The files a unified diff touches, on either side.
pub fn diff_files(patch: &str) -> Vec<String> {
    let mut out: Vec<String> = patch
        .lines()
        .filter_map(|l| l.strip_prefix("+++ b/").or_else(|| l.strip_prefix("--- a/")))
        .map(|p| p.trim_end().to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Whether `file` (as a reviewer named it: relative, `./`-led or absolute) is one of the diff's files.
pub fn in_diff(file: &str, files: &[String]) -> bool {
    let f = file.trim().trim_start_matches("./");
    files.iter().any(|p| f == p || f.ends_with(&format!("/{p}")))
}

/// Code review triage's check on a `pr` mark: a low-severity finding in a file the change doesn't touch
/// can't hold the PR, so it blocks the goal instead (nothing, outside a goal). `None` when it stands.
pub fn off_diff(b: &Blocks, severity: &str, file: &str, patch: Option<&str>, goal_id: Option<i64>) -> Option<(Blocks, String)> {
    let patch = patch.filter(|p| !p.trim().is_empty())?;
    if b.level != PR || !low_severity(severity) || file.trim().is_empty() || in_diff(file, &diff_files(patch)) {
        return None;
    }
    let to = if goal_id.is_some() { GOAL } else { NONE };
    let instead = if to == GOAL { "blocks the goal instead" } else { "blocks nothing (it isn't in a goal)" };
    let text = format!("Filed as blocking the PR, but it's a {} finding in {}, which the change doesn't touch, so it {instead}.", severity.trim(), file.trim());
    Some((Blocks { level: to, why: if to == GOAL { b.why.clone() } else { None } }, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-x\n+y\n\
diff --git a/old.rs b/old.rs\n--- a/old.rs\n+++ /dev/null\n";

    #[test]
    fn diff_files_reads_both_sides() {
        assert_eq!(diff_files(PATCH), vec!["old.rs".to_string(), "src/a.rs".to_string()]);
    }

    #[test]
    fn in_diff_matches_relative_and_absolute_paths() {
        let files = diff_files(PATCH);
        assert!(in_diff("src/a.rs", &files));
        assert!(in_diff("./src/a.rs", &files));
        assert!(in_diff("/work/repo/src/a.rs", &files));
        assert!(!in_diff("src/b.rs", &files));
        assert!(!in_diff("xsrc/a.rs", &files));
    }

    #[test]
    fn off_diff_downgrades_only_low_findings_outside_the_change() {
        let pr = Blocks { level: PR, why: Some("crash".into()) };
        assert!(off_diff(&pr, "high", "src/b.rs", Some(PATCH), Some(1)).is_none());
        assert!(off_diff(&pr, "low", "src/a.rs", Some(PATCH), Some(1)).is_none());
        assert!(off_diff(&pr, "low", "src/b.rs", None, Some(1)).is_none());
        assert!(off_diff(&pr, "low", "src/b.rs", Some(""), Some(1)).is_none());
        let (to, text) = off_diff(&pr, "minor", "src/b.rs", Some(PATCH), Some(1)).unwrap();
        assert_eq!(to.level, GOAL);
        assert!(text.contains("src/b.rs"), "{text}");
        assert_eq!(off_diff(&pr, "nit", "src/b.rs", Some(PATCH), None).unwrap().0.level, NONE);
    }
}
