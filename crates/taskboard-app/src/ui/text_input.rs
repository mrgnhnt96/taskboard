//! A shared single-line text field with IME (`EntityInputHandler`), a cursor, selection and
//! the usual macOS editing keys. Used by the ⌘K palette, the Rules test strip, the Triggers
//! secret field and the Settings Ask box (through `screen_kit::LineInput`).
//!
//! Typing arrives through GPUI's input handler (so dead keys, option characters and IME
//! composition work, with the candidate window anchored at the cursor). The field's own key
//! handler takes the editing keys (arrows, ⌥/⌘ word and line moves, ⇧ selection, ⌫/⌦, ⌘A/C/X/V);
//! everything else (↩, esc, ⇥, ↑/↓ outside `wrap`, ⌘-shortcuts) bubbles to the parent, which decides what
//! submit/cancel mean. Font, size and color are inherited from the parent element.
//!
//! With `wrap` set (the image sheet's note field) the text wraps at the field's width and the
//! field grows a line at a time instead of scrolling sideways. The [`Newline`] action
//! (`keys.note_newline`, ⇧↩ by default) starts a new line there; ↩ stays the parent's. ↑/↓
//! (⇧ to select) move a row there, to the start or end past the first or last row; ⌘↑/↓ still
//! bubble.
use crate::theme::Theme;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::ops::Range;

actions!(
    taskboard,
    [
        /// Start a new line in a wrapping field (`keys.note_newline`).
        Newline,
    ]
);

/// Key context of a wrapping field, where [`Newline`] is bound.
pub const CTX_WRAP: &str = "TextFieldWrap";

/// Emitted on every content change.
pub struct FieldChanged;

pub struct TextField {
    pub focus: FocusHandle,
    content: String,
    /// Byte range into `content`.
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    pub placeholder: SharedString,
    /// Draw bullets instead of the text (and never copy it out).
    pub secret: bool,
    /// Wrap at the field's width (growing taller) instead of scrolling sideways, and keep
    /// newlines (⇧↩, pastes).
    pub wrap: bool,
    last_layout: Option<Shaped>,
    last_line_h: Pixels,
    last_bounds: Option<Bounds<Pixels>>,
    selecting: bool,
}

impl EventEmitter<FieldChanged> for TextField {}

impl TextField {
    pub fn new(cx: &mut Context<Self>, secret: bool, placeholder: impl Into<SharedString>) -> TextField {
        TextField {
            focus: cx.focus_handle(),
            content: String::new(),
            selected: 0..0,
            reversed: false,
            marked: None,
            placeholder: placeholder.into(),
            secret,
            wrap: false,
            last_layout: None,
            last_line_h: px(0.),
            last_bounds: None,
            selecting: false,
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    /// Replace the whole text (cursor at the end).
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.content == text {
            return;
        }
        self.wipe();
        self.content = text.to_string();
        self.selected = self.content.len()..self.content.len();
        self.marked = None;
        cx.emit(FieldChanged);
        cx.notify();
    }

    /// Select the whole text (typing replaces it).
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selected = 0..self.content.len();
        self.reversed = false;
        cx.notify();
    }

