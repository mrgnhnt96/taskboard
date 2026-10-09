//! The Board page (the web board's `renderBoard`): the sessions strip, the goal bar and the
//! Backlog / Queued / Working / Needs you / Done columns. The goals rail that filters it lives in
//! the sidebar.
//!
//! What each part shows is worked out by the pure `*_vm` functions below (tested one-to-one
//! against the web board's own output, `parity/gen/board.mjs`); the render code only lays it
//! out. Actions are named functions (`start_new`, `close_done`, `set_done_window`, `goal_pick`,
//! `end_rename`, `keep_issue`) so tests can drive them and check the requests.
use crate::app::{Filters, MainWindow, Page, Panel};
use crate::fmt::{self, arr, b, i, obj, opt_s, s};
use crate::theme::Theme;
use crate::ui::kit::{self, KeyOutcome};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const STRIP_MAX: usize = 6;
const COL_MIN: f32 = 220.;
const COL_GAP: f32 = 14.;
/// `DONE_WINDOWS`: id, menu label, "Nothing in the …" phrase.
const DONE_WINDOWS: [(&str, &str, &str); 3] = [("24h", "Last 24 hours", "last 24 hours"), ("7d", "Last 7 days", "last 7 days"), ("all", "Everything", "whole history")];
const MENU_DONE: &str = "board-done";
/// The web board waits this long after a click on a terminal's name, in case it's a double-click.
const DOUBLE_CLICK_WAIT: Duration = Duration::from_millis(260);
/// How long a rename's ok / failed flash stays (`FLASH_MS`).
const FLASH_OK: Duration = Duration::from_millis(1200);
const FLASH_BAD: Duration = Duration::from_millis(1800);
/// How long a column note stays (`setNote`): 5 s, 15 s for an error.
const NOTE_OK: Duration = Duration::from_secs(5);
const NOTE_ERR: Duration = Duration::from_secs(15);

#[derive(Default)]
pub struct State {
    /// Issue refs to keep in the Backlog column after the reader changed them (`?keep=`).
    pub keep: Vec<String>,
    /// The terminal being renamed in the strip, its name field, and the focus-out watch that
    /// saves it (dropped with the edit).
    rename: Option<(String, kit::Input, Subscription)>,
    /// Renames sent and not yet confirmed: id → the new name (`S.renameWatch`).
    rename_watch: HashMap<String, String>,
    /// A finished rename's flash: id → (ok, name, why it failed, since) (`S.renameFlash`).
    flash: HashMap<String, Flash>,
    /// Bumped by every click on a terminal's name; a pending single click opens only if unchanged.
    name_click: u64,
    /// The Done column's note (`note('done-col')`): text, error, when.
    done_note: Option<(String, bool, Instant)>,
}

#[derive(Clone, Debug)]
pub struct Flash {
    pub ok: bool,
    pub to: String,
    pub why: String,
    pub at: Instant,
}

// ================================================================== view models

/// `nameView`: what a terminal's name shows (and its tooltip) while renaming, after a rename, or
/// normally.
#[derive(Clone, Debug, PartialEq)]
pub struct NameView {
    pub text: String,
    pub renaming: bool,
    /// The flash after a rename: Some(true) took, Some(false) failed.
    pub flash: Option<bool>,
    pub title: String,
}

pub fn name_view(x: &Value, flash: Option<&Flash>, hint: &str) -> NameView {
    if let Some(r) = opt_s(x, "renaming") {
        return NameView { text: r.into(), renaming: true, flash: None, title: "Renaming in Midna…".into() };
    }
    if let Some(f) = flash.filter(|f| !f.ok) {
        return NameView { text: f.to.clone(), renaming: false, flash: Some(false), title: format!("Rename failed: {}", f.why) };
    }
    let title = match opt_s(x, "rename_error") {
        Some(e) if flash.is_none() => format!("Last rename failed: {e}"),
        _ => s(x, "name").to_string(),
    };
    let text = opt_s(x, "name").unwrap_or(s(x, "id")).to_string();
    NameView { text, renaming: false, flash: flash.map(|_| true), title: format!("{title}{hint}") }
}

/// The web's `SESS` labels; an unknown status reads as idle.
fn sess_status(status: &str) -> (&'static str, &'static str) {
    match status {
        "working" => ("working", "Working"),
        "needs" => ("needs", "Needs you"),
        "offline" => ("offline", "No network"),
        "waiting" => ("waiting", "Waiting"),
        "gone" => ("gone", "Gone"),
        _ => ("idle", "Idle"),
    }
}

/// `sessCard`.
#[derive(Clone, Debug)]
pub struct SessCardVm {
    pub id: String,
    pub name: NameView,
    pub status: &'static str,
    pub state_label: String,
    pub project: String,
    /// Tooltip on the project: its folder.
    pub project_title: String,
    pub sub: Option<String>,
    /// The task the sub line opens (when the terminal has one).
    pub task_ref: Option<String>,
    /// "Compacting since 3:05 PM".
    pub compacting: Option<String>,
}

#[cfg(test)]
impl SessCardVm {
    pub fn text(&self) -> String {
        join([self.name.text.as_str(), &self.state_label, &self.project, self.compacting.as_deref().unwrap_or(""), self.sub.as_deref().unwrap_or("")])
    }
    pub fn acts(&self) -> Vec<&'static str> {
        let mut a = vec!["open-session"];
        if self.task_ref.is_some() {
            a.push("open-task");
        }
        a
    }
}

/// `sessionsHtml`.
#[derive(Clone, Debug)]
pub struct StripVm {
    /// "Showing 6 of 8".
    pub more: Option<String>,
    pub cards: Vec<SessCardVm>,
    /// Loading, can't load, or none yet.
    pub empty: Option<String>,
}

#[cfg(test)]
impl StripVm {
    pub fn text(&self) -> String {
        let mut parts = vec!["Sessions".to_string()];
        parts.extend(self.more.clone());
        parts.extend(self.cards.iter().map(SessCardVm::text));
        parts.extend(self.empty.clone());
        join(parts.iter().map(String::as_str))
    }
    pub fn acts(&self) -> Vec<&'static str> {
        self.cards.iter().flat_map(SessCardVm::acts).collect()
    }
}

pub fn strip_vm(state: &Value, project: &str, down: bool, flashes: &HashMap<String, Flash>) -> StripVm {
    if state.is_null() {
        return StripVm { more: None, cards: vec![], empty: Some(if down { "Can’t load the sessions." } else { "Loading…" }.into()) };
    }
    let order = |x: &Value| match sess_status(s(x, "status")).0 {
        "needs" | "offline" => 0,
        "working" | "waiting" => 1,
        "gone" => 3,
        _ => 2,
    };
    let mut list: Vec<&Value> = arr(state, "sessions").iter().filter(|x| project == "all" || s(x, "project") == project).collect();
    // `Array.prototype.sort` is stable, like `sort_by_key`.
    list.sort_by_key(|x| order(x));
    let total = list.len();
    let cards: Vec<SessCardVm> = list
        .into_iter()
        .take(STRIP_MAX)
        .map(|x| {
            let (status, label) = sess_status(s(x, "status"));
            let state_label = match i(x, "background") {
                n if status == "waiting" && n > 0 => format!("Waiting on {n}"),
                _ => label.to_string(),
            };
            let id = s(x, "id").to_string();
            let sub = opt_s(x, "task_title").map(str::to_string);
            let task_ref = opt_s(x, "task_ref").map(str::to_string);
            SessCardVm {
                name: name_view(x, flashes.get(&id), " · double-click to rename"),
                id,
                status,
                state_label,
                project: s(x, "project").to_string(),
                project_title: s(x, "project_path").to_string(),
                // With a task the line is the (clickable) title even when the title is empty.
                sub: if task_ref.is_some() { Some(sub.unwrap_or_default()) } else { sub },
                task_ref,
                compacting: fmt::compacting(x),
            }
        })
        .collect();
    let empty = total.eq(&0).then(|| {
        let where_ = if project == "all" { String::new() } else { format!(" in {project}") };
        format!("No Midna terminals{where_} yet. They show up here once Midna lists them.")
    });
    StripVm { more: (total > cards.len()).then(|| format!("Showing {} of {total}", cards.len())), cards, empty }
}

/// The goal summary counts the board uses (`goalCounts` on a goal without its task list).
fn goal_counts(g: &Value) -> (i64, i64, i64) {
    (i(g, "total"), i(g, "done"), i(g, "active"))
}

/// `goalBarHtml`.
#[derive(Clone, Debug)]
pub struct GoalBarVm {
    pub r: String,
    pub name: String,
    pub summary: String,
}

