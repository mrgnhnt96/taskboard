//! The Goals page (`GET /goals`): the goals in progress as rows that open in place to their tasks
//! by wave (`GET /goals/:id` for each), and a strip below for what's next, paused, set aside
//! and finished. A goal's full page is still its own screen; a row's ref opens it.
use crate::app::MainWindow;
use crate::fmt::{self, arr, i, opt_s, s};
use crate::theme::Theme;
use crate::ui::goals::nav_kind;
use crate::ui::issue_panel as ip;
use crate::ui::modals::locale_cmp;
use crate::ui::status::{self, St};
use crate::ui::{goal, kit};
use chrono::{DateTime, Datelike, Duration, Utc};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

const PAGE_X: f32 = 28.;
const PROJECT_FILTER: &str = "goals-project";
/// A roomy shelf's ref column, so each goal's second line starts under its name.
const REF_W: f32 = 34.;
/// The left edge of wave headers and task rows inside an open goal (under its name).
const INSET: f32 = 60.;

#[derive(Default)]
pub struct State {
    /// The goals open in place.
    pub open: HashSet<String>,
    /// "all" or one project.
    pub project: String,
    /// Older finished goals shown under this week's.
    pub earlier: bool,
    /// `GET /goals/:id` for each goal in progress, by ref.
    pub details: HashMap<String, Value>,
}

impl State {
    pub fn project(&self) -> &str {
        if self.project.is_empty() { "all" } else { &self.project }
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

/// The projects goal `g` holds work in: its own, then the others its tasks are in.
pub(crate) fn goal_projects(g: &Value) -> Vec<&str> {
    let ps: Vec<&str> = fmt::arr(g, "projects").iter().filter_map(|p| p.as_str()).collect();
    if ps.is_empty() { vec![s(g, "project")] } else { ps }
}

fn in_project(g: &Value, project: &str) -> bool {
    project == "all" || goal_projects(g).contains(&project)
}

/// The goals in progress, in the project picked: the ones the page fetches in full.
pub fn in_progress(goals: &[Value], project: &str) -> Vec<String> {
    let mut v: Vec<&Value> = goals.iter().filter(|g| !fmt::b(g, "archived") && goal_group(g) == GoalGroup::Active && in_project(g, project)).collect();
    v.sort_by(|a, b| locale_cmp(s(a, "project"), s(b, "project")).then(i(a, "id").cmp(&i(b, "id"))));
    v.into_iter().map(|g| fmt::ref_of(g, "G")).collect()
}

/// How many of a goal's tasks are in each status.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Tally {
    needs: i64,
    working: i64,
    review: i64,
    queued: i64,
    planned: i64,
    done: i64,
    failed: i64,
    /// Failed but still open (a lost terminal, a start that didn't take): live, unlike a task
    /// that finished failed.
    stuck: i64,
}

impl Tally {
    fn add(&mut self, st: St) {
        match st {
            St::Needs => self.needs += 1,
            St::Working => self.working += 1,
            St::Review => self.review += 1,
            St::Queued => self.queued += 1,
            St::Planned => self.planned += 1,
            St::Done => self.done += 1,
            St::Failed => self.failed += 1,
        }
    }

    /// From the goal's tasks, else from its summary's counts.
    pub(crate) fn of(g: &Value, detail: Option<&Value>) -> Tally {
        let mut t = Tally::default();
        if let Some(d) = detail.filter(|d| d["tasks"].is_array()) {
            for c in arr(d, "tasks") {
                let st = status::row(c).st;
                t.add(st);
                if st == St::Failed && s(c, "status") != "done" {
                    t.stuck += 1;
                }
            }
            return t;
        }
        let review = arr(g, "prs_open").len() as i64;
        Tally {
            needs: i(g, "needs"),
            working: (i(g, "active") - i(g, "needs")).max(0),
            review,
            queued: i(g, "queued"),
            planned: i(g, "planned"),
            done: i(g, "done"),
            failed: 0,
            stuck: 0,
        }
    }

    /// Failed tasks still open, not ones that finished failed.
    pub(crate) fn stuck(&self) -> i64 {
        self.stuck
    }

    /// The bar's parts, finished work first.
    pub(crate) fn parts(&self) -> Vec<(St, i64)> {
        vec![(St::Done, self.done), (St::Review, self.review), (St::Working, self.working), (St::Needs, self.needs), (St::Failed, self.failed), (St::Queued, self.queued), (St::Planned, self.planned)]
    }
}

/// The signal a goal row offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Pause,
    /// Continue past the wave it stopped after for your review.
    Continue(i64),
}

