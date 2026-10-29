//! The PR feed: PR activity that arrives as events instead of the board's own poll. Any listener (a
//! Slack app reading the team's git-notification channel, a webhook relay, a script) posts one event
//! per PR change or build to `POST /prs/event` (`tb feed event`), and the board reads that one PR
//! again and steps it. With `[feed] on`, the board also watches the feed's health and holds the
//! review loop while it can't trust it.
//!
//! # Health
//!
//! The feed is *stuck* when it has sent heartbeats (`POST /prs/heartbeat`) before and none came in
//! `stuck_secs` (180 by default), and *silent* when neither an event nor a heartbeat came in
//! `silent_secs` (an hour) inside its hours. Its hours are the listener's own when its heartbeats say
//! (`active`, `idle_until`): while it says it's idle it isn't stuck or silent, however long it's
//! quiet, and from `idle_until` on it's timed again. A listener that doesn't say keeps the work hours.
//! Either problem makes it unhealthy:
//!
//! - the board restarts it (`restart_cmd`, or the `listener` it runs itself) `restart_mins` after it
//!   went bad (1, 5 and 15 minutes), and raises the `pr-feed` alert when a restart fails or it's
//!   still bad five minutes after the last one; the alert clears once the feed is healthy again;
//! - [`feed_healthy`] answers false, and [`holding`] tells the reviewer ask, nudge and swap logic to
//!   wait; a stuck feed holds even a PR's first ask, at any hour;
//! - outside the feed's hours (the listener idle, or the work hours closed for one that doesn't
//!   say) the feed may miss events, so [`holding`] holds everything but a PR's first ask;
//! - once it's healthy again, and after every connect (the first heartbeat, a heartbeat's own
//!   `connected_at`, the listener waking from idle, the board starting its `listener`), [`holding`]
//!   keeps holding for `settle_secs` (5 minutes) while the events it missed catch up;
//! - the PR poll (`[intervals] prs`) runs as a fallback; while the feed is healthy it only runs with
//!   `poll_while_healthy`.
//!
//! Without `[feed] on` the feed is never unhealthy and the poll runs as before; events still refresh
//! their PR.
//!
//! # Sources
//!
//! The work event stream (`stream.rs`) has four sources, prs, builds, jira and qa, and health is kept
//! per source ([`source`]): prs is the PR feed above (watched with `[feed] on` or in stream mode),
//! the others are watched in stream mode with their `[stream.sources.<name>]` settings, keep their
//! state in `stream_feed_<name>` and raise `stream-<name>`. A heartbeat names the sources it covers
//! ([`heartbeat_for`]). While a stream-mode source is healthy its own poll rests ([`poll_rests`]): prs
//! is the PR poll, builds the owner-PR read (`prbuilds.rs`), qa the Jira comment poll (`qa.rs`).
//!
//! # Relays
//!
//! Each stream relay (an event's `source`) is also kept on its own, under its source's `relays`
//! ([`note_relay`]): its own heartbeats, idle state and last event, and what its heartbeats say it
//! covers (`covers: [{provider, repo?}]`, every PR when left out). Once a relay has reported for a
//! source, the source's health is its relays': it's unhealthy while any of them is stuck or silent,
//! each timed on its own, so one relay's heartbeat or idle state doesn't cover for another's silence.
//! A relay that never sent a heartbeat (a webhook relay) is event-only: never stuck or silent, so its
//! quiet doesn't count against the source, and a source's `restart_cmd` only runs for a listener's
//! problem. In either mode, a PR a healthy, active relay covers by its heartbeats ([`pr_poll_rests`])
//! is only read by the poll to reconcile: every `reconcile_secs` (an hour), and once after the relay
//! comes back from being stuck (`back_at`) or connects ([`pr_poll_skips`]); the others are polled as
//! in board mode, and so are its PRs again once it goes stuck or silent. In board mode each of its
//! events still reads its PR from the host, and [`relay_hint`] says so, with the command to switch.
//! The old listener's endpoints (`POST /prs/heartbeat`, `POST /prs/event`) aren't relays: without
//! stream relays they work as above.
//!
//! # State
//!
//! The setting `pr_feed` keeps `{since, last_event_at, last_event, last_heartbeat_at, heartbeats,
//! unhealthy_since, problem, restarts: [{at, ok, error?}], alerted_at, healthy_at, connected_at,
//! active, idle_until}` (`healthy_at`: when it last came back, `connected_at`: when the listener last
//! connected; the later starts the settle window; `active` and `idle_until`: the listener's own hours,
//! as its last heartbeat that said gave them).

use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, dispatch, hours, p, prflow};

/// `[feed]` in config.toml.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FeedConfig {
    /// PR updates come from a feed: watch its health and hold the review loop while it's unhealthy.
    pub on: bool,
    /// A command the board runs (`/bin/sh -c`) and keeps running: the listener that posts the events.
    /// It gets TASKBOARD_URL and TB_EVENT_URL. Empty: the listener runs on its own (a LaunchAgent).
    pub listener: String,
    /// Restarts the feed when it's unhealthy (`launchctl kickstart -k gui/501/com.me.pr-listener`).
    /// Empty: the board restarts its own `listener`.
    pub restart_cmd: String,
    /// No heartbeat for this long (once the feed has sent one): it's stuck.
    pub stuck_secs: f64,
    /// No event or heartbeat for this long inside the feed's hours: it's silent.
    pub silent_secs: f64,
    /// Minutes after the feed went bad at which the board restarts it.
    pub restart_mins: Vec<f64>,
    /// Keep polling every PR (`[intervals] prs`) while the feed is healthy too.
    pub poll_while_healthy: bool,
    /// After the feed comes back or the listener connects, reviewer asks, nudges and swaps wait this
    /// many more seconds while the events it missed catch up.
    pub settle_secs: f64,
    /// A PR a healthy, active stream relay covers is read by the poll only this often (and once each
    /// time the relay comes back or connects): its events carry the rest. 0: never while covered.
    pub reconcile_secs: f64,
}

