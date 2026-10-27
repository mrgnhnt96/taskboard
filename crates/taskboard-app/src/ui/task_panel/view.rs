//! The task panel as data: what the web board's `taskPanel()` (app.js) showed for a task, built as a
//! tree of nodes that carry the visible text, the controls (named by the web's `data-act`), tooltips
//! and field contents. `task_panel.rs` draws the tree and dispatches its acts; the parity tests
//! compare its text / acts / titles / values with the web board's own output
//! (`parity/golden/task.json`), so the two can't drift apart.
//!
//! Every function here is a port of the web function named in its doc comment.
use crate::fmt::{self, arr, b, obj, opt_s, s};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

// ------------------------------------------------------------------ nodes

/// The pill colours of the web's `.pill.st-*` / `.pill.*` classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Queued,
    Review,
    Needs,
    Done,
    Failed,
    Planned,
    Working,
    Blocked,
    Neutral,
    Idle,
    Gone,
    Closed,
    Jira,
    Design,
}

/// Button looks (`.btn` classes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Look {
    #[default]
    Plain,
    Primary,
    PrimaryWide,
    SoftWide,
    Soft,
    SoftSmall,
    Danger,
    /// `.btn.link`: text-only.
    Link,
    /// An icon button (`.icon-btn`): close, back, focus, the attachment menu, remove a field.
    Icon(Icon),
    /// A tab / log chip / attachment-kind segment.
    Tab { on: bool },
    /// A menu item (`role=menuitem`).
    Menu,
    /// A task ref shown as a link (blocked-by refs).
    Ref,
    /// A header chip (`.hchip`); `on` while its dropdown is open.
    Chip { on: bool },
    /// A footer view (`.foot-btn`); `on` while it's the one open.
    Foot { on: bool },
    /// One of a question's options in the needs-you banner (`.opt`); `on` while it's a picked
    /// choice waiting for Send.
    Opt { on: bool },
    /// "Write an answer": a link with an open-elsewhere icon, since it answers in a terminal.
    Out,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Close,
    Back,
    More,
    Remove,
    /// Focus the terminal in Midna (a box with an arrow out of it).
    Focus,
}

/// How a plain link looks (`.goal-name`, `.jkey`, `.term-link`, `.att-name`, `.att-src.g`…).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LinkLook {
    #[default]
    Plain,
    Jkey,
    AttName,
    Small,
    /// The breadcrumb's goal (`.crumb a`).
    Crumb,
    /// A header chip (`.hchip`); a Jira chip's text is its key, a space, then its status.
    Chip,
    JiraChip,
}

/// Where a plain link (`<a href>`) goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Go {
    Goal(String),
    Session(String),
    /// The board page with this issue's panel open.
    Issue(String),
    /// A web URL, or a file path opened with `open` (an attachment).
    Url(String),
    /// The task's step results that hold findings, opened: the PR bar's review step links here.
    Findings,
}

/// A control: the web's `data-act` / `data-arg` / `data-id` / `data-grp`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Act {
    pub act: &'static str,
    pub arg: String,
    pub id: String,
    pub grp: String,
}

impl Act {
    pub fn new(act: &'static str, arg: impl Into<String>, id: impl Into<String>, grp: impl Into<String>) -> Act {
        Act { act, arg: arg.into(), id: id.into(), grp: grp.into() }
    }

    /// The web's `busyKey`: act, arg and id.
    pub fn busy_key(&self) -> String {
        format!("{}:{}:{}", self.act, self.arg, self.id)
    }
}

/// Text styles (the web's classes on spans and paragraphs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum St {
    #[default]
    Plain,
    Title,
    Small,
    Muted,
    Help,
    WarnHelp,
    BoxLabel,
    H3,
    Strong,
    Mono,
    Empty,
    Time,
    Kind,
    Handoff,
    Code,
    Pre,
    StepName,
    StepSub,
    LrowKey,
    Count,
    /// `.jkey` without a link.
    Jkey,
    /// A Jira header chip without a link: key, a space, then status.
    JiraChip,
    /// A coloured list glyph (✓ → • !).
    Glyph(Dot),
    /// A timeline stop's name (`.tl-h`) and its subline (`.tl-s`).
    TlH,
    TlS,
    /// What a timeline stop says (`.tl-text`): its summary, or the latest line.
    TlText,
    /// What an opened stop says (`.det p`): a step's result.
    DetText,
    /// The now stop's name while it waits on you (`.tl-h.warn`).
    TlHWarn,
    /// What the needs-you banner says (`.nb-q`): the question, or what went wrong.
    BannerText,
    /// How long an ask has waited on its reviewer, in work time; orange once it's about to swap.
    Wait { late: bool },
}

/// Containers (the web's layout elements).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum K {
    #[default]
    Col,
    Top,
    /// The breadcrumb: goal › ref.
    Crumb,
    /// The header chips under the title.
    Head,
    /// The Attached chip's dropdown.
    Drop,
    /// A dropdown row: kind and icon (the file's type, else the kind), then the title (a link
    /// when it opens), "Kind · ref · age", and the hover card's rows (a `Kv`).
    DropItem(&'static str, &'static str),
    Tabs,
    Body,
    /// `.box` in a tone: ask (warn), lost / failed (down), done (up), info.
    Box(BoxTone),
    /// The full-width needs-you banner under the chips (`.nb`): ask (warn) or lost / failed (down).
    Banner(BoxTone),
    Row,
    Stack,
    /// A `<details>` fold: first kid is the summary, the rest show when open.
    Fold(Fold, bool),
    /// `.lrow`: first kid is the key text.
    Lrow,
    Line,
    PrHead,
    Steps,
    Step(StepSt),
    PrBar { warn: bool },
    Kv,
    KvRow,
    List,
    Li,
    Files,
    MetaRow,
    Chips,
    LogItem,
    Md,
    MdP,
    MdUl,
    MdLi,
    /// The task's timeline (`.tline`): its stops in order.
    Tline,
    /// One stop (`.tl-node`): its dot, then a head and what's under it; `card` for the now stop.
    TlNode { st: TlSt, card: bool },
    /// A stop's name, subline and time.
    TlHead,
    /// A finished stop that opens in place: its head, then what it shows once open.
    TlStep { step: TlStep, open: bool },
    /// What an opened stop shows (`.det`).
    Det,
    /// One of the task's terminals in Started's detail (`.dr`): its name, then its state; `live` for the one it's in.
    DetTerm { live: bool },
    /// A row of an opened stop (`.kv`): its label, then its value.
    DetRow,
    /// Focus in Midna and the ⋯ menu.
    TlActs,
    /// The terminal's ⋯ menu.
    TermMenu,
    /// A menu row: its button, then what it does.
    MenuItem { danger: bool },
    /// The views at the panel's foot: What to do, Context, Full log.
    Foot,
    /// A reviewer's name in the PR's Reviewers rows, then their popover while it's `open`.
    RevName { open: bool },
    /// The reviewer popover: its head, the open asks (or that there are none), the week's answered ones.
    RevPop,
    /// The popover's head: the name and host login, then the counts line.
    RevHead,
    /// An open ask: its PR line, then its task line; `here` for the task in the panel.
    RevAsk { here: bool },
    /// A line of an ask (PR, title and wait; or task, goal, when and notes).
    RevLine,
}

/// A finished timeline stop that opens in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TlStep {
    Started,
    /// The nth of the task's `step_results`.
    Result(usize),
}

/// A timeline stop's dot (`.n-*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlSt {
    Done,
    Now,
    Warn,
    Fail,
    Todo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxTone {
    Ask,
    Lost,
    Done,
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fold {
    /// A step's findings (`step_results`), under its headline.
    Findings,
    /// A stuck-looking terminal's last lines, under its check-in.
    Tail,
    /// The reviews a reviewer gave this week, under their open asks.
    Reviewed,
}

impl Fold {
    /// The localStorage key the web kept this fold's state under.
    pub fn key(self) -> &'static str {
        match self {
            Fold::Findings => "tb.fold.findings",
            Fold::Tail => "tb.fold.tail",
            Fold::Reviewed => "tb.fold.reviewed",
        }
    }
}

/// A PR step's state (`checksStep` / `reviewStep` / `mergeStep` `st`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepSt {
    Done,
    Fail,
    Ask,
    Wait,
    Now,
    Todo,
}

/// Log dots by kind (`LOG_DOT`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dot {
    Accent,
    Faint,
    Warn,
    AccentFg,
    Up,
    Goal,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Text { s: String, st: St, tip: Option<String> },
    Pill { s: String, tone: Tone, tip: Option<String> },
    Btn { label: String, act: Act, look: Look, disabled: bool, tip: Option<String> },
    Link { s: String, go: Go, tip: Option<String>, look: LinkLook },
    /// An input (one line) or textarea (`multi`); `key` is the web's `data-k` / field name.
    Field { key: String, value: String, placeholder: &'static str, multi: bool, mono: bool },
    Note { s: String, err: bool },
    Dot(Dot),
    El { k: K, tip: Option<String>, kids: Vec<Node> },
    /// The Full log's entries (`<ol class="log">`), drawn only as they scroll into view: each
    /// becomes a [`log_item`] when drawn. `more` means older ones are still to be fetched.
    Log { rows: Rc<Vec<Value>>, more: bool },
}

fn el(k: K, kids: Vec<Node>) -> Node {
    Node::El { k, tip: None, kids }
}

fn txt(s: impl Into<String>, st: St) -> Node {
    Node::Text { s: s.into(), st, tip: None }
}

fn txt_tip(s: impl Into<String>, st: St, tip: impl Into<String>) -> Node {
    Node::Text { s: s.into(), st, tip: Some(tip.into()) }
}

fn pill(s: impl Into<String>, tone: Tone) -> Node {
    Node::Pill { s: s.into(), tone, tip: None }
}

// ------------------------------------------------------------------ what the panel reads

/// The page-global bits of the web's `S` the panel reads (drafts, busy buttons, inline notes,
/// open forms, the log filter, the attachment menu/edit, fetched handoffs). They survive switching
/// between tasks, as they did in the web board.
#[derive(Clone, Debug, Default)]
pub struct Ui {
    pub drafts: HashMap<String, String>,
    pub busy: HashSet<String>,
    pub notes: HashMap<String, (String, bool)>,
    pub meta_edit: HashMap<String, Vec<(String, String)>>,
    pub handoffs: HashMap<String, String>,
    /// "" means "all".
    pub log_filter: String,
    pub att_menu: Option<String>,
    pub att_edit: Option<String>,
    /// The header's Attached dropdown is open.
    pub att_drop: bool,
    /// The now stop's ⋯ terminal menu is open.
    pub term_menu: bool,
    /// The finished timeline stops opened in place.
    pub tl_open: HashSet<TlStep>,
    /// Whose reviewer popover is open, by their host login.
    pub rev_pop: Option<String>,
    /// Each reviewer's asks as fetched (`GET /reviewers/asks?user=`), or why they didn't load.
    pub rev_asks: HashMap<String, Result<Value, String>>,
}

impl Ui {
    pub fn log_filter(&self) -> &str {
        if self.log_filter.is_empty() { "all" } else { &self.log_filter }
    }
}

pub struct Ctx<'a> {
    pub state: &'a Value,
    pub task: Option<&'a Value>,
    /// The task's row from the page it opened from, shown until `task` answers.
    pub card: Option<&'a Value>,
    /// The panel's task ref (`S.taskRef`).
    pub r: &'a str,
    /// overview | what | context | log
    pub tab: &'a str,
    /// Tasks opened from the panel before this one (`taskTrail()`).
    pub trail: &'a [String],
    /// Why the task couldn't be fetched (`S.taskErr`).
    pub err: Option<&'a str>,
    pub ui: &'a Ui,
    /// Saved fold states by localStorage key ("open" / "closed").
    pub folds: &'a HashMap<String, String>,
    /// The Full log's entries under the current filter.
    pub log: &'a LogRows,
}

impl Ctx<'_> {
    /// `foldOpen(k, shut)`.
    fn fold_open(&self, f: Fold, shut: bool) -> bool {
        match self.folds.get(f.key()) {
            Some(v) => v == "open",
            None => !shut,
        }
    }

    /// The web's `note(grp)`.
    fn note(&self, grp: &str) -> Option<Node> {
        self.ui.notes.get(grp).map(|(s, err)| Node::Note { s: s.clone(), err: *err })
    }

    fn draft(&self, k: &str) -> String {
        self.ui.drafts.get(k).cloned().unwrap_or_default()
    }

    /// The web's `btn(label, act, o)`: a busy button reads "Sending…" and is disabled.
    fn btn(&self, label: &str, act: Act, look: Look, disabled: bool, tip: Option<&str>) -> Node {
        let busy = self.ui.busy.contains(&act.busy_key());
        Node::Btn {
            label: if busy { "Sending…".into() } else { label.to_string() },
            act,
            look,
            disabled: disabled || busy,
            tip: tip.filter(|t| !t.is_empty()).map(str::to_string),
        }
    }
}

// ------------------------------------------------------------------ small helpers (app.js)

/// `STATUS` / `stKey`.
pub fn st_key(t: &Value) -> &'static str {
    match s(t, "status") {
        "done" if b(t, "failed") => "failed",
        "queued" if b(t, "blocked") => "blocked",
        "planned" => "planned",
        "queued" => "queued",
        "working" => "working",
        "needs" => "needs",
        "done" => "done",
        _ => "queued",
    }
}

pub fn status_label(k: &str) -> &'static str {
    match k {
        "planned" => "Planned",
        "queued" => "Queued",
        "blocked" => "Blocked",
        "working" => "Working",
        "needs" => "Needs you",
        "done" => "Done",
        "failed" => "Failed",
        _ => "",
    }
}

fn st_tone(k: &str) -> Tone {
    match k {
        "planned" => Tone::Planned,
        "blocked" => Tone::Blocked,
        "working" | "waiting" => Tone::Working,
        "needs" => Tone::Needs,
        "done" => Tone::Done,
        "failed" => Tone::Failed,
        "idle" => Tone::Idle,
        "gone" => Tone::Gone,
        "closed" => Tone::Closed,
        _ => Tone::Queued,
    }
}

/// `prOpen`.
fn pr_open(p: &Value) -> bool {
    opt_s(p, "state").unwrap_or("OPEN").to_uppercase() == "OPEN"
}

/// `t.pr` when it has a number.
fn pr_of(t: &Value) -> Option<&Value> {
    obj(t, "pr").filter(|p| p.get("num").is_some_and(|n| !n.is_null()))
}

/// `awaitingMerge`.
fn awaiting_merge(t: &Value) -> bool {
    s(t, "status") == "done" && !b(t, "failed") && pr_of(t).is_some_and(pr_open)
}

/// `prStopped`: the stage's `stopped` object.
fn pr_stopped(t: &Value) -> Option<&Value> {
    if !awaiting_merge(t) {
        return None;
    }
    pr_of(t).and_then(|p| obj(p, "stage")).and_then(|st| obj(st, "stopped"))
}

/// `PR_STAGE_CLS`.
fn stage_tone(phase: &str) -> Tone {
    match phase {
        "checks" => Tone::Queued,
        "fix" | "rebase" => Tone::Failed,
        "lens" | "ask" | "review" | "rereview" => Tone::Review,
        "comments" => Tone::Needs,
        "merge" => Tone::Working,
        "merged" => Tone::Done,
        "declined" => Tone::Neutral,
        _ => Tone::Working,
    }
}

/// `sessionLive`.
fn session_live(t: &Value) -> bool {
    obj(t, "session").is_some_and(|x| s(x, "status") != "gone")
}

