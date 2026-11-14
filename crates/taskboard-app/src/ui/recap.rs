//! A weekly or monthly recap, under Days: the page the daemon wrote for it, drawn component by
//! component. A component the app doesn't know is left out.
mod headline;
pub mod parts;
mod shipped;
mod time;
mod trends;

use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, s};
use crate::theme::Theme;
use crate::ui::kit;
use crate::ui::select::sel;
use gpui_kit::prelude::*;
use gpui_kit::*;
use parts::Ctx;
use serde_json::{Value, json};
use std::collections::HashSet;

const MENU: &str = "recap-list";
/// Recaps the All recaps menu shows before it scrolls.
const SHOWN: usize = 12;
/// Below this window width, half-width components each take a row.
const PAIR_AT: f32 = 1100.;

#[derive(Default)]
pub struct State {
    /// The recap shown; `None` for the newest.
    pub id: Option<i64>,
    /// Recaps already marked seen from here, so each is sent once.
    marked: HashSet<i64>,
    /// The page's open disclosures.
    open: HashSet<String>,
}

type Draw = fn(&Ctx, &Value, &mut Context<MainWindow>) -> Option<AnyElement>;

/// The view for a component id; `None` for one this app doesn't draw.
fn view(id: &str) -> Option<Draw> {
    Some(match id {
        "open" => headline::open,
        "hero" => headline::hero,
        "heroR" => headline::hero_r,
        "kpis" => headline::kpis,
        "fact" => headline::fact,
        "quiet" => headline::quiet,
        "first" => headline::first,
        "gcards" => shipped::gcards,
        "story" => shipped::story,
        "spot" => shipped::spot,
        "timeline" => shipped::timeline,
        "train" => shipped::train,
        "list" => shipped::list,
        "time" => time::time,
        "vs" => time::vs,
        "wait" => time::wait,
        "usage" => time::usage,
        "days" => trends::days,
        "ba" => trends::ba,
        "trend" => trends::trend,
        "lines" => trends::lines,
        "board" => trends::board,
        "plan" => trends::plan,
        "lessons" => trends::lessons,
        "issues" => trends::issues,
        _ => return None,
    })
}

/// The page's components this app draws, in order, with whether each takes the full width.
pub fn components(page: &Value) -> Vec<(Value, bool)> {
    arr(page, "components").iter().filter(|c| view(s(c, "id")).is_some()).map(|c| (c.clone(), s(c, "span") != "half")).collect()
}

/// Rows of the page: a full-width component alone, two halves side by side when `pair`.
pub fn rows(full: &[bool], pair: bool) -> Vec<Vec<usize>> {
    let mut out = vec![];
    let mut i = 0;
    while i < full.len() {
        if pair && !full[i] && full.get(i + 1) == Some(&false) {
            out.push(vec![i, i + 1]);
            i += 2;
        } else {
            out.push(vec![i]);
            i += 1;
        }
    }
    out
}

/// "Nov 2 – Nov 8" for a week, "October 2026" for a month.
pub fn period(r: &Value) -> String {
    let day = |k: &str, edge: &str| parts::date(s(r, k)).or_else(|| fmt::parse(s(r, edge)).map(|t| t.date_naive()));
    let Some(first) = day("first_day", "period_start") else { return String::new() };
    if s(r, "kind") == "monthly" {
        return first.format("%B %Y").to_string();
    }
    let last = day("last_day", "period_end").map(|d| d.format("%b %-d").to_string()).unwrap_or_default();
    format!("{} – {last}", first.format("%b %-d"))
}

pub fn current(m: &MainWindow) -> Option<i64> {
    m.recap.id.or_else(|| m.data.recaps.as_ref().and_then(|l| arr(l, "recaps").first()).and_then(|r| r["id"].as_i64()))
}

// ------------------------------------------------------------------ actions

/// Open recap `id` (`None`: the newest), from a link or the Days page.
pub fn open(m: &mut MainWindow, id: Option<i64>, cx: &mut Context<MainWindow>) {
    if m.recap.id != id || id.is_none() {
        m.data.recap = None;
        m.data.errs.recap = None;
        m.recap.open.clear();
    }
    m.recap.id = id;
    m.go(Page::Recap, cx);
    m.refresh(cx);
}

fn pick(m: &mut MainWindow, id: i64, cx: &mut Context<MainWindow>) {
    m.menu = None;
    if current(m) != Some(id) {
        m.data.recap = None;
        m.data.errs.recap = None;
        m.recap.open.clear();
    }
    m.recap.id = Some(id);
    m.refresh(cx);
}

/// Opens or closes a disclosure. A key with a `|` closes the others of its group (before the `|`).
pub fn toggle(m: &mut MainWindow, key: &str, cx: &mut Context<MainWindow>) {
    flip(&mut m.recap.open, key);
    cx.notify();
}

