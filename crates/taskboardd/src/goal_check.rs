//! `tb submit` on a goal's last open task: the goal's done-when is checked first. While a miss on it is open
//! (`tb found --misses-goal`) the task can't finish, unless its own work fixes that miss (`--fixes B<n>`).
//! Otherwise it's asked once to check the done-when against its results on each platform it names, so a miss
//! is filed for the owner before the goal shows every task done.

use serde_json::json;

use crate::app::App;
use crate::board;
use crate::util::*;

/// Refuses the last open task's submit while its goal's done-when has an open miss, then asks it once to
/// check the done-when. `fixes` are the issues this submit names with `--fixes`.
pub fn refusal(app: &App, t: &Row, fixes: &[i64]) -> Result<()> {
    let Some(goal) = t.i("goal_id").map(|g| board::find_goal(app, Some(g))).transpose()?.flatten() else { return Ok(()) };
    if goal.b("archived") || !last_open(app, goal.id(), t.id())? {
        return Ok(());
    }
    let me = rf("task", t.id());
    let g = format!("{} “{}”", rf("goal", goal.id()), one_line(&goal.st("name"), 120));
    let tb = board::tb_cmd(app);
    let fixing: Vec<i64> = fixes.iter().copied().chain(crate::fixes::of_task(t)).collect();
    // A miss this task fixes, or one made into this very task, doesn't hold it.
    let misses: Vec<Row> = board::open_miss_rows(app, goal.id())?
        .into_iter()
        .filter(|m| !fixing.contains(&m.id()) && m.i("task_id") != Some(t.id()))
        .collect();
    if !misses.is_empty() {
        let lines: Vec<String> = misses
            .iter()
            .map(|m| {
                let what = format!("- {} · {}", rf("issue", m.id()), one_line(&m.st("title"), 120));
                match m.i("task_id").filter(|_| m.s("state") == Some("task")) {
                    Some(n) => format!("{what} (made into {}, not done yet)", rf("task", n)),
                    None => format!("{what} (with the owner)"),
                }
            })
            .collect();
        return err(
            409,
            format!(
                "{me} is the last open task in {g}, and a miss on its done-when is still open:\n{}\n\
                 The goal's last task can't finish while one is. If this task's work fixes it, run the same {tb} submit with --fixes B<n>. \
                 For one made into a task, {tb} wait-for T<n>. For one with the owner, ask how they'll settle it with {tb} question \"…\" and wait.",
                lines.join("\n")
            ),
        );
    }
    let outcome = goal.st("outcome").trim().to_string();
    let mut ctx = board::task_context(t);
    if outcome.is_empty() || ctx.get("goal_check_asked") == Some(&json!(true)) {
        return Ok(());
    }
    ctx.insert("goal_check_asked".into(), json!(true));
    app.db.tx(|| board::save_context(app, t.id(), &ctx, false))?;
    err(
        409,
        format!(
            "Before {me} finishes: it's the last open task in {g}, so check the goal's done-when against what the work shows, not what was planned:\n\
             {}\n\
             Go through it on each platform, device or place it names, one at a time, using results you have (tests, evidence, a run). \
             For each one it misses, run {tb} found \"<what misses, and where>\" --misses-goal: the board makes it a task for an agent (with --needs-owner, when it needs the owner's call, it goes to them) and keeps the goal open. \
             When every one holds, run the same {tb} submit again.",
            one_line(&outcome, 1000)
        ),
    )
}

/// Every other task the goal counts is done (or a human task let go), so this one finishing ends its work.
fn last_open(app: &App, goal_id: i64, task: i64) -> Result<bool> {
    let rows = crate::shared::counted_tasks(app, goal_id)?;
    Ok(rows.iter().any(|r| r.id() == task)
        && rows.iter().filter(|r| r.id() != task).all(|r| crate::human::released(r) || r.s("status") == Some("done")))
}
