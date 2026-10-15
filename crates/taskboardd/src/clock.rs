//! When the Mac slept, so the board's waits and timeouts count only the time it was awake.
//!
//! Timestamps on the board are wall-clock times, and a closed lid doesn't stop the wall clock: after a
//! night asleep every job looks hours late, every alert overdue and every terminal idle for hours.
//! `Instant` does stop while the Mac sleeps, so a gap between how far the two clocks moved is time
//! spent asleep. The watcher notes each gap, and `awake_since` leaves them out.
//!
//! The owner can be away with the Mac awake (keep-awake holds it up for agents at night), so the
//! watcher also notes each stretch with no keyboard or mouse input for `AWAY_SECS`: `away_spans`
//! gives those with the sleeps, for the Days page's waits on the owner.

use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::json;

use crate::app::App;
use crate::util::now_ts;

/// A gap shorter than this is a slow thread or a clock nudge, not sleep.
const MIN_SLEEP_SECS: f64 = 15.0;
/// How long a sleep is remembered: longer than any wait that measures with it.
const KEEP_SECS: f64 = 14.0 * 86400.0;
const SETTING: &str = "sleeps";
/// No input for this long and the owner is away, from their last input.
const AWAY_SECS: f64 = 15.0 * 60.0;
const AWAY_SETTING: &str = "aways";

static SLEEPS: Lazy<Mutex<Vec<(f64, f64)>>> = Lazy::new(|| Mutex::new(Vec::new()));
static AWAYS: Lazy<Mutex<Vec<(f64, f64)>>> = Lazy::new(|| Mutex::new(Vec::new()));
/// When the owner's current stretch away began, while it goes on.
static AWAY_NOW: Lazy<Mutex<Option<f64>>> = Lazy::new(|| Mutex::new(None));

/// Seconds the Mac slept between `from` and `to` (board timestamps).
pub fn slept_between(from: f64, to: f64) -> f64 {
    SLEEPS.lock().iter().map(|&(a, b)| (b.min(to) - a.max(from)).max(0.0)).sum()
}

/// Seconds since `ts` that the Mac was awake.
pub fn awake_since(ts: f64) -> f64 {
    let now = now_ts();
    now - ts - slept_between(ts, now)
}

/// Remembers that the Mac slept from `from` to `to`, forgetting sleeps older than `KEEP_SECS`.
pub fn note_sleep(from: f64, to: f64) {
    let mut s = SLEEPS.lock();
    s.push((from, to));
    s.retain(|&(_, b)| b >= to - KEEP_SECS);
}

pub fn forget_sleeps() {
    SLEEPS.lock().clear();
}

/// Remembers that the owner was away (no input) from `from` to `to`.
pub fn note_away(from: f64, to: f64) {
    let mut s = AWAYS.lock();
    s.push((from, to));
    s.retain(|&(_, b)| b >= to - KEEP_SECS);
}

/// Marks the owner away since `from` until further notice, or back (`None`).
pub fn set_away_now(from: Option<f64>) {
    *AWAY_NOW.lock() = from;
}

pub fn forget_aways() {
    AWAYS.lock().clear();
    *AWAY_NOW.lock() = None;
}

/// The stretches in `[from, to)` the owner was away or the Mac slept, clipped, merged and in order.
pub fn away_spans(from: f64, to: f64) -> Vec<(f64, f64)> {
    let mut all: Vec<(f64, f64)> = SLEEPS.lock().iter().chain(AWAYS.lock().iter()).copied().collect();
    if let Some(a) = *AWAY_NOW.lock() {
        all.push((a, f64::INFINITY));
    }
    merge(all.into_iter().map(|(a, b)| (a.max(from), b.min(to))).filter(|(a, b)| b > a).collect())
}

/// Overlapping or touching stretches as one, in order.
pub fn merge(mut v: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (a, b) in v {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

fn load_log(app: &App, key: &str) -> Vec<(f64, f64)> {
    let saved = app.db.get_setting(key).ok().flatten().unwrap_or_default();
    serde_json::from_str(&saved).unwrap_or_default()
}

fn load(app: &App) {
    for (a, b) in load_log(app, SETTING) {
        note_sleep(a, b);
    }
    for (a, b) in load_log(app, AWAY_SETTING) {
        note_away(a, b);
    }
}

fn save(app: &App) {
    let list = json!(*SLEEPS.lock()).to_string();
    if let Err(e) = app.db.set_setting(SETTING, Some(&list)) {
        app.info(format!("clock: couldn't save the sleep log: {e}"));
    }
}

fn save_aways(app: &App) {
    let list = json!(*AWAYS.lock()).to_string();
    if let Err(e) = app.db.set_setting(AWAY_SETTING, Some(&list)) {
        app.info(format!("clock: couldn't save the away log: {e}"));
    }
}

/// Seconds since the owner's last keyboard or mouse input.
#[cfg(target_os = "macos")]
fn idle_secs() -> Option<f64> {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state: i32, kind: u32) -> f64;
    }
    // kCGEventSourceStateCombinedSessionState, kCGAnyInputEventType.
    let s = unsafe { CGEventSourceSecondsSinceLastEventType(0, u32::MAX) };
    (s.is_finite() && s >= 0.0).then_some(s)
}

#[cfg(not(target_os = "macos"))]
fn idle_secs() -> Option<f64> {
    None
}

/// Notes the start and end of each stretch away from the idle time now.
fn check_away(app: &App) {
    let Some(idle) = idle_secs() else { return };
    let now = now_ts();
    let away = *AWAY_NOW.lock();
    match away {
        None if idle >= AWAY_SECS => set_away_now(Some(now - idle)),
        Some(from) if idle < AWAY_SECS => {
            set_away_now(None);
            note_away(from, now - idle);
            save_aways(app);
        }
        _ => {}
    }
}

/// Watches for sleep until the board stops.
pub fn watch(app: Arc<App>) {
    load(&app);
    // Real time, not `now_ts`: the tests move the board's clock forward, and that isn't sleep.
    let real = || Utc::now().timestamp_millis() as f64 / 1000.0;
    while !app.stopping() {
        let (wall, mono) = (real(), Instant::now());
        app.sleep(2.0);
        let slept = (real() - wall) - mono.elapsed().as_secs_f64();
        if slept >= MIN_SLEEP_SECS {
            let now = now_ts();
            note_sleep(now - slept, now);
            save(&app);
            app.info(format!("clock: the Mac slept for {} minutes", (slept / 60.0).round() as i64));
            app.wake_runner();
        }
        check_away(&app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap() {
        forget_sleeps();
        note_sleep(100.0, 200.0);
        note_sleep(300.0, 350.0);
        assert_eq!(slept_between(0.0, 1000.0), 150.0);
        assert_eq!(slept_between(150.0, 320.0), 70.0);
        assert_eq!(slept_between(200.0, 300.0), 0.0);
        forget_sleeps();
    }

    #[test]
    fn merged_spans() {
        assert_eq!(merge(vec![(5.0, 8.0), (0.0, 2.0), (1.0, 3.0), (3.0, 4.0)]), vec![(0.0, 4.0), (5.0, 8.0)]);
    }
}
