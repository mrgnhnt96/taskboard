//! The Backlog page: triage the backlog by making goals from it. An open issue that isn't in a goal
//! yet is untriaged; it's triaged once it's in a goal, deferred or closed as won't do.
//!
//! The untriaged issues (`GET /backlog/triage`) come grouped by a lens (Similar, Area, Impact or
//! Priority), narrowed by kind and by the filter's words. The board sends every group's true total
//! and issues, in its order, so a group's count and Select all cover the issues not loaded yet.
//! Picking issues adds them to the new goal on the right and to the bar's actions (move to a goal,
//! set priority, defer, won't do), each with a key. Claude plans the goal's waves when asked
//! (`POST /backlog/plan`); picking more afterwards asks for a re-plan, removing one doesn't. Create
//! makes the goal (`POST /backlog/goal`): one planned task per issue, in its wave.
//!
//! The list comes a page at a time, group by group, and draws only the lines in view: an older page
//! is asked for as it nears its end. ↑ ↓ move through the issues (the open one, while the issue
//! panel shows), space picks and ↩ opens; ⌘F goes to the filter.
use crate::app::{MainWindow, Page, Panel, Toast};
use crate::fmt::{self, arr, s};
use crate::theme::Theme;
use crate::ui::focus;
use crate::ui::issue_panel as ip;
use crate::ui::kit;
use crate::ui::paged::Paged;
use crate::ui::text_input::FieldChanged;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

const SYNC_EVERY: Duration = Duration::from_secs(3);
/// While Claude plans, ask for the plan this often.
const PLAN_EVERY: Duration = Duration::from_millis(1500);
/// Issues per page of `GET /backlog/triage`.
pub const PAGE: usize = 100;
/// Ask for the next page this many lines before the list's end.
const AHEAD: usize = 40;
/// Below this window width the rows stack and the new goal sits above the list.
const NARROW: f32 = 1180.;
/// A toast with an Open it link stays this long.
const OPEN_TOAST: Duration = Duration::from_secs(8);
const KINDS: [&str; 4] = ["bug", "gap", "follow", "clean"];
const KIND_LABELS: [&str; 4] = ["Bugs", "Gaps", "Follow-ups", "Clean-ups"];
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

    fn param(self) -> &'static str {
        match self {
            Lens::Similar => "similar",
            Lens::Area => "area",
            Lens::Impact => "impact",
            Lens::Priority => "priority",
        }
    }
}

/// What the held issues were asked for: another lens, kind, project or filter starts over.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Scope {
    lens: Lens,
    kinds: [bool; 4],
    project: String,
    q: String,
}

pub struct State {
    pub lens: Lens,
    /// Kinds shown (bug, gap, follow, clean): all on is All, one on is that kind.
    pub kinds: [bool; 4],
    pub project: String,
    /// Picked issues, in the order picked: the new goal's issues and the bar's selection.
    pub picked: Vec<String>,
    focus: Option<FocusHandle>,
    filter: Option<kit::Input>,
    /// The filter's words, trimmed.
    q: String,
    /// Groups closed by their chevron, by key. Another lens opens them all.
    folded: HashSet<String>,
    /// The issue ↑ ↓ are on.
    cursor: Option<String>,
    /// The picked issues as the board has them, whichever page or project they're on.
    picks: Rc<Vec<Value>>,
    /// The toast whose Open it opens a task: when it was shown, and the task.
    pub open_toast: Option<(Instant, String)>,
    /// `GET /backlog/triage`'s first page (the counts, groups and projects), and the scope it was for.
    data: Option<Rc<Value>>,
    data_scope: Option<Scope>,
    /// The issues held so far, page by page, and the scope they were asked for.
    held: Paged,
    scope: Option<Scope>,
    list: Option<ListState>,
    view: Option<View>,
    narrow: bool,
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
            filter: None,
            q: String::new(),
            folded: HashSet::new(),
            cursor: None,
            picks: Rc::default(),
            open_toast: None,
            data: None,
            data_scope: None,
            held: Paged::default(),
            scope: None,
            list: None,
            view: None,
            narrow: false,
            err: None,
            plan: None,
            synced: None,
            generation: 0,
            busy: false,
            just_made: None,
        }
    }
}

impl State {
    fn scope_now(&self) -> Scope {
        Scope { lens: self.lens, kinds: self.kinds, project: self.project.clone(), q: self.q.clone() }
    }

    /// The query every page is asked with.
    fn query(&self) -> Vec<(&'static str, String)> {
        let mut q = vec![("project", self.project.clone()), ("lens", self.lens.param().to_string())];
        if self.kinds != [true; 4] {
            q.push(("kinds", KINDS.iter().zip(self.kinds).filter(|(_, on)| *on).map(|(k, _)| *k).collect::<Vec<_>>().join(",")));
        }
        if !self.q.is_empty() {
            q.push(("q", self.q.clone()));
        }
        q
    }
}

// ------------------------------------------------------------------ view model

/// One line of the list: each is drawn only while it's in view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line {
    /// Loading, can't load, or the board isn't running, before anything has loaded.
    State,
    /// The list shown is the last one that loaded: a refresh failed or the board stopped.
    Stale,
    Empty,
    Grouping,
    Group(usize),
    Row(usize, usize),
    End,
}

/// The grouped issues and the list's lines, rebuilt when the issues, groups, picks or folds change.
struct View {
    key: (u64, u64, Vec<String>, Vec<String>, bool, bool),
    scope: Option<Scope>,
    groups: Rc<Vec<GroupView>>,
    lines: Vec<Line>,
    more: bool,
}

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
    pub project: String,
    pub area: String,
    pub impact: String,
    pub priority: String,
    pub is_new: bool,
    pub picked: bool,
}

#[derive(Clone, Debug)]
pub struct GroupView {
    pub key: String,
    pub label: String,
    pub about: String,
    /// Every issue in the group, loaded or not.
    pub total: usize,
    pub refs: Vec<String>,
    /// The group's issues loaded so far, in the board's order.
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
    /// None of the picked issues has been planned.
    pub never_planned: bool,
    pub planning: bool,
    pub note: Option<String>,
    /// The plan's suggested name.
    pub suggest: String,
    /// More than one project among the picked issues (a goal holds one).
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

fn row_view(b: &Value, picked: &[String]) -> RowView {
    let r = fmt::ref_of(b, "B");
    RowView {
        picked: picked.contains(&r),
        r,
        kind: s(b, "kind").to_string(),
        title: s(b, "title").to_string(),
        project: s(b, "project").to_string(),
        area: s(b, "area").to_string(),
        impact: s(b, "impact").to_string(),
        priority: s(b, "priority").to_string(),
        is_new: is_new(b),
    }
}

/// The board's groups, in its order with their true totals, each holding the issues loaded so far.
pub fn groups(data: &Value, issues: &[Value], lens: Lens, picked: &[String]) -> Vec<GroupView> {
    let by_ref: HashMap<String, &Value> = issues.iter().map(|b| (fmt::ref_of(b, "B"), b)).collect();
    arr(data, "groups")
        .iter()
        .map(|g| {
            let key = s(g, "key").to_string();
            let refs: Vec<String> = arr(g, "refs").iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            let rows: Vec<RowView> = refs.iter().filter_map(|r| by_ref.get(r)).map(|b| row_view(b, picked)).collect();
            let n = refs.iter().filter(|r| picked.contains(r)).count();
            let check = if n == 0 { Check::None } else if n == refs.len() { Check::All } else { Check::Some };
            let label = match lens {
                Lens::Impact => impact_label(&key).to_string(),
                Lens::Priority => key.to_uppercase(),
                _ if key.is_empty() => "Not grouped yet".to_string(),
                _ => key.clone(),
            };
            let total = g.get("total").and_then(Value::as_u64).map(|n| n as usize).unwrap_or(refs.len());
            GroupView { key, label, about: s(g, "about").to_string(), total, refs, rows, check }
        })
        .collect()
}

/// "B9", "B9 and B20", "B9, B20 and B39", then "and 2 more".
pub fn names(refs: &[String]) -> String {
    match refs {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} and {b}"),
        [a, b, c] => format!("{a}, {b} and {c}"),
        [a, b, c, rest @ ..] => format!("{a}, {b}, {c} and {} more", rest.len()),
    }
}

