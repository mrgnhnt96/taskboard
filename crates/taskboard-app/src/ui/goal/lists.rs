//! The Goal page body as two lists that draw only what's in view: the wave cards, one item per
//! card head and task row, and the aside, one item per section piece, note and open issue.
//!
//! The goal carries its newest page of notes and their total; older pages are fetched by cursor
//! as the aside nears its end, the same way the task panel's Full log pages.

use super::*;
use std::rc::Rc;

/// How many older notes one fetch asks for (the goal itself carries the newest `NOTE_PAGE`).
const NOTE_PAGE: usize = 100;
/// Fetch the next page once the aside is scrolled within this many items of the notes' end.
const NOTE_AHEAD: usize = 20;

#[derive(Default)]
pub struct Lists {
    waves: Option<Body>,
    aside: Option<Body>,
    cards: Rc<Vec<waves::Card>>,
    wave_items: Rc<Vec<WItem>>,
    aside_items: Rc<Vec<AItem>>,
    issues: Rc<Vec<Issue>>,
    notes: Notes,
    cache: NoteCache,
}

struct Body {
    state: ListState,
    keys: Vec<String>,
}

impl Body {
    /// Keep the list's items in step with `keys`: what changed between the common start and end
    /// is spliced, so the place scrolled to stays put.
    fn sync(slot: &mut Option<Body>, keys: Vec<String>) {
        let Some(b) = slot else {
            *slot = Some(Body { state: ListState::new(keys.len(), ListAlignment::Top, px(800.)), keys });
            return;
        };
        if b.keys == keys {
            return;
        }
        let head = b.keys.iter().zip(&keys).take_while(|(a, c)| a == c).count();
        let room = b.keys.len().min(keys.len()) - head;
        let tail = b.keys.iter().rev().zip(keys.iter().rev()).take(room).take_while(|(a, c)| a == c).count();
        b.state.splice(head..b.keys.len() - tail, keys.len() - head - tail);
        b.keys = keys;
    }
}

/// The notes as the aside shows them: those fetched so far under the kind filter.
#[derive(Default)]
pub struct Notes {
    /// Every note the goal has, fetched or not.
    pub total: usize,
    /// The kinds among the notes fetched so far.
    pub groups: Vec<&'static str>,
    pub filter: Option<&'static str>,
    pub shown: Rc<Vec<NoteItem>>,
    /// Older notes are still to come.
    pub more: bool,
}

#[derive(Default)]
struct NoteCache {
    goal: String,
    /// The goal's page last merged in, so an unchanged one isn't merged again.
    seen: Value,
    rows: Vec<Value>,
    /// The board has nothing older.
    done: bool,
    loading: bool,
}

impl NoteCache {
    /// Take in the goal's newest page: the whole list when it holds every note, else merged into
    /// the older pages fetched before it.
    fn take(&mut self, page: &[Value], total: usize) {
        if total <= page.len() {
            self.rows = page.to_vec();
            self.rows.sort_by(note_order);
            self.done = true;
        } else {
            self.add(page.iter().cloned());
        }
    }

    /// Add notes, a newer copy replacing the one held, keeping the board's order.
    fn add(&mut self, new: impl IntoIterator<Item = Value>) {
        for n in new {
            match self.rows.iter_mut().find(|o| o["id"] == n["id"]) {
                Some(o) => *o = n,
                None => self.rows.push(n),
            }
        }
        self.rows.sort_by(note_order);
    }
}

#[derive(Clone, Copy)]
enum WItem {
    /// Piece `k` of card `c`.
    Piece(usize, usize),
    /// The space under the last card.
    End,
}

#[derive(Clone, Copy)]
enum AItem {
    How,
    NotesHead,
    Note(usize),
    NotesMore,
    Att(usize),
    Devices,
    Bits,
    BacklogHead,
    Issue(usize),
    BacklogMore(usize),
    /// The space under the aside.
    End,
}

