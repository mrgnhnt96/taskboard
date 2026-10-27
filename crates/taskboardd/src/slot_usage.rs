//! How busy the worktree slot pool (`slots.rs`) was, for the Days page. Each minute the runner notes
//! which projects' pools are full, which tasks wait for a slot and which hold one while their terminal
//! isn't working, as stretches in `slot_spans`. A full pool is the pool working as meant, so this only
//! reports it.

use std::collections::{BTreeMap, HashSet};

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, p, slots};

/// How often the runner samples the pool.
pub const EVERY_SECS: f64 = 60.0;
/// A stretch that ended this recently goes on, so a late sample doesn't split it.
const JOIN_SECS: f64 = 3.0 * EVERY_SECS;
/// An idle hold shorter than this on a day isn't listed: a terminal sits idle between turns.
const IDLE_SHOWN_MIN: f64 = 10.0;

/// Note the pool's state now.
pub fn sample(app: &App) -> Result<()> {
    sample_at(app, now_ts())
}

fn sample_at(app: &App, now: f64) -> Result<()> {
    let mut seen: Vec<(String, &str, Option<i64>)> = vec![];
    for repo in app.db.q("SELECT DISTINCT repo FROM worktree_slots ORDER BY repo", p![])? {
        let slots = slots::rows(app, &repo.st("repo"))?;
        let project = slots.iter().find_map(|s| s.s("project").filter(|p| !p.is_empty())).unwrap_or("").to_string();
        let mut held = 0;
        for s in &slots {
            let Some(t) = slots::holder(app, s)? else { continue };
            held += 1;
            let session = board::get_session(app, t.s("session_id"))?;
            if session.as_ref().and_then(|s| s.s("status")) != Some("working") {
                seen.push((project.clone(), "idle", Some(t.id())));
            }
        }
        if held > 0 && held == slots.len() && slots.len() as i64 >= slots::config(app, Some(&project)).max_slots {
            seen.push((project, "full", None));
        }
    }
    for t in app.db.q("SELECT * FROM tasks WHERE status = 'queued' ORDER BY id", p![])? {
        if slots::blocker(app, &t)?.is_some() {
            seen.push((t.st("project"), "wait", Some(t.id())));
        }
    }
    for (project, kind, task) in seen {
        note(app, &project, kind, task, now)?;
    }
    Ok(())
}

/// Carry on the stretch of `kind` that ended in the last [`JOIN_SECS`], or start one.
fn note(app: &App, project: &str, kind: &str, task: Option<i64>, now: f64) -> Result<()> {
    let open = app.db.q1(
        "SELECT id FROM slot_spans WHERE project = ? AND kind = ? AND task_id IS ? AND to_at >= ? ORDER BY to_at DESC LIMIT 1",
        p![project, kind, task, iso(now - JOIN_SECS)],
    )?;
    match open {
        Some(r) => {
            app.db.changed("UPDATE slot_spans SET to_at = ? WHERE id = ?", p![iso(now), r.id()])?;
        }
        None => {
            app.db.insert("slot_spans", vec![("project", json!(project)), ("kind", json!(kind)), ("task_id", json!(task)), ("from_at", json!(iso(now))), ("to_at", json!(iso(now)))])?;
        }
    }
    Ok(())
}

fn mins(secs: f64) -> f64 {
    (secs / 60.0 * 10.0).round() / 10.0
}

#[derive(Default)]
struct Pool {
    full: Vec<(f64, f64)>,
    waits: BTreeMap<i64, f64>,
    idle: BTreeMap<i64, f64>,
}

