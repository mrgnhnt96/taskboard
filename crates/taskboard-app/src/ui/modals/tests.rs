//! The name sort against the web board (`localeSort` cases in `parity/golden/forms.json`), and
//! the alerts dialog in a headless window.
use super::*;
// An explicit import beats the glob above, which would bring in GPUI's own `test` attribute
// (and `#[gpui_kit::test]` expands to a plain `#[test]`).
#[allow(unused_imports)]
use ::core::prelude::v1::test;
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
        m.data.state.as_mut().unwrap()["alerts"] = one_alert();
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
        m.data.state.as_mut().unwrap()["alerts"] = one_alert();
        m.set_modal(Some(Modal::Alerts), window, cx);
    })
    .unwrap();
    redraw(w, cx);
    w.update(cx, |m, _, _| {
        assert!(matches!(m.modal, Some(Modal::Alerts)));
        m.data.state.as_mut().unwrap()["alerts"] = json!([]);
    })
    .unwrap();
    redraw(w, cx);
    w.update(cx, |m, _, _| assert!(m.modal.is_none())).unwrap();
}