    /// Clear, overwriting the old buffer first so a secret doesn't linger in memory.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if self.content.is_empty() {
            return;
        }
        self.wipe();
        self.selected = 0..0;
        self.marked = None;
        cx.emit(FieldChanged);
        cx.notify();
    }

    fn wipe(&mut self) {
        let n = self.content.len();
        self.content.clear();
        self.content.extend(std::iter::repeat_n('\0', n));
        self.content.clear();
    }

    fn cursor(&self) -> usize {
        if self.reversed { self.selected.start } else { self.selected.end }
    }

    fn move_to(&mut self, off: usize, cx: &mut Context<Self>) {
        self.selected = off..off;
        self.reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, off: usize, cx: &mut Context<Self>) {
        if self.reversed {
            self.selected.start = off;
        } else {
            self.selected.end = off;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        cx.notify();
    }

    fn prev_char(&self, off: usize) -> usize {
        self.content[..off].char_indices().next_back().map(|(i, _)| i).unwrap_or(0)
    }

    fn next_char(&self, off: usize) -> usize {
        self.content[off..].chars().next().map(|c| off + c.len_utf8()).unwrap_or(self.content.len())
    }

    /// Start of the word before `off` (skipping spaces first), like ⌥←.
    fn prev_word(&self, off: usize) -> usize {
        word_left(&self.content, off)
    }

    fn next_word(&self, off: usize) -> usize {
        word_right(&self.content, off)
    }

    /// The offset one drawn row above (`up`) or below `off` at the same x, from the last paint:
    /// the start of the text above the first row, its end below the last. `None` when not wrapping.
    fn vertical(&self, off: usize, up: bool) -> Option<usize> {
        let line = self.last_layout.as_ref().filter(|_| self.wrap)?;
        let Shaped::Wrapped(ps) = line else { return None };
        let lh = self.last_line_h;
        let at = line.pos(self.display_offset(off), lh);
        let y = at.y + lh * 0.5 + if up { -lh } else { lh };
        let rows: usize = ps.iter().map(Para::rows).sum();
        Some(if y < px(0.) {
            0
        } else if y >= lh * rows as f32 {
            self.content.len()
        } else {
            self.content_offset(line.closest(point(at.x, y), lh))
        })
    }

    fn replace(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        let text = if self.wrap { text.replace("\r\n", "\n").replace('\r', "\n") } else { text.replace(['\r', '\n'], " ") };
        self.content.replace_range(range.clone(), &text);
        let end = range.start + text.len();
        self.selected = end..end;
        self.reversed = false;
        self.marked = None;
        cx.emit(FieldChanged);
        cx.notify();
    }

    fn newline(&mut self, _: &Newline, _w: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return;
        }
        let r = self.selected.clone();
        self.replace(r, "\n", cx);
    }

    fn delete_to(&mut self, off: usize, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            let r = self.selected.clone();
            self.replace(r, "", cx);
            return;
        }
        let c = self.cursor();
        let r = if off < c { off..c } else { c..off };
        if !r.is_empty() {
            self.replace(r, "", cx);
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        // While the IME composes, every key belongs to it.
        if self.marked.is_some() {
            return;
        }
        let c = self.cursor();
        let len = self.content.len();
        let target = |s: &Self, left: bool| -> usize {
            match (left, m.platform, m.alt) {
                (true, true, _) => 0,
                (false, true, _) => len,
                (true, _, true) => s.prev_word(c),
                (false, _, true) => s.next_word(c),
                (true, _, _) => s.prev_char(c),
                (false, _, _) => s.next_char(c),
            }
        };
        match ks.key.as_str() {
            "left" | "right" if !m.control => {
                let left = ks.key == "left";
                if m.shift {
                    let t = target(self, left);
                    self.select_to(t, cx);
                } else if !self.selected.is_empty() && !m.platform && !m.alt {
                    let edge = if left { self.selected.start } else { self.selected.end };
                    self.move_to(edge, cx);
                } else {
                    let t = target(self, left);
                    self.move_to(t, cx);
                }
            }
            "up" | "down" if self.wrap && !m.platform && !m.control && !m.alt => {
                let up = ks.key == "up";
                // Without ⇧ a selection collapses from its edge on that side.
                let from = if m.shift || self.selected.is_empty() {
                    c
                } else if up {
                    self.selected.start
                } else {
                    self.selected.end
                };
                let Some(t) = self.vertical(from, up) else { return };
                if m.shift { self.select_to(t, cx) } else { self.move_to(t, cx) }
            }
            "home" => self.move_to(0, cx),
            "end" => self.move_to(len, cx),
            "a" if m.control => self.move_to(0, cx),
            "e" if m.control => self.move_to(len, cx),
            "k" if m.control => {
                let r = c..len;
                if !r.is_empty() {
                    self.replace(r, "", cx);
                }
            }
            "backspace" => {
                let to = if m.platform {
                    0
                } else if m.alt {
                    self.prev_word(c)
                } else {
                    self.prev_char(c)
                };
                self.delete_to(to, cx);
            }
            "delete" => {
                let to = if m.alt { self.next_word(c) } else { self.next_char(c) };
                self.delete_to(to, cx);
            }
            "a" if m.platform => {
                self.selected = 0..len;
                self.reversed = false;
                cx.notify();
            }
            "c" | "x" if m.platform => {
                if self.secret || self.selected.is_empty() {
                    return; // let the parent's copy (if any) run
                }
                cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected.clone()].to_string()));
                if ks.key == "x" {
                    let r = self.selected.clone();
                    self.replace(r, "", cx);
                }
            }
            "v" if m.platform => {
                if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    let r = self.selected.clone();
                    self.replace(r, t.trim_end_matches(['\r', '\n']), cx);
                }
            }
            _ => return, // not ours: typing goes to the IME handler, the rest to the parent
        }
        let _ = window;
        cx.stop_propagation();
    }

    // ------------------------------------------------------------------ utf16 / display

    fn to_utf16(&self, off: usize) -> usize {
        self.content[..off.min(self.content.len())].encode_utf16().count()
    }

    fn utf16_to_byte(&self, u: usize) -> usize {
        let mut n = 0;
        for (i, ch) in self.content.char_indices() {
            if n >= u {
                return i;
            }
            n += ch.len_utf16();
        }
        self.content.len()
    }

    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.to_utf16(r.start)..self.to_utf16(r.end)
    }

    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.utf16_to_byte(r.start)..self.utf16_to_byte(r.end)
    }

    /// What is drawn, and a map from content offsets to drawn offsets.
    fn display(&self) -> String {
        if self.secret { "•".repeat(self.content.chars().count()) } else { self.content.clone() }
    }

    fn display_offset(&self, off: usize) -> usize {
        if self.secret { self.content[..off.min(self.content.len())].chars().count() * '•'.len_utf8() } else { off }
    }

    fn content_offset(&self, disp: usize) -> usize {
        if !self.secret {
            return disp.min(self.content.len());
        }
        let n = disp / '•'.len_utf8();
        self.content.char_indices().nth(n).map(|(i, _)| i).unwrap_or(self.content.len())
    }

    fn index_for_point(&self, p: Point<Pixels>) -> usize {
        let (Some(b), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref()) else {
            return self.content.len();
        };
        if self.content.is_empty() {
            return 0;
        }
        self.content_offset(line.closest(p - b.origin, self.last_line_h))
    }
}

