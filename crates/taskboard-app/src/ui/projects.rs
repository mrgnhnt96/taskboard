//! The Projects page: every project on the left (the ones with open goals first, then the rest
//! under "No open goals"), and the picked one on the right: its path, latest release, the projects
//! it waits for, its backlog, its open goals (each opening in place to its tasks, `GET /goals/:id`)
//! and the goals it finished. The facts come from the state's `projects`; read-only.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, i, opt_s, s};
use crate::theme::Theme;
use crate::ui::goals_page::{self, GoalGroup, Tally};
use crate::ui::modals::locale_cmp;
use crate::ui::status::{self, St};
use crate::ui::{goal, kit};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Finished goals shown before Show all.
const FINISHED_SHOWN: usize = 5;
const LIST_W: f32 = 280.;

#[derive(Default)]
pub struct State {
    /// The project picked; the first in the list when none is (or it's gone).
    pub selected: Option<String>,
    /// The goals open in place.
    pub open: HashSet<String>,
    /// Every finished goal, not only the newest.
    pub show_all: bool,
    /// `GET /goals/:id` for the picked project's open goals and the finished ones open, by ref.
    pub details: HashMap<String, Value>,
}

// ================================================================== view models

/// One project in the list.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectRow {
    pub name: String,
    /// Goals not finished (active, next, paused or set aside).
    pub open: usize,
    /// Terminals working in it now.
    pub running: usize,
    pub queued: bool,
    pub needs: bool,
    pub waits_on: Vec<String>,
    pub finished: usize,
}

impl ProjectRow {
    /// "5 goals · 2 running", "6 goals · queued", "1 goal · waits for lens".
    pub fn sub(&self) -> String {
        let mut v = fmt::plural(self.open as i64, "goal", "goals");
        if self.running > 0 {
            v.push_str(&format!(" · {} running", self.running));
        } else if self.queued {
            v.push_str(" · queued");
        } else if !self.waits_on.is_empty() {
            v.push_str(&format!(" · waits for {}", self.waits_on.join(", ")));
        }
        v
    }
}

fn goals_in<'a>(goals: &'a [Value], project: &str) -> impl Iterator<Item = &'a Value> {
    goals.iter().filter(move |g| !fmt::b(g, "archived") && goals_page::goal_projects(g).contains(&project))
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    arr(v, key).iter().filter_map(|x| x.as_str()).map(str::to_string).collect()
}

/// The list: projects with open goals, then the ones without, each by name.
pub fn list(state: &Value, goals: &[Value]) -> (Vec<ProjectRow>, Vec<ProjectRow>) {
    let mut rows: Vec<ProjectRow> = arr(state, "projects")
        .iter()
        .map(|p| {
            let name = s(p, "name").to_string();
            let (mut open, mut finished, mut queued, mut needs) = (0, 0, false, false);
            for g in goals_in(goals, &name) {
                if goals_page::goal_group(g) == GoalGroup::Finished {
                    finished += 1;
                } else {
                    open += 1;
                    queued |= i(g, "queued") > 0;
                    needs |= i(g, "needs") > 0;
                }
            }
            let running = arr(state, "sessions").iter().filter(|x| s(x, "project") == name && s(x, "status") == "working").count();
            ProjectRow { name, open, running, queued, needs, waits_on: strings(p, "waits_on"), finished }
        })
        .collect();
    rows.sort_by(|a, b| locale_cmp(&a.name, &b.name));
    rows.into_iter().partition(|r| r.open > 0)
}

/// The project shown: the one picked while it's still there, else the first in the list.
pub fn selected(st: &State, open: &[ProjectRow], idle: &[ProjectRow]) -> Option<String> {
    let all = || open.iter().chain(idle);
    st.selected.clone().filter(|p| all().any(|r| &r.name == p)).or_else(|| all().next().map(|r| r.name.clone()))
}

/// What the picked project's header says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Header {
    pub path: String,
    /// The latest release's ref and name.
    pub release: Option<(String, String)>,
    /// "Oct 17 · 13 unreleased"; empty with releases off.
    pub release_meta: String,
    pub waits_on: Vec<String>,
    pub backlog: i64,
}