/// `startsByHand`.
fn starts_by_hand(t: &Value) -> bool {
    matches!(s(t, "status"), "queued" | "planned") && obj(t, "goal").is_none() && !b(t, "starting")
}

/// `uncommitted`.
fn uncommitted(v: &Value) -> String {
    match v {
        Value::Number(n) => match n.as_f64().unwrap_or(0.) {
            x if x == 0. => "Nothing".into(),
            _ => fmt::plural(n.as_i64().unwrap_or(0), "file", "files"),
        },
        Value::String(x) => x.clone(),
        other => js_string(other),
    }
}

/// JS `a === b` on fields that may be missing (`undefined`) or `null`.
fn js_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// `x.k` (None = undefined; a missing parent is undefined too).
fn field<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.as_object().and_then(|o| o.get(k))
}

/// `String(x)` for a JSON value.
fn js_string(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::String(x) => x.clone(),
        Value::Array(a) => a.iter().map(|x| if x.is_null() { String::new() } else { js_string(x) }).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".into(),
        other => other.to_string(),
    }
}

/// `esc(x)` of a possibly missing field: "" for null/undefined.
fn or_empty(v: &Value) -> String {
    if v.is_null() { String::new() } else { js_string(v) }
}

/// `savedLine`.
fn saved_line(t: &Value) -> String {
    let c = &t["context"];
    match opt_s(c, "saved_at") {
        Some(at) => format!(
            "Saved at {} · {} · {}",
            fmt::hhmm(at),
            fmt::plural(c["turns"].as_i64().unwrap_or(0), "turn", "turns"),
            fmt::plural(c["checkpoints"].as_i64().unwrap_or(0), "checkpoint", "checkpoints")
        ),
        None => String::new(),
    }
}

/// `hoursOpenAt`: "Thu 6am" (or "6pm" today) while work hours are closed; "" otherwise.
pub fn hours_open_at(state: &Value) -> String {
    let h = &state["work_hours"];
    if !h.is_object() || !b(h, "on") || b(h, "open") {
        return String::new();
    }
    opt_s(h, "next_open").map(fmt::day_clock).unwrap_or_default()
}

/// `sentNote`.
pub fn sent_note(state: &Value) -> &'static str {
    if state["midna"]["up"] == Value::Bool(false) { "Saved. It runs once Midna is back." } else { "Sent to Midna" }
}

/// `ref(x, p)`.
fn rf(v: &Value, p: &str) -> String {
    fmt::ref_of(v, p)
}

/// `who` in `overviewTab`.
fn who(t: &Value) -> String {
    opt_s(t, "who").or_else(|| obj(t, "session").and_then(|x| opt_s(x, "name"))).unwrap_or("The terminal").to_string()
}

// ------------------------------------------------------------------ status pill

/// `statusPill(t, k)` (with `prStatus`).
pub fn status_pill(t: &Value) -> Node {
    if awaiting_merge(t) {
        let p = pr_of(t).unwrap_or(&Value::Null);
        let num = or_empty(&p["num"]);
        if let Some(st) = pr_stopped(t) {
            let tip = if b(st, "asked") { format!("Its terminal asked you about PR #{num}") } else { format!("Its terminal stopped on PR #{num} before it was finished") };
            return Node::Pill { s: "Needs you".into(), tone: Tone::Needs, tip: Some(tip) };
        }
        let tip = format!("The work is done; PR #{num} isn’t merged yet");
        if let Some(stage) = obj(p, "stage") {
            return Node::Pill { s: or_empty(&stage["label"]), tone: stage_tone(s(stage, "phase")), tip: Some(tip) };
        }
        return Node::Pill { s: "Awaiting merge".into(), tone: Tone::Working, tip: Some(tip) };
    }
    let k = st_key(t);
    if k == "queued" && b(t, "starting") {
        return pill("Starting", Tone::Queued);
    }
    pill(status_label(k), st_tone(k))
}

// ------------------------------------------------------------------ panel

fn back_btn(c: &Ctx) -> Option<Node> {
    let prev = c.trail.last()?;
    Some(c.btn("", Act::new("task-back", "", "", ""), Look::Icon(Icon::Back), false, Some(&format!("Back to {prev}"))))
}

fn close_btn(c: &Ctx) -> Node {
    c.btn("", Act::new("close-panel", "", "", ""), Look::Icon(Icon::Close), false, None)
}

/// `panelTop(t)`: back, the breadcrumb (goal › ref) and close.
fn top_row(c: &Ctx, t: Option<&Value>) -> Node {
    let mut crumb = Vec::new();
    if let Some(g) = t.and_then(|t| obj(t, "goal")).filter(|g| g.get("id").is_some_and(|x| !x.is_null()) || opt_s(g, "ref").is_some()) {
        let gr = rf(g, "G");
        crumb.push(Node::Link { s: format!("{gr} {}", or_empty(&g["name"])), go: Go::Goal(gr), tip: None, look: LinkLook::Crumb });
        crumb.push(txt("›", St::Muted));
    }
    crumb.push(txt(t.map(|t| rf(t, "T")).unwrap_or_else(|| c.r.to_string()), St::Mono));
    let mut top = Vec::new();
    top.extend(back_btn(c));
    top.push(el(K::Crumb, crumb));
    top.push(close_btn(c));
    el(K::Top, top)
}

/// `headChips(t)`: project, Jira, the goal position and Attached, each only when the task has it.
fn head_chips(c: &Ctx, t: &Value) -> Option<Node> {
    let mut chips = Vec::new();
    if s(t, "kind") == "human" {
        chips.push(Node::Pill { s: "Human task".into(), tone: Tone::Needs, tip: Some("Only you can do it; no agent works on it".into()) });
    }
    if s(t, "kind") == "design" {
        chips.push(Node::Pill { s: "Design".into(), tone: Tone::Design, tip: Some("Ends with its design attached, not a PR".into()) });
    }
    let project = or_empty(&t["project"]);
    if !project.is_empty() {
        let path = opt_s(t, "repo_path")
            .or_else(|| arr(c.state, "projects").iter().find(|p| s(p, "name") == project).and_then(|p| opt_s(p, "path")))
            .map(|p| format!("Project · {p}"));
        chips.push(c.btn(&project, Act::new("open-project", &project, "", ""), Look::Chip { on: false }, false, path.as_deref()));
    }
    let j = &t["jira"];
    if let Some(key) = opt_s(j, "key") {
        let label = match opt_s(j, "status") {
            Some(st) => format!("{key} {st}"),
            None => key.to_string(),
        };
        chips.push(match opt_s(j, "url") {
            Some(url) => Node::Link { s: label, go: Go::Url(url.to_string()), tip: Some(format!("Open {key} in Jira")), look: LinkLook::JiraChip },
            None => txt(label, St::JiraChip),
        });
    }
    chips.extend(goal_chip(t));
    let list: Vec<Value> = arr(t, "attachments").iter().chain(arr(t, "goal_attachments")).cloned().collect();
    if !list.is_empty() {
        chips.push(c.btn(&format!("Attached {}", list.len()), Act::new("att-drop", "", "", ""), Look::Chip { on: c.ui.att_drop }, false, None));
        if c.ui.att_drop {
            chips.push(el(K::Drop, list.iter().map(drop_item).collect()));
        }
    }
    (!chips.is_empty()).then(|| el(K::Head, chips))
}

/// `goalChip(t)`: "Wave 5 · 18 of 20", opening the goal.
fn goal_chip(t: &Value) -> Option<Node> {
    let g = obj(t, "goal").filter(|g| g.get("id").is_some_and(|x| !x.is_null()) || opt_s(g, "ref").is_some())?;
    let gr = rf(g, "G");
    let wave = t["wave"].as_i64();
    let (pos, total) = (g["position"].as_i64(), g["total"].as_i64().unwrap_or(0));
    let mut words = Vec::new();
    words.extend(wave.map(|w| format!("Wave {w}")));
    words.extend(pos.filter(|p| total > 0 && *p <= total).map(|p| format!("{p} of {total}")));
    if words.is_empty() {
        return None;
    }
    let mut tip = format!("Open {gr}");
    tip.extend(wave.map(|w| format!(" at wave {w}")));
    tip.extend(opt_s(g, "next_ref").map(|n| format!(" · next up: {n}")));
    Some(Node::Link { s: words.join(" · "), go: Go::Goal(gr), tip: Some(tip), look: LinkLook::Chip })
}

/// `dropItem(a)`: one attachment in the Attached dropdown; its summary, when it has one, rides
/// last for the hover card.
pub fn drop_item(a: &Value) -> Node {
    let kind = ATT_KINDS.iter().find(|k| **k == s(a, "kind")).copied().unwrap_or("other");
    let title = or_empty(&a["title"]);
    let name = match att_href(a) {
        Some(h) => Node::Link { s: title, go: Go::Url(h), tip: None, look: LinkLook::AttName },
        None => txt(title, St::Strong),
    };
    let mut meta = vec![att_kind_label(kind).to_string()];
    meta.extend(opt_s(a, "task").or(opt_s(a, "goal")).map(str::to_string));
    meta.extend(opt_s(a, "at").map(fmt::ago));
    let url = s(a, "url");
    let ftype = att_file_type(url);
    let icon = ftype.as_ref().map_or(kind, |(t, _)| *t);
    let rows: Vec<Node> = att_hover_rows(a, kind, ftype)
        .into_iter()
        .map(|(k, v)| el(K::KvRow, vec![txt(k, St::LrowKey), txt(v, St::Plain)]))
        .collect();
    let mut kids = vec![name, txt(meta.join(" · "), St::Small), el(K::Kv, rows)];
    kids.extend(opt_s(a, "summary").map(|x| txt(x, St::Small)));
    el(K::DropItem(kind, icon), kids)
}

/// The hover card under an attachment's title: where it lives, what it is, who added it when.
fn att_hover_rows(a: &Value, kind: &str, ftype: Option<(&'static str, String)>) -> Vec<(&'static str, String)> {
    let url = s(a, "url");
    let mut rows = Vec::new();
    if is_web(url) {
        rows.push(("Link", url.to_string()));
    } else if !url.is_empty() {
        let p = url.trim_end_matches('/');
        match p.rsplit_once('/') {
            Some((dir, file)) => {
                rows.push(("File", file.to_string()));
                if !dir.is_empty() {
                    rows.push(("Folder", fmt::short_path(dir)));
                }
            }
            None => rows.push(("File", p.to_string())),
        }
    }
    if let Some((t, ext)) = ftype {
        rows.push(("Type", format!("{} · {}", att_type_label(t), ext.to_uppercase())));
    }
    rows.push(("Kind", att_kind_label(kind).to_string()));
    if let Some(r) = opt_s(a, "task").or(opt_s(a, "goal")) {
        rows.push(("On", r.to_string()));
    }
    let when = opt_s(a, "at").and_then(fmt::parse).map(|t| fmt::local(t).format("%b %-d, %Y, %-I:%M %p").to_string());
    let added = [opt_s(a, "added_by").map(str::to_string), when].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    if !added.is_empty() {
        rows.push(("Added", added));
    }
    rows
}

/// What an attachment's file is, from its extension, and that extension: picks its icon in the
/// dropdown. None for a page, a folder, or a file of no type we draw.
pub fn att_file_type(url: &str) -> Option<(&'static str, String)> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    // A link's host isn't a file name: only its path counts.
    let path = if is_web(path) { path.splitn(4, '/').nth(3)? } else { path };
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty())?.1.to_ascii_lowercase();
    let t = match ext.as_str() {
        "mp4" | "mov" | "m4v" | "webm" | "mkv" | "avi" => "video",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "svg" | "bmp" | "tif" | "tiff" => "image",
        "mp3" | "wav" | "m4a" | "aac" | "flac" | "ogg" => "audio",
        "pdf" => "pdf",
        "csv" | "tsv" | "xls" | "xlsx" | "numbers" => "sheet",
        "zip" | "tar" | "gz" | "tgz" | "xz" | "7z" | "rar" => "archive",
        "rs" | "dart" | "js" | "ts" | "tsx" | "jsx" | "py" | "go" | "swift" | "kt" | "java" | "rb" | "sh" | "json"
        | "yaml" | "yml" | "toml" | "html" | "css" | "sql" | "diff" | "patch" => "code",
        "md" | "txt" | "log" | "rtf" | "doc" | "docx" => "text",
        _ => return None,
    };
    Some((t, ext))
}

/// A file type's name in the hover card.
pub fn att_type_label(t: &str) -> &'static str {
    match t {
        "video" => "Video",
        "image" => "Image",
        "audio" => "Audio",
        "pdf" => "PDF",
        "sheet" => "Spreadsheet",
        "archive" => "Archive",
        "code" => "Code",
        "text" => "Text",
        _ => "File",
    }
}

/// `taskPanel()`.
/// A task just opened, before its own answer: what its row knew (title, project, PR), the rest
/// still loading. Nothing to press yet but Back, close and the chips.
fn card_panel(c: &Ctx, t: &Value) -> Node {
    let mut kids = vec![top_row(c, Some(t)), txt(or_empty(&t["title"]), St::Title)];
    kids.extend(head_chips(c, t));
    if pr_of(t).is_some() {
        let mut nodes = pr_nodes(c, t);
        nodes.reverse();
        kids.push(el(K::Tline, nodes));
    }
    kids.push(txt("Loading…", St::Muted));
    el(K::Col, kids)
}

pub fn panel(c: &Ctx) -> Node {
    let Some(t) = c.task else {
        if let Some(card) = c.card.filter(|_| c.err.is_none()) {
            return card_panel(c, card);
        }
        let msg = match c.err {
            Some(e) => Node::Note { s: e.to_string(), err: true },
            None => txt("Loading…", St::Muted),
        };
        return el(K::Col, vec![top_row(c, None), msg]);
    };
    let r = rf(t, "T");
    let alert = arr(c.state, "alerts").iter().find(|a| fmt::same_task(s(a, "task"), &r));
    let tab = if matches!(c.tab, "what" | "context" | "log") { c.tab } else { "overview" };

    let mut kids = vec![top_row(c, Some(t))];
    if let Some(e) = c.err {
        kids.push(Node::Note { s: e.to_string(), err: true });
    }
    kids.push(txt(or_empty(&t["title"]), St::Title));
    kids.extend(head_chips(c, t));
    kids.extend(banner(c, t, alert));
    let body = match tab {
        "what" => vec![what_view(t)],
        "context" => context_tab(c, t),
        "log" => log_tab(c, t),
        _ => timeline(c, t),
    };
    kids.push(el(K::Body, body));
    kids.push(panel_foot(c, t, tab));
    el(K::Col, kids)
}

/// `panelFoot(t, tab)`: What to do, Context and the full log; Timeline goes back.
fn panel_foot(c: &Ctx, t: &Value, tab: &str) -> Node {
    // The task carries only its newest page of the log; `log_total` counts them all.
    let n = t["log_total"].as_u64().map_or(arr(t, "log").len(), |n| n as usize);
    let mut kids = Vec::new();
    for (id, label) in [("overview", "Timeline"), ("what", "What to do"), ("context", "Context"), ("log", "Full log")] {
        if id == "overview" && tab == "overview" {
            continue;
        }
        kids.push(c.btn(label, Act::new("tab", id, "", ""), Look::Foot { on: tab == id }, false, None));
        if id == "log" && n > 0 {
            kids.push(txt(n.to_string(), St::Count));
        }
    }
    el(K::Foot, kids)
}

