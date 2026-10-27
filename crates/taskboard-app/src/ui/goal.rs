//! A goal's page: where it stands, its waves as sections, and what it runs with beside them.
//!
//! The header has the goal's ref, project and epic, its name and TLDR, the run signals (Start,
//! Pause or Resume, Open in Claude) and a status bar. Below it each wave is a card of its tasks
//! (`waves`); beside them one card says how the goal runs and lists its notes, attachments,
//! devices and bits (`pool`) and open backlog. Nothing here edits the goal: Claude and `tb` do.
//!
//! An action with a group (`ggate:G1`, `wgate:G1`) shows its result as an inline note there; one
//! without a group shows a toast; a busy action disables its button and shows its busy label.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, b, i, s};
use crate::theme::Theme;
use crate::ui::kit;
use crate::ui::select::sel;
use crate::ui::status::{self, St};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

mod attached;
mod lists;
mod pool;
mod waves;

const START_MENU: &str = "goal-start";
const WAIT_MENU: &str = "goal-wait";
const MISSES_MENU: &str = "goal-misses";
/// How many open issues the Backlog section lists before "N more".
const BACKLOG_SHOWN: usize = 5;

#[derive(Default)]
pub struct State {
    // Reset when the goal changes.
    for_goal: String,
    /// Opened from a backlog link: the Backlog section lists every open issue.
    pub backlog_view: bool,
    /// The Notes section is expanded, and the one kind it lists (`None`: every kind).
    notes_open: bool,
    /// The Devices section is expanded; folded by default.
    devices_open: bool,
    note_filter: Option<&'static str>,
    /// The kind of file the Attached card narrows to.
    att_filter: attached::Filter,
    // Kept across goals: the Attached card's folded waves and tasks (`w2`, `T5`) by goal, a
    // request's result by group, and the requests running.
    att_closed: HashMap<String, HashSet<String>>,
    notes: HashMap<String, (String, bool)>,
    busy: HashSet<String>,
    /// The body's two lists and the notes fetched so far; started over for each goal.
    lists: lists::Lists,
}

impl State {
    pub fn reset_for(&mut self, goal: &str) {
        self.for_goal = goal.to_string();
        self.lists = lists::Lists::default();
        self.backlog_view = std::env::var("TASKBOARD_GOAL_VIEW").is_ok_and(|v| v == "backlog");
        // Dev: `TASKBOARD_GOAL_NOTES=open|Decision|Finding|Reference` expands Notes, filtered to that kind (screenshots).
        let notes = std::env::var("TASKBOARD_GOAL_NOTES").unwrap_or_default();
        self.notes_open = !notes.is_empty();
        self.devices_open = false;
        self.note_filter = ["Decision", "Finding", "Reference"].into_iter().find(|k| *k == notes);
        self.att_filter = attached::Filter::All;
        // Dev: `TASKBOARD_GOAL_ATT_CLOSED=w1,T5` folds those waves and tasks in Attached (screenshots).
        if let Ok(keys) = std::env::var("TASKBOARD_GOAL_ATT_CLOSED") {
            self.att_closed.insert(goal.to_string(), keys.split(',').filter(|k| !k.is_empty()).map(str::to_string).collect());
        }
    }
}

/// Open a goal's page, with its whole backlog listed when `backlog` (`#/goals/G3?view=backlog`).
pub fn open(m: &mut MainWindow, r: &str, backlog: bool, cx: &mut Context<MainWindow>) {
    m.go(Page::Goal(r.to_string()), cx);
    m.goal_page.reset_for(r);
    m.goal_page.backlog_view = backlog;
    cx.notify();
}

// ------------------------------------------------------------------ goal state

pub(crate) fn pr_open(pr: &Value) -> bool {
    fmt::opt_s(pr, "state").unwrap_or("OPEN").eq_ignore_ascii_case("open")
}

fn has_pr(t: &Value) -> bool {
    t["pr"].is_object() && !t["pr"]["num"].is_null()
}

pub(crate) fn awaiting_merge(t: &Value) -> bool {
    s(t, "status") == "done" && !b(t, "failed") && has_pr(t) && pr_open(&t["pr"])
}

fn pr_stopped(t: &Value) -> bool {
    awaiting_merge(t) && t["pr"]["stage"].is_object() && is_truthy(&t["pr"]["stage"]["stopped"])
}

fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(x) => *x,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.),
        Value::String(x) => !x.is_empty(),
        _ => true,
    }
}

/// The goal's own tasks, then the shared ones from other goals that also count toward it.
fn counted(g: &Value) -> Vec<Value> {
    arr(g, "tasks").iter().chain(arr(g, "shared")).cloned().collect()
}

/// The goal's state label and its tone (the Goals list shows it; tests pin it).
#[cfg_attr(not(test), allow(dead_code))]
pub fn goal_state(g: &Value) -> (String, &'static str) {
    let tasks = counted(g);
    if !tasks.is_empty() && tasks.iter().all(|t| s(t, "status") == "done") {
        let open: Vec<&Value> = tasks.iter().filter(|t| awaiting_merge(t)).collect();
        if open.iter().any(|t| pr_stopped(t)) {
            return ("Waiting on you".into(), "warn");
        }
        if !open.is_empty() {
            return (if open.len() > 1 { format!("{} PRs awaiting merge", open.len()) } else { "Awaiting merge".into() }, "accent");
        }
        if i(g, "misses") > 0 {
            return ("Outcome not met".into(), "warn");
        }
        // A backend flag not made yet holds back a finished goal.
        let bits = i(g, "bits_waiting");
        if bits > 0 {
            return (format!("Waiting on {}", fmt::plural(bits, "bit", "bits")), "warn");
        }
        return ("Done".into(), "up");
    }
    if b(g, "deprioritized") {
        return ("Deprioritized".into(), "queued");
    }
    if b(g, "paused") {
        return ("Paused".into(), "warn");
    }
    if tasks.iter().any(|t| b(t, "lost")) {
        return ("Needs a restart".into(), "down");
    }
    if tasks.iter().any(|t| s(t, "status") == "needs") {
        return ("Waiting on you".into(), "warn");
    }
    // Stopped at a wave for the owner (a review stop or a failed task), even with a task still
    // working (one taken by hand).
    if fmt::opt_s(g, "stopped").is_some() {
        return ("Waiting on you".into(), "warn");
    }
    if tasks.iter().any(|t| s(t, "status") == "working") {
        return ("In progress".into(), "accent");
    }
    let queued: Vec<&Value> = tasks.iter().filter(|t| s(t, "status") == "queued").collect();
    if queued.iter().any(|t| b(t, "blocked")) {
        return ("Blocked".into(), "warn");
    }
    if !queued.is_empty() {
        return ("Queued".into(), "accent");
    }
    (if tasks.is_empty() { "No tasks yet" } else { "Not started" }.into(), "queued")
}

/// The status bar's parts in the design's order (done, working, needs you, in review, failed,
/// queued, planned), and the line beside it: ("2 of 7", " done · 2 working · 1 needs you").
/// A `/goals` row, without its tasks, counts from its summary.
pub fn progress(g: &Value) -> (Vec<(St, i64)>, String, String) {
    let mut n: HashMap<St, i64> = HashMap::new();
    let total = if g["tasks"].is_array() {
        let tasks = counted(g);
        for t in &tasks {
            *n.entry(status::row(t).st).or_default() += 1;
        }
        tasks.len() as i64
    } else {
        let counts = [
            (St::Done, i(g, "done")),
            (St::Working, (i(g, "active") - i(g, "needs")).max(0)),
            (St::Needs, i(g, "needs")),
            (St::Review, i(g, "review")),
            (St::Failed, i(g, "failed")),
            (St::Queued, i(g, "queued")),
            (St::Planned, i(g, "planned")),
        ];
        n.extend(counts.into_iter().filter(|(_, x)| *x > 0));
        i(g, "total")
    };
    let order = [St::Done, St::Pending, St::Working, St::Needs, St::Review, St::Failed, St::Queued, St::Planned];
    let parts: Vec<(St, i64)> = order.iter().filter_map(|st| n.get(st).map(|x| (*st, *x))).collect();
    let lead = format!("{} of {total}", n.get(&St::Done).copied().unwrap_or(0));
    let mut tail = " done".to_string();
    for st in &order[1..] {
        if let Some(x) = n.get(st) {
            tail.push_str(&format!(" · {x} {}", st.label().to_lowercase()));
        }
    }
    (parts, lead, tail)
}

