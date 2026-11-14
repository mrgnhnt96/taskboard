//! Search (⌘K and the sidebar's search box): a palette over the page that finds projects, tasks,
//! goals, ideas, backlog issues, attachments and terminals as you type. One row is highlighted at a
//! time: ↑ ↓ and the pointer move it, ↩ opens it on its own page, ⌘↩ opens a task's pull request,
//! esc closes. Each kind shows five rows and a "Show all" row that opens the rest in place.
//! Opened on a goal's page it searches that goal alone (its tasks' detail and summary too); ⌫ in
//! the empty field, the chip's ×, or the last row widens it to everything.
use crate::app::MainWindow;
use crate::fmt::{self, arr, i, opt_s, s};
use crate::theme::Theme;
use crate::ui::goals_page::{GoalGroup, goal_group};
use crate::ui::modals::Modal;
use crate::ui::projects::{self, ProjectRow};
use crate::ui::status::{self, St};
use crate::ui::text_input::FieldChanged;
use crate::ui::{issue_panel, kit, sidebar};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

/// Rows shown per kind until its "Show all" is opened; the group's header counts them all.
const PER_GROUP: usize = 5;
/// The tasks, issues, ideas and attachments asked of the board per query, best first; the palette ranks them again.
const WINDOW: usize = 30;
/// Asked instead once a board-ranked kind's "Show all" is open.
const ALL: usize = 2000;
/// How long typing pauses before the board is asked about it, so a burst of keys asks once.
const PAUSE: Duration = Duration::from_millis(120);

// Each row's height, fixed so the list knows how tall it is before drawing it.
const HEAD_H: f32 = 30.;
const ROW_H: f32 = 40.;
const WHY_ROW_H: f32 = 54.;
const MORE_H: f32 = 36.;

#[derive(Default)]
pub struct State {
    pub input: Option<kit::Input>,
    /// The highlighted row, counted over the rows that can be picked.
    pub sel: usize,
    /// The results list: only the rows in view are drawn.
    list: Option<ListState>,
    /// The rows last drawn.
    view: Option<Rc<View>>,
    /// What the board last answered: the ask and how many of each kind match it in all.
    found: Option<Found>,
    /// The attachments and ideas the board found for it, best first.
    atts: Option<Value>,
    ideas: Option<Value>,
    /// The ask waiting for typing to pause; the next key drops it and waits again.
    soon: Option<Task<()>>,
    /// The goal searched alone, when ⌘K opened on its page.
    pub scope: Option<Scope>,
    /// The kind whose "Show all" is open.
    pub all: Option<Kind>,
    /// When the board stopped answering searches; its next answer clears it.
    failed: Option<String>,
}

struct Found {
    ask: Ask,
    tasks: usize,
    issues: usize,
    atts: usize,
    ideas: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scope {
    pub id: i64,
    /// G12.
    pub r: String,
    pub name: String,
}

/// What's asked of the board: the query, the goal it's kept to and how many of each kind to send.
pub type Ask = (String, Option<i64>, usize);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Projects,
    Tasks,
    Goals,
    Ideas,
    Backlog,
    Attachments,
    Terminals,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Projects => "Projects",
            Kind::Tasks => "Tasks",
            Kind::Goals => "Goals",
            Kind::Ideas => "Ideas",
            Kind::Backlog => "Backlog",
            Kind::Attachments => "Attachments",
            Kind::Terminals => "Terminals",
        }
    }

    /// "Show all 12 tasks", "Show all 7 backlog issues".
    fn many(self) -> &'static str {
        match self {
            Kind::Projects => "projects",
            Kind::Tasks => "tasks",
            Kind::Goals => "goals",
            Kind::Ideas => "ideas",
            Kind::Backlog => "backlog issues",
            Kind::Attachments => "attachments",
            Kind::Terminals => "terminals",
        }
    }

    /// Ranked by the board, which sends only its best few unless asked for them all.
    fn from_board(self) -> bool {
        matches!(self, Kind::Tasks | Kind::Backlog | Kind::Ideas | Kind::Attachments)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Its name.
    Project(String),
    Task(String),
    Goal(String),
    Idea(String),
    Issue(String),
    /// Its link or file path.
    Attachment(String),
    Terminal(String),
}

/// The colour of a status pill: a task status's own, or one of the board's named tones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tone {
    St(St),
    Named(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub target: Target,
    /// T12 / G3 / I4 / B7; empty for a project, an attachment or a terminal.
    pub r: String,
    pub title: String,
    /// The matched spans of `title` (byte ranges).
    pub hl: Vec<Range<usize>>,
    /// Its status in words.
    pub status: Option<(String, Tone)>,
    /// The muted text on the right: its goal, else its project.
    pub place: String,
    /// Under the title, when the words were found only in its description or details.
    pub why: Option<&'static str>,
    /// A task's pull request, for ⌘↩.
    pub pr: Option<String>,
}

impl Hit {
    fn new(target: Target, r: String, title: String, hl: Vec<Range<usize>>) -> Hit {
        Hit { target, r, title, hl, status: None, place: String::new(), why: None, pr: None }
    }
}

#[derive(Debug, PartialEq)]
pub struct Group {
    pub kind: Kind,
    pub total: usize,
    pub hits: Vec<Hit>,
    /// Said in the header instead of rows: why the kind can't be shown.
    pub note: Option<&'static str>,
}

/// What the palette searches.
#[derive(Clone, Copy, Default)]
pub struct Sources<'a> {
    pub tasks: Option<&'a Value>,
    pub goals: &'a [Value],
    pub ideas: Option<&'a Value>,
    pub backlog: Option<&'a Value>,
    pub atts: Option<&'a Value>,
    pub sessions: Option<&'a Value>,
}

// ================================================================== matching

#[cfg(test)]
use taskboardd::palette::find_ci;
use taskboardd::palette::{score, words};

type Match = Option<(u8, Vec<Range<usize>>)>;

/// Ranked hits of one kind, best first, then in each object's own order.
fn rank<T>(rows: Vec<(T, Match)>, words: &[String], order: impl Fn(&T) -> (u8, i64)) -> Vec<(T, Vec<Range<usize>>)> {
    let mut v: Vec<(u8, T, Vec<Range<usize>>)> = rows.into_iter().filter_map(|(t, m)| if words.is_empty() { Some((0, t, vec![])) } else { m.map(|(sc, hl)| (sc, t, hl)) }).collect();
    v.sort_by_key(|(sc, t, _)| (*sc, order(t)));
    v.into_iter().map(|(_, t, hl)| (t, hl)).collect()
}

/// The ref, title and `other` words first; failing those, `deeper` too, and then `why` says so.
fn score_why(r: &str, title: &str, other: &[&str], deeper: &[&str], w: &[String], why: &'static str) -> (Match, Option<&'static str>) {
    if let Some(m) = score(r, title, other, w) {
        return (Some(m), None);
    }
    let all: Vec<&str> = other.iter().chain(deeper).copied().collect();
    match score(r, title, &all, w) {
        Some(m) => (Some(m), Some(why)),
        None => (None, None),
    }
}

fn group(kind: Kind, hits: Vec<Hit>, all: Option<Kind>) -> Group {
    let shown = if all == Some(kind) { hits.len() } else { PER_GROUP };
    Group { kind, total: hits.len(), hits: hits.into_iter().take(shown).collect(), note: None }
}

/// One task the board found.
struct Item {
    id: i64,
    r: String,
    title: String,
    project: String,
    /// (ref, name) of its goal.
    goal: Option<(String, String)>,
    wave: Option<i64>,
    detail: String,
    summary: String,
    /// Still to do: not done, and not failed once finished (a lost terminal can still be restarted).
    open: bool,
    row: status::Row,
    pr: Option<String>,
}

