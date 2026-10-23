//! `tb needs`: the app's Needs you list in a terminal, so the owner can work through it with an agent.
//!
//! It reads the same `GET /home` the app's home page does (the alerts, each with its task's card, and
//! the goals held after a wave for review) and acts through the same endpoints its buttons post to:
//! an answer or a picker's choices, Restart, Mark done, Continue, Dismiss and Snooze. What only the
//! owner's own click may do (Start, Done on a human task) it points at instead: `tb start`, which
//! checks the owner's word, or the app.

use clap::Subcommand;
use serde_json::{json, Value};

#[derive(Subcommand, Debug, PartialEq)]
pub enum NeedsCmd {
    /// Everything that needs the owner, the worst first (the default)
    List {
        /// Only this project's
        #[arg(long)]
        project: Option<String>,
        /// The items as JSON
        #[arg(long)]
        json: bool,
    },
    /// One item in full: what it asks, its options and what can be done about it
    Show { item: String },
    /// Answer a task's question, only on the owner's word: tb needs answer T12 "use the old API",
    /// or by option number: tb needs answer T12 --pick 2 (a picker with several questions: --pick 1 --pick 2,3)
    Answer {
        item: String,
        /// The answer, in the owner's words
        text: Option<String>,
        /// An option's number, one --pick per question (comma-separated for a multi-select)
        #[arg(long)]
        pick: Vec<String>,
    },
    /// A task that lost its terminal: start it again from its handoff, only on the owner's word
    Restart { item: String },
    /// A task that lost its terminal but whose work shipped: a new terminal checks it and hands it in, only on the owner's word
    Finish { item: String },
    /// A goal held after a wave for review: let the next wave start, only on the owner's word
    Continue { item: String },
    /// Take an alert off the list, only on the owner's word
    Dismiss { item: String },
    /// Take an alert off the list for a while, only on the owner's word (30 minutes when left out)
    Snooze {
        item: String,
        #[arg(long, conflicts_with = "until")]
        mins: Option<i64>,
        /// tomorrow (when the work hours start) or back (after the owner's next stretch away)
        #[arg(long, value_parser = ["tomorrow", "back"])]
        until: Option<String>,
    },
}

/// A question as (question, [(label, description)], multi).
pub type Asked = (String, Vec<(String, String)>, bool);

/// One thing on the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// What `tb needs <verb>` takes: the task's ref, else the goal's for a wave stop, else the alert's id.
    pub handle: String,
    pub kind: &'static str,
    pub title: String,
    /// What it says, when that's more than its title.
    pub text: String,
    pub task: Option<String>,
    pub goal: Option<String>,
    pub alert: Option<String>,
    pub wave: Option<i64>,
    pub project: String,
    pub at: String,
    pub pr: Option<String>,
    /// The task waits on an answer.
    pub answer: bool,
    pub asks: Vec<Asked>,
    /// The asks are Claude's picker in the task's terminal: answered by option number.
    pub picker: bool,
    /// The alert's suggested fixes as (label, what to do).
    pub fixes: Vec<(String, String)>,
    pub dismiss: bool,
    pub snooze: bool,
    /// The backlog item a done-when miss holds the goal open on.
    pub miss: Option<String>,
    pub review: Option<i64>,
}

fn st<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

fn opt(v: &Value, k: &str) -> Option<String> {
    v[k].as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn phase(card: &Value) -> &str {
    card["pr"]["stage"]["phase"].as_str().unwrap_or("")
}

/// The same kinds, in the same order, as the app's home page (`ask_vm`).
fn kind(a: &Value, card: &Value) -> &'static str {
    let reason = st(card, "needs_reason");
    let has_card = card.is_object();
    let needs = st(card, "status") == "needs";
    if a["review_id"].as_i64().is_some() {
        "Weekly review"
    } else if st(a, "key").starts_with("miss:") {
        "Done-when missed"
    } else if a["urgent"] == true {
        "Urgent"
    } else if has_card && card["lost"] == true && card["looks_finished"].as_array().is_some_and(|f| !f.is_empty()) {
        "Looks finished"
    } else if has_card && card["lost"] == true {
        "Terminal lost"
    } else if reason == "start_failed" {
        "Couldn't start"
    } else if card["failed"] == true {
        "Failed"
    } else if phase(card) == "fix" {
        "Checks failed"
    } else if a["review"] == true {
        "Review the PR"
    } else if phase(card) == "merge" {
        "Ready to merge"
    } else if reason == "human" {
        "Your task"
    } else if needs && reason == "question" {
        "Question"
    } else {
        "Needs you"
    }
}

