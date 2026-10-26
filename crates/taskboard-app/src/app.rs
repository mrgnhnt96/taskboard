//! The main window: what the board last answered (`Data`), where the reader is (`Page`, the side
//! panel, a modal), the poll (every 3 s, slower while the window is away), and the chrome around the pages: the header (the pages as tabs),
//! the alert banner, toasts and the status bar.
//!
//! Every page lives in its own module under `ui/` with a `State` struct held here and a
//! `render(m, window, cx)` function. They read `m.data` and act through [`MainWindow::post`].
use crate::backend::{Backend, CallError};
use crate::fingerprint::{self, Fingerprint};
use crate::fmt::{self, arr, b, s};
use crate::theme::Theme;
use crate::ui::{self, kit};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_secs(3);
/// A poll that brings nothing new still redraws after this long without one, so ages ("5m ago",
/// "running 2h 5m") keep up.
const AGES: Duration = Duration::from_secs(15);
/// The poll while the window shows but another app is in front.
const POLL_AWAY: Duration = Duration::from_secs(15);
/// The poll while none of the window is on screen: minimized, covered, on another Space, the app hidden.
const POLL_HIDDEN: Duration = Duration::from_secs(30);
/// How long a toast stays (the web's `toast`): 3 s, an error 6 s.
const TOAST_OK: Duration = Duration::from_secs(3);
const TOAST_ERR: Duration = Duration::from_secs(6);
/// How many goals just left stay on hand to show again at once.
const RECENT_GOALS: usize = 8;
/// The least a clicked button reads "Sending…", so a quick answer doesn't flash it. None in
/// tests, whose clock only moves when told (it would bring on polls).
#[cfg(not(test))]
const HOLD: Duration = Duration::from_millis(600);
#[cfg(test)]
const HOLD: Duration = Duration::ZERO;

type Settle = Box<dyn FnOnce(&mut MainWindow, &mut Context<MainWindow>)>;

#[derive(Clone, Debug, PartialEq)]
pub enum Page {
    Board,
    /// Every task, grouped and filtered.
    Tasks,
    /// Every goal, as cards: active, deprioritized or finished.
    Goals,
    /// A goal ref, `G3`.
    Goal(String),
    /// The backlog: triage it by making goals from untriaged issues.
    Backlog,
    Sessions,
    /// Every project and its goals (`ui::projects`).
    Projects,
    Days,
    /// The weekly time review (`ui::review`): which week is in `MainWindow::review`.
    Review,
    /// Each project's releases, for the projects with releases on (`ui::releases`).
    Releases,
    /// One project's releases, by project name (`ui::releases`).
    ReleasesOf(String),
    /// Each project's ideas (`ui::ideas`).
    Ideas,
    /// An idea ref, `I3` (`ui::idea`).
    Idea(String),
}

/// A place in the ⌘← / ⌘→ history: a page, and the task open over it.
#[derive(Clone, Debug, PartialEq)]
struct Nav {
    page: Page,
    task: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TaskTab {
    #[default]
    Overview,
    What,
    Context,
    Log,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Panel {
    Task { r: String, tab: TaskTab },
    Issue { r: String },
}

/// What the board last answered. `None` until the first answer. The board state, goals and home
/// answer sit behind `Rc` so pages hold them for a frame without copying them.
#[derive(Default)]
pub struct Data {
    pub state: Option<Rc<Value>>,
    pub goals: Rc<Vec<Value>>,
    /// `GET /tasks/:id` for the open task panel.
    pub task: Option<Value>,
    /// The open task's row as the page already had it, shown until `task` answers.
    pub task_card: Option<Value>,
    /// `GET /backlog/:id` for the open issue panel.
    pub issue: Option<Value>,
    /// `GET /goals/:id` for the goal page, shared with what the page builds from it.
    pub goal: Option<Rc<Value>>,
    /// `GET /sessions`, `GET /sessions/closed`, `GET /sessions/:id` for the Sessions page.
    pub sessions: Option<Value>,
    pub closed: Option<Value>,
    pub session: Option<Value>,
    /// `GET /days` for the Days page, shared with the view the page builds from it.
    pub days: Option<Rc<Value>>,
    /// `GET /home` and today's `GET /days` for the home page.
    pub home: Option<Rc<Value>>,
    pub today: Option<Rc<Value>>,
    /// The ⌘K palette's best tasks and backlog issues for what's typed (`GET /search?palette=1`).
    pub tasks: Option<Value>,
    pub backlog: Option<Value>,
    /// `GET /reviews` and the one shown, `GET /reviews/:id`, for the Review page.
    pub reviews: Option<Value>,
    pub review: Option<Value>,
    /// `GET /ideas/:id` for an idea's page (the Ideas page holds its own, `ui::ideas`).
    pub idea: Option<Value>,
    /// Why the last fetch of each failed (the web board's `taskErr`, `issueErr`, `goalErr`,
    /// `SS.listErr`, `SS.detailErr`): shown in place of "Loading…" while there's
    /// nothing to show, cleared by the next good answer.
    pub errs: Errs,
}

#[derive(Default, Clone, Debug)]
pub struct Errs {
    pub task: Option<String>,
    pub issue: Option<String>,
    pub goal: Option<String>,
    pub sessions: Option<String>,
    pub session: Option<String>,
    pub days: Option<String>,
    pub review: Option<String>,
    pub ideas: Option<String>,
    pub idea: Option<String>,
}

/// The board's filters (sent with `GET /state`).
#[derive(Clone, Debug)]
pub struct Filters {
    pub project: String,
    pub goal: String,
    pub done: String,
}

impl Default for Filters {
    fn default() -> Self {
        Filters { project: "all".into(), goal: "all".into(), done: "24h".into() }
    }
}

impl Filters {
    /// The saved filters (`loadFilter`): each field kept only when it's a valid value.
    pub fn load() -> Filters {
        let mut f = Filters::default();
        let v = crate::prefs::get("taskboard.filter").unwrap_or(Value::Null);
        if let Some(p) = v["project"].as_str() {
            f.project = p.to_string();
        }
        if let Some(g) = v["goal"].as_str().filter(|g| *g == "all" || is_goal_ref(g)) {
            f.goal = g.to_string();
        }
        if let Some(d) = v["done"].as_str().filter(|d| ["24h", "7d", "all"].contains(d)) {
            f.done = d.to_string();
        }
        f
    }

    /// `saveFilter`.
    pub fn save(&self) {
        crate::prefs::set("taskboard.filter", json!({"project": self.project, "goal": self.goal, "done": self.done}));
    }
}

/// `projectNames`: every project the board knows (`state.projects`, names or objects) plus the
/// project of each goal given, without duplicates, sorted with `localeCompare`. Each caller passes
/// its own `allGoals()` list.
pub fn project_names<'a>(state: &Value, goals: impl IntoIterator<Item = &'a Value>) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let mut add = |n: &str| {
        if !n.is_empty() && !v.iter().any(|x| x == n) {
            v.push(n.to_string());
        }
    };
    for p in arr(state, "projects") {
        add(p.as_str().unwrap_or_else(|| s(p, "name")));
    }
    for g in goals {
        add(s(g, "project"));
    }
    v.sort_by(|a, b| ui::modals::locale_cmp(a, b));
    v
}

/// `G3` (the web's `isGoalRef`: capital G and digits).
pub fn is_goal_ref(s: &str) -> bool {
    s.len() > 1 && s.starts_with('G') && s[1..].chars().all(|c| c.is_ascii_digit())
}

pub struct Toast {
    pub text: String,
    pub err: bool,
    pub at: Instant,
    /// The POST its Undo sends, if it has one.
    pub undo: Option<String>,
}

pub struct MainWindow {
    pub backend: Arc<dyn Backend>,
    pub data: Data,
    pub filters: Filters,
    pub page: Page,
    /// Places left behind, newest last; ⌘← walks back through them.
    back: Vec<Nav>,
    /// Places ⌘← stepped away from, newest last; ⌘→ returns to them and a fresh visit clears them.
    forward: Vec<Nav>,
    pub panel: Option<Panel>,
    pub modal: Option<ui::modals::Modal>,
    /// A popover menu, by key, and where it opens.
    pub menu: Option<(String, Point<Pixels>)>,
    pub toasts: Vec<Toast>,
    /// Why the last `GET /state` failed (the "isn't answering" banner).
    pub down: Option<String>,
    pub focus: FocusHandle,
    // Per-page state.
    pub board: ui::board::State,
    pub goal_page: ui::goal::State,
    pub backlog: ui::backlog::State,
    pub sessions: ui::sessions::State,
    pub task_panel: ui::task_panel::State,
    pub issue_panel: ui::issue_panel::State,
    pub hours: ui::hours::State,
    pub days: ui::days::State,
    pub review: ui::review::State,
    pub tasks: ui::tasks::State,
    pub goals: ui::goals_page::State,
    pub projects: ui::projects::State,
    pub releases: ui::releases::State,
    pub ideas: ui::ideas::State,
    pub idea: ui::idea::State,
    pub search: ui::search::State,
    /// The goal list answered at least once (before that the state's goals stand in).
    goals_loaded: bool,
    /// Bumped by every refresh so a slow, older answer never overwrites a newer one.
    generation: u64,
    /// The newest refresh that has answered: behind `generation` while one is still out.
    answered: u64,
    /// The generation when a change was last sent or answered: a refresh asked at or before it may
    /// have read the board before the change, so its answer is dropped whole.
    posted: u64,
    /// The refresh each part of the view (`state`, `home`, `goal_row/G3`, …) last swapped in from
    /// and the fingerprint of what it brought ([`Got::fingerprint`]): an older answer landing late
    /// doesn't put back what a newer one showed, and a poll's part answering the same skips the redraw.
    shown: std::collections::HashMap<String, (u64, u64)>,
    /// When the window last drew.
    drawn: Instant,
    /// What waits for the board's first answer after a change (see [`settle`](Self::settle)): the
    /// generation it was asked at, when the button was clicked, and what to do.
    settling: Vec<(u64, Instant, Settle)>,
    /// The task or issue whose own `GET` is out (its panel opened just now): a refresh leaves it out.
    panel_fetch: Option<String>,
    /// Likewise the goal whose page opened just now.
    goal_fetch: Option<String>,
    /// The goals opened last, newest first, so going back to one shows it at once.
    recent_goals: Vec<Rc<Value>>,
    /// The Claude Code hooks (status bar), and whether an install is running.
    pub hooks: crate::hooks::Hooks,
    pub hooks_busy: bool,
    /// How often the poll asks now ([`pace`]), and its loop.
    every: Duration,
    poller: Option<Task<()>>,
}

/// How often to poll: slower while the window is away.
fn pace(window: &Window) -> Duration {
    if !window.is_visible() {
        POLL_HIDDEN
    } else if !window.is_window_active() {
        POLL_AWAY
    } else {
        POLL
    }
}

