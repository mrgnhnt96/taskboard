//! The Backlog page: triage the backlog by making goals from it. An open issue that isn't in a goal
//! yet is untriaged; it's triaged once it's in a goal, deferred or dropped.
//!
//! The untriaged issues (`GET /backlog/triage`) are grouped by a lens — Similar (Claude's groups),
//! Area, Impact or Priority — and filtered by kind. Checking issues adds them to the new goal on
//! the right and selects them for the bar's actions (move to a goal, set priority, defer, drop),
//! each with a key. Claude plans the goal's waves when asked (`POST /backlog/plan`); checking more
//! afterwards asks for a re-plan, removing one doesn't. Create makes the goal
//! (`POST /backlog/goal`): one planned task per issue, in its wave. Starting it is on its goal page.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, s};
use crate::theme::Theme;
<<<<<<< Updated upstream
use crate::ui::issue_panel as ip;
=======
use crate::ui::issue_panel::{self as ip, Act, AsideView, Ctx};
#[cfg(test)]
use crate::ui::issue_panel::flat;
>>>>>>> Stashed changes
use crate::ui::kit;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const SYNC_EVERY: Duration = Duration::from_secs(3);
/// While Claude plans, ask for the plan this often.
const PLAN_EVERY: Duration = Duration::from_millis(1500);
const KINDS: [&str; 4] = ["bug", "gap", "follow", "clean"];
const MENU_MOVE: &str = "plan-move";
const MENU_PRIO: &str = "plan-prio";
const MENU_PROJECT: &str = "plan-project";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Lens {
    #[default]
    Similar,
    Area,
    Impact,
    Priority,
}

impl Lens {
    pub const ALL: [Lens; 4] = [Lens::Similar, Lens::Area, Lens::Impact, Lens::Priority];

    pub fn label(self) -> &'static str {
        match self {
            Lens::Similar => "Similar",
            Lens::Area => "Area",
            Lens::Impact => "Impact",
            Lens::Priority => "Priority",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Lens::Similar => "Claude groups issues that touch the same code or the same problem.",
            Lens::Area => "Grouped by the part of the app each issue touches.",
            Lens::Impact => "How much fixing it helps: fewer wasted agent turns, problems you see sooner.",
            Lens::Priority => "P1 first. Claude suggests one when an issue is reported.",
        }
    }
}

pub struct State {
    pub lens: Lens,
    /// Kinds shown (bug, gap, follow, clean).
    pub kinds: [bool; 4],
    pub project: String,
    /// Checked issues, in the order checked: the new goal's issues and the bar's selection.
    pub picked: Vec<String>,
    focus: Option<FocusHandle>,
    /// `GET /backlog/triage` and `GET /backlog/plan`.
    data: Option<Value>,
    err: Option<String>,
    plan: Option<Value>,
    synced: Option<Instant>,
    generation: u64,
    busy: bool,
    pub just_made: Option<(String, String)>,
}

impl Default for State {
    fn default() -> Self {
        State {
            lens: Lens::Similar,
            kinds: [true; 4],
            project: "all".into(),
            picked: vec![],
            focus: None,
            data: None,
            err: None,
            plan: None,
            synced: None,
            generation: 0,
            busy: false,
            just_made: None,
        }
    }
}

// ------------------------------------------------------------------ view model

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    None,
    Some,
    All,
}

#[derive(Clone, Debug)]
pub struct RowView {
    pub r: String,
    pub kind: String,
    pub title: String,
    pub area: String,
    pub impact: String,
    pub priority: String,
    pub is_new: bool,
    pub picked: bool,
}

#[derive(Clone, Debug)]
pub struct GroupView {
    pub label: String,
    pub about: String,
    pub rows: Vec<RowView>,
    pub check: Check,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub r: String,
    pub title: String,
    pub priority: String,
    pub after: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct WaveView {
    pub name: String,
    pub why: String,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Default)]
pub struct DraftView {
    pub count: usize,
    pub waves: Vec<WaveView>,
    pub unplanned: Vec<Item>,
    /// None of the checked issues has been planned.
    pub never_planned: bool,
    pub planning: bool,
    pub note: Option<String>,
    /// The plan's suggested name.
    pub suggest: String,
    /// More than one project among the checked issues (a goal holds one).
    pub mixed: bool,
}

impl DraftView {
    pub fn stale(&self) -> bool {
        !self.never_planned && !self.unplanned.is_empty()
    }
    pub fn can_create(&self) -> bool {
        self.count > 0 && !self.planning && !self.never_planned && !self.stale() && !self.mixed
    }
}

fn key_of(lens: Lens, b: &Value) -> String {
    match lens {
        Lens::Similar => s(b, "group"),
        Lens::Area => s(b, "area"),
        Lens::Impact => s(b, "impact"),
        Lens::Priority => s(b, "priority"),
    }
    .to_string()
}

fn impact_label(i: &str) -> &'static str {
    match i {
        "high" => "High impact",
        "med" => "Medium impact",
        _ => "Low impact",
    }
}

/// Created in the last two days.
fn is_new(b: &Value) -> bool {
    fmt::parse(s(b, "created_at")).is_some_and(|t| chrono::Utc::now().signed_duration_since(t).num_hours() < 48)
}

