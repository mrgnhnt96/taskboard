//! The home page, Needs you (`GET /home` and today's numbers from `GET /days`): everything that
//! waits on you, the questions to answer first (answered right here) and then the rest to act on,
//! each with the one signal it needs, under today's numbers and a project switch.
//!
//! What each part shows is worked out by the pure `*_vm` functions; the render code only lays it
//! out. The page is read-only: an answer, a signal (Restart, Start, Open PR, Continue, Focus in
//! Midna) or a dismiss is all it sends. An item opens in place; its title links to its page.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, b, i, obj, opt_s, s};
use crate::theme::Theme;
use crate::ui::issue_panel as ip;
use crate::ui::kit;
use crate::ui::status::St;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

const PAGE_X: f32 = 36.;
const PROJECT_FILTER: &str = "home-project";

thread_local! {
    /// The answer fields by task ref: a draft outlives renders and refetches until it's sent.
    static FIELDS: RefCell<HashMap<String, kit::Input>> = RefCell::new(HashMap::new());
    /// Items opened or shut against their default (a question starts open, the rest shut).
    static FLIPPED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

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
    /// A goal held after a wave for your review: `POST /goals/:id/waves/:n/continue`.
    Continue(String, i64),
    OpenPr(String),
    OpenTask(String),
    OpenGoal(String),
    /// A weekly review is ready: its Review page, which marks it seen.
    OpenReview(i64),
}

