//! The statuses every list shows (Needs you, Working, Queued, Stopped, In review, Pending merge,
//! Done, Failed, and Planned for work nothing runs yet): one chip color each, the same everywhere. The detail a
//! task's old pill carried ("Question", "Checks failed", "Starting"…) goes on the row's line.
// The Needs you, Goals, Tasks and goal pages draw their rows with it.
#![allow(dead_code)]
use crate::fmt::{self, b, opt_s, s};
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
    /// Its terminal was closed by hand: nothing failed and nothing will run it until it's restarted.
    Stopped,
    Review,
    /// Approved and green, held behind the PR it stacks on (a tail merge).
    Pending,
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
            St::Stopped => "Stopped",
            St::Review => "In review",
            St::Pending => "Pending merge",
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
            St::Pending => 4,
            St::Stopped => 5,
            St::Queued => 6,
            St::Planned => 7,
            St::Done => 8,
        }
    }

    /// (text, background) for the chip; the solid color for bars and dots.
    pub fn colors(self, t: &Theme) -> (Hsla, Hsla, Hsla) {
        match self {
            St::Needs => (t.warn_fg, t.warn_soft, t.warn),
            St::Working => (t.accent_fg, t.accent_soft, t.accent),
            St::Queued | St::Stopped => (t.text_2, t.seg, t.border_2),
            St::Review => (t.results, t.results_soft, t.results),
            St::Pending => (t.up_fg, gpui_kit::transparent_black(), t.up),
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
        "Stopped" => (St::Stopped, format!("Stopped · {}", tile.line), false, short(fmt::ago(when))),
        "Looks finished" => (St::Needs, format!("Looks finished · {}", tile.line), true, short(fmt::ago(when))),
        "Couldn’t start" | "Couldn’t compact" => (St::Failed, format!("{} · {}", tile.status, tile.line), true, short(fmt::ago(when))),
        "Question" => (St::Needs, format!("Question: {}", opt_s(c, "question").unwrap_or("")), true, short(fmt::ago(when))),
        "Needs you" | "Needs setup" if s(c, "status") == "needs" => (St::Needs, opt_s(c, "latest").unwrap_or(&tile.line).to_string(), true, short(fmt::ago(when))),
        "Needs you" | "Needs setup" => (St::Needs, tile.line, true, String::new()),
        "Working" => (St::Working, opt_s(c, "latest").unwrap_or("").to_string(), false, run),
        "Maybe stuck" => (St::Working, format!("Maybe stuck · {}", tile.line), true, run),
        "Compacting" => (St::Working, tile.line, false, run),
        "Starting" => (St::Working, format!("Starting · {}", tile.line), false, String::new()),
        "Queued" => (St::Queued, opt_s(c, "waiting").map(str::to_string).unwrap_or_default(), false, String::new()),
        "Checks failed" | "Conflicts" => (St::Review, format!("{} · {}", tile.status, tile.line), true, String::new()),
        "Checks running" | "Comments" | "Ready to merge" => (St::Review, format!("{} · {}", tile.status, tile.line), false, String::new()),
        "In review" => (St::Review, tile.line, false, String::new()),
        "Pending merge" => (St::Pending, tile.line, false, String::new()),
        "Failed" => (St::Failed, opt_s(c, "summary").unwrap_or("").to_string(), false, fmt::hhmm(when)),
        "Done" if !crate::ui::goal::awaiting_merge(c) => (St::Done, opt_s(c, "summary").map(str::to_string).unwrap_or_default(), false, fmt::hhmm(when)),
        // A label this match doesn't know yet goes by the task itself, never to Done: a done task
        // with its PR still open is in review until the PR merges.
        _ => (by_task(c), format!("{} · {}", tile.status, tile.line), false, String::new()),
    };
    // A done row says how its PR ended: merged, or declined while its waiters still wait on it,
    // unless its work landed on the main branch another way.
    let pr_num = c["pr"]["num"].as_i64();
    let landed = opt_s(c, "landed").filter(|l| !l.is_empty());
    let declined = c["pr"]["state"].as_str().is_some_and(|st| !st.eq_ignore_ascii_case("MERGED") && !st.eq_ignore_ascii_case("OPEN"));
    let line = match (st == St::Done, pr_num, landed) {
        (true, Some(_), Some(sha)) if declined => format!("Landed in {}", &sha[..sha.len().min(7)]),
        (true, Some(n), None) if declined => format!("PR #{n} declined"),
        (true, Some(n), _) if line.is_empty() => format!("PR #{n} merged"),
        _ => line,
    };
    Row { st, line, loud, age }
}