impl Default for FeedConfig {
    fn default() -> Self {
        FeedConfig {
            on: false,
            listener: String::new(),
            restart_cmd: String::new(),
            stuck_secs: 180.0,
            silent_secs: 3600.0,
            restart_mins: vec![1.0, 5.0, 15.0],
            poll_while_healthy: false,
            settle_secs: 300.0,
            reconcile_secs: 3600.0,
        }
    }
}

const KEY: &str = "pr_feed";
pub const ALERT_KEY: &str = "pr-feed";
/// After the last restart, how long the feed may stay bad before the owner is told.
const ALERT_AFTER_LAST_SECS: f64 = 300.0;
const RESTART_TIMEOUT_SECS: f64 = 60.0;

/// One source's feed (`stream::SOURCES`) as its health is kept: prs is the PR feed, set by `[feed]`;
/// builds, jira and qa by their `[stream.sources.<name>]`. A source is watched (`on`) while its mode is
/// stream, and prs also with `[feed] on`.
pub struct Source {
    pub name: String,
    key: String,
    alert: String,
    label: String,
    pub on: bool,
    stuck_secs: f64,
    silent_secs: f64,
    restart_mins: Vec<f64>,
    restart_cmd: String,
    listener: String,
    settle_secs: f64,
    poll_while_healthy: bool,
}

pub fn source(app: &App, name: &str) -> Source {
    let streams = crate::stream::streams(app, name);
    if name == "prs" {
        let c = &app.cfg.feed;
        return Source {
            name: name.into(),
            key: KEY.into(),
            alert: ALERT_KEY.into(),
            label: "the PR feed".into(),
            on: c.on || streams,
            stuck_secs: c.stuck_secs,
            silent_secs: c.silent_secs,
            restart_mins: c.restart_mins.clone(),
            restart_cmd: c.restart_cmd.clone(),
            listener: c.listener.clone(),
            settle_secs: c.settle_secs,
            poll_while_healthy: c.poll_while_healthy && !streams,
        };
    }
    let c = crate::stream::source_config(app, name);
    Source {
        name: name.into(),
        key: format!("stream_feed_{name}"),
        alert: format!("stream-{name}"),
        label: format!("the {} stream", crate::stream::label(name)),
        on: streams,
        stuck_secs: c.stuck_secs,
        silent_secs: c.silent_secs,
        restart_mins: c.restart_mins,
        restart_cmd: c.restart_cmd,
        listener: String::new(),
        settle_secs: c.settle_secs,
        poll_while_healthy: false,
    }
}

fn prs(app: &App) -> Source {
    source(app, "prs")
}

fn state(app: &App, src: &Source) -> Row {
    jloads_obj(app.db.get_setting(&src.key).ok().flatten().as_deref())
}

fn save(app: &App, src: &Source, st: &Row) -> Result<()> {
    app.db.set_setting(&src.key, Some(&jdumps(&Value::Object(st.clone()))))
}

fn ts(st: &Row, k: &str) -> Option<f64> {
    st.s(k).and_then(parse_iso)
}

/// Whether the listener says how its hours go (its heartbeats carry `active`).
fn says_hours(st: &Row) -> bool {
    st.get("active").and_then(|v| v.as_bool()).is_some()
}

/// Whether the listener says it's idle now (its own hours): its last heartbeat that said had
/// `active: false`, and its `idle_until`, if it gave one, hasn't come yet.
fn listener_idle(st: &Row, now: f64) -> bool {
    st.get("active").and_then(|v| v.as_bool()) == Some(false) && ts(st, "idle_until").is_none_or(|u| u > now)
}

/// Why the PR feed is outside its hours now, if it is: the listener says it's idle, or (for a
/// listener that doesn't say) the work hours are closed. The feed may miss events then.
pub fn off_hours(app: &App) -> Option<String> {
    off_hours_of(app, &prs(app))
}

fn off_hours_of(app: &App, src: &Source) -> Option<String> {
    if !src.on {
        return None;
    }
    let st = state(app, src);
    if listener_idle(&st, now_ts()) {
        return Some(match st.s("idle_until") {
            Some(u) => format!("{}'s listener is idle until {}", src.label, local_clock(Some(u))),
            None => format!("{}'s listener is idle", src.label),
        });
    }
    if !says_hours(&st) && !hours::is_open(app) {
        return Some(format!("it's outside the work hours, when {} may miss events", src.label));
    }
    None
}

/// What's wrong with the PR feed, if anything: (`stuck` or `silent`, a sentence).
pub fn problem(app: &App) -> Option<(&'static str, String)> {
    problem_of(app, &prs(app))
}

/// What's wrong with a source's feed, if anything: (`stuck` or `silent`, a sentence). An idle
/// listener is neither; once its `idle_until` has come, it's timed from then.
pub fn problem_of(app: &App, src: &Source) -> Option<(&'static str, String)> {
    if !src.on {
        return None;
    }
    let st = state(app, src);
    let relays = relays_of(&st);
    if relays.is_empty() {
        return timed_problem(app, src, &st, &src.label);
    }
    relays.iter().find_map(|(name, r)| relay_problem(app, src, name, r))
}

/// One relay's problem, timed on its own: (`stuck` or `silent`, a sentence naming it). A relay that
/// never sent a heartbeat (a webhook relay) is event-only: its quiet is no problem. It covers no PR,
/// so the poll still reads its PRs, and a source's `restart_cmd` only ever runs for a listener's.
fn relay_problem(app: &App, src: &Source, name: &str, r: &Row) -> Option<(&'static str, String)> {
    if event_only(r) {
        return None;
    }
    timed_problem(app, src, r, &format!("{}'s relay {name}", src.label))
}