/// Each project's pool between `from` and `to`, leaving out `hide` and projects with nothing to show:
/// `[{project, max_slots, full: [{from, to}], full_times, full_min, waited, wait_min,
/// waits: [{task, ref, title, min}], idle: [{task, ref, title, min}]}]`, the tasks longest first.
pub fn day(app: &App, from: f64, to: f64, hide: &HashSet<String>, no_project: &str) -> Result<Value> {
    let rows = app.db.q("SELECT project, kind, task_id, from_at, to_at FROM slot_spans WHERE to_at >= ? AND from_at < ? ORDER BY from_at, id", p![iso(from), iso(to)])?;
    let mut pools: BTreeMap<String, Pool> = BTreeMap::new();
    for r in &rows {
        let (Some(x), Some(y)) = (r.s("from_at").and_then(parse_iso), r.s("to_at").and_then(parse_iso)) else { continue };
        let (a, b) = (x.max(from), y.min(to));
        // A stretch that only touches the day's edge isn't on it; a single sample (no length) is.
        if b < a || (b == a && y > x) {
            continue;
        }
        let project = r.s("project").filter(|p| !p.is_empty()).unwrap_or(no_project).to_string();
        if hide.contains(&project) {
            continue;
        }
        let pool = pools.entry(project).or_default();
        match (r.s("kind"), r.i("task_id")) {
            (Some("full"), _) => pool.full.push((a, b)),
            (Some("wait"), Some(t)) => *pool.waits.entry(t).or_default() += b - a,
            (Some("idle"), Some(t)) => *pool.idle.entry(t).or_default() += b - a,
            _ => {}
        }
    }
    let mut out = vec![];
    for (project, mut pool) in pools {
        pool.idle.retain(|_, secs| mins(*secs) >= IDLE_SHOWN_MIN);
        if pool.full.is_empty() && pool.waits.is_empty() && pool.idle.is_empty() {
            continue;
        }
        let tasks = |by: &BTreeMap<i64, f64>| -> Result<Vec<Value>> {
            let mut v: Vec<(i64, f64)> = by.iter().map(|(t, s)| (*t, *s)).collect();
            v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            v.into_iter()
                .map(|(t, secs)| {
                    let title = board::find_task(app, Some(t))?.map(|r| r.st("title")).unwrap_or_default();
                    Ok(json!({"task": t, "ref": rf("task", t), "title": title, "min": mins(secs)}))
                })
                .collect()
        };
        out.push(json!({
            "project": project,
            "max_slots": slots::config(app, Some(&project)).max_slots,
            "full": pool.full.iter().map(|(a, b)| json!({"from": iso(*a), "to": iso(*b)})).collect::<Vec<_>>(),
            "full_times": pool.full.len(),
            "full_min": mins(pool.full.iter().map(|(a, b)| b - a).sum()),
            "waited": pool.waits.len(),
            "wait_min": mins(pool.waits.values().sum()),
            "waits": tasks(&pool.waits)?,
            "idle": tasks(&pool.idle)?,
        }));
    }
    Ok(json!(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(dir: &std::path::Path) -> std::sync::Arc<App> {
        App::for_tests(crate::config::Config::for_tests(dir))
    }

    fn spans(app: &App) -> Vec<(String, String, Option<i64>, f64)> {
        app.db
            .q("SELECT * FROM slot_spans ORDER BY id", p![])
            .unwrap()
            .iter()
            .map(|r| {
                let len = r.s("to_at").and_then(parse_iso).unwrap() - r.s("from_at").and_then(parse_iso).unwrap();
                (r.st("project"), r.st("kind"), r.i("task_id"), len)
            })
            .collect()
    }

    #[test]
    fn a_stretch_goes_on_while_samples_keep_coming_and_starts_again_after_a_gap() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        let t0 = 1_800_000_000.0;
        for k in 0..5 {
            note(&app, "web", "full", None, t0 + k as f64 * EVERY_SECS).unwrap();
        }
        note(&app, "web", "wait", Some(7), t0).unwrap();
        note(&app, "web", "wait", Some(8), t0).unwrap();
        note(&app, "web", "full", None, t0 + 4.0 * EVERY_SECS + JOIN_SECS + 1.0).unwrap();
        assert_eq!(
            spans(&app),
            vec![
                ("web".into(), "full".into(), None, 240.0),
                ("web".into(), "wait".into(), Some(7), 0.0),
                ("web".into(), "wait".into(), Some(8), 0.0),
                ("web".into(), "full".into(), None, 0.0),
            ]
        );
    }

    #[test]
    fn a_full_pool_and_its_idle_holder_are_noted() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        app.db.x("INSERT INTO sessions(id, status) VALUES('s1', 'working'), ('s2', 'idle')", p![]).unwrap();
        let busy = app.db.x("INSERT INTO tasks(title, status, project, session_id) VALUES('Busy', 'working', 'web', 's1')", p![]).unwrap();
        let idle = app.db.x("INSERT INTO tasks(title, status, project, session_id) VALUES('Idle', 'needs', 'web', 's2')", p![]).unwrap();
        let max = slots::config(&app, Some("web")).max_slots;
        for n in 1..=max {
            let holder = if n == 1 { idle } else { busy };
            app.db.x("INSERT INTO worktree_slots(repo, n, path, project, task_id) VALUES('/r', ?, ?, 'web', ?)", p![n, format!("/r/slot-{n}"), holder]).unwrap();
        }
        sample_at(&app, 1_800_000_000.0).unwrap();
        let got = spans(&app);
        assert!(got.contains(&("web".into(), "full".into(), None, 0.0)), "{got:?}");
        assert!(got.contains(&("web".into(), "idle".into(), Some(idle), 0.0)), "{got:?}");
        assert!(!got.iter().any(|s| s.2 == Some(busy) && s.1 == "idle"), "a working holder isn't idle: {got:?}");

        app.db.x("UPDATE worktree_slots SET task_id = NULL WHERE n = 1", p![]).unwrap();
        app.db.x("DELETE FROM slot_spans", p![]).unwrap();
        sample_at(&app, 1_800_000_000.0).unwrap();
        assert!(!spans(&app).iter().any(|s| s.1 == "full"), "a free slot means the pool isn't full");
    }

    #[test]
    fn the_day_adds_up_each_project_s_stretches_cut_to_the_day() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        let a = app.db.x("INSERT INTO tasks(title, status, project) VALUES('Waiter', 'working', 'web')", p![]).unwrap();
        let b = app.db.x("INSERT INTO tasks(title, status, project) VALUES('Sleeper', 'needs', 'web')", p![]).unwrap();
        let from = 1_800_000_000.0;
        let at = |h: f64| from + h * 3600.0;
        for (project, kind, task, x, y) in [
            ("web", "full", None, -1.0, 0.5),
            ("web", "full", None, 10.0, 10.25),
            ("web", "wait", Some(a), 10.0, 10.5),
            ("web", "idle", Some(b), 9.0, 11.0),
            ("web", "idle", Some(a), 12.0, 12.1),
            ("api", "wait", Some(a), 8.0, 9.0),
            ("web", "full", None, 30.0, 31.0),
        ] {
            app.db.x("INSERT INTO slot_spans(project, kind, task_id, from_at, to_at) VALUES(?, ?, ?, ?, ?)", p![project, kind, task, iso(at(x)), iso(at(y))]).unwrap();
        }
        let hide: HashSet<String> = ["api".to_string()].into();
        let v = day(&app, from, at(24.0), &hide, "Other").unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1, "a hidden project has no row");
        let w = &v[0];
        assert_eq!(w["project"], "web");
        assert_eq!(w["full_times"], 2);
        assert_eq!(w["full_min"], 45.0, "the one from the night before counts from midnight");
        assert_eq!(w["waited"], 1);
        assert_eq!(w["wait_min"], 30.0);
        assert_eq!(w["waits"][0]["ref"], rf("task", a));
        assert_eq!(w["idle"].as_array().unwrap().len(), 1, "a short idle hold isn't listed");
        assert_eq!(w["idle"][0]["title"], "Sleeper");
        assert_eq!(w["idle"][0]["min"], 120.0);

        let now = day(&app, from, at(10.1), &hide, "Other").unwrap();
        assert_eq!(now[0]["full_min"], 36.0, "today stops at now");
        assert!(day(&app, at(48.0), at(72.0), &hide, "Other").unwrap().as_array().unwrap().is_empty());
    }
}
