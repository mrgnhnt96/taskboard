//! Weekly and monthly recaps: the facts of a finished week or month, kept in `recaps` with the
//! page made from them (`writing`, and `writing_status`: `none`, `writing`, `done` or `failed`;
//! see `recap_page.rs`). `POST /recaps` makes one for a named period; the app reads them
//! (`GET /recaps`, `GET /recaps/<id>`, `POST /recaps/<id>/seen`). There's no `tb` command for them.
//!
//! # Schedule
//!
//! `[recap]` makes them on its own: the week's on `weekly_day` (Monday) at `at` (9:00 AM), for the
//! last Monday to Sunday that has ended, and the month's on `monthly_day` (the 1st) at `at`, for the
//! last calendar month that has ended. Each period is made once however late the board wakes, and a
//! period with no finished task is skipped. Once its page is written, or its writing failed, a
//! notification opens the recap; there's no Needs you item.
//!
//! # Periods
//!
//! A week runs from Monday 12:00 AM to the next Monday 12:00 AM, whatever `first_weekday` says,
//! because its recap appears on the Monday after. A month is the calendar month. Days, weeks and
//! months are counted in the Mac's local time zone; the times stored are UTC.
//!
//! # Agent time
//!
//! The Days page's agent time (`day_stats.agent_min`) is every task's time in the `working` status
//! added up. Tasks that work side by side each count, so it's agent-hours, not hours on the clock,
//! and it covers everything an agent does in its turn: thinking, writing, and waiting on the
//! commands it runs. The spans (`spans.rs`) only time the tool calls, so they come out far lower:
//! the week of November 2, 2026 had 139 agent-hours, 43 hours with at least one agent working, and
//! 33 hours of tool calls (17 hours once overlapping calls count once). The recap keeps three
//! numbers and labels each:
//!
//! - `agent_min`, **agent time**, the headline: tasks' working time added up, the same as the Days page.
//! - `busy_min`, **busy hours**: time on the clock with at least one agent working.
//! - `command_min`, **command time**: the tools the agents ran (builds, tests, git and the rest),
//!   left out of which are a subagent's wrapper span and refused calls. It's what the time by
//!   category is made of.
//!
//! Agent time by project is agent time; time by category is command time.
//!
//! # Shipped
//!
//! A task counts in the period it last finished: done, not failed, and not answered "can't do" or
//! "not needed". Of those, as they stand when the recap is made:
//!
//! - **merged**: an agent task whose PR merged (`pr_state` is `MERGED`), or that had no PR. A task
//!   that shares another task's PR goes by that PR.
//! - **PR open**: its PR hasn't merged yet; counted on its own, not shipped.
//! - **closed PR**: its PR was closed without merging; left out of the lists and the shipped count.
//! - **human tasks** (`kind` is `human`, the owner's own) are counted on their own and flagged.
//!
//! **Released** is reported apart from merged: the releases cut in the period (not failed), each with
//! the tasks it carried. PRs and lines changed come from the merges in the period (`pr_merges`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::days::{self, NO_PROJECT};
use crate::util::*;
use crate::{dispatch, hours, human, midna, p, quality};

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS recaps(
  id INTEGER PRIMARY KEY, kind TEXT NOT NULL, period_start TEXT NOT NULL, period_end TEXT NOT NULL,
  created_at TEXT, facts TEXT, writing TEXT, writing_status TEXT NOT NULL DEFAULT 'none', seen_at TEXT,
  UNIQUE(kind, period_start));
"#;

pub const KINDS: &[&str] = &["weekly", "monthly"];

/// Bumped when the facts change shape.
const VERSION: i64 = 1;
/// Loose tasks (not in a goal) listed per project; the rest are counted.
const LOOSE_CAP: usize = 8;
/// Tasks listed per release; the rest are counted.
const RELEASE_CAP: usize = 20;
/// Tool spans that are a subagent's whole run, which the subagent's own spans already count.
const WRAPPERS: &[&str] = &["Task", "Agent"];

fn kind_of(kind: &str) -> Result<&'static str> {
    KINDS.iter().find(|k| **k == kind).copied().ok_or_else(|| ApiError::new(400, "A recap is weekly or monthly."))
}

/// The Monday on or before `d`.
pub fn monday_of(d: NaiveDate) -> NaiveDate {
    d - Duration::days(d.weekday().num_days_from_monday() as i64)
}

/// The `kind` period that holds `d`: its first day and the day after its last.
pub fn period(kind: &str, d: NaiveDate) -> Result<(NaiveDate, NaiveDate)> {
    Ok(match kind_of(kind)? {
        "weekly" => {
            let m = monday_of(d);
            (m, m + Duration::days(7))
        }
        _ => {
            let first = d.with_day(1).unwrap_or(d);
            let next = if first.month() == 12 { NaiveDate::from_ymd_opt(first.year() + 1, 1, 1) } else { NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1) };
            (first, next.unwrap_or(first + Duration::days(31)))
        }
    })
}

/// The period before the one starting `first`.
pub fn previous(kind: &str, first: NaiveDate) -> Result<(NaiveDate, NaiveDate)> {
    period(kind, first - Duration::days(1))
}

/// The latest `kind` period that has ended by `today`.
pub fn last_complete(kind: &str, today: NaiveDate) -> Result<(NaiveDate, NaiveDate)> {
    previous(kind, period(kind, today)?.0)
}

/// Rounded to a tenth; an empty sum's `-0.0` comes out as `0.0`.
fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0 + 0.0
}

fn mins(secs: f64) -> f64 {
    round1(secs / 60.0)
}

