//! Couldn't start and Terminal lost items often come from a short outage (git fetch couldn't reach
//! github.com, Midna restarting). During the work hours the board tries them again on its own, with no
//! model: a start that failed is queued to start again, as `tb start` does, and a lost task is queued to
//! start again from its handoff, as `tb needs restart` does, unless its terminal was closed by hand, it
//! looks finished or it waits on an open question. Each try waits longer (`WAITS`), so the item is off
//! Needs you while it waits; once the last one fails it stays on Needs you and says why. Outside the
//! hours an item waits on the list, and `sweep` tries it once they open. Each try is logged on the task as Handled for you (`handled::record`, by `retry`).
//!
//! This comes after the quick retries a start or a lost terminal already gets (`jobs::start_failed`,
//! `board::mark_lost`), where the task would otherwise go on Needs you.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, dispatch, fields, handled, hours, p};

/// `{"tries", "at", "held"?}`: how many times the board tried the task again on its own and when it
/// last did. `held` ("gave_up", or "no" for a start that isn't tried again) stops the tries until the
/// task leaves Needs you.
pub const ADDED: &[(&str, &str, &str)] = &[("tasks", "needs_retry", "TEXT")];

/// How long each try waits before the task starts again: 10, 30 and 60 minutes.
pub const WAITS: [i64; 3] = [600, 1800, 3600];
/// A task's tries start over this long after its last one: a later outage gets tries of its own.
pub const FRESH_SECS: f64 = 4.0 * 3600.0;

const GAVE_UP: &str = "gave_up";
const NOT_RETRIED: &str = "no";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Couldn't start (`needs_reason` start_failed).
    Start,
    /// Terminal lost (`lost`).
    Lost,
}

/// A task's `needs_retry`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub tries: i64,
    /// When the last try was, as a board timestamp.
    pub at: Option<f64>,
    pub held: bool,
}

impl State {
    pub fn of(t: &Row) -> State {
        let r = jloads_obj(t.s("needs_retry"));
        State { tries: r.i0("tries"), at: r.s("at").and_then(parse_iso), held: r.s("held").is_some() }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Queue it to start again after `wait` seconds: try `n` of `WAITS.len()`.
    Retry { n: i64, wait: i64 },
    /// It failed after `tries` tries: it stays on Needs you.
    GiveUp { tries: i64 },
    /// Leave it as it is: outside the hours, or the board stopped trying.
    Wait,
}

/// The tries that count now: none once the last was `FRESH_SECS` ago.
pub fn tries(s: &State, now: f64) -> i64 {
    match s.at {
        Some(at) if now - at < FRESH_SECS => s.tries,
        _ => 0,
    }
}

/// What the board does with a task that would go, or is, on Needs you, with the work hours `open` or not.
pub fn step(s: &State, open: bool, now: f64) -> Step {
    if s.held || !open {
        return Step::Wait;
    }
    let tries = tries(s, now);
    match WAITS.get(tries as usize) {
        Some(&wait) => Step::Retry { n: tries + 1, wait },
        None => Step::GiveUp { tries },
    }
}

/// Whether a task off Needs you forgets its tries: it left after the board stopped trying (Morgan
/// acted on it), or its last try was long enough ago that a new failure starts over.
pub fn forget(s: &State, now: f64) -> bool {
    s.held || tries(s, now) == 0
}

/// What `try_again` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Queued to start again; it's off Needs you.
    Retried,
    /// The last try failed after this many: it goes on Needs you, saying so (`gave_up_line`).
    GaveUp(i64),
    /// Not now: it goes on Needs you as it would have.
    No,
}

/// Where task `t` would go on Needs you as `kind`, `why` being what its item would say: during the
/// work hours, queue it to start again after the next wait. In a transaction.
pub fn try_again(app: &App, t: &Row, kind: Kind, why: &str) -> Result<Outcome> {
    let s = State::of(t);
    match step(&s, hours::may_start(app), now_ts()) {
        Step::Wait => Ok(Outcome::No),
        Step::GiveUp { tries } => {
            save(app, t.id(), json!({"tries": tries, "at": s.at.map(iso), "held": GAVE_UP}))?;
            app.info(format!("task {} {}: gave up after {tries} tries", rf("task", t.id()), word(kind)));
            Ok(Outcome::GaveUp(tries))
        }
        Step::Retry { n, wait } => {
            requeue(app, t, kind, n, wait, why)?;
            Ok(Outcome::Retried)
        }
    }
}

/// Whether `try_again` would queue task `t` to start again now.
pub fn will_retry(app: &App, t: &Row) -> bool {
    matches!(step(&State::of(t), hours::may_start(app), now_ts()), Step::Retry { .. })
}