impl Act {
    pub fn label(&self) -> &'static str {
        match self {
            Act::Restart(_) => "Restart",
            Act::Start(_) => "Start",
            Act::Focus(_) => "Focus in Midna",
            Act::Continue(..) => "Continue",
            Act::OpenPr(_) => "Open PR",
            Act::OpenTask(_) => "Open task",
            Act::OpenGoal(_) => "Open goal",
            Act::OpenReview(_) => "Open review",
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
    /// What the row says beside its ref: the task's title, or the wave that stopped.
    pub title: String,
    /// Under the title: the PR's state, or what the alert said.
    pub line: String,
    pub project: String,
    pub wave: Option<i64>,
    /// When it was raised (ISO), for the order and "oldest".
    pub at: String,
    /// "41m", "3h", "now".
    pub time: String,
    pub act: Option<Act>,
    /// A task waiting on your answer: it goes under Answer with a field of its own.
    pub answer: bool,
    /// Review and urgent alerts stay until what raised them clears.
    pub dismiss: bool,
    /// The alert's fixes as (label, command), a button each beside Dismiss.
    pub actions: Vec<(String, String)>,
}

impl Ask {
    /// The item's key for its element ids and open state.
    pub fn key(&self) -> String {
        if !self.id.is_empty() {
            return self.id.clone();
        }
        self.task.as_ref().or(self.goal.as_ref()).map(|(r, _)| r.clone()).unwrap_or_default()
    }
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
    ask_rest(a).to_string()
}

/// The alert's text less the "T12: " its task's ref already says.
fn ask_rest(a: &Value) -> &str {
    let text = s(a, "text");
    let rest = text.strip_prefix(s(a, "task")).map(|x| x.trim_start_matches(':').trim_start());
    rest.filter(|x| !x.is_empty()).unwrap_or(text)
}

fn pr_url(card: &Value) -> Option<String> {
    obj(card, "pr").and_then(|p| opt_s(p, "url")).map(str::to_string)
}

fn phase(card: &Value) -> &str {
    obj(card, "pr").and_then(|p| obj(p, "stage")).map(|st| s(st, "phase")).unwrap_or("")
}

/// A lost task's newest sign that its work finished anyway: "Tag v0.2.0 · 4:21 PM".
pub fn looks_finished(card: &Value) -> Option<String> {
    let found = card["looks_finished"].as_array()?;
    found.last().map(|f| format!("{} · {}", s(f, "what"), fmt::hhmm(s(f, "at"))))
}

pub fn ask_vm(a: &Value) -> Ask {
    let card = &a["card"];
    let r = opt_s(a, "task").map(str::to_string);
    let has_card = card.is_object();
    let reason = s(card, "needs_reason");
    let (sev, kind, act): (Sev, &str, Option<Act>) = if let Some(id) = a["review_id"].as_i64() {
        (Sev::Ok, "Weekly review", Some(Act::OpenReview(id)))
    } else if a["urgent"] == true {
        (Sev::Crit, "Urgent", r.clone().map(Act::OpenTask))
    } else if has_card && b(card, "lost") && looks_finished(card).is_some() {
        (Sev::Warn, "Looks finished", r.clone().map(Act::OpenTask))
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
    let goal_name = opt_s(a, "goal_name").filter(|n| !n.is_empty()).or_else(|| obj(card, "goal").and_then(|g| opt_s(g, "name"))).unwrap_or("");
    let answer = matches!(kind, "Question" | "Needs you") && has_card && s(card, "status") == "needs";
    // Whether this alert is the agent's question itself, not another alert on a task that waits on you.
    let asks_this = answer && opt_s(card, "question").is_some_and(|q| ask_rest(a) == q);
    // An alert with a title of its own (a setup alert's, or Haiku's) says it, with its detail line
    // under it. A card you answer keeps the whole question for its body, where it's answered.
    let own_title = opt_s(a, "title").filter(|x| !x.is_empty() && (!answer || asks_this));
    let detail = opt_s(a, "detail").filter(|d| !d.is_empty() && own_title.is_some());
    let text = match own_title {
        _ if kind == "Looks finished" => looks_finished(card).unwrap_or_default(),
        Some(_) if !answer => detail.unwrap_or(s(a, "text")).to_string(),
        _ => ask_text(a, card),
    };
    let pr_open = obj(card, "pr").is_some_and(|p| !p["num"].is_null() && crate::ui::goal::pr_open(p));
    let title = if let Some(t) = own_title {
        t.to_string()
    } else if has_card {
        s(card, "title").to_string()
    } else if !goal_name.is_empty() {
        goal_name.to_string()
    } else {
        text.clone()
    };
    Ask {
        id: s(a, "id").to_string(),
        sev,
        kind: kind.to_string(),
        // On a card you answer, only the question's own suggestions are answers to it.
        actions: if answer && !asks_this { Vec::new() } else { crate::app::alert_actions(a) },
        line: if pr_open {
            tile_vm(card).line
        } else if let Some(d) = detail {
            d.to_string()
        } else if title == text {
            String::new()
        } else {
            text.clone()
        },
        text,
        title,
        task: r.map(|r| (r, s(card, "title").to_string())),
        goal: goal_ref.map(|g| (g, goal_name.to_string())),
        project: opt_s(card, "project").or(opt_s(a, "project")).unwrap_or("").to_string(),
        wave: card["wave"].as_i64(),
        at: s(a, "at").to_string(),
        time: short_ago(s(a, "at")),
        act,
        answer,
        // The board keeps these until they're handled (`dispatch::stays`).
        dismiss: !s(a, "id").is_empty() && a["review"] != true && a["urgent"] != true && a["quiet"] != true,
    }
}

/// A goal held after a wave for your review (its `stopped` line, "Wave 1 is done. Review it,
/// then continue"): Continue lets the next wave start.
fn stop_vm(e: &Value) -> Option<Ask> {
    let g = &e["goal"];
    let line = opt_s(g, "stopped").filter(|l| l.contains(" is done"))?;
    let n: i64 = line.strip_prefix("Wave ")?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    let (gr, name) = (fmt::ref_of(g, "G"), s(g, "name").to_string());
    let queued = i(e, "queued");
    let waiting = match queued {
        q if q <= 0 || e["wave"].as_i64() != Some(n + 1) => String::new(),
        1 => format!("1 task waits in wave {}", n + 1),
        q => format!("{q} tasks wait in wave {}", n + 1),
    };
    let at = s(g, "updated_at").to_string();
    Some(Ask {
        id: format!("stop-{gr}-{n}"),
        sev: Sev::Warn,
        kind: "Wave stop".into(),
        text: line.to_string(),
        task: None,
        title: format!("{name} · wave {n} finished"),
        line: waiting,
        goal: Some((gr.clone(), name)),
        project: s(e, "project").to_string(),
        wave: Some(n),
        time: short_ago(&at),
        at,
        act: Some(Act::Continue(gr, n)),
        answer: false,
        dismiss: false,
        actions: Vec::new(),
    })
}

/// The Needs you items, alerts and wave stops: the worst first, then the newest.
pub fn asks_vm(home: &Value) -> Vec<Ask> {
    let mut v: Vec<Ask> = arr(home, "needs").iter().map(ask_vm).chain(arr(home, "goals").iter().filter_map(stop_vm)).collect();
    v.sort_by(|a, b| (a.sev as u8).cmp(&(b.sev as u8)).then(b.at.cmp(&a.at)));
    v
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stat {
    pub label: &'static str,
    pub value: String,
    /// Time you kept the agents waiting: in the Needs you color.
    pub warn: bool,
}

fn num(v: &Value, k: &str) -> f64 {
    v[k].as_f64().unwrap_or(0.0)
}

fn count(x: f64) -> String {
    let r = (x * 10.0).round() / 10.0;
    if r.fract() == 0.0 { format!("{}", r as i64) } else { format!("{r}") }
}

/// Today's numbers from `GET /days`, and the week's saved time when there is a week.
pub fn stats_vm(days: &Value) -> Vec<Stat> {
    let day = &days["day"];
    let dur = crate::ui::days::dur;
    let stat = |label, value, warn| Stat { label, value, warn };
    let mut out = vec![
        stat("Agent time today", dur(num(day, "agent_min")), false),
        stat("Done today", count(num(day, "done")), false),
        stat("PRs opened", count(num(day, "prs")), false),
        stat("Waiting on you", dur(num(day, "wait_min")), num(day, "wait_min") > 0.0),
    ];
    if let Some(v) = crate::ui::days::View::of(days) {
        out.push(stat("Saved this week", v.saved.total.clone(), false));
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
            // A GitHub login is short; a Bitbucket `{uuid}` or `account_id` is not a name.
            let handle = opt_s(r, "user").filter(|u| !u.is_empty() && !u.starts_with('{') && !u.contains(':'));
            let name = handle.or(opt_s(r, "name")).unwrap_or("").to_string();
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
        "needs" if b(c, "lost") && looks_finished(c).is_some() => ("Looks finished", "needs", looks_finished(c).unwrap_or_default()),
        "needs" if b(c, "lost") => ("Lost", "down", format!("Terminal closed {}", fmt::hhmm(s(c, "when")))),
        "needs" if s(c, "needs_reason") == "start_failed" => ("Couldn’t start", "down", "Midna didn’t start it".into()),
        "needs" if s(c, "needs_reason") == "compact_failed" => ("Couldn’t compact", "down", "Its conversation didn’t compact".into()),
        "needs" if s(c, "needs_reason") == "question" => ("Question", "needs", since("Asked")),
        "needs" => ("Needs you", "needs", since("Updated")),
        "working" if c["compacting"].is_string() => ("Compacting", "working", fmt::compacting(c).unwrap_or_default()),
        "working" if c["stuck"].is_object() => ("Maybe stuck", "needs", fmt::stuck(c).unwrap_or_default()),
        "working" => ("Working", "working", busy(c)),
        "queued" if c["compacting"].is_string() => ("Compacting", "working", fmt::compacting(c).unwrap_or_default()),
        "queued" if b(c, "starting") =>("Starting", "working", opt_s(c, "waiting").unwrap_or("Opening a terminal").to_string()),
        "queued" => ("Queued", "queued", opt_s(c, "waiting").unwrap_or("When a terminal is free").to_string()),
        // A merged or closed PR has nothing left to wait on: the task is plain Done.
        "done" if pr.is_some_and(crate::ui::goal::pr_open) => {
            let p = pr.unwrap_or(&Value::Null);
            let stage = obj(p, "stage");
            let label = stage.map(|st| s(st, "label")).filter(|l| !l.is_empty());
            let num = pr_num.clone().unwrap_or_default();
            let with = |l: Option<&str>| l.map(|l| format!("{num} · {l}")).unwrap_or(num.clone());
            if stage.and_then(|st| obj(st, "stopped")).is_some() {
                ("Needs you", "needs", with(label))
            } else if stage.is_some_and(|st| b(st, "setup")) {
                // Held at review with nobody who could review it: the project's setup is what's missing.
                ("Needs setup", "needs", with(label))
            } else {
                match stage.map(|st| s(st, "phase")).unwrap_or("") {
                    "checks" => ("Checks running", "goal", with(label)),
                    "fix" => ("Checks failed", "down", with(label)),
                    "comments" => ("Comments", "goal", num.clone()),
                    "merge" => ("Ready to merge", "up", num.clone()),
                    // Partly approved: "PR #119 · 1 of 2 approved", out of the approvals it needs.
                    _ => {
                        let have = p["bar"]["approvals"].as_i64().unwrap_or(0);
                        match p["bar"]["need"].as_i64() {
                            Some(need) if have > 0 && have < need => ("In review", "goal", format!("{num} · {have} of {need} approved")),
                            _ => ("In review", "goal", num.clone()),
                        }
                    }
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

// ================================================================== signals

fn sent(m: &mut MainWindow, text: String, cx: &mut Context<MainWindow>) {
    m.toast(text, false, cx);
}

pub fn run_act(m: &mut MainWindow, act: &Act, cx: &mut Context<MainWindow>) {
    match act.clone() {
        Act::Restart(r) => m.post(format!("tasks/{r}/resume"), json!({"mode": "fresh"}), cx, move |m, _, cx| sent(m, format!("Restarting {r}"), cx)),
        Act::Start(r) => m.post(format!("tasks/{r}/start"), json!({"mode": "new"}), cx, move |m, _, cx| sent(m, format!("Starting {r}"), cx)),
        Act::Focus(r) => m.post(format!("tasks/{r}/focus"), json!({}), cx, move |m, _, cx| sent(m, format!("Showing {r}’s terminal in Midna"), cx)),
        Act::Continue(g, n) => m.post(format!("goals/{g}/waves/{n}/continue"), json!({}), cx, move |m, v, cx| {
            let note = if v["let_start"] == true { format!("Wave {} can start", n + 1) } else { "The goal goes on".into() };
            sent(m, note, cx)
        }),
        Act::OpenPr(url) => cx.open_url(&url),
        Act::OpenTask(r) => m.open_task(r, cx),
        Act::OpenGoal(g) => m.go(Page::Goal(g), cx),
        Act::OpenReview(id) => crate::ui::review::open(m, Some(id), cx),
    }
}

/// Dismiss on a Needs you item: `POST /alerts/:id/dismiss`, gone from the list at once.
pub fn dismiss(m: &mut MainWindow, id: &str, cx: &mut Context<MainWindow>) {
    if let Some(needs) = m.data.home.as_mut().and_then(|h| h.get_mut("needs")).and_then(Value::as_array_mut) {
        needs.retain(|a| s(a, "id") != id);
    }
    crate::app::dismiss_alert(m, id, cx);
    cx.notify();
}

/// When a held answer goes out ("Thu 6am"), while work hours are closed; "" otherwise.
fn hours_open_at(state: &Value) -> String {
    let h = &state["work_hours"];
    if !h.is_object() || !b(h, "on") || b(h, "open") {
        return String::new();
    }
    opt_s(h, "next_open").map(fmt::day_clock).unwrap_or_default()
}

fn sent_note(state: &Value) -> &'static str {
    if state["midna"]["up"] == Value::Bool(false) { "Saved. It runs once Midna is back." } else { "Sent to Midna" }
}

/// Send what's in a question's field: `POST /tasks/:id/answer`, now or when work hours open.
fn answer(m: &mut MainWindow, r: &str, morning: bool, cx: &mut Context<MainWindow>) {
    let text = FIELDS.with(|f| f.borrow().get(r).map(|i| i.text(cx))).unwrap_or_default().trim().to_string();
    if text.is_empty() {
        m.toast("Write an answer first.", true, cx);
        return;
    }
    let key = r.to_string();
    let body = json!({"text": text, "when": if morning { "morning" } else { "now" }});
    m.post(format!("tasks/{r}/answer"), body, cx, move |m, _, cx| {
        FIELDS.with(|f| f.borrow_mut().remove(&key));
        let note = if morning {
            let at = hours_open_at(m.state());
            format!("Saved. It sends at {}.", if at.is_empty() { "the start of work hours".to_string() } else { at })
        } else {
            sent_note(m.state()).to_string()
        };
        sent(m, note, cx);
    });
}

fn is_open(a: &Ask) -> bool {
    a.answer != FLIPPED.with(|f| f.borrow().contains(&a.key()))
}

fn flip(key: String) {
    FLIPPED.with(|f| {
        let mut f = f.borrow_mut();
        if !f.remove(&key) {
            f.insert(key);
        }
    });
}

// ================================================================== render

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let home = m.data.home.clone().unwrap_or(Value::Null);
    // A break's urgent alert is already its "Master is red" line.
    let st = m.state().clone();
    let needs: Vec<Value> = arr(&home, "needs").iter().filter(|a| !crate::ui::prwatch::shown_as_master(&st, a)).cloned().collect();
    let all = asks_vm(&json!({ "needs": needs, "goals": home["goals"] }));
    let stats = m.data.today.as_ref().map(stats_vm).unwrap_or_default();

    let project = m.filters.project.clone();
    let names = crate::app::project_names(&st, arr(&home, "goals").iter().map(|e| &e["goal"]));
    let asks: Vec<&Ask> = all.iter().filter(|a| project == "all" || a.project == project).collect();
    let (answers, acts): (Vec<&Ask>, Vec<&Ask>) = asks.iter().partition(|a| a.answer);

    let mut page = div().flex().flex_col().px(px(PAGE_X)).pt(px(22.)).pb(px(32.)).line_height(relative(1.4));

    // Needs you · how many and the oldest, then which project.
    let oldest = asks.iter().filter(|a| !a.at.is_empty()).min_by(|a, b| a.at.cmp(&b.at)).map(|a| a.time.clone());
    let sub = match (asks.len(), oldest) {
        (0, _) => String::new(),
        (n, Some(o)) => format!("{n} · oldest {o}"),
        (n, None) => n.to_string(),
    };
    let counts: Vec<(String, usize)> = names.iter().map(|p| (p.clone(), all.iter().filter(|a| &a.project == p).count())).collect();
    let tabs = ip::project_filter(&t, PROJECT_FILTER, &project, &counts, cx, |m, p, cx| {
        let mut f = m.filters.clone();
        f.project = p;
        m.set_filters(f, cx);
    });
    page = page.child(
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).text_color(t.text).child("Needs you"))
            .when(!sub.is_empty(), |d| d.child(div().text_size(px(13.)).text_color(t.muted).child(sub)))
            .child(div().flex_1())
            .when(names.len() > 1, |d| d.child(tabs)),
    );

    if !stats.is_empty() {
        let mut row = div().flex().gap(px(10.)).mt(px(16.));
        for x in &stats {
            row = row.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .px(px(16.))
                    .py(px(12.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(t.border)
                    .bg(t.card)
                    .child(div().text_size(px(20.)).font_weight(FontWeight::BOLD).text_color(if x.warn { t.warn_fg } else { t.text }).child(x.value.clone()))
                    .child(div().text_size(px(12.)).text_color(t.muted).truncate().child(x.label)),
            );
        }
        page = page.child(row);
    }

    if m.data.home.is_none() {
        page = page.child(kit::empty(&t, if m.down.is_some() { "Can’t load the board." } else { "Loading…" }));
    } else if asks.is_empty() {
        page = page.child(kit::empty(&t, "Nothing needs you.").pt(px(32.)));
    }
    if !answers.is_empty() {
        page = page.child(group_label(&t, "Answer", answers.len()));
        for a in &answers {
            page = page.child(item(m, &t, a, window, cx));
        }
    }
    if !acts.is_empty() {
        page = page.child(group_label(&t, "Act on", acts.len()));
        for a in &acts {
            page = page.child(item(m, &t, a, window, cx));
        }
    }
    let filter = ip::render_project_filter(m, &t, PROJECT_FILTER, window, cx);
    div().id("home-page").flex_1().min_w_0().h_full().overflow_y_scroll().bg(t.bg).child(page).children(filter).into_any_element()
}

fn group_label(t: &Theme, text: &str, n: usize) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .pt(px(18.))
        .pb(px(8.))
        .text_size(px(12.))
        .text_color(t.muted)
        .child(div().font_weight(FontWeight::BOLD).child(text.to_uppercase()))
        .child(n.to_string())
}

/// The kind's color: a question in the Needs you color, the rest by what they are.
fn kind_color(t: &Theme, a: &Ask) -> Hsla {
    let st = match (a.answer, a.kind.as_str(), a.sev) {
        (true, ..) => St::Needs,
        (_, "Wave stop", _) => return t.goal,
        (_, "Review the PR", _) => St::Review,
        (_, "Ready to merge", _) => St::Done,
        (_, _, Sev::Crit) => St::Failed,
        (_, _, Sev::Warn) => St::Needs,
        (_, _, Sev::Ok) => St::Done,
    };
    st.colors(t).0
}

/// One item: its head row (open or shut on a click, its title a link to its page) and, open, a
/// question's answer field or the rest of what the alert said.
fn item(m: &MainWindow, t: &Theme, a: &Ask, window: &Window, cx: &mut Context<MainWindow>) -> Div {
    let key = a.key();
    let open = is_open(a);
    let id = |part: &str| SharedString::from(format!("home-{part}-{key}"));
    let question = a.answer;

    // The ref and title, and under them the PR's state or what the alert said.
    let (r, link_act) = match (&a.task, &a.goal) {
        (Some((r, _)), _) => (r.clone(), Act::OpenTask(r.clone())),
        (None, Some((g, _))) => (g.clone(), Act::OpenGoal(g.clone())),
        _ => (String::new(), Act::OpenTask(String::new())),
    };
    let link = {
        let act = link_act.clone();
        div()
            .id(id("title"))
            .min_w_0()
            .truncate()
            .text_size(px(if question { 15. } else { 14. }))
            .font_weight(FontWeight::BOLD)
            .text_color(t.text)
            .cursor_pointer()
            .hover(|s| s.underline())
            .child(a.title.clone())
            .when(!r.is_empty(), |d| {
                d.on_click(cx.listener(move |m, _, _, cx| {
                    cx.stop_propagation();
                    run_act(m, &act, cx);
                }))
            })
    };
    let title = div()
        .flex()
        .flex_col()
        .flex_1()
        .flex_basis(px(200.))
        .min_w(px(80.))
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(8.))
                .min_w_0()
                .when(!r.is_empty(), |d| d.child(div().flex_none().font_family(t.mono_font.clone()).text_size(px(11.)).text_color(t.muted).child(r.clone())))
                .child(link),
        )
        .when(!a.line.is_empty() && !(question && a.line == a.text), |d| d.child(div().text_size(px(12.)).text_color(t.muted).truncate().child(a.line.clone())));

    let goal = match (&a.goal, &a.task) {
        (Some((_, name)), _) if !name.is_empty() => match a.wave.filter(|_| question) {
            Some(n) => format!("{name} · wave {n}"),
            None => name.clone(),
        },
        (_, Some(_)) => "Not in a goal".to_string(),
        _ => String::new(),
    };

    // An open question has its signals under its field.
    let button = a.act.clone().filter(|_| !(question && open)).map(|act| {
        kit::btn(t, id("act"), act.label()).h(px(30.)).px(px(12.)).rounded(px(7.)).text_size(px(12.)).on_click(cx.listener(move |m, _, _, cx| {
            cx.stop_propagation();
            run_act(m, &act, cx);
        }))
    });

    let head = div()
        .id(id("head"))
        .flex()
        .items_center()
        .gap(px(18.))
        .px(px(20.))
        .py(px(13.))
        .cursor_pointer()
        .on_click(cx.listener({
            let key = key.clone();
            move |_, _, _, cx| {
                flip(key.clone());
                cx.notify();
            }
        }))
        .child(div().flex_none().w(px(20.)).flex().justify_center().child(chevron(open, t.muted)))
        .child(div().flex_shrink(1.).w(px(130.)).min_w(px(64.)).text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(kind_color(t, a)).truncate().child(a.kind.clone()))
        .child(title)
        .child(div().flex_shrink(1.).min_w_0().max_w(px(260.)).text_size(px(12.)).text_color(if goal == "Not in a goal" { t.muted } else { t.goal }).truncate().child(goal))
        .child(div().flex_none().w(px(56.)).flex().justify_end().text_size(px(12.)).text_color(t.muted).child(a.time.clone()))
        .child(div().flex_none().w(px(110.)).h(px(30.)).flex().justify_end().children(button));

    let mut card = div()
        .flex()
        .flex_col()
        .mb(px(8.))
        .rounded(px(12.))
        .border_1()
        .border_color(if question { t.warn_line } else { t.border })
        .bg(t.card)
        .overflow_hidden()
        .child(head);
    if !open {
        return card;
    }
    let body = div().flex().flex_col().gap(px(10.)).pl(px(58.)).pr(px(20.)).pb(px(16.));
    if question {
        let r = a.task.as_ref().map(|(r, _)| r.clone()).unwrap_or_default();
        let field = FIELDS.with(|f| {
            let mut f = f.borrow_mut();
            let input = f.entry(r.clone()).or_insert_with(|| kit::Input::new(cx, "", true));
            input.render(t, id("field"), window).items_start().min_h(px(62.)).px(px(12.)).py(px(10.)).text_size(px(14.))
        });
        let field = field.on_key_down(cx.listener({
            let r = r.clone();
            move |m, ev: &KeyDownEvent, _, cx| {
                let submit = FIELDS.with(|f| f.borrow().get(&r).map(|i| i.on_key(ev, cx)));
                if matches!(submit, Some(kit::KeyOutcome::Submit)) {
                    cx.stop_propagation();
                    answer(m, &r, false, cx);
                }
            }
        }));
        let later = hours_open_at(m.state());
        let send = {
            let r = r.clone();
            kit::btn_primary(t, id("send"), "Send answer").on_click(cx.listener(move |m, _, _, cx| answer(m, &r, false, cx)))
        };
        let send_later = (!later.is_empty()).then(|| {
            let r = r.clone();
            kit::btn(t, id("later"), format!("Send at {later}")).on_click(cx.listener(move |m, _, _, cx| answer(m, &r, true, cx)))
        });
        let focus = kit::btn(t, id("focus"), Act::Focus(r.clone()).label()).ml_auto().on_click(cx.listener({
            let r = r.clone();
            move |m, _, _, cx| run_act(m, &Act::Focus(r.clone()), cx)
        }));
        // The suggested answers: a click puts one in the field to edit and send.
        let quick: Vec<_> = a
            .actions
            .iter()
            .enumerate()
            .map(|(n, (label, _))| {
                let (r, label) = (r.clone(), label.clone());
                kit::btn(t, id(&format!("quick-{n}")), label.clone()).on_click(cx.listener(move |_, _, window, cx| {
                    FIELDS.with(|f| {
                        if let Some(input) = f.borrow().get(&r) {
                            input.set_text(&label, cx);
                            window.focus(&input.focus, cx);
                        }
                    });
                    cx.notify();
                }))
            })
            .collect();
        let quick = (!quick.is_empty()).then(|| div().flex().flex_wrap().gap(px(8.)).children(quick));
        card = card.child(
            body.child(div().text_size(px(15.)).text_color(t.warn_text).child(a.text.clone()))
                .children(quick)
                .child(div().flex().flex_col().gap(px(6.)).child(div().text_size(px(12.)).text_color(t.muted).child("Your answer")).child(field))
                .child(div().flex().items_center().gap(px(8.)).child(send).children(send_later).child(focus)),
        );
    } else {
        let dismiss_btn = a.dismiss.then(|| {
            let aid = a.id.clone();
            kit::btn(t, id("dismiss"), "Dismiss").on_click(cx.listener(move |m, _, _, cx| dismiss(m, &aid, cx)))
        });
        let fixes: Vec<_> = a
            .actions
            .iter()
            .enumerate()
            .map(|(n, (label, command))| {
                let (aid, label, command) = (a.id.clone(), label.clone(), command.clone());
                kit::btn(t, id(&format!("fix-{n}")), label.clone()).on_click(cx.listener(move |m, _, _, cx| crate::app::run_alert_action(m, &aid, n, &label, &command, cx)))
            })
            .collect();
        let buttons = (!fixes.is_empty() || dismiss_btn.is_some()).then(|| div().flex().flex_wrap().gap(px(8.)).children(fixes).children(dismiss_btn));
        card = card.child(body.child(div().text_size(px(13.)).text_color(t.text_2).child(a.text.clone())).children(buttons));
    }
    card
}

/// A 16px chevron: right shut, down open.
fn chevron(open: bool, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let k = b.size.width.as_f32() / 24.;
            let u = |x: f32, y: f32| point(b.origin.x + px(x * k), b.origin.y + px(y * k));
            let pts = if open { [(6., 9.), (12., 15.), (18., 9.)] } else { [(9., 6.), (15., 12.), (9., 18.)] };
            let mut p = PathBuilder::stroke(px(1.8));
            p.move_to(u(pts[0].0, pts[0].1));
            for (x, y) in &pts[1..] {
                p.line_to(u(*x, *y));
            }
            if let Ok(p) = p.build() {
                window.paint_path(p, color);
            }
        },
    )
    .size(px(16.))
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
                "bar": {"reviewer_rows": [{"name": "Jo Lee", "user": "jlee", "state": "approved"}, {"name": "M Park", "user": "mpark", "state": "waiting"}, {"name": "K", "user": "kwong", "state": "changes"},
                    {"name": "Sam Ortiz", "user": "{52009081-5c95-408e-8570-0231381f3dec}", "state": "waiting"}]}}}))
        };
        let t = tile_vm(&pr("review"));
        assert_eq!((t.status.as_str(), t.line.as_str()), ("In review", "PR #119"));
        assert_eq!(t.reviewers.iter().map(|r| (r.name.as_str(), r.state)).collect::<Vec<_>>(), vec![("jlee", Review::Approved), ("mpark", Review::Pending), ("kwong", Review::Changes), ("Sam Ortiz", Review::Pending)]);
        assert_eq!(t.reviewers[0].initials, "JL");
        assert_eq!(tile_vm(&pr("checks")).line, "PR #119 · Watching checks");
        assert_eq!(tile_vm(&pr("merge")).status, "Ready to merge");
        assert_eq!(tile_vm(&pr("fix")).tone, "down");
    }

    #[test]
    fn a_partly_approved_pr_tile_counts_its_approvals_against_the_ones_needed() {
        let pr = |approvals: i64, need: Value| {
            card(json!({"status": "done", "pr": {"num": 119, "url": "https://x/pr/119", "state": "OPEN", "stage": {"phase": "review", "label": "Awaiting reviews"},
                "bar": {"approvals": approvals, "reviewers": 3, "need": need, "reviewer_rows": []}}}))
        };
        let t = tile_vm(&pr(1, json!(2)));
        assert_eq!((t.status.as_str(), t.line.as_str()), ("In review", "PR #119 · 1 of 2 approved"));
        assert_eq!(tile_vm(&pr(0, json!(2))).line, "PR #119", "none yet");
        assert_eq!(tile_vm(&pr(1, Value::Null)).line, "PR #119", "no count: the host decides");
    }

    #[test]
    fn a_pr_nobody_can_review_tile_needs_setup() {
        let t = tile_vm(&card(json!({"status": "done", "pr": {"num": 2, "url": "https://x/pr/2", "state": "OPEN",
            "stage": {"phase": "review", "label": "Stuck: no reviewers", "setup": true}}})));
        assert_eq!((t.status.as_str(), t.tone, t.line.as_str()), ("Needs setup", "needs", "PR #2 · Stuck: no reviewers"));
    }

    #[test]
    fn a_merged_pr_tile_is_done_not_in_review() {
        let merged = tile_vm(&card(json!({"status": "done", "pr": {"num": 195, "state": "MERGED", "stage": {"phase": "review"}}})));
        assert_eq!((merged.status.as_str(), merged.tone), ("Done", "up"));
        let closed = tile_vm(&card(json!({"status": "done", "pr": {"num": 195, "state": "CLOSED"}})));
        assert_eq!(closed.status, "Done");
    }

    #[test]
    fn needs_you_is_worst_first_with_one_thing_to_do() {
        let home = json!({"needs": [
            {"id": "a1", "at": "2026-10-09T10:00:00Z", "text": "T5: Keep the old form?", "task": "T5", "goal": "G2", "goal_name": "Login",
             "card": card(json!({"ref": "T5", "status": "needs", "needs_reason": "question", "question": "Keep the old form?", "title": "Form", "wave": 2}))},
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
    fn a_lost_task_whose_work_shipped_looks_finished() {
        let found = json!([{"what": "Commit abc123: chore: release v0.2.0", "at": "2026-10-09T21:21:00Z"},
                           {"what": "Release v0.2.0 published", "at": "2026-10-09T21:26:00Z", "url": "https://x/r"}]);
        let c = card(json!({"ref": "T6", "status": "needs", "lost": true, "looks_finished": found}));
        let ask = ask_vm(&json!({"id": "a1", "task": "T6", "text": "T6 lost its terminal.", "card": c}));
        assert_eq!((ask.kind.as_str(), ask.sev), ("Looks finished", Sev::Warn));
        assert_eq!(ask.act, Some(Act::OpenTask("T6".into())));
        let line = format!("Release v0.2.0 published · {}", fmt::hhmm("2026-10-09T21:26:00Z"));
        assert_eq!(ask.text, line);
        let tile = tile_vm(&c);
        assert_eq!((tile.status.as_str(), tile.tone, tile.line.as_str()), ("Looks finished", "needs", line.as_str()));
    }

    #[test]
    fn a_ready_review_opens_its_page_and_stays_until_opened() {
        let home = json!({"needs": [
            {"id": "a9", "at": "2026-10-12T09:00:00Z", "text": "Weekly review: 10.0 h of agent time", "quiet": true, "review_id": 4,
             "url": "taskboard://review/4", "key": "review:4"}
        ]});
        let asks = asks_vm(&home);
        assert_eq!(asks[0].kind, "Weekly review");
        assert_eq!(asks[0].act, Some(Act::OpenReview(4)));
        assert_eq!(asks[0].act.as_ref().map(Act::label), Some("Open review"));
        assert!(!asks[0].dismiss, "the board keeps it until the review is opened");
    }

    #[test]
    fn a_setup_alert_has_a_short_title_a_detail_line_and_its_fixes() {
        let home = json!({"needs": [
            {"id": "a3", "at": "2026-10-12T09:00:00Z", "text": "work-deck isn't set up to finish its work: a long sentence.", "key": "setup:work-deck",
             "title": "work-deck PRs can't merge", "detail": "T117 is waiting for 2 approvals, and work-deck has no reviewers", "project": "work-deck",
             "actions": [{"label": "Turn off review for work-deck", "prompt": "p", "command": "tb project set work-deck --review off"},
                         {"label": "Add a reviewer", "prompt": "p", "command": "tb reviewers add <name> --user <host id> --project work-deck"}]},
            {"id": "a4", "at": "2026-10-12T08:00:00Z", "text": "Something plain happened"}
        ]});
        let asks = asks_vm(&home);
        assert_eq!(asks[0].title, "work-deck PRs can't merge");
        assert_eq!(asks[0].line, "T117 is waiting for 2 approvals, and work-deck has no reviewers");
        assert_eq!(asks[0].text, asks[0].line);
        assert_eq!(asks[0].project, "work-deck");
        assert_eq!(asks[0].actions.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), vec!["Turn off review for work-deck", "Add a reviewer"]);
        assert!(asks[0].dismiss);
        // An old plain alert reads as before.
        assert_eq!((asks[1].title.as_str(), asks[1].line.as_str(), asks[1].text.as_str()), ("Something plain happened", "", "Something plain happened"));
        assert!(asks[1].actions.is_empty());
    }

    #[test]
    fn a_suggested_title_and_its_answers_show_on_a_task_card() {
        let actions = json!([{"label": "Keep the old form", "prompt": "p", "command": ""}, {"label": "Drop it", "prompt": "p", "command": ""}]);
        let home = json!({"needs": [
            {"id": "a1", "at": "2026-10-09T10:00:00Z", "text": "T5: Keep the old form? It still has users.", "task": "T5", "suggest": "done",
             "title": "T5 asks about the old form", "detail": "The new login form is done; the old one still has users.", "actions": actions,
             "card": card(json!({"ref": "T5", "status": "needs", "needs_reason": "question", "question": "Keep the old form? It still has users.", "title": "Form"}))},
            {"id": "a2", "at": "2026-10-09T09:00:00Z", "text": "T6: the staging key expired", "task": "T6", "suggest": "done",
             "title": "Staging key expired", "detail": "T6 can't deploy until the key is renewed.", "actions": [{"label": "Renew the key", "prompt": "p", "command": ""}],
             "card": card(json!({"ref": "T6", "status": "working", "title": "Deploy"}))}
        ]});
        let asks = asks_vm(&home);
        let q = asks.iter().find(|a| a.kind == "Question").unwrap();
        assert!(q.answer);
        assert_eq!(q.title, "T5 asks about the old form");
        assert_eq!(q.line, "The new login form is done; the old one still has users.");
        assert_eq!(q.text, "Keep the old form? It still has users.", "the body keeps the whole question");
        assert_eq!(q.actions.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), vec!["Keep the old form", "Drop it"]);
        let raised = asks.iter().find(|a| a.id == "a2").unwrap();
        assert_eq!((raised.title.as_str(), raised.line.as_str()), ("Staging key expired", "T6 can't deploy until the key is renewed."));
        assert_eq!(raised.task, Some(("T6".into(), "Deploy".into())));
    }

    #[test]
    fn a_card_you_answer_keeps_its_question_over_another_alerts_suggestions() {
        let home = json!({"needs": [
            {"id": "a1", "at": "2026-10-09T10:00:00Z", "text": "T6: the staging key expired", "task": "T6", "suggest": "done",
             "title": "Staging key expired", "detail": "T6 can't deploy until the key is renewed.", "actions": [{"label": "Renew the key", "prompt": "p", "command": ""}],
             "card": card(json!({"ref": "T6", "status": "needs", "needs_reason": "question", "question": "Which region?", "title": "Deploy"}))},
            {"id": "a2", "at": "2026-10-09T09:00:00Z", "text": "T5: Keep the old form?", "task": "T5", "suggest": "done", "title": "T5 asks about the old form",
             "actions": [{"label": "Keep it", "prompt": "p", "command": ""}],
             "card": card(json!({"ref": "T5", "status": "needs", "needs_reason": "question", "question": "Keep the old form?", "title": "Form"}))}
        ]});
        let asks = asks_vm(&home);
        let other = asks.iter().find(|a| a.id == "a1").unwrap();
        assert!(other.answer);
        assert_eq!((other.title.as_str(), other.text.as_str()), ("Deploy", "Which region?"));
        assert!(other.actions.is_empty(), "another alert's fixes aren't answers to the question");
        // A suggested title with no detail: the line is the question, which the body already shows.
        let q = asks.iter().find(|a| a.id == "a2").unwrap();
        assert_eq!((q.title.as_str(), q.text.as_str()), ("T5 asks about the old form", "Keep the old form?"));
        assert_eq!(q.line, q.text);
        assert_eq!(q.actions.len(), 1);
    }

    #[test]
    fn a_question_is_answered_and_the_rest_acted_on() {
        let home = json!({"needs": [
            {"id": "a1", "at": "2026-10-09T10:00:00Z", "text": "T5: Keep the old form?", "task": "T5", "goal": "G2", "goal_name": "Login",
             "card": card(json!({"ref": "T5", "status": "needs", "needs_reason": "question", "question": "Keep the old form?", "title": "Form", "wave": 2}))},
            {"id": "a3", "at": "2026-10-09T11:00:00Z", "text": "PR #4 waits for your review", "task": "T8", "review": true,
             "card": card(json!({"ref": "T8", "status": "done", "pr": {"num": 4, "url": "https://x/pr/4", "state": "OPEN"}}))}
        ]});
        let asks = asks_vm(&home);
        let q = asks.iter().find(|a| a.kind == "Question").unwrap();
        assert!(q.answer);
        assert_eq!((q.title.as_str(), q.wave, q.project.as_str(), q.key().as_str()), ("Form", Some(2), "webapp", "a1"));
        let pr = asks.iter().find(|a| a.kind == "Review the PR").unwrap();
        assert!(!pr.answer);
        assert_eq!(pr.title, "Wait in panel");
    }

    #[test]
    fn a_wave_stop_waits_for_continue() {
        let home = json!({"needs": [], "goals": [
            {"project": "webapp", "wave": 2, "queued": 4,
             "goal": {"ref": "G5", "name": "Faster sign-in", "stopped": "Wave 1 is done. Review it, then continue", "updated_at": "2026-10-09T10:00:00Z"}},
            {"project": "api", "wave": 3, "queued": 1, "goal": {"ref": "G6", "name": "Quiet", "stopped": "T9 in wave 2 failed"}},
            {"project": "api", "wave": 1, "queued": 2, "goal": {"ref": "G7", "name": "Running"}}
        ]});
        let asks = asks_vm(&home);
        assert_eq!(asks.len(), 1);
        let a = &asks[0];
        assert_eq!((a.kind.as_str(), a.title.as_str(), a.line.as_str()), ("Wave stop", "Faster sign-in · wave 1 finished", "4 tasks wait in wave 2"));
        assert_eq!(a.act, Some(Act::Continue("G5".into(), 1)));
        assert_eq!(a.act.as_ref().map(Act::label), Some("Continue"));
        assert!(!a.dismiss && !a.answer);
        assert_eq!(a.project, "webapp");
    }

    #[test]
    fn today_has_five_numbers_and_waiting_is_loud() {
        let days = json!({"day": {"agent_min": 400.0, "done": 5.0, "prs": 4.0, "wait_min": 38.0}});
        let st = stats_vm(&days);
        assert_eq!(st.iter().map(|x| x.label).collect::<Vec<_>>()[..4], ["Agent time today", "Done today", "PRs opened", "Waiting on you"]);
        assert_eq!(st[1].value, "5");
        assert!(st[3].warn && !st[0].warn);
        let calm = stats_vm(&json!({"day": {"wait_min": 0.0}}));
        assert!(!calm[3].warn);
    }
}
