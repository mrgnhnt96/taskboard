//! A backlog issue, as the web board showed it: the side panel on the Board (`issuePanel`), and
//! the issue column beside the Backlog page's list (`issueAside`). Both share the detail
//! (`issueDetail`: move to a goal, how it was reported, what was happening, history, notes).
//!
//! Also here, shared with the Backlog page: the issue labels (`issueState`, `issueFrom`, …), the
//! issue actions (`promote`, `ticket`, `drop`, `issue-move`, `issue-note`) with the web board's
//! inline notes and "Sending…" busy buttons, and the searchable project / goal picker.
//!
//! What is drawn comes from the pure view models (`PanelView`, `AsideView`, `DetailView`); their
//! visible text and offered actions are checked against the web board in `parity/golden/backlog.json`.
use crate::app::{MainWindow, Page, Panel, TaskTab};
use crate::fmt::{self, arr, s};
use crate::theme::Theme;
use crate::ui::kit;
use crate::ui::modals::locale_cmp;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// `.panel`: `width: min(480px, 100vw)`.
pub const WIDTH: f32 = 480.;

/// Notes and busy buttons (the web board's `S.notes`, `S.busy` for issues), plus the open picker.
#[derive(Default)]
pub struct State {
    /// Inline notes by group (`issue:B1`): text, error, when set.
    pub notes: HashMap<String, (String, bool, Instant)>,
    /// Running calls by button key (`promote:board:B1`, `ticket::B1`, `drop::B1`).
    pub busy: HashSet<String>,
    pub picker: Option<Picker>,
}

// ------------------------------------------------------------------ labels

pub fn kind_label(kind: &str) -> String {
    match kind {
        "bug" => "Bug".into(),
        "gap" => "Test gap".into(),
        "follow" => "Follow-up".into(),
        "clean" => "Clean-up".into(),
        "" => "Issue".into(),
        k => fmt::cap(k),
    }
}

/// The kind chip's colors (`.k-bug` … in the web CSS).
pub fn kind_colors(t: &Theme, kind: &str) -> (Hsla, Hsla) {
    match kind {
        "bug" => (t.down, t.down_soft),
        "gap" => (t.warn_fg, t.warn_soft),
        "follow" => (t.accent_fg, t.accent_soft),
        _ => (t.text_2, t.col),
    }
}

/// The goal an issue belongs to: `goal_id`, else `goal.id` (`issueGoalId`).
pub fn goal_id(b: &Value) -> Option<i64> {
    b.get("goal_id").and_then(Value::as_i64).or_else(|| b.get("goal").and_then(|g| g.get("id")).and_then(Value::as_i64))
}

/// Every goal that isn't archived (`allGoals`).
pub fn live_goals(goals: &[Value]) -> Vec<&Value> {
    goals.iter().filter(|g| !fmt::b(g, "archived")).collect()
}

/// The task that reported it, as a ref (`foundTask`).
pub fn found_task(b: &Value) -> Option<String> {
    match b.get("found_by_task")? {
        Value::Null => None,
        v => Some(fmt::ref_of(v, "T")).filter(|r| !r.is_empty()),
    }
}

/// `issueState`: (short, text) for an issue that isn't open; None while open.
pub fn issue_state(b: &Value) -> Option<(String, String)> {
    let tr = b.get("task_id").and_then(Value::as_i64).map(|n| format!("T{n}"));
    match s(b, "state") {
        "task" => Some(("Now a task".into(), tr.map(|r| format!("Made into task {r}")).unwrap_or_else(|| "Made into a task".into()))),
        "ticket" => {
            let key = fmt::opt_s(b, "jira_key");
            Some((key.unwrap_or("Ticket asked for").to_string(), key.map(|k| format!("Jira {k}")).unwrap_or_else(|| "Jira ticket being created".into())))
        }
        "drop" => Some(("Won’t do".into(), "Closed as won’t do".into())),
        "defer" => Some(("Deferred".into(), "Deferred: not for now".into())),
        _ => None,
    }
}

