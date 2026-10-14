//! The Tasks page (`GET /lanes`): the tasks outside goals in four lanes, Queued (in the order the
//! board will start them), Running, Needs you and Done, each as tall as its cards. Chips narrow it
//! to one project, and for a project with areas a second row narrows it to one area or groups each
//! lane by area; a switch adds the tasks in goals. A card opens its task.
use crate::app::{MainWindow, Page};
use crate::fmt::{arr, i, opt_s, s};
use crate::theme::Theme;
use crate::ui::home::{Tile, tile_vm};
use crate::ui::kit;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

const PAGE_X: f32 = 28.;
const LANE_W: f32 = 280.;

#[derive(Default)]
pub struct State {
    /// The project chip picked; None shows every project.
    pub project: Option<String>,
    /// The picked project's area chip picked; None shows all of it.
    pub area: Option<String>,
    /// Split each lane by area while a project with areas is picked.
    pub by_area: bool,
    /// Also show the tasks in goals.
    pub goals: bool,
}

// ================================================================== view models

pub struct Card {
    pub project: String,
    pub area: String,
    /// The tile's pill when it says more than the lane does ("Starting", "In review", "Failed"…).
    pub pill: Option<(String, &'static str)>,
    pub tile: Tile,
}

pub struct Lane {
    pub key: &'static str,
    pub label: &'static str,
    pub cards: Vec<Card>,
}

/// (lane key, label, the tile status the lane already says).
const LANES: [(&str, &str, &str); 4] = [("queued", "Queued", "Queued"), ("running", "Running", "Working"), ("needs", "Needs you", "Needs you"), ("done", "Done", "Done")];

pub fn lanes_vm(v: &Value, project: Option<&str>, area: Option<&str>) -> Vec<Lane> {
    LANES
        .iter()
        .map(|(key, label, plain)| {
            let cards = arr(v, key)
                .iter()
                .filter(|c| project.is_none_or(|p| s(c, "project") == p) && area.is_none_or(|a| s(c, "area") == a))
                .map(|c| {
                    let mut tile = tile_vm(c);
                    let mut pill = (tile.status != *plain).then(|| (tile.status.clone(), tile.tone));
                    // A queued card says only what holds it back: a pill when that is a terminal,
                    // nothing when it just waits for Start.
                    if *key == "queued" && pill.is_none() {
                        match opt_s(c, "waiting") {
                            Some(w) if w.ends_with("press Start") => tile.line.clear(),
                            Some(w) => {
                                if let Some(busy) = w.strip_prefix("Waits for a free terminal: ") {
                                    pill = Some(("Waiting for a terminal".into(), "queued"));
                                    tile.line = busy.to_string();
                                }
                            }
                            None => {
                                pill = Some(("Waiting for a terminal".into(), "queued"));
                                tile.line.clear();
                            }
                        }
                    }
                    Card { project: s(c, "project").to_string(), area: s(c, "area").to_string(), pill, tile }
                })
                .collect();
            Lane { key, label, cards }
        })
        .collect()
}

pub struct ProjectChip {
    pub name: String,
    pub count: i64,
    /// The areas its cards are in, with how many each, in `.taskboard.toml`'s order.
    pub areas: Vec<(String, i64)>,
}

/// The project chips, by name.
pub fn projects_vm(v: &Value) -> Vec<ProjectChip> {
    arr(v, "projects")
        .iter()
        .map(|p| ProjectChip {
            name: s(p, "name").to_string(),
            count: i(p, "count"),
            areas: arr(p, "areas").iter().map(|a| (s(a, "name").to_string(), i(a, "count"))).collect(),
        })
        .collect()
}

/// A lane's cards split by area, in the areas' order, the ones with no area last (as None).
/// Areas with no cards in this lane are left out.
pub fn by_area<'a>(cards: &'a [Card], order: &[String]) -> Vec<(Option<String>, Vec<&'a Card>)> {
    let mut out: Vec<(Option<String>, Vec<&Card>)> = vec![];
    for a in order.iter().map(|a| Some(a.as_str())).chain([None]) {
        let these: Vec<&Card> = cards
            .iter()
            .filter(|c| match a {
                Some(a) => c.area == a,
                None => !order.contains(&c.area),
            })
            .collect();
        if !these.is_empty() {
            out.push((a.map(String::from), these));
        }
    }
    out
}

// ================================================================== render

fn lane_dot(t: &Theme, key: &str) -> Hsla {
    match key {
        "running" => t.accent,
        "needs" => t.warn,
        "done" => t.up,
        _ => t.faint,
    }
}

