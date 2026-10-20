//! The Releases page (`GET /releases` per project): Unreleased first, then each release newest
//! first, each a row that opens in place to its tasks by goal (`GET /releases/:id`). Only the
//! projects with releases on (the state's `releases`) have one; with none the page is hidden.
//! Read-only: cutting and editing releases stay in `tb`.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, i, opt_s, s};
use crate::theme::Theme;
use crate::ui::issue_panel as ip;
use crate::ui::{goal, kit};
use chrono::{DateTime, Datelike, Utc};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

const PAGE_X: f32 = 28.;
const PROJECT_FILTER: &str = "releases-project";
/// The left edge of goal headers and task rows inside an open release (under its name).
const INSET: f32 = 60.;

#[derive(Default)]
pub struct State {
    /// The rows open in place, by [`Row::key`].
    pub open: HashSet<String>,
    /// "all" or one project.
    pub project: String,
    /// `GET /releases?project=` by project.
    pub lists: HashMap<String, Value>,
    /// `GET /releases/:id` (or `/releases/unreleased`) for each open row, by key.
    pub details: HashMap<String, Value>,
}

impl State {
    pub fn project(&self) -> &str {
        if self.project.is_empty() { "all" } else { &self.project }
    }
}

/// The projects with releases on, from `GET /state`.
pub fn projects_on(state: &Value) -> Vec<String> {
    arr(state, "releases").iter().filter_map(|p| p.as_str()).map(str::to_string).collect()
}

/// The projects the page shows: the picked one while it still has releases, else every one.
pub fn shown(state: &Value, picked: &str) -> Vec<String> {
    let on = projects_on(state);
    if on.iter().any(|p| p == picked) { vec![picked.to_string()] } else { on }
}

/// The key of a project's Unreleased row.
pub fn unreleased_key(project: &str) -> String {
    format!("unreleased:{project}")
}

/// The project filter that shows release `r`: the picked one when its list has it, else every one.
pub fn project_showing(st: &State, r: &str) -> String {
    let has = |l: &Value| arr(l, "releases").iter().any(|x| s(x, "ref") == r);
    if st.lists.get(st.project()).is_some_and(has) { st.project.clone() } else { String::new() }
}

/// A `taskboard://release/R3` link (Midna links R refs): the Releases page with that row open.
pub fn open(m: &mut MainWindow, r: &str, cx: &mut Context<MainWindow>) {
    m.releases.project = project_showing(&m.releases, r);
    m.releases.open.insert(r.to_string());
    m.go(Page::Releases, cx);
    m.refresh(cx);
}

/// What to fetch for an open row: the path and query.
pub fn detail_call(key: &str) -> (String, Vec<(&'static str, String)>) {
    match key.strip_prefix("unreleased:") {
        Some(p) => ("releases/unreleased".into(), vec![("project", p.to_string())]),
        None => (format!("releases/{key}"), vec![]),
    }
}

/// The open rows in the projects shown: the ones the page fetches in full.
pub fn open_rows(st: &State, shown: &[String]) -> Vec<String> {
    let mut v: Vec<String> = st
        .open
        .iter()
        .filter(|k| match k.strip_prefix("unreleased:") {
            Some(p) => shown.iter().any(|s| s == p),
            None => shown.iter().any(|p| st.lists.get(p).is_some_and(|l| arr(l, "releases").iter().any(|r| s(r, "ref") == k.as_str()))),
        })
        .cloned()
        .collect();
    v.sort();
    v
}

// ================================================================== view models

/// One row: Unreleased or a release.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: String,
    pub name: String,
    /// When it was cut; empty for Unreleased.
    pub when: String,
    pub count: String,
    /// Marked failed (`tb release fail`): its work went back to Unreleased.
    pub failed: bool,
}

/// A project's rows: Unreleased first, then its releases newest first.
pub fn rows(project: &str, list: &Value, now: DateTime<Utc>) -> Vec<Row> {
    let mut v = vec![Row { key: unreleased_key(project), name: "Unreleased".into(), when: String::new(), count: fmt::plural(i(list, "unreleased"), "task", "tasks"), failed: false }];
    let mut rels: Vec<&Value> = arr(list, "releases").iter().collect();
    rels.sort_by_key(|r| std::cmp::Reverse(i(r, "id")));
    v.extend(rels.into_iter().map(|r| Row {
        key: s(r, "ref").to_string(),
        name: s(r, "name").to_string(),
        when: fmt::parse(s(r, "cut_at")).map(|t| cut_at(t, now)).unwrap_or_default(),
        count: fmt::plural(i(r, "tasks"), "task", "tasks"),
        failed: opt_s(r, "failed_at").is_some(),
    }));
    v
}

