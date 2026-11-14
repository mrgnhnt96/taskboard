//! What Needs you shows, worked out from its items: the four groups, one row per goal in each, what
//! each row finishes with, what its Later and More menus offer, and the Later line.
use super::{Act, Ask, Sev};
use crate::fmt::{self, arr, b, opt_s, s};
use chrono::{Datelike, Days, Months, NaiveDate};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Waiting,
    Decide,
    ToDos,
    Stopped,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Waiting => "Agents waiting",
            Kind::Decide => "Decide",
            Kind::ToDos => "Your to-dos",
            Kind::Stopped => "Stopped",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Kind::Waiting => "waiting",
            Kind::Decide => "decide",
            Kind::ToDos => "todos",
            Kind::Stopped => "stopped",
        }
    }
}

/// The group item `a` belongs in.
pub fn kind_of(a: &Ask) -> Kind {
    if a.waiting {
        return Kind::Waiting;
    }
    if a.human || a.kind == "Weekly review" {
        return Kind::ToDos;
    }
    // A task that needs you with no agent behind it has stopped: its conversation didn't compact, say.
    let stopped = matches!(a.kind.as_str(), "Stopped" | "Terminal lost" | "Couldn’t start" | "Failed" | "Looks finished");
    if stopped || a.answer { Kind::Stopped } else { Kind::Decide }
}

/// What a row waits to release.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Holds {
    pub tasks: Vec<String>,
    /// The goal stays open on it: a missed target does.
    pub goal: bool,
}

impl Holds {
    pub fn of(v: &Value, miss: bool) -> Holds {
        let tasks = arr(v, "tasks").iter().filter_map(Value::as_str).map(str::to_string).collect();
        Holds { tasks, goal: miss && opt_s(v, "goal").is_some() }
    }

    /// "Holds 2 tasks", "Holds the goal", or nothing.
    pub fn label(&self) -> String {
        match self.tasks.len() {
            0 if self.goal => "Holds the goal".into(),
            0 => String::new(),
            n => format!("Holds {}", fmt::plural(n as i64, "task", "tasks")),
        }
    }
}

/// A goal's items in one group: a row that opens in place, the goal named above them.
#[derive(Clone, Debug, PartialEq)]
pub struct GoalRow {
    pub key: String,
    /// (ref, name)
    pub goal: (String, String),
    pub title: String,
    /// "3 missed"
    pub tag: String,
    pub kids: Vec<Ask>,
    /// The goal's terminals you closed: Restart starts them all.
    pub finish: Option<Act>,
    pub holds: Holds,
    pub time: String,
    pub quiet: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    One(Ask),
    Goal(GoalRow),
}

impl Row {
    pub fn key(&self) -> String {
        match self {
            Row::One(a) => a.key(),
            Row::Goal(g) => g.key.clone(),
        }
    }

    /// What Later puts off: the row's item, or each of the goal's.
    pub fn later_ids(&self) -> Vec<String> {
        match self {
            Row::One(a) => a.snooze.then(|| a.id.clone()).into_iter().collect(),
            Row::Goal(g) => g.kids.iter().filter(|a| a.snooze).map(|a| a.id.clone()).collect(),
        }
    }

    pub fn goal_ref(&self) -> Option<&str> {
        match self {
            Row::One(a) => a.goal.as_ref().map(|(g, _)| g.as_str()),
            Row::Goal(g) => Some(&g.goal.0),
        }
    }