fn chip(t: &Theme, id: String, on: bool) -> Stateful<Div> {
    let hover = t.border_2;
    div()
        .id(SharedString::from(id))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(28.))
        .px(px(11.))
        .rounded_full()
        .border_1()
        .text_size(px(12.5))
        .cursor_pointer()
        .when(on, |d| d.bg(t.accent_soft).border_color(t.accent_line).text_color(t.accent_fg).font_weight(FontWeight::SEMIBOLD))
        .when(!on, |d| d.border_color(t.border).text_color(t.muted).hover(move |s| s.border_color(hover)))
}

fn switch(t: &Theme, on: bool) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .w(px(28.))
        .h(px(16.))
        .p(px(2.))
        .rounded_full()
        .when(on, |d| d.justify_end().bg(t.accent_btn))
        .when(!on, |d| d.bg(t.seg).border_1().border_color(t.border_2))
        .child(div().size(px(10.)).rounded_full().bg(if on { t.on_accent } else { t.faint }))
}

fn card(t: &Theme, lane: &str, c: &Card, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let open = c.tile.r.clone();
    let hover = t.border_2;
    let mut top = div().flex().items_center().gap(px(6.)).child(div().flex_none().font_family(t.mono_font.clone()).text_size(px(11.5)).text_color(t.muted).child(c.project.clone()));
    if !c.area.is_empty() {
        top = top.child(div().flex_none().px(px(6.)).rounded(px(6.)).bg(t.panel_2).text_size(px(11.)).text_color(t.text_2).child(c.area.clone()));
    }
    top = top.child(div().flex_1());
    if let Some((label, tone)) = &c.pill {
        top = top.child(kit::tone_pill(t, tone, label.clone()));
    }
    div()
        .id(SharedString::from(format!("lane-{}", c.tile.r)))
        .flex()
        .flex_col()
        .gap(px(6.))
        .p(px(10.))
        .rounded(px(10.))
        .border_1()
        .border_color(if lane == "needs" { t.warn_line } else { t.border })
        .bg(t.card)
        .cursor_pointer()
        .hover(move |s| s.border_color(hover))
        .on_click(cx.listener(move |m, _, _, cx| m.open_task(open.clone(), cx)))
        .child(top)
        .child(
            div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).line_height(relative(1.3)).line_clamp(2).child(
                StyledText::new(format!("{} {}", c.tile.r, c.tile.title))
                    .with_highlights([(0..c.tile.r.len(), HighlightStyle { color: Some(t.faint), font_weight: Some(FontWeight::NORMAL), ..Default::default() })]),
            ),
        )
        .when(!c.tile.line.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(if lane == "needs" { t.warn_fg } else { t.muted }).line_clamp(2).child(c.tile.line.clone())))
}

fn group_head(t: &Theme, label: String, n: usize) -> Div {
    div().px(px(4.)).pt(px(4.)).text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_2).child(format!("{label} · {n}"))
}

/// `groups`: the picked project's areas, to split the lane by them.
fn lane(t: &Theme, l: &Lane, groups: Option<&[String]>, cx: &mut Context<MainWindow>) -> Div {
    let mut head = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(4.))
        .pb(px(2.))
        .child(kit::dot(lane_dot(t, l.key), 7.))
        .child(div().text_size(px(11.)).font_weight(FontWeight::BOLD).text_color(t.faint).child(format!("{} · {}", l.label.to_uppercase(), l.cards.len())));
    if l.key == "queued" && l.cards.len() > 1 {
        head = head.child(div().ml_auto().text_size(px(11.5)).text_color(t.faint).child("runs top first"));
    }
    let mut d = div().flex().flex_col().flex_none().w(px(LANE_W)).gap(px(8.)).p(px(10.)).rounded(px(14.)).bg(t.col).border_1().border_color(t.divider).child(head);
    if l.cards.is_empty() {
        d = d.child(div().px(px(4.)).pb(px(2.)).text_size(px(12.5)).text_color(t.faint).child("None"));
    }
    match groups {
        Some(order) => {
            for (area, cards) in by_area(&l.cards, order) {
                d = d.child(group_head(t, area.unwrap_or_else(|| "No area".into()), cards.len()));
                for c in cards {
                    d = d.child(card(t, l.key, c, cx));
                }
            }
        }
        None => {
            for c in &l.cards {
                d = d.child(card(t, l.key, c, cx));
            }
        }
    }
    d
}