#[cfg(test)]
impl GoalBarVm {
    pub fn text(&self) -> String {
        join([self.name.as_str(), &self.summary, "Open goal"])
    }
    pub fn acts(&self) -> Vec<&'static str> {
        vec!["goal-pick"]
    }
}

/// Non-archived goals: the full list once `GET /goals` answered, else the state's (`allGoals`).
fn all_goals<'a>(goals: &'a [Value], state: &'a Value) -> Vec<&'a Value> {
    let src = if goals.is_empty() { arr(state, "goals") } else { goals };
    src.iter().filter(|g| !b(g, "archived")).collect()
}

fn goal_by_ref<'a>(goals: &'a [Value], state: &'a Value, r: &str) -> Option<&'a Value> {
    all_goals(goals, state).into_iter().find(|g| fmt::ref_of(g, "G") == r)
}

pub fn goal_bar_vm(state: &Value, goals: &[Value], f: &Filters) -> Option<GoalBarVm> {
    if f.goal == "all" {
        return None;
    }
    let g = goal_by_ref(goals, state, &f.goal)?;
    let (n, done, active) = goal_counts(g);
    let needs = i(g, "needs");
    let mut bits = vec![format!("{done} of {n} done")];
    if needs > 0 {
        bits.push(format!("{needs} need{} you", if needs == 1 { "s" } else { "" }));
    }
    if active - needs > 0 {
        bits.push(format!("{} working", active - needs));
    }
    if i(g, "open_issues") > 0 {
        bits.push(format!("{} in the backlog", i(g, "open_issues")));
    }
    if b(g, "deprioritized") {
        bits.push("Deprioritized".into());
    } else if b(g, "paused") {
        bits.push("Paused".into());
    }
    Some(GoalBarVm { r: fmt::ref_of(g, "G"), name: s(g, "name").to_string(), summary: bits.join(" · ") })
}

/// A chip on a card: its text and the web's class (which picks its colors).
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    pub text: String,
    pub cls: String,
}

/// `taskCard`.
#[derive(Clone, Debug)]
pub struct TaskCardVm {
    pub r: String,
    pub goal_ref: Option<String>,
    pub goal_name: Option<String>,
    pub when: String,
    pub when_title: String,
    pub title: String,
    pub high: bool,
    /// Project, Jira key, PR, Failed, Terminal lost, lent devices, bits (in that order).
    pub chips: Vec<Chip>,
    pub who: Option<String>,
    pub needs: bool,
    /// Queued/planned, not in a goal, not starting: drag it to Working to start it.
    pub draggable: bool,
    pub selected: bool,
}

#[cfg(test)]
impl TaskCardVm {
    pub fn text(&self) -> String {
        let mut p: Vec<&str> = Vec::new();
        if let Some(g) = &self.goal_ref {
            p.push(g);
            p.push("·");
        }
        p.extend([self.r.as_str(), &self.when, &self.title]);
        if self.high {
            p.push("High");
        }
        p.extend(self.chips.iter().map(|c| c.text.as_str()));
        p.extend(self.who.as_deref());
        join(p)
    }
    /// The chip classes in page order (the High chip sits in the title row, before the rest).
    pub fn chip_classes(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.high {
            v.push("high".to_string());
        }
        v.extend(self.chips.iter().map(|c| c.cls.clone()));
        v
    }
}

/// `checksOf`: a PR's checks as (label, class).
pub fn checks_of(pr: &Value) -> (&'static str, &'static str) {
    match s(pr, "checks").to_lowercase().as_str() {
        "pass" => ("Passed", "b-pass"),
        "fail" => ("Failed", "b-fail"),
        "pending" => ("Running", "b-run"),
        "none" => ("No checks", "b-unk"),
        _ => ("Unknown", "b-unk"),
    }
}

/// `startsByHand`. A task waiting in a terminal's line starts by hand too, goal or not: Start takes it
/// out of the line.
pub fn starts_by_hand(x: &Value) -> bool {
    matches!(s(x, "status"), "queued" | "planned") && (obj(x, "goal").is_none() || !x["line"].is_null()) && !b(x, "starting")
}

pub fn task_card_vm(x: &Value, selected: Option<&str>) -> TaskCardVm {
    let r = fmt::ref_of(x, "T");
    let goal = obj(x, "goal");
    let mut chips = vec![Chip { text: s(x, "project").to_string(), cls: "repo".into() }];
    if let Some(key) = obj(x, "jira").and_then(|j| opt_s(j, "key")) {
        chips.push(Chip { text: key.to_string(), cls: "jira".into() });
    }
    if let Some(pr) = obj(x, "pr").filter(|p| p.get("num").is_some_and(|n| !n.is_null())) {
        let (label, cls) = checks_of(pr);
        let num = pr["num"].as_i64().map(|n| n.to_string()).unwrap_or_else(|| pr["num"].as_str().unwrap_or("").to_string());
        chips.push(Chip { text: format!("PR #{num} · {label}"), cls: cls.into() });
    }
    if b(x, "failed") {
        chips.push(Chip { text: "Failed".into(), cls: "bad".into() });
    }
    if b(x, "lost") {
        chips.push(Chip { text: "Terminal lost".into(), cls: "bad".into() });
    }
    if let Some(c) = fmt::compacting(x) {
        chips.push(Chip { text: c, cls: "compact".into() });
    }
    // Where it waits its turn: "To resume in Term 3" or "Queued in Term 3".
    if let Some(l) = opt_s(&x["line"], "label") {
        chips.push(Chip { text: l.to_string(), cls: "line".into() });
    }
    // The devices lent to it, and its bits (⚑, warn while a backend one isn't made).
    for d in arr(&x["devices"], "lent").iter().filter_map(|d| d.as_str()) {
        chips.push(Chip { text: d.to_string(), cls: "device".into() });
    }
    for bit in arr(x, "bits") {
        chips.push(Chip { text: format!("⚑ {}", s(bit, "name")), cls: if b(bit, "waiting") { "bit-wait" } else { "bit" }.into() });
    }
    TaskCardVm {
        goal_ref: goal.map(|g| fmt::ref_of(g, "G")),
        goal_name: goal.map(|g| s(g, "name").to_string()),
        when: fmt::ago(opt_s(x, "when").or(opt_s(x, "updated_at")).unwrap_or("")),
        when_title: fmt::when_line(x),
        title: s(x, "title").to_string(),
        high: s(x, "priority") == "high",
        chips,
        who: opt_s(x, "who").map(str::to_string),
        needs: s(x, "status") == "needs",
        draggable: starts_by_hand(x),
        selected: selected == Some(r.as_str()),
        r,
    }
}

/// `issueState`: (short label, class) for an issue that isn't open.
pub fn issue_state(x: &Value) -> Option<(String, &'static str)> {
    match s(x, "state") {
        "task" => Some(("Now a task".into(), "st-task")),
        "ticket" => Some((opt_s(x, "jira_key").unwrap_or("Ticket asked for").to_string(), "st-ticket")),
        "drop" => Some(("Won’t do".into(), "st-drop")),
        _ => None,
    }
}

/// `kindLabel`.
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

/// `issueGoalId`.
pub fn issue_goal_id(x: &Value) -> Option<i64> {
    x.get("goal_id").and_then(Value::as_i64).or_else(|| obj(x, "goal").and_then(|g| g.get("id")).and_then(Value::as_i64))
}

