//! A goal's page: the web board's `goalMain` and friends (pages.js), one to one.
//!
//! A header with the goal's state and its run buttons (Start / Start tomorrow ▾ Start now, Resume,
//! Pause, Plan in Claude, Deprioritize), then Tasks or Backlog on the left; with Tasks, Attached and
//! Goal notes on the right. The backlog's selected issue opens in the issue panel (the web showed it
//! in the right column).
//!
//! Every visible string and every request comes from a pure function below (`*_view`, `*_toast`,
//! the action functions), which `parity/gen/goal.mjs` checks against the web board's own code.
//! Feedback follows the web's `run()`: an action with a group (`gset:G1`, `gnote:G1`, `ggate:G1`,
//! `att:goals:G1`, `issue:B1`, `bulk`) shows its result as an inline note there; one without a group
//! shows a toast; a busy action disables its button and shows its busy label.
use crate::app::{MainWindow, Page, Panel};
use crate::fmt::{self, arr, b, i, s};
use crate::theme::Theme;
use crate::ui::text_input::FieldChanged;
use crate::ui::{kit, modals};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

const START_MENU: &str = "goal-start";
const MOVE_MENU: &str = "goal-bulk-move";
const ATT_MENU: &str = "goal-att";
const NOTE_KINDS: [(&str, &str); 3] = [("finding", "Finding"), ("decision", "Decision"), ("reference", "Reference")];
const MAX_TERMINALS: [i64; 7] = [1, 2, 3, 4, 5, 6, 8];
const ATT_KINDS: [(&str, &str); 6] = [("design", "Design"), ("proposal", "Proposal"), ("doc", "Doc"), ("evidence", "Evidence"), ("results", "Results"), ("other", "Link")];
const FOLD_NOTES: &str = "tb.fold.gnotes";
const BACKLOG_HELP: &str = "Issues found while testing, building or reviewing that no task covers yet. Terminals add them instead of fixing them on the side.";
const NOTES_HELP: &str = "Every task in this goal gets these, and the open backlog, in its handoff. A new terminal starts with what earlier tasks learned.";
const ATTACH_HELP: &str = "Designs, proposals and docs for the whole goal. Every task’s handoff lists them.";

/// The bulk move's goal picker (the web's picker dialog for kind `goal`).
struct Picker {
    input: kit::Input,
    active: usize,
    _sub: Subscription,
}

/// An attachment being edited (`S.attEdit` and its drafts). A field is sent only once touched.
struct AttEdit {
    id: i64,
    kind: Option<String>,
    title: kit::Input,
    url: kit::Input,
    touched: HashSet<&'static str>,
    _subs: Vec<Subscription>,
}

#[derive(Default)]
pub struct State {
    // Reset when the goal changes (the web's resetPageState).
    for_goal: String,
    pub backlog_view: bool,
    /// Picked backlog refs, in the order picked (`P.sel`).
    sel: Vec<String>,
    anchor: Option<String>,
    /// Issues changed here, kept in the list where they were (`P.kept` / `P.order`).
    kept: HashMap<String, Value>,
    order: Vec<String>,
    show_dropped: bool,
    deprio_open: bool,
    note_dialog: Option<Value>,
    dialog_focus: Option<FocusHandle>,
    picker: Option<Picker>,
    // Kept across goals, like the web's S.notes / S.busy / S.drafts / S.reveal / S.attEdit.
    notes: HashMap<String, (String, bool)>,
    busy: HashSet<String>,
    note_inputs: HashMap<String, kit::Input>,
    note_kinds: HashMap<String, &'static str>,
    note_open: HashSet<String>,
    att_edit: Option<AttEdit>,
}

impl State {
    pub fn reset_for(&mut self, goal: &str) {
        self.for_goal = goal.to_string();
        self.backlog_view = std::env::var("TASKBOARD_GOAL_VIEW").is_ok_and(|v| v == "backlog");
        self.sel.clear();
        self.anchor = None;
        self.kept.clear();
        self.order.clear();
        self.show_dropped = false;
        self.deprio_open = false;
        self.note_dialog = None;
        self.picker = None;
    }
}

/// Open a goal's page, on its Backlog tab when `backlog` (`#/goals/G3?view=backlog`).
pub fn open(m: &mut MainWindow, r: &str, backlog: bool, cx: &mut Context<MainWindow>) {
    m.go(Page::Goal(r.to_string()), cx);
    m.goal_page.reset_for(r);
    m.goal_page.backlog_view = backlog;
    cx.notify();
}

// ------------------------------------------------------------------ shared lookups (app.js)

/// `stKey`.
fn status_key(t: &Value) -> &'static str {
    match s(t, "status") {
        "done" if b(t, "failed") => "failed",
        "queued" if b(t, "blocked") => "blocked",
        "planned" => "planned",
        "working" => "working",
        "needs" => "needs",
        "done" => "done",
        _ => "queued",
    }
}

/// `STATUS`.
fn status_label(k: &str) -> &'static str {
    match k {
        "planned" => "Planned",
        "blocked" => "Blocked",
        "working" => "Working",
        "needs" => "Needs you",
        "done" => "Done",
        "failed" => "Failed",
        _ => "Queued",
    }
}

/// The chip colors (`.st-*` in app.css).
fn status_tone(k: &str) -> &'static str {
    match k {
        "planned" => "planned",
        "working" => "accent",
        "needs" | "blocked" => "warn",
        "done" => "up",
        "failed" => "down",
        _ => "queued",
    }
}

/// `prOpen`: a PR with no state counts as open.
fn pr_open(pr: &Value) -> bool {
    fmt::opt_s(pr, "state").unwrap_or("OPEN").eq_ignore_ascii_case("open")
}

fn has_pr(t: &Value) -> bool {
    t["pr"].is_object() && !t["pr"]["num"].is_null()
}

/// `awaitingMerge`.
fn awaiting_merge(t: &Value) -> bool {
    s(t, "status") == "done" && !b(t, "failed") && has_pr(t) && pr_open(&t["pr"])
}

/// `prStopped`.
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

