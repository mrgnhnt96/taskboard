//! Stacked PRs (`tb task new|set --stack-on T<n>`): a task whose work builds on another task's PR
//! before that PR merges. The task waits for its parent like `waits_for` (in any goal), its worktree
//! starts from the parent's branch, its PR goes into the parent's branch and waits there (`waits`)
//! until the parent's PR merges, and then the board points it at the parent's own base.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, p, prflow, waitsfor};

/// The task this one stacks on.
pub fn parent_id(t: &Row) -> Option<i64> {
    t.i("pr_after").filter(|n| *n > 0)
}

pub fn parent(app: &App, t: &Row) -> Result<Option<Row>> {
    match parent_id(t) {
        Some(n) => board::find_task(app, Some(n)),
        None => Ok(None),
    }
}

/// A stack-on value from a body (`T12`, `12`, `none`): the parent's id, checked like a wait.
pub fn clean(app: &App, value: &Value, t: Option<&Row>) -> Result<Option<i64>> {
    let text = match value {
        Value::Null => return Ok(None),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.trim().to_string(),
        other => other.to_string(),
    };
    if text.is_empty() || text.eq_ignore_ascii_case("none") || text.eq_ignore_ascii_case("off") {
        return Ok(None);
    }
    let Some(n) = parse_ref_str(&text, "task")? else { return err(400, format!("“{text}” isn't a task; give one like T12.")) };
    let other = board::get_task(app, n)?;
    if let Some(t) = t {
        if n == t.id() {
            return err(400, "A task can't stack on itself.");
        }
        if other.s("project") != t.s("project") {
            return err(409, format!("{} is in {}, not {}, so this task's PR can't build on its branch.", rf("task", n), other.st("project"), t.st("project")));
        }
        if waitsfor::reaches(app, n, t.id(), &mut vec![])? {
            return err(409, format!("{} already builds on {}, so this would wait forever.", rf("task", n), rf("task", t.id())));
        }
    }
    Ok(Some(n))
}

/// Sets a task's parent from a body's `stack_on`, logging the change. False when it didn't change.
pub fn set(app: &App, t: &Row, value: &Value, who: &str) -> Result<bool> {
    let n = clean(app, value, Some(t))?;
    if n == parent_id(t) {
        return Ok(false);
    }
    board::update_task(app, t.id(), fields!["pr_after" => n])?;
    let text = match n {
        Some(n) => format!("Stacks on {}: its PR builds on that task's PR", rf("task", n)),
        None => "Stacks on no other task".into(),
    };
    board::log_event(app, t.id(), who, "note", &text)?;
    Ok(true)
}

pub fn merged(t: &Row) -> bool {
    t.st("pr_state").eq_ignore_ascii_case("MERGED") || t.s("pr_phase") == Some("merged")
}

/// A task's branch: what its PR says, else where its terminal worked.
pub fn branch(t: &Row) -> Option<String> {
    let f = jloads_obj(t.s("pr_flow"));
    f.get("rec")
        .and_then(|r| r["branch"].as_str())
        .filter(|b| !b.is_empty())
        .map(|b| b.to_string())
        .or_else(|| waitsfor::branch_of(t))
}

/// The branch this task's PR goes into while its parent's PR is unmerged: the parent's branch.
pub fn base_branch(app: &App, t: &Row) -> Result<Option<String>> {
    let Some(p) = parent(app, t)? else { return Ok(None) };
    if merged(&p) {
        return Ok(None);
    }
    Ok(branch(&p))
}

/// The task's PR has to wait for its parent's to merge first.
pub fn holds(app: &App, t: &Row) -> Result<bool> {
    Ok(parent(app, t)?.map(|p| !merged(&p)).unwrap_or(false))
}

