//! The main window: what the board last answered (`Data`), where the reader is (`Page`, the side
//! panel, a modal), the 3-second poll, and the chrome around the pages: the header (the pages as tabs),
//! the alert banner, toasts and the status bar.
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
    /// Every task, grouped and filtered.
    Tasks,
    /// Every goal, as cards: active, deprioritized or finished.
    Goals,
    /// A goal ref, `G3`.
    Goal(String),
    /// The backlog: triage it by making goals from untriaged issues.
    Backlog,
    Sessions,
    Days,
    /// The weekly time review (`ui::review`): which week is in `MainWindow::review`.
    Review,
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
    /// `GET /home` and today's `GET /days` for the home page.
    pub home: Option<Value>,
    pub today: Option<Value>,
    /// `GET /tasks` for the Tasks page.
    pub tasks: Option<Value>,
    /// Every backlog issue (`GET /backlog?state=all`) for the ⌘K palette.
    pub backlog: Option<Value>,
    /// `GET /reviews` and the one shown, `GET /reviews/:id`, for the Review page.
    pub reviews: Option<Value>,
    pub review: Option<Value>,
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
    /// Pages left behind, newest last; ⌘← walks back through them.
    back: Vec<Page>,
    /// Pages ⌘← stepped away from, newest last; ⌘→ returns to them and a fresh visit clears them.
    forward: Vec<Page>,
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
    pub search: ui::search::State,
    /// The goal list answered at least once (before that the state's goals stand in).
    goals_loaded: bool,
    /// Bumped by every refresh so a slow, older answer never overwrites a newer one.
    generation: u64,
    /// The newest refresh that has answered: behind `generation` while one is still out.
    answered: u64,
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
            search: Default::default(),
            goals_loaded: false,
            generation: 0,
            answered: 0,
            hooks: if crate::hooks::target().is_some() { crate::hooks::Hooks::Checking } else { crate::hooks::Hooks::Hidden },
            hooks_busy: false,
        };
        if let Ok(p) = std::env::var("TASKBOARD_PAGE") {
            m.page = match p.as_str() {
                "backlog" | "plan" => Page::Backlog,
                "sessions" => Page::Sessions,
                "days" => Page::Days,
                "review" => Page::Review,
                "tasks" => Page::Tasks,
                "goals" => Page::Goals,
                g if g.starts_with('G') => Page::Goal(g.into()),
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
        // Dev: `TASKBOARD_SEARCH=pass` opens the ⌘K palette with that query (screenshots).
        if let Ok(q) = std::env::var("TASKBOARD_SEARCH") {
            ui::search::open(&mut m, window, cx);
            if let Some(i) = &m.search.input {
                i.set_text(&q, cx);
            }
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

    pub fn go(&mut self, page: Page, cx: &mut Context<Self>) {
        // A task is a page of its own: going anywhere leaves it.
        if self.task_page_open() {
            self.panel = None;
            self.data.task = None;
            cx.notify();
        }
        if self.page != page {
            self.back.push(self.page.clone());
            self.forward.clear();
            self.visit(page, cx);
        }
    }

    /// ⌘←: the page before this one.
    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = self.back.pop() {
            self.forward.push(self.page.clone());
            self.visit(page, cx);
        }
    }

    /// ⌘→: the page ⌘← left.
    pub fn go_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = self.forward.pop() {
            self.back.push(self.page.clone());
            self.visit(page, cx);
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

    /// Whether a task fills the main area (its page) over the current page.
    pub fn task_page_open(&self) -> bool {
        matches!(self.panel, Some(Panel::Task { .. }))
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

    /// A `taskboard://task/T12` / `goal/G3` / `session/<id>` link (a clicked notification), or
    /// a web-board route after a `#` (`tasks#/goals/G3`, `#/?task=T6`), as `tb` prints them.
    pub fn open_link(&mut self, url: &str, cx: &mut Context<Self>) {
        let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
        let (path, query) = match rest.split_once('#') {
            Some((_, route)) => route.split_once('?').unwrap_or((route, "")),
            None => (rest, ""),
        };
        let param = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(k)?.strip_prefix('='));
        let path = path.trim_matches('/');
        let (kind, arg) = path.split_once('/').unwrap_or((path, ""));
        if let Some(t) = param("task").filter(|t| !t.is_empty()) {
            self.open_link(path, cx);
            return self.open_task(t.to_uppercase(), cx);
        }
        match (kind, arg) {
            ("task", r) if !r.is_empty() => self.open_task(r.to_uppercase(), cx),
            ("issue", r) if !r.is_empty() => self.open_issue(r.to_uppercase(), cx),
            ("goal" | "goals", r) if !r.is_empty() => ui::goal::open(self, &r.to_uppercase(), param("view") == Some("backlog"), cx),
            ("goals", _) => self.open_goals(cx),
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
                    .update(cx, |m, cx| m.poll(cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        // A poll waits for a refresh that's still out (a page just opened) instead of starting over
        // and throwing its answer away.
        if self.answered == self.generation {
            self.refresh(cx);
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

    /// Fetch everything the current view shows, then swap it in (unless a newer refresh started).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let backend = self.backend.clone();
        let want = Wants::of(self);
        cx.spawn(async move |this, cx| {
            let got = cx.background_executor().spawn(async move { want.fetch(&*backend) }).await;
            let _ = this.update(cx, |m, cx| {
                m.answered = m.answered.max(generation);
                if m.generation == generation {
                    let asked: Vec<String> = got.goal_rows.iter().map(|(r, _)| r.clone()).collect();
                    got.apply(m);
                    cx.notify();
                    // The Goals page learns which goals are in progress from this answer; fetch the
                    // ones it didn't ask for now rather than on the next poll.
                    if m.page == Page::Goals && ui::goals_page::in_progress(&m.data.goals, m.goals.project()).iter().any(|r| !asked.contains(r)) {
                        m.refresh(cx);
                    }
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
        project_names(self.state(), ui::goals::all_goals(self.data.state.as_ref(), self.goals_loaded.then_some(self.data.goals.as_slice())))
    }

}

// ------------------------------------------------------------------ fetching

/// What one refresh fetches, decided on the UI thread from the current view.
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
    /// The Tasks page, for `GET /tasks`.
    tasks: bool,
    /// The ⌘K palette, for `GET /backlog`.
    backlog: bool,
    /// The Review page: the list, and the picked review (`None`: the newest).
    review: Option<Option<i64>>,
}

/// One fetch's answer: the value, or the board's sentence for why it failed.
type Answer = Result<Value, String>;

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
    tasks: Option<Value>,
    backlog: Option<Value>,
    reviews: Option<Answer>,
    review: Option<(i64, Answer)>,
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
            goal_rows: if m.page == Page::Goals { ui::goals_page::in_progress(&m.data.goals, m.goals.project()) } else { vec![] },
            // The ⌘K palette searches the task, terminal and backlog lists too.
            sessions: m.page == Page::Sessions || ui::search::is_open(m),
            session: if m.page == Page::Sessions { m.sessions.selected.clone() } else { None },
            days_q: (m.page == Page::Days).then(|| m.days.query()),
            home: (m.page == Page::Board).then(|| m.filters.project.clone()),
            tasks: m.page == Page::Tasks || ui::search::is_open(m),
            backlog: ui::search::is_open(m),
            review: (m.page == Page::Review).then_some(m.review.id),
        }
    }

    /// Every call at once, so the page's own answer doesn't wait behind `GET /state`.
    fn fetch(self, be: &dyn Backend) -> Got {
        fn join<T>(h: std::thread::ScopedJoinHandle<'_, T>) -> T {
            h.join().expect("fetch thread")
        }
        let get = |p: &str, q: &[(&str, String)]| -> Answer { be.get(p, q).map_err(|e| e.message) };
        let got = std::thread::scope(|s| {
            let state = s.spawn(|| be.get("state", &self.state_q));
            let goals = s.spawn(|| get("goals", &[]));
            let task = self.task.as_ref().map(|r| s.spawn(move || (r.clone(), get(&format!("tasks/{r}"), &[]))));
            let issue = self.issue.as_ref().map(|r| s.spawn(move || (r.clone(), get(&format!("backlog/{r}"), &[]))));
            let goal = self.goal.as_ref().map(|r| s.spawn(move || (r.clone(), get(&format!("goals/{r}"), &[]))));
            let goal_rows: Vec<_> = self.goal_rows.iter().map(|r| s.spawn(move || (r.clone(), get(&format!("goals/{r}"), &[])))).collect();
            let sessions = self.sessions.then(|| s.spawn(move || get("sessions", &[("project", "all".into())])));
            let closed = self.sessions.then(|| s.spawn(move || get("sessions/closed", &[("project", "all".into())])));
            let session = self.session.as_ref().map(|id| s.spawn(move || (id.clone(), get(&format!("sessions/{id}"), &[]))));
            let days = self.days_q.as_ref().map(|q| s.spawn(move || get("days", q)));
            let today = self.home.is_some().then(|| s.spawn(move || get("days", &[])));
            let home = self.home.as_ref().map(|p| s.spawn(move || get("home", &[("project", p.clone())])));
            let tasks = self.tasks.then(|| s.spawn(move || get("tasks", &[])));
            let backlog = self.backlog.then(|| s.spawn(move || get("backlog", &[("state", "all".into())])));
            let reviews = self.review.map(|id| {
                s.spawn(move || {
                    let list = get("reviews", &[]);
                    let newest = list.as_ref().ok().and_then(|l| arr(l, "reviews").first()?["id"].as_i64());
                    let one = id.or(newest).map(|id| (id, get(&format!("reviews/{id}"), &[])));
                    (list, one)
                })
            });
            let (reviews, review) = reviews.map(join).map_or((None, None), |(l, one)| (Some(l), one));
            Got {
                state: Some(join(state)),
                goals: join(goals).ok(),
                task: task.map(join),
                issue: issue.map(join),
                goal: goal.map(join),
                goal_rows: goal_rows.into_iter().map(join).collect(),
                sessions: sessions.map(join),
                closed: closed.map(join).and_then(Result::ok),
                session: session.map(join),
                days: days.map(join),
                today: today.map(join).and_then(Result::ok),
                home: home.map(join).and_then(Result::ok),
                tasks: tasks.map(join).and_then(Result::ok),
                backlog: backlog.map(join).and_then(Result::ok),
                reviews,
                review,
            }
        });
        // While the board is down only the banner speaks (`S.stateErr`); the rest waits.
        if got.state.as_ref().is_some_and(|r| r.is_ok()) { got } else { Got { state: got.state, ..Got::default() } }
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
        let searching = ui::search::is_open(m);
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
        if m.page == Page::Goals {
            for (r, v) in self.goal_rows {
                if let Ok(v) = v {
                    m.goals.details.insert(r, merge_goal(v));
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
                land(&mut d.days, &mut d.errs.days, v, |v| v);
            }
        }
        if self.home.is_some() {
            d.home = self.home;
        }
        if self.today.is_some() {
            d.today = self.today;
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
        if self.tasks.is_some() && (m.page == Page::Tasks || searching) {
            d.tasks = self.tasks;
        }
        if self.backlog.is_some() && searching {
            d.backlog = self.backlog;
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
    ]);
}

// ------------------------------------------------------------------ render

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<Theme>().clone();
        window.set_window_title(&self.title());

        let page = match self.page.clone() {
            _ if self.task_page_open() => ui::task_panel::render_page(self, window, cx),
            Page::Board => ui::home::render(self, window, cx),
            Page::Tasks => ui::tasks::render(self, window, cx),
            Page::Goals => ui::goals_page::render(self, window, cx),
            Page::Goal(_) => ui::goal::render(self, cx),
            Page::Backlog => ui::backlog::render(self, window, cx),
            Page::Sessions => ui::sessions::render(self, window, cx),
            Page::Days => ui::days::render(self, window, cx),
            Page::Review => ui::review::render(self, window, cx),
        };
        let panel = match self.panel.clone() {
            Some(Panel::Task { .. }) => None,
            Some(Panel::Issue { .. }) => Some(ui::issue_panel::render(self, window, cx)),
            None => None,
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
                    ),
            )
            .children(panel)
            .children(modal)
            .children(hours_menu)
            .children(toasts(self, &t))
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
        Page::Backlog => "Backlog",
        Page::Sessions => "Sessions",
        Page::Days => "Days",
        Page::Review => "Review",
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
                .shadow(ui::kit::shadow_modal(t))
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
    fn a_task_is_a_page_that_any_page_leaves(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            m.go(Page::Goals, cx);
            m.open_task("T1", cx);
            assert!(m.task_page_open());
            m.go(Page::Goals, cx);
            assert!(!m.task_page_open(), "the same page in the sidebar leaves the task too");
            assert_eq!(m.page, Page::Goals);
            m.open_task("T1", cx);
            m.go(Page::Board, cx);
            assert!(m.panel.is_none() && m.page == Page::Board);
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
    fn goals_link_opens_the_goals_page(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            m.open_goals(cx);
            assert_eq!(m.page, Page::Goals);
        })
        .unwrap();
    }
}
