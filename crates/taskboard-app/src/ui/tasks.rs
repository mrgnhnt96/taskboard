//! The Tasks page (`GET /tasks`): every task, goal tasks included, in one list. A project, a status
//! and words in the title or ref narrow it; it groups by goal, status or project, each group a card
//! whose header carries its progress and what it waits on. A group opens in place, a row opens its
//! task and a goal's name opens the goal's page. The tasks come a page at a time (`ui::paged`) and
//! the page is one list of lines (the header, each group's header and rows) drawn only in view.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, b, i, opt_s, s};
use crate::theme::Theme;
use crate::ui::issue_panel as ip;
use crate::ui::kit;
use crate::ui::paged::Paged;
use crate::ui::status::{self, St};
use crate::ui::text_input::FieldChanged;
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;
use std::collections::HashSet;

const PAGE_X: f32 = 28.;
const PROJECT_FILTER: &str = "tasks-project";
/// How many tasks one fetch asks for.
pub const PAGE: usize = 200;
/// Fetch the next page once the list is scrolled within this many lines of its end.
const AHEAD: usize = 60;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroupBy {
    #[default]
    Goal,
    Status,
    Project,
}

impl GroupBy {
    pub const ALL: [GroupBy; 3] = [GroupBy::Goal, GroupBy::Status, GroupBy::Project];

    fn label(self) -> &'static str {
        match self {
            GroupBy::Goal => "By goal",
            GroupBy::Status => "By status",
            GroupBy::Project => "By project",
        }
    }
}

/// The status filter: every open task, one status, or the finished ones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Show {
    #[default]
    Open,
    Only(St),
    Done,
}

/// The kind filter: every task, or one kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    All,
    Design,
    Build,
    Human,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::All, Kind::Design, Kind::Build, Kind::Human];

    fn label(self) -> &'static str {
        match self {
            Kind::All => "All kinds",
            Kind::Design => "Design",
            Kind::Build => "Build",
            Kind::Human => "Human",
        }
    }

    /// The task card's `kind` this one picks.
    fn card_kind(self) -> Option<&'static str> {
        match self {
            Kind::All => None,
            Kind::Design => Some("design"),
            Kind::Build => Some("agent"),
            Kind::Human => Some("human"),
        }
    }
}

const SHOW: [Show; 6] = [Show::Open, Show::Only(St::Needs), Show::Only(St::Working), Show::Only(St::Queued), Show::Only(St::Review), Show::Done];

#[derive(Default)]
pub struct State {
    /// The project picked; None shows every project.
    pub project: Option<String>,
    pub show: Show,
    pub kind: Kind,
    pub group: GroupBy,
    /// The groups opened or closed against how they start, by key.
    pub toggled: HashSet<String>,
    pub search: Option<kit::Input>,
    /// The tasks fetched so far.
    pub held: Paged,
    /// The page as last drawn, rebuilt when the tasks or a filter change.
    view: Option<View>,
    list: Option<ListState>,
}

/// What the grouped view is built from.
#[derive(Clone, PartialEq)]
struct Key {
    version: u64,
    project: Option<String>,
    show: Show,
    kind: Kind,
    words: String,
    group: GroupBy,
    goals: std::rc::Rc<Vec<Value>>,
    since: DateTime<Utc>,
}

struct View {
    key: Key,
    all: Head,
    hd: Head,
    picked: Option<String>,
    groups: Vec<Group>,
    toggled: HashSet<String>,
    more: bool,
    lines: Vec<Line>,
}

/// One item of the page's list.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Line {
    /// The title, counts and filters.
    Top,
    /// "No tasks", or "Loading…" while older pages are still to come.
    Empty,
    /// A group's header: the top of its card.
    Group(usize),
    /// A group's row; the last closes the card.
    Row(usize, usize),
    End,
}

// ================================================================== view models

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: i64,
    pub r: String,
    pub title: String,
    pub project: String,
    /// "agent", "design" or "human".
    pub kind: String,
    /// (ref, name, id) of its goal.
    pub goal: Option<(String, String, i64)>,
    pub wave: Option<i64>,
    /// Still to do: not done, and not failed once finished (a lost terminal can still be restarted).
    pub open: bool,
    pub row: status::Row,
    pub finished: Option<DateTime<Utc>>,
}

/// The `GET /tasks` answer as rows.
pub fn items(v: &Value) -> Vec<Item> {
    items_of(arr(v, "tasks"))
}

fn items_of(rows: &[Value]) -> Vec<Item> {
    rows.iter()
        .map(|c| {
            let row = status::row(c);
            let open = row.st != St::Done && !(row.st == St::Failed && s(c, "status") == "done");
            let g = &c["goal"];
            Item {
                id: i(c, "id"),
                r: fmt::ref_of(c, "T"),
                title: s(c, "title").to_string(),
                project: s(c, "project").to_string(),
                kind: opt_s(c, "kind").unwrap_or("agent").to_string(),
                goal: g.is_object().then(|| (fmt::ref_of(g, "G"), s(g, "name").to_string(), i(g, "id"))),
                wave: c["wave"].as_i64(),
                open,
                row,
                finished: opt_s(c, "finished_at").and_then(fmt::parse),
            }
        })
        .collect()
}

