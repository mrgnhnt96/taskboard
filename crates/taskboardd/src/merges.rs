//! Every PR the board saw merge, with the day it merged and the lines it added and removed, for the
//! Days page's "PRs merged" and lines changed.
//!
//! A trigger keeps `pr_merges`: whichever way a PR merges (the poll reads it merged, `tb pr merge`,
//! the merge button), the task's `pr_phase` turns `merged` and the row is written then. The line
//! counts come later from the PR's host, a few PRs at a time, so the read never holds a merge up.

use std::collections::{BTreeMap, HashSet};

use chrono::NaiveDate;
use serde_json::{json, Value};

use crate::app::App;
use crate::prhost::{self, PrRef};
use crate::util::*;
use crate::p;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS pr_merges(
  id INTEGER PRIMARY KEY, task_id INT NOT NULL, pr_num INT NOT NULL, project TEXT, merged_at TEXT NOT NULL,
  added INT, removed INT, tries INT NOT NULL DEFAULT 0, UNIQUE(task_id, pr_num));
CREATE INDEX IF NOT EXISTS pr_merges_at ON pr_merges(merged_at);
CREATE TRIGGER IF NOT EXISTS pr_merges_insert AFTER UPDATE OF pr_phase ON tasks
  WHEN NEW.pr_phase = 'merged' AND OLD.pr_phase IS NOT 'merged' AND NEW.pr_num IS NOT NULL
BEGIN
  INSERT OR IGNORE INTO pr_merges(task_id, pr_num, project, merged_at)
    VALUES(NEW.id, NEW.pr_num, NEW.project, COALESCE(NEW.updated_at, strftime('%Y-%m-%dT%H:%M:%SZ', 'now')));
END;
"#;

/// PRs merged before `pr_merges` existed: merged when the task log says so, else when the task
/// finished. Only into an empty table, so merges the cleanup dropped don't come back on the next start.
pub const BACKFILL: &str = r#"
INSERT OR IGNORE INTO pr_merges(task_id, pr_num, project, merged_at)
  SELECT t.id, t.pr_num, t.project,
    COALESCE((SELECT MIN(e.at) FROM events e WHERE e.task_id = t.id AND (e.text = 'PR #' || t.pr_num || ' was merged'
      OR e.text = 'Merged PR #' || t.pr_num OR e.text LIKE 'Merged PR #' || t.pr_num || ' %')),
      t.finished_at, t.updated_at, t.created_at)
  FROM tasks t
  WHERE t.pr_phase = 'merged' AND t.pr_num IS NOT NULL AND NOT EXISTS (SELECT 1 FROM pr_merges)
    AND COALESCE(t.finished_at, t.updated_at, t.created_at) IS NOT NULL;
"#;

/// Reads of a PR's line counts that may fail before the board stops asking.
const TRIES: i64 = 3;
/// PRs counted per pass, newest first.
const PER_PASS: i64 = 10;
const PASS_SECS: f64 = 5.0 * 60.0;

/// Reads the line counts of up to `n` merged PRs that have none yet, newest first. A host that is
/// holding calls back is skipped, and a read that fails while it does doesn't count as a try.
/// How many it counted.
pub fn fill(app: &App, n: i64) -> Result<usize> {
    let rows = app.db.q(
        "SELECT m.id, m.pr_num, t.pr_host, t.pr_repo, t.pr_url FROM pr_merges m JOIN tasks t ON t.id = m.task_id
         WHERE m.added IS NULL AND m.tries < ? ORDER BY m.merged_at DESC, m.id DESC LIMIT ?",
        p![TRIES, n],
    )?;
    let mut counted = 0;
    for r in &rows {
        if app.stopping() {
            break;
        }
        let pr = PrRef { host: r.st("pr_host"), repo: r.st("pr_repo"), num: r.i0("pr_num"), url: r.st("pr_url") };
        let tried = |app: &App| app.db.x("UPDATE pr_merges SET tries = tries + 1 WHERE id = ?", p![r.id()]);
        let Ok(host) = prhost::host_for(app, &pr.host) else {
            tried(app)?;
            continue;
        };
        if host.held_until().is_some() {
            continue;
        }
        match host.lines(&pr) {
            Ok((added, removed)) => {
                app.db.x("UPDATE pr_merges SET added = ?, removed = ? WHERE id = ?", p![added, removed, r.id()])?;
                counted += 1;
            }
            Err(_) if host.held_until().is_some() => {}
            Err(_) => {
                tried(app)?;
            }
        }
    }
    Ok(counted)
}

