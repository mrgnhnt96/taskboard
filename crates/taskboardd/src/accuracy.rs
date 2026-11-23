//! How past estimates turned out, so new ones get closer (`tb estimates`, `GET /estimates`).
//!
//! The samples are finished work with an estimate (`estimate.rs`): a task done and not failed, its
//! working time against its estimate; a goal whose counted tasks are all done, its running time
//! against its estimate. The bias is the median of actual ÷ estimate, with the share that landed
//! within ±25%, overall, by project and by the size band the estimate fell in; the misses are the
//! biggest among the most recent. Until enough work has an estimate, the typical working time of
//! past finished tasks by project still gives a scale to estimate against.
//!
//! A call reads the board in four queries, however many tasks there are: the finished tasks, their
//! working stretches, the estimated goals' tasks and those tasks' stretches.
//!
//! [`window`] is the app's view of the same numbers: the tasks finished in a period (a recap's
//! `estimates` fact, the Days page's last 30 days), with over and under counted and a trend by month.
//! These are the agents' own estimates (`est_min`), not the developer-time estimate (`human_min`)
//! behind the hours-saved numbers.

use std::collections::{BTreeMap, HashMap};

use chrono::{Datelike, Duration, NaiveDate};
use serde_json::{json, Value};

use crate::app::App;
use crate::estimate::{len, stretches};
use crate::util::*;
use crate::p;

/// The size bands an estimate falls in, by its upper bound in minutes.
const BANDS: [(&str, f64); 4] = [("under 30m", 30.0), ("30m–2h", 120.0), ("2h–8h", 480.0), ("over 8h", f64::INFINITY)];

/// Finished tasks this recent give the typical working times.
const TYPICAL_DAYS: f64 = 180.0;

/// A project's own numbers stand for it in the summary once it has this many estimated tasks.
const PROJECT_MIN: usize = 3;

/// The misses are picked from this many of the most recently finished samples.
const RECENT: usize = 30;
const MISSES: usize = 5;

/// Within ±25% of the estimate counts as on target.
const NEAR: (f64, f64) = (0.75, 1.25);

/// One finished task or goal with an estimate.
#[derive(Clone, Debug)]
pub struct Sample {
    pub rf: String,
    pub title: String,
    pub project: String,
    pub est_min: f64,
    pub actual_min: f64,
    pub finished_at: String,
}

impl Sample {
    fn ratio(&self) -> f64 {
        self.actual_min / self.est_min
    }
}

/// What the board knows: estimated tasks and goals, and every recent finished task's working minutes
/// with its project (for the typical times).
#[derive(Default, Debug)]
pub struct Data {
    pub tasks: Vec<Sample>,
    pub goals: Vec<Sample>,
    pub worked: Vec<(String, f64)>,
}

fn project_of(r: &Row) -> String {
    r.s("project").filter(|p| !p.is_empty()).unwrap_or(crate::days::NO_PROJECT).to_string()
}

fn ids(rows: &[Row], k: &str) -> String {
    rows.iter().filter_map(|r| r.i(k)).map(|i| i.to_string()).collect::<Vec<_>>().join(",")
}

/// Reads the samples off the board.
pub fn load(app: &App) -> Result<Data> {
    let since = iso(now_ts() - TYPICAL_DAYS * 86400.0);
    let tasks = app.db.q(
        "SELECT id, title, project, est_min, finished_at FROM tasks \
         WHERE status = 'done' AND COALESCE(failed, 0) = 0 AND COALESCE(kind, '') != ? AND finished_at IS NOT NULL \
         AND (est_min IS NOT NULL OR finished_at >= ?)",
        p![crate::human::KIND, since],
    )?;
    let work = crate::estimate::work_by_task(app, &tasks.iter().map(|t| t.id()).collect::<Vec<_>>())?;
    let mut data = Data::default();
    for t in &tasks {
        let actual = work.get(&t.id()).copied().unwrap_or(0.0) / 60.0;
        if actual <= 0.0 {
            continue;
        }
        let finished = t.st("finished_at");
        if finished >= since {
            data.worked.push((project_of(t), actual));
        }
        if let Some(est) = t.i("est_min").filter(|e| *e > 0) {
            data.tasks.push(Sample { rf: rf("task", t.id()), title: t.st("title"), project: project_of(t), est_min: est as f64, actual_min: actual, finished_at: finished });
        }
    }
    data.goals = finished_goals(app)?;
    Ok(data)
}

