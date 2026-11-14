//! The Releases pages. The overview (`Page::Releases`) is a card per project with releases on (the
//! state's `releases`): its newest release, what waits in Unreleased (and its next cut), and the
//! few releases before. A project's page (`Page::ReleasesOf`) shows its counts, then the next
//! release (what it cuts after, then the work already finished), and every release by month, each
//! a row that opens in place to its tasks by goal (`GET /releases/:id`); the newest starts open.
//! That page is a virtual list, paged from the board (`GET /releases?limit=`) as it nears its end.
//! A failed answer says so with Try again and keeps what loaded before. With no project on, the
//! pages are hidden. Read-only: cutting and editing releases stay in `tb`.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, i, opt_s, s};
use crate::theme::Theme;
use crate::ui::{focus, goal, kit, sidebar};
use chrono::{DateTime, Datelike, Utc};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

const PAGE_X: f32 = 28.;
/// The project page's column.
const MAX_W: f32 = 920.;
/// An overview card's narrowest width.
const CARD_MIN: f32 = 340.;
const GAP: f32 = 16.;
/// The releases an overview card lists: the newest and the three before it.
pub const CARD_RELEASES: usize = 4;
/// How many releases the project page asks for at first, and how many more each time its list nears its end.
pub const PAGE: usize = 50;
/// Ask for more once the list is scrolled within this many items of its end.
const AHEAD: usize = 20;
/// How far a goal's tasks sit in from its heading.
const INDENT: f32 = 34.;

#[derive(Default)]
pub struct State {
    /// The older releases open in place on a project's page, by ref.
    pub open: HashSet<String>,
    /// The newest releases closed by hand (the newest starts open), by ref.
    pub shut: HashSet<String>,
    /// `GET /releases?project=` by project: an overview card's few, or the project page's pages.
    pub lists: HashMap<String, Value>,
    /// `GET /releases/:id` (or `/releases/unreleased`) by [`unreleased_key`] or ref.
    pub details: HashMap<String, Value>,
    /// Why the last ask for a project's list failed, by project, until one comes back.
    pub list_errs: HashMap<String, String>,
    /// Why the last ask for a release (or Unreleased) failed, by key, until one comes back.
    pub row_errs: HashMap<String, String>,
    /// How many releases the project page asks for (at least [`PAGE`]).
    pub limit: usize,
    /// A release a link opened: its project's page scrolls to it once it's listed.
    pub focus: Option<String>,
    pub scroll: ScrollHandle,
    list: Option<History>,
}

impl State {
    pub fn limit(&self) -> usize {
        self.limit.max(PAGE)
    }
}

/// The board's answers: each one that came back replaces what was there; a failed one is kept as an
/// error beside the older data, so the page says it can't load rather than Loading for ever.
pub fn land(st: &mut State, lists: Vec<(String, Result<Value, String>)>, rows: Vec<(String, Result<Value, String>)>) {
    for (p, v) in lists {
        match v {
            Ok(v) => {
                st.list_errs.remove(&p);
                st.lists.insert(p, v);
            }
            Err(e) => {
                st.list_errs.insert(p, e);
            }
        }
    }
    for (k, v) in rows {
        match v {
            Ok(v) => {
                st.row_errs.remove(&k);
                st.details.insert(k, v);
            }
            Err(e) => {
                st.row_errs.insert(k, e);
            }
        }
    }
}

/// The project page's list: what it was last told it holds, and how each item looked.
struct History {
    project: String,
    state: ListState,
    items: Vec<Item>,
    sigs: Vec<String>,
}

/// The projects with releases on, from `GET /state`.
pub fn projects_on(state: &Value) -> Vec<String> {
    arr(state, "releases").iter().filter_map(|p| p.as_str()).map(str::to_string).collect()
}

/// The project whose page is open, if any.
pub fn page_project(page: &Page) -> Option<&str> {
    match page {
        Page::ReleasesOf(p) => Some(p),
        _ => None,
    }
}

/// The projects a Releases page lists: every one on for the overview, else its own while it's on.
pub fn shown(state: &Value, page: &Page) -> Vec<String> {
    let on = projects_on(state);
    match page {
        Page::Releases => on,
        Page::ReleasesOf(p) if on.contains(p) => vec![p.clone()],
        _ => vec![],
    }
}

/// How many releases to ask for: an overview card's few, or the project page's limit.
pub fn list_limit(st: &State, page: &Page) -> usize {
    if matches!(page, Page::ReleasesOf(_)) { st.limit() } else { CARD_RELEASES }
}

/// The key of a project's Unreleased.
pub fn unreleased_key(project: &str) -> String {
    format!("unreleased:{project}")
}

fn lists_release(l: &Value, r: &str) -> bool {
    arr(l, "releases").iter().any(|x| s(x, "ref") == r)
}

/// Whether a release is open in place: the newest unless closed by hand, any other once opened.
pub fn is_open(st: &State, r: &str, newest: bool) -> bool {
    if newest { !st.shut.contains(r) } else { st.open.contains(r) }
}

fn toggle(st: &mut State, r: &str, newest: bool) {
    let set = if newest { &mut st.shut } else { &mut st.open };
    if !set.remove(r) {
        set.insert(r.to_string());
    }
}

/// A `taskboard://release/R3` link (Midna links R refs): its project's page with that release open.
/// Before its project is known the overview shows, and [`focus_moves`] goes on once it is.
pub fn open(m: &mut MainWindow, r: &str, cx: &mut Context<MainWindow>) {
    let project = m.releases.lists.iter().find(|(_, l)| lists_release(l, r)).map(|(p, _)| p.clone());
    m.releases.focus = Some(r.to_string());
    m.releases.open.insert(r.to_string());
    m.releases.shut.remove(r);
    m.go(project.map_or(Page::Releases, Page::ReleasesOf), cx);
    m.refresh(cx);
}

/// `project`'s page, scrolled to release `r` and with it open.
pub fn open_in(m: &mut MainWindow, project: String, r: &str, cx: &mut Context<MainWindow>) {
    m.releases.focus = Some(r.to_string());
    m.releases.open.insert(r.to_string());
    m.releases.shut.remove(r);
    m.go(Page::ReleasesOf(project), cx);
    m.refresh(cx);
}

