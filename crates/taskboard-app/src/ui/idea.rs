//! An idea's page (`GET /ideas/:id`): its ref, project, state and verdict, its detail and notes,
//! and its report rounds newest first.
//!
//! Its actions are Dig (`POST /ideas/:id/dig`: Claude reads the project's code and reports), Plan
//! (`POST /ideas/:id/plan`: Claude makes a planned goal from it) and Close (`POST
//! /ideas/:id/shelve|drop`, or `reopen` once closed). Dig and Plan are start signals: the page
//! shows the dig's progress and links to the goal once planned. Closing offers Undo, which reopens
//! it. Talk it over (`POST /ideas/:id/talk`, in the header and on each question) opens a Claude
//! terminal in the project's checkout to talk it over in; what's settled there comes back as notes.
//! Nothing else here edits the idea: `tb idea` does.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, b, s};
use crate::theme::Theme;
use crate::ui::ideas::{state_pill, verdict_pill};
use crate::ui::releases::cut_at;
use crate::ui::select::sel;
use crate::ui::{goal, kit, md};
use chrono::{DateTime, Utc};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

/// How much of the detail the page renders.
const DETAIL_MAX: usize = 20_000;
const CLOSE_MENU: &str = "idea-close";

#[derive(Default)]
pub struct State {
    /// The requests running, by `dig::I3` / `plan::I3` / `close::I3` / `talk::I3`, and
    /// `talk::I3::<question>` for a question's Talk it over.
    busy: HashSet<String>,
    /// Rounds opened or closed by hand, by `I3::<round id>`; the rest follow [`starts_open`].
    open: HashMap<String, bool>,
    /// Points showing their detail, by `I3::<round id>::<block>::<item>`.
    more: HashSet<String>,
}

/// The newest round starts open, the older ones collapsed.
pub fn starts_open(n: usize) -> bool {
    n == 0
}

fn round_open(m: &MainWindow, key: &str, n: usize) -> bool {
    m.idea.open.get(key).copied().unwrap_or(starts_open(n))
}

/// Open an idea's page.
pub fn open(m: &mut MainWindow, r: &str, cx: &mut Context<MainWindow>) {
    m.go(Page::Idea(r.to_string()), cx);
    cx.notify();
}

// ================================================================== view models

/// A header button: its label, whether it can be pressed, and why not.
#[derive(Clone, Debug, PartialEq)]
pub struct Btn {
    pub label: String,
    pub enabled: bool,
    pub tip: String,
}

/// The Plan slot: a button, or the goal the idea became.
#[derive(Clone, Debug, PartialEq)]
pub enum PlanSlot {
    Button(Btn),
    Goal(String),
}

/// The round a dig is still writing: the newest, unfinished and without an error.
fn open_round(i: &Value) -> Option<&Value> {
    arr(i, "rounds").first().filter(|r| r["finished_at"].is_null() && r["error"].is_null())
}

/// "Digging…", then "Digging… 4m" once it has run a minute.
fn digging_label(started: &str, now: DateTime<Utc>) -> String {
    match fmt::parse(started).map(|t| (now - t).num_minutes()) {
        Some(m) if m >= 1 => format!("Digging… {}", fmt::span(m)),
        _ => "Digging…".into(),
    }
}

/// Why neither button works on an idea in this state, if one doesn't.
fn closed_reason(i: &Value) -> Option<String> {
    if b(i, "planning") {
        return Some("A plan is being made".into());
    }
    match s(i, "state") {
        "planned" => Some(format!("It became {}", fmt::opt_s(i, "goal").unwrap_or("a goal"))),
        "shelved" => Some("It’s shelved; reopen it first".into()),
        "dropped" => Some("It’s closed as won’t do; reopen it first".into()),
        _ => None,
    }
}

pub fn dig_button(i: &Value, now: DateTime<Utc>) -> Btn {
    if s(i, "state") == "digging" {
        let started = open_round(i).map(|r| s(r, "started_at")).unwrap_or("");
        return Btn { label: digging_label(started, now), enabled: false, tip: "Claude is reading the code".into() };
    }
    let label = if arr(i, "rounds").iter().any(|r| r["report"].is_object()) { "Dig again" } else { "Dig" }.to_string();
    match closed_reason(i) {
        Some(why) => Btn { label, enabled: false, tip: why },
        None => Btn { label, enabled: true, tip: "Claude reads the project’s code and reports how the idea fits".into() },
    }
}

pub fn plan_slot(i: &Value) -> PlanSlot {
    if let Some(g) = fmt::opt_s(i, "goal").filter(|g| !g.is_empty() && s(i, "state") == "planned") {
        return PlanSlot::Goal(g.to_string());
    }
    if b(i, "planning") {
        return PlanSlot::Button(Btn { label: "Planning…".into(), enabled: false, tip: "Claude is making a goal from it".into() });
    }
    let label = "Plan".to_string();
    if s(i, "state") == "digging" {
        return PlanSlot::Button(Btn { label, enabled: false, tip: "Wait for the dig to finish".into() });
    }
    match closed_reason(i) {
        Some(why) => PlanSlot::Button(Btn { label, enabled: false, tip: why }),
        None => PlanSlot::Button(Btn { label, enabled: true, tip: "Claude makes a planned goal from it and its reports, not started".into() }),
    }
}

fn is_closed(i: &Value) -> bool {
    matches!(s(i, "state"), "shelved" | "dropped")
}

/// The Close button: "Close", or the state it was closed as. None once planned: the goal link
/// stands in.
pub fn close_button(i: &Value) -> Option<Btn> {
    if s(i, "state") == "planned" {
        return None;
    }
    if is_closed(i) {
        let (label, _) = state_pill(i);
        return Some(Btn { label: label.into(), enabled: true, tip: "Reopen it".into() });
    }
    let label = "Close".to_string();
    Some(if b(i, "planning") {
        Btn { label, enabled: false, tip: "A plan is being made".into() }
    } else if s(i, "state") == "digging" {
        Btn { label, enabled: false, tip: "Wait for the dig to finish".into() }
    } else {
        Btn { label, enabled: true, tip: "Shelve it for later or close it as won’t do".into() }
    })
}

/// Talk it over: open whatever state it's in, as long as it hasn't become a goal.
pub fn talk_button(i: &Value) -> Option<Btn> {
    (s(i, "state") != "planned").then(|| Btn { label: "Talk it over".into(), enabled: true, tip: "Talk it over with Claude in a terminal in the project’s checkout".into() })
}

/// The Close menu's items: (what it posts, its label).
pub fn close_menu_view(i: &Value) -> Vec<(&'static str, &'static str)> {
    if is_closed(i) { vec![("reopen", "Reopen")] } else { vec![("shelve", "Shelve for later"), ("drop", "Won’t do")] }
}