/// `issueFrom(b, false)`: who reported it and when.
pub fn issue_from(x: &Value) -> String {
    let found = x.get("found_by_task").is_some_and(|v| !v.is_null());
    let who = match s(x, "source") {
        "you" => "Added by you".to_string(),
        "" if opt_s(x, "found_by_name").is_none() && !found => "Added by you".to_string(),
        "answer" => "From your answer".to_string(),
        src if fmt::from_review_log(src) => "From the Review log".to_string(),
        _ => opt_s(x, "found_by_name").unwrap_or("Found by a terminal").to_string(),
    };
    [who, fmt::hhmm(s(x, "created_at"))].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// `issueCard`.
#[derive(Clone, Debug)]
pub struct IssueCardVm {
    pub r: String,
    pub kind: String,
    pub kind_cls: String,
    pub state: Option<(String, &'static str)>,
    pub when: String,
    pub when_title: String,
    pub title: String,
    /// "G3", or "Not in a goal".
    pub goal: String,
    pub goal_title: String,
    pub in_goal: bool,
    pub selected: bool,
}

#[cfg(test)]
impl IssueCardVm {
    pub fn text(&self) -> String {
        let mut p = vec![self.kind.as_str()];
        if let Some((l, _)) = &self.state {
            p.push(l);
        }
        p.extend([self.when.as_str(), &self.title, &self.goal]);
        join(p)
    }
    pub fn chip_classes(&self) -> Vec<String> {
        let mut v = vec![self.kind_cls.clone()];
        v.extend(self.state.as_ref().map(|(_, c)| c.to_string()));
        v
    }
}

pub fn issue_card_vm(x: &Value, goals: &[Value], state: &Value, selected: Option<&str>) -> IssueCardVm {
    let r = fmt::ref_of(x, "B");
    let gid = issue_goal_id(x);
    let goal_title = match obj(x, "goal").and_then(|g| opt_s(g, "name")) {
        Some(n) => n.to_string(),
        None => gid.and_then(|id| all_goals(goals, state).into_iter().find(|g| i(g, "id") == id)).map(|g| s(g, "name").to_string()).unwrap_or_default(),
    };
    let state_info = if matches!(s(x, "state"), "" | "open") { None } else { issue_state(x) };
    IssueCardVm {
        kind: kind_label(s(x, "kind")),
        kind_cls: format!("k-{}", s(x, "kind")),
        state: state_info,
        when: fmt::ago(s(x, "created_at")),
        when_title: fmt::cap(&issue_from(x)),
        title: s(x, "title").to_string(),
        goal: gid.map(|id| format!("G{id}")).unwrap_or_else(|| "Not in a goal".into()),
        goal_title,
        in_goal: gid.is_some(),
        selected: selected == Some(r.as_str()),
        r,
    }
}

/// A card in a column.
#[derive(Clone, Debug)]
pub enum CardVm {
    Task(TaskCardVm),
    Issue(IssueCardVm),
}

/// The link under a column's cards.
#[derive(Clone, Debug, PartialEq)]
pub enum More {
    /// "N more on the Backlog page".
    Backlog(String),
    /// "N older tasks hidden · show more" → that Done window.
    Done(String, &'static str),
}

/// `colShell` + what `columnsHtml` puts in it.
#[derive(Clone, Debug)]
pub struct ColVm {
    pub key: &'static str,
    pub name: &'static str,
    pub count: Option<i64>,
    /// The Backlog column's "See all".
    pub see_all: bool,
    /// The Done column's options menu.
    pub done_menu: bool,
    /// The column's note (`note('done-col')`): text, error.
    pub note: Option<(String, bool)>,
    pub cards: Vec<CardVm>,
    pub more: Option<More>,
    pub empty: Option<String>,
}

#[cfg(test)]
impl ColVm {
    pub fn text(&self) -> String {
        let mut p: Vec<String> = vec![self.name.into()];
        p.extend(self.count.map(|n| n.to_string()));
        if self.see_all {
            p.push("See all".into());
        }
        p.extend(self.note.as_ref().map(|(t, _)| t.clone()));
        for c in &self.cards {
            p.push(match c {
                CardVm::Task(t) => t.text(),
                CardVm::Issue(x) => x.text(),
            });
        }
        if self.cards.is_empty() {
            p.extend(self.empty.clone());
        }
        match &self.more {
            Some(More::Backlog(t)) => {
                // The web puts the Backlog link before "Nothing here".
                if self.cards.is_empty() {
                    p.pop();
                    p.push(t.clone());
                    p.extend(self.empty.clone());
                } else {
                    p.push(t.clone());
                }
            }
            Some(More::Done(t, _)) => p.push(t.clone()),
            None => {}
        }
        join(p.iter().map(String::as_str))
    }
    pub fn acts(&self) -> Vec<&'static str> {
        let mut a = Vec::new();
        if self.done_menu {
            a.push("done-menu");
        }
        for c in &self.cards {
            a.push(match c {
                CardVm::Task(_) => "open-task",
                CardVm::Issue(_) => "open-issue",
            });
        }
        if matches!(self.more, Some(More::Done(..))) {
            a.push("done-window");
        }
        a
    }
}

impl ColVm {
    /// Only Working takes dropped cards.
    pub fn takes_drop(&self) -> bool {
        self.key == "working"
    }
}

/// `inFilter`.
fn in_filter(f: &Filters, project: &str, goal_ref: Option<String>) -> bool {
    (f.project == "all" || project == f.project) && (f.goal == "all" || goal_ref.as_deref() == Some(f.goal.as_str()))
}

const COLS: [(&str, &str); 5] = [("backlog", "Backlog"), ("queued", "Queued"), ("working", "Working"), ("needs", "Needs you"), ("done", "Done")];

/// `columnsHtml`. `sel_task` / `sel_issue` are the open task and (on the board) issue.
pub fn columns_vm(
    state: &Value,
    goals: &[Value],
    f: &Filters,
    down: bool,
    sel_task: Option<&str>,
    sel_issue: Option<&str>,
    note: Option<(String, bool)>,
) -> Vec<ColVm> {
    let shell = |key: &'static str, name: &'static str| ColVm {
        key,
        name,
        count: None,
        see_all: false,
        done_menu: false,
        note: None,
        cards: vec![],
        more: None,
        empty: None,
    };
    if state.is_null() {
        let msg = if down { "Can’t load the board." } else { "Loading…" };
        return COLS.iter().map(|(k, n)| ColVm { empty: Some(msg.into()), ..shell(k, n) }).collect();
    }
    let cols = &state["columns"];
    COLS.iter()
        .map(|&(k, n)| {
            if k == "backlog" {
                let issues: Vec<&Value> = arr(cols, "backlog").iter().filter(|x| in_filter(f, s(x, "project"), issue_goal_id(x).map(|id| format!("G{id}")))).collect();
                let shown_open = issues.iter().filter(|x| matches!(s(x, "state"), "" | "open")).count() as i64;
                let open = cols.get("backlog_open").and_then(Value::as_i64).unwrap_or(shown_open);
                let more = open - shown_open;
                return ColVm {
                    count: Some(open),
                    see_all: true,
                    cards: issues.iter().map(|x| CardVm::Issue(issue_card_vm(x, goals, state, sel_issue))).collect(),
                    more: (more > 0).then(|| More::Backlog(format!("{more} more on the Backlog page"))),
                    empty: Some("Nothing here".into()),
                    ..shell(k, n)
                };
            }
            let tasks: Vec<&Value> = arr(cols, k).iter().filter(|x| in_filter(f, s(x, "project"), obj(x, "goal").map(|g| fmt::ref_of(g, "G")))).collect();
            let cards: Vec<CardVm> = tasks.iter().map(|x| CardVm::Task(task_card_vm(x, sel_task))).collect();
            if k == "done" {
                let hidden = i(&state["counts"], "done_hidden");
                let cur = if f.done.is_empty() { "24h" } else { f.done.as_str() };
                let win = DONE_WINDOWS.iter().find(|(id, _, _)| *id == cur).unwrap_or(&DONE_WINDOWS[0]);
                let next = if cur == "24h" { "7d" } else { "all" };
                return ColVm {
                    count: Some(cards.len() as i64),
                    done_menu: true,
                    note: note.clone(),
                    empty: Some(if hidden > 0 { format!("Nothing in the {}", win.2) } else { "Nothing here".into() }),
                    more: (hidden > 0).then(|| More::Done(format!("{} hidden · show more", fmt::plural(hidden, "older task", "older tasks")), next)),
                    cards,
                    ..shell(k, n)
                };
            }
            ColVm { count: Some(cards.len() as i64), empty: Some("Nothing here".into()), cards, ..shell(k, n) }
        })
        .collect()
}

/// `doneMenuButton` (open): the window items (current one ticked) and whether "Close their
/// terminals" is offered.
pub fn done_menu_vm(state: &Value, f: &Filters) -> (Vec<(String, &'static str)>, bool) {
    let cur = if f.done.is_empty() { "24h" } else { f.done.as_str() };
    let items = DONE_WINDOWS.iter().map(|(id, l, _)| (if *id == cur { format!("✓ {l}") } else { l.to_string() }, *id)).collect();
    let has_done = !state.is_null() && !arr(&state["columns"], "done").is_empty();
    (items, has_done)
}

/// `loadState`'s query: the goal goes as its number. (For `app.rs`'s refresh to call.)
#[cfg_attr(not(test), allow(dead_code))]
pub fn state_query(f: &Filters, keep: &[String]) -> Vec<(&'static str, String)> {
    let goal = if f.goal == "all" { "all".to_string() } else { f.goal.trim_start_matches(|c: char| c.is_ascii_alphabetic()).to_string() };
    let mut q = vec![
        ("project", if f.project.is_empty() { "all".to_string() } else { f.project.clone() }),
        ("goal", goal),
        ("done", if f.done.is_empty() { "24h".to_string() } else { f.done.clone() }),
    ];
    if !keep.is_empty() {
        q.push(("keep", keep.join(",")));
    }
    q
}

/// Words joined the way the reader sees them: non-empty parts separated by single spaces.
#[cfg(test)]
fn join<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts.into_iter().flat_map(str::split_whitespace).collect::<Vec<_>>().join(" ")
}

// ================================================================== actions

/// Drop on Working (`drop`): start it in a new Midna terminal.
pub fn start_new(m: &mut MainWindow, r: &str, cx: &mut Context<MainWindow>) {
    let r = r.to_string();
    m.post(format!("tasks/{r}/start"), json!({"mode": "new"}), cx, move |m, _, cx| m.toast(format!("Starting {r}"), false, cx));
}

/// `close-done`: close every done task's still-open terminal; the answer shows in the Done
/// column's note (`sentNote`).
pub fn close_done(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    m.menu = None;
    m.board.done_note = None;
    m.post_or(
        "done/close-terminals",
        json!({}),
        cx,
        |m, _, cx| {
            let text = sent_note(m.state());
            set_done_note(m, text, false, cx);
        },
        |m, e, cx| set_done_note(m, e.message, true, cx),
    );
    cx.notify();
}

/// `sentNote`.
pub fn sent_note(state: &Value) -> String {
    if state["midna"]["up"] == Value::Bool(false) { "Saved. It runs once Midna is back.".into() } else { "Sent to Midna".into() }
}

fn set_done_note(m: &mut MainWindow, text: String, err: bool, cx: &mut Context<MainWindow>) {
    let at = Instant::now();
    m.board.done_note = Some((text, err, at));
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(if err { NOTE_ERR } else { NOTE_OK }).await;
        let _ = this.update(cx, |m, cx| {
            if m.board.done_note.as_ref().is_some_and(|(_, _, t)| *t == at) {
                m.board.done_note = None;
                cx.notify();
            }
        });
    })
    .detach();
    cx.notify();
}