pub fn fill_loop(app: std::sync::Arc<App>) {
    while !app.stopping() {
        if let Err(e) = fill(&app, PER_PASS) {
            app.info(format!("counting merged PRs' lines failed: {}", e.message));
        }
        app.sleep(PASS_SECS);
    }
}

/// One day's merged PRs, all visible projects together.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Merged {
    pub prs: i64,
    pub added: i64,
    pub removed: i64,
    /// PRs whose line counts the host gave.
    pub counted: i64,
}

impl Merged {
    /// `merged`, and `added` and `removed`, which are null while no PR of the day has its counts.
    pub fn put(&self, v: &mut Value) {
        let known = |n: i64| if self.counted > 0 { json!(n) } else { Value::Null };
        v["merged"] = json!(self.prs);
        v["added"] = known(self.added);
        v["removed"] = known(self.removed);
    }
}

/// The PRs merged from `from` up to `to` (timestamps), by the local day they merged, leaving out
/// `hide`'s projects (a PR with no project is `no_project`'s).
pub fn by_day(app: &App, from: f64, to: f64, hide: &HashSet<String>, no_project: &str) -> Result<BTreeMap<NaiveDate, Merged>> {
    let rows = app.db.q("SELECT project, merged_at, added, removed FROM pr_merges WHERE merged_at >= ? AND merged_at < ?", p![iso(from), iso(to)])?;
    let mut out: BTreeMap<NaiveDate, Merged> = BTreeMap::new();
    for r in &rows {
        let project = r.s("project").filter(|p| !p.trim().is_empty()).unwrap_or(no_project);
        let Some(at) = r.s("merged_at").and_then(parse_iso) else { continue };
        if hide.contains(project) {
            continue;
        }
        let m = out.entry(local_dt(at).date_naive()).or_default();
        m.prs += 1;
        if let (Some(a), Some(d)) = (r.i("added"), r.i("removed")) {
            m.added += a;
            m.removed += d;
            m.counted += 1;
        }
    }
    Ok(out)
}

