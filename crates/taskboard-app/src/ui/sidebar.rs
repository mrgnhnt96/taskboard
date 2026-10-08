//! The left sidebar. On the goal page it is the web's goal list (`goalNavList`: Active /
//! Deprioritized / Finished, grouped by project); everywhere else it is the web's goals rail
//! (`goalsRailHtml`: the brand, "Goals", goals grouped by project with folding, the recently
//! completed group, the hover peek card and the resizable width). Above both sit the pages
//! (Board, Backlog, Sessions), which the web reached through links instead.
//!
//! The view-model functions here (`goal_counts`, `status_of`, `nav_status`, `ring`, `rail_view`,
//! `peek_view`, `nav_view`, `landing`, `pick_goal`, `pick_project`, …) are the web's logic one to
//! one; the parity tests run them against the web's own output.
use crate::app::{Filters, MainWindow, Page};
use crate::fmt::{self, arr, b, i, s};
use crate::theme::Theme;
use crate::ui::{kit, modals};
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::f32::consts::PI;

pub const RAIL_MIN: f32 = 220.;
pub const RAIL_MAX: f32 = 600.;
pub const RAIL_DEFAULT: f32 = 300.;
const PEEK_DELAY_MS: u64 = 300;
const PEEK_HIDE_MS: u64 = 150;

#[derive(Default)]
pub struct State {
    /// A width drag in progress: (mouse x at the start, width at the start).
    pub drag: Option<(f32, f32)>,
    /// The goal list's view and the goal it was chosen for (the web's `P.gnav`).
    pub gnav: Option<(String, String)>,
    /// The peek card: which goal and where.
    pub peek: Option<(String, Point<Pixels>)>,
    /// Bumped on every hover change, so a stale show/hide timer does nothing.
    pub hover_gen: u64,
    /// The pointer is over the peek card itself.
    pub over_peek: bool,
}

// ------------------------------------------------------------------ counts and status

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Counts {
    pub n: i64,
    pub done: i64,
    pub active: i64,
    pub queued: i64,
    pub prs: i64,
    pub finished: bool,
}

/// `awaitingMerge`: a done, not failed task whose PR is still open.
fn awaiting_merge(t: &Value) -> bool {
    let pr = &t["pr"];
    s(t, "status") == "done" && !b(t, "failed") && pr.is_object() && !pr["num"].is_null() && pr["state"].as_str().unwrap_or("OPEN").to_uppercase() == "OPEN"
}

/// `goalCounts`: from the goal's task list when it has one (goal detail), else its summary.
pub fn goal_counts(g: &Value) -> Counts {
    let c = match g.get("tasks").and_then(Value::as_array) {
        Some(tasks) => Counts {
            n: tasks.len() as i64,
            done: tasks.iter().filter(|t| s(t, "status") == "done").count() as i64,
            active: tasks.iter().filter(|t| matches!(s(t, "status"), "working" | "needs")).count() as i64,
            queued: tasks.iter().filter(|t| s(t, "status") == "queued").count() as i64,
            prs: tasks.iter().filter(|t| awaiting_merge(t)).count() as i64,
            finished: false,
        },
        None => Counts { n: i(g, "total"), done: i(g, "done"), active: i(g, "active"), queued: 0, prs: arr(g, "prs_open").len() as i64, finished: false },
    };
    Counts { finished: c.n > 0 && c.done == c.n && c.prs == 0, ..c }
}

/// `goalDone`.
pub fn finished(g: &Value) -> bool {
    goal_counts(g).finished
}

/// `agentsHeldWhy`: why no agent may start now ("After hours", "Out of usage"), or "".
pub fn held_why(state: &Value) -> &'static str {
    let h = &state["work_hours"];
    let five = arr(&state["usage"], "windows").iter().find(|w| s(w, "key") == "five_hour");
    if h.is_object() && b(h, "on") && !b(h, "open") {
        return "After hours";
    }
    if five.is_some_and(|w| w["pct"].as_f64().unwrap_or(0.) >= 100.) {
        return "Out of usage";
    }
    ""
}

#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub key: &'static str,
    pub label: String,
}

/// `goalStatus`: the rail's state of a goal (None when there's nothing to say).
pub fn status_of(g: &Value, c: &Counts, held: bool) -> Option<Status> {
    let st = |key, label: String| Some(Status { key, label });
    if c.finished {
        return None;
    }
    if c.n > 0 && c.done == c.n && c.prs > 0 {
        return st("queued", if c.prs > 1 { format!("{} PRs awaiting merge", c.prs) } else { "Awaiting merge".into() });
    }
    let queued = g.get("queued").and_then(Value::as_i64).unwrap_or(c.queued);
    let needs = i(g, "needs");
    if needs > 0 {
        return st("needs", format!("{} need{} you", fmt::plural(needs, "task", "tasks"), if needs == 1 { "s" } else { "" }));
    }
    if c.active > 0 {
        return st("working", "Running".into());
    }
    if i(g, "starting") > 0 {
        return st("starting", "Starting".into());
    }
    if b(g, "paused") {
        return st("paused", "Paused".into());
    }
    if queued == 0 {
        return None;
    }
    if i(g, "blocked") > 0 && i(g, "blocked") >= queued {
        return st("blocked", "Blocked".into());
    }
    if held {
        return st("held", "Queued until agents can start".into());
    }
    st("queued", "Queued".into())
}

/// `goalNavStatus`: the goal list's line for a goal: (kind, label); kinds run, warn, done, queued, idle.
pub fn nav_status(g: &Value, c: &Counts) -> (&'static str, String) {
    let needs = i(g, "needs");
    if c.n > 0 && c.done == c.n {
        return if c.prs > 0 {
            ("run", if c.prs > 1 { format!("{} PRs awaiting merge", c.prs) } else { "Awaiting merge".into() })
        } else {
            ("done", "Finished".into())
        };
    }
    if needs > 0 {
        return ("warn", if needs > 1 { format!("{needs} need you") } else { "Needs you".into() });
    }
    if b(g, "paused") {
        return ("warn", "Paused".into());
    }
    let running = c.active - needs;
    if running != 0 {
        return ("run", if running > 1 { format!("{running} running") } else { "Running".into() });
    }
    if i(g, "queued") > 0 {
        return if i(g, "blocked") > 0 { ("warn", "Blocked".into()) } else { ("queued", "Queued".into()) };
    }
    if c.n == 0 {
        return ("idle", "No tasks yet".into());
    }
    ("idle", if c.done > 0 { "Not running".into() } else { "Not started".into() })
}

/// Kept for other pages: the goal list's (label, tone) for a goal summary.
#[allow(dead_code)]
pub fn goal_status(g: &Value) -> (String, &'static str) {
    let (kind, label) = nav_status(g, &goal_counts(g));
    (label, match kind {
        "run" | "queued" => "accent",
        "warn" => "warn",
        "done" => "up",
        _ => "muted",
    })
}

