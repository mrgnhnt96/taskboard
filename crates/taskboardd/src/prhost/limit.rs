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
//! the host; when the host gave no time, it says the time is the board's own backoff. Each backoff logs a line with the board's own calls in the last hour, so it's clear
//! whether the board used the limit up. [`check`] (every runner tick) raises the `rate-limit:<host>`
//! alert once a run of limits has lasted 30 minutes inside the work hours, and clears it once the
//! account is past its limit and the slow restart.
//!
//! # Slowing down
//!
//! After a backoff, the account restarts slowly for as long as it backed off: only writes and the
//! reads marked [`priority`] (the PR poll's reads of the board's open PRs, whose stage can change, and
//! the reads of a `tb pr …` command someone asked for) go; every other read (the review sweep, master
//! breaks, CI reads, …) waits, and [`stretch`] doubles the PR poll's interval. A read held meanwhile
//! says the host's limit is over and it's the board's own hold; `tb pr lift` ([`Limits::lift`]) ends it.
//! It doesn't hold the agents' PR visits or held replies ([`Limits::held_until`]): their writes and
//! reads go. The same happens before a limit when the host's headers say the account is
//! near it (Bitbucket's `X-RateLimit-NearLimit`, or `X-RateLimit-Remaining` under a fifth of
//! `X-RateLimit-Limit`), until its `X-RateLimit-Reset` or for 5 minutes.
//!
//! # Budget
//!
//! `[hosts.<host>] max_per_hour` caps the board's own calls on a host's account in any hour; unset,
//! it's 90% of what the host allows ([`default_budget`]: Bitbucket 900, GitHub 4,500), leaving the
//! rest to the owner's other tools; 0 turns it off. Reads stop a tenth short of it, keeping that for
//! writes; writes stop at it, so the board never runs the account past the cap itself. A call past it
//! waits until the hour's oldest call ages out. Before that, [`stretch`] widens the PR poll and the
//! review sweep (`[intervals] prs` and `reviews`) so the last ten minutes' pace fits the budget.
//!
//! # Pacing writes
//!
//! Hosts limit bursts of writes on top of the hourly count (three merges and five retargets in a few
//! seconds drew a 429 at 107 calls in the hour), so each account's writes go out one slot at a time,
//! `[hosts.<host>] write_gap_secs` apart (Bitbucket 2s, GitHub 1s by default): a write whose slot
//! hasn't come waits for it ([`Limits::write_slot`]). A merge that moves stacked PRs first is a queue
//! of paced writes, not a burst.
//!
//! # On the Days page
//!
//! Each stretch the board couldn't work with an account goes in `host_limits` ([`Span`], written on
//! [`check`]): `limited` (the host's limit), `slow` (easing back in, or near the limit) and `budget`
//! (the board's own hour used up). A stretch that runs on extends its row; `tb pr lift` ends them.
//!
//! # Across restarts
//!
//! The hour's calls, the limits and the slow restarts are kept in the `prhost_limits` setting
//! ([`check`] saves them at most every 30 seconds, and at once after a limit), so a daemon restart
//! neither forgets the hour's calls nor starts a limited account at full speed.
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
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;

/// `[hosts.<host>]` in config.toml (`github`, `bitbucket`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HostConfig {
    /// The most calls the board makes on the host's account in an hour; unset: [`default_budget`];
    /// 0: no cap.
    pub max_per_hour: Option<u32>,
    /// The least time between two writes on the account; unset: [`default_write_gap`].
    pub write_gap_secs: Option<f64>,
}

/// The board's own hourly budget on a host when `max_per_hour` isn't set: 90% of what the host
/// allows an account, so the owner's other tools on it keep the rest.
pub fn default_budget(host: &str) -> Option<u32> {
    match host {
        "bitbucket" => Some(900),
        "github" => Some(4500),
        _ => None,
    }
}

