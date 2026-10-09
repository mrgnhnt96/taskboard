//! The board's PR watch in the window's chrome: the status-bar pills of an unhealthy PR feed
//! (`state.pr_feed`) and of stopped PR builds (`state.pr_builds`), and the "Master is red" banner
//! line for each open break of a watched default branch that's the owner's (`state.master`), which
//! stands in for that break's urgent alert.

use crate::app::{MainWindow, Pill, pill_shell};
use crate::fmt::{self, arr, s};
use crate::theme::Theme;
use crate::ui::kit;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

/// "PR feed stuck" / "PR feed silent", only while the feed is on and unhealthy.
pub fn feed_pill(f: &Value) -> Option<Pill> {
    if f["on"] != true || f["healthy"] != false {
        return None;
    }
    let mut title = vec![fmt::cap(s(f, "why"))];
    if let Some(at) = fmt::opt_s(f, "last_event_at") {
        title.push(format!("Last event {}", fmt::hhmm(at)));
    }
    if let Some(h) = fmt::opt_s(f, "holding") {
        title.push(h.to_string());
    }
    Some(Pill { cls: "down", label: format!("PR feed {}", s(f, "problem")), title: title.join("\n") })
}

/// "PR builds stopped" while the owner has them stopped, with who and when.
pub fn builds_pill(v: &Value) -> Option<Pill> {
    if v["stopped"] != true {
        return None;
    }
    let mut title = format!("Stopped by {}", fmt::opt_s(v, "by").unwrap_or("the owner"));
    if let Some(at) = fmt::opt_s(v, "at") {
        title += &format!(" at {}", fmt::hhmm(at));
    }
    if let Some(r) = fmt::opt_s(v, "reason") {
        title += &format!(": {r}");
    }
    Some(Pill { cls: "hours", label: "PR builds stopped".into(), title })
}

/// One "Master is red" line: its text, and the fix task to open.
#[derive(Clone, Debug, PartialEq)]
pub struct MasterRow {
    pub r#ref: String,
    pub text: String,
    pub ago: String,
    pub task: Option<String>,
}

/// The open breaks that are the owner's, oldest first.
pub fn master_rows(st: &Value) -> Vec<MasterRow> {
    arr(st, "master")
        .iter()
        .filter(|m| s(m, "verdict") == "ours")
        .map(|m| {
            let checks: Vec<&str> = arr(m, "checks").iter().filter_map(|c| c.as_str()).collect();
            MasterRow {
                r#ref: s(m, "ref").to_string(),
                text: format!("Master is red on {}: {} {} on {}. Yours to fix.", s(m, "project"), checks.join(", "), if checks.len() == 1 { "fails" } else { "fail" }, s(m, "branch")),
                ago: fmt::ago(s(m, "opened_at")),
                task: fmt::opt_s(&m["task"], "ref").map(str::to_string),
            }
        })
        .collect()
}

/// Whether an alert is a break's urgent alert (`master:M<n>`) that its "Master is red" line already shows.
pub fn shown_as_master(st: &Value, a: &Value) -> bool {
    let Some(r) = a["key"].as_str().and_then(|k| k.strip_prefix("master:")) else { return false };
    master_rows(st).iter().any(|m| m.r#ref == r)
}

/// The banner's "Master is red" lines (red, like urgent alerts), above the alerts.
pub fn banner(m: &MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Vec<AnyElement> {
    master_rows(m.state())
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let accent = t.accent;
            div()
                .flex()
                .flex_none()
                .items_center()
                .flex_wrap()
                .gap_x(px(10.))
                .gap_y(px(6.))
                .px(px(40.))
                .py(px(7.))
                .text_size(px(13.))
                .bg(t.down_soft)
                .text_color(t.down)
                .border_b_1()
                .border_color(t.down_line)
                .child(kit::dot(t.down, 8.))
                .child(div().flex_1().min_w_0().flex().items_baseline().gap(px(4.)).font_weight(FontWeight::SEMIBOLD).child(r.text.clone()).child(
                    div().ml(px(4.)).text_size(px(10.8)).font_weight(FontWeight::NORMAL).text_color(t.muted).child(r.ago.clone()),
                ))
                .children(r.task.clone().map(|task| {
                    div()
                        .id(("master-open", i))
                        .flex()
                        .flex_none()
                        .items_center()
                        .h(px(32.))
                        .px(px(10.))
                        .rounded(px(7.))
                        .text_size(px(12.5))
                        .cursor_pointer()
                        .bg(t.accent_soft)
                        .text_color(accent)
                        .font_weight(FontWeight::SEMIBOLD)
                        .hover(|s| s.opacity(0.9))
                        .child(format!("Open {task}"))
                        .on_click(cx.listener(move |m, _, _, cx| m.open_task(task.clone(), cx)))
                }))
                .into_any_element()
        })
        .collect()
}