// ------------------------------------------------------------------ run signals

fn after_hours(hours: &Value) -> bool {
    b(hours, "on") && !b(hours, "open")
}

/// When work hours open next, ("tomorrow", "6am"); ("later", "") when unknown.
pub fn opens_at(hours: &Value) -> (String, String) {
    let Some(t) = fmt::parse(s(hours, "next_open")) else { return ("later".into(), String::new()) };
    let l = fmt::local(t);
    let today = fmt::local(fmt::now()).date_naive();
    let day = if l.date_naive() == today {
        "today".to_string()
    } else if Some(l.date_naive()) == today.succ_opt() {
        "tomorrow".to_string()
    } else {
        l.format("%A").to_string()
    };
    (day, fmt::clock12(&l.format("%H:%M").to_string()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunButton {
    /// goal-run, goal-pause, wait-menu or goal-plan-edit.
    pub act: &'static str,
    pub label: String,
    pub title: String,
    /// What the button says while its request runs.
    pub busy_label: &'static str,
}

/// Whether Start is the split "Start tomorrow ▾" button, and the header's buttons in order.
pub fn run_buttons_view(g: &Value, hours: &Value) -> (bool, Vec<RunButton>) {
    let tasks = arr(g, "tasks");
    let planned = tasks.iter().filter(|x| s(x, "status") == "planned").count() as i64;
    let going = tasks.iter().filter(|x| matches!(s(x, "status"), "queued" | "working" | "needs")).count() as i64;
    let unstarted = tasks.iter().any(|x| s(x, "status") == "needs" && s(x, "needs_reason") == "start_failed");
    let mut out = Vec::new();
    let mut split = false;
    if planned > 0 && after_hours(hours) {
        split = true;
        let (day, at) = opens_at(hours);
        let later = format!("Start {day}");
        let more = if going > 0 { fmt::plural(planned, "more task", "more tasks") } else { String::new() };
        out.push(RunButton {
            act: "goal-run",
            label: if more.is_empty() { later } else { format!("{later}: {more}") },
            title: format!("Queue the planned tasks now; they start when work hours open{}", if at.is_empty() { String::new() } else { format!(" ({day} at {at})") }),
            busy_label: "Queuing…",
        });
    } else if planned > 0 || (going > 0 && (b(g, "paused") || unstarted)) {
        let label = if planned == 0 {
            "Resume".to_string()
        } else if going > 0 {
            format!("Start {}", fmt::plural(planned, "more task", "more tasks"))
        } else {
            "Start".to_string()
        };
        let title = if planned > 0 {
            format!("Queue the {}; the goal’s rules decide what runs when", fmt::plural(planned, "planned task", "planned tasks"))
        } else if unstarted {
            "Queue the tasks that couldn’t start again".into()
        } else {
            "Let new tasks start again".into()
        };
        out.push(RunButton { act: "goal-run", label, title, busy_label: "Starting…" });
    }
    // Outside work hours with tasks queued, and not started now: Pause gives way to a button that says
    // it's waiting for the hours, its menu starting it now or pausing it; Start tomorrow's menu holds
    // Pause instead when that shows.
    let waiting = waits_for_hours(g, hours);
    if going > 0 && !b(g, "paused") && !b(g, "deprioritized") && !unstarted && !(split && waiting) {
        out.push(if waiting {
            let (day, at) = opens_at(hours);
            RunButton {
                act: "wait-menu",
                label: if at.is_empty() { "Waiting for work hours".into() } else { format!("Waiting until {day} {at}") },
                title: format!("The queued tasks start when work hours open{}", if at.is_empty() { String::new() } else { format!(" ({day} at {at})") }),
                busy_label: "Sending…",
            }
        } else {
            RunButton { act: "goal-pause", label: "Pause".into(), title: "Nothing new starts; running tasks carry on".into(), busy_label: "Sending…" }
        });
    }
    out.push(RunButton {
        act: "goal-plan-edit",
        label: "Open in Claude".into(),
        title: "Open a Claude terminal in Midna on this goal’s plan, to change it by talking it through".into(),
        busy_label: "Opening…",
    });
    (split, out)
}

/// The split button's menu: (act, title, sub); Pause too while the goal waits for the hours.
pub fn start_menu_view(g: &Value, hours: &Value) -> Vec<(&'static str, String, String)> {
    let (day, at) = opens_at(hours);
    let mut out = vec![
        ("goal-run", format!("Start {day}"), format!("Queued now, starts when work hours open{}", if at.is_empty() { String::new() } else { format!(" at {at}") })),
        ("goal-run-now", "Start now".into(), "Runs outside work hours until they open".into()),
    ];
    if waits_for_hours(g, hours) {
        out.push(("goal-pause", "Pause".into(), "Nothing new starts; running tasks carry on".into()));
    }
    out
}

/// Whether the goal's queued tasks wait for work hours: not paused, and not started now.
fn waits_for_hours(g: &Value, hours: &Value) -> bool {
    let tasks = arr(g, "tasks");
    !b(g, "paused")
        && !b(g, "deprioritized")
        && !tasks.iter().any(|x| s(x, "status") == "needs" && s(x, "needs_reason") == "start_failed")
        && tasks.iter().any(|x| s(x, "status") == "queued")
        && after_hours(hours)
        && !ran_now(g, hours)
}

/// Whether the goal was started now and runs outside the hours until they next open.
fn ran_now(g: &Value, hours: &Value) -> bool {
    let (until, next) = (s(g, "hours_until"), s(hours, "next_open"));
    !until.is_empty() && !next.is_empty() && until >= next
}

/// The waiting button's menu: (act, title, sub).
pub fn wait_menu_view() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("goal-run-now", "Start now", "Runs outside work hours until they open"),
        ("goal-pause", "Pause", "Nothing new starts; running tasks carry on"),
    ]
}

/// The toast after `goal-run`.
pub fn run_toast(v: &Value, hours: &Value) -> String {
    let (n, r) = (i(v, "queued_now"), i(v, "resumed_now"));
    let said = if n == 0 && r == 0 {
        "Going again".into()
    } else if after_hours(hours) {
        let (day, at) = opens_at(hours);
        let when = [day, at].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" at ");
        format!("{}. They start {when}.", run_counts(n, r))
    } else {
        format!("Started: {}", run_counts(n, r))
    };
    with_waits(said, v)
}

/// The toast after `goal-run-now`.
pub fn run_now_toast(v: &Value) -> String {
    let said = format!("Started now: {}. The goal runs until work hours open.", run_counts(i(v, "queued_now"), i(v, "resumed_now")));
    with_waits(said, v)
}

/// "2 tasks queued", "1 parked task resumed", or both.
fn run_counts(queued: i64, resumed: i64) -> String {
    let mut parts = vec![];
    if queued > 0 || resumed == 0 {
        parts.push(format!("{} queued", fmt::plural(queued, "task", "tasks")));
    }
    if resumed > 0 {
        parts.push(format!("{} resumed", fmt::plural(resumed, "parked task", "parked tasks")));
    }
    parts.join(", ")
}

/// A run's toast with why the tasks that still can't start wait ("T192 waits for you to do H197").
fn with_waits(said: String, v: &Value) -> String {
    let waits: Vec<&str> = arr(v, "waiting").iter().filter_map(|w| w.as_str()).collect();
    if waits.is_empty() {
        return said;
    }
    let mut out = vec![said.trim_end_matches('.').to_string()];
    out.extend(waits.iter().take(2).map(|w| w.to_string()));
    if waits.len() > 2 {
        out.push(format!("{} more wait", waits.len() - 2));
    }
    format!("{}.", out.join(". "))
}

/// A paused or deprioritized goal's banner: (label, text, (act, button)).
pub fn gate_view(g: &Value) -> Option<(&'static str, &'static str, (&'static str, &'static str))> {
    if b(g, "deprioritized") {
        Some(("Deprioritized", "It’s off the board’s goal list and nothing new starts. Tasks already running carry on.", ("goal-deprio", "Bring it back")))
    } else if b(g, "paused") {
        Some(("Paused", "Nothing new starts in this goal. Tasks already running carry on.", ("goal-pause", "Resume the goal")))
    } else {
        None
    }
}