/// The least time between two writes on an account when `write_gap_secs` isn't set.
pub fn default_write_gap(host: &str) -> f64 {
    match host {
        "bitbucket" => 2.0,
        "github" => 1.0,
        _ => 0.0,
    }
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
/// Reads stop this share of the budget short of it, keeping the rest for writes.
const WRITE_RESERVE: f64 = 0.1;
/// The window a backoff's log line counts the burst before it over.
const BURST_SECS: f64 = 60.0;
/// The setting the limits are kept in across restarts.
const SAVED_KEY: &str = "prhost_limits";
/// The limits are saved at most this often, unless a limit came.
const SAVE_EVERY_SECS: f64 = 30.0;

/// One account on a host.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Account {
    /// `github`, `bitbucket`, …
    host: String,
    /// When the board's calls went out, in the last hour.
    calls: VecDeque<f64>,
    /// When the board's writes went out, in the last hour (also in `calls`).
    writes: VecDeque<f64>,
    /// The next write's slot: writes go out no closer than the host's write gap.
    next_write: f64,
    /// Calls stop until then.
    until: Option<f64>,
    /// `until` is the host's time (`Retry-After` / reset); false: the board's own backoff.
    host_time: bool,
    /// When this run of limits started (kept until a quiet hour).
    since: Option<f64>,
    /// Limits with less than [`QUIET_SECS`] between them: what doubles the backoff.
    strikes: u32,
    why: String,
    #[serde(skip)]
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
        while self.writes.front().is_some_and(|t| *t <= now - HOUR) {
            self.writes.pop_front();
        }
    }

    /// When the hour's calls fall back under `n`: once the oldest past it ages out.
    fn under_at(&self, n: usize, now: f64) -> f64 {
        // Under `n` once the hour's oldest `len - n + 1` calls have aged out; the last of them is at `len - n`.
        match self.calls.len().checked_sub(n.max(1)) {
            Some(i) => self.calls[i] + HOUR,
            None => now,
        }
    }

    /// "it made 107 calls on this account in the last hour, 12 in the last minute (8 writes)".
    fn pace_text(&self, now: f64) -> String {
        let burst = self.calls.iter().filter(|t| **t > now - BURST_SECS).count();
        let writes = self.writes.iter().filter(|t| **t > now - BURST_SECS).count();
        format!("it made {} calls on this account in the last hour, {burst} in the last minute ({})", self.calls.len(), plural(writes as i64, "write"))
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

/// A change to the stretches the board couldn't work with a host, for `host_limits`.
#[derive(Debug, Clone, PartialEq)]
enum Span {
    /// `kind` on `host` from `from` to `to`: joins the host's row of that kind it touches, else a new one.
    Open { host: String, kind: &'static str, from: f64, to: f64, why: String },
    /// The host's rows of `kind` (every kind when None) end at `at` if they ran past it.
    Cut { host: String, kind: Option<&'static str>, at: f64 },
}

/// Rows this close together are one stretch.
const SPAN_JOIN_SECS: f64 = 60.0;

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
    /// `[hosts.<host>]` by host.
    hosts: Mutex<BTreeMap<String, HostConfig>>,
    /// Something changed since the limits were last saved, and when they were.
    dirty: Mutex<(bool, f64)>,
    /// A limit came since they were saved: save at once.
    urgent: std::sync::atomic::AtomicBool,
    /// Changes to `host_limits`, written on the next [`check`].
    spans: Mutex<Vec<Span>>,
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
    /// The `Retry-After` and `X-RateLimit-*` headers it sent, as sent, for the log.
    pub headers: Vec<(String, String)>,
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

/// "Bitbucket rate-limited until 3:40pm" when the host gave the time; when the board picked it, says so.
fn until_text(host: &str, until: f64, host_time: bool) -> String {
    let at = local_clock(Some(&iso(until)));
    match host_time {
        true => format!("{} rate-limited until {at}", label(host)),
        false => format!("{} rate-limited the account and gave no time, so the board backs off until {at} (its own backoff; `tb pr lift` ends it)", label(host)),
    }
}

impl Limits {
    pub fn set_hosts(&self, hosts: BTreeMap<String, HostConfig>) {
        *self.hosts.lock() = hosts;
    }

    /// The board's hourly budget on `host`: `max_per_hour`, else [`default_budget`]; None: no cap.
    fn budget(&self, host: &str) -> Option<u32> {
        match self.hosts.lock().get(host).and_then(|c| c.max_per_hour) {
            Some(b) => Some(b).filter(|b| *b > 0),
            None => default_budget(host),
        }
    }

    fn write_gap(&self, host: &str) -> f64 {
        self.hosts.lock().get(host).and_then(|c| c.write_gap_secs).unwrap_or_else(|| default_write_gap(host)).max(0.0)
    }

    fn touch(&self) {
        self.dirty.lock().0 = true;
    }

    /// Takes the next write slot on `key`: how long a write made now waits for it (0: it goes now).
    /// Each write takes the slot after the last one's, so writes asked for together go out one by one.
    pub fn write_slot(&self, key: &str, host: &str, now: f64) -> f64 {
        let gap = self.write_gap(host);
        let mut all = self.accounts.lock();
        let a = all.entry(key.to_string()).or_insert_with(|| Account { host: host.to_string(), ..Default::default() });
        let at = a.next_write.max(now);
        a.next_write = at + gap;
        at - now
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
                return Err(match a.host_time {
                    true => format!("{}; the board holds its calls until then", until_text(host, u, true)),
                    false => until_text(host, u, false),
                });
            }
        }
        if let Some(s) = a.slow(now) {
            if !write && !priority {
                return Err(slow_text(&a.slow_why, s));
            }
        }
        if let Some(b) = budget {
            let (stop, what) = if write { (b as usize, "writes") } else { (read_stop(b), "reads") };
            if a.calls.len() >= stop {
                let free_at = a.under_at(stop, now);
                self.spans.lock().push(Span::Open {
                    host: host.to_string(),
                    kind: "budget",
                    from: now,
                    to: free_at,
                    why: format!("The board made {} of its {b} {} calls this hour; {what} waited", a.calls.len(), label(host)),
                });
                return Err(format!(
                    "the board made {} of its {b} {} calls this hour ([hosts.{host}] max_per_hour); {what} wait until {}",
                    a.calls.len(),
                    label(host),
                    local_clock(Some(&iso(free_at)))
                ));
            }
        }
        a.calls.push_back(now);
        if write {
            a.writes.push_back(now);
        }
        drop(all);
        self.touch();
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
        a.host_time = told == Some(until) || (standing == Some(until) && a.host_time);
        a.until = Some(until);
        a.since.get_or_insert(now);
        a.why = hit.why.clone();
        a.slow_until = Some(until + (until - now).max(FIRST_BACKOFF_SECS));
        a.slow_why = format!("{}'s limit on the account ended at {}; the board is easing back in", label(host), local_clock(Some(&iso(until))));
        let budget = self.budget(host).map(|b| format!(" (budget {b})")).unwrap_or_default();
        let source = if a.host_time { "the host's time" } else { "the board's backoff" };
        let headers = match hit.headers.is_empty() {
            true => "it sent no Retry-After or X-RateLimit headers".to_string(),
            false => format!("it sent {}", hit.headers.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join(", ")),
        };
        self.log.lock().push(format!(
            "rate limit: {} answered \"{}\" ({headers}); the board backs off until {} ({}, {source}); {}{budget}",
            label(host),
            one_line(&hit.why, 160),
            local_clock(Some(&iso(until))),
            dur_text(until - now),
            a.pace_text(now),
        ));
        let slow_until = a.slow_until.unwrap_or(until);
        let pace = a.pace_text(now);
        drop(all);
        let mut spans = self.spans.lock();
        spans.push(Span::Cut { host: host.to_string(), kind: Some("slow"), at: now });
        spans.push(Span::Open { host: host.to_string(), kind: "limited", from: now, to: until, why: format!("{} answered \"{}\"; {pace}", label(host), one_line(&hit.why, 160)) });
        spans.push(Span::Open { host: host.to_string(), kind: "slow", from: until, to: slow_until, why: format!("The board eased back in after {}'s limit", label(host)) });
        drop(spans);
        self.touch();
        self.urgent.store(true, std::sync::atomic::Ordering::SeqCst);
        until
    }

    /// `tb pr lift`: ends the board's own hold on `host`'s accounts (every host when None): the slow
    /// restart after a limit, and a limit the host set, since the owner says it's over. The next limit
    /// backs off from 5 minutes again. Answers a line per account it lifted.
    pub fn lift(&self, host: Option<&str>, now: f64) -> Vec<String> {
        let mut out = vec![];
        for a in self.accounts.lock().values_mut().filter(|a| host.is_none_or(|h| h == a.host)) {
            let held = a.until.filter(|u| *u > now).or_else(|| a.slow(now));
            if let Some(h) = held {
                out.push(format!("{}: lifted the board's hold, which ran until {}", label(&a.host), local_clock(Some(&iso(h)))));
            }
            self.spans.lock().push(Span::Cut { host: a.host.clone(), kind: None, at: now });
            a.until = a.until.filter(|u| *u <= now);
            a.slow_until = None;
            a.strikes = 0;
            a.since = None;
        }
        if !out.is_empty() {
            self.log.lock().push(format!("rate limit: the owner lifted the board's hold ({})", out.join("; ")));
            self.touch();
            self.urgent.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        out
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
        self.spans.lock().push(Span::Open { host: host.to_string(), kind: "slow", from: now, to: until, why: format!("{} said the account was near its rate limit", label(host)) });
        a.slow_until = Some(a.slow_until.filter(|s| *s > now).unwrap_or(now).max(until));
        drop(all);
        self.touch();
    }

    /// How much wider the PR poll should be so the board's pace on every budgeted host fits its
    /// budget: 1 when it fits (or there's no budget). Worked out at most once a minute per host.
    pub fn stretch(&self, now: f64) -> f64 {
        let mut hosts: Vec<String> = self.accounts.lock().values().map(|a| a.host.clone()).collect();
        hosts.sort();
        hosts.dedup();
        let budgets: Vec<(String, u32)> = hosts.into_iter().filter_map(|h| self.budget(&h).map(|b| (h, b))).collect();
        let mut out: f64 = 1.0;
        for (host, b) in budgets.iter() {
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

    /// When the account `key` takes writes again: the end of its limit, or of a spent budget. None
    /// when it isn't held. The slow restart after a limit doesn't hold it: writes and `tb pr` reads go
    /// then. Other accounts, and other hosts, don't count.
    pub fn held_until(&self, key: &str, now: f64) -> Option<f64> {
        let budget = self.budget(&self.accounts.lock().get(key)?.host);
        let mut all = self.accounts.lock();
        let a = all.get_mut(key)?;
        a.forget_old(now);
        let spent = budget.filter(|b| a.calls.len() >= *b as usize).map(|b| a.under_at(b as usize, now));
        a.until.filter(|u| *u > now).or(spent)
    }

    /// The accounts as kept across restarts.
    fn saved(&self) -> Value {
        json!(&*self.accounts.lock())
    }

    /// Takes back what [`Limits::saved`] kept, before this daemon made calls of its own: the last
    /// hour's calls, and limits and slow restarts that haven't ended.
    fn restore(&self, v: &Value, now: f64) {
        let Ok(saved) = serde_json::from_value::<HashMap<String, Account>>(v.clone()) else { return };
        let mut all = self.accounts.lock();
        for (key, mut a) in saved {
            a.forget_old(now);
            let here = all.entry(key).or_insert_with(|| Account { host: a.host.clone(), ..Default::default() });
            let mut calls: Vec<f64> = a.calls.iter().chain(here.calls.iter()).copied().collect();
            calls.sort_by(f64::total_cmp);
            let mut writes: Vec<f64> = a.writes.iter().chain(here.writes.iter()).copied().collect();
            writes.sort_by(f64::total_cmp);
            a.calls = calls.into();
            a.writes = writes.into();
            a.next_write = a.next_write.max(here.next_write);
            *here = a;
        }
    }

    /// Whether the limits should be saved now; marks them saved.
    fn due_save(&self, now: f64) -> bool {
        let urgent = self.urgent.swap(false, std::sync::atomic::Ordering::SeqCst);
        let mut d = self.dirty.lock();
        if !d.0 || !(urgent || now - d.1 >= SAVE_EVERY_SECS) {
            return false;
        }
        *d = (false, now);
        true
    }

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
                    "text": limited.map(|u| until_text(&a.host, u, a.host_time)),
                    "slow_until": slow.map(iso),
                    "slow_text": slow.map(|s| slow_text(&a.slow_why, s)),
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
                    let hold = match a.host_time {
                        true => "The board's PR reads wait until then.",
                        false => "That time is the board's own backoff, since the host gave none: `tb pr lift` ends it.",
                    };
                    out.push((a.host.clone(), format!("{} rate-limited the account {} ago, until {}: {}. {hold}", label(&a.host), dur_text(now - s), local_clock(Some(&iso(u))), one_line(&a.why, 160))));
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

    fn take_spans(&self) -> Vec<Span> {
        std::mem::take(&mut *self.spans.lock())
    }

    fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock())
    }
}

/// Reads stop here, keeping the rest of budget `b` for writes.
fn read_stop(b: u32) -> usize {
    (b as f64 * (1.0 - WRITE_RESERVE)).floor().max(1.0) as usize
}

/// What a read held by the slow restart says: whose hold it is, and how to end it.
fn slow_text(why: &str, until: f64) -> String {
    format!("{why}: it holds its background reads until {} (the PR poll and `tb pr` commands still go; `tb pr lift` ends it)", local_clock(Some(&iso(until))))
}

/// "4s": a short wait.
fn dur_secs_text(secs: f64) -> String {
    format!("{}s", secs.ceil().max(1.0) as i64)
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
        if write {
            let wait = l.write_slot(&self.key, &self.host, now_ts());
            if wait > 0.0 {
                // Said as progress, so a `tb pr` command queued behind other writes doesn't time out.
                let what = format!("a {} write slot ({} from now; writes go out one at a time)", label(&self.host), dur_secs_text(wait));
                crate::progress::waiting(&what, crate::progress::WAITING_EVERY, || std::thread::sleep(std::time::Duration::from_secs_f64(wait)));
            }
        }
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
                let host_time = l.accounts.lock().get(&self.key).is_some_and(|a| a.host_time);
                Err(format!("{e} ({})", until_text(&self.host, until, host_time)))
            }
            Err((e, None)) => Err(e),
        }
    }

    /// The host's headers on a call that went through ([`Limits::headroom`]).
    pub fn headroom(&self, h: &Headroom) {
        limits().headroom(&self.key, &self.host, h, now_ts());
    }
}

