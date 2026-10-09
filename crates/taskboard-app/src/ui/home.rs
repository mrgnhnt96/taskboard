//! The home page (`GET /home` and today's numbers from `GET /days`): what needs you, today's
//! numbers, and the goals with work in flight, grouped by project. Each goal shows the tasks of
//! the wave it's on and what's still open from earlier waves; nothing else about its waves.
//!
//! What each part shows is worked out by the pure `*_vm` functions; the render code only lays it
//! out. The page is read-only: a tile opens its task, a Needs you item offers the one thing to do
//! about it (open the terminal, the PR or the task, or restart a lost terminal) and dismisses with ×.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, b, i, obj, opt_s, s};
use crate::theme::Theme;
use crate::ui::kit;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};

const PAGE_X: f32 = 28.;
const GAP: f32 = 14.;
const CARD_MIN: f32 = 340.;
/// Needs you shows this many; "Show all" opens the rest in the alerts dialog.
const ASKS_MAX: usize = 6;

// ================================================================== view models

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sev {
    Crit,
    Warn,
    Ok,
}

/// What a Needs you item's button does.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// A lost terminal: `POST /tasks/:id/resume {mode: fresh}`.
    Restart(String),
    /// Midna couldn't start it: `POST /tasks/:id/start {mode: new}`.
    Start(String),
    /// Show the task's terminal: `POST /tasks/:id/focus`.
    Focus(String),
    OpenPr(String),
    OpenTask(String),
    OpenGoal(String),
}

