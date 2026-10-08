//! Dialogs over the main window: the alerts list, and the New task / New goal / Edit goal /
//! Add an issue forms (the web board's `taskFormHtml`, `goalFormHtml`, `newIssueHtml`), their
//! pickers (`pickerItems`), and drafts (`saveDraft` / `restoreDraft`, prefs `tb.draft.<kind>`).
//!
//! The logic is in pure functions mirroring the web code one to one, tested against the web
//! board itself (`parity/gen/forms.mjs` → `parity/golden/forms.json`):
//! - `open_*_vals`: what a form starts with (`openTaskForm`, `openGoalForm`, `openNewIssue`);
//! - `*_request`: validation message, or the exact POST path and body (`submitTask` …);
//! - [`picker_items`]: a picker's list for a search (`pickerBase` + `pickerItems`);
//! - `*_draft`: what is kept for next time (`saveDraft`);
//! - `*_view`: what the form shows (labels, help, pressed choices, picker labels); `render` draws
//!   the view, so what's tested is what's on screen.
//!
//! Keys, as on the web: ↩ in a one-line field submits; in a picker ↑/↓ move, ↩ picks, esc closes
//! it. Esc and clicks on the backdrop never close a form (they keep what you typed); esc closes the
//! read-only alerts dialog. ⌘↩ also submits from anywhere, and ⇥ moves between the text fields.
use crate::app::{CloseOverlay, MainWindow, Page, alert_row};
use crate::fmt::{self, arr, b, s};
use crate::theme::Theme;
use crate::ui::kit::{self, Input};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::cmp::Ordering;

/// What a New task form starts with (from a goal page, an idle terminal, a backlog issue…).
#[derive(Clone, Debug, Default)]
pub struct TaskFormOpts {
    pub project: Option<String>,
    pub goal_id: Option<i64>,
    /// Pick up in this idle terminal (`pickup.mode = attach`). A native addition: the Sessions
    /// page's "New task here".
    pub session_id: Option<String>,
    /// Add to the goal's plan (`status: planned`).
    pub planned: bool,
}

pub enum Modal {
    Alerts,
    Task(TaskForm),
    Goal(GoalForm),
    Issue(IssueForm),
}

// ------------------------------------------------------------------ the board as forms see it

/// What the forms read from the board (`S.state`, `S.goalsAll`, `S.filter`).
pub struct Board<'a> {
    pub state: &'a Value,
    /// `GET /goals` (`S.goalsAll`), or `state.goals` until it answers.
    pub goals: &'a [Value],
    pub filter_project: &'a str,
    pub filter_goal: &'a str,
}

impl<'a> Board<'a> {
    pub fn of(m: &'a MainWindow) -> Board<'a> {
        let goals = if m.data.goals.is_empty() { arr(m.state(), "goals") } else { m.data.goals.as_slice() };
        Board { state: m.state(), goals, filter_project: &m.filters.project, filter_goal: &m.filters.goal }
    }

    /// `allGoals`: every goal not archived.
    fn all_goals(&self) -> impl Iterator<Item = &'a Value> {
        self.goals.iter().filter(|g| !b(g, "archived"))
    }

    /// `goalByRef`.
    pub fn goal_by_ref(&self, r: &str) -> Option<&'a Value> {
        let want = web_ref(&Value::String(r.to_string()), "G");
        if want.is_empty() {
            return None;
        }
        self.all_goals().find(|g| web_ref(g, "G") == want)
    }

    /// `projectNames` (see [`crate::app::project_names`]).
    pub fn project_names(&self) -> Vec<String> {
        crate::app::project_names(self.state, self.all_goals())
    }

    /// `projectPaths`.
    fn project_path(&self, name: &str) -> Option<&'a str> {
        arr(self.state, "projects").iter().find(|p| s(p, "name") == name).and_then(|p| fmt::opt_s(p, "path"))
    }

    pub fn jira_on(&self) -> bool {
        b(&self.state["jira"], "enabled")
    }

    /// `idleTerminals`: terminals that can take a task, in `project` (any when empty).
    pub fn idle_terminals(&self, project: &str) -> Vec<&'a Value> {
        arr(self.state, "sessions").iter().filter(|x| b(x, "can_take") && (project.is_empty() || s(x, "project") == project)).collect()
    }

    /// `defaultProject`.
    fn default_project(&self) -> String {
        if self.filter_project != "all" && !self.filter_project.is_empty() {
            return self.filter_project.to_string();
        }
        self.project_names().into_iter().next().unwrap_or_default()
    }

    /// The board's goal filter as a ref (`S.filter.goal`), "" when it's "all".
    fn filter_goal_ref(&self) -> String {
        if self.filter_goal.is_empty() || self.filter_goal == "all" { String::new() } else { web_ref(&Value::String(self.filter_goal.to_string()), "G") }
    }
}

/// The web's `ref(x, p)`: an object's `ref` (or p+id); a string like `g3` upper-cased; else p+x.
pub fn web_ref(x: &Value, p: &str) -> String {
    match x {
        Value::Null => String::new(),
        Value::Object(_) => match x.get("ref").and_then(Value::as_str).filter(|r| !r.is_empty()) {
            Some(r) => r.to_string(),
            None => match x.get("id") {
                Some(Value::Null) | None => String::new(),
                Some(id) => format!("{p}{}", id.as_str().map(str::to_string).unwrap_or_else(|| id.to_string())),
            },
        },
        Value::String(s) if s.is_empty() => String::new(),
        Value::String(s) => {
            let mut c = s.chars();
            let ok = c.next().is_some_and(|f| f.is_ascii_alphabetic()) && s.len() > 1 && c.all(|d| d.is_ascii_digit());
            if ok { s.to_uppercase() } else { format!("{p}{s}") }
        }
        other => format!("{p}{other}"),
    }
}

/// The web's `idOf`: the number in a ref (`G3` → 3).
fn id_of(r: &str) -> Option<i64> {
    if r.is_empty() {
        return None;
    }
    r.trim_start_matches(|c: char| c.is_ascii_alphabetic()).parse().ok()
}

/// JavaScript's `a.localeCompare(b)` (ICU root collation) for the names the board shows:
/// whitespace, then punctuation in ICU's order, then digits, then letters ignoring case; accents
/// break ties before case, and lower case sorts before upper case.
pub fn locale_cmp(a: &str, b: &str) -> Ordering {
    const PUNCT: &str = "_-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";
    fn base(c: char) -> (char, bool) {
        let plain = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'A',
            'ç' => 'c',
            'Ç' => 'C',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'È' | 'É' | 'Ê' | 'Ë' => 'E',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'Ì' | 'Í' | 'Î' | 'Ï' => 'I',
            'ñ' => 'n',
            'Ñ' => 'N',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => 'O',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'Ù' | 'Ú' | 'Û' | 'Ü' => 'U',
            'ý' | 'ÿ' => 'y',
            'Ý' => 'Y',
            _ => return (c, false),
        };
        (plain, true)
    }
    fn keys(s: &str) -> (Vec<(u8, u32)>, Vec<u8>, Vec<u8>) {
        let (mut p, mut sec, mut ter) = (Vec::new(), Vec::new(), Vec::new());
        for c in s.chars() {
            let (c, accent) = base(c);
            let primary = if c.is_whitespace() {
                (0, 0)
            } else if let Some(i) = PUNCT.find(c) {
                (1, i as u32)
            } else if c.is_ascii_digit() {
                (2, c as u32)
            } else if c.is_alphabetic() {
                (3, c.to_lowercase().next().unwrap_or(c) as u32)
            } else {
                (4, c as u32)
            };
            p.push(primary);
            sec.push(accent as u8);
            ter.push(c.is_uppercase() as u8);
        }
        (p, sec, ter)
    }
    let (ka, kb) = (keys(a), keys(b));
    ka.0.cmp(&kb.0).then_with(|| ka.1.cmp(&kb.1)).then_with(|| ka.2.cmp(&kb.2))
}

// ------------------------------------------------------------------ form values (S.modal)

/// The task form's values, as the web keeps them in `S.modal` (`goal` is a ref, "" for none).
#[derive(Clone, Debug, PartialEq)]
pub struct TaskVals {
    pub title: String,
    pub detail: String,
    pub project: String,
    pub goal: String,
    pub planned: bool,
    /// normal | high
    pub priority: String,
    /// queue | new | attach | manual
    pub pickup: String,
    pub session: String,
    /// create | link | none
    pub jira_mode: String,
    pub jira_key: String,
    pub auto_close: bool,
    /// Picked up from a draft (shows the draft note).
    pub restored: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GoalVals {
    /// The goal being edited (its ref), None for a new one.
    pub id: Option<String>,
    pub name: String,
    pub tldr: String,
    pub outcome: String,
    pub project: String,
    /// create | link | none for a new goal; keep when editing.
    pub epic_mode: String,
    pub epic_key: String,
    pub run_in_order: bool,
    pub max_terminals: i64,
    pub auto_close: bool,
    pub restored: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IssueVals {
    pub title: String,
    /// bug | gap | follow | clean
    pub kind: String,
    pub goal: String,
    pub project: String,
    pub said: String,
    pub detail: String,
}

/// The web's `x !== false && x !== 0` (missing counts as on).
fn on_unless_off(g: &Value, k: &str) -> bool {
    match g.get(k) {
        Some(Value::Bool(false)) => false,
        Some(Value::Number(n)) => n.as_f64() != Some(0.),
        _ => true,
    }
}

/// `Number(x) || 2`.
fn number_or_2(v: &Value) -> i64 {
    let n = match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.),
        Value::String(s) if s.trim().is_empty() => 0.,
        Value::String(s) => s.trim().parse::<f64>().unwrap_or(0.),
        Value::Bool(true) => 1.,
        _ => 0.,
    };
    if n == 0. || n.is_nan() { 2 } else { n as i64 }
}