/// Writes the changes to `host_limits` ([`Span`]).
fn write_spans(app: &App, spans: Vec<Span>) -> Result<()> {
    if spans.is_empty() {
        return Ok(());
    }
    app.db.tx(|| {
        for s in &spans {
            match s {
                Span::Open { host, kind, from, to, why } => {
                    let near = app.db.q(
                        "SELECT id, from_at, to_at FROM host_limits WHERE host = ? AND kind = ? AND to_at >= ? AND from_at <= ? ORDER BY to_at DESC LIMIT 1",
                        crate::p![host, kind, iso(from - SPAN_JOIN_SECS), iso(to + SPAN_JOIN_SECS)],
                    )?;
                    match near.first() {
                        Some(r) => {
                            let a = r.s("from_at").and_then(parse_iso).unwrap_or(*from).min(*from);
                            let b = r.s("to_at").and_then(parse_iso).unwrap_or(*to).max(*to);
                            app.db.changed("UPDATE host_limits SET from_at = ?, to_at = ? WHERE id = ?", crate::p![iso(a), iso(b), r.id()])?;
                        }
                        None => {
                            app.db.insert("host_limits", vec![("host", json!(host)), ("kind", json!(kind)), ("from_at", json!(iso(*from))), ("to_at", json!(iso(*to))), ("why", json!(why))])?;
                        }
                    }
                }
                Span::Cut { host, kind, at } => {
                    let at = iso(*at);
                    app.db.changed("UPDATE host_limits SET to_at = ?1 WHERE host = ?2 AND (?3 IS NULL OR kind = ?3) AND to_at > ?1 AND from_at <= ?1", crate::p![at, host, kind])?;
                    app.db.changed("DELETE FROM host_limits WHERE host = ?2 AND (?3 IS NULL OR kind = ?3) AND from_at > ?1", crate::p![at, host, kind])?;
                }
            }
        }
        Ok(())
    })
}