impl Act {
    pub fn label(&self) -> &'static str {
        match self {
            Act::Restart(_) => "Restart",
            Act::Start(_) => "Start",
            Act::Focus(_) => "Open terminal",
            Act::OpenPr(_) => "Open PR",
            Act::OpenTask(_) => "Open task",
            Act::OpenGoal(_) => "Open goal",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub id: String,
    pub sev: Sev,
    pub kind: String,
    pub text: String,
    /// (ref, title)
    pub task: Option<(String, String)>,
    /// (ref, name)
    pub goal: Option<(String, String)>,
    /// "41m", "3h", "now".
    pub time: String,
    pub act: Option<Act>,
    /// Review and urgent alerts stay until what raised them clears.
    pub dismiss: bool,
}

/// "41m", "3h", "2d" (no "ago"); "now" under a minute.
pub fn short_ago(iso: &str) -> String {
    let a = fmt::ago(iso);
    if a == "just now" { "now".into() } else { a.trim_end_matches(" ago").to_string() }
}

/// The alert's text less the "T12: " its task's ref already says.
fn ask_text(a: &Value, card: &Value) -> String {
    if let Some(q) = opt_s(card, "question").filter(|_| s(card, "status") == "needs") {
        return q.to_string();
    }
    let text = s(a, "text");
    let r = s(a, "task");
    let rest = text.strip_prefix(r).map(|x| x.trim_start_matches(':').trim_start());
    rest.filter(|x| !x.is_empty()).unwrap_or(text).to_string()
}

fn pr_url(card: &Value) -> Option<String> {
    obj(card, "pr").and_then(|p| opt_s(p, "url")).map(str::to_string)
}

fn phase(card: &Value) -> &str {
    obj(card, "pr").and_then(|p| obj(p, "stage")).map(|st| s(st, "phase")).unwrap_or("")
}

pub fn ask_vm(a: &Value) -> Ask {
    let card = &a["card"];
    let r = opt_s(a, "task").map(str::to_string);
    let has_card = card.is_object();
    let reason = s(card, "needs_reason");
    let (sev, kind, act): (Sev, &str, Option<Act>) = if a["urgent"] == true {
        (Sev::Crit, "Urgent", r.clone().map(Act::OpenTask))
    } else if has_card && b(card, "lost") {
        (Sev::Crit, "Terminal lost", r.clone().map(Act::Restart))
    } else if has_card && reason == "start_failed" {
        (Sev::Crit, "Couldn’t start", r.clone().map(Act::Start))
    } else if has_card && b(card, "failed") {
        (Sev::Crit, "Failed", r.clone().map(Act::OpenTask))
    } else if has_card && phase(card) == "fix" {
        (Sev::Crit, "Checks failed", pr_url(card).map(Act::OpenPr))
    } else if a["review"] == true {
        (Sev::Ok, "Review the PR", pr_url(card).map(Act::OpenPr))
    } else if has_card && phase(card) == "merge" {
        (Sev::Ok, "Ready to merge", pr_url(card).map(Act::OpenPr))
    } else if has_card && s(card, "status") == "needs" && reason == "question" {
        (Sev::Warn, "Question", r.clone().map(Act::Focus))
    } else if has_card && s(card, "status") == "needs" {
        (Sev::Warn, "Needs you", r.clone().map(Act::Focus))
    } else {
        (Sev::Warn, "Needs you", r.clone().map(Act::OpenTask))
    };
    let act = act.or_else(|| r.clone().map(Act::OpenTask)).or_else(|| opt_s(a, "goal").map(|g| Act::OpenGoal(g.to_string())));
    let goal_ref = opt_s(a, "goal").map(str::to_string).or_else(|| obj(card, "goal").map(|g| s(g, "ref").to_string()));
    Ask {
        id: s(a, "id").to_string(),
        sev,
        kind: kind.to_string(),
        text: ask_text(a, card),
        task: r.map(|r| (r, s(card, "title").to_string())),
        goal: goal_ref.map(|g| (g, s(a, "goal_name").to_string())),
        time: short_ago(s(a, "at")),
        act,
        dismiss: !s(a, "id").is_empty() && a["review"] != true && a["urgent"] != true,
    }
}

/// The Needs you items: the worst first, then the newest.
pub fn asks_vm(home: &Value) -> Vec<Ask> {
    let mut v: Vec<(Ask, String)> = arr(home, "needs").iter().map(|a| (ask_vm(a), s(a, "at").to_string())).collect();
    v.sort_by(|(a, at), (b, bt)| (a.sev as u8).cmp(&(b.sev as u8)).then(bt.cmp(at)));
    v.into_iter().map(|(a, _)| a).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trend {
    Good,
    Bad,
    Plain,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stat {
    pub label: &'static str,
    pub value: String,
    pub delta: String,
    pub trend: Trend,
}

fn num(v: &Value, k: &str) -> f64 {
    v[k].as_f64().unwrap_or(0.0)
}

fn signed(x: f64, f: impl Fn(f64) -> String) -> String {
    if x >= 0.0 { format!("+{}", f(x)) } else { format!("−{}", f(-x)) }
}

fn count(x: f64) -> String {
    let r = (x * 10.0).round() / 10.0;
    if r.fract() == 0.0 { format!("{}", r as i64) } else { format!("{r}") }
}

/// Today's numbers from `GET /days`, each against the usual day (the median of the two weeks
/// before) when there is one. More done is good; more waiting on you is bad.
pub fn stats_vm(days: &Value) -> Vec<Stat> {
    let (day, usual) = (&days["day"], &days["usual"]);
    let have_usual = num(usual, "days") > 0.0;
    let dur = crate::ui::days::dur;
    let delta = |k: &str, more_is: Trend, f: fn(f64) -> String| -> (String, Trend) {
        if !have_usual {
            return (String::new(), Trend::Plain);
        }
        let d = num(day, k) - num(usual, k);
        if d.abs() < 0.5 {
            return (String::new(), Trend::Plain);
        }
        let trend = match (d.abs() < 0.5, more_is) {
            (true, _) | (_, Trend::Plain) => Trend::Plain,
            (false, Trend::Good) => if d > 0.0 { Trend::Good } else { Trend::Bad },
            (false, Trend::Bad) => if d > 0.0 { Trend::Bad } else { Trend::Good },
        };
        (signed(d, f), trend)
    };
    let (human, est) = (num(day, "human_min"), num(day, "est_agent_min"));
    let mut out = vec![Stat { label: "Agent time", value: dur(num(day, "agent_min")), delta: String::new(), trend: Trend::Plain }];
    out.push(Stat {
        label: "Human estimate",
        value: if human > 0.0 { dur(human) } else { "—".into() },
        delta: if human > 0.0 && est > 0.0 { format!("{:.1}× agent time", human / est) } else { String::new() },
        trend: Trend::Good,
    });
    for (label, k, more_is, f) in [
        ("Tasks done", "done", Trend::Good, count as fn(f64) -> String),
        ("PRs opened", "prs", Trend::Good, count),
        ("Waiting on you", "wait_min", Trend::Bad, dur),
    ] {
        let (d, trend) = delta(k, more_is, f);
        out.push(Stat { label, value: if k == "wait_min" { dur(num(day, k)) } else { count(num(day, k)) }, delta: d, trend });
    }
    if let Some(v) = crate::ui::days::View::of(days) {
        let up = !v.saved.vs_last.starts_with('−');
        out.push(Stat {
            label: "Saved this week",
            value: v.saved.total.clone(),
            delta: v.saved.vs_last.replace(" on last week", " vs last week"),
            trend: if v.saved.vs_last.is_empty() { Trend::Plain } else if up { Trend::Good } else { Trend::Bad },
        });
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Review {
    Approved,
    Changes,
    Pending,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reviewer {
    pub name: String,
    pub initials: String,
    pub state: Review,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub r: String,
    pub title: String,
    pub status: String,
    /// A `kit::tone` name.
    pub tone: &'static str,
    pub line: String,
    pub reviewers: Vec<Reviewer>,
}

fn initials(name: &str) -> String {
    let parts: Vec<&str> = name.split(|c: char| c.is_whitespace() || c == '-' || c == '_' || c == '.').filter(|p| !p.is_empty()).collect();
    let pick: String = if parts.len() >= 2 { parts.iter().take(2).filter_map(|p| p.chars().next()).collect() } else { name.chars().take(2).collect() };
    pick.to_uppercase()
}

fn reviewers(pr: &Value) -> Vec<Reviewer> {
    arr(&pr["bar"], "reviewer_rows")
        .iter()
        .map(|r| {
            let name = opt_s(r, "user").filter(|u| !u.is_empty()).or(opt_s(r, "name")).unwrap_or("").to_string();
            let state = match s(r, "state") {
                "approved" => Review::Approved,
                "changes" => Review::Changes,
                _ => Review::Pending,
            };
            Reviewer { initials: initials(&name), name, state }
        })
        .collect()
}

/// A task tile's status pill and line.
pub fn tile_vm(c: &Value) -> Tile {
    let pr = obj(c, "pr").filter(|p| !p["num"].is_null());
    let pr_num = pr.map(|p| p["num"].as_i64().map(|n| format!("PR #{n}")).unwrap_or_else(|| format!("PR #{}", s(p, "num"))));
    let since = |verb: &str| format!("{verb} {}", fmt::ago(opt_s(c, "when").or(opt_s(c, "updated_at")).unwrap_or("")));
    let busy = |c: &Value| {
        let run = fmt::running_for(c).trim_start_matches("running ").to_string();
        match opt_s(c, "latest") {
            Some(l) if !run.is_empty() => format!("{l} · {run}"),
            Some(l) => l.to_string(),
            None => if run.is_empty() { "Working".into() } else { format!("Running {run}") },
        }
    };
    let (status, tone, line): (&str, &str, String) = match s(c, "status") {
        "needs" if b(c, "lost") => ("Lost", "down", format!("Terminal closed {}", fmt::hhmm(s(c, "when")))),
        "needs" if s(c, "needs_reason") == "start_failed" => ("Couldn’t start", "down", "Midna didn’t start it".into()),
        "needs" if s(c, "needs_reason") == "question" => ("Question", "needs", since("Asked")),
        "needs" => ("Needs you", "needs", since("Updated")),
        "working" if c["compacting"].is_string() => ("Compacting", "working", fmt::compacting(c).unwrap_or_default()),
        "working" => ("Working", "working", busy(c)),
        "queued" if b(c, "starting") => ("Starting", "working", opt_s(c, "waiting").unwrap_or("Opening a terminal").to_string()),
        "queued" => ("Queued", "queued", opt_s(c, "waiting").unwrap_or("When a terminal is free").to_string()),
        "done" if pr.is_some() => {
            let p = pr.unwrap_or(&Value::Null);
            let stage = obj(p, "stage");
            let label = stage.map(|st| s(st, "label")).filter(|l| !l.is_empty());
            let num = pr_num.clone().unwrap_or_default();
            let with = |l: Option<&str>| l.map(|l| format!("{num} · {l}")).unwrap_or(num.clone());
            if stage.and_then(|st| obj(st, "stopped")).is_some() {
                ("Needs you", "needs", with(label))
            } else {
                match stage.map(|st| s(st, "phase")).unwrap_or("") {
                    "checks" => ("Checks running", "goal", with(label)),
                    "fix" => ("Checks failed", "down", with(label)),
                    "comments" => ("Comments", "goal", num.clone()),
                    "merge" => ("Ready to merge", "up", num.clone()),
                    _ => ("In review", "goal", num.clone()),
                }
            }
        }
        "done" if b(c, "failed") => ("Failed", "down", since("Stopped")),
        _ => ("Done", "up", since("Finished")),
    };
    Tile {
        r: fmt::ref_of(c, "T"),
        title: s(c, "title").to_string(),
        status: status.to_string(),
        tone,
        line,
        reviewers: pr.map(reviewers).unwrap_or_default(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GoalCard {
    /// None for a project's tasks outside any goal.
    pub r: Option<String>,
    pub name: String,
    pub count: String,
    /// Done share, 0..1.
    pub done: f32,
    pub state: Option<(String, &'static str)>,
    pub now_label: String,
    pub now: Vec<Tile>,
    pub left_label: String,
    pub left: Vec<Tile>,
    pub queued: Option<String>,
}

fn waves_text(ws: &[Value]) -> String {
    let n: Vec<String> = ws.iter().filter_map(Value::as_i64).map(|w| w.to_string()).collect();
    match n.len() {
        0 => String::new(),
        1 => format!("wave {}", n[0]),
        _ => format!("waves {} and {}", n[..n.len() - 1].join(", "), n[n.len() - 1]),
    }
}

/// The goal's chip: what it waits on, worst first.
fn goal_chip(g: &Value, now: &[Tile], left: &[Tile]) -> (String, &'static str) {
    let all = || now.iter().chain(left.iter());
    if all().any(|t| t.status == "Lost" || t.status == "Couldn’t start") {
        ("Needs a restart".into(), "down")
    } else if all().any(|t| t.tone == "needs") {
        ("Waiting on you".into(), "needs")
    } else if fmt::opt_s(g, "stopped").is_some() {
        ("Waiting on you".into(), "needs")
    } else if b(g, "paused") {
        ("Paused".into(), "needs")
    } else if all().any(|t| t.tone == "working") {
        ("In progress".into(), "working")
    } else if !now.is_empty() && now.iter().all(|t| t.tone == "goal" || t.tone == "up" || t.tone == "down") {
        (if now.len() > 1 { format!("{} PRs awaiting merge", now.len()) } else { "Awaiting merge".into() }, "working")
    } else {
        ("Queued".into(), "queued")
    }
}

pub fn goal_card_vm(e: &Value) -> GoalCard {
    let g = &e["goal"];
    let now: Vec<Tile> = arr(e, "now").iter().map(tile_vm).collect();
    let left: Vec<Tile> = arr(e, "left").iter().map(tile_vm).collect();
    let wave = e["wave"].as_i64();
    let in_wave = wave.map(|w| format!(" in wave {w}")).unwrap_or_default();
    let queued = i(e, "queued");
    let queued = (queued > 0).then(|| format!("{}{in_wave}", if now.is_empty() { format!("{queued} queued") } else { format!("{queued} more queued") }));
    if g.is_null() {
        return GoalCard {
            r: None,
            name: "Not in a goal".into(),
            count: String::new(),
            done: 0.,
            state: None,
            now_label: "Now".into(),
            now,
            left_label: String::new(),
            left,
            queued,
        };
    }
    let (total, done) = (i(g, "total"), i(g, "done"));
    GoalCard {
        r: Some(fmt::ref_of(g, "G")),
        name: s(g, "name").to_string(),
        count: format!("{done} of {total} done"),
        done: if total > 0 { done as f32 / total as f32 } else { 0. },
        state: Some(goal_chip(g, &now, &left)),
        now_label: wave.map(|w| format!("Now · wave {w}")).unwrap_or_else(|| "Now".into()),
        left_label: format!("Still open from {}", waves_text(arr(e, "left_waves"))),
        now,
        left,
        queued,
    }
}

/// The goals grouped by project, in the board's order (projects by name).
pub fn projects_vm(home: &Value) -> Vec<(String, Vec<GoalCard>)> {
    let mut out: Vec<(String, Vec<GoalCard>)> = Vec::new();
    for e in arr(home, "goals") {
        let p = s(e, "project").to_string();
        let card = goal_card_vm(e);
        match out.iter_mut().find(|(name, _)| *name == p) {
            Some((_, cards)) => cards.push(card),
            None => out.push((p, vec![card])),
        }
    }
    out
}

// ================================================================== actions

fn sent(m: &mut MainWindow, text: String, cx: &mut Context<MainWindow>) {
    m.toast(text, false, cx);
}

pub fn run_act(m: &mut MainWindow, act: &Act, cx: &mut Context<MainWindow>) {
    match act.clone() {
        Act::Restart(r) => m.post(format!("tasks/{r}/resume"), json!({"mode": "fresh"}), cx, move |m, _, cx| sent(m, format!("Restarting {r}"), cx)),
        Act::Start(r) => m.post(format!("tasks/{r}/start"), json!({"mode": "new"}), cx, move |m, _, cx| sent(m, format!("Starting {r}"), cx)),
        Act::Focus(r) => m.post(format!("tasks/{r}/focus"), json!({}), cx, move |m, _, cx| sent(m, format!("Showing {r}’s terminal in Midna"), cx)),
        Act::OpenPr(url) => cx.open_url(&url),
        Act::OpenTask(r) => m.open_task(r, cx),
        Act::OpenGoal(g) => m.go(Page::Goal(g), cx),
    }
}

/// × on a Needs you item: `POST /alerts/:id/dismiss`, gone from the list at once.
pub fn dismiss(m: &mut MainWindow, id: &str, cx: &mut Context<MainWindow>) {
    if let Some(needs) = m.data.home.as_mut().and_then(|h| h.get_mut("needs")).and_then(Value::as_array_mut) {
        needs.retain(|a| s(a, "id") != id);
    }
    crate::app::dismiss_alert(m, id, cx);
    cx.notify();
}

// ================================================================== render

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let home = m.data.home.clone().unwrap_or(Value::Null);
    // A break's urgent alert is already its "Master is red" line.
    let st = m.state().clone();
    let needs: Vec<Value> = arr(&home, "needs").iter().filter(|a| !crate::ui::prwatch::shown_as_master(&st, a)).cloned().collect();
    let asks = asks_vm(&json!({ "needs": needs }));
    let stats = m.data.today.as_ref().map(stats_vm).unwrap_or_default();
    let mut projects = projects_vm(&home);
    // The rail's goal filter: only that goal's card.
    if m.filters.goal != "all" {
        for (_, cards) in projects.iter_mut() {
            cards.retain(|c| c.r.as_deref() == Some(m.filters.goal.as_str()));
        }
        projects.retain(|(_, cards)| !cards.is_empty());
    }

    let rail = m.sidebar.drag.map(|_| m.sidebar_width).unwrap_or_else(crate::ui::sidebar::rail_width);
    let main_w = (window.viewport_size().width.as_f32() - rail - PAGE_X * 2.).max(320.);
    let cols = ((main_w + GAP) / (CARD_MIN + GAP)).floor().clamp(1., 3.);
    let card_w = (main_w - GAP * (cols - 1.)) / cols;

    let mut page = div().flex().flex_col().gap(px(24.)).px(px(PAGE_X)).pt(px(22.)).pb(px(32.)).line_height(relative(1.4));

    // Needs you beside Today when both fit; stacked on a narrow window.
    let wide = main_w >= 1000.;
    let mut top = div().flex().gap(px(22.)).when(!wide, |d| d.flex_col());
    if !asks.is_empty() {
        let ask_w = if wide { main_w * 0.6 - 11. } else { main_w };
        top = top.child(needs_section(&t, &asks, ask_w, cx));
    }
    if !stats.is_empty() {
        let stat_w = if asks.is_empty() || !wide { main_w } else { main_w * 0.4 - 11. };
        top = top.child(today_section(&t, &stats, stat_w));
    }
    page = page.child(top);

    if m.data.home.is_none() {
        page = page.child(kit::empty(&t, if m.down.is_some() { "Can’t load the board." } else { "Loading…" }));
    } else if projects.is_empty() {
        page = page.child(kit::empty(&t, "Nothing is running or queued."));
    }
    for (name, cards) in projects {
        let mut grid = div().flex().flex_wrap().items_start().gap(px(GAP));
        for c in cards {
            grid = grid.child(goal_card(&t, &c, card_w, cx));
        }
        page = page.child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(div().font_family(t.mono_font.clone()).text_size(px(12.5)).text_color(t.muted).child(name))
                .child(grid),
        );
    }
    div().id("home-page").flex_1().min_w_0().h_full().overflow_y_scroll().child(page).into_any_element()
}

fn section_label(t: &Theme, text: String) -> Div {
    div().text_size(px(12.5)).font_weight(FontWeight::BOLD).text_color(t.muted).child(text.to_uppercase())
}

fn sev_colors(t: &Theme, sev: Sev) -> (Hsla, Hsla, Hsla) {
    match sev {
        Sev::Crit => (t.down, t.down_soft, t.down_line),
        Sev::Warn => (t.warn_fg, t.warn_soft, t.warn_line),
        Sev::Ok => (t.up_fg, t.up_soft, t.up),
    }
}

fn needs_section(t: &Theme, asks: &[Ask], w: f32, cx: &mut Context<MainWindow>) -> Div {
    let per_row = if w >= 600. { 3. } else if w >= 400. { 2. } else { 1. };
    let tile_w = (w - 10. * (per_row - 1.)) / per_row;
    let mut grid = div().flex().flex_wrap().gap(px(10.));
    for a in asks.iter().take(ASKS_MAX) {
        grid = grid.child(ask_tile(t, a, tile_w, cx));
    }
    let mut head = div().flex().items_center().gap(px(12.)).child(section_label(t, format!("Needs you · {}", asks.len())));
    if asks.len() > ASKS_MAX {
        head = head.child(
            kit::link(t, "asks-all", format!("Show all {}", asks.len()))
                .text_size(px(12.5))
                .on_click(cx.listener(|m, _, window, cx| m.set_modal(Some(crate::ui::modals::Modal::Alerts), window, cx))),
        );
    }
    div().flex().flex_col().gap(px(10.)).w(px(w)).flex_none().child(head).child(grid)
}

fn ref_link(t: &Theme, id: String, text: String, weight: FontWeight, size: f32) -> Stateful<Div> {
    let fg = t.text;
    div()
        .id(SharedString::from(id))
        .min_w_0()
        .truncate()
        .text_size(px(size))
        .font_weight(weight)
        .text_color(fg)
        .underline()
        .cursor_pointer()
        .hover(|s| s.opacity(0.75))
        .child(text)
}

fn ask_tile(t: &Theme, a: &Ask, w: f32, cx: &mut Context<MainWindow>) -> Div {
    let (fg, bg, line) = sev_colors(t, a.sev);
    let mut head = div().flex().items_center().justify_between().gap(px(8.)).child(
        div().text_size(px(11.5)).font_weight(FontWeight::BOLD).text_color(fg).child(a.kind.to_uppercase()),
    );
    if a.dismiss {
        let id = a.id.clone();
        let muted = t.muted;
        head = head.child(
            div()
                .id(SharedString::from(format!("ask-x-{}", a.id)))
                .flex()
                .items_center()
                .justify_center()
                .size(px(22.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover(move |s| s.bg(hsla(0., 0., 0., 0.06)))
                .tooltip(kit::tip("Dismiss"))
                .on_click(cx.listener(move |m, _, _, cx| dismiss(m, &id, cx)))
                .child(kit::glyph(kit::Glyph::X, 12., muted)),
        );
    }
    let mut body = div().flex().flex_col().gap(px(5.)).min_w_0().child(head).child(div().text_size(px(13.5)).line_clamp(3).child(a.text.clone()));
    if let Some((r, title)) = &a.task {
        let open = r.clone();
        body = body.child(
            ref_link(t, format!("ask-task-{}", a.id), format!("{r} {title}"), FontWeight::SEMIBOLD, 13.)
                .on_click(cx.listener(move |m, _, _, cx| m.open_task(open.clone(), cx))),
        );
    }
    if let Some((g, name)) = &a.goal {
        let open = g.clone();
        let text = if name.is_empty() { g.clone() } else { format!("{g} {name}") };
        body = body.child(
            ref_link(t, format!("ask-goal-{}", a.id), text, FontWeight::NORMAL, 12.5)
                .text_color(t.text_2)
                .on_click(cx.listener(move |m, _, _, cx| m.go(Page::Goal(open.clone()), cx))),
        );
    }
    let mut foot = div().flex().items_center().gap(px(8.)).mt(px(4.));
    if let Some(act) = &a.act {
        let act2 = act.clone();
        foot = foot.child(kit::btn_primary(t, SharedString::from(format!("ask-act-{}", a.id)), act.label()).h(px(28.)).text_size(px(12.5)).on_click(cx.listener(move |m, _, _, cx| run_act(m, &act2, cx))));
    }
    foot = foot.child(div().flex_1()).child(div().text_size(px(12.)).text_color(t.muted).child(a.time.clone()));
    div().w(px(w)).flex_none().flex().flex_col().p(px(12.)).rounded(px(12.)).border_1().border_color(line).bg(bg).child(body.child(foot))
}

fn today_section(t: &Theme, stats: &[Stat], w: f32) -> Div {
    let per_row = if w >= 420. { 3. } else { 2. };
    let tile_w = (w - 10. * (per_row - 1.)) / per_row;
    let mut grid = div().flex().flex_wrap().gap(px(10.));
    for st in stats {
        let color = match st.trend {
            Trend::Good => t.up_fg,
            Trend::Bad => t.warn_fg,
            Trend::Plain => t.muted,
        };
        grid = grid.child(
            div()
                .w(px(tile_w))
                .flex_none()
                .flex()
                .flex_col()
                .px(px(12.))
                .py(px(10.))
                .rounded(px(12.))
                .border_1()
                .border_color(t.border)
                .bg(t.card)
                .child(div().text_size(px(12.)).text_color(t.muted).child(st.label))
                .child(div().text_size(px(20.)).font_weight(FontWeight::BOLD).child(st.value.clone()))
                .child(div().h(px(18.)).text_size(px(12.)).text_color(color).child(st.delta.clone())),
        );
    }
    div().flex().flex_col().gap(px(10.)).w(px(w)).flex_none().child(section_label(t, "Today".into())).child(grid)
}

/// The goal's progress ring: done share in green on the divider color.
fn ring(t: &Theme, share: f32) -> impl IntoElement {
    let (track, fill) = (t.divider, t.up);
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let size = bounds.size.width.as_f32();
            let (cx0, cy0) = (bounds.origin.x.as_f32() + size / 2., bounds.origin.y.as_f32() + size / 2.);
            let r = size / 2. - 3.;
            let arc = |from: f32, to: f32, color: Hsla, window: &mut Window| {
                let steps = ((to - from) / 0.1).ceil().max(1.) as usize;
                let mut p = PathBuilder::stroke(px(5.));
                for k in 0..=steps {
                    let a = from + (to - from) * k as f32 / steps as f32 - std::f32::consts::FRAC_PI_2;
                    let pt = point(px(cx0 + r * a.cos()), px(cy0 + r * a.sin()));
                    if k == 0 { p.move_to(pt) } else { p.line_to(pt) }
                }
                if let Ok(p) = p.build() {
                    window.paint_path(p, color);
                }
            };
            arc(0., std::f32::consts::TAU, track, window);
            if share > 0. {
                arc(0., std::f32::consts::TAU * share.min(1.), fill, window);
            }
        },
    )
    .size(px(38.))
    .flex_none()
}

fn sub_label(t: &Theme, text: String) -> Div {
    div().text_size(px(11.)).font_weight(FontWeight::BOLD).text_color(t.faint).child(text.to_uppercase())
}

fn goal_card(t: &Theme, c: &GoalCard, w: f32, cx: &mut Context<MainWindow>) -> Div {
    let inner = w - 30.;
    let tile_w = (inner - 8.) / 2.;
    let mut head = div().flex().items_start().gap(px(12.));
    if c.r.is_some() {
        head = head.child(ring(t, c.done));
    }
    let mut title = div().flex_1().min_w_0().flex().flex_col().gap(px(2.));
    if let Some(r) = &c.r {
        let open = r.clone();
        title = title
            .child(
                div()
                    .id(SharedString::from(format!("goal-{r}")))
                    .text_size(px(15.5))
                    .font_weight(FontWeight::BOLD)
                    .line_height(relative(1.25))
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .on_click(cx.listener(move |m, _, _, cx| m.go(Page::Goal(open.clone()), cx)))
                    .child(c.name.clone()),
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child(div().font_family(t.mono_font.clone()).text_color(t.goal).child(r.clone()))
                    .child("·")
                    .child(c.count.clone()),
            );
    } else {
        title = title.child(div().text_size(px(15.5)).font_weight(FontWeight::BOLD).child(c.name.clone()));
    }
    head = head.child(title);
    if let Some((label, tone)) = &c.state {
        head = head.child(kit::tone_pill(t, tone, label.clone()));
    }
    let mut card = div().w(px(w)).flex_none().flex().flex_col().gap(px(12.)).p(px(14.)).rounded(px(14.)).border_1().border_color(t.border).bg(t.card).child(head);
    if !c.now.is_empty() {
        card = card.child(tiles(t, c.now_label.clone(), &c.now, tile_w, cx));
    }
    if !c.left.is_empty() {
        card = card.child(tiles(t, c.left_label.clone(), &c.left, tile_w, cx));
    }
    if let Some(q) = &c.queued {
        let link = kit::link(t, SharedString::from(format!("queued-{}", c.r.clone().unwrap_or_else(|| c.name.clone()))), q.clone()).text_size(px(12.5));
        card = card.child(match c.r.clone() {
            Some(g) => link.on_click(cx.listener(move |m, _, _, cx| m.go(Page::Goal(g.clone()), cx))).into_any_element(),
            None => div().text_size(px(12.5)).text_color(t.muted).child(q.clone()).into_any_element(),
        });
    }
    card
}

fn tiles(t: &Theme, label: String, list: &[Tile], tile_w: f32, cx: &mut Context<MainWindow>) -> Div {
    let mut grid = div().flex().flex_wrap().gap(px(8.));
    for x in list {
        grid = grid.child(tile(t, x, tile_w, cx));
    }
    div().flex().flex_col().gap(px(6.)).child(sub_label(t, label)).child(grid)
}

fn tile(t: &Theme, x: &Tile, w: f32, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let open = x.r.clone();
    let hover = t.border_2;
    let mut d = div()
        .id(SharedString::from(format!("tile-{}", x.r)))
        .w(px(w))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(5.))
        .px(px(10.))
        .py(px(9.))
        .rounded(px(10.))
        .border_1()
        .border_color(t.border)
        .bg(t.tint)
        .cursor_pointer()
        .hover(move |s| s.border_color(hover))
        .on_click(cx.listener(move |m, _, _, cx| m.open_task(open.clone(), cx)))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.))
                .child(kit::tone_pill(t, x.tone, x.status.clone()))
                .child(div().font_family(t.mono_font.clone()).text_size(px(11.5)).text_color(t.faint).child(x.r.clone())),
        )
        .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).line_height(relative(1.3)).line_clamp(2).child(x.title.clone()))
        .child(div().text_size(px(12.)).text_color(t.muted).truncate().child(x.line.clone()));
    if !x.reviewers.is_empty() {
        let mut row = div().flex().flex_wrap().gap(px(4.));
        for r in &x.reviewers {
            row = row.child(reviewer_chip(t, r));
        }
        d = d.child(row);
    }
    d
}

fn reviewer_chip(t: &Theme, r: &Reviewer) -> Div {
    let (fg, line, mark, tip) = match r.state {
        Review::Approved => (t.up_fg, t.up, Mark::Check, "Approved"),
        Review::Changes => (t.warn_fg, t.warn_line, Mark::Pencil, "Changes requested"),
        Review::Pending => (t.muted, t.border, Mark::Clock, "Pending"),
    };
    let _ = tip;
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .h(px(22.))
        .pl(px(3.))
        .pr(px(7.))
        .rounded_full()
        .border_1()
        .border_color(line)
        .bg(t.card)
        .text_size(px(11.5))
        .text_color(fg)
        .child(div().flex().items_center().justify_center().size(px(16.)).rounded_full().bg(t.seg).text_color(t.text_2).text_size(px(8.5)).font_weight(FontWeight::BOLD).child(r.initials.clone()))
        .child(r.name.clone())
        .child(mark_icon(mark, 12., fg))
}

#[derive(Clone, Copy)]
enum Mark {
    Check,
    Pencil,
    Clock,
}

/// The reviewer marks, on a 12-unit grid.
fn mark_icon(mark: Mark, size: f32, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let k = bounds.size.width.as_f32() / 12.;
            let o = bounds.origin;
            let u = |x: f32, y: f32| point(o.x + px(x * k), o.y + px(y * k));
            let line = |pts: &[(f32, f32)], w: f32, window: &mut Window| {
                let mut p = PathBuilder::stroke(px(w * k));
                p.move_to(u(pts[0].0, pts[0].1));
                for (x, y) in &pts[1..] {
                    p.line_to(u(*x, *y));
                }
                if let Ok(p) = p.build() {
                    window.paint_path(p, color);
                }
            };
            match mark {
                Mark::Check => line(&[(2.5, 6.3), (4.8, 8.6), (9.5, 3.6)], 1.8, window),
                Mark::Pencil => line(&[(7.8, 2.2), (9.8, 4.2), (4.5, 9.5), (2.5, 9.5), (2.5, 7.5), (7.8, 2.2)], 1.5, window),
                Mark::Clock => {
                    let ring: Vec<(f32, f32)> = (0..=24).map(|i| i as f32 * std::f32::consts::TAU / 24.).map(|a| (6. + 4.3 * a.cos(), 6. + 4.3 * a.sin())).collect();
                    line(&ring, 1.4, window);
                    line(&[(6., 3.8), (6., 6.), (7.5, 7.)], 1.4, window);
                }
            }
        },
    )
    .size(px(size))
    .flex_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    // Beats the glob's `gpui_kit::test`.
    use ::core::prelude::v1::test;

