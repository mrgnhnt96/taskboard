//! The PR hosts' rate limits, kept per account for the whole daemon: every reader that signs in as the
//! same account (the PR poll, master breaks, the owner's PR builds, review bots, CI reads, `tb pr …`)
//! goes through the same [`Gate`], since the host counts them together, along with the owner's other
//! tools on that account.
//!
//! # Backing off
//!
//! When the host says the account is limited (a 429; GitHub's "API rate limit exceeded" or secondary
//! limit; `X-RateLimit-Remaining: 0`), every call on that account stops until the host's
//! `Retry-After` / `X-RateLimit-Reset`, else for 5 minutes, doubling up to an hour while the limit
//! keeps coming back. A call made meanwhile fails at once with "Bitbucket rate-limited until 3:40pm"
//! instead of reaching the host. Each backoff logs a line with the board's own calls in the last hour,
//! so it's clear whether the board used the limit up. [`check`] (every runner tick) raises the
//! `rate-limit:<host>` alert once a limit has lasted 30 minutes inside the work hours, and clears it
//! when the limit ends.
//!
//! # Budget
//!
//! `[hosts.<host>] max_per_hour` caps the board's own calls on a host. Reads past it wait until the
//! hour's oldest call ages out (writes still go). Before that, [`stretch`] widens the PR poll and the
//! review sweep (`[intervals] prs` and `reviews`) so the last ten minutes' pace fits the budget.
//!
//! # Showing it
//!
//! [`status`]: each account's host, its limit (`limited_until`, `why`), the board's calls in the last
//! hour and its budget, on `/state` (`rate_limits`) and `GET /prs/feed` (`tb feed`).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::OnceLock;

use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;

/// `[hosts.<host>]` in config.toml (`github`, `bitbucket`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HostConfig {
    /// The most calls the board makes on the host's account in an hour; unset: no cap.
    pub max_per_hour: Option<u32>,
}

/// A limit with no `Retry-After` / reset: 5 minutes, doubling while it keeps coming back.
const FIRST_BACKOFF_SECS: f64 = 300.0;
const MAX_BACKOFF_SECS: f64 = 3600.0;
const HOUR: f64 = 3600.0;
/// The window the pace for [`stretch`] is read over.
const PACE_SECS: f64 = 600.0;
/// Aim under the budget, so the hard cap rarely bites.
const AIM: f64 = 0.9;
const MAX_STRETCH: f64 = 30.0;
/// A limit this long inside the work hours raises an alert.
const ALERT_AFTER_SECS: f64 = 1800.0;
pub const ALERT_PREFIX: &str = "rate-limit:";

/// One account on a host.
#[derive(Debug, Default)]
struct Account {
    /// `github`, `bitbucket`, …
    host: String,
    /// When the board's calls went out, in the last hour.
    calls: VecDeque<f64>,
    /// Calls stop until then.
    until: Option<f64>,
    /// When this run of limits started (kept across a doubling).
    since: Option<f64>,
    /// Limits in a row without a call getting through: what doubles the backoff.
    strikes: u32,
    why: String,
    alerted: bool,
}

impl Account {
    fn forget_old(&mut self, now: f64) {
        while self.calls.front().is_some_and(|t| *t <= now - HOUR) {
            self.calls.pop_front();
        }
    }
}

/// Every account's limits and calls.
#[derive(Debug, Default)]
pub struct Limits {
    accounts: Mutex<HashMap<String, Account>>,
    /// `max_per_hour` by host.
    budgets: Mutex<BTreeMap<String, u32>>,
    /// The poll stretch by host, and when it was last worked out.
    stretch: Mutex<HashMap<String, (f64, f64)>>,
    /// Lines for the daemon's log, written on the next [`check`].
    log: Mutex<Vec<String>>,
}

/// What the host said about a call that hit its limit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hit {
    /// `Retry-After`, in seconds.
    pub retry_after: Option<f64>,
    /// `X-RateLimit-Reset`: when the limit lifts, epoch seconds.
    pub reset_at: Option<f64>,
    /// What the host said, for the log and the alert.
    pub why: String,
}