/// A point: its short line, the detail it opens to, and its kind (security, privacy, …).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Point {
    pub short: String,
    pub detail: String,
    pub tag: Option<String>,
}

/// One option of a comparison.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Opt {
    pub name: String,
    pub pros: Vec<String>,
    pub cons: Vec<String>,
}

/// A report block as the page draws it. Blocks of a type the page doesn't know are left out.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Gist(String),
    /// (label, value, tone) per cell: Fit, then Size and Risk when set.
    Scorecard(Vec<(&'static str, &'static str, &'static str)>),
    /// (step, whether it's new).
    Flow(Vec<(String, bool)>),
    /// (what, its files): "Builds on".
    Code(Vec<(String, Vec<String>)>),
    Points { title: String, items: Vec<Point> },
    Question { text: String, choices: Vec<String> },
    Compare { lean: String, options: Vec<Opt> },
    Related(Vec<String>),
}

/// One report round as the page shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoundView {
    /// The round's id, which keys its open state.
    pub id: String,
    /// "Round 2": counted from the first.
    pub title: String,
    pub focus: String,
    pub verdict: Option<(&'static str, &'static str)>,
    /// The report's one line: its gist, else its why.
    pub gist: String,
    pub stale: bool,
    pub when: String,
    /// "Digging… 4m" while it runs.
    pub running: Option<String>,
    pub error: Option<String>,
    /// It read the web too.
    pub web: bool,
    pub blocks: Vec<Block>,
}

impl RoundView {
    /// The line beside the title: the focus while open; collapsed, "focus — gist".
    pub fn line(&self, open: bool) -> String {
        let tail = if self.gist.is_empty() { self.error.clone().unwrap_or_default() } else { self.gist.clone() };
        if open || tail.is_empty() {
            return self.focus.clone();
        }
        if self.focus.is_empty() { tail } else { format!("{} — {tail}", self.focus) }
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_string).collect()).unwrap_or_default()
}

fn scorecard(b: &Value) -> Block {
    let mut cells = vec![];
    let fit = match s(b, "fit") {
        "fits" => Some(("Fits", "")),
        "partly" => Some(("Partly", "warn")),
        "doesnt_fit" => Some(("Doesn’t fit", "down")),
        _ => None,
    };
    let size = match s(b, "size") {
        "s" => Some(("Small", "")),
        "m" => Some(("Medium", "")),
        "l" => Some(("Large", "")),
        _ => None,
    };
    let risk = match s(b, "risk") {
        "low" => Some(("Low", "")),
        "medium" => Some(("Medium", "warn")),
        "high" => Some(("High", "down")),
        _ => None,
    };
    for (label, cell) in [("Fit", fit), ("Size", size), ("Risk", risk)] {
        if let Some((value, tone)) = cell {
            cells.push((label, value, tone));
        }
    }
    Block::Scorecard(cells)
}

fn points(title: &str, items: Vec<Point>) -> Option<Block> {
    (!items.is_empty()).then(|| Block::Points { title: title.to_string(), items })
}

fn block(v: &Value) -> Option<Block> {
    let named = |list: &[Value], k: &str| list.iter().filter(|x| !s(x, k).is_empty()).cloned().collect::<Vec<_>>();
    let block = match s(v, "type") {
        "gist" => Block::Gist(s(v, "text").trim().to_string()),
        "scorecard" => scorecard(v),
        "flow" => Block::Flow(named(arr(v, "steps"), "name").iter().map(|x| (s(x, "name").to_string(), b(x, "new"))).collect()),
        "code" => Block::Code(named(arr(v, "items"), "what").iter().map(|x| (s(x, "what").to_string(), fmt::opt_s(x, "file").filter(|f| !f.is_empty()).map(str::to_string).into_iter().collect())).collect()),
        "points" => Block::Points {
            title: s(v, "title").to_string(),
            items: named(arr(v, "items"), "short")
                .iter()
                .map(|x| Point { short: s(x, "short").to_string(), detail: s(x, "detail").trim().to_string(), tag: fmt::opt_s(x, "tag").map(str::to_string) })
                .collect(),
        },
        "question" => Block::Question { text: s(v, "text").trim().to_string(), choices: strings(&v["choices"]) },
        "compare" => Block::Compare {
            lean: s(v, "lean").to_string(),
            options: named(arr(v, "options"), "name").iter().map(|x| Opt { name: s(x, "name").to_string(), pros: strings(&x["pros"]), cons: strings(&x["cons"]) }).collect(),
        },
        "related" => Block::Related(strings(&v["refs"])),
        _ => return None,
    };
    let empty = match &block {
        Block::Gist(x) | Block::Question { text: x, .. } => x.is_empty(),
        Block::Scorecard(x) => x.is_empty(),
        Block::Flow(x) => x.is_empty(),
        Block::Code(x) => x.is_empty(),
        Block::Points { items, .. } => items.is_empty(),
        Block::Compare { options, .. } => options.is_empty(),
        Block::Related(x) => x.is_empty(),
    };
    (!empty).then_some(block)
}

/// A report from before blocks: its lists drawn as the blocks that hold the same things.
fn old_blocks(rep: &Value) -> Vec<Block> {
    let mut out = vec![];
    let code: Vec<_> = arr(rep, "builds_on").iter().filter(|x| !s(x, "what").is_empty()).map(|x| (s(x, "what").to_string(), strings(&x["files"]))).collect();
    if !code.is_empty() {
        out.push(Block::Code(code));
    }
    let short = |x: String| Point { short: x, ..Point::default() };
    let mut watch: Vec<Point> = arr(rep, "risks")
        .iter()
        .filter(|x| !s(x, "risk").is_empty())
        .map(|x| Point { short: s(x, "risk").to_string(), detail: String::new(), tag: fmt::opt_s(x, "kind").filter(|k| !k.is_empty()).map(str::to_string) })
        .collect();
    watch.extend(strings(&rep["pitfalls"]).into_iter().map(short));
    out.extend(points("Watch out", watch));
    out.extend(points("Could grow into", strings(&rep["ways_to_extend"]).into_iter().map(short).collect()));
    out.extend(strings(&rep["open_questions"]).into_iter().map(|text| Block::Question { text, choices: vec![] }));
    let related = strings(&rep["related"]);
    if !related.is_empty() {
        out.push(Block::Related(related));
    }
    out
}

/// A report's blocks, its gist first: the why stands in when it has none.
fn report_blocks(rep: &Value) -> Vec<Block> {
    let mut out: Vec<Block> = match rep["blocks"].as_array() {
        Some(list) => list.iter().filter_map(block).collect(),
        None => old_blocks(rep),
    };
    if !out.iter().any(|x| matches!(x, Block::Gist(_))) {
        let why = s(rep, "why").trim();
        if !why.is_empty() {
            out.insert(0, Block::Gist(why.to_string()));
        }
    }
    out
}