/// One goal in progress, folded.
#[derive(Clone, Debug, PartialEq)]
pub struct GoalRow {
    pub r: String,
    pub name: String,
    pub project: String,
    pub parts: Vec<(St, i64)>,
    /// Tasks finished; a task whose PR is still open counts in `review` until it merges.
    pub done: i64,
    pub review: i64,
    pub total: i64,
    /// "Wave 2 of 3"; empty with no waves or until the goal's tasks are in.
    pub wave: String,
    /// What it's doing now, in `now_st`'s color.
    pub now: String,
    pub now_st: St,
    /// What comes after, or what it waits on.
    pub next: String,
    pub action: Option<Action>,
}

pub(crate) fn waves(d: Option<&Value>) -> Vec<Value> {
    d.map(|d| arr(d, "waves").to_vec()).unwrap_or_default()
}

/// The wave the goal is at: the first that isn't done.
pub(crate) fn current_wave(ws: &[Value]) -> Option<usize> {
    ws.iter().position(|w| s(w, "state") != "done")
}

fn tasks_in(d: &Value, wave: i64) -> Vec<&Value> {
    arr(d, "tasks").iter().filter(|t| t["wave"].as_i64() == Some(wave)).collect()
}

/// "G4" from the chain's "Its goal runs after G4 (…), which isn't finished yet".
fn after_ref(g: &Value) -> Option<String> {
    let line = opt_s(g, "after_blocker")?;
    line.split(|c: char| !c.is_ascii_alphanumeric()).find(|w| w.len() > 1 && w.starts_with('G') && w[1..].chars().all(|c| c.is_ascii_digit())).map(str::to_string)
}

fn now_line(t: &Tally, stopped: bool) -> (String, St) {
    if stopped {
        return ("Stopped for your review".into(), St::Needs);
    }
    let mut bits = vec![];
    if t.working > 0 {
        bits.push(format!("{} working", t.working));
    }
    if t.needs > 0 {
        bits.push(format!("{} needs you", t.needs));
    }
    if t.failed > 0 {
        bits.push(format!("{} failed", t.failed));
    }
    if bits.is_empty() && t.review > 0 {
        bits.push(format!("{} in review", t.review));
    }
    let st = if t.needs > 0 {
        St::Needs
    } else if t.failed > 0 {
        St::Failed
    } else if t.working > 0 {
        St::Working
    } else if t.review > 0 {
        St::Review
    } else {
        St::Queued
    };
    if bits.is_empty() {
        bits.push(if t.queued > 0 { "Queued".into() } else { "Done".into() });
    }
    (bits.join(" · "), st)
}

/// What's after now: the wave waiting on you, a question, a failure, open PRs, the next wave.
fn next_line(g: &Value, d: Option<&Value>) -> String {
    let Some(d) = d.filter(|d| d["tasks"].is_array()) else {
        let peek = arr(g, "peek");
        return match peek.iter().find(|p| s(p, "status") == "needs") {
            Some(p) => format!("{} needs you", s(p, "ref")),
            None => String::new(),
        };
    };
    let ws = waves(Some(d));
    let more = |n: i64| ws.iter().find(|w| i(w, "wave") == n + 1).map(|w| format!(" · {} in wave {}", fmt::plural(i(w, "total"), "task", "tasks"), n + 1)).unwrap_or_default();
    if let Some(w) = ws.iter().find(|w| s(w, "state") == "stopped") {
        let n = i(w, "wave");
        return format!("Wave {n} done{}", more(n));
    }
    let tasks = arr(d, "tasks");
    let rows: Vec<(&Value, status::Row)> = tasks.iter().map(|t| (t, status::row(t))).collect();
    if let Some((t, _)) = rows.iter().find(|(_, r)| r.st == St::Needs) {
        let why = if s(t, "needs_reason") == "question" { "waits on your answer" } else { "needs you" };
        return format!("{} {why}", s(t, "ref"));
    }
    if let Some((t, _)) = rows.iter().find(|(_, r)| r.st == St::Failed) {
        return format!("{} failed", s(t, "ref"));
    }
    let review: Vec<String> = rows.iter().filter(|(_, r)| r.st == St::Review).map(|(t, r)| if r.line.is_empty() { s(t, "ref").to_string() } else { r.line.clone() }).collect();
    if !review.is_empty() {
        return review.join(" · ");
    }
    match current_wave(&ws).map(|k| &ws[k]) {
        Some(w) if s(w, "state") == "running" && fmt::b(w, "stop_after") && opt_s(w, "released_at").is_none() => "Stops for your review after this wave".into(),
        Some(w) => more(i(w, "wave")).trim_start_matches(" · ").to_string(),
        None => String::new(),
    }
}