    pub fn more(&self, jira: bool) -> Vec<Entry> {
        match self {
            Row::One(a) => more_vm(a, jira),
            Row::Goal(g) => goal_entries(&g.goal.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub kind: Kind,
    /// Items, not rows: a goal's row counts each thing in it.
    pub count: usize,
    pub rows: Vec<Row>,
}

/// One closed task of a goal's Stopped line, as an item of its own.
fn closed_kid(a: &Ask, r: &str, title: &str) -> Ask {
    Ask {
        id: format!("{}{r}", super::TASK_ID),
        title: title.to_string(),
        text: String::new(),
        task: Some((r.to_string(), title.to_string())),
        act: Some(Act::Restart(r.to_string())),
        also: Some(Act::MarkDone(r.to_string())),
        snooze: true,
        closed: Vec::new(),
        holds: Holds::default(),
        ..a.clone()
    }
}

fn is_miss(a: &Ask) -> bool {
    a.issue.is_some()
}

fn goal_tag(kind: Kind, kids: &[Ask], closed: bool) -> String {
    let n = kids.len();
    match kind {
        _ if kids.iter().all(is_miss) => format!("{n} missed"),
        Kind::Stopped if closed => String::new(),
        Kind::Stopped => format!("{n} stopped"),
        Kind::Waiting => format!("{n} waiting"),
        Kind::Decide => format!("{n} to decide"),
        Kind::ToDos => format!("{n} to do"),
    }
}

fn goal_row(kind: Kind, goal: (String, String), kids: Vec<Ask>, restart: Option<&Ask>) -> GoalRow {
    let mut holds = Holds::default();
    for x in kids.iter().flat_map(|a| &a.holds.tasks).chain(restart.iter().flat_map(|a| &a.holds.tasks)) {
        if !holds.tasks.contains(x) {
            holds.tasks.push(x.clone());
        }
    }
    holds.goal = kids.iter().any(|a| a.holds.goal);
    let all_closed = restart.is_some_and(|a| a.closed.len() == kids.len());
    let title = match restart {
        Some(a) if all_closed => a.title.clone(),
        _ => goal.1.clone(),
    };
    let oldest = kids.iter().filter(|a| !a.at.is_empty()).min_by(|a, b| a.at.cmp(&b.at));
    GoalRow {
        key: format!("goal:{}:{}", goal.0, kind.slug()),
        tag: goal_tag(kind, &kids, all_closed),
        title,
        finish: restart.and_then(|a| a.act.clone()),
        holds,
        time: oldest.map(|a| a.time.clone()).unwrap_or_default(),
        quiet: kids.iter().all(|a| a.sev == Sev::Quiet),
        goal,
        kids,
    }
}

fn rows_of(kind: Kind, items: Vec<Ask>, restarts: &[&Ask]) -> Vec<Row> {
    let mut by_goal: Vec<((String, String), Vec<Ask>)> = Vec::new();
    let mut order: Vec<Result<usize, Ask>> = Vec::new();
    for a in items {
        match a.goal.clone() {
            Some(g) => match by_goal.iter().position(|(x, _)| x.0 == g.0) {
                Some(n) => by_goal[n].1.push(a),
                None => {
                    order.push(Ok(by_goal.len()));
                    by_goal.push((g, vec![a]));
                }
            },
            None => order.push(Err(a)),
        }
    }
    let mut goals: Vec<Option<((String, String), Vec<Ask>)>> = by_goal.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|o| match o {
            Err(a) => Some(Row::One(a)),
            Ok(n) => {
                let (goal, mut kids) = goals[n].take()?;
                let restart = restarts.iter().find(|a| a.goal.as_ref().is_some_and(|g| g.0 == goal.0)).copied();
                if kids.len() == 1 && restart.is_none() {
                    return kids.pop().map(Row::One);
                }
                Some(Row::Goal(goal_row(kind, goal, kids, restart)))
            }
        })
        .collect()
}

/// The groups that hold something, in the page's order. Each keeps the items' order, the worst then
/// the newest; a goal's row sits where its first item would.
pub fn groups_vm(asks: &[&Ask]) -> Vec<Group> {
    [Kind::Waiting, Kind::Decide, Kind::ToDos, Kind::Stopped]
        .into_iter()
        .filter_map(|kind| {
            let mine: Vec<&Ask> = asks.iter().copied().filter(|a| kind_of(a) == kind).collect();
            let restarts: Vec<&Ask> = mine.iter().copied().filter(|a| !a.closed.is_empty()).collect();
            let mut items = Vec::new();
            for a in &mine {
                if a.closed.is_empty() {
                    items.push((*a).clone());
                } else {
                    items.extend(a.closed.iter().map(|(r, title)| closed_kid(a, r, title)));
                }
            }
            let count = items.len();
            (count > 0).then(|| Group { kind, count, rows: rows_of(kind, items, &restarts) })
        })
        .collect()
}

/// The small word on a row, only where it says something the group doesn't.
pub fn tag_of(a: &Ask, nested: bool) -> Option<String> {
    match a.kind.as_str() {
        "Done-when missed" if nested => None,
        "Done-when missed" => Some("1 missed".into()),
        "Gone quiet" | "Failed" | "Terminal lost" | "Couldn’t start" | "Checks failed" | "Review the PR" | "Ready to merge" | "Urgent" | "Looks finished" | "Holds a PR" => Some(a.kind.clone()),
        _ => None,
    }
}

/// A human task's note, which an open row asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    Done,
    CantDo,
    NotNeeded,
    WaitingOn,
}

impl Note {
    /// The note box's name: what it holds.
    pub fn name(self) -> &'static str {
        match self {
            Note::Done => "Note for the agents",
            Note::CantDo => "Why you can't do this",
            Note::NotNeeded => "Why it isn't needed",
            Note::WaitingOn => "Who you're waiting on",
        }
    }

