//! Small building blocks every page uses: buttons, chips, dots, section labels, the segmented
//! control, text fields, tooltips, popover menus and the modal shell. Their sizes follow the
//! web board's `app.css` so the native app reads the same.
use crate::theme::Theme;
use crate::ui::text_input::TextField;
use gpui_kit::prelude::*;
use gpui_kit::*;

// ------------------------------------------------------------------ text

/// `.h3`: 12px semibold uppercase muted section label.
pub fn h3(t: &Theme, text: impl Into<SharedString>) -> Div {
    let text: SharedString = text.into();
    div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(t.muted).whitespace_nowrap().child(text.to_uppercase())
}

/// `.help`: 12.5px muted.
pub fn help(t: &Theme, text: impl Into<SharedString>) -> Div {
    div().text_size(px(12.5)).text_color(t.muted).child(text.into())
}

/// `.empty`: centered muted placeholder.
pub fn empty(t: &Theme, text: impl Into<SharedString>) -> Div {
    div().py(px(20.)).px(px(8.)).flex().justify_center().text_size(px(13.)).text_color(t.muted).child(text.into())
}

pub fn mono(t: &Theme, text: impl Into<SharedString>) -> Div {
    div().font_family(t.mono_font.clone()).text_size(px(12.)).child(text.into())
}

// ------------------------------------------------------------------ marks

/// A round status dot.
pub fn dot(color: Hsla, size: f32) -> Div {
    div().flex_none().size(px(size)).rounded_full().bg(color)
}

/// `.chip`: 20px outlined label.
pub fn chip(t: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(20.))
        .px(px(7.))
        .rounded(px(6.))
        .border_1()
        .border_color(t.border)
        .text_size(px(11.5))
        .text_color(t.muted)
        .whitespace_nowrap()
        .child(text.into())
}

/// A filled pill in a tone (fg on its soft bg).
pub fn pill(fg: Hsla, bg: Hsla, text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .h(px(20.))
        .px(px(8.))
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_size(px(11.5))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .child(text.into())
}

/// The pill tones the board uses, by name.
pub fn tone(t: &Theme, name: &str) -> (Hsla, Hsla) {
    match name {
        "accent" | "working" => (t.accent_fg, t.accent_soft),
        "warn" | "needs" => (t.warn_fg, t.warn_soft),
        "up" | "done" => (t.up_fg, t.up_soft),
        "down" | "failed" => (t.down, t.down_soft),
        "goal" | "planned" => (t.goal, t.goal_soft),
        "results" => (t.results, t.results_soft),
        _ => (t.muted, t.seg),
    }
}

pub fn tone_pill(t: &Theme, name: &str, text: impl Into<SharedString>) -> Div {
    let (fg, bg) = tone(t, name);
    pill(fg, bg, text)
}

/// A thin horizontal divider.
pub fn divider(t: &Theme) -> Div {
    div().h(px(1.)).w_full().flex_none().bg(t.divider)
}

// ------------------------------------------------------------------ buttons

fn btn_base(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(30.))
        .px(px(12.))
        .rounded(px(8.))
        .border_1()
        .border_color(t.border_2)
        .bg(t.card)
        .text_color(t.text)
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .cursor_pointer()
        .child(label.into())
}

/// `.btn`: outlined button.
pub fn btn(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let h = t.panel_2;
    btn_base(t, id, label).hover(move |s| s.bg(h))
}

/// `.btn.primary`: accent filled.
pub fn btn_primary(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    btn_base(t, id, label).bg(t.accent_btn).border_color(t.accent_btn).text_color(t.on_accent).hover(|s| s.opacity(0.9))
}

/// `.btn.danger`: red outlined.
pub fn btn_danger(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let soft = t.down_soft;
    btn_base(t, id, label).text_color(t.down).border_color(t.down_line).hover(move |s| s.bg(soft))
}

/// A small 24px button (row actions).
pub fn btn_small(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    btn(t, id, label).h(px(24.)).px(px(8.)).rounded(px(6.)).text_size(px(12.))
}

/// A borderless text button (links inside rows).
pub fn link(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let fg = t.accent;
    div().id(id).flex_none().cursor_pointer().text_color(fg).hover(|s| s.underline()).child(label.into())
}