/// The shaped text from the last paint: one line, or paragraphs wrapped into rows.
enum Shaped {
    Line(ShapedLine),
    Wrapped(Vec<Para>),
}

/// One paragraph of wrapped text: where it starts in the text and on screen.
struct Para {
    start: usize,
    y: Pixels,
    line: WrappedLine,
}

impl Para {
    fn rows(&self) -> usize {
        self.line.wrap_boundaries().len() + 1
    }
}

impl Shaped {
    /// Top-left of the cursor slot before display offset `i`, relative to the text's origin.
    fn pos(&self, i: usize, line_h: Pixels) -> Point<Pixels> {
        match self {
            Shaped::Line(l) => point(l.x_for_index(i), px(0.)),
            Shaped::Wrapped(ps) => {
                let Some(p) = ps.iter().rev().find(|p| p.start <= i) else { return point(px(0.), px(0.)) };
                let at = p.line.position_for_index(i - p.start, line_h).unwrap_or_else(|| point(p.line.width(), px(0.)));
                point(at.x, p.y + at.y)
            }
        }
    }

    /// The paragraph under `y` (the first or last one when above or below the text).
    fn para_at(ps: &[Para], y: Pixels, line_h: Pixels) -> Option<&Para> {
        ps.iter().find(|p| y < p.y + line_h * p.rows() as f32).or(ps.last())
    }

