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
use crate::ui::kit;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

mod waves;

const START_MENU: &str = "goal-start";
const ATT_MENU: &str = "goal-att";
const MAX_TERMINALS: [i64; 7] = [1, 2, 3, 4, 5, 6, 8];
const ATT_KINDS: [(&str, &str); 6] = [("design", "Design"), ("proposal", "Proposal"), ("doc", "Doc"), ("evidence", "Evidence"), ("results", "Results"), ("other", "Link")];
const FOLD_NOTES: &str = "tb.fold.gnotes";
const BACKLOG_HELP: &str = "Issues found while testing, building or reviewing that no task covers yet. Terminals add them instead of fixing them on the side.";
const NOTES_HELP: &str = "Every task in this goal gets these, and the open backlog, in its handoff. A new terminal starts with what earlier tasks learned.";
const ATTACH_HELP: &str = "Designs, proposals and docs for the whole goal. Every task’s handoff lists them.";

#[derive(Default)]
pub struct State {
    // Reset when the goal changes (the web's resetPageState).
    for_goal: String,
    pub backlog_view: bool,
    /// The QA tab (shown only while the goal has QA comments: Settings ▸ QA on).
    pub qa_view: bool,
    /// Waves opened or closed by hand, by "G1:2" / "G1:none" (`S.waveOpen`).
    pub wave_open: HashMap<String, bool>,
    /// QA comments whose "Their comment" is open.
    qa_open: HashSet<String>,
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
    // Kept across goals, like the web's S.notes / S.busy.
    notes: HashMap<String, (String, bool)>,
    busy: HashSet<String>,
}

impl State {
    pub fn reset_for(&mut self, goal: &str) {
        self.for_goal = goal.to_string();
        self.backlog_view = std::env::var("TASKBOARD_GOAL_VIEW").is_ok_and(|v| v == "backlog");
        self.qa_view = std::env::var("TASKBOARD_GOAL_VIEW").is_ok_and(|v| v == "qa");
        self.qa_open.clear();
        self.wave_open.clear();
        self.sel.clear();
        self.anchor = None;
        self.kept.clear();
        self.order.clear();
        self.show_dropped = false;
        self.deprio_open = false;
        self.note_dialog = None;
    }
}

/// Open a goal's page, on its Backlog tab when `backlog` (`#/goals/G3?view=backlog`).
pub fn open(m: &mut MainWindow, r: &str, backlog: bool, cx: &mut Context<MainWindow>) {
    m.go(Page::Goal(r.to_string()), cx);
    m.goal_page.reset_for(r);
    m.goal_page.backlog_view = backlog;
    m.goal_page.qa_view = false;
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
    } else if fmt::from_review_log(source.unwrap_or("")) {
        "From the Review log".to_string()
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

// ------------------------------------------------------------------ goal state and counts

/// `countedTasks(g)`: the goal's own tasks, then the shared ones from other goals that also count toward it.
fn counted(g: &Value) -> Vec<Value> {
    arr(g, "tasks").iter().chain(arr(g, "shared")).cloned().collect()
}

/// `goalState(countedTasks(g), g)`: the goal's state label and its chip color.
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

/// `goalCounts(g)` for a goal detail (it has its tasks, and the shared ones).
fn counts(g: &Value) -> Counts {
    let tasks = counted(g);
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
    /// The Tasks / Backlog (/ QA) tab labels (the page draws the counts from `counts` itself).
    #[cfg_attr(not(test), allow(dead_code))]
    pub tabs: Vec<String>,
    #[cfg_attr(not(test), allow(dead_code))]
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
        tabs: tab_items(g).into_iter().map(|(label, n, _)| format!("{label} {n}")).collect(),
        backlog_hot: open > 0,
    }
}

/// The goal page's tabs: (label, count, hot). QA shows while the goal has QA comments: the
/// count is the ones waiting on you (hot), else all of them.
pub fn tab_items(g: &Value) -> Vec<(&'static str, i64, bool)> {
    let c = counts(g);
    let open = open_issue_count(g);
    let mut items = vec![("Tasks", c.n, false), ("Backlog", open, open > 0)];
    let qa = arr(g, "qa");
    if !qa.is_empty() {
        let asks = qa.iter().filter(|x| b(x, "waiting")).count() as i64;
        items.push(("QA", if asks > 0 { asks } else { qa.len() as i64 }, asks > 0));
    }
    items
}

// ------------------------------------------------------------------ QA tab

pub struct QaItem {
    pub r: String,
    pub wait: bool,
    pub chip: String,
    /// The chip's status key (`st-<key>`).
    pub key: &'static str,
    pub ask: Vec<Inline>,
    pub author: String,
    pub jira_key: String,
    pub url: String,
    pub at: String,
    pub source_task: Option<String>,
    /// The follow-up task: its ref, "working" and title.
    pub task: Option<(String, String, Option<String>)>,
    pub by: Option<String>,
    pub text: Option<String>,
}

/// `qaState(c)`: (label, status key).
fn qa_state(c: &Value) -> (String, &'static str) {
    if b(c, "waiting") {
        return ("Waiting on you".into(), "needs");
    }
    if let Some(t) = fmt::opt_s(c, "task") {
        return (format!("Now {t}"), status_key(&json!({"status": c["task_status"]})));
    }
    if c["verdict"].is_null() {
        return ("Being read".into(), "queued");
    }
    ("Left as it is".into(), "planned")
}

/// `qaItem(c)`.
pub fn qa_item(c: &Value) -> QaItem {
    let (chip, key) = qa_state(c);
    let ask = fmt::opt_s(c, "ask").or(fmt::opt_s(c, "title")).unwrap_or("Read the comment on the ticket");
    QaItem {
        r: s(c, "ref").to_string(),
        wait: b(c, "waiting"),
        chip,
        key,
        ask: inline_segments(ask),
        author: fmt::opt_s(c, "author").unwrap_or("QA").to_string(),
        jira_key: s(c, "jira_key").to_string(),
        url: s(c, "url").to_string(),
        at: fmt::hhmm(s(c, "created_at")),
        source_task: fmt::opt_s(c, "source_task").map(str::to_string),
        task: fmt::opt_s(c, "task").map(|t| {
            (t.to_string(), status_label(status_key(&json!({"status": c["task_status"]}))).to_lowercase(), fmt::opt_s(c, "task_title").map(str::to_string))
        }),
        by: fmt::opt_s(c, "handled_by").filter(|_| fmt::opt_s(c, "task").is_none() && fmt::opt_s(c, "handled_at").is_some()).map(str::to_string),
        text: fmt::opt_s(c, "text").map(str::to_string),
    }
}

/// `goalQa(qa)`: "Waiting on you" first, then "Handled".
pub fn qa_groups(qa: &[Value]) -> Vec<(&'static str, Vec<QaItem>)> {
    let waiting: Vec<QaItem> = qa.iter().filter(|c| b(c, "waiting")).map(qa_item).collect();
    let rest: Vec<QaItem> = qa.iter().filter(|c| !b(c, "waiting")).map(qa_item).collect();
    [("Waiting on you", waiting), ("Handled", rest)].into_iter().filter(|(_, v)| !v.is_empty()).collect()
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
    /// The row's tooltip (shared rows: which goal runs it).
    pub tip: Option<String>,
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
    let mut parts = parts;
    let also: Vec<&str> = arr(t, "also").iter().map(|x| s(x, "ref")).collect();
    if !also.is_empty() {
        parts.push(Some(format!("also for {}", also.join(", "))));
    }
    let waiting = fmt::opt_s(t, "waiting").is_some();
    parts.extend(waves::plan_facts(t).into_iter().filter(|(_, refs)| !(*refs && waiting)).map(|(text, _)| Some(text)));
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
        tip: None,
    }
}

/// A row of `goalTasks`' "From other goals": a shared task its home goal runs (n is 0: it shows "·").
pub fn shared_row_view(t: &Value) -> TaskRow {
    let home = t.get("goal").filter(|g| g.is_object());
    let mut row = task_row_view(t, 0, std::slice::from_ref(t), &json!({}));
    row.n = 0;
    row.jira_tip = None;
    row.meta = match home {
        Some(g) => format!("From {} · {}", s(g, "ref"), s(g, "name")),
        None => "From another goal".into(),
    };
    row.tip = Some(format!("Counts toward this goal; {} runs it", home.map(|g| s(g, "ref")).unwrap_or("its own goal")));
    row
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
            buttons.extend(["Won’t do", "Clear"]);
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
        _ => format!("Closed {} as won’t do", fmt::plural(n, "issue", "issues")),
    }
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
        .border_color(if fill { t.accent } else { t.border_2 })
        .bg(if fill { t.accent } else { t.card })
        .text_color(t.on_accent)
        .text_size(px(11.))
        .line_height(px(14.))
        .font_weight(FontWeight::BOLD)
        .child(if on { "✓" } else if mixed { "–" } else { "" })
}

