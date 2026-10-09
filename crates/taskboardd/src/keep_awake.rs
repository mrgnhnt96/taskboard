//! Midna keeps the Mac awake for agents (`keep_awake.*`), and the board's work hours are its
//! schedule: their start, end and days, and "today until". The board gives them to Midna whenever
//! they change, and again on a sync if Midna's have drifted (edited in Midna, or Midna reset). With
//! no work hours, agents start any time, so Midna's window is all day, every day. Everything else
//! (on/off, mode, battery floor, linger, one day's own hours, "off today") is Midna's, changed
//! through `tb keep-awake`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::app::App;
use crate::hours::{self, DAYS};
use crate::midna::{call_timeout, MidnaError};
use crate::util::*;

/// How soon the board gives Midna the same schedule again when Midna's still differs.
const RETRY: Duration = Duration::from_secs(60);
const FROM_HOURS: [&str; 3] = ["start", "end", "days"];

/// `GET/POST /keep-awake`: Midna's status, or a change to it. The schedule isn't changed here.
pub fn call(app: &App, set: Option<&Value>) -> Result<Value> {
    if let Some(b) = set {
        if FROM_HOURS.iter().any(|k| body_has(b, k)) {
            return err(400, "Keep-awake follows the work hours. Change them with `tb hours`.");
        }
        let today = body_str(b, "today").trim().to_lowercase();
        if body_has(b, "today") && !matches!(today.as_str(), "off" | "clear") {
            return err(400, "Today's keep-awake ends with today's work hours: use `tb hours --today-until`. Here, only off or clear.");
        }
    }
    if set.is_some() && !app.cfg.runner {
        return err(409, "This board has no runner (a dev board), so it leaves Midna's keep-awake alone.");
    }
    let r = match set {
        Some(b) => call_timeout(app, "keep_awake.set", b.clone(), 10.0),
        None => call_timeout(app, "keep_awake.status", json!({}), 10.0),
    };
    match r {
        Ok(v) if v.is_object() => {
            app.shared.lock().midna_keep_awake = Some(v.clone());
            Ok(v)
        }
        Ok(_) => err(502, "Midna gave back no keep-awake status."),
        Err(MidnaError::Down(m)) => err(503, format!("{m}, so keep-awake can't be read or changed.")),
        Err(MidnaError::Refused(m)) if m.starts_with("unknown method") => err(501, "This Midna has no keep-awake yet. Update Midna."),
        Err(MidnaError::Refused(m)) => err(400, m),
    }
}

/// The schedule Midna should have: the work hours, or all day every day without them.
pub fn wanted(app: &App) -> Value {
    let h = hours::get(app);
    if h.on {
        json!({"start": h.start, "end": h.end, "days": h.days})
    } else {
        json!({"start": "00:00", "end": "00:00", "days": DAYS})
    }
}

fn differs(status: &Value, want: &Value) -> bool {
    FROM_HOURS.iter().any(|k| status["settings"][*k] != want[*k])
}

/// Only a board with its runner gives Midna a schedule: a dev board's hours aren't the owner's.
fn push(app: &App, body: Value, want: Value) {
    if !app.cfg.runner {
        return;
    }
    app.shared.lock().keep_awake_pushed = Some((Instant::now(), want));
    match call_timeout(app, "keep_awake.set", body, 10.0) {
        Ok(v) if v.is_object() => app.shared.lock().midna_keep_awake = Some(v),
        Ok(_) => {}
        Err(e) => app.info(format!("midna: couldn't give keep-awake the work hours: {e}")),
    }
}

/// The work hours just changed: give Midna the schedule now. `today` is the new "today until"
/// when that changed too (`None` inside = back to the usual end).
pub fn follow_hours(app: &App, today: Option<Option<String>>) {
    if app.shared.lock().midna_keep_awake.is_none() {
        return;
    }
    let want = wanted(app);
    let mut body = want.clone();
    if let Some(t) = today {
        body["today"] = t.map(|u| json!({"on": true, "until": u})).unwrap_or_else(|| json!("clear"));
    }
    push(app, body, want);
}

/// Each Midna sync: read keep-awake's status and put the work hours back if Midna's differ.
pub fn sync(app: &App) {
    let status = call_timeout(app, "keep_awake.status", json!({}), 5.0).ok().filter(|v| v.is_object());
    app.shared.lock().midna_keep_awake = status.clone();
    let Some(status) = status else { return };
    let want = wanted(app);
    if !differs(&status, &want) {
        return;
    }
    let recent = app.shared.lock().keep_awake_pushed.as_ref().is_some_and(|(at, w)| *w == want && at.elapsed() < RETRY);
    if !recent {
        push(app, want.clone(), want);
    }
}
