//! The sidebar on every page: search, the pages with their counts, the goals in flight with a
//! status bar and last update each, and the board's health (Midna, work hours, usage, hooks,
//! accounts, updates, settings) at the foot. Its top is the window's title bar, clear of the traffic lights.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, b, i, s};
use crate::theme::{Theme, ThemeMode};
use crate::ui::goals_page::{GoalGroup, Tally, goal_group};
use crate::ui::{kit, status};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

pub const WIDTH: f32 = 248.;

/// The goals pinned in the sidebar: in progress, then up next and paused, newest first in each.
pub fn pinned_goals(goals: &[Value]) -> Vec<&Value> {
    let mut v: Vec<&Value> = goals.iter().filter(|g| matches!(goal_group(g), GoalGroup::Active | GoalGroup::Next | GoalGroup::Paused)).collect();
    v.sort_by_key(|g| (goal_group(g) as u8, std::cmp::Reverse(i(g, "id"))));
    v
}

/// The goals in the sidebar: every one in progress, running or not, newest first. Up next, paused
/// and finished ones are on the Goals page.
pub fn active_goals(goals: &[Value]) -> Vec<&Value> {
    let mut v: Vec<&Value> = goals.iter().filter(|g| goal_group(g) == GoalGroup::Active).collect();
    v.sort_by_key(|g| std::cmp::Reverse(i(g, "id")));
    v
}

/// Minutes a goal can go without an update before it looks stuck: a running task should show
/// signs of life often, work in review or between waves less so.
const STALE_RUNNING_MINS: i64 = 30;
const STALE_WAITING_MINS: i64 = 120;

/// The goal hasn't moved for longer than its work should take to show something. A goal waiting
/// on you is never stale: it's yours to move.
pub fn stale(g: &Value) -> bool {
    let Some(mins) = fmt::opt_s(g, "last_update").and_then(fmt::minutes_since) else { return false };
    if i(g, "needs") > 0 {
        return false;
    }
    mins > if i(g, "active") > 0 { STALE_RUNNING_MINS } else { STALE_WAITING_MINS }
}

/// The goal's line under its name: "2 of 7 · 1 needs you", "Paused", "After G22".
pub fn goal_sub(g: &Value) -> String {
    let count = format!("{} of {}", i(g, "done"), i(g, "total"));
    if b(g, "paused") {
        return format!("Paused · {count}");
    }
    if let Some(after) = arr(g, "after").first().and_then(Value::as_str).filter(|_| fmt::opt_s(g, "after_blocker").is_some()) {
        return format!("After {after}");
    }
    if i(g, "needs") > 0 {
        return format!("{count} · {} you", fmt::plural(i(g, "needs"), "needs", "need"));
    }
    if fmt::opt_s(g, "stopped").is_some() {
        return format!("{count} · stopped for you");
    }
    count
}

/// The sidebar's background: a step darker than the page in light, a step lighter in dark.
pub fn bg(t: &Theme) -> Hsla {
    if t.mode == ThemeMode::Dark { t.tint } else { t.col }
}

fn nav_item(t: &Theme, id: &'static str, label: &'static str, mark: Div, count: Option<(String, bool)>, on: bool) -> Stateful<Div> {
    let (text, muted) = (t.text, t.text_2);
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(10.))
        .h(px(32.))
        .px(px(10.))
        .rounded(px(7.))
        .text_size(px(14.))
        .cursor_pointer()
        .when(on, |d| d.bg(t.card).text_color(text).font_weight(FontWeight::BOLD).shadow(kit::shadow_seg(t)))
        .when(!on, |d| d.text_color(muted).hover(move |s| s.text_color(text)))
        .child(mark)
        .child(div().flex_1().min_w_0().truncate().child(label))
        .children(count.map(|(n, loud)| {
            div().text_size(px(12.)).text_color(if loud { t.warn_fg } else { t.muted }).when(loud, |d| d.font_weight(FontWeight::BOLD)).child(n)
        }))
}

fn square(color: Hsla, outline: bool) -> Div {
    let d = div().flex_none().size(px(8.)).rounded(px(2.));
    if outline { d.border_1().border_color(color) } else { d.bg(color) }
}

