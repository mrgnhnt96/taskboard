//! The PR hosts' rate limits, kept per account for the whole daemon: every reader that signs in as the
//! same account (the PR poll, master breaks, the owner's PR builds, review bots, CI reads, `tb pr …`)
//! goes through the same [`Gate`], since the host counts them together, along with the owner's other
//! tools on that account.
//!
//! # Backing off
//!
//! When the host says the account is limited (a 429; GitHub's "API rate limit exceeded" or secondary
//! limit; `X-RateLimit-Remaining: 0`), every call on that account stops until the host's
//! `Retry-After` / `X-RateLimit-Reset`, else for a flat 10 minutes, however often the limit comes back:
//! the host's limit rolls by the hour, so waiting longer only wastes the calls that would get through.
//! A call made meanwhile fails at once with "Bitbucket rate-limited until 3:40pm" instead of reaching
//! the host; when the host gave no time, it says the time is the board's own backoff. Each backoff logs a line with the board's own calls in the last hour, so it's clear
//! whether the board used the limit up. [`check`] (every runner tick) raises the `rate-limit:<host>`
//! alert once a run of limits has lasted 30 minutes inside the work hours, and clears it once the
//! account is past its limit and the slow restart.
//!
//! # Slowing down
//!
//! After a backoff, the account restarts slowly for 10 minutes: only writes and the
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
//! of paced writes, not a burst. The line is bounded: past [`WRITE_LINE`] writes waiting, a write
//! fails at once saying the account's write queue is full, rather than parking one more thread.
//!
//! # Pacing reads
//!
//! Reads are capped per account per minute, `[hosts.<host>] max_reads_per_minute` (Bitbucket 30 by
//! default; unset elsewhere, 0 turns it off): never more than that in any 60 seconds
//! ([`Limits::read_turn`]). [`priority`] reads and writes go ahead of background reads: a priority
//! read takes the next free turn in line (failing at once when that's over [`PRIORITY_WAIT_SECS`]
//! away), while a background read keeps a fifth of the minute free for them, and doesn't start while
//! a priority read or a queued write waits on the account. A background read that can't go within
//! [`BACKGROUND_WAIT_SECS`] is skipped with an error; the next poll asks again.
//!
//! # Sharing reads
//!
//! The same read on an account goes out once ([`Gate::request`] with a GET): a read whose URL is
//! already in flight waits for that answer instead of sending its own, and an answer is kept for
//! [`KEEP_SECS`] for the same read again. Only the read that goes counts in the limits. A write drops
//! the reads it may change once it's done, whether or not it went through ([`Limits::wrote`]): a write
//! to a PR (`…/pullrequests/<n>…`) drops that PR's reads and the account's reads of no one PR (lists,
//! searches, commit statuses); any other write drops all the account's reads. A read in flight then
//! still answers those already waiting on it, but isn't kept and later reads don't wait on it. A
//! host's error is shared with those waiting but not kept, nor is an empty answer (no body, or one
//! that isn't JSON); a read that never reached the host (skipped, or the account limited) lets each
//! one waiting try itself.
//!
//! Others change a PR too (a push, a review, a build), and no write here says so. Reads made to see
//! such a change ([`fresh`]: after an event or a hint, and `tb pr` commands) take only an answer sent
//! after they began: one kept from before, or in flight since before, is read again.
//!
//! # Collapsing writes
//!
//! A write that only sets a PR's state (its reviewers, its base, its description) is marked
//! [`last_wins`] with the PR and the field it sets. While one waits in the line for its slot, a newer
//! one with the same key on the account replaces it ([`Limits::queue_last`]): the newer one takes the
//! older one's place (it goes out at that slot, and doesn't take one of its own, so it never finds the
//! line full), the older one never reaches the host, and its caller gets the newer one's answer. A
//! write already gone out isn't replaced; the next one waits for a slot of its own. Merges, comments,
//! approvals and anything else not marked never collapse: each goes out.
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
//! the board's calls in the last hour and its budget, split by the reader that made them
//! (`calls_by_reader`, `readers_text`), the board's share of the calls in the hour before the last
//! limit (`share_text`), what the last PR poll cost (`poll_text`, from [`counting`] and
//! [`Limits::note_poll`]), and the last `X-Request-Count` Bitbucket sent (`request_count_text`), on
//! `/state` (`rate_limits`), `GET /prs/limits` (`tb pr limits`), `GET /prs/feed` (`tb feed`) and
//! `POST /prs/refresh` (`tb pr check`).
//!
//! # Who made the calls
//!
//! Each call is counted against the reader that made it ([`reader`]: the timed PR poll, its full-read
//! fallback when the host couldn't say what changed, reads after a review hint, the master watch, the
//! review sweep, and `tb` commands); calls made outside one count as `other`. The `rate limit:` line
//! a limit logs gives the split for the hour and the minute, with the last `X-Request-Count` Bitbucket
//! sent: its own count of the account's calls, which takes in other tools on the same account. The
//! board only shows that count; it doesn't pace on it. Once an hour the log gets a line with the
//! hour's calls and their split ([`Limits::hourly`]).

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, OnceLock};

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
    /// The most reads the board makes on the account in a minute; unset:
    /// [`default_reads_per_minute`]; 0: no cap.
    pub max_reads_per_minute: Option<u32>,
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

/// The board's reads a minute on an account when `max_reads_per_minute` isn't set: Bitbucket drew
/// 429s at 55 and 120 calls in a minute.
pub fn default_reads_per_minute(host: &str) -> Option<u32> {
    match host {
        "bitbucket" => Some(30),
        _ => None,
    }
}

/// The most writes waiting on an account; one more fails at once.
pub const WRITE_LINE: usize = 10;
/// The window the read cap counts over.
const READ_WINDOW_SECS: f64 = 60.0;
/// The share of the minute's reads kept for priority reads.
const PRIORITY_SHARE: f64 = 0.2;
/// A priority read whose turn is further off than this fails at once.
pub const PRIORITY_WAIT_SECS: f64 = 60.0;
/// A background read that can't go within this is skipped.
pub const BACKGROUND_WAIT_SECS: f64 = 5.0;
/// How long a read's answer is kept for the same read on the account.
pub const KEEP_SECS: f64 = 30.0;

/// A limit with no `Retry-After` / reset: a flat 10 minutes, every time.
const BACKOFF_SECS: f64 = 600.0;
/// The slow restart after a backoff.
const SLOW_SECS: f64 = 600.0;
const HOUR: f64 = 3600.0;
/// This long past a limit with no other limit ends the run of limits the alert counts.
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
    /// The reads' turns in the last minute, in order; a priority read's turn can be ahead of now.
    #[serde(skip)]
    reads: VecDeque<f64>,
    /// Calls stop until then.
    until: Option<f64>,
    /// `until` is the host's time (`Retry-After` / reset); false: the board's own backoff.
    host_time: bool,
    /// When this run of limits started (kept until a quiet hour).
    since: Option<f64>,
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
    /// The reader named for each call in `calls` that went out inside one ([`reader`]).
    by_reader: VecDeque<(f64, String)>,
    /// The last `X-Request-Count` the host sent, and when.
    request_count: Option<(u64, f64)>,
    /// When the hourly calls line was last logged ([`Limits::hourly`]).
    #[serde(skip)]
    logged_at: Option<f64>,
}

impl Account {
    fn forget_old(&mut self, now: f64) {
        while self.calls.front().is_some_and(|t| *t <= now - HOUR) {
            self.calls.pop_front();
        }
        while self.writes.front().is_some_and(|t| *t <= now - HOUR) {
            self.writes.pop_front();
        }
        while self.by_reader.front().is_some_and(|(t, _)| *t <= now - HOUR) {
            self.by_reader.pop_front();
        }
        while self.reads.front().is_some_and(|t| *t <= now - READ_WINDOW_SECS) {
            self.reads.pop_front();
        }
    }

    /// Writes waiting for their slot: the slots taken past the one that's next.
    fn write_line(&self, gap: f64, now: f64) -> usize {
        match gap > 0.0 && self.next_write > now {
            true => (((self.next_write - now) / gap).ceil() as usize).saturating_sub(1),
            false => 0,
        }
    }

