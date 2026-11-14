//! The name sort against the web board (`localeSort` cases in `parity/golden/forms.json`), and
//! the alerts dialog in a headless window.
use super::*;
// An explicit import beats the glob above, which would bring in GPUI's own `test` attribute
// (and `#[gpui_kit::test]` expands to a plain `#[test]`).
#[allow(unused_imports)]
use ::core::prelude::v1::test;
use crate::app::Page;
use crate::backend::Backend;
use crate::parity::{self, golden};
use gpui_kit::{TestAppContext, WindowHandle};
use serde_json::{Value, json};

#[test]
fn names_sort_like_the_web() {
    golden("forms").only("localeSort").check(|i| {
        let mut v: Vec<String> = i["list"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        v.sort_by(|a, b| locale_cmp(a, b));
        json!(v)
    });
}

type W = WindowHandle<MainWindow>;

/// Draw the window once (the dialog checks its alerts while it renders).
fn redraw(w: W, cx: &mut TestAppContext) {
    cx.update_window(w.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    })
    .unwrap();
    parity::settle(cx);
}

fn one_alert() -> Value {
    json!([{"id": "a1", "at": "2026-10-07T14:00:00Z", "text": "T3 couldn't start.", "task": "T3", "goal": null}])
}

#[gpui_kit::test]
fn escape_closes_the_alerts_dialog(cx: &mut TestAppContext) {
    let (w, _) = parity::window(cx);
    w.update(cx, |m, window, cx| {
        // An alert, so the dialog doesn't close by itself.
        std::rc::Rc::make_mut(m.data.state.as_mut().unwrap())["alerts"] = one_alert();
        m.set_modal(Some(Modal::Alerts), window, cx);
    })
    .unwrap();
    redraw(w, cx);
    w.update(cx, |m, _, _| assert!(matches!(m.modal, Some(Modal::Alerts)), "the dialog stays while there are alerts")).unwrap();
    cx.simulate_keystrokes(w.into(), "escape");
    parity::settle(cx);
    w.update(cx, |m, _, _| assert!(m.modal.is_none(), "esc closes the alerts dialog")).unwrap();
}

#[gpui_kit::test]
fn the_alerts_dialog_closes_itself_when_none_are_left(cx: &mut TestAppContext) {
    let (w, _) = parity::window(cx);
    w.update(cx, |m, window, cx| {
        std::rc::Rc::make_mut(m.data.state.as_mut().unwrap())["alerts"] = one_alert();
        m.set_modal(Some(Modal::Alerts), window, cx);
    })
    .unwrap();
    redraw(w, cx);
    w.update(cx, |m, _, _| {
        assert!(matches!(m.modal, Some(Modal::Alerts)));
        std::rc::Rc::make_mut(m.data.state.as_mut().unwrap())["alerts"] = json!([]);
    })
    .unwrap();
    redraw(w, cx);
    w.update(cx, |m, _, _| assert!(m.modal.is_none())).unwrap();
}

fn finish_note(m: &MainWindow) -> Option<String> {
    match &m.modal {
        Some(Modal::Finish(f)) => f.note.clone(),
        _ => panic!("the confirm is open"),
    }
}