    fn card(v: Value) -> Value {
        let mut base = json!({"id": 7, "ref": "T7", "title": "Wait in panel", "project": "webapp", "status": "working"});
        if let (Some(o), Some(extra)) = (base.as_object_mut(), v.as_object()) {
            for (k, x) in extra {
                o.insert(k.clone(), x.clone());
            }
        }
        base
    }

    #[test]
    fn tiles_say_what_each_task_is_doing() {
        assert_eq!(tile_vm(&card(json!({"latest": "Running cargo test"}))).status, "Working");
        assert_eq!(tile_vm(&card(json!({"latest": "Running cargo test"}))).line, "Running cargo test");
        let lost = tile_vm(&card(json!({"status": "needs", "lost": true})));
        assert_eq!((lost.status.as_str(), lost.tone), ("Lost", "down"));
        let q = tile_vm(&card(json!({"status": "needs", "needs_reason": "question", "question": "10 or 30?"})));
        assert_eq!((q.status.as_str(), q.tone), ("Question", "needs"));
        let queued = tile_vm(&card(json!({"status": "queued", "waiting": "Waits for T4 to finish"})));
        assert_eq!((queued.status.as_str(), queued.line.as_str()), ("Queued", "Waits for T4 to finish"));
    }

    #[test]
    fn a_pr_tile_names_its_stage_and_reviewers() {
        let pr = |phase: &str| {
            card(json!({"status": "done", "pr": {"num": 119, "url": "https://x/pr/119", "state": "OPEN", "stage": {"phase": phase, "label": "Watching checks"},
                "bar": {"reviewer_rows": [{"name": "Jo Lee", "user": "jlee", "state": "approved"}, {"name": "M Park", "user": "mpark", "state": "waiting"}, {"name": "K", "user": "kwong", "state": "changes"}]}}}))
        };
        let t = tile_vm(&pr("review"));
        assert_eq!((t.status.as_str(), t.line.as_str()), ("In review", "PR #119"));
        assert_eq!(t.reviewers.iter().map(|r| (r.name.as_str(), r.state)).collect::<Vec<_>>(), vec![("jlee", Review::Approved), ("mpark", Review::Pending), ("kwong", Review::Changes)]);
        assert_eq!(t.reviewers[0].initials, "JL");
        assert_eq!(tile_vm(&pr("checks")).line, "PR #119 · Watching checks");
        assert_eq!(tile_vm(&pr("merge")).status, "Ready to merge");
        assert_eq!(tile_vm(&pr("fix")).tone, "down");
    }