/// Takes back the limits the last daemon kept, once ([`Limits::restore`]). The daemon also calls it
/// at startup, before its first call goes out.
pub fn restore(app: &App) {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        limits().set_hosts(app.cfg.hosts.clone());
        if let Some(v) = app.db.get_setting(SAVED_KEY).ok().flatten().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
            limits().restore(&v, now_ts());
        }
    });
}

/// `/state`'s `rate_limits`, and `tb feed`'s.
pub fn status() -> Vec<Value> {
    limits().status(now_ts())
}

/// When the account `key` takes the board's calls again, while it's limited ([`Limits::held_until`]).
pub fn held_until(key: &str) -> Option<f64> {
    limits().held_until(key, now_ts())
}

/// `POST /prs/limits/lift` (`tb pr lift [--host <host>]`): ends the board's own hold ([`Limits::lift`]).
pub fn lift(app: &App, body: &Value) -> Result<Value> {
    let host = body["host"].as_str().map(|h| h.trim().to_lowercase()).filter(|h| !h.is_empty());
    let lifted = limits().lift(host.as_deref(), now_ts());
    save(app, now_ts())?;
    write_spans(app, limits().take_spans())?;
    for line in limits().take_log() {
        app.info(line);
    }
    for h in limits().lifted(now_ts()) {
        crate::dispatch::clear_alert_key(app, &format!("{ALERT_PREFIX}{h}"))?;
    }
    Ok(json!({"lifted": lifted, "rate_limits": status()}))
}