pub fn header(state: &Value, project: &str) -> Header {
    let Some(p) = arr(state, "projects").iter().find(|p| s(p, "name") == project) else { return Header::default() };
    let rel = &p["latest_release"];
    let release = opt_s(rel, "ref").map(|r| (r.to_string(), s(rel, "name").to_string()));
    let mut meta: Vec<String> = vec![];
    if fmt::b(p, "releases") {
        if let Some(t) = opt_s(rel, "cut_at").and_then(fmt::parse) {
            meta.push(fmt::local(t).format("%b %-d").to_string());
        }
        if let Some(n) = p["unreleased"].as_i64().filter(|n| *n > 0) {
            meta.push(format!("{n} unreleased"));
        }
    }
    Header {
        path: opt_s(p, "path").map(fmt::short_path).unwrap_or_default(),
        release: release.filter(|_| fmt::b(p, "releases")),
        release_meta: meta.join(" · "),
        waits_on: strings(p, "waits_on"),
        backlog: i(p, "backlog_open"),
    }
}

/// One open goal's row.
#[derive(Clone, Debug, PartialEq)]
pub struct GoalLine {
    pub r: String,
    pub name: String,
    /// "with midna" when its tasks are in other projects too.
    pub with: String,
    pub parts: Vec<(St, i64)>,
    /// "0 of 4 · wave 1".
    pub progress: String,
    pub pill: (St, &'static str),
    /// Paused or set aside: drawn quieter.
    pub idle: bool,
}

fn group_rank(g: GoalGroup) -> u8 {
    match g {
        GoalGroup::Active => 0,
        GoalGroup::Next => 1,
        GoalGroup::Paused => 2,
        GoalGroup::Deprioritized => 3,
        GoalGroup::Finished => 4,
    }
}

/// The status pill: what most needs saying about the goal now.
fn pill(g: &Value, t: &Tally) -> (St, &'static str) {
    let n = |st: St| t.parts().iter().find(|(s, _)| *s == st).map(|(_, n)| *n).unwrap_or(0);
    match goals_page::goal_group(g) {
        GoalGroup::Paused => (St::Queued, "Paused"),
        GoalGroup::Deprioritized => (St::Queued, "Set aside"),
        _ if n(St::Needs) > 0 || opt_s(g, "stopped").is_some() => (St::Needs, St::Needs.label()),
        _ if n(St::Working) > 0 => (St::Working, St::Working.label()),
        _ if t.stuck() > 0 => (St::Failed, St::Failed.label()),
        _ if n(St::Review) > 0 => (St::Review, St::Review.label()),
        _ if n(St::Queued) > 0 || opt_s(g, "after_blocker").is_some() => (St::Queued, St::Queued.label()),
        // A task that finished failed is history once the goal moves on; it says Failed only
        // when nothing else is live. A failure the goal stops on already says Needs you.
        _ if n(St::Failed) > 0 => (St::Failed, St::Failed.label()),
        _ => (St::Planned, St::Planned.label()),
    }
}

fn goal_line(g: &Value, d: Option<&Value>, project: &str) -> GoalLine {
    let t = Tally::of(g, d);
    let parts = t.parts();
    let done = parts.iter().find(|(s, _)| *s == St::Done).map(|(_, n)| *n).unwrap_or(0);
    let counted: i64 = parts.iter().map(|(_, n)| n).sum();
    // The summary's counts leave out blocked and held tasks; its total doesn't.
    let total = if d.is_some_and(|d| d["tasks"].is_array()) { counted } else { counted.max(i(g, "total")) };
    let mut progress = format!("{done} of {total}");
    let ws = goals_page::waves(d);
    if let Some(w) = goals_page::current_wave(&ws).filter(|_| ws.len() > 1) {
        progress.push_str(&format!(" · wave {}", i(&ws[w], "wave")));
    }
    let others: Vec<&str> = goals_page::goal_projects(g).into_iter().filter(|p| *p != project).collect();
    let group = goals_page::goal_group(g);
    GoalLine {
        r: fmt::ref_of(g, "G"),
        name: s(g, "name").to_string(),
        with: if others.is_empty() { String::new() } else { format!("with {}", others.join(", ")) },
        parts,
        progress,
        pill: pill(g, &t),
        idle: matches!(group, GoalGroup::Paused | GoalGroup::Deprioritized),
    }
}

/// The project's goals not finished: started first, then next, paused and set aside, newest first in each.
pub fn active(goals: &[Value], details: &HashMap<String, Value>, project: &str) -> Vec<GoalLine> {
    let mut v: Vec<&Value> = goals_in(goals, project).filter(|g| goals_page::goal_group(g) != GoalGroup::Finished).collect();
    v.sort_by(|a, b| group_rank(goals_page::goal_group(a)).cmp(&group_rank(goals_page::goal_group(b))).then(i(b, "id").cmp(&i(a, "id"))));
    v.into_iter().map(|g| goal_line(g, details.get(&fmt::ref_of(g, "G")), project)).collect()
}

/// One finished goal's row.
#[derive(Clone, Debug, PartialEq)]
pub struct DoneLine {
    pub r: String,
    pub name: String,
    pub tasks: String,
    /// "Oct 18".
    pub when: String,
}

/// The project's finished goals, newest first, and how many there are in all.
pub fn finished(goals: &[Value], project: &str, all: bool) -> (Vec<DoneLine>, usize) {
    let mut v: Vec<&Value> = goals_in(goals, project).filter(|g| goals_page::goal_group(g) == GoalGroup::Finished).collect();
    v.sort_by(|a, b| s(b, "finished_at").cmp(s(a, "finished_at")).then(i(b, "id").cmp(&i(a, "id"))));
    let n = v.len();
    let lines = v
        .into_iter()
        .take(if all { n } else { FINISHED_SHOWN })
        .map(|g| DoneLine {
            r: fmt::ref_of(g, "G"),
            name: s(g, "name").to_string(),
            tasks: fmt::plural(i(g, "total"), "task", "tasks"),
            when: opt_s(g, "finished_at").and_then(fmt::parse).map(|t| fmt::local(t).format("%b %-d").to_string()).unwrap_or_default(),
        })
        .collect();
    (lines, n)
}

/// The goals the page fetches in full: the picked project's open goals and its finished ones open.
pub fn wanted(st: &State, state: &Value, goals: &[Value]) -> Vec<String> {
    let (open, idle) = list(state, goals);
    let Some(p) = selected(st, &open, &idle) else { return vec![] };
    let mut v: Vec<String> = goals_in(goals, &p)
        .filter(|g| goals_page::goal_group(g) != GoalGroup::Finished || st.open.contains(&fmt::ref_of(g, "G")))
        .map(|g| fmt::ref_of(g, "G"))
        .collect();
    v.sort();
    v
}

/// What an open goal's task row says beside its title: what it's doing, else its wave.
fn task_meta(c: &Value, row: &status::Row) -> String {
    if !row.line.is_empty() {
        return row.line.clone();
    }
    if !row.age.is_empty() && row.st == St::Working {
        return format!("for {}", row.age);
    }
    if row.st.open() {
        if let Some(w) = c["wave"].as_i64() {
            return format!("wave {w}");
        }
    }
    String::new()
}

// ================================================================== render

fn section(t: &Theme, title: String) -> Div {
    div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(t.muted).child(title)
}

fn goal_pill(t: &Theme, (st, label): (St, &'static str)) -> Div {
    let (fg, bg, line) = st.colors(t);
    let (fg, bg) = if label == "Paused" || label == "Set aside" { (t.muted, t.seg) } else { (fg, bg) };
    kit::pill(fg, bg, label).w(px(96.)).justify_center().when(st == St::Planned, |d| d.border_1().border_color(line))
}

fn project_row(t: &Theme, r: &ProjectRow, on: bool, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let name = r.name.clone();
    let hover = t.panel_2;
    let dot = if r.needs { Some(t.warn) } else if r.running > 0 { Some(t.accent) } else { None };
    let idle = r.open == 0;
    div()
        .id(SharedString::from(format!("projects-row-{}", r.name)))
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(12.))
        .py(px(if idle { 7. } else { 10. }))
        .rounded(px(8.))
        .cursor_pointer()
        .when(on, |d| d.bg(t.panel_2))
        .hover(move |s| s.bg(hover))
        .child(if idle {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_baseline()
                .justify_between()
                .gap(px(8.))
                .child(div().truncate().text_size(px(14.)).text_color(if on { t.text } else { t.muted }).child(r.name.clone()))
                .when(r.finished > 0, |d| d.child(div().flex_none().text_size(px(12.)).text_color(t.faint).child(format!("{} done", r.finished))))
        } else {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().truncate().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(r.name.clone()))
                .child(div().truncate().text_size(px(12.)).text_color(t.muted).child(r.sub()))
        })
        .children(dot.map(|c| kit::dot(c, 8.)))
        .on_click(cx.listener(move |m, _, _, cx| {
            m.projects.selected = Some(name.clone());
            m.projects.show_all = false;
            m.refresh(cx);
        }))
}