/// Disabled look for any button (the caller simply doesn't attach `on_click`).
pub fn disabled(b: Stateful<Div>) -> Stateful<Div> {
    b.opacity(0.45).cursor_default()
}

/// `.seg`: a segmented control. `on` is the selected index.
pub fn seg<F>(t: &Theme, id: &str, labels: &[&str], on: usize, mut f: F) -> Div
where
    F: FnMut(usize, Stateful<Div>) -> Stateful<Div>,
{
    let mut row = div().flex().flex_none().items_center().gap(px(2.)).p(px(2.)).rounded(px(8.)).bg(t.seg);
    for (i, l) in labels.iter().enumerate() {
        let sel = i == on;
        let hover = t.text;
        let item = div()
            .id(SharedString::from(format!("{id}-{i}")))
            .flex()
            .items_center()
            .h(px(24.))
            .px(px(10.))
            .rounded(px(6.))
            .text_size(px(12.5))
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .whitespace_nowrap()
            .text_color(if sel { t.text } else { t.muted })
            .when(sel, |d| d.bg(t.card).shadow_sm())
            .hover(move |s| s.text_color(hover))
            .child(l.to_string());
        row = row.child(f(i, item));
    }
    row
}

// ------------------------------------------------------------------ cards and shells

/// `.card`: white rounded box with a border.
pub fn card(t: &Theme) -> Div {
    div().flex().flex_col().rounded(px(10.)).border_1().border_color(t.border).bg(t.card)
}

/// The dimmed full-window backdrop behind a modal or panel; clicks on it call `on_close`.
pub fn scrim(t: &Theme, id: &str) -> Stateful<Div> {
    div().id(SharedString::from(id.to_string())).absolute().top_0().left_0().size_full().bg(t.backdrop)
}

/// The modal dialog box (centered by the caller's scrim).
pub fn modal_box(t: &Theme, width: f32) -> Div {
    div()
        .flex()
        .flex_col()
        .w(px(width))
        .max_w(relative(0.94))
        .max_h(relative(0.9))
        .rounded(px(14.))
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .shadow_lg()
        .overflow_hidden()
}

/// A popover menu box (anchored by the caller).
pub fn menu_box(t: &Theme, width: f32) -> Div {
    div().flex().flex_col().w(px(width)).p(px(4.)).rounded(px(10.)).border_1().border_color(t.border).bg(t.card).shadow_lg()
}

/// One row in a popover menu.
pub fn menu_item(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>, selected: bool) -> Stateful<Div> {
    let h = t.panel_2;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(30.))
        .px(px(10.))
        .rounded(px(6.))
        .text_size(px(13.))
        .cursor_pointer()
        .text_color(if selected { t.accent_fg } else { t.text })
        .when(selected, |d| d.bg(t.accent_soft))
        .hover(move |s| s.bg(h))
        .child(label.into())
}

/// A popover anchored under the element that opened it, drawn above everything.
pub fn popover(at: Point<Pixels>, child: impl IntoElement) -> AnyElement {
    deferred(anchored().position(at).snap_to_window_with_margin(px(8.)).child(child)).with_priority(3).into_any_element()
}

// ------------------------------------------------------------------ text fields

pub enum KeyOutcome {
    Submit,
    Cancel,
    Ignored,
}

/// A text field entity plus its focus handle. `multi` wraps and grows (⇧↩ for a new line).
pub struct Input {
    pub field: Entity<TextField>,
    pub focus: FocusHandle,
}

impl Input {
    pub fn new(cx: &mut App, placeholder: impl Into<SharedString>, multi: bool) -> Input {
        let field = cx.new(|cx| {
            let mut f = TextField::new(cx, false, placeholder);
            f.wrap = multi;
            f
        });
        let focus = field.read(cx).focus.clone();
        Input { field, focus }
    }

    /// A one-line field that shows bullets and never copies its text out (tokens).
    pub fn secret(cx: &mut App, placeholder: impl Into<SharedString>) -> Input {
        let field = cx.new(|cx| TextField::new(cx, true, placeholder));
        let focus = field.read(cx).focus.clone();
        Input { field, focus }
    }

