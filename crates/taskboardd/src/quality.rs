//! How well goals were planned, per project and over time. A finished goal is the unit: how many of
//! its tasks were added after its work started (not counting ones made with the goal or planned by
//! one of its design tasks), how many failed or had to be started again, how many review rounds
//! that found something to fix (see [`fix_rounds`]) and questions each task took, and the agent
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

/// Tasks made this soon after their goal are part of its plan, even when work had already started.
const WITH_THE_GOAL_SECS: f64 = 5.0 * 60.0;

/// Bumped when the numbers are counted differently, so kept ones are worked out again.
const VERSION: &str = "v3";

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

fn at(t: &Row, k: &str) -> Option<f64> {
    t.s(k).and_then(parse_iso)
}

/// The goal's tasks its plan didn't have: made after its first task started. A task made with the
/// goal (within [`WITH_THE_GOAL_SECS`] after it) is part of the plan, and so is one made while a design
/// task in the goal was planning the work: from the design's start until the first other task
/// started after it finished.
pub fn added_late(goal: &Row, tasks: &[Row]) -> Vec<i64> {
    let Some(started) = tasks.iter().filter_map(|t| at(t, "started_at")).reduce(f64::min) else { return vec![] };
    let goal_made = at(goal, "created_at");
    let is_design = |t: &Row| t.s("kind") == Some(crate::design::KIND);
    let windows: Vec<(f64, f64)> = tasks
        .iter()
        .filter(|d| is_design(d))
        .filter_map(|d| {
            let from = at(d, "started_at")?;
            let done = at(d, "finished_at").unwrap_or(f64::INFINITY);
            let next = tasks.iter().filter(|t| !is_design(t)).filter_map(|t| at(t, "started_at")).filter(|s| *s >= done).reduce(f64::min);
            Some((from, next.unwrap_or(f64::INFINITY)))
        })
        .collect();
    tasks
        .iter()
        .filter(|t| {
            let Some(made) = at(t, "created_at") else { return false };
            made > started
                && !goal_made.is_some_and(|g| made >= g && made <= g + WITH_THE_GOAL_SECS)
                && !windows.iter().any(|(from, to)| made >= *from && made <= *to)
        })
        .map(|t| t.id())
        .collect()
}

/// The review rounds a task took that found something to fix: PR visits to fix failing checks or
/// address review comments, steps that didn't pass, and code review rounds with a finding triaged
/// to fix. Visits to rebase or merge, and rounds that passed, aren't counted: every PR's merge
/// visit used to count as two rounds, which kept the number at 2.0 a task.
pub fn fix_rounds(app: &App, id: i64) -> Result<i64> {
    let visits: Vec<String> = ["fix", "comments"].iter().map(|p| format!(": {}", crate::prflow::label(p).to_lowercase())).collect();
    let handoffs = app.db.q("SELECT text FROM events WHERE task_id = ? AND kind = 'handoff' AND text LIKE 'Brought the conversation back for PR #%'", p![id])?;
    let mut n = handoffs.iter().filter(|e| visits.iter().any(|v| e.st("text").ends_with(v.as_str()))).count();
    n += crate::steps::rounds(app, id)?.iter().filter(|r| !r.passed).count();
    n += crate::code_review::rounds(app, id)?
        .iter()
        .filter(|r| r["triage"].as_object().is_some_and(|o| o.values().any(|t| t["outcome"] == "fix")))
        .count();
    Ok(n as i64)
}

/// The questions a task asked the owner: its `question` events that start "Asked" (in its reply, in
/// its terminal, on its PR or through the board). The rest of that kind are stops, not questions:
/// usage limits and API errors, "Waiting for you" notices and PR visits that stopped early.
pub fn real_questions(app: &App, id: i64) -> Result<i64> {
    count(app, "SELECT COUNT(*) FROM events WHERE task_id = ? AND kind = 'question' AND text LIKE 'Asked%'", id)
}

/// The numbers for one finished goal's tasks.
pub fn work_out(app: &App, goal: &Row, tasks: &[Row]) -> Result<Value> {
    let finished = tasks.iter().filter_map(|t| t.s("finished_at")).max().map(|s| s.to_string());
    let late = added_late(goal, tasks).len();
    let (mut failed, mut rounds, mut questions, mut est, mut work) = (0, 0, 0, 0.0, 0.0);
    for t in tasks {
        let id = t.id();
        let failed_runs = count(app, "SELECT COUNT(*) FROM task_states WHERE task_id = ? AND failed = 1", id)?;
        if failed_runs > 0 || t.i0("start_tries") > 1 {
            failed += 1;
        }
        rounds += fix_rounds(app, id)?;
        questions += real_questions(app, id)?;
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

/// Changes when the goal gets another task or one finishes again, or the numbers are counted
/// differently, so kept numbers are worked out afresh.
fn fingerprint(tasks: &[Row]) -> String {
    format!("{VERSION}:{}", bare_fingerprint(tasks))
}

fn bare_fingerprint(tasks: &[Row]) -> String {
    format!("{}:{}", tasks.len(), tasks.iter().filter_map(|t| t.s("finished_at")).max().unwrap_or(""))
}

/// The kept numbers were for these tasks, whichever way they were counted.
fn same_tasks(kept: &Value, tasks: &[Row]) -> bool {
    let f = kept["fingerprint"].as_str().unwrap_or("");
    let bare = f.strip_prefix('v').and_then(|r| r.split_once(':')).map_or(f, |(_, b)| b);
    bare == bare_fingerprint(tasks)
}

/// The cleanup took the events behind the goal's numbers: none of its tasks has any left.
fn events_gone(app: &App, tasks: &[Row]) -> Result<bool> {
    let ids = tasks.iter().map(|t| t.id().to_string()).collect::<Vec<_>>().join(",");
    Ok(app.db.val(&format!("SELECT COUNT(*) FROM events WHERE task_id IN ({ids})"), p![])?.as_i64().unwrap_or(0) == 0)
}

/// A finished goal's numbers: the kept ones while they still fit its tasks, else worked out and kept.
pub fn for_goal(app: &App, goal: &Row) -> Result<Option<Value>> {
    let Some(tasks) = finished_tasks(app, goal.id())? else { return Ok(None) };
    let key = setting_key(goal.id());
    let kept = app.db.get_setting(&key)?.and_then(|s| serde_json::from_str::<Value>(&s).ok());
    if let Some(mut v) = kept {
        if v["fingerprint"] == fingerprint(&tasks) {
            return Ok(Some(v));
        }
        // Counted the old way, and the events to count them again are gone: keep those numbers,
        // but the tasks added late come from the tasks alone, so count them again.
        if same_tasks(&v, &tasks) && events_gone(app, &tasks)? {
            v["added_late"] = json!(added_late(goal, &tasks).len());
            v["fingerprint"] = json!(fingerprint(&tasks));
            app.db.set_setting(&key, Some(&jdumps(&v)))?;
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
        "actual_vs_estimate": if est > 0.0 { Some((work / est * 1000.0).round() / 1000.0) } else { None },
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