/// `whatView(t)`.
fn what_view(t: &Value) -> Node {
    match opt_s(t, "detail") {
        Some(d) => note_body(d),
        None => txt("Nothing written yet.", St::Plain),
    }
}

/// `answerBtns(r)`.
fn answer_btns(c: &Ctx, r: &str) -> Vec<Node> {
    let at = hours_open_at(c.state);
    let grp = format!("ask:{r}");
    let mut out = vec![c.btn("Send answer", Act::new("answer", "", r, &grp), Look::Primary, false, None)];
    if !at.is_empty() {
        out.push(c.btn(
            &format!("Send at {at}"),
            Act::new("answer", "morning", r, &grp),
            Look::Plain,
            false,
            Some("Hold the answer until work hours start; the task doesn’t start before then"),
        ));
    }
    out
}

/// `askOptions(ask, r)`: what the task asks, option by option. A `tb question`'s options answer it with
/// one click; a picker's are answered in its terminal.
fn ask_options(c: &Ctx, ask: &Value, r: &str, grp: &str) -> Vec<Node> {
    let picker = b(ask, "picker");
    let qs = arr(ask, "questions");
    // A picker with one single-select question answers in a click; otherwise each click toggles a
    // choice (one per single-select question) and Send answers once every question has one.
    let one_click = picker && qs.len() == 1 && !b(&qs[0], "multi");
    let picked = picked_of(&c.draft(&format!("pick:{r}")));
    let mut out = Vec::new();
    for (qi, q) in qs.iter().enumerate() {
        if qs.len() > 1 {
            out.push(txt(or_empty(&q["question"]), St::Strong));
        }
        let opts = arr(q, "options");
        let described = opts.iter().any(|o| opt_s(o, "description").is_some_and(|d| !d.is_empty()));
        let mut row = Vec::new();
        for (oi, o) in opts.iter().enumerate() {
            let label = or_empty(&o["label"]);
            let key = format!("{qi}:{oi}");
            let opt = if !picker {
                c.btn(&label, Act::new("answer-opt", label.as_str(), r, grp), Look::Opt { on: false }, false, None)
            } else if one_click {
                c.btn(&label, Act::new("pick", oi.to_string(), r, grp), Look::Opt { on: false }, false, None)
            } else {
                let on = picked.contains(&key);
                c.btn(&label, Act::new("pick-opt", key, r, grp), Look::Opt { on }, false, None)
            };
            if !described {
                row.push(opt);
                continue;
            }
            let mut kids = vec![opt];
            if let Some(d) = opt_s(o, "description").filter(|d| !d.is_empty()) {
                kids.push(txt(d, St::Small));
            }
            out.push(el(K::Row, kids));
        }
        if !row.is_empty() {
            out.push(el(K::Row, row));
        }
    }
    if picker && !one_click {
        let all = (0..qs.len()).all(|qi| picked.iter().any(|k| k.starts_with(&format!("{qi}:"))));
        out.push(el(K::Row, vec![c.btn("Send", Act::new("pick", "", r, grp), Look::Primary, !all, None)]));
    }
    out
}

/// The picker choices toggled so far ("question:option"), kept with the drafts until sent.
pub fn picked_of(draft: &str) -> Vec<String> {
    draft.split(',').filter(|k| !k.is_empty()).map(str::to_string).collect()
}

/// Toggles `key` ("question:option"); a single-select question keeps one choice.
pub fn toggle_pick(draft: &str, key: &str, ask: &Value) -> String {
    let mut picked = picked_of(draft);
    let qi = key.split(':').next().unwrap_or("");
    let multi = qi.parse::<usize>().ok().and_then(|i| arr(ask, "questions").get(i)).is_some_and(|q| b(q, "multi"));
    if let Some(i) = picked.iter().position(|k| k == key) {
        picked.remove(i);
    } else {
        if !multi {
            picked.retain(|k| !k.starts_with(&format!("{qi}:")));
        }
        picked.push(key.to_string());
    }
    picked.join(",")
}

/// What the pick endpoint takes: the chosen option indexes, question by question.
pub fn pick_answers(draft: &str, ask: &Value) -> Vec<Vec<usize>> {
    let picked: Vec<(usize, usize)> = picked_of(draft).iter().filter_map(|k| k.split_once(':')).filter_map(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))).collect();
    (0..arr(ask, "questions").len())
        .map(|qi| {
            let mut v: Vec<usize> = picked.iter().filter(|(a, _)| *a == qi).map(|(_, b)| *b).collect();
            v.sort_unstable();
            v
        })
        .collect()
}

fn answer_field(c: &Ctx, r: &str) -> Node {
    let k = format!("answer:{r}");
    Node::Field { value: c.draft(&k), key: k, placeholder: "", multi: true, mono: false }
}

/// The web links in a human task's steps, each once.
fn step_links(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in text.split(|ch: char| ch.is_whitespace() || matches!(ch, '(' | ')' | '<' | '>' | '[' | ']' | '"' | '`')) {
        let u = w.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'']);
        if is_web(u) && !out.iter().any(|x| x == u) {
            out.push(u.to_string());
        }
    }
    out
}

fn link_label(u: &str) -> String {
    let host = u.split("://").nth(1).unwrap_or(u).split('/').next().unwrap_or(u);
    format!("Open {}", host.trim_start_matches("www."))
}

/// A human task (`kind: human`): the owner's steps, their links, Done with a note for the agents
/// waiting on it, and Talk it over, which opens a Claude terminal to ask about it in. No agent ever
/// works on it.
fn human_tab(c: &Ctx, t: &Value) -> Vec<Node> {
    let r = rf(t, "T");
    let grp = format!("did:{r}");
    let status = s(t, "status");
    let detail = opt_s(t, "detail").unwrap_or("");
    let mut out = Vec::new();
    if status == "done" {
        let mut bx = vec![txt("Done by you", St::BoxLabel)];
        if let Some(n) = opt_s(t, "owner_note") {
            bx.push(txt(n, St::Plain));
        }
        out.push(el(K::Box(BoxTone::Done), bx));
    } else {
        let mut bx = vec![txt("Yours to do", St::BoxLabel)];
        if let Some(w) = opt_s(t, "waiting").filter(|_| status == "queued") {
            bx.push(txt(w, St::Small));
        }
        bx.push(note_body(detail));
        let links = step_links(detail);
        if !links.is_empty() {
            bx.push(el(K::Row, links.iter().map(|u| c.btn(&link_label(u), Act::new("open-url", u, &r, &grp), Look::Plain, false, Some(u))).collect()));
        }
        let k = format!("did:{r}");
        bx.push(Node::Field { value: c.draft(&k), key: k, placeholder: "", multi: true, mono: false });
        let mut row = vec![
            c.btn("Done", Act::new("did", "", &r, &grp), Look::Primary, false, Some("Anything waiting for this goes on, with what you wrote")),
            c.btn("Talk it over", Act::new("talk", "", &r, &grp), Look::Out, false, Some("Ask Claude about it in a Midna terminal")),
        ];
        row.extend(c.note(&grp));
        bx.push(el(K::Row, row));
        out.push(el(K::Box(BoxTone::Ask), bx));
    }
    out.extend(blocked_row(c, t));
    out
}

/// `tlNode(st, head, body, card)`.
fn tl_node(st: TlSt, head: Node, body: Vec<Node>, card: bool) -> Node {
    let mut kids = vec![head];
    kids.extend(body);
    el(K::TlNode { st, card }, kids)
}

/// `tlHead(name, sub, at)`.
fn tl_head(name: Node, sub: &str, at: &str) -> Node {
    let mut kids = vec![name];
    if !sub.is_empty() {
        kids.push(txt(sub, St::TlS));
    }
    if !at.is_empty() {
        kids.push(txt(at, St::Time));
    }
    el(K::TlHead, kids)
}

fn tl_name(name: &str) -> Node {
    txt(name, St::TlH)
}

/// `timeline(t)`: what's left on top, then what it's doing now, the steps it passed and where it
/// started at the bottom, so the stop it's at reads first.
fn timeline(c: &Ctx, t: &Value) -> Vec<Node> {
    if s(t, "kind") == "human" {
        return human_tab(c, t);
    }
    let status = s(t, "status");
    let mut nodes = Vec::new();
    if opt_s(t, "started_at").is_some() && !matches!(status, "queued" | "planned") {
        nodes.push(started_node(c, t));
    }
    nodes.extend(arr(t, "step_results").iter().enumerate().map(|(ix, x)| result_node(c, t, ix, x)));
    if status == "done" {
        nodes.push(done_node(c, t));
    }
    if status != "done" || pr_stopped(t).is_some() {
        nodes.push(now_node(c, t));
    }
    nodes.extend(pr_nodes(c, t));
    nodes.reverse();
    vec![el(K::Tline, nodes)]
}

/// A finished stop's head, opening in place onto `detail`; just the head when there's nothing to show.
fn tl_step(c: &Ctx, step: TlStep, head: Node, detail: Vec<Node>) -> Node {
    if detail.is_empty() {
        return head;
    }
    el(K::TlStep { step, open: c.ui.tl_open.contains(&step) }, vec![head, el(K::Det, detail)])
}

fn det_row(label: &str, value: Node) -> Node {
    el(K::DetRow, vec![txt(label, St::LrowKey), value])
}

/// `startedNode(t)`: "In Midna 3 · 2 closed earlier".
fn started_node(c: &Ctx, t: &Value) -> Node {
    let name = obj(t, "session").and_then(|x| opt_s(x, "name")).or(opt_s(t, "who"));
    let cur = field(&t["session"], "id");
    let closed = arr(t, "terminals").iter().filter(|x| !js_eq(field(x, "id"), cur) && opt_s(x, "status").is_none_or(|st| st == "gone")).count();
    let mut sub = Vec::new();
    sub.extend(name.map(|n| format!("In {n}")));
    if closed > 0 {
        sub.push(format!("{closed} closed earlier"));
    }
    let head = tl_head(tl_name("Started"), &sub.join(" · "), &fmt::hhmm(s(t, "started_at")));
    tl_node(TlSt::Done, tl_step(c, TlStep::Started, head, started_detail(c, t)), vec![], false)
}

/// Started, opened: the task's terminals (the one it's in with its state and Focus, earlier ones
/// when they closed), its branch and what isn't committed.
fn started_detail(c: &Ctx, t: &Value) -> Vec<Node> {
    let r = rf(t, "T");
    let cur = field(&t["session"], "id");
    let mut out: Vec<Node> = arr(t, "terminals")
        .iter()
        .map(|x| {
            let live = cur.is_some() && js_eq(field(x, "id"), cur) && session_live(t);
            let mut kids = vec![txt(s(x, "name"), St::Plain)];
            if live {
                kids.push(term_state(x));
                kids.push(c.btn("", Act::new("focus", "", &r, format!("manage:{r}")), Look::Icon(Icon::Focus), false, Some("Focus in Midna")));
            } else if opt_s(x, "status").is_none_or(|st| st == "gone") {
                let closed = opt_s(x, "gone_at").map(fmt::hhmm).filter(|h| !h.is_empty());
                kids.push(txt(closed.map_or("Closed".to_string(), |h| format!("Closed {h}")), St::Time));
            } else {
                kids.push(term_state(x));
            }
            el(K::DetTerm { live }, kids)
        })
        .collect();
    let w = &t["context"]["where"];
    if let Some(br) = opt_s(w, "branch") {
        out.push(det_row("Branch", txt(br, St::Mono)));
    }
    let mut changes = Vec::new();
    match &w["uncommitted"] {
        Value::Null => {}
        Value::Number(n) if n.as_f64() == Some(0.) => changes.push("Nothing uncommitted".to_string()),
        v => changes.push(format!("{} not committed", uncommitted(v))),
    }
    let last = opt_s(t, "last_commit_at")
        .or_else(|| arr(t, "log").iter().filter(|e| s(e, "kind") == "commit").map(|e| s(e, "at")).max())
        .map(fmt::hhmm)
        .filter(|h| !h.is_empty());
    if let Some(h) = last {
        changes.push(format!("{} {h}", if changes.is_empty() { "Last commit" } else { "last commit" }));
    }
    if !changes.is_empty() {
        out.push(det_row("Changes", txt(changes.join(" · "), St::Plain)));
    }
    out
}

/// A terminal's state, as words beside its name (the row's dot is its one marker): what's running in
/// its background, else its status.
fn term_state(x: &Value) -> Node {
    let st = s(x, "status");
    if !x["background"].is_null() {
        return txt(fmt::background_label(&x["background"]), St::Time);
    }
    let label = match st {
        "working" => "Working",
        "waiting" => "Waiting",
        "needs" => "Needs you",
        "offline" => "Offline",
        _ => "Idle",
    };
    txt(label, St::Time)
}

/// A step that ran a round (`step_results`): its headline, opening onto what came of it.
fn result_node(c: &Ctx, t: &Value, ix: usize, x: &Value) -> Node {
    let open = x["open"].as_i64().unwrap_or(0);
    let mut body = Vec::new();
    if b(x, "stale") {
        body.push(el(K::Line, vec![pill("Moved since", Tone::Needs)]));
    }
    let head = tl_head(tl_name(s(x, "name")), s(x, "headline"), &fmt::hhmm(s(x, "at")));
    tl_node(if open > 0 { TlSt::Warn } else { TlSt::Done }, tl_step(c, TlStep::Result(ix), head, result_detail(t, x)), body, false)
}

/// The step results a findings link opens: the ones holding findings.
pub fn findings_steps(t: &Value) -> Vec<TlStep> {
    arr(t, "step_results").iter().enumerate().filter(|(_, x)| !arr(x, "findings").is_empty()).map(|(ix, _)| TlStep::Result(ix)).collect()
}

/// A step result, opened: what came of it, its findings with their state and what it ran on (the
/// task's devices, the commit).
fn result_detail(t: &Value, x: &Value) -> Vec<Node> {
    let mut out = Vec::new();
    if let Some(note) = opt_s(x, "note").filter(|n| *n != s(x, "headline")) {
        out.push(txt(note, St::DetText));
    }
    let findings = arr(x, "findings");
    if !findings.is_empty() {
        out.push(el(K::List, findings.iter().map(finding_item).collect()));
    }
    let mut on: Vec<String> = lent_devices(t).into_iter().map(str::to_string).collect();
    if let Some(h) = opt_s(x, "head") {
        on.push(format!("commit {}", h.chars().take(7).collect::<String>()));
    }
    if !on.is_empty() {
        out.push(det_row("Ran on", txt(on.join(" · "), St::Plain)));
    }
    out
}

/// `doneNode(t)`: Done or Failed, with its summary.
fn done_node(c: &Ctx, t: &Value) -> Node {
    let r = rf(t, "T");
    let act = format!("act:{r}");
    let failed = b(t, "failed");
    let mut body = vec![txt(opt_s(t, "summary").or(opt_s(t, "latest")).unwrap_or("No summary was given."), St::TlText)];
    for (k, label) in [("no_pr", "No PR:"), ("no_evidence", "No evidence:")] {
        if let Some(why) = opt_s(t, k) {
            let mut line = vec![txt(label, St::Strong)];
            line.extend(linkify(c, why));
            body.push(el(K::Line, line));
        }
    }
    let mut row = vec![c.btn(if failed { "Try again" } else { "Queue again" }, Act::new("requeue", "", &r, &act), Look::Plain, false, None)];
    if session_live(t) {
        row.push(c.btn("Close its terminal", Act::new("close-term", "", &r, &act), Look::Danger, false, None));
    }
    body.push(el(K::Row, row));
    body.extend(c.note(&act));
    let name = if failed { "Failed" } else { "Done" };
    tl_node(if failed { TlSt::Fail } else { TlSt::Done }, tl_head(tl_name(name), "", &fmt::hhmm(s(t, "finished_at"))), body, false)
}