/// `openTaskForm(o)` (with `draft` = the saved `tb.draft.task`, used only when no goal was given).
pub fn open_task_vals(board: &Board, opts: &TaskFormOpts, draft: Option<&Value>) -> TaskVals {
    let o_goal = opts.goal_id.map(|id| format!("G{id}")).unwrap_or_default();
    let g = if o_goal.is_empty() { None } else { board.goal_by_ref(&o_goal) };
    let project = g.map(|g| s(g, "project")).filter(|p| !p.is_empty()).map(str::to_string).or_else(|| opts.project.clone().filter(|p| !p.is_empty())).unwrap_or_else(|| board.default_project());
    let goal = match g {
        Some(g) => web_ref(g, "G"),
        None if !o_goal.is_empty() => o_goal.clone(),
        None => board.filter_goal_ref(),
    };
    let mut v = TaskVals {
        title: String::new(),
        detail: String::new(),
        project,
        goal,
        planned: opts.planned,
        priority: "normal".into(),
        pickup: "queue".into(),
        session: String::new(),
        jira_mode: "none".into(),
        jira_key: String::new(),
        auto_close: g.is_none_or(|g| on_unless_off(g, "auto_close")),
        restored: false,
    };
    if o_goal.is_empty() {
        if let Some(d) = draft.filter(|d| has_words(d)) {
            let st = |k: &str, cur: &str| d.get(k).map(|x| x.as_str().map(str::to_string).unwrap_or_default()).unwrap_or_else(|| cur.to_string());
            v.title = st("title", &v.title);
            v.detail = st("detail", &v.detail);
            v.project = st("project", &v.project);
            v.goal = st("goal", &v.goal);
            v.priority = st("priority", &v.priority);
            v.pickup = st("pickup", &v.pickup);
            v.session = st("session", &v.session);
            v.jira_mode = st("jiraMode", &v.jira_mode);
            v.jira_key = st("jiraKey", &v.jira_key);
            if let Some(a) = d.get("auto_close") {
                v.auto_close = a.as_bool().unwrap_or(false);
            }
            v.restored = true;
        }
    }
    // Native: "New task here" on the Sessions page hands it to that terminal.
    if let Some(sid) = &opts.session_id {
        v.pickup = "attach".into();
        v.session = sid.clone();
    }
    v
}

/// `openGoalForm(g)` (with `draft` = the saved `tb.draft.goal`, used only for a new goal).
pub fn open_goal_vals(board: &Board, goal: Option<&Value>, draft: Option<&Value>) -> GoalVals {
    match goal {
        Some(g) => GoalVals {
            id: Some(web_ref(g, "G")),
            name: s(g, "name").into(),
            tldr: s(g, "tldr").into(),
            outcome: s(g, "outcome").into(),
            project: s(g, "project").into(),
            epic_mode: "keep".into(),
            epic_key: s(g, "epic_key").into(),
            run_in_order: on_unless_off(g, "run_in_order"),
            max_terminals: number_or_2(g.get("max_terminals").unwrap_or(&Value::Null)),
            auto_close: on_unless_off(g, "auto_close"),
            restored: false,
        },
        None => {
            let mut v = GoalVals {
                id: None,
                name: String::new(),
                tldr: String::new(),
                outcome: String::new(),
                project: board.default_project(),
                epic_mode: "none".into(),
                epic_key: String::new(),
                run_in_order: true,
                max_terminals: 2,
                auto_close: true,
                restored: false,
            };
            if let Some(d) = draft.filter(|d| has_words(d)) {
                let st = |k: &str, cur: &str| d.get(k).map(|x| x.as_str().map(str::to_string).unwrap_or_default()).unwrap_or_else(|| cur.to_string());
                v.name = st("name", &v.name);
                v.tldr = st("tldr", &v.tldr);
                v.outcome = st("outcome", &v.outcome);
                v.project = st("project", &v.project);
                v.epic_mode = st("epicMode", &v.epic_mode);
                v.epic_key = st("epicKey", &v.epic_key);
                if let Some(x) = d.get("run_in_order") {
                    v.run_in_order = x.as_bool().unwrap_or(false);
                }
                if let Some(x) = d.get("max_terminals") {
                    v.max_terminals = number_or_2(x);
                }
                if let Some(x) = d.get("auto_close") {
                    v.auto_close = x.as_bool().unwrap_or(false);
                }
                v.restored = true;
            }
            v
        }
    }
}

/// `openNewIssue(goalRef)`. `backlog` = (on the Backlog page, its project filter, its goal filter).
pub fn open_issue_vals(board: &Board, goal_ref: Option<&str>, backlog: Option<(&str, &str)>) -> IssueVals {
    let goal = match goal_ref.filter(|g| !g.is_empty()) {
        Some(r) => web_ref(&Value::String(r.to_string()), "G"),
        None => match backlog {
            Some((_, g)) if !g.is_empty() && g != "all" && g != "none" => web_ref(&Value::String(g.to_string()), "G"),
            _ => String::new(),
        },
    };
    let gg = if goal.is_empty() { None } else { board.goal_by_ref(&goal) };
    let project = gg
        .map(|g| s(g, "project").to_string())
        .filter(|p| !p.is_empty())
        .or_else(|| backlog.map(|(p, _)| p).filter(|p| !p.is_empty() && *p != "all").map(str::to_string))
        .unwrap_or_else(|| board.default_project());
    IssueVals { title: String::new(), kind: "bug".into(), goal, project, said: String::new(), detail: String::new() }
}

// ------------------------------------------------------------------ requests (submit*)

/// `/^[A-Z][A-Z0-9_]*-\d+$/`.
fn is_key(k: &str) -> bool {
    let Some((a, n)) = k.split_once('-') else { return false };
    a.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && a.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !n.is_empty()
        && n.chars().all(|c| c.is_ascii_digit())
}

/// `submitTask`: the error to show, or the request to send.
pub fn task_request(v: &TaskVals, board: &Board) -> Result<(String, Value), String> {
    let title = v.title.trim();
    let key = v.jira_key.trim().to_uppercase();
    let planned = !v.goal.is_empty() && v.planned;
    let terms = board.idle_terminals(&v.project);
    let pickup = if v.pickup == "attach" && terms.is_empty() { "queue" } else { v.pickup.as_str() };
    // The web sent a terminal picked before the project changed; here it must be one of the
    // project's idle terminals (the web's own select showed "Pick a terminal" for it).
    let session = if terms.iter().any(|t| s(t, "id") == v.session) { v.session.as_str() } else { "" };
    let err = if title.is_empty() {
        "Give the task a title."
    } else if v.project.is_empty() {
        "Pick a project."
    } else if !planned && pickup == "attach" && session.is_empty() {
        "Pick the terminal to hand it to."
    } else if board.jira_on() && v.jira_mode == "link" && !is_key(&key) {
        "Enter a ticket key like PROJ-123."
    } else {
        ""
    };
    if !err.is_empty() {
        return Err(err.into());
    }
    let mut body = json!({
        "title": title, "detail": v.detail.trim(), "project": v.project,
        "priority": if v.priority == "high" { "high" } else { "normal" },
        "goal_id": id_of(&v.goal), "auto_close": v.auto_close,
        "pickup": if planned { json!({"mode": "queue"}) } else if pickup == "attach" { json!({"mode": "attach", "session_id": session}) } else { json!({"mode": pickup}) },
    });
    if planned {
        body["status"] = json!("planned");
    }
    if board.jira_on() {
        body["jira"] = if v.jira_mode == "link" { json!({"mode": "link", "key": key}) } else { json!({"mode": if v.jira_mode == "create" { "create" } else { "none" }}) };
    }
    Ok(("tasks".into(), body))
}

/// `submitGoal`.
pub fn goal_request(v: &GoalVals, board: &Board) -> Result<(String, Value), String> {
    let name = v.name.trim();
    let key = v.epic_key.trim().to_uppercase();
    let jira = board.jira_on();
    let need_key = jira && if v.id.is_some() { !key.is_empty() } else { v.epic_mode == "link" };
    let err = if name.is_empty() {
        "Give the goal a name."
    } else if v.project.is_empty() {
        "Pick a project."
    } else if need_key && !is_key(&key) {
        "Enter an epic key like PROJ-123."
    } else {
        ""
    };
    if !err.is_empty() {
        return Err(err.into());
    }
    let mut body = json!({
        "name": name, "tldr": v.tldr.trim(), "outcome": v.outcome.trim(), "project": v.project,
        "run_in_order": v.run_in_order, "max_terminals": if v.max_terminals == 0 { 2 } else { v.max_terminals }, "auto_close": v.auto_close,
    });
    match &v.id {
        Some(r) => {
            if jira {
                body["epic_key"] = if key.is_empty() { Value::Null } else { json!(key) };
            }
            Ok((format!("goals/{r}"), body))
        }
        None => {
            body["epic"] = if !jira {
                json!({"mode": "none"})
            } else if v.epic_mode == "link" {
                json!({"mode": "link", "key": key})
            } else {
                json!({"mode": v.epic_mode})
            };
            Ok(("goals".into(), body))
        }
    }
}

