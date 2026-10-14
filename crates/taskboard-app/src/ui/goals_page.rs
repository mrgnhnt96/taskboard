//! The Goals page (`GET /goals`): every goal as a card, in Active / Deprioritized / Finished views.
//! Finished goals group by the week they finished in, newest first; a card opens its goal.
use crate::app::MainWindow;
use crate::fmt::{self, i, s};
use crate::theme::Theme;
use crate::ui::modals::locale_cmp;
use crate::ui::goals::{goal_counts, nav_kind};
use crate::ui::kit::shadow_seg;
use crate::ui::{goal, kit};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

const PAGE_X: f32 = 28.;
const COLS: usize = 3;

pub const VIEWS: [(&str, &str); 3] = [("active", "Active"), ("deprio", "Deprioritized"), ("done", "Finished")];

pub struct State {
    pub view: &'static str,
}

impl Default for State {
    fn default() -> Self {
        State { view: "active" }
    }
}

// ================================================================== view models

/// Where a goal sits on the Goals page and in the sidebar, in that order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalGroup {
    /// Work started: something done, running, waiting on you or in review.
    Active,
    /// Not started yet, or chained after a goal that isn't finished.
    Next,
    Paused,
    Deprioritized,
    Finished,
}

pub fn goal_group(g: &Value) -> GoalGroup {
    match nav_kind(g) {
        "done" => GoalGroup::Finished,
        "deprio" => GoalGroup::Deprioritized,
        _ if fmt::b(g, "paused") => GoalGroup::Paused,
        _ if fmt::opt_s(g, "after_blocker").is_some() => GoalGroup::Next,
        _ if i(g, "done") == 0 && i(g, "active") == 0 && i(g, "needs") == 0 && i(g, "queued") == 0 && fmt::opt_s(g, "stopped").is_none() && fmt::arr(g, "prs_open").is_empty() => GoalGroup::Next,
        _ => GoalGroup::Active,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub r: String,
    pub name: String,
    pub project: String,
    /// The done fraction, for goals still going.
    pub bar: Option<f32>,
    pub left: String,
    pub right: String,
    pub tone: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub title: String,
    pub sub: String,
    pub cards: Vec<Card>,
}

/// (view key, label, how many goals).
pub fn tabs(goals: &[&Value]) -> Vec<(&'static str, &'static str, usize)> {
    VIEWS.iter().map(|(k, l)| (*k, *l, goals.iter().filter(|g| nav_kind(g) == *k).count())).collect()
}

fn prs_merged(g: &Value) -> String {
    match (i(g, "prs_planned") - i(g, "prs_canceled")).max(0) {
        0 => "No PR".into(),
        1 => "1 PR merged".into(),
        n => format!("{n} PRs merged"),
    }
}

fn finished_card(g: &Value) -> Card {
    let at = fmt::parse(s(g, "finished_at")).map(|t| fmt::local(t).format("%a, %b %-d · %-I:%M %p").to_string()).unwrap_or_default();
    Card { r: fmt::ref_of(g, "G"), name: s(g, "name").into(), project: s(g, "project").into(), bar: None, left: prs_merged(g), right: at, tone: "up" }
}

/// The status of a goal still going, from its summary's counts (the summary has no task list).
fn going_status(g: &Value) -> (String, &'static str) {
    let c = goal_counts(g);
    let queued = i(g, "queued");
    if c.n > 0 && c.done == c.n {
        return match (c.prs, i(g, "bits_waiting")) {
            (0, 0) => ("Done".into(), "up"),
            (0, bits) => (format!("Waiting on {}", fmt::plural(bits, "bit", "bits")), "warn"),
            (1, _) => ("Awaiting merge".into(), "accent"),
            (n, _) => (format!("{n} PRs awaiting merge"), "accent"),
        };
    }
    if fmt::b(g, "paused") {
        ("Paused".into(), "warn")
    } else if i(g, "needs") > 0 || fmt::opt_s(g, "stopped").is_some() {
        ("Waiting on you".into(), "warn")
    } else if c.active > 0 {
        ("In progress".into(), "accent")
    } else if i(g, "blocked") > 0 && i(g, "blocked") + i(g, "held") >= queued {
        ("Blocked".into(), "warn")
    } else if queued > 0 {
        ("Queued".into(), "accent")
    } else if c.n == 0 {
        ("No tasks yet".into(), "muted")
    } else {
        ("Not started".into(), "muted")
    }
}

fn going_card(g: &Value) -> Card {
    let c = goal_counts(g);
    let (right, tone) = going_status(g);
    let left = if c.n == 0 {
        "No tasks yet".into()
    } else if c.active > 0 {
        format!("{} of {} done · {} running", c.done, c.n, c.active)
    } else {
        format!("{} of {} done", c.done, c.n)
    };
    let bar = (c.n > 0).then(|| c.done as f32 / c.n as f32);
    Card { r: fmt::ref_of(g, "G"), name: s(g, "name").into(), project: s(g, "project").into(), bar, left, right, tone }
}

/// "Oct 4 – 10", or "Sep 27 – Oct 3" across a month.
fn span(from: NaiveDate, to: NaiveDate) -> String {
    let end = if from.month() == to.month() { to.format("%-d") } else { to.format("%b %-d") };
    format!("{} – {}", from.format("%b %-d"), end)
}

/// The finished goals by the local week (Sunday on) they finished in: this week, last week, earlier.
fn by_week(mut goals: Vec<&Value>, now: DateTime<Utc>) -> Vec<Section> {
    let at = |g: &Value| fmt::parse(s(g, "finished_at"));
    goals.sort_by_key(|g| std::cmp::Reverse(at(g).map(|t| t.timestamp_millis()).unwrap_or(0)));
    let today = fmt::local(now).date_naive();
    let this_week = today - Duration::days(today.weekday().num_days_from_sunday() as i64);
    let last_week = this_week - Duration::days(7);
    let weeks = [
        ("This week", span(this_week, today)),
        ("Last week", span(last_week, this_week - Duration::days(1))),
        ("Earlier", format!("Before {}", last_week.format("%b %-d"))),
    ];
    let week_of = |g: &Value| match at(g).map(|t| fmt::local(t).date_naive()) {
        Some(d) if d >= this_week => 0,
        Some(d) if d >= last_week => 1,
        _ => 2,
    };
    weeks
        .into_iter()
        .enumerate()
        .map(|(n, (title, sub))| Section { title: title.into(), sub, cards: goals.iter().filter(|g| week_of(g) == n).map(|g| finished_card(g)).collect() })
        .filter(|sec| !sec.cards.is_empty())
        .collect()
}

pub fn sections(goals: &[&Value], view: &str, now: DateTime<Utc>) -> Vec<Section> {
    let mut list: Vec<&Value> = goals.iter().copied().filter(|g| nav_kind(g) == view).collect();
    if view == "done" {
        return by_week(list, now);
    }
    list.sort_by(|a, b| locale_cmp(s(a, "project"), s(b, "project")).then(i(a, "id").cmp(&i(b, "id"))));
    if list.is_empty() {
        return Vec::new();
    }
    vec![Section { title: String::new(), sub: String::new(), cards: list.into_iter().map(going_card).collect() }]
}

fn empty_line(view: &str) -> &'static str {
    match view {
        "deprio" => "No deprioritized goals",
        "done" => "No finished goals",
        _ => "No active goals",
    }
}