pub fn render(m: &mut MainWindow, _window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let shell = div().id("tasks-page").flex_1().min_w_0().h_full().overflow_y_scroll().restrict_scroll_to_axis();
    let goals = m.tasks.goals;
    let toggle = div()
        .id("tasks-goals")
        .flex()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .text_size(px(12.5))
        .text_color(t.muted)
        .child(switch(&t, goals))
        .child("Show tasks in goals")
        .on_click(cx.listener(|m, _, _, cx| {
            m.tasks.goals = !m.tasks.goals;
            m.refresh(cx);
        }));
    let head = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Tasks"))
        .child(toggle);
    let mut page = div().flex().flex_col().gap(px(18.)).px(px(PAGE_X)).pt(px(22.)).pb(px(32.)).line_height(relative(1.4)).child(head);

    let Some(v) = m.data.lanes.clone() else {
        let msg = if m.down.is_some() { "Can’t load the board." } else { "Loading…" };
        return shell.child(page.child(kit::empty(&t, msg))).into_any_element();
    };
    let projects = projects_vm(&v);
    // A picked project with nothing left in it falls back to every project.
    let picked = m.tasks.project.clone().filter(|p| projects.iter().any(|c| c.name == *p));
    let total: i64 = projects.iter().map(|c| c.count).sum();
    let mut chips = div().flex().flex_wrap().gap(px(6.)).child(
        chip(&t, "tasks-project-all".into(), picked.is_none()).child(format!("All projects · {total}")).on_click(cx.listener(|m, _, _, cx| {
            m.tasks.project = None;
            m.tasks.area = None;
            cx.notify();
        })),
    );
    for ProjectChip { name, count: n, .. } in &projects {
        let pick = name.clone();
        chips = chips.child(
            chip(&t, format!("tasks-project-{name}"), picked.as_deref() == Some(name.as_str()))
                .child(div().font_family(t.mono_font.clone()).text_size(px(12.)).child(name.clone()))
                .child(n.to_string())
                .on_click(cx.listener(move |m, _, _, cx| {
                    m.tasks.project = if m.tasks.project.as_deref() == Some(pick.as_str()) { None } else { Some(pick.clone()) };
                    m.tasks.area = None;
                    cx.notify();
                })),
        );
    }
    page = page.child(chips);

    // The picked project's areas beneath it: one narrows to it, or every lane splits by area.
    let areas: Vec<(String, i64)> = picked.as_ref().and_then(|p| projects.iter().find(|c| c.name == *p)).map(|c| c.areas.clone()).unwrap_or_default();
    let area = m.tasks.area.clone().filter(|a| areas.iter().any(|(n, _)| n == a));
    let mut groups: Option<Vec<String>> = None;
    if let Some(project) = picked.as_ref().filter(|_| !areas.is_empty()) {
        let n = projects.iter().find(|c| c.name == *project).map_or(0, |c| c.count);
        let mut row = div().flex().flex_wrap().items_center().gap(px(6.)).child(
            chip(&t, "tasks-area-all".into(), area.is_none()).child(format!("All of {project} · {n}")).on_click(cx.listener(|m, _, _, cx| {
                m.tasks.area = None;
                cx.notify();
            })),
        );
        for (name, n) in &areas {
            let pick = name.clone();
            row = row.child(
                chip(&t, format!("tasks-area-{name}"), area.as_deref() == Some(name.as_str())).child(name.clone()).child(n.to_string()).on_click(cx.listener(move |m, _, _, cx| {
                    m.tasks.area = if m.tasks.area.as_deref() == Some(pick.as_str()) { None } else { Some(pick.clone()) };
                    cx.notify();
                })),
            );
        }
        if area.is_none() {
            let on = m.tasks.by_area;
            row = row.child(
                div()
                    .id("tasks-by-area")
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .ml_auto()
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .text_color(t.muted)
                    .child(switch(&t, on))
                    .child("Group by area")
                    .on_click(cx.listener(|m, _, _, cx| {
                        m.tasks.by_area = !m.tasks.by_area;
                        cx.notify();
                    })),
            );
            if on {
                groups = Some(areas.iter().map(|(a, _)| a.clone()).collect());
            }
        }
        page = page.child(row);
    }

    let lanes = lanes_vm(&v, picked.as_deref(), area.as_deref());
    if lanes.iter().all(|l| l.cards.is_empty()) {
        let msg = if goals { "Nothing is queued, running or done today." } else { "Nothing outside a goal is queued, running or done today." };
        return shell.child(page.child(kit::empty(&t, msg))).into_any_element();
    }
    // The lanes sit side by side and scroll sideways rather than wrapping into a grid.
    let mut row = div().id("tasks-lanes").flex().items_start().gap(px(14.)).overflow_x_scroll().restrict_scroll_to_axis();
    for l in &lanes {
        row = row.child(lane(&t, l, groups.as_deref(), cx));
    }
    shell.child(page.child(row)).into_any_element()
}

