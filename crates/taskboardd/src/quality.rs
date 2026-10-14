//! How well goals were planned, per project and over time. A finished goal is the unit: how many of
//! its tasks were added after its work started, how many failed or had to be started again, how
//! many review rounds (review steps and PR fix cycles) and questions each task took, and the agent
//! time its tasks took against the time a developer was estimated to take. A goal's numbers are
//! worked out once it finishes and kept (setting `quality:G<n>`), so they outlive the cleanup of
//! the events behind them; `tb quality` shows them by month, the Days page the last 30 days.

use std::collections::BTreeMap;

use chrono::{Duration, NaiveDate, TimeZone};
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, p};

/// The Days page compares the 30 days up to the day shown with this many days before them.
const RECENT_DAYS: i64 = 30;
const BEFORE_DAYS: i64 = 180;

fn setting_key(gid: i64) -> String {
    format!("quality:{}", rf("goal", gid))
}

/// A goal's counted tasks when it has finished (every task done, no PR still open), else `None`.
fn finished_tasks(app: &App, gid: i64) -> Result<Option<Vec<Row>>> {
    let tasks = crate::shared::counted_tasks(app, gid)?;
    let done = !tasks.is_empty() && tasks.iter().all(|t| t.s("status") == Some("done")) && !tasks.iter().any(|t| !t.b("failed") && board::pr_still_open(t));
    Ok(done.then_some(tasks))
}

fn count(app: &App, sql: &str, id: i64) -> Result<i64> {
    Ok(app.db.val(sql, p![id])?.as_i64().unwrap_or(0))
}

/// Minutes the task spent working, from its status changes.
fn work_min(app: &App, id: i64) -> Result<f64> {
    let states = app.db.q("SELECT at, status FROM task_states WHERE task_id = ? ORDER BY at, id", p![id])?;
    let mut sum = 0.0;
    for (i, s) in states.iter().enumerate() {
        if s.s("status") != Some("working") {
            continue;
        }
        let a = s.s("at").and_then(parse_iso);
        let b = states.get(i + 1).and_then(|n| n.s("at")).and_then(parse_iso);
        if let (Some(a), Some(b)) = (a, b) {
            sum += (b - a).max(0.0);
        }
    }
    Ok(sum / 60.0)
}

/// The numbers for one finished goal's tasks.
pub fn work_out(app: &App, goal: &Row, tasks: &[Row]) -> Result<Value> {
    let started = tasks.iter().filter_map(|t| t.s("started_at")).min().map(|s| s.to_string());
    let finished = tasks.iter().filter_map(|t| t.s("finished_at")).max().map(|s| s.to_string());
    let (mut late, mut failed, mut rounds, mut questions, mut est, mut work) = (0, 0, 0, 0, 0.0, 0.0);
    for t in tasks {
        let id = t.id();
        if started.as_deref().is_some_and(|s| t.s("created_at").is_some_and(|c| c > s)) {
            late += 1;
        }
        let failed_runs = count(app, "SELECT COUNT(*) FROM task_states WHERE task_id = ? AND failed = 1", id)?;
        if failed_runs > 0 || t.i0("start_tries") > 1 {
            failed += 1;
        }
        rounds += count(app, "SELECT COUNT(*) FROM events WHERE task_id = ? AND kind = 'step'", id)?;
        rounds += count(app, "SELECT COUNT(*) FROM events WHERE task_id = ? AND kind = 'handoff' AND text LIKE '%for PR #%'", id)?;
        questions += count(app, "SELECT COUNT(*) FROM events WHERE task_id = ? AND kind = 'question'", id)?;
        if let Some(m) = t.i("human_min").filter(|m| *m > 0) {
            est += m as f64;
            work += work_min(app, id)?;
        }
    }
    Ok(json!({
        "goal": rf("goal", goal.id()), "name": goal.st("name"), "project": goal.s("project").filter(|p| !p.is_empty()),
        "finished_at": finished, "fingerprint": fingerprint(tasks),
        "tasks": tasks.len(), "added_late": late, "failed": failed, "review_rounds": rounds, "questions": questions,
        "est_min": est.round(), "work_min": work.round(),
    }))
}

/// Changes when the goal gets another task or one finishes again, so kept numbers are worked out afresh.
fn fingerprint(tasks: &[Row]) -> String {
    format!("{}:{}", tasks.len(), tasks.iter().filter_map(|t| t.s("finished_at")).max().unwrap_or(""))
}

/// A finished goal's numbers: the kept ones while they still fit its tasks, else worked out and kept.
pub fn for_goal(app: &App, goal: &Row) -> Result<Option<Value>> {
    let Some(tasks) = finished_tasks(app, goal.id())? else { return Ok(None) };
    let key = setting_key(goal.id());
    if let Some(v) = app.db.get_setting(&key)?.and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
        if v["fingerprint"] == fingerprint(&tasks) {
            return Ok(Some(v));
        }
    }
    let v = work_out(app, goal, &tasks)?;
    app.db.set_setting(&key, Some(&jdumps(&v)))?;
    Ok(Some(v))
}