fn flip(open: &mut HashSet<String>, key: &str) {
    if open.remove(key) {
        return;
    }
    if let Some((group, _)) = key.split_once('|') {
        let pre = format!("{group}|");
        open.retain(|k| !k.starts_with(&pre));
    }
    open.insert(key.to_string());
}

/// Opening an unseen recap clears its Needs you item, withdraws its notification, and takes it off
/// the Days tab's count.
fn mark_seen(m: &mut MainWindow, r: &Value, cx: &mut Context<MainWindow>) {
    let Some(id) = r["id"].as_i64() else { return };
    if !r["seen_at"].is_null() || !m.recap.marked.insert(id) {
        return;
    }
    m.post(format!("recaps/{id}/seen"), json!({}), cx, |m, v, cx| {
        if m.data.recap.as_ref().and_then(|r| r["id"].as_i64()) == v["id"].as_i64() {
            m.data.recap = Some(v);
        }
        m.refresh(cx);
    });
}

// ------------------------------------------------------------------ the page

fn page(header: Div) -> Div {
    div().flex().flex_col().gap(px(16.)).w_full().max_w(px(1180.)).px(px(28.)).py(px(22.)).child(header)
}

fn header(t: &Theme, title: String, sub: Option<String>, nav: Div) -> Div {
    let title = div()
        .flex()
        .items_baseline()
        .gap(px(10.))
        .child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).child(sel(t, "recap-period", title)))
        .children(sub.map(|a| div().text_size(px(13.)).text_color(t.muted).child(a)));
    div().flex().flex_wrap().items_center().gap(px(14.)).child(title).child(div().flex_1()).child(nav)
}

/// The summary and highlights, for a page without an opening line.
fn summary(c: &Ctx) -> Option<AnyElement> {
    if c.summary.is_empty() {
        return None;
    }
    let body = vec![headline::lede(c, &c.summary), headline::highlights(c)].into_iter().flatten().collect();
    Some(parts::card(c, "summary", None, None, body))
}