/// The issues shown, grouped by the lens: groups by size (Similar), name (Area), or rank.
pub fn groups(issues: &[Value], lens: Lens, kinds: [bool; 4], picked: &[String]) -> Vec<GroupView> {
    let shown: Vec<&Value> = issues.iter().filter(|b| KINDS.iter().position(|k| *k == s(b, "kind")).map(|i| kinds[i]).unwrap_or(true)).collect();
    let mut keys: Vec<String> = vec![];
    for b in &shown {
        let k = key_of(lens, b);
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    let count = |k: &str| shown.iter().filter(|b| key_of(lens, b) == k).count();
    match lens {
        Lens::Similar => keys.sort_by(|a, b| count(b).cmp(&count(a))),
        Lens::Area => keys.sort_by(|a, b| crate::ui::modals::locale_cmp(a, b)),
        Lens::Impact => keys.sort_by_key(|k| ["high", "med", "low"].iter().position(|x| x == k).unwrap_or(9)),
        Lens::Priority => keys.sort(),
    }
    keys.into_iter()
        .map(|k| {
            let rows: Vec<RowView> = shown
                .iter()
                .filter(|b| key_of(lens, b) == k)
                .map(|b| {
                    let r = fmt::ref_of(b, "B");
                    RowView {
                        picked: picked.contains(&r),
                        r,
                        kind: s(b, "kind").to_string(),
                        title: s(b, "title").to_string(),
                        area: s(b, "area").to_string(),
                        impact: s(b, "impact").to_string(),
                        priority: s(b, "priority").to_string(),
                        is_new: is_new(b),
                    }
                })
                .collect();
            let n = rows.iter().filter(|r| r.picked).count();
            let check = if n == 0 { Check::None } else if n == rows.len() { Check::All } else { Check::Some };
            let about = if lens == Lens::Similar { rows.first().and_then(|r| issues.iter().find(|b| fmt::ref_of(b, "B") == r.r)).map(|b| s(b, "group_about").to_string()).unwrap_or_default() } else { String::new() };
            let label = match lens {
                Lens::Impact => impact_label(&k).to_string(),
                Lens::Priority => k.to_uppercase(),
                _ => k,
            };
            GroupView { label, about, rows, check }
        })
        .collect()
}

/// The new goal: the plan's waves (only checked issues; `after` only to those), and the checked
/// issues the plan doesn't cover.
pub fn draft(issues: &[Value], plan: Option<&Value>, picked: &[String]) -> DraftView {
    let find = |r: &str| issues.iter().find(|b| fmt::ref_of(b, "B") == r);
    let item = |r: &str, after: Vec<String>| {
        let b = find(r);
        Item { r: r.to_string(), title: b.map(|b| s(b, "title").to_string()).unwrap_or_default(), priority: b.map(|b| s(b, "priority").to_string()).unwrap_or_default(), after }
    };
    let picks: Vec<&String> = picked.iter().filter(|r| find(r).is_some()).collect();
    let projects: Vec<&str> = picks.iter().filter_map(|r| find(r)).map(|b| s(b, "project")).collect();
    let mixed = projects.iter().any(|p| *p != projects[0]);
    let covered: Vec<String> = plan.map(|p| arr(p, "ids").iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let state = plan.map(|p| s(p, "state")).unwrap_or("none");
    let planning = state == "planning" && picks.iter().any(|r| covered.contains(r));
    let mut waves = vec![];
    if state == "ready" {
        for w in plan.map(|p| arr(p, "waves").to_vec()).unwrap_or_default() {
            let items: Vec<Item> = arr(&w, "items")
                .iter()
                .map(|i| s(i, "ref").to_string())
                .zip(arr(&w, "items").iter().map(|i| arr(i, "after").iter().filter_map(|a| a.as_str().map(str::to_string)).collect::<Vec<_>>()))
                .filter(|(r, _)| picks.contains(&r))
                .map(|(r, after)| item(&r, after.into_iter().filter(|a| picks.contains(&a)).collect()))
                .collect();
            if !items.is_empty() {
                waves.push(WaveView { name: format!("Wave {}", waves.len() + 1), why: s(&w, "why").to_string(), items });
            }
        }
<<<<<<< Updated upstream
    }
    let planned: Vec<String> = waves.iter().flat_map(|w| w.items.iter().map(|i| i.r.clone())).collect();
    let unplanned: Vec<Item> = if planning { vec![] } else { picks.iter().filter(|r| !planned.contains(r)).map(|r| item(r, vec![])).collect() };
    DraftView {
        count: picks.len(),
        never_planned: planned.is_empty(),
        waves,
        unplanned,
        planning,
        note: plan.and_then(|p| fmt::opt_s(p, "note")).filter(|_| state == "ready").map(str::to_string),
        suggest: plan.map(|p| s(p, "name").to_string()).unwrap_or_default(),
        mixed,
=======
    };
    let aside = match &p.aside {
        None => AsideState::None,
        Some((_, None)) => AsideState::Loading(None),
        Some((_, Some(Err(e)))) => AsideState::Loading(Some(e.clone())),
        Some((_, Some(Ok(b)))) => AsideState::Issue(Box::new(ip::aside_view(b))),
    };
    let pspec = [("all".to_string(), "All projects".to_string())];
    let gspec = [("all".to_string(), "All goals".to_string()), ("none".to_string(), "Not in a goal".to_string())];
    PageView {
        open,
        project_label: ip::picker_label(false, st.f_project(), &pspec, goals),
        goal_label: ip::picker_label(true, st.f_goal(), &gspec, goals),
        kind_on: KINDS.iter().position(|(k, _)| *k == st.f_kind()).unwrap_or(0),
        state_options: STATES.iter().map(|(k, _)| *k).filter(|k| *k != "ticket" || p.ctx.jira || st.f_state() == "ticket").collect(),
        list,
        aside,
>>>>>>> Stashed changes
    }
}

/// `POST /backlog/goal`'s waves, from what the draft shows.
pub fn goal_waves(d: &DraftView) -> Value {
    json!(d.waves.iter().map(|w| json!({"why": w.why, "items": w.items.iter().map(|i| json!({"ref": i.r, "after": i.after})).collect::<Vec<_>>()})).collect::<Vec<_>>())
}

// ------------------------------------------------------------------ fetching and actions

/// Fetch the list again on the next render (after a change made elsewhere, e.g. the issue panel).
pub fn stale(m: &mut MainWindow) {
    m.backlog.synced = None;
}

pub fn enter(m: &mut MainWindow) {
    m.backlog.synced = None;
    m.backlog.just_made = None;
}

fn issues(m: &MainWindow) -> Vec<Value> {
    m.backlog.data.as_ref().map(|d| arr(d, "issues").to_vec()).unwrap_or_default()
}

/// The checked issues still on the page.
fn selected(m: &MainWindow) -> Vec<String> {
    let have = issues(m);
    m.backlog.picked.iter().filter(|r| have.iter().any(|b| fmt::ref_of(b, "B") == **r)).cloned().collect()
}

fn sync(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let planning = m.backlog.plan.as_ref().is_some_and(|p| s(p, "state") == "planning");
    let every = if planning { PLAN_EVERY } else { SYNC_EVERY };
    if m.backlog.synced.is_some_and(|t| t.elapsed() < every) {
        return;
    }
    m.backlog.synced = Some(Instant::now());
    m.backlog.generation += 1;
    let generation = m.backlog.generation;
    let project = m.backlog.project.clone();
    let backend = m.backend.clone();
    cx.spawn(async move |this, cx| {
        let got = cx
            .background_executor()
            .spawn(async move { (backend.get("backlog/triage", &[("project", project)]), backend.get("backlog/plan", &[])) })
            .await;
        let _ = this.update(cx, |m, cx| {
            if m.backlog.generation != generation {
                return;
            }
            let (list, plan) = got;
            match list {
                Ok(v) => {
                    m.backlog.data = Some(v);
                    m.backlog.err = None;
                }
                Err(e) => m.backlog.err = Some(e.message),
            }
            if let Ok(p) = plan {
                let was_planning = m.backlog.plan.as_ref().is_some_and(|p| s(p, "state") == "planning");
                m.backlog.plan = Some(p);
                if was_planning {
                    // Poll again soon until the plan lands.
                    m.backlog.synced = None;
                }
            }
            cx.notify();
        });
    })
    .detach();
}

fn resync(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    m.backlog.synced = None;
    cx.notify();
}

fn toggle(m: &mut MainWindow, refs: &[String], on: bool, cx: &mut Context<MainWindow>) {
    for r in refs {
        let has = m.backlog.picked.contains(r);
        if on && !has {
            m.backlog.picked.push(r.clone());
        } else if !on && has {
            m.backlog.picked.retain(|x| x != r);
        }
    }
    m.backlog.just_made = None;
    cx.notify();
}

fn clear(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    m.backlog.picked.clear();
    if m.menu.as_ref().is_some_and(|(k, _)| k.starts_with("plan-")) {
        m.menu = None;
    }
    cx.notify();
}

/// Esc on the Backlog page: close its menu and uncheck everything. False when there was nothing to
/// do, or a task or issue panel (or a dialog) is open: Esc closes that first.
pub fn escape(m: &mut MainWindow, cx: &mut Context<MainWindow>) -> bool {
    if m.page != Page::Backlog || m.backlog.picked.is_empty() || m.panel.is_some() || m.modal.is_some() {
        return false;
    }
    clear(m, cx);
    true
}

/// The selection bar's actions: `POST /backlog/bulk` for the checked issues.
fn bulk(m: &mut MainWindow, action: &str, extra: Value, cx: &mut Context<MainWindow>) {
    let ids = selected(m);
    if ids.is_empty() || m.backlog.busy {
        return;
    }
    let mut body = json!({"ids": ids, "action": action});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        body[k] = v;
    }
    let keep = action == "priority";
    let (n, what) = (ids.len() as i64, action.to_string());
    m.menu = None;
    m.backlog.busy = true;
    m.post_or(
        "backlog/bulk",
        body,
        cx,
        move |m, v, cx| {
            m.backlog.busy = false;
            if !keep {
                m.backlog.picked.retain(|r| !ids.contains(r));
            }
            let issues = fmt::plural(n, "issue", "issues");
            let text = match what.as_str() {
                "defer" => format!("Deferred {issues}"),
                "drop" => format!("Dropped {issues}"),
                "goal" => format!("Moved {issues} into the goal as planned tasks"),
                _ => format!("Priority set on {issues}"),
            };
            let _ = v;
            m.toast(text, false, cx);
            resync(m, cx);
        },
        |m, e, cx| {
            m.backlog.busy = false;
            m.toast(e.message, true, cx);
        },
    );
}

fn plan_waves(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let ids = selected(m);
    if ids.is_empty() {
        return;
    }
    m.post("backlog/plan", json!({"ids": ids}), cx, |m, v, cx| {
        m.backlog.plan = Some(v);
        resync(m, cx);
    });
}

fn create(m: &mut MainWindow, d: &DraftView, cx: &mut Context<MainWindow>) {
    if !d.can_create() || m.backlog.busy {
        return;
    }
    // Goals from the backlog are named by the plan (Claude's name, else the main group's).
    let name = if d.suggest.trim().is_empty() { "Backlog goal".to_string() } else { d.suggest.trim().to_string() };
    m.backlog.busy = true;
    m.post_or(
        "backlog/goal",
        json!({"name": name, "waves": goal_waves(d)}),
        cx,
        move |m, v, cx| {
            m.backlog.busy = false;
            let g = if v["goal"].is_object() { v["goal"].clone() } else { v };
            let r = fmt::ref_of(&g, "G");
            m.backlog.picked.clear();
            m.backlog.plan = None;
            m.backlog.just_made = Some((r.clone(), s(&g, "name").to_string()));
            m.toast(format!("Made {r}. Start it when you're ready."), false, cx);
            resync(m, cx);
            // Straight to the new goal, where it's started.
            if !r.is_empty() {
                m.go(Page::Goal(r), cx);
            }
        },
        |m, e, cx| {
            m.backlog.busy = false;
            m.toast(e.message, true, cx);
        },
    );
}

/// Keys while issues are checked: M move, P priority (1 2 3 set it), D defer, X drop.
fn on_key(m: &mut MainWindow, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<MainWindow>) {
    let _ = window;
    if m.modal.is_some() || m.panel.is_some() {
        return;
    }
    let ks = &ev.keystroke;
    if ks.modifiers.platform || ks.modifiers.control || ks.modifiers.alt || selected(m).is_empty() {
        return;
    }
    let at = point(px(0.), px(0.));
    match ks.key.as_str() {
        "m" => m.toggle_menu(MENU_MOVE, at, cx),
        "p" => m.toggle_menu(MENU_PRIO, at, cx),
        "1" | "2" | "3" => bulk(m, "priority", json!({"priority": format!("p{}", ks.key)}), cx),
        "d" => bulk(m, "defer", json!({}), cx),
        "x" => bulk(m, "drop", json!({}), cx),
        _ => return,
    }
    cx.stop_propagation();
}

// ------------------------------------------------------------------ render

/// Text on the goal color: light purple in the dark theme takes dark text.
fn on_goal(t: &Theme) -> Hsla {
    if t.mode == crate::theme::ThemeMode::Dark { t.bg } else { t.on_accent }
}

fn check_box(t: &Theme, id: SharedString, c: Check) -> Stateful<Div> {
    let on = c != Check::None;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(18.))
        .rounded(px(5.))
        .border_1()
        .border_color(if on { t.goal } else { t.border_2 })
        .when(on, |d| d.bg(t.goal))
        .text_color(on_goal(t))
        .text_size(px(12.))
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        .child(match c {
            Check::All => "✓",
            Check::Some => "–",
            Check::None => "",
        })
}

