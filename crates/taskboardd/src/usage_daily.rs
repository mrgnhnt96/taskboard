//! The daily limit on the weekly (7-day) usage: each local day gets a share of what's left of the week,
//! and once today's share is used the board starts no new work until midnight or an override. Running
//! agents carry on.

use chrono::{Duration, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::App;
use crate::hours;
use crate::usage;
use crate::util::*;

/// The owner's setting: `{"base": "off" | "even" | <N>, "carry": "next_day" | "spread"}`.
pub const SETTING_KEY: &str = "usage_daily";
/// The plan since the weekly window (or the setting) last changed, and today's start.
pub const PLAN_KEY: &str = "usage_daily_plan";
/// The local day (`YYYY-MM-DD`) the limit is lifted for.
pub const OVERRIDE_KEY: &str = "usage_daily_override";
/// The local day the owner was last told the limit was reached.
pub const TOLD_KEY: &str = "usage_daily_told";
const DAY: &str = "%Y-%m-%d";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Base {
    Off,
    Even,
    Pct(f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Carry {
    NextDay,
    Spread,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Setting {
    pub base: Base,
    pub carry: Carry,
}

impl Default for Setting {
    fn default() -> Self {
        Setting { base: Base::Off, carry: Carry::NextDay }
    }
}

fn parse_base(v: &Value) -> std::result::Result<Base, String> {
    if let Some(n) = v.as_f64() {
        return pct_base(n);
    }
    let s = v.as_str().unwrap_or("").trim().to_ascii_lowercase();
    match s.as_str() {
        "" | "off" | "none" => Ok(Base::Off),
        "even" => Ok(Base::Even),
        _ => match s.trim_end_matches('%').trim().parse::<f64>() {
            Ok(n) => pct_base(n),
            Err(_) => Err(format!("The daily limit's base is off, even or a percent of the week per day (like 15), not \"{s}\".")),
        },
    }
}

fn pct_base(n: f64) -> std::result::Result<Base, String> {
    if n > 0.0 && n <= 100.0 {
        Ok(Base::Pct(n))
    } else {
        Err("A daily limit of N% of the week takes an N from 1 to 100.".into())
    }
}

fn parse_carry(v: &Value) -> std::result::Result<Carry, String> {
    match v.as_str().unwrap_or("").trim().to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
        "" | "next_day" | "next" => Ok(Carry::NextDay),
        "spread" => Ok(Carry::Spread),
        other => Err(format!("The daily limit's carry is next-day or spread, not \"{other}\".")),
    }
}

impl Setting {
    pub fn parse(raw: Option<&str>) -> Setting {
        let o = Value::Object(jloads_obj(raw));
        Setting { base: parse_base(&o["base"]).unwrap_or(Base::Off), carry: parse_carry(&o["carry"]).unwrap_or(Carry::NextDay) }
    }

    pub fn on(&self) -> bool {
        self.base != Base::Off
    }

    pub fn to_json(&self) -> Value {
        json!({"base": base_json(self.base), "carry": match self.carry { Carry::NextDay => "next_day", Carry::Spread => "spread" }})
    }

    /// What the plan was made for: changing either starts a fresh plan.
    fn sig(&self) -> String {
        self.to_json().to_string()
    }
}

fn base_json(b: Base) -> Value {
    match b {
        Base::Off => json!("off"),
        Base::Even => json!("even"),
        Base::Pct(n) => json!(n),
    }
}

/// The weekly window as the plan reads it: the percentage used, its reset, and that reset in local time.
#[derive(Clone, Copy, Debug)]
pub struct Week {
    pub pct: f64,
    pub resets_at: f64,
    pub reset_local: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub setting: String,
    pub resets_at: f64,
    pub start_day: String,
    pub start_pct: f64,
    /// Each day's allotment: N, or for even, the week left at the plan's start over its days left.
    pub base: f64,
    pub day: String,
    pub day_pct: f64,
}

fn date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, DAY).ok()
}

fn day_str(d: NaiveDate) -> String {
    d.format(DAY).to_string()
}

/// The days left to the reset, today included; a reset at midnight leaves its own day out.
pub fn days_left(today: NaiveDate, reset_local: NaiveDateTime) -> i64 {
    let d = (reset_local.date() - today).num_days();
    let part = reset_local.time() > chrono::NaiveTime::MIN;
    (d + i64::from(part)).max(1)
}

const SAME_WINDOW_SECS: f64 = 3600.0;

/// The plan for `today`: the one kept, rolled to today, or a fresh one when there's none, the setting
/// changed, the weekly window reset, or the clock went back past its start. A fresh plan for a new
/// setting in the same window keeps today's start, so what today already used still counts.
pub fn advance(kept: Option<Plan>, s: &Setting, w: &Week, today: NaiveDate) -> Plan {
    let sig = s.sig();
    let same_window = |p: &Plan| (p.resets_at - w.resets_at).abs() < SAME_WINDOW_SECS;
    let today_start = kept.as_ref().filter(|p| same_window(p) && p.day == day_str(today)).map(|p| p.day_pct.min(w.pct));
    let kept = kept.filter(|p| {
        p.setting == sig && same_window(p) && date(&p.start_day).is_some_and(|d| d <= today) && date(&p.day).is_some_and(|d| d <= today)
    });
    let Some(mut p) = kept else {
        let start = today_start.unwrap_or(w.pct);
        let base = match s.base {
            Base::Pct(n) => n,
            _ => (100.0 - start).max(0.0) / days_left(today, w.reset_local) as f64,
        };
        return Plan { setting: sig, resets_at: w.resets_at, start_day: day_str(today), start_pct: start, base, day: day_str(today), day_pct: start };
    };
    if p.day != day_str(today) {
        p.day = day_str(today);
        p.day_pct = w.pct.max(p.day_pct);
    }
    p
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Today {
    pub allowed: f64,
    pub used: f64,
    pub left: f64,
    /// Allowance carried into today from the plan's earlier days: negative after an overspent day.
    pub carry: f64,
    pub limited: bool,
}

/// Today's numbers from the plan, at `pct` used of the week.
pub fn today(p: &Plan, carry: Carry, pct: f64, today: NaiveDate, reset_local: NaiveDateTime) -> Today {
    let start = date(&p.start_day).unwrap_or(today);
    let days = (today - start).num_days().max(0) as f64;
    let carried = p.base * days - (p.day_pct - p.start_pct).max(0.0);
    let allowed = match carry {
        Carry::NextDay => p.base + carried,
        Carry::Spread => p.base + carried / days_left(today, reset_local) as f64,
    };
    let allowed = allowed.min(100.0 - p.day_pct).max(0.0);
    let used = (pct - p.day_pct).max(0.0);
    Today { allowed, used, left: (allowed - used).max(0.0), carry: carried, limited: used >= allowed }
}

pub fn setting(app: &App) -> Setting {
    Setting::parse(app.db.get_setting(SETTING_KEY).ok().flatten().as_deref())
}

pub fn set(app: &App, body: &Value) -> Result<Setting> {
    let mut s = setting(app);
    if let Some(b) = body.get("base").filter(|v| !v.is_null()) {
        s.base = parse_base(b).map_err(|e| ApiError::new(400, e))?;
    }
    if let Some(c) = body.get("carry").filter(|v| !v.is_null()) {
        s.carry = parse_carry(c).map_err(|e| ApiError::new(400, e))?;
    }
    // The plan stays while the limit is off, so turning it back on today keeps today's start.
    app.db.set_setting(SETTING_KEY, Some(&s.to_json().to_string()))?;
    app.notify_jobs();
    Ok(s)
}

fn today_local() -> NaiveDate {
    hours::now_local().date()
}

/// Whether the override (the day it was set for) lifts the limit on `day`: it ends at that day's midnight.
pub fn lifted(set_for: Option<&str>, day: NaiveDate) -> bool {
    set_for == Some(day_str(day).as_str())
}

fn overridden(app: &App, day: NaiveDate) -> bool {
    lifted(app.db.get_setting(OVERRIDE_KEY).ok().flatten().as_deref(), day)
}

/// Lifts today's limit until local midnight, or (`off`) puts it back.
pub fn set_override(app: &App, off: bool) -> Result<()> {
    let day = day_str(today_local());
    app.db.set_setting(OVERRIDE_KEY, if off { None } else { Some(&day) })?;
    if off {
        app.db.set_setting(TOLD_KEY, None)?;
    }
    app.notify_jobs();
    Ok(())
}

struct Now {
    today: Today,
    day: NaiveDate,
    week: Week,
    overridden: bool,
}

/// Today's numbers, keeping the plan up to date. None while the limit is off or the weekly usage isn't
/// known (or its window has reset with no new report): then nothing is held.
fn now(app: &App) -> Option<Now> {
    let s = setting(app);
    if !s.on() {
        return None;
    }
    let (pct, resets_at) = usage::weekly(app)?;
    let week = Week { pct, resets_at, reset_local: local_dt(resets_at).naive_local() };
    let day = today_local();
    let raw = app.db.get_setting(PLAN_KEY).ok().flatten();
    let kept = raw.as_deref().and_then(|r| serde_json::from_str::<Plan>(r).ok());
    let plan = advance(kept.clone(), &s, &week, day);
    if kept.as_ref() != Some(&plan) {
        if let Ok(j) = serde_json::to_string(&plan) {
            let _ = app.db.set_setting(PLAN_KEY, Some(&j));
        }
    }
    let today = today(&plan, s.carry, pct, day, week.reset_local);
    Some(Now { today, day, week, overridden: overridden(app, day) })
}

fn midnight_after(day: NaiveDate) -> String {
    hours::minutes_iso(&(day + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap_or_default())
}

/// While today's share of the week is used up (and not lifted), the local midnight new work waits for.
pub fn limited_until(app: &App) -> Option<String> {
    let n = now(app)?;
    (n.today.limited && !n.overridden).then(|| midnight_after(n.day))
}

fn pct(n: f64) -> String {
    let r = (n * 10.0).round() / 10.0;
    if r.fract() == 0.0 {
        format!("{}%", r as i64)
    } else {
        format!("{r:.1}%")
    }
}

fn signed(n: f64) -> String {
    if n < 0.0 {
        format!("−{}", pct(-n))
    } else {
        format!("+{}", pct(n))
    }
}

fn line(n: &Now) -> String {
    let t = &n.today;
    if n.overridden {
        return format!("Today's limit is lifted until midnight: {} used of {}", pct(t.used), pct(t.allowed));
    }
    if t.limited {
        return format!("Today's {} is used up: new work starts again {}", pct(t.allowed), hours::say_when(&midnight_after(n.day)));
    }
    let carried = if round1(t.carry) == 0.0 { String::new() } else { format!(" ({} carried)", signed(t.carry)) };
    format!("Today: {} used of {}, {} left{carried}", pct(t.used), pct(t.allowed), pct(t.left))
}

fn round1(n: f64) -> f64 {
    (n * 10.0).round() / 10.0
}

/// The `daily` block of `GET /usage`: the setting, and while it's on and the week is known, today's numbers.
pub fn state(app: &App) -> Value {
    let s = setting(app);
    let mut v = s.to_json();
    v["on"] = json!(s.on());
    let Some(n) = now(app) else { return v };
    let t = n.today;
    let extra = json!({
        "allowed": round1(t.allowed),
        "used": round1(t.used),
        "left": round1(t.left),
        "carried": round1(t.carry),
        "limited": t.limited && !n.overridden,
        "overridden": n.overridden,
        "day": day_str(n.day),
        "until": midnight_after(n.day),
        "days_left": days_left(n.day, n.week.reset_local),
        "resets_at": iso(n.week.resets_at),
        "line": line(&n),
    });
    if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
        o.extend(e);
    }
    v
}

/// Tells the owner once a day when today's share runs out.
pub fn tick(app: &App) -> Result<()> {
    let Some(n) = now(app) else { return Ok(()) };
    if !n.today.limited || n.overridden {
        return Ok(());
    }
    let day = day_str(n.day);
    if app.db.get_setting(TOLD_KEY)?.as_deref() == Some(day.as_str()) {
        return Ok(());
    }
    app.db.set_setting(TOLD_KEY, Some(&day))?;
    app.info(format!("usage: today's {} of the week is used; holding new work until midnight", pct(n.today.allowed)));
    crate::dispatch::notify(
        app,
        &format!("Today's {} of the weekly usage is used up. New work starts again {}; running agents carry on.", pct(n.today.allowed), hours::say_when(&midnight_after(n.day))),
        &app.cfg.page_url,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        date(s).unwrap()
    }

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").unwrap()
    }

    const RESETS: f64 = 1_800_000_000.0;

    /// The week resets Sunday 3pm; Monday through Sunday is 7 days, today included.
    fn week(pct: f64) -> Week {
        Week { pct, resets_at: RESETS, reset_local: at("2026-11-08T15:00") }
    }

    fn even(carry: Carry) -> Setting {
        Setting { base: Base::Even, carry }
    }

    fn fixed(n: f64, carry: Carry) -> Setting {
        Setting { base: Base::Pct(n), carry }
    }

    /// Walks the plan through `days` (day, the week's % at its first reading), returning the last day's plan.
    fn walk(s: &Setting, days: &[(&str, f64)]) -> Plan {
        let mut p = None;
        for (day, pct) in days {
            p = Some(advance(p, s, &week(*pct), d(day)));
        }
        p.unwrap()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn days_left_counts_today_and_a_reset_days_part() {
        assert_eq!(days_left(d("2026-11-02"), at("2026-11-08T15:00")), 7);
        assert_eq!(days_left(d("2026-11-02"), at("2026-11-08T00:00")), 6, "a midnight reset leaves its day out");
        assert_eq!(days_left(d("2026-11-08"), at("2026-11-08T15:00")), 1);
        assert_eq!(days_left(d("2026-11-09"), at("2026-11-08T15:00")), 1, "never less than today");
    }

    #[test]
    fn even_splits_the_week_left_over_the_days_left() {
        let s = even(Carry::NextDay);
        let p = advance(None, &s, &week(30.0), d("2026-11-02"));
        assert!(close(p.base, 10.0));
        let t = today(&p, s.carry, 34.0, d("2026-11-02"), week(34.0).reset_local);
        assert!(close(t.allowed, 10.0) && close(t.used, 4.0) && close(t.left, 6.0) && close(t.carry, 0.0));
        assert!(!t.limited);
        assert!(today(&p, s.carry, 40.0, d("2026-11-02"), week(40.0).reset_local).limited);
    }

    #[test]
    fn a_fixed_percent_is_each_days_allotment() {
        let s = fixed(15.0, Carry::NextDay);
        let p = advance(None, &s, &week(30.0), d("2026-11-02"));
        let t = today(&p, s.carry, 30.0, d("2026-11-02"), week(30.0).reset_local);
        assert!(close(t.allowed, 15.0) && close(t.left, 15.0));
    }

    #[test]
    fn a_fixed_percent_never_allows_more_than_the_week_has_left() {
        let s = fixed(15.0, Carry::NextDay);
        let p = advance(None, &s, &week(90.0), d("2026-11-02"));
        assert!(close(today(&p, s.carry, 90.0, d("2026-11-02"), week(90.0).reset_local).allowed, 10.0));
    }

    #[test]
    fn an_underspent_day_rolls_to_the_next_day() {
        let s = even(Carry::NextDay);
        let p = walk(&s, &[("2026-11-02", 30.0), ("2026-11-03", 34.0)]);
        let t = today(&p, s.carry, 34.0, d("2026-11-03"), week(34.0).reset_local);
        assert!(close(t.carry, 6.0), "6% unused on Monday: {t:?}");
        assert!(close(t.allowed, 16.0));
    }

    #[test]
    fn spread_shares_the_leftover_over_the_days_left() {
        let s = even(Carry::Spread);
        let p = walk(&s, &[("2026-11-02", 30.0), ("2026-11-03", 34.0)]);
        let t = today(&p, s.carry, 34.0, d("2026-11-03"), week(34.0).reset_local);
        assert!(close(t.allowed, 66.0 / 6.0), "the week left over the 6 days left: {t:?}");
        assert!(close(t.carry, 6.0));
    }

    #[test]
    fn spread_on_a_fixed_percent_adds_a_share_of_the_leftover() {
        let s = fixed(10.0, Carry::Spread);
        let p = walk(&s, &[("2026-11-02", 30.0), ("2026-11-03", 34.0)]);
        let t = today(&p, s.carry, 34.0, d("2026-11-03"), week(34.0).reset_local);
        assert!(close(t.allowed, 10.0 + 6.0 / 6.0));
    }

    #[test]
    fn an_overspent_override_day_takes_from_the_next_days() {
        let s = even(Carry::NextDay);
        let p = walk(&s, &[("2026-11-02", 30.0), ("2026-11-03", 55.0)]);
        let t = today(&p, s.carry, 55.0, d("2026-11-03"), week(55.0).reset_local);
        assert!(close(t.carry, -15.0), "25% used on a 10% day: {t:?}");
        assert!(close(t.allowed, 0.0) && t.limited, "nothing left for the day after");
        let p = advance(Some(p), &s, &week(55.0), d("2026-11-04"));
        let t = today(&p, s.carry, 55.0, d("2026-11-04"), week(55.0).reset_local);
        assert!(close(t.allowed, 5.0), "two days' allotments less the overspend: {t:?}");

        let spread = even(Carry::Spread);
        let p = walk(&spread, &[("2026-11-02", 30.0), ("2026-11-03", 55.0)]);
        let t = today(&p, spread.carry, 55.0, d("2026-11-03"), week(55.0).reset_local);
        assert!(close(t.allowed, 45.0 / 6.0), "spread shares the overspend too: {t:?}");
    }

    #[test]
    fn a_new_day_starts_from_the_weeks_reading_then() {
        let s = even(Carry::NextDay);
        let p = walk(&s, &[("2026-11-02", 30.0), ("2026-11-02", 38.0)]);
        assert_eq!(p.day, "2026-11-02");
        assert!(close(p.day_pct, 30.0), "a later reading the same day keeps the day's start");
        let p = advance(Some(p), &s, &week(38.0), d("2026-11-03"));
        assert_eq!(p.day, "2026-11-03");
        assert!(close(p.day_pct, 38.0));
        assert_eq!(p.start_day, "2026-11-02", "the plan carries on");
    }

    #[test]
    fn a_weekly_reset_or_a_new_setting_starts_a_fresh_plan() {
        let s = even(Carry::NextDay);
        let p = walk(&s, &[("2026-11-02", 30.0), ("2026-11-03", 55.0)]);
        let reset = Week { pct: 2.0, resets_at: RESETS + 7.0 * 86400.0, reset_local: at("2026-11-10T15:00") };
        let fresh = advance(Some(p.clone()), &s, &reset, d("2026-11-03"));
        assert_eq!(fresh.start_day, "2026-11-03");
        assert!(close(fresh.start_pct, 2.0) && close(fresh.base, 98.0 / 8.0));
        let t = today(&fresh, s.carry, 2.0, d("2026-11-03"), reset.reset_local);
        assert!(close(t.carry, 0.0) && !t.limited, "the old week's overspend is gone: {t:?}");

        let other = fixed(12.0, Carry::NextDay);
        let fresh = advance(Some(p), &other, &week(55.0), d("2026-11-03"));
        assert_eq!(fresh.start_day, "2026-11-03");
        assert!(close(fresh.base, 12.0) && close(fresh.start_pct, 55.0));
    }

    #[test]
    fn a_new_setting_keeps_what_today_already_used() {
        let p = walk(&even(Carry::NextDay), &[("2026-11-02", 30.0), ("2026-11-03", 34.0)]);
        let s = fixed(12.0, Carry::NextDay);
        let fresh = advance(Some(p.clone()), &s, &week(40.0), d("2026-11-03"));
        assert!(close(fresh.day_pct, 34.0) && close(fresh.start_pct, 34.0), "today began at 34%: {fresh:?}");
        let t = today(&fresh, s.carry, 40.0, d("2026-11-03"), week(40.0).reset_local);
        assert!(close(t.used, 6.0) && close(t.allowed, 12.0) && close(t.left, 6.0), "{t:?}");

        let e = advance(Some(p.clone()), &even(Carry::Spread), &week(40.0), d("2026-11-03"));
        assert!(close(e.base, 66.0 / 6.0), "even splits the week left at today's start: {e:?}");

        let tomorrow = advance(Some(p), &s, &week(40.0), d("2026-11-04"));
        assert!(close(tomorrow.day_pct, 40.0), "a plan from an earlier day doesn't carry its start: {tomorrow:?}");
    }

    #[test]
    fn an_override_lasts_until_midnight() {
        assert!(lifted(Some("2026-11-02"), d("2026-11-02")));
        assert!(!lifted(Some("2026-11-02"), d("2026-11-03")), "it expires at midnight");
        assert!(!lifted(None, d("2026-11-02")));
    }

    #[test]
    fn the_settings_parse_and_default_off() {
        assert_eq!(Setting::parse(None), Setting::default());
        assert!(!Setting::default().on());
        assert_eq!(Setting::parse(Some(r#"{"base":"even","carry":"spread"}"#)), even(Carry::Spread));
        assert_eq!(Setting::parse(Some(r#"{"base":15}"#)), fixed(15.0, Carry::NextDay));
        assert_eq!(parse_base(&json!("12%")), Ok(Base::Pct(12.0)));
        assert!(parse_base(&json!("0")).is_err() && parse_base(&json!("lots")).is_err());
        assert_eq!(parse_carry(&json!("next-day")), Ok(Carry::NextDay));
        assert!(parse_carry(&json!("later")).is_err());
        assert_eq!(Setting::parse(Some(&fixed(15.0, Carry::Spread).to_json().to_string())), fixed(15.0, Carry::Spread));
    }
}