/// The project page to go to for a linked release once its own answer says which project it's in.
pub fn focus_moves(st: &State, page: &Page) -> Option<String> {
    let f = st.focus.as_deref()?;
    let p = opt_s(st.details.get(f)?, "project")?;
    (page_project(page) != Some(p)).then(|| p.to_string())
}

/// What to fetch for a key: the path and query.
pub fn detail_call(key: &str) -> (String, Vec<(&'static str, String)>) {
    match key.strip_prefix("unreleased:") {
        Some(p) => ("releases/unreleased".into(), vec![("project", p.to_string())]),
        None => (format!("releases/{key}"), vec![]),
    }
}

/// What the page fetches in full: on a project's page its Unreleased and the open releases (the
/// newest among them unless closed); and a linked release, to learn its project.
pub fn open_rows(st: &State, page: &Page) -> Vec<String> {
    let mut v: Vec<String> = st.focus.iter().cloned().collect();
    if let Some(p) = page_project(page) {
        v.push(unreleased_key(p));
        if let Some(l) = st.lists.get(p) {
            for (ix, r) in arr(l, "releases").iter().enumerate() {
                let r = s(r, "ref");
                if is_open(st, r, ix == 0) {
                    v.push(r.to_string());
                }
            }
        }
    }
    v.sort();
    v.dedup();
    v
}

// ================================================================== view models

/// A release as a row.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: String,
    pub name: String,
    /// The first line of its notes.
    pub headline: String,
    /// When it was cut.
    pub when: String,
    pub count: String,
    /// When it was marked failed (`tb release fail`): its work went back to Unreleased.
    pub failed: Option<String>,
}

fn row(r: &Value, now: DateTime<Utc>) -> Row {
    let at = |k: &str| fmt::parse(s(r, k)).map(|t| cut_at(t, now)).unwrap_or_default();
    Row {
        key: s(r, "ref").to_string(),
        name: s(r, "name").to_string(),
        headline: headline(r),
        when: at("cut_at"),
        count: fmt::plural(i(r, "tasks"), "task", "tasks"),
        failed: opt_s(r, "failed_at").map(|_| at("failed_at")),
    }
}

/// A project's releases as rows, newest first.
pub fn rows(list: &Value, now: DateTime<Utc>) -> Vec<Row> {
    arr(list, "releases").iter().map(|r| row(r, now)).collect()
}

fn headline(r: &Value) -> String {
    opt_s(r, "notes").and_then(|n| n.lines().map(str::trim).find(|l| !l.is_empty())).unwrap_or("").to_string()
}

/// The notes after their first line (the headline, already on the row).
fn more_notes(notes: &str) -> String {
    let mut lines = notes.lines().skip_while(|l| l.trim().is_empty());
    lines.next();
    lines.collect::<Vec<_>>().join("\n").trim().to_string()
}

/// "Failed Oct 21, 9:12 PM · its work went back to Unreleased".
fn failed_line(when: &str) -> String {
    let when = if when.is_empty() { String::new() } else { format!(" {when}") };
    format!("Failed{when} · its work went back to Unreleased")
}

/// An overview card: the project's newest release, the ones before it, Unreleased and its next cut.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub total: i64,
    pub latest: Option<Row>,
    pub previous: Vec<Row>,
    /// Finished tasks not in a release yet.
    pub unreleased: i64,
    /// The next cut's pill, its tone and words: "v1.5 · 2 still to finish", "v1.5 · Cutting", or
    /// "v1.5 dropped" once the board gave up on it.
    pub cut: Option<(&'static str, String)>,
}

pub fn card(list: &Value, now: DateTime<Utc>) -> Card {
    let mut rows = rows(list, now).into_iter();
    let total = list["counts"]["total"].as_i64().unwrap_or(arr(list, "releases").len() as i64);
    let cut = match (pending(list), dropped(list)) {
        (Some(p), _) => Some((if p.running { "working" } else { "neutral" }, format!("{} · {}", p.name, p.progress()))),
        (None, Some(d)) => Some(("failed", format!("{} dropped", d.name))),
        (None, None) => None,
    };
    Card { total, latest: rows.next(), previous: rows.take(CARD_RELEASES - 1).collect(), unreleased: i(list, "unreleased"), cut }
}

/// Where a task or goal a pending cut waits for stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Standing {
    Met,
    Waiting,
    /// It can't be met any more: the cut will be dropped.
    Dead,
}

/// One task or goal a pending cut waits for.
#[derive(Clone, Debug, PartialEq)]
pub struct Wait {
    pub r: String,
    pub title: String,
    pub standing: Standing,
    /// "Done", "Running", "In review", "2 of 3 done", "Can't finish".
    pub label: String,
    /// Why it can't be met.
    pub why: Option<String>,
}

/// The project's pending cut: its name, what it waits for, and whether the board is cutting it now.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub name: String,
    pub after: Vec<Wait>,
    pub running: bool,
}

impl Pending {
    /// "2 still to finish", "Ready to cut", or "Cutting".
    pub fn progress(&self) -> String {
        if self.running {
            return "Cutting".into();
        }
        match self.after.iter().filter(|w| w.standing != Standing::Met).count() {
            0 => "Ready to cut".into(),
            n => format!("{n} still to finish"),
        }
    }
}

fn wait(a: &Value) -> Wait {
    let r = s(a, "ref").to_string();
    let goal = r.starts_with('G');
    let (standing, label) = match s(a, "state") {
        "met" => (Standing::Met, "Done".to_string()),
        "dead" => (Standing::Dead, "Can't finish".to_string()),
        _ if goal => (Standing::Waiting, format!("{} of {} done", i(a, "done"), i(a, "total"))),
        _ => (Standing::Waiting, task_label(s(a, "status"), s(a, "pr_phase")).to_string()),
    };
    Wait { title: s(a, "title").to_string(), standing, label, why: opt_s(a, "why").map(str::to_string), r }
}

/// A waiting task's state: its status, or its PR's stage once it's done but not merged.
fn task_label(status: &str, phase: &str) -> &'static str {
    match status {
        "done" => match phase {
            "checks" => "Checks",
            "fix" => "Fixing",
            "merge" => "Merging",
            _ => "In review",
        },
        "working" => "Running",
        "needs" => "Needs you",
        "blocked" => "Blocked",
        "planned" => "Planned",
        _ => "Queued",
    }
}