fn kbd(t: &Theme, k: &str) -> Div {
    div().px(px(5.)).rounded(px(4.)).bg(t.seg).text_color(t.muted).font_family(t.mono_font.clone()).text_size(px(10.5)).child(k.to_string())
}

fn prio_pill(t: &Theme, p: &str) -> Div {
    let (fg, bg) = match p {
        "p1" => (t.warn_fg, t.warn_soft),
        "p2" => (t.accent_fg, t.accent_soft),
        _ => (t.muted, t.seg),
    };
    kit::pill(fg, bg, p.to_uppercase()).h(px(18.)).px(px(7.))
}

/// Impact as signal bars: three for high, two for medium, one for low.
fn impact_bars(t: &Theme, id: SharedString, impact: &str) -> Stateful<Div> {
    let (n, color) = match impact {
        "high" => (3, t.warn),
        "med" => (2, t.accent),
        _ => (1, t.muted),
    };
    let mut d = div().id(id).flex().flex_none().items_end().gap(px(2.)).h(px(12.)).tooltip(kit::tip(impact_label(impact)));
    for (i, h) in [5., 8., 12.].into_iter().enumerate() {
        d = d.child(div().w(px(3.)).h(px(h)).rounded(px(1.)).bg(if i < n { color } else { t.border_2 }));
    }
    d
}

