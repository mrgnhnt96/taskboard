//! The Sessions page: live Claude terminals (grouped by project), closed ones, and one terminal's detail.
//!
//! A one-to-one port of the web board's `sessions.js` (and the rename helpers in `app.js`), checked
//! against it by the `sessions` parity golden (`parity/gen/sessions.mjs`). What each part shows is
//! worked out by pure view-model functions ([`list_vm`], [`row_vm`], [`header_vm`], [`menu_items`],
//! [`detail_vm`], [`bulk_vm`], [`name_vm`]) that the render code draws from, so the tests see exactly
//! what's on screen. The list sits on the left (search, status filter, collapsible project groups,
//! picking idle terminals to close together, the Closed group) and the selected terminal on the
//! right (status, path / branch / diff, Show in Midna, Close or press-and-hold Force close, rename by
//! double-clicking its name, its task, the last prompt and reply, stats and the timeline). Right-click
//! a row for its menu.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, b, i, obj, opt_s, s};
use crate::theme::{Theme, ThemeMode};
use crate::ui::text_input::FieldChanged;
use crate::ui::{kit, md};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const LIST_W: f32 = 460.;
const STALE_MS: i64 = 60 * 60 * 1000;
const HOLD: Duration = Duration::from_millis(1200);
const MENU_PREFIX: &str = "ss-menu:";
const FILTERS: [(&str, &str); 5] = [("all", "All"), ("needs", "Needs you"), ("working", "Working"), ("idle", "Idle"), ("stale", "Idle over 1 hour")];
/// localStorage key of the collapsed project groups.
const COLLAPSED_KEY: &str = "tb.sessCollapsed";
/// How long a rename's ✓ / ✗ shows (`FLASH_MS`).
const FLASH_OK: Duration = Duration::from_millis(1200);
const FLASH_BAD: Duration = Duration::from_millis(1800);
/// Inline notes go after 5 s, errors after 15 s (`setNote`).
const NOTE_OK: Duration = Duration::from_secs(5);
const NOTE_ERR: Duration = Duration::from_secs(15);
const DETAIL_HINT: &str = " · double-click to rename";

/// A rename's result, shown briefly on the name (`renameFlash`).
#[derive(Clone, Debug)]
pub struct Flash {
    pub ok: bool,
    pub to: String,
    pub why: String,
    pub at: Instant,
}

/// An inline note under a group of buttons (`setNote`).
#[derive(Clone, Debug)]
pub struct Note {
    pub text: String,
    pub err: bool,
    pub at: Instant,
}

/// What the app shows for a session until the board's next answer replaces it: the name being
/// renamed to (the web board set `renaming` on its copies right away) or why the rename failed.
#[derive(Clone, Debug)]
struct Pending {
    renaming: Option<String>,
    error: Option<String>,
    /// The row as the board last answered when the rename was sent / failed; a different answer
    /// (or 4 s) retires the override.
    seen: String,
    at: Instant,
}

#[derive(Default)]
pub struct State {
    /// The terminal shown on the right (the web route's `s=`).
    pub selected: Option<String>,
    /// "all" | "needs" | "working" | "idle" | "stale" ("" = all; the route's `f=`).
    filter: String,
    search: Option<kit::Input>,
    /// Collapsed project groups, in the order they were collapsed (saved as `tb.sessCollapsed`).
    collapsed: Vec<String>,
    collapsed_loaded: bool,
    closed_open: bool,
    /// A selection that sits in a group the reader collapsed: it doesn't force that group open.
    hide_sel: Option<String>,
    /// Picking idle terminals to close together, and the picks in the order they were made.
    selecting: bool,
    picked: Vec<String>,
    /// The "Close N idle terminals?" dialog is up.
    bulk: bool,
    /// The terminal whose "Close …?" confirmation is showing.
    confirm: Option<String>,
    /// The terminal being renamed, its name field, and the blur listener that saves it.
    rename: Option<(String, kit::Input)>,
    rename_sub: Option<Subscription>,
    /// Press-and-hold Force close: which terminal, and since when.
    hold: Option<(String, Instant)>,
    /// Requests in flight (`S.busy`: `<act>:<arg>:<id>`), whose buttons read "Sending…".
    busy: HashSet<String>,
    /// Inline notes by group (`ss:<id>`, `ss-bulk`).
    notes: HashMap<String, Note>,
    /// Renames sent and the name asked for (`S.renameWatch`), and their results (`S.renameFlash`).
    watch: HashMap<String, String>,
    flash: HashMap<String, Flash>,
    pending: HashMap<String, Pending>,
    /// "Back to T12": the task the reader came from (the route's `back=`).
    back: Option<String>,
}

// ------------------------------------------------------------------ helpers (sessions.js)

/// `SESS[s.status] ? s.status : 'idle'`, plus the native "offline" (its last turn lost the network)
/// and "waiting" (its last turn left background commands or agents running).
fn display_status(x: &Value) -> &str {
    match s(x, "status") {
        st @ ("working" | "needs" | "idle" | "gone" | "offline" | "waiting") => st,
        _ => "idle",
    }
}

/// `s.status || 'idle'` (filters compare the raw status).
fn raw_status(x: &Value) -> &str {
    opt_s(x, "status").unwrap_or("idle")
}

/// `SESS`.
fn sess_label(st: &str) -> &'static str {
    match st {
        "working" => "Working",
        "needs" => "Needs you",
        "offline" => "No network",
        "waiting" => "Waiting",
        "gone" => "Gone",
        _ => "Idle",
    }
}

/// The status pill's text: what's running ("2 agents running") while its background work runs,
/// else `sess_label`.
fn state_label(x: &Value, st: &str) -> String {
    match st {
        "waiting" => fmt::background_label(&x["background"]),
        _ => sess_label(st).to_string(),
    }
}

/// `SESS_ORDER[s.status] ?? 2`.
fn rank(x: &Value) -> u8 {
    match s(x, "status") {
        "needs" | "offline" => 0,
        "working" | "waiting" => 1,
        "gone" => 3,
        _ => 2,
    }
}

/// `idleMs`: since `last_activity` (else `seen_at`), 0 when neither parses. The board's `idle_secs`
/// wins when it sends one: it leaves out the time the Mac slept.
fn idle_ms(x: &Value) -> i64 {
    if let Some(secs) = x.get("idle_secs").and_then(|v| v.as_i64()) {
        return secs * 1000;
    }
    opt_s(x, "last_activity").or(opt_s(x, "seen_at")).and_then(fmt::parse).map(|t| (fmt::now() - t).num_milliseconds()).unwrap_or(0)
}

fn is_stale(x: &Value) -> bool {
    raw_status(x) == "idle" && idle_ms(x) > STALE_MS
}

/// Idle, no task, not already closing: can be closed in a batch.
fn bulkable(x: &Value) -> bool {
    s(x, "close") == "close" && opt_s(x, "task_ref").is_none() && !b(x, "closing")
}

pub fn for_filter(x: &Value, f: &str) -> bool {
    match f {
        "" | "all" => true,
        "stale" => is_stale(x),
        "needs" => matches!(raw_status(x), "needs" | "offline"),
        "working" => matches!(raw_status(x), "working" | "waiting"),
        f => raw_status(x) == f,
    }
}

/// `ssMatch`: the trimmed, lower-cased query in the name, project, task title or ref, or branch.
pub fn matches(x: &Value, q: &str) -> bool {
    let q = q.trim().to_lowercase();
    q.is_empty() || ["name", "project", "task_title", "task_ref", "branch"].iter().any(|k| s(x, k).to_lowercase().contains(&q))
}

/// `longAgo`: "under a minute", "12 min", "2 h 5 min", "3 h", "2 days".
pub fn long_ago(ms: i64) -> String {
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

/// `groupedSessions`: by project ("No project"), rows by status then most recent, groups with the
/// most urgent row first, then by name.
pub fn grouped(list: Vec<&Value>) -> Vec<(String, Vec<&Value>)> {
    let mut groups: Vec<(String, Vec<&Value>)> = Vec::new();
    for x in list {
        let k = opt_s(x, "project").unwrap_or("No project").to_string();
        match groups.iter_mut().find(|(p, _)| *p == k) {
            Some((_, v)) => v.push(x),
            None => groups.push((k, vec![x])),
        }
    }
    for (_, v) in groups.iter_mut() {
        v.sort_by_key(|x| (rank(x), idle_ms(x)));
    }
    groups.sort_by(|a, b| {
        let ra = a.1.iter().map(|x| rank(x)).min().unwrap_or(2);
        let rb = b.1.iter().map(|x| rank(x)).min().unwrap_or(2);
        ra.cmp(&rb).then_with(|| crate::ui::modals::locale_cmp(&a.0, &b.0))
    });
    groups
}

/// `statusLine`: "for 12 min" (how long in this status), or "2h ago" once closed.
pub fn status_line(d: &Value) -> String {
    let st = s(d, "status");
    if opt_s(d, "gone_at").is_some() || st == "gone" {
        return fmt::ago(opt_s(d, "gone_at").or(opt_s(d, "last_activity")).unwrap_or(""));
    }
    let since = |at: &str| fmt::parse(at).map(|t| format!("for {}", long_ago((fmt::now() - t).num_milliseconds()))).unwrap_or_default();
    let at = |k: &str| opt_s(d, "status_at").or_else(|| obj(d, k).and_then(|o| opt_s(o, "at"))).unwrap_or("");
    match st {
        "working" => since(at("prompt")),
        "needs" | "offline" => since(at("waiting")),
        "waiting" => since(opt_s(d, "status_at").unwrap_or("")),
        _ => format!("for {}", long_ago(idle_ms(d))),
    }
}

/// `diffView`: (text, hover title).
pub fn diff_vm(d: &Value) -> Option<(String, Option<String>)> {
    match obj(d, "diff") {
        None => d.get("dirty").and_then(Value::as_i64).map(|n| (if n > 0 { fmt::plural(n, "file", "files") } else { "No changes".into() }, None)),
        Some(g) if i(g, "files") == 0 => Some(("No changes".into(), None)),
        Some(g) => {
            let new = i(g, "new");
            let title = if new > 0 { format!("Not committed yet · {new} new") } else { "Not committed yet".into() };
            Some((format!("{} +{} −{}", fmt::plural(i(g, "files"), "file", "files"), i(g, "added"), i(g, "removed")), Some(title)))
        }
    }
}

/// `closeCopy`: (title, text, button label).
pub fn close_copy(d: &Value) -> (String, String, &'static str) {
    let force = s(d, "close") == "force";
    let name = s(d, "name");
    let title = if force { format!("Force close {name}?") } else { format!("Close {name}?") };
    let mut text = String::new();
    if force {
        match obj(d, "prompt").and_then(|p| opt_s(p, "text")) {
            Some(p) => text.push_str(&format!("It’s in the middle of a turn: “{}”. Closing it stops that. ", one(p, 120))),
            None => text.push_str("It’s in the middle of something. Closing it stops that. "),
        }
    }
    if obj(d, "task").is_some() {
        text.push_str("Its task goes back to Queued with everything saved so far, so another terminal can pick it up. ");
    }
    text.push_str("The board keeps the conversation, so you can reopen it later. Midna may ask you to confirm on the Mac.");
    (title, text, if force { "Force close" } else { "Close terminal" })
}

/// `one`: on one line, cut to `n` characters.
fn one(text: &str, n: usize) -> String {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > n { t.chars().take(n - 1).collect::<String>() + "…" } else { t }
}

/// `project_path.replace(/^\/Users\/[^/]+/, '~')`.
fn short_path(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("/Users/") {
        let end = rest.find('/').unwrap_or(rest.len());
        if end > 0 {
            return format!("~{}", &rest[end..]);
        }
    }
    p.to_string()
}

/// `STATUS[stKey(t)]` and the pill's tone.
fn task_pill(t: &Value) -> (&'static str, &'static str) {
    match s(t, "status") {
        "done" if b(t, "failed") => ("Failed", "down"),
        "queued" if b(t, "blocked") => ("Blocked", "muted"),
        "planned" => ("Planned", "goal"),
        "working" => ("Working", "accent"),
        "needs" => ("Needs you", "warn"),
        "done" => ("Done", "up"),
        _ => ("Queued", "muted"),
    }
}

/// `TIMELINE_KINDS`: the dot and the label of a timeline entry.
fn timeline_kind(t: &Theme, kind: &str) -> (Hsla, &'static str) {
    let dot = match kind {
        "prompt" | "turn" | "take" => t.accent,
        "commit" | "done" => t.up,
        "checkpoint" => t.goal,
        "found" => seen(t),
        "ask" | "wait" => t.warn,
        "fail" | "close" | "closed" | "close_failed" => t.down,
        _ => t.faint,
    };
    (dot, timeline_label(kind))
}

/// `sentNote`.
pub fn sent_note(state: &Value) -> &'static str {
    if state["midna"]["up"] == Value::Bool(false) { "Saved. It runs once Midna is back." } else { "Sent to Midna" }
}

/// Collapse whitespace the way the parity tests compare visible text.
#[cfg(test)]
fn squash(parts: &[&str]) -> String {
    parts.iter().flat_map(|p| p.split_whitespace()).collect::<Vec<_>>().join(" ")
}

// ------------------------------------------------------------------ names (nameView)

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameKind {
    Plain,
    Renaming,
    Ok,
    Bad,
}

#[cfg(test)]
impl NameKind {
    /// The web board's class for this state (`renaming`, `rename-ok`, `rename-bad`).
    pub fn class(self) -> &'static str {
        match self {
            NameKind::Plain => "",
            NameKind::Renaming => "renaming",
            NameKind::Ok => "rename-ok",
            NameKind::Bad => "rename-bad",
        }
    }
}

#[derive(Clone, Debug)]
pub struct NameVm {
    pub text: String,
    pub kind: NameKind,
    pub title: String,
}

/// `renameFlash`: a watched rename whose `renaming` is gone becomes a ✓ (the name took) or ✗ flash.
/// Returns true when a flash started (the caller schedules its end).
pub fn observe_rename(watch: &mut HashMap<String, String>, flash: &mut HashMap<String, Flash>, x: &Value) -> bool {
    let id = s(x, "id");
    let Some(to) = watch.get(id).cloned() else { return false };
    if opt_s(x, "renaming").is_some() {
        return false;
    }
    watch.remove(id);
    let ok = opt_s(x, "rename_error").is_none() && s(x, "name") == to;
    let why = opt_s(x, "rename_error").unwrap_or("Midna kept the old name").to_string();
    flash.insert(id.to_string(), Flash { ok, to, why, at: Instant::now() });
    true
}