/// `goalRing`: the done and done+active fractions of the circle, and the glyph inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    pub done: f32,
    pub active: f32,
    /// `tick` when finished, else the status key (working, starting, needs, paused, held, blocked, queued).
    pub glyph: Option<&'static str>,
}

pub fn ring(c: &Counts, st: Option<&Status>) -> Ring {
    let n = c.n.max(0) as f32;
    let (d, a) = if c.n > 0 { (c.done as f32 / n, (c.done + c.active) as f32 / n) } else { (0., 0.) };
    let glyph = if c.finished {
        Some("tick")
    } else {
        st.map(|s| s.key).filter(|k| matches!(*k, "working" | "starting" | "needs" | "paused" | "held" | "blocked" | "queued"))
    };
    Ring { done: d, active: a, glyph }
}

// ------------------------------------------------------------------ recently completed

/// `recentSince`: a day back, or back to Friday's start (local) on weekends and Mondays.
pub fn recent_since(now: DateTime<Utc>) -> DateTime<Utc> {
    let day_ago = now - Duration::days(1);
    let l = fmt::local(now);
    let wd = l.weekday().num_days_from_sunday() as i64;
    if !matches!(wd, 6 | 0 | 1) {
        return day_ago;
    }
    let since_friday = (wd + 2) % 7;
    let date = l.date_naive() - Duration::days(since_friday);
    let friday = l.offset().from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap_or_default()).single().map(|t| t.with_timezone(&Utc)).unwrap_or(day_ago);
    friday.min(day_ago)
}

/// `recentlyDone`.
pub fn recently_done(g: &Value) -> bool {
    fmt::parse(s(g, "finished_at")).is_some_and(|at| at >= recent_since(fmt::now()))
}

// ------------------------------------------------------------------ the rail

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub r: String,
    pub name: String,
    pub on: bool,
    /// Finished or deprioritized (greyed name).
    pub dim: bool,
    pub status: Option<Status>,
    /// "done/n".
    pub count: String,
    pub ring: Ring,
    /// The selected row also offers "Open the goal".
    pub open: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub project: String,
    pub open: bool,
    /// The board is filtered to this project.
    pub on: bool,
    /// Folded with a goal that needs you.
    pub dot: bool,
    pub count: usize,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rail {
    /// Active goals (not finished, not deprioritized).
    pub count: usize,
    pub groups: Vec<Group>,
    /// "No goals yet" / "Loading…" when there's no project at all.
    pub none: Option<&'static str>,
    /// Recently completed: (open, how many, rows when open).
    pub recent: Option<(bool, usize, Vec<Row>)>,
}

/// Every goal the rail knows (`allGoals`): the full list, else the state's, minus archived ones.
pub fn all_goals<'a>(state: Option<&'a Value>, goals: Option<&'a [Value]>) -> Vec<&'a Value> {
    let list: &[Value] = match goals {
        Some(g) => g,
        None => state.map(|s| arr(s, "goals")).unwrap_or(&[]),
    };
    list.iter().filter(|g| !b(g, "archived")).collect()
}

fn same_ref(g: &Value, r: &str) -> bool {
    !r.is_empty() && r != "all" && fmt::ref_of(g, "G") == fmt::ref_of(&json!(r), "G")
}

fn row(g: &Value, f: &Filters, held: bool) -> Row {
    let r = fmt::ref_of(g, "G");
    let c = goal_counts(g);
    let st = status_of(g, &c, held);
    let on = same_ref(g, &f.goal);
    Row {
        name: format!("{r} {}", s(g, "name")),
        on,
        dim: c.finished || b(g, "deprioritized"),
        count: format!("{}/{}", c.done, c.n),
        ring: ring(&c, st.as_ref()),
        status: st,
        open: on,
        r,
    }
}

/// `goalsRailHtml`, as data. `goals` is None before the goal list first answered.
pub fn rail_view(state: Option<&Value>, goals: Option<&[Value]>, f: &Filters, shut: &[String], recent_open: bool) -> Rail {
    let every = all_goals(state, goals);
    let held = state.is_some_and(|s| !held_why(s).is_empty());
    let active: Vec<&Value> = every.iter().copied().filter(|g| !finished(g) && !b(g, "deprioritized")).collect();
    let mut recent: Vec<&Value> = every.iter().copied().filter(|g| finished(g) && recently_done(g)).collect();
    recent.sort_by_key(|g| std::cmp::Reverse(fmt::parse(s(g, "finished_at")).map(|t| t.timestamp_millis()).unwrap_or(0)));
    let mut seen: Vec<String> = state.map(|s| arr(s, "session_projects").iter().filter_map(|p| p.as_str().map(str::to_string)).collect()).unwrap_or_default();
    for g in &active {
        if let Some(p) = fmt::opt_s(g, "project") {
            seen.push(p.to_string());
        }
    }
    if f.project != "all" {
        seen.push(f.project.clone());
    }
    seen.sort_by(|a, b| locale_cmp(a, b));
    seen.dedup();
    let groups = seen
        .iter()
        .map(|p| {
            let gs: Vec<&Value> = active.iter().copied().filter(|g| s(g, "project") == p).collect();
            let closed = shut.iter().any(|x| x == p) && !gs.iter().any(|g| same_ref(g, &f.goal));
            let needs: i64 = gs.iter().map(|g| i(g, "needs")).sum();
            Group {
                project: p.clone(),
                open: !closed,
                on: f.project == *p,
                dot: closed && needs > 0,
                count: gs.len(),
                rows: if closed { Vec::new() } else { gs.iter().map(|g| row(g, f, held)).collect() },
            }
        })
        // Unlike the web, a project with no active goals is left out, unless the board is
        // filtered to it (its header is the way back to every project).
        .filter(|g| g.count > 0 || g.on)
        .collect::<Vec<_>>();
    let none = groups.is_empty().then_some(if goals.is_some() || state.is_some() { "No goals yet" } else { "Loading…" });
    let recent = (!recent.is_empty()).then(|| {
        let open = recent_open || recent.iter().any(|g| same_ref(g, &f.goal));
        (open, recent.len(), if open { recent.iter().map(|g| row(g, f, held)).collect() } else { Vec::new() })
    });
    Rail { count: active.len(), groups, none, recent }
}

/// `String.prototype.localeCompare` (the forms' node-verified port).
fn locale_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    crate::ui::modals::locale_cmp(a, b)
}

/// `rail-project`: a project name filters the board to it (again: back to all); with a goal
/// picked it switches to the whole project.
pub fn pick_project(f: &Filters, p: &str) -> Filters {
    let mut f = f.clone();
    if f.goal != "all" {
        f.goal = "all".into();
        f.project = p.to_string();
    } else {
        f.project = if f.project == p { "all".into() } else { p.to_string() };
    }
    f
}

/// `railClear`.
pub fn rail_clear(f: &Filters) -> Filters {
    Filters { goal: "all".into(), project: "all".into(), done: f.done.clone() }
}

