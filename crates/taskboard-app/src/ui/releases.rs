//! The Releases pages. The overview (`Page::Releases`) is a card per project with releases on (the
//! state's `releases`): its newest release, what waits in Unreleased (and its pending cut), and the
//! few releases before. A project's page (`Page::ReleasesOf`) shows its counts, then Unreleased (the
//! pending cut's checklist on top, then the work by goal), its newest release always open, and every
//! older release by month, each a row that opens in place to its tasks by goal (`GET /releases/:id`).
//! That page is a virtual list, paged from the board (`GET /releases?limit=`) as it nears its end.
//! With no project on, the pages are hidden. Read-only: cutting and editing releases stay in `tb`.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, i, opt_s, s};
use crate::theme::Theme;
use crate::ui::{goal, kit, sidebar};
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

#[derive(Default)]
pub struct State {
    /// The older releases open in place on a project's page, by ref.
    pub open: HashSet<String>,
    /// `GET /releases?project=` by project: an overview card's few, or the project page's pages.
    pub lists: HashMap<String, Value>,
    /// `GET /releases/:id` (or `/releases/unreleased`) by [`unreleased_key`] or ref.
    pub details: HashMap<String, Value>,
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

/// A `taskboard://release/R3` link (Midna links R refs): its project's page with that release open.
/// Before its project is known the overview shows, and [`focus_moves`] goes on once it is.
pub fn open(m: &mut MainWindow, r: &str, cx: &mut Context<MainWindow>) {
    let project = m.releases.lists.iter().find(|(_, l)| lists_release(l, r)).map(|(p, _)| p.clone());
    m.releases.focus = Some(r.to_string());
    m.releases.open.insert(r.to_string());
    m.go(project.map_or(Page::Releases, Page::ReleasesOf), cx);
    m.refresh(cx);
}

/// `project`'s page, scrolled to release `r` and with it open.
pub fn open_in(m: &mut MainWindow, project: String, r: &str, cx: &mut Context<MainWindow>) {
    m.releases.focus = Some(r.to_string());
    m.releases.open.insert(r.to_string());
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

/// What the page fetches in full: on a project's page its Unreleased, its newest release and the
/// open older ones; and a linked release, to learn its project.
pub fn open_rows(st: &State, page: &Page) -> Vec<String> {
    let mut v: Vec<String> = st.focus.iter().cloned().collect();
    if let Some(p) = page_project(page) {
        v.push(unreleased_key(p));
        if let Some(l) = st.lists.get(p) {
            let rels = arr(l, "releases");
            v.extend(rels.first().map(|r| s(r, "ref").to_string()));
            v.extend(rels.iter().skip(1).map(|r| s(r, "ref").to_string()).filter(|r| st.open.contains(r)));
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
    /// Marked failed (`tb release fail`): its work went back to Unreleased.
    pub failed: bool,
}

fn row(r: &Value, now: DateTime<Utc>) -> Row {
    Row {
        key: s(r, "ref").to_string(),
        name: s(r, "name").to_string(),
        headline: headline(r),
        when: fmt::parse(s(r, "cut_at")).map(|t| cut_at(t, now)).unwrap_or_default(),
        count: fmt::plural(i(r, "tasks"), "task", "tasks"),
        failed: opt_s(r, "failed_at").is_some(),
    }
}

/// A project's releases as rows, newest first.
pub fn rows(list: &Value, now: DateTime<Utc>) -> Vec<Row> {
    arr(list, "releases").iter().map(|r| row(r, now)).collect()
}

fn headline(r: &Value) -> String {
    opt_s(r, "notes").and_then(|n| n.lines().map(str::trim).find(|l| !l.is_empty())).unwrap_or("").to_string()
}

/// An overview card: the project's newest release, the ones before it, Unreleased and its pending cut.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub total: i64,
    pub latest: Option<Row>,
    pub previous: Vec<Row>,
    pub unreleased: String,
    /// "v1.5 pending", or "v1.5 cutting" while the board cuts it.
    pub pending: Option<String>,
}

pub fn card(list: &Value, now: DateTime<Utc>) -> Card {
    let mut rows = rows(list, now).into_iter();
    let total = list["counts"]["total"].as_i64().unwrap_or(arr(list, "releases").len() as i64);
    Card {
        total,
        latest: rows.next(),
        previous: rows.take(CARD_RELEASES - 1).collect(),
        unreleased: fmt::plural(i(list, "unreleased"), "task", "tasks"),
        pending: pending(list).map(|p| format!("{} {}", p.name, if p.running { "cutting" } else { "pending" })),
    }
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
    /// "Done", "Running", "In review", "2 of 3", "Can't finish".
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
    /// "1 of 3 ready", or "Cutting".
    pub fn progress(&self) -> String {
        if self.running {
            return "Cutting".into();
        }
        let met = self.after.iter().filter(|w| w.standing == Standing::Met).count();
        format!("{met} of {} ready", self.after.len())
    }
}

fn wait(a: &Value) -> Wait {
    let r = s(a, "ref").to_string();
    let goal = r.starts_with('G');
    let (standing, label) = match s(a, "state") {
        "met" => (Standing::Met, "Done".to_string()),
        "dead" => (Standing::Dead, "Can't finish".to_string()),
        _ if goal => (Standing::Waiting, format!("{} of {}", i(a, "done"), i(a, "total"))),
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

/// One item of the project page's list: everything above the older releases, a month's heading, an
/// older release (`first`/`last` in its month, for the box around them), or the end.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Top,
    Month(String),
    Release { r: String, first: bool, last: bool },
    End,
}

/// The project page's items: the older releases (all but the newest) under their months.
pub fn items(list: &Value) -> Vec<Item> {
    let mut v = vec![Item::Top];
    let rels: Vec<&Value> = arr(list, "releases").iter().skip(1).collect();
    for (ix, r) in rels.iter().enumerate() {
        let m = month(r);
        let first = ix == 0 || month(rels[ix - 1]) != m;
        let last = rels.get(ix + 1).is_none_or(|n| month(n) != m);
        if first {
            v.push(Item::Month(m));
        }
        v.push(Item::Release { r: s(r, "ref").to_string(), first, last });
    }
    v.push(Item::End);
    v
}

// ================================================================== render

fn goal_link(t: &Theme, id: String, r: String, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let r2 = r.clone();
    div()
        .id(SharedString::from(id))
        .flex_none()
        .font_family(t.mono_font.clone())
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.goal)
        .cursor_pointer()
        .hover(|s| s.underline())
        .child(r)
        .on_click(cx.listener(move |m, _, _, cx| goal::open(m, &r2, false, cx)))
}

fn task_row(t: &Theme, at: &str, c: &Value, cx: &mut Context<MainWindow>) -> Stateful<Div> {
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
        .id(SharedString::from(format!("releases-task-{at}-{r}")))
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(34.))
        .px(px(16.))
        .border_t_1()
        .border_color(t.divider)
        .text_size(px(13.))
        .hover(move |s| s.bg(hover))
        .child(div().flex_none().w(px(46.)).text_size(px(12.)).font_family(t.mono_font.clone()).text_color(t.muted).child(r.clone()))
        .child(
            div().min_w_0().flex_1().flex().child(
                div()
                    .id(SharedString::from(format!("releases-task-title-{at}-{r}")))
                    .min_w_0()
                    .truncate()
                    .cursor_pointer()
                    .hover(move |s| s.text_color(link).underline())
                    .child(s(c, "title").to_string())
                    .on_click(cx.listener(move |m, _, _, cx| m.open_task(o.clone(), cx))),
            ),
        )
        .child(div().flex_none().w(px(56.)).flex().justify_end().children(pr.map(|(label, url)| {
            kit::link(t, SharedString::from(format!("releases-pr-{at}-{r}")), label).text_size(px(12.)).font_family(t.mono_font.clone()).on_click(move |_, _, cx| cx.open_url(&url))
        })))
        .child(div().flex_none().w(px(52.)).whitespace_nowrap().text_size(px(12.)).text_color(t.muted).text_right().child(finished(c)))
}

/// A goal's tasks in their own box: the goal's ref and name over its task rows.
fn goal_card(t: &Theme, at: &str, g: &Value, cx: &mut Context<MainWindow>) -> Div {
    let tasks = arr(g, "tasks");
    let head = div()
        .flex()
        .items_baseline()
        .gap(px(8.))
        .px(px(16.))
        .py(px(10.))
        .text_size(px(13.))
        .children(opt_s(g, "goal").map(|r| goal_link(t, format!("releases-goal-{at}-{r}"), r.to_string(), cx)))
        .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::BOLD).child(group_title(g)))
        .child(div().flex_none().text_color(t.muted).child(fmt::plural(tasks.len() as i64, "task", "tasks")));
    let mut card = div().flex().flex_col().rounded(px(12.)).border_1().border_color(t.border).bg(t.card).overflow_hidden().child(head);
    for c in tasks {
        card = card.child(task_row(t, at, c, cx));
    }
    card
}

