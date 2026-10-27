//! The Days page: what each day looked like, and how long the board keeps it.
//!
//! A day is worked out from `task_states` (every status change, kept by a trigger), the task log
//! (`events`: commits, linked PRs, questions) and the backlog (issues found). Each past day is
//! worked out once and kept in `day_stats`, one row per project, so the week views and older days
//! still have their bars and totals after the cleanup has removed the events behind them. Today is
//! worked out on every call.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone, Timelike};
use serde_json::{json, Map, Value};

use crate::app::App;
use crate::util::*;
use crate::hours::{self, weekday_key};
use crate::{clock, p};

/// Ten-minute slots in a day, for "how many terminals were working".
pub const SLOTS: usize = 144;
/// The project of a task that has none.
pub const NO_PROJECT: &str = "Other";
/// The `day_stats` row that marks a day as worked out when nothing happened in it.
const EMPTY_ROW: &str = "";

const DETAIL_KEY: &str = "history_detail_days";
const SUMMARY_KEY: &str = "history_summary_days";
const LAST_CLEANUP_KEY: &str = "history_last_cleanup";
pub const DETAIL_DAYS: i64 = 90;
pub const SUMMARY_DAYS: i64 = 365;
pub const DETAIL_CHOICES: &[i64] = &[30, 90, 180, 365];
/// `0` keeps day summaries forever.
pub const SUMMARY_CHOICES: &[i64] = &[180, 365, 730, 0];

// ------------------------------------------------------------------ dates

pub fn today() -> NaiveDate {
    local_now().date_naive()
}

pub fn start_of(d: NaiveDate) -> f64 {
    let naive = d.and_hms_opt(0, 0, 0).unwrap_or_default();
    Local.from_local_datetime(&naive).earliest().map(|t| t.timestamp() as f64).unwrap_or(0.0)
}

fn bounds(d: NaiveDate) -> (f64, f64) {
    (start_of(d), start_of(d + Duration::days(1)))
}

