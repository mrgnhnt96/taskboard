//! Claude's rate limits, from Midna's `usage.get` or the newest status-line file.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;

const WINDOWS: &[(&str, &str)] = &[("five_hour", "5h"), ("seven_day", "7d")];
const OUT_PCT: f64 = 100.0;
const SCAN_LIMIT: usize = 12;
/// The reset the board last saw the usage held until (`out_until`).
const OUT_KEY: &str = "usage_out_until";

/// The rate limits in the newest status-line files, each with when it was written.
fn status_lines(app: &App) -> Vec<(Row, f64)> {
    let Ok(dir) = std::fs::read_dir(&app.cfg.statusline_dir) else { return vec![] };
    let mut files: Vec<(std::path::PathBuf, f64)> = dir
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .filter_map(|p| {
            let m = p.metadata().ok()?.modified().ok()?;
            let secs = m.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64();
            Some((p, secs))
        })
        .collect();
    files.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = vec![];
    for (p, seen) in files.into_iter().take(SCAN_LIMIT) {
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let Ok(Value::Object(d)) = serde_json::from_str::<Value>(&text) else { continue };
        if let Some(Value::Object(l)) = d.get("rate_limits") {
            if !l.is_empty() {
                out.push((l.clone(), seen));
            }
        }
    }
    out
}

fn midna_limits(app: &App) -> Option<(Row, f64)> {
    let u = app.shared.lock().midna_usage.clone()?;
    let mut limits = Row::new();
    for (key, _) in WINDOWS {
        if let Some(w) = u.get(*key).and_then(|w| w.as_object()) {
            let resets = w.get("resets_at").and_then(|v| v.as_str()).and_then(parse_iso);
            limits.insert(key.to_string(), json!({"used_percentage": w.get("used_percentage"), "resets_at": resets}));
        }
    }
    if limits.is_empty() {
        return None;
    }
    let seen = u.get("observed_at").and_then(|v| v.as_str()).and_then(parse_iso).unwrap_or(0.0);
    Some((limits, seen))
}

/// Two reports' `resets_at` this close are the same window; a window's next one resets at least 5h later.
const SAME_WINDOW_SECS: f64 = 3600.0;

/// Whether report `w` (written at `seen`) beats `k`, the window's best report so far. A status-line file is
/// rewritten whenever its session redraws, idle or not, so its time says nothing about how fresh its limits
/// are. Usage only grows within a window, so the later window wins, then the higher percentage in it.
fn beats(w: &Row, seen: f64, k: &Value) -> bool {
    let pct = |v: &Value| v["used_percentage"].as_f64().unwrap_or(0.0);
    let resets = |v: &Value| v["resets_at"].as_f64();
    let w = Value::Object(w.clone());
    match (resets(&w), resets(k)) {
        (Some(a), Some(b)) if (a - b).abs() < SAME_WINDOW_SECS => pct(&w) > pct(k),
        (Some(a), Some(b)) => a > b,
        _ => k["seen"].as_f64().map(|s| s < seen).unwrap_or(true),
    }
}

/// Folds each source's windows into `known` (window key -> its limits and `seen`), keeping each window's
/// best report (`beats`), so a report that leaves a window out doesn't drop it.
fn fold(known: &mut Row, sources: Vec<(Row, f64)>) {
    for (l, seen) in sources {
        for (key, _) in WINDOWS {
            let Some(Value::Object(w)) = l.get(*key) else { continue };
            if w.get("used_percentage").and_then(|v| v.as_f64()).is_none() {
                continue;
            }
            match known.get_mut(*key) {
                Some(k) if !beats(w, seen, k) => {
                    if k["seen"].as_f64().map(|s| s < seen).unwrap_or(true) {
                        k["seen"] = json!(seen);
                    }
                }
                _ => {
                    let mut w = w.clone();
                    w.insert("seen".into(), json!(seen));
                    known.insert(key.to_string(), Value::Object(w));
                }
            }
        }
    }
}

fn sources(app: &App) -> Vec<(Row, f64)> {
    let mut all = status_lines(app);
    all.extend(midna_limits(app));
    all
}