/// `--accent-tint` (the theme has no field for it).
fn accent_tint(t: &Theme) -> Hsla {
    match t.mode {
        crate::theme::ThemeMode::Light => rgb(0xf5f8ff).into(),
        crate::theme::ThemeMode::Dark => rgb(0x172036).into(),
    }
}

/// `.card-box`: 14px 16px padding, 12px radius, 10px gap.
fn card_box(t: &Theme) -> Div {
    div().flex().flex_col().gap(px(10.)).px(px(16.)).py(px(14.)).rounded(px(12.)).border_1().border_color(t.border).bg(t.card)
}

/// `.aside-card`: 12px 14px padding, 12px radius, 10px gap.
fn aside_card(t: &Theme) -> Div {
    div().flex().flex_col().min_w_0().gap(px(10.)).px(px(14.)).py(px(12.)).rounded(px(12.)).border_1().border_color(t.border).bg(t.card)
}

/// `.h3` at a size (13px; 12.5px in an aside card): semibold, muted, uppercase.
fn h3(t: &Theme, size: f32, text: &str) -> Div {
    div().text_size(px(size)).font_weight(FontWeight::SEMIBOLD).text_color(t.muted).whitespace_nowrap().child(text.to_uppercase())
}

/// `.pill`: 12px semibold, 2px 10px, fully rounded.
fn pill(fg: Hsla, bg: Hsla) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .px(px(10.))
        .py(px(2.))
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_size(px(12.))
        .line_height(px(18.))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
}

/// `.chip`: 11.5px semibold, 1px 6px, 5px radius.
fn chip(fg: Hsla, bg: Hsla, text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .px(px(6.))
        .py(px(1.))
        .rounded(px(5.))
        .bg(bg)
        .text_color(fg)
        .text_size(px(11.5))
        .line_height(px(17.25))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .child(text.into())
}

/// The `.st-*` colors for a goal or task tone: (fg, bg, inset border).
fn st_colors(t: &Theme, tone: &str) -> (Hsla, Hsla, Option<Hsla>) {
    match tone {
        "accent" => (t.accent_fg, t.accent_soft, None),
        "warn" => (t.warn_fg, t.warn_soft, None),
        "up" => (t.up_fg, t.up_soft, None),
        "down" => (t.down, t.down_soft, None),
        "planned" => (t.muted, t.card, Some(t.border)),
        _ => (t.text_2, t.col, None),
    }
}

/// `.chip.st-*`.
fn st_chip(t: &Theme, tone: &str, label: impl Into<SharedString>) -> Div {
    let (fg, bg, line) = st_colors(t, tone);
    chip(fg, bg, label).when_some(line, |d, c| d.shadow(vec![BoxShadow { color: c, offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(1.), inset: true }]))
}

/// `.k-*`: a backlog issue's kind chip.
fn kind_chip(t: &Theme, kind: &str, label: impl Into<SharedString>) -> Div {
    let (fg, bg) = match kind {
        "bug" => (t.down, t.down_soft),
        "gap" => (t.warn_fg, t.warn_soft),
        "follow" => (t.accent_fg, t.accent_soft),
        _ => (t.text_2, t.col),
    };
    chip(fg, bg, label)
}

/// `.btn.md` (34px, 0 12px, 7px radius, 13px): `primary`, `danger` or outlined; the caller adds
/// the icon and label.
fn md_btn(t: &Theme, id: impl Into<ElementId>, kind: &str) -> Stateful<Div> {
    let b = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(34.))
        .px(px(12.))
        .rounded(px(7.))
        .border_1()
        .text_size(px(13.))
        .whitespace_nowrap()
        .cursor_pointer();
    match kind {
        "primary" => b.bg(t.accent_btn).border_color(t.accent_btn).text_color(t.on_accent).font_weight(FontWeight::SEMIBOLD).hover(|s| s.opacity(0.94)),
        "danger" => {
            let h = t.down_soft;
            b.bg(t.card).border_color(t.down_line).text_color(t.down).hover(move |s| s.bg(h))
        }
        _ => {
            let h = t.border_2;
            b.bg(t.card).border_color(t.border).text_color(t.text).hover(move |s| s.border_color(h))
        }
    }
}

/// `.btn.sm` (32px, 0 10px, 7px radius, 12.5px): `soft`, outlined, or `ghost`.
fn btn_sm(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>, style: &str) -> Stateful<Div> {
    let b = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(32.))
        .px(px(10.))
        .rounded(px(7.))
        .border_1()
        .text_size(px(12.5))
        .whitespace_nowrap()
        .cursor_pointer()
        .child(label.into());
    match style {
        "soft" => b.bg(t.accent_soft).border_color(t.accent_soft).text_color(t.accent).font_weight(FontWeight::SEMIBOLD),
        "ghost" => {
            let hover = t.text;
            b.border_color(transparent_black()).text_color(t.muted).hover(move |s| s.text_color(hover))
        }
        _ => {
            let h = t.border_2;
            b.bg(t.card).border_color(t.border).text_color(t.text).hover(move |s| s.border_color(h))
        }
    }
}

/// `.seg.sm` with a `.count` in every button (the Tasks / Backlog tabs).
fn seg_counts(t: &Theme, id: &str, items: &[(&str, i64, bool)], on: usize, mut f: impl FnMut(usize, Stateful<Div>) -> Stateful<Div>) -> Div {
    let mut row = div().flex().flex_none().items_center().gap(px(2.)).p(px(3.)).rounded(px(9.)).bg(t.seg);
    for (ix, (label, n, hot)) in items.iter().enumerate() {
        let sel = ix == on;
        let hover = t.text;
        let (cfg, cbg) = if *hot { (t.warn_fg, t.warn_soft) } else { (t.muted, t.col) };
        let item = div()
            .id(SharedString::from(format!("{id}-{ix}")))
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(32.))
            .px(px(11.))
            .rounded(px(7.))
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .whitespace_nowrap()
            .text_color(if sel { t.text } else { t.muted })
            .when(sel, |d| d.bg(t.card).shadow(vec![BoxShadow { color: hsla(0., 0., 0., if t.mode == crate::theme::ThemeMode::Dark { 0.4 } else { 0.1 }), offset: point(px(0.), px(1.)), blur_radius: px(2.), spread_radius: px(0.), inset: false }]))
            .hover(move |s| s.text_color(hover))
            .child(label.to_string())
            .child(div().px(px(7.)).rounded_full().bg(cbg).text_color(cfg).text_size(px(12.)).line_height(px(18.)).font_weight(FontWeight::SEMIBOLD).child(n.to_string()));
        row = row.child(f(ix, item));
    }
    row
}

/// `PR_STAGE_ICON[phase]` (none for a phase it doesn't know).
fn stage_icon(phase: &str) -> Option<&'static str> {
    ["checks", "fix", "review", "comments", "merge", "merged", "declined"].into_iter().find(|p| *p == phase)
}

