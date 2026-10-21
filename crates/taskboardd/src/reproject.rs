//! Moving a task or backlog issue to another project (`tb task set --project`, `tb backlog set --project`).
//! Its goal stays (a goal can hold more than one project's work); a task's repo and area follow the move.
//! Any task moves, so work filed under the wrong project can still be finished: one on a terminal keeps it
//! (its work is often already in the new repo, refused there until the move), one with a PR keeps it, and a
//! finished one waits in the new project's Unreleased. A task that ran but has no terminal now waits for
//! Start in the new repo.

use serde_json::Value;

use crate::app::App;
use crate::util::*;
use crate::{board, deliver, fields, lines, projects};

/// The board's own spelling of a project named in any case; refused when the board doesn't know it.
pub fn name(app: &App, v: &Value) -> Result<String> {
    let want = v.as_str().unwrap_or("").trim();
    if want.is_empty() {
        return err(400, "Say which project to move it to.");
    }
    let known: Vec<String> = projects::list_projects(app)?.iter().filter_map(|p| p["name"].as_str().map(|s| s.to_string())).collect();
    if let Some(p) = known.iter().find(|p| p.as_str() == want).or_else(|| known.iter().find(|p| p.eq_ignore_ascii_case(want))) {
        return Ok(p.clone());
    }
    err(400, format!("The board knows no project “{want}”. Its projects: {}.", known.join(", ")))
}

/// Refuses a task stacked on a parent in the old repo: its PR can't follow it there.
pub fn check_task(t: &Row) -> Result<()> {
    let r = rf("task", t.id());
    if let Some(parent) = t.i("pr_after") {
        return err(409, format!("{r} is stacked on {}; unstack it first with tb task set {r} --stack-on none.", rf("task", parent)));
    }
    Ok(())
}

/// Whether task `t` is on a terminal that's still open: it carries on there after the move, as it was.
/// One whose terminal closed (lost) has nothing to carry on in.
fn on_terminal(t: &Row) -> bool {
    t.s("session_id").is_some() && !t.b("lost") && t.s("status") != Some("done")
}

/// Whether task `t` is finished (a failed one is open to a retry): it stays finished, its PR and its
/// jobs on that PR with it.
fn finished(t: &Row) -> bool {
    t.s("status") == Some("done") && !t.b("failed")
}

/// Whether task `t` ran and has no terminal to carry on in: moving it puts it back for Start in the new repo.
fn restarts(t: &Row) -> bool {
    if on_terminal(t) || finished(t) {
        return false;
    }
    t.i("start_job").is_some() || matches!(t.s("status"), Some("working") | Some("needs") | Some("idle") | Some("done"))
}

/// The fields that put task `t` (in goal `goal`) in `project`: its repo and area there, out of the old project's
/// release, and back on the board waiting for Start when it ran and has no terminal to carry on in.
pub fn task_fields(app: &App, t: &Row, project: &str, goal: Option<i64>) -> Result<Vec<(&'static str, Value)>> {
    let repo = board::find_goal(app, goal)?
        .filter(|g| g.s("project") == Some(project))
        .and_then(|g| g.s("repo_path").filter(|r| !r.is_empty()).map(|r| r.to_string()))
        .or(projects::project_path(app, Some(project))?);
    let area = crate::areas::clean(app, project, &t.v("area")).unwrap_or(Value::Null);
    let mut f = fields!["project" => project, "repo_path" => repo, "area" => area, "release_id" => null];
    if restarts(t) {
        f.extend(fields!["session_id" => null, "claude_session_id" => null, "start_job" => null, "status" => "queued",
                         "pickup" => "manual", "pickup_session" => null, "needs_reason" => null, "question" => null, "lost" => 0,
                         "failed" => 0, "finished_at" => null, "summary" => null, "start_tries" => 0, "retry_at" => null,
                         "no_pr" => null, "latest" => format!("Moved to {project}. Press Start to run it there.")]);
    }
    Ok(f)
}

/// What moving task `t` leaves behind in the old repo: its open jobs, its place in a terminal's line and
/// the checkout it last ran in. A finished one keeps all of it; one on a terminal keeps its jobs and
/// line, and only the checkouts already in the new repo.
pub fn leave_old(app: &App, t: &Row) -> Result<()> {
    if finished(t) {
        return Ok(());
    }
    if on_terminal(t) {
        return forget_old_checkouts(app, t.id());
    }
    deliver::drop(app, t.id(), "it moved to another project")?;
    lines::leave(app, t)?;
    let mut ctx = board::task_context(t);
    if ctx.remove("where").is_some() {
        board::save_context(app, t.id(), &ctx, false)?;
    }
    Ok(())
}

/// Drops the checkouts task `id` recorded in another repository than its (new) project's, so a PR isn't
/// opened from the old repo: its worktree with the branch and commit seen there, and where it committed.
fn forget_old_checkouts(app: &App, id: i64) -> Result<()> {
    let t = board::get_task(app, id)?;
    let mut ctx = board::task_context(&t);
    let Some(w) = ctx.get_mut("where").and_then(|w| w.as_object_mut()) else { return Ok(()) };
    let old = |key: &str| w.get(key).and_then(|v| v.as_str()).is_some_and(|d| crate::steps::outside(&t, d).is_some());
    let mut keys = vec![];
    if old("worktree") {
        keys.extend(["worktree", "branch", "last_commit", "sha", "uncommitted"]);
    }
    if old("committed_in") {
        keys.extend(["committed_in", "committed_branch"]);
    }
    if keys.is_empty() {
        return Ok(());
    }
    for k in keys {
        w.remove(k);
    }
    board::save_context(app, id, &ctx, false)
}

/// The log line for task `t` moving to `to`, naming the terminal that carries on with it.
pub fn moved_text(t: &Row, to: &str) -> String {
    let from = t.s("project").filter(|p| !p.is_empty()).unwrap_or("no project");
    if on_terminal(t) {
        let n = t.s("session_name").filter(|n| !n.is_empty()).unwrap_or("its terminal");
        return format!("Moved from {from} to {to}; {n} carries on with it in {to}'s repo");
    }
    if restarts(t) {
        return format!("Moved from {from} to {to}; press Start to run it in {to}");
    }
    format!("Moved from {from} to {to}")
}

/// The fields that put issue `b` in `project`, with its area there when the project has one by that name.
pub fn issue_fields(app: &App, b: &Row, project: &str) -> Vec<(&'static str, Value)> {
    fields!["project" => project, "area" => crate::areas::clean(app, project, &b.v("area")).unwrap_or(Value::Null)]
}