/// `issueFrom`: "Found by T4 · api terminal · 3:05 PM" / "Added by you · Oct 7".
#[cfg(test)]
pub fn issue_from(b: &Value, long: bool) -> String {
    let ft = found_task(b);
    let source = s(b, "source");
    let name = fmt::opt_s(b, "found_by_name");
    let who = if source == "you" || (source.is_empty() && name.is_none() && ft.is_none()) {
        "Added by you".to_string()
    } else if source == "answer" {
        "From your answer".to_string()
    } else if let (true, Some(r)) = (long, ft.as_ref()) {
        format!("Found by {r} · {}", name.unwrap_or("a terminal"))
    } else {
        name.unwrap_or("Found by a terminal").to_string()
    };
    [who, fmt::hhmm(s(b, "created_at"))].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// `issueHow`: one sentence on how it was reported.
pub fn issue_how(b: &Value) -> String {
    if let Some(h) = fmt::opt_s(b, "how") {
        return h.to_string();
    }
    match s(b, "source") {
        "you" => "You added it.".into(),
        "answer" => "It came from one of your answers.".into(),
        _ => {
            let who = fmt::opt_s(b, "found_by_name").map(|n| format!("The {n} terminal")).unwrap_or_else(|| "A terminal".into());
            let on = found_task(b).map(|r| format!(" while working on {r}")).unwrap_or_default();
            format!("{who} reported it{on}.")
        }
    }
}

const SNAP_ORDER: [&str; 11] = ["from", "task", "step", "branch", "worktree", "last_commit", "commit", "uncommitted", "file", "last_turn", "output"];

fn snap_label(k: &str) -> String {
    match k {
        "from" => "From".into(),
        "task" => "Task".into(),
        "step" => "Step".into(),
        "branch" => "Branch".into(),
        "worktree" => "Worktree".into(),
        "last_commit" | "commit" => "Last commit".into(),
        "uncommitted" => "Not committed".into(),
        "file" => "File".into(),
        "last_turn" => "Last turn".into(),
        "output" => "Output".into(),
        "cwd" => "Folder".into(),
        k => fmt::cap(&k.replace('_', " ")),
    }
}

/// `snapRows`: the snapshot as labeled rows, in the web board's order.
pub fn snap_rows(snap: Option<&Value>) -> Vec<(String, String)> {
    let parsed;
    let snap = match snap {
        None | Some(Value::Null) => return vec![],
        Some(Value::String(x)) if x.is_empty() => return vec![],
        Some(Value::String(x)) => match serde_json::from_str::<Value>(x) {
            Ok(v) => {
                parsed = v;
                &parsed
            }
            Err(_) => return vec![("Snapshot".into(), x.clone())],
        },
        Some(v) => v,
    };
    let mut pairs: Vec<(String, Value)> = match snap {
        Value::Object(o) => o.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        Value::Array(a) => a
            .iter()
            .filter_map(Value::as_array)
            .map(|x| (x.first().map(|k| k.as_str().map(str::to_string).unwrap_or_else(|| k.to_string())).unwrap_or_default(), x.get(1).cloned().unwrap_or(Value::Null)))
            .collect(),
        _ => vec![],
    };
    if snap.is_object() {
        let rank = |k: &str| SNAP_ORDER.iter().position(|x| *x == k).unwrap_or(99);
        pairs.sort_by_key(|(k, _)| rank(k));
    }
    pairs
        .into_iter()
        .filter(|(_, v)| !v.is_null() && v.as_str() != Some(""))
        .map(|(k, v)| {
            let val = match (&*k, &v) {
                ("uncommitted", Value::Number(n)) if n.as_f64() == Some(0.) => "Nothing".into(),
                ("uncommitted", Value::Number(n)) => fmt::plural(n.as_i64().unwrap_or(0), "file", "files"),
                (_, Value::String(x)) => x.clone(),
                (_, Value::Object(_)) if !s(&v, "ref").is_empty() || !s(&v, "title").is_empty() => {
                    [s(&v, "ref"), s(&v, "title")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")
                }
                _ => v.to_string(),
            };
            (snap_label(&k), val)
        })
        .collect()
}

// ------------------------------------------------------------------ view models

/// A button the web board offered: its `data-act`, label ("Sending…" while it runs), `arg`.
#[derive(Clone, Debug, PartialEq)]
pub struct Act {
    pub act: &'static str,
    pub label: String,
    pub arg: &'static str,
    pub busy: bool,
}

/// What the views read besides the issue.
pub struct Ctx<'a> {
    pub jira: bool,
    pub goals: &'a [Value],
    pub notes: &'a HashMap<String, (String, bool, Instant)>,
    pub busy: &'a HashSet<String>,
}

impl<'a> Ctx<'a> {
    pub fn of(m: &'a MainWindow) -> Ctx<'a> {
        let st = &m.issue_panel;
        Ctx { jira: m.jira_on(), goals: &m.data.goals, notes: &st.notes, busy: &st.busy, }
    }

    pub fn note(&self, grp: &str) -> Option<(String, bool)> {
        self.notes.get(grp).map(|(t, e, _)| (t.clone(), *e))
    }

    /// `btn()`: label, or "Sending…" while `act:arg:id` runs.
    fn act(&self, act: &'static str, label: &str, arg: &'static str, id: &str) -> Act {
        let busy = self.busy.contains(&format!("{act}:{arg}:{id}"));
        Act { act, label: if busy { "Sending…".into() } else { label.into() }, arg, busy }
    }
}

/// `issueActions`: Make it a task (into `place`), Create ticket (with Jira), Won't do.
pub fn issue_actions(cx: &Ctx, r: &str, place: &'static str) -> Vec<Act> {
    let mut v = vec![cx.act("promote", "Make it a task", place, r)];
    if cx.jira {
        v.push(cx.act("ticket", "Create ticket", "", r));
    }
    v.push(cx.act("drop", "Won’t do", "", r));
    v
}

pub struct HistRow {
    pub time: String,
    pub full: String,
    pub kind: String,
    /// "who · text" (as the web joins them).
    pub text: String,
    /// Who did it (bold in the panel), and the rest after " · ".
    pub who: Option<String>,
    pub what: String,
}

/// `issueDetail`.
pub struct DetailView {
    pub r: String,
    pub how: String,
    pub said: Option<String>,
    pub detail: Option<String>,
    pub snap: Vec<(String, String)>,
    /// (task ref, "Open T4’s log at 3:05 PM").
    pub found_log: Option<(String, String)>,
    pub history: Vec<HistRow>,
}

pub fn detail_view(b: &Value) -> DetailView {
    let r = fmt::ref_of(b, "B");
    let mut hist: Vec<&Value> = arr(b, "history").iter().collect();
    let at = |h: &Value| h.get("at").map(|a| a.as_str().map(str::to_string).unwrap_or_else(|| a.to_string())).unwrap_or_else(|| "undefined".into());
    hist.sort_by(|x, y| locale_cmp(&at(x), &at(y)));
    DetailView {
        how: issue_how(b),
        said: fmt::opt_s(b, "said").map(str::to_string),
        detail: fmt::opt_s(b, "detail").map(str::to_string),
        snap: snap_rows(b.get("snapshot")),
        found_log: found_task(b).map(|fr| {
            let at = fmt::opt_s(b, "created_at").map(|c| format!(" at {}", fmt::hhmm(c))).unwrap_or_default();
            (fr.clone(), format!("Open {fr}’s log{at}"))
        }),
        history: hist
            .into_iter()
            .map(|h| HistRow {
                time: fmt::hhmm(s(h, "at")),
                full: fmt::full_time(s(h, "at")),
                kind: s(h, "kind").to_string(),
                who: fmt::opt_s(h, "who").map(str::to_string),
                what: s(h, "text").to_string(),
                text: match fmt::opt_s(h, "who") {
                    Some(w) => format!("{w} · {}", s(h, "text")),
                    None => s(h, "text").to_string(),
                },
            })
            .collect(),
        r,
    }
}

impl DetailView {
    #[cfg(test)]
    pub fn parts(&self, out: &mut Vec<String>) {
        out.push("How it was reported".into());
        out.push(self.how.clone());
        out.extend(self.said.clone());
        out.extend(self.detail.clone());
        out.push("What was happening".into());
        if self.snap.is_empty() {
            out.push("No snapshot was saved with this issue.".into());
        }
        for (k, v) in &self.snap {
            out.push(k.clone());
            out.push(v.clone());
        }
        out.extend(self.found_log.as_ref().map(|f| f.1.clone()));
        out.push("History".into());
        if self.history.is_empty() {
            out.push("Nothing yet.".into());
        }
        for h in &self.history {
            out.push(h.time.clone());
            out.push(h.text.clone());
        }
    }
}

/// `issuePanel` (the Board's side panel).
pub struct PanelView {
    pub r: String,
    pub kind: String,
    pub title: String,
    pub project: String,
    pub state_line: String,
    /// "Open it" (made into a task): the task ref.
    pub open_task: Option<String>,
    /// While open: the box's sentence and the actions.
    pub open_box: Option<(String, Vec<Act>)>,
    pub note: Option<(String, bool)>,
    pub detail: DetailView,
}

pub fn panel_view(cx: &Ctx, b: &Value) -> PanelView {
    let r = fmt::ref_of(b, "B");
    let state_line = issue_state(b).map(|x| x.1).unwrap_or_else(|| format!("Open · reported at {}", fmt::hhmm(s(b, "created_at"))));
    let open = matches!(s(b, "state"), "" | "open");
    PanelView {
        kind: kind_label(s(b, "kind")),
        title: s(b, "title").to_string(),
        project: s(b, "project").to_string(),
        state_line,
        open_task: (s(b, "state") == "task").then(|| b.get("task_id").and_then(Value::as_i64)).flatten().map(|n| format!("T{n}")),
        open_box: open.then(|| {
            let what = format!("Not part of any task yet. Make it a task to queue it{}.", if cx.jira { ", or send it to Jira" } else { "" });
            (what, issue_actions(cx, &r, "board"))
        }),
        note: cx.note(&format!("issue:{r}")),
        detail: detail_view(b),
        r,
    }
}

impl PanelView {
    #[cfg(test)]
    pub fn text_and_acts(&self) -> (String, Vec<&'static str>) {
        let mut out = vec!["Backlog".to_string(), self.kind.clone(), self.r.clone()];
        let mut acts = vec!["close-panel"];
        out.push(self.title.clone());
        out.push(self.project.clone());
        out.push(self.state_line.clone());
        if self.open_task.is_some() {
            out.push("· Open it".into());
        }
        if let Some((what, a)) = &self.open_box {
            out.push(what.clone());
            for x in a {
                out.push(x.label.clone());
                acts.push(x.act);
            }
        }
        out.extend(self.note.as_ref().map(|n| n.0.clone()));
        self.detail.parts(&mut out);
        out.push("See every backlog issue".into());
        (flat(&out), acts)
    }
}

/// `issueAside` (beside the Backlog page's list): no actions box, the move picker, compact.
pub struct AsideView {
    pub r: String,
    pub kind: String,
    /// The state, or "Open · reported at …".
    pub help: String,
    pub title: String,
    /// "Open task T7".
    pub open_task: Option<String>,
    pub detail: DetailView,
}

pub fn aside_view(b: &Value) -> AsideView {
    AsideView {
        r: fmt::ref_of(b, "B"),
        kind: kind_label(s(b, "kind")),
        help: issue_state(b).map(|x| x.1).unwrap_or_else(|| format!("Open · reported at {}", fmt::hhmm(s(b, "created_at")))),
        title: s(b, "title").to_string(),
        open_task: (s(b, "state") == "task").then(|| b.get("task_id").and_then(Value::as_i64)).flatten().map(|n| format!("T{n}")),
        detail: detail_view(b),
    }
}

impl AsideView {
    #[cfg(test)]
    pub fn text_and_acts(&self, err: Option<&str>) -> (String, Vec<&'static str>) {
        let mut out = vec![self.kind.clone(), self.help.clone(), self.r.clone(), self.title.clone()];
        out.extend(self.open_task.as_ref().map(|t| format!("Open task {t}")));
        out.extend(err.map(str::to_string));
        self.detail.parts(&mut out);
        (flat(&out), vec![])
    }
}

/// Visible text as the browser shows it: parts joined, whitespace collapsed.
#[cfg(test)]
pub fn flat(parts: &[String]) -> String {
    parts.join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

// ------------------------------------------------------------------ actions

/// `setNote`: an inline note under the buttons of `grp`, gone after 5 s (15 s for an error).
pub fn set_note(m: &mut MainWindow, grp: &str, text: impl Into<String>, err: bool, cx: &mut Context<MainWindow>) {
    let at = Instant::now();
    m.issue_panel.notes.insert(grp.to_string(), (text.into(), err, at));
    let grp = grp.to_string();
    let wait = Duration::from_secs(if err { 15 } else { 5 });
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(wait).await;
        let _ = this.update(cx, |m, cx| {
            if m.issue_panel.notes.get(&grp).is_some_and(|n| n.2 == at) {
                m.issue_panel.notes.remove(&grp);
                cx.notify();
            }
        });
    })
    .detach();
    cx.notify();
}

/// `run()`: one call per button at a time ("Sending…"), the note group cleared first, the
/// result (`ok` → a note, or nothing) or the error shown in the group, then a refresh.
pub fn run(
    m: &mut MainWindow,
    key: String,
    grp: String,
    path: String,
    body: Value,
    cx: &mut Context<MainWindow>,
    ok: impl FnOnce(&mut MainWindow, &Value, &mut Context<MainWindow>) -> Option<String> + 'static,
) {
    if !m.issue_panel.busy.insert(key.clone()) {
        return;
    }
    m.issue_panel.notes.remove(&grp);
    let (k1, k2, g1, g2) = (key.clone(), key, grp.clone(), grp);
    m.post_or(
        path,
        body,
        cx,
        move |m, v, cx| {
            m.issue_panel.busy.remove(&k1);
            if let Some(msg) = ok(m, &v, cx).filter(|x| !x.is_empty()) {
                set_note(m, &g1, msg, false, cx);
            }
            crate::ui::backlog::stale(m);
        },
        move |m, e, cx| {
            m.issue_panel.busy.remove(&k2);
            set_note(m, &g2, e.message, true, cx);
            crate::ui::backlog::stale(m);
        },
    );
    cx.notify();
}

/// `keepIssue`: keep a changed issue where it was on the Board's Backlog column (it may no longer
/// match the filters).
pub fn keep_issue(m: &mut MainWindow, r: &str) {
    crate::ui::board::keep_issue(m, r);
}

/// `promote`: `POST /backlog/:id/promote {where}`. On the Board the new task opens.
pub fn promote(m: &mut MainWindow, r: &str, place: &'static str, cx: &mut Context<MainWindow>) {
    keep_issue(m, r);
    run(m, format!("promote:{place}:{r}"), format!("issue:{r}"), format!("backlog/{r}/promote"), json!({"where": place}), cx, |m, res, cx| {
        let t = res.get("task").filter(|t| t.is_object()).or_else(|| (s(res, "ref").starts_with('T')).then_some(res));
        let tr = t.map(|t| fmt::ref_of(t, "T")).or_else(|| res.get("task_id").and_then(Value::as_i64).map(|n| format!("T{n}"))).unwrap_or_default();
        if m.page == Page::Board && !tr.is_empty() {
            m.open_task(tr, cx);
            return None;
        }
        (m.page == Page::Board).then(|| "Made into a task".to_string())
    });
}

pub fn ticket(m: &mut MainWindow, r: &str, cx: &mut Context<MainWindow>) {
    keep_issue(m, r);
    run(m, format!("ticket::{r}"), format!("issue:{r}"), format!("backlog/{r}/ticket"), json!({}), cx, |_, _, _| Some("Asked Jira for a ticket".into()));
}

pub fn drop_issue(m: &mut MainWindow, r: &str, cx: &mut Context<MainWindow>) {
    keep_issue(m, r);
    run(m, format!("drop::{r}"), format!("issue:{r}"), format!("backlog/{r}/drop"), json!({}), cx, |_, _, _| Some("Closed as won’t do".into()));
}

/// "Open T4’s log": the Board, with that task's Log tab.
pub fn open_log(m: &mut MainWindow, tr: &str, cx: &mut Context<MainWindow>) {
    m.go(Page::Board, cx);
    m.open_task(tr.to_string(), cx);
    m.panel = Some(Panel::Task { r: tr.to_string(), tab: TaskTab::Log });
}

/// "See every backlog issue": the Backlog page.
pub fn see_all(m: &mut MainWindow, r: &str, cx: &mut Context<MainWindow>) {
    let _ = r;
    m.close_panel(cx);
    m.go(Page::Backlog, cx);
}

// ------------------------------------------------------------------ the picker

/// The web board's searchable project / goal picker (`openPicker`, `pickerItems`, `pickerKey`).
pub struct Picker {
    /// The menu key it opened under (positions it; any outside click closes it).
    pub key: String,
    pub goal: bool,
    /// The current value ("all", "none", a project name or a goal ref).
    pub value: String,
    pub specials: Vec<(String, String)>,
    /// Goals of this project only ("" / "all" = every goal).
    pub proj: String,
    pub q: kit::Input,
    pub active: usize,
    on_pick: std::rc::Rc<dyn Fn(&mut MainWindow, String, &mut Context<MainWindow>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PickItem {
    pub v: String,
    pub name: String,
    pub sub: String,
    pub special: bool,
}

/// `projectNames` (see [`crate::app::project_names`]).
pub fn project_names(state: &Value, goals: &[Value]) -> Vec<String> {
    crate::app::project_names(state, live_goals(goals))
}

/// `pickerBase` + `pickerItems`: specials first (without a query), then goals / projects; a
/// query filters and ranks (starts with, a word starts with, contains).
pub fn picker_items(goal: bool, value: &str, specials: &[(String, String)], proj: &str, q: &str, state: &Value, goals: &[Value]) -> Vec<PickItem> {
    let mut items: Vec<PickItem> = if goal {
        let mut v: Vec<PickItem> = live_goals(goals)
            .into_iter()
            .filter(|g| proj.is_empty() || proj == "all" || s(g, "project") == proj)
            .map(|g| PickItem {
                v: fmt::ref_of(g, "G"),
                name: s(g, "name").to_string(),
                sub: [s(g, "project"), s(g, "epic_key")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · "),
                special: false,
            })
            .collect();
        v.sort_by(|a, b| locale_cmp(&a.name, &b.name));
        v
    } else {
        let mut names = project_names(state, goals);
        if !value.is_empty() && !specials.iter().any(|(v, _)| v == value) && !names.iter().any(|n| n == value) {
            names.push(value.to_string());
        }
        let paths: HashMap<String, String> = arr(state, "projects").iter().filter_map(|p| Some((fmt::opt_s(p, "name")?.to_string(), fmt::opt_s(p, "path")?.to_string()))).collect();
        names.into_iter().map(|n| PickItem { sub: paths.get(&n).map(|p| home_tilde(p)).unwrap_or_default(), v: n.clone(), name: n, special: false }).collect()
    };
    let q = q.trim().to_lowercase();
    if !q.is_empty() {
        let rank = |it: &PickItem| {
            let l = it.name.to_lowercase();
            if l.starts_with(&q) {
                0
            } else if l.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w.starts_with(&q)) {
                1
            } else if l.contains(&q) {
                2
            } else {
                3
            }
        };
        items.retain(|it| it.name.to_lowercase().contains(&q) || (goal && it.sub.to_lowercase().contains(&q)));
        items.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| locale_cmp(&a.name, &b.name)));
        return items;
    }
    let mut out: Vec<PickItem> = specials.iter().map(|(v, n)| PickItem { v: v.clone(), name: n.clone(), sub: String::new(), special: true }).collect();
    out.extend(items);
    out
}

/// `/Users/alex/x` → `~/x`.
fn home_tilde(p: &str) -> String {
    match p.strip_prefix("/Users/") {
        Some(rest) => match rest.find('/') {
            Some(i) => format!("~{}", &rest[i..]),
            None => "~".into(),
        },
        None => p.to_string(),
    }
}

/// `pickerButton`'s label: a special's name, the goal's name ("Choose a goal" when unknown) or
/// the project ("Choose a project" when empty).
#[cfg(test)]
pub fn picker_label(goal: bool, value: &str, specials: &[(String, String)], goals: &[Value]) -> String {
    if let Some((_, n)) = specials.iter().find(|(v, _)| v == value) {
        return n.clone();
    }
    if goal {
        return live_goals(goals).into_iter().find(|g| fmt::ref_of(g, "G") == value).map(|g| s(g, "name").to_string()).unwrap_or_else(|| "Choose a goal".into());
    }
    if value.is_empty() { "Choose a project".into() } else { value.to_string() }
}

/// Open a picker under the click. `on_pick` runs only when the choice changes (`pickItem`).
#[allow(clippy::too_many_arguments)]
pub fn open_picker(
    m: &mut MainWindow,
    key: &str,
    goal: bool,
    value: String,
    specials: Vec<(String, String)>,
    proj: String,
    at: Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<MainWindow>,
    on_pick: impl Fn(&mut MainWindow, String, &mut Context<MainWindow>) + 'static,
) {
    let q = kit::Input::new(cx, if goal { "Search goals by name, project or epic" } else { "Search projects" }, false);
    window.focus(&q.focus, cx);
    let items = picker_items(goal, &value, &specials, &proj, "", m.state(), &m.data.goals);
    let active = items.iter().position(|it| it.v == value).unwrap_or(0);
    m.issue_panel.picker = Some(Picker { key: key.to_string(), goal, value, specials, proj, q, active, on_pick: std::rc::Rc::new(on_pick) });
    m.menu = Some((key.to_string(), at));
    cx.notify();
}

fn close_picker(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    m.issue_panel.picker = None;
    m.menu = None;
    cx.notify();
}

fn pick(m: &mut MainWindow, v: String, cx: &mut Context<MainWindow>) {
    let Some(p) = m.issue_panel.picker.take() else { return };
    m.menu = None;
    if v != p.value {
        (p.on_pick)(m, v, cx);
    }
    cx.notify();
}

/// The open picker, if its menu is open.
pub fn render_picker(m: &mut MainWindow, t: &Theme, window: &mut Window, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let p = m.issue_panel.picker.as_ref()?;
    let at = m.menu_open(&p.key)?;
    let q = p.q.text(cx);
    let items = picker_items(p.goal, &p.value, &p.specials, &p.proj, &q, m.state(), &m.data.goals);
    let active = p.active.min(items.len().saturating_sub(1));
    let (title, none) = if p.goal { ("Choose a goal", "No goal matches") } else { ("Choose a project", "No project matches") };
    let mut list = div().id("picker-list").flex().flex_col().max_h(px(320.)).overflow_y_scroll().p(px(4.));
    if items.is_empty() {
        list = list.child(div().p(px(10.)).text_size(px(13.)).text_color(t.muted).child(format!("{none} “{}”.", q.trim())));
    }
    for (i, it) in items.iter().enumerate() {
        let v = it.v.clone();
        let hover = t.panel_2;
        list = list.child(
            div()
                .id(("picker-item", i))
                .flex()
                .items_center()
                .gap(px(8.))
                .min_h(px(32.))
                .px(px(10.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(13.))
                .when(i == active, |d| d.bg(t.accent_soft))
                .when(i != active, |d| d.hover(move |s| s.bg(hover)))
                .when(it.special, |d| d.text_color(t.muted))
                .child(div().flex_1().min_w_0().truncate().child(it.name.clone()))
                .when(!it.sub.is_empty(), |d| d.child(div().flex_none().max_w(px(180.)).truncate().text_size(px(12.)).text_color(t.faint).child(it.sub.clone())))
                .when(it.v == p.value, |d| d.child(div().flex_none().text_size(px(11.5)).text_color(t.accent_fg).child("Current")))
                .on_click(cx.listener(move |m, _, _, cx| pick(m, v.clone(), cx))),
        );
    }
    let field = p.q.render(t, "picker-q", window).on_key_down(cx.listener(|m, ev: &KeyDownEvent, _, cx| {
        let Some(p) = m.issue_panel.picker.as_ref() else { return };
        let q = p.q.text(cx);
        let items = picker_items(p.goal, &p.value, &p.specials, &p.proj, &q, m.state(), &m.data.goals);
        let n = items.len();
        match ev.keystroke.key.as_str() {
            "escape" => {
                cx.stop_propagation();
                close_picker(m, cx);
            }
            "down" | "up" if n > 0 => {
                cx.stop_propagation();
                let down = ev.keystroke.key == "down";
                if let Some(p) = m.issue_panel.picker.as_mut() {
                    p.active = (p.active.min(n - 1) + if down { 1 } else { n - 1 }) % n;
                }
                cx.notify();
            }
            "enter" => {
                cx.stop_propagation();
                let a = p.active.min(n.saturating_sub(1));
                if let Some(it) = items.get(a) {
                    let v = it.v.clone();
                    pick(m, v, cx);
                }
            }
            "tab" => cx.stop_propagation(),
            _ => {
                // Typing changes the query: back to the first item.
                if let Some(p) = m.issue_panel.picker.as_mut() {
                    p.active = 0;
                }
            }
        }
    }));
    let foot = div().flex().gap(px(14.)).px(px(12.)).py(px(8.)).border_t_1().border_color(t.divider).text_size(px(11.5)).text_color(t.faint).child("↑ ↓ to move").child("↩ to choose").child("esc to close");
    let card = kit::menu_box(t, 360.)
        .id("picker")
        .p(px(0.))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(div().px(px(12.)).pt(px(10.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(t.muted).child(title))
        .child(div().p(px(8.)).child(field))
        .child(list)
        .child(foot);
    Some(kit::popover(at, card))
}

/// The button that opens a picker: a search glyph and the label (`pickerButton`).
pub fn picker_button(t: &Theme, id: impl Into<ElementId>, label: String, empty: bool) -> Stateful<Div> {
    kit::btn_small(t, id, format!("⌕  {label}")).max_w(px(320.)).overflow_hidden().when(empty, |d| d.text_color(t.muted))
}

// ------------------------------------------------------------------ render

/// Inline note text (`.note.ok` / `.note.err`).
pub fn note_el(t: &Theme, n: &Option<(String, bool)>) -> Option<Div> {
    n.as_ref().map(|(text, err)| div().text_size(px(12.5)).text_color(if *err { t.down } else { t.up_fg }).child(text.clone()))
}

/// A button for an [`Act`]: `primary` / plain / `ghost` styles as the web board's classes.
pub fn act_button(t: &Theme, id: impl Into<ElementId>, a: &Act, style: &str, small: bool) -> Stateful<Div> {
    let b = match style {
        "primary" => kit::btn_primary(t, id, a.label.clone()),
        "soft" => kit::btn(t, id, a.label.clone()).bg(t.accent_soft).border_color(t.accent_line).text_color(t.accent_fg),
        "ghost" => {
            let h = t.panel_2;
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .h(px(30.))
                .px(px(12.))
                .rounded(px(8.))
                .text_color(t.muted)
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .whitespace_nowrap()
                .cursor_pointer()
                .hover(move |s| s.bg(h))
                .child(a.label.clone())
        }
        _ => kit::btn(t, id, a.label.clone()),
    };
    // `.btn` is 36px tall, padding 0 14px (`.ghost` 0 10px), radius 8px, 13px; primary is bold.
    let b = if small {
        b.h(px(26.)).px(px(10.)).rounded(px(7.)).text_size(px(12.5))
    } else {
        let b = b.h(px(36.)).px(px(if style == "ghost" { 10. } else { 14. })).rounded(px(8.)).text_size(px(13.));
        match style {
            "primary" => b.font_weight(FontWeight::SEMIBOLD),
            "ghost" => b,
            _ => b.font_weight(FontWeight::NORMAL).border_color(t.border),
        }
    };
    if a.busy { kit::disabled(b) } else { b }
}

/// Buttons for [`issue_actions`], wired.
pub fn action_row(t: &Theme, r: &str, acts: &[Act], small: bool, id: &str, cx: &mut Context<MainWindow>) -> Div {
    let mut row = div().flex().flex_wrap().items_center().gap(px(if small { 6. } else { 8. }));
    for a in acts {
        let style = match (a.act, small) {
            ("promote", true) => "soft",
            ("promote", false) => "primary",
            ("drop", _) => "ghost",
            _ => "",
        };
        let b = act_button(t, SharedString::from(format!("{id}-{}-{r}", a.act)), a, style, small);
        let (rr, act, arg) = (r.to_string(), a.act, a.arg);
        row = row.child(if a.busy {
            b
        } else {
            b.on_click(cx.listener(move |m, _, _, cx| {
                cx.stop_propagation();
                match act {
                    "promote" => promote(m, &rr, arg, cx),
                    "ticket" => ticket(m, &rr, cx),
                    _ => drop_issue(m, &rr, cx),
                }
            }))
        });
    }
    row
}

fn section(t: &Theme, title: &str) -> Div {
    div().flex().flex_col().gap(px(8.)).child(kit::h3(t, title.to_string()))
}

/// The panel's `.stack` (gap 6px) under a 13px `.h3`.
fn panel_section(t: &Theme, title: &str, gap: f32) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(gap))
        .min_w_0()
        .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.muted).whitespace_nowrap().child(title.to_uppercase()))
}