/// `railShut` (`tb.rail.shut`, a JSON array in a string).
pub fn rail_shut() -> Vec<String> {
    crate::prefs::get_str("tb.rail.shut").and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok()).unwrap_or_default()
}

/// `rail-fold`.
pub fn toggle_shut(p: &str) {
    let mut set = rail_shut();
    if let Some(i) = set.iter().position(|x| x == p) {
        set.remove(i);
    } else {
        set.push(p.to_string());
    }
    crate::prefs::set("tb.rail.shut", json!(serde_json::to_string(&set).unwrap_or_default()));
}

/// `railRecentOpen` (`tb.rail.recent`).
pub fn recent_open() -> bool {
    crate::prefs::get_str("tb.rail.recent").as_deref() == Some("open")
}

/// `rail-recent`.
pub fn toggle_recent() {
    crate::prefs::set("tb.rail.recent", json!(if recent_open() { "closed" } else { "open" }));
}

/// `railWidth` (`tb.rail.w`): the saved width clamped to 220–600, else 300.
pub fn rail_width() -> f32 {
    let v = crate::prefs::get("tb.rail.w");
    let n = match &v {
        Some(Value::String(t)) => js_number(t),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.),
        _ => 0.,
    };
    if n == 0. || n.is_nan() { RAIL_DEFAULT } else { (n as f32).clamp(RAIL_MIN, RAIL_MAX) }
}

/// JS `Number(str)` for the values the web saved ("" is 0, junk is NaN).
fn js_number(t: &str) -> f64 {
    let t = t.trim();
    if t.is_empty() { 0. } else { t.parse::<f64>().unwrap_or(f64::NAN) }
}

/// `setRailWidth(…, save)`: rounded and clamped.
pub fn save_rail_width(w: f32) -> f32 {
    let w = w.clamp(RAIL_MIN, RAIL_MAX).round();
    crate::prefs::set("tb.rail.w", json!(format!("{w}")));
    w
}

// ------------------------------------------------------------------ peek card

#[derive(Clone, Debug, PartialEq)]
pub struct PeekTask {
    pub r: String,
    pub title: String,
    pub why: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PeekView {
    /// "G1 Sign-in with passkeys".
    pub title: String,
    /// The held reason (shown in warn colours), if any.
    pub held: Option<&'static str>,
    /// "1 task needs you · 1 of 4 done · 1 planned".
    pub sub: String,
    /// (label, chip tone, tasks) for needs, working, failed, starting, blocked.
    pub groups: Vec<(&'static str, &'static str, Vec<PeekTask>)>,
}

const PEEK_GROUPS: [(&str, &str, &str); 5] =
    [("needs", "Needs you", "needs"), ("working", "Running", "working"), ("failed", "Failed", "failed"), ("starting", "Starting", "working"), ("blocked", "Blocked", "blocked")];

/// `goalPeekHtml`, as data.
pub fn peek_view(g: &Value, state: &Value) -> PeekView {
    let c = goal_counts(g);
    let why = held_why(state);
    let st = status_of(g, &c, !why.is_empty());
    let planned = if i(g, "planned") > 0 { format!(" · {} planned", i(g, "planned")) } else { String::new() };
    let st_part = st.as_ref().map(|st| format!("{} · ", if st.key == "held" { "Queued" } else { st.label.as_str() })).unwrap_or_default();
    let held = (st.is_some() && !why.is_empty()).then_some(why);
    PeekView {
        title: format!("{} {}", fmt::ref_of(g, "G"), s(g, "name")),
        held,
        sub: format!("{st_part}{} of {} done{planned}", c.done, c.n),
        groups: PEEK_GROUPS
            .iter()
            .filter_map(|(k, label, chip)| {
                let ts: Vec<PeekTask> = arr(g, "peek")
                    .iter()
                    .filter(|t| s(t, "status") == *k)
                    .map(|t| PeekTask { r: s(t, "ref").to_string(), title: s(t, "title").to_string(), why: fmt::opt_s(t, "why").map(str::to_string) })
                    .collect();
                (!ts.is_empty()).then_some((*label, *chip, ts))
            })
            .collect(),
    }
}

// ------------------------------------------------------------------ goal list (goal page)

#[derive(Clone, Debug, PartialEq)]
pub struct NavItem {
    pub r: String,
    pub name: String,
    pub current: bool,
    pub kind: &'static str,
    /// A coloured dot before the label (every kind but idle).
    pub dot: bool,
    pub label: String,
    /// "· 1 of 4 done".
    pub n: String,
    pub ring: Ring,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NavView {
    pub view: &'static str,
    /// (view, label, count).
    pub tabs: Vec<(&'static str, &'static str, usize)>,
    /// (project or "No project", items).
    pub groups: Vec<(String, Vec<NavItem>)>,
    pub empty: Option<&'static str>,
}

const NAV_VIEWS: [(&str, &str); 3] = [("active", "Active"), ("deprio", "Deprioritized"), ("done", "Finished")];

/// `gnavView`.
pub fn nav_kind(g: &Value) -> &'static str {
    if finished(g) {
        "done"
    } else if b(g, "deprioritized") {
        "deprio"
    } else {
        "active"
    }
}

/// The view the list shows (the web's `P.gnav` rule): the current goal's own view when the
/// current goal changed since the view was chosen, else the chosen view, else Active.
pub fn nav_view_for(goals: &[&Value], cur: Option<&str>, chosen: Option<&(String, String)>) -> &'static str {
    let g = cur.and_then(|c| goals.iter().find(|g| same_ref(g, c)));
    if let Some(g) = g {
        let r = fmt::ref_of(g, "G");
        if chosen.is_none_or(|(_, f)| *f != r) {
            return nav_kind(g);
        }
    }
    let v = chosen.map(|(v, _)| v.as_str()).unwrap_or("active");
    NAV_VIEWS.iter().map(|(k, _)| *k).find(|k| *k == v).unwrap_or("active")
}

/// `goalNavList`, as data.
pub fn nav_view(goals: &[&Value], cur: Option<&str>, view: &'static str) -> NavView {
    let set = |k: &str| -> Vec<&Value> {
        let mut v: Vec<&Value> = goals.iter().copied().filter(|g| if k == "done" { finished(g) } else { nav_kind(g) == k }).collect();
        if k == "done" {
            v.sort_by_key(|g| std::cmp::Reverse(fmt::parse(s(g, "finished_at")).map(|t| t.timestamp_millis()).unwrap_or(0)));
        }
        v
    };
    let tabs = NAV_VIEWS.iter().map(|(k, l)| (*k, *l, set(k).len())).collect();
    let list = set(view);
    let mut projects: Vec<String> = list.iter().map(|g| s(g, "project").to_string()).collect();
    projects.sort_by(|a, b| locale_cmp(a, b));
    projects.dedup();
    let groups = projects
        .iter()
        .map(|p| {
            let items = list
                .iter()
                .filter(|g| s(g, "project") == p)
                .map(|g| {
                    let c = goal_counts(g);
                    let (kind, label) = nav_status(g, &c);
                    let st = status_of(g, &c, false);
                    NavItem {
                        r: fmt::ref_of(g, "G"),
                        name: s(g, "name").to_string(),
                        current: cur.is_some_and(|c| same_ref(g, c)),
                        kind,
                        dot: kind != "idle",
                        label,
                        n: format!("· {} of {} done", c.done, c.n),
                        ring: ring(&c, st.as_ref()),
                    }
                })
                .collect();
            (if p.is_empty() { "No project".to_string() } else { p.clone() }, items)
        })
        .collect::<Vec<_>>();
    let empty = list.is_empty().then_some(match view {
        "deprio" => "No deprioritized goals",
        "done" => "No finished goals yet",
        _ => "No active goals",
    });
    NavView { view, tabs, groups, empty }
}