fn project_name(p: Option<&str>) -> String {
    p.map(str::trim).filter(|p| !p.is_empty()).unwrap_or(NO_PROJECT).to_string()
}

fn date_str(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

// ------------------------------------------------------------------ gathering a period

/// A task that finished in the period, with how it shipped.
struct Done {
    id: i64,
    row: Row,
    project: String,
    human: bool,
    /// `merged`, `no_pr`, `open`, `closed` or `human`.
    shipped: &'static str,
    pr: Option<i64>,
    at: f64,
}

impl Done {
    fn merged(&self) -> bool {
        matches!(self.shipped, "merged" | "no_pr")
    }

    fn item(&self) -> Value {
        json!({"task": human::task_ref(&self.row), "title": self.row.st("title"), "summary": self.row.s("summary").map(|s| one_line(s, 400)),
               "project": self.project, "goal": rf_opt("goal", self.row.i("goal_id")), "human": self.human, "shipped": self.shipped,
               "pr": self.pr, "release": rf_opt("release", self.row.i("release_id")), "finished_at": iso(self.at)})
    }
}

struct Merge {
    project: String,
    added: i64,
    removed: i64,
}

/// Everything the facts are made of, for one stretch of time.
struct Gathered {
    from: f64,
    to: f64,
    done: Vec<Done>,
    merges: Vec<Merge>,
    commits: Vec<(String, f64)>,
    releases: Vec<Row>,
    /// Working stretches: task, project, from, to.
    working: Vec<(i64, String, f64, f64)>,
    /// Tool calls: project, category, seconds.
    spans: Vec<(String, String, f64)>,
}

fn done_tasks(app: &App, from: f64, to: f64) -> Result<Vec<Done>> {
    let rows = app.db.q(
        "SELECT t.*,
           CASE WHEN t.pr_num IS NOT NULL THEN t.pr_num ELSE a.pr_num END AS any_pr,
           CASE WHEN t.pr_num IS NOT NULL THEN t.pr_state ELSE a.pr_state END AS any_state,
           CASE WHEN t.pr_num IS NOT NULL THEN t.pr_phase ELSE a.pr_phase END AS any_phase
         FROM tasks t LEFT JOIN tasks a ON a.id = t.same_pr
         WHERE t.status = 'done' AND t.failed = 0 AND COALESCE(t.owner_answer, '') NOT IN ('cant_do', 'not_needed')
           AND t.finished_at >= ? AND t.finished_at < ?
         ORDER BY t.finished_at, t.id",
        p![iso(from), iso(to)],
    )?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let human = human::is(&r);
            let pr = r.i("any_pr");
            let state = r.st("any_state").to_uppercase();
            let phase = r.st("any_phase");
            let shipped = if human {
                "human"
            } else if pr.is_none() {
                "no_pr"
            } else if state == "MERGED" || phase == "merged" {
                "merged"
            } else if state == "CLOSED" || phase == "declined" {
                "closed"
            } else {
                "open"
            };
            Done { id: r.id(), project: project_name(r.s("project")), human, shipped, pr, at: r.s("finished_at").and_then(parse_iso).unwrap_or(from), row: r }
        })
        .collect())
}

fn working(app: &App, from: f64, to: f64) -> Result<Vec<(i64, String, f64, f64)>> {
    let rows = app.db.q(
        "SELECT s.task_id, s.at, s.status, t.project FROM task_states s LEFT JOIN tasks t ON t.id = s.task_id WHERE s.at < ?2 AND s.task_id IN (
           SELECT task_id FROM task_states WHERE at >= ?1 AND at < ?2
           UNION SELECT s.task_id FROM task_states s WHERE s.at < ?1 AND s.status = 'working'
             AND s.id = (SELECT x.id FROM task_states x WHERE x.task_id = s.task_id AND x.at < ?1 ORDER BY x.at DESC, x.id DESC LIMIT 1))
         ORDER BY s.task_id, s.at, s.id",
        p![iso(from), iso(to)],
    )?;
    let now = now_ts();
    let mut out = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        if r.s("status") != Some("working") {
            continue;
        }
        let task = r.i0("task_id");
        let Some(a) = r.s("at").and_then(parse_iso) else { continue };
        let b = rows.get(i + 1).filter(|n| n.i0("task_id") == task).and_then(|n| n.s("at")).and_then(parse_iso).unwrap_or(now);
        let (a, b) = (a.max(from), b.min(to).min(now));
        if b > a {
            out.push((task, project_name(r.s("project")), a, b));
        }
    }
    Ok(out)
}

