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
use crate::ui::stop::{self as stop_state, Step, Stop};
use crate::ui::{focus, goal, kit};
use chrono::{DateTime, Datelike, Duration, Utc};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

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
    /// The area picked, with the project it's in: it counts only while that project is picked.
    pub area: Option<(String, String)>,
    /// Older finished goals shown under this week's.
    pub earlier: bool,
    /// `GET /goals/:id` for each goal in progress, by ref.
    pub details: HashMap<String, Value>,
    /// Counts each change to `details`.
    rev: u64,
    built: Option<Rc<Built>>,
    rows: Option<Rows>,
}

impl State {
    pub fn project(&self) -> &str {
        if self.project.is_empty() { "all" } else { &self.project }
    }

    /// The area picked in the project picked.
    pub fn area(&self) -> Option<&str> {
        self.area.as_ref().filter(|(p, _)| p == self.project() && p != "all").map(|(_, a)| a.as_str())
    }

    /// Goal `r` in full, as `GET /goals/:id` answered.
    pub fn land(&mut self, r: String, g: Value) {
        self.details.insert(r, g);
        self.rev += 1;
    }
}

/// What the page asked for: the project and area picked, whether the goals have loaded, and the
/// details' revision.
type Asked = (String, Option<String>, bool, u64);

/// What the page shows of a goal list, built once per list, project and area picked and set of details.
struct Built {
    goals: Rc<Vec<Value>>,
    asked: Asked,
    /// Each project's tab and how many goals it has.
    tabs: Vec<(String, usize)>,
    /// The project shown: the one picked, or "all" when it has no goals left.
    project: String,
    /// The project shown's areas, with how many of its goals each has; none for "all".
    areas: Vec<(String, usize)>,
    /// The area shown: the one picked, or None when it has no goals left.
    area: Option<String>,
    /// No goal outside the archive, and none in the project shown.
    none: bool,
    none_shown: bool,
    going: Vec<String>,
    rows: Vec<GoalRow>,
    shelves: Vec<Shelf>,
}

impl Built {
    fn of(goals: &Rc<Vec<Value>>, details: &HashMap<String, Value>, asked: Asked, now: DateTime<Utc>) -> Built {
        let all: Vec<&Value> = goals.iter().filter(|g| !fmt::b(g, "archived")).collect();
        let tabs = projects(goals);
        let (picked, picked_area, loaded, _) = &asked;
        let mut project = picked.clone();
        if project != "all" && !tabs.iter().any(|(p, _)| *p == project) && *loaded {
            project = "all".into();
        }
        let areas = areas(goals, &project);
        let area = picked_area.clone().filter(|a| areas.iter().any(|(x, _)| x == a) || !*loaded);
        let shown: Vec<&Value> = all.iter().copied().filter(|g| in_project(g, &project) && area.as_ref().is_none_or(|a| in_area(g, &project, a))).collect();
        let going: Vec<String> = in_progress(goals, &project).into_iter().filter(|r| shown.iter().any(|g| fmt::ref_of(g, "G") == *r)).collect();
        let mut rows: Vec<GoalRow> = going.iter().filter_map(|r| shown.iter().find(|g| fmt::ref_of(g, "G") == *r)).map(|g| goal_row(g, details.get(&fmt::ref_of(g, "G")))).collect();
        // Goals stopped for the owner first, the worst stop first; the rest keep their order.
        rows.sort_by_key(|r| (r.stop.is_none(), r.stop.as_ref().map(|x| x.kind)));
        let mut shelves = shelves(&shown, now);
        // One area picked: every goal shown is in it, so the tags would all say the same.
        if area.is_some() {
            rows.iter_mut().for_each(|r| r.area.clear());
            shelves.iter_mut().flat_map(|sh| sh.lines.iter_mut().chain(sh.earlier.iter_mut())).for_each(|l| l.area.clear());
        }
        Built { goals: goals.clone(), asked, tabs, project, areas, area, none: all.is_empty(), none_shown: shown.is_empty(), going, rows, shelves }
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
        _ if stop_state::of(g).is_some() => GoalGroup::Active,
        _ if i(g, "done") == 0 && i(g, "active") == 0 && i(g, "needs") == 0 && i(g, "hand_closed") == 0 && i(g, "queued") == 0 && fmt::opt_s(g, "stopped").is_none() && fmt::arr(g, "prs_open").is_empty() => GoalGroup::Next,
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

/// Whether goal `g` is in `project`'s area `area`: a goal's area is one of its own project's.
fn in_area(g: &Value, project: &str, area: &str) -> bool {
    s(g, "project") == project && s(g, "area") == area
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
    pending: i64,
    stopped: i64,
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
            St::Pending => self.pending += 1,
            St::Stopped => self.stopped += 1,
            St::Queued => self.queued += 1,
            St::Planned => self.planned += 1,
            St::Done => self.done += 1,
            St::Failed => self.failed += 1,
        }
    }