/// The goal just finished: keep its numbers while the events behind them are still there.
pub fn finished(app: &App, goal: &Row) -> Result<()> {
    for_goal(app, goal).map(|_| ())
}

/// Every finished goal's numbers (in `project`, when given), oldest first.
pub fn goals(app: &App, project: Option<&str>) -> Result<Vec<Value>> {
    let rows = match project {
        Some(p) => app.db.q("SELECT * FROM goals WHERE project = ? ORDER BY id", p![p])?,
        None => app.db.q("SELECT * FROM goals ORDER BY id", p![])?,
    };
    let mut out = vec![];
    for g in &rows {
        if let Some(v) = for_goal(app, g)? {
            out.push(v);
        }
    }
    out.sort_by(|a, b| a["finished_at"].as_str().cmp(&b["finished_at"].as_str()));
    Ok(out)
}

/// Goals' numbers added up, with the rates that make them comparable.
pub fn sum(goals: &[&Value]) -> Value {
    let n = |k: &str| goals.iter().map(|g| g[k].as_f64().unwrap_or(0.0)).sum::<f64>();
    let tasks = n("tasks");
    let per = |x: f64| if tasks > 0.0 { Some((x / tasks * 100.0).round() / 100.0) } else { None };
    let (est, work) = (n("est_min"), n("work_min"));
    json!({
        "goals": goals.len(), "tasks": tasks, "added_late": n("added_late"), "failed": n("failed"),
        "review_rounds": n("review_rounds"), "questions": n("questions"), "est_min": est, "work_min": work,
        "late_share": per(n("added_late")), "failed_share": per(n("failed")),
        "rounds_per_task": per(n("review_rounds")), "questions_per_task": per(n("questions")),
        "actual_vs_estimate": if est > 0.0 { Some((work / est * 100.0).round() / 100.0) } else { None },
    })
}

fn local_date(iso_s: &str) -> Option<NaiveDate> {
    let ts = parse_iso(iso_s)?;
    chrono::Local.timestamp_opt(ts as i64, 0).single().map(|d| d.date_naive())
}

fn project_of(g: &Value) -> String {
    g["project"].as_str().unwrap_or(crate::days::NO_PROJECT).to_string()
}

/// `GET /quality?project=<name>|all` (or `?cwd=` for the project it's in): each project's numbers by
/// the month its goals finished, newest first, and its goals.
pub fn get(app: &App, query: &crate::api::Query) -> Result<Value> {
    let arg = |k: &str| query.get(k).map(|s| s.as_str()).filter(|s| !s.is_empty());
    let project = match arg("project") {
        Some("all") => None,
        Some(p) => Some(p.to_string()),
        None => crate::projects::project_for_path(app, arg("cwd"))?,
    };
    let all = goals(app, project.as_deref())?;
    let mut by: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for g in &all {
        by.entry(project_of(g)).or_default().push(g);
    }
    let projects: Vec<Value> = by
        .iter()
        .map(|(name, gs)| {
            let mut months: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
            for g in gs {
                let m = g["finished_at"].as_str().and_then(local_date).map(|d| d.format("%Y-%m").to_string()).unwrap_or_default();
                months.entry(m).or_default().push(g);
            }
            let months: Vec<Value> = months.iter().rev().map(|(m, gs)| {
                let mut v = sum(gs);
                v["month"] = json!(m);
                v
            }).collect();
            json!({"project": name, "total": sum(gs), "months": months, "goals": gs.iter().rev().collect::<Vec<_>>()})
        })
        .collect();
    Ok(json!({"project": project, "projects": projects}))
}

/// The Days page's planning card: per project, the goals finished in the 30 days up to `date`
/// against the half year before them. Only projects that finished a goal in those 30 days.
pub fn recent(app: &App, date: NaiveDate) -> Result<Vec<Value>> {
    let from = date - Duration::days(RECENT_DAYS - 1);
    let before = from - Duration::days(BEFORE_DAYS);
    let all = goals(app, None)?;
    let mut by: BTreeMap<String, (Vec<&Value>, Vec<&Value>)> = BTreeMap::new();
    for g in &all {
        let Some(d) = g["finished_at"].as_str().and_then(local_date) else { continue };
        let slot = by.entry(project_of(g)).or_default();
        if d >= from && d <= date {
            slot.0.push(g);
        } else if d >= before && d < from {
            slot.1.push(g);
        }
    }
    Ok(by
        .into_iter()
        .filter(|(_, (now, _))| !now.is_empty())
        .map(|(p, (now, earlier))| json!({"project": p, "recent": sum(&now), "before": if earlier.is_empty() { Value::Null } else { sum(&earlier) }}))
        .collect())
}