/// Estimated goals whose counted tasks are all done with no PR still open, against their running time.
fn finished_goals(app: &App) -> Result<Vec<Sample>> {
    let goals = app.db.q("SELECT id, name, project, est_min FROM goals WHERE est_min > 0", p![])?;
    if goals.is_empty() {
        return Ok(vec![]);
    }
    let list = ids(&goals, "id");
    let tasks = app.db.q(
        &format!(
            "SELECT t.*, x.gid FROM (SELECT id AS tid, goal_id AS gid FROM tasks WHERE goal_id IN ({list}) \
             UNION SELECT task_id, goal_id FROM task_goals WHERE goal_id IN ({list})) x JOIN tasks t ON t.id = x.tid"
        ),
        p![],
    )?;
    let worked: Vec<i64> = tasks.iter().filter(|t| !crate::human::is(t)).map(|t| t.id()).collect();
    let spans = stretches(app, &worked)?;
    let mut by: HashMap<i64, Vec<&Row>> = HashMap::new();
    for t in &tasks {
        by.entry(t.i0("gid")).or_default().push(t);
    }
    let mut out = vec![];
    for g in &goals {
        let Some(ts) = by.get(&g.id()) else { continue };
        let done = ts.iter().all(|t| t.s("status") == Some("done")) && !ts.iter().any(|t| !t.b("failed") && crate::board::pr_still_open(t));
        if !done {
            continue;
        }
        let all: Vec<(f64, f64)> = ts.iter().filter(|t| !crate::human::is(t)).flat_map(|t| spans.get(&t.id()).cloned().unwrap_or_default()).collect();
        let running: f64 = crate::clock::merge(all).iter().map(|(a, b)| b - a).sum::<f64>() / 60.0;
        if running <= 0.0 {
            continue;
        }
        let finished = ts.iter().filter_map(|t| t.s("finished_at")).max().unwrap_or("").to_string();
        out.push(Sample { rf: rf("goal", g.id()), title: g.st("name"), project: project_of(g), est_min: g.i0("est_min") as f64, actual_min: running, finished_at: finished });
    }
    Ok(out)
}

/// The middle value; the mean of the middle two for an even count.
pub fn median(xs: &[f64]) -> Option<f64> {
    quantile(xs, 0.5)
}

fn quantile(xs: &[f64], q: f64) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = q * (v.len() - 1) as f64;
    let (lo, hi) = (at.floor() as usize, at.ceil() as usize);
    Some(v[lo] + (v[hi] - v[lo]) * (at - lo as f64))
}

/// The band an estimate of `min` minutes falls in.
pub fn band(min: f64) -> &'static str {
    BANDS.iter().find(|(_, top)| min < *top).map(|(b, _)| *b).unwrap_or(BANDS[3].0)
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Whether work ran over its estimates (`short`: they were too short), under (`long`) or `close`.
pub fn runs(ratio: f64) -> &'static str {
    if ratio > 1.1 {
        "short"
    } else if ratio < 0.9 {
        "long"
    } else {
        "close"
    }
}