fn gather(app: &App, from: f64, to: f64) -> Result<Gathered> {
    let span = p![iso(from), iso(to)];
    let mut merges: BTreeMap<(String, i64), Merge> = BTreeMap::new();
    for r in app.db.q(
        "SELECT m.pr_num, COALESCE(NULLIF(m.project, ''), t.project) AS project, m.added, m.removed FROM pr_merges m LEFT JOIN tasks t ON t.id = m.task_id
         WHERE m.merged_at >= ? AND m.merged_at < ?",
        span.clone(),
    )? {
        let project = project_name(r.s("project"));
        let pr = r.i0("pr_num");
        let m = merges.entry((project.clone(), pr)).or_insert(Merge { project, added: 0, removed: 0 });
        m.added = m.added.max(r.i0("added"));
        m.removed = m.removed.max(r.i0("removed"));
    }
    let commits = app
        .db
        .q("SELECT e.at, t.project FROM events e LEFT JOIN tasks t ON t.id = e.task_id WHERE e.kind = 'commit' AND e.text LIKE 'Committed%' AND e.at >= ? AND e.at < ?", span.clone())?
        .iter()
        .filter_map(|r| Some((project_name(r.s("project")), parse_iso(r.s("at")?)?)))
        .collect();
    let releases = app.db.q("SELECT * FROM releases WHERE failed_at IS NULL AND cut_at >= ? AND cut_at < ? ORDER BY cut_at, id", span.clone())?;
    let wrappers = WRAPPERS.iter().map(|w| format!("'{w}'")).collect::<Vec<_>>().join(", ");
    let spans = app
        .db
        .q(&format!("SELECT project, category, seconds FROM spans WHERE started_at >= ? AND started_at < ? AND refused = 0 AND tool NOT IN ({wrappers})"), span)?
        .iter()
        .map(|r| (project_name(r.s("project")), r.s("category").filter(|c| !c.is_empty()).unwrap_or("other").to_string(), r.f("seconds").unwrap_or(0.0).max(0.0)))
        .collect();
    Ok(Gathered { from, to, done: done_tasks(app, from, to)?, merges: merges.into_values().collect(), commits, releases, working: working(app, from, to)?, spans })
}

/// Seconds on the clock covered by at least one of `stretches`.
fn busy(stretches: impl Iterator<Item = (f64, f64)>) -> f64 {
    crate::clock::merge(stretches.collect()).iter().map(|(a, b)| b - a).sum()
}

/// Each task's working seconds, ever (for agent time against the estimate).
fn total_work(app: &App, tasks: &[i64]) -> Result<f64> {
    let now = now_ts();
    let mut sum = 0.0;
    for &t in tasks {
        let states = app.db.q("SELECT at, status FROM task_states WHERE task_id = ? ORDER BY at, id", p![t])?;
        for (i, s) in states.iter().enumerate() {
            if s.s("status") != Some("working") {
                continue;
            }
            let a = s.s("at").and_then(parse_iso).unwrap_or(now);
            let b = states.get(i + 1).and_then(|n| n.s("at")).and_then(parse_iso).unwrap_or(now);
            sum += (b - a).max(0.0);
        }
    }
    Ok(sum)
}

/// The headline numbers of one stretch (or one project's share of it).
fn totals(app: &App, g: &Gathered, project: Option<&str>) -> Result<Value> {
    let mine = |p: &str| project.is_none_or(|x| x == p);
    let done: Vec<&Done> = g.done.iter().filter(|d| mine(&d.project)).collect();
    let n = |s: &str| done.iter().filter(|d| d.shipped == s).count();
    let estimated: Vec<&&Done> = done.iter().filter(|d| d.merged() && d.row.i("human_min").is_some_and(|m| m > 0)).collect();
    let est_ids: Vec<i64> = estimated.iter().map(|d| d.id).collect();
    let merges: Vec<&Merge> = g.merges.iter().filter(|m| mine(&m.project)).collect();
    let working: Vec<&(i64, String, f64, f64)> = g.working.iter().filter(|w| mine(&w.1)).collect();
    let spans: f64 = g.spans.iter().filter(|s| mine(&s.0)).map(|s| s.2).sum();
    Ok(json!({
        "tasks": done.iter().filter(|d| d.shipped != "closed").count(),
        "merged": n("merged") + n("no_pr"), "merged_pr": n("merged"), "no_pr": n("no_pr"), "pr_open": n("open"), "closed_pr": n("closed"),
        "human_done": n("human"),
        "prs": merges.len(), "added": merges.iter().map(|m| m.added).sum::<i64>(), "removed": merges.iter().map(|m| m.removed).sum::<i64>(),
        "commits": g.commits.iter().filter(|c| mine(&c.0)).count(),
        "releases": g.releases.iter().filter(|r| mine(&project_name(r.s("project")))).count(),
        "agent_min": mins(working.iter().map(|w| w.3 - w.2).sum()),
        "busy_min": mins(busy(working.iter().map(|w| (w.2, w.3)))),
        "command_min": mins(spans),
        "est_human_min": estimated.iter().filter_map(|d| d.row.i("human_min")).sum::<i64>(),
        "est_agent_min": mins(total_work(app, &est_ids)?),
        "estimated": estimated.len(),
    }))
}

/// No finished task and no agent time: the board has nothing for the stretch.
fn empty(t: &Value) -> bool {
    t["tasks"] == 0 && t["human_done"] == 0 && t["agent_min"].as_f64().unwrap_or(0.0) == 0.0
}

// ------------------------------------------------------------------ the facts

fn by_minutes(m: HashMap<String, f64>) -> Vec<Value> {
    let mut v: Vec<(String, f64)> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v.into_iter().map(|(k, s)| json!({"name": k, "minutes": mins(s)})).collect()
}

fn agent(g: &Gathered) -> Value {
    let mut working: HashMap<String, f64> = HashMap::new();
    for w in &g.working {
        *working.entry(w.1.clone()).or_default() += w.3 - w.2;
    }
    let mut cats: HashMap<String, f64> = HashMap::new();
    let mut by_project: HashMap<String, HashMap<String, f64>> = HashMap::new();
    for (p, c, s) in &g.spans {
        *cats.entry(c.clone()).or_default() += s;
        *by_project.entry(p.clone()).or_default().entry(c.clone()).or_default() += s;
    }
    let names: BTreeSet<&String> = working.keys().chain(by_project.keys()).collect();
    let mut projects: Vec<Value> = names
        .into_iter()
        .map(|p| {
            let c = by_project.get(p).cloned().unwrap_or_default();
            json!({"name": p, "agent_min": mins(working.get(p).copied().unwrap_or(0.0)),
                   "busy_min": mins(busy(g.working.iter().filter(|w| &w.1 == p).map(|w| (w.2, w.3)))),
                   "command_min": mins(c.values().sum()), "categories": by_minutes(c)})
        })
        .collect();
    projects.sort_by(|a, b| b["agent_min"].as_f64().unwrap_or(0.0).total_cmp(&a["agent_min"].as_f64().unwrap_or(0.0)));
    json!({"categories": by_minutes(cats), "projects": projects})
}

