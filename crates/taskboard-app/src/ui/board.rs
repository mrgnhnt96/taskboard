//! The board's filters (the web board's `loadState` query and `keepIssue`). The page they
//! filter is the home page (`ui::home`).
//!
//! `keep_issue` is tested one-to-one against the web board's own output (`parity/gen/board.mjs`).
use crate::app::{Filters, MainWindow, Page};

#[derive(Default)]
pub struct State {
    /// Issue refs the board keeps after the reader changed them (`?keep=`).
    pub keep: Vec<String>,
}

/// `loadState`'s query: the project, and the issues to keep. The board's own defaults stand for
/// the rest (every goal, tasks done in the last 24h). (For `app.rs`'s refresh to call.)
pub fn state_query(f: &Filters, keep: &[String]) -> Vec<(&'static str, String)> {
    let mut q = vec![("project", if f.project.is_empty() { "all".to_string() } else { f.project.clone() })];
    if !keep.is_empty() {
        q.push(("keep", keep.join(",")));
    }
    q
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
    use crate::parity::{golden, window};
    use gpui_kit::TestAppContext;
    use serde_json::json;

    #[test]
    fn state_query_sends_the_project_and_the_kept_issues() {
        let q = |project: &str, keep: &[&str]| state_query(&Filters { project: project.into() }, &keep.iter().map(|k| k.to_string()).collect::<Vec<_>>());
        assert_eq!(q("all", &[]), [("project", "all".to_string())]);
        assert_eq!(q("", &[]), [("project", "all".to_string())], "no project is all of them");
        assert_eq!(q("webapp", &["B3", "B4"]), [("project", "webapp".to_string()), ("keep", "B3,B4".to_string())]);
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
