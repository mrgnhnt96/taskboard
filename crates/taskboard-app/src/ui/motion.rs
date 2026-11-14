//! Shared entrances and transitions, built on gpui's `with_animation`.
//!
//! Every helper eases with an ease-out quint and can wait before it starts, so a list can stagger
//! its rows with [`st`]. An animation's state is keyed by its element id and plays once, the first
//! time that id is drawn: put whatever should replay it (the recap shown, an expand's open state)
//! into the id. When the Mac asks for less motion, gpui draws the end state straight away; tests
//! draw it too, so they see the settled page.
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::time::Duration;

/// Fade and lift in.
pub const FADE: u64 = 240;
/// Opacity only.
pub const OP: u64 = 160;
/// A bar growing to its width.
pub const GROW_W: u64 = 400;
/// A column growing to its height.
pub const GROW_H: u64 = 320;
/// A dot popping in.
pub const POP: u64 = 240;
/// A number counting up.
pub const COUNT: u64 = 600;
/// An expand opening.
pub const EXPAND: u64 = 200;

/// Animations play outside tests.
fn on() -> bool {
    !cfg!(test)
}

/// The ease-out quint every entrance uses.
pub fn quint(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(5)
}

/// The delay of the `i`th item of a staggered list: `base`, then `step` more per item, capped at
/// the ninth so long lists don't keep the last rows waiting.
pub fn st(i: usize, step: u64, base: u64) -> u64 {
    base + i.min(8) as u64 * step
}

/// The usual stagger for rows in a card.
pub fn row(i: usize) -> u64 {
    st(i, 40, 150)
}

/// Draws `el` through `f` with a value going 0 → 1 over `dur` ms, after `delay` ms at 0.
pub fn play<E: IntoElement + 'static>(el: E, id: impl Into<ElementId>, delay: u64, dur: u64, f: impl Fn(E, f32) -> E + 'static) -> AnyElement {
    if !on() {
        return f(el, 1.).into_any_element();
    }
    let (dur, total) = (dur.max(1) as f32, (delay + dur.max(1)) as f32);
    let run = Animation::new(Duration::from_millis(delay + dur as u64));
    el.with_animation(id, run, move |el, t| f(el, quint((t * total - delay as f32) / dur))).into_any_element()
}

/// Fades in and rises 8px.
pub fn fade<E: IntoElement + Styled + 'static>(el: E, id: impl Into<ElementId>, delay: u64) -> AnyElement {
    play(el, id, delay, FADE, |el, v| el.opacity(v).relative().top(px((1. - v) * 8.)))
}

/// Fades in sliding 16px from the left.
pub fn fade_x<E: IntoElement + Styled + 'static>(el: E, id: impl Into<ElementId>, delay: u64) -> AnyElement {
    play(el, id, delay, FADE, |el, v| el.opacity(v).relative().left(px((v - 1.) * 16.)))
}

/// Fades in where it is.
pub fn op<E: IntoElement + Styled + 'static>(el: E, id: impl Into<ElementId>, delay: u64) -> AnyElement {
    play(el, id, delay, OP, |el, v| el.opacity(v))
}

/// A bar `h` px high that grows to `share` (0–1) of its track's width.
pub fn grow_w(id: impl Into<ElementId>, delay: u64, dur: u64, share: f32, color: Hsla, h: f32) -> AnyElement {
    let bar = div().flex_none().h(px(h)).bg(color).rounded_r(px(h / 2.));
    play(bar, id, delay, dur, move |el, v| el.w(relative((share * v).clamp(0., 1.))))
}

/// A column that grows up to `share` (0–1) of its box's height.
pub fn grow_h<E: IntoElement + Styled + 'static>(el: E, id: impl Into<ElementId>, delay: u64, dur: u64, share: f32) -> AnyElement {
    play(el, id, delay, dur, move |el, v| el.h(relative((share * v).clamp(0., 1.))))
}

/// `base` showing `to` counted up from 0 by `show`.
pub fn count(base: Div, id: impl Into<ElementId>, delay: u64, to: f64, show: impl Fn(f64) -> String + 'static) -> AnyElement {
    play(base, id, delay, COUNT, move |el, v| el.child(show(to * v as f64)))
}

/// An overlay in `cover` that uncovers its (relative) parent from left to right.
pub fn wipe(id: impl Into<ElementId>, delay: u64, dur: u64, cover: Hsla) -> AnyElement {
    if !on() {
        return div().into_any_element();
    }
    let veil = div().absolute().top_0().bottom_0().right_0().bg(cover);
    play(veil, id, delay, dur, |el, v| el.w(relative(1. - v)))
}

/// A round dot `size` px across that pops in.
pub fn pop(id: impl Into<ElementId>, delay: u64, size: f32, color: Hsla) -> AnyElement {
    let d = div().flex_none().rounded_full().bg(color);
    play(d, id, delay, POP, move |el, v| el.size(px(size * v)))
}

/// An expanded box opening to about `h` px over [`EXPAND`] ms; its rows play their own entrances.
pub fn expand(el: Div, id: impl Into<ElementId>, h: f32) -> AnyElement {
    play(el, id, 0, EXPAND, move |el, v| if v < 1. { el.max_h(px(h * v)).overflow_hidden() } else { el })
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's `test` attribute.
    use super::{quint, st};

    #[test]
    fn quint_eases_out_from_0_to_1() {
        assert_eq!(quint(0.), 0.);
        assert_eq!(quint(1.), 1.);
        assert!(quint(0.5) > 0.9, "most of the way there by half time");
        assert_eq!(quint(2.), 1.);
    }

    #[test]
    fn stagger_caps_at_the_ninth_item() {
        assert_eq!(st(0, 40, 150), 150);
        assert_eq!(st(3, 40, 150), 270);
        assert_eq!(st(8, 40, 150), st(20, 40, 150));
    }
}