/// `nameView`: what a session's name reads, given its flash.
pub fn name_vm(x: &Value, hint: &str, flash: Option<&Flash>) -> NameVm {
    if let Some(r) = opt_s(x, "renaming") {
        return NameVm { text: r.into(), kind: NameKind::Renaming, title: "Renaming in Midna…".into() };
    }
    if let Some(f) = flash.filter(|f| !f.ok) {
        return NameVm { text: f.to.clone(), kind: NameKind::Bad, title: format!("Rename failed: {}", f.why) };
    }
    let base = match opt_s(x, "rename_error") {
        Some(e) if flash.is_none() => format!("Last rename failed: {e}"),
        _ => s(x, "name").to_string(),
    };
    NameVm { text: opt_s(x, "name").unwrap_or(s(x, "id")).to_string(), kind: if flash.is_some() { NameKind::Ok } else { NameKind::Plain }, title: base + hint }
}

// ------------------------------------------------------------------ view models

/// What the list is drawn from (the web's `SS` and route).
pub struct ListCtx<'a> {
    pub live: Option<&'a [Value]>,
    /// `SS.listErr`: why the list couldn't be fetched (shown instead of "Loading…").
    pub list_err: Option<&'a str>,
    pub closed: &'a [Value],
    /// `GET /sessions/closed` `total` (all closed rows before the 50-row cut).
    pub closed_total: i64,
    pub sel: Option<&'a str>,
    pub filter: &'a str,
    /// The search box as typed.
    pub q: &'a str,
    pub collapsed: &'a [String],
    pub hide_sel: Option<&'a str>,
    pub selecting: bool,
    pub picked: &'a [String],
    pub closed_open: bool,
    pub flash: &'a HashMap<String, Flash>,
}

impl ListCtx<'_> {
    fn f(&self) -> &str {
        if self.filter.is_empty() { "all" } else { self.filter }
    }

    /// `holdsSel`.
    fn holds_sel(&self, rows: &[&Value]) -> bool {
        self.sel.is_some_and(|sel| Some(sel) != self.hide_sel && rows.iter().any(|x| s(x, "id") == sel))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Not picking.
    None,
    /// A checkbox (picked or not).
    Box(bool),
    /// Picking, but this one can't be closed in a batch.
    Spacer,
}

#[derive(Clone, Debug)]
pub struct RowVm {
    pub id: String,
    pub name: NameVm,
    pub sub: String,
    pub state: String,
    pub when: String,
    /// The dot: idle / working / needs / gone.
    pub status: String,
    pub on: bool,
    pub closing: bool,
    pub closed: bool,
    pub pick: Pick,
}

#[cfg(test)]
impl RowVm {
    pub fn text(&self) -> String {
        squash(&[&self.name.text, &self.sub, &self.state, &self.when])
    }
}

/// `ssRow`.
pub fn row_vm(x: &Value, c: &ListCtx) -> RowVm {
    let id = s(x, "id").to_string();
    let status = display_status(x).to_string();
    let mut sub = match (opt_s(x, "task_ref"), opt_s(x, "role")) {
        (Some(r), _) => format!("{r} · {}", s(x, "task_title")),
        // The Jira desk: "Handles Jira for the board".
        (None, Some(role)) => role.to_string(),
        (None, None) => ["No task", s(x, "branch")].iter().filter(|v| !v.is_empty()).copied().collect::<Vec<_>>().join(" · "),
    };
    // The tasks waiting their turn in it: "T12 · Fix the header · then T14, T9".
    let line: Vec<&str> = arr(x, "line").iter().filter_map(|l| opt_s(l, "ref")).collect();
    if !line.is_empty() {
        sub = format!("{sub} · then {}", line.join(", "));
    }
    let state = if b(x, "closing") {
        "Closing…".to_string()
    } else if opt_s(x, "compacting").is_some() {
        "Compacting".to_string()
    } else {
        state_label(x, &status)
    };
    let when = if status == "idle" {
        if idle_ms(x) > 60000 { format!("for {}", long_ago(idle_ms(x))) } else { "just now".into() }
    } else {
        fmt::ago(s(x, "last_activity"))
    };
    let can = bulkable(x);
    RowVm {
        on: c.sel == Some(id.as_str()),
        name: name_vm(x, "", c.flash.get(&id)),
        pick: if !c.selecting { Pick::None } else if can { Pick::Box(c.picked.contains(&id)) } else { Pick::Spacer },
        closing: b(x, "closing"),
        closed: false,
        id,
        sub,
        state,
        when,
        status,
    }
}

/// `ssClosedRow`.
pub fn closed_row_vm(x: &Value, c: &ListCtx) -> RowVm {
    let id = s(x, "id").to_string();
    let task = match opt_s(x, "task_ref") {
        Some(r) => format!("{r} · {}", s(x, "task_title")),
        None => "No task".into(),
    };
    let sub = [s(x, "project"), task.as_str()].iter().filter(|v| !v.is_empty()).copied().collect::<Vec<_>>().join(" · ");
    RowVm {
        on: c.sel == Some(id.as_str()),
        name: NameVm { text: s(x, "name").to_string(), kind: NameKind::Plain, title: s(x, "name").to_string() },
        sub,
        state: "Closed".into(),
        when: fmt::ago(s(x, "closed_at")),
        status: "gone".into(),
        closing: false,
        closed: true,
        pick: if c.selecting { Pick::Spacer } else { Pick::None },
        id,
    }
}

#[derive(Clone, Debug)]
pub struct GroupVm {
    /// The project ("" for the Closed group).
    pub key: String,
    pub label: String,
    /// "3 terminals", plus " · 1 needs you" on a collapsed group.
    pub count: String,
    pub open: bool,
    pub closed: bool,
    pub rows: Vec<RowVm>,
}

#[derive(Clone, Debug)]
pub struct BulkBarVm {
    pub line: String,
    pub add: Option<String>,
    pub close: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ListVm {
    /// "Loading…" until the first answer.
    /// "Loading…" (or `SS.listErr`) until the first answer; None once there is a list.
    pub loading: Option<String>,
    /// "Select N idle" next to the search box.
    pub stale_btn: Option<(String, String)>,
    pub bulk: Option<BulkBarVm>,
    pub groups: Vec<GroupVm>,
    pub empty: Option<String>,
    pub closed: Option<GroupVm>,
}

#[cfg(test)]
impl ListVm {
    pub fn text(&self) -> String {
        let mut p: Vec<String> = Vec::new();
        if let Some((l, _)) = &self.stale_btn {
            p.push(l.clone());
        }
        if let Some(bb) = &self.bulk {
            p.push(bb.line.clone());
            p.extend(bb.add.clone());
            p.push("Done".into());
            p.extend(bb.close.clone());
        }
        if let Some(l) = &self.loading {
            p.push(l.clone());
        }
        for g in &self.groups {
            p.push(format!("{} {}", g.label, g.count));
            p.extend(g.rows.iter().map(RowVm::text));
        }
        p.extend(self.empty.clone());
        if let Some(g) = &self.closed {
            p.push(format!("{} {}", g.label, g.count));
            p.extend(g.rows.iter().map(RowVm::text));
        }
        squash(&p.iter().map(String::as_str).collect::<Vec<_>>())
    }

    pub fn acts(&self) -> Vec<&'static str> {
        let mut a = Vec::new();
        if self.stale_btn.is_some() {
            a.push("ss-stale");
        }
        if let Some(bb) = &self.bulk {
            if bb.add.is_some() {
                a.push("ss-stale");
            }
            a.push("ss-done");
            if bb.close.is_some() {
                a.push("ss-bulk");
            }
        }
        for g in self.groups.iter().chain(self.closed.iter()) {
            a.push(if g.closed { "ss-closed" } else { "ss-group" });
            a.extend(g.rows.iter().map(|_| "ss-open"));
        }
        a
    }
}

fn close_label(n: usize) -> String {
    if n == 1 { "Close 1 terminal".into() } else { format!("Close {n} terminals") }
}

/// `ssList`.
pub fn list_vm(c: &ListCtx) -> ListVm {
    let Some(live) = c.live else {
        return ListVm { loading: Some(c.list_err.unwrap_or("Loading…").to_string()), stale_btn: None, bulk: None, groups: Vec::new(), empty: None, closed: None };
    };
    let f = c.f();
    let q = c.q.trim();
    let stale = live.iter().filter(|x| bulkable(x) && is_stale(x)).count();
    let stale_btn = (!c.selecting && stale > 1 && f == "all")
        .then(|| (format!("Select {stale} idle"), format!("Select the {stale} terminals that have been idle for over an hour, to close them together")));
    let bulk = c.selecting.then(|| {
        let n = c.picked.len();
        let more = stale > 1 && f == "all" && live.iter().any(|x| bulkable(x) && is_stale(x) && !c.picked.iter().any(|p| p == s(x, "id")));
        BulkBarVm { line: format!("{n} selected"), add: more.then(|| format!("Add the {stale} idle over 1 h")), close: (n > 0).then(|| close_label(n)) }
    });
    let visible: Vec<&Value> = live.iter().filter(|x| for_filter(x, f) && matches(x, q)).collect();
    let groups: Vec<GroupVm> = grouped(visible)
        .into_iter()
        .map(|(proj, rows)| {
            let open = !c.collapsed.contains(&proj) || !q.is_empty() || c.holds_sel(&rows);
            let needs = rows.iter().filter(|x| matches!(s(x, "status"), "needs" | "offline")).count();
            let mut count = fmt::plural(rows.len() as i64, "terminal", "terminals");
            if !open && needs > 0 {
                count.push_str(&format!(" · {needs} need{} you", if needs == 1 { "s" } else { "" }));
            }
            GroupVm { label: proj.clone(), key: proj, count, open, closed: false, rows: if open { rows.iter().map(|x| row_vm(x, c)).collect() } else { Vec::new() } }
        })
        .collect();
    let empty = groups.is_empty().then(|| {
        if !q.is_empty() {
            format!("No terminal matches “{q}”.")
        } else if f == "all" {
            "No Claude terminals in Midna right now.".into()
        } else {
            "None right now.".into()
        }
    });
    let closed = (!c.closed.is_empty() && f == "all")
        .then(|| {
            let shown: Vec<&Value> = c.closed.iter().filter(|x| matches(x, q)).collect();
            (!shown.is_empty()).then(|| {
                let open = c.closed_open || c.holds_sel(&shown);
                let total = if !q.is_empty() { shown.len() as i64 } else if c.closed_total > 0 { c.closed_total } else { c.closed.len() as i64 };
                GroupVm {
                    key: String::new(),
                    label: "Closed".into(),
                    count: fmt::plural(total, "terminal", "terminals"),
                    open,
                    closed: true,
                    rows: if open { shown.iter().take(50).map(|x| closed_row_vm(x, c)).collect() } else { Vec::new() },
                }
            })
        })
        .flatten();
    ListVm { loading: None, stale_btn, bulk, groups, empty, closed }
}

#[derive(Clone, Debug)]
pub struct HeaderVm {
    /// "Back to T12" when the reader came from a task.
    pub back: Option<String>,
    pub count: String,
    pub filters: Vec<(&'static str, String, usize)>,
    pub pressed: &'static str,
}

#[cfg(test)]
impl HeaderVm {
    pub fn text(&self) -> String {
        let mut p = vec!["Sessions".to_string(), self.count.clone()];
        p.extend(self.filters.iter().map(|(_, l, n)| format!("{l} {n}")));
        squash(&p.iter().map(String::as_str).collect::<Vec<_>>())
    }
}

/// `ssHeader`.
pub fn header_vm(live: Option<&[Value]>, filter: &str, back: Option<&str>) -> HeaderVm {
    let all = live.unwrap_or(&[]);
    let f = if filter.is_empty() { "all" } else { filter };
    HeaderVm {
        back: back.map(|r| format!("Back to {}", fmt::ref_of(&json!(r), "T"))),
        count: if live.is_some() { format!("{} in Midna", fmt::plural(all.len() as i64, "Claude terminal", "Claude terminals")) } else { String::new() },
        filters: FILTERS.iter().map(|(k, l)| (*k, l.to_string(), all.iter().filter(|x| for_filter(x, k)).count())).collect(),
        pressed: FILTERS.iter().find(|(k, _)| *k == f).map(|(k, _)| *k).unwrap_or("all"),
    }
}

/// `ssMenuItems`: (key, label, danger).
pub fn menu_items(x: &Value, selecting: bool, picked: &[String]) -> Vec<(&'static str, &'static str, bool)> {
    if s(x, "status") == "gone" || opt_s(x, "closed_at").is_some() {
        return if b(x, "can_reopen") { vec![("reopen", "Reopen its conversation", false)] } else { Vec::new() };
    }
    let mut items = vec![("open", "Open", false), ("focus", "Show in Midna", false), ("rename", "Rename…", false)];
    if bulkable(x) {
        items.push(("select", if selecting && picked.iter().any(|p| p == s(x, "id")) { "Deselect" } else { "Select" }, false));
    }
    if !b(x, "closing") && matches!(s(x, "close"), "close" | "force") {
        items.push(("close", if s(x, "close") == "force" { "Force close…" } else { "Close terminal…" }, true));
    }
    items
}

/// What the detail pane is drawn from.
pub struct DetailCtx<'a> {
    pub sel: Option<&'a str>,
    /// `SS.detailErr`.
    pub err: Option<&'a str>,
    /// `GET /sessions/:sel`, once it answered for this selection.
    pub detail: Option<&'a Value>,
    pub closed: &'a [Value],
    pub renaming: bool,
    pub confirm: Option<&'a str>,
    pub note: Option<&'a Note>,
    pub busy: &'a HashSet<String>,
    pub holding: bool,
    pub flash: Option<&'a Flash>,
}

#[derive(Clone, Debug)]
pub struct ActVm {
    pub act: &'static str,
    pub label: String,
    pub busy: bool,
}

#[derive(Clone, Debug)]
pub struct ConfirmVm {
    pub title: String,
    pub text: String,
    pub label: String,
    pub force: bool,
    pub busy: bool,
}

#[derive(Clone, Debug)]
pub enum LinkTarget {
    Task(String),
    Goal(String),
}