pub fn pending(list: &Value) -> Option<Pending> {
    let p = list.get("pending").filter(|p| p.is_object())?;
    Some(Pending { name: s(p, "name").to_string(), after: arr(p, "after").iter().map(wait).collect(), running: p["running"] == true })
}

/// The last pending cut the board dropped, until it's cut by hand or a new one is set.
#[derive(Clone, Debug, PartialEq)]
pub struct Dropped {
    pub name: String,
    pub why: String,
    /// The command that cuts it by hand.
    pub next: String,
}

pub fn dropped(list: &Value) -> Option<Dropped> {
    let d = list.get("dropped").filter(|d| d.is_object())?;
    let name = s(d, "name").to_string();
    let next = opt_s(d, "next").map_or_else(|| format!("tb release cut {name}"), str::to_string);
    Some(Dropped { why: s(d, "why").to_string(), next, name })
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

/// "October 2026": the month heading a release sits under.
fn month(r: &Value) -> String {
    fmt::parse(s(r, "cut_at")).map(|t| fmt::local(t).format("%B %Y").to_string()).unwrap_or_default()
}

/// A goal group's heading: its name, or "No goal" for tasks without one.
fn group_title(g: &Value) -> String {
    match opt_s(g, "name").filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None if g["goal"].is_null() => "No goal".into(),
        None => "Untitled goal".into(),
    }
}

/// "9 tasks · 2 goals" for a release's (or Unreleased's) detail.
fn tally(d: &Value) -> String {
    let groups = arr(d, "groups");
    let tasks: usize = groups.iter().map(|g| arr(g, "tasks").len()).sum();
    let goals = groups.iter().filter(|g| !g["goal"].is_null()).count() as i64;
    let mut out = fmt::plural(tasks as i64, "task", "tasks");
    if goals > 0 {
        out = format!("{out} · {}", fmt::plural(goals, "goal", "goals"));
    }
    out
}

/// One item of the project page's list: everything above the releases, a month's heading, a
/// release, or the end.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Top,
    Month(String),
    Release(String),
    End,
}

/// The project page's items: its releases, newest first, under their months.
pub fn items(list: &Value) -> Vec<Item> {
    let mut v = vec![Item::Top];
    let mut last: Option<String> = None;
    for r in arr(list, "releases") {
        let m = month(r);
        if last.as_ref() != Some(&m) {
            v.push(Item::Month(m.clone()));
            last = Some(m);
        }
        v.push(Item::Release(s(r, "ref").to_string()));
    }
    v.push(Item::End);
    v
}

// ================================================================== render

/// The open task's (or goal's) row: a soft accent fill and an accent edge on its left.
fn marked(t: &Theme, el: Stateful<Div>, on: bool) -> Stateful<Div> {
    let fill = t.accent_soft;
    el.border_l_2().border_color(if on { t.accent } else { transparent_black() }).when(on, move |d| d.bg(fill))
}

/// A row that opens something as a whole: a Tab stop, the hand and a hover fill.
fn target(t: &Theme, id: impl Into<ElementId>) -> Stateful<Div> {
    let hover = t.panel_2;
    focus::stop(t, div().id(id)).cursor_pointer().hover(move |s| s.bg(hover))
}

/// "Can't load …" with a way to ask again; whatever loaded before stays.
fn cant_load(t: &Theme, id: &str, text: String, cx: &mut Context<MainWindow>) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(10.))
        .py(px(4.))
        .text_size(px(13.))
        .child(div().text_color(t.down).child(text))
        .child(kit::link(t, SharedString::from(format!("releases-retry-{id}")), "Try again").on_click(cx.listener(|m, _, _, cx| m.refresh(cx))))
}

/// A task's PR, its own target inside the row that opens the task.
fn pr_link(t: &Theme, id: String, c: &Value) -> Option<Stateful<Div>> {
    let (label, url) = match (c["pr_num"].as_i64(), opt_s(c, "pr_url").filter(|u| !u.is_empty())) {
        (Some(n), Some(u)) => (format!("#{n}"), u.to_string()),
        (None, Some(u)) => ("PR".to_string(), u.to_string()),
        _ => return None,
    };
    Some(kit::link(t, SharedString::from(id), label).text_size(px(12.)).font_family(t.mono_font.clone()).on_click(move |_, _, cx| {
        cx.stop_propagation();
        cx.open_url(&url)
    }))
}

/// A finished task: the whole row opens it.
fn task_row(t: &Theme, at: &str, c: &Value, indent: bool, open: Option<&str>, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let r = s(c, "ref").to_string();
    let o = r.clone();
    let row = target(t, SharedString::from(format!("releases-task-{at}-{r}")))
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(36.))
        .pl(px(if indent { INDENT } else { 10. }))
        .pr(px(10.))
        .text_size(px(13.))
        .child(div().flex_none().w(px(16.)).font_weight(FontWeight::BOLD).text_color(t.up_fg).child("✓"))
        .child(div().flex_none().w(px(46.)).text_size(px(12.)).font_family(t.mono_font.clone()).font_weight(FontWeight::MEDIUM).text_color(t.muted).child(r.clone()))
        .child(div().flex_1().min_w_0().truncate().child(s(c, "title").to_string()))
        .children(pr_link(t, format!("releases-pr-{at}-{r}"), c))
        .on_click(cx.listener(move |m, _, _, cx| m.open_task(o.clone(), cx)));
    marked(t, row, open == Some(r.as_str()))
}

/// A goal's tasks under its ref and name (the heading opens the goal); tasks without a goal stand alone.
fn group(t: &Theme, at: &str, g: &Value, open: Option<&str>, cx: &mut Context<MainWindow>) -> Div {
    let mut col = div().flex().flex_col();
    let goal = opt_s(g, "goal").map(str::to_string);
    if let Some(r) = goal.clone() {
        let o = r.clone();
        let head = target(t, SharedString::from(format!("releases-goal-{at}-{r}")))
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(30.))
            .px(px(10.))
            .text_size(px(12.))
            .font_weight(FontWeight::BOLD)
            .child(div().flex_none().font_family(t.mono_font.clone()).text_color(t.goal).child(r.clone()))
            .child(div().flex_1().min_w_0().truncate().text_color(t.text_2).child(group_title(g)))
            .on_click(cx.listener(move |m, _, _, cx| goal::open(m, &o, false, cx)));
        col = col.child(marked(t, head, open == Some(r.as_str())));
    }
    for c in arr(g, "tasks") {
        col = col.child(task_row(t, at, c, goal.is_some(), open, cx));
    }
    col
}

