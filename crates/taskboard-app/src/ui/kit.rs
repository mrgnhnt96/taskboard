//! Small building blocks every page uses: buttons, chips, dots, section labels, the segmented
//! control, text fields, tooltips, popover menus and the modal shell. Their sizes follow the
//! web board's `app.css` so the native app reads the same.
use crate::theme::{Theme, height, opacity, radius, space, text as size};
use crate::ui::text_input::TextField;
use gpui_kit::prelude::*;
use gpui_kit::*;

// ------------------------------------------------------------------ shadows

/// `--shadow-modal`: 0 24px 64px.
pub fn shadow_modal(t: &Theme) -> Vec<BoxShadow> {
    t.shadow_modal()
}

/// The chosen segment's lift: 0 1px 2px.
pub fn shadow_seg(t: &Theme) -> Vec<BoxShadow> {
    t.shadow_raised()
}

// ------------------------------------------------------------------ text

/// A section label: 11px bold uppercase muted.
pub fn h3(t: &Theme, text: impl Into<SharedString>) -> Div {
    let text: SharedString = text.into();
    div().text_size(px(size::CAPTION)).font_weight(FontWeight::BOLD).text_color(t.muted).whitespace_nowrap().child(text.to_uppercase())
}

/// Help under a field or control: 12px muted.
pub fn help(t: &Theme, text: impl Into<SharedString>) -> Div {
    div().text_size(px(size::META)).text_color(t.muted).child(text.into())
}

/// `.empty`: centered muted placeholder.
pub fn empty(t: &Theme, text: impl Into<SharedString>) -> Div {
    div().py(px(space::S6)).px(px(space::S2)).flex().justify_center().text_size(px(size::BODY)).text_color(t.muted).child(text.into())
}

pub fn mono(t: &Theme, text: impl Into<SharedString>) -> Div {
    div().font_family(t.mono_font.clone()).text_size(px(size::META)).child(text.into())
}

// ------------------------------------------------------------------ marks

/// A round status dot.
pub fn dot(color: Hsla, size: f32) -> Div {
    div().flex_none().size(px(size)).rounded_full().bg(color)
}

/// A chip: a 20px outlined label with square-ish corners (a fact, never a control).
pub fn chip(t: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(height::TAG))
        .px(px(space::S2))
        .rounded(px(radius::INNER))
        .border_1()
        .border_color(t.border_2)
        .text_size(px(size::META))
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.muted)
        .whitespace_nowrap()
        .child(text.into())
}

/// A status pill: a 20px round tag, its tone's text on the tone's soft fill. Shorter than the
/// smallest button, fully round and never hovered, so it never reads as something to press.
pub fn pill(fg: Hsla, bg: Hsla, text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(space::S1))
        .h(px(height::TAG))
        .px(px(space::S2))
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_size(px(size::META))
        .font_weight(FontWeight::BOLD)
        .line_height(px(height::TAG))
        .whitespace_nowrap()
        .cursor_default()
        .child(text.into())
}

/// Whether a task card is a design task (`kind: "design"`).
pub fn is_design(c: &serde_json::Value) -> bool {
    c["kind"] == "design"
}

/// The "Design" pill on a design task's row, card and panel.
pub fn design_pill(t: &Theme) -> Div {
    pill(t.goal, t.goal_soft, "Design")
}

/// Whether a task card is a research task (`kind: "research"`).
pub fn is_research(c: &serde_json::Value) -> bool {
    c["kind"] == "research"
}

/// The "Research" pill on a research task's row, card and panel.
pub fn research_pill(t: &Theme) -> Div {
    pill(t.results, t.results_soft, "Research")
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

// ------------------------------------------------------------------ buttons

/// What a button does, which sets its look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The one main action: filled accent.
    Primary,
    /// Any other action: outlined.
    Secondary,
    /// Removes or stops something: red text and edge.
    Danger,
    /// A quiet action inside a row or bar: no edge until hovered.
    Quiet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    /// 24px, for row actions.
    Small,
    /// 32px.
    Regular,
}