fn date_str(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

pub fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

/// The first day of `d`'s week, for weeks that start on `first` (config.toml's `first_weekday`).
pub fn week_start(d: NaiveDate, first: chrono::Weekday) -> NaiveDate {
    let back = (d.weekday().num_days_from_monday() + 7 - first.num_days_from_monday()) % 7;
    d - Duration::days(back as i64)
}

fn mins(secs: f64) -> f64 {
    (secs / 60.0 * 10.0).round() / 10.0
}

// ------------------------------------------------------------------ working out a day

/// A stretch of one task in one status, clipped to the day.
#[derive(Clone, Debug)]
struct Seg {
    task: i64,
    from: f64,
    to: f64,
    /// `working` or `needs`.
    status: &'static str,
    /// For `working`: how it ended (`done`, `failed`, `lost`, `stopped`, or `working` while it
    /// goes on or the task stopped to ask). For `needs`: why it waits.
    end: String,
    /// The whole wait, not clipped to the day, for `needs`: only the time it counts (see `away`).
    whole: f64,
    open: bool,
    /// For `needs`: the stretch cut where the owner was away, each piece with whether it was away.
    pieces: Vec<(f64, f64, bool)>,
}

/// The furthest back a wait's away time is worked out from.
const AWAY_BACK_SECS: f64 = 14.0 * 86400.0;

/// The stretches in `[from, to)` that don't count as waiting on the owner, merged and in order: the
/// Mac asleep, the owner away from it (no input), and outside the work hours when they're on.
fn away(app: &App, from: f64, to: f64) -> Vec<(f64, f64)> {
    let from = from.max(to - AWAY_BACK_SECS);
    let mut v = clock::away_spans(from, to);
    let h = hours::get(app);
    if h.on {
        let mut start: Option<f64> = None;
        let mut m = (from / 60.0).floor() * 60.0;
        while m < to {
            let off = !hours::within(&h, &local_dt(m).naive_local());
            match (off, start) {
                (true, None) => start = Some(m.max(from)),
                (false, Some(a)) => {
                    v.push((a, m));
                    start = None;
                }
                _ => {}
            }
            m += 60.0;
        }
        if let Some(a) = start {
            v.push((a, to));
        }
    }
    clock::merge(v)
}

/// `[a, b)` cut by `gaps` (merged, in order) into pieces, each with whether it lies in a gap.
fn cut(a: f64, b: f64, gaps: &[(f64, f64)]) -> Vec<(f64, f64, bool)> {
    let mut out = Vec::new();
    let mut at = a;
    for &(g0, g1) in gaps {
        let (g0, g1) = (g0.max(a), g1.min(b));
        if g1 <= g0 {
            continue;
        }
        if g0 > at {
            out.push((at, g0, false));
        }
        out.push((g0, g1, true));
        at = g1;
    }
    if b > at {
        out.push((at, b, false));
    }
    out
}

/// Seconds of `[a, b)` outside `gaps`.
fn counted(a: f64, b: f64, gaps: &[(f64, f64)]) -> f64 {
    cut(a, b, gaps).iter().filter(|p| !p.2).map(|p| p.1 - p.0).sum()
}

struct Task {
    title: String,
    project: String,
    human_min: Option<i64>,
}

/// Every working and waiting stretch that overlaps `[from, to)`, clipped to it and to `now`.
fn segments(app: &App, from: f64, to: f64, now: f64) -> Result<Vec<Seg>> {
    let (f, t) = (iso(from), iso(to));
    let rows = app.db.q(
        "SELECT * FROM task_states WHERE at < ?2 AND task_id IN (
           SELECT task_id FROM task_states WHERE at >= ?1 AND at < ?2
           UNION
           SELECT s.task_id FROM task_states s WHERE s.at < ?1 AND s.status IN ('working', 'needs')
             AND s.id = (SELECT x.id FROM task_states x WHERE x.task_id = s.task_id AND x.at < ?1 ORDER BY x.at DESC, x.id DESC LIMIT 1))
         ORDER BY task_id, at, id",
        p![f, t],
    )?;
    let ended: Vec<(i64, f64)> = app
        .db
        .q("SELECT task_id, at FROM events WHERE kind = 'status' AND text LIKE 'Terminal ended%' AND at >= ? AND at < ?", p![iso(from - 60.0), iso(to + 60.0)])?
        .iter()
        .filter_map(|e| Some((e.i("task_id")?, parse_iso(e.s("at")?)?)))
        .collect();
    let mut by_task: BTreeMap<i64, Vec<&Row>> = BTreeMap::new();
    for r in &rows {
        by_task.entry(r.i0("task_id")).or_default().push(r);
    }
    let mut out = Vec::new();
    for (task, states) in by_task {
        for (i, s) in states.iter().enumerate() {
            let status = match s.s("status") {
                Some("working") => "working",
                Some("needs") => "needs",
                _ => continue,
            };
            let Some(start) = s.s("at").and_then(parse_iso) else { continue };
            let next = states.get(i + 1);
            let stop = next.and_then(|n| n.s("at")).and_then(parse_iso).unwrap_or(now);
            let open = next.is_none();
            let end = if status == "needs" {
                s.st("needs_reason")
            } else {
                match next {
                    None => "working".into(),
                    Some(n) => match n.s("status") {
                        Some("done") if n.b("failed") => "failed".into(),
                        Some("done") => "done".into(),
                        Some("needs") if n.s("needs_reason") == Some("lost") => "lost".into(),
                        Some("needs") | Some("working") => "working".into(),
                        _ if ended.iter().any(|(t, at)| *t == task && (at - stop).abs() < 30.0) => "lost".into(),
                        _ => "stopped".into(),
                    },
                }
            };
            // A stretch that goes on into the next day hasn't ended yet, as far as this day goes.
            let end = if status == "working" && stop > to { "working".into() } else { end };
            let (a, b) = (start.max(from), stop.min(to).min(now));
            if b > a {
                let (whole, pieces) = if status == "needs" {
                    let gaps = away(app, start, stop.min(now));
                    (counted(start, stop.min(now), &gaps), cut(a, b, &gaps))
                } else {
                    (stop.min(now) - start, Vec::new())
                };
                out.push(Seg { task, from: a, to: b, status, end, whole, open, pieces });
            }
        }
    }
    Ok(out)
}

/// Minutes each task has spent working, ever (for "how much faster than by hand").
fn total_work(app: &App, tasks: &[i64], now: f64) -> Result<HashMap<i64, f64>> {
    let mut out = HashMap::new();
    for &t in tasks {
        let states = app.db.q("SELECT at, status FROM task_states WHERE task_id = ? ORDER BY at, id", p![t])?;
        let mut sum = 0.0;
        for (i, s) in states.iter().enumerate() {
            if s.s("status") != Some("working") {
                continue;
            }
            let a = s.s("at").and_then(parse_iso).unwrap_or(now);
            let b = states.get(i + 1).and_then(|n| n.s("at")).and_then(parse_iso).unwrap_or(now);
            sum += (b - a).max(0.0);
        }
        out.insert(t, mins(sum));
    }
    Ok(out)
}

fn tasks_by_id(app: &App, ids: impl IntoIterator<Item = i64>) -> Result<HashMap<i64, Task>> {
    let ids: Vec<i64> = ids.into_iter().collect::<BTreeSet<_>>().into_iter().collect();
    let mut out = HashMap::new();
    for chunk in ids.chunks(400) {
        let qs = vec!["?"; chunk.len()].join(", ");
        let rows = app.db.q(&format!("SELECT id, title, project, human_min FROM tasks WHERE id IN ({qs})"), chunk.iter().map(|i| json!(i)).collect())?;
        for r in rows {
            let project = r.s("project").filter(|p| !p.trim().is_empty()).unwrap_or(NO_PROJECT).to_string();
            out.insert(r.id(), Task { title: r.st("title"), project, human_min: r.i("human_min") });
        }
    }
    Ok(out)
}