    #[test]
    fn needs_you_is_worst_first_with_one_thing_to_do() {
        let home = json!({"needs": [
            {"id": "a1", "at": "2026-10-09T10:00:00Z", "text": "T5: Keep the old form?", "task": "T5", "goal": "G2", "goal_name": "Login",
             "card": card(json!({"ref": "T5", "status": "needs", "needs_reason": "question", "question": "Keep the old form?", "title": "Form"}))},
            {"id": "a2", "at": "2026-10-09T09:00:00Z", "text": "T6 lost its terminal.", "task": "T6", "goal_name": "Login",
             "card": card(json!({"ref": "T6", "status": "needs", "lost": true, "title": "Frames", "goal": {"ref": "G2", "name": "Login"}}))},
            {"id": "a3", "at": "2026-10-09T11:00:00Z", "text": "PR #4 waits for your review", "task": "T8", "review": true,
             "card": card(json!({"ref": "T8", "status": "done", "pr": {"num": 4, "url": "https://x/pr/4"}}))}
        ]});
        let asks = asks_vm(&home);
        assert_eq!(asks.iter().map(|a| a.kind.as_str()).collect::<Vec<_>>(), vec!["Terminal lost", "Question", "Review the PR"]);
        assert_eq!(asks[0].act, Some(Act::Restart("T6".into())));
        assert_eq!(asks[0].goal, Some(("G2".into(), "Login".into())));
        assert_eq!(asks[1].text, "Keep the old form?");
        assert_eq!(asks[1].act, Some(Act::Focus("T5".into())));
        assert_eq!(asks[2].act, Some(Act::OpenPr("https://x/pr/4".into())));
        assert!(!asks[2].dismiss, "a PR waiting on your review stays until you review it");
        assert!(asks[0].dismiss);
    }