#[derive(Clone, Debug)]
pub struct LinkVm {
    pub k: &'static str,
    /// The bold ref ("" for "No task").
    pub r: String,
    pub title: String,
    pub pill: Option<(&'static str, &'static str)>,
    pub go: Option<(&'static str, LinkTarget)>,
}

#[derive(Clone, Debug)]
pub struct BoxesVm {
    pub prompt_when: String,
    pub prompt_text: String,
    pub reply_when: String,
    /// Markdown (rendered with `md`), or None for "Nothing yet.".
    pub reply_text: Option<String>,
    pub waiting: bool,
}

#[derive(Clone, Debug)]
pub struct TimelineVm {
    pub time: String,
    pub full: String,
    pub text: String,
    pub kind: String,
}

#[derive(Clone, Debug)]
pub struct DetailVm {
    pub id: String,
    pub gone: bool,
    pub status: String,
    pub pill: String,
    pub status_line: Option<String>,
    /// "Compacting since 3:05 PM".
    pub compacting: Option<String>,
    /// None while the rename field replaces the title.
    pub title: Option<NameVm>,
    pub path: String,
    pub branch: String,
    pub diff: Option<(String, Option<String>)>,
    pub actions: Vec<ActVm>,
    pub close_note: Option<&'static str>,
    pub note: Option<(String, bool)>,
    pub confirm: Option<ConfirmVm>,
    pub closing: Option<String>,
    pub links: Vec<LinkVm>,
    pub stats: Vec<(String, &'static str)>,
    pub boxes: Option<BoxesVm>,
    pub timeline: Vec<TimelineVm>,
}

pub enum DetailView {
    /// "Pick a terminal…", "Loading…".
    Message(String),
    /// `SS.detailErr` while there's no detail (`note err`).
    Error(String),
    Full(Box<DetailVm>),
}

#[cfg(test)]
impl DetailView {
    pub fn text(&self) -> String {
        let d = match self {
            DetailView::Message(m) | DetailView::Error(m) => return m.clone(),
            DetailView::Full(d) => d,
        };
        let mut p: Vec<String> = vec![d.pill.clone()];
        p.extend(d.status_line.clone());
        p.extend(d.title.as_ref().map(|t| t.text.clone()));
        p.extend([d.path.clone(), d.branch.clone()]);
        p.extend(d.diff.as_ref().map(|(t, _)| t.clone()));
        p.extend(d.actions.iter().map(|a| a.label.clone()));
        p.extend(d.close_note.map(String::from));
        p.extend(d.note.as_ref().map(|(t, _)| t.clone()));
        if let Some(c) = &d.confirm {
            p.extend([c.title.clone(), c.text.clone(), c.label.clone(), "Keep it open".into()]);
        }
        p.extend(d.closing.clone());
        for l in &d.links {
            p.extend([l.k.to_string(), l.r.clone(), l.title.clone()]);
            p.extend(l.pill.map(|(x, _)| x.to_string()));
            p.extend(l.go.as_ref().map(|(g, _)| g.to_string()));
        }
        p.extend(d.stats.iter().map(|(v, k)| format!("{v} {k}")));
        if let Some(bx) = &d.boxes {
            p.extend([bx.prompt_when.clone(), bx.prompt_text.clone(), bx.reply_when.clone(), bx.reply_text.clone().unwrap_or_else(|| "Nothing yet.".into())]);
        }
        p.push("Turns and events".into());
        if d.timeline.is_empty() {
            p.push("Nothing recorded yet. Its turns show up here once it runs with the task board plugin.".into());
        }
        for e in &d.timeline {
            p.extend([e.time.clone(), e.text.clone(), timeline_label(&e.kind).into()]);
        }
        squash(&p.iter().map(String::as_str).collect::<Vec<_>>())
    }

    pub fn acts(&self) -> Vec<&'static str> {
        let DetailView::Full(d) = self else { return Vec::new() };
        let mut a: Vec<&'static str> = d.actions.iter().map(|x| x.act).collect();
        if d.confirm.is_some() {
            a.extend(["ss-close", "ss-cancel"]);
        }
        a
    }
}

fn timeline_label(kind: &str) -> &'static str {
    match kind {
        "prompt" => "Prompt",
        "reply" => "Reply",
        "turn" => "Turn",
        "commit" => "Commit",
        "checkpoint" => "Checkpoint",
        "found" => "Issue",
        "ask" => "Question",
        "wait" => "Waiting",
        "compact" => "Compacted",
        "start" => "Started",
        "end" => "Ended",
        "take" => "Task",
        "added" => "Added",
        "done" => "Done",
        "fail" => "Failed",
        "rename" => "Renamed",
        "close" | "close_failed" => "Close",
        "closed" => "Closed",
        _ => "",
    }
}

/// `ssDetail`.
pub fn detail_vm(c: &DetailCtx) -> DetailView {
    let Some(sel) = c.sel else { return DetailView::Message("Pick a terminal to see what it’s done.".into()) };
    let Some(d) = c.detail.filter(|d| s(d, "id") == sel) else {
        return match c.err {
            Some(e) => DetailView::Error(e.to_string()),
            None => DetailView::Message("Loading…".into()),
        };
    };
    let id = s(d, "id").to_string();
    let gone = s(d, "status") == "gone";
    let status = display_status(d).to_string();
    let closing = b(d, "closing");
    let closed_row = c.closed.iter().find(|x| s(x, "id") == id);
    let busy = |k: String| c.busy.contains(&k);

    let mut actions = Vec::new();
    let mut close_note = None;
    if gone {
        if closed_row.is_some_and(|r| b(r, "can_reopen")) {
            let bz = busy(format!("ss-reopen::{id}"));
            actions.push(ActVm { act: "ss-reopen", label: if bz { "Sending…".into() } else { "Reopen its conversation".into() }, busy: bz });
        }
        close_note = Some(if closed_row.is_some_and(|r| !b(r, "can_reopen")) {
            "The board doesn’t know enough about this conversation to reopen it."
        } else {
            "Reopening starts a new Midna terminal with this conversation."
        });
    } else if !closing && c.confirm != Some(id.as_str()) {
        let bz = busy(format!("ss-focus::{id}"));
        actions.push(ActVm { act: "ss-focus", label: if bz { "Sending…".into() } else { "Show in Midna".into() }, busy: bz });
        match s(d, "close") {
            "close" => actions.push(ActVm { act: "ss-ask", label: "Close terminal".into(), busy: false }),
            "force" => {
                let bz = busy(format!("ss-hold:force:{id}"));
                let label = if bz { "Sending…" } else if c.holding { "Keep holding…" } else { "Force close" };
                actions.push(ActVm { act: "ss-hold", label: label.into(), busy: bz });
            }
            _ => {}
        }
    }
    let confirm = (c.confirm == Some(id.as_str()) && !closing).then(|| {
        let (title, text, label) = close_copy(d);
        let force = s(d, "close") == "force";
        let bz = busy(format!("ss-close:{}:{id}", if force { "force" } else { "" }));
        ConfirmVm { title, text, label: if bz { "Sending…".into() } else { label.into() }, force, busy: bz }
    });

    let mut links = Vec::new();
    if let Some(t) = obj(d, "task") {
        let r = s(t, "ref").to_string();
        links.push(LinkVm { k: "Task", r: r.clone(), title: s(t, "title").into(), pill: Some(task_pill(t)), go: Some(("Open task", LinkTarget::Task(r))) });
        if let Some(g) = obj(t, "goal") {
            let gr = s(g, "ref").to_string();
            links.push(LinkVm { k: "Goal", r: gr.clone(), title: s(g, "name").into(), pill: None, go: Some(("Open goal", LinkTarget::Goal(gr))) });
        }
    } else {
        let last = obj(d, "last_task").map(|lt| {
            let r = s(lt, "ref").to_string();
            LinkVm { k: "Last task", r: r.clone(), title: s(lt, "title").into(), pill: None, go: Some(("Open task", LinkTarget::Task(r))) }
        });
        if !gone {
            let title = opt_s(d, "role").unwrap_or("No task").to_string();
            links.push(LinkVm { k: "Task", r: String::new(), title, pill: None, go: None });
        }
        links.extend(last);
    }
    for l in arr(d, "line") {
        let r = s(l, "ref").to_string();
        let k = if s(&l["line"], "kind") == "resume" { "To resume" } else { "Queued here" };
        links.push(LinkVm { k, r: r.clone(), title: s(l, "title").into(), pill: None, go: Some(("Open task", LinkTarget::Task(r))) });
    }

    let st = &d["stats"];
    let stats = vec![
        (i(st, "turns").to_string(), "Turns"),
        (i(st, "commits").to_string(), "Commits"),
        (i(st, "files").to_string(), "Files edited"),
        (opt_s(d, "last_activity").map(fmt::ago).unwrap_or_else(|| "—".into()), "Last activity"),
    ];
    let prompt = obj(d, "prompt");
    let now_box = match (obj(d, "waiting"), obj(d, "reply")) {
        (Some(w), _) => Some((format!("Waiting since {}", fmt::hhmm(s(w, "at"))), s(w, "text").to_string(), true)),
        (None, Some(r)) => Some((format!("{} reply · {}", if status == "working" { "Latest" } else { "Last" }, fmt::hhmm(s(r, "at"))), s(r, "text").to_string(), false)),
        _ => None,
    };
    let boxes = (prompt.is_some() || now_box.is_some()).then(|| BoxesVm {
        prompt_when: prompt.map(|p| format!("Last prompt · {}", fmt::hhmm(s(p, "at")))).unwrap_or_else(|| "Last prompt".into()),
        prompt_text: prompt.map(|p| one(s(p, "text"), 500)).unwrap_or_else(|| "Nothing yet.".into()),
        reply_when: now_box.as_ref().map(|n| n.0.clone()).unwrap_or_else(|| "Last reply".into()),
        reply_text: now_box.as_ref().map(|n| n.1.clone()),
        waiting: now_box.as_ref().is_some_and(|n| n.2),
    });
    let timeline = arr(d, "timeline")
        .iter()
        .map(|e| {
            let files = match i(e, "files") {
                0 => String::new(),
                f => format!(" · {} edited", fmt::plural(f, "file", "files")),
            };
            TimelineVm { time: fmt::hhmm(s(e, "at")), full: fmt::full_time(s(e, "at")), text: format!("{}{files}", one(s(e, "text"), 260)), kind: s(e, "kind").to_string() }
        })
        .collect();

    DetailView::Full(Box::new(DetailVm {
        pill: if closing {
            "Closing".into()
        } else if gone {
            "Closed".into()
        } else {
            state_label(d, &status)
        },
        status_line: (!closing).then(|| status_line(d)),
        compacting: fmt::compacting(d).filter(|_| !closing),
        title: (!c.renaming).then(|| name_vm(d, if gone { "" } else { DETAIL_HINT }, c.flash)),
        path: opt_s(d, "project_path").map(short_path).unwrap_or_else(|| s(d, "project").to_string()),
        branch: s(d, "branch").to_string(),
        diff: if gone { None } else { diff_vm(d) },
        actions,
        close_note,
        note: c.note.map(|n| (n.text.clone(), n.err)),
        confirm,
        closing: closing.then(|| format!("Asked Midna to close {}. It moves to Closed once it’s gone.", s(d, "name"))),
        links,
        stats,
        boxes,
        timeline,
        gone,
        status,
        id,
    }))
}

#[derive(Clone, Debug)]
pub struct BulkVm {
    pub title: String,
    pub lines: Vec<String>,
    pub go: String,
    pub note: Option<(String, bool)>,
    pub busy: bool,
}

pub const BULK_TEXT: &str = "They’re idle with no task. The board keeps each one’s conversation, so you can reopen it later. Midna may ask you to confirm on the Mac.";

#[cfg(test)]
impl BulkVm {
    pub fn text(&self) -> String {
        let mut p = vec![self.title.clone(), BULK_TEXT.into()];
        p.extend(self.lines.clone());
        p.extend(self.note.as_ref().map(|(t, _)| t.clone()));
        p.extend(["Keep them open".into(), self.go.clone()]);
        squash(&p.iter().map(String::as_str).collect::<Vec<_>>())
    }
}

/// `ssBulkConfirm`: the picks in the order they were made.
pub fn bulk_vm(picked: &[String], live: &[Value], closed: &[Value], note: Option<&Note>, busy: bool) -> BulkVm {
    let rows: Vec<&Value> = picked.iter().filter_map(|id| live.iter().chain(closed.iter()).find(|x| s(x, "id") == id)).collect();
    let n = rows.len();
    BulkVm {
        title: if n == 1 { "Close 1 idle terminal?".into() } else { format!("Close {n} idle terminals?") },
        lines: rows.iter().map(|x| format!("{} · {} · idle for {}", s(x, "name"), s(x, "project"), long_ago(idle_ms(x)))).collect(),
        go: if busy { "Sending…".into() } else { close_label(n) },
        note: note.map(|n| (n.text.clone(), n.err)),
        busy,
    }
}

// ------------------------------------------------------------------ data with local overrides

fn live_list(m: &MainWindow) -> Vec<Value> {
    m.data.sessions.as_ref().map(|v| arr(v, "sessions").iter().map(|x| effective(&m.sessions, x)).collect()).unwrap_or_default()
}

fn closed_list(m: &MainWindow) -> Vec<Value> {
    m.data.closed.as_ref().map(|v| arr(v, "sessions").to_vec()).unwrap_or_default()
}

fn closed_total(m: &MainWindow) -> i64 {
    m.data.closed.as_ref().map(|v| i(v, "total")).unwrap_or(0)
}

fn detail_data(m: &MainWindow) -> Option<Value> {
    m.data.session.as_ref().map(|d| effective(&m.sessions, d))
}

/// `ssSession`: a live row, else a closed one.
fn find_session(m: &MainWindow, id: &str) -> Option<Value> {
    live_list(m).into_iter().chain(closed_list(m)).find(|x| s(x, "id") == id)
}

/// The row with the rename the board hasn't caught up with yet applied.
fn effective(st: &State, x: &Value) -> Value {
    match st.pending.get(s(x, "id")) {
        Some(p) => {
            let mut v = x.clone();
            if let Some(r) = &p.renaming {
                v["renaming"] = json!(r);
            }
            if let Some(e) = &p.error {
                v["renaming"] = Value::Null;
                v["rename_error"] = json!(e);
            }
            v
        }
        None => x.clone(),
    }
}