/// The new goal: the plan's waves (only picked issues; `after` only to those), and the picked
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
    }
}

/// `POST /backlog/goal`'s waves, from what the draft shows.
pub fn goal_waves(d: &DraftView) -> Value {
    json!(d.waves.iter().map(|w| json!({"why": w.why, "items": w.items.iter().map(|i| json!({"ref": i.r, "after": i.after})).collect::<Vec<_>>()})).collect::<Vec<_>>())
}

/// The goals picked issues can move into: live, not finished, and in their one project.
fn move_goals<'a>(goals: &'a [Value], project: Option<&str>) -> Vec<&'a Value> {
    let mut out: Vec<&Value> = ip::live_goals(goals).into_iter().filter(|g| fmt::opt_s(g, "finished_at").is_none()).filter(|g| project.is_none_or(|p| s(g, "project") == p)).collect();
    out.sort_by(|a, b| crate::ui::modals::locale_cmp(s(a, "name"), s(b, "name")));
    out
}

/// What a goal in the move menu is doing.
fn goal_doing(g: &Value) -> String {
    if fmt::b(g, "paused") {
        "Paused".into()
    } else if fmt::i(g, "active") > 0 {
        "Running".into()
    } else if arr(g, "tasks").iter().all(|t| matches!(s(t, "status"), "planned" | "")) {
        "Not started".into()
    } else {
        crate::ui::goal::goal_state(g).0
    }
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

fn issues(m: &MainWindow) -> Rc<Vec<Value>> {
    m.backlog.held.rows.clone()
}

/// The held issues and the picked ones from elsewhere: what the new goal and the menus read.
fn pool(m: &MainWindow) -> Vec<Value> {
    let held = issues(m);
    let have: HashSet<String> = held.iter().map(|b| fmt::ref_of(b, "B")).collect();
    held.iter().cloned().chain(m.backlog.picks.iter().filter(|b| !have.contains(&fmt::ref_of(b, "B"))).cloned()).collect()
}

/// The picked issues: the bar acts on all of them, loaded or not.
fn selected(m: &MainWindow) -> Vec<String> {
    m.backlog.picked.clone()
}

/// Add an issue to the new goal from elsewhere (the issue panel's Select for goal).
pub fn pick(m: &mut MainWindow, r: &str) {
    if !m.backlog.picked.iter().any(|x| x == r) {
        m.backlog.picked.push(r.to_string());
    }
    m.backlog.just_made = None;
    m.backlog.synced = None;
}

/// A toast with Open it, which opens `task` in the task panel.
pub fn toast_open(m: &mut MainWindow, text: String, task: String, cx: &mut Context<MainWindow>) {
    let at = Instant::now();
    m.toasts.retain(|t| t.err);
    m.toasts.push(Toast { text, err: false, at, undo: None });
    m.backlog.open_toast = Some((at, task));
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(OPEN_TOAST).await;
        let _ = this.update(cx, |m, cx| m.dismiss_toast(at, cx));
    })
    .detach();
    cx.notify();
}