fn projects(app: &App, g: &Gathered) -> Result<Vec<Value>> {
    let names: BTreeSet<String> = g
        .done
        .iter()
        .map(|d| d.project.clone())
        .chain(g.merges.iter().map(|m| m.project.clone()))
        .chain(g.commits.iter().map(|c| c.0.clone()))
        .chain(g.releases.iter().map(|r| project_name(r.s("project"))))
        .chain(g.working.iter().map(|w| w.1.clone()))
        .collect();
    let mut out = Vec::new();
    for name in names {
        let mut t = totals(app, g, Some(&name))?;
        t["name"] = json!(name);
        out.push(t);
    }
    out.sort_by(|a, b| b["tasks"].as_i64().cmp(&a["tasks"].as_i64()).then(b["agent_min"].as_f64().unwrap_or(0.0).total_cmp(&a["agent_min"].as_f64().unwrap_or(0.0))));
    Ok(out)
}

/// The goals that finished in the stretch (quality.rs's idea of finished), with what they did.
fn finished_goals(app: &App, all: &[Value], g: &Gathered) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for q in in_range(all, g.from, g.to) {
        let Some(gid) = q["goal"].as_str().and_then(|s| s.strip_prefix('G')).and_then(|s| s.parse::<i64>().ok()) else { continue };
        let Some(goal) = app.db.q1("SELECT * FROM goals WHERE id = ?", p![gid])? else { continue };
        let tasks = crate::shared::counted_tasks(app, gid)?;
        let ids: Vec<i64> = tasks.iter().map(|t| t.id()).collect();
        let list = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let lines = if ids.is_empty() {
            json!({"prs": 0, "added": 0, "removed": 0})
        } else {
            let r = app.db.q1(&format!("SELECT COUNT(DISTINCT pr_num) AS prs, SUM(added) AS added, SUM(removed) AS removed FROM pr_merges WHERE task_id IN ({list})"), p![])?;
            let r = r.unwrap_or_default();
            json!({"prs": r.i0("prs"), "added": r.i0("added"), "removed": r.i0("removed")})
        };
        let started = tasks.iter().filter_map(|t| t.s("started_at")).min();
        out.push(json!({
            "goal": q["goal"], "name": goal.st("name"), "tldr": goal.s("tldr").filter(|s| !s.is_empty()), "project": project_name(goal.s("project")),
            "started_at": started, "finished_at": q["finished_at"], "tasks": ids.len(),
            "human_tasks": tasks.iter().filter(|t| human::is(t)).count(),
            "prs": lines["prs"], "added": lines["added"], "removed": lines["removed"],
            "agent_min": mins(total_work(app, &ids)?), "est_human_min": q["est_min"],
        }));
    }
    Ok(out)
}

fn in_range<'a>(all: &'a [Value], from: f64, to: f64) -> Vec<&'a Value> {
    all.iter().filter(|q| q["finished_at"].as_str().and_then(parse_iso).is_some_and(|t| t >= from && t < to)).collect()
}

/// The stretch's finished tasks (closed PRs left out): by goal, then the loose ones by project, capped.
fn task_lists(app: &App, g: &Gathered) -> Result<Value> {
    let mut goals: BTreeMap<i64, Vec<&Done>> = BTreeMap::new();
    let mut loose: BTreeMap<String, Vec<&Done>> = BTreeMap::new();
    for d in g.done.iter().filter(|d| d.shipped != "closed") {
        match d.row.i("goal_id") {
            Some(gid) => goals.entry(gid).or_default().push(d),
            None => loose.entry(d.project.clone()).or_default().push(d),
        }
    }
    let mut by_goal = Vec::new();
    for (gid, ds) in goals {
        let goal = app.db.q1("SELECT name, project FROM goals WHERE id = ?", p![gid])?.unwrap_or_default();
        by_goal.push(json!({"goal": rf("goal", gid), "name": goal.st("name"), "project": project_name(goal.s("project")),
                            "tasks": ds.iter().map(|d| d.item()).collect::<Vec<_>>()}));
    }
    by_goal.sort_by(|a, b| b["tasks"].as_array().map_or(0, Vec::len).cmp(&a["tasks"].as_array().map_or(0, Vec::len)));
    let mut loose: Vec<Value> = loose
        .into_iter()
        .map(|(p, ds)| json!({"project": p, "count": ds.len(), "more": ds.len().saturating_sub(LOOSE_CAP), "tasks": ds.iter().take(LOOSE_CAP).map(|d| d.item()).collect::<Vec<_>>()}))
        .collect();
    loose.sort_by(|a, b| b["count"].as_i64().cmp(&a["count"].as_i64()));
    Ok(json!({"by_goal": by_goal, "loose": loose}))
}