/// Drop overrides once the board answered something new about that session (or after 4 s).
fn retire_pending(m: &mut MainWindow) {
    if m.sessions.pending.is_empty() {
        return;
    }
    let rows: Vec<Value> = m.data.sessions.as_ref().map(|v| arr(v, "sessions").to_vec()).unwrap_or_default();
    m.sessions.pending.retain(|id, p| {
        let now = rows.iter().find(|x| s(x, "id") == id).map(|x| x.to_string()).unwrap_or_default();
        p.at.elapsed() < Duration::from_secs(4) && now == p.seen
    });
}

fn board_row(m: &MainWindow, id: &str) -> String {
    m.data.sessions.as_ref().and_then(|v| arr(v, "sessions").iter().find(|x| s(x, "id") == id).map(|x| x.to_string())).unwrap_or_default()
}

fn query(m: &MainWindow, cx: &App) -> String {
    m.sessions.search.as_ref().map(|i| i.text(cx)).unwrap_or_default()
}

// ------------------------------------------------------------------ actions

/// Busy-key the web builds from a button (`act:arg:id`).
fn key(act: &str, arg: &str, id: &str) -> String {
    format!("{act}:{arg}:{id}")
}

fn set_note(m: &mut MainWindow, grp: String, text: String, err: bool, cx: &mut Context<MainWindow>) {
    let at = Instant::now();
    m.sessions.notes.insert(grp.clone(), Note { text, err, at });
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(if err { NOTE_ERR } else { NOTE_OK }).await;
        let _ = this.update(cx, |m, cx| {
            if m.sessions.notes.get(&grp).is_some_and(|n| n.at == at) {
                m.sessions.notes.remove(&grp);
                cx.notify();
            }
        });
    })
    .detach();
}

/// The web's `run(el, …)`: mark the button busy, clear its group's note, POST, then show the
/// result in the group's note (or a toast without a group).
fn run(
    m: &mut MainWindow,
    busy: String,
    grp: Option<String>,
    path: String,
    body: Value,
    cx: &mut Context<MainWindow>,
    ok: impl FnOnce(&mut MainWindow, &Value, &mut Context<MainWindow>) -> Option<String> + 'static,
) {
    if !m.sessions.busy.insert(busy.clone()) {
        return;
    }
    if let Some(g) = &grp {
        m.sessions.notes.remove(g);
    }
    let (b2, g2) = (busy.clone(), grp.clone());
    m.post_or(
        path,
        body,
        cx,
        move |m, v, cx| {
            m.sessions.busy.remove(&busy);
            if let Some(text) = ok(m, &v, cx).filter(|t| !t.is_empty()) {
                match grp {
                    Some(g) => set_note(m, g, text, false, cx),
                    None => m.toast(text, false, cx),
                }
            }
            cx.notify();
        },
        move |m, e, cx| {
            m.sessions.busy.remove(&b2);
            match g2 {
                Some(g) => set_note(m, g, e.message, true, cx),
                None => m.toast(e.message, true, cx),
            }
            cx.notify();
        },
    );
}

/// `ss-open` (and the menu's Open): show this terminal on the right.
pub fn open(m: &mut MainWindow, id: &str, cx: &mut Context<MainWindow>) {
    m.sessions.confirm = None;
    if m.sessions.selected.as_deref() != Some(id) {
        m.sessions.selected = Some(id.to_string());
        m.data.session = None;
        m.refresh(cx);
    }
    cx.notify();
}

/// A click on a row: toggles its pick while picking (if it can be picked), else opens it.
pub fn row_click(m: &mut MainWindow, id: &str, cx: &mut Context<MainWindow>) {
    let can = m.sessions.selecting && find_session(m, id).is_some_and(|x| bulkable(&x));
    if can {
        toggle_pick(m, id, cx);
    } else {
        open(m, id, cx);
    }
}

pub fn set_filter(m: &mut MainWindow, f: &str, cx: &mut Context<MainWindow>) {
    m.sessions.confirm = None;
    m.sessions.filter = if f == "all" { String::new() } else { f.to_string() };
    cx.notify();
}

/// `startRename` (the detail title's double-click, the menu's Rename…).
pub fn start_rename(m: &mut MainWindow, id: &str, window: &mut Window, cx: &mut Context<MainWindow>) {
    let cur = find_session(m, id).or_else(|| detail_data(m).filter(|d| s(d, "id") == id));
    let Some(cur) = cur else { return };
    let name = opt_s(&cur, "renaming").or(opt_s(&cur, "name")).unwrap_or("").to_string();
    let input = kit::Input::with_text(cx, "New name in Midna", false, &name);
    input.field.update(cx, |f, cx| f.select_all(cx));
    window.focus(&input.focus, cx);
    // Clicking away saves it, as leaving the web board's field did.
    let sub = cx.on_blur(&input.focus, window, |m, window, cx| end_rename(m, true, window, cx));
    m.sessions.rename = Some((id.to_string(), input));
    m.sessions.rename_sub = Some(sub);
    cx.notify();
}

/// `endRename`: ↩ or leaving the field saves, esc drops it. Nothing is sent when the name is empty
/// or unchanged (compared with a pending rename first).
pub fn end_rename(m: &mut MainWindow, save: bool, window: &mut Window, cx: &mut Context<MainWindow>) {
    let Some((id, input)) = m.sessions.rename.take() else { return };
    m.sessions.rename_sub = None;
    let name: String = input.text(cx).trim().chars().take(80).collect();
    if input.focus.is_focused(window) {
        window.focus(&m.focus, cx);
    }
    let cur = find_session(m, &id).or_else(|| detail_data(m).filter(|d| s(d, "id") == id));
    let was = cur.as_ref().map(|c| opt_s(c, "renaming").or(opt_s(c, "name")).unwrap_or("").to_string());
    cx.notify();
    if !save || name.is_empty() || was.is_none() || was.as_deref() == Some(name.as_str()) {
        return;
    }
    send_rename(m, &id, name, cx);
}

fn send_rename(m: &mut MainWindow, id: &str, name: String, cx: &mut Context<MainWindow>) {
    let seen = board_row(m, id);
    m.sessions.pending.insert(id.to_string(), Pending { renaming: Some(name.clone()), error: None, seen, at: Instant::now() });
    m.sessions.watch.insert(id.to_string(), name.clone());
    let (rid, rid2) = (id.to_string(), id.to_string());
    m.post_or(
        format!("sessions/{id}/rename"),
        json!({"name": name}),
        cx,
        move |m, _, _| {
            // Keep showing the new name until the board's answer (fetched next) says otherwise.
            let seen = board_row(m, &rid);
            if let Some(p) = m.sessions.pending.get_mut(&rid) {
                p.seen = seen;
                p.at = Instant::now();
            }
        },
        move |m, e, cx| {
            let seen = board_row(m, &rid2);
            m.sessions.pending.insert(rid2.clone(), Pending { renaming: None, error: Some(e.message), seen, at: Instant::now() });
            cx.notify();
        },
    );
}

/// The menu's "Show in Midna": a toast.
pub fn menu_focus(m: &mut MainWindow, id: &str, cx: &mut Context<MainWindow>) {
    let name = find_session(m, id).map(|x| s(&x, "name").to_string()).unwrap_or_else(|| "it".into());
    m.post(format!("sessions/{id}/focus"), json!({}), cx, move |m, _, cx| m.toast(format!("Showing {name} in Midna"), false, cx));
}

/// The detail's "Show in Midna": "Sent to Midna" under the buttons.
pub fn focus(m: &mut MainWindow, id: &str, cx: &mut Context<MainWindow>) {
    run(m, key("ss-focus", "", id), Some(format!("ss:{id}")), format!("sessions/{id}/focus"), json!({}), cx, |m, _, _| Some(sent_note(m.state()).into()));
}

/// The confirmation's Close terminal / Force close.
pub fn close(m: &mut MainWindow, id: &str, force: bool, cx: &mut Context<MainWindow>) {
    let k = key("ss-close", if force { "force" } else { "" }, id);
    run(m, k, Some(format!("ss:{id}")), format!("sessions/{id}/close"), json!({"force": force}), cx, |m, _, _| {
        m.sessions.confirm = None;
        None
    });
}

/// The press-and-hold Force close, once held long enough.
pub fn hold_fire(m: &mut MainWindow, id: &str, cx: &mut Context<MainWindow>) {
    run(m, key("ss-hold", "force", id), Some(format!("ss:{id}")), format!("sessions/{id}/close"), json!({"force": true}), cx, |_, _, _| None);
}

/// The detail's "Reopen its conversation".
pub fn reopen(m: &mut MainWindow, id: &str, cx: &mut Context<MainWindow>) {
    run(m, key("ss-reopen", "", id), Some(format!("ss:{id}")), format!("sessions/{id}/reopen"), json!({}), cx, |_, _, _| {
        Some("Asked Midna to reopen it in a new terminal.".into())
    });
}

/// The menu's "Reopen its conversation": a toast.
pub fn menu_reopen(m: &mut MainWindow, id: &str, cx: &mut Context<MainWindow>) {
    m.post(format!("sessions/{id}/reopen"), json!({}), cx, |m, _, cx| m.toast("Asked Midna to reopen it in a new terminal.", false, cx));
}

/// `ss-bulk-go`: close the picks (in pick order); picking mode stays on.
pub fn bulk_close(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let ids = m.sessions.picked.clone();
    run(m, key("ss-bulk-go", "", ""), Some("ss-bulk".into()), "sessions/close".into(), json!({"ids": ids}), cx, |m, r, cx| {
        m.sessions.bulk = false;
        m.sessions.picked.clear();
        let n = arr(r, "closing").len() as i64;
        let skipped = arr(r, "skipped").len();
        let extra = if skipped > 0 { format!("; {skipped} got busy or picked up a task, so they stay open") } else { String::new() };
        m.toast(format!("Asked Midna to close {}{extra}. Confirm on the Mac.", fmt::plural(n, "terminal", "terminals")), false, cx);
        None
    });
}

pub fn toggle_pick(m: &mut MainWindow, id: &str, cx: &mut Context<MainWindow>) {
    let p = &mut m.sessions.picked;
    match p.iter().position(|x| x == id) {
        Some(i) => {
            p.remove(i);
        }
        None => p.push(id.to_string()),
    }
    cx.notify();
}

/// `ss-stale`: pick every idle-over-an-hour terminal with no task.
pub fn pick_stale(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    m.sessions.selecting = true;
    for x in live_list(m).iter().filter(|x| bulkable(x) && is_stale(x)) {
        let id = s(x, "id").to_string();
        if !m.sessions.picked.contains(&id) {
            m.sessions.picked.push(id);
        }
    }
    cx.notify();
}

/// `ss-done`.
pub fn done_picking(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    m.sessions.selecting = false;
    m.sessions.picked.clear();
    cx.notify();
}

fn load_collapsed(st: &mut State) {
    if st.collapsed_loaded {
        return;
    }
    st.collapsed_loaded = true;
    st.collapsed = crate::prefs::get(COLLAPSED_KEY).and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
}

/// `ss-group`: collapse or open a project group (saved as `tb.sessCollapsed`). `open` is how it
/// was drawn (a search or the selection can hold a collapsed group open).
pub fn toggle_group(m: &mut MainWindow, project: &str, open: bool, cx: &mut Context<MainWindow>) {
    load_collapsed(&mut m.sessions);
    let st = &mut m.sessions;
    if open {
        if !st.collapsed.iter().any(|p| p == project) {
            st.collapsed.push(project.to_string());
        }
        st.hide_sel = st.selected.clone();
    } else {
        st.collapsed.retain(|p| p != project);
    }
    crate::prefs::set(COLLAPSED_KEY, json!(st.collapsed));
    cx.notify();
}

/// `ss-closed`.
pub fn toggle_closed(m: &mut MainWindow, open: bool, cx: &mut Context<MainWindow>) {
    m.sessions.closed_open = !open;
    if open {
        m.sessions.hide_sel = m.sessions.selected.clone();
    }
    cx.notify();
}

/// Esc on the Sessions page (`sessions.js` keydown): closes the bulk dialog or a Close
/// confirmation, else leaves picking mode (unless typing in a field). True when it handled it.
/// (The row menu is closed by the main window first, as the web board did.)
pub fn escape(m: &mut MainWindow, window: &Window, cx: &mut Context<MainWindow>) -> bool {
    if m.page != Page::Sessions {
        return false;
    }
    let st = &mut m.sessions;
    if st.bulk || st.confirm.is_some() {
        st.bulk = false;
        st.confirm = None;
        cx.notify();
        return true;
    }
    let typing = st.search.as_ref().is_some_and(|i| i.focus.is_focused(window)) || st.rename.as_ref().is_some_and(|(_, i)| i.focus.is_focused(window));
    if st.selecting && !typing {
        st.selecting = false;
        st.picked.clear();
        cx.notify();
        return true;
    }
    false
}

/// Show a terminal from a task ("Back to T12" leads back to it), like the web board's terminal links.
pub fn open_from_task(m: &mut MainWindow, session_id: &str, task_ref: &str, cx: &mut Context<MainWindow>) {
    m.sessions.back = Some(task_ref.to_string());
    m.go(Page::Sessions, cx);
    open(m, session_id, cx);
}

/// The page was left (the web route lost its `back=`).
pub fn left(m: &mut MainWindow) {
    m.sessions.back = None;
}

/// Press-and-hold: after `HOLD` with the button still down (and the pointer still on it), force close.
fn hold_start(m: &mut MainWindow, id: &str, cx: &mut Context<MainWindow>) {
    if m.sessions.hold.is_some() || m.sessions.busy.contains(&key("ss-hold", "force", id)) {
        return;
    }
    let started = Instant::now();
    m.sessions.hold = Some((id.to_string(), started));
    let id = id.to_string();
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(Duration::from_millis(30)).await;
            let go = this.update(cx, |m, cx| {
                if !matches!(&m.sessions.hold, Some((h, at)) if *h == id && *at == started) {
                    return false;
                }
                if started.elapsed() >= HOLD {
                    m.sessions.hold = None;
                    hold_fire(m, &id, cx);
                    return false;
                }
                cx.notify();
                true
            });
            if !matches!(go, Ok(true)) {
                break;
            }
        }
    })
    .detach();
    cx.notify();
}

fn hold_end(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    if m.sessions.hold.take().is_some() {
        cx.notify();
    }
}