/// The one button: every button on every page is this in a kind and a size.
pub fn button(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>, kind: Kind, size: Size) -> Stateful<Div> {
    let b = button_shape(id, label, size);
    match kind {
        Kind::Primary => {
            let (hover, down) = (t.accent_hover, t.accent_down);
            b.bg(t.accent_btn).border_color(t.accent_btn).text_color(t.on_accent).hover(move |s| s.bg(hover).border_color(hover)).active(move |s| s.bg(down).border_color(down))
        }
        Kind::Secondary => {
            let (hover, down) = (t.panel_2, t.border);
            b.bg(t.card).border_color(t.border_2).text_color(t.text).hover(move |s| s.bg(hover)).active(move |s| s.bg(down))
        }
        Kind::Danger => {
            let (soft, line) = (t.down_soft, t.down);
            b.bg(t.card).border_color(t.down_line).text_color(t.down).hover(move |s| s.bg(soft)).active(move |s| s.bg(soft).border_color(line))
        }
        Kind::Quiet => {
            let (hover, text) = (t.hover_overlay, t.text);
            b.border_color(transparent_black()).text_color(t.muted).hover(move |s| s.bg(hover).text_color(text))
        }
    }
}

/// A primary button filled in another colour (a status colour): it darkens on hover and press
/// instead of turning accent blue.
pub fn filled(id: impl Into<ElementId>, label: impl Into<SharedString>, fill: Hsla, text: Hsla) -> Stateful<Div> {
    button_shape(id, label, Size::Regular).bg(fill).border_color(fill).text_color(text).hover(|s| s.opacity(0.9)).active(|s| s.opacity(0.8))
}

/// Every button's size, corners and label, before its kind's colours.
fn button_shape(id: impl Into<ElementId>, label: impl Into<SharedString>, size: Size) -> Stateful<Div> {
    let (h, pad, text, gap) = match size {
        Size::Small => (height::SMALL, space::S2, size::META, space::S1),
        Size::Regular => (height::CONTROL, space::S3, size::BODY, space::S2),
    };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(gap))
        .h(px(h))
        .px(px(pad))
        .rounded(px(radius::CONTROL))
        .border_1()
        .text_size(px(text))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .cursor_pointer()
        .child(label.into())
}

/// An outlined button.
pub fn btn(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    button(t, id, label, Kind::Secondary, Size::Regular)
}

/// The filled accent button.
pub fn btn_primary(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    button(t, id, label, Kind::Primary, Size::Regular)
}

/// A red outlined button.
pub fn btn_danger(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    button(t, id, label, Kind::Danger, Size::Regular)
}

/// A small outlined button (row actions).
pub fn btn_small(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    button(t, id, label, Kind::Secondary, Size::Small)
}

/// A square 24px button holding one icon; muted until hovered.
#[allow(dead_code)]
pub fn icon_btn(t: &Theme, id: impl Into<ElementId>, child: impl IntoElement) -> Stateful<Div> {
    let (hover, down) = (t.panel_2, t.border);
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(height::SMALL))
        .rounded(px(radius::CONTROL))
        .text_color(t.muted)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .active(move |s| s.bg(down))
        .child(child)
}

/// A borderless text button (links inside rows).
pub fn link(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let fg = t.accent;
    div().id(id).flex_none().cursor_pointer().text_color(fg).hover(|s| s.underline()).child(label.into())
}

/// Disabled look for any button (the caller simply doesn't attach `on_click`).
pub fn disabled(b: Stateful<Div>) -> Stateful<Div> {
    b.opacity(opacity::DISABLED).cursor_default()
}

/// A label that takes the bold width even while regular, so choosing it never shifts its row.
fn steady_label(label: &str, bold: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .child(div().when(bold, |d| d.font_weight(FontWeight::BOLD)).child(label.to_string()))
        .child(div().h(px(0.)).overflow_hidden().font_weight(FontWeight::BOLD).child(label.to_string()))
}