fn project_list(t: &Theme, open: &[ProjectRow], idle: &[ProjectRow], picked: Option<&str>, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let mut col = div()
        .id("projects-list")
        .flex()
        .flex_col()
        .flex_none()
        .w(px(LIST_W))
        .h_full()
        .overflow_y_scroll()
        .border_r_1()
        .border_color(t.border)
        .px(px(12.))
        .pt(px(22.))
        .pb(px(24.))
        .gap(px(2.))
        .child(div().px(px(12.)).pb(px(10.)).text_size(px(20.)).font_weight(FontWeight::BOLD).child("Projects"));
    for r in open {
        col = col.child(project_row(t, r, picked == Some(r.name.as_str()), cx));
    }
    if !idle.is_empty() {
        col = col.child(div().px(px(12.)).pt(px(18.)).pb(px(4.)).child(section(t, "No open goals".into())));
        for r in idle {
            col = col.child(project_row(t, r, picked == Some(r.name.as_str()), cx));
        }
    }
    col
}

fn head(t: &Theme, project: &str, h: &Header, cx: &mut Context<MainWindow>) -> Div {
    let mut meta = div().flex().flex_wrap().items_center().gap_x(px(16.)).gap_y(px(6.)).text_size(px(13.)).text_color(t.muted);
    if !h.path.is_empty() {
        meta = meta.child(div().child(h.path.clone()));
    }
    if h.release.is_some() || !h.release_meta.is_empty() {
        let mut rel = div().flex().gap(px(6.));
        if let Some((r, name)) = h.release.clone() {
            let p = project.to_string();
            rel = rel.child(kit::link(t, "projects-release", name).on_click(cx.listener(move |m, _, _, cx| {
                m.releases.project = p.clone();
                m.releases.open.insert(r.clone());
                m.go(Page::Releases, cx);
            })));
        }
        meta = meta.child(rel.when(!h.release_meta.is_empty(), |d| d.child(div().child(h.release_meta.clone()))));
    }
    if !h.waits_on.is_empty() {
        meta = meta.child(div().child(format!("Waits for {}", h.waits_on.join(", "))));
    }
    let p = project.to_string();
    meta = meta.child(kit::link(t, "projects-backlog", format!("Backlog {}", h.backlog)).on_click(cx.listener(move |m, _, _, cx| {
        m.backlog.project = p.clone();
        m.go(Page::Backlog, cx);
    })));
    div().flex().flex_col().gap(px(8.)).child(div().text_size(px(26.)).font_weight(FontWeight::BOLD).child(project.to_string())).child(meta)
}