/// A release's (or Unreleased's) notes and its goals, each in its box.
fn work(t: &Theme, at: &str, d: Option<&Value>, notes: bool, cx: &mut Context<MainWindow>) -> Div {
    let col = div().flex().flex_col().gap(px(10.));
    let Some(d) = d.filter(|d| d["groups"].is_array()) else {
        return col.child(kit::help(t, "Loading…"));
    };
    let col = col.when_some(notes.then(|| opt_s(d, "notes").map(str::trim).filter(|n| !n.is_empty())).flatten(), |c, n| {
        c.child(div().text_size(px(14.)).text_color(t.text_2).child(n.to_string()))
    });
    let groups = arr(d, "groups");
    if groups.is_empty() {
        return col.child(kit::help(t, "No tasks"));
    }
    groups.iter().fold(col, |c, g| c.child(goal_card(t, at, g, cx)))
}

/// The ring, tick or cross in front of a task or goal a pending cut waits for.
fn mark(t: &Theme, w: Standing) -> Div {
    let ring = div().flex_none().size(px(16.)).rounded_full().flex().items_center().justify_center().text_size(px(10.)).font_weight(FontWeight::BOLD);
    match w {
        Standing::Met => ring.bg(t.up).text_color(t.on_accent).child("✓"),
        Standing::Waiting => ring.border_2().border_color(t.warn),
        Standing::Dead => ring.bg(t.down).text_color(t.on_accent).child("✕"),
    }
}