/// `submitNewIssue`.
pub fn issue_request(v: &IssueVals) -> Result<(String, Value), String> {
    let title = v.title.trim();
    if title.is_empty() {
        return Err("Give the issue a title.".into());
    }
    if v.project.is_empty() {
        return Err("Pick a project.".into());
    }
    let mut body = json!({"title": title, "kind": v.kind, "goal_id": id_of(&v.goal), "project": v.project});
    if !v.said.trim().is_empty() {
        body["said"] = json!(v.said.trim());
    }
    if !v.detail.trim().is_empty() {
        body["detail"] = json!(v.detail.trim());
    }
    Ok(("backlog".into(), body))
}

// ------------------------------------------------------------------ drafts

/// `hasWords`.
fn has_words(d: &Value) -> bool {
    ["title", "detail", "name", "tldr", "outcome"].iter().any(|k| d[*k].as_str().is_some_and(|x| !x.trim().is_empty()))
}

/// `saveDraft` for a new task: Some(draft) to keep, None to remove it.
pub fn task_draft(v: &TaskVals) -> Option<Value> {
    let d = json!({
        "title": v.title, "detail": v.detail, "project": v.project, "goal": v.goal, "priority": v.priority,
        "pickup": v.pickup, "session": v.session, "jiraMode": v.jira_mode, "jiraKey": v.jira_key, "auto_close": v.auto_close,
    });
    has_words(&d).then_some(d)
}

/// `saveDraft` for a new goal (never for an edit).
pub fn goal_draft(v: &GoalVals) -> Option<Value> {
    if v.id.is_some() {
        return None;
    }
    let d = json!({
        "name": v.name, "tldr": v.tldr, "outcome": v.outcome, "project": v.project, "epicMode": v.epic_mode,
        "epicKey": v.epic_key, "run_in_order": v.run_in_order, "max_terminals": v.max_terminals, "auto_close": v.auto_close,
    });
    has_words(&d).then_some(d)
}

/// A form's values, for drafts (an issue form never has one).
pub enum FormVals {
    Task(TaskVals),
    Goal(GoalVals),
    Issue,
}

/// `saveDraft`: which draft to write (Some(draft)) or remove (None); None when nothing is touched
/// (an issue, an edit, or a form being sent).
pub fn draft_write(form: &FormVals, busy: bool) -> Option<(&'static str, Option<Value>)> {
    if busy {
        return None;
    }
    match form {
        FormVals::Task(v) => Some((DRAFT_TASK, task_draft(v))),
        FormVals::Goal(v) if v.id.is_none() => Some((DRAFT_GOAL, goal_draft(v))),
        _ => None,
    }
}

const DRAFT_TASK: &str = "tb.draft.task";
const DRAFT_GOAL: &str = "tb.draft.goal";

/// `loadDraft`.
fn load_draft(key: &str) -> Option<Value> {
    crate::prefs::get(key).filter(has_words)
}

/// Write (or remove) a draft only when it changed.
fn store_draft(key: &str, d: Option<Value>) {
    match d {
        Some(d) => crate::prefs::set(key, d),
        None => crate::prefs::remove(key),
    }
}

// ------------------------------------------------------------------ pickers

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickKind {
    Project,
    Goal,
}

/// One row in a picker (`{v, name, sub, special}` plus whether it's the current value).
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub v: String,
    pub name: String,
    pub sub: String,
    pub special: bool,
    pub current: bool,
}

/// `PICKERS[kind].search`.
pub fn picker_search(kind: PickKind) -> &'static str {
    match kind {
        PickKind::Project => "Search projects",
        PickKind::Goal => "Search goals by name, project or epic",
    }
}

/// What an empty picker list says (`${PICKERS[kind].none} “q”.`).
pub fn picker_empty(kind: PickKind, q: &str) -> String {
    let none = match kind {
        PickKind::Project => "No project matches",
        PickKind::Goal => "No goal matches",
    };
    format!("{none} “{}”.", q.trim())
}

/// `pickerBase` + `pickerItems`: the list for search `q`. `proj` limits goals to one project.
pub fn picker_items(board: &Board, kind: PickKind, value: &str, specials: &[(&str, &str)], proj: &str, q: &str) -> Vec<Item> {
    let mut items: Vec<Item> = match kind {
        PickKind::Goal => {
            let mut v: Vec<Item> = board
                .all_goals()
                .filter(|g| proj.is_empty() || proj == "all" || s(g, "project") == proj)
                .map(|g| Item {
                    v: web_ref(g, "G"),
                    name: s(g, "name").into(),
                    sub: [s(g, "project"), s(g, "epic_key")].iter().filter(|x| !x.is_empty()).copied().collect::<Vec<_>>().join(" · "),
                    special: false,
                    current: false,
                })
                .collect();
            v.sort_by(|a, b| locale_cmp(&a.name, &b.name));
            v
        }
        PickKind::Project => {
            let mut names = board.project_names();
            if !value.is_empty() && !specials.iter().any(|(v, _)| *v == value) && !names.iter().any(|n| n == value) {
                names.push(value.to_string());
            }
            names
                .into_iter()
                .map(|n| {
                    let sub = board.project_path(&n).map(home_short).unwrap_or_default();
                    Item { v: n.clone(), name: n, sub, special: false, current: false }
                })
                .collect()
        }
    };
    let q = q.trim().to_lowercase();
    if !q.is_empty() {
        let rank = |it: &Item| {
            let l = it.name.to_lowercase();
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
        items.retain(|it| it.name.to_lowercase().contains(&q) || (kind != PickKind::Project && it.sub.to_lowercase().contains(&q)));
        items.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| locale_cmp(&a.name, &b.name)));
    } else {
        let mut all: Vec<Item> = specials.iter().map(|(v, n)| Item { v: v.to_string(), name: n.to_string(), sub: String::new(), special: true, current: false }).collect();
        all.extend(items);
        items = all;
    }
    for it in &mut items {
        it.current = it.v == value;
    }
    items
}

/// `openPicker`: the row highlighted when a picker opens (the current value, else the first).
pub fn picker_start(items: &[Item], value: &str) -> usize {
    items.iter().position(|it| it.v == value).unwrap_or(0)
}

/// `/Users/alex/x` → `~/x` (the picker's path line).
fn home_short(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("/Users/") {
        if let Some(i) = rest.find('/') {
            return format!("~{}", &rest[i..]);
        }
        return "~".into();
    }
    p.to_string()
}

/// The label on a picker's button (`pickerButton`).
pub fn picker_label(board: &Board, kind: PickKind, value: &str, specials: &[(&str, &str)]) -> String {
    if let Some((_, l)) = specials.iter().find(|(v, _)| *v == value) {
        return l.to_string();
    }
    match kind {
        PickKind::Goal => match (!value.is_empty()).then(|| board.goal_by_ref(value)).flatten() {
            Some(g) => s(g, "name").into(),
            None => "Choose a goal".into(),
        },
        PickKind::Project if value.is_empty() => "Choose a project".into(),
        PickKind::Project => value.into(),
    }
}

const NO_GOAL: [(&str, &str); 1] = [("", "Not in a goal")];

// ------------------------------------------------------------------ what the forms show

pub const PICKUPS: [(&str, &str); 4] =
    [("queue", "When the repo’s free"), ("new", "Now, in a new terminal"), ("attach", "In an idle terminal"), ("manual", "Only when I press Start")];
pub const KINDS: [(&str, &str); 4] = [("bug", "Bug"), ("gap", "Test gap"), ("follow", "Follow-up"), ("clean", "Clean-up")];

fn pickup_help(p: &str) -> &'static str {
    match p {
        "new" => "It starts in a new Midna terminal right away, even if another task is working in this project.",
        "attach" => "The terminal you pick gets the handoff as its next prompt.",
        "manual" => "It waits in Queued until you press Start on it.",
        _ => "It starts in a new Midna terminal once no other task is working in this project, inside work hours.",
    }
}

fn epic_help(e: &str) -> &'static str {
    match e {
        "create" => "The board creates the epic in your Jira project. Tasks’ tickets go under it.",
        "link" => "Tasks’ tickets go under this epic.",
        "none" => "You can add an epic later.",
        _ => "",
    }
}

const HELP_DETAIL: &str = "The terminal gets this in its handoff.";
const HELP_PLANNED: &str = "It joins the goal’s plan and starts when you press Start on the goal.";
const HELP_TLDR: &str = "The problem and what's changing, in plain words. It sits at the top of the goal page.";
const HELP_OUTCOME: &str = "What’s true once the goal is finished. Every task in it sees this.";
const HELP_ISSUE: &str = "It goes in the backlog. Nothing starts until you make it a task.";
const CHECK_PLANNED: &str = "Add it to the goal’s plan instead of the queue";
const CHECK_TASK_CLOSE: &str = "Close its terminal when it’s done";
const CHECK_ORDER: &str = "Run the tasks in order, one after another";
const CHECK_GOAL_CLOSE: &str = "Close each terminal when its task is done";