/// A number field as JS prints it (`#12`).
fn num_text(v: &Value) -> String {
    match v {
        Value::String(x) => x.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

/// `KINDS` / `kindLabel`.
pub fn kind_label(k: &str) -> String {
    match k {
        "bug" => "Bug".into(),
        "gap" => "Test gap".into(),
        "follow" => "Follow-up".into(),
        "clean" => "Clean-up".into(),
        "" => "Issue".into(),
        k => fmt::cap(k),
    }
}

/// `CHECKS` (`checksOf(pr)[0]`).
fn checks_label(pr: &Value) -> &'static str {
    match s(pr, "checks").to_lowercase().as_str() {
        "pass" => "Passed",
        "fail" => "Failed",
        "pending" => "Running",
        "none" => "No checks",
        _ => "Unknown",
    }
}

/// `issueState(b).text`.
fn issue_state_text(bl: &Value) -> Option<String> {
    let tr = bl.get("task_id").filter(|v| !v.is_null()).map(|v| format!("T{}", num_text(v)));
    match s(bl, "state") {
        "task" => Some(tr.map(|t| format!("Made into task {t}")).unwrap_or_else(|| "Made into a task".into())),
        "ticket" => Some(fmt::opt_s(bl, "jira_key").map(|k| format!("Jira {k}")).unwrap_or_else(|| "Jira ticket being created".into())),
        "drop" => Some("Closed as won’t do".into()),
        _ => None,
    }
}

/// `issueFrom(b, long)`.
pub fn issue_from(bl: &Value, long: bool) -> String {
    let ft = bl.get("found_by_task").filter(|v| is_truthy(v)).map(|f| fmt::ref_of(f, "T"));
    let name = fmt::opt_s(bl, "found_by_name");
    let source = fmt::opt_s(bl, "source");
    let who = if source == Some("you") || (source.is_none() && name.is_none() && ft.is_none()) {
        "Added by you".to_string()
    } else if source == Some("answer") {
        "From your answer".to_string()
    } else if let (true, Some(r)) = (long, &ft) {
        format!("Found by {r} · {}", name.unwrap_or("a terminal"))
    } else {
        name.unwrap_or("Found by a terminal").to_string()
    };
    [who, fmt::hhmm(s(bl, "created_at"))].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// JS `String.length` (UTF-16 code units).
fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `String.prototype.localeCompare` (node-verified port in the forms module).
use crate::ui::modals::locale_cmp;

// ------------------------------------------------------------------ goal state and counts

/// `goalState(tasks, g)`: the goal's state label and its chip color.
pub fn goal_state(g: &Value) -> (String, &'static str) {
    let tasks = arr(g, "tasks");
    if !tasks.is_empty() && tasks.iter().all(|t| s(t, "status") == "done") {
        let open: Vec<&Value> = tasks.iter().filter(|t| awaiting_merge(t)).collect();
        if open.iter().any(|t| pr_stopped(t)) {
            return ("Waiting on you".into(), "warn");
        }
        if !open.is_empty() {
            return (if open.len() > 1 { format!("{} PRs awaiting merge", open.len()) } else { "Awaiting merge".into() }, "accent");
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

struct Counts {
    n: i64,
    done: i64,
    active: i64,
    queued: i64,
}

/// `goalCounts(g)` for a goal detail (it has its tasks).
fn counts(g: &Value) -> Counts {
    let tasks = arr(g, "tasks");
    let count = |f: &dyn Fn(&Value) -> bool| tasks.iter().filter(|t| f(t)).count() as i64;
    Counts {
        n: tasks.len() as i64,
        done: count(&|t| s(t, "status") == "done"),
        active: count(&|t| matches!(s(t, "status"), "working" | "needs")),
        queued: count(&|t| s(t, "status") == "queued"),
    }
}

/// `goalPrCount(tasks)`: "2 of 3 PRs merged" / "1 PR open" / "".
fn pr_count(tasks: &[Value]) -> String {
    let prs: Vec<&Value> = tasks.iter().filter(|t| has_pr(t)).collect();
    let merged = prs.iter().filter(|t| s(&t["pr"], "state").eq_ignore_ascii_case("merged")).count() as i64;
    let n = prs.len() as i64;
    if n == 0 {
        String::new()
    } else if merged > 0 {
        format!("{merged} of {} merged", fmt::plural(n, "PR", "PRs"))
    } else {
        format!("{} open", fmt::plural(n, "PR", "PRs"))
    }
}

fn open_issue_count(g: &Value) -> i64 {
    arr(g, "backlog").iter().filter(|x| fmt::opt_s(x, "state").unwrap_or("open") == "open").count() as i64
}

// ------------------------------------------------------------------ header

pub struct HeaderView {
    pub pills: [String; 3],
    pub state_tone: &'static str,
    pub name: String,
    pub tldr: Option<String>,
    pub project: String,
    pub epic: Option<String>,
    pub epic_link: Option<String>,
    pub counts: String,
    pub tabs: [String; 2],
    pub backlog_hot: bool,
}

/// `goalMain`'s header and tab bar.
pub fn header_view(g: &Value, jira_on: bool) -> HeaderView {
    let (state, tone) = goal_state(g);
    let c = counts(g);
    let open = open_issue_count(g);
    let prs = pr_count(arr(g, "tasks"));
    let epic = match fmt::opt_s(g, "epic_key") {
        Some(key) => Some(format!("Jira epic {key}{}", fmt::opt_s(g, "epic_status").map(|x| format!(" · {x}")).unwrap_or_default())),
        None if jira_on => Some("No Jira epic".into()),
        None => None,
    };
    let epic_link = fmt::opt_s(g, "epic_key").and(fmt::opt_s(g, "epic_url")).map(|u| if u.starts_with("http://") || u.starts_with("https://") { u.to_string() } else { "#".into() });
    HeaderView {
        pills: ["Goal".into(), state, fmt::ref_of(g, "G")],
        state_tone: tone,
        name: s(g, "name").to_string(),
        tldr: fmt::opt_s(g, "tldr").map(str::to_string),
        project: s(g, "project").to_string(),
        epic,
        epic_link,
        counts: format!(
            "{} of {} done · {} active{} · {open} open in the backlog",
            c.done,
            c.n,
            c.active,
            if prs.is_empty() { String::new() } else { format!(" · {prs}") }
        ),
        tabs: [format!("Tasks {}", c.n), format!("Backlog {open}")],
        backlog_hot: open > 0,
    }
}

// ------------------------------------------------------------------ run buttons

fn after_hours(hours: &Value) -> bool {
    b(hours, "on") && !b(hours, "open")
}

/// `opensAt()`: when work hours open next, ("tomorrow", "6am"); ("later", "") when unknown.
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
    /// The web's `data-act`: goal-run, goal-pause, goal-plan-edit, goal-deprio-ask.
    pub act: &'static str,
    pub label: String,
    pub title: String,
    /// What the button says while its request runs.
    pub busy_label: &'static str,
}

/// `goalRunButtons(g)` (+ `startLater`): whether Start is the split "Start tomorrow ▾" button, and
/// the buttons in order.
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
    if going > 0 && !b(g, "paused") && !b(g, "deprioritized") && !unstarted {
        out.push(RunButton { act: "goal-pause", label: "Pause".into(), title: "Nothing new starts; running tasks carry on".into(), busy_label: "Sending…" });
    }
    let done = !tasks.is_empty() && tasks.iter().all(|x| s(x, "status") == "done");
    out.push(RunButton {
        act: "goal-plan-edit",
        label: "Plan in Claude".into(),
        title: "Open a Claude terminal in Midna on this goal’s plan, to change it by talking it through".into(),
        busy_label: "Opening…",
    });
    if !b(g, "deprioritized") && !done {
        out.push(RunButton { act: "goal-deprio-ask", label: "Deprioritize".into(), title: "Take it off the board’s goal list; nothing new starts".into(), busy_label: "Sending…" });
    }
    (split, out)
}

/// The split button's menu: (act, title, sub).
pub fn start_menu_view(hours: &Value) -> Vec<(&'static str, String, String)> {
    let (day, at) = opens_at(hours);
    vec![
        ("goal-run", format!("Start {day}"), format!("Queued now, starts when work hours open{}", if at.is_empty() { String::new() } else { format!(" at {at}") })),
        ("goal-run-now", "Start now".into(), "Runs outside work hours until they open".into()),
    ]
}

/// The toast after `goal-run`.
pub fn run_toast(v: &Value, hours: &Value) -> String {
    let n = i(v, "queued_now");
    if n == 0 {
        "Going again".into()
    } else if after_hours(hours) {
        let (day, at) = opens_at(hours);
        let when = [day, at].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" at ");
        format!("{} queued. They start {when}.", fmt::plural(n, "task", "tasks"))
    } else {
        format!("Started: {} queued", fmt::plural(n, "task", "tasks"))
    }
}

/// The toast after `goal-run-now`.
pub fn run_now_toast(v: &Value) -> String {
    format!("Started now: {} queued. The goal runs until work hours open.", fmt::plural(i(v, "queued_now"), "task", "tasks"))
}

// ------------------------------------------------------------------ tasks

#[derive(Debug, PartialEq)]
pub struct TaskRow {
    pub r: String,
    pub n: usize,
    pub key: &'static str,
    pub chip: &'static str,
    pub title: String,
    pub meta: String,
    pub jira_tip: Option<String>,
    pub pr_text: Option<String>,
    pub pr_tip: Option<String>,
    pub pr_phase: String,
    pub planned: bool,
}

/// `goalTaskMeta(t, i, tasks, g)`.
pub fn task_meta(t: &Value, idx: usize, tasks: &[Value], g: &Value) -> String {
    let prev_open = || (0..idx).rev().find(|&j| s(&tasks[j], "status") != "done").map(|j| j + 1).unwrap_or(0);
    let who = fmt::opt_s(t, "who").map(str::to_string);
    let key = t.get("jira").and_then(|j| fmt::opt_s(j, "key")).map(str::to_string);
    let high = (s(t, "priority") == "high").then(|| "High".to_string());
    let after = |p: usize| (p > 0).then(|| format!("after task {p}"));
    let in_order = is_truthy(&g["run_in_order"]);
    let parts: Vec<Option<String>> = match s(t, "status") {
        "done" => vec![who.clone(), Some(format!("{} {}", if b(t, "failed") { "stopped" } else { "finished" }, fmt::ago(s(t, "when")))), key],
        _ if b(t, "lost") => vec![Some("Terminal lost".into()), who.clone()],
        "working" | "needs" => vec![
            who.clone(),
            Some(fmt::running_for(t)),
            Some(format!("{} {}", if fmt::opt_s(t, "question").is_some() { "asked" } else { "updated" }, fmt::ago(s(t, "when")))),
            key,
        ],
        "queued" if b(t, "starting") => vec![high, Some("starting".into()), key],
        "queued" if fmt::opt_s(t, "waiting").is_some() => vec![high, fmt::opt_s(t, "waiting").map(str::to_string), key],
        "queued" => vec![high, after(if in_order { prev_open() } else { 0 }).or(Some("starts when a terminal is free".into())), key],
        _ => vec![after(if in_order { prev_open() } else { 0 })],
    };
    let mut list: Vec<String> = parts.into_iter().flatten().filter(|x| !x.is_empty()).collect();
    if let Some(first) = list.first_mut() {
        if Some(first.as_str()) != who.as_deref() {
            *first = fmt::cap(first);
        }
    }
    // "finished " with no time: the web's trailing space never shows.
    list.join(" · ").trim_end().to_string()
}

/// One row of `goalTasks`.
pub fn task_row_view(t: &Value, idx: usize, tasks: &[Value], g: &Value) -> TaskRow {
    let key = status_key(t);
    let jira_tip = t.get("jira").and_then(|j| fmt::opt_s(j, "key").map(|k| format!("{k}{}", fmt::opt_s(j, "status").map(|x| format!(" · {x}")).unwrap_or_default())));
    let (pr_text, pr_tip, pr_phase) = if has_pr(t) {
        let pr = &t["pr"];
        let stage = pr.get("stage").filter(|v| v.is_object());
        let num = num_text(&pr["num"]);
        let tip = format!("PR #{num} · {}checks {}", stage.map(|st| format!("{} · ", s(st, "label"))).unwrap_or_default(), checks_label(pr).to_lowercase());
        (Some(format!("#{num}")), Some(tip), stage.map(|st| s(st, "phase").to_string()).unwrap_or_default())
    } else {
        (None, None, String::new())
    };
    TaskRow {
        r: fmt::ref_of(t, "T"),
        n: idx + 1,
        key,
        chip: status_label(key),
        title: s(t, "title").to_string(),
        meta: task_meta(t, idx, tasks, g),
        jira_tip,
        pr_text,
        pr_tip,
        pr_phase,
        planned: s(t, "status") == "planned",
    }
}

pub struct HowRuns {
    pub options: Vec<(i64, String)>,
    /// The option matching the goal's setting (none when it isn't one of them).
    pub selected: Option<String>,
    pub run_in_order: bool,
    pub auto_close: bool,
}

/// "How this goal runs".
pub fn how_runs_view(g: &Value) -> HowRuns {
    let raw = &g["max_terminals"];
    let n = match raw {
        Value::Number(x) => x.as_f64().unwrap_or(0.),
        Value::String(x) => x.trim().parse::<f64>().unwrap_or(0.),
        Value::Bool(true) => 1.,
        _ => 0.,
    };
    let max = if n == 0. || n.is_nan() { 2. } else { n };
    let checked = |k: &str| !matches!(g.get(k), Some(Value::Bool(false))) && !matches!(g.get(k), Some(Value::Number(x)) if x.as_f64() == Some(0.));
    let options: Vec<(i64, String)> = MAX_TERMINALS.iter().map(|&k| (k, fmt::plural(k, "terminal", "terminals"))).collect();
    HowRuns {
        selected: options.iter().find(|(k, _)| *k as f64 == max).map(|(_, l)| l.clone()),
        options,
        run_in_order: checked("run_in_order"),
        auto_close: checked("auto_close"),
    }
}

/// `gateBanner(g)`: (label, text, (act, button)).
pub fn gate_view(g: &Value) -> Option<(&'static str, &'static str, (&'static str, &'static str))> {
    if b(g, "deprioritized") {
        Some(("Deprioritized", "It’s off the board’s goal list and nothing new starts. Tasks already running carry on.", ("goal-deprio", "Bring it back")))
    } else if b(g, "paused") {
        Some(("Paused", "Nothing new starts in this goal. Tasks already running carry on.", ("goal-pause", "Resume the goal")))
    } else {
        None
    }
}

/// `gdeprioHtml`: title, the two paragraphs, the buttons.
pub fn deprio_dialog_view(g: &Value) -> (&'static str, [String; 2], [&'static str; 2]) {
    let tasks = arr(g, "tasks");
    let running = tasks.iter().filter(|x| matches!(s(x, "status"), "working" | "needs")).count() as i64;
    let queued = tasks.iter().filter(|x| s(x, "status") == "queued").count() as i64;
    let mut after = Vec::new();
    if running > 0 {
        after.push(format!("{} already running carr{} on.", fmt::plural(running, "task", "tasks"), if running == 1 { "ies" } else { "y" }));
    }
    if queued > 0 {
        after.push(format!("{} won’t start.", fmt::plural(queued, "queued task", "queued tasks")));
    }
    let name = fmt::opt_s(g, "name").map(str::to_string).unwrap_or_else(|| fmt::ref_of(g, "G"));
    (
        "Deprioritize this goal?",
        [
            format!("{name} goes off the board’s goal list and nothing new starts. {}", after.join(" ")).trim_end().to_string(),
            "You can bring it back from the Deprioritized list under Goals.".into(),
        ],
        ["Cancel", "Deprioritize"],
    )
}

// ------------------------------------------------------------------ notes

/// `noteFrom(n)`.
fn note_from(n: &Value) -> String {
    let at = fmt::opt_s(n, "at").map(fmt::hhmm).unwrap_or_default();
    match fmt::opt_s(n, "source") {
        None => at,
        Some("you") => if at.is_empty() { "you".into() } else { format!("you, {at}") },
        Some(src) => src.to_string(),
    }
}

/// `noteLong`.
fn note_long(t: &str) -> bool {
    js_len(t) > 220 || t.split('\n').count() > 4
}

#[derive(Debug)]
pub struct NoteItem {
    pub note: Value,
    /// As the browser shows it (white-space collapsed).
    pub text: String,
    pub from: String,
    pub long: bool,
    pub pinned: bool,
}

/// `notesAside`'s groups: (title, items). No notes: the "Nothing yet" group.
pub fn notes_view(notes: &[Value]) -> Vec<(&'static str, Vec<NoteItem>)> {
    let at_key = |n: &Value| match n.get("at") {
        None => "undefined".to_string(),
        Some(Value::Null) => "null".to_string(),
        Some(Value::String(x)) => x.clone(),
        Some(v) => v.to_string(),
    };
    let mut list: Vec<&Value> = notes.iter().collect();
    list.sort_by(|a, c| b(c, "pinned").cmp(&b(a, "pinned")).then_with(|| at_key(a).cmp(&at_key(c))));
    let groups: Vec<(&'static str, Vec<NoteItem>)> = [("finding", "Findings"), ("decision", "Decisions"), ("reference", "References")]
        .into_iter()
        .map(|(k, title)| {
            let items = list
                .iter()
                .filter(|n| {
                    let nk = s(n, "kind");
                    (if nk == "decision" || nk == "reference" { nk } else { "finding" }) == k
                })
                .map(|n| {
                    let t = s(n, "text").trim().to_string();
                    NoteItem { note: (*n).clone(), text: t.split_whitespace().collect::<Vec<_>>().join(" "), from: note_from(n), long: note_long(&t), pinned: b(n, "pinned") }
                })
                .collect::<Vec<_>>();
            (title, items)
        })
        .filter(|(_, l)| !l.is_empty())
        .collect();
    if groups.is_empty() {
        let empty = NoteItem { note: Value::Null, text: "Notes from tasks and from you collect here.".into(), from: String::new(), long: false, pinned: false };
        return vec![("Nothing yet", vec![empty])];
    }
    groups
}

/// `noteDialogHtml`'s title (after the "Pinned" mark).
pub fn note_dialog_title(n: &Value) -> String {
    let kind = match s(n, "kind") {
        "decision" => "Decision",
        "reference" => "Reference",
        _ => "Finding",
    };
    [kind.to_string(), note_from(n)].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(String),
    Link(String),
    Code(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum NoteBlock {
    /// Lines of a paragraph (the web joins them with line breaks).
    P(Vec<Vec<Inline>>),
    Ul(Vec<Vec<Inline>>),
    Pre(String),
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

/// `noteBody(text)`: paragraphs, bullet lists (indented lines continue a bullet) and indented code.
pub fn note_blocks(text: &str) -> Vec<NoteBlock> {
    #[derive(PartialEq, Clone, Copy)]
    enum K {
        Li,
        Code,
        P,
    }
    let mut out = Vec::new();
    let mut kind: Option<K> = None;
    let mut buf: Vec<String> = Vec::new();
    let flush = |kind: Option<K>, buf: &mut Vec<String>, out: &mut Vec<NoteBlock>| {
        if buf.is_empty() {
            return;
        }
        match kind {
            Some(K::Li) => out.push(NoteBlock::Ul(buf.iter().map(|l| inline_segments(l)).collect())),
            Some(K::Code) => out.push(NoteBlock::Pre(buf.join("\n"))),
            _ => out.push(NoteBlock::P(buf.iter().map(|l| inline_segments(l)).collect())),
        }
        buf.clear();
    };
    let bullet = |l: &str| {
        let t = l.trim_start();
        ["- ", "* ", "• "].iter().find_map(|m| t.strip_prefix(m)).map(str::to_string)
    };
    for raw in text.split('\n') {
        let line = raw.trim_end();
        let k = if line.trim().is_empty() {
            None
        } else if bullet(line).is_some() {
            Some(K::Li)
        } else if line.starts_with("  ") || line.starts_with('\t') {
            Some(K::Code)
        } else {
            Some(K::P)
        };
        if k == Some(K::Code) && kind == Some(K::Li) && !buf.is_empty() {
            let last = buf.len() - 1;
            buf[last] = format!("{} {}", buf[last], line.trim());
            continue;
        }
        if k != kind || k.is_none() {
            flush(kind, &mut buf, &mut out);
        }
        kind = k;
        match k {
            Some(K::Li) => buf.push(bullet(line).unwrap_or_default()),
            Some(K::Code) => buf.push(line.strip_prefix("  ").or_else(|| line.strip_prefix('\t')).unwrap_or(line).to_string()),
            Some(K::P) => buf.push(line.trim().to_string()),
            None => {}
        }
    }
    flush(kind, &mut buf, &mut out);
    out
}

// ------------------------------------------------------------------ attachments

fn att_kind(a: &Value) -> &'static str {
    let k = s(a, "kind");
    ATT_KINDS.iter().find(|(id, _)| *id == k).map(|(id, _)| *id).unwrap_or("other")
}

fn att_web(a: &Value) -> bool {
    let u = s(a, "url").to_lowercase();
    u.starts_with("http://") || u.starts_with("https://")
}

/// `attHref`: a web link, or a `~` / absolute file path the board can open.
fn att_has_href(a: &Value) -> bool {
    att_web(a) || s(a, "url").starts_with('~') || s(a, "url").starts_with('/')
}

#[derive(Debug, PartialEq)]
pub struct AttItem {
    pub id: i64,
    pub name: String,
    pub linked: bool,
    pub meta: String,
    pub tip: String,
    /// `att-src`: the task (T4) or other goal (G2) it came from.
    pub src: Option<String>,
}

/// `attachList(list, 'goals', gr)`.
pub fn attachments_view(list: &[Value], gr: &str) -> Vec<AttItem> {
    list.iter()
        .map(|a| {
            let kind = att_kind(a);
            let label = ATT_KINDS.iter().find(|(id, _)| *id == kind).map(|(_, l)| *l).unwrap_or("Link");
            let src = fmt::opt_s(a, "task").or(fmt::opt_s(a, "goal")).filter(|x| *x != gr).map(str::to_string);
            let meta = [Some(label.to_string()), src.clone(), fmt::opt_s(a, "at").map(fmt::ago)].into_iter().flatten().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
            let linked = att_has_href(a);
            let tip = if linked { if att_web(a) { s(a, "title").to_string() } else { format!("{}\n{}", s(a, "title"), s(a, "url")) } } else { s(a, "url").to_string() };
            AttItem { id: i(a, "id"), name: s(a, "title").to_string(), linked, meta, tip, src }
        })
        .collect()
}

/// `attMenuHtml`'s items.
pub fn att_menu_items(a: &Value) -> Vec<&'static str> {
    let mut v = Vec::new();
    if att_has_href(a) {
        v.push("Open");
    }
    v.push(if att_web(a) { "Copy link" } else { "Copy path" });
    v.extend(["Edit", "Remove"]);
    v
}

// ------------------------------------------------------------------ backlog

#[derive(Debug, PartialEq)]
pub struct BacklogRow {
    pub r: String,
    pub checkbox: bool,
    pub kind: String,
    pub title: String,
    pub from: String,
    pub actions: Vec<&'static str>,
    pub state: Option<String>,
}

#[derive(Debug, PartialEq)]
pub struct BulkBar {
    pub label: String,
    pub buttons: Vec<&'static str>,
}

pub struct BacklogView {
    pub rows: Vec<BacklogRow>,
    pub pickable: Vec<String>,
    pub bar: Option<BulkBar>,
    /// The line under the list, and its Show / Hide button.
    pub closed: Option<(String, Option<&'static str>)>,
}

/// `goalBacklogRows` (`withKept` + the won't-do filter).
fn backlog_rows(g: &Value, kept: &HashMap<String, Value>, order: &[String], show_dropped: bool) -> Vec<Value> {
    let mut rows: Vec<Value> = arr(g, "backlog").to_vec();
    let mut have: HashSet<String> = rows.iter().map(|x| fmt::ref_of(x, "B")).collect();
    for (ix, r) in order.iter().enumerate() {
        if !have.contains(r) {
            if let Some(row) = kept.get(r) {
                rows.insert(ix.min(rows.len()), row.clone());
                have.insert(r.clone());
            }
        }
    }
    rows.into_iter().filter(|x| show_dropped || s(x, "state") != "drop" || kept.contains_key(&fmt::ref_of(x, "B"))).collect()
}

/// `goalIssueState(b, g)`.
fn goal_issue_state(bl: &Value, g: &Value) -> Option<String> {
    if s(bl, "state") == "task" {
        let tasks = arr(g, "tasks");
        if let Some(ix) = tasks.iter().position(|t| !bl["task_id"].is_null() && t["id"] == bl["task_id"]) {
            return Some(format!("Now task {} · {}", ix + 1, status_label(status_key(&tasks[ix])).to_lowercase()));
        }
    }
    issue_state_text(bl)
}

/// `goalBacklog(g)` + `bulkBar`.
pub fn backlog_view(g: &Value, sel: &[String], show_dropped: bool, kept: &HashMap<String, Value>, order: &[String], jira: bool, bulk_busy: bool) -> BacklogView {
    let rows = backlog_rows(g, kept, order, show_dropped);
    let pickable: Vec<String> = rows.iter().filter(|x| fmt::opt_s(x, "state").unwrap_or("open") == "open").map(|x| fmt::ref_of(x, "B")).collect();
    let n = sel.iter().filter(|r| pickable.contains(r)).count();
    let bar = (!pickable.is_empty() && !rows.is_empty()).then(|| {
        if n == 0 {
            BulkBar { label: "Select all".into(), buttons: vec![] }
        } else {
            let _ = bulk_busy;
            let mut buttons = vec!["Make tasks"];
            if jira {
                buttons.push("Create tickets");
            }
            buttons.extend(["Won’t do", "Move to a goal", "Clear"]);
            BulkBar { label: format!("{n} selected"), buttons }
        }
    });
    let view_rows = rows
        .iter()
        .map(|bl| {
            let open = fmt::opt_s(bl, "state").unwrap_or("open") == "open";
            let mut actions = Vec::new();
            if open {
                actions.push("Make it a task");
                if jira {
                    actions.push("Create ticket");
                }
                actions.push("Won’t do");
            }
            BacklogRow {
                r: fmt::ref_of(bl, "B"),
                checkbox: open,
                kind: kind_label(s(bl, "kind")),
                title: s(bl, "title").to_string(),
                from: issue_from(bl, true),
                actions,
                state: if open { None } else { issue_state_text(bl).and(goal_issue_state(bl, g)) },
            }
        })
        .collect();
    let closed_n = match g.get("closed_count") {
        Some(Value::Number(x)) => x.as_i64().unwrap_or(0),
        Some(v) if !v.is_null() => i(g, "closed_count"),
        _ => arr(g, "backlog").iter().filter(|x| s(x, "state") == "drop").count() as i64,
    };
    let closed = if rows.is_empty() && closed_n == 0 {
        Some(("Nothing in the backlog yet. Issues that terminals find will collect here.".to_string(), None))
    } else if closed_n > 0 {
        Some((format!("{closed_n} closed as won’t do · "), Some(if show_dropped { "Hide" } else { "Show" })))
    } else {
        None
    };
    BacklogView { rows: view_rows, pickable, bar, closed }
}

/// `BULK_DONE[action](n)`.
pub fn bulk_toast(action: &str, n: i64) -> String {
    match action {
        "task" => format!("Made {}", fmt::plural(n, "task", "tasks")),
        "ticket" => format!("Asked Jira for {}", fmt::plural(n, "ticket", "tickets")),
        "drop" => format!("Closed {} as won’t do", fmt::plural(n, "issue", "issues")),
        _ => format!("Moved {}", fmt::plural(n, "issue", "issues")),
    }
}

/// The goal picker's items (`pickerItems` for kind `goal`, special "Not in a goal"): (value, name, sub).
pub fn goal_picker_items(goals: &[Value], q: &str) -> Vec<(String, String, String)> {
    let q = q.trim().to_lowercase();
    let mut items: Vec<(String, String, String)> = goals
        .iter()
        .filter(|g| !b(g, "archived"))
        .map(|g| {
            let sub = [fmt::opt_s(g, "project"), fmt::opt_s(g, "epic_key")].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            (fmt::ref_of(g, "G"), s(g, "name").to_string(), sub)
        })
        .collect();
    items.sort_by(|a, c| locale_cmp(&a.1, &c.1));
    if q.is_empty() {
        let mut out = vec![("none".to_string(), "Not in a goal".to_string(), String::new())];
        out.extend(items);
        return out;
    }
    let rank = |name: &str| {
        let l = name.to_lowercase();
        if l.starts_with(&q) {
            0
        } else if l.split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit())).any(|w| w.starts_with(&q)) {
            1
        } else if l.contains(&q) {
            2
        } else {
            3
        }
    };
    let mut items: Vec<_> = items.into_iter().filter(|(_, name, sub)| name.to_lowercase().contains(&q) || sub.to_lowercase().contains(&q)).collect();
    items.sort_by(|a, c| rank(&a.1).cmp(&rank(&c.1)).then_with(|| locale_cmp(&a.1, &c.1)));
    items
}

// ------------------------------------------------------------------ actions (the web's run())

/// What a request shows when it's done: a note in its group, else a toast.
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

/// `run(el, () => post(path, body), {grp, toast, ok})`: once at a time per busy key.
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

/// `goal-run` / `goal-run-now`.
pub fn run_goal(m: &mut MainWindow, gr: &str, now: bool, cx: &mut Context<MainWindow>) {
    m.menu = None;
    if now {
        run(m, format!("goal-run-now::{gr}"), None, true, format!("goals/{gr}/run"), json!({"now": true}), cx, |_, v, _| run_now_toast(v));
    } else {
        run(m, format!("goal-run::{gr}"), None, true, format!("goals/{gr}/run"), json!({}), cx, |m, v, _| run_toast(v, &hours(m)));
    }
}

/// `goal-plan-edit`.
pub fn plan_goal(m: &mut MainWindow, gr: &str, cx: &mut Context<MainWindow>) {
    run(m, format!("goal-plan-edit::{gr}"), None, true, format!("goals/{gr}/plan"), json!({"mode": "edit"}), cx, |_, _, _| {
        "Opening a Claude terminal in Midna. It starts in about half a minute.".into()
    });
}

/// `goal-pause` (`on`) from the run buttons, or Resume (`off`) from the gate banner.
pub fn set_paused(m: &mut MainWindow, gr: &str, on: bool, cx: &mut Context<MainWindow>) {
    let grp = (!on).then(|| format!("ggate:{gr}"));
    let arg = if on { "on" } else { "off" };
    run(m, format!("goal-pause:{arg}:{gr}"), grp, true, format!("goals/{gr}"), json!({"paused": on}), cx, move |_, _, _| {
        if on { "Paused. Running tasks carry on.".into() } else { "Going again".into() }
    });
}

/// `goal-deprio-yes` (the dialog) or "Bring it back" (`goal-deprio` off, the gate banner).
pub fn set_deprioritized(m: &mut MainWindow, gr: &str, on: bool, cx: &mut Context<MainWindow>) {
    if on {
        run(m, format!("goal-deprio-yes::{gr}"), None, true, format!("goals/{gr}"), json!({"deprioritized": true}), cx, |m, _, cx| {
            close_dialog(m, cx);
            "Deprioritized. Running tasks carry on.".into()
        });
    } else {
        run(m, format!("goal-deprio:off:{gr}"), Some(format!("ggate:{gr}")), true, format!("goals/{gr}"), json!({"deprioritized": false}), cx, |_, _, _| {
            "Back on the board".into()
        });
    }
}

/// `goal-set`: "How this goal runs" (saved at once; shown at once).
pub fn set_run_option(m: &mut MainWindow, gr: &str, field: &'static str, v: Value, cx: &mut Context<MainWindow>) {
    if m.goal_page.busy.contains(&format!("goal-set::{gr}")) {
        return;
    }
    if let Some(goal) = m.data.goal.as_mut().filter(|g| fmt::ref_of(g, "G") == gr) {
        goal[field] = v.clone();
    }
    run(m, format!("goal-set::{gr}"), Some(format!("gset:{gr}")), false, format!("goals/{gr}"), json!({ field: v }), cx, |_, _, _| "Saved".into());
}

/// `gnote-save`.
pub fn save_note(m: &mut MainWindow, gr: &str, cx: &mut Context<MainWindow>) {
    let k = format!("gnote:{gr}");
    let text = m.goal_page.note_inputs.get(gr).map(|n| n.text(cx).trim().to_string()).unwrap_or_default();
    if text.is_empty() {
        m.goal_page.notes.insert(k, ("Write the note first.".into(), true));
        cx.notify();
        return;
    }
    let kind = m.goal_page.note_kinds.get(gr).copied().unwrap_or("finding");
    let g = gr.to_string();
    run(m, format!("gnote-save::{gr}"), Some(k), false, format!("goals/{gr}/notes"), json!({"kind": kind, "text": text, "source": "you"}), cx, move |m, _, cx| {
        if let Some(n) = m.goal_page.note_inputs.get(&g) {
            n.clear(cx);
        }
        m.goal_page.note_open.remove(&g);
        "Note added".into()
    });
}

/// `att-remove`.
pub fn remove_attachment(m: &mut MainWindow, id: i64, gr: &str, cx: &mut Context<MainWindow>) {
    m.menu = None;
    run(m, format!("att-remove::{id}"), Some(format!("att:goals:{gr}")), false, format!("attachments/{id}/remove"), json!({}), cx, |_, _, _| "Removed".into());
}

/// `att-esave`: only the fields touched since Edit.
pub fn save_attachment(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let Some(e) = m.goal_page.att_edit.as_ref() else { return };
    let id = e.id;
    let mut body = serde_json::Map::new();
    if e.touched.contains("title") {
        body.insert("title".into(), json!(e.title.text(cx)));
    }
    if e.touched.contains("url") {
        body.insert("url".into(), json!(e.url.text(cx)));
    }
    if let Some(k) = &e.kind {
        body.insert("kind".into(), json!(k));
    }
    run(m, format!("att-esave::{id}"), Some(format!("attedit:{id}")), true, format!("attachments/{id}"), Value::Object(body), cx, |m, _, _| {
        m.goal_page.att_edit = None;
        "Saved".into()
    });
}

/// `keepIssue`: hold the row where it is, then refresh it from the board (`refreshKept`).
fn keep_issue(m: &mut MainWindow, r: &str) {
    if let Some(row) = m.data.goal.as_ref().and_then(|g| arr(g, "backlog").iter().find(|x| fmt::ref_of(x, "B") == r).cloned()) {
        m.goal_page.kept.insert(r.to_string(), row);
    }
}

fn refresh_kept(m: &mut MainWindow, refs: Vec<String>, cx: &mut Context<MainWindow>) {
    let backend = m.backend.clone();
    cx.spawn(async move |this, cx| {
        let got = cx
            .background_executor()
            .spawn(async move { refs.into_iter().filter_map(|r| backend.get(&format!("backlog/{r}"), &[]).ok().map(|v| (r, v))).collect::<Vec<_>>() })
            .await;
        let _ = this.update(cx, |m, cx| {
            for (r, v) in got {
                if m.goal_page.kept.contains_key(&r) {
                    m.goal_page.kept.insert(r, if v["issue"].is_object() { v["issue"].clone() } else { v });
                }
            }
            cx.notify();
        });
    })
    .detach();
}

/// The row actions: `promote` (where goal), `ticket`, `drop`.
pub fn issue_action(m: &mut MainWindow, act: &'static str, r: &str, cx: &mut Context<MainWindow>) {
    keep_issue(m, r);
    let (busy, path, body) = match act {
        "promote" => (format!("promote:goal:{r}"), format!("backlog/{r}/promote"), json!({"where": "goal"})),
        "ticket" => (format!("ticket::{r}"), format!("backlog/{r}/ticket"), json!({})),
        _ => (format!("drop::{r}"), format!("backlog/{r}/drop"), json!({})),
    };
    let rr = r.to_string();
    run(m, busy, Some(format!("issue:{r}")), false, path, body, cx, move |m, _, cx| {
        refresh_kept(m, vec![rr], cx);
        match act {
            "promote" => String::new(),
            "ticket" => "Asked Jira for a ticket".into(),
            _ => "Closed as won’t do".into(),
        }
    });
}

/// `bulkChange(action, extra)`.
pub fn bulk(m: &mut MainWindow, action: &'static str, extra: Value, cx: &mut Context<MainWindow>) {
    let ids = m.goal_page.sel.clone();
    if ids.is_empty() {
        return;
    }
    m.menu = None;
    m.goal_page.picker = None;
    for r in &ids {
        keep_issue(m, r);
    }
    let mut body = json!({"ids": ids, "action": action});
    if action == "task" {
        body["where"] = json!("goal");
    }
    if let (Value::Object(o), Value::Object(e)) = (&mut body, extra) {
        o.extend(e);
    }
    let n_ids = ids.len() as i64;
    run(m, format!("bl-bulk:{action}:"), Some("bulk".into()), false, "backlog/bulk".into(), body, cx, move |m, v, cx| {
        m.goal_page.sel.clear();
        m.goal_page.anchor = None;
        let n = match v.get("count").and_then(Value::as_i64) {
            Some(c) if c != 0 => c,
            _ => n_ids,
        };
        m.toast(bulk_toast(action, n), false, cx);
        refresh_kept(m, ids, cx);
        String::new()
    });
}

/// `bl-bulk-move`: a goal ref or "none".
pub fn move_selected(m: &mut MainWindow, v: &str, cx: &mut Context<MainWindow>) {
    let goal_id = if v == "none" { Value::Null } else { v.trim_start_matches(|c: char| c.is_ascii_alphabetic()).parse::<i64>().map(Value::from).unwrap_or(Value::Null) };
    bulk(m, "move", json!({"goal_id": goal_id}), cx);
}

/// `bl-pick` (shift extends from the last one picked) and `bl-pick-all`.
fn pick(m: &mut MainWindow, r: &str, on: bool, shift: bool, pickable: &[String]) {
    let st = &mut m.goal_page;
    let span: Vec<String> = match (shift, st.anchor.as_ref().and_then(|a| pickable.iter().position(|x| x == a)), pickable.iter().position(|x| x == r)) {
        (true, Some(a), Some(j)) => pickable[a.min(j)..=a.max(j)].to_vec(),
        _ => vec![r.to_string()],
    };
    for x in span {
        if on {
            if !st.sel.contains(&x) {
                st.sel.push(x);
            }
        } else {
            st.sel.retain(|y| *y != x);
        }
    }
    st.anchor = Some(r.to_string());
}

fn close_dialog(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    m.goal_page.deprio_open = false;
    m.goal_page.note_dialog = None;
    m.goal_page.dialog_focus = None;
    cx.notify();
}

fn open_dialog(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    let f = cx.focus_handle();
    window.focus(&f, cx);
    m.goal_page.dialog_focus = Some(f);
    cx.notify();
}

// ------------------------------------------------------------------ render pieces

fn busy(m: &MainWindow, key: &str) -> bool {
    m.goal_page.busy.contains(key)
}

/// A button that shows its busy label and can't be pressed while its request runs.
fn act_btn(b: Stateful<Div>, is_busy: bool) -> Stateful<Div> {
    if is_busy { kit::disabled(b) } else { b }
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

fn checkbox(t: &Theme, on: bool, mixed: bool) -> Div {
    let fill = on || mixed;
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(16.))
        .rounded(px(4.))
        .border_1()
        .border_color(if fill { t.accent_btn } else { t.border_2 })
        .bg(if fill { t.accent_btn } else { t.card })
        .text_color(t.on_accent)
        .text_size(px(11.))
        .font_weight(FontWeight::BOLD)
        .child(if on { "✓" } else if mixed { "–" } else { "" })
}

fn section(t: &Theme) -> Div {
    kit::card(t).p(px(16.)).gap(px(10.))
}

/// The `.st-*` chip.
fn st_chip(t: &Theme, tone: &str, label: impl Into<SharedString>) -> Div {
    match tone {
        "planned" => kit::pill(t.muted, t.card, label).border_1().border_color(t.border),
        "queued" => kit::pill(t.text_2, t.col, label),
        other => kit::tone_pill(t, other, label),
    }
}

fn open_attachment(url: &str, cx: &mut App) {
    if url.starts_with("http://") || url.starts_with("https://") {
        cx.open_url(url);
    } else {
        let path = taskboardd::util::expand_home(url);
        let _ = std::process::Command::new("/usr/bin/open").arg(path).spawn();
    }
}

fn header(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Div {
    let h = header_view(g, m.jira_on());
    let c = counts(g);
    let pct = |x: i64| relative(if c.n == 0 { 0. } else { x as f32 / c.n as f32 });
    let bar = div()
        .flex()
        .w_full()
        .h(px(6.))
        .rounded_full()
        .bg(t.seg)
        .overflow_hidden()
        .child(div().h_full().bg(t.up).w(pct(c.done)))
        .child(div().h_full().bg(t.accent).w(pct(c.active)))
        .child(div().h_full().bg(t.faint).opacity(0.6).w(pct(c.queued)));
    let epic: Option<AnyElement> = h.epic.clone().map(|text| match (&h.epic_link, fmt::opt_s(g, "epic_key")) {
        (Some(url), Some(key)) => {
            let url = url.clone();
            let prefix = "Jira epic ".to_string();
            let rest = text.trim_start_matches("Jira epic ").trim_start_matches(key).to_string();
            div()
                .flex()
                .child(prefix)
                .child(kit::link(t, "goal-epic", key.to_string()).on_click(move |_, _, cx| if url != "#" { cx.open_url(&url) }))
                .child(rest)
                .into_any_element()
        }
        _ => div().child(text).into_any_element(),
    });
    let repo = fmt::opt_s(g, "repo_path").map(str::to_string);
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(10.))
                .child(kit::tone_pill(t, "goal", "⚑ Goal"))
                .child(st_chip(t, h.state_tone, h.pills[1].clone()))
                .child(kit::chip(t, h.pills[2].clone()))
                .child(div().flex_1())
                .child(run_buttons(m, t, g, cx)),
        )
        .child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).line_height(px(30.)).child(h.name.clone()))
        .when_some(h.tldr.clone(), |d, tl| {
            d.child(
                div()
                    .flex()
                    .gap(px(6.))
                    .text_size(px(14.))
                    .text_color(t.text_2)
                    .child(div().flex_none().font_weight(FontWeight::BOLD).child("TLDR"))
                    .child(div().flex_1().min_w_0().child(tl)),
            )
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(12.))
                .text_size(px(12.5))
                .text_color(t.muted)
                .child(
                    kit::chip(t, h.project.clone())
                        .font_family(t.mono_font.clone())
                        .id("goal-project")
                        .when_some(repo, |c, p| c.tooltip(kit::tip(p))),
                )
                .children(epic)
                .child(h.counts.clone()),
        )
        .child(bar)
}