#[derive(Clone, Copy)]
enum Ico {
    Flag,
    Play,
    Pause,
    Chat,
    PrOpen,
    Jira,
    Chev,
    Right,
    Close,
    Stage(&'static str),
    Att(&'static str),
}

/// The web's inline SVG icons on this page, drawn on a canvas (`size` px square).
fn ico(kind: Ico, size: f32, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let grid = if matches!(kind, Ico::Att(_)) { 16. } else { 24. };
            let k = bounds.size.width.as_f32() / grid;
            let o = bounds.origin;
            let u = move |x: f32, y: f32| point(o.x + px(x * k), o.y + px(y * k));
            let arc = |cx: f32, cy: f32, r: f32, a0: f32, a1: f32| -> Vec<(f32, f32)> {
                (0..=12).map(|i| (a0 + (a1 - a0) * i as f32 / 12.).to_radians()).map(|a| (cx + r * a.cos(), cy + r * a.sin())).collect()
            };
            let line = |pts: &[(f32, f32)], w: f32, window: &mut Window| {
                let mut p = PathBuilder::stroke(px(w * k));
                p.move_to(u(pts[0].0, pts[0].1));
                for (x, y) in &pts[1..] {
                    p.line_to(u(*x, *y));
                }
                if let Ok(p) = p.build() {
                    window.paint_path(p, color);
                }
                // Round caps and joins.
                for (x, y) in pts {
                    let mut d = PathBuilder::fill();
                    let r = w / 2.;
                    d.move_to(u(x + r, *y));
                    for i in 1..=10 {
                        let a = std::f32::consts::TAU * i as f32 / 10.;
                        d.line_to(u(x + r * a.cos(), y + r * a.sin()));
                    }
                    d.close();
                    if let Ok(d) = d.build() {
                        window.paint_path(d, color);
                    }
                }
            };
            let fill = |pts: &[(f32, f32)], window: &mut Window| {
                let mut p = PathBuilder::fill();
                p.move_to(u(pts[0].0, pts[0].1));
                for (x, y) in &pts[1..] {
                    p.line_to(u(*x, *y));
                }
                p.close();
                if let Ok(p) = p.build() {
                    window.paint_path(p, color);
                }
            };
            let circle = |cx: f32, cy: f32, r: f32| arc(cx, cy, r, 0., 360.);
            match kind {
                Ico::Flag => {
                    line(&[(5., 21.), (5., 4.)], 2.2, window);
                    line(&[(5., 4.), (16., 4.), (14., 8.), (16., 12.), (5., 12.)], 2.2, window);
                }
                Ico::Play => fill(&[(8., 5.), (19.7, 12.), (8., 19.)], window),
                Ico::Pause => {
                    fill(&[(6., 5.), (10., 5.), (10., 19.), (6., 19.)], window);
                    fill(&[(14., 5.), (18., 5.), (18., 19.), (14., 19.)], window);
                }
                Ico::Chat => {
                    let mut pts = vec![(20., 15.)];
                    pts.extend(arc(18., 15., 2., 0., 90.));
                    pts.extend([(8., 17.), (4., 21.), (4., 5.)]);
                    pts.extend(arc(6., 5., 2., 180., 270.));
                    pts.push((18., 3.));
                    pts.extend(arc(18., 5., 2., 270., 360.));
                    pts.push((20., 15.));
                    line(&pts, 2., window);
                }
                Ico::PrOpen => {
                    for (x, y) in [(6., 6.), (6., 18.), (18., 18.)] {
                        fill(&circle(x, y, 2.6), window);
                    }
                    line(&[(6., 8.5), (6., 15.5)], 2.2, window);
                    let mut pts = vec![(18., 15.5), (18., 9.)];
                    pts.extend(arc(15., 9., 3., 0., -90.));
                    pts.push((11., 6.));
                    line(&pts, 2.2, window);
                    line(&[(13., 3.5), (10.5, 6.), (13., 8.5)], 2.2, window);
                }
                Ico::Jira => {
                    for (x0, y0) in [(11.45, 0.), (5.74, 5.76), (0., 11.51)] {
                        let mut pts = vec![(x0, y0), (x0 + 11.56, y0), (x0 + 12.55, y0 + 1.)];
                        pts.extend([(x0 + 12.55, y0 + 12.48), (x0 + 9.5, y0 + 11.), (x0 + 7.35, y0 + 7.27), (x0 + 7.35, y0 + 5.2), (x0 + 5.2, y0 + 5.2), (x0 + 1.5, y0 + 3.5)]);
                        fill(&pts, window);
                    }
                }
                Ico::Chev => line(&[(6., 9.), (12., 15.), (18., 9.)], 2.2, window),
                Ico::Close => {
                    line(&[(6., 6.), (18., 18.)], 2.2, window);
                    line(&[(18., 6.), (6., 18.)], 2.2, window);
                }
                Ico::Right => line(&[(9., 6.), (15., 12.), (9., 18.)], 2.2, window),
                // `PR_STAGE_ICON` (stroke 2.2).
                Ico::Stage(phase) => match phase {
                    "fix" => {
                        line(&[(14.5, 6.5), (16.5, 8.5)], 2.2, window);
                        line(&arc(17.5, 6.5, 3.5, 135., 405.), 2.2, window);
                        line(&[(15., 9.), (7.5, 16.5)], 2.2, window);
                        line(&arc(10.5, 17.5, 1.6, 0., 360.), 2.2, window);
                    }
                    "review" => {
                        line(&[(2.5, 12.), (6., 7.), (12., 5.5), (18., 7.), (21.5, 12.), (18., 17.), (12., 18.5), (6., 17.), (2.5, 12.)], 2.2, window);
                        line(&circle(12., 12., 2.8), 2.2, window);
                    }
                    "comments" => line(&[(4., 5.), (20., 5.), (20., 16.), (9., 16.), (4., 20.), (4., 5.)], 2.2, window),
                    "merge" => {
                        for (x, y) in [(6., 5.), (6., 19.), (18., 12.)] {
                            line(&circle(x, y, 2.3), 2.2, window);
                        }
                        line(&[(6., 7.3), (6., 16.7)], 2.2, window);
                        line(&[(6., 7.3), (7., 10.), (10., 11.5), (15.7, 12.)], 2.2, window);
                    }
                    "merged" => line(&[(5., 12.5), (9.5, 17.), (19., 7.5)], 2.2, window),
                    "declined" => {
                        line(&[(6., 6.), (18., 18.)], 2.2, window);
                        line(&[(18., 6.), (6., 18.)], 2.2, window);
                    }
                    _ => {
                        line(&circle(12., 12., 8.5), 2.2, window);
                        line(&[(12., 7.5), (12., 12.), (15., 14.)], 2.2, window);
                    }
                },
                Ico::Att(kind) => {
                    let w = 1.6;
                    match kind {
                        "design" => {
                            line(&[(2.5, 2.5), (13.5, 2.5), (13.5, 13.5), (2.5, 13.5), (2.5, 2.5)], w, window);
                            line(&[(2.5, 6.5), (13.5, 6.5)], w, window);
                            line(&[(6.5, 6.5), (6.5, 13.5)], w, window);
                        }
                        "proposal" => {
                            let mut pts = arc(8., 6.5, 4., 140., 400.);
                            pts.extend([(9.5, 11.3), (6.5, 11.3)]);
                            pts.push(pts[0]);
                            line(&pts, w, window);
                            line(&[(6.5, 13.5), (9.5, 13.5)], w, window);
                        }
                        "doc" => {
                            line(&[(4., 2.5), (9.5, 2.5), (12., 5.), (12., 13.5), (4., 13.5), (4., 2.5)], w, window);
                            line(&[(6.5, 8.), (9.5, 8.)], w, window);
                            line(&[(6.5, 10.5), (9.5, 10.5)], w, window);
                        }
                        "evidence" => {
                            line(&[(2.5, 13.5), (13.5, 13.5)], w, window);
                            line(&[(4.5, 11.), (4.5, 8.)], w, window);
                            line(&[(8., 11.), (8., 4.5)], w, window);
                            line(&[(11.5, 11.), (11.5, 6.5)], w, window);
                        }
                        "results" => {
                            line(&[(2.5, 13.5), (13.5, 13.5)], w, window);
                            line(&[(3.5, 10.5), (6.5, 7.5), (9., 9.5), (13., 5.)], w, window);
                            line(&[(10., 5.), (13., 5.), (13., 8.)], w, window);
                        }
                        _ => {
                            line(&arc(11.2, 5.2, 2.4, 135., 315.), w, window);
                            line(&arc(4.8, 10.8, 2.4, -45., 135.), w, window);
                            line(&[(6.2, 9.8), (9.8, 6.2)], w, window);
                        }
                    }
                }
            }
        },
    )
    .size(px(size))
    .flex_none()
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
    // `.bar.lg`: 8px, 4px radius.
    let bar = div()
        .flex()
        .w_full()
        .h(px(8.))
        .rounded(px(4.))
        .bg(t.col)
        .overflow_hidden()
        .child(div().h_full().bg(t.up).w(pct(c.done)))
        .child(div().h_full().bg(t.accent).w(pct(c.active)))
        .child(div().h_full().bg(t.backlog_dot).w(pct(c.queued)));
    let epic: Option<AnyElement> = h.epic.clone().map(|text| match (&h.epic_link, fmt::opt_s(g, "epic_key")) {
        (Some(url), Some(key)) => {
            let url = url.clone();
            let rest = text.trim_start_matches("Jira epic ").trim_start_matches(key).to_string();
            let fg = t.accent_fg;
            div()
                .flex()
                .whitespace_nowrap()
                .child("Jira epic ")
                .child(
                    div()
                        .id("goal-epic")
                        .cursor_pointer()
                        .font_family(t.mono_font.clone())
                        .font_weight(FontWeight::BOLD)
                        .text_color(fg)
                        .underline()
                        .child(key.to_string())
                        .on_click(move |_, _, cx| if url != "#" { cx.open_url(&url) }),
                )
                .child(rest)
                .into_any_element()
        }
        _ => div().child(text).into_any_element(),
    });
    let repo = fmt::opt_s(g, "repo_path").map(str::to_string);
    let (sfg, sbg, _) = st_colors(t, h.state_tone);
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
                .child(pill(t.goal, t.goal_soft).child(ico(Ico::Flag, 12., t.goal)).child("Goal"))
                .child(pill(sfg, sbg).child(h.pills[1].clone()))
                .child(pill(t.muted, t.panel_2).font_family(t.mono_font.clone()).font_weight(FontWeight::MEDIUM).child(h.pills[2].clone()))
                .child(div().flex_1())
                .child(run_buttons(m, t, g, cx)),
        )
        .child(div().text_size(px(26.)).font_weight(FontWeight::BOLD).line_height(relative(1.25)).child(h.name.clone()))
        .when_some(h.tldr.clone(), |d, tl| {
            d.child(
                div()
                    .max_w(px(820.))
                    .text_size(px(15.))
                    .line_height(relative(1.55))
                    .text_color(t.text)
                    .flex()
                    .items_baseline()
                    .child(div().flex_none().mr(px(8.)).text_size(px(11.)).font_weight(FontWeight::BOLD).text_color(t.text_2).child("TLDR"))
                    .child(div().flex_1().min_w_0().child(tl)),
            )
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(px(16.))
                .gap_y(px(8.))
                .text_size(px(13.))
                .text_color(t.muted)
                .child(
                    chip(t.muted, t.panel_2, h.project.clone())
                        .font_family(t.mono_font.clone())
                        .font_weight(FontWeight::NORMAL)
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
                let main = md_btn(t, "goal-run", "primary").child(ico(Ico::Play, 14., t.on_accent)).child(label).tooltip(kit::tip(bt.title.clone()));
                let main = act_btn(main, is_busy);
                let main = if is_busy { main } else { main.on_click(cx.listener(move |m, _, _, cx| run_goal(m, &gr2, false, cx))) };
                if split {
                    div()
                        .flex()
                        .child(main.rounded_r(px(0.)))
                        .child(
                            md_btn(t, "goal-run-caret", "primary")
                                .child(ico(Ico::Chev, 14., t.on_accent))
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
                let b = act_btn(md_btn(t, "goal-pause", "").child(ico(Ico::Pause, 14., t.text)).child(label).tooltip(kit::tip(bt.title.clone())), is_busy);
                if is_busy { b } else { b.on_click(cx.listener(move |m, _, _, cx| set_paused(m, &gr2, true, cx))) }.into_any_element()
            }
            "goal-plan-edit" => {
                let b = act_btn(md_btn(t, "goal-plan", "").child(ico(Ico::Chat, 14., t.text)).child(label).tooltip(kit::tip(bt.title.clone())), is_busy);
                if is_busy { b } else { b.on_click(cx.listener(move |m, _, _, cx| plan_goal(m, &gr2, cx))) }.into_any_element()
            }
            _ => md_btn(t, "goal-deprio", "danger").child(label)
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
    let bt = act_btn(md_btn(t, "goal-gate", "primary").h(px(36.)).px(px(14.)).rounded(px(8.)).child(if is_busy { "Sending…" } else { button }), is_busy);
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
            .px(px(16.))
            .py(px(14.))
            .rounded(px(12.))
            .bg(t.warn_soft)
            .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(t.warn_fg).child(label.to_uppercase()))
            .child(div().text_size(px(14.)).child(text))
            .child(div().flex().child(bt))
            .children(note),
    )
}

fn task_rows(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Div {
    let tasks = arr(g, "tasks");
    let shared = arr(g, "shared");
    if tasks.is_empty() && shared.is_empty() {
        // `.empty-box`.
        return div()
            .p(px(24.))
            .flex()
            .justify_center()
            .text_size(px(14.))
            .text_color(t.muted)
            .bg(t.card)
            .border_1()
            .border_dashed()
            .border_color(t.border_2)
            .rounded(px(12.))
            .child("No tasks in this goal yet. Add one, or let Claude plan them.");
    }
    let open_task = match &m.panel {
        Some(Panel::Task { r, .. }) => Some(r.clone()),
        _ => None,
    };
    // `.rows.trows`.
    let rows = |views: Vec<TaskRow>, cx: &mut Context<MainWindow>| {
        let mut list = div().flex().flex_col().rounded(px(12.)).border_1().border_color(t.border).bg(t.card).overflow_hidden();
        for (ix, row) in views.into_iter().enumerate() {
            let on = open_task.as_deref() == Some(row.r.as_str());
            list = list.child(task_row_el(t, row, ix, on, cx));
        }
        list
    };
    let mut out = div().flex().flex_col();
    // A goal with waves shows them as a rail; its shared tasks branch into the rail there.
    let by_wave = !arr(g, "waves").is_empty();
    if !tasks.is_empty() && by_wave {
        out = out.child(waves::rail(m, t, g, cx));
    } else if !tasks.is_empty() {
        out = out.child(rows(tasks.iter().enumerate().map(|(ix, task)| task_row_view(task, ix, tasks, g)).collect(), cx));
    }
    if !shared.is_empty() && !by_wave {
        // `.h3.shared-h`: 18px above, 8px below.
        out = out
            .child(h3(t, 13., "From other goals").when(!tasks.is_empty(), |d| d.mt(px(18.))).mb(px(8.)))
            .child(rows(shared.iter().map(shared_row_view).collect(), cx));
    }
    out
}

/// One `.trow`: the step number (or "·" for a shared task), the status chip, Jira and PR marks, title and meta.
fn task_row_el(t: &Theme, row: TaskRow, ix: usize, on: bool, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let hover = accent_tint(t);
    let target = row.r.clone();
    let pr_color = match row.pr_phase.as_str() {
        "merged" => t.up_fg,
        "declined" => t.muted,
        "fix" => t.down,
        "comments" => t.warn_fg,
        _ => t.accent_fg,
    };
    let fade = if row.planned { 0.85 } else { 1. };
    let id = if row.n == 0 { format!("goal-shared-{}", row.r) } else { format!("goal-task-{}", row.r) };
    div()
        .id(SharedString::from(id))
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(14.))
        .py(px(12.))
        .bg(t.card)
        .cursor_pointer()
        .when(ix > 0, |d| d.border_t_1().border_color(t.divider))
        .when(on, |d| d.bg(hover))
        .hover(move |d| d.bg(hover))
        // `.n`: the plain step number in a 28px column.
        .child(
            div()
                .flex()
                .flex_none()
                .justify_center()
                .w(px(28.))
                .opacity(fade)
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.muted)
                .child(if row.n == 0 { "·".to_string() } else { row.n.to_string() }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .opacity(fade)
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
                                .flex()
                                .flex_none()
                                .child(ico(Ico::Jira, 14., t.jira))
                                .tooltip(kit::tip(tip))
                        }))
                        .children(row.pr_text.clone().map(|txt| {
                            div()
                                .id(SharedString::from(format!("goal-pr-{}", row.r)))
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(px(3.))
                                .text_size(px(12.))
                                .text_color(pr_color)
                                .child(ico(Ico::PrOpen, 13., pr_color))
                                .child(txt)
                                .when_some(stage_icon(&row.pr_phase), |d, st| d.child(div().ml(px(3.)).child(ico(Ico::Stage(st), 13., pr_color))))
                                .tooltip(kit::tip(row.pr_tip.clone().unwrap_or_default()))
                        }))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(row.title.clone())),
                )
                .when(!row.meta.is_empty(), |d| d.child(div().text_size(px(12.5)).text_color(t.muted).truncate().child(row.meta.clone()))),
        )
        .when_some(row.tip.clone(), |d, tip| d.tooltip(kit::tip(tip)))
        .on_click(cx.listener(move |m, _, _, cx| m.open_task(target.clone(), cx)))
}

