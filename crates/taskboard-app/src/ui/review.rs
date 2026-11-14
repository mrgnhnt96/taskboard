//! The Review page: one weekly time review (`GET /reviews/:id`) with the same numbers as
//! `tb review`, a picker for earlier weeks, and its suggested fixes as backlog issues. Read-only:
//! opening a review marks it seen (its Needs you item goes), and Plan a goal hands the open fixes
//! to the Backlog page, checked.
//!
//! Layout: the week and its key numbers against the review before; the suggested fixes, since
//! they're what to act on; where the time went (by category, by project, waiting on you); then
//! the slowest tasks and the commands behind them.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, s};
use crate::theme::{Theme, ThemeMode};
use crate::ui::kit;
use crate::ui::select::sel;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::HashSet;

const MENU_WEEK: &str = "review-week";
/// Weeks the week menu shows before it scrolls.
const WEEKS_SHOWN: usize = 12;
/// Tasks the Shipped card shows before its list scrolls, and each row's height.
const TASKS_SHOWN: usize = 10;
const SHIPPED_ROW: f32 = 34.;

#[derive(Default)]
pub struct State {
    /// The review shown; `None` for the newest.
    pub id: Option<i64>,
    /// Reviews already marked seen from here, so each is sent once.
    marked: HashSet<i64>,
    /// The Commands card's tab: 0 slow repeated, 1 timed out, 2 rerun.
    tab: usize,
}

// ------------------------------------------------------------------ view model

/// "3.2 h", "12 min", "40 s" (`tb review`'s `took`).
pub fn took(secs: f64) -> String {
    let secs = secs.abs();
    if secs >= 3600.0 {
        format!("{:.1} h", secs / 3600.0)
    } else if secs >= 60.0 {
        format!("{} min", (secs / 60.0).round() as i64)
    } else {
        format!("{} s", secs.round() as i64)
    }
}

fn secs(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// "Mon Oct 5 – Sun Oct 11": a review's week, in the reader's zone, ending on its last day (the
/// end is the next week's start, so the day before it).
pub fn period(r: &Value) -> String {
    let day = |k: &str, back: i64| fmt::parse(s(r, k)).map(|t| fmt::local(t - chrono::Duration::seconds(back)).format("%a %b %-d").to_string()).unwrap_or_default();
    format!("{} – {}", day("period_start", 0), day("period_end", 1))
}

/// A change against the review before: its words, which way it went (`None`: it didn't move) and
/// whether that's good (`None`: neither, as for agent time, where less can mean less got done).
#[derive(Clone, Debug, PartialEq)]
pub struct Delta {
    pub text: String,
    pub down: Option<bool>,
    pub good: Option<bool>,
}

impl Delta {
    fn same() -> Delta {
        Delta { text: "no change".into(), down: None, good: None }
    }
}

/// Less is better for every time here but agent time, which [`view`] makes neutral.
fn time_delta(d: f64) -> Option<Delta> {
    (d.abs() >= 60.0).then(|| Delta { text: format!("{} {}", took(d), if d < 0.0 { "less" } else { "more" }), down: Some(d < 0.0), good: Some(d < 0.0) })
}

fn count_delta(d: i64) -> Option<Delta> {
    (d != 0).then(|| Delta { text: format!("{} {}", d.abs(), if d < 0 { "fewer" } else { "more" }), down: Some(d < 0), good: Some(d < 0) })
}

/// One of the key numbers at the top: its value, a line of detail under it, or its change.
#[derive(Clone, Debug, PartialEq)]
pub struct Kpi {
    pub label: &'static str,
    pub value: String,
    pub sub: Option<String>,
    pub delta: Option<Delta>,
}

/// A release cut in the week: opens on its project's Releases page.
#[derive(Clone, Debug, PartialEq)]
pub struct ShippedRelease {
    pub r: String,
    pub name: String,
    pub headline: String,
    pub project: String,
}

/// A task finished in the week, with its PR's label and link when it has one.
#[derive(Clone, Debug, PartialEq)]
pub struct ShippedTask {
    pub r: String,
    pub title: String,
    pub pr: Option<(String, String)>,
}

/// What the week shipped: releases cut, goals finished (ref, name) and tasks done.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shipped {
    pub releases: Vec<ShippedRelease>,
    pub goals: Vec<(String, String)>,
    pub tasks: Vec<ShippedTask>,
}

impl Shipped {
    fn read(v: &Value) -> Shipped {
        let releases = arr(v, "releases")
            .iter()
            .map(|r| ShippedRelease {
                r: s(r, "ref").to_string(),
                name: s(r, "name").to_string(),
                headline: s(r, "notes").lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string(),
                project: s(r, "project").to_string(),
            })
            .collect();
        let goals = arr(v, "goals").iter().map(|g| (s(g, "ref").to_string(), s(g, "name").to_string())).collect();
        let tasks = arr(v, "tasks")
            .iter()
            .map(|t| {
                let url = s(t, "pr_url");
                let pr = (!url.is_empty()).then(|| (t["pr_num"].as_i64().map_or("PR".to_string(), |n| format!("#{n}")), url.to_string()));
                ShippedTask { r: s(t, "ref").to_string(), title: s(t, "title").to_string(), pr }
            })
            .collect();
        Shipped { releases, goals, tasks }
    }

    fn is_empty(&self) -> bool {
        self.releases.is_empty() && self.goals.is_empty() && self.tasks.is_empty()
    }
}

