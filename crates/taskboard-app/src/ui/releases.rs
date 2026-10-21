//! The Releases page (`GET /releases` per project): Unreleased first, then each release newest
//! first, each a row that opens in place to its tasks by goal (`GET /releases/:id`). A pending cut
//! (`tb release cut --after`) sits above them with the tasks and goals it waits for. Only the
//! projects with releases on (the state's `releases`) have one; with none the page is hidden.
//! The page is one list that draws only the items in view; each project's newest page of releases
//! comes with the refresh and older pages are fetched (`before=`) as the list nears their end.
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
use std::rc::Rc;

const PAGE_X: f32 = 28.;
/// How many releases one fetch asks for.
pub const PAGE: usize = 100;
/// Fetch a project's next page once the list is scrolled within this many items of its end.
const AHEAD: usize = 60;
const PROJECT_FILTER: &str = "releases-project";
/// The left edge of goal headers and task rows inside an open release (under its name).
const INSET: f32 = 60.;

#[derive(Default)]
pub struct State {
    /// The rows open in place, by [`Row::key`].
    pub open: HashSet<String>,
    /// "all" or one project.
    pub project: String,
    /// `GET /releases?project=` by project: the newest page with the older pages fetched under it.
    pub lists: HashMap<String, Value>,
    /// `GET /releases/:id` (or `/releases/unreleased`) for each open row, by key.
    pub details: HashMap<String, Value>,
    /// The row a link opened, scrolled to once it and the open rows above it have loaded.
    pub focus: Option<String>,
    /// The projects whose next older page is on its way.
    loading: HashSet<String>,
    /// The page's list, which draws only the items in view.
    list: Option<ListState>,
    /// The items the list was last told about.
    items: Rc<Vec<Item>>,
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
    let project = project_showing(&m.releases, r);
    open_in(m, project, r, cx);
}

/// The Releases page filtered to `project`, scrolled to row `r` and open.
pub fn open_in(m: &mut MainWindow, project: String, r: &str, cx: &mut Context<MainWindow>) {
    m.releases.project = project;
    m.releases.open.insert(r.to_string());
    m.releases.focus = Some(r.to_string());
    m.go(Page::Releases, cx);
    m.refresh(cx);
}

/// Whether row `f` is in a shown project's list (as far as it has been fetched).
fn listed(st: &State, shown: &[String], f: &str) -> bool {
    match f.strip_prefix("unreleased:") {
        Some(p) => shown.iter().any(|s| s == p && st.lists.contains_key(p)),
        None => shown.iter().any(|p| st.lists.get(p).is_some_and(|l| arr(l, "releases").iter().any(|r| s(r, "ref") == f))),
    }
}