/// With nothing selected, show the first terminal of the first open group (`afterSessionsLoad`);
/// one picked from a collapsed group leaves the group collapsed.
pub fn auto_select(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    if m.sessions.selected.is_some() || m.data.sessions.is_none() {
        return;
    }
    load_collapsed(&mut m.sessions);
    let live = live_list(m);
    let q = query(m, cx);
    let f = if m.sessions.filter.is_empty() { "all".to_string() } else { m.sessions.filter.clone() };
    let groups = grouped(live.iter().filter(|x| for_filter(x, &f) && matches(x, &q)).collect());
    let pick = groups.iter().find(|(p, _)| !m.sessions.collapsed.contains(p)).or(groups.first()).map(|(p, v)| (p.clone(), s(v[0], "id").to_string()));
    if let Some((proj, id)) = pick {
        if m.sessions.collapsed.contains(&proj) {
            m.sessions.hide_sel = Some(id.clone());
        }
        m.sessions.selected = Some(id);
        m.refresh(cx);
    }
}

// ------------------------------------------------------------------ render

/// Watch renames finish (✓ / ✗ flash), expire flashes, and retire local overrides.
fn tick_renames(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    retire_pending(m);
    let mut rows = live_list(m);
    rows.extend(detail_data(m));
    let mut started = false;
    for x in &rows {
        started |= observe_rename(&mut m.sessions.watch, &mut m.sessions.flash, x);
    }
    m.sessions.flash.retain(|_, f| f.at.elapsed() < if f.ok { FLASH_OK } else { FLASH_BAD });
    if started {
        cx.spawn(async move |this, cx| {
            for wait in [FLASH_OK, FLASH_BAD - FLASH_OK] {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();
    }
}

// ------------------------------------------------------------------ look (app.css values)

/// `--accent-tint` (not in the shared theme).
fn accent_tint(t: &Theme) -> Hsla {
    if matches!(t.mode, ThemeMode::Dark) { rgb(0x172036).into() } else { rgb(0xf5f8ff).into() }
}

/// `--seen` (the timeline's "Issue" dot).
fn seen(t: &Theme) -> Hsla {
    if matches!(t.mode, ThemeMode::Dark) { rgb(0xb9764f).into() } else { rgb(0xe39a6f).into() }
}

/// The web's 24×24 stroke icons (`ICON.chev`, `ICON.search`, `ICON.back`).
#[derive(Clone, Copy)]
enum Glyph {
    ChevDown,
    ChevRight,
    Search,
    Back,
}

fn glyph(g: Glyph, color: Hsla, size: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let k = bounds.size.width.as_f32() / 24.;
            let u = |x: f32, y: f32| point(bounds.origin.x + px(x * k), bounds.origin.y + px(y * k));
            let line = |pts: &[(f32, f32)], window: &mut Window| {
                let mut p = PathBuilder::stroke(px(2.2 * k));
                p.move_to(u(pts[0].0, pts[0].1));
                for (x, y) in &pts[1..] {
                    p.line_to(u(*x, *y));
                }
                if let Ok(p) = p.build() {
                    window.paint_path(p, color);
                }
            };
            match g {
                Glyph::ChevDown => line(&[(6., 9.), (12., 15.), (18., 9.)], window),
                Glyph::ChevRight => line(&[(9., 6.), (15., 12.), (9., 18.)], window),
                Glyph::Back => line(&[(15., 6.), (9., 12.), (15., 18.)], window),
                Glyph::Search => {
                    let pts: Vec<(f32, f32)> = (0..=32).map(|n| {
                        let a = 2. * std::f32::consts::PI * n as f32 / 32.;
                        (11. + 6.5 * a.cos(), 11. + 6.5 * a.sin())
                    }).collect();
                    line(&pts, window);
                    line(&[(16., 16.), (20.5, 20.5)], window);
                }
            }
        },
    )
    .size(px(size))
    .flex_none()
}

/// `.pill` (`sm`: `.pill.sm`): 12px semibold on a soft fill.
fn pill_el(fg: Hsla, bg: Hsla, label: impl Into<SharedString>, sm: bool) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .px(px(if sm { 8. } else { 10. }))
        .py(px(if sm { 1. } else { 2. }))
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_size(px(12.))
        .line_height(px(18.))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .child(label.into())
}

/// `.pill.st-{idle,working,needs,gone}` (plus the native-only `offline` and `waiting`).
fn sess_pill(t: &Theme, status: &str, label: String) -> Div {
    let (fg, bg) = match status {
        "working" | "waiting" => (t.accent_fg, t.accent_soft),
        "needs" => (t.warn_fg, t.warn_soft),
        "offline" => (t.down, t.down_soft),
        "gone" => (t.muted, t.panel_2),
        _ => (t.text_2, t.panel_2),
    };
    pill_el(fg, bg, label, false)
}

/// `.pill.sm.st-{task status}`, by its label.
fn task_st_pill(t: &Theme, label: &'static str) -> Div {
    let p = |fg, bg| pill_el(fg, bg, label, true);
    match label {
        "Working" => p(t.accent_fg, t.accent_soft),
        "Needs you" | "Blocked" => p(t.warn_fg, t.warn_soft),
        "Done" => p(t.up_fg, t.up_soft),
        "Failed" => p(t.down, t.down_soft),
        "Planned" => p(t.muted, t.card).border_1().border_color(t.border),
        _ => p(t.text_2, t.col),
    }
}

/// `.btn` (36px, 13px, 0 14px, radius 8) without its hover; each tone sets its own.
fn btn_base(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(36.))
        .px(px(14.))
        .rounded(px(8.))
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .text_color(t.text)
        .text_size(px(13.))
        .whitespace_nowrap()
        .cursor_pointer()
        .child(label.into())
}

/// `.btn.soft`.
fn soft_btn(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let fill = t.accent_soft;
    btn_base(t, id, label).text_color(t.accent).bg(fill).border_color(transparent_black()).font_weight(FontWeight::SEMIBOLD).hover(move |s| s.border_color(fill))
}

/// `.btn.danger`.
fn danger_btn(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let hover = t.border_2;
    btn_base(t, id, label).text_color(t.down).border_color(t.down_line).hover(move |s| s.border_color(hover))
}

/// `.btn.danger.solid`.
fn solid_danger_btn(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let fill = t.down;
    btn_base(t, id, label).text_color(gpui_kit::white()).bg(fill).border_color(transparent_black()).font_weight(FontWeight::SEMIBOLD).hover(move |s| s.border_color(fill).opacity(0.94))
}

/// `.btn.ghost`.
fn ghost_btn(t: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let hover = t.text;
    btn_base(t, id, label).text_color(t.muted).bg(transparent_black()).border_color(transparent_black()).px(px(10.)).hover(move |s| s.border_color(transparent_black()).text_color(hover))
}

/// `.btn.sm` on top of any of the above.
fn sm(b: Stateful<Div>) -> Stateful<Div> {
    b.h(px(32.)).text_size(px(12.5)).px(px(10.)).rounded(px(7.))
}

/// `.h3`: 13px semibold uppercase muted.
fn h3(t: &Theme, text: &str) -> Div {
    div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.muted).whitespace_nowrap().child(text.to_uppercase())
}

/// `.none-yet`: 13px muted.
fn none_yet(t: &Theme, text: impl Into<SharedString>) -> Div {
    div().py(px(10.)).text_size(px(13.)).text_color(t.muted).child(text.into())
}

/// `.slist` / `.sdetail`: the two cards.
fn page_card(t: &Theme) -> Div {
    div().flex().flex_col().min_h_0().rounded(px(14.)).border_1().border_color(t.border).bg(t.card)
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    if m.sessions.search.is_none() {
        let input = kit::Input::new(cx, "Search by terminal, project, task or branch", false);
        cx.subscribe(&input.field, |_, _, _: &FieldChanged, cx| cx.notify()).detach();
        m.sessions.search = Some(input);
    }
    load_collapsed(&mut m.sessions);
    // Drop picks that are gone or can't be closed together any more (`loadSessionList`).
    let live = live_list(m);
    let closed = closed_list(m);
    m.sessions.picked.retain(|id| live.iter().chain(closed.iter()).any(|x| s(x, "id") == id && bulkable(x)));
    tick_renames(m, cx);
    auto_select(m, cx);

    // `.spage`: 24px 32px 28px, 18px between the header and the grid; body text 14px / 1.5.
    div()
        .relative()
        .flex()
        .flex_col()
        .gap(px(18.))
        .size_full()
        .min_w_0()
        .pt(px(24.))
        .px(px(32.))
        .pb(px(28.))
        .text_size(px(14.))
        .line_height(relative(1.5))
        .text_color(t.text)
        .child(header(m, &t, cx))
        .child(div().flex().gap(px(20.)).flex_1().min_h_0().child(list(m, &t, window, cx)).child(detail(m, &t, window, cx)))
        .children(row_menu(m, &t, cx))
        .children(bulk_dialog(m, &t, cx))
        .into_any_element()
}

fn header(m: &mut MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Div {
    let live = m.data.sessions.as_ref().map(|_| live_list(m));
    let vm = header_vm(live.as_deref(), &m.sessions.filter, m.sessions.back.as_deref());
    // `.seg.sm`: 3px padding, 2px gaps, radius 9; 32px semibold buttons with the count in faint.
    let mut seg = div().flex().flex_none().items_center().gap(px(2.)).p(px(3.)).rounded(px(9.)).bg(t.seg);
    for (idx, (k, label, n)) in vm.filters.iter().enumerate() {
        let on = *k == vm.pressed;
        let hover = t.text;
        let item = div()
            .id(SharedString::from(format!("ss-filter-{idx}")))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(32.))
            .px(px(11.))
            .rounded(px(7.))
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .whitespace_nowrap()
            .cursor_pointer()
            .text_color(if on { t.text } else { t.muted })
            .when(on, |d| d.bg(t.card).shadow(vec![BoxShadow { color: hsla(0., 0., 0., if matches!(t.mode, ThemeMode::Dark) { 0.4 } else { 0.10 }), offset: point(px(0.), px(1.)), blur_radius: px(2.), spread_radius: px(0.), inset: false }]))
            .hover(move |s| s.text_color(hover))
            .child(label.to_string())
            .child(div().font_weight(FontWeight::MEDIUM).text_color(if on { t.muted } else { t.faint }).child(n.to_string()))
            .on_click(cx.listener(move |m, _, _, cx| set_filter(m, FILTERS[idx].0, cx)));
        seg = seg.child(item);
    }
    // `.back` (only when it came from a task; the rail is the way back to the board).
    let back = vm.back.clone().zip(m.sessions.back.clone()).map(|(label, r)| {
        let (muted, hover) = (t.muted, t.text);
        div()
            .id("ss-back")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(32.))
            .text_size(px(13.))
            .text_color(muted)
            .cursor_pointer()
            .hover(move |s| s.text_color(hover))
            .child(glyph(Glyph::Back, muted, 16.))
            .child(label)
            .on_click(cx.listener(move |m, _, _, cx| {
                m.sessions.back = None;
                m.go(Page::Board, cx);
                m.open_task(r.clone(), cx);
            }))
    });
    // `.top`: 14px column gap; the h1 is 20px bold, 4px in.
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(14.))
        .children(back)
        .child(div().ml(px(4.)).text_size(px(20.)).font_weight(FontWeight::BOLD).whitespace_nowrap().child("Sessions"))
        .child(div().text_color(t.muted).whitespace_nowrap().child(vm.count))
        .child(div().flex_1())
        .child(seg)
}

fn list(m: &mut MainWindow, t: &Theme, window: &mut Window, cx: &mut Context<MainWindow>) -> Div {
    let live = m.data.sessions.as_ref().map(|_| live_list(m));
    let closed = closed_list(m);
    let q = query(m, cx);
    let st = &m.sessions;
    let vm = list_vm(&ListCtx {
        live: live.as_deref(),
        list_err: m.data.errs.sessions.as_deref(),
        closed: &closed,
        closed_total: closed_total(m),
        sel: st.selected.as_deref(),
        filter: &st.filter,
        q: &q,
        collapsed: &st.collapsed,
        hide_sel: st.hide_sel.as_deref(),
        selecting: st.selecting,
        picked: &st.picked,
        closed_open: st.closed_open,
        flash: &st.flash,
    });

    // `.slist-search`: 12px 14px 8px, the 16px search icon, then `.input.sm` (34px, 13px, 6px 10px, radius 7).
    let search = m.sessions.search.as_ref().map(|i| i.render(t, "ss-search", window).flex_1().min_h(px(34.)).h(px(34.)).py(px(6.)).px(px(10.)).rounded(px(7.)).text_size(px(13.)));
    let mut top = div().flex().flex_none().items_center().gap(px(8.)).pt(px(12.)).px(px(14.)).pb(px(8.)).child(glyph(Glyph::Search, t.muted, 16.)).children(search);
    if let Some((label, tip)) = vm.stale_btn.clone() {
        top = top.child(sm(soft_btn(t, "ss-stale", label)).tooltip(kit::tip(tip)).on_click(cx.listener(|m, _, _, cx| pick_stale(m, cx))));
    }

    // `.slist-body`: 4px 0 10px.
    let mut body = div().id("ss-list").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().pt(px(4.)).pb(px(10.));
    if let Some(l) = &vm.loading {
        body = body.child(div().p(px(16.)).text_size(px(13.)).text_color(t.muted).child(l.clone()));
    }
    // Groups, then "None right now." (when no group shows), then Closed: the web's order.
    let mut empty = vm.empty.clone().map(|e| div().p(px(16.)).text_size(px(13.)).text_color(t.muted).child(e));
    let mut placed_empty = false;
    for g in vm.groups.iter().chain(vm.closed.iter()) {
        if g.closed && !placed_empty {
            body = body.children(empty.take());
            placed_empty = true;
        }
        let (key, open, is_closed) = (g.key.clone(), g.open, g.closed);
        let id = if is_closed { "ss-closed".to_string() } else { format!("ss-group-{key}") };
        let head = group_head(t, &id, &g.label, &g.count, open).on_click(cx.listener(move |m, _, _, cx| {
            if is_closed {
                toggle_closed(m, open, cx);
            } else {
                toggle_group(m, &key, open, cx);
            }
        }));
        // `.sgroup.open` has 4px below it; `.sgroup.closed` 8px above and a divider.
        let mut group = div().flex().flex_col().flex_none().when(open, |d| d.pb(px(4.))).when(is_closed, |d| d.mt(px(8.)).border_t_1().border_color(t.divider)).child(head);
        for r in &g.rows {
            group = group.child(row(t, r, cx));
        }
        body = body.child(group);
    }
    if !placed_empty {
        body = body.children(empty);
    }

    // `.spage-grid`: minmax(380px, 460px) next to the detail.
    page_card(t)
        .flex_none()
        .w(px(LIST_W))
        .h_full()
        .overflow_hidden()
        .child(top)
        .children(vm.bulk.as_ref().map(|bb| bulk_bar(t, bb, cx)))
        .child(body)
}

