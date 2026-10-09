//! The main window: what the board last answered (`Data`), where the reader is (`Page`, the side
//! panel, a modal), the 3-second poll, and the chrome around the pages: the sidebar (pages and the
//! goals rail), the alert banner, toasts and the status bar.
//!
//! Every page lives in its own module under `ui/` with a `State` struct held here and a
//! `render(m, window, cx)` function. They read `m.data` and act through [`MainWindow::post`].
use crate::backend::{Backend, CallError};
use crate::fmt::{self, arr, b, s};
use crate::theme::Theme;
use crate::ui::{self, kit};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_secs(3);
/// How long a toast stays (the web's `toast`): 3 s, an error 6 s.
const TOAST_OK: Duration = Duration::from_secs(3);
const TOAST_ERR: Duration = Duration::from_secs(6);

#[derive(Clone, Debug, PartialEq)]
pub enum Page {
    Board,
    /// A goal ref, `G3`.
    Goal(String),
    /// The backlog: triage it by making goals from untriaged issues.
    Backlog,
    Sessions,
    Days,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TaskTab {
    #[default]
    Overview,
    Context,
    Log,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Panel {
    Task { r: String, tab: TaskTab },
    Issue { r: String },
}

/// What the board last answered. `None` until the first answer.
#[derive(Default)]
pub struct Data {
    pub state: Option<Value>,
    pub goals: Vec<Value>,
    /// `GET /tasks/:id` for the open task panel.
    pub task: Option<Value>,
    /// `GET /backlog/:id` for the open issue panel.
    pub issue: Option<Value>,
    /// `GET /goals/:id` for the goal page.
    pub goal: Option<Value>,
    /// `GET /sessions`, `GET /sessions/closed`, `GET /sessions/:id` for the Sessions page.
    pub sessions: Option<Value>,
    pub closed: Option<Value>,
    pub session: Option<Value>,
    /// `GET /days` for the Days page.
    pub days: Option<Value>,
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
}

pub struct MainWindow {
    pub backend: Arc<dyn Backend>,
    pub data: Data,
    pub filters: Filters,
    pub page: Page,
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
    pub sidebar: ui::sidebar::State,
    /// The sidebar's width while it's being dragged.
    pub sidebar_width: f32,
    /// The goal list answered at least once (before that the rail falls back to the state's).
    goals_loaded: bool,
    /// Bumped by every refresh so a slow, older answer never overwrites a newer one.
    generation: u64,
    /// The Claude Code hooks (status bar), and whether an install is running.
    pub hooks: crate::hooks::Hooks,
    pub hooks_busy: bool,
}

impl MainWindow {
    pub fn new(backend: Arc<dyn Backend>, window: &mut Window, cx: &mut Context<Self>) -> MainWindow {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut m = MainWindow {
            backend,
            data: Data::default(),
            filters: Filters::load(),
            page: Page::Board,
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
            sidebar: Default::default(),
            sidebar_width: ui::sidebar::rail_width(),
            goals_loaded: false,
            generation: 0,
            hooks: if crate::hooks::target().is_some() { crate::hooks::Hooks::Checking } else { crate::hooks::Hooks::Hidden },
            hooks_busy: false,
        };
        if let Ok(p) = std::env::var("TASKBOARD_PAGE") {
            m.page = match p.as_str() {
                "backlog" | "plan" => Page::Backlog,
                "sessions" => Page::Sessions,
                "days" => Page::Days,
                g if g.starts_with('G') => Page::Goal(g.into()),
                _ => Page::Board,
            };
        }
        // Dev: `TASKBOARD_PICK=B2,B3` checks those issues on the Backlog page (screenshots).
        if let Ok(p) = std::env::var("TASKBOARD_PICK") {
            m.backlog.picked = p.split(',').map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect();
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
        m.start_polling(cx);
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

    /// The rail's goal (`goal-pick`): filter the board to it (again: unfilter), on the board.
    pub fn rail_pick_goal(&mut self, r: &str, cx: &mut Context<Self>) {
        self.go(Page::Board, cx);
        ui::board::goal_pick(self, r, cx);
    }

    /// The rail's project name (`rail-project`).
    pub fn rail_pick_project(&mut self, p: &str, cx: &mut Context<Self>) {
        let f = ui::sidebar::pick_project(&self.filters, p);
        self.board.keep.clear();
        self.go(Page::Board, cx);
        self.set_filters(f, cx);
    }

    /// Esc on the board with a filter set (`railClear`).
    pub fn rail_clear(&mut self, cx: &mut Context<Self>) {
        let f = ui::sidebar::rail_clear(&self.filters);
        self.board.keep.clear();
        self.set_filters(f, cx);
    }

    /// "Goals" (the web's `#/goals`): the goal page of the board's goal, else the first active one.
    pub fn open_goals(&mut self, cx: &mut Context<Self>) {
        let goals: Vec<&Value> = self.data.goals.iter().filter(|g| !b(g, "archived")).collect();
        if let Some(r) = ui::sidebar::landing(&goals, &self.filters.goal) {
            self.go(Page::Goal(r), cx);
        }
    }

    pub fn goals_loaded(&self) -> bool {
        self.goals_loaded
    }

    pub fn go(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.page != page {
            // `resetPageState`: leaving Sessions drops its picking state; another goal's page
            // starts empty; arriving on the Backlog starts with no kept rows or selection.
            if self.page == Page::Sessions {
                ui::sessions::left(self);
            }
            if matches!(self.page, Page::Goal(_)) {
                self.goal_page.reset_for("");
            }
            if let Page::Goal(r) = &page {
                if self.data.goal.as_ref().and_then(|g| g["ref"].as_str()) != Some(r.as_str()) {
                    self.data.goal = None;
                    self.data.errs.goal = None;
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

    pub fn open_task(&mut self, r: impl Into<String>, cx: &mut Context<Self>) {
        let r = r.into();
        if !matches!(&self.panel, Some(Panel::Task { r: cur, .. }) if *cur == r) {
            self.data.task = None;
            self.data.errs.task = None;
            self.task_panel = Default::default();
        }
        self.panel = Some(Panel::Task { r, tab: TaskTab::Overview });
        self.refresh(cx);
    }

    pub fn open_issue(&mut self, r: impl Into<String>, cx: &mut Context<Self>) {
        let r = r.into();
        if !matches!(&self.panel, Some(Panel::Issue { r: cur }) if *cur == r) {
            self.data.issue = None;
            self.data.errs.issue = None;
            self.issue_panel = Default::default();
        }
        self.panel = Some(Panel::Issue { r });
        self.refresh(cx);
    }

    /// A `taskboard://task/T12` / `goal/G3` / `session/<id>` link (a clicked notification).
    pub fn open_link(&mut self, url: &str, cx: &mut Context<Self>) {
        let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url).trim_matches('/');
        let (kind, arg) = rest.split_once('/').unwrap_or((rest, ""));
        match (kind, arg) {
            ("task", r) if !r.is_empty() => self.open_task(r.to_uppercase(), cx),
            ("issue", r) if !r.is_empty() => self.open_issue(r.to_uppercase(), cx),
            ("goal", r) if !r.is_empty() => self.go(Page::Goal(r.to_uppercase()), cx),
            ("session", id) if !id.is_empty() => {
                self.sessions.selected = Some(id.to_string());
                self.go(Page::Sessions, cx);
                self.refresh(cx);
            }
            ("backlog" | "plan", _) => self.go(Page::Backlog, cx),
            ("sessions", _) => self.go(Page::Sessions, cx),
            ("days", _) => self.go(Page::Days, cx),
            _ => self.go(Page::Board, cx),
        }
    }

    pub fn close_panel(&mut self, cx: &mut Context<Self>) {
        self.panel = None;
        self.data.task = None;
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
        let at = Instant::now();
        self.toasts = vec![Toast { text: text.into(), err, at }];
        let stay = if err { TOAST_ERR } else { TOAST_OK };
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
    /// then, on the board with nothing focused, the rail's filters.
    pub fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        match &self.panel {
            Some(Panel::Task { .. }) => return self.close_panel(cx),
            Some(Panel::Issue { .. }) if matches!(self.page, Page::Board | Page::Backlog) => return self.close_panel(cx),
            _ => {}
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
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { backend.post(&path, body) }).await;
            let _ = this.update(cx, |m, cx| {
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
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this
                    .update(cx, |m, cx| {
                        m.refresh(cx);
                        m.check_hooks(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
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

    /// Fetch everything the current view shows, then swap it in (unless a newer refresh started).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let backend = self.backend.clone();
        let want = Wants::of(self);
        cx.spawn(async move |this, cx| {
            let got = cx.background_executor().spawn(async move { want.fetch(&*backend) }).await;
            let _ = this.update(cx, |m, cx| {
                if m.generation == generation {
                    got.apply(m);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // -------------------------------------------------------------- helpers for pages

    pub fn state(&self) -> &Value {
        static NULL: Value = Value::Null;
        self.data.state.as_ref().unwrap_or(&NULL)
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
        project_names(self.state(), ui::sidebar::all_goals(self.data.state.as_ref(), self.goals_loaded.then_some(self.data.goals.as_slice())))
    }

}

// ------------------------------------------------------------------ fetching

/// What one refresh fetches, decided on the UI thread from the current view.
struct Wants {
    state_q: Vec<(&'static str, String)>,
    task: Option<String>,
    issue: Option<String>,
    goal: Option<String>,
    sessions: bool,
    session: Option<String>,
    days_q: Option<Vec<(&'static str, String)>>,
}

/// One fetch's answer: the value, or the board's sentence for why it failed.
type Answer = Result<Value, String>;

struct Got {
    state: Option<Result<Value, CallError>>,
    goals: Option<Value>,
    task: Option<(String, Answer)>,
    issue: Option<(String, Answer)>,
    goal: Option<(String, Answer)>,
    sessions: Option<Answer>,
    closed: Option<Value>,
    session: Option<(String, Answer)>,
    days: Option<Answer>,
}

impl Wants {
    fn of(m: &MainWindow) -> Wants {
        let state_q = crate::ui::board::state_query(&m.filters, &m.board.keep);
        Wants {
            state_q,
            task: match &m.panel {
                Some(Panel::Task { r, .. }) => Some(r.clone()),
                _ => None,
            },
            issue: match &m.panel {
                Some(Panel::Issue { r }) => Some(r.clone()),
                _ => None,
            },
            goal: match &m.page {
                Page::Goal(g) => Some(g.clone()),
                _ => None,
            },
            sessions: m.page == Page::Sessions,
            session: if m.page == Page::Sessions { m.sessions.selected.clone() } else { None },
            days_q: (m.page == Page::Days).then(|| m.days.query()),
        }
    }

    fn fetch(self, be: &dyn Backend) -> Got {
        let state = Some(be.get("state", &self.state_q));
        // While the board is down only the banner speaks (`S.stateErr`); the rest waits.
        let up = state.as_ref().is_some_and(|r| r.is_ok());
        let get = |p: &str, q: &[(&str, String)]| -> Option<Answer> { up.then(|| be.get(p, q).map_err(|e| e.message)) };
        Got {
            goals: get("goals", &[]).and_then(Result::ok),
            task: self.task.and_then(|r| get(&format!("tasks/{r}"), &[]).map(|v| (r, v))),
            issue: self.issue.and_then(|r| get(&format!("backlog/{r}"), &[]).map(|v| (r, v))),
            goal: self.goal.and_then(|r| get(&format!("goals/{r}"), &[]).map(|v| (r, v))),
            sessions: if self.sessions { get("sessions", &[("project", "all".into())]) } else { None },
            closed: if self.sessions { get("sessions/closed", &[("project", "all".into())]).and_then(Result::ok) } else { None },
            session: self.session.and_then(|id| get(&format!("sessions/{id}"), &[]).map(|v| (id, v))),
            days: self.days_q.and_then(|q| get("days", &q)),
            state,
        }
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

impl Got {
    fn apply(self, m: &mut MainWindow) {
        match self.state {
            Some(Ok(v)) => {
                fmt::set_server_now(s(&v, "now"));
                if arr(&v, "alerts").is_empty() && matches!(m.modal, Some(ui::modals::Modal::Alerts)) {
                    m.modal = None;
                }
                m.data.state = Some(v);
                m.down = None;
            }
            Some(Err(e)) => m.down = Some(e.message),
            None => {}
        }
        if let Some(g) = self.goals {
            m.data.goals = if g.is_array() { g.as_array().cloned().unwrap_or_default() } else { arr(&g, "goals").to_vec() };
            m.goals_loaded = true;
        }
        // Only keep answers for what is still open.
        let d = &mut m.data;
        if let Some((r, v)) = self.task {
            if matches!(&m.panel, Some(Panel::Task { r: cur, .. }) if *cur == r) {
                land(&mut d.task, &mut d.errs.task, v, |v| v);
            }
        }
        if let Some((r, v)) = self.issue {
            if matches!(&m.panel, Some(Panel::Issue { r: cur }) if *cur == r) {
                // The board may wrap it: `b.issue || b`.
                land(&mut d.issue, &mut d.errs.issue, v, |v| if v["issue"].is_object() { v["issue"].clone() } else { v });
            }
        }
        if let Some((r, v)) = self.goal {
            if m.page == Page::Goal(r) {
                land(&mut d.goal, &mut d.errs.goal, v, merge_goal);
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
                land(&mut d.days, &mut d.errs.days, v, |v| v);
            }
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

actions!(taskboard, [CloseOverlay, GoBoard, GoBacklog, GoSessions, GoDays, Refresh]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", CloseOverlay, Some("MainWindow")),
        KeyBinding::new("cmd-1", GoBoard, Some("MainWindow")),
        KeyBinding::new("cmd-2", GoBacklog, Some("MainWindow")),
        KeyBinding::new("cmd-3", GoSessions, Some("MainWindow")),
        KeyBinding::new("cmd-4", GoDays, Some("MainWindow")),
        KeyBinding::new("cmd-r", Refresh, Some("MainWindow")),
    ]);
}

// ------------------------------------------------------------------ render

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<Theme>().clone();
        window.set_window_title(&self.title());

        let page = match self.page.clone() {
            Page::Board => ui::board::render(self, window, cx),
            Page::Goal(_) => ui::goal::render(self, cx),
            Page::Backlog => ui::backlog::render(self, window, cx),
            Page::Sessions => ui::sessions::render(self, window, cx),
            Page::Days => ui::days::render(self, window, cx),
        };
        let panel = match self.panel.clone() {
            Some(Panel::Task { .. }) => Some(ui::task_panel::render(self, window, cx)),
            Some(Panel::Issue { .. }) => Some(ui::issue_panel::render(self, window, cx)),
            None => None,
        };
        let modal = self.modal.is_some().then(|| ui::modals::render(self, window, cx));
        let hours_menu = ui::hours::render_menu(self, window, cx);
        let peek = ui::sidebar::render_peek(self, window, cx);

        div()
            .id("main")
            .key_context("MainWindow")
            .track_focus(&self.focus)
            .on_action(cx.listener(|m, _: &CloseOverlay, window, cx| m.escape(window, cx)))
            // The sidebar's width drag (the rail grip).
            .on_mouse_move(cx.listener(|m, e: &MouseMoveEvent, _, cx| {
                if let Some((x0, w0)) = m.sidebar.drag {
                    m.sidebar_width = (w0 + e.position.x.as_f32() - x0).clamp(ui::sidebar::RAIL_MIN, ui::sidebar::RAIL_MAX);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|m, _, _, cx| {
                    if m.sidebar.drag.take().is_some() {
                        m.sidebar_width = ui::sidebar::save_rail_width(m.sidebar_width);
                        cx.notify();
                    }
                }),
            )
            .on_action(cx.listener(|m, _: &GoBoard, _, cx| m.go(Page::Board, cx)))
            .on_action(cx.listener(|m, _: &GoBacklog, _, cx| m.go(Page::Backlog, cx)))
            .on_action(cx.listener(|m, _: &GoSessions, _, cx| m.go(Page::Sessions, cx)))
            .on_action(cx.listener(|m, _: &GoDays, _, cx| m.go(Page::Days, cx)))
            .on_action(cx.listener(|m, _: &Refresh, _, cx| m.refresh(cx)))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(t.bg)
            .text_color(t.text)
            .font_family(t.ui_font.clone())
            .text_size(px(14.))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(ui::sidebar::render(self, window, cx))
                    .child(div().flex().flex_col().flex_1().min_w_0().children(banner(self, &t, cx)).child(div().flex().flex_1().min_h_0().child(page))),
            )
            .child(status_bar(self, &t, cx))
            .children(panel)
            .children(modal)
            .children(hours_menu)
            .children(peek)
            .children(toasts(self, &t))
            .when(self.menu.is_some(), |d| {
                // A click anywhere outside an open menu closes it.
                d.child(deferred(div().id("menu-dismiss").absolute().top_0().left_0().size_full().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|m, _, _, cx| {
                        m.menu = None;
                        cx.notify();
                    }),
                )).with_priority(2))
            })
    }
}

// ------------------------------------------------------------------ chrome view models

/// `renderAll`'s title: the page's name, with "(n) " in front while tasks need you.
pub fn page_title(page: &Page, needs: i64) -> String {
    let base = match page {
        Page::Goal(_) => "Goals",
        Page::Backlog => "Backlog",
        Page::Sessions => "Sessions",
        Page::Days => "Days",
        Page::Board => "Task board",
    };
    if needs > 0 { format!("({needs}) {base}") } else { base.to_string() }
}

/// One banner line (`renderBanner`) or alerts-dialog row: what it says and its buttons.
#[derive(Clone, Debug, PartialEq)]
pub struct BannerRow {
    /// "down" (the board isn't answering) or "alert".
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

/// `renderBanner`: the "isn't answering" line, then either one line per alert or, with more
/// than one, a single "N tasks need your attention." line with Show all.
pub fn banner_view(down: Option<&str>, alerts: &[Value]) -> Vec<BannerRow> {
    let mut out = Vec::new();
    if let Some(e) = down.filter(|e| !e.is_empty()) {
        out.push(BannerRow { kind: "down", text: format!("{e} Trying again every few seconds."), ago: None, buttons: Vec::new() });
    }
    if alerts.len() > 1 {
        out.push(BannerRow {
            kind: "alert",
            text: format!("{} your attention.", fmt::plural(alerts.len() as i64, "task needs", "tasks need")),
            ago: Some(fmt::ago(s(alerts.last().unwrap_or(&Value::Null), "at"))),
            buttons: vec![("alerts-open", "Show all".into())],
        });
    } else {
        for a in alerts {
            out.push(BannerRow { kind: "alert", text: s(a, "text").to_string(), ago: Some(fmt::ago(s(a, "at"))), buttons: alert_buttons(a) });
        }
    }
    out
}

/// `alertsDialogHtml`: (title, rows); its foot is "Dismiss all" while any alert can be dismissed. (For the dialog in modals.rs.)
pub fn alerts_dialog_view(alerts: &[Value]) -> (String, Vec<BannerRow>) {
    let title = format!("{} your attention", fmt::plural(alerts.len() as i64, "task needs", "tasks need"));
    let rows = alerts.iter().map(|a| BannerRow { kind: "alert", text: s(a, "text").to_string(), ago: Some(fmt::ago(s(a, "at"))), buttons: alert_buttons(a) }).collect();
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

/// The banner lines: the board down, the login item (native), the alerts.
fn banner(m: &MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Vec<AnyElement> {
    let mut out = Vec::new();
    let alerts = arr(m.state(), "alerts").to_vec();
    let rows = banner_view(m.down.as_deref(), &alerts);
    for r in rows.iter().filter(|r| r.kind == "down") {
        out.push(bar(t.down_soft, t.down, t.down_line).child(kit::dot(t.down, 8.)).child(div().flex_1().min_w_0().child(r.text.clone())).into_any_element());
    }
    match crate::install::status() {
        crate::install::LoginItem::RequiresApproval => out.push(
            bar(t.warn_soft, t.warn_text, t.warn_line)
                .child(kit::dot(t.warn, 8.))
                .child(div().flex_1().min_w_0().child("The board runs in the background as a login item. Switch on Taskboard in System Settings ▸ Login Items."))
                .child(kit::btn_small(t, "open-login-items", "Open Login Items").on_click(|_, _, _| crate::install::open_login_items()))
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
    if alerts.len() > 1 {
        let r = rows.iter().find(|r| r.kind == "alert").cloned();
        if let Some(r) = r {
            out.push(
                bar(t.down_soft, t.down, t.down_line)
                    .child(kit::dot(t.down, 8.))
                    .child(alert_text(t, &r))
                    .child(banner_btn(t, "alerts-open", "Show all", true).on_click(cx.listener(|m, _, window, cx| {
                        m.set_modal(Some(ui::modals::Modal::Alerts), window, cx);
                    })))
                    .into_any_element(),
            );
        }
    } else {
        for a in &alerts {
            out.push(alert_row(t, a, cx).into_any_element());
        }
    }
    out
}

fn alert_text(t: &Theme, r: &BannerRow) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_wrap()
        .items_baseline()
        .gap(px(4.))
        .child(r.text.clone())
        .children(r.ago.clone().map(|a| div().ml(px(4.)).text_size(px(10.8)).text_color(t.muted).child(a)))
}

/// One alert: its text and age, "Open T12" and Dismiss. Also used by the alerts dialog.
pub fn alert_row(t: &Theme, a: &Value, cx: &mut Context<MainWindow>) -> Div {
    let id = s(a, "id").to_string();
    let r = BannerRow { kind: "alert", text: s(a, "text").to_string(), ago: Some(fmt::ago(s(a, "at"))), buttons: alert_buttons(a) };
    let (task, goal) = (fmt::opt_s(a, "task").map(str::to_string), fmt::opt_s(a, "goal").map(str::to_string));
    bar(t.down_soft, t.down, t.down_line)
        .child(kit::dot(t.down, 8.))
        .child(alert_text(t, &r))
        .children(r.buttons.iter().filter(|(act, _)| *act == "alert-open").map(|(_, label)| {
            let (task, goal) = (task.clone(), goal.clone());
            banner_btn(t, SharedString::from(format!("alert-open-{id}")), label.clone(), true).on_click(cx.listener(move |m, _, window, cx| {
                open_alert(m, task.as_deref(), goal.as_deref(), window, cx);
            }))
        }))
        .when(r.buttons.iter().any(|(act, _)| *act == "alert-dismiss"), |d| {
            d.child(banner_btn(t, SharedString::from(format!("alert-dismiss-{id}")), "Dismiss", false).on_click(cx.listener(move |m, _, _, cx| dismiss_alert(m, &id, cx))))
        })
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

/// Bottom bar (`statusBarHtml`): Midna connection, work hours and usage pills.
fn status_bar(m: &mut MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Div {
    let st = m.state().clone();
    // `#statusbar`: min-height 34, 4px 12px, 8px gaps.
    let mut bar = div().flex().flex_none().items_center().gap(px(8.)).min_h(px(34.)).py(px(4.)).px(px(12.)).border_t_1().border_color(t.border).bg(t.card);
    if let Some(mid) = st.get("midna").filter(|v| v.is_object()) {
        let p = conn_pill(mid);
        let (fg, bg, dot) = if p.cls == "up" { (t.up_fg, t.up_soft, t.up) } else { (t.warn_fg, t.warn_soft, t.warn) };
        bar = bar.child(pill_shell(t, "conn-pill", fg, bg).child(kit::dot(dot, 8.)).child(p.label).tooltip(kit::tip(p.title)));
    }
    bar = bar.child(ui::hours::pill(m, t, cx));
    if let Some(u) = st.get("usage").and_then(usage_view) {
        bar = bar.child(usage_pill(t, &u));
    }
    bar = bar.child(div().flex_1());
    if let Some(h) = hooks_item(m, t, cx) {
        bar = bar.child(h);
    }
    if m.backend.label() == "fake" {
        bar = bar.child(kit::tone_pill(t, "goal", "Sample board"));
    }
    if let Some(p) = accounts_pill(&st, t) {
        bar = bar.child(p);
    }
    let text = t.text;
    bar = bar.child(
        div()
            .id("open-settings")
            .text_size(px(12.5))
            .text_color(t.muted)
            .cursor_pointer()
            .hover(move |s| s.text_color(text))
            .tooltip(kit::tip("Settings: GitHub, Bitbucket and Slack accounts (⌘,)"))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::OpenSettings), cx))
            .child("Settings"),
    );
    bar
}

/// Amber "GitHub: sign in again" when an account lacks scopes Taskboard needs (or stopped working);
/// opens Settings. `state.accounts` is the daemon's `accounts::attention`.
fn accounts_pill(st: &Value, t: &Theme) -> Option<AnyElement> {
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

/// "Install hooks", "● Hooks" or an amber "Reinstall hooks", as in Midna's status bar.
fn hooks_item(m: &MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
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
fn usage_pill(t: &Theme, u: &UsageView) -> AnyElement {
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
fn toasts(m: &MainWindow, t: &Theme) -> Option<AnyElement> {
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
                .shadow(ui::sidebar::shadow_modal(t))
                .text_size(px(13.))
                .bg(if x.err { t.down } else { t.text })
                .text_color(if x.err { gpui_kit::white() } else { t.bg })
                .child(x.text.clone()),
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
    fn urgent_alerts_cant_be_dismissed() {
        let urgent = json!({"id": "a1", "text": "Main is red", "at": "2026-10-08T10:00:00Z", "urgent": true, "task": "T3"});
        let rows = banner_view(None, &[urgent.clone()]);
        assert!(rows[0].buttons.iter().all(|(a, _)| *a != "alert-dismiss"));
        assert!(rows[0].buttons.iter().any(|(a, _)| *a == "alert-open"));
        assert!(super::alert_stays(&urgent));
        assert!(!super::alert_stays(&json!({"id": "a2"})));
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
            if let Some(st) = m.data.state.as_mut() {
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
    fn escape_follows_the_web_order(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, window, cx| {
            m.set_modal(Some(ui::modals::Modal::Alerts), window, cx);
            m.escape(window, cx);
            assert!(m.modal.is_none(), "the alerts dialog closes");
            // The task panel closes; an issue panel only on the board.
            m.panel = Some(Panel::Task { r: "T1".into(), tab: TaskTab::Overview });
            m.escape(window, cx);
            assert!(m.panel.is_none());
            m.page = Page::Goal("G1".into());
            m.panel = Some(Panel::Issue { r: "B1".into() });
            m.escape(window, cx);
            assert!(m.panel.is_some(), "on a goal page the issue stays");
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
            // Then the rail's filters (railClear), with the board focused.
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
            m.data.goals.push(json!({"id": 99, "ref": "G99", "name": "x", "project": "Zebra"}));
            m.data.goals.push(json!({"id": 98, "ref": "G98", "name": "y", "project": "archived-only", "archived": true}));
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
    fn goals_link_opens_the_first_active_goal(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            m.open_goals(cx);
            assert_eq!(m.page, Page::Goal("G1".into()));
        })
        .unwrap();
    }
}