/// `{n, ratio, within, runs, median_actual_min, median_est_min}` for a set of samples; null when empty.
pub fn bias(samples: &[&Sample]) -> Value {
    let ratios: Vec<f64> = samples.iter().map(|s| s.ratio()).collect();
    let Some(r) = median(&ratios) else { return Value::Null };
    let near = ratios.iter().filter(|r| **r >= NEAR.0 && **r <= NEAR.1).count();
    let actual: Vec<f64> = samples.iter().map(|s| s.actual_min).collect();
    let est: Vec<f64> = samples.iter().map(|s| s.est_min).collect();
    json!({"n": samples.len(), "ratio": round2(r), "within": round2(near as f64 / samples.len() as f64), "runs": runs(r),
           "median_actual_min": median(&actual).map(f64::round), "median_est_min": median(&est).map(f64::round)})
}

fn sample_json(s: &Sample) -> Value {
    json!({"ref": s.rf, "title": s.title, "project": s.project, "est_min": s.est_min.round(), "actual_min": s.actual_min.round(),
           "ratio": round2(s.ratio()), "finished_at": s.finished_at})
}

/// The biggest misses among the most recently finished samples, biggest first; ones within ±25% aren't misses.
fn misses(samples: &[&Sample]) -> Vec<Value> {
    let mut recent: Vec<&Sample> = samples.to_vec();
    recent.sort_by(|a, b| b.finished_at.cmp(&a.finished_at));
    recent.truncate(RECENT);
    recent.retain(|s| s.ratio() < NEAR.0 || s.ratio() > NEAR.1);
    recent.sort_by(|a, b| b.ratio().ln().abs().partial_cmp(&a.ratio().ln().abs()).unwrap_or(std::cmp::Ordering::Equal));
    recent.into_iter().take(MISSES).map(sample_json).collect()
}

/// The typical working minutes of finished tasks, by project, busiest first.
fn typical(worked: &[(String, f64)]) -> Vec<Value> {
    let mut by: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for (p, m) in worked {
        by.entry(p).or_default().push(*m);
    }
    let mut out: Vec<Value> = by
        .into_iter()
        .map(|(p, ms)| {
            json!({"project": p, "n": ms.len(), "median_min": median(&ms).map(f64::round), "low_min": quantile(&ms, 0.25).map(f64::round),
                   "high_min": quantile(&ms, 0.75).map(f64::round)})
        })
        .collect();
    out.sort_by(|a, b| b["n"].as_u64().cmp(&a["n"].as_u64()));
    out
}

/// The report over `data`, narrowed to `project` when given.
pub fn report(data: &Data, project: Option<&str>) -> Value {
    let mine = |s: &&Sample| project.is_none_or(|p| s.project == p);
    let tasks: Vec<&Sample> = data.tasks.iter().filter(mine).collect();
    let goals: Vec<&Sample> = data.goals.iter().filter(mine).collect();
    let mut by_project: BTreeMap<&str, Vec<&Sample>> = BTreeMap::new();
    for s in &tasks {
        by_project.entry(&s.project).or_default().push(s);
    }
    let mut projects: Vec<Value> = by_project.iter().map(|(p, ss)| with(bias(ss), json!({"project": p}))).collect();
    projects.sort_by(|a, b| b["n"].as_u64().cmp(&a["n"].as_u64()));
    let bands: Vec<Value> = BANDS
        .iter()
        .filter_map(|(b, _)| {
            let ss: Vec<&Sample> = tasks.iter().copied().filter(|s| band(s.est_min) == *b).collect();
            (!ss.is_empty()).then(|| with(bias(&ss), json!({"band": b})))
        })
        .collect();
    let worked: Vec<(String, f64)> = data.worked.iter().filter(|(p, _)| project.is_none_or(|x| x == p)).cloned().collect();
    let all: Vec<&Sample> = tasks.iter().chain(goals.iter()).copied().collect();
    json!({"project": project, "tasks": bias(&tasks), "goals": bias(&goals), "projects": projects, "bands": bands,
           "misses": misses(&all), "typical": typical(&worked)})
}