// ------------------------------------------------------------------ aside

/// "How it runs": (key, value) rows; the review stop only while one is ahead.
pub fn how_runs(g: &Value) -> Vec<(&'static str, String)> {
    let max = match &g["max_terminals"] {
        Value::Number(x) => x.as_i64().unwrap_or(0),
        Value::String(x) => x.trim().parse().unwrap_or(0),
        _ => 0,
    };
    let max = if max <= 0 { 2 } else { max };
    let in_use = arr(g, "tasks").iter().filter(|t| matches!(s(t, "status"), "working" | "needs")).count();
    let on = |k: &str| !matches!(g.get(k), Some(Value::Bool(false))) && !matches!(g.get(k), Some(Value::Number(x)) if x.as_f64() == Some(0.));
    let waved = !arr(g, "waves").is_empty();
    let order = match (on("run_in_order"), waved) {
        (true, true) => "In order, by wave",
        (true, false) => "In order",
        (false, _) => "Side by side",
    };
    let mut out = vec![
        ("Terminals", format!("{max} at most · {in_use} in use")),
        ("Order", order.to_string()),
        ("When done", if on("auto_close") { "Terminals close" } else { "Terminals stay open" }.to_string()),
    ];
    let stops: Vec<String> = arr(g, "waves")
        .iter()
        .filter(|w| b(w, "stop_after") && s(w, "state") != "done")
        .map(|w| format!("After wave {}", i(w, "wave")))
        .collect();
    if !stops.is_empty() {
        out.push(("Review stop", stops.join(", ")));
    }
    out
}

/// One note: "Decision · pinned" / "Finding · T12" / "Reference · you", its tone, and the text.
#[derive(Debug, PartialEq)]
pub struct NoteItem {
    /// Decision, Finding or Reference: what the Notes filter matches.
    pub group: &'static str,
    pub kind: String,
    /// goal, results or muted.
    pub tone: &'static str,
    pub text: String,
}

/// The Notes filters: each kind the notes hold, in a fixed order.
pub fn note_groups(items: &[NoteItem]) -> Vec<&'static str> {
    ["Decision", "Finding", "Reference"].into_iter().filter(|k| items.iter().any(|x| x.group == *k)).collect()
}

/// The order the board pages a goal's notes in: pinned first, then newest.
pub fn note_order(a: &Value, c: &Value) -> std::cmp::Ordering {
    b(c, "pinned").cmp(&b(a, "pinned")).then_with(|| s(c, "at").cmp(s(a, "at"))).then_with(|| c["id"].as_i64().cmp(&a["id"].as_i64()))
}

/// The goal's notes, pinned first, then newest first.
pub fn note_items(notes: &[Value]) -> Vec<NoteItem> {
    let mut list: Vec<&Value> = notes.iter().collect();
    list.sort_by(|a, c| note_order(a, c));
    list.into_iter()
        .map(|n| {
            let (label, tone) = match s(n, "kind") {
                "decision" => ("Decision", "goal"),
                "reference" => ("Reference", "muted"),
                _ => ("Finding", "results"),
            };
            let parts = [Some(label.to_string()), b(n, "pinned").then(|| "pinned".to_string()), fmt::opt_s(n, "source").map(str::to_string)];
            let kind = parts.into_iter().flatten().collect::<Vec<_>>().join(" · ");
            NoteItem { group: label, kind, tone, text: s(n, "text").split_whitespace().collect::<Vec<_>>().join(" ") }
        })
        .collect()
}

fn att_web(a: &Value) -> bool {
    let u = s(a, "url").to_lowercase();
    u.starts_with("http://") || u.starts_with("https://")
}

/// A web link, or a `~` / absolute file path the board can open.
fn att_has_href(a: &Value) -> bool {
    att_web(a) || s(a, "url").starts_with('~') || s(a, "url").starts_with('/')
}

fn open_attachment(url: &str, cx: &mut App) {
    if url.starts_with("http://") || url.starts_with("https://") {
        cx.open_url(url);
    } else {
        let path = taskboardd::util::expand_home(url);
        let _ = std::process::Command::new("/usr/bin/open").arg(path).spawn();
    }
}