/// The releases cut in the stretch, each with the tasks it carried.
fn releases(app: &App, g: &Gathered) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for r in &g.releases {
        let tasks = app.db.q("SELECT * FROM tasks WHERE release_id = ? ORDER BY finished_at, id", p![r.id()])?;
        let earlier = app.db.count(
            "SELECT COUNT(*) FROM releases WHERE project = ? AND failed_at IS NULL AND (cut_at < ? OR (cut_at = ? AND id < ?))",
            p![r.st("project"), r.st("cut_at"), r.st("cut_at"), r.id()],
        )?;
        out.push(json!({
            "release": rf("release", r.id()), "project": r.st("project"), "name": r.st("name"), "cut_at": r.s("cut_at"),
            "notes": r.s("notes").filter(|n| !n.trim().is_empty()), "first": earlier == 0,
            "count": tasks.len(), "more": tasks.len().saturating_sub(RELEASE_CAP),
            "tasks": tasks.iter().take(RELEASE_CAP).map(|t| json!({"task": human::task_ref(t), "title": t.st("title"), "summary": t.s("summary").map(|s| one_line(s, 400))})).collect::<Vec<_>>(),
        }));
    }
    Ok(out)
}

/// Time tasks waited on the owner (review.rs's count), with which waits were the owner's own tasks.
fn waiting(app: &App, from: f64, to: f64) -> Result<Value> {
    let mut w = crate::review::owner_wait(app, from, to)?;
    if let Some(tasks) = w["tasks"].as_array_mut() {
        for t in tasks {
            let id = t["task"].as_str().and_then(|s| s.strip_prefix('T')).and_then(|s| s.parse::<i64>().ok());
            let Some(row) = id.map(|i| app.db.q1("SELECT * FROM tasks WHERE id = ?", p![i])).transpose()?.flatten() else { continue };
            t["task"] = json!(human::task_ref(&row));
            t["human"] = json!(human::is(&row));
            t["project"] = json!(project_name(row.s("project")));
        }
    }
    let secs = w["seconds"].as_f64().unwrap_or(0.0);
    Ok(json!({"minutes": mins(secs), "reasons": w["reasons"].as_array().map(|rs| rs.iter().map(|r| json!({"name": r["name"], "minutes": mins(r["seconds"].as_f64().unwrap_or(0.0))})).collect::<Vec<_>>()),
              "tasks": w["tasks"].as_array().map(|ts| ts.iter().map(|t| {
                  let mut t = t.clone();
                  t["minutes"] = json!(mins(t["seconds"].as_f64().unwrap_or(0.0)));
                  if let Some(o) = t.as_object_mut() { o.remove("seconds"); }
                  t
              }).collect::<Vec<_>>())}))
}

/// The plan's usage percentages over the stretch, per window; `null` when no samples were taken.
fn usage(app: &App, from: f64, to: f64, days: &[NaiveDate]) -> Result<Value> {
    let rows = app.db.q("SELECT window, at, pct FROM usage_samples WHERE at >= ? AND at < ? ORDER BY at, id", p![iso(from), iso(to)])?;
    if rows.is_empty() {
        return Ok(Value::Null);
    }
    let mut out = serde_json::Map::new();
    for window in ["five_hour", "seven_day"] {
        let samples: Vec<(f64, f64)> = rows.iter().filter(|r| r.s("window") == Some(window)).filter_map(|r| Some((parse_iso(r.s("at")?)?, r.f("pct")?))).collect();
        if samples.is_empty() {
            continue;
        }
        let peak = samples.iter().map(|s| s.1).fold(0.0, f64::max);
        let avg = samples.iter().map(|s| s.1).sum::<f64>() / samples.len() as f64;
        let by_day: Vec<Value> = days
            .iter()
            .map(|d| {
                let (a, b) = (days::start_of(*d), days::start_of(*d + Duration::days(1)));
                let p = samples.iter().filter(|s| s.0 >= a && s.0 < b).map(|s| s.1).fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))));
                json!({"date": date_str(*d), "peak": p.map(round1)})
            })
            .collect();
        out.insert(window.into(), json!({"peak": round1(peak), "avg": round1(avg), "samples": samples.len(), "by_day": by_day}));
    }
    Ok(Value::Object(out))
}

fn issues(app: &App, from: f64, to: f64) -> Result<Value> {
    let rows = app.db.q("SELECT kind, COUNT(*) AS n FROM issues WHERE created_at >= ? AND created_at < ? GROUP BY kind ORDER BY n DESC, kind", p![iso(from), iso(to)])?;
    Ok(json!({"found": rows.iter().map(|r| r.i0("n")).sum::<i64>(), "kinds": rows.iter().map(|r| json!({"kind": r.st("kind"), "found": r.i0("n")})).collect::<Vec<_>>()}))
}

fn lessons_kept(app: &App, from: f64, to: f64) -> Result<Vec<Value>> {
    Ok(app
        .db
        .q("SELECT * FROM lessons WHERE state = 'kept' AND decided_at >= ? AND decided_at < ? ORDER BY decided_at, id", p![iso(from), iso(to)])?
        .iter()
        .map(|l| json!({"lesson": rf("lesson", l.id()), "kind": l.st("kind"), "project": l.s("project"), "text": l.st("text"), "confirmations": l.i0("confirmations"), "kept_at": l.s("decided_at")}))
        .collect())
}

fn day_list(first: NaiveDate, end: NaiveDate) -> Vec<NaiveDate> {
    let mut out = Vec::new();
    let mut d = first;
    while d < end {
        out.push(d);
        d += Duration::days(1);
    }
    out
}

fn short_totals(app: &App, from: f64, to: f64) -> Result<Value> {
    let t = totals(app, &gather(app, from, to)?, None)?;
    Ok(json!({"tasks": t["tasks"], "merged": t["merged"], "human_done": t["human_done"], "prs": t["prs"], "commits": t["commits"],
              "releases": t["releases"], "agent_min": t["agent_min"]}))
}