    fn closest(&self, p: Point<Pixels>, line_h: Pixels) -> usize {
        match self {
            Shaped::Line(l) => l.closest_index_for_x(p.x),
            Shaped::Wrapped(ps) => Self::para_at(ps, p.y, line_h)
                .map_or(0, |q| q.start + q.line.closest_index_for_position(point(p.x, (p.y - q.y).max(px(0.))), line_h).unwrap_or_else(|i| i)),
        }
    }

    fn index(&self, p: Point<Pixels>, line_h: Pixels) -> Option<usize> {
        match self {
            Shaped::Line(l) => l.index_for_x(p.x),
            Shaped::Wrapped(ps) => {
                let q = Self::para_at(ps, p.y, line_h)?;
                q.line.index_for_position(point(p.x, p.y - q.y), line_h).ok().map(|i| q.start + i)
            }
        }
    }

    fn paint(&self, origin: Point<Pixels>, line_h: Pixels, window: &mut Window, cx: &mut App) {
        match self {
            Shaped::Line(l) => {
                let _ = l.paint(origin, line_h, TextAlign::Left, None, window, cx);
            }
            Shaped::Wrapped(ps) => {
                for p in ps {
                    let _ = p.line.paint(origin + point(px(0.), p.y), line_h, TextAlign::Left, None, window, cx);
                }
            }
        }
    }
}

/// The double-clicked word at byte offset `off`: a run of word characters, of spaces, or the
/// one other character there (the one before it at the end of a line).
pub fn word_at(s: &str, off: usize) -> Range<usize> {
    let class = |c: char| if c.is_alphanumeric() || c == '_' { 0 } else if c == ' ' || c == '\t' { 1 } else { 2 };
    let at = s[off..].chars().next().filter(|&c| c != '\n').map(|c| (off, c)).or_else(|| s[..off].char_indices().next_back().filter(|&(_, c)| c != '\n'));
    let Some((i, c)) = at else { return off..off };
    let k = class(c);
    if k == 2 {
        return i..i + c.len_utf8();
    }
    let start = s[..i].char_indices().rev().take_while(|&(_, c)| class(c) == k).last().map_or(i, |(j, _)| j);
    let end = s[i..].char_indices().find(|&(_, c)| class(c) != k).map_or(s.len(), |(j, _)| i + j);
    start..end
}

/// Byte offset of the start of the word left of `off` (spaces skipped first). Right after
/// punctuation (`foo.`, `--`), that run of punctuation is the word, so it always moves.
pub fn word_left(s: &str, off: usize) -> usize {
    let sep = |c: char| "/.-_:=,".contains(c);
    let trimmed = s[..off].trim_end();
    let word = trimmed.trim_end_matches(sep);
    if word.len() < trimmed.len() {
        return word.len();
    }
    trimmed.rfind(|c: char| c.is_whitespace() || sep(c)).map(|i| i + trimmed[i..].chars().next().map_or(1, char::len_utf8)).unwrap_or(0)
}

/// Byte offset of the end of the word right of `off`.
pub fn word_right(s: &str, off: usize) -> usize {
    let after = &s[off..];
    let skip = after.len() - after.trim_start().len();
    let rest = &after[skip..];
    let word =
        rest.find(|c: char| c.is_whitespace() || "/.-_:=,".contains(c)).map(|i| if i == 0 { rest.chars().next().map_or(0, char::len_utf8) } else { i }).unwrap_or(rest.len());
    off + skip + word
}