fn num_text(v: &Value) -> String {
    match v {
        Value::String(x) => x.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub r: String,
    pub title: String,
    /// Reported as a miss on the goal's done-when: the goal stays open until it's settled.
    pub misses_goal: bool,
}

/// The open backlog, misses on the done-when first, otherwise in the goal's order.
pub fn open_issues(g: &Value) -> Vec<Issue> {
    let mut v: Vec<Issue> = arr(g, "backlog")
        .iter()
        .filter(|x| fmt::opt_s(x, "state").unwrap_or("open") == "open")
        .map(|x| Issue { r: fmt::ref_of(x, "B"), title: s(x, "title").to_string(), misses_goal: b(x, "misses_goal") })
        .collect();
    v.sort_by_key(|x| !x.misses_goal);
    v
}

/// The misses holding the goal open: in the backlog, or made into a task that isn't done yet.
pub fn open_misses(g: &Value) -> Vec<Issue> {
    let done: HashSet<String> = arr(g, "tasks")
        .iter()
        .filter(|t| s(t, "status") == "done" && !b(t, "failed"))
        .map(|t| fmt::ref_of(t, "T"))
        .collect();
    arr(g, "backlog")
        .iter()
        .filter(|x| b(x, "misses_goal"))
        .filter(|x| match fmt::opt_s(x, "state").unwrap_or("open") {
            "open" => true,
            "task" => fmt::opt_s(x, "task_ref").is_none_or(|r| !done.contains(r)),
            _ => false,
        })
        .map(|x| Issue { r: fmt::ref_of(x, "B"), title: s(x, "title").to_string(), misses_goal: true })
        .collect()
}

/// Why a goal with every task done is still open, when it's an open miss on its done-when.
pub fn outcome_not_met(g: &Value) -> Option<String> {
    let misses = i(g, "misses");
    (misses > 0).then(|| format!("Outcome not met · {}", fmt::plural(misses, "miss", "misses")))
}

// ------------------------------------------------------------------ note text (links and code)

#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(String),
    Link(String),
    Code(String),
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

const PATH_EXTS: [&str; 22] =
    ["dart", "kt", "kts", "java", "swift", "py", "js", "mjs", "ts", "tsx", "jsx", "json", "yml", "yaml", "md", "sh", "gradle", "xml", "html", "css", "toml", "lock"];

/// `INLINE` at byte `p` of `s` (chars `cs`): (end, kind 0 url / 1 tick / 2 path), first alternative wins.
fn inline_at(cs: &[(usize, char)], k: usize) -> Option<(usize, u8)> {
    let ch = |j: usize| cs.get(j).map(|x| x.1);
    let starts = |pat: &str| pat.chars().enumerate().all(|(o, pc)| ch(k + o) == Some(pc));
    // 1. https?:\/\/[^\s<]+[^\s<.,;:)\]'"]
    for scheme in ["https://", "http://"] {
        if starts(scheme) {
            let from = k + scheme.chars().count();
            let mut end = from;
            while let Some(c) = ch(end) {
                if c.is_whitespace() || c == '<' {
                    break;
                }
                end += 1;
            }
            let mut last = end;
            while last >= from + 2 {
                let c = ch(last - 1).unwrap();
                if !".,;:)]'\"".contains(c) {
                    return Some((last, 0));
                }
                last -= 1;
            }
        }
    }
    // 2. `([^`\n]+)`
    if ch(k) == Some('`') {
        let mut j = k + 1;
        while let Some(c) = ch(j) {
            if c == '`' {
                if j > k + 1 {
                    return Some((j + 1, 1));
                }
                break;
            }
            if c == '\n' {
                break;
            }
            j += 1;
        }
    }
    // 3a. (?:[\w.-]+|\.\.\.)\/(?:[\w.-]+\/|\.\.\.\/)*(?:[\w-]+(?:\.[\w-]+)*)?
    let pathc = |c: char| is_word(c) || c == '.' || c == '-';
    let mut j = k;
    while ch(j).is_some_and(pathc) {
        j += 1;
    }
    if j > k && ch(j) == Some('/') {
        let mut end = j + 1;
        loop {
            let mut q = end;
            while ch(q).is_some_and(pathc) {
                q += 1;
            }
            if q > end && ch(q) == Some('/') {
                end = q + 1;
            } else {
                break;
            }
        }
        // [\w-]+(?:\.[\w-]+)*
        let seg = |c: char| is_word(c) || c == '-';
        let mut q = end;
        while ch(q).is_some_and(seg) {
            q += 1;
        }
        if q > end {
            end = q;
            loop {
                if ch(end) != Some('.') {
                    break;
                }
                let mut r = end + 1;
                while ch(r).is_some_and(seg) {
                    r += 1;
                }
                if r > end + 1 {
                    end = r;
                } else {
                    break;
                }
            }
        }
        return Some((end, 2));
    }
    // 3b. \b[\w-]+\.(?:ext)\b
    let boundary_before = ch(k).is_some_and(is_word) && (k == 0 || !ch(k - 1).is_some_and(is_word));
    if boundary_before {
        let seg = |c: char| is_word(c) || c == '-';
        let mut q = k;
        while ch(q).is_some_and(seg) {
            q += 1;
        }
        // Backtrack: the run may end at any '.' followed by an extension and a word boundary.
        let mut stop = q;
        while stop > k {
            if ch(stop) == Some('.') {
                let mut e = stop + 1;
                while ch(e).is_some_and(is_word) {
                    e += 1;
                }
                let word: String = (stop + 1..e).filter_map(ch).collect();
                if PATH_EXTS.contains(&word.as_str()) {
                    return Some((e, 2));
                }
            }
            stop -= 1;
        }
    }
    None
}

/// `pathLike`.
fn path_like(p: &str) -> bool {
    let ends_ext = p.rfind('.').is_some_and(|d| {
        let tail = &p[d + 1..];
        !tail.is_empty() && tail.chars().all(is_word)
    });
    p.ends_with('/') || ends_ext || p.contains('_') || p.contains('-') || p.contains("...")
}

/// `inlineText(s)` as segments: links, `code` / paths, and text.
pub fn inline_segments(s: &str) -> Vec<Inline> {
    let cs: Vec<(usize, char)> = s.char_indices().collect();
    let byte = |k: usize| cs.get(k).map(|x| x.0).unwrap_or(s.len());
    let mut out = Vec::new();
    let mut text_from = 0usize;
    let mut k = 0usize;
    while k < cs.len() {
        let Some((end, kind)) = inline_at(&cs, k) else {
            k += 1;
            continue;
        };
        let whole = &s[byte(k)..byte(end)];
        if kind == 2 && !path_like(whole) {
            k = end;
            continue;
        }
        if byte(k) > text_from {
            out.push(Inline::Text(s[text_from..byte(k)].to_string()));
        }
        out.push(match kind {
            0 => Inline::Link(whole.to_string()),
            1 => Inline::Code(whole[1..whole.len() - 1].to_string()),
            _ => Inline::Code(whole.to_string()),
        });
        text_from = byte(end);
        k = end;
    }
    if text_from < s.len() {
        out.push(Inline::Text(s[text_from..].to_string()));
    }
    out
}

// ------------------------------------------------------------------ requests

fn note(m: &mut MainWindow, grp: Option<&str>, text: String, err: bool, toast: bool, cx: &mut Context<MainWindow>) {
    if text.is_empty() {
        return;
    }
    match grp {
        Some(g) => {
            m.goal_page.notes.insert(g.to_string(), (text, err));
        }
        None if toast || err => m.toast(text, err, cx),
        None => {}
    }
}

/// Post once at a time per busy key; the answer goes to `grp`'s inline note, or a toast.
fn run(
    m: &mut MainWindow,
    busy: String,
    grp: Option<String>,
    toast: bool,
    path: String,
    body: Value,
    cx: &mut Context<MainWindow>,
    ok: impl FnOnce(&mut MainWindow, &Value, &mut Context<MainWindow>) -> String + 'static,
) {
    if !m.goal_page.busy.insert(busy.clone()) {
        return;
    }
    if let Some(g) = &grp {
        m.goal_page.notes.remove(g);
    }
    let (b1, b2, g1, g2) = (busy.clone(), busy, grp.clone(), grp);
    m.post_or(
        path,
        body,
        cx,
        move |m, v, cx| {
            m.goal_page.busy.remove(&b1);
            let msg = ok(m, &v, cx);
            note(m, g1.as_deref(), msg, false, toast, cx);
        },
        move |m, e, cx| {
            m.goal_page.busy.remove(&b2);
            note(m, g2.as_deref(), e.message, true, toast, cx);
        },
    );
    cx.notify();
}

fn hours(m: &MainWindow) -> Value {
    m.state()["work_hours"].clone()
}

/// Start (`now`: start outside work hours too).
pub fn run_goal(m: &mut MainWindow, gr: &str, now: bool, cx: &mut Context<MainWindow>) {
    m.menu = None;
    if now {
        run(m, format!("goal-run-now::{gr}"), None, true, format!("goals/{gr}/run"), json!({"now": true}), cx, |_, v, _| run_now_toast(v));
    } else {
        run(m, format!("goal-run::{gr}"), None, true, format!("goals/{gr}/run"), json!({}), cx, |m, v, _| run_toast(v, &hours(m)));
    }
}

/// Open in Claude: a Claude terminal in Midna on the goal's plan.
pub fn plan_goal(m: &mut MainWindow, gr: &str, cx: &mut Context<MainWindow>) {
    run(m, format!("goal-plan-edit::{gr}"), None, true, format!("goals/{gr}/plan"), json!({"mode": "edit"}), cx, |_, _, _| {
        "Opening a Claude terminal in Midna on this goal’s plan".into()
    });
}

/// Pause (`on`) from the header, or Resume (`off`) from the gate banner.
pub fn set_paused(m: &mut MainWindow, gr: &str, on: bool, cx: &mut Context<MainWindow>) {
    let grp = (!on).then(|| format!("ggate:{gr}"));
    let arg = if on { "on" } else { "off" };
    run(m, format!("goal-pause:{arg}:{gr}"), grp, true, format!("goals/{gr}"), json!({"paused": on}), cx, move |_, _, _| {
        if on { "Paused. Running tasks carry on.".into() } else { "Going again".into() }
    });
}

/// Jira off (`off`) or back on for the goal's tasks, from the How it runs box.
pub fn set_jira_off(m: &mut MainWindow, gr: &str, off: bool, cx: &mut Context<MainWindow>) {
    run(m, format!("goal-jira::{gr}"), Some(format!("gjira:{gr}")), false, format!("goals/{gr}"), json!({"jira_off": off}), cx, move |_, _, _| {
        if off { "Jira off: its tasks ask for no tickets.".into() } else { "Jira on for new tasks. Tasks it already has still make no ticket.".into() }
    });
}

/// "Bring it back": a deprioritized goal's banner.
pub fn bring_back(m: &mut MainWindow, gr: &str, cx: &mut Context<MainWindow>) {
    run(m, format!("goal-deprio:off:{gr}"), Some(format!("ggate:{gr}")), true, format!("goals/{gr}"), json!({"deprioritized": false}), cx, |_, _, _| {
        "Back on the board".into()
    });
}

fn busy(m: &MainWindow, key: &str) -> bool {
    m.goal_page.busy.contains(key)
}

fn inline_note(m: &MainWindow, t: &Theme, grp: &str) -> Option<Div> {
    m.goal_page.notes.get(grp).map(|(text, err)| {
        div()
            .text_size(px(12.5))
            .text_color(if *err { t.down } else { t.up_fg })
            .when(*err, |d| d.font_weight(FontWeight::MEDIUM))
            .child(text.clone())
    })
}

// ------------------------------------------------------------------ render

/// An aside section: an uppercase title with its count, then the body; a divider under it.
pub(super) fn side(t: &Theme, title: &str, count: Option<String>, body: impl IntoElement) -> Div {
    side_box(t).child(side_head(t, title, count).pb(px(8.))).child(body)
}

fn side_box(t: &Theme) -> Div {
    div().flex().flex_col().px(px(18.)).py(px(16.)).border_b_1().border_color(t.divider)
}

/// One piece of an aside section drawn as its own list item: the first holds the section's top,
/// the last its bottom and divider; `gap` stands for the space its column had above it.
pub(super) fn side_piece(t: &Theme, first: bool, last: bool, gap: f32, body: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .px(px(18.))
        .pt(px(if first { 16. } else { gap }))
        .when(last, |d| d.pb(px(16.)).border_b_1().border_color(t.divider))
        .child(body)
}

pub(super) fn side_head(t: &Theme, title: &str, count: Option<String>) -> Div {
    div()
        .flex()
        .items_baseline()
        .gap(px(8.))
        .text_size(px(11.))
        .font_weight(FontWeight::BOLD)
        .text_color(t.muted)
        .child(title.to_uppercase())
        .when_some(count, |d, n| d.child(div().font_weight(FontWeight::NORMAL).child(n)))
}

/// Goals › haymay › G31: the project crumb opens the Goals page on that project, Goals on all.
fn crumbs(t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Div {
    let hover = t.text;
    let link = |id: &'static str, text: String| div().id(id).cursor_pointer().hover(move |s| s.text_color(hover)).child(text);
    let sep = || div().text_color(t.faint).child("›");
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(12.))
        .text_color(t.muted)
        .child(link("goal-crumb", "Goals".into()).on_click(cx.listener(|m, _, _, cx| open_project(m, "all", cx))))
        .when_some(project_of(g), |d, p| {
            let open = p.clone();
            d.child(sep()).child(link("goal-crumb-project", p).on_click(cx.listener(move |m, _, _, cx| open_project(m, &open, cx))))
        })
        .child(sep())
        .child(div().font_family(t.mono_font.clone()).child(fmt::ref_of(g, "G")))
}

fn project_of(g: &Value) -> Option<String> {
    fmt::opt_s(g, "project").filter(|p| !p.is_empty()).map(str::to_string)
}

/// The Goals page on one project's goals ("all" for every project's).
fn open_project(m: &mut MainWindow, project: &str, cx: &mut Context<MainWindow>) {
    m.goals.project = project.to_string();
    m.go(Page::Goals, cx);
}

fn buttons(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Div {
    let gr = fmt::ref_of(g, "G");
    let (split, list) = run_buttons_view(g, &hours(m));
    let mut row = div().flex().flex_none().items_center().gap(px(8.));
    for bt in list {
        let key = match bt.act {
            "goal-pause" => format!("goal-pause:on:{gr}"),
            a => format!("{a}::{gr}"),
        };
        let is_busy = busy(m, &key);
        let label = if is_busy { bt.busy_label.to_string() } else { bt.label.clone() };
        let gr2 = gr.clone();
        let el = match bt.act {
            "goal-run" => {
                let main = kit::btn_primary(t, "goal-run", label).tooltip(kit::tip(bt.title.clone()));
                let main = if is_busy { kit::disabled(main) } else { main.on_click(cx.listener(move |m, _, _, cx| run_goal(m, &gr2, false, cx))) };
                if split {
                    let on = t.on_accent;
                    div()
                        .flex()
                        .child(main.rounded_r(px(0.)))
                        .child(
                            kit::btn_primary(t, "goal-run-caret", "▾")
                                .rounded_l(px(0.))
                                .border_l_1()
                                .border_color(t.accent_fg)
                                .px(px(8.))
                                .text_color(on)
                                .tooltip(kit::tip("Start now instead"))
                                .on_click(cx.listener(|m, e: &ClickEvent, _, cx| {
                                    let p = e.position();
                                    m.toggle_menu(START_MENU, point(p.x - px(240.), p.y + px(14.)), cx);
                                })),
                        )
                        .into_any_element()
                } else {
                    main.into_any_element()
                }
            }
            "goal-pause" => {
                let b = kit::btn(t, "goal-pause", label).tooltip(kit::tip(bt.title.clone()));
                if is_busy { kit::disabled(b) } else { b.on_click(cx.listener(move |m, _, _, cx| set_paused(m, &gr2, true, cx))) }.into_any_element()
            }
            "wait-menu" => {
                // Busy while its Start now or Pause runs.
                let is_busy = busy(m, &format!("goal-run-now::{gr}")) || busy(m, &format!("goal-pause:on:{gr}"));
                let b = kit::btn(t, "goal-wait", if is_busy { bt.busy_label.to_string() } else { format!("{label} ▾") }).tooltip(kit::tip(bt.title.clone()));
                if is_busy {
                    kit::disabled(b)
                } else {
                    b.on_click(cx.listener(|m, e: &ClickEvent, _, cx| {
                        let p = e.position();
                        m.toggle_menu(WAIT_MENU, point(p.x - px(240.), p.y + px(14.)), cx);
                    }))
                }
                .into_any_element()
            }
            _ => {
                let b = kit::btn(t, "goal-plan", label).tooltip(kit::tip(bt.title.clone()));
                if is_busy { kit::disabled(b) } else { b.on_click(cx.listener(move |m, _, _, cx| plan_goal(m, &gr2, cx))) }.into_any_element()
            }
        };
        row = row.child(el);
    }
    row
}

fn start_menu(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let at = m.menu_open(START_MENU)?;
    let gr = fmt::ref_of(g, "G");
    let mut menu = kit::menu_box(t, 290.).id("goal-start-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for (act, title, sub) in start_menu_view(g, &hours(m)) {
        let hover = t.panel_2;
        let gr = gr.clone();
        menu = menu.child(
            div()
                .id(act)
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover(move |d| d.bg(hover))
                .child(div().text_size(px(13.)).font_weight(FontWeight::BOLD).child(title))
                .child(div().text_size(px(12.)).text_color(t.muted).child(sub))
                .on_click(cx.listener(move |m, _, _, cx| {
                    if act == "goal-pause" {
                        m.menu = None;
                        set_paused(m, &gr, true, cx)
                    } else {
                        run_goal(m, &gr, act == "goal-run-now", cx)
                    }
                })),
        );
    }
    Some(kit::popover(at, menu))
}

fn wait_menu(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let at = m.menu_open(WAIT_MENU)?;
    let gr = fmt::ref_of(g, "G");
    let mut menu = kit::menu_box(t, 290.).id("goal-wait-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for (act, title, sub) in wait_menu_view() {
        let hover = t.panel_2;
        let gr = gr.clone();
        menu = menu.child(
            div()
                .id(act)
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover(move |d| d.bg(hover))
                .child(div().text_size(px(13.)).font_weight(FontWeight::BOLD).child(title))
                .child(div().text_size(px(12.)).text_color(t.muted).child(sub))
                .on_click(cx.listener(move |m, _, _, cx| {
                    m.menu = None;
                    if act == "goal-pause" { set_paused(m, &gr, true, cx) } else { run_goal(m, &gr, true, cx) }
                })),
        );
    }
    Some(kit::popover(at, menu))
}

/// The misses behind "Outcome not met": each opens its issue.
fn misses_menu(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let at = m.menu_open(MISSES_MENU)?;
    let mut menu = kit::menu_box(t, 380.).id("goal-misses-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for Issue { r, title, .. } in open_misses(g) {
        let hover = t.panel_2;
        let target = r.clone();
        menu = menu.child(
            div()
                .id(SharedString::from(format!("goal-miss-{r}")))
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(6.))
                .text_size(px(13.))
                .cursor_pointer()
                .hover(move |d| d.bg(hover))
                .child(div().flex_none().font_family(t.mono_font.clone()).text_size(px(11.)).text_color(t.muted).child(r))
                .child(div().flex_1().min_w_0().truncate().child(title))
                .on_click(cx.listener(move |m, _, _, cx| {
                    m.menu = None;
                    m.open_issue(target.clone(), cx)
                })),
        );
    }
    Some(kit::popover(at, menu))
}

/// The run buttons wait for the goal's tasks (`loaded`); a `/goals` row alone can't tell which apply.
fn header(m: &MainWindow, t: &Theme, g: &Value, loaded: bool, cx: &mut Context<MainWindow>) -> Div {
    let gr = fmt::ref_of(g, "G");
    let epic: Option<AnyElement> = fmt::opt_s(g, "epic_key").map(|key| {
        let url = fmt::opt_s(g, "epic_url").filter(|u| u.starts_with("http://") || u.starts_with("https://")).map(str::to_string);
        let label = format!("{key}{}", fmt::opt_s(g, "epic_status").map(|x| format!(" · {x}")).unwrap_or_default());
        match url {
            Some(u) => kit::link(t, "goal-epic", label).text_size(px(12.)).on_click(move |_, _, cx| cx.open_url(&u)).into_any_element(),
            None => div().text_size(px(12.)).text_color(t.muted).child(label).into_any_element(),
        }
    });
    let project = project_of(g).map(|p| {
        let hover = t.text;
        let open = p.clone();
        div()
            .id("goal-project")
            .cursor_pointer()
            .hover(move |s| s.text_color(hover))
            .on_click(cx.listener(move |m, _, _, cx| open_project(m, &open, cx)))
            .px(px(7.))
            .py(px(1.))
            .rounded(px(9.))
            .bg(t.panel_2)
            .text_size(px(11.))
            .text_color(t.muted)
            .child(p)
            .when_some(fmt::opt_s(g, "repo_path").map(str::to_string), |d, r| d.tooltip(kit::tip(r)))
    });
    let (parts, lead, tail) = progress(g);
    let not_met = outcome_not_met(g);
    div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(16.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(6.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(div().text_size(px(12.)).font_family(t.mono_font.clone()).text_color(t.muted).child(sel(t, "goal-ref", gr)))
                                .children(project)
                                .children(epic),
                        )
                        .child(div().text_size(px(26.)).font_weight(FontWeight::BOLD).line_height(relative(1.2)).child(sel(t, "goal-name", s(g, "name").to_string())))
                        .when_some(fmt::opt_s(g, "tldr").map(str::to_string), |d, tl| {
                            d.child(div().max_w(px(820.)).text_size(px(14.)).line_height(relative(1.5)).text_color(t.text_2).child(sel(t, "goal-tldr", tl)))
                        }),
                )
                .when(loaded, |d| d.child(buttons(m, t, g, cx))),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(16.))
                .child(div().flex_1().min_w_0().child(status::bar(t, &parts, 10.)))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .whitespace_nowrap()
                        .text_size(px(13.))
                        .text_color(t.text_2)
                        .child(div().font_weight(FontWeight::BOLD).text_color(t.text).child(lead))
                        .child(tail)
                        .when_some(not_met, |d, why| {
                            d.child(
                                div()
                                    .id("goal-not-met")
                                    .pl(px(10.))
                                    .text_color(t.warn_text)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .cursor_pointer()
                                    .hover(|s| s.underline())
                                    .child(why)
                                    .on_click(cx.listener(|m, e: &ClickEvent, _, cx| {
                                        let p = e.position();
                                        m.toggle_menu(MISSES_MENU, point(p.x - px(300.), p.y + px(14.)), cx);
                                    })),
                            )
                        }),
                ),
        )
}

