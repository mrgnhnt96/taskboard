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
