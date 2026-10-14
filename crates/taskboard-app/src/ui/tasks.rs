//! The Tasks page (`GET /tasks`): every task, goal tasks included, in one list. A project, a status
//! and words in the title or ref narrow it; it groups by goal, status or project, each group a card
//! whose header carries its progress and what it waits on. A group opens in place, a row opens its
//! task and a goal's name opens the goal's page.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, b, i, opt_s, s};
use crate::theme::Theme;
use crate::ui::kit;
use crate::ui::status::{self, St};
use crate::ui::text_input::FieldChanged;
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;
use std::collections::HashSet;

const PAGE_X: f32 = 28.;

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

const SHOW: [Show; 6] = [Show::Open, Show::Only(St::Needs), Show::Only(St::Working), Show::Only(St::Queued), Show::Only(St::Review), Show::Done];

#[derive(Default)]
pub struct State {
    /// The project picked; None shows every project.
    pub project: Option<String>,
    pub show: Show,
    pub group: GroupBy,
    /// The groups opened or closed against how they start, by key.
    pub toggled: HashSet<String>,
    pub search: Option<kit::Input>,
}

// ================================================================== view models

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: i64,
    pub r: String,
    pub title: String,
    pub project: String,
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
    arr(v, "tasks")
        .iter()
        .map(|c| {
            let row = status::row(c);
            let open = row.st != St::Done && !(row.st == St::Failed && s(c, "status") == "done");
            let g = &c["goal"];
            Item {
                id: i(c, "id"),
                r: fmt::ref_of(c, "T"),
                title: s(c, "title").to_string(),
                project: s(c, "project").to_string(),
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

    pub fn matches(&self, it: &Item) -> bool {
        self.project(it) && self.words(it) && self.show(it)
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
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub key: String,
    /// The goal's ref ("" for the other groupings and the tasks outside goals).
    pub r: String,
    pub name: String,
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
        .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(it.title.clone()))
        .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(if it.row.loud { fg } else { t.muted }).child(it.row.line.clone()))
        .child(div().flex_none().w(px(60.)).truncate().text_size(px(12.)).text_color(t.muted).child(col))
        .child(div().flex_none().w(px(96.)).whitespace_nowrap().text_right().text_size(px(12.)).text_color(t.muted).child(it.row.age.clone()))
}

fn group_card(t: &Theme, g: &Group, open: bool, by: GroupBy, cx: &mut Context<MainWindow>) -> Div {
    let key = g.key.clone();
    let mut title = div().flex().flex_1().min_w_0().items_baseline().gap(px(8.));
    if let Some(st) = g.st {
        title = title.child(div().flex_none().self_center().child(kit::dot(st.colors(t).2, 8.)));
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
    let mut card = kit::card(t).rounded(px(12.)).overflow_hidden().child(header);
    if open {
        for it in &g.rows {
            let col = match by {
                GroupBy::Goal => it.wave.map(|w| format!("Wave {w}")).unwrap_or_default(),
                _ => it.goal.as_ref().map(|g| g.0.clone()).unwrap_or_default(),
            };
            card = card.child(task_row(t, it, col, cx));
        }
    }
    card
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    if m.tasks.search.is_none() {
        let input = kit::Input::new(cx, "Filter by title or ref", false);
        cx.subscribe(&input.field, |_, _, _: &FieldChanged, cx| cx.notify()).detach();
        m.tasks.search = Some(input);
    }
    let words = m.tasks.search.as_ref().map(|x| x.text(cx)).unwrap_or_default();
    let shell = div().id("tasks-page").flex_1().min_w_0().h_full().overflow_y_scroll().restrict_scroll_to_axis();
    let search = m.tasks.search.as_ref().map(|x| {
        div()
            .relative()
            .flex_none()
            .w(px(340.))
            .child(x.render(&t, "tasks-search", window).pl(px(32.)))
            .child(div().absolute().left(px(11.)).top_0().bottom_0().flex().items_center().child(kit::glyph(kit::Glyph::Search, 14., t.muted)))
    });

    let Some(v) = m.data.tasks.clone() else {
        let msg = if m.down.is_some() { "Can’t load the board." } else { "Loading…" };
        let head = div().flex().items_center().gap(px(16.)).child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Tasks"));
        return shell.child(div().flex().flex_col().gap(px(18.)).px(px(PAGE_X)).pt(px(22.)).pb(px(32.)).child(head).child(kit::empty(&t, msg))).into_any_element();
    };
    let items = items(&v);
    let all = head(&items, None, week_start());
    // A picked project with no tasks left falls back to every project.
    let picked = m.tasks.project.clone().filter(|p| all.projects.iter().any(|(n, _)| n == p));
    let hd = head(&items, picked.as_deref(), week_start());

    let head_row = div()
        .flex()
        .items_center()
        .gap(px(14.))
        .child(div().flex_none().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Tasks"))
        .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).text_color(t.muted).child(format!("{} open · {} done this week", hd.open, hd.done_week)))
        .children(search);

    let names: Vec<String> = all.projects.iter().map(|(p, _)| p.clone()).collect();
    let labels: Vec<String> = ["All projects".to_string()].into_iter().chain(all.projects.iter().map(|(p, n)| format!("{p} · {n}"))).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let on = picked.as_ref().and_then(|p| names.iter().position(|n| n == p)).map_or(0, |i| i + 1);
    let projects = kit::seg(&t, "tasks-project", &refs, on, |i, item| {
        let pick = if i == 0 { None } else { names.get(i - 1).cloned() };
        item.on_click(cx.listener(move |m, _, _, cx| {
            m.tasks.project = pick.clone();
            cx.notify();
        }))
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
    let shows = kit::seg(&t, "tasks-show", &refs, on, |i, item| {
        item.on_click(cx.listener(move |m, _, _, cx| {
            m.tasks.show = SHOW[i];
            cx.notify();
        }))
    });

    let refs: Vec<&str> = GroupBy::ALL.iter().map(|g| g.label()).collect();
    let on = GroupBy::ALL.iter().position(|g| *g == m.tasks.group).unwrap_or(0);
    let group_by = kit::seg(&t, "tasks-group-by", &refs, on, |i, item| {
        item.on_click(cx.listener(move |m, _, _, cx| {
            m.tasks.group = GroupBy::ALL[i];
            cx.notify();
        }))
    });
    let filters = div().flex().flex_wrap().items_center().gap(px(10.)).child(projects).child(shows).child(div().ml_auto().child(group_by));

    let by = m.tasks.group;
    let f = Filter { project: picked.as_deref(), show: m.tasks.show, words: &words };
    let gs = groups(&items, &m.data.goals, by, &f);
    let mut list = div().flex().flex_col().gap(px(8.));
    if gs.is_empty() {
        list = list.child(kit::empty(&t, "No tasks"));
    }
    for g in &gs {
        let open = g.open != m.tasks.toggled.contains(&g.key);
        list = list.child(group_card(&t, g, open, by, cx));
    }
    let page = div().flex().flex_col().gap(px(16.)).px(px(PAGE_X)).pt(px(22.)).pb(px(32.)).line_height(relative(1.4)).child(head_row).child(filters).child(list);
    shell.child(page).into_any_element()
}

/// The home page's "N queued" on a project's card: the Tasks page, narrowed to that project.
pub fn open_for(m: &mut MainWindow, project: &str, cx: &mut Context<MainWindow>) {
    m.tasks.project = Some(project.to_string());
    m.tasks.show = Show::Open;
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
        let f = Filter { project: None, show: Show::Open, words: "" };
        let gs = groups(&it, &goals, GroupBy::Goal, &f);
        let got: Vec<(&str, &str, Option<&str>)> = gs.iter().map(|g| (g.r.as_str(), g.name.as_str(), g.state.as_ref().map(|s| s.0.as_str()))).collect();
        assert_eq!(got, [("G1", "Goal 1", Some("1 needs you")), ("G2", "Goal 2", Some("1 working")), ("G3", "Goal 3", Some("Paused")), ("", "Not in a goal", Some("1 queued"))]);
        // The header counts the whole goal; the rows are only the open ones.
        assert_eq!((gs[0].count.as_str(), refs(&gs[0])), ("2 open", vec!["T2", "T3"]));
        assert_eq!(gs[0].bar.iter().filter(|(_, n)| *n > 0).copied().collect::<Vec<_>>(), [(St::Done, 1), (St::Needs, 1), (St::Queued, 1)]);
        // A group that needs you or runs starts open; a paused or queued one starts closed.
        assert_eq!(gs.iter().map(|g| g.open).collect::<Vec<_>>(), [true, true, false, false]);
    }

    #[test]
    fn filters_narrow_by_project_status_and_words() {
        let it = board();
        let gs = groups(&it, &[], GroupBy::Status, &Filter { project: Some("api"), show: Show::Open, words: "" });
        assert_eq!(gs.iter().map(|g| (g.name.as_str(), refs(g))).collect::<Vec<_>>(), [("Working", vec!["T4"]), ("Queued", vec!["T5"])]);
        let done = groups(&it, &[], GroupBy::Status, &Filter { project: None, show: Show::Done, words: "" });
        assert_eq!(done.iter().map(|g| (g.name.as_str(), refs(g))).collect::<Vec<_>>(), [("Failed", vec!["T6"]), ("Done", vec!["T1"])]);
        let queued = groups(&it, &[], GroupBy::Project, &Filter { project: None, show: Show::Only(St::Queued), words: "" });
        assert_eq!(queued.iter().map(|g| (g.name.as_str(), refs(g))).collect::<Vec<_>>(), [("web", vec!["T3", "T7"]), ("api", vec!["T5"])]);
        // Words match the title or the start of the ref, and open every group.
        let found = groups(&it, &[], GroupBy::Goal, &Filter { project: None, show: Show::Open, words: "t7" });
        assert_eq!((found.len(), refs(&found[0]), found[0].open), (1, vec!["T7"], true));
        assert_eq!(groups(&it, &[], GroupBy::Goal, &Filter { project: None, show: Show::Open, words: "task 4" })[0].r, "G2");
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
    fn a_goal_after_another_or_stopped_says_so() {
        let it = items(&json!({"tasks": [task(1, "web", "queued", Some(4), json!({})), task(2, "web", "queued", Some(5), json!({}))]}));
        let goals = [json!({"ref": "G4", "after": ["G9"], "after_blocker": "G9 isn't done"}), json!({"ref": "G5", "stopped": "Wave 1 stopped for review"})];
        let gs = groups(&it, &goals, GroupBy::Goal, &Filter { project: None, show: Show::Open, words: "" });
        let got: Vec<(&str, Option<&(String, Option<St>)>)> = gs.iter().map(|g| (g.r.as_str(), g.state.as_ref())).collect();
        assert_eq!(got, [("G5", Some(&("Stopped for your review".to_string(), Some(St::Needs)))), ("G4", Some(&("After G9".to_string(), None)))]);
    }
}
