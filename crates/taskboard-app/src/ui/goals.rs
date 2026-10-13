//! Goal logic the app still needs now that the goals rail is gone: a goal's counts, which goals
//! there are, where "Goals" lands and what Esc on the filtered board resets to. The functions are
//! the web board's logic one to one; the parity tests run them against its output.
use crate::app::Filters;
use crate::fmt::{self, arr, b, i, s};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Counts {
    pub n: i64,
    pub done: i64,
    pub active: i64,
    pub queued: i64,
    pub prs: i64,
    pub finished: bool,
}

/// `awaitingMerge`: a done, not failed task whose PR is still open.
fn awaiting_merge(t: &Value) -> bool {
    let pr = &t["pr"];
    s(t, "status") == "done" && !b(t, "failed") && pr.is_object() && !pr["num"].is_null() && pr["state"].as_str().unwrap_or("OPEN").to_uppercase() == "OPEN"
}

/// `goalCounts`: from the goal's task list when it has one (goal detail), else its summary.
pub fn goal_counts(g: &Value) -> Counts {
    let c = match g.get("tasks").and_then(Value::as_array) {
        Some(tasks) => Counts {
            n: tasks.len() as i64,
            done: tasks.iter().filter(|t| s(t, "status") == "done").count() as i64,
            active: tasks.iter().filter(|t| matches!(s(t, "status"), "working" | "needs")).count() as i64,
            queued: tasks.iter().filter(|t| s(t, "status") == "queued").count() as i64,
            prs: tasks.iter().filter(|t| awaiting_merge(t)).count() as i64,
            finished: false,
        },
        None => Counts { n: i(g, "total"), done: i(g, "done"), active: i(g, "active"), queued: 0, prs: arr(g, "prs_open").len() as i64, finished: false },
    };
    // A backend bit not made yet holds back a finished goal ("Waiting on N bits").
    Counts { finished: c.n > 0 && c.done == c.n && c.prs == 0 && i(g, "bits_waiting") == 0, ..c }
}

/// `goalDone`.
pub fn finished(g: &Value) -> bool {
    goal_counts(g).finished
}

/// Every goal (`allGoals`): the full list, else the state's, minus archived ones.
pub fn all_goals<'a>(state: Option<&'a Value>, goals: Option<&'a [Value]>) -> Vec<&'a Value> {
    let list: &[Value] = match goals {
        Some(g) => g,
        None => state.map(|s| arr(s, "goals")).unwrap_or(&[]),
    };
    list.iter().filter(|g| !b(g, "archived")).collect()
}

fn same_ref(g: &Value, r: &str) -> bool {
    !r.is_empty() && r != "all" && fmt::ref_of(g, "G") == fmt::ref_of(&json!(r), "G")
}

/// `String.prototype.localeCompare` (the forms' node-verified port).
fn locale_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    crate::ui::modals::locale_cmp(a, b)
}

/// `railClear`.
pub fn rail_clear(f: &Filters) -> Filters {
    Filters { goal: "all".into(), project: "all".into(), done: f.done.clone() }
}

/// `gnavView`.
pub fn nav_kind(g: &Value) -> &'static str {
    if finished(g) {
        "done"
    } else if b(g, "deprioritized") {
        "deprio"
    } else {
        "active"
    }
}

/// The goal "Goals" opens (the web's `#/goals` with no id): the board's goal filter, else the
/// first active goal by project, else the first goal.
pub fn landing(goals: &[&Value], filter_goal: &str) -> Option<String> {
    if let Some(g) = goals.iter().find(|g| same_ref(g, filter_goal)) {
        return Some(fmt::ref_of(g, "G"));
    }
    let mut active: Vec<&&Value> = goals.iter().filter(|g| nav_kind(g) == "active").collect();
    active.sort_by(|a, b| locale_cmp(s(a, "project"), s(b, "project")));
    active.first().map(|g| fmt::ref_of(g, "G")).or_else(|| goals.first().map(|g| fmt::ref_of(g, "G")))
}

#[cfg(test)]
mod tests {
    use super::{Filters, goal_counts, landing, rail_clear};
    use crate::fmt::{b, s};
    use crate::parity::golden;
    use serde_json::{Value, json};

    fn filters(v: &Value) -> Filters {
        Filters { project: s(v, "project").into(), goal: s(v, "goal").into(), done: s(v, "done").into() }
    }

    #[test]
    fn counts_landing_and_clear_match_web() {
        let g = golden("chrome");
        g.only("goalCounts").check(|i| {
            let c = goal_counts(&i["goal"]);
            json!({"n": c.n, "done": c.done, "active": c.active, "queued": c.queued, "prs": c.prs, "finished": c.finished})
        });
        g.only("goalsLanding").check(|i| {
            let goals: Vec<&Value> = i["goals"].as_array().unwrap().iter().filter(|g| !b(g, "archived")).collect();
            json!(landing(&goals, i["filterGoal"].as_str().unwrap()))
        });
        g.only("railClear").check(|i| {
            let f = rail_clear(&filters(&i["filter"]));
            json!({"filter": {"project": f.project, "goal": f.goal, "done": f.done}, "keep": []})
        });
    }

    #[test]
    fn a_backend_bit_not_made_yet_holds_back_a_finished_goal() {
        let bits = json!({"tasks": [{"status": "done"}], "bits_waiting": 2});
        assert!(!goal_counts(&bits).finished);
    }
}