/// A start that isn't tried again (stopped by a hook, refused by Morgan, nothing to start from): the
/// board leaves it on Needs you.
pub fn not_retried(app: &App, id: i64) -> Result<()> {
    save(app, id, json!({"tries": 0, "at": now_iso(), "held": NOT_RETRIED}))
}

/// What an item says after the board's last try failed.
pub fn gave_up_line(why: &str, kind: Kind, tries: i64) -> String {
    let did = match kind {
        Kind::Start => "tried to start it",
        Kind::Lost => "started it again from its handoff",
    };
    format!("{}. The board {did} {} during work hours, and it failed each time, so it's waiting for you.", trim(why), times(tries))
}

fn times(n: i64) -> String {
    match n {
        1 => "once".into(),
        2 => "twice".into(),
        n => format!("{n} times"),
    }
}

fn trim(why: &str) -> &str {
    why.trim().trim_end_matches('.')
}

fn word(kind: Kind) -> &'static str {
    match kind {
        Kind::Start => "couldn't start",
        Kind::Lost => "lost its terminal",
    }
}

/// The `item` of its Handled for you record.
pub fn item(kind: Kind) -> &'static str {
    match kind {
        Kind::Start => "start",
        Kind::Lost => "lost",
    }
}

/// Keeps `needs_retry` without touching `updated_at`: the card's times read it.
fn save(app: &App, id: i64, v: Value) -> Result<()> {
    app.db.update("tasks", &json!(id), fields!["needs_retry" => jdumps(&v)]).map(|_| ())
}

fn requeue(app: &App, t: &Row, kind: Kind, n: i64, wait: i64, why: &str) -> Result<()> {
    let at = iso(now_ts() + wait as f64);
    let when = local_clock(Some(&at));
    let of = WAITS.len();
    let mut f = match kind {
        // As `tb start`, after the quick retries: a failure of this try comes straight back here.
        Kind::Start => {
            let pickup = if t.s("pickup") == Some("manual") { "queue".to_string() } else { t.st("pickup") };
            fields!["status" => "queued", "needs_reason" => null, "start_job" => null, "pickup" => pickup,
                    "start_tries" => crate::jobs::START_RETRY_WAITS.len() as i64]
        }
        // As `board::mark_lost` queues a lost task to start again from its handoff.
        Kind::Lost => {
            let pickup = if matches!(t.s("pickup"), Some("manual") | Some("attach")) { "new".to_string() } else { t.st("pickup") };
            fields!["status" => "queued", "lost" => 0, "needs_reason" => null, "session_id" => null, "start_job" => null,
                    "pickup" => pickup, "pickup_session" => null]
        }
    };
    f.extend(fields![
        "retry_at" => at.clone(),
        "latest" => format!("{}. The board tries it again at {when} ({n} of {of}).", trim(why)),
        "needs_retry" => jdumps(&json!({"tries": n, "at": now_iso()})),
    ]);
    board::update_task(app, t.id(), f)?;
    // Its item is off Needs you while it waits, and so is the alert that came with it.
    dispatch::clear_alerts(app, Some(t.id()), None)?;
    let what = match kind {
        Kind::Start => "start it",
        Kind::Lost => "start it from its handoff in a new terminal",
    };
    // Logged as Handled for you: Pull back puts it on Needs you again as it was (`reason`, `was`).
    let reason = match kind {
        Kind::Start => "start_failed",
        Kind::Lost => "lost",
    };
    handled::record(
        app,
        t.id(),
        "retry",
        &format!("Tries again on its own at {when} to {what} ({n} of {of})"),
        json!({"item": item(kind), "verdict": "retry", "reason": reason, "was": trim(why), "try": n, "of": of, "retry_at": at}),
    )?;
    app.info(format!("task {} {}: retry {n} of {of} at {when}", rf("task", t.id()), word(kind)));
    Ok(())
}

/// A Couldn't start item from before the board kept `needs_retry` that shouldn't be tried again.
fn refused(latest: &str) -> bool {
    latest.starts_with("Stopped from starting") || (latest.starts_with("Midna refused") && crate::jobs::you_said_no(latest))
}

/// Which kind of retry a task on Needs you gets, if any.
fn kind_of(app: &App, t: &Row) -> Result<Option<Kind>> {
    if t.s("status") != Some("needs") {
        return Ok(None);
    }
    match t.s("needs_reason") {
        Some("start_failed") if !refused(&t.st("latest")) => Ok(Some(Kind::Start)),
        Some("lost") if t.b("lost") => {
            let asked = t.s("question").is_some_and(|q| !q.trim().is_empty());
            let found = !crate::lost_done::card(t).is_null();
            Ok((!asked && !found && !board::closed_by_hand(app, t)?).then_some(Kind::Lost))
        }
        _ => Ok(None),
    }
}

