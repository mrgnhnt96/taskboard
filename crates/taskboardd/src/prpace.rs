//! How often the timed poll reads each open PR (#677), with `[pr] pace_polls` on. A PR that moved in
//! the last half hour is read every 3 minutes; the longer it stays the same, the less often: every 10
//! minutes after half an hour, 30 after two hours, hourly after six. Any change a read finds, and any
//! stream event for the PR, starts it over (`moved_at` in its flow).
//!
//! Outside work hours (and a goal's `--now` run) only a PR that can move at night keeps that pace: an
//! approved, green PR the agents merge, or one waiting on a reviewer whose bot runs on a schedule. The
//! rest are read hourly, or when the stream says something. A PR whose checks have run `STUCK_TIMES`
//! its project's `expected_wait_mins` is read hourly too, and raised as stuck once per push.

use serde_json::{json, Value};

use crate::app::App;
use crate::prflow;
use crate::util::*;
use crate::{board, dispatch, hours};

/// (still for at least, read every), in seconds.
const LADDER: [(f64, f64); 4] = [(0.0, 180.0), (1800.0, 600.0), (7200.0, 1800.0), (21600.0, 3600.0)];
/// The slowest pace: off-hours PRs that can't move at night, and stuck checks.
const HOURLY: f64 = 3600.0;
/// Checks running this many times the expected wait are stuck.
const STUCK_TIMES: f64 = 3.0;

/// Whether the timed poll reads this open PR this round. A PR never read is always read.
pub fn due(app: &App, t: &Row) -> Result<bool> {
    if !app.cfg.pr.pace_polls {
        return Ok(true);
    }
    let f = prflow::flow(t);
    let Some(read) = f.s("read_at").and_then(parse_iso) else { return Ok(true) };
    // Half a poll round of slack, so a PR due just after this round isn't left for the next one.
    let round = app.cfg.intervals.prs * crate::prhost::limit::stretch();
    Ok(now_ts() - read + round / 2.0 >= every(app, t)?)
}

/// Seconds between reads of this PR.
pub fn every(app: &App, t: &Row) -> Result<f64> {
    let f = prflow::flow(t);
    let rec = f.get("rec").cloned().unwrap_or(Value::Null);
    let moved = f.s("moved_at").or(f.s("read_at")).and_then(parse_iso).unwrap_or_else(now_ts);
    let still = now_ts() - moved;
    let mut secs = LADDER.iter().rev().find(|(after, _)| still >= *after).map(|(_, e)| *e).unwrap_or(LADDER[0].1);
    if checks_stuck(app, t, &f, &rec) || (!open_now(app, t)? && !moves_at_night(app, t, &rec)?) {
        secs = secs.max(HOURLY);
    }
    Ok(secs)
}

/// Whether a read found the PR changed since the last one: anything but its base's head, or its stage.
pub fn moved(old: Option<&Value>, rec: &Value, phase_changed: bool) -> bool {
    let without_base = |r: &Value| {
        let mut r = r.clone();
        if let Some(o) = r.as_object_mut() {
            o.remove("base_head");
        }
        r
    };
    phase_changed || old.is_none_or(|o| without_base(o) != without_base(rec))
}

/// A stream event for the PR: its pace starts over.
pub fn stirred(app: &App, task_id: i64) -> Result<()> {
    prflow::merge_flow(app, task_id, crate::fields!["moved_at" => now_iso()]).map(|_| ())
}

fn open_now(app: &App, t: &Row) -> Result<bool> {
    Ok(hours::is_open(app) || hours::goal_open(app, board::find_goal(app, t.i("goal_id"))?.as_ref()))
}

/// The PR can move while no one works: the agents merge it, or a reviewer's bot is due to look.
fn moves_at_night(app: &App, t: &Row, rec: &Value) -> Result<bool> {
    match t.s("pr_phase").unwrap_or("") {
        "merge" => Ok(prflow::agents_merge_on(app, t.s("project"))),
        "review" | "rereview" => {
            let project = t.s("project").unwrap_or("");
            for r in rec["reviewers"].as_array().into_iter().flatten().filter(|r| r["state"] == "pending" || r["requested"] == true) {
                let user = r["user"].as_str().unwrap_or("");
                if let Some(row) = crate::reviewers::by_host_user(app, project, user)? {
                    if crate::botrun::timed(app, &row)? {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn checks_stuck(app: &App, t: &Row, f: &Row, rec: &Value) -> bool {
    t.s("pr_phase") == Some("checks") && prflow::head_waited(f, rec) > stuck_after(app, t)
}

fn stuck_after(app: &App, t: &Row) -> f64 {
    STUCK_TIMES * prflow::expected_wait_mins(app, t) * 60.0
}

/// In `step`: the flow fields for this read's pace, and the one alert for checks that are stuck.
pub fn note(app: &App, t: &Row, f: &Row, rec: &Value, phase: &str, phase_changed: bool) -> Result<Vec<(&'static str, Value)>> {
    let mut changes = vec![];
    if !f.contains_key("moved_at") || moved(f.get("rec"), rec, phase_changed) {
        changes.push(("moved_at", json!(now_iso())));
    }
    let head = rec["head"].as_str().unwrap_or("");
    let waited = prflow::head_waited(f, rec);
    if app.cfg.pr.pace_polls && phase == "checks" && waited > stuck_after(app, t) && f.s("checks_stuck") != Some(head) {
        dispatch::add_alert(
            app,
            &format!(
                "PR #{} for {} has been in checks for {}, far past its {} wait: it looks stuck, so the board reads it hourly now.",
                t.i0("pr_num"),
                rf("task", t.id()),
                crate::feed::span(waited),
                crate::feed::span(prflow::expected_wait_mins(app, t) * 60.0),
            ),
            Some(t.id()),
            t.i("goal_id"),
            None,
            Some("pr"),
        )?;
        changes.push(("checks_stuck", json!(head)));
    }
    Ok(changes)
}