/// `draftNote`.
fn draft_note(kind: &str) -> String {
    format!("Picked up the {kind} you hadn’t added yet.")
}

/// One choice in a segmented control or the Start options.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub id: &'static str,
    pub label: &'static str,
    pub pressed: bool,
    pub disabled: bool,
}

fn choices(opts: &[(&'static str, &'static str)], cur: &str) -> Vec<Choice> {
    opts.iter().map(|(id, label)| Choice { id, label, pressed: *id == cur, disabled: false }).collect()
}

/// What the task form shows.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskView {
    pub project_label: String,
    pub goal_label: String,
    /// The "add to the goal's plan" checkbox (Some(on) when a goal is chosen).
    pub planned: Option<bool>,
    pub priority: Vec<Choice>,
    /// None when it joins the goal's plan (the planned help shows instead).
    pub start: Option<Vec<Choice>>,
    /// The terminal list when "In an idle terminal" is on: ("", "Pick a terminal") first.
    pub terminals: Vec<(String, String)>,
    pub help: &'static str,
    /// Jira ticket choices (Jira on) and whether the key field shows.
    pub jira: Option<(Vec<Choice>, bool)>,
    pub auto_close: bool,
    pub draft: Option<String>,
    pub err: Option<String>,
    pub submit: &'static str,
}

pub fn task_view(v: &TaskVals, board: &Board, err: Option<&str>, busy: bool) -> TaskView {
    let terms = board.idle_terminals(&v.project);
    let pickup = if v.pickup == "attach" && terms.is_empty() { "queue" } else { v.pickup.as_str() };
    let planned_now = !v.goal.is_empty() && v.planned;
    let start = (!planned_now).then(|| {
        PICKUPS.iter().map(|(id, label)| Choice { id, label, pressed: *id == pickup, disabled: *id == "attach" && terms.is_empty() }).collect::<Vec<_>>()
    });
    let mut terminals = Vec::new();
    if !planned_now && pickup == "attach" {
        terminals.push((String::new(), "Pick a terminal".to_string()));
        terminals.extend(terms.iter().map(|t| (s(t, "id").to_string(), s(t, "name").to_string())));
    }
    TaskView {
        project_label: picker_label(board, PickKind::Project, &v.project, &[]),
        goal_label: picker_label(board, PickKind::Goal, &v.goal, &NO_GOAL),
        planned: (!v.goal.is_empty()).then_some(v.planned),
        priority: choices(&[("normal", "Normal"), ("high", "High")], &v.priority),
        start,
        terminals,
        help: if planned_now { HELP_PLANNED } else { pickup_help(pickup) },
        jira: board.jira_on().then(|| (choices(&[("create", "Create one"), ("link", "Link existing"), ("none", "None")], &v.jira_mode), v.jira_mode == "link")),
        auto_close: v.auto_close,
        draft: v.restored.then(|| draft_note("task")),
        err: err.filter(|e| !e.is_empty()).map(str::to_string),
        submit: if busy { "Adding…" } else { "Add task" },
    }
}

/// What the goal form shows.
#[derive(Clone, Debug, PartialEq)]
pub struct GoalView {
    pub title: &'static str,
    pub project_label: String,
    /// Edit with Jira: the epic key field (label, placeholder, help).
    pub epic_field: Option<(&'static str, &'static str)>,
    /// New with Jira: the epic choices, whether the key field shows, and the help.
    pub epic_choices: Option<(Vec<Choice>, bool, &'static str)>,
    pub max_terminals: i64,
    pub run_in_order: bool,
    pub auto_close: bool,
    pub draft: Option<String>,
    pub err: Option<String>,
    pub submit: &'static str,
}

pub fn goal_view(v: &GoalVals, board: &Board, err: Option<&str>, busy: bool) -> GoalView {
    let jira = board.jira_on();
    GoalView {
        title: if v.id.is_some() { "Edit goal" } else { "New goal" },
        project_label: picker_label(board, PickKind::Project, &v.project, &[]),
        epic_field: (jira && v.id.is_some()).then_some(("PROJ-123, or leave empty", "Tasks’ tickets go under this epic.")),
        epic_choices: (jira && v.id.is_none())
            .then(|| (choices(&[("create", "Create an epic"), ("link", "Link existing"), ("none", "None")], &v.epic_mode), v.epic_mode == "link", epic_help(&v.epic_mode))),
        max_terminals: v.max_terminals,
        run_in_order: v.run_in_order,
        auto_close: v.auto_close,
        draft: v.restored.then(|| draft_note("goal")),
        err: err.filter(|e| !e.is_empty()).map(str::to_string),
        submit: if busy { "Saving…" } else if v.id.is_some() { "Save goal" } else { "Add goal" },
    }
}

/// What the issue form shows.
#[derive(Clone, Debug, PartialEq)]
pub struct IssueView {
    pub goal_label: String,
    pub project_label: String,
    pub kinds: Vec<Choice>,
    pub err: Option<String>,
    pub submit: &'static str,
}

pub fn issue_view(v: &IssueVals, board: &Board, err: Option<&str>, busy: bool) -> IssueView {
    IssueView {
        goal_label: picker_label(board, PickKind::Goal, &v.goal, &NO_GOAL),
        project_label: picker_label(board, PickKind::Project, &v.project, &[]),
        kinds: choices(&KINDS, &v.kind),
        err: err.filter(|e| !e.is_empty()).map(str::to_string),
        submit: if busy { "Adding…" } else { "Add issue" },
    }
}

// ------------------------------------------------------------------ form state (entities)

/// A picker open in a form: which one, its search field, and the highlighted row.
pub struct Picker {
    pub open: Option<PickKind>,
    pub search: Input,
    pub active: usize,
    /// The search text the active row was chosen for (typing moves it back to the top).
    last_q: String,
}

impl Picker {
    fn new(cx: &mut App) -> Picker {
        Picker { open: None, search: Input::new(cx, "", false), active: 0, last_q: String::new() }
    }
}

pub struct TaskForm {
    pub title: Input,
    pub detail: Input,
    pub jira_key: Input,
    /// Everything but the text fields (those live in the inputs).
    pub v: TaskVals,
    pub picker: Picker,
    pub err: Option<String>,
    pub busy: bool,
}

pub struct GoalForm {
    pub name: Input,
    pub tldr: Input,
    pub outcome: Input,
    pub epic_key: Input,
    pub v: GoalVals,
    pub picker: Picker,
    pub err: Option<String>,
    pub busy: bool,
}

pub struct IssueForm {
    pub title: Input,
    pub said: Input,
    pub detail: Input,
    pub v: IssueVals,
    pub picker: Picker,
    pub err: Option<String>,
    pub busy: bool,
}

impl TaskForm {
    pub fn vals(&self, cx: &App) -> TaskVals {
        TaskVals { title: self.title.text(cx), detail: self.detail.text(cx), jira_key: self.jira_key.text(cx), ..self.v.clone() }
    }
}

impl GoalForm {
    pub fn vals(&self, cx: &App) -> GoalVals {
        GoalVals { name: self.name.text(cx), tldr: self.tldr.text(cx), outcome: self.outcome.text(cx), epic_key: self.epic_key.text(cx), ..self.v.clone() }
    }
}

impl IssueForm {
    pub fn vals(&self, cx: &App) -> IssueVals {
        IssueVals { title: self.title.text(cx), said: self.said.text(cx), detail: self.detail.text(cx), ..self.v.clone() }
    }
}

impl Modal {
    /// The text fields in tab order.
    fn fields(&self) -> Vec<FocusHandle> {
        match self {
            Modal::Alerts => vec![],
            Modal::Task(f) => {
                let mut v = vec![f.title.focus.clone(), f.detail.focus.clone()];
                if f.v.jira_mode == "link" {
                    v.push(f.jira_key.focus.clone());
                }
                v
            }
            Modal::Goal(f) => {
                let mut v = vec![f.name.focus.clone(), f.tldr.focus.clone(), f.outcome.focus.clone()];
                if f.v.id.is_some() || f.v.epic_mode == "link" {
                    v.push(f.epic_key.focus.clone());
                }
                v
            }
            Modal::Issue(f) => vec![f.title.focus.clone(), f.said.focus.clone(), f.detail.focus.clone()],
        }
    }

    pub fn focus_first(&self, window: &mut Window, cx: &mut App) {
        if let Some(f) = self.fields().first() {
            window.focus(f, cx);
        }
    }

    fn picker(&self) -> Option<&Picker> {
        match self {
            Modal::Task(f) => Some(&f.picker),
            Modal::Goal(f) => Some(&f.picker),
            Modal::Issue(f) => Some(&f.picker),
            Modal::Alerts => None,
        }
    }