/// The QA tab: `goalQa(g.qa)`.
fn qa_tab(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Div {
    let groups = qa_groups(arr(g, "qa"));
    let mut out = div().flex().flex_col();
    for (gi, (title, items)) in groups.into_iter().enumerate() {
        // `.h3` (`.shared-h` after the first group), then `.rows.qrows` 8px below.
        out = out.child(h3(t, 13., title).when(gi > 0, |d| d.mt(px(18.))));
        let mut list = div().mt(px(8.)).flex().flex_col().rounded(px(12.)).border_1().border_color(t.border).bg(t.card).overflow_hidden();
        for (ix, c) in items.into_iter().enumerate() {
            list = list.child(qa_row(m, t, c, ix, cx));
        }
        out = out.child(list);
    }
    out
}

/// One `.qrow`: state chip and ref, what they ask, who / when / which tasks, and their comment folded.
fn qa_row(m: &MainWindow, t: &Theme, c: QaItem, ix: usize, cx: &mut Context<MainWindow>) -> Div {
    let link = |id: String, label: String, target: String, cx: &mut Context<MainWindow>| {
        let hover = t.accent;
        div()
            .id(SharedString::from(id))
            .cursor_pointer()
            .text_color(t.accent_fg)
            .hover(move |d| d.text_color(hover))
            .child(label)
            .on_click(cx.listener(move |m, _, _, cx| m.open_task(target.clone(), cx)))
    };
    let url = c.url.clone();
    let mut meta = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(4.))
        .text_size(px(13.))
        .text_color(t.muted)
        .child(format!("{} on", c.author))
        .child(
            div()
                .id(SharedString::from(format!("qa-key-{}", c.r)))
                .cursor_pointer()
                .text_color(t.accent_fg)
                .child(c.jira_key.clone())
                .on_click(move |_, _, cx| cx.open_url(&url)),
        )
        .child(format!("· {}", c.at));
    if let Some(src) = c.source_task.clone() {
        meta = meta.child("· about").child(link(format!("qa-src-{}", c.r), src.clone(), src, cx));
    }
    if let Some((tr, st, title)) = c.task.clone() {
        meta = meta.child("·").child(link(format!("qa-task-{}", c.r), tr.clone(), tr, cx)).child(format!("{st}{}", title.map(|x| format!(" · {x}")).unwrap_or_default()));
    }
    if let Some(by) = &c.by {
        meta = meta.child(format!("· by {by}"));
    }
    let open = m.goal_page.qa_open.contains(&c.r);
    let r = c.r.clone();
    let fold = c.text.clone().map(|text| {
        let hover = t.text;
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(SharedString::from(format!("qa-fold-{}", c.r)))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .text_color(t.muted)
                    .hover(move |d| d.text_color(hover))
                    .child(if open { "▾" } else { "▸" })
                    .child("Their comment")
                    .on_click(cx.listener(move |m, _, _, cx| {
                        if !m.goal_page.qa_open.remove(&r) {
                            m.goal_page.qa_open.insert(r.clone());
                        }
                        cx.notify();
                    })),
            )
            .when(open, |d| d.child(div().mt(px(8.)).text_size(px(13.)).child(note_body_keyed(t, &note_blocks(&text), &format!("qa-{}", c.r)))))
    });
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .px(px(14.))
        .py(px(12.))
        .when(ix > 0, |d| d.border_t_1().border_color(t.divider))
        // `.qrow.wait`: a 3px warn edge on the left.
        .when(c.wait, |d| d.border_l(px(3.)).border_color(t.warn))
        .child(div().flex().items_center().gap(px(8.)).child(st_chip(t, if c.wait { "warn" } else { status_tone(c.key) }, c.chip.clone())).child(pill(t.muted, t.panel_2).font_family(t.mono_font.clone()).font_weight(FontWeight::MEDIUM).child(c.r.clone())))
        .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).line_height(relative(1.45)).child(inline_el(t, &c.ask, format!("qa-ask-{}", c.r))))
        .child(meta)
        .children(fold)
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
    card_box(t)
        .child(h3(t, 13., "How this goal runs"))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(10.))
                .text_size(px(14.))
                .child("At most")
                // `.select.sm`: 34px, 0 8px, 7px radius, 13px.
                .child(
                    div()
                        .id("goal-max")
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(34.))
                        .pl(px(8.))
                        .pr(px(6.))
                        .rounded(px(7.))
                        .border_1()
                        .border_color(t.border_2)
                        .bg(t.card)
                        .text_size(px(13.))
                        .cursor_pointer()
                        .child(shown)
                        .child(ico(Ico::Chev, 12., t.text))
                        .on_click(cx.listener(move |m, e: &ClickEvent, _, cx| {
                    let p = e.position();
                    m.toggle_menu(&mk, point(p.x - px(20.), p.y + px(14.)), cx);
                        })),
                )
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

