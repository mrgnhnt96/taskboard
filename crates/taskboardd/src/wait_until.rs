//! A working task that parks itself until a set time (`tb wait-until 6:30am --then "…"`), such as for a
//! nightly build: its agent ends the turn with something of its own set to wake it then (a trigger, a
//! scheduled wake-up). The board records when it resumes (`waiting_until`, an instant) and what it does
//! next (`resume_plan`), keeps it off Needs you while it waits, and doesn't close its idle terminal
//! after hours. The next turn on its terminal clears the wait. When the time passes by [`GRACE_SECS`]
//! with no turn, the task needs the owner, naming the time it missed.

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use serde_json::{json, Value};

use crate::app::App;
use crate::board;
use crate::util::*;
use crate::{fields, hours, p};

pub const ADDED: &[(&str, &str, &str)] = &[("tasks", "waiting_until", "TEXT"), ("tasks", "resume_plan", "TEXT")];

/// How long past its resume time a waiting task may go with no turn before it needs the owner.
pub const GRACE_SECS: f64 = 15.0 * 60.0;

/// Why a task that missed its resume time needs the owner. Any turn on its terminal clears it.
pub const OVERDUE: &str = "wait_overdue";

/// Whether working task `t` is parked until a set time.
pub fn waiting(t: &Row) -> bool {
    has(t.s("waiting_until")) && t.s("status") == Some("working") && !t.b("lost")
}

/// The instant (seconds) `v` names, after `now`: a time as `scheduled::parse_at` reads it ("6:30am",
/// "tomorrow 9am", "2026-11-24T06:30"), the same in UTC ("06:30 UTC", "6:30am GMT"), a full RFC 3339
/// instant, or a span from now ("in 40m", "2h").
pub fn parse(v: &str, now: f64) -> Result<f64> {
    let s = v.trim();
    let bad = || ApiError::new(400, "The time should look like 6:30am, 06:30 UTC, tomorrow 9am or in 40m.");
    if s.is_empty() {
        return Err(bad());
    }
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        let at = d.timestamp() as f64;
        return if at > now { Ok(at) } else { err(400, "That time has already passed.") };
    }
    if let Some(secs) = span_secs(s) {
        return Ok(now + secs);
    }
    let lower = s.to_lowercase();
    let utc = ["utc", "gmt", "z"].iter().find_map(|z| lower.strip_suffix(z).filter(|rest| rest.ends_with(|c: char| c.is_ascii_digit() || c == ' ' || c == 'm')));
    let now_dt = Utc.timestamp_opt(now.floor() as i64, 0).single().ok_or_else(bad)?;
    let at = match utc {
        Some(rest) => {
            let base = now_dt.naive_utc();
            let at = crate::scheduled::parse_at(rest.trim(), &base).map_err(|e| if e.message.contains("passed") { e } else { bad() })?;
            Utc.from_utc_datetime(&at).timestamp()
        }
        None => {
            let base = now_dt.with_timezone(&Local).naive_local();
            let at: NaiveDateTime = crate::scheduled::parse_at(s, &base).map_err(|e| if e.message.contains("passed") { e } else { bad() })?;
            Local.from_local_datetime(&at).earliest().ok_or_else(bad)?.timestamp()
        }
    };
    Ok(at as f64)
}

/// "in 40m", "2h", "1h30m", "90 minutes": its seconds.
fn span_secs(s: &str) -> Option<f64> {
    let s = s.to_lowercase();
    let s = s.strip_prefix("in ").unwrap_or(&s).replace(' ', "");
    let mut total = 0.0;
    let mut num = String::new();
    let mut rest = s.as_str();
    let mut any = false;
    while !rest.is_empty() {
        let c = rest.chars().next()?;
        if c.is_ascii_digit() {
            num.push(c);
            rest = &rest[1..];
            continue;
        }
        let unit_len = rest.find(|c: char| c.is_ascii_digit()).unwrap_or(rest.len());
        let unit = &rest[..unit_len];
        let n: f64 = num.parse().ok()?;
        let per = match unit {
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
            _ => return None,
        };
        total += n * per;
        num.clear();
        any = true;
        rest = &rest[unit_len..];
    }
    (any && num.is_empty() && total > 0.0).then_some(total)
}

/// When `at` (an ISO instant) comes, as the owner reads it: "6:30 AM", "tomorrow 6:30 AM".
pub fn say(at: &str) -> String {
    hours::say_reset(at)
}

/// "Waiting · resumes 6:30 AM".
pub fn line(at: &str) -> String {
    format!("Waiting · resumes {}", say(at))
}

/// Parks working task `t` until `at` (seconds), then to do `plan`. Call inside a transaction.
pub fn park(app: &App, t: &Row, at: f64, plan: &str, who: &str) -> Result<String> {
    let iso = iso(at);
    let when = say(&iso);
    board::update_task(app, t.id(), fields!["waiting_until" => iso.clone(), "resume_plan" => plan, "latest" => format!("{}: {plan}", line(&iso))])?;
    board::log_event(app, t.id(), who, "status", &format!("Waits until {when}, then: {plan}"))?;
    Ok(iso)
}