/// A relay that has never sent a heartbeat: it only posts when its host fires a webhook.
fn event_only(r: &Row) -> bool {
    !r.contains_key("last_heartbeat_at")
}

/// What a relay covers, as `GET /stream` shows it: `"all"` when its heartbeats left `covers` out,
/// `[]` when it never sent one, else the list.
fn covers_shown(r: &Row) -> Value {
    if event_only(r) {
        return json!([]);
    }
    r.get("covers").filter(|c| c.is_array()).cloned().unwrap_or_else(|| json!("all"))
}

/// The stuck and silent checks on one feed's state (a source's, or one relay's).
fn timed_problem(app: &App, src: &Source, st: &Row, who: &str) -> Option<(&'static str, String)> {
    let now = now_ts();
    if listener_idle(st, now) {
        return None;
    }
    let woke = ts(st, "idle_until").filter(|_| says_hours(st));
    if let Some(hb) = ts(st, "last_heartbeat_at") {
        let from = woke.map_or(hb, |w| w.max(hb));
        if src.stuck_secs > 0.0 && now - from > src.stuck_secs {
            return Some(("stuck", format!("{who} hasn't sent a heartbeat for {}", span(now - hb))));
        }
    }
    let heard = [ts(st, "last_event_at"), ts(st, "last_heartbeat_at"), ts(st, "since"), woke].into_iter().flatten().fold(f64::MIN, f64::max);
    let in_hours = says_hours(st) || hours::is_open(app);
    if src.silent_secs > 0.0 && heard > f64::MIN && now - heard > src.silent_secs && in_hours {
        return Some(("silent", format!("nothing has come from {who} for {}", span(now - heard))));
    }
    None
}

/// A source's stream relays, by name.
fn relays_of(st: &Row) -> Vec<(String, Row)> {
    st.get("relays").and_then(|v| v.as_object()).map(|m| m.iter().filter_map(|(k, v)| v.as_object().map(|r| (k.clone(), r.clone()))).collect()).unwrap_or_default()
}

/// Whether a relay's heartbeats say it covers this PR: every PR without `covers`, else an entry with
/// its provider and, when the entry names one, its repo (by full or short name, in any case). A relay
/// that never sent a heartbeat covers nothing.
fn covers(r: &Row, host: &str, repo: &str) -> bool {
    if event_only(r) {
        return false;
    }
    let Some(list) = r.get("covers").and_then(|v| v.as_array()) else { return true };
    let short = |x: &str| x.rsplit('/').next().unwrap_or("").to_lowercase();
    list.iter().any(|c| {
        let want = c["repo"].as_str().unwrap_or("");
        let same_repo = want.is_empty() || want.eq_ignore_ascii_case(repo) || (!(want.contains('/') && repo.contains('/')) && short(want) == short(repo));
        c["provider"].as_str().is_some_and(|p| p.eq_ignore_ascii_case(host)) && same_repo
    })
}

/// A relay that's healthy and not idle now and covers this PR (or, with none, every PR).
fn relay_covers_now(app: &App, src: &Source, name: &str, r: &Row, pr: Option<(&str, &str)>) -> bool {
    let covered = match pr {
        Some((host, repo)) => covers(r, host, repo),
        None => !event_only(r) && !r.contains_key("covers"),
    };
    covered && !listener_idle(r, now_ts()) && relay_problem(app, src, name, r).is_none()
}

/// The PR poll runs but skips some PRs: a healthy, active relay covers a list of them, and none
/// covers every PR.
fn polling_some(app: &App, src: &Source) -> bool {
    if src.name != "prs" || poll_rests(app, "prs") || !src.on || src.poll_while_healthy {
        return false;
    }
    relays_of(&state(app, src)).iter().any(|(n, r)| {
        r.get("covers").and_then(|c| c.as_array()).is_some_and(|c| !c.is_empty())
            && !event_only(r)
            && !listener_idle(r, now_ts())
            && relay_problem(app, src, n, r).is_none()
    })
}

/// While the PR poll [polls some](polling_some): the open PRs it still reads, by repo, and why: no
/// healthy relay covers the repo (`not covered`), or one does and the PR's reconcile is due (`reconcile`).
fn polled_prs(app: &App, src: &Source) -> Vec<Value> {
    if !polling_some(app, src) {
        return vec![];
    }
    let tasks = app
        .db
        .q(
            "SELECT * FROM tasks WHERE pr_host IN ('github', 'bitbucket') AND pr_num IS NOT NULL AND (pr_phase IS NULL OR pr_phase NOT IN ('merged', 'declined'))",
            p![],
        )
        .unwrap_or_default();
    let mut by: std::collections::BTreeMap<(String, String, &str), i64> = Default::default();
    for t in tasks.iter().filter(|t| !pr_poll_skips(app, t)) {
        let (host, repo) = (t.st("pr_host"), t.st("pr_repo"));
        let why = if pr_poll_rests(app, &host, &repo) { "reconcile" } else { "not covered" };
        *by.entry((host, repo, why)).or_default() += 1;
    }
    by.into_iter().map(|((host, repo, why), prs)| json!({"host": host, "repo": repo, "prs": prs, "why": why})).collect()
}

pub(crate) fn span(secs: f64) -> String {
    let m = (secs / 60.0).round() as i64;
    if secs < 60.0 {
        plural(secs.round() as i64, "second")
    } else if m < 120 {
        plural(m, "minute")
    } else {
        plural(m / 60, "hour")
    }
}