/// The goal "Goals" opens (the web's `#/goals` with no id): the board's goal filter, else the
/// first active goal by project, else the first goal.
pub fn landing(goals: &[&Value], filter_goal: &str) -> Option<String> {
    if let Some(g) = goals.iter().find(|g| same_ref(g, filter_goal)) {
        return Some(fmt::ref_of(g, "G"));
    }
    let mut active: Vec<&&Value> = goals.iter().filter(|g| nav_kind(g) == "active").collect();
    active.sort_by(|a, b| locale_cmp(s(a, "project"), s(b, "project")));
    active.first().map(|g| fmt::ref_of(g, "G")).or_else(|| goals.first().map(|g| fmt::ref_of(g, "G")))
}

// ------------------------------------------------------------------ drawing

fn ring_el(t: &Theme, r: Ring) -> impl IntoElement {
    let (track, act, done, tick_c) = (t.border_2, t.accent, t.up, t.card);
    let glyph_c = match r.glyph {
        Some("working" | "starting") => t.accent,
        Some("needs" | "blocked") => t.warn,
        Some("paused") => t.muted,
        _ => t.faint,
    };
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let c = bounds.center();
            let scale = bounds.size.width.as_f32() / 18.;
            let rad = 7. * scale;
            let at = |a: f32| point(c.x + px(rad * a.cos()), c.y + px(rad * a.sin()));
            let arc = |frac: f32, color: Hsla, window: &mut Window| {
                if frac <= 0. {
                    return;
                }
                let (from, to) = (-PI / 2., -PI / 2. + 2. * PI * frac.min(1.));
                let steps = ((to - from) / (2. * PI) * 64.).ceil().max(2.) as usize;
                let mut p = PathBuilder::stroke(px(2.5 * scale));
                p.move_to(at(from));
                for k in 1..=steps {
                    p.line_to(at(from + (to - from) * k as f32 / steps as f32));
                }
                if let Ok(p) = p.build() {
                    window.paint_path(p, color);
                }
            };
            arc(1., track, window);
            arc(r.active, act, window);
            arc(r.done, done, window);
            let u = |x: f32, y: f32| point(bounds.origin.x + px(x * scale), bounds.origin.y + px(y * scale));
            let line = |pts: &[(f32, f32)], w: f32, color: Hsla, window: &mut Window| {
                let mut p = PathBuilder::stroke(px(w * scale));
                p.move_to(u(pts[0].0, pts[0].1));
                for (x, y) in &pts[1..] {
                    p.line_to(u(*x, *y));
                }
                if let Ok(p) = p.build() {
                    window.paint_path(p, color);
                }
            };
            let fill_poly = |pts: &[(f32, f32)], color: Hsla, window: &mut Window| {
                let mut p = PathBuilder::fill();
                p.move_to(u(pts[0].0, pts[0].1));
                for (x, y) in &pts[1..] {
                    p.line_to(u(*x, *y));
                }
                p.close();
                if let Ok(p) = p.build() {
                    window.paint_path(p, color);
                }
            };
            match r.glyph {
                Some("tick") => line(&[(5.5, 9.2), (7.8, 11.5), (12.3, 6.7)], 2., tick_c, window),
                Some("working" | "starting") => fill_poly(&[(7.7, 6.4), (7.7, 11.6), (11.8, 9.)], glyph_c, window),
                Some("needs") => {
                    line(&[(9., 5.8), (9., 9.5)], 1.8, glyph_c, window);
                    line(&[(9., 12.), (9., 12.4)], 1.8, glyph_c, window);
                }
                Some("paused") => {
                    line(&[(7.6, 6.6), (7.6, 11.4)], 1.8, glyph_c, window);
                    line(&[(10.4, 6.6), (10.4, 11.4)], 1.8, glyph_c, window);
                }
                Some("blocked") => {
                    line(&[(6.6, 8.6), (11.4, 8.6), (11.4, 12.), (6.6, 12.), (6.6, 8.6)], 1.1, glyph_c, window);
                    line(&[(7.6, 8.6), (7.6, 7.7), (9., 6.3), (10.4, 7.7), (10.4, 8.6)], 1.1, glyph_c, window);
                }
                Some("held") => fill_poly(&[(10.6, 6.1), (8., 6.6), (6.6, 9.), (8., 11.6), (10.8, 12.), (12.6, 11.), (10.6, 10.4), (9.6, 9.), (10.6, 6.1)], glyph_c, window),
                Some("queued") => {
                    let mut p = PathBuilder::stroke(px(1.4 * scale));
                    let cc = u(9., 9.);
                    let rr = 3.1 * scale;
                    p.move_to(point(cc.x + px(rr), cc.y));
                    for k in 1..=24 {
                        let a = 2. * PI * k as f32 / 24.;
                        p.line_to(point(cc.x + px(rr * a.cos()), cc.y + px(rr * a.sin())));
                    }
                    if let Ok(p) = p.build() {
                        window.paint_path(p, glyph_c);
                    }
                    line(&[(9., 7.4), (9., 9.), (10.1, 9.8)], 1.4, glyph_c, window);
                }
                _ => {}
            }
        },
    )
    .size(px(22.))
    .flex_none()
}

fn chev(open: bool) -> &'static str {
    if open { "▾" } else { "▸" }
}

/// The hover peek: after 300 ms on a goal (at once when a card already shows); away for 150 ms
/// hides it unless the pointer moved onto the card.
fn on_goal_hover(m: &mut MainWindow, r: String, hovered: bool, window: &mut Window, cx: &mut Context<MainWindow>) {
    m.sidebar.hover_gen += 1;
    let seq = m.sidebar.hover_gen;
    if hovered {
        let p = window.mouse_position();
        let at = point(px(rail_width_now(m) + 8.), p.y - px(18.));
        if m.sidebar.peek.is_some() {
            m.sidebar.peek = Some((r, at));
            cx.notify();
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(PEEK_DELAY_MS)).await;
            let _ = this.update(cx, |m, cx| {
                if m.sidebar.hover_gen == seq {
                    m.sidebar.peek = Some((r, at));
                    cx.notify();
                }
            });
        })
        .detach();
    } else {
        hide_peek_soon(m, seq, cx);
    }
}