/// A release's (or Unreleased's) work by goal, or why it isn't here.
fn work(t: &Theme, at: &str, d: Option<&Value>, err: bool, what: &str, open: Option<&str>, cx: &mut Context<MainWindow>) -> Div {
    let mut col = div().flex().flex_col();
    if err {
        col = col.child(div().px(px(10.)).child(cant_load(t, at, format!("Can't load {what}."), cx)));
    }
    let Some(d) = d.filter(|d| d["groups"].is_array()) else {
        return if err { col } else { col.child(div().px(px(10.)).py(px(4.)).child(kit::help(t, "Loading…"))) };
    };
    let groups = arr(d, "groups");
    if groups.is_empty() {
        return col.child(div().px(px(10.)).py(px(4.)).child(kit::help(t, "No tasks")));
    }
    for g in groups {
        col = col.child(group(t, at, g, open, cx));
    }
    col
}

/// The tick, ring, chevron or cross in front of what a pending cut waits for.
fn mark(t: &Theme, w: Standing, goal: bool) -> Div {
    let slot = div().flex_none().w(px(16.)).flex().items_center().justify_center().text_size(px(13.)).font_weight(FontWeight::BOLD);
    match w {
        Standing::Met => slot.text_color(t.up_fg).child("✓"),
        Standing::Dead => slot.text_color(t.down).child("✕"),
        Standing::Waiting if goal => slot.text_color(t.muted).child("›"),
        Standing::Waiting => slot.child(div().size(px(11.)).rounded_full().border_2().border_color(t.accent)),
    }
}

/// One task or goal the next cut waits for: the whole row opens it.
fn wait_row(t: &Theme, project: &str, w: &Wait, open: Option<&str>, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let (r, goal) = (w.r.clone(), w.r.starts_with('G'));
    let label = div()
        .id(SharedString::from(format!("releases-why-{project}-{r}")))
        .flex_none()
        .text_size(px(12.))
        .text_color(if w.standing == Standing::Dead { t.down } else { t.muted })
        .child(w.label.clone())
        .when_some(w.why.clone(), |d, why| d.tooltip(kit::tip(why)));
    let o = r.clone();
    let row = target(t, SharedString::from(format!("releases-wait-{project}-{r}")))
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(40.))
        .px(px(10.))
        .text_size(px(13.))
        .child(mark(t, w.standing, goal))
        .child(div().flex_none().w(px(46.)).text_size(px(12.)).font_family(t.mono_font.clone()).font_weight(FontWeight::MEDIUM).text_color(if goal { t.goal } else { t.muted }).child(r.clone()))
        .child(div().flex_1().min_w_0().truncate().child(w.title.clone()))
        .child(label)
        .on_click(cx.listener(move |m, _, _, cx| if goal { goal::open(m, &o, false, cx) } else { m.open_task(o.clone(), cx) }));
    marked(t, row, open == Some(w.r.as_str()))
}

/// A small bold label over a group of rows.
fn label(t: &Theme, text: &'static str) -> Div {
    div().pt(px(8.)).pb(px(2.)).px(px(12.)).text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(t.muted).child(text)
}