fn items(v: &Value) -> Vec<Item> {
    arr(v, "tasks")
        .iter()
        .map(|c| {
            let row = status::row(c);
            let open = row.st != St::Done && !(row.st == St::Failed && s(c, "status") == "done");
            let g = &c["goal"];
            let pr = fmt::obj(c, "pr").filter(|p| p.get("num").is_some_and(|n| !n.is_null())).and_then(|p| opt_s(p, "url")).map(str::to_string);
            Item {
                id: i(c, "id"),
                r: fmt::ref_of(c, "T"),
                title: s(c, "title").to_string(),
                project: s(c, "project").to_string(),
                goal: g.is_object().then(|| (fmt::ref_of(g, "G"), s(g, "name").to_string())),
                wave: c["wave"].as_i64(),
                detail: s(c, "detail").to_string(),
                summary: s(c, "summary").to_string(),
                open,
                row,
                pr,
            }
        })
        .collect()
}

/// A task's own words are its ref and title, then its project and goal; failing those, its detail
/// and summary, which the row then says. Within a goal (`scoped`) the goal's name would match
/// every task, so it's left out.
fn task_hits(tasks: &Value, w: &[String], scoped: bool) -> Vec<Hit> {
    let items = items(tasks);
    // With nothing typed: the tasks still to do, those that need you first.
    let items: Vec<Item> = if w.is_empty() { items.into_iter().filter(|it| it.open).collect() } else { items };
    let rows = items
        .into_iter()
        .map(|it| {
            let gname = it.goal.as_ref().map(|g| g.1.as_str()).unwrap_or("");
            let other: Vec<&str> = if scoped { vec![] } else { vec![&it.project, gname] };
            let (m, why) = score_why(&it.r, &it.title, &other, &[&it.detail, &it.summary], w, "Matched in its description");
            ((it, why), m)
        })
        .collect();
    rank(rows, w, |(it, _)| ((!it.open) as u8 * 10 + it.row.st.rank(), -it.id))
        .into_iter()
        .map(|((it, why), hl)| {
            let place = match (&it.goal, it.wave) {
                (Some(_), Some(n)) if scoped => format!("wave {n}"),
                (Some((_, name)), _) if !name.is_empty() => name.clone(),
                (Some((g, _)), _) => g.clone(),
                (None, _) => it.project.clone(),
            };
            let st = it.row.st;
            Hit { status: Some((st.label().into(), Tone::St(st))), place, why, pr: it.pr, ..Hit::new(Target::Task(it.r.clone()), it.r, it.title, hl) }
        })
        .collect()
}

/// A goal's status in words, from the counts the goal list carries.
fn goal_status(g: &Value) -> (String, Tone) {
    let (label, tone) = match goal_group(g) {
        GoalGroup::Finished => ("Done", Tone::St(St::Done)),
        GoalGroup::Deprioritized => ("Deprioritized", Tone::Named("muted")),
        GoalGroup::Paused => ("Paused", Tone::Named("warn")),
        _ if i(g, "needs") > 0 => ("Needs you", Tone::St(St::Needs)),
        _ if i(g, "active") > 0 => ("Working", Tone::St(St::Working)),
        GoalGroup::Next => ("Up next", Tone::Named("goal")),
        GoalGroup::Active if i(g, "queued") > 0 => ("Queued", Tone::St(St::Queued)),
        GoalGroup::Active => ("In progress", Tone::Named("muted")),
    };
    (label.into(), tone)
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
            let place = if i(g, "total") > 0 { format!("{} of {} done", i(g, "done"), i(g, "total")) } else { s(g, "project").to_string() };
            Hit { status: Some(goal_status(g)), place, ..Hit::new(Target::Goal(r.clone()), r, s(g, "name").to_string(), hl) }
        })
        .collect()
}

/// Ideas only once something is typed: the open ones first, newest first.
fn idea_hits(ideas: &Value, w: &[String]) -> Vec<Hit> {
    if w.is_empty() {
        return vec![];
    }
    let rows = ideas
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|x| {
            let r = fmt::ref_of(x, "I");
            let (m, why) = score_why(&r, s(x, "title"), &[s(x, "project")], &[s(x, "detail")], w, "Matched in details");
            ((x, why), m)
        })
        .collect();
    rank(rows, w, |(x, _)| (matches!(s(x, "state"), "planned" | "shelved" | "dropped") as u8, -i(x, "id")))
        .into_iter()
        .map(|((x, why), hl)| {
            let r = fmt::ref_of(x, "I");
            let (label, tone) = crate::ui::ideas::state_pill(x);
            Hit { status: Some((label.into(), Tone::Named(tone))), place: s(x, "project").to_string(), why, ..Hit::new(Target::Idea(r.clone()), r, s(x, "title").to_string(), hl) }
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
            let (m, why) = score_why(&r, s(b, "title"), &[s(b, "project"), s(&b["goal"], "name"), s(b, "area")], &[s(b, "detail"), s(b, "said")], w, "Matched in details");
            ((b, why), m)
        })
        .collect();
    rank(rows, w, |(b, _)| (issue_panel::issue_state(b).is_some() as u8, -i(b, "id")))
        .into_iter()
        .map(|((b, why), hl)| {
            let r = fmt::ref_of(b, "B");
            let status = match issue_panel::issue_state(b) {
                Some((short, _)) => short,
                None => issue_panel::kind_label(s(b, "kind")),
            };
            let place = opt_s(&b["goal"], "name").unwrap_or(s(b, "project")).to_string();
            Hit { status: Some((status, Tone::Named("muted"))), place, why, ..Hit::new(Target::Issue(r.clone()), r, s(b, "title").to_string(), hl) }
        })
        .collect()
}

/// Attachments only once something is typed, as the board ranked them. Each says its kind and
/// whose it is: a task's, else (outside the goal it's kept to) a goal's.
fn attachment_hits(atts: &Value, w: &[String], scoped: bool) -> Vec<Hit> {
    if w.is_empty() {
        return vec![];
    }
    let rows = atts
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|a| (a, score("", s(a, "title"), &[s(a, "summary"), s(a, "kind"), s(a, "url")], w)))
        .collect();
    rank(rows, w, |a| (0, -i(a, "id")))
        .into_iter()
        .map(|(a, hl)| {
            let place = match (opt_s(a, "task"), opt_s(a, "goal")) {
                (Some(t), _) => t.to_string(),
                (None, Some(g)) if !scoped => g.to_string(),
                _ => String::new(),
            };
            let status = Some((capitalized(s(a, "kind")), Tone::Named("muted")));
            Hit { status, place, ..Hit::new(Target::Attachment(s(a, "url").to_string()), String::new(), s(a, "title").to_string(), hl) }
        })
        .collect()
}

