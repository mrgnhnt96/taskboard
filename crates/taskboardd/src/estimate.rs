//! Estimates against actual working time.
//!
//! An agent estimates, before the work starts, how long a task or goal will take it: `tb task new --est
//! 45m`, `::est=45m` on each `--task` item of `tb goal new` and `tb propose`, `--est` on `tb goal new`
//! for the goal's running time. The board keeps it in `est_min` with `est_at`, when it was made. The
//! owner never makes or changes one. Once work starts the first estimate is kept: it can be changed
//! only while nothing has started, and a task or goal made before estimates gets its first one
//! whenever it comes (`tb estimate <len>`, which a task's handoff asks for).
//!
//! The actual time counts the `working` status only, from `task_states`: needs, queued, planned and
//! done are left out, so time waiting on the owner or a review never counts. A goal has two: its
//! tasks' working time added up (`work_min`), and its running time (`actual_min`), the time at least
//! one of its tasks was working, which is what the goal's estimate is compared with.
//!
//! `human_min` (`tb submit --human`) is another number: how long a developer would have taken by hand.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, human, p};

/// Minutes in an estimate as given: `45m`, `3h`, `1h30m` or a number of minutes. `None` when none
/// was given.
pub fn parse(v: Option<&Value>) -> Result<Option<i64>> {
    let bad = || ApiError::new(400, "Give the estimate as a length of working time, like 45m, 3h or 1h30m.");
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n.as_i64().filter(|m| *m > 0).map(Some).ok_or_else(bad),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => parse_minutes(s).map(Some).ok_or_else(bad),
        _ => Err(bad()),
    }
}

/// What the refusals ask for: one line on what an estimate is, and that it will be compared.
pub fn what_it_is(app: &App) -> String {
    format!(
        "your honest guess of the agent working time it takes, not counting time waiting on {}. The board compares it with the time actually spent working.",
        app.cfg.owner
    )
}

/// A new agent task's estimate, refused when it has none.
pub fn need_task(app: &App, body: &Value) -> Result<i64> {
    match parse(body.get("est"))? {
        Some(m) => Ok(m),
        None => err(400, format!("Add an estimate: --est <len> (45m, 3h, 1h30m), {}", what_it_is(app))),
    }
}

/// A new goal's estimate of its running time, refused when it has none.
pub fn need_goal(app: &App, body: &Value) -> Result<i64> {
    match parse(body.get("est"))? {
        Some(m) => Ok(m),
        None => err(
            400,
            format!(
                "Add an estimate of the goal's running time: --est <len> (3h, 1d), from its first task starting to its last one finishing, \
                 counting only time a task is working; and ::est=<len> on each --task. It's {}",
                what_it_is(app)
            ),
        ),
    }
}

/// Refuses `--task` items with no `::est=` (human tasks need none), naming them all.
pub fn need_items(app: &App, missing: &[String]) -> Result<()> {
    if missing.is_empty() {
        return Ok(());
    }
    let list = missing.iter().map(|t| format!("“{t}”")).collect::<Vec<_>>().join(", ");
    err(400, format!("Each --task needs an estimate: add ::est=<len> (45m, 3h, 1h30m) to {list}. It's {}", what_it_is(app)))
}

/// Whether work on the task has started.
fn started(t: &Row) -> bool {
    has(t.s("started_at"))
}

/// Whether any of the goal's tasks has started.
fn goal_started(app: &App, gid: i64) -> Result<bool> {
    Ok(crate::shared::counted_tasks(app, gid)?.iter().any(started))
}

fn refuse_owner(app: &App, who: &str) -> Result<()> {
    if who == board::OWNER || who == app.cfg.owner {
        return err(403, format!("Estimates are the agent's to make; {} doesn't set them.", app.cfg.owner));
    }
    Ok(())
}