/// The gate for the review loop: the PR feed is healthy (always while it isn't watched). Reviewer
/// asks, nudges and swaps check it through [`holding`].
pub fn feed_healthy(app: &App) -> bool {
    problem(app).is_none()
}

/// Seconds left of the PR feed's settle window, if it's in one.
pub fn settling(app: &App) -> Option<f64> {
    settling_of(app, &prs(app))
}

/// Seconds left of a source's settle window after its feed came back or the listener connected
/// (`settle_secs`), if it's in one. A feed that's healthy again before [`check`] has noticed is in it
/// too.
fn settling_of(app: &App, src: &Source) -> Option<f64> {
    if !src.on || src.settle_secs <= 0.0 || problem_of(app, src).is_some() {
        return None;
    }
    let st = state(app, src);
    if st.contains_key("unhealthy_since") {
        return Some(src.settle_secs);
    }
    let from = [ts(&st, "healthy_at"), ts(&st, "connected_at")].into_iter().flatten().reduce(f64::max)?;
    let left = src.settle_secs - (now_ts() - from);
    (left > 0.0).then_some(left)
}

/// Why the reviewer ask, nudge or swap logic should wait now, if it should: the PR feed is unhealthy,
/// it came back or connected less than `settle_secs` ago, or it's outside its hours ([`off_hours`]).
/// `first_ask` is the first request for review on a PR: it still goes out outside the feed's hours,
/// but not when the feed is stuck (down or stale) or settling.
pub fn holding(app: &App, first_ask: bool) -> Option<String> {
    if let Some((_, why)) = problem(app) {
        return Some(format!("Holding reviewer asks, nudges and swaps: {why}."));
    }
    if let Some(left) = settling(app) {
        return Some(format!("Holding reviewer asks, nudges and swaps: the PR feed just came back, so its missed events catch up first ({} left).", span(left)));
    }
    let why = off_hours(app)?;
    (!first_ask).then(|| format!("Holding reviewer asks, nudges and swaps: {why}; a PR's first ask still goes out."))
}

/// Whether a source's own poll rests this round: its feed is watched and healthy (and, for prs,
/// without `poll_while_healthy`). prs is the PR poll, builds the owner-PR poll, qa the QA comment
/// poll; jira has none.
pub fn poll_rests(app: &App, name: &str) -> bool {
    let src = source(app, name);
    if !src.on || src.poll_while_healthy {
        return false;
    }
    let relays = relays_of(&state(app, &src));
    if name == "prs" && !relays.is_empty() {
        // The whole PR poll rests only while a healthy, active relay covers every PR; otherwise it runs
        // and skips the PRs one covers (`pr_poll_rests`).
        return relays.iter().any(|(n, r)| relay_covers_now(app, &src, n, r, None));
    }
    problem_of(app, &src).is_none()
}

/// Whether a healthy, active stream relay covers this PR's provider and repo now, in either mode (none
/// does with `poll_while_healthy`).
pub fn pr_poll_rests(app: &App, host: &str, repo: &str) -> bool {
    let src = prs(app);
    !src.poll_while_healthy && relays_of(&state(app, &src)).iter().any(|(n, r)| relay_covers_now(app, &src, n, r, Some((host, repo))))
}

/// When the healthy, active stream relays that cover this provider and repo now ([`pr_poll_rests`]) last
/// connected or came back from being stuck, the latest of them; None when none covers it.
pub fn relay_back_at(app: &App, host: &str, repo: &str) -> Option<f64> {
    let src = prs(app);
    if src.poll_while_healthy {
        return None;
    }
    relays_of(&state(app, &src))
        .iter()
        .filter(|(n, r)| relay_covers_now(app, &src, n, r, Some((host, repo))))
        .map(|(_, r)| [ts(r, "connected_at"), ts(r, "back_at")].into_iter().flatten().fold(f64::MIN, f64::max))
        .reduce(f64::max)
}

/// Whether the timed PR poll skips this PR this round: a healthy, active stream relay covers it
/// ([`pr_poll_rests`]) and its host was read (`read_at`) since that relay last came back or connected,
/// less than `reconcile_secs` ago. The relay's events read the PR (board mode) or are its state
/// (stream mode); the slow reconcile catches what they missed. A relay that goes stuck or silent stops
/// covering, so its PRs are polled again from the next round.
pub fn pr_poll_skips(app: &App, t: &Row) -> bool {
    let back = relay_back_at(app, t.s("pr_host").unwrap_or(""), t.s("pr_repo").unwrap_or(""));
    let (Some(back), Some(read)) = (back, ts(&jloads_obj(t.s("pr_flow")), "read_at")) else { return false };
    let every = app.cfg.feed.reconcile_secs;
    read >= back && (every <= 0.0 || now_ts() - read < every)
}

/// While builds streams and a healthy, active builds relay covers this repo now: the latest time one
/// that covers it connected or came back. The master watch's timer rests on it and reads the branch
/// again only to reconcile ([`crate::breaks::tick`]).
pub fn branch_covered_since(app: &App, host: &str, repo: &str) -> Option<f64> {
    if !crate::stream::streams(app, "builds") {
        return None;
    }
    let src = source(app, "builds");
    relays_of(&state(app, &src))
        .iter()
        .filter(|(n, r)| relay_covers_now(app, &src, n, r, Some((host, repo))))
        .map(|(_, r)| [ts(r, "connected_at"), ts(r, "back_at")].into_iter().flatten().fold(f64::MIN, f64::max))
        .reduce(f64::max)
}

/// Whether the PR poll runs this round: always without the feed, else while it's unhealthy (or with
/// `poll_while_healthy`). With stream relays it always runs, and skips the PRs they cover until their
/// reconcile is due ([`pr_poll_skips`]).
pub fn poll_due(app: &App) -> bool {
    !poll_rests(app, "prs") || !relays_of(&state(app, &prs(app))).is_empty()
}

