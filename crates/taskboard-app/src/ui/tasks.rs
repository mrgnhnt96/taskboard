//! The Tasks page (`GET /lanes`): the tasks outside goals in four lanes, Queued (in the order the
//! board will start them), Running, Needs you and Done, each as tall as its cards. Chips narrow it
//! to one project; a switch adds the tasks in goals. A card opens its task.
use crate::app::{MainWindow, Page};
use crate::fmt::{arr, i, s};
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
    /// Also show the tasks in goals.
    pub goals: bool,
}

// ================================================================== view models

pub struct Card {
    pub project: String,
    /// Queued only: "Next", "2nd", "3rd"…
    pub order: Option<String>,
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

pub fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    if n == 1 { "Next".into() } else { format!("{n}{suffix}") }
}

pub fn lanes_vm(v: &Value, project: Option<&str>) -> Vec<Lane> {
    LANES
        .iter()
        .map(|(key, label, plain)| {
            let cards = arr(v, key)
                .iter()
                .filter(|c| project.is_none_or(|p| s(c, "project") == p))
                .enumerate()
                .map(|(ix, c)| {
                    let tile = tile_vm(c);
                    Card {
                        project: s(c, "project").to_string(),
                        order: (*key == "queued").then(|| ordinal(ix + 1)),
                        pill: (tile.status != *plain).then(|| (tile.status.clone(), tile.tone)),
                        tile,
                    }
                })
                .collect();
            Lane { key, label, cards }
        })
        .collect()
}

/// The project chips: (name, how many cards), by name.
pub fn projects_vm(v: &Value) -> Vec<(String, i64)> {
    arr(v, "projects").iter().map(|p| (s(p, "name").to_string(), i(p, "count"))).collect()
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
    let mut top = div().flex().items_center().gap(px(6.)).child(div().flex_1().min_w_0().truncate().font_family(t.mono_font.clone()).text_size(px(11.5)).text_color(t.muted).child(c.project.clone()));
    if let Some(o) = &c.order {
        top = top.child(div().flex_none().px(px(7.)).rounded_full().bg(t.seg).text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(t.muted).child(o.clone()));
    }
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
        .child(div().text_size(px(12.)).text_color(if lane == "needs" { t.warn_fg } else { t.muted }).line_clamp(2).child(c.tile.line.clone()))
}

fn lane(t: &Theme, l: &Lane, cx: &mut Context<MainWindow>) -> Div {
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
    for c in &l.cards {
        d = d.child(card(t, l.key, c, cx));
    }
    d
}

pub fn render(m: &mut MainWindow, _window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let shell = div().id("tasks-page").flex_1().min_w_0().h_full().overflow_y_scroll();
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
    let picked = m.tasks.project.clone().filter(|p| projects.iter().any(|(n, _)| n == p));
    let total: i64 = projects.iter().map(|(_, n)| n).sum();
    let mut chips = div().flex().flex_wrap().gap(px(6.)).child(
        chip(&t, "tasks-project-all".into(), picked.is_none()).child(format!("All projects · {total}")).on_click(cx.listener(|m, _, _, cx| {
            m.tasks.project = None;
            cx.notify();
        })),
    );
    for (name, n) in &projects {
        let pick = name.clone();
        chips = chips.child(
            chip(&t, format!("tasks-project-{name}"), picked.as_deref() == Some(name.as_str()))
                .child(div().font_family(t.mono_font.clone()).text_size(px(12.)).child(name.clone()))
                .child(n.to_string())
                .on_click(cx.listener(move |m, _, _, cx| {
                    m.tasks.project = if m.tasks.project.as_deref() == Some(pick.as_str()) { None } else { Some(pick.clone()) };
                    cx.notify();
                })),
        );
    }
    page = page.child(chips);

    let lanes = lanes_vm(&v, picked.as_deref());
    if lanes.iter().all(|l| l.cards.is_empty()) {
        let msg = if goals { "Nothing is queued, running or done today." } else { "Nothing outside a goal is queued, running or done today." };
        return shell.child(page.child(kit::empty(&t, msg))).into_any_element();
    }
    let mut row = div().flex().flex_wrap().items_start().gap(px(14.));
    for l in &lanes {
        row = row.child(lane(&t, l, cx));
    }
    shell.child(page.child(row)).into_any_element()
}

/// The home page's "N queued" on the card of a project's tasks outside goals: the Tasks page,
/// narrowed to that project.
pub fn open_for(m: &mut MainWindow, project: &str, cx: &mut Context<MainWindow>) {
    m.tasks.project = Some(project.to_string());
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
    fn ordinals_say_next_then_count() {
        let got: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 23].into_iter().map(ordinal).collect();
        assert_eq!(got, ["Next", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "23rd"]);
    }

    #[test]
    fn lanes_number_the_queue_within_the_picked_project() {
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
        let all = lanes_vm(&v, None);
        assert_eq!(all.iter().map(|l| l.key).collect::<Vec<_>>(), ["queued", "running", "needs", "done"]);
        assert_eq!(all[0].cards.iter().map(|c| c.order.clone().unwrap()).collect::<Vec<_>>(), ["Next", "2nd", "3rd"]);
        // The lane already says Queued and Working; a card says more only when it differs.
        assert!(all[0].cards[0].pill.is_none());
        assert_eq!(all[0].cards[2].pill.as_ref().map(|p| p.0.as_str()), Some("Starting"));
        assert!(all[1].cards[0].pill.is_none());
        assert_eq!(all[3].cards[0].pill.as_ref().map(|p| p.0.as_str()), Some("Failed"));

        let web = lanes_vm(&v, Some("web"));
        assert_eq!(web[0].cards.iter().map(|c| (c.tile.r.clone(), c.order.clone().unwrap())).collect::<Vec<_>>(), [("T1".to_string(), "Next".to_string()), ("T3".to_string(), "2nd".to_string())]);
        assert!(web[3].cards.is_empty());
    }
}