/// What each number means, for the page that shows them.
fn labels() -> Value {
    json!({
        "agent_min": "Agent time: every task's working time added up; tasks working side by side each count",
        "busy_min": "Busy hours: time with at least one agent working",
        "command_min": "Command time: the tools agents ran, such as builds, tests and git",
        "merged": "Merged: tasks whose PR merged or that needed no PR",
        "pr_open": "Finished with a PR still open",
        "human_done": "Your own tasks done",
        "releases": "Releases cut",
        "first": "The first release the board recorded for the project",
        "est_human_min": "What the merged tasks would have taken a developer by hand, as estimated",
        "est_agent_min": "Agent time those same tasks took",
        "waiting": "Time tasks waited on you while you were at the Mac in work hours",
        "usage": "Plan usage, in percent of each limit",
    })
}

/// The facts of the `kind` period starting `first`.
pub fn facts(app: &App, kind: &str, first: NaiveDate) -> Result<Value> {
    let kind = kind_of(kind)?;
    let (first, end) = period(kind, first)?;
    let (from, to) = (days::start_of(first), days::start_of(end));
    let g = gather(app, from, to)?;
    let (pfirst, pend) = previous(kind, first)?;
    let (pfrom, pto) = (days::start_of(pfirst), days::start_of(pend));
    let before = totals(app, &gather(app, pfrom, pto)?, None)?;
    let all_goals = quality::goals(app, None)?;
    let goals = finished_goals(app, &all_goals, &g)?;
    let days = day_list(first, end);
    let mut t = totals(app, &g, None)?;
    t["goals_finished"] = json!(goals.len());
    let per_day: Vec<Value> = days
        .iter()
        .map(|d| {
            let (a, b) = (days::start_of(*d), days::start_of(*d + Duration::days(1)));
            let done: Vec<&Done> = g.done.iter().filter(|x| x.at >= a && x.at < b).collect();
            json!({"date": date_str(*d), "tasks": done.iter().filter(|x| x.shipped != "closed").count(), "merged": done.iter().filter(|x| x.merged()).count(),
                   "commits": g.commits.iter().filter(|c| c.1 >= a && c.1 < b).count(),
                   "agent_min": mins(g.working.iter().map(|w| (w.3.min(b) - w.2.max(a)).max(0.0)).sum())})
        })
        .collect();
    let mut f = json!({
        "version": VERSION, "kind": kind,
        "period": {"start": iso(from), "end": iso(to), "first_day": date_str(first), "last_day": date_str(end - Duration::days(1))},
        "labels": labels(),
        "totals": t,
        "before": if empty(&before) { Value::Null } else {
            let mut b = before;
            b["goals_finished"] = json!(in_range(&all_goals, pfrom, pto).len());
            b["first_day"] = json!(date_str(pfirst));
            b["last_day"] = json!(date_str(pend - Duration::days(1)));
            b
        },
        "projects": projects(app, &g)?,
        "goals": goals,
        "tasks": task_lists(app, &g)?,
        "releases": releases(app, &g)?,
        "agent": agent(&g),
        "waiting": waiting(app, from, to)?,
        "usage": usage(app, from, to, &days)?,
        "issues": issues(app, from, to)?,
        "days": per_day,
    });
    if kind == "monthly" {
        let mut weeks = Vec::new();
        let mut m = monday_of(first);
        while m < end {
            let (a, b) = (m.max(first), (m + Duration::days(7)).min(end));
            let mut w = short_totals(app, days::start_of(a), days::start_of(b))?;
            w["first_day"] = json!(date_str(a));
            w["last_day"] = json!(date_str(b - Duration::days(1)));
            weeks.push(w);
            m += Duration::days(7);
        }
        let this = in_range(&all_goals, from, to);
        let prev = in_range(&all_goals, pfrom, pto);
        f["weeks"] = json!(weeks);
        f["quality"] = json!({"this": quality::sum(&this), "before": if prev.is_empty() { Value::Null } else { quality::sum(&prev) }});
        f["lessons"] = json!(lessons_kept(app, from, to)?);
    }
    Ok(f)
}

// ------------------------------------------------------------------ keeping and reading recaps

/// Makes and keeps the recap of the `kind` period holding `day`, once: a period that already has
/// one keeps it. Returns its id.
pub fn save(app: &App, kind: &str, day: NaiveDate) -> Result<i64> {
    let kind = kind_of(kind)?;
    let (first, end) = period(kind, day)?;
    let start = iso(days::start_of(first));
    if let Some(r) = app.db.q1("SELECT id FROM recaps WHERE kind = ? AND period_start = ?", p![kind, start])? {
        return Ok(r.id());
    }
    let f = facts(app, kind, first)?;
    app.db.x(
        "INSERT INTO recaps(kind, period_start, period_end, created_at, facts) VALUES(?, ?, ?, ?, ?)",
        p![kind, start, iso(days::start_of(end)), now_iso(), jdumps(&f)],
    )
}

/// Makes the recap of the `kind` period holding `day`: its facts are kept first, then its page is
/// written (on the AI queue when Claude is there). A recap that already has a page keeps it unless
/// `again`. Returns its id.
pub fn make(app: &App, kind: &str, day: NaiveDate, again: bool) -> Result<i64> {
    let id = app.db.tx(|| save(app, kind, day))?;
    let status = app.db.q1("SELECT writing_status FROM recaps WHERE id = ?", p![id])?.map(|r| r.st("writing_status")).unwrap_or_default();
    if again || matches!(status.as_str(), "none" | "failed") {
        crate::recap_page::queue(app, id)?;
    }
    Ok(id)
}