/// "design" → "Design".
fn capitalized(w: &str) -> String {
    let mut c = w.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Its order (needs you, working, idle) and its status: a pill, else how long it's been idle.
fn terminal_status(x: &Value) -> (u8, Option<(String, Tone)>, String) {
    match s(x, "status") {
        "needs" | "offline" => (0, Some(("Needs you".into(), Tone::St(St::Needs))), String::new()),
        "working" | "waiting" => (1, Some(("Working".into(), Tone::St(St::Working))), String::new()),
        _ => (2, None, format!("idle {}", long_ago(idle_ms(x)))),
    }
}

/// Since `last_activity` (else `seen_at`), 0 when neither parses. The board's `idle_secs` wins
/// when it sends one: it leaves out the time the Mac slept.
fn idle_ms(x: &Value) -> i64 {
    if let Some(secs) = x.get("idle_secs").and_then(|v| v.as_i64()) {
        return secs * 1000;
    }
    opt_s(x, "last_activity").or(opt_s(x, "seen_at")).and_then(fmt::parse).map(|t| (fmt::now() - t).num_milliseconds()).unwrap_or(0)
}

/// "under a minute", "12 min", "2 h 5 min", "3 h", "2 days".
fn long_ago(ms: i64) -> String {
    let m = (ms as f64 / 60000.).round() as i64;
    if m < 1 {
        return "under a minute".into();
    }
    if m < 60 {
        return format!("{m} min");
    }
    let (h, r) = (m / 60, m % 60);
    if h < 24 {
        return if r > 0 && h < 6 { format!("{h} h {r} min") } else { format!("{h} h") };
    }
    fmt::plural((h as f64 / 24.).round() as i64, "day", "days")
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
            let (_, status, idle) = terminal_status(x);
            let place = match (opt_s(x, "task_ref"), idle.is_empty()) {
                (Some(t), true) => t.to_string(),
                (Some(t), false) => format!("{t} · {idle}"),
                (None, _) => idle,
            };
            Hit { status, place, ..Hit::new(Target::Terminal(s(x, "id").to_string()), String::new(), terminal_title(x), hl) }
        })
        .collect()
}

fn terminal_title(x: &Value) -> String {
    match opt_s(x, "project") {
        Some(p) => format!("{p} · {}", s(x, "name")),
        None => s(x, "name").to_string(),
    }
}

/// A result's ref typed in full: its kind goes first, so ↩ opens it.
fn names_ref(g: &Group, q: &str) -> bool {
    let q = q.trim();
    !q.is_empty() && g.hits.iter().any(|h| h.r.eq_ignore_ascii_case(q))
}

/// The results for `q`: tasks, goals, ideas, backlog issues, attachments, then terminals, leaving
/// out the kinds with none; a ref typed in full puts its kind first. Kept to a goal (`scoped`): its
/// tasks, issues and attachments alone, as the board sent them. `all` is the kind shown in full.
pub fn results(q: &str, src: &Sources, scoped: bool, all: Option<Kind>) -> Vec<Group> {
    let w = words(q);
    let mut groups: Vec<Group> = [
        (Kind::Tasks, src.tasks.map(|v| task_hits(v, &w, scoped)).unwrap_or_default()),
        (Kind::Goals, if scoped { vec![] } else { goal_hits(src.goals, &w) }),
        (Kind::Ideas, src.ideas.filter(|_| !scoped).map(|v| idea_hits(v, &w)).unwrap_or_default()),
        (Kind::Backlog, src.backlog.map(|v| backlog_hits(v, &w)).unwrap_or_default()),
        (Kind::Attachments, src.atts.map(|v| attachment_hits(v, &w, scoped)).unwrap_or_default()),
        (Kind::Terminals, src.sessions.filter(|_| !scoped).map(|v| terminal_hits(v, &w)).unwrap_or_default()),
    ]
    .into_iter()
    .map(|(k, hits)| group(k, hits, all))
    .filter(|g| !g.hits.is_empty())
    .collect();
    groups.sort_by_key(|g| !names_ref(g, q));
    groups
}

/// Projects by name, those with open work first; the right says what's open, else how many goals finished.
fn project_hits(rows: Vec<ProjectRow>, w: &[String]) -> Vec<Hit> {
    let rows = rows
        .into_iter()
        .map(|p| {
            let m = score("", &p.name, &[], w);
            (p, m)
        })
        .collect();
    rank(rows, w, |_| (0, 0))
        .into_iter()
        .map(|(p, hl)| {
            let place = match (p.open > 0 || p.tasks > 0, p.finished) {
                (true, _) => p.sub(),
                (false, 0) => String::new(),
                (false, n) => format!("{n} done"),
            };
            Hit { place, ..Hit::new(Target::Project(p.name.clone()), String::new(), p.name, hl) }
        })
        .collect()
}

/// The projects matching `q`, on top so one isn't pushed out of sight by the tasks that merely
/// belong to it; a ref typed in full keeps its result first, for ↩.
fn add_projects(groups: &mut Vec<Group>, rows: Vec<ProjectRow>, q: &str, all: Option<Kind>) {
    let w = words(q);
    if w.is_empty() {
        return;
    }
    let found = group(Kind::Projects, project_hits(rows, &w), all);
    if found.hits.is_empty() {
        return;
    }
    let at = groups.iter().position(|g| names_ref(g, q)).map_or(0, |g| g + 1);
    groups.insert(at, found);
}

const MIDNA_DOWN: &str = "Midna isn’t running, so terminals can’t be shown";

fn current(m: &MainWindow, cx: &App) -> Vec<Group> {
    let q = typed(m, cx);
    let scoped = m.search.scope.is_some();
    let src = Sources {
        tasks: m.data.tasks.as_ref(),
        goals: &m.data.goals[..],
        ideas: m.search.ideas.as_ref(),
        backlog: m.data.backlog.as_ref(),
        atts: m.search.atts.as_ref(),
        sessions: m.data.sessions.as_deref(),
    };
    let mut groups = results(&q, &src, scoped, m.search.all);
    if !scoped {
        let (open, idle) = projects::list(m.state(), &m.data.goals);
        add_projects(&mut groups, open.into_iter().chain(idle).collect(), &q, m.search.all);
    }
    // The board sent the best few: its counts are the whole.
    if let Some(f) = m.search.found.as_ref().filter(|f| answers(m, f, &q)) {
        for g in &mut groups {
            match g.kind {
                Kind::Tasks => g.total = g.total.max(f.tasks),
                Kind::Backlog => g.total = g.total.max(f.issues),
                Kind::Attachments => g.total = g.total.max(f.atts),
                Kind::Ideas => g.total = g.total.max(f.ideas),
                _ => {}
            }
        }
    }
    // Rows for terminals Midna can't show would only repeat the tasks.
    if m.state()["midna"]["up"] == Value::Bool(false) {
        for g in groups.iter_mut().filter(|g| g.kind == Kind::Terminals) {
            g.hits.clear();
            g.note = Some(MIDNA_DOWN);
        }
    }
    groups
}

fn typed(m: &MainWindow, cx: &App) -> String {
    m.search.input.as_ref().map(|i| i.text(cx)).unwrap_or_default()
}

/// The board's answer is for `q`, in the goal searched now.
fn answers(m: &MainWindow, f: &Found, q: &str) -> bool {
    f.ask.0 == q && f.ask.1 == m.search.scope.as_ref().map(|s| s.id)
}

fn answered(m: &MainWindow, q: &str) -> bool {
    m.search.found.as_ref().is_some_and(|f| answers(m, f, q))
}

/// What to ask the board about while the palette is open: what's typed, the goal it's kept to,
/// and how many of each kind (all of them once a board-ranked kind's "Show all" is open).
pub fn query(m: &MainWindow, cx: &App) -> Option<Ask> {
    let limit = if m.search.all.is_some_and(Kind::from_board) { ALL } else { WINDOW };
    m.search.input.as_ref().filter(|_| is_open(m)).map(|i| (i.text(cx), m.search.scope.as_ref().map(|s| s.id), limit))
}

/// `GET /search?palette=1`: the best tasks, backlog issues, ideas and attachments for the ask, and how many match.
pub fn ask(be: &dyn crate::backend::Backend, (q, goal, limit): &Ask) -> Result<Value, String> {
    let mut args = vec![("palette", "1".to_string()), ("q", q.to_string()), ("limit", limit.to_string())];
    if let Some(g) = goal {
        args.push(("goal", g.to_string()));
    }
    be.get("search", &args).map_err(|e| e.message)
}

