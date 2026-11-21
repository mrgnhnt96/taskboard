//! A PR stacked on another can merge into the parent's base carrying every one of the parent's commits
//! (#1072): it was pointed at the base while the parent was still open, and merged there. The parent's
//! work has landed then, but its own PR stays open, turns conflicting, and the tasks waiting for it to
//! merge wait for good. So once such a child has merged and its head holds the parent's head
//! (`stack_drop::contains`: an ancestor, or merging it changes nothing), the board counts the parent's
//! PR as merged, which lets its waiters go on as `tb pr merged` would, and closes it on its host with a
//! comment saying where its commits went.

use crate::app::App;
use crate::prhost::PrRef;
use crate::util::*;
use crate::{board, dispatch, p, prflow, stack, stack_drop};

fn alert_key(id: i64) -> String {
    format!("stack-carried:{id}")
}

/// The base branch its PR record says it goes into.
fn rec_base(t: &Row) -> Option<String> {
    prflow::flow(t).get("rec").and_then(|r| r["base"].as_str()).filter(|b| !b.is_empty()).map(|b| b.to_string())
}

/// The merged tail-merge child whose PR took every one of `p`'s commits into `p`'s base, if one did.
/// Runs git, so it stays off hot paths and out of transactions.
fn carrier(app: &App, p: &Row) -> Result<Option<(Row, String)>> {
    let (Some(base), Some(inner)) = (rec_base(p), stack_drop::rec_head(p)) else { return Ok(None) };
    let Some(repo) = crate::worktrees::repo_of(app, p)? else { return Ok(None) };
    let kids = app.db.q(
        "SELECT * FROM tasks WHERE pr_after = ? AND id != ? AND pr_num IS NOT NULL AND (UPPER(pr_state) = 'MERGED' OR pr_phase = 'merged') ORDER BY id",
        p![p.id(), p.id()],
    )?;
    for k in kids {
        if stack::head(&k) || rec_base(&k).as_deref() != Some(base.as_str()) {
            continue;
        }
        let Some(outer) = stack_drop::rec_head(&k) else { continue };
        if stack_drop::contains(&repo, &outer, &inner) == Some(true) {
            return Ok(Some((k, base)));
        }
    }
    Ok(None)
}

/// The runner's sweep: each open PR that a merged stacked child carried into its base is counted as
/// merged and closed on its host. How many it landed.
pub fn sweep(app: &App) -> Result<i64> {
    let rows = app.db.q(
        "SELECT * FROM tasks WHERE pr_num IS NOT NULL AND (pr_phase IS NULL OR pr_phase NOT IN ('merged', 'landing', 'declined')) \
         AND id IN (SELECT pr_after FROM tasks WHERE pr_after IS NOT NULL AND (UPPER(pr_state) = 'MERGED' OR pr_phase = 'merged'))",
        p![],
    )?;
    let mut landed = 0;
    for p in rows {
        if !board::pr_still_open(&p) {
            continue;
        }
        let Some((k, base)) = carrier(app, &p)? else { continue };
        land(app, &p, &k, &base)?;
        landed += 1;
    }
    if landed > 0 {
        stack::retarget(app)?;
    }
    Ok(landed)
}

/// Marks `p`'s PR merged through `k`'s, then closes it on its host. It's marked first, so a poll that
/// reads it closed doesn't take it for declined; a close the host refuses is left to the owner.
fn land(app: &App, p: &Row, k: &Row, base: &str) -> Result<()> {
    app.db.tx(|| prflow::mark_carried(app, &board::get_task(app, p.id())?, k, base))?;
    let (num, child) = (p.i0("pr_num"), k.i0("pr_num"));
    let comment = format!("Every commit of this PR reached {base} through #{child}, which was stacked on it and has merged, so it's closed as landed.");
    let res = match PrRef::of(p) {
        Some(pr) => crate::prhost::host_for(app, &pr.host).and_then(|h| h.close(&pr, Some(&comment))),
        None => Err("the board doesn't know which repo it's in".to_string()),
    };
    app.db.tx(|| {
        let me = rf("task", p.id());
        match &res {
            Ok(()) => board::log_event(app, p.id(), board::BOARD, "status", &format!("Closed PR #{num} on its host: its commits are on {base} through PR #{child}")).map(|_| ()),
            Err(e) => {
                let text = format!(
                    "{me}'s PR #{num} landed on {base} through {}'s PR #{child}, but the board couldn't close it: {e}. Close it by hand.",
                    rf("task", k.id())
                );
                board::log_event(app, p.id(), board::BOARD, "status", &text)?;
                dispatch::add_alert_keyed(app, &text, Some(p.id()), p.i("goal_id"), &alert_key(p.id()))
            }
        }
    })
}

