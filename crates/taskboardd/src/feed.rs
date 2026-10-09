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
//! `silent_secs` (an hour) inside the work hours. Either one makes it unhealthy:
//!
//! - the board restarts it (`restart_cmd`, or the `listener` it runs itself) `restart_mins` after it
//!   went bad (1, 5 and 15 minutes), and raises the `pr-feed` alert when a restart fails or it's
//!   still bad five minutes after the last one; the alert clears once the feed is healthy again;
//! - [`feed_healthy`] answers false, and [`holding`] tells the reviewer ask, nudge and swap logic to
//!   wait; a stuck feed holds even a PR's first ask, at any hour (only a feed that's merely quiet
//!   outside the work hours, when that's expected, lets the first ask through);
//! - once it's healthy again, [`holding`] keeps holding for `settle_secs` (5 minutes) while the
//!   events it missed catch up;
//! - the PR poll (`[intervals] prs`) runs as a fallback; while the feed is healthy it only runs with
//!   `poll_while_healthy`.
//!
//! Without `[feed] on` the feed is never unhealthy and the poll runs as before; events still refresh
//! their PR.
//!
//! # State
//!
//! The setting `pr_feed` keeps `{since, last_event_at, last_event, last_heartbeat_at, heartbeats,
//! unhealthy_since, problem, restarts: [{at, ok, error?}], alerted_at, healthy_at}` (`healthy_at`: when it
//! last came back, which starts the settle window).

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
    /// No event or heartbeat for this long inside the work hours: it's silent.
    pub silent_secs: f64,
    /// Minutes after the feed went bad at which the board restarts it.
    pub restart_mins: Vec<f64>,
    /// Keep polling every PR (`[intervals] prs`) while the feed is healthy too.
    pub poll_while_healthy: bool,
    /// After the feed comes back, reviewer asks, nudges and swaps wait this many more seconds while the
    /// events it missed catch up.
    pub settle_secs: f64,
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
        }
    }
}

const KEY: &str = "pr_feed";
pub const ALERT_KEY: &str = "pr-feed";
/// After the last restart, how long the feed may stay bad before the owner is told.
const ALERT_AFTER_LAST_SECS: f64 = 300.0;
const RESTART_TIMEOUT_SECS: f64 = 60.0;

fn state(app: &App) -> Row {
    jloads_obj(app.db.get_setting(KEY).ok().flatten().as_deref())
}

fn save(app: &App, st: &Row) -> Result<()> {
    app.db.set_setting(KEY, Some(&jdumps(&Value::Object(st.clone()))))
}

fn ts(st: &Row, k: &str) -> Option<f64> {
    st.s(k).and_then(parse_iso)
}

/// What's wrong with the feed, if anything: (`stuck` or `silent`, a sentence).
pub fn problem(app: &App) -> Option<(&'static str, String)> {
    if !app.cfg.feed.on {
        return None;
    }
    let st = state(app);
    let now = now_ts();
    let c = &app.cfg.feed;
    if let Some(hb) = ts(&st, "last_heartbeat_at") {
        if c.stuck_secs > 0.0 && now - hb > c.stuck_secs {
            return Some(("stuck", format!("the PR feed hasn't sent a heartbeat for {}", span(now - hb))));
        }
    }
    let heard = [ts(&st, "last_event_at"), ts(&st, "last_heartbeat_at"), ts(&st, "since")].into_iter().flatten().fold(f64::MIN, f64::max);
    if c.silent_secs > 0.0 && heard > f64::MIN && now - heard > c.silent_secs && hours::is_open(app) {
        return Some(("silent", format!("nothing has come from the PR feed for {}", span(now - heard))));
    }
    None
}

fn span(secs: f64) -> String {
    let m = (secs / 60.0).round() as i64;
    if secs < 60.0 {
        plural(secs.round() as i64, "second")
    } else if m < 120 {
        plural(m, "minute")
    } else {
        plural(m / 60, "hour")
    }
}

/// The gate for the review loop: the feed is healthy (always, without `[feed] on`). Reviewer asks,
/// nudges and swaps check it through [`holding`].
pub fn feed_healthy(app: &App) -> bool {
    problem(app).is_none()
}