fn gate(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Option<Div> {
    let gr = fmt::ref_of(g, "G");
    let note = inline_note(m, t, &format!("ggate:{gr}"));
    let Some((label, text, (act, button))) = gate_view(g) else {
        return note.map(|n| div().child(n));
    };
    let key = if act == "goal-deprio" { format!("goal-deprio:off:{gr}") } else { format!("goal-pause:off:{gr}") };
    let is_busy = busy(m, &key);
    let bt = kit::btn_primary(t, "goal-gate", if is_busy { "Sending…" } else { button });
    let bt = if is_busy {
        kit::disabled(bt)
    } else {
        bt.on_click(cx.listener(move |m, _, _, cx| if act == "goal-deprio" { bring_back(m, &gr, cx) } else { set_paused(m, &gr, false, cx) }))
    };
    Some(
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(18.))
            .py(px(12.))
            .rounded(px(12.))
            .bg(t.warn_soft)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(11.)).font_weight(FontWeight::BOLD).text_color(t.warn_fg).child(label.to_uppercase()))
                            .child(div().text_size(px(13.)).text_color(t.warn_text).child(text)),
                    )
                    .child(div().flex_none().child(bt)),
            )
            .children(note),
    )
}

fn how_runs_el(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Div {
    let mut kv = div().flex().flex_col().gap(px(6.)).text_size(px(13.));
    for (k, v) in how_runs(g) {
        kv = kv.child(div().flex().child(div().flex_none().w(px(96.)).text_color(t.muted).child(k)).child(div().flex_1().min_w_0().child(v)));
    }
    if m.jira_on() {
        kv = kv.child(jira_row(m, t, g, cx));
    }
    side(t, "How it runs", None, kv.children(inline_note(m, t, &format!("gjira:{}", fmt::ref_of(g, "G")))))
}

/// Jira On/Off for the goal's tasks: off, none of them asks for a ticket unless it opts in.
fn jira_row(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Div {
    let gr = fmt::ref_of(g, "G");
    let off = b(g, "jira_off");
    let is_busy = busy(m, &format!("goal-jira::{gr}"));
    let control = kit::seg(t, "goal-jira", &["On", "Off"], off as usize, |i, item| {
        let gr = gr.clone();
        if is_busy || (i == 1) == off { item } else { item.on_click(cx.listener(move |m, _, _, cx| set_jira_off(m, &gr, i == 1, cx))) }
    });
    div()
        .flex()
        .items_center()
        .child(div().flex_none().w(px(96.)).text_color(t.muted).child("Jira tickets"))
        .child(div().flex_1().min_w_0().flex().child(control))
}

/// An aside head's Show or Hide, flipping the section's open flag.
fn fold_toggle(t: &Theme, id: &'static str, open: bool, cx: &mut Context<MainWindow>, flag: fn(&mut MainWindow) -> &mut bool) -> Stateful<Div> {
    let hover = t.accent;
    div()
        .id(id)
        .text_size(px(12.))
        .font_weight(FontWeight::NORMAL)
        .text_color(t.accent)
        .cursor_pointer()
        .hover(move |s| s.text_color(hover).underline())
        .child(if open { "Hide" } else { "Show" })
        .on_click(cx.listener(move |m, _, _, cx| {
            let f = flag(m);
            *f = !*f;
            cx.notify();
        }))
}

/// The Notes head: the count and Show or Hide; open, the kind filter under it.
fn notes_head(m: &MainWindow, t: &Theme, v: &lists::Notes, cx: &mut Context<MainWindow>) -> Div {
    let open = m.goal_page.notes_open;
    let count = if v.filter.is_some() { format!("{} of {}", v.shown.len(), v.total) } else { v.total.to_string() };
    let head = side_head(t, "Notes", Some(count)).child(div().flex_1()).child(fold_toggle(t, "goal-notes-toggle", open, cx, |m| &mut m.goal_page.notes_open));
    if !open {
        return side_box(t).child(head);
    }
    let mut out = side_piece(t, true, v.shown.is_empty() && !v.more, 0., head.pb(px(8.)));
    if v.groups.len() > 1 {
        let groups = v.groups.clone();
        let labels: Vec<&str> = std::iter::once("All").chain(groups.iter().map(|k| match *k {
            "Decision" => "Decisions",
            "Finding" => "Findings",
            _ => "References",
        })).collect();
        let on = v.filter.and_then(|k| groups.iter().position(|x| *x == k)).map_or(0, |i| i + 1);
        out = out.child(div().pb(px(10.)).child(kit::seg(t, "goal-note-kind", &labels, on, |i, item| {
            let pick = i.checked_sub(1).map(|i| groups[i]);
            item.on_click(cx.listener(move |m, _, _, cx| {
                m.goal_page.note_filter = pick;
                cx.notify();
            }))
        })));
    }
    out
}

/// One note, the `ix`th shown.
fn note_piece(t: &Theme, x: &NoteItem, ix: usize, last: bool) -> Div {
    let fg = match x.tone {
        "goal" => t.goal,
        "results" => t.results,
        _ => t.muted,
    };
    let note = div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(div().text_size(px(11.)).font_weight(FontWeight::BOLD).text_color(fg).child(x.kind.clone()))
        .child(div().text_size(px(13.)).line_height(relative(1.45)).child(inline_el(t, &inline_segments(&x.text), format!("gnote-{ix}"))));
    side_piece(t, false, last, if ix == 0 { 0. } else { 10. }, note)
}

/// Under the notes while older ones are still to come.
fn notes_more(t: &Theme, any: bool) -> Div {
    side_piece(t, false, true, if any { 10. } else { 0. }, kit::help(t, "Loading…"))
}

fn backlog_head(t: &Theme, n: usize) -> Div {
    side_piece(t, true, false, 0., side_head(t, "Backlog", Some(format!("{n} open"))).pb(px(8.)))
}

/// One open issue of the Backlog section, the `ix`th.
fn issue_piece(t: &Theme, Issue { r, title, misses_goal }: Issue, ix: usize, last: bool, cx: &mut Context<MainWindow>) -> Div {
    let hover = t.accent;
    let target = r.clone();
    let row = div()
        .id(SharedString::from(format!("goal-issue-{r}")))
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(13.))
        .cursor_pointer()
        .hover(move |s| s.text_color(hover))
        .child(div().flex_none().font_family(t.mono_font.clone()).text_size(px(11.)).text_color(t.muted).child(r))
        .child(div().flex_1().min_w_0().truncate().child(title))
        .when(misses_goal, |d| d.child(kit::pill(t.warn_text, t.warn_soft, "Misses goal")))
        .on_click(cx.listener(move |m, _, _, cx| m.open_issue(target.clone(), cx)));
    side_piece(t, false, last, if ix == 0 { 0. } else { 6. }, row)
}

/// "N more" under the first few open issues: lists them all.
fn backlog_more(t: &Theme, more: usize, cx: &mut Context<MainWindow>) -> Div {
    let link = kit::link(t, "goal-backlog-more", format!("{more} more")).text_size(px(12.)).on_click(cx.listener(|m, _, _, cx| {
        m.goal_page.backlog_view = true;
        cx.notify();
    }));
    side_piece(t, false, true, 6., link)
}

pub fn render(m: &mut MainWindow, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let want = match &m.page {
        Page::Goal(r) => r.clone(),
        _ => String::new(),
    };
    if m.goal_page.for_goal != want {
        m.goal_page.reset_for(&want);
    }
    let loaded = m.data.goal.as_ref().is_some_and(|g| fmt::ref_of(g, "G") == want);
    // Until the goal's own answer lands, its `/goals` row fills the header and progress.
    let row = || m.data.goals.iter().find(|g| fmt::ref_of(g, "G") == want).cloned().map(Rc::new);
    let Some(g) = (if loaded { m.data.goal.clone() } else { row() }) else {
        return div().flex_1().pt(px(20.)).px(px(36.)).child(loading(m, &t)).into_any_element();
    };
    let body = if loaded {
        lists::sync(m, &g, cx);
        lists::body(m, cx)
    } else {
        div().pt(px(20.)).px(px(36.)).child(loading(m, &t))
    };
    // The header stays put; under it the wave cards and the aside each scroll as a list that
    // draws only what's in view.
    div()
        .id("goal-page")
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .line_height(relative(1.5))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .gap(px(16.))
                .pt(px(20.))
                .px(px(36.))
                .child(crumbs(&t, &g, cx))
                .child(header(m, &t, &g, loaded, cx))
                .children(gate(m, &t, &g, cx)),
        )
        .child(body)
        .when(loaded, |d| d.children(start_menu(m, &t, &g, cx)).children(wait_menu(m, &t, &g, cx)).children(misses_menu(m, &t, &g, cx)))
        .into_any_element()
}