fn with(mut a: Value, b: Value) -> Value {
    if let (Some(a), Value::Object(b)) = (a.as_object_mut(), b) {
        a.extend(b);
    }
    a
}

/// A ratio as the board writes it: 1.4, 2 (not 2.0).
fn times(x: &Value) -> String {
    format!("{}×", x.as_f64().unwrap_or(0.0))
}

fn pct(x: &Value) -> String {
    format!("{}%", (x.as_f64().unwrap_or(0.0) * 100.0).round())
}

fn mins(x: &Value) -> String {
    len(x.as_f64().unwrap_or(0.0))
}

fn n_of(v: &Value, word: &str) -> String {
    plural(v["n"].as_i64().unwrap_or(0), word)
}

/// "Your taskboard estimates have run short: work took 1.4× the estimate (median of 23 tasks; 35% within ±25%)."
fn bias_line(whose: &str, b: &Value) -> String {
    let how = match b["runs"].as_str() {
        Some("short") => "have run short",
        Some("long") => "have run long",
        _ => "have been close",
    };
    format!("Your {whose}estimates {how}: work took {} the estimate (median of {}; {} within ±25%).", times(&b["ratio"]), n_of(b, "task"), pct(&b["within"]))
}

/// "Past taskboard tasks took a median 1h 10m of working time (middle half 35m–2h, 140 tasks)."
fn typical_line(whose: &str, t: &Value) -> String {
    format!(
        "Past {whose}tasks took a median {} of working time (middle half {}–{}, {}).",
        mins(&t["median_min"]),
        mins(&t["low_min"]),
        mins(&t["high_min"]),
        n_of(t, "task")
    )
}

/// The two lines `tb planning` and the replies to `tb task new`, `tb goal new` and `tb propose` carry:
/// the bias for the project (all projects while it has too few) and what the commonest band took; with
/// no estimates yet, the typical working time of past tasks. Nothing when there's nothing to go on.
pub fn summary(data: &Data, project: Option<&str>) -> Vec<String> {
    let own = project.map(|p| report(data, Some(p)));
    let enough = own.as_ref().filter(|r| r["tasks"]["n"].as_u64().unwrap_or(0) as usize >= PROJECT_MIN);
    let (rep, whose) = match (enough, project) {
        (Some(r), Some(p)) => (r.clone(), format!("{p} ")),
        _ => (report(data, None), String::new()),
    };
    if !rep["tasks"].is_null() {
        let mut second = rep["bands"]
            .as_array()
            .and_then(|bs| bs.iter().max_by_key(|b| b["n"].as_u64()))
            .map(|b| format!("Tasks you called {} took a median {} ({}).", b["band"].as_str().unwrap_or(""), mins(&b["median_actual_min"]), n_of(b, "task")))
            .unwrap_or_default();
        if !rep["goals"].is_null() {
            second += &format!(" Goals took {} their estimate (median of {}).", times(&rep["goals"]["ratio"]), n_of(&rep["goals"], "goal"));
        }
        return vec![bias_line(&whose, &rep["tasks"]), second.trim().to_string()];
    }
    let own: Vec<(String, f64)> = data.worked.iter().filter(|(p, _)| project == Some(p.as_str())).cloned().collect();
    let (worked, whose) = match project {
        Some(p) if !own.is_empty() => (own, format!("{p} ")),
        _ => (data.worked.iter().map(|(_, m)| (String::new(), *m)).collect(), String::new()),
    };
    match typical(&worked).first() {
        Some(t) => vec![format!("No finished {whose}work with an estimate yet."), typical_line(&whose, t)],
        None => vec![],
    }
}