/// The status-bar pills for the PR watch.
pub fn pills(st: &Value, t: &Theme) -> Vec<AnyElement> {
    let mut out = vec![];
    if let Some(p) = st.get("pr_builds").and_then(builds_pill) {
        out.push(pill_shell(t, "builds-pill", t.warn_fg, t.warn_soft).child(kit::dot(t.warn, 7.)).child(p.label).tooltip(kit::tip(p.title)).into_any_element());
    }
    if let Some(p) = st.get("pr_feed").and_then(feed_pill) {
        out.push(pill_shell(t, "feed-pill", t.warn_fg, t.warn_soft).child(kit::dot(t.warn, 7.)).child(p.label).tooltip(kit::tip(p.title)).into_any_element());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[::core::prelude::v1::test]
    fn the_feed_pill_shows_only_while_the_feed_is_unhealthy() {
        assert_eq!(feed_pill(&json!({"on": false, "healthy": false})), None);
        assert_eq!(feed_pill(&json!({"on": true, "healthy": true})), None);
        let p = feed_pill(&json!({"on": true, "healthy": false, "problem": "stuck", "why": "the PR feed hasn't sent a heartbeat for 4 minutes",
                                   "holding": "Holding reviewer asks, nudges and swaps: …"}))
        .unwrap();
        assert_eq!(p.label, "PR feed stuck");
        assert!(p.title.starts_with("The PR feed hasn't sent a heartbeat"));
        assert!(p.title.contains("Holding reviewer asks"));
    }

    #[::core::prelude::v1::test]
    fn master_is_red_once_for_each_open_break_that_is_the_owner_s() {
        assert!(master_rows(&json!({})).is_empty());
        let st = json!({"master": [
            {"ref": "M3", "project": "webapp", "branch": "main", "checks": ["build"], "verdict": "ours", "task": {"ref": "T12"}, "opened_at": "2026-10-08T10:00:00Z"},
            {"ref": "M4", "project": "api", "branch": "master", "checks": ["lint", "test"], "verdict": "unsure", "task": null},
            {"ref": "M5", "project": "ios", "branch": "master", "checks": ["lint", "test"], "verdict": "ours", "task": null}]});
        let rows = master_rows(&st);
        assert_eq!(rows.len(), 2, "not the unsure one");
        assert_eq!(rows[0].text, "Master is red on webapp: build fails on main. Yours to fix.");
        assert_eq!(rows[0].task.as_deref(), Some("T12"));
        assert_eq!(rows[1].text, "Master is red on ios: lint, test fail on master. Yours to fix.");
        assert_eq!(rows[1].task, None);
        assert!(shown_as_master(&st, &json!({"key": "master:M3", "urgent": true})), "its urgent alert isn't a second line");
        assert!(!shown_as_master(&st, &json!({"key": "master:M4", "urgent": true})));
        assert!(!shown_as_master(&st, &json!({"key": "qa:3"})));
    }

    #[::core::prelude::v1::test]
    fn the_builds_pill_says_who_stopped_them() {
        assert_eq!(builds_pill(&json!({"stopped": false})), None);
        let p = builds_pill(&json!({"stopped": true, "by": "Sam", "reason": "CI minutes ran out"})).unwrap();
        assert_eq!(p.label, "PR builds stopped");
        assert_eq!(p.title, "Stopped by Sam: CI minutes ran out");
    }
}