fn chevron(t: &Theme, open: bool) -> Div {
    div().flex_none().w(px(18.)).text_size(px(13.)).text_color(t.muted).child(if open { "⌄" } else { "›" })
}

fn goal_ref(t: &Theme, r: &str, color: Hsla, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let gr = r.to_string();
    div()
        .id(SharedString::from(format!("projects-ref-{r}")))
        .flex_none()
        .w(px(44.))
        .text_size(px(12.))
        .font_family(t.mono_font.clone())
        .text_color(color)
        .cursor_pointer()
        .hover(|s| s.underline())
        .child(r.to_string())
        .on_click(cx.listener(move |m, _, _, cx| {
            cx.stop_propagation();
            goal::open(m, &gr, false, cx);
        }))
}

fn toggle(r: &str) -> impl Fn(&mut MainWindow, &ClickEvent, &mut Window, &mut Context<MainWindow>) + 'static {
    let r = r.to_string();
    move |m, _, _, cx| {
        if !m.projects.open.remove(&r) {
            m.projects.open.insert(r.clone());
        }
        m.refresh(cx);
    }
}

fn task_row(t: &Theme, c: &Value, cx: &mut Context<MainWindow>) -> Div {
    let r = s(c, "ref").to_string();
    let row = status::row(c);
    let (loud_fg, _, _) = row.st.colors(t);
    let (o1, o2) = (r.clone(), r.clone());
    let link = t.accent;
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(16.))
        .py(px(7.))
        .border_t_1()
        .border_color(t.divider)
        .text_size(px(13.))
        .child(div().flex_none().w(px(18.)))
        .child(
            div()
                .id(SharedString::from(format!("projects-task-ref-{r}")))
                .flex_none()
                .w(px(44.))
                .text_size(px(12.))
                .font_family(t.mono_font.clone())
                .text_color(t.muted)
                .cursor_pointer()
                .hover(move |s| s.text_color(link).underline())
                .child(r.clone())
                .on_click(cx.listener(move |m, _, _, cx| m.open_task(o1.clone(), cx))),
        )
        .child(goals_page::fr(
            div().flex().child(
                div()
                    .id(SharedString::from(format!("projects-task-{r}")))
                    .min_w_0()
                    .truncate()
                    .cursor_pointer()
                    .hover(move |s| s.text_color(link).underline())
                    .child(s(c, "title").to_string())
                    .on_click(cx.listener(move |m, _, _, cx| m.open_task(o2.clone(), cx))),
            ),
            1.,
        ))
        .child(div().flex_none().w(px(190.)).truncate().text_size(px(12.)).text_color(if row.loud { loud_fg } else { t.muted }).child(task_meta(c, &row)))
        .child(div().flex_none().w(px(96.)).flex().justify_end().child(status::chip(t, row.st)))
}