/// A timeline mark: a commit, a linked PR, a question, a finished task or an issue found.
#[derive(Clone, Debug)]
struct Mark {
    at: f64,
    kind: &'static str,
    task: i64,
    text: String,
}

fn marks(app: &App, from: f64, to: f64) -> Result<Vec<Mark>> {
    let mut out = Vec::new();
    for e in app.db.q(
        "SELECT task_id, at, kind, text FROM events WHERE at >= ? AND at < ? AND (kind IN ('commit', 'question') OR (kind = 'status' AND text LIKE 'Linked PR #%')) ORDER BY at, id",
        p![iso(from), iso(to)],
    )? {
        let (Some(task), Some(at)) = (e.i("task_id"), e.s("at").and_then(parse_iso)) else { continue };
        let text = e.st("text");
        let kind = match e.s("kind") {
            Some("commit") if text.starts_with("Committed") => "commit",
            Some("commit") => continue,
            Some("question") => "question",
            _ => "pr",
        };
        out.push(Mark { at, kind, task, text });
    }
    for i in app.db.q(
        "SELECT id, title, found_by_task, created_at FROM issues WHERE found_by_task IS NOT NULL AND created_at >= ? AND created_at < ?",
        p![iso(from), iso(to)],
    )? {
        let (Some(task), Some(at)) = (i.i("found_by_task"), i.s("created_at").and_then(parse_iso)) else { continue };
        out.push(Mark { at, kind: "found", task, text: format!("{} · {}", rf("issue", i.id()), i.st("title")) });
    }
    Ok(out)
}

/// One project's day, as kept in `day_stats`.
fn work_out(app: &App, date: NaiveDate, now: f64) -> Result<(BTreeMap<String, Value>, Vec<Mark>)> {
    let (from, to) = bounds(date);
    let segs = segments(app, from, to, now)?;
    let mks = marks(app, from, to)?;
    let finished: Vec<(i64, f64)> = app
        .db
        .q("SELECT task_id, at FROM task_states WHERE status = 'done' AND failed = 0 AND at >= ? AND at < ? ORDER BY at, id", p![iso(from), iso(to)])?
        .iter()
        .filter_map(|r| Some((r.i("task_id")?, parse_iso(r.s("at")?)?)))
        .collect();
    let done_ids: Vec<i64> = finished.iter().map(|(t, _)| *t).collect::<BTreeSet<_>>().into_iter().collect();
    let tasks = tasks_by_id(app, segs.iter().map(|s| s.task).chain(mks.iter().map(|m| m.task)).chain(done_ids.iter().copied()))?;
    let estimated: Vec<i64> = done_ids.iter().copied().filter(|t| tasks.get(t).is_some_and(|x| x.human_min.is_some())).collect();
    let work = total_work(app, &estimated, now)?;
    let project = |t: i64| tasks.get(&t).map(|x| x.project.clone()).unwrap_or_else(|| NO_PROJECT.to_string());

    #[derive(Default)]
    struct P {
        agent: f64,
        human: f64,
        est_agent: f64,
        done: i64,
        prs: i64,
        commits: i64,
        waits: Vec<Value>,
        running: Vec<i64>,
        hourly: Vec<i64>,
        tasks: BTreeMap<i64, (f64, f64, String)>,
        bars: Vec<Value>,
        finished: Vec<Value>,
    }
    fn slot(ps: &mut BTreeMap<String, P>, name: String) -> &mut P {
        ps.entry(name).or_insert_with(|| P { running: vec![0; SLOTS], hourly: vec![0; 24], ..Default::default() })
    }
    let mut ps: BTreeMap<String, P> = BTreeMap::new();
    for s in &segs {
        let p = slot(&mut ps, project(s.task));
        let entry = p.tasks.entry(s.task).or_insert((0.0, 0.0, String::new()));
        if s.status == "working" {
            p.agent += s.to - s.from;
            entry.0 += s.to - s.from;
            for k in 0..SLOTS {
                let mid = from + (k as f64 + 0.5) * 600.0;
                if s.from <= mid && mid < s.to {
                    p.running[k] += 1;
                }
            }
            entry.2 = s.end.clone();
        } else {
            entry.1 += s.pieces.iter().filter(|x| !x.2).map(|x| x.1 - x.0).sum::<f64>();
            entry.2 = "needs".into();
            if s.from >= from {
                p.waits.push(json!({"task": s.task, "ref": rf("task", s.task), "at": iso(s.from), "min": mins(s.whole), "open": s.open, "reason": s.end}));
            }
        }
        let title = tasks.get(&s.task).map(|t| t.title.clone()).unwrap_or_default();
        let bar = |a: f64, b: f64, kind: &str, open: bool| {
            json!({"task": s.task, "ref": rf("task", s.task), "title": title, "from": iso(a), "to": iso(b), "kind": kind, "open": open})
        };
        if s.status == "needs" {
            // Waits while the owner was away draw as `away`, and don't count.
            let last = s.pieces.len().saturating_sub(1);
            for (i, &(a, b, gone)) in s.pieces.iter().enumerate() {
                p.bars.push(bar(a, b, if gone { "away" } else { "needs" }, s.open && i == last));
            }
        } else {
            p.bars.push(bar(s.from, s.to, s.end.as_str(), s.open));
        }
    }
    for &(t, at) in &finished {
        let title = tasks.get(&t).map(|x| x.title.clone()).unwrap_or_default();
        let p = slot(&mut ps, project(t));
        p.finished.push(json!({"task": t, "ref": rf("task", t), "title": title, "at": iso(at)}));
        p.tasks.entry(t).or_insert((0.0, 0.0, String::new())).2 = "done".into();
    }
    for &t in &done_ids {
        let p = slot(&mut ps, project(t));
        p.done += 1;
        if let (Some(h), Some(w)) = (tasks.get(&t).and_then(|x| x.human_min), work.get(&t)) {
            p.human += h as f64;
            p.est_agent += w;
        }
    }
    for m in &mks {
        let p = slot(&mut ps, project(m.task));
        match m.kind {
            "commit" => p.commits += 1,
            "pr" => p.prs += 1,
            _ => {}
        }
        let h = local_dt(m.at).hour() as usize;
        p.hourly[h.min(23)] += 1;
    }
    let mut out = BTreeMap::new();
    for (name, p) in ps {
        let tasks_json: Vec<Value> = p
            .tasks
            .iter()
            .map(|(id, (w, wait, state))| {
                json!({"task": id, "ref": rf("task", *id), "title": tasks.get(id).map(|t| t.title.clone()).unwrap_or_default(),
                       "work_min": mins(*w), "wait_min": mins(*wait), "state": state})
            })
            .collect();
        out.insert(
            name,
            json!({"agent_min": mins(p.agent), "human_min": p.human, "est_agent_min": p.est_agent, "done": p.done, "prs": p.prs,
                   "commits": p.commits, "waits": p.waits, "running": p.running, "hourly": p.hourly, "tasks": tasks_json, "bars": p.bars,
                   "finished": p.finished}),
        );
    }
    Ok((out, mks))
}

