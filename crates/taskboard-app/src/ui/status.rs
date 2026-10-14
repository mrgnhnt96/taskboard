//! The six statuses every list shows (Needs you, Working, Queued, In review, Done, Failed, and
//! Planned for work nothing runs yet): one chip color each, the same everywhere. The detail a
//! task's old pill carried ("Question", "Checks failed", "Starting"…) goes on the row's line.
// The Needs you, Goals, Tasks and goal pages draw their rows with it.
#![allow(dead_code)]
use crate::fmt::{self, opt_s, s};
use crate::theme::Theme;
use crate::ui::home::tile_vm;
use crate::ui::kit;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum St {
    Needs,
    Working,
    Queued,
    Review,
    Done,
    Failed,
    Planned,
}

impl St {
    pub fn label(self) -> &'static str {
        match self {
            St::Needs => "Needs you",
            St::Working => "Working",
            St::Queued => "Queued",
            St::Review => "In review",
            St::Done => "Done",
            St::Failed => "Failed",
            St::Planned => "Planned",
        }
    }

    /// The order lists group them in: what needs you first, finished work last.
    pub fn rank(self) -> u8 {
        match self {
            St::Needs => 0,
            St::Failed => 1,
            St::Working => 2,
            St::Review => 3,
            St::Queued => 4,
            St::Planned => 5,
            St::Done => 6,
        }
    }

    /// (text, background) for the chip; the solid color for bars and dots.
    pub fn colors(self, t: &Theme) -> (Hsla, Hsla, Hsla) {
        match self {
            St::Needs => (t.warn_fg, t.warn_soft, t.warn),
            St::Working => (t.accent_fg, t.accent_soft, t.accent),
            St::Queued => (t.text_2, t.seg, t.border_2),
            St::Review => (t.results, t.results_soft, t.results),
            St::Done => (t.up_fg, t.up_soft, t.up),
            St::Failed => (t.down, t.down_soft, t.down),
            St::Planned => (t.muted, gpui_kit::transparent_black(), t.border),
        }
    }

    /// Whether the work is still open (not done or failed).
    pub fn open(self) -> bool {
        !matches!(self, St::Done | St::Failed)
    }
}

/// What one task row says: its status, a line on what it's doing, and how long.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub st: St,
    pub line: String,
    /// The line in the status's color (a question, a failed check) rather than muted.
    pub loud: bool,
    /// "38m" running, "12m" since it asked, "2:05 PM" when it finished.
    pub age: String,
}

fn short(a: String) -> String {
    if a == "just now" { "now".into() } else { a.trim_end_matches(" ago").to_string() }
}

/// A task card's row.
pub fn row(c: &Value) -> Row {
    let when = opt_s(c, "when").or(opt_s(c, "updated_at")).unwrap_or("");
    if s(c, "status") == "planned" {
        return Row { st: St::Planned, line: String::new(), loud: false, age: String::new() };
    }
    let tile = tile_vm(c);
    let run = fmt::running_for(c).trim_start_matches("running ").to_string();
    let (st, line, loud, age) = match tile.status.as_str() {
        "Lost" => (St::Failed, format!("Terminal lost · {}", tile.line), true, short(fmt::ago(when))),
        "Couldn’t start" | "Couldn’t compact" => (St::Failed, format!("{} · {}", tile.status, tile.line), true, short(fmt::ago(when))),
        "Question" => (St::Needs, format!("Question: {}", opt_s(c, "question").unwrap_or("")), true, short(fmt::ago(when))),
        "Needs you" | "Needs setup" if s(c, "status") == "needs" => (St::Needs, opt_s(c, "latest").unwrap_or(&tile.line).to_string(), true, short(fmt::ago(when))),
        "Needs you" | "Needs setup" => (St::Needs, tile.line, true, String::new()),
        "Working" => (St::Working, opt_s(c, "latest").unwrap_or("").to_string(), false, run),
        "Compacting" => (St::Working, tile.line, false, run),
        "Starting" => (St::Working, format!("Starting · {}", tile.line), false, String::new()),
        "Queued" => (St::Queued, opt_s(c, "waiting").map(str::to_string).unwrap_or_default(), false, String::new()),
        "Checks failed" => (St::Review, format!("Checks failed · {}", tile.line), true, String::new()),
        "Checks running" | "Comments" | "Ready to merge" => (St::Review, format!("{} · {}", tile.status, tile.line), false, String::new()),
        "In review" => (St::Review, tile.line, false, String::new()),
        "Failed" => (St::Failed, opt_s(c, "summary").unwrap_or("").to_string(), false, fmt::hhmm(when)),
        _ => (St::Done, opt_s(c, "summary").map(str::to_string).unwrap_or_default(), false, fmt::hhmm(when)),
    };
    // A merged PR says so on a done row.
    let line = if st == St::Done && line.is_empty() {
        c["pr"]["num"].as_i64().map(|n| format!("PR #{n} merged")).unwrap_or_default()
    } else {
        line
    };
    Row { st, line, loud, age }
}

