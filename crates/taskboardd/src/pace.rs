//! Plan-check warnings on how fast a goal's plan can go, so a re-plan that slows it down says so when it's
//! made rather than days later: a long line of tasks that each wait for the one before (tasks that share a
//! PR share a branch and wait for the group's last task, a stacked task waits for its parent, and plain
//! waits), a plan spread over many waves or grown to many more than it had, and far more work queued than
//! the goal's terminal limit runs at once. The cure is usually the same: small findings on one screen or
//! area go into one task with a checklist of the backlog items it fixes, and screens run side by side.

use std::collections::{BTreeSet, HashMap};

use crate::app::App;
use crate::util::*;
use crate::{board, waitsfor};

/// More tasks than this in one line, each waiting for the one before, runs a goal single file.
pub const LINE_MAX: usize = 5;
/// More waves than this still to run is a plan that crawls.
pub const WAVES_MAX: usize = 12;
/// A change that grows the waves still to run by this many, to at least twice what they were, is a
/// re-plan that slowed the goal down.
const WAVES_GROWTH: usize = 6;
/// More open tasks than this many per terminal the goal allows, and its limit sets its pace.
pub const TASKS_PER_TERMINAL: usize = 8;
/// The goal's terminal limit when it sets none (`runner::goal_blocker`).
const DEFAULT_TERMINALS: i64 = 2;
/// How many refs of a long line the warning names at each end.
const LINE_ENDS: usize = 3;

/// The goal's open tasks that run in a terminal: not done, not the owner's.
fn open(app: &App, g: &Row) -> Result<Vec<Row>> {
    Ok(board::goal_tasks(app, g.id())?.into_iter().filter(|t| t.s("status") != Some("done") && !crate::human::is(t)).collect())
}

/// How many waves the goal's open tasks still spread over: what a change compares against.
pub fn waves_left(app: &App, g: &Row) -> Result<usize> {
    Ok(waves_of(&open(app, g)?))
}

fn waves_of(tasks: &[Row]) -> usize {
    tasks.iter().filter_map(|t| t.i("wave").filter(|w| *w > 0)).collect::<BTreeSet<i64>>().len()
}

/// The longest line of open tasks where each needs the one before it, first to last.
fn longest_line(tasks: &[Row]) -> Vec<i64> {
    let ids: BTreeSet<i64> = tasks.iter().map(|t| t.id()).collect();
    let deps: HashMap<i64, Vec<i64>> = tasks.iter().map(|t| (t.id(), waitsfor::deps(t).into_iter().filter(|d| ids.contains(d)).collect())).collect();
    let mut memo: HashMap<i64, Vec<i64>> = HashMap::new();
    fn walk(id: i64, deps: &HashMap<i64, Vec<i64>>, memo: &mut HashMap<i64, Vec<i64>>, seen: &mut Vec<i64>) -> Vec<i64> {
        if let Some(line) = memo.get(&id) {
            return line.clone();
        }
        if seen.contains(&id) {
            return vec![];
        }
        seen.push(id);
        let mut best: Vec<i64> = vec![];
        for d in deps.get(&id).into_iter().flatten() {
            let line = walk(*d, deps, memo, seen);
            if line.len() > best.len() {
                best = line;
            }
        }
        seen.pop();
        best.push(id);
        memo.insert(id, best.clone());
        best
    }
    let mut longest: Vec<i64> = vec![];
    for id in &ids {
        let line = walk(*id, &deps, &mut memo, &mut vec![]);
        if line.len() > longest.len() {
            longest = line;
        }
    }
    longest
}

/// "T1, T2, T3 … T9": a long line named by its ends.
fn line_text(line: &[i64]) -> String {
    let refs: Vec<String> = line.iter().map(|id| rf("task", *id)).collect();
    if refs.len() <= LINE_ENDS * 2 {
        return refs.join(", ");
    }
    format!("{} … {}", refs[..LINE_ENDS].join(", "), refs[refs.len() - LINE_ENDS..].join(", "))
}

const FOLD: &str = "put small findings on the same screen or area in one task, with a checklist of the backlog items it fixes \
                    (tb task new … --issue B<n> for each), not a line of tasks sharing a PR, and let the screens run side by side";

/// The plan check's pace warnings for goal `g`. `waves_before`: the waves its open tasks spread over
/// before this change, when the change added tasks.
pub fn plan_warnings(app: &App, g: &Row, waves_before: Option<usize>) -> Result<Vec<String>> {
    let tasks = open(app, g)?;
    let mut out = vec![];
    let line = longest_line(&tasks);
    if line.len() > LINE_MAX {
        out.push(format!(
            "{} tasks run one after another, each waiting for the one before (shared PRs, stacks and waits): {}. \
             More than {LINE_MAX} in a line keeps the goal single file however many terminals it has: {FOLD}.",
            line.len(),
            line_text(&line)
        ));
    }
    let waves = waves_of(&tasks);
    let grew = waves_before.filter(|b| *b > 0 && waves >= b * 2 && waves - b >= WAVES_GROWTH);
    if waves > WAVES_MAX || grew.is_some() {
        let was = waves_before.filter(|b| *b > 0 && *b != waves).map(|b| format!(" (it had {b} before this change)")).unwrap_or_default();
        out.push(format!(
            "The plan has {waves} waves still to run{was}. Tasks that share a PR go in different waves, so a line of them adds a wave each: {FOLD}."
        ));
    }
    let max = g.i("max_terminals").unwrap_or(DEFAULT_TERMINALS).max(1);
    if tasks.len() > max as usize * TASKS_PER_TERMINAL {
        let gr = rf("goal", g.id());
        out.push(format!(
            "{} has {} open tasks and runs at most {} at a time, so that limit sets its pace: fold small findings into one task per screen or area, \
             and tell the owner, who can raise it (tb goal set {gr} --max-terminals <n>).",
            gr,
            tasks.len(),
            plural(max, "terminal")
        ));
    }
    Ok(out)
}