/// Each runner tick: forgets the tries of tasks that left Needs you, and during the work hours tries
/// again the Couldn't start and Terminal lost items already on it (from before the hours opened, or from
/// before the board tried them on its own).
pub fn sweep(app: &App) -> Result<()> {
    let now = now_ts();
    for t in app.db.q("SELECT id, needs_retry FROM tasks WHERE needs_retry IS NOT NULL AND status != 'needs'", p![])? {
        if forget(&State::of(&t), now) {
            app.db.update("tasks", &json!(t.id()), fields!["needs_retry" => null])?;
        }
    }
    if !hours::may_start(app) {
        return Ok(());
    }
    for t in app.db.q("SELECT * FROM tasks WHERE status = 'needs' AND needs_reason IN ('start_failed', 'lost')", p![])? {
        let Some(kind) = kind_of(app, &t)? else { continue };
        if let Outcome::GaveUp(n) = try_again(app, &t, kind, &t.st("latest"))? {
            let line = gave_up_line(&t.st("latest"), kind, n);
            app.db.update("tasks", &json!(t.id()), fields!["latest" => line.clone()])?;
            board::log_event(app, t.id(), board::BOARD, "midna", &line)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_800_000_000.0;

    fn st(tries: i64, ago: Option<f64>, held: bool) -> State {
        State { tries, at: ago.map(|a| NOW - a), held }
    }

    #[test]
    fn tries_wait_longer_each_time_during_the_hours() {
        assert_eq!(step(&State::default(), true, NOW), Step::Retry { n: 1, wait: 600 });
        assert_eq!(step(&st(1, Some(700.0), false), true, NOW), Step::Retry { n: 2, wait: 1800 });
        assert_eq!(step(&st(2, Some(1900.0), false), true, NOW), Step::Retry { n: 3, wait: 3600 });
    }

    #[test]
    fn it_gives_up_after_the_last_try() {
        assert_eq!(step(&st(3, Some(3700.0), false), true, NOW), Step::GiveUp { tries: 3 });
    }

    #[test]
    fn nothing_is_tried_outside_the_hours() {
        assert_eq!(step(&State::default(), false, NOW), Step::Wait);
        assert_eq!(step(&st(3, Some(60.0), false), false, NOW), Step::Wait);
    }

    #[test]
    fn a_held_task_isnt_tried_again() {
        assert_eq!(step(&st(3, Some(60.0), true), true, NOW), Step::Wait);
        // However long it has waited on Needs you: only leaving it lets the board try again.
        assert_eq!(step(&st(3, Some(FRESH_SECS * 10.0), true), true, NOW), Step::Wait);
    }

    #[test]
    fn tries_start_over_long_after_the_last() {
        assert_eq!(step(&st(3, Some(FRESH_SECS + 1.0), false), true, NOW), Step::Retry { n: 1, wait: 600 });
        assert_eq!(step(&st(2, Some(FRESH_SECS - 1.0), false), true, NOW), Step::Retry { n: 3, wait: 3600 });
        assert_eq!(tries(&st(2, None, false), NOW), 0);
    }

    #[test]
    fn tries_are_forgotten_once_held_or_old() {
        assert!(forget(&st(3, Some(60.0), true), NOW));
        assert!(forget(&st(1, Some(FRESH_SECS + 1.0), false), NOW));
        // A task the board just queued again keeps its tries, so a crash loop still runs out of them.
        assert!(!forget(&st(1, Some(60.0), false), NOW));
    }

    #[test]
    fn the_item_says_why_once_the_board_gives_up() {
        assert_eq!(
            gave_up_line("Couldn't start: git fetch failed.", Kind::Start, 3),
            "Couldn't start: git fetch failed. The board tried to start it 3 times during work hours, and it failed each time, so it's waiting for you."
        );
        assert!(gave_up_line("The terminal closed at 1:05 PM.", Kind::Lost, 1).contains("started it again from its handoff once"));
    }

    #[test]
    fn a_start_refused_before_the_board_kept_tries_isnt_tried_again() {
        assert!(refused("Stopped from starting by hook `x`: no"));
        assert!(refused("Midna refused: denied by user"));
        assert!(!refused("Midna refused: no session s1"));
        assert!(!refused("Couldn't start: git fetch failed"));
    }
}
