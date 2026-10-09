//! Work hours: when the runner may start agents. In the Mac's local time.

use chrono::{Datelike, Duration, NaiveDateTime, Timelike};
use serde_json::{json, Value};

use crate::app::App;
use crate::usage;
use crate::util::*;

const KEY: &str = "work_hours";
const TODAY_KEY: &str = "work_hours_today";
pub const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

#[derive(Debug, Clone)]
pub struct Hours {
    pub on: bool,
    pub start: String,
    pub end: String,
    pub days: Vec<String>,
    pub alert_every_mins: i64,
}

impl Hours {
    fn to_json(&self) -> Value {
        json!({"on": self.on, "start": self.start, "end": self.end, "days": self.days, "alert_every_mins": self.alert_every_mins})
    }
}

fn mins(hhmm: &str) -> i64 {
    let mut it = hhmm.split(':');
    let h: i64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let m: i64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    h * 60 + m
}

pub fn parse_time(v: &str, label: &str) -> Result<String> {
    let bad = || ApiError::new(400, format!("The {label} time should look like 06:00 or 3pm."));
    let mut s = v.trim().to_lowercase().replace('.', ":");
    let pm = s.ends_with("pm");
    let am = s.ends_with("am");
    if pm || am {
        s = s[..s.len() - 2].trim().to_string();
    }
    let (hs, ms) = match s.split_once(':') {
        Some((h, m)) => (h.to_string(), m.to_string()),
        None => (s.clone(), "0".to_string()),
    };
    let mut h: i64 = hs.trim().parse().map_err(|_| bad())?;
    let m: i64 = ms.trim().parse().map_err(|_| bad())?;
    if am || pm {
        if !(1..=12).contains(&h) {
            return Err(bad());
        }
        h = h % 12 + if pm { 12 } else { 0 };
    }
    if !(0..=23).contains(&h) || !(0..=59).contains(&m) {
        return Err(bad());
    }
    Ok(format!("{h:02}:{m:02}"))
}

pub fn parse_days(v: &Value) -> Result<Vec<String>> {
    let items: Vec<String> = match v {
        Value::String(s) => {
            let s = s.trim().to_lowercase();
            match s.as_str() {
                "all" | "every" | "everyday" | "every day" | "daily" => return Ok(DAYS.iter().map(|d| d.to_string()).collect()),
                "weekdays" | "mon-fri" => return Ok(DAYS[..5].iter().map(|d| d.to_string()).collect()),
                "weekends" | "sat-sun" => return Ok(DAYS[5..].iter().map(|d| d.to_string()).collect()),
                _ => s.replace(' ', ",").split(',').filter(|p| !p.is_empty()).map(|p| p.to_string()).collect(),
            }
        }
        Value::Array(a) => a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect(),
        _ => vec![],
    };
    let idx = |d: &str| DAYS.iter().position(|x| *x == d);
    let mut out = [false; 7];
    for p in items {
        let p = p.trim().to_lowercase();
        if let Some((a, b)) = p.split_once('-') {
            let a3: String = a.chars().take(3).collect();
            let b3: String = b.chars().take(3).collect();
            let (Some(i), Some(j)) = (idx(&a3), idx(&b3)) else {
                return err(400, format!("“{p}” isn't a range of days. Use mon, tue … sun."));
            };
            let mut k = i;
            loop {
                out[k] = true;
                if k == j {
                    break;
                }
                k = (k + 1) % 7;
            }
        } else {
            let p3: String = p.chars().take(3).collect();
            match idx(&p3) {
                Some(i) => out[i] = true,
                None => return err(400, format!("“{p}” isn't a day. Use mon, tue … sun.")),
            }
        }
    }
    let days: Vec<String> = DAYS.iter().enumerate().filter(|(i, _)| out[*i]).map(|(_, d)| d.to_string()).collect();
    if days.is_empty() {
        return err(400, "Pick at least one day.");
    }
    Ok(days)
}