/// A day's rows by project: worked out now for today, read back (or worked out once and kept) for
/// a past day, empty for a day still to come.
fn day(app: &App, date: NaiveDate, cache: &mut HashMap<NaiveDate, BTreeMap<String, Value>>) -> Result<BTreeMap<String, Value>> {
    if let Some(v) = cache.get(&date) {
        return Ok(v.clone());
    }
    let today = today();
    let v = if date > today {
        BTreeMap::new()
    } else if date == today {
        work_out(app, date, now_ts())?.0
    } else {
        stored(app, date)?
    };
    cache.insert(date, v.clone());
    Ok(v)
}

fn stored(app: &App, date: NaiveDate) -> Result<BTreeMap<String, Value>> {
    let key = date_str(date);
    let rows = app.db.q("SELECT project, data FROM day_stats WHERE date = ?", p![key])?;
    if !rows.is_empty() {
        return Ok(rows
            .iter()
            .filter(|r| r.s("project") != Some(EMPTY_ROW))
            .map(|r| (r.st("project"), serde_json::from_str(r.s("data").unwrap_or("{}")).unwrap_or(json!({}))))
            .collect());
    }
    let (v, _) = work_out(app, date, now_ts())?;
    app.db.tx(|| {
        if v.is_empty() {
            app.db.x("INSERT OR REPLACE INTO day_stats(date, project, data, at) VALUES(?, ?, '{}', ?)", p![key, EMPTY_ROW, now_iso()])?;
        }
        for (proj, data) in &v {
            app.db.x("INSERT OR REPLACE INTO day_stats(date, project, data, at) VALUES(?, ?, ?, ?)", p![key, proj, jdumps(data), now_iso()])?;
        }
        Ok(())
    })?;
    Ok(v)
}

// ------------------------------------------------------------------ adding projects up

fn f(v: &Value, k: &str) -> f64 {
    v[k].as_f64().unwrap_or(0.0)
}

#[derive(Default, Clone)]
struct Sum {
    agent: f64,
    human: f64,
    est_agent: f64,
    done: f64,
    prs: f64,
    commits: f64,
    wait: f64,
    questions: f64,
    running: Vec<i64>,
    hourly: Vec<i64>,
    by_project: Map<String, Value>,
}

fn sum(rows: &BTreeMap<String, Value>, hide: &HashSet<String>) -> Sum {
    let mut s = Sum { running: vec![0; SLOTS], hourly: vec![0; 24], ..Default::default() };
    for (proj, v) in rows {
        if hide.contains(proj) {
            continue;
        }
        s.agent += f(v, "agent_min");
        s.human += f(v, "human_min");
        s.est_agent += f(v, "est_agent_min");
        s.done += f(v, "done");
        s.prs += f(v, "prs");
        s.commits += f(v, "commits");
        for w in v["waits"].as_array().into_iter().flatten() {
            s.wait += f(w, "min");
            s.questions += 1.0;
        }
        for (k, n) in v["running"].as_array().into_iter().flatten().enumerate().take(SLOTS) {
            s.running[k] += n.as_i64().unwrap_or(0);
        }
        for (k, n) in v["hourly"].as_array().into_iter().flatten().enumerate().take(24) {
            s.hourly[k] += n.as_i64().unwrap_or(0);
        }
        s.by_project.insert(proj.clone(), json!(f(v, "agent_min")));
    }
    s
}