/// What a relay says it covers, in words: "every PR", or "bitbucket acme/webapp, github every repo".
fn covers_words(r: &Row) -> String {
    match covers_shown(r) {
        Value::Array(a) => a
            .iter()
            .map(|c| format!("{} {}", c["provider"].as_str().unwrap_or("?"), c["repo"].as_str().unwrap_or("every repo")))
            .collect::<Vec<_>>()
            .join(", "),
        _ => "every PR".into(),
    }
}

/// For a source in board mode that a healthy, active relay covers: what the board still polls
/// although the relay covers it, and the command that switches the source to stream mode.
pub fn relay_hint(app: &App, src: &Source) -> Option<String> {
    if crate::stream::streams(app, &src.name) {
        return None;
    }
    let now = now_ts();
    let covering: Vec<(String, String)> = relays_of(&state(app, src))
        .iter()
        .filter(|(n, r)| {
            !event_only(r)
                && r.get("covers").and_then(|c| c.as_array()).is_none_or(|c| !c.is_empty())
                && !listener_idle(r, now)
                && relay_problem(app, src, n, r).is_none()
        })
        .map(|(n, r)| (n.clone(), covers_words(r)))
        .collect();
    let (relay, what) = covering.first()?;
    let more = if covering.len() > 1 { format!(" (and {} more)", plural(covering.len() as i64 - 1, "relay")) } else { String::new() };
    let switch = format!("`tb stream mode {} stream` would stop that", src.name);
    Some(if src.name == "prs" {
        let every = app.cfg.feed.reconcile_secs;
        let poll = if every > 0.0 { format!("the poll reads them again every {}", span(every)) } else { "the poll leaves them".into() };
        format!("prs: relay {relay}{more} covers {what}, so {poll}, but each of its events still reads its PR from the host; {switch}")
    } else {
        format!("{}: polling although relay {relay}{more} covers {what}; {switch}", src.name)
    })
}

/// [`relay_hint`] for each source that has one.
pub fn relay_hints(app: &App) -> Vec<String> {
    crate::stream::SOURCES.iter().filter_map(|name| relay_hint(app, &source(app, name))).collect()
}

/// `GET /prs/feed` (`tb feed`).
pub fn health(app: &App) -> Value {
    let src = prs(app);
    let mut h = health_of(app, &src);
    h["on"] = json!(src.on);
    h["listener"] = if src.listener.trim().is_empty() { Value::Null } else { json!(listener_running()) };
    h["holding"] = json!(holding(app, false));
    h["rate_limits"] = json!(crate::prhost::limit::status());
    h
}

/// One source's feed health, as `GET /stream` and `GET /prs/feed` show it.
pub fn health_of(app: &App, src: &Source) -> Value {
    let st = state(app, src);
    let p = problem_of(app, src);
    json!({
        "watched": src.on,
        "healthy": p.is_none(),
        "problem": p.as_ref().map(|(k, _)| *k),
        "why": p.as_ref().map(|(_, w)| w.clone()),
        "polling": !poll_rests(app, &src.name),
        "polling_some": polling_some(app, src),
        "polled": polled_prs(app, src),
        "last_event_at": st.v("last_event_at"),
        "last_event": st.v("last_event"),
        "last_heartbeat_at": st.v("last_heartbeat_at"),
        "unhealthy_since": st.v("unhealthy_since"),
        "restarts": st.get("restarts").cloned().unwrap_or(json!([])),
        "settling_secs": settling_of(app, src).map(|s| s.round()),
        "connected_at": st.v("connected_at"),
        "active": st.v("active"),
        "idle_until": st.v("idle_until"),
        "off_hours": off_hours_of(app, src),
        "relays": relays_of(&st).iter().map(|(name, r)| relay_health(app, src, name, r)).collect::<Vec<_>>(),
        "relay_hint": relay_hint(app, src),
    })
}

/// One relay's health under its source, as `GET /stream` lists it.
fn relay_health(app: &App, src: &Source, name: &str, r: &Row) -> Value {
    let p = relay_problem(app, src, name, r);
    json!({
        "relay": name,
        "healthy": p.is_none(),
        "problem": p.as_ref().map(|(k, _)| *k),
        "why": p.as_ref().map(|(_, w)| w.clone()),
        "idle": listener_idle(r, now_ts()),
        "last_event_at": r.v("last_event_at"),
        "last_event": r.v("last_event"),
        "last_heartbeat_at": r.v("last_heartbeat_at"),
        "connected_at": r.v("connected_at"),
        "active": r.v("active"),
        "idle_until": r.v("idle_until"),
        "event_only": event_only(r),
        "covers": covers_shown(r),
    })
}

/// A time a heartbeat gives (`idle_until`, `connected_at`), as ISO; 400 when it isn't one.
fn when_of(body: &Value, key: &str) -> Result<Option<String>> {
    let v = body_str(body, key);
    if v.is_empty() {
        return Ok(None);
    }
    match parse_iso(&v) {
        Some(t) => Ok(Some(iso(t))),
        None => err(400, format!("{key} is a time like 2026-10-08T08:00:00Z.")),
    }
}

/// `POST /prs/heartbeat` (`tb feed heartbeat`): the PR feed is alive. `{active?, idle_until?,
/// connected_at?}`: the listener's own hours (`active: false` while it's off, until `idle_until`;
/// `activeNow` is read too) and when it last connected. The first heartbeat, one with a later
/// `connected_at` and one that says it's active after it was idle are each a connect: the settle
/// window starts again.
pub fn heartbeat(app: &App, body: &Value) -> Result<Value> {
    heartbeat_for(app, body, &["prs"])?;
    Ok(health(app))
}