/// The display name of a host id.
pub fn label(host: &str) -> String {
    match host {
        "github" => "GitHub".into(),
        "bitbucket" => "Bitbucket".into(),
        "azure devops" => "Azure DevOps".into(),
        other => other.to_string(),
    }
}

fn until_text(host: &str, until: f64) -> String {
    format!("{} rate-limited until {}", label(host), local_clock(Some(&iso(until))))
}

impl Limits {
    pub fn set_budgets(&self, budgets: BTreeMap<String, u32>) {
        *self.budgets.lock() = budgets;
    }

    fn budget(&self, host: &str) -> Option<u32> {
        self.budgets.lock().get(host).copied().filter(|b| *b > 0)
    }

    /// Whether a call on `key` may go out now; counts it when it does. Err: why it waits.
    pub fn admit(&self, key: &str, host: &str, write: bool, now: f64) -> std::result::Result<(), String> {
        let budget = self.budget(host);
        let mut all = self.accounts.lock();
        let a = all.entry(key.to_string()).or_insert_with(|| Account { host: host.to_string(), ..Default::default() });
        a.forget_old(now);
        if let Some(u) = a.until {
            if u > now {
                return Err(format!("{}; the board holds its calls until then", until_text(host, u)));
            }
        }
        if let Some(b) = budget {
            if !write && a.calls.len() >= b as usize {
                let free_at = a.calls.front().map(|t| t + HOUR).unwrap_or(now);
                return Err(format!(
                    "the board made its {b} {} calls this hour ([hosts.{host}] max_per_hour); reads wait until {}",
                    label(host),
                    local_clock(Some(&iso(free_at)))
                ));
            }
        }
        a.calls.push_back(now);
        Ok(())
    }

    /// The host answered: the limit (if there was one) is over.
    pub fn answered(&self, key: &str, now: f64) {
        if let Some(a) = self.accounts.lock().get_mut(key) {
            if a.until.is_none_or(|u| u <= now) {
                a.strikes = 0;
                a.until = None;
                a.since = None;
            }
        }
    }

    /// The host says `key` is rate-limited: stop its calls until the host says, else back off.
    /// Answers the time calls start again.
    pub fn hit(&self, key: &str, host: &str, hit: &Hit, now: f64) -> f64 {
        let mut all = self.accounts.lock();
        let a = all.entry(key.to_string()).or_insert_with(|| Account { host: host.to_string(), ..Default::default() });
        a.forget_old(now);
        let told = match (hit.retry_after, hit.reset_at) {
            (Some(s), _) if s > 0.0 => Some(now + s),
            (_, Some(t)) if t > now => Some(t),
            _ => None,
        };
        let standing = a.until.filter(|u| *u > now);
        // A limit already standing doesn't back off again for the calls that were already out; only
        // a later time from the host moves it.
        let until = match (standing, told) {
            (Some(u), t) => u.max(t.unwrap_or(u)),
            (None, Some(t)) => t,
            (None, None) => now + (FIRST_BACKOFF_SECS * 2f64.powi(a.strikes.min(8) as i32)).min(MAX_BACKOFF_SECS),
        };
        if standing.is_none() {
            a.strikes += 1;
        }
        a.until = Some(until);
        a.since.get_or_insert(now);
        a.why = hit.why.clone();
        let budget = self.budget(host).map(|b| format!(" (budget {b})")).unwrap_or_default();
        self.log.lock().push(format!(
            "rate limit: {} answered \"{}\"; the board backs off until {} ({}); it made {} calls on this account in the last hour{budget}",
            label(host),
            one_line(&hit.why, 160),
            local_clock(Some(&iso(until))),
            dur_text(until - now),
            a.calls.len(),
        ));
        until
    }

    /// How much wider the PR poll should be so the board's pace on every budgeted host fits its
    /// budget: 1 when it fits (or there's no budget). Worked out at most once a minute per host.
    pub fn stretch(&self, now: f64) -> f64 {
        let budgets = self.budgets.lock().clone();
        let mut out: f64 = 1.0;
        for (host, b) in budgets.iter().filter(|(_, b)| **b > 0) {
            let mut st = self.stretch.lock();
            let (f, at) = st.get(host).copied().unwrap_or((1.0, f64::MIN));
            let f = if now - at < 60.0 {
                f
            } else {
                let recent: usize = self.accounts.lock().values().filter(|a| &a.host == host).map(|a| a.calls.iter().filter(|t| **t > now - PACE_SECS).count()).sum();
                let pace = recent as f64 * HOUR / PACE_SECS;
                // The pace scales with 1 / stretch, so this is the stretch that would have fit.
                let f = (pace * f / (*b as f64 * AIM)).clamp(1.0, MAX_STRETCH);
                st.insert(host.clone(), (f, now));
                f
            };
            out = out.max(f);
        }
        out
    }

