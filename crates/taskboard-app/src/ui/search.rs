//! Search (⌘K and the sidebar's search box): a palette over the page that finds tasks, goals,
//! backlog issues and terminals as you type. ↑ ↓ move, ↩ opens the result on its own page, esc closes.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, i, s};
use crate::theme::Theme;
use crate::ui::goals_page::{GoalGroup, goal_group};
use crate::ui::modals::Modal;
use crate::ui::status::St;
use crate::ui::text_input::FieldChanged;
use crate::ui::{issue_panel, kit, sessions, sidebar, tasks};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;
use std::ops::Range;
use std::time::Duration;

/// Results shown per kind; the group's header counts them all.
const PER_GROUP: usize = 6;
/// The tasks and issues asked of the board per query, best first; the palette ranks them again.
const WINDOW: usize = 30;
/// How long typing pauses before the board is asked about it, so a burst of keys asks once.
const PAUSE: Duration = Duration::from_millis(120);

#[derive(Default)]
pub struct State {
    pub input: Option<kit::Input>,
    /// The highlighted result, counted down the whole list.
    pub sel: usize,
    scroll: ScrollHandle,
    /// What the board last answered: the query and how many tasks and issues match it in all.
    found: Option<(String, usize, usize)>,
    /// The ask waiting for typing to pause; the next key drops it and waits again.
    soon: Option<Task<()>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Tasks,
    Goals,
    Backlog,
    Terminals,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Tasks => "Tasks",
            Kind::Goals => "Goals",
            Kind::Backlog => "Backlog",
            Kind::Terminals => "Terminals",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Task(String),
    Goal(String),
    Issue(String),
    Terminal(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dot {
    Status(St),
    Goal,
    /// A backlog issue: filled while it's open.
    Issue { open: bool },
    Idle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub target: Target,
    /// T12 / G3 / B7; empty for a terminal.
    pub r: String,
    pub title: String,
    /// The matched spans of `title` (byte ranges).
    pub hl: Vec<Range<usize>>,
    /// The muted text on the right.
    pub place: String,
    pub dot: Dot,
}

#[derive(Debug, PartialEq)]
pub struct Group {
    pub kind: Kind,
    pub total: usize,
    pub hits: Vec<Hit>,
}

// ================================================================== matching

#[cfg(test)]
use taskboardd::palette::find_ci;
use taskboardd::palette::{score, words};

/// Ranked hits of one kind, best first, then in each object's own order.
fn rank<T>(rows: Vec<(T, Option<(u8, Vec<Range<usize>>)>)>, words: &[String], order: impl Fn(&T) -> (u8, i64)) -> Vec<(T, Vec<Range<usize>>)> {
    let mut v: Vec<(u8, T, Vec<Range<usize>>)> = rows.into_iter().filter_map(|(t, m)| if words.is_empty() { Some((0, t, vec![])) } else { m.map(|(sc, hl)| (sc, t, hl)) }).collect();
    v.sort_by_key(|(sc, t, _)| (*sc, order(t)));
    v.into_iter().map(|(_, t, hl)| (t, hl)).collect()
}

fn group(kind: Kind, hits: Vec<Hit>) -> Group {
    Group { kind, total: hits.len(), hits: hits.into_iter().take(PER_GROUP).collect() }
}

fn task_hits(tasks: &Value, w: &[String]) -> Vec<Hit> {
    let items = tasks::items(tasks);
    // With nothing typed: the tasks still to do, those that need you first.
    let items: Vec<tasks::Item> = if w.is_empty() { items.into_iter().filter(|it| it.open).collect() } else { items };
    let rows = items
        .into_iter()
        .map(|it| {
            let g = it.goal.as_ref().map(|(r, n, _)| (r.as_str(), n.as_str()));
            let m = score(&it.r, &it.title, &[&it.project, g.map(|g| g.1).unwrap_or("")], w);
            (it, m)
        })
        .collect();
    rank(rows, w, |it| ((!it.open) as u8 * 10 + it.row.st.rank(), -it.id))
        .into_iter()
        .map(|(it, hl)| {
            let place = match (&it.goal, it.wave) {
                (Some((g, _, _)), Some(n)) => format!("{g} · wave {n}"),
                (Some((g, _, _)), None) => g.clone(),
                (None, _) => it.project.clone(),
            };
            Hit { target: Target::Task(it.r.clone()), r: it.r, title: it.title, hl, place, dot: Dot::Status(it.row.st) }
        })
        .collect()
}

fn goal_hits(goals: &[Value], w: &[String]) -> Vec<Hit> {
    let list: Vec<&Value> = if w.is_empty() { sidebar::pinned_goals(goals) } else { goals.iter().filter(|g| !fmt::b(g, "archived")).collect() };
    let rows = list
        .into_iter()
        .map(|g| {
            let r = fmt::ref_of(g, "G");
            let m = score(&r, s(g, "name"), &[s(g, "project")], w);
            (g, m)
        })
        .collect();
    rank(rows, w, |g| (goal_group(g) as u8, -i(g, "id")))
        .into_iter()
        .map(|(g, hl)| {
            let r = fmt::ref_of(g, "G");
            let place = if goal_group(g) == GoalGroup::Finished { "Finished".into() } else { sidebar::goal_sub(g) };
            Hit { target: Target::Goal(r.clone()), r, title: s(g, "name").to_string(), hl, place, dot: Dot::Goal }
        })
        .collect()
}

/// Backlog issues only once something is typed: the open ones first, newest first.
fn backlog_hits(backlog: &Value, w: &[String]) -> Vec<Hit> {
    if w.is_empty() {
        return vec![];
    }
    let rows = arr(backlog, "issues")
        .iter()
        .map(|b| {
            let r = fmt::ref_of(b, "B");
            let m = score(&r, s(b, "title"), &[s(b, "project"), s(&b["goal"], "name"), s(b, "area"), s(b, "detail"), s(b, "said")], w);
            (b, m)
        })
        .collect();
    rank(rows, w, |b| (issue_panel::issue_state(b).is_some() as u8, -i(b, "id")))
        .into_iter()
        .map(|(b, hl)| {
            let r = fmt::ref_of(b, "B");
            let state = issue_panel::issue_state(b);
            let place = match (&state, fmt::opt_s(&b["goal"], "ref")) {
                (Some((short, _)), _) => short.clone(),
                (None, Some(g)) => format!("{} · {g}", issue_panel::kind_label(s(b, "kind"))),
                (None, None) => format!("{} · {}", issue_panel::kind_label(s(b, "kind")), s(b, "project")),
            };
            Hit { target: Target::Issue(r.clone()), r, title: s(b, "title").to_string(), hl, place, dot: Dot::Issue { open: state.is_none() } }
        })
        .collect()
}

fn terminal_status(x: &Value) -> (u8, Dot, String) {
    match s(x, "status") {
        "needs" | "offline" => (0, Dot::Status(St::Needs), "needs you".into()),
        "working" | "waiting" => (1, Dot::Status(St::Working), "working".into()),
        _ => (2, Dot::Idle, format!("idle {}", sessions::long_ago(sessions::idle_ms(x)))),
    }
}

fn terminal_hits(sessions: &Value, w: &[String]) -> Vec<Hit> {
    let rows = arr(sessions, "sessions")
        .iter()
        .map(|x| {
            let title = terminal_title(x);
            let m = score("", &title, &[s(x, "task_ref"), s(x, "task_title"), s(x, "branch")], w);
            (x, m)
        })
        .collect();
    rank(rows, w, |x| (terminal_status(x).0, 0))
        .into_iter()
        .map(|(x, hl)| {
            let (_, dot, status) = terminal_status(x);
            let place = match fmt::opt_s(x, "task_ref") {
                Some(t) => format!("{t} · {status}"),
                None => status,
            };
            Hit { target: Target::Terminal(s(x, "id").to_string()), r: String::new(), title: terminal_title(x), hl, place, dot }
        })
        .collect()
}

fn terminal_title(x: &Value) -> String {
    match fmt::opt_s(x, "project") {
        Some(p) => format!("{p} · {}", s(x, "name")),
        None => s(x, "name").to_string(),
    }
}

/// The results for `q`: tasks, goals, backlog issues, then terminals, leaving out the kinds with none.
pub fn results(q: &str, tasks: Option<&Value>, goals: &[Value], backlog: Option<&Value>, sessions: Option<&Value>) -> Vec<Group> {
    let w = words(q);
    [
        group(Kind::Tasks, tasks.map(|v| task_hits(v, &w)).unwrap_or_default()),
        group(Kind::Goals, goal_hits(goals, &w)),
        group(Kind::Backlog, backlog.map(|v| backlog_hits(v, &w)).unwrap_or_default()),
        group(Kind::Terminals, sessions.map(|v| terminal_hits(v, &w)).unwrap_or_default()),
    ]
    .into_iter()
    .filter(|g| !g.hits.is_empty())
    .collect()
}

fn current(m: &MainWindow, cx: &App) -> Vec<Group> {
    let q = m.search.input.as_ref().map(|i| i.text(cx)).unwrap_or_default();
    let mut groups = results(&q, m.data.tasks.as_ref(), &m.data.goals, m.data.backlog.as_ref(), m.data.sessions.as_deref());
    // The board sent the best few: its counts are the whole.
    if let Some((_, tasks, issues)) = m.search.found.as_ref().filter(|f| f.0 == q) {
        for g in &mut groups {
            match g.kind {
                Kind::Tasks => g.total = g.total.max(*tasks),
                Kind::Backlog => g.total = g.total.max(*issues),
                _ => {}
            }
        }
    }
    groups
}

/// The query to ask the board about while the palette is open.
pub fn query(m: &MainWindow, cx: &App) -> Option<String> {
    m.search.input.as_ref().filter(|_| is_open(m)).map(|i| i.text(cx))
}

/// `GET /search?palette=1`: the best tasks and backlog issues for `q`, and how many match.
pub fn ask(be: &dyn crate::backend::Backend, q: &str) -> Result<Value, String> {
    be.get("search", &[("palette", "1".into()), ("q", q.to_string()), ("limit", WINDOW.to_string())]).map_err(|e| e.message)
}

/// Keep the board's answer for `q` if it's still what's typed.
pub fn land(m: &mut MainWindow, q: &str, v: Value, cx: &App) {
    if query(m, cx).as_deref() != Some(q) {
        return;
    }
    let total = |k: &str| v[k].as_u64().unwrap_or(0) as usize;
    m.search.found = Some((q.to_string(), total("tasks_total"), total("issues_total")));
    m.data.tasks = Some(serde_json::json!({"tasks": v["tasks"]}));
    m.data.backlog = Some(serde_json::json!({"issues": v["issues"]}));
}

/// Ask the board about what's typed now, without waiting for the rest of a refresh.
fn fetch(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let Some(q) = query(m, cx) else { return };
    let backend = m.backend.clone();
    cx.spawn(async move |this, cx| {
        let ask_q = q.clone();
        let v = cx.background_executor().spawn(async move { ask(&*backend, &ask_q) }).await;
        let _ = this.update(cx, |m, cx| {
            if let Ok(v) = v {
                land(m, &q, v, cx);
                cx.notify();
            }
        });
    })
    .detach();
}

/// [`fetch`] once typing pauses for [`PAUSE`]; meanwhile the palette ranks the last answer again.
fn fetch_soon(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    m.search.soon = Some(cx.spawn(async move |this, cx| {
        cx.background_executor().timer(PAUSE).await;
        let _ = this.update(cx, |m, cx| fetch(m, cx));
    }));
}

fn flat(groups: &[Group]) -> Vec<&Hit> {
    groups.iter().flat_map(|g| &g.hits).collect()
}

// ================================================================== actions

pub fn is_open(m: &MainWindow) -> bool {
    matches!(m.modal, Some(Modal::Search))
}

/// ⌘K: open the palette with an empty query (again: back to the field).
pub fn open(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    if !is_open(m) {
        let input = kit::Input::new(cx, "Search tasks, goals, backlog and terminals", false);
        cx.subscribe(&input.field, |m: &mut MainWindow, _, _: &FieldChanged, cx| {
            m.search.sel = 0;
            m.search.scroll.scroll_to_item(0);
            fetch_soon(m, cx);
            cx.notify();
        })
        .detach();
        m.search = State { input: Some(input), ..Default::default() };
        m.set_modal(Some(Modal::Search), window, cx);
        // The terminal list loads with the palette, the tasks and issues for what's typed.
        m.refresh(cx);
        fetch(m, cx);
    }
    if let Some(i) = &m.search.input {
        window.focus(&i.focus, cx);
    }
}

pub fn close(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    m.search.input = None;
    m.set_modal(None, window, cx);
}

fn go(m: &mut MainWindow, target: Target, window: &mut Window, cx: &mut Context<MainWindow>) {
    close(m, window, cx);
    match target {
        Target::Task(r) => m.open_task(r, cx),
        Target::Goal(r) => crate::ui::goal::open(m, &r, false, cx),
        Target::Issue(r) => m.open_issue(r, cx),
        Target::Terminal(id) => {
            m.go(Page::Sessions, cx);
            sessions::open(m, &id, cx);
        }
    }
}

fn on_key(m: &mut MainWindow, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<MainWindow>) {
    let groups = current(m, cx);
    let hits = flat(&groups);
    let n = hits.len();
    match ev.keystroke.key.as_str() {
        "escape" => {
            cx.stop_propagation();
            close(m, window, cx);
        }
        "down" | "up" if n > 0 => {
            cx.stop_propagation();
            let down = ev.keystroke.key == "down";
            m.search.sel = (m.search.sel.min(n - 1) + if down { 1 } else { n - 1 }) % n;
            m.search.scroll.scroll_to_item(row_child(&groups, m.search.sel));
            cx.notify();
        }
        "enter" => {
            cx.stop_propagation();
            if let Some(h) = hits.get(m.search.sel.min(n.saturating_sub(1))) {
                let target = h.target.clone();
                go(m, target, window, cx);
            }
        }
        _ => {}
    }
}

/// The list's child index of result `sel` (each group's header is a child too).
fn row_child(groups: &[Group], sel: usize) -> usize {
    let mut seen = 0;
    for (gi, g) in groups.iter().enumerate() {
        if sel < seen + g.hits.len() {
            return gi + 1 + sel;
        }
        seen += g.hits.len();
    }
    0
}

// ================================================================== view

pub fn render(m: &mut MainWindow, _window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let Some(input) = m.search.input.as_ref() else { return div().into_any_element() };
    let field = input.field.clone();
    let focus = input.focus.clone();
    let typed = !input.text(cx).trim().is_empty();
    let groups = current(m, cx);
    let sel = m.search.sel.min(flat(&groups).len().saturating_sub(1));

    let head = div()
        .id("search-q")
        .flex()
        .flex_none()
        .items_center()
        .gap(px(12.))
        .h(px(60.))
        .px(px(20.))
        .text_size(px(18.))
        .text_color(t.text)
        .cursor_text()
        .on_click(move |_, window, cx| window.focus(&focus, cx))
        .child(kit::glyph(kit::Glyph::Search, 18., t.muted))
        .child(div().flex_1().min_w_0().child(field));

    let mut list = div().id("search-results").flex().flex_col().min_h_0().overflow_y_scroll().track_scroll(&m.search.scroll).pb(px(8.));
    let mut at = 0;
    for g in &groups {
        list = list.child(
            div()
                .px(px(16.))
                .pt(px(12.))
                .pb(px(4.))
                .text_size(px(11.))
                .font_weight(FontWeight::BOLD)
                .text_color(t.muted)
                .child(format!("{} · {}", g.kind.label().to_uppercase(), g.total)),
        );
        for h in &g.hits {
            list = list.child(result_row(&t, h, at, at == sel, cx));
            at += 1;
        }
    }
    let empty = groups.is_empty() && typed;

    let dialog = kit::modal_box(&t, 640.)
        .min_h_0()
        .max_h(relative(1.))
        .on_key_down(cx.listener(on_key))
        .child(head)
        .when(!groups.is_empty(), |d| d.child(div().h(px(1.)).flex_none().bg(t.divider)).child(list))
        .when(empty, |d| {
            d.child(div().h(px(1.)).flex_none().bg(t.divider))
                .child(div().px(px(20.)).py(px(18.)).text_size(px(14.)).text_color(t.muted).child("Nothing matches."))
        });

    deferred(
        kit::scrim(&t, "search-scrim")
            .flex()
            .flex_col()
            .items_center()
            .pt(px(96.))
            .on_mouse_down(MouseButton::Left, cx.listener(|m, _, window, cx| close(m, window, cx)))
            .child(div().id("search-box").flex().flex_col().min_h_0().max_h(relative(0.8)).on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(dialog)),
    )
    .with_priority(1)
    .into_any_element()
}

fn result_row(t: &Theme, h: &Hit, ix: usize, selected: bool, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let dot = match h.dot {
        Dot::Status(st) => kit::dot(st.colors(t).2, 8.),
        Dot::Goal => div().flex_none().size(px(8.)).rounded(px(2.)).bg(t.goal),
        Dot::Issue { open } => kit::dot(if open { t.backlog_dot } else { t.border_2 }, 8.),
        Dot::Idle => kit::dot(t.border_2, 8.),
    };
    let mark = HighlightStyle { background_color: Some(t.mark), color: Some(t.mark_fg), ..Default::default() };
    let title = StyledText::new(h.title.clone()).with_highlights(h.hl.iter().map(|r| (r.clone(), mark)));
    let hover = t.seg;
    let target = h.target.clone();
    div()
        .id(("search-hit", ix))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .h(px(40.))
        .px(px(16.))
        .mx(px(6.))
        .rounded(px(8.))
        .text_size(px(14.))
        .text_color(t.text)
        .cursor_pointer()
        .when(selected, |d| d.bg(t.accent_soft))
        .when(!selected, |d| d.hover(move |s| s.bg(hover)))
        .child(dot)
        .when(!h.r.is_empty(), |d| d.child(div().flex_none().min_w(px(34.)).font_family(t.mono_font.clone()).text_size(px(12.5)).text_color(t.muted).child(h.r.clone())))
        .child(div().flex_1().min_w_0().truncate().child(title))
        .child(div().flex_none().max_w(px(220.)).truncate().text_size(px(12.)).text_color(t.muted).child(h.place.clone()))
        .on_click(cx.listener(move |m, _, window, cx| go(m, target.clone(), window, cx)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    // Beats the glob's `gpui_kit::test`.
    use ::core::prelude::v1::test;

    fn titles(g: &Group) -> Vec<&str> {
        g.hits.iter().map(|h| h.title.as_str()).collect()
    }

    #[test]
    fn find_ignores_case_and_keeps_byte_ranges() {
        assert_eq!(find_ci("Sign in with a Passkey", "pass"), Some(15..19));
        assert_eq!(find_ci("Café passkeys", "pass"), Some(6..10));
        assert_eq!(find_ci("abc", "abcd"), None);
        assert_eq!(find_ci("abc", ""), None);
    }

    #[test]
    fn ref_beats_title_start_beats_word_beats_inside_beats_other_text() {
        let w = words("t1");
        assert_eq!(score("T1", "x", &[], &w).unwrap().0, 0);
        assert_eq!(score("T12", "x", &[], &w).unwrap().0, 1);
        let w = words("pass");
        assert_eq!(score("T9", "Passkeys", &[], &w).unwrap().0, 2);
        assert_eq!(score("T9", "Sign in with passkeys", &[], &w).unwrap().0, 3);
        assert_eq!(score("T9", "Bypass", &[], &w).unwrap().0, 4);
        assert_eq!(score("T9", "Sign in", &["passkeys"], &w).unwrap().0, 5);
        assert_eq!(score("T9", "Sign in", &["webapp"], &w), None);
    }

    #[test]
    fn every_word_must_match_and_each_is_marked() {
        let w = words("sign key");
        let (_, hl) = score("T2", "Sign in with a passkey", &[], &w).unwrap();
        assert_eq!(hl, vec![0..4, 19..22]);
        assert_eq!(score("T2", "Sign in", &[], &w), None);
    }

    fn sample() -> (Value, Vec<Value>, Value) {
        let g3 = json!({"id": 3, "name": "Passkeys"});
        let t2 = json!({"id": 2, "title": "Sign in with a passkey", "project": "webapp", "status": "done", "summary": "Shipped", "goal": g3.clone(), "wave": 1});
        let t3 = json!({"id": 3, "title": "Settings page: manage passkeys", "project": "webapp", "status": "queued", "goal": g3, "wave": 2});
        let t7 = json!({"id": 7, "title": "Fix the API shell", "project": "api", "status": "queued"});
        let tasks = json!({"tasks": vec![t2, t3, t7]});
        let goals = vec![json!({"id": 3, "name": "Sign-in with passkeys", "project": "webapp", "done": 1, "total": 3, "active": 1})];
        let s1 = json!({"id": "s1", "name": "api shell", "project": "api", "status": "idle", "idle_secs": 720});
        let s2 = json!({"id": "s2", "name": "T3 Settings", "project": "webapp", "status": "needs", "task_ref": "T3", "branch": "passkeys-settings"});
        let sessions = json!({"sessions": vec![s1, s2]});
        (tasks, goals, sessions)
    }

    #[test]
    fn groups_tasks_goals_then_terminals_with_their_counts() {
        let (tasks, goals, sessions) = sample();
        let g = results("pass", Some(&tasks), &goals, None, Some(&sessions));
        assert_eq!(g.iter().map(|g| (g.kind, g.total)).collect::<Vec<_>>(), vec![(Kind::Tasks, 2), (Kind::Goals, 1), (Kind::Terminals, 1)]);
        // Both are a word inside the title; the one still to do comes first.
        assert_eq!(titles(&g[0]), vec!["Settings page: manage passkeys", "Sign in with a passkey"]);
        assert_eq!(g[0].hits[0].place, "G3 · wave 2");
        assert_eq!(g[0].hits[0].target, Target::Task("T3".into()));
        assert_eq!(g[1].hits[0].r, "G3");
        assert_eq!(g[1].hits[0].dot, Dot::Goal);
        // The terminal matched on its branch: no mark in its title.
        assert_eq!(g[2].hits[0].title, "webapp · T3 Settings");
        assert!(g[2].hits[0].hl.is_empty());
        assert_eq!(g[2].hits[0].place, "T3 · needs you");
        assert_eq!(g[2].hits[0].target, Target::Terminal("s2".into()));
    }

    #[test]
    fn a_ref_finds_its_task_first() {
        let (tasks, goals, sessions) = sample();
        let g = results("t3", Some(&tasks), &goals, None, Some(&sessions));
        assert_eq!(g[0].hits[0].r, "T3");
        assert_eq!(g[0].total, 1);
    }

    #[test]
    fn nothing_typed_shows_open_tasks_pinned_goals_and_every_terminal() {
        let (tasks, goals, sessions) = sample();
        let g = results("", Some(&tasks), &goals, Some(&backlog()), Some(&sessions));
        assert_eq!(titles(&g[0]), vec!["Fix the API shell", "Settings page: manage passkeys"]);
        assert_eq!(g[1].total, 1);
        // The one that needs you first; an idle one says for how long.
        assert_eq!(titles(&g[2]), vec!["webapp · T3 Settings", "api · api shell"]);
        assert_eq!(g[2].hits[1].place, "idle 12 min");
        assert_eq!(g[2].hits[1].dot, Dot::Idle);
    }

    #[test]
    fn no_match_leaves_out_the_kind_and_lists_load_late() {
        let (tasks, goals, _) = sample();
        let g = results("shell", Some(&tasks), &goals, None, None);
        assert_eq!(g.iter().map(|g| g.kind).collect::<Vec<_>>(), vec![Kind::Tasks]);
        assert!(results("zzz", Some(&tasks), &goals, None, None).is_empty());
    }

    fn backlog() -> Value {
        let b4 = json!({"id": 4, "ref": "B4", "title": "Passkey prompt flickers", "kind": "bug", "project": "webapp", "state": "open", "goal": {"id": 3, "ref": "G3", "name": "Sign-in with passkeys"}});
        let b5 = json!({"id": 5, "ref": "B5", "title": "Old passkey copy", "kind": "clean", "project": "webapp", "state": "drop"});
        let b6 = json!({"id": 6, "ref": "B6", "title": "Retry the shell", "kind": "follow", "project": "api", "state": "open", "detail": "After a passkey login fails"});
        json!({"issues": vec![b6, b5, b4]})
    }

    #[test]
    fn backlog_issues_open_first_and_open_their_panel() {
        let (tasks, goals, sessions) = sample();
        let g = results("passkey", Some(&tasks), &goals, Some(&backlog()), Some(&sessions));
        assert_eq!(g.iter().map(|g| g.kind).collect::<Vec<_>>(), vec![Kind::Tasks, Kind::Goals, Kind::Backlog, Kind::Terminals]);
        let b = &g[2];
        // Title matches before the one found in its detail; of those, the open one first.
        assert_eq!(titles(b), vec!["Passkey prompt flickers", "Old passkey copy", "Retry the shell"]);
        assert_eq!(b.hits[0].target, Target::Issue("B4".into()));
        assert_eq!(b.hits[0].place, "Bug · G3");
        assert_eq!((b.hits[1].place.as_str(), b.hits[1].dot), ("Won’t do", Dot::Issue { open: false }));
        assert_eq!(b.hits[2].place, "Follow-up · api");
        assert_eq!(results("b6", None, &[], Some(&backlog()), None)[0].hits[0].r, "B6");
    }

    #[test]
    fn a_group_shows_a_few_and_counts_all() {
        let many: Vec<Value> = (1..=9).map(|n| json!({"id": n, "title": format!("Task {n}"), "status": "queued"})).collect();
        let g = results("task", Some(&json!({"tasks": many})), &[], None, None);
        assert_eq!((g[0].total, g[0].hits.len()), (9, PER_GROUP));
        assert_eq!(row_child(&g, 0), 1);
        assert_eq!(row_child(&g, 5), 6);
    }

    #[gpui_kit::test]
    fn a_burst_of_keys_asks_the_board_once_it_pauses(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = crate::parity::window(cx);
        w.update(cx, |m, window, cx| open(m, window, cx)).unwrap();
        crate::parity::settle(cx);
        rec.take_gets();
        for q in ["s", "si", "sig"] {
            w.update(cx, |m, _, cx| m.search.input.as_ref().unwrap().set_text(q, cx)).unwrap();
            cx.executor().advance_clock(PAUSE / 2);
        }
        crate::parity::settle(cx);
        assert!(rec.take_gets().is_empty(), "nothing asked while typing");
        cx.executor().advance_clock(PAUSE);
        crate::parity::settle(cx);
        assert_eq!(rec.take_gets(), ["search"], "one ask, for search alone");
        w.update(cx, |m, _, _| assert_eq!(m.search.found.as_ref().map(|f| f.0.as_str()), Some("sig"))).unwrap();
    }

    #[gpui_kit::test]
    fn an_answer_for_older_text_is_dropped(cx: &mut gpui_kit::TestAppContext) {
        let (w, _) = crate::parity::window(cx);
        w.update(cx, |m, window, cx| open(m, window, cx)).unwrap();
        w.update(cx, |m, _, cx| m.search.input.as_ref().unwrap().set_text("sig", cx)).unwrap();
        cx.executor().advance_clock(PAUSE);
        crate::parity::settle(cx);
        w.update(cx, |m, _, cx| {
            land(m, "si", json!({"tasks": [], "issues": [], "tasks_total": 0, "issues_total": 0}), cx);
            assert_eq!(m.search.found.as_ref().map(|f| f.0.as_str()), Some("sig"));
            assert!(m.data.tasks.as_ref().is_some_and(|t| t["tasks"].as_array().is_some_and(|a| !a.is_empty())), "the answer for sig stays");
        })
        .unwrap();
    }
}