// ================================================================== render

fn tab_bar(t: &Theme, view: &str, tabs: &[(&'static str, &'static str, usize)], cx: &mut Context<MainWindow>) -> Div {
    let mut bar = div().flex().flex_none().gap(px(2.)).p(px(3.)).rounded(px(9.)).bg(t.seg);
    for (k, label, n) in tabs.iter().copied() {
        let on = k == view;
        let hover = t.text;
        let (count_fg, count_bg) = if on { (t.accent_fg, t.accent_soft) } else { (t.text_2, t.border) };
        bar = bar.child(
            div()
                .id(SharedString::from(format!("goals-view-{k}")))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(30.))
                .px(px(12.))
                .rounded(px(7.))
                .text_size(px(13.))
                .cursor_pointer()
                .when(on, |d| d.bg(t.card).text_color(t.text).font_weight(FontWeight::SEMIBOLD).shadow(shadow_seg(t)))
                .when(!on, |d| d.text_color(t.muted).hover(move |s| s.text_color(hover)))
                .child(label)
                .child(div().px(px(6.)).rounded(px(8.)).text_size(px(11.)).font_weight(FontWeight::MEDIUM).bg(count_bg).text_color(count_fg).child(n.to_string()))
                .on_click(cx.listener(move |m, _, _, cx| {
                    m.goals.view = k;
                    cx.notify();
                })),
        );
    }
    bar
}