/// The pending cut as a checklist: what it waits for, each with where it stands.
fn next_cut(t: &Theme, project: &str, p: &Pending, cx: &mut Context<MainWindow>) -> Div {
    let tone = if p.running { "working" } else { "warn" };
    let head = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(16.))
        .py(px(11.))
        .bg(t.warn_soft)
        .child(div().flex_1().min_w_0().flex().items_baseline().gap(px(6.)).text_size(px(14.)).font_weight(FontWeight::BOLD).child("Next cut").child(div().min_w_0().truncate().font_family(t.mono_font.clone()).font_weight(FontWeight::MEDIUM).child(p.name.clone())))
        .child(kit::tone_pill(t, tone, p.progress()));
    let mut card = div().flex().flex_col().rounded(px(12.)).border_1().border_color(t.warn_line).bg(t.card).overflow_hidden().child(head);
    for w in &p.after {
        let (r, goal) = (w.r.clone(), w.r.starts_with('G'));
        let ref_color = match w.standing {
            Standing::Met => t.up_fg,
            Standing::Dead => t.down,
            Standing::Waiting if goal => t.goal,
            Standing::Waiting => t.accent,
        };
        let state_color = match w.standing {
            Standing::Met => t.up_fg,
            Standing::Dead => t.down,
            Standing::Waiting => t.warn_text,
        };
        let label = div().flex_none().text_size(px(12.)).text_color(state_color).child(w.label.clone());
        let label = match &w.why {
            Some(why) => label.id(SharedString::from(format!("releases-why-{project}-{r}"))).tooltip(kit::tip(why.clone())).into_any_element(),
            None => label.into_any_element(),
        };
        let link = div()
            .id(SharedString::from(format!("releases-wait-{project}-{r}")))
            .flex_none()
            .w(px(46.))
            .text_size(px(12.))
            .font_family(t.mono_font.clone())
            .text_color(ref_color)
            .cursor_pointer()
            .hover(|s| s.underline())
            .child(r.clone())
            .on_click(cx.listener(move |m, _, _, cx| if goal { goal::open(m, &r, false, cx) } else { m.open_task(r.clone(), cx) }));
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .min_h(px(40.))
                .px(px(16.))
                .py(px(6.))
                .border_t_1()
                .border_color(t.warn_line)
                .text_size(px(13.))
                .child(mark(t, w.standing))
                .child(link)
                .child(div().flex_1().min_w_0().truncate().child(w.title.clone()))
                .child(label),
        );
    }
    card
}