impl EntityInputHandler for TextField {
    fn text_for_range(&mut self, r: Range<usize>, actual: &mut Option<Range<usize>>, _w: &mut Window, _cx: &mut Context<Self>) -> Option<String> {
        if self.secret {
            return None;
        }
        let range = self.range_from_utf16(&r);
        actual.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(&mut self, _ignore: bool, _w: &mut Window, _cx: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.selected), reversed: self.reversed })
    }

    fn marked_text_range(&self, _w: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _w: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, r: Option<Range<usize>>, text: &str, _w: &mut Window, cx: &mut Context<Self>) {
        let range = r.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked.clone()).unwrap_or(self.selected.clone());
        self.replace(range, text, cx);
    }

    fn replace_and_mark_text_in_range(&mut self, r: Option<Range<usize>>, text: &str, sel: Option<Range<usize>>, _w: &mut Window, cx: &mut Context<Self>) {
        let range = r.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked.clone()).unwrap_or(self.selected.clone());
        self.content.replace_range(range.clone(), text);
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        self.selected = sel
            .as_ref()
            .map(|s| {
                // `sel` is relative to the marked text, in UTF-16.
                let sub = &text.encode_utf16().collect::<Vec<_>>();
                let to8 = |u: usize| String::from_utf16_lossy(&sub[..u.min(sub.len())]).len();
                range.start + to8(s.start)..range.start + to8(s.end)
            })
            .unwrap_or(range.start + text.len()..range.start + text.len());
        cx.emit(FieldChanged);
        cx.notify();
    }

    fn bounds_for_range(&mut self, r: Range<usize>, b: Bounds<Pixels>, _w: &mut Window, _cx: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&r);
        let h = self.last_line_h;
        let p0 = line.pos(self.display_offset(range.start), h);
        let p1 = line.pos(self.display_offset(range.end), h);
        let x1 = if p1.y == p0.y { p1.x } else { p0.x };
        Some(Bounds::from_corners(point(b.left() + p0.x, b.top() + p0.y), point(b.left() + x1, b.top() + p0.y + h)))
    }

    fn character_index_for_point(&mut self, p: Point<Pixels>, _w: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        let b = self.last_bounds?;
        let line = self.last_layout.as_ref()?;
        let i = line.index(p - b.origin, self.last_line_h)?;
        Some(self.to_utf16(self.content_offset(i)))
    }
}

impl Focusable for TextField {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TextField {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("text-field")
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .track_focus(&self.focus)
            .when(self.wrap, |d| d.key_context(CTX_WRAP).on_action(cx.listener(Self::newline)))
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|f, ev: &MouseDownEvent, window, cx| {
                    f.focus.focus(window, cx);
                    f.selecting = true;
                    let i = f.index_for_point(ev.position);
                    if ev.modifiers.shift {
                        f.select_to(i, cx);
                    } else if ev.click_count == 2 && !f.secret {
                        f.selected = word_at(&f.content, i);
                        f.reversed = false;
                        cx.notify();
                    } else if ev.click_count >= 2 {
                        f.selected = 0..f.content.len();
                        f.reversed = false;
                        cx.notify();
                    } else {
                        f.move_to(i, cx);
                    }
                }),
            )
            .on_mouse_up(MouseButton::Left, cx.listener(|f, _: &MouseUpEvent, _, _| f.selecting = false))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|f, _: &MouseUpEvent, _, _| f.selecting = false))
            .on_mouse_move(cx.listener(|f, ev: &MouseMoveEvent, _, cx| {
                if f.selecting {
                    let i = f.index_for_point(ev.position);
                    f.select_to(i, cx);
                }
            }))
            .child(TextLine { field: cx.entity() })
    }
}

/// The painted text: text (or placeholder), IME underline, selection and cursor.
struct TextLine {
    field: Entity<TextField>,
}

struct LinePrepaint {
    line: Option<Shaped>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
    scroll: Pixels,
    line_h: Pixels,
}