/// The day a `POST /recaps` names: `2026-11-02`, or `2026-10` for a month.
pub fn parse_day(s: &str) -> Result<NaiveDate> {
    let s = s.trim();
    let full = if s.len() == 7 { format!("{s}-01") } else { s.to_string() };
    NaiveDate::parse_from_str(&full, "%Y-%m-%d").or_else(|_| err(400, "Give the day as YYYY-MM-DD, or a month as YYYY-MM."))
}

/// The Needs you key a recap's alert uses.
pub fn key(id: i64) -> String {
    format!("recap:{id}")
}

/// The id a recap's notification goes out under.
pub fn notification_id(id: i64) -> String {
    format!("taskboard-recap-{id}")
}

fn card(r: &Row) -> Value {
    let facts: Value = r.s("facts").and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
    let t = &facts["totals"];
    json!({"id": r.id(), "kind": r.s("kind"), "period_start": r.s("period_start"), "period_end": r.s("period_end"),
           "first_day": facts["period"]["first_day"], "last_day": facts["period"]["last_day"],
           "created_at": r.s("created_at"), "seen_at": r.s("seen_at"), "writing_status": r.st("writing_status"),
           "tasks": t["tasks"], "merged": t["merged"], "prs": t["prs"], "releases": t["releases"], "agent_min": t["agent_min"]})
}

/// Every kept recap, the latest period first.
pub fn list(app: &App) -> Result<Value> {
    let rows = app.db.q("SELECT * FROM recaps ORDER BY period_start DESC, kind DESC, id DESC", p![])?;
    Ok(json!({"recaps": rows.iter().map(card).collect::<Vec<_>>()}))
}

pub fn get(app: &App, id: i64) -> Result<Value> {
    let Some(r) = app.db.q1("SELECT * FROM recaps WHERE id = ?", p![id])? else { return err(404, "There's no recap with that id.") };
    let mut v = card(&r);
    v["facts"] = r.s("facts").and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
    v["writing"] = r.s("writing").and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
    Ok(v)
}

/// The owner opened recap `id`: its Needs you item and notification go.
pub fn seen(app: &App, id: i64) -> Result<Value> {
    get(app, id)?;
    app.db.x("UPDATE recaps SET seen_at = COALESCE(seen_at, ?) WHERE id = ?", p![now_iso(), id])?;
    dispatch::clear_alert_key(app, &key(id))?;
    dispatch::withdraw_id(app, &notification_id(id));
    get(app, id)
}

// ------------------------------------------------------------------ the schedule

/// `[recap]` in config.toml: `weekly_day` and `at` are the Mac's local time; `monthly_day` is 1 to 28.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RecapConfig {
    pub on: bool,
    pub weekly_day: String,
    pub monthly_day: u32,
    pub at: String,
}

impl Default for RecapConfig {
    fn default() -> Self {
        RecapConfig { on: true, weekly_day: "monday".into(), monthly_day: 1, at: "9:00 AM".into() }
    }
}

const TICK_SECS: f64 = 60.0;
/// The first day of the last `kind` period the schedule made, per kind.
const LAST_KEY: &str = "recap_last_";
/// Recaps the schedule made whose notification waits for their page.
const PENDING_KEY: &str = "recap_announce";

pub fn run(app: std::sync::Arc<App>) {
    if let Err(e) = rewrite_stale(&app) {
        app.info(format!("recap: {}", e.message));
    }
    while !app.stopping() {
        if let Err(e) = tick(&app, hours::now_local()) {
            app.info(format!("recap: {}", e.message));
        }
        app.sleep(TICK_SECS);
    }
}

/// A page still `writing` when the board starts lost its place on the AI queue when the board
/// stopped: it's written again.
fn rewrite_stale(app: &App) -> Result<()> {
    for r in app.db.q("SELECT id FROM recaps WHERE writing_status = 'writing'", p![])? {
        app.db.x("UPDATE recaps SET writing_status = 'none' WHERE id = ?", p![r.id()])?;
        crate::recap_page::queue(app, r.id())?;
    }
    Ok(())
}

fn at_time(cfg: &RecapConfig) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(&hours::parse_time(&cfg.at, "recap").ok()?, "%H:%M").ok()
}

/// The latest moment at or before `now` the `kind` recap is due, if the schedule is on.
pub fn due_at(cfg: &RecapConfig, kind: &str, now: NaiveDateTime) -> Option<NaiveDateTime> {
    if !cfg.on {
        return None;
    }
    let at = at_time(cfg)?;
    let today = now.date();
    let day = match kind {
        "weekly" => {
            let wd = crate::config::parse_weekday(&cfg.weekly_day)?;
            today - Duration::days(((today.weekday().num_days_from_monday() + 7 - wd.num_days_from_monday()) % 7) as i64)
        }
        "monthly" => today.with_day(cfg.monthly_day.clamp(1, 28))?,
        _ => return None,
    };
    let when = day.and_time(at);
    if when <= now {
        return Some(when);
    }
    let earlier = match kind {
        "weekly" => day - Duration::days(7),
        _ => (day.with_day(1)? - Duration::days(1)).with_day(day.day())?,
    };
    Some(earlier.and_time(at))
}

/// Whether a task finished in the `kind` period starting `first` (one whose PR was closed unmerged doesn't count).
pub fn finished_work(app: &App, kind: &str, first: NaiveDate) -> Result<bool> {
    let (first, end) = period(kind, first)?;
    Ok(done_tasks(app, days::start_of(first), days::start_of(end))?.iter().any(|d| d.shipped != "closed"))
}