pub fn render(m: &mut MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> AnyElement {
    let st = m.state_rc();
    let page = m.page.clone();
    let needs = m.data.home.as_ref().map(|h| arr(h, "needs").iter().filter(|a| !crate::ui::prwatch::shown_as_master(&st, a)).count() as i64).unwrap_or_else(|| m.needs_count());
    let running = arr(&st, "sessions").iter().filter(|x| s(x, "status") == "working").count();
    let projects = arr(&st, "projects").len();
    let counts = &st["counts"];
    let open_tasks = i(counts, "queued") + i(counts, "working") + i(counts, "needs");
    let all_goals = m.data.goals.clone();
    let goals = active_goals(&all_goals);
    let active_goals = goals.len();
    let untriaged = i(counts, "untriaged");

    let nav = |id: &'static str, label: &'static str, mark: Div, count: Option<(String, bool)>, to: Page, on: bool, cx: &mut Context<MainWindow>| {
        nav_item(t, id, label, mark, count, on).on_click(cx.listener(move |m, _, _, cx| {
            m.go(to.clone(), cx);
        }))
    };
    let on = |p: Page| page == p;
    let mut col = div()
        .id("sidebar")
        .flex()
        .flex_col()
        .flex_none()
        .w(px(WIDTH))
        .h_full()
        .bg(bg(t))
        .border_r_1()
        .border_color(t.border)
        .px(px(12.))
        .pb(px(10.))
        .gap(px(4.))
        // The traffic lights sit over this strip; it moves the window like a title bar.
        .child(div().flex_none().h(px(44.)).on_mouse_down(MouseButton::Left, crate::settings::drag_window))
        .child(search_box(t, cx))
        .child(div().h(px(6.)))
        .child(nav("nav-needs", "Needs you", kit::dot(t.warn, 8.), (needs > 0).then(|| (needs.to_string(), true)), Page::Board, on(Page::Board), cx))
        .child(nav("nav-running", "Running", kit::dot(t.accent, 8.), (running > 0).then(|| (running.to_string(), false)), Page::Sessions, on(Page::Sessions), cx))
        .child(nav("nav-projects", "Projects", square(t.text, true), (projects > 0).then(|| (projects.to_string(), false)), Page::Projects, on(Page::Projects), cx))
        .child(nav("nav-goals", "Goals", square(t.goal, false), (active_goals > 0).then(|| (active_goals.to_string(), false)), Page::Goals, on(Page::Goals) || matches!(page, Page::Goal(_)), cx))
        .child(nav("nav-tasks", "Tasks", square(t.muted, true), (open_tasks > 0).then(|| (open_tasks.to_string(), false)), Page::Tasks, on(Page::Tasks), cx))
        .child(nav("nav-backlog", "Backlog", kit::dot(t.backlog_dot, 8.), (untriaged > 0).then(|| (untriaged.to_string(), false)), Page::Backlog, on(Page::Backlog), cx))
        .child(nav("nav-ideas", "Ideas", kit::dot(t.goal, 8.), None, Page::Ideas, on(Page::Ideas) || matches!(page, Page::Idea(_)), cx))
        .child(nav("nav-days", "Days", square(t.results, false), None, Page::Days, on(Page::Days) || page == Page::Review, cx));
    if !crate::ui::releases::projects_on(&st).is_empty() {
        col = col.child(nav("nav-releases", "Releases", square(t.up, true), None, Page::Releases, on(Page::Releases) || matches!(page, Page::ReleasesOf(_)), cx));
    }

    if !goals.is_empty() {
        col = col.child(div().pt(px(18.)).pb(px(4.)).px(px(10.)).text_size(px(11.)).font_weight(FontWeight::BOLD).text_color(t.muted).child("ACTIVE GOALS"));
        let mut list = div().id("sidebar-goals").flex().flex_col().gap(px(1.)).flex_1().min_h_0().overflow_y_scroll();
        let open_goal = match &page {
            Page::Goal(r) => Some(r.clone()),
            _ => None,
        };
        for g in &goals {
            let r = fmt::ref_of(g, "G");
            let sel = open_goal.as_deref() == Some(r.as_str());
            let text = t.text;
            let open = r.clone();
            let stale = stale(g);
            let updated = fmt::opt_s(g, "last_update").map(|at| {
                div()
                    .id(SharedString::from(format!("side-goal-at-{r}")))
                    .flex_none()
                    .text_color(if stale { t.warn_fg } else { t.muted })
                    .when(stale, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .tooltip(kit::tip(format!("Last update {}", fmt::full_time(at))))
                    .child(fmt::ago(at))
            });
            list = list.child(
                div()
                    .id(SharedString::from(format!("side-goal-{r}")))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(10.))
                    .pt(px(7.))
                    .pb(px(8.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .when(sel, |d| d.bg(t.card))
                    .hover(move |s| s.bg(hsla(0., 0., 0.5, 0.08)))
                    .on_click(cx.listener(move |m, _, _, cx| crate::ui::goal::open(m, &open, false, cx)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().flex_none().text_size(px(12.)).font_family(t.mono_font.clone()).font_weight(FontWeight::SEMIBOLD).text_color(t.goal).child(r.clone()))
                            .child(div().flex_1().min_w_0().text_size(px(13.)).text_color(text).truncate().child(s(g, "name").to_string()))
                            .when(i(g, "needs") > 0, |d| d.child(kit::dot(t.warn, 7.))),
                    )
                    .child(status::bar(t, &Tally::of(g, None).parts(), 4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(11.))
                            .child(div().flex_1().min_w_0().text_color(t.muted).truncate().child(goal_sub(g)))
                            .children(updated),
                    ),
            );
        }
        col = col.child(list);
    } else {
        col = col.child(div().flex_1());
    }
    col.child(footer(m, t, cx)).into_any_element()
}

fn search_box(t: &Theme, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let line = t.border_2;
    div()
        .id("sidebar-search")
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .h(px(34.))
        .px(px(10.))
        .rounded(px(8.))
        .bg(t.card)
        .border_1()
        .border_color(t.border)
        .text_size(px(13.))
        .text_color(t.muted)
        .cursor_text()
        .hover(move |s| s.border_color(line))
        .on_click(cx.listener(|m, _, window, cx| crate::ui::search::open(m, window, cx)))
        .child(kit::glyph(kit::Glyph::Search, 13., t.muted))
        .child(div().flex_1().min_w_0().truncate().child("Search tasks, goals, T14…"))
        .child(div().font_family(t.mono_font.clone()).text_size(px(11.)).child("⌘K"))
}

/// The board's health: Midna, work hours and usage, then whatever needs a look (hooks, accounts,
/// an update, the PR feed), and Settings.
fn footer(m: &mut MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Div {
    let st = m.state_rc();
    let mut wrap = div().flex().flex_wrap().gap(px(6.)).pt(px(10.)).border_t_1().border_color(t.border);
    if let Some(mid) = st.get("midna").filter(|v| v.is_object()) {
        let p = crate::app::conn_pill(mid);
        let (fg, bg, dot) = if p.cls == "up" { (t.up_fg, t.up_soft, t.up) } else { (t.warn_fg, t.warn_soft, t.warn) };
        let label = if p.cls == "up" { "Midna".to_string() } else { p.label };
        wrap = wrap.child(crate::app::pill_shell(t, "conn-pill", fg, bg).child(kit::dot(dot, 8.)).child(label).tooltip(kit::tip(p.title)));
    }
    wrap = wrap.child(crate::ui::hours::pill(m, t, cx));
    if let Some(u) = st.get("usage").and_then(crate::app::usage_view) {
        wrap = wrap.child(
            div().id("usage-pill-go").cursor_pointer().child(crate::app::usage_pill(t, &u)).on_click(cx.listener(|m, _, _, cx| m.go(crate::app::Page::Days, cx))),
        );
    }
    wrap = wrap.children(crate::ui::prwatch::pills(&st, t));
    if let Some(h) = crate::app::hooks_item(m, t, cx) {
        wrap = wrap.child(h);
    }
    if m.backend.label() == "fake" {
        wrap = wrap.child(kit::tone_pill(t, "goal", "Sample board"));
    }
    if let Some(p) = crate::app::accounts_pill(&st, t) {
        wrap = wrap.child(p);
    }
    if let Some(p) = crate::app::update_pill(t) {
        wrap = wrap.child(p);
    }
    let text = t.text;
    wrap.child(
        div()
            .id("open-settings")
            .flex()
            .items_center()
            .h(px(25.))
            .px(px(4.))
            .text_size(px(12.5))
            .text_color(t.muted)
            .cursor_pointer()
            .hover(move |s| s.text_color(text))
            .tooltip(kit::tip("Settings (⌘,)"))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::OpenSettings), cx))
            .child("Settings"),
    )
}

#[cfg(test)]
mod tests {
    use super::{active_goals, stale};
    use crate::fmt::i;
    use serde_json::{Value, json};

    fn ago(mins: i64) -> String {
        (crate::fmt::now() - chrono::Duration::minutes(mins)).to_rfc3339()
    }

    #[test]
    fn every_goal_in_progress_is_listed_running_or_not() {
        let goals = vec![
            json!({"id": 1, "done": 2, "total": 6, "active": 1, "needs": 0}),
            json!({"id": 2, "done": 0, "total": 2, "active": 0, "needs": 0}),
            json!({"id": 3, "done": 0, "total": 4, "active": 1, "needs": 1}),
            json!({"id": 4, "done": 4, "total": 9, "active": 2, "needs": 0}),
            json!({"id": 5, "done": 1, "total": 3, "active": 1, "needs": 0, "paused": true}),
            json!({"id": 6, "done": 3, "total": 8, "active": 0, "needs": 0, "prs_open": [12]}),
        ];
        let ids: Vec<i64> = active_goals(&goals).iter().map(|g| i(g, "id")).collect();
        assert_eq!(ids, vec![6, 4, 3, 1]);
    }

    #[test]
    fn a_running_goal_goes_stale_sooner_than_one_in_review() {
        let goal = |active: i64, mins: i64| -> Value { json!({"active": active, "needs": 0, "last_update": ago(mins)}) };
        assert!(!stale(&goal(1, 20)));
        assert!(stale(&goal(1, 45)));
        assert!(!stale(&goal(0, 45)));
        assert!(stale(&goal(0, 180)));
    }

    #[test]
    fn a_goal_waiting_on_you_or_with_no_update_is_never_stale() {
        assert!(!stale(&json!({"active": 1, "needs": 1, "last_update": ago(600)})));
        assert!(!stale(&json!({"active": 1, "needs": 0})));
    }
}