fn run_buttons(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Div {
    let gr = fmt::ref_of(g, "G");
    let (split, buttons) = run_buttons_view(g, &hours(m));
    let mut row = div().flex().flex_wrap().items_center().gap(px(8.));
    for bt in buttons {
        let key = match bt.act {
            "goal-pause" => format!("goal-pause:on:{gr}"),
            a => format!("{a}::{gr}"),
        };
        let is_busy = busy(m, &key);
        let label = if is_busy { bt.busy_label.to_string() } else { bt.label.clone() };
        let gr2 = gr.clone();
        let el = match bt.act {
            "goal-run" => {
                let main = act_btn(kit::btn_primary(t, "goal-run", format!("▶  {label}")).tooltip(kit::tip(bt.title.clone())), is_busy);
                let main = if is_busy { main } else { main.on_click(cx.listener(move |m, _, _, cx| run_goal(m, &gr2, false, cx))) };
                if split {
                    div()
                        .flex()
                        .child(main.rounded_r(px(0.)))
                        .child(
                            kit::btn_primary(t, "goal-run-caret", "▾")
                                .rounded_l(px(0.))
                                .border_l_1()
                                .border_color(t.accent_fg)
                                .px(px(8.))
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
                let b = act_btn(kit::btn(t, "goal-pause", format!("❚❚  {label}")).tooltip(kit::tip(bt.title.clone())), is_busy);
                if is_busy { b } else { b.on_click(cx.listener(move |m, _, _, cx| set_paused(m, &gr2, true, cx))) }.into_any_element()
            }
            "goal-plan-edit" => {
                let b = act_btn(kit::btn(t, "goal-plan", label).tooltip(kit::tip(bt.title.clone())), is_busy);
                if is_busy { b } else { b.on_click(cx.listener(move |m, _, _, cx| plan_goal(m, &gr2, cx))) }.into_any_element()
            }
            _ => kit::btn_danger(t, "goal-deprio", label)
                .tooltip(kit::tip(bt.title.clone()))
                .on_click(cx.listener(|m, _, window, cx| {
                    m.goal_page.deprio_open = true;
                    open_dialog(m, window, cx);
                }))
                .into_any_element(),
        };
        row = row.child(el);
    }
    row
}

fn start_menu(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let at = m.menu_open(START_MENU)?;
    let gr = fmt::ref_of(g, "G");
    let mut menu = kit::menu_box(t, 290.).id("goal-start-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for (act, title, sub) in start_menu_view(&hours(m)) {
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
                .on_click(cx.listener(move |m, _, _, cx| run_goal(m, &gr, act == "goal-run-now", cx))),
        );
    }
    Some(kit::popover(at, menu))
}

fn gate(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Option<Div> {
    let gr = fmt::ref_of(g, "G");
    let note = inline_note(m, t, &format!("ggate:{gr}"));
    let Some((label, text, (act, button))) = gate_view(g) else {
        return note.map(|n| div().child(n));
    };
    let key = if act == "goal-deprio" { format!("goal-deprio:off:{gr}") } else { format!("goal-pause:off:{gr}") };
    let is_busy = busy(m, &key);
    let bt = act_btn(kit::btn_primary(t, "goal-gate", if is_busy { "Sending…" } else { button }), is_busy);
    let bt = if is_busy {
        bt
    } else {
        bt.on_click(cx.listener(move |m, _, _, cx| if act == "goal-deprio" { set_deprioritized(m, &gr, false, cx) } else { set_paused(m, &gr, false, cx) }))
    };
    Some(
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(14.))
            .rounded(px(10.))
            .border_1()
            .border_color(t.warn_line)
            .bg(t.warn_soft)
            .text_color(t.warn_text)
            .child(div().text_size(px(11.5)).font_weight(FontWeight::BOLD).child(label.to_uppercase()))
            .child(div().text_size(px(13.5)).child(text))
            .child(div().flex().child(bt))
            .children(note),
    )
}

fn task_rows(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Div {
    let tasks = arr(g, "tasks");
    if tasks.is_empty() {
        return kit::card(t).border_dashed().child(kit::empty(t, "No tasks in this goal yet. Add one, or let Claude plan them."));
    }
    let open_task = match &m.panel {
        Some(Panel::Task { r, .. }) => Some(r.clone()),
        _ => None,
    };
    let mut list = kit::card(t).overflow_hidden();
    for (ix, task) in tasks.iter().enumerate() {
        let row = task_row_view(task, ix, tasks, g);
        let hover = t.tint;
        let target = row.r.clone();
        let on = open_task.as_deref() == Some(row.r.as_str());
        let pr_color = match row.pr_phase.as_str() {
            "merged" => t.up,
            "declined" => t.down,
            "fix" | "comments" => t.warn,
            _ => t.accent,
        };
        list = list.child(
            div()
                .id(SharedString::from(format!("goal-task-{}", row.r)))
                .flex()
                .items_start()
                .gap(px(12.))
                .px(px(14.))
                .py(px(10.))
                .cursor_pointer()
                .when(ix > 0, |d| d.border_t_1().border_color(t.divider))
                .when(on, |d| d.bg(t.accent_soft))
                .hover(move |d| d.bg(hover))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(22.))
                        .mt(px(1.))
                        .rounded_full()
                        .border_1()
                        .border_color(if row.key == "done" { t.up } else { t.border_2 })
                        .bg(if row.key == "done" { t.up_soft } else { t.card })
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if row.key == "done" { t.up_fg } else { t.muted })
                        .child(row.n.to_string()),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(3.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .min_w_0()
                                .child(st_chip(t, status_tone(row.key), row.chip))
                                .children(row.jira_tip.clone().map(|tip| {
                                    div()
                                        .id(SharedString::from(format!("goal-jira-{}", row.r)))
                                        .flex_none()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(t.jira)
                                        .child("◆")
                                        .tooltip(kit::tip(tip))
                                }))
                                .children(row.pr_text.clone().map(|txt| {
                                    div()
                                        .id(SharedString::from(format!("goal-pr-{}", row.r)))
                                        .flex_none()
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(pr_color)
                                        .child(txt)
                                        .tooltip(kit::tip(row.pr_tip.clone().unwrap_or_default()))
                                }))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(if row.planned { t.text_2 } else { t.text })
                                        .child(row.title.clone()),
                                ),
                        )
                        .when(!row.meta.is_empty(), |d| d.child(div().text_size(px(12.5)).text_color(t.muted).truncate().child(row.meta.clone()))),
                )
                .on_click(cx.listener(move |m, _, _, cx| m.open_task(target.clone(), cx))),
        );
    }
    list
}