/// "Today, 3:05 PM", "Oct 7, 3:05 PM", or "Oct 7, 2025, 3:05 PM" in another year.
pub(crate) fn cut_at(t: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let (l, today) = (fmt::local(t), fmt::local(now).date_naive());
    let clock = l.format("%-I:%M %p");
    match l.date_naive() {
        d if d == today => format!("Today, {clock}"),
        d if d.year() == today.year() => format!("{}, {clock}", l.format("%b %-d")),
        _ => format!("{}, {clock}", l.format("%b %-d, %Y")),
    }
}

/// "Oct 7" for a task's finish.
fn finished(c: &Value) -> String {
    fmt::parse(s(c, "finished_at")).map(|t| fmt::local(t).format("%b %-d").to_string()).unwrap_or_default()
}

/// A goal group's heading: its name, or "No goal" for tasks without one.
fn group_title(g: &Value) -> String {
    match opt_s(g, "name").filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None if g["goal"].is_null() => "No goal".into(),
        None => "Untitled goal".into(),
    }
}

/// (project, its Unreleased count) for the filter.
fn tabs(st: &State, on: &[String]) -> Vec<(String, usize)> {
    on.iter().map(|p| (p.clone(), st.lists.get(p).map(|l| i(l, "unreleased").max(0) as usize).unwrap_or(0))).collect()
}

// ================================================================== render

fn row_head(t: &Theme, row: &Row, open: bool, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let key = row.key.clone();
    div()
        .id(SharedString::from(format!("releases-row-{}", row.key)))
        .flex()
        .items_center()
        .gap(px(16.))
        .px(px(20.))
        .py(px(14.))
        .cursor_pointer()
        .child(div().flex_none().w(px(20.)).text_size(px(12.)).text_color(t.muted).child(if open { "▾" } else { "▸" }))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().min_w_0().text_size(px(16.)).font_weight(FontWeight::BOLD).truncate().child(row.name.clone()))
                .when(row.failed, |d| d.child(kit::tone_pill(t, "failed", "Failed"))),
        )
        .when(!row.when.is_empty(), |d| d.child(div().flex_none().text_size(px(13.)).text_color(t.muted).child(row.when.clone())))
        .child(div().flex_none().w(px(80.)).text_right().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_2).child(row.count.clone()))
        .on_click(cx.listener(move |m, _, _, cx| {
            if !m.releases.open.remove(&key) {
                m.releases.open.insert(key.clone());
            }
            m.refresh(cx);
        }))
}

fn group_head(t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Div {
    let r = opt_s(g, "goal").map(str::to_string);
    let link = t.goal;
    div()
        .flex()
        .items_baseline()
        .gap(px(8.))
        .pl(px(INSET))
        .pr(px(20.))
        .pt(px(8.))
        .pb(px(6.))
        .border_t_1()
        .border_color(t.divider)
        .bg(t.panel_2)
        .text_size(px(12.))
        .children(r.map(|r| {
            let r2 = r.clone();
            div()
                .id(SharedString::from(format!("releases-goal-{r}")))
                .font_family(t.mono_font.clone())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(link)
                .cursor_pointer()
                .hover(|s| s.underline())
                .child(r)
                .on_click(cx.listener(move |m, _, _, cx| goal::open(m, &r2, false, cx)))
        }))
        .child(div().font_weight(FontWeight::BOLD).child(group_title(g)))
}

fn task_row(t: &Theme, c: &Value, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let r = s(c, "ref").to_string();
    let o = r.clone();
    let hover = t.panel_2;
    let link = t.accent;
    let pr = match (c["pr_num"].as_i64(), opt_s(c, "pr_url").filter(|u| !u.is_empty())) {
        (Some(n), Some(u)) => Some((format!("#{n}"), u.to_string())),
        (None, Some(u)) => Some(("PR".to_string(), u.to_string())),
        _ => None,
    };
    div()
        .id(SharedString::from(format!("releases-task-{r}")))
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(34.))
        .pl(px(INSET))
        .pr(px(20.))
        .border_t_1()
        .border_color(t.divider)
        .text_size(px(13.))
        .hover(move |s| s.bg(hover))
        .child(div().flex_none().w(px(44.)).text_size(px(12.)).font_family(t.mono_font.clone()).text_color(t.muted).child(r.clone()))
        .child(
            div().min_w_0().flex_1().flex().child(
                div()
                    .id(SharedString::from(format!("releases-task-title-{r}")))
                    .min_w_0()
                    .truncate()
                    .cursor_pointer()
                    .hover(move |s| s.text_color(link).underline())
                    .child(s(c, "title").to_string())
                    .on_click(cx.listener(move |m, _, _, cx| m.open_task(o.clone(), cx))),
            ),
        )
        .child(div().flex_none().w(px(64.)).flex().justify_end().children(pr.map(|(label, url)| {
            kit::link(t, SharedString::from(format!("releases-pr-{r}")), label).text_size(px(12.)).on_click(move |_, _, cx| cx.open_url(&url))
        })))
        .child(div().flex_none().w(px(64.)).whitespace_nowrap().text_size(px(12.)).text_color(t.muted).text_right().child(finished(c)))
}