fn totals(s: &Sum) -> Value {
    let peak = s.running.iter().copied().max().unwrap_or(0);
    json!({"agent_min": s.agent, "human_min": s.human, "est_agent_min": s.est_agent, "done": s.done, "prs": s.prs,
           "commits": s.commits, "wait_min": (s.wait * 10.0).round() / 10.0, "questions": s.questions, "peak": peak,
           "by_project": s.by_project})
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let m = v.len() / 2;
    if v.len() % 2 == 1 {
        v[m]
    } else {
        (v[m - 1] + v[m]) / 2.0
    }
}

// ------------------------------------------------------------------ GET /days

/// `GET /days?date=YYYY-MM-DD&hide=a,b`: everything the Days page shows for one day, its week and
/// the week before.
pub fn page(app: &App, date: Option<&str>, hide: Option<&str>) -> Result<Value> {
    let today = today();
    let date = match date.filter(|d| !d.trim().is_empty()) {
        Some(d) => parse_date(d).ok_or_else(|| ApiError::new(400, "Give the day as YYYY-MM-DD."))?,
        None => today,
    }
    .min(today);
    let hide: HashSet<String> = hide.unwrap_or("").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    let mut cache = HashMap::new();
    let now = now_ts();

    // The day itself, with the timeline's marks while the events are still kept.
    let (rows, mks) = if date == today {
        let (r, m) = work_out(app, date, now)?;
        cache.insert(date, r.clone());
        (r, m)
    } else {
        let r = day(app, date, &mut cache)?;
        let (from, to) = bounds(date);
        (r, marks(app, from, to)?)
    };
    let mut projects: BTreeSet<String> = rows.keys().cloned().collect();

    let mon = week_start(date, app.cfg.first_weekday);
    let mut week = Vec::new();
    let mut last_week = Vec::new();
    let mut week_tasks: BTreeMap<i64, Value> = BTreeMap::new();
    for k in 0..7 {
        for (list, d) in [(&mut week, mon + Duration::days(k)), (&mut last_week, mon + Duration::days(k - 7))] {
            let r = day(app, d, &mut cache)?;
            projects.extend(r.keys().cloned());
            let s = sum(&r, &hide);
            let mut t = totals(&s);
            t["date"] = json!(date_str(d));
            t["future"] = json!(d > today);
            list.push(t);
        }
        // How long tasks took this week: each task's work and waits, added up over its days.
        let d = mon + Duration::days(k);
        for (proj, v) in day(app, d, &mut cache)? {
            if hide.contains(&proj) {
                continue;
            }
            for t in v["tasks"].as_array().into_iter().flatten() {
                let id = t["task"].as_i64().unwrap_or(0);
                let e = week_tasks.entry(id).or_insert_with(|| json!({"task": id, "ref": t["ref"], "title": t["title"], "project": proj, "work_min": 0.0, "wait_min": 0.0}));
                e["work_min"] = json!(f(e, "work_min") + f(t, "work_min"));
                e["wait_min"] = json!(f(e, "wait_min") + f(t, "wait_min"));
                e["state"] = t["state"].clone();
                e["date"] = json!(date_str(d));
            }
        }
    }
    let mut week_tasks: Vec<Value> = week_tasks.into_values().collect();
    week_tasks.sort_by(|a, b| (f(b, "work_min") + f(b, "wait_min")).partial_cmp(&(f(a, "work_min") + f(a, "wait_min"))).unwrap_or(std::cmp::Ordering::Equal));
    let finished: Vec<f64> = week_tasks.iter().filter(|t| t["state"] == "done").map(|t| f(t, "work_min") + f(t, "wait_min")).collect();
    week_tasks.truncate(12);

    // The usual: the 14 days before, counting only days with work.
    let mut usual_days = Vec::new();
    let mut usual_waits = Vec::new();
    for k in 1..=14 {
        let r = day(app, date - Duration::days(k), &mut cache)?;
        let s = sum(&r, &hide);
        if s.agent > 0.0 {
            usual_days.push(s);
        }
        for (proj, v) in &r {
            if !hide.contains(proj) {
                usual_waits.extend(v["waits"].as_array().into_iter().flatten().map(|w| f(w, "min")));
            }
        }
    }
    let med = |g: fn(&Sum) -> f64| median(usual_days.iter().map(g).collect());
    let usual = json!({
        "days": usual_days.len(),
        "agent_min": med(|s| s.agent), "done": med(|s| s.done), "prs": med(|s| s.prs),
        "wait_min": med(|s| s.wait), "human_min": med(|s| s.human), "wait_each": median(usual_waits),
    });

    // The day, all visible projects together.
    let s = sum(&rows, &hide);
    let mut lanes = Vec::new();
    let tasks = tasks_by_id(app, mks.iter().map(|m| m.task))?;
    let mut waits = Vec::new();
    let (mut lo, mut hi) = (24.0f64, 0.0f64);
    let hour_of = |iso_s: &str| parse_iso(iso_s).map(|t| (t - bounds(date).0) / 3600.0);
    for (proj, v) in &rows {
        if hide.contains(proj) {
            continue;
        }
        let bars = v["bars"].as_array().cloned().unwrap_or_default();
        for b in &bars {
            if let (Some(a), Some(z)) = (hour_of(b["from"].as_str().unwrap_or("")), hour_of(b["to"].as_str().unwrap_or(""))) {
                lo = lo.min(a);
                hi = hi.max(z);
            }
        }
        let mut lane_marks: Vec<Value> = mks
            .iter()
            .filter(|m| tasks.get(&m.task).map(|t| t.project.as_str()).unwrap_or(NO_PROJECT) == proj)
            .map(|m| json!({"at": iso(m.at), "kind": m.kind, "task": m.task, "ref": rf("task", m.task), "text": one_line(&m.text, 200)}))
            .collect();
        for x in v["finished"].as_array().into_iter().flatten() {
            lane_marks.push(json!({"at": x["at"], "kind": "done", "task": x["task"], "ref": x["ref"], "text": format!("Finished {}", x["title"].as_str().unwrap_or(""))}));
        }
        for w in v["waits"].as_array().into_iter().flatten() {
            let id = w["task"].as_i64().unwrap_or(0);
            let at = w["at"].as_str().and_then(parse_iso).unwrap_or(0.0);
            let asked = mks.iter().filter(|m| m.task == id && m.kind == "question" && (m.at - at).abs() < 120.0).map(|m| m.text.clone()).next();
            let mut w = w.clone();
            w["text"] = json!(asked.map(|t| one_line(t.trim_start_matches("Asked: ").trim_start_matches("Waiting for you: "), 200)).unwrap_or_default());
            w["title"] = json!(bars.iter().find(|b| b["task"] == id).and_then(|b| b["title"].as_str()).unwrap_or(""));
            w["project"] = json!(proj);
            waits.push(w);
        }
        lane_marks.sort_by(|a, b| a["at"].as_str().cmp(&b["at"].as_str()));
        if !bars.is_empty() || !lane_marks.is_empty() {
            lanes.push(json!({"project": proj, "bars": bars, "marks": lane_marks}));
        }
    }
    for m in &mks {
        let h = (m.at - bounds(date).0) / 3600.0;
        lo = lo.min(h);
        hi = hi.max(h);
    }
    let wh = hours::get(app);
    let hh = |s: &str| s.split(':').next().and_then(|h| h.parse::<f64>().ok());
    lo = lo.min(hh(&wh.start).unwrap_or(9.0));
    hi = hi.max(hh(&wh.end).unwrap_or(17.0));
    if date == today {
        let n = local_dt(now);
        hi = hi.max(n.hour() as f64 + n.minute() as f64 / 60.0);
    }
    let (from_hour, to_hour) = (lo.floor().clamp(0.0, 23.0) as i64, hi.ceil().clamp(1.0, 24.0) as i64);
    let mut all: Vec<String> = projects.into_iter().collect();
    all.sort_by_key(|p| (p == NO_PROJECT, p.to_lowercase()));

    let mut day_json = totals(&s);
    let peak = s.running.iter().copied().max().unwrap_or(0);
    day_json["peak_slots"] = json!(s.running.iter().filter(|n| **n == peak && peak > 0).count());
    day_json["peak_first"] = json!(s.running.iter().position(|n| *n == peak && peak > 0));
    day_json["running"] = json!(s.running);
    day_json["hourly"] = json!(s.hourly);
    day_json["usage"] = crate::usage::day_slots(app, bounds(date).0, now)?;
    day_json["lanes"] = json!(lanes);
    day_json["waits"] = json!(waits);
    day_json["tasks"] = json!(rows.iter().filter(|(p, _)| !hide.contains(*p)).flat_map(|(_, v)| v["tasks"].as_array().cloned().unwrap_or_default()).collect::<Vec<_>>());
    day_json["finished"] = json!(finished_on(app, &rows, &hide)?);

    Ok(json!({
        "date": date_str(date),
        "today": date_str(today),
        "is_today": date == today,
        "now": iso(now),
        "day_start": iso(bounds(date).0),
        "from_hour": from_hour,
        "to_hour": to_hour,
        "projects": all,
        "hidden": hide.iter().collect::<Vec<_>>(),
        "day": day_json,
        "week": week,
        "last_week": last_week,
        "week_tasks": week_tasks,
        "week_task_median": median(finished),
        "first_weekday": weekday_key(app.cfg.first_weekday),
        "usual": usual,
        "oldest": oldest(app)?.map(date_str),
        "planning": crate::quality::recent(app, date)?.into_iter().filter(|v| !hide.contains(v["project"].as_str().unwrap_or(""))).collect::<Vec<_>>(),
    }))
}

