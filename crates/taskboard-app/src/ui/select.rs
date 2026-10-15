//! Text the reader can select with the mouse and copy with ⌘C.
//!
//! Every [`Sel`] joins the window's one selection (`TextSelectionLayer`, drawn first in the main
//! window), so a drag can run across a title, a quote and a table's values and copy them in
//! reading order. Unlike gpui-base's `SelectableText`, a [`Sel`] keeps highlights (a bold name,
//! markdown's `code` and *italic*) and opens its links on a plain click.
use crate::theme::Theme;
use gpui_kit::base::{TextSelection, TextSelectionHandle, TextSelectionRegistration, TextSelectionRun};
use gpui_kit::*;
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Paint order, which is reading order: a copy joins the runs in this order. It only climbs, so
/// each frame's runs sort after the last frame's and in the order they were drawn.
static ORDER: AtomicU64 = AtomicU64::new(1);

/// Selectable text: `sel(t, "issue-title", title)`, styled by its parent like any text.
pub fn sel(t: &Theme, id: impl Into<ElementId>, text: impl Into<SharedString>) -> Sel {
    Sel::new(t, id, text, Vec::new())
}

/// Selectable text with some of it highlighted.
pub fn sel_styled(t: &Theme, id: impl Into<ElementId>, text: impl Into<SharedString>, highlights: Vec<(Range<usize>, HighlightStyle)>) -> Sel {
    Sel::new(t, id, text, highlights)
}

pub struct Sel {
    id: ElementId,
    text: SharedString,
    styled: StyledText,
    color: Hsla,
    links: Rc<Vec<(Range<usize>, String)>>,
    order: u64,
}

/// What a [`Sel`] keeps between frames: its selection handle, the link a press went down on and
/// the link the mouse is over.
#[derive(Clone)]
pub struct Kept {
    handle: TextSelectionHandle,
    pressed: Rc<Cell<Option<usize>>>,
    hovered: Rc<Cell<Option<usize>>>,
}

impl Sel {
    fn new(t: &Theme, id: impl Into<ElementId>, text: impl Into<SharedString>, highlights: Vec<(Range<usize>, HighlightStyle)>) -> Self {
        let text: SharedString = text.into();
        Self { id: id.into(), styled: StyledText::new(text.clone()).with_highlights(highlights), text, color: t.accent.opacity(0.3), links: Rc::new(Vec::new()), order: 0 }
    }

    /// Byte ranges that open a URL on a click that didn't drag.
    pub fn links(mut self, links: Vec<(Range<usize>, String)>) -> Self {
        self.links = Rc::new(links);
        self
    }

    fn link_at(links: &[(Range<usize>, String)], layout: &TextLayout, at: Point<Pixels>) -> Option<usize> {
        let ix = layout.index_for_position(at).ok()?;
        links.iter().position(|(r, _)| r.contains(&ix))
    }
}

impl IntoElement for Sel {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Sel {
    type RequestLayoutState = Kept;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, id: Option<&GlobalElementId>, inspector: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, Kept) {
        let kept = window.with_element_state(id.expect("Sel has an id"), |kept: Option<Kept>, _| {
            let kept = kept.unwrap_or_else(|| Kept { handle: TextSelectionHandle::new(self.text.clone(), cx), pressed: Rc::default(), hovered: Rc::default() });
            (kept.clone(), kept)
        });
        let (layout, ()) = self.styled.request_layout(id, inspector, window, cx);
        (layout, kept)
    }

    fn prepaint(&mut self, id: Option<&GlobalElementId>, inspector: Option<&InspectorElementId>, bounds: Bounds<Pixels>, kept: &mut Kept, window: &mut Window, cx: &mut App) -> Hitbox {
        self.styled.prepaint(id, inspector, bounds, &mut (), window, cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        self.order = ORDER.fetch_add(1, Ordering::Relaxed);
        let registration = TextSelectionRegistration::new(hitbox.clone(), bounds).with_document_order(self.order).with_text_bounds(vec![bounds]).with_rendered_element(&kept.handle, window, cx);
        kept.handle.register(registration, window, cx);
        hitbox
    }

    fn paint(&mut self, id: Option<&GlobalElementId>, inspector: Option<&InspectorElementId>, bounds: Bounds<Pixels>, kept: &mut Kept, hitbox: &mut Hitbox, window: &mut Window, cx: &mut App) {
        let layout = self.styled.layout().clone();
        let before = TextSelection::selected_text(window, cx);
        let projection = kept.handle.update_runs(&[TextSelectionRun::new(self.text.clone(), layout.clone(), bounds).with_document_order(self.order)], cx);
        if before != TextSelection::selected_text(window, cx) {
            window.refresh();
        }
        for range in projection.ranges().iter().flatten() {
            paint_selection(&layout, range.clone(), self.color, window);
        }
        self.styled.paint(id, inspector, bounds, &mut (), &mut (), window, cx);
        if self.links.is_empty() {
            window.set_cursor_style(CursorStyle::IBeam, hitbox);
            return;
        }
        let over = hitbox.is_hovered(window).then(|| Self::link_at(&self.links, &layout, window.mouse_position())).flatten();
        window.set_cursor_style(if over.is_some() { CursorStyle::PointingHand } else { CursorStyle::IBeam }, hitbox);
        kept.hovered.set(over);
        let (links, hb, l) = (self.links.clone(), hitbox.clone(), layout.clone());
        let hovered = kept.hovered.clone();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, _| {
            if phase.bubble() {
                let now = hb.is_hovered(window).then(|| Self::link_at(&links, &l, e.position)).flatten();
                if now != hovered.get() {
                    window.refresh();
                }
            }
        });
        let (links, hb, l) = (self.links.clone(), hitbox.clone(), layout.clone());
        let pressed = kept.pressed.clone();
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, _| {
            if phase.bubble() && e.button == MouseButton::Left {
                pressed.set(hb.is_hovered(window).then(|| Self::link_at(&links, &l, e.position)).flatten());
            }
        });
        let (links, hb) = (self.links.clone(), hitbox.clone());
        let pressed = kept.pressed.clone();
        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
            if !phase.bubble() || e.button != MouseButton::Left {
                return;
            }
            let down = pressed.take();
            let up = hb.is_hovered(window).then(|| Self::link_at(&links, &layout, e.position)).flatten();
            if let (Some(a), Some(b)) = (down, up)
                && a == b
                && !TextSelection::has_selection(window, cx)
            {
                cx.open_url(&links[a].1);
            }
        });
    }
}