fn item(a: &Value) -> Item {
    let card = &a["card"];
    let task = opt(a, "task").or_else(|| opt(card, "ref"));
    let goal = opt(a, "goal").or_else(|| opt(&card["goal"], "ref"));
    let alert = opt(a, "id");
    let kind = kind(a, card);
    let answer = matches!(kind, "Question" | "Needs you") && st(card, "status") == "needs";
    let text = if answer { opt(card, "question").unwrap_or_else(|| st(a, "text").to_string()) } else { st(a, "text").to_string() };
    let title = opt(a, "title").or_else(|| opt(card, "title")).or_else(|| Some(text.clone()).filter(|t| !t.is_empty())).or_else(|| opt(a, "goal_name")).unwrap_or_default();
    let asks = card["ask"]["questions"]
        .as_array()
        .map(|qs| {
            qs.iter()
                .map(|q| {
                    let options = q["options"].as_array().map(|os| os.iter().map(|o| (st(o, "label").to_string(), st(o, "description").to_string())).collect()).unwrap_or_default();
                    (st(q, "question").to_string(), options, q["multi"] == true)
                })
                .collect()
        })
        .unwrap_or_default();
    let fixes = a["actions"]
        .as_array()
        .map(|xs| {
            xs.iter()
                .filter(|x| !st(x, "label").is_empty())
                .map(|x| (st(x, "label").to_string(), opt(x, "prompt").or_else(|| opt(x, "command")).unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default();
    let has_id = alert.is_some();
    Item {
        handle: task.clone().map(|t| t.replacen('H', "T", 1)).or_else(|| alert.clone()).or_else(|| goal.clone()).unwrap_or_default(),
        kind,
        text: if text == title { String::new() } else { text },
        title,
        task: task.map(|t| t.replacen('H', "T", 1)),
        goal,
        wave: card["wave"].as_i64(),
        project: opt(card, "project").or_else(|| opt(a, "project")).unwrap_or_default(),
        at: st(a, "at").to_string(),
        pr: opt(&card["pr"], "url"),
        answer,
        asks: if answer { asks } else { vec![] },
        picker: answer && card["ask"]["picker"] == true,
        fixes,
        dismiss: has_id && a["review"] != true && a["urgent"] != true && a["quiet"] != true,
        snooze: has_id,
        miss: st(a, "key").strip_prefix("miss:").filter(|n| !n.is_empty()).map(|n| format!("B{n}")),
        review: a["review_id"].as_i64(),
        alert,
    }
}

/// A goal held after a wave for review (its `stopped` line, "Wave 1 is done. Review it, then continue").
fn stop(e: &Value) -> Option<Item> {
    let g = &e["goal"];
    let line = g["stopped"].as_str().filter(|l| l.contains(" is done"))?;
    let n: i64 = line.strip_prefix("Wave ")?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    let gr = opt(g, "ref").or_else(|| g["id"].as_i64().map(|id| format!("G{id}")))?;
    Some(Item {
        handle: gr.clone(),
        kind: "Wave stop",
        title: format!("{} · wave {n} finished", st(g, "name")),
        text: line.to_string(),
        task: None,
        goal: Some(gr),
        alert: None,
        wave: Some(n),
        project: st(e, "project").to_string(),
        at: st(g, "updated_at").to_string(),
        pr: None,
        answer: false,
        asks: vec![],
        picker: false,
        fixes: vec![],
        dismiss: false,
        snooze: false,
        miss: None,
        review: None,
    })
}

fn rank(i: &Item) -> u8 {
    match i.kind {
        "Urgent" | "Terminal lost" | "Couldn't start" | "Failed" | "Checks failed" => 0,
        "Question" | "Needs you" => 1,
        "Weekly review" | "Review the PR" | "Ready to merge" => 3,
        _ => 2,
    }
}

/// The Needs you list from `GET /home`: the worst first, then the oldest.
pub fn items(home: &Value, project: Option<&str>) -> Vec<Item> {
    let mut out: Vec<Item> = home["needs"].as_array().map(|n| n.iter().map(item).collect()).unwrap_or_default();
    out.extend(home["goals"].as_array().into_iter().flatten().filter_map(stop));
    out.retain(|i| project.is_none_or(|p| i.project.is_empty() || i.project == p));
    out.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.at.cmp(&b.at)));
    out
}

/// The item `handle` names: its handle, its alert's id, or its task's ref (T12 or H12).
pub fn find<'a>(items: &'a [Item], handle: &str) -> Result<&'a Item, String> {
    let h = handle.trim();
    let as_task = h.strip_prefix(['T', 't', 'H', 'h']).filter(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit())).map(|d| format!("T{d}"));
    let as_goal = h.strip_prefix(['G', 'g']).filter(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit())).map(|d| format!("G{d}"));
    items
        .iter()
        .find(|i| i.handle == h || i.alert.as_deref() == Some(h) || (as_task.is_some() && i.task == as_task) || (as_goal.is_some() && i.task.is_none() && i.goal == as_goal))
        .ok_or_else(|| format!("{h} isn't on the Needs you list. tb needs lists what is."))
}