pub fn rounds(i: &Value, now: DateTime<Utc>) -> Vec<RoundView> {
    let all = arr(i, "rounds");
    let open = open_round(i).map(|r| r["id"].clone());
    all.iter()
        .enumerate()
        .map(|(n, r)| {
            let rep = &r["report"];
            let at = fmt::opt_s(r, "finished_at").or(fmt::opt_s(r, "started_at")).unwrap_or("");
            let blocks = if rep.is_object() { report_blocks(rep) } else { vec![] };
            let gist = blocks.iter().find_map(|x| if let Block::Gist(g) = x { Some(g.clone()) } else { None }).unwrap_or_default();
            RoundView {
                id: r["id"].to_string(),
                title: format!("Round {}", all.len() - n),
                focus: s(r, "focus").to_string(),
                verdict: verdict_pill(&rep["verdict"]),
                gist,
                stale: b(r, "stale"),
                when: fmt::parse(at).map(|t| cut_at(t, now)).unwrap_or_default(),
                running: (open.as_ref() == Some(&r["id"])).then(|| digging_label(s(r, "started_at"), now)),
                error: fmt::opt_s(r, "error").filter(|e| !e.is_empty()).map(str::to_string),
                web: b(r, "web"),
                blocks,
            }
        })
        .collect()
}

/// A file chip's name: the file's last part without its line.
pub fn file_name(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.split(':').next().unwrap_or(name).to_string()
}

/// A note's byline: who left it and when.
pub fn note_byline(n: &Value, now: DateTime<Utc>) -> String {
    let when = fmt::parse(s(n, "at")).map(|t| cut_at(t, now)).unwrap_or_default();
    [s(n, "source"), when.as_str()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")
}

// ================================================================== requests

/// Dig or Plan: a start signal, once at a time; the board's answer shows on the next refresh.
pub(crate) fn start(m: &mut MainWindow, act: &'static str, r: &str, cx: &mut Context<MainWindow>) {
    let key = format!("{act}::{r}");
    if !m.idea.busy.insert(key.clone()) {
        return;
    }
    let k2 = key.clone();
    m.post_or(
        format!("ideas/{r}/{act}"),
        json!({}),
        cx,
        move |m, _, _| {
            m.idea.busy.remove(&key);
        },
        move |m, e, cx| {
            m.idea.busy.remove(&k2);
            m.toast(e.message, true, cx);
        },
    );
    cx.notify();
}

/// Shelve, drop or reopen it, once at a time. Closing it offers Undo, which reopens it.
pub(crate) fn close(m: &mut MainWindow, act: &'static str, r: &str, cx: &mut Context<MainWindow>) {
    m.menu = None;
    let key = format!("close::{r}");
    if !m.idea.busy.insert(key.clone()) {
        return;
    }
    let k2 = key.clone();
    let r = r.to_string();
    m.post_or(
        format!("ideas/{r}/{act}"),
        json!({}),
        cx,
        move |m, _, cx| {
            m.idea.busy.remove(&key);
            match act {
                "shelve" => m.toast_undo(format!("{r} shelved"), format!("ideas/{r}/reopen"), cx),
                "drop" => m.toast_undo(format!("{r} closed as won’t do"), format!("ideas/{r}/reopen"), cx),
                _ => m.toast(format!("{r} reopened"), false, cx),
            }
        },
        move |m, e, cx| {
            m.idea.busy.remove(&k2);
            m.toast(e.message, true, cx);
        },
    );
    cx.notify();
}

/// Talk it over, about one of its reports' questions when `question` is given: once at a time, the
/// terminal opens on its own.
pub(crate) fn talk(m: &mut MainWindow, r: &str, question: Option<&str>, cx: &mut Context<MainWindow>) {
    let key = match question {
        Some(q) => format!("talk::{r}::{q}"),
        None => format!("talk::{r}"),
    };
    if !m.idea.busy.insert(key.clone()) {
        return;
    }
    let k2 = key.clone();
    let body = question.map_or(json!({}), |q| json!({"question": q}));
    m.post_or(
        format!("ideas/{r}/talk"),
        body,
        cx,
        move |m, _, _| {
            m.idea.busy.remove(&key);
        },
        move |m, e, cx| {
            m.idea.busy.remove(&k2);
            m.toast(e.message, true, cx);
        },
    );
    cx.notify();
}

// ================================================================== render

/// Ideas › webapp › I3: the project crumb opens the Ideas page on that project.
fn crumbs(t: &Theme, i: &Value, cx: &mut Context<MainWindow>) -> Div {
    let hover = t.text;
    let link = |id: &'static str, text: String| div().id(id).cursor_pointer().hover(move |s| s.text_color(hover)).child(text);
    let sep = || div().text_color(t.faint).child("›");
    let project = s(i, "project").to_string();
    let open = project.clone();
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(12.))
        .text_color(t.muted)
        .child(link("idea-crumb", "Ideas".into()).on_click(cx.listener(|m, _, _, cx| open_ideas(m, "all", cx))))
        .child(sep())
        .child(link("idea-crumb-project", project).on_click(cx.listener(move |m, _, _, cx| open_ideas(m, &open, cx))))
        .child(sep())
        .child(div().font_family(t.mono_font.clone()).child(fmt::ref_of(i, "I")))
}

fn open_ideas(m: &mut MainWindow, project: &str, cx: &mut Context<MainWindow>) {
    m.ideas.project = project.to_string();
    m.go(Page::Ideas, cx);
}

fn button(t: &Theme, id: &'static str, bt: Btn, primary: bool, busy: bool) -> Stateful<Div> {
    let make = if primary { kit::btn_primary } else { kit::btn };
    let label = if busy { "Sending…".to_string() } else { bt.label };
    let el = make(t, id, label).tooltip(kit::tip(bt.tip));
    if bt.enabled && !busy { el } else { kit::disabled(el) }
}