/// Keep the board's answer for `a` if it's still what's asked, in the same goal.
pub fn land(m: &mut MainWindow, a: &Ask, v: Value, cx: &App) {
    m.search.failed = None;
    if query(m, cx).as_ref() != Some(a) {
        return;
    }
    let total = |k: &str| v[k].as_u64().unwrap_or(0) as usize;
    m.search.found = Some(Found { ask: a.clone(), tasks: total("tasks_total"), issues: total("issues_total"), atts: total("attachments_total"), ideas: total("ideas_total") });
    m.search.atts = Some(v["attachments"].clone());
    m.search.ideas = Some(v["ideas"].clone());
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
            match v {
                Ok(v) => land(m, &q, v, cx),
                Err(_) => {
                    m.search.failed.get_or_insert_with(|| fmt::device_now().to_rfc3339());
                }
            }
            cx.notify();
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

// ================================================================== rows

/// One row of the results list.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Line {
    /// A kind's header, by group.
    Head(usize),
    /// (group, hit).
    Hit(usize, usize),
    /// "Show all N tasks", by group.
    More(usize),
    /// Within a goal: "Search everything for “…”".
    Every,
}

#[derive(Debug, PartialEq)]
struct View {
    groups: Vec<Group>,
    lines: Vec<Line>,
    /// The lines that can be highlighted and opened, in order.
    picks: Vec<usize>,
}

impl View {
    fn new(groups: Vec<Group>, all: Option<Kind>, scoped: bool) -> View {
        let mut lines = vec![];
        for (gi, g) in groups.iter().enumerate() {
            lines.push(Line::Head(gi));
            lines.extend((0..g.hits.len()).map(|h| Line::Hit(gi, h)));
            if g.note.is_none() && g.hits.len() < g.total && all != Some(g.kind) {
                lines.push(Line::More(gi));
            }
        }
        if scoped {
            lines.push(Line::Every);
        }
        let picks = lines.iter().enumerate().filter(|(_, l)| !matches!(l, Line::Head(_))).map(|(ix, _)| ix).collect();
        View { groups, lines, picks }
    }

    fn height(&self, l: &Line) -> f32 {
        match l {
            Line::Head(_) => HEAD_H,
            Line::Hit(g, h) if self.groups[*g].hits[*h].why.is_some() => WHY_ROW_H,
            Line::Hit(..) | Line::Every => ROW_H,
            Line::More(_) => MORE_H,
        }
    }

    fn hit(&self, k: usize) -> Option<&Hit> {
        match self.lines.get(*self.picks.get(k)?)? {
            Line::Hit(g, h) => Some(&self.groups[*g].hits[*h]),
            _ => None,
        }
    }
}

/// The rows for what's typed now, kept for the list to draw; the list starts over when they change.
fn refresh_view(m: &mut MainWindow, cx: &App) -> Rc<View> {
    let v = View::new(current(m, cx), m.search.all, m.search.scope.is_some());
    if let Some(old) = m.search.view.as_ref().filter(|old| ***old == v) {
        return old.clone();
    }
    let v = Rc::new(v);
    let n = v.lines.len();
    match &m.search.list {
        Some(l) => {
            l.reset(n);
            if let Some(&line) = v.picks.get(m.search.sel) {
                l.scroll_to_reveal_item(line);
            }
        }
        None => m.search.list = Some(ListState::new(n, ListAlignment::Top, px(400.))),
    }
    m.search.view = Some(v.clone());
    v
}

// ================================================================== actions

pub fn is_open(m: &MainWindow) -> bool {
    matches!(m.modal, Some(Modal::Search))
}

/// ⌘K: open the palette with an empty query (again: back to the field).
pub fn open(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    if !is_open(m) {
        let scope = goal_scope(m);
        let input = kit::Input::new(cx, placeholder(scope.is_some()), false);
        cx.subscribe(&input.field, |m: &mut MainWindow, _, _: &FieldChanged, cx| {
            m.search.sel = 0;
            m.search.all = None;
            fetch_soon(m, cx);
            cx.notify();
        })
        .detach();
        m.search = State { input: Some(input), scope, ..Default::default() };
        m.set_modal(Some(Modal::Search), window, cx);
        // The terminal list loads with the palette, the tasks and issues for what's typed.
        m.refresh(cx);
        fetch(m, cx);
    }
    if let Some(i) = &m.search.input {
        window.focus(&i.focus, cx);
    }
}

fn placeholder(scoped: bool) -> &'static str {
    if scoped { "Search this goal’s tasks, backlog and attachments" } else { "Search tasks, goals, ideas, backlog and terminals" }
}

/// The goal whose page is showing, to keep the palette to.
fn goal_scope(m: &MainWindow) -> Option<Scope> {
    let crate::app::Page::Goal(r) = &m.page else { return None };
    let id = r.trim_start_matches(['G', 'g']).parse().ok()?;
    let same = |g: &&Value| fmt::ref_of(g, "G") == *r;
    let name = m.data.goals.iter().find(same).or(m.data.goal.as_deref().filter(same)).map(|g| s(g, "name").to_string()).unwrap_or_default();
    Some(Scope { id, r: r.clone(), name })
}

/// Search everything again, keeping what's typed.
fn widen(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    if m.search.scope.take().is_none() {
        return;
    }
    if let Some(i) = &m.search.input {
        i.field.update(cx, |f, _| f.placeholder = placeholder(false).into());
    }
    m.search.sel = 0;
    m.search.all = None;
    // The goal's tasks aren't the board's best for everything: drop them and ask again.
    m.data.tasks = None;
    m.data.backlog = None;
    m.search.atts = None;
    m.search.ideas = None;
    m.refresh(cx);
    fetch(m, cx);
    cx.notify();
}

/// "Show all": the kind's every match, in place, asking the board for the rest of a kind it ranks.
fn show_all(m: &mut MainWindow, kind: Kind, cx: &mut Context<MainWindow>) {
    m.search.all = Some(kind);
    if kind.from_board() {
        fetch(m, cx);
    }
    cx.notify();
}

fn retry(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    m.search.failed = None;
    fetch(m, cx);
    cx.notify();
}

pub fn close(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    m.search.input = None;
    m.set_modal(None, window, cx);
}

fn go(m: &mut MainWindow, target: Target, window: &mut Window, cx: &mut Context<MainWindow>) {
    close(m, window, cx);
    match target {
        Target::Project(name) => projects::open_for(m, &name, cx),
        Target::Task(r) => m.open_task(r, cx),
        Target::Goal(r) => crate::ui::goal::open(m, &r, false, cx),
        Target::Idea(r) => crate::ui::idea::open(m, &r, cx),
        Target::Issue(r) => m.open_issue(r, cx),
        Target::Attachment(url) => crate::ui::goal::open_attachment(&url, cx),
        Target::Terminal(id) => m.show_in_midna(&id, cx),
    }
}

/// Open pick `k`: a result, a "Show all" row, or the row that searches everything.
fn pick(m: &mut MainWindow, k: usize, window: &mut Window, cx: &mut Context<MainWindow>) {
    let v = refresh_view(m, cx);
    let Some(line) = v.picks.get(k).and_then(|&l| v.lines.get(l)).copied() else { return };
    match line {
        Line::Hit(g, h) => go(m, v.groups[g].hits[h].target.clone(), window, cx),
        Line::More(g) => show_all(m, v.groups[g].kind, cx),
        Line::Every => widen(m, cx),
        Line::Head(_) => {}
    }
}