fn hide_peek_soon(m: &mut MainWindow, seq: u64, cx: &mut Context<MainWindow>) {
    let _ = m;
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(std::time::Duration::from_millis(PEEK_HIDE_MS)).await;
        let _ = this.update(cx, |m, cx| {
            if m.sidebar.hover_gen == seq && !m.sidebar.over_peek {
                m.sidebar.peek = None;
                cx.notify();
            }
        });
    })
    .detach();
}

fn rail_width_now(m: &MainWindow) -> f32 {
    let _ = m;
    rail_width()
}

fn rail_row(t: &Theme, r: &Row, cx: &mut Context<MainWindow>) -> Div {
    let hover = t.col;
    let rid = r.r.clone();
    let pick = div()
        .id(SharedString::from(format!("goal-pick-{}", r.r)))
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap(px(10.))
        .h(px(36.))
        .pl(px(10.))
        .pr(px(6.))
        .cursor_pointer()
        .child(ring_el(t, r.ring))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .font_weight(if r.on { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                .text_color(if r.dim { t.muted } else { t.text })
                .child(
                    StyledText::new(r.name.clone()).with_highlights([(0..r.r.len(), HighlightStyle { color: Some(t.faint), ..Default::default() })]),
                ),
        )
        .child(div().flex_none().text_size(px(12.)).text_color(t.faint).child(r.count.clone()))
        .when_some(r.status.clone(), |d, st| d.tooltip(kit::tip(st.label)))
        .on_hover(cx.listener({
            let rid = rid.clone();
            move |m, h: &bool, window, cx| on_goal_hover(m, rid.clone(), *h, window, cx)
        }))
        .on_click(cx.listener({
            let rid = rid.clone();
            move |m, _, _, cx| m.rail_pick_goal(&rid, cx)
        }));
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .pr(px(4.))
        .rounded(px(8.))
        .when(r.on, |d| d.bg(t.goal_tint).border_1().border_color(t.goal_line))
        .when(!r.on, |d| d.hover(move |s| s.bg(hover)))
        .child(pick)
        .when(r.open, |d| {
            let target = rid.clone();
            d.child(
                div()
                    .id(SharedString::from(format!("goal-open-{}", r.r)))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(26.))
                    .rounded(px(6.))
                    .bg(t.goal_soft)
                    .text_color(t.goal)
                    .cursor_pointer()
                    .child("→")
                    .tooltip(kit::tip("Open the goal"))
                    .on_click(cx.listener(move |m, _, _, cx| m.go(Page::Goal(target.clone()), cx))),
            )
        })
}

fn group_head(t: &Theme, g: &Group, cx: &mut Context<MainWindow>) -> Div {
    let (p1, p2) = (g.project.clone(), g.project.clone());
    div()
        .flex()
        .items_center()
        .mt(px(10.))
        .mb(px(4.))
        .rounded(px(8.))
        .when(g.on, |d| d.bg(t.accent_soft))
        .child(
            div()
                .id(SharedString::from(format!("rail-fold-{}", g.project)))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(px(28.))
                .h(px(30.))
                .text_color(t.faint)
                .cursor_pointer()
                .child(chev(g.open))
                .tooltip(kit::tip(format!("{} {} goals", if g.open { "Hide" } else { "Show" }, g.project)))
                .on_click(cx.listener(move |m, _, _, cx| {
                    toggle_shut(&p1);
                    cx.notify();
                    let _ = m;
                })),
        )
        .child(
            div()
                .id(SharedString::from(format!("rail-project-{}", g.project)))
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(px(8.))
                .h(px(30.))
                .pr(px(8.))
                .cursor_pointer()
                .text_size(px(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if g.on { t.accent } else { t.faint })
                .child(div().flex_1().min_w_0().truncate().child(g.project.to_uppercase()))
                .when(g.dot, |d| d.child(kit::dot(t.warn, 7.)))
                .child(div().text_size(px(12.)).font_weight(FontWeight::NORMAL).text_color(t.faint).child(g.count.to_string()))
                .tooltip(kit::tip(if g.on { "Show every project".to_string() } else { format!("Show only {}", g.project) }))
                .on_click(cx.listener(move |m, _, _, cx| m.rail_pick_project(&p2, cx))),
        )
}

fn rail_list(m: &mut MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Vec<AnyElement> {
    let goals = (m.goals_loaded()).then_some(m.data.goals.as_slice());
    let view = rail_view(m.data.state.as_ref(), goals, &m.filters, &rail_shut(), recent_open());
    let mut out: Vec<AnyElement> = Vec::new();
    out.push(
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(6.))
            .min_h(px(28.))
            .child(
                div()
                    .id("rail-goals-title")
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.8))
                    .child(kit::h3(t, "Goals ›"))
                    .tooltip(kit::tip("See every goal: active, deprioritized and finished"))
                    .on_click(cx.listener(|m, _, _, cx| m.open_goals(cx))),
            )
            .child(div().text_size(px(12.)).text_color(t.faint).child(view.count.to_string()))
            .child(div().flex_1())
            .child(kit::btn_small(t, "rail-new-goal", "+").w(px(26.)).px(px(0.)).tooltip(kit::tip("New goal")).on_click(cx.listener(|m, _, window, cx| modals::open_goal_form(m, None, window, cx))))
            .into_any_element(),
    );
    let mut list = div().id("goals-rail").flex().flex_col().gap(px(1.)).flex_1().min_h_0().overflow_y_scroll();
    for g in &view.groups {
        list = list.child(group_head(t, g, cx));
        for r in &g.rows {
            list = list.child(rail_row(t, r, cx));
        }
    }
    if let Some(none) = view.none {
        list = list.child(div().px(px(8.)).text_size(px(12.5)).text_color(t.muted).child(none));
    }
    out.push(list.into_any_element());
    if let Some((open, n, rows)) = view.recent {
        let mut rec = div().flex().flex_col().gap(px(1.)).pt(px(10.)).border_t_1().border_color(t.border).child(
            div()
                .id("rail-recent")
                .flex()
                .items_center()
                .cursor_pointer()
                .text_color(t.muted)
                .hover(|s| s.opacity(0.8))
                .child(div().flex().items_center().justify_center().w(px(28.)).h(px(30.)).text_color(t.faint).child(chev(open)))
                .child(div().flex_1().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).child("RECENTLY COMPLETED"))
                .child(div().pr(px(8.)).text_size(px(12.)).text_color(t.faint).child(n.to_string()))
                .on_click(cx.listener(|_, _, _, cx| {
                    toggle_recent();
                    cx.notify();
                })),
        );
        for r in &rows {
            rec = rec.child(rail_row(t, r, cx));
        }
        out.push(rec.into_any_element());
    }
    out
}

