//! A head-merged PR (`stack.rs`) lands in its parent's branch, and reaches the main branch only with the
//! stack's bottom PR. A push that rewrites the parent's branch (a rebase and force-push) can drop the
//! merged commits again, and the PR stays merged. So the board checks the commits are really there:
//! a head-merged task whose work the branch lost is marked in its PR flow and flagged, and it doesn't
//! count as on the main branch (`waitsfor::stack_landed`) until the work is back.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::json;

use crate::app::App;
use crate::util::*;
use crate::worktrees::{git, GIT_SECS};
use crate::{board, dispatch, fields, p, prflow};

/// The PR flow's mark that the task's merged work isn't on the branch it merged into.
const MARK: &str = "dropped";

fn alert_key(id: i64) -> String {
    format!("stack-dropped:{id}")
}

fn pr_merged(o: &Row) -> bool {
    o.st("pr_state").eq_ignore_ascii_case("MERGED") || o.s("pr_phase") == Some("merged")
}

/// The head commit its PR record holds.
fn rec_head(t: &Row) -> Option<String> {
    prflow::flow(t).get("rec").and_then(|r| r["head"].as_str()).filter(|h| !h.is_empty()).map(|h| h.to_string())
}

/// Whether the board found this head-merged task's work dropped from the branch it merged into.
pub fn dropped(t: &Row) -> bool {
    prflow::flow(t).get(MARK).is_some_and(|m| m.is_object())
}

/// Whether commit `outer` holds commit `inner`'s work: `inner` is an ancestor, or (after a squash or
/// rebase merge) merging `inner` into `outer` changes nothing. None when git can't tell: a commit isn't
/// in the repo yet, or the merge conflicts.
pub fn contains(repo: &str, outer: &str, inner: &str) -> Option<bool> {
    static SEEN: OnceLock<Mutex<HashMap<(String, String, String), bool>>> = OnceLock::new();
    if outer == inner {
        return Some(true);
    }
    let key = (repo.to_string(), outer.to_string(), inner.to_string());
    let seen = SEEN.get_or_init(Default::default);
    if let Some(v) = seen.lock().ok().and_then(|m| m.get(&key).copied()) {
        return Some(v);
    }
    if !std::path::Path::new(repo).is_dir() {
        return None;
    }
    let have = |sha: &str| git(repo, &["cat-file", "-e", &format!("{sha}^{{commit}}")], GIT_SECS).code == 0;
    if !have(outer) || !have(inner) {
        return None;
    }
    let v = match git(repo, &["merge-base", "--is-ancestor", inner, outer], GIT_SECS).code {
        0 => true,
        1 => {
            let m = git(repo, &["merge-tree", "--write-tree", outer, inner], GIT_SECS);
            if m.code != 0 {
                return None;
            }
            let tree = git(repo, &["rev-parse", &format!("{outer}^{{tree}}")], GIT_SECS);
            if tree.code != 0 {
                return None;
            }
            m.out.lines().next().map(str::trim) == Some(tree.out.trim())
        }
        _ => return None,
    };
    if let Ok(mut m) = seen.lock() {
        m.insert(key, v);
    }
    Some(v)
}

/// Where a head-merged task's work sits now: the first task down its stack whose PR is still open, or the
/// bottom one that took the stack to the main branch.
fn holder(app: &App, t: &Row) -> Result<Option<Row>> {
    let mut t = t.clone();
    for _ in 0..64 {
        if !crate::stack::head(&t) {
            return Ok(None);
        }
        let Some(p) = crate::stack::parent(app, &t)? else { return Ok(None) };
        if !pr_merged(&p) || !crate::stack::head(&p) {
            return Ok(Some(p));
        }
        t = p;
    }
    Ok(None)
}