/// The home page's "N queued" on the card of a project's tasks outside goals: the Tasks page,
/// narrowed to that project.
pub fn open_for(m: &mut MainWindow, project: &str, cx: &mut Context<MainWindow>) {
    m.tasks.project = Some(project.to_string());
    m.tasks.area = None;
    m.tasks.goals = false;
    m.go(Page::Tasks, cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    // Beats the glob's `gpui_kit::test`.
    use ::core::prelude::v1::test;
    use serde_json::json;

    #[test]
    fn lanes_keep_the_queue_within_the_picked_project() {
        let card = |r: &str, project: &str, status: &str| json!({"ref": r, "title": r, "project": project, "status": status});
        let mut starting = card("T3", "web", "queued");
        starting["starting"] = json!(true);
        let mut failed = card("T5", "api", "done");
        failed["failed"] = json!(true);
        let v = json!({
            "queued": [card("T1", "web", "queued"), card("T2", "api", "queued"), starting],
            "running": [card("T4", "web", "working")],
            "needs": [],
            "done": [failed],
        });
        let all = lanes_vm(&v, None, None);
        assert_eq!(all.iter().map(|l| l.key).collect::<Vec<_>>(), ["queued", "running", "needs", "done"]);
        assert_eq!(all[0].cards.iter().map(|c| c.tile.r.as_str()).collect::<Vec<_>>(), ["T1", "T2", "T3"]);
        // Nothing holds T1 back but a terminal.
        assert_eq!(all[0].cards[0].pill.as_ref().map(|p| p.0.as_str()), Some("Waiting for a terminal"));
        assert!(all[0].cards[0].tile.line.is_empty());
        // The lane already says Queued and Working; a card says more only when it differs.
        assert_eq!(all[0].cards[2].pill.as_ref().map(|p| p.0.as_str()), Some("Starting"));
        assert!(all[1].cards[0].pill.is_none());
        assert_eq!(all[3].cards[0].pill.as_ref().map(|p| p.0.as_str()), Some("Failed"));

        let web = lanes_vm(&v, Some("web"), None);
        assert_eq!(web[0].cards.iter().map(|c| c.tile.r.as_str()).collect::<Vec<_>>(), ["T1", "T3"]);
        assert!(web[3].cards.is_empty());
    }

    #[test]
    fn queued_cards_say_what_holds_them_back() {
        let card = |r: &str, waiting: &str| json!({"ref": r, "title": r, "project": "web", "status": "queued", "waiting": waiting});
        let v = json!({
            "queued": [card("T1", "Waits for you to press Start"), card("T2", "Waits for a free terminal: 2 of 2 busy in web"), card("T3", "Waits for PR #4 to merge")],
            "running": [], "needs": [], "done": [],
        });
        let q = &lanes_vm(&v, None, None)[0].cards;
        let got: Vec<(Option<&str>, &str)> = q.iter().map(|c| (c.pill.as_ref().map(|p| p.0.as_str()), c.tile.line.as_str())).collect();
        assert_eq!(got, [(None, ""), (Some("Waiting for a terminal"), "2 of 2 busy in web"), (None, "Waits for PR #4 to merge")]);
    }

    #[test]
    fn a_project_narrows_to_an_area_or_splits_by_area() {
        let card = |r: &str, area: Value| json!({"ref": r, "title": r, "project": "rune", "area": area, "status": "working"});
        let v = json!({
            "queued": [], "needs": [], "done": [],
            "running": [card("T1", json!("Art")), card("T2", Value::Null), card("T3", json!("Quill")), card("T4", json!("Art")), card("T5", json!("Gone"))],
            "projects": [{"name": "rune", "count": 5, "areas": [{"name": "Quill", "count": 1}, {"name": "Art", "count": 2}]}],
        });
        let p = &projects_vm(&v)[0];
        assert_eq!(p.areas, [("Quill".to_string(), 1), ("Art".to_string(), 2)]);

        let art = lanes_vm(&v, Some("rune"), Some("Art"));
        assert_eq!(art[1].cards.iter().map(|c| c.tile.r.as_str()).collect::<Vec<_>>(), ["T1", "T4"]);

        let all = lanes_vm(&v, Some("rune"), None);
        let order: Vec<String> = p.areas.iter().map(|(a, _)| a.clone()).collect();
        let split = by_area(&all[1].cards, &order);
        let groups: Vec<(Option<&str>, Vec<&str>)> =
            split.iter().map(|(a, cs)| (a.as_deref(), cs.iter().map(|c| c.tile.r.as_str()).collect())).collect();
        // The file's order; no area (or one the file no longer lists) last; an empty lane has no groups.
        assert_eq!(groups, [(Some("Quill"), vec!["T3"]), (Some("Art"), vec!["T1", "T4"]), (None, vec!["T2", "T5"])]);
        assert!(by_area(&all[0].cards, &order).is_empty());
    }
}