/// Everything above the releases: the way back, the counts, a dropped cut, the next release (what
/// it cuts after and what's already finished) and the Past releases heading.
fn top(m: &MainWindow, t: &Theme, p: &str, l: &Value, cx: &mut Context<MainWindow>) -> Div {
    let open = crate::app::open_ref(m).map(str::to_string);
    let open = open.as_deref();
    let counts = &l["counts"];
    let back = div().flex().items_center().gap(px(6.)).text_size(px(13.)).child(div().text_color(t.accent).child("‹")).child(kit::link(t, "releases-back", "Releases").underline().on_click(cx.listener(|m, _, _, cx| m.go(Page::Releases, cx))));
    let mut sub = vec![fmt::plural(i(counts, "total"), "release", "releases"), format!("{} in the last 30 days", i(counts, "recent"))];
    if i(counts, "failed") > 0 {
        sub.push(format!("{} failed", i(counts, "failed")));
    }
    let mut col = div()
        .flex()
        .flex_col()
        .pt(px(22.))
        .pb(px(4.))
        .child(back)
        .child(div().pt(px(14.)).text_size(px(22.)).font_weight(FontWeight::BOLD).child(p.to_string()))
        .child(div().text_size(px(13.)).text_color(t.muted).child(sub.join(" · ")));
    if m.releases.list_errs.contains_key(p) {
        col = col.child(div().pt(px(10.)).child(cant_load(t, p, format!("Can't load {p}'s releases."), cx)));
    }
    if let Some(d) = dropped(l) {
        let why = if d.why.is_empty() { String::new() } else { format!(": {}", d.why) };
        col = col.child(
            div()
                .pt(px(16.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .text_size(px(13.))
                .child(div().font_weight(FontWeight::BOLD).text_color(t.down).child(format!("{} was dropped{why}", d.name)))
                .child(div().flex().flex_wrap().gap(px(6.)).text_color(t.muted).child("Cut it by hand once it's ready:").child(div().font_family(t.mono_font.clone()).text_color(t.text_2).child(d.next))),
        );
    }

    let key = unreleased_key(p);
    let pc = pending(l);
    let un = m.releases.details.get(&key);
    let un_err = m.releases.row_errs.contains_key(&key);
    let waiting = i(l, "unreleased");
    let heading = |title: &'static str| div().pt(px(24.)).pb(px(4.)).flex().flex_wrap().items_center().gap(px(8.)).child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child(title));
    if let Some(pc) = &pc {
        col = col.child(heading("Next release").child(div().text_size(px(14.)).font_family(t.mono_font.clone()).font_weight(FontWeight::BOLD).child(pc.name.clone())).child(kit::tone_pill(t, if pc.running { "working" } else { "neutral" }, pc.progress())));
        if !pc.after.is_empty() {
            col = col.child(label(t, "Cuts when these finish"));
            for w in &pc.after {
                col = col.child(wait_row(t, p, w, open, cx));
            }
        }
        if waiting > 0 {
            col = col.child(label(t, "Already finished")).child(work(t, &key, un, un_err, "Unreleased", open, cx));
        }
    } else if waiting > 0 {
        let n = un.filter(|d| d["groups"].is_array()).map(tally).unwrap_or_else(|| fmt::plural(waiting, "task", "tasks"));
        col = col.child(heading("Unreleased").child(div().text_size(px(13.)).text_color(t.muted).child(n))).child(work(t, &key, un, un_err, "Unreleased", open, cx));
    }

    if !arr(l, "releases").is_empty() {
        col = col.child(div().pt(px(28.)).text_size(px(16.)).font_weight(FontWeight::BOLD).child("Past releases"));
    } else if waiting == 0 && pc.is_none() {
        col = col.child(div().pt(px(20.)).child(kit::empty(t, "No releases yet")));
    }
    col
}

/// A release: the whole row opens it in place, to its notes and its work by goal.
fn release_row(m: &MainWindow, t: &Theme, r: &Value, newest: bool, cx: &mut Context<MainWindow>) -> Div {
    let open_ref = crate::app::open_ref(m).map(str::to_string);
    let row = row(r, fmt::now());
    let open = is_open(&m.releases, &row.key, newest);
    let key = row.key.clone();
    let end = match row.failed {
        Some(_) => kit::tone_pill(t, "failed", "Failed").into_any_element(),
        None => div().flex_none().whitespace_nowrap().text_size(px(12.)).text_color(t.muted).child(row.count.clone()).into_any_element(),
    };
    let head = target(t, SharedString::from(format!("releases-row-{}", row.key)))
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(40.))
        .px(px(10.))
        .text_size(px(13.))
        .child(div().flex_none().w(px(14.)).text_color(t.muted).child(if open { "⌄" } else { "›" }))
        .child(div().flex_none().w(px(84.)).truncate().font_family(t.mono_font.clone()).font_weight(FontWeight::BOLD).child(row.name.clone()))
        .child(div().flex_1().min_w_0().truncate().child(row.headline.clone()))
        .child(div().flex_none().whitespace_nowrap().text_size(px(12.)).text_color(t.muted).child(row.when.clone()))
        .child(end)
        .on_click(cx.listener(move |m, _, _, cx| {
            toggle(&mut m.releases, &key, newest);
            m.refresh(cx);
        }));
    let mut out = div().flex().flex_col().child(head);
    if open {
        let d = m.releases.details.get(&row.key);
        let notes = d.and_then(|d| opt_s(d, "notes")).or(opt_s(r, "notes")).map(more_notes).unwrap_or_default();
        let mut body = div().flex().flex_col().gap(px(2.)).pl(px(26.)).pb(px(10.));
        if let Some(f) = &row.failed {
            body = body.child(div().px(px(10.)).text_size(px(13.)).text_color(t.down).child(failed_line(f)));
        }
        if !notes.is_empty() {
            body = body.child(div().px(px(10.)).pb(px(4.)).text_size(px(13.)).text_color(t.muted).child(notes));
        }
        let has_work = d.is_some_and(|d| !arr(d, "groups").is_empty());
        if row.failed.is_none() || has_work {
            body = body.child(work(t, &row.key, d, m.releases.row_errs.contains_key(&row.key), &row.name, open_ref.as_deref(), cx));
        }
        out = out.child(body);
    }
    out
}

/// One item of the project page's list.
fn item(m: &mut MainWindow, ix: usize, _window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let Some(h) = &m.releases.list else { return div().into_any_element() };
    let (p, it) = (h.project.clone(), h.items.get(ix).cloned());
    let Some(l) = m.releases.lists.get(&p).cloned() else { return div().into_any_element() };
    let wrap = |c: AnyElement| div().px(px(PAGE_X)).child(div().max_w(px(MAX_W)).child(c)).into_any_element();
    match it {
        Some(Item::Top) => wrap(top(m, &t, &p, &l, cx).into_any_element()),
        Some(Item::Month(name)) => wrap(div().pt(px(14.)).pb(px(4.)).px(px(12.)).text_size(px(11.)).font_weight(FontWeight::BOLD).text_color(t.muted).child(name.to_uppercase()).into_any_element()),
        Some(Item::Release(r)) => {
            let rels = arr(&l, "releases");
            match rels.iter().position(|x| s(x, "ref") == r) {
                Some(ix) => wrap(release_row(m, &t, &rels[ix], ix == 0, cx).into_any_element()),
                None => div().into_any_element(),
            }
        }
        Some(Item::End) => wrap(div().pt(px(12.)).pb(px(32.)).when(l["more"] == true, |d| d.child(kit::help(&t, "Loading…"))).into_any_element()),
        None => div().into_any_element(),
    }
}

/// How an item looks, so only the ones that changed are measured again.
fn sig(st: &State, l: &Value, it: &Item) -> String {
    match it {
        Item::Release(r) => {
            let newest = arr(l, "releases").first().is_some_and(|x| s(x, "ref") == r);
            let open = is_open(st, r, newest);
            let n = if open { st.details.get(r).map_or(-1, |d| arr(d, "groups").iter().map(|g| arr(g, "tasks").len() as i64 + 1).sum()) } else { 0 };
            format!("{open}:{n}:{}", st.row_errs.contains_key(r))
        }
        _ => String::new(),
    }
}

/// Keep the list's items in step with the project's releases: an older page comes in at the end, so
/// the place scrolled to stays put. The top always measures again (it changes with the data).
fn sync(st: &mut State, project: &str, l: &Value, items: Vec<Item>) {
    let sigs: Vec<String> = items.iter().map(|it| sig(st, l, it)).collect();
    let h = match &mut st.list {
        Some(h) if h.project == project => h,
        slot => slot.insert(History { project: project.to_string(), state: ListState::new(items.len(), ListAlignment::Top, px(800.)), items: items.clone(), sigs: sigs.clone() }),
    };
    if h.items != items {
        let keep = h.items.len().saturating_sub(1);
        if items.len() > keep && items[..keep] == h.items[..keep] {
            h.state.splice(keep..h.items.len(), items.len() - keep);
        } else {
            h.state.reset(items.len());
        }
        h.sigs = vec![String::new(); items.len()];
        h.items = items;
    }
    for (ix, s) in sigs.iter().enumerate() {
        if h.sigs.get(ix) != Some(s) {
            h.state.remeasure_items(ix..ix + 1);
        }
    }
    h.sigs = sigs;
    h.state.remeasure_items(0..1);
}

/// What a page shows before a project's list has come: Loading, or why it can't.
fn not_loaded(m: &MainWindow, t: &Theme, p: &str, cx: &mut Context<MainWindow>) -> Div {
    if m.down.is_some() {
        cant_load(t, p, "Can't load the board.".into(), cx)
    } else if m.releases.list_errs.contains_key(p) {
        cant_load(t, p, format!("Can't load {p}'s releases."), cx)
    } else {
        kit::help(t, "Loading…")
    }
}

fn project_page(m: &mut MainWindow, p: &str, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let Some(l) = m.releases.lists.get(p).cloned() else {
        return div().px(px(PAGE_X)).pt(px(22.)).child(not_loaded(m, &t, p, cx)).into_any_element();
    };
    let items = items(&l);
    sync(&mut m.releases, p, &l, items);
    let Some(h) = &m.releases.list else { return div().into_any_element() };
    let len = arr(&l, "releases").len();

    // A linked release: scroll to it once listed, or ask for more until it is.
    if let Some(f) = m.releases.focus.clone() {
        match h.items.iter().position(|it| matches!(it, Item::Release(r) if *r == f)) {
            Some(ix) => {
                h.state.scroll_to_reveal_item(ix);
                m.releases.focus = None;
            }
            None if l["more"] == true => {
                if len >= m.releases.limit() {
                    m.releases.limit = len + PAGE;
                    m.refresh(cx);
                }
            }
            None => m.releases.focus = None,
        }
    }
    // Near the end: ask for the next page (once the last answer has caught up with the limit).
    let Some(h) = &m.releases.list else { return div().into_any_element() };
    if l["more"] == true && len >= m.releases.limit() && h.state.logical_scroll_top().item_ix + AHEAD >= h.items.len() {
        m.releases.limit = len + PAGE;
        m.refresh(cx);
    }
    let Some(h) = &m.releases.list else { return div().into_any_element() };
    list(h.state.clone(), cx.processor(item)).size_full().line_height(relative(1.4)).into_any_element()
}

fn overview_card(m: &MainWindow, t: &Theme, p: &str, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let go = |id: String, r: Option<String>, cx: &mut Context<MainWindow>| {
        let p = p.to_string();
        target(t, SharedString::from(id)).on_click(cx.listener(move |m, _, _, cx| match &r {
            Some(r) => open_in(m, p.clone(), r, cx),
            None => m.go(Page::ReleasesOf(p.clone()), cx),
        }))
    };
    let col = div().id(SharedString::from(format!("releases-card-{p}"))).flex_1().min_w_0().flex().flex_col().gap(px(10.)).p(px(16.)).rounded(px(12.)).border_1().border_color(t.border).bg(t.card);
    let head = div().flex().items_center().gap(px(10.)).child(div().flex_1().min_w_0().truncate().text_size(px(15.)).font_weight(FontWeight::BOLD).child(p.to_string()));
    let Some(l) = m.releases.lists.get(p) else {
        return col.child(head).child(not_loaded(m, t, p, cx));
    };
    let c = card(l, fmt::now());
    let all = go(format!("releases-all-{p}"), None, cx)
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(6.))
        .py(px(2.))
        .rounded(px(6.))
        .text_size(px(13.))
        .text_color(t.muted)
        .child(fmt::plural(c.total, "release", "releases"))
        .child("›");
    let mut col = col.child(head.child(all));
    if m.releases.list_errs.contains_key(p) {
        col = col.child(cant_load(t, p, format!("Can't load {p}'s releases."), cx));
    }
    if let Some(r) = &c.latest {
        let meta = [r.when.clone(), r.count.clone()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
        col = col.child(
            go(format!("releases-latest-{p}"), Some(r.key.clone()), cx)
                .flex()
                .flex_col()
                .gap(px(2.))
                .mx(px(-8.))
                .px(px(8.))
                .py(px(4.))
                .rounded(px(8.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().min_w_0().truncate().text_size(px(13.)).font_family(t.mono_font.clone()).font_weight(FontWeight::BOLD).child(r.name.clone()))
                        .child(div().flex_none().text_size(px(12.)).text_color(t.muted).child(meta))
                        .when(r.failed.is_some(), |d| d.child(kit::tone_pill(t, "failed", "Failed"))),
                )
                .when(!r.headline.is_empty(), |d| d.child(div().truncate().text_size(px(13.)).child(r.headline.clone()))),
        );
    }
    if c.unreleased > 0 || c.cut.is_some() {
        let n = fmt::plural(c.unreleased, "task", "tasks");
        col = col.child(
            go(format!("releases-unreleased-{p}"), None, cx)
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(9.))
                .rounded(px(9.))
                .bg(t.seg)
                .text_size(px(13.))
                .child(div().flex_1().min_w_0().truncate().flex().gap(px(4.)).child(div().flex_none().font_weight(FontWeight::BOLD).child("Unreleased")).child(div().text_color(t.text_2).child(format!("· {n}"))))
                .children(c.cut.clone().map(|(tone, x)| kit::tone_pill(t, tone, x)))
                .child(div().flex_none().text_color(t.muted).child("›")),
        );
    }
    if !c.previous.is_empty() {
        let mut prev = div().flex().flex_col();
        for r in &c.previous {
            let end = match r.failed {
                Some(_) => kit::tone_pill(t, "failed", "Failed").into_any_element(),
                None => div().flex_none().whitespace_nowrap().text_size(px(12.)).text_color(t.muted).child(r.when.clone()).into_any_element(),
            };
            prev = prev.child(
                go(format!("releases-prev-{}", r.key), Some(r.key.clone()), cx)
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .h(px(30.))
                    .border_t_1()
                    .border_color(t.divider)
                    .text_size(px(13.))
                    .child(div().flex_none().w(px(72.)).truncate().font_family(t.mono_font.clone()).font_weight(FontWeight::BOLD).child(r.name.clone()))
                    .child(div().flex_1().min_w_0().truncate().child(r.headline.clone()))
                    .child(end),
            );
        }
        col = col.child(prev);
    }
    col
}