/// One row of a list: what it is, how long, a line under it, and its share of the list's longest.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// A task ref to open, when the row is a task.
    pub task: Option<String>,
    pub name: String,
    pub took: String,
    pub sub: String,
    pub share: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Category {
    pub name: String,
    pub took: String,
    pub share: f32,
    pub delta: Option<Delta>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fix {
    pub r: String,
    pub title: String,
    pub kind: String,
    /// "In G4", "Done", "Dropped": where it went once triaged; "" while it waits on the backlog.
    pub went: String,
    /// Open and in no goal: Plan a goal checks it.
    pub untriaged: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub id: i64,
    pub period: String,
    /// "vs Oct 5 – Oct 12" once there's a review to compare with.
    pub against: Option<String>,
    pub kpis: Vec<Kpi>,
    pub categories: Vec<Category>,
    pub projects: Vec<Line>,
    pub slowest: Vec<Line>,
    pub repeated: Vec<Line>,
    pub timeouts: Vec<Line>,
    pub reruns: Vec<Line>,
    /// Why tasks waited on the owner, and the tasks that waited longest.
    pub wait_reasons: Vec<Line>,
    pub wait_tasks: Vec<Line>,
    pub fixes: Vec<Fix>,
    /// While no fix is filed: the running line, else the plain suggestions.
    pub fixes_note: Option<String>,
    /// Filing them failed: why, and how to file one by hand.
    pub fixes_failed: Option<String>,
    pub suggestions: Vec<(String, String)>,
    pub shipped: Shipped,
}

impl View {
    /// The fixes the plan button checks on the Backlog page.
    pub fn plan(&self) -> Vec<String> {
        self.fixes.iter().filter(|f| f.untriaged).map(|f| f.r.clone()).collect()
    }

    /// "2 open · 1 in a goal · 1 closed": where the filed fixes stand.
    pub fn fixes_tally(&self) -> String {
        let open = self.fixes.iter().filter(|f| f.untriaged).count();
        let in_goal = self.fixes.iter().filter(|f| f.went.starts_with("In ")).count();
        let closed = self.fixes.len() - open - in_goal;
        [(open, "open"), (in_goal, "in a goal"), (closed, "closed")].iter().filter(|(n, _)| *n > 0).map(|(n, w)| format!("{n} {w}")).collect::<Vec<_>>().join(" · ")
    }
}

/// Sets each line's share of the longest line's time.
fn shares(mut lines: Vec<(Line, f64)>) -> Vec<Line> {
    let max = lines.iter().map(|(_, t)| *t).fold(0.0, f64::max);
    for (l, t) in &mut lines {
        l.share = if max > 0.0 { (*t / max) as f32 } else { 0.0 };
    }
    lines.into_iter().map(|(l, _)| l).collect()
}

pub fn view(r: &Value, list: &[Value]) -> View {
    let d = &r["data"];
    let c = &d["change"];
    let has_change = c.is_object();
    let total = secs(&d["seconds"]);
    let wait = &d["owner_wait"];
    let n_reruns = arr(d, "reruns").len();
    let shipped = Shipped::read(&d["shipped"]);
    // Against the review before, a number that didn't move says so rather than showing nothing.
    let kpi = |label, value: String, delta: Option<Delta>| Kpi { label, value, sub: None, delta: has_change.then(|| delta.unwrap_or_else(Delta::same)) };
    let made = Kpi {
        label: "Shipped",
        value: fmt::plural(shipped.tasks.len() as i64, "task", "tasks"),
        sub: Some(format!("{} · {}", fmt::plural(shipped.goals.len() as i64, "goal", "goals"), fmt::plural(shipped.releases.len() as i64, "release", "releases"))),
        delta: None,
    };
    let kpis = vec![
        made,
        kpi("Agent time", took(total), time_delta(secs(&c["seconds"])).map(|d| Delta { good: None, ..d })),
        kpi("Waiting on you", took(secs(&wait["seconds"])), time_delta(secs(&c["owner_wait"]))),
        kpi("Timeouts", fmt::i(&d["timeouts"], "count").to_string(), count_delta(c["timeouts"].as_i64().unwrap_or(0))),
        kpi("Failed, then rerun", n_reruns.to_string(), count_delta(c["reruns"].as_i64().unwrap_or(0))),
    ];
    let against = has_change.then(|| list.iter().find(|x| x["id"] == c["review"]).map(period)).flatten().map(|p| format!("vs {p}"));
    let categories = arr(d, "categories")
        .iter()
        .map(|x| {
            let name = s(x, "name");
            let was = arr(c, "categories").iter().find(|y| s(y, "name") == name).map(|y| secs(&y["seconds"]));
            Category { name: name.to_string(), took: took(secs(&x["seconds"])), share: if total > 0.0 { (secs(&x["seconds"]) / total) as f32 } else { 0.0 }, delta: was.and_then(time_delta) }
        })
        .collect();
    let line = |task: Option<&str>, name: &str, t: f64, sub: String| (Line { task: task.filter(|t| !t.is_empty()).map(str::to_string), name: name.to_string(), took: took(t), sub, share: 0.0 }, t);
    let projects = shares(arr(d, "projects").iter().map(|x| line(None, s(x, "name"), secs(&x["seconds"]), format!("mostly {}", s(&x["categories"][0], "name")))).collect());
    let slowest = shares(arr(d, "slowest_tasks").iter().map(|x| line(Some(s(x, "task")), s(x, "title"), secs(&x["seconds"]), format!("mostly {}", s(x, "top_category")))).collect());
    let repeated = shares(
        arr(d, "repeated")
            .iter()
            .map(|x| line(None, s(x, "command"), secs(&x["median"]), format!("median · p90 {} · {}", took(secs(&x["p90"])), fmt::plural(fmt::i(x, "runs"), "run", "runs"))))
            .collect(),
    );
    let timeouts = shares(arr(&d["timeouts"], "commands").iter().map(|x| line(None, s(x, "command"), secs(&x["seconds"]), fmt::plural(fmt::i(x, "count"), "timeout", "timeouts"))).collect());
    let reruns = shares(
        arr(d, "reruns")
            .iter()
            .map(|x| {
                let end = if x["passed"] == true { "then passed" } else { "never passed" };
                line(Some(s(x, "task")), s(x, "command"), secs(&x["seconds"]), format!("{} · {} runs, {end}", s(x, "task"), fmt::i(x, "runs")))
            })
            .collect(),
    );
    let wait_reasons = shares(arr(wait, "reasons").iter().map(|x| line(None, &fmt::cap(s(x, "name")), secs(&x["seconds"]), String::new())).collect());
    let wait_tasks = shares(arr(wait, "tasks").iter().map(|x| line(Some(s(x, "task")), s(x, "title"), secs(&x["seconds"]), String::new())).collect());
    let fixes: Vec<Fix> = arr(r, "fix_issues")
        .iter()
        .map(|b| {
            let state = s(b, "state");
            // The board sends the goal as `{id, ref, name}`.
            let goal = b["goal"].as_str().unwrap_or_else(|| s(&b["goal"], "ref"));
            let went = match state {
                "open" if goal.is_empty() => String::new(),
                "open" => format!("In {goal}"),
                st => fmt::cap(st),
            };
            Fix { r: fmt::ref_of(b, "B"), title: s(b, "title").to_string(), kind: s(b, "kind_label").to_string(), untriaged: went.is_empty(), went }
        })
        .collect();
    let f = &r["fixes"];
    let fixes_note = (fixes.is_empty() && s(f, "state") == "running").then(|| "Filing these as backlog issues…".to_string());
    let fixes_failed = (fixes.is_empty() && s(f, "state") == "failed")
        .then(|| format!("Couldn’t file these as backlog issues: {}. To file one by hand, run tb backlog add.", s(f, "error").trim_end_matches('.')));
    let suggestions = if fixes.is_empty() { arr(d, "suggestions").iter().map(|x| (s(x, "title").to_string(), s(x, "detail").to_string())).collect() } else { vec![] };
    View {
        id: r["id"].as_i64().unwrap_or(0),
        period: period(r),
        against,
        kpis,
        categories,
        projects,
        slowest,
        repeated,
        timeouts,
        reruns,
        wait_reasons,
        wait_tasks,
        fixes,
        fixes_note,
        fixes_failed,
        suggestions,
        shipped,
    }
}

/// The review to show: the picked one, else the newest in the list.
pub fn current(m: &MainWindow) -> Option<i64> {
    m.review.id.or_else(|| m.data.reviews.as_ref().and_then(|l| arr(l, "reviews").first()).and_then(|r| r["id"].as_i64()))
}

/// The first review's day and time ("Friday, Oct 23 · 5:00 PM") and how far off it is ("in 3 days"),
/// from `GET /reviews`' `next`; `None` while the schedule is off.
pub fn first_review(next: &str, now: chrono::DateTime<chrono::Utc>) -> Option<(String, String)> {
    let at = fmt::local(fmt::parse(next)?);
    let today = fmt::local(now).date_naive();
    let away = match (at.date_naive() - today).num_days() {
        ..=0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        n => format!("in {n} days"),
    };
    Some((at.format("%A, %b %-d · %-I:%M %p").to_string(), away))
}

/// The category palette, in order of time spent: the Days page's project colours, then teal, then grey.
fn category_color(mode: ThemeMode, index: usize) -> Hsla {
    crate::theme::chart(mode, index)
}

// ------------------------------------------------------------------ actions

/// Open review `id` (`None`: the newest), from a link, Home or the Days page.
pub fn open(m: &mut MainWindow, id: Option<i64>, cx: &mut Context<MainWindow>) {
    if m.review.id != id || id.is_none() {
        m.data.review = None;
        m.data.errs.review = None;
    }
    m.review.id = id;
    m.go(Page::Review, cx);
    m.refresh(cx);
}

fn pick(m: &mut MainWindow, id: i64, cx: &mut Context<MainWindow>) {
    m.menu = None;
    if current(m) != Some(id) {
        m.data.review = None;
        m.data.errs.review = None;
    }
    m.review.id = Some(id);
    m.refresh(cx);
}

/// Opening an unseen review clears its Needs you item and withdraws its notification.
fn mark_seen(m: &mut MainWindow, r: &Value, cx: &mut Context<MainWindow>) {
    let Some(id) = r["id"].as_i64() else { return };
    if !r["seen_at"].is_null() || !m.review.marked.insert(id) {
        return;
    }
    m.post(format!("reviews/{id}/seen"), json!({}), cx, |m, v, cx| {
        if m.data.review.as_ref().and_then(|r| r["id"].as_i64()) == v["id"].as_i64() {
            m.data.review = Some(v);
        }
        cx.notify();
    });
}

/// Plan fixes in Backlog: the Backlog page with the review's open fixes checked beside anything
/// already checked there; planning, Create and Start go on from there as for any picked issues.
fn plan(m: &mut MainWindow, refs: Vec<String>, cx: &mut Context<MainWindow>) {
    for r in refs {
        if !m.backlog.picked.contains(&r) {
            m.backlog.picked.push(r);
        }
    }
    m.backlog.project = "all".into();
    m.backlog.kinds = [true; 4];
    m.go(Page::Backlog, cx);
}

// ------------------------------------------------------------------ render

fn card(t: &Theme) -> Div {
    div().flex().flex_col().gap(px(12.)).p(px(16.)).bg(t.card).border_1().border_color(t.border).rounded(px(12.))
}

fn h2(t: &Theme, text: impl Into<SharedString>) -> Div {
    div().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(text.into())
}

/// A card's title, with what sits at its right (a count, a button, tabs).
fn head(t: &Theme, title: &str, right: Option<AnyElement>) -> Div {
    div().flex().items_center().gap(px(10.)).min_h(px(28.)).child(h2(t, title.to_string())).child(div().flex_1()).children(right)
}

/// A change: green when it went the good way, red the bad way, muted when neither or unmoved.
fn delta_el(t: &Theme, d: &Delta) -> Div {
    let fg = match d.good {
        Some(true) => t.up_fg,
        Some(false) => t.down,
        None => t.muted,
    };
    let arrow = d.down.map(|down| if down { "↓" } else { "↑" });
    div().flex().items_center().gap(px(3.)).text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(fg).children(arrow).child(d.text.clone())
}

/// A thin bar of `share` (0..1) on the theme's track.
fn bar(t: &Theme, share: f32, color: Hsla, h: f32) -> Div {
    div().w_full().h(px(h)).rounded_full().bg(t.seg).child(div().h_full().rounded_full().w(relative(share.clamp(0.0, 1.0).max(0.01))).bg(color))
}

/// A list row: name, its detail and time over a bar of its share, its time at the right.
/// Task rows open the task.
fn line_el(t: &Theme, id: String, l: &Line, mono: bool, color: Hsla, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let name = sel(t, SharedString::from(format!("{id}-name")), l.name.clone());
    let name = if mono { div().font_family(t.mono_font.clone()).text_size(px(12.5)) } else { div().text_size(px(13.)) }.min_w_0().overflow_hidden().text_ellipsis().child(name);
    let top = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .children(l.task.clone().map(|r| kit::mono(t, r).flex_none().text_color(t.muted)))
        .child(name.flex_1())
        .when(!l.sub.is_empty(), |d| d.child(div().flex_none().text_size(px(12.)).text_color(t.muted).child(l.sub.clone())))
        .child(div().flex_none().min_w(px(52.)).text_align(TextAlign::Right).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(l.took.clone()));
    let row = div()
        .id(SharedString::from(id))
        .flex()
        .flex_col()
        .gap(px(5.))
        .px(px(8.))
        .py(px(7.))
        .mx(px(-8.))
        .rounded(px(8.))
        .child(top)
        .child(bar(t, l.share, color, 4.));
    match l.task.clone() {
        Some(r) => {
            let h = t.panel_2;
            crate::ui::focus::stop(t, row).cursor_pointer().hover(move |d| d.bg(h)).on_click(cx.listener(move |m, _, _, cx| m.open_task(r.clone(), cx)))
        }
        None => row,
    }
}

fn lines(t: &Theme, key: &str, list: &[Line], mono: bool, color: Hsla, cx: &mut Context<MainWindow>) -> Div {
    div().flex().flex_col().gap(px(2.)).children(list.iter().enumerate().map(|(n, l)| line_el(t, format!("review-{key}-{n}"), l, mono, color, cx)))
}

fn kpis_el(t: &Theme, v: &View) -> Div {
    let any_line = v.kpis.iter().any(|k| k.delta.is_some() || k.sub.is_some());
    div().flex().flex_wrap().gap(px(12.)).children(v.kpis.iter().map(|k| {
        let line = k.delta.as_ref().map(|x| delta_el(t, x)).or_else(|| k.sub.clone().map(|s| div().text_size(px(12.)).text_color(t.muted).child(s)));
        card(t)
            .flex_1()
            .min_w(px(150.))
            .gap(px(4.))
            .child(div().text_size(px(12.)).text_color(t.muted).child(k.label))
            .child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).child(k.value.clone()))
            .when(any_line, |d| d.child(div().min_h(px(16.)).children(line)))
    }))
}