    pub fn with_text(cx: &mut App, placeholder: impl Into<SharedString>, multi: bool, text: &str) -> Input {
        let i = Input::new(cx, placeholder, multi);
        i.set_text(text, cx);
        i
    }

    pub fn text(&self, cx: &App) -> String {
        self.field.read(cx).text().to_string()
    }

    pub fn set_text(&self, text: &str, cx: &mut App) {
        self.field.update(cx, |f, cx| f.set_text(text, cx));
    }

    pub fn clear(&self, cx: &mut App) {
        self.field.update(cx, |f, cx| f.clear(cx));
    }

    /// Keys the field leaves to the parent: ↩ submits (⌘↩ in a multi-line field), esc cancels.
    pub fn on_key(&self, ev: &KeyDownEvent, cx: &App) -> KeyOutcome {
        let ks = &ev.keystroke;
        let multi = self.field.read(cx).wrap;
        match ks.key.as_str() {
            "enter" if !multi && !ks.modifiers.shift => KeyOutcome::Submit,
            "enter" if multi && ks.modifiers.platform => KeyOutcome::Submit,
            "escape" => KeyOutcome::Cancel,
            _ => KeyOutcome::Ignored,
        }
    }

    /// The field's box. The caller attaches `on_key_down` if it wants submit/cancel.
    pub fn render(&self, t: &Theme, id: impl Into<ElementId>, window: &Window) -> Stateful<Div> {
        let focused = self.focus.is_focused(window);
        let focus = self.focus.clone();
        div()
            .id(id)
            .flex()
            .items_center()
            .min_h(px(32.))
            .px(px(10.))
            .py(px(6.))
            .rounded(px(8.))
            .border_1()
            .border_color(if focused { t.accent } else { t.border_2 })
            .bg(t.card)
            .text_size(px(13.5))
            .text_color(t.text)
            .overflow_hidden()
            .cursor_text()
            .on_click(move |_, window, cx| window.focus(&focus, cx))
            .child(self.field.clone())
    }
}

// ------------------------------------------------------------------ tooltips

struct Tip {
    text: SharedString,
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<Theme>();
        div()
            .max_w(px(360.))
            .px(px(8.))
            .py(px(5.))
            .rounded(px(6.))
            .bg(t.text)
            .text_color(t.bg)
            .text_size(px(12.))
            .font_family(t.ui_font.clone())
            .child(self.text.clone())
    }
}

/// A plain text tooltip: `.tooltip(tip("…"))`.
pub fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| Tip { text: text.clone() }).into()
}

// ------------------------------------------------------------------ line icons

/// The web board's inline SVG icons (`ICON.*` in `app.js`), drawn on a 24-unit grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Glyph {
    /// `ICON.flag` (stroke 2.2).
    Flag,
    /// `ICON.x` (stroke 2.2).
    X,
    /// `ICON.fwd`: a right chevron (stroke 2.2).
    Fwd,
    /// `ICON.more`: three filled dots.
    More,
    /// `ICON.jira`: a ticket (stroke 2.4).
    Jira,
    /// `ICON.pr`: a pull request (stroke 2.4).
    Pr,
}

