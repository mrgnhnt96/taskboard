//! Starts at a set time (`tb start T8 --at 5pm`, the app's Start at…), whatever the work hours say.
//!
//! A task scheduled to start waits in the queue with `start_at` (a local minute, `2026-11-04T17:00`);
//! once that minute comes, the runner starts it outside the work hours too and ahead of its goal's
//! waves, as the owner's Start would. `start_was` keeps the status and pickup it had, so cancelling puts
//! it back. A goal's `start_at` runs it at that minute as Start now does.

use chrono::NaiveDateTime;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::board::{self, OWNER};
use crate::{fields, hours, p};
use crate::util::*;

pub const ADDED: &[(&str, &str, &str)] = &[("tasks", "start_at", "TEXT"), ("tasks", "start_was", "TEXT"), ("goals", "start_at", "TEXT")];

/// Whether `v` asks to drop a schedule rather than set one.
pub fn is_off(v: &str) -> bool {
    matches!(v.trim().to_lowercase().as_str(), "off" | "none" | "cancel" | "clear")
}

/// The next local minute it's `v`: `5pm`, `17:30`, `tomorrow 9am`, or a minute `2026-11-04T17:00`.
pub fn parse_at(v: &str, now: &NaiveDateTime) -> Result<NaiveDateTime> {
    let s = v.trim();
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
        if dt <= *now {
            return err(400, "That time has already passed.");
        }
        return Ok(dt);
    }
    hours::next_time(now, &spoken(s)).map_err(|_| ApiError::new(400, "The start time should look like 5pm, 17:30 or tomorrow 9am."))
}

static DOTTED_MERIDIEM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s*\b([ap])\.?\s?m\b\.?").unwrap());
static FILLER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b(?:at|for|by|on|around|about|today|tmrw|tmr|noon|midnight)\b").unwrap());