/// `.seg`: a segmented control. `on` is the selected index. The chosen one is raised on the
/// card colour with a firm edge and a bold label.
pub fn seg<F>(t: &Theme, id: &str, labels: &[&str], on: usize, mut f: F) -> Div
where
    F: FnMut(usize, Stateful<Div>) -> Stateful<Div>,
{
    let mut row = div().flex().flex_none().items_center().gap(px(space::S1)).p(px(space::S1)).rounded(px(radius::CONTROL)).bg(t.seg);
    for (i, l) in labels.iter().enumerate() {
        let sel = i == on;
        let (text, hover) = (t.text, t.border);
        let item = div()
            .id(SharedString::from(format!("{id}-{i}")))
            .flex()
            .items_center()
            .h(px(height::SMALL))
            .px(px(space::S3))
            .rounded(px(radius::INNER))
            .border_1()
            .text_size(px(size::BODY))
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .whitespace_nowrap()
            .map(|d| if sel { d.bg(t.card).border_color(t.control_line).text_color(t.text).shadow(t.shadow_raised()) } else { d.border_color(transparent_black()).text_color(t.muted).hover(move |s| s.bg(hover).text_color(text)) })
            .child(steady_label(l, sel));
        row = row.child(f(i, item));
    }
    row
}

/// One underlined tab: the chosen one has a bold label over a 2px bar in the text colour.
pub fn tab(t: &Theme, id: impl Into<ElementId>, label: &str, on: bool) -> Stateful<Div> {
    let (text, line) = (t.text, t.border_2);
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(height::CONTROL))
        .px(px(space::S2))
        .mb(px(-1.))
        .border_b_2()
        .text_size(px(size::BODY))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .whitespace_nowrap()
        .map(|d| if on { d.border_color(t.text).text_color(t.text) } else { d.border_color(transparent_black()).text_color(t.muted).hover(move |s| s.text_color(text).border_color(line)) })
        .child(steady_label(label, on))
}

// ------------------------------------------------------------------ cards and shells

/// A card: the card colour, a hairline edge and 12px corners. The caller sets its padding.
pub fn card(t: &Theme) -> Div {
    div().flex().flex_col().rounded(px(radius::CONTAINER)).border_1().border_color(t.border).bg(t.card)
}

/// The dimmed full-window backdrop behind a modal or panel; clicks on it call `on_close`. It occludes
/// what's behind it, so wheel and hover over the overlay never reach the board underneath.
pub fn scrim(t: &Theme, id: &str) -> Stateful<Div> {
    div().id(SharedString::from(id.to_string())).absolute().top_0().left_0().size_full().bg(t.backdrop).occlude()
}

/// The modal dialog box (centered by the caller's scrim).
pub fn modal_box(t: &Theme, width: f32) -> Div {
    div()
        .flex()
        .flex_col()
        .w(px(width))
        .max_w(relative(0.94))
        .max_h(relative(0.9))
        .rounded(px(radius::DIALOG))
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .shadow(t.shadow_modal())
        .overflow_hidden()
}

/// A popover menu box (anchored by the caller).
pub fn menu_box(t: &Theme, width: f32) -> Div {
    div()
        .flex()
        .flex_col()
        .w(px(width))
        .p(px(space::S1))
        .rounded(px(radius::CONTAINER))
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .shadow(t.shadow_overlay())
}

/// One row in a popover menu.
pub fn menu_item(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>, selected: bool) -> Stateful<Div> {
    let h = t.panel_2;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(space::S2))
        .min_h(px(height::CONTROL))
        .pl(px(space::S2))
        .pr(px(space::S3))
        .rounded(px(radius::CONTROL))
        .text_size(px(size::BODY))
        .cursor_pointer()
        .text_color(if selected { t.accent_fg } else { t.text })
        .when(selected, |d| d.bg(t.accent_soft).font_weight(FontWeight::MEDIUM))
        .hover(move |s| s.bg(h))
        .child(label.into())
}

/// A popover at a point (a right-click menu at the pointer), drawn above everything.
pub fn popover(at: Point<Pixels>, child: impl IntoElement) -> AnyElement {
    deferred(anchored().position(at).snap_to_window_with_margin(px(space::S2)).child(child)).with_priority(3).into_any_element()
}

thread_local! {
    static ANCHORS: std::cell::RefCell<std::collections::HashMap<SharedString, Bounds<Pixels>>> = Default::default();
}

