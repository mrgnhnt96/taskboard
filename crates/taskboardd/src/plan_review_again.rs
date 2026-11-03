//! Reviewing a running goal's plan again once its scope moved. The first review reads the plan before
//! the goal runs; work added mid-run, or a decision a task settled, changes what the rest of the plan
//! has to prove. The board's tick looks at each running goal against its latest review round and runs
//! one more round when, since that round started, two or more tasks joined the goal (one task joining
//! is too small to look again) or a task that was a decision, spec, baseline, investigation or design
//! finished. It waits until the goal has been quiet for a few minutes, so tasks proposed together or
//! one after another are reviewed in one round, and while the review can't run (off, no Claude, no
//! usage) it waits without keeping a skipped round. Each round keeps the newest task id it saw, so a
//! change is counted toward one round only.

use once_cell::sync::Lazy;
use regex::Regex;

use crate::app::App;
use crate::util::*;
use crate::{board, p, plan_review};

pub const ADDED: &[(&str, &str, &str)] = &[("plan_reviews", "upto", "INT")];

/// How long a goal has to be quiet after the last change before it's reviewed again.
pub const QUIET_SECS: f64 = 180.0;
/// Tasks that have to join a running goal before it's reviewed again.
pub const JOINED_MIN: usize = 2;
pub const SOURCE: &str = "board";

static DECISION: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(decid\w*|decision\w*|specs?|specif\w*|baselines?|investigat\w*)\b").unwrap());

/// The newest task in the goal now: what a round starting now has seen.
pub fn upto(app: &App, gid: i64) -> Result<i64> {
    app.db.count("SELECT COALESCE(MAX(id), 0) FROM tasks WHERE goal_id = ?", p![gid])
}

/// Whether finishing this task can change what the rest of the plan has to prove.
pub fn settles_a_decision(kind: &str, title: &str) -> bool {
    kind == "design" || DECISION.is_match(title)
}

/// Reviews each running goal again whose scope moved since its latest round and has been quiet since.
pub fn tick(app: &App, now: f64) -> Result<Vec<i64>> {
    if plan_review::held(app).is_some() {
        return Ok(vec![]);
    }
    let goals = app.db.q(
        "SELECT g.id FROM goals g WHERE COALESCE(g.archived, 0) = 0 AND g.ran_at IS NOT NULL \
         AND EXISTS (SELECT 1 FROM plan_reviews r WHERE r.goal_id = g.id)",
        p![],
    )?;
    let mut queued = vec![];
    for g in goals {
        let gid = g.id();
        if !board::goal_running(app, gid)? || app.shared.lock().plan_reviewing.contains(&gid) {
            continue;
        }
        if let Some(reason) = due(app, gid, now)? {
            plan_review::queue(app, gid, &reason, SOURCE);
            queued.push(gid);
        }
    }
    Ok(queued)
}

/// Why the goal is reviewed again now, or None while nothing moved enough or it hasn't been quiet.
fn due(app: &App, gid: i64, now: f64) -> Result<Option<String>> {
    let Some(last) = app.db.q1("SELECT * FROM plan_reviews WHERE goal_id = ? ORDER BY id DESC LIMIT 1", p![gid])? else {
        return Ok(None);
    };
    let Some(since) = last.s("finished_at").and(last.s("started_at")) else {
        return Ok(None);
    };
    let seen = match last.i("upto") {
        Some(n) => n,
        None => app.db.count("SELECT COALESCE(MAX(id), 0) FROM tasks WHERE goal_id = ? AND created_at <= ?", p![gid, since])?,
    };
    let joined = app.db.q("SELECT id, created_at FROM tasks WHERE goal_id = ? AND id > ? ORDER BY id", p![gid, seen])?;
    let decided: Vec<Row> = app
        .db
        .q(
            "SELECT id, kind, title, finished_at FROM tasks WHERE goal_id = ? AND status = 'done' AND COALESCE(failed, 0) = 0 \
             AND finished_at > ? ORDER BY id",
            p![gid, since],
        )?
        .into_iter()
        .filter(|t| settles_a_decision(&t.st("kind"), &t.st("title")))
        .collect();
    let joined = if joined.len() >= JOINED_MIN { joined } else { vec![] };
    if joined.is_empty() && decided.is_empty() {
        return Ok(None);
    }
    let latest = joined.iter().filter_map(|t| t.s("created_at")).chain(decided.iter().filter_map(|t| t.s("finished_at"))).filter_map(parse_iso).fold(f64::MIN, f64::max);
    if now - latest < QUIET_SECS {
        return Ok(None);
    }
    let mut why = vec![];
    if !joined.is_empty() {
        why.push(format!("{} joined the running goal", plural(joined.len() as i64, "task")));
    }
    if !decided.is_empty() {
        let refs: Vec<String> = decided.iter().map(|t| rf("task", t.id())).collect();
        why.push(format!("{} settled {}", refs.join(", "), if refs.len() == 1 { "a decision" } else { "decisions" }));
    }
    Ok(Some(why.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_tasks_are_known_by_kind_or_title() {
        assert!(settles_a_decision("design", "Theme toggle mockups"));
        for t in ["Decide the palette", "Write the sync spec", "Specify the API", "Baseline the build time", "Investigate the slow start", "Record the decision"] {
            assert!(settles_a_decision("", t), "{t}");
        }
        for t in ["Add the toggle", "Respect the system theme", "Speculative cache"] {
            assert!(!settles_a_decision("", t), "{t}");
        }
    }
}