/// Whether to scroll to the focused row now: its list has it and every open row has loaded, so
/// nothing above it will grow. `None` drops a focus no shown list has once they've all loaded, older
/// pages included.
fn focus_ready(st: &State, shown: &[String]) -> Option<bool> {
    let f = st.focus.as_deref()?;
    if !listed(st, shown, f) {
        return if shown.iter().all(|p| st.lists.get(p).is_some_and(|l| l["more"] != true)) { None } else { Some(false) };
    }
    Some(open_rows(st, shown).iter().all(|k| st.details.contains_key(k)))
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

/// A newest page of `GET /releases` over the list held so far: the older pages already fetched stay
/// under it while they still join on, so a refresh doesn't drop what was scrolled to.
pub fn merge_page(held: Option<&Value>, mut page: Value) -> Value {
    let ids = |l: &Value| arr(l, "releases").iter().map(|r| i(r, "id")).collect::<Vec<_>>();
    let (Some(held), Some(&min)) = (held, ids(&page).iter().min()) else { return page };
    let joins = ids(held).iter().any(|&id| id >= min);
    if page["more"] != true || !joins {
        return page;
    }
    let older: Vec<Value> = arr(held, "releases").iter().filter(|r| i(r, "id") < min).cloned().collect();
    if !older.is_empty() {
        page["more"] = held["more"].clone();
        if let Some(rs) = page["releases"].as_array_mut() {
            rs.extend(older);
        }
    }
    page
}

/// An older page (`before=`) added under the list held.
pub fn add_older(list: &mut Value, page: &Value) {
    let min = arr(list, "releases").iter().map(|r| i(r, "id")).min().unwrap_or(i64::MAX);
    let older: Vec<Value> = arr(page, "releases").iter().filter(|r| i(r, "id") < min).cloned().collect();
    if let Some(rs) = list["releases"].as_array_mut() {
        rs.extend(older);
    }
    list["more"] = page["more"].clone();
}

/// Fetch the next older page of each shown project whose end is near the place scrolled to, or of
/// every one while a linked release isn't listed yet.
fn load_older(m: &mut MainWindow, shown: &[String], cx: &mut Context<MainWindow>) {
    let Some(l) = &m.releases.list else { return };
    let near = l.logical_scroll_top().item_ix + AHEAD;
    let st = &m.releases;
    let hunting = st.focus.as_deref().is_some_and(|f| !listed(st, shown, f));
    for p in shown {
        let Some(held) = st.lists.get(p).filter(|l| l["more"] == true) else { continue };
        let end = st.items.iter().position(|it| it.kind == Kind::More(p.clone()));
        if st.loading.contains(p) || (!hunting && end.is_none_or(|e| e > near)) {
            continue;
        }
        let Some(last) = arr(held, "releases").iter().min_by_key(|r| i(r, "id")).map(|r| s(r, "ref").to_string()) else { continue };
        m.releases.loading.insert(p.clone());
        let query = vec![("project", p.clone()), ("before", last), ("limit", PAGE.to_string())];
        let (p, backend) = (p.clone(), m.backend.clone());
        cx.spawn(async move |this, cx| {
            let got = cx.background_executor().spawn(async move { backend.get("releases", &query) }).await;
            let _ = this.update(cx, |m, cx| {
                m.releases.loading.remove(&p);
                if let Some(held) = m.releases.lists.get_mut(&p) {
                    match got {
                        Ok(v) => add_older(held, &v),
                        // Stop asking; the releases already here stay.
                        Err(_) => held["more"] = false.into(),
                    }
                }
                cx.notify();
            });
        })
        .detach();
        return;
    }
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

/// The project's pending cut: its name, each task or goal it waits for (and whether that's through),
/// and whether the board is cutting it now.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub name: String,
    pub after: Vec<(String, bool)>,
    pub running: bool,
}

pub fn pending(list: &Value) -> Option<Pending> {
    let p = list.get("pending").filter(|p| p.is_object())?;
    Some(Pending {
        name: s(p, "name").to_string(),
        after: arr(p, "after").iter().map(|a| (s(a, "ref").to_string(), s(a, "state") == "met")).collect(),
        running: p["running"] == true,
    })
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

/// What an open row shows under its head: its notes, then each goal group's heading and tasks.
fn body_items(d: Option<&Value>) -> Vec<Kind> {
    let Some(d) = d.filter(|d| d["groups"].is_array()) else { return vec![Kind::Note("Loading…")] };
    let mut v = Vec::new();
    if let Some(n) = opt_s(d, "notes").map(str::trim).filter(|n| !n.is_empty()) {
        v.push(Kind::Notes(n.to_string()));
    }
    let groups = arr(d, "groups");
    if groups.is_empty() {
        v.push(Kind::Note("No tasks"));
    }
    for g in groups {
        v.push(Kind::Group(serde_json::json!({"goal": g["goal"], "name": g["name"]})));
        v.extend(arr(g, "tasks").iter().cloned().map(Kind::Task));
    }
    v
}

fn pending_card(t: &Theme, project: &str, p: &Pending) -> Stateful<Div> {
    let (tone, label) = if p.running { ("working", "Cutting") } else { ("planned", "Pending") };
    div()
        .id(SharedString::from(format!("releases-pending-{project}")))
        .flex()
        .items_center()
        .gap(px(16.))
        .px(px(20.))
        .py(px(14.))
        .rounded(px(12.))
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .child(div().flex_none().w(px(20.)))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().min_w_0().text_size(px(16.)).font_weight(FontWeight::BOLD).truncate().child(p.name.clone()))
                .child(kit::tone_pill(t, tone, label)),
        )
        .child(div().flex_none().flex().items_center().gap(px(8.)).text_size(px(12.)).font_family(t.mono_font.clone()).children(p.after.iter().map(|(r, met)| {
            let text = if *met { format!("✓ {r}") } else { r.clone() };
            div().text_color(if *met { t.up_fg } else { t.text_2 }).child(text)
        })))
}