/// `.sbulk`: 48px, 10px 14px 12px, a divider under it; tinted once something is picked.
fn bulk_bar(t: &Theme, bb: &BulkBarVm, cx: &mut Context<MainWindow>) -> Div {
    let on = bb.close.is_some();
    div()
        .flex()
        .flex_wrap()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .min_h(px(48.))
        .pt(px(10.))
        .px(px(14.))
        .pb(px(12.))
        .border_b_1()
        .border_color(t.divider)
        .when(on, |d| d.bg(accent_tint(t)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .text_color(if on { t.accent_fg } else { t.muted })
                .when(on, |d| d.font_weight(FontWeight::SEMIBOLD))
                .child(bb.line.clone()),
        )
        .children(bb.add.clone().map(|a| {
            let fg = t.accent;
            div()
                .id("ss-stale-add")
                .flex()
                .flex_none()
                .items_center()
                .h(px(32.))
                .px(px(2.))
                .text_size(px(12.5))
                .text_color(fg)
                .cursor_pointer()
                .hover(|s| s.underline())
                .child(a)
                .on_click(cx.listener(|m, _, _, cx| pick_stale(m, cx)))
        }))
        .child(sm(ghost_btn(t, "ss-done", "Done")).on_click(cx.listener(|m, _, _, cx| done_picking(m, cx))))
        .children(bb.close.clone().map(|c| {
            sm(danger_btn(t, "ss-bulk", c)).on_click(cx.listener(|m, _, _, cx| {
                m.sessions.bulk = true;
                cx.notify();
            }))
        }))
}

/// `.sgroup-h.sgroup-toggle`: 10px 16px, 13px semibold, the 14px chevron, the count in 12px muted.
fn group_head(t: &Theme, id: &str, label: &str, count: &str, open: bool) -> Stateful<Div> {
    let hover = t.panel_2;
    // " · N need(s) you" on a collapsed group is `<b>` in warn.
    let (plain, hot) = match count.split_once(" · ") {
        Some((a, b)) => (a.to_string(), Some(b.to_string())),
        None => (count.to_string(), None),
    };
    div()
        .id(SharedString::from(id.to_string()))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .w_full()
        .py(px(10.))
        .px(px(16.))
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(move |d| d.bg(hover))
        .child(glyph(if open { Glyph::ChevDown } else { Glyph::ChevRight }, t.muted, 14.))
        .child(div().whitespace_nowrap().child(label.to_string()))
        .child(
            div()
                .flex()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.muted)
                .whitespace_nowrap()
                .child(plain)
                .children(hot.map(|h| div().flex().child(" · ").child(div().font_weight(FontWeight::SEMIBOLD).text_color(t.warn).child(h)))),
        )
}

/// The name, styled by its rename state.
fn name_el(t: &Theme, id: &str, n: &NameVm) -> Stateful<Div> {
    name_box(t, id, n).truncate()
}

/// The name without the one-line cut (the detail title wraps).
fn name_box(t: &Theme, id: &str, n: &NameVm) -> Stateful<Div> {
    let d = div().id(SharedString::from(id.to_string())).child(n.text.clone()).tooltip(kit::tip(n.title.clone()));
    match n.kind {
        NameKind::Renaming => d.text_color(t.faint),
        NameKind::Ok => d.text_color(t.up),
        NameKind::Bad => d.text_color(t.down),
        NameKind::Plain => d,
    }
}

/// `SESS_DOT`, and the 3px ring around a row's dot (`.srow .dot`).
fn dot_color(t: &Theme, status: &str) -> (Hsla, Hsla) {
    match status {
        "working" | "waiting" => (t.accent, t.accent_soft),
        "needs" => (t.warn, t.warn_soft),
        "offline" => (t.down, t.down_soft),
        _ => (t.faint, t.col),
    }
}

/// An 8px dot with a 3px ring that, like a box-shadow, takes no room.
fn ringed_dot(dot: Hsla, ring: Hsla) -> Div {
    div()
        .relative()
        .flex_none()
        .size(px(8.))
        .child(div().absolute().top(px(-3.)).left(px(-3.)).size(px(14.)).rounded_full().bg(ring))
        .child(div().absolute().top_0().left_0().size(px(8.)).rounded_full().bg(dot))
}

/// `.srow`: 0 14px 0 13px with a 3px left edge (accent when selected); `.srow-main` 9px tall padding.
fn row(t: &Theme, r: &RowVm, cx: &mut Context<MainWindow>) -> AnyElement {
    let id = r.id.clone();
    let hover = t.panel_2;
    let (rid, rid2, rid3) = (id.clone(), id.clone(), id.clone());
    let check = match r.pick {
        Pick::None => None,
        Pick::Spacer => Some(div().flex_none().w(px(16.)).into_any_element()),
        Pick::Box(on) => Some(
            div()
                .id(SharedString::from(format!("ss-pick-{id}")))
                .flex_none()
                .size(px(16.))
                .rounded(px(4.))
                .border_1()
                .border_color(if on { t.accent } else { t.border_2 })
                .bg(if on { t.accent } else { t.card })
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.on_accent)
                .text_size(px(11.))
                .cursor_pointer()
                .child(if on { "✓" } else { "" })
                .on_click(cx.listener(move |m, _, _, cx| {
                    cx.stop_propagation();
                    toggle_pick(m, &rid3, cx);
                }))
                .into_any_element(),
        ),
    };
    let (dot, ring) = dot_color(t, &r.status);
    let state_c = match r.status.as_str() {
        "working" | "waiting" => t.accent,
        "needs" => t.warn,
        "offline" => t.down,
        _ => t.muted,
    };
    div()
        .id(SharedString::from(format!("ss-row-{id}")))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .pl(px(13.))
        .pr(px(14.))
        .border_l(px(3.))
        .border_color(if r.on { t.accent } else { transparent_black() })
        .cursor_pointer()
        .when(r.on, |d| d.bg(accent_tint(t)))
        .when(!r.on, |d| d.hover(move |s| s.bg(hover)))
        .when(r.closing || r.closed, |d| d.opacity(if r.closed { 0.7 } else { 0.6 }))
        .children(check)
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(px(10.))
                .py(px(9.))
                .child(ringed_dot(dot, ring))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).child(name_el(t, &format!("ss-name-{id}"), &r.name)))
                        .child(div().truncate().text_size(px(12.5)).text_color(t.muted).child(r.sub.clone())),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_none()
                        .items_end()
                        .text_size(px(12.))
                        .child(div().font_weight(FontWeight::SEMIBOLD).text_color(state_c).child(r.state.clone()))
                        .child(div().text_color(t.muted).child(r.when.clone())),
                ),
        )
        .on_click(cx.listener(move |m, _, _, cx| row_click(m, &rid, cx)))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |m, e: &MouseDownEvent, _, cx| {
                if find_session(m, &rid2).is_some_and(|x| !menu_items(&x, m.sessions.selecting, &m.sessions.picked).is_empty()) {
                    m.toggle_menu(&format!("{MENU_PREFIX}{rid2}"), e.position, cx);
                }
            }),
        )
        .into_any_element()
}

/// The right-click menu of a row (`ssMenu`).
fn row_menu(m: &mut MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let (key, at) = m.menu.clone()?;
    let id = key.strip_prefix(MENU_PREFIX)?.to_string();
    let x = find_session(m, &id)?;
    let items = menu_items(&x, m.sessions.selecting, &m.sessions.picked);
    if items.is_empty() {
        return None;
    }
    let mut menu = kit::menu_box(t, 210.).id("ss-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for (k, label, danger) in items {
        let id = id.clone();
        let item = kit::menu_item(t, SharedString::from(format!("ss-menu-{k}")), label, false).when(danger, |d| d.text_color(t.down));
        menu = menu.child(item.on_click(cx.listener(move |m, _, window, cx| {
            m.menu = None;
            menu_action(m, k, &id, window, cx);
        })));
    }
    Some(kit::popover(at, menu))
}

/// One of the row menu's items (`ss-menu`).
pub fn menu_action(m: &mut MainWindow, k: &str, id: &str, window: &mut Window, cx: &mut Context<MainWindow>) {
    let select = |m: &mut MainWindow, cx: &mut Context<MainWindow>| {
        if m.sessions.selected.as_deref() != Some(id) {
            m.sessions.selected = Some(id.to_string());
            m.data.session = None;
            m.refresh(cx);
        }
    };
    match k {
        "open" => open(m, id, cx),
        "focus" => menu_focus(m, id, cx),
        "rename" => {
            select(m, cx);
            start_rename(m, id, window, cx);
        }
        "select" => {
            m.sessions.selecting = true;
            toggle_pick(m, id, cx);
        }
        "close" => {
            select(m, cx);
            m.sessions.confirm = Some(id.to_string());
        }
        "reopen" => menu_reopen(m, id, cx),
        _ => {}
    }
    cx.notify();
}

/// "Close N idle terminals?" over the page (`ssBulkConfirm`); a click outside keeps them open.
fn bulk_dialog(m: &mut MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    if !m.sessions.bulk {
        return None;
    }
    let vm = bulk_vm(&m.sessions.picked, &live_list(m), &closed_list(m), m.sessions.notes.get("ss-bulk"), m.sessions.busy.contains(&key("ss-bulk-go", "", "")));
    let mut names = div().flex().flex_col().gap(px(3.)).text_size(px(12.5)).text_color(t.text_2);
    for l in &vm.lines {
        names = names.child(format!("• {l}"));
    }
    let foot = div()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(8.))
        .children(vm.note.clone().map(|(n, err)| div().flex_1().text_size(px(12.5)).text_color(if err { t.down } else { t.up }).child(n)))
        .child(kit::btn(t, "ss-bulk-cancel", "Keep them open").on_click(cx.listener(|m, _, _, cx| {
            m.sessions.bulk = false;
            cx.notify();
        })))
        .child({
            let b = kit::btn(t, "ss-bulk-go", vm.go.clone()).bg(t.down).border_color(t.down).text_color(t.on_accent);
            if vm.busy { kit::disabled(b) } else { b.on_click(cx.listener(|m, _, _, cx| bulk_close(m, cx))) }
        });
    let dialog = kit::modal_box(t, 440.)
        .p(px(20.))
        .gap(px(12.))
        .child(div().text_size(px(17.)).font_weight(FontWeight::BOLD).child(vm.title.clone()))
        .child(div().text_size(px(13.)).text_color(t.text_2).child(BULK_TEXT))
        .child(names)
        .child(foot);
    Some(
        deferred(
            kit::scrim(t, "ss-bulk-bg")
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|m, _, _, cx| {
                        m.sessions.bulk = false;
                        cx.notify();
                    }),
                )
                .child(div().id("ss-bulk-box").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(dialog)),
        )
        .with_priority(1)
        .into_any_element(),
    )
}

/// The rename box in the detail title (↩ saves, esc drops it, leaving it saves).
fn rename_field(m: &mut MainWindow, t: &Theme, window: &mut Window, cx: &mut Context<MainWindow>) -> Div {
    let Some((_, input)) = m.sessions.rename.as_ref() else { return div() };
    let field = input.render(t, "ss-rename", window).flex_1().min_h(px(34.)).text_size(px(18.)).on_key_down(cx.listener(|m, ev: &KeyDownEvent, window, cx| {
        let Some((_, input)) = m.sessions.rename.as_ref() else { return };
        match input.on_key(ev, cx) {
            kit::KeyOutcome::Submit => end_rename(m, true, window, cx),
            kit::KeyOutcome::Cancel => {
                cx.stop_propagation();
                end_rename(m, false, window, cx)
            }
            kit::KeyOutcome::Ignored => {}
        }
    }));
    div().flex().items_center().w_full().max_w(px(560.)).child(field)
}

// ------------------------------------------------------------------ detail

