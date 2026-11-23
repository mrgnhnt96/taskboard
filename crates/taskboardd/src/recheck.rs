//! The Needs you re-check. The screener (`screen.rs`) reads a question or human task once, when it's
//! raised; items raised before it existed, or before the owner's rule files last changed, were never
//! read against what's true now. During work hours, every `questions.recheck_every_mins`, the board
//! reads those again on the AI thread. The first model (`questions.model`) reads each, and what it
//! would leave with the owner goes to the second (`questions.human_model`) before it stays.
//!
//! What needn't wait on the owner comes off the list, and each such step is recorded for the app's
//! Handled for you list (`handled.rs`):
//! - an answer goes to the task as the owner's Answer would (`api::give_answer`); for a human task the
//!   answer finishes it, and the tasks that wait for it read it;
//! - a human task an agent can do goes back to the agent that made it or waits for it, with the why,
//!   or to a new agent task (no PR) when none is live.
//!
//! An item is stamped with what it said when it was read: it's read again only once it or the rules change.

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, deliver, handled, handoff, hours, human, ops, p, screen, waitsfor};

/// The settings key for when the last re-check started.
const STATE: &str = "recheck_at";
/// How many items one re-check reads: each is one model call or two.
const PER_SWEEP: usize = 12;
/// When the board began screening human tasks (v0.1.0-beta.83): ones raised before it were never read.
const HUMAN_SCREEN_SINCE: &str = "2026-11-10T00:00:00Z";
/// The task context key that holds an item's last re-check.
const STAMP: &str = "recheck";
/// The fewest minutes between re-checks, whatever the config says.
const MIN_EVERY_MINS: u64 = 10;

static RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Item {
    Question,
    Human,
}

impl Item {
    fn name(self) -> &'static str {
        match self {
            Item::Question => "question",
            Item::Human => "human",
        }
    }
}

/// What the screeners decided about an item.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// It stays with the owner.
    Keep,
    /// The rules or the task settle it.
    Answered { answer: String, source: String },
    /// An agent can do it.
    Agent { why: String },
}