    /// The first time at or after `now` a read keeps the minute's reads within `stop`.
    fn read_free_at(&self, stop: usize, now: f64) -> f64 {
        match self.reads.len().checked_sub(stop.max(1)) {
            Some(i) => (self.reads[i] + READ_WINDOW_SECS).max(now),
            None => now,
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

    /// "it made 107 calls on this account in the last hour (poll 60, hint 40, agent 7), 12 in the last
    /// minute (8 writes; poll 4, agent 8)".
    fn pace_text(&self, now: f64) -> String {
        let burst = self.calls.iter().filter(|t| **t > now - BURST_SECS).count();
        let writes = self.writes.iter().filter(|t| **t > now - BURST_SECS).count();
        let hour = self.readers_text(now - HOUR).map(|s| format!(" ({s})")).unwrap_or_default();
        let minute = self.readers_text(now - BURST_SECS).map(|s| format!("; {s}")).unwrap_or_default();
        format!("it made {} calls on this account in the last hour{hour}, {burst} in the last minute ({}{minute})", self.calls.len(), plural(writes as i64, "write"))
    }

    /// The calls since `since` by reader, in [`READERS`] order, the rest as `other`; empty with none.
    fn by_reader(&self, since: f64) -> Vec<(String, usize)> {
        let total = self.calls.iter().filter(|t| **t > since).count();
        let mut n: BTreeMap<&str, usize> = BTreeMap::new();
        for (_, r) in self.by_reader.iter().filter(|(t, _)| *t > since) {
            *n.entry(r.as_str()).or_default() += 1;
        }
        let named: usize = n.values().sum();
        let mut out: Vec<(String, usize)> = READERS.iter().filter_map(|r| n.remove(r).map(|c| (r.to_string(), c))).collect();
        out.extend(n.into_iter().map(|(r, c)| (r.to_string(), c)));
        if total > named {
            out.push(("other".to_string(), total - named));
        }
        out
    }

    /// "poll 60, hint 40, agent 7": the calls since `since` by reader; None while none was made inside
    /// a [`reader`].
    fn readers_text(&self, since: f64) -> Option<String> {
        let split = self.by_reader(since);
        if split.iter().all(|(r, _)| r == "other") {
            return None;
        }
        Some(split.iter().map(|(r, c)| format!("{r} {c}")).collect::<Vec<_>>().join(", "))
    }

    /// "Bitbucket's x-request-count said 2,189 at 11:13:35am".
    fn request_count_text(&self) -> Option<String> {
        let (n, at) = self.request_count?;
        Some(format!("{}'s x-request-count said {} at {}", label(&self.host), grouped(n), clock_secs(at)))
    }

    /// "the board made 436 calls on this account in the last hour (poll 210, …); Bitbucket's
    /// x-request-count said 2,189 at 11:13:35am".
    fn hour_text(&self, now: f64) -> String {
        let split = self.readers_text(now - HOUR).map(|s| format!(" ({s})")).unwrap_or_default();
        let told = self.request_count_text().map(|t| format!("; {t}")).unwrap_or_default();
        format!("the board made {} on this account in the last hour{split}{told}", plural(self.calls.len() as i64, "call"))
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
    static FRESH: Cell<Option<f64>> = const { Cell::new(None) };
    static COUNTED: RefCell<Option<HashMap<String, usize>>> = const { RefCell::new(None) };
    static LAST_WINS: RefCell<Option<String>> = const { RefCell::new(None) };
    static READER: Cell<Option<&'static str>> = const { Cell::new(None) };
}

/// The readers [`reader`] names, in the order the split shows them.
pub const READERS: [&str; 6] = ["poll", "full-read fallback", "hint", "master", "reviewers", "agent"];

/// Runs `f` with the calls it makes counted against `name` (one of [`READERS`]) in the split of an
/// account's calls (see "Who made the calls"); an inner one names its own calls.
pub fn reader<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    let was = READER.with(|r| r.replace(Some(name)));
    let out = f();
    READER.with(|r| r.set(was));
    out
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

/// Runs `f` with its reads taking only answers sent from now on (see "Sharing reads"): for reads made
/// to see what changed on the host.
pub fn fresh<T>(f: impl FnOnce() -> T) -> T {
    let was = FRESH.with(|s| s.replace(Some(now_ts())));
    let out = f();
    FRESH.with(|s| s.set(was));
    out
}

/// Since when [`fresh`] reads take answers; the dawn of time outside one.
fn fresh_since() -> f64 {
    FRESH.with(|s| s.get()).unwrap_or(f64::NEG_INFINITY)
}

/// Runs `f` with its writes marked last-wins under `key`, the PR and the field they set (`<PR>
/// reviewers`): each only sets state, so a newer one with the same key replaces one still waiting for
/// its slot (see "Collapsing writes"). Never for a merge, a comment or an approval.
pub fn last_wins<T>(key: String, f: impl FnOnce() -> T) -> T {
    /// Puts back the key outside, even when `f` panics: a pooled thread mustn't keep marking writes.
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            LAST_WINS.with(|k| k.replace(self.0.take()));
        }
    }
    let _restore = Restore(LAST_WINS.with(|k| k.replace(Some(key))));
    f()
}

/// The key the current thread's writes are marked with, set by an enclosing [`last_wins`], if any.
pub(super) fn last_wins_key() -> Option<String> {
    LAST_WINS.with(|k| k.borrow().clone())
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
    /// Reads in flight or answered lately, by account and URL ([`Gate::request`]).
    shared: Mutex<HashMap<(String, String), Shared>>,
    /// Last-wins writes waiting for their slot, by account, key and the answer's type
    /// ([`Limits::queue_last`]).
    queued: Mutex<HashMap<(String, String, TypeId), Queued>>,
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

    /// The most reads on `host` in a minute: `max_reads_per_minute`, else [`default_reads_per_minute`];
    /// None: no cap.
    fn reads_per_minute(&self, host: &str) -> Option<u32> {
        match self.hosts.lock().get(host).and_then(|c| c.max_reads_per_minute) {
            Some(n) => Some(n).filter(|n| *n > 0),
            None => default_reads_per_minute(host),
        }
    }

    /// Takes the next write slot on `key`: how long a write made now waits for it (0: it goes now).
    /// Each write takes the slot after the last one's, so writes asked for together go out one by one.
    /// Err: [`WRITE_LINE`] writes already wait, so this one fails now rather than joining them.
    pub fn write_slot(&self, key: &str, host: &str, now: f64) -> std::result::Result<f64, String> {
        let gap = self.write_gap(host);
        let mut all = self.accounts.lock();
        let a = all.entry(key.to_string()).or_insert_with(|| Account { host: host.to_string(), ..Default::default() });
        let line = a.write_line(gap, now);
        if line >= WRITE_LINE {
            return Err(format!(
                "the account's {} write queue is full: {line} writes wait, one every {} ([hosts.{host}] write_gap_secs); try again in {}",
                label(host),
                dur_secs_text(gap),
                dur_secs_text(a.next_write - now - line as f64 * gap)
            ));
        }
        let at = a.next_write.max(now);
        a.next_write = at + gap;
        Ok(at - now)
    }

    /// Why a call on `a` can't go out now (the limit, the slow restart, the budget), without counting it.
    fn refusal(&self, a: &mut Account, host: &str, write: bool, priority: bool, budget: Option<u32>, now: f64) -> Option<String> {
        if let Some(u) = a.until {
            if u > now {
                return Some(match a.host_time {
                    true => format!("{}; the board holds its calls until then", until_text(host, u, true)),
                    false => until_text(host, u, false),
                });
            }
        }
        if let Some(s) = a.slow(now) {
            if !write && !priority {
                return Some(slow_text(&a.slow_why, s));
            }
        }
        let b = budget?;
        let (stop, what) = if write { (b as usize, "writes") } else { (read_stop(b), "reads") };
        if a.calls.len() < stop {
            return None;
        }
        let free_at = a.under_at(stop, now);
        self.spans.lock().push(Span::Open {
            host: host.to_string(),
            kind: "budget",
            from: now,
            to: free_at,
            why: format!("The board made {} of its {b} {} calls this hour; {what} waited", a.calls.len(), label(host)),
        });
        Some(format!(
            "the board made {} of its {b} {} calls this hour ([hosts.{host}] max_per_hour); {what} wait until {}",
            a.calls.len(),
            label(host),
            local_clock(Some(&iso(free_at)))
        ))
    }

    /// A read's turn on `key` under the minute's cap ([`Turn`]). A `priority` read takes the next free
    /// turn in line; a background one only goes now, and only while it leaves the priority share free
    /// and no priority read or write waits on the account.
    pub fn read_turn(&self, key: &str, host: &str, priority: bool, now: f64) -> Turn {
        let budget = self.budget(host);
        let cap = self.reads_per_minute(host);
        let gap = self.write_gap(host);
        let mut all = self.accounts.lock();
        let a = all.entry(key.to_string()).or_insert_with(|| Account { host: host.to_string(), ..Default::default() });
        a.forget_old(now);
        if let Some(e) = self.refusal(a, host, false, priority, budget, now) {
            return Turn::Refused(e);
        }
        let Some(cap) = cap else { return Turn::Now };
        let last = a.reads.back().copied().unwrap_or(now);
        if priority {
            let at = a.read_free_at(cap as usize, now).max(last);
            if at - now > PRIORITY_WAIT_SECS {
                return Turn::Refused(format!(
                    "the account's {} read queue is full: it takes {cap} reads a minute ([hosts.{host}] max_reads_per_minute) and the next free turn is {} away; try again then",
                    label(host),
                    dur_secs_text(at - now)
                ));
            }
            a.reads.push_back(at);
            return match at > now {
                true => Turn::At(at),
                false => Turn::Now,
            };
        }
        if last > now || a.write_line(gap, now) > 0 || a.reads.len() >= background_stop(cap) {
            return Turn::Later;
        }
        a.reads.push_back(now);
        Turn::Now
    }

    /// Whether a call on `key` may go out now; counts it when it does. `priority`: a read that still
    /// goes while the account restarts slowly. Err: why it waits.
    pub fn admit(&self, key: &str, host: &str, write: bool, priority: bool, now: f64) -> std::result::Result<(), String> {
        let budget = self.budget(host);
        let mut all = self.accounts.lock();
        let a = all.entry(key.to_string()).or_insert_with(|| Account { host: host.to_string(), ..Default::default() });
        a.forget_old(now);
        if let Some(e) = self.refusal(a, host, write, priority, budget, now) {
            return Err(e);
        }
        a.calls.push_back(now);
        if write {
            a.writes.push_back(now);
        }
        if let Some(r) = READER.with(|r| r.get()) {
            a.by_reader.push_back((now, r.to_string()));
        }
        drop(all);
        self.touch();
        Ok(())
    }

    /// The host's own count of the account's calls (`X-Request-Count`) on an answer to `key`: kept with
    /// its time to show, never paced on.
    pub fn request_count(&self, key: &str, host: &str, n: u64, now: f64) {
        let mut all = self.accounts.lock();
        let a = all.entry(key.to_string()).or_insert_with(|| Account { host: host.to_string(), ..Default::default() });
        a.request_count = Some((n, now));
        drop(all);
        self.touch();
    }

    /// Once an hour per account that made calls (the first an hour after it was first seen), a log
    /// line with the hour's calls, their split by reader and the host's last `X-Request-Count`.
    fn hourly(&self, now: f64) {
        let mut lines = vec![];
        for a in self.accounts.lock().values_mut() {
            let Some(at) = a.logged_at else {
                a.logged_at = Some(now);
                continue;
            };
            if now - at < HOUR {
                continue;
            }
            a.logged_at = Some(now);
            a.forget_old(now);
            if !a.calls.is_empty() {
                lines.push(format!("rate limit: hourly on {}: {}", label(&a.host), a.hour_text(now)));
            }
        }
        self.log.lock().extend(lines);
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
            a.since = None;
        }
        // A limit already standing doesn't back off again for the calls that were already out; only
        // a later time from the host moves it.
        let until = match (standing, told) {
            (Some(u), t) => u.max(t.unwrap_or(u)),
            (None, Some(t)) => t,
            (None, None) => now + BACKOFF_SECS,
        };
        if standing.is_none() {
            a.share = Some((now, a.calls.len()));
        }
        a.host_time = told == Some(until) || (standing == Some(until) && a.host_time);
        a.until = Some(until);
        a.since.get_or_insert(now);
        a.why = hit.why.clone();
        a.slow_until = Some(until + SLOW_SECS);
        a.slow_why = format!("{}'s limit on the account ended at {}; the board is easing back in", label(host), local_clock(Some(&iso(until))));
        let budget = self.budget(host).map(|b| format!(" (budget {b})")).unwrap_or_default();
        let source = if a.host_time { "the host's time" } else { "the board's backoff" };
        let headers = match hit.headers.is_empty() {
            true => "it sent no Retry-After or X-RateLimit headers".to_string(),
            false => format!("it sent {}", hit.headers.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join(", ")),
        };
        let told = a.request_count_text().map(|t| format!("; {t}")).unwrap_or_default();
        self.log.lock().push(format!(
            "rate limit: {} answered \"{}\" ({headers}); the board backs off until {} ({}, {source}); {}{budget}{told}",
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
    /// restart after a limit, and a limit the host set, since the owner says it's over. Answers a line
    /// per account it lifted.
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
            let mut by_reader: Vec<(f64, String)> = a.by_reader.iter().chain(here.by_reader.iter()).cloned().collect();
            by_reader.sort_by(|x, y| x.0.total_cmp(&y.0));
            a.calls = calls.into();
            a.writes = writes.into();
            a.by_reader = by_reader.into();
            a.request_count = [a.request_count, here.request_count].into_iter().flatten().max_by(|x, y| x.1.total_cmp(&y.1));
            a.logged_at = here.logged_at;
            a.next_write = a.next_write.max(here.next_write);
            a.reads = std::mem::take(&mut here.reads);
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
                    "calls_by_reader": reader_map(a.by_reader(now - HOUR)),
                    "calls_by_reader_last_minute": reader_map(a.by_reader(now - BURST_SECS)),
                    "readers_text": a.readers_text(now - HOUR),
                    "calls_last_minute": a.calls.iter().filter(|t| **t > now - BURST_SECS).count(),
                    "readers_minute_text": a.readers_text(now - BURST_SECS),
                    "request_count": a.request_count.map(|(n, _)| n),
                    "request_count_at": a.request_count.map(|(_, at)| iso(at)),
                    "request_count_text": a.request_count_text(),
                    "max_per_hour": self.budget(&a.host),
                    "reads_last_minute": a.reads.iter().filter(|t| **t <= now).count(),
                    "max_reads_per_minute": self.reads_per_minute(&a.host),
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

fn reader_map(split: Vec<(String, usize)>) -> Value {
    Value::Object(split.into_iter().map(|(r, c)| (r, json!(c))).collect())
}

/// "2,189".
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "11:13:35am", local time.
fn clock_secs(t: f64) -> String {
    local_dt(t).format("%-I:%M:%S%P").to_string()
}

/// Background reads stop here, keeping the rest of the minute's `cap` for priority reads.
fn background_stop(cap: u32) -> usize {
    (cap as f64 * (1.0 - PRIORITY_SHARE)).floor().max(1.0) as usize
}

/// What a background read that didn't get a turn in [`BACKGROUND_WAIT_SECS`] says.
fn skip_text(host: &str, cap: Option<u32>) -> String {
    format!(
        "skipped this {} read: the account is at its {} reads a minute ([hosts.{host}] max_reads_per_minute), or `tb pr` commands and writes went first; the next poll asks again",
        label(host),
        cap.unwrap_or_default()
    )
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

/// A read shared on an account: in flight, or answered lately.
#[derive(Debug)]
enum Shared {
    Flying(Arc<Flight>),
    /// Landed at `at`; went out at `sent`.
    Kept { at: f64, sent: f64, v: Value },
}

/// A read one caller makes for everyone asking the same.
#[derive(Debug, Default)]
pub struct Flight {
    /// When it went out (its leader joined).
    sent: f64,
    landed: Mutex<Option<Landed>>,
    done: parking_lot::Condvar,
}

impl Flight {
    fn wait(&self) -> Landed {
        let mut landed = self.landed.lock();
        loop {
            match &*landed {
                Some(l) => return l.clone(),
                None => self.done.wait(&mut landed),
            }
        }
    }
}

/// How a shared read ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Landed {
    /// The host's answer, for everyone waiting.
    Answer(std::result::Result<Value, String>),
    /// It never reached the host (skipped, or the account limited): each one waiting tries itself.
    Retry,
}

/// A read's place among the account's shared reads ([`Limits::join`]).
#[derive(Debug)]
pub enum Join {
    /// The answer kept from the same read lately.
    Kept(Value),
    /// The same read is in flight: wait for it.
    Wait(Arc<Flight>),
    /// None is: this caller makes it, and lands it for the rest ([`Limits::land`]).
    Lead(Arc<Flight>),
}

#[cfg(test)]
impl Join {
    fn kept(self) -> Option<Value> {
        match self {
            Join::Kept(v) => Some(v),
            _ => None,
        }
    }
}

/// The PR a host URL is about: its path up to `pullrequests/<n>`, lowercased; None for a read of no
/// one PR.
fn pr_scope(url: &str) -> Option<String> {
    const MARK: &str = "/pullrequests/";
    let path = url.split(['?', '#']).next()?.to_ascii_lowercase();
    let at = path.find(MARK)? + MARK.len();
    let n = path[at..].split('/').next()?;
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| path[..at + n.len()].to_string())
}

impl Limits {
    /// A read of `url` on account `key`: the answer kept from the last [`KEEP_SECS`], else the same
    /// read in flight, else this caller leads it.
    pub fn join(&self, key: &str, url: &str, now: f64) -> Join {
        self.join_after(key, url, now, f64::NEG_INFINITY)
    }

    /// [`join`](Limits::join), taking only a read that went out at `after` or later ([`fresh`]): one
    /// from before is led again, and its leader's answer then isn't kept.
    pub fn join_after(&self, key: &str, url: &str, now: f64, after: f64) -> Join {
        let mut shared = self.shared.lock();
        shared.retain(|_, s| !matches!(s, Shared::Kept { at, .. } if now - *at >= KEEP_SECS));
        let k = (key.to_string(), url.to_string());
        match shared.get(&k) {
            Some(Shared::Kept { sent, v, .. }) if *sent >= after => Join::Kept(v.clone()),
            Some(Shared::Flying(f)) if f.sent >= after => Join::Wait(f.clone()),
            _ => {
                let f = Arc::new(Flight { sent: now, ..Default::default() });
                shared.insert(k, Shared::Flying(f.clone()));
                Join::Lead(f)
            }
        }
    }

    /// The read `flight` led landed: wakes those waiting on it, and keeps its answer unless it's an
    /// error or empty (Null), or a write or a [`fresh`] read took its place meanwhile.
    pub fn land(&self, key: &str, url: &str, flight: &Arc<Flight>, landed: Landed, now: f64) {
        {
            let mut shared = self.shared.lock();
            let k = (key.to_string(), url.to_string());
            if matches!(shared.get(&k), Some(Shared::Flying(f)) if Arc::ptr_eq(f, flight)) {
                match &landed {
                    Landed::Answer(Ok(v)) if !v.is_null() => shared.insert(k, Shared::Kept { at: now, sent: flight.sent, v: v.clone() }),
                    _ => shared.remove(&k),
                };
            }
        }
        *flight.landed.lock() = Some(landed);
        flight.done.notify_all();
    }

    /// A write to `url` on account `key` is done: drops the account's shared reads it may change. A
    /// write to a PR drops that PR's reads and those of no one PR; any other write drops them all.
    pub fn wrote(&self, key: &str, url: &str) {
        let scope = pr_scope(url);
        self.shared.lock().retain(|(k, u), _| k != key || scope.as_ref().is_some_and(|s| pr_scope(u).is_some_and(|r| &r != s)));
    }
}

/// A last-wins write waiting for its slot ([`Limits::queue_last`]).
#[derive(Debug)]
struct Queued {
    /// When its slot comes, epoch seconds.
    at: f64,
    /// Whose write goes: each newer one that replaces it takes the next turn.
    turn: u64,
    /// What the one that goes lands for every caller it stands for: an `Outcome<T>`.
    out: Arc<dyn Any + Send + Sync>,
}

/// How a last-wins write ended, for every caller it replaced.
#[derive(Debug)]
pub struct Outcome<T> {
    res: Mutex<Option<std::result::Result<T, String>>>,
    done: parking_lot::Condvar,
}

impl<T: Clone> Outcome<T> {
    fn wait(&self) -> std::result::Result<T, String> {
        let mut res = self.res.lock();
        loop {
            match &*res {
                Some(r) => return r.clone(),
                None => self.done.wait(&mut res),
            }
        }
    }
}

impl<T> Outcome<T> {
    fn land(&self, res: std::result::Result<T, String>) {
        *self.res.lock() = Some(res);
        self.done.notify_all();
    }
}

impl Limits {
    /// Queues a last-wins write marked `last` on account `key` ([`last_wins`]): when one with the same
    /// key still waits for its slot, this one replaces it, taking its slot and its outcome, so it takes
    /// no slot of its own and never finds the line full; else it takes the next slot
    /// ([`write_slot`](Limits::write_slot)). Gives when the slot comes (epoch seconds), this caller's
    /// turn ([`go_last`](Limits::go_last)) and the outcome it shares.
    pub fn queue_last<T: Send + Sync + 'static>(&self, key: &str, host: &str, last: &str, now: f64) -> std::result::Result<(f64, u64, Arc<Outcome<T>>), String> {
        let mut queued = self.queued.lock();
        let k = (key.to_string(), last.to_string(), TypeId::of::<T>());
        // Keyed by the answer's type too, so the outcome is always one this caller can read.
        if let Some(q) = queued.get_mut(&k) {
            if let Ok(out) = q.out.clone().downcast::<Outcome<T>>() {
                q.turn += 1;
                return Ok((q.at, q.turn, out));
            }
        }
        let at = now + self.write_slot(key, host, now)?;
        let out = Arc::new(Outcome { res: Mutex::new(None), done: parking_lot::Condvar::new() });
        queued.insert(k, Queued { at, turn: 0, out: out.clone() });
        Ok((at, 0, out))
    }

    /// Whether the last-wins write queued with `turn` and `out` goes now its slot has come: false when a
    /// newer one replaced it. The one that goes leaves the queue, so a write after it takes a slot of
    /// its own.
    pub fn go_last<T: Send + Sync + 'static>(&self, key: &str, last: &str, turn: u64, out: &Arc<Outcome<T>>) -> bool {
        let mut queued = self.queued.lock();
        let k = (key.to_string(), last.to_string(), TypeId::of::<T>());
        let ours = queued.get(&k).is_some_and(|q| q.turn == turn && Arc::as_ptr(&q.out) as *const () == Arc::as_ptr(out) as *const ());
        if ours {
            queued.remove(&k);
        }
        ours
    }
}

/// Lands a last-wins write's outcome for the callers it replaced once it's done, or an error should it
/// unwind first, so they never wait forever.
struct LastWins<'a, T: Send + Sync + 'static> {
    l: &'a Limits,
    key: &'a str,
    last: &'a str,
    turn: u64,
    out: Arc<Outcome<T>>,
    /// A newer write replaced this one: that one lands the outcome.
    replaced: bool,
    /// This one went out.
    went: bool,
    res: Option<std::result::Result<T, String>>,
}

impl<T: Send + Sync + 'static> Drop for LastWins<'_, T> {
    fn drop(&mut self) {
        if self.replaced || !(self.went || self.l.go_last(self.key, self.last, self.turn, &self.out)) {
            return;
        }
        self.out.land(self.res.take().unwrap_or_else(|| Err("the write that replaced this one broke off before it was done".into())));
    }
}

/// Lands a led read when the leader is done with it, or `Retry` should it unwind before it lands, so
/// those waiting never wait forever.
struct Leading<'a> {
    l: &'a Limits,
    key: &'a str,
    url: &'a str,
    flight: Arc<Flight>,
    landed: Option<Landed>,
}