    fn picker_mut(&mut self) -> Option<&mut Picker> {
        match self {
            Modal::Task(f) => Some(&mut f.picker),
            Modal::Goal(f) => Some(&mut f.picker),
            Modal::Issue(f) => Some(&mut f.picker),
            Modal::Alerts => None,
        }
    }

    fn set_err(&mut self, e: Option<String>) {
        match self {
            Modal::Task(f) => (f.err, f.busy) = (e, false),
            Modal::Goal(f) => (f.err, f.busy) = (e, false),
            Modal::Issue(f) => (f.err, f.busy) = (e, false),
            Modal::Alerts => {}
        }
    }

    /// The value a picker of `kind` is choosing, and its special rows.
    fn pick_value(&self, kind: PickKind) -> (String, &'static [(&'static str, &'static str)]) {
        match (self, kind) {
            (Modal::Task(f), PickKind::Project) => (f.v.project.clone(), &[]),
            (Modal::Task(f), PickKind::Goal) => (f.v.goal.clone(), &NO_GOAL),
            (Modal::Goal(f), _) => (f.v.project.clone(), &[]),
            (Modal::Issue(f), PickKind::Project) => (f.v.project.clone(), &[]),
            (Modal::Issue(f), PickKind::Goal) => (f.v.goal.clone(), &NO_GOAL),
            (Modal::Alerts, _) => (String::new(), &[]),
        }
    }
}

// ------------------------------------------------------------------ opening

fn text_input(cx: &mut App, placeholder: &str, multi: bool, text: &str) -> Input {
    Input::with_text(cx, placeholder.to_string(), multi, text)
}

fn task_form(v: TaskVals, cx: &mut App) -> TaskForm {
    TaskForm {
        title: text_input(cx, "", false, &v.title),
        detail: text_input(cx, "", true, &v.detail),
        jira_key: text_input(cx, "PROJ-123", false, &v.jira_key),
        v,
        picker: Picker::new(cx),
        err: None,
        busy: false,
    }
}

fn goal_form(v: GoalVals, cx: &mut App) -> GoalForm {
    let key_hint = if v.id.is_some() { "PROJ-123, or leave empty" } else { "PROJ-123" };
    GoalForm {
        name: text_input(cx, "", false, &v.name),
        tldr: text_input(cx, "", true, &v.tldr),
        outcome: text_input(cx, "", true, &v.outcome),
        epic_key: text_input(cx, key_hint, false, &v.epic_key),
        v,
        picker: Picker::new(cx),
        err: None,
        busy: false,
    }
}

pub fn open_task_form(m: &mut MainWindow, opts: TaskFormOpts, window: &mut Window, cx: &mut Context<MainWindow>) {
    let draft = load_draft(DRAFT_TASK);
    let v = open_task_vals(&Board::of(m), &opts, draft.as_ref());
    let form = task_form(v, cx);
    m.set_modal(Some(Modal::Task(form)), window, cx);
}

pub fn open_goal_form(m: &mut MainWindow, goal: Option<Value>, window: &mut Window, cx: &mut Context<MainWindow>) {
    let draft = if goal.is_none() { load_draft(DRAFT_GOAL) } else { None };
    let v = open_goal_vals(&Board::of(m), goal.as_ref(), draft.as_ref());
    let form = goal_form(v, cx);
    m.set_modal(Some(Modal::Goal(form)), window, cx);
}

pub fn open_issue_form(m: &mut MainWindow, goal_ref: Option<String>, window: &mut Window, cx: &mut Context<MainWindow>) {
    let backlog = (m.page == Page::Backlog).then(|| (m.backlog.project.clone(), "all".to_string()));
    let v = open_issue_vals(&Board::of(m), goal_ref.as_deref(), backlog.as_ref().map(|(p, g)| (p.as_str(), g.as_str())));
    let form = IssueForm {
        title: text_input(cx, "", false, ""),
        said: text_input(cx, "", true, ""),
        detail: text_input(cx, "", true, ""),
        v,
        picker: Picker::new(cx),
        err: None,
        busy: false,
    };
    m.set_modal(Some(Modal::Issue(form)), window, cx);
}

/// `closeModal` (nothing opens a form with a `back` modal, so this is always back to none).
fn close(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    m.set_modal(None, window, cx);
}

// ------------------------------------------------------------------ render

/// The modal over a dimmed backdrop. As on the web, clicks on the backdrop do nothing.
pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    // The alerts dialog closes itself once nothing needs you (renderBanner).
    if matches!(m.modal, Some(Modal::Alerts)) && arr(m.state(), "alerts").is_empty() && m.data.state.is_some() {
        m.modal = None;
        window.focus(&m.focus, cx);
        return div().into_any_element();
    }
    save_draft(m, cx);
    let body = match &m.modal {
        Some(Modal::Alerts) => alerts(m, &t, cx),
        Some(Modal::Task(_)) => render_task(m, &t, window, cx),
        Some(Modal::Goal(_)) => render_goal(m, &t, window, cx),
        Some(Modal::Issue(_)) => render_issue(m, &t, window, cx),
        None => return div().into_any_element(),
    };
    deferred(
        kit::scrim(&t, "modal-scrim")
            .flex()
            .items_center()
            .justify_center()
            // Esc inside a dialog: closes an open picker, then the read-only alerts dialog; a
            // form stays (the web board kept forms open so nothing typed is lost).
            .on_action(cx.listener(|m, _: &CloseOverlay, window, cx| on_escape(m, window, cx)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().id("modal-body").flex().flex_col().max_h(relative(0.92)).child(body)),
    )
    .with_priority(1)
    .into_any_element()
}

/// Esc in a dialog.
pub fn on_escape(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    if m.modal.as_ref().and_then(|x| x.picker()).is_some_and(|p| p.open.is_some()) {
        close_picker(m, window, cx);
    } else if matches!(m.modal, Some(Modal::Alerts)) {
        close(m, window, cx);
    }
}

/// `saveDraft`, on every change: a new task or goal that isn't being sent.
fn save_draft(m: &MainWindow, cx: &App) {
    let (form, busy) = match &m.modal {
        Some(Modal::Task(f)) => (FormVals::Task(f.vals(cx)), f.busy),
        Some(Modal::Goal(f)) => (FormVals::Goal(f.vals(cx)), f.busy),
        Some(Modal::Issue(f)) => (FormVals::Issue, f.busy),
        _ => return,
    };
    if let Some((key, d)) = draft_write(&form, busy) {
        store_draft(key, d);
    }
}

/// Title row with a close button.
pub fn head(t: &Theme, title: impl Into<SharedString>, cx: &mut Context<MainWindow>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .px(px(20.))
        .pt(px(18.))
        .pb(px(12.))
        .child(div().flex_1().text_size(px(17.)).font_weight(FontWeight::BOLD).child(title.into()))
        .child(kit::btn_small(t, "modal-close", "Close").on_click(cx.listener(|m, _, window, cx| close(m, window, cx))))
}

fn alerts(m: &MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> AnyElement {
    let list = arr(m.state(), "alerts").to_vec();
    let (title, _) = crate::app::alerts_dialog_view(&list);
    let mut col = div().id("alerts-list").flex().flex_col().overflow_y_scroll();
    for a in &list {
        col = col.child(alert_row(t, a, cx));
    }
    kit::modal_box(t, 560.)
        .child(head(t, title, cx))
        .child(col)
        .child(
            div().flex().justify_end().p(px(16.)).child(
                kit::btn_small(t, "alerts-dismiss-all", "Dismiss all")
                    // The dialog closes by itself once the alerts are gone.
                    .on_click(cx.listener(|m, _, _, cx| crate::app::dismiss_all_alerts(m, cx))),
            ),
        )
        .into_any_element()
}

/// The form's frame: head, draft note, scrolling body, footer with the error, Cancel and submit.
#[allow(clippy::too_many_arguments)]
fn form_shell(
    t: &Theme,
    title: &str,
    draft: Option<String>,
    body: Div,
    err: Option<String>,
    busy: bool,
    submit_label: &str,
    submit: fn(&mut MainWindow, &mut Window, &mut Context<MainWindow>),
    cx: &mut Context<MainWindow>,
) -> AnyElement {
    let mut go = kit::btn_primary(t, "modal-submit", submit_label.to_string()).h(px(34.)).px(px(16.));
    go = if busy { kit::disabled(go) } else { go.on_click(cx.listener(move |m, _, window, cx| submit(m, window, cx))) };
    let note = draft.map(|text| {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .mx(px(20.))
            .mb(px(12.))
            .px(px(12.))
            .py(px(8.))
            .rounded(px(8.))
            .bg(t.accent_soft)
            .text_size(px(13.))
            .text_color(t.text_2)
            .child(text)
            .child(kit::link(t, "draft-discard", "Discard it and start over").on_click(cx.listener(|m, _, window, cx| discard_draft(m, window, cx))))
    });
    kit::modal_box(t, 580.)
        .max_h_full()
        .id("modal-form")
        .on_key_down(cx.listener(move |m, ev: &KeyDownEvent, window, cx| on_form_key(m, ev, submit, window, cx)))
        .child(head(t, title.to_string(), cx))
        .children(note)
        .child(div().id("modal-scroll").flex().flex_col().gap(px(16.)).px(px(20.)).pb(px(18.)).flex_1().min_h_0().overflow_y_scroll().child(body))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(10.))
                .px(px(20.))
                .py(px(14.))
                .border_t_1()
                .border_color(t.divider)
                .bg(t.tint)
                .child(div().flex_1().min_w_0().text_size(px(13.)).text_color(t.down).children(err))
                .child(kit::btn(t, "modal-cancel", "Cancel").h(px(34.)).on_click(cx.listener(|m, _, window, cx| close(m, window, cx))))
                .child(go),
        )
        .into_any_element()
}

