//! The time review: where a period's agent time went, built from the stored spans (`spans.rs`) and the
//! tasks' status changes (`task_states`). It runs on `[review]`'s schedule, keeps every review it makes,
//! and tells the owner with a notification and a Needs you item that stays until the review is opened.

use std::collections::{BTreeMap, HashMap};

use chrono::{Datelike, Duration, Local, NaiveDateTime, NaiveTime, TimeZone};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{days, dispatch, hours, midna, p};

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS reviews(
  id INTEGER PRIMARY KEY, period_start TEXT, period_end TEXT, created_at TEXT, scheduled INT DEFAULT 0,
  data TEXT, seen_at TEXT);
"#;

/// Columns added since the first `reviews` schema: `fixes` is the suggested fixes pass's status and what it filed.
pub const ADDED: &[(&str, &str, &str)] = &[("reviews", "fixes", "TEXT")];

/// `[review]` in config.toml: `cadence` is `weekly` or `off`; `day` and `at` are the Mac's local time.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ReviewConfig {
    pub cadence: String,
    pub day: String,
    pub at: String,
    /// After each kept review, Claude files its suggested fixes to the backlog.
    pub suggest: bool,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        ReviewConfig { cadence: "weekly".into(), day: "friday".into(), at: "5:00 PM".into(), suggest: true }
    }
}

const TICK_SECS: f64 = 60.0;
const LAST_KEY: &str = "review_last_scheduled";
const TOP: usize = 10;
/// A command run fewer times than this isn't "repeated".
const REPEATED: usize = 3;
/// Tools whose span is a subagent's whole run, which the subagent's own spans already count.
const WRAPPERS: &[&str] = &["Task", "Agent"];
/// Spans that wait rather than work, left out of the command timings.
const WAITS: &[&str] = &["background poll", "load wait", "ci wait"];

pub fn run(app: std::sync::Arc<App>) {
    while !app.stopping() {
        if let Err(e) = tick(&app, hours::now_local()) {
            app.info(format!("review: {}", e.message));
        }
        app.sleep(TICK_SECS);
    }
}

/// The latest scheduled moment at or before `now`, if the schedule is on.
pub fn due_at(cfg: &ReviewConfig, now: NaiveDateTime) -> Option<NaiveDateTime> {
    if cfg.cadence.trim().to_lowercase() != "weekly" {
        return None;
    }
    let day = crate::config::parse_weekday(&cfg.day)?;
    let at = hours::parse_time(&cfg.at, "review").ok()?;
    let at = NaiveTime::parse_from_str(&at, "%H:%M").ok()?;
    let back = (now.weekday().num_days_from_monday() + 7 - day.num_days_from_monday()) % 7;
    let mut when = (now.date() - Duration::days(back as i64)).and_time(at);
    if when > now {
        when -= Duration::days(7);
    }
    Some(when)
}

fn ts_of(local: NaiveDateTime) -> f64 {
    Local.from_local_datetime(&local).earliest().map(|t| t.timestamp() as f64).unwrap_or(0.0)
}

/// Makes the week's review once its scheduled moment has passed (once, however late the board wakes).
pub fn tick(app: &App, now: NaiveDateTime) -> Result<Option<i64>> {
    let Some(when) = due_at(&app.cfg.review, now) else { return Ok(None) };
    let key = when.format("%Y-%m-%dT%H:%M").to_string();
    if app.db.get_setting(LAST_KEY)?.as_deref() >= Some(key.as_str()) {
        return Ok(None);
    }
    app.db.set_setting(LAST_KEY, Some(&key))?;
    let end = ts_of(when);
    let start = end - 7.0 * 86400.0;
    if app.db.count("SELECT COUNT(*) FROM spans WHERE started_at >= ? AND started_at < ?", p![iso(start), iso(end)])? == 0 {
        return Ok(None);
    }
    let id = save(app, start, end, true)?;
    crate::review_fixes::after_save(app, id)?;
    Ok(Some(id))
}