/// The status of a task whose tile label `row` doesn't know, from its own fields.
fn by_task(c: &Value) -> St {
    match s(c, "status") {
        "done" if b(c, "failed") => St::Failed,
        "done" if crate::ui::goal::awaiting_merge(c) => St::Review,
        "done" => St::Done,
        "working" => St::Working,
        "queued" => St::Queued,
        _ => St::Needs,
    }
}

/// The chip's width: fits "Pending merge", the longest label.
pub const CHIP_W: f32 = 104.;

/// The status chip, a fixed width so rows line up.
pub fn chip(t: &Theme, st: St) -> Div {
    let (fg, bg, line) = st.colors(t);
    kit::pill(fg, bg, st.label())
        .w(px(CHIP_W))
        .justify_center()
        .when(matches!(st, St::Planned | St::Pending), |d| d.border_1().border_color(line))
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
        let fill = match st {
            St::Queued | St::Planned => t.border_2,
            St::Pending => solid.opacity(0.5),
            _ => solid,
        };
        let mut seg = div().h_full().bg(fill);
        seg.style().flex_grow = Some(n as f32);
        seg.style().flex_shrink = Some(1.);
        seg.style().flex_basis = Some(relative(0.).into());
        d = d.child(seg);
    }
    d
}

/// Puts a failure in plain words. The board's own sentences pass through; raw text (JSON, error
/// codes, "os error", a Rust or HTTP message) becomes a plain line saying it didn't work.
pub fn plain_error(raw: &str) -> String {
    let t = raw.trim();
    let lower = t.to_ascii_lowercase();
    let raw_looking = t.is_empty()
        || t.starts_with('{')
        || t.starts_with('[')
        || ["os error", "errno", "panicked", "stack", "json", "expected value", "connection refused", "timed out", "broken pipe", "unexpected", "::", "http "]
            .iter()
            .any(|w| lower.contains(w));
    if lower.contains("can’t reach") || lower.contains("can't reach") || lower.contains("connection refused") {
        return "The board isn’t answering. Check that it’s running, then try again.".into();
    }
    if raw_looking {
        return "That didn’t work. Try again in a moment.".into();
    }
    // The board answered "The board answered 500." for a failure it gave no sentence for.
    if lower.starts_with("the board answered") {
        return "That didn’t work. The board hit a problem.".into();
    }
    let mut out = t.to_string();
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// Tells VoiceOver (it speaks it without moving focus). A no-op off macOS and in tests.
#[cfg(all(target_os = "macos", not(test)))]
pub fn announce(text: &str) {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSDictionary, NSString};
    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {
        fn NSAccessibilityPostNotificationWithUserInfo(element: *mut AnyObject, notification: &NSString, info: &NSDictionary<NSString, AnyObject>);
    }
    unsafe {
        let app: Option<Retained<AnyObject>> = msg_send![class!(NSApplication), sharedApplication];
        let Some(app) = app else { return };
        let (note, key) = (NSString::from_str("AXAnnouncementRequested"), NSString::from_str("AXAnnouncement"));
        let msg = NSString::from_str(text);
        let info: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_retained_objects(&[&*key], &[Retained::cast_unchecked::<AnyObject>(msg)]);
        NSAccessibilityPostNotificationWithUserInfo(Retained::as_ptr(&app) as *mut AnyObject, &note, &info);
    }
}
#[cfg(not(all(target_os = "macos", not(test))))]
pub fn announce(_text: &str) {}