fn body(c: &Ctx, page_v: &Value, pair: bool, cx: &mut Context<MainWindow>) -> Div {
    let comps = components(page_v);
    let has_open = comps.iter().any(|(x, _)| s(x, "id") == "open");
    let mut drawn: Vec<(AnyElement, bool)> = vec![];
    for (comp, full) in &comps {
        if let Some(el) = view(s(comp, "id")).and_then(|f| f(c, comp, cx)) {
            drawn.push((el, *full));
        }
    }
    let full: Vec<bool> = drawn.iter().map(|d| d.1).collect();
    let mut els: Vec<Option<AnyElement>> = drawn.into_iter().map(|d| Some(d.0)).collect();
    let mut out = div().flex().flex_col().gap(px(16.)).children((!has_open).then(|| summary(c)).flatten());
    for row in rows(&full, pair) {
        let lone_half = row.len() == 1 && !full[row[0]] && pair;
        let line = div()
            .flex()
            .gap(px(16.))
            .children(row.iter().filter_map(|&i| els[i].take()).map(|el| div().flex_1().min_w_0().child(el)))
            .when(lone_half, |d| d.child(div().flex_1()));
        out = out.child(line);
    }
    out
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let shell = div().id("recap-page").flex().flex_col().flex_1().min_w_0().size_full().overflow_y_scroll();
    let list: Vec<Value> = m.data.recaps.as_ref().map(|l| arr(l, "recaps").to_vec()).unwrap_or_default();
    let want = current(m);
    let shown = m.data.recap.clone().filter(|r| r["id"].as_i64().is_some() && r["id"].as_i64() == want);
    let Some(r) = shown else {
        let msg = match (&m.data.errs.recap, &m.data.recaps) {
            (Some(e), _) => e.clone(),
            (None, Some(_)) if list.is_empty() => "No recaps yet. The first comes on Monday.".into(),
            _ => "Loading…".into(),
        };
        let nav = div().flex().child(crate::ui::review::day_week(&t, 2, cx));
        return shell.child(page(header(&t, String::new(), None, nav)).child(kit::empty(&t, msg))).into_any_element();
    };
    mark_seen(m, &r, cx);
    let id = r["id"].as_i64().unwrap_or_default();
    let month = s(&r, "kind") == "monthly";

    // ---- header: the period, with earlier and later recaps around a picker for any of them
    let at = list.iter().position(|x| x["id"].as_i64() == Some(id));
    let older = at.and_then(|i| list.get(i + 1)).and_then(|x| x["id"].as_i64());
    let newer = at.and_then(|i| i.checked_sub(1)).and_then(|i| list.get(i)).and_then(|x| x["id"].as_i64());
    let nav = div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(kit::btn(&t, "recap-older", "‹").w(px(36.)).when(older.is_none(), kit::disabled).tooltip(kit::tip("Earlier recap")).on_click(cx.listener(move |m, _, _, cx| {
            if let Some(id) = older {
                pick(m, id, cx)
            }
        })))
        .child(kit::btn(&t, "recap-list-btn", "All recaps  ▾").on_click(cx.listener(|m, e: &ClickEvent, _, cx| {
            let p = e.position();
            m.toggle_menu(MENU, point(p.x - px(160.), p.y + px(14.)), cx);
        })))
        .child(kit::btn(&t, "recap-newer", "›").w(px(36.)).when(newer.is_none(), kit::disabled).tooltip(kit::tip("Later recap")).on_click(cx.listener(move |m, _, _, cx| {
            if let Some(id) = newer {
                pick(m, id, cx)
            }
        })))
        .child(div().w(px(6.)))
        .child(crate::ui::review::day_week(&t, 2, cx));

    let mut menus: Vec<AnyElement> = vec![];
    if let Some(at) = m.menu_open(MENU) {
        let items: Vec<(i64, String)> = list
            .iter()
            .filter_map(|x| {
                let kind = if s(x, "kind") == "monthly" { "Month" } else { "Week" };
                let dot = if x["seen_at"].is_null() { "● " } else { "" };
                Some((x["id"].as_i64()?, format!("{dot}{} · {kind}", period(x))))
            })
            .collect();
        let (n, tt) = (items.len(), t.clone());
        // Only the recaps in view are drawn; past a dozen the menu scrolls.
        let el = uniform_list(
            "recap-list-items",
            n,
            cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|k| {
                        let (rid, label) = (items[k].0, items[k].1.clone());
                        kit::menu_item(&tt, SharedString::from(format!("recap-item-{rid}")), label, rid == id).on_click(cx.listener(move |m, _, _, cx| pick(m, rid, cx)))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .h(px(n.min(SHOWN) as f32 * 30.));
        let menu = kit::menu_box(&t, 240.).id("recap-menu").child(el);
        menus.push(kit::popover(at, menu.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())));
    }

    let title = period(&r);
    let sub = Some(if month { "Monthly recap" } else { "Weekly recap" }.to_string());
    let head = page(header(&t, title, sub, nav));
    let writing = &r["writing"];
    let content = match writing["page"].is_object() {
        true => {
            let mut c = Ctx::new(t.clone(), id, month, r["facts"].clone(), m.recap.open.clone());
            c.summary = s(&writing["page"], "summary").to_string();
            c.highlights = arr(&writing["page"], "highlights").iter().filter_map(|h| h.as_str().map(str::to_string)).collect();
            let pair = window.viewport_size().width >= px(PAIR_AT);
            body(&c, &writing["page"], pair, cx)
        }
        false => {
            let msg = match s(&r, "writing_status") {
                "failed" => "This recap's page couldn't be written.",
                _ => "Writing this recap…",
            };
            div().child(kit::empty(&t, msg))
        }
    };
    shell.child(head.child(content)).children(menus).into_any_element()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's `test` attribute.
    use super::{components, flip, period, rows};
    use crate::fmt::s;
    use serde_json::{Value, json};
    use std::collections::HashSet;

    #[test]
    fn an_unknown_component_is_left_out() {
        let page = json!({"summary": "s", "components": [
            {"id": "hero", "span": "half"}, {"id": "heat", "span": "full"}, {"id": "made-up"}, {"id": "kpis", "span": "full"}, {"span": "half"}
        ]});
        let got: Vec<(String, bool)> = components(&page).iter().map(|(c, f)| (s(c, "id").to_string(), *f)).collect();
        assert_eq!(got, vec![("hero".into(), false), ("kpis".into(), true)]);
        assert!(components(&json!({"components": "nope"})).is_empty());
        assert!(components(&Value::Null).is_empty());
    }

    #[test]
    fn halves_pair_up_only_side_by_side() {
        let full = [false, false, true, false, true, false, false, false];
        assert_eq!(rows(&full, true), vec![vec![0, 1], vec![2], vec![3], vec![4], vec![5, 6], vec![7]]);
        assert_eq!(rows(&full, false).len(), full.len());
    }

    #[test]
    fn a_group_keeps_one_open() {
        let mut open = HashSet::new();
        flip(&mut open, "kpi|0");
        flip(&mut open, "facts");
        flip(&mut open, "kpi|2");
        assert!(open.contains("kpi|2") && open.contains("facts") && !open.contains("kpi|0"));
        flip(&mut open, "kpi|2");
        assert!(!open.contains("kpi|2"));
    }

    #[test]
    fn period_names_the_week_or_the_month() {
        assert_eq!(period(&json!({"kind": "weekly", "first_day": "2026-11-02", "last_day": "2026-11-08"})), "Nov 2 – Nov 8");
        assert_eq!(period(&json!({"kind": "monthly", "first_day": "2026-10-01", "last_day": "2026-10-31"})), "October 2026");
    }
}