/// A section of the project page: Unreleased, or the newest release.
fn section(t: &Theme, line: Hsla) -> Div {
    div().flex().flex_col().gap(px(10.)).p(px(18.)).rounded(px(14.)).border_1().border_color(line).bg(t.card)
}

fn stat(t: &Theme, label: &'static str, n: i64) -> Div {
    div().flex().flex_col().gap(px(2.)).child(div().text_size(px(13.)).text_color(t.muted).child(label)).child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child(n.to_string()))
}

/// Everything above the older releases: the way back, the counts, Unreleased and the newest release.
fn top(m: &MainWindow, t: &Theme, p: &str, l: &Value, cx: &mut Context<MainWindow>) -> Div {
    let now = fmt::now();
    let counts = &l["counts"];
    let back = kit::link(t, "releases-back", "Releases").on_click(cx.listener(|m, _, _, cx| m.go(Page::Releases, cx)));
    let mut col = div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .pt(px(22.))
        .pb(px(8.))
        .child(div().flex().gap(px(6.)).text_size(px(13.)).text_color(t.muted).child(back).child("/").child(p.to_string()))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_end()
                .gap(px(24.))
                .child(div().flex_1().text_size(px(22.)).font_weight(FontWeight::BOLD).child(p.to_string()))
                .child(div().flex().gap(px(28.)).child(stat(t, "Releases", i(counts, "total"))).child(stat(t, "Last 30 days", i(counts, "recent"))).child(stat(t, "Failed", i(counts, "failed")))),
        );

    let key = unreleased_key(p);
    let pc = pending(l);
    let unreleased = m.releases.details.get(&key);
    if i(l, "unreleased") > 0 || pc.is_some() {
        let mut sec = section(t, t.border).child(
            div()
                .flex()
                .flex_wrap()
                .items_baseline()
                .gap(px(12.))
                .child(div().flex_1().text_size(px(18.)).font_weight(FontWeight::BOLD).child("Unreleased"))
                .child(div().text_size(px(13.)).text_color(t.muted).child(unreleased.map(tally).unwrap_or_else(|| fmt::plural(i(l, "unreleased"), "task", "tasks")))),
        );
        if let Some(pc) = &pc {
            sec = sec.child(next_cut(t, p, pc, cx));
        }
        if i(l, "unreleased") > 0 {
            sec = sec.child(work(t, &key, unreleased, false, cx));
        }
        col = col.child(sec);
    }

    let Some(newest) = arr(l, "releases").first() else {
        return col.when(i(l, "unreleased") == 0 && pc.is_none(), |c| c.child(kit::empty(t, "No releases yet")));
    };
    let r = row(newest, now);
    let d = m.releases.details.get(&r.key);
    let meta = [Some(r.when.clone()).filter(|w| !w.is_empty()), Some(d.map(tally).unwrap_or(r.count.clone()))].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    let pill = if r.failed { kit::tone_pill(t, "failed", "Failed") } else { kit::tone_pill(t, "up", "Latest") };
    col.child(
        section(t, if r.failed { t.down_line } else { t.accent_line })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.))
                    .child(div().text_size(px(22.)).font_family(t.mono_font.clone()).font_weight(FontWeight::MEDIUM).child(r.name.clone()))
                    .child(pill)
                    .child(div().flex_1())
                    .child(div().text_size(px(13.)).text_color(t.muted).child(meta)),
            )
            .child(work(t, &r.key, d, true, cx)),
    )
}