fn fixes_el(t: &Theme, v: &View, cx: &mut Context<MainWindow>) -> Option<Div> {
    if v.fixes.is_empty() && v.fixes_note.is_none() && v.fixes_failed.is_none() && v.suggestions.is_empty() {
        return None;
    }
    let refs = v.plan();
    let n = refs.len();
    let action = (n > 0).then(|| {
        kit::btn_primary(t, "review-plan", format!("Plan {} in Backlog", fmt::plural(n as i64, "fix", "fixes")))
            .tooltip(kit::tip("Checks them on the Backlog page, beside anything already checked there"))
            .on_click(cx.listener(move |m, _, _, cx| plan(m, refs.clone(), cx)))
            .into_any_element()
    });
    let tally = if v.fixes.is_empty() { (!v.suggestions.is_empty()).then(|| v.suggestions.len().to_string()) } else { Some(v.fixes_tally()) };
    let title = div().flex().items_center().gap(px(8.)).child(h2(t, "Suggested fixes")).children(tally.map(|n| kit::tone_pill(t, "", n)));
    let chip = |kind: String| div().flex_none().px(px(7.)).py(px(1.)).rounded(px(5.)).border_1().border_color(t.border_2).text_size(px(11.5)).text_color(t.text_2).child(kind);
    let rows = v.fixes.iter().map(|f| {
        let r = f.r.clone();
        crate::ui::focus::stop(t, div().id(SharedString::from(format!("review-fix-{}", f.r))))
            .flex()
            .items_start()
            .gap(px(10.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover({
                let h = t.panel_2;
                move |d| d.bg(h)
            })
            .when(!f.kind.is_empty(), |d| d.child(chip(f.kind.clone())))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .flex_1()
                    .min_w_0()
                    .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).overflow_hidden().text_ellipsis().when(!f.untriaged, |d| d.text_color(t.text_2)).child(f.title.clone()))
                    .when(!f.went.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(t.muted).child(f.went.clone()))),
            )
            .child(kit::mono(t, f.r.clone()).flex_none().text_size(px(12.)).text_color(t.muted))
            .on_click(cx.listener(move |m, _, _, cx| m.open_issue(r.clone(), cx)))
    });
    let failed = v.fixes_failed.clone().map(|e| {
        div()
            .flex()
            .items_start()
            .gap(px(8.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(8.))
            .bg(t.down_soft)
            .text_size(px(13.))
            .text_color(t.down)
            .child("⚠")
            .child(div().flex_1().min_w_0().child(sel(t, "review-fixes-failed", e)))
    });
    let suggestions = v.suggestions.iter().enumerate().map(|(n, (title, detail))| {
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(10.))
            .py(px(8.))
            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(sel(t, SharedString::from(format!("review-sugg-title-{n}")), title.clone())))
            .child(div().text_size(px(12.)).text_color(t.muted).child(sel(t, SharedString::from(format!("review-sugg-detail-{n}")), detail.clone())))
    });
    Some(
        card(t)
            .child(div().flex().items_center().gap(px(10.)).min_h(px(28.)).child(title).child(div().flex_1()).children(action))
            .children(v.fixes_note.clone().map(|n| div().text_size(px(13.)).text_color(t.muted).child(sel(t, "review-fixes-note", n))))
            .children(failed)
            .child(div().flex().flex_col().gap(px(2.)).children(rows).children(suggestions)),
    )
}

