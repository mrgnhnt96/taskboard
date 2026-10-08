//! Claude's rate limits, from Midna's `usage.get` or the newest status-line file.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;

const WINDOWS: &[(&str, &str)] = &[("five_hour", "5h"), ("seven_day", "7d")];
const OUT_PCT: f64 = 100.0;
const SCAN_LIMIT: usize = 12;

fn newest_status_line(app: &App) -> Option<(Row, f64)> {
    let dir = std::fs::read_dir(&app.cfg.statusline_dir).ok()?;
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
    for (p, seen) in files.into_iter().take(SCAN_LIMIT) {
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let Ok(Value::Object(d)) = serde_json::from_str::<Value>(&text) else { continue };
        if let Some(Value::Object(l)) = d.get("rate_limits") {
            if !l.is_empty() {
                return Some((l.clone(), seen));
            }
        }
    }
    None
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

fn limits(app: &App) -> Option<(Row, f64)> {
    let a = midna_limits(app);
    let b = newest_status_line(app);
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.1 > a.1 { b } else { a }),
        (a, b) => a.or(b),
    }
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
    let Some((l, seen)) = limits(app) else { return Value::Null };
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

/// When the 5-hour window is used up, the time it resets.
pub fn out_until(app: &App) -> Option<String> {
    let (l, _) = limits(app)?;
    let w = window(l.get("five_hour"), now_ts())?;
    if w["pct"].as_f64().unwrap_or(0.0) >= OUT_PCT {
        w["resets_at"].as_str().map(|s| s.to_string())
    } else {
        None
    }
}