/// Whether a done head-merged task's work is on the branch that holds it; None when there's nothing to
/// check or git can't tell. Runs git, so callers keep it off hot paths.
pub fn on_holder(app: &App, t: &Row) -> Result<Option<(Row, bool)>> {
    if !crate::stack::head(t) || !pr_merged(t) || t.s("status") != Some("done") {
        return Ok(None);
    }
    let Some(h) = holder(app, t)? else { return Ok(None) };
    let (Some(inner), Some(outer)) = (rec_head(t), rec_head(&h)) else { return Ok(None) };
    let Some(repo) = crate::worktrees::repo_of(app, t)? else { return Ok(None) };
    Ok(contains(&repo, &outer, &inner).map(|v| (h, v)))
}

/// Checks a head-merged task's work against the branch that holds it, marks and flags it when a push
/// dropped it, and clears the mark when it's back. Whether it's dropped now.
pub fn check(app: &App, t: &Row) -> Result<bool> {
    match on_holder(app, t)? {
        Some((h, false)) => flag(app, t, &h).map(|_| true),
        Some((h, true)) => restore(app, t, &h).map(|_| false),
        None => Ok(dropped(t)),
    }
}

fn flag(app: &App, t: &Row, h: &Row) -> Result<()> {
    let outer = rec_head(h).unwrap_or_default();
    let was = prflow::flow(t).get(MARK).and_then(|m| m["head"].as_str()).map(|s| s.to_string());
    if was.as_deref() == Some(outer.as_str()) {
        return Ok(());
    }
    prflow::merge_flow(app, t.id(), fields![MARK => json!({"into": h.id(), "head": outer, "at": now_iso()})])?;
    if was.is_some() {
        return Ok(());
    }
    let (me, into) = (rf("task", t.id()), rf("task", h.id()));
    let branch = crate::stack::branch(h).unwrap_or_else(|| format!("{into}'s branch"));
    let pr = t.i0("pr_num");
    board::log_event(
        app,
        t.id(),
        board::BOARD,
        "note",
        &format!("PR #{pr}'s commits aren't on {branch} any more: a push rewrote it without them, so this work won't reach the main branch with {into}'s PR"),
    )?;
    let text = format!(
        "{me}'s PR #{pr} merged into {into}'s branch {branch}, but a push rewrote {branch} without its commits, so {me}'s work won't reach the main branch. \
         Bring them back to {branch} (merge {me}'s branch into it again) before {into}'s PR merges."
    );
    dispatch::add_alert_keyed(app, &text, Some(t.id()), t.i("goal_id"), &alert_key(t.id()))
}

fn restore(app: &App, t: &Row, h: &Row) -> Result<()> {
    if !dropped(t) {
        return Ok(());
    }
    prflow::merge_flow(app, t.id(), fields![MARK => null])?;
    dispatch::clear_alert_key(app, &alert_key(t.id()))?;
    let branch = crate::stack::branch(h).unwrap_or_else(|| format!("{}'s branch", rf("task", h.id())));
    board::log_event(app, t.id(), board::BOARD, "note", &format!("PR #{}'s commits are back on {branch}", t.i0("pr_num")))?;
    Ok(())
}

/// The runner's sweep: each done head-merged task whose stack hasn't reached the main branch yet is
/// checked against the branch holding its work. Git runs outside the database's transaction.
pub fn sweep(app: &App) -> Result<()> {
    let rows = app.db.q(
        "SELECT * FROM tasks WHERE stack_merge = 'head' AND pr_after IS NOT NULL AND status = 'done' \
         AND (UPPER(pr_state) = 'MERGED' OR pr_phase = 'merged')",
        p![],
    )?;
    for t in rows {
        let Some(h) = holder(app, &t)? else { continue };
        if pr_merged(&h) && !dropped(&t) {
            continue;
        }
        match on_holder(app, &t)? {
            Some((h, false)) => app.db.tx(|| flag(app, &t, &h))?,
            Some((h, true)) => app.db.tx(|| restore(app, &t, &h))?,
            None => {}
        }
    }
    Ok(())
}