/// A Shipped row that opens something as a whole; `on` marks the one open in the panel.
fn shipped_row(t: &Theme, id: String, on: bool) -> Stateful<Div> {
    let (h, fill) = (t.panel_2, t.accent_soft);
    crate::ui::focus::stop(t, div().id(SharedString::from(id)))
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(SHIPPED_ROW))
        .px(px(10.))
        .rounded(px(6.))
        .text_size(px(13.))
        .cursor_pointer()
        .hover(move |d| d.bg(h))
        .when(on, move |d| d.bg(fill))
}

/// A finished task: the row opens it in the panel, its PR link opens the PR.
fn shipped_task(t: &Theme, task: &ShippedTask, open: bool, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let r = task.r.clone();
    let pr = task.pr.clone().map(|(label, url)| {
        kit::link(t, SharedString::from(format!("review-pr-{}", task.r)), label).text_size(px(12.)).font_family(t.mono_font.clone()).on_click(move |_, _, cx| {
            cx.stop_propagation();
            cx.open_url(&url)
        })
    });
    shipped_row(t, format!("review-shipped-{}", task.r), open)
        .child(kit::mono(t, task.r.clone()).w(px(52.)).flex_none().text_size(px(12.)).text_color(t.muted))
        .child(div().flex_1().min_w_0().truncate().child(task.title.clone()))
        .children(pr)
        .on_click(cx.listener(move |m, _, _, cx| m.open_task(r.clone(), cx)))
}