/// Drops the merges from before `before` (a timestamp). How many went.
pub fn prune(app: &App, before: f64) -> Result<i64> {
    app.db.changed("DELETE FROM pr_merges WHERE merged_at < ?", p![iso(before)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields;
    use crate::prhost::{FakeHost, Record};

    fn app() -> (tempfile::TempDir, std::sync::Arc<App>) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::for_tests(crate::config::Config::for_tests(dir.path()));
        (dir, app)
    }

    fn task(app: &App, project: &str, num: i64) -> i64 {
        app.db.insert("tasks", vec![("title", json!("A")), ("status", json!("done")), ("project", json!(project)), ("pr_num", json!(num)), ("pr_host", json!("github")), ("pr_repo", json!("o/r")), ("pr_phase", json!("open"))]).unwrap()
    }

    #[test]
    fn a_pr_turning_merged_is_recorded_once() {
        let (_d, app) = app();
        let t = task(&app, "web", 7);
        crate::board::update_task(&app, t, fields!["pr_phase" => "merged"]).unwrap();
        crate::board::update_task(&app, t, fields!["pr_phase" => "merged", "title" => "B"]).unwrap();
        let rows = app.db.q("SELECT * FROM pr_merges", p![]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].i("task_id"), rows[0].i("pr_num"), rows[0].s("project")), (Some(t), Some(7), Some("web")));
        assert!(rows[0].i("added").is_none());
    }

    #[test]
    fn older_merges_are_backfilled_from_the_task_log() {
        let (_d, app) = app();
        let t = app.db.insert("tasks", vec![("title", json!("A")), ("pr_num", json!(9)), ("pr_phase", json!("merged")), ("finished_at", json!("2026-10-02T12:00:00Z"))]).unwrap();
        app.db.x("INSERT INTO events(task_id, at, text) VALUES(?, '2026-10-01T15:00:00Z', 'Merged PR #9')", p![t]).unwrap();
        let u = app.db.insert("tasks", vec![("title", json!("B")), ("pr_num", json!(4)), ("pr_phase", json!("merged")), ("finished_at", json!("2026-10-03T12:00:00Z"))]).unwrap();
        app.db.x("INSERT INTO events(task_id, at, text) VALUES(?, '2026-09-01T15:00:00Z', 'Merged PR #40')", p![u]).unwrap();
        app.db.x(BACKFILL, p![]).unwrap();
        app.db.x(BACKFILL, p![]).unwrap();
        let at = |id: i64| app.db.q1("SELECT merged_at FROM pr_merges WHERE task_id = ?", p![id]).unwrap().unwrap().st("merged_at");
        assert_eq!(app.db.q("SELECT id FROM pr_merges", p![]).unwrap().len(), 2);
        assert_eq!(at(t), "2026-10-01T15:00:00Z");
        assert_eq!(at(u), "2026-10-03T12:00:00Z", "another PR's line isn't this one's merge");
        prune(&app, parse_iso("2026-10-02T00:00:00Z").unwrap()).unwrap();
        app.db.x(BACKFILL, p![]).unwrap();
        assert_eq!(app.db.q("SELECT id FROM pr_merges", p![]).unwrap().len(), 1, "a pruned merge stays gone");
    }

    #[test]
    fn fill_reads_line_counts_and_gives_up_after_a_few_failures() {
        let (_d, app) = app();
        let host = FakeHost::new("github", Record::default());
        prhost::install(&app, host.clone());
        let t = task(&app, "web", 7);
        crate::board::update_task(&app, t, fields!["pr_phase" => "merged"]).unwrap();
        for _ in 0..TRIES + 1 {
            assert_eq!(fill(&app, 5).unwrap(), 0);
        }
        assert_eq!(host.calls().iter().filter(|c| *c == "lines 7").count(), TRIES as usize);

        app.db.x("UPDATE pr_merges SET tries = 0", p![]).unwrap();
        *host.held.lock() = Some(now_ts() + 60.0);
        fill(&app, 5).unwrap();
        assert_eq!(host.calls().iter().filter(|c| *c == "lines 7").count(), TRIES as usize, "a held host isn't asked");

        *host.held.lock() = None;
        *host.lines.lock() = Some((120, 30));
        assert_eq!(fill(&app, 5).unwrap(), 1);
        let r = app.db.q1("SELECT added, removed FROM pr_merges", p![]).unwrap().unwrap();
        assert_eq!((r.i("added"), r.i("removed")), (Some(120), Some(30)));
        assert_eq!(fill(&app, 5).unwrap(), 0, "a counted PR isn't read again");
    }

    #[test]
    fn days_add_up_visible_projects_and_know_when_lines_are_unknown() {
        let (_d, app) = app();
        let day = NaiveDate::from_ymd_opt(2026, 10, 27).unwrap();
        let at = |h: f64| iso(crate::days::start_of(day) + h * 3600.0);
        for (num, project, h, lines) in [(1, Some("web"), 9.0, Some((10, 2))), (2, None, 15.0, None), (3, Some("app"), 11.0, Some((5, 5))), (4, Some("web"), 30.0, None)] {
            let (a, r) = lines.map_or((Value::Null, Value::Null), |(a, r)| (json!(a), json!(r)));
            app.db.x("INSERT INTO pr_merges(task_id, pr_num, project, merged_at, added, removed) VALUES(?, ?, ?, ?, ?, ?)", p![num, num, project, at(h), a, r]).unwrap();
        }
        let from = crate::days::start_of(day);
        let hide: HashSet<String> = ["app".to_string()].into();
        let days = by_day(&app, from, from + 2.0 * 86400.0, &hide, "Other").unwrap();
        assert_eq!(days[&day], Merged { prs: 2, added: 10, removed: 2, counted: 1 });
        let next = days[&(day + chrono::Duration::days(1))];
        assert_eq!(next.prs, 1);
        let mut v = json!({});
        next.put(&mut v);
        assert_eq!(v, json!({"merged": 1, "added": null, "removed": null}));
        let hide_other: HashSet<String> = ["Other".to_string(), "app".to_string()].into();
        assert_eq!(by_day(&app, from, from + 86400.0, &hide_other, "Other").unwrap()[&day].prs, 1, "no project is Other's");
    }
}