/// A heartbeat for each of `sources`, as [`heartbeat`] takes it.
pub fn heartbeat_for(app: &App, body: &Value, sources: &[&str]) -> Result<()> {
    let active = body.get("active").or_else(|| body.get("activeNow")).filter(|v| !v.is_null()).map(|v| as_bool(Some(v), true));
    let idle_until = when_of(body, "idle_until")?;
    let connected_at = when_of(body, "connected_at")?;
    for name in sources {
        let src = source(app, name);
        let mut st = state(app, &src);
        beat(&mut st, active, idle_until.as_deref(), connected_at.as_deref());
        save(app, &src, &st)?;
    }
    Ok(())
}

/// Applies one heartbeat to a feed's state (a source's, or one relay's).
fn beat(st: &mut Row, active: Option<bool>, idle_until: Option<&str>, connected_at: Option<&str>) {
    let now = now_iso();
    let mut connect = !st.contains_key("last_heartbeat_at");
    if let Some(c) = connected_at.and_then(parse_iso) {
        connect |= ts(st, "connected_at").is_none_or(|old| c > old);
    }
    if let Some(a) = active {
        connect |= a && st.get("active").and_then(|v| v.as_bool()) == Some(false);
        st.insert("active".into(), json!(a));
        match idle_until {
            Some(u) if !a => st.insert("idle_until".into(), json!(u)),
            _ => st.remove("idle_until"),
        };
    } else if let Some(u) = idle_until {
        st.insert("idle_until".into(), json!(u));
    }
    if connect {
        st.insert("connected_at".into(), json!(connected_at.map(str::to_string).unwrap_or_else(|| now.clone())));
    }
    st.insert("last_heartbeat_at".into(), json!(now));
    st.insert("heartbeats".into(), json!(st.i0("heartbeats") + 1));
}

/// A stream event was accepted from `relay` (its `source`): kept under each source it counts for, a
/// heartbeat (with what it `covers`) under the sources it names, any other event as that relay's last.
pub fn note_relay(app: &App, relay: &str, kind: &str, ev: &Value, sources: &[&str]) -> Result<()> {
    if relay.is_empty() {
        return Ok(());
    }
    let active = ev.get("active").filter(|v| !v.is_null()).map(|v| as_bool(Some(v), true));
    let idle_until = when_of(ev, "idle_until")?;
    let connected_at = when_of(ev, "connected_at")?;
    let now = now_iso();
    for name in sources {
        let src = source(app, name);
        let mut st = state(app, &src);
        let mut relays = st.get("relays").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        let mut r = relays.get(relay).and_then(|v| v.as_object()).cloned().unwrap_or_default();
        if !r.contains_key("since") {
            r.insert("since".into(), json!(now));
        }
        if kind == "heartbeat" {
            // Back after it was stuck: the PRs and watched branches it covers are read once more
            // (`pr_poll_skips`, `branch_covered_since`).
            if src.stuck_secs > 0.0 && ts(&r, "last_heartbeat_at").is_some_and(|t| now_ts() - t > src.stuck_secs) {
                r.insert("back_at".into(), json!(now));
            }
            beat(&mut r, active, idle_until.as_deref(), connected_at.as_deref());
            match ev.get("covers").filter(|v| v.is_array()) {
                Some(c) => r.insert("covers".into(), c.clone()),
                None => r.remove("covers"),
            };
        } else {
            r.insert("last_event_at".into(), json!(now));
            r.insert("last_event".into(), json!({"kind": kind, "at": now, "id": ev.get("id")}));
        }
        relays.insert(relay.to_string(), Value::Object(r));
        st.insert("relays".into(), Value::Object(relays));
        save(app, &src, &st)?;
    }
    Ok(())
}

/// The board started its own `listener`: a connect, so the settle window starts again.
fn note_connect(app: &App) {
    let src = prs(app);
    let mut st = state(app, &src);
    st.insert("connected_at".into(), json!(now_iso()));
    let _ = save(app, &src, &st);
}

/// The task a PR event is about: `task`, else the PR's link (`url`), else `repo` and `num` (and `host`).
/// The repo matches a board task's by its short name in any case (`webapp` or `ACME/webapp` for
/// `acme/webapp`), as the Python board matched them, among the board's PRs with that number on that
/// host (`prbuilds::pick_repo`): two orgs' repos of one name don't clash.
pub fn task_of_event(app: &App, body: &Value) -> Result<Option<Row>> {
    if let Some(v) = body.get("task").filter(|v| !v.is_null() && v.as_str() != Some("")) {
        return Ok(Some(board::get_task(app, need_ref(v, "task")?)?));
    }
    let url = body_str(body, "url");
    let (host, repo, num) = match find_pr(&url) {
        Some(l) => (l.host, l.repo, l.num),
        None => (body_str(body, "host"), body_str(body, "repo"), body["num"].as_i64().or_else(|| body_str(body, "num").parse().ok()).unwrap_or(0)),
    };
    if !repo.is_empty() && num > 0 {
        let tasks = app.db.q(
            "SELECT * FROM tasks WHERE pr_num = ? AND pr_repo IS NOT NULL AND (? = '' OR pr_host IS NULL OR pr_host = '' OR LOWER(pr_host) = LOWER(?)) ORDER BY id DESC",
            p![num, host, host],
        )?;
        let Some(r) = crate::prbuilds::pick_repo(tasks.iter().filter_map(|t| t.s("pr_repo")), &repo) else { return Ok(None) };
        return Ok(tasks.into_iter().find(|t| t.s("pr_repo").is_some_and(|x| x.eq_ignore_ascii_case(&r))));
    }
    if !url.is_empty() {
        return app.db.q1("SELECT * FROM tasks WHERE pr_url = ? ORDER BY id DESC LIMIT 1", p![url.trim_end_matches('/')]);
    }
    Ok(None)
}