/// The tasks finished on the day, in the order they finished (a task finished twice counts at its
/// last), each with its project and the PR it has now.
fn finished_on(app: &App, rows: &BTreeMap<String, Value>, hide: &HashSet<String>) -> Result<Vec<Value>> {
    let mut last: BTreeMap<i64, Value> = BTreeMap::new();
    for (proj, v) in rows.iter().filter(|(p, _)| !hide.contains(*p)) {
        for x in v["finished"].as_array().into_iter().flatten() {
            let Some(id) = x["task"].as_i64() else { continue };
            let mut x = x.clone();
            x["project"] = json!(proj);
            last.insert(id, x);
        }
    }
    let ids: Vec<i64> = last.keys().copied().collect();
    for chunk in ids.chunks(400) {
        let qs = vec!["?"; chunk.len()].join(", ");
        let rows = app.db.q(&format!("SELECT id, pr_num, pr_url, pr_state, pr_phase FROM tasks WHERE id IN ({qs})"), chunk.iter().map(|i| json!(i)).collect())?;
        for t in rows {
            let Some(x) = last.get_mut(&t.id()) else { continue };
            if t.i("pr_num").is_none() && !has(t.s("pr_url")) {
                continue;
            }
            let st = t.st("pr_state").to_uppercase();
            let state = if st == "MERGED" || t.s("pr_phase") == Some("merged") {
                "merged"
            } else if matches!(st.as_str(), "DECLINED" | "CLOSED" | "SUPERSEDED") || t.s("pr_phase") == Some("declined") {
                "declined"
            } else {
                "open"
            };
            x["pr"] = json!({"num": t.v("pr_num"), "url": t.st("pr_url"), "state": state});
        }
    }
    let mut out: Vec<Value> = last.into_values().collect();
    out.sort_by(|a, b| b["at"].as_str().cmp(&a["at"].as_str()));
    Ok(out)
}