    /// Each account the board has called: its host, limit and calls in the last hour.
    pub fn status(&self, now: f64) -> Vec<Value> {
        let stretch = self.stretch.lock().clone();
        let mut all = self.accounts.lock();
        let mut out: Vec<Value> = all
            .values_mut()
            .map(|a| {
                a.forget_old(now);
                let limited = a.until.filter(|u| *u > now);
                json!({
                    "host": a.host, "label": label(&a.host),
                    "limited_until": limited.map(iso),
                    "limited_since": limited.and(a.since).map(iso),
                    "why": limited.map(|_| a.why.clone()),
                    "text": limited.map(|u| until_text(&a.host, u)),
                    "calls_last_hour": a.calls.len(),
                    "max_per_hour": self.budget(&a.host),
                    "stretch": stretch.get(&a.host).map(|(f, _)| *f).filter(|f| *f > 1.0),
                })
            })
            .collect();
        out.sort_by(|a, b| a["host"].as_str().cmp(&b["host"].as_str()));
        out
    }

    /// Limits that have lasted [`ALERT_AFTER_SECS`] and haven't been alerted: (host, text). Marks them.
    fn due_alerts(&self, now: f64) -> Vec<(String, String)> {
        let mut out = vec![];
        for a in self.accounts.lock().values_mut() {
            match (a.until, a.since) {
                (Some(u), Some(s)) if u > now && now - s >= ALERT_AFTER_SECS && !a.alerted => {
                    a.alerted = true;
                    out.push((a.host.clone(), format!("{} for {} now: {}. The board's PR reads wait until then.", until_text(&a.host, u), dur_text(now - s), one_line(&a.why, 160))));
                }
                _ => {}
            }
        }
        out
    }

    /// Hosts with no account limited any more, whose alert can go.
    fn lifted(&self, now: f64) -> Vec<String> {
        let all = self.accounts.lock();
        let mut hosts: Vec<String> = all.values().map(|a| a.host.clone()).collect();
        hosts.sort();
        hosts.dedup();
        hosts.retain(|h| !all.values().any(|a| &a.host == h && a.until.is_some_and(|u| u > now)));
        hosts
    }

    fn reset_alerts(&self, now: f64) {
        for a in self.accounts.lock().values_mut() {
            if a.until.is_none_or(|u| u <= now) {
                a.alerted = false;
            }
        }
    }

    fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock())
    }
}

fn dur_text(secs: f64) -> String {
    let mins = (secs / 60.0).ceil().max(1.0) as i64;
    if mins < 60 {
        plural(mins, "minute")
    } else if mins % 60 == 0 {
        plural(mins / 60, "hour")
    } else {
        format!("{} {}", plural(mins / 60, "hour"), plural(mins % 60, "minute"))
    }
}

/// The daemon's limits.
pub fn limits() -> &'static Limits {
    static ALL: OnceLock<Limits> = OnceLock::new();
    ALL.get_or_init(Limits::default)
}

/// One account's way to its host: every call goes through [`Gate::call`].
#[derive(Debug, Clone)]
pub struct Gate {
    /// The account (`prhost::account_key`).
    pub key: String,
    pub host: String,
}

impl Gate {
    pub fn new(host: &str, key: String) -> Gate {
        Gate { key, host: host.to_string() }
    }