/// Highlight pick `k`, from the keys or the pointer: the one highlight there is.
fn select(m: &mut MainWindow, k: usize, reveal: bool, cx: &mut Context<MainWindow>) {
    if m.search.sel == k {
        return;
    }
    m.search.sel = k;
    if reveal && let (Some(l), Some(v)) = (&m.search.list, &m.search.view) {
        // The first row brings its group's header along.
        let line = if k == 0 { 0 } else { v.picks.get(k).copied().unwrap_or(0) };
        l.scroll_to_reveal_item(line);
    }
    cx.notify();
}

fn on_key(m: &mut MainWindow, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<MainWindow>) {
    let v = refresh_view(m, cx);
    let n = v.picks.len();
    match ev.keystroke.key.as_str() {
        "escape" => {
            cx.stop_propagation();
            close(m, window, cx);
        }
        "down" | "up" if n > 0 => {
            cx.stop_propagation();
            let down = ev.keystroke.key == "down";
            let k = (m.search.sel.min(n - 1) + if down { 1 } else { n - 1 }) % n;
            select(m, k, true, cx);
        }
        "enter" if ev.keystroke.modifiers.platform => {
            cx.stop_propagation();
            if let Some(url) = v.hit(m.search.sel.min(n.saturating_sub(1))).and_then(|h| h.pr.clone()) {
                close(m, window, cx);
                cx.open_url(&url);
            }
        }
        "enter" => {
            cx.stop_propagation();
            pick(m, m.search.sel.min(n.saturating_sub(1)), window, cx);
        }
        _ => {}
    }
}

/// ⌫ in the empty field, before the field takes it: search everything, not just the goal.
fn on_key_capture(m: &mut MainWindow, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<MainWindow>) {
    let empty = m.search.input.as_ref().is_some_and(|i| i.text(cx).is_empty());
    if ev.keystroke.key == "backspace" && m.search.scope.is_some() && empty {
        cx.stop_propagation();
        widen(m, cx);
    }
}

// ================================================================== view

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let Some(input) = m.search.input.as_ref() else { return div().into_any_element() };
    let field = input.field.clone();
    let focus = input.focus.clone();
    let q = input.text(cx);
    let typed = !q.trim().is_empty();
    let v = refresh_view(m, cx);
    let waiting = typed && !answered(m, &q);
    let down = m.search.failed.clone().filter(|_| !answered(m, &q));
    let scoped = m.search.scope.is_some();

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
        .when_some(m.search.scope.clone(), |d, sc| d.child(scope_chip(&t, &sc, cx)))
        .child(div().flex_1().min_w_0().child(field));

    let body: Option<AnyElement> = if let Some(at) = down {
        Some(board_down(&t, &at, cx).into_any_element())
    } else if v.lines.is_empty() || (scoped && v.lines == [Line::Every]) {
        match (typed, waiting) {
            (false, _) => None,
            (true, true) => Some(message(&t, "Searching…").into_any_element()),
            (true, false) => Some(nothing(&t, q.trim(), scoped, cx).into_any_element()),
        }
    } else {
        let content = px(v.lines.iter().map(|l| v.height(l)).sum::<f32>() + 8.);
        // Long results fill the window below the field.
        let room = window.viewport_size().height - px(96. + 60. + 1. + 48.);
        let least = px(ROW_H * 3.);
        let h = if content < room { content } else if room < least { least } else { room };
        let state = m.search.list.clone().unwrap_or_else(|| ListState::new(v.lines.len(), ListAlignment::Top, px(400.)));
        Some(div().id("search-results").flex_none().h(h).pb(px(8.)).child(list(state, cx.processor(list_item)).size_full()).into_any_element())
    };

    // The box's own cap is a share of a parent with no height; the window's room below the top
    // gap is what it may fill.
    let cap = window.viewport_size().height - px(96. + 48.);
    let dialog = kit::modal_box(&t, 640.)
        .max_h(cap)
        .min_h_0()
        .capture_key_down(cx.listener(on_key_capture))
        .on_key_down(cx.listener(on_key))
        .child(head)
        .when_some(body, |d, body| d.child(div().h(px(1.)).flex_none().bg(t.divider)).child(body));

    deferred(
        kit::scrim(&t, "search-scrim")
            .track_focus(&m.modal_focus)
            .flex()
            .flex_col()
            .items_center()
            .pt(px(96.))
            .on_mouse_down(MouseButton::Left, cx.listener(|m, _, window, cx| close(m, window, cx)))
            .child(div().id("search-box").flex().flex_col().min_h_0().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(dialog)),
    )
    .with_priority(1)
    .into_any_element()
}

/// One row of the results list, drawn only while it's in view.
fn list_item(m: &mut MainWindow, ix: usize, _: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let Some(v) = m.search.view.clone() else { return div().into_any_element() };
    let sel = m.search.sel.min(v.picks.len().saturating_sub(1));
    let k = v.picks.iter().position(|&l| l == ix).unwrap_or(usize::MAX);
    let on = k == sel;
    match v.lines.get(ix) {
        Some(Line::Head(g)) => group_head(&t, &v.groups[*g]).into_any_element(),
        Some(Line::Hit(g, h)) => result_row(&t, &v.groups[*g].hits[*h], k, on, cx).into_any_element(),
        Some(Line::More(g)) => {
            let g = &v.groups[*g];
            pick_row(&t, ("search-more", k), k, on, MORE_H, cx).text_size(px(13.)).text_color(t.accent).child(format!("Show all {} {}", g.total, g.kind.many())).into_any_element()
        }
        Some(Line::Every) => {
            let q = typed(m, cx);
            let label = if q.trim().is_empty() { "Search everything".to_string() } else { format!("Search everything for “{}”", q.trim()) };
            pick_row(&t, ("search-every", k), k, on, ROW_H, cx).gap(px(10.)).child(kit::glyph(kit::Glyph::Search, 14., t.muted)).child(div().min_w_0().truncate().child(label)).into_any_element()
        }
        None => div().into_any_element(),
    }
}

/// "TASKS · 12"; for terminals Midna can't show, the reason after it.
fn group_head(t: &Theme, g: &Group) -> Div {
    div()
        .flex()
        .items_end()
        .gap(px(6.))
        .h(px(HEAD_H))
        .px(px(16.))
        .pb(px(4.))
        .text_size(px(11.))
        .text_color(t.muted)
        .child(div().flex_none().font_weight(FontWeight::BOLD).child(format!("{} · {}", g.kind.label().to_uppercase(), g.total)))
        .when_some(g.note, |d, note| d.child(div().min_w_0().truncate().child(format!("· {note}"))))
}

/// A row that can be highlighted and opened; the pointer moves the one highlight onto it.
fn pick_row(t: &Theme, id: impl Into<ElementId>, k: usize, on: bool, h: f32, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(h))
        .px(px(16.))
        .mx(px(6.))
        .rounded(px(8.))
        .text_size(px(14.))
        .text_color(t.text)
        .cursor_pointer()
        .when(on, |d| d.bg(t.accent_soft))
        .on_mouse_move(cx.listener(move |m, _: &MouseMoveEvent, _, cx| select(m, k, false, cx)))
        .on_click(cx.listener(move |m, _, window, cx| pick(m, k, window, cx)))
}