/// The overview's columns at this window width.
fn columns(window: &Window) -> usize {
    let room = f32::from(window.viewport_size().width) - sidebar::WIDTH - 2. * PAGE_X;
    (((room + GAP) / (CARD_MIN + GAP)).floor() as usize).clamp(1, 4)
}

fn overview(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let projects = projects_on(m.state());
    let lists: Vec<Option<&Value>> = projects.iter().map(|p| m.releases.lists.get(p)).collect();
    let waiting: i64 = lists.iter().flatten().map(|l| i(l, "unreleased")).sum();
    let cuts = lists.iter().flatten().filter(|l| pending(l).is_some()).count() as i64;
    let mut sum = vec![fmt::plural(waiting, "unreleased task", "unreleased tasks")];
    if cuts > 0 {
        sum.push(fmt::plural(cuts, "pending cut", "pending cuts"));
    }
    let head = div()
        .flex()
        .flex_wrap()
        .items_baseline()
        .gap(px(14.))
        .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Releases"))
        .when(lists.iter().all(Option::is_some), |d| d.child(div().text_size(px(13.)).text_color(t.muted).child(sum.join(" · "))));
    let mut page = div().flex().flex_col().gap(px(20.)).px(px(PAGE_X)).pt(px(22.)).pb(px(32.)).line_height(relative(1.4)).child(head);
    if projects.is_empty() {
        page = page.child(if m.down.is_some() { cant_load(&t, "board", "Can't load the board.".into(), cx) } else { kit::help(&t, "Loading…") });
    }
    let cols = columns(window);
    for chunk in projects.chunks(cols) {
        let mut line = div().flex().items_start().gap(px(GAP));
        for p in chunk {
            line = line.child(overview_card(m, &t, p, cx));
        }
        for _ in chunk.len()..cols {
            line = line.child(div().flex_1().min_w_0());
        }
        page = page.child(line);
    }
    div().id("releases-page").flex_1().min_w_0().h_full().overflow_y_scroll().restrict_scroll_to_axis().track_scroll(&m.releases.scroll).child(page).into_any_element()
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    match m.page.clone() {
        Page::ReleasesOf(p) => div().flex_1().min_w_0().h_full().child(project_page(m, &p, cx)).into_any_element(),
        _ => overview(m, window, cx),
    }
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

    fn rel(id: i64, name: &str, cut: DateTime<Utc>, tasks: i64) -> Value {
        json!({"id": id, "ref": format!("R{id}"), "name": name, "cut_at": cut.to_rfc3339(), "tasks": tasks})
    }

    #[test]
    fn a_card_shows_the_newest_release_and_the_three_before() {
        let now = at(2026, 10, 15, 16);
        let mut newest = rel(5, "v1.4", at(2026, 10, 15, 15), 2);
        newest["notes"] = json!("\n  Faster log\nand more");
        let mut failed = rel(3, "v1.2", at(2025, 12, 1, 9), 4);
        failed["failed_at"] = json!(at(2025, 12, 2, 21).to_rfc3339());
        let list = json!({"unreleased": 3, "counts": {"total": 31}, "pending": {"name": "v1.5", "running": false, "after": [{"ref": "T1", "state": "waiting"}, {"ref": "T2", "state": "met"}]}, "releases": [
            newest,
            rel(4, "v1.3", at(2026, 10, 7, 9), 1),
            failed,
            rel(2, "v1.1", at(2025, 11, 1, 9), 1),
        ]});
        let c = card(&list, now);
        assert_eq!(c.total, 31);
        let latest = c.latest.unwrap();
        assert_eq!((latest.name.as_str(), latest.headline.as_str(), latest.when.as_str(), latest.count.as_str()), ("v1.4", "Faster log", "Today, 3:05 PM", "2 tasks"));
        assert_eq!(c.previous.iter().map(|r| (r.name.as_str(), r.when.as_str())).collect::<Vec<_>>(), [("v1.3", "Oct 7, 9:05 AM"), ("v1.2", "Dec 1, 2025, 9:05 AM"), ("v1.1", "Nov 1, 2025, 9:05 AM")]);
        assert_eq!(c.previous[1].failed.as_deref(), Some("Dec 2, 2025, 9:05 PM"));
        assert_eq!((c.unreleased, c.cut), (3, Some(("neutral", "v1.5 · 1 still to finish".to_string()))));
        assert_eq!(card(&json!({"unreleased": 0, "releases": []}), now), Card { total: 0, latest: None, previous: vec![], unreleased: 0, cut: None });
    }

    #[test]
    fn a_card_shows_a_cut_in_progress_or_dropped() {
        let now = at(2026, 10, 15, 16);
        let running = json!({"releases": [], "pending": {"name": "v2", "running": true, "after": []}});
        assert_eq!(card(&running, now).cut, Some(("working", "v2 · Cutting".to_string())));
        let gone = json!({"releases": [], "pending": null, "dropped": {"name": "v0.14.0", "why": "failed", "next": "tb release cut v0.14.0"}});
        assert_eq!(card(&gone, now).cut, Some(("failed", "v0.14.0 dropped".to_string())));
        assert_eq!(dropped(&gone), Some(Dropped { name: "v0.14.0".into(), why: "failed".into(), next: "tb release cut v0.14.0".into() }));
        assert_eq!(dropped(&json!({"dropped": null})), None);
    }

    #[test]
    fn releases_sit_under_their_months_newest_first() {
        let list = json!({"releases": [
            rel(5, "v1.4", at(2026, 10, 15, 15), 2),
            rel(4, "v1.3", at(2026, 10, 7, 9), 1),
            rel(2, "v1.1", at(2026, 9, 20, 9), 1),
        ]});
        let r = |r: &str| Item::Release(r.into());
        assert_eq!(items(&list), [Item::Top, Item::Month("October 2026".into()), r("R5"), r("R4"), Item::Month("September 2026".into()), r("R2"), Item::End]);
        assert_eq!(items(&json!({"releases": []})), [Item::Top, Item::End]);
    }

    #[test]
    fn only_projects_with_releases_on_are_shown() {
        let state = json!({"releases": ["kass", "midna"]});
        assert_eq!(projects_on(&json!({})), Vec::<String>::new());
        assert_eq!(shown(&state, &Page::Releases), ["kass", "midna"]);
        assert_eq!(shown(&state, &Page::ReleasesOf("midna".into())), ["midna"]);
        // A project whose releases were turned off shows nothing; the page goes back to the overview.
        assert_eq!(shown(&state, &Page::ReleasesOf("web".into())), Vec::<String>::new());
        assert_eq!(shown(&state, &Page::Board), Vec::<String>::new());
    }

    #[test]
    fn a_pending_cut_lists_what_it_waits_for() {
        assert_eq!(pending(&json!({"on": true, "releases": [], "pending": null})), None);
        let list = json!({"pending": {"name": "v1.5", "running": false, "after": [
            {"ref": "T627", "state": "met", "title": "Push on review", "status": "done"},
            {"ref": "T630", "state": "waiting", "title": "Phone link", "status": "working"},
            {"ref": "T631", "state": "waiting", "title": "Docs", "status": "done", "pr_phase": "review"},
            {"ref": "G12", "state": "waiting", "title": "Releases page", "done": 2, "total": 3},
            {"ref": "T9", "state": "dead", "why": "T9 failed", "title": "Old"}]}});
        let p = pending(&list).unwrap();
        let got: Vec<(&str, &str, Standing, &str)> = p.after.iter().map(|w| (w.r.as_str(), w.title.as_str(), w.standing, w.label.as_str())).collect();
        assert_eq!(
            got,
            [
                ("T627", "Push on review", Standing::Met, "Done"),
                ("T630", "Phone link", Standing::Waiting, "Running"),
                ("T631", "Docs", Standing::Waiting, "In review"),
                ("G12", "Releases page", Standing::Waiting, "2 of 3 done"),
                ("T9", "Old", Standing::Dead, "Can't finish"),
            ]
        );
        assert_eq!(p.after[4].why.as_deref(), Some("T9 failed"));
        assert_eq!(p.progress(), "4 still to finish");
        assert_eq!(Pending { after: vec![p.after[0].clone()], ..p.clone() }.progress(), "Ready to cut");
        assert_eq!(Pending { running: true, ..p }.progress(), "Cutting");
    }

    #[test]
    fn a_project_page_fetches_unreleased_and_the_open_rows() {
        let mut st = State::default();
        let page = Page::ReleasesOf("web".into());
        assert_eq!(open_rows(&st, &page), ["unreleased:web"]);
        st.lists.insert("web".into(), json!({"releases": [{"ref": "R3"}, {"ref": "R2"}, {"ref": "R1"}]}));
        st.open.extend(["R1".to_string(), "R9".to_string()]);
        assert_eq!(open_rows(&st, &page), ["R1", "R3", "unreleased:web"], "the newest starts open");
        toggle(&mut st, "R3", true);
        assert_eq!(open_rows(&st, &page), ["R1", "unreleased:web"], "closed by hand");
        toggle(&mut st, "R3", true);
        toggle(&mut st, "R1", false);
        assert_eq!(open_rows(&st, &page), ["R3", "unreleased:web"]);
        assert_eq!(open_rows(&st, &Page::Releases), Vec::<String>::new(), "the overview fetches no rows");
        st.focus = Some("R7".into());
        assert_eq!(open_rows(&st, &Page::Releases), ["R7"], "a linked release, to learn its project");
        assert_eq!(detail_call("R2"), ("releases/R2".to_string(), vec![]));
        assert_eq!(detail_call("unreleased:web"), ("releases/unreleased".to_string(), vec![("project", "web".to_string())]));
    }

    #[test]
    fn a_failed_answer_is_kept_beside_the_older_data_until_one_comes_back() {
        let mut st = State::default();
        land(&mut st, vec![("web".into(), Ok(json!({"releases": []})))], vec![("R2".into(), Ok(json!({"groups": []})))]);
        land(&mut st, vec![("web".into(), Err("busy".into())), ("api".into(), Err("busy".into()))], vec![("R2".into(), Err("busy".into()))]);
        assert_eq!(st.lists.get("web"), Some(&json!({"releases": []})), "the older list stays");
        assert!(st.list_errs.contains_key("web") && st.list_errs.contains_key("api") && st.row_errs.contains_key("R2"));
        assert!(st.details.contains_key("R2"));
        land(&mut st, vec![("api".into(), Ok(json!({"releases": []})))], vec![("R2".into(), Ok(json!({"groups": []})))]);
        assert!(!st.list_errs.contains_key("api") && !st.row_errs.contains_key("R2"));
        assert!(st.list_errs.contains_key("web"), "still failing");
    }

    #[test]
    fn a_linked_release_moves_to_its_projects_page() {
        let mut st = State::default();
        assert_eq!(focus_moves(&st, &Page::Releases), None);
        st.focus = Some("R7".into());
        assert_eq!(focus_moves(&st, &Page::Releases), None, "its answer hasn't come");
        st.details.insert("R7".into(), json!({"ref": "R7", "project": "api"}));
        assert_eq!(focus_moves(&st, &Page::Releases).as_deref(), Some("api"));
        assert_eq!(focus_moves(&st, &Page::ReleasesOf("api".into())), None, "already there");
    }

    #[test]
    fn the_overview_asks_for_a_few_and_a_project_page_for_pages() {
        let mut st = State::default();
        assert_eq!(list_limit(&st, &Page::Releases), CARD_RELEASES);
        assert_eq!(list_limit(&st, &Page::ReleasesOf("web".into())), PAGE);
        st.limit = 120;
        assert_eq!(list_limit(&st, &Page::ReleasesOf("web".into())), 120);
    }

    #[test]
    fn a_tally_counts_tasks_and_goals() {
        let d = json!({"groups": [{"goal": "G2", "tasks": [{}, {}]}, {"goal": null, "tasks": [{}]}]});
        assert_eq!(tally(&d), "3 tasks · 1 goal");
        assert_eq!(tally(&json!({"groups": [{"goal": null, "tasks": [{}]}]})), "1 task");
    }

    #[test]
    fn groups_without_a_goal_say_so() {
        assert_eq!(group_title(&json!({"goal": "G2", "name": "Billing"})), "Billing");
        assert_eq!(group_title(&json!({"goal": null, "name": null})), "No goal");
        assert_eq!(group_title(&json!({"goal": "G4", "name": ""})), "Untitled goal");
    }

    #[test]
    fn an_open_release_shows_its_notes_after_the_headline_and_why_it_failed() {
        assert_eq!(more_notes("\n Passkeys\nAlso trims the bundle.\n"), "Also trims the bundle.");
        assert_eq!(more_notes("Only a headline"), "");
        assert_eq!(failed_line("Oct 21, 9:12 PM"), "Failed Oct 21, 9:12 PM · its work went back to Unreleased");
        assert_eq!(failed_line(""), "Failed · its work went back to Unreleased");
    }
}