/// What the app and `tb` show about the PR a task stacks on.
pub fn card(app: &App, t: &Row) -> Result<Value> {
    let Some(p) = parent(app, t)? else { return Ok(Value::Null) };
    Ok(json!({"ref": rf("task", p.id()), "title": p.v("title"), "num": p.v("pr_num"), "url": p.v("pr_url"),
              "branch": branch(&p), "merged": merged(&p), "line": line(&p)}))
}

/// "Stacks on T3's PR #12" (or its branch, before the PR is open).
pub fn line(p: &Row) -> String {
    let r = rf("task", p.id());
    match p.i("pr_num") {
        Some(n) if merged(p) => format!("Stacked on {r}'s PR #{n}, which is merged"),
        Some(n) => format!("Stacks on {r}'s PR #{n}"),
        None => match branch(p) {
            Some(b) => format!("Stacks on {r}'s branch {b}"),
            None => format!("Stacks on {r}"),
        },
    }
}

/// The handoff's lines about the parent: where to cut the branch, and where the PR goes.
pub fn handoff_line(app: &App, t: &Row, remote: &str) -> Result<Option<String>> {
    let Some(p) = parent(app, t)? else { return Ok(None) };
    let r = rf("task", p.id());
    if merged(&p) {
        return Ok(Some(format!("This task stacked on {r}, whose PR is merged now: build on the main branch as usual.")));
    }
    let Some(b) = branch(&p) else {
        return Ok(Some(format!("This task stacks on {r}: its PR builds on that task's branch, once it's known (tb status {r}).")));
    };
    Ok(Some(format!(
        "This task stacks on {r} (branch {b}{}): cut this task's branch from {remote}/{b}, rebase onto it (never merge), \
         and open the PR into {b}, not the main branch. The PR waits there until {r}'s merges; then the board points it at \
         the main branch and tells you to rebase.",
        p.i("pr_num").map(|n| format!(", PR #{n}")).unwrap_or_default()
    )))
}

/// Open stacked PRs whose parent merged and that still point at the parent's branch: point each at the
/// parent's base. Outside any transaction (it runs the host's tool). Returns how many moved.
pub fn retarget(app: &App) -> Result<i64> {
    let rows = app.db.q(
        "SELECT * FROM tasks WHERE pr_after IS NOT NULL AND pr_num IS NOT NULL \
         AND (pr_phase IS NULL OR pr_phase NOT IN ('merged', 'declined'))",
        p![],
    )?;
    let mut moved = 0;
    for t in rows {
        let Some(p) = parent(app, &t)? else { continue };
        if !merged(&p) || !board::pr_still_open(&t) {
            continue;
        }
        let f = jloads_obj(t.s("pr_flow"));
        if f.contains_key("retargeted") {
            continue;
        }
        let base = jloads_obj(p.s("pr_flow")).get("rec").and_then(|r| r["base"].as_str()).filter(|b| !b.is_empty()).map(|b| b.to_string());
        let Some(base) = base.or_else(|| crate::steps::default_base(&t.st("repo_path"))) else { continue };
        let pr = rf("task", p.id());
        let res = crate::propen::host::retarget(app, &t, &base);
        app.db.tx(|| {
            match &res {
                Ok(()) => {
                    prflow::merge_flow(app, t.id(), fields!["retargeted" => base.clone()])?;
                    board::log_event(app, t.id(), board::BOARD, "status", &format!("{pr}'s PR merged, so PR #{} now goes into {base}", t.i0("pr_num")))?;
                }
                Err(e) => {
                    prflow::merge_flow(app, t.id(), fields!["retargeted" => format!("failed: {e}")])?;
                    let line = format!("{pr}'s PR merged, but PR #{} for {} couldn't be pointed at {base}: {e}", t.i0("pr_num"), rf("task", t.id()));
                    board::log_event(app, t.id(), board::BOARD, "status", &line)?;
                    crate::dispatch::add_alert(app, &line, Some(t.id()), t.i("goal_id"), None, Some("pr"))?;
                }
            }
            Ok(())
        })?;
        if res.is_ok() {
            moved += 1;
        }
    }
    Ok(moved)
}