fn pending(app: &App) -> Result<Vec<i64>> {
    Ok(app.db.get_setting(PENDING_KEY)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
}

fn set_pending(app: &App, ids: &[i64]) -> Result<()> {
    app.db.set_setting(PENDING_KEY, if ids.is_empty() { None } else { Some(jdumps(&json!(ids))) }.as_deref())
}

/// Makes each recap that's due and hasn't been made (once, however late the board wakes), then
/// sends the notification of each whose page is written or failed. Returns the recaps it made.
pub fn tick(app: &App, now: NaiveDateTime) -> Result<Vec<i64>> {
    let mut made = Vec::new();
    for kind in KINDS {
        let Some(when) = due_at(&app.cfg.recap, kind, now) else { continue };
        let (first, _) = last_complete(kind, when.date())?;
        let key = format!("{LAST_KEY}{kind}");
        let start = date_str(first);
        if app.db.get_setting(&key)?.as_deref() >= Some(start.as_str()) {
            continue;
        }
        app.db.set_setting(&key, Some(&start))?;
        if !finished_work(app, kind, first)? {
            continue;
        }
        let id = make(app, kind, first, false)?;
        let mut ids = pending(app)?;
        if !ids.contains(&id) {
            ids.push(id);
            set_pending(app, &ids)?;
        }
        made.push(id);
    }
    announce_ready(app)?;
    Ok(made)
}

/// Sends the notification of each waiting recap whose page is written or failed; one the owner
/// already opened, or that's gone, needs none.
fn announce_ready(app: &App) -> Result<()> {
    let ids = pending(app)?;
    if ids.is_empty() {
        return Ok(());
    }
    let mut left = Vec::new();
    for id in ids {
        let Some(r) = app.db.q1("SELECT writing_status, seen_at FROM recaps WHERE id = ?", p![id])? else { continue };
        if r.s("seen_at").is_some() {
            continue;
        }
        if matches!(r.st("writing_status").as_str(), "done" | "failed") {
            announce(app, id)?;
        } else {
            left.push(id);
        }
    }
    set_pending(app, &left)
}

fn month_day(d: NaiveDate) -> String {
    format!("{} {}", d.format("%b"), d.day())
}

/// The notification for recap `id`: "Weekly recap: Nov 2 – 8", "12 tasks finished", opening the recap.
pub fn notice(app: &App, id: i64) -> Result<Value> {
    let r = get(app, id)?;
    let day = |k: &str| r[k].as_str().and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
    let (Some(first), Some(last)) = (day("first_day"), day("last_day")) else { return err(500, "The recap has no period.") };
    let title = if r["kind"] == "monthly" {
        format!("Monthly recap: {}", first.format("%B %Y"))
    } else if first.month() == last.month() {
        format!("Weekly recap: {} – {}", month_day(first), last.day())
    } else {
        format!("Weekly recap: {} – {}", month_day(first), month_day(last))
    };
    let n = r["tasks"].as_i64().unwrap_or(0);
    let mut body = format!("{n} {} finished", if n == 1 { "task" } else { "tasks" });
    if r["writing_status"] == "failed" {
        body.push_str(", numbers only");
    }
    let url = dispatch::page_link(app, &format!("recap/{id}"));
    Ok(json!({"title": title, "body": body, "category": "board", "sound": true, "open": url, "id": notification_id(id)}))
}

/// Tells the owner recap `id` is ready with a notification that opens it.
pub fn announce(app: &App, id: i64) -> Result<()> {
    let params = notice(app, id)?;
    if app.cfg.notify && app.cfg.runner {
        let m = app.cfg.midna.clone();
        std::thread::spawn(move || {
            let _ = midna::notify_send(&m, &params);
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(d: &str, t: &str) -> NaiveDateTime {
        NaiveDateTime::new(NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap(), NaiveTime::parse_from_str(t, "%H:%M").unwrap())
    }

    #[test]
    fn the_week_is_due_monday_at_nine() {
        let c = RecapConfig::default();
        assert_eq!(due_at(&c, "weekly", at("2026-11-09", "09:00")), Some(at("2026-11-09", "09:00")));
        assert_eq!(due_at(&c, "weekly", at("2026-11-09", "08:59")), Some(at("2026-11-02", "09:00")));
        assert_eq!(due_at(&c, "weekly", at("2026-11-14", "12:00")), Some(at("2026-11-09", "09:00")));
        let off = RecapConfig { on: false, ..RecapConfig::default() };
        assert_eq!(due_at(&off, "weekly", at("2026-11-09", "10:00")), None);
        let wed = RecapConfig { weekly_day: "wed".into(), at: "5pm".into(), ..RecapConfig::default() };
        assert_eq!(due_at(&wed, "weekly", at("2026-11-14", "12:00")), Some(at("2026-11-11", "17:00")));
    }

    #[test]
    fn the_month_is_due_on_the_first_at_nine() {
        let c = RecapConfig::default();
        assert_eq!(due_at(&c, "monthly", at("2026-11-01", "09:00")), Some(at("2026-11-01", "09:00")));
        assert_eq!(due_at(&c, "monthly", at("2026-11-01", "08:00")), Some(at("2026-10-01", "09:00")));
        assert_eq!(due_at(&c, "monthly", at("2026-01-01", "08:00")), Some(at("2025-12-01", "09:00")), "across the year");
        let late = RecapConfig { monthly_day: 31, ..RecapConfig::default() };
        assert_eq!(due_at(&late, "monthly", at("2026-03-10", "08:00")), Some(at("2026-02-28", "09:00")), "past 28 runs on the 28th");
    }
}