pub struct Filter<'a> {
    pub project: Option<&'a str>,
    pub show: Show,
    pub kind: Kind,
    pub words: &'a str,
}

impl Filter<'_> {
    fn project(&self, it: &Item) -> bool {
        self.project.is_none_or(|p| it.project == p)
    }

    fn words(&self, it: &Item) -> bool {
        let w = self.words.trim().to_lowercase();
        w.is_empty() || it.title.to_lowercase().contains(&w) || it.r.to_lowercase() == w || it.r.to_lowercase().starts_with(&w)
    }

    fn show(&self, it: &Item) -> bool {
        match self.show {
            Show::Open => it.open,
            Show::Done => !it.open,
            Show::Only(st) => it.open && it.row.st == st,
        }
    }

    fn kind(&self, it: &Item) -> bool {
        self.kind.card_kind().is_none_or(|k| it.kind == k)
    }

    pub fn matches(&self, it: &Item) -> bool {
        self.project(it) && self.words(it) && self.show(it) && self.kind(it)
    }
}

/// The header's counts and the filters' choices.
#[derive(Debug, PartialEq)]
pub struct Head {
    pub open: usize,
    pub done_week: usize,
    /// Every project with a task, with how many are open, the most first.
    pub projects: Vec<(String, usize)>,
    /// The open tasks in each status, in the picked project.
    pub counts: Vec<(St, usize)>,
    /// Some task isn't a plain build task, so the kind filter has something to pick.
    pub kinds: bool,
}

/// Midnight starting this Monday, in the reader's zone.
fn week_start() -> DateTime<Utc> {
    let now = fmt::local(fmt::now());
    let monday = now.date_naive() - Duration::days(now.weekday().num_days_from_monday() as i64);
    now.timezone().from_local_datetime(&monday.and_hms_opt(0, 0, 0).unwrap_or_default()).single().map_or(fmt::now(), |t| t.with_timezone(&Utc))
}

