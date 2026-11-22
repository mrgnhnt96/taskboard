//! Backlog › Deferred and won't do: the issues closed one of those ways (`GET /backlog/closed`, a page
//! at a time, last closed first), under a tab for each, with Reopen on every row
//! (`POST /backlog/<ref>/reopen`). A row opens the issue's panel. It's the Backlog page's sub-page,
//! opened from its header and left with ‹ Backlog.
use crate::app::MainWindow;
use crate::fmt::{self, s};
use crate::theme::Theme;
use crate::ui::issue_panel as ip;
use crate::ui::kit;
use crate::ui::paged::Paged;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::time::{Duration, Instant};

const SYNC_EVERY: Duration = Duration::from_secs(5);
/// Issues per page.
const PAGE: usize = 100;
/// Ask for the next page this many rows before the list's end.
const AHEAD: usize = 40;

#[derive(Default)]
pub struct State {
    /// The sub-page is showing instead of the Backlog's list.
    pub open: bool,
    /// The Won't do tab is showing.
    wont_do: bool,
    held: Paged,
    /// (deferred, won't do), every project.
    counts: Option<(usize, usize)>,
    err: Option<String>,
    synced: Option<Instant>,
    generation: u64,
    /// Issues whose Reopen was sent and not answered yet.
    busy: HashSet<String>,
    list: Option<ListState>,
    /// `held.version` and the tab the list's rows were laid out for.
    drawn: Option<(u64, bool)>,
}

impl State {
    fn state(&self) -> &'static str {
        if self.wont_do { "wont_do" } else { "defer" }
    }

    fn show(&mut self, wont_do: bool) {
        if self.wont_do != wont_do {
            self.wont_do = wont_do;
            self.held = Paged::default();
            self.drawn = None;
        }
        self.synced = None;
    }
}

/// The header's way in.
pub fn link(t: &Theme, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    kit::link(t, "plan-closed", "Deferred and won’t do").text_size(px(13.)).on_click(cx.listener(|m, _, _, cx| {
        let c = &mut m.backlog.closed;
        c.open = true;
        c.show(false);
        cx.notify();
    }))
}

/// Ask the board again on the next draw: something was closed or reopened.
pub fn stale(m: &mut MainWindow) {
    m.backlog.closed.synced = None;
}

fn sync(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let c = &mut m.backlog.closed;
    if c.synced.is_some_and(|t| t.elapsed() < SYNC_EVERY) {
        return;
    }
    c.synced = Some(Instant::now());
    c.generation += 1;
    let (generation, wont_do) = (c.generation, c.wont_do);
    let query = vec![("state", c.state().to_string())];
    // Any issue held can be reopened elsewhere, so a refresh asks for them all again.
    let recheck = c.held.recheck(|_| true);
    let backend = m.backend.clone();
    cx.spawn(async move |this, cx| {
        let got = cx.background_executor().spawn(async move { Paged::fetch(&*backend, "backlog/closed", "issues", &query, PAGE, recheck) }).await;
        let _ = this.update(cx, |m, cx| {
            let c = &mut m.backlog.closed;
            if c.generation != generation || c.wont_do != wont_do {
                return;
            }
            match got {
                Ok(f) => {
                    let n = |k: &str| f.first()["counts"][k].as_u64().unwrap_or(0) as usize;
                    c.counts = Some((n("defer"), n("wont_do")));
                    c.held.land("issues", f);
                    c.err = None;
                }
                Err(e) => c.err = Some(e),
            }
            cx.notify();
        });
    })
    .detach();
}

/// Fetch the next older page once the list nears its end.
fn load_older(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let c = &mut m.backlog.closed;
    let (Some(before), Some(l)) = (c.held.next.clone(), &c.list) else { return };
    if c.held.loading || c.held.rows.len() > l.logical_scroll_top().item_ix + AHEAD {
        return;
    }
    c.held.loading = true;
    let backend = m.backend.clone();
    let query = vec![("state", c.state().to_string()), ("limit", PAGE.to_string()), ("before", before.clone())];
    cx.spawn(async move |this, cx| {
        let got = cx.background_executor().spawn(async move { backend.get("backlog/closed", &query).map_err(|e| e.message) }).await;
        let _ = this.update(cx, |m, cx| {
            m.backlog.closed.held.land_older("issues", &before, got);
            cx.notify();
        });
    })
    .detach();
}