fn result_row(t: &Theme, h: &Hit, k: usize, on: bool, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let mark = HighlightStyle { background_color: Some(t.mark), color: Some(t.mark_fg), ..Default::default() };
    let title = StyledText::new(h.title.clone()).with_highlights(h.hl.iter().map(|r| (r.clone(), mark)));
    let status = h.status.as_ref().map(|(label, tone)| match tone {
        Tone::St(st) => {
            let (fg, bg, _) = st.colors(t);
            kit::pill(fg, bg, label.clone())
        }
        Tone::Named(n) => kit::tone_pill(t, n, label.clone()),
    });
    pick_row(t, ("search-hit", k), k, on, if h.why.is_some() { WHY_ROW_H } else { ROW_H }, cx)
        .gap(px(10.))
        .when(!h.r.is_empty(), |d| d.child(div().flex_none().min_w(px(38.)).font_family(t.mono_font.clone()).text_size(px(12.5)).text_color(t.muted).child(h.r.clone())))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(div().truncate().child(title))
                .when_some(h.why, |d, why| d.child(div().truncate().text_size(px(12.)).text_color(t.muted).child(why))),
        )
        .when_some(status, |d, p| d.child(div().flex_none().child(p)))
        .when(!h.place.is_empty(), |d| d.child(div().flex_none().max_w(px(180.)).truncate().text_size(px(12.)).text_color(t.muted).child(h.place.clone())))
}

fn message(t: &Theme, text: &str) -> Div {
    div().px(px(20.)).py(px(18.)).text_size(px(14.)).text_color(t.muted).child(text.to_string())
}

fn nothing(t: &Theme, q: &str, scoped: bool, cx: &mut Context<MainWindow>) -> Div {
    let place = if scoped { "this goal" } else { "tasks, goals, ideas, backlog or terminals" };
    message(t, &format!("Nothing matches “{q}” in {place}."))
        .flex()
        .flex_col()
        .items_start()
        .gap(px(10.))
        .when(scoped, |d| d.child(kit::btn_small(t, "search-widen", "Search everything").on_click(cx.listener(|m, _, _, cx| widen(m, cx)))))
}

/// The board stopped answering: since when, and a way to ask again.
fn board_down(t: &Theme, at: &str, cx: &mut Context<MainWindow>) -> Div {
    div()
        .flex()
        .flex_col()
        .items_start()
        .gap(px(4.))
        .px(px(20.))
        .py(px(18.))
        .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child("Can’t reach the board"))
        .child(div().text_size(px(13.)).text_color(t.muted).child(format!("It stopped answering at {}.", fmt::hhmm(at))))
        .child(div().pt(px(8.)).child(kit::btn_small(t, "search-retry", "Retry").on_click(cx.listener(|m, _, _, cx| retry(m, cx)))))
}

