//! Started work that has gone quiet: a task that's working, back in the queue after it started, or
//! done with its PR still open, with nothing in its history for `after_hours`. A done task's open PR
//! has no terminal for the check-ins to read, so without this it can sit for weeks unseen. Each one
//! raises a `quiet:T<n>` alert, taken down when the task moves or stops being open work. Dismissed or
//! pushed out by the alert cap while still quiet, it comes back after another `after_hours`.
//!
//! Tasks in paused, set-aside or archived goals are left alone: nobody expects them to move.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, dispatch, p};

/// `[quiet]` in config.toml.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct QuietConfig {
    pub on: bool,
    /// How long started work can go without an update before the owner hears of it.
    pub after_hours: u64,
}

impl Default for QuietConfig {
    fn default() -> Self {
        QuietConfig { on: true, after_hours: 48 }
    }
}

const TICK_SECS: f64 = 600.0;
const KEY_PREFIX: &str = "quiet:";
/// When each quiet task's alert was last raised, by ref.
const RAISED: &str = "quiet_raised";
const OPEN_WORK: &str = "SELECT t.* FROM tasks t LEFT JOIN goals g ON g.id = t.goal_id \
     WHERE t.status IN ('working', 'queued', 'done') \
     AND (t.status <> 'done' OR UPPER(COALESCE(t.pr_state, 'OPEN')) = 'OPEN') \
     AND COALESCE(g.paused, 0) = 0 AND COALESCE(g.deprioritized, 0) = 0 AND COALESCE(g.archived, 0) = 0";

pub fn run(app: std::sync::Arc<App>) {
    while !app.stopping() {
        if app.cfg.quiet.on {
            if let Err(e) = tick(&app) {
                app.info(format!("quiet: {e}"));
            }
        }
        app.sleep(TICK_SECS);
    }
}

pub fn is_key(key: &str) -> bool {
    key.starts_with(KEY_PREFIX)
}

fn key(task: &Row) -> String {
    format!("{KEY_PREFIX}{}", rf("task", task.id()))
}

/// A task in the queue that waits on other work isn't quiet: what it waits on is.
fn started(t: &Row) -> bool {
    match t.s("status") {
        Some("working") => true,
        Some("queued") => t.s("started_at").is_some() && t.s("waits_for").is_none_or(|w| w.is_empty() || w == "[]"),
        Some("done") => !t.b("failed") && board::pr_still_open(t),
        _ => false,
    }
}

fn last_update(app: &App, t: &Row) -> Result<Option<f64>> {
    let at = app.db.val("SELECT MAX(at) FROM events WHERE task_id = ? AND kind <> 'release'", p![t.id()])?;
    Ok(at.as_str().or(t.s("updated_at")).and_then(parse_iso))
}

fn quiet_for(after_hours: u64, last: f64, now: f64) -> bool {
    now - last >= after_hours as f64 * 3600.0
}

fn tick(app: &App) -> Result<()> {
    let after = app.cfg.quiet.after_hours;
    let now = now_ts();
    let before = jloads_obj(crate::reqcache::setting(app, RAISED).ok().flatten().as_deref());
    let mut raised = Row::new();
    let up = dispatch::alerts(app);
    for t in app.db.q(OPEN_WORK, p![])?.iter().filter(|t| started(t)) {
        let Some(last) = last_update(app, t)?.filter(|l| quiet_for(after, *l, now)) else { continue };
        let r = rf("task", t.id());
        let when = before.get(&r).and_then(Value::as_str).and_then(parse_iso);
        let shown = up.iter().any(|a| a["task_id"].as_i64() == Some(t.id()));
        if shown || when.is_some_and(|w| !quiet_for(after, w, now)) {
            if let Some(w) = before.get(&r) {
                raised.insert(r, w.clone());
            }
            continue;
        }
        raise(app, t, last)?;
        raised.insert(r, json!(iso(now)));
    }
    if raised != before {
        crate::reqcache::forget_setting(RAISED);
        app.db.set_setting(RAISED, Some(&jdumps(&Value::Object(raised))))?;
    }
    Ok(())
}

fn raise(app: &App, t: &Row, last: f64) -> Result<()> {
    let since = local_dt(last).format("%b %-d");
    let what = match t.i("pr_num") {
        Some(n) if t.s("status") == Some("done") => format!("PR #{n} is open with no update since {since}"),
        _ => format!("No update since {since}"),
    };
    let title = t.st("title");
    let text = format!("{}: {title}. {what}.", rf("task", t.id()));
    let fields = json!({"title": title, "detail": what});
    dispatch::add_alert_keyed_with(app, &text, Some(t.id()), t.i("goal_id"), &key(t), fields)
}