/// The period `--week` (`this`, `last`, or a date in the week) or `--since` (a date, or `3d`/`2w` back)
/// names; the last 7 days without either.
pub fn period(app: &App, since: Option<&str>, week: Option<&str>) -> Result<(f64, f64)> {
    let now = now_ts();
    if let Some(w) = week.map(str::trim).filter(|w| !w.is_empty()) {
        let today = days::today();
        let d = match w {
            "this" => today,
            "last" => today - Duration::days(7),
            _ => days::parse_date(w).ok_or_else(|| ApiError::new(400, "--week takes this, last or a date like 2026-10-09."))?,
        };
        let first = days::week_start(d, app.cfg.first_weekday);
        let start = ts_of(first.and_hms_opt(0, 0, 0).unwrap_or_default());
        return Ok((start, ts_of((first + Duration::days(7)).and_hms_opt(0, 0, 0).unwrap_or_default()).min(now)));
    }
    if let Some(s) = since.map(str::trim).filter(|s| !s.is_empty()) {
        let back = |n: &str, unit: f64| n.parse::<f64>().ok().filter(|n| *n > 0.0).map(|n| now - n * unit);
        let start = if let Some(n) = s.strip_suffix('d') {
            back(n, 86400.0)
        } else if let Some(n) = s.strip_suffix('w') {
            back(n, 7.0 * 86400.0)
        } else if let Some(d) = days::parse_date(s) {
            Some(ts_of(d.and_hms_opt(0, 0, 0).unwrap_or_default()))
        } else {
            parse_iso(s)
        };
        let start = start.ok_or_else(|| ApiError::new(400, "--since takes a date like 2026-10-09, or 3d or 2w back."))?;
        if start >= now {
            return err(400, "--since has to be in the past.");
        }
        return Ok((start, now));
    }
    Ok((now - 7.0 * 86400.0, now))
}

struct Span {
    task_id: Option<i64>,
    claude_id: String,
    project: String,
    tool: String,
    category: String,
    fingerprint: String,
    command: String,
    seconds: f64,
    exit_code: Option<i64>,
    timed_out: bool,
    background: bool,
}

fn spans_in(app: &App, start: f64, end: f64) -> Result<Vec<Span>> {
    let rows = app.db.q("SELECT * FROM spans WHERE started_at >= ? AND started_at < ? AND refused = 0 ORDER BY started_at, id", p![iso(start), iso(end)])?;
    Ok(rows
        .iter()
        .map(|r| Span {
            task_id: r.i("task_id"),
            claude_id: r.st("claude_id"),
            project: r.st("project"),
            tool: r.st("tool"),
            category: r.st("category"),
            fingerprint: r.st("fingerprint"),
            command: r.st("command"),
            seconds: r.f("seconds").unwrap_or(0.0).max(0.0),
            exit_code: r.i("exit_code"),
            timed_out: r.b("timed_out"),
            background: r.b("background"),
        })
        .collect())
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn by_seconds(m: HashMap<String, f64>) -> Vec<Value> {
    let mut v: Vec<(String, f64)> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v.into_iter().map(|(k, s)| json!({"name": k, "seconds": round1(s)})).collect()
}

fn top_category(spans: &[&Span]) -> String {
    let mut m: HashMap<&str, f64> = HashMap::new();
    for s in spans {
        *m.entry(s.category.as_str()).or_default() += s.seconds;
    }
    m.into_iter().max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(a.0))).map(|(k, _)| k.to_string()).unwrap_or_default()
}

/// Each failed command an agent ran again: the runs from the first failure through the run that passed
/// (or the last one), within one conversation.
fn rerun_chains(spans: &[&Span]) -> Vec<Value> {
    let mut runs: BTreeMap<(&str, &str), Vec<&Span>> = BTreeMap::new();
    for s in spans.iter().filter(|s| s.tool == "Bash" && !s.background) {
        runs.entry((s.claude_id.as_str(), s.fingerprint.as_str())).or_default().push(s);
    }
    let mut chains = vec![];
    for list in runs.values() {
        let mut chain: Vec<&Span> = vec![];
        for s in list {
            let failed = s.exit_code.is_some_and(|c| c != 0) || s.timed_out;
            if chain.is_empty() && !failed {
                continue;
            }
            chain.push(s);
            if !failed {
                chains.push(std::mem::take(&mut chain));
            }
        }
        if chain.len() > 1 {
            chains.push(chain);
        }
    }
    let mut out: Vec<Value> = chains
        .into_iter()
        .map(|c| {
            let passed = c.last().is_some_and(|s| s.exit_code == Some(0) && !s.timed_out);
            json!({"command": one_line(&c[0].command, 200), "fingerprint": c[0].fingerprint, "task": rf_opt("task", c[0].task_id),
                   "runs": c.len(), "seconds": round1(c.iter().map(|s| s.seconds).sum()), "passed": passed})
        })
        .collect();
    out.sort_by(|a, b| b["seconds"].as_f64().unwrap_or(0.0).total_cmp(&a["seconds"].as_f64().unwrap_or(0.0)));
    out
}

