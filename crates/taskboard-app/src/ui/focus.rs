//! Keyboard focus: Tab stops with a visible ring, and Tab kept inside an open dialog.
use crate::theme::Theme;
use gpui_kit::*;

/// The ring's width around a control focused from the keyboard.
pub const RING: f32 = 2.;

/// The ring: an accent outline drawn outside the control, so nothing moves when it shows. A
/// card-coloured gap sits between, so it still shows around an accent-filled button.
pub fn ring(t: &Theme) -> Vec<BoxShadow> {
    let edge = |color, spread: f32| BoxShadow { color, offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(spread), inset: false };
    vec![edge(t.accent, RING * 2.), edge(t.card, RING)]
}

/// Makes a control a Tab stop that shows the ring while focused from the keyboard. Return and
/// Space then press it (GPUI turns them into a click on the focused element).
pub fn stop<E: InteractiveElement>(t: &Theme, el: E) -> E {
    let ring = ring(t);
    el.tab_index(0).focus_visible(move |s| s.shadow(ring))
}

/// Tab or Shift-Tab: the next or previous stop. While `trap` is open (a dialog), focus cycles
/// inside it and never lands on the page behind.
pub fn step(back: bool, trap: Option<&FocusHandle>, window: &mut Window, cx: &mut App) {
    let go = |window: &mut Window, cx: &mut App| if back { window.focus_prev(cx) } else { window.focus_next(cx) };
    go(window, cx);
    let Some(trap) = trap else { return };
    let start = window.focused(cx);
    let mut tries = 0;
    while !trap.contains_focused(window, cx) && tries < 200 {
        go(window, cx);
        tries += 1;
        if window.focused(cx) == start {
            break;
        }
    }
    if !trap.contains_focused(window, cx) {
        window.focus(trap, cx);
    }
}

/// Whether a text field has the keyboard, so Esc and plain keys belong to it, not the page.
/// A button clicked with the mouse takes focus too, which doesn't count.
pub fn typing(window: &Window) -> bool {
    use crate::ui::text_input::{CTX, CTX_WRAP};
    window.context_stack().iter().any(|c| c.contains(CTX) || c.contains(CTX_WRAP))
}