/// Whether alert `a`, a quiet task's, can come down: the task moved, or it isn't open work now.
pub fn alert_resolved(app: &App, a: &Value) -> Result<bool> {
    let Some(id) = a["task_id"].as_i64() else { return Ok(true) };
    let open = app.db.q(&format!("{OPEN_WORK} AND t.id = ?"), p![id])?;
    let Some(t) = open.iter().find(|t| started(t)) else { return Ok(true) };
    let at = a["at"].as_str().and_then(parse_iso).unwrap_or(0.0);
    Ok(last_update(app, t)?.is_some_and(|last| last > at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::fields;

    const OLD: &str = "2026-01-01T12:00:00Z";

    fn app(dir: &std::path::Path) -> std::sync::Arc<App> {
        App::for_tests(Config::for_tests(dir))
    }

    fn task(app: &App, f: Vec<(&str, Value)>) -> i64 {
        let mut all = fields!["title" => "Fix the form", "updated_at" => OLD];
        all.extend(f);
        let id = app.db.insert("tasks", all).unwrap();
        event(app, id, OLD, "status");
        id
    }

    fn event(app: &App, id: i64, at: &str, kind: &str) {
        app.db.insert("events", fields!["task_id" => id, "at" => at, "who" => "Terminal", "kind" => kind, "text" => "x"]).unwrap();
    }

    fn open_pr(app: &App) -> i64 {
        task(app, fields!["status" => "done", "pr_num" => 7, "pr_state" => "OPEN", "pr_phase" => "merge"])
    }

    fn quiet_alerts(app: &App) -> Vec<Value> {
        dispatch::alerts(app).into_iter().filter(|a| a["key"].as_str().is_some_and(is_key)).collect()
    }

    #[test]
    fn an_open_pr_with_no_update_reaches_the_owner_and_survives_the_prune() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        open_pr(&app);
        tick(&app).unwrap();
        dispatch::prune_alerts(&app).unwrap();
        let up = quiet_alerts(&app);
        assert_eq!(up.len(), 1);
        assert!(up[0]["detail"].as_str().unwrap().starts_with("PR #7 is open with no update since"), "{up:?}");
    }

    #[test]
    fn started_work_counts_and_unstarted_or_settled_work_does_not() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        task(&app, fields!["status" => "working"]);
        task(&app, fields!["status" => "queued", "started_at" => OLD]);
        task(&app, fields!["status" => "queued"]);
        task(&app, fields!["status" => "queued", "started_at" => OLD, "waits_for" => "[3]"]);
        task(&app, fields!["status" => "planned"]);
        task(&app, fields!["status" => "needs"]);
        task(&app, fields!["status" => "done", "pr_num" => 8, "pr_state" => "MERGED"]);
        task(&app, fields!["status" => "done"]);
        tick(&app).unwrap();
        assert_eq!(quiet_alerts(&app).len(), 2);
    }

    #[test]
    fn a_recent_update_keeps_it_off_the_list() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let id = open_pr(&app);
        event(&app, id, &now_iso(), "status");
        tick(&app).unwrap();
        assert!(quiet_alerts(&app).is_empty());
    }

    #[test]
    fn paused_and_set_aside_goals_stay_quiet() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        for f in [fields!["name" => "Paused", "paused" => 1], fields!["name" => "Aside", "deprioritized" => 1]] {
            let g = app.db.insert("goals", f).unwrap();
            task(&app, fields!["status" => "working", "goal_id" => g]);
        }
        tick(&app).unwrap();
        assert!(quiet_alerts(&app).is_empty());
    }

    #[test]
    fn it_comes_down_when_the_task_moves() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let id = open_pr(&app);
        tick(&app).unwrap();
        let a = quiet_alerts(&app).remove(0);
        assert!(!alert_resolved(&app, &a).unwrap());
        event(&app, id, &iso(now_ts() + 60.0), "status");
        assert!(alert_resolved(&app, &a).unwrap());
    }

    #[test]
    fn a_dismissed_one_waits_a_full_window_before_it_comes_back() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let id = open_pr(&app);
        tick(&app).unwrap();
        dispatch::clear_alert_key(&app, &key(&board::get_task(&app, id).unwrap())).unwrap();
        tick(&app).unwrap();
        assert!(quiet_alerts(&app).is_empty(), "too soon");
        let r = rf("task", id);
        crate::reqcache::forget_setting(RAISED);
        app.db.set_setting(RAISED, Some(&jdumps(&json!({ r: OLD })))).unwrap();
        tick(&app).unwrap();
        assert_eq!(quiet_alerts(&app).len(), 1);
    }

    #[test]
    fn a_task_already_on_the_list_gets_no_second_alert() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let id = open_pr(&app);
        dispatch::add_alert_keyed(&app, "Waiting on review", Some(id), None, "other").unwrap();
        tick(&app).unwrap();
        assert!(quiet_alerts(&app).is_empty());
    }
}