#[gpui_kit::test]
fn a_link_menus_mark_done_only_asks_until_confirmed(cx: &mut TestAppContext) {
    let (w, rec) = parity::window(cx);
    w.update(cx, |m, _, cx| m.open_link("taskboard-dev://task/t2?do=done", cx)).unwrap();
    redraw(w, cx);
    w.update(cx, |m, _, _| {
        assert!(matches!(&m.panel, Some(crate::app::Panel::Task { r, .. }) if r == "T2"), "its panel opens under the confirm");
        assert!(matches!(&m.modal, Some(Modal::Finish(f)) if f.r == "T2" && f.ask == Ask::Done));
        assert_eq!(finish_note(m), None);
    })
    .unwrap();
    assert!(rec.posts().is_empty(), "nothing is done before the confirm");
    cx.simulate_keystrokes(w.into(), "escape");
    parity::settle(cx);
    w.update(cx, |m, _, _| {
        assert!(m.modal.is_none());
        assert!(m.task_panel_open(), "backing out leaves the panel open");
    })
    .unwrap();
    assert!(rec.posts().is_empty());

    w.update(cx, |m, _, cx| m.open_link("taskboard://task/T2?do=done", cx)).unwrap();
    parity::settle(cx);
    w.update(cx, |m, _, cx| confirm_finish(m, cx)).unwrap();
    parity::settle(cx);
    assert_eq!(rec.posts().iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), ["tasks/T2/done"]);
    w.update(cx, |m, _, _| {
        assert!(m.modal.is_none());
        assert_eq!(m.toasts.first().map(|t| t.text.as_str()), Some("Marked T2 done"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn cancel_task_fails_it_as_the_owner(cx: &mut TestAppContext) {
    let (w, rec) = parity::window(cx);
    w.update(cx, |m, _, cx| m.open_link("taskboard://task/T2?do=cancel", cx)).unwrap();
    parity::settle(cx);
    w.update(cx, |m, _, _| assert!(matches!(&m.modal, Some(Modal::Finish(f)) if f.ask == Ask::Cancel))).unwrap();
    w.update(cx, |m, _, cx| confirm_finish(m, cx)).unwrap();
    parity::settle(cx);
    assert_eq!(rec.last("tasks/T2/fail"), Some(json!({"reason": "Cancelled by you"})));
    let t = rec.board().get("tasks/T2", &[]).unwrap();
    assert_eq!((t["status"].as_str(), t["failed"].as_bool()), (Some("done"), Some(true)));
}

#[gpui_kit::test]
fn a_task_finished_meanwhile_gets_a_note_not_a_post(cx: &mut TestAppContext) {
    let (w, rec) = parity::window(cx);
    w.update(cx, |m, _, cx| m.open_link("taskboard://task/T2?do=done", cx)).unwrap();
    parity::settle(cx);
    rec.board().post("tasks/T2/done", json!({"summary": "Done elsewhere"})).unwrap();
    w.update(cx, |m, _, cx| confirm_finish(m, cx)).unwrap();
    parity::settle(cx);
    assert!(rec.posts().is_empty(), "no POST for a task that's no longer open");
    w.update(cx, |m, _, _| assert_eq!(finish_note(m).as_deref(), Some("T2 is already done."))).unwrap();

    // Opened on one that's already done, the dialog says so from the start.
    w.update(cx, |m, _, cx| m.open_link("taskboard://task/T1?do=cancel", cx)).unwrap();
    redraw(w, cx);
    w.update(cx, |m, _, _| assert_eq!(super::shown_task(m, "T1").and_then(|t| super::already(t, Ask::Cancel)).as_deref(), Some("T1 is already done."))).unwrap();
}

#[test]
fn merge_asks_only_while_the_pr_is_open() {
    let t = |pr: Value| json!({"ref": "T4", "status": "done", "pr": pr});
    assert_eq!(super::already(&t(Value::Null), Ask::Merge).as_deref(), Some("T4 has no PR."));
    assert_eq!(super::already(&t(json!({"num": 9, "state": "OPEN"})), Ask::Merge), None, "a done task's open PR can still merge");
    assert_eq!(super::already(&t(json!({"num": 9, "state": "MERGED"})), Ask::Merge).as_deref(), Some("PR #9 already merged."));
    assert_eq!(super::already(&t(json!({"num": 9, "state": "CLOSED"})), Ask::Merge).as_deref(), Some("PR #9 isn't open."));
}

#[gpui_kit::test]
fn merge_without_a_pr_gets_a_note_not_a_post(cx: &mut TestAppContext) {
    let (w, rec) = parity::window(cx);
    w.update(cx, |m, _, cx| {
        m.modal = Some(Modal::finish("T2".into(), Ask::Merge));
        confirm_finish(m, cx);
    })
    .unwrap();
    parity::settle(cx);
    assert!(rec.posts().is_empty());
    w.update(cx, |m, _, _| assert_eq!(finish_note(m).as_deref(), Some("T2 has no PR."))).unwrap();
}

#[gpui_kit::test]
fn other_do_words_just_open_the_task(cx: &mut TestAppContext) {
    let (w, rec) = parity::window(cx);
    w.update(cx, |m, _, cx| m.open_link("taskboard://task/T2?do=delete", cx)).unwrap();
    parity::settle(cx);
    w.update(cx, |m, _, _| {
        assert!(m.modal.is_none());
        assert!(matches!(&m.panel, Some(crate::app::Panel::Task { r, .. }) if r == "T2"));
    })
    .unwrap();
    assert!(rec.posts().is_empty());
}

/// Opens the alerts dialog (with an alert, so it stays) and draws it.
fn open_alerts(w: W, cx: &mut TestAppContext) {
    w.update(cx, |m, window, cx| {
        std::rc::Rc::make_mut(m.data.state.as_mut().unwrap())["alerts"] = one_alert();
        m.set_modal(Some(Modal::Alerts), window, cx);
    })
    .unwrap();
    redraw(w, cx);
}

#[gpui_kit::test]
fn a_dialog_takes_focus_and_gives_it_back(cx: &mut TestAppContext) {
    let (w, _) = parity::window(cx);
    redraw(w, cx);
    // A control on the page has focus, as after a Tab.
    let before = w
        .update(cx, |m, window, cx| {
            window.focus(&m.focus, cx);
            window.focus_next(cx);
            window.focused(cx).filter(|f| *f != m.focus).expect("the page has a tab stop")
        })
        .unwrap();
    open_alerts(w, cx);
    w.update(cx, |m, window, cx| assert!(m.modal_focus.contains_focused(window, cx), "the dialog has focus")).unwrap();
    cx.simulate_keystrokes(w.into(), "escape");
    redraw(w, cx);
    w.update(cx, |m, window, cx| {
        assert!(m.modal.is_none());
        assert_eq!(window.focused(cx), Some(before), "focus goes back where it was");
    })
    .unwrap();
}

#[gpui_kit::test]
fn tab_stays_inside_an_open_dialog(cx: &mut TestAppContext) {
    let (w, _) = parity::window(cx);
    open_alerts(w, cx);
    for keys in ["tab", "tab", "tab", "tab", "tab", "shift-tab", "shift-tab", "shift-tab"] {
        cx.simulate_keystrokes(w.into(), keys);
        redraw(w, cx);
        w.update(cx, |m, window, cx| {
            assert!(m.modal_focus.contains_focused(window, cx), "{keys} keeps focus in the dialog");
            assert!(window.focused(cx) != Some(m.modal_focus.clone()), "{keys} lands on a control in it");
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn page_keys_pause_while_a_dialog_is_open(cx: &mut TestAppContext) {
    let (w, _) = parity::window(cx);
    open_alerts(w, cx);
    cx.simulate_keystrokes(w.into(), "cmd-4");
    parity::settle(cx);
    cx.simulate_keystrokes(w.into(), "cmd-k");
    parity::settle(cx);
    w.update(cx, |m, _, _| {
        assert!(m.page == Page::Board, "the page behind stays put");
        assert!(matches!(m.modal, Some(Modal::Alerts)), "search doesn't open over the dialog");
    })
    .unwrap();
    cx.simulate_keystrokes(w.into(), "escape");
    redraw(w, cx);
    cx.simulate_keystrokes(w.into(), "cmd-4");
    parity::settle(cx);
    w.update(cx, |m, _, _| assert!(m.page == Page::Backlog, "once it closes, they work again")).unwrap();
}

#[gpui_kit::test]
fn esc_closes_the_top_layer_and_keeps_the_checked_issues(cx: &mut TestAppContext) {
    let (w, _) = parity::window(cx);
    w.update(cx, |m, window, cx| {
        m.page = Page::Backlog;
        m.backlog.picked = vec!["B1".into(), "B2".into()];
        m.menu = Some(("plan-B1".into(), point(px(0.), px(0.))));
        m.panel = Some(crate::app::Panel::Issue { r: "B1".into() });
        m.escape(window, cx);
        assert!(m.menu.is_none(), "the menu closes first");
        assert!(m.panel.is_some() && m.backlog.picked.len() == 2, "the panel and checks stay");
        m.set_modal(Some(Modal::Note("x".into())), window, cx);
        m.escape(window, cx);
        assert!(m.modal.is_none() && m.panel.is_some() && m.backlog.picked.len() == 2, "then the dialog alone");
        m.escape(window, cx);
        assert!(m.panel.is_none() && m.backlog.picked.len() == 2, "then the panel alone");
        m.escape(window, cx);
        assert!(m.backlog.picked.is_empty(), "then the checks");
    })
    .unwrap();
}

#[test]
fn merge_names_the_holds_it_skips() {
    assert_eq!(super::holds(&json!({"checks": "pending", "review": "pending", "approvals": {"have": 0, "need": 1}})), ["CI, still running", "Review, 0 of 1 approvals"]);
    assert_eq!(super::holds(&json!({"checks": "fail", "review": "changes"})), ["CI, failing", "Review, changes asked for"]);
    assert_eq!(super::holds(&json!({"checks": "pass", "review": "pending", "approvals": {"have": 1, "need": 1}})), ["Review, not approved yet"]);
    assert!(super::holds(&json!({"checks": "pass", "review": "approved"})).is_empty());
}

#[test]
fn a_refused_merge_says_who_refused_and_what_next() {
    assert_eq!(super::refuser(&json!({"host": "github"})), "GitHub");
    assert_eq!(super::refuser(&json!({})), "The board");
    assert_eq!(super::after_refusal("The branch is 3 commits behind main."), Some("Rebase it from its terminal, then merge again."));
    assert_eq!(super::after_refusal("Merge conflict in app.rs"), Some("Rebase it from its terminal, then merge again."));
    assert_eq!(super::after_refusal("Not allowed"), None);
}

#[gpui_kit::test]
fn the_merge_dialog_holds_its_answer_until_closed(cx: &mut TestAppContext) {
    let (w, _) = parity::window(cx);
    let refused = Refused { why: "The branch is 3 commits behind main.".into(), at: "3:16 PM".into() };
    w.update(cx, |m, _, _| {
        m.modal = Some(Modal::Finish(Finish { r: "T2".into(), ask: Ask::Merge, note: None, busy: true, refused: None }));
    })
    .unwrap();
    redraw(w, cx);
    w.update(cx, |m, _, _| assert!(matches!(&m.modal, Some(Modal::Finish(f)) if f.busy), "merging, the dialog stays")).unwrap();
    w.update(cx, |m, _, _| {
        if let Some(Modal::Finish(f)) = &mut m.modal {
            f.busy = false;
            f.refused = Some(refused.clone());
        }
    })
    .unwrap();
    redraw(w, cx);
    redraw(w, cx);
    w.update(cx, |m, _, _| assert!(matches!(&m.modal, Some(Modal::Finish(f)) if f.refused.as_ref() == Some(&refused)), "a refusal stays on screen")).unwrap();
    cx.simulate_keystrokes(w.into(), "escape");
    parity::settle(cx);
    w.update(cx, |m, _, _| assert!(m.modal.is_none(), "esc closes it")).unwrap();
}

#[gpui_kit::test]
fn closing_the_confirm_while_it_works_still_shows_the_answer(cx: &mut TestAppContext) {
    let (w, rec) = parity::window(cx);
    w.update(cx, |m, _, cx| m.open_link("taskboard://task/T2?do=done", cx)).unwrap();
    parity::settle(cx);
    w.update(cx, |m, _, cx| {
        confirm_finish(m, cx);
        m.modal = None;
    })
    .unwrap();
    parity::settle(cx);
    assert_eq!(rec.posts().iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), ["tasks/T2/done"]);
    w.update(cx, |m, _, _| assert_eq!(m.toasts.first().map(|t| t.text.as_str()), Some("Marked T2 done"), "a toast says what happened")).unwrap();
}

#[test]
fn each_confirm_says_what_it_skips() {
    assert!(super::what_it_does(Ask::Done).contains("without waiting for its agent"));
    assert!(super::what_it_does(Ask::Cancel).contains("cancelled by you"));
    assert!(super::what_it_does(Ask::Merge).contains("past its checks and reviews"));
}