/// The first day the board has anything for.
fn oldest(app: &App) -> Result<Option<NaiveDate>> {
    let a = app.db.val("SELECT MIN(date) FROM day_stats WHERE project != ''", p![])?;
    let b = app.db.val("SELECT MIN(at) FROM task_states", p![])?;
    let from_states = b.as_str().and_then(parse_iso).map(|t| local_dt(t).date_naive());
    let from_stats = a.as_str().and_then(parse_date);
    Ok(match (from_stats, from_states) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, y) => x.or(y),
    })
}

// ------------------------------------------------------------------ keeping and cleaning up

fn setting_days(app: &App, key: &str, default: i64) -> i64 {
    app.db.get_setting(key).ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(default)
}

pub fn detail_days(app: &App) -> i64 {
    setting_days(app, DETAIL_KEY, DETAIL_DAYS)
}

pub fn summary_days(app: &App) -> i64 {
    setting_days(app, SUMMARY_KEY, SUMMARY_DAYS)
}

/// `GET /history`.
pub fn history(app: &App) -> Result<Value> {
    let ev = app.db.q1("SELECT COUNT(*) AS n, COALESCE(SUM(LENGTH(text) + COALESCE(LENGTH(data), 0) + 48), 0) AS b FROM events", p![])?.unwrap_or_default();
    let st = app.db.q1("SELECT COUNT(*) AS n, COALESCE(SUM(48), 0) AS b FROM task_states", p![])?.unwrap_or_default();
    let se = app.db.q1("SELECT COUNT(*) AS n, COALESCE(SUM(LENGTH(text) + COALESCE(LENGTH(full), 0) + 48), 0) AS b FROM session_events", p![])?.unwrap_or_default();
    let ds = app.db.q1("SELECT COUNT(DISTINCT date) AS n, COALESCE(SUM(LENGTH(data) + 32), 0) AS b FROM day_stats WHERE project != ''", p![])?.unwrap_or_default();
    Ok(json!({
        "detail_days": detail_days(app),
        "summary_days": summary_days(app),
        "detail_choices": DETAIL_CHOICES,
        "summary_choices": SUMMARY_CHOICES,
        "events": ev.i0("n") + st.i0("n") + se.i0("n"),
        "events_bytes": ev.i0("b") + st.i0("b") + se.i0("b"),
        "summaries": ds.i0("n"),
        "summaries_bytes": ds.i0("b"),
        "last_cleanup": app.db.get_setting(LAST_CLEANUP_KEY)?,
        "oldest": oldest(app)?.map(date_str),
    }))
}