fn hist_color(t: &Theme, kind: &str) -> Hsla {
    match kind {
        "report" => t.warn,
        "seen" => t.warn_line,
        "ticket" => t.accent,
        "task" | "move" => t.goal,
        "lost" => t.down,
        _ => t.faint,
    }
}

/// `issueDetail`, drawn. `compact` is the aside's smaller text.
pub fn render_detail(t: &Theme, v: &DetailView, compact: bool, cx: &mut Context<MainWindow>) -> Div {
    let r = v.r.clone();
    let mut col = div().flex().flex_col().gap(px(18.));
    let sec = |title: &str, gap: f32| if compact { section(t, title) } else { panel_section(t, title, gap) };
    let mut how = sec("How it was reported", 6.).child(div().text_size(px(if compact { 13. } else { 14. })).child(v.how.clone()));
    if let Some(said) = &v.said {
        how = how.child(div().text_size(px(14.)).text_color(t.text_2).bg(t.bg).border_l(px(3.)).border_color(t.border_2).rounded_r(px(8.)).px(px(12.)).py(px(8.)).child(said.clone()));
    }
    if let Some(d) = &v.detail {
        // `.what`: plain text, line breaks kept.
        let mut p = div().flex().flex_col().text_size(px(13.));
        for line in d.split('\n') {
            p = p.child(div().min_h(px(17.)).child(line.to_string()));
        }
        how = how.child(p);
    }
    col = col.child(how);

    let mut happening = sec("What was happening", 6.);
    if v.snap.is_empty() {
        happening = happening.child(kit::help(t, "No snapshot was saved with this issue."));
    } else {
        let mut kv = div().flex().flex_col().rounded(px(if compact { 10. } else { 12. })).border_1().border_color(t.border);
        for (i, (k, val)) in v.snap.iter().enumerate() {
            kv = kv.child(
                div()
                    .flex()
                    .gap(px(if compact { 10. } else { 12. }))
                    .px(px(if compact { 12. } else { 16. }))
                    .py(px(if compact { 7. } else { 9. }))
                    .when(i > 0, |d| d.border_t_1().border_color(t.divider))
                    .child(div().flex_none().w(px(if compact { 96. } else { 110. })).text_size(px(if compact { 12. } else { 12.5 })).font_weight(FontWeight::SEMIBOLD).text_color(t.muted).child(k.clone()))
                    .child(div().flex_1().min_w_0().text_size(px(if compact { 12.5 } else { 13. })).child(val.clone())),
            );
        }
        happening = happening.child(kv);
    }
    if let Some((fr, label)) = v.found_log.clone() {
        happening = happening.child(div().flex().text_size(px(13.)).child(kit::link(t, SharedString::from(format!("issue-found-log-{r}")), label).when(!compact, |d| d.underline()).on_click(cx.listener(move |m, _, _, cx| open_log(m, &fr, cx)))));
    }
    col = col.child(happening);

    let mut history = sec("History", 8.);
    if v.history.is_empty() {
        history = history.child(kit::help(t, "Nothing yet."));
    }
    for (n, h) in v.history.iter().enumerate() {
        if !compact {
            // `.hist li`: 52px time · 10px dot · text, gap 8px.
            let text = match &h.who {
                Some(w) => div().flex_1().min_w_0().text_size(px(13.5)).child(
                    StyledText::new(format!("{w} · {}", h.what)).with_highlights([(0..w.len(), HighlightStyle { font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() })]),
                ),
                None => div().flex_1().min_w_0().text_size(px(13.5)).child(h.what.clone()),
            };
            history = history.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .child(div().id(SharedString::from(format!("issue-hist-{r}-{n}"))).flex_none().w(px(52.)).whitespace_nowrap().text_color(t.muted).text_size(px(12.5)).child(h.time.clone()).tooltip(kit::tip(h.full.clone())))
                    .child(div().flex_none().w(px(10.)).pt(px(6.)).child(kit::dot(hist_color(t, &h.kind), 8.)))
                    .child(text),
            );
            continue;
        }
        history = history.child(
            div()
                .flex()
                .items_start()
                .gap(px(10.))
                .text_size(px(13.))
                .child(div().id(SharedString::from(format!("issue-hist-{r}-{n}"))).flex_none().w(px(64.)).text_color(t.muted).text_size(px(12.)).child(h.time.clone()).tooltip(kit::tip(h.full.clone())))
                .child(div().flex_none().pt(px(6.)).child(kit::dot(hist_color(t, &h.kind), 7.)))
                .child(div().flex_1().min_w_0().child(h.text.clone())),
        );
    }
    col.child(history)
}