/// A time as the owner says it ("tomorrow at 6 a.m", "tomorrow for 6am", "tmrw 6 AM", "noon") in the
/// form [`hours::next_time`] reads ("tomorrow 6am", "12pm").
pub fn spoken(v: &str) -> String {
    let s = v.trim().to_lowercase();
    let s = DOTTED_MERIDIEM_RE.replace_all(&s, "${1}m");
    let s = FILLER_RE.replace_all(&s, |c: &regex::Captures| match &c[0] {
        "tmrw" | "tmr" => "tomorrow",
        "noon" => "12pm",
        "midnight" => "12am",
        _ => "",
    });
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a task waits for its scheduled minute.
pub fn waiting(t: &Row) -> bool {
    t.s("start_at").is_some_and(|at| !at.is_empty() && !hours::reached(Some(at)))
}

/// Whether a task's scheduled minute has come: it starts now, whatever the hours and its waves say.
pub fn due(t: &Row) -> bool {
    t.s("start_at").is_some_and(|at| !at.is_empty() && hours::reached(Some(at)))
}

/// "Starts at 5pm" / "Starts tomorrow 9am".
pub fn line(at: &str) -> String {
    format!("Starts {}", hours::at_time(&hours::say_when(at)))
}

/// Schedules task `id` to start at `at`. Call inside a transaction.
pub fn schedule_task(app: &App, id: i64, at: &NaiveDateTime, by: &str) -> Result<()> {
    let t = board::get_task(app, id)?;
    let status = t.st("status");
    if status == "done" {
        return err(409, "This task is done. Queue it again first.");
    }
    if matches!(status.as_str(), "working" | "needs") && board::live_session(app, &t)?.is_some() && !t.b("lost") {
        return err(409, format!("It's already on {}.", t.s("session_name").unwrap_or("a terminal")));
    }
    if t.s("line_session").is_some() && status == "queued" {
        crate::lines::leave(app, &t)?;
    }
    let was = match t.s("start_at").filter(|s| !s.is_empty()) {
        Some(_) => t.v("start_was"),
        None => json!(format!("{status}:{}", t.st("pickup"))),
    };
    let iso = hours::minutes_iso(at);
    board::update_task(
        app,
        id,
        fields!["status" => "queued", "pickup" => "new", "pickup_session" => null, "start_at" => iso.clone(), "start_was" => was,
                "needs_reason" => null, "question" => null, "lost" => 0, "start_tries" => 0, "retry_at" => null,
                "latest" => format!("{}, whatever the work hours.", line(&iso))],
    )?;
    board::log_event(app, id, OWNER, "status", &format!("{by}; it starts {}, whatever the work hours", hours::at_time(&hours::say_when(&iso))))?;
    Ok(())
}

/// Drops task `id`'s schedule, back to the status it had. Call inside a transaction.
pub fn unschedule_task(app: &App, id: i64, by: &str) -> Result<()> {
    let t = board::get_task(app, id)?;
    if !waiting(&t) {
        return err(409, format!("{} isn't scheduled to start.", rf("task", id)));
    }
    let was = t.st("start_was");
    let (status, pickup) = was.split_once(':').unwrap_or(("", ""));
    let (status, pickup) = match status {
        "planned" | "queued" => (status, if pickup.is_empty() { "manual" } else { pickup }),
        _ => ("queued", "manual"),
    };
    board::update_task(app, id, fields!["status" => status, "pickup" => pickup, "start_at" => null, "start_was" => null, "latest" => null])?;
    board::log_event(app, id, OWNER, "status", &format!("{by}; its scheduled start is off"))?;
    Ok(())
}

/// Clears a task's schedule once it starts some other way (the owner's Start) or on time.
pub fn started(app: &App, id: i64) -> Result<()> {
    app.db.x("UPDATE tasks SET start_at = NULL, start_was = NULL WHERE id = ? AND start_at IS NOT NULL", p![id])?;
    Ok(())
}

/// Schedules goal `id` to run at `at`, as Start now would then. Call inside a transaction.
pub fn schedule_goal(app: &App, id: i64, at: &NaiveDateTime, by: &str) -> Result<()> {
    let g = board::get_goal(app, id)?;
    if !board::goal_tasks(app, id)?.iter().any(|t| t.s("status") == Some("planned")) {
        return err(409, format!("{} has no planned tasks to start.", rf("goal", id)));
    }
    let iso = hours::minutes_iso(at);
    board::update_goal(app, g.id(), fields!["start_at" => iso.clone(), "updated_at" => now_iso()])?;
    let text = format!("{by}; it starts {}, whatever the work hours", hours::at_time(&hours::say_when(&iso)));
    board::add_goal_note(app, id, "decision", &text, None, false, None)?;
    Ok(())
}

/// Drops goal `id`'s schedule. Call inside a transaction.
pub fn unschedule_goal(app: &App, id: i64, by: &str) -> Result<()> {
    let g = board::get_goal(app, id)?;
    if g.s("start_at").filter(|s| !s.is_empty()).is_none() {
        return err(409, format!("{} isn't scheduled to start.", rf("goal", id)));
    }
    board::update_goal(app, id, fields!["start_at" => null, "updated_at" => now_iso()])?;
    board::add_goal_note(app, id, "decision", &format!("{by}; its scheduled start is off"), None, false, None)?;
    Ok(())
}

/// Runs the goals whose scheduled minute has come. Tasks are started by the runner (`start_queued`).
pub fn tick(app: &App) -> Result<()> {
    let now = hours::minutes_iso(&hours::now_local());
    for g in app.db.q("SELECT id FROM goals WHERE start_at IS NOT NULL AND start_at != '' AND start_at <= ?", p![now])? {
        let id = g.id();
        app.db.tx(|| app.db.update("goals", &json!(id), fields!["start_at" => null]).map(|_| ()))?;
        if let Err(e) = crate::api::run_goal_on_schedule(app, id) {
            app.info(format!("scheduled: couldn't run {}: {}", rf("goal", id), e.message));
        }
    }
    Ok(())
}

/// The goal's card fields: its scheduled minute.
pub fn goal_card(g: &Row) -> Value {
    g.v("start_at")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").unwrap()
    }

    #[test]
    fn a_time_is_the_next_time_it_comes() {
        let now = at("2026-11-03T14:10");
        assert_eq!(parse_at("5pm", &now).unwrap(), at("2026-11-03T17:00"));
        assert_eq!(parse_at("9am", &now).unwrap(), at("2026-11-04T09:00"));
        assert_eq!(parse_at("tomorrow 5pm", &now).unwrap(), at("2026-11-04T17:00"));
        assert_eq!(parse_at("2026-11-05T08:30", &now).unwrap(), at("2026-11-05T08:30"));
    }

    #[test]
    fn the_owners_spellings_of_a_time_are_read() {
        let now = at("2026-11-09T15:25");
        let six = at("2026-11-10T06:00");
        for v in ["tomorrow at 6 a.m", "tomorrow at 6 a.m.", "tomorrow for 6am", "tomorrow 6am", "Tomorrow 6 AM", "tmrw 6am", "6am", "6 a.m.", "at 6am"] {
            assert_eq!(parse_at(v, &now).unwrap(), six, "{v}");
        }
        assert_eq!(parse_at("tomorrow at noon", &now).unwrap(), at("2026-11-10T12:00"));
        assert_eq!(parse_at("5:30 p.m.", &now).unwrap(), at("2026-11-09T17:30"));
        assert_eq!(spoken("tomorrow at 6 a.m"), "tomorrow 6am");
    }

    #[test]
    fn a_past_minute_or_nonsense_is_refused() {
        let now = at("2026-11-03T14:10");
        assert!(parse_at("2026-11-03T09:00", &now).is_err());
        assert!(parse_at("soonish", &now).is_err());
    }

    #[test]
    fn off_words_drop_a_schedule() {
        assert!(is_off("off") && is_off(" Cancel ") && !is_off("5pm"));
    }
}