fn goal_tasks(t: &Theme, d: Option<&Value>, cx: &mut Context<MainWindow>) -> Div {
    let mut body = div().flex().flex_col().bg(t.panel_2);
    let Some(d) = d.filter(|d| d["tasks"].is_array()) else {
        return body.child(div().px(px(16.)).pl(px(90.)).py(px(8.)).border_t_1().border_color(t.divider).text_size(px(12.)).text_color(t.muted).child("Loading…"));
    };
    let mut tasks: Vec<&Value> = arr(d, "tasks").iter().collect();
    tasks.sort_by_key(|c| (c["wave"].as_i64().unwrap_or(i64::MAX), status::row(c).st.rank(), i(c, "id")));
    for c in tasks {
        body = body.child(task_row(t, c, cx));
    }
    body
}

fn active_row(t: &Theme, g: &GoalLine, open: bool, d: Option<&Value>, first: bool, cx: &mut Context<MainWindow>) -> Div {
    let name_color = if g.idle { t.muted } else { t.text };
    let head = div()
        .id(SharedString::from(format!("projects-goal-{}", g.r)))
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(16.))
        .py(px(12.))
        .cursor_pointer()
        .child(chevron(t, open))
        .child(goal_ref(t, &g.r, t.muted, cx))
        .child(goals_page::fr(
            div()
                .flex()
                .gap(px(6.))
                .text_size(px(14.))
                .child(div().min_w_0().truncate().text_color(name_color).when(open || !g.idle, |d| d.font_weight(FontWeight::SEMIBOLD)).child(g.name.clone()))
                .when(!g.with.is_empty(), |d| d.child(div().flex_none().text_color(t.muted).child(format!("· {}", g.with)))),
            1.,
        ))
        .child(div().flex_none().w(px(150.)).flex().flex_col().gap(px(4.)).child(status::bar(t, &g.parts, 5.)).child(div().text_size(px(12.)).text_color(t.muted).child(g.progress.clone())))
        .child(div().flex_none().w(px(96.)).flex().justify_end().child(goal_pill(t, g.pill)))
        .on_click(cx.listener(toggle(&g.r)));
    div().flex().flex_col().when(!first, |d| d.border_t_1().border_color(t.divider)).child(head).when(open, |el| el.child(goal_tasks(t, d, cx)))
}