fn buttons(m: &MainWindow, t: &Theme, i: &Value, cx: &mut Context<MainWindow>) -> Div {
    let r = fmt::ref_of(i, "I");
    let dig = dig_button(i, fmt::now());
    let dig_busy = m.idea.busy.contains(&format!("dig::{r}"));
    let can_dig = dig.enabled && !dig_busy;
    let r1 = r.clone();
    let dig_el = button(t, "idea-dig", dig, true, dig_busy).when(can_dig, |d| d.on_click(cx.listener(move |m, _, _, cx| start(m, "dig", &r1, cx))));
    let plan_el = match plan_slot(i) {
        PlanSlot::Goal(g) => {
            let g2 = g.clone();
            kit::btn(t, "idea-goal", format!("{g} →")).tooltip(kit::tip("Open the goal it became")).on_click(cx.listener(move |m, _, _, cx| goal::open(m, &g2, false, cx)))
        }
        PlanSlot::Button(bt) => {
            let busy = m.idea.busy.contains(&format!("plan::{r}"));
            let can = bt.enabled && !busy;
            let r2 = r.clone();
            button(t, "idea-plan", bt, false, busy).when(can, |d| d.on_click(cx.listener(move |m, _, _, cx| start(m, "plan", &r2, cx))))
        }
    };
    let close_el = close_button(i).map(|bt| {
        let busy = m.idea.busy.contains(&format!("close::{r}"));
        let can = bt.enabled && !busy;
        let bt = Btn { label: format!("{} ▾", bt.label), ..bt };
        button(t, "idea-close", bt, false, busy).when(can, |d| {
            d.on_click(cx.listener(|m, e: &ClickEvent, _, cx| {
                let p = e.position();
                m.toggle_menu(CLOSE_MENU, point(p.x - px(160.), p.y + px(14.)), cx);
            }))
        })
    });
    let talk_el = talk_button(i).map(|bt| {
        let busy = m.idea.busy.contains(&format!("talk::{r}"));
        let r3 = r.clone();
        button(t, "idea-talk", bt, false, busy).when(!busy, |d| d.on_click(cx.listener(move |m, _, _, cx| talk(m, &r3, None, cx))))
    });
    div().flex().flex_none().items_center().gap(px(8.)).child(dig_el).child(plan_el).children(talk_el).children(close_el)
}

fn close_menu(m: &MainWindow, t: &Theme, i: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let at = m.menu_open(CLOSE_MENU)?;
    let r = fmt::ref_of(i, "I");
    let mut menu = kit::menu_box(t, 200.).id("idea-close-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for (act, label) in close_menu_view(i) {
        let r = r.clone();
        menu = menu.child(kit::menu_item(t, SharedString::from(format!("idea-{act}")), label, false).on_click(cx.listener(move |m, _, _, cx| close(m, act, &r, cx))));
    }
    Some(kit::popover(at, menu))
}

fn header(m: &MainWindow, t: &Theme, i: &Value, cx: &mut Context<MainWindow>) -> Div {
    let (state, tone) = state_pill(i);
    let project = div().px(px(7.)).py(px(1.)).rounded(px(9.)).bg(t.panel_2).text_size(px(11.)).text_color(t.muted).child(s(i, "project").to_string());
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
                        .child(div().text_size(px(12.)).font_family(t.mono_font.clone()).text_color(t.muted).child(sel(t, "idea-ref", fmt::ref_of(i, "I"))))
                        .child(project)
                        .child(kit::tone_pill(t, tone, state))
                        .children(verdict_pill(&i["verdict"]).map(|(label, tone)| kit::tone_pill(t, tone, label))),
                )
                .child(div().text_size(px(26.)).font_weight(FontWeight::BOLD).line_height(relative(1.2)).child(sel(t, "idea-title", s(i, "title").to_string()))),
        )
        .child(buttons(m, t, i, cx))
}

fn section(t: &Theme, title: &str) -> Div {
    div().flex().flex_col().gap(px(8.)).child(kit::h3(t, title.to_string()))
}

fn notes(t: &Theme, i: &Value) -> Option<Div> {
    let list = arr(i, "notes");
    if list.is_empty() {
        return None;
    }
    let now = fmt::now();
    let mut card = kit::card(t).flex().flex_col();
    for (n, note) in list.iter().enumerate() {
        card = card.child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(px(16.))
                .py(px(10.))
                .when(n > 0, |d| d.border_t_1().border_color(t.divider))
                .child(div().text_size(px(13.5)).text_color(t.text).child(sel(t, SharedString::from(format!("idea-note-{n}")), s(note, "text").to_string())))
                .child(div().text_size(px(11.5)).text_color(t.muted).child(note_byline(note, now))),
        );
    }
    Some(section(t, "Notes").child(card))
}

/// `.h3` inside a report: 12px bold.
fn label(t: &Theme, text: &str) -> Div {
    div().text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(t.text_2).child(text.to_string())
}

fn related_link(t: &Theme, key: &str, r: &str, cx: &mut Context<MainWindow>) -> AnyElement {
    let id = SharedString::from(format!("idea-related-{key}-{r}"));
    let el = kit::link(t, id, r.to_string()).font_family(t.mono_font.clone()).text_size(px(12.));
    let r = r.to_string();
    match r.chars().next() {
        Some('G') => el.text_color(t.goal).on_click(cx.listener(move |m, _, _, cx| goal::open(m, &r, false, cx))).into_any_element(),
        Some('T') => el.on_click(cx.listener(move |m, _, _, cx| m.open_task(r.clone(), cx))).into_any_element(),
        Some('B') => el.on_click(cx.listener(move |m, _, _, cx| m.open_issue(r.clone(), cx))).into_any_element(),
        Some('I') => el.on_click(cx.listener(move |m, _, _, cx| open(m, &r, cx))).into_any_element(),
        _ => kit::mono(t, r).text_color(t.muted).into_any_element(),
    }
}

fn tone_color(t: &Theme, tone: &str) -> Hsla {
    match tone {
        "warn" => t.warn_fg,
        "down" => t.down,
        _ => t.text,
    }
}

fn scorecard_el(t: &Theme, cells: &[(&'static str, &'static str, &'static str)]) -> Div {
    div().flex().gap(px(8.)).max_w(px(520.)).children(cells.iter().map(|(label, value, tone)| {
        div()
            .flex_1()
            .min_w_0()
            .px(px(12.))
            .py(px(8.))
            .rounded(px(8.))
            .border_1()
            .border_color(t.border)
            .child(div().text_size(px(11.)).font_weight(FontWeight::MEDIUM).text_color(t.muted).child(*label))
            .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(tone_color(t, tone)).child(*value))
    }))
}

/// The steps in order: the first in the accent, new ones dashed.
fn flow_el(t: &Theme, steps: &[(String, bool)]) -> Div {
    let mut row = div().flex().flex_wrap().items_center().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD);
    for (n, (name, new)) in steps.iter().enumerate() {
        if n > 0 {
            row = row.child(div().px(px(6.)).text_color(t.muted).child("→"));
        }
        let step = div().px(px(10.)).py(px(6.)).rounded(px(7.)).border_1();
        row = row.child(if *new {
            step.border_dashed().border_color(t.warn_line).bg(t.warn_soft).text_color(t.warn_fg).child(format!("{name} (new)"))
        } else if n == 0 {
            step.border_color(t.accent_line).bg(t.accent_soft).text_color(t.accent_fg).child(name.clone())
        } else {
            step.border_color(t.border).bg(t.panel_2).text_color(t.text).child(name.clone())
        });
    }
    row
}