pub fn goal_row(g: &Value, d: Option<&Value>) -> GoalRow {
    let tally = Tally::of(g, d);
    let ws = waves(d);
    let stopped = ws.iter().find(|w| s(w, "state") == "stopped").map(|w| i(w, "wave"));
    let (now, now_st) = now_line(&tally, stopped.is_some() || (d.is_none() && opt_s(g, "stopped").is_some()));
    let wave = match (d, current_wave(&ws)) {
        (None, _) => String::new(),
        (Some(_), _) if ws.is_empty() => String::new(),
        (Some(_), Some(k)) => format!("Wave {} of {}", k + 1, ws.len()),
        (Some(_), None) => format!("Wave {0} of {0}", ws.len()),
    };
    GoalRow {
        r: fmt::ref_of(g, "G"),
        name: s(g, "name").into(),
        project: goal_projects(g).join(" + "),
        parts: tally.parts(),
        done: i(g, "done"),
        review: tally.review,
        total: i(g, "total"),
        wave,
        now,
        now_st,
        next: next_line(g, d),
        action: Some(stopped.map_or(Action::Pause, Action::Continue)),
    }
}

/// A wave header's line: "Done Tuesday", "Now · stops for your review after", "After wave 1".
fn wave_meta(w: &Value, tasks: &[&Value], now: DateTime<Utc>) -> String {
    let n = i(w, "wave");
    match s(w, "state") {
        "done" => {
            let last = tasks.iter().filter_map(|t| fmt::parse(opt_s(t, "finished_at").or(opt_s(t, "when")).unwrap_or(""))).max();
            last.map(|t| format!("Done {}", day(t, now))).unwrap_or_else(|| "Done".into())
        }
        "stopped" => "Done · waiting for your review".into(),
        "failed" => "A task failed".into(),
        "held" => "Held".into(),
        "running" if fmt::b(w, "stop_after") && opt_s(w, "released_at").is_none() => "Now · stops for your review after".into(),
        "running" => "Now".into(),
        _ if n > 1 => format!("After wave {}", n - 1),
        _ => "Next".into(),
    }
}

/// "2:05 PM" today, "yesterday", "Tuesday" this past week, "Oct 7" before that.
fn day(t: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let (l, today) = (fmt::local(t), fmt::local(now).date_naive());
    let d = l.date_naive();
    if d == today {
        l.format("%-I:%M %p").to_string()
    } else if d == today - Duration::days(1) {
        "yesterday".into()
    } else if d > today - Duration::days(7) {
        l.format("%A").to_string()
    } else {
        l.format("%b %-d").to_string()
    }
}

/// One goal in the strip below: its ref, name and a short line.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub r: String,
    pub name: String,
    pub meta: String,
    /// The goal's tl;dr, else its outcome: shown when the strip has the page to itself.
    pub about: String,
}

/// A tile in the strip: "Up next", "Paused", "Set aside", "Finished this week".
#[derive(Clone, Debug, PartialEq)]
pub struct Shelf {
    pub key: &'static str,
    pub title: &'static str,
    pub lines: Vec<Line>,
    /// Finished goals from before this week, shown when asked.
    pub earlier: Vec<Line>,
}

fn prs_merged(g: &Value) -> i64 {
    (i(g, "prs_planned") - i(g, "prs_canceled")).max(0)
}

fn start_of_week(now: DateTime<Utc>) -> chrono::NaiveDate {
    let today = fmt::local(now).date_naive();
    today - Duration::days(today.weekday().num_days_from_sunday() as i64)
}

pub fn shelves(goals: &[&Value], now: DateTime<Utc>) -> Vec<Shelf> {
    let of = |grp: GoalGroup| -> Vec<&Value> {
        let mut v: Vec<&Value> = goals.iter().copied().filter(|g| goal_group(g) == grp).collect();
        v.sort_by_key(|g| i(g, "id"));
        v
    };
    let progress = |g: &Value| format!("{} of {}", i(g, "done"), i(g, "total"));
    let line = |g: &Value, meta: String| {
        let about = opt_s(g, "tldr").or_else(|| opt_s(g, "outcome")).unwrap_or_default().to_string();
        Line { r: fmt::ref_of(g, "G"), name: s(g, "name").into(), meta, about }
    };
    let next_meta = |g: &Value| {
        let tasks = fmt::plural(i(g, "total"), "task", "tasks");
        after_ref(g).map(|r| format!("{tasks} · After {r}")).unwrap_or(tasks)
    };
    let next = of(GoalGroup::Next).into_iter().map(|g| line(g, next_meta(g))).collect();
    let paused = of(GoalGroup::Paused).into_iter().map(|g| line(g, format!("Paused · {}", progress(g)))).collect();
    let aside = of(GoalGroup::Deprioritized).into_iter().map(|g| line(g, progress(g))).collect();
    let mut fin = of(GoalGroup::Finished);
    let at = |g: &Value| fmt::parse(s(g, "finished_at")).map(|t| t.timestamp_millis()).unwrap_or(0);
    fin.sort_by_key(|g| std::cmp::Reverse(at(g)));
    let week = start_of_week(now);
    let this_week = |g: &Value| fmt::parse(s(g, "finished_at")).is_some_and(|t| fmt::local(t).date_naive() >= week);
    let done_line = |g: &Value| {
        let when = fmt::parse(s(g, "finished_at")).map(|t| day(t, now)).unwrap_or_default();
        let prs = match prs_merged(g) {
            0 => "No PR".into(),
            n => fmt::plural(n, "PR merged", "PRs merged"),
        };
        let prs = match i(g, "failed_accepted") {
            0 => prs,
            n => format!("{prs} · {n} failed (accepted)"),
        };
        line(g, if when.is_empty() { prs } else { format!("{prs} · {when}") })
    };
    vec![
        Shelf { key: "next", title: "Up next", lines: next, earlier: vec![] },
        Shelf { key: "paused", title: "Paused", lines: paused, earlier: vec![] },
        Shelf { key: "aside", title: "Set aside", lines: aside, earlier: vec![] },
        Shelf {
            key: "done",
            title: "Finished this week",
            lines: fin.iter().filter(|g| this_week(g)).map(|g| done_line(g)).collect(),
            earlier: fin.iter().filter(|g| !this_week(g)).map(|g| done_line(g)).collect(),
        },
    ]
    .into_iter()
    .filter(|sh| !sh.lines.is_empty() || !sh.earlier.is_empty())
    .collect()
}

