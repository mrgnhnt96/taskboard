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
    High,
    Ref,
    Repo,
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
    Small,
    Danger,
    DangerSmall,
    Ghost,
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
    /// `.att-src.t`: a task ref chip.
    Src,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Close,
    Back,
    External,
    More,
    Remove,
}

/// How a plain link looks (`.goal-name`, `.jkey`, `.term-link`, `.att-name`, `.att-src.g`…).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LinkLook {
    #[default]
    Plain,
    Term,
    GoalName,
    Jkey,
    AttName,
    SrcGoal,
    Small,
}

/// Where a plain link (`<a href>`) goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Go {
    Goal(String),
    GoalBacklog(String),
    Session(String),
    /// The board page with this issue's panel open.
    Issue(String),
    /// A web URL, or a file path opened with `open` (an attachment).
    Url(String),
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
    Sub,
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
    /// A coloured list glyph (✓ → • !).
    Glyph(Dot),
}

/// Containers (the web's layout elements).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum K {
    #[default]
    Col,
    Top,
    Sub,
    Tabs,
    Body,
    /// `.box` in a tone: ask (warn), lost / failed (down), done (up), info.
    Box(BoxTone),
    Row,
    Stack,
    /// A `<details>` fold: first kid is the summary, the rest show when open.
    Fold(Fold, bool),
    Linked,
    /// `.lrow`: first kid is the key text.
    Lrow,
    Lsec,
    Line,
    Tl,
    TlItem,
    TlName,
    TlSub,
    PrHead,
    Steps,
    Step(StepSt),
    PrBar { warn: bool },
    Atts,
    Att,
    AttMeta,
    AttMenu,
    AttEdit,
    Seg,
    Kv,
    KvRow,
    List,
    Li,
    Files,
    MetaRow,
    Chips,
    Log,
    LogItem,
    Md,
    MdP,
    MdUl,
    MdLi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxTone {
    Ask,
    Lost,
    Done,
    Failed,
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fold {
    Linked,
    Summary,
    What,
    Terms,
    /// A step's findings (`step_results`), under its headline.
    Findings,
}

impl Fold {
    /// The localStorage key the web kept this fold's state under.
    pub fn key(self) -> &'static str {
        match self {
            Fold::Linked => "tb.linked",
            Fold::Summary => "tb.fold.summary",
            Fold::What => "tb.fold.what",
            Fold::Terms => "tb.fold.terms",
            Fold::Findings => "tb.fold.findings",
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

/// Session dot colours (`SESS_DOT`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dot {
    Idle,
    Working,
    Needs,
    Gone,
    /// A closed terminal: no colour.
    None,
    /// Log dots by kind (`LOG_DOT`).
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
}

impl Ui {
    pub fn log_filter(&self) -> &str {
        if self.log_filter.is_empty() { "all" } else { &self.log_filter }
    }
}

pub struct Ctx<'a> {
    pub state: &'a Value,
    pub task: Option<&'a Value>,
    /// The panel's task ref (`S.taskRef`).
    pub r: &'a str,
    /// overview | context | log
    pub tab: &'a str,
    /// Tasks opened from the panel before this one (`taskTrail()`).
    pub trail: &'a [String],
    /// The goal whose page is open, if any (`S.route.page === 'goal'`, `S.route.id`).
    pub goal_page: Option<&'a str>,
    /// Why the task couldn't be fetched (`S.taskErr`).
    pub err: Option<&'a str>,
    pub ui: &'a Ui,
    /// Saved fold states by localStorage key ("open" / "closed").
    pub folds: &'a HashMap<String, String>,
}

impl Ctx<'_> {
    /// `linkedOpen`: the Details fold, open unless closed.
    fn linked_open(&self) -> bool {
        self.folds.get("tb.linked").map(String::as_str) != Some("closed")
    }

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
        "working" => Tone::Working,
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
        "fix" => Tone::Failed,
        "ask" | "review" | "rereview" => Tone::Review,
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

/// `taskPanel()`.
pub fn panel(c: &Ctx) -> Node {
    let Some(t) = c.task else {
        let mut top = Vec::new();
        top.extend(back_btn(c));
        top.push(pill(c.r, Tone::Ref));
        top.push(close_btn(c));
        let msg = match c.err {
            Some(e) => Node::Note { s: e.to_string(), err: true },
            None => txt("Loading…", St::Muted),
        };
        return el(K::Col, vec![el(K::Top, top), msg]);
    };
    let r = rf(t, "T");
    let alert = arr(c.state, "alerts").iter().find(|a| s(a, "task") == r);
    let tab = if matches!(c.tab, "context" | "log") { c.tab } else { "overview" };

    let mut top = Vec::new();
    top.extend(back_btn(c));
    top.push(status_pill(t));
    if let Some(a) = alert {
        top.push(Node::Pill { s: "Needs you".into(), tone: Tone::Needs, tip: Some(or_empty(&a["text"])) });
    }
    if s(t, "priority") == "high" {
        top.push(pill("High priority", Tone::High));
    }
    top.push(pill(&r, Tone::Ref));
    top.push(close_btn(c));

    let mut sub = vec![pill(or_empty(&t["project"]), Tone::Repo), txt(fmt::when_line(t), St::Sub)];
    let took = fmt::took_line(t);
    if !took.is_empty() {
        sub.push(txt(took, St::Sub));
    }
    if matches!(s(t, "status"), "working" | "needs") {
        let run = fmt::running_for(t);
        if !run.is_empty() {
            sub.push(txt(fmt::cap(&run), St::Sub));
        }
    }

    let mut kids = vec![el(K::Top, top)];
    if let Some(e) = c.err {
        kids.push(Node::Note { s: e.to_string(), err: true });
    }
    kids.push(txt(or_empty(&t["title"]), St::Title));
    kids.push(el(K::Sub, sub));
    if let Some(a) = alert {
        kids.push(el(K::Box(BoxTone::Ask), vec![txt("Needs you", St::BoxLabel), txt(or_empty(&a["text"]), St::Plain)]));
    }
    let tabs = [("overview", "Overview"), ("context", "Context"), ("log", "Log")]
        .iter()
        .map(|(id, label)| c.btn(label, Act::new("tab", *id, "", ""), Look::Tab { on: tab == *id }, false, None))
        .collect();
    kids.push(el(K::Tabs, tabs));
    let body = match tab {
        "context" => context_tab(c, t),
        "log" => log_tab(c, t),
        _ => overview_tab(c, t),
    };
    kids.push(el(K::Body, body));
    el(K::Col, kids)
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

fn answer_field(c: &Ctx, r: &str) -> Node {
    let k = format!("answer:{r}");
    Node::Field { value: c.draft(&k), key: k, placeholder: "", multi: true, mono: false }
}

/// `overviewTab(t)`.
fn overview_tab(c: &Ctx, t: &Value) -> Vec<Node> {
    let r = rf(t, "T");
    let who = who(t);
    let act = format!("act:{r}");
    let status = s(t, "status");
    let mut out = Vec::new();

    if b(t, "lost") {
        let saved = saved_line(t);
        let claude = opt_s(t, "claude_session_id").is_some();
        let grp = format!("lost:{r}");
        let mut bx = vec![txt("Terminal lost", St::BoxLabel), txt(opt_s(t, "latest").unwrap_or("The terminal closed before the task was done."), St::Plain)];
        if !saved.is_empty() {
            bx.push(txt(saved, St::Small));
        }
        bx.push(el(
            K::Row,
            vec![
                c.btn("Resume in a new terminal", Act::new("resume", "fresh", &r, &grp), Look::Primary, false, None),
                c.btn(
                    "Reopen the old conversation",
                    Act::new("resume", "reopen", &r, &grp),
                    Look::Plain,
                    !claude,
                    (!claude).then_some("No conversation was saved for this task"),
                ),
            ],
        ));
        bx.extend(c.note(&grp));
        bx.push(c.btn("See what the new terminal gets", Act::new("tab", "context", "", ""), Look::Link, false, None));
        out.push(el(K::Box(BoxTone::Lost), bx));
    } else if status == "needs" && s(t, "needs_reason") == "start_failed" {
        let mut bx = vec![
            txt("Couldn’t start in Midna", St::BoxLabel),
            txt(opt_s(t, "latest").unwrap_or("Midna didn’t start a terminal for this task."), St::Plain),
            el(
                K::Row,
                vec![
                    c.btn("New Midna terminal", Act::new("start", "new", &r, &act), Look::Primary, false, None),
                    c.btn("Queue in Midna", Act::new("start", "queue", &r, &act), Look::Plain, false, None),
                ],
            ),
        ];
        bx.extend(c.note(&act));
        out.push(el(K::Box(BoxTone::Lost), bx));
    } else if status == "needs" {
        let q = opt_s(t, "question");
        let grp = format!("ask:{r}");
        let mut row = step_btns(c, t, &r, &grp);
        row.extend(answer_btns(c, &r));
        if q.is_none() && session_live(t) {
            row.push(c.btn("Focus in Midna", Act::new("focus", "", &r, &grp), Look::Plain, false, None));
        }
        row.extend(c.note(&grp));
        out.push(el(
            K::Box(BoxTone::Ask),
            vec![
                txt(format!("{who} {}", if q.is_some() { "is asking" } else { "needs you" }), St::BoxLabel),
                txt(q.or(opt_s(t, "latest")).unwrap_or("It’s waiting for you in Midna."), St::Plain),
                answer_field(c, &r),
                el(K::Row, row),
            ],
        ));
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

    if let Some(stopped) = pr_stopped(t) {
        let p = pr_of(t).unwrap_or(&Value::Null);
        let term = arr(t, "terminals").iter().find(|x| js_eq(field(x, "id"), field(&p["stage"], "session"))).and_then(|x| opt_s(x, "name")).unwrap_or("Its terminal");
        let grp = format!("ask:{r}");
        let mut row = answer_btns(c, &r);
        row.push(c.btn("Focus in Midna", Act::new("focus", "", &r, &grp), Look::Plain, false, None));
        row.extend(c.note(&grp));
        out.push(el(
            K::Box(BoxTone::Ask),
            vec![
                txt(format!("{term} {}", if b(stopped, "asked") { "asks you about the PR" } else { "stopped on the PR" }), St::BoxLabel),
                txt(or_empty(&stopped["message"]), St::Plain),
                answer_field(c, &r),
                el(K::Row, row),
            ],
        ));
    }

    if status == "done" {
        let failed = b(t, "failed");
        let summary = fold(
            Fold::Summary,
            c.fold_open(Fold::Summary, true),
            txt(format!("Summary from {who}"), St::BoxLabel),
            vec![txt(opt_s(t, "summary").or(opt_s(t, "latest")).unwrap_or("No summary was given."), St::Plain)],
        );
        let mut row = vec![c.btn(if failed { "Try again" } else { "Queue again" }, Act::new("requeue", "", &r, &act), Look::Plain, false, None)];
        if session_live(t) {
            row.push(c.btn("Close its terminal", Act::new("close-term", "", &r, &act), Look::Danger, false, None));
        }
        let mut bx = vec![summary, el(K::Row, row)];
        bx.extend(c.note(&act));
        out.push(el(K::Box(if failed { BoxTone::Failed } else { BoxTone::Done }), bx));
    } else if status != "planned" {
        out.push(manage_box(c, t));
    }

    let linked: Vec<Node> = [blocked_row(c, t), goal_row(c, t), Some(terminal_row(c, t)), jira_row(c, t), Some(pr_row(c, t)), steps_row(c, t), Some(attach_row(c, t))].into_iter().flatten().collect();
    out.push(fold(Fold::Linked, c.linked_open(), txt("Details", St::Strong), vec![el(K::Linked, linked)]));
    let what = match opt_s(t, "detail") {
        Some(d) => note_body(d),
        None => txt("Nothing written yet.", St::Plain),
    };
    out.push(fold(Fold::What, c.fold_open(Fold::What, true), txt("What to do", St::Strong), vec![what]));
    out
}

/// A fold (`<details>`): its summary, then what shows when it's open.
fn fold(f: Fold, open: bool, summary: Node, body: Vec<Node>) -> Node {
    let mut kids = vec![summary];
    kids.extend(body);
    el(K::Fold(f, open), kids)
}

/// `manageBox(t)`. No Mark done / Mark failed: Claude finishes a task (`tb done`, `tb fail`).
fn manage_box(c: &Ctx, t: &Value) -> Node {
    let r = rf(t, "T");
    let grp = format!("manage:{r}");
    let live = session_live(t);
    let mut buttons = Vec::new();
    if live && s(t, "status") == "working" {
        buttons.push(c.btn("Focus in Midna", Act::new("focus", "", &r, &grp), Look::Small, false, None));
    }
    if opt_s(t, "session_id").is_some() || live {
        buttons.push(c.btn("Detach", Act::new("detach", "", &r, &grp), Look::Small, false, Some("Take it off its terminal and put it back in the queue")));
    }
    if live {
        let arg = if s(&t["session"], "status") == "idle" { "" } else { "force" };
        buttons.push(c.btn("Close its terminal", Act::new("close-term", arg, &r, &grp), Look::DangerSmall, false, Some("Close the terminal; a busy one is stopped")));
    }
    let mut kids = vec![el(K::Row, buttons)];
    kids.extend(c.note(&grp));
    el(K::Stack, kids)
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
        body.push(el(K::Line, vec![c.btn(&br, Act::new("open-task", "", &br, ""), Look::Ref, false, None), status_pill(x)]));
        body.push(txt(or_empty(&x["title"]), St::Plain));
    }
    Some(lrow("Blocked by", body))
}

/// `goalRow(t)`.
fn goal_row(c: &Ctx, t: &Value) -> Option<Node> {
    if let Some(g) = obj(t, "goal").filter(|g| !g["id"].is_null() || !g["ref"].is_null()) {
        let gr = rf(g, "G");
        if c.goal_page == Some(gr.as_str()) {
            return None;
        }
        let total = g["total"].as_f64().unwrap_or(0.);
        let pos = g["position"].as_i64().filter(|_| g["position"].is_i64() || g["position"].is_u64()).filter(|p| total != 0. && (*p as f64) <= total);
        let mut line = vec![Node::Link { s: or_empty(&g["name"]), go: Go::Goal(gr.clone()), tip: None, look: LinkLook::GoalName }];
        if let Some(p) = pos {
            line.push(txt(format!("Task {p} of {}", or_empty(&g["total"])), St::Small));
        }
        let n = g["open_issues"].as_i64().unwrap_or(0);
        return Some(lrow(
            "Goal",
            vec![
                el(K::Line, line),
                txt(match opt_s(g, "next_title") {
                    Some(x) => format!("Next in this goal: {x}"),
                    None => "Last task in this goal".into(),
                }, St::Small),
                Node::Link {
                    s: if n != 0 { format!("{n} open in the goal’s backlog") } else { "Nothing in the goal’s backlog".into() },
                    go: Go::GoalBacklog(gr),
                    tip: None,
                    look: LinkLook::Small,
                },
            ],
        ));
    }
    Some(lrow("Goal", vec![txt("Not in a goal.", St::Small)]))
}

/// `SESS`.
fn sess_label(st: &str) -> &'static str {
    match st {
        "idle" => "Idle",
        "working" => "Working",
        "needs" => "Needs you",
        _ => "Gone",
    }
}

/// `termItem(x, cur)`.
fn term_item(c: &Ctx, x: &Value, cur: &Value) -> Node {
    let raw = match s(x, "status") {
        k @ ("idle" | "working" | "needs" | "gone") => k,
        _ => "gone",
    };
    let stage = &cur["pr"]["stage"];
    // `waitsOnYou`
    let waiting = raw != "gone" && obj(stage, "stopped").is_some() && js_eq(field(stage, "session"), field(x, "id"));
    let st = if waiting { "needs" } else { raw };
    let live = st != "gone";
    let why = if waiting {
        if b(&stage["stopped"], "asked") { "Asked you a question".to_string() } else { "Stopped before it was finished".to_string() }
    } else {
        match s(x, "why") {
            "Worked on the task" => "Task".into(),
            "Worked on the PR" => "PR".into(),
            _ => or_empty(&x["why"]),
        }
    };
    let id = opt_s(x, "id").map(str::to_string);
    let name = opt_s(x, "name").map(str::to_string).unwrap_or_else(|| or_empty(&x["id"]));
    let state = if live {
        sess_label(st)
    } else if b(cur, "lost") && js_eq(field(x, "id"), cur.get("session").and_then(|ss| field(ss, "id"))) {
        "Gone"
    } else {
        "Closed"
    };
    let mut head = Vec::new();
    match &id {
        Some(sid) => head.push(Node::Link { s: name.clone(), go: Go::Session(sid.clone()), tip: Some("Open this terminal".into()), look: LinkLook::Term }),
        None => head.push(txt(name.clone(), St::Plain)),
    }
    if let (true, Some(sid)) = (live, &id) {
        head.push(c.btn("", Act::new("term-focus", "", sid, format!("term:{}", rf(cur, "T"))), Look::Icon(Icon::External), false, Some("Focus in Midna")));
    }
    let mut sub = vec![pill(state, st_tone(if live { st } else { "closed" }))];
    if !why.is_empty() {
        sub.push(txt(why, St::Small));
    }
    let dot = if !live {
        Dot::None
    } else {
        match st {
            "working" => Dot::Working,
            "needs" => Dot::Needs,
            "idle" => Dot::Idle,
            _ => Dot::Gone,
        }
    };
    let mut kids = vec![Node::Dot(dot), el(K::TlName, head), el(K::TlSub, sub)];
    if let Some(at) = opt_s(x, "at") {
        kids.push(txt(fmt::hhmm(at), St::Time));
    }
    el(K::TlItem, kids)
}

/// `terminalRow(t)`.
fn terminal_row(c: &Ctx, t: &Value) -> Node {
    let grp = format!("term:{}", rf(t, "T"));
    let sess = &t["session"];
    let mut all: Vec<Value> = arr(t, "terminals").to_vec();
    if all.is_empty() && session_live(t) {
        let name = opt_s(sess, "name").or(opt_s(t, "who")).map(Value::from).unwrap_or_else(|| sess["id"].clone());
        all.push(serde_json::json!({"id": sess["id"], "name": name, "status": sess["status"], "why": "Attached", "at": t["started_at"]}));
    }
    if all.is_empty() && opt_s(t, "who").is_some() && !matches!(s(t, "status"), "queued" | "planned") {
        all.push(serde_json::json!({"name": t["who"], "status": "gone"}));
    }
    if all.is_empty() {
        let mut body = vec![txt(if s(t, "status") == "done" { "No terminal." } else { "No terminal yet." }, St::Small)];
        if s(t, "status") != "done" {
            body.extend(c.note(&grp));
        }
        return lrow("Terminal", body);
    }
    let cur = opt_s(sess, "id");
    let rank = |x: &Value| {
        if cur.is_some() && opt_s(x, "id") == cur {
            0
        } else if opt_s(x, "status").is_some_and(|st| st != "gone") {
            1
        } else {
            2
        }
    };
    let mut ranked: Vec<(usize, Value)> = all.into_iter().enumerate().collect();
    ranked.sort_by_key(|(k, x)| (rank(x), *k));
    let all: Vec<Value> = ranked.into_iter().map(|(_, x)| x).collect();
    let shown = if all.len() > 2 { 1 } else { all.len() };
    let mut body = vec![el(K::Tl, all[..shown].iter().map(|x| term_item(c, x, t)).collect())];
    let rest = &all[shown..];
    if !rest.is_empty() {
        body.push(fold(
            Fold::Terms,
            c.fold_open(Fold::Terms, true),
            txt(fmt::plural(rest.len() as i64, "earlier terminal", "earlier terminals"), St::Small),
            vec![el(K::Tl, rest.iter().map(|x| term_item(c, x, t)).collect())],
        ));
    }
    body.extend(c.note(&grp));
    lrow(if all.len() > 1 { "Terminals" } else { "Terminal" }, body)
}

/// `jiraRow(t)`.
fn jira_row(c: &Ctx, t: &Value) -> Option<Node> {
    let j = obj(t, "jira");
    let key = j.and_then(|j| opt_s(j, "key"));
    let on = b(&c.state["jira"], "enabled");
    if !on && key.is_none() {
        return None;
    }
    let v = match (j, key) {
        (Some(j), Some(key)) => {
            let mut line = vec![match opt_s(j, "url") {
                Some(url) => Node::Link { s: key.to_string(), go: Go::Url(url.to_string()), tip: Some(format!("Open {key} in Jira")), look: LinkLook::Jkey },
                None => txt(key, St::Jkey),
            }];
            if let Some(st) = opt_s(j, "status") {
                line.push(pill(st, Tone::Jira));
            }
            el(K::Line, line)
        }
        // A ticket that couldn't be made reads as a warning (`tb task set --jira new` tries again).
        (Some(j), None) if b(j, "failed") => txt(opt_s(j, "status").unwrap_or("Ticket asked for"), St::WarnHelp),
        (Some(j), None) => txt(opt_s(j, "status").unwrap_or("Ticket asked for"), St::Small),
        _ => txt("No ticket.", St::Small),
    };
    Some(lrow("Jira", vec![v]))
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
            Some(if obj(stage, "stopped").is_some() { "Fix stopped" } else if !stage["session"].is_null() && stage["session"] != "" { "Being fixed" } else { "Fix starting" }.into()),
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
    pub link: Option<String>,
}

fn step(name: impl Into<String>, st: StepSt, sub: Option<&str>, link: Option<String>) -> PrStep {
    PrStep { name: name.into(), st, sub: sub.map(str::to_string), link }
}

/// The author-side review step (`wd`: its `bar` name, like WD, and its latest round's headline).
pub fn wd_step(wd: &Value) -> PrStep {
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

/// The steps row's nodes: each step's name, and its state's words (a link when it has one).
fn step_nodes(list: Vec<PrStep>) -> Node {
    let mut steps = Vec::new();
    for step in list {
        let mut kids = vec![txt(step.name, St::StepName)];
        match (step.sub, step.link) {
            (Some(x), Some(url)) => kids.push(Node::Link { s: x, go: Go::Url(url), tip: None, look: LinkLook::Small }),
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
        _ => step(checks.0, checks.1, checks.2.as_deref(), if checks.1 == StepSt::Todo { None } else { build }),
    });
    out.push(match s(bar, "you") {
        "waiting" => step("You", StepSt::Ask, Some("Waiting on you"), None),
        "reviewed" => step("You", StepSt::Done, Some("Reviewed"), None),
        "skipped" => step("You", StepSt::Done, Some("Skipped"), None),
        _ => step("You", StepSt::Todo, None, None),
    });
    let url = opt_s(p, "url").filter(|u| is_web(u)).map(str::to_string);
    let approvals = bar["approvals"].as_i64().unwrap_or(0);
    let reviewers = bar["reviewers"].as_i64().unwrap_or(0);
    let new_comments = bar["new_comments"].as_i64().unwrap_or(0);
    let rv = js_lower(&p["review"]);
    out.push(if phase == "comments" || (new_comments > 0 && rv != "changes" && phase != "rereview") {
        // "2 new comments", linking to the first unread thread.
        let words = if new_comments > 0 { fmt::plural(new_comments, "new comment", "new comments") } else { "New comments".to_string() };
        let go = opt_s(bar, "comments_url").filter(|u| is_web(u)).map(str::to_string).or(url);
        step("Review", StepSt::Ask, Some(&words), go)
    } else if reviewers > 0 && matches!(review.1, StepSt::Done | StepSt::Wait | StepSt::Todo) && phase != "rereview" {
        let st = if approvals >= reviewers { StepSt::Done } else if approvals > 0 || rv == "pending" { StepSt::Wait } else { review.1 };
        step("Review", st, Some(&format!("{approvals} of {reviewers}")), url)
    } else {
        step(review.0, review.1, review.2.as_deref(), url)
    });
    out.push(if phase == "waits" && pr_open(p) { step("Merge", StepSt::Wait, Some("Waits on base"), None) } else { step(merge.0, merge.1, merge.2.as_deref(), None) });
    out
}

/// "Stacks on T3 · PR #12": the task it builds on opens in the panel, its PR on the host.
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
    el(K::Line, kids)
}

/// One row per reviewer (`pr.bar.reviewer_rows`: name, state, link), when the board sends them.
fn reviewer_rows(bar: &Value) -> Vec<Node> {
    arr(bar, "reviewer_rows")
        .iter()
        .filter_map(|r| {
            let name = opt_s(r, "name")?;
            let mut kids = vec![txt(name, St::Plain)];
            if let Some(st) = opt_s(r, "state") {
                let (label, tone) = match st {
                    "approved" => ("Approved", Tone::Done),
                    "changes" => ("Changes asked", Tone::Needs),
                    "rereview" => ("Re-review waiting", Tone::Review),
                    "waiting" => ("Waiting", Tone::Queued),
                    "commented" => ("Commented", Tone::Neutral),
                    other => (other, Tone::Neutral),
                };
                kids.push(pill(label, tone));
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
            Some(el(K::Line, kids))
        })
        .collect()
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
    w.len() > 1 && w.starts_with('T') && w[1..].chars().all(|c| c.is_ascii_digit())
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

/// The PR row of a task with no PR: why it finished without one, or whether it's meant to have one.
fn no_pr_row(c: &Ctx, t: &Value) -> Node {
    let mut body = Vec::new();
    if let Some(why) = opt_s(t, "no_pr") {
        let mut line = vec![txt("PR canceled:", St::Strong)];
        line.extend(linkify(c, why));
        body.push(el(K::Line, line));
    } else if t["ships_pr"].is_boolean() {
        body.push(if t["ships_pr"] == true {
            el(K::Line, vec![pill("Ends in a PR", Tone::Review), txt("No PR yet.", St::Small)])
        } else {
            el(K::Line, vec![pill("No PR", Tone::Neutral)])
        });
    } else {
        body.push(txt("No PR yet.", St::Small));
    }
    // The review step (WD) runs before the PR opens: it shows from its first round.
    let wd = obj(t, "wd").or_else(|| arr(t, "step_results").iter().find(|r| opt_s(r, "bar").is_some()));
    if let Some(wd) = wd.filter(|_| opt_s(t, "no_pr").is_none()) {
        body.push(step_nodes(vec![wd_step(wd)]));
    }
    if let Some(so) = obj(t, "stack_on") {
        body.push(stacks_line(c, so));
    }
    if let Some(why) = opt_s(t, "no_evidence") {
        let mut line = vec![txt("No evidence:", St::Strong)];
        line.extend(linkify(c, why));
        body.push(el(K::Line, line));
    }
    lrow("Pull request", body)
}

/// Each step that ran a round (`step_results`): its headline, and its findings in a fold.
fn steps_row(c: &Ctx, t: &Value) -> Option<Node> {
    let results = arr(t, "step_results");
    if results.is_empty() {
        return None;
    }
    let mut body = Vec::new();
    for r in results {
        let mut line = vec![txt(s(r, "name"), St::Strong), txt(s(r, "headline"), St::Plain)];
        if b(r, "stale") {
            line.push(pill("Moved since", Tone::Needs));
        }
        body.push(el(K::Line, line));
        let findings = arr(r, "findings");
        if findings.is_empty() {
            continue;
        }
        let open = r["open"].as_i64().unwrap_or(0);
        let summary = if open > 0 { format!("{} · Findings ›", fmt::plural(open, "open finding", "open findings")) } else { "Findings ›".to_string() };
        let items = findings.iter().map(finding_item).collect();
        body.push(fold(Fold::Findings, c.fold_open(Fold::Findings, true), txt(summary, St::Small), vec![el(K::List, items)]));
    }
    Some(lrow("Steps", body))
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

/// `prRow(t)` with `prBar`.
fn pr_row(c: &Ctx, t: &Value) -> Node {
    let Some(p) = pr_of(t) else {
        return no_pr_row(c, t);
    };
    let key = obj(t, "jira").and_then(|j| opt_s(j, "key"));
    let raw = opt_s(p, "title");
    let title: Option<String> = match (key, raw) {
        (Some(k), Some(x)) if x.starts_with(k) => Some(x[k.len()..].trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ':' | '–' | '-')).to_string()),
        (_, x) => x.map(str::to_string),
    };
    let shown = title.clone().filter(|x| !x.is_empty()).or_else(|| opt_s(p, "repo").map(str::to_string)).unwrap_or_default();
    let label = format!("#{} {shown}", or_empty(&p["num"]));
    let head = match opt_s(p, "url").filter(|u| is_web(u)) {
        Some(url) => Node::Link { s: label, go: Go::Url(url.to_string()), tip: Some("Open the pull request".into()), look: LinkLook::Plain },
        None => {
            // safeUrl('') → '#': a url that isn't http(s) still renders as a link that goes nowhere.
            match opt_s(p, "url") {
                Some(_) => Node::Link { s: label, go: Go::Url(String::new()), tip: Some("Open the pull request".into()), look: LinkLook::Plain },
                None => txt(label, St::Strong),
            }
        }
    };
    let repo = if title.as_deref().is_some_and(|x| !x.is_empty()) { or_empty(&p["repo"]) } else { String::new() };
    let bar = obj(p, "bar");
    let list: Vec<PrStep> = match bar {
        Some(bar) => pr_steps_full(p, bar),
        None => pr_steps(p).into_iter().map(|(name, st, sub)| PrStep { name: name.to_string(), st, sub, link: None }).collect(),
    };
    let mut body = vec![el(K::PrHead, vec![head, txt(repo, St::Small)]), step_nodes(list)];
    if let Some(n) = not_ours_box(p) {
        body.push(n);
    }
    if let Some(line) = bar.and_then(|bar| obj(bar, "stacks_on")).map(|so| stacks_line(c, so)) {
        body.push(line);
    }
    body.extend(bar.map(reviewer_rows).unwrap_or_default());
    // `prBar`
    if let Some(stage) = obj(p, "stage").filter(|st| pr_open(p) && matches!(s(st, "phase"), "fix" | "comments" | "merge" | "ask")) {
        let stopped = obj(stage, "stopped").is_some();
        let label = or_empty(&stage["label"]);
        let text = if stopped { format!("Needs you · {label}") } else { label };
        let mut kids = vec![Node::Dot(if stopped { Dot::Warn } else { Dot::Accent }), txt(text, St::Plain)];
        let (tip, go) = match opt_s(stage, "session") {
            Some(sid) => {
                kids.push(Node::Link { s: "Terminal ›".into(), go: Go::Session(sid.to_string()), tip: None, look: LinkLook::Plain });
                (if stopped { "Its terminal stopped before it was finished" } else { "Open the terminal doing this" }, true)
            }
            None => ("Waiting for its terminal to open", false),
        };
        let _ = go;
        body.push(Node::El { k: K::PrBar { warn: stopped && opt_s(stage, "session").is_some() }, tip: Some(tip.into()), kids });
    }
    // The owner's own look at a green PR (the You step): "Waiting on your review · Mark reviewed ›".
    if bar.is_some_and(|bar| s(bar, "you") == "waiting") {
        let r = rf(t, "T");
        let grp = format!("pr:{r}");
        let mark = c.btn("Mark reviewed ›", Act::new("pr-reviewed", "", &r, &grp), Look::Link, false, Some("Tell the board you've looked at this PR"));
        body.push(Node::El { k: K::PrBar { warn: false }, tip: None, kids: vec![Node::Dot(Dot::Accent), txt("Waiting on your review", St::Plain), mark] });
        body.extend(c.note(&grp));
    } else if bar.is_none() && obj(p, "stage").is_some_and(|st| b(st, "awaiting_you")) {
        // Native: the owner's one-click "I reviewed it" while a green PR waits for them.
        let r = rf(t, "T");
        let grp = format!("pr:{r}");
        body.push(el(K::Row, vec![c.btn("I reviewed it", Act::new("pr-reviewed", "", &r, &grp), Look::SoftSmall, false, Some("Tell the board you've looked at this PR"))]));
        body.extend(c.note(&grp));
    }
    lrow("Pull request", body)
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

/// `attachList(list, 'tasks', r)` items (with `attSrc`, `attMenuHtml`, `attEditHtml`).
fn attach_items(c: &Ctx, list: &[Value], owner: &str, r: &str) -> Vec<Node> {
    let fk = format!("att:{owner}:{r}");
    let mut out = Vec::new();
    for a in list {
        let aid = or_empty(&a["id"]);
        if c.ui.att_edit.as_deref() == Some(aid.as_str()) {
            out.push(att_edit(c, a));
            continue;
        }
        let url = or_empty(&a["url"]);
        let web = is_web(&url);
        let href = att_href(a);
        let kind = if att_kind_label(s(a, "kind")).is_empty() { "other" } else { s(a, "kind") };
        let title = or_empty(&a["title"]);
        let name = match &href {
            Some(h) => Node::Link { s: title.clone(), go: Go::Url(h.clone()), tip: Some(if web { title.clone() } else { format!("{title}\n{url}") }), look: LinkLook::AttName },
            None => Node::Text { s: title.clone(), st: St::Mono, tip: Some(url.clone()) },
        };
        let mut meta = vec![txt(att_kind_label(kind), St::Small)];
        // `attSrc`
        let src = opt_s(a, "task").or(opt_s(a, "goal"));
        if let Some(x) = src.filter(|x| !(owner == "goals" && *x == r)) {
            meta.push(txt("·", St::Small));
            if opt_s(a, "task").is_some() {
                meta.push(c.btn(x, Act::new("open-task", "", x, ""), Look::Src, false, Some(&format!("Open {x}"))));
            } else {
                meta.push(Node::Link { s: x.to_string(), go: Go::Goal(x.to_string()), tip: Some(format!("Open {x}")), look: LinkLook::SrcGoal });
            }
        }
        if let Some(at) = opt_s(a, "at") {
            meta.push(txt("·", St::Small));
            meta.push(txt(fmt::ago(at), St::Small));
        }
        let mut kids = vec![name, el(K::AttMeta, meta), c.btn("", Act::new("att-menu", "", &aid, ""), Look::Icon(Icon::More), false, None)];
        if c.ui.att_menu.as_deref() == Some(aid.as_str()) {
            let mut menu = Vec::new();
            if href.is_some() {
                menu.push(c.btn("Open", Act::new("att-menu-close", "", &aid, ""), Look::Menu, false, None));
            }
            menu.push(c.btn(if web { "Copy link" } else { "Copy path" }, Act::new("att-copy", "", &aid, &fk), Look::Menu, false, None));
            menu.push(c.btn("Edit", Act::new("att-edit", "", &aid, &fk), Look::Menu, false, None));
            menu.push(c.btn("Remove", Act::new("att-remove", "", &aid, &fk), Look::Menu, false, None));
            kids.push(el(K::AttMenu, menu));
        }
        out.push(el(K::Att, kids));
    }
    out
}

/// `attEditHtml(a)`.
fn att_edit(c: &Ctx, a: &Value) -> Node {
    let aid = or_empty(&a["id"]);
    let k = format!("attedit:{aid}");
    let kind = c.ui.drafts.get(&format!("{k}:kind")).cloned().or(opt_s(a, "kind").map(str::to_string)).unwrap_or_else(|| "other".into());
    let seg = ATT_KINDS.iter().map(|id| c.btn(att_kind_label(id), Act::new("att-ekind", *id, &aid, ""), Look::Tab { on: kind == *id }, false, None)).collect();
    let title = c.ui.drafts.get(&format!("{k}:title")).cloned().unwrap_or_else(|| or_empty(&a["title"]));
    let url = c.ui.drafts.get(&format!("{k}:url")).cloned().unwrap_or_else(|| or_empty(&a["url"]));
    let mut kids = vec![
        el(K::Seg, seg),
        Node::Field { key: format!("{k}:title"), value: title, placeholder: "Title", multi: false, mono: false },
        Node::Field { key: format!("{k}:url"), value: url, placeholder: "https://… or /path/to/file", multi: false, mono: true },
        el(
            K::Row,
            vec![
                c.btn("Save", Act::new("att-esave", "", &aid, &k), Look::SoftSmall, false, None),
                c.btn("Cancel", Act::new("att-ecancel", "", &aid, ""), Look::Ghost, false, None),
            ],
        ),
    ];
    kids.extend(c.note(&k));
    el(K::AttEdit, kids)
}

/// `attachRow(t)`.
fn attach_row(c: &Ctx, t: &Value) -> Node {
    let r = rf(t, "T");
    let list: Vec<Value> = arr(t, "attachments").iter().chain(arr(t, "goal_attachments")).cloned().collect();
    let fk = format!("att:tasks:{r}");
    let items = attach_items(c, &list, "tasks", &r);
    let mut head = vec![txt("Attached", St::Strong)];
    if !list.is_empty() {
        head.push(txt(list.len().to_string(), St::Count));
    }
    let mut kids = vec![el(K::Row, head)];
    if items.is_empty() {
        kids.push(txt("Nothing yet. Designs, proposals, docs and results go here.", St::Small));
    } else {
        kids.push(el(K::Atts, items));
    }
    kids.extend(c.note(&fk));
    el(K::Lsec, kids)
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

/// The devices it has (or asks for) and its bits, as Context rows. A backend bit not made yet has
/// "Mark created" (the owner's word that it's made in the flag tool, as on the goal page).
fn pool_rows(c: &Ctx, t: &Value) -> Vec<Node> {
    let r = rf(t, "T");
    let mut out = vec![];
    let d = &t["devices"];
    let lent: Vec<&str> = arr(d, "lent").iter().filter_map(|x| x.as_str()).collect();
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
fn log_group(kind: &str) -> &'static str {
    match kind {
        "status" | "midna" | "handoff" => "status",
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
        k => fmt::cap(k),
    }
}

/// `LOG_DOT`.
fn log_dot(kind: &str) -> Dot {
    match kind {
        "status" | "checkpoint" => Dot::Accent,
        "question" | "answer" | "found" => Dot::Warn,
        "jira" => Dot::AccentFg,
        "commit" => Dot::Up,
        "handoff" => Dot::Goal,
        _ => Dot::Faint,
    }
}

/// `logTab(t)`.
fn log_tab(c: &Ctx, t: &Value) -> Vec<Node> {
    let f = c.ui.log_filter();
    let all = arr(t, "log");
    let list: Vec<&Value> = all.iter().filter(|e| f == "all" || log_group(s(e, "kind")) == f).collect();
    let chips = LOG_FILTERS.iter().map(|(id, label)| c.btn(label, Act::new("log-filter", *id, "", ""), Look::Tab { on: f == *id }, false, None)).collect();
    let mut out = vec![el(K::Chips, chips)];
    if list.is_empty() {
        out.push(txt(if all.is_empty() { "Nothing logged yet." } else { "Nothing of this kind yet." }, St::Empty));
    } else {
        let items = list
            .iter()
            .map(|e| {
                let at = s(e, "at");
                let kind = s(e, "kind");
                el(
                    K::LogItem,
                    vec![
                        txt_tip(fmt::hhmm(at), St::Time, fmt::full_time(at)),
                        Node::Dot(log_dot(kind)),
                        txt(opt_s(e, "who").unwrap_or("Task board"), St::Strong),
                        txt(log_label(kind), St::Kind),
                        txt(or_empty(&e["text"]), St::Plain),
                    ],
                )
            })
            .collect();
        out.push(el(K::Log, items));
    }
    out.push(txt("The log is saved as it happens, so it survives a closed terminal or a restart.", St::Help));
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

/// `inlineText(s)`: text with links and `code` (paths count as code).
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
                b'u' => Node::Link { s: text.clone(), go: Go::Url(text), tip: None, look: LinkLook::Plain },
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

/// `noteBody(text)`: paragraphs (lines joined by line breaks), `- ` lists and indented code.
pub fn note_body(text: &str) -> Node {
    let mut out = Vec::new();
    let mut kind: Option<&str> = None;
    let mut buf: Vec<String> = Vec::new();
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
    let (kids, closed): (&[Node], bool) = match n {
        Node::El { k: K::Fold(_, open), kids, .. } => (kids, !open),
        Node::El { kids, .. } => (kids, false),
        _ => (&[], false),
    };
    kids.iter().take(if closed { 1 } else { usize::MAX })
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