/// `nowNode(t)`: what the task is doing now, with what it needs from you and its terminal's controls.
fn now_node(c: &Ctx, t: &Value) -> Node {
    let r = rf(t, "T");
    let who = who(t);
    let act = format!("act:{r}");
    let status = s(t, "status");
    let working = status == "working";
    let warn = b(t, "lost") || status == "needs" || pr_stopped(t).is_some();
    let sub = if working {
        let mut sub = vec![fmt::run_span(t)];
        sub.extend(opt_s(t, "updated_at").map(|at| format!("updated {}", fmt::ago(at))));
        sub.into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")
    } else {
        String::new()
    };
    // What it needs shows in the banner up top; here it just waits, with when it asked.
    if warn {
        // Lost or a failed start isn't a question: nothing was asked.
        let asked = status == "needs" && !b(t, "lost") && !matches!(s(t, "needs_reason"), "start_failed" | "compact_failed");
        let (sub, at) = match opt_s(t, "updated_at").filter(|_| asked) {
            Some(at) => {
                let mut sub = vec![format!("Asked {}", fmt::ago(at))];
                sub.extend(Some(fmt::span_between(s(t, "started_at"), at)).filter(|x| !x.is_empty()).map(|x| format!("after {x} of work")));
                (sub.join(", "), fmt::hhmm(at))
            }
            None => (String::new(), String::new()),
        };
        let mut out: Vec<Node> = blocked_row(c, t).into_iter().collect();
        if status != "done" {
            out.extend(term_actions(c, t));
        }
        return tl_node(TlSt::Warn, tl_head(txt("Waiting on you", St::TlHWarn), &sub, &at), out, true);
    }
    let mut out = Vec::new();
    if let Some(latest) = opt_s(t, "latest").filter(|_| working) {
        out.push(txt(latest, St::TlText));
    }

    if let Some(at) = fmt::outside(t) {
        let project = opt_s(&t["outside"], "project").unwrap_or("its project");
        out.push(el(
            K::Box(BoxTone::Ask),
            vec![txt(format!("{who} is working outside {project}"), St::BoxLabel), txt(format!("In {at}. The board doesn’t track that work."), St::Plain)],
        ));
    }

    if let Some(label) = opt_s(&t["line"], "label") {
        let when = match opt_s(&t["line"], "after") {
            Some(a) => format!("It starts there by itself once {a} is done."),
            None => "It starts there by itself next.".to_string(),
        };
        out.push(el(K::Box(BoxTone::Info), vec![txt(label, St::BoxLabel), txt(when, St::Plain)]));
    }

    if starts_by_hand(t) {
        let mut st = vec![el(
            K::Row,
            vec![
                c.btn("Start", Act::new("start", "new", &r, &act), Look::PrimaryWide, false, None),
                c.btn("Start when the repo’s free", Act::new("start", "queue", &r, &act), Look::SoftWide, false, None),
            ],
        )];
        st.extend(c.note(&act));
        if let Some(w) = opt_s(t, "waiting") {
            st.push(txt(format!("Not starting yet: {w}. Start runs it now anyway."), St::WarnHelp));
        }
        out.push(el(K::Stack, st));
    } else if status == "planned" {
        let mut st = vec![el(
            K::Row,
            vec![c.btn(
                "Queue it now",
                Act::new("queue-planned", "", &r, &act),
                Look::Soft,
                false,
                Some("Take it out of the plan and queue it; the goal’s rules decide when it starts"),
            )],
        )];
        st.extend(c.note(&act));
        out.push(el(K::Stack, st));
    }

    out.extend(blocked_row(c, t));
    if status != "done" {
        out.extend(term_actions(c, t));
    }
    tl_node(TlSt::Now, tl_head(status_pill(t), &sub, ""), out, true)
}

/// `banner(t, alert)`: what the task needs from you, full width under the chips: a question with its
/// options and Write an answer, a lost or failed start with its ways back, a stuck check-in, or the
/// PR's terminal stopping. A board alert about the task shows in it too.
fn banner(c: &Ctx, t: &Value, alert: Option<&Value>) -> Option<Node> {
    let r = rf(t, "T");
    let who = who(t);
    let act = format!("act:{r}");
    let status = s(t, "status");
    let (tone, mut bx) = if b(t, "lost") {
        let saved = saved_line(t);
        let claude = opt_s(t, "claude_session_id").is_some();
        let grp = format!("lost:{r}");
        // Signs its work shipped from another terminal after this one closed (`lost_done.rs`).
        let found = arr(t, "looks_finished");
        let label = if found.is_empty() { "Terminal lost" } else { "Looks finished" };
        let mut bx = vec![txt(label, St::BoxLabel), txt(opt_s(t, "latest").unwrap_or("The terminal closed before the task was done."), St::BannerText)];
        for f in found {
            let line = format!("{} · {}", s(f, "what"), fmt::hhmm(s(f, "at")));
            bx.push(match opt_s(f, "url").filter(|u| is_web(u)) {
                Some(u) => Node::Link { s: line, go: Go::Url(u.to_string()), tip: None, look: LinkLook::Small },
                None => txt(line, St::Small),
            });
        }
        if !saved.is_empty() {
            bx.push(txt(saved, St::Small));
        }
        let mut row = vec![
            c.btn("Resume in a new terminal", Act::new("resume", "fresh", &r, &grp), Look::Primary, false, None),
            c.btn(
                "Reopen the old conversation",
                Act::new("resume", "reopen", &r, &grp),
                Look::Plain,
                !claude,
                (!claude).then_some("No conversation was saved for this task"),
            ),
            // A new terminal checks the work shipped and hands it in with `tb submit`.
            c.btn("Mark done", Act::new("resume", "finish", &r, &grp), Look::Plain, false, None),
        ];
        row.extend(c.note(&grp));
        bx.push(el(K::Row, row));
        bx.push(c.btn("See what the new terminal gets", Act::new("tab", "context", "", ""), Look::Link, false, None));
        (BoxTone::Lost, bx)
    } else if status == "needs" && s(t, "needs_reason") == "start_failed" {
        let mut row = vec![
            c.btn("New Midna terminal", Act::new("start", "new", &r, &act), Look::Primary, false, None),
            c.btn("Queue in Midna", Act::new("start", "queue", &r, &act), Look::Plain, false, None),
        ];
        row.extend(c.note(&act));
        let bx = vec![
            txt("Couldn’t start in Midna", St::BoxLabel),
            txt(opt_s(t, "latest").unwrap_or("Midna didn’t start a terminal for this task."), St::BannerText),
            el(K::Row, row),
        ];
        (BoxTone::Lost, bx)
    } else if status == "needs" && s(t, "needs_reason") == "compact_failed" {
        let grp = format!("compact:{r}");
        let mut row = vec![
            c.btn("Start fresh with the handoff", Act::new("resume", "fresh", &r, &grp), Look::Primary, false, None),
            c.btn("Reopen the old conversation", Act::new("resume", "reopen", &r, &grp), Look::Plain, false, None),
        ];
        row.extend(c.note(&grp));
        let bx = vec![
            txt("Couldn’t compact its conversation", St::BoxLabel),
            txt(opt_s(t, "latest").unwrap_or("The board couldn’t compact its cold conversation before resuming it."), St::BannerText),
            el(K::Row, row),
        ];
        (BoxTone::Lost, bx)
    } else if status == "needs" {
        let grp = format!("ask:{r}");
        let mut bx = vec![txt(opt_s(t, "question").or(opt_s(t, "latest")).unwrap_or("It’s waiting for you in Midna."), St::BannerText)];
        bx.extend(ask_options(c, &t["ask"], &r, &grp));
        // A human task has no terminal to answer in: its box below holds Done and Talk it over.
        if s(t, "kind") == "human" {
            return Some(el(K::Banner(BoxTone::Ask), bx));
        }
        let mut row = step_btns(c, t, &r, &grp);
        // Its terminal when it has one; a question with none reopens one that asks it again; otherwise a new one, picking up from its handoff.
        let write = if session_live(t) || opt_s(t, "question").is_some_and(|q| !q.is_empty()) {
            Act::new("focus", "", &r, &grp)
        } else {
            Act::new("resume", "fresh", &r, &grp)
        };
        row.push(c.btn("Write an answer", write, Look::Out, false, None));
        row.extend(c.note(&grp));
        bx.push(el(K::Row, row));
        (BoxTone::Ask, bx)
    } else if let Some(why) = fmt::stuck(t).filter(|_| status == "working") {
        let mut bx = vec![txt(format!("{who} may be stuck"), St::BoxLabel), txt(why, St::BannerText)];
        if let Some(tail) = opt_s(&t["stuck"], "tail").filter(|x| !x.is_empty()) {
            bx.push(fold(Fold::Tail, c.fold_open(Fold::Tail, false), txt("Its terminal’s last lines", St::Small), vec![txt(tail, St::Code)]));
        }
        (BoxTone::Ask, bx)
    } else if let Some(stopped) = pr_stopped(t) {
        let p = pr_of(t).unwrap_or(&Value::Null);
        let term = arr(t, "terminals").iter().find(|x| js_eq(field(x, "id"), field(&p["stage"], "session"))).and_then(|x| opt_s(x, "name")).unwrap_or("Its terminal");
        let grp = format!("ask:{r}");
        let mut row = answer_btns(c, &r);
        row.push(c.btn("Focus in Midna", Act::new("focus", "", &r, &grp), Look::Plain, false, None));
        row.extend(c.note(&grp));
        let bx = vec![
            txt(format!("{term} {}", if b(stopped, "asked") { "asks you about the PR" } else { "stopped on the PR" }), St::BoxLabel),
            txt(or_empty(&stopped["message"]), St::BannerText),
            answer_field(c, &r),
            el(K::Row, row),
        ];
        (BoxTone::Ask, bx)
    } else {
        (BoxTone::Ask, Vec::new())
    };
    if let Some(a) = alert {
        bx.push(txt(or_empty(&a["text"]), if bx.is_empty() { St::BannerText } else { St::Small }));
    }
    (!bx.is_empty()).then(|| el(K::Banner(tone), bx))
}

/// A fold (`<details>`): its summary, then what shows when it's open.
fn fold(f: Fold, open: bool, summary: Node, body: Vec<Node>) -> Node {
    let mut kids = vec![summary];
    kids.extend(body);
    el(K::Fold(f, open), kids)
}

/// `termActions(t)`: Focus in Midna, and Detach and Close terminal behind the ⋯ menu. No Mark done /
/// Mark failed: Claude finishes a task (`tb submit`, `tb fail`).
fn term_actions(c: &Ctx, t: &Value) -> Vec<Node> {
    let r = rf(t, "T");
    let grp = format!("manage:{r}");
    let live = session_live(t);
    let mut row = Vec::new();
    if live && s(t, "status") == "working" {
        row.push(c.btn("Focus in Midna", Act::new("focus", "", &r, &grp), Look::SoftSmall, false, None));
    }
    if opt_s(t, "session_id").is_some() || live {
        row.push(c.btn("", Act::new("term-menu", "", &r, ""), Look::Icon(Icon::More), false, Some("Terminal actions")));
        if c.ui.term_menu {
            let mut items = vec![menu_item(c, "Detach", "Take it off this terminal and queue it again", Act::new("detach", "", &r, &grp), false)];
            if live {
                let arg = if s(&t["session"], "status") == "idle" { "" } else { "force" };
                items.push(menu_item(c, "Close terminal", "Stops the command that is running", Act::new("close-term", arg, &r, &grp), true));
            }
            row.push(el(K::TermMenu, items));
        }
    }
    let mut out = Vec::new();
    if !row.is_empty() {
        out.push(el(K::TlActs, row));
    }
    out.extend(c.note(&grp));
    out
}

fn menu_item(c: &Ctx, label: &str, sub: &str, act: Act, danger: bool) -> Node {
    el(K::MenuItem { danger }, vec![c.btn(label, act, Look::Menu, false, None), txt(sub, St::Small)])
}

fn lrow(key: &str, mut body: Vec<Node>) -> Node {
    body.insert(0, txt(key, St::LrowKey));
    el(K::Lrow, body)
}

/// `blockedRow(t)`.
fn blocked_row(c: &Ctx, t: &Value) -> Option<Node> {
    let bs = arr(t, "blocked_by");
    if bs.is_empty() {
        return None;
    }
    let mut body = Vec::new();
    for x in bs {
        let br = or_empty(&x["ref"]);
        body.push(el(K::Row, vec![c.btn(&br, Act::new("open-task", "", &br, ""), Look::Ref, false, None), status_pill(x)]));
        body.push(txt(or_empty(&x["title"]), St::Plain));
    }
    Some(lrow("Blocked by", body))
}

/// The note for testers the board posts on the ticket at merge (`[jira] merged_note`), or that it's waiting for one.
/// It grows, so it stacks like Attached: the label on top, the note full width under it.
fn merged_note_row(c: &Ctx, t: &Value) -> Option<Node> {
    let n = obj(t, "merged_note")?;
    let mut body = vec![txt("Note for testers", St::LrowKey)];
    if b(n, "waiting") {
        let state = if opt_s(n, "text").is_some() { "Posting it, then moving the ticket" } else { "Merged without one; the ticket waits" };
        body.push(el(K::Line, vec![pill("Waiting", Tone::Needs), txt(state, St::Small)]));
    }
    if let Some(text) = opt_s(n, "text") {
        let (preview, more) = note_preview(text);
        body.push(preview);
        if more {
            body.push(c.btn("Show all", Act::new("note-all", "", s(t, "ref"), ""), Look::Link, false, None));
        }
    }
    Some(el(K::Stack, body))
}

/// The lines of a note the panel shows; Show all opens the whole note in a dialog.
const NOTE_PREVIEW_LINES: usize = 3;

/// `note_body` of the note's first lines, and whether more of it is behind Show all.
pub fn note_preview(text: &str) -> (Node, bool) {
    let text = join_labels(text);
    let mut lines = text.split('\n');
    let mut head = Vec::new();
    let mut shown = 0;
    for line in lines.by_ref() {
        if shown == NOTE_PREVIEW_LINES {
            if line.trim().is_empty() {
                continue;
            }
            return (note_body(&head.join("\n")), true);
        }
        shown += usize::from(!line.trim().is_empty());
        head.push(line);
    }
    (note_body(&text), false)
}

/// A line that's only a bold label (`**What to test:**`) takes the line under it as its value, so every
/// label reads `**Label:** value` on one line; a list under a label stays a list.
fn join_labels(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_end();
        let label = line.trim();
        let is_label = label.len() > 4 && label.starts_with("**") && (label.ends_with(":**") || label.ends_with("**:")) && !label[2..label.len() - 3].contains("**");
        let next = lines.get(i + 1).map(|l| l.trim()).unwrap_or("");
        let is_item = next.starts_with("- ") || next.starts_with("* ") || next.starts_with("• ");
        if is_label && !next.is_empty() && !is_item {
            out.push(format!("{label} {next}"));
            i += 2;
        } else {
            out.push(line.to_string());
            i += 1;
        }
    }
    out.join("\n")
}