/// The issue panel on the Board and the Backlog page (the Goal page draws [`render_aside`] beside
/// its list instead).
pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let r = match &m.panel {
        Some(Panel::Issue { r }) if !matches!(m.page, Page::Goal(_)) => r.clone(),
        _ => return div().into_any_element(),
    };
    let close = close_btn(&t).on_click(cx.listener(|m, _, _, cx| m.close_panel(cx)));
    let issue = m.data.issue.clone().filter(|b| fmt::ref_of(b, "B") == r);
    let body = match issue {
        Some(b) => panel_body(m, &t, &b, close, cx),
        None => div()
            .id("issue-panel-body")
            .flex()
            .flex_col()
            .gap(px(18.))
            .px(px(24.))
            .pt(px(24.))
            .line_height(relative(1.5))
            .child(div().flex().items_center().gap(px(8.)).child(web_pill(&t, t.text_2, t.col, "Backlog", false)).child(div().flex_1()).child(close))
            .child(kit::help(&t, "Loading…")),
    };
    let picker = render_picker(m, &t, window, cx);
    deferred(
        // The web's `.panel` sits over the board without dimming it; a click outside still closes it.
        kit::scrim(&t, "issue-scrim")
            .bg(transparent_black())
            .on_mouse_down(MouseButton::Left, cx.listener(|m, _, _, cx| m.close_panel(cx)))
            .child(
                div()
                    .id("issue-panel")
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(px(WIDTH))
                    .max_w(relative(1.))
                    .bg(t.card)
                    .border_l_1()
                    .border_color(t.border)
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(body),
            )
            .children(picker),
    )
    .with_priority(1)
    .into_any_element()
}