fn card(t: &Theme, c: &Card, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let open = c.r.clone();
    let hover = t.border_2;
    let (tone_fg, _) = kit::tone(t, c.tone);
    let head = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(11.))
        .child(div().font_family(t.mono_font.clone()).font_weight(FontWeight::SEMIBOLD).text_color(t.goal).child(c.r.clone()))
        .child(div().px(px(7.)).rounded(px(8.)).bg(t.panel_2).text_color(t.text_2).child(c.project.clone()));
    let mut body = div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(head)
        .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).line_height(relative(1.3)).line_clamp(2).child(c.name.clone()));
    if let Some(f) = c.bar {
        body = body.child(div().mt(px(4.)).h(px(6.)).rounded(px(3.)).bg(t.divider).overflow_hidden().child(div().h_full().rounded(px(3.)).bg(t.accent).w(relative(f))));
    }
    let foot = div()
        .mt_auto()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .pt(px(10.))
        .border_t_1()
        .border_color(t.divider)
        .text_size(px(12.))
        .child(div().min_w_0().truncate().text_color(t.muted).child(c.left.clone()))
        .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(tone_fg).child(c.right.clone()));
    div()
        .id(SharedString::from(format!("goals-card-{}", c.r)))
        .flex()
        .flex_col()
        .flex_1()
        .flex_basis(px(0.))
        .min_w_0()
        .gap(px(14.))
        .p(px(16.))
        .rounded(px(12.))
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .cursor_pointer()
        .hover(move |s| s.border_color(hover))
        .on_click(cx.listener(move |m, _, _, cx| goal::open(m, &open, false, cx)))
        .child(body)
        .child(foot)
}

fn grid(t: &Theme, cards: &[Card], cx: &mut Context<MainWindow>) -> Div {
    let mut rows = div().flex().flex_col().gap(px(14.));
    for chunk in cards.chunks(COLS) {
        let mut row = div().flex().items_stretch().gap(px(14.));
        for c in chunk {
            row = row.child(card(t, c, cx));
        }
        for _ in chunk.len()..COLS {
            row = row.child(div().flex_1().flex_basis(px(0.)));
        }
        rows = rows.child(row);
    }
    rows
}