/// `draft-discard`: forget the draft and start a fresh form of the same kind.
fn discard_draft(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    match &m.modal {
        Some(Modal::Task(_)) => {
            crate::prefs::remove(DRAFT_TASK);
            close(m, window, cx);
            open_task_form(m, TaskFormOpts::default(), window, cx);
        }
        Some(Modal::Goal(f)) if f.v.id.is_none() => {
            crate::prefs::remove(DRAFT_GOAL);
            close(m, window, cx);
            open_goal_form(m, None, window, cx);
        }
        _ => {}
    }
}

fn on_form_key(m: &mut MainWindow, ev: &KeyDownEvent, submit: fn(&mut MainWindow, &mut Window, &mut Context<MainWindow>), window: &mut Window, cx: &mut Context<MainWindow>) {
    let ks = &ev.keystroke;
    if ks.key == "tab" && !ks.modifiers.platform && !ks.modifiers.control {
        let Some(modal) = &m.modal else { return };
        let fields = modal.fields();
        if fields.is_empty() {
            return;
        }
        let cur = fields.iter().position(|f| f.is_focused(window));
        let n = fields.len();
        let next = match cur {
            Some(c) if ks.modifiers.shift => (c + n - 1) % n,
            Some(c) => (c + 1) % n,
            None => 0,
        };
        window.focus(&fields[next], cx);
        cx.stop_propagation();
        return;
    }
    // ↩ in a one-line field submits (the form's submit on the web); ⌘↩ anywhere.
    let in_search = m.modal.as_ref().and_then(|x| x.picker()).is_some_and(|p| p.search.focus.is_focused(window));
    if in_search {
        return;
    }
    let focused_input = m.modal.as_ref().map(|x| x.fields()).unwrap_or_default().into_iter().find(|f| f.is_focused(window));
    let submits = ks.key == "enter" && (ks.modifiers.platform || (focused_input.is_some() && is_single_line(m, window, cx) && !ks.modifiers.shift));
    if submits {
        cx.stop_propagation();
        submit(m, window, cx);
    }
}

fn is_single_line(m: &MainWindow, window: &Window, cx: &App) -> bool {
    let inputs: Vec<&Input> = match &m.modal {
        Some(Modal::Task(f)) => vec![&f.title, &f.detail, &f.jira_key],
        Some(Modal::Goal(f)) => vec![&f.name, &f.tldr, &f.outcome, &f.epic_key],
        Some(Modal::Issue(f)) => vec![&f.title, &f.said, &f.detail],
        _ => vec![],
    };
    inputs.into_iter().find(|i| i.focus.is_focused(window)).is_some_and(|i| !i.field.read(cx).wrap)
}

// ------------------------------------------------------------------ shared controls

/// A checkbox with a label.
fn check(t: &Theme, id: &str, on: bool, label: &str) -> Stateful<Div> {
    div()
        .id(SharedString::from(id.to_string()))
        .flex()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .text_size(px(13.5))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(16.))
                .rounded(px(4.))
                .border_1()
                .border_color(if on { t.accent_btn } else { t.border_2 })
                .bg(if on { t.accent_btn } else { t.card })
                .text_color(t.on_accent)
                .text_size(px(11.))
                .child(if on { "✓" } else { "" }),
        )
        .child(label.to_string())
}

/// A radio row (one choice of several).
fn radio(t: &Theme, id: &str, on: bool, label: &str) -> Stateful<Div> {
    let hover = t.panel_2;
    div()
        .id(SharedString::from(id.to_string()))
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(28.))
        .px(px(6.))
        .rounded(px(6.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .text_size(px(13.5))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(16.))
                .rounded_full()
                .border_1()
                .border_color(if on { t.accent_btn } else { t.border_2 })
                .bg(t.card)
                .child(div().size(px(8.)).rounded_full().when(on, |d| d.bg(t.accent_btn))),
        )
        .child(label.to_string())
}

/// The button that opens a picker, showing the current value (greyed when nothing is chosen).
fn pick_button(t: &Theme, id: &str, label: &str, empty: bool, open: bool) -> Stateful<Div> {
    let hover = t.panel_2;
    div()
        .id(SharedString::from(id.to_string()))
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(34.))
        .px(px(10.))
        .rounded(px(8.))
        .border_1()
        .border_color(if open { t.accent } else { t.border_2 })
        .bg(t.card)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .text_size(px(13.5))
        .tooltip(kit::tip(format!("{label} · click to search")))
        .child(div().flex_1().min_w_0().truncate().text_color(if empty { t.faint } else { t.text }).child(label.to_string()))
        .child(div().text_color(t.muted).text_size(px(9.)).child(if open { "▲" } else { "▼" }))
}