    /// The toast once it's sent.
    pub fn said(self) -> &'static str {
        match self {
            Note::Done => "Marked done",
            Note::CantDo => "Told the agents you can't do this",
            Note::NotNeeded => "Marked not needed",
            Note::WaitingOn => "It comes back on that day",
        }
    }
}

/// The post that sends a human task's note: its path and body. None while Waiting on someone has no date.
pub fn note_post(r: &str, kind: Note, note: &str, until: Option<&str>) -> Option<(String, Value)> {
    let note = note.trim();
    let answer = |answer: &str| (format!("tasks/{r}/owner-answer"), json!({"answer": answer, "note": note}));
    match kind {
        Note::Done => Some((format!("tasks/{r}/did"), json!({"note": note}))),
        Note::CantDo => Some(answer("cant_do")),
        Note::NotNeeded => Some(answer("not_needed")),
        Note::WaitingOn => until.map(|u| (format!("tasks/{r}/owner-answer"), json!({"answer": "waiting", "note": note, "until": u}))),
    }
}

/// What a row's button or menu item does.
#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    Act(Act),
    /// A human task's Done: at once on a shut row, with the note on an open one.
    Done(String),
    /// Open the row's note field for this answer.
    Ask(Note),
    DontWait(String),
    TalkTask(String),
    TalkAlert(String),
    MakeTask(String),
    WontDo(String),
    Ticket(String),
    FinishWithout(String),
    Dismiss(String),
    PauseGoal(String),
    SetAside(String),
    /// The dialogs a task's page has for these.
    ConfirmDone(String),
    ConfirmCancel(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Item(&'static str, Do),
    Line,
}

fn task_ref(a: &Ask) -> Option<String> {
    a.task.as_ref().map(|(r, _)| r.clone())
}

/// A plain alert has nothing of its own to press: talking it over is how it's settled.
fn talks_first(a: &Ask) -> bool {
    a.is_alert() && !a.answer && (a.kind == "Needs you" || a.act.is_none())
}

/// The buttons on the row itself.
pub fn finish_vm(a: &Ask) -> Vec<(String, Do)> {
    if let (true, Some(r)) = (a.human, task_ref(a)) {
        return vec![("Done".into(), Do::Done(r))];
    }
    if let Some(i) = &a.issue {
        return vec![("Make it a task".into(), Do::MakeTask(i.clone())), ("Won’t do".into(), Do::WontDo(i.clone()))];
    }
    if talks_first(a) {
        return vec![("Talk it over".into(), Do::TalkAlert(a.id.clone()))];
    }
    let shipped = a.also.iter().filter(|_| a.kind == "Looks finished");
    shipped.chain(a.act.iter()).map(|act| (act.label().to_string(), Do::Act(act.clone()))).collect()
}

/// Pause and Set aside, which anything in a goal offers.
fn goal_entries(g: &str) -> Vec<Entry> {
    vec![Entry::Item("Pause goal", Do::PauseGoal(g.to_string())), Entry::Item("Set goal aside", Do::SetAside(g.to_string()))]
}

/// The row's More menu.
pub fn more_vm(a: &Ask, jira: bool) -> Vec<Entry> {
    let mut top = Vec::new();
    let mut rest = Vec::new();
    let r = task_ref(a);
    if let (true, Some(r)) = (a.human, r.clone()) {
        top.push(Entry::Item("Can't do this", Do::Ask(Note::CantDo)));
        top.push(Entry::Item("Waiting on someone", Do::Ask(Note::WaitingOn)));
        top.push(Entry::Item("Don't wait for me", Do::DontWait(r.clone())));
        top.push(Entry::Item("Not needed", Do::Ask(Note::NotNeeded)));
        rest.push(Entry::Item("Talk it over", Do::TalkTask(r)));
    } else if let Some(i) = &a.issue {
        if jira {
            top.push(Entry::Item("Create ticket", Do::Ticket(i.clone())));
        }
        top.push(Entry::Item("Finish the goal without it", Do::FinishWithout(i.clone())));
    } else {
        let can_finish = kind_of(a) == Kind::Stopped && a.kind != "Failed";
        if let Some(r) = r.clone().filter(|_| can_finish) {
            match &a.also {
                _ if a.kind == "Looks finished" => {}
                Some(act) => top.push(Entry::Item("Mark done", Do::Act(act.clone()))),
                None => top.push(Entry::Item("Mark done", Do::ConfirmDone(r.clone()))),
            }
            top.push(Entry::Item("Cancel task", Do::ConfirmCancel(r)));
        }
        if let Some(r) = r.filter(|_| matches!(a.act, Some(Act::Focus(_) | Act::AskAgain(_) | Act::OpenPr(_)))) {
            top.push(Entry::Item("Open task", Do::Act(Act::OpenTask(r))));
        }
        if a.is_alert() && !a.answer {
            if !talks_first(a) {
                rest.push(Entry::Item("Talk it over", Do::TalkAlert(a.id.clone())));
            }
            if a.dismiss {
                rest.push(Entry::Item("Dismiss", Do::Dismiss(a.id.clone())));
            }
        }
    }
    if let Some((g, _)) = &a.goal {
        rest.extend(goal_entries(g));
    }
    if !top.is_empty() && !rest.is_empty() {
        top.push(Entry::Line);
    }
    top.extend(rest);
    top
}

/// The Later menu above its line: (label, `POST /alerts/:id/snooze` body).
pub fn later_choices() -> [(&'static str, Value); 4] {
    [
        ("1 hour", json!({"mins": 60})),
        ("3 hours", json!({"mins": 180})),
        ("Tomorrow", json!({"until": "tomorrow"})),
        ("Until I’m back", json!({"until": "back"})),
    ]
}

/// The goals After a goal offers: those going, less the row's own, which it may hold open. (ref, name)
pub fn after_goals(home: &Value, own: Option<&str>) -> Vec<(String, String)> {
    arr(home, "goals")
        .iter()
        .filter(|e| e["goal"].is_object() && !b(&e["goal"], "paused"))
        .map(|e| (fmt::ref_of(&e["goal"], "G"), s(&e["goal"], "name").to_string()))
        .filter(|(g, _)| Some(g.as_str()) != own)
        .collect()
}

/// The toast once `a`, the alert a snooze answers with, is off the list.
pub fn later_note(a: &Value) -> String {
    match (opt_s(a, "snoozed_until"), a["snoozed_goal"].as_i64()) {
        (_, Some(g)) => format!("Back when G{g} finishes"),
        (Some(at), _) if fmt::hhmm(at).contains(':') => format!("Back at {}", fmt::hhmm(at)),
        (Some(at), _) => format!("Back {}", fmt::hhmm(at)),
        _ => "Back when you are".into(),
    }
}

/// Waiting on someone's dates from `today`: (label, day).
pub fn wait_dates(today: NaiveDate) -> Vec<(&'static str, String)> {
    let monday = today + Days::new(7 - u64::from(today.weekday().num_days_from_monday()));
    let month = today.checked_add_months(Months::new(1)).unwrap_or(today + Days::new(30));
    [("Tomorrow", today + Days::new(1)), ("Next week", monday), ("2 weeks", today + Days::new(14)), ("1 month", month)]
        .into_iter()
        .map(|(l, d)| (l, d.format("%Y-%m-%d").to_string()))
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct LaterItem {
    pub key: String,
    pub title: String,
    /// "Back Nov 12", "Waiting on Sam · back Nov 12".
    pub when: String,
    pub open: Option<Act>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LaterLine {
    /// "Later 2 · back Nov 12"
    pub text: String,
    pub items: Vec<LaterItem>,
    /// Put-off agents, which still count as waiting.
    pub waiting: usize,
}

fn back_at(iso: &str) -> String {
    fmt::hhmm(iso)
}

fn later_when(l: &Value) -> String {
    let back = match (opt_s(l, "until"), opt_s(l, "after_goal")) {
        (_, Some(g)) => format!("after {g}"),
        (Some(at), _) => format!("back {}", back_at(at)),
        _ if b(l, "back") => "when you’re back".to_string(),
        _ => String::new(),
    };
    let sentence = |x: &str| {
        let mut c = x.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
    };
    match opt_s(l, "waiting_on").filter(|w| !w.is_empty()) {
        Some(w) if back.is_empty() => format!("Waiting on {w}"),
        Some(w) => format!("Waiting on {w} · {back}"),
        None => sentence(&back),
    }
}

fn later_item(i: &Value) -> LaterItem {
    let when = later_when(&i["later"]);
    if b(i, "stage") {
        let goal = s(i, "goal").to_string();
        let title = format!("{} · wave {} finished", s(i, "goal_name"), fmt::i(i, "wave"));
        return LaterItem { key: s(i, "id").to_string(), title, when, open: Some(Act::OpenGoal(goal)) };
    }
    let a = super::ask_vm(i);
    let open = a.task.as_ref().map(|(r, _)| Act::OpenTask(r.clone())).or(a.act.clone());
    LaterItem { key: a.key(), title: a.title, when, open }
}

fn in_project(i: &Value, project: &str) -> bool {
    let of = opt_s(&i["card"], "project").or(opt_s(i, "project")).unwrap_or("");
    project == "all" || of.is_empty() || of == project
}

/// The quiet line of what's put off, and what's on it. None with nothing put off.
pub fn later_vm(home: &Value, project: &str) -> Option<LaterLine> {
    let shown: Vec<&Value> = arr(home, "later").iter().filter(|i| in_project(i, project)).collect();
    if shown.is_empty() {
        return None;
    }
    let next = shown.iter().filter_map(|i| opt_s(&i["later"], "until")).min();
    let waiting = shown.iter().filter(|i| b(i, "waiting")).count();
    let mut text = format!("Later {}", shown.len());
    if let Some(at) = next {
        text.push_str(&format!(" · back {}", back_at(at)));
    }
    if waiting > 0 {
        text.push_str(&format!(" · {} waiting", fmt::plural(waiting as i64, "agent", "agents")));
    }
    Some(LaterLine { text, items: shown.iter().map(|i| later_item(i)).collect(), waiting })
}

/// The header's line, which says so when no agent waits.
pub fn waiting_line(n: usize) -> String {
    match n {
        0 => "No agents waiting".into(),
        n => format!("{} waiting", fmt::plural(n as i64, "agent", "agents")),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ask_vm, asks_vm};
    use super::*;
    // Beats the glob's `gpui_kit::test`.
    use ::core::prelude::v1::test;

    fn groups(needs: Value) -> Vec<Group> {
        let asks = asks_vm(&json!({"needs": needs, "goals": []}));
        groups_vm(&asks.iter().collect::<Vec<_>>())
    }

    fn miss(n: i64, goal: &str) -> Value {
        json!({"id": format!("a{n}"), "key": format!("miss:{n}"), "text": format!("Miss {n}"), "title": format!("Miss {n}"), "goal": goal, "goal_name": "Picto", "project": "p",
               "at": format!("2026-11-0{n}T10:00:00Z"), "card": null, "waiting": false, "holds": {"tasks": [], "goal": goal}})
    }

    fn todo(r: &str, goal: Option<&str>, holds: Value) -> Value {
        let card = json!({"ref": r, "title": "Set up the skill", "project": "p", "status": "needs", "needs_reason": "human", "kind": "human"});
        json!({"id": format!("task:{r}"), "task": r, "goal": goal, "goal_name": goal.map(|_| "Picto"), "card": card, "text": "", "at": "2026-11-05T10:00:00Z",
               "waiting": false, "holds": {"tasks": holds, "goal": goal}})
    }

    fn agent(r: &str, goal: &str) -> Value {
        let card = json!({"ref": r, "title": "Asks", "project": "p", "status": "needs", "needs_reason": "question", "question": "Which?", "terminal_open": true});
        json!({"id": format!("task:{r}"), "task": r, "goal": goal, "goal_name": "Picto", "card": card, "text": "Which?", "at": "2026-11-05T10:00:00Z", "waiting": true, "holds": {"tasks": [], "goal": goal}})
    }

    fn labels(v: &[Entry]) -> Vec<&'static str> {
        v.iter().map(|e| if let Entry::Item(l, _) = e { *l } else { "—" }).collect()
    }

    #[test]
    fn items_sort_into_the_four_groups_in_order() {
        let stopped = json!({"id": "stopped:G2", "goal": "G2", "goal_name": "Web", "title": "Web · 1 closed", "text": "T9 Review", "project": "p", "at": "2026-11-05T10:00:00Z",
                             "card": null, "waiting": false, "holds": {"tasks": [], "goal": "G2"}, "stopped": {"tasks": ["T9"]}});
        let quiet = json!({"id": "a7", "key": "quiet:T7", "task": "T7", "text": "No update", "at": "2026-11-05T10:00:00Z",
                           "card": {"ref": "T7", "title": "Quiet", "status": "working"}, "waiting": true, "holds": {"tasks": [], "goal": null}});
        let got = groups(json!([stopped, todo("H1", None, json!([])), miss(1, "G1"), quiet]));
        let kinds: Vec<Kind> = got.iter().map(|g| g.kind).collect();
        assert_eq!(kinds, vec![Kind::Waiting, Kind::Decide, Kind::ToDos, Kind::Stopped]);
        assert_eq!(got[0].rows.len(), 1, "gone quiet sits with the agents");
    }

    #[test]
    fn agents_waiting_is_gone_when_nothing_waits_and_the_header_says_so() {
        let got = groups(json!([miss(1, "G1")]));
        assert!(got.iter().all(|g| g.kind != Kind::Waiting));
        assert_eq!(waiting_line(0), "No agents waiting");
        assert_eq!(waiting_line(1), "1 agent waiting");
        assert_eq!(waiting_line(3), "3 agents waiting");
    }

    #[test]
    fn a_goals_items_fold_into_one_row_that_counts_each_item() {
        let got = groups(json!([miss(4, "G1"), miss(2, "G1"), miss(3, "G1"), miss(1, "G2")]));
        assert_eq!(got[0].count, 4, "the group counts items, not rows");
        assert_eq!(got[0].rows.len(), 2);
        let Row::Goal(g) = &got[0].rows[0] else { panic!("three misses fold") };
        assert_eq!((g.title.as_str(), g.tag.as_str(), g.kids.len()), ("Picto", "3 missed", 3));
        assert_eq!(g.time, super::super::short_ago("2026-11-02T10:00:00Z"), "it shows how long the oldest has waited");
        assert_eq!(g.holds.label(), "Holds the goal");
        assert!(matches!(got[0].rows[1], Row::One(_)), "one item stays a row of its own");
    }

    #[test]
    fn a_goal_row_is_put_off_as_a_whole() {
        let got = groups(json!([miss(1, "G1"), miss(2, "G1")]));
        let mut ids = got[0].rows[0].later_ids();
        ids.sort();
        assert_eq!(ids, vec!["a1", "a2"]);
        assert_eq!(labels(&got[0].rows[0].more(false)), vec!["Pause goal", "Set goal aside"]);
    }

    #[test]
    fn a_goals_closed_terminals_are_a_row_of_tasks_with_one_restart() {
        let stopped = json!({"id": "stopped:G2", "goal": "G2", "goal_name": "Web", "title": "Web · 2 closed", "text": "T8 One\nT9 Two title", "project": "p", "at": "2026-11-05T10:00:00Z",
                             "card": null, "waiting": false, "holds": {"tasks": ["T11"], "goal": "G2"}, "stopped": {"tasks": ["T8", "T9"]}});
        let got = groups(json!([stopped]));
        assert_eq!((got[0].kind, got[0].count), (Kind::Stopped, 2));
        let Row::Goal(g) = &got[0].rows[0] else { panic!("a goal row") };
        assert_eq!(g.title, "Web · 2 closed");
        assert_eq!(g.tag, "", "the title already says it");
        assert_eq!(g.finish.as_ref().map(Act::label), Some("Restart both"));
        assert_eq!(g.holds.label(), "Holds 1 task");
        assert!(g.quiet);
        assert_eq!(g.kids[1].title, "Two title");
        assert_eq!(g.kids[1].id, "task:T9", "Later puts the one task off");
        assert_eq!(finish_vm(&g.kids[1]), vec![("Restart".to_string(), Do::Act(Act::Restart("T9".into())))]);
        assert_eq!(labels(&more_vm(&g.kids[1], false)), vec!["Mark done", "Cancel task", "—", "Pause goal", "Set goal aside"]);
    }

    #[test]
    fn a_todo_offers_done_on_the_row_and_the_owners_answers_in_more() {
        let a = ask_vm(&todo("H1", Some("G1"), json!(["T4"])));
        assert_eq!(finish_vm(&a), vec![("Done".to_string(), Do::Done("H1".into()))]);
        assert_eq!(
            labels(&more_vm(&a, false)),
            vec!["Can't do this", "Waiting on someone", "Don't wait for me", "Not needed", "—", "Talk it over", "Pause goal", "Set goal aside"]
        );
        assert_eq!(a.holds.label(), "Holds 1 task");
        assert_eq!(labels(&more_vm(&ask_vm(&todo("H2", None, json!([]))), false)).last(), Some(&"Talk it over"), "no goal, no goal items");
    }

    #[test]
    fn a_missed_target_is_settled_from_its_row() {
        let a = ask_vm(&miss(3, "G1"));
        let labels_on_row: Vec<String> = finish_vm(&a).into_iter().map(|(l, _)| l).collect();
        assert_eq!(labels_on_row, vec!["Make it a task", "Won’t do"]);
        assert_eq!(labels(&more_vm(&a, true)), vec!["Create ticket", "Finish the goal without it", "—", "Pause goal", "Set goal aside"]);
        assert_eq!(labels(&more_vm(&a, false))[0], "Finish the goal without it", "no Jira, no ticket");
        assert_eq!(a.holds.label(), "Holds the goal");
        assert_eq!(tag_of(&a, false).as_deref(), Some("1 missed"));
        assert_eq!(tag_of(&a, true), None, "its goal's row counts it");
    }

    #[test]
    fn a_plain_alert_talks_it_over_and_more_dismisses_it() {
        let a = ask_vm(&json!({"id": "a1", "text": "Release 11.05.26 was dropped", "at": "2026-11-05T10:00:00Z", "card": null, "waiting": false, "holds": {"tasks": [], "goal": null}}));
        assert_eq!(finish_vm(&a), vec![("Talk it over".to_string(), Do::TalkAlert("a1".into()))]);
        assert_eq!(labels(&more_vm(&a, false)), vec!["Dismiss"]);
    }

    #[test]
    fn an_agent_question_can_not_be_dismissed_away() {
        let a = ask_vm(&agent("T5", "G1"));
        assert_eq!(kind_of(&a), Kind::Waiting);
        assert_eq!(finish_vm(&a), vec![("Open terminal".to_string(), Do::Act(Act::Focus("T5".into())))]);
        assert_eq!(labels(&more_vm(&a, false)), vec!["Open task", "—", "Pause goal", "Set goal aside"]);
    }

    #[test]
    fn a_pr_to_review_opens_its_pr_and_more_its_task() {
        let card = json!({"ref": "T6", "title": "Fix", "status": "done", "pr": {"num": 4, "url": "https://x/4", "state": "open"}});
        let a = ask_vm(&json!({"id": "a2", "text": "Review", "task": "T6", "review": true, "card": card, "at": "2026-11-05T10:00:00Z", "waiting": false, "holds": {"tasks": [], "goal": null}}));
        assert_eq!(kind_of(&a), Kind::Decide);
        assert_eq!(finish_vm(&a)[0].0, "Open PR");
        assert_eq!(labels(&more_vm(&a, false)), vec!["Open task", "—", "Talk it over"], "the review stays until it's done, so no Dismiss");
    }

    #[test]
    fn a_shipped_lost_task_finishes_with_mark_done() {
        let card = json!({"ref": "T5", "title": "Ship", "status": "needs", "lost": true, "looks_finished": [{"what": "Tag v1", "at": "2026-11-05T10:00:00Z"}]});
        let a = ask_vm(&json!({"id": "task:T5", "task": "T5", "text": "", "card": card, "at": "2026-11-05T10:00:00Z", "waiting": false, "holds": {"tasks": [], "goal": null}}));
        let on_row: Vec<String> = finish_vm(&a).into_iter().map(|(l, _)| l).collect();
        assert_eq!(on_row, vec!["Mark done", "Open task"]);
        assert_eq!(labels(&more_vm(&a, false)), vec!["Cancel task"]);
    }

    #[test]
    fn every_later_menu_has_the_designs_choices() {
        let l: Vec<&str> = later_choices().iter().map(|(l, _)| *l).collect();
        assert_eq!(l, vec!["1 hour", "3 hours", "Tomorrow", "Until I’m back"]);
    }

    #[test]
    fn after_a_goal_offers_the_goals_going_but_not_the_rows_own() {
        let home = json!({"goals": [
            {"goal": {"id": 1, "ref": "G1", "name": "One"}}, {"goal": {"id": 2, "ref": "G2", "name": "Two"}},
            {"goal": {"id": 3, "ref": "G3", "name": "Paused", "paused": true}}, {"goal": null}]});
        assert_eq!(after_goals(&home, Some("G1")), vec![("G2".to_string(), "Two".to_string())]);
    }

    #[test]
    fn a_note_goes_to_the_answer_it_was_written_for() {
        assert_eq!(note_post("H1", Note::Done, " Sent ", None), Some(("tasks/H1/did".to_string(), json!({"note": "Sent"}))));
        assert_eq!(note_post("H1", Note::CantDo, "No key", None), Some(("tasks/H1/owner-answer".to_string(), json!({"answer": "cant_do", "note": "No key"}))));
        assert_eq!(note_post("H1", Note::NotNeeded, "", None).unwrap().1["answer"], "not_needed");
        assert_eq!(note_post("H1", Note::WaitingOn, "Apple", None), None, "it needs a date");
        assert_eq!(note_post("H1", Note::WaitingOn, "Apple", Some("2026-11-12")).unwrap().1, json!({"answer": "waiting", "note": "Apple", "until": "2026-11-12"}));
    }

    #[test]
    fn waiting_dates_count_from_today() {
        let friday = NaiveDate::from_ymd_opt(2026, 11, 6).unwrap();
        let dates: Vec<String> = wait_dates(friday).into_iter().map(|(_, d)| d).collect();
        assert_eq!(dates, vec!["2026-11-07", "2026-11-09", "2026-11-20", "2026-12-06"]);
    }

    #[test]
    fn the_later_line_says_how_many_and_the_soonest_back() {
        assert_eq!(later_vm(&json!({"later": []}), "all"), None, "nothing put off, no line");
        let later = |until: &str, who: Value| json!({"until": until, "back": false, "after_goal": null, "waiting_on": who});
        let home = json!({"later": [
            {"id": "a1", "text": "One", "title": "One", "project": "p", "card": null, "waiting": false, "at": "2026-11-05T10:00:00Z", "holds": {"tasks": [], "goal": null}, "later": later("2099-11-20T15:00:00Z", Value::Null)},
            {"id": "a2", "text": "Two", "title": "Two", "project": "q", "card": null, "waiting": true, "at": "2026-11-05T10:00:00Z", "holds": {"tasks": [], "goal": null}, "later": later("2099-11-12T15:00:00Z", json!("Sam"))},
            {"id": "stage:G3:1", "text": "Wave 1", "goal": "G3", "goal_name": "Web", "stage": true, "wave": 1, "waiting": false, "holds": {"tasks": [], "goal": "G3"},
             "later": {"until": null, "back": true, "after_goal": null, "waiting_on": null}}]});
        let l = later_vm(&home, "all").unwrap();
        assert_eq!(l.text, "Later 3 · back Nov 12 · 1 agent waiting");
        assert_eq!(l.items[1].when, "Waiting on Sam · back Nov 12");
        assert_eq!(l.items[2].title, "Web · wave 1 finished");
        assert_eq!(l.items[2].when, "When you’re back");
        let q = later_vm(&home, "p").unwrap();
        assert_eq!(q.text, "Later 2 · back Nov 20", "only the project's, and what has no project");
    }

    #[test]
    fn later_says_when_it_comes_back() {
        assert_eq!(later_note(&json!({"snoozed_goal": 3})), "Back when G3 finishes");
        assert_eq!(later_note(&json!({"snoozed_until": "2099-11-20T15:00:00Z"})), "Back Nov 20");
        assert_eq!(later_note(&json!({"snoozed_back": "x"})), "Back when you are");
    }
}