/// (project, how many goals not finished), for the project tabs.
fn projects(goals: &[Value]) -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = vec![];
    for g in goals.iter().filter(|g| !fmt::b(g, "archived") && goal_group(g) != GoalGroup::Finished) {
        for project in goal_projects(g) {
            match v.iter_mut().find(|(p, _)| p == project) {
                Some((_, n)) => *n += 1,
                None => v.push((project.into(), 1)),
            }
        }
    }
    v.sort_by(|a, b| locale_cmp(&a.0, &b.0));
    v
}

// ================================================================== render

/// A flex column `grow` parts wide (the design's `fr`).
pub(crate) fn fr(d: Div, grow: f32) -> Div {
    let mut d = d.min_w_0();
    d.style().flex_grow = Some(grow);
    d.style().flex_shrink = Some(1.);
    d.style().flex_basis = Some(relative(0.).into());
    d
}

fn group_label(t: &Theme, title: &str, n: usize) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.))
        .font_weight(FontWeight::BOLD)
        .text_color(t.muted)
        .child(title.to_uppercase())
        .child(div().font_weight(FontWeight::MEDIUM).text_color(t.faint).child(n.to_string()))
}

fn action_btn(t: &Theme, r: &str, a: Action, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let gr = r.to_string();
    match a {
        Action::Pause => kit::btn_small(t, SharedString::from(format!("goals-pause-{r}")), "Pause").on_click(cx.listener(move |m, _, _, cx| {
            cx.stop_propagation();
            goal::set_paused(m, &gr, true, cx);
        })),
        Action::Continue(n) => kit::btn_primary(t, SharedString::from(format!("goals-continue-{r}")), "Continue")
            .h(px(24.))
            .px(px(10.))
            .rounded(px(6.))
            .text_size(px(12.))
            .on_click(cx.listener(move |m, _, _, cx| {
                cx.stop_propagation();
                m.post(format!("goals/{gr}/waves/{n}/continue"), serde_json::json!({}), cx, move |m, v, cx| {
                    let msg = if v["let_start"] == true { format!("Wave {n} can start") } else { "The goal goes on".into() };
                    m.toast(msg, false, cx);
                });
            })),
    }
}

fn goal_head(t: &Theme, row: &GoalRow, open: bool, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let (r, r2) = (row.r.clone(), row.r.clone());
    let (now_fg, _, _) = row.now_st.colors(t);
    let now_fg = if row.now_st == St::Queued { t.text_2 } else { now_fg };
    let goal_link = t.goal;
    let name = div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .id(SharedString::from(format!("goals-ref-{r}")))
                        .text_size(px(12.))
                        .font_family(t.mono_font.clone())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(goal_link)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .child(row.r.clone())
                        .on_click(cx.listener(move |m, _, _, cx| {
                            cx.stop_propagation();
                            goal::open(m, &r2, false, cx);
                        })),
                )
                .child(kit::chip(t, row.project.clone())),
        )
        .child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).truncate().child(row.name.clone()));
    let progress = div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(status::bar(t, &row.parts, 8.))
        .child(
            div()
                .flex()
                .justify_between()
                .gap(px(8.))
                .text_size(px(12.))
                .text_color(t.muted)
                .child(div().flex().gap(px(4.)).child(div().font_weight(FontWeight::BOLD).text_color(t.text).child(format!("{} of {}", row.done, row.total))).child(if row.review > 0 { format!("done · {} in review", row.review) } else { "done".into() }))
                .child(div().truncate().child(row.wave.clone())),
        );
    let now = div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .text_size(px(13.))
        .child(div().font_weight(FontWeight::SEMIBOLD).text_color(now_fg).truncate().child(row.now.clone()))
        .when(!row.next.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(t.muted).truncate().child(row.next.clone())));
    let mut act = div().flex().flex_none().w(px(110.)).justify_end();
    if let Some(a) = row.action {
        act = act.child(action_btn(t, &row.r, a, cx));
    }
    div()
        .id(SharedString::from(format!("goals-row-{}", row.r)))
        .flex()
        .items_center()
        .gap(px(20.))
        .px(px(20.))
        .py(px(14.))
        .cursor_pointer()
        .child(div().flex_none().w(px(20.)).text_size(px(12.)).text_color(t.muted).child(if open { "▾" } else { "▸" }))
        .child(fr(name, 2.2))
        .child(fr(progress, 2.))
        .child(fr(now, 2.4))
        .child(act)
        .on_click(cx.listener(move |m, _, _, cx| {
            if !m.goals.open.remove(&r) {
                m.goals.open.insert(r.clone());
            }
            m.refresh(cx);
        }))
}