/// The lines `tb estimates` prints for a report.
pub fn lines(rep: &Value) -> Vec<String> {
    let mut out = vec![];
    let row = |label: &str, b: &Value| format!("  {label:<14} {:>6}  {:>4} within ±25%  median {} for {} estimated  ({})",
        times(&b["ratio"]), pct(&b["within"]), mins(&b["median_actual_min"]), mins(&b["median_est_min"]), n_of(b, "task"));
    let t = &rep["tasks"];
    if t.is_null() {
        out.push("No finished work with an estimate yet.".to_string());
    } else {
        let over = match t["runs"].as_str() {
            Some("short") => "Estimates run short: work takes longer than estimated.",
            Some("long") => "Estimates run long: work takes less time than estimated.",
            _ => "Estimates are close to the actual time.",
        };
        out.push(format!("Tasks took {} their estimate (median of {}); {} landed within ±25%. {over}", times(&t["ratio"]), n_of(t, "task"), pct(&t["within"])));
        let g = &rep["goals"];
        if !g.is_null() {
            out.push(format!("Goals took {} their estimate of running time (median of {}); {} within ±25%.", times(&g["ratio"]), n_of(g, "goal"), pct(&g["within"])));
        }
        let ps = rep["projects"].as_array().cloned().unwrap_or_default();
        if rep["project"].is_null() && !ps.is_empty() {
            out.push(String::new());
            out.push("By project:".into());
            out.extend(ps.iter().map(|p| row(p["project"].as_str().unwrap_or(""), p)));
        }
        out.push(String::new());
        out.push("By the size you estimated:".into());
        out.extend(rep["bands"].as_array().into_iter().flatten().map(|b| row(b["band"].as_str().unwrap_or(""), b)));
        let ms = rep["misses"].as_array().cloned().unwrap_or_default();
        if !ms.is_empty() {
            out.push(String::new());
            out.push("Biggest recent misses:".into());
            out.extend(ms.iter().map(|m| {
                format!("  {} {} · est {} · actual {} ({})", m["ref"].as_str().unwrap_or(""), m["title"].as_str().unwrap_or(""), mins(&m["est_min"]), mins(&m["actual_min"]), times(&m["ratio"]))
            }));
        }
    }
    let ty = rep["typical"].as_array().cloned().unwrap_or_default();
    if !ty.is_empty() {
        out.push(String::new());
        out.push(format!("Working time of finished tasks, last {TYPICAL_DAYS} days:"));
        out.extend(ty.iter().map(|t| {
            format!("  {:<14} median {} (middle half {}–{}, {})", t["project"].as_str().unwrap_or(""), mins(&t["median_min"]), mins(&t["low_min"]), mins(&t["high_min"]), n_of(t, "task"))
        }));
    }
    out
}

/// `GET /estimates?project=<name>` (every project's when left out): the report, with its summary.
pub fn get(app: &App, query: &crate::api::Query) -> Result<Value> {
    let project = query.get("project").map(|s| s.as_str()).filter(|s| !s.is_empty() && *s != "all");
    let data = load(app)?;
    let mut rep = report(&data, project);
    rep["summary"] = json!(summary(&data, project));
    rep["lines"] = json!(lines(&rep));
    Ok(rep)
}

/// The summary for `project`, for a reply; nothing (and a log line) when the board can't be read.
pub fn summary_for(app: &App, project: Option<&str>) -> Vec<String> {
    match load(app) {
        Ok(d) => summary(&d, project),
        Err(e) => {
            app.info(format!("estimates: couldn't read past estimates: {}", e.message));
            vec![]
        }
    }
}

// ------------------------------------------------------------------ the app's cards

/// Months the app's trend covers, the last one the month the window ends in.
pub const MONTHS: u32 = 6;

/// One finished estimated task in a window.
struct Done {
    task: String,
    title: String,
    project: String,
    est: f64,
    actual: f64,
    at: f64,
}

impl Done {
    fn ratio(&self) -> f64 {
        self.actual / self.est
    }
}