impl MainWindow {
    pub fn new(backend: Arc<dyn Backend>, window: &mut Window, cx: &mut Context<Self>) -> MainWindow {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        cx.set_global(ui::select::CopyFocus(focus.clone()));
        let mut m = MainWindow {
            backend,
            data: Data::default(),
            // Nothing on screen shows or clears a project or goal filter, so a saved one would
            // hide work silently: start unfiltered.
            filters: Filters { project: "all".into(), goal: "all".into(), ..Filters::load() },
            page: Page::Board,
            back: Vec::new(),
            forward: Vec::new(),
            panel: None,
            modal: None,
            menu: None,
            toasts: Vec::new(),
            down: None,
            focus,
            board: Default::default(),
            goal_page: Default::default(),
            backlog: Default::default(),
            sessions: Default::default(),
            task_panel: Default::default(),
            issue_panel: Default::default(),
            hours: Default::default(),
            days: Default::default(),
            review: Default::default(),
            tasks: Default::default(),
            goals: Default::default(),
            projects: Default::default(),
            releases: Default::default(),
            ideas: Default::default(),
            idea: Default::default(),
            search: Default::default(),
            goals_loaded: false,
            generation: 0,
            answered: 0,
            posted: 0,
            shown: Default::default(),
            drawn: Instant::now(),
            settling: Vec::new(),
            panel_fetch: None,
            goal_fetch: None,
            recent_goals: Vec::new(),
            hooks: if crate::hooks::target().is_some() { crate::hooks::Hooks::Checking } else { crate::hooks::Hooks::Hidden },
            hooks_busy: false,
            every: pace(window),
            poller: None,
        };
        if let Ok(p) = std::env::var("TASKBOARD_PAGE") {
            m.page = match p.as_str() {
                "backlog" | "plan" => Page::Backlog,
                "sessions" => Page::Sessions,
                "days" => Page::Days,
                "review" => Page::Review,
                "tasks" => Page::Tasks,
                "goals" => Page::Goals,
                "projects" => Page::Projects,
                "releases" => Page::Releases,
                r if r.starts_with("releases:") => Page::ReleasesOf(r["releases:".len()..].into()),
                "ideas" => Page::Ideas,
                g if g.starts_with('G') => Page::Goal(g.into()),
                i if i.starts_with('I') => Page::Idea(i.into()),
                _ => Page::Board,
            };
        }
        // Dev: `TASKBOARD_PICK=B2,B3` checks those issues on the Backlog page (screenshots).
        if let Ok(p) = std::env::var("TASKBOARD_PICK") {
            m.backlog.picked = p.split(',').map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect();
        }
        // Dev: `TASKBOARD_TASKS_BY=status|project` groups the Tasks page that way (screenshots).
        match std::env::var("TASKBOARD_TASKS_BY").as_deref() {
            Ok("status") => m.tasks.group = ui::tasks::GroupBy::Status,
            Ok("project") => m.tasks.group = ui::tasks::GroupBy::Project,
            _ => {}
        }
        // Dev: `TASKBOARD_GOALS_OPEN=G1,G2` opens those goals in place on the Goals page (screenshots).
        if let Ok(v) = std::env::var("TASKBOARD_GOALS_OPEN") {
            m.goals.open = v.split(',').map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect();
        }
        // Dev: `TASKBOARD_PROJECT=web` picks that project and `TASKBOARD_PROJECTS_OPEN=G1,G2` opens those
        // goals in place on the Projects page (screenshots).
        if let Ok(v) = std::env::var("TASKBOARD_PROJECT") {
            m.projects.selected = Some(v);
        }
        if let Ok(v) = std::env::var("TASKBOARD_PROJECTS_OPEN") {
            m.projects.open = v.split(',').map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect();
        }
        // Dev: `TASKBOARD_RELEASES_OPEN=R1,unreleased:web` opens those rows on the Releases page (screenshots).
        if let Ok(v) = std::env::var("TASKBOARD_RELEASES_OPEN") {
            m.releases.open = v.split(',').map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect();
        }
        if let Ok(r) = std::env::var("TASKBOARD_OPEN") {
            m.panel = Some(if r.starts_with('B') { Panel::Issue { r } } else { Panel::Task { r, tab: TaskTab::Overview } });
        }
        // Follow the system's light / dark switch (unless TASKBOARD_THEME pins one).
        cx.observe_window_appearance(window, |_, window, cx| {
            let dark = matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark);
            let t = cx.global::<Theme>();
            let mode = crate::theme::startup_mode(dark);
            if t.mode != mode {
                let next = Theme::new(mode, t.ui_font.clone(), t.mono_font.clone());
                cx.set_global(next);
                cx.notify();
            }
        })
        .detach();
        // Poll slower while the window is away; catch up when it's back.
        cx.observe_window_activation(window, |m, window, cx| m.repace(window, cx)).detach();
        cx.observe_window_visibility(window, |m, _, window, cx| m.repace(window, cx)).detach();
        m.start_polling(cx);
        // Dev: `TASKBOARD_SEARCH=pass` opens the ⌘K palette with that query (screenshots).
        if let Ok(q) = std::env::var("TASKBOARD_SEARCH") {
            ui::search::open(&mut m, window, cx);
            if let Some(i) = &m.search.input {
                i.set_text(&q, cx);
            }
        }
        // Dev: `TASKBOARD_LINK=taskboard://task/T2?do=cancel` follows that link at startup (screenshots).
        if let Ok(u) = std::env::var("TASKBOARD_LINK") {
            m.open_link(&u, cx);
        }
        m
    }

    // -------------------------------------------------------------- navigation

    /// Change the board filters (saved for the next launch, as the web board did) and refetch.
    pub fn set_filters(&mut self, f: Filters, cx: &mut Context<Self>) {
        // The web cleared the kept backlog cards on every project or goal change.
        if f.project != self.filters.project || f.goal != self.filters.goal {
            self.board.keep.clear();
        }
        f.save();
        self.filters = f;
        self.refresh(cx);
    }

    /// Esc on the board with a filter set (`railClear`).
    pub fn rail_clear(&mut self, cx: &mut Context<Self>) {
        let f = ui::goals::rail_clear(&self.filters);
        self.board.keep.clear();
        self.set_filters(f, cx);
    }

    /// "Goals" (the web's `#/goals`): the Goals page.
    pub fn open_goals(&mut self, cx: &mut Context<Self>) {
        self.go(Page::Goals, cx);
    }

    pub fn goals_loaded(&self) -> bool {
        self.goals_loaded
    }

    /// Go to a page; an open task panel stays over it.
    pub fn go(&mut self, page: Page, cx: &mut Context<Self>) {
        let to = Nav { page, task: self.here().task };
        if self.here() != to {
            self.remember();
            self.show(to, cx);
        }
    }

    /// ⌘←: the place before this one.
    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        let here = self.here();
        while let Some(nav) = self.back.pop() {
            if nav != here {
                self.forward.push(here);
                return self.show(nav, cx);
            }
        }
    }

    /// ⌘→: the place ⌘← left.
    pub fn go_forward(&mut self, cx: &mut Context<Self>) {
        let here = self.here();
        while let Some(nav) = self.forward.pop() {
            if nav != here {
                self.back.push(here);
                return self.show(nav, cx);
            }
        }
    }

    /// A task's own "Back to T1": ⌘← when that's the place before this one. False when it isn't.
    pub fn back_to_task(&mut self, r: &str, cx: &mut Context<Self>) -> bool {
        let to = Nav { page: self.page.clone(), task: Some(r.to_string()) };
        if self.back.last() != Some(&to) {
            return false;
        }
        self.go_back(cx);
        true
    }

    /// The page on screen, and the task over it.
    fn here(&self) -> Nav {
        let task = match &self.panel {
            Some(Panel::Task { r, .. }) => Some(r.clone()),
            _ => None,
        };
        Nav { page: self.page.clone(), task }
    }

    /// A fresh visit: keep this place for ⌘← and drop the forward ones.
    fn remember(&mut self) {
        let here = self.here();
        if self.back.last() != Some(&here) {
            self.back.push(here);
        }
        self.forward.clear();
    }

    /// Go to a place without touching the history.
    fn show(&mut self, nav: Nav, cx: &mut Context<Self>) {
        if nav.task.is_none() && self.task_panel_open() {
            self.panel = None;
            self.data.task = None;
            self.data.task_card = None;
            cx.notify();
        }
        self.visit(nav.page, cx);
        if let Some(r) = nav.task {
            self.show_task(r, cx);
        }
    }

    /// Switch pages without touching the history.
    fn visit(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.page != page {
            // `resetPageState`: leaving Sessions drops its picking state; another goal's page
            // starts empty; arriving on the Backlog starts with no kept rows or selection.
            if self.page == Page::Sessions {
                ui::sessions::left(self);
            }
            if matches!(self.page, Page::Goal(_)) {
                self.goal_page.reset_for("");
                if let Some(g) = self.data.goal.take() {
                    self.keep_goal(g);
                }
            }
            if let Page::Goal(r) = &page {
                // A goal opened lately shows as it was until its own answer lands.
                self.data.goal = self.recent_goals.iter().find(|g| g["ref"].as_str() == Some(r.as_str())).cloned();
                self.data.errs.goal = None;
                self.fetch_goal(r.clone(), cx);
            }
            if let Page::Idea(r) = &page {
                if self.data.idea.as_ref().and_then(|i| i["ref"].as_str()) != Some(r.as_str()) {
                    self.data.idea = None;
                    self.data.errs.idea = None;
                }
            }
            let entering_backlog = page == Page::Backlog;
            self.page = page;
            if entering_backlog {
                ui::backlog::enter(self);
            }
            self.menu = None;
            self.refresh(cx);
        }
    }

    /// Whether the task panel is open beside the current page.
    pub fn task_panel_open(&self) -> bool {
        matches!(self.panel, Some(Panel::Task { .. }))
    }

    pub fn open_task(&mut self, r: impl Into<String>, cx: &mut Context<Self>) {
        let r = r.into();
        if self.here().task.as_ref() != Some(&r) {
            self.remember();
        }
        self.show_task(r, cx);
    }

    fn show_task(&mut self, r: String, cx: &mut Context<Self>) {
        // The same task stays as it was (its tab too) beside a new page.
        let tab = match &self.panel {
            Some(Panel::Task { r: cur, tab }) if *cur == r => *tab,
            _ => {
                // What the clicked row already knows shows at once; the rest fills in when the
                // task's own answer lands.
                self.data.task = None;
                self.data.task_card = task_card(&self.data, &self.goals.details, &self.tasks.held.rows, &r);
                self.data.errs.task = None;
                self.task_panel = Default::default();
                self.fetch_panel(r.clone(), format!("tasks/{r}"), cx);
                TaskTab::Overview
            }
        };
        self.panel = Some(Panel::Task { r, tab });
        self.refresh(cx);
    }

    /// A panel's own `GET` (`tasks/:id` or `backlog/:id`), so a task or issue just opened doesn't
    /// wait for the rest of a refresh.
    fn fetch_panel(&mut self, r: String, path: String, cx: &mut Context<Self>) {
        self.panel_fetch = Some(r.clone());
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let v: Answer = cx.background_executor().spawn(async move { backend.get(&path, &[]).map_err(|e| e.message) }).await;
            let _ = this.update(cx, |m, cx| {
                if m.panel_fetch.as_ref() == Some(&r) {
                    m.panel_fetch = None;
                }
                let d = &mut m.data;
                match &m.panel {
                    Some(Panel::Task { r: cur, .. }) if *cur == r => land(&mut d.task, &mut d.errs.task, v, |v| v),
                    Some(Panel::Issue { r: cur }) if *cur == r => land(&mut d.issue, &mut d.errs.issue, v, unwrap_issue),
                    _ => return,
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A goal page's own `GET /goals/:id`, so it doesn't wait for `GET /state` and the rest of a
    /// refresh. An answer from before a change is dropped and asked for again.
    fn fetch_goal(&mut self, r: String, cx: &mut Context<Self>) {
        self.goal_fetch = Some(r.clone());
        let (backend, asked) = (self.backend.clone(), self.generation);
        let path = format!("goals/{r}");
        cx.spawn(async move |this, cx| {
            let v: Answer = cx.background_executor().spawn(async move { backend.get(&path, &[]).map_err(|e| e.message) }).await;
            let _ = this.update(cx, |m, cx| {
                if m.goal_fetch.as_ref() != Some(&r) {
                    return;
                }
                m.goal_fetch = None;
                if m.page != Page::Goal(r) {
                    return;
                }
                if m.posted > asked {
                    return m.refresh(cx);
                }
                land_goal(&mut m.data.goal, &mut m.data.errs.goal, v);
                cx.notify();
            });
        })
        .detach();
    }

    /// Keep a goal just left for going back to, dropping the oldest past [`RECENT_GOALS`].
    fn keep_goal(&mut self, g: Rc<Value>) {
        self.recent_goals.retain(|x| x["ref"] != g["ref"]);
        self.recent_goals.insert(0, g);
        self.recent_goals.truncate(RECENT_GOALS);
    }

    pub fn open_issue(&mut self, r: impl Into<String>, cx: &mut Context<Self>) {
        let r = r.into();
        if !matches!(&self.panel, Some(Panel::Issue { r: cur }) if *cur == r) {
            self.data.issue = None;
            self.data.errs.issue = None;
            self.issue_panel = Default::default();
            self.fetch_panel(r.clone(), format!("backlog/{r}"), cx);
        }
        self.panel = Some(Panel::Issue { r });
        self.refresh(cx);
    }

    /// A `taskboard://task/T12` / `goal/G3` / `release/R2` / `session/<id>` link (a clicked notification), or
    /// a web-board route after a `#` (`tasks#/goals/G3`, `#/?task=T6`), as `tb` prints them.
    /// `task/T12?do=done|cancel` (a terminal link menu) opens the task with its confirm over it.
    pub fn open_link(&mut self, url: &str, cx: &mut Context<Self>) {
        let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
        let (path, query) = match rest.split_once('#') {
            Some((_, route)) => route.split_once('?').unwrap_or((route, "")),
            None => rest.split_once('?').unwrap_or((rest, "")),
        };
        let param = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(k)?.strip_prefix('='));
        let path = path.trim_matches('/');
        let (kind, arg) = path.split_once('/').unwrap_or((path, ""));
        if let Some(t) = param("task").filter(|t| !t.is_empty()) {
            self.open_link(path, cx);
            return self.open_task(t.to_uppercase(), cx);
        }
        match (kind, arg) {
            ("task", r) if !r.is_empty() => {
                self.open_task(r.to_uppercase(), cx);
                // A link menu's Mark done / Cancel task…: only asks; the dialog does it.
                if let Some(cancel) = match param("do") {
                    Some("done") => Some(false),
                    Some("cancel") => Some(true),
                    _ => None,
                } {
                    self.modal = Some(ui::modals::Modal::finish(r.to_uppercase(), cancel));
                    cx.notify();
                }
            }
            ("issue", r) if !r.is_empty() => self.open_issue(r.to_uppercase(), cx),
            ("goal" | "goals", r) if !r.is_empty() => ui::goal::open(self, &r.to_uppercase(), param("view") == Some("backlog"), cx),
            ("goals", _) => self.open_goals(cx),
            ("idea" | "ideas", r) if !r.is_empty() => ui::idea::open(self, &r.to_uppercase(), cx),
            ("ideas", _) => self.go(Page::Ideas, cx),
            ("session", id) if !id.is_empty() => {
                self.sessions.selected = Some(id.to_string());
                self.go(Page::Sessions, cx);
                self.refresh(cx);
            }
            ("backlog" | "plan", _) => self.go(Page::Backlog, cx),
            ("sessions", _) => self.go(Page::Sessions, cx),
            ("days", _) => self.go(Page::Days, cx),
            ("review" | "reviews", id) => ui::review::open(self, id.parse().ok(), cx),
            ("tasks", _) => self.go(Page::Tasks, cx),
            ("projects", p) => {
                if !p.is_empty() {
                    self.projects.selected = Some(p.to_string());
                }
                self.go(Page::Projects, cx);
            }
            ("release" | "releases", r) if !r.is_empty() && !ui::releases::projects_on(self.state()).is_empty() => ui::releases::open(self, &r.to_uppercase(), cx),
            ("releases", _) if !ui::releases::projects_on(self.state()).is_empty() => self.go(Page::Releases, cx),
            _ => self.go(Page::Board, cx),
        }
    }

    pub fn close_panel(&mut self, cx: &mut Context<Self>) {
        // Closing a task is a step in the history: ⌘← opens it again.
        if self.task_panel_open() {
            self.remember();
        }
        self.panel = None;
        self.data.task = None;
        self.data.task_card = None;
        self.data.issue = None;
        cx.notify();
    }

    pub fn set_modal(&mut self, modal: Option<ui::modals::Modal>, window: &mut Window, cx: &mut Context<Self>) {
        self.modal = modal;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn toggle_menu(&mut self, key: &str, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.menu = match &self.menu {
            Some((k, _)) if k == key => None,
            _ => Some((key.to_string(), at)),
        };
        cx.notify();
    }

    pub fn menu_open(&self, key: &str) -> Option<Point<Pixels>> {
        self.menu.as_ref().filter(|(k, _)| k == key).map(|(_, p)| *p)
    }

    // -------------------------------------------------------------- toasts

    /// One toast at a time, as the web did: a new one replaces the last; it goes after 3 s (6 s
    /// for an error).
    pub fn toast(&mut self, text: impl Into<String>, err: bool, cx: &mut Context<Self>) {
        self.show_toast(Toast { text: text.into(), err, at: Instant::now(), undo: None }, cx);
    }

    /// A toast with Undo, which POSTs `undo`; it stays as long as an error's.
    pub fn toast_undo(&mut self, text: impl Into<String>, undo: impl Into<String>, cx: &mut Context<Self>) {
        self.show_toast(Toast { text: text.into(), err: false, at: Instant::now(), undo: Some(undo.into()) }, cx);
    }

    fn show_toast(&mut self, toast: Toast, cx: &mut Context<Self>) {
        let at = toast.at;
        let stay = if toast.err || toast.undo.is_some() { TOAST_ERR } else { TOAST_OK };
        self.toasts = vec![toast];
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(stay).await;
            let _ = this.update(cx, |m, cx| {
                m.toasts.retain(|t| t.at != at);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The window title (`renderAll`): the page's name, with the needs-you count in front.
    pub fn title(&self) -> String {
        page_title(&self.page, self.needs_count())
    }

    /// Esc (the web's keydown handler, in its order): an open menu, then the alerts dialog, then the task panel (or an issue panel on the board),
    /// then, on the board with nothing focused, the board's filters.
    pub fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // An open picker closes first, giving focus back and keeping the Backlog's checks.
        if self.issue_panel.picker.as_ref().is_some_and(|p| self.menu_open(&p.key).is_some()) {
            return ui::issue_panel::close_picker(self, window, cx);
        }
        // On the Backlog page esc unchecks everything, closing its menu too.
        if ui::backlog::escape(self, cx) {
            return;
        }
        if self.menu.is_some() {
            self.menu = None;
            cx.notify();
            return;
        }
        if ui::sessions::escape(self, window, cx) {
            return;
        }
        if self.modal.is_some() {
            self.set_modal(None, window, cx);
            return;
        }
        if ui::task_panel::escape(cx) {
            return;
        }
        match &self.panel {
            Some(Panel::Task { .. }) => return self.close_panel(cx),
            Some(Panel::Issue { .. }) => return self.close_panel(cx),
            None => {}
        }
        if self.page == Page::Board && self.focus.is_focused(window) && (self.filters.goal != "all" || self.filters.project != "all") {
            self.rail_clear(cx);
        }
    }

    // -------------------------------------------------------------- calls

    /// `POST` to the board off the UI thread. On success runs `ok` (with the answer) and
    /// refreshes; on failure shows the board's sentence as an error toast.
    pub fn post(
        &mut self,
        path: impl Into<String>,
        body: Value,
        cx: &mut Context<Self>,
        ok: impl FnOnce(&mut MainWindow, Value, &mut Context<MainWindow>) + 'static,
    ) {
        self.post_or(path, body, cx, ok, |m, e, cx| m.toast(e.message, true, cx));
    }

    /// Like [`post`](Self::post) with the caller handling the error (inline notes in forms).
    pub fn post_or(
        &mut self,
        path: impl Into<String>,
        body: Value,
        cx: &mut Context<Self>,
        ok: impl FnOnce(&mut MainWindow, Value, &mut Context<MainWindow>) + 'static,
        fail: impl FnOnce(&mut MainWindow, CallError, &mut Context<MainWindow>) + 'static,
    ) {
        let (backend, path) = (self.backend.clone(), path.into());
        // A refresh already out may have read the board before this change: drop its answer
        // (polls wait meanwhile) so it can't put back what the click just changed. The refresh
        // after the change answers instead.
        self.generation += 1;
        self.posted = self.generation;
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { backend.post(&path, body) }).await;
            let _ = this.update(cx, |m, cx| {
                // So may one asked (a page opened) while the change was out.
                m.posted = m.generation;
                match r {
                    Ok(v) => ok(m, v, cx),
                    Err(e) => fail(m, e, cx),
                }
                m.refresh(cx);
            });
        })
        .detach();
    }

    // -------------------------------------------------------------- polling

    fn start_polling(&mut self, cx: &mut Context<Self>) {
        self.refresh(cx);
        self.check_hooks(cx);
        self.schedule(cx);
    }

    /// Poll every `self.every` from now on. Replacing the held loop drops (cancels) the old one.
    fn schedule(&mut self, cx: &mut Context<Self>) {
        let every = self.every;
        self.poller = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(every).await;
                if this
                    .update(cx, |m, cx| m.poll(cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    /// The window came forward, went behind another app or left the screen: poll at its pace,
    /// and at once when it came back (it may have shown a 30 s old board).
    fn repace(&mut self, window: &Window, cx: &mut Context<Self>) {
        let every = pace(window);
        if every == self.every {
            return;
        }
        let back = every < self.every;
        self.every = every;
        if back {
            self.poll(cx);
        }
        self.schedule(cx);
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        // A poll waits for a refresh that's still out (a page just opened) instead of starting over
        // and throwing its answer away.
        if self.answered == self.generation {
            self.ask(true, cx);
        }
        self.check_hooks(cx);
    }

    /// Re-read Claude Code's config for the hooks' state (it can change outside the app).
    fn check_hooks(&mut self, cx: &mut Context<Self>) {
        let Some((want, _)) = crate::hooks::target() else { return };
        if self.hooks_busy {
            return;
        }
        cx.spawn(async move |this, cx| {
            let h = cx.background_executor().spawn(async move { crate::hooks::check(&crate::hooks::claude_dir(), &want) }).await;
            let _ = this.update(cx, |m, cx| {
                if m.hooks != h && !m.hooks_busy {
                    m.hooks = h;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The status bar's "Install hooks" / "Reinstall hooks".
    pub fn install_hooks(&mut self, cx: &mut Context<Self>) {
        let Some((want, true)) = crate::hooks::target() else { return };
        if self.hooks_busy {
            return;
        }
        self.hooks_busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let w = want.clone();
            let r = cx.background_executor().spawn(async move { crate::hooks::install(&w) }).await;
            let h = cx.background_executor().spawn(async move { crate::hooks::check(&crate::hooks::claude_dir(), &want) }).await;
            let _ = this.update(cx, |m, cx| {
                m.hooks_busy = false;
                m.hooks = h;
                match r {
                    Ok(()) => m.toast("Hooks installed. New Claude sessions report to the board.", false, cx),
                    Err(e) => m.toast(e, true, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Fetch everything the current view shows, every call at once, each swapping in as it answers
    /// (see [`part`](Self::part)) so a page's own answer doesn't wait behind `GET /state`. Once all
    /// have, the refresh has answered (see [`landed`](Self::landed)).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.ask(false, cx);
    }

    /// A refresh. One for a `poll` redraws only when its answer differs from the one shown.
    fn ask(&mut self, poll: bool, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let asked = Wants::of(self, cx);
        let (tx, rx) = async_channel::unbounded();
        for (key, call) in asked.clone().calls() {
            let (backend, tx, asked) = (self.backend.clone(), tx.clone(), asked.clone());
            cx.background_executor()
                .spawn(async move {
                    let mut got = call(&*backend);
                    got.print = got.fingerprint(&asked);
                    tx.send((key, got)).await.ok()
                })
                .detach();
        }
        drop(tx);
        cx.spawn(async move |this, cx| {
            // While the board is down only the banner speaks (`S.stateErr`): a page's error waits
            // for `GET /state`, and is dropped when that failed too.
            let (mut up, mut held) = (None, Vec::new());
            while let Ok((key, got)) = rx.recv().await {
                let failed = got.failed();
                if let Some(s) = &got.state {
                    up = Some(s.is_ok());
                }
                match up {
                    None if failed => {
                        held.push((key, got));
                        continue;
                    }
                    Some(false) if failed => continue,
                    _ => {}
                }
                let mut now = vec![(key, got)];
                if up == Some(true) {
                    now.append(&mut held);
                }
                let landed = this.update(cx, |m, cx| {
                    for (key, got) in now {
                        m.part(generation, &asked, key, got, poll, cx);
                    }
                });
                if landed.is_err() {
                    return;
                }
            }
            let _ = this.update(cx, |m, cx| m.landed(generation, &asked, poll, cx));
        })
        .detach();
    }

    /// One call of the refresh asked at `generation` (for `asked`) answered, filling the part named
    /// `key`. The newest refresh's swaps in. One a page or panel change outdated still swaps in what
    /// the view asks for now (the board, the goals) instead of waiting a whole round trip for the
    /// newer one; one from before a change, or under a newer answer, is dropped. A poll's part that
    /// brings nothing new skips the redraw (but for ages, every [`AGES`]).
    fn part(&mut self, generation: u64, asked: &Wants, key: String, got: Got, poll: bool, cx: &mut Context<Self>) {
        let outdated = self.generation != generation;
        let got = if !outdated {
            got
        } else if generation > self.answered && generation > self.posted {
            got.still(asked, &Wants::of(self, cx))
        } else {
            return;
        };
        let (at, print) = self.shown.entry(key).or_default();
        if *at > generation {
            return;
        }
        let redraw = outdated || !poll || *print != got.print || !self.settling.is_empty() || self.drawn.elapsed() >= AGES;
        // What an outdated answer keeps isn't what its fingerprint covers, so the next one redraws.
        (*at, *print) = (generation, if outdated { 0 } else { got.print });
        got.apply(self, cx);
        if redraw {
            cx.notify();
        }
    }

    /// Every call of the refresh asked at `generation` (for `asked`) has answered. After the newest
    /// one, what waited on it runs and a page that learned of more to fetch asks for it.
    fn landed(&mut self, generation: u64, asked: &Wants, poll: bool, cx: &mut Context<Self>) {
        self.answered = self.answered.max(generation);
        if self.generation != generation {
            return;
        }
        let asked_rows = &asked.goal_rows;
        let asked_releases: Vec<String> = asked.releases.iter().map(|(p, _)| p.clone()).chain(asked.release_rows.iter().cloned()).collect();
        let redraw = !poll || !self.settling.is_empty();
        self.settled(generation, cx);
        if redraw {
            cx.notify();
        }
        // A linked release whose project the page didn't know: its answer says which.
        if let Some(p) = ui::releases::focus_moves(&self.releases, &self.page) {
            self.go(Page::ReleasesOf(p), cx);
        }
        // The Goals page learns which goals are in progress from this answer; fetch the
        // ones it didn't ask for now rather than on the next poll.
        if self.page == Page::Goals && ui::goals_page::in_progress(&self.data.goals, self.goals.project()).iter().any(|r| !asked_rows.contains(r)) {
            self.refresh(cx);
        }
        // So does the Projects page, for the picked project's goals.
        if self.page == Page::Projects && ui::projects::wanted(&self.projects, self.state(), &self.data.goals).iter().any(|r| !asked_rows.contains(r)) {
            self.refresh(cx);
        }
        // Likewise the Releases page learns which projects have releases from the state.
        if is_releases(&self.page) && releases_wanted(self).iter().any(|k| !asked_releases.contains(k)) {
            self.refresh(cx);
        }
    }

    /// Run `f` once the board's answer after a change has landed (the clicked button shows what
    /// changed instead of flashing back first), and no sooner than [`HOLD`] after `since` (a quick
    /// answer still leaves the button's "Sending…" up long enough to see). Call it from a
    /// [`post`](Self::post)'s `ok`: the refresh that follows is the answer it waits for.
    pub fn settle(&mut self, since: Instant, f: impl FnOnce(&mut MainWindow, &mut Context<MainWindow>) + 'static) {
        self.settling.push((self.generation, since, Box::new(f)));
    }

    /// The refresh asked at `generation` landed: run what was waiting on an answer from after it.
    fn settled(&mut self, generation: u64, cx: &mut Context<Self>) {
        let (due, wait): (Vec<_>, Vec<_>) = std::mem::take(&mut self.settling).into_iter().partition(|(g, ..)| *g < generation);
        self.settling = wait;
        for (_, since, f) in due {
            let left = HOLD.saturating_sub(since.elapsed());
            if left.is_zero() {
                f(self, cx);
                continue;
            }
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(left).await;
                let _ = this.update(cx, |m, cx| {
                    f(m, cx);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    // -------------------------------------------------------------- helpers for pages

    pub fn state(&self) -> &Value {
        static NULL: Value = Value::Null;
        self.data.state.as_deref().unwrap_or(&NULL)
    }

    /// The board state as a handle a page keeps while it borrows the window mutably.
    pub fn state_rc(&self) -> Rc<Value> {
        self.data.state.clone().unwrap_or_default()
    }

    pub fn jira_on(&self) -> bool {
        b(&self.state()["jira"], "enabled")
    }

    pub fn needs_count(&self) -> i64 {
        fmt::i(&self.state()["counts"], "needs")
    }

    /// `projectNames` over this window's goals (`allGoals`: non-archived).
    #[cfg(test)]
    pub fn project_names(&self) -> Vec<String> {
        project_names(self.state(), ui::goals::all_goals(self.data.state.as_deref(), self.goals_loaded.then_some(self.data.goals.as_slice())))
    }

}

// ------------------------------------------------------------------ fetching

fn is_releases(page: &Page) -> bool {
    matches!(page, Page::Releases | Page::ReleasesOf(_))
}

/// The projects a Releases page lists, and how many releases of each (none on another page).
fn releases_shown(m: &MainWindow) -> Vec<(String, usize)> {
    let limit = ui::releases::list_limit(&m.releases, &m.page);
    ui::releases::shown(m.state(), &m.page).into_iter().map(|p| (p, limit)).collect()
}

/// What a Releases page fetches: its projects' lists and its rows in full.
fn releases_wanted(m: &MainWindow) -> Vec<String> {
    let rows = ui::releases::open_rows(&m.releases, &m.page);
    releases_shown(m).into_iter().map(|(p, _)| p).chain(rows).collect()
}

/// Task `r`'s row in the lists already loaded (the board's columns, the Tasks page, home, a goal
/// page), for its panel to show before `GET /tasks/:id` answers.
fn task_card(d: &Data, goal_rows: &std::collections::HashMap<String, Value>, held: &[Value], r: &str) -> Option<Value> {
    let state = d.state.as_ref();
    let columns = state.and_then(|s| s["columns"].as_object()).into_iter().flat_map(|c| c.values());
    let lists = columns
        .chain(state.map(|s| &s["planned"]))
        .chain(d.tasks.as_ref().map(|t| &t["tasks"]))
        .chain(d.goal.as_ref().map(|g| &g["tasks"]))
        .chain(goal_rows.values().map(|g| &g["tasks"]))
        .chain(d.home.as_ref().into_iter().flat_map(|h| arr(h, "goals").iter().map(|g| &g["now"])));
    let needs = d.home.as_ref().into_iter().flat_map(|h| arr(h, "needs").iter().map(|n| &n["card"]));
    lists
        .filter_map(Value::as_array)
        .flatten()
        .chain(held)
        .chain(needs)
        .find(|t| t["ref"].as_str().is_some_and(|x| crate::fmt::same_task(x, r)) && t["title"].is_string())
        .cloned()
}

/// What one refresh fetches, decided on the UI thread from the current view.
#[derive(Clone, PartialEq, Hash)]
struct Wants {
    state_q: Vec<(&'static str, String)>,
    task: Option<String>,
    issue: Option<String>,
    goal: Option<String>,
    /// The Goals page's goals in progress, each fetched in full for its row.
    goal_rows: Vec<String>,
    sessions: bool,
    session: Option<String>,
    days_q: Option<Vec<(&'static str, String)>>,
    /// The board's project filter, for `GET /home`.
    home: Option<String>,
    /// The ⌘K palette's query, for its tasks and backlog issues (`GET /search?palette=1`).
    palette: Option<String>,
    /// The Tasks page: its first page of `GET /tasks`, and the held tasks to ask for again.
    task_page: Option<Vec<i64>>,
    /// The Review page: the list, and the picked review (`None`: the newest).
    review: Option<Option<i64>>,
    /// A Releases page: the projects it lists (with how many releases of each), and its rows in full.
    releases: Vec<(String, usize)>,
    release_rows: Vec<String>,
    /// The Ideas page: its first page of `GET /ideas`, and the held ideas to ask for again.
    ideas: Option<Vec<i64>>,
    /// An idea's page, for `GET /ideas/:id`.
    idea: Option<String>,
}

/// One fetch's answer: the value, or the board's sentence for why it failed.
type Answer = Result<Value, String>;

/// One call of a refresh, answering with just its part of a [`Got`].
type Call = Box<dyn FnOnce(&dyn Backend) -> Got + Send>;

#[derive(Default)]
struct Got {
    state: Option<Result<Value, CallError>>,
    goals: Option<Value>,
    task: Option<(String, Answer)>,
    issue: Option<(String, Answer)>,
    goal: Option<(String, Answer)>,
    goal_rows: Vec<(String, Answer)>,
    sessions: Option<Answer>,
    closed: Option<Value>,
    session: Option<(String, Answer)>,
    days: Option<Answer>,
    home: Option<Value>,
    today: Option<Value>,
    palette: Option<(String, Value)>,
    task_page: Option<Result<ui::paged::Fresh, String>>,
    reviews: Option<Answer>,
    review: Option<(i64, Answer)>,
    releases: Vec<(String, Answer)>,
    release_rows: Vec<(String, Answer)>,
    ideas: Option<Result<ui::paged::Fresh, String>>,
    idea: Option<(String, Answer)>,
    /// What was asked and answered, as one number ([`Got::fingerprint`]).
    print: u64,
}

impl Wants {
    fn of(m: &MainWindow, cx: &App) -> Wants {
        let mut state_q = crate::ui::board::state_query(&m.filters, &m.board.keep);
        // The goal list comes from `GET /goals` on every poll; the state needn't carry it twice.
        state_q.push(("goals", "0".into()));
        Wants {
            state_q,
            // A task or issue that just opened is fetched on its own (`fetch_panel`).
            task: match &m.panel {
                Some(Panel::Task { r, .. }) if m.panel_fetch.as_ref() != Some(r) => Some(r.clone()),
                _ => None,
            },
            issue: match &m.panel {
                Some(Panel::Issue { r }) if m.panel_fetch.as_ref() != Some(r) => Some(r.clone()),
                _ => None,
            },
            // So is a goal page that just opened (`fetch_goal`).
            goal: match &m.page {
                Page::Goal(g) if m.goal_fetch.as_ref() != Some(g) => Some(g.clone()),
                _ => None,
            },
            goal_rows: match m.page {
                Page::Goals => ui::goals_page::in_progress(&m.data.goals, m.goals.project()),
                Page::Projects => ui::projects::wanted(&m.projects, m.state(), &m.data.goals),
                _ => vec![],
            },
            // The ⌘K palette searches the terminal list too.
            sessions: m.page == Page::Sessions || ui::search::is_open(m),
            session: if m.page == Page::Sessions { m.sessions.selected.clone() } else { None },
            days_q: (m.page == Page::Days).then(|| m.days.query()),
            home: (m.page == Page::Board).then(|| m.filters.project.clone()),
            palette: ui::search::query(m, cx),
            task_page: (m.page == Page::Tasks).then(|| m.tasks.held.recheck(|v| v["status"] != "done")),
            review: (m.page == Page::Review).then_some(m.review.id),
            releases: releases_shown(m),
            release_rows: ui::releases::open_rows(&m.releases, &m.page),
            ideas: (m.page == Page::Ideas).then(|| m.ideas.held.recheck(ui::ideas::is_open)),
            idea: match &m.page {
                Page::Idea(r) => Some(r.clone()),
                _ => None,
            },
        }
    }

    /// Every call on its own, keyed by the part of the view it fills.
    fn calls(self) -> Vec<(String, Call)> {
        fn get(be: &dyn Backend, p: &str, q: &[(&str, String)]) -> Answer {
            be.get(p, q).map_err(|e| e.message)
        }
        fn add(c: &mut Vec<(String, Call)>, key: impl Into<String>, f: impl FnOnce(&dyn Backend) -> Got + Send + 'static) {
            c.push((key.into(), Box::new(f)));
        }
        let mut c = Vec::new();
        let Wants { state_q, task, issue, goal, goal_rows, sessions, session, days_q, home, palette, task_page, review, releases, release_rows, ideas, idea } = self;
        add(&mut c, "state", move |be| Got { state: Some(be.get("state", &state_q)), ..Got::default() });
        add(&mut c, "goals", |be| Got { goals: get(be, "goals", &[]).ok(), ..Got::default() });
        if let Some(r) = task {
            add(&mut c, "task", move |be| Got { task: Some((r.clone(), get(be, &format!("tasks/{r}"), &[]))), ..Got::default() });
        }
        if let Some(r) = issue {
            add(&mut c, "issue", move |be| Got { issue: Some((r.clone(), get(be, &format!("backlog/{r}"), &[]))), ..Got::default() });
        }
        if let Some(r) = goal {
            add(&mut c, "goal", move |be| Got { goal: Some((r.clone(), get(be, &format!("goals/{r}"), &[]))), ..Got::default() });
        }
        for r in goal_rows {
            add(&mut c, format!("goal_row/{r}"), move |be| Got { goal_rows: vec![(r.clone(), get(be, &format!("goals/{r}"), &[]))], ..Got::default() });
        }
        if sessions {
            add(&mut c, "sessions", |be| Got { sessions: Some(get(be, "sessions", &[("project", "all".into())])), ..Got::default() });
            add(&mut c, "closed", |be| Got { closed: get(be, "sessions/closed", &[("project", "all".into())]).ok(), ..Got::default() });
        }
        if let Some(id) = session {
            add(&mut c, "session", move |be| Got { session: Some((id.clone(), get(be, &format!("sessions/{id}"), &[]))), ..Got::default() });
        }
        if let Some(q) = days_q {
            add(&mut c, "days", move |be| Got { days: Some(get(be, "days", &q)), ..Got::default() });
        }
        if let Some(p) = home {
            add(&mut c, "today", |be| Got { today: get(be, "days", &[]).ok(), ..Got::default() });
            add(&mut c, "home", move |be| Got { home: get(be, "home", &[("project", p)]).ok(), ..Got::default() });
        }
        if let Some(q) = palette {
            add(&mut c, "palette", move |be| Got { palette: ui::search::ask(be, &q).ok().map(|v| (q, v)), ..Got::default() });
        }
        if let Some(ids) = task_page {
            add(&mut c, "task_page", move |be| Got { task_page: Some(ui::paged::Paged::fetch(be, "tasks", "tasks", &[], ui::tasks::PAGE, ids)), ..Got::default() });
        }
        if let Some(id) = review {
            add(&mut c, "reviews", move |be| {
                let mut list = get(be, "reviews", &[]);
                // Before the first review the page shows this week so far, kept on the list as `this_week`.
                if let Ok(l) = list.as_mut() && arr(l, "reviews").is_empty() {
                    l["this_week"] = get(be, "reviews/now", &[("week", "this".into())]).unwrap_or(Value::Null);
                }
                let newest = list.as_ref().ok().and_then(|l| arr(l, "reviews").first()?["id"].as_i64());
                let review = id.or(newest).map(|id| (id, get(be, &format!("reviews/{id}"), &[])));
                Got { reviews: Some(list), review, ..Got::default() }
            });
        }
        for (p, n) in releases {
            add(&mut c, format!("releases/{p}"), move |be| Got { releases: vec![(p.clone(), get(be, "releases", &[("project", p), ("limit", n.to_string())]))], ..Got::default() });
        }
        for k in release_rows {
            add(&mut c, format!("release_row/{k}"), move |be| {
                let (path, q) = ui::releases::detail_call(&k);
                Got { release_rows: vec![(k, get(be, &path, &q))], ..Got::default() }
            });
        }
        if let Some(ids) = ideas {
            add(&mut c, "ideas", move |be| Got { ideas: Some(ui::paged::Paged::fetch(be, "ideas", "ideas", &ui::ideas::query(), ui::ideas::PAGE, ids)), ..Got::default() });
        }
        if let Some(r) = idea {
            add(&mut c, "idea", move |be| Got { idea: Some((r.clone(), get(be, &format!("ideas/{r}"), &[]))), ..Got::default() });
        }
        c
    }
}

/// Swap in a good answer (clearing its error) or keep what's shown and remember why it failed.
fn land(slot: &mut Option<Value>, err: &mut Option<String>, answer: Answer, map: impl FnOnce(Value) -> Value) {
    match answer {
        Ok(v) => {
            *slot = Some(map(v));
            *err = None;
        }
        Err(e) => *err = Some(e),
    }
}

/// An answer lands as a new `Rc`, so the page knows to rebuild from it.
fn land_rc(slot: &mut Option<Rc<Value>>, err: &mut Option<String>, answer: Answer, map: impl FnOnce(Value) -> Value) {
    let mut v = None;
    land(&mut v, err, answer, map);
    if let Some(v) = v {
        *slot = Some(Rc::new(v));
    }
}

fn land_goal(slot: &mut Option<Rc<Value>>, err: &mut Option<String>, answer: Answer) {
    land_rc(slot, err, answer, merge_goal);
}

impl Got {
    /// Everything asked and answered, without the clocks ([`fingerprint::CLOCKS`]).
    fn fingerprint(&self, asked: &Wants) -> u64 {
        let mut f = Fingerprint::new(asked);
        f.add(&self.state).add(&self.goals).add(&self.task).add(&self.issue).add(&self.goal).add(&self.goal_rows);
        f.add(&self.sessions).add(&self.closed).add(&self.session).add(&self.home).add(&self.palette);
        f.add(&self.task_page).add(&self.reviews).add(&self.review).add(&self.releases).add(&self.release_rows);
        f.add(&self.ideas).add(&self.idea);
        f.skip = fingerprint::DAY_CLOCKS;
        f.add(&self.days).add(&self.today);
        f.finish()
    }

    /// Whether a page would show this answer's error (the board's own, `state`, is the banner's).
    fn failed(&self) -> bool {
        fn err<T, E>(r: Option<&Result<T, E>>) -> bool {
            r.is_some_and(Result::is_err)
        }
        err(self.task.as_ref().map(|(_, a)| a))
            || err(self.issue.as_ref().map(|(_, a)| a))
            || err(self.goal.as_ref().map(|(_, a)| a))
            || err(self.sessions.as_ref())
            || err(self.session.as_ref().map(|(_, a)| a))
            || err(self.days.as_ref())
            || err(self.task_page.as_ref())
            || err(self.reviews.as_ref())
            || err(self.review.as_ref().map(|(_, a)| a))
            || err(self.ideas.as_ref())
            || err(self.idea.as_ref().map(|(_, a)| a))
    }

    /// What of an answer asked for `asked` still holds for a view that asks for `now`. Answers for one
    /// task, goal, issue and so on keep (`apply` takes them only while that one is still open); the
    /// rest keep only when the view asks for them the same way.
    fn still(mut self, asked: &Wants, now: &Wants) -> Got {
        if asked.state_q != now.state_q {
            self.state = None;
        }
        if asked.days_q != now.days_q {
            self.days = None;
        }
        if asked.home != now.home {
            self.home = None;
        }
        if asked.task_page != now.task_page {
            self.task_page = None;
        }
        if asked.ideas != now.ideas {
            self.ideas = None;
        }
        if asked.releases != now.releases {
            self.releases.clear();
        }
        self
    }

    fn apply(self, m: &mut MainWindow, cx: &App) {
        match self.state {
            Some(Ok(v)) => {
                fmt::set_server_now(s(&v, "now"));
                if arr(&v, "alerts").is_empty() && matches!(m.modal, Some(ui::modals::Modal::Alerts)) {
                    m.modal = None;
                }
                m.data.state = Some(Rc::new(v));
                m.down = None;
            }
            Some(Err(e)) => m.down = Some(e.message),
            None => {}
        }
        if let Some(g) = self.goals {
            m.data.goals = Rc::new(if g.is_array() { g.as_array().cloned().unwrap_or_default() } else { arr(&g, "goals").to_vec() });
            m.goals_loaded = true;
        }
        if is_releases(&m.page) {
            if m.data.state.is_some() && ui::releases::projects_on(m.state()).is_empty() {
                m.page = Page::Board;
            } else if m.data.state.is_some() && ui::releases::shown(m.state(), &m.page).is_empty() {
                // That project's releases were turned off.
                m.page = Page::Releases;
            }
            for (p, v) in self.releases {
                if let Ok(v) = v {
                    m.releases.lists.insert(p, v);
                }
            }
            for (k, v) in self.release_rows {
                if let Ok(v) = v {
                    m.releases.details.insert(k, v);
                }
            }
        }
        // Only keep answers for what is still open.
        if let Some((q, v)) = self.palette {
            ui::search::land(m, &q, v, cx);
        }
        let d = &mut m.data;
        if let Some((r, v)) = self.task {
            if matches!(&m.panel, Some(Panel::Task { r: cur, .. }) if *cur == r) {
                land(&mut d.task, &mut d.errs.task, v, |v| v);
            }
        }
        if let Some((r, v)) = self.issue {
            if matches!(&m.panel, Some(Panel::Issue { r: cur }) if *cur == r) {
                land(&mut d.issue, &mut d.errs.issue, v, unwrap_issue);
            }
        }
        if let Some((r, v)) = self.goal {
            if m.page == Page::Goal(r) {
                land_goal(&mut d.goal, &mut d.errs.goal, v);
            }
        }
        match self.ideas.filter(|_| m.page == Page::Ideas) {
            Some(Ok(f)) => {
                d.errs.ideas = None;
                m.ideas.projects = ui::ideas::tabs(f.first());
                m.ideas.split = ui::ideas::split(f.first());
                m.ideas.held.land("ideas", f);
            }
            Some(Err(e)) => d.errs.ideas = Some(e),
            None => {}
        }
        if let Some((r, v)) = self.idea {
            if m.page == Page::Idea(r) {
                land(&mut d.idea, &mut d.errs.idea, v, |v| v);
            }
        }
        if m.page == Page::Goals {
            for (r, v) in self.goal_rows {
                if let Ok(v) = v {
                    m.goals.land(r, merge_goal(v));
                }
            }
        } else if m.page == Page::Projects {
            for (r, v) in self.goal_rows {
                if let Ok(v) = v {
                    m.projects.land(r, merge_goal(v));
                }
            }
        }
        if let Some(v) = self.sessions {
            land(&mut d.sessions, &mut d.errs.sessions, v, |v| v);
        }
        if self.closed.is_some() {
            d.closed = self.closed;
        }
        if let Some(v) = self.days {
            if m.page == Page::Days {
                land_rc(&mut d.days, &mut d.errs.days, v, |v| v);
            }
        }
        if self.home.is_some() {
            d.home = self.home.map(Rc::new);
        }
        if self.today.is_some() {
            d.today = self.today.map(Rc::new);
        }
        if m.page == Page::Review {
            if let Some(v) = self.reviews {
                land(&mut d.reviews, &mut d.errs.review, v, |v| v);
            }
            if let Some((id, v)) = self.review {
                if m.review.id.is_none_or(|want| want == id) {
                    land(&mut d.review, &mut d.errs.review, v, |v| v);
                }
            }
        }
        if let Some(Ok(f)) = self.task_page.filter(|_| m.page == Page::Tasks) {
            m.tasks.held.land("tasks", f);
        }
        if let Some((id, v)) = self.session {
            if m.sessions.selected.as_deref() == Some(id.as_str()) {
                // `sessionsLoads`: a failed fetch for another terminal than the one shown clears it.
                if v.is_err() && d.session.as_ref().and_then(|x| x["id"].as_str()) != Some(id.as_str()) {
                    d.session = None;
                }
                land(&mut d.session, &mut d.errs.session, v, |v| v);
            }
        }
    }
}

/// The board may wrap an issue: `b.issue || b`.
fn unwrap_issue(v: Value) -> Value {
    if v["issue"].is_object() { v["issue"].clone() } else { v }
}

/// The contract allows `{"goal": {...}, ...rest}`; flatten it.
fn merge_goal(v: Value) -> Value {
    match v {
        Value::Object(mut o) => {
            if let Some(Value::Object(g)) = o.remove("goal") {
                for (k, x) in g {
                    o.entry(k).or_insert(x);
                }
            }
            Value::Object(o)
        }
        v => v,
    }
}

// ------------------------------------------------------------------ actions

actions!(taskboard, [CloseOverlay, GoBoard, GoBacklog, GoSessions, GoDays, GoTasks, GoGoals, Refresh, CopyRef, CopySelection, Search, NavBack, NavForward]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", CloseOverlay, Some("MainWindow")),
        // In the sidebar's order.
        KeyBinding::new("cmd-1", GoBoard, Some("MainWindow")),
        KeyBinding::new("cmd-2", GoSessions, Some("MainWindow")),
        KeyBinding::new("cmd-3", GoGoals, Some("MainWindow")),
        KeyBinding::new("cmd-4", GoTasks, Some("MainWindow")),
        KeyBinding::new("cmd-5", GoBacklog, Some("MainWindow")),
        KeyBinding::new("cmd-6", GoDays, Some("MainWindow")),
        KeyBinding::new("cmd-k", Search, Some("MainWindow")),
        KeyBinding::new("cmd-r", Refresh, Some("MainWindow")),
        KeyBinding::new("cmd-shift-c", CopyRef, Some("MainWindow")),
        KeyBinding::new("cmd-c", CopySelection, Some("MainWindow")),
        KeyBinding::new("cmd-left", NavBack, Some("MainWindow")),
        KeyBinding::new("cmd-right", NavForward, Some("MainWindow")),
        // In a text field ⌘← and ⌘→ move the cursor to the line's ends instead.
        KeyBinding::new("cmd-left", NoAction, Some(ui::text_input::CTX)),
        KeyBinding::new("cmd-right", NoAction, Some(ui::text_input::CTX)),
        // A text field copies its own selection.
        KeyBinding::new("cmd-c", NoAction, Some(ui::text_input::CTX)),
        // ⇧↩ starts a new line in a multi-line field (New idea's detail, answers); ↩ stays the parent's.
        KeyBinding::new("shift-enter", ui::text_input::Newline, Some(ui::text_input::CTX_WRAP)),
    ]);
}

// ------------------------------------------------------------------ render

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.drawn = Instant::now();
        let t = cx.global::<Theme>().clone();
        window.set_window_title(&self.title());

        let page = match self.page.clone() {
            Page::Board => ui::home::render(self, window, cx),
            Page::Tasks => ui::tasks::render(self, window, cx),
            Page::Goals => ui::goals_page::render(self, window, cx),
            Page::Projects => ui::projects::render(self, window, cx),
            Page::Goal(_) => ui::goal::render(self, cx),
            Page::Backlog => ui::backlog::render(self, window, cx),
            Page::Sessions => ui::sessions::render(self, window, cx),
            Page::Days => ui::days::render(self, window, cx),
            Page::Review => ui::review::render(self, window, cx),
            Page::Releases | Page::ReleasesOf(_) => ui::releases::render(self, window, cx),
            Page::Ideas => ui::ideas::render(self, window, cx),
            Page::Idea(_) => ui::idea::render(self, cx),
        };
        let (task_panel, panel) = match self.panel.clone() {
            Some(Panel::Task { .. }) => (Some(ui::task_panel::render(self, window, cx)), None),
            Some(Panel::Issue { .. }) => (None, Some(ui::issue_panel::render(self, window, cx))),
            None => (None, None),
        };
        let modal = self.modal.is_some().then(|| ui::modals::render(self, window, cx));
        let hours_menu = ui::hours::render_menu(self, window, cx);

        div()
            .id("main")
            .key_context("MainWindow")
            .track_focus(&self.focus)
            .on_action(cx.listener(|m, _: &CloseOverlay, window, cx| m.escape(window, cx)))
            .on_action(cx.listener(|m, _: &GoBoard, _, cx| m.go(Page::Board, cx)))
            .on_action(cx.listener(|m, _: &GoBacklog, _, cx| m.go(Page::Backlog, cx)))
            .on_action(cx.listener(|m, _: &GoSessions, _, cx| m.go(Page::Sessions, cx)))
            .on_action(cx.listener(|m, _: &GoDays, _, cx| m.go(Page::Days, cx)))
            .on_action(cx.listener(|m, _: &GoTasks, _, cx| m.go(Page::Tasks, cx)))
            .on_action(cx.listener(|m, _: &GoGoals, _, cx| m.go(Page::Goals, cx)))
            .on_action(cx.listener(|m, _: &Refresh, _, cx| m.refresh(cx)))
            .on_action(cx.listener(|m, _: &CopyRef, _, cx| copy_ref(m, cx)))
            .on_action(|_: &CopySelection, window, cx| ui::select::copy(window, cx))
            .on_action(cx.listener(|m, _: &Search, window, cx| ui::search::open(m, window, cx)))
            .on_action(cx.listener(|m, _: &NavBack, _, cx| m.go_back(cx)))
            .on_action(cx.listener(|m, _: &NavForward, _, cx| m.go_forward(cx)))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(t.bg)
            .text_color(t.text)
            .font_family(t.ui_font.clone())
            .text_size(px(14.))
            // Drawn first so a press that a control stops (a button, a menu) doesn't start a selection.
            .child(gpui_kit::base::TextSelectionLayer)
            // The sidebar holds the pages, search, the goals in flight and the board's health.
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(ui::sidebar::render(self, &t, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            // The window's title bar over the page: it moves the window.
                            .child(div().flex_none().h(px(12.)).on_mouse_down(MouseButton::Left, crate::settings::drag_window))
                            .children(banner(self, &t, cx))
                            .child(div().flex().flex_1().min_h_0().child(page)),
                    )
                    // The task panel floats over the page's right edge; no scrim, so the page stays usable.
                    .children(task_panel.map(|p| {
                        div().absolute().top_0().right_0().bottom_0().w(px(ui::task_panel::WIDTH)).max_w(relative(0.6)).child(p)
                    })),
            )
            .children(panel)
            .children(modal)
            .children(hours_menu)
            .children(toasts(self, &t, cx))
            .when(self.menu.is_some(), |d| {
                // A click anywhere outside an open menu closes it.
                d.child(deferred(div().id("menu-dismiss").absolute().top_0().left_0().size_full().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|m, _, window, cx| ui::issue_panel::close_picker(m, window, cx)),
                )).with_priority(2))
            })
    }
}

// ------------------------------------------------------------------ chrome view models

/// `renderAll`'s title: the page's name, with "(n) " in front while tasks need you.
pub fn page_title(page: &Page, needs: i64) -> String {
    let base = match page {
        Page::Goal(_) | Page::Goals => "Goals",
        Page::Projects => "Projects",
        Page::Backlog => "Backlog",
        Page::Sessions => "Sessions",
        Page::Days => "Days",
        Page::Review => "Review",
        Page::Releases | Page::ReleasesOf(_) => "Releases",
        Page::Ideas | Page::Idea(_) => "Ideas",
        Page::Tasks => "Tasks",
        Page::Board => "Task board",
    };
    if needs > 0 { format!("({needs}) {base}") } else { base.to_string() }
}

/// One banner line (`renderBanner`) or alerts-dialog row: what it says and its buttons.
#[derive(Clone, Debug, PartialEq)]
pub struct BannerRow {
    /// "down" (the board isn't answering), "urgent" (an urgent alert, red, in a row of its own) or "alert".
    pub kind: &'static str,
    pub text: String,
    /// The small "5m ago" after the text.
    pub ago: Option<String>,
    /// (action, label): `alerts-open` Show all, `alert-open` Open T4, `alert-dismiss` Dismiss.
    pub buttons: Vec<(&'static str, String)>,
}

/// `alertStays`: a PR waiting on your review stays until you review it, and an urgent alert until it clears (the board refuses a dismiss).
pub fn alert_stays(a: &Value) -> bool {
    a["review"] == true || a["urgent"] == true
}

fn alert_buttons(a: &Value) -> Vec<(&'static str, String)> {
    let mut b = Vec::new();
    if let Some(r) = fmt::opt_s(a, "task").or(fmt::opt_s(a, "goal")) {
        b.push(("alert-open", format!("Open {r}")));
    }
    if !alert_stays(a) {
        b.push(("alert-dismiss", "Dismiss".into()));
    }
    b
}

fn alert_kind(a: &Value) -> &'static str {
    if a["urgent"] == true { "urgent" } else { "alert" }
}

/// What the alert says: its title and detail line when it has them, else its text.
pub fn alert_says(a: &Value) -> String {
    let (title, detail) = (s(a, "title"), s(a, "detail"));
    match (title.is_empty(), detail.is_empty()) {
        (true, _) => s(a, "text").to_string(),
        (false, true) => title.to_string(),
        (false, false) => format!("{title}. {detail}"),
    }
}

/// The alert's fixes as (label, command), one button each.
pub fn alert_actions(a: &Value) -> Vec<(String, String)> {
    arr(a, "actions").iter().filter(|x| !s(x, "label").is_empty()).map(|x| (s(x, "label").to_string(), s(x, "command").to_string())).collect()
}

fn alert_row_view(a: &Value) -> BannerRow {
    BannerRow { kind: alert_kind(a), text: alert_says(a), ago: Some(fmt::ago(s(a, "at"))), buttons: alert_buttons(a) }
}

/// An alert's fix button: the board opens a Claude terminal in Midna with the request typed in for
/// the owner to send. When it can't, the command goes on the clipboard instead.
pub fn run_alert_action(m: &mut MainWindow, id: &str, n: usize, label: &str, command: &str, cx: &mut Context<MainWindow>) {
    let (label, command) = (label.to_string(), command.to_string());
    m.post_or(
        format!("alerts/{id}/act"),
        json!({"n": n}),
        cx,
        move |m, _, cx| m.toast(format!("{label}: opened Claude in Midna"), false, cx),
        move |m, e, cx| {
            if command.is_empty() {
                return m.toast(e.message, true, cx);
            }
            cx.write_to_clipboard(ClipboardItem::new_string(command.clone()));
            m.toast(format!("Copied {command}"), false, cx);
        },
    );
}

/// `renderBanner`: the "isn't answering" line, a line for each urgent alert, then either one line per other
/// alert or, with more than one, a single "N tasks need your attention." line with Show all.
pub fn banner_view(down: Option<&str>, alerts: &[Value]) -> Vec<BannerRow> {
    let mut out = Vec::new();
    if let Some(e) = down.filter(|e| !e.is_empty()) {
        out.push(BannerRow { kind: "down", text: format!("{e} Trying again every few seconds."), ago: None, buttons: Vec::new() });
    }
    let (urgent, alerts): (Vec<Value>, Vec<Value>) = alerts.iter().cloned().partition(|a| a["urgent"] == true);
    for a in &urgent {
        out.push(BannerRow { kind: "urgent", ..alert_row_view(a) });
    }
    if alerts.len() > 1 {
        out.push(BannerRow {
            kind: "alert",
            text: format!("{} your attention.", fmt::plural(alerts.len() as i64, "task needs", "tasks need")),
            ago: Some(fmt::ago(s(alerts.last().unwrap_or(&Value::Null), "at"))),
            buttons: vec![("alerts-open", "Show all".into())],
        });
    } else {
        for a in &alerts {
            out.push(BannerRow { kind: "alert", ..alert_row_view(a) });
        }
    }
    out
}

/// `alertsDialogHtml`: (title, rows); its foot is "Dismiss all" while any alert can be dismissed. (For the dialog in modals.rs.)
pub fn alerts_dialog_view(alerts: &[Value]) -> (String, Vec<BannerRow>) {
    let title = format!("{} your attention", fmt::plural(alerts.len() as i64, "task needs", "tasks need"));
    let rows = alerts.iter().map(alert_row_view).collect();
    (title, rows)
}

/// A status-bar pill: its class (`up`, `down`, `hours`, `hours closed`), label and tooltip.
#[derive(Clone, Debug, PartialEq)]
pub struct Pill {
    pub cls: &'static str,
    pub label: String,
    pub title: String,
}

/// `connPill`.
pub fn conn_pill(m: &Value) -> Pill {
    let heard = fmt::opt_s(m, "seen_at").map(|x| format!("Last heard from {}", fmt::ago(x))).unwrap_or_else(|| "Not heard from yet".into());
    if b(m, "up") {
        Pill { cls: "up", label: "Connected to Midna".into(), title: heard }
    } else {
        Pill { cls: "down", label: "Midna isn’t running".into(), title: format!("{heard}. Starting, messaging and closing terminals wait until it’s back.") }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageWin {
    /// ok, warm (75 %+), hot (90 %+).
    pub level: &'static str,
    pub label: String,
    /// Bar width in percent (0–100).
    pub width: f64,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageView {
    pub stale: bool,
    pub title: String,
    pub windows: Vec<UsageWin>,
}

/// `usageLevel`.
pub fn usage_level(pct: f64) -> &'static str {
    if pct >= 90. {
        "hot"
    } else if pct >= 75. {
        "warm"
    } else {
        "ok"
    }
}

/// A JSON number as JS prints it (`35`, `35.5`).
fn js_num(v: &Value) -> String {
    match v.as_f64() {
        Some(f) if f.fract() == 0. && f.abs() < 1e15 => format!("{}", f as i64),
        Some(f) => format!("{f}"),
        None => v.as_str().unwrap_or("").to_string(),
    }
}

/// `usagePill`: None when there's no reading (or no windows).
pub fn usage_view(u: &Value) -> Option<UsageView> {
    let wins = arr(u, "windows");
    if !u.is_object() || wins.is_empty() {
        return None;
    }
    // The web compares with the reader's own clock (not the board's).
    let stale = fmt::parse(s(u, "seen_at")).is_some_and(|x| (fmt::device_now() - x).num_milliseconds() > 3_600_000);
    let mut title: Vec<String> = wins
        .iter()
        .map(|w| {
            let name = match s(w, "key") {
                "five_hour" => "5-hour".to_string(),
                "seven_day" => "7-day".to_string(),
                _ => s(w, "label").to_string(),
            };
            let resets = fmt::opt_s(w, "resets_at").map(|r| format!(", resets {}", fmt::day_clock(r))).unwrap_or_default();
            format!("{name} limit: {}% used{resets}", js_num(&w["pct"]))
        })
        .collect();
    if stale {
        title.push(format!("Last read {}", fmt::ago(s(u, "seen_at"))));
    }
    let windows = wins
        .iter()
        .map(|w| {
            let pct = w["pct"].as_f64().unwrap_or(0.);
            UsageWin { level: usage_level(pct), label: s(w, "label").to_string(), width: pct.clamp(0., 100.), value: format!("{}%", js_num(&w["pct"])) }
        })
        .collect();
    Some(UsageView { stale, title: title.join("\n"), windows })
}

// ------------------------------------------------------------------ chrome drawing

/// `.banner`: 7px 40px, 6/10 gaps, 13px, a bottom line.
fn bar(bg: Hsla, fg: Hsla, line: Hsla) -> Div {
    div().flex().flex_none().items_center().flex_wrap().gap_x(px(10.)).gap_y(px(6.)).px(px(40.)).py(px(7.)).text_size(px(13.)).bg(bg).text_color(fg).border_b_1().border_color(line)
}

/// `.btn.sm` in the banner: 32px tall, 12.5px, 0 10px, radius 7. `soft` is accent on accent-soft
/// (semibold); otherwise `.ghost` (muted, no fill).
fn banner_btn(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>, soft: bool) -> Stateful<Div> {
    let text = t.text;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(32.))
        .px(px(10.))
        .rounded(px(7.))
        .text_size(px(12.5))
        .whitespace_nowrap()
        .cursor_pointer()
        .when(soft, |d| d.bg(t.accent_soft).text_color(t.accent).font_weight(FontWeight::SEMIBOLD).hover(|s| s.opacity(0.9)))
        .when(!soft, |d| d.text_color(t.muted).hover(move |s| s.text_color(text)))
        .child(label.into())
}

/// The banner lines: the board down, the login item (native), master and PR-build problems. The
/// alerts are the home page's Needs you list.
fn banner(m: &MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Vec<AnyElement> {
    let mut out = Vec::new();
    let rows = banner_view(m.down.as_deref(), &[]);
    for r in rows.iter().filter(|r| r.kind == "down") {
        let text = r.text.clone();
        out.push(
            bar(t.down_soft, t.down, t.down_line)
                .child(kit::dot(t.down, 8.))
                .child(div().flex_1().min_w_0().child(r.text.clone()))
                .child(banner_btn(t, "down-copy", "Copy", false).on_click(cx.listener(move |m, _, _, cx| copy_text(m, &text, cx))))
                .into_any_element(),
        );
    }
    match crate::install::status() {
        crate::install::LoginItem::RequiresApproval => out.push(
            bar(t.warn_soft, t.warn_text, t.warn_line)
                .child(kit::dot(t.warn, 8.))
                .child(div().flex_1().min_w_0().child("The board runs in the background as a login item. Switch on Taskboard in System Settings ▸ Login Items."))
                .child(kit::btn_small(t, "open-login-items", "Open Login Items").on_click(|_, _, _| crate::install::open_login_items()))
                .into_any_element(),
        ),
        crate::install::LoginItem::Blocked => out.push(
            bar(t.warn_soft, t.warn_text, t.warn_line)
                .child(kit::dot(t.warn, 8.))
                .child(div().flex_1().min_w_0().child("Another board holds the port, so this one didn't start its background service or link tb. Stop the other board, then open Taskboard again."))
                .into_any_element(),
        ),
        crate::install::LoginItem::Failed(e) => out.push(
            bar(t.warn_soft, t.warn_text, t.warn_line)
                .child(kit::dot(t.warn, 8.))
                .child(div().flex_1().min_w_0().child(format!("The board's background service couldn't be registered: {e}")))
                .into_any_element(),
        ),
        _ => {}
    }
    for p in crate::install::in_the_way() {
        out.push(bar(t.warn_soft, t.warn_text, t.warn_line).child(kit::dot(t.warn, 8.)).child(div().flex_1().min_w_0().child(p)).into_any_element());
    }
    out.extend(ui::prwatch::banner(m, t, cx));
    out
}

/// The alert's text, then its age in `ago` (the muted color, or the row's own on a solid red one).
fn alert_text(r: &BannerRow, ago: Hsla) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_wrap()
        .items_baseline()
        .gap(px(4.))
        .child(r.text.clone())
        .children(r.ago.clone().map(|a| div().ml(px(4.)).text_size(px(10.8)).text_color(ago).child(a)))
}

/// One alert: its text and age, "Open T12" and Dismiss. Also used by the alerts dialog.
pub fn alert_row(t: &Theme, a: &Value, cx: &mut Context<MainWindow>) -> Div {
    let id = s(a, "id").to_string();
    let r = alert_row_view(a);
    let (task, goal) = (fmt::opt_s(a, "task").map(str::to_string), fmt::opt_s(a, "goal").map(str::to_string));
    let fixes = alert_actions(a).into_iter().enumerate().map(|(n, (label, command))| {
        let aid = id.clone();
        banner_btn(t, SharedString::from(format!("alert-act-{id}-{n}")), label.clone(), true)
            .when(a["urgent"] == true, |d| d.text_color(t.on_accent))
            .on_click(cx.listener(move |m, _, _, cx| run_alert_action(m, &aid, n, &label, &command, cx)))
    });
    let fixes: Vec<_> = fixes.collect();
    // An urgent alert is solid red; the others are red on the soft red.
    let urgent = r.kind == "urgent";
    let row = if urgent { bar(t.down, t.on_accent, t.down).font_weight(FontWeight::SEMIBOLD) } else { bar(t.down_soft, t.down, t.down_line) };
    row.child(kit::dot(if urgent { t.on_accent } else { t.down }, 8.))
        .child(alert_text(&r, if urgent { t.on_accent } else { t.muted }))
        .children(r.buttons.iter().filter(|(act, _)| *act == "alert-open").map(|(_, label)| {
            let (task, goal) = (task.clone(), goal.clone());
            banner_btn(t, SharedString::from(format!("alert-open-{id}")), label.clone(), true).on_click(cx.listener(move |m, _, window, cx| {
                open_alert(m, task.as_deref(), goal.as_deref(), window, cx);
            }))
        }))
        .children(fixes)
        .child({
            let text = r.text.clone();
            banner_btn(t, SharedString::from(format!("alert-copy-{id}")), "Copy", false)
                .when(urgent, |d| d.text_color(t.on_accent))
                .on_click(cx.listener(move |m, _, _, cx| copy_text(m, &text, cx)))
        })
        .when(r.buttons.iter().any(|(act, _)| *act == "alert-dismiss"), |d| {
            d.child(banner_btn(t, SharedString::from(format!("alert-dismiss-{id}")), "Dismiss", false).on_click(cx.listener(move |m, _, _, cx| dismiss_alert(m, &id, cx))))
        })
}

/// The banner's Copy: its text on the clipboard, and a toast.
pub fn copy_text(m: &mut MainWindow, text: &str, cx: &mut Context<MainWindow>) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
    m.toast("Copied".to_string(), false, cx);
}

/// ⌘⇧C: what's open on the clipboard, to hand to a session. The side panel's task or issue wins
/// over the goal page under it; elsewhere there's nothing to copy.
pub fn open_ref(m: &MainWindow) -> Option<&str> {
    match (&m.panel, &m.page) {
        (Some(Panel::Task { r, .. } | Panel::Issue { r }), _) | (None, Page::Goal(r)) => Some(r),
        _ => None,
    }
}

fn copy_ref(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let Some(r) = open_ref(m).map(str::to_string) else { return };
    cx.write_to_clipboard(ClipboardItem::new_string(r.clone()));
    m.toast(format!("Copied {r}"), false, cx);
}

/// `alert-open`: close the alerts dialog, then the task (or else the goal).
pub fn open_alert(m: &mut MainWindow, task: Option<&str>, goal: Option<&str>, window: &mut Window, cx: &mut Context<MainWindow>) {
    if matches!(m.modal, Some(ui::modals::Modal::Alerts)) {
        m.set_modal(None, window, cx);
    }
    if let Some(t) = task {
        m.open_task(t.to_string(), cx);
    } else if let Some(g) = goal {
        m.go(Page::Goal(g.to_string()), cx);
    }
}

/// `alert-dismiss`: `POST /alerts/:id/dismiss {}` (no toast).
pub fn dismiss_alert(m: &mut MainWindow, id: &str, cx: &mut Context<MainWindow>) {
    m.post(format!("alerts/{id}/dismiss"), json!({}), cx, |_, _, _| {});
}

/// `alerts-dismiss-all`: dismiss every current alert; the dialog closes once none are left.
/// (For the dialog in modals.rs.)
pub fn dismiss_all_alerts(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let ids: Vec<String> = arr(m.state(), "alerts").iter().filter(|a| !alert_stays(a)).map(|a| s(a, "id").to_string()).collect();
    for id in ids {
        dismiss_alert(m, &id, cx);
    }
}

/// `.conn`: 12.5px, 3px 10px, fully rounded, 6px gaps (line height 1.5).
pub fn pill_shell(t: &Theme, id: &'static str, fg: Hsla, bg: Hsla) -> Stateful<Div> {
    let _ = t;
    div().id(id).flex().flex_none().items_center().gap(px(6.)).py(px(3.)).px(px(10.)).rounded_full().text_size(px(12.5)).line_height(px(18.75)).whitespace_nowrap().text_color(fg).bg(bg)
}

/// Amber "GitHub: sign in again" when an account lacks scopes Taskboard needs (or stopped working);
/// opens Settings. `state.accounts` is the daemon's `accounts::attention`.
pub(crate) fn accounts_pill(st: &Value, t: &Theme) -> Option<AnyElement> {
    let list = st.get("accounts")?.as_array().filter(|l| !l.is_empty())?;
    let label = match list.as_slice() {
        [one] => format!("{}: {}", one["label"].as_str().unwrap_or("Account"), if one["reauth"] == true { "sign in again" } else { "check sign-in" }),
        many => format!("{} accounts need you", many.len()),
    };
    let tip = list.iter().map(|a| format!("{}: {}", a["label"].as_str().unwrap_or(""), a["reason"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n");
    Some(
        pill_shell(t, "accounts-pill", t.warn_fg, t.warn_soft)
            .cursor_pointer()
            .child(kit::dot(t.warn, 7.))
            .child(label)
            .tooltip(kit::tip(tip))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::OpenSettings), cx))
            .into_any_element(),
    )
}

/// "Taskboard 1.2.0 is ready" once a newer release is downloaded and checked; opens Settings › System,
/// where Restart to update installs it.
pub(crate) fn update_pill(t: &Theme) -> Option<AnyElement> {
    let crate::updater::State::Ready { version, .. } = crate::updater::state() else { return None };
    Some(
        pill_shell(t, "update-pill", t.accent, t.accent_soft)
            .cursor_pointer()
            .child(kit::dot(t.accent, 7.))
            .child(format!("Taskboard {version} is ready"))
            .tooltip(kit::tip(format!("A new release, downloaded and checked. Restart to update from {} to {version}.", crate::updater::VERSION)))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::OpenSystemSettings), cx))
            .into_any_element(),
    )
}

/// "Install hooks", "● Hooks" or an amber "Reinstall hooks", as in Midna's status bar.
pub(crate) fn hooks_item(m: &MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    use crate::hooks::Hooks;
    let can_install = crate::hooks::target().is_some_and(|(_, can)| can);
    let text = t.text;
    let base = div().id("hooks").flex().items_center().gap(px(5.)).text_size(px(12.5)).whitespace_nowrap().text_color(t.muted);
    let clickable = |d: Stateful<Div>| -> Stateful<Div> {
        if can_install {
            d.cursor_pointer().hover(move |s| s.text_color(text)).on_click(cx.listener(|m, _, _, cx| m.install_hooks(cx)))
        } else {
            d
        }
    };
    let by_app = if can_install { "" } else { "\nInstall them from Taskboard.app." };
    let el = if m.hooks_busy {
        base.child("Installing hooks…")
    } else {
        match &m.hooks {
            Hooks::Hidden | Hooks::Checking => return None,
            Hooks::Current => base
                .child(kit::dot(t.up, 7.))
                .child("Hooks")
                .tooltip(kit::tip("The task-board plugin is installed in Claude Code, so sessions in Midna terminals report to the board.")),
            Hooks::NotInstalled => clickable(base.text_color(t.accent).child("Install hooks")).tooltip(kit::tip(format!(
                "Install the task-board plugin in Claude Code so sessions in Midna terminals report to the board.{by_app}"
            ))),
            Hooks::NeedsReinstall(why) => clickable(base.text_color(t.warn_fg).child(kit::dot(t.warn, 7.)).child("Reinstall hooks"))
                .tooltip(kit::tip(format!("The hooks are out of date, so sessions may not report to the board.\n{why}{by_app}"))),
        }
    };
    Some(el.into_any_element())
}

/// `.conn.usage`: per window `.u-k` label, a 34×5 bar and the value; warm/hot recolour both.
pub(crate) fn usage_pill(t: &Theme, u: &UsageView) -> AnyElement {
    let mut row = pill_shell(t, "usage-pill", t.muted, t.seg)
        .gap(px(0.))
        .px(px(4.))
        .border_1()
        .border_color(gpui_kit::transparent_black())
        .when(u.stale, |d| d.opacity(0.6))
        .tooltip(kit::tip(u.title.clone()));
    for (n, w) in u.windows.iter().enumerate() {
        let (bar_c, val_c) = match w.level {
            "hot" => (t.down, t.down),
            "warm" => (t.warn, t.warn_fg),
            _ => (t.up, t.text_2),
        };
        row = row.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(7.))
                .when(n > 0, |d| d.border_l_1().border_color(t.border_2))
                .child(div().text_size(px(11.5)).font_weight(FontWeight::MEDIUM).text_color(t.faint).child(w.label.clone()))
                .child(div().w(px(34.)).h(px(5.)).rounded_full().overflow_hidden().bg(t.border_2).child(div().h_full().rounded_full().bg(bar_c).w(relative((w.width / 100.) as f32))))
                .child(div().min_w(px(22.)).flex().justify_end().text_color(val_c).when(w.level == "hot", |d| d.font_weight(FontWeight::SEMIBOLD)).child(w.value.clone())),
        );
    }
    row.into_any_element()
}

/// `.toast`: centred 24px above the bottom, 9px 16px, radius 9, 13px, `--shadow-modal`.
fn toasts(m: &MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    if m.toasts.is_empty() {
        return None;
    }
    let mut col = div().absolute().bottom(px(24.)).left_0().right_0().px(px(16.)).flex().flex_col().items_center().gap(px(6.));
    // (One at a time: `toast` replaces the last.)
    for (i, x) in m.toasts.iter().enumerate() {
        col = col.child(
            div()
                .id(("toast", i))
                .px(px(16.))
                .py(px(9.))
                .rounded(px(9.))
                .shadow(ui::kit::shadow_modal(t))
                .text_size(px(13.))
                .bg(if x.err { t.down } else { t.text })
                .text_color(if x.err { gpui_kit::white() } else { t.bg })
                .flex()
                .items_center()
                .gap(px(14.))
                .child(x.text.clone())
                .children(x.undo.clone().map(|path| {
                    let at = x.at;
                    div()
                        .id("toast-undo")
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.accent_soft)
                        .cursor_pointer()
                        .child("Undo")
                        .on_click(cx.listener(move |m, _, _, cx| {
                            m.toasts.retain(|t| t.at != at);
                            m.post(path.clone(), serde_json::json!({}), cx, |_, _, _| {});
                        }))
                })),
        );
    }
    Some(deferred(col).with_priority(4).into_any_element())
}

#[cfg(test)]
mod tests {
    use super::{MainWindow, Page, Panel, TaskTab, alerts_dialog_view, banner_view, conn_pill, dismiss_alert, dismiss_all_alerts, page_title, usage_view};
    use crate::fmt::arr;
    use crate::parity::{golden, settle, window};
    use crate::ui;
    use serde_json::{Value, json};

    fn rows_json(rows: &[super::BannerRow]) -> Value {
        json!(rows.iter().map(|r| json!({"kind": r.kind, "text": r.text, "ago": r.ago,
            "buttons": r.buttons.iter().map(|(a, l)| format!("{a}:{l}")).collect::<Vec<_>>()})).collect::<Vec<_>>())
    }

    #[::core::prelude::v1::test]
    fn banner_and_alerts_dialog_match_web() {
        let g = golden("chrome");
        g.only("banner").check(|i| rows_json(&banner_view(i["down"].as_str(), arr(i, "alerts"))));
        g.only("alertsDialog").check(|i| {
            let (title, rows) = alerts_dialog_view(arr(i, "alerts"));
            let rows: Vec<Value> = rows.iter().map(|r| json!({"text": r.text, "ago": r.ago,
                "buttons": r.buttons.iter().map(|(a, l)| format!("{a}:{l}")).collect::<Vec<_>>()})).collect();
            let foot: Vec<&str> = if arr(i, "alerts").iter().any(|a| !super::alert_stays(a)) { vec!["alerts-dismiss-all"] } else { vec![] };
            json!({"title": title, "rows": rows, "foot": foot})
        });
    }

    #[::core::prelude::v1::test]
    fn urgent_alerts_get_rows_of_their_own_first() {
        let a = |id: &str, urgent: bool| json!({"id": id, "text": format!("Alert {id}"), "at": "2026-10-08T10:00:00Z", "urgent": urgent, "task": "T3"});
        let rows = banner_view(None, &[a("n1", false), a("u1", true), a("n2", false), a("u2", true)]);
        let kinds: Vec<&str> = rows.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, vec!["urgent", "urgent", "alert"]);
        assert_eq!((rows[0].text.as_str(), rows[1].text.as_str()), ("Alert u1", "Alert u2"));
        assert!(rows[2].text.starts_with("2 tasks need"), "only the other alerts fold: {}", rows[2].text);
        let rows = banner_view(None, &[a("u1", true), a("n1", false)]);
        assert_eq!(rows.iter().map(|r| r.kind).collect::<Vec<_>>(), vec!["urgent", "alert"]);
        assert_eq!(rows[1].text, "Alert n1");
    }

    #[::core::prelude::v1::test]
    fn urgent_alerts_cant_be_dismissed() {
        let urgent = json!({"id": "a1", "text": "Main is red", "at": "2026-10-08T10:00:00Z", "urgent": true, "task": "T3"});
        let rows = banner_view(None, &[urgent.clone()]);
        assert_eq!(rows[0].kind, "urgent");
        assert!(rows[0].buttons.iter().all(|(a, _)| *a != "alert-dismiss"));
        assert!(rows[0].buttons.iter().any(|(a, _)| *a == "alert-open"));
        assert!(super::alert_stays(&urgent));
        assert!(!super::alert_stays(&json!({"id": "a2"})));
    }

    #[::core::prelude::v1::test]
    fn a_structured_alert_says_its_title_and_detail_and_offers_its_fixes() {
        let a = json!({"id": "a1", "text": "work-deck isn't set up to finish its work: a long sentence.", "at": "2026-10-08T10:00:00Z",
            "title": "work-deck PRs can't merge", "detail": "T117 is waiting for 2 approvals, and work-deck has no reviewers",
            "actions": [{"label": "Turn off review for work-deck", "prompt": "Turn off…", "command": "tb project set work-deck --review off"}, {"label": "", "command": "x"}]});
        let rows = banner_view(None, &[a.clone()]);
        assert_eq!(rows[0].text, "work-deck PRs can't merge. T117 is waiting for 2 approvals, and work-deck has no reviewers");
        assert_eq!(super::alert_actions(&a), vec![("Turn off review for work-deck".to_string(), "tb project set work-deck --review off".to_string())]);
        let plain = json!({"id": "a2", "text": "Alert a2", "at": "2026-10-08T10:00:00Z"});
        assert_eq!(banner_view(None, &[plain.clone()])[0].text, "Alert a2");
        assert!(super::alert_actions(&plain).is_empty());
    }

    #[::core::prelude::v1::test]
    fn status_bar_matches_web() {
        let g = golden("chrome");
        g.only("connPill").check(|i| {
            let p = conn_pill(&i["midna"]);
            json!({"cls": p.cls, "label": p.label, "title": p.title})
        });
        g.only("usagePill").check(|i| match usage_view(&i["usage"]) {
            None => Value::Null,
            Some(u) => json!({"stale": u.stale, "title": u.title, "windows": u.windows.iter().map(|w| json!({"level": w.level, "label": w.label,
                "width": if w.width.fract() == 0. { json!(w.width as i64) } else { json!(w.width) }, "value": w.value})).collect::<Vec<_>>()}),
        });
    }

    #[::core::prelude::v1::test]
    fn update_pill_shows_only_once_a_release_is_ready() {
        use crate::updater::{State, set_for_test};
        let t = crate::theme::Theme::new(crate::theme::ThemeMode::Light, "Helvetica Neue".into(), "Menlo".into());
        set_for_test(State::Downloading { version: "9.0.0".into() });
        assert!(super::update_pill(&t).is_none(), "not while it downloads");
        set_for_test(State::Ready { version: "9.0.0".into(), page: String::new() });
        assert!(super::update_pill(&t).is_some());
        set_for_test(State::Idle);
    }

    #[::core::prelude::v1::test]
    fn title_matches_web() {
        golden("chrome").only("title").check(|i| {
            let page = match i["page"].as_str().unwrap() {
                "goal" => Page::Goal("G1".into()),
                "backlog" => Page::Backlog,
                "sessions" => Page::Sessions,
                _ => Page::Board,
            };
            json!(page_title(&page, i["needs"].as_i64().unwrap()))
        });
    }

    #[gpui_kit::test]
    fn dismissing_alerts_posts_like_the_web(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            dismiss_alert(m, "a1", cx);
            if let Some(st) = m.data.state.as_mut().map(std::rc::Rc::make_mut) {
                st["alerts"] = json!([{"id": "x1"}, {"id": "x2"}]);
            }
            dismiss_all_alerts(m, cx);
        })
        .unwrap();
        settle(cx);
        let posts: Vec<(String, Value)> = rec.posts();
        assert_eq!(posts[0], ("alerts/a1/dismiss".to_string(), json!({})));
        assert!(posts.iter().any(|(p, b)| p == "alerts/x1/dismiss" && *b == json!({})));
        assert!(posts.iter().any(|(p, b)| p == "alerts/x2/dismiss" && *b == json!({})));
    }

    #[gpui_kit::test]
    fn the_task_panel_stays_open_across_pages(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, window, cx| {
            m.go(Page::Goals, cx);
            m.open_task("T1", cx);
            assert!(m.task_panel_open());
            m.go(Page::Board, cx);
            assert!(m.task_panel_open() && m.page == Page::Board, "another page keeps the panel beside it");
            if let Some(Panel::Task { tab, .. }) = m.panel.as_mut() {
                *tab = TaskTab::Log;
            }
            m.go(Page::Goal("G1".into()), cx);
            assert!(matches!(m.panel, Some(Panel::Task { tab: TaskTab::Log, .. })) && m.page == Page::Goal("G1".into()), "on the tab it was on");
            m.go_back(cx);
            assert!(matches!(m.panel, Some(Panel::Task { tab: TaskTab::Log, .. })), "⌘← keeps the tab too");
            m.go_forward(cx);
            m.escape(window, cx);
            assert!(m.panel.is_none() && m.page == Page::Goal("G1".into()), "Esc closes it and keeps the page");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn the_projects_page_loads_its_goals_details(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| m.go(Page::Projects, cx)).unwrap();
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, _| {
            let wanted = ui::projects::wanted(&m.projects, m.state(), &m.data.goals);
            assert!(!wanted.is_empty(), "the picked project has open goals");
            for r in &wanted {
                assert!(m.projects.details.contains_key(r), "{r} landed for the Projects page");
                assert!(!m.goals.details.contains_key(r), "{r} stays out of the Goals page's details");
            }
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn cmd_left_leaves_a_task_for_the_page_it_was_opened_from(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            let task = |m: &MainWindow| match &m.panel {
                Some(Panel::Task { r, .. }) => Some(r.clone()),
                _ => None,
            };
            m.go(Page::Goals, cx);
            m.go(Page::Goal("G1".into()), cx);
            m.open_task("T1", cx);
            m.open_task("T2", cx);
            m.go_back(cx);
            assert_eq!((m.page.clone(), task(m)), (Page::Goal("G1".into()), Some("T1".into())), "back to the task before");
            m.go_back(cx);
            assert_eq!((m.page.clone(), task(m)), (Page::Goal("G1".into()), None), "back to the goal, not the Goals page under a task");
            m.go_back(cx);
            assert_eq!((m.page.clone(), task(m)), (Page::Goals, None));
            m.go_forward(cx);
            m.go_forward(cx);
            assert_eq!((m.page.clone(), task(m)), (Page::Goal("G1".into()), Some("T1".into())), "forward reopens the task");
            m.close_panel(cx);
            assert_eq!(task(m), None);
            m.go_back(cx);
            assert_eq!((m.page.clone(), task(m)), (Page::Goal("G1".into()), Some("T1".into())), "a closed task is a step back");
            m.go(Page::Board, cx);
            m.go_back(cx);
            assert_eq!((m.page.clone(), task(m)), (Page::Goal("G1".into()), Some("T1".into())), "leaving a task by the sidebar comes back to it");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn cmd_left_and_right_walk_the_page_history(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            m.go_back(cx);
            assert!(m.page == Page::Board, "nothing to go back to");
            m.go(Page::Backlog, cx);
            m.go(Page::Goal("G2".into()), cx);
            m.go_back(cx);
            assert!(m.page == Page::Backlog);
            m.go_back(cx);
            assert!(m.page == Page::Board);
            m.go_forward(cx);
            assert!(m.page == Page::Backlog);
            m.go(Page::Days, cx);
            m.go_forward(cx);
            assert!(m.page == Page::Days, "a fresh visit drops the forward pages");
            m.go_back(cx);
            assert!(m.page == Page::Backlog);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn a_review_link_opens_the_review_and_marks_it_seen(cx: &mut gpui_kit::TestAppContext) {
        use crate::backend::Backend as _;
        let (w, rec) = window(cx);
        let id = rec.board().post("reviews", json!({})).unwrap()["id"].as_i64().unwrap();
        w.update(cx, |m: &mut MainWindow, _, cx| m.open_link(&format!("taskboard://review/{id}"), cx)).unwrap();
        settle(cx);
        {
            use gpui_kit::AppContext as _;
            cx.update_window(w.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
        }
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, _| {
            assert!(m.page == Page::Review);
            assert_eq!(m.review.id, Some(id));
            assert_eq!(m.data.review.as_ref().and_then(|r| r["id"].as_i64()), Some(id));
            assert!(m.data.review.as_ref().is_some_and(|r| !r["seen_at"].is_null()), "the seen answer replaces what's shown");
        })
        .unwrap();
        let seen = format!("reviews/{id}/seen");
        assert_eq!(rec.posts().iter().filter(|(p, _)| *p == seen).count(), 1, "opening it marks it seen once");
    }

    /// Review opened without a pick (the sidebar) shows the newest review.
    #[gpui_kit::test]
    fn the_review_page_opens_on_the_newest_review(cx: &mut gpui_kit::TestAppContext) {
        use crate::backend::Backend as _;
        let (w, rec) = window(cx);
        rec.board().post("reviews", json!({})).unwrap();
        rec.board().post("reviews", json!({})).unwrap();
        let newest = arr(&rec.board().get("reviews", &[]).unwrap(), "reviews")[0]["id"].as_i64();
        w.update(cx, |m: &mut MainWindow, _, cx| m.go(Page::Review, cx)).unwrap();
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, _| {
            assert_eq!(m.review.id, None);
            assert!(newest.is_some());
            assert_eq!(m.data.review.as_ref().and_then(|r| r["id"].as_i64()), newest);
        })
        .unwrap();
    }

    /// A poll while a refresh is still out leaves it be; once it has answered, the next poll refreshes.
    #[gpui_kit::test]
    fn a_poll_waits_for_the_refresh_thats_out(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            m.refresh(cx);
            let out = m.generation;
            m.poll(cx);
            assert_eq!(m.generation, out, "the refresh that's out isn't restarted");
        })
        .unwrap();
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            assert_eq!(m.answered, m.generation);
            let before = m.generation;
            m.poll(cx);
            assert_eq!(m.generation, before + 1);
        })
        .unwrap();
    }

    /// A poll that brings nothing new doesn't redraw; one that does, a refresh, or a poll after
    /// `AGES` without a redraw does.
    #[gpui_kit::test]
    fn a_poll_that_brings_nothing_new_skips_the_redraw(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        settle(cx);
        let e = w.update(cx, |_, _, cx| cx.entity()).unwrap();
        let drawn = std::rc::Rc::new(std::cell::Cell::new(0));
        let d2 = drawn.clone();
        cx.update(|cx| cx.observe(&e, move |_, _| d2.set(d2.get() + 1)).detach());
        let ask = |cx: &mut gpui_kit::TestAppContext, poll: bool| {
            drawn.set(0);
            w.update(cx, |m: &mut MainWindow, _, cx| if poll { m.poll(cx) } else { m.refresh(cx) }).unwrap();
            settle(cx);
            drawn.get()
        };
        assert_eq!(ask(cx, true), 0, "nothing new");
        assert!(ask(cx, false) > 0, "a refresh always redraws");
        rec.fail("state", 503, "down");
        assert!(ask(cx, true) > 0, "the board went down");
        assert_eq!(ask(cx, true), 0, "still down");
        rec.heal();
        assert!(ask(cx, true) > 0, "back up");
        w.update(cx, |m: &mut MainWindow, _, _| m.drawn -= super::AGES).unwrap();
        assert!(ask(cx, true) > 0, "ages are due");
    }

    /// Behind another app the poll slows to 15 s, off screen to 30 s; back in front it refreshes at once
    /// and goes back to every 3 s.
    #[gpui_kit::test]
    fn the_poll_slows_while_the_window_is_away(cx: &mut gpui_kit::TestAppContext) {
        use super::{POLL, POLL_AWAY, POLL_HIDDEN};
        let (w, _rec) = window(cx);
        let asked = |cx: &mut gpui_kit::TestAppContext| w.update(cx, |m: &mut MainWindow, _, _| m.generation).unwrap();
        let tick = |cx: &mut gpui_kit::TestAppContext, d: std::time::Duration| {
            cx.executor().advance_clock(d);
            settle(cx);
        };
        let vcx = &mut gpui_kit::VisualTestContext::from_window(w.into(), cx);
        vcx.update(|window, _| window.activate_window());
        settle(vcx);
        assert_eq!(w.update(vcx, |m: &mut MainWindow, _, _| m.every).unwrap(), POLL);
        let g = asked(vcx);
        tick(vcx, POLL);
        assert_eq!(asked(vcx), g + 1, "in front it polls every 3 s");

        vcx.deactivate_window();
        settle(vcx);
        let g = asked(vcx);
        tick(vcx, POLL * 2);
        assert_eq!(asked(vcx), g, "behind another app 3 s isn't enough");
        tick(vcx, POLL_AWAY - POLL * 2);
        assert_eq!(asked(vcx), g + 1, "it polls at 15 s");

        vcx.simulate_visibility_change(gpui_kit::WindowVisibility::Hidden);
        settle(vcx);
        let g = asked(vcx);
        tick(vcx, POLL_AWAY);
        assert_eq!(asked(vcx), g, "off screen 15 s isn't enough");
        tick(vcx, POLL_HIDDEN - POLL_AWAY);
        assert_eq!(asked(vcx), g + 1, "it polls at 30 s");

        let g = asked(vcx);
        vcx.simulate_visibility_change(gpui_kit::WindowVisibility::Visible);
        vcx.update(|window, _| window.activate_window());
        settle(vcx);
        assert!(asked(vcx) > g, "coming back refreshes at once");
        let g = asked(vcx);
        tick(vcx, POLL);
        assert_eq!(asked(vcx), g + 1, "and polls every 3 s again");
    }

    /// What waits on a change runs once the refresh after it lands, not before.
    #[gpui_kit::test]
    fn settle_waits_for_the_refresh_after_the_change(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        let ran = std::rc::Rc::new(std::cell::Cell::new(false));
        let r2 = ran.clone();
        w.update(cx, |m: &mut MainWindow, _, _| m.settle(std::time::Instant::now(), move |_, _| r2.set(true))).unwrap();
        w.update(cx, |m: &mut MainWindow, _, cx| m.settled(m.generation, cx)).unwrap();
        assert!(!ran.get(), "an answer asked before the change doesn't count");
        w.update(cx, |m: &mut MainWindow, _, cx| m.refresh(cx)).unwrap();
        settle(cx);
        assert!(ran.get());
    }

    /// A refresh out when a change is posted may predate it: its answer is dropped.
    #[gpui_kit::test]
    fn a_post_drops_the_refresh_already_out(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            m.refresh(cx);
            let out = m.generation;
            m.post("alerts/a1/dismiss", json!({}), cx, |_, _, _| {});
            assert!(m.generation > out, "the refresh that's out is now stale");
        })
        .unwrap();
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, _| assert_eq!(m.answered, m.generation, "the refresh after the post answers")).unwrap();
    }

    /// A refresh asked now whose answer lands only after `then` ran (a slow board), with the board
    /// and goals it read marked "slow".
    fn slow_refresh(m: &mut MainWindow, rec: &crate::parity::Recording, cx: &mut gpui_kit::Context<MainWindow>, then: impl FnOnce(&mut MainWindow, &mut gpui_kit::Context<MainWindow>)) {
        m.generation += 1;
        let out = m.generation;
        let asked = super::Wants::of(m, cx);
        let mut parts: Vec<_> = asked.clone().calls().into_iter().map(|(k, call)| (k, call(rec))).collect();
        for (_, got) in &mut parts {
            if let Some(Ok(s)) = got.state.as_mut() {
                s["marker"] = json!("slow");
            }
            if got.goals.is_some() {
                got.goals = Some(json!([{"ref": "G99", "title": "Slow"}]));
            }
            if let Some(h) = got.home.as_mut() {
                h["marker"] = json!("slow");
            }
        }
        then(m, cx);
        for (k, got) in parts {
            m.part(out, &asked, k, got, false, cx);
        }
        m.landed(out, &asked, false, cx);
    }

    /// Each call's answer swaps in as it lands: the Days page fills while `/state` is still out,
    /// and the refresh counts as answered only once all have.
    #[gpui_kit::test]
    fn a_page_answer_lands_before_the_board(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            m.page = Page::Days;
            m.generation += 1;
            let out = m.generation;
            let asked = super::Wants::of(m, cx);
            let before = m.data.state.clone();
            m.part(out, &asked, "days".into(), super::Got { days: Some(Ok(json!({"marker": "days"}))), ..Default::default() }, false, cx);
            assert_eq!(m.data.days.as_ref().map(|d| d["marker"].clone()), Some(json!("days")));
            assert!(std::rc::Rc::ptr_eq(m.data.state.as_ref().unwrap(), before.as_ref().unwrap()), "the board hasn't answered yet");
            assert!(m.answered < out, "the refresh is still out");
            m.landed(out, &asked, false, cx);
            assert_eq!(m.answered, out);
        })
        .unwrap();
    }

    /// An older refresh's answer that lands after a newer one's doesn't put the old one back.
    #[gpui_kit::test]
    fn an_older_answer_landing_late_is_dropped(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            let home = |marker: &str| super::Got { home: Some(json!({"marker": marker})), ..Default::default() };
            m.generation += 1;
            let old = m.generation;
            let asked = super::Wants::of(m, cx);
            m.generation += 1;
            let new = m.generation;
            m.part(new, &asked, "home".into(), home("new"), false, cx);
            m.part(old, &asked, "home".into(), home("old"), false, cx);
            assert_eq!(m.data.home.as_ref().map(|h| h["marker"].clone()), Some(json!("new")));
        })
        .unwrap();
    }

    /// Opening a page while a slow refresh is out keeps the board and goals it read instead of
    /// waiting a whole round trip for the newer one; the old page's own answer is dropped.
    #[gpui_kit::test]
    fn a_page_change_keeps_the_board_from_the_refresh_out(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            slow_refresh(m, &rec, cx, |m, cx| m.go(Page::Tasks, cx));
            assert_eq!(m.state()["marker"], "slow", "the board it read lands");
            assert!(m.data.goals.iter().any(|g| g["ref"] == "G99"), "so do the goals");
            assert!(m.data.home.as_ref().is_none_or(|h| h["marker"] != "slow"), "the Board page's own answer doesn't");
        })
        .unwrap();
    }

    /// A slow refresh out when a change is posted may put back what the click changed: none of it lands.
    #[gpui_kit::test]
    fn a_post_drops_all_of_the_slow_refresh_out(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            slow_refresh(m, &rec, cx, |m, cx| m.post("alerts/a1/dismiss", json!({}), cx, |_, _, _| {}));
            assert_ne!(m.state()["marker"], "slow");
            assert!(!m.data.goals.iter().any(|g| g["ref"] == "G99"));
        })
        .unwrap();
    }

    /// An issue's panel fills from its own fetch rather than with the rest of a refresh (here
    /// `/state` fails, which holds the rest back).
    #[gpui_kit::test]
    fn an_issue_panel_does_not_wait_for_the_refresh(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        rec.fail("state", 503, "slow");
        w.update(cx, |m: &mut MainWindow, _, cx| {
            m.open_issue("B1", cx);
            assert_eq!(super::Wants::of(m, cx).issue, None, "the refresh leaves it to its own fetch");
        })
        .unwrap();
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            assert!(m.data.issue.as_ref().is_some_and(|b| b["title"].is_string()), "B1 loaded");
            assert_eq!(super::Wants::of(m, cx).issue.as_deref(), Some("B1"), "later refreshes keep it current");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn cmd_left_in_a_text_field_stays_on_the_page(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        let draw = |cx: &mut gpui_kit::TestAppContext| {
            use gpui_kit::AppContext as _;
            cx.update_window(w.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
            settle(cx);
        };
        w.update(cx, |m: &mut MainWindow, _, cx| m.go(Page::Sessions, cx)).unwrap();
        draw(cx);
        w.update(cx, |m: &mut MainWindow, window, cx| window.focus(&m.sessions.search.as_ref().unwrap().focus, cx)).unwrap();
        cx.simulate_keystrokes(w.into(), "cmd-left");
        settle(cx);
        w.update(cx, |m: &mut MainWindow, window, cx| {
            assert!(m.page == Page::Sessions, "the field keeps ⌘← for its cursor");
            window.focus(&m.focus, cx);
        })
        .unwrap();
        cx.simulate_keystrokes(w.into(), "cmd-left");
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, _| assert!(m.page == Page::Board)).unwrap();
    }

    #[gpui_kit::test]
    fn the_banner_copies_its_text(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| super::copy_text(m, "T4 didn't start: no folder", cx)).unwrap();
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("T4 didn't start: no folder"));
    }

    #[gpui_kit::test]
    fn cmd_shift_c_copies_the_open_ref(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            m.page = Page::Board;
            m.panel = None;
            assert_eq!(super::open_ref(m), None, "nothing open");
            m.page = Page::Goal("G3".into());
            assert_eq!(super::open_ref(m), Some("G3"));
            m.panel = Some(Panel::Task { r: "T12".into(), tab: TaskTab::Log });
            assert_eq!(super::open_ref(m), Some("T12"), "the task panel wins over its goal");
            super::copy_ref(m, cx);
        })
        .unwrap();
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("T12"));
    }

    #[gpui_kit::test]
    fn escape_follows_the_web_order(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, window, cx| {
            m.set_modal(Some(ui::modals::Modal::Alerts), window, cx);
            m.escape(window, cx);
            assert!(m.modal.is_none(), "the alerts dialog closes");
            // Either panel closes, on any page.
            m.panel = Some(Panel::Task { r: "T1".into(), tab: TaskTab::Overview });
            m.escape(window, cx);
            assert!(m.panel.is_none());
            m.page = Page::Goal("G1".into());
            m.panel = Some(Panel::Issue { r: "B1".into() });
            m.escape(window, cx);
            assert!(m.panel.is_none(), "on a goal page the issue closes too");
            m.panel = Some(Panel::Issue { r: "B1".into() });
            m.page = Page::Backlog;
            m.backlog.picked = vec!["B1".into(), "B2".into()];
            m.escape(window, cx);
            assert!(m.panel.is_none(), "on the Backlog page it closes, as on the board");
            assert_eq!(m.backlog.picked.len(), 2, "closing the panel keeps the checked issues");
            m.panel = Some(Panel::Task { r: "T1".into(), tab: TaskTab::Overview });
            m.escape(window, cx);
            assert!(m.panel.is_none() && m.backlog.picked.len() == 2, "a task panel too");
            m.escape(window, cx);
            assert!(m.backlog.picked.is_empty(), "then Esc unchecks them");
            // Then the board's filters (railClear), with the board focused.
            m.page = Page::Board;
            m.filters = crate::app::Filters { project: "webapp".into(), goal: "G1".into(), done: "7d".into() };
            m.board.keep = vec!["B1".into()];
            window.focus(&m.focus, cx);
            m.escape(window, cx);
            assert_eq!((m.filters.project.as_str(), m.filters.goal.as_str(), m.filters.done.as_str()), ("all", "all", "7d"));
            assert!(m.board.keep.is_empty());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn project_names_include_goal_projects(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, _, _| {
            std::rc::Rc::make_mut(&mut m.data.goals).push(json!({"id": 99, "ref": "G99", "name": "x", "project": "Zebra"}));
            std::rc::Rc::make_mut(&mut m.data.goals).push(json!({"id": 98, "ref": "G98", "name": "y", "project": "archived-only", "archived": true}));
            let names = m.project_names();
            assert!(names.contains(&"Zebra".to_string()), "{names:?}");
            assert!(!names.contains(&"archived-only".to_string()));
            let mut sorted = names.clone();
            sorted.sort_by(|a, b| ui::modals::locale_cmp(a, b));
            assert_eq!(names, sorted);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn one_toast_at_a_time(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            m.toast("first", false, cx);
            m.toast("second", true, cx);
            assert_eq!(m.toasts.len(), 1);
            assert_eq!(m.toasts[0].text, "second");
            assert!(m.toasts[0].err);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn goals_link_opens_the_goals_page(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            m.open_goals(cx);
            assert_eq!(m.page, Page::Goals);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn idea_links_open_the_ideas_pages_and_another_idea_starts_empty(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| m.open_link("ideas", cx)).unwrap();
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            assert_eq!(m.page, Page::Ideas);
            assert_eq!(m.ideas.held.rows.len(), 6);
            m.open_link("idea/i1", cx);
        })
        .unwrap();
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            assert_eq!(m.page, Page::Idea("I1".into()));
            assert_eq!(m.data.idea.as_ref().unwrap()["ref"], "I1");
            m.open_link("ideas/I4", cx);
            assert!(m.data.idea.is_none(), "I1 isn't shown as I4 while it loads");
        })
        .unwrap();
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, _| assert_eq!(m.data.idea.as_ref().unwrap()["ref"], "I4")).unwrap();
    }

    #[gpui_kit::test]
    fn releases_page_shows_only_while_a_project_has_releases_on(cx: &mut gpui_kit::TestAppContext) {
        use crate::backend::Backend as _;
        let (w, rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            m.open_link("releases", cx);
            assert_eq!(m.page, Page::Board);
        })
        .unwrap();
        rec.board().post("projects/webapp", json!({"releases": true})).unwrap();
        rec.board().post("releases", json!({"project": "webapp", "name": "v1.0"})).unwrap();
        w.update(cx, |m: &mut MainWindow, _, cx| m.refresh(cx)).unwrap();
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| m.open_link("releases", cx)).unwrap();
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, _| {
            assert_eq!(m.page, Page::Releases);
            let card = ui::releases::card(&m.releases.lists["webapp"], crate::fmt::now());
            assert_eq!(card.latest.map(|r| r.name), Some("v1.0".to_string()));
            assert!(m.releases.details.is_empty(), "the overview fetches no releases in full");
        })
        .unwrap();
        // A release link opens its project's page, its newest release in full.
        w.update(cx, |m: &mut MainWindow, _, cx| m.open_link("release/R1", cx)).unwrap();
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, _| {
            assert_eq!(m.page, Page::ReleasesOf("webapp".into()));
            let groups = arr(&m.releases.details["R1"], "groups");
            let refs: Vec<&str> = groups.iter().flat_map(|g| arr(g, "tasks")).filter_map(|t| t["ref"].as_str()).collect();
            assert!(refs.contains(&"T1"));
            assert_eq!(m.releases.lists["webapp"]["counts"]["total"], 1);
            assert!(m.releases.details.contains_key("unreleased:webapp"));
            assert!(rec.posts().is_empty());
        })
        .unwrap();
        rec.board().post("projects/webapp", json!({"releases": false})).unwrap();
        w.update(cx, |m: &mut MainWindow, _, cx| m.refresh(cx)).unwrap();
        settle(cx);
        w.update(cx, |m: &mut MainWindow, _, _| assert_eq!(m.page, Page::Board)).unwrap();
    }
}