/// `.toast`: centred 24px above the bottom, 9px 16px, radius 9, 13px, `--shadow-modal`. Errors carry a
/// Dismiss button and stay; the rest go on their own.
pub fn toasts(m: &crate::app::MainWindow, t: &Theme, cx: &mut Context<crate::app::MainWindow>) -> Option<AnyElement> {
    if m.toasts.is_empty() {
        return None;
    }
    let mut col = div().absolute().bottom(px(24.)).left_0().right_0().px(px(16.)).flex().flex_col().items_center().gap(px(6.));
    for (i, x) in m.toasts.iter().enumerate() {
        let at = x.at;
        col = col.child(
            div()
                .id(("toast", i))
                .px(px(16.))
                .py(px(9.))
                .rounded(px(9.))
                .shadow(kit::shadow_modal(t))
                .text_size(px(13.))
                .bg(if x.err { t.down } else { t.text })
                .text_color(if x.err { gpui_kit::white() } else { t.bg })
                .flex()
                .items_center()
                .gap(px(14.))
                .child(x.text.clone())
                .children(x.undo.clone().map(|path| {
                    div()
                        .id("toast-undo")
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.accent_soft)
                        .cursor_pointer()
                        .child("Undo")
                        .on_click(cx.listener(move |m, _, _, cx| {
                            m.dismiss_toast(at, cx);
                            m.post(path.clone(), serde_json::json!({}), cx, |_, _, _| {});
                        }))
                }))
                .children(m.backlog.open_toast.clone().filter(|(when, _)| *when == at).map(|(_, task)| {
                    div()
                        .id("toast-open")
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.accent_soft)
                        .cursor_pointer()
                        .child("Open it")
                        .on_click(cx.listener(move |m, _, _, cx| {
                            m.dismiss_toast(at, cx);
                            m.open_task(task.clone(), cx);
                        }))
                }))
                .children((m.idea_toast == Some(at)).then(|| {
                    div()
                        .id("toast-reopen")
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.accent_soft)
                        .cursor_pointer()
                        .child("Reopen ⌘N")
                        .on_click(cx.listener(move |m, _, window, cx| {
                            m.dismiss_toast(at, cx);
                            m.new_idea(window, cx);
                        }))
                }))
                .children(x.err.then(|| {
                    div()
                        .id(("toast-dismiss", i))
                        .font_weight(FontWeight::SEMIBOLD)
                        .cursor_pointer()
                        .child("Dismiss")
                        .on_click(cx.listener(move |m, _, _, cx| m.dismiss_toast(at, cx)))
                })),
        );
    }
    Some(deferred(col).with_priority(4).into_any_element())
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
        // Lost with signs it shipped still waits on the user, never reads as done.
        let found = json!([{"what": "Tag v1.2.3 pushed", "at": "2026-10-15T16:21:00Z"}]);
        let looks = row(&card(json!({"status": "needs", "lost": true, "looks_finished": found})));
        assert_eq!((looks.st, looks.loud), (St::Needs, true));
        assert!(looks.line.starts_with("Looks finished · Tag v1.2.3 pushed"), "{}", looks.line);
        assert_eq!(row(&card(json!({"latest": "Running tests"}))).line, "Running tests");
        let queued = row(&card(json!({"status": "queued", "waiting": "Waits for T4 to finish"})));
        assert_eq!((queued.st, queued.line.as_str()), (St::Queued, "Waits for T4 to finish"));
        assert_eq!(row(&card(json!({"status": "planned"}))).st, St::Planned);
        let fix = row(&card(json!({"status": "done", "pr": {"num": 4, "state": "OPEN", "stage": {"phase": "fix"}}})));
        assert_eq!((fix.st, fix.line.as_str()), (St::Review, "Checks failed · PR #4"));
        let merged = row(&card(json!({"status": "done", "pr": {"num": 4, "state": "MERGED"}})));
        assert_eq!((merged.st, merged.line.as_str()), (St::Done, "PR #4 merged"));
        // A declined PR never reads as merged, even over the summary; once its work landed another
        // way the row says where.
        let declined = row(&card(json!({"status": "done", "summary": "Fixed it", "pr": {"num": 4, "state": "DECLINED"}})));
        assert_eq!((declined.st, declined.line.as_str()), (St::Done, "PR #4 declined"));
        let landed = row(&card(json!({"status": "done", "landed": "0123456789abcdef", "pr": {"num": 4, "state": "DECLINED"}})));
        assert_eq!((landed.st, landed.line.as_str()), (St::Done, "Landed in 0123456"));
        assert_eq!(row(&card(json!({"status": "done", "failed": true}))).st, St::Failed);
    }

    #[test]
    fn an_open_pr_with_conflicts_is_in_review_and_a_stuck_task_is_still_working() {
        let pr = json!({"num": 50, "state": "OPEN", "stage": {"phase": "rebase", "label": "Rebasing"}});
        let conflicts = row(&card(json!({"status": "done", "pr": pr})));
        assert_eq!((conflicts.st, conflicts.line.as_str(), conflicts.loud), (St::Review, "Conflicts · PR #50 · Rebasing", true));
        let stuck = row(&card(json!({"stuck": {"since": "2026-10-27T10:00:00Z"}})));
        assert_eq!((stuck.st, stuck.loud), (St::Working, true));
    }

    #[test]
    fn a_tile_label_it_doesnt_know_goes_by_the_task_never_to_done() {
        let open = card(json!({"status": "done", "pr": {"num": 7, "state": "OPEN"}}));
        assert_eq!(by_task(&open), St::Review);
        assert_eq!(by_task(&card(json!({"status": "done", "pr": {"num": 7, "state": "MERGED"}}))), St::Done);
        assert_eq!(by_task(&card(json!({"status": "done", "failed": true, "pr": {"num": 7, "state": "OPEN"}}))), St::Failed);
        assert_eq!(by_task(&card(json!({"status": "working"}))), St::Working);
        assert_eq!(by_task(&card(json!({"status": "needs"}))), St::Needs);
        // Every phase the daemon can send on an open PR, known or new, stays in review.
        for phase in ["checks", "fix", "rebase", "comments", "merge", "review", "rereview", "steps", "stack", "ask", "something-new"] {
            let c = card(json!({"status": "done", "pr": {"num": 7, "state": "OPEN", "stage": {"phase": phase}}}));
            assert_eq!(row(&c).st, St::Review, "phase {phase}");
        }
    }

    #[test]
    fn an_approved_pr_held_behind_its_base_is_pending_merge() {
        let bar = json!({"waits_on_base": {"ref": "T212", "line": "T212 waiting on its own base"}});
        let c = card(json!({"status": "done", "pr": {"num": 59, "state": "OPEN", "stage": {"phase": "waits", "label": "Waits on base"}, "bar": bar}}));
        let r = row(&c);
        assert_eq!((r.st, r.line.as_str()), (St::Pending, "PR #59 · after T212"));
        let no_base = card(json!({"status": "done", "pr": {"num": 59, "state": "OPEN", "stage": {"phase": "waits"}}}));
        assert_eq!(row(&no_base).line, "PR #59");
        assert!(St::Pending.open());
    }

    #[test]
    fn a_closed_terminal_is_a_quiet_stopped_row_not_a_failure() {
        let closed = row(&card(json!({"status": "needs", "lost": true, "closed_by_hand": true})));
        assert_eq!((closed.st, closed.loud), (St::Stopped, false));
        assert!(closed.line.starts_with("Stopped · "), "{}", closed.line);
    }

    #[::core::prelude::v1::test]
    fn errors_are_put_in_plain_words() {
        assert_eq!(plain_error("Can’t reach the task board server."), "The board isn’t answering. Check that it’s running, then try again.");
        assert_eq!(plain_error("{\"error\":\"boom\"}"), "That didn’t work. Try again in a moment.");
        assert_eq!(plain_error("No such file (os error 2)"), "That didn’t work. Try again in a moment.");
        assert_eq!(plain_error("The board answered 500."), "That didn’t work. The board hit a problem.");
        assert_eq!(plain_error("T3 has no terminal to stop"), "T3 has no terminal to stop.");
        assert_eq!(plain_error("Midna isn't running, so the board can't open a terminal."), "Midna isn't running, so the board can't open a terminal.");
    }
}