fn reopen(m: &mut MainWindow, r: String, cx: &mut Context<MainWindow>) {
    if !m.backlog.closed.busy.insert(r.clone()) {
        return;
    }
    let (r1, r2) = (r.clone(), r.clone());
    m.post_or(
        format!("backlog/{r}/reopen"),
        json!({}),
        cx,
        move |m, _, cx| {
            m.backlog.closed.busy.remove(&r1);
            crate::ui::backlog::stale(m);
            m.toast(format!("Opened {r1} again"), false, cx);
        },
        move |m, e, cx| {
            m.backlog.closed.busy.remove(&r2);
            m.toast(e.message, true, cx);
        },
    );
    cx.notify();
}

/// "Deferred Nov 9", "Won’t do today": how and when it closed.
fn closed_line(b: &Value) -> String {
    let how = if s(b, "state") == "defer" { "Deferred" } else { "Won’t do" };
    let Some(at) = fmt::parse(s(b, "closed_at")) else { return how.into() };
    let (day, today) = (fmt::local(at).date_naive(), fmt::local(fmt::now()).date_naive());
    let when = if day == today {
        "today".to_string()
    } else if chrono::Datelike::year(&day) == chrono::Datelike::year(&today) {
        day.format("%b %-d").to_string()
    } else {
        day.format("%b %-d, %Y").to_string()
    };
    format!("{how} {when}")
}

/// "project · area", either alone when the other is missing.
fn place(b: &Value) -> String {
    [s(b, "project"), s(b, "area")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")
}

fn row(m: &MainWindow, t: &Theme, b: &Value, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let r = fmt::ref_of(b, "B");
    let busy = m.backlog.closed.busy.contains(&r);
    let hover = t.hover_overlay;
    let btn = kit::btn_small(t, SharedString::from(format!("closed-reopen-{r}")), if busy { "Sending…" } else { "Reopen" })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    let btn = if busy {
        kit::disabled(btn)
    } else {
        let r = r.clone();
        btn.on_click(cx.listener(move |m, _, _, cx| {
            cx.stop_propagation();
            reopen(m, r.clone(), cx);
        }))
    };
    let open = r.clone();
    crate::ui::focus::stop(t, div().id(SharedString::from(format!("closed-row-{r}"))))
        .w_full()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(10.))
        .py(px(9.))
        .border_b_1()
        .border_color(t.divider)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .tooltip(kit::tip("Open the issue: what was said and its history"))
        .child(div().flex_none().w(px(68.)).text_size(px(12.)).text_color(t.muted).child(ip::kind_label(s(b, "kind"))))
        .child(kit::mono(t, r.clone()).flex_none().w(px(52.)).text_color(t.faint))
        .child(div().min_w_0().flex_1().truncate().text_size(px(13.5)).child(s(b, "title").to_string()))
        .child(div().flex_none().w(px(180.)).truncate().text_size(px(12.)).text_color(t.muted).child(place(b)))
        .child(div().flex_none().w(px(110.)).whitespace_nowrap().text_size(px(12.)).text_color(t.muted).child(closed_line(b)))
        .child(btn)
        .on_click(cx.listener(move |m, _, _, cx| m.open_issue(open.clone(), cx)))
}

fn line_item(m: &mut MainWindow, ix: usize, _: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let rows = m.backlog.closed.held.rows.clone();
    match rows.get(ix) {
        Some(b) => row(m, &t, b, cx).into_any_element(),
        None => div().h(px(32.)).into_any_element(),
    }
}