/// Bring the lists, the notes and their items up to date with goal `g`, and fetch older notes
/// when the aside nears their end.
pub fn sync(m: &mut MainWindow, g: &Value, cx: &mut Context<MainWindow>) {
    let gr = fmt::ref_of(g, "G");
    let total = g["notes_total"].as_u64().map_or(arr(g, "notes").len(), |n| n as usize);
    let l = &mut m.goal_page.lists;
    if l.cache.goal != gr {
        l.cache = NoteCache { goal: gr.clone(), ..Default::default() };
    }
    let seen = json!([g["notes"], total]);
    if l.cache.seen != seen {
        l.cache.take(arr(g, "notes"), total);
        l.cache.seen = seen;
    }
    // The rows are already in the board's order, so each lines up with its item.
    let items = note_items(&l.cache.rows);
    let groups = note_groups(&items);
    let filter = m.goal_page.note_filter.filter(|k| groups.contains(k));
    let (ids, shown): (Vec<String>, Vec<NoteItem>) = l.cache.rows.iter().zip(items).filter(|(_, x)| filter.is_none_or(|k| x.group == k)).map(|(n, x)| (format!("{}:{}", n["id"], s(n, "text").len()), x)).unzip();
    l.notes = Notes { total, groups, filter, shown: Rc::new(shown), more: total > l.cache.rows.len() && !l.cache.done };

    // The wave cards.
    let cards = waves::cards(g);
    let mut wi = vec![];
    let mut wk = vec![];
    for (c, card) in cards.iter().enumerate() {
        let head = (card.head.is_some() || card.gate.is_some()) as usize;
        for k in 0..waves::pieces(card) {
            wi.push(WItem::Piece(c, k));
            wk.push(match k.checked_sub(head) {
                Some(r) => format!("w:{}:{}", card.key, card.rows[r].r),
                None => format!("w:{}:head", card.key),
            });
        }
    }
    wi.push(WItem::End);
    wk.push("end".into());

    // The aside's sections, top to bottom.
    let mut ai = vec![AItem::How];
    let mut ak = vec!["how".to_string()];
    if total > 0 {
        ai.push(AItem::NotesHead);
        ak.push(format!("notes:{}", m.goal_page.notes_open));
        if m.goal_page.notes_open {
            for (k, id) in ids.into_iter().enumerate() {
                ai.push(AItem::Note(k));
                ak.push(format!("note:{id}"));
            }
            if l.notes.more {
                ai.push(AItem::NotesMore);
                ak.push("notes-more".into());
            }
        }
    }
    let att = attached::attached_parts(m, g);
    for k in 0..att {
        ai.push(AItem::Att(k));
        ak.push(format!("att:{k}:{att}"));
    }
    if g["devices"].is_object() {
        ai.push(AItem::Devices);
        ak.push("devices".into());
    }
    if !pool::bit_rows(g).is_empty() {
        ai.push(AItem::Bits);
        ak.push("bits".into());
    }
    let issues = open_issues(g);
    if !issues.is_empty() {
        ai.push(AItem::BacklogHead);
        ak.push(format!("backlog:{}", issues.len()));
        let shown = if m.goal_page.backlog_view { issues.len() } else { issues.len().min(BACKLOG_SHOWN) };
        for (k, Issue { r, .. }) in issues.iter().take(shown).enumerate() {
            ai.push(AItem::Issue(k));
            ak.push(format!("issue:{r}"));
        }
        if shown < issues.len() {
            ai.push(AItem::BacklogMore(issues.len() - shown));
            ak.push("backlog-more".into());
        }
    }
    ai.push(AItem::End);
    ak.push("end".into());

    let l = &mut m.goal_page.lists;
    l.cards = Rc::new(cards);
    l.wave_items = Rc::new(wi);
    l.aside_items = Rc::new(ai);
    l.issues = Rc::new(issues);
    Body::sync(&mut l.waves, wk);
    Body::sync(&mut l.aside, ak);
    load_older_notes(m, cx);
}

/// Fetch the next older page of notes when the aside is scrolled near their end.
fn load_older_notes(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let l = &mut m.goal_page.lists;
    let Some(aside) = &l.aside else { return };
    if !m.goal_page.notes_open || !l.notes.more || l.cache.loading {
        return;
    }
    let end = l.aside_items.iter().position(|x| matches!(x, AItem::NotesMore)).unwrap_or(0);
    if end > aside.state.logical_scroll_top().item_ix + NOTE_AHEAD {
        return;
    }
    l.cache.loading = true;
    let gr = l.cache.goal.clone();
    let mut query = vec![("limit", NOTE_PAGE.to_string())];
    if let Some(id) = l.cache.rows.last().and_then(|n| n["id"].as_i64()) {
        query.push(("before", id.to_string()));
    }
    let (backend, path) = (m.backend.clone(), format!("goals/{gr}/notes"));
    cx.spawn(async move |this, cx| {
        let got = cx.background_executor().spawn(async move { backend.get(&path, &query) }).await;
        let _ = this.update(cx, |m, cx| {
            let c = &mut m.goal_page.lists.cache;
            // Another goal since: its cache started over and isn't waiting on this.
            if c.goal != gr {
                return;
            }
            c.loading = false;
            match got {
                Ok(v) => {
                    c.done = !v["more"].as_bool().unwrap_or(false);
                    c.add(arr(&v, "notes").iter().cloned());
                }
                // Stop asking; the notes already here stay.
                Err(_) => c.done = true,
            }
            cx.notify();
        });
    })
    .detach();
}

/// The two lists side by side under the header.
pub fn body(m: &MainWindow, cx: &mut Context<MainWindow>) -> Div {
    let l = &m.goal_page.lists;
    let mut out = div().flex_1().min_h_0().flex().gap(px(16.)).pt(px(16.)).px(px(36.));
    if let (Some(w), Some(a)) = (&l.waves, &l.aside) {
        out = out
            .child(list(w.state.clone(), cx.processor(wave_item)).flex_1().min_w_0().h_full())
            .child(list(a.state.clone(), cx.processor(aside_item)).flex_none().w(px(330.)).h_full());
    }
    out
}