/// `done-window`: show the Done column from a window (saved with the filters). The kept issues
/// stay, as on the web.
pub fn set_done_window(m: &mut MainWindow, w: &str, cx: &mut Context<MainWindow>) {
    m.menu = None;
    let mut f = m.filters.clone();
    f.done = w.to_string();
    m.set_filters(f, cx);
}

/// `goal-pick`: show one goal (its project too), or `all`; picking the shown goal again shows
/// every goal. Clears the kept issues.
pub fn goal_pick(m: &mut MainWindow, arg: &str, cx: &mut Context<MainWindow>) {
    let mut f = m.filters.clone();
    f.goal = if arg != "all" && arg == f.goal { "all".into() } else { arg.to_string() };
    if f.goal != "all" {
        if let Some(p) = goal_by_ref(&m.data.goals, m.state(), &f.goal).map(|g| s(g, "project").to_string()).filter(|p| !p.is_empty()) {
            f.project = p;
        }
    }
    if f.goal == "all" {
        f.project = "all".into();
    }
    m.board.keep.clear();
    m.set_filters(f, cx);
}

/// `keepIssue`: keep an issue the reader just changed in the Backlog column (board page only).
/// The issue panel's actions call it.
#[cfg_attr(not(test), allow(dead_code))]
pub fn keep_issue(m: &mut MainWindow, r: &str) {
    if m.page == Page::Board && !m.board.keep.iter().any(|k| k == r) {
        m.board.keep.push(r.to_string());
    }
}

/// `open-session`: that terminal on the Sessions page.
pub fn open_session(m: &mut MainWindow, id: &str, cx: &mut Context<MainWindow>) {
    m.sessions.selected = Some(id.to_string());
    m.data.session = None;
    m.go(Page::Sessions, cx);
    m.refresh(cx);
}

/// Every copy of a terminal the page holds (`sessionCopies`).
fn session_copies<'a>(m: &'a mut MainWindow, id: &str) -> Vec<&'a mut Value> {
    let mut out = Vec::new();
    let data = &mut m.data;
    if let Some(list) = data.state.as_mut().and_then(|v| v.get_mut("sessions")).and_then(Value::as_array_mut) {
        out.extend(list.iter_mut().filter(|x| s(x, "id") == id));
    }
    if let Some(list) = data.sessions.as_mut().and_then(|v| v.get_mut("sessions")).and_then(Value::as_array_mut) {
        out.extend(list.iter_mut().filter(|x| s(x, "id") == id));
    }
    if let Some(d) = data.session.as_mut().filter(|d| s(d, "id") == id) {
        out.push(d);
    }
    out
}

fn find_session(m: &MainWindow, id: &str) -> Option<Value> {
    arr(m.state(), "sessions").iter().find(|x| s(x, "id") == id).cloned()
}

/// `startRename`: edit a terminal's name in place.
pub fn start_rename(m: &mut MainWindow, id: &str, window: &mut Window, cx: &mut Context<MainWindow>) {
    let Some(x) = find_session(m, id) else { return };
    let input = kit::Input::with_text(cx, "", false, opt_s(&x, "renaming").unwrap_or(s(&x, "name")));
    input.field.update(cx, |f, cx| f.select_all(cx));
    window.focus(&input.focus, cx);
    // Focus leaving the field saves, like the web's focusout.
    let rid = id.to_string();
    let watch = cx.on_blur(&input.focus, window, move |m, _, cx| {
        if let Some(text) = m.board.rename.as_ref().filter(|(r, _, _)| *r == rid).map(|(_, i, _)| i.text(cx)) {
            end_rename(m, &rid, &text, true, cx);
        }
    });
    m.board.rename = Some((id.to_string(), input, watch));
    cx.notify();
}

/// `endRename`: save (↩, focus leaving) or drop (esc) the edit. Nothing is sent for an empty or
/// unchanged name.
pub fn end_rename(m: &mut MainWindow, id: &str, name: &str, save: bool, cx: &mut Context<MainWindow>) {
    if m.board.rename.as_ref().is_some_and(|(r, _, _)| r == id) {
        m.board.rename = None;
    }
    let name = name.trim().to_string();
    let Some(x) = find_session(m, id) else {
        cx.notify();
        return;
    };
    let cur = opt_s(&x, "renaming").unwrap_or(s(&x, "name")).to_string();
    if !save || name.is_empty() || name == cur {
        cx.notify();
        return;
    }
    for c in session_copies(m, id) {
        c["renaming"] = json!(name);
    }
    m.board.rename_watch.insert(id.to_string(), name.clone());
    let sid = id.to_string();
    m.post_or(
        format!("sessions/{id}/rename"),
        json!({"name": name}),
        cx,
        |_, _, _| {},
        move |m, e, cx| {
            for c in session_copies(m, &sid) {
                c["renaming"] = Value::Null;
                c["rename_error"] = json!(e.message);
            }
            cx.notify();
        },
    );
    cx.notify();
}