/// One event came from the PR feed: it's alive (for the silent check), and `last_event` says what it was.
pub fn note_event(app: &App, kind: &str, body: &Value, task: Option<i64>) -> Result<()> {
    note_event_for(app, "prs", kind, body, task)
}

/// One event came from a source's feed.
pub fn note_event_for(app: &App, name: &str, kind: &str, body: &Value, task: Option<i64>) -> Result<()> {
    let src = source(app, name);
    let mut st = state(app, &src);
    let now = now_iso();
    st.insert("last_event_at".into(), json!(now));
    st.insert(
        "last_event".into(),
        json!({"kind": kind, "at": now, "task": task.map(|t| rf("task", t)), "url": body.get("url"), "state": body.get("state"),
               "source": body.get("source")}),
    );
    save(app, &src, &st)
}

/// `POST /pr-builds/event {repo, branch, pr?}` (the old board's listener): a push to, or a new or
/// updated PR on, one of the owner's branches. Taken as a build event of the owner's (`kind: build`,
/// `num: pr`, `mine: true`, `state: started`, `head: sha`), so while PR builds are stopped its builds
/// are cancelled, with the follow-up sweeps.
pub fn old_push(app: &App, body: &Value) -> Result<Value> {
    let mut ev = body.as_object().cloned().unwrap_or_default();
    ev.insert("kind".into(), json!("build"));
    ev.insert("mine".into(), json!(true));
    for (to, from) in [("num", "pr"), ("head", "sha")] {
        if ev.get(to).is_none_or(|v| v.is_null()) {
            if let Some(v) = body.get(from).filter(|v| !v.is_null()) {
                ev.insert(to.into(), v.clone());
            }
        }
    }
    if body_str(body, "state").is_empty() {
        ev.insert("state".into(), json!("started"));
    }
    if body_str(body, "source").is_empty() {
        ev.insert("source".into(), json!("pr-builds"));
    }
    intake(app, &Value::Object(ev))
}

/// `POST /prs/event` (`tb feed event`): one PR changed (`kind: pr`, the default) or one of its builds
/// did (`kind: build`, with `state`). The board notes it for the PR feed's health (a build for the
/// builds source's too) and reads that PR again.
pub fn intake(app: &App, body: &Value) -> Result<Value> {
    let kind = body_str(body, "kind").to_lowercase();
    if kind == "heartbeat" {
        return heartbeat(app, body);
    }
    let sources: &[&str] = if kind == "build" { &["prs", "builds"] } else { &["prs"] };
    intake_from(app, body, sources, None)
}

/// A PR or build event for `sources`' health. `pr`: a stream pr event to apply as the PR's state
/// without reading the host ([`prflow::apply_event`]); without it, or when it can't be applied, the
/// board reads the PR again.
pub fn intake_from(app: &App, body: &Value, sources: &[&str], pr: Option<&Value>) -> Result<Value> {
    let kind = { let k = body_str(body, "kind"); if k.is_empty() { "pr".to_string() } else { k.to_lowercase() } };
    if !matches!(kind.as_str(), "pr" | "build") {
        return err(400, "An event's kind is pr, build or heartbeat.");
    }
    let mut t = task_of_event(app, body)?;
    if t.is_none() && kind == "pr" {
        // The owner's PR opened outside the board, on a task's branch.
        if let Some(id) = prflow::link_by_branch(app, body)? {
            t = Some(board::get_task(app, id)?);
        }
    }
    for name in sources {
        note_event_for(app, name, &kind, body, t.as_ref().map(|t| t.id()))?;
    }
    let mut out = json!({"ok": true, "kind": kind, "task": t.as_ref().map(|t| rf("task", t.id()))});
    let Some(t) = t else {
        if kind == "build" {
            out["builds"] = crate::prbuilds::on_push_build(app, body)?;
        } else {
            out["owner_pr"] = crate::prbuilds::on_pr_event(app, body)?;
        }
        return Ok(out);
    };
    if kind == "build" {
        out["builds"] = crate::prbuilds::on_build_event(app, &t, body)?;
    }
    read_pr(app, &t, pr, &mut out)?;
    Ok(out)
}

/// Brings a task's open, watched PR up to date after an event about it: applies a complete stream pr
/// event when there is one, else reads the PR from its host. Says how in `out` (`applied`, `refreshed`,
/// `phase` or `read_error`).
pub fn read_pr(app: &App, t: &Row, pr: Option<&Value>, out: &mut Value) -> Result<()> {
    if crate::prhost::watched(t.s("pr_host")) && t.i("pr_num").is_some() && !matches!(t.s("pr_phase"), Some("merged" | "declined")) {
        crate::prpace::stirred(app, t.id())?;
        let applied = match pr {
            Some(ev) => prflow::apply_event(app, t.id(), ev)?,
            None => None,
        };
        out["applied"] = json!(applied.is_some());
        let r = match applied {
            Some(rec) => Ok(rec),
            None => prflow::refresh_task(app, t.id())?,
        };
        match r {
            Ok(_) => {
                out["refreshed"] = json!(true);
                out["phase"] = board::get_task(app, t.id())?.v("pr_phase");
            }
            Err(e) => {
                out["refreshed"] = json!(false);
                out["read_error"] = json!(e);
            }
        }
    } else {
        out["refreshed"] = json!(false);
    }
    Ok(())
}

/// Runs on every runner tick: for each watched source, notices its feed going bad or coming back,
/// restarts it on schedule and tells the owner when restarting doesn't help.
pub fn check(app: &App) -> Result<()> {
    for name in crate::stream::SOURCES {
        let src = source(app, name);
        if src.on {
            check_one(app, &src)?;
        }
    }
    Ok(())
}