/// A [`Glyph`] `size` px square in `color`.
pub fn glyph(g: Glyph, size: f32, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let k = bounds.size.width.as_f32() / 24.;
            let o = bounds.origin;
            let u = move |x: f32, y: f32| point(o.x + px(x * k), o.y + px(y * k));
            let stroke = |pts: &[(f32, f32)], w: f32, window: &mut Window| {
                let mut p = PathBuilder::stroke(px(w * k));
                p.move_to(u(pts[0].0, pts[0].1));
                for (x, y) in &pts[1..] {
                    p.line_to(u(*x, *y));
                }
                if let Ok(p) = p.build() {
                    window.paint_path(p, color);
                }
            };
            let ring = |cx: f32, cy: f32, r: f32| -> Vec<(f32, f32)> {
                (0..=24).map(|i| i as f32 * std::f32::consts::TAU / 24.).map(|a| (cx + r * a.cos(), cy + r * a.sin())).collect()
            };
            match g {
                Glyph::Flag => {
                    stroke(&[(5., 21.), (5., 4.)], 2.2, window);
                    stroke(&[(5., 4.), (16., 4.), (14., 8.), (16., 12.), (5., 12.)], 2.2, window);
                }
                Glyph::X => {
                    stroke(&[(6., 6.), (18., 18.)], 2.2, window);
                    stroke(&[(18., 6.), (6., 18.)], 2.2, window);
                }
                Glyph::Fwd => stroke(&[(9., 6.), (15., 12.), (9., 18.)], 2.2, window),
                Glyph::More => {
                    for x in [5., 12., 19.] {
                        let mut p = PathBuilder::fill();
                        let pts = ring(x, 12., 1.8);
                        p.move_to(u(pts[0].0, pts[0].1));
                        for (x, y) in &pts[1..] {
                            p.line_to(u(*x, *y));
                        }
                        p.close();
                        if let Ok(p) = p.build() {
                            window.paint_path(p, color);
                        }
                    }
                }
                Glyph::Jira => {
                    stroke(&[(4., 6.), (20., 6.), (20., 18.), (4., 18.), (4., 6.)], 2.4, window);
                    stroke(&[(8., 10.), (16., 10.)], 2.4, window);
                    stroke(&[(8., 14.), (13., 14.)], 2.4, window);
                }
                Glyph::Pr => {
                    for (x, y) in [(6., 6.), (6., 18.), (18., 18.)] {
                        stroke(&ring(x, y, 2.5), 2.4, window);
                    }
                    stroke(&[(6., 8.5), (6., 15.5)], 2.4, window);
                    // M18 15.5V9a3 3 0 0 0-3-3h-4
                    let mut pts = vec![(18., 15.5), (18., 9.)];
                    pts.extend((1..=6).map(|i| i as f32 * std::f32::consts::FRAC_PI_2 / 6.).map(|a| (15. + 3. * a.cos(), 9. - 3. * a.sin())));
                    pts.push((11., 6.));
                    stroke(&pts, 2.4, window);
                }
            }
        },
    )
    .size(px(size))
    .flex_none()
}

// ------------------------------------------------------------------ icons

/// The web's stroked `ICON` set (24-unit viewBox, round caps and joins), drawn on a canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// `ICON.chev`: a down chevron (stroke 2.2).
    Chev,
    /// `ICON.fwd`: a right chevron (stroke 2.2).
    Fwd,
    /// `ICON.board`: the logo's three columns (stroke 2).
    Board,
    /// Midna's mark: a rounded window with its crescent moon.
    Midna,
}

/// A crescent: the disc at (cx, cy) radius r less the disc at (bx, by) radius br, as one outline
/// (the outer arc outside the bite, then the bite's arc back inside the disc).
pub fn crescent(c: (f32, f32), r: f32, bite: (f32, f32), br: f32) -> Vec<(f32, f32)> {
    let n = 48;
    let inside = |p: (f32, f32), o: (f32, f32), rr: f32| (p.0 - o.0).powi(2) + (p.1 - o.1).powi(2) < rr * rr;
    let ring = |o: (f32, f32), rr: f32| -> Vec<(f32, f32)> {
        (0..n).map(|i| std::f32::consts::TAU * i as f32 / n as f32).map(|a| (o.0 + rr * a.cos(), o.1 + rr * a.sin())).collect()
    };
    let outer = ring(c, r);
    let inner = ring(bite, br);
    // Start the outer walk just after a point the bite covers, so the kept points come out in one run.
    let start = (0..n).find(|&i| inside(outer[i], bite, br) && !inside(outer[(i + 1) % n], bite, br)).map_or(0, |i| i + 1);
    let mut pts: Vec<(f32, f32)> = (0..n).map(|k| outer[(start + k) % n]).filter(|p| !inside(*p, bite, br)).collect();
    let istart = (0..n).find(|&i| !inside(inner[i], c, r) && inside(inner[(i + 1) % n], c, r)).map_or(0, |i| i + 1);
    let back: Vec<(f32, f32)> = (0..n).map(|k| inner[(istart + k) % n]).filter(|p| inside(*p, c, r)).collect();
    // The outer arc runs one way round; the bite's arc must come back the other way.
    pts.extend(back.into_iter().rev());
    pts
}