fn ago(at: &str) -> String {
    match taskboardd::util::wall_age_secs(Some(at)) {
        Some(s) if s < 60.0 => "now".into(),
        Some(s) if s < 3600.0 => format!("{}m", (s / 60.0) as i64),
        Some(s) if s < 86400.0 => format!("{}h", (s / 3600.0) as i64),
        Some(s) => format!("{}d", (s / 86400.0) as i64),
        None => String::new(),
    }
}

fn short(s: &str, max: usize) -> String {
    let line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max { line } else { format!("{}…", line.chars().take(max - 1).collect::<String>()) }
}

pub fn list_lines(items: &[Item]) -> Vec<String> {
    if items.is_empty() {
        return vec!["Nothing needs you.".into()];
    }
    let mut out = vec![match items.len() {
        1 => "1 thing needs you:".to_string(),
        n => format!("{n} things need you:"),
    }];
    for i in items {
        let mut where_ = vec![];
        if let Some(g) = &i.goal {
            where_.push(g.clone());
        }
        if !i.project.is_empty() {
            where_.push(i.project.clone());
        }
        let age = ago(&i.at);
        if !age.is_empty() {
            where_.push(age);
        }
        out.push(format!("{}  {} · {} · {}", i.handle, i.kind, short(&i.title, 110), where_.join(" · ")));
    }
    out.push("tb needs show <item> for one in full and what can be done about it.".into());
    out
}

/// What can be done about the item, as commands.
pub fn todo(i: &Item) -> Vec<String> {
    let h = &i.handle;
    let mut out = vec![];
    match i.kind {
        "Question" | "Needs you" if i.answer => {
            if i.picker {
                let picks = i.asks.iter().map(|_| "--pick <n>").collect::<Vec<_>>().join(" ");
                out.push(format!("tb needs answer {h} {picks}   (it's Claude's picker in the task's terminal)"));
            } else {
                out.push(format!("tb needs answer {h} \"<the answer>\""));
                if i.asks.iter().any(|q| !q.1.is_empty()) {
                    out.push(format!("tb needs answer {h} --pick <n>"));
                }
            }
        }
        "Terminal lost" | "Looks finished" => {
            out.push(format!("tb needs restart {h}   (a new terminal from its handoff)"));
            out.push(format!("tb needs finish {h}   (its work shipped: a new terminal checks it and hands it in)"));
        }
        "Couldn't start" => out.push(format!("tb start {h}")),
        "Wave stop" => out.push(format!("tb needs continue {h}   (wave {} starts)", i.wave.map(|n| n + 1).unwrap_or(0))),
        "Your task" => out.push("Only the owner does a human task: follow its steps, then press Done on its page in the app.".into()),
        "Done-when missed" => {
            if let Some(b) = &i.miss {
                out.push(format!("tb backlog show {b}, then tb backlog task {b}, tb backlog ticket {b} or tb backlog drop {b} --reason \"…\""));
            }
        }
        "Weekly review" => {
            if let Some(n) = i.review {
                out.push(format!("tb review {n}   (it stays until the owner opens it in the app)"));
            }
        }
        _ => {}
    }
    if let Some(pr) = &i.pr {
        if matches!(i.kind, "Checks failed" | "Review the PR" | "Ready to merge" | "Failed") {
            out.push(format!("The PR: {pr}"));
        }
    }
    for (label, what) in &i.fixes {
        out.push(format!("Suggested · {label}: {what}"));
    }
    if i.dismiss {
        out.push(format!("tb needs dismiss {h}"));
    }
    if i.snooze {
        out.push(format!("tb needs snooze {h} [--mins 60 | --until tomorrow | --until back]"));
    }
    out
}