fn check_one(app: &App, src: &Source) -> Result<()> {
    let mut st = state(app, src);
    if !st.contains_key("since") {
        st.insert("since".into(), json!(now_iso()));
        save(app, src, &st)?;
    }
    let Some((kind, why)) = problem_of(app, src) else {
        if st.contains_key("unhealthy_since") {
            for k in ["unhealthy_since", "problem", "restarts", "alerted_at"] {
                st.remove(k);
            }
            st.insert("healthy_at".into(), json!(now_iso()));
            save(app, src, &st)?;
            app.info(format!("feed: {} is healthy again", src.label));
            dispatch::clear_alert_key(app, &src.alert)?;
        }
        return Ok(());
    };
    let Some(since) = ts(&st, "unhealthy_since") else {
        st.insert("unhealthy_since".into(), json!(now_iso()));
        st.insert("problem".into(), json!(kind));
        save(app, src, &st)?;
        app.info(format!("feed: {why}"));
        return Ok(());
    };
    let bad_for = now_ts() - since;
    let mut restarts = st.get("restarts").and_then(|r| r.as_array()).cloned().unwrap_or_default();
    let plan = &src.restart_mins;
    let n = restarts.len();
    let label = capitalized(&src.label);
    let meanwhile = if src.name == "prs" { "Reviewer asks, nudges and swaps wait until it's back." } else { "The board polls instead until it's back." };
    if n < plan.len() && bad_for >= plan[n] * 60.0 {
        let r = restart(src);
        app.info(format!("feed: {} restart {} of {}: {}", src.name, n + 1, plan.len(), r.as_ref().err().map(|e| e.as_str()).unwrap_or("done")));
        restarts.push(json!({"at": now_iso(), "ok": r.is_ok(), "error": r.as_ref().err()}));
        st.insert("restarts".into(), Value::Array(restarts));
        save(app, src, &st)?;
        if let Err(e) = r {
            raise(app, src, &mut st, &format!("{label} is {kind}: {why}. Restarting it failed: {e}. {meanwhile}"))?;
        }
        return Ok(());
    }
    let last = plan.last().copied().unwrap_or(0.0);
    if n >= plan.len() && !st.contains_key("alerted_at") && bad_for >= last * 60.0 + ALERT_AFTER_LAST_SECS {
        raise(app, src, &mut st, &format!("{label} is still {kind} after {}: {why}. {meanwhile}", plural(n as i64, "restart")))?;
    }
    Ok(())
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn raise(app: &App, src: &Source, st: &mut Row, text: &str) -> Result<()> {
    if st.contains_key("alerted_at") {
        return Ok(());
    }
    dispatch::raise(app, text, None, None, Some(&src.alert), false)?;
    st.insert("alerted_at".into(), json!(now_iso()));
    save(app, src, st)
}

/// Restarts a source's feed: its `restart_cmd`, or (prs) the board's own listener.
fn restart(src: &Source) -> std::result::Result<(), String> {
    let cmd = src.restart_cmd.trim();
    if !cmd.is_empty() {
        let out = crate::proc::run(std::path::Path::new("/bin/sh"), &["-c".into(), cmd.to_string()], None, RESTART_TIMEOUT_SECS)
            .map_err(|_| format!("restart_cmd didn't finish in {RESTART_TIMEOUT_SECS} seconds"))?;
        if out.code != Some(0) {
            let why = out.stderr.lines().chain(out.stdout.lines()).find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
            return Err(if why.is_empty() { format!("restart_cmd exited {}", out.code.unwrap_or(-1)) } else { format!("restart_cmd: {why}") });
        }
        return Ok(());
    }
    if !src.listener.trim().is_empty() {
        stop_listener();
        return Ok(());
    }
    if src.name == "prs" {
        return Err("there's no restart_cmd (or listener) in [feed] to restart it with".into());
    }
    Err(format!("there's no restart_cmd in [stream.sources.{}] to restart it with", src.name))
}

static LISTENER: Mutex<Option<Child>> = Mutex::new(None);

fn listener_running() -> bool {
    LISTENER.lock().as_mut().map(|c| matches!(c.try_wait(), Ok(None))).unwrap_or(false)
}

fn stop_listener() {
    if let Some(mut c) = LISTENER.lock().take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// Keeps `[feed] listener` running (a thread, with the runner): starts it, and starts it again when
/// it exits or a restart stopped it.
pub fn listener_loop(app: Arc<App>) {
    let cmd = app.cfg.feed.listener.trim().to_string();
    if cmd.is_empty() {
        return;
    }
    let mut fails = 0u32;
    while !app.stopping() {
        if !listener_running() {
            if LISTENER.lock().take().is_some() {
                app.info("feed: the listener exited; starting it again");
            }
            let base = app.cfg.url();
            let spawned = Command::new("/bin/sh")
                .args(["-c", &cmd])
                .env("TASKBOARD_URL", &base)
                .env("TB_EVENT_URL", format!("{base}/tasks/api/prs/event"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            match spawned {
                Ok(c) => {
                    *LISTENER.lock() = Some(c);
                    fails = 0;
                    if prs(&app).on {
                        note_connect(&app);
                    }
                }
                Err(e) => {
                    fails += 1;
                    app.info(format!("feed: couldn't start the listener: {e}"));
                }
            }
        }
        app.sleep(if fails > 0 { (5.0 * fails as f64).min(60.0) } else { 5.0 });
    }
    stop_listener();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_read_like_a_person() {
        assert_eq!(span(30.0), "30 seconds");
        assert_eq!(span(240.0), "4 minutes");
        assert_eq!(span(3.0 * 3600.0), "3 hours");
    }
}