    /// From the goal's tasks and the shared tasks of other goals it counts, else from its
    /// summary's counts (which already take in the shared ones).
    pub(crate) fn of(g: &Value, detail: Option<&Value>) -> Tally {
        let mut t = Tally::default();
        if let Some(d) = detail.filter(|d| d["tasks"].is_array()) {
            for c in arr(d, "tasks").iter().chain(arr(d, "shared")) {
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
            pending: 0,
            stopped: i(g, "hand_closed"),
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
        vec![(St::Done, self.done), (St::Pending, self.pending), (St::Review, self.review), (St::Working, self.working), (St::Needs, self.needs), (St::Failed, self.failed), (St::Stopped, self.stopped), (St::Queued, self.queued), (St::Planned, self.planned)]
    }
}

/// The buttons a goal row offers: Pause while it runs; a stopped goal's next step instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Pause,
    /// A stop's step: (label, what it does, the main one).
    Step(String, Step, bool),
}

/// One goal in progress, folded.
#[derive(Clone, Debug, PartialEq)]
pub struct GoalRow {
    pub r: String,
    pub name: String,
    pub project: String,
    /// Its area in its project; empty when it has none.
    pub area: String,
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
    pub actions: Vec<Action>,
    /// What the goal stopped for, when it waits on the owner.
    pub stop: Option<Stop>,
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
pub(crate) fn after_ref(g: &Value) -> Option<String> {
    let line = opt_s(g, "after_blocker")?;
    line.split(|c: char| !c.is_ascii_alphanumeric()).find(|w| w.len() > 1 && w.starts_with('G') && w[1..].chars().all(|c| c.is_ascii_digit())).map(str::to_string)
}

fn now_line(t: &Tally) -> (String, St) {
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
    if bits.is_empty() && t.pending > 0 {
        bits.push(format!("{} pending merge", t.pending));
    }
    if bits.is_empty() && t.stopped > 0 {
        bits.push(format!("{} stopped", t.stopped));
    }
    let st = if t.needs > 0 {
        St::Needs
    } else if t.failed > 0 {
        St::Failed
    } else if t.working > 0 {
        St::Working
    } else if t.review > 0 {
        St::Review
    } else if t.pending > 0 {
        St::Pending
    } else if t.stopped > 0 {
        St::Stopped
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
    let review: Vec<String> = rows.iter().filter(|(_, r)| matches!(r.st, St::Review | St::Pending)).map(|(t, r)| if r.line.is_empty() { s(t, "ref").to_string() } else { r.line.clone() }).collect();
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
    let stop = stop_state::of(d.filter(|d| d["tasks"].is_array()).unwrap_or(g));
    let (now, now_st, next) = match &stop {
        Some(st) => (st.kind.label(), if st.kind.bad() { St::Failed } else { St::Needs }, st.line.clone()),
        None => {
            let (now, st) = now_line(&tally);
            (now, st, next_line(g, d))
        }
    };
    let actions = match &stop {
        Some(st) => st.secondary.iter().map(|(l, x)| Action::Step(l.clone(), x.clone(), false)).chain([Action::Step(st.primary.0.clone(), st.primary.1.clone(), true)]).collect(),
        None => vec![Action::Pause],
    };
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
        area: s(g, "area").into(),
        parts: tally.parts(),
        done: i(g, "done"),
        review: tally.review,
        total: i(g, "total"),
        wave,
        now,
        now_st,
        next,
        actions,
        stop,
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
        "stopped" => format!("{} {}", stop_state::Kind::Review.glyph(), "Done · ready for review"),
        "failed" => stop_state::Kind::Failed.label(),
        "held" => stop_state::Kind::Held.label(),
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
    /// Its area in its project; empty when it has none.
    pub area: String,
    pub meta: String,
    /// The goal's tl;dr, else its outcome: shown when the strip has the page to itself.
    pub about: String,
    /// A paused goal's Resume, saying what it starts, and whether the goal has planned tasks.
    pub resume: Option<(String, bool)>,
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
        Line { r: fmt::ref_of(g, "G"), name: s(g, "name").into(), area: s(g, "area").into(), meta, about, resume: None }
    };
    let next_meta = |g: &Value| {
        let tasks = fmt::plural(i(g, "total"), "task", "tasks");
        after_ref(g).map(|r| format!("{tasks} · Starts after {r}")).unwrap_or(tasks)
    };
    let next = of(GoalGroup::Next).into_iter().map(|g| line(g, next_meta(g))).collect();
    let paused = of(GoalGroup::Paused)
        .into_iter()
        .map(|g| Line { resume: Some((goal::resume_label(g), i(g, "planned") > 0)), ..line(g, format!("‖ Paused · {}", progress(g))) })
        .collect();
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

/// `project`'s areas, by name, with how many of its goals not finished are in each: none for "all".
fn areas(goals: &[Value], project: &str) -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = vec![];
    if project == "all" {
        return v;
    }
    for g in goals.iter().filter(|g| !fmt::b(g, "archived") && goal_group(g) != GoalGroup::Finished && s(g, "project") == project) {
        let a = s(g, "area");
        if a.is_empty() {
            continue;
        }
        match v.iter_mut().find(|(x, _)| x == a) {
            Some((_, n)) => *n += 1,
            None => v.push((a.into(), 1)),
        }
    }
    v.sort_by(|a, b| locale_cmp(&a.0, &b.0));
    v
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

fn action_btn(t: &Theme, r: &str, ix: usize, a: Action, busy: bool, cx: &mut Context<MainWindow>) -> AnyElement {
    let gr = r.to_string();
    match a {
        Action::Pause => kit::btn_small(t, SharedString::from(format!("goals-pause-{r}")), "Pause")
            .tooltip(kit::tip("Nothing new starts; running tasks carry on"))
            .on_click(cx.listener(move |m, _, _, cx| {
                cx.stop_propagation();
                goal::set_paused(m, &gr, true, cx);
            }))
            .into_any_element(),
        Action::Step(label, step, main) => {
            let id = SharedString::from(format!("goals-step-{r}-{ix}"));
            let label = if busy && matches!(step, Step::Continue(_)) { "Sending…".to_string() } else { label };
            let b = if main { kit::button(t, id, label, kit::Kind::Primary, kit::Size::Small) } else { kit::btn_small(t, id, label) };
            if busy && matches!(step, Step::Continue(_)) {
                return kit::disabled(b).into_any_element();
            }
            b.on_click(cx.listener(move |m, e: &ClickEvent, _, cx| {
                cx.stop_propagation();
                goal::take_step(m, &gr, &step, false, e.position(), cx);
            }))
            .into_any_element()
        }
    }
}

fn goal_head(t: &Theme, row: &GoalRow, open: bool, busy: bool, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let (r, r2, r3) = (row.r.clone(), row.r.clone(), row.r.clone());
    let (now_fg, _, _) = row.now_st.colors(t);
    let now_fg = match &row.stop {
        Some(st) => st.kind.colors(t).0,
        None if row.now_st == St::Queued => t.text_2,
        None => now_fg,
    };
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
                    focus::stop(t, div().id(SharedString::from(format!("goals-ref-{r}"))))
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
                .child(kit::chip(t, row.project.clone()))
                .when(!row.area.is_empty(), |d| d.child(kit::chip(t, row.area.clone()))),
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
    let mut act = div().flex().flex_none().w(px(240.)).items_center().justify_end().gap(px(8.));
    if row.stop.as_ref().is_some_and(|x| x.review) {
        // Its review: the wave's tasks, open in place.
        act = act.child(kit::link(t, SharedString::from(format!("goals-review-{}", row.r)), "Review").text_size(px(12.)).on_click(cx.listener(move |m, _, _, cx| {
            cx.stop_propagation();
            m.goals.open.insert(r3.clone());
            m.refresh(cx);
        })));
    }
    for (ix, a) in row.actions.iter().cloned().enumerate() {
        act = act.child(action_btn(t, &row.r, ix, a, busy, cx));
    }
    focus::stop(t, div().id(SharedString::from(format!("goals-row-{}", row.r))))
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
    } else if let (St::Review | St::Pending, Some(url)) = (row.st, pr_url) {
        act = act.child(kit::btn_small(t, SharedString::from(format!("goals-pr-{r}")), "Open PR").on_click(move |_, _, cx| {
            cx.stop_propagation();
            cx.open_url(&url);
        }));
    }
    let hover = t.panel_2;
    let link = t.accent;
    focus::stop(t, div().id(SharedString::from(format!("goals-task-{r}"))))
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
        .child(div().flex_none().w(px(46.)).whitespace_nowrap().text_size(px(12.)).font_family(t.mono_font.clone()).text_color(t.muted).child(r.clone()))
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
            .when(kit::is_design(c), |d| d.child(kit::design_pill(t)))
            .when(kit::is_research(c), |d| d.child(kit::research_pill(t))),
            1.,
        ))
        .child(fr(div().text_size(px(12.)).truncate().text_color(if row.loud { loud_fg } else { t.muted }).child(row.line), 1.2))
        .child(div().flex_none().w(px(96.)).whitespace_nowrap().text_size(px(12.)).text_color(t.muted).text_right().child(row.age))
        .child(act)
}