/// One PR step: name, state, sub (`checksStep` / `reviewStep` / `mergeStep`).
pub fn pr_steps(p: &Value) -> [(&'static str, StepSt, Option<String>); 3] {
    let stage = &p["stage"];
    let phase = if stage.is_object() { s(stage, "phase") } else { "" };
    let c = js_lower(&p["checks"]);
    let checks = match c.as_str() {
        "none" => (StepSt::Done, Some("None".to_string())),
        "pass" => (StepSt::Done, Some("Passed".into())),
        "fail" if phase == "fix" => (
            StepSt::Fail,
            // Being fixed only while a terminal is on it; `owner`: only the owner can fix it; `idle`: nobody is.
            Some(if obj(stage, "owner").is_some() {
                "Needs you"
            } else if obj(stage, "stopped").is_some() {
                "Fix stopped"
            } else if !stage["session"].is_null() && stage["session"] != "" {
                "Being fixed"
            } else if b(stage, "idle") {
                "Failed"
            } else {
                "Fix starting"
            }
            .into()),
        ),
        "fail" => (StepSt::Fail, Some("Failed".into())),
        "pending" => (StepSt::Now, Some("Running".into())),
        _ => (if pr_open(p) { StepSt::Wait } else { StepSt::Todo }, Some("Waiting".into())),
    };
    let rv = js_lower(&p["review"]);
    let review = if phase == "rereview" {
        (StepSt::Wait, Some("Awaiting re-review".to_string()))
    } else if rv == "changes" {
        (StepSt::Ask, Some("Changes asked".to_string()))
    } else if phase == "comments" {
        (StepSt::Ask, Some("New comments".into()))
    } else if rv == "approved" {
        (StepSt::Done, Some("Approved".into()))
    } else if let (Some(have), Some(need)) = (p["approvals"]["have"].as_i64().filter(|n| *n > 0), p["approvals"]["need"].as_i64().filter(|n| *n > 0)) {
        // Short of the approvals it needs ("approved" says it has them): "1 of 2".
        (StepSt::Wait, Some(format!("{} of {need}", have.min(need))))
    } else if rv == "pending" {
        (StepSt::Wait, Some("Waiting".into()))
    } else {
        (StepSt::Todo, Some("Not asked".into()))
    };
    let state = opt_s(p, "state").unwrap_or("OPEN").to_uppercase();
    let merge = if state == "MERGED" {
        (StepSt::Done, Some("Merged".to_string()))
    } else if state != "OPEN" {
        (StepSt::Fail, Some("Declined".into()))
    } else if phase == "merge" {
        (StepSt::Now, Some("Merging".into()))
    } else {
        (StepSt::Todo, None)
    };
    [("Checks", checks.0, checks.1), ("Review", review.0, review.1), ("Merge", merge.0, merge.1)]
}

/// One step of the PR bar: its name, state, the state's words and where they link.
pub struct PrStep {
    pub name: String,
    pub st: StepSt,
    pub sub: Option<String>,
    pub link: Option<Go>,
}

fn step(name: impl Into<String>, st: StepSt, sub: Option<&str>, link: Option<String>) -> PrStep {
    PrStep { name: name.into(), st, sub: sub.map(str::to_string), link: link.map(Go::Url) }
}

/// The author-side review step (`wd`: its `bar` name, like WD, and its latest round's headline, which
/// opens its findings); "Not reviewed by WD yet" before its first round (`pending`).
pub fn wd_step(wd: &Value) -> PrStep {
    if b(wd, "pending") {
        return step(s(wd, "bar"), StepSt::Todo, Some(&format!("Not reviewed by {} yet", s(wd, "bar"))), None);
    }
    let mut out = wd_round(wd);
    if out.sub.is_some() {
        out.link = Some(Go::Findings);
    }
    out
}

fn wd_round(wd: &Value) -> PrStep {
    let headline = opt_s(wd, "headline").unwrap_or("");
    let open = wd["open"].as_i64().unwrap_or(0);
    let (st, sub) = if s(wd, "verdict") == "skip" {
        (StepSt::Done, headline.to_string())
    } else if b(wd, "stale") {
        (StepSt::Wait, "Moved since".to_string())
    } else if open > 0 {
        (StepSt::Ask, headline.to_string())
    } else if b(wd, "passed") {
        (StepSt::Done, headline.to_string())
    } else {
        (StepSt::Fail, headline.to_string())
    };
    step(s(wd, "bar"), st, Some(&sub).filter(|x| !x.is_empty()).map(|x| x.as_str()), None)
}

/// The Lens step (`pr.bar.lens`): lens's review of a new PR, whose findings are on the PR itself.
pub fn lens_step(l: &Value) -> PrStep {
    match s(l, "state") {
        "queued" => step("Lens", StepSt::Wait, Some("Queued"), None),
        "running" => step("Lens", StepSt::Now, Some("Reviewing"), None),
        "done" => step("Lens", StepSt::Done, Some("Posted"), None),
        // The PR merged or closed first.
        "skipped" => step("Lens", StepSt::Done, Some("Skipped"), None),
        "stopped" => step("Lens", StepSt::Done, Some("Stopped"), None),
        _ => step("Lens", StepSt::Fail, Some("Failed"), None),
    }
}

/// The steps row's nodes: each step's name, and its state's words (a link when it has one).
fn step_nodes(list: Vec<PrStep>) -> Node {
    let mut steps = Vec::new();
    for step in list {
        let mut kids = vec![txt(step.name, St::StepName)];
        match (step.sub, step.link) {
            (Some(x), Some(go)) => kids.push(Node::Link { s: x, go, tip: None, look: LinkLook::Small }),
            (Some(x), None) => kids.push(txt(x, St::StepSub)),
            _ => {}
        }
        steps.push(el(K::Step(step.st), kids));
    }
    el(K::Steps, steps)
}

/// The PR bar's steps when the board sends `pr.bar`: the author-side review step (its `bar` name, like
/// WD), Checks, You, Review and Merge, with the states the old board showed.
pub fn pr_steps_full(p: &Value, bar: &Value) -> Vec<PrStep> {
    let [checks, review, merge] = pr_steps(p);
    let stage = &p["stage"];
    let phase = if stage.is_object() { s(stage, "phase") } else { "" };
    let mut out = Vec::new();
    if let Some(wd) = obj(bar, "wd") {
        out.push(wd_step(wd));
    }
    let build = opt_s(bar, "build_url").filter(|u| is_web(u)).map(str::to_string);
    out.push(match s(bar, "checks") {
        "not_ours" => step("Checks", StepSt::Done, Some("Not this PR's"), None),
        "skipped" => step("Checks", StepSt::Done, Some("Skipped"), None),
        "not_needed" => step("Checks", StepSt::Done, Some("Not needed"), None),
        // Its PR into the base's branch has no builds; they may start once it moves to the main branch.
        "none_yet" => step("Checks", StepSt::Todo, Some("None yet"), None),
        _ => step(checks.0, checks.1, checks.2.as_deref(), if checks.1 == StepSt::Todo { None } else { build }),
    });
    if let Some(l) = obj(bar, "lens") {
        out.push(lens_step(l));
    }
    out.push(match s(bar, "you") {
        "waiting" => step("You", StepSt::Ask, Some("Waiting on you"), None),
        "reviewed" => step("You", StepSt::Done, Some("Reviewed"), None),
        "skipped" => step("You", StepSt::Done, Some("Skipped"), None),
        "off" => step("You", StepSt::Done, Some("Off"), None),
        // Not green yet: the owner may mark it reviewed now; it asks for them once checks pass.
        "early" => step("You", StepSt::Todo, Some("After checks"), None),
        _ => step("You", StepSt::Todo, None, None),
    });
    let url = opt_s(p, "url").filter(|u| is_web(u)).map(str::to_string);
    let approvals = bar["approvals"].as_i64().unwrap_or(0);
    let reviewers = bar["reviewers"].as_i64().unwrap_or(0);
    let new_comments = bar["new_comments"].as_i64().unwrap_or(0);
    let rv = js_lower(&p["review"]);
    let need = bar["need"].as_i64().filter(|n| *n > 0);
    out.push(if s(bar, "review") == "off" {
        step("Review", StepSt::Done, Some("Off"), None)
    } else if phase == "comments" || (new_comments > 0 && rv != "changes" && phase != "rereview") {
        // "2 new comments", linking to the first unread thread.
        let words = if new_comments > 0 { fmt::plural(new_comments, "new comment", "new comments") } else { "New comments".to_string() };
        let go = opt_s(bar, "comments_url").filter(|u| is_web(u)).map(str::to_string).or(url);
        step("Review", StepSt::Ask, Some(&words), go)
    } else if need.is_none() && rv == "approved" && phase != "rereview" {
        // No count set: the host decides, and it has approved, however few of the reviewers have.
        step("Review", StepSt::Done, Some("Approved"), url)
    } else if (reviewers > 0 || approvals > 0) && matches!(review.1, StepSt::Done | StepSt::Wait | StepSt::Todo) && phase != "rereview" {
        // "1 of 2": out of the approvals it needs, else (the host decides) the reviewers on it; more than
        // needed still reads "2 of 2".
        let of = need.unwrap_or(reviewers);
        let st = if approvals >= of { StepSt::Done } else if approvals > 0 || rv == "pending" { StepSt::Wait } else { review.1 };
        step("Review", st, Some(&format!("{} of {of}", approvals.min(of))), url)
    } else if s(bar, "review") == "setup" && review.1 == StepSt::Todo {
        step("Review", StepSt::Ask, Some("Needs setup"), None)
    } else {
        // "Not asked" has nowhere to go.
        step(review.0, review.1, review.2.as_deref(), if review.1 == StepSt::Todo { None } else { url })
    });
    // Merge steps (`before = "merge"`), judged on the PR's head; a skip shows its reason.
    for m in arr(bar, "merge_steps") {
        out.push(match s(m, "state") {
            "passed" => step(s(m, "name"), StepSt::Done, Some("Passed"), None),
            "skipped" => match opt_s(m, "why") {
                Some(why) => step(s(m, "name"), StepSt::Done, Some(&format!("Skipped: {why}")), None),
                None => step(s(m, "name"), StepSt::Done, Some("Skipped"), None),
            },
            _ if pr_open(p) && phase == "steps" => step(s(m, "name"), StepSt::Ask, Some("To do"), None),
            _ => step(s(m, "name"), StepSt::Todo, None, None),
        });
    }
    out.push(match phase {
        "waits" if pr_open(p) => match obj(bar, "waits_on_base") {
            // "Waits on T3", to the base's running build or its PR.
            Some(wb) => step("Merge", StepSt::Wait, Some(&format!("Waits on {}", s(wb, "ref"))), opt_s(wb, "url").filter(|u| is_web(u)).map(str::to_string)),
            None => step("Merge", StepSt::Wait, Some("Waits on base"), None),
        },
        "steps" if pr_open(p) => step("Merge", StepSt::Wait, Some("Waits on steps"), None),
        // A head-merge child's PR has to merge into this one first.
        "stack" if pr_open(p) => step("Merge", StepSt::Wait, Some("Waits on stack"), None),
        _ => step(merge.0, merge.1, merge.2.as_deref(), None),
    });
    out
}

/// "Stacks on T3 · PR #12 · Head merge": the task it builds on opens in the panel, its PR on the host.
fn stacks_line(c: &Ctx, so: &Value) -> Node {
    let r = s(so, "ref");
    let merged = b(so, "merged");
    let mut kids = vec![txt(if merged { "Stacked on" } else { "Stacks on" }, St::Small), c.btn(r, Act::new("open-task", "", r, ""), Look::Ref, false, Some(&format!("Open {r}")))];
    if let Some(n) = so.get("num").filter(|n| !n.is_null()) {
        let label = format!("PR #{}{}", or_empty(n), if merged { " · merged" } else { "" });
        kids.push(match opt_s(so, "url").filter(|u| is_web(u)) {
            Some(u) => Node::Link { s: label, go: Go::Url(u.to_string()), tip: Some("Open its pull request".into()), look: LinkLook::Small },
            None => txt(label, St::Small),
        });
    } else if let Some(br) = opt_s(so, "branch") {
        kids.push(txt(format!("branch {br}"), St::Small));
    }
    match opt_s(so, "merge") {
        Some("head") => kids.push(txt_tip("Head merge", St::Small, format!("Its PR merges into {r}'s branch; {r}'s PR takes it to the main branch"))),
        Some("tail") => kids.push(txt_tip("Tail merge", St::Small, format!("Its PR moves to the main branch once {r}'s merges"))),
        _ => {}
    }
    el(K::Row, kids)
}

/// "2 new comments ›" by the reviewer rows while the Review step shows the review instead (changes asked,
/// or a re-review): new comments show in every state, and matter most then.
fn comments_line(p: &Value, bar: &Value) -> Option<Node> {
    let n = bar["new_comments"].as_i64().filter(|n| *n > 0)?;
    let phase = obj(p, "stage").map(|st| s(st, "phase")).unwrap_or("");
    if phase == "comments" || (js_lower(&p["review"]) != "changes" && phase != "rereview") {
        return None;
    }
    let words = format!("{} ›", fmt::plural(n, "new comment", "new comments"));
    let go = opt_s(bar, "comments_url").or_else(|| opt_s(p, "url")).filter(|u| is_web(u)).map(|u| Go::Url(u.to_string()));
    Some(el(K::Line, vec![match go {
        Some(go) => Node::Link { s: words, go, tip: None, look: LinkLook::Small },
        None => txt(words, St::Small),
    }]))
}

/// One row per reviewer (`pr.bar.reviewer_rows`: name, state, link), when the board sends them. A
/// reviewer the PR still waits on says for how long.
fn reviewer_rows(c: &Ctx, bar: &Value) -> Vec<Node> {
    arr(bar, "reviewer_rows")
        .iter()
        .filter_map(|r| {
            let name = opt_s(r, "name")?;
            let user = opt_s(r, "user").unwrap_or(name);
            let open = c.ui.rev_pop.as_deref() == Some(user);
            let mut who = vec![c.btn(name, Act::new("rev-pop", user, c.r, ""), Look::Plain, false, None)];
            if open {
                who.push(reviewer_pop(c, name, user));
            }
            let mut kids = vec![el(K::RevName { open }, who)];
            if let Some(st) = opt_s(r, "state") {
                let (label, tone) = match st {
                    "approved" => ("Approved", Tone::Done),
                    "changes" => ("Changes asked", Tone::Needs),
                    "rereview" => ("Re-review waiting", Tone::Review),
                    "waiting" => ("Waiting", Tone::Queued),
                    "commented" => ("Commented", Tone::Neutral),
                    other => (other, Tone::Neutral),
                };
                // How long the PR has waited on them, by when they were asked.
                let waited = opt_s(r, "asked_at").and_then(fmt::minutes_since).filter(|m| *m >= 1 && matches!(st, "waiting" | "rereview"));
                kids.push(pill(waited.map_or(label.to_string(), |m| format!("{label} {}", fmt::span(m))), tone));
            }
            if let Some(n) = r["swaps"].as_i64().filter(|n| *n > 0) {
                kids.push(txt(format!("Swapped {}", if n == 1 { "once".to_string() } else { format!("{n} times") }), St::Small));
            }
            if let Some(at) = opt_s(r, "asked_at").map(crate::fmt::hhmm).filter(|a| !a.is_empty()) {
                kids.push(txt(format!("Asked {at}"), St::Small));
            }
            if let Some(u) = opt_s(r, "url").filter(|u| is_web(u)) {
                kids.push(Node::Link { s: "Open ›".into(), go: Go::Url(u.to_string()), tip: None, look: LinkLook::Small });
            }
            Some(el(K::Row, kids))
        })
        .collect()
}

/// An ask this close to being swapped (work minutes) shows its wait and note in orange.
const SWAP_SOON_MINS: f64 = 30.;

/// The popover from a reviewer's name: every PR they have an open ask on, oldest first, then
/// the reviews they gave this week. Read-only: its links open the PR, the task or the goal.
pub fn reviewer_pop(c: &Ctx, name: &str, user: &str) -> Node {
    let got = c.ui.rev_asks.get(user);
    let w = got.and_then(|g| g.as_ref().ok());
    let name = w.and_then(|w| opt_s(w, "name")).unwrap_or(name);
    let mut top = vec![txt(name, St::Plain)];
    if !user.eq_ignore_ascii_case(name) {
        top.push(txt(user, St::Mono));
    }
    let mut head = vec![el(K::Line, top)];
    let mut kids = Vec::new();
    match got {
        None => kids.push(txt("Loading…", St::Empty)),
        Some(Err(e)) => kids.push(txt(format!("Couldn’t load their reviews: {e}"), St::WarnHelp)),
        Some(Ok(w)) => {
            let open = arr(w, "open");
            let mut line = vec![format!("{} open", open.len())];
            if let Some(m) = w["median_work_mins"].as_f64() {
                line.push(format!("reviews in about {}", about(m)));
            }
            if let Some(at) = opt_s(w, "last_review_at").map(ask_when).filter(|a| !a.is_empty()) {
                line.push(format!("last review {at}"));
            }
            head.push(txt(line.join(" · "), St::Small));
            if open.is_empty() {
                kids.push(txt("No open reviews", St::Empty));
            }
            kids.extend(open.iter().map(|a| ask_row(c, a)));
            let done = arr(w, "answered_this_week");
            if !done.is_empty() {
                let mut fold = vec![txt(format!("Reviewed this week · {}", done.len()), St::Plain)];
                fold.extend(done.iter().map(answered_row));
                kids.push(el(K::Fold(Fold::Reviewed, c.fold_open(Fold::Reviewed, true)), fold));
            }
        }
    }
    let mut out = vec![el(K::RevHead, head)];
    out.extend(kids);
    el(K::RevPop, out)
}

/// One open ask: repo#num (opens the PR), its title and how long it has waited in work time;
/// then the task and goal (which open them), when it was asked (or the changes addressed) and its notes.
fn ask_row(c: &Ctx, a: &Value) -> Node {
    let pr = &a["pr"];
    let here = opt_s(a, "task") == Some(c.r);
    let swap_in = a["swap_in_mins"].as_f64();
    let late = swap_in.is_some_and(|m| m <= SWAP_SOON_MINS);
    let title = opt_s(pr, "title").unwrap_or("");
    let mut top = Vec::new();
    if let Some(n) = pr["num"].as_i64() {
        let label = format!("{}#{n}", s(pr, "repo"));
        top.push(match opt_s(pr, "url").filter(|u| is_web(u)) {
            Some(u) => Node::Link { s: label, go: Go::Url(u.to_string()), tip: Some(title.to_string()).filter(|t| !t.is_empty()), look: LinkLook::Jkey },
            None => txt(label, St::Jkey),
        });
    }
    top.push(txt(title, St::Plain));
    top.push(txt(wait_span(a["wait_work_mins"].as_f64().unwrap_or(0.)), St::Wait { late }));
    let mut sub = Vec::new();
    if let Some(t) = opt_s(a, "task") {
        sub.push(c.btn(t, Act::new("open-task", "", t, ""), Look::Ref, false, Some(&format!("Open {t}"))));
    }
    if let Some(g) = opt_s(a, "goal") {
        sub.push(Node::Link { s: g.to_string(), go: Go::Goal(g.to_string()), tip: Some(format!("Open {g}")), look: LinkLook::Crumb });
    }
    let rereview = b(a, "rereview");
    if let Some(at) = opt_s(a, "asked_at").map(ask_when).filter(|a| !a.is_empty()) {
        if !sub.is_empty() {
            sub.push(txt("·", St::Small));
        }
        sub.push(txt(format!("{} {at}", if rereview { "Changes addressed" } else { "Asked" }), St::Small));
    }
    // Its notes, at the line's end.
    let mut notes = Vec::new();
    if late {
        let m = swap_in.unwrap_or(0.).ceil() as i64;
        notes.push(txt(if m < 1 { "Swaps now".to_string() } else { format!("Swaps in {}", fmt::span(m)) }, St::WarnHelp));
    }
    if rereview {
        notes.push(txt("Re-review", St::Small));
    }
    if here {
        notes.push(txt("This task", St::Small));
    }
    if !notes.is_empty() {
        sub.push(el(K::Row, notes));
    }
    el(K::RevAsk { here }, vec![el(K::RevLine, top), el(K::RevLine, sub)])
}

/// A review given this week: the PR, the answer, how long it took in work time, and when.
fn answered_row(a: &Value) -> Node {
    let pr = &a["pr"];
    let mut kids = Vec::new();
    if let Some(n) = pr["num"].as_i64() {
        let label = format!("{}#{n}", s(pr, "repo"));
        kids.push(match opt_s(pr, "url").filter(|u| is_web(u)) {
            Some(u) => Node::Link { s: label, go: Go::Url(u.to_string()), tip: opt_s(pr, "title").map(str::to_string), look: LinkLook::Jkey },
            None => txt(label, St::Jkey),
        });
    }
    let answer = match s(a, "answer") {
        "approved" => "Approved",
        "changes" => "Asked for changes",
        "commented" => "Commented",
        _ => "Reviewed",
    };
    let mut line = vec![answer.to_string()];
    if let Some(m) = a["work_mins"].as_f64() {
        line.push(format!("took {}", wait_span(m)));
    }
    if let Some(at) = opt_s(a, "answered_at").map(ask_when).filter(|a| !a.is_empty()) {
        line.push(at);
    }
    kids.push(txt(line.join(" · "), St::Small));
    el(K::Row, kids)
}

/// When an ask was made or answered, in 12-hour time: "9:40 AM" today, "Mon 2:10 PM" in the
/// past week, "Oct 7, 2:10 PM" before that.
pub fn ask_when(iso: &str) -> String {
    let Some(t) = fmt::parse(iso) else { return String::new() };
    let l = fmt::local(t);
    let days = (fmt::local(fmt::now()).date_naive() - l.date_naive()).num_days();
    match days {
        0 => l.format("%-I:%M %p").to_string(),
        1..=6 => l.format("%a %-I:%M %p").to_string(),
        _ => l.format("%b %-d, %-I:%M %p").to_string(),
    }
}

/// A work-time wait as the popover shows it: "32m", "1h 04m", "26h 10m".
pub fn wait_span(mins: f64) -> String {
    let m = mins.max(0.).round() as i64;
    if m < 60 { format!("{m}m") } else { format!("{}h {:02}m", m / 60, m % 60) }
}

/// A typical review time, rounded for "reviews in about …": "25m", "2h".
fn about(mins: f64) -> String {
    let m = mins.max(0.);
    if m < 55. { format!("{}m", ((m / 5.).round() as i64 * 5).max(5)) } else { format!("{}h", (m / 60.).round() as i64) }
}

/// A done task's reason, its task refs as buttons that open them and its ticket keys as links.
pub fn linkify(c: &Ctx, text: &str) -> Vec<Node> {
    let site = c.state["jira"]["site"].as_str().map(str::trim).filter(|x| !x.is_empty());
    let mut out = Vec::new();
    let mut buf = String::new();
    let flush = |buf: &mut String, out: &mut Vec<Node>| {
        let t = buf.trim();
        if !t.is_empty() {
            out.push(txt(t, St::Plain));
        }
        buf.clear();
    };
    let mut word = String::new();
    let mut words: Vec<(bool, String)> = Vec::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            word.push(ch);
        } else {
            if !word.is_empty() {
                words.push((true, std::mem::take(&mut word)));
            }
            words.push((false, ch.to_string()));
        }
    }
    if !word.is_empty() {
        words.push((true, word));
    }
    for (is_word, w) in words {
        if is_word && is_task_ref(&w) {
            flush(&mut buf, &mut out);
            out.push(c.btn(&w, Act::new("open-task", "", &w, ""), Look::Ref, false, Some(&format!("Open {w}"))));
        } else if is_word && is_ticket(&w) {
            flush(&mut buf, &mut out);
            out.push(match site {
                Some(site) => Node::Link { s: w.clone(), go: Go::Url(format!("https://{site}/browse/{w}")), tip: Some(format!("Open {w} in Jira")), look: LinkLook::Jkey },
                None => txt(w, St::Jkey),
            });
        } else {
            buf.push_str(&w);
        }
    }
    flush(&mut buf, &mut out);
    out
}