/// "Builds on": what, with its file's name; the whole path shows on hover.
fn code_el(t: &Theme, key: &str, items: &[(String, Vec<String>)]) -> Div {
    let chips = items.iter().enumerate().map(|(n, (what, files))| {
        let chip = div()
            .id(SharedString::from(format!("idea-code-{key}-{n}")))
            .flex()
            .items_baseline()
            .gap(px(6.))
            .px(px(9.))
            .py(px(4.))
            .rounded(px(7.))
            .bg(t.panel_2)
            .text_size(px(12.5))
            .child(what.clone())
            .children(files.first().map(|f| div().font_family(t.mono_font.clone()).text_size(px(11.)).text_color(t.muted).child(file_name(f))));
        if files.is_empty() { chip } else { chip.tooltip(kit::tip(files.join("\n"))) }
    });
    div().flex().flex_col().gap(px(6.)).child(label(t, "Builds on")).child(div().flex().flex_wrap().gap(px(6.)).children(chips))
}

fn tag_pill(t: &Theme, tag: &str) -> Div {
    let tone = if tag == "security" || tag == "privacy" { "down" } else { "warn" };
    let (fg, bg) = kit::tone(t, tone);
    div().flex_none().mt(px(2.)).px(px(7.)).rounded(px(8.)).bg(bg).text_color(fg).text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).line_height(px(18.)).child(tag.to_string())
}

/// A points list: each point's short line, opening to its detail with More.
fn points_el(m: &MainWindow, t: &Theme, key: &str, title: &str, items: &[Point], cx: &mut Context<MainWindow>) -> Div {
    let mut col = div().flex().flex_col().flex_1().min_w_0().gap(px(6.)).when(!title.is_empty(), |d| d.child(label(t, title)));
    for (n, p) in items.iter().enumerate() {
        let id = format!("{key}::{n}");
        let open = m.idea.more.contains(&id);
        let mark = match &p.tag {
            Some(tag) => tag_pill(t, tag),
            None => div().flex_none().text_color(t.muted).child("•"),
        };
        let more = !p.detail.is_empty();
        let line = div()
            .id(SharedString::from(format!("idea-point-{id}")))
            .flex()
            .items_start()
            .gap(px(8.))
            .text_size(px(13.5))
            .line_height(relative(1.45))
            .child(mark)
            .child(div().flex_1().min_w_0().child(p.short.clone()))
            .when(more, |d| {
                let id = id.clone();
                d.cursor_pointer()
                    .child(div().flex_none().text_size(px(12.)).text_color(t.muted).child(if open { "Less" } else { "More" }))
                    .on_click(cx.listener(move |m, _, _, cx| {
                        if !m.idea.more.remove(&id) {
                            m.idea.more.insert(id.clone());
                        }
                        cx.notify();
                    }))
            });
        col = col.child(
            div().flex().flex_col().gap(px(4.)).child(line).when(open && more, |d| {
                d.child(
                    div()
                        .mb(px(4.))
                        .pl(px(10.))
                        .border_l_2()
                        .border_color(t.border)
                        .text_size(px(13.))
                        .line_height(relative(1.5))
                        .text_color(t.text_2)
                        .child(sel(t, SharedString::from(format!("idea-detail-{id}")), p.detail.clone())),
                )
            }),
        );
    }
    col
}

/// "To decide": each question with its choices, and Talk it over on each while `talk` is the idea.
/// Answering them is still `tb idea`'s.
fn questions_el(m: &MainWindow, t: &Theme, key: &str, talk: Option<&str>, qs: &[(&String, &Vec<String>)], cx: &mut Context<MainWindow>) -> Div {
    let cards = qs.iter().enumerate().map(|(n, (text, choices))| {
        let talk_el = talk.map(|r| {
            let busy = m.idea.busy.contains(&format!("talk::{r}::{text}"));
            let (r, q) = (r.to_string(), (*text).clone());
            let el = kit::btn_small(t, SharedString::from(format!("idea-talk-{key}::{n}")), if busy { "Sending…" } else { "Talk it over" })
                .tooltip(kit::tip("Talk this one over with Claude in a terminal"));
            if busy { kit::disabled(el) } else { el.on_click(cx.listener(move |m, _, _, cx| self::talk(m, &r, Some(&q), cx))) }
        });
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(14.))
            .py(px(12.))
            .rounded(px(9.))
            .border_1()
            .border_color(t.border)
            .bg(t.panel_2)
            .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child((*text).clone()))
            .when(!choices.is_empty(), |d| {
                d.child(div().flex().flex_wrap().gap(px(6.)).children(choices.iter().map(|c| {
                    div().flex().items_center().min_h(px(32.)).px(px(12.)).rounded(px(16.)).border_1().border_color(t.border_2).bg(t.card).text_size(px(13.)).child(c.clone())
                })))
            })
            .children(talk_el.map(|b| div().flex().child(b)))
    });
    div().flex().flex_col().gap(px(8.)).child(label(t, "To decide")).children(cards)
}

/// Options side by side, the one it leans to outlined in the accent.
fn compare_el(t: &Theme, lean: &str, options: &[Opt]) -> Div {
    div().flex().gap(px(8.)).children(options.iter().map(|o| {
        let leaning = !lean.is_empty() && o.name == lean;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(6.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(9.))
            .border_1()
            .border_color(if leaning { t.accent } else { t.border })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().text_size(px(13.5)).font_weight(FontWeight::BOLD).child(o.name.clone()))
                    .when(leaning, |d| d.child(kit::tone_pill(t, "accent", "Leaning"))),
            )
            .children(o.pros.iter().map(|x| div().text_size(px(12.5)).text_color(t.up_fg).child(format!("+ {x}"))))
            .children(o.cons.iter().map(|x| div().text_size(px(12.5)).text_color(t.warn_fg).child(format!("− {x}"))))
    }))
}

fn related_el(t: &Theme, key: &str, refs: &[String], cx: &mut Context<MainWindow>) -> Div {
    div().flex().flex_wrap().items_center().gap(px(10.)).text_size(px(12.)).child(label(t, "Related")).children(refs.iter().map(|r| related_link(t, key, r, cx)))
}