fn release_row(m: &MainWindow, t: &Theme, r: &Value, first: bool, last: bool, cx: &mut Context<MainWindow>) -> Div {
    let row = row(r, fmt::now());
    let open = m.releases.open.contains(&row.key);
    let key = row.key.clone();
    let head = div()
        .id(SharedString::from(format!("releases-row-{}", row.key)))
        .flex()
        .items_center()
        .gap(px(14.))
        .h(px(44.))
        .px(px(16.))
        .text_size(px(13.))
        .cursor_pointer()
        .hover({
            let bg = t.panel_2;
            move |s| s.bg(bg)
        })
        .child(div().flex_none().w(px(10.)).text_size(px(12.)).text_color(t.muted).child(if open { "▾" } else { "▸" }))
        .child(div().flex_none().w(px(96.)).truncate().text_size(px(14.)).font_family(t.mono_font.clone()).font_weight(FontWeight::MEDIUM).child(row.name.clone()))
        .child(div().flex_1().min_w_0().truncate().text_color(t.text_2).child(row.headline.clone()))
        .when(row.failed, |d| d.child(kit::tone_pill(t, "failed", "Failed")))
        .child(div().flex_none().w(px(124.)).text_right().whitespace_nowrap().text_color(t.muted).child(row.when.clone()))
        .child(div().flex_none().w(px(60.)).text_right().whitespace_nowrap().text_color(t.muted).child(row.count.clone()))
        .on_click(cx.listener(move |m, _, _, cx| {
            if !m.releases.open.remove(&key) {
                m.releases.open.insert(key.clone());
            }
            m.refresh(cx);
        }));
    let mut b = div().flex().flex_col().bg(t.card).border_l_1().border_r_1().border_color(t.border).overflow_hidden();
    b = if first { b.border_t_1().rounded_t(px(12.)) } else { b.border_t_1().border_color(t.border) };
    if last {
        b = b.border_b_1().rounded_b(px(12.));
    }
    b.child(head).when(open, |b| b.child(div().px(px(16.)).pt(px(4.)).pb(px(16.)).pl(px(40.)).child(work(t, &row.key, m.releases.details.get(&row.key), true, cx))))
}

/// One item of the project page's list.
fn item(m: &mut MainWindow, ix: usize, _window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let Some(h) = &m.releases.list else { return div().into_any_element() };
    let (p, it) = (h.project.clone(), h.items.get(ix).cloned());
    let Some(l) = m.releases.lists.get(&p).cloned() else { return div().into_any_element() };
    let wrap = |c: AnyElement| div().px(px(PAGE_X)).child(div().max_w(px(MAX_W)).child(c)).into_any_element();
    match it {
        Some(Item::Top) => wrap(top(m, &t, &p, &l, cx).pb(px(12.)).into_any_element()),
        Some(Item::Month(name)) => wrap(div().pt(px(14.)).pb(px(8.)).text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(t.muted).child(name.to_uppercase()).into_any_element()),
        Some(Item::Release { r, first, last }) => match arr(&l, "releases").iter().find(|x| s(x, "ref") == r) {
            Some(rel) => wrap(release_row(m, &t, rel, first, last, cx).into_any_element()),
            None => div().into_any_element(),
        },
        Some(Item::End) => wrap(div().pt(px(12.)).pb(px(32.)).when(l["more"] == true, |d| d.child(kit::help(&t, "Loading…"))).into_any_element()),
        None => div().into_any_element(),
    }
}

/// How an item looks, so only the ones that changed are measured again.
fn sig(st: &State, it: &Item) -> String {
    match it {
        Item::Release { r, .. } => {
            let open = st.open.contains(r);
            let n = if open { st.details.get(r).map_or(-1, |d| arr(d, "groups").len() as i64) } else { 0 };
            format!("{open}:{n}")
        }
        _ => String::new(),
    }
}

