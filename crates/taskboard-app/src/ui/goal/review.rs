//! The goal page's Plan review: what the latest review of the goal's plan against its outcome found
//! (`plan_review` of `GET /goals/:id`). One line per finding, its detail on a click; nothing at all
//! when the review found nothing, was skipped, failed or is still running. Read-only: the review is
//! run again with `tb goal review G<n> --again`.
use super::*;

/// The section's rows scroll past this height, so a long review leaves the waves in view.
const LIST_MAX_H: f32 = 196.;
/// The daemon clips a finding's detail well under this; it only bounds the markdown parse.
const DETAIL_MAX: usize = 2_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    /// What kind of miss: "Outcome not proven", "Needs the owner", …
    pub kind: String,
    pub short: String,
    pub detail: String,
    /// The tasks and other refs it names: "T4, G7".
    pub about: String,
}

fn kind_label(kind: &str) -> String {
    match kind {
        "outcome_unproven" => "Outcome not proven",
        "owner_prereq" => "Needs the owner",
        "past_decision" => "Past an open decision",
        "overlap" => "Overlaps another goal",
        "lesson_ignored" => "Goes against a lesson",
        other => other,
    }
    .to_string()
}

/// The latest review's findings, in the reviewer's order.
pub fn findings(g: &Value) -> Vec<Finding> {
    let r = &g["plan_review"];
    if s(r, "state") != "done" {
        return vec![];
    }
    arr(r, "findings")
        .iter()
        .filter(|f| !s(f, "short").trim().is_empty())
        .map(|f| Finding {
            kind: kind_label(s(f, "kind")),
            short: s(f, "short").trim().to_string(),
            detail: s(f, "detail").trim().to_string(),
            about: ["tasks", "refs"].iter().flat_map(|k| arr(f, k).iter().filter_map(Value::as_str)).collect::<Vec<_>>().join(", "),
        })
        .collect()
}

/// The section under the header, or None when there's nothing to show.
pub(super) fn section(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Option<Div> {
    let found = findings(g);
    if found.is_empty() {
        return None;
    }
    let when = fmt::hhmm(s(&g["plan_review"], "finished_at"));
    let head = side_head(t, "Plan review", Some(fmt::plural(found.len() as i64, "finding", "findings")))
        .child(div().flex_1())
        .when(!when.is_empty(), |d| d.child(div().font_weight(FontWeight::NORMAL).child(when)));
    let rows = found.into_iter().enumerate().fold(div().flex().flex_col().gap(px(2.)), |list, (ix, f)| {
        let open = !f.detail.is_empty() && m.goal_page.review_open.contains(&ix);
        list.child(row(t, ix, f, open, cx))
    });
    Some(
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(18.))
            .py(px(12.))
            .rounded(px(12.))
            .bg(t.card)
            .border_1()
            .border_color(t.border)
            .child(head)
            .child(div().id("goal-review-list").max_h(px(LIST_MAX_H)).overflow_y_scroll().child(rows)),
    )
}