pub fn get(app: &App) -> Hours {
    let d = &app.cfg.work_hours;
    let saved = jloads_obj(app.db.get_setting(KEY).ok().flatten().as_deref());
    let days_src: Vec<String> = match saved.get("days") {
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(),
        _ => d.days.clone(),
    };
    Hours {
        on: saved.get("on").and_then(|v| v.as_bool()).unwrap_or(d.on),
        start: saved.s("start").map(|s| s.to_string()).unwrap_or_else(|| d.start.clone()),
        end: saved.s("end").map(|s| s.to_string()).unwrap_or_else(|| d.end.clone()),
        days: DAYS.iter().filter(|x| days_src.iter().any(|d| d == *x)).map(|d| d.to_string()).collect(),
        alert_every_mins: saved.i("alert_every_mins").unwrap_or(d.alert_every_mins),
    }
}

pub fn set(app: &App, body: &Value) -> Result<Hours> {
    let mut h = get(app);
    if let Some(on) = body.get("on").filter(|v| !v.is_null()) {
        h.on = as_bool(Some(on), h.on);
    }
    if body_has(body, "start") {
        h.start = parse_time(&body_str(body, "start"), "start")?;
    }
    if body_has(body, "end") {
        h.end = parse_time(&body_str(body, "end"), "end")?;
    }
    if body_has(body, "days") {
        h.days = parse_days(&body["days"])?;
    }
    if body_has(body, "alert_every_mins") {
        let v = body["alert_every_mins"].as_i64().or_else(|| body_str(body, "alert_every_mins").parse().ok());
        match v {
            Some(n) => h.alert_every_mins = n.max(0),
            None => return err(400, "Give how often alerts repeat in whole minutes, or 0 for never."),
        }
    }
    if h.start == h.end {
        return err(400, "The start and end times are the same. Turn work hours off instead.");
    }
    app.db.set_setting(KEY, Some(&jdumps(&h.to_json())))?;
    Ok(h)
}

pub fn now_local() -> NaiveDateTime {
    local_now().naive_local()
}

fn date_str(d: &NaiveDateTime) -> String {
    d.format("%Y-%m-%d").to_string()
}

pub fn minutes_iso(d: &NaiveDateTime) -> String {
    d.format("%Y-%m-%dT%H:%M").to_string()
}

pub fn today_until(app: &App, now: &NaiveDateTime) -> Option<String> {
    let t = jloads_obj(app.db.get_setting(TODAY_KEY).ok().flatten().as_deref());
    if t.s("date") == Some(&date_str(now)) {
        t.s("until").map(|s| s.to_string())
    } else {
        None
    }
}

pub fn set_today_until(app: &App, v: &str) -> Result<Option<String>> {
    let now = now_local();
    let s = v.trim().to_lowercase();
    if matches!(s.as_str(), "" | "off" | "none" | "normal" | "clear") {
        app.db.set_setting(TODAY_KEY, None)?;
        return Ok(None);
    }
    if !get(app).on {
        return err(400, "Work hours are off, so agents already start any time.");
    }
    let until = parse_time(v, "end")?;
    if mins(&until) <= (now.hour() * 60 + now.minute()) as i64 {
        return err(400, format!("{} has already passed today.", clock(&until)));
    }
    app.db.set_setting(TODAY_KEY, Some(&jdumps(&json!({"date": date_str(&now), "until": until}))))?;
    Ok(Some(until))
}

/// A weekday as the board's day key (`sun`, `mon` …).
pub fn weekday_key(w: chrono::Weekday) -> &'static str {
    DAYS[w.num_days_from_monday() as usize]
}

/// The day keys in week order, starting on `first` (config.toml's `first_weekday`).
pub fn week_days(first: chrono::Weekday) -> Vec<&'static str> {
    let k = first.num_days_from_monday() as usize;
    (0..7).map(|i| DAYS[(k + i) % 7]).collect()
}

fn minute_of(dt: &NaiveDateTime) -> i64 {
    (dt.hour() * 60 + dt.minute()) as i64
}

fn weekday(dt: &NaiveDateTime) -> usize {
    dt.weekday().num_days_from_monday() as usize
}

fn in_window(h: &Hours, dt: &NaiveDateTime) -> bool {
    let (s, e, m) = (mins(&h.start), mins(&h.end), minute_of(dt));
    let day = DAYS[weekday(dt)];
    let has = |d: &str| h.days.iter().any(|x| x == d);
    if s < e {
        return has(day) && s <= m && m < e;
    }
    if m >= s {
        return has(day);
    }
    m < e && has(DAYS[(weekday(dt) + 6) % 7])
}