/// Drops working task `t`'s wait: a turn came on its terminal, or it said `tb wait-until none`. Call
/// inside a transaction. Whether it was waiting.
pub fn resume(app: &App, t: &Row, who: &str, why: &str) -> Result<bool> {
    if !has(t.s("waiting_until")) {
        return Ok(false);
    }
    app.db.x("UPDATE tasks SET waiting_until = NULL, resume_plan = NULL WHERE id = ?", p![t.id()])?;
    if t.s("status") == Some("working") {
        board::update_task(app, t.id(), fields!["latest" => null])?;
        board::log_event(app, t.id(), who, "status", why)?;
    }
    Ok(true)
}

/// The text a task that missed its resume time asks the owner with.
pub fn overdue_text(t: &Row, at: &str) -> String {
    let plan = one_line(&t.st("resume_plan"), 300);
    let mut text = format!("{} was due back at {} and hasn't resumed", rf("task", t.id()), hours::clock(&local_hhmm(Some(at))));
    if !plan.is_empty() {
        text.push_str(&format!(" (it was going to: {plan})"));
    }
    text
}

/// The waiting tasks whose resume time passed [`GRACE_SECS`] ago with no turn need the owner; a wait
/// left on a task that isn't working any more is dropped. Call inside a transaction.
pub fn sweep(app: &App) -> Result<()> {
    let now = now_ts();
    for t in app.db.q("SELECT * FROM tasks WHERE waiting_until IS NOT NULL AND waiting_until != ''", p![])? {
        if t.s("status") != Some("working") {
            app.db.x("UPDATE tasks SET waiting_until = NULL, resume_plan = NULL WHERE id = ?", p![t.id()])?;
            continue;
        }
        if t.b("lost") {
            continue;
        }
        let at = t.st("waiting_until");
        if parse_iso(&at).is_none_or(|ts| now < ts + GRACE_SECS) {
            continue;
        }
        let ask = overdue_text(&t, &at);
        app.db.x("UPDATE tasks SET waiting_until = NULL, resume_plan = NULL WHERE id = ?", p![t.id()])?;
        board::update_task(app, t.id(), fields!["status" => "needs", "needs_reason" => OVERDUE, "question" => ask.clone(), "answered_at" => null])?;
        board::log_event(app, t.id(), board::BOARD, "question", &ask)?;
    }
    Ok(())
}

/// The card's fields: when it resumes and what it does then, while it waits.
pub fn card(t: &Row) -> (Value, Value) {
    if waiting(t) {
        (t.v("waiting_until"), t.v("resume_plan"))
    } else {
        (Value::Null, Value::Null)
    }
}

/// What `tb wait-until` tells the agent once its task waits.
pub fn parked_text(app: &App, t: &Row, at: &str) -> String {
    let due = parse_iso(at).map(|ts| iso(ts + GRACE_SECS)).unwrap_or_default();
    format!(
        "{} waits until {} ({}). End your turn now. The board doesn't wake this conversation: whatever you set to \
         wake it then (a trigger, a scheduled wake-up) must. The next turn here clears the wait; if none comes by {}, \
         the board puts {} on {}.",
        rf("task", t.id()),
        say(at),
        at,
        hours::clock(&local_hhmm(Some(&due))),
        rf("task", t.id()),
        app.cfg.owner,
    )
}

/// The JSON a `tb wait-until` answers with.
pub fn answer(t: &Row, at: Option<&str>) -> Value {
    json!({"waiting_until": at, "resume_plan": at.map(|_| t.v("resume_plan")).unwrap_or(Value::Null)})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> f64 {
        DateTime::parse_from_rfc3339(s).unwrap().timestamp() as f64
    }

    #[test]
    fn a_utc_time_is_the_next_time_it_comes_in_utc() {
        let now = ts("2026-11-23T20:00:00Z");
        assert_eq!(parse("06:30 UTC", now).unwrap(), ts("2026-11-24T06:30:00Z"));
        assert_eq!(parse("6:30am utc", now).unwrap(), ts("2026-11-24T06:30:00Z"));
        assert_eq!(parse("21:15 GMT", now).unwrap(), ts("2026-11-23T21:15:00Z"));
        assert_eq!(parse("2026-11-24T06:30:00Z", now).unwrap(), ts("2026-11-24T06:30:00Z"));
    }

    #[test]
    fn a_span_is_from_now() {
        let now = ts("2026-11-23T20:00:00Z");
        assert_eq!(parse("in 40m", now).unwrap(), now + 2400.0);
        assert_eq!(parse("2h", now).unwrap(), now + 7200.0);
        assert_eq!(parse("1h30m", now).unwrap(), now + 5400.0);
        assert_eq!(parse("in 90 minutes", now).unwrap(), now + 5400.0);
    }

    #[test]
    fn a_local_time_reads_like_a_scheduled_start() {
        let now = ts("2026-11-23T20:00:00Z");
        let base = Utc.timestamp_opt(now as i64, 0).unwrap().with_timezone(&Local).naive_local();
        let want = Local.from_local_datetime(&crate::scheduled::parse_at("tomorrow 6am", &base).unwrap()).earliest().unwrap().timestamp() as f64;
        assert_eq!(parse("tomorrow at 6 a.m.", now).unwrap(), want);
    }

    #[test]
    fn nonsense_and_the_past_are_refused() {
        let now = ts("2026-11-23T20:00:00Z");
        assert!(parse("soonish", now).is_err());
        assert!(parse("", now).is_err());
        assert!(parse("2026-11-23T06:30:00Z", now).unwrap_err().message.contains("passed"));
        assert!(span_secs("40").is_none() && span_secs("m").is_none() && span_secs("0m").is_none());
    }
}