/// `renameFlash`: once a sent rename is no longer pending, flash whether it took.
fn update_flashes(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let done: Vec<(String, String)> = m
        .board
        .rename_watch
        .iter()
        .filter_map(|(id, to)| find_session(m, id).filter(|x| opt_s(x, "renaming").is_none()).map(|_| (id.clone(), to.clone())))
        .collect();
    for (id, to) in done {
        m.board.rename_watch.remove(&id);
        let x = find_session(m, &id).unwrap_or(Value::Null);
        let ok = opt_s(&x, "rename_error").is_none() && s(&x, "name") == to;
        let why = opt_s(&x, "rename_error").unwrap_or("Midna kept the old name").to_string();
        let at = Instant::now();
        m.board.flash.insert(id.clone(), Flash { ok, to, why, at });
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(if ok { FLASH_OK } else { FLASH_BAD }).await;
            let _ = this.update(cx, |m, cx| {
                if m.board.flash.get(&id).is_some_and(|f| f.at == at) {
                    m.board.flash.remove(&id);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

// ================================================================== render

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    update_flashes(m, cx);
    let st = m.state().clone();
    let down = m.down.is_some();
    let strip = strip_vm(&st, &m.filters.project, down, &m.board.flash);
    let bar = goal_bar_vm(&st, &m.data.goals, &m.filters);
    let (sel_task, sel_issue) = match &m.panel {
        Some(Panel::Task { r, .. }) => (Some(r.clone()), None),
        Some(Panel::Issue { r }) => (None, Some(r.clone())),
        None => (None, None),
    };
    let note = m.board.done_note.as_ref().map(|(t, e, _)| (t.clone(), *e));
    let cols = columns_vm(&st, &m.data.goals, &m.filters, down, sel_task.as_deref(), sel_issue.as_deref(), note);
    // The page's width (the window less the sidebar): the strip's `auto-fill` grid and the
    // columns' `minmax(220px, 1fr)` both size from it.
    let rail = m.sidebar.drag.map(|_| m.sidebar_width).unwrap_or_else(crate::ui::sidebar::rail_width);
    let main_w = (window.viewport_size().width.as_f32() - rail).max(0.);

    // `.board`: padding 28px 40px 0, gap 24px; the body's 14px/1.5 text.
    let mut page = div().id("board-page").flex().flex_col().flex_1().min_w_0().h_full().gap(px(24.)).pt(px(28.)).line_height(relative(1.5));
    page = page.child(sessions_strip(m, &t, &strip, main_w - PAGE_X * 2., window, cx));
    if let Some(bar) = bar {
        page = page.child(goal_bar(&t, &bar, cx));
    }
    page = page.child(columns(m, &t, &st, cols, main_w, cx));
    let mut out = div().relative().flex().flex_1().min_w_0().h_full().child(page);
    if let Some(menu) = done_menu(m, &t, &st, cx) {
        out = out.child(menu);
    }
    out.into_any_element()
}

/// `.board`'s side padding.
const PAGE_X: f32 = 40.;

/// `.h3`: 13px semibold muted uppercase.
fn h3(t: &Theme, text: &str) -> Div {
    div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.muted).whitespace_nowrap().child(text.to_uppercase())
}

// ------------------------------------------------------------------ sessions strip

fn sessions_strip(m: &mut MainWindow, t: &Theme, vm: &StripVm, width: f32, window: &mut Window, cx: &mut Context<MainWindow>) -> Div {
    let hover = t.accent_fg;
    let head = div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .id("board-sessions-title")
                .cursor_pointer()
                .tooltip(kit::tip("See every session"))
                .child(h3(t, "Sessions").hover(move |d| d.text_color(hover)))
                .on_click(cx.listener(|m, _, _, cx| m.go(Page::Sessions, cx))),
        )
        .children(vm.more.clone().map(|x| div().text_size(px(12.5)).text_color(t.muted).child(x)));
    let mut out = div().flex().flex_col().flex_none().gap(px(10.)).px(px(PAGE_X)).child(head);
    if let Some(e) = &vm.empty {
        // `.none-yet`.
        out = out.child(div().py(px(10.)).text_size(px(13.)).text_color(t.muted).child(e.clone()));
    }
    if !vm.cards.is_empty() {
        // `.sessions`: `repeat(auto-fill, minmax(190px, 1fr))`, gap 12px.
        let n = (((width + 12.) / (190. + 12.)).floor() as u16).max(1);
        let mut grid = div().grid().grid_cols(n).gap(px(12.));
        for c in &vm.cards {
            grid = grid.child(session_card(m, t, c, window, cx));
        }
        out = out.child(grid);
    }
    out
}

/// A status dot with the web's 3px halo (`box-shadow: 0 0 0 3px …`), which takes no space.
fn halo_dot(color: Hsla, halo: Hsla) -> Div {
    div()
        .relative()
        .flex_none()
        .size(px(8.))
        .child(div().absolute().top(px(-3.)).left(px(-3.)).size(px(14.)).rounded_full().bg(halo))
        .child(div().absolute().top_0().left_0().size(px(8.)).rounded_full().bg(color))
}

fn session_card(m: &mut MainWindow, t: &Theme, c: &SessCardVm, window: &mut Window, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let (dot, halo) = match c.status {
        "working" | "waiting" => (t.accent, t.accent_soft),
        "needs" => (t.warn, t.warn_soft),
        "offline" => (t.down, t.down_soft),
        _ => (t.faint, t.col),
    };
    let state_color = match c.status {
        "working" | "waiting" => t.accent,
        "needs" => t.warn,
        "offline" => t.down,
        _ => t.muted,
    };
    let editing = m.board.rename.as_ref().filter(|(id, _, _)| *id == c.id).map(|(_, i, _)| i.field.clone());
    let _ = window;
    let name_el: AnyElement = match editing {
        Some(field) => {
            let id = c.id.clone();
            div()
                .id(SharedString::from(format!("sess-rename-{id}")))
                .flex_1()
                .min_w_0()
                .px(px(6.))
                .py(px(1.))
                .rounded(px(5.))
                .border_1()
                .border_color(t.accent)
                .bg(t.card)
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .on_click(|_, _, cx| cx.stop_propagation())
                .on_key_down(cx.listener(move |m, ev: &KeyDownEvent, _, cx| {
                    let Some((_, input, _)) = m.board.rename.as_ref() else { return };
                    let text = input.text(cx);
                    match input.on_key(ev, cx) {
                        KeyOutcome::Submit => {
                            cx.stop_propagation();
                            end_rename(m, &id, &text, true, cx);
                        }
                        KeyOutcome::Cancel => {
                            cx.stop_propagation();
                            end_rename(m, &id, &text, false, cx);
                        }
                        KeyOutcome::Ignored => {}
                    }
                }))
                .child(field)
                .into_any_element()
        }
        None => {
            let id = c.id.clone();
            let color = match c.name.flash {
                Some(false) => t.down,
                Some(true) => t.up,
                None if c.name.renaming => t.faint,
                None => t.text,
            };
            div()
                .id(SharedString::from(format!("sess-name-{}", c.id)))
                .flex_1()
                .min_w_0()
                .truncate()
                .cursor_text()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .tooltip(kit::tip(c.name.title.clone()))
                .child(c.name.text.clone())
                .on_click(cx.listener(move |m, e: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    m.board.name_click += 1;
                    if e.click_count() >= 2 {
                        start_rename(m, &id, window, cx);
                        return;
                    }
                    // A single click opens the terminal, unless a second click follows.
                    let n = m.board.name_click;
                    let id = id.clone();
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(DOUBLE_CLICK_WAIT).await;
                        let _ = this.update(cx, |m, cx| {
                            if m.board.name_click == n && m.board.rename.is_none() {
                                open_session(m, &id, cx);
                            }
                        });
                    })
                    .detach();
                }))
                .into_any_element()
        }
    };
    let hover = t.border_2;
    let open_id = c.id.clone();
    let mut card = div()
        .id(SharedString::from(format!("sess-{}", c.id)))
        .flex()
        .flex_col()
        .gap(px(6.))
        .min_w_0()
        .px(px(14.))
        .py(px(12.))
        .rounded(px(12.))
        .border_1()
        .border_color(match c.status {
            "needs" => t.warn_line,
            "offline" => t.down_line,
            _ => t.border,
        })
        .bg(t.card)
        .cursor_pointer()
        .when(c.status == "gone", |d| d.opacity(0.65))
        .hover(move |d| d.border_color(hover))
        .tooltip(kit::tip(format!("See what {} has done", if c.name.text.is_empty() { "it" } else { &c.name.text })))
        .on_click(cx.listener(move |m, _, _, cx| open_session(m, &open_id, cx)))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .min_w_0()
                .child(halo_dot(dot, halo))
                .child(name_el)
                .child(div().flex_none().ml_auto().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(state_color).child(c.state_label.clone())),
        )
        .child(
            div()
                .id(SharedString::from(format!("sess-proj-{}", c.id)))
                .text_size(px(12.))
                .text_color(t.muted)
                .truncate()
                .when(!c.project_title.is_empty(), |d| d.tooltip(kit::tip(c.project_title.clone())))
                .child(c.project.clone()),
        );
    if let Some(text) = &c.compacting {
        card = card.child(div().flex().child(chip(t, "compact", text.clone())));
    }
    match (&c.task_ref, &c.sub) {
        (Some(r), sub) => {
            let open = r.clone();
            let accent = t.accent_fg;
            card = card.child(
                div()
                    .id(SharedString::from(format!("sess-task-{}", c.id)))
                    .text_size(px(13.))
                    .truncate()
                    .text_color(accent)
                    .cursor_pointer()
                    .hover(|d| d.underline())
                    .tooltip(kit::tip(format!("Open {r}")))
                    .child(sub.clone().unwrap_or_default())
                    .on_click(cx.listener(move |m, _, _, cx| {
                        cx.stop_propagation();
                        m.open_task(open.clone(), cx);
                    })),
            );
        }
        (None, Some(sub)) => card = card.child(div().text_size(px(13.)).truncate().child(sub.clone())),
        (None, None) => {}
    }
    card
}

// ------------------------------------------------------------------ goal bar

fn goal_bar(t: &Theme, vm: &GoalBarVm, cx: &mut Context<MainWindow>) -> Div {
    let r = vm.r.clone();
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(14.))
        .mx(px(PAGE_X))
        .pl(px(16.))
        .pr(px(10.))
        .py(px(8.))
        .rounded(px(12.))
        .border_1()
        .border_color(t.goal_line)
        .bg(t.goal_tint)
        .child(kit::glyph(kit::Glyph::Flag, 12., t.goal))
        .child(div().text_size(px(14.)).font_weight(FontWeight::BOLD).whitespace_nowrap().child(vm.name.clone()))
        .child(div().text_size(px(13.)).text_color(t.muted).truncate().child(vm.summary.clone()))
        .child(div().flex_1())
        .child(
            div()
                .id("goalbar-open")
                .flex()
                .items_center()
                .gap(px(4.))
                .h(px(32.))
                .pl(px(12.))
                .pr(px(10.))
                .rounded(px(8.))
                .bg(t.goal_soft)
                .text_color(t.goal)
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .whitespace_nowrap()
                .child("Open goal")
                .child(kit::glyph(kit::Glyph::Fwd, 16., t.goal))
                .on_click(cx.listener(move |m, _, _, cx| m.go(Page::Goal(r.clone()), cx))),
        )
        .child(
            div()
                .id("goalbar-clear")
                .size(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .cursor_pointer()
                .tooltip(kit::tip("Show every goal"))
                .child(kit::glyph(kit::Glyph::X, 16., t.muted))
                .on_click(cx.listener(|m, _, _, cx| goal_pick(m, "all", cx))),
        )
}