/// Seconds left of the settle window after the feed came back (`settle_secs`), if it's in one. A feed
/// that's healthy again before [`check`] has noticed is in it too.
pub fn settling(app: &App) -> Option<f64> {
    let c = &app.cfg.feed;
    if !c.on || c.settle_secs <= 0.0 || problem(app).is_some() {
        return None;
    }
    let st = state(app);
    if st.contains_key("unhealthy_since") {
        return Some(c.settle_secs);
    }
    let left = c.settle_secs - (now_ts() - ts(&st, "healthy_at")?);
    (left > 0.0).then_some(left)
}

/// Why the reviewer ask, nudge or swap logic should wait now, if it should: the feed is unhealthy, or
/// it came back less than `settle_secs` ago. `first_ask` is the first request for review on a PR: it
/// still goes out when the feed is merely quiet outside the work hours, but not when it's stuck (down
/// or stale) or settling.
pub fn holding(app: &App, first_ask: bool) -> Option<String> {
    if let Some((kind, why)) = problem(app) {
        if first_ask && kind == "silent" && !hours::is_open(app) {
            return None;
        }
        return Some(format!("Holding reviewer asks, nudges and swaps: {why}."));
    }
    let left = settling(app)?;
    Some(format!("Holding reviewer asks, nudges and swaps: the PR feed just came back, so its missed events catch up first ({} left).", span(left)))
}

/// Whether the PR poll runs this round: always without the feed, else while it's unhealthy (or with
/// `poll_while_healthy`).
pub fn poll_due(app: &App) -> bool {
    !app.cfg.feed.on || app.cfg.feed.poll_while_healthy || !feed_healthy(app)
}

/// `GET /prs/feed` (`tb feed`).
pub fn health(app: &App) -> Value {
    let st = state(app);
    let p = problem(app);
    json!({
        "on": app.cfg.feed.on,
        "healthy": p.is_none(),
        "problem": p.as_ref().map(|(k, _)| *k),
        "why": p.as_ref().map(|(_, w)| w.clone()),
        "last_event_at": st.v("last_event_at"),
        "last_event": st.v("last_event"),
        "last_heartbeat_at": st.v("last_heartbeat_at"),
        "unhealthy_since": st.v("unhealthy_since"),
        "restarts": st.get("restarts").cloned().unwrap_or(json!([])),
        "listener": if app.cfg.feed.listener.trim().is_empty() { Value::Null } else { json!(listener_running()) },
        "holding": holding(app, false),
        "settling_secs": settling(app).map(|s| s.round()),
    })
}

/// `POST /prs/heartbeat` (`tb feed heartbeat`): the feed is alive.
pub fn heartbeat(app: &App) -> Result<Value> {
    let mut st = state(app);
    st.insert("last_heartbeat_at".into(), json!(now_iso()));
    st.insert("heartbeats".into(), json!(st.i0("heartbeats") + 1));
    save(app, &st)?;
    Ok(health(app))
}

/// The task a PR event is about: `task`, else the PR's link (`url`), else `repo` and `num`.
pub fn task_of_event(app: &App, body: &Value) -> Result<Option<Row>> {
    if let Some(v) = body.get("task").filter(|v| !v.is_null() && v.as_str() != Some("")) {
        return Ok(Some(board::get_task(app, need_ref(v, "task")?)?));
    }
    let url = body_str(body, "url");
    let (repo, num) = match find_pr(&url) {
        Some(l) => (l.repo, l.num),
        None => (body_str(body, "repo"), body["num"].as_i64().or_else(|| body_str(body, "num").parse().ok()).unwrap_or(0)),
    };
    if !repo.is_empty() && num > 0 {
        return app.db.q1("SELECT * FROM tasks WHERE pr_repo = ? AND pr_num = ? ORDER BY id DESC LIMIT 1", p![repo, num]);
    }
    if !url.is_empty() {
        return app.db.q1("SELECT * FROM tasks WHERE pr_url = ? ORDER BY id DESC LIMIT 1", p![url.trim_end_matches('/')]);
    }
    Ok(None)
}