/// The goal the palette is kept to, by name; its × searches everything.
fn scope_chip(t: &Theme, sc: &Scope, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let label = format!("In {}", if sc.name.is_empty() { &sc.r } else { &sc.name });
    div()
        .id("search-scope")
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .max_w(px(260.))
        .h(px(26.))
        .pl(px(8.))
        .pr(px(4.))
        .rounded(px(6.))
        .bg(t.seg)
        .text_size(px(13.))
        .text_color(t.text)
        .child(div().flex_none().size(px(8.)).rounded(px(2.)).bg(t.goal))
        .child(div().min_w_0().truncate().child(label))
        .child(
            div()
                .id("search-scope-x")
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(18.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(t.border_2))
                .child(kit::glyph(kit::Glyph::X, 10., t.muted))
                .on_click(cx.listener(|m, _, _, cx| {
                    cx.stop_propagation();
                    widen(m, cx);
                })),
        )
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

    fn status(h: &Hit) -> &str {
        h.status.as_ref().map(|s| s.0.as_str()).unwrap_or("")
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
        let t2 = json!({"id": 2, "title": "Sign in with a passkey", "project": "webapp", "status": "done", "summary": "Shipped", "goal": g3.clone(), "wave": 1,
                        "pr": {"num": 41, "state": "MERGED", "url": "https://x.test/pr/41"}});
        let t3 = json!({"id": 3, "title": "Settings page: manage passkeys", "project": "webapp", "status": "queued", "goal": g3, "wave": 2});
        let t7 = json!({"id": 7, "title": "Fix the API shell", "project": "api", "status": "queued", "detail": "The passkey login drops the shell"});
        let tasks = json!({"tasks": vec![t2, t3, t7]});
        let goals = vec![json!({"id": 3, "name": "Sign-in with passkeys", "project": "webapp", "done": 1, "total": 3, "active": 1})];
        let s1 = json!({"id": "s1", "name": "api shell", "project": "api", "status": "idle", "idle_secs": 720});
        let s2 = json!({"id": "s2", "name": "T3 Settings", "project": "webapp", "status": "needs", "task_ref": "T3", "branch": "passkeys-settings"});
        let sessions = json!({"sessions": vec![s1, s2]});
        (tasks, goals, sessions)
    }

    fn src<'a>(tasks: &'a Value, goals: &'a [Value], sessions: &'a Value) -> Sources<'a> {
        Sources { tasks: Some(tasks), goals, sessions: Some(sessions), ..Default::default() }
    }

    #[test]
    fn rows_say_their_status_in_words_and_their_goal() {
        let (tasks, goals, sessions) = sample();
        let g = results("pass", &src(&tasks, &goals, &sessions), false, None);
        assert_eq!(g.iter().map(|g| (g.kind, g.total)).collect::<Vec<_>>(), vec![(Kind::Tasks, 3), (Kind::Goals, 1), (Kind::Terminals, 1)]);
        // Both are a word inside the title; the one still to do comes first. The one found only
        // in its description is last and says so.
        assert_eq!(titles(&g[0]), vec!["Settings page: manage passkeys", "Sign in with a passkey", "Fix the API shell"]);
        let t = &g[0].hits;
        assert_eq!((status(&t[0]), t[0].place.as_str(), t[0].why), ("Queued", "Passkeys", None));
        assert_eq!((status(&t[1]), t[1].pr.as_deref()), ("Done", Some("https://x.test/pr/41")));
        assert_eq!((t[2].place.as_str(), t[2].why), ("api", Some("Matched in its description")));
        assert!(t[2].hl.is_empty(), "no mark in a title the words aren't in");
        assert_eq!(t[0].target, Target::Task("T3".into()));
        assert_eq!((g[1].hits[0].r.as_str(), status(&g[1].hits[0]), g[1].hits[0].place.as_str()), ("G3", "Working", "1 of 3 done"));
        // The terminal matched on its branch: no mark in its title.
        assert_eq!(g[2].hits[0].title, "webapp · T3 Settings");
        assert!(g[2].hits[0].hl.is_empty());
        assert_eq!((status(&g[2].hits[0]), g[2].hits[0].place.as_str()), ("Needs you", "T3"));
        assert_eq!(g[2].hits[0].target, Target::Terminal("s2".into()));
    }

    #[test]
    fn a_ref_typed_in_full_puts_its_kind_and_row_first() {
        let (tasks, goals, sessions) = sample();
        let g = results("t3", &src(&tasks, &goals, &sessions), false, None);
        assert_eq!(g[0].hits[0].r, "T3");
        assert_eq!(g[0].total, 1);
        let g = results("g3", &src(&tasks, &goals, &sessions), false, None);
        assert_eq!((g[0].kind, g[0].hits[0].r.as_str()), (Kind::Goals, "G3"), "the goal's group moves ahead of tasks");
    }

    #[test]
    fn nothing_typed_shows_open_tasks_pinned_goals_and_every_terminal() {
        let (tasks, goals, sessions) = sample();
        let s = Sources { backlog: Some(&backlog()), ..src(&tasks, &goals, &sessions) };
        let g = results("", &s, false, None);
        assert_eq!(titles(&g[0]), vec!["Fix the API shell", "Settings page: manage passkeys"]);
        assert_eq!(g[1].total, 1);
        // The one that needs you first; an idle one says for how long, with no pill.
        assert_eq!(titles(&g[2]), vec!["webapp · T3 Settings", "api · api shell"]);
        assert_eq!((g[2].hits[1].place.as_str(), &g[2].hits[1].status), ("idle 12 min", &None));
    }

    #[test]
    fn no_match_leaves_out_the_kind_and_lists_load_late() {
        let (tasks, goals, _) = sample();
        let s = Sources { tasks: Some(&tasks), goals: &goals, ..Default::default() };
        let g = results("shell", &s, false, None);
        assert_eq!(g.iter().map(|g| g.kind).collect::<Vec<_>>(), vec![Kind::Tasks]);
        assert!(results("zzz", &s, false, None).is_empty());
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
        let bl = backlog();
        let s = Sources { backlog: Some(&bl), ..src(&tasks, &goals, &sessions) };
        let g = results("passkey", &s, false, None);
        assert_eq!(g.iter().map(|g| g.kind).collect::<Vec<_>>(), vec![Kind::Tasks, Kind::Goals, Kind::Backlog, Kind::Terminals]);
        let b = &g[2];
        // Title matches before the one found in its detail; of those, the open one first.
        assert_eq!(titles(b), vec!["Passkey prompt flickers", "Old passkey copy", "Retry the shell"]);
        assert_eq!(b.hits[0].target, Target::Issue("B4".into()));
        assert_eq!((status(&b.hits[0]), b.hits[0].place.as_str()), ("Bug", "Sign-in with passkeys"));
        assert_eq!(status(&b.hits[1]), "Won’t do");
        assert_eq!((status(&b.hits[2]), b.hits[2].place.as_str(), b.hits[2].why), ("Follow-up", "api", Some("Matched in details")));
        let only = Sources { backlog: Some(&bl), ..Default::default() };
        assert_eq!(results("b6", &only, false, None)[0].hits[0].r, "B6");
    }

    #[test]
    fn ideas_join_the_results_after_goals() {
        let (tasks, goals, sessions) = sample();
        let ideas = json!([
            {"id": 2, "ref": "I2", "title": "Dark app icon", "project": "webapp", "state": "new", "detail": "Matches the passkey sheet"},
            {"id": 3, "ref": "I3", "title": "Let the CLI sign in with a passkey", "project": "webapp", "state": "reported"},
        ]);
        let s = Sources { ideas: Some(&ideas), ..src(&tasks, &goals, &sessions) };
        let g = results("passkey", &s, false, None);
        assert_eq!(g.iter().map(|g| g.kind).collect::<Vec<_>>(), vec![Kind::Tasks, Kind::Goals, Kind::Ideas, Kind::Terminals]);
        let i = &g[2];
        assert_eq!(titles(i), vec!["Let the CLI sign in with a passkey", "Dark app icon"]);
        assert_eq!((status(&i.hits[0]), i.hits[0].place.as_str(), &i.hits[0].target), ("Reported", "webapp", &Target::Idea("I3".into())));
        assert_eq!(i.hits[1].why, Some("Matched in details"));
        assert!(results("", &s, false, None).iter().all(|g| g.kind != Kind::Ideas), "none until something's typed");
        assert!(results("passkey", &s, true, None).iter().all(|g| g.kind != Kind::Ideas), "nor within a goal");
    }

    #[test]
    fn attachments_say_their_kind_and_whose_they_are() {
        let atts = json!([
            {"id": 9, "title": "Passkey mockups", "kind": "design", "url": "https://x.test/m", "task": null, "goal": "G3"},
            {"id": 8, "title": "Spec", "summary": "How passkeys sync", "kind": "doc", "url": "/tmp/spec.md", "task": "T2", "goal": null},
        ]);
        let s = Sources { atts: Some(&atts), ..Default::default() };
        let g = results("passkey", &s, false, None);
        assert_eq!(g[0].kind, Kind::Attachments);
        assert_eq!(titles(&g[0]), vec!["Passkey mockups", "Spec"], "a title match first");
        assert_eq!((status(&g[0].hits[0]), g[0].hits[0].place.as_str()), ("Design", "G3"));
        assert_eq!(g[0].hits[1].place, "T2");
        assert_eq!(g[0].hits[1].target, Target::Attachment("/tmp/spec.md".into()));
        assert_eq!(results("passkey", &s, true, None)[0].hits[0].place, "", "the goal's own, within it");
        assert!(results("", &s, false, None).is_empty(), "none until something's typed");
    }

    fn many_tasks(n: i64) -> Value {
        json!({"tasks": (1..=n).map(|n| json!({"id": n, "title": format!("Task {n}"), "status": "queued"})).collect::<Vec<_>>()})
    }

    #[test]
    fn a_group_shows_five_then_show_all_opens_the_rest() {
        let tasks = many_tasks(9);
        let s = Sources { tasks: Some(&tasks), ..Default::default() };
        let g = results("task", &s, false, None);
        assert_eq!((g[0].total, g[0].hits.len()), (9, PER_GROUP));
        let v = View::new(g, None, false);
        assert_eq!(v.lines.last(), Some(&Line::More(0)));
        assert_eq!(v.picks.len(), PER_GROUP + 1, "the five rows and Show all");
        let g = results("task", &s, false, Some(Kind::Tasks));
        assert_eq!(g[0].hits.len(), 9);
        let v = View::new(g, Some(Kind::Tasks), false);
        assert!(!v.lines.contains(&Line::More(0)));
        assert_eq!(v.height(&Line::Head(0)) + v.height(&Line::Hit(0, 0)), HEAD_H + ROW_H);
    }

    #[test]
    fn within_a_goal_the_last_row_searches_everything() {
        let tasks = many_tasks(2);
        let s = Sources { tasks: Some(&tasks), ..Default::default() };
        let v = View::new(results("task", &s, true, None), None, true);
        assert_eq!(v.lines, vec![Line::Head(0), Line::Hit(0, 0), Line::Hit(0, 1), Line::Every]);
        assert_eq!(v.picks, vec![1, 2, 3]);
        assert!(v.hit(2).is_none(), "Every isn't a result");
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
        w.update(cx, |m, _, _| assert_eq!(m.search.found.as_ref().map(|f| f.ask.0.as_str()), Some("sig"))).unwrap();
    }

    #[gpui_kit::test]
    fn an_answer_for_older_text_is_dropped(cx: &mut gpui_kit::TestAppContext) {
        let (w, _) = crate::parity::window(cx);
        w.update(cx, |m, window, cx| open(m, window, cx)).unwrap();
        w.update(cx, |m, _, cx| m.search.input.as_ref().unwrap().set_text("sig", cx)).unwrap();
        cx.executor().advance_clock(PAUSE);
        crate::parity::settle(cx);
        w.update(cx, |m, _, cx| {
            land(m, &("si".into(), None, WINDOW), json!({"tasks": [], "issues": [], "tasks_total": 0, "issues_total": 0}), cx);
            assert_eq!(m.search.found.as_ref().map(|f| f.ask.0.as_str()), Some("sig"));
            assert!(m.data.tasks.as_ref().is_some_and(|t| t["tasks"].as_array().is_some_and(|a| !a.is_empty())), "the answer for sig stays");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn show_all_asks_the_board_for_every_match(cx: &mut gpui_kit::TestAppContext) {
        let (w, _) = crate::parity::window(cx);
        crate::parity::settle(cx);
        w.update(cx, |m, window, cx| open(m, window, cx)).unwrap();
        w.update(cx, |m, _, cx| m.search.input.as_ref().unwrap().set_text("t", cx)).unwrap();
        cx.executor().advance_clock(PAUSE);
        crate::parity::settle(cx);
        let more = w
            .update(cx, |m, _, cx| {
                let v = refresh_view(m, cx);
                v.picks.iter().position(|&l| matches!(v.lines[l], Line::More(g) if v.groups[g].kind == Kind::Tasks))
            })
            .unwrap();
        let Some(k) = more else { return };
        w.update(cx, |m, window, cx| pick(m, k, window, cx)).unwrap();
        crate::parity::settle(cx);
        w.update(cx, |m, _, cx| {
            assert_eq!(m.search.all, Some(Kind::Tasks));
            assert_eq!(m.search.found.as_ref().map(|f| f.ask.2), Some(ALL));
            let g = current(m, cx);
            let tasks = g.iter().find(|g| g.kind == Kind::Tasks).unwrap();
            assert!(tasks.hits.len() > PER_GROUP);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn the_pointer_moves_the_one_highlight(cx: &mut gpui_kit::TestAppContext) {
        let (w, _) = crate::parity::window(cx);
        crate::parity::settle(cx);
        w.update(cx, |m, window, cx| open(m, window, cx)).unwrap();
        crate::parity::settle(cx);
        w.update(cx, |m, _, cx| {
            select(m, 2, false, cx);
            assert_eq!(m.search.sel, 2);
        })
        .unwrap();
        cx.simulate_keystrokes(w.into(), "down");
        w.update(cx, |m, _, _| assert_eq!(m.search.sel, 3, "the keys carry on from where the pointer left it")).unwrap();
    }

    #[gpui_kit::test]
    fn with_midna_down_terminals_are_one_line(cx: &mut gpui_kit::TestAppContext) {
        let (w, _) = crate::parity::window(cx);
        crate::parity::settle(cx);
        w.update(cx, |m, window, cx| {
            let mut st = m.state().clone();
            st["midna"] = json!({"up": false});
            m.data.state = Some(Rc::new(st));
            open(m, window, cx);
        })
        .unwrap();
        crate::parity::settle(cx);
        w.update(cx, |m, _, cx| {
            let mut st = m.state().clone();
            st["midna"] = json!({"up": false});
            m.data.state = Some(Rc::new(st));
            let g = current(m, cx);
            let terms = g.iter().find(|g| g.kind == Kind::Terminals).expect("terminals counted");
            assert!(terms.hits.is_empty());
            assert_eq!(terms.note, Some(MIDNA_DOWN));
            assert!(terms.total > 0);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn a_board_that_stops_answering_says_when(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = crate::parity::window(cx);
        crate::parity::settle(cx);
        rec.fail("search", 503, "down");
        w.update(cx, |m, window, cx| open(m, window, cx)).unwrap();
        w.update(cx, |m, _, cx| m.search.input.as_ref().unwrap().set_text("sig", cx)).unwrap();
        cx.executor().advance_clock(PAUSE);
        crate::parity::settle(cx);
        w.update(cx, |m, _, _| assert!(m.search.failed.is_some())).unwrap();
    }

    fn project(name: &str, open: usize, finished: usize) -> ProjectRow {
        ProjectRow { name: name.into(), open, running: 0, queued: false, needs: false, waits_on: vec![], finished, tasks: 0 }
    }

    #[test]
    fn a_project_shows_by_name_above_the_tasks_in_it() {
        let (tasks, goals, sessions) = sample();
        let mut g = results("webapp", &src(&tasks, &goals, &sessions), false, None);
        add_projects(&mut g, vec![project("webapp", 1, 2), project("api", 0, 0)], "webapp", None);
        assert_eq!(g[0].kind, Kind::Projects);
        assert_eq!(titles(&g[0]), vec!["webapp"]);
        assert_eq!(g[0].hits[0].target, Target::Project("webapp".into()));
        assert_eq!(g[0].hits[0].hl, vec![0..6]);
        assert_eq!(g[0].hits[0].place, "1 goal");
        assert_eq!(g[1].kind, Kind::Tasks);
    }

    #[test]
    fn an_idle_project_says_what_finished_or_nothing() {
        let w = words("p");
        let hits = project_hits(vec![project("p1", 0, 3), project("p2", 0, 0)], &w);
        assert_eq!(hits.iter().map(|h| h.place.as_str()).collect::<Vec<_>>(), vec!["3 done", ""]);
    }

    #[test]
    fn a_ref_typed_in_full_stays_ahead_of_a_project() {
        let (tasks, goals, sessions) = sample();
        let mut g = results("t3", &src(&tasks, &goals, &sessions), false, None);
        add_projects(&mut g, vec![project("t3-tools", 1, 0)], "t3", None);
        assert_eq!(g.iter().map(|g| g.kind).take(2).collect::<Vec<_>>(), vec![Kind::Tasks, Kind::Projects]);
    }

    #[test]
    fn projects_wait_for_something_typed() {
        let mut g = vec![];
        add_projects(&mut g, vec![project("webapp", 1, 0)], "  ", None);
        assert!(g.is_empty());
    }

    #[gpui_kit::test]
    fn typing_a_project_finds_it_and_opening_it_shows_its_page(cx: &mut gpui_kit::TestAppContext) {
        let (w, _) = crate::parity::window(cx);
        crate::parity::settle(cx);
        w.update(cx, |m, window, cx| open(m, window, cx)).unwrap();
        crate::parity::settle(cx);
        let name = w.update(cx, |m, _, _| s(&arr(m.state(), "projects")[0], "name").to_string()).unwrap();
        cx.simulate_input(w.into(), &name);
        crate::parity::settle(cx);
        w.update(cx, |m, window, cx| {
            let groups = current(m, cx);
            assert_eq!(groups[0].kind, Kind::Projects);
            let target = groups[0].hits[0].target.clone();
            assert_eq!(target, Target::Project(name.clone()));
            go(m, target, window, cx);
            assert_eq!(m.page, crate::app::Page::Projects);
            assert_eq!(m.projects.selected.as_deref(), Some(name.as_str()));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn on_a_goal_page_it_searches_that_goal_until_widened(cx: &mut gpui_kit::TestAppContext) {
        let (w, _) = crate::parity::window(cx);
        crate::parity::settle(cx);
        w.update(cx, |m, _, cx| m.go(crate::app::Page::Goal("G1".into()), cx)).unwrap();
        crate::parity::settle(cx);
        w.update(cx, |m, window, cx| open(m, window, cx)).unwrap();
        crate::parity::settle(cx);
        let kinds = |m: &MainWindow, cx: &App| current(m, cx).iter().map(|g| g.kind).collect::<Vec<_>>();
        w.update(cx, |m, _, cx| {
            assert_eq!(m.search.scope.as_ref().map(|s| s.r.as_str()), Some("G1"));
            assert!(!m.search.scope.as_ref().unwrap().name.is_empty(), "the chip names the goal");
            let tasks = arr(m.data.tasks.as_ref().unwrap(), "tasks").to_vec();
            assert!(!tasks.is_empty() && tasks.iter().all(|t| fmt::ref_of(&t["goal"], "G") == "G1"), "only G1's tasks");
            assert!(!kinds(m, cx).contains(&Kind::Goals) && !kinds(m, cx).contains(&Kind::Terminals));
        })
        .unwrap();
        cx.simulate_keystrokes(w.into(), "backspace");
        crate::parity::settle(cx);
        w.update(cx, |m, _, cx| {
            assert!(m.search.scope.is_none(), "⌫ in the empty field searches everything");
            assert!(kinds(m, cx).contains(&Kind::Goals));
            assert!(arr(m.data.tasks.as_ref().unwrap(), "tasks").iter().any(|t| fmt::ref_of(&t["goal"], "G") != "G1"));
        })
        .unwrap();
    }
}