pub fn head(items: &[Item], project: Option<&str>, since: DateTime<Utc>) -> Head {
    let mut projects: Vec<(String, usize)> = vec![];
    for it in items {
        match projects.iter_mut().find(|(p, _)| *p == it.project) {
            Some((_, n)) => *n += it.open as usize,
            None => projects.push((it.project.clone(), it.open as usize)),
        }
    }
    projects.retain(|(p, _)| !p.is_empty());
    projects.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let scope: Vec<&Item> = items.iter().filter(|it| project.is_none_or(|p| it.project == p)).collect();
    let counts = [St::Needs, St::Working, St::Queued, St::Review].map(|st| (st, scope.iter().filter(|it| it.open && it.row.st == st).count())).to_vec();
    Head {
        open: scope.iter().filter(|it| it.open).count(),
        done_week: scope.iter().filter(|it| it.row.st == St::Done && it.finished.is_some_and(|f| f >= since)).count(),
        projects,
        counts,
        kinds: items.iter().any(|it| it.kind != "agent"),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub key: String,
    /// The goal's ref ("" for the other groupings and the tasks outside goals).
    pub r: String,
    pub name: String,
    /// A goal's projects, when no project is picked ("" otherwise).
    pub project: String,
    /// The status a status group holds.
    pub st: Option<St>,
    /// "5 open".
    pub count: String,
    pub bar: Vec<(St, i64)>,
    /// What the group is doing or waiting on, in a status's color or muted (None).
    pub state: Option<(String, Option<St>)>,
    pub rows: Vec<Item>,
    /// Open to start with: something in it needs you, failed, runs or is in review.
    pub open: bool,
}

/// The order a progress bar runs in: finished work first.
const BAR: [St; 7] = [St::Done, St::Review, St::Working, St::Needs, St::Failed, St::Queued, St::Planned];

/// A goal or project's state line, and how urgent it is (lower first).
fn state_of(tasks: &[&Item], goal: Option<&Value>) -> (Option<(String, Option<St>)>, u8) {
    let n = |st: St| tasks.iter().filter(|it| it.open && it.row.st == st).count() as i64;
    let flag = |k: &str| goal.is_some_and(|g| b(g, k));
    let after = goal.filter(|g| opt_s(g, "after_blocker").is_some()).and_then(|g| arr(g, "after").first().and_then(Value::as_str).map(String::from));
    let stopped = goal.is_some_and(|g| opt_s(g, "stopped").is_some());
    let (line, st, rank): (String, Option<St>, u8) = if n(St::Needs) > 0 {
        (format!("{} you", fmt::plural(n(St::Needs), "needs", "need")), Some(St::Needs), 0)
    } else if stopped {
        ("Stopped for your review".into(), Some(St::Needs), 0)
    } else if n(St::Failed) > 0 {
        (format!("{} failed", n(St::Failed)), Some(St::Failed), 1)
    } else if n(St::Review) > 0 {
        (format!("{} in review", n(St::Review)), Some(St::Review), 2)
    } else if n(St::Working) > 0 {
        (format!("{} working", n(St::Working)), Some(St::Working), 3)
    } else if flag("paused") {
        ("Paused".into(), None, 4)
    } else if let Some(after) = after {
        (format!("After {after}"), None, 5)
    } else if n(St::Queued) > 0 {
        (format!("{} queued", n(St::Queued)), Some(St::Queued), 6)
    } else if n(St::Planned) > 0 {
        (format!("{} planned", n(St::Planned)), None, 7)
    } else if !tasks.is_empty() && tasks.iter().all(|it| it.row.st == St::Done) {
        ("Done".into(), Some(St::Done), 8)
    } else {
        return (None, 9);
    };
    (Some((line, st)), rank)
}

/// The rows grouped. A group shows when a row in it passes the filter; its header counts every
/// task in it within the picked project.
pub fn groups(items: &[Item], goals: &[Value], by: GroupBy, f: &Filter) -> Vec<Group> {
    let mut buckets: Vec<(String, Vec<&Item>)> = vec![];
    for it in items.iter().filter(|it| f.project(it)) {
        let key = match by {
            GroupBy::Goal => it.goal.as_ref().map_or("none".to_string(), |g| g.0.clone()),
            GroupBy::Status => it.row.st.label().to_string(),
            GroupBy::Project => it.project.clone(),
        };
        match buckets.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(it),
            None => buckets.push((key, vec![it])),
        }
    }
    let mut out: Vec<((u8, u8, i64, String), Group)> = vec![];
    for (key, tasks) in buckets {
        let mut rows: Vec<Item> = tasks.iter().filter(|it| f.matches(it)).map(|it| (*it).clone()).collect();
        if rows.is_empty() {
            continue;
        }
        rows.sort_by_key(|it| (it.row.st.rank(), it.wave.unwrap_or(i64::MAX), -it.id));
        let first = tasks[0];
        let open_n = tasks.iter().filter(|it| it.open).count() as i64;
        let count = if open_n > 0 || by == GroupBy::Status { format!("{} open", open_n) } else { format!("{} done", tasks.len()) };
        let bar = BAR.iter().map(|st| (*st, tasks.iter().filter(|it| it.row.st == *st).count() as i64)).collect();
        let busy = rows.iter().any(|it| it.open && matches!(it.row.st, St::Needs | St::Failed | St::Working | St::Review));
        let (r, name, gid) = match by {
            GroupBy::Goal => first.goal.clone().unwrap_or_else(|| (String::new(), "Not in a goal".into(), i64::MAX)),
            GroupBy::Status => (String::new(), key.clone(), 0),
            GroupBy::Project => (String::new(), if key.is_empty() { "No project".into() } else { key.clone() }, 0),
        };
        let goal = goals.iter().find(|g| !r.is_empty() && fmt::ref_of(g, "G") == r);
        let mut projects: Vec<&str> = vec![];
        if by == GroupBy::Goal && f.project.is_none() {
            for it in &tasks {
                if !it.project.is_empty() && !projects.contains(&it.project.as_str()) {
                    projects.push(&it.project);
                }
            }
        }
        let (state, urgency) = if by == GroupBy::Status { (None, first.row.st.rank()) } else { state_of(&tasks, goal) };
        let sort = match by {
            GroupBy::Goal => ((r.is_empty()) as u8, urgency, gid, String::new()),
            GroupBy::Status => (0, urgency, 0, String::new()),
            GroupBy::Project => (0, urgency, 0, name.clone()),
        };
        out.push((
            sort,
            Group {
                key: format!("{}-{key}", by.label()),
                r,
                name,
                project: projects.join(", "),
                st: (by == GroupBy::Status).then_some(first.row.st),
                count: if by == GroupBy::Status { fmt::plural(rows.len() as i64, "task", "tasks") } else { count },
                bar: if by == GroupBy::Status { vec![] } else { bar },
                state,
                rows,
                open: busy,
            },
        ));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out: Vec<Group> = out.into_iter().map(|(_, g)| g).collect();
    // One group, or a search: every row shows.
    if out.len() == 1 || !f.words.trim().is_empty() {
        for g in &mut out {
            g.open = true;
        }
    }
    out
}

// ================================================================== render

fn task_row(t: &Theme, it: &Item, col: String, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let r = it.r.clone();
    let hover = t.panel_2;
    let (fg, _, _) = it.row.st.colors(t);
    div()
        .id(SharedString::from(format!("tasks-row-{}", it.r)))
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(34.))
        .pl(px(54.))
        .pr(px(16.))
        .border_t_1()
        .border_color(t.divider)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .on_click(cx.listener(move |m, _, _, cx| m.open_task(r.clone(), cx)))
        .child(div().flex_none().w(px(86.)).child(status::chip(t, it.row.st)))
        .child(div().flex_none().w(px(36.)).font_family(t.mono_font.clone()).text_size(px(11.)).text_color(t.faint).child(it.r.clone()))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().min_w_0().truncate().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(it.title.clone()))
                .when(it.kind == "design", |d| d.child(kit::design_pill(t))),
        )
        .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(if it.row.loud { fg } else { t.muted }).child(it.row.line.clone()))
        .child(div().flex_none().w(px(60.)).truncate().text_size(px(12.)).text_color(t.muted).child(col))
        .child(div().flex_none().w(px(96.)).whitespace_nowrap().text_right().text_size(px(12.)).text_color(t.muted).child(it.row.age.clone()))
}