/// `POST /history {detail_days, summary_days}`.
pub fn set_history(app: &App, body: &Value) -> Result<Value> {
    if let Some(v) = body.get("detail_days").filter(|v| !v.is_null()) {
        let n = v.as_i64().filter(|n| DETAIL_CHOICES.contains(n)).ok_or_else(|| ApiError::new(400, "Keep every event for 30, 90, 180 or 365 days."))?;
        app.db.set_setting(DETAIL_KEY, Some(&n.to_string()))?;
    }
    if let Some(v) = body.get("summary_days").filter(|v| !v.is_null()) {
        let n = v.as_i64().filter(|n| SUMMARY_CHOICES.contains(n)).ok_or_else(|| ApiError::new(400, "Keep day summaries for 180, 365 or 730 days, or 0 for always."))?;
        app.db.set_setting(SUMMARY_KEY, Some(&n.to_string()))?;
    }
    history(app)
}

/// Remove what's older than the History settings allow: events (and status changes and terminal
/// lines) of finished tasks after `detail_days`, each day's summary after `summary_days`. Every
/// day loses its events only after its summary is kept.
pub fn cleanup(app: &App) -> Result<Value> {
    let today = today();
    let cut = today - Duration::days(detail_days(app));
    // Keep a summary for every day about to lose its events (at most a year and a bit back).
    if let Some(first) = oldest(app)? {
        let mut d = first.max(today - Duration::days(400));
        while d < cut {
            stored(app, d)?;
            d += Duration::days(1);
        }
    }
    let before = iso(start_of(cut));
    let done = "SELECT id FROM tasks WHERE status = 'done'";
    let (events, states, lines) = app.db.tx(|| {
        let e = app.db.changed(&format!("DELETE FROM events WHERE at < ? AND task_id IN ({done})"), p![before])?;
        // Keep each finished task's last status change, so it still reads as done.
        let s = app.db.changed(
            &format!("DELETE FROM task_states WHERE at < ? AND task_id IN ({done}) AND id NOT IN (SELECT MAX(id) FROM task_states GROUP BY task_id)"),
            p![before],
        )?;
        let l = app.db.changed("DELETE FROM session_events WHERE at < ?", p![before])?;
        app.db.changed("DELETE FROM usage_samples WHERE at < ?", p![before])?;
        Ok((e, s, l))
    })?;
    let days = match summary_days(app) {
        0 => 0,
        n => app.db.changed("DELETE FROM day_stats WHERE date < ?", p![date_str(today - Duration::days(n))])?,
    };
    app.db.set_setting(LAST_CLEANUP_KEY, Some(&now_iso()))?;
    app.info(format!("history cleanup: {events} events, {states} status changes, {lines} terminal lines, {days} day rows removed"));
    Ok(json!({"removed": {"events": events, "status_changes": states, "terminal_lines": lines, "day_rows": days}, "history": history(app)?}))
}

/// Once a day, after 3 AM local time, clean up.
pub fn cleanup_loop(app: std::sync::Arc<App>) {
    while !app.stopping() {
        let now = local_now();
        let last = app.db.get_setting(LAST_CLEANUP_KEY).ok().flatten().and_then(|s| parse_iso(&s)).map(|t| local_dt(t).date_naive());
        if now.hour() >= 3 && last != Some(now.date_naive()) {
            if let Err(e) = cleanup(&app) {
                app.info(format!("history cleanup failed: {}", e.message));
            }
        }
        app.sleep(30.0 * 60.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Weekday;

    #[test]
    fn weeks_start_on_the_first_weekday() {
        // 2026-10-08 is a Thursday.
        let d = NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
        assert_eq!(week_start(d, Weekday::Sun), NaiveDate::from_ymd_opt(2026, 10, 4).unwrap());
        assert_eq!(week_start(d, Weekday::Mon), NaiveDate::from_ymd_opt(2026, 10, 5).unwrap());
        assert_eq!(week_start(d, Weekday::Thu), d);
        assert_eq!(week_start(d, Weekday::Fri), NaiveDate::from_ymd_opt(2026, 10, 2).unwrap());
    }

    #[test]
    fn cleanup_counts_the_rows_each_table_lost() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::for_tests(crate::config::Config::for_tests(dir.path()));
        let old = iso(start_of(today() - Duration::days(200)));
        let t = app.db.x("INSERT INTO tasks(title, status) VALUES('Old', 'done')", p![]).unwrap();
        for _ in 0..3 {
            app.db.x("INSERT INTO events(task_id, at, kind, text) VALUES(?, ?, 'note', 'x')", p![t, old.clone()]).unwrap();
        }
        // The last of these is kept, so the task still reads as done.
        for s in ["working", "review", "done"] {
            app.db.x("INSERT INTO task_states(task_id, at, status) VALUES(?, ?, ?)", p![t, old.clone(), s]).unwrap();
        }
        for _ in 0..4 {
            app.db.x("INSERT INTO session_events(session_id, at, kind, text) VALUES('s1', ?, 'line', 'x')", p![old.clone()]).unwrap();
        }
        app.db.x("INSERT INTO day_stats(date, project, data) VALUES('2020-01-01', '', '')", p![]).unwrap();

        let out = cleanup(&app).unwrap();
        assert_eq!(out["removed"], json!({"events": 3, "status_changes": 2, "terminal_lines": 4, "day_rows": 1}));
    }
}