/// What the week shipped: releases (to their project's Releases page), goals finished (to the
/// goal's page) and tasks done (to the task panel). Past a few, the task list scrolls.
fn shipped_el(m: &MainWindow, t: &Theme, v: &View, cx: &mut Context<MainWindow>) -> Option<Div> {
    let sh = &v.shipped;
    if sh.is_empty() {
        return None;
    }
    let group = |title: &str| div().pt(px(6.)).pb(px(2.)).px(px(10.)).child(kit::h3(t, title.to_string()));
    let releases: Vec<_> = sh
        .releases
        .iter()
        .map(|r| {
            let (p, rr) = (r.project.clone(), r.r.clone());
            shipped_row(t, format!("review-shipped-{}-{}", r.project, r.name), false)
                .child(kit::mono(t, r.name.clone()).flex_none().font_weight(FontWeight::BOLD))
                .child(div().flex_1().min_w_0().truncate().child(r.headline.clone()))
                .child(div().flex_none().text_size(px(12.)).text_color(t.muted).child(r.project.clone()))
                .on_click(cx.listener(move |m, _, _, cx| {
                    if rr.is_empty() {
                        m.go(Page::ReleasesOf(p.clone()), cx);
                        m.refresh(cx);
                    } else {
                        crate::ui::releases::open_in(m, p.clone(), &rr, cx)
                    }
                }))
        })
        .collect();
    let goals: Vec<_> = sh
        .goals
        .iter()
        .map(|(g, name)| {
            let r = g.clone();
            shipped_row(t, format!("review-shipped-{g}"), false)
                .child(kit::mono(t, g.clone()).w(px(52.)).flex_none().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(t.goal))
                .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                .on_click(cx.listener(move |m, _, _, cx| crate::ui::goal::open(m, &r, false, cx)))
        })
        .collect();
    let tasks = (!sh.tasks.is_empty()).then(|| {
        let n = sh.tasks.len();
        if n <= TASKS_SHOWN {
            let open = crate::app::open_ref(m);
            return div().flex().flex_col().children(sh.tasks.iter().map(|x| shipped_task(t, x, open == Some(x.r.as_str()), cx))).into_any_element();
        }
        // Only the rows in view are drawn.
        let (list, tt) = (sh.tasks.clone(), t.clone());
        uniform_list(
            "review-shipped-tasks",
            n,
            cx.processor(move |m, range: std::ops::Range<usize>, _, cx| {
                let open = crate::app::open_ref(m).map(str::to_string);
                range.map(|k| shipped_task(&tt, &list[k], open.as_deref() == Some(list[k].r.as_str()), cx)).collect::<Vec<_>>()
            }),
        )
        .h(px(TASKS_SHOWN as f32 * SHIPPED_ROW))
        .into_any_element()
    });
    Some(
        card(t)
            .gap(px(4.))
            .child(head(t, "Shipped", None))
            .when(!releases.is_empty(), |d| d.child(group("Releases")).child(div().flex().flex_col().children(releases)))
            .when(!goals.is_empty(), |d| d.child(group("Goals finished")).child(div().flex().flex_col().children(goals)))
            .when(tasks.is_some(), |d| d.child(group("Tasks")).children(tasks)),
    )
}

/// Where the time went: one bar split by category, then each category with its change.
fn categories_el(t: &Theme, v: &View) -> Option<Div> {
    (!v.categories.is_empty()).then(|| {
        let c = |i| category_color(t.mode, i);
        let stacked = div().flex().gap(px(2.)).h(px(14.)).w_full().children(v.categories.iter().enumerate().filter(|(_, x)| x.share > 0.0).map(|(i, x)| div().h_full().rounded(px(3.)).w(relative(x.share)).bg(c(i))));
        let rows = v.categories.iter().enumerate().map(|(i, x)| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .py(px(6.))
                .border_t_1()
                .border_color(t.divider)
                .text_size(px(13.))
                .child(div().flex_none().size(px(10.)).rounded(px(3.)).bg(c(i)))
                .child(div().flex_1().min_w_0().truncate().child(fmt::cap(&x.name)))
                .child(div().w(px(110.)).flex().flex_none().justify_end().children(x.delta.as_ref().map(|d| delta_el(t, d))))
                .child(div().w(px(64.)).flex_none().text_align(TextAlign::Right).font_weight(FontWeight::SEMIBOLD).child(x.took.clone()))
                .child(div().w(px(40.)).flex_none().text_align(TextAlign::Right).text_size(px(12.)).text_color(t.muted).child(format!("{:.0}%", x.share * 100.0)))
        });
        card(t).child(head(t, "Where the time went", None)).child(stacked).child(div().flex().flex_col().children(rows))
    })
}

fn wait_el(t: &Theme, v: &View, cx: &mut Context<MainWindow>) -> Option<Div> {
    (!v.wait_reasons.is_empty()).then(|| {
        // Its total is the key number at the top; the card says where it went.
        let mut c = card(t).child(head(t, "Waiting on you", None)).child(lines(t, "wait", &v.wait_reasons, false, t.warn, cx));
        if !v.wait_tasks.is_empty() {
            c = c.child(div().h(px(1.)).bg(t.divider)).child(kit::h3(t, "Longest waits")).child(lines(t, "wait-task", &v.wait_tasks, false, t.warn, cx));
        }
        c
    })
}

fn commands_el(m: &MainWindow, t: &Theme, v: &View, cx: &mut Context<MainWindow>) -> Option<Div> {
    let tabs: Vec<(usize, String, &[Line])> = [(0, "Slow repeated", &v.repeated), (1, "Timed out", &v.timeouts), (2, "Rerun", &v.reruns)]
        .into_iter()
        .filter(|(_, _, l)| !l.is_empty())
        .map(|(i, name, l)| (i, format!("{name} {}", l.len()), l.as_slice()))
        .collect();
    if tabs.is_empty() {
        return None;
    }
    let on = tabs.iter().position(|(i, _, _)| *i == m.review.tab).unwrap_or(0);
    let labels: Vec<&str> = tabs.iter().map(|(_, l, _)| l.as_str()).collect();
    let keys: Vec<usize> = tabs.iter().map(|(i, _, _)| *i).collect();
    let seg = kit::seg(t, "review-cmd-tab", &labels, on, |n, item| {
        let k = keys[n];
        item.on_click(cx.listener(move |m, _, _, cx| {
            m.review.tab = k;
            cx.notify();
        }))
    });
    let color = if keys[on] == 0 { t.accent } else { t.down };
    Some(card(t).child(head(t, "Commands", Some(seg.into_any_element()))).child(lines(t, "cmd", tabs[on].2, true, color, cx)))
}