fn is_task_ref(w: &str) -> bool {
    w.len() > 1 && (w.starts_with('T') || w.starts_with('H')) && w[1..].chars().all(|c| c.is_ascii_digit())
}

/// `ABC-12`: a Jira key.
fn is_ticket(w: &str) -> bool {
    let Some((key, n)) = w.split_once('-') else { return false };
    key.len() >= 2
        && key.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && !n.is_empty()
        && n.chars().all(|c| c.is_ascii_digit())
}

/// One finding: its state, severity, title and where it is.
fn finding_item(f: &Value) -> Node {
    let state = opt_s(f, "state").unwrap_or("open");
    let tone = match state {
        "fixed" | "resolved" => Tone::Done,
        "answered" | "replied" => Tone::Review,
        "dismissed" | "wontfix" | "false-positive" | "ignored" => Tone::Closed,
        _ => Tone::Needs,
    };
    let mut kids = vec![txt(s(f, "id"), St::Mono), pill(state, tone)];
    if let Some(sev) = opt_s(f, "severity") {
        kids.push(txt(sev, St::Small));
    }
    kids.push(txt(s(f, "title"), St::Plain));
    if let Some(file) = opt_s(f, "file") {
        kids.push(txt(match f["line"].as_i64() { Some(l) => format!("{file}:{l}"), None => file.to_string() }, St::Small));
    }
    if let Some(n) = opt_s(f, "note") {
        kids.push(txt(n, St::Small));
    }
    if let Some(u) = opt_s(f, "url").filter(|u| is_web(u)) {
        kids.push(Node::Link { s: "Open ›".into(), go: Go::Url(u.to_string()), tip: None, look: LinkLook::Small });
    }
    el(K::Line, kids)
}

/// `String(x || '').toLowerCase()`.
fn js_lower(v: &Value) -> String {
    match v {
        Value::Null | Value::Bool(false) => String::new(),
        Value::String(x) => x.to_lowercase(),
        other => js_string(other).to_lowercase(),
    }
}

/// The review step a `step_results` node already shows (its `bar` name, like WD), left out of the PR's steps.
fn shown_bar(t: &Value) -> Option<&str> {
    arr(t, "step_results").iter().find_map(|r| opt_s(r, "bar"))
}

/// `prNodes(t)`: Pull request, Checks and review, Merged.
fn pr_nodes(c: &Ctx, t: &Value) -> Vec<Node> {
    let Some(p) = pr_of(t) else {
        return no_pr_nodes(c, t);
    };
    let key = obj(t, "jira").and_then(|j| opt_s(j, "key"));
    let raw = opt_s(p, "title");
    let title: Option<String> = match (key, raw) {
        (Some(k), Some(x)) if x.starts_with(k) => Some(x[k.len()..].trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ':' | '–' | '-')).to_string()),
        (_, x) => x.map(str::to_string),
    };
    let shown = title.clone().filter(|x| !x.is_empty()).or_else(|| opt_s(p, "repo").map(str::to_string)).unwrap_or_default();
    let label = format!("#{} {shown}", or_empty(&p["num"]));
    let head = match opt_s(p, "url") {
        // safeUrl('') → '#': a url that isn't http(s) still renders as a link that goes nowhere.
        Some(url) => Node::Link { s: label, go: Go::Url(if is_web(url) { url.to_string() } else { String::new() }), tip: Some("Open the pull request".into()), look: LinkLook::Plain },
        None => txt(label, St::Strong),
    };
    let repo = if title.as_deref().is_some_and(|x| !x.is_empty()) { or_empty(&p["repo"]) } else { String::new() };
    let bar = obj(p, "bar");
    let mut list: Vec<PrStep> = match bar {
        Some(bar) => pr_steps_full(p, bar),
        None => pr_steps(p).into_iter().map(|(name, st, sub)| PrStep { name: name.to_string(), st, sub, link: None }).collect(),
    };
    let merge = list.pop().expect("the PR's steps end with Merge");
    if let Some(wd) = shown_bar(t) {
        list.retain(|x| x.name != wd);
    }
    let st = if list.iter().any(|x| x.st == StepSt::Fail) {
        TlSt::Fail
    } else if list.iter().any(|x| x.st == StepSt::Ask) {
        TlSt::Warn
    } else if list.iter().all(|x| x.st == StepSt::Done) {
        TlSt::Done
    } else if list.iter().all(|x| x.st == StepSt::Todo) {
        TlSt::Todo
    } else {
        TlSt::Now
    };

    let mut body = vec![step_nodes(list)];
    if let Some(n) = not_ours_box(p) {
        body.push(n);
    }
    if let Some(line) = bar.and_then(|bar| obj(bar, "stacks_on")).map(|so| stacks_line(c, so)) {
        body.push(line);
    }
    // Phase `waits`: what the base is at ("T3's checks running, 2 of 5 passed").
    if let Some(line) = bar.and_then(|bar| obj(bar, "waits_on_base")).and_then(|wb| opt_s(wb, "line")).filter(|_| pr_open(p)) {
        body.push(el(K::Line, vec![txt(line, St::Small)]));
    }
    // Phase `stack`: which head-merge PR has to merge into this one first.
    if let Some(why) = bar.and_then(|bar| opt_s(bar, "waits_on_stack")).filter(|_| pr_open(p)) {
        body.push(el(K::Line, vec![txt(why, St::Small)]));
    }
    body.extend(bar.map(|bar| reviewer_rows(c, bar)).unwrap_or_default());
    body.extend(bar.and_then(|bar| comments_line(p, bar)));
    // `prBar`
    if let Some(stage) = obj(p, "stage").filter(|st| pr_open(p) && matches!(s(st, "phase"), "fix" | "rebase" | "comments" | "merge" | "ask")) {
        let stopped = obj(stage, "stopped").is_some();
        // The banner up top says it stopped; the warn dot marks the stage.
        let mut kids = vec![Node::Dot(if stopped { Dot::Warn } else { Dot::Accent }), txt(or_empty(&stage["label"]), St::Plain)];
        let tip = match opt_s(stage, "session") {
            Some(sid) => {
                kids.push(Node::Link { s: "Terminal ›".into(), go: Go::Session(sid.to_string()), tip: None, look: LinkLook::Plain });
                if stopped { "Its terminal stopped before it was finished" } else { "Open the terminal doing this" }
            }
            None => "Waiting for its terminal to open",
        };
        body.push(Node::El { k: K::PrBar { warn: stopped && opt_s(stage, "session").is_some() }, tip: Some(tip.into()), kids });
    }
    // The owner's own look at the PR (the You step): "Waiting on your review · Mark reviewed ›" once it's
    // green, "Not reviewed yet · Mark reviewed ›" before.
    if let Some(you) = bar.map(|bar| s(bar, "you")).filter(|y| matches!(*y, "waiting" | "early")) {
        let r = rf(t, "T");
        let grp = format!("pr:{r}");
        let mark = c.btn("Mark reviewed ›", Act::new("pr-reviewed", "", &r, &grp), Look::Link, false, Some("Tell the board you've looked at this PR"));
        let (dot, line) = if you == "waiting" { (Dot::Accent, "Waiting on your review") } else { (Dot::Faint, "Not reviewed yet") };
        body.push(Node::El { k: K::PrBar { warn: false }, tip: None, kids: vec![Node::Dot(dot), txt(line, St::Plain), mark] });
        body.extend(c.note(&grp));
    } else if bar.is_none() && obj(p, "stage").is_some_and(|st| b(st, "awaiting_you")) {
        // Native: the owner's one-click "I reviewed it" while a green PR waits for them.
        let r = rf(t, "T");
        let grp = format!("pr:{r}");
        body.push(el(K::Row, vec![c.btn("I reviewed it", Act::new("pr-reviewed", "", &r, &grp), Look::SoftSmall, false, Some("Tell the board you've looked at this PR"))]));
        body.extend(c.note(&grp));
    }

    let merge_st = match merge.st {
        StepSt::Todo => TlSt::Todo,
        StepSt::Done => TlSt::Done,
        StepSt::Fail => TlSt::Fail,
        _ => TlSt::Now,
    };
    let merge_sub = if matches!(merge_st, TlSt::Done | TlSt::Fail) { String::new() } else { merge.sub.unwrap_or_default() };
    // Named for what has happened only once it has: "Pending merge" until the PR lands.
    let merge_name = match merge_st {
        TlSt::Done => "Merged",
        TlSt::Fail => "Closed without merging",
        _ => "Pending merge",
    };
    // "Updated 4m ago": when the board last read the open PR, so a stale read shows.
    let updated = opt_s(p, "checked_at").filter(|_| pr_open(p)).map(|at| format!("Updated {}", fmt::ago(at))).unwrap_or_default();
    vec![
        tl_node(TlSt::Done, tl_head(tl_name("Pull request"), "", ""), vec![el(K::PrHead, vec![head, txt(repo, St::Small)])], false),
        tl_node(st, tl_head(tl_name("Checks and review"), "", &updated), body, false),
        tl_node(merge_st, tl_head(tl_name(merge_name), &merge_sub, ""), merged_note_row(c, t).into_iter().collect(), false),
    ]
}

