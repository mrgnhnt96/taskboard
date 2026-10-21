//! Goal logic the app still needs now that the goals rail is gone: a goal's counts, which goals
//! there are, which view each belongs in and what Esc on the filtered board resets to. The functions are
//! the web board's logic one to one; the parity tests run them against its output.
use crate::app::Filters;
use crate::fmt::{arr, b, i, s};
use serde_json::Value;

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
    // A backend bit not made yet holds back a finished goal ("Waiting on N bits"), and so does a missed
    // outcome: a failed task or an open miss on the goal's done-when ("Outcome not met"). A failed task the
    // owner finished the goal without (`tb goal done`) no longer holds it.
    let accepted = i(g, "failed_accepted");
    let (failed, settled) = match g.get("tasks").and_then(Value::as_array) {
        Some(tasks) => (tasks.iter().filter(|t| s(t, "status") == "done" && b(t, "failed")).count() as i64, c.done),
        None => (i(g, "failed"), c.done + accepted),
    };
    let met = failed == accepted && i(g, "misses") == 0;
    Counts { finished: c.n > 0 && settled == c.n && c.prs == 0 && i(g, "bits_waiting") == 0 && met, ..c }
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

#[cfg(test)]
mod tests {
    use super::{Filters, goal_counts, rail_clear};
    use crate::fmt::s;
    use crate::parity::golden;
    use serde_json::{Value, json};

    fn filters(v: &Value) -> Filters {
        Filters { project: s(v, "project").into(), goal: s(v, "goal").into(), done: s(v, "done").into() }
    }

    #[test]
    fn counts_and_clear_match_web() {
        let g = golden("chrome");
        g.only("goalCounts").check(|i| {
            let c = goal_counts(&i["goal"]);
            json!({"n": c.n, "done": c.done, "active": c.active, "queued": c.queued, "prs": c.prs, "finished": c.finished})
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

    #[test]
    fn a_missed_outcome_holds_back_a_finished_goal() {
        let failed = json!({"tasks": [{"status": "done"}, {"status": "done", "failed": true}]});
        assert!(!goal_counts(&failed).finished);
        let miss = json!({"tasks": [{"status": "done"}], "misses": 1});
        assert!(!goal_counts(&miss).finished);
        let summary = json!({"total": 2, "done": 1, "failed": 1});
        assert!(!goal_counts(&summary).finished);
        assert!(goal_counts(&json!({"tasks": [{"status": "done"}]})).finished);
        let accepted = json!({"tasks": [{"status": "done"}, {"status": "done", "failed": true}], "failed_accepted": 1});
        assert!(goal_counts(&accepted).finished);
        assert!(goal_counts(&json!({"total": 2, "done": 1, "failed": 1, "failed_accepted": 1})).finished);
        assert!(!goal_counts(&json!({"total": 3, "done": 1, "failed": 2, "failed_accepted": 1})).finished);
    }
}