fn panel_body(m: &mut MainWindow, t: &Theme, b: &Value, close: Stateful<Div>, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let v = panel_view(&Ctx::of(m), b);
    let r = v.r.clone();
    let (kfg, kbg) = kind_colors(t, s(b, "kind"));
    // `.panel-top`: Backlog, the kind and the ref as `.pill`s, the close `.icon-btn`.
    let top = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(8.))
        .child(web_pill(t, t.text_2, t.col, "Backlog", false))
        .child(web_pill(t, kfg, kbg, v.kind.clone(), false))
        .child(web_pill(t, t.muted, t.panel_2, r.clone(), true))
        .child(div().flex_1())
        .child(close);
    // `h2.title` (20px / 1.3) and the `.subline` (13px muted, gap 6px 12px).
    let title = div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(div().text_size(px(20.)).font_weight(FontWeight::BOLD).line_height(relative(1.3)).child(v.title.clone()))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(px(12.))
                .gap_y(px(6.))
                .text_size(px(13.))
                .text_color(t.muted)
                .child(div().font_family(t.mono_font.clone()).child(v.project.clone()))
                .child(div().flex().gap(px(4.)).child(v.state_line.clone()).when_some(v.open_task.clone(), |d, tr| {
                    d.child("·").child(kit::link(t, "issue-open-task", "Open it").underline().on_click(cx.listener(move |m, _, _, cx| m.open_task(tr.clone(), cx))))
                })),
        );
    let mut col = div()
        .id("issue-panel-body")
        .flex()
        .flex_col()
        .gap(px(18.))
        .size_full()
        .px(px(24.))
        .pt(px(24.))
        .pb(px(32.))
        .line_height(relative(1.5))
        .overflow_y_scroll()
        .child(top)
        .child(title);
    match &v.open_box {
        Some((what, acts)) => {
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .px(px(16.))
                    .py(px(14.))
                    .rounded(px(12.))
                    .bg(accent_tint(t))
                    .border_1()
                    .border_color(t.accent_line)
                    .child(div().text_size(px(13.)).child(what.clone()))
                    .child(action_row(t, &r, acts, false, "issue", cx))
                    .children(note_el(t, &v.note)),
            )
        }
        None => col = col.children(note_el(t, &v.note)),
    }
    col = col.child(render_detail(t, &v.detail, false, cx));
    let rr = r.clone();
    col.child(div().flex().text_size(px(13.)).child(kit::link(t, "issue-all", "See every backlog issue").underline().on_click(cx.listener(move |m, _, _, cx| see_all(m, &rr, cx)))))
}