/// `POST /prs/event` (`tb feed event`): one PR changed (`kind: pr`, the default) or one of its builds
/// did (`kind: build`, with `state`). The board notes it for the feed's health and reads that PR again.
pub fn intake(app: &App, body: &Value) -> Result<Value> {
    let kind = { let k = body_str(body, "kind"); if k.is_empty() { "pr".to_string() } else { k.to_lowercase() } };
    if kind == "heartbeat" {
        return heartbeat(app);
    }
    if !matches!(kind.as_str(), "pr" | "build") {
        return err(400, "An event's kind is pr, build or heartbeat.");
    }
    let t = task_of_event(app, body)?;
    let mut st = state(app);
    let now = now_iso();
    st.insert("last_event_at".into(), json!(now));
    st.insert(
        "last_event".into(),
        json!({"kind": kind, "at": now, "task": t.as_ref().map(|t| rf("task", t.id())), "url": body.get("url"), "state": body.get("state"),
               "source": body.get("source")}),
    );
    save(app, &st)?;
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
    if crate::prhost::watched(t.s("pr_host")) && t.i("pr_num").is_some() && !matches!(t.s("pr_phase"), Some("merged" | "declined")) {
        match prflow::refresh_task(app, t.id())? {
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
    Ok(out)
}

/// Runs on every runner tick: notices the feed going bad or coming back, restarts it on schedule and
/// tells the owner when restarting doesn't help.
pub fn check(app: &App) -> Result<()> {
    if !app.cfg.feed.on {
        return Ok(());
    }
    let mut st = state(app);
    if !st.contains_key("since") {
        st.insert("since".into(), json!(now_iso()));
        save(app, &st)?;
    }
    let Some((kind, why)) = problem(app) else {
        if st.contains_key("unhealthy_since") {
            for k in ["unhealthy_since", "problem", "restarts", "alerted_at"] {
                st.remove(k);
            }
            st.insert("healthy_at".into(), json!(now_iso()));
            save(app, &st)?;
            app.info("feed: the PR feed is healthy again");
            dispatch::clear_alert_key(app, ALERT_KEY)?;
        }
        return Ok(());
    };
    let Some(since) = ts(&st, "unhealthy_since") else {
        st.insert("unhealthy_since".into(), json!(now_iso()));
        st.insert("problem".into(), json!(kind));
        save(app, &st)?;
        app.info(format!("feed: {why}"));
        return Ok(());
    };
    let bad_for = now_ts() - since;
    let mut restarts = st.get("restarts").and_then(|r| r.as_array()).cloned().unwrap_or_default();
    let plan = &app.cfg.feed.restart_mins;
    let n = restarts.len();
    if n < plan.len() && bad_for >= plan[n] * 60.0 {
        let r = restart(app);
        app.info(format!("feed: restart {} of {}: {}", n + 1, plan.len(), r.as_ref().err().map(|e| e.as_str()).unwrap_or("done")));
        restarts.push(json!({"at": now_iso(), "ok": r.is_ok(), "error": r.as_ref().err()}));
        st.insert("restarts".into(), Value::Array(restarts));
        save(app, &st)?;
        if let Err(e) = r {
            raise(app, &mut st, &format!("The PR feed is {kind}: {why}. Restarting it failed: {e}. Reviewer asks, nudges and swaps wait until it's back."))?;
        }
        return Ok(());
    }
    let last = plan.last().copied().unwrap_or(0.0);
    if n >= plan.len() && !st.contains_key("alerted_at") && bad_for >= last * 60.0 + ALERT_AFTER_LAST_SECS {
        raise(
            app,
            &mut st,
            &format!("The PR feed is still {kind} after {}: {why}. Reviewer asks, nudges and swaps wait until it's back.", plural(n as i64, "restart")),
        )?;
    }
    Ok(())
}

fn raise(app: &App, st: &mut Row, text: &str) -> Result<()> {
    if st.contains_key("alerted_at") {
        return Ok(());
    }
    dispatch::raise(app, text, None, None, Some(ALERT_KEY), false)?;
    st.insert("alerted_at".into(), json!(now_iso()));
    save(app, st)
}

/// Restarts the feed: `restart_cmd`, or the board's own listener.
fn restart(app: &App) -> std::result::Result<(), String> {
    let cmd = app.cfg.feed.restart_cmd.trim();
    if !cmd.is_empty() {
        let out = crate::proc::run(std::path::Path::new("/bin/sh"), &["-c".into(), cmd.to_string()], None, RESTART_TIMEOUT_SECS)
            .map_err(|_| format!("restart_cmd didn't finish in {RESTART_TIMEOUT_SECS} seconds"))?;
        if out.code != Some(0) {
            let why = out.stderr.lines().chain(out.stdout.lines()).find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
            return Err(if why.is_empty() { format!("restart_cmd exited {}", out.code.unwrap_or(-1)) } else { format!("restart_cmd: {why}") });
        }
        return Ok(());
    }
    if !app.cfg.feed.listener.trim().is_empty() {
        stop_listener();
        return Ok(());
    }
    Err("there's no restart_cmd (or listener) in [feed] to restart it with".into())
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