pub fn render(m: &mut MainWindow, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    sync(m, cx);
    let c = &mut m.backlog.closed;
    // One line per issue held, then a gap at the end.
    let count = c.held.rows.len() + 1;
    let key = (c.held.version, c.wont_do);
    match &c.list {
        None => c.list = Some(ListState::new(count, ListAlignment::Top, px(600.))),
        Some(l) if c.drawn != Some(key) => {
            let same_tab = c.drawn.is_some_and(|(_, w)| w == c.wont_do);
            let top = l.logical_scroll_top();
            l.reset(count);
            if same_tab {
                l.scroll_to(ListOffset { item_ix: top.item_ix.min(count - 1), offset_in_item: top.offset_in_item });
            }
        }
        Some(_) => {}
    }
    c.drawn = Some(key);
    load_older(m, cx);
    let c = &m.backlog.closed;

    let back = kit::btn(&t, "closed-back", "‹  Backlog").on_click(cx.listener(|m, _, _, cx| {
        m.backlog.closed.open = false;
        crate::ui::backlog::stale(m);
        cx.notify();
    }));
    let header = div().flex().items_center().gap(px(16.)).child(back).child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Deferred and won’t do"));
    let (defer, wont) = c.counts.unwrap_or_default();
    let labels = [format!("Deferred {defer}"), format!("Won’t do {wont}")];
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let tabs = div().flex().child(kit::seg(&t, "closed-tab", &labels, c.wont_do as usize, |i, item| {
        item.on_click(cx.listener(move |m, _, _, cx| {
            m.backlog.closed.show(i == 1);
            cx.notify();
        }))
    }));
    let body: AnyElement = if !c.held.loaded {
        match &c.err {
            Some(e) => div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.))
                .py(px(40.))
                .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child("These didn’t load"))
                .child(div().text_size(px(12.5)).text_color(t.muted).child(e.clone()))
                .child(kit::btn(&t, "closed-retry", "Try again").on_click(cx.listener(|m, _, _, cx| {
                    stale(m);
                    cx.notify();
                })))
                .into_any_element(),
            None => kit::empty(&t, "Loading…").into_any_element(),
        }
    } else if c.held.rows.is_empty() {
        kit::empty(&t, if c.wont_do { "Nothing closed as won’t do" } else { "Nothing deferred" }).into_any_element()
    } else {
        let state = c.list.clone().unwrap();
        gpui_kit::list(state, cx.processor(line_item)).flex_1().min_h_0().into_any_element()
    };
    div()
        .id("plan-closed-page")
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .size_full()
        .child(div().flex().flex_col().gap(px(16.)).flex_1().min_h_0().w_full().max_w(px(1320.)).px(px(28.)).pt(px(22.)).child(header).child(tabs).child(body))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // Beats the glob's `gpui_kit::test`.
    use ::core::prelude::v1::test;

    #[test]
    fn a_row_says_how_and_when_it_closed_and_where_it_was() {
        let now = fmt::now();
        let year_ago = now - chrono::Duration::days(400);
        assert_eq!(closed_line(&json!({"state": "defer", "closed_at": now.to_rfc3339()})), "Deferred today");
        let old = closed_line(&json!({"state": "drop", "closed_at": year_ago.to_rfc3339()}));
        assert!(old.starts_with("Won’t do ") && old.contains(", "), "another year names it: {old}");
        assert_eq!(closed_line(&json!({"state": "drop"})), "Won’t do");
        assert_eq!(place(&json!({"project": "webapp", "area": "Export"})), "webapp · Export");
        assert_eq!(place(&json!({"project": "webapp", "area": null})), "webapp");
    }

    #[gpui_kit::test]
    fn the_tabs_list_deferred_and_wont_do_issues_and_reopen_takes_one_off(cx: &mut gpui_kit::TestAppContext) {
        use crate::backend::Backend;
        use crate::parity;
        let (w, rec) = parity::window(cx);
        let mk = |title: &str| fmt::ref_of(&rec.board().post("backlog", json!({"title": title, "kind": "bug", "project": "webapp"})).unwrap(), "B");
        let (a, b) = (mk("Tooltip flickers over the chart legend"), mk("No dark mode for printed reports"));
        rec.board().post("backlog/bulk", json!({"action": "defer", "ids": [a.clone()]})).unwrap();
        rec.board().post(&format!("backlog/{b}/drop"), json!({})).unwrap();
        let refs = |m: &MainWindow| m.backlog.closed.held.rows.iter().map(|x| fmt::ref_of(x, "B")).collect::<Vec<_>>();
        w.update(cx, |m, _, cx| {
            m.backlog.closed.open = true;
            sync(m, cx);
        })
        .unwrap();
        parity::settle(cx);
        w.update(cx, |m, _, cx| {
            assert_eq!((refs(m), m.backlog.closed.counts), (vec![a.clone()], Some((1, 1))));
            m.backlog.closed.show(true);
            sync(m, cx);
        })
        .unwrap();
        parity::settle(cx);
        w.update(cx, |m, _, cx| {
            assert_eq!(refs(m), [b.clone()]);
            reopen(m, b.clone(), cx);
        })
        .unwrap();
        parity::settle(cx);
        assert_eq!(rec.last(&format!("backlog/{b}/reopen")), Some(json!({})));
        w.update(cx, |m, _, cx| sync(m, cx)).unwrap();
        parity::settle(cx);
        w.update(cx, |m, _, _| {
            assert!(refs(m).is_empty(), "open again, it leaves the list");
            assert_eq!(m.backlog.closed.counts, Some((1, 0)));
            assert_eq!(m.toasts.last().map(|t| t.text.clone()), Some(format!("Opened {b} again")));
        })
        .unwrap();
    }
}