fn open_at(h: &Hours, until: Option<&str>, dt: &NaiveDateTime) -> bool {
    if !h.on {
        return true;
    }
    if let Some(u) = until {
        let m = minute_of(dt);
        return m < mins(u) && (in_window(h, dt) || m >= mins(&h.start));
    }
    in_window(h, dt)
}

pub fn is_open(app: &App) -> bool {
    let now = now_local();
    open_at(&get(app), today_until(app, &now).as_deref(), &now)
}

pub fn may_start(app: &App) -> bool {
    is_open(app) && usage::out_until(app).is_none()
}

/// Whether a goal's work may start now: inside the hours, or the goal was started "now" until the hours open.
pub fn goal_open(app: &App, g: Option<&Row>) -> bool {
    if usage::out_until(app).is_some() {
        return false;
    }
    if is_open(app) {
        return true;
    }
    match g.and_then(|g| g.s("hours_until")).filter(|s| !s.is_empty()) {
        Some(until) => minutes_iso(&now_local()).as_str() < until,
        None => false,
    }
}

pub fn until_open(app: &App) -> Option<String> {
    if is_open(app) {
        return None;
    }
    let now = now_local();
    let nxt = next_open(&get(app), &now, today_until(app, &now).as_deref()).unwrap_or(now + Duration::days(7));
    Some(minutes_iso(&nxt))
}

pub fn next_open(h: &Hours, now: &NaiveDateTime, until: Option<&str>) -> Option<NaiveDateTime> {
    let midnight = now.date().and_hms_opt(0, 0, 0)?;
    for n in 0..8 {
        let at = midnight + Duration::days(n) + Duration::minutes(mins(&h.start));
        if at <= *now {
            continue;
        }
        let ok = if n == 0 && until.is_some() { open_at(h, until, &at) } else { h.days.iter().any(|d| d == DAYS[weekday(&at)]) };
        if ok {
            return Some(at);
        }
    }
    None
}

/// Whether an ISO minute (`2026-10-01T06:00`) has been reached.
pub fn reached(at: Option<&str>) -> bool {
    match at.filter(|s| !s.is_empty()) {
        None => true,
        Some(at) => minutes_iso(&now_local()).as_str() >= at,
    }
}

pub fn clock(hhmm: &str) -> String {
    twelve_hour(hhmm)
}

fn when(dt: &NaiveDateTime, now: &NaiveDateTime) -> String {
    let t = clock(&dt.format("%H:%M").to_string());
    if dt.date() == now.date() {
        return t;
    }
    if dt.date() == (*now + Duration::days(1)).date() {
        return format!("tomorrow {t}");
    }
    format!("{} {t}", dt.format("%a"))
}

pub fn say_when(at: &str) -> String {
    match NaiveDateTime::parse_from_str(at, "%Y-%m-%dT%H:%M") {
        Ok(dt) => when(&dt, &now_local()),
        Err(_) => at.to_string(),
    }
}

