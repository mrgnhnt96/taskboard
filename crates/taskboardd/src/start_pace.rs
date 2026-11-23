//! Starts the board makes on its own, spaced out. When the work hours open, the board's sweeps can put
//! many tasks in motion in one tick: Couldn't start and Terminal lost retries (`needs_retry`), done-when
//! misses given to agents (`misses`), human tasks handed to new agent tasks (`recheck`). Started together,
//! they open a burst of terminals at once. Each gets its own start time instead, [`GAP_SECS`] from every
//! other board start: it's off Needs you and waits its turn in the queue (`retry_at`, which the runner
//! waits for). The owner's Start still starts a task at once and clears its time.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields};

/// How far apart the board's own starts are.
pub const GAP_SECS: f64 = 180.0;
/// The settings key for the start times handed out, as board timestamps.
const STATE: &str = "board_starts";

/// The first time at or after `earliest` that's `gap` from every time in `taken`.
pub fn pick(taken: &[f64], earliest: f64, gap: f64) -> f64 {
    let mut sorted = taken.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mut at = earliest;
    for t in sorted {
        if t + gap <= at {
            continue;
        }
        if at + gap <= t {
            break;
        }
        at = t + gap;
    }
    at
}

/// Hands out the board start time for a start that may run from `earliest`, and keeps it so the next
/// one spaces from it. Call inside a transaction.
pub fn slot(app: &App, earliest: f64) -> Result<f64> {
    let now = now_ts();
    let mut taken: Vec<f64> = jloads_arr(app.db.get_setting(STATE)?.as_deref())
        .iter()
        .filter_map(Value::as_f64)
        .filter(|t| *t + GAP_SECS > now)
        .collect();
    let at = pick(&taken, earliest, GAP_SECS);
    taken.push(at);
    app.db.set_setting(STATE, Some(&jdumps(&json!(taken))))?;
    Ok(at)
}

/// Task `id`, just queued by the board, waits for its board start time when that isn't now. In a transaction.
pub fn hold(app: &App, id: i64) -> Result<()> {
    let now = now_ts();
    let at = slot(app, now)?;
    if at <= now + 1.0 {
        return Ok(());
    }
    let at = iso(at);
    let line = format!("The board starts it at {}, spaced from its other starts.", local_clock(Some(&at)));
    board::update_task(app, id, fields!["retry_at" => at, "latest" => line])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: f64 = 180.0;

    #[test]
    fn the_first_start_is_now() {
        assert_eq!(pick(&[], 1000.0, G), 1000.0);
        assert_eq!(pick(&[500.0], 1000.0, G), 1000.0, "the last one was long enough ago");
    }

    #[test]
    fn starts_together_are_spaced_apart() {
        let mut taken = vec![];
        for _ in 0..4 {
            let at = pick(&taken, 1000.0, G);
            taken.push(at);
        }
        assert_eq!(taken, vec![1000.0, 1180.0, 1360.0, 1540.0]);
    }

    #[test]
    fn a_start_fits_in_a_gap_before_a_later_one() {
        // A retry at 2000 doesn't push a start that can run now past it.
        assert_eq!(pick(&[2000.0], 1000.0, G), 1000.0);
        assert_eq!(pick(&[1100.0, 2000.0], 1000.0, G), 1280.0);
        assert_eq!(pick(&[1100.0, 1400.0], 1000.0, G), 1580.0, "no room between them");
    }

    #[test]
    fn a_later_start_spaces_from_the_ones_near_it() {
        assert_eq!(pick(&[1600.0], 1500.0, G), 1780.0);
        assert_eq!(pick(&[1000.0, 1180.0], 1600.0, G), 1600.0);
    }
}