fn how_runs(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Div {
    let gr = fmt::ref_of(g, "G");
    let v = how_runs_view(g);
    let shown = v.selected.clone().unwrap_or_else(|| v.options[0].1.clone());
    let menu_key = format!("goal-max:{gr}");
    let toggle = |id: &'static str, field: &'static str, on: bool, label: &'static str| {
        let gr = gr.clone();
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .text_size(px(14.))
            .child(checkbox(t, on, false))
            .child(label)
            .on_click(cx.listener(move |m, _, _, cx| set_run_option(m, &gr, field, json!(!on), cx)))
    };
    let mk = menu_key.clone();
    section(t)
        .child(kit::h3(t, "How this goal runs"))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(10.))
                .text_size(px(14.))
                .child("At most")
                .child(kit::btn_small(t, "goal-max", format!("{shown}  ▾")).on_click(cx.listener(move |m, e: &ClickEvent, _, cx| {
                    let p = e.position();
                    m.toggle_menu(&mk, point(p.x - px(20.), p.y + px(14.)), cx);
                })))
                .child(div().text_size(px(13.)).text_color(t.muted).child("working on this goal at once")),
        )
        .child(toggle("goal-in-order", "run_in_order", v.run_in_order, "Run the tasks in order, one after another"))
        .child(toggle("goal-auto-close", "auto_close", v.auto_close, "Close each terminal when its task is done"))
        .children(inline_note(m, t, &format!("gset:{gr}")))
        .children(m.menu_open(&menu_key).map(|at| {
            let mut menu = kit::menu_box(t, 170.).id("goal-max-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
            for (n, label) in v.options.iter().cloned() {
                let gr = gr.clone();
                menu = menu.child(kit::menu_item(t, SharedString::from(format!("goal-max-{n}")), label.clone(), Some(&label) == v.selected.as_ref()).on_click(cx.listener(
                    move |m, _, _, cx| {
                        m.menu = None;
                        set_run_option(m, &gr, "max_terminals", json!(n), cx);
                    },
                )));
            }
            kit::popover(at, menu)
        }))
}