fn attachments(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Div {
    let gr = fmt::ref_of(g, "G");
    let list = arr(g, "attachments");
    let card = aside_card(t).child(h3(t, 12.5, "Attached")).children(inline_note(m, t, &format!("att:goals:{gr}")));
    if list.is_empty() {
        return card.child(div().text_size(px(13.)).text_color(t.muted).child(ATTACH_HELP));
    }
    // `.atts`: one `.att` row each (kind icon, name over meta, the actions button on hover).
    let mut atts = div().flex().flex_col();
    for (a, item) in list.iter().zip(attachments_view(list, &gr)) {
        let id = item.id;
        let url = s(a, "url").to_string();
        let name: AnyElement = if item.linked {
            let u = url.clone();
            let fg = t.accent_fg;
            div()
                .id(SharedString::from(format!("att-open-{id}")))
                .cursor_pointer()
                .text_color(fg)
                .font_weight(FontWeight::SEMIBOLD)
                .truncate()
                .hover(|s| s.underline())
                .child(item.name.clone())
                .tooltip(kit::tip(item.tip.clone()))
                .on_click(move |_, _, cx| open_attachment(&u, cx))
                .into_any_element()
        } else {
            div()
                .id(SharedString::from(format!("att-name-{id}")))
                .text_color(t.accent_fg)
                .font_weight(FontWeight::SEMIBOLD)
                .font_family(t.mono_font.clone())
                .truncate()
                .child(item.name.clone())
                .tooltip(kit::tip(item.tip.clone()))
                .into_any_element()
        };
        // `.att-m`: kind · source · age, the source (T4 / G2) as an `.att-src` chip.
        let mut meta = div().flex().items_center().text_size(px(12.)).text_color(t.muted).whitespace_nowrap().overflow_hidden();
        for (k, part) in item.meta.split(" · ").enumerate() {
            if k > 0 {
                meta = meta.child(div().flex_none().child("\u{a0}·\u{a0}"));
            }
            if item.src.as_deref() == Some(part) {
                let target = part.to_string();
                let (fg, bg) = if part.starts_with('T') { (t.accent_fg, t.accent_soft) } else { (t.goal, t.goal_soft) };
                meta = meta.child(
                    div()
                        .id(SharedString::from(format!("att-src-{id}")))
                        .flex_none()
                        .cursor_pointer()
                        .px(px(5.))
                        .rounded(px(5.))
                        .bg(bg)
                        .text_color(fg)
                        .font_family(t.mono_font.clone())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(11.5))
                        .line_height(relative(1.5))
                        .hover(|s| s.underline())
                        .child(part.to_string())
                        .tooltip(kit::tip(format!("Open {part}")))
                        .on_click(cx.listener(move |m, _, _, cx| {
                            if target.starts_with('T') {
                                m.open_task(target.clone(), cx);
                            } else {
                                m.go(Page::Goal(target.clone()), cx);
                            }
                        })),
                );
            } else {
                meta = meta.child(div().truncate().child(part.to_string()));
            }
        }
        let kind = att_kind(a);
        let (kfg, kbg) = match kind {
            "design" => (t.warn, t.warn_soft),
            "proposal" => (t.goal, t.goal_soft),
            "doc" => (t.accent_fg, t.accent_soft),
            "evidence" => (t.up, t.up_soft),
            "results" => (t.results, t.results_soft),
            _ => (t.text_2, t.col),
        };
        let menu_key = format!("{ATT_MENU}:{id}");
        let mk = menu_key.clone();
        let menu_open = m.menu_open(&menu_key).is_some();
        let group = SharedString::from(format!("att-{id}"));
        let (hover_fg, hover_line) = (t.text, t.border_2);
        atts = atts.child(
            div()
                .id(SharedString::from(format!("att-row-{id}")))
                .group(group.clone())
                .flex()
                .items_center()
                .gap(px(10.))
                .p(px(6.))
                .mx(px(-6.))
                .rounded(px(8.))
                .text_size(px(13.))
                .child(div().flex().flex_none().items_center().justify_center().size(px(26.)).rounded(px(7.)).bg(kbg).child(ico(Ico::Att(kind), 14., kfg)))
                .child(div().flex().flex_col().flex_1().min_w_0().gap(px(1.)).child(name).child(meta))
                .child(
                    div()
                        .id(SharedString::from(format!("att-menu-{id}")))
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(24.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(t.border)
                        .bg(t.card)
                        .text_color(t.muted)
                        .cursor_pointer()
                        .when(!menu_open, |d| d.opacity(0.).group_hover(group.clone(), |s| s.opacity(1.)))
                        .hover(move |s| s.text_color(hover_fg).border_color(hover_line))
                        .child("•••")
                        .text_size(px(9.))
                        .tooltip(kit::tip(format!("Actions for {}", item.name)))
                        .on_click(cx.listener(move |m, e: &ClickEvent, _, cx| {
                            let p = e.position();
                            m.toggle_menu(&mk, point(p.x - px(150.), p.y + px(14.)), cx);
                        })),
                )
                .children(m.menu_open(&menu_key).map(|at| att_menu(t, a, at, cx))),
        );
    }
    card.child(atts)
}

fn att_menu(t: &Theme, a: &Value, at: Point<Pixels>, cx: &mut Context<MainWindow>) -> AnyElement {
    let id = i(a, "id");
    let url = s(a, "url").to_string();
    let mut menu = kit::menu_box(t, 170.).id("att-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for label in att_menu_items(a) {
        let url = url.clone();
        let item = kit::menu_item(t, SharedString::from(format!("att-{id}-{label}")), label, false);
        let item = match label {
            "Open" => item.on_click(cx.listener(move |m, _, _, cx| {
                m.menu = None;
                open_attachment(&url, cx);
            })),
            _ => item.on_click(cx.listener(move |m, _, _, cx| {
                m.menu = None;
                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                m.toast("Copied", false, cx);
            })),
        };
        menu = menu.child(item);
    }
    kit::popover(at, menu)
}

fn notes(m: &mut MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Div {
    let gr = fmt::ref_of(g, "G");
    let fk = format!("gnote:{gr}");
    let folded_open = crate::prefs::get_str(FOLD_NOTES).map(|v| v == "open").unwrap_or(true);
    let head = div()
        .id("gnotes-head")
        .flex()
        .items_center()
        .cursor_pointer()
        .gap(px(8.))
        .child(div().flex().flex_none().items_center().justify_center().w(px(10.)).child(ico(if folded_open { Ico::Chev } else { Ico::Right }, 14., t.muted)))
        .child(div().flex_1().child(h3(t, 12.5, "Goal notes")))
        .on_click(cx.listener(move |_, _, _, cx| {
            crate::prefs::set(FOLD_NOTES, json!(if folded_open { "closed" } else { "open" }));
            cx.notify();
        }));
    let mut card = aside_card(t).child(head);
    if !folded_open {
        return card;
    }
    card = card.child(div().text_size(px(13.)).text_color(t.muted).child(NOTES_HELP));
    card = card.children(inline_note(m, t, &fk));
    for (title, items) in notes_view(arr(g, "notes")) {
        let mut grp = div().flex().flex_col().gap(px(4.)).child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).child(title));
        let mut ul = div().flex().flex_col().gap(px(3.));
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
                        .child(kit::link(t, SharedString::from(format!("gnote-open-{id}")), "Show all").text_size(px(12.5)).text_color(t.accent_fg).underline().on_click(cx.listener(move |m, _, window, cx| {
                            m.goal_page.note_dialog = Some(n.clone());
                            open_dialog(m, window, cx);
                        })))
                        .child(from),
                )
            } else {
                div().flex().flex_col().child(text)
            };
            ul = ul.child(div().flex().text_size(px(13.5)).child(div().flex_none().w(px(16.)).pl(px(4.)).text_color(t.text_2).child("•")).child(row.flex_1().min_w_0()));
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
    let mut col = div().flex().flex_col().gap(px(12.)).child(div().text_size(px(13.)).text_color(t.muted).child(BACKLOG_HELP));
    if let Some(bar) = &v.bar {
        let n = m.goal_page.sel.len();
        let all = n > 0 && n == pickable.len();
        let picks = pickable.clone();
        // `.bl-head` (`.on` with a selection).
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .min_h(px(36.))
            .px(px(16.))
            .when(n > 0, |d| d.pl(px(16.)).pr(px(12.)).py(px(8.)).bg(t.accent_soft).border_1().border_color(t.accent_line).rounded(px(12.)))
            .child(
                div()
                    .id("bl-pick-all")
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .text_color(t.muted)
                    .when(n > 0, |d| d.text_color(t.text).font_weight(FontWeight::SEMIBOLD).mr(px(4.)))
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
                _ => "clear",
            };
            let own_busy = busy(m, &format!("bl-bulk:{action}:"));
            let shown = if own_busy { "Sending…" } else { label };
            let el = match action {
                "clear" => kit::link(t, "bl-pick-clear", label).text_color(t.accent_fg).underline().when(!bulk_busy, |d| {
                    d.on_click(cx.listener(|m, _, _, cx| {
                        m.goal_page.sel.clear();
                        m.goal_page.anchor = None;
                        m.goal_page.notes.remove("bulk");
                        cx.notify();
                    }))
                }),
                a => act_btn(btn_sm(t, SharedString::from(format!("bl-bulk-{a}")), shown, match a { "task" => "soft", "drop" => "ghost", _ => "" }), bulk_busy)
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
        let picks_any = !pickable.is_empty();
        let mut list = div().flex().flex_col().rounded(px(12.)).border_1().border_color(t.border).bg(t.card).overflow_hidden();
        for (ix, row) in v.rows.iter().enumerate() {
            let r = row.r.clone();
            let picked = m.goal_page.sel.contains(&r);
            let on = issue_open.as_deref() == Some(r.as_str());
            let tint = accent_tint(t);
            let (pick_r, open_r) = (r.clone(), r.clone());
            let picks = pickable.clone();
            let mut acts = div().flex().flex_wrap().items_center().gap(px(6.));
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
                let style = match act {
                    "promote" => "soft",
                    "drop" => "ghost",
                    _ => "",
                };
                let bt = act_btn(btn_sm(t, SharedString::from(format!("bl-{act}-{r}")), if is_busy { "Sending…" } else { a }, style), is_busy);
                acts = acts.child(if is_busy { bt } else { bt.on_click(cx.listener(move |m, _, _, cx| issue_action(m, act, &rr, cx))) });
            }
            let state_kind = s(arr(g, "backlog").iter().find(|x| fmt::ref_of(x, "B") == r).unwrap_or(&Value::Null), "state").to_string();
            let (sfg, sbg) = match state_kind.as_str() {
                "task" => (t.goal, t.goal_soft),
                "ticket" => (t.accent_fg, t.accent_soft),
                _ => (t.muted, t.col),
            };
            // `.irow`: 12px 14px (44px on the left when rows can be picked), kind chip + title,
            // who found it, then the row's buttons or its state.
            list = list.child(
                div()
                    .id(SharedString::from(format!("goal-issue-{r}")))
                    .relative()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .py(px(12.))
                    .pr(px(14.))
                    .pl(px(if picks_any { 44. } else { 14. }))
                    .when(ix > 0, |d| d.border_t_1().border_color(t.divider))
                    .when(picked && !on, |d| d.bg(tint.opacity(0.8)))
                    .when(!on, |d| d.hover(move |s| s.bg(tint.opacity(0.6))))
                    .when(on, |d| d.bg(tint).child(div().absolute().left_0().top_0().bottom_0().w(px(3.)).bg(t.accent)))
                    .when(row.checkbox, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("bl-pick-{r}")))
                                .absolute()
                                .left(px(16.))
                                .top(px(15.))
                                .cursor_pointer()
                                .child(checkbox(t, picked, false))
                                .on_click(cx.listener(move |m, e: &ClickEvent, _, cx| {
                                    let on = !m.goal_page.sel.contains(&pick_r);
                                    pick(m, &pick_r, on, e.modifiers().shift, &picks);
                                    cx.stop_propagation();
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("bl-open-{r}")))
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .gap(px(4.))
                            .cursor_pointer()
                            .child(
                                div()
                                    .flex()
                                    .items_start()
                                    .gap(px(8.))
                                    .min_w_0()
                                    .child(kind_chip(t, s(arr(g, "backlog").iter().find(|x| fmt::ref_of(x, "B") == r).unwrap_or(&Value::Null), "kind"), row.kind.clone()).mt(px(2.)))
                                    .child(div().flex_1().min_w_0().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).line_height(relative(1.4)).child(row.title.clone())),
                            )
                            .child(div().text_size(px(12.5)).text_color(t.muted).child(row.from.clone()))
                            .on_click(cx.listener(move |m, _, _, cx| m.open_issue(open_r.clone(), cx))),
                    )
                    .child(match &row.state {
                        Some(state) if row.actions.is_empty() => div()
                            .flex()
                            .child(div().px(px(8.)).py(px(2.)).rounded(px(6.)).bg(sbg).text_color(sfg).text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).child(state.clone()))
                            .into_any_element(),
                        _ => acts.into_any_element(),
                    })
                    .children(inline_note(m, t, &format!("issue:{r}"))),
            );
        }
        col = col.child(list);
    }
    if let Some((line, button)) = v.closed {
        col = col.child(div().flex().items_center().gap(px(4.)).text_size(px(12.5)).text_color(t.muted).child(line.trim_end().to_string()).when_some(button, |d, label| {
            d.child(kit::link(t, "show-dropped", label).text_size(px(12.5)).text_color(t.accent_fg).underline().on_click(cx.listener(|m, _, _, cx| {
                m.goal_page.show_dropped = !m.goal_page.show_dropped;
                cx.notify();
            })))
        }));
    }
    col
}

