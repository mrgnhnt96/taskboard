//! The board's filters (the web board's `loadState` query, `goal-pick` and `keepIssue`): the
//! project and goal the rail picks. The page they filter is the home page (`ui::home`).
//!
//! `state_query`, `goal_pick` and `keep_issue` are tested one-to-one against the web board's own
//! output (`parity/gen/board.mjs`).
use crate::app::{Filters, MainWindow, Page};
use crate::fmt::{self, arr, b, s};
use gpui_kit::*;
use serde_json::Value;

#[derive(Default)]
pub struct State {
    /// Issue refs the board keeps after the reader changed them (`?keep=`).
    pub keep: Vec<String>,
}

/// Non-archived goals: the full list once `GET /goals` answered, else the state's (`allGoals`).
fn all_goals<'a>(goals: &'a [Value], state: &'a Value) -> Vec<&'a Value> {
    let src = if goals.is_empty() { arr(state, "goals") } else { goals };
    src.iter().filter(|g| !b(g, "archived")).collect()
}

fn goal_by_ref<'a>(goals: &'a [Value], state: &'a Value, r: &str) -> Option<&'a Value> {
    all_goals(goals, state).into_iter().find(|g| fmt::ref_of(g, "G") == r)
}

/// `loadState`'s query: the goal goes as its number. (For `app.rs`'s refresh to call.)
pub fn state_query(f: &Filters, keep: &[String]) -> Vec<(&'static str, String)> {
    let goal = if f.goal == "all" { "all".to_string() } else { f.goal.trim_start_matches(|c: char| c.is_ascii_alphabetic()).to_string() };
    let mut q = vec![
        ("project", if f.project.is_empty() { "all".to_string() } else { f.project.clone() }),
        ("goal", goal),
        ("done", if f.done.is_empty() { "24h".to_string() } else { f.done.clone() }),
    ];
    if !keep.is_empty() {
        q.push(("keep", keep.join(",")));
    }
    q
}

/// `goal-pick`: show one goal (its project too), or `all`; picking the shown goal again shows
/// every goal. Clears the kept issues.
pub fn goal_pick(m: &mut MainWindow, arg: &str, cx: &mut Context<MainWindow>) {
    let mut f = m.filters.clone();
    f.goal = if arg != "all" && arg == f.goal { "all".into() } else { arg.to_string() };
    if f.goal != "all" {
        if let Some(p) = goal_by_ref(&m.data.goals, m.state(), &f.goal).map(|g| s(g, "project").to_string()).filter(|p| !p.is_empty()) {
            f.project = p;
        }
    }
    if f.goal == "all" {
        f.project = "all".into();
    }
    m.board.keep.clear();
    m.set_filters(f, cx);
}

/// `keepIssue`: keep an issue the reader just changed (board page only). The issue panel's
/// actions call it.
pub fn keep_issue(m: &mut MainWindow, r: &str) {
    if m.page == Page::Board && !m.board.keep.iter().any(|k| k == r) {
        m.board.keep.push(r.to_string());
    }
}

// ================================================================== tests

#[cfg(test)]
mod tests {
    use super::*;
    // Beats the glob's `gpui_kit::test`, which `#[gpui_kit::test]`'s own `#[test]` would hit.
    use ::core::prelude::v1::test;
    use crate::parity::{golden, window};
    use serde_json::json;

    fn filters(v: &Value) -> Filters {
        Filters {
            project: v["project"].as_str().unwrap_or("all").into(),
            goal: v["goal"].as_str().unwrap_or("all").into(),
            done: v["done"].as_str().unwrap_or("").into(),
        }
    }

    #[test]
    fn state_query_matches_web() {
        golden("board").only("state_query").check(|i| {
            let keep: Vec<String> = i["keep"].as_array().unwrap().iter().map(|k| k.as_str().unwrap().to_string()).collect();
            let q: serde_json::Map<String, Value> = state_query(&filters(&i["filter"]), &keep).into_iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
            Value::Object(q)
        });
    }

    fn with_filters(m: &mut MainWindow, f: &Value) {
        let mut x = filters(f);
        if f["done"].is_null() {
            x.done = String::new();
        }
        m.filters = x;
    }

    fn as_json(f: &Filters, had_done: bool) -> Value {
        let mut v = json!({"project": f.project, "goal": f.goal});
        if had_done || !f.done.is_empty() {
            v["done"] = json!(f.done);
        }
        v
    }

    #[gpui_kit::test]
    fn goal_pick_matches_web(cx: &mut TestAppContext) {
        let (w, _rec) = window(cx);
        for c in golden("board").cases {
            let f = &c["input"];
            if f["fn"].as_str() != Some("goal_pick") {
                continue;
            }
            let had_done = !f["filter"]["done"].is_null();
            let got = w
                .update(cx, |m, _, cx| {
                    // The golden's goals: G2 lives in api.
                    m.data.goals = vec![json!({"id": 1, "ref": "G1", "project": "webapp"}), json!({"id": 2, "ref": "G2", "project": "api"})];
                    with_filters(m, &f["filter"]);
                    m.board.keep = vec!["B1".into()];
                    goal_pick(m, f["arg"].as_str().unwrap(), cx);
                    json!({"filter": as_json(&m.filters, had_done), "keep": m.board.keep})
                })
                .unwrap();
            assert_eq!(got, c["expect"], "{}", c["name"]);
        }
    }

    #[gpui_kit::test]
    fn keep_issue_only_on_the_board(cx: &mut TestAppContext) {
        let (w, _rec) = window(cx);
        for c in golden("board").only("keep_issue").cases {
            let i = &c["input"];
            let got = w
                .update(cx, |m, _, _| {
                    m.page = if i["page"] == "board" { Page::Board } else { Page::Backlog };
                    m.board.keep = vec!["B1".into()];
                    for r in i["add"].as_array().unwrap() {
                        keep_issue(m, r.as_str().unwrap());
                    }
                    json!(m.board.keep)
                })
                .unwrap();
            assert_eq!(got, c["expect"], "{}", c["name"]);
        }
    }
}