/// `.pill` (12px semibold, padding 2px 10px); `mono` is `.pill.ref`.
fn web_pill(t: &Theme, fg: Hsla, bg: Hsla, text: impl Into<SharedString>, mono: bool) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .px(px(10.))
        .py(px(2.))
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_size(px(12.))
        .line_height(relative(1.5))
        .font_weight(if mono { FontWeight::MEDIUM } else { FontWeight::SEMIBOLD })
        .when(mono, |d| d.font_family(t.mono_font.clone()))
        .whitespace_nowrap()
        .child(text.into())
}

/// `.icon-btn` (36px, bordered) holding `ICON.x`.
fn close_btn(t: &Theme) -> Stateful<Div> {
    let hover = t.border_2;
    div()
        .id("issue-close")
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(36.))
        .rounded(px(8.))
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .cursor_pointer()
        .hover(move |d| d.border_color(hover))
        .tooltip(kit::tip("Close"))
        .child(kit::glyph(kit::Glyph::X, 16., t.muted))
}

/// `--accent-tint` (#f5f8ff light, #172036 dark), which the theme doesn't carry.
fn accent_tint(t: &Theme) -> Hsla {
    if matches!(t.mode, crate::theme::ThemeMode::Dark) { rgb(0x172036).into() } else { rgb(0xf5f8ff).into() }
}

