//! `tb needs`: the app's Needs you list in a terminal, so the owner can work through it with an agent.
//!
//! It reads the same `GET /home` the app's home page does (the alerts, each with its task's card, and
//! the goals held after a wave for review) and acts through the same endpoints its buttons post to:
//! an answer or a picker's choices, Restart, Mark done, Continue, Dismiss, Snooze, the answers on a
//! to-do other than Done, Finish the goal without it and Set goal aside. What only the owner's own
//! click may do (Start, Done on a human task) it points at instead: `tb start`, which checks the
//! owner's word, or the app.

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
    /// or by option number: tb needs answer T12 --pick 2 (a picker with several questions: --pick 1 --pick 2,3).
    /// A backlog item that holds a PR: tb needs answer B12 --pick 1 approves it, --pick 2 rejects it
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
        #[arg(long, conflicts_with_all = ["until", "after_goal"])]
        mins: Option<i64>,
        /// tomorrow or next-week (when those work hours start), or back (after the owner's next stretch away)
        #[arg(long, value_parser = ["tomorrow", "next-week", "back"], conflicts_with = "after_goal")]
        until: Option<String>,
        /// Until this goal finishes: G<n>
        #[arg(long)]
        after_goal: Option<String>,
    },
    /// A to-do the owner can't do: hand it back so what waits finds another way, only on the owner's word
    CantDo {
        item: String,
        /// Why, for the agents that wait for it
        note: Option<String>,
    },
    /// A to-do that waits on someone else: off the list until a date, with what waits still waiting, only on the owner's word
    WaitingOn {
        item: String,
        /// The date it comes back: 2026-11-12
        #[arg(long)]
        until: String,
        /// Who or what it waits on
        note: Option<String>,
    },
    /// A to-do nothing should wait for: what waits carries on and it stays on the list, only on the owner's word
    DontWait {
        item: String,
        /// A note for the agents that carry on
        note: Option<String>,
    },
    /// A to-do that isn't needed: cancel it, only on the owner's word
    NotNeeded {
        item: String,
        /// A note for the agents that wait for it
        note: Option<String>,
    },
    /// A missed target: leave it in the backlog so the goal can finish, only on the owner's word
    Without { item: String },
    /// Set the item's goal aside, only on the owner's word
    SetAside { item: String },
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
    /// The backlog item that holds a PR and waits for the owner to approve or reject it.
    pub critical: Option<String>,
    pub review: Option<i64>,
    /// The tasks it holds up.
    pub holds: Vec<String>,
    /// The goal it holds open.
    pub holds_goal: Option<String>,
    /// An agent waits on the owner's word.
    pub waiting: bool,
}

fn st<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