/// The time tasks spent waiting on the owner (status `needs`, other than lost terminals), within the period.
fn owner_wait(app: &App, start: f64, end: f64) -> Result<Value> {
    let rows = app.db.q(
        "SELECT task_id, at, status, needs_reason FROM task_states WHERE at < ?2 AND task_id IN (
           SELECT task_id FROM task_states WHERE at >= ?1 AND at < ?2
           UNION SELECT s.task_id FROM task_states s WHERE s.at < ?1 AND s.status = 'needs'
             AND s.id = (SELECT x.id FROM task_states x WHERE x.task_id = s.task_id AND x.at < ?1 ORDER BY x.at DESC, x.id DESC LIMIT 1))
         ORDER BY task_id, at, id",
        p![iso(start), iso(end)],
    )?;
    let mut by_reason: HashMap<String, f64> = HashMap::new();
    let mut by_task: HashMap<i64, f64> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        if r.s("status") != Some("needs") || r.s("needs_reason") == Some("lost") {
            continue;
        }
        let Some(from) = r.s("at").and_then(parse_iso) else { continue };
        let task = r.i0("task_id");
        let to = rows.get(i + 1).filter(|n| n.i0("task_id") == task).and_then(|n| n.s("at")).and_then(parse_iso).unwrap_or(end);
        let secs = to.min(end) - from.max(start);
        if secs > 0.0 {
            *by_reason.entry(r.s("needs_reason").filter(|s| !s.is_empty()).unwrap_or("other").to_string()).or_default() += secs;
            *by_task.entry(task).or_default() += secs;
        }
    }
    let total: f64 = by_task.values().sum();
    let mut tasks: Vec<(i64, f64)> = by_task.into_iter().collect();
    tasks.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let tasks: Vec<Value> = tasks.into_iter().take(TOP).map(|(t, s)| json!({"task": rf("task", t), "title": title(app, t), "seconds": round1(s)})).collect();
    Ok(json!({"seconds": round1(total), "reasons": by_seconds(by_reason), "tasks": tasks}))
}

fn title(app: &App, task: i64) -> String {
    app.db.q1("SELECT title FROM tasks WHERE id = ?", p![task]).ok().flatten().map(|r| r.st("title")).unwrap_or_default()
}