fn group_header(t: &Theme, g: &Group, open: bool, by: GroupBy, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let key = g.key.clone();
    let mut title = div().flex().flex_1().min_w_0().items_center().gap(px(8.));
    if let Some(st) = g.st {
        title = title.child(div().flex_none().child(kit::dot(st.colors(t).2, 8.)));
    }
    if !g.r.is_empty() {
        title = title.child(div().flex_none().font_family(t.mono_font.clone()).text_size(px(11.)).text_color(t.muted).child(g.r.clone()));
    }
    let name = div().min_w_0().truncate().text_size(px(15.)).font_weight(FontWeight::BOLD).child(g.name.clone());
    title = if g.r.is_empty() {
        title.child(name)
    } else {
        let r = g.r.clone();
        title.child(
            name.id(SharedString::from(format!("tasks-goal-{}", g.r))).cursor_pointer().hover(|s| s.underline()).on_click(cx.listener(move |m, _, _, cx| {
                cx.stop_propagation();
                m.go(Page::Goal(r.clone()), cx);
            })),
        )
    };
    if !g.project.is_empty() {
        title = title.child(div().flex_none().text_size(px(12.)).text_color(t.muted).child(g.project.clone()));
    }
    title = title.child(div().flex_none().text_size(px(12.)).text_color(t.muted).child(g.count.clone()));
    let mut header = div()
        .id(SharedString::from(format!("tasks-group-{}", g.key)))
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(48.))
        .px(px(16.))
        .cursor_pointer()
        .on_click(cx.listener(move |m, _, _, cx| {
            if !m.tasks.toggled.remove(&key) {
                m.tasks.toggled.insert(key.clone());
            }
            cx.notify();
        }))
        .child(div().flex_none().w(px(20.)).flex().justify_center().child(kit::icon(if open { kit::Icon::Chev } else { kit::Icon::ChevRight }, 14., t.muted)))
        .child(title);
    if !g.bar.is_empty() {
        header = header.child(div().flex_none().w(px(180.)).child(status::bar(t, &g.bar, 6.)));
    }
    if by != GroupBy::Status {
        let (text, color) = match &g.state {
            Some((line, Some(st))) => (line.clone(), st.colors(t).0),
            Some((line, None)) => (line.clone(), t.muted),
            None => (String::new(), t.muted),
        };
        header = header.child(div().flex_none().w(px(150.)).text_right().truncate().text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(color).child(text));
    }
    header
}

/// Bring the grouped view and the list's lines up to date, rebuilding only what changed.
fn sync(m: &mut MainWindow, words: &str) {
    let s = &mut m.tasks;
    let key = Key {
        version: s.held.version,
        project: s.project.clone(),
        show: s.show,
        kind: s.kind,
        words: words.to_string(),
        group: s.group,
        goals: m.data.goals.clone(),
        since: week_start(),
    };
    let more = s.held.next.is_some();
    let old = s.view.take();
    let filters_same = old.as_ref().is_some_and(|v| (&v.key.project, v.key.show, v.key.kind, &v.key.words, v.key.group) == (&key.project, key.show, key.kind, &key.words, key.group));
    let mut v = match old {
        Some(v) if v.key == key => v,
        _ => {
            let items = items_of(&s.held.rows);
            let all = head(&items, None, key.since);
            // A picked project with no tasks left falls back to every project.
            let picked = key.project.clone().filter(|p| all.projects.iter().any(|(n, _)| n == p));
            let hd = head(&items, picked.as_deref(), key.since);
            let f = Filter { project: picked.as_deref(), show: key.show, kind: key.kind, words };
            let groups = groups(&items, &m.data.goals, key.group, &f);
            View { key, all, hd, picked, groups, toggled: HashSet::new(), more, lines: vec![] }
        }
    };
    let mut lines = vec![Line::Top];
    if v.groups.is_empty() {
        lines.push(Line::Empty);
    }
    for (gi, g) in v.groups.iter().enumerate() {
        lines.push(Line::Group(gi));
        if g.open != s.toggled.contains(&g.key) {
            lines.extend((0..g.rows.len()).map(|ri| Line::Row(gi, ri)));
        }
    }
    lines.push(Line::End);
    let changed = lines != v.lines || v.more != more;
    let count = lines.len();
    match &s.list {
        None => s.list = Some(ListState::new(count, ListAlignment::Top, px(600.))),
        Some(l) if changed || !filters_same => {
            let top = l.logical_scroll_top();
            l.reset(count);
            // New tasks or a group opened keep the place scrolled to; another filter starts at the top.
            if filters_same {
                l.scroll_to(ListOffset { item_ix: top.item_ix.min(count - 1), offset_in_item: top.offset_in_item });
            }
        }
        // The header's counts can change with the lines staying put.
        Some(l) => l.remeasure_items(0..1),
    }
    v.lines = lines;
    v.more = more;
    v.toggled = s.toggled.clone();
    s.view = Some(v);
}