/// Day | Week | Recaps: the Days page, the weekly review and the recaps, from any of them.
/// `on` is the one shown.
pub fn day_week(t: &Theme, on: usize, cx: &mut Context<MainWindow>) -> Div {
    kit::seg(t, "day-week", &["Day", "Week", "Recaps"], on, |i, item| {
        item.on_click(cx.listener(move |m, _, _, cx| match i {
            0 if m.page != Page::Days => m.go(Page::Days, cx),
            1 if m.page != Page::Review => open(m, None, cx),
            2 if m.page != Page::Recap => crate::ui::recap::open(m, None, cx),
            _ => {}
        }))
    })
}

/// A review's cards under the header: its key numbers, the suggested fixes, where the time went
/// (with waiting on you and projects in the rail), then the commands.
fn body(m: &MainWindow, t: &Theme, v: &View, cx: &mut Context<MainWindow>) -> Div {
    let accent = t.accent;
    let projects = (!v.projects.is_empty()).then(|| card(t).child(head(t, "Agent time by project", None)).child(lines(t, "proj", &v.projects, false, accent, cx)));
    let slowest = (!v.slowest.is_empty()).then(|| card(t).child(head(t, "Slowest tasks", None)).child(lines(t, "slow", &v.slowest, false, accent, cx)));
    let left = div().flex().flex_col().gap(px(16.)).flex_1().min_w(px(420.)).children(fixes_el(t, v, cx)).children(shipped_el(m, t, v, cx)).children(categories_el(t, v));
    let rail = div().flex().flex_col().gap(px(16.)).w(px(340.)).flex_none().children(wait_el(t, v, cx)).children(slowest).children(projects);
    div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(kpis_el(t, v))
        .child(div().flex().flex_wrap().items_start().gap(px(16.)).child(left).child(rail))
        .children(commands_el(m, t, v, cx))
}

fn page(header: Div) -> Div {
    div().flex().flex_col().gap(px(16.)).w_full().max_w(px(1180.)).px(px(28.)).py(px(22.)).child(header)
}

/// "Weekly review · Thu Oct 29 – Wed Nov 4" with a muted line under it ("vs …"): the earlier and
/// later week buttons before it, the week menu after it, and Day | Week | Recaps at the right.
fn header(t: &Theme, period: Option<String>, sub: Option<String>, before: Option<Div>, after: Option<AnyElement>, right: Div) -> Div {
    let title = div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Weekly review"))
        .children(period.map(|p| div().text_size(px(22.)).text_color(t.text_2).child(sel(t, "review-period", format!("· {p}")))))
        .children(after);
    let block = div().flex().flex_col().gap(px(2.)).min_w_0().child(title).children(sub.map(|a| div().text_size(px(12.)).text_color(t.muted).child(sel(t, "review-sub", a))));
    div().flex().flex_wrap().items_center().gap(px(12.)).children(before).child(block).child(div().flex_1()).child(right)
}

/// Before the first review: when it comes, and this week so far with the review's own cards.
fn first_week(m: &MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Div {
    let list = m.data.reviews.as_ref();
    let next = list.and_then(|l| first_review(s(l, "next"), chrono::Utc::now()));
    let week = list.map(|l| &l["this_week"]).filter(|w| w["data"].is_object());
    let v = week.map(|w| view(w, &[]));
    let tracked = v.as_ref().is_some_and(|v| !v.categories.is_empty());
    let sub = v.as_ref().filter(|_| tracked).and_then(|v| v.kpis.iter().find(|k| k.label == "Agent time")).map(|k| format!("{} of agent time so far", k.value));
    let title = v.as_ref().filter(|_| tracked).map(|v| v.period.clone());

    // ---- when the first review comes: the week's days up to it, today and the review day marked
    let strip = |at: chrono::NaiveDate| {
        let start = week.and_then(|w| fmt::parse(s(w, "period_start"))).map(|t| fmt::local(t).date_naive());
        let today = fmt::local(chrono::Utc::now()).date_naive();
        let first = start.unwrap_or(today).min(at);
        div().flex().gap(px(6.)).children((0..7).map(|k| first + chrono::Duration::days(k)).map(|d| {
            let (fg, bg, border) = if d == at {
                (t.accent_fg, t.accent_soft, t.accent)
            } else if d == today {
                (t.text, t.panel_2, t.border_2)
            } else if d < today {
                (t.muted, t.seg, t.seg)
            } else {
                (t.faint, t.card, t.border)
            };
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(1.))
                .w(px(40.))
                .h(px(44.))
                .rounded(px(8.))
                .border_1()
                .border_color(border)
                .bg(bg)
                .text_color(fg)
                .child(div().text_size(px(10.5)).font_weight(FontWeight::MEDIUM).child(d.format("%a").to_string()))
                .child(div().text_size(px(14.)).font_weight(FontWeight::BOLD).child(d.format("%-d").to_string()))
        }))
    };
    let next_at = list.and_then(|l| fmt::parse(s(l, "next"))).map(|t| fmt::local(t).date_naive());
    let left = match &next {
        Some((when, away)) => div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(t.accent_fg).child(format!("First review {away}")))
            .child(div().text_size(px(18.)).font_weight(FontWeight::BOLD).child(sel(t, "review-first-at", when.clone()))),
        None => div().flex().flex_col().gap(px(4.)).child(div().text_size(px(18.)).font_weight(FontWeight::BOLD).child("Weekly reviews are off")),
    };
    // With nothing tracked yet, the card says so itself rather than a second card holding one line.
    let left = left.when(!tracked, |d| {
        d.child(div().text_size(px(13.)).text_color(t.muted).child("No agent time this week yet"))
            .children(m.data.errs.review.clone().map(|e| div().text_size(px(13.)).text_color(t.muted).child(e)))
    });
    let callout = card(t).flex_row().flex_wrap().px(px(20.)).items_center().justify_between().gap(px(16.)).child(left).children(next_at.map(strip));
    let content = v.filter(|_| tracked).map(|v| body(m, t, &v, cx));
    page(header(t, title, sub, None, None, day_week(t, 1, cx))).child(callout).children(content)
}