/// The review of `start`..`end`: the numbers and the fixes they suggest.
pub fn report(app: &App, start: f64, end: f64) -> Result<Value> {
    let spans = spans_in(app, start, end)?;
    let counted: Vec<&Span> = spans.iter().filter(|s| !WRAPPERS.contains(&s.tool.as_str())).collect();
    let total: f64 = counted.iter().map(|s| s.seconds).sum();

    let mut cats: HashMap<String, f64> = HashMap::new();
    let mut projects: HashMap<String, Vec<&Span>> = HashMap::new();
    let mut tasks: HashMap<i64, Vec<&Span>> = HashMap::new();
    for s in &counted {
        *cats.entry(s.category.clone()).or_default() += s.seconds;
        projects.entry(if s.project.is_empty() { "untracked".into() } else { s.project.clone() }).or_default().push(s);
        if let Some(t) = s.task_id {
            tasks.entry(t).or_default().push(s);
        }
    }
    let mut projects: Vec<Value> = projects
        .into_iter()
        .map(|(name, ss)| {
            let mut c: HashMap<String, f64> = HashMap::new();
            for s in &ss {
                *c.entry(s.category.clone()).or_default() += s.seconds;
            }
            json!({"name": name, "seconds": round1(ss.iter().map(|s| s.seconds).sum()), "categories": by_seconds(c)})
        })
        .collect();
    projects.sort_by(|a, b| b["seconds"].as_f64().unwrap_or(0.0).total_cmp(&a["seconds"].as_f64().unwrap_or(0.0)));

    let mut slow: Vec<(i64, f64, String)> = tasks.iter().map(|(t, ss)| (*t, ss.iter().map(|s| s.seconds).sum(), top_category(ss))).collect();
    slow.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let slow: Vec<Value> = slow
        .into_iter()
        .take(TOP)
        .map(|(t, s, c)| json!({"task": rf("task", t), "title": title(app, t), "seconds": round1(s), "top_category": c}))
        .collect();

    let mut cmds: HashMap<&str, Vec<&Span>> = HashMap::new();
    for s in counted.iter().filter(|s| s.tool == "Bash" && !s.background && !WAITS.contains(&s.category.as_str())) {
        cmds.entry(s.fingerprint.as_str()).or_default().push(s);
    }
    let mut repeated: Vec<Value> = cmds
        .values()
        .filter(|ss| ss.len() >= REPEATED)
        .map(|ss| {
            let mut secs: Vec<f64> = ss.iter().map(|s| s.seconds).collect();
            secs.sort_by(f64::total_cmp);
            json!({"command": one_line(&ss[0].command, 200), "fingerprint": ss[0].fingerprint, "category": ss[0].category, "runs": ss.len(),
                   "median": round1(quantile(&secs, 0.5)), "p90": round1(quantile(&secs, 0.9)), "seconds": round1(secs.iter().sum())})
        })
        .collect();
    repeated.sort_by(|a, b| b["median"].as_f64().unwrap_or(0.0).total_cmp(&a["median"].as_f64().unwrap_or(0.0)));
    repeated.truncate(TOP);

    let mut timeouts: HashMap<&str, Vec<&Span>> = HashMap::new();
    for s in counted.iter().filter(|s| s.timed_out) {
        timeouts.entry(s.fingerprint.as_str()).or_default().push(s);
    }
    let timeout_count: usize = timeouts.values().map(Vec::len).sum();
    let mut timeouts: Vec<Value> = timeouts
        .values()
        .map(|ss| json!({"command": one_line(&ss[0].command, 200), "fingerprint": ss[0].fingerprint, "count": ss.len(), "seconds": round1(ss.iter().map(|s| s.seconds).sum())}))
        .collect();
    timeouts.sort_by(|a, b| b["seconds"].as_f64().unwrap_or(0.0).total_cmp(&a["seconds"].as_f64().unwrap_or(0.0)));

    let reruns = rerun_chains(&counted);
    let wait = owner_wait(app, start, end)?;
    let mut r = json!({
        "period_start": iso(start), "period_end": iso(end), "seconds": round1(total), "spans": counted.len(),
        "categories": by_seconds(cats), "projects": projects, "slowest_tasks": slow, "repeated": repeated,
        "timeouts": {"count": timeout_count, "commands": timeouts}, "reruns": reruns, "owner_wait": wait,
    });
    r["change"] = change(app, &r, start)?;
    r["suggestions"] = json!(suggest(&r));
    Ok(r)
}

fn category_secs(r: &Value, name: &str) -> f64 {
    r["categories"].as_array().into_iter().flatten().find(|c| c["name"] == name).and_then(|c| c["seconds"].as_f64()).unwrap_or(0.0)
}