/// Fetch the next older page once the list nears its end (a short list is always near it).
fn load_older(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let s = &mut m.tasks;
    let (Some(before), Some(l), Some(v)) = (s.held.next.clone(), &s.list, &s.view) else { return };
    if s.held.loading || v.lines.len() > l.logical_scroll_top().item_ix + AHEAD {
        return;
    }
    s.held.loading = true;
    let backend = m.backend.clone();
    let query = vec![("limit", PAGE.to_string()), ("before", before.clone())];
    cx.spawn(async move |this, cx| {
        let got = cx.background_executor().spawn(async move { backend.get("tasks", &query).map_err(|e| e.message) }).await;
        let _ = this.update(cx, |m, cx| {
            m.tasks.held.land_older("tasks", &before, got);
            cx.notify();
        });
    })
    .detach();
}

/// The title, its counts and the filters: the list's first line.
fn top(m: &MainWindow, t: &Theme, v: &View, window: &mut Window, cx: &mut Context<MainWindow>) -> Div {
    let search = m.tasks.search.as_ref().map(|x| {
        div()
            .relative()
            .flex_none()
            .w(px(340.))
            .child(x.render(t, "tasks-search", window).pl(px(32.)))
            .child(div().absolute().left(px(11.)).top_0().bottom_0().flex().items_center().child(kit::glyph(kit::Glyph::Search, 14., t.muted)))
    });
    let hd = &v.hd;
    let head_row = div()
        .flex()
        .items_center()
        .gap(px(14.))
        .child(div().flex_none().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Tasks"))
        .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).text_color(t.muted).child(format!("{} open · {} done this week", hd.open, hd.done_week)))
        .children(search);

    let projects = ip::project_filter(t, PROJECT_FILTER, v.picked.as_deref().unwrap_or("all"), &v.all.projects, cx, |m, p, cx| {
        m.tasks.project = (p != "all").then_some(p);
        cx.notify();
    });

    let show_labels: Vec<String> = SHOW
        .iter()
        .map(|sh| match sh {
            Show::Open => "Open".to_string(),
            Show::Done => "Done".to_string(),
            Show::Only(st) => format!("{} · {}", st.label(), hd.counts.iter().find(|(c, _)| c == st).map_or(0, |(_, n)| *n)),
        })
        .collect();
    let refs: Vec<&str> = show_labels.iter().map(String::as_str).collect();
    let on = SHOW.iter().position(|sh| *sh == m.tasks.show).unwrap_or(0);
    let shows = kit::seg(t, "tasks-show", &refs, on, |i, item| {
        item.on_click(cx.listener(move |m, _, _, cx| {
            m.tasks.show = SHOW[i];
            cx.notify();
        }))
    });

    let refs: Vec<&str> = GroupBy::ALL.iter().map(|g| g.label()).collect();
    let on = GroupBy::ALL.iter().position(|g| *g == m.tasks.group).unwrap_or(0);
    let group_by = kit::seg(t, "tasks-group-by", &refs, on, |i, item| {
        item.on_click(cx.listener(move |m, _, _, cx| {
            m.tasks.group = GroupBy::ALL[i];
            cx.notify();
        }))
    });
    // Only once there's a kind to pick, or one is picked.
    let kinds = (v.all.kinds || m.tasks.kind != Kind::All).then(|| {
        let refs: Vec<&str> = Kind::ALL.iter().map(|k| k.label()).collect();
        let on = Kind::ALL.iter().position(|k| *k == m.tasks.kind).unwrap_or(0);
        kit::seg(t, "tasks-kind", &refs, on, |i, item| {
            item.on_click(cx.listener(move |m, _, _, cx| {
                m.tasks.kind = Kind::ALL[i];
                cx.notify();
            }))
        })
    });
    let filters = div().flex().flex_wrap().items_center().gap(px(10.)).child(projects).child(shows).children(kinds).child(div().ml_auto().child(group_by));
    div().flex().flex_col().gap(px(16.)).pt(px(22.)).pb(px(16.)).child(head_row).child(filters)
}