/// The page as the list draws it, top to bottom. A goal card comes as a slice per head, wave
/// header and task row, and the finished goals from before this week as a line each, so Expand all
/// and a long Done shelf draw only what's in view.
#[derive(Clone, Debug, PartialEq)]
enum Item {
    Head,
    /// "Stopped" or "In progress", and how many goals it holds.
    Group(&'static str, usize),
    /// The head of row `k`'s card.
    Goal(usize),
    /// Row `k` is open and its tasks aren't in yet.
    Loading(usize),
    /// Row `k`'s header for a wave, or for the tasks in none.
    Wave(usize, Option<i64>),
    /// Row `k`'s task at this index of its goal's tasks.
    Task(usize, usize),
    /// What's next, paused, set aside and finished this week.
    Shelves,
    /// The finished goal from before this week at this index, under the strip's last tile.
    Earlier(usize),
    /// The end of that tile, with its "Only this week".
    EarlierFoot,
    End,
}

impl Item {
    /// The card row `k` an item is a slice of.
    fn card(&self) -> Option<usize> {
        match self {
            Item::Goal(k) | Item::Loading(k) | Item::Wave(k, _) | Item::Task(k, _) => Some(*k),
            _ => None,
        }
    }
}

/// The list and the items it was built from, with a key per item that changes when the item
/// would draw at another height.
struct Rows {
    state: ListState,
    keys: Vec<String>,
    items: Rc<Vec<Item>>,
    /// What the items were built from: the goals as built, the goals open and the earlier ones shown.
    from: (Rc<Built>, HashSet<String>, bool),
}

/// An open goal's items: under a header per wave, or in one run when it has no waves.
fn body_items(k: usize, d: Option<&Value>) -> Vec<Item> {
    let Some(d) = d.filter(|d| d["tasks"].is_array()) else { return vec![Item::Loading(k)] };
    let tasks = arr(d, "tasks");
    let ws = waves(Some(d));
    if ws.is_empty() {
        let mut ix: Vec<usize> = (0..tasks.len()).collect();
        ix.sort_by_key(|&c| status::row(&tasks[c]).st.rank());
        return ix.into_iter().map(|c| Item::Task(k, c)).collect();
    }
    let mut v = vec![];
    let run = |wave: Option<i64>, v: &mut Vec<Item>| {
        let ix: Vec<usize> = (0..tasks.len()).filter(|&c| tasks[c]["wave"].as_i64() == wave).collect();
        if wave.is_some() || !ix.is_empty() {
            v.push(Item::Wave(k, wave));
            v.extend(ix.into_iter().map(|c| Item::Task(k, c)));
        }
    };
    for w in &ws {
        run(Some(i(w, "wave")), &mut v);
    }
    run(None, &mut v);
    v
}

fn items(b: &Built, open: &HashSet<String>, earlier: bool, details: &HashMap<String, Value>) -> Vec<Item> {
    let mut v = vec![Item::Head];
    let stopped = b.rows.iter().filter(|r| r.stop.is_some()).count();
    for (title, rows) in [("Stopped", 0..stopped), ("In progress", stopped..b.rows.len())] {
        if rows.is_empty() {
            continue;
        }
        v.push(Item::Group(title, rows.len()));
        for k in rows {
            v.push(Item::Goal(k));
            let r = &b.rows[k].r;
            if open.contains(r) {
                v.extend(body_items(k, details.get(r)));
            }
        }
    }
    if let Some(done) = b.shelves.last() {
        v.push(Item::Shelves);
        if earlier && !done.earlier.is_empty() {
            v.extend((0..done.earlier.len()).map(Item::Earlier));
            v.push(Item::EarlierFoot);
        }
    }
    v.push(Item::End);
    v
}

/// The last slice of its card: the next item belongs to another card or none.
fn card_ends(items: &[Item], ix: usize) -> bool {
    items.get(ix + 1).and_then(Item::card) != items[ix].card()
}

fn key(b: &Built, items: &[Item], ix: usize) -> String {
    let row = |k: usize| &b.rows[k].r;
    let end = card_ends(items, ix);
    match &items[ix] {
        Item::Head => "head".into(),
        Item::Group(title, n) => format!("group:{title}:{n}"),
        Item::Goal(k) => format!("goal:{}:{end}", row(*k)),
        Item::Loading(k) => format!("loading:{}", row(*k)),
        Item::Wave(k, w) => format!("wave:{}:{w:?}", row(*k)),
        Item::Task(k, c) => format!("task:{}:{c}:{end}", row(*k)),
        // The strip's height follows its longest tile, and the second lines when roomy.
        Item::Shelves => {
            let tiles: Vec<String> = b.shelves.iter().map(|sh| sh.lines.iter().map(|l| if l.about.is_empty() { '1' } else { '2' }).collect()).collect();
            format!("shelves:{}:{}:{}", b.going.is_empty(), tiles.join("|"), items.get(ix + 1) == Some(&Item::Earlier(0)))
        }
        Item::Earlier(k) => format!("earlier:{k}"),
        Item::EarlierFoot => "earlier-foot".into(),
        Item::End => "end".into(),
    }
}

/// Keep the list in step with the page: what changed between the common start and end is spliced,
/// so the place scrolled to stays put.
fn sync(m: &mut MainWindow, b: &Rc<Built>) {
    let st = &mut m.goals;
    if st.rows.as_ref().is_some_and(|l| Rc::ptr_eq(&l.from.0, b) && l.from.1 == st.open && l.from.2 == st.earlier) {
        return;
    }
    let items = items(b, &st.open, st.earlier, &st.details);
    let keys: Vec<String> = (0..items.len()).map(|ix| key(b, &items, ix)).collect();
    let from = (b.clone(), st.open.clone(), st.earlier);
    let items = Rc::new(items);
    let Some(l) = &mut st.rows else {
        st.rows = Some(Rows { state: ListState::new(keys.len(), ListAlignment::Top, px(800.)), keys, items, from });
        return;
    };
    let head = l.keys.iter().zip(&keys).take_while(|(a, c)| a == c).count();
    let room = l.keys.len().min(keys.len()) - head;
    let tail = l.keys.iter().rev().zip(keys.iter().rev()).take(room).take_while(|(a, c)| a == c).count();
    if head + tail < l.keys.len() || keys.len() != l.keys.len() {
        l.state.splice(head..l.keys.len() - tail, keys.len() - head - tail);
    }
    *l = Rows { state: l.state.clone(), keys, items, from };
}

fn card_line(t: &Theme, row: &GoalRow, open: bool) -> Hsla {
    match &row.stop {
        _ if open => t.accent_line,
        Some(st) => st.kind.colors(t).2,
        None => t.border,
    }
}

/// One slice of a card: its sides on each, its top on the first and its bottom on the last.
fn card_slice(t: &Theme, line: Hsla, first: bool, last: bool, el: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .bg(t.card)
        .border_l_1()
        .border_r_1()
        .border_color(line)
        .overflow_hidden()
        .when(first, |d| d.border_t_1().rounded_t(px(12.)))
        .when(last, |d| d.border_b_1().rounded_b(px(12.)))
        .child(el)
}

/// The finished goals' tile's padding and the space between its lines.
fn tile_space(roomy: bool) -> (f32, f32) {
    if roomy { (16., 14.) } else { (14., 8.) }
}

fn shelf_line(t: &Theme, key: &str, l: &Line, roomy: bool, busy: bool, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let r = l.r.clone();
    let hover = t.text;
    let dot = (key == "done").then(|| kit::dot(t.up, 7.));
    let head = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .children(dot)
        .child(div().flex_none().whitespace_nowrap().text_size(px(12.)).font_family(t.mono_font.clone()).font_weight(FontWeight::SEMIBOLD).text_color(t.goal).when(roomy, |d| d.w(px(REF_W))).child(l.r.clone()))
        .child(div().min_w_0().flex_1().truncate().font_weight(FontWeight::SEMIBOLD).text_color(t.text_2).hover(move |s| s.text_color(hover)).child(l.name.clone()))
        .when(!l.area.is_empty(), |d| d.child(kit::chip(t, l.area.clone())))
        .child(div().flex_none().text_size(px(12.)).text_color(t.muted).child(l.meta.clone()))
        .when_some(l.resume.clone(), |d, (label, planned)| {
            let gr = l.r.clone();
            let id = SharedString::from(format!("goals-resume-{gr}"));
            let b = if busy { kit::disabled(kit::btn_small(t, id, "Sending…")) } else {
                kit::btn_small(t, id, label).on_click(cx.listener(move |m, _, _, cx| {
                    cx.stop_propagation();
                    goal::resume(m, &gr, planned, false, cx);
                }))
            };
            d.child(div().flex_none().child(b))
        });
    let about = (roomy && !l.about.is_empty()).then(|| div().max_w(px(820.)).text_size(px(12.5)).line_height(relative(1.45)).text_color(t.muted).line_clamp(2).child(l.about.clone()));
    focus::stop(t, div().id(SharedString::from(format!("goals-shelf-{}", l.r))))
        .flex()
        .flex_col()
        .gap(px(3.))
        .text_size(px(13.))
        .cursor_pointer()
        .child(head)
        .children(about.map(|a| a.ml(px(REF_W + 8. + if key == "done" { 15. } else { 0. }))))
        .on_click(cx.listener(move |m, _, _, cx| goal::open(m, &r, false, cx)))
}

fn earlier_link(t: &Theme, label: String, cx: &mut Context<MainWindow>) -> Div {
    div().flex().child(kit::link(t, "goals-earlier", label).text_size(px(12.)).on_click(cx.listener(|m, _, _, cx| {
        m.goals.earlier = !m.goals.earlier;
        cx.notify();
    })))
}

/// `roomy` when the strip has the page to itself: each goal gets a second line saying what it's for.
/// `open_below`: the tile's earlier goals follow as items of their own, so it has no bottom here.
fn shelf_tile(t: &Theme, sh: &Shelf, roomy: bool, open_below: bool, busy: &dyn Fn(&str, bool) -> bool, cx: &mut Context<MainWindow>) -> Div {
    let (pad, gap) = tile_space(roomy);
    let mut tile = div().flex().flex_col().gap(px(gap)).px(px(pad)).pt(px(pad)).bg(t.card).border_t_1().border_l_1().border_r_1().rounded_t(px(12.));
    tile = if sh.key == "next" { tile.border_dashed().border_color(t.border_2) } else { tile.border_color(t.border) };
    if !open_below {
        tile = tile.pb(px(pad)).border_b_1().rounded_b(px(12.));
    }
    for l in &sh.lines {
        let b = l.resume.as_ref().is_some_and(|(_, planned)| busy(&l.r, *planned));
        tile = tile.child(shelf_line(t, sh.key, l, roomy, b, cx));
    }
    if !sh.earlier.is_empty() && !open_below {
        tile = tile.child(earlier_link(t, format!("{} earlier", sh.earlier.len()), cx));
    }
    tile
}

fn shelf_strip(t: &Theme, list: &[Shelf], earlier: bool, roomy: bool, busy: &dyn Fn(&str, bool) -> bool, cx: &mut Context<MainWindow>) -> Div {
    let mut strip = div().flex().items_start().gap(px(14.));
    for sh in list {
        let open_below = earlier && sh.key == "done" && !sh.earlier.is_empty();
        let col = div().flex().flex_col().gap(px(10.)).child(group_label(t, sh.title, sh.lines.len())).child(shelf_tile(t, sh, roomy, open_below, busy, cx));
        strip = strip.child(fr(col, 1.));
    }
    strip
}

/// A piece of the strip's last tile, drawn under it: the columns before it stay empty.
fn under_last_tile(shelves: usize, el: Div) -> Div {
    let mut row = div().flex().gap(px(14.));
    for _ in 1..shelves {
        row = row.child(fr(div(), 1.));
    }
    row.child(fr(el, 1.))
}

fn page_head(t: &Theme, b: &Built, cx: &mut Context<MainWindow>) -> Div {
    let mut right = div().flex().flex_none().gap(px(6.));
    if !b.going.is_empty() {
        let refs = b.going.clone();
        right = right
            .child(kit::btn_small(t, "goals-expand-all", "Expand all").on_click(cx.listener(move |m, _, _, cx| {
                m.goals.open.extend(refs.iter().cloned());
                m.refresh(cx);
            })))
            .child(kit::btn_small(t, "goals-collapse-all", "Collapse all").on_click(cx.listener(|m, _, _, cx| {
                m.goals.open.clear();
                cx.notify();
            })));
    }
    div()
        .flex()
        .items_center()
        .gap(px(16.))
        .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Goals"))
        .when(b.tabs.len() > 1, |d| {
            d.child(ip::project_filter(t, PROJECT_FILTER, &b.project, &b.tabs, cx, |m, p, cx| {
                m.goals.project = p;
                m.refresh(cx);
            }))
        })
        .when(!b.areas.is_empty(), |d| {
            let all = b.tabs.iter().find(|(p, _)| *p == b.project).map_or(0, |(_, n)| *n) as i64;
            let areas: Vec<(String, i64)> = b.areas.iter().map(|(a, n)| (a.clone(), *n as i64)).collect();
            d.child(ip::area_chips(t, "goals-area", all, &areas, b.area.as_deref(), cx, |m, a, cx| {
                m.goals.area = a.map(|a| (m.goals.project().to_string(), a));
                m.refresh(cx);
            }))
        })
        .child(div().flex_1())
        .child(right)
}

fn item(m: &mut MainWindow, ix: usize, _: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let Some(l) = &m.goals.rows else { return div().into_any_element() };
    let (b, items) = (l.from.0.clone(), l.items.clone());
    let Some(it) = items.get(ix) else { return div().into_any_element() };
    let now = fmt::now();
    let open = |m: &MainWindow, k: usize| m.goals.open.contains(&b.rows[k].r);
    let slice = |m: &MainWindow, k: usize, el: AnyElement| card_slice(&t, card_line(&t, &b.rows[k], open(m, k)), matches!(it, Item::Goal(_)), card_ends(&items, ix), el);
    let (top, el): (f32, AnyElement) = match it {
        Item::Head => (22., page_head(&t, &b, cx).into_any_element()),
        Item::Group(title, n) => (20., div().mb(px(2.)).child(group_label(&t, title, *n)).into_any_element()),
        Item::Goal(k) => {
            let row = &b.rows[*k];
            let busy = row.actions.iter().any(|a| matches!(a, Action::Step(_, x, _) if goal::step_busy(m, &row.r, x)));
            let head = goal_head(&t, row, open(m, *k), busy, cx).into_any_element();
            (8., slice(m, *k, head).into_any_element())
        }
        Item::Loading(k) => {
            let el = div().pl(px(INSET)).py(px(10.)).border_t_1().border_color(t.divider).text_size(px(12.)).text_color(t.muted).child("Loading…");
            (0., slice(m, *k, el.into_any_element()).into_any_element())
        }
        Item::Wave(k, w) => {
            let d = m.goals.details.get(&b.rows[*k].r);
            let head = match (w, d) {
                (Some(n), Some(d)) => {
                    let wv = waves(Some(d)).into_iter().find(|x| i(x, "wave") == *n).unwrap_or_default();
                    let name = s(&wv, "name");
                    let title = if name.is_empty() { format!("Wave {n}") } else { format!("Wave {n} · {name}") };
                    wave_head(&t, title, wave_meta(&wv, &tasks_in(d, *n), now))
                }
                _ => wave_head(&t, "No wave".into(), String::new()),
            };
            (0., slice(m, *k, head.into_any_element()).into_any_element())
        }
        Item::Task(k, c) => {
            let d = m.goals.details.get(&b.rows[*k].r).cloned();
            let el = match d.as_ref().and_then(|d| arr(d, "tasks").get(*c)) {
                Some(task) => task_row(&t, task, cx).into_any_element(),
                None => div().into_any_element(),
            };
            (0., slice(m, *k, el).into_any_element())
        }
        Item::Shelves => {
            let busy = |r: &str, planned: bool| goal::resume_busy(m, r, planned);
            (20., shelf_strip(&t, &b.shelves, m.goals.earlier, b.going.is_empty(), &busy, cx).into_any_element())
        }
        Item::Earlier(k) => {
            let roomy = b.going.is_empty();
            let (pad, gap) = tile_space(roomy);
            let Some(done) = b.shelves.last() else { return div().into_any_element() };
            let Some(line) = done.earlier.get(*k) else { return div().into_any_element() };
            let first_gap = *k > 0 || !done.lines.is_empty();
            let el = div().px(px(pad)).when(first_gap, |d| d.pt(px(gap))).bg(t.card).border_l_1().border_r_1().border_color(t.border).child(shelf_line(&t, done.key, line, roomy, false, cx));
            (0., under_last_tile(b.shelves.len(), el).into_any_element())
        }
        Item::EarlierFoot => {
            let (pad, gap) = tile_space(b.going.is_empty());
            let el = div().px(px(pad)).pt(px(gap)).pb(px(pad)).bg(t.card).border_l_1().border_r_1().border_b_1().rounded_b(px(12.)).border_color(t.border).child(earlier_link(&t, "Only this week".into(), cx));
            (0., under_last_tile(b.shelves.len(), el).into_any_element())
        }
        Item::End => return div().h(px(32.)).into_any_element(),
    };
    div().px(px(PAGE_X)).pt(px(top)).child(el).into_any_element()
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let now = fmt::now();
    let goals = m.data.goals.clone();
    let asked = (m.goals.project().to_string(), m.goals.area().map(str::to_string), m.goals_loaded(), m.goals.rev);
    let b = match &m.goals.built {
        Some(b) if Rc::ptr_eq(&b.goals, &goals) && b.asked == asked => b.clone(),
        _ => {
            let b = Rc::new(Built::of(&goals, &m.goals.details, asked, now));
            m.goals.built = Some(b.clone());
            b
        }
    };
    let filter = ip::render_project_filter(m, &t, PROJECT_FILTER, window, cx);
    let shell = div().id("goals-page").relative().flex().flex_col().flex_1().min_w_0().h_full().children(filter);
    let empty = if !m.goals_loaded() && b.none {
        Some(if m.down.is_some() { "Can’t load the board." } else { "Loading…" })
    } else {
        b.none_shown.then_some("No goals")
    };
    if let Some(msg) = empty {
        let page = div().flex().flex_col().gap(px(20.)).px(px(PAGE_X)).pt(px(22.)).pb(px(32.)).line_height(relative(1.4)).child(page_head(&t, &b, cx));
        return shell.child(page.child(kit::empty(&t, msg))).into_any_element();
    }
    sync(m, &b);
    let Some(l) = &m.goals.rows else { return shell.into_any_element() };
    shell.child(list(l.state.clone(), cx.processor(item)).flex_1().min_h_0().line_height(relative(1.4))).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backend;
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
                ("Up next", vec![("G5", "4 tasks · Starts after G1")], 0),
                ("Paused", vec![("G6", "‖ Paused · 1 of 8")], 0),
                ("Set aside", vec![("G2", "0 of 2")], 0),
                ("Finished this week", vec![("G3", "2 PRs merged · yesterday")], 1),
            ]
        );
        assert_eq!(projects(&gs), [("api".to_string(), 1), ("web".to_string(), 3)]);
        // A paused goal's Resume says what it starts.
        assert_eq!(sh_all[1].lines[0].resume, Some(("Resume".to_string(), false)));
        let queued = goal(8, "web", 1, 4, json!({"paused": true, "queued": 3}));
        assert_eq!(shelves(&[&queued], now)[0].lines[0].resume, Some(("Resume: 3 tasks start".to_string(), false)));
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
    fn a_goal_bar_counts_the_shared_tasks_of_other_goals() {
        // Its summary's done and total take in the shared task, so its bar does too.
        let g = goal(1, "web", 1, 3, json!({"active": 1}));
        let d = json!({"tasks": [task("T1", 1, "done", json!({})), task("T2", 1, "working", json!({}))], "shared": [task("T9", 1, "working", json!({}))], "waves": []});
        let row = goal_row(&g, Some(&d));
        let n = |st: St| row.parts.iter().find(|(s, _)| *s == st).map(|(_, n)| *n).unwrap();
        assert_eq!((n(St::Done), n(St::Working)), (1, 2));
        assert_eq!(row.parts.iter().map(|(_, n)| n).sum::<i64>(), row.total);
        assert_eq!(row.now, "2 working");
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
    fn a_goal_summary_with_only_closed_terminals_reads_stopped_not_needs_you() {
        // The sidebar draws its bar from the summary alone: closed terminals come as hand_closed, not needs.
        let g = goal(1, "web", 1, 3, json!({"needs": 0, "hand_closed": 2}));
        assert_eq!(goal_group(&g), GoalGroup::Active);
        let row = goal_row(&g, None);
        assert_eq!((row.now.as_str(), row.now_st), ("2 stopped", St::Stopped));
        assert_eq!(Tally::of(&g, None).parts().iter().filter(|(_, n)| *n > 0).copied().collect::<Vec<_>>(), [(St::Done, 1), (St::Stopped, 2)]);
        let unstarted = goal(2, "web", 0, 2, json!({"hand_closed": 2}));
        assert_eq!(goal_group(&unstarted), GoalGroup::Active, "closed terminals mean it started");
    }

    #[test]
    fn a_goal_with_closed_terminals_reads_stopped_not_queued() {
        let g = goal(1, "web", 1, 3, json!({"needs": 2}));
        let closed = json!({"needs_reason": "lost", "lost": true, "closed_by_hand": true});
        let d = json!({"tasks": [task("T1", 1, "done", json!({})), task("T2", 1, "needs", closed.clone()), task("T3", 1, "needs", closed)],
                       "waves": [wave(1, "running", 3, json!({}))]});
        let row = goal_row(&g, Some(&d));
        assert_eq!((row.now.as_str(), row.now_st), ("2 stopped", St::Stopped));
        assert_eq!(row.parts.iter().filter(|(_, n)| *n > 0).copied().collect::<Vec<_>>(), [(St::Done, 1), (St::Stopped, 2)]);
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
        // A task waiting on the owner's answer stops the goal for them: its step, not Pause.
        assert_eq!((row.now.as_str(), row.now_st), ("? Needs your answer", St::Needs));
        assert_eq!(row.next, "T5 waits on your answer");
        assert_eq!(row.actions, [Action::Step("Answer T5".into(), Step::Task("T5".into()), true)]);
        let mut running = d.clone();
        running["tasks"][4]["status"] = json!("working");
        let row = goal_row(&goal(1, "web", 2, 6, json!({"active": 3})), Some(&running));
        assert_eq!((row.now.as_str(), row.stop.is_none()), ("3 working", true));
        assert_eq!(row.actions, [Action::Pause]);
        let row = goal_row(&g, Some(&d));
        assert_eq!(row.parts.iter().filter(|(_, n)| *n > 0).copied().collect::<Vec<_>>(), [(St::Done, 2), (St::Working, 2), (St::Needs, 1), (St::Queued, 1)]);
    }

    #[test]
    fn a_wave_stopped_for_review_offers_continue() {
        let stop = json!({"kind": "review", "wave": 1});
        let g = goal(1, "web", 2, 6, json!({"stopped": "Wave 1 is done", "stop": stop}));
        let d = json!({"stop": stop, "tasks": [task("T1", 1, "done", json!({})), task("T2", 2, "queued", json!({}))],
            "waves": [wave(1, "stopped", 1, json!({})), wave(2, "waiting", 4, json!({}))]});
        let row = goal_row(&g, Some(&d));
        assert_eq!((row.now.as_str(), row.next.as_str()), ("◐ Ready for review", "Wave 1 done · look it over"));
        assert_eq!(row.actions, [Action::Step("Continue to wave 2".into(), Step::Continue(1), true)]);
        // Before its tasks are in, the summary still says it waits on you.
        let row = goal_row(&g, None);
        assert_eq!((row.now.as_str(), row.wave.as_str()), ("◐ Ready for review", ""));
        assert_eq!(goal_row(&goal(2, "web", 0, 1, json!({"active": 1})), Some(&json!({"tasks": [], "waves": []}))).wave, "");
    }

    #[test]
    fn stopped_goals_sit_apart_first_and_a_failure_comes_first() {
        let gs = Rc::new(vec![
            goal(1, "web", 1, 3, json!({"active": 1})),
            goal(2, "web", 1, 3, json!({"active": 1, "stop": {"kind": "review", "wave": 1}})),
            goal(3, "web", 1, 3, json!({"active": 1, "stop": {"kind": "failed", "wave": 1, "tasks": ["T9"]}})),
            goal(4, "web", 0, 3, json!({"stop": {"kind": "held", "wave": 1}})),
        ]);
        let now = fmt::parse(&noon(2026, 10, 13)).unwrap();
        let b = Built::of(&gs, &HashMap::new(), ("all".into(), None, true, 0), now);
        let got: Vec<(&str, Option<stop_state::Kind>)> = b.rows.iter().map(|r| (r.r.as_str(), r.stop.as_ref().map(|x| x.kind))).collect();
        use stop_state::Kind::*;
        assert_eq!(got, [("G3", Some(Failed)), ("G4", Some(Held)), ("G2", Some(Review)), ("G1", None)]);
        let failed = &b.rows[0];
        assert_eq!(failed.actions, [Action::Step("Continue anyway".into(), Step::Continue(1), false), Action::Step("Open T9".into(), Step::Task("T9".into()), true)]);
        assert_eq!(failed.now_st, St::Failed);
    }

    #[test]
    fn a_projects_areas_narrow_its_goals_and_tag_them() {
        let gs = Rc::new(vec![
            goal(1, "rune", 1, 3, json!({"active": 1, "area": "Quill"})),
            goal(2, "rune", 1, 3, json!({"active": 1, "area": "Art"})),
            goal(3, "rune", 0, 2, json!({"area": "Quill"})),
            goal(4, "rune", 1, 3, json!({"active": 1})),
            goal(5, "web", 1, 3, json!({"active": 1, "area": "Quill"})),
            goal(6, "rune", 2, 2, json!({"area": "Remote", "finished_at": "2026-10-12T10:00:00Z"})),
        ]);
        let now = fmt::parse(&noon(2026, 10, 13)).unwrap();
        let all = Built::of(&gs, &HashMap::new(), ("all".into(), Some("Quill".into()), true, 0), now);
        assert!(all.areas.is_empty() && all.area.is_none(), "an area means nothing across every project");
        assert_eq!(all.rows.iter().find(|r| r.r == "G1").unwrap().area, "Quill", "a goal row carries its area");

        let rune = Built::of(&gs, &HashMap::new(), ("rune".into(), None, true, 0), now);
        // Finished goals count toward no area, as they count toward no project tab.
        assert_eq!(rune.areas, [("Art".to_string(), 1), ("Quill".to_string(), 2)]);
        assert_eq!(rune.going, ["G1", "G2", "G4"]);

        let quill = Built::of(&gs, &HashMap::new(), ("rune".into(), Some("Quill".into()), true, 0), now);
        assert_eq!(quill.going, ["G1"], "web's Quill goal isn't rune's");
        let next: Vec<&str> = quill.shelves.iter().flat_map(|sh| sh.lines.iter().map(|l| l.r.as_str())).collect();
        assert_eq!(next, ["G3"]);
        assert!(quill.rows.iter().all(|r| r.area.is_empty()) && quill.shelves.iter().flat_map(|sh| &sh.lines).all(|l| l.area.is_empty()), "one area picked: no tags");

        let gone = Built::of(&gs, &HashMap::new(), ("rune".into(), Some("Remote".into()), true, 0), now);
        assert_eq!((gone.area, gone.going.len()), (None, 3), "an area with no goals left shows all of the project");
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

    #[test]
    fn the_page_is_a_list_of_card_slices_and_lines() {
        let gs = Rc::new(vec![
            goal(1, "web", 0, 3, json!({"active": 1})),
            goal(2, "web", 0, 2, json!({"active": 1})),
            goal(3, "api", 2, 2, json!({"finished_at": noon(2026, 10, 12)})),
            goal(4, "api", 1, 1, json!({"finished_at": noon(2026, 9, 30)})),
            goal(5, "api", 1, 1, json!({"finished_at": noon(2026, 9, 29)})),
        ]);
        let now = fmt::parse(&noon(2026, 10, 13)).unwrap();
        let tasks = [task("T1", 1, "done", json!({})), task("T2", 2, "working", json!({})), task("T3", 1, "queued", json!({"wave": null}))];
        let details = HashMap::from([("G1".to_string(), json!({"tasks": tasks, "waves": [wave(1, "done", 1, json!({})), wave(2, "running", 1, json!({}))]}))]);
        let b = Built::of(&gs, &details, ("all".into(), None, true, 0), now);
        let open = HashSet::from(["G1".to_string(), "G2".to_string()]);
        use Item::*;
        let got = items(&b, &open, false, &details);
        assert_eq!(got, [Head, Group("In progress", 2), Goal(0), Wave(0, Some(1)), Task(0, 0), Wave(0, Some(2)), Task(0, 1), Wave(0, None), Task(0, 2), Goal(1), Loading(1), Shelves, End]);
        let ends: Vec<usize> = (0..got.len()).filter(|&ix| got[ix].card().is_some() && card_ends(&got, ix)).collect();
        assert_eq!(ends, [8, 10], "each card closes on its last slice");

        let folded = items(&b, &HashSet::new(), true, &details);
        assert_eq!(folded, [Head, Group("In progress", 2), Goal(0), Goal(1), Shelves, Earlier(0), Earlier(1), EarlierFoot, End], "the earlier goals come a line each");
        assert!(card_ends(&folded, 2) && card_ends(&folded, 3));
        let keys: Vec<String> = (0..folded.len()).map(|ix| key(&b, &folded, ix)).collect();
        assert_eq!(keys.iter().collect::<HashSet<_>>().len(), keys.len(), "a key each: {keys:?}");
    }

    #[gpui_kit::test]
    fn expand_all_draws_only_the_rows_in_view(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = crate::parity::window(cx);
        let draw = |cx: &mut gpui_kit::TestAppContext| {
            let _ = cx.update_window(w.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
            crate::parity::settle(cx);
        };
        w.update(cx, |m, _, cx| m.go(crate::app::Page::Goals, cx)).unwrap();
        crate::parity::settle(cx);
        w.update(cx, |m, _, cx| {
            m.data.goals = Rc::new((1..=40).map(|id| goal(id, "web", 0, 20, json!({"active": 20}))).collect());
            for id in 1..=40 {
                let tasks: Vec<Value> = (0..20).map(|k| task(&format!("T{}", id * 100 + k), 1, "working", json!({}))).collect();
                m.goals.land(format!("G{id}"), json!({"tasks": tasks, "waves": [wave(1, "running", 20, json!({}))]}));
                m.goals.open.insert(format!("G{id}"));
            }
            cx.notify();
        })
        .unwrap();
        draw(cx);
        let seen = |cx: &mut gpui_kit::TestAppContext| {
            w.update(cx, |m, _, _| {
                let l = m.goals.rows.as_ref().expect("the page is a list");
                let n = l.items.len();
                (n, l.state.item_count(), l.state.bounds_for_item(2).is_some(), (0..n).filter(|&ix| l.state.bounds_for_item(ix).is_some()).count())
            })
            .unwrap()
        };
        let (n, count, top, drawn) = seen(cx);
        assert_eq!((n, count), (40 * 22 + 3, n), "every goal head, wave header and task row is an item");
        assert!(top, "the first goal is drawn");
        assert!(drawn < 120, "only the rows in view and a buffer are drawn: {drawn} of {n}");

        w.update(cx, |m, _, cx| {
            m.goals.open.clear();
            cx.notify();
        })
        .unwrap();
        draw(cx);
        assert_eq!(seen(cx).0, 40 + 3, "Collapse all folds the cards back to their heads");
    }

    #[gpui_kit::test]
    fn the_shelves_are_built_once_per_answer_and_project(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = crate::parity::window(cx);
        let draw = |cx: &mut gpui_kit::TestAppContext| {
            let _ = cx.update_window(w.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
            crate::parity::settle(cx);
        };
        w.update(cx, |m, _, cx| m.go(crate::app::Page::Goals, cx)).unwrap();
        crate::parity::settle(cx);
        draw(cx);
        let built = |cx: &mut gpui_kit::TestAppContext| w.update(cx, |m, _, _| m.goals.built.clone().expect("the goals were built")).unwrap();
        let first = built(cx);
        draw(cx);
        assert!(Rc::ptr_eq(&first, &built(cx)), "a redraw keeps what was built");

        w.update(cx, |m, _, cx| {
            m.goals.project = "nowhere".into();
            cx.notify();
        })
        .unwrap();
        draw(cx);
        let picked = built(cx);
        assert!(!Rc::ptr_eq(&first, &picked), "picking a project builds the shelves again");

        w.update(cx, |m, _, cx| m.refresh(cx)).unwrap();
        crate::parity::settle(cx);
        draw(cx);
        assert!(Rc::ptr_eq(&picked, &built(cx)), "an unchanged answer keeps what was built");

        rec.board().post("goals", json!({"name": "Brushes", "project": "web"})).unwrap();
        w.update(cx, |m, _, cx| m.refresh(cx)).unwrap();
        crate::parity::settle(cx);
        draw(cx);
        assert!(!Rc::ptr_eq(&picked, &built(cx)), "a new answer builds the shelves again");
    }
}