/// Records the task's estimate: its first at any time, a new one only while it hasn't started.
pub fn set_task(app: &App, t: &Row, min: i64, who: &str) -> Result<()> {
    refuse_owner(app, who)?;
    if human::is(t) {
        return err(400, format!("{} is a human task: it has no estimate.", human::task_ref(t)));
    }
    if let Some(had) = t.i("est_min") {
        if had == min {
            return Ok(());
        }
        if started(t) {
            return err(
                409,
                format!("{} started with an estimate of {}; the first estimate is kept once work starts.", human::task_ref(t), len(had as f64)),
            );
        }
    }
    board::update_task(app, t.id(), fields!["est_min" => min, "est_at" => now_iso()])?;
    let text = match t.i("est_min") {
        Some(had) => format!("Estimate changed from {} to {}", len(had as f64), len(min as f64)),
        None => format!("Estimated {} of working time", len(min as f64)),
    };
    board::log_event(app, t.id(), who, "note", &text).map(|_| ())
}

/// Records the goal's estimate of its running time, under the same rule as a task's.
pub fn set_goal(app: &App, g: &Row, min: i64, who: &str) -> Result<()> {
    refuse_owner(app, who)?;
    if let Some(had) = g.i("est_min") {
        if had == min {
            return Ok(());
        }
        if goal_started(app, g.id())? {
            return err(
                409,
                format!("{}'s work started with an estimate of {}; the first estimate is kept once work starts.", rf("goal", g.id()), len(had as f64)),
            );
        }
    }
    board::update_goal(app, g.id(), fields!["est_min" => min, "est_at" => now_iso()])
}

/// Each task's stretches in the `working` status, ever, as epoch seconds (tasks that never worked are
/// left out). One still working runs to now.
pub fn stretches(app: &App, tasks: &[i64]) -> Result<HashMap<i64, Vec<(f64, f64)>>> {
    let mut out: HashMap<i64, Vec<(f64, f64)>> = HashMap::new();
    if tasks.is_empty() {
        return Ok(out);
    }
    let now = now_ts();
    let list = tasks.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(",");
    let states = app.db.q(&format!("SELECT task_id, at, status FROM task_states WHERE task_id IN ({list}) ORDER BY task_id, at, id"), p![])?;
    for (i, s) in states.iter().enumerate() {
        if s.s("status") != Some("working") {
            continue;
        }
        let task = s.i0("task_id");
        let a = s.s("at").and_then(parse_iso).unwrap_or(now);
        let b = states.get(i + 1).filter(|n| n.i0("task_id") == task).and_then(|n| n.s("at")).and_then(parse_iso).unwrap_or(now);
        if b > a {
            out.entry(task).or_default().push((a, b));
        }
    }
    Ok(out)
}

/// Each task's working seconds, ever, by task (tasks that never worked are left out).
pub fn work_by_task(app: &App, tasks: &[i64]) -> Result<HashMap<i64, f64>> {
    Ok(stretches(app, tasks)?.into_iter().map(|(t, s)| (t, s.iter().map(|(a, b)| b - a).sum())).collect())
}

/// A goal's working seconds: its tasks' added up, and the running time (seconds at least one was working).
pub fn goal_work(app: &App, gid: i64) -> Result<(f64, f64)> {
    let ids: Vec<i64> = crate::shared::counted_tasks(app, gid)?.iter().filter(|t| !human::is(t)).map(|t| t.id()).collect();
    let all: Vec<(f64, f64)> = stretches(app, &ids)?.into_values().flatten().collect();
    let sum = all.iter().map(|(a, b)| b - a).sum();
    let running = crate::clock::merge(all).iter().map(|(a, b)| b - a).sum();
    Ok((sum, running))
}