    /// Makes the call unless the account is limited (or out of budget for a read); notes a limit the
    /// call's answer reports (`limited` reads it from the error), and a call that got through.
    pub fn call<T>(&self, write: bool, f: impl FnOnce() -> std::result::Result<(T, Option<Hit>), (String, Option<Hit>)>) -> std::result::Result<T, String> {
        let l = limits();
        l.admit(&self.key, &self.host, write, now_ts())?;
        match f() {
            Ok((v, hit)) => {
                match hit {
                    Some(h) => {
                        l.hit(&self.key, &self.host, &h, now_ts());
                    }
                    None => l.answered(&self.key, now_ts()),
                }
                Ok(v)
            }
            Err((e, Some(h))) => {
                let until = l.hit(&self.key, &self.host, &h, now_ts());
                Err(format!("{e} ({})", until_text(&self.host, until)))
            }
            Err((e, None)) => Err(e),
        }
    }
}

/// `/state`'s `rate_limits`, and `tb feed`'s.
pub fn status() -> Vec<Value> {
    limits().status(now_ts())
}

/// How much to widen the PR poll and the review sweep ([`Limits::stretch`]).
pub fn stretch() -> f64 {
    limits().stretch(now_ts())
}

/// Every runner tick: takes in `[hosts]`, logs the backoffs since the last tick, raises the alert for a
/// limit that's lasted 30 minutes inside the work hours, and clears it once the limit is over.
pub fn check(app: &App) -> Result<()> {
    let l = limits();
    l.set_budgets(app.cfg.hosts.iter().filter_map(|(h, c)| c.max_per_hour.map(|b| (h.clone(), b))).collect());
    for line in l.take_log() {
        app.info(line);
    }
    let now = now_ts();
    for host in l.lifted(now) {
        crate::dispatch::clear_alert_key(app, &format!("{ALERT_PREFIX}{host}"))?;
    }
    l.reset_alerts(now);
    if crate::hours::is_open(app) {
        for (host, text) in l.due_alerts(now) {
            app.info(format!("rate limit: {text}"));
            crate::dispatch::raise(app, &text, None, None, Some(&format!("{ALERT_PREFIX}{host}")), false)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: f64 = 1_800_000_000.0;

    fn hit(why: &str) -> Hit {
        Hit { why: why.into(), ..Default::default() }
    }

    #[test]
    fn a_limit_stops_every_call_on_the_account_until_the_host_says() {
        let l = Limits::default();
        l.admit("bb:1", "bitbucket", false, T0).unwrap();
        let until = l.hit("bb:1", "bitbucket", &Hit { retry_after: Some(120.0), ..hit("429") }, T0);
        assert_eq!(until, T0 + 120.0);
        let e = l.admit("bb:1", "bitbucket", false, T0 + 60.0).unwrap_err();
        assert!(e.starts_with("Bitbucket rate-limited until "), "{e}");
        assert!(l.admit("bb:1", "bitbucket", true, T0 + 60.0).is_err(), "writes wait too: the host would refuse them");
        assert!(l.admit("bb:2", "bitbucket", false, T0 + 60.0).is_ok(), "another account isn't limited");
        assert!(l.admit("bb:1", "bitbucket", false, T0 + 121.0).is_ok());
    }

    #[test]
    fn a_reset_time_is_used_when_there_is_no_retry_after() {
        let l = Limits::default();
        assert_eq!(l.hit("gh:1", "github", &Hit { reset_at: Some(T0 + 900.0), ..hit("API rate limit exceeded") }, T0), T0 + 900.0);
    }

    #[test]
    fn without_a_time_it_backs_off_five_minutes_doubling_to_an_hour() {
        let l = Limits::default();
        let mut t = T0;
        let mut waits = vec![];
        for _ in 0..6 {
            let u = l.hit("gh:1", "github", &hit("secondary rate limit"), t);
            waits.push(((u - t) / 60.0).round() as i64);
            t = u + 1.0;
        }
        assert_eq!(waits, vec![5, 10, 20, 40, 60, 60]);
        l.answered("gh:1", t);
        assert_eq!(l.hit("gh:1", "github", &hit("again"), t) - t, 300.0, "a call that got through starts the doubling over");
    }

    #[test]
    fn calls_already_out_don_t_double_a_standing_limit() {
        let l = Limits::default();
        let first = l.hit("gh:1", "github", &hit("x"), T0);
        assert_eq!(l.hit("gh:1", "github", &hit("x"), T0 + 1.0), first);
    }

    #[test]
    fn each_backoff_logs_the_board_s_own_calls() {
        let l = Limits::default();
        for i in 0..3 {
            l.admit("bb:1", "bitbucket", false, T0 + i as f64).unwrap();
        }
        l.hit("bb:1", "bitbucket", &hit("Bitbucket answered 429"), T0 + 5.0);
        let log = l.take_log();
        assert_eq!(log.len(), 1);
        assert!(log[0].contains("Bitbucket answered \"Bitbucket answered 429\""), "{}", log[0]);
        assert!(log[0].contains("it made 3 calls on this account in the last hour"), "{}", log[0]);
        assert!(l.take_log().is_empty());
    }

    #[test]
    fn the_budget_holds_reads_but_not_writes() {
        let l = Limits::default();
        l.set_budgets([("bitbucket".to_string(), 2)].into());
        l.admit("bb:1", "bitbucket", false, T0).unwrap();
        l.admit("bb:1", "bitbucket", false, T0 + 10.0).unwrap();
        let e = l.admit("bb:1", "bitbucket", false, T0 + 20.0).unwrap_err();
        assert!(e.contains("[hosts.bitbucket] max_per_hour"), "{e}");
        assert!(l.admit("bb:1", "bitbucket", true, T0 + 20.0).is_ok(), "a write still goes");
        assert!(l.admit("gh:1", "github", false, T0 + 20.0).is_ok(), "GitHub has no budget");
        assert!(l.admit("bb:1", "bitbucket", false, T0 + HOUR + 5.0).is_err(), "the write counts too");
        assert!(l.admit("bb:1", "bitbucket", false, T0 + HOUR + 11.0).is_ok(), "the hour's first two calls aged out");
    }

    #[test]
    fn the_poll_stretches_to_fit_the_budget() {
        let l = Limits::default();
        assert_eq!(l.stretch(T0), 1.0, "no budget, no stretch");
        l.set_budgets([("bitbucket".to_string(), 900)].into());
        // 300 calls in ten minutes: 1,800 an hour against 900 (aiming at 810).
        for i in 0..300 {
            l.admit("bb:1", "bitbucket", true, T0 + i as f64 * 2.0).unwrap();
        }
        let f = l.stretch(T0 + 600.0);
        assert!((f - 1800.0 / 810.0).abs() < 0.01, "{f}");
        assert_eq!(l.stretch(T0 + 630.0), f, "worked out at most once a minute");
        let st = l.status(T0 + 630.0);
        assert_eq!(st[0]["calls_last_hour"], 300);
        assert_eq!(st[0]["max_per_hour"], 900);
    }

    #[test]
    fn status_says_who_is_limited_until_when() {
        let l = Limits::default();
        l.admit("bb:1", "bitbucket", false, T0).unwrap();
        l.hit("bb:1", "bitbucket", &Hit { retry_after: Some(600.0), ..hit("Bitbucket answered 429") }, T0);
        let st = l.status(T0 + 1.0);
        assert_eq!(st[0]["label"], "Bitbucket");
        assert_eq!(st[0]["limited_until"], iso(T0 + 600.0));
        assert!(st[0]["text"].as_str().unwrap().starts_with("Bitbucket rate-limited until "));
        assert!(l.status(T0 + 601.0)[0]["limited_until"].is_null());
    }

    #[test]
    fn a_limit_is_alerted_once_after_half_an_hour_and_lifted_after() {
        let l = Limits::default();
        let mut t = l.hit("bb:1", "bitbucket", &hit("429"), T0);
        assert!(l.due_alerts(T0 + 60.0).is_empty());
        // It keeps coming back once each backoff ends, with no call getting through in between.
        for _ in 0..3 {
            t = l.hit("bb:1", "bitbucket", &hit("429"), t);
        }
        let due = l.due_alerts(T0 + 31.0 * 60.0);
        assert_eq!(due.len(), 1, "{due:?}");
        assert_eq!(due[0].0, "bitbucket");
        assert!(l.due_alerts(T0 + 32.0 * 60.0).is_empty(), "once");
        assert!(l.lifted(T0 + 32.0 * 60.0).is_empty());
        assert_eq!(l.lifted(t + 1.0), vec!["bitbucket".to_string()]);
    }
}