/// `.sdetail`: the right card, 22px 26px, 16px between its parts.
fn detail(m: &mut MainWindow, t: &Theme, window: &mut Window, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let shell = page_card(t).id("ss-detail").flex_1().min_w_0().h_full().overflow_y_scroll();
    let d = detail_data(m);
    let closed = closed_list(m);
    let sel = m.sessions.selected.clone();
    let st = &m.sessions;
    let view = detail_vm(&DetailCtx {
        sel: sel.as_deref(),
        err: m.data.errs.session.as_deref(),
        detail: d.as_ref(),
        closed: &closed,
        renaming: st.rename.as_ref().is_some_and(|(r, _)| Some(r) == sel.as_ref()),
        confirm: st.confirm.as_deref(),
        note: sel.as_ref().and_then(|id| st.notes.get(&format!("ss:{id}"))),
        busy: &st.busy,
        holding: st.hold.as_ref().is_some_and(|(h, _)| Some(h) == sel.as_ref()),
        flash: sel.as_ref().and_then(|id| st.flash.get(id)),
    });
    let v = match view {
        DetailView::Message(msg) => return shell.items_center().justify_center().child(none_yet(t, msg)),
        DetailView::Error(msg) => return shell.items_center().justify_center().child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(t.down).child(msg)),
        DetailView::Full(v) => v,
    };
    let id = v.id.clone();

    // `.pill.st-{status}` and the muted "for 4 min".
    let status_row = div().flex().items_center().gap(px(8.)).child(sess_pill(t, &v.status, v.pill.clone()))
        .children(v.compacting.clone().map(|c| pill_el(t.accent_fg, t.accent_soft, c, false)))
        .children(v.status_line.clone().map(|l| div().text_color(t.muted).child(l)));

    // `.shead h2`: 22px bold, 1.3 line height, 4px above.
    let title: AnyElement = match &v.title {
        None => rename_field(m, t, window, cx).into_any_element(),
        Some(n) => {
            let el = name_box(t, "ss-title", n).mt(px(4.)).text_size(px(22.)).line_height(relative(1.3)).font_weight(FontWeight::BOLD);
            if v.gone {
                el.into_any_element()
            } else {
                let rid = id.clone();
                el.cursor_text()
                    .on_click(cx.listener(move |m, e: &ClickEvent, window, cx| {
                        if e.click_count() >= 2 {
                            start_rename(m, &rid, window, cx);
                        }
                    }))
                    .into_any_element()
            }
        }
    };

    // `.smeta`: 13px muted, 10px apart with a faint "·" between; paths and branches in 12.5px mono.
    let mut parts: Vec<AnyElement> = Vec::new();
    for p in [&v.path, &v.branch] {
        if !p.is_empty() {
            parts.push(div().font_family(t.mono_font.clone()).text_size(px(12.5)).child(p.clone()).into_any_element());
        }
    }
    if let Some((text, tip)) = v.diff.clone() {
        let mut words = text.splitn(3, ' ');
        let el = match (tip, words.next(), words.next(), words.next()) {
            (Some(tip), Some(n), Some(files_add), Some(rest)) => {
                // "3 files +40 −7": the counts in semibold green and red.
                let (files, add) = files_add.split_once(' ').map(|(a, b)| (format!("{n} {a}"), b.to_string())).unwrap_or((format!("{n} {files_add}"), String::new()));
                let (add, del) = if add.is_empty() { rest.split_once(' ').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((rest.into(), String::new())) } else { (add, rest.to_string()) };
                div()
                    .id("ss-diff")
                    .flex()
                    .gap(px(4.))
                    .child(files)
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(t.up).child(add))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(t.down).child(del))
                    .tooltip(kit::tip(tip))
                    .into_any_element()
            }
            _ => div().child(text).into_any_element(),
        };
        parts.push(el);
    }
    let mut meta = div().flex().flex_wrap().items_center().gap_x(px(10.)).gap_y(px(4.)).text_size(px(13.)).text_color(t.muted);
    for (n, p) in parts.into_iter().enumerate() {
        // The "·" goes with the part after it (`.smeta > * + *::before`), so a wrap takes it along.
        meta = meta.child(div().flex().items_center().when(n > 0, |d| d.child(div().mr(px(10.)).text_color(t.faint).child("·"))).child(p));
    }

    // `.sactions`: 40px buttons, 8px apart.
    let mut actions = div().flex().flex_none().items_start().gap(px(8.));
    for a in &v.actions {
        let rid = id.clone();
        let el = match a.act {
            "ss-hold" => hold_button(t, &rid, &a.label, a.busy, a.label == "Keep holding…", cx),
            "ss-ask" => danger_btn(t, "ss-ask", a.label.clone()).h(px(40.)).on_click(cx.listener(move |m, _, _, cx| {
                m.sessions.confirm = Some(rid.clone());
                cx.notify();
            })),
            act => {
                let b = soft_btn(t, act, a.label.clone()).h(px(40.));
                if a.busy {
                    kit::disabled(b)
                } else if act == "ss-reopen" {
                    b.on_click(cx.listener(move |m, _, _, cx| reopen(m, &rid, cx)))
                } else {
                    b.on_click(cx.listener(move |m, _, _, cx| focus(m, &rid, cx)))
                }
            }
        };
        actions = actions.child(el);
    }

    let head = div()
        .flex()
        .items_start()
        .gap(px(16.))
        .child(div().flex().flex_col().flex_1().min_w_0().gap(px(4.)).child(status_row).child(title).child(meta))
        .when(!v.actions.is_empty(), |d| d.child(actions));

    let mut col = shell.gap(px(16.)).py(px(22.)).px(px(26.)).child(head);
    if let Some(n) = v.close_note {
        col = col.child(div().mt(px(-8.)).text_color(t.muted).child(n));
    }
    if let Some((n, err)) = v.note.clone() {
        col = col.child(div().text_size(px(12.5)).when(err, |d| d.font_weight(FontWeight::MEDIUM)).text_color(if err { t.down } else { t.up_fg }).child(n));
    }
    if let Some(c) = &v.confirm {
        // `.sconfirm`: 14px 16px on down-soft, radius 12.
        let rid = id.clone();
        let force = c.force;
        let go = solid_danger_btn(t, "ss-close", c.label.clone());
        col = col.child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .py(px(14.))
                .px(px(16.))
                .rounded(px(12.))
                .bg(t.down_soft)
                .child(div().font_weight(FontWeight::BOLD).text_color(t.down).child(c.title.clone()))
                .child(div().text_size(px(13.5)).child(c.text.clone()))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .child(if c.busy { kit::disabled(go) } else { go.on_click(cx.listener(move |m, _, _, cx| close(m, &rid, force, cx))) })
                        .child(ghost_btn(t, "ss-cancel", "Keep it open").on_click(cx.listener(|m, _, _, cx| {
                            m.sessions.confirm = None;
                            cx.notify();
                        }))),
                ),
        );
    }
    if let Some(c) = &v.closing {
        // `.sclosing`.
        col = col.child(div().py(px(14.)).px(px(16.)).rounded(px(12.)).bg(t.panel_2).text_color(t.text_2).text_size(px(13.5)).child(c.clone()));
    }
    if !v.links.is_empty() {
        col = col.child(links(t, &v.links, cx));
    }
    col = col.child(stats(t, &v.stats));
    if let Some(bx) = &v.boxes {
        col = col.child(boxes(t, bx, &id));
    }
    col.child(timeline(t, &v.timeline))
}

/// "Force close" (`.btn.danger.solid.hold`) that only fires after being held down for `HOLD`
/// (letting go or leaving it stops).
fn hold_button(t: &Theme, id: &str, label: &str, busy: bool, holding: bool, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let rid = id.to_string();
    let b = solid_danger_btn(t, "ss-hold", label.to_string()).h(px(40.)).tooltip(kit::tip("Press and hold to stop what it’s doing and close it"));
    if busy {
        return kit::disabled(b);
    }
    b.when(holding, |d| d.bg(t.down.opacity(0.85)))
        .on_mouse_down(MouseButton::Left, cx.listener(move |m, _, _, cx| hold_start(m, &rid, cx)))
        .on_mouse_up(MouseButton::Left, cx.listener(|m, _, _, cx| hold_end(m, cx)))
        .on_mouse_up_out(MouseButton::Left, cx.listener(|m, _, _, cx| hold_end(m, cx)))
        .on_hover(cx.listener(|m, over: &bool, _, cx| {
            if !*over {
                hold_end(m, cx);
            }
        }))
}

/// `.slinks`: its task (and goal), or "No task" and the last task it worked on.
fn links(t: &Theme, rows: &[LinkVm], cx: &mut Context<MainWindow>) -> Div {
    let mut card = div().flex().flex_col().flex_none().rounded(px(10.)).border_1().border_color(t.border).overflow_hidden();
    for (n, l) in rows.iter().enumerate() {
        let hover = t.panel_2;
        let mut row = div()
            .id(SharedString::from(format!("ss-link-{n}")))
            .flex()
            .items_center()
            .gap(px(12.))
            .min_h(px(44.))
            .py(px(10.))
            .px(px(14.))
            .when(n > 0, |d| d.border_t_1().border_color(t.divider))
            .child(div().w(px(72.)).flex_none().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(t.muted).child(l.k.to_uppercase()))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .overflow_hidden()
                    .when(!l.r.is_empty(), |d| d.child(div().flex_none().mr(px(4.)).font_family(t.mono_font.clone()).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(l.r.clone())))
                    .child(div().pl(px(4.)).truncate().when(l.go.is_none(), |d| d.text_color(t.muted)).child(l.title.clone()))
                    .children(l.pill.map(|(label, _)| div().flex_none().ml(px(6.)).child(task_st_pill(t, label)))),
            );
        if let Some((go, target)) = l.go.clone() {
            row = row.cursor_pointer().hover(move |s| s.bg(hover)).child(div().flex_none().text_size(px(13.)).text_color(t.accent).child(go)).on_click(cx.listener(move |m, _, _, cx| match &target {
                LinkTarget::Task(r) => m.open_task(r.clone(), cx),
                LinkTarget::Goal(r) => m.go(Page::Goal(r.clone()), cx),
            }));
        }
        card = card.child(row);
    }
    card
}

/// `.sstats`: four tiles, 10px apart.
fn stats(t: &Theme, items: &[(String, &'static str)]) -> Div {
    let mut row = div().flex().flex_none().gap(px(10.));
    for (v, k) in items {
        row = row.child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .flex_basis(px(0.))
                .min_w_0()
                .gap(px(2.))
                .py(px(12.))
                .px(px(14.))
                .rounded(px(10.))
                .bg(t.panel_2)
                .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(v.clone()))
                .child(div().text_size(px(12.5)).text_color(t.muted).child(*k)),
        );
    }
    row
}

/// `.snow`: the last prompt next to the latest reply (or what it's waiting on).
fn boxes(t: &Theme, bx: &BoxesVm, id: &str) -> Div {
    let sbox = || div().flex().flex_col().flex_1().flex_basis(px(0.)).min_w_0().gap(px(4.)).py(px(14.)).px(px(16.)).rounded(px(12.)).bg(t.panel_2);
    let when = |text: String| div().text_size(px(12.)).text_color(t.muted).child(text);
    let left = sbox().child(when(bx.prompt_when.clone())).child(div().child(bx.prompt_text.clone()));
    let mut right = sbox().child(when(bx.reply_when.clone()));
    if bx.waiting {
        right = right.bg(t.warn_soft);
    }
    right = match &bx.reply_text {
        Some(x) => right.child(div().id(SharedString::from(format!("ss-reply-box-{id}"))).max_h(px(320.)).overflow_y_scroll().child(md::render(t, x, 1200, &format!("ss-reply-{id}")))),
        None => right.child("Nothing yet."),
    };
    div().flex().flex_none().gap(px(12.)).child(left).child(right)
}