impl Drop for Leading<'_> {
    fn drop(&mut self) {
        self.l.land(self.key, self.url, &self.flight, self.landed.take().unwrap_or(Landed::Retry), now_ts());
    }
}

/// A read's turn under the minute's cap ([`Limits::read_turn`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Turn {
    /// It goes now.
    Now,
    /// A priority read's turn, ahead of now: it waits for it.
    At(f64),
    /// A background read that can't go yet: it asks again shortly, or is skipped.
    Later,
    /// It can't go: why.
    Refused(String),
}

/// How often a background read waiting for a turn asks again.
const RETRY_SECS: f64 = 0.5;

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
    /// read); notes a limit the call's answer reports (`limited` reads it from the error). A write
    /// marked [`last_wins`] may be replaced by a newer one while it waits (see "Collapsing writes").
    pub fn call<T: Clone + Send + Sync + 'static>(&self, write: bool, f: impl FnOnce() -> std::result::Result<(T, Option<Hit>), (String, Option<Hit>)>) -> std::result::Result<T, String> {
        self.call_on(limits(), BACKGROUND_WAIT_SECS, write, f)
    }

    /// [`Gate::call`] on `l`, a background read asking for its turn for `patience` seconds.
    fn call_on<T: Clone + Send + Sync + 'static>(&self, l: &Limits, patience: f64, write: bool, f: impl FnOnce() -> std::result::Result<(T, Option<Hit>), (String, Option<Hit>)>) -> std::result::Result<T, String> {
        let priority = is_priority();
        if let (true, Some(last)) = (write, last_wins_key()) {
            return self.write_last(l, &last, priority, f);
        }
        let wait = match write {
            true => l.write_slot(&self.key, &self.host, now_ts())?,
            false => self.read_wait(l, priority, patience)?,
        };
        self.wait_turn(write, wait);
        self.go(l, write, priority, f)
    }

    /// A write marked [`last_wins`] under `last`: waits for its slot, unless a newer one with the same
    /// key replaced it meanwhile; then it gives that one's answer instead of calling the host.
    fn write_last<T: Clone + Send + Sync + 'static>(&self, l: &Limits, last: &str, priority: bool, f: impl FnOnce() -> std::result::Result<(T, Option<Hit>), (String, Option<Hit>)>) -> std::result::Result<T, String> {
        let (at, turn, out) = l.queue_last::<T>(&self.key, &self.host, last, now_ts())?;
        let mut mine = LastWins { l, key: &self.key, last, turn, out, replaced: false, went: false, res: None };
        self.wait_turn(true, at - now_ts());
        if !l.go_last(&self.key, last, turn, &mine.out) {
            mine.replaced = true;
            let what = format!("the newer {} write to the same PR that replaced this one", label(&self.host));
            return crate::progress::waiting(&what, crate::progress::WAITING_EVERY, || mine.out.wait());
        }
        mine.went = true;
        let res = self.go(l, true, priority, f);
        mine.res = Some(res.clone());
        res
    }

    /// Waits `wait` seconds for a call's turn.
    fn wait_turn(&self, write: bool, wait: f64) {
        if wait > 0.0 {
            // Said as progress, so a `tb pr` command queued behind other calls doesn't time out.
            let what = match write {
                true => format!("a {} write slot ({} from now; writes go out one at a time)", label(&self.host), dur_secs_text(wait)),
                false => format!("a {} read turn ({} from now; the account takes so many reads a minute)", label(&self.host), dur_secs_text(wait)),
            };
            crate::progress::waiting(&what, crate::progress::WAITING_EVERY, || std::thread::sleep(std::time::Duration::from_secs_f64(wait)));
        }
    }

    /// Makes the call once its turn has come, unless the account is limited or out of budget.
    fn go<T>(&self, l: &Limits, write: bool, priority: bool, f: impl FnOnce() -> std::result::Result<(T, Option<Hit>), (String, Option<Hit>)>) -> std::result::Result<T, String> {
        l.admit(&self.key, &self.host, write, priority, now_ts())?;
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

    /// One HTTP call to `url` through [`call`](Gate::call): a GET is a read, shared on the account
    /// (see "Sharing reads"); any other method is a write, which then drops the reads it may change.
    pub fn request(&self, method: &str, url: &str, f: impl FnOnce() -> std::result::Result<(Value, Option<Hit>), (String, Option<Hit>)>) -> std::result::Result<Value, String> {
        self.request_on(limits(), method, url, f)
    }

    fn request_on(&self, l: &Limits, method: &str, url: &str, f: impl FnOnce() -> std::result::Result<(Value, Option<Hit>), (String, Option<Hit>)>) -> std::result::Result<Value, String> {
        match method {
            "GET" => self.read_on(l, url, f),
            _ => self.write_on(l, url, f),
        }
    }

    /// A read of `url`: the same read in flight is waited for, and a kept answer is used, instead of
    /// calling the host.
    fn read_on(&self, l: &Limits, url: &str, f: impl FnOnce() -> std::result::Result<(Value, Option<Hit>), (String, Option<Hit>)>) -> std::result::Result<Value, String> {
        let after = fresh_since();
        loop {
            let flight = match l.join_after(&self.key, url, now_ts(), after) {
                Join::Kept(v) => return Ok(v),
                Join::Wait(flight) => match flight.wait() {
                    Landed::Answer(a) => return a,
                    Landed::Retry => continue,
                },
                Join::Lead(flight) => flight,
            };
            let mut lead = Leading { l, key: &self.key, url, flight, landed: None };
            let mut reached = false;
            let res = self.call_on(l, BACKGROUND_WAIT_SECS, false, || {
                reached = true;
                f()
            });
            lead.landed = Some(match reached {
                true => Landed::Answer(res.clone()),
                false => Landed::Retry,
            });
            return res;
        }
    }

    /// A write to `url`; then drops the shared reads it may change ([`Limits::wrote`]), even when it
    /// failed: it may have gone through all the same.
    fn write_on<T: Clone + Send + Sync + 'static>(&self, l: &Limits, url: &str, f: impl FnOnce() -> std::result::Result<(T, Option<Hit>), (String, Option<Hit>)>) -> std::result::Result<T, String> {
        let res = self.call_on(l, BACKGROUND_WAIT_SECS, true, f);
        l.wrote(&self.key, url);
        res
    }

    /// How long a read waits for its turn ([`Limits::read_turn`]); a background read asks again until
    /// `patience` seconds ([`BACKGROUND_WAIT_SECS`]) have gone by, then is skipped.
    fn read_wait(&self, l: &Limits, priority: bool, patience: f64) -> std::result::Result<f64, String> {
        let give_up = now_ts() + patience;
        loop {
            let now = now_ts();
            match l.read_turn(&self.key, &self.host, priority, now) {
                Turn::Now => return Ok(0.0),
                Turn::At(at) => return Ok(at - now),
                Turn::Refused(e) => return Err(e),
                Turn::Later if now < give_up => std::thread::sleep(std::time::Duration::from_secs_f64(RETRY_SECS.min(give_up - now))),
                Turn::Later => return Err(skip_text(&self.host, l.reads_per_minute(&self.host))),
            }
        }
    }

    /// The host's own count of the account's calls (`X-Request-Count`) on any answer.
    pub fn request_count(&self, n: u64) {
        limits().request_count(&self.key, &self.host, n, now_ts());
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

/// `/state`'s `rate_limits`, `GET /prs/limits`' (`tb pr limits`) and `tb feed`'s.
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
    l.hourly(now_ts());
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
    fn without_a_time_it_backs_off_ten_minutes_every_time() {
        let l = Limits::default();
        let mut t = T0;
        let mut waits = vec![];
        for _ in 0..6 {
            let u = l.hit("gh:1", "github", &hit("secondary rate limit"), t);
            waits.push(((u - t) / 60.0).round() as i64);
            t = u + 1.0;
        }
        assert_eq!(waits, vec![10; 6]);
    }

    #[test]
    fn a_limit_that_keeps_coming_back_between_calls_still_waits_ten_minutes() {
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
        assert_eq!(waits, vec![10, 10, 10]);
    }

    #[test]
    fn a_limit_that_comes_back_keeps_the_host_s_time() {
        let l = Limits::default();
        let u = l.hit("bb:1", "bitbucket", &Hit { retry_after: Some(60.0), ..hit("429") }, T0);
        assert_eq!(u, T0 + 60.0, "the host's time, the first time");
        let t = u + 120.0;
        assert_eq!(l.hit("bb:1", "bitbucket", &Hit { retry_after: Some(60.0), ..hit("429") }, t), t + 60.0, "and again when it comes back");
    }

    #[test]
    fn after_a_backoff_only_the_pr_poll_s_reads_and_writes_go_for_ten_minutes() {
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
        assert!(l.admit("bb:1", "bitbucket", false, false, until + 599.0).is_err(), "still easing back in");
        let done = until + 601.0;
        assert!(l.admit("bb:1", "bitbucket", false, false, done).is_ok(), "every read goes 10 minutes after the limit");
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
    fn last_wins_puts_back_the_key_outside_even_when_its_writes_panic() {
        let inner = last_wins("PR1 base".into(), || last_wins("PR1 description".into(), last_wins_key));
        assert_eq!(inner.as_deref(), Some("PR1 description"));
        let panicked = std::panic::catch_unwind(|| last_wins("PR1 base".into(), || panic!("the write broke")));
        assert!(panicked.is_err());
        assert_eq!(last_wins_key(), None, "the thread's later writes aren't marked");
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
    fn calls_already_out_don_t_move_a_standing_limit() {
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
    fn a_backoff_splits_the_hour_s_and_the_minute_s_calls_by_reader() {
        let l = Limits::default();
        reader("poll", || l.admit("bb:1", "bitbucket", false, false, T0 - 600.0)).unwrap();
        reader("poll", || l.admit("bb:1", "bitbucket", false, false, T0)).unwrap();
        reader("poll", || reader("full-read fallback", || l.admit("bb:1", "bitbucket", false, false, T0 + 1.0))).unwrap();
        reader("agent", || l.admit("bb:1", "bitbucket", true, false, T0 + 2.0)).unwrap();
        l.admit("bb:1", "bitbucket", false, false, T0 + 3.0).unwrap();
        l.hit("bb:1", "bitbucket", &hit("Bitbucket answered 429"), T0 + 5.0);
        let log = l.take_log();
        assert!(
            log[0].contains("it made 5 calls on this account in the last hour (poll 2, full-read fallback 1, agent 1, other 1), 4 in the last minute (1 write; poll 1, full-read fallback 1, agent 1, other 1)"),
            "{}",
            log[0]
        );
        let st = &l.status(T0 + 5.0)[0];
        assert_eq!(st["calls_by_reader"], json!({"poll": 2, "full-read fallback": 1, "agent": 1, "other": 1}));
        assert_eq!(st["calls_by_reader_last_minute"]["poll"], 1);
        assert_eq!(st["readers_text"], "poll 2, full-read fallback 1, agent 1, other 1");
        assert_eq!((st["calls_last_minute"].as_u64(), st["readers_minute_text"].as_str()), (Some(4), Some("poll 1, full-read fallback 1, agent 1, other 1")));
        assert_eq!(READER.with(|r| r.get()), None, "the reader ends with its calls");
    }

    #[test]
    fn calls_outside_a_reader_show_no_split() {
        let l = Limits::default();
        l.admit("bb:1", "bitbucket", false, false, T0).unwrap();
        l.hit("bb:1", "bitbucket", &hit("429"), T0 + 1.0);
        let log = l.take_log();
        assert!(log[0].contains("it made 1 calls on this account in the last hour, 1 in the last minute (0 writes) (budget 900)"), "{}", log[0]);
        assert_eq!(l.status(T0 + 1.0)[0]["readers_text"], Value::Null);
    }

    #[test]
    fn the_host_s_request_count_shows_in_the_backoff_and_the_status() {
        let l = Limits::default();
        l.request_count("bb:1", "bitbucket", 2100, T0 - 30.0);
        l.request_count("bb:1", "bitbucket", 2189, T0);
        let until = l.hit("bb:1", "bitbucket", &hit("429"), T0 + 1.0);
        let said = format!("Bitbucket's x-request-count said 2,189 at {}", clock_secs(T0));
        let log = l.take_log();
        assert!(log[0].ends_with(&format!("; {said}")), "{}", log[0]);
        let st = &l.status(T0 + 1.0)[0];
        assert_eq!((st["request_count"].as_u64(), st["request_count_at"].as_str()), (Some(2189), Some(iso(T0).as_str())));
        assert_eq!(st["request_count_text"], said.as_str());
        assert_eq!(st["calls_last_hour"], 0, "the header isn't one of the board's calls");
        assert_eq!(l.held_until("bb:1", T0 + 1.0), Some(until), "the header doesn't change the hold");
    }

    #[test]
    fn the_hourly_line_gives_the_hour_s_calls_once_an_hour() {
        let l = Limits::default();
        reader("poll", || l.admit("bb:1", "bitbucket", false, false, T0)).unwrap();
        l.hourly(T0);
        assert!(l.take_log().is_empty(), "the first hour starts when the account is first seen");
        reader("agent", || l.admit("bb:1", "bitbucket", false, false, T0 + 1800.0)).unwrap();
        l.request_count("bb:1", "bitbucket", 2189, T0 + 1800.0);
        l.hourly(T0 + HOUR - 1.0);
        assert!(l.take_log().is_empty());
        l.hourly(T0 + HOUR);
        assert_eq!(
            l.take_log(),
            vec![format!("rate limit: hourly on Bitbucket: the board made 1 call on this account in the last hour (agent 1); Bitbucket's x-request-count said 2,189 at {}", clock_secs(T0 + 1800.0))]
        );
        l.hourly(T0 + HOUR + 60.0);
        assert!(l.take_log().is_empty());
        l.hourly(T0 + 3.0 * HOUR);
        assert!(l.take_log().is_empty(), "an hour with no calls logs nothing");
    }

    #[test]
    fn counts_are_grouped_by_thousands() {
        assert_eq!([0, 999, 1000, 2189, 1_234_567].map(grouped), ["0", "999", "1,000", "2,189", "1,234,567"]);
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
        assert_eq!(l.write_slot("bb:1", "bitbucket", T0), Ok(0.0), "the first goes now");
        assert_eq!(l.write_slot("bb:1", "bitbucket", T0 + 0.5), Ok(1.5), "the next waits out the 2s gap");
        assert_eq!(l.write_slot("bb:1", "bitbucket", T0 + 0.5), Ok(3.5), "and the one after, its own slot");
        assert_eq!(l.write_slot("bb:2", "bitbucket", T0 + 0.5), Ok(0.0), "another account has its own slots");
        assert_eq!(l.write_slot("bb:1", "bitbucket", T0 + 60.0), Ok(0.0), "a quiet account writes at once");
        assert_eq!(l.write_slot("gh:1", "github", T0), Ok(0.0));
        assert_eq!(l.write_slot("gh:1", "github", T0), Ok(1.0), "GitHub's gap is a second");
        l.set_hosts([("bitbucket".to_string(), HostConfig { write_gap_secs: Some(5.0), ..Default::default() })].into());
        assert_eq!(l.write_slot("bb:3", "bitbucket", T0), Ok(0.0));
        assert_eq!(l.write_slot("bb:3", "bitbucket", T0), Ok(5.0), "write_gap_secs sets it");
    }

    fn reads_a_minute(host: &str, n: u32) -> BTreeMap<String, HostConfig> {
        [(host.to_string(), HostConfig { max_reads_per_minute: Some(n), ..Default::default() })].into()
    }

    #[test]
    fn a_full_write_line_fails_at_once_naming_the_queue() {
        let l = Limits::default();
        for i in 0..=WRITE_LINE {
            assert_eq!(l.write_slot("bb:1", "bitbucket", T0), Ok(2.0 * i as f64), "write {i} takes its slot");
        }
        let e = l.write_slot("bb:1", "bitbucket", T0).unwrap_err();
        assert!(e.contains("Bitbucket write queue is full") && e.contains("10 writes wait"), "{e}");
        assert!(e.contains("try again in 2s"), "{e}");
        assert_eq!(l.write_slot("bb:2", "bitbucket", T0), Ok(0.0), "another account's line is its own");
        for now in [T0 + 0.5, T0 + 1.9] {
            let e = l.write_slot("bb:1", "bitbucket", now).unwrap_err();
            assert!(e.contains("write queue is full") && e.contains("10 writes wait"), "between slots at {now}: {e}");
        }
        assert_eq!(l.write_slot("bb:1", "bitbucket", T0 + 2.0), Ok(20.0), "a slot that went out lets one more in");
        assert_eq!(l.write_slot("bb:1", "bitbucket", T0 + 60.0), Ok(0.0), "the line empties");
    }

    #[test]
    fn reads_a_minute_default_to_30_on_bitbucket_and_0_turns_them_off() {
        let l = Limits::default();
        assert_eq!(l.reads_per_minute("bitbucket"), Some(30));
        assert_eq!(l.reads_per_minute("github"), None);
        l.set_hosts(reads_a_minute("bitbucket", 0));
        assert_eq!(l.reads_per_minute("bitbucket"), None);
        for i in 0..100 {
            assert_eq!(l.read_turn("bb:1", "bitbucket", false, T0 + i as f64 * 0.01), Turn::Now, "no cap: read {i} goes");
        }
        l.set_hosts(reads_a_minute("bitbucket", 10));
        assert_eq!(l.reads_per_minute("bitbucket"), Some(10));
    }

    #[test]
    fn no_minute_holds_more_reads_than_the_cap() {
        let l = Limits::default();
        let mut went = vec![];
        let mut t = T0;
        while t < T0 + 300.0 {
            let priority = (t - T0) as i64 % 3 == 0;
            match l.read_turn("bb:1", "bitbucket", priority, t) {
                Turn::Now => went.push(t),
                Turn::At(at) => went.push(at),
                Turn::Later | Turn::Refused(_) => {}
            }
            t += 0.25;
        }
        went.sort_by(f64::total_cmp);
        assert!(went.len() > 120, "reads kept going: {}", went.len());
        for (i, at) in went.iter().enumerate() {
            let in_minute = went[i..].iter().take_while(|t| **t < at + 60.0).count();
            assert!(in_minute <= 30, "{in_minute} reads in the minute from {at}");
        }
    }

    #[test]
    fn background_reads_leave_a_fifth_of_the_minute_to_priority_reads() {
        let l = Limits::default();
        for i in 0..24 {
            assert_eq!(l.read_turn("bb:1", "bitbucket", false, T0), Turn::Now, "background read {i}");
        }
        assert_eq!(l.read_turn("bb:1", "bitbucket", false, T0), Turn::Later, "the rest is the priority share");
        for i in 0..6 {
            assert_eq!(l.read_turn("bb:1", "bitbucket", true, T0), Turn::Now, "priority read {i} still goes");
        }
        assert_eq!(l.read_turn("bb:1", "bitbucket", true, T0 + 1.0), Turn::At(T0 + 60.0), "past the cap, the next turn in the minute");
        assert_eq!(l.read_turn("bb:1", "bitbucket", true, T0 + 1.0), Turn::At(T0 + 60.0));
        assert_eq!(l.read_turn("bb:1", "bitbucket", false, T0 + 59.0), Turn::Later, "a background read waits behind them");
        assert_eq!(l.read_turn("bb:1", "bitbucket", false, T0 + 121.0), Turn::Now, "a quiet minute later it goes");
    }

    #[test]
    fn a_priority_read_more_than_a_minute_off_fails_at_once() {
        let l = Limits::default();
        l.set_hosts(reads_a_minute("bitbucket", 2));
        assert_eq!(l.read_turn("bb:1", "bitbucket", true, T0), Turn::Now);
        assert_eq!(l.read_turn("bb:1", "bitbucket", true, T0), Turn::Now);
        assert_eq!(l.read_turn("bb:1", "bitbucket", true, T0), Turn::At(T0 + 60.0));
        assert_eq!(l.read_turn("bb:1", "bitbucket", true, T0), Turn::At(T0 + 60.0));
        let Turn::Refused(e) = l.read_turn("bb:1", "bitbucket", true, T0) else { panic!("a turn two minutes off") };
        assert!(e.contains("Bitbucket read queue is full") && e.contains("max_reads_per_minute"), "{e}");
        assert_eq!(l.status(T0)[0]["reads_last_minute"], 2, "turns ahead of now aren't counted yet");
        assert_eq!(l.status(T0)[0]["max_reads_per_minute"], 2);
    }

    #[test]
    fn background_reads_yield_to_a_waiting_write() {
        let l = Limits::default();
        l.write_slot("bb:1", "bitbucket", T0).unwrap();
        assert_eq!(l.read_turn("bb:1", "bitbucket", false, T0), Turn::Now, "a write that went out holds nothing");
        assert_eq!(l.write_slot("bb:1", "bitbucket", T0), Ok(2.0));
        assert_eq!(l.read_turn("bb:1", "bitbucket", false, T0), Turn::Later, "a write waits for its slot");
        assert_eq!(l.read_turn("bb:1", "bitbucket", true, T0), Turn::Now, "a priority read doesn't");
        assert_eq!(l.read_turn("bb:1", "bitbucket", false, T0 + 2.0), Turn::Now, "once it went, reads go again");
    }

    #[test]
    fn a_limited_account_takes_no_read_turns() {
        let l = Limits::default();
        l.hit("bb:1", "bitbucket", &hit("429"), T0);
        assert!(matches!(l.read_turn("bb:1", "bitbucket", true, T0), Turn::Refused(_)));
        assert_eq!(l.status(T0)[0]["reads_last_minute"], 0, "a refused read holds no turn");
    }

    #[test]
    fn a_background_read_with_no_turn_is_skipped_and_reaches_no_host() {
        let l = Limits::default();
        l.set_hosts(reads_a_minute("bitbucket", 1));
        let gate = Gate::new("bitbucket", "bb:skip".into());
        let mut reached = 0;
        gate.call_on(&l, 0.6, false, || {
            reached += 1;
            Ok(((), None))
        })
        .unwrap();
        let start = now_ts();
        let e = gate
            .call_on(&l, 0.6, false, || {
                reached += 1;
                Ok(((), None))
            })
            .unwrap_err();
        let waited = now_ts() - start;
        assert!(e.starts_with("skipped this Bitbucket read") && e.contains("the next poll asks again"), "{e}");
        assert!(waited >= 0.5, "it asked for its turn for {waited}s before giving up");
        assert_eq!(reached, 1, "the skipped read never went out");
    }

    #[test]
    fn a_background_read_asks_again_and_goes_once_a_write_has_gone() {
        let l = Limits::default();
        l.set_hosts([("bitbucket".to_string(), HostConfig { write_gap_secs: Some(0.6), ..Default::default() })].into());
        let gate = Gate::new("bitbucket", "bb:later".into());
        let now = now_ts();
        l.write_slot("bb:later", "bitbucket", now).unwrap();
        assert!(l.write_slot("bb:later", "bitbucket", now).unwrap() > 0.0, "a write waits for its slot");
        let mut reached = 0;
        gate.call_on(&l, 5.0, false, || {
            reached += 1;
            Ok(((), None))
        })
        .unwrap();
        assert!(now_ts() - now >= 0.5, "the read kept asking until the write went");
        assert_eq!(reached, 1);
    }

    #[test]
    fn a_priority_read_sleeps_until_its_turn_and_a_full_write_line_never_runs_the_write() {
        let l = Limits::default();
        l.set_hosts(reads_a_minute("bitbucket", 1));
        let gate = Gate::new("bitbucket", "bb:at".into());
        let start = now_ts();
        assert_eq!(l.read_turn("bb:at", "bitbucket", false, start - READ_WINDOW_SECS + 0.5), Turn::Now);
        let mut reached = 0;
        priority(|| gate.call_on(&l, 0.0, false, || {
            reached += 1;
            Ok(((), None))
        }))
        .unwrap();
        assert!(now_ts() - start >= 0.4, "the priority read waited for the turn that aged out");
        assert_eq!(reached, 1);
        for _ in 0..=WRITE_LINE {
            l.write_slot("bb:at", "bitbucket", now_ts()).unwrap();
        }
        let e = gate
            .call_on(&l, 0.0, true, || {
                reached += 1;
                Ok(((), None))
            })
            .unwrap_err();
        assert!(e.contains("write queue is full"), "{e}");
        assert_eq!(reached, 1, "the write never ran");
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
        let until = l.hit("bb:1", "bitbucket", &Hit { retry_after: Some(1800.0), ..hit("429") }, T0);
        let spans = l.take_spans();
        assert_eq!(spans[0], Span::Cut { host: "bitbucket".into(), kind: Some("slow"), at: T0 }, "a limit ends the slow stretch it came in");
        assert!(matches!(&spans[1], Span::Open { kind: "limited", from, to, .. } if *from == T0 && *to == until), "{spans:?}");
        assert!(matches!(&spans[2], Span::Open { kind: "slow", from, to, .. } if *from == until && *to == until + 600.0), "10 minutes after a long limit too: {spans:?}");
    }

    #[test]
    fn lift_ends_the_board_s_hold() {
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
        assert_eq!(l.hit("bb:1", "bitbucket", &hit("429"), t + 10.0) - (t + 10.0), 600.0, "the next limit backs off 10 minutes");
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
        assert_eq!(next - (until + 1.0), BACKOFF_SECS, "a limit after the restart backs off 10 minutes");
        let later = Limits::default();
        later.restore(&saved, T0 + 2.0 * HOUR + 120.0);
        assert!(later.admit("bb:1", "bitbucket", false, false, T0 + 2.0 * HOUR + 120.0).is_ok(), "an old hold doesn't come back");
    }

    #[test]
    fn the_split_and_the_request_count_survive_a_restart() {
        let before = Limits::default();
        reader("poll", || before.admit("bb:1", "bitbucket", false, false, T0)).unwrap();
        before.request_count("bb:1", "bitbucket", 2189, T0);
        let saved = before.saved();
        let after = Limits::default();
        reader("agent", || after.admit("bb:1", "bitbucket", false, false, T0 + 10.0)).unwrap();
        after.restore(&saved, T0 + 20.0);
        let st = &after.status(T0 + 20.0)[0];
        assert_eq!(st["readers_text"], "poll 1, agent 1");
        assert_eq!(st["request_count"], 2189);
        after.request_count("bb:1", "bitbucket", 2200, T0 + 30.0);
        after.restore(&saved, T0 + 40.0);
        assert_eq!(after.status(T0 + 40.0)[0]["request_count"], 2200, "the later count wins");
    }

    #[test]
    fn a_restore_keeps_the_reads_taken_in_this_process() {
        let before = Limits::default();
        before.admit("bb:1", "bitbucket", false, false, T0 - 10.0).unwrap();
        let saved = before.saved();
        let after = Limits::default();
        assert_eq!(after.read_turn("bb:1", "bitbucket", false, T0), Turn::Now);
        after.restore(&saved, T0 + 1.0);
        assert_eq!(after.status(T0 + 1.0)[0]["reads_last_minute"], 1, "the turn taken before the restore still counts");
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

    const PR1: &str = "https://api.bitbucket.org/2.0/repositories/ws/repo/pullrequests/1";
    const PR2: &str = "https://api.bitbucket.org/2.0/repositories/ws/repo/pullrequests/2";

    /// Who holds the read of `url` in flight: the shared map, its leader and each one waiting.
    fn holding(l: &Limits, key: &str, url: &str) -> usize {
        match l.shared.lock().get(&(key.to_string(), url.to_string())) {
            Some(Shared::Flying(f)) => Arc::strong_count(f),
            _ => 0,
        }
    }

    fn wait_until_held(l: &Limits, key: &str, url: &str, n: usize) {
        let give_up = now_ts() + 5.0;
        while holding(l, key, url) < n {
            assert!(now_ts() < give_up, "{n} never held the read");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn calls(l: &Limits, key: &str) -> usize {
        l.accounts.lock().get(key).map_or(0, |a| a.calls.len())
    }

    #[test]
    fn pr_scope_is_the_url_up_to_the_pr_number() {
        assert_eq!(pr_scope(&format!("{PR1}/comments?pagelen=100")).as_deref(), Some("https://api.bitbucket.org/2.0/repositories/ws/repo/pullrequests/1"));
        assert_eq!(pr_scope(PR1), pr_scope(&format!("{PR1}/merge")));
        assert_ne!(pr_scope(PR1), pr_scope(&format!("{PR1}2")), "PR 12 isn't PR 1");
        assert_eq!(pr_scope("https://dev.azure.com/o/p/_apis/git/repositories/r/pullRequests/7?x=1"), pr_scope("https://dev.azure.com/o/p/_apis/git/repositories/r/pullrequests/7"));
        assert_eq!(pr_scope("https://api.bitbucket.org/2.0/repositories/ws/repo/pullrequests?state=OPEN"), None);
        assert_eq!(pr_scope("https://api.bitbucket.org/2.0/repositories/ws/repo/pullrequests/"), None);
        assert_eq!(pr_scope("https://api.bitbucket.org/2.0/repositories/ws/repo/commit/abc/statuses"), None);
    }

    #[test]
    fn the_same_read_in_flight_goes_out_once_and_both_get_its_answer() {
        let l = Limits::default();
        let gate = Gate::new("bitbucket", "bb:fly".into());
        let reached = std::sync::atomic::AtomicUsize::new(0);
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let (gate, l, reached) = (&gate, &l, &reached);
        let (first, second) = std::thread::scope(|s| {
            let first = s.spawn(move || {
                gate.read_on(l, PR1, || {
                    reached.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    wait.recv().unwrap();
                    Ok((json!({"id": 1}), None))
                })
            });
            wait_until_held(l, "bb:fly", PR1, 2);
            let second = s.spawn(|| {
                gate.read_on(l, PR1, || {
                    reached.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok((json!({"id": "its own"}), None))
                })
            });
            wait_until_held(l, "bb:fly", PR1, 3);
            go.send(()).unwrap();
            (first.join().unwrap(), second.join().unwrap())
        });
        assert_eq!(first, Ok(json!({"id": 1})));
        assert_eq!(second, Ok(json!({"id": 1})), "the second waited for the first's answer");
        assert_eq!(reached.load(std::sync::atomic::Ordering::SeqCst), 1, "one read reached the host");
        assert_eq!(calls(l, "bb:fly"), 1, "and counts as one call");
    }

    #[test]
    fn an_answer_is_kept_30s_for_the_same_read_on_the_account() {
        let l = Limits::default();
        let gate = Gate::new("bitbucket", "bb:keep".into());
        let mut reached = 0;
        for _ in 0..3 {
            let v = gate
                .read_on(&l, PR1, || {
                    reached += 1;
                    Ok((json!({"id": 1}), None))
                })
                .unwrap();
            assert_eq!(v, json!({"id": 1}));
        }
        assert_eq!(reached, 1, "the kept answer was used");
        assert_eq!(calls(&l, "bb:keep"), 1);
        assert!(matches!(l.join("bb:keep", &format!("{PR1}/comments"), now_ts()), Join::Lead(_)), "another URL isn't shared");
        assert!(matches!(l.join("bb:other", PR1, now_ts()), Join::Lead(_)), "another account isn't shared");
        assert!(matches!(l.join("bb:keep", PR1, now_ts() + KEEP_SECS - 1.0), Join::Kept(_)));
        assert!(matches!(l.join("bb:keep", PR1, now_ts() + KEEP_SECS), Join::Lead(_)), "it's read again after 30s");
    }

    #[test]
    fn a_write_to_a_pr_drops_its_reads_and_lists_but_not_another_pr_s() {
        let l = Limits::default();
        let gate = Gate::new("bitbucket", "bb:w".into());
        let list = "https://api.bitbucket.org/2.0/repositories/ws/repo/pullrequests?state=OPEN";
        let comments = format!("{PR1}/comments?pagelen=100");
        for url in [PR1, comments.as_str(), PR2, list] {
            gate.read_on(&l, url, || Ok((json!(url), None))).unwrap();
        }
        Gate::new("bitbucket", "bb:else".into()).read_on(&l, PR1, || Ok((json!("else"), None))).unwrap();
        let mut wrote = 0;
        gate.write_on(&l, &format!("{PR1}/merge"), || {
            wrote += 1;
            Ok(((), None))
        })
        .unwrap();
        assert_eq!(wrote, 1);
        let now = now_ts();
        assert!(matches!(l.join("bb:w", PR1, now), Join::Lead(_)), "the PR's read is dropped");
        assert!(matches!(l.join("bb:w", &comments, now), Join::Lead(_)), "and its comments");
        assert!(matches!(l.join("bb:w", list, now), Join::Lead(_)), "and the list it's in");
        assert_eq!(l.join("bb:w", PR2, now).kept(), Some(json!(PR2)), "another PR's read stays");
        assert_eq!(l.join("bb:else", PR1, now).kept(), Some(json!("else")), "another account's stays");
        l.wrote("bb:w", "https://api.bitbucket.org/2.0/repositories/ws/repo/pullrequests");
        assert!(matches!(l.join("bb:w", PR2, now), Join::Lead(_)), "a write to no one PR drops them all");
        assert_eq!(l.join("bb:else", PR1, now).kept(), Some(json!("else")));
    }

    #[test]
    fn a_read_in_flight_when_a_write_lands_answers_its_waiters_but_isn_t_kept_or_joined() {
        let l = Limits::default();
        let Join::Lead(before) = l.join("bb:1", PR1, T0) else { panic!("no read in flight yet") };
        let Join::Wait(waiting) = l.join("bb:1", PR1, T0) else { panic!("the same read waits") };
        assert!(Arc::ptr_eq(&before, &waiting));
        l.wrote("bb:1", PR1);
        let Join::Lead(after) = l.join("bb:1", PR1, T0 + 1.0) else { panic!("a read after the write doesn't wait on the one before") };
        l.land("bb:1", PR1, &before, Landed::Answer(Ok(json!("old"))), T0 + 2.0);
        assert_eq!(waiting.wait(), Landed::Answer(Ok(json!("old"))), "those already waiting get its answer");
        assert!(matches!(l.join("bb:1", PR1, T0 + 3.0), Join::Wait(f) if Arc::ptr_eq(&f, &after)), "the answer from before the write isn't kept");
        l.land("bb:1", PR1, &after, Landed::Answer(Ok(json!("new"))), T0 + 4.0);
        assert_eq!(l.join("bb:1", PR1, T0 + 5.0).kept(), Some(json!("new")));
    }

    #[test]
    fn a_host_error_is_shared_with_those_waiting_but_not_kept() {
        let l = Limits::default();
        let Join::Lead(f) = l.join("bb:1", PR1, T0) else { panic!() };
        let Join::Wait(w) = l.join("bb:1", PR1, T0) else { panic!() };
        l.land("bb:1", PR1, &f, Landed::Answer(Err("Bitbucket answered 500".into())), T0 + 1.0);
        assert_eq!(w.wait(), Landed::Answer(Err("Bitbucket answered 500".into())));
        assert!(matches!(l.join("bb:1", PR1, T0 + 2.0), Join::Lead(_)), "the next read asks again");
    }

    #[test]
    fn a_read_that_never_reached_the_host_lets_those_waiting_try_themselves() {
        let l = Limits::default();
        l.hit("bb:held", "bitbucket", &Hit { retry_after: Some(600.0), ..hit("429") }, now_ts());
        let gate = Gate::new("bitbucket", "bb:held".into());
        let Join::Lead(led) = l.join("bb:held", PR1, now_ts()) else { panic!() };
        let mut reached = false;
        let res = std::thread::scope(|s| {
            let waiter = s.spawn(|| {
                gate.read_on(&l, PR1, || {
                    reached = true;
                    Ok((json!(1), None))
                })
            });
            wait_until_held(&l, "bb:held", PR1, 3);
            l.land("bb:held", PR1, &led, Landed::Retry, now_ts());
            waiter.join().unwrap()
        });
        let e = res.unwrap_err();
        assert!(e.starts_with("Bitbucket rate-limited until "), "the waiter tried itself and was held: {e}");
        assert!(!reached);
        assert!(matches!(l.join("bb:held", PR1, now_ts()), Join::Lead(_)), "nothing kept, nothing left in flight");
    }

    #[test]
    fn a_leader_that_unwinds_doesn_t_leave_its_read_in_flight() {
        let l = Limits::default();
        let gate = Gate::new("bitbucket", "bb:panic".into());
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gate.read_on(&l, PR1, || panic!("the read broke"))));
        assert!(unwound.is_err());
        assert!(matches!(l.join("bb:panic", PR1, now_ts()), Join::Lead(_)));
    }

    #[test]
    fn a_leader_refused_before_the_host_lets_its_waiter_try_itself() {
        let l = Limits::default();
        l.set_hosts([("bitbucket".to_string(), HostConfig { write_gap_secs: Some(60.0), ..Default::default() })].into());
        let now = now_ts();
        l.write_slot("bb:ref", "bitbucket", now).unwrap();
        l.write_slot("bb:ref", "bitbucket", now).unwrap();
        let gate = Gate::new("bitbucket", "bb:ref".into());
        let (gate, l) = (&gate, &l);
        let (led, waited) = std::thread::scope(|s| {
            let leader = s.spawn(move || gate.read_on(l, PR1, || panic!("the background read never reaches the host")));
            wait_until_held(l, "bb:ref", PR1, 2);
            let waiter = s.spawn(move || priority(|| gate.read_on(l, PR1, || Ok((json!("its own"), None)))));
            wait_until_held(l, "bb:ref", PR1, 3);
            l.headroom("bb:ref", "bitbucket", &Headroom { cap: None, near: true, reset_at: None }, now_ts());
            (leader.join().unwrap(), waiter.join().unwrap())
        });
        let e = led.unwrap_err();
        assert!(e.contains("holds its background reads"), "the leader was held by the slow restart: {e}");
        assert_eq!(waited, Ok(json!("its own")), "the waiter made its own read rather than share the leader's refusal");
    }

    #[test]
    fn a_leader_s_host_error_goes_to_its_waiter_and_isn_t_kept() {
        let l = Limits::default();
        let gate = Gate::new("bitbucket", "bb:err".into());
        let waiter_reached = std::sync::atomic::AtomicBool::new(false);
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let (gate, l, waiter_reached) = (&gate, &l, &waiter_reached);
        let (led, waited) = std::thread::scope(|s| {
            let leader = s.spawn(move || {
                gate.read_on(l, PR1, || {
                    wait.recv().unwrap();
                    Err(("Bitbucket answered 500".into(), None))
                })
            });
            wait_until_held(l, "bb:err", PR1, 2);
            let waiter = s.spawn(|| {
                gate.read_on(l, PR1, || {
                    waiter_reached.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok((json!(1), None))
                })
            });
            wait_until_held(l, "bb:err", PR1, 3);
            go.send(()).unwrap();
            (leader.join().unwrap(), waiter.join().unwrap())
        });
        assert_eq!(led, Err("Bitbucket answered 500".into()));
        assert_eq!(waited, Err("Bitbucket answered 500".into()), "the waiter got the host's answer");
        assert!(!waiter_reached.load(std::sync::atomic::Ordering::SeqCst));
        assert!(matches!(l.join("bb:err", PR1, now_ts()), Join::Lead(_)), "the error isn't kept");
    }

    #[test]
    fn an_empty_answer_isn_t_kept() {
        let l = Limits::default();
        let gate = Gate::new("bitbucket", "bb:null".into());
        let mut reached = 0;
        for _ in 0..2 {
            let v = gate.read_on(&l, PR1, || {
                reached += 1;
                Ok((Value::Null, None))
            });
            assert_eq!(v, Ok(Value::Null));
        }
        assert_eq!(reached, 2, "an unreadable body is asked for again");
    }

    #[test]
    fn a_failed_write_still_drops_the_pr_s_reads() {
        let l = Limits::default();
        let gate = Gate::new("bitbucket", "bb:wf".into());
        gate.read_on(&l, PR1, || Ok((json!(1), None))).unwrap();
        let e = gate.write_on(&l, &format!("{PR1}/merge"), || Err::<((), _), _>(("Can't reach Bitbucket".into(), None))).unwrap_err();
        assert_eq!(e, "Can't reach Bitbucket");
        assert!(matches!(l.join("bb:wf", PR1, now_ts()), Join::Lead(_)), "the write may have gone through: the PR's read is dropped");
    }

    #[test]
    fn a_get_is_a_shared_read_and_any_other_method_a_write() {
        let l = Limits::default();
        l.set_hosts([("bitbucket".to_string(), HostConfig { write_gap_secs: Some(0.0), ..Default::default() })].into());
        let gate = Gate::new("bitbucket", "bb:m".into());
        let comments = format!("{PR1}/comments");
        let mut reads = 0;
        for _ in 0..2 {
            gate.request_on(&l, "GET", PR1, || {
                reads += 1;
                Ok((json!("pr"), None))
            })
            .unwrap();
        }
        assert_eq!(reads, 1, "the second GET used the kept answer");
        for method in ["POST", "PUT", "DELETE"] {
            let mut wrote = 0;
            for _ in 0..2 {
                let v = gate
                    .request_on(&l, method, &comments, || {
                        wrote += 1;
                        Ok((json!({"id": wrote}), None))
                    })
                    .unwrap();
                assert_eq!(v, json!({"id": wrote}), "a {method} answers its own reply");
            }
            assert_eq!(wrote, 2, "each {method} goes out");
            assert!(matches!(l.join("bb:m", PR1, now_ts()), Join::Lead(_)), "a {method} drops the PR's reads");
            l.shared.lock().clear();
        }
        assert_eq!(l.accounts.lock()["bb:m"].writes.len(), 6, "they took write slots");
    }

    #[test]
    fn a_fresh_read_takes_only_an_answer_sent_after_it_began() {
        let l = Limits::default();
        let gate = Gate::new("bitbucket", "bb:fresh".into());
        gate.read_on(&l, PR1, || Ok((json!("before"), None))).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert_eq!(fresh(|| gate.read_on(&l, PR1, || Ok((json!("after"), None)))), Ok(json!("after")), "the kept answer is from before");
        assert_eq!(l.join("bb:fresh", PR1, now_ts()).kept(), Some(json!("after")), "and the fresh one is kept for the rest");
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert_eq!(fresh(|| gate.read_on(&l, PR1, || Ok((json!("again"), None)))), Ok(json!("again")));

        let Join::Lead(old) = l.join("bb:2", PR1, T0) else { panic!() };
        let Join::Lead(new) = l.join_after("bb:2", PR1, T0 + 2.0, T0 + 1.0) else { panic!("a read in flight since before isn't waited for") };
        assert!(matches!(l.join_after("bb:2", PR1, T0 + 3.0, T0 + 1.0), Join::Wait(f) if Arc::ptr_eq(&f, &new)), "one sent since is");
        assert!(matches!(l.join("bb:2", PR1, T0 + 3.0), Join::Wait(f) if Arc::ptr_eq(&f, &new)), "and others wait on the new one");
        l.land("bb:2", PR1, &old, Landed::Answer(Ok(json!("old"))), T0 + 4.0);
        assert!(matches!(l.join("bb:2", PR1, T0 + 5.0), Join::Wait(_)), "the old answer isn't kept");
        l.land("bb:2", PR1, &new, Landed::Answer(Ok(json!("new"))), T0 + 6.0);
        assert_eq!(l.join_after("bb:2", PR1, T0 + 7.0, T0 + 2.0).kept(), Some(json!("new")), "kept and sent since");
        assert!(matches!(l.join_after("bb:2", PR1, T0 + 7.0, T0 + 2.5), Join::Lead(_)), "kept but sent before");
    }

    #[test]
    fn a_newer_last_wins_write_takes_the_queued_one_s_slot_and_outcome() {
        let l = Limits::default();
        let (at, turn, older) = l.queue_last::<String>("bb:1", "bitbucket", "PR1 reviewers", T0).unwrap();
        assert_eq!((at, turn), (T0, 0));
        let (at, turn, newer) = l.queue_last::<String>("bb:1", "bitbucket", "PR1 reviewers", T0 + 0.5).unwrap();
        assert_eq!((at, turn), (T0, 1), "it goes at the older one's slot");
        assert!(Arc::ptr_eq(&older, &newer), "and lands the outcome the older one waits on");
        assert_eq!(l.queue_last::<String>("bb:1", "bitbucket", "PR1 base", T0 + 0.5).unwrap().0, T0 + 2.0, "it took no slot of its own; another field takes the next");
        assert_eq!(l.queue_last::<String>("bb:1", "bitbucket", "PR2 reviewers", T0 + 0.5).unwrap().0, T0 + 4.0, "and so does another PR");
        assert_eq!(l.queue_last::<String>("bb:2", "bitbucket", "PR1 reviewers", T0 + 0.5).unwrap().0, T0 + 0.5, "another account's line is its own");
        assert!(!l.go_last("bb:1", "PR1 reviewers", 0, &older), "the older one was replaced");
        assert!(l.go_last("bb:1", "PR1 reviewers", 1, &newer), "the newer one goes");
        assert!(!l.go_last("bb:1", "PR1 reviewers", 1, &newer), "once");
        let (at, turn, after) = l.queue_last::<String>("bb:1", "bitbucket", "PR1 reviewers", T0 + 1.0).unwrap();
        assert_eq!((at, turn), (T0 + 6.0, 0), "a write after one went out waits for a slot of its own");
        assert!(!Arc::ptr_eq(&older, &after));
        assert!(!l.go_last("bb:1", "PR1 reviewers", 0, &older), "a replaced write slow to wake never takes the later one's turn");
    }

    #[test]
    fn a_full_write_line_still_lets_a_last_wins_write_replace_a_queued_one() {
        let l = Limits::default();
        l.queue_last::<String>("bb:1", "bitbucket", "PR1 base", T0).unwrap();
        for _ in 0..WRITE_LINE {
            l.write_slot("bb:1", "bitbucket", T0).unwrap();
        }
        assert!(l.write_slot("bb:1", "bitbucket", T0).is_err(), "the line is full");
        assert_eq!(l.queue_last::<String>("bb:1", "bitbucket", "PR1 base", T0).map(|(at, turn, _)| (at, turn)), Ok((T0, 1)), "it takes the queued one's place");
        let e = l.queue_last::<String>("bb:1", "bitbucket", "PR1 description", T0).unwrap_err();
        assert!(e.contains("write queue is full"), "{e}");
    }

    /// Whose turn the last-wins write marked `last` on `key` is, while one waits for its slot.
    fn queued_turn(l: &Limits, key: &str, last: &str) -> Option<u64> {
        l.queued.lock().get(&(key.to_string(), last.to_string(), TypeId::of::<String>())).map(|q| q.turn)
    }

    fn wait_until_queued(l: &Limits, key: &str, last: &str, turn: u64) {
        let give_up = now_ts() + 5.0;
        while queued_turn(l, key, last) != Some(turn) {
            assert!(now_ts() < give_up, "turn {turn} never queued");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// `l` with writes on Bitbucket `gap` seconds apart.
    fn gapped(gap: f64) -> Limits {
        let l = Limits::default();
        l.set_hosts([("bitbucket".to_string(), HostConfig { write_gap_secs: Some(gap), ..Default::default() })].into());
        l
    }

    #[test]
    fn a_queued_last_wins_write_is_replaced_by_a_newer_one_and_gets_its_answer() {
        let l = gapped(1.0);
        let gate = Gate::new("bitbucket", "bb:lw".into());
        l.write_slot("bb:lw", "bitbucket", now_ts()).unwrap();
        let reached = Mutex::new(vec![]);
        let (gate, l, reached) = (&gate, &l, &reached);
        let write = |last: Option<&'static str>, answer: &'static str| {
            let f = move || {
                gate.call_on(l, BACKGROUND_WAIT_SECS, true, || {
                    reached.lock().push(answer);
                    Ok((answer.to_string(), None))
                })
            };
            match last {
                Some(last) => last_wins(last.to_string(), f),
                None => f(),
            }
        };
        let (older, newer, base, plain) = std::thread::scope(|s| {
            let older = s.spawn(move || write(Some("PR1 reviewers"), "older"));
            wait_until_queued(l, "bb:lw", "PR1 reviewers", 0);
            let newer = s.spawn(move || write(Some("PR1 reviewers"), "newer"));
            wait_until_queued(l, "bb:lw", "PR1 reviewers", 1);
            let base = s.spawn(move || write(Some("PR1 base"), "base"));
            let plain = s.spawn(move || write(None, "plain"));
            (older.join().unwrap(), newer.join().unwrap(), base.join().unwrap(), plain.join().unwrap())
        });
        assert_eq!(older, Ok("newer".to_string()), "the replaced caller gets the newer one's answer");
        assert_eq!(newer, Ok("newer".to_string()));
        assert_eq!((base, plain), (Ok("base".to_string()), Ok("plain".to_string())), "another field and an unmarked write each go");
        let mut reached = reached.lock().clone();
        reached.sort();
        assert_eq!(reached, vec!["base", "newer", "plain"], "the older one never reached the host");
        assert_eq!(calls(l, "bb:lw"), 3);
        assert_eq!(queued_turn(l, "bb:lw", "PR1 reviewers"), None, "nothing left queued");
    }

    #[test]
    fn a_last_wins_write_already_gone_out_isn_t_replaced() {
        let l = gapped(0.2);
        let gate = Gate::new("bitbucket", "bb:out".into());
        let (inside, is_in) = std::sync::mpsc::channel::<()>();
        let (finish, finished) = std::sync::mpsc::channel::<()>();
        let (gate, l) = (&gate, &l);
        let (first, second) = std::thread::scope(|s| {
            let first = s.spawn(move || {
                last_wins("PR1 base".into(), || {
                    gate.call_on(l, BACKGROUND_WAIT_SECS, true, || {
                        inside.send(()).unwrap();
                        finished.recv().unwrap();
                        Ok(("first".to_string(), None))
                    })
                })
            });
            is_in.recv().unwrap();
            let second = s.spawn(move || last_wins("PR1 base".into(), || gate.call_on(l, BACKGROUND_WAIT_SECS, true, || Ok(("second".to_string(), None)))));
            wait_until_queued(l, "bb:out", "PR1 base", 0);
            finish.send(()).unwrap();
            (first.join().unwrap(), second.join().unwrap())
        });
        assert_eq!((first, second), (Ok("first".to_string()), Ok("second".to_string())), "each went out with its own answer");
        assert_eq!(calls(l, "bb:out"), 2);
    }

    #[test]
    fn a_replaced_write_hears_when_the_newer_one_breaks_off() {
        let l = gapped(0.5);
        let gate = Gate::new("bitbucket", "bb:broke".into());
        l.write_slot("bb:broke", "bitbucket", now_ts()).unwrap();
        let (gate, l) = (&gate, &l);
        let older = std::thread::scope(|s| {
            let older = s.spawn(move || last_wins("PR1 description".into(), || gate.call_on(l, BACKGROUND_WAIT_SECS, true, || Ok(("older".to_string(), None)))));
            wait_until_queued(l, "bb:broke", "PR1 description", 0);
            let newer = s.spawn(move || last_wins("PR1 description".into(), || gate.call_on::<String>(l, BACKGROUND_WAIT_SECS, true, || panic!("the write broke"))));
            assert!(newer.join().is_err());
            older.join().unwrap()
        });
        let e = older.unwrap_err();
        assert!(e.contains("broke off"), "{e}");
        assert_eq!(queued_turn(l, "bb:broke", "PR1 description"), None);
    }
}