fn wave_head(t: &Theme, title: String, meta: String) -> Div {
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
        .child(div().font_weight(FontWeight::BOLD).child(title))
        .when(!meta.is_empty(), |d| d.child(div().text_color(t.muted).child(meta)))
}

fn task_row(t: &Theme, c: &Value, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let r = s(c, "ref").to_string();
    let row = status::row(c);
    let (loud_fg, _, _) = row.st.colors(t);
    let (o1, o2) = (r.clone(), r.clone());
    let pr_url = c["pr"]["url"].as_str().filter(|u| !u.is_empty()).map(str::to_string);
    let mut act = div().flex().flex_none().w(px(84.)).justify_end();
    if row.st == St::Needs {
        act = act.child(kit::btn_small(t, SharedString::from(format!("goals-answer-{r}")), "Answer").on_click(cx.listener(move |m, _, _, cx| {
            cx.stop_propagation();
            m.open_task(o1.clone(), cx);
        })));
    } else if let (St::Review, Some(url)) = (row.st, pr_url) {
        act = act.child(kit::btn_small(t, SharedString::from(format!("goals-pr-{r}")), "Open PR").on_click(move |_, _, cx| {
            cx.stop_propagation();
            cx.open_url(&url);
        }));
    }
    let hover = t.panel_2;
    let link = t.accent;
    div()
        .id(SharedString::from(format!("goals-task-{r}")))
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
        .cursor_pointer()
        .on_click(cx.listener(move |m, _, _, cx| m.open_task(o2.clone(), cx)))
        .child(div().flex_none().child(status::chip(t, row.st)))
        .child(div().flex_none().w(px(36.)).text_size(px(12.)).font_family(t.mono_font.clone()).text_color(t.muted).child(r.clone()))
        .child(fr(
            div().flex().child(
                div()
                    .id(SharedString::from(format!("goals-task-title-{r}")))
                    .min_w_0()
                    .truncate()
                    .hover(move |s| s.text_color(link).underline())
                    .child(s(c, "title").to_string()),
            )
            .items_center()
            .gap(px(8.))
            .when(kit::is_design(c), |d| d.child(kit::design_pill(t))),
            1.,
        ))
        .child(fr(div().text_size(px(12.)).truncate().text_color(if row.loud { loud_fg } else { t.muted }).child(row.line), 1.2))
        .child(div().flex_none().w(px(96.)).whitespace_nowrap().text_size(px(12.)).text_color(t.muted).text_right().child(row.age))
        .child(act)
}

/// An open goal's tasks: under a header per wave, or in one list when it has no waves.
fn goal_body(t: &Theme, d: Option<&Value>, now: DateTime<Utc>, cx: &mut Context<MainWindow>) -> Div {
    let mut body = div().flex().flex_col();
    let Some(d) = d.filter(|d| d["tasks"].is_array()) else {
        return body.child(div().pl(px(INSET)).py(px(10.)).border_t_1().border_color(t.divider).text_size(px(12.)).text_color(t.muted).child("Loading…"));
    };
    let ws = waves(Some(d));
    if ws.is_empty() {
        let mut tasks: Vec<&Value> = arr(d, "tasks").iter().collect();
        tasks.sort_by_key(|c| status::row(c).st.rank());
        for c in tasks {
            body = body.child(task_row(t, c, cx));
        }
        return body;
    }
    for w in &ws {
        let n = i(w, "wave");
        let tasks = tasks_in(d, n);
        let name = s(w, "name");
        let title = if name.is_empty() { format!("Wave {n}") } else { format!("Wave {n} · {name}") };
        body = body.child(wave_head(t, title, wave_meta(w, &tasks, now)));
        for c in tasks {
            body = body.child(task_row(t, c, cx));
        }
    }
    let loose: Vec<&Value> = arr(d, "tasks").iter().filter(|c| c["wave"].as_i64().is_none()).collect();
    if !loose.is_empty() {
        body = body.child(wave_head(t, "No wave".into(), String::new()));
        for c in loose {
            body = body.child(task_row(t, c, cx));
        }
    }
    body
}