/// A round's blocks in order: points lists side by side in twos, questions under one heading (with
/// Talk it over while `talk` is the idea).
fn blocks_el(m: &MainWindow, t: &Theme, key: &str, talk: Option<&str>, blocks: &[Block], cx: &mut Context<MainWindow>) -> Vec<AnyElement> {
    let mut out = vec![];
    let mut k = 0;
    while k < blocks.len() {
        let bkey = format!("{key}::{k}");
        match &blocks[k] {
            Block::Gist(g) => out.push(
                div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).line_height(relative(1.45)).child(sel(t, SharedString::from(format!("idea-gist-{bkey}")), g.clone())).into_any_element(),
            ),
            Block::Scorecard(cells) => out.push(scorecard_el(t, cells).into_any_element()),
            Block::Flow(steps) => out.push(flow_el(t, steps).into_any_element()),
            Block::Code(items) => out.push(code_el(t, &bkey, items).into_any_element()),
            Block::Points { title, items } => {
                let mut row = div().flex().items_start().gap(px(18.)).child(points_el(m, t, &bkey, title, items, cx));
                if let Some(Block::Points { title, items }) = blocks.get(k + 1) {
                    k += 1;
                    row = row.child(points_el(m, t, &format!("{key}::{k}"), title, items, cx));
                }
                out.push(row.into_any_element());
            }
            Block::Question { .. } => {
                let mut qs = vec![];
                while let Some(Block::Question { text, choices }) = blocks.get(k) {
                    qs.push((text, choices));
                    k += 1;
                }
                k -= 1;
                out.push(questions_el(m, t, &bkey, talk, &qs, cx).into_any_element());
            }
            Block::Compare { lean, options } => out.push(compare_el(t, lean, options).into_any_element()),
            Block::Related(refs) => out.push(related_el(t, &bkey, refs, cx).into_any_element()),
        }
        k += 1;
    }
    out
}

/// A round: its header line always, its report while open. `talk`: whether its questions offer Talk it over.
fn round_card(m: &MainWindow, t: &Theme, idea: &str, talk: bool, n: usize, rv: &RoundView, cx: &mut Context<MainWindow>) -> Div {
    let key = format!("{idea}::{}", rv.id);
    let open = round_open(m, &key, n);
    let k2 = key.clone();
    let hover = t.panel_2;
    let chev = div().flex_none().size(px(14.)).child(kit::icon(if open { kit::Icon::Chev } else { kit::Icon::ChevRight }, 14., t.muted));
    let head = div()
        .id(SharedString::from(format!("idea-round-{key}")))
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(16.))
        .py(px(12.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .on_click(cx.listener(move |m, _, _, cx| {
            let was = round_open(m, &k2, n);
            m.idea.open.insert(k2.clone(), !was);
            cx.notify();
        }))
        .child(chev)
        .child(div().flex_none().text_size(px(14.)).font_weight(FontWeight::BOLD).child(rv.title.clone()))
        .children(rv.verdict.map(|(label, tone)| kit::tone_pill(t, tone, label)))
        .children(rv.running.clone().map(|l| kit::tone_pill(t, "accent", l)))
        .when(rv.web, |d| d.child(kit::chip(t, "Web")))
        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(13.)).text_color(t.text_2).child(rv.line(open)))
        .when(rv.stale, |d| d.child(kit::tone_pill(t, "warn", "Stale")))
        .child(div().flex_none().whitespace_nowrap().text_size(px(12.)).text_color(t.muted).child(rv.when.clone()));
    let mut card = kit::card(t).overflow_hidden().child(head);
    if open && (rv.error.is_some() || !rv.blocks.is_empty()) {
        card = card.child(
            div()
                .flex()
                .flex_col()
                .gap(px(18.))
                .pt(px(4.))
                .pr(px(18.))
                .pb(px(18.))
                .pl(px(40.))
                .children(rv.error.clone().map(|e| div().text_size(px(12.5)).text_color(t.down).font_weight(FontWeight::MEDIUM).child(e)))
                .children(blocks_el(m, t, &key, talk.then_some(idea), &rv.blocks, cx)),
        );
    }
    card
}