// ------------------------------------------------------------------ columns

/// What's being dragged: a task that starts by hand.
#[derive(Clone)]
struct DragTask {
    r: String,
    title: String,
}

impl Render for DragTask {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<Theme>();
        div()
            .w(px(220.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(12.))
            .border_1()
            .border_color(t.accent)
            .bg(t.card)
            .shadow_lg()
            .font_family(t.ui_font.clone())
            .text_size(px(13.))
            .text_color(t.text)
            .child(div().text_size(px(12.)).text_color(t.faint).child(self.r.clone()))
            .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(self.title.clone()))
    }
}

fn columns(m: &mut MainWindow, t: &Theme, st: &Value, cols: Vec<ColVm>, main_w: f32, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let colors: HashMap<&str, Hsla> = HashMap::from([("backlog", t.backlog_dot), ("queued", t.faint), ("working", t.accent), ("needs", t.warn), ("done", t.up)]);
    let dragging = cx.has_active_drag();
    // `.cols`: five `minmax(220px, 1fr)` columns 14px apart, scrolling sideways past the page's
    // 40px padding; each column as tall as its cards (`align-items: start`).
    let n = cols.len().max(1) as f32;
    let col_w = ((main_w - PAGE_X * 2. - COL_GAP * (n - 1.)) / n).max(COL_MIN);
    let mut row = div().flex().items_start().gap(px(COL_GAP)).h_full().w(px(col_w * n + COL_GAP * (n - 1.) + PAGE_X * 2.)).px(px(PAGE_X)).pb(px(24.));
    for c in cols {
        // `.col-body`: gap 10px, padding 2px 12px 12px.
        let mut body = div().flex().flex_col().gap(px(10.)).pt(px(2.)).px(px(12.)).pb(px(12.));
        for card in &c.cards {
            body = body.child(match card {
                CardVm::Task(x) => task_card(t, x, st, cx).into_any_element(),
                CardVm::Issue(x) => issue_card(t, x, cx).into_any_element(),
            });
        }
        let more = c.more.clone().map(|mo| {
            let (text, el) = match mo {
                More::Backlog(text) => (text, None),
                More::Done(text, next) => (text, Some(next)),
            };
            // `.more`: centered 12.5px, padding 4px; the Done one is a `.btn.link` (28px tall).
            let fg = if el.is_some() { t.accent_fg } else { t.accent };
            div().flex().justify_center().child(
                div()
                    .id(SharedString::from(format!("board-{}-more", c.key)))
                    .flex()
                    .items_center()
                    .when(el.is_some(), |d| d.min_h(px(28.)))
                    .p(px(4.))
                    .text_size(px(12.5))
                    .text_color(fg)
                    .underline()
                    .cursor_pointer()
                    .child(text)
                    .on_click(cx.listener(move |m, _, _, cx| match el {
                        Some(next) => set_done_window(m, next, cx),
                        None => m.go(Page::Backlog, cx),
                    })),
            )
        });
        if c.cards.is_empty() {
            if let Some(e) = &c.empty {
                body = body.child(kit::empty(t, e.clone()));
            }
        }
        body = body.children(more);
        let mut extra: Option<AnyElement> = None;
        if c.see_all {
            extra = Some(
                kit::link(t, "board-backlog-all", "See all")
                    .ml_auto()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .underline()
                    .on_click(cx.listener(|m, _, _, cx| m.go(Page::Backlog, cx)))
                    .into_any_element(),
            );
        }
        if c.done_menu {
            // `.icon-btn.sm` with `ICON.more`.
            let hover = t.border_2;
            extra = Some(
                div()
                    .id("board-done-menu")
                    .ml_auto()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(32.))
                    .rounded(px(7.))
                    .border_1()
                    .border_color(t.border)
                    .bg(t.card)
                    .cursor_pointer()
                    .hover(move |d| d.border_color(hover))
                    .tooltip(kit::tip("Options"))
                    .child(kit::glyph(kit::Glyph::More, 16., t.muted))
                    .on_click(cx.listener(|m, e: &ClickEvent, _, cx| {
                        let p = e.position();
                        m.toggle_menu(MENU_DONE, point(p.x - px(200.), p.y + px(14.)), cx);
                    }))
                    .into_any_element(),
            );
        }
        let color = colors.get(c.key).copied().unwrap_or(t.faint);
        let mut col = div()
            .id(SharedString::from(format!("col-shell-{}", c.key)))
            .flex()
            .flex_col()
            .flex_none()
            .w(px(col_w))
            .max_h_full()
            .rounded(px(14.))
            .overflow_hidden()
            .bg(t.col)
            .child(
                // `.col-head`: padding 14px 16px 8px, min-height 52px.
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .min_h(px(52.))
                    .px(px(16.))
                    .pt(px(14.))
                    .pb(px(8.))
                    .child(div().flex_none().size(px(10.)).rounded(px(3.)).bg(color))
                    .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().child(c.name))
                    .children(c.count.map(|n| div().px(px(8.)).rounded_full().bg(t.card).text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(t.muted).child(n.to_string())))
                    .children(extra),
            )
            .children(c.note.clone().map(|(text, err)| {
                // `.col-note` holding a `.note`.
                div().px(px(16.)).pb(px(6.)).mt(px(-4.)).text_size(px(12.5)).when(err, |d| d.font_weight(FontWeight::MEDIUM)).text_color(if err { t.down } else { t.up_fg }).child(text)
            }))
            .child(div().id(SharedString::from(format!("col-{}", c.key))).flex().flex_col().flex_shrink(1.).min_h_0().overflow_y_scroll().child(body));
        if c.takes_drop() {
            // `.drop-ready` / `.drop-over` (the web's --accent-tint is the theme's accent_soft here).
            let (line, over_bg, over_line) = (t.accent_line, t.accent_soft, t.accent);
            col = col
                .when(dragging, |d| d.border_2().border_color(line))
                .drag_over::<DragTask>(move |s, _, _, _| s.bg(over_bg).border_2().border_color(over_line))
                .on_drop(cx.listener(|m, d: &DragTask, _, cx| start_new(m, &d.r, cx)));
        }
        row = row.child(col);
    }
    let _ = m;
    div().id("board-cols").flex_1().min_h_0().overflow_x_scroll().child(row)
}

/// The Done column's menu: Show (window) and "Close their terminals".
fn done_menu(m: &mut MainWindow, t: &Theme, st: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let at = m.menu_open(MENU_DONE)?;
    let (items, has_done) = done_menu_vm(st, &m.filters);
    let cur = if m.filters.done.is_empty() { "24h".to_string() } else { m.filters.done.clone() };
    let mut list = kit::menu_box(t, 230.).id("board-done-menu-list").child(div().px(px(10.)).pt(px(6.)).pb(px(4.)).child(kit::h3(t, "Show")));
    for (n, (label, id)) in items.into_iter().enumerate() {
        list = list.child(kit::menu_item(t, ("board-done-win", n), label, cur == id).on_click(cx.listener(move |m, _, _, cx| set_done_window(m, id, cx))));
    }
    list = list.child(div().my(px(4.)).child(kit::divider(t)));
    let close = kit::menu_item(t, "board-done-close", "Close their terminals", false);
    list = list.child(if has_done { close.on_click(cx.listener(|m, _, _, cx| close_done(m, cx))) } else { close.opacity(0.45).cursor_default() });
    Some(kit::popover(at, list))
}

// ------------------------------------------------------------------ cards

/// The web's chip classes as colors.
fn chip_colors(t: &Theme, cls: &str) -> (Hsla, Hsla) {
    match cls {
        "high" => (t.warn, t.warn_soft),
        "repo" => (t.muted, t.panel_2),
        "jira" => (t.accent_fg, t.accent_soft),
        "b-pass" => (t.up_fg, t.up_soft),
        "b-run" => (t.accent_fg, t.accent_soft),
        "b-fail" | "bad" | "k-bug" => (t.down, t.down_soft),
        "k-gap" => (t.warn_fg, t.warn_soft),
        "k-follow" | "st-ticket" => (t.accent_fg, t.accent_soft),
        "st-task" => (t.goal, t.goal_soft),
        "st-drop" => (t.muted, t.col),
        "compact" | "line" => (t.accent_fg, t.accent_soft),
        "device" => (t.goal, t.goal_soft),
        "bit-wait" => (t.warn_fg, t.warn_soft),
        "bit" => (t.muted, t.panel_2),
        _ => (t.text_2, t.col),
    }
}