fn wave_item(m: &mut MainWindow, ix: usize, _: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let l = &m.goal_page.lists;
    let (items, cards) = (l.wave_items.clone(), l.cards.clone());
    let gr = l.cache.goal.clone();
    match items.get(ix) {
        Some(WItem::Piece(c, k)) => waves::piece(m, &t, &gr, &cards, *c, *k, cx),
        _ => div().h(px(48.)).into_any_element(),
    }
}

fn aside_item(m: &mut MainWindow, ix: usize, _: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let Some(g) = m.data.goal.clone() else { return div().into_any_element() };
    let l = &m.goal_page.lists;
    let items = l.aside_items.clone();
    let Some(&item) = items.get(ix) else { return div().into_any_element() };
    let body = match item {
        AItem::How => how_runs_el(m, &t, &g, cx),
        AItem::NotesHead => notes_head(m, &t, &l.notes, cx),
        AItem::Note(k) => {
            let shown = l.notes.shown.clone();
            let last = k + 1 == shown.len() && !l.notes.more;
            note_piece(&t, &shown[k], k, last)
        }
        AItem::NotesMore => notes_more(&t, !l.notes.shown.is_empty()),
        AItem::Att(k) => attached::attached_piece(m, &t, &g, k, cx),
        AItem::Devices => pool::devices_aside(m, &t, &g, cx).unwrap_or_else(div),
        AItem::Bits => pool::bits_aside(&t, &g).unwrap_or_else(div),
        AItem::BacklogHead => backlog_head(&t, l.issues.len()),
        AItem::Issue(k) => {
            let issues = l.issues.clone();
            let last = k + 1 == issues.len() || (k + 1 == BACKLOG_SHOWN && !m.goal_page.backlog_view);
            issue_piece(&t, issues[k].clone(), k, last, cx)
        }
        AItem::BacklogMore(n) => backlog_more(&t, n, cx),
        AItem::End => return div().h(px(48.)).into_any_element(),
    };
    // The aside's card, drawn a slice per item: its sides on each, its top and bottom on the ends.
    let last = ix + 2 == items.len();
    div()
        .flex()
        .flex_col()
        .bg(t.card)
        .border_l_1()
        .border_r_1()
        .border_color(t.border)
        .overflow_hidden()
        .when(ix == 0, |d| d.border_t_1().rounded_t(px(10.)))
        .when(last, |d| d.border_b_1().rounded_b(px(10.)))
        .child(body)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backend;
    use ::core::prelude::v1::test;

    #[gpui_kit::test]
    fn a_goal_with_hundreds_of_notes_pages_them_in_as_the_aside_scrolls(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = crate::parity::window(cx);
        for k in 0..300 {
            let kind = if k % 3 == 0 { "decision" } else { "finding" };
            rec.board().post("goals/G1/notes", json!({"kind": kind, "text": format!("Paged note {k}")})).unwrap();
        }
        let draw = |cx: &mut gpui_kit::TestAppContext| {
            let _ = cx.update_window(w.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
            crate::parity::settle(cx);
        };
        w.update(cx, |m, _, cx| m.go(Page::Goal("G1".into()), cx)).unwrap();
        crate::parity::settle(cx);
        draw(cx);
        let held = |cx: &mut gpui_kit::TestAppContext| w.update(cx, |m, _, _| (m.goal_page.lists.cache.rows.len(), m.goal_page.lists.notes.more, m.goal_page.lists.notes.total)).unwrap();
        let (n, more, total) = held(cx);
        assert!(total >= 300, "the head counts every note: {total}");
        assert_eq!((n, more), (NOTE_PAGE, true), "only the goal's newest page until the notes are shown");

        w.update(cx, |m, _, cx| {
            m.goal_page.notes_open = true;
            cx.notify();
        })
        .unwrap();
        for _ in 0..6 {
            draw(cx);
            w.update(cx, |m, _, _| {
                let l = &m.goal_page.lists;
                l.aside.as_ref().unwrap().state.scroll_to(ListOffset { item_ix: l.aside_items.len(), offset_in_item: px(0.) });
            })
            .unwrap();
        }
        draw(cx);
        assert_eq!(held(cx), (total, false, total), "scrolling to the end pages back through the rest");
        let ids: HashSet<i64> = w.update(cx, |m, _, _| m.goal_page.lists.cache.rows.iter().map(|n| n["id"].as_i64().unwrap()).collect()).unwrap();
        assert_eq!(ids.len(), total, "each note once");
        let order = w.update(cx, |m, _, _| m.goal_page.lists.cache.rows.windows(2).all(|p| note_order(&p[0], &p[1]).is_le())).unwrap();
        assert!(order, "pinned first, then newest, across pages");
    }
}
