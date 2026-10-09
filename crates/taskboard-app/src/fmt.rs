//! Text helpers ported from the web board (`app.js`): refs, relative times, durations, clocks,
//! and reading fields out of the API's JSON.
use chrono::{DateTime, FixedOffset, NaiveDateTime, Utc};
#[cfg(test)]
use chrono::{Offset, TimeZone};
#[cfg(not(test))]
use chrono::Local;
use serde_json::Value;
use std::sync::atomic::{AtomicI64, Ordering};

/// Server clock minus ours, in ms (from `state.now`), so "5m ago" matches the board.
static CLOCK_OFFSET_MS: AtomicI64 = AtomicI64::new(0);

pub fn set_server_now(iso: &str) {
    if let Some(t) = parse(iso) {
        CLOCK_OFFSET_MS.store((t - Utc::now()).num_milliseconds(), Ordering::Relaxed);
    }
}

pub fn now() -> DateTime<Utc> {
    #[cfg(test)]
    if let Some(t) = crate::parity::frozen_now() {
        return t;
    }
    Utc::now() + chrono::Duration::milliseconds(CLOCK_OFFSET_MS.load(Ordering::Relaxed))
}

/// A time in the reader's zone (with that moment's offset, so DST is right). Tests run in UTC,
/// as the parity goldens do, so local-time text is stable.
pub fn local(t: DateTime<Utc>) -> DateTime<FixedOffset> {
    #[cfg(test)]
    return t.with_timezone(&Utc.fix());
    #[cfg(not(test))]
    t.with_timezone(&Local).fixed_offset()
}

/// A wall-clock time without a zone (`work_hours.next_open`) read in the reader's zone.
fn from_local(n: NaiveDateTime) -> Option<DateTime<Utc>> {
    #[cfg(test)]
    return Some(Utc.from_utc_datetime(&n));
    #[cfg(not(test))]
    n.and_local_timezone(Local).single().map(|t| t.with_timezone(&Utc))
}

/// Same calendar day as now, in the reader's zone.
pub fn is_today(t: DateTime<Utc>) -> bool {
    local(t).date_naive() == local(now()).date_naive()
}

/// An ISO time from the API (UTC with `Z`, an offset, or naive local like `next_open`).
pub fn parse(iso: &str) -> Option<DateTime<Utc>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(iso) {
        return Some(t.with_timezone(&Utc));
    }
    for f in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(n) = NaiveDateTime::parse_from_str(iso, f) {
            return from_local(n);
        }
    }
    None
}

fn minutes_since(iso: &str) -> Option<i64> {
    parse(iso).map(|t| ((now() - t).num_seconds() as f64 / 60.).round() as i64)
}

/// "just now", "5m ago", "3h ago", "2d ago".
pub fn ago(iso: &str) -> String {
    let Some(m) = minutes_since(iso) else { return String::new() };
    if m < 1 {
        "just now".into()
    } else if m < 60 {
        format!("{m}m ago")
    } else if m < 60 * 24 {
        format!("{}h ago", (m as f64 / 60.).round() as i64)
    } else {
        format!("{}d ago", (m as f64 / 1440.).round() as i64)
    }
}

/// A time today as "3:05 PM", any other day as "Oct 7" (the web's `hhmm`).
pub fn hhmm(iso: &str) -> String {
    let Some(t) = parse(iso) else { return String::new() };
    let l = local(t);
    if is_today(t) { l.format("%-I:%M %p").to_string() } else { l.format("%b %-d").to_string() }
}

/// "10/7/2026, 3:05:00 PM" (the web's `fullTime`: `toLocaleString()` in en-US).
pub fn full_time(iso: &str) -> String {
    parse(iso).map(|t| local(t).format("%-m/%-d/%Y, %-I:%M:%S %p").to_string()).unwrap_or_default()
}

/// A duration in minutes: "under a minute", "12m", "2h 5m", "1d 3h".
pub fn span(m: i64) -> String {
    if m < 1 {
        "under a minute".into()
    } else if m < 60 {
        format!("{m}m")
    } else if m < 60 * 24 {
        if m % 60 > 0 { format!("{}h {}m", m / 60, m % 60) } else { format!("{}h", m / 60) }
    } else {
        format!("{}d {}h", m / 1440, ((m % 1440) as f64 / 60.).round() as i64)
    }
}

/// "running 2h 5m" from a task's `started_at`.
pub fn running_for(t: &Value) -> String {
    match minutes_since(s(t, "started_at")) {
        Some(m) if m >= 0 => format!("running {}", span(m)),
        _ => String::new(),
    }
}