fn chip(t: &Theme, cls: &str, text: impl Into<SharedString>) -> Div {
    let (fg, bg) = chip_colors(t, cls);
    let mono = matches!(cls, "repo" | "jira");
    let regular = mono || cls.starts_with("b-");
    let icon = match cls {
        "jira" => Some(kit::Glyph::Jira),
        c if c.starts_with("b-") => Some(kit::Glyph::Pr),
        _ => None,
    };
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
        .line_height(relative(1.5))
        .font_weight(if regular { FontWeight::NORMAL } else { FontWeight::SEMIBOLD })
        .when(mono, |d| d.font_family(t.mono_font.clone()))
        .whitespace_nowrap()
        .children(icon.map(|g| kit::glyph(g, 11., fg)))
        .child(text.into())
}

/// `.goal-line`: the flag and the goal's ref (muted when it's "Not in a goal").
fn goal_line(t: &Theme, id: SharedString, text: String, title: String, in_goal: bool) -> Stateful<Div> {
    let fg = if in_goal { t.goal } else { t.muted };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .min_w_0()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .when(!title.is_empty(), |d| d.tooltip(kit::tip(title)))
        .child(kit::glyph(kit::Glyph::Flag, 12., fg))
        .child(div().min_w_0().truncate().child(text))
}

fn task_card(t: &Theme, x: &TaskCardVm, st: &Value, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let _ = st;
    let hover = t.border_2;
    let open = x.r.clone();
    let mut top = div().flex().items_center().gap(px(6.)).text_size(px(12.)).min_w_0();
    if let (Some(g), Some(name)) = (&x.goal_ref, &x.goal_name) {
        top = top
            .child(goal_line(t, SharedString::from(format!("tc-goal-{}", x.r)), g.clone(), name.clone(), true))
            .child(div().font_family(t.mono_font.clone()).text_color(t.faint).child("·"));
    }
    top = top
        .child(div().font_family(t.mono_font.clone()).text_color(t.faint).child(x.r.clone()))
        .child(div().flex_1())
        .child(div().id(SharedString::from(format!("tc-when-{}", x.r))).flex_none().text_color(t.faint).whitespace_nowrap().child(x.when.clone()).tooltip(kit::tip(x.when_title.clone())));
    let title = div()
        .flex()
        .items_start()
        .gap(px(8.))
        .child(div().flex_1().min_w_0().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).line_height(relative(1.35)).line_clamp(3).child(x.title.clone()))
        .when(x.high, |d| d.child(chip(t, "high", "High")));
    let mut chips = div().flex().flex_wrap().items_center().gap(px(6.)).min_w_0();
    for c in &x.chips {
        chips = chips.child(chip(t, &c.cls, c.text.clone()));
    }
    let mut card = div()
        .id(SharedString::from(format!("tcard-{}", x.r)))
        .flex()
        .flex_col()
        .flex_none()
        .gap(px(8.))
        .p(px(14.))
        .rounded(px(12.))
        .border_1()
        .border_color(if x.selected { t.accent } else if x.needs { t.warn_line } else { t.border })
        .bg(t.card)
        // `--shadow`: 0 1px 2px rgba(16, 24, 40, .05) in light, none in dark.
        .when(!matches!(t.mode, crate::theme::ThemeMode::Dark), |d| {
            d.shadow(vec![BoxShadow { color: hsla(220. / 360., 0.43, 0.11, 0.05), offset: point(px(0.), px(1.)), blur_radius: px(2.), spread_radius: px(0.), inset: false }])
        })
        .cursor_pointer()
        .when(!x.selected, |d| d.hover(move |s| s.border_color(hover)))
        .when(x.selected, |d| d.border_2())
        .on_click(cx.listener(move |m, _, _, cx| m.open_task(open.clone(), cx)))
        .child(top)
        .child(title)
        .child(chips)
        .children(x.who.clone().map(|w| div().flex().items_center().gap(px(6.)).min_w_0().text_size(px(12.)).child(div().min_w_0().truncate().font_weight(FontWeight::MEDIUM).text_color(t.text).child(w))));
    if x.draggable {
        let drag = DragTask { r: x.r.clone(), title: x.title.clone() };
        card = card.cursor_grab().tooltip(kit::tip("Drag to Working to start it")).on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()));
    }
    card
}

fn issue_card(t: &Theme, x: &IssueCardVm, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let hover = t.border_2;
    let open = x.r.clone();
    div()
        .id(SharedString::from(format!("icard-{}", x.r)))
        .flex()
        .flex_col()
        .flex_none()
        .gap(px(6.))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(12.))
        .border_1()
        .border_dashed()
        .border_color(if x.selected { t.accent } else { t.border_2 })
        .when(x.selected, |d| d.border_2())
        .bg(t.tint)
        .cursor_pointer()
        .when(!x.selected, |d| d.hover(move |s| s.border_color(hover)))
        .on_click(cx.listener(move |m, _, _, cx| m.open_issue(open.clone(), cx)))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.))
                .child(chip(t, &x.kind_cls, x.kind.clone()))
                .children(x.state.as_ref().map(|(l, c)| chip(t, c, l.clone())))
                .child(div().flex_1())
                .child(div().id(SharedString::from(format!("ic-when-{}", x.r))).text_size(px(12.)).text_color(t.faint).whitespace_nowrap().child(x.when.clone()).tooltip(kit::tip(x.when_title.clone()))),
        )
        .child(div().text_size(px(13.5)).font_weight(FontWeight::MEDIUM).line_height(relative(1.35)).line_clamp(3).child(x.title.clone()))
        .child(goal_line(t, SharedString::from(format!("ic-goal-{}", x.r)), x.goal.clone(), x.goal_title.clone(), x.in_goal))
}

// ================================================================== tests

#[cfg(test)]
mod tests {
    use super::*;
    // Beats the glob's `gpui_kit::test`, which `#[gpui_kit::test]`'s own `#[test]` would hit.
    use ::core::prelude::v1::test;
    use crate::parity::{golden, settle, window};
    use serde_json::json;

    fn filters(v: &Value) -> Filters {
        Filters {
            project: v["project"].as_str().unwrap_or("all").into(),
            goal: v["goal"].as_str().unwrap_or("all").into(),
            done: v["done"].as_str().unwrap_or("").into(),
        }
    }