fn opt(v: &Value, k: &str) -> Option<String> {
    v[k].as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn refs(holds: &Value) -> Vec<String> {
    holds["tasks"].as_array().map(|ts| ts.iter().filter_map(|t| t.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn phase(card: &Value) -> &str {
    card["pr"]["stage"]["phase"].as_str().unwrap_or("")
}

/// The key of the alert a `pr` backlog item raises while it waits for the owner (`critical-ask:<n>`).
const CRITICAL: &str = "critical-ask:";

/// The same kinds, in the same order, as the app's home page (`ask_vm`).
fn kind(a: &Value, card: &Value) -> &'static str {
    let reason = st(card, "needs_reason");
    let has_card = card.is_object();
    let needs = st(card, "status") == "needs";
    if a["review_id"].as_i64().is_some() {
        "Weekly review"
    } else if st(a, "key").starts_with("quiet:") {
        "Gone quiet"
    } else if st(a, "key").starts_with("miss:") {
        "Done-when missed"
    } else if st(a, "key").starts_with(CRITICAL) {
        "Holds a PR"
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
    let critical = st(a, "key").strip_prefix(CRITICAL).filter(|n| !n.is_empty()).map(|n| format!("B{n}"));
    let (answer, asks, fixes) = match critical {
        Some(_) => (true, vec![critical_ask()], vec![]),
        None => (answer, if answer { asks } else { vec![] }, fixes),
    };
    Item {
        handle: critical.clone().or_else(|| task.clone().map(|t| t.replacen('H', "T", 1))).or_else(|| alert.clone()).or_else(|| goal.clone()).unwrap_or_default(),
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
        asks,
        picker: answer && card["ask"]["picker"] == true,
        fixes,
        dismiss: has_id && a["review"] != true && a["urgent"] != true && a["quiet"] != true && critical.is_none(),
        snooze: has_id,
        miss: st(a, "key").strip_prefix("miss:").filter(|n| !n.is_empty()).map(|n| format!("B{n}")),
        critical,
        review: a["review_id"].as_i64(),
        alert,
        holds: refs(&a["holds"]),
        holds_goal: opt(&a["holds"], "goal"),
        waiting: a["waiting"] == true,
    }
}

/// The owner's choice on a backlog item that holds a PR: option 1 approves it, option 2 rejects it.
fn critical_ask() -> Asked {
    let options = vec![
        ("Approve".to_string(), "its fix task runs and fixes it before the PR merges".to_string()),
        ("Reject".to_string(), "it goes back to the backlog and stops holding the PR".to_string()),
    ];
    ("Approve or reject?".to_string(), options, false)
}

/// A goal held after a wave for review (its `stopped` line, "Wave 1 is done. Review it, then continue").
fn stop(e: &Value) -> Option<Item> {
    let g = &e["goal"];
    let line = g["stopped"].as_str().filter(|l| l.contains(" is done"))?;
    let n: i64 = line.strip_prefix("Wave ")?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    let gr = opt(g, "ref").or_else(|| g["id"].as_i64().map(|id| format!("G{id}")))?;
    let stage = &e["stage"];
    Some(Item {
        handle: gr.clone(),
        kind: "Wave stop",
        title: format!("{} · wave {n} finished", st(g, "name")),
        text: line.to_string(),
        task: None,
        goal: Some(gr),
        alert: opt(stage, "id"),
        wave: Some(n),
        project: st(e, "project").to_string(),
        at: st(g, "updated_at").to_string(),
        pr: None,
        answer: false,
        asks: vec![],
        picker: false,
        fixes: vec![],
        dismiss: false,
        snooze: stage.is_object(),
        miss: None,
        critical: None,
        review: None,
        holds: refs(&stage["holds"]),
        holds_goal: opt(&stage["holds"], "goal"),
        waiting: false,
    })
}

fn rank(i: &Item) -> u8 {
    match i.kind {
        "Urgent" | "Terminal lost" | "Couldn't start" | "Failed" | "Checks failed" => 0,
        "Question" | "Needs you" | "Holds a PR" => 1,
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
    let as_issue = h.strip_prefix(['B', 'b']).filter(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit())).map(|d| format!("B{d}"));
    let as_goal = h.strip_prefix(['G', 'g']).filter(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit())).map(|d| format!("G{d}"));
    items
        .iter()
        .find(|i| i.handle == h || i.alert.as_deref() == Some(h) || (as_issue.is_some() && i.critical == as_issue) || (as_task.is_some() && i.task == as_task) || (as_goal.is_some() && i.task.is_none() && i.critical.is_none() && i.goal == as_goal))
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
        where_.extend(holds_line(i));
        out.push(format!("{}  {} · {} · {}", i.handle, i.kind, short(&i.title, 110), where_.join(" · ")));
    }
    out.push("tb needs show <item> for one in full and what can be done about it.".into());
    out
}

fn holds_line(i: &Item) -> Option<String> {
    match (i.holds.len(), &i.holds_goal) {
        (0, None) => None,
        (0, Some(_)) => Some("Holds its goal".into()),
        (1, _) => Some("Holds 1 task".into()),
        (n, _) => Some(format!("Holds {n} tasks")),
    }
}

/// How much is put off and when the next of it is back: "Later 2 · back Nov 12", or None with nothing put off.
pub fn later_line(home: &Value) -> Option<String> {
    let n = home["later_count"].as_i64().filter(|n| *n > 0)?;
    Some(match home["next_back"].as_str() {
        Some(at) => format!("Later {n} · back {}", when(at, "%b %-d")),
        None => format!("Later {n}"),
    })
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
        "Holds a PR" => {
            out.push(format!("tb needs answer {h} --pick 1   (approve: its fix task runs before the PR merges)"));
            out.push(format!("tb needs answer {h} --pick 2   (reject: back to the backlog, and the PR stops waiting on it)"));
        }
        "Couldn't start" => out.push(format!("tb start {h}")),
        "Wave stop" => out.push(format!("tb needs continue {h}   (wave {} starts)", i.wave.map(|n| n + 1).unwrap_or(0))),
        "Your task" => {
            out.push("Only the owner does a human task: follow its steps, then press Done on its page in the app.".into());
            out.push(format!("tb needs cant-do {h} \"<why>\"   (hand it back: what waits finds another way)"));
            out.push(format!("tb needs waiting-on {h} --until <YYYY-MM-DD> \"<who>\"   (off the list until then)"));
            out.push(format!("tb needs dont-wait {h}   (what waits carries on, and it stays listed)"));
            out.push(format!("tb needs not-needed {h}   (cancel it)"));
        }
        "Done-when missed" => {
            if let Some(b) = &i.miss {
                out.push(format!("tb backlog show {b}, then tb backlog task {b}, tb backlog ticket {b} or tb backlog drop {b} --reason \"…\""));
                out.push(format!("tb needs without {h}   (it stays in the backlog and the goal can finish)"));
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
        out.push(format!("tb needs snooze {h} [--mins 60 | --until tomorrow | --until next-week | --until back | --after-goal G<n>]"));
    }
    if let Some(g) = &i.goal {
        out.push(format!("tb needs set-aside {h}   (sets {g} aside)"));
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
    if !i.holds.is_empty() {
        facts.push(format!("Holds {}", i.holds.join(", ")));
    } else if let Some(g) = &i.holds_goal {
        facts.push(format!("Holds {g}"));
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
    if let Some(b) = &i.critical {
        return critical_request(i, b, text, picks);
    }
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

/// Approving (`--pick 1`, or "approve") or rejecting (`--pick 2`, or "reject") backlog item `b`.
fn critical_request(i: &Item, b: &str, text: Option<&str>, picks: &[String]) -> Result<(String, Value), String> {
    let approve = match (text.map(str::trim).filter(|t| !t.is_empty()), picks) {
        (Some(_), [_, ..]) => return Err("Give the answer or --pick, not both.".into()),
        (None, [p]) => picked(i, 0, p)? == [0],
        (Some(t), []) if t.eq_ignore_ascii_case("approve") => true,
        (Some(t), []) if t.eq_ignore_ascii_case("reject") => false,
        _ => return Err(format!("Approve or reject {b}: tb needs answer {b} --pick 1 (approve) or --pick 2 (reject).")),
    };
    Ok((format!("/backlog/{b}/critical"), json!({"approve": approve})))
}

/// An answer on a to-do of the owner's other than Done.
fn owner_answer(i: &Item, answer: &str, note: &Option<String>) -> Result<(String, Value), String> {
    match i.task.as_deref().filter(|_| i.kind == "Your task") {
        Some(t) => Ok((format!("/tasks/{t}/owner-answer"), json!({"answer": answer, "note": note.as_deref().unwrap_or("")}))),
        None => Err(format!("{} isn't a to-do of the owner's ({}).", i.handle, i.kind)),
    }
}

/// The request for an action on `i`, anything but an answer: (path, body).
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
            "Holds a PR" => format!("{} stays until the owner approves or rejects it.", i.handle),
            _ => format!("{} has no alert to dismiss: it stays until it's handled.", i.handle),
        }),
        NeedsCmd::Dismiss { .. } => Ok((format!("/alerts/{}/dismiss", alert()?), json!({}))),
        NeedsCmd::Snooze { mins, until, after_goal, .. } => {
            let body = match (mins, until, after_goal) {
                (_, _, Some(g)) => json!({"after_goal": g}),
                (_, Some(u), None) => json!({"until": u.replace('-', "_")}),
                (Some(m), None, None) => json!({"mins": m}),
                (None, None, None) => json!({"mins": 30}),
            };
            Ok((format!("/alerts/{}/snooze", alert()?), body))
        }
        NeedsCmd::CantDo { note, .. } => owner_answer(i, "cant_do", note),
        NeedsCmd::NotNeeded { note, .. } => owner_answer(i, "not_needed", note),
        NeedsCmd::DontWait { note, .. } => owner_answer(i, "dont_wait", note),
        NeedsCmd::WaitingOn { until, note, .. } => {
            let (path, mut body) = owner_answer(i, "waiting", note)?;
            body["until"] = json!(until);
            Ok((path, body))
        }
        NeedsCmd::Without { .. } => match &i.miss {
            Some(b) => Ok((format!("/backlog/{b}/without-goal"), json!({}))),
            None => Err(format!("{} isn't a missed target ({}).", i.handle, i.kind)),
        },
        NeedsCmd::SetAside { .. } => match &i.goal {
            Some(g) => Ok((format!("/goals/{g}/set-aside"), json!({}))),
            None => Err(format!("{} isn't in a goal.", i.handle)),
        },
        _ => Err("not an action".into()),
    }
}

/// What to print once `cmd` went through.
pub fn done_line(cmd: &NeedsCmd, i: &Item, v: &Value) -> String {
    let h = &i.handle;
    match cmd {
        NeedsCmd::Answer { .. } if i.critical.is_some() => v["text"].as_str().map(str::to_string).unwrap_or_else(|| format!("Answered {h}.")),
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
        NeedsCmd::Snooze { after_goal: Some(g), .. } => format!("Snoozed {h} until {g} finishes."),
        NeedsCmd::Snooze { .. } => match v["alert"]["snoozed_until"].as_str() {
            Some(at) => format!("Snoozed {h} until {}.", when(at, "%b %-d, %-I:%M %p")),
            None => format!("Snoozed {h} until the owner's back."),
        },
        NeedsCmd::CantDo { .. } => format!("Handed {h} back. What waits for it finds another way."),
        NeedsCmd::NotNeeded { .. } => format!("Cancelled {h}."),
        NeedsCmd::DontWait { .. } => format!("Nothing waits for {h} now. It stays on the list."),
        NeedsCmd::WaitingOn { until, .. } => format!("{h} is off the list until {until}."),
        NeedsCmd::Without { .. } => format!("{} stays in the backlog, and the goal can finish without it.", i.miss.as_deref().unwrap_or(h)),
        NeedsCmd::SetAside { .. } => format!("Set {} aside.", i.goal.as_deref().unwrap_or(h)),
        _ => String::new(),
    }
}

/// `at` as a time on the 12-hour clock today, or in `other_day`'s format on any other day.
fn when(at: &str, other_day: &str) -> String {
    use chrono::{DateTime, Local};
    let Ok(t) = DateTime::parse_from_rfc3339(at).map(|t| t.with_timezone(&Local)) else { return at.to_string() };
    let format = if t.date_naive() == Local::now().date_naive() { "%-I:%M %p" } else { other_day };
    t.format(format).to_string()
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
        | NeedsCmd::Snooze { item, .. }
        | NeedsCmd::CantDo { item, .. }
        | NeedsCmd::WaitingOn { item, .. }
        | NeedsCmd::DontWait { item, .. }
        | NeedsCmd::NotNeeded { item, .. }
        | NeedsCmd::Without { item }
        | NeedsCmd::SetAside { item } => Some(item),
    }
}

pub fn json_of(i: &Item) -> Value {
    json!({
        "item": i.handle, "kind": i.kind, "title": i.title, "text": i.text, "task": i.task, "goal": i.goal,
        "alert": i.alert, "wave": i.wave, "project": i.project, "at": i.at, "pr": i.pr,
        "asks": i.asks.iter().map(|(q, os, multi)| json!({"question": q, "multi": multi,
            "options": os.iter().map(|(l, d)| json!({"label": l, "description": d})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        "picker": i.picker, "fixes": i.fixes.iter().map(|(l, w)| json!({"label": l, "do": w})).collect::<Vec<_>>(),
        "holds": {"tasks": i.holds, "goal": i.holds_goal}, "waiting": i.waiting,
        "todo": todo(i),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> Value {
        json!({
            "goals": [
                {"goal": {"ref": "G3", "name": "Polish", "stopped": "Wave 1 is done. Review it, then continue", "updated_at": "2026-10-20T10:00:00Z"}, "project": "web",
                 "stage": {"id": "stage:G3:1", "wave": 1, "holds": {"tasks": ["T20", "T21"], "goal": "G3"}}},
                {"goal": {"ref": "G4", "name": "Other", "stopped": null}, "project": "web"}
            ],
            "needs": [
                {"id": "aaaa111111", "at": "2026-10-20T09:00:00Z", "text": "Weekly review: 5h on builds", "review_id": 2, "key": "review:2", "quiet": true, "card": null},
                {"id": "bbbb222222", "at": "2026-10-20T12:00:00Z", "text": "T12: Which API?", "task": "T12", "goal": "G3", "waiting": true,
                 "holds": {"tasks": ["T30"], "goal": "G3"},
                 "card": {"ref": "T12", "title": "Move to v2", "status": "needs", "needs_reason": "question", "project": "web",
                          "question": "Which API?", "ask": {"picker": false, "questions": [{"question": "Which API?", "options": [{"label": "Old", "description": ""}, {"label": "New", "description": "v2"}]}]}}},
                {"id": null, "at": "2026-10-20T11:00:00Z", "text": "", "task": "T13",
                 "card": {"ref": "T13", "title": "Lost one", "status": "needs", "needs_reason": "lost", "lost": true, "project": "api"}},
                {"id": "cccc333333", "at": "2026-10-20T08:00:00Z", "text": "Picto misses its done-when (B268)", "goal": "G23", "key": "miss:268", "card": null,
                 "holds": {"tasks": [], "goal": "G23"},
                 "actions": [{"label": "Look", "prompt": "Look at B268", "command": ""}]},
                {"id": "ffff666666", "at": "2026-10-20T14:00:00Z", "text": "B301 holds T40's PR #12: …", "title": "B301 holds T40's PR #12", "goal": "G3",
                 "key": "critical-ask:301", "project": "web", "card": null,
                 "actions": [{"label": "Approve", "prompt": "Run it", "command": "tb needs answer B301 --pick 1"}]},
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
                ("B301", "Holds a PR"),
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
        let snooze = NeedsCmd::Snooze { item: "T12".into(), mins: None, until: Some("tomorrow".into()), after_goal: None };
        assert_eq!(act_request(&snooze, q).unwrap(), ("/alerts/bbbb222222/snooze".into(), json!({"until": "tomorrow"})));
        let review = find(&items, "aaaa111111").unwrap();
        assert!(act_request(&NeedsCmd::Dismiss { item: "x".into() }, review).unwrap_err().contains("stays until the owner opens it"));
        assert!(act_request(&NeedsCmd::Dismiss { item: "T13".into() }, lost).unwrap_err().contains("no alert to dismiss"));
    }

    fn snooze(item: &str, until: Option<&str>, after_goal: Option<&str>) -> NeedsCmd {
        NeedsCmd::Snooze { item: item.into(), mins: None, until: until.map(str::to_string), after_goal: after_goal.map(str::to_string) }
    }

    #[test]
    fn later_takes_next_week_a_goal_and_a_finished_stage() {
        let items = items(&home(), None);
        let q = find(&items, "T12").unwrap();
        assert_eq!(act_request(&snooze("T12", Some("next-week"), None), q).unwrap().1, json!({"until": "next_week"}));
        assert_eq!(act_request(&snooze("T12", None, Some("G9")), q).unwrap().1, json!({"after_goal": "G9"}));
        assert_eq!(done_line(&snooze("T12", None, Some("G9")), q, &json!({})), "Snoozed T12 until G9 finishes.");
        let stage = find(&items, "G3").unwrap();
        assert!(stage.snooze && !stage.dismiss);
        assert_eq!(act_request(&snooze("G3", Some("tomorrow"), None), stage).unwrap().0, "/alerts/stage:G3:1/snooze");
        assert!(todo(stage).iter().any(|l| l.contains("--after-goal")));
    }

    #[test]
    fn a_goal_that_read_as_stopped_without_a_stage_cant_be_put_off() {
        let home = json!({"goals": [{"goal": {"ref": "G3", "name": "Polish", "stopped": "Wave 1 is done. Review it, then continue"}, "project": "web"}], "needs": []});
        let items = items(&home, None);
        assert!(act_request(&snooze("G3", Some("tomorrow"), None), &items[0]).unwrap_err().contains("no alert"));
    }

    #[test]
    fn a_to_do_takes_the_other_answers_and_nothing_else_does() {
        let items = items(&home(), None);
        let to_do = find(&items, "H14").unwrap();
        let note = Some("Oracle won't take my card".to_string());
        let cant = NeedsCmd::CantDo { item: "T14".into(), note: note.clone() };
        assert_eq!(act_request(&cant, to_do).unwrap(), ("/tasks/T14/owner-answer".into(), json!({"answer": "cant_do", "note": note})));
        assert_eq!(act_request(&NeedsCmd::NotNeeded { item: "T14".into(), note: None }, to_do).unwrap().1, json!({"answer": "not_needed", "note": ""}));
        assert_eq!(act_request(&NeedsCmd::DontWait { item: "T14".into(), note: None }, to_do).unwrap().1, json!({"answer": "dont_wait", "note": ""}));
        let waiting = NeedsCmd::WaitingOn { item: "T14".into(), until: "2026-11-12".into(), note: Some("Sam".into()) };
        assert_eq!(act_request(&waiting, to_do).unwrap().1, json!({"answer": "waiting", "note": "Sam", "until": "2026-11-12"}));
        assert_eq!(done_line(&waiting, to_do, &json!({})), "T14 is off the list until 2026-11-12.");
        assert!(act_request(&cant, find(&items, "T12").unwrap()).unwrap_err().contains("isn't a to-do"));
        let lines = show_lines(to_do);
        for verb in ["cant-do", "waiting-on", "dont-wait", "not-needed"] {
            assert!(lines.iter().any(|l| l.contains(&format!("tb needs {verb} T14"))), "{lines:#?}");
        }
    }

    #[test]
    fn a_miss_finishes_without_it_and_a_goals_item_sets_it_aside() {
        let items = items(&home(), None);
        let miss = find(&items, "cccc333333").unwrap();
        let without = NeedsCmd::Without { item: "cccc333333".into() };
        assert_eq!(act_request(&without, miss).unwrap(), ("/backlog/B268/without-goal".into(), json!({})));
        assert_eq!(done_line(&without, miss, &json!({})), "B268 stays in the backlog, and the goal can finish without it.");
        assert!(act_request(&without, find(&items, "T12").unwrap()).unwrap_err().contains("isn't a missed target"));
        assert!(show_lines(miss).iter().any(|l| l.contains("tb needs without cccc333333")));
        assert_eq!(act_request(&NeedsCmd::SetAside { item: "T12".into() }, find(&items, "T12").unwrap()).unwrap().0, "/goals/G3/set-aside");
        assert!(act_request(&NeedsCmd::SetAside { item: "T13".into() }, find(&items, "T13").unwrap()).unwrap_err().contains("isn't in a goal"));
    }

    #[test]
    fn rows_say_what_they_hold_and_a_line_says_whats_put_off() {
        let items = items(&home(), None);
        let lines = list_lines(&items);
        let row = |h: &str| lines.iter().find(|l| l.starts_with(h)).unwrap().clone();
        assert!(row("T12").ends_with("Holds 1 task"), "{lines:#?}");
        assert!(row("G3").ends_with("Holds 2 tasks"), "{lines:#?}");
        assert!(row("cccc333333").ends_with("Holds its goal"), "{lines:#?}");
        assert!(!row("T13").contains("Holds"), "{lines:#?}");
        assert!(show_lines(find(&items, "T12").unwrap())[1].contains("Holds T30"));
        assert_eq!(later_line(&json!({"later_count": 0})), None);
        assert_eq!(later_line(&json!({"later_count": 2})).unwrap(), "Later 2");
        assert_eq!(later_line(&json!({"later_count": 2, "next_back": "2030-11-12T17:00:00Z"})).unwrap(), "Later 2 · back Nov 12");
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

    #[test]
    fn a_pr_holding_item_is_approved_or_rejected_and_stays_until_it_is() {
        let items = items(&home(), None);
        let b = find(&items, "b301").unwrap();
        assert_eq!(b.handle, "B301");
        assert_eq!(answer_request(b, None, &["1".into()]).unwrap(), ("/backlog/B301/critical".into(), json!({"approve": true})));
        assert_eq!(answer_request(b, None, &["2".into()]).unwrap().1, json!({"approve": false}));
        assert_eq!(answer_request(b, Some("Reject"), &[]).unwrap().1, json!({"approve": false}));
        assert!(answer_request(b, Some("maybe"), &[]).unwrap_err().contains("--pick 1 (approve)"));
        assert!(act_request(&NeedsCmd::Dismiss { item: "B301".into() }, b).unwrap_err().contains("approves or rejects"));
        let lines = show_lines(b);
        assert!(lines.contains(&"  1. Approve: its fix task runs and fixes it before the PR merges".to_string()), "{lines:#?}");
        assert!(lines.iter().any(|l| l.starts_with("  tb needs answer B301 --pick 2")), "{lines:#?}");
        assert!(!lines.iter().any(|l| l.contains("Suggested")), "{lines:#?}");
        let done = done_line(&NeedsCmd::Answer { item: "B301".into(), text: None, pick: vec!["1".into()] }, b, &json!({"text": "Approved B301."}));
        assert_eq!(done, "Approved B301.");
    }
}