pub fn render(m: &mut MainWindow, _window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let goals: Vec<Value> = m.data.goals.iter().filter(|g| !fmt::b(g, "archived")).cloned().collect();
    let goals: Vec<&Value> = goals.iter().collect();
    let view = m.goals.view;
    let head = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Goals"))
        .child(tab_bar(&t, view, &tabs(&goals), cx));
    let mut page = div().flex().flex_col().gap(px(24.)).px(px(PAGE_X)).pt(px(22.)).pb(px(32.)).line_height(relative(1.4)).child(head);
    let shell = div().id("goals-page").flex_1().min_w_0().h_full().overflow_y_scroll().restrict_scroll_to_axis();

    if !m.goals_loaded() && goals.is_empty() {
        let msg = if m.down.is_some() { "Can’t load the board." } else { "Loading…" };
        return shell.child(page.child(kit::empty(&t, msg))).into_any_element();
    }
    let secs = sections(&goals, view, fmt::now());
    if secs.is_empty() {
        return shell.child(page.child(kit::empty(&t, empty_line(view)))).into_any_element();
    }
    for sec in &secs {
        let mut block = div().flex().flex_col().gap(px(12.));
        if !sec.title.is_empty() {
            block = block.child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .child(div().text_size(px(15.)).font_weight(FontWeight::BOLD).child(sec.title.clone()))
                    .child(div().text_size(px(12.)).text_color(t.muted).child(sec.sub.clone())),
            );
        }
        page = page.child(block.child(grid(&t, &sec.cards, cx)));
    }
    shell.child(page).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // Beats the glob's `gpui_kit::test`.
    use ::core::prelude::v1::test;
    use chrono::TimeZone;
    use serde_json::json;

    fn goal(id: i64, project: &str, done: i64, total: i64, extra: Value) -> Value {
        let mut g = json!({"id": id, "ref": format!("G{id}"), "name": format!("Goal {id}"), "project": project, "done": done, "total": total, "active": 0, "prs_open": []});
        for (k, v) in extra.as_object().unwrap() {
            g[k] = v.clone();
        }
        g
    }

    fn noon(y: i32, m: u32, d: u32) -> String {
        fmt::local(Utc::now()).offset().with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap().with_timezone(&Utc).to_rfc3339()
    }

    #[test]
    fn tabs_count_each_view() {
        let gs = [
            goal(1, "web", 1, 2, json!({})),
            goal(2, "web", 0, 2, json!({"deprioritized": true})),
            goal(3, "api", 2, 2, json!({"finished_at": noon(2026, 10, 12)})),
            goal(4, "api", 1, 1, json!({"finished_at": noon(2026, 10, 1), "deprioritized": true})),
        ];
        let refs: Vec<&Value> = gs.iter().collect();
        assert_eq!(tabs(&refs), [("active", "Active", 1), ("deprio", "Deprioritized", 1), ("done", "Finished", 2)]);
    }

    #[test]
    fn finished_goals_group_by_the_week_they_finished_in() {
        let fin = |id, d: u32, m: u32, planned: i64| goal(id, "web", 2, 2, json!({"finished_at": noon(2026, m, d), "prs_planned": planned, "prs_canceled": 0}));
        let gs = [fin(1, 30, 9, 1), fin(2, 6, 10, 2), fin(3, 12, 10, 0), fin(4, 11, 10, 3), fin(5, 4, 10, 1)];
        let refs: Vec<&Value> = gs.iter().collect();
        let now = fmt::parse(&noon(2026, 10, 13)).unwrap();
        let secs = sections(&refs, "done", now);
        let got: Vec<(&str, &str, Vec<&str>)> = secs.iter().map(|s| (s.title.as_str(), s.sub.as_str(), s.cards.iter().map(|c| c.r.as_str()).collect())).collect();
        assert_eq!(got, [("This week", "Oct 11 – 13", vec!["G3", "G4"]), ("Last week", "Oct 4 – 10", vec!["G2", "G5"]), ("Earlier", "Before Oct 4", vec!["G1"])]);
        let g3 = &secs[0].cards[0];
        assert_eq!((g3.left.as_str(), g3.right.as_str(), g3.bar), ("No PR", "Mon, Oct 12 · 12:00 PM", None));
        assert_eq!(secs[0].cards[1].left, "3 PRs merged");
        assert_eq!(secs[2].cards[0].left, "1 PR merged");
    }

    #[test]
    fn a_week_across_months_names_both() {
        let d = |m, d| NaiveDate::from_ymd_opt(2026, m, d).unwrap();
        assert_eq!(span(d(9, 27), d(10, 3)), "Sep 27 – Oct 3");
    }

    #[test]
    fn active_cards_show_progress_and_status() {
        let gs = [goal(10, "web", 4, 6, json!({"active": 2})), goal(11, "api", 0, 8, json!({"planned": 8})), goal(12, "api", 0, 0, json!({}))];
        let refs: Vec<&Value> = gs.iter().collect();
        let secs = sections(&refs, "active", Utc::now());
        assert_eq!(secs.len(), 1);
        let cards = &secs[0].cards;
        assert_eq!(cards.iter().map(|c| c.r.as_str()).collect::<Vec<_>>(), ["G11", "G12", "G10"]);
        assert_eq!((cards[2].left.as_str(), cards[2].bar), ("4 of 6 done · 2 running", Some(4. / 6.)));
        assert_eq!((cards[0].left.as_str(), cards[0].bar), ("0 of 8 done", Some(0.)));
        assert_eq!((cards[1].left.as_str(), cards[1].bar), ("No tasks yet", None));
        assert_eq!(cards.iter().map(|c| c.right.as_str()).collect::<Vec<_>>(), ["Not started", "No tasks yet", "In progress"]);
        assert!(sections(&refs, "deprio", Utc::now()).is_empty());
    }

    #[test]
    fn a_goal_with_every_task_done_still_waits_on_its_prs() {
        let g = goal(1, "web", 2, 2, json!({"prs_open": [4, 5]}));
        assert_eq!(going_status(&g), ("2 PRs awaiting merge".to_string(), "accent"));
        let g = goal(2, "web", 1, 3, json!({"needs": 1, "active": 1}));
        assert_eq!(going_status(&g).0, "Waiting on you");
    }
}