fn bar_btn(t: &Theme, id: &'static str, label: &str, key: &str) -> Stateful<Div> {
    let h = t.panel_2;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(28.))
        .px(px(8.))
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(13.))
        .hover(move |s| s.bg(h))
        .child(label.to_string())
        .child(kbd(t, key))
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    sync(m, cx);
    if m.backlog.focus.is_none() {
        m.backlog.focus = Some(cx.focus_handle());
    }
    let list = issues(m);
    let groups = groups(&list, m.backlog.lens, m.backlog.kinds, &m.backlog.picked);
    let d = draft(&list, m.backlog.plan.as_ref(), &m.backlog.picked);
    let data = m.backlog.data.clone().unwrap_or(Value::Null);
    let (untriaged, triaged, total) = (fmt::i(&data, "untriaged"), fmt::i(&data, "triaged"), fmt::i(&data, "total"));
    let sel = selected(m);

    // ---- header: how much is triaged, the project, the lens
    let pct = if total > 0 { triaged as f32 / total as f32 } else { 1. };
    let meter = div()
        .flex()
<<<<<<< Updated upstream
        .flex_col()
        .gap(px(5.))
        .child(
            div().flex().items_baseline().gap(px(8.)).child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Plan from the backlog")).child(
                div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(if untriaged > 0 { t.warn_fg } else { t.up_fg }).child(if m.backlog.data.is_none() {
                    m.backlog.err.clone().unwrap_or_else(|| "Loading…".into())
                } else if untriaged > 0 {
                    format!("{} not in a goal yet", fmt::plural(untriaged, "issue", "issues"))
                } else {
                    "Backlog triaged".into()
                }),
            ),
        )
        .child(
            div()
                .id("plan-meter")
                .w(px(260.))
                .h(px(6.))
                .rounded_full()
                .bg(t.seg)
                .tooltip(kit::tip(format!("{triaged} of {total} issues triaged: in a goal, deferred or dropped")))
                .child(div().h_full().rounded_full().w(relative(pct)).bg(if untriaged > 0 { t.warn } else { t.up })),
        );
    let projects: Vec<Value> = arr(&data, "projects").to_vec();
    let project_btn = (projects.len() > 1 || m.backlog.project != "all").then(|| {
        let label = if m.backlog.project == "all" { "All projects".to_string() } else { m.backlog.project.clone() };
        kit::btn_small(&t, "plan-project-btn", format!("{label}  ▾")).on_click(cx.listener(|m, e: &ClickEvent, _, cx| {
            let p = e.position();
            m.toggle_menu(MENU_PROJECT, point(p.x - px(10.), p.y + px(14.)), cx);
=======
        .flex_none()
        .items_center()
        .gap(px(12.))
        .child(kit::link(&t, "bl-back", "‹ Task board").text_size(px(13.5)).on_click(cx.listener(|m, _, _, cx| m.go(Page::Board, cx))))
        .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Backlog"))
        .children(v.open.map(|n| kit::tone_pill(&t, "warn", format!("{n} open"))))
        .child(div().flex_1())
        .child(kit::link(&t, "bl-goals", "Goals").text_size(px(13.5)).on_click(cx.listener(|m, _, _, cx| {
            if let Some(g) = goals_landing(m) {
                m.go(Page::Goal(g), cx);
            }
        })));

    // ---- filters
    let st = &m.backlog;
    let (fp, fg) = (st.f_project().to_string(), st.f_goal().to_string());
    let labels: Vec<&str> = KINDS.iter().map(|(_, l)| *l).collect();
    let kinds = kit::seg(&t, "bl-kind", &labels, v.kind_on, |i, item| {
        item.on_click(cx.listener(move |m, _, _, cx| {
            m.backlog.kind = KINDS[i].0.to_string();
            filters_changed(m, cx);
>>>>>>> Stashed changes
        }))
    });
    let lens_on = Lens::ALL.iter().position(|l| *l == m.backlog.lens).unwrap_or(0);
    let labels: Vec<&str> = Lens::ALL.iter().map(|l| l.label()).collect();
    let lenses = kit::seg(&t, "plan-lens", &labels, lens_on, |i, item| {
        item.on_click(cx.listener(move |m, _, _, cx| {
            m.backlog.lens = Lens::ALL[i];
            cx.notify();
        }))
    });
    let add = kit::btn(&t, "backlog-add", "Add an issue").on_click(cx.listener(|m, _, window, cx| crate::ui::modals::open_issue_form(m, None, window, cx)));
    let header = div().flex().flex_wrap().items_center().gap(px(16.)).child(meter).child(div().flex_1()).children(project_btn).child(lenses).child(add);

    // ---- kinds and the selection bar
    let mut kinds = div().flex().flex_wrap().gap(px(6.));
    for (i, k) in KINDS.iter().enumerate() {
        let on = m.backlog.kinds[i];
        let (fg, bg) = ip::kind_colors(&t, k);
        kinds = kinds.child(
            div()
                .id(SharedString::from(format!("plan-kind-{k}")))
                .flex()
                .items_center()
                .h(px(26.))
                .px(px(10.))
                .rounded_full()
                .cursor_pointer()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .when(on, |d| d.bg(bg).text_color(fg))
                .when(!on, |d| d.border_1().border_color(t.border).text_color(t.faint))
                .child(ip::kind_label(k))
                .on_click(cx.listener(move |m, _, _, cx| {
                    m.backlog.kinds[i] = !m.backlog.kinds[i];
                    cx.notify();
                })),
        );
    }
    let bar: AnyElement = if sel.is_empty() {
        div().into_any_element()
    } else {
        let move_open = m.menu_open(MENU_MOVE).is_some();
        let prio_open = m.menu_open(MENU_PRIO).is_some();
        let sep = || div().w(px(1.)).h(px(18.)).bg(t.goal_line);
        let mut move_wrap = div().relative().child(bar_btn(&t, "plan-move-btn", "Move to goal", "M").on_click(cx.listener(|m, _, _, cx| m.toggle_menu(MENU_MOVE, point(px(0.), px(0.)), cx))));
        if move_open {
            move_wrap = move_wrap.child(deferred(div().absolute().top(px(32.)).left_0().child(move_menu(m, &t, &sel, cx))).with_priority(3));
        }
        let mut prio_wrap = div().relative().child(bar_btn(&t, "plan-prio-btn", "Set priority", "P").on_click(cx.listener(|m, _, _, cx| m.toggle_menu(MENU_PRIO, point(px(0.), px(0.)), cx))));
        if prio_open {
            prio_wrap = prio_wrap.child(deferred(div().absolute().top(px(32.)).left_0().child(prio_menu(m, &t, &list, &sel, cx))).with_priority(3));
        }
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .pl(px(12.))
            .pr(px(4.))
            .py(px(3.))
            .rounded(px(10.))
            .border_1()
            .border_color(t.goal_line)
            .bg(t.goal_tint)
            .child(div().mr(px(6.)).text_size(px(13.)).font_weight(FontWeight::BOLD).text_color(t.goal).child(format!("{} selected", sel.len())))
            .child(move_wrap)
            .child(sep())
            .child(prio_wrap)
            .child(sep())
            .child(bar_btn(&t, "plan-defer", "Defer", "D").on_click(cx.listener(|m, _, _, cx| bulk(m, "defer", json!({}), cx))))
            .child(bar_btn(&t, "plan-drop", "Drop", "X").text_color(t.down).on_click(cx.listener(|m, _, _, cx| bulk(m, "drop", json!({}), cx))))
            .child(bar_btn(&t, "plan-clear", "Clear", "Esc").text_color(t.muted).on_click(cx.listener(|m, _, _, cx| clear(m, cx))))
            .into_any_element()
    };
    let toolbar = div().flex().flex_wrap().items_center().justify_between().gap(px(10.)).min_h(px(36.)).child(kinds).child(bar);

    // ---- the grouped list
    let mut col = div().flex().flex_col().gap(px(14.)).flex_1().min_w_0();
    col = col.child(div().text_size(px(12.5)).text_color(t.muted).child(m.backlog.lens.help()));
    if m.backlog.data.is_some() && groups.is_empty() {
        col = col.child(
            div()
                .p(px(24.))
                .rounded(px(12.))
                .border_1()
                .border_color(t.border)
                .flex()
                .justify_center()
                .text_size(px(13.))
                .text_color(if untriaged == 0 { t.up_fg } else { t.muted })
                .child(if untriaged == 0 { "Backlog is triaged. Every open issue is in a goal, deferred or dropped." } else { "No issues of the kinds shown." }),
        );
    }
    if fmt::b(&data, "grouping") {
        col = col.child(div().text_size(px(12.5)).text_color(t.goal).child("Claude is grouping new issues…"));
    }
    for (gi, g) in groups.iter().enumerate() {
        let refs: Vec<String> = g.rows.iter().map(|r| r.r.clone()).collect();
        let all = g.check == Check::All;
        let head = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .child(check_box(&t, SharedString::from(format!("plan-gcheck-{gi}")), g.check).tooltip(kit::tip(if all { "Deselect all" } else { "Select all" })).on_click(cx.listener(move |m, _, _, cx| toggle(m, &refs, !all, cx))))
            .child(div().text_size(px(14.)).font_weight(FontWeight::BOLD).child(g.label.clone()))
            .child(div().text_size(px(12.)).text_color(t.faint).child(g.rows.len().to_string()))
            .child(div().min_w_0().truncate().text_size(px(12.5)).text_color(t.muted).child(g.about.clone()));
        let mut rows = div().flex().flex_col().gap(px(6.));
        for row in &g.rows {
            rows = rows.child(row_el(&t, row, m.backlog.lens, cx));
        }
        col = col.child(div().flex().flex_col().gap(px(6.)).child(head).child(rows));
    }

<<<<<<< Updated upstream
    // ---- the new goal
    let aside = draft_el(m, &t, &d, window, cx);

    let mut menus: Vec<AnyElement> = vec![];
    if let Some(at) = m.menu_open(MENU_PROJECT) {
        let mut list = kit::menu_box(&t, 220.).id("plan-project-menu");
        let mut opts = vec![("all".to_string(), "All projects".to_string())];
        opts.extend(projects.iter().map(|p| (s(p, "name").to_string(), format!("{} · {}", s(p, "name"), fmt::i(p, "untriaged")))));
        for (n, (v, l)) in opts.into_iter().enumerate() {
            let cur = m.backlog.project == v;
            list = list.child(kit::menu_item(&t, SharedString::from(format!("plan-project-{n}")), l, cur).on_click(cx.listener(move |m, _, _, cx| {
                m.menu = None;
                m.backlog.project = v.clone();
                m.backlog.data = None;
                resync(m, cx);
            })));
=======
    // ---- the issue beside it
    let aside_el = match &v.aside {
        AsideState::None => None,
        AsideState::Loading(e) => Some(ip::render_aside(&t, None, e.as_deref(), cx)),
        AsideState::Issue(view) => {
            let b = aside.as_ref().and_then(|(_, g)| g.as_ref()).and_then(|g| g.as_ref().ok()).cloned();
            Some(ip::render_aside(&t, b.as_ref().map(|b| (b, &**view)), None, cx))
>>>>>>> Stashed changes
        }
        menus.push(kit::popover(at, list.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())));
    }

    let focus = m.backlog.focus.clone().unwrap();
    div()
        .id("plan-page")
        .track_focus(&focus)
        .on_mouse_down(MouseButton::Left, cx.listener(|m, _, window, cx| {
            if let Some(f) = m.backlog.focus.as_ref() {
                window.focus(f, cx);
            }
        }))
        .on_key_down(cx.listener(on_key))
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .size_full()
        .overflow_y_scroll()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(16.))
                .w_full()
                .max_w(px(1320.))
                .px(px(28.))
                .pt(px(22.))
                .pb(px(40.))
                .child(header)
                .child(toolbar)
                .child(div().flex().items_start().gap(px(20.)).child(col).child(div().flex_none().w(px(380.)).child(aside))),
        )
        .children(menus)
        .into_any_element()
}

fn row_el(t: &Theme, v: &RowView, lens: Lens, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let (fg, bg) = ip::kind_colors(t, &v.kind);
    let r = v.r.clone();
    let on = v.picked;
    div()
        .id(SharedString::from(format!("plan-row-{}", v.r)))
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(10.))
        .py(px(8.))
        .rounded(px(9.))
        .border_1()
        .border_color(if on { t.goal_line } else { t.border })
        .bg(if on { t.goal_tint } else { t.card })
        .child(check_box(t, SharedString::from(format!("plan-check-{}", v.r)), if on { Check::All } else { Check::None }).on_click(cx.listener(move |m, _, _, cx| toggle(m, std::slice::from_ref(&r), !on, cx))))
        .child(kit::pill(fg, bg, ip::kind_label(&v.kind)))
        .child(kit::mono(t, v.r.clone()).flex_none().text_color(t.faint))
        .child({
            let r = v.r.clone();
            let hover = t.accent_fg;
            div()
                .id(SharedString::from(format!("plan-title-{}", v.r)))
                .flex_1()
                .min_w_0()
                .truncate()
                .cursor_pointer()
                .text_size(px(13.5))
                .hover(move |s| s.text_color(hover))
                .tooltip(kit::tip("Open the issue: what was said, its history, reopen"))
                .child(v.title.clone())
                .on_click(cx.listener(move |m, _, _, cx| m.open_issue(r.clone(), cx)))
        })
        .children(v.is_new.then(|| kit::pill(t.warn_fg, t.warn_soft, "NEW").h(px(18.)).px(px(6.))))
        .children((lens != Lens::Area).then(|| div().flex_none().max_w(px(140.)).truncate().text_size(px(12.)).text_color(t.muted).child(v.area.clone())))
        .child(impact_bars(t, SharedString::from(format!("plan-impact-{}", v.r)), &v.impact))
        .child(prio_pill(t, &v.priority))
}

fn move_menu(m: &MainWindow, t: &Theme, sel: &[String], cx: &mut Context<MainWindow>) -> impl IntoElement {
    let list = issues(m);
    let projects: Vec<String> = sel.iter().filter_map(|r| list.iter().find(|b| fmt::ref_of(b, "B") == *r)).map(|b| s(b, "project").to_string()).collect();
    let one = projects.first().filter(|p| projects.iter().all(|x| x == *p)).cloned();
    let mut goals: Vec<&Value> = ip::live_goals(&m.data.goals).into_iter().filter(|g| one.as_deref().is_none_or(|p| s(g, "project") == p)).collect();
    goals.sort_by(|a, b| s(a, "name").cmp(s(b, "name")));
    let mut box_ = kit::menu_box(t, 280.).id("plan-move-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    if goals.is_empty() {
        box_ = box_.child(div().p(px(8.)).text_size(px(12.5)).text_color(t.muted).child("No goals in this project yet. Make one on the right."));
    }
    for (n, g) in goals.into_iter().enumerate() {
        let id = fmt::i(g, "id");
        let label = format!("{} {}", fmt::ref_of(g, "G"), s(g, "name"));
        box_ = box_.child(kit::menu_item(t, SharedString::from(format!("plan-move-{n}")), label, false).on_click(cx.listener(move |m, _, _, cx| bulk(m, "goal", json!({"goal_id": id}), cx))));
    }
    box_
}

fn prio_menu(m: &MainWindow, t: &Theme, list: &[Value], sel: &[String], cx: &mut Context<MainWindow>) -> impl IntoElement {
    let _ = m;
    let mut box_ = kit::menu_box(t, 230.).id("plan-prio-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for (n, (p, what)) in [("p1", "Do first"), ("p2", "Normal"), ("p3", "When there’s time")].into_iter().enumerate() {
        let all = !sel.is_empty() && sel.iter().all(|r| list.iter().find(|b| fmt::ref_of(b, "B") == *r).is_some_and(|b| s(b, "priority") == p));
        box_ = box_.child(
            kit::menu_item(t, SharedString::from(format!("plan-prio-{n}")), "", all)
                .child(prio_pill(t, p))
                .child(div().flex_1().child(what))
                .child(kbd(t, &(n + 1).to_string()))
                .on_click(cx.listener(move |m, _, _, cx| bulk(m, "priority", json!({"priority": p}), cx))),
        );
    }
    box_
}

fn draft_el(m: &mut MainWindow, t: &Theme, d: &DraftView, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let count = if d.count == 0 {
        String::new()
    } else if d.waves.is_empty() {
        format!("{} · not planned", fmt::plural(d.count as i64, "issue", "issues"))
    } else {
        format!("{} · {}", fmt::plural(d.count as i64, "issue", "issues"), fmt::plural(d.waves.len() as i64, "wave", "waves"))
    };
    let _ = window;
    // The plan names the goal; until there's a plan it has none.
    let name = (!d.waves.is_empty() && !d.suggest.is_empty()).then(|| div().text_size(px(16.)).font_weight(FontWeight::BOLD).child(d.suggest.clone()));
    let mut col = div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .p(px(16.))
        .rounded(px(14.))
        .border_1()
        .border_color(t.goal_line)
        .bg(t.card)
        .child(div().flex().items_center().justify_between().child(kit::h3(t, "New backlog goal").text_color(t.goal)).child(div().text_size(px(12.)).text_color(t.faint).child(count)))
        .children(name);
    if d.count == 0 {
        col = col.child(div().p(px(18.)).rounded(px(10.)).border_1().border_color(t.border_2).text_size(px(13.)).text_color(t.muted).flex().justify_center().child("Check issues on the left to add them to this goal."));
    }
    if d.mixed {
        col = col.child(div().text_size(px(12.5)).text_color(t.down).child("A goal holds one project's work. Check issues from one project."));
    }
    if d.planning {
        col = col.child(div().p(px(12.)).rounded(px(10.)).bg(t.goal_tint).text_size(px(13.)).text_color(t.goal).child("Claude is planning the waves…"));
    }
    if let Some(note) = &d.note {
        col = col.child(div().text_size(px(12.)).text_color(t.warn_fg).child(note.clone()));
    }
    for (wi, w) in d.waves.iter().enumerate() {
        let mut wave = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(10.))
            .rounded(px(10.))
            .border_1()
            .border_color(t.border)
            .bg(t.tint)
            .child(div().flex().justify_between().child(div().text_size(px(13.)).font_weight(FontWeight::BOLD).child(w.name.clone())).child(div().text_size(px(11.5)).text_color(t.faint).child(fmt::plural(w.items.len() as i64, "issue", "issues"))))
            .children((!w.why.is_empty()).then(|| div().text_size(px(12.)).text_color(t.muted).child(w.why.clone())));
        for it in &w.items {
            wave = wave.child(item_el(t, it, &format!("w{wi}"), cx));
        }
        col = col.child(wave);
    }
    if !d.unplanned.is_empty() {
        let title = if d.never_planned { "Not planned yet" } else { "Added since the last plan" };
        let mut box_ = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(10.))
            .rounded(px(10.))
            .border_1()
            .border_dashed()
            .border_color(t.goal_line)
            .bg(t.goal_tint)
            .child(div().flex().justify_between().child(div().text_size(px(13.)).font_weight(FontWeight::BOLD).text_color(t.goal).child(title)).child(div().text_size(px(11.5)).text_color(t.muted).child(fmt::plural(d.unplanned.len() as i64, "issue", "issues"))));
        if d.never_planned {
            box_ = box_.child(div().text_size(px(12.)).text_color(t.muted).child("Claude splits these into waves: what goes first, what can run side by side, and what has to wait so terminals don't edit the same files."));
        }
        for it in &d.unplanned {
            box_ = box_.child(item_el(t, it, "u", cx));
        }
        let label = if d.never_planned { "Plan waves with Claude" } else { "Re-plan with Claude" };
        let b = if d.never_planned { kit::btn_primary(t, "plan-plan", label).bg(t.goal).border_color(t.goal).text_color(on_goal(t)) } else { kit::btn(t, "plan-plan", label).text_color(t.goal).border_color(t.goal_line) };
        box_ = box_.child(b.mt(px(4.)).h(px(34.)).on_click(cx.listener(|m, _, _, cx| plan_waves(m, cx))));
        col = col.child(box_);
    }
    let label = if d.can_create() {
        "Create goal".to_string()
    } else if d.stale() {
        "Re-plan before creating".into()
    } else if d.count > 0 && d.never_planned && !d.planning {
        "Plan before creating".into()
    } else {
        "Create goal".into()
    };
    let mut btn = kit::btn_primary(t, "plan-create", label).h(px(38.)).bg(t.goal).border_color(t.goal).text_color(on_goal(t));
    if d.can_create() && !m.backlog.busy {
        let dd = d.clone();
        btn = btn.on_click(cx.listener(move |m, _, _, cx| create(m, &dd, cx)));
    } else {
        btn = kit::disabled(btn);
    }
    // Nothing checked: nothing to make a goal of, so no button.
    if d.count > 0 {
        col = col.child(btn);
    }
    if let Some((r, name)) = m.backlog.just_made.clone() {
        let rr = r.clone();
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .p(px(10.))
                .rounded(px(9.))
                .bg(t.up_soft)
                .text_size(px(13.))
                .text_color(t.up_fg)
                .child(div().flex_1().child(format!("{r} · {name} created.")))
                .child(kit::link(t, "plan-open-goal", "Open it").on_click(cx.listener(move |m, _, _, cx| m.go(Page::Goal(rr.clone()), cx)))),
        );
    }
    col.into_any_element()
}

