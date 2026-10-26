//! The name sort against the web board (`localeSort` cases in `parity/golden/forms.json`), and
//! the alerts dialog in a headless window.
use super::*;
// An explicit import beats the glob above, which would bring in GPUI's own `test` attribute
// (and `#[gpui_kit::test]` expands to a plain `#[test]`).
#[allow(unused_imports)]
use ::core::prelude::v1::test;
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
        assert!(matches!(&m.modal, Some(Modal::Finish(f)) if f.r == "T2" && !f.cancel));
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
    w.update(cx, |m, _, _| assert!(matches!(&m.modal, Some(Modal::Finish(f)) if f.cancel))).unwrap();
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
    w.update(cx, |m, _, _| assert_eq!(super::shown_task(m, "T1").and_then(super::already).as_deref(), Some("T1 is already done."))).unwrap();
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