fn newest(known: &Row) -> f64 {
    known.values().filter_map(|w| w["seen"].as_f64()).fold(0.0, f64::max)
}

/// The windows the current reports hold.
fn limits(app: &App) -> Option<(Row, f64)> {
    let mut known = Row::new();
    fold(&mut known, sources(app));
    (!known.is_empty()).then(|| {
        let seen = newest(&known);
        (known, seen)
    })
}

/// Every window's last known limits, kept between reports so the meters hold their last value.
fn remembered(app: &App) -> Option<(Row, f64)> {
    let fresh = sources(app);
    let mut sh = app.shared.lock();
    fold(&mut sh.usage_known, fresh);
    (!sh.usage_known.is_empty()).then(|| (sh.usage_known.clone(), newest(&sh.usage_known)))
}

fn window(raw: Option<&Value>, now: f64) -> Option<Value> {
    let raw = raw?.as_object()?;
    let pct = raw.get("used_percentage")?.as_f64()?;
    let resets = raw.get("resets_at").and_then(|v| v.as_f64());
    let rolled = resets.map(|r| r <= now).unwrap_or(false);
    Some(json!({
        "pct": if rolled { 0 } else { pct.round() as i64 },
        "resets_at": match resets { Some(r) if !rolled => json!(iso(r)), _ => Value::Null },
    }))
}

pub fn state(app: &App) -> Value {
    let Some((l, seen)) = remembered(app) else { return Value::Null };
    let now = now_ts();
    let windows: Vec<Value> = WINDOWS
        .iter()
        .filter_map(|(key, label)| {
            window(l.get(*key), now).map(|w| json!({"key": key, "label": label, "pct": w["pct"], "resets_at": w["resets_at"]}))
        })
        .collect();
    if windows.is_empty() {
        Value::Null
    } else {
        json!({"windows": windows, "seen_at": iso(seen)})
    }
}

/// When the usage is used up, the time it's back: the latest reset of the windows at their limit
/// (5-hour or 7-day). It holds until that reset unless the window that ran out is reported in a new
/// period, so a report that leaves the window out, or an older one from before it ran out, can't let
/// queued work start, get closed and start again.
pub fn out_until(app: &App) -> Option<String> {
    let l = limits(app).map(|(l, _)| l).unwrap_or_default();
    let held = app.db.get_setting(OUT_KEY).ok().flatten();
    let out = held_out(&l, held.as_deref(), now_ts());
    if out != held {
        let _ = app.db.set_setting(OUT_KEY, out.as_deref());
    }
    out.and_then(|o| o.split_once(' ').map(|(_, r)| r.to_string()))
}

/// `out_until` as "<window> <reset>", from the reports' windows and the one it held before.
fn held_out(l: &Row, held: Option<&str>, now: f64) -> Option<String> {
    let mut out: Option<(&str, String)> = None;
    for (key, _) in WINDOWS {
        let Some(w) = window(l.get(*key), now) else { continue };
        let Some(r) = w["resets_at"].as_str() else { continue };
        if w["pct"].as_f64().unwrap_or(0.0) >= OUT_PCT && out.as_ref().is_none_or(|(_, o)| r > o.as_str()) {
            out = Some((key, r.to_string()));
        }
    }
    if let Some((key, r)) = out {
        return Some(format!("{key} {r}"));
    }
    let (key, r) = held?.split_once(' ')?;
    let still = parse_iso(r).is_some_and(|t| t > now) && window(l.get(key), now).is_none_or(|w| w["resets_at"].as_str() == Some(r));
    still.then(|| format!("{key} {r}"))
}

#[cfg(test)]
mod out_tests {
    use super::*;

    const NOW: f64 = 1_800_000_000.0;