fn paint_selection(layout: &TextLayout, range: Range<usize>, color: Hsla, window: &mut Window) {
    let (Some(start), Some(end)) = (layout.position_for_index(range.start), layout.position_for_index(range.end)) else {
        return;
    };
    let (b, lh) = (layout.bounds(), layout.line_height());
    let mut quads = Vec::new();
    if start.y == end.y {
        quads.push(Bounds::from_corners(start, point(end.x, end.y + lh)));
    } else {
        quads.push(Bounds::from_corners(start, point(b.right(), start.y + lh)));
        if end.y > start.y + lh {
            quads.push(Bounds::from_corners(point(b.left(), start.y + lh), point(b.right(), end.y)));
        }
        quads.push(Bounds::from_corners(point(b.left(), end.y), point(end.x, end.y + lh)));
    }
    for q in quads {
        window.paint_quad(fill(q, color));
    }
}

/// ⌘C outside a text field: the window's selection, when there is one.
pub fn copy(window: &mut Window, cx: &mut App) {
    let text = TextSelection::selected_text(window, cx);
    if !text.is_empty() {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
}

#[cfg(test)]
mod tests {
    use super::sel;
    use crate::theme::{Theme, ThemeMode};
    use gpui_kit::base::{TextSelection, TextSelectionLayer};
    use gpui_kit::{Context, IntoElement, Modifiers, MouseButton, ParentElement as _, Render, InteractiveElement as _, Styled as _, TestAppContext, VisualTestContext, Window, div, point, px};

    struct View;

    impl Render for View {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let t = cx.global::<Theme>().clone();
            // Two runs on the page, and one in a deferred, occluding panel like the issue panel.
            let panel = div().id("panel").absolute().top(px(100.)).left_0().w(px(300.)).h(px(40.)).occlude().child(sel(&t, "c", "delta epsilon"));
            div()
                .size_full()
                .child(TextSelectionLayer)
                .child(div().flex().flex_col().w(px(240.)).child(div().h(px(20.)).child(sel(&t, "a", "alpha beta"))).child(div().h(px(20.)).child(sel(&t, "b", "gamma"))))
                .child(gpui_kit::deferred(panel))
        }
    }

    #[gpui_kit::test]
    fn a_drag_selects_across_runs(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::new(ThemeMode::Dark, "Helvetica".into(), "Menlo".into())));
        let (_, cx) = cx.add_window_view(|_, _| View);
        cx.update(|window, cx| drop(window.draw(cx)));
        cx.simulate_mouse_down(point(px(1.), px(10.)), MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(point(px(200.), px(30.)), Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(point(px(200.), px(30.)), MouseButton::Left, Modifiers::default());
        cx.update(|window, cx| {
            drop(window.draw(cx));
            assert_eq!(TextSelection::selected_text(window, cx), "alpha beta\ngamma");
        });
        cx.simulate_mouse_down(point(px(1.), px(110.)), MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(point(px(290.), px(110.)), Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(point(px(290.), px(110.)), MouseButton::Left, Modifiers::default());
        cx.update(|window, cx| {
            drop(window.draw(cx));
            assert_eq!(TextSelection::selected_text(window, cx), "delta epsilon");
        });
    }

    #[gpui_kit::test]
    fn an_issue_title_selects_and_copies(cx: &mut TestAppContext) {
        let (w, _rec) = crate::parity::window(cx);
        w.update(cx, |m, _, cx| m.open_issue("B1", cx)).unwrap();
        crate::parity::settle(cx);
        let title = w.update(cx, |m, _, _| m.data.issue.as_ref().and_then(|b| b["title"].as_str()).unwrap_or_default().to_string()).unwrap();
        assert!(!title.is_empty(), "B1 loaded");
        let cx = &mut VisualTestContext::from_window(w.into(), cx);
        let width = cx.update(|window, cx| {
            drop(window.draw(cx));
            window.viewport_size().width
        });
        // The panel's title: 24px in from the panel's left edge, under the pills.
        let left = width - px(crate::ui::issue_panel::WIDTH) + px(24.);
        cx.simulate_mouse_down(point(left + px(2.), px(84.)), MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(point(left + px(80.), px(84.)), Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(point(left + px(80.), px(84.)), MouseButton::Left, Modifiers::default());
        let picked = cx.update(|window, cx| {
            drop(window.draw(cx));
            TextSelection::selected_text(window, cx)
        });
        assert!(!picked.is_empty() && title.starts_with(&picked), "selected {picked:?} of {title:?}");
        cx.dispatch_action(crate::app::CopySelection);
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()), Some(picked));
    }
}