fn goal_card(t: &Theme, row: &GoalRow, d: Option<&Value>, open: bool, now: DateTime<Utc>, cx: &mut Context<MainWindow>) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(12.))
        .border_1()
        .border_color(if open { t.accent_line } else { t.border })
        .bg(t.card)
        .overflow_hidden()
        .child(goal_head(t, row, open, cx))
        .when(open, |c| c.child(goal_body(t, d, now, cx)))
}

/// `roomy` when the strip has the page to itself: each goal gets a second line saying what it's for.
fn shelf_tile(t: &Theme, sh: &Shelf, earlier: bool, roomy: bool, cx: &mut Context<MainWindow>) -> Div {
    let mut tile = div().flex().flex_col().gap(px(if roomy { 14. } else { 8. })).p(px(if roomy { 16. } else { 14. })).rounded(px(12.)).border_1().bg(t.card);
    tile = if sh.key == "next" { tile.border_dashed().border_color(t.border_2) } else { tile.border_color(t.border) };
    let mut lines: Vec<&Line> = sh.lines.iter().collect();
    if earlier {
        lines.extend(sh.earlier.iter());
    }
    for l in lines {
        let r = l.r.clone();
        let hover = t.text;
        let dot = (sh.key == "done").then(|| kit::dot(t.up, 7.));
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .children(dot)
            .child(div().flex_none().text_size(px(12.)).font_family(t.mono_font.clone()).font_weight(FontWeight::SEMIBOLD).text_color(t.goal).when(roomy, |d| d.w(px(REF_W))).child(l.r.clone()))
            .child(div().min_w_0().flex_1().truncate().font_weight(FontWeight::SEMIBOLD).text_color(t.text_2).hover(move |s| s.text_color(hover)).child(l.name.clone()))
            .child(div().flex_none().text_size(px(12.)).text_color(t.muted).child(l.meta.clone()));
        let about = (roomy && !l.about.is_empty()).then(|| {
            div().max_w(px(820.)).text_size(px(12.5)).line_height(relative(1.45)).text_color(t.muted).line_clamp(2).child(l.about.clone())
        });
        tile = tile.child(
            div()
                .id(SharedString::from(format!("goals-shelf-{}", l.r)))
                .flex()
                .flex_col()
                .gap(px(3.))
                .text_size(px(13.))
                .cursor_pointer()
                .child(head)
                .children(about.map(|a| a.ml(px(REF_W + 8. + if sh.key == "done" { 15. } else { 0. }))))
                .on_click(cx.listener(move |m, _, _, cx| goal::open(m, &r, false, cx))),
        );
    }
    if !sh.earlier.is_empty() {
        let label = if earlier { "Only this week".to_string() } else { format!("{} earlier", sh.earlier.len()) };
        tile = tile.child(div().flex().child(kit::link(t, "goals-earlier", label).text_size(px(12.)).on_click(cx.listener(|m, _, _, cx| {
            m.goals.earlier = !m.goals.earlier;
            cx.notify();
        }))));
    }
    tile
}

