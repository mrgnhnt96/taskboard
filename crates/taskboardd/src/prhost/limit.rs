//! The PR hosts' rate limits, kept per account for the whole daemon: every reader that signs in as the
//! same account (the PR poll, master breaks, the owner's PR builds, review bots, CI reads, `tb pr …`)
//! goes through the same [`Gate`], since the host counts them together, along with the owner's other
//! tools on that account.
//!
//! # Backing off
//!
//! When the host says the account is limited (a 429; GitHub's "API rate limit exceeded" or secondary
//! limit; `X-RateLimit-Remaining: 0`), every call on that account stops until the host's
//! `Retry-After` / `X-RateLimit-Reset`, else for 5 minutes. Each limit that comes back doubles the
//! backoff, up to an hour (never shorter than the host's time); only an hour with no limit starts it
//! over at 5 minutes, since calls getting through between limits is what keeps the account limited.
//! A call made meanwhile fails at once with "Bitbucket rate-limited until 3:40pm" instead of reaching
//! the host. Each backoff logs a line with the board's own calls in the last hour, so it's clear
//! whether the board used the limit up. [`check`] (every runner tick) raises the `rate-limit:<host>`
//! alert once a run of limits has lasted 30 minutes inside the work hours, and clears it once the
//! account is past its limit and the slow restart.
//!
//! # Slowing down
//!
//! After a backoff, the account restarts slowly for as long as it backed off: only writes and the
//! reads marked [`priority`] (the PR poll's reads of the board's open PRs, whose stage can change) go;
//! every other read (the review sweep, master breaks, CI reads, …) waits, and [`stretch`] doubles the
//! PR poll's interval. The same happens before a limit when the host's headers say the account is
//! near it (Bitbucket's `X-RateLimit-NearLimit`, or `X-RateLimit-Remaining` under a fifth of
//! `X-RateLimit-Limit`), until its `X-RateLimit-Reset` or for 5 minutes.
//!
//! # Budget
//!
//! `[hosts.<host>] max_per_hour` caps the board's own calls on a host. Reads past it wait until the
//! hour's oldest call ages out (writes still go). Before that, [`stretch`] widens the PR poll and the
//! review sweep (`[intervals] prs` and `reviews`) so the last ten minutes' pace fits the budget.
//!
//! # Showing it
//!
//! [`status`]: each account's host, its limit (`limited_until`, `why`), its slow restart (`slow_text`),
//! the board's calls in the last hour and its budget, and the board's share of the calls in the hour
//! before the last limit (`share_text`), and what the last PR poll cost (`poll_text`, from [`counting`]
//! and [`Limits::note_poll`]), on `/state` (`rate_limits`), `GET /prs/feed` (`tb feed`) and
//! `POST /prs/refresh` (`tb pr check`).

use std::cell::{Cell, RefCell};
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
/// This long past a limit with no other limit starts the backoff over at 5 minutes.
const QUIET_SECS: f64 = HOUR;
/// Near the limit with no reset time from the host: slow down for this long.
const NEAR_SECS: f64 = 300.0;
/// Near the limit: under this share of the hour's calls left.
pub const NEAR_SHARE: f64 = 0.2;
/// How much wider the PR poll is while an account restarts slowly.
const SLOW_STRETCH: f64 = 2.0;
/// The board's share at the last limit is shown for a day.
const SHARE_SECS: f64 = 24.0 * HOUR;
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
    /// When this run of limits started (kept until a quiet hour).
    since: Option<f64>,
    /// Limits with less than [`QUIET_SECS`] between them: what doubles the backoff.
    strikes: u32,
    why: String,
    alerted: bool,
    /// Only writes and [`priority`] reads go until then: the slow restart after a backoff, or the host
    /// saying the account is near its limit.
    slow_until: Option<f64>,
    slow_why: String,
    /// When the last limit came, and the board's calls in the hour before it.
    share: Option<(f64, usize)>,
    /// The account's calls an hour, when the host says (`X-RateLimit-Limit`).
    cap: Option<u32>,
}