fn item_el(t: &Theme, it: &Item, prefix: &str, cx: &mut Context<MainWindow>) -> Div {
    let r = it.r.clone();
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.5))
        .child(kit::mono(t, it.r.clone()).flex_none().w(px(36.)).text_color(t.faint))
        .child(div().flex_1().min_w_0().truncate().text_color(t.text_2).child(it.title.clone()))
        .children((!it.after.is_empty()).then(|| div().flex_none().text_size(px(11.5)).text_color(t.muted).child(format!("after {}", it.after.join(", ")))))
        .children((!it.priority.is_empty()).then(|| prio_pill(t, &it.priority)))
        .child(
            div()
                .id(SharedString::from(format!("plan-rm-{prefix}-{}", it.r)))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(22.))
                .rounded(px(6.))
                .bg(t.seg)
                .text_color(t.muted)
                .cursor_pointer()
                .tooltip(kit::tip("Remove from this goal"))
                .child("×")
                .on_click(cx.listener(move |m, _, _, cx| toggle(m, std::slice::from_ref(&r), false, cx))),
        )
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's `test` attribute, which `#[gpui_kit::test]` then recurses on.
    use super::{Check, Item, Lens, create, draft, goal_waves, groups, issues, plan_waves, sync, toggle, bulk};
    use crate::app::Page;
    use crate::backend::Backend;
    use crate::fmt;
    use crate::parity;
    use serde_json::{Value, json};

    fn b(r: &str, kind: &str, group: &str, area: &str, impact: &str, prio: &str) -> Value {
        json!({"ref": r, "kind": kind, "title": format!("{r} title"), "group": group, "group_about": format!("{group} about"),
               "area": area, "impact": impact, "priority": prio, "project": "webapp"})
    }

    fn list() -> Vec<Value> {
        vec![
            b("B1", "bug", "Runner", "Runner", "high", "p1"),
            b("B2", "gap", "Runner", "Runner", "med", "p2"),
            b("B3", "clean", "Tidy", "App", "low", "p3"),
        ]
    }

    #[::core::prelude::v1::test]