/// What's left before it merges, on a task with no PR yet; nothing on one that doesn't end in a PR.
fn no_pr_nodes(c: &Ctx, t: &Value) -> Vec<Node> {
    if opt_s(t, "no_pr").is_some() || t["ships_pr"] == false {
        return Vec::new();
    }
    let pr = obj(t, "stack_on").map(|so| stacks_line(c, so)).into_iter().collect();
    // The whole way the PR will take, as it shows once the PR is open: the review step (WD) runs before
    // the PR opens ("Not reviewed by WD yet" before its first round; a round it ran is its own node),
    // then the steps still ahead.
    let mut steps: Vec<PrStep> = obj(t, "wd").filter(|_| shown_bar(t).is_none()).map(wd_step).into_iter().collect();
    steps.extend(["Checks", "You", "Review"].map(|name| step(name, StepSt::Todo, None, None)));
    vec![
        tl_node(TlSt::Todo, tl_head(tl_name("Pull request"), "", ""), pr, false),
        tl_node(TlSt::Todo, tl_head(tl_name("Checks and review"), "", ""), vec![step_nodes(steps)], false),
        tl_node(TlSt::Todo, tl_head(tl_name("Pending merge"), "", ""), vec![], false),
    ]
}

/// The "Failed, but not because of this PR" box: this push's checks cleared with `tb pr not-ours`,
/// with the reason, the proof links (named by the board) and when it was checked.
fn not_ours_box(p: &Value) -> Option<Node> {
    let n = obj(p, "stage").and_then(|st| obj(st, "not_ours"))?;
    if !pr_open(p) {
        return None;
    }
    let mut kids = vec![txt("Failed, but not because of this PR", St::BoxLabel)];
    if let Some(t) = opt_s(n, "title").filter(|t| !t.is_empty()) {
        kids.push(txt(t, St::Strong));
    }
    let checks: Vec<&str> = arr(n, "checks").iter().filter_map(|c| c.as_str()).collect();
    if !checks.is_empty() {
        kids.push(txt(checks.join(", "), St::Small));
    }
    if let Some(r) = opt_s(n, "reason").filter(|r| !r.is_empty()) {
        kids.push(txt(r, St::Plain));
    }
    let links = arr(n, "links");
    if links.is_empty() {
        for u in arr(n, "proof").iter().filter_map(|u| u.as_str()).filter(|u| is_web(u)) {
            kids.push(Node::Link { s: u.to_string(), go: Go::Url(u.to_string()), tip: None, look: LinkLook::Plain });
        }
    }
    for l in links {
        let Some(u) = opt_s(l, "url").filter(|u| is_web(u)) else { continue };
        let label = opt_s(l, "label").filter(|x| !x.is_empty()).unwrap_or(u);
        kids.push(Node::Link { s: label.to_string(), go: Go::Url(u.to_string()), tip: Some(u.to_string()), look: LinkLook::Plain });
    }
    if let Some(at) = opt_s(n, "at").map(fmt::ago).filter(|a| !a.is_empty()) {
        kids.push(txt(format!("Checked {at}"), St::Small));
    }
    Some(el(K::Box(BoxTone::Info), kids))
}

fn is_web(u: &str) -> bool {
    let l = u.to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// `ATT_KIND`.
pub fn att_kind_label(k: &str) -> &'static str {
    match k {
        "design" => "Design",
        "proposal" => "Proposal",
        "doc" => "Doc",
        "evidence" => "Evidence",
        "results" => "Results",
        "other" => "Link",
        _ => "",
    }
}

pub const ATT_KINDS: [&str; 6] = ["design", "proposal", "doc", "evidence", "results", "other"];

/// What clicking an attachment's name opens (`attHref`): a web URL, a local path, or nothing.
pub fn att_href(a: &Value) -> Option<String> {
    let url = s(a, "url");
    if is_web(url) || url.starts_with('~') || url.starts_with('/') { Some(url.to_string()) } else { None }
}

// ------------------------------------------------------------------ context

/// `handoffText(t)`.
pub fn handoff_text(t: &Value, ui: &Ui) -> Option<String> {
    match t.get("handoff") {
        Some(Value::String(x)) => return Some(x.clone()),
        Some(h) if h.get("text").and_then(Value::as_str).is_some() => return Some(s(h, "text").to_string()),
        _ => {}
    }
    ui.handoffs.get(&rf(t, "T")).cloned()
}

/// `metaRows(t)`.
pub fn meta_rows(t: &Value, ui: &Ui) -> Vec<(String, String)> {
    let r = rf(t, "T");
    if let Some(rows) = ui.meta_edit.get(&r) {
        return rows.clone();
    }
    arr(t, "meta")
        .iter()
        .map(|p| match p {
            Value::Array(a) => (a.first().map(or_empty).unwrap_or_default(), a.get(1).map(or_empty).unwrap_or_default()),
            o => {
                let k = if !o["name"].is_null() { or_empty(&o["name"]) } else { or_empty(&o["k"]) };
                let v = if !o["value"].is_null() { or_empty(&o["value"]) } else { or_empty(&o["v"]) };
                (k, v)
            }
        })
        .collect()
}

/// `originRow(t)`: where the task came from ("From Backlog B3: Flaky test · You"), linked when it has a URL.
fn origin_row(t: &Value) -> Option<Node> {
    let o = obj(t, "origin")?;
    let from = opt_s(o, "from").or_else(|| crate::fmt::from_review_log(opt_s(o, "source").unwrap_or("")).then_some("the Review log"))?;
    let by = opt_s(o, "by").map(|b| format!(" · {b}")).unwrap_or_default();
    let value = match opt_s(o, "url") {
        Some(url) => el(K::Line, vec![Node::Link { s: from.to_string(), go: Go::Url(url.to_string()), tip: None, look: LinkLook::Plain }, txt(by, St::Plain)]),
        None => txt(format!("{from}{by}"), St::Plain),
    };
    Some(el(K::KvRow, vec![txt("From", St::LrowKey), value]))
}

/// The devices lent to it, each with its kind and target ("dev-a (Android emulator, emulator-5554)")
/// when the board gives them.
fn lent_devices(t: &Value) -> Vec<&str> {
    let d = &t["devices"];
    let lent: Vec<&str> = arr(d, "lent_labels").iter().filter_map(|x| x.as_str()).collect();
    if !lent.is_empty() {
        return lent;
    }
    arr(d, "lent").iter().filter_map(|x| x.as_str()).collect()
}

/// The devices it has (or asks for) and its bits, as Context rows. A backend bit not made yet has
/// "Mark created" (the owner's word that it's made in the flag tool, as on the goal page).
fn pool_rows(c: &Ctx, t: &Value) -> Vec<Node> {
    let r = rf(t, "T");
    let mut out = vec![];
    let d = &t["devices"];
    let lent = lent_devices(t);
    let devices = if !lent.is_empty() { Some(lent.join(", ")) } else { opt_s(d, "needs_text").map(|n| format!("Needs {n}")) };
    if let Some(v) = devices {
        out.push(el(K::KvRow, vec![txt("Devices", St::LrowKey), txt(v, St::Plain)]));
    }
    let mut bits: Vec<Node> = vec![];
    for (ix, x) in arr(t, "bits").iter().enumerate() {
        let state = if s(x, "kind") == "local" { "local" } else if b(x, "made") { "created" } else { "not created" };
        let sep = if ix + 1 < arr(t, "bits").len() { ", " } else { "" };
        bits.push(txt(format!("⚑ {} ({state}){sep}", s(x, "name")), St::Plain));
        if s(x, "kind") != "local" && !b(x, "made") {
            bits.push(c.btn("Mark created", Act::new("bit-made", s(x, "name"), &r, ""), Look::SoftSmall, false, Some(&format!("Tell the board {} is made", s(x, "name")))));
        }
    }
    if !bits.is_empty() {
        out.push(el(K::KvRow, vec![txt("Bits", St::LrowKey), el(K::Line, bits)]));
    }
    out
}

/// `contextTab(t)`.
fn context_tab(c: &Ctx, t: &Value) -> Vec<Node> {
    let r = rf(t, "T");
    let cx = &t["context"];
    let w = &cx["where"];
    let truthy = |v: &Value| !matches!(v, Value::Null | Value::Bool(false)) && v != "" && v.as_f64() != Some(0.);
    let mut wh: Vec<(&str, String)> = vec![
        (
            "Branch",
            opt_s(w, "branch").map(str::to_string).unwrap_or_else(|| if truthy(&t["started_at"]) || truthy(&t["who"]) { "Not reported yet".into() } else { "Not started".into() }),
        ),
        ("Worktree", opt_s(w, "worktree").or(opt_s(t, "repo_path")).or(opt_s(t, "project")).unwrap_or("").to_string()),
    ];
    if let Some(lc) = opt_s(w, "last_commit") {
        wh.push(("Last commit", lc.to_string()));
    }
    if w.get("uncommitted").is_some_and(|v| !v.is_null()) {
        wh.push(("Not committed", uncommitted(&w["uncommitted"])));
    }
    if truthy(&w["conversation"]) || truthy(&t["claude_session_id"]) {
        wh.push(("Conversation", "Can be reopened in a new terminal".into()));
    }
    let mut out = Vec::new();
    let saved = saved_line(t);
    if !saved.is_empty() {
        out.push(txt(saved, St::Muted));
    }
    let mut kv: Vec<Node> = wh.into_iter().map(|(k, v)| el(K::KvRow, vec![txt(k, St::LrowKey), txt(v, St::Plain)])).collect();
    kv.extend(origin_row(t));
    kv.extend(pool_rows(c, t));
    out.push(el(K::Kv, kv));
    for (title, key, glyph, dot) in [("Done so far", "done", "✓", Dot::Up), ("Next", "next", "→", Dot::Accent), ("Decisions", "decisions", "•", Dot::Faint), ("Your answers", "answers", "•", Dot::Warn)] {
        let items = arr(cx, key);
        if items.is_empty() {
            continue;
        }
        let list = items.iter().map(|x| el(K::Li, vec![txt(glyph, St::Glyph(dot)), txt(or_empty(x), St::Plain)])).collect();
        out.push(el(K::Stack, vec![txt(title, St::H3), el(K::List, list)]));
    }
    let found = arr(t, "found");
    if !found.is_empty() {
        let goal = obj(t, "goal").is_some();
        let list = found
            .iter()
            .map(|f| {
                el(
                    K::Li,
                    vec![
                        txt("!", St::Glyph(Dot::Warn)),
                        Node::Link { s: or_empty(&f["title"]), go: Go::Issue(rf(f, "B")), tip: None, look: LinkLook::Plain },
                        txt(format!("(in the {}backlog)", if goal { "goal’s " } else { "" }), St::Muted),
                    ],
                )
            })
            .collect();
        out.push(el(K::Stack, vec![txt("Found, not fixed here", St::H3), el(K::List, list)]));
    }
    let files = arr(cx, "files");
    let mut fl = vec![txt("Files touched", St::H3)];
    if files.is_empty() {
        fl.push(txt("None yet.", St::Help));
    } else {
        fl.push(el(
            K::Files,
            files
                .iter()
                .map(|f| {
                    let full = js_string(f);
                    txt_tip(full.rsplit('/').next().unwrap_or("").to_string(), St::Mono, full)
                })
                .collect(),
        ));
    }
    out.push(el(K::Stack, fl));
    let meta = meta_rows(t, c.ui);
    if !meta.is_empty() {
        let mut kids = vec![txt("Details", St::H3)];
        for (i, (k, v)) in meta.iter().enumerate() {
            kids.push(el(
                K::MetaRow,
                vec![
                    Node::Field { key: format!("meta:{r}:{i}:0"), value: k.clone(), placeholder: "Name", multi: false, mono: false },
                    Node::Field { key: format!("meta:{r}:{i}:1"), value: v.clone(), placeholder: "Value", multi: false, mono: false },
                    c.btn("", Act::new("meta-del", i.to_string(), &r, ""), Look::Icon(Icon::Remove), false, None),
                ],
            ));
        }
        kids.extend(c.note(&format!("meta:{r}")));
        out.push(el(K::Stack, kids));
    }
    let text = handoff_text(t, c.ui).unwrap_or_else(|| "Loading…".into());
    out.push(el(K::Box(BoxTone::Info), vec![txt("What a new terminal gets", St::H3), txt(text, St::Handoff)]));
    out
}

// ------------------------------------------------------------------ log

pub const LOG_FILTERS: [(&str, &str); 7] = [("all", "All"), ("status", "Status"), ("turn", "Turns"), ("checkpoint", "Checkpoints"), ("question", "Questions"), ("found", "Found"), ("jira", "Jira")];

/// `LOG_GROUP`.
pub fn log_group(kind: &str) -> &'static str {
    match kind {
        "status" | "midna" | "handoff" | "checkin" | "outside" => "status",
        "turn" | "commit" => "turn",
        "checkpoint" => "checkpoint",
        "question" | "answer" => "question",
        "found" => "found",
        "jira" => "jira",
        _ => "",
    }
}

/// `LOG_LABEL`.
fn log_label(kind: &str) -> String {
    match kind {
        "status" => "Status".into(),
        "note" => "Note".into(),
        "turn" => "Turn".into(),
        "checkpoint" => "Checkpoint".into(),
        "question" => "Question".into(),
        "answer" => "Answer".into(),
        "jira" => "Jira".into(),
        "found" => "Found".into(),
        "commit" => "Commit".into(),
        "midna" => "Midna".into(),
        "handoff" => "Handoff".into(),
        "checkin" => "Check-in".into(),
        "outside" => "Outside".into(),
        k => fmt::cap(k),
    }
}