impl IntoElement for TextLine {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl TextLine {
    /// What to shape: the shown text (or the placeholder) and its runs, with the IME's marked
    /// text underlined.
    fn text_and_runs(&self, window: &Window, cx: &App) -> (String, Vec<TextRun>) {
        let theme = cx.global::<Theme>();
        let f = self.field.read(cx);
        let style = window.text_style();
        let shown = f.display();
        let (text, color) = if shown.is_empty() { (f.placeholder.to_string(), theme.dim) } else { (shown, style.color) };
        let run = TextRun { len: text.len(), font: style.font(), color, background_color: None, underline: None, strikethrough: None };
        let runs = match f.marked.as_ref().filter(|_| !f.content.is_empty()) {
            Some(m) => {
                let (a, b) = (f.display_offset(m.start), f.display_offset(m.end));
                let ul = Some(UnderlineStyle { color: Some(color), thickness: px(1.), wavy: false });
                vec![TextRun { len: a, ..run.clone() }, TextRun { len: b - a, underline: ul, ..run.clone() }, TextRun { len: text.len() - b, ..run }]
                    .into_iter()
                    .filter(|r| r.len > 0)
                    .collect()
            }
            None => vec![run],
        };
        (text, runs)
    }
}

/// `text` split at its newlines, each paragraph wrapped at `width` and placed below the last.
fn wrapped(window: &Window, text: String, size: Pixels, runs: &[TextRun], width: Pixels, line_h: Pixels) -> Vec<Para> {
    let lines = window.text_system().shape_text(SharedString::from(text), size, runs, Some(width), None).unwrap_or_default();
    let (mut start, mut y) = (0, px(0.));
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let p = Para { start, y, line };
        start += p.line.len() + 1;
        y += line_h * p.rows() as f32;
        out.push(p);
    }
    out
}

impl Element for TextLine {
    type RequestLayoutState = ();
    type PrepaintState = LinePrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _id: Option<&GlobalElementId>, _i: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let lh = line_h(window);
        if !self.field.read(cx).wrap {
            style.size.height = lh.into();
            return (window.request_layout(style, [], cx), ());
        }
        // As tall as the text's rows at the width the layout settles on.
        let (text, runs) = self.text_and_runs(window, cx);
        let size = window.text_style().font_size.to_pixels(window.rem_size());
        let id = window.request_measured_layout(style, move |known, avail, window, _cx| {
            let width = known.width.unwrap_or(match avail.width {
                AvailableSpace::Definite(w) => w,
                _ => px(f32::MAX),
            });
            let rows: usize = wrapped(window, text.clone(), size, &runs, width, lh).iter().map(Para::rows).sum();
            gpui_kit::size(width, lh * rows.max(1) as f32)
        });
        (id, ())
    }

    fn prepaint(&mut self, _id: Option<&GlobalElementId>, _i: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _r: &mut (), window: &mut Window, cx: &mut App) -> LinePrepaint {
        let theme = cx.global::<Theme>().clone();
        let (text, runs) = self.text_and_runs(window, cx);
        let f = self.field.read(cx);
        let size = window.text_style().font_size.to_pixels(window.rem_size());
        let wrap = f.wrap;
        let line_h = if wrap { line_h(window) } else { bounds.size.height };
        let line = if wrap {
            Shaped::Wrapped(wrapped(window, text, size, &runs, bounds.size.width, line_h))
        } else {
            Shaped::Line(window.text_system().shape_line(SharedString::from(text), size, &runs, None))
        };
        let empty = f.content.is_empty();
        let pos_of = |off: usize| {
            if empty { point(px(0.), px(0.)) } else { line.pos(f.display_offset(off), line_h) }
        };
        let cur = pos_of(f.cursor());
        // Keep the cursor visible in a narrow single-line field: scroll the line left.
        let width = bounds.size.width - px(2.);
        let scroll = if !wrap && cur.x > width { cur.x - width } else { px(0.) };
        let at = |p: Point<Pixels>| point(bounds.left() + p.x - scroll, bounds.top() + p.y);
        let (cursor, selection) = if f.selected.is_empty() {
            (Some(fill(Bounds::new(at(cur) + point(px(0.), line_h * 0.12), size_of(px(1.5), line_h * 0.76)), theme.accent)), vec![])
        } else {
            let (a, b) = (pos_of(f.selected.start), pos_of(f.selected.end));
            let tint = theme.accent.opacity(0.3);
            let row = |y: Pixels, x0: Pixels, x1: Pixels| fill(Bounds::from_corners(at(point(x0, y)), at(point(x1, y + line_h))), tint);
            let quads = if a.y == b.y {
                vec![row(a.y, a.x, b.x)]
            } else {
                // First row to the edge, whole rows between, the last row up to the end.
                let mut q = vec![row(a.y, a.x, bounds.size.width)];
                let mut y = a.y + line_h;
                while y < b.y {
                    q.push(row(y, px(0.), bounds.size.width));
                    y += line_h;
                }
                q.push(row(b.y, px(0.), b.x));
                q
            };
            (None, quads)
        };
        LinePrepaint { line: Some(line), cursor, selection, scroll, line_h }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _i: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _r: &mut (),
        pp: &mut LinePrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.field.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.field.clone()), cx);
        let Some(line) = pp.line.take() else { return };
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for s in pp.selection.drain(..) {
                window.paint_quad(s);
            }
            line.paint(point(bounds.left() - pp.scroll, bounds.top()), pp.line_h, window, cx);
            if focus.is_focused(window)
                && let Some(c) = pp.cursor.take()
            {
                window.paint_quad(c);
            }
        });
        let shifted = Bounds::new(point(bounds.left() - pp.scroll, bounds.top()), bounds.size);
        let line_h = pp.line_h;
        self.field.update(cx, |f, _| {
            f.last_layout = Some(line);
            f.last_bounds = Some(shifted);
            f.last_line_h = line_h;
        });
    }
}