/// A segmented control over `choices`; a click sets the field with `set` and clears the error
/// (the web's `m-set`).
fn seg_choices(t: &Theme, id: &str, choices: &[Choice], cx: &mut Context<MainWindow>, set: fn(&mut Modal, &'static str)) -> Div {
    let labels: Vec<&str> = choices.iter().map(|c| c.label).collect();
    let on = choices.iter().position(|c| c.pressed).unwrap_or(usize::MAX);
    let ids: Vec<&'static str> = choices.iter().map(|c| c.id).collect();
    div().flex().child(kit::seg(t, id, &labels, on, |n, item| {
        let v = ids[n];
        item.on_click(cx.listener(move |m, _, _, cx| {
            if let Some(x) = m.modal.as_mut() {
                set(x, v);
                x.set_err(None);
            }
            cx.notify();
        }))
    }))
}

/// The open picker's list under its button: a search field and the matching rows.
fn picker_list(m: &mut MainWindow, t: &Theme, kind: PickKind, window: &Window, cx: &mut Context<MainWindow>) -> Div {
    let Some(modal) = m.modal.as_ref() else { return div() };
    let (value, specials) = modal.pick_value(kind);
    let Some(p) = modal.picker() else { return div() };
    let q = p.search.text(cx);
    let search = Input { field: p.search.field.clone(), focus: p.search.focus.clone() };
    let items = picker_items(&Board::of(m), kind, &value, specials, "", &q);
    if let Some(p) = m.modal.as_mut().and_then(|x| x.picker_mut()) {
        if p.last_q != q {
            p.last_q = q.clone();
            p.active = 0;
        }
        p.active = p.active.min(items.len().saturating_sub(1));
    }
    let active = m.modal.as_ref().and_then(|x| x.picker()).map(|p| p.active).unwrap_or(0);
    let mut list = div().id(SharedString::from(format!("pick-list-{kind:?}"))).flex().flex_col().max_h(px(240.)).overflow_y_scroll();
    if items.is_empty() {
        list = list.child(div().px(px(10.)).py(px(8.)).text_size(px(13.)).text_color(t.muted).child(picker_empty(kind, &q)));
    }
    for (n, it) in items.into_iter().enumerate() {
        let v = it.v.clone();
        let mut row = kit::menu_item(t, SharedString::from(format!("pick-{kind:?}-{n}")), it.name, n == active).h_auto().min_h(px(30.)).py(px(4.));
        if it.special {
            row = row.italic();
        }
        row = row
            .children((!it.sub.is_empty()).then(|| div().ml_auto().pl(px(8.)).text_size(px(11.5)).text_color(t.faint).truncate().child(it.sub)))
            .children(it.current.then(|| div().flex_none().ml(px(8.)).text_size(px(11.)).text_color(t.accent_fg).child("Current")))
            .on_click(cx.listener(move |m, _, window, cx| pick_item(m, kind, &v, window, cx)));
        list = list.child(row);
    }
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .p(px(6.))
        .rounded(px(10.))
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .shadow_md()
        .child(search.render(t, SharedString::from(format!("pick-search-{kind:?}")), window).on_key_down(cx.listener(move |m, ev: &KeyDownEvent, window, cx| {
            if picker_key(m, kind, ev.keystroke.key.as_str(), window, cx) {
                cx.stop_propagation();
            }
        })))
        .child(list)
        .child(
            div()
                .flex()
                .gap(px(12.))
                .px(px(6.))
                .pt(px(2.))
                .text_size(px(11.5))
                .text_color(t.faint)
                .child("↑ ↓ to move")
                .child("Enter to choose")
                .child("Esc to close"),
        )
}

/// `pickerKey`: ↑/↓ move (wrapping), ↩ picks, esc closes, ⇥ does nothing. True when handled.
pub fn picker_key(m: &mut MainWindow, kind: PickKind, key: &str, window: &mut Window, cx: &mut Context<MainWindow>) -> bool {
    let Some(modal) = m.modal.as_ref() else { return false };
    let (value, specials) = modal.pick_value(kind);
    let q = modal.picker().map(|p| p.search.text(cx)).unwrap_or_default();
    let items = picker_items(&Board::of(m), kind, &value, specials, "", &q);
    let n = items.len();
    match key {
        "escape" => close_picker(m, window, cx),
        "up" | "down" => {
            if let Some(p) = m.modal.as_mut().and_then(|x| x.picker_mut()) {
                if n > 0 {
                    p.active = if key == "down" { (p.active + 1) % n } else { (p.active + n - 1) % n };
                }
            }
            cx.notify();
        }
        "enter" => {
            let active = m.modal.as_ref().and_then(|x| x.picker()).map(|p| p.active).unwrap_or(0);
            if let Some(it) = items.get(active) {
                let v = it.v.clone();
                pick_item(m, kind, &v, window, cx);
            }
        }
        "tab" => {}
        _ => return false,
    }
    true
}

/// `openPicker`: open it with the search empty and the current value highlighted.
pub fn open_picker(m: &mut MainWindow, kind: PickKind, window: &mut Window, cx: &mut Context<MainWindow>) {
    let Some(modal) = m.modal.as_ref() else { return };
    if modal.picker().is_some_and(|p| p.open == Some(kind)) {
        close_picker(m, window, cx);
        return;
    }
    let (value, specials) = modal.pick_value(kind);
    let items = picker_items(&Board::of(m), kind, &value, specials, "", "");
    let active = picker_start(&items, &value);
    let Some(p) = m.modal.as_mut().and_then(|x| x.picker_mut()) else { return };
    p.open = Some(kind);
    p.search.clear(cx);
    p.search.field.update(cx, |f, _| f.placeholder = picker_search(kind).into());
    p.last_q.clear();
    p.active = active;
    let f = p.search.focus.clone();
    window.focus(&f, cx);
    cx.notify();
}

fn close_picker(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    if let Some(p) = m.modal.as_mut().and_then(|x| x.picker_mut()) {
        p.open = None;
    }
    if let Some(modal) = &m.modal {
        modal.focus_first(window, cx);
    }
    cx.notify();
}

/// `pickItem` + `applyPick`: set just that field (nothing else changes), unless it's the same.
pub fn pick_item(m: &mut MainWindow, kind: PickKind, value: &str, window: &mut Window, cx: &mut Context<MainWindow>) {
    close_picker(m, window, cx);
    let Some(modal) = m.modal.as_mut() else { return };
    if modal.pick_value(kind).0 == value {
        return;
    }
    match (modal, kind) {
        (Modal::Task(f), PickKind::Project) => f.v.project = value.to_string(),
        (Modal::Task(f), PickKind::Goal) => f.v.goal = value.to_string(),
        (Modal::Goal(f), _) => f.v.project = value.to_string(),
        (Modal::Issue(f), PickKind::Project) => f.v.project = value.to_string(),
        (Modal::Issue(f), PickKind::Goal) => f.v.goal = value.to_string(),
        (Modal::Alerts, _) => {}
    }
    cx.notify();
}

/// A label above a control.
fn labeled(t: &Theme, label: &str, control: impl IntoElement) -> Div {
    kit::field(t, label, control, None)
}

fn with_modal<F>(f: F) -> impl Fn(&mut MainWindow, &ClickEvent, &mut Window, &mut Context<MainWindow>) + 'static
where
    F: Fn(&mut Modal) + 'static,
{
    move |m, _, _, cx| {
        if let Some(x) = m.modal.as_mut() {
            f(x);
        }
        cx.notify();
    }
}

fn clone_input(i: &Input) -> Input {
    Input { field: i.field.clone(), focus: i.focus.clone() }
}

// ------------------------------------------------------------------ New task

fn render_task(m: &mut MainWindow, t: &Theme, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let Some(Modal::Task(f)) = &m.modal else { return div().into_any_element() };
    let (title_in, detail, jira_key) = (clone_input(&f.title), clone_input(&f.detail), clone_input(&f.jira_key));
    let (v, open, err, busy) = (f.vals(cx), f.picker.open, f.err.clone(), f.busy);
    let view = task_view(&v, &Board::of(m), err.as_deref(), busy);

    let mut body = div().flex().flex_col().gap(px(16.));
    body = body.child(labeled(t, "Title", title_in.render(t, "nt-title", window)));
    body = body.child(kit::field(t, "What to do", detail.render(t, "nt-detail", window).min_h(px(96.)).items_start(), Some(HELP_DETAIL)));

    let proj_btn = pick_button(t, "nt-project", &view.project_label, v.project.is_empty(), open == Some(PickKind::Project))
        .on_click(cx.listener(|m, _, window, cx| open_picker(m, PickKind::Project, window, cx)));
    let goal_empty = !v.goal.is_empty() && view.goal_label == "Choose a goal";
    let goal_btn = pick_button(t, "nt-goal", &view.goal_label, goal_empty, open == Some(PickKind::Goal))
        .on_click(cx.listener(|m, _, window, cx| open_picker(m, PickKind::Goal, window, cx)));
    body = body.child(
        div()
            .flex()
            .gap(px(12.))
            .child(div().flex_1().min_w_0().child(labeled(t, "Project", proj_btn)))
            .child(div().flex_1().min_w_0().child(labeled(t, "Goal", goal_btn))),
    );
    if let Some(kind) = open {
        body = body.child(picker_list(m, t, kind, window, cx));
    }
    if let Some(on) = view.planned {
        body = body.child(check(t, "nt-planned", on, CHECK_PLANNED).on_click(cx.listener(with_modal(|x| {
            if let Modal::Task(f) = x {
                f.v.planned = !f.v.planned;
            }
        }))));
    }
    body = body.child(labeled(
        t,
        "Priority",
        seg_choices(t, "nt-priority", &view.priority, cx, |x, v| {
            if let Modal::Task(f) = x {
                f.v.priority = v.into();
            }
        }),
    ));
    match &view.start {
        None => body = body.child(kit::help(t, view.help)),
        Some(start) => {
            let mut opts = div().flex().flex_col().gap(px(2.));
            for c in start {
                let id = c.id;
                let mut row = radio(t, &format!("nt-pickup-{id}"), c.pressed, c.label);
                row = if c.disabled {
                    row.opacity(0.45).cursor_default().tooltip(kit::tip("No idle terminal in this project"))
                } else {
                    row.on_click(cx.listener(move |m, _, _, cx| {
                        if let Some(Modal::Task(f)) = m.modal.as_mut() {
                            f.v.pickup = id.into();
                            f.err = None;
                        }
                        cx.notify();
                    }))
                };
                opts = opts.child(row);
            }
            let mut start_col = div().flex().flex_col().gap(px(8.)).child(opts);
            if !view.terminals.is_empty() {
                let mut list = div().flex().flex_col().gap(px(2.)).p(px(4.)).rounded(px(8.)).border_1().border_color(t.border);
                let chosen = view.terminals.iter().skip(1).any(|(id, _)| *id == v.session);
                for (n, (id, name)) in view.terminals.iter().enumerate() {
                    let sel = if id.is_empty() { !chosen } else { *id == v.session };
                    let id = id.clone();
                    let mut row = kit::menu_item(t, SharedString::from(format!("nt-term-{n}")), name.clone(), sel);
                    if n == 0 {
                        row = row.text_color(t.muted);
                    }
                    list = list.child(row.on_click(cx.listener(with_modal(move |x| {
                        if let Modal::Task(f) = x {
                            f.v.session = id.clone();
                        }
                    }))));
                }
                start_col = start_col.child(list);
            }
            start_col = start_col.child(kit::help(t, view.help));
            body = body.child(labeled(t, "Start", start_col));
        }
    }
    if let Some((modes, key_field)) = &view.jira {
        let mut j = div().flex().flex_col().gap(px(8.)).child(seg_choices(t, "nt-jira", modes, cx, |x, v| {
            if let Modal::Task(f) = x {
                f.v.jira_mode = v.into();
            }
        }));
        if *key_field {
            j = j.child(jira_key.render(t, "nt-jira", window).font_family(t.mono_font.clone()));
        }
        body = body.child(labeled(t, "Jira ticket", j));
    }
    body = body.child(check(t, "nt-auto-close", view.auto_close, CHECK_TASK_CLOSE).on_click(cx.listener(with_modal(|x| {
        if let Modal::Task(f) = x {
            f.v.auto_close = !f.v.auto_close;
        }
    }))));
    form_shell(t, "New task", view.draft, body, view.err, busy, view.submit, submit_task, cx)
}

fn start_busy(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    match m.modal.as_mut() {
        Some(Modal::Task(f)) => (f.busy, f.err) = (true, None),
        Some(Modal::Goal(f)) => (f.busy, f.err) = (true, None),
        Some(Modal::Issue(f)) => (f.busy, f.err) = (true, None),
        _ => {}
    }
    cx.notify();
}

fn show_err(m: &mut MainWindow, e: String, cx: &mut Context<MainWindow>) {
    if let Some(x) = m.modal.as_mut() {
        x.set_err(Some(e));
    }
    cx.notify();
}

fn fail(m: &mut MainWindow, e: crate::backend::CallError, cx: &mut Context<MainWindow>) {
    show_err(m, e.message, cx);
}

fn submit_task(m: &mut MainWindow, _window: &mut Window, cx: &mut Context<MainWindow>) {
    let Some(Modal::Task(f)) = &m.modal else { return };
    if f.busy {
        return;
    }
    let v = f.vals(cx);
    match task_request(&v, &Board::of(m)) {
        Err(e) => show_err(m, e, cx),
        Ok((path, body)) => {
            start_busy(m, cx);
            m.post_or(
                path,
                body,
                cx,
                |m, v, cx| {
                    crate::prefs::remove(DRAFT_TASK);
                    m.modal = None;
                    let r = web_ref(&v, "T");
                    m.toast(if r.is_empty() { "Task added".into() } else { format!("Added {r}") }, false, cx);
                    if !r.is_empty() {
                        m.open_task(r, cx);
                    }
                },
                fail,
            );
        }
    }
}

// ------------------------------------------------------------------ New / Edit goal

fn render_goal(m: &mut MainWindow, t: &Theme, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let Some(Modal::Goal(f)) = &m.modal else { return div().into_any_element() };
    let (name, tldr, outcome, epic_key) = (clone_input(&f.name), clone_input(&f.tldr), clone_input(&f.outcome), clone_input(&f.epic_key));
    let (v, open, err, busy) = (f.vals(cx), f.picker.open, f.err.clone(), f.busy);
    let view = goal_view(&v, &Board::of(m), err.as_deref(), busy);

    let mut body = div().flex().flex_col().gap(px(16.));
    body = body.child(labeled(t, "Name", name.render(t, "ng-name", window)));
    body = body.child(kit::field(t, "TLDR", tldr.render(t, "ng-tldr", window).min_h(px(64.)).items_start(), Some(HELP_TLDR)));
    body = body.child(kit::field(t, "Done when", outcome.render(t, "ng-outcome", window).min_h(px(64.)).items_start(), Some(HELP_OUTCOME)));
    let proj_btn = pick_button(t, "ng-project", &view.project_label, v.project.is_empty(), open == Some(PickKind::Project))
        .on_click(cx.listener(|m, _, window, cx| open_picker(m, PickKind::Project, window, cx)));
    body = body.child(labeled(t, "Project", proj_btn));
    if open == Some(PickKind::Project) {
        body = body.child(picker_list(m, t, PickKind::Project, window, cx));
    }
    if let Some((_, help)) = view.epic_field {
        body = body.child(kit::field(t, "Jira epic", epic_key.render(t, "ng-epic", window).font_family(t.mono_font.clone()), Some(help)));
    }
    if let Some((modes, key_field, help)) = &view.epic_choices {
        let mut e = div().flex().flex_col().gap(px(8.)).child(seg_choices(t, "ng-epic-mode", modes, cx, |x, v| {
            if let Modal::Goal(f) = x {
                f.v.epic_mode = v.into();
            }
        }));
        if *key_field {
            e = e.child(epic_key.render(t, "ng-epic", window).font_family(t.mono_font.clone()));
        }
        e = e.child(kit::help(t, *help));
        body = body.child(labeled(t, "Jira epic", e));
    }
    let max = view.max_terminals;
    let step = |id: &'static str, glyph: &'static str, by: i64, enabled: bool, cx: &mut Context<MainWindow>| {
        let bt = kit::btn_small(t, id, glyph).w(px(26.)).px(px(0.));
        if enabled {
            bt.on_click(cx.listener(with_modal(move |x| {
                if let Modal::Goal(f) = x {
                    // The web's choices are 1 to 5 terminals.
                    f.v.max_terminals = (f.v.max_terminals + by).clamp(1, 5);
                }
            })))
        } else {
            kit::disabled(bt)
        }
    };
    let runs = div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(14.))
        .rounded(px(10.))
        .border_1()
        .border_color(t.border)
        .bg(t.tint)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(13.5))
                .child("At most")
                .child(step("ng-max-dec", "−", -1, max > 1, cx))
                .child(div().min_w(px(84.)).flex().justify_center().font_weight(FontWeight::SEMIBOLD).child(fmt::plural(max, "terminal", "terminals")))
                .child(step("ng-max-inc", "+", 1, max < 5, cx))
                .child(div().text_size(px(13.)).text_color(t.muted).child("working on this goal at once")),
        )
        .child(check(t, "ng-order", view.run_in_order, CHECK_ORDER).on_click(cx.listener(with_modal(|x| {
            if let Modal::Goal(f) = x {
                f.v.run_in_order = !f.v.run_in_order;
            }
        }))))
        .child(check(t, "ng-auto-close", view.auto_close, CHECK_GOAL_CLOSE).on_click(cx.listener(with_modal(|x| {
            if let Modal::Goal(f) = x {
                f.v.auto_close = !f.v.auto_close;
            }
        }))));
    body = body.child(runs);
    form_shell(t, view.title, view.draft, body, view.err, busy, view.submit, submit_goal, cx)
}