/// One line of the page's list. A group's card is drawn in pieces: its header carries the top
/// edge and each row the sides, the last row (or a closed header) the bottom.
fn line_item(m: &mut MainWindow, ix: usize, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let Some(v) = m.tasks.view.take() else { return div().into_any_element() };
    let line = div().px(px(PAGE_X)).line_height(relative(1.4));
    let by = v.key.group;
    let el = match v.lines.get(ix).copied() {
        Some(Line::Top) => line.child(top(m, &t, &v, window, cx)),
        Some(Line::Empty) => line.child(kit::empty(&t, if v.more { "Loading…" } else { "No tasks" })),
        Some(Line::Group(gi)) => {
            let g = &v.groups[gi];
            let open = g.open != v.toggled.contains(&g.key);
            let mut card = div().flex().flex_col().bg(t.card).border_color(t.border).border_t_1().border_x_1().rounded_t(px(12.));
            if !open || g.rows.is_empty() {
                card = card.border_b_1().rounded_b(px(12.));
            }
            line.when(gi > 0, |d| d.pt(px(8.))).child(card.child(group_header(&t, g, open, by, cx)))
        }
        Some(Line::Row(gi, ri)) => {
            let g = &v.groups[gi];
            let it = &g.rows[ri];
            let col = match by {
                GroupBy::Goal => it.wave.map(|w| format!("Wave {w}")).unwrap_or_default(),
                _ => it.goal.as_ref().map(|g| g.0.clone()).unwrap_or_default(),
            };
            let mut card = div().bg(t.card).border_color(t.border).border_x_1();
            if ri + 1 == g.rows.len() {
                card = card.border_b_1().rounded_b(px(12.)).overflow_hidden();
            }
            line.child(card.child(task_row(&t, it, col, cx)))
        }
        Some(Line::End) | None => line.h(px(32.)),
    };
    m.tasks.view = Some(v);
    el.into_any_element()
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    if m.tasks.search.is_none() {
        let input = kit::Input::new(cx, "Filter by title or ref", false);
        cx.subscribe(&input.field, |_, _, _: &FieldChanged, cx| cx.notify()).detach();
        m.tasks.search = Some(input);
    }
    let words = m.tasks.search.as_ref().map(|x| x.text(cx)).unwrap_or_default();
    let shell = div().id("tasks-page").relative().flex().flex_col().flex_1().min_w_0().h_full();

    if !m.tasks.held.loaded {
        let msg = if m.down.is_some() { "Can’t load the board." } else { "Loading…" };
        let head = div().flex().items_center().gap(px(16.)).child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Tasks"));
        return shell.child(div().flex().flex_col().gap(px(18.)).px(px(PAGE_X)).pt(px(22.)).pb(px(32.)).child(head).child(kit::empty(&t, msg))).into_any_element();
    }
    sync(m, &words);
    load_older(m, cx);
    let Some(state) = m.tasks.list.clone() else { return shell.into_any_element() };
    let filter = ip::render_project_filter(m, &t, PROJECT_FILTER, window, cx);
    shell.child(list(state, cx.processor(line_item)).flex_1().min_h_0()).children(filter).into_any_element()
}