pub fn render(m: &mut MainWindow, _window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let shell = div().id("review-page").flex().flex_col().flex_1().min_w_0().size_full().overflow_y_scroll();
    let list: Vec<Value> = m.data.reviews.as_ref().map(|l| arr(l, "reviews").to_vec()).unwrap_or_default();
    if m.data.reviews.is_some() && list.is_empty() {
        return shell.child(first_week(m, &t, cx)).into_any_element();
    }
    let want = current(m);
    let shown = m.data.review.clone().filter(|r| r["id"].as_i64().is_some() && r["id"].as_i64() == want);
    let Some(r) = shown else {
        let state = match m.data.errs.review.clone() {
            Some(e) => kit::empty(&t, e).child(kit::link(&t, "review-retry", "Try again").on_click(cx.listener(|m, _, _, cx| m.refresh(cx)))),
            None => kit::empty(&t, "Loading…"),
        };
        let head = header(&t, None, None, None, None, day_week(&t, 1, cx));
        return shell.child(page(head).child(state)).into_any_element();
    };
    mark_seen(m, &r, cx);
    let v = view(&r, &list);

    // ---- header: the week, with earlier and later weeks around a picker for any of them
    let at = list.iter().position(|x| x["id"].as_i64() == Some(v.id));
    let older = at.and_then(|i| list.get(i + 1)).and_then(|x| x["id"].as_i64());
    let newer = at.and_then(|i| i.checked_sub(1)).and_then(|i| list.get(i)).and_then(|x| x["id"].as_i64());
    let nav = div()
        .flex()
        .items_center()
        .gap(px(4.))
        .child(kit::btn(&t, "review-older", "‹").w(px(30.)).when(older.is_none(), kit::disabled).tooltip(kit::tip("Earlier week")).on_click(cx.listener(move |m, _, _, cx| {
            if let Some(id) = older {
                pick(m, id, cx)
            }
        })))
        .child(kit::btn(&t, "review-newer", "›").w(px(30.)).when(newer.is_none(), kit::disabled).tooltip(kit::tip("Later week")).on_click(cx.listener(move |m, _, _, cx| {
            if let Some(id) = newer {
                pick(m, id, cx)
            }
        })));
    let weeks_btn = kit::btn(&t, "review-week-btn", "⌄").w(px(30.)).child(kit::anchor(MENU_WEEK)).tooltip(kit::tip("Any week")).on_click(cx.listener(|m, e: &ClickEvent, _, cx| {
        let p = e.position();
        m.toggle_menu(MENU_WEEK, point(p.x - px(20.), p.y + px(14.)), cx);
    }));

    let mut menus: Vec<AnyElement> = vec![];
    if let Some(at) = m.menu_open(MENU_WEEK) {
        let weeks: Vec<(i64, String)> = list.iter().filter_map(|x| Some((x["id"].as_i64()?, format!("{} · {}", period(x), took(secs(&x["seconds"])))))).collect();
        let (n, shown, tt) = (weeks.len(), v.id, t.clone());
        // Only the weeks in view are drawn; past a dozen the menu scrolls.
        let items = uniform_list(
            "review-week-list",
            n,
            cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|k| {
                        let (id, label) = (weeks[k].0, weeks[k].1.clone());
                        kit::menu_item(&tt, SharedString::from(format!("review-week-{id}")), label, id == shown).on_click(cx.listener(move |m, _, _, cx| pick(m, id, cx)))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .h(px(n.min(WEEKS_SHOWN) as f32 * crate::theme::height::CONTROL));
        let menu = kit::menu_box(&t, 220.).id("review-week-menu").child(items);
        menus.push(kit::menu_under(MENU_WEEK, kit::Align::Left, at, menu.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())));
    }

    let head = header(&t, Some(v.period.clone()), v.against.clone(), Some(nav), Some(weeks_btn.into_any_element()), day_week(&t, 1, cx));
    shell.child(page(head).child(body(m, &t, &v, cx))).children(menus).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review() -> Value {
        json!({
            "id": 4, "period_start": "2026-10-05T00:00:00Z", "period_end": "2026-10-12T00:00:00Z", "seen_at": null,
            "fixes": {"state": "done", "filed": ["B12", "B13"], "linked": ["B9"]},
            "data": {
                "seconds": 36000.0,
                "change": {"review": 3, "seconds": -1800.0, "owner_wait": 600.0, "timeouts": -2, "reruns": 1},
                "categories": [{"name": "build", "seconds": 18000.0}, {"name": "test", "seconds": 9000.0}],
                "projects": [{"name": "app", "seconds": 30000.0, "categories": [{"name": "build", "seconds": 20000.0}]}],
                "slowest_tasks": [{"task": "T12", "title": "Make it fast", "seconds": 7200.0, "top_category": "test"}],
                "repeated": [{"command": "cargo test", "runs": 9, "median": 300.0, "p90": 540.0}],
                "timeouts": {"count": 2, "commands": [{"command": "cargo build", "count": 2, "seconds": 1200.0}]},
                "reruns": [{"command": "cargo test -p x", "task": "T12", "runs": 3, "seconds": 900.0, "passed": true}],
                "owner_wait": {"seconds": 5400.0, "reasons": [{"name": "question", "seconds": 3600.0}, {"name": "review", "seconds": 1800.0}]},
                "suggestions": [{"title": "Cap builds", "detail": "Too many at once"}],
                "shipped": {
                    "tasks": [
                        {"ref": "T63", "title": "Drop the unused icon font", "project": "app", "goal": null, "pr_num": 416, "pr_url": "https://example.com/pr/416"},
                        {"ref": "T62", "title": "Inline the critical CSS", "project": "app", "goal": "G3", "pr_num": null, "pr_url": null}
                    ],
                    "goals": [{"ref": "G3", "name": "Faster pages", "project": "app"}],
                    "releases": [{"ref": "R7", "project": "app", "name": "v0.13.0", "notes": "\nPasskeys and faster first paint\n- more", "cut_at": "2026-10-09T00:00:00Z"}]
                }
            },
            "fix_issues": [
                {"id": 12, "ref": "B12", "title": "Cap concurrent builds", "kind_label": "Gap", "state": "open", "goal": null, "linked": false},
                {"id": 13, "ref": "B13", "title": "Warm caches", "kind_label": "Follow-up", "state": "open", "goal": {"id": 40, "ref": "G40", "name": "Faster builds"}, "linked": false},
                {"id": 9, "ref": "B9", "title": "Long test timeouts", "kind_label": "Bug", "state": "done", "goal": null, "linked": true}
            ]
        })
    }

    #[::core::prelude::v1::test]
    fn a_review_reads_as_tb_review_does() {
        let v = view(&review(), &[]);
        // The week ends on its last day, not on the next week's first.
        assert_eq!(v.period, "Mon Oct 5 – Sun Oct 11");
        assert_eq!(v.kpis.iter().map(|k| k.value.as_str()).collect::<Vec<_>>(), ["2 tasks", "10.0 h", "1.5 h", "2", "1"]);
        assert_eq!(v.kpis[0].sub.as_deref(), Some("1 goal · 1 release"));
        assert_eq!(v.categories[0], Category { name: "build".into(), took: "5.0 h".into(), share: 0.5, delta: None });
        assert_eq!(v.projects[0].sub, "mostly build");
        assert_eq!(v.slowest[0], Line { task: Some("T12".into()), name: "Make it fast".into(), took: "2.0 h".into(), sub: "mostly test".into(), share: 1.0 });
        assert_eq!(v.repeated[0].took, "5 min");
        assert_eq!(v.repeated[0].sub, "median · p90 9 min · 9 runs");
        assert_eq!(v.timeouts[0].sub, "2 timeouts");
        assert_eq!(v.reruns[0].sub, "T12 · 3 runs, then passed");
        let reasons: Vec<(&str, &str, f32)> = v.wait_reasons.iter().map(|l| (l.name.as_str(), l.took.as_str(), l.share)).collect();
        assert_eq!(reasons, [("Question", "1.0 h", 1.0), ("Review", "30 min", 0.5)]);
    }

    #[::core::prelude::v1::test]
    fn the_key_numbers_compare_with_the_review_before() {
        let mut r = review();
        r["data"]["change"]["categories"] = json!([{"name": "build", "seconds": 900.0}, {"name": "test", "seconds": 20.0}]);
        let list = [json!({"id": 3, "period_start": "2026-09-28T00:00:00Z", "period_end": "2026-10-05T00:00:00Z"})];
        let v = view(&r, &list);
        assert_eq!(v.against.as_deref(), Some("vs Mon Sep 28 – Sun Oct 4"));
        let deltas: Vec<Option<Delta>> = v.kpis.iter().map(|k| k.delta.clone()).collect();
        let d = |text: &str, down, good| Some(Delta { text: text.into(), down, good });
        assert_eq!(
            deltas,
            [
                None,
                // Less agent time isn't better or worse on its own: it's drawn muted.
                d("30 min less", Some(true), None),
                d("10 min more", Some(false), Some(false)),
                d("2 fewer", Some(true), Some(true)),
                d("1 more", Some(false), Some(false)),
            ]
        );
        // A key number that didn't move says so.
        let mut same = r.clone();
        same["data"]["change"]["reruns"] = json!(0);
        assert_eq!(view(&same, &list).kpis[4].delta, d("no change", None, None));
        // A category that barely moved shows no change.
        assert_eq!(v.categories[0].delta, d("15 min more", Some(false), Some(false)));
        assert_eq!(v.categories[1].delta, None);
        // The first review has nothing to compare with.
        r["data"]["change"] = Value::Null;
        let v = view(&r, &list);
        assert!(v.against.is_none() && v.kpis.iter().all(|k| k.delta.is_none()));
    }

    #[::core::prelude::v1::test]
    fn the_first_review_says_when_and_how_far_off() {
        let next = fmt::parse("2026-10-23T17:00:00Z").unwrap();
        let (when, away) = first_review("2026-10-23T17:00:00Z", next - chrono::Duration::days(3)).unwrap();
        assert_eq!(when, fmt::local(next).format("%A, %b %-d · %-I:%M %p").to_string());
        assert_eq!(away, "in 3 days");
        assert_eq!(first_review("2026-10-23T17:00:00Z", next - chrono::Duration::days(1)).unwrap().1, "tomorrow");
        assert_eq!(first_review("2026-10-23T17:00:00Z", next).unwrap().1, "today");
        assert_eq!(first_review("", next), None, "the schedule is off");
    }

    #[::core::prelude::v1::test]
    fn plan_a_goal_checks_only_the_fixes_still_on_the_backlog() {
        let v = view(&review(), &[]);
        let went: Vec<&str> = v.fixes.iter().map(|f| f.went.as_str()).collect();
        assert_eq!(went, ["", "In G40", "Done"]);
        assert_eq!(v.plan(), ["B12"]);
        assert_eq!(v.fixes_tally(), "1 open · 1 in a goal · 1 closed");
        // Filed fixes replace the plain suggestions.
        assert!(v.suggestions.is_empty() && v.fixes_note.is_none());
    }

    #[::core::prelude::v1::test]
    fn with_nothing_filed_the_suggestions_show_and_say_why() {
        let mut r = review();
        r["fix_issues"] = json!([]);
        r["fixes"] = json!({"state": "failed", "error": "Claude timed out"});
        let v = view(&r, &[]);
        assert_eq!(v.fixes_note, None);
        assert_eq!(v.fixes_failed.as_deref(), Some("Couldn’t file these as backlog issues: Claude timed out. To file one by hand, run tb backlog add."));
        assert_eq!(v.suggestions, [("Cap builds".to_string(), "Too many at once".to_string())]);
        assert!(v.plan().is_empty());
        r["fixes"] = json!({"state": "none"});
        assert_eq!(view(&r, &[]).fixes_failed, None);
        r["fixes"] = json!({"state": "running"});
        assert_eq!(view(&r, &[]).fixes_note.as_deref(), Some("Filing these as backlog issues…"));
    }

    #[::core::prelude::v1::test]
    fn the_week_lists_what_it_shipped() {
        let v = view(&review(), &[]);
        let sh = &v.shipped;
        assert_eq!(sh.releases, [ShippedRelease { r: "R7".into(), name: "v0.13.0".into(), headline: "Passkeys and faster first paint".into(), project: "app".into() }]);
        assert_eq!(sh.goals, [("G3".to_string(), "Faster pages".to_string())]);
        assert_eq!(sh.tasks[0].pr, Some(("#416".to_string(), "https://example.com/pr/416".to_string())));
        assert_eq!(sh.tasks[1].pr, None);
        // A review kept before it listed what shipped reads as nothing shipped.
        let mut old = review();
        old["data"].as_object_mut().unwrap().remove("shipped");
        let v = view(&old, &[]);
        assert!(v.shipped.is_empty());
        assert_eq!((v.kpis[0].value.as_str(), v.kpis[0].sub.as_deref()), ("0 tasks", Some("0 goals · 0 releases")));
    }
}