/// An `Icon` at `size` px in `color`.
pub fn icon(kind: Icon, size: f32, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let k = bounds.size.width.as_f32() / 24.;
            let o = bounds.origin;
            let u = |x: f32, y: f32| point(o.x + px(x * k), o.y + px(y * k));
            // An open polyline with round caps and joins (a dot at every vertex).
            let poly = |pts: &[(f32, f32)], w: f32, window: &mut Window| {
                let mut p = PathBuilder::stroke(px(w * k));
                p.move_to(u(pts[0].0, pts[0].1));
                for (x, y) in &pts[1..] {
                    p.line_to(u(*x, *y));
                }
                if let Ok(p) = p.build() {
                    window.paint_path(p, color);
                }
                for (x, y) in pts {
                    let mut d = PathBuilder::fill();
                    let r = w / 2.;
                    d.move_to(u(x + r, *y));
                    for i in 1..=12 {
                        let a = std::f32::consts::TAU * i as f32 / 12.;
                        d.line_to(u(x + r * a.cos(), y + r * a.sin()));
                    }
                    d.close();
                    if let Ok(d) = d.build() {
                        window.paint_path(d, color);
                    }
                }
            };
            // A stroked rounded rectangle.
            let rect = |x: f32, y: f32, w: f32, h: f32, rx: f32, sw: f32, window: &mut Window| {
                let mut pts = Vec::new();
                for (cx0, cy0, a0) in [(x + w - rx, y + rx, -90f32), (x + w - rx, y + h - rx, 0.), (x + rx, y + h - rx, 90.), (x + rx, y + rx, 180.)] {
                    for i in 0..=4 {
                        let a = (a0 + 22.5 * i as f32).to_radians();
                        pts.push((cx0 + rx * a.cos(), cy0 + rx * a.sin()));
                    }
                }
                let mut p = PathBuilder::stroke(px(sw * k));
                p.move_to(u(pts[0].0, pts[0].1));
                for (px_, py_) in &pts[1..] {
                    p.line_to(u(*px_, *py_));
                }
                p.close();
                if let Ok(p) = p.build() {
                    window.paint_path(p, color);
                }
            };
            match kind {
                Icon::Chev => poly(&[(6., 9.), (12., 15.), (18., 9.)], 2.2, window),
                Icon::Fwd => poly(&[(9., 6.), (15., 12.), (9., 18.)], 2.2, window),
                Icon::Board => {
                    rect(3., 4., 5., 16., 1.5, 2., window);
                    rect(10., 4., 5., 11., 1.5, 2., window);
                    rect(17., 4., 4., 7., 1.5, 2., window);
                }
                Icon::Midna => {
                    rect(3., 3., 18., 18., 4.5, 2., window);
                    let pts = crescent((11.5, 12.5), 5., (14., 10.), 4.);
                    if let Some(first) = pts.first() {
                        let mut p = PathBuilder::fill();
                        p.move_to(u(first.0, first.1));
                        for (x, y) in &pts[1..] {
                            p.line_to(u(*x, *y));
                        }
                        p.close();
                        if let Ok(p) = p.build() {
                            window.paint_path(p, color);
                        }
                    }
                }
            }
        },
    )
    .size(px(size))
    .flex_none()
}

#[cfg(test)]
mod tests {
    #[::core::prelude::v1::test]
    fn the_crescent_is_one_simple_outline() {
        let pts = super::crescent((11.5, 12.5), 5., (14., 10.), 4.);
        // Its area (shoelace) is the disc less the overlap, so the two arcs run opposite ways.
        let n = pts.len();
        let area = (0..n).map(|i| pts[i].0 * pts[(i + 1) % n].1 - pts[(i + 1) % n].0 * pts[i].1).sum::<f32>().abs() / 2.;
        let (r, br, d) = (5f32, 4f32, ((14f32 - 11.5).powi(2) + (10f32 - 12.5).powi(2)).sqrt());
        let lens = r * r * ((d * d + r * r - br * br) / (2. * d * r)).acos() + br * br * ((d * d + br * br - r * r) / (2. * d * br)).acos()
            - 0.5 * ((-d + r + br) * (d + r - br) * (d - r + br) * (d + r + br)).sqrt();
        let want = std::f32::consts::PI * r * r - lens;
        assert!((area - want).abs() < want * 0.08, "area {area}, want {want}");
    }
}