fn shelf_strip(t: &Theme, list: &[Shelf], earlier: bool, roomy: bool, cx: &mut Context<MainWindow>) -> Div {
    let mut strip = div().flex().items_start().gap(px(14.));
    for sh in list {
        let col = div().flex().flex_col().gap(px(10.)).child(group_label(t, sh.title, sh.lines.len())).child(shelf_tile(t, sh, earlier, roomy, cx));
        strip = strip.child(fr(col, 1.));
    }
    strip
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let now = fmt::now();
    let all: Vec<Value> = m.data.goals.iter().filter(|g| !fmt::b(g, "archived")).cloned().collect();
    let tabs = projects(&all);
    let mut project = m.goals.project().to_string();
    if project != "all" && !tabs.iter().any(|(p, _)| *p == project) && m.goals_loaded() {
        project = "all".into();
    }
    let shown: Vec<&Value> = all.iter().filter(|g| in_project(g, &project)).collect();
    let going = in_progress(&all, &project);

    let mut right = div().flex().flex_none().gap(px(6.));
    if !going.is_empty() {
        let refs = going.clone();
        right = right
            .child(kit::btn_small(&t, "goals-expand-all", "Expand all").on_click(cx.listener(move |m, _, _, cx| {
                m.goals.open.extend(refs.iter().cloned());
                m.refresh(cx);
            })))
            .child(kit::btn_small(&t, "goals-collapse-all", "Collapse all").on_click(cx.listener(|m, _, _, cx| {
                m.goals.open.clear();
                cx.notify();
            })));
    }
    let head = div()
        .flex()
        .items_center()
        .gap(px(16.))
        .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Goals"))
        .when(tabs.len() > 1, |d| {
            d.child(ip::project_filter(&t, PROJECT_FILTER, &project, &tabs, cx, |m, p, cx| {
                m.goals.project = p;
                m.refresh(cx);
            }))
        })
        .child(div().flex_1())
        .child(right);
    let mut page = div().flex().flex_col().gap(px(20.)).px(px(PAGE_X)).pt(px(22.)).pb(px(32.)).line_height(relative(1.4)).child(head);
    let filter = ip::render_project_filter(m, &t, PROJECT_FILTER, window, cx);
    let shell = div().id("goals-page").flex_1().min_w_0().h_full().overflow_y_scroll().restrict_scroll_to_axis().children(filter);

    if !m.goals_loaded() && all.is_empty() {
        let msg = if m.down.is_some() { "Can’t load the board." } else { "Loading…" };
        return shell.child(page.child(kit::empty(&t, msg))).into_any_element();
    }
    if shown.is_empty() {
        return shell.child(page.child(kit::empty(&t, "No goals"))).into_any_element();
    }
    if !going.is_empty() {
        let mut list = div().flex().flex_col().gap(px(8.)).child(div().mb(px(2.)).child(group_label(&t, "In progress", going.len())));
        for r in &going {
            let Some(g) = shown.iter().find(|g| fmt::ref_of(g, "G") == *r) else { continue };
            let d = m.goals.details.get(r);
            let row = goal_row(g, d);
            list = list.child(goal_card(&t, &row, d, m.goals.open.contains(r), now, cx));
        }
        page = page.child(list);
    }
    let strip = shelves(&shown, now);
    if !strip.is_empty() {
        page = page.child(shelf_strip(&t, &strip, m.goals.earlier, going.is_empty(), cx));
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

    fn task(r: &str, wave: i64, status: &str, extra: Value) -> Value {
        let mut c = json!({"id": r[1..].parse::<i64>().unwrap(), "ref": r, "title": format!("Task {r}"), "status": status, "wave": wave});
        for (k, v) in extra.as_object().unwrap() {
            c[k] = v.clone();
        }
        c
    }

    fn wave(n: i64, state: &str, total: i64, extra: Value) -> Value {
        let mut w = json!({"wave": n, "name": format!("W{n}"), "state": state, "total": total, "stop_after": false, "released_at": null});
        for (k, v) in extra.as_object().unwrap() {
            w[k] = v.clone();
        }
        w
    }

    #[test]
    fn goals_sort_into_progress_and_the_strip() {
        let gs = [
            goal(1, "web", 1, 3, json!({"active": 1})),
            goal(2, "web", 0, 2, json!({"deprioritized": true})),
            goal(3, "api", 2, 2, json!({"finished_at": noon(2026, 10, 12), "prs_planned": 2})),
            goal(4, "api", 1, 1, json!({"finished_at": noon(2026, 9, 30)})),
            goal(5, "api", 0, 4, json!({"after_blocker": "Its goal runs after G1 (Goal 1), which isn't finished yet"})),
            goal(6, "web", 1, 8, json!({"paused": true})),
            goal(7, "api", 0, 0, json!({"archived": true, "active": 1})),
        ];
        assert_eq!(in_progress(&gs, "all"), ["G1"]);
        assert!(in_progress(&gs, "api").is_empty());
        let refs: Vec<&Value> = gs.iter().filter(|g| !fmt::b(g, "archived")).collect();
        let now = fmt::parse(&noon(2026, 10, 13)).unwrap();
        let sh_all = shelves(&refs, now);
        let got: Vec<(&str, Vec<(&str, &str)>, usize)> = sh_all.iter().map(|sh| (sh.title, sh.lines.iter().map(|l| (l.r.as_str(), l.meta.as_str())).collect(), sh.earlier.len())).collect();
        assert_eq!(
            got,
            [
                ("Up next", vec![("G5", "4 tasks · After G1")], 0),
                ("Paused", vec![("G6", "Paused · 1 of 8")], 0),
                ("Set aside", vec![("G2", "0 of 2")], 0),
                ("Finished this week", vec![("G3", "2 PRs merged · yesterday")], 1),
            ]
        );
        assert_eq!(projects(&gs), [("api".to_string(), 1), ("web".to_string(), 3)]);
    }

    #[test]
    fn a_shelf_line_says_what_its_goal_is_for() {
        let gs = [
            goal(1, "web", 0, 2, json!({"tldr": "Short", "outcome": "Long"})),
            goal(2, "web", 0, 2, json!({"outcome": "Long"})),
            goal(3, "web", 0, 2, json!({"tldr": "Short"})),
            goal(4, "web", 0, 2, json!({})),
        ];
        let refs: Vec<&Value> = gs.iter().collect();
        let sh = shelves(&refs, Utc::now());
        let mut got: Vec<(&str, &str)> = sh[0].lines.iter().map(|l| (l.r.as_str(), l.about.as_str())).collect();
        got.sort();
        assert_eq!(got, [("G1", "Short"), ("G2", "Long"), ("G3", "Short"), ("G4", "")]);
    }

    #[test]
    fn a_goal_shows_under_every_project_its_tasks_are_in() {
        let gs = [goal(1, "web", 1, 3, json!({"active": 1, "projects": ["web", "api"]})), goal(2, "web", 0, 1, json!({"active": 1}))];
        assert_eq!(in_progress(&gs, "api"), ["G1"]);
        assert_eq!(in_progress(&gs, "web"), ["G1", "G2"]);
        assert_eq!(projects(&gs), [("api".to_string(), 1), ("web".to_string(), 2)]);
        assert_eq!(goal_row(&gs[0], None).project, "web + api");
    }

    #[test]
    fn a_task_in_review_isnt_counted_done_and_keeps_its_goal_in_progress() {
        let g = goal(1, "web", 14, 15, json!({"prs_open": [9], "review": 1}));
        assert_eq!(goal_group(&g), GoalGroup::Active);
        let row = goal_row(&g, None);
        assert_eq!((row.done, row.review, row.total), (14, 1, 15));
        assert_eq!(row.parts.iter().filter(|(_, n)| *n > 0).copied().collect::<Vec<_>>(), [(St::Done, 14), (St::Review, 1)]);
        assert_eq!(row.now, "1 in review");
    }

    #[test]
    fn a_folded_row_says_what_runs_now_and_what_waits() {
        let g = goal(1, "web", 2, 6, json!({"active": 3, "needs": 1}));
        let d = json!({"tasks": [
            task("T1", 1, "done", json!({})),
            task("T2", 1, "done", json!({})),
            task("T3", 2, "working", json!({})),
            task("T4", 2, "working", json!({})),
            task("T5", 2, "needs", json!({"needs_reason": "question", "question": "10 or 30?"})),
            task("T6", 3, "queued", json!({})),
        ], "waves": [wave(1, "done", 2, json!({})), wave(2, "running", 3, json!({})), wave(3, "waiting", 1, json!({}))]});
        let row = goal_row(&g, Some(&d));
        assert_eq!((row.done, row.total, row.wave.as_str()), (2, 6, "Wave 2 of 3"));
        assert_eq!((row.now.as_str(), row.now_st), ("2 working · 1 needs you", St::Needs));
        assert_eq!(row.next, "T5 waits on your answer");
        assert_eq!(row.action, Some(Action::Pause));
        assert_eq!(row.parts.iter().filter(|(_, n)| *n > 0).copied().collect::<Vec<_>>(), [(St::Done, 2), (St::Working, 2), (St::Needs, 1), (St::Queued, 1)]);
    }

    #[test]
    fn a_wave_stopped_for_review_offers_continue() {
        let g = goal(1, "web", 2, 6, json!({"stopped": "Wave 1 is done"}));
        let d = json!({"tasks": [task("T1", 1, "done", json!({})), task("T2", 2, "queued", json!({}))],
            "waves": [wave(1, "stopped", 1, json!({})), wave(2, "waiting", 4, json!({}))]});
        let row = goal_row(&g, Some(&d));
        assert_eq!((row.now.as_str(), row.next.as_str(), row.action), ("Stopped for your review", "Wave 1 done · 4 tasks in wave 2", Some(Action::Continue(1))));
        // Before its tasks are in, the summary still says it waits on you.
        let row = goal_row(&g, None);
        assert_eq!((row.now.as_str(), row.wave.as_str()), ("Stopped for your review", ""));
        assert_eq!(goal_row(&goal(2, "web", 0, 1, json!({"active": 1})), Some(&json!({"tasks": [], "waves": []}))).wave, "");
    }

    #[test]
    fn wave_headers_say_when_and_what_they_wait_for() {
        let now = fmt::parse(&noon(2026, 10, 15)).unwrap();
        let t = task("T1", 1, "done", json!({"finished_at": noon(2026, 10, 13)}));
        assert_eq!(wave_meta(&wave(1, "done", 1, json!({})), &[&t], now), "Done Tuesday");
        assert_eq!(wave_meta(&wave(2, "running", 1, json!({"stop_after": true})), &[], now), "Now · stops for your review after");
        assert_eq!(wave_meta(&wave(3, "waiting", 1, json!({})), &[], now), "After wave 2");
        assert_eq!(day(fmt::parse(&noon(2026, 10, 15)).unwrap(), now), "12:00 PM");
        assert_eq!(day(fmt::parse(&noon(2026, 10, 1)).unwrap(), now), "Oct 1");
    }
}