fn attachments(m: &MainWindow, t: &Theme, g: &Value, window: &mut Window, cx: &mut Context<MainWindow>) -> Div {
    let gr = fmt::ref_of(g, "G");
    let list = arr(g, "attachments");
    let mut card = section(t).child(kit::h3(t, "Attached")).children(inline_note(m, t, &format!("att:goals:{gr}")));
    if list.is_empty() {
        return card.child(kit::help(t, ATTACH_HELP));
    }
    for (a, item) in list.iter().zip(attachments_view(list, &gr)) {
        let id = item.id;
        if let Some(e) = m.goal_page.att_edit.as_ref().filter(|e| e.id == id) {
            card = card.child(att_edit_form(m, t, a, e, window, cx));
            continue;
        }
        let url = s(a, "url").to_string();
        let name: AnyElement = if item.linked {
            let u = url.clone();
            kit::link(t, SharedString::from(format!("att-open-{id}")), item.name.clone())
                .text_size(px(13.5))
                .truncate()
                .tooltip(kit::tip(item.tip.clone()))
                .on_click(move |_, _, cx| open_attachment(&u, cx))
                .into_any_element()
        } else {
            div()
                .id(SharedString::from(format!("att-name-{id}")))
                .text_size(px(13.5))
                .font_family(t.mono_font.clone())
                .truncate()
                .child(item.name.clone())
                .tooltip(kit::tip(item.tip.clone()))
                .into_any_element()
        };
        let src = item.src.clone();
        let meta = div().flex().gap(px(4.)).text_size(px(12.)).text_color(t.muted).child(item.meta.clone()).when_some(src, |d, src| {
            let target = src.clone();
            d.child(kit::link(t, SharedString::from(format!("att-src-{id}")), format!("Open {src}")).text_size(px(12.)).on_click(cx.listener(move |m, _, _, cx| {
                if target.starts_with('T') {
                    m.open_task(target.clone(), cx);
                } else {
                    m.go(Page::Goal(target.clone()), cx);
                }
            })))
        });
        let menu_key = format!("{ATT_MENU}:{id}");
        let mk = menu_key.clone();
        card = card.child(
            div()
                .flex()
                .items_start()
                .gap(px(8.))
                .child(div().flex().flex_col().flex_1().min_w_0().child(name).child(meta))
                .child(kit::btn_small(t, SharedString::from(format!("att-menu-{id}")), "•••").tooltip(kit::tip(format!("Actions for {}", item.name))).on_click(
                    cx.listener(move |m, e: &ClickEvent, _, cx| {
                        let p = e.position();
                        m.toggle_menu(&mk, point(p.x - px(150.), p.y + px(14.)), cx);
                    }),
                ))
                .children(m.menu_open(&menu_key).map(|at| att_menu(t, a, &gr, at, cx))),
        );
    }
    card
}

fn att_menu(t: &Theme, a: &Value, gr: &str, at: Point<Pixels>, cx: &mut Context<MainWindow>) -> AnyElement {
    let id = i(a, "id");
    let url = s(a, "url").to_string();
    let mut menu = kit::menu_box(t, 170.).id("att-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for label in att_menu_items(a) {
        let (url, gr, a2) = (url.clone(), gr.to_string(), a.clone());
        let item = kit::menu_item(t, SharedString::from(format!("att-{id}-{label}")), label, false);
        let item = match label {
            "Open" => item.on_click(cx.listener(move |m, _, _, cx| {
                m.menu = None;
                open_attachment(&url, cx);
            })),
            "Edit" => item.on_click(cx.listener(move |m, _, _, cx| start_att_edit(m, &a2, cx))),
            "Remove" => item.text_color(t.down).on_click(cx.listener(move |m, _, _, cx| remove_attachment(m, id, &gr, cx))),
            _ => item.on_click(cx.listener(move |m, _, _, cx| {
                m.menu = None;
                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                m.toast("Copied", false, cx);
            })),
        };
        menu = menu.when(label == "Remove", |d| d.child(kit::divider(t).my(px(4.)))).child(item);
    }
    kit::popover(at, menu)
}

fn start_att_edit(m: &mut MainWindow, a: &Value, cx: &mut Context<MainWindow>) {
    m.menu = None;
    let title = kit::Input::with_text(cx, "Title", false, s(a, "title"));
    let url = kit::Input::with_text(cx, "https://… or /path/to/file", false, s(a, "url"));
    let subs = vec![
        cx.subscribe(&title.field, |m, _, _: &FieldChanged, _| {
            if let Some(e) = m.goal_page.att_edit.as_mut() {
                e.touched.insert("title");
            }
        }),
        cx.subscribe(&url.field, |m, _, _: &FieldChanged, _| {
            if let Some(e) = m.goal_page.att_edit.as_mut() {
                e.touched.insert("url");
            }
        }),
    ];
    let id = i(a, "id");
    m.goal_page.notes.remove(&format!("attedit:{id}"));
    m.goal_page.att_edit = Some(AttEdit { id, kind: None, title, url, touched: HashSet::new(), _subs: subs });
    cx.notify();
}

fn att_edit_form(m: &MainWindow, t: &Theme, a: &Value, e: &AttEdit, window: &mut Window, cx: &mut Context<MainWindow>) -> Div {
    let id = e.id;
    let cur = e.kind.clone().unwrap_or_else(|| fmt::opt_s(a, "kind").unwrap_or("other").to_string());
    let labels: Vec<&str> = ATT_KINDS.iter().map(|(_, l)| *l).collect();
    let on = ATT_KINDS.iter().position(|(k, _)| *k == cur).unwrap_or(usize::MAX);
    let is_busy = busy(m, &format!("att-esave::{id}"));
    let save = act_btn(kit::btn_small(t, "att-esave", if is_busy { "Sending…" } else { "Save" }), is_busy);
    let save = if is_busy { save } else { save.on_click(cx.listener(|m, _, _, cx| save_attachment(m, cx))) };
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .w_full()
        .child(kit::seg(t, "att-ekind", &labels, on, |ix, item| {
            item.on_click(cx.listener(move |m, _, _, cx| {
                if let Some(e) = m.goal_page.att_edit.as_mut() {
                    e.kind = Some(ATT_KINDS[ix].0.to_string());
                }
                cx.notify();
            }))
        }))
        .child(e.title.render(t, "att-etitle", window))
        .child(e.url.render(t, "att-eurl", window).font_family(t.mono_font.clone()))
        .child(
            div().flex().gap(px(8.)).child(save).child(kit::btn_small(t, "att-ecancel", "Cancel").on_click(cx.listener(|m, _, _, cx| {
                m.goal_page.att_edit = None;
                cx.notify();
            }))),
        )
        .children(inline_note(m, t, &format!("attedit:{id}")))
}

fn notes(m: &mut MainWindow, t: &Theme, g: &Value, window: &mut Window, cx: &mut Context<MainWindow>) -> Div {
    let gr = fmt::ref_of(g, "G");
    let fk = format!("gnote:{gr}");
    let open_form = m.goal_page.note_open.contains(&gr);
    let folded_open = crate::prefs::get_str(FOLD_NOTES).map(|v| v == "open").unwrap_or(true);
    let add_gr = gr.clone();
    let head = div()
        .id("gnotes-head")
        .flex()
        .items_center()
        .cursor_pointer()
        .child(div().flex_none().w(px(16.)).text_color(t.muted).child(if folded_open { "▾" } else { "▸" }))
        .child(div().flex_1().child(kit::h3(t, "Goal notes")))
        .when(!open_form, |d| {
            d.child(kit::link(t, "gnote-add", "Add a note").text_size(px(12.5)).on_click(cx.listener(move |m, _, window, cx| {
                let gr = add_gr.clone();
                if !m.goal_page.note_inputs.contains_key(&gr) {
                    let input = kit::Input::new(cx, "What every task in this goal should know", true);
                    m.goal_page.note_inputs.insert(gr.clone(), input);
                }
                m.goal_page.note_open.insert(gr.clone());
                crate::prefs::set(FOLD_NOTES, json!("open"));
                if let Some(n) = m.goal_page.note_inputs.get(&gr) {
                    window.focus(&n.focus, cx);
                }
                cx.stop_propagation();
                cx.notify();
            })))
        })
        .on_click(cx.listener(move |_, _, _, cx| {
            crate::prefs::set(FOLD_NOTES, json!(if folded_open { "closed" } else { "open" }));
            cx.notify();
        }));
    let mut card = section(t).child(head);
    if !folded_open {
        return card;
    }
    card = card.child(div().text_size(px(13.)).text_color(t.muted).child(NOTES_HELP));
    if open_form {
        if let Some(input) = m.goal_page.note_inputs.get(&gr) {
            let kind = m.goal_page.note_kinds.get(&gr).copied().unwrap_or("finding");
            let labels: Vec<&str> = NOTE_KINDS.iter().map(|(_, l)| *l).collect();
            let on = NOTE_KINDS.iter().position(|(k, _)| *k == kind).unwrap_or(0);
            let (key_gr, save_gr, kind_gr, cancel_gr) = (gr.clone(), gr.clone(), gr.clone(), gr.clone());
            let field = input.render(t, "gnote-input", window).min_h(px(68.)).items_start().on_key_down(cx.listener(move |m, ev: &KeyDownEvent, window, cx| {
                let outcome = m.goal_page.note_inputs.get(&key_gr).map(|n| n.on_key(ev, cx));
                if let Some(kit::KeyOutcome::Cancel) = outcome {
                    m.goal_page.note_open.remove(&key_gr);
                    window.focus(&m.focus, cx);
                    cx.notify();
                    cx.stop_propagation();
                }
            }));
            let is_busy = busy(m, &format!("gnote-save::{gr}"));
            let save = act_btn(kit::btn_small(t, "gnote-save", if is_busy { "Sending…" } else { "Save note" }), is_busy);
            let save = if is_busy { save } else { save.on_click(cx.listener(move |m, _, _, cx| save_note(m, &save_gr, cx))) };
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(kit::seg(t, "gnote-kind", &labels, on, |ix, item| {
                        let gr = kind_gr.clone();
                        item.on_click(cx.listener(move |m, _, _, cx| {
                            m.goal_page.note_kinds.insert(gr.clone(), NOTE_KINDS[ix].0);
                            cx.notify();
                        }))
                    }))
                    .child(field)
                    .child(div().flex().items_center().gap(px(8.)).child(save).child(kit::btn_small(t, "gnote-cancel", "Cancel").on_click(cx.listener(
                        move |m, _, _, cx| {
                            m.goal_page.note_open.remove(&cancel_gr);
                            cx.notify();
                        },
                    )))),
            );
        }
    }
    card = card.children(inline_note(m, t, &fk));
    for (title, items) in notes_view(arr(g, "notes")) {
        let mut grp = div().flex().flex_col().gap(px(4.)).child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).child(title));
        let mut ul = div().flex().flex_col().gap(px(3.)).pl(px(16.));
        for item in items {
            let id = i(&item.note, "id");
            // The pin mark, the text and (short notes) the byline flow as one wrapping line.
            let mut runs = String::new();
            let mut hl: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
            if item.pinned {
                runs.push_str("Pinned");
                hl.push((0..runs.len(), HighlightStyle { color: Some(t.goal), font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() }));
                runs.push(' ');
            }
            runs.push_str(&item.text);
            if !item.long && !item.from.is_empty() {
                runs.push(' ');
                let at = runs.len();
                runs.push_str(&item.from);
                hl.push((at..runs.len(), HighlightStyle { color: Some(t.muted), ..Default::default() }));
            }
            let text = div()
                .w_full()
                .min_w_0()
                .text_size(px(13.5))
                .text_color(t.text_2)
                .when(item.long, |d| d.line_clamp(3))
                .child(StyledText::new(runs).with_highlights(hl));
            let from = div().text_size(px(12.)).text_color(t.muted).child(item.from.clone());
            let row = if item.long {
                let n = item.note.clone();
                div().flex().flex_col().child(text).child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(8.))
                        .mt(px(2.))
                        .child(kit::link(t, SharedString::from(format!("gnote-open-{id}")), "Show all").text_size(px(12.5)).on_click(cx.listener(move |m, _, window, cx| {
                            m.goal_page.note_dialog = Some(n.clone());
                            open_dialog(m, window, cx);
                        })))
                        .child(from),
                )
            } else {
                div().flex().flex_col().child(text)
            };
            ul = ul.child(div().flex().gap(px(6.)).child(div().flex_none().text_color(t.muted).child("•")).child(row.flex_1().min_w_0()));
        }
        grp = grp.child(ul);
        card = card.child(grp);
    }
    card
}