fn nav_list(m: &mut MainWindow, t: &Theme, cur: &str, cx: &mut Context<MainWindow>) -> Vec<AnyElement> {
    let goals: Vec<&Value> = m.data.goals.iter().filter(|g| !b(g, "archived")).collect();
    let view = nav_view_for(&goals, Some(cur), m.sidebar.gnav.as_ref());
    m.sidebar.gnav = Some((view.to_string(), cur.to_string()));
    let nv = nav_view(&goals, Some(cur), view);
    let mut out: Vec<AnyElement> = Vec::new();
    out.push(
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(6.))
            .child(div().flex_1().text_size(px(18.)).font_weight(FontWeight::BOLD).child("Goals"))
            .child(kit::btn_small(t, "gnav-new-goal", "+ New goal").on_click(cx.listener(|m, _, window, cx| modals::open_goal_form(m, None, window, cx))))
            .into_any_element(),
    );
    let tabs: Vec<String> = nv.tabs.iter().map(|(_, l, n)| format!("{l} {n}")).collect();
    let tab_refs: Vec<&str> = tabs.iter().map(String::as_str).collect();
    let on = nv.tabs.iter().position(|(k, _, _)| *k == nv.view).unwrap_or(0);
    let keys: Vec<&'static str> = nv.tabs.iter().map(|(k, _, _)| *k).collect();
    let cur_s = cur.to_string();
    out.push(
        kit::seg(t, "gnav-view", &tab_refs, on, |ix, item| {
            let (k, c) = (keys[ix], cur_s.clone());
            item.flex_1().justify_center().text_size(px(12.)).on_click(cx.listener(move |m, _, _, cx| {
                m.sidebar.gnav = Some((k.to_string(), c.clone()));
                cx.notify();
            }))
        })
        .into_any_element(),
    );
    let mut list = div().id("goal-nav").flex().flex_col().gap(px(10.)).flex_1().min_h_0().overflow_y_scroll();
    for (p, items) in &nv.groups {
        let mut grp = div().flex().flex_col().gap(px(4.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .pt(px(6.))
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.faint)
                .child(div().flex_1().truncate().child(p.to_uppercase()))
                .child(div().text_size(px(12.)).font_weight(FontWeight::NORMAL).child(items.len().to_string())),
        );
        for it in items {
            let color = match it.kind {
                "run" => t.accent,
                "warn" => t.warn,
                "done" => t.up,
                "queued" => t.accent,
                _ => t.faint,
            };
            let (r1, r2) = (it.r.clone(), it.r.clone());
            let hover = t.col;
            grp = grp.child(
                div()
                    .id(SharedString::from(format!("gitem-{}", it.r)))
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(it.current, |d| d.bg(t.goal_tint).border_1().border_color(t.goal_line))
                    .when(!it.current, |d| d.hover(move |s| s.bg(hover)))
                    .child(ring_el(t, it.ring))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .gap(px(2.))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(6.))
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(div().flex_none().text_color(t.faint).font_family(t.mono_font.clone()).text_size(px(11.5)).child(it.r.clone()))
                                    .child(div().min_w_0().truncate().child(it.name.clone())),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .text_size(px(12.))
                                    .text_color(color)
                                    .when(it.dot, |d| d.child(kit::dot(color, 6.)))
                                    .child(it.label.clone())
                                    .child(div().text_color(t.faint).child(it.n.clone())),
                            ),
                    )
                    .on_hover(cx.listener(move |m, h: &bool, window, cx| on_goal_hover(m, r1.clone(), *h, window, cx)))
                    .on_click(cx.listener(move |m, _, _, cx| m.go(Page::Goal(r2.clone()), cx))),
            );
        }
        list = list.child(grp);
    }
    if let Some(e) = nv.empty {
        list = list.child(div().px(px(10.)).child(kit::help(t, e)));
    }
    out.push(list.into_any_element());
    out
}

/// The peek card for the hovered goal (drawn above everything, next to the sidebar).
pub fn render_peek(m: &mut MainWindow, _window: &mut Window, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let (r, at) = m.sidebar.peek.clone()?;
    let g = m.data.goals.iter().find(|g| same_ref(g, &r))?.clone();
    let t = cx.global::<Theme>().clone();
    let pv = peek_view(&g, m.state());
    let mut card = kit::menu_box(&t, 320.)
        .id("goal-peek")
        .p(px(12.))
        .gap(px(10.))
        .max_h(px(480.))
        .overflow_y_scroll()
        .on_hover(cx.listener(|m, h: &bool, _, cx| {
            m.sidebar.over_peek = *h;
            if !*h {
                m.sidebar.hover_gen += 1;
                let seq = m.sidebar.hover_gen;
                hide_peek_soon(m, seq, cx);
            }
        }))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).child(
                        StyledText::new(pv.title.clone()).with_highlights([(0..fmt::ref_of(&g, "G").len(), HighlightStyle { color: Some(t.faint), ..Default::default() })]),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .text_size(px(12.))
                        .text_color(t.faint)
                        .when_some(pv.held, |d, h| d.child(div().text_color(t.warn_fg).font_weight(FontWeight::MEDIUM).child(h)).child("\u{a0}·\u{a0}"))
                        .child(pv.sub.clone()),
                ),
        );
    for (label, chip, tasks) in &pv.groups {
        let mut grp = div().flex().flex_col().gap(px(2.)).pt(px(8.)).border_t_1().border_color(t.border).child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .mb(px(2.))
                .child(kit::tone_pill(&t, chip, *label))
                .child(div().text_size(px(12.)).text_color(t.faint).child(tasks.len().to_string())),
        );
        for tk in tasks {
            let tr = tk.r.clone();
            let hover = t.col;
            grp = grp.child(
                div()
                    .id(SharedString::from(format!("peek-{}", tk.r)))
                    .flex()
                    .gap(px(8.))
                    .px(px(6.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(div().flex_none().min_w(px(30.)).font_family(t.mono_font.clone()).text_size(px(12.)).text_color(t.faint).child(tk.r.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .gap(px(1.))
                            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(tk.title.clone()))
                            .children(tk.why.clone().map(|w| div().text_size(px(12.)).text_color(t.muted).child(w))),
                    )
                    .on_click(cx.listener(move |m, _, _, cx| {
                        m.sidebar.peek = None;
                        m.open_task(tr.clone(), cx);
                    })),
            );
        }
        card = card.child(grp);
    }
    Some(deferred(anchored().position(at).snap_to_window_with_margin(px(8.)).child(card)).with_priority(3).into_any_element())
}