/// `LOG_DOT`.
fn log_dot(kind: &str) -> Dot {
    match kind {
        "status" | "checkpoint" => Dot::Accent,
        "question" | "answer" | "found" | "checkin" | "outside" => Dot::Warn,
        "jira" => Dot::AccentFg,
        "commit" => Dot::Up,
        "handoff" => Dot::Goal,
        _ => Dot::Faint,
    }
}

/// The kinds a log filter shows ("all" for every kind), as the board's `?kind=` takes them.
pub fn log_kinds(f: &str) -> String {
    const KINDS: [&str; 14] = ["status", "midna", "handoff", "checkin", "outside", "turn", "commit", "checkpoint", "question", "answer", "found", "jira", "note", ""];
    if f == "all" {
        return f.to_string();
    }
    KINDS.iter().filter(|k| !k.is_empty() && log_group(k) == f).copied().collect::<Vec<_>>().join(",")
}

/// The Full log's entries under a filter, newest first; `more` when older ones are left to fetch.
#[derive(Clone, Debug, Default)]
pub struct LogRows {
    pub rows: Rc<Vec<Value>>,
    pub more: bool,
}

#[cfg(test)]
impl LogRows {
    /// Just the task's own page of the log.
    pub fn of(t: &Value, f: &str) -> Self {
        let rows = arr(t, "log").iter().filter(|e| f == "all" || log_group(s(e, "kind")) == f).cloned().collect();
        LogRows { rows: Rc::new(rows), more: false }
    }
}

pub const LOG_HELP: &str = "The log is saved as it happens, so it survives a closed terminal or a restart.";

/// One Full log entry (`.log li`).
pub fn log_item(e: &Value) -> Node {
    let at = s(e, "at");
    let kind = s(e, "kind");
    el(
        K::LogItem,
        vec![
            txt_tip(fmt::log_time(at), St::Time, fmt::full_time(at)),
            Node::Dot(log_dot(kind)),
            txt(opt_s(e, "who").unwrap_or("Task board"), St::Strong),
            txt(log_label(kind), St::Kind),
            txt(or_empty(&e["text"]), St::Plain),
        ],
    )
}

/// `logTab(t)`: the entries go in a [`Node::Log`], with [`LOG_HELP`] drawn after them.
fn log_tab(c: &Ctx, t: &Value) -> Vec<Node> {
    let f = c.ui.log_filter();
    let chips = LOG_FILTERS.iter().map(|(id, label)| c.btn(label, Act::new("log-filter", *id, "", ""), Look::Tab { on: f == *id }, false, None)).collect();
    let mut out = vec![el(K::Chips, chips)];
    if c.log.rows.is_empty() && c.log.more {
        out.push(txt("Loading…", St::Muted));
    } else if c.log.rows.is_empty() {
        out.push(txt(if arr(t, "log").is_empty() { "Nothing logged yet." } else { "Nothing of this kind yet." }, St::Empty));
        out.push(txt(LOG_HELP, St::Help));
    } else {
        out.push(Node::Log { rows: c.log.rows.clone(), more: c.log.more });
    }
    out
}

// ------------------------------------------------------------------ noteBody (pages.js)

const EXTS: [&str; 22] = ["dart", "kt", "kts", "java", "swift", "py", "js", "mjs", "ts", "tsx", "jsx", "json", "yaml", "yml", "md", "sh", "gradle", "xml", "html", "css", "toml", "lock"];

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// One `INLINE` match at `i` (url, `code` or a path): (end, kind, text).
fn inline_at(ch: &[char], i: usize) -> Option<(usize, u8, String)> {
    let rest: String = ch[i..ch.len().min(i + 8)].iter().collect();
    // (https?:\/\/[^\s<]+[^\s<.,;:)\]'"])
    for scheme in ["https://", "http://"] {
        if rest.starts_with(scheme) {
            let start = i + scheme.len();
            let mut end = start;
            while end < ch.len() && !ch[end].is_whitespace() && ch[end] != '<' {
                end += 1;
            }
            while end > start && matches!(ch[end - 1], '.' | ',' | ';' | ':' | ')' | ']' | '\'' | '"') {
                end -= 1;
            }
            if end - start >= 2 {
                return Some((end, b'u', ch[i..end].iter().collect()));
            }
        }
    }
    // \*\*([^*\n]+)\*\*
    if ch[i] == '*' && ch.get(i + 1) == Some(&'*') {
        let mut j = i + 2;
        while j < ch.len() && ch[j] != '*' && ch[j] != '\n' {
            j += 1;
        }
        if j > i + 2 && ch.get(j + 1) == Some(&'*') && ch[j] == '*' {
            return Some((j + 2, b'b', ch[i + 2..j].iter().collect()));
        }
    }
    // `([^`\n]+)`
    if ch[i] == '`' {
        let mut j = i + 1;
        while j < ch.len() && ch[j] != '`' && ch[j] != '\n' {
            j += 1;
        }
        if j < ch.len() && ch[j] == '`' && j > i + 1 {
            return Some((j + 1, b't', ch[i + 1..j].iter().collect()));
        }
    }
    // ((?:[\w.-]+|\.\.\.)\/(?:[\w.-]+\/|\.\.\.\/)*(?:[\w-]+(?:\.[\w-]+)*)?
    let seg = |c: char| is_word(c) || c == '.' || c == '-';
    if seg(ch[i]) {
        let mut j = i;
        let mut last_slash = None;
        loop {
            let s0 = j;
            while j < ch.len() && seg(ch[j]) {
                j += 1;
            }
            if j > s0 && j < ch.len() && ch[j] == '/' {
                j += 1;
                last_slash = Some(j);
            } else {
                break;
            }
        }
        if let Some(p) = last_slash {
            // final [\w-]+(\.[\w-]+)*
            let name = |c: char| is_word(c) || c == '-';
            let mut k = p;
            let mut end = p;
            while k < ch.len() && name(ch[k]) {
                k += 1;
            }
            if k > p {
                end = k;
                while k < ch.len() && ch[k] == '.' {
                    let d = k + 1;
                    let mut e = d;
                    while e < ch.len() && name(ch[e]) {
                        e += 1;
                    }
                    if e > d {
                        end = e;
                        k = e;
                    } else {
                        break;
                    }
                }
            }
            return Some((end, b'p', ch[i..end].iter().collect()));
        }
    }
    // \b[\w-]+\.(?:exts)\b
    let boundary = (i == 0 || !is_word(ch[i - 1])) && is_word(ch[i]);
    if boundary {
        let mut j = i;
        while j < ch.len() && (is_word(ch[j]) || ch[j] == '-') {
            j += 1;
        }
        // backtrack over the greedy run for a '.' + extension + \b
        let mut k = j;
        while k > i {
            if k < ch.len() && ch[k] == '.' {
                for e in EXTS {
                    let ext: Vec<char> = e.chars().collect();
                    let end = k + 1 + ext.len();
                    if end <= ch.len() && ch[k + 1..end] == ext[..] && (end == ch.len() || !is_word(ch[end])) {
                        return Some((end, b'p', ch[i..end].iter().collect()));
                    }
                }
            }
            k -= 1;
        }
    }
    None
}

/// `pathLike`.
fn path_like(p: &str) -> bool {
    p.ends_with('/') || p.contains('_') || p.contains('-') || p.contains("...") || {
        match p.rfind('.') {
            Some(d) => d + 1 < p.len() && p[d + 1..].chars().all(is_word),
            None => false,
        }
    }
}

/// `inlineText(s)`: text with links, **bold** and `code` (paths count as code).
fn inline_text(line: &str) -> Vec<Node> {
    let ch: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut i = 0;
    while i < ch.len() {
        if let Some((end, kind, text)) = inline_at(&ch, i) {
            if kind == b'p' && !path_like(&text) {
                plain.extend(&ch[i..end]);
                i = end.max(i + 1);
                continue;
            }
            if !plain.is_empty() {
                out.push(txt(std::mem::take(&mut plain), St::Plain));
            }
            out.push(match kind {
                b'u' => Node::Link { s: text.clone(), go: Go::Url(text.clone()), tip: Some(text), look: LinkLook::Plain },
                b'b' => txt(text, St::Strong),
                _ => txt(text, St::Code),
            });
            i = end;
        } else {
            plain.push(ch[i]);
            i += 1;
        }
    }
    if !plain.is_empty() {
        out.push(txt(plain, St::Plain));
    }
    out
}

/// Starts with two whitespace characters (`\s{2}`).
fn two_ws(line: &str) -> bool {
    let mut c = line.chars();
    c.next().is_some_and(char::is_whitespace) && c.next().is_some_and(char::is_whitespace)
}

/// `noteBody(text)`: paragraphs (lines joined by line breaks), `- ` lists, and code: indented, or
/// fenced in ``` (kept as written, blank lines and all).
pub fn note_body(text: &str) -> Node {
    let text = join_labels(text);
    let mut out = Vec::new();
    let mut kind: Option<&str> = None;
    let mut buf: Vec<String> = Vec::new();
    let mut fenced = false;
    let flush = |kind: Option<&str>, buf: &mut Vec<String>, out: &mut Vec<Node>| {
        if buf.is_empty() {
            return;
        }
        match kind {
            Some("li") => out.push(el(K::MdUl, buf.iter().map(|l| el(K::MdLi, inline_text(l))).collect())),
            Some("code") => out.push(txt(buf.join("\n"), St::Pre)),
            _ => {
                let mut kids = Vec::new();
                for l in buf.iter() {
                    kids.push(el(K::Line, inline_text(l)));
                }
                out.push(el(K::MdP, kids));
            }
        }
        buf.clear();
    };
    for raw in text.split('\n') {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            flush(kind, &mut buf, &mut out);
            kind = if fenced { None } else { Some("code") };
            fenced = !fenced;
            continue;
        }
        if fenced {
            buf.push(line.to_string());
            continue;
        }
        let k = if line.trim().is_empty() {
            None
        } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("• ") {
            Some("li")
        } else if two_ws(line) || line.starts_with('\t') {
            Some("code")
        } else {
            Some("p")
        };
        if k == Some("code") && kind == Some("li") && !buf.is_empty() {
            let last = buf.len() - 1;
            buf[last] = format!("{} {}", buf[last], line.trim());
            continue;
        }
        if k != kind || k.is_none() {
            flush(kind, &mut buf, &mut out);
        }
        kind = k;
        match k {
            Some("li") => buf.push(trimmed.chars().skip(2).collect()),
            Some("code") => buf.push(if two_ws(line) { line.chars().skip(2).collect() } else { line.chars().skip(1).collect() }),
            Some(_) => buf.push(line.trim().to_string()),
            None => {}
        }
    }
    flush(kind, &mut buf, &mut out);
    el(K::Md, out)
}

// ------------------------------------------------------------------ walks (what the tests compare)

#[cfg(test)]
/// Visible text in order, whitespace collapsed (closed folds show only their summary).
pub fn text(n: &Node) -> String {
    let mut parts = Vec::new();
    walk_text(n, &mut parts);
    parts.join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
fn walk_text(n: &Node, out: &mut Vec<String>) {
    match n {
        Node::Text { s, .. } | Node::Pill { s, .. } | Node::Link { s, .. } | Node::Note { s, .. } => out.push(s.clone()),
        Node::Btn { label, .. } => out.push(label.clone()),
        Node::Field { .. } | Node::Dot(_) => {}
        Node::El { .. } => shown(n).for_each(|k| walk_text(k, out)),
        Node::Log { rows, .. } => {
            rows.iter().for_each(|e| walk_text(&log_item(e), out));
            out.push(LOG_HELP.to_string());
        }
    }
}

#[cfg(test)]
/// Every control in order: "act", "act:arg", "!" when disabled.
pub fn acts(n: &Node) -> Vec<String> {
    let mut out = Vec::new();
    walk_acts(n, &mut out);
    out
}

#[cfg(test)]
fn walk_acts(n: &Node, out: &mut Vec<String>) {
    match n {
        Node::Btn { act, disabled, .. } => {
            let mut x = act.act.to_string();
            if !act.arg.is_empty() {
                x.push(':');
                x.push_str(&act.arg);
            }
            if *disabled {
                x.push('!');
            }
            out.push(x);
        }
        Node::El { .. } => shown(n).for_each(|k| walk_acts(k, out)),
        _ => {}
    }
}

/// The kids the reader sees: a closed fold shows only its summary.
pub fn shown(n: &Node) -> impl Iterator<Item = &Node> {
    let (kids, take): (&[Node], usize) = match n {
        Node::El { k: K::Fold(_, open) | K::TlStep { open, .. }, kids, .. } => (kids, if *open { usize::MAX } else { 1 }),
        // A dropdown row's hover card shows only while it's hovered.
        Node::El { k: K::DropItem(..), kids, .. } => (kids, 2),
        Node::El { kids, .. } => (kids, usize::MAX),
        _ => (&[], usize::MAX),
    };
    kids.iter().take(take)
}

#[cfg(test)]
/// Every tooltip in order.
pub fn titles(n: &Node) -> Vec<String> {
    let mut out = Vec::new();
    walk_titles(n, &mut out);
    out
}

#[cfg(test)]
fn walk_titles(n: &Node, out: &mut Vec<String>) {
    match n {
        Node::Text { tip: Some(t), .. } | Node::Pill { tip: Some(t), .. } | Node::Btn { tip: Some(t), .. } | Node::Link { tip: Some(t), .. } => out.push(t.clone()),
        Node::El { tip, .. } => {
            if let Some(t) = tip {
                out.push(t.clone());
            }
            shown(n).for_each(|k| walk_titles(k, out));
        }
        Node::Log { rows, .. } => rows.iter().for_each(|e| walk_titles(&log_item(e), out)),
        _ => {}
    }
}

#[cfg(test)]
/// Every field's contents in order.
pub fn values(n: &Node) -> Vec<String> {
    let mut out = Vec::new();
    walk_values(n, &mut out);
    out
}

#[cfg(test)]
fn walk_values(n: &Node, out: &mut Vec<String>) {
    match n {
        Node::Field { value, .. } => out.push(value.clone()),
        Node::El { .. } => shown(n).for_each(|k| walk_values(k, out)),
        _ => {}
    }
}

/// Find the first control with this act (and arg, when given).
pub fn find_act<'a>(n: &'a Node, act: &str, arg: Option<&str>) -> Option<&'a Act> {
    match n {
        Node::Btn { act: a, .. } if a.act == act && arg.is_none_or(|x| a.arg == x) => Some(a),
        Node::El { .. } => shown(n).find_map(|k| find_act(k, act, arg)),
        _ => None,
    }
}

/// A task waiting on one of the owner's steps (`t.step`): open where it happens, then Done; a step that
/// can't pass can be skipped.
fn step_btns(c: &Ctx, t: &Value, r: &str, grp: &str) -> Vec<Node> {
    let st = &t["step"];
    let Some(name) = opt_s(st, "name") else { return Vec::new() };
    let mut row = Vec::new();
    if let Some(url) = opt_s(st, "open") {
        row.push(c.btn("Open", Act::new("open-url", url, r, grp), Look::Plain, false, Some(url)));
    }
    if b(st, "failed") {
        row.push(c.btn("Skip this step", Act::new("step", "skip", r, grp), Look::Plain, false, Some(&format!("Count “{name}” as done and carry on"))));
    } else {
        row.push(c.btn("Done", Act::new("step", "done", r, grp), Look::Primary, false, Some(&format!("“{name}” is done; the agent carries on"))));
    }
    row
}