/// Where the goal's answer will show: why it failed, or that it's on its way.
fn loading(m: &MainWindow, t: &Theme) -> Div {
    match m.data.errs.goal.clone() {
        Some(e) => div().text_size(px(12.5)).text_color(t.down).font_weight(FontWeight::MEDIUM).child(e),
        None => kit::help(t, "Loading…"),
    }
}

fn inline_el(t: &Theme, segs: &[Inline], key: String) -> AnyElement {
        let mut text = String::new();
        let mut hl: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
        let mut links: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        for seg in segs {
            let start = text.len();
            match seg {
                Inline::Text(x) => text.push_str(x),
                Inline::Code(x) => {
                    text.push_str(x);
                    hl.push((start..text.len(), HighlightStyle { background_color: Some(t.bg), ..Default::default() }));
                }
                Inline::Link(u) => {
                    text.push_str(u);
                    hl.push((start..text.len(), HighlightStyle { color: Some(t.accent_fg), underline: Some(UnderlineStyle { thickness: px(1.), color: Some(t.accent_fg), wavy: false }), ..Default::default() }));
                    links.push((start..text.len(), u.clone()));
                }
            }
        }
        let styled = StyledText::new(text).with_highlights(hl);
        if links.is_empty() {
            return styled.into_any_element();
        }
        let ranges: Vec<_> = links.iter().map(|(r, _)| r.clone()).collect();
        let urls: Vec<String> = links.into_iter().map(|(_, u)| u).collect();
        InteractiveText::new(SharedString::from(key), styled).on_click(ranges, move |ix, _, cx| if let Some(u) = urls.get(ix) { cx.open_url(u) }).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parity::{self, golden};
    // An explicit import beats the glob: `#[gpui_kit::test]` expands to `#[test]`, which must be std's.
    use ::core::prelude::v1::test;

    /// The run signals still match the web board's, less Deprioritize (only the banner's "Bring it
    /// back" is left) and with the plan button named "Open in Claude".
    #[test]
    fn run_signals_match_web() {
        let g = golden("goal");
        let keep = ["goalState", "runButtons", "startMenu", "opensAt", "gate"];
        let cases = g
            .cases
            .iter()
            .filter(|c| keep.contains(&c["input"]["fn"].as_str().unwrap_or("")))
            .cloned()
            .map(|mut c| {
                if c["input"]["fn"] == "runButtons" {
                    let bs: Vec<Value> = arr(&c["expect"], "buttons")
                        .iter()
                        .filter(|b| b["act"] != "goal-deprio-ask")
                        .map(|b| {
                            let mut b = b.clone();
                            if b["act"] == "goal-plan-edit" {
                                b["label"] = json!("Open in Claude");
                            }
                            b
                        })
                        .collect();
                    c["expect"]["buttons"] = json!(bs);
                }
                c
            })
            .collect();
        parity::Golden { cases }.check(|i| {
            let goal = &i["goal"];
            match i["fn"].as_str().unwrap() {
                "goalState" => json!(goal_state(goal).0),
                "runButtons" => {
                    let (split, bs) = run_buttons_view(goal, &i["hours"]);
                    json!({"split": split, "buttons": bs.iter().map(|b| json!({"act": b.act, "label": b.label, "title": b.title})).collect::<Vec<_>>()})
                }
                "waitMenu" => json!(wait_menu_view().into_iter().map(|(a, t, s)| json!({"act": a, "title": t, "sub": s})).collect::<Vec<_>>()),
                "startMenu" => json!(start_menu_view(goal, &i["hours"]).into_iter().map(|(a, t, s)| json!({"act": a, "title": t, "sub": s})).collect::<Vec<_>>()),
                "opensAt" => {
                    let (d, a) = opens_at(&i["hours"]);
                    json!([d, a])
                }
                "gate" => match gate_view(goal) {
                    Some((label, text, (act, button))) => json!({"label": label, "text": text, "buttons": [{"act": act, "label": button}]}),
                    None => json!({"label": null, "text": null, "buttons": []}),
                },
                f => panic!("no app function for {f}"),
            }
        });
    }

    #[test]
    fn run_toasts_match_web() {
        let g = golden("goal").only("action");
        let find = |n: &str| g.cases.iter().find(|c| c["name"] == format!("action {n}")).map(|c| c["expect"].clone()).unwrap();
        let open = json!({"on": true, "open": true});
        let closed = json!({"on": true, "open": false, "next_open": "2026-10-08T06:00"});
        assert_eq!(json!({"toast": run_toast(&json!({"queued_now": 2}), &open)}), find("run")["feedback"]);
        assert_eq!(json!({"toast": run_toast(&json!({"queued_now": 0}), &open)}), find("run, nothing queued")["feedback"]);
        assert_eq!(json!({"toast": run_toast(&json!({"queued_now": 3}), &closed)}), find("run after hours")["feedback"]);
        assert_eq!(json!({"toast": run_now_toast(&json!({"queued_now": 2}))}), find("run now")["feedback"]);
    }

    #[test]
    fn run_toast_says_what_it_resumed_and_why_the_rest_wait() {
        let open = json!({"on": true, "open": true});
        assert_eq!(run_toast(&json!({"queued_now": 0, "resumed_now": 1}), &open), "Started: 1 parked task resumed");
        assert_eq!(
            run_toast(&json!({"queued_now": 0, "resumed_now": 1, "waiting": ["T192 waits for you to do H197"]}), &open),
            "Started: 1 parked task resumed. T192 waits for you to do H197."
        );
        assert_eq!(
            run_toast(&json!({"queued_now": 2, "waiting": ["T1 waits for you to do H2", "T3 is blocked by T4", "T5 waits for T6"]}), &open),
            "Started: 2 tasks queued. T1 waits for you to do H2. T3 is blocked by T4. 1 more wait."
        );
        assert_eq!(
            run_now_toast(&json!({"queued_now": 1, "resumed_now": 2})),
            "Started now: 1 task queued, 2 parked tasks resumed. The goal runs until work hours open."
        );
    }

    #[test]
    fn progress_counts_each_status_in_the_bars_order() {
        let g = json!({
            "tasks": [{"status": "done"}, {"status": "working"}, {"status": "planned"}, {"status": "needs"}, {"status": "done"}, {"status": "queued"}],
            "shared": [{"status": "working"}],
        });
        let (parts, lead, tail) = progress(&g);
        assert_eq!(parts, vec![(St::Done, 2), (St::Working, 2), (St::Needs, 1), (St::Queued, 1), (St::Planned, 1)]);
        assert_eq!(lead, "2 of 7");
        assert_eq!(tail, " done · 2 working · 1 needs you · 1 queued · 1 planned");
        let (parts, lead, tail) = progress(&json!({"tasks": []}));
        assert!(parts.is_empty());
        assert_eq!((lead.as_str(), tail.as_str()), ("0 of 0", " done"));
    }

    /// A `/goals` row, before the goal's own answer, counts the same from its summary.
    #[test]
    fn progress_of_a_goals_row_counts_from_its_summary() {
        let row = json!({"done": 2, "total": 7, "active": 3, "needs": 1, "review": 0, "failed": 0, "queued": 1, "planned": 1});
        let (parts, lead, tail) = progress(&row);
        assert_eq!(parts, vec![(St::Done, 2), (St::Working, 2), (St::Needs, 1), (St::Queued, 1), (St::Planned, 1)]);
        assert_eq!(lead, "2 of 7");
        assert_eq!(tail, " done · 2 working · 1 needs you · 1 queued · 1 planned");
    }

    #[test]
    fn how_it_runs_shows_the_rules_and_the_stops_ahead() {
        let g = json!({
            "max_terminals": 3, "run_in_order": true, "auto_close": false,
            "tasks": [{"status": "working"}, {"status": "needs"}, {"status": "queued"}],
            "waves": [{"wave": 1, "stop_after": true, "state": "done"}, {"wave": 2, "stop_after": true, "state": "running"}, {"wave": 3}],
        });
        assert_eq!(
            how_runs(&g),
            vec![
                ("Terminals", "3 at most · 2 in use".to_string()),
                ("Order", "In order, by wave".to_string()),
                ("When done", "Terminals stay open".to_string()),
                ("Review stop", "After wave 2".to_string()),
            ]
        );
        let rows = how_runs(&json!({"run_in_order": 0, "tasks": []}));
        assert_eq!(rows[0].1, "2 at most · 0 in use", "no max: the board's default");
        assert_eq!(rows[1].1, "Side by side");
        assert_eq!(rows[2].1, "Terminals close");
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn notes_put_pinned_first_then_newest() {
        let notes = vec![
            json!({"kind": "finding", "text": "Late  one", "at": "2026-10-09T10:00:00Z", "source": "T12"}),
            json!({"kind": "reference", "text": "Old", "at": "2026-10-01T10:00:00Z"}),
            json!({"kind": "decision", "text": "Pinned", "at": "2026-10-10T10:00:00Z", "pinned": true, "source": "T3"}),
        ];
        let got = note_items(&notes);
        assert_eq!(got[0], NoteItem { group: "Decision", kind: "Decision · pinned · T3".into(), tone: "goal", text: "Pinned".into() });
        assert_eq!(got[1], NoteItem { group: "Finding", kind: "Finding · T12".into(), tone: "results", text: "Late one".into() });
        assert_eq!((got[2].kind.as_str(), got[2].tone), ("Reference", "muted"));
        assert_eq!(note_groups(&got), vec!["Decision", "Finding", "Reference"]);
        assert_eq!(note_groups(&got[1..2]), vec!["Finding"], "one kind: no filter to pick");
    }

    #[test]
    fn backlog_lists_only_open_issues() {
        let g = json!({"backlog": [
            {"ref": "B1", "title": "One"},
            {"ref": "B2", "title": "Two", "state": "closed"},
            {"ref": "B3", "title": "Three", "state": "open"},
        ]});
        let refs: Vec<String> = open_issues(&g).into_iter().map(|x| x.r).collect();
        assert_eq!(refs, ["B1", "B3"]);
    }

    #[test]
    fn open_misses_skip_settled_ones() {
        let g = json!({
            "tasks": [{"ref": "T1", "status": "done"}, {"ref": "T2", "status": "working"}],
            "backlog": [
                {"ref": "B1", "title": "Open", "state": "open", "misses_goal": true},
                {"ref": "B2", "title": "Not a miss", "state": "open"},
                {"ref": "B3", "title": "Task done", "state": "task", "task_ref": "T1", "misses_goal": true},
                {"ref": "B4", "title": "Task working", "state": "task", "task_ref": "T2", "misses_goal": true},
                {"ref": "B5", "title": "Closed", "state": "closed", "misses_goal": true},
            ],
        });
        let got: Vec<String> = open_misses(&g).into_iter().map(|x| x.r).collect();
        assert_eq!(got, ["B1", "B4"]);
    }

    #[test]
    fn backlog_lists_misses_on_the_goal_first() {
        let g = json!({"backlog": [
            {"ref": "B1", "title": "One"},
            {"ref": "B2", "title": "Two", "misses_goal": true},
            {"ref": "B3", "title": "Three"},
        ]});
        let got = open_issues(&g);
        assert_eq!(got.iter().map(|x| x.r.as_str()).collect::<Vec<_>>(), ["B2", "B1", "B3"]);
        assert!(got[0].misses_goal && !got[1].misses_goal);
    }

    #[test]
    fn says_the_outcome_is_not_met_while_a_miss_is_open() {
        let g = json!({"tasks": [{"status": "done"}, {"status": "done"}], "misses": 2});
        assert_eq!(outcome_not_met(&g).as_deref(), Some("Outcome not met · 2 misses"));
        assert_eq!(goal_state(&g), ("Outcome not met".to_string(), "warn"));
        assert_eq!(outcome_not_met(&json!({"tasks": [{"status": "done"}]})), None);
    }

    #[test]
    fn note_text_links_urls_and_code() {
        let segs = inline_segments("See https://x.example/a and `cargo test` now");
        assert!(segs.iter().any(|s| matches!(s, Inline::Link(u) if u == "https://x.example/a")));
        assert!(segs.iter().any(|s| matches!(s, Inline::Code(c) if c == "cargo test")));
    }

    /// Start posts the run; the header's Pause pauses; the banner's Resume unpauses.
    #[gpui_kit::test]
    fn the_header_signals_post_to_the_goal(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = parity::window(cx);
        w.update(cx, |m, _, cx| open(m, "G1", false, cx)).unwrap();
        parity::settle(cx);
        w.update(cx, |m, _, cx| {
            run_goal(m, "G1", false, cx);
            set_paused(m, "G1", true, cx);
        })
        .unwrap();
        parity::settle(cx);
        w.update(cx, |m, _, cx| set_paused(m, "G1", false, cx)).unwrap();
        parity::settle(cx);
        let posts = rec.posts.lock().unwrap().clone();
        assert!(posts.iter().any(|(p, _)| p == "goals/G1/run"), "{posts:?}");
        assert!(posts.iter().any(|(p, b)| p == "goals/G1" && b["paused"] == json!(true)), "{posts:?}");
        assert!(posts.iter().any(|(p, b)| p == "goals/G1" && b["paused"] == json!(false)), "{posts:?}");
    }
}