/// Keep the list's items in step with the project's releases: an older page comes in at the end, so
/// the place scrolled to stays put. The top always measures again (it changes with the data).
fn sync(st: &mut State, project: &str, items: Vec<Item>) {
    let sigs: Vec<String> = items.iter().map(|it| sig(st, it)).collect();
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

fn project_page(m: &mut MainWindow, p: &str, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let Some(l) = m.releases.lists.get(p).cloned() else {
        return div().px(px(PAGE_X)).pt(px(22.)).child(kit::empty(&t, if m.down.is_some() { "Can’t load the board." } else { "Loading…" })).into_any_element();
    };
    let items = items(&l);
    sync(&mut m.releases, p, items);
    let Some(h) = &m.releases.list else { return div().into_any_element() };
    let len = arr(&l, "releases").len();

    // A linked release: scroll to it once listed, or ask for more until it is.
    if let Some(f) = m.releases.focus.clone() {
        let at = if arr(&l, "releases").first().is_some_and(|r| s(r, "ref") == f) {
            Some(0)
        } else {
            h.items.iter().position(|it| matches!(it, Item::Release { r, .. } if *r == f))
        };
        match at {
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

fn overview_card(t: &Theme, p: &str, l: Option<&Value>, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let go = |id: String, p: String, r: Option<String>, cx: &mut Context<MainWindow>| {
        div().id(SharedString::from(id)).cursor_pointer().on_click(cx.listener(move |m, _, _, cx| match &r {
            Some(r) => open_in(m, p.clone(), r, cx),
            None => m.go(Page::ReleasesOf(p.clone()), cx),
        }))
    };
    let mut col = div().id(SharedString::from(format!("releases-card-{p}"))).flex_1().min_w_0().flex().flex_col().rounded(px(12.)).border_1().border_color(t.border).bg(t.card).overflow_hidden();
    let head = div().flex().items_center().gap(px(10.)).px(px(18.)).pt(px(16.)).child(div().flex_1().min_w_0().truncate().text_size(px(15.)).font_weight(FontWeight::BOLD).child(p.to_string()));
    let Some(l) = l else {
        return col.child(head).child(div().px(px(18.)).py(px(16.)).child(kit::help(t, "Loading…")));
    };
    let c = card(l, fmt::now());
    let total = fmt::plural(c.total, "release", "releases");
    col = col.child(head.child(kit::link(t, SharedString::from(format!("releases-all-{p}")), total).text_size(px(13.)).on_click({
        let p = p.to_string();
        cx.listener(move |m, _, _, cx| m.go(Page::ReleasesOf(p.clone()), cx))
    })));
    if let Some(r) = &c.latest {
        let meta = [r.when.clone(), r.count.clone()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
        col = col.child(
            go(format!("releases-latest-{p}"), p.to_string(), Some(r.key.clone()), cx)
                .flex()
                .flex_col()
                .gap(px(6.))
                .px(px(18.))
                .pt(px(14.))
                .pb(px(16.))
                .child(div().flex().items_center().gap(px(10.)).child(div().min_w_0().truncate().text_size(px(26.)).font_family(t.mono_font.clone()).font_weight(FontWeight::MEDIUM).child(r.name.clone())).when(r.failed, |d| d.child(kit::tone_pill(t, "failed", "Failed"))))
                .child(div().text_size(px(13.)).text_color(t.muted).child(meta)),
        );
    }
    let waiting = !c.unreleased.starts_with("0 ");
    col = col.child(
        go(format!("releases-unreleased-{p}"), p.to_string(), None, cx)
            .mx(px(18.))
            .mt(px(if c.latest.is_some() { 0. } else { 14. }))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(9.))
            .bg(if waiting { t.accent_soft } else { t.seg })
            .flex()
            .items_center()
            .gap(px(10.))
            .text_size(px(13.))
            .child(div().flex_none().font_weight(FontWeight::BOLD).child("Unreleased"))
            .child(div().flex_1().min_w_0().truncate().text_color(t.text_2).child(c.unreleased.clone()))
            .children(c.pending.clone().map(|x| kit::tone_pill(t, "warn", x))),
    );
    let mut prev = div().flex().flex_col().mt(px(10.)).pb(px(8.));
    for r in &c.previous {
        prev = prev.child(
            go(format!("releases-prev-{}", r.key), p.to_string(), Some(r.key.clone()), cx)
                .flex()
                .items_center()
                .gap(px(12.))
                .h(px(34.))
                .px(px(18.))
                .border_t_1()
                .border_color(t.divider)
                .text_size(px(13.))
                .hover({
                    let bg = t.panel_2;
                    move |s| s.bg(bg)
                })
                .child(div().flex_none().w(px(90.)).truncate().font_family(t.mono_font.clone()).child(r.name.clone()))
                .child(div().flex_1().min_w_0().truncate().text_color(t.muted).child(r.when.clone()))
                .when(r.failed, |d| d.child(kit::tone_pill(t, "failed", "Failed")))
                .child(div().flex_none().text_color(t.muted).child(r.count.clone())),
        );
    }
    col.child(prev)
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
        page = page.child(kit::empty(&t, if m.down.is_some() { "Can’t load the board." } else { "Loading…" }));
    }
    let cols = columns(window);
    for chunk in projects.chunks(cols) {
        let mut line = div().flex().items_start().gap(px(GAP));
        for p in chunk {
            line = line.child(overview_card(&t, p, m.releases.lists.get(p), cx));
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
        let list = json!({"unreleased": 3, "counts": {"total": 31}, "pending": {"name": "v1.5", "running": false, "after": []}, "releases": [
            newest,
            rel(4, "v1.3", at(2026, 10, 7, 9), 1),
            rel(3, "v1.2", at(2025, 12, 1, 9), 4),
            rel(2, "v1.1", at(2025, 11, 1, 9), 1),
        ]});
        let c = card(&list, now);
        assert_eq!(c.total, 31);
        let latest = c.latest.unwrap();
        assert_eq!((latest.name.as_str(), latest.headline.as_str(), latest.when.as_str(), latest.count.as_str()), ("v1.4", "Faster log", "Today, 3:05 PM", "2 tasks"));
        assert_eq!(c.previous.iter().map(|r| (r.name.as_str(), r.when.as_str())).collect::<Vec<_>>(), [("v1.3", "Oct 7, 9:05 AM"), ("v1.2", "Dec 1, 2025, 9:05 AM"), ("v1.1", "Nov 1, 2025, 9:05 AM")]);
        assert_eq!((c.unreleased.as_str(), c.pending.as_deref()), ("3 tasks", Some("v1.5 pending")));
        assert_eq!(card(&json!({"unreleased": 0, "releases": []}), now), Card { total: 0, latest: None, previous: vec![], unreleased: "0 tasks".into(), pending: None });
    }

    #[test]
    fn older_releases_sit_under_their_months() {
        let list = json!({"releases": [
            rel(5, "v1.4", at(2026, 10, 15, 15), 2),
            rel(4, "v1.3", at(2026, 10, 7, 9), 1),
            rel(3, "v1.2", at(2026, 10, 2, 9), 1),
            rel(2, "v1.1", at(2026, 9, 20, 9), 1),
        ]});
        let r = |r: &str, first, last| Item::Release { r: r.into(), first, last };
        assert_eq!(
            items(&list),
            [Item::Top, Item::Month("October 2026".into()), r("R4", true, false), r("R3", false, true), Item::Month("September 2026".into()), r("R2", true, true), Item::End]
        );
        assert_eq!(items(&json!({"releases": []})), [Item::Top, Item::End], "the newest sits in the top");
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
                ("G12", "Releases page", Standing::Waiting, "2 of 3"),
                ("T9", "Old", Standing::Dead, "Can't finish"),
            ]
        );
        assert_eq!(p.after[4].why.as_deref(), Some("T9 failed"));
        assert_eq!(p.progress(), "1 of 5 ready");
        assert_eq!(Pending { running: true, ..p }.progress(), "Cutting");
    }

    #[test]
    fn a_project_page_fetches_unreleased_the_newest_and_the_open_rows() {
        let mut st = State::default();
        let page = Page::ReleasesOf("web".into());
        assert_eq!(open_rows(&st, &page), ["unreleased:web"]);
        st.lists.insert("web".into(), json!({"releases": [{"ref": "R3"}, {"ref": "R2"}, {"ref": "R1"}]}));
        st.open.extend(["R1".to_string(), "R9".to_string()]);
        assert_eq!(open_rows(&st, &page), ["R1", "R3", "unreleased:web"]);
        assert_eq!(open_rows(&st, &Page::Releases), Vec::<String>::new(), "the overview fetches no rows");
        st.focus = Some("R7".into());
        assert_eq!(open_rows(&st, &Page::Releases), ["R7"], "a linked release, to learn its project");
        assert_eq!(detail_call("R2"), ("releases/R2".to_string(), vec![]));
        assert_eq!(detail_call("unreleased:web"), ("releases/unreleased".to_string(), vec![("project", "web".to_string())]));
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
}