/// How this period compares with the last review made before it.
fn change(app: &App, r: &Value, start: f64) -> Result<Value> {
    let prev = app.db.q1("SELECT id, data FROM reviews WHERE period_end <= ? ORDER BY period_end DESC, id DESC LIMIT 1", p![iso(start + 3600.0)])?;
    let Some(prev) = prev else { return Ok(Value::Null) };
    let p: Value = serde_json::from_str(&prev.st("data")).unwrap_or(Value::Null);
    let num = |v: &Value, path: &[&str]| path.iter().fold(v, |v, k| &v[*k]).as_f64().unwrap_or(0.0);
    let mut names: Vec<String> = [r, &p].iter().flat_map(|v| v["categories"].as_array().into_iter().flatten()).filter_map(|c| c["name"].as_str().map(str::to_string)).collect();
    names.sort();
    names.dedup();
    let cats: Vec<Value> = names.iter().map(|n| json!({"name": n, "seconds": round1(category_secs(r, n) - category_secs(&p, n))})).collect();
    Ok(json!({
        "review": prev.id(),
        "seconds": round1(num(r, &["seconds"]) - num(&p, &["seconds"])),
        "owner_wait": round1(num(r, &["owner_wait", "seconds"]) - num(&p, &["owner_wait", "seconds"])),
        "timeouts": num(r, &["timeouts", "count"]) as i64 - num(&p, &["timeouts", "count"]) as i64,
        "reruns": r["reruns"].as_array().map_or(0, Vec::len) as i64 - p["reruns"].as_array().map_or(0, Vec::len) as i64,
        "categories": cats,
    }))
}

pub fn hours_text(secs: f64) -> String {
    let h = secs / 3600.0;
    if h >= 1.0 {
        format!("{} h", round1(h))
    } else {
        format!("{} min", (secs / 60.0).round() as i64)
    }
}

/// The fixes the numbers point at, biggest first: each `{kind, title, detail}`, ready to become a backlog issue.
pub fn suggest(r: &Value) -> Vec<Value> {
    let total = r["seconds"].as_f64().unwrap_or(0.0);
    let mut out: Vec<(f64, Value)> = vec![];
    if total <= 0.0 {
        return vec![];
    }
    for (cat, what) in [("build", "Builds"), ("test", "Tests")] {
        let s = category_secs(r, cat);
        if s / total >= 0.3 && s >= 3600.0 {
            out.push((s, json!({"kind": "follow", "title": format!("{what} take {:.0}% of agent time", s / total * 100.0),
                "detail": format!("{what} ran {} of {} this period. Look at what rebuilds from cold, and builds that run at the same time.", hours_text(s), hours_text(total))})));
        }
    }
    let waits = category_secs(r, "load wait") + category_secs(r, "background poll");
    if waits / total >= 0.1 && waits >= 1800.0 {
        out.push((waits, json!({"kind": "follow", "title": "Agents spend a lot of time waiting and polling",
            "detail": format!("{} went to load waits and polling background commands ({:.0}% of agent time).", hours_text(waits), waits / total * 100.0)})));
    }
    for c in r["repeated"].as_array().into_iter().flatten() {
        let (p90, secs) = (c["p90"].as_f64().unwrap_or(0.0), c["seconds"].as_f64().unwrap_or(0.0));
        if p90 >= 300.0 && secs >= 1800.0 {
            let cmd = one_line(c["command"].as_str().unwrap_or(""), 80);
            out.push((secs, json!({"kind": "follow", "title": format!("Speed up a slow repeated command ({})", c["category"].as_str().unwrap_or("other")),
                "detail": format!("`{cmd}` ran {} times: median {}, p90 {}, {} in all.", c["runs"], hours_text(c["median"].as_f64().unwrap_or(0.0)), hours_text(p90), hours_text(secs))})));
        }
    }
    for c in r["timeouts"]["commands"].as_array().into_iter().flatten() {
        if c["count"].as_i64().unwrap_or(0) >= 2 {
            let cmd = one_line(c["command"].as_str().unwrap_or(""), 80);
            out.push((c["seconds"].as_f64().unwrap_or(0.0), json!({"kind": "bug", "title": "A command keeps timing out",
                "detail": format!("`{cmd}` timed out {} times ({}). Run it in the background or give it a longer timeout.", c["count"], hours_text(c["seconds"].as_f64().unwrap_or(0.0)))})));
        }
    }
    let mut chains: HashMap<&str, (usize, f64, &str)> = HashMap::new();
    for c in r["reruns"].as_array().into_iter().flatten() {
        let e = chains.entry(c["fingerprint"].as_str().unwrap_or("")).or_insert((0, 0.0, c["command"].as_str().unwrap_or("")));
        e.0 += 1;
        e.1 += c["seconds"].as_f64().unwrap_or(0.0);
    }
    for (n, secs, cmd) in chains.values() {
        if *n >= 3 && *secs >= 1200.0 {
            out.push((*secs, json!({"kind": "follow", "title": "A command often fails and gets rerun",
                "detail": format!("`{}` failed and was rerun {n} times, {} in all.", one_line(cmd, 80), hours_text(*secs))})));
        }
    }
    let wait = r["owner_wait"]["seconds"].as_f64().unwrap_or(0.0);
    if wait >= 4.0 * 3600.0 {
        let why = r["owner_wait"]["reasons"].as_array().and_then(|v| v.first()).and_then(|c| c["name"].as_str()).unwrap_or("other").to_string();
        out.push((wait, json!({"kind": "gap", "title": "Tasks wait a long time on the owner",
            "detail": format!("Tasks waited {} on you, most for {why}.", hours_text(wait))})));
    }
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
    out.into_iter().map(|(_, v)| v).take(TOP).collect()
}