/// The counts and shares of a set of tasks: on the mark within ±25%, over above it, under below it.
fn tally(ds: &[&Done]) -> Value {
    let ratios: Vec<f64> = ds.iter().map(|d| d.ratio()).collect();
    let over = ratios.iter().filter(|r| **r > NEAR.1).count();
    let under = ratios.iter().filter(|r| **r < NEAR.0).count();
    let on = ratios.len() - over - under;
    json!({
        "tasks": ds.len(),
        "median_ratio": median(&ratios).map(round2),
        "within": if ds.is_empty() { Value::Null } else { json!(round2(on as f64 / ds.len() as f64)) },
        "over": over, "under": under, "on": on,
        "est_min": ds.iter().map(|d| d.est).sum::<f64>().round(),
        "actual_min": ds.iter().map(|d| d.actual).sum::<f64>().round(),
    })
}

/// The first day of the month `back` months before `d`'s.
fn month_start(d: NaiveDate, back: u32) -> NaiveDate {
    let n = d.year() * 12 + d.month0() as i32 - back as i32;
    NaiveDate::from_ymd_opt(n.div_euclid(12), n.rem_euclid(12) as u32 + 1, 1).unwrap_or(d)
}

/// The finished estimated agent tasks from `from` to `to` (epoch seconds), each with its working time.
fn finished_in(app: &App, from: f64, to: f64) -> Result<Vec<Done>> {
    let rows = app.db.q(
        "SELECT * FROM tasks WHERE status = 'done' AND COALESCE(failed, 0) = 0 AND est_min > 0 AND finished_at >= ? AND finished_at < ? ORDER BY finished_at, id",
        p![iso(from), iso(to)],
    )?;
    let rows: Vec<Row> = rows.into_iter().filter(|r| !crate::human::is(r)).collect();
    let work = crate::estimate::work_by_task(app, &rows.iter().map(|r| r.id()).collect::<Vec<_>>())?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let actual = (work.get(&r.id())? / 60.0).round();
            (actual > 0.0).then(|| Done {
                task: crate::human::task_ref(r),
                title: r.st("title").to_string(),
                project: project_of(r),
                est: r.i0("est_min") as f64,
                actual,
                at: r.s("finished_at").and_then(parse_iso).unwrap_or(from),
            })
        })
        .collect())
}