fn sync(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let now = m.backlog.scope_now();
    if m.backlog.scope.as_ref() != Some(&now) {
        m.backlog.held = Paged::default();
        m.backlog.scope = Some(now.clone());
        m.backlog.synced = None;
    }
    let planning = m.backlog.plan.as_ref().is_some_and(|p| s(p, "state") == "planning");
    let every = if planning { PLAN_EVERY } else { SYNC_EVERY };
    if m.backlog.synced.is_some_and(|t| t.elapsed() < every) {
        return;
    }
    m.backlog.synced = Some(Instant::now());
    m.backlog.generation += 1;
    let generation = m.backlog.generation;
    let query = m.backlog.query();
    let asked = m.backlog.picked.clone();
    // Any issue held can be triaged or changed elsewhere, so a refresh asks for them all again.
    let recheck = m.backlog.held.recheck(|_| true);
    let backend = m.backend.clone();
    cx.spawn(async move |this, cx| {
        let picks_q = asked.clone();
        let got = cx
            .background_executor()
            .spawn(async move {
                let picks = (!picks_q.is_empty()).then(|| backend.get("backlog/triage", &[("ids", picks_q.join(",")), ("project", "all".into()), ("limit", "500".into())]).map_err(|e| e.message));
                (Paged::fetch(&*backend, "backlog/triage", "issues", &query, PAGE, recheck), backend.get("backlog/plan", &[]), picks)
            })
            .await;
        let _ = this.update(cx, |m, cx| {
            if m.backlog.generation != generation {
                return;
            }
            let (list, plan, picks) = got;
            match list {
                Ok(f) => {
                    m.backlog.data = Some(Rc::new(f.first().clone()));
                    m.backlog.data_scope = Some(now);
                    m.backlog.held.land("issues", f);
                    m.backlog.err = None;
                }
                Err(e) => m.backlog.err = Some(e),
            }
            if let Some(Ok(p)) = picks {
                let came: HashSet<String> = arr(&p, "issues").iter().map(|b| fmt::ref_of(b, "B")).collect();
                // Triaged elsewhere since it was picked: it's no longer the goal's to take.
                m.backlog.picked.retain(|r| !asked.contains(r) || came.contains(r));
                m.backlog.picks = Rc::new(arr(&p, "issues").to_vec());
            } else if asked.is_empty() {
                m.backlog.picks = Rc::default();
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

/// Fetch the next older page once the list nears its end (a short list is always near it).
fn load_older(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let st = &mut m.backlog;
    let (Some(before), Some(l), Some(v)) = (st.held.next.clone(), &st.list, &st.view) else { return };
    if st.held.loading || v.lines.len() > l.logical_scroll_top().item_ix + AHEAD {
        return;
    }
    st.held.loading = true;
    let backend = m.backend.clone();
    let mut query = st.query();
    query.push(("limit", PAGE.to_string()));
    query.push(("before", before.clone()));
    cx.spawn(async move |this, cx| {
        let got = cx.background_executor().spawn(async move { backend.get("backlog/triage", &query).map_err(|e| e.message) }).await;
        let _ = this.update(cx, |m, cx| {
            m.backlog.held.land_older("issues", &before, got);
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
    let mut added = false;
    for r in refs {
        let has = m.backlog.picked.contains(r);
        if on && !has {
            m.backlog.picked.push(r.clone());
            added = true;
        } else if !on && has {
            m.backlog.picked.retain(|x| x != r);
        }
    }
    m.backlog.just_made = None;
    if added {
        // The picks' details come with the next fetch, for those not loaded yet.
        m.backlog.synced = None;
    }
    cx.notify();
}

fn clear(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    m.backlog.picked.clear();
    if m.menu.as_ref().is_some_and(|(k, _)| k.starts_with("plan-")) {
        m.menu = None;
    }
    cx.notify();
}

/// Esc on the Backlog page: close its menu and unpick everything. False when there was nothing to
/// do, or a task or issue panel (or a dialog) is open: Esc closes that first.
pub fn escape(m: &mut MainWindow, cx: &mut Context<MainWindow>) -> bool {
    if m.page != Page::Backlog || m.backlog.picked.is_empty() || m.panel.is_some() || m.modal.is_some() {
        return false;
    }
    clear(m, cx);
    true
}

/// The selection bar's actions: `POST /backlog/bulk` for the picked issues.
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
    let what = action.to_string();
    let prio = s(&body, "priority").to_uppercase();
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
            m.toast(bulk_note(&what, &ids, &prio, &v), false, cx);
            resync(m, cx);
        },
        |m, e, cx| {
            m.backlog.busy = false;
            m.toast(e.message, true, cx);
        },
    );
}

/// What the bar's action did, naming the issues.
fn bulk_note(action: &str, ids: &[String], prio: &str, v: &Value) -> String {
    let who = names(ids);
    match action {
        "defer" => format!("Deferred {who}"),
        "drop" => format!("Closed {who} as won’t do"),
        "goal" => moved_note(&who, v),
        _ => format!("Set {who} to {prio}"),
    }
}

/// The toast once issues move into a goal: its tasks are planned, or queued when the goal is running.
fn moved_note(issues: &str, v: &Value) -> String {
    let tasks = v["tasks"].as_array().cloned().unwrap_or_default();
    let queued = tasks.iter().filter(|t| s(t, "status") == "queued").count();
    match (queued, tasks.len() - queued) {
        (0, _) => format!("Moved {issues} into the goal as planned tasks"),
        (_, 0) => format!("Moved {issues} into the goal. It’s running, so they’re queued"),
        (q, p) => format!("Moved {issues} into the goal: {q} queued, {p} planned"),
    }
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

/// The issues ↑ ↓ step through: each shown row, in order, with its line.
fn shown_rows(m: &MainWindow) -> Vec<(usize, String)> {
    let Some(v) = m.backlog.view.as_ref() else { return vec![] };
    v.lines
        .iter()
        .enumerate()
        .filter_map(|(ix, l)| match l {
            Line::Row(gi, ri) => Some((ix, v.groups[*gi].rows[*ri].r.clone())),
            _ => None,
        })
        .collect()
}

/// The issue the issue panel shows, if it's open.
fn open_issue(m: &MainWindow) -> Option<String> {
    match &m.panel {
        Some(Panel::Issue { r }) => Some(r.clone()),
        _ => None,
    }
}

/// ↑ or ↓: the next issue in the list. With the issue panel open, that issue opens in it.
fn step(m: &mut MainWindow, down: bool, cx: &mut Context<MainWindow>) {
    let rows = shown_rows(m);
    if rows.is_empty() {
        return;
    }
    let from = open_issue(m).or_else(|| m.backlog.cursor.clone());
    let at = from.and_then(|r| rows.iter().position(|(_, x)| *x == r));
    let next = match at {
        None => 0,
        Some(i) if down => (i + 1).min(rows.len() - 1),
        Some(i) => i.saturating_sub(1),
    };
    let (ix, r) = rows[next].clone();
    m.backlog.cursor = Some(r.clone());
    if let Some(l) = &m.backlog.list {
        l.scroll_to_reveal_item(ix);
    }
    if open_issue(m).is_some_and(|o| o != r) {
        m.open_issue(r, cx);
    }
    cx.notify();
}

fn focus_filter(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    if let Some(i) = &m.backlog.filter {
        window.focus(&i.focus, cx);
    }
}

/// The page's keys. ⌘F the filter; in it, ↩ goes to the list and esc clears it. On the list, ↑ ↓
/// move, space picks, ↩ opens. With issues picked and no panel open: M move, P priority (1 2 3 set
/// it), D defer, X won't do.
fn on_key(m: &mut MainWindow, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<MainWindow>) {
    // A menu or the project filter's search field has the keys first.
    if m.modal.is_some() || m.menu.is_some() {
        return;
    }
    let ks = &ev.keystroke;
    if ks.modifiers.platform && ks.key == "f" {
        focus_filter(m, window, cx);
        cx.stop_propagation();
        return;
    }
    let in_filter = m.backlog.filter.as_ref().is_some_and(|i| i.focus.is_focused(window));
    if in_filter {
        let Some(i) = m.backlog.filter.as_ref() else { return };
        match i.on_key(ev, cx) {
            kit::KeyOutcome::Cancel => {
                if i.text(cx).is_empty() {
                    if let Some(f) = m.backlog.focus.clone() {
                        window.focus(&f, cx);
                    }
                } else {
                    i.clear(cx);
                }
            }
            kit::KeyOutcome::Submit => {
                if let Some(f) = m.backlog.focus.clone() {
                    window.focus(&f, cx);
                }
                if m.backlog.cursor.is_none() {
                    m.backlog.cursor = shown_rows(m).first().map(|(_, r)| r.clone());
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
        return;
    }
    if focus::typing(window) || ks.modifiers.platform || ks.modifiers.control || ks.modifiers.alt {
        return;
    }
    let on_page = m.backlog.focus.as_ref().is_some_and(|f| f.is_focused(window));
    match ks.key.as_str() {
        "down" => step(m, true, cx),
        "up" => step(m, false, cx),
        "space" if on_page && m.backlog.cursor.is_some() => {
            let r = m.backlog.cursor.clone().unwrap_or_default();
            let on = !m.backlog.picked.contains(&r);
            toggle(m, &[r], on, cx);
        }
        "enter" if on_page && m.backlog.cursor.is_some() => {
            let r = m.backlog.cursor.clone().unwrap_or_default();
            m.open_issue(r, cx);
        }
        k if m.panel.is_none() && !selected(m).is_empty() => {
            let at = point(px(0.), px(0.));
            match k {
                "m" => m.toggle_menu(MENU_MOVE, at, cx),
                "p" => m.toggle_menu(MENU_PRIO, at, cx),
                "1" | "2" | "3" => bulk(m, "priority", json!({"priority": format!("p{k}")}), cx),
                "d" => bulk(m, "defer", json!({}), cx),
                "x" => bulk(m, "drop", json!({}), cx),
                _ => return,
            }
        }
        _ => return,
    }
    cx.stop_propagation();
}

// ------------------------------------------------------------------ render

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
        .border_color(if on { t.goal } else { t.control_line })
        .when(on, |d| d.bg(t.goal))
        .text_color(t.on_status)
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

/// P1 is the one to do first: an orange pill. P2 and P3 are plain words.
fn prio_pill(t: &Theme, p: &str) -> Div {
    if p == "p1" {
        kit::pill(t.warn_fg, t.warn_soft, "P1").h(px(18.)).px(px(7.))
    } else {
        div().flex_none().w(px(26.)).text_size(px(12.)).text_color(t.muted).child(p.to_uppercase())
    }
}

/// Impact as signal bars: three for high, two for medium, one for low.
fn impact_bars(t: &Theme, id: SharedString, impact: &str) -> Stateful<Div> {
    let n = match impact {
        "high" => 3,
        "med" => 2,
        _ => 1,
    };
    let mut d = div().id(id).flex().flex_none().items_end().gap(px(2.)).h(px(12.)).tooltip(kit::tip(impact_label(impact)));
    for (i, h) in [5., 8., 12.].into_iter().enumerate() {
        d = d.child(div().w(px(3.)).h(px(h)).rounded(px(1.)).bg(if i < n { t.text_2 } else { t.border_2 }));
    }
    d
}

fn bar_btn(t: &Theme, id: &'static str, label: &str, key: &str) -> Stateful<Div> {
    let h = t.panel_2;
    focus::stop(t, div().id(id))
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

/// A page state in place of the list: loading, can't load (with Try again), or the board is down.
fn state_box(t: &Theme, text: String, retry: bool, cx: &mut Context<MainWindow>) -> Div {
    div()
        .pt(px(14.))
        .child(
            div()
                .p(px(24.))
                .rounded(px(12.))
                .border_1()
                .border_color(t.border)
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.))
                .text_size(px(13.))
                .text_color(t.muted)
                .child(text)
                .children(retry.then(|| kit::btn_small(t, "plan-retry", "Try again").on_click(cx.listener(|m, _, _, cx| resync(m, cx))))),
        )
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    if m.backlog.focus.is_none() {
        m.backlog.focus = Some(cx.focus_handle());
    }
    if m.backlog.filter.is_none() {
        let input = kit::Input::new(cx, "Filter", false);
        cx.subscribe(&input.field, |m: &mut MainWindow, _, _: &FieldChanged, cx| {
            let q = m.backlog.filter.as_ref().map(|i| i.text(cx).trim().to_string()).unwrap_or_default();
            if q != m.backlog.q {
                m.backlog.q = q;
                m.backlog.cursor = None;
                cx.notify();
            }
        })
        .detach();
        m.backlog.filter = Some(input);
    }
    sync(m, cx);
    m.backlog.narrow = window.viewport_size().width < px(NARROW);
    sync_view(m);
    load_older(m, cx);
    let pool = pool(m);
    let d = draft(&pool, m.backlog.plan.as_ref(), &m.backlog.picked);
    let data = m.backlog.data.clone().unwrap_or_default();
    let untriaged = fmt::i(&data, "untriaged");
    let sel = selected(m);
    let narrow = m.backlog.narrow;

    // ---- header: the count, the project, the lens
    let title = div()
        .flex()
        .items_baseline()
        .gap(px(10.))
        .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Backlog"))
        .children(m.backlog.data.is_some().then(|| {
            div().id("plan-count").text_size(px(15.)).text_color(t.muted).child(untriaged.to_string()).tooltip(kit::tip(format!("{} not in a goal", fmt::plural(untriaged, "issue", "issues"))))
        }));
    let projects = arr(&data, "projects");
    let project_btn = (projects.len() > 1 || m.backlog.project != "all").then(|| {
        let counts: Vec<(String, usize)> = projects.iter().map(|p| (s(p, "name").to_string(), fmt::i(p, "untriaged") as usize)).collect();
        ip::project_filter(&t, MENU_PROJECT, &m.backlog.project.clone(), &counts, cx, |m, v, cx| {
            m.backlog.project = v;
            resync(m, cx);
        })
    });
    let lens_on = Lens::ALL.iter().position(|l| *l == m.backlog.lens).unwrap_or(0);
    let labels: Vec<&str> = Lens::ALL.iter().map(|l| l.label()).collect();
    let lenses = kit::seg(&t, "plan-lens", &labels, lens_on, |i, item| {
        item.tooltip(kit::tip(Lens::ALL[i].help())).on_click(cx.listener(move |m, _, _, cx| {
            if m.backlog.lens != Lens::ALL[i] {
                m.backlog.lens = Lens::ALL[i];
                m.backlog.folded.clear();
                cx.notify();
            }
        }))
    });
    let header = div().flex().flex_wrap().items_center().gap(px(16.)).child(title).child(div().flex_1()).children(project_btn).child(lenses);

    // ---- kinds or the selection bar, and the filter
    let left: AnyElement = if sel.is_empty() {
        let counts = data.get("kind_counts").cloned().unwrap_or_default();
        let n = |k: &str| fmt::i(&counts, k);
        let all: i64 = KINDS.iter().map(|k| n(k)).sum();
        let mut labels = vec![format!("All {all}")];
        labels.extend(KINDS.iter().zip(KIND_LABELS).map(|(k, l)| format!("{l} {}", n(k))));
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let on = if m.backlog.kinds == [true; 4] { 0 } else { m.backlog.kinds.iter().position(|x| *x).map(|i| i + 1).unwrap_or(0) };
        kit::seg(&t, "plan-kind", &refs, on, |i, item| {
            item.on_click(cx.listener(move |m, _, _, cx| {
                m.backlog.kinds = if i == 0 { [true; 4] } else { std::array::from_fn(|k| k == i - 1) };
                m.backlog.cursor = None;
                cx.notify();
            }))
        })
        .into_any_element()
    } else {
        let move_open = m.menu_open(MENU_MOVE).is_some();
        let prio_open = m.menu_open(MENU_PRIO).is_some();
        let sep = || div().w(px(1.)).h(px(18.)).bg(t.goal_line);
        let mut move_wrap = div().relative().child(bar_btn(&t, "plan-move-btn", "Move to goal ▾", "M").child(kit::anchor(MENU_MOVE)).on_click(cx.listener(|m, _, _, cx| m.toggle_menu(MENU_MOVE, point(px(0.), px(0.)), cx))));
        if move_open {
            move_wrap = move_wrap.child(kit::menu_under(MENU_MOVE, kit::Align::Left, point(px(0.), px(0.)), move_menu(m, &t, &pool, &sel, cx)));
        }
        let mut prio_wrap = div().relative().child(bar_btn(&t, "plan-prio-btn", "Priority ▾", "P").child(kit::anchor(MENU_PRIO)).on_click(cx.listener(|m, _, _, cx| m.toggle_menu(MENU_PRIO, point(px(0.), px(0.)), cx))));
        if prio_open {
            prio_wrap = prio_wrap.child(kit::menu_under(MENU_PRIO, kit::Align::Left, point(px(0.), px(0.)), prio_menu(&t, &pool, &sel, cx)));
        }
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.))
            .pl(px(12.))
            .pr(px(4.))
            .py(px(3.))
            .rounded(px(10.))
            .border_1()
            .border_color(t.goal_line)
            .bg(t.goal_tint)
            .child(div().mr(px(6.)).text_size(px(13.)).font_weight(FontWeight::BOLD).text_color(t.goal).child(format!("{} picked", sel.len())))
            .child(bar_btn(&t, "plan-clear", "Clear", "Esc").text_color(t.muted).on_click(cx.listener(|m, _, _, cx| clear(m, cx))))
            .child(sep())
            .child(move_wrap)
            .child(prio_wrap)
            .child(bar_btn(&t, "plan-defer", "Defer", "D").on_click(cx.listener(|m, _, _, cx| bulk(m, "defer", json!({}), cx))))
            .child(bar_btn(&t, "plan-drop", "Won’t do", "X").on_click(cx.listener(|m, _, _, cx| bulk(m, "drop", json!({}), cx))))
            .into_any_element()
    };
    let filter = m.backlog.filter.as_ref().map(|i| {
        let focused = i.focus.is_focused(window);
        let fh = i.focus.clone();
        let field = div()
            .id("plan-filter")
            .flex()
            .flex_1()
            .items_center()
            .gap(px(8.))
            .h(px(32.))
            .px(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(if focused { t.focus_ring } else { t.control_line })
            .bg(t.card)
            .text_size(px(13.))
            .overflow_hidden()
            .cursor_text()
            .on_click(move |_, window, cx| window.focus(&fh, cx))
            .child(kit::glyph(kit::Glyph::Search, 13., t.muted))
            .child(div().flex_1().min_w_0().child(i.field.clone()))
            .child(div().flex_none().text_size(px(12.)).text_color(t.faint).child("⌘F"));
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .w(px(260.))
            .child(field)
            .children((!m.backlog.q.is_empty()).then(|| {
                kit::btn_small(&t, "plan-filter-clear", "Clear").on_click(cx.listener(|m, _, _, cx| {
                    if let Some(i) = &m.backlog.filter {
                        i.clear(cx);
                    }
                }))
            }))
    });
    let toolbar = div().flex().flex_wrap().items_center().justify_between().gap(px(10.)).min_h(px(36.)).child(left).children(filter);

    // ---- the new goal: only once something is picked or a goal was just made
    let aside = (d.count > 0 || m.backlog.just_made.is_some()).then(|| draft_el(m, &t, &d, cx));
    let menus = ip::render_project_filter(m, &t, MENU_PROJECT, window, cx);

    let focus = m.backlog.focus.clone().unwrap();
    let state = m.backlog.list.clone().unwrap();
    let sticky = sticky_head(m, &t, cx);
    let col = div()
        .relative()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .h_full()
        .child(gpui_kit::list(state, cx.processor(line_item)).size_full())
        .children(sticky);
    let body = if narrow {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(px(14.))
            .children(aside.map(|a| div().id("plan-draft").flex_none().max_h(px(320.)).overflow_y_scroll().child(a)))
            .child(col)
    } else {
        div().flex().flex_1().min_h_0().gap(px(20.)).child(col).children(aside.map(|a| div().id("plan-draft").flex_none().w(px(380.)).h_full().overflow_y_scroll().pb(px(40.)).child(a)))
    };
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
        .child(div().flex().flex_col().gap(px(16.)).flex_1().min_h_0().w_full().max_w(px(1320.)).px(px(28.)).pt(px(22.)).child(header).child(toolbar).child(body))
        .children(menus)
        .into_any_element()
}

/// Group the held issues by the board's groups and lay out the list's lines, keeping the place
/// scrolled to unless the lens, kinds, project or filter changed.
fn sync_view(m: &mut MainWindow) {
    let st = &mut m.backlog;
    let current = st.data_scope.as_ref() == st.scope.as_ref();
    // A new scope's first page hasn't landed: keep showing the last list until it does.
    if !current && st.view.is_some() && st.err.is_none() {
        return;
    }
    let data_v = st.data.as_ref().map(|d| Rc::as_ptr(d) as u64).unwrap_or(0);
    let mut folded: Vec<String> = st.folded.iter().cloned().collect();
    folded.sort();
    let key = (st.held.version, data_v, st.picked.clone(), folded, st.err.is_some(), m.down.is_some());
    let more = st.held.next.is_some();
    let old = st.view.take();
    let same_scope = old.as_ref().is_some_and(|v| v.scope == st.data_scope);
    let groups = match &old {
        Some(v) if v.key.0 == key.0 && v.key.1 == key.1 && v.key.2 == key.2 => v.groups.clone(),
        _ => Rc::new(st.data.as_ref().map(|d| groups(d, &st.held.rows, st.lens, &st.picked)).unwrap_or_default()),
    };
    let data = st.data.as_ref();
    let mut lines = vec![];
    if data.is_none() {
        lines.push(Line::State);
    } else {
        if st.err.is_some() || m.down.is_some() {
            lines.push(Line::Stale);
        }
        if groups.is_empty() {
            lines.push(Line::Empty);
        }
        if data.is_some_and(|d| fmt::b(d, "grouping")) {
            lines.push(Line::Grouping);
        }
        for (gi, g) in groups.iter().enumerate() {
            lines.push(Line::Group(gi));
            if !st.folded.contains(&g.key) {
                lines.extend((0..g.rows.len()).map(|ri| Line::Row(gi, ri)));
            }
        }
        lines.push(Line::End);
    }
    let changed = old.as_ref().is_none_or(|v| v.lines != lines || v.more != more);
    let count = lines.len();
    match &st.list {
        None => st.list = Some(ListState::new(count, ListAlignment::Top, px(600.))),
        Some(l) if changed || !same_scope => {
            let top = l.logical_scroll_top();
            l.reset(count);
            // An older page or a refresh keeps the place scrolled to; another lens or filter starts at the top.
            if same_scope {
                l.scroll_to(ListOffset { item_ix: top.item_ix.min(count.saturating_sub(1)), offset_in_item: top.offset_in_item });
            }
        }
        _ => {}
    }
    st.view = Some(View { key, scope: st.data_scope.clone(), groups, lines, more });
}

/// A group's head: its fold chevron, Select all over every issue in it, name, true total and what
/// it's about.
fn group_head(m: &MainWindow, t: &Theme, gi: usize, id: &str, cx: &mut Context<MainWindow>) -> Div {
    let Some(g) = m.backlog.view.as_ref().and_then(|v| v.groups.get(gi)).cloned() else { return div() };
    let folded = m.backlog.folded.contains(&g.key);
    let all = g.check == Check::All;
    let refs = g.refs.clone();
    let key = g.key.clone();
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(10.))
        .py(px(6.))
        .bg(t.bg)
        .child(
            check_box(t, SharedString::from(format!("{id}-check-{gi}")), g.check)
                .tooltip(kit::tip(if all { "Unpick all".to_string() } else { format!("Pick all {}", g.total) }))
                .on_click(cx.listener(move |m, _, _, cx| toggle(m, &refs, !all, cx))),
        )
        .child(
            focus::stop(t, div().id(SharedString::from(format!("{id}-fold-{gi}"))))
                .flex()
                .items_center()
                .justify_center()
                .size(px(20.))
                .rounded(px(5.))
                .cursor_pointer()
                .tooltip(kit::tip(if folded { "Show this group" } else { "Hide this group" }))
                .child(kit::icon(if folded { kit::Icon::ChevRight } else { kit::Icon::Chev }, 14., t.muted))
                .on_click(cx.listener(move |m, _, _, cx| {
                    if !m.backlog.folded.remove(&key) {
                        m.backlog.folded.insert(key.clone());
                    }
                    cx.notify();
                })),
        )
        .child(div().text_size(px(15.)).font_weight(FontWeight::BOLD).child(g.label.clone()))
        .child(div().text_size(px(12.)).text_color(t.faint).child(g.total.to_string()))
        .child(div().min_w_0().truncate().text_size(px(12.5)).text_color(t.muted).child(g.about.clone()))
}

/// The group the list's top is inside, pinned over the list while its issues scroll under it.
fn sticky_head(m: &MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Option<Div> {
    let v = m.backlog.view.as_ref()?;
    let top = m.backlog.list.as_ref()?.logical_scroll_top();
    let gi = match v.lines.get(top.item_ix)? {
        Line::Row(gi, _) => *gi,
        Line::Group(gi) if top.offset_in_item > px(0.) => *gi,
        _ => return None,
    };
    Some(div().absolute().top_0().left_0().right_0().border_b_1().border_color(t.divider).child(group_head(m, t, gi, "plan-sticky", cx)))
}

/// One line of the grouped list: a page state, a group's head or one of its issues.
fn line_item(m: &mut MainWindow, ix: usize, _: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let Some(v) = m.backlog.view.as_ref() else { return div().into_any_element() };
    let (groups, more) = (v.groups.clone(), v.more);
    let line = v.lines.get(ix).copied();
    let untriaged = m.backlog.data.as_ref().map(|d| fmt::i(d, "untriaged")).unwrap_or(0);
    let filtered = !m.backlog.q.is_empty() || m.backlog.kinds != [true; 4];
    match line {
        Some(Line::State) => {
            if m.down.is_some() {
                state_box(&t, "The board isn’t running, so the backlog can’t load.".into(), true, cx).into_any_element()
            } else if m.backlog.err.is_some() {
                state_box(&t, "Can’t load the backlog.".into(), true, cx).into_any_element()
            } else {
                state_box(&t, "Loading…".into(), false, cx).into_any_element()
            }
        }
        Some(Line::Stale) => {
            let text = if m.down.is_some() { "The board isn’t running. This is the backlog as it last loaded." } else { "Couldn’t refresh the backlog. This is how it last loaded." };
            div()
                .pt(px(6.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(9.))
                        .bg(t.down_soft)
                        .text_size(px(12.5))
                        .text_color(t.down)
                        .child(div().flex_1().child(text))
                        .child(kit::btn_small(&t, "plan-stale-retry", "Try again").on_click(cx.listener(|m, _, _, cx| resync(m, cx)))),
                )
                .into_any_element()
        }
        Some(Line::Empty) => {
            let text = if untriaged == 0 && !filtered {
                "Every open issue is in a goal, deferred or closed."
            } else if more {
                "Loading…"
            } else if !m.backlog.q.is_empty() {
                "No issues match the filter."
            } else {
                "No issues of this kind."
            };
            state_box(&t, text.into(), false, cx).into_any_element()
        }
        Some(Line::Grouping) => div().pt(px(14.)).text_size(px(12.5)).text_color(t.goal).child("Claude is grouping new issues…").into_any_element(),
        Some(Line::Group(gi)) => div().pt(px(14.)).child(group_head(m, &t, gi, "plan-group", cx)).into_any_element(),
        Some(Line::Row(gi, ri)) => {
            let row = groups[gi].rows[ri].clone();
            let first = !matches!(v.lines.get(ix.wrapping_sub(1)), Some(Line::Row(..)));
            div().when(!first, |d| d.border_t_1().border_color(t.divider)).child(row_el(m, &t, &row, cx)).into_any_element()
        }
        Some(Line::End) | None => div().pb(px(40.)).pt(px(14.)).when(more, |d| d.text_size(px(12.5)).text_color(t.muted).child("Loading…")).into_any_element(),
    }
}

fn row_el(m: &MainWindow, t: &Theme, v: &RowView, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let r = v.r.clone();
    let on = v.picked;
    let open = open_issue(m).as_deref() == Some(&v.r);
    let marked = open || m.backlog.cursor.as_deref() == Some(&v.r);
    let narrow = m.backlog.narrow;
    let lens = m.backlog.lens;
    let all_projects = m.backlog.project == "all";
    let hover = t.hover_overlay;
    let mut place: Vec<String> = vec![];
    if all_projects && !v.project.is_empty() {
        place.push(v.project.clone());
    }
    if lens != Lens::Area && !v.area.is_empty() {
        place.push(v.area.clone());
    }
    let check = {
        let r = r.clone();
        check_box(t, SharedString::from(format!("plan-check-{}", v.r)), if on { Check::All } else { Check::None })
            .tooltip(kit::tip(if on { "Unpick" } else { "Pick for the new goal" }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |m, _, _, cx| {
                cx.stop_propagation();
                toggle(m, std::slice::from_ref(&r), !on, cx);
            }))
    };
    let lead = div()
        .flex()
        .items_center()
        .gap(px(10.))
        .min_w_0()
        .flex_1()
        .child(check)
        .child(div().flex_none().w(px(68.)).text_size(px(12.)).text_color(t.muted).child(ip::kind_label(&v.kind)))
        .child(kit::mono(t, v.r.clone()).flex_none().text_color(t.faint))
        .child(div().min_w_0().truncate().text_size(px(13.5)).child(v.title.clone()))
        .children(v.is_new.then(|| kit::chip(t, "New")))
        .child(div().flex_1());
    let meta = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(12.))
        .children((!place.is_empty()).then(|| div().flex_none().max_w(px(220.)).truncate().text_size(px(12.)).text_color(t.muted).child(place.join(" · "))))
        .child(impact_bars(t, SharedString::from(format!("plan-impact-{}", v.r)), &v.impact))
        .child(prio_pill(t, &v.priority));
    focus::stop(t, div().id(SharedString::from(format!("plan-row-{}", v.r))))
        .flex()
        .when(narrow, |d| d.flex_col().items_start().gap(px(6.)))
        .when(!narrow, |d| d.items_center().gap(px(12.)))
        .px(px(10.))
        .py(px(9.))
        .border_l_2()
        .border_color(if marked { t.accent } else { transparent_black() })
        .when(marked, |d| d.bg(t.accent_soft))
        .when(on && !marked, |d| d.bg(t.goal_tint))
        .when(m.down.is_some(), |d| d.opacity(0.6))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .tooltip(kit::tip("Open the issue: what was said and its history"))
        .child(lead.w_full())
        .child(if narrow { meta.pl(px(28.)) } else { meta })
        .on_click(cx.listener(move |m, _, _, cx| {
            m.backlog.cursor = Some(r.clone());
            m.open_issue(r.clone(), cx);
        }))
}

fn move_menu(m: &MainWindow, t: &Theme, pool: &[Value], sel: &[String], cx: &mut Context<MainWindow>) -> impl IntoElement {
    let projects: Vec<String> = sel.iter().filter_map(|r| pool.iter().find(|b| fmt::ref_of(b, "B") == *r)).map(|b| s(b, "project").to_string()).collect();
    let one = projects.first().filter(|p| projects.iter().all(|x| x == *p)).cloned();
    let goals = move_goals(&m.data.goals, one.as_deref());
    let mut box_ = kit::menu_box(t, 320.).id("plan-move-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    if one.is_none() {
        box_ = box_.child(div().p(px(8.)).text_size(px(12.5)).text_color(t.muted).child("The picked issues are in more than one project. A goal holds one project’s work."));
    } else if goals.is_empty() {
        box_ = box_.child(div().p(px(8.)).text_size(px(12.5)).text_color(t.muted).child("No open goals in this project."));
    }
    for (n, g) in goals.into_iter().enumerate().filter(|_| one.is_some()) {
        let id = fmt::i(g, "id");
        let label = format!("{} {}", fmt::ref_of(g, "G"), s(g, "name"));
        box_ = box_.child(
            kit::menu_item(t, SharedString::from(format!("plan-move-{n}")), "", false)
                .child(div().flex_1().min_w_0().truncate().child(label))
                .child(div().flex_none().text_size(px(12.)).text_color(t.muted).child(goal_doing(g)))
                .on_click(cx.listener(move |m, _, _, cx| bulk(m, "goal", json!({"goal_id": id}), cx))),
        );
    }
    box_
}

fn prio_menu(t: &Theme, list: &[Value], sel: &[String], cx: &mut Context<MainWindow>) -> impl IntoElement {
    let mut box_ = kit::menu_box(t, 230.).id("plan-prio-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for (n, (p, what)) in [("p1", "Do first"), ("p2", "Normal"), ("p3", "When there’s time")].into_iter().enumerate() {
        let all = !sel.is_empty() && sel.iter().all(|r| list.iter().find(|b| fmt::ref_of(b, "B") == *r).is_some_and(|b| s(b, "priority") == p));
        box_ = box_.child(
            kit::menu_item(t, SharedString::from(format!("plan-prio-{n}")), "", all)
                .child(div().flex_none().w(px(28.)).font_weight(FontWeight::SEMIBOLD).child(p.to_uppercase()))
                .child(div().flex_1().child(what))
                .child(kbd(t, &(n + 1).to_string()))
                .on_click(cx.listener(move |m, _, _, cx| bulk(m, "priority", json!({"priority": p}), cx))),
        );
    }
    box_
}

fn draft_el(m: &mut MainWindow, t: &Theme, d: &DraftView, cx: &mut Context<MainWindow>) -> AnyElement {
    let count = if d.count == 0 {
        String::new()
    } else if d.waves.is_empty() {
        format!("{} · not planned", fmt::plural(d.count as i64, "issue", "issues"))
    } else {
        format!("{} · {}", fmt::plural(d.count as i64, "issue", "issues"), fmt::plural(d.waves.len() as i64, "wave", "waves"))
    };
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
    if d.mixed {
        col = col.child(div().text_size(px(12.5)).text_color(t.down).child("A goal holds one project’s work. Pick issues from one project."));
    }
    if d.planning {
        col = col.child(div().p(px(12.)).rounded(px(10.)).bg(t.goal_tint).text_size(px(13.)).text_color(t.goal).child("Claude is planning the waves…"));
    }
    if let Some(note) = &d.note {
        col = col.child(div().text_size(px(12.)).text_color(t.muted).child(note.clone()));
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
        let b = if d.never_planned { kit::filled(t, "plan-plan", label, t.goal, t.on_status) } else { kit::btn(t, "plan-plan", label).text_color(t.goal).border_color(t.goal_line) };
        let b = b.mt(px(4.)).h(px(34.));
        box_ = box_.child(if d.mixed { kit::disabled(b).tooltip(kit::tip("Pick issues from one project to plan a goal")) } else { b.on_click(cx.listener(|m, _, _, cx| plan_waves(m, cx))) });
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
    let mut btn = kit::filled(t, "plan-create", label, t.goal, t.on_status).h(px(38.));
    if d.can_create() && !m.backlog.busy {
        let dd = d.clone();
        btn = btn.on_click(cx.listener(move |m, _, _, cx| create(m, &dd, cx)));
    } else {
        btn = kit::disabled(btn);
    }
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
        .child(kit::mono(t, it.r.clone()).flex_none().w(px(44.)).text_color(t.faint))
        .child(div().flex_1().min_w_0().truncate().text_color(t.text_2).child(it.title.clone()))
        .children((!it.after.is_empty()).then(|| div().flex_none().text_size(px(11.5)).text_color(t.muted).child(format!("after {}", it.after.join(", ")))))
        .children((!it.priority.is_empty()).then(|| prio_pill(t, &it.priority)))
        .child(
            focus::stop(t, div().id(SharedString::from(format!("plan-rm-{prefix}-{}", it.r))))
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
    use super::{Check, Item, Lens, Line, MENU_PROJECT, PAGE, bulk, bulk_note, create, draft, goal_doing, goal_waves, groups, move_goals, moved_note, names, on_key, plan_waves, pool, step, sync, sync_view, toggle};
    use crate::app::{Page, Panel};
    use crate::backend::Backend;
    use crate::fmt;
    use crate::parity;
    use crate::ui::issue_panel as ip;
    use serde_json::{Value, json};

    fn b(r: &str, kind: &str, area: &str, impact: &str, prio: &str) -> Value {
        json!({"ref": r, "kind": kind, "title": format!("{r} title"), "area": area, "impact": impact, "priority": prio, "project": "webapp"})
    }

    fn list() -> Vec<Value> {
        vec![b("B1", "bug", "Runner", "high", "p1"), b("B2", "gap", "Runner", "med", "p2"), b("B3", "clean", "App", "low", "p3")]
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[::core::prelude::v1::test]
    fn groups_take_the_boards_order_totals_and_issues() {
        // B9 is in the group but on a page not loaded yet.
        let data = json!({"groups": [
            {"key": "Runner", "about": "Runner about", "total": 3, "refs": ["B1", "B2", "B9"]},
            {"key": "Tidy", "about": "", "total": 1, "refs": ["B3"]}]});
        let g = groups(&data, &list(), Lens::Similar, &strs(&["B1"]));
        assert_eq!(g.iter().map(|g| (g.label.as_str(), g.total, g.rows.len(), g.check)).collect::<Vec<_>>(), [("Runner", 3, 2, Check::Some), ("Tidy", 1, 1, Check::None)]);
        assert_eq!(g[0].about, "Runner about");
        let g = groups(&data, &list(), Lens::Similar, &strs(&["B1", "B2"]));
        assert_eq!(g[0].check, Check::Some, "B9 isn't picked, loaded or not");
        let g = groups(&data, &list(), Lens::Similar, &strs(&["B1", "B2", "B9"]));
        assert_eq!(g[0].check, Check::All);

        let data = json!({"groups": [{"key": "high", "total": 1, "refs": ["B1"]}, {"key": "low", "total": 1, "refs": ["B3"]}]});
        let g = groups(&data, &list(), Lens::Impact, &[]);
        assert_eq!(g.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(), ["High impact", "Low impact"]);
        let data = json!({"groups": [{"key": "p1", "total": 1, "refs": ["B1"]}]});
        assert_eq!(groups(&data, &list(), Lens::Priority, &[])[0].label, "P1");
    }

    #[::core::prelude::v1::test]
    fn toasts_name_the_issues() {
        assert_eq!(names(&strs(&["B9"])), "B9");
        assert_eq!(names(&strs(&["B9", "B20"])), "B9 and B20");
        assert_eq!(names(&strs(&["B9", "B20", "B39"])), "B9, B20 and B39");
        assert_eq!(names(&strs(&["B9", "B20", "B39", "B40", "B41"])), "B9, B20, B39 and 2 more");
        assert_eq!(bulk_note("defer", &strs(&["B9"]), "", &json!({})), "Deferred B9");
        assert_eq!(bulk_note("drop", &strs(&["B9", "B20"]), "", &json!({})), "Closed B9 and B20 as won’t do");
        assert_eq!(bulk_note("priority", &strs(&["B9"]), "P1", &json!({})), "Set B9 to P1");
    }

    #[::core::prelude::v1::test]
    fn the_move_menu_offers_open_goals_in_the_one_project() {
        let goals = vec![
            json!({"id": 1, "name": "Beta", "project": "webapp", "tasks": [{"status": "running"}], "active": 1}),
            json!({"id": 2, "name": "Alpha", "project": "webapp", "tasks": [{"status": "done"}], "finished_at": "2026-11-10T10:00:00Z"}),
            json!({"id": 3, "name": "Gamma", "project": "webapp", "archived": true, "tasks": []}),
            json!({"id": 4, "name": "Delta", "project": "api", "tasks": []}),
            json!({"id": 5, "name": "Echo", "project": "webapp", "paused": true, "tasks": [{"status": "planned"}]}),
        ];
        let got: Vec<&str> = move_goals(&goals, Some("webapp")).iter().map(|g| fmt::s(g, "name")).collect();
        assert_eq!(got, ["Beta", "Echo"], "finished, archived and other projects' goals are left out");
        assert_eq!(goal_doing(&goals[0]), "Running");
        assert_eq!(goal_doing(&goals[4]), "Paused");
    }

    #[::core::prelude::v1::test]
    fn the_draft_shows_the_plan_for_what_is_still_checked() {
        let picked = strs(&["B1", "B2"]);
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

        let plan2 = json!({"id": 2, "state": "ready", "ids": ["B1"], "waves": [{"why": "", "items": [{"ref": "B1"}]}]});
        let d = draft(&list(), Some(&plan2), &picked);
        assert!(d.stale() && !d.can_create());
        assert_eq!(d.unplanned.iter().map(|i| i.r.as_str()).collect::<Vec<_>>(), ["B2"]);

        let planning = json!({"id": 3, "state": "planning", "ids": ["B1", "B2"]});
        let d = draft(&list(), Some(&planning), &picked);
        assert!(d.planning && d.unplanned.is_empty() && !d.can_create());
    }

    #[::core::prelude::v1::test]
    fn a_goal_holds_one_project() {
        let mut l = list();
        l[2]["project"] = json!("api");
        let plan = json!({"id": 1, "state": "ready", "ids": ["B1", "B3"], "waves": [{"why": "", "items": [{"ref": "B1"}, {"ref": "B3"}]}]});
        let d = draft(&l, Some(&plan), &strs(&["B1", "B3"]));
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
            assert!(m.backlog.held.rows.iter().any(|x| fmt::ref_of(x, "B") == a), "untriaged issues are listed");
            toggle(m, &[a.clone(), b.clone()], true, cx);
            plan_waves(m, cx);
        })
        .unwrap();
        parity::settle(cx);
        assert_eq!(rec.last("backlog/plan"), Some(json!({"ids": [a, b]})));
        w.update(cx, |m, _, cx| {
            let d = draft(&pool(m), m.backlog.plan.as_ref(), &m.backlog.picked);
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
            m.backlog.synced = None;
            sync(m, cx);
        })
        .unwrap();
        parity::settle(cx);
        w.update(cx, |m, _, cx| {
            assert!(!m.backlog.held.rows.iter().any(|x| fmt::ref_of(x, "B") == a), "in a goal now: triaged");
            toggle(m, std::slice::from_ref(&later), true, cx);
            bulk(m, "defer", json!({}), cx);
        })
        .unwrap();
        parity::settle(cx);
        assert_eq!(rec.last("backlog/bulk"), Some(json!({"ids": [later], "action": "defer"})));
        let toast = w.update(cx, |m, _, _| m.toasts.last().map(|t| t.text.clone())).unwrap();
        assert_eq!(toast.as_deref(), Some(format!("Deferred {later}").as_str()));
    }

    #[gpui_kit::test]
    fn the_filter_and_kind_narrow_the_list_and_the_counts_stay(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = parity::window(cx);
        let mk = |title: &str, kind: &str| fmt::ref_of(&rec.board().post("backlog", json!({"title": title, "kind": kind, "project": "webapp"})).unwrap(), "B");
        let a = mk("Restarts ignore the cap", "bug");
        let g = mk("No test for spool replay", "gap");
        w.update(cx, |m, _, cx| {
            m.go(Page::Backlog, cx);
            sync(m, cx);
        })
        .unwrap();
        parity::settle(cx);
        w.update(cx, |m, _, cx| {
            m.backlog.kinds = [true, false, false, false];
            sync(m, cx);
        })
        .unwrap();
        parity::settle(cx);
        w.update(cx, |m, _, _| {
            let rows = &m.backlog.held.rows;
            assert!(rows.iter().all(|x| fmt::s(x, "kind") == "bug"), "only bugs");
            assert!(rows.iter().any(|x| fmt::ref_of(x, "B") == a) && !rows.iter().any(|x| fmt::ref_of(x, "B") == g));
            let counts = &m.backlog.data.as_ref().unwrap()["kind_counts"];
            assert!(fmt::i(counts, "gap") >= 1, "the kind counts ignore the kind picked");
        })
        .unwrap();
        w.update(cx, |m, _, cx| {
            m.backlog.kinds = [true; 4];
            m.backlog.q = "spool".into();
            sync(m, cx);
        })
        .unwrap();
        parity::settle(cx);
        w.update(cx, |m, _, _| {
            let refs: Vec<String> = m.backlog.held.rows.iter().map(|x| fmt::ref_of(x, "B")).collect();
            assert_eq!(refs, [g.clone()], "the filter's words narrow the list");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn arrows_move_through_the_issues_and_the_open_one(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = parity::window(cx);
        for t in ["One", "Two", "Three"] {
            rec.board().post("backlog", json!({"title": t, "kind": "bug", "project": "webapp"})).unwrap();
        }
        w.update(cx, |m, _, cx| {
            m.go(Page::Backlog, cx);
            sync(m, cx);
        })
        .unwrap();
        parity::settle(cx);
        let order = w
            .update(cx, |m, _, cx| {
                sync_view(m);
                let v = m.backlog.view.as_ref().unwrap();
                let rows: Vec<String> = v.lines.iter().filter_map(|l| if let Line::Row(g, r) = l { Some(v.groups[*g].rows[*r].r.clone()) } else { None }).collect();
                step(m, true, cx);
                assert_eq!(m.backlog.cursor.as_ref(), rows.first(), "↓ starts at the first issue");
                step(m, true, cx);
                assert_eq!(m.backlog.cursor.as_ref(), rows.get(1));
                step(m, false, cx);
                assert_eq!(m.backlog.cursor.as_ref(), rows.first());
                step(m, false, cx);
                assert_eq!(m.backlog.cursor.as_ref(), rows.first(), "↑ stops at the top");
                rows
            })
            .unwrap();
        w.update(cx, |m, _, cx| {
            m.open_issue(order[0].clone(), cx);
            step(m, true, cx);
            assert!(matches!(&m.panel, Some(Panel::Issue { r }) if *r == order[1]), "with the panel open, ↓ opens the next issue");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn a_long_backlog_fetches_older_pages_as_it_scrolls(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        let (w, rec) = parity::window(cx);
        for k in 0..300 {
            rec.board().post("backlog", json!({"title": format!("Paged {k}"), "kind": "bug", "project": "webapp"})).unwrap();
        }
        let draw = |cx: &mut gpui_kit::TestAppContext| {
            let _ = cx.update_window(w.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
            parity::settle(cx);
        };
        w.update(cx, |m, _, cx| m.go(Page::Backlog, cx)).unwrap();
        parity::settle(cx);
        draw(cx);
        let held = |cx: &mut gpui_kit::TestAppContext| w.update(cx, |m, _, _| (m.backlog.held.rows.len(), m.backlog.held.next.is_some())).unwrap();
        assert_eq!(held(cx), (PAGE, true), "only the first page until the list nears its end");
        let (untriaged, total) = w
            .update(cx, |m, _, _| {
                let d = m.backlog.data.as_ref().unwrap();
                (fmt::i(d, "untriaged") as usize, fmt::arr(d, "groups").iter().map(|g| fmt::i(g, "total") as usize).sum::<usize>())
            })
            .unwrap();
        assert!(untriaged >= 300, "the header counts every issue: {untriaged}");
        assert_eq!(total, untriaged, "the groups' totals cover the issues not loaded yet");

        for _ in 0..5 {
            w.update(cx, |m, _, _| {
                let n = m.backlog.view.as_ref().unwrap().lines.len();
                m.backlog.list.as_ref().unwrap().scroll_to(gpui_kit::ListOffset { item_ix: n, offset_in_item: gpui_kit::px(0.) });
            })
            .unwrap();
            draw(cx);
        }
        assert_eq!(held(cx), (untriaged, false), "scrolling to the end pages back through the rest");
        let rows = w.update(cx, |m, _, _| m.backlog.view.as_ref().unwrap().lines.iter().filter(|l| matches!(l, Line::Row(..))).count()).unwrap();
        assert_eq!(rows, untriaged, "every issue has a line");
    }

    #[gpui_kit::test]
    fn typing_in_the_project_filter_is_not_a_shortcut(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = parity::window(cx);
        let a = fmt::ref_of(&rec.board().post("backlog", json!({"title": "Restarts ignore the cap", "kind": "bug", "project": "webapp"})).unwrap(), "B");
        w.update(cx, |m, _, cx| {
            m.go(Page::Backlog, cx);
            sync(m, cx);
        })
        .unwrap();
        parity::settle(cx);
        let d = gpui_kit::KeyDownEvent { keystroke: gpui_kit::Keystroke::parse("d").unwrap(), is_held: false, prefer_character_input: false };
        w.update(cx, |m, window, cx| {
            toggle(m, std::slice::from_ref(&a), true, cx);
            ip::open_project_filter(m, MENU_PROJECT, "all", &[("webapp".into(), 1)], gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)), window, cx, |_, _, _| {});
            on_key(m, &d, window, cx);
            m.escape(window, cx);
            assert!(m.menu.is_none(), "esc closes the filter");
            assert_eq!(m.backlog.picked, [a.clone()], "and keeps the checks");
        })
        .unwrap();
        parity::settle(cx);
        assert_eq!(rec.last("backlog/bulk"), None, "the d went to the search, not to defer");
    }

    #[test]
    fn moving_issues_into_a_running_goal_says_they_are_queued() {
        let tasks = |st: &[&str]| json!({"tasks": st.iter().map(|s| json!({"status": s})).collect::<Vec<_>>()});
        assert_eq!(moved_note("B1 and B2", &tasks(&["planned", "planned"])), "Moved B1 and B2 into the goal as planned tasks");
        assert_eq!(moved_note("B1 and B2", &tasks(&["queued", "queued"])), "Moved B1 and B2 into the goal. It’s running, so they’re queued");
        assert_eq!(moved_note("B1, B2 and B3", &tasks(&["queued", "planned", "queued"])), "Moved B1, B2 and B3 into the goal: 2 queued, 1 planned");
    }
}