/// Saves the review of `start`..`end` and gives its id.
pub fn save(app: &App, start: f64, end: f64, scheduled: bool) -> Result<i64> {
    let r = report(app, start, end)?;
    app.db.x(
        "INSERT INTO reviews(period_start, period_end, created_at, scheduled, data) VALUES(?, ?, ?, ?, ?)",
        p![iso(start), iso(end), now_iso(), scheduled as i64, jdumps(&r)],
    )
}

/// How many fixes a review suggests: the ones Claude filed and linked once it has, else the rules' own.
fn fix_count(data: &Value, fixes: &Value) -> usize {
    if fixes["state"] == "done" {
        return fixes["filed"].as_array().map_or(0, Vec::len) + fixes["linked"].as_array().map_or(0, Vec::len);
    }
    data["suggestions"].as_array().map_or(0, Vec::len)
}

/// "Weekly review: 12.5 h on builds, 3 fixes suggested".
pub fn headline(r: &Value, fixes: &Value) -> String {
    let builds = round1(category_secs(r, "build") / 3600.0);
    let n = fix_count(r, fixes);
    format!("Weekly review: {builds} h on builds, {n} {} suggested", if n == 1 { "fix" } else { "fixes" })
}

fn key(id: i64) -> String {
    format!("review:{id}")
}

fn notification_id(id: i64) -> String {
    format!("taskboard-review-{id}")
}

/// Tells the owner review `id` is ready: a notification that opens it, and a Needs you item that stays until it's opened.
pub fn announce(app: &App, id: i64) -> Result<()> {
    let r = get(app, id)?;
    let text = headline(&r["data"], &r["fixes"]);
    let url = dispatch::page_link(app, &format!("review/{id}"));
    dispatch::add_quiet_alert(app, &text, &key(id), json!({"review_id": id, "url": url}))?;
    if app.cfg.notify && app.cfg.runner {
        let params = json!({"title": text, "body": "Task board", "category": "board", "sound": true, "open": url, "id": notification_id(id)});
        let m = app.cfg.midna.clone();
        std::thread::spawn(move || {
            let _ = midna::call_exe(&m, "notify.send", &params, 30.0);
        });
    }
    Ok(())
}

fn card(r: &crate::util::Row) -> Value {
    let data: Value = serde_json::from_str(&r.st("data")).unwrap_or(Value::Null);
    let fixes: Value = r.s("fixes").and_then(|s| serde_json::from_str(s).ok()).unwrap_or_else(|| json!({"state": "none"}));
    json!({"id": r.id(), "period_start": r.s("period_start"), "period_end": r.s("period_end"), "created_at": r.s("created_at"),
           "scheduled": r.b("scheduled"), "seen_at": r.s("seen_at"), "headline": headline(&data, &fixes),
           "seconds": data["seconds"], "suggestions": fix_count(&data, &fixes), "fixes": fixes})
}

/// Every kept review, newest first.
pub fn list(app: &App) -> Result<Value> {
    let rows = app.db.q("SELECT * FROM reviews ORDER BY id DESC", p![])?;
    Ok(json!({"reviews": rows.iter().map(card).collect::<Vec<_>>()}))
}