/// The status chip, a fixed width so rows line up.
pub fn chip(t: &Theme, st: St) -> Div {
    let (fg, bg, line) = st.colors(t);
    kit::pill(fg, bg, st.label())
        .w(px(84.))
        .justify_center()
        .when(st == St::Planned, |d| d.border_1().border_color(line))
}

/// A bar split by status: (status, how many), in the order given.
pub fn bar(t: &Theme, parts: &[(St, i64)], h: f32) -> Div {
    let total: i64 = parts.iter().map(|(_, n)| n).sum();
    let mut d = div().flex().w_full().h(px(h)).rounded(px(h / 2.)).overflow_hidden().gap(px(2.)).bg(t.divider);
    if total == 0 {
        return d;
    }
    for (st, n) in parts.iter().copied().filter(|(_, n)| *n > 0) {
        let (_, _, solid) = st.colors(t);
        let mut seg = div().h_full().bg(if st == St::Queued || st == St::Planned { t.border_2 } else { solid });
        seg.style().flex_grow = Some(n as f32);
        seg.style().flex_shrink = Some(1.);
        seg.style().flex_basis = Some(relative(0.).into());
        d = d.child(seg);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;
    use serde_json::json;

    fn card(extra: Value) -> Value {
        let mut c = json!({"id": 1, "ref": "T1", "title": "x", "status": "working"});
        for (k, v) in extra.as_object().unwrap() {
            c[k] = v.clone();
        }
        c
    }

    #[test]
    fn six_statuses_with_the_detail_on_the_line() {
        let q = row(&card(json!({"status": "needs", "needs_reason": "question", "question": "10 or 30?"})));
        assert_eq!((q.st, q.line.as_str(), q.loud), (St::Needs, "Question: 10 or 30?", true));
        assert_eq!(row(&card(json!({"status": "needs", "lost": true}))).st, St::Failed);
        assert_eq!(row(&card(json!({"latest": "Running tests"}))).line, "Running tests");
        let queued = row(&card(json!({"status": "queued", "waiting": "Waits for T4 to finish"})));
        assert_eq!((queued.st, queued.line.as_str()), (St::Queued, "Waits for T4 to finish"));
        assert_eq!(row(&card(json!({"status": "planned"}))).st, St::Planned);
        let fix = row(&card(json!({"status": "done", "pr": {"num": 4, "state": "OPEN", "stage": {"phase": "fix"}}})));
        assert_eq!((fix.st, fix.line.as_str()), (St::Review, "Checks failed · PR #4"));
        let merged = row(&card(json!({"status": "done", "pr": {"num": 4, "state": "MERGED"}})));
        assert_eq!((merged.st, merged.line.as_str()), (St::Done, "PR #4 merged"));
        assert_eq!(row(&card(json!({"status": "done", "failed": true}))).st, St::Failed);
    }
}