/// `.stl`: time (48px, 12px mono muted), the dot, the text (13.5px), the kind (12px muted).
fn timeline(t: &Theme, items: &[TimelineVm]) -> Div {
    let col = div().flex().flex_col().flex_none().gap(px(8.)).child(h3(t, "Turns and events"));
    if items.is_empty() {
        return col.child(none_yet(t, "Nothing recorded yet. Its turns show up here once it runs with the task board plugin."));
    }
    let mut list = div().flex().flex_col();
    for (n, e) in items.iter().enumerate() {
        let (dot, label) = timeline_kind(t, &e.kind);
        list = list.child(
            div()
                .id(("ss-tl", n))
                .flex()
                .items_start()
                .gap(px(10.))
                .py(px(9.))
                .border_t_1()
                .border_color(t.divider)
                .child(div().flex_none().w(px(48.)).pt(px(1.)).font_family(t.mono_font.clone()).text_size(px(12.)).text_color(t.muted).child(e.time.clone()))
                .child(div().flex_none().w(px(10.)).self_center().child(kit::dot(dot, 8.)))
                .child(div().flex_1().min_w_0().text_size(px(13.5)).child(e.text.clone()))
                .child(div().flex_none().pt(px(1.)).text_size(px(12.)).text_color(t.muted).whitespace_nowrap().child(label))
                .tooltip(kit::tip(e.full.clone())),
        );
    }
    col.child(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parity::{self, golden};
    // `super::*` brings in GPUI's own `test`; std's, by name, wins over the glob.
    use ::core::prelude::v1::test;

    fn strs(v: &Value) -> Vec<String> {
        v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
    }

    fn arr_of(v: &Value) -> Vec<Value> {
        v.as_array().cloned().unwrap_or_default()
    }

    /// The task ref in a web `back=` route (`#/?task=T12`), as the web header reads it.
    fn back_task(back: &Value) -> Option<String> {
        let b = back.as_str().filter(|b| b.starts_with("#/"))?;
        let q = b.split_once('?')?.1;
        q.split('&').find_map(|kv| kv.strip_prefix("task=")).map(str::to_string)
    }

    /// One case's list context, the way the web page state was set up for it.
    fn list_case(i: &Value, f: impl FnOnce(&ListCtx) -> Value) -> Value {
        let live = i.get("list").filter(|l| !l.is_null()).map(arr_of);
        let closed = i.get("closed").map(arr_of).unwrap_or_default();
        let closed_total = if i.get("closed").is_some() { i["closedTotal"].as_i64().unwrap_or(closed.len() as i64) } else { 0 };
        let (collapsed, picked) = (strs(&i["collapsed"]), strs(&i["picked"]));
        let flash = HashMap::new();
        f(&ListCtx {
            live: live.as_deref(),
            list_err: i["listErr"].as_str().filter(|e| !e.is_empty()),
            closed: &closed,
            closed_total,
            sel: i["sel"].as_str(),
            filter: i["f"].as_str().unwrap_or(""),
            q: i["q"].as_str().unwrap_or(""),
            collapsed: &collapsed,
            hide_sel: i["hideSel"].as_str(),
            selecting: i["selecting"].as_bool().unwrap_or(false),
            picked: &picked,
            closed_open: i["closedOpen"].as_bool().unwrap_or(false),
            flash: &flash,
        })
    }

    fn detail_case(i: &Value) -> Value {
        let closed = i.get("closed").map(arr_of).unwrap_or_default();
        let sel = i["sel"].as_str();
        let detail = i.get("detail").filter(|d| d.is_object());
        let id = detail.map(|d| s(d, "id")).unwrap_or("");
        let note = i["notes"].get(format!("ss:{id}")).map(|n| Note { text: s(n, "text").into(), err: b(n, "err"), at: Instant::now() });
        let busy: HashSet<String> = strs(&i["busy"]).into_iter().collect();
        let v = detail_vm(&DetailCtx {
            sel,
            err: i["detailErr"].as_str().filter(|e| !e.is_empty()),
            detail,
            closed: &closed,
            renaming: i["rename"].as_str().is_some_and(|r| r == id),
            confirm: i["confirm"].as_str(),
            note: note.as_ref(),
            busy: &busy,
            holding: i["hold"].as_str().is_some_and(|h| h == id),
            flash: None,
        });
        json!({"text": v.text(), "acts": v.acts()})
    }

    #[::core::prelude::v1::test]
    fn desk_row_shows_its_role() {
        let x = json!({"id": "s9", "name": "TB Jira desk", "status": "idle", "role": "Handles Jira for the board", "close": null});
        list_case(&json!({"f": "all"}), |c| {
            assert_eq!(row_vm(&x, c).sub, "Handles Jira for the board");
            json!(null)
        });
        assert!(!bulkable(&x), "the desk is never closed in a batch");
    }

    #[::core::prelude::v1::test]
    fn a_terminal_waiting_on_background_work_says_so() {
        let x = json!({"id": "s9", "name": "T35 Release", "status": "waiting", "background": {"agents": 2, "commands": 0}, "close": "force"});
        list_case(&json!({"f": "all"}), |c| {
            assert_eq!(row_vm(&x, c).state, "2 agents running");
            json!(null)
        });
        assert!(for_filter(&x, "working"));
        assert!(!for_filter(&x, "idle"));
        assert!(!for_filter(&x, "stale"));
        assert_eq!(rank(&x), rank(&json!({"status": "working"})));
    }

    #[::core::prelude::v1::test]
    fn sessions_match_web() {
        let g = golden("sessions");
        g.check(|i| match i["fn"].as_str().unwrap() {
            "longAgo" => json!(long_ago(i["ms"].as_f64().unwrap() as i64)),
            "forFilter" => json!(for_filter(&i["session"], i["f"].as_str().unwrap())),
            "ssMatch" => json!(matches(&i["session"], i["q"].as_str().unwrap())),
            "grouped" => {
                let list = arr_of(&i["list"]);
                json!(grouped(list.iter().collect()).iter().map(|(p, v)| json!([p, v.iter().map(|x| s(x, "id")).collect::<Vec<_>>()])).collect::<Vec<_>>())
            }
            "statusLine" => json!(status_line(&i["detail"])),
            "diffView" => match diff_vm(&i["detail"]) {
                Some((text, title)) => json!({"text": text, "title": title}),
                None => json!({"text": "", "title": null}),
            },
            "closeCopy" => {
                let (title, text, label) = close_copy(&i["detail"]);
                json!({"title": title, "text": text, "label": label})
            }
            "nameView" => {
                let mut watch: HashMap<String, String> = i["watch"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
                let mut flash = HashMap::new();
                observe_rename(&mut watch, &mut flash, &i["session"]);
                let n = name_vm(&i["session"], i["hint"].as_str().unwrap(), flash.get(s(&i["session"], "id")));
                json!({"text": n.text, "cls": n.kind.class(), "title": n.title})
            }
            "ssRow" => list_case(i, |c| {
                let r = row_vm(&i["session"], c);
                json!({"text": r.text(), "acts": ["ss-open"]})
            }),
            "ssClosedRow" => list_case(i, |c| {
                let r = closed_row_vm(&i["session"], c);
                json!({"text": r.text(), "acts": ["ss-open"]})
            }),
            "ssHeader" => {
                let live = i.get("list").filter(|l| !l.is_null()).map(arr_of);
                let h = header_vm(live.as_deref(), i["f"].as_str().unwrap_or(""), back_task(&i["back"]).as_deref());
                // ↔ The web header always had a back link; with no task to go back to it read
                // "Task board". The app's sidebar is that link, so the header shows none then.
                json!({"back": h.back.clone().unwrap_or_else(|| "Task board".into()), "text": h.text(), "pressed": h.pressed})
            }
            "ssMenuItems" => {
                let items = menu_items(&i["session"], i["selecting"].as_bool().unwrap_or(false), &strs(&i["picked"]));
                json!(items.iter().map(|(k, l, d)| if *d { json!([k, l, "danger"]) } else { json!([k, l]) }).collect::<Vec<_>>())
            }
            "ssList" => list_case(i, |c| {
                let v = list_vm(c);
                json!({"text": v.text(), "acts": v.acts()})
            }),
            "ssDetail" => detail_case(i),
            "ssBulkConfirm" => {
                if !i["bulk"].as_bool().unwrap_or(false) {
                    return json!({"text": "", "acts": []});
                }
                let v = bulk_vm(&strs(&i["picked"]), &arr_of(&i["list"]), &[], None, false);
                json!({"text": v.text(), "acts": ["ss-bulk-bg", "ss-bulk-cancel", "ss-bulk-go"]})
            }
            "sentNote" => json!(sent_note(&json!({"midna": {"up": !i["midnaDown"].as_bool().unwrap_or(false)}}))),
            f => panic!("no app function for {f}"),
        });
    }

    #[::core::prelude::v1::test]
    fn a_failed_rename_flashes_then_clears() {
        let mut watch = HashMap::from([("s1".to_string(), "new".to_string())]);
        let mut flash = HashMap::new();
        // Still renaming: nothing yet.
        assert!(!observe_rename(&mut watch, &mut flash, &json!({"id": "s1", "name": "old", "renaming": "new"})));
        assert!(observe_rename(&mut watch, &mut flash, &json!({"id": "s1", "name": "old"})));
        assert!(watch.is_empty(), "a rename is watched once");
        let f = &flash["s1"];
        assert!(!f.ok);
        assert_eq!(f.why, "Midna kept the old name");
        assert_eq!(name_vm(&json!({"id": "s1", "name": "old"}), "", Some(f)).text, "new");
    }

    // ------------------------------------------------------------------ actions

    use gpui_kit::TestAppContext;

    /// The window on the Sessions page with its lists loaded.
    fn page(cx: &mut TestAppContext) -> (WindowHandle<MainWindow>, std::sync::Arc<parity::Recording>) {
        let (w, rec) = parity::window(cx);
        w.update(cx, |m, _, cx| m.go(Page::Sessions, cx)).unwrap();
        parity::settle(cx);
        w.update(cx, |m, _, _| assert!(m.data.sessions.is_some(), "sessions loaded")).unwrap();
        (w, rec)
    }

    #[gpui_kit::test]
    fn show_in_midna_says_so_under_the_buttons(cx: &mut TestAppContext) {
        let (w, rec) = page(cx);
        w.update(cx, |m, _, cx| focus(m, "fake-s3", cx)).unwrap();
        w.update(cx, |m, _, _| assert!(m.sessions.busy.contains("ss-focus::fake-s3"), "busy while sending")).unwrap();
        parity::settle(cx);
        assert_eq!(rec.last("sessions/fake-s3/focus"), Some(json!({})));
        w.update(cx, |m, _, _| {
            assert!(m.sessions.busy.is_empty());
            let n = m.sessions.notes.get("ss:fake-s3").expect("a note");
            if !n.err {
                assert_eq!(n.text, sent_note(m.state()));
            }
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn the_menu_shows_in_midna_with_a_toast(cx: &mut TestAppContext) {
        let (w, rec) = page(cx);
        w.update(cx, |m, window, cx| menu_action(m, "focus", "fake-s3", window, cx)).unwrap();
        parity::settle(cx);
        assert_eq!(rec.last("sessions/fake-s3/focus"), Some(json!({})));
        w.update(cx, |m, _, _| assert!(m.sessions.notes.is_empty(), "no inline note from the menu")).unwrap();
    }

    #[gpui_kit::test]
    fn close_and_force_close_send_force(cx: &mut TestAppContext) {
        let (w, rec) = page(cx);
        w.update(cx, |m, window, cx| menu_action(m, "close", "fake-s3", window, cx)).unwrap();
        w.update(cx, |m, _, _| {
            assert_eq!(m.sessions.selected.as_deref(), Some("fake-s3"), "the menu's Close opens it");
            assert_eq!(m.sessions.confirm.as_deref(), Some("fake-s3"), "and asks first");
        })
        .unwrap();
        assert!(rec.posts().is_empty(), "nothing sent before confirming");
        w.update(cx, |m, _, cx| close(m, "fake-s3", false, cx)).unwrap();
        parity::settle(cx);
        assert_eq!(rec.last("sessions/fake-s3/close"), Some(json!({"force": false})));
        w.update(cx, |m, _, cx| hold_fire(m, "fake-s1", cx)).unwrap();
        parity::settle(cx);
        assert_eq!(rec.last("sessions/fake-s1/close"), Some(json!({"force": true})));
    }

    #[gpui_kit::test]
    fn reopen_from_the_detail_and_the_menu(cx: &mut TestAppContext) {
        let (w, rec) = page(cx);
        w.update(cx, |m, _, cx| reopen(m, "closed-x", cx)).unwrap();
        parity::settle(cx);
        assert_eq!(rec.last("sessions/closed-x/reopen"), Some(json!({})));
        rec.clear();
        w.update(cx, |m, window, cx| menu_action(m, "reopen", "closed-y", window, cx)).unwrap();
        parity::settle(cx);
        assert_eq!(rec.posts(), vec![("sessions/closed-y/reopen".to_string(), json!({}))]);
    }

    #[gpui_kit::test]
    fn rename_sends_only_a_new_name(cx: &mut TestAppContext) {
        let (w, rec) = page(cx);
        let rename = |cx: &mut TestAppContext, text: &str, save: bool| {
            w.update(cx, |m, window, cx| {
                start_rename(m, "fake-s3", window, cx);
                let (_, input) = m.sessions.rename.as_ref().expect("renaming");
                assert_eq!(input.text(cx), "api shell", "starts from the current name");
                input.set_text(text, cx);
                end_rename(m, save, window, cx);
                assert!(m.sessions.rename.is_none());
            })
            .unwrap();
            parity::settle(cx);
        };
        rename(cx, "api shell", true);
        rename(cx, "   ", true);
        rename(cx, "whatever", false);
        assert!(rec.posts().is_empty(), "unchanged, empty or cancelled: nothing sent");
        rename(cx, "  API shell (2)  ", true);
        assert_eq!(rec.last("sessions/fake-s3/rename"), Some(json!({"name": "API shell (2)"})));
        w.update(cx, |m, _, _| assert_eq!(m.sessions.watch.get("fake-s3").map(String::as_str), Some("API shell (2)"), "watched for the ✓/✗")).unwrap();
    }

    #[gpui_kit::test]
    fn bulk_close_sends_picks_in_order_and_stays_picking(cx: &mut TestAppContext) {
        let (w, rec) = page(cx);
        w.update(cx, |m, window, cx| {
            menu_action(m, "select", "fake-s3", window, cx);
            assert!(m.sessions.selecting);
            assert_eq!(m.sessions.picked, vec!["fake-s3".to_string()]);
            // A click on a pickable row while picking toggles it rather than opening it.
            let before = m.sessions.selected.clone();
            row_click(m, "fake-s3", cx);
            assert!(m.sessions.picked.is_empty());
            assert_eq!(m.sessions.selected, before);
            row_click(m, "fake-s3", cx);
            m.sessions.bulk = true;
            bulk_close(m, cx);
        })
        .unwrap();
        parity::settle(cx);
        assert_eq!(rec.last("sessions/close"), Some(json!({"ids": ["fake-s3"]})));
        w.update(cx, |m, _, _| {
            if m.sessions.notes.get("ss-bulk").is_none_or(|n| !n.err) {
                assert!(!m.sessions.bulk && m.sessions.picked.is_empty());
                assert!(m.sessions.selecting, "the web board stays in picking mode");
            }
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn collapsed_groups_are_saved(cx: &mut TestAppContext) {
        let (w, _rec) = page(cx);
        w.update(cx, |m, _, cx| {
            m.sessions.selected = Some("fake-s3".into());
            toggle_group(m, "api", true, cx);
            assert_eq!(crate::prefs::get(COLLAPSED_KEY), Some(json!(["api"])));
            assert_eq!(m.sessions.hide_sel.as_deref(), Some("fake-s3"), "collapsing keeps the selection from reopening it");
            toggle_group(m, "webapp", true, cx);
            toggle_group(m, "api", false, cx);
            assert_eq!(crate::prefs::get(COLLAPSED_KEY), Some(json!(["webapp"])));
        })
        .unwrap();
        // A fresh page reads them back.
        let mut st = State::default();
        load_collapsed(&mut st);
        assert_eq!(st.collapsed, vec!["webapp".to_string()]);
    }

    #[gpui_kit::test]
    fn auto_select_skips_collapsed_groups(cx: &mut TestAppContext) {
        let (w, _rec) = page(cx);
        w.update(cx, |m, _, cx| {
            m.sessions.selected = None;
            m.sessions.collapsed_loaded = true;
            m.sessions.collapsed = vec!["webapp".into()];
            auto_select(m, cx);
            assert_eq!(m.sessions.selected.as_deref(), Some("fake-s3"), "first terminal of the first open group");
            assert_eq!(m.sessions.hide_sel, None);
            m.sessions.selected = None;
            m.sessions.collapsed = vec!["webapp".into(), "api".into()];
            auto_select(m, cx);
            assert_eq!(m.sessions.selected.as_deref(), Some("fake-s2"), "all collapsed: the first group's first terminal");
            assert_eq!(m.sessions.hide_sel.as_deref(), Some("fake-s2"), "and its group stays collapsed");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn escape_closes_confirmations_then_picking(cx: &mut TestAppContext) {
        let (w, _rec) = page(cx);
        w.update(cx, |m, window, cx| {
            m.sessions.selecting = true;
            m.sessions.picked = vec!["fake-s3".into()];
            m.sessions.bulk = true;
            m.sessions.confirm = Some("fake-s1".into());
            assert!(escape(m, window, cx));
            assert!(!m.sessions.bulk && m.sessions.confirm.is_none() && m.sessions.selecting, "first the dialogs");
            assert!(escape(m, window, cx));
            assert!(!m.sessions.selecting && m.sessions.picked.is_empty(), "then picking");
            assert!(!escape(m, window, cx), "then nothing for this page");
            m.go(Page::Board, cx);
            m.sessions.confirm = Some("fake-s1".into());
            assert!(!escape(m, window, cx), "only on the Sessions page");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn back_to_the_task(cx: &mut TestAppContext) {
        let (w, _rec) = page(cx);
        w.update(cx, |m, _, cx| {
            m.go(Page::Board, cx);
            open_from_task(m, "fake-s2", "T3", cx);
            assert_eq!(m.page, Page::Sessions);
            assert_eq!(m.sessions.selected.as_deref(), Some("fake-s2"));
            assert_eq!(header_vm(None, "", m.sessions.back.as_deref()).back.as_deref(), Some("Back to T3"));
            m.go(Page::Board, cx);
            assert_eq!(m.sessions.back, None, "leaving the page drops it");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn filter_and_open_drop_a_close_confirmation(cx: &mut TestAppContext) {
        let (w, _rec) = page(cx);
        w.update(cx, |m, _, cx| {
            m.sessions.confirm = Some("fake-s3".into());
            set_filter(m, "needs", cx);
            assert!(m.sessions.confirm.is_none());
            assert_eq!(m.sessions.filter, "needs");
            m.sessions.confirm = Some("fake-s3".into());
            m.sessions.selected = Some("fake-s3".into());
            open(m, "fake-s3", cx);
            assert!(m.sessions.confirm.is_none(), "even reopening the same one");
        })
        .unwrap();
    }
}