/// "Took 1h" on a done task.
pub fn took_line(t: &Value) -> String {
    let (Some(a), Some(b)) = (parse(s(t, "started_at")), parse(s(t, "finished_at"))) else { return String::new() };
    let m = ((b - a).num_seconds() as f64 / 60.).round() as i64;
    if s(t, "status") != "done" || m < 0 { String::new() } else { format!("Took {}", span(m)) }
}

/// "Added 5m ago" / "Updated …" / "Asked …" / "Lost …" / "Finished …" / "Stopped …".
pub fn when_line(t: &Value) -> String {
    let w = opt_s(t, "when").or(opt_s(t, "updated_at")).unwrap_or("");
    let a = ago(w);
    if a.is_empty() {
        return a;
    }
    let verb = match s(t, "status") {
        "queued" | "planned" => "Added",
        "working" => "Updated",
        "needs" if b(t, "lost") => "Lost",
        "needs" if opt_s(t, "question").is_some() => "Asked",
        "needs" => "Updated",
        _ if b(t, "failed") => "Stopped",
        _ => "Finished",
    };
    format!("{verb} {a}")
}

/// "09:30" → "9:30am", "15:00" → "3pm".
pub fn clock12(hhmm: &str) -> String {
    let mut it = hhmm.split(':').map(|p| p.parse::<u32>().unwrap_or(0));
    let (h, m) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    let mm = if m > 0 { format!(":{m:02}") } else { String::new() };
    format!("{h12}{mm}{}", if h < 12 { "am" } else { "pm" })
}

/// A time as "3pm" today or "Thu 6am" another day.
pub fn day_clock(iso: &str) -> String {
    let Some(t) = parse(iso) else { return String::new() };
    let l = local(t);
    let day = if is_today(t) { String::new() } else { format!("{} ", l.format("%a")) };
    format!("{day}{}", clock12(&l.format("%H:%M").to_string()))
}

/// "Compacting since 3:05 PM" while a terminal (or a task's terminal) is compacting its conversation.
pub fn compacting(v: &Value) -> Option<String> {
    opt_s(v, "compacting").map(|at| format!("Compacting since {}", hhmm(at)))
}

pub fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// An issue or task that came in from the external PR feed (the Review log): source `review_log` (or `review`).
pub fn from_review_log(source: &str) -> bool {
    matches!(source, "review_log" | "review")
}

pub fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// `T12` for a task object or id, `G3` for a goal, `B7` for an issue.
pub fn ref_of(v: &Value, prefix: &str) -> String {
    if let Some(r) = v.get("ref").and_then(Value::as_str) {
        return r.to_string();
    }
    match v.get("id").unwrap_or(v) {
        Value::Number(n) => format!("{prefix}{n}"),
        Value::String(s) if !s.is_empty() => {
            if s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) { s.to_uppercase() } else { format!("{prefix}{s}") }
        }
        _ => String::new(),
    }
}

// ------------------------------------------------------------------ JSON field access

/// A string field, "" when missing or null.
pub fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// A non-empty string field.
pub fn opt_s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// A bool field (also 0/1).
pub fn b(v: &Value, k: &str) -> bool {
    match v.get(k) {
        Some(Value::Bool(x)) => *x,
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        _ => false,
    }
}

pub fn i(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(Value::as_i64).unwrap_or(0)
}

/// An array field, empty when missing.
pub fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

/// An object field that isn't null.
pub fn obj<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.get(k).filter(|x| x.is_object())
}


/// The reader's own clock, without the board's skew correction (the web's bare `Date.now()`).
pub fn device_now() -> DateTime<Utc> {
    #[cfg(test)]
    if let Some(t) = crate::parity::frozen_now() {
        return t;
    }
    Utc::now()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn clocks_and_spans() {
        assert_eq!(clock12("09:30"), "9:30am");
        assert_eq!(clock12("15:00"), "3pm");
        assert_eq!(clock12("00:00"), "12am");
        assert_eq!(span(0), "under a minute");
        assert_eq!(span(125), "2h 5m");
        assert_eq!(span(120), "2h");
    }

    #[test]
    fn refs() {
        assert_eq!(ref_of(&json!({"id": 4}), "T"), "T4");
        assert_eq!(ref_of(&json!({"ref": "G2", "id": 2}), "G"), "G2");
        assert_eq!(ref_of(&json!(7), "B"), "B7");
        assert_eq!(ref_of(&json!("t9"), "T"), "T9");
    }
}
