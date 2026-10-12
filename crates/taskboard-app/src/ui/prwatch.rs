//! The board's PR watch in the window's chrome: the status-bar pills of an unhealthy PR feed
//! (`state.pr_feed`), of the work event stream's sources (`state.stream`, read-only: modes change
//! with `tb stream mode`) and of stopped PR builds (`state.pr_builds`), and the "Master is red" banner
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

/// One source's row in the Streams card: what `tb stream` prints, in words.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamRow {
    pub name: &'static str,
    /// "Board" or "Stream".
    pub mode: &'static str,
    /// Healthy, Stuck, Silent or Settling, while its stream is watched.
    pub health: Option<String>,
    /// The health's tone: up, warn or down.
    pub tone: &'static str,
    /// "3:05 PM" today, else "Oct 7, 3:05 PM".
    pub last_event: Option<String>,
    /// "Polling", "Poll resting", or none for Jira (it has no poll).
    pub poll: Option<&'static str>,
}

/// The Streams pill: shown while any source's stream is watched (stream mode, or `[feed] on` for
/// PRs). Its dot is the worst source's tone; hovering lists every source.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamsView {
    pub tone: &'static str,
    pub rows: Vec<StreamRow>,
}

pub fn streams_view(v: &Value) -> Option<StreamsView> {
    let sources = arr(v, "sources");
    if !sources.iter().any(|x| x["watched"] == true) {
        return None;
    }
    let rows: Vec<StreamRow> = sources
        .iter()
        .map(|x| {
            let (health, tone) = match (x["watched"] == true, fmt::opt_s(x, "problem")) {
                (false, _) => (None, "up"),
                (true, Some(p)) => (Some(fmt::cap(p)), "down"),
                (true, None) if x["settling_secs"].is_number() => (Some("Settling".into()), "warn"),
                (true, None) => (Some("Healthy".into()), "up"),
            };
            let name = s(x, "source");
            StreamRow {
                name: match name {
                    "prs" => "PRs",
                    "builds" => "Builds",
                    "jira" => "Jira",
                    _ => "QA",
                },
                mode: if s(x, "mode") == "stream" { "Stream" } else { "Board" },
                health,
                tone,
                last_event: fmt::opt_s(x, "last_event_at").map(|t| fmt::log_time(t).replace('\n', ", ")),
                poll: (name != "jira").then_some(if x["polling"] == false { "Poll resting" } else { "Polling" }),
            }
        })
        .collect();
    let tone = ["down", "warn"].into_iter().find(|t| rows.iter().any(|r| r.tone == *t)).unwrap_or("up");
    Some(StreamsView { tone, rows })
}

fn tone_color(t: &Theme, tone: &str) -> Hsla {
    match tone {
        "down" => t.down,
        "warn" => t.warn,
        _ => t.up,
    }
}

/// The hover card: one row per source, in the tooltip's inverted colours.
struct StreamsCard {
    rows: Vec<StreamRow>,
}

impl Render for StreamsCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<Theme>();
        let cell = |w: f32| div().w(px(w)).flex_none().flex().items_center().gap(px(5.));
        let mut card =
            div().flex().flex_col().gap(px(3.)).px(px(10.)).py(px(7.)).rounded(px(6.)).bg(t.text).text_color(t.bg).text_size(px(12.)).font_family(t.ui_font.clone());
        for r in &self.rows {
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .child(cell(56.).font_weight(FontWeight::SEMIBOLD).child(r.name))
                    .child(cell(56.).child(r.mode))
                    .child(cell(80.).children(r.health.clone().map(|h| [kit::dot(tone_color(t, r.tone), 6.).into_any_element(), h.into_any_element()]).into_iter().flatten()))
                    .child(cell(120.).children(r.last_event.clone()))
                    .child(cell(84.).children(r.poll)),
            );
        }
        card
    }
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
    if let Some(v) = st.get("stream").and_then(streams_view) {
        let rows = v.rows;
        out.push(
            pill_shell(t, "streams-pill", t.muted, t.seg)
                .child(kit::dot(tone_color(t, v.tone), 7.))
                .child("Streams")
                .tooltip(move |_, cx| cx.new(|_| StreamsCard { rows: rows.clone() }).into())
                .into_any_element(),
        );
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
    fn the_streams_pill_lists_each_source_while_any_stream_is_watched() {
        let src = |name: &str, extra: Value| {
            let mut x = json!({"source": name, "mode": "board", "watched": false, "problem": null, "polling": true, "settling_secs": null, "last_event_at": null});
            for (k, v) in extra.as_object().unwrap() {
                x[k] = v.clone();
            }
            x
        };
        let board = json!({"sources": [src("prs", json!({})), src("builds", json!({})), src("jira", json!({})), src("qa", json!({}))]});
        assert_eq!(streams_view(&board), None, "nothing watched: no pill");

        let at = fmt::log_time("2026-10-12T15:04:00Z").replace('\n', ", ");
        let v = streams_view(&json!({"sources": [
            src("prs", json!({"mode": "stream", "watched": true, "polling": false, "last_event_at": "2026-10-12T15:04:00Z"})),
            src("builds", json!({"mode": "stream", "watched": true, "settling_secs": 40})),
            src("jira", json!({"mode": "stream", "watched": true, "polling": false, "problem": "silent"})),
            src("qa", json!({}))]}))
        .unwrap();
        assert_eq!(v.tone, "down", "the worst source colours the dot");
        let r = &v.rows;
        assert_eq!((r[0].name, r[0].mode, r[0].health.as_deref(), r[0].poll), ("PRs", "Stream", Some("Healthy"), Some("Poll resting")));
        assert_eq!(r[0].last_event.as_deref(), Some(at.as_str()));
        assert!(at.contains("AM") || at.contains("PM"), "12-hour: {at}");
        assert_eq!((r[1].health.as_deref(), r[1].tone, r[1].poll), (Some("Settling"), "warn", Some("Polling")));
        assert_eq!((r[2].name, r[2].health.as_deref(), r[2].poll), ("Jira", Some("Silent"), None), "Jira has no poll");
        assert_eq!((r[3].name, r[3].mode, r[3].health.as_deref(), r[3].last_event.as_deref()), ("QA", "Board", None, None));
    }

    #[::core::prelude::v1::test]
    fn the_builds_pill_says_who_stopped_them() {
        assert_eq!(builds_pill(&json!({"stopped": false})), None);
        let p = builds_pill(&json!({"stopped": true, "by": "Sam", "reason": "CI minutes ran out"})).unwrap();
        assert_eq!(p.label, "PR builds stopped");
        assert_eq!(p.title, "Stopped by Sam: CI minutes ran out");
    }
}