/// The home page's "N queued" on a project's card: the Tasks page, narrowed to that project.
pub fn open_for(m: &mut MainWindow, project: &str, cx: &mut Context<MainWindow>) {
    m.tasks.project = Some(project.to_string());
    m.tasks.show = Show::Open;
    m.tasks.kind = Kind::All;
    m.tasks.toggled.clear();
    m.go(Page::Tasks, cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    // Beats the glob's `gpui_kit::test`.
    use ::core::prelude::v1::test;
    use serde_json::json;

    fn task(id: i64, project: &str, status: &str, goal: Option<i64>, extra: Value) -> Value {
        let mut c = json!({"id": id, "ref": format!("T{id}"), "title": format!("Task {id}"), "project": project, "status": status});
        if let Some(g) = goal {
            c["goal"] = json!({"id": g, "ref": format!("G{g}"), "name": format!("Goal {g}")});
        }
        for (k, v) in extra.as_object().unwrap() {
            c[k] = v.clone();
        }
        c
    }

    fn board() -> Vec<Item> {
        items(&json!({"tasks": [
            task(1, "web", "done", Some(1), json!({"wave": 1, "finished_at": "2026-10-13T15:00:00Z"})),
            task(2, "web", "needs", Some(1), json!({"wave": 2, "needs_reason": "question", "question": "10 or 30?"})),
            task(3, "web", "queued", Some(1), json!({"wave": 2})),
            task(4, "api", "working", Some(2), json!({"wave": 1, "latest": "Running tests"})),
            task(5, "api", "queued", None, json!({})),
            task(6, "api", "done", None, json!({"failed": true})),
            task(7, "web", "queued", Some(3), json!({})),
        ]}))
    }

    fn refs(g: &Group) -> Vec<&str> {
        g.rows.iter().map(|it| it.r.as_str()).collect()
    }

    #[test]
    fn open_leaves_out_done_and_finished_failures() {
        let it = board();
        let open: Vec<&str> = it.iter().filter(|it| it.open).map(|it| it.r.as_str()).collect();
        assert_eq!(open, ["T2", "T3", "T4", "T5", "T7"]);
        let lost = &items(&json!({"tasks": [task(8, "web", "needs", None, json!({"lost": true}))]}))[0];
        assert_eq!((lost.row.st, lost.open), (St::Failed, true), "a lost terminal can still be restarted");
    }

    #[test]
    fn grouped_by_goal_most_urgent_first_outside_goals_last() {
        let it = board();
        let goals = [json!({"ref": "G3", "paused": true})];
        let f = Filter { project: None, show: Show::Open, kind: Kind::All, words: "" };
        let gs = groups(&it, &goals, GroupBy::Goal, &f);
        let got: Vec<(&str, &str, Option<&str>)> = gs.iter().map(|g| (g.r.as_str(), g.name.as_str(), g.state.as_ref().map(|s| s.0.as_str()))).collect();
        assert_eq!(got, [("G1", "Goal 1", Some("1 needs you")), ("G2", "Goal 2", Some("1 working")), ("G3", "Goal 3", Some("Paused")), ("", "Not in a goal", Some("1 queued"))]);
        // The header counts the whole goal; the rows are only the open ones.
        assert_eq!((gs[0].count.as_str(), refs(&gs[0])), ("2 open", vec!["T2", "T3"]));
        assert_eq!(gs[0].bar.iter().filter(|(_, n)| *n > 0).copied().collect::<Vec<_>>(), [(St::Done, 1), (St::Needs, 1), (St::Queued, 1)]);
        // A group that needs you or runs starts open; a paused or queued one starts closed.
        assert_eq!(gs.iter().map(|g| g.open).collect::<Vec<_>>(), [true, true, false, false]);
        // A goal names its project; a picked project already says it.
        assert_eq!(gs.iter().map(|g| g.project.as_str()).collect::<Vec<_>>(), ["web", "api", "web", "api"]);
        let picked = groups(&it, &goals, GroupBy::Goal, &Filter { project: Some("web"), show: Show::Open, kind: Kind::All, words: "" });
        assert!(picked.iter().all(|g| g.project.is_empty()));
    }

    #[test]
    fn filters_narrow_by_project_status_and_words() {
        let it = board();
        let gs = groups(&it, &[], GroupBy::Status, &Filter { project: Some("api"), show: Show::Open, kind: Kind::All, words: "" });
        assert_eq!(gs.iter().map(|g| (g.name.as_str(), refs(g))).collect::<Vec<_>>(), [("Working", vec!["T4"]), ("Queued", vec!["T5"])]);
        let done = groups(&it, &[], GroupBy::Status, &Filter { project: None, show: Show::Done, kind: Kind::All, words: "" });
        assert_eq!(done.iter().map(|g| (g.name.as_str(), refs(g))).collect::<Vec<_>>(), [("Failed", vec!["T6"]), ("Done", vec!["T1"])]);
        let queued = groups(&it, &[], GroupBy::Project, &Filter { project: None, show: Show::Only(St::Queued), kind: Kind::All, words: "" });
        assert_eq!(queued.iter().map(|g| (g.name.as_str(), refs(g))).collect::<Vec<_>>(), [("web", vec!["T3", "T7"]), ("api", vec!["T5"])]);
        // Words match the title or the start of the ref, and open every group.
        let found = groups(&it, &[], GroupBy::Goal, &Filter { project: None, show: Show::Open, kind: Kind::All, words: "t7" });
        assert_eq!((found.len(), refs(&found[0]), found[0].open), (1, vec!["T7"], true));
        assert_eq!(groups(&it, &[], GroupBy::Goal, &Filter { project: None, show: Show::Open, kind: Kind::All, words: "task 4" })[0].r, "G2");
    }

    #[test]
    fn header_counts_open_tasks_and_this_weeks_done() {
        let it = board();
        let since = fmt::parse("2026-10-12T00:00:00Z").unwrap();
        let all = head(&it, None, since);
        assert_eq!((all.open, all.done_week), (5, 1));
        assert_eq!(all.projects, [("web".to_string(), 3), ("api".to_string(), 2)]);
        let api = head(&it, Some("api"), since);
        assert_eq!((api.open, api.done_week), (2, 0));
        assert_eq!(api.counts, [(St::Needs, 0), (St::Working, 1), (St::Queued, 1), (St::Review, 0)]);
    }

    #[test]
    fn the_kind_filter_narrows_and_shows_only_once_a_task_has_a_kind() {
        let since = fmt::parse("2026-10-12T00:00:00Z").unwrap();
        assert!(!head(&board(), None, since).kinds, "all agent tasks: no kind filter");
        let it = items(&json!({"tasks": [
            task(1, "web", "queued", Some(1), json!({"kind": "design"})),
            task(2, "web", "queued", Some(1), json!({"kind": "agent"})),
            task(3, "web", "needs", None, json!({"kind": "human"})),
            task(4, "web", "queued", None, json!({})),
        ]}));
        assert!(head(&it, None, since).kinds);
        let only = |k: Kind| -> Vec<String> {
            groups(&it, &[], GroupBy::Project, &Filter { project: None, show: Show::Open, kind: k, words: "" }).iter().flat_map(|g| refs(g).into_iter().map(String::from).collect::<Vec<_>>()).collect()
        };
        assert_eq!(only(Kind::Design), ["T1"]);
        assert_eq!(only(Kind::Build), ["T4", "T2"]);
        assert_eq!(only(Kind::Human), ["T3"]);
        assert_eq!(only(Kind::All).len(), 4);
    }

    #[test]
    fn a_goal_after_another_or_stopped_says_so() {
        let it = items(&json!({"tasks": [task(1, "web", "queued", Some(4), json!({})), task(2, "web", "queued", Some(5), json!({}))]}));
        let goals = [json!({"ref": "G4", "after": ["G9"], "after_blocker": "G9 isn't done"}), json!({"ref": "G5", "stopped": "Wave 1 stopped for review"})];
        let gs = groups(&it, &goals, GroupBy::Goal, &Filter { project: None, show: Show::Open, kind: Kind::All, words: "" });
        let got: Vec<(&str, Option<&(String, Option<St>)>)> = gs.iter().map(|g| (g.r.as_str(), g.state.as_ref())).collect();
        assert_eq!(got, [("G5", Some(&("Stopped for your review".to_string(), Some(St::Needs)))), ("G4", Some(&("After G9".to_string(), None)))]);
    }

    #[gpui_kit::test]
    fn a_long_task_list_fetches_older_pages_as_it_scrolls(cx: &mut gpui_kit::TestAppContext) {
        use crate::backend::Backend;
        use gpui_kit::AppContext as _;
        let (w, rec) = crate::parity::window(cx);
        let project = rec.board().get("tasks", &[]).unwrap()["tasks"][0]["project"].as_str().unwrap().to_string();
        for k in 0..450 {
            rec.board().post("tasks", json!({"title": format!("Paged {k}"), "project": project, "status": "planned"})).unwrap();
        }
        let draw = |cx: &mut gpui_kit::TestAppContext| {
            let _ = cx.update_window(w.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
            crate::parity::settle(cx);
        };
        w.update(cx, |m, _, cx| m.go(Page::Tasks, cx)).unwrap();
        crate::parity::settle(cx);
        draw(cx);
        let held = |cx: &mut gpui_kit::TestAppContext| w.update(cx, |m, _, _| (m.tasks.held.rows.len(), m.tasks.held.next.is_some(), m.tasks.held.total)).unwrap();
        let (n, more, total) = held(cx);
        assert_eq!((n, more), (PAGE, true), "only the first page until the list nears its end");
        assert!(total > 450, "the header counts every task: {total}");

        for _ in 0..5 {
            w.update(cx, |m, _, _| {
                let n = m.tasks.view.as_ref().unwrap().lines.len();
                m.tasks.list.as_ref().unwrap().scroll_to(ListOffset { item_ix: n, offset_in_item: px(0.) });
            })
            .unwrap();
            draw(cx);
        }
        assert_eq!(held(cx), (total, false, total), "scrolling to the end pages back through the rest");
        let ids: HashSet<i64> = w.update(cx, |m, _, _| m.tasks.held.rows.iter().map(|t| t["id"].as_i64().unwrap()).collect()).unwrap();
        assert_eq!(ids.len(), total, "each task once");
        // Idle groups start closed; open them all and every task gets its line.
        w.update(cx, |m, _, _| m.tasks.toggled = m.tasks.view.as_ref().unwrap().groups.iter().filter(|g| !g.open).map(|g| g.key.clone()).collect()).unwrap();
        draw(cx);
        let rows = w.update(cx, |m, _, _| m.tasks.view.as_ref().unwrap().lines.iter().filter(|l| matches!(l, Line::Row(..))).count()).unwrap();
        assert!(rows >= 450, "every planned task has a line: {rows}");
    }
}