/// Tall enough for descenders whatever line height the parent set.
fn line_h(window: &Window) -> Pixels {
    let fs = window.text_style().font_size.to_pixels(window.rem_size());
    window.line_height().max(fs * 1.4)
}

fn size_of(w: Pixels, h: Pixels) -> Size<Pixels> {
    size(w, h)
}

#[cfg(test)]
mod tests {
    use super::{word_at, word_left, word_right};

    #[test]
    fn word_moves() {
        let s = "git push --force origin";
        assert_eq!(word_left(s, s.len()), 17);
        assert_eq!(word_left(s, 17), 11);
        assert_eq!(word_left(s, 4), 0);
        assert_eq!(word_right(s, 0), 3);
        assert_eq!(word_right(s, 3), 8);
        assert_eq!(word_left("héllo wörld", "héllo wörld".len()), "héllo ".len());
        // Right after punctuation, ⌥⌫ takes the punctuation instead of nothing.
        assert_eq!(word_left(s, 11), 9);
        assert_eq!(word_left("see foo.", 8), 7);
        assert_eq!(word_left("see foo. ", 9), 7);
        assert_eq!(word_left("a/b/", 4), 3);
        assert_eq!(word_left("...", 3), 0);
    }

    #[test]
    fn double_click_word() {
        let s = "These shortcuts don't work.";
        let w = |off| &s[word_at(s, off)];
        assert_eq!(w(8), "shortcuts");
        assert_eq!(w(6), "shortcuts");
        assert_eq!(w(0), "These");
        assert_eq!(w(5), " ");
        assert_eq!(w(19), "'");
        assert_eq!(w(s.len()), ".");
        assert_eq!(w(s.len() - 1), ".");
        assert_eq!(word_at("a\nbc", 1), 0..1);
        assert_eq!(word_at("", 0), 0..0);
        assert_eq!(&"héllo wörld"[word_at("héllo wörld", 8)], "wörld");
    }
}