fn backlog(m: &mut MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Div {
    let bulk_busy = m.goal_page.busy.iter().any(|k| k.starts_with("bl-bulk:"));
    // Kept rows follow the board's latest copy while they're still in the goal (refreshKept).
    for row in arr(g, "backlog") {
        let r = fmt::ref_of(row, "B");
        if m.goal_page.kept.contains_key(&r) {
            m.goal_page.kept.insert(r, row.clone());
        }
    }
    let st = &m.goal_page;
    let v = backlog_view(g, &st.sel, st.show_dropped, &st.kept, &st.order, m.jira_on(), bulk_busy);
    let order: Vec<String> = v.rows.iter().map(|r| r.r.clone()).collect();
    let pickable = v.pickable.clone();
    m.goal_page.sel.retain(|r| pickable.contains(r));
    m.goal_page.order = order;
    let issue_open = match &m.panel {
        Some(Panel::Issue { r }) => Some(r.clone()),
        _ => None,
    };
    let mut col = div().flex().flex_col().gap(px(10.)).child(div().text_size(px(13.)).text_color(t.muted).child(BACKLOG_HELP));
    if let Some(bar) = &v.bar {
        let n = m.goal_page.sel.len();
        let all = n > 0 && n == pickable.len();
        let picks = pickable.clone();
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(6.))
            .rounded(px(8.))
            .when(n > 0, |d| d.bg(t.accent_soft))
            .child(
                div()
                    .id("bl-pick-all")
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .child(checkbox(t, all, n > 0 && !all))
                    .child(bar.label.clone())
                    .on_click(cx.listener(move |m, _, _, cx| {
                        if m.goal_page.sel.len() == picks.len() {
                            m.goal_page.sel.clear();
                        } else {
                            m.goal_page.sel = picks.clone();
                        }
                        m.goal_page.anchor = None;
                        cx.notify();
                    })),
            );
        for label in &bar.buttons {
            let label = *label;
            let action = match label {
                "Make tasks" => "task",
                "Create tickets" => "ticket",
                "Won’t do" => "drop",
                "Move to a goal" => "move",
                _ => "clear",
            };
            let own_busy = busy(m, &format!("bl-bulk:{action}:"));
            let shown = if own_busy { "Sending…" } else { label };
            let el = match action {
                "clear" => kit::link(t, "bl-pick-clear", label).text_size(px(12.5)).when(!bulk_busy, |d| {
                    d.on_click(cx.listener(|m, _, _, cx| {
                        m.goal_page.sel.clear();
                        m.goal_page.anchor = None;
                        m.goal_page.notes.remove("bulk");
                        cx.notify();
                    }))
                }),
                "move" => act_btn(kit::btn_small(t, "bl-bulk-move", format!("⌕ {label}")), bulk_busy).when(!bulk_busy, |d| {
                    d.on_click(cx.listener(|m, e: &ClickEvent, window, cx| {
                        let p = e.position();
                        open_picker(m, point(p.x - px(20.), p.y + px(14.)), window, cx);
                    }))
                }),
                a => act_btn(kit::btn_small(t, SharedString::from(format!("bl-bulk-{a}")), shown), bulk_busy)
                    .when(!bulk_busy, |d| d.on_click(cx.listener(move |m, _, _, cx| bulk(m, a, json!({}), cx)))),
            };
            if action == "clear" {
                row = row.children(inline_note(m, t, "bulk")).child(div().flex_1());
            }
            row = row.child(el);
        }
        col = col.child(row);
    }
    if !v.rows.is_empty() {
        let mut list = kit::card(t).overflow_hidden();
        for (ix, row) in v.rows.iter().enumerate() {
            let r = row.r.clone();
            let picked = m.goal_page.sel.contains(&r);
            let on = issue_open.as_deref() == Some(r.as_str());
            let hover = t.tint;
            let (pick_r, open_r) = (r.clone(), r.clone());
            let picks = pickable.clone();
            let mut acts = div().flex().flex_none().gap(px(6.));
            for a in &row.actions {
                let act = match *a {
                    "Make it a task" => "promote",
                    "Create ticket" => "ticket",
                    _ => "drop",
                };
                let key = match act {
                    "promote" => format!("promote:goal:{r}"),
                    a => format!("{a}::{r}"),
                };
                let is_busy = busy(m, &key);
                let rr = r.clone();
                let bt = act_btn(kit::btn_small(t, SharedString::from(format!("bl-{act}-{r}")), if is_busy { "Sending…" } else { a }), is_busy);
                acts = acts.child(if is_busy { bt } else { bt.on_click(cx.listener(move |m, _, _, cx| issue_action(m, act, &rr, cx))) });
            }
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .when(ix > 0, |d| d.border_t_1().border_color(t.divider))
                    .child(
                        div()
                            .id(SharedString::from(format!("goal-issue-{r}")))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .px(px(12.))
                            .py(px(9.))
                            .when(picked || on, |d| d.bg(t.accent_soft))
                            .hover(move |d| d.bg(hover))
                            .when(row.checkbox, |d| {
                                d.child(div().id(SharedString::from(format!("bl-pick-{r}"))).cursor_pointer().child(checkbox(t, picked, false)).on_click(cx.listener(
                                    move |m, e: &ClickEvent, _, cx| {
                                        let on = !m.goal_page.sel.contains(&pick_r);
                                        pick(m, &pick_r, on, e.modifiers().shift, &picks);
                                        cx.stop_propagation();
                                        cx.notify();
                                    },
                                )))
                            })
                            .child(
                                div()
                                    .id(SharedString::from(format!("bl-open-{r}")))
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(3.))
                                    .cursor_pointer()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.))
                                            .min_w_0()
                                            .child(kit::chip(t, row.kind.clone()))
                                            .child(div().flex_1().min_w_0().truncate().text_size(px(13.5)).font_weight(FontWeight::BOLD).child(row.title.clone())),
                                    )
                                    .child(div().text_size(px(12.)).text_color(t.muted).truncate().child(row.from.clone()))
                                    .on_click(cx.listener(move |m, _, _, cx| m.open_issue(open_r.clone(), cx))),
                            )
                            .child(match &row.state {
                                Some(state) if row.actions.is_empty() => div().flex_none().text_size(px(12.)).text_color(t.muted).child(state.clone()).into_any_element(),
                                _ => acts.into_any_element(),
                            }),
                    )
                    .children(inline_note(m, t, &format!("issue:{r}")).map(|n| n.px(px(12.)).pb(px(8.)))),
            );
        }
        col = col.child(list);
    }
    if let Some((line, button)) = v.closed {
        col = col.child(div().flex().items_center().gap(px(4.)).text_size(px(12.5)).text_color(t.muted).child(line.trim_end().to_string()).when_some(button, |d, label| {
            d.child(kit::link(t, "show-dropped", label).text_size(px(12.5)).on_click(cx.listener(|m, _, _, cx| {
                m.goal_page.show_dropped = !m.goal_page.show_dropped;
                cx.notify();
            })))
        }));
    }
    col
}

fn open_picker(m: &mut MainWindow, at: Point<Pixels>, window: &mut Window, cx: &mut Context<MainWindow>) {
    let input = kit::Input::new(cx, "Search goals by name, project or epic", false);
    let sub = cx.subscribe(&input.field, |m, _, _: &FieldChanged, cx| {
        if let Some(p) = m.goal_page.picker.as_mut() {
            p.active = 0;
        }
        cx.notify();
    });
    window.focus(&input.focus, cx);
    m.goal_page.picker = Some(Picker { input, active: 0, _sub: sub });
    m.menu = Some((MOVE_MENU.to_string(), at));
    cx.notify();
}