/// The screener's output as a verdict: None when it gave none (it failed or is off).
pub fn verdict_of(v: Option<&Value>) -> Option<Verdict> {
    let v = v?;
    let text = |k: &str| v[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    match v["verdict"].as_str()? {
        "answered" => Some(Verdict::Answered { answer: text("answer")?, source: text("source").unwrap_or_else(|| "the rules".into()) }),
        "agent" => Some(Verdict::Agent { why: text("why").unwrap_or_else(|| "an agent can do it with the tools it has".into()) }),
        "ask" | "owner" => Some(Verdict::Keep),
        _ => None,
    }
}

/// The two reads together: the first model's verdict stands unless it would keep the item with the
/// owner (or gave none), and then the second model's does. None when neither gave one, so the item
/// isn't stamped and is read again next time.
pub fn settle(first: Option<Verdict>, second: impl FnOnce() -> Option<Verdict>) -> Option<Verdict> {
    match first {
        Some(Verdict::Keep) => second().or(Some(Verdict::Keep)),
        Some(v) => Some(v),
        None => second(),
    }
}

/// Whether an item is due a re-check. `stamp` is its last one (`{key, at}`), `key` what it says now,
/// `raised` when it reached the owner, `rules_at` when the rule files last changed and `since` when the
/// screener began reading items like it. Read once already: due when it or the rules changed since.
/// Never read: due when it was raised before the screener read such items, or before the rules changed.
pub fn due(stamp: Option<&Value>, key: &str, raised: f64, rules_at: f64, since: f64) -> bool {
    match stamp.filter(|s| s.is_object()) {
        Some(s) => s["key"].as_str() != Some(key) || s["at"].as_str().and_then(parse_iso).is_none_or(|at| at < rules_at),
        None => raised < rules_at.max(since),
    }
}

/// What the item says, as the screener reads it: the question, or the human task's title and steps.
fn key_of(t: &Row, item: Item) -> String {
    let text = match item {
        Item::Question => t.st("question"),
        Item::Human => format!("{}\n{}", t.st("title"), t.st("detail")),
    };
    format!("{:016x}", fnv(&text))
}

/// FNV-1a: a stable hash, so a stamp still matches after the daemon is rebuilt.
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// When the owner's rule files last changed: the newest of their modified times, 0 with none.
fn rules_at(app: &App) -> f64 {
    app.cfg
        .questions
        .rules
        .iter()
        .flat_map(|r| screen::rule_files(r))
        .filter_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .filter_map(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .fold(0.0, f64::max)
}

/// When the item reached the owner: its latest question, or when the human task was made ready.
fn raised_at(app: &App, t: &Row, item: Item) -> Result<f64> {
    let at = match item {
        Item::Question => app.db.val("SELECT MAX(at) FROM events WHERE task_id = ? AND kind = 'question'", p![t.id()])?,
        Item::Human => app.db.val(
            "SELECT MAX(at) FROM events WHERE task_id = ? AND kind = 'status' AND (text = 'Ready for you' OR text LIKE 'Reopened%')",
            p![t.id()],
        )?,
    };
    Ok(at.as_str().or(t.s("created_at")).and_then(parse_iso).unwrap_or(0.0))
}

/// A human task the owner made, or one an agent made on their word: it's theirs, whatever it is.
fn owners_own(t: &Row) -> bool {
    let o = jloads_obj(t.s("origin"));
    o.get("by").and_then(|b| b.as_str()) == Some(board::OWNER) || o.get("from").and_then(|f| f.as_str()) == Some("Added on the board")
}

/// The items waiting on the owner that are due a re-check, oldest first.
pub fn picks(app: &App) -> Result<Vec<(Row, Item)>> {
    let rules = rules_at(app);
    let human_since = parse_iso(HUMAN_SCREEN_SINCE).unwrap_or(0.0);
    let rows = app.db.q(
        "SELECT * FROM tasks WHERE status = 'needs' AND answered_at IS NULL AND COALESCE(question, '') != '' \
         AND ((needs_reason = 'question' AND COALESCE(kind, '') != ?) OR (needs_reason = ? AND kind = ? AND owner_answer IS NULL)) ORDER BY id",
        p![human::KIND, human::NEEDS, human::KIND],
    )?;
    let mut out = vec![];
    for t in rows {
        let item = if human::is(&t) { Item::Human } else { Item::Question };
        if (item == Item::Question && crate::ask::is_picker(&t)) || (item == Item::Human && owners_own(&t)) {
            continue;
        }
        // Questions were screened from the start: only a change to the rules makes one due.
        let since = if item == Item::Human { human_since } else { 0.0 };
        if due(board::task_context(&t).get(STAMP), &key_of(&t, item), raised_at(app, &t, item)?, rules, since) {
            out.push((t, item));
        }
    }
    Ok(out)
}

/// The runner's turn: starts a re-check on the AI thread when one is due, during work hours.
pub fn tick(app: &App) -> Result<()> {
    let q = &app.cfg.questions;
    if !q.screen || !q.recheck || !hours::is_open(app) {
        return Ok(());
    }
    let every = q.recheck_every_mins.max(MIN_EVERY_MINS) as f64 * 60.0;
    if app.db.get_setting(STATE)?.as_deref().and_then(parse_iso).is_some_and(|at| now_ts() - at < every) {
        return Ok(());
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    app.db.set_setting(STATE, Some(&now_iso()))?;
    app.queue_ai(Box::new(|a: &App| {
        let r = run(a);
        RUNNING.store(false, Ordering::SeqCst);
        if let Err(e) = r {
            a.info(format!("recheck: failed: {}", e.message));
        }
    }));
    Ok(())
}

/// One re-check: reads up to [`PER_SWEEP`] due items and acts on each verdict.
pub fn run(app: &App) -> Result<()> {
    for (t, item) in picks(app)?.into_iter().take(PER_SWEEP) {
        if let Err(e) = check(app, &t, item) {
            app.info(format!("recheck: {} failed: {}", human::task_ref(&t), e.message));
        }
    }
    Ok(())
}

/// The agent's task behind a human task, if open: its origin names the terminal it was made from,
/// whose name starts with its task ("T12 Fix the login…") when the board named it, or else the task on
/// the terminal of that name now.
fn maker(app: &App, h: &Row) -> Result<Option<Row>> {
    let o = jloads_obj(h.s("origin"));
    let by = o.get("by").and_then(|b| b.as_str()).unwrap_or("").trim();
    if by.is_empty() {
        return Ok(None);
    }
    let named = by.split_whitespace().next().and_then(|w| w.strip_prefix('T')).and_then(|n| n.parse::<i64>().ok());
    let t = match named {
        Some(id) => board::find_task(app, Some(id))?,
        None => match app.db.val("SELECT id FROM sessions WHERE name = ? AND COALESCE(status, '') != 'gone' ORDER BY seen_at DESC LIMIT 1", p![by])?.as_str() {
            Some(sid) => board::task_for_session(app, Some(sid))?,
            None => None,
        },
    };
    Ok(t.filter(|t| t.s("status") != Some("done") && !human::is(t)))
}

fn check(app: &App, t: &Row, item: Item) -> Result<()> {
    let q = &app.cfg.questions;
    let key = key_of(t, item);
    let read = |model: &str| -> Option<Verdict> {
        match item {
            Item::Question => verdict_of(screen::question_by(app, Some(t), &t.st("question"), model).as_ref()),
            Item::Human => {
                let m = maker(app, t).ok().flatten();
                let sid = m.as_ref().and_then(|m| m.s("session_id")).map(str::to_string);
                verdict_of(screen::human_task_by(app, m.as_ref(), sid.as_deref(), &t.st("title"), &t.st("detail"), model).as_ref())
            }
        }
    };
    let first = read(&q.model);
    let second_model = q.human_model();
    let verdict = settle(first, || if second_model == q.model { None } else { read(second_model) });
    let Some(verdict) = verdict else {
        app.info(format!("recheck: no verdict on {}; it's read again next time", human::task_ref(t)));
        return Ok(());
    };
    app.db.tx(|| {
        let now = board::get_task(app, t.id())?;
        // It changed while it was read: the next re-check reads it as it is now.
        if now.s("status") != Some("needs") || now.s("answered_at").is_some() || key_of(&now, item) != key {
            return Ok(());
        }
        let mut ctx = board::task_context(&now);
        ctx.insert(STAMP.into(), json!({"key": key, "at": now_iso(), "verdict": verdict_name(&verdict)}));
        board::save_context(app, now.id(), &ctx, false)?;
        act(app, &now, item, &verdict)
    })
}

fn verdict_name(v: &Verdict) -> &'static str {
    match v {
        Verdict::Keep => "keep",
        Verdict::Answered { .. } => "answered",
        Verdict::Agent { .. } => "agent",
    }
}

/// Acts on the verdict for item `t`, in a transaction.
pub fn act(app: &App, t: &Row, item: Item, v: &Verdict) -> Result<()> {
    match (item, v) {
        (_, Verdict::Keep) => Ok(()),
        (Item::Question, Verdict::Answered { answer, source }) => answer_question(app, t, answer, source),
        // The question screener has no agent verdict; should one come, the question stays.
        (Item::Question, Verdict::Agent { .. }) => Ok(()),
        (Item::Human, Verdict::Answered { answer, source }) => settle_human(app, t, answer, source),
        (Item::Human, Verdict::Agent { why }) => hand_human_back(app, t, why),
    }
}

fn answer_question(app: &App, t: &Row, answer: &str, source: &str) -> Result<()> {
    let owner = &app.cfg.owner;
    let asked = t.st("question");
    let message = format!(
        "{} Answered from {source} without asking {owner}, when the board read your question again:\n{answer}\n\nCarry on with the work. \
         If this answer really doesn't fit, ask again with {} question and say why.",
        handoff::marker(t.id()),
        board::tb_cmd(app)
    );
    crate::api::give_answer(app, t.id(), answer, &message, board::BOARD, &format!("Answered from {source} without asking you: {answer}"))?;
    let text = format!("Answered its question from {source}: {answer}");
    handled::record(app, t.id(), "recheck", &text, json!({"item": Item::Question.name(), "verdict": "answered", "was": asked, "answer": answer, "source": source}))?;
    Ok(())
}

/// The live tasks a human task's verdict goes back to: the agent's task that made it, when it's in a
/// terminal, and the open tasks waiting for it.
fn takers(app: &App, h: &Row) -> Result<(Option<Row>, Vec<Row>)> {
    let made = maker(app, h)?.filter(|m| board::live_session(app, m).ok().flatten().is_some());
    let waiting = waitsfor::waiting_on(app, h)?;
    Ok((made, waiting))
}

fn settle_human(app: &App, h: &Row, answer: &str, source: &str) -> Result<()> {
    let (owner, r) = (&app.cfg.owner, human::task_ref(h));
    let (made, waiting) = takers(app, h)?;
    let note = format!("{answer} (from {source})");
    if let Some(m) = made.as_ref().filter(|m| !waiting.iter().any(|w| w.id() == m.id())) {
        let text = format!(
            "{} The board took {r} “{}” off {owner}'s list: {owner}'s rules already settle it ({source}): {answer}\nCarry on with that.",
            handoff::marker(m.id()),
            short(&h.st("title"), 80)
        );
        deliver::add(app, "recheck", &text, m.id(), None, None)?;
    }
    human::take_off(app, h, human::BOARD_ANSWERED, &note, &format!("Settled by {owner}'s rules ({source}): {answer}"))?;
    let text = format!("Settled by {source}: {answer}");
    handled::record(app, h.id(), "recheck", &text, json!({"item": Item::Human.name(), "verdict": "answered", "answer": answer, "source": source}))?;
    Ok(())
}

fn hand_human_back(app: &App, h: &Row, why: &str) -> Result<()> {
    let (owner, r) = (&app.cfg.owner, human::task_ref(h));
    let (made, waiting) = takers(app, h)?;
    let title = short(&h.st("title"), 80);
    let mut to: Vec<String> = waiting.iter().map(|w| rf("task", w.id())).collect();
    if let Some(m) = made.as_ref().filter(|m| !waiting.iter().any(|w| w.id() == m.id())) {
        let text = format!(
            "{} The board took {r} “{title}” off {owner}'s list: an agent can do it. {why}\nDo it yourself, or make it an agent task \
             (--no-pr) and {} wait-for T<n> on it. If only part of it truly needs {owner}, make a human task of just that part.\n\nIts steps:\n{}",
            handoff::marker(m.id()),
            board::tb_cmd(app),
            clip(&h.st("detail"), 4000)
        );
        deliver::add(app, "recheck", &text, m.id(), None, None)?;
        to.push(rf("task", m.id()));
    }
    let mut note = why.to_string();
    if to.is_empty() {
        let made = ops::new_task(
            app,
            &json!({"title": h.st("title"), "project": h.st("project"), "ships_pr": false, "pickup": {"mode": "queue"},
                    "detail": format!("{owner} was asked to do this as a human task ({r}), but an agent can: {why}\n\n{}\n\nIf a step truly needs {owner}, make a human task of just that step.", h.st("detail")),
                    "origin": {"from": format!("Taken off {owner}'s Needs you list ({r})"), "by": board::BOARD}}),
            board::BOARD,
            Some(&format!("Made from {r}, which an agent can do")),
        )?;
        let t = made["ref"].as_str().unwrap_or("").to_string();
        note = format!("{why} {t} does it.");
        to.push(t);
    }
    human::take_off(app, h, human::BOARD_AGENT, &note, &format!("An agent can do it, so it went to {}: {why}", to.join(", ")))?;
    let text = format!("Not for {owner}: an agent can do it. Went to {}. {why}", to.join(", "));
    handled::record(app, h.id(), "recheck", &text, json!({"item": Item::Human.name(), "verdict": "agent", "why": why, "to": to}))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(key: &str, at: &str) -> Value {
        json!({"key": key, "at": at, "verdict": "keep"})
    }

    #[test]
    fn an_item_never_read_is_due_when_raised_before_the_screener_or_the_rules() {
        let (since, rules) = (1000.0, 2000.0);
        assert!(due(None, "k", 500.0, 0.0, since), "raised before the screener read such items");
        assert!(due(None, "k", 1500.0, rules, since), "raised before the rules changed");
        assert!(!due(None, "k", 2500.0, rules, since), "read when it was raised, under today's rules");
        assert!(!due(None, "k", 1500.0, 0.0, since), "no rule files and screened when raised");
    }

    #[test]
    fn an_item_read_already_is_due_only_once_it_or_the_rules_change() {
        let at = "2026-11-20T10:00:00Z";
        let read = parse_iso(at).unwrap();
        assert!(!due(Some(&stamp("k", at)), "k", 0.0, read - 60.0, 0.0), "unchanged since its last read");
        assert!(due(Some(&stamp("k", at)), "k2", 0.0, read - 60.0, 0.0), "its text changed");
        assert!(due(Some(&stamp("k", at)), "k", 0.0, read + 60.0, 0.0), "the rules changed after");
        assert!(due(Some(&json!({"key": "k"})), "k", 0.0, 1.0, 0.0), "a stamp with no time");
        assert!(due(Some(&json!("old")), "k", 0.0, 0.0, 1.0), "not a stamp: read as never read");
    }

    #[test]
    fn the_key_follows_what_the_item_says() {
        let mut t = Row::new();
        t.insert("question".into(), json!("Which plan?"));
        let k = key_of(&t, Item::Question);
        assert_eq!(k, key_of(&t, Item::Question));
        t.insert("question".into(), json!("Which plan now?"));
        assert_ne!(k, key_of(&t, Item::Question));
        t.insert("title".into(), json!("Find the retention"));
        assert_ne!(key_of(&t, Item::Human), key_of(&t, Item::Question));
    }

    #[test]
    fn verdicts_need_what_they_act_on() {
        assert_eq!(verdict_of(Some(&json!({"verdict": "ask", "question": "x"}))), Some(Verdict::Keep));
        assert_eq!(verdict_of(Some(&json!({"verdict": "owner"}))), Some(Verdict::Keep));
        assert_eq!(verdict_of(Some(&json!({"verdict": "answered", "answer": " "}))), None, "an answer with no words");
        assert_eq!(
            verdict_of(Some(&json!({"verdict": "answered", "answer": "Use the old API"}))),
            Some(Verdict::Answered { answer: "Use the old API".into(), source: "the rules".into() })
        );
        assert_eq!(
            verdict_of(Some(&json!({"verdict": "agent", "why": " Use the Amplitude MCP. "}))),
            Some(Verdict::Agent { why: "Use the Amplitude MCP.".into() })
        );
        assert_eq!(verdict_of(Some(&json!({"verdict": "maybe"}))), None);
        assert_eq!(verdict_of(None), None);
    }

    #[test]
    fn the_second_model_reads_only_what_the_first_would_keep() {
        let agent = Verdict::Agent { why: "x".into() };
        let answered = Verdict::Answered { answer: "a".into(), source: "s".into() };
        assert_eq!(settle(Some(agent.clone()), || panic!("not asked")), Some(agent.clone()));
        assert_eq!(settle(Some(Verdict::Keep), || Some(answered.clone())), Some(answered.clone()));
        assert_eq!(settle(Some(Verdict::Keep), || None), Some(Verdict::Keep), "the second failed: it stays");
        assert_eq!(settle(None, || Some(agent.clone())), Some(agent));
        assert_eq!(settle(None, || None), None, "neither read it: not stamped");
    }
}