/// Put inside the button that opens a menu: it notes where the button was drawn, under `key`,
/// so [`menu_under`] can open the menu beside it.
pub fn anchor(key: impl Into<SharedString>) -> impl IntoElement {
    let key = key.into();
    canvas(move |bounds, _, _| ANCHORS.with(|a| a.borrow_mut().insert(key.clone(), bounds)), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
}

/// Which edge of its button a menu lines up with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// Where a menu opens for a button drawn at `button`: 4px under it, lined up with its `align`
/// edge. The second point is where it goes instead, 4px above, when there's no room below.
pub fn menu_spot(button: Bounds<Pixels>, align: Align) -> (Point<Pixels>, Point<Pixels>, Anchor) {
    let gap = px(space::ANCHOR_GAP);
    let x = if align == Align::Right { button.right() } else { button.left() };
    let corner = if align == Align::Right { Anchor::TopRight } else { Anchor::TopLeft };
    (point(x, button.bottom() + gap), point(x, button.top() - gap), corner)
}

/// A menu beside the button that put [`anchor`]`(key)` inside it: under it, or above it when the
/// window is too short below, never over it. Before the button has been drawn, at `at`.
pub fn menu_under(key: &str, align: Align, at: Point<Pixels>, child: impl IntoElement) -> AnyElement {
    let Some(button) = ANCHORS.with(|a| a.borrow().get(key).copied()) else { return popover(at, child) };
    let (below, above, corner) = menu_spot(button, align);
    // Placed at the spot above and pushed down past the button: when it doesn't fit below,
    // `anchored` flips it to end at the spot above instead.
    let a = anchored().anchor(corner).position(above).offset(point(px(0.), below.y - above.y)).child(child);
    deferred(a).with_priority(3).into_any_element()
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
            .min_h(px(height::CONTROL))
            .px(px(space::S3))
            .py(px(space::S1))
            .rounded(px(radius::CONTROL))
            .border_1()
            .border_color(if focused { t.focus_ring } else { t.control_line })
            .bg(t.card)
            .text_size(px(size::BODY))
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
        // The tip is placed at the pointer and flips up near the window's bottom edge: an equal
        // clear band above and below keeps the visible box off the pointer and its button either way.
        div().py(px(space::S6)).child(
            div()
                .max_w(px(360.))
                .px(px(space::S2))
                .py(px(space::S1))
                .rounded(px(radius::CONTROL))
                .bg(t.text)
                .text_color(t.bg)
                .text_size(px(size::META))
                .line_height(px(16.))
                .font_family(t.ui_font.clone())
                .shadow(t.shadow_overlay())
                .child(self.text.clone()),
        )
    }
}

/// A plain text tooltip: `.tooltip(tip("…"))`.
pub fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| Tip { text: text.clone() }).into()
}

/// Text that truncates to its box and, only when it was cut, shows all of it in a tooltip on hover.
/// Set `truncate()` (and the text style) on the parent; this takes them from there.
pub fn clipped(id: impl Into<ElementId>, text: impl Into<SharedString>) -> InteractiveText {
    let text = text.into();
    let styled = StyledText::new(text.clone());
    let layout = styled.layout().clone();
    InteractiveText::new(id, styled).tooltip(move |_, _, cx| was_cut(&layout, &text).then(|| cx.new(|_| Tip { text: text.clone() }).into()))
}

/// A truncated layout lays out fewer words and an ellipsis, so its text isn't the text given.
fn was_cut(layout: &TextLayout, text: &str) -> bool {
    layout.text() != text
}

struct CardTip {
    title: SharedString,
    summary: Option<String>,
    rows: Vec<(String, String)>,
}