/// How the estimates of tasks finished from `from` to `to` turned out, with the trend by month over
/// the [`MONTHS`] up to `to`'s month: overall, by project, the biggest misses and `months`. Null when
/// no task finished in the window had an estimate. `hide` leaves out those projects.
pub fn window(app: &App, from: f64, to: f64, hide: &[String]) -> Result<Value> {
    let last_day = local_dt(to - 1.0).date_naive();
    let first_month = month_start(last_day, MONTHS - 1);
    let start = crate::days::start_of(first_month).min(from);
    let all: Vec<Done> = finished_in(app, start, to)?.into_iter().filter(|d| !hide.contains(&d.project)).collect();
    let now: Vec<&Done> = all.iter().filter(|d| d.at >= from).collect();
    if now.is_empty() {
        return Ok(Value::Null);
    }
    let mut out = tally(&now);

    let mut names: Vec<&str> = now.iter().map(|d| d.project.as_str()).collect();
    names.sort();
    names.dedup();
    let mut projects: Vec<Value> = names
        .iter()
        .map(|n| with(tally(&now.iter().copied().filter(|d| d.project == *n).collect::<Vec<_>>()), json!({"name": n})))
        .collect();
    projects.sort_by(|a, b| b["tasks"].as_u64().cmp(&a["tasks"].as_u64()).then(a["name"].as_str().cmp(&b["name"].as_str())));
    out["projects"] = json!(projects);

    let mut misses: Vec<&&Done> = now.iter().filter(|d| d.ratio() < NEAR.0 || d.ratio() > NEAR.1).collect();
    misses.sort_by(|a, b| b.ratio().ln().abs().total_cmp(&a.ratio().ln().abs()).then(b.at.total_cmp(&a.at)));
    out["misses"] = json!(misses
        .iter()
        .take(MISSES)
        .map(|d| json!({"task": d.task, "title": d.title, "project": d.project, "est_min": d.est, "actual_min": d.actual, "ratio": round2(d.ratio())}))
        .collect::<Vec<_>>());

    out["months"] = json!((0..MONTHS)
        .rev()
        .map(|back| {
            let m = month_start(last_day, back);
            let (a, b) = (crate::days::start_of(m), crate::days::start_of(month_start(m + Duration::days(32), 0)));
            with(tally(&all.iter().filter(|d| d.at >= a && d.at < b).collect::<Vec<_>>()), json!({"month": m.format("%Y-%m").to_string()}))
        })
        .collect::<Vec<_>>());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(project: &str, est: f64, actual: f64, at: &str) -> Sample {
        Sample { rf: format!("T{}", at.len()), title: format!("{project} {est}→{actual}"), project: project.into(), est_min: est, actual_min: actual, finished_at: at.into() }
    }

    #[test]
    fn medians_and_quantiles() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[3.0]), Some(3.0));
        assert_eq!(median(&[4.0, 1.0, 3.0]), Some(3.0));
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), Some(2.5));
        assert_eq!(quantile(&[0.0, 10.0, 20.0, 30.0, 40.0], 0.25), Some(10.0));
    }

    #[test]
    fn bands_split_at_30m_2h_and_8h() {
        assert_eq!(band(15.0), "under 30m");
        assert_eq!(band(29.0), "under 30m");
        assert_eq!(band(30.0), "30m–2h");
        assert_eq!(band(119.0), "30m–2h");
        assert_eq!(band(120.0), "2h–8h");
        assert_eq!(band(479.0), "2h–8h");
        assert_eq!(band(480.0), "over 8h");
        assert_eq!(band(3000.0), "over 8h");
    }

    #[test]
    fn bias_is_the_median_ratio_with_the_share_near_the_estimate() {
        let a = [s("tb", 60.0, 60.0, "1"), s("tb", 60.0, 120.0, "2"), s("tb", 30.0, 45.0, "3"), s("tb", 100.0, 70.0, "4")];
        let b = bias(&a.iter().collect::<Vec<_>>());
        assert_eq!(b["n"], 4);
        // Ratios 1.0, 2.0, 1.5, 0.7: the median is 1.25; only 1.0 is within ±25%.
        assert_eq!(b["ratio"], 1.25);
        assert_eq!(b["within"], 0.25);
        assert_eq!(b["runs"], "short");
        assert_eq!(runs(0.8), "long");
        assert_eq!(runs(1.05), "close");
        assert!(bias(&[]).is_null());
    }

    #[test]
    fn report_groups_by_project_and_band_and_picks_recent_misses() {
        let data = Data {
            tasks: vec![
                s("tb", 60.0, 90.0, "2026-11-01"),
                s("tb", 60.0, 60.0, "2026-11-02"),
                s("tb", 20.0, 80.0, "2026-11-03"),
                s("midna", 300.0, 150.0, "2026-11-04"),
            ],
            goals: vec![s("tb", 600.0, 900.0, "2026-11-05")],
            worked: vec![("tb".into(), 60.0), ("tb".into(), 30.0), ("midna".into(), 120.0)],
        };
        let r = report(&data, None);
        assert_eq!(r["tasks"]["n"], 4);
        assert_eq!(r["projects"][0]["project"], "tb");
        assert_eq!(r["projects"][0]["n"], 3);
        assert_eq!(r["projects"][0]["ratio"], 1.5);
        let bands: Vec<&str> = r["bands"].as_array().unwrap().iter().map(|b| b["band"].as_str().unwrap()).collect();
        assert_eq!(bands, ["under 30m", "30m–2h", "2h–8h"]);
        assert_eq!(r["goals"]["ratio"], 1.5);
        let misses: Vec<f64> = r["misses"].as_array().unwrap().iter().map(|m| m["ratio"].as_f64().unwrap()).collect();
        // 4× and 0.5× are the biggest; 1.5× twice next; the 1.0× isn't a miss.
        assert_eq!(misses, [4.0, 0.5, 1.5, 1.5]);
        let mine = report(&data, Some("midna"));
        assert_eq!(mine["tasks"]["n"], 1);
        assert!(mine["goals"].is_null());
        assert_eq!(mine["typical"][0]["median_min"], 120.0);
    }

    #[test]
    fn summary_uses_the_project_once_it_has_enough_and_the_commonest_band() {
        let data = Data {
            tasks: vec![s("tb", 60.0, 90.0, "1"), s("tb", 60.0, 84.0, "2"), s("tb", 90.0, 110.0, "3"), s("tb", 20.0, 40.0, "4"), s("midna", 60.0, 30.0, "5")],
            ..Default::default()
        };
        let lines = summary(&data, Some("tb"));
        assert_eq!(lines[0], "Your tb estimates have run short: work took 1.45× the estimate (median of 4 tasks; 25% within ±25%).");
        assert_eq!(lines[1], "Tasks you called 30m–2h took a median 1h 30m (3 tasks).");
        // midna has one estimate: the summary falls back to every project's.
        assert!(summary(&data, Some("midna"))[0].starts_with("Your estimates have run short: work took 1.4× the estimate (median of 5 tasks;"));
    }

    #[test]
    fn with_no_estimates_only_the_typical_times_show() {
        let data = Data { worked: vec![("tb".into(), 30.0), ("tb".into(), 60.0), ("tb".into(), 120.0), ("midna".into(), 10.0)], ..Default::default() };
        let r = report(&data, None);
        assert!(r["tasks"].is_null() && r["bands"].as_array().unwrap().is_empty() && r["misses"].as_array().unwrap().is_empty());
        assert_eq!(r["typical"][0]["project"], "tb");
        assert_eq!(summary(&data, Some("tb")), ["No finished tb work with an estimate yet.", "Past tb tasks took a median 1h of working time (middle half 45m–1h 30m, 3 tasks)."]);
        assert_eq!(summary(&data, Some("kass"))[1], "Past tasks took a median 45m of working time (middle half 25m–1h 15m, 4 tasks).");
        let printed = lines(&r);
        assert_eq!(printed[0], "No finished work with an estimate yet.");
        assert!(printed.iter().any(|l| l.starts_with("  tb") && l.contains("median 1h")), "{printed:?}");
        assert!(summary(&Data::default(), None).is_empty());
    }

    fn done(est: f64, actual: f64) -> Done {
        Done { task: "T1".into(), title: "t".into(), project: "p".into(), est, actual, at: 0.0 }
    }

    #[test]
    fn a_window_counts_over_under_and_on_the_mark() {
        let ds = [done(60.0, 60.0), done(60.0, 120.0), done(60.0, 30.0), done(40.0, 50.0)];
        let v = tally(&ds.iter().collect::<Vec<_>>());
        assert_eq!(v["tasks"], 4);
        assert_eq!((v["over"].as_u64(), v["under"].as_u64(), v["on"].as_u64()), (Some(1), Some(1), Some(2)));
        assert_eq!(v["within"], json!(0.5));
        assert_eq!(v["median_ratio"], json!(1.13));
        let empty = tally(&[]);
        assert!(empty["median_ratio"].is_null() && empty["within"].is_null());
    }

    #[test]
    fn months_step_back_across_years() {
        let d = NaiveDate::from_ymd_opt(2026, 2, 14).unwrap();
        assert_eq!(month_start(d, 0), NaiveDate::from_ymd_opt(2026, 2, 1).unwrap());
        assert_eq!(month_start(d, 5), NaiveDate::from_ymd_opt(2025, 9, 1).unwrap());
    }
}