/// One item of the page's list.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    kind: Kind,
    /// The space above it.
    pt: f32,
    /// Its place in a card, which is drawn in pieces: (first, last, open).
    card: Option<(bool, bool, bool)>,
}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    /// The title and (with several projects on) the project filter.
    Head { picked: String, tabs: Vec<(String, usize)>, filter: bool },
    Empty(&'static str),
    /// A project's name, with several shown.
    Label(String),
    Pending(String, Pending),
    Row(Row),
    Notes(String),
    Note(&'static str),
    Group(Value),
    Task(Value),
    /// The end of a project's releases while older ones are left to fetch.
    More(String),
    End,
}

/// The page as list items: the head, then per project its pending cut and its rows, an open row
/// followed by what it shows.
fn items(st: &State, projects: &[String], head: Kind, down: bool, now: DateTime<Utc>) -> Vec<Item> {
    let mut v = vec![Item { kind: head, pt: 22., card: None }];
    if projects.is_empty() {
        v.push(Item { kind: Kind::Empty(if down { "Can’t load the board." } else { "Loading…" }), pt: 20., card: None });
    }
    for p in projects {
        // The page's 20px gap above each project, its cards' 8px between them.
        let mut top = 20.;
        let mut add = |v: &mut Vec<Item>, kind: Kind, card| {
            v.push(Item { kind, pt: top, card });
            top = 8.;
        };
        if projects.len() > 1 {
            add(&mut v, Kind::Label(p.to_uppercase()), None);
        }
        let Some(l) = st.lists.get(p) else {
            add(&mut v, Kind::Empty("Loading…"), None);
            continue;
        };
        if let Some(pc) = pending(l) {
            add(&mut v, Kind::Pending(p.clone(), pc), None);
        }
        for row in rows(p, l, now) {
            let open = st.open.contains(&row.key);
            let body = if open { body_items(st.details.get(&row.key)) } else { vec![] };
            let n = body.len();
            add(&mut v, Kind::Row(row), Some((true, n == 0, open)));
            v.extend(body.into_iter().enumerate().map(|(k, kind)| Item { kind, pt: 0., card: Some((false, k + 1 == n, open)) }));
        }
        if l["more"] == true {
            add(&mut v, Kind::More(p.clone()), None);
        }
    }
    v.push(Item { kind: Kind::End, pt: 0., card: None });
    v
}

/// Keep the list's items in step with the page: only the run that changed is spliced, so the place
/// scrolled to stays put as rows open and older pages come in under it.
fn sync_list(st: &mut State, new: Vec<Item>) {
    let n = new.len();
    let Some(l) = &st.list else {
        st.list = Some(ListState::new(n, ListAlignment::Top, px(800.)));
        st.items = Rc::new(new);
        return;
    };
    let old = &st.items;
    let pre = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    if pre == n && n == old.len() {
        return;
    }
    let suf = old.iter().rev().zip(new.iter().rev()).take(old.len().min(n) - pre).take_while(|(a, b)| a == b).count();
    l.splice(pre..old.len() - suf, n - pre - suf);
    st.items = Rc::new(new);
}

fn list_item(m: &mut MainWindow, ix: usize, _: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let Some(it) = m.releases.items.get(ix).cloned() else { return div().into_any_element() };
    let note = |text: &str, size: f32, color| div().pl(px(INSET)).pr(px(20.)).py(px(10.)).border_t_1().border_color(t.divider).text_size(px(size)).text_color(color).child(text.to_string());
    let inner = match &it.kind {
        Kind::Head { picked, tabs, filter } => div()
            .flex()
            .items_center()
            .gap(px(16.))
            .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Releases"))
            .when(*filter, |d| {
                d.child(ip::project_filter(&t, PROJECT_FILTER, picked, tabs, cx, |m, p, cx| {
                    m.releases.project = p;
                    m.refresh(cx);
                }))
            })
            .into_any_element(),
        Kind::Empty(msg) => kit::empty(&t, *msg).into_any_element(),
        Kind::More(_) => kit::empty(&t, "Loading…").into_any_element(),
        Kind::Label(p) => div().mb(px(2.)).text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(t.muted).child(p.clone()).into_any_element(),
        Kind::Pending(p, pc) => pending_card(&t, p, pc).into_any_element(),
        Kind::Row(row) => row_head(&t, row, it.card.is_some_and(|c| c.2), cx).into_any_element(),
        Kind::Notes(n) => note(n, 13., t.text_2).into_any_element(),
        Kind::Note(text) => note(text, 12., t.muted).into_any_element(),
        Kind::Group(g) => group_head(&t, g, cx).into_any_element(),
        Kind::Task(c) => task_row(&t, c, cx).into_any_element(),
        Kind::End => return div().h(px(32.)).into_any_element(),
    };
    let el = div().px(px(PAGE_X)).pt(px(it.pt)).line_height(relative(1.4));
    let Some((first, last, open)) = it.card else { return el.child(inner).into_any_element() };
    // `card`: rounded, bordered and clipped as one box across its items.
    let card = div()
        .flex()
        .flex_col()
        .border_x_1()
        .border_color(if open { t.accent_line } else { t.border })
        .bg(t.card)
        .overflow_hidden()
        .when(first, |c| c.border_t_1().rounded_t(px(12.)))
        .when(last, |c| c.border_b_1().rounded_b(px(12.)));
    el.child(card.child(inner)).into_any_element()
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let on = projects_on(m.state());
    let projects = shown(m.state(), m.releases.project());
    let picked = if projects.len() == 1 && on.len() > 1 { projects[0].clone() } else { "all".to_string() };
    let head = Kind::Head { picked, tabs: tabs(&m.releases, &on), filter: on.len() > 1 };
    let focus = match focus_ready(&m.releases, &projects) {
        Some(true) => m.releases.focus.take(),
        None => {
            m.releases.focus = None;
            None
        }
        Some(false) => None,
    };
    let items = items(&m.releases, &projects, head, m.down.is_some(), fmt::now());
    sync_list(&mut m.releases, items);
    let state = m.releases.list.clone().expect("synced");
    if let Some(f) = focus {
        if let Some(ix) = m.releases.items.iter().position(|it| matches!(&it.kind, Kind::Row(r) if r.key == f)) {
            state.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(m.releases.items[ix].pt) });
        }
    }
    load_older(m, &projects, cx);
    let filter = ip::render_project_filter(m, &t, PROJECT_FILTER, window, cx);
    div()
        .id("releases-page")
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .flex_col()
        .child(list(state, cx.processor(list_item)).flex_1().min_h_0())
        .children(filter)
        .into_any_element()
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
    fn a_pending_cut_shows_what_it_waits_for() {
        assert_eq!(pending(&json!({"on": true, "releases": [], "pending": null})), None);
        let list = json!({"pending": {"name": "v1.5", "running": false, "after": [
            {"ref": "T627", "state": "met"}, {"ref": "G12", "state": "waiting"}]}});
        assert_eq!(
            pending(&list),
            Some(Pending { name: "v1.5".into(), after: vec![("T627".into(), true), ("G12".into(), false)], running: false })
        );
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
    fn a_linked_row_scrolls_once_it_and_the_open_rows_have_loaded() {
        let mut st = State::default();
        let shown = vec!["web".to_string()];
        assert_eq!(focus_ready(&st, &shown), None, "nothing to scroll to");
        st.focus = Some("R2".into());
        st.open.extend(["R2".to_string(), unreleased_key("web")]);
        assert_eq!(focus_ready(&st, &shown), Some(false), "the list is loading");
        st.lists.insert("web".into(), json!({"releases": [{"ref": "R2"}]}));
        st.details.insert("R2".into(), json!({"groups": []}));
        assert_eq!(focus_ready(&st, &shown), Some(false), "Unreleased above it is loading");
        st.details.insert(unreleased_key("web"), json!({"groups": []}));
        assert_eq!(focus_ready(&st, &shown), Some(true));
        st.focus = Some("R9".into());
        assert_eq!(focus_ready(&st, &shown), None, "a release no list has is dropped");
        st.lists.insert("web".into(), json!({"releases": [{"ref": "R2"}], "more": true}));
        assert_eq!(focus_ready(&st, &shown), Some(false), "it may be on a page not fetched yet");
    }

    #[test]
    fn groups_without_a_goal_say_so() {
        assert_eq!(group_title(&json!({"goal": "G2", "name": "Billing"})), "Billing");
        assert_eq!(group_title(&json!({"goal": null, "name": null})), "No goal");
        assert_eq!(group_title(&json!({"goal": "G4", "name": ""})), "Untitled goal");
    }

    #[test]
    fn a_refresh_keeps_the_older_pages_under_the_newest() {
        let list = |ids: &[i64], more: bool| json!({"releases": ids.iter().map(|id| json!({"id": id, "ref": format!("R{id}")})).collect::<Vec<_>>(), "more": more});
        let ids = |l: &Value| arr(l, "releases").iter().map(|r| i(r, "id")).collect::<Vec<_>>();
        let held = list(&[9, 8, 7, 6, 5, 4], true);
        let got = merge_page(Some(&held), list(&[10, 9, 8], true));
        assert_eq!((ids(&got), got["more"] == true), (vec![10, 9, 8, 7, 6, 5, 4], true));
        assert_eq!(ids(&merge_page(Some(&held), list(&[10, 9], false))), [10, 9], "a whole list is the list");
        assert_eq!(ids(&merge_page(Some(&list(&[3, 2], false)), list(&[10, 9], true))), [10, 9], "older pages that don't join on are dropped");

        let mut l = list(&[10, 9], true);
        add_older(&mut l, &list(&[9, 8, 7], false));
        assert_eq!((ids(&l), l["more"] == true), (vec![10, 9, 8, 7], false));
    }

    #[gpui_kit::test]
    fn a_long_release_list_fetches_older_pages_as_it_scrolls(cx: &mut gpui_kit::TestAppContext) {
        use crate::backend::Backend as _;
        use crate::parity::{settle, window};
        let (w, rec) = window(cx);
        rec.board().post("projects/webapp", json!({"releases": true})).unwrap();
        for k in 0..340 {
            rec.board().app().db.insert("releases", vec![("project", json!("webapp")), ("name", json!(format!("v{k}"))), ("cut_at", json!("2026-10-15T10:00:00Z"))]).unwrap();
        }
        w.update(cx, |m: &mut MainWindow, _, cx| m.refresh(cx)).unwrap();
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| m.open_link("releases", cx)).unwrap();
        settle(cx);
        let draw = |cx: &mut gpui_kit::TestAppContext| {
            let _ = cx.update_window(w.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
            settle(cx);
        };
        draw(cx);
        let held = |cx: &mut gpui_kit::TestAppContext| w.update(cx, |m: &mut MainWindow, _, _| (arr(&m.releases.lists["webapp"], "releases").len(), m.releases.lists["webapp"]["more"] == true)).unwrap();
        assert_eq!(held(cx), (PAGE, true), "only the newest page until the list nears its end");
        assert_eq!(w.update(cx, |m: &mut MainWindow, _, _| m.releases.lists["webapp"]["total"].as_i64()).unwrap(), Some(340));

        for _ in 0..6 {
            w.update(cx, |m: &mut MainWindow, _, _| {
                let n = m.releases.items.len();
                m.releases.list.as_ref().unwrap().scroll_to(ListOffset { item_ix: n - 1, offset_in_item: px(0.) });
            })
            .unwrap();
            draw(cx);
        }
        assert_eq!(held(cx), (340, false), "scrolling to the end pages back through the rest");
        let rows: Vec<i64> = w
            .update(cx, |m: &mut MainWindow, _, _| {
                m.releases.items.iter().filter_map(|it| match &it.kind {
                    Kind::Row(r) => r.key.strip_prefix('R').and_then(|n| n.parse().ok()),
                    _ => None,
                })
                .collect()
            })
            .unwrap();
        assert_eq!(rows.len(), 340);
        assert!(rows.windows(2).all(|p| p[0] > p[1]), "newest first, each once");
        assert!(rec.posts().is_empty());
    }
}