impl Render for CardTip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<Theme>();
        let rows = self.rows.iter().map(|(k, v)| {
            div()
                .flex()
                .gap(px(10.))
                .child(div().flex_none().w(px(52.)).text_color(t.muted).child(k.clone()))
                .child(div().flex_1().min_w_0().text_color(t.text).child(v.clone()))
        });
        // Clear of the pointer and its row, as [`Tip`].
        div().py(px(space::S6)).child(
            div()
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .w(px(340.))
                .px(px(space::S3))
                .py(px(10.))
                .rounded(px(radius::CONTROL))
                .border_1()
                .border_color(t.border)
                .bg(t.card)
                .shadow(t.shadow_overlay())
                .text_size(px(size::META))
                .font_family(t.ui_font.clone())
                .child(div().pb(px(space::S1)).text_size(px(size::BODY)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(self.title.clone()))
                .children(self.summary.clone().map(|s| div().mt(px(-4.)).pb(px(space::S1)).text_color(t.text_2).child(s)))
                .children(rows),
        )
    }
}

/// A hover card: a title, an optional summary line under it, then label · value rows
/// (`.tooltip(card_tip(…))`).
pub fn card_tip(title: impl Into<SharedString>, summary: Option<String>, rows: Vec<(String, String)>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let title = title.into();
    move |_, cx| cx.new(|_| CardTip { title: title.clone(), summary: summary.clone(), rows: rows.clone() }).into()
}

// ------------------------------------------------------------------ line icons

/// The web board's inline SVG icons (`ICON.*` in `app.js`), drawn on a 24-unit grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Glyph {
    /// `ICON.x` (stroke 2.2).
    X,
    /// A magnifier (stroke 2).
    Search,
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
            match g {
                Glyph::X => {
                    stroke(&[(6., 6.), (18., 18.)], 2.2, window);
                    stroke(&[(18., 6.), (6., 18.)], 2.2, window);
                }
                Glyph::Search => {
                    let ring: Vec<(f32, f32)> = (0..=32).map(|i| i as f32 * std::f32::consts::TAU / 32.).map(|a| (11. + 7. * a.cos(), 11. + 7. * a.sin())).collect();
                    stroke(&ring, 2., window);
                    stroke(&[(16.2, 16.2), (20.5, 20.5)], 2., window);
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
    /// The same chevron pointing right, for a closed group.
    ChevRight,
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
                Icon::ChevRight => poly(&[(9., 6.), (15., 12.), (9., 18.)], 2.2, window),
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
    use gpui_kit::{Context, IntoElement, ParentElement, Render, Styled, StyledText, TestAppContext, TextLayout, Window, div, px};

    const WORDS: &str = "Open an X Money account and save the sign-in in Keychain";

    struct Boxes(TextLayout, TextLayout);

    impl Render for Boxes {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let (narrow, wide) = (StyledText::new(WORDS), StyledText::new(WORDS));
            (self.0, self.1) = (narrow.layout().clone(), wide.layout().clone());
            div().text_size(px(13.)).child(div().w(px(80.)).truncate().child(narrow)).child(div().w(px(900.)).truncate().child(wide))
        }
    }

    #[gpui_kit::test]
    fn only_words_cut_off_offer_the_rest(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| Boxes(StyledText::new("").layout().clone(), StyledText::new("").layout().clone()));
        cx.update(|window, cx| drop(window.draw(cx)));
        let (narrow, wide) = view.read_with(cx, |b, _| (b.0.clone(), b.1.clone()));
        assert!(super::was_cut(&narrow, WORDS), "{}", narrow.text());
        assert!(!super::was_cut(&wide, WORDS), "{}", wide.text());
    }

    #[::core::prelude::v1::test]
    fn menus_open_under_their_button_and_flip_above_it() {
        use super::{Align, menu_spot};
        use gpui_kit::{Anchor, Bounds, point, size};
        let button = Bounds::new(point(px(100.), px(50.)), size(px(80.), px(32.)));
        let (below, above, corner) = menu_spot(button, Align::Right);
        assert_eq!((below, above, corner), (point(px(180.), px(86.)), point(px(180.), px(46.)), Anchor::TopRight));
        let (below, above, corner) = menu_spot(button, Align::Left);
        assert_eq!((below, above, corner), (point(px(100.), px(86.)), point(px(100.), px(46.)), Anchor::TopLeft));
    }

    #[::core::prelude::v1::test]
    fn status_pills_stay_shorter_than_the_smallest_button() {
        use crate::theme::height;
        assert!(height::TAG < height::SMALL && height::SMALL < height::CONTROL);
    }

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