/// The Deprioritize confirmation and the full-note dialog (the web's `gdeprio` / `gnote` modals).
fn dialog(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let focus = m.goal_page.dialog_focus.clone()?;
    let gr = fmt::ref_of(g, "G");
    // `.modal-head`: an 18px title and the 36px close button.
    let head = |title: AnyElement| {
        let (fg, hover, line) = (t.muted, t.text, t.border_2);
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().flex_1().text_size(px(18.)).font_weight(FontWeight::BOLD).child(title))
            .child(
                div()
                    .id("goal-dialog-close")
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(36.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(t.border)
                    .bg(t.card)
                    .text_color(fg)
                    .cursor_pointer()
                    .hover(move |s| s.text_color(hover).border_color(line))
                    .child(ico(Ico::Close, 16., fg))
                    .tooltip(kit::tip("Close"))
                    .on_click(cx.listener(|m, _, _, cx| close_dialog(m, cx))),
            )
    };
    // `.modal.narrow`: 560px, 28px padding, 18px gap, 16px radius.
    let shell = || kit::modal_box(t, 560.).border_0().rounded(px(16.)).p(px(28.)).gap(px(18.));
    let body: Div = if m.goal_page.deprio_open && !b(g, "deprioritized") {
        let (title, paras, buttons) = deprio_dialog_view(g);
        let name = fmt::opt_s(g, "name").map(str::to_string).unwrap_or_else(|| gr.clone());
        let rest = paras[0].strip_prefix(&name).unwrap_or(&paras[0]).to_string();
        let is_busy = busy(m, &format!("goal-deprio-yes::{gr}"));
        let lg = |b: Stateful<Div>| b.h(px(42.)).px(px(16.)).rounded(px(8.)).text_size(px(14.));
        let yes = act_btn(lg(md_btn(t, "goal-deprio-yes", "danger")).bg(t.down).border_color(t.down).text_color(t.on_accent).font_weight(FontWeight::SEMIBOLD).child(if is_busy { "Deprioritizing…" } else { buttons[1] }), is_busy);
        let yes = if is_busy { yes } else { yes.on_click(cx.listener(move |m, _, _, cx| set_deprioritized(m, &gr, true, cx))) };
        shell()
            .child(head(div().child(title).into_any_element()))
            .child(div().text_size(px(14.)).child(StyledText::new(format!("{name}{rest}")).with_highlights([(0..name.len(), HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() })])))
            .child(kit::help(t, paras[1].clone()))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(14.))
                    .border_t_1()
                    .border_color(t.divider)
                    .child(lg(md_btn(t, "goal-deprio-no", "")).child(buttons[0]).on_click(cx.listener(|m, _, _, cx| close_dialog(m, cx))))
                    .child(yes),
            )
    } else if let Some(n) = &m.goal_page.note_dialog {
        let title = div()
            .flex()
            .items_baseline()
            .gap(px(6.))
            .when(b(n, "pinned"), |d| d.child(div().text_color(t.goal).font_weight(FontWeight::SEMIBOLD).text_size(px(13.)).child("Pinned")))
            .child(note_dialog_title(n));
        shell().child(head(title.into_any_element())).child(note_body(t, &note_blocks(s(n, "text")), i(n, "id")))
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
/// `inlineText`'s segments as one run of text: code on a tinted ground, links that open.
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

fn note_body(t: &Theme, blocks: &[NoteBlock], id: i64) -> Div {
    note_body_keyed(t, blocks, &format!("gnb-{id}"))
}

fn note_body_keyed(t: &Theme, blocks: &[NoteBlock], key: &str) -> Div {
    let inline = |segs: &[Inline], k: String| inline_el(t, segs, k);
    let mut col = div().flex().flex_col().gap(px(10.)).text_size(px(14.)).line_height(relative(1.55)).text_color(t.text);
    for (bi, block) in blocks.iter().enumerate() {
        col = col.child(match block {
            NoteBlock::P(lines) => {
                let mut p = div().flex().flex_col();
                for (li, l) in lines.iter().enumerate() {
                    p = p.child(inline(l, format!("{key}-{bi}-{li}")));
                }
                p
            }
            NoteBlock::Ul(items) => {
                let mut ul = div().flex().flex_col().gap(px(4.)).pl(px(18.));
                for (li, it) in items.iter().enumerate() {
                    ul = ul.child(div().flex().gap(px(6.)).child(div().flex_none().text_color(t.muted).child("•")).child(div().flex_1().min_w_0().child(inline(it, format!("{key}-{bi}-{li}")))));
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
fn issue_aside(m: &mut MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    use crate::ui::issue_panel as ip;
    let r = match &m.panel {
        Some(Panel::Issue { r }) => r.clone(),
        _ => return None,
    };
    let b = m.data.issue.clone().filter(|b| fmt::ref_of(b, "B") == r);
    Some(match b {
        Some(b) => {
            let view = ip::aside_view(&b);
            ip::render_aside(t, Some((&b, &view)), None, cx)
        }
        None => {
            let err = m.data.errs.issue.clone();
            ip::render_aside(t, None, err.as_deref(), cx)
        }
    })
}

// ------------------------------------------------------------------ page

pub fn render(m: &mut MainWindow, cx: &mut Context<MainWindow>) -> AnyElement {
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
    let backlog_view = m.goal_page.backlog_view;
    let qa_view = !backlog_view && m.goal_page.qa_view && !arr(&g, "qa").is_empty();
    let items = tab_items(&g);
    let tabs = seg_counts(&t, "goal-view", &items, if backlog_view { 1 } else if qa_view { 2 } else { 0 }, |ix, item| {
        item.on_click(cx.listener(move |m, _, _, cx| {
            m.goal_page.backlog_view = ix == 1;
            m.goal_page.qa_view = ix == 2;
            // `goal-view` also closes the open issue.
            if matches!(m.panel, Some(Panel::Issue { .. })) {
                m.close_panel(cx);
            }
            cx.notify();
        }))
    });
    // `.gbody`: the section (1.5fr) and the aside (1fr), 24px apart; the aside starts level with the
    // list, under the `.gbar` (38px) and its 12px gap.
    let mut left = div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(px(12.))
        .child(div().flex().items_center().gap(px(8.)).child(tabs))
        .when(!backlog_view && !qa_view, |d| d.children(gate(m, &t, &g, cx)).child(task_rows(m, &t, &g, cx)).child(how_runs(m, &t, &g, cx)))
        .when(qa_view, |d| d.child(qa_tab(m, &t, &g, cx)))
        .when(backlog_view, |d| d.child(backlog(m, &t, &g, cx)));
    left.style().flex_grow = Some(1.5);
    left.style().flex_shrink = Some(1.);
    left.style().flex_basis = Some(relative(0.).into());
    // Tasks: Attached + Goal notes. Backlog: the picked issue (`issueAside(false)`: no move picker).
    let right_body = if backlog_view {
        issue_aside(m, &t, cx).map(|a| div().flex().flex_col().child(a))
    } else {
        Some(div().flex().flex_col().gap(px(16.)).child(attachments(m, &t, &g, cx)).child(notes(m, &t, &g, cx)))
    };
    let right = div().flex_1().min_w_0().mt(px(50.)).children(right_body);
    let overlays: Vec<AnyElement> = [start_menu(m, &t, &g, cx), dialog(m, &t, &g, cx)].into_iter().flatten().collect();
    div()
        .id("goal-page")
        .flex_1()
        .min_w_0()
        .overflow_y_scroll()
        .line_height(relative(1.5))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(22.))
                .pt(px(32.))
                .px(px(40.))
                .pb(px(48.))
                .child(header(m, &t, &g, cx))
                .child(div().flex().items_start().gap(px(24.)).child(left).child(right)),
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
                "waveRail" => {
                    let mut open = HashMap::new();
                    if b(i, "open") {
                        for w in arr(goal, "waves") {
                            open.insert(format!("G1:{}", num_text(&w["wave"])), true);
                        }
                        open.insert("G1:none".to_string(), true);
                    }
                    let wt = |x: &waves::WTask| json!({"ref": x.r, "title": x.title, "planned": x.planned, "jira": x.jira.as_ref().map(|j| j.1.clone()),
                        "pr": x.pr_text.as_ref().map(|p| format!("{p}{}", x.pr_label.as_ref().map(|l| format!(" {l}")).unwrap_or_default())),
                        "when": x.when, "facts": x.facts.iter().map(|(c, t)| json!({"cls": c, "text": t})).collect::<Vec<_>>(),
                        "term": x.term.as_ref().map(|s| format!("#/sessions?s={s}"))});
                    let items: Vec<Value> = waves::rail_view(goal, &open).iter().map(|it| match it {
                        waves::Item::Wave { wave, state, open, name, count, stop, gate, tasks } => json!({"kind": "wave", "state": state, "open": open,
                            "n": format!("Wave {wave}"), "name": name, "count": count.iter().map(|(c, t)| json!({"cls": c, "text": t})).collect::<Vec<_>>(),
                            "stop": stop, "gate": gate.as_ref().map(|g| json!({"cls": g.cls, "text": g.text, "button": g.button.as_ref().map(|(a, l)| json!({"act": a, "label": l}))})),
                            "tasks": if *open { tasks.iter().map(wt).collect::<Vec<_>>() } else { vec![] }}),
                        waves::Item::Post { open, tasks } => json!({"kind": "post", "state": "loose", "open": open, "n": null, "name": "Post", "count": [], "stop": false, "gate": null,
                            "tasks": if *open { tasks.iter().map(wt).collect::<Vec<_>>() } else { vec![] }}),
                        waves::Item::Ref { done, badge, home, landed, task } => json!({"kind": "ref", "done": done, "badge": badge, "badge_link": home.as_ref().map(|h| format!("#/goals/{h}")),
                            "dashed": !landed, "line": if *landed { "var(--up)" } else { "var(--goal-line)" }, "task": wt(task)}),
                        waves::Item::Finish { done, text } => json!({"kind": "finish", "done": done, "text": text}),
                    }).collect();
                    json!({"rail": !arr(goal, "tasks").is_empty() && !arr(goal, "waves").is_empty(), "shared_list": !arr(goal, "shared").is_empty() && arr(goal, "waves").is_empty(), "items": items})
                }
                "qaTab" => {
                    let items = tab_items(goal);
                    json!({
                        "tabs": items.iter().map(|(l, n, _)| format!("{l} {n}")).collect::<Vec<_>>(),
                        "hot": items.get(2).map(|x| x.2).unwrap_or(false),
                        "selected": "qa",
                        "groups": qa_groups(arr(goal, "qa")).into_iter().map(|(title, items)| json!({"title": title, "items": items.iter().map(|c| {
                            let ask = c.ask.iter().map(|x| match x { Inline::Text(s) | Inline::Code(s) | Inline::Link(s) => s.clone() }).collect::<Vec<_>>().join(" ");
                            let mut meta = vec![format!("{} on {}", c.author, c.jira_key), c.at.clone()];
                            meta.extend(c.source_task.as_ref().map(|s| format!("about {s}")));
                            meta.extend(c.task.as_ref().map(|(r, st, title)| format!("{r} {st}{}", title.as_ref().map(|x| format!(" · {x}")).unwrap_or_default())));
                            meta.extend(c.by.as_ref().map(|b| format!("by {b}")));
                            let mut opens: Vec<String> = c.source_task.iter().cloned().collect();
                            opens.extend(c.task.as_ref().map(|x| x.0.clone()));
                            json!({"wait": c.wait, "chip": c.chip, "chip_cls": format!("st-{}", c.key), "ref": c.r,
                                   "ask": ask.split_whitespace().collect::<Vec<_>>().join(" "), "meta": meta.join(" · "),
                                   "link": c.url, "opens": opens, "comment": c.text.is_some()})
                        }).collect::<Vec<_>>()})).collect::<Vec<_>>(),
                    })
                }
                "taskRows" => {
                    let tasks = arr(goal, "tasks");
                    let h = how_runs_view(goal);
                    let mut out = json!({
                        "empty": (tasks.is_empty() && arr(goal, "shared").is_empty()).then_some("No tasks in this goal yet. Add one, or let Claude plan them."),
                        "rows": tasks.iter().enumerate().map(|(ix, t)| {
                            let r = task_row_view(t, ix, tasks, goal);
                            json!({"n": r.n.to_string(), "chip": r.chip, "title": r.title, "meta": r.meta, "jira_tip": r.jira_tip, "pr_text": r.pr_text, "pr_tip": r.pr_tip, "planned": r.planned})
                        }).collect::<Vec<_>>(),
                        "max_options": h.options.iter().map(|(_, l)| l.clone()).collect::<Vec<_>>(),
                        "max_selected": h.selected, "run_in_order": h.run_in_order, "auto_close": h.auto_close,
                    });
                    if !arr(goal, "shared").is_empty() {
                        out["shared"] = json!(arr(goal, "shared").iter().map(|t| {
                            let r = shared_row_view(t);
                            json!({"chip": r.chip, "title": r.title, "meta": r.meta, "tip": r.tip, "pr_text": r.pr_text, "planned": r.planned})
                        }).collect::<Vec<_>>());
                    }
                    out
                }
                "gate" => match gate_view(goal) {
                    Some((label, text, (act, button))) => json!({"label": label, "text": text, "buttons": [{"act": act, "label": button}]}),
                    None => json!({"label": null, "text": null, "buttons": []}),
                },
                "deprioDialog" => {
                    let (title, body, buttons) = deprio_dialog_view(goal);
                    json!({"title": title, "body": body, "buttons": buttons})
                }
                "notes" => json!({"help": NOTES_HELP, "groups": note_items_json(notes_view(arr(i, "notes"))), "add_button": false}),
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
        for (name, action) in [("bulk make tasks", "task"), ("bulk tickets", "ticket"), ("bulk drop", "drop")] {
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

    #[::core::prelude::v1::test]
    fn review_log_issues_say_so() {
        let from = |src: &str| issue_from(&json!({"source": src, "found_by_name": "Sam", "created_at": ""}), true);
        assert_eq!(from("review_log"), "From the Review log");
        assert_eq!(crate::ui::board::issue_from(&json!({"source": "review_log", "created_at": ""})), "From the Review log");
        assert_eq!(from("you"), "Added by you");
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
    fn issues_send_what_the_web_sent(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = goal_window(cx);
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