pub fn show_lines(i: &Item) -> Vec<String> {
    let mut out = vec![format!("{} · {} · {}", i.handle, i.kind, i.title)];
    let mut facts = vec![];
    if let Some(t) = &i.task {
        facts.push(format!("Task {t}"));
    }
    if let Some(g) = &i.goal {
        facts.push(format!("Goal {g}"));
    }
    if !i.project.is_empty() {
        facts.push(i.project.clone());
    }
    let age = ago(&i.at);
    if !age.is_empty() {
        facts.push(format!("{age} ago").replace("now ago", "just now"));
    }
    if let Some(a) = &i.alert {
        facts.push(format!("alert {a}"));
    }
    out.push(facts.join(" · "));
    if !i.text.is_empty() {
        out.push(String::new());
        out.extend(i.text.lines().map(str::to_string));
    }
    for (n, (q, options, multi)) in i.asks.iter().enumerate() {
        out.push(String::new());
        let q = if q.is_empty() { format!("Question {}", n + 1) } else { q.clone() };
        out.push(if *multi { format!("{q} (pick any)") } else { q });
        for (k, (label, desc)) in options.iter().enumerate() {
            out.push(if desc.is_empty() { format!("  {}. {label}", k + 1) } else { format!("  {}. {label}: {desc}", k + 1) });
        }
    }
    let todo = todo(i);
    if !todo.is_empty() {
        out.push(String::new());
        out.push("What can be done:".into());
        out.extend(todo.into_iter().map(|t| format!("  {t}")));
    }
    out
}