pub fn get(app: &App, id: i64) -> Result<Value> {
    let Some(r) = app.db.q1("SELECT * FROM reviews WHERE id = ?", p![id])? else { return err(404, "There's no review with that id.") };
    let mut v = card(&r);
    v["data"] = serde_json::from_str(&r.st("data")).unwrap_or(Value::Null);
    v["fix_issues"] = json!(crate::review_fixes::issues(app, id)?);
    Ok(v)
}

/// The owner opened review `id`: its Needs you item and notification go.
pub fn seen(app: &App, id: i64) -> Result<Value> {
    get(app, id)?;
    app.db.x("UPDATE reviews SET seen_at = COALESCE(seen_at, ?) WHERE id = ?", p![now_iso(), id])?;
    dispatch::clear_alert_key(app, &key(id))?;
    dispatch::withdraw_id(app, &notification_id(id));
    get(app, id)
}

/// One task's spans in order (`tb review T12`), with its time by category.
pub fn task_spans(app: &App, task: i64) -> Result<Value> {
    let rows = app.db.q("SELECT * FROM spans WHERE task_id = ? ORDER BY started_at, id", p![task])?;
    let mut cats: HashMap<String, f64> = HashMap::new();
    let spans: Vec<Value> = rows
        .iter()
        .map(|r| {
            if !WRAPPERS.contains(&r.st("tool").as_str()) {
                *cats.entry(r.st("category")).or_default() += r.f("seconds").unwrap_or(0.0);
            }
            json!({"started_at": r.s("started_at"), "tool": r.s("tool"), "category": r.s("category"), "command": one_line(&r.st("command"), 200),
                   "seconds": round1(r.f("seconds").unwrap_or(0.0)), "exit_code": r.i("exit_code"), "timed_out": r.b("timed_out"),
                   "background": r.b("background"), "refused": r.b("refused"), "subagent": r.b("subagent"), "load": r.f("load")})
        })
        .collect();
    let total: f64 = cats.values().sum();
    Ok(json!({"task": rf("task", task), "title": title(app, task), "seconds": round1(total), "categories": by_seconds(cats), "spans": spans}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(d: &str, t: &str) -> NaiveDateTime {
        NaiveDateTime::new(NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap(), NaiveTime::parse_from_str(t, "%H:%M").unwrap())
    }

    #[test]
    fn schedule_lands_on_the_last_friday_at_five() {
        let c = ReviewConfig::default();
        assert_eq!(due_at(&c, at("2026-10-16", "17:00")), Some(at("2026-10-16", "17:00")));
        assert_eq!(due_at(&c, at("2026-10-16", "16:59")), Some(at("2026-10-09", "17:00")));
        assert_eq!(due_at(&c, at("2026-10-14", "09:00")), Some(at("2026-10-09", "17:00")));
        let off = ReviewConfig { cadence: "off".into(), ..ReviewConfig::default() };
        assert_eq!(due_at(&off, at("2026-10-16", "18:00")), None);
        let mon = ReviewConfig { day: "mon".into(), at: "9am".into(), ..ReviewConfig::default() };
        assert_eq!(due_at(&mon, at("2026-10-14", "09:00")), Some(at("2026-10-12", "09:00")));
    }

    #[test]
    fn quantiles_pick_the_nearest_rank() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        assert_eq!(quantile(&v, 0.5), 6.0);
        assert_eq!(quantile(&v, 0.9), 9.0);
        assert_eq!(quantile(&[], 0.5), 0.0);
    }

    #[test]
    fn headline_counts_build_hours_and_fixes() {
        let r = json!({"categories": [{"name": "build", "seconds": 45000.0}], "suggestions": [{}, {}, {}]});
        let none = json!({"state": "none"});
        assert_eq!(headline(&r, &none), "Weekly review: 12.5 h on builds, 3 fixes suggested");
        assert_eq!(headline(&json!({}), &none), "Weekly review: 0 h on builds, 0 fixes suggested");
        let done = json!({"state": "done", "filed": ["B1"], "linked": []});
        assert_eq!(headline(&r, &done), "Weekly review: 12.5 h on builds, 1 fix suggested");
    }
}