fn body(t: &Theme, d: Option<&Value>, cx: &mut Context<MainWindow>) -> Div {
    let mut body = div().flex().flex_col();
    let note = |text: String| div().pl(px(INSET)).pr(px(20.)).py(px(10.)).border_t_1().border_color(t.divider).text_size(px(12.)).text_color(t.muted).child(text);
    let Some(d) = d.filter(|d| d["groups"].is_array()) else {
        return body.child(note("Loading…".into()));
    };
    if let Some(n) = opt_s(d, "notes").map(str::trim).filter(|n| !n.is_empty()) {
        body = body.child(div().pl(px(INSET)).pr(px(20.)).py(px(10.)).border_t_1().border_color(t.divider).text_size(px(13.)).text_color(t.text_2).child(n.to_string()));
    }
    let groups = arr(d, "groups");
    if groups.is_empty() {
        return body.child(note("No tasks".into()));
    }
    for g in groups {
        body = body.child(group_head(t, g, cx));
        for c in arr(g, "tasks") {
            body = body.child(task_row(t, c, cx));
        }
    }
    body
}

fn card(t: &Theme, row: &Row, d: Option<&Value>, open: bool, cx: &mut Context<MainWindow>) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(12.))
        .border_1()
        .border_color(if open { t.accent_line } else { t.border })
        .bg(t.card)
        .overflow_hidden()
        .child(row_head(t, row, open, cx))
        .when(open, |c| c.child(body(t, d, cx)))
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let now = fmt::now();
    let on = projects_on(m.state());
    let projects = shown(m.state(), m.releases.project());
    let picked = if projects.len() == 1 && on.len() > 1 { projects[0].clone() } else { "all".to_string() };
    let tabs = tabs(&m.releases, &on);
    let head = div()
        .flex()
        .items_center()
        .gap(px(16.))
        .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Releases"))
        .when(on.len() > 1, |d| {
            d.child(ip::project_filter(&t, PROJECT_FILTER, &picked, &tabs, cx, |m, p, cx| {
                m.releases.project = p;
                m.refresh(cx);
            }))
        });
    let mut page = div().flex().flex_col().gap(px(20.)).px(px(PAGE_X)).pt(px(22.)).pb(px(32.)).line_height(relative(1.4)).child(head);
    let filter = ip::render_project_filter(m, &t, PROJECT_FILTER, window, cx);
    let shell = div().id("releases-page").flex_1().min_w_0().h_full().overflow_y_scroll().restrict_scroll_to_axis().children(filter);

    if projects.is_empty() {
        let msg = if m.down.is_some() { "Can’t load the board." } else { "Loading…" };
        return shell.child(page.child(kit::empty(&t, msg))).into_any_element();
    }
    for p in &projects {
        let mut list = div().flex().flex_col().gap(px(8.));
        if projects.len() > 1 {
            list = list.child(div().mb(px(2.)).text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(t.muted).child(p.to_uppercase()));
        }
        match m.releases.lists.get(p) {
            None => list = list.child(kit::empty(&t, "Loading…")),
            Some(l) => {
                for row in rows(p, l, now) {
                    let open = m.releases.open.contains(&row.key);
                    list = list.child(card(&t, &row, m.releases.details.get(&row.key), open, cx));
                }
            }
        }
        page = page.child(list);
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

    fn at(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
        fmt::local(Utc::now()).offset().with_ymd_and_hms(y, mo, d, h, 5, 0).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn unreleased_comes_first_then_releases_newest_first() {
        let now = at(2026, 10, 15, 16);
        let list = json!({"project": "web", "on": true, "unreleased": 1, "releases": [
            {"id": 1, "ref": "R1", "name": "v1.0", "cut_at": at(2025, 12, 1, 9).to_rfc3339(), "tasks": 4},
            {"id": 3, "ref": "R3", "name": "v1.2", "cut_at": at(2026, 10, 15, 15).to_rfc3339(), "tasks": 2},
            {"id": 2, "ref": "R2", "name": "v1.1", "cut_at": at(2026, 10, 7, 9).to_rfc3339(), "tasks": 1},
            {"id": 4, "ref": "R4", "name": "v1.3", "cut_at": at(2026, 10, 15, 15).to_rfc3339(), "tasks": 0, "failed_at": at(2026, 10, 15, 16).to_rfc3339()},
        ]});
        let all = rows("web", &list, now);
        assert_eq!(all.iter().map(|r| r.failed).collect::<Vec<_>>(), [false, true, false, false, false], "only R4 failed");
        let got: Vec<(String, String, String, String)> = all.into_iter().filter(|r| !r.failed).map(|r| (r.key, r.name, r.when, r.count)).collect();
        let row = |k: &str, n: &str, w: &str, c: &str| (k.to_string(), n.to_string(), w.to_string(), c.to_string());
        assert_eq!(
            got,
            [
                row("unreleased:web", "Unreleased", "", "1 task"),
                row("R3", "v1.2", "Today, 3:05 PM", "2 tasks"),
                row("R2", "v1.1", "Oct 7, 9:05 AM", "1 task"),
                row("R1", "v1.0", "Dec 1, 2025, 9:05 AM", "4 tasks"),
            ]
        );
    }

    #[test]
    fn only_projects_with_releases_on_are_shown() {
        let state = json!({"releases": ["kass", "midna"]});
        assert_eq!(projects_on(&json!({})), Vec::<String>::new());
        assert_eq!(shown(&state, "all"), ["kass", "midna"]);
        assert_eq!(shown(&state, "midna"), ["midna"]);
        // A project whose releases were turned off falls back to every one still on.
        assert_eq!(shown(&state, "web"), ["kass", "midna"]);
    }

    #[test]
    fn a_release_link_keeps_the_filter_only_when_it_shows_the_release() {
        let mut st = State::default();
        st.lists.insert("web".into(), json!({"releases": [{"ref": "R2"}]}));
        st.lists.insert("api".into(), json!({"releases": [{"ref": "R5"}]}));
        st.project = "web".into();
        assert_eq!(project_showing(&st, "R2"), "web");
        assert_eq!(project_showing(&st, "R5"), "");
        st.project = String::new();
        assert_eq!(project_showing(&st, "R2"), "");
    }

    #[test]
    fn open_rows_fetch_the_release_or_unreleased() {
        let mut st = State::default();
        st.lists.insert("web".into(), json!({"releases": [{"ref": "R2"}]}));
        st.open.extend(["R2".to_string(), "R9".to_string(), unreleased_key("web"), unreleased_key("api")]);
        let shown = vec!["web".to_string()];
        assert_eq!(open_rows(&st, &shown), ["R2", "unreleased:web"]);
        assert_eq!(detail_call("R2"), ("releases/R2".to_string(), vec![]));
        assert_eq!(detail_call("unreleased:web"), ("releases/unreleased".to_string(), vec![("project", "web".to_string())]));
    }

    #[test]
    fn groups_without_a_goal_say_so() {
        assert_eq!(group_title(&json!({"goal": "G2", "name": "Billing"})), "Billing");
        assert_eq!(group_title(&json!({"goal": null, "name": null})), "No goal");
        assert_eq!(group_title(&json!({"goal": "G4", "name": ""})), "Untitled goal");
    }
}