/// A length of time as the board writes it: `45m`, `3h`, `1h 10m`.
pub fn len(min: f64) -> String {
    let m = min.max(0.0).round() as i64;
    match (m / 60, m % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

fn whole_min(secs: f64) -> i64 {
    (secs / 60.0).round() as i64
}

/// Actual over estimate, to one place: 1.6 took 60% longer than estimated.
fn ratio(est: Option<i64>, actual_min: i64) -> Option<f64> {
    let e = est.filter(|e| *e > 0)?;
    (actual_min > 0).then(|| (actual_min as f64 / e as f64 * 10.0).round() / 10.0)
}

/// The estimate fields on a task's detail: `est_min`, `est_at`, `actual_min` (working minutes) and
/// `est_ratio` (actual over estimate).
pub fn task_view(app: &App, t: &Row) -> Result<Value> {
    let actual = whole_min(work_by_task(app, &[t.id()])?.get(&t.id()).copied().unwrap_or(0.0));
    Ok(json!({"est_min": t.v("est_min"), "est_at": t.v("est_at"), "actual_min": actual, "est_ratio": ratio(t.i("est_min"), actual)}))
}

/// The estimate fields on a goal's detail: `actual_min` is its running time, which `est_min` estimates;
/// `work_min` its tasks' working time added up.
pub fn goal_view(app: &App, g: &Row) -> Result<Value> {
    let (sum, running) = goal_work(app, g.id())?;
    let actual = whole_min(running);
    Ok(json!({"est_min": g.v("est_min"), "est_at": g.v("est_at"), "actual_min": actual, "work_min": whole_min(sum),
              "est_ratio": ratio(g.i("est_min"), actual)}))
}

/// "Est 45m · Actual 1h 10m (1.6×)" from a view; none when there's neither an estimate nor working time.
pub fn line(v: &Value) -> Option<String> {
    let est = v["est_min"].as_i64();
    let actual = v["actual_min"].as_i64().unwrap_or(0);
    if est.is_none() && actual == 0 {
        return None;
    }
    let mut s = format!("Est {}", est.map(|e| len(e as f64)).unwrap_or_else(|| "none".into()));
    if actual > 0 {
        s += &format!(" · Actual {}", len(actual as f64));
        if let Some(r) = v["est_ratio"].as_f64() {
            s += &format!(" ({r}×)");
        }
    }
    Some(s)
}

/// The handoff's ask for an estimate, for an agent task that has none.
pub fn handoff_lines(app: &App, t: &Row) -> Vec<String> {
    if human::is(t) || t.i("est_min").is_some() {
        return vec![];
    }
    let tb = board::tb_cmd(app);
    vec![format!(
        "This task has no estimate. Before any other work, run {tb} estimate <len> (45m, 3h, 1h30m): {}",
        what_it_is(app)
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lengths_and_refuses_others() {
        assert_eq!(parse(Some(&json!("45m"))).unwrap(), Some(45));
        assert_eq!(parse(Some(&json!("3h"))).unwrap(), Some(180));
        assert_eq!(parse(Some(&json!("1h30m"))).unwrap(), Some(90));
        assert_eq!(parse(Some(&json!(20))).unwrap(), Some(20));
        assert_eq!(parse(Some(&json!(""))).unwrap(), None);
        assert_eq!(parse(None).unwrap(), None);
        assert!(parse(Some(&json!("soon"))).is_err());
        assert!(parse(Some(&json!(0))).is_err());
    }

    #[test]
    fn writes_lengths_and_lines() {
        assert_eq!(len(45.0), "45m");
        assert_eq!(len(180.0), "3h");
        assert_eq!(len(70.0), "1h 10m");
        assert_eq!(line(&json!({"est_min": 45, "actual_min": 70, "est_ratio": ratio(Some(45), 70)})).unwrap(), "Est 45m · Actual 1h 10m (1.6×)");
        assert_eq!(line(&json!({"est_min": 45, "actual_min": 0})).unwrap(), "Est 45m");
        assert_eq!(line(&json!({"est_min": null, "actual_min": 30})).unwrap(), "Est none · Actual 30m");
        assert_eq!(line(&json!({"est_min": null, "actual_min": 0})), None);
    }
}