/// `issueAside`: the issue (and its view) beside the Backlog page's list. None = still loading
/// (or `err`).
pub fn render_aside(t: &Theme, issue: Option<(&Value, &AsideView)>, err: Option<&str>, cx: &mut Context<MainWindow>) -> AnyElement {
    let card = div().flex().flex_col().gap(px(16.)).p(px(18.)).rounded(px(14.)).border_1().border_color(t.border).bg(t.card);
    let Some((b, v)) = issue else {
        return card.child(div().text_size(px(13.)).text_color(if err.is_some() { t.down } else { t.muted }).child(err.unwrap_or("Loading…").to_string())).into_any_element();
    };
    let head = div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child({
                    let (fg, bg) = kind_colors(t, s(b, "kind"));
                    kit::pill(fg, bg, v.kind.clone())
                })
                .child(div().flex_1().min_w_0().text_size(px(12.5)).text_color(t.muted).child(v.help.clone()))
                .child(kit::chip(t, v.r.clone())),
        )
        .child(div().text_size(px(17.)).font_weight(FontWeight::BOLD).line_height(px(23.)).child(v.title.clone()))
        .when_some(v.open_task.clone(), |d, tr| {
            let label = format!("Open task {tr}");
            d.child(div().flex().text_size(px(12.5)).child(kit::link(t, "aside-open-task", label).on_click(cx.listener(move |m, _, _, cx| m.open_task(tr.clone(), cx)))))
        });
    card.child(head)
        .children(err.map(|e| div().text_size(px(12.5)).text_color(t.down).child(e.to_string())))
        .child(render_detail(t, &v.detail, true, cx))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's `test` attribute, which `#[gpui_kit::test]` then recurses on.
    use super::{Ctx, drop_issue, issue_from, issue_how, issue_state, kind_label, panel_view, picker_items, picker_label, promote, snap_rows, ticket};
    use crate::app::Panel;
    use crate::backend::Backend;
    use crate::fmt::{self, arr, s};
    use crate::parity::{self, golden};
    use serde_json::{Value, json};
    use std::collections::{HashMap, HashSet};
    use std::time::Instant;

    struct Fix {
        notes: HashMap<String, (String, bool, Instant)>,
        busy: HashSet<String>,
        goals: Vec<Value>,
        jira: bool,
    }

    impl Fix {
        fn of(i: &Value) -> Fix {
            let now = Instant::now();
            Fix {
                notes: i["notes"].as_object().map(|o| o.iter().map(|(k, n)| (k.clone(), (s(n, "text").to_string(), fmt::b(n, "err"), now))).collect()).unwrap_or_default(),
                busy: arr(i, "busy").iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
                goals: arr(i, "goals").to_vec(),
                jira: fmt::b(i, "jira"),
            }
        }
        fn ctx(&self) -> Ctx<'_> {
            Ctx { jira: self.jira, goals: &self.goals, notes: &self.notes, busy: &self.busy }
        }
    }

    #[::core::prelude::v1::test]
    fn issue_labels_match_web() {
        let g = golden("backlog");
        for f in ["issueState", "issueFrom", "issueHow", "snapRows", "kindLabel"] {
            g.only(f).check(|i| match f {
                "issueState" => issue_state(&i["issue"]).map(|(a, b)| json!({"short": a, "text": b})).unwrap_or(Value::Null),
                "issueFrom" => json!(issue_from(&i["issue"], fmt::b(i, "long"))),
                "issueHow" => json!(issue_how(&i["issue"])),
                "snapRows" => json!(snap_rows(Some(&i["snapshot"])).into_iter().map(|(k, v)| vec![k, v]).collect::<Vec<_>>()),
                _ => json!(kind_label(s(i, "kind"))),
            });
        }
    }

    #[::core::prelude::v1::test]
    fn issue_panel_matches_web() {
        golden("backlog").only("panel").check(|i| {
            let fix = Fix::of(i);
            let (text, acts) = panel_view(&fix.ctx(), &i["issue"]).text_and_acts();
            json!({"text": text, "acts": acts})
        });
    }

    #[::core::prelude::v1::test]
    fn picker_items_rank_and_filter_like_web() {
        parity::freeze("2026-10-07T15:00:00Z");
        let goals = vec![
            json!({"id": 1, "name": "Sign-in with passkeys", "project": "webapp", "epic_key": "PROJ-9"}),
            json!({"id": 2, "name": "Billing revamp", "project": "api"}),
            json!({"id": 3, "name": "Archived", "project": "api", "archived": true}),
        ];
        let state = json!({"projects": [{"name": "webapp", "path": "/Users/alex/code/webapp"}, {"name": "api"}]});
        let none = vec![("none".to_string(), "Not in a goal".to_string())];
        let all = picker_items(true, "G2", &none, "", "", &state, &goals);
        assert_eq!(all.iter().map(|x| x.v.as_str()).collect::<Vec<_>>(), ["none", "G2", "G1"]);
        assert_eq!(all[2].sub, "webapp · PROJ-9");
        assert_eq!(picker_items(true, "", &none, "api", "", &state, &goals).len(), 2);
        // A query drops the specials and searches the project / epic too.
        assert_eq!(picker_items(true, "", &none, "", "proj-9", &state, &goals).iter().map(|x| x.v.as_str()).collect::<Vec<_>>(), ["G1"]);
        let p = picker_items(false, "mobile", &[("all".into(), "All projects".into())], "", "", &state, &goals);
        assert_eq!(p.iter().map(|x| x.v.as_str()).collect::<Vec<_>>(), ["all", "api", "webapp", "mobile"]);
        assert_eq!(p[2].sub, "~/code/webapp");
        assert_eq!(picker_label(true, "G9", &none, &goals), "Choose a goal");
        assert_eq!(picker_label(false, "", &[], &goals), "Choose a project");
    }

    // ---------------------------------------------------------- actions (Recording backend)

    fn add_issue(rec: &parity::Recording, body: Value) -> String {
        let v = rec.board().post("backlog", body).expect("add issue");
        fmt::ref_of(v.get("issue").unwrap_or(&v), "B")
    }

    #[gpui_kit::test]
    fn panel_actions_send_what_the_web_sent(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = parity::window(cx);
        let r = add_issue(&rec, json!({"title": "Flaky test", "kind": "gap", "project": "webapp", "goal_id": null}));
        w.update(cx, |m, _, cx| {
            m.open_issue(r.clone(), cx);
            ticket(m, &r, cx);
        })
        .unwrap();
        parity::settle(cx);
        assert_eq!(rec.last(&format!("backlog/{r}/ticket")), Some(json!({})));
        // The Board keeps the changed issue in its Backlog column (`keepIssue`).
        w.update(cx, |m, _, _| assert!(m.board.keep.contains(&r))).unwrap();
        w.update(cx, |m, _, cx| drop_issue(m, &r, cx)).unwrap();
        parity::settle(cx);
        assert_eq!(rec.last(&format!("backlog/{r}/drop")), Some(json!({})));
        w.update(cx, |m, _, _| assert_eq!(m.issue_panel.notes.get(&format!("issue:{r}")).map(|n| n.0.clone()), Some("Closed as won’t do".into()))).unwrap();
    }

    #[gpui_kit::test]
    fn promoting_on_the_board_opens_the_new_task(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = parity::window(cx);
        let r = add_issue(&rec, json!({"title": "Add retries", "kind": "follow", "project": "webapp"}));
        w.update(cx, |m, _, cx| {
            m.open_issue(r.clone(), cx);
            promote(m, &r, "board", cx);
        })
        .unwrap();
        parity::settle(cx);
        assert_eq!(rec.last(&format!("backlog/{r}/promote")), Some(json!({"where": "board"})));
        w.update(cx, |m, _, _| assert!(matches!(&m.panel, Some(Panel::Task { .. })), "the new task's panel opens")).unwrap();
    }
}