    fn report(v: Value) -> Row {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn a_report_without_the_window_or_from_before_it_ran_out_keeps_the_usage_out() {
        let out = held_out(&report(json!({"five_hour": {"used_percentage": 100, "resets_at": NOW + 3600.0}})), None, NOW).unwrap();
        assert_eq!(out, format!("five_hour {}", iso(NOW + 3600.0)));
        assert_eq!(held_out(&Row::new(), Some(&out), NOW).as_deref(), Some(out.as_str()), "no report");
        let partial = report(json!({"seven_day": {"used_percentage": 40, "resets_at": NOW + 86400.0}}));
        assert_eq!(held_out(&partial, Some(&out), NOW).as_deref(), Some(out.as_str()), "the window left out");
        let older = report(json!({"five_hour": {"used_percentage": 97, "resets_at": NOW + 3600.0}}));
        assert_eq!(held_out(&older, Some(&out), NOW).as_deref(), Some(out.as_str()), "the same period, from before it ran out");
    }

    #[test]
    fn the_usage_is_back_at_its_reset_or_with_the_window_in_a_new_period() {
        let out = format!("five_hour {}", iso(NOW + 3600.0));
        assert_eq!(held_out(&Row::new(), Some(&out), NOW + 3601.0), None, "past the reset");
        let fresh = report(json!({"five_hour": {"used_percentage": 2, "resets_at": NOW + 3600.0 * 5.0}}));
        assert_eq!(held_out(&fresh, Some(&out), NOW), None, "a new period");
    }

    #[test]
    fn the_weekly_window_counts_and_the_later_reset_wins() {
        let l = report(json!({
            "five_hour": {"used_percentage": 100, "resets_at": NOW + 3600.0},
            "seven_day": {"used_percentage": 100, "resets_at": NOW + 86400.0 * 2.0},
        }));
        assert_eq!(held_out(&l, None, NOW), Some(format!("seven_day {}", iso(NOW + 86400.0 * 2.0))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(v: Value) -> Row {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn a_report_without_a_window_keeps_its_last_known_value() {
        let mut known = Row::new();
        fold(&mut known, vec![(report(json!({"five_hour": {"used_percentage": 40.0}, "seven_day": {"used_percentage": 66.0}})), 10.0)]);
        fold(&mut known, vec![(report(json!({"five_hour": {"used_percentage": 43.0}})), 20.0)]);
        assert_eq!(known["five_hour"]["used_percentage"], 43.0);
        assert_eq!(known["seven_day"]["used_percentage"], 66.0, "7d holds while reports leave it out");
        assert_eq!(newest(&known), 20.0);
    }

    #[test]
    fn the_freshest_report_of_each_window_wins() {
        let mut known = Row::new();
        fold(
            &mut known,
            vec![
                (report(json!({"seven_day": {"used_percentage": 70.0}})), 30.0),
                (report(json!({"seven_day": {"used_percentage": 60.0}, "five_hour": {"used_percentage": 5.0}})), 25.0),
            ],
        );
        assert_eq!(known["seven_day"]["used_percentage"], 70.0);
        assert_eq!(known["five_hour"]["used_percentage"], 5.0);
    }

    #[test]
    fn an_idle_sessions_stale_limits_lose_to_higher_usage_in_the_same_window() {
        let mut known = Row::new();
        let live = json!({"five_hour": {"used_percentage": 100.0, "resets_at": 5000.0}, "seven_day": {"used_percentage": 82.0, "resets_at": 90000.0}});
        let idle = json!({"five_hour": {"used_percentage": 46.0, "resets_at": 5001.0}, "seven_day": {"used_percentage": 51.0, "resets_at": 90000.0}});
        fold(&mut known, vec![(report(live), 10.0), (report(idle), 20.0)]);
        assert_eq!(known["five_hour"]["used_percentage"], 100.0);
        assert_eq!(known["seven_day"]["used_percentage"], 82.0);
        assert_eq!(newest(&known), 20.0, "the redraw still counts as seen");
    }

    #[test]
    fn a_new_window_replaces_the_old_one_even_with_lower_usage() {
        let mut known = Row::new();
        fold(&mut known, vec![(report(json!({"five_hour": {"used_percentage": 100.0, "resets_at": 5000.0}})), 10.0)]);
        fold(&mut known, vec![(report(json!({"five_hour": {"used_percentage": 1.0, "resets_at": 23000.0}})), 9.0)]);
        assert_eq!(known["five_hour"]["used_percentage"], 1.0);
    }
}