fn span(h: &Hours, first: chrono::Weekday) -> String {
    let all: Vec<String> = DAYS.iter().map(|d| d.to_string()).collect();
    let d = if h.days == all {
        String::new()
    } else if h.days == all[..5] {
        " weekdays".into()
    } else if h.days == all[5..] {
        " weekends".into()
    } else {
        let mut ds = h.days.clone();
        ds.sort_by_key(|d| week_days(first).iter().position(|x| x == d));
        format!(" {}", ds.iter().map(|d| capitalize(d)).collect::<Vec<_>>().join(", "))
    };
    format!("{}–{}{d}", clock(&h.start), clock(&h.end))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

pub fn state(app: &App) -> Value {
    let now = now_local();
    let h = get(app);
    let until = if h.on { today_until(app, &now) } else { None };
    let open = open_at(&h, until.as_deref(), &now);
    let nxt = if open { None } else { next_open(&h, &now, until.as_deref()) };
    let mut line = if !h.on {
        "No work hours: agents start any time".to_string()
    } else if open {
        format!(
            "Work hours {}: agents start until {}{}",
            span(&h, app.cfg.first_weekday),
            clock(until.as_deref().unwrap_or(&h.end)),
            if until.is_some() { " today" } else { "" }
        )
    } else if let Some(n) = nxt {
        format!("Outside work hours: agents start again {}", when(&n, &now))
    } else {
        "Outside work hours".into()
    };
    if let Some(out) = usage::out_until(app) {
        line = format!("Out of 5-hour usage: agents start again at {}", clock(&local_hhmm(Some(&out))));
    }
    let every = h.alert_every_mins;
    let alerts_line = if every > 0 {
        format!("Alerts repeat every {every} min{}", if h.on { " in work hours" } else { "" })
    } else {
        "Alerts don't repeat".into()
    };
    let mut v = h.to_json();
    let o = v.as_object_mut().unwrap();
    o.insert("open".into(), json!(open));
    o.insert("line".into(), json!(line));
    o.insert("alerts_line".into(), json!(alerts_line));
    o.insert("span".into(), json!(span(&h, app.cfg.first_weekday)));
    o.insert("today_until".into(), json!(until));
    o.insert("next_open".into(), json!(nxt.map(|n| minutes_iso(&n))));
    o.insert("week_days".into(), json!(week_days(app.cfg.first_weekday)));
    v
}

/// Why a queued task waits on the clock, if it does.
pub fn blocker(app: &App) -> Option<String> {
    if let Some(out) = usage::out_until(app) {
        return Some(format!("Waits for the 5-hour usage to reset ({})", clock(&local_hhmm(Some(&out)))));
    }
    if is_open(app) {
        return None;
    }
    let now = now_local();
    Some(match next_open(&get(app), &now, today_until(app, &now).as_deref()) {
        Some(n) => format!("Waits for work hours ({})", when(&n, &now)),
        None => "Waits for work hours".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn h(start: &str, end: &str, days: &[&str]) -> Hours {
        Hours { on: true, start: start.into(), end: end.into(), days: days.iter().map(|d| d.to_string()).collect(), alert_every_mins: 5 }
    }

    fn at(y: i32, mo: u32, d: u32, hh: u32, mm: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(hh, mm, 0).unwrap()
    }

    #[test]
    fn parses_times_and_days() {
        assert_eq!(parse_time("3pm", "end").unwrap(), "15:00");
        assert_eq!(parse_time("06:30", "start").unwrap(), "06:30");
        assert_eq!(parse_time("12am", "start").unwrap(), "00:00");
        assert!(parse_time("25:00", "start").is_err());
        assert_eq!(parse_days(&json!("mon-fri")).unwrap().len(), 5);
        assert_eq!(parse_days(&json!("fri-mon")).unwrap(), vec!["mon", "fri", "sat", "sun"]);
        assert!(parse_days(&json!("funday")).is_err());
    }

    #[test]
    fn windows_and_overnight() {
        let day = h("09:00", "17:00", &["mon", "tue", "wed", "thu", "fri"]);
        // 2026-10-05 is a Monday.
        assert!(open_at(&day, None, &at(2026, 10, 5, 10, 0)));
        assert!(!open_at(&day, None, &at(2026, 10, 5, 17, 0)));
        assert!(!open_at(&day, None, &at(2026, 10, 4, 10, 0)));
        let night = h("22:00", "02:00", &["mon"]);
        assert!(open_at(&night, None, &at(2026, 10, 5, 23, 0)));
        assert!(open_at(&night, None, &at(2026, 10, 6, 1, 0)));
        assert!(!open_at(&night, None, &at(2026, 10, 7, 1, 0)));
        assert!(open_at(&day, Some("19:00"), &at(2026, 10, 5, 18, 0)));
        assert_eq!(next_open(&day, &at(2026, 10, 9, 18, 0), None), Some(at(2026, 10, 12, 9, 0)));
    }

    #[test]
    fn weeks_start_on_the_configured_day() {
        use chrono::Weekday;
        assert_eq!(week_days(Weekday::Sun), vec!["sun", "mon", "tue", "wed", "thu", "fri", "sat"]);
        assert_eq!(week_days(Weekday::Mon)[6], "sun");
        assert_eq!(weekday_key(Weekday::Sat), "sat");
        let odd = h("09:00", "17:00", &["mon", "sat", "sun"]);
        assert_eq!(span(&odd, Weekday::Sun), "9am–5pm Sun, Mon, Sat");
        assert_eq!(span(&odd, Weekday::Mon), "9am–5pm Mon, Sat, Sun");
    }
}