fn submit_goal(m: &mut MainWindow, _window: &mut Window, cx: &mut Context<MainWindow>) {
    let Some(Modal::Goal(f)) = &m.modal else { return };
    if f.busy {
        return;
    }
    let v = f.vals(cx);
    match goal_request(&v, &Board::of(m)) {
        Err(e) => show_err(m, e, cx),
        Ok((path, body)) => {
            start_busy(m, cx);
            if v.id.is_some() {
                m.post_or(
                    path,
                    body,
                    cx,
                    |m, _, cx| {
                        m.modal = None;
                        m.toast("Goal saved", false, cx);
                    },
                    fail,
                );
            } else {
                m.post_or(
                    path,
                    body,
                    cx,
                    |m, v, cx| {
                        crate::prefs::remove(DRAFT_GOAL);
                        m.modal = None;
                        let g = v.get("goal").filter(|g| g.is_object()).unwrap_or(&v);
                        let r = web_ref(g, "G");
                        m.toast(if r.is_empty() { "Goal added".into() } else { format!("Added {r}") }, false, cx);
                        if !r.is_empty() {
                            m.go(Page::Goal(r), cx);
                        }
                    },
                    fail,
                );
            }
        }
    }
}

// ------------------------------------------------------------------ Add an issue

fn render_issue(m: &mut MainWindow, t: &Theme, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let Some(Modal::Issue(f)) = &m.modal else { return div().into_any_element() };
    let (title, said, detail) = (clone_input(&f.title), clone_input(&f.said), clone_input(&f.detail));
    let (v, open, err, busy) = (f.vals(cx), f.picker.open, f.err.clone(), f.busy);
    let view = issue_view(&v, &Board::of(m), err.as_deref(), busy);

    let mut body = div().flex().flex_col().gap(px(16.));
    body = body.child(labeled(t, "Title", title.render(t, "ni-title", window)));
    body = body.child(labeled(
        t,
        "Type",
        seg_choices(t, "ni-kind", &view.kinds, cx, |x, v| {
            if let Modal::Issue(f) = x {
                f.v.kind = v.into();
            }
        }),
    ));
    let goal_empty = !v.goal.is_empty() && view.goal_label == "Choose a goal";
    let goal_btn = pick_button(t, "ni-goal", &view.goal_label, goal_empty, open == Some(PickKind::Goal))
        .on_click(cx.listener(|m, _, window, cx| open_picker(m, PickKind::Goal, window, cx)));
    let proj_btn = pick_button(t, "ni-project", &view.project_label, v.project.is_empty(), open == Some(PickKind::Project))
        .on_click(cx.listener(|m, _, window, cx| open_picker(m, PickKind::Project, window, cx)));
    body = body.child(
        div()
            .flex()
            .gap(px(12.))
            .child(div().flex_1().min_w_0().child(labeled(t, "Goal", goal_btn)))
            .child(div().flex_1().min_w_0().child(labeled(t, "Project", proj_btn))),
    );
    if let Some(k) = open {
        body = body.child(picker_list(m, t, k, window, cx));
    }
    body = body.child(labeled(t, "What you saw", said.render(t, "ni-said", window).min_h(px(64.)).items_start()));
    body = body.child(kit::field(t, "More detail (optional)", detail.render(t, "ni-detail", window).min_h(px(64.)).items_start(), Some(HELP_ISSUE)));
    form_shell(t, "Add an issue", None, body, view.err, busy, view.submit, submit_issue, cx)
}

fn submit_issue(m: &mut MainWindow, _window: &mut Window, cx: &mut Context<MainWindow>) {
    let Some(Modal::Issue(f)) = &m.modal else { return };
    if f.busy {
        return;
    }
    match issue_request(&f.vals(cx)) {
        Err(e) => show_err(m, e, cx),
        Ok((path, body)) => {
            start_busy(m, cx);
            m.post_or(
                path,
                body,
                cx,
                |m, v, cx| {
                    m.modal = None;
                    let x = v.get("issue").filter(|x| x.is_object()).unwrap_or(&v);
                    let r = web_ref(x, "B");
                    m.toast(if r.is_empty() { "Issue added".into() } else { format!("Added {r}") }, false, cx);
                    // On the board the new issue just shows up in the Backlog column.
                    if !r.is_empty() && m.page != Page::Board {
                        m.open_issue(r, cx);
                    }
                },
                fail,
            );
        }
    }
}

#[cfg(test)]
mod tests;