    #[test]
    fn a_goal_card_shows_its_wave_leftovers_and_queue() {
        let e = json!({"project": "webapp", "wave": 2, "waves": 3, "queued": 3, "left_waves": [1],
            "goal": {"ref": "G14", "name": "Background work", "total": 10, "done": 2},
            "now": [card(json!({"ref": "T122"})), card(json!({"ref": "T123", "status": "needs", "needs_reason": "question"}))],
            "left": [card(json!({"ref": "T121", "status": "done", "pr": {"num": 119, "stage": {"phase": "comments"}}}))]});
        let c = goal_card_vm(&e);
        assert_eq!(c.now_label, "Now · wave 2");
        assert_eq!(c.left_label, "Still open from wave 1");
        assert_eq!(c.queued.as_deref(), Some("3 more queued in wave 2"));
        assert_eq!(c.count, "2 of 10 done");
        assert_eq!(c.state, Some(("Waiting on you".into(), "needs")));
        let shipping = goal_card_vm(&json!({"project": "webapp", "wave": null, "queued": 0, "goal": {"ref": "G13", "name": "S", "total": 2, "done": 2},
            "now": [card(json!({"status": "done", "pr": {"num": 1, "stage": {"phase": "merge"}}})), card(json!({"status": "done", "pr": {"num": 2, "stage": {"phase": "checks"}}}))]}));
        assert_eq!(shipping.state, Some(("2 PRs awaiting merge".into(), "working")));
        assert_eq!(shipping.now_label, "Now");
    }

    #[test]
    fn today_drops_vs_usual_and_colors_the_change() {
        let days = json!({"day": {"agent_min": 400.0, "human_min": 1260.0, "est_agent_min": 390.0, "done": 5.0, "prs": 4.0, "wait_min": 38.0},
            "usual": {"days": 10, "agent_min": 330.0, "done": 3.0, "prs": 3.0, "wait_min": 26.0}});
        let st = stats_vm(&days);
        let find = |l: &str| st.iter().find(|x| x.label == l).cloned().unwrap();
        assert_eq!(find("Agent time").delta, "");
        assert_eq!((find("Tasks done").delta.as_str(), find("Tasks done").trend), ("+2", Trend::Good));
        assert_eq!((find("Waiting on you").delta.as_str(), find("Waiting on you").trend), ("+12m", Trend::Bad));
        assert_eq!(find("Human estimate").delta, "3.2× agent time");
    }
}