fn done_row(t: &Theme, g: &DoneLine, open: bool, d: Option<&Value>, first: bool, cx: &mut Context<MainWindow>) -> Div {
    let head = div()
        .id(SharedString::from(format!("projects-done-{}", g.r)))
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(16.))
        .py(px(10.))
        .cursor_pointer()
        .text_size(px(14.))
        .child(chevron(t, open))
        .child(goal_ref(t, &g.r, t.faint, cx))
        .child(goals_page::fr(div().truncate().text_color(t.text_2).child(g.name.clone()), 1.))
        .child(div().flex_none().w(px(150.)).text_size(px(12.)).text_color(t.muted).child(g.tasks.clone()))
        .child(div().flex_none().w(px(96.)).text_right().text_size(px(12.)).text_color(t.muted).child(g.when.clone()))
        .on_click(cx.listener(toggle(&g.r)));
    div().flex().flex_col().when(!first, |d| d.border_t_1().border_color(t.divider)).child(head).when(open, |el| el.child(goal_tasks(t, d, cx)))
}

fn card(t: &Theme) -> Div {
    div().flex().flex_col().rounded(px(12.)).border_1().border_color(t.border).bg(t.card).overflow_hidden()
}

pub fn render(m: &mut MainWindow, _window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let state = m.state().clone();
    let goals: Vec<Value> = m.data.goals.clone();
    let (open, idle) = list(&state, &goals);
    let picked = selected(&m.projects, &open, &idle);
    let shell = div().id("projects-page").flex().flex_1().min_w_0().h_full().line_height(relative(1.4)).child(project_list(&t, &open, &idle, picked.as_deref(), cx));
    let Some(p) = picked else {
        let msg = if m.data.state.is_none() { if m.down.is_some() { "Can’t load the board." } else { "Loading…" } } else { "No projects" };
        return shell.child(div().flex_1().child(kit::empty(&t, msg))).into_any_element();
    };
    let mut main = div().id("projects-main").flex_1().min_w_0().h_full().overflow_y_scroll().restrict_scroll_to_axis().px(px(32.)).pt(px(24.)).pb(px(32.)).child(head(&t, &p, &header(&state, &p), cx));

    let act = active(&goals, &m.projects.details, &p);
    if !act.is_empty() {
        let mut c = card(&t);
        for (n, g) in act.iter().enumerate() {
            let r = &g.r;
            c = c.child(active_row(&t, g, m.projects.open.contains(r), m.projects.details.get(r), n == 0, cx));
        }
        main = main.child(div().mt(px(22.)).mb(px(8.)).child(section(&t, "Active".into()))).child(c);
    }

    let (done, n) = finished(&goals, &p, m.projects.show_all);
    if n > 0 {
        let more = n > FINISHED_SHOWN;
        let all = m.projects.show_all;
        let label = div()
            .flex()
            .items_center()
            .justify_between()
            .mt(px(22.))
            .mb(px(8.))
            .child(section(&t, format!("Finished · {n}")))
            .when(more, |d| {
                d.child(kit::link(&t, "projects-show-all", if all { "Show fewer" } else { "Show all" }).text_size(px(12.)).on_click(cx.listener(|m, _, _, cx| {
                    m.projects.show_all = !m.projects.show_all;
                    cx.notify();
                })))
            });
        let mut c = card(&t);
        for (n, g) in done.iter().enumerate() {
            let r = &g.r;
            c = c.child(done_row(&t, g, m.projects.open.contains(r), m.projects.details.get(r), n == 0, cx));
        }
        main = main.child(label).child(c);
    }
    if act.is_empty() && n == 0 {
        main = main.child(div().mt(px(22.)).child(kit::empty(&t, "No goals")));
    }
    shell.child(main).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;
    use serde_json::json;

    fn goal(id: i64, project: &str, done: i64, total: i64, extra: Value) -> Value {
        let mut g = json!({"id": id, "ref": format!("G{id}"), "name": format!("Goal {id}"), "project": project, "projects": [], "done": done, "total": total,
            "active": 0, "needs": 0, "queued": 0, "planned": 0, "review": 0, "prs_open": [], "paused": false, "archived": false});
        for (k, v) in extra.as_object().unwrap() {
            g[k] = v.clone();
        }
        g
    }

    fn state() -> Value {
        json!({
            "projects": [
                {"name": "web", "path": "/srv/web", "releases": true, "latest_release": {"ref": "R4", "name": "v1.2", "cut_at": "2026-10-17T18:00:00Z"}, "unreleased": 13, "waits_on": ["api"], "backlog_open": 9},
                {"name": "api", "path": null, "releases": false, "latest_release": null, "unreleased": null, "waits_on": [], "backlog_open": 0},
                {"name": "docs", "path": "/srv/docs", "releases": false, "latest_release": null, "unreleased": null, "waits_on": ["web"], "backlog_open": 2},
                {"name": "lens", "path": "/srv/lens", "releases": true, "latest_release": null, "unreleased": 3, "waits_on": [], "backlog_open": 1},
            ],
            "sessions": [
                {"project": "web", "status": "working"},
                {"project": "web", "status": "working"},
                {"project": "web", "status": "needs"},
                {"project": "api", "status": "idle"},
            ],
        })
    }

    fn goals() -> Vec<Value> {
        vec![
            goal(1, "web", 1, 4, json!({"active": 2, "needs": 1})),
            goal(2, "web", 0, 3, json!({"paused": true})),
            goal(3, "web", 2, 2, json!({"finished_at": "2026-10-12T17:00:00Z"})),
            goal(4, "web", 3, 3, json!({"finished_at": "2026-10-18T17:00:00Z"})),
            goal(5, "api", 0, 2, json!({"queued": 2, "projects": ["api", "web"]})),
            goal(6, "docs", 0, 1, json!({"planned": 1})),
            goal(7, "lens", 1, 1, json!({"finished_at": "2026-10-01T17:00:00Z"})),
            goal(8, "web", 0, 0, json!({"archived": true, "active": 1})),
        ]
    }

    #[test]
    fn the_list_puts_projects_without_open_goals_last() {
        let (open, idle) = list(&state(), &goals());
        let got: Vec<(&str, String, bool)> = open.iter().map(|r| (r.name.as_str(), r.sub(), r.needs)).collect();
        assert_eq!(got, [("api", "1 goal · queued".to_string(), false), ("docs", "1 goal · waits for web".to_string(), false), ("web", "3 goals · 2 running".to_string(), true)]);
        assert_eq!(idle.iter().map(|r| (r.name.as_str(), r.finished)).collect::<Vec<_>>(), [("lens", 1)]);
    }

    #[test]
    fn the_picked_project_falls_back_to_the_first() {
        let (open, idle) = list(&state(), &goals());
        let mut st = State::default();
        assert_eq!(selected(&st, &open, &idle).as_deref(), Some("api"));
        st.selected = Some("lens".into());
        assert_eq!(selected(&st, &open, &idle).as_deref(), Some("lens"));
        st.selected = Some("gone".into());
        assert_eq!(selected(&st, &open, &idle).as_deref(), Some("api"));
        assert_eq!(selected(&st, &[], &[]), None);
    }

    #[test]
    fn the_header_shows_release_facts_only_with_releases_on() {
        let h = header(&state(), "web");
        assert_eq!(h.release, Some(("R4".into(), "v1.2".into())));
        assert_eq!(h.release_meta, "Oct 17 · 13 unreleased");
        assert_eq!((h.path.as_str(), h.waits_on.clone(), h.backlog), ("/srv/web", vec!["api".to_string()], 9));
        let h = header(&state(), "api");
        assert_eq!((h.release, h.release_meta.as_str(), h.path.as_str()), (None, "", ""));
        let h = header(&state(), "lens");
        assert_eq!((h.release, h.release_meta.as_str()), (None, "3 unreleased"));
        assert_eq!(header(&state(), "gone"), Header::default());
    }

    #[test]
    fn active_goals_list_started_first_with_their_status() {
        let gs = goals();
        let got: Vec<(String, String, String, &str, bool)> = active(&gs, &HashMap::new(), "web").into_iter().map(|g| (g.r, g.with, g.progress, g.pill.1, g.idle)).collect();
        assert_eq!(
            got,
            [
                ("G5".into(), "with api".into(), "0 of 2".into(), "Queued", false),
                ("G1".into(), "".into(), "1 of 4".into(), "Needs you", false),
                ("G2".into(), "".into(), "0 of 3".into(), "Paused", true),
            ]
        );
    }

    #[test]
    fn an_open_goal_says_its_wave_from_its_detail() {
        let g = goal(1, "web", 1, 3, json!({"active": 1}));
        let d = json!({"tasks": [
            {"id": 1, "ref": "T1", "title": "a", "status": "done", "wave": 1},
            {"id": 2, "ref": "T2", "title": "b", "status": "working", "wave": 2},
            {"id": 3, "ref": "T3", "title": "c", "status": "planned", "wave": 3},
        ], "waves": [{"wave": 1, "state": "done"}, {"wave": 2, "state": "running"}, {"wave": 3, "state": "waiting"}]});
        let details = HashMap::from([("G1".to_string(), d)]);
        let line = &active(std::slice::from_ref(&g), &details, "web")[0];
        assert_eq!((line.progress.as_str(), line.pill.1), ("1 of 3 · wave 2", "Working"));
    }

    #[test]
    fn a_goal_past_its_failed_tasks_says_in_review() {
        let pill = |tasks: Value| {
            let d = json!({"tasks": tasks, "waves": [{"wave": 1, "state": "done"}, {"wave": 2, "state": "running"}]});
            let details = HashMap::from([("G1".to_string(), d)]);
            active(&[goal(1, "web", 1, 3, json!({}))], &details, "web")[0].pill.1
        };
        let failed = json!({"id": 1, "ref": "T1", "title": "a", "status": "done", "failed": true, "wave": 1});
        let review = json!({"id": 2, "ref": "T2", "title": "b", "status": "done", "wave": 2, "pr": {"num": 9, "state": "open"}});
        assert_eq!(pill(json!([failed, review])), "In review");
        assert_eq!(pill(json!([failed, {"id": 2, "ref": "T2", "title": "b", "status": "queued", "wave": 2}])), "Queued");
        assert_eq!(pill(json!([failed, {"id": 2, "ref": "T2", "title": "b", "status": "done", "wave": 2}])), "Failed");
        // A lost terminal is live: it still says Failed over a PR in review.
        assert_eq!(pill(json!([review, {"id": 3, "ref": "T3", "title": "c", "status": "needs", "lost": true, "wave": 2}])), "Failed");
    }

    #[test]
    fn finished_goals_come_newest_first_with_show_all() {
        let mut gs = goals();
        for n in 0..5 {
            gs.push(goal(20 + n, "web", 1, 1, json!({"finished_at": format!("2026-09-0{}T17:00:00Z", n + 1)})));
        }
        let (lines, n) = finished(&gs, "web", false);
        assert_eq!(n, 7);
        assert_eq!(lines.iter().map(|l| l.r.as_str()).collect::<Vec<_>>(), ["G4", "G3", "G24", "G23", "G22"]);
        assert_eq!((lines[0].tasks.as_str(), lines[0].when.as_str()), ("3 tasks", "Oct 18"));
        assert_eq!(finished(&gs, "web", true).0.len(), 7);
    }

    #[test]
    fn the_page_fetches_the_picked_projects_open_goals_and_open_finished_ones() {
        let gs = goals();
        let mut st = State { selected: Some("web".into()), ..Default::default() };
        assert_eq!(wanted(&st, &state(), &gs), ["G1", "G2", "G5"]);
        st.open.insert("G3".into());
        assert_eq!(wanted(&st, &state(), &gs), ["G1", "G2", "G3", "G5"]);
    }
}