fn nav_item(t: &Theme, id: &str, label: &str, count: Option<i64>, alert: bool, on: bool) -> Stateful<Div> {
    let hover = t.panel_2;
    div()
        .id(SharedString::from(format!("nav-{id}")))
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(30.))
        .px(px(10.))
        .rounded(px(8.))
        .cursor_pointer()
        .text_size(px(13.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if on { t.accent_fg } else { t.text_2 })
        .when(on, |d| d.bg(t.accent_soft))
        .when(!on, |d| d.hover(move |s| s.bg(hover)))
        .child(div().flex_1().child(label.to_string()))
        .children(count.filter(|n| *n > 0).map(|n| {
            let (fg, bg) = if alert { (t.on_accent, t.warn) } else { (t.muted, t.seg) };
            kit::pill(fg, bg, n.to_string()).h(px(18.)).px(px(6.))
        }))
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let st = m.state().clone();
    let counts = &st["counts"];
    let page = m.page.clone();
    let sessions = arr(&st, "sessions").len() as i64;
    let width = m.sidebar.drag.map(|_| m.sidebar_width).unwrap_or_else(rail_width);
    let _ = window;

    let brand = div()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(4.))
        .child(div().flex_1().text_size(px(18.)).font_weight(FontWeight::BOLD).child("Task board"))
        .child(kit::btn_primary(&t, "new-task", "+ New task").h(px(26.)).px(px(10.)).text_size(px(12.5)).on_click(cx.listener(|m, _, window, cx| {
            modals::open_task_form(m, Default::default(), window, cx);
        })));
    let nav = div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(nav_item(&t, "board", "Board", Some(i(counts, "needs")), true, page == Page::Board).on_click(cx.listener(|m, _, _, cx| m.go(Page::Board, cx))))
        .child(nav_item(&t, "backlog", "Backlog", Some(i(counts, "untriaged")), false, page == Page::Backlog).on_click(cx.listener(|m, _, _, cx| m.go(Page::Backlog, cx))))
        .child(nav_item(&t, "sessions", "Sessions", Some(sessions), false, page == Page::Sessions).on_click(cx.listener(|m, _, _, cx| m.go(Page::Sessions, cx))))
        .child(nav_item(&t, "days", "Days", None, false, page == Page::Days).on_click(cx.listener(|m, _, _, cx| m.go(Page::Days, cx))));
    let body = match &page {
        Page::Goal(cur) => nav_list(m, &t, &cur.clone(), cx),
        _ => rail_list(m, &t, cx),
    };
    let grip = div()
        .id("rail-grip")
        .absolute()
        .top_0()
        .bottom_0()
        .right(px(-4.))
        .w(px(8.))
        .cursor_col_resize()
        .child(div().absolute().top_0().bottom_0().left(px(3.)).w(px(2.)).when(m.sidebar.drag.is_some(), |d| d.bg(t.accent)))
        .hover(|s| s.bg(gpui_kit::transparent_black()))
        .tooltip(kit::tip("Drag to resize · double-click to reset"))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |m, e: &MouseDownEvent, _, cx| {
                if e.click_count >= 2 {
                    crate::prefs::remove("tb.rail.w");
                    m.sidebar.drag = None;
                } else {
                    m.sidebar_width = rail_width();
                    m.sidebar.drag = Some((e.position.x.as_f32(), m.sidebar_width));
                }
                cx.stop_propagation();
                cx.notify();
            }),
        );

    div()
        .relative()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(width))
        .h_full()
        .bg(t.tint)
        .border_r_1()
        .border_color(t.border)
        // Room for the traffic lights (the title bar is transparent).
        .pt(px(44.))
        .px(px(10.))
        .pb(px(10.))
        .gap(px(12.))
        .child(brand)
        .child(nav)
        .children(body)
        .child(grip)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{
        Filters, Row, Status, goal_counts, pick_project, held_why, landing, nav_status, nav_view, nav_view_for, peek_view, rail_clear, rail_shut, rail_view, rail_width,
        recent_open, recent_since, ring, status_of, toggle_recent, toggle_shut,
    };
    use crate::fmt::{self, b, s};
    use crate::parity::golden;
    use serde_json::{Value, json};

    fn filters(v: &Value) -> Filters {
        Filters { project: s(v, "project").into(), goal: s(v, "goal").into(), done: s(v, "done").into() }
    }

    fn st_json(st: Option<Status>) -> Value {
        st.map(|s| json!({"key": s.key, "label": s.label})).unwrap_or(Value::Null)
    }

    fn row_json(r: &Row) -> Value {
        json!({"ref": r.r, "on": r.on, "dim": r.dim, "name": r.name, "status": r.status.as_ref().map(|s| s.label.clone()),
               "count": r.count, "glyph": r.ring.glyph, "open": r.open})
    }

    /// The web's dasharray lengths (`(frac * 2π7).toFixed(1)`).
    fn dash(frac: f32) -> Value {
        let v = ((frac as f64 * 2. * std::f64::consts::PI * 7.) * 10.).round() / 10.;
        if v.fract() == 0. { json!(v as i64) } else { json!(v) }
    }

    fn set_prefs(p: &Value) {
        crate::prefs::reset();
        for (k, v) in p.as_object().into_iter().flatten() {
            crate::prefs::set(k, v.clone());
        }
    }

    #[::core::prelude::v1::test]
    fn status_counts_ring_match_web() {
        let g = golden("chrome");
        g.only("goalStatus").check(|i| {
            let goal = &i["goal"];
            st_json(status_of(goal, &goal_counts(goal), !held_why(&i["state"]).is_empty()))
        });
        g.only("agentsHeldWhy").check(|i| json!(held_why(&i["state"])));
        g.only("goalCounts").check(|i| {
            let c = goal_counts(&i["goal"]);
            json!({"n": c.n, "done": c.done, "active": c.active, "queued": c.queued, "prs": c.prs, "finished": c.finished})
        });
        g.only("goalNavStatus").check(|i| {
            let (k, l) = nav_status(&i["goal"], &goal_counts(&i["goal"]));
            json!({"k": k, "label": l})
        });
        g.only("goalRing").check(|i| {
            let goal = &i["goal"];
            let c = goal_counts(goal);
            let r = ring(&c, status_of(goal, &c, false).as_ref());
            json!({"active": dash(r.active), "done": dash(r.done), "glyph": r.glyph})
        });
    }

    #[::core::prelude::v1::test]
    fn recent_window_matches_web() {
        golden("chrome").only("recentSince").check(|i| {
            let now = fmt::parse(i["now"].as_str().unwrap()).unwrap();
            json!(recent_since(now).format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        });
    }

    #[::core::prelude::v1::test]
    fn rail_matches_web() {
        // Native difference: projects with no active goals aren't listed (unless the board is
        // filtered to one), so the web's empty groups are dropped from what's expected.
        let mut g = golden("chrome").only("rail");
        for c in &mut g.cases {
            let e = &mut c["expect"];
            if let Some(gs) = e["groups"].as_array_mut() {
                let had = !gs.is_empty();
                gs.retain(|g| g["count"] != 0 || g["on"] == true);
                if had && gs.is_empty() {
                    e["none"] = json!("No goals yet");
                }
            }
        }
        g.check(|i| {
            set_prefs(&i["prefs"]);
            let state = (!i["state"].is_null()).then_some(&i["state"]);
            let goals = i["goals"].as_array().map(|v| v.as_slice());
            let v = rail_view(state, goals, &filters(&i["filter"]), &rail_shut(), recent_open());
            json!({
                "brand": "Task board New task",
                "head": if v.none.is_some() && state.is_none() && goals.is_none() { json!({"title": "Goals", "count": v.count}) } else { json!({"title": "Goals", "count": v.count}) },
                "groups": v.groups.iter().map(|g| json!({"project": g.project, "open": g.open, "on": g.on, "dot": g.dot, "count": g.count,
                    "rows": g.rows.iter().map(row_json).collect::<Vec<_>>()})).collect::<Vec<_>>(),
                "none": v.none,
                "recent": v.recent.as_ref().map(|(open, n, rows)| json!({"open": open, "count": n, "rows": rows.iter().map(row_json).collect::<Vec<_>>()})),
            })
        });
    }

    #[::core::prelude::v1::test]
    fn rail_actions_match_web() {
        let g = golden("chrome");
        let not_pick = crate::parity::Golden { cases: g.only("railAction").cases.into_iter().filter(|c| c["input"]["act"] != "goal-pick").collect() };
        not_pick.check(|i| {
            set_prefs(&i["prefs"]);
            let f = filters(&i["filter"]);
            let arg = i["arg"].as_str().unwrap_or("");
            let mut keep = vec!["B1".to_string()];
            let after = match i["act"].as_str().unwrap() {
                "rail-project" => {
                    keep.clear();
                    let f = pick_project(&f, arg);
                    f.save();
                    f
                }
                "rail-fold" => {
                    toggle_shut(arg);
                    f
                }
                "rail-recent" => {
                    toggle_recent();
                    f
                }
                a => panic!("{a}"),
            };
            let saved = crate::prefs::get("taskboard.filter").map(|v| serde_json::to_string(&v).unwrap());
            json!({"filter": {"project": after.project, "goal": after.goal, "done": after.done}, "keep": keep,
                   "shut": crate::prefs::get_str("tb.rail.shut"), "recent": crate::prefs::get_str("tb.rail.recent"), "saved": saved})
        });
        g.only("railClear").check(|i| {
            let f = rail_clear(&filters(&i["filter"]));
            json!({"filter": {"project": f.project, "goal": f.goal, "done": f.done}, "keep": []})
        });
        g.only("railWidth").check(|i| {
            crate::prefs::reset();
            if let Some(v) = i["saved"].as_str() {
                crate::prefs::set("tb.rail.w", json!(v));
            }
            let w = rail_width();
            if w.fract() == 0. { json!(w as i64) } else { json!((w as f64 * 10.).round() / 10.) }
        });
    }

    /// The goals the rail action cases were run with (the same fixture as the rail cases).
    fn g_goals() -> Vec<Value> {
        golden("chrome").cases.iter().find(|c| c["name"] == "rail default").unwrap()["input"]["goals"].as_array().unwrap().clone()
    }

    #[::core::prelude::v1::test]
    fn peek_matches_web() {
        golden("chrome").only("peek").check(|i| {
            let p = peek_view(&i["goal"], &i["state"]);
            let sub = match p.held {
                Some(h) => format!("{h} · {}", p.sub),
                None => p.sub.clone(),
            };
            json!({"title": p.title, "sub": sub, "groups": p.groups.iter().map(|(l, _, ts)| json!({"label": l, "count": ts.len(),
                "tasks": ts.iter().map(|t| json!({"ref": t.r, "title": t.title, "why": t.why})).collect::<Vec<_>>()})).collect::<Vec<_>>()})
        });
    }

    #[::core::prelude::v1::test]
    fn goal_list_matches_web() {
        let g = golden("chrome");
        g.only("goalNav").check(|i| {
            let goals: Vec<&Value> = i["goals"].as_array().unwrap().iter().collect();
            let cur = i["cur"].as_str();
            let chosen = i["view"].as_str().map(|v| (v.to_string(), cur.unwrap_or("").to_string()));
            let view = nav_view_for(&goals, cur, chosen.as_ref());
            let nv = nav_view(&goals, cur, view);
            json!({
                "tabs": nv.tabs.iter().map(|(k, l, n)| json!({"view": k, "on": *k == nv.view, "label": l, "count": n})).collect::<Vec<_>>(),
                "groups": nv.groups.iter().map(|(p, items)| json!({"project": p, "count": items.len(), "items": items.iter().map(|it|
                    json!({"ref": it.r, "current": it.current, "kind": it.kind, "dot": it.dot, "label": it.label, "n": it.n})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
                "empty": nv.empty,
            })
        });
        g.only("goalsLanding").check(|i| {
            let goals: Vec<&Value> = i["goals"].as_array().unwrap().iter().filter(|g| !b(g, "archived")).collect();
            json!(landing(&goals, i["filterGoal"].as_str().unwrap()))
        });
    }

    /// `goal-pick` runs through the board's own handler (the rail and the board share it).
    #[gpui_kit::test]
    fn goal_pick_matches_web(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = crate::parity::window(cx);
        let g = golden("chrome");
        let picks = crate::parity::Golden { cases: g.only("railAction").cases.into_iter().filter(|c| c["input"]["act"] == "goal-pick").collect() };
        assert!(!picks.cases.is_empty());
        let goals = g_goals();
        picks.check(|i| {
            set_prefs(&i["prefs"]);
            let arg = i["arg"].as_str().unwrap().to_string();
            let f = filters(&i["filter"]);
            let goals = goals.clone();
            w.update(cx, |m, _, cx| {
                m.data.goals = goals;
                m.filters = f;
                m.board.keep = vec!["B1".into()];
                m.rail_pick_goal(&arg, cx);
                let saved = crate::prefs::get("taskboard.filter").map(|v| serde_json::to_string(&v).unwrap());
                json!({"filter": {"project": m.filters.project, "goal": m.filters.goal, "done": m.filters.done}, "keep": m.board.keep,
                       "shut": crate::prefs::get_str("tb.rail.shut"), "recent": crate::prefs::get_str("tb.rail.recent"), "saved": saved})
            })
            .unwrap()
        });
    }

    #[gpui_kit::test]
    fn rail_clicks_filter_the_board(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = crate::parity::window(cx);
        w.update(cx, |m, _, cx| {
            m.board.keep = vec!["B1".into()];
            m.rail_pick_goal("G1", cx);
            assert_eq!((m.filters.goal.as_str(), m.filters.project.as_str()), ("G1", "webapp"));
            assert!(m.board.keep.is_empty(), "keepIssues cleared");
            assert_eq!(crate::prefs::get("taskboard.filter"), Some(json!({"project": "webapp", "goal": "G1", "done": "24h"})));
            m.rail_pick_goal("G1", cx);
            assert_eq!((m.filters.goal.as_str(), m.filters.project.as_str()), ("all", "all"));
            m.rail_pick_project("webapp", cx);
            assert_eq!(m.filters.project, "webapp");
        })
        .unwrap();
    }
}