/// One finding: its kind, its line and what it names; a click opens or folds its detail.
fn row(t: &Theme, ix: usize, f: Finding, open: bool, cx: &mut Context<MainWindow>) -> Div {
    let has_detail = !f.detail.is_empty();
    let chev = div().flex_none().size(px(14.)).when(has_detail, |d| d.child(kit::icon(if open { kit::Icon::Chev } else { kit::Icon::ChevRight }, 14., t.muted)));
    let hover = t.text;
    let line = div()
        .id(SharedString::from(format!("goal-review-{ix}")))
        .flex()
        .items_center()
        .gap(px(8.))
        .py(px(2.))
        .text_size(px(13.))
        .child(chev)
        .child(div().flex_none().whitespace_nowrap().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(t.warn_text).child(f.kind))
        .child(div().flex_1().min_w_0().truncate().text_color(t.text_2).child(f.short.clone()))
        .when(!f.about.is_empty(), |d| d.child(div().flex_none().font_family(t.mono_font.clone()).text_size(px(11.)).text_color(t.muted).child(f.about)))
        .when(has_detail, |d| {
            d.cursor_pointer().hover(move |s| s.text_color(hover)).on_click(cx.listener(move |m, _, _, cx| {
                let o = &mut m.goal_page.review_open;
                if !o.remove(&ix) {
                    o.insert(ix);
                }
                cx.notify();
            }))
        })
        .when(!has_detail, |d| d.tooltip(kit::tip(f.short)));
    div().flex().flex_col().child(line).when(open, |d| {
        d.child(
            div()
                .pl(px(22.))
                .pb(px(6.))
                .text_size(px(13.))
                .text_color(t.text_2)
                .child(crate::ui::md::render(t, &f.detail, DETAIL_MAX, &format!("goal-review-detail-{ix}"))),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // An explicit import beats the glob: `#[gpui_kit::test]` expands to `#[test]`, which must be std's.
    use ::core::prelude::v1::test;

    #[test]
    fn only_a_finished_review_with_findings_has_lines() {
        let g = json!({"plan_review": {"state": "done", "findings": [
            {"kind": "owner_prereq", "short": " No task for the store sign-in ", "detail": "Add a human task first.", "tasks": ["T4"], "refs": ["G7"]},
            {"kind": "overlap", "short": "", "detail": "blank lines are dropped"},
            {"kind": "something_new", "short": "Unknown kinds show as they are", "detail": ""},
        ]}});
        assert_eq!(findings(&g), vec![
            Finding { kind: "Needs the owner".into(), short: "No task for the store sign-in".into(), detail: "Add a human task first.".into(), about: "T4, G7".into() },
            Finding { kind: "something_new".into(), short: "Unknown kinds show as they are".into(), detail: String::new(), about: String::new() },
        ]);
        for state in ["running", "skipped", "failed"] {
            let mut g = g.clone();
            g["plan_review"]["state"] = json!(state);
            assert!(findings(&g).is_empty(), "{state}");
        }
        assert!(findings(&json!({"plan_review": null})).is_empty());
        assert!(findings(&json!({"plan_review": {"state": "done", "findings": []}})).is_empty());
    }

    /// The goal's own answer from the daemon carries the latest review, and the page reads its
    /// findings from there; a later review that's still running hides the section again.
    #[gpui_kit::test]
    fn the_goal_answer_feeds_the_section(cx: &mut gpui_kit::TestAppContext) {
        use crate::parity;
        let (w, rec) = parity::window(cx);
        let db = &rec.board().app().db;
        let now = "2026-11-03T15:00:00Z";
        let found = json!([{"kind": "overlap", "short": "Same screen as G2", "detail": "Both change `auth.ts`.", "tasks": ["T2"], "refs": ["G2"]}]);
        db.insert("plan_reviews", taskboardd::fields!["goal_id" => 1, "started_at" => now, "finished_at" => now, "findings" => found.to_string()]).unwrap();
        w.update(cx, |m, _, cx| super::open(m, "G1", false, cx)).unwrap();
        parity::settle(cx);
        let shown = |cx: &mut gpui_kit::TestAppContext| w.update(cx, |m, _, _| m.data.goal.as_deref().map(findings).unwrap_or_default()).unwrap();
        assert_eq!(shown(cx), vec![Finding { kind: "Overlaps another goal".into(), short: "Same screen as G2".into(), detail: "Both change `auth.ts`.".into(), about: "T2, G2".into() }]);
        assert!(w.update(cx, |m, _, _| m.goal_page.review_open.is_empty()).unwrap(), "details start folded");

        db.insert("plan_reviews", taskboardd::fields!["goal_id" => 1, "started_at" => now]).unwrap();
        w.update(cx, |m, _, cx| m.refresh(cx)).unwrap();
        parity::settle(cx);
        assert!(shown(cx).is_empty());
    }
}