impl Account {
    fn forget_old(&mut self, now: f64) {
        while self.calls.front().is_some_and(|t| *t <= now - HOUR) {
            self.calls.pop_front();
        }
    }

    /// Past its limit and the slow restart after it.
    fn over(&self, now: f64) -> bool {
        self.until.is_none_or(|u| u <= now) && self.slow(now).is_none()
    }

    fn slow(&self, now: f64) -> Option<f64> {
        self.slow_until.filter(|s| *s > now && self.until.is_none_or(|u| u <= now))
    }

    /// The account's calls an hour: the host's word, else what the host documents.
    fn cap(&self) -> Option<u32> {
        self.cap.or(match self.host.as_str() {
            "bitbucket" => Some(1000),
            "github" => Some(5000),
            _ => None,
        })
    }

    /// "The board made 464 calls in the hour before Bitbucket limited the account …".
    fn share_text(&self, now: f64) -> Option<String> {
        let (at, calls) = self.share.filter(|(at, _)| now - at < SHARE_SECS)?;
        let mut text = format!("The board made {} in the hour before {} limited the account at {}.", plural(calls as i64, "call"), label(&self.host), local_clock(Some(&iso(at))));
        match self.cap() {
            Some(cap) if calls >= cap as usize => text += &format!(" That's the account's {cap} an hour: the board used it up itself."),
            _ => text += " The rest came from other tools on the same account.",
        }
        Some(text)
    }
}

/// What a host's headers say about the hour's calls left on a call that went through.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Headroom {
    /// `X-RateLimit-Limit`.
    pub cap: Option<u32>,
    /// Under [`NEAR_SHARE`] left, or the host flags it.
    pub near: bool,
    /// `X-RateLimit-Reset`, epoch seconds.
    pub reset_at: Option<f64>,
}

thread_local! {
    static PRIORITY: Cell<bool> = const { Cell::new(false) };
    static COUNTED: RefCell<Option<HashMap<String, usize>>> = const { RefCell::new(None) };
}

/// Runs `f` and counts the calls it made that reached each host, by host id.
pub fn counting<T>(f: impl FnOnce() -> T) -> (T, HashMap<String, usize>) {
    let was = COUNTED.with(|c| c.replace(Some(HashMap::new())));
    let out = f();
    let got = COUNTED.with(|c| c.replace(was)).unwrap_or_default();
    for (host, n) in &got {
        COUNTED.with(|c| c.borrow_mut().as_mut().map(|m| *m.entry(host.clone()).or_default() += n));
    }
    (out, got)
}

fn count(host: &str) {
    COUNTED.with(|c| c.borrow_mut().as_mut().map(|m| *m.entry(host.to_string()).or_default() += 1));
}

/// Runs `f` with its reads marked as the highest value: they still go while an account restarts
/// slowly after a limit, or is near one.
pub fn priority<T>(f: impl FnOnce() -> T) -> T {
    let was = PRIORITY.with(|p| p.replace(true));
    let out = f();
    PRIORITY.with(|p| p.set(was));
    out
}

fn is_priority() -> bool {
    PRIORITY.with(|p| p.get())
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
    /// The last PR poll by host.
    polls: Mutex<HashMap<String, Poll>>,
}

/// What one PR poll cost on a host.
#[derive(Debug, Clone, PartialEq)]
struct Poll {
    prs: usize,
    /// PRs read in full: the rest hadn't changed.
    full: usize,
    calls: usize,
    at: f64,
}

impl Poll {
    fn text(&self) -> String {
        let at = local_clock(Some(&iso(self.at)));
        let cost = format!("The last PR poll, at {at}, made {} for {}", plural(self.calls as i64, "call"), plural(self.prs as i64, "PR"));
        match self.full {
            0 => format!("{cost}: none had changed."),
            n if n == self.prs => format!("{cost} and read them all in full."),
            n => format!("{cost} and read {n} of them in full."),
        }
    }
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

