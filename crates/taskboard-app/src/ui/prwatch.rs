//! The board's PR watch in the window's chrome: the status-bar pill of an unhealthy PR feed
//! (`state.pr_feed`).

use crate::app::{Pill, pill_shell};
use crate::fmt::{self, s};
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

/// The status-bar pills for the PR watch.
pub fn pills(st: &Value, t: &Theme) -> Vec<AnyElement> {
    let mut out = vec![];
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
}