/// One `--pick` (1-based, "2" or "1,3") as the option indexes of question `q`.
fn picked(i: &Item, q: usize, pick: &str) -> Result<Vec<usize>, String> {
    let (_, options, multi) = &i.asks[q];
    let mut out = vec![];
    for p in pick.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let n: usize = p.parse().map_err(|_| format!("--pick takes option numbers, got {p:?}"))?;
        if n == 0 || n > options.len() {
            return Err(format!("question {} has options 1 to {}, not {n}", q + 1, options.len()));
        }
        out.push(n - 1);
    }
    if out.is_empty() {
        return Err(format!("--pick names no option for question {}", q + 1));
    }
    if out.len() > 1 && !multi {
        return Err(format!("question {} takes one option", q + 1));
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// The request that answers `i`: (path, body).
pub fn answer_request(i: &Item, text: Option<&str>, picks: &[String]) -> Result<(String, Value), String> {
    let task = i.task.as_deref().filter(|_| i.answer).ok_or_else(|| format!("{} isn't waiting on an answer ({}).", i.handle, i.kind))?;
    let text = text.map(str::trim).filter(|t| !t.is_empty());
    if i.picker {
        if text.is_some() {
            return Err(format!("{task} asks with Claude's picker: answer it by option number, --pick <n>, or in its terminal."));
        }
        if picks.len() != i.asks.len() {
            return Err(format!("{task}'s picker has {} question(s): give one --pick each.", i.asks.len()));
        }
        let answers = picks.iter().enumerate().map(|(q, p)| picked(i, q, p)).collect::<Result<Vec<_>, _>>()?;
        return Ok((format!("/tasks/{task}/pick"), json!({"answers": answers})));
    }
    let text = match (text, picks) {
        (Some(_), [_, ..]) => return Err("Give the answer or --pick, not both.".into()),
        (Some(t), []) => t.to_string(),
        (None, [p]) if !i.asks.is_empty() => picked(i, 0, p)?.iter().map(|&k| i.asks[0].1[k].0.clone()).collect::<Vec<_>>().join(", "),
        (None, [_]) => return Err(format!("{task}'s question has no options: give the answer in words.")),
        (None, [_, _, ..]) => return Err("One --pick: the question has no picker.".into()),
        (None, []) => return Err("Give the answer: tb needs answer <item> \"<the answer>\".".into()),
    };
    Ok((format!("/tasks/{task}/answer"), json!({"text": text, "when": "now"})))
}

/// The request for Restart, Finish, Continue, Dismiss or Snooze on `i`: (path, body).
pub fn act_request(cmd: &NeedsCmd, i: &Item) -> Result<(String, Value), String> {
    let lost = matches!(i.kind, "Terminal lost" | "Looks finished");
    let alert = || i.alert.clone().ok_or_else(|| format!("{} has no alert to take off the list: it stays until it's handled.", i.handle));
    match cmd {
        NeedsCmd::Restart { .. } | NeedsCmd::Finish { .. } if !lost => Err(format!("{} hasn't lost its terminal ({}).", i.handle, i.kind)),
        NeedsCmd::Restart { .. } => Ok((format!("/tasks/{}/resume", i.handle), json!({"mode": "fresh"}))),
        NeedsCmd::Finish { .. } => Ok((format!("/tasks/{}/resume", i.handle), json!({"mode": "finish"}))),
        NeedsCmd::Continue { .. } => match (&i.goal, i.wave) {
            (Some(g), Some(n)) if i.kind == "Wave stop" => Ok((format!("/goals/{g}/waves/{n}/continue"), json!({}))),
            _ => Err(format!("{} isn't a goal held after a wave.", i.handle)),
        },
        NeedsCmd::Dismiss { .. } if !i.dismiss => Err(match i.kind {
            "Urgent" => "An urgent alert stays until it clears.".into(),
            "Weekly review" => "A weekly review stays until the owner opens it.".into(),
            "Review the PR" => "A PR waiting for the owner's review stays until they review it.".into(),
            _ => format!("{} has no alert to dismiss: it stays until it's handled.", i.handle),
        }),
        NeedsCmd::Dismiss { .. } => Ok((format!("/alerts/{}/dismiss", alert()?), json!({}))),
        NeedsCmd::Snooze { mins, until, .. } => {
            let body = match (mins, until) {
                (_, Some(u)) => json!({"until": u}),
                (Some(m), None) => json!({"mins": m}),
                (None, None) => json!({"mins": 30}),
            };
            Ok((format!("/alerts/{}/snooze", alert()?), body))
        }
        _ => Err("not an action".into()),
    }
}

/// What to print once `cmd` went through.
pub fn done_line(cmd: &NeedsCmd, i: &Item, v: &Value) -> String {
    let h = &i.handle;
    match cmd {
        NeedsCmd::Answer { .. } => format!("Answered {h}. Its agent carries on with it."),
        NeedsCmd::Restart { .. } => format!("Restarting {h} in a new terminal from its handoff."),
        NeedsCmd::Finish { .. } => format!("A new terminal checks {h} and hands it in."),
        NeedsCmd::Continue { .. } => {
            if v["let_start"] == true {
                format!("{h}'s wave {} can start.", i.wave.unwrap_or(0) + 1)
            } else {
                format!("{h} goes on.")
            }
        }
        NeedsCmd::Dismiss { .. } => format!("Dismissed {h}."),
        NeedsCmd::Snooze { .. } => match v["alert"]["snoozed_until"].as_str() {
            Some(at) => format!("Snoozed {h} until {}.", clock(at)),
            None => format!("Snoozed {h} until the owner's back."),
        },
        _ => String::new(),
    }
}

fn clock(at: &str) -> String {
    use chrono::{DateTime, Local};
    DateTime::parse_from_rfc3339(at).map(|t| t.with_timezone(&Local).format("%-I:%M %p").to_string()).unwrap_or_else(|_| at.to_string())
}

pub fn handle_of(cmd: &NeedsCmd) -> Option<&str> {
    match cmd {
        NeedsCmd::List { .. } => None,
        NeedsCmd::Show { item }
        | NeedsCmd::Answer { item, .. }
        | NeedsCmd::Restart { item }
        | NeedsCmd::Finish { item }
        | NeedsCmd::Continue { item }
        | NeedsCmd::Dismiss { item }
        | NeedsCmd::Snooze { item, .. } => Some(item),
    }
}

pub fn json_of(i: &Item) -> Value {
    json!({
        "item": i.handle, "kind": i.kind, "title": i.title, "text": i.text, "task": i.task, "goal": i.goal,
        "alert": i.alert, "wave": i.wave, "project": i.project, "at": i.at, "pr": i.pr,
        "asks": i.asks.iter().map(|(q, os, multi)| json!({"question": q, "multi": multi,
            "options": os.iter().map(|(l, d)| json!({"label": l, "description": d})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        "picker": i.picker, "fixes": i.fixes.iter().map(|(l, w)| json!({"label": l, "do": w})).collect::<Vec<_>>(),
        "todo": todo(i),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> Value {
        json!({
            "goals": [
                {"goal": {"ref": "G3", "name": "Polish", "stopped": "Wave 1 is done. Review it, then continue", "updated_at": "2026-10-20T10:00:00Z"}, "project": "web"},
                {"goal": {"ref": "G4", "name": "Other", "stopped": null}, "project": "web"}
            ],
            "needs": [
                {"id": "aaaa111111", "at": "2026-10-20T09:00:00Z", "text": "Weekly review: 5h on builds", "review_id": 2, "key": "review:2", "quiet": true, "card": null},
                {"id": "bbbb222222", "at": "2026-10-20T12:00:00Z", "text": "T12: Which API?", "task": "T12", "goal": "G3",
                 "card": {"ref": "T12", "title": "Move to v2", "status": "needs", "needs_reason": "question", "project": "web",
                          "question": "Which API?", "ask": {"picker": false, "questions": [{"question": "Which API?", "options": [{"label": "Old", "description": ""}, {"label": "New", "description": "v2"}]}]}}},
                {"id": null, "at": "2026-10-20T11:00:00Z", "text": "", "task": "T13",
                 "card": {"ref": "T13", "title": "Lost one", "status": "needs", "needs_reason": "lost", "lost": true, "project": "api"}},
                {"id": "cccc333333", "at": "2026-10-20T08:00:00Z", "text": "Picto misses its done-when (B268)", "goal": "G23", "key": "miss:268", "card": null,
                 "actions": [{"label": "Look", "prompt": "Look at B268", "command": ""}]},
                {"id": "dddd444444", "at": "2026-10-20T08:30:00Z", "text": "H14: do the thing", "task": "T14",
                 "card": {"ref": "H14", "title": "Do the thing", "status": "needs", "needs_reason": "human", "project": "web"}},
                {"id": "eeee555555", "at": "2026-10-20T13:00:00Z", "task": "T15",
                 "card": {"ref": "T15", "title": "Pick", "status": "needs", "needs_reason": "question", "project": "web", "question": "Two things",
                          "ask": {"picker": true, "questions": [
                              {"question": "Color?", "options": [{"label": "Red"}, {"label": "Blue"}]},
                              {"question": "Sizes?", "multi": true, "options": [{"label": "S"}, {"label": "M"}, {"label": "L"}]}]}}}
            ]
        })
    }

    #[test]
    fn lists_the_worst_first_with_a_handle_each() {
        let items = items(&home(), None);
        let got: Vec<(&str, &str)> = items.iter().map(|i| (i.handle.as_str(), i.kind)).collect();
        assert_eq!(
            got,
            vec![
                ("T13", "Terminal lost"),
                ("T12", "Question"),
                ("T15", "Question"),
                ("cccc333333", "Done-when missed"),
                ("T14", "Your task"),
                ("G3", "Wave stop"),
                ("aaaa111111", "Weekly review"),
            ]
        );
        assert_eq!(items[1].text, "Which API?");
        assert_eq!(super::items(&home(), Some("api")).iter().map(|i| i.handle.as_str()).collect::<Vec<_>>(), vec!["T13", "cccc333333", "aaaa111111"]);
    }

    #[test]
    fn finds_an_item_by_handle_alert_id_or_human_ref() {
        let items = items(&home(), None);
        assert_eq!(find(&items, "t12").unwrap().handle, "T12");
        assert_eq!(find(&items, "bbbb222222").unwrap().handle, "T12");
        assert_eq!(find(&items, "H14").unwrap().handle, "T14");
        assert_eq!(find(&items, "G3").unwrap().kind, "Wave stop");
        assert!(find(&items, "T99").unwrap_err().contains("isn't on the Needs you list"));
    }

    #[test]
    fn answers_in_words_or_by_option_and_picks_through_the_picker() {
        let items = items(&home(), None);
        let q = find(&items, "T12").unwrap();
        assert_eq!(answer_request(q, Some("the new one"), &[]).unwrap(), ("/tasks/T12/answer".into(), json!({"text": "the new one", "when": "now"})));
        assert_eq!(answer_request(q, None, &["2".into()]).unwrap().1["text"], "New");
        assert!(answer_request(q, None, &["3".into()]).unwrap_err().contains("options 1 to 2"));
        let p = find(&items, "T15").unwrap();
        assert_eq!(answer_request(p, None, &["2".into(), "3,1".into()]).unwrap(), ("/tasks/T15/pick".into(), json!({"answers": [[1], [0, 2]]})));
        assert!(answer_request(p, None, &["1,2".into(), "1".into()]).unwrap_err().contains("takes one option"));
        assert!(answer_request(p, Some("red"), &[]).unwrap_err().contains("picker"));
        assert!(answer_request(find(&items, "T13").unwrap(), Some("x"), &[]).unwrap_err().contains("isn't waiting on an answer"));
    }

    #[test]
    fn each_action_posts_where_the_apps_button_does() {
        let items = items(&home(), None);
        let lost = find(&items, "T13").unwrap();
        let restart = NeedsCmd::Restart { item: "T13".into() };
        assert_eq!(act_request(&restart, lost).unwrap(), ("/tasks/T13/resume".into(), json!({"mode": "fresh"})));
        assert_eq!(act_request(&NeedsCmd::Finish { item: "T13".into() }, lost).unwrap().1, json!({"mode": "finish"}));
        assert!(act_request(&restart, find(&items, "T12").unwrap()).unwrap_err().contains("hasn't lost its terminal"));
        let stop = find(&items, "G3").unwrap();
        assert_eq!(act_request(&NeedsCmd::Continue { item: "G3".into() }, stop).unwrap().0, "/goals/G3/waves/1/continue");
        let q = find(&items, "T12").unwrap();
        assert_eq!(act_request(&NeedsCmd::Dismiss { item: "T12".into() }, q).unwrap().0, "/alerts/bbbb222222/dismiss");
        let snooze = NeedsCmd::Snooze { item: "T12".into(), mins: None, until: Some("tomorrow".into()) };
        assert_eq!(act_request(&snooze, q).unwrap(), ("/alerts/bbbb222222/snooze".into(), json!({"until": "tomorrow"})));
        let review = find(&items, "aaaa111111").unwrap();
        assert!(act_request(&NeedsCmd::Dismiss { item: "x".into() }, review).unwrap_err().contains("stays until the owner opens it"));
        assert!(act_request(&NeedsCmd::Dismiss { item: "T13".into() }, lost).unwrap_err().contains("no alert to dismiss"));
    }

    #[test]
    fn show_numbers_the_options_and_says_what_can_be_done() {
        let items = items(&home(), None);
        let lines = show_lines(find(&items, "T12").unwrap());
        assert!(lines.contains(&"  2. New: v2".to_string()), "{lines:#?}");
        assert!(lines.contains(&"  tb needs answer T12 \"<the answer>\"".to_string()), "{lines:#?}");
        let miss = show_lines(find(&items, "cccc333333").unwrap());
        assert!(miss.iter().any(|l| l.contains("tb backlog task B268")), "{miss:#?}");
        assert!(miss.contains(&"  Suggested · Look: Look at B268".to_string()), "{miss:#?}");
        assert!(show_lines(find(&items, "T14").unwrap()).iter().any(|l| l.contains("press Done on its page in the app")));
    }
}