pub fn render(m: &mut MainWindow, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let want = match &m.page {
        Page::Idea(r) => r.clone(),
        _ => String::new(),
    };
    let Some(i) = m.data.idea.clone().filter(|i| fmt::ref_of(i, "I") == want) else {
        let err = m.data.errs.idea.clone();
        return div()
            .flex_1()
            .pt(px(20.))
            .px(px(36.))
            .child(match err {
                Some(e) => div().text_size(px(12.5)).text_color(t.down).font_weight(FontWeight::MEDIUM).child(e),
                None => kit::help(&t, "Loading…"),
            })
            .into_any_element();
    };
    let detail = s(&i, "detail").trim().to_string();
    let views = rounds(&i, fmt::now());
    let menu = close_menu(m, &t, &i, cx);
    let r = fmt::ref_of(&i, "I");
    let talk = talk_button(&i).is_some();
    let mut reports = section(&t, "Reports");
    for (n, rv) in views.iter().enumerate() {
        reports = reports.child(round_card(m, &t, &r, talk, n, rv, cx));
    }
    div()
        .id("idea-page")
        .flex_1()
        .min_w_0()
        .overflow_y_scroll()
        .line_height(relative(1.5))
        .children(menu)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(20.))
                .max_w(px(920.))
                .pt(px(20.))
                .px(px(36.))
                .pb(px(48.))
                .child(crumbs(&t, &i, cx))
                .child(header(m, &t, &i, cx))
                .when(!detail.is_empty(), |d| d.child(div().text_size(px(14.)).text_color(t.text_2).child(md::render(&t, &detail, DETAIL_MAX, "idea-detail"))))
                .children(notes(&t, &i))
                .when(!views.is_empty(), |d| d.child(reports)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // Beats the glob's `gpui_kit::test`.
    use ::core::prelude::v1::test;
    use chrono::{Duration, TimeZone};

    fn at(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
        fmt::local(Utc::now()).offset().with_ymd_and_hms(y, mo, d, h, 5, 0).unwrap().with_timezone(&Utc)
    }

    fn idea(state: &str, rounds: Value) -> Value {
        json!({"ref": "I3", "project": "web", "state": state, "planning": false, "goal": null, "rounds": rounds, "notes": []})
    }

    #[test]
    fn dig_shows_progress_while_digging_and_is_closed_once_planned() {
        let now = at(2026, 10, 15, 16);
        let open = json!([{"id": 2, "started_at": (now - Duration::minutes(4)).to_rfc3339(), "finished_at": null, "error": null, "report": null}]);
        assert_eq!(dig_button(&idea("digging", open), now), Btn { label: "Digging… 4m".into(), enabled: false, tip: "Claude is reading the code".into() });
        let fresh = json!([{"id": 2, "started_at": now.to_rfc3339(), "finished_at": null, "error": null}]);
        assert_eq!(dig_button(&idea("digging", fresh), now).label, "Digging…");

        let new = dig_button(&idea("new", json!([])), now);
        assert_eq!((new.label.as_str(), new.enabled), ("Dig", true));
        let done = json!([{"id": 1, "report": {"verdict": "fits"}, "finished_at": now.to_rfc3339()}]);
        let again = dig_button(&idea("reported", done), now);
        assert_eq!((again.label.as_str(), again.enabled), ("Dig again", true));

        let mut planned = idea("planned", json!([]));
        planned["goal"] = json!("G5");
        assert_eq!(dig_button(&planned, now), Btn { label: "Dig".into(), enabled: false, tip: "It became G5".into() });
        assert!(!dig_button(&idea("shelved", json!([])), now).enabled);
    }

    #[test]
    fn plan_links_to_the_goal_once_planned() {
        let mut planned = idea("planned", json!([]));
        planned["goal"] = json!("G5");
        assert_eq!(plan_slot(&planned), PlanSlot::Goal("G5".into()));

        let mut planning = idea("reported", json!([]));
        planning["planning"] = json!(true);
        assert_eq!(plan_slot(&planning), PlanSlot::Button(Btn { label: "Planning…".into(), enabled: false, tip: "Claude is making a goal from it".into() }));
        assert!(!dig_button(&planning, Utc::now()).enabled, "no dig while a plan runs");

        let enabled = |state: &str| matches!(plan_slot(&idea(state, json!([]))), PlanSlot::Button(Btn { enabled: true, .. }));
        assert!(enabled("new") && enabled("reported"));
        assert!(!enabled("digging") && !enabled("dropped") && !enabled("shelved"));
    }

    #[test]
    fn rounds_read_newest_first_and_only_the_newest_starts_open() {
        let now = at(2026, 10, 15, 16);
        let rounds = json!([
            {"id": 3, "focus": "", "report": null, "sha": "", "web": false, "started_at": (now - Duration::minutes(2)).to_rfc3339(), "finished_at": null, "error": null, "behind": null, "stale": false},
            {"id": 2, "focus": "offline sync", "sha": "abc1234def", "web": true, "started_at": at(2026, 10, 15, 9).to_rfc3339(),
             "finished_at": at(2026, 10, 15, 10).to_rfc3339(), "error": null, "behind": 3, "stale": false,
             "report": {"verdict": "partly", "why": "Needs a queue.", "blocks": [{"type": "gist", "text": "A queue makes it work offline."}]}},
            {"id": 1, "focus": "", "report": null, "sha": "0011223344", "web": false, "started_at": at(2026, 10, 7, 9).to_rfc3339(),
             "finished_at": at(2026, 10, 7, 9).to_rfc3339(), "error": "claude timed out", "behind": null, "stale": true},
        ]);
        let v = rounds_of(&idea("digging", rounds), now);
        assert_eq!(v.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["Round 3", "Round 2", "Round 1"]);
        assert_eq!(v[0].running.as_deref(), Some("Digging… 2m"));
        assert_eq!(
            v[1],
            RoundView {
                id: "2".into(),
                title: "Round 2".into(),
                focus: "offline sync".into(),
                verdict: Some(("Partly fits", "warn")),
                gist: "A queue makes it work offline.".into(),
                stale: false,
                when: "Today, 10:05 AM".into(),
                running: None,
                error: None,
                web: true,
                blocks: vec![Block::Gist("A queue makes it work offline.".into())],
            }
        );
        assert_eq!(v[1].line(true), "offline sync");
        assert_eq!(v[1].line(false), "offline sync — A queue makes it work offline.");
        assert!((v[2].stale, v[2].error.as_deref()) == (true, Some("claude timed out")));
        assert_eq!(v[2].line(false), "claude timed out");
        assert!(starts_open(0) && !starts_open(1));
    }

    #[test]
    fn report_blocks_draw_in_order_skipping_unknown_and_empty_ones() {
        let rep = json!({"verdict": "fits", "why": "Small change.", "blocks": [
            {"type": "scorecard", "fit": "partly", "size": "m", "risk": null},
            {"type": "chart", "data": [1, 2]},
            {"type": "flow", "steps": [{"name": "Dig", "new": false}, {"name": "Queue", "new": true}]},
            {"type": "code", "items": [{"what": "The lens thread", "file": "src/lens.rs:40"}, {"what": "", "file": "x.rs"}]},
            {"type": "points", "title": "Risks", "items": [{"short": "Tokens on disk", "detail": "They sit in plain text.", "tag": "security"}]},
            {"type": "points", "title": "Empty", "items": []},
            {"type": "question", "id": "q1", "text": "Which store?", "choices": ["SQLite", "Files"]},
            {"type": "compare", "lean": "SQLite", "options": [{"name": "SQLite", "pros": ["Fast"], "cons": []}]},
            {"type": "related", "refs": ["G2", "T9"]},
        ]});
        assert_eq!(
            report_blocks(&rep),
            vec![
                Block::Gist("Small change.".into()),
                Block::Scorecard(vec![("Fit", "Partly", "warn"), ("Size", "Medium", "")]),
                Block::Flow(vec![("Dig".into(), false), ("Queue".into(), true)]),
                Block::Code(vec![("The lens thread".into(), vec!["src/lens.rs:40".into()])]),
                Block::Points { title: "Risks".into(), items: vec![Point { short: "Tokens on disk".into(), detail: "They sit in plain text.".into(), tag: Some("security".into()) }] },
                Block::Question { text: "Which store?".into(), choices: vec!["SQLite".into(), "Files".into()] },
                Block::Compare { lean: "SQLite".into(), options: vec![Opt { name: "SQLite".into(), pros: vec!["Fast".into()], cons: vec![] }] },
                Block::Related(vec!["G2".into(), "T9".into()]),
            ]
        );
        assert_eq!(file_name("crates/app/src/lens.rs:40"), "lens.rs");
    }

    #[test]
    fn an_old_report_draws_its_lists_as_blocks() {
        let rep = json!({"verdict": "partly", "why": "Needs a queue.", "builds_on": [{"what": "The lens thread", "files": ["src/lens.rs:40"]}],
                         "ways_to_extend": ["Sync settings"], "risks": [{"kind": "security", "risk": "Tokens on disk"}],
                         "pitfalls": ["Clock skew"], "open_questions": ["Which store?"], "related": ["G2", "T9"]});
        let short = |x: &str| Point { short: x.into(), ..Point::default() };
        assert_eq!(
            report_blocks(&rep),
            vec![
                Block::Gist("Needs a queue.".into()),
                Block::Code(vec![("The lens thread".into(), vec!["src/lens.rs:40".into()])]),
                Block::Points { title: "Watch out".into(), items: vec![Point { tag: Some("security".into()), ..short("Tokens on disk") }, short("Clock skew")] },
                Block::Points { title: "Could grow into".into(), items: vec![short("Sync settings")] },
                Block::Question { text: "Which store?".into(), choices: vec![] },
                Block::Related(vec!["G2".into(), "T9".into()]),
            ]
        );
    }

    fn rounds_of(i: &Value, now: DateTime<Utc>) -> Vec<RoundView> {
        rounds(i, now)
    }

    #[test]
    fn a_notes_byline_is_who_and_when() {
        let now = at(2026, 10, 15, 16);
        assert_eq!(note_byline(&json!({"source": "T12", "at": at(2026, 10, 15, 14).to_rfc3339()}), now), "T12 · Today, 2:05 PM");
        assert_eq!(note_byline(&json!({"source": null, "at": at(2026, 10, 2, 8).to_rfc3339()}), now), "Oct 2, 8:05 AM");
    }

    #[test]
    fn close_hides_once_planned_waits_on_a_dig_and_reopens_once_closed() {
        let now = Utc::now();
        assert_eq!(close_button(&idea("planned", json!([]))), None);
        let open = close_button(&idea("reported", json!([]))).unwrap();
        assert_eq!((open.label.as_str(), open.enabled), ("Close", true));
        assert_eq!(close_menu_view(&idea("new", json!([]))), [("shelve", "Shelve for later"), ("drop", "Won’t do")]);
        assert_eq!(close_button(&idea("digging", json!([]))).unwrap(), Btn { label: "Close".into(), enabled: false, tip: "Wait for the dig to finish".into() });
        let mut planning = idea("reported", json!([]));
        planning["planning"] = json!(true);
        assert!(!close_button(&planning).unwrap().enabled);

        let shelved = idea("shelved", json!([]));
        assert_eq!(close_button(&shelved).unwrap(), Btn { label: "Shelved".into(), enabled: true, tip: "Reopen it".into() });
        assert_eq!(close_button(&idea("dropped", json!([]))).unwrap().label, "Won’t do");
        assert_eq!(close_menu_view(&shelved), [("reopen", "Reopen")]);
        assert_eq!(dig_button(&shelved, now).tip, "It’s shelved; reopen it first");
    }

    #[test]
    fn talk_hides_once_planned_and_is_open_otherwise() {
        assert_eq!(talk_button(&idea("planned", json!([]))), None);
        for state in ["new", "digging", "reported", "shelved", "dropped"] {
            let bt = talk_button(&idea(state, json!([]))).unwrap();
            assert_eq!((bt.label.as_str(), bt.enabled), ("Talk it over", true), "{state}");
        }
    }

    #[gpui_kit::test]
    fn talk_sends_once_names_the_question_and_a_refusal_frees_the_button(cx: &mut gpui_kit::TestAppContext) {
        use crate::backend::Backend;
        use crate::parity::{settle, window};
        let (w, rec) = window(cx);
        let list = rec.board().get("ideas", &crate::ui::ideas::query()).unwrap();
        let r = fmt::ref_of(arr(&list, "ideas").iter().find(|i| s(i, "state") == "reported").unwrap(), "I");
        w.update(cx, |m, _, cx| {
            talk(m, &r, None, cx);
            assert!(m.idea.busy.contains(&format!("talk::{r}")), "sending");
            talk(m, &r, None, cx);
        })
        .unwrap();
        settle(cx);
        let path = format!("ideas/{r}/talk");
        assert_eq!(rec.posts().iter().filter(|(p, _)| *p == path).count(), 1, "once at a time");
        assert_eq!(rec.last(&path), Some(json!({})));
        let toast = |w: &gpui_kit::WindowHandle<MainWindow>, cx: &mut gpui_kit::TestAppContext| {
            w.update(cx, |m, _, _| {
                assert!(m.idea.busy.is_empty(), "a refused talk doesn't stay on Sending…");
                (m.toasts[0].text.clone(), m.toasts[0].err)
            })
            .unwrap()
        };
        assert_eq!(toast(&w, cx), ("Midna isn't running, so the board can't open a terminal.".to_string(), true));
        w.update(cx, |m, _, cx| {
            talk(m, &r, Some("Which store?"), cx);
            assert!(m.idea.busy.contains(&format!("talk::{r}::Which store?")));
        })
        .unwrap();
        settle(cx);
        assert_eq!(rec.last(&path), Some(json!({"question": "Which store?"})));
        assert_eq!(toast(&w, cx), (format!("{r}'s reports don't ask “Which store?”."), true));
    }

    #[gpui_kit::test]
    fn shelving_offers_undo_which_reopens_it(cx: &mut gpui_kit::TestAppContext) {
        use crate::backend::Backend;
        use crate::parity::{settle, window};
        let (w, rec) = window(cx);
        let list = rec.board().get("ideas", &crate::ui::ideas::query()).unwrap();
        let r = fmt::ref_of(arr(&list, "ideas").iter().find(|i| s(i, "state") == "reported").unwrap(), "I");
        w.update(cx, |m, _, cx| {
            close(m, "shelve", &r, cx);
            close(m, "shelve", &r, cx);
        })
        .unwrap();
        settle(cx);
        assert_eq!(rec.posts().iter().filter(|(p, _)| *p == format!("ideas/{r}/shelve")).count(), 1, "once at a time");
        let undo = w
            .update(cx, |m, _, _| {
                assert!(m.idea.busy.is_empty());
                let t = &m.toasts[0];
                assert_eq!((t.text.clone(), t.err), (format!("{r} shelved"), false));
                t.undo.clone()
            })
            .unwrap();
        assert_eq!(undo, Some(format!("ideas/{r}/reopen")));
        w.update(cx, |m, _, cx| close(m, "reopen", &r, cx)).unwrap();
        settle(cx);
        assert_eq!(rec.last(&format!("ideas/{r}/reopen")), Some(json!({})));
        let state = rec.board().get(&format!("ideas/{r}"), &[]).unwrap();
        assert_eq!(s(&state, "state"), "reported", "back to its report");
    }

    #[gpui_kit::test]
    fn dig_sends_once_and_a_refusal_frees_the_button(cx: &mut gpui_kit::TestAppContext) {
        use crate::parity::{settle, window};
        let (w, rec) = window(cx);
        w.update(cx, |m, _, cx| {
            start(m, "dig", "I5", cx);
            assert!(m.idea.busy.contains("dig::I5"), "sending");
            start(m, "dig", "I5", cx);
        })
        .unwrap();
        settle(cx);
        assert_eq!(rec.posts().iter().filter(|(p, _)| p == "ideas/I5/dig").count(), 1);
        assert_eq!(rec.last("ideas/I5/dig"), Some(json!({})));
        w.update(cx, |m, _, _| {
            assert!(m.idea.busy.is_empty(), "a refused dig doesn't stay on Sending…");
            assert_eq!(m.toasts.iter().map(|t| (t.text.as_str(), t.err)).collect::<Vec<_>>(), [("I5 became a goal already; work on the goal instead.", true)]);
        })
        .unwrap();
    }
}