<<<<<<< Updated upstream
    fn groups_follow_the_lens_and_the_checks() {
        let picked = vec!["B1".to_string()];
        let g = groups(&list(), Lens::Similar, [true; 4], &picked);
        assert_eq!(g.iter().map(|g| (g.label.as_str(), g.rows.len(), g.check)).collect::<Vec<_>>(), [("Runner", 2, Check::Some), ("Tidy", 1, Check::None)]);
        assert_eq!(g[0].about, "Runner about");
        let g = groups(&list(), Lens::Impact, [true; 4], &picked);
        assert_eq!(g.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(), ["High impact", "Medium impact", "Low impact"]);
        assert_eq!(g[0].check, Check::All);
        let g = groups(&list(), Lens::Priority, [true, false, true, true], &[]);
        assert_eq!(g.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(), ["P1", "P3"], "test gaps hidden");
=======
    fn page_matches_web() {
        golden("backlog").only("page").check(|i| {
            let f = &i["filter"];
            let mut st = State {
                project: s(f, "project").into(),
                goal: s(f, "goal").into(),
                kind: s(f, "kind").into(),
                state: s(f, "state").into(),
                sort: s(f, "sort").into(),
                ..Default::default()
            };
            st.open_all = i["openAll"].as_i64();
            let mut kept: HashMap<String, Value> = i["kept"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
            let order: Vec<String> = arr(i, "order").iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            let backlog = i.get("backlog").filter(|b| b.is_object());
            let rows = backlog.map(|b| with_kept(arr(b, "issues"), &mut kept, &order)).unwrap_or_default();
            let now = Instant::now();
            let notes: HashMap<String, (String, bool, Instant)> = i["notes"].as_object().map(|o| o.iter().map(|(k, n)| (k.clone(), (s(n, "text").to_string(), fmt::b(n, "err"), now))).collect()).unwrap_or_default();
            let busy: HashSet<String> = arr(i, "busy").iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            let goals = arr(i, "goals").to_vec();
            let state = json!({"projects": i["projects"], "counts": i["counts"]});
            let aside = fmt::opt_s(i, "selected").map(|r| (r.to_string(), Some(Ok(i["issue"].clone()))));
            let v = page_view(&PageInput {
                st: &st,
                ctx: Ctx { jira: fmt::b(i, "jira"), goals: &goals, notes: &notes, busy: &busy },
                state: &state,
                backlog,
                backlog_err: i["backlogErr"].as_str(),
                rows,
                aside,
            });
            let rows: Vec<Value> = match &v.list {
                ListView::Rows { rows, .. } => rows
                    .iter()
                    .map(|r| {
                        let (text, acts) = r.text_and_acts();
                        json!({"text": text, "acts": acts})
                    })
                    .collect(),
                ListView::Loading(_) => vec![],
            };
            let (aside, aside_acts) = v.aside_text_and_acts();
            json!({
                "header": v.header_text(), "header_acts": ["add-issue"],
                "filters": v.filters_text(), "filter_values": [v.project_label, v.goal_label],
                "state_options": v.state_options, "list_head": v.list_head(),
                "rows": rows, "aside": aside, "aside_acts": aside_acts,
            })
        });
>>>>>>> Stashed changes
    }

    #[::core::prelude::v1::test]
    fn the_draft_shows_the_plan_for_what_is_still_checked() {
        let picked: Vec<String> = ["B1", "B2"].iter().map(|s| s.to_string()).collect();
        let d = draft(&list(), None, &picked);
        assert!(d.never_planned && !d.can_create());
        assert_eq!(d.unplanned.len(), 2);

        let plan = json!({"id": 1, "state": "ready", "ids": ["B1", "B2", "B3"], "name": "Runner reliability", "waves": [
            {"why": "First", "items": [{"ref": "B1", "after": []}]},
            {"why": "Then", "items": [{"ref": "B3", "after": []}, {"ref": "B2", "after": ["B3"]}]}]});
        let d = draft(&list(), Some(&plan), &picked);
        assert!(d.can_create(), "B3 unchecked: it drops out, no re-plan needed");
        assert_eq!(d.waves.len(), 2);
        assert_eq!(d.waves[1].items, vec![Item { r: "B2".into(), title: "B2 title".into(), priority: "p2".into(), after: vec![] }]);
        assert_eq!(goal_waves(&d)[1]["items"][0], json!({"ref": "B2", "after": []}));

        // Checking one more after planning asks for a re-plan.
        let more: Vec<String> = ["B1", "B2"].iter().map(|s| s.to_string()).collect();
        let plan2 = json!({"id": 2, "state": "ready", "ids": ["B1"], "waves": [{"why": "", "items": [{"ref": "B1"}]}]});
        let d = draft(&list(), Some(&plan2), &more);
        assert!(d.stale() && !d.can_create());
        assert_eq!(d.unplanned.iter().map(|i| i.r.as_str()).collect::<Vec<_>>(), ["B2"]);

        let planning = json!({"id": 3, "state": "planning", "ids": ["B1", "B2"]});
        let d = draft(&list(), Some(&planning), &more);
        assert!(d.planning && d.unplanned.is_empty() && !d.can_create());
    }

    #[::core::prelude::v1::test]
    fn a_goal_holds_one_project() {
        let mut l = list();
        l[2]["project"] = json!("api");
        let picked: Vec<String> = ["B1", "B3"].iter().map(|s| s.to_string()).collect();
        let plan = json!({"id": 1, "state": "ready", "ids": ["B1", "B3"], "waves": [{"why": "", "items": [{"ref": "B1"}, {"ref": "B3"}]}]});
        let d = draft(&l, Some(&plan), &picked);
        assert!(d.mixed && !d.can_create());
    }

    #[gpui_kit::test]
    fn checking_planning_and_creating_send_what_the_board_needs(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = parity::window(cx);
        let mk = |title: &str, kind: &str| fmt::ref_of(&rec.board().post("backlog", json!({"title": title, "kind": kind, "project": "webapp"})).unwrap(), "B");
        let (a, b) = (mk("Restarts ignore the cap", "bug"), mk("No test for spool replay", "gap"));
        let later = mk("Old prefs keys", "clean");
        w.update(cx, |m, _, cx| {
            m.go(Page::Backlog, cx);
            sync(m, cx);
        })
        .unwrap();
        parity::settle(cx);
        w.update(cx, |m, _, cx| {
            assert!(issues(m).iter().any(|x| fmt::ref_of(x, "B") == a), "untriaged issues are listed");
            toggle(m, &[a.clone(), b.clone()], true, cx);
            plan_waves(m, cx);
        })
        .unwrap();
        parity::settle(cx);
        assert_eq!(rec.last("backlog/plan"), Some(json!({"ids": [a, b]})));
        w.update(cx, |m, _, cx| {
            let d = draft(&issues(m), m.backlog.plan.as_ref(), &m.backlog.picked);
            assert!(d.can_create(), "planned without Claude: priority waves");
            create(m, &d, cx);
        })
        .unwrap();
        parity::settle(cx);
        let body = rec.last("backlog/goal").expect("made a goal");
        assert!(!body["name"].as_str().unwrap().is_empty(), "the goal takes the plan's name");
        let sent: Vec<String> = body["waves"].as_array().unwrap().iter().flat_map(|w| w["items"].as_array().unwrap().iter().map(|i| i["ref"].as_str().unwrap().to_string())).collect();
        assert_eq!(sent.len(), 2);
        w.update(cx, |m, _, cx| {
            assert!(m.backlog.picked.is_empty() && m.backlog.just_made.is_some());
            assert!(matches!(&m.page, Page::Goal(r) if r.starts_with('G')), "the new goal opens");
            m.go(Page::Backlog, cx);
            // Then the selection bar: defer what's left.
            m.backlog.synced = None;
            sync(m, cx);
        })
        .unwrap();
        parity::settle(cx);
        w.update(cx, |m, _, cx| {
            assert!(!issues(m).iter().any(|x| fmt::ref_of(x, "B") == a), "in a goal now: triaged");
            toggle(m, std::slice::from_ref(&later), true, cx);
            bulk(m, "defer", json!({}), cx);
        })
        .unwrap();
        parity::settle(cx);
        assert_eq!(rec.last("backlog/bulk"), Some(json!({"ids": [later], "action": "defer"})));
    }
}