fn picker(m: &MainWindow, t: &Theme, window: &mut Window, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let at = m.menu_open(MOVE_MENU)?;
    let p = m.goal_page.picker.as_ref()?;
    let q = p.input.text(cx);
    let items = goal_picker_items(&m.data.goals, &q);
    let active = p.active.min(items.len().saturating_sub(1));
    let keys = items.iter().map(|x| x.0.clone()).collect::<Vec<_>>();
    let field = p.input.render(t, "goal-picker-q", window).on_key_down(cx.listener(move |m, ev: &KeyDownEvent, _, cx| {
        let n = keys.len();
        match ev.keystroke.key.as_str() {
            "down" | "up" if n > 0 => {
                if let Some(p) = m.goal_page.picker.as_mut() {
                    p.active = if ev.keystroke.key == "down" { (p.active.min(n - 1) + 1) % n } else { (p.active.min(n - 1) + n - 1) % n };
                }
                cx.stop_propagation();
                cx.notify();
            }
            "enter" if n > 0 => {
                let ix = m.goal_page.picker.as_ref().map(|p| p.active.min(n - 1)).unwrap_or(0);
                move_selected(m, &keys[ix], cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }));
    let mut list = div().id("goal-picker-list").flex().flex_col().max_h(px(300.)).overflow_y_scroll();
    if items.is_empty() {
        list = list.child(div().px(px(10.)).py(px(8.)).text_size(px(13.)).text_color(t.muted).child(format!("No goal matches “{}”.", q.trim())));
    }
    for (ix, (v, name, sub)) in items.iter().enumerate() {
        let v = v.clone();
        let hover = t.panel_2;
        list = list.child(
            div()
                .id(SharedString::from(format!("goal-pick-{v}")))
                .flex()
                .flex_col()
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(ix == active, |d| d.bg(t.accent_soft))
                .hover(move |d| d.bg(hover))
                .child(div().text_size(px(13.)).when(v == "none", |d| d.text_color(t.muted)).child(name.clone()))
                .when(!sub.is_empty(), |d| d.child(div().text_size(px(11.5)).text_color(t.faint).child(sub.clone())))
                .on_click(cx.listener(move |m, _, _, cx| move_selected(m, &v, cx))),
        );
    }
    let menu = kit::menu_box(t, 320.)
        .id("goal-picker")
        .p(px(8.))
        .gap(px(6.))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(div().px(px(4.)).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child("Choose a goal"))
        .child(field)
        .child(list);
    Some(kit::popover(at, menu))
}

/// The Deprioritize confirmation and the full-note dialog (the web's `gdeprio` / `gnote` modals).
fn dialog(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let focus = m.goal_page.dialog_focus.clone()?;
    let gr = fmt::ref_of(g, "G");
    let head = |title: AnyElement| {
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(div().flex_1().text_size(px(17.)).font_weight(FontWeight::BOLD).child(title))
            .child(kit::btn_small(t, "goal-dialog-close", "Close").on_click(cx.listener(|m, _, _, cx| close_dialog(m, cx))))
    };
    let body: Div = if m.goal_page.deprio_open && !b(g, "deprioritized") {
        let (title, paras, buttons) = deprio_dialog_view(g);
        let name = fmt::opt_s(g, "name").map(str::to_string).unwrap_or_else(|| gr.clone());
        let rest = paras[0].strip_prefix(&name).unwrap_or(&paras[0]).to_string();
        let is_busy = busy(m, &format!("goal-deprio-yes::{gr}"));
        let yes = act_btn(kit::btn_danger(t, "goal-deprio-yes", if is_busy { "Deprioritizing…" } else { buttons[1] }).bg(t.down).text_color(t.on_accent), is_busy);
        let yes = if is_busy { yes } else { yes.on_click(cx.listener(move |m, _, _, cx| set_deprioritized(m, &gr, true, cx))) };
        kit::modal_box(t, 480.)
            .p(px(20.))
            .gap(px(12.))
            .child(head(div().child(title).into_any_element()))
            .child(div().flex().flex_wrap().text_size(px(14.)).child(div().font_weight(FontWeight::BOLD).child(name)).child(rest))
            .child(kit::help(t, paras[1].clone()))
            .child(div().flex().justify_end().gap(px(8.)).child(kit::btn(t, "goal-deprio-no", buttons[0]).on_click(cx.listener(|m, _, _, cx| close_dialog(m, cx)))).child(yes))
    } else if let Some(n) = &m.goal_page.note_dialog {
        let title = div()
            .flex()
            .gap(px(6.))
            .when(b(n, "pinned"), |d| d.child(div().text_color(t.goal).font_weight(FontWeight::SEMIBOLD).text_size(px(13.)).child("Pinned")))
            .child(note_dialog_title(n));
        kit::modal_box(t, 560.).p(px(20.)).gap(px(12.)).child(head(title.into_any_element())).child(note_body(t, &note_blocks(s(n, "text")), i(n, "id")))
    } else {
        return None;
    };
    Some(
        deferred(
            kit::scrim(t, "goal-dialog-scrim")
                .track_focus(&focus)
                .key_context("GoalDialog")
                // Esc closes the read-only note dialog (`READ_ONLY_MODALS`); in the Deprioritize
                // dialog it does nothing, and either way it goes no further.
                .on_action(cx.listener(|m, _: &crate::app::CloseOverlay, _, cx| {
                    if m.goal_page.note_dialog.is_some() && !m.goal_page.deprio_open {
                        close_dialog(m, cx);
                    }
                }))
                .flex()
                .items_center()
                .justify_center()
                // A click on the backdrop does nothing (`modal-bg`), but doesn't reach the page.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(div().id("goal-dialog").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(body)),
        )
        .with_priority(1)
        .into_any_element(),
    )
}

/// `noteBody` rendered: paragraphs, bullets, code blocks, links and `code`.
fn note_body(t: &Theme, blocks: &[NoteBlock], id: i64) -> Div {
    let inline = |segs: &[Inline], key: String| -> AnyElement {
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
                    hl.push((start..text.len(), HighlightStyle { color: Some(t.accent), underline: Some(UnderlineStyle { thickness: px(1.), color: Some(t.accent), wavy: false }), ..Default::default() }));
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
    };
    let mut col = div().flex().flex_col().gap(px(10.)).text_size(px(14.)).line_height(px(21.)).text_color(t.text);
    for (bi, block) in blocks.iter().enumerate() {
        col = col.child(match block {
            NoteBlock::P(lines) => {
                let mut p = div().flex().flex_col();
                for (li, l) in lines.iter().enumerate() {
                    p = p.child(inline(l, format!("gnb-{id}-{bi}-{li}")));
                }
                p
            }
            NoteBlock::Ul(items) => {
                let mut ul = div().flex().flex_col().gap(px(4.)).pl(px(18.));
                for (li, it) in items.iter().enumerate() {
                    ul = ul.child(div().flex().gap(px(6.)).child(div().flex_none().text_color(t.muted).child("•")).child(div().flex_1().min_w_0().child(inline(it, format!("gnb-{id}-{bi}-{li}")))));
                }
                ul
            }
            NoteBlock::Pre(code) => div()
                .px(px(12.))
                .py(px(10.))
                .rounded(px(8.))
                .bg(t.bg)
                .border_1()
                .border_color(t.border)
                .font_family(t.mono_font.clone())
                .text_size(px(12.5))
                .child(code.clone()),
        });
    }
    col
}

/// `issueAside(false)`: the issue picked in the goal's backlog, beside it (nothing when none is).
fn issue_aside(m: &mut MainWindow, t: &Theme, window: &mut Window, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    use crate::ui::issue_panel as ip;
    let r = match &m.panel {
        Some(Panel::Issue { r }) => r.clone(),
        _ => return None,
    };
    let b = m.data.issue.clone().filter(|b| fmt::ref_of(b, "B") == r);
    Some(match b {
        Some(b) => {
            let view = {
                let ctx = ip::Ctx::of(m);
                let mut v = ip::aside_view(&ctx, &b);
                v.detail = ip::detail_view(&ctx, &b, false);
                v
            };
            ip::render_aside(m, t, Some((&b, &view)), None, window, cx)
        }
        None => {
            let err = m.data.errs.issue.clone();
            ip::render_aside(m, t, None, err.as_deref(), window, cx)
        }
    })
}

// ------------------------------------------------------------------ page

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let want = match &m.page {
        Page::Goal(r) => r.clone(),
        _ => String::new(),
    };
    if m.goal_page.for_goal != want {
        m.goal_page.reset_for(&want);
    }
    let Some(g) = m.data.goal.clone().filter(|g| fmt::ref_of(g, "G") == want) else {
        // `P.goalErr` in place of "Loading…" (pages.js `renderGoalPage`).
        let err = m.data.errs.goal.clone();
        return div()
            .flex_1()
            .pt(px(44.))
            .px(px(28.))
            .child(match err {
                Some(e) => div().text_size(px(12.5)).text_color(t.down).font_weight(FontWeight::MEDIUM).child(e),
                None => kit::help(&t, "Loading…"),
            })
            .into_any_element();
    };
    let gr = fmt::ref_of(&g, "G");
    let backlog_view = m.goal_page.backlog_view;
    let h = header_view(&g, m.jira_on());
    let hot = h.backlog_hot;
    let labels = [h.tabs[0].as_str(), h.tabs[1].as_str()];
    let tabs = kit::seg(&t, "goal-view", &labels, if backlog_view { 1 } else { 0 }, |ix, item| {
        item.when(ix == 1 && hot, |d| d.text_color(t.warn_fg)).on_click(cx.listener(move |m, _, _, cx| {
            m.goal_page.backlog_view = ix == 1;
            // `goal-view` also closes the open issue.
            if matches!(m.panel, Some(Panel::Issue { .. })) {
                m.close_panel(cx);
            }
            cx.notify();
        }))
    });
    let tool = if backlog_view {
        let gr = gr.clone();
        kit::btn(&t, "goal-add-issue", "Add an issue").on_click(cx.listener(move |m, _, window, cx| modals::open_issue_form(m, Some(gr.clone()), window, cx)))
    } else {
        let (gid, project) = (i(&g, "id"), fmt::opt_s(&g, "project").map(str::to_string));
        kit::btn(&t, "goal-add-task", "+  Add a task").on_click(cx.listener(move |m, _, window, cx| {
            let opts = modals::TaskFormOpts { goal_id: Some(gid), project: project.clone(), planned: true, ..Default::default() };
            modals::open_task_form(m, opts, window, cx);
        }))
    };
    let left = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(380.))
        .gap(px(12.))
        .child(div().flex().items_center().gap(px(8.)).child(tabs).child(div().flex_1()).child(tool))
        .when(!backlog_view, |d| d.children(gate(m, &t, &g, cx)).child(task_rows(m, &t, &g, cx)).child(how_runs(m, &t, &g, cx)))
        .when(backlog_view, |d| d.child(backlog(m, &t, &g, cx)));
    // Tasks: Attached + Goal notes. Backlog: the picked issue (`issueAside(false)`: no move picker).
    let right = if backlog_view {
        issue_aside(m, &t, window, cx).map(|a| div().flex().flex_col().flex_none().w(px(360.)).child(a))
    } else {
        Some(div().flex().flex_col().flex_none().w(px(320.)).gap(px(16.)).child(attachments(m, &t, &g, window, cx)).child(notes(m, &t, &g, window, cx)))
    };
    let overlays: Vec<AnyElement> = [start_menu(m, &t, &g, cx), picker(m, &t, window, cx), dialog(m, &t, &g, cx)].into_iter().flatten().collect();
    div()
        .id("goal-page")
        .flex_1()
        .min_w_0()
        .overflow_y_scroll()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(22.))
                .max_w(px(1180.))
                .pt(px(40.))
                .px(px(28.))
                .pb(px(40.))
                .child(header(m, &t, &g, cx))
                .child(div().flex().items_start().gap(px(20.)).child(left).children(right)),
        )
        .children(overlays)
        .into_any_element()
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parity::{self, golden};
    // An explicit import beats the glob: `#[gpui_kit::test]` expands to `#[test]`, which must be std's.
    use ::core::prelude::v1::test;

    fn blocks_json(blocks: &[NoteBlock]) -> Value {
        let seg = |l: &Vec<Inline>| -> Value {
            Value::Array(
                l.iter()
                    .map(|s| match s {
                        Inline::Text(x) => json!(["text", x]),
                        Inline::Link(x) => json!(["link", x]),
                        Inline::Code(x) => json!(["code", x]),
                    })
                    .collect(),
            )
        };
        Value::Array(
            blocks
                .iter()
                .map(|b| match b {
                    NoteBlock::P(lines) => json!({"p": lines.iter().map(seg).collect::<Vec<_>>()}),
                    NoteBlock::Ul(items) => json!({"ul": items.iter().map(seg).collect::<Vec<_>>()}),
                    NoteBlock::Pre(x) => json!({"pre": x}),
                })
                .collect(),
        )
    }

    fn note_items_json(groups: Vec<(&'static str, Vec<NoteItem>)>) -> Value {
        json!(groups
            .into_iter()
            .map(|(title, items)| json!({"title": title, "items": items.iter().map(|n| json!({
                "text": if n.pinned { format!("Pinned {}", n.text) } else { n.text.clone() },
                "from": n.from, "long": n.long, "pinned": n.pinned})).collect::<Vec<_>>()}))
            .collect::<Vec<_>>())
    }

    /// Every pure view against the web board's own output (times of day: `times_match_web`).
    #[::core::prelude::v1::test]
    fn views_match_web() {
        let g = golden("goal");
        let skip = ["action", "noteFromTimes", "issueFromTimes"];
        let cases = parity::Golden { cases: g.cases.iter().filter(|c| !skip.contains(&c["input"]["fn"].as_str().unwrap_or(""))).cloned().collect() };
        cases.check(|i| {
            let goal = &i["goal"];
            match i["fn"].as_str().unwrap() {
                "goalState" => json!(goal_state(goal).0),
                "header" => {
                    let h = header_view(goal, i["jira"].as_bool().unwrap_or(false));
                    let meta = [Some(h.project.clone()), h.epic.clone(), Some(h.counts.clone())].into_iter().flatten().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" ");
                    json!({"pills": h.pills, "name": h.name, "tldr": h.tldr.map(|x| format!("TLDR {x}")), "meta": meta, "epic_link": h.epic_link,
                           "tabs": h.tabs, "tool": "Add a task"})
                }
                "backlogTool" => json!("Add an issue"),
                "runButtons" => {
                    let (split, bs) = run_buttons_view(goal, &i["hours"]);
                    json!({"split": split, "buttons": bs.iter().map(|b| json!({"act": b.act, "label": b.label, "title": b.title})).collect::<Vec<_>>()})
                }
                "startMenu" => json!(start_menu_view(&i["hours"]).into_iter().map(|(a, t, s)| json!({"act": a, "title": t, "sub": s})).collect::<Vec<_>>()),
                "opensAt" => {
                    let (d, a) = opens_at(&i["hours"]);
                    json!([d, a])
                }
                "taskRows" => {
                    let tasks = arr(goal, "tasks");
                    let h = how_runs_view(goal);
                    json!({
                        "empty": tasks.is_empty().then_some("No tasks in this goal yet. Add one, or let Claude plan them."),
                        "rows": tasks.iter().enumerate().map(|(ix, t)| {
                            let r = task_row_view(t, ix, tasks, goal);
                            json!({"n": r.n.to_string(), "chip": r.chip, "title": r.title, "meta": r.meta, "jira_tip": r.jira_tip, "pr_text": r.pr_text, "pr_tip": r.pr_tip, "planned": r.planned})
                        }).collect::<Vec<_>>(),
                        "max_options": h.options.iter().map(|(_, l)| l.clone()).collect::<Vec<_>>(),
                        "max_selected": h.selected, "run_in_order": h.run_in_order, "auto_close": h.auto_close,
                    })
                }
                "gate" => match gate_view(goal) {
                    Some((label, text, (act, button))) => json!({"label": label, "text": text, "buttons": [{"act": act, "label": button}]}),
                    None => json!({"label": null, "text": null, "buttons": []}),
                },
                "deprioDialog" => {
                    let (title, body, buttons) = deprio_dialog_view(goal);
                    json!({"title": title, "body": body, "buttons": buttons})
                }
                "notes" => json!({"help": NOTES_HELP, "groups": note_items_json(notes_view(arr(i, "notes"))), "add_button": true}),
                "noteForm" => json!({"kinds": NOTE_KINDS.iter().map(|(_, l)| *l).collect::<Vec<_>>(), "placeholder": "What every task in this goal should know",
                                     "buttons": ["Save note", "Cancel"], "add_button": false}),
                "noteBody" => blocks_json(&note_blocks(i["text"].as_str().unwrap())),
                "noteDialog" => {
                    let n = &i["note"];
                    json!(format!("{}{}", if b(n, "pinned") { "Pinned " } else { "" }, note_dialog_title(n)))
                }
                "attachments" => {
                    let list = arr(goal, "attachments");
                    json!({"empty": list.is_empty().then_some(ATTACH_HELP),
                           "items": attachments_view(list, &fmt::ref_of(goal, "G")).iter().map(|a| json!({"name": a.name, "linked": a.linked, "meta": a.meta, "tip": a.tip})).collect::<Vec<_>>()})
                }
                "attMenu" => json!(att_menu_items(&i["att"])),
                "attEdit" => {
                    let a = &i["att"];
                    let cur = fmt::opt_s(a, "kind").unwrap_or("other");
                    json!({"kinds": ATT_KINDS.iter().map(|(k, l)| json!([l, *k == cur])).collect::<Vec<_>>(), "title": s(a, "title"),
                           "url": "https://… or /path/to/file", "buttons": ["Save", "Cancel"]})
                }
                "backlog" => {
                    let sel: Vec<String> = arr(i, "sel").iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
                    let v = backlog_view(goal, &sel, b(i, "showDropped"), &HashMap::new(), &[], b(i, "jira"), false);
                    json!({
                        "help": BACKLOG_HELP,
                        "bar": v.bar.map(|b| json!({"label": b.label, "buttons": b.buttons})),
                        "rows": v.rows.iter().map(|r| json!({"checkbox": r.checkbox, "kind": r.kind, "title": r.title, "from": r.from, "actions": r.actions, "state": r.state})).collect::<Vec<_>>(),
                        "closed_line": v.closed.map(|(l, b)| match b { Some(b) => format!("{} {b}", l.trim_end()), None => l }),
                    })
                }
                "goalPicker" => json!(goal_picker_items(arr(i, "goals"), i["q"].as_str().unwrap()).into_iter().map(|(v, n, s)| json!({"v": v, "name": n, "sub": s})).collect::<Vec<_>>()),
                f => panic!("no app function for {f}"),
            }
        });
    }

    /// The toasts the actions show, from the web's own `ok` messages.
    #[::core::prelude::v1::test]
    fn toasts_match_web() {
        let g = golden("goal").only("action");
        let find = |n: &str| g.cases.iter().find(|c| c["name"] == format!("action {n}")).map(|c| c["expect"].clone()).unwrap();
        let open = json!({"on": true, "open": true});
        let closed = json!({"on": true, "open": false, "next_open": "2026-10-08T06:00"});
        assert_eq!(json!({"toast": run_toast(&json!({"queued_now": 2}), &open)}), find("run")["feedback"]);
        assert_eq!(json!({"toast": run_toast(&json!({"queued_now": 0}), &open)}), find("run, nothing queued")["feedback"]);
        assert_eq!(json!({"toast": run_toast(&json!({"queued_now": 1}), &open)}), find("run, 1 queued")["feedback"]);
        assert_eq!(json!({"toast": run_toast(&json!({"queued_now": 3}), &closed)}), find("run after hours")["feedback"]);
        assert_eq!(json!({"toast": run_now_toast(&json!({"queued_now": 2}))}), find("run now")["feedback"]);
        assert_eq!(json!({"toast": run_now_toast(&json!({}))}), find("run now, none")["feedback"]);
        for (name, action) in [("bulk make tasks", "task"), ("bulk tickets", "ticket"), ("bulk drop", "drop"), ("bulk move to G2", "move")] {
            assert_eq!(json!([bulk_toast(action, 2), bulk_toast(action, 1)]), find(name)["bulk_toast"], "{name}");
        }
    }

    /// Times of day ("you, 2:05 PM", "Oct 3") through the shared `fmt::hhmm`.
    #[::core::prelude::v1::test]
    fn times_match_web() {
        let g = golden("goal");
        let mut cases = g.only("noteFromTimes").cases;
        cases.extend(g.only("issueFromTimes").cases);
        parity::Golden { cases }.check(|i| match i["fn"].as_str().unwrap() {
            "noteFromTimes" => json!(arr(i, "notes").iter().map(note_from).collect::<Vec<_>>()),
            _ => json!([
                issue_from(&json!({"source": "terminal", "found_by_name": "T1 Add", "found_by_task": 1, "created_at": "2026-10-07T14:05:00Z"}), true),
                issue_from(&json!({"source": "you", "created_at": "2026-10-03T09:00:00Z"}), true)
            ]),
        });
    }

    // ---------------------------------------------------------------- actions through a real window

    fn expect_post(name: &str) -> (String, Value, Value) {
        let g = golden("goal").only("action");
        let c = g.cases.iter().find(|c| c["name"] == format!("action {name}")).unwrap_or_else(|| panic!("no golden action {name}"));
        let post = &c["expect"]["posts"][0];
        (post[0].as_str().unwrap().trim_start_matches('/').to_string(), post[1].clone(), c["expect"]["feedback"].clone())
    }

    fn goal_window(cx: &mut gpui_kit::TestAppContext) -> (gpui_kit::WindowHandle<MainWindow>, std::sync::Arc<parity::Recording>) {
        let (w, rec) = parity::window(cx);
        w.update(cx, |m, _, cx| m.go(Page::Goal("G1".into()), cx)).unwrap();
        parity::settle(cx);
        rec.clear();
        (w, rec)
    }

    /// Path and body of the last POST to `path`, and what it showed.
    fn sent(rec: &parity::Recording, path: &str) -> Value {
        rec.last(path).unwrap_or_else(|| panic!("nothing posted to {path}: {:?}", rec.posts()))
    }

    fn feedback(w: &gpui_kit::WindowHandle<MainWindow>, cx: &mut gpui_kit::TestAppContext, grp: Option<&str>) -> Value {
        w.update(cx, |m, _, _| match grp {
            Some(g) => m.goal_page.notes.get(g).map(|(t, err)| if *err { json!({"error": t}) } else { json!({"note": g, "text": t}) }).unwrap_or(Value::Null),
            None => m.toasts.last().map(|t| json!({"toast": t.text})).unwrap_or(Value::Null),
        })
        .unwrap()
    }

    #[gpui_kit::test]
    fn run_and_settings_send_what_the_web_sent(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = goal_window(cx);
        let cases: Vec<(&str, Box<dyn Fn(&mut MainWindow, &mut Context<MainWindow>)>)> = vec![
            ("run", Box::new(|m, cx| run_goal(m, "G1", false, cx))),
            ("run now", Box::new(|m, cx| run_goal(m, "G1", true, cx))),
            ("plan", Box::new(|m, cx| plan_goal(m, "G1", cx))),
            ("pause", Box::new(|m, cx| set_paused(m, "G1", true, cx))),
            ("resume (gate)", Box::new(|m, cx| set_paused(m, "G1", false, cx))),
            ("deprioritize (dialog)", Box::new(|m, cx| set_deprioritized(m, "G1", true, cx))),
            ("bring back (gate)", Box::new(|m, cx| set_deprioritized(m, "G1", false, cx))),
            ("set max terminals", Box::new(|m, cx| set_run_option(m, "G1", "max_terminals", json!(3), cx))),
            ("set run in order off", Box::new(|m, cx| set_run_option(m, "G1", "run_in_order", json!(false), cx))),
            ("set auto close on", Box::new(|m, cx| set_run_option(m, "G1", "auto_close", json!(true), cx))),
        ];
        for (name, f) in cases {
            rec.clear();
            w.update(cx, |m, _, cx| f(m, cx)).unwrap();
            parity::settle(cx);
            let (path, body, fb) = expect_post(name);
            assert_eq!(sent(&rec, &path), body, "{name}: body");
            assert_eq!(rec.posts().len(), 1, "{name}: one request");
            // Where the result shows (toast vs the group's note); run's own messages depend on the board's answer.
            let grp = fb.get("note").and_then(Value::as_str);
            let got = feedback(&w, cx, grp);
            if grp.is_some() {
                assert_eq!(got.get("note"), fb.get("note"), "{name}: shown in {grp:?}, got {got}");
            } else if name != "run" && name != "run now" {
                assert_eq!(got, fb, "{name}: toast");
            }
        }
    }

    #[gpui_kit::test]
    fn notes_attachments_and_issues_send_what_the_web_sent(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = goal_window(cx);
        // A note: trimmed, kind defaults to finding; then a decision.
        w.update(cx, |m, _, cx| {
            let input = kit::Input::new(cx, "", true);
            input.set_text("  A note  ", cx);
            m.goal_page.note_inputs.insert("G1".into(), input);
            save_note(m, "G1", cx);
        })
        .unwrap();
        parity::settle(cx);
        let (path, body, fb) = expect_post("note, finding");
        assert_eq!(sent(&rec, &path), body);
        assert_eq!(feedback(&w, cx, Some("gnote:G1")), fb);
        w.update(cx, |m, _, cx| {
            m.goal_page.note_inputs["G1"].set_text("Decided", cx);
            m.goal_page.note_kinds.insert("G1".into(), "decision");
            save_note(m, "G1", cx);
        })
        .unwrap();
        parity::settle(cx);
        assert_eq!(sent(&rec, &path), expect_post("note, decision").1);
        // An empty note sends nothing and says so.
        rec.clear();
        w.update(cx, |m, _, cx| {
            m.goal_page.note_inputs["G1"].set_text("   ", cx);
            save_note(m, "G1", cx);
        })
        .unwrap();
        assert!(rec.posts().is_empty());
        assert_eq!(feedback(&w, cx, Some("gnote:G1")), json!({"error": "Write the note first."}));

        // Attachments: remove, and edits that send only what was touched.
        rec.clear();
        w.update(cx, |m, _, cx| remove_attachment(m, 2, "G1", cx)).unwrap();
        parity::settle(cx);
        assert_eq!(sent(&rec, "attachments/2/remove"), expect_post("remove attachment").1);
        let att = json!({"id": 2, "kind": "results", "title": "Bench numbers", "url": "~/bench/results.md"});
        for (name, edit) in [
            ("edit attachment, title + kind", Box::new(|m: &mut MainWindow, cx: &mut Context<MainWindow>| {
                let e = m.goal_page.att_edit.as_mut().unwrap();
                e.kind = Some("doc".into());
                let title = e.title.field.clone();
                title.update(cx, |f, cx| f.set_text("New title", cx));
            }) as Box<dyn Fn(&mut MainWindow, &mut Context<MainWindow>)>),
            ("edit attachment, url", Box::new(|m: &mut MainWindow, cx: &mut Context<MainWindow>| {
                let url = m.goal_page.att_edit.as_ref().unwrap().url.field.clone();
                url.update(cx, |f, cx| f.set_text("~/x.md", cx));
            })),
            ("edit attachment, untouched", Box::new(|_: &mut MainWindow, _: &mut Context<MainWindow>| {})),
        ] {
            rec.clear();
            let a = att.clone();
            w.update(cx, |m, _, cx| start_att_edit(m, &a, cx)).unwrap();
            w.update(cx, |m, _, cx| edit(m, cx)).unwrap();
            parity::settle(cx);
            w.update(cx, |m, _, cx| save_attachment(m, cx)).unwrap();
            parity::settle(cx);
            assert_eq!(sent(&rec, "attachments/2"), expect_post(name).1, "{name}");
        }

        // Backlog rows and the bulk bar.
        for (name, act) in [("promote (goal row)", "promote"), ("ticket (goal row)", "ticket"), ("drop (goal row)", "drop")] {
            rec.clear();
            w.update(cx, |m, _, cx| issue_action(m, act, "B1", cx)).unwrap();
            parity::settle(cx);
            let (path, body, _) = expect_post(name);
            assert_eq!(sent(&rec, &path), body, "{name}");
        }
        w.update(cx, |m, _, _| assert!(m.goal_page.kept.contains_key("B1"), "a changed issue stays in the list")).unwrap();
        for (name, sel, f) in [
            ("bulk make tasks", vec!["B1", "B2"], Box::new(|m: &mut MainWindow, cx: &mut Context<MainWindow>| bulk(m, "task", json!({}), cx)) as Box<dyn Fn(&mut MainWindow, &mut Context<MainWindow>)>),
            ("bulk tickets", vec!["B2"], Box::new(|m: &mut MainWindow, cx: &mut Context<MainWindow>| bulk(m, "ticket", json!({}), cx))),
            ("bulk drop", vec!["B3", "B1"], Box::new(|m: &mut MainWindow, cx: &mut Context<MainWindow>| bulk(m, "drop", json!({}), cx))),
            ("bulk move to G2", vec!["B1"], Box::new(|m: &mut MainWindow, cx: &mut Context<MainWindow>| move_selected(m, "G2", cx))),
            ("bulk move out", vec!["B1"], Box::new(|m: &mut MainWindow, cx: &mut Context<MainWindow>| move_selected(m, "none", cx))),
        ] {
            rec.clear();
            w.update(cx, |m, _, cx| {
                m.goal_page.sel = sel.iter().map(|x| x.to_string()).collect();
                f(m, cx);
            })
            .unwrap();
            parity::settle(cx);
            assert_eq!(sent(&rec, "backlog/bulk"), expect_post(name).1, "{name}");
            // The sample board only has B1 and G1: other ids fail, and then (as on the web) the selection stays.
            let failed = w.update(cx, |m, _, _| m.goal_page.notes.get("bulk").is_some_and(|(_, err)| *err)).unwrap();
            let left = w.update(cx, |m, _, _| m.goal_page.sel.len()).unwrap();
            if name == "bulk move out" {
                assert!(!failed && left == 0, "{name}: done, selection cleared ({:?})", w.update(cx, |m, _, _| m.goal_page.notes.get("bulk").cloned()).unwrap());
            } else {
                assert!(!failed || left == sel.len(), "{name}: a failure keeps the selection");
            }
        }
    }

    #[::core::prelude::v1::test]
    fn shift_click_picks_a_range() {
        let pickable: Vec<String> = ["B1", "B2", "B3", "B4"].iter().map(|x| x.to_string()).collect();
        let mut st = State::default();
        let pick_on = |st: &mut State, r: &str, shift: bool| {
            let span: Vec<String> = match (shift, st.anchor.as_ref().and_then(|a| pickable.iter().position(|x| x == a)), pickable.iter().position(|x| x == r)) {
                (true, Some(a), Some(j)) => pickable[a.min(j)..=a.max(j)].to_vec(),
                _ => vec![r.to_string()],
            };
            for x in span {
                if !st.sel.contains(&x) {
                    st.sel.push(x);
                }
            }
            st.anchor = Some(r.to_string());
        };
        pick_on(&mut st, "B2", false);
        pick_on(&mut st, "B4", true);
        assert_eq!(st.sel, vec!["B2", "B3", "B4"]);
    }

    #[::core::prelude::v1::test]
    fn kept_rows_stay_where_they_were() {
        let g = json!({"backlog": [{"id": 1, "ref": "B1", "state": "open"}, {"id": 3, "ref": "B3", "state": "open"}]});
        let mut kept = HashMap::new();
        kept.insert("B2".to_string(), json!({"id": 2, "ref": "B2", "state": "drop"}));
        let order = vec!["B1".to_string(), "B2".to_string(), "B3".to_string()];
        let rows = backlog_rows(&g, &kept, &order, false);
        assert_eq!(rows.iter().map(|r| fmt::ref_of(r, "B")).collect::<Vec<_>>(), vec!["B1", "B2", "B3"]);
        // Without being kept, a dropped issue is hidden until Show.
        let g2 = json!({"backlog": [{"id": 2, "ref": "B2", "state": "drop"}]});
        assert!(backlog_rows(&g2, &HashMap::new(), &[], false).is_empty());
        assert_eq!(backlog_rows(&g2, &HashMap::new(), &[], true).len(), 1);
    }
}