fn save(app: &App, now: f64) -> Result<()> {
    limits().due_save(now);
    app.db.set_setting(SAVED_KEY, Some(&limits().saved().to_string()))
}

/// How much to widen the PR poll and the review sweep ([`Limits::stretch`]).
pub fn stretch() -> f64 {
    limits().stretch(now_ts())
}

/// Every runner tick: takes in `[hosts]`, logs the backoffs since the last tick, raises the alert for a
/// limit that's lasted 30 minutes inside the work hours, and clears it once the limit is over.
pub fn check(app: &App) -> Result<()> {
    let l = limits();
    l.set_hosts(app.cfg.hosts.clone());
    restore(app);
    if l.due_save(now_ts()) {
        app.db.set_setting(SAVED_KEY, Some(&l.saved().to_string()))?;
    }
    for line in l.take_log() {
        app.info(line);
    }
    write_spans(app, l.take_spans())?;
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
    fn held_until_is_the_account_s_own_limit_alone() {
        let l = Limits::default();
        assert_eq!(l.held_until("bb:1", T0), None);
        l.admit("gh:1", "github", false, false, T0).unwrap();
        l.hit("bb:1", "bitbucket", &Hit { retry_after: Some(120.0), ..hit("429") }, T0);
        assert_eq!(l.held_until("bb:1", T0 + 60.0), Some(T0 + 120.0));
        assert_eq!(l.held_until("bb:2", T0 + 60.0), None, "another account on the host isn't held");
        assert_eq!(l.held_until("gh:1", T0 + 60.0), None, "another host isn't held");
        assert_eq!(l.held_until("bb:1", T0 + 121.0), None, "the board's slow restart after it doesn't hold PR visits or replies");
        assert!(l.admit("bb:1", "bitbucket", false, false, T0 + 121.0).is_err(), "it still holds background reads");
        assert!(l.admit("bb:1", "bitbucket", true, false, T0 + 121.0).is_ok(), "writes go");
    }

    #[test]
    fn a_limit_with_no_time_from_the_host_says_the_board_picked_it() {
        let l = Limits::default();
        let until = l.hit("bb:1", "bitbucket", &hit("429"), T0);
        let e = l.admit("bb:1", "bitbucket", true, false, T0 + 60.0).unwrap_err();
        assert!(e.starts_with("Bitbucket rate-limited the account and gave no time, so the board backs off until "), "{e}");
        assert!(e.contains("its own backoff; `tb pr lift` ends it"), "{e}");
        assert!(l.status(T0 + 60.0)[0]["text"].as_str().unwrap().contains("the board backs off until"));
        assert!(l.take_log()[0].contains("the board's backoff"));
        l.hit("bb:1", "bitbucket", &Hit { retry_after: Some(until - T0 + 600.0), ..hit("429") }, T0 + 10.0);
        let e = l.admit("bb:1", "bitbucket", false, false, T0 + 60.0).unwrap_err();
        assert!(e.starts_with("Bitbucket rate-limited until "), "a later time from the host is the host's: {e}");
        l.hit("bb:1", "bitbucket", &hit("429"), T0 + 20.0);
        let e = l.admit("bb:1", "bitbucket", false, false, T0 + 60.0).unwrap_err();
        assert!(e.starts_with("Bitbucket rate-limited until "), "a call already out doesn't make it the board's: {e}");
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
        assert!(e.starts_with("Bitbucket's limit on the account ended at "), "says the host's limit is over: {e}");
        assert!(e.contains("it holds its background reads until") && e.contains("`tb pr lift` ends it"), "{e}");
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

    fn budget(host: &str, b: u32) -> BTreeMap<String, HostConfig> {
        [(host.to_string(), HostConfig { max_per_hour: Some(b), ..Default::default() })].into()
    }

    #[test]
    fn the_budget_keeps_a_tenth_for_writes_and_stops_writes_at_it() {
        let l = Limits::default();
        l.set_hosts(budget("bitbucket", 20));
        for i in 0..18 {
            l.admit("bb:1", "bitbucket", false, false, T0 + i as f64).unwrap();
        }
        let e = l.admit("bb:1", "bitbucket", false, false, T0 + 20.0).unwrap_err();
        assert!(e.contains("made 18 of its 20 Bitbucket calls") && e.contains("[hosts.bitbucket] max_per_hour") && e.contains("reads wait until"), "{e}");
        l.admit("bb:1", "bitbucket", true, false, T0 + 20.0).unwrap();
        l.admit("bb:1", "bitbucket", true, false, T0 + 21.0).unwrap();
        let e = l.admit("bb:1", "bitbucket", true, false, T0 + 22.0).unwrap_err();
        assert!(e.contains("writes wait until"), "the board never runs past its budget itself: {e}");
        assert_eq!(l.held_until("bb:1", T0 + 22.0), Some(T0 + HOUR), "a spent budget holds the account until its first call ages out");
        assert!(l.admit("bb:1", "bitbucket", true, false, T0 + HOUR + 0.5).is_ok(), "the hour's first call aged out");
        let spans = l.take_spans();
        assert!(spans.iter().any(|s| matches!(s, Span::Open { kind: "budget", .. })), "{spans:?}");
    }

    #[test]
    fn budgets_default_to_nine_tenths_of_the_host_s_and_0_turns_them_off() {
        let l = Limits::default();
        assert_eq!(l.budget("bitbucket"), Some(900));
        assert_eq!(l.budget("github"), Some(4500));
        assert_eq!(l.budget("azure devops"), None);
        l.set_hosts(budget("bitbucket", 0));
        assert_eq!(l.budget("bitbucket"), None);
        l.set_hosts(budget("bitbucket", 500));
        assert_eq!(l.budget("bitbucket"), Some(500));
    }

    #[test]
    fn writes_on_an_account_go_out_one_slot_apart() {
        let l = Limits::default();
        assert_eq!(l.write_slot("bb:1", "bitbucket", T0), 0.0, "the first goes now");
        assert_eq!(l.write_slot("bb:1", "bitbucket", T0 + 0.5), 1.5, "the next waits out the 2s gap");
        assert_eq!(l.write_slot("bb:1", "bitbucket", T0 + 0.5), 3.5, "and the one after, its own slot");
        assert_eq!(l.write_slot("bb:2", "bitbucket", T0 + 0.5), 0.0, "another account has its own slots");
        assert_eq!(l.write_slot("bb:1", "bitbucket", T0 + 60.0), 0.0, "a quiet account writes at once");
        assert_eq!(l.write_slot("gh:1", "github", T0), 0.0);
        assert_eq!(l.write_slot("gh:1", "github", T0), 1.0, "GitHub's gap is a second");
        l.set_hosts([("bitbucket".to_string(), HostConfig { write_gap_secs: Some(5.0), ..Default::default() })].into());
        assert_eq!(l.write_slot("bb:3", "bitbucket", T0), 0.0);
        assert_eq!(l.write_slot("bb:3", "bitbucket", T0), 5.0, "write_gap_secs sets it");
    }

    #[test]
    fn a_backoff_logs_the_host_s_headers_and_the_burst_before_it() {
        let l = Limits::default();
        for i in 0..5 {
            l.admit("bb:1", "bitbucket", i % 2 == 0, false, T0 + i as f64).unwrap();
        }
        let h = Hit { retry_after: Some(120.0), headers: vec![("Retry-After".into(), "120".into()), ("X-RateLimit-Limit".into(), "1000".into())], ..hit("Bitbucket answered 429") };
        l.hit("bb:1", "bitbucket", &h, T0 + 10.0);
        let line = l.take_log().remove(0);
        assert!(line.contains("it sent Retry-After: 120, X-RateLimit-Limit: 1000"), "{line}");
        assert!(line.contains("the host's time"), "{line}");
        assert!(line.contains("5 calls on this account in the last hour, 5 in the last minute (3 writes)"), "{line}");
        l.hit("bb:2", "bitbucket", &hit("Bitbucket answered 429"), T0);
        let line = l.take_log().remove(0);
        assert!(line.contains("it sent no Retry-After or X-RateLimit headers") && line.contains("the board's backoff"), "{line}");
    }

    #[test]
    fn a_limit_opens_a_limited_stretch_and_the_slow_one_after_it() {
        let l = Limits::default();
        let until = l.hit("bb:1", "bitbucket", &Hit { retry_after: Some(600.0), ..hit("429") }, T0);
        let spans = l.take_spans();
        assert_eq!(spans[0], Span::Cut { host: "bitbucket".into(), kind: Some("slow"), at: T0 }, "a limit ends the slow stretch it came in");
        assert!(matches!(&spans[1], Span::Open { kind: "limited", from, to, .. } if *from == T0 && *to == until), "{spans:?}");
        assert!(matches!(&spans[2], Span::Open { kind: "slow", from, to, .. } if *from == until && *to == until + 600.0), "{spans:?}");
    }

    #[test]
    fn lift_ends_the_board_s_hold_and_starts_the_backoff_over() {
        let l = Limits::default();
        let mut t = T0;
        for _ in 0..3 {
            t = l.hit("bb:1", "bitbucket", &hit("429"), t) + 1.0;
        }
        l.hit("gh:1", "github", &hit("x"), t);
        l.take_spans();
        assert!(l.admit("bb:1", "bitbucket", false, false, t).is_err(), "easing back in");
        let lifted = l.lift(Some("bitbucket"), t);
        assert_eq!(lifted.len(), 1, "{lifted:?}");
        assert!(lifted[0].starts_with("Bitbucket: lifted the board's hold"), "{}", lifted[0]);
        assert!(l.admit("bb:1", "bitbucket", false, false, t).is_ok());
        assert!(l.admit("gh:1", "github", false, false, t).is_err(), "only the host named");
        assert_eq!(l.take_spans(), vec![Span::Cut { host: "bitbucket".into(), kind: None, at: t }]);
        assert_eq!(l.hit("bb:1", "bitbucket", &hit("429"), t + 10.0) - (t + 10.0), 300.0, "the next limit backs off from 5 minutes");
        assert!(l.lift(Some("bitbucket"), T0 + 10.0 * HOUR).is_empty(), "nothing held, nothing lifted");
    }

    #[test]
    fn the_hour_s_calls_and_a_hold_survive_a_restart() {
        let before = Limits::default();
        for i in 0..40 {
            before.admit("bb:1", "bitbucket", false, false, T0 + i as f64).unwrap();
        }
        let until = before.hit("bb:1", "bitbucket", &hit("429"), T0 + 50.0);
        let saved = before.saved();
        let after = Limits::default();
        after.admit("bb:1", "bitbucket", false, true, T0 - HOUR).unwrap();
        after.restore(&saved, T0 + 60.0);
        assert!(after.admit("bb:1", "bitbucket", true, false, T0 + 60.0).is_err(), "still limited after the restart");
        assert_eq!(after.held_until("bb:1", T0 + 60.0), Some(until));
        assert_eq!(after.status(T0 + 60.0)[0]["calls_last_hour"], 40, "the hour's calls came back; the one older than an hour aged out");
        let next = after.hit("bb:1", "bitbucket", &hit("429"), until + 1.0);
        assert_eq!(next - (until + 1.0), 2.0 * FIRST_BACKOFF_SECS, "the backoff step came back too");
        let later = Limits::default();
        later.restore(&saved, T0 + 2.0 * HOUR + 120.0);
        assert!(later.admit("bb:1", "bitbucket", false, false, T0 + 2.0 * HOUR + 120.0).is_ok(), "an old hold doesn't come back");
    }

    #[test]
    fn stretches_join_the_row_they_touch_and_a_cut_ends_them() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::for_tests(crate::config::Config::for_tests(dir.path()));
        let open = |kind, from: f64, to: f64| Span::Open { host: "bitbucket".into(), kind, from, to, why: "429".into() };
        write_spans(&app, vec![open("limited", T0, T0 + 300.0), open("slow", T0 + 300.0, T0 + 600.0)]).unwrap();
        // The limit came back while it stood: the same row runs on.
        write_spans(&app, vec![open("limited", T0 + 200.0, T0 + 900.0)]).unwrap();
        // The budget ran out much later: a row of its own.
        write_spans(&app, vec![open("budget", T0 + 5000.0, T0 + 5100.0), open("budget", T0 + 5130.0, T0 + 5200.0)]).unwrap();
        let rows = || {
            app.db.q("SELECT kind, from_at, to_at FROM host_limits ORDER BY from_at, kind", crate::p![]).unwrap().iter().map(|r| (r.st("kind"), parse_iso(r.s("from_at").unwrap()).unwrap() - T0, parse_iso(r.s("to_at").unwrap()).unwrap() - T0)).collect::<Vec<_>>()
        };
        assert_eq!(rows(), vec![("limited".into(), 0.0, 900.0), ("slow".into(), 300.0, 600.0), ("budget".into(), 5000.0, 5200.0)]);
        write_spans(&app, vec![Span::Cut { host: "bitbucket".into(), kind: None, at: T0 + 400.0 }]).unwrap();
        assert_eq!(rows(), vec![("limited".into(), 0.0, 400.0), ("slow".into(), 300.0, 400.0)], "lifted at 400: what ran past it ends there, what came after goes");
    }

    #[test]
    fn the_limits_are_saved_every_30_seconds_and_at_once_after_a_limit() {
        let l = Limits::default();
        assert!(!l.due_save(T0), "nothing changed");
        l.admit("bb:1", "bitbucket", false, false, T0).unwrap();
        assert!(l.due_save(T0 + 31.0));
        l.admit("bb:1", "bitbucket", false, false, T0 + 32.0).unwrap();
        assert!(!l.due_save(T0 + 40.0), "saved under 30 seconds ago");
        l.hit("bb:1", "bitbucket", &hit("429"), T0 + 41.0);
        assert!(l.due_save(T0 + 41.0), "a limit saves at once");
    }

    #[test]
    fn the_poll_stretches_to_fit_the_budget() {
        let l = Limits::default();
        assert_eq!(l.stretch(T0), 1.0, "no calls, no stretch");
        l.set_hosts(budget("bitbucket", 900));
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