    fn sel<'a>(route: &'a Value, k: &str) -> Option<&'a str> {
        route["q"][k].as_str()
    }

    #[test]
    fn a_task_in_a_terminals_line_says_where_and_still_has_start() {
        let x = json!({"id": 14, "ref": "T14", "title": "Footer", "project": "webapp", "status": "queued",
                       "line": {"session": "s1", "name": "Term 3", "kind": "resume", "pos": 1, "label": "To resume in Term 3", "after": "T12"}});
        let card = task_card_vm(&x, None);
        assert!(card.chips.iter().any(|c| c.text == "To resume in Term 3" && c.cls == "line"));
        assert!(starts_by_hand(&x), "Start takes it out of the line");
        let mut in_goal = x.clone();
        in_goal["goal"] = json!({"id": 3, "title": "Launch"});
        assert!(starts_by_hand(&in_goal), "a goal task bumped into a line can be started elsewhere");
        in_goal["line"] = Value::Null;
        assert!(!starts_by_hand(&in_goal));
    }

    #[test]
    fn board_matches_web() {
        let g = golden("board").only("board");
        assert!(g.cases.len() >= 10, "board scenarios missing from the golden");
        g.check(|i| {
            let st = &i["state"];
            let goals = i["goals"].as_array().cloned().unwrap_or_default();
            let f = filters(&i["filter"]);
            let down = !i["down"].is_null();
            let strip = strip_vm(st, &f.project, down, &HashMap::new());
            let cols = columns_vm(st, &goals, &f, down, sel(&i["route"], "task"), sel(&i["route"], "issue"), None);
            let tasks: Vec<Value> = cols
                .iter()
                .flat_map(|c| c.cards.iter())
                .filter_map(|c| match c {
                    CardVm::Task(x) => Some(json!({
                        "ref": x.r, "text": x.text(), "acts": ["open-task"], "when_title": x.when_title, "goal_title": x.goal_name,
                        "drag": x.draggable, "drag_title": if x.draggable { json!("Drag to Working to start it") } else { Value::Null },
                        "selected": x.selected, "needs": x.needs, "chips": x.chip_classes(),
                    })),
                    _ => None,
                })
                .collect();
            let issues: Vec<Value> = cols
                .iter()
                .flat_map(|c| c.cards.iter())
                .filter_map(|c| match c {
                    CardVm::Issue(x) => Some(json!({
                        "ref": x.r, "text": x.text(), "acts": ["open-issue"], "when_title": x.when_title, "goal_title": x.goal_title,
                        "goal_none": !x.in_goal, "selected": x.selected, "chips": x.chip_classes(),
                    })),
                    _ => None,
                })
                .collect();
            let (items, has_done) = done_menu_vm(st, &f);
            let menu_text = join(std::iter::once("Show").chain(items.iter().map(|(l, _)| l.as_str())).chain(std::iter::once("Close their terminals")));
            json!({
                "strip": {
                    "text": strip.text(), "acts": strip.acts(),
                    "cards": strip.cards.iter().map(|c| json!({
                        "id": c.id, "text": c.text(), "acts": c.acts(), "name_title": c.name.title, "project_title": c.project_title,
                        "renaming": c.name.renaming, "needs": c.status == "needs", "gone": c.status == "gone",
                    })).collect::<Vec<_>>(),
                },
                "columns": cols.iter().map(|c| json!({"text": c.text(), "acts": c.acts(), "drop": c.takes_drop()})).collect::<Vec<_>>(),
                "task_cards": tasks,
                "issue_cards": issues,
                "goal_bar": goal_bar_vm(st, &goals, &f).map(|b| json!({"text": b.text(), "acts": b.acts()})),
                "done_menu": {
                    "text": menu_text,
                    "acts": std::iter::once("done-menu").chain(items.iter().map(|_| "done-window")).chain(std::iter::once("close-done")).collect::<Vec<_>>(),
                    "close_disabled": !has_done,
                },
            })
        });
    }

    #[test]
    fn state_query_matches_web() {
        golden("board").only("state_query").check(|i| {
            let keep: Vec<String> = i["keep"].as_array().unwrap().iter().map(|k| k.as_str().unwrap().to_string()).collect();
            let q: serde_json::Map<String, Value> = state_query(&filters(&i["filter"]), &keep).into_iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
            Value::Object(q)
        });
    }

    #[test]
    fn name_view_matches_web() {
        golden("board").only("name_view").check(|i| {
            let v = name_view(&i["session"], None, " · double-click to rename");
            json!({"text": v.text, "renaming": v.renaming, "title": v.title})
        });
    }

    /// The fake board's goals as the board knows them (G1 is the sample goal in webapp).
    fn with_filters(m: &mut MainWindow, f: &Value) {
        let mut x = filters(f);
        if f["done"].is_null() {
            x.done = String::new();
        }
        m.filters = x;
    }

    fn as_json(f: &Filters, had_done: bool) -> Value {
        let mut v = json!({"project": f.project, "goal": f.goal});
        if had_done || !f.done.is_empty() {
            v["done"] = json!(f.done);
        }
        v
    }

    #[gpui_kit::test]
    fn goal_pick_and_done_window_match_web(cx: &mut TestAppContext) {
        let (w, _rec) = window(cx);
        for c in golden("board").cases {
            let i = &c["input"];
            let f = match i["fn"].as_str() {
                Some("goal_pick") | Some("done_window") => i,
                _ => continue,
            };
            let had_done = !f["filter"]["done"].is_null();
            let got = w
                .update(cx, |m, _, cx| {
                    // The golden's goals: G2 lives in api.
                    m.data.goals = vec![json!({"id": 1, "ref": "G1", "project": "webapp"}), json!({"id": 2, "ref": "G2", "project": "api"})];
                    with_filters(m, &f["filter"]);
                    m.board.keep = vec!["B1".into()];
                    let arg = f["arg"].as_str().unwrap();
                    if f["fn"] == "goal_pick" { goal_pick(m, arg, cx) } else { set_done_window(m, arg, cx) }
                    json!({"filter": as_json(&m.filters, had_done || f["fn"] == "done_window"), "keep": m.board.keep})
                })
                .unwrap();
            assert_eq!(got, c["expect"], "{}", c["name"]);
        }
    }

    #[gpui_kit::test]
    fn keep_issue_only_on_the_board(cx: &mut TestAppContext) {
        let (w, _rec) = window(cx);
        for c in golden("board").only("keep_issue").cases {
            let i = &c["input"];
            let got = w
                .update(cx, |m, _, _| {
                    m.page = if i["page"] == "board" { Page::Board } else { Page::Backlog };
                    m.board.keep = vec!["B1".into()];
                    for r in i["add"].as_array().unwrap() {
                        keep_issue(m, r.as_str().unwrap());
                    }
                    json!(m.board.keep)
                })
                .unwrap();
            assert_eq!(got, c["expect"], "{}", c["name"]);
        }
    }

    #[gpui_kit::test]
    fn dropping_on_working_starts_a_new_terminal(cx: &mut TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |m, _, cx| start_new(m, "T5", cx)).unwrap();
        settle(cx);
        assert_eq!(rec.posts(), vec![("tasks/T5/start".to_string(), json!({"mode": "new"}))]);
        w.update(cx, |m, _, _| assert!(m.toasts.iter().any(|t| t.text == "Starting T5" && !t.err))).unwrap();
    }

    #[gpui_kit::test]
    fn close_done_posts_and_notes(cx: &mut TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |m, _, cx| close_done(m, cx)).unwrap();
        settle(cx);
        assert_eq!(rec.last("done/close-terminals"), Some(json!({})));
        w.update(cx, |m, _, _| {
            let expect = sent_note(m.state());
            assert_eq!(m.board.done_note.as_ref().map(|(t, e, _)| (t.clone(), *e)), Some((expect, false)));
        })
        .unwrap();
    }

    #[test]
    fn sent_note_matches_web_wording() {
        assert_eq!(sent_note(&json!({"midna": {"up": false}})), "Saved. It runs once Midna is back.");
        assert_eq!(sent_note(&json!({"midna": {"up": true}})), "Sent to Midna");
        assert_eq!(sent_note(&json!({})), "Sent to Midna");
    }

    #[gpui_kit::test]
    fn done_window_saves_the_filters(cx: &mut TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m, _, cx| set_done_window(m, "7d", cx)).unwrap();
        assert_eq!(crate::prefs::get("taskboard.filter").unwrap()["done"], json!("7d"));
    }

    #[gpui_kit::test]
    fn rename_from_the_strip(cx: &mut TestAppContext) {
        let (w, rec) = window(cx);
        // Unchanged, empty and cancelled renames send nothing.
        w.update(cx, |m, _, cx| {
            let name = s(&find_session(m, "fake-s3").unwrap(), "name").to_string();
            end_rename(m, "fake-s3", &name, true, cx);
            end_rename(m, "fake-s3", "   ", true, cx);
            end_rename(m, "fake-s3", "new one", false, cx);
        })
        .unwrap();
        settle(cx);
        assert!(rec.posts().is_empty());
        w.update(cx, |m, _, cx| {
            end_rename(m, "fake-s3", "  api terminal ", true, cx);
            // Shown as renaming at once.
            assert_eq!(s(&find_session(m, "fake-s3").unwrap(), "renaming"), "api terminal");
        })
        .unwrap();
        settle(cx);
        assert_eq!(rec.last("sessions/fake-s3/rename"), Some(json!({"name": "api terminal"})));
    }

    #[test]
    fn a_compacting_terminal_shows_a_chip() {
        let at = fmt::now().to_rfc3339();
        let card = task_card_vm(&json!({"id": 3, "ref": "T3", "title": "Fix", "project": "web", "status": "working", "compacting": at}), None);
        let chip = card.chips.iter().find(|c| c.cls == "compact").expect("a compacting chip");
        assert!(chip.text.starts_with("Compacting since ") && (chip.text.ends_with("AM") || chip.text.ends_with("PM")), "{}", chip.text);
        let st = json!({"sessions": [{"id": "s1", "name": "T3 Fix", "status": "working", "project": "web", "compacting": at}, {"id": "s2", "name": "Other", "status": "idle", "project": "web"}]});
        let strip = strip_vm(&st, "all", false, &HashMap::new());
        assert!(strip.cards[0].compacting.as_deref().is_some_and(|c| c.starts_with("Compacting since ")));
        assert!(strip.cards[1].compacting.is_none());
    }

    #[gpui_kit::test]
    fn open_session_goes_to_the_sessions_page(cx: &mut TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m, _, cx| {
            open_session(m, "fake-s2", cx);
            assert_eq!(m.page, Page::Sessions);
            assert_eq!(m.sessions.selected.as_deref(), Some("fake-s2"));
        })
        .unwrap();
    }
}
