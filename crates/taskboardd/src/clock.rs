//! When the Mac slept, so the board's waits and timeouts count only the time it was awake.
//!
//! Timestamps on the board are wall-clock times, and a closed lid doesn't stop the wall clock: after a
//! night asleep every job looks hours late, every alert overdue and every terminal idle for hours.
//! `Instant` does stop while the Mac sleeps, so a gap between how far the two clocks moved is time
//! spent asleep. The watcher notes each gap, and `awake_since` leaves them out.

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

static SLEEPS: Lazy<Mutex<Vec<(f64, f64)>>> = Lazy::new(|| Mutex::new(Vec::new()));

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

fn load(app: &App) {
    let saved = app.db.get_setting(SETTING).ok().flatten().unwrap_or_default();
    let list: Vec<(f64, f64)> = serde_json::from_str(&saved).unwrap_or_default();
    for (a, b) in list {
        note_sleep(a, b);
    }
}

fn save(app: &App) {
    let list = json!(*SLEEPS.lock()).to_string();
    if let Err(e) = app.db.set_setting(SETTING, Some(&list)) {
        app.info(format!("clock: couldn't save the sleep log: {e}"));
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
}