    /// Whether a call on `key` may go out now; counts it when it does. `priority`: a read that still
    /// goes while the account restarts slowly. Err: why it waits.
    pub fn admit(&self, key: &str, host: &str, write: bool, priority: bool, now: f64) -> std::result::Result<(), String> {
        let budget = self.budget(host);
        let mut all = self.accounts.lock();
        let a = all.entry(key.to_string()).or_insert_with(|| Account { host: host.to_string(), ..Default::default() });
        a.forget_old(now);
        if let Some(u) = a.until {
            if u > now {
                return Err(format!("{}; the board holds its calls until then", until_text(host, u)));
            }
        }
        if let Some(s) = a.slow(now) {
            if !write && !priority {
                return Err(format!("{}; reads other than the PR poll's wait until {}", a.slow_why, local_clock(Some(&iso(s)))));
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

    /// The host says `key` is rate-limited: stop its calls until the host says, else back off, and
    /// restart slowly for as long after. Answers the time calls start again.
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
        if standing.is_none() && a.until.is_none_or(|u| now - u >= QUIET_SECS) {
            a.strikes = 0;
            a.since = None;
        }
        let doubled = now + (FIRST_BACKOFF_SECS * 2f64.powi(a.strikes.min(8) as i32)).min(MAX_BACKOFF_SECS);
        // A limit already standing doesn't back off again for the calls that were already out; only
        // a later time from the host moves it. A limit that came back waits at least the doubled time.
        let until = match (standing, told) {
            (Some(u), t) => u.max(t.unwrap_or(u)),
            (None, Some(t)) if a.strikes == 0 => t,
            (None, Some(t)) => t.max(doubled),
            (None, None) => doubled,
        };
        if standing.is_none() {
            a.strikes += 1;
            a.share = Some((now, a.calls.len()));
        }
        a.until = Some(until);
        a.since.get_or_insert(now);
        a.why = hit.why.clone();
        a.slow_until = Some(until + (until - now).max(FIRST_BACKOFF_SECS));
        a.slow_why = format!("{} limited the account until {}; the board is easing back in", label(host), local_clock(Some(&iso(until))));
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

    /// What the host's headers said on a call that went through: notes the account's hourly limit and,
    /// near it, slows down until the host's reset (else for [`NEAR_SECS`]).
    pub fn headroom(&self, key: &str, host: &str, h: &Headroom, now: f64) {
        let mut all = self.accounts.lock();
        let a = all.entry(key.to_string()).or_insert_with(|| Account { host: host.to_string(), ..Default::default() });
        if h.cap.is_some() {
            a.cap = h.cap;
        }
        if !h.near {
            return;
        }
        let until = h.reset_at.filter(|t| *t > now).unwrap_or(now + NEAR_SECS);
        if a.slow(now).is_none() {
            a.forget_old(now);
            self.log.lock().push(format!(
                "rate limit: {} says the account is near its limit; the board holds reads other than the PR poll's until {}; it made {} calls on this account in the last hour",
                label(host),
                local_clock(Some(&iso(until))),
                a.calls.len()
            ));
            a.slow_why = format!("{} says the account is near its rate limit", label(host));
        }
        a.slow_until = Some(a.slow_until.filter(|s| *s > now).unwrap_or(now).max(until));
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
        if self.accounts.lock().values().any(|a| a.slow(now).is_some()) {
            out = out.max(SLOW_STRETCH);
        }
        out
    }

    /// A PR poll on `host` looked at `prs` PRs, read `full` of them in full and made `calls` calls.
    pub fn note_poll(&self, host: &str, prs: usize, full: usize, calls: usize, now: f64) {
        self.polls.lock().insert(host.to_string(), Poll { prs, full, calls, at: now });
    }

    /// Each account the board has called: its host, limit and calls in the last hour.
    pub fn status(&self, now: f64) -> Vec<Value> {
        let stretch = self.stretch.lock().clone();
        let polls = self.polls.lock().clone();
        let mut all = self.accounts.lock();
        let mut out: Vec<Value> = all
            .values_mut()
            .map(|a| {
                a.forget_old(now);
                let limited = a.until.filter(|u| *u > now);
                let slow = a.slow(now);
                json!({
                    "host": a.host, "label": label(&a.host),
                    "limited_until": limited.map(iso),
                    "limited_since": limited.and(a.since).map(iso),
                    "why": limited.map(|_| a.why.clone()),
                    "text": limited.map(|u| until_text(&a.host, u)),
                    "slow_until": slow.map(iso),
                    "slow_text": slow.map(|s| format!("{}; reads other than the PR poll's wait until {}", a.slow_why, local_clock(Some(&iso(s))))),
                    "share_text": a.share_text(now),
                    "poll_text": polls.get(&a.host).map(Poll::text),
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

    /// Hosts with no account limited or restarting slowly any more, whose alert can go.
    fn lifted(&self, now: f64) -> Vec<String> {
        let all = self.accounts.lock();
        let mut hosts: Vec<String> = all.values().map(|a| a.host.clone()).collect();
        hosts.sort();
        hosts.dedup();
        hosts.retain(|h| !all.values().any(|a| &a.host == h && !a.over(now)));
        hosts
    }

    fn reset_alerts(&self, now: f64) {
        for a in self.accounts.lock().values_mut() {
            if a.over(now) {
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

    /// Makes the call unless the account is limited (or out of budget, or restarting slowly, for a
    /// read); notes a limit the call's answer reports (`limited` reads it from the error).
    pub fn call<T>(&self, write: bool, f: impl FnOnce() -> std::result::Result<(T, Option<Hit>), (String, Option<Hit>)>) -> std::result::Result<T, String> {
        let l = limits();
        l.admit(&self.key, &self.host, write, is_priority(), now_ts())?;
        count(&self.host);
        match f() {
            Ok((v, hit)) => {
                if let Some(h) = hit {
                    l.hit(&self.key, &self.host, &h, now_ts());
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

    /// The host's headers on a call that went through ([`Limits::headroom`]).
    pub fn headroom(&self, h: &Headroom) {
        limits().headroom(&self.key, &self.host, h, now_ts());
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
        l.admit("bb:1", "bitbucket", false, false, T0).unwrap();
        let until = l.hit("bb:1", "bitbucket", &Hit { retry_after: Some(120.0), ..hit("429") }, T0);
        assert_eq!(until, T0 + 120.0);
        let e = l.admit("bb:1", "bitbucket", false, false, T0 + 60.0).unwrap_err();
        assert!(e.starts_with("Bitbucket rate-limited until "), "{e}");
        assert!(l.admit("bb:1", "bitbucket", true, false, T0 + 60.0).is_err(), "writes wait too: the host would refuse them");
        assert!(l.admit("bb:2", "bitbucket", false, false, T0 + 60.0).is_ok(), "another account isn't limited");
        assert!(l.admit("bb:1", "bitbucket", false, true, T0 + 121.0).is_ok(), "the PR poll's reads go once it lifts");
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
    }

    #[test]
    fn calls_getting_through_between_limits_don_t_start_the_doubling_over() {
        let l = Limits::default();
        let mut t = T0;
        let mut waits = vec![];
        for _ in 0..3 {
            let u = l.hit("bb:1", "bitbucket", &hit("Bitbucket answered 429"), t);
            waits.push(((u - t) / 60.0).round() as i64);
            // The slow restart lets the PR poll through, and the limit comes back 20 minutes later.
            l.admit("bb:1", "bitbucket", false, true, u + 1.0).unwrap();
            t = u + 1200.0;
        }
        assert_eq!(waits, vec![5, 10, 20]);
        let until = l.hit("bb:1", "bitbucket", &hit("429"), t);
        let quiet = until + HOUR;
        assert_eq!(l.hit("bb:1", "bitbucket", &hit("429"), quiet) - quiet, 300.0, "an hour with no limit starts it over");
    }

    #[test]
    fn a_limit_that_comes_back_waits_at_least_the_doubled_time_whatever_the_host_says() {
        let l = Limits::default();
        let u = l.hit("bb:1", "bitbucket", &Hit { retry_after: Some(60.0), ..hit("429") }, T0);
        assert_eq!(u, T0 + 60.0, "the host's time, the first time");
        let t = u + 120.0;
        assert_eq!(l.hit("bb:1", "bitbucket", &Hit { retry_after: Some(60.0), ..hit("429") }, t), t + 600.0);
    }

    #[test]
    fn after_a_backoff_only_the_pr_poll_s_reads_and_writes_go_for_as_long() {
        let l = Limits::default();
        let until = l.hit("bb:1", "bitbucket", &hit("Bitbucket answered 429"), T0);
        let t = until + 1.0;
        let e = l.admit("bb:1", "bitbucket", false, false, t).unwrap_err();
        assert!(e.contains("Bitbucket limited the account until") && e.contains("reads other than the PR poll's wait until"), "{e}");
        assert!(l.admit("bb:1", "bitbucket", false, true, t).is_ok(), "a priority read goes");
        assert!(l.admit("bb:1", "bitbucket", true, false, t).is_ok(), "a write goes");
        assert_eq!(l.stretch(t), SLOW_STRETCH, "the poll runs slower meanwhile");
        let st = l.status(t);
        assert!(st[0]["slow_text"].as_str().unwrap().contains("easing back in"), "{}", st[0]);
        assert!(st[0]["text"].is_null());
        assert!(l.lifted(t).is_empty(), "the alert stays through the slow restart");
        let done = until + 301.0;
        assert!(l.admit("bb:1", "bitbucket", false, false, done).is_ok(), "every read goes after as long as it backed off");
        assert_eq!(l.stretch(done), 1.0);
        assert_eq!(l.lifted(done), vec!["bitbucket".to_string()]);
    }

    #[test]
    fn priority_marks_the_reads_inside_it() {
        assert!(!is_priority());
        assert!(priority(|| priority(is_priority) && is_priority()));
        assert!(!is_priority());
    }

    #[test]
    fn near_the_limit_it_slows_down_before_reaching_it() {
        let l = Limits::default();
        l.admit("bb:1", "bitbucket", false, false, T0).unwrap();
        l.headroom("bb:1", "bitbucket", &Headroom { cap: Some(1000), near: false, reset_at: None }, T0);
        assert!(l.admit("bb:1", "bitbucket", false, false, T0 + 1.0).is_ok(), "plenty left");
        l.headroom("bb:1", "bitbucket", &Headroom { cap: Some(1000), near: true, reset_at: Some(T0 + 900.0) }, T0 + 2.0);
        let e = l.admit("bb:1", "bitbucket", false, false, T0 + 3.0).unwrap_err();
        assert!(e.starts_with("Bitbucket says the account is near its rate limit"), "{e}");
        assert!(l.admit("bb:1", "bitbucket", false, true, T0 + 3.0).is_ok());
        let log = l.take_log();
        assert_eq!(log.len(), 1, "{log:?}");
        assert!(log[0].contains("near its limit"), "{}", log[0]);
        l.headroom("bb:1", "bitbucket", &Headroom { cap: Some(1000), near: true, reset_at: None }, T0 + 4.0);
        assert!(l.take_log().is_empty(), "logged once");
        assert!(l.admit("bb:1", "bitbucket", false, false, T0 + 901.0).is_ok(), "the host's reset ends it");
    }

    #[test]
    fn status_says_the_board_s_share_of_the_hour_before_the_limit() {
        let l = Limits::default();
        for i in 0..464 {
            l.admit("bb:1", "bitbucket", false, false, T0 + i as f64).unwrap();
        }
        l.hit("bb:1", "bitbucket", &hit("Bitbucket answered 429"), T0 + 600.0);
        let text = l.status(T0 + 700.0)[0]["share_text"].as_str().unwrap().to_string();
        assert!(text.starts_with("The board made 464 calls in the hour before Bitbucket limited the account at "), "{text}");
        assert!(text.ends_with("The rest came from other tools on the same account."), "{text}");
        l.headroom("bb:1", "bitbucket", &Headroom { cap: Some(400), ..Default::default() }, T0 + 700.0);
        assert!(l.status(T0 + 700.0)[0]["share_text"].as_str().unwrap().ends_with("the board used it up itself."));
        assert!(l.status(T0 + 600.0 + SHARE_SECS)[0]["share_text"].is_null(), "shown for a day");
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
            l.admit("bb:1", "bitbucket", false, false, T0 + i as f64).unwrap();
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
        l.admit("bb:1", "bitbucket", false, false, T0).unwrap();
        l.admit("bb:1", "bitbucket", false, false, T0 + 10.0).unwrap();
        let e = l.admit("bb:1", "bitbucket", false, false, T0 + 20.0).unwrap_err();
        assert!(e.contains("[hosts.bitbucket] max_per_hour"), "{e}");
        assert!(l.admit("bb:1", "bitbucket", true, false, T0 + 20.0).is_ok(), "a write still goes");
        assert!(l.admit("gh:1", "github", false, false, T0 + 20.0).is_ok(), "GitHub has no budget");
        assert!(l.admit("bb:1", "bitbucket", false, false, T0 + HOUR + 5.0).is_err(), "the write counts too");
        assert!(l.admit("bb:1", "bitbucket", false, false, T0 + HOUR + 11.0).is_ok(), "the hour's first two calls aged out");
    }

    #[test]
    fn the_poll_stretches_to_fit_the_budget() {
        let l = Limits::default();
        assert_eq!(l.stretch(T0), 1.0, "no budget, no stretch");
        l.set_budgets([("bitbucket".to_string(), 900)].into());
        // 300 calls in ten minutes: 1,800 an hour against 900 (aiming at 810).
        for i in 0..300 {
            l.admit("bb:1", "bitbucket", true, false, T0 + i as f64 * 2.0).unwrap();
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
        l.admit("bb:1", "bitbucket", false, false, T0).unwrap();
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
        assert!(l.lifted(t + 1.0).is_empty(), "still restarting slowly");
        assert_eq!(l.lifted(t + HOUR + 1.0), vec!["bitbucket".to_string()]);
    }

    #[test]
    fn counting_counts_the_calls_that_reached_each_host_inside_it_and_around_it() {
        let gate = Gate::new("counted-host", "counted:1".into());
        let call = || gate.call(false, || Ok(((), None))).unwrap();
        let ((inner, ()), outer) = counting(|| {
            call();
            let inner = counting(|| {
                call();
                call();
            });
            (inner.1, ())
        });
        assert_eq!(inner.get("counted-host"), Some(&2));
        assert_eq!(outer.get("counted-host"), Some(&3), "the inner count adds to the outer one");
        let refused = Gate::new("counted-host", "counted:2".into());
        limits().hit("counted:2", "counted-host", &hit("429"), now_ts());
        let ((), none) = counting(|| assert!(refused.call(false, || Ok(((), None))).is_err()));
        assert!(none.is_empty(), "a call the limit stopped never reached the host: {none:?}");
    }

    #[test]
    fn the_status_says_what_the_last_poll_cost() {
        let l = Limits::default();
        l.admit("bb:1", "bitbucket", false, false, T0).unwrap();
        assert!(l.status(T0)[0]["poll_text"].is_null(), "no poll yet");
        let text = |prs, full, calls| {
            l.note_poll("bitbucket", prs, full, calls, T0);
            let st = l.status(T0);
            st[0]["poll_text"].as_str().unwrap().split(", made ").nth(1).unwrap().to_string()
        };
        assert_eq!(text(20, 1, 2), "2 calls for 20 PRs and read 1 of them in full.");
        assert_eq!(text(20, 0, 1), "1 call for 20 PRs: none had changed.");
        assert_eq!(text(3, 3, 4), "4 calls for 3 PRs and read them all in full.");
        let st = l.status(T0);
        let at = st[0]["poll_text"].as_str().unwrap().split(", at ").nth(1).unwrap().split(',').next().unwrap().to_string();
        assert!(at.ends_with("am") || at.ends_with("pm"), "12-hour time: {at}");
    }
}
