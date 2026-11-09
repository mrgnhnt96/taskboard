//! Tasks that need another task's work first (`tb wait-for T<n>`).

use serde_json::{json, Value};

use crate::app::App;
use crate::board;
use crate::util::*;
use crate::{deliver, fields, hours, p, prflow};

pub const FOLLOW: &str = "follow";

pub fn ids(t: &Row) -> Vec<i64> {
    jloads_arr(t.s("waits_for"))
        .iter()
        .filter_map(|x| x.as_i64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
        .collect()
}

/// Every task this one needs first: what it waits for, and the task it stacks on (`stack.rs`).
pub fn deps(t: &Row) -> Vec<i64> {
    let mut all = ids(t);
    if let Some(p) = crate::stack::parent_id(t) {
        if !all.contains(&p) {
            all.push(p);
        }
    }
    all
}

/// The task ids in a list of refs (`T4 T5`, `T4,T5`, `["T4", 5]`), each once; `none` is skipped.
pub fn ref_ids(value: &Value) -> Result<Vec<i64>> {
    let items: Vec<String> = match value {
        Value::Null => vec![],
        Value::String(s) => s.replace(',', " ").split_whitespace().map(|x| x.to_string()).collect(),
        Value::Array(a) => a.iter().map(|x| x.as_str().map(|s| s.to_string()).unwrap_or_else(|| x.to_string())).collect(),
        other => vec![other.to_string()],
    };
    let mut out: Vec<i64> = vec![];
    for v in items.iter().filter(|v| !v.eq_ignore_ascii_case("none")) {
        if let Some(n) = parse_ref_str(v, "task")? {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    Ok(out)
}

pub fn clean(app: &App, value: &Value, t: Option<&Row>) -> Result<Option<String>> {
    let mut out: Vec<i64> = vec![];
    for n in ref_ids(value)? {
        if let Some(t) = t {
            if n == t.id() {
                return err(400, "A task can't wait for itself.");
            }
        }
        let other = board::get_task(app, n)?;
        if let Some(t) = t {
            if !crate::projects::may_wait_on(app, &t.st("project"), &other.st("project")) {
                return err(
                    409,
                    format!(
                        "{} is in {}, not {}, so its work can't be brought into this task. \
                         To let {}'s tasks wait for {}'s: tb project set {} --waits-on {}",
                        rf("task", n),
                        other.st("project"),
                        t.st("project"),
                        t.st("project"),
                        other.st("project"),
                        t.st("project"),
                        other.st("project")
                    ),
                );
            }
            if reaches(app, n, t.id(), &mut vec![])? {
                return err(409, format!("{} already waits for {}, so this would wait forever.", rf("task", n), rf("task", t.id())));
            }
        }
        if !out.contains(&n) {
            out.push(n);
        }
    }
    Ok(if out.is_empty() { None } else { Some(jdumps(&json!(out))) })
}

pub fn reaches(app: &App, start: i64, target: i64, seen: &mut Vec<i64>) -> Result<bool> {
    if start == target {
        return Ok(true);
    }
    if seen.contains(&start) {
        return Ok(false);
    }
    seen.push(start);
    let Some(t) = board::find_task(app, Some(start))? else { return Ok(false) };
    for n in deps(&t) {
        if reaches(app, n, target, seen)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn label(app: &App, other: &Row) -> Result<String> {
    let g = board::find_goal(app, other.i("goal_id"))?;
    Ok(crate::human::task_ref(other) + &g.map(|g| format!(" ({})", rf("goal", g.id()))).unwrap_or_default())
}

pub fn ready(other: Option<&Row>) -> bool {
    other.map(|o| (o.s("status") == Some("done") && !o.b("failed")) || crate::human::released(o)).unwrap_or(false)
}

/// The tasks this one waits for until their PR merges, not just until they're done (`tb wait-for --merged`).
pub fn merged_ids(t: &Row) -> Vec<i64> {
    board::task_context(t)
        .get("waits_merged")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
        .unwrap_or_default()
}

/// Adds tasks to the ones this one waits for until their PR merges.
pub fn set_merged(app: &App, t: &Row, wanted: &[i64]) -> Result<()> {
    let mut ctx = board::task_context(t);
    let mut all = merged_ids(t);
    for n in wanted {
        if !all.contains(n) {
            all.push(*n);
        }
    }
    ctx.insert("waits_merged".into(), json!(all));
    board::save_context(app, t.id(), &ctx, false)
}

/// Stops `t` waiting for the tasks in `value` (`tb wait-for --drop`), merge waits too, and keeps its
/// other waits. The task it stacks on isn't a wait to drop: `tb task set --stack-on none` ends that.
pub fn drop(app: &App, t: &Row, value: &Value) -> Result<Vec<i64>> {
    let gone = ref_ids(value)?;
    if gone.is_empty() {
        return err(400, "Say which wait to drop, for example: tb wait-for --drop T14.");
    }
    let waits = ids(t);
    let me = rf("task", t.id());
    for n in &gone {
        if crate::stack::parent_id(t) == Some(*n) {
            return err(409, format!("{me} stacks on {}, so it needs its PR merged: tb task set {me} --stack-on none stops that.", rf("task", *n)));
        }
        if !waits.contains(n) {
            return err(400, format!("{me} doesn't wait for {}.", rf("task", *n)));
        }
    }
    let rest: Vec<i64> = waits.into_iter().filter(|n| !gone.contains(n)).collect();
    let rest = if rest.is_empty() { None } else { Some(jdumps(&json!(rest))) };
    board::update_task(app, t.id(), fields!["waits_for" => rest])?;
    let t = board::get_task(app, t.id())?;
    let until_merged = merged_ids(&t);
    if until_merged.iter().any(|n| gone.contains(n)) {
        let mut ctx = board::task_context(&t);
        let left: Vec<i64> = until_merged.into_iter().filter(|n| !gone.contains(n)).collect();
        if left.is_empty() {
            ctx.remove("waits_merged");
        } else {
            ctx.insert("waits_merged".into(), json!(left));
        }
        board::save_context(app, t.id(), &ctx, false)?;
    }
    Ok(gone)
}

/// Forgets every merge wait (`tb wait-for none`).
pub fn clear_merged(app: &App, t: &Row) -> Result<()> {
    let mut ctx = board::task_context(t);
    if ctx.remove("waits_merged").is_some() {
        board::save_context(app, t.id(), &ctx, false)?;
    }
    Ok(())
}

fn pr_merged(o: &Row) -> bool {
    o.st("pr_state").eq_ignore_ascii_case("MERGED") || o.s("pr_phase") == Some("merged")
}

pub fn pr_declined(o: &Row) -> bool {
    matches!(o.st("pr_state").to_uppercase().as_str(), "CLOSED" | "DECLINED" | "SUPERSEDED") || o.s("pr_phase") == Some("declined")
}

/// The commit someone said its work landed in on the main branch outside its PR (`tb task set --landed`).
pub fn landed(o: &Row) -> Option<&str> {
    o.s("landed").filter(|s| !s.is_empty())
}

/// Done, and its PR is open: neither merged nor declined.
fn pr_open(o: &Row) -> bool {
    ready(Some(o)) && o.i("pr_num").is_some() && !pr_merged(o) && !pr_declined(o)
}

/// Turns a wait on a done task whose PR is still open into a tail-merge stack on that PR (`stack.rs`), so
/// the task's branch starts from that PR's branch and its PR goes into it and waits for it to merge,
/// instead of building on unmerged work its own PR into main would carry. Run before the task's
/// worktree is cut or its conversation carries on. A wait for a merged PR, a `--merged` wait, another
/// project's task and a task that stacks already stay as they are. The parent's id when it stacked.
pub fn stack_on_open(app: &App, t: &Row) -> Result<Option<i64>> {
    if crate::stack::parent_id(t).is_some() || crate::samepr::anchor_id(t).is_some() {
        return Ok(None);
    }
    let waits = ids(t);
    let until_merged = merged_ids(t);
    for n in waits.iter().copied().filter(|n| !until_merged.contains(n)) {
        let Some(o) = board::find_task(app, Some(n))? else { continue };
        if o.s("project") != t.s("project") || crate::human::is(&o) || !pr_open(&o) {
            continue;
        }
        let Some(b) = crate::stack::branch(&o) else { continue };
        let rest: Vec<i64> = waits.iter().copied().filter(|x| *x != n).collect();
        let rest = if rest.is_empty() { None } else { Some(jdumps(&json!(rest))) };
        board::update_task(app, t.id(), fields!["waits_for" => rest, "pr_after" => n, "stack_merge" => null])?;
        let pr = o.i0("pr_num");
        board::log_event(
            app,
            t.id(),
            board::BOARD,
            "note",
            &format!(
                "Stacks on {}: its PR #{pr} isn't merged yet, so this task's branch starts from {b} and its PR goes into {b} until #{pr} merges",
                rf("task", n)
            ),
        )?;
        return Ok(Some(n));
    }
    Ok(None)
}

/// Whether the task's PR, once merged, has its work on the main branch: it isn't a head merge (`stack.rs`),
/// or the parent it merged into is there too. A head merge lands in its parent's branch, so it only
/// reaches the main branch with the bottom PR of its stack, and not at all while a push to that branch
/// has dropped its commits (`stack_drop.rs`).
pub fn stack_landed(app: &App, t: &Row) -> Result<bool> {
    let mut t = t.clone();
    for _ in 0..64 {
        if !crate::stack::head(&t) {
            return Ok(true);
        }
        if crate::stack_drop::dropped(&t) {
            return Ok(false);
        }
        let Some(p) = crate::stack::parent(app, &t)? else { return Ok(true) };
        if !pr_merged(&p) {
            return Ok(false);
        }
        t = p;
    }
    Ok(false)
}

/// Its work is on the main branch: its PR merged there, or it landed another way.
pub fn on_main(app: &App, t: &Row) -> Result<bool> {
    Ok(landed(t).is_some() || (pr_merged(t) && stack_landed(app, t)?))
}

/// The head-merge tasks whose PR merged into this task's branch, to count as merged once it reaches the main branch.
pub fn head_merged_children(app: &App, id: i64) -> Result<Vec<Row>> {
    let kids = app.db.q("SELECT * FROM tasks WHERE pr_after = ? AND id != ? AND stack_merge = 'head' ORDER BY id", p![id, id])?;
    Ok(kids.into_iter().filter(pr_merged).collect())
}

/// The bottom PR of a head-merged task's stack, the one that takes it to the main branch.
fn stack_bottom(app: &App, t: &Row) -> Result<Option<Row>> {
    let mut b = None;
    let mut t = t.clone();
    for _ in 0..64 {
        if !crate::stack::head(&t) {
            break;
        }
        let Some(p) = crate::stack::parent(app, &t)? else { break };
        b = Some(p.clone());
        t = p;
    }
    Ok(b)
}

/// The task in a head-merged task's stack whose merged commits a push dropped (`stack_drop.rs`), and the
/// task whose branch lost them.
fn dropped_in(app: &App, t: &Row) -> Result<Option<(Row, Row)>> {
    let mut t = t.clone();
    for _ in 0..64 {
        if !crate::stack::head(&t) {
            break;
        }
        let Some(p) = crate::stack::parent(app, &t)? else { break };
        if crate::stack_drop::dropped(&t) {
            return Ok(Some((t, p)));
        }
        t = p;
    }
    Ok(None)
}

/// Done and not failed, and its work is on the base branch: its PR merged (with its stack, for a head
/// merge), it landed another way, or it ships no PR.
pub fn merged(app: &App, other: Option<&Row>) -> Result<bool> {
    let Some(o) = other.filter(|o| ready(Some(o))) else { return Ok(false) };
    if landed(o).is_some() {
        return Ok(true);
    }
    if pr_merged(o) {
        return stack_landed(app, o);
    }
    if o.i("pr_num").is_some() {
        return Ok(false);
    }
    Ok(has(o.s("no_pr")) || !crate::projects::task_ships_pr(app, o)?)
}

/// Whether `t` can go on with task `n`'s work: done, or merged when `t` waits for its PR to merge.
pub fn ready_for(app: &App, t: &Row, n: i64, other: Option<&Row>) -> Result<bool> {
    if merged_ids(t).contains(&n) {
        merged(app, other)
    } else {
        Ok(ready(other))
    }
}

pub fn blocker(app: &App, t: &Row) -> Result<Option<String>> {
    for n in deps(t) {
        let other = board::find_task(app, Some(n))?;
        let Some(other) = other else { continue };
        if !ready_for(app, t, n, Some(&other))? {
            return blocker_reason(app, t, &other).map(Some);
        }
    }
    Ok(None)
}

/// Why unfinished `other` holds `t` back.
fn blocker_reason(app: &App, t: &Row, other: &Row) -> Result<String> {
    let n = other.id();
    let who = label(app, other)?;
    if crate::human::is(other) && other.s("status") != Some("done") {
        return Ok(format!("Waits for you to do {who}"));
    }
    let pr = other.i("pr_num").map(|p| format!("PR #{p}"));
    let dropped = if pr_merged(other) { dropped_in(app, other)? } else { None };
    Ok(match other.s("status") {
        Some("done") if !other.b("failed") => match pr {
            Some(pr) if dropped.is_some() => {
                let (d, into) = dropped.unwrap();
                format!(
                    "Blocked by {who}: {} merged into {}'s branch, but a push to that branch dropped its commits, so its work isn't on the main branch",
                    if d.id() == other.id() { format!("its {pr}") } else { format!("{}'s PR #{}", rf("task", d.id()), d.i0("pr_num")) },
                    rf("task", into.id())
                )
            }
            Some(pr) if pr_merged(other) => match stack_bottom(app, other)? {
                Some(bt) => format!(
                    "Blocked by {who}: its {pr} merged into {}'s branch, and reaches the main branch once {}'s{} merges",
                    rf("task", bt.id()),
                    rf("task", bt.id()),
                    bt.i("pr_num").map(|n| format!(" PR #{n}")).unwrap_or_else(|| " PR".into())
                ),
                None => format!("Blocked by {who} until {pr} merges"),
            },
            Some(pr) if pr_declined(other) && ids(t).contains(&n) => {
                let r = rf("task", n);
                format!(
                    "Blocked by {who}, whose {pr} was declined, so its work isn't merged. If it landed another way: tb task set {r} --landed <commit>. To go on without it: tb wait-for --drop {r} --task {}",
                    rf("task", t.id())
                )
            }
            Some(pr) if pr_declined(other) => format!("Blocked by {who}, whose {pr} was declined, so its work isn't merged"),
            Some(pr) => format!("Blocked by {who} until {pr} merges"),
            None if has(other.s("pr_url")) => format!("Blocked by {who} until its PR merges"),
            None if ids(t).contains(&n) => {
                let r = rf("task", n);
                format!(
                    "Blocked by {who}, which finished with no PR linked, so no merge is coming. If its work went through another PR: tb task set {r} --pr-link <link>. \
                     If it landed another way: tb task set {r} --landed <commit>. To go on without it: tb wait-for --drop {r} --task {}",
                    rf("task", t.id())
                )
            }
            None => format!("Blocked by {who}, which finished with no PR linked, so no merge is coming"),
        },
        Some("done") =>format!("Blocked by {who}, which failed. It starts once {} is done", rf("task", n)),
        Some("planned") => format!("Blocked by {who}, which isn't queued yet"),
        _ => format!("Blocked by {who}"),
    })
}

/// Whether `t` waits for a Start on the board: its pickup is manual and it isn't in a goal that was run. A
/// running goal starts its tasks itself (its pause still holds them); a task on a terminal's line waits there.
pub fn waits_for_start(app: &App, t: &Row) -> Result<bool> {
    if t.s("pickup") != Some("manual") {
        return Ok(false);
    }
    if has(t.s("line_session")) {
        return Ok(true);
    }
    let goal = board::find_goal(app, t.i("goal_id"))?;
    Ok(!goal.is_some_and(|g| has(g.s("ran_at")) && !g.b("archived")))
}

/// The line a queued card shows when something holds it back.
pub fn waiting_line(app: &App, t: &Row) -> Result<Value> {
    if crate::scheduled::waiting(t) {
        return Ok(json!(crate::scheduled::line(&t.st("start_at"))));
    }
    if t.i("start_job").is_some() {
        let pending = board::live_start_job(app, t)?.filter(|j| j.s("state") == Some("pending"));
        let hold = match pending {
            Some(j) => crate::dispatch::start_hold(app, &j)?,
            None => crate::startwatch::waiting_line(app, t)?,
        };
        return Ok(hold.map(Value::String).unwrap_or(Value::Null));
    }
    if let Some(b) = blocker(app, t)? {
        return Ok(json!(b));
    }
    if let Some(why) = crate::jira::ticket_wait(app, t)? {
        return Ok(json!(why));
    }
    if has(t.s("line_session")) && crate::lines::held_by_clock(app, t)? {
        return Ok(json!(hours::blocker(app)));
    }
    if waits_for_start(app, t)? {
        return Ok(json!("Waits for you to press Start"));
    }
    if let Some(r) = t.s("retry_at").filter(|r| r > &now_iso().as_str()) {
        return Ok(json!(format!("Trying again at {}", local_clock(Some(r)))));
    }
    if let Some(g) = board::find_goal(app, t.i("goal_id"))? {
        if !hours::goal_open(app, Some(&g)) {
            return Ok(json!(hours::blocker(app)));
        }
        if let Some(why) = crate::runner::goal_blocker(app, t, &g)? {
            return Ok(json!(why));
        }
    } else if let Some(why) = hours::blocker(app) {
        return Ok(json!(why));
    }
    if let Some(why) = crate::locks::blocker(app, t)? {
        return Ok(json!(why));
    }
    if let Some(why) = crate::runner::slot_hold(app, t)? {
        return Ok(json!(why));
    }
    Ok(crate::devices::blocker(app, t)?.map(Value::String).unwrap_or(Value::Null))
}

pub fn blocked_by(app: &App, t: &Row) -> Result<Vec<Value>> {
    if !board::is_blocked(app, t)? {
        return Ok(vec![]);
    }
    let mut out = vec![];
    for n in deps(t) {
        if let Some(o) = board::find_task(app, Some(n))? {
            if !ready_for(app, t, n, Some(&o))? {
                let mut card = board::task_card(app, &o)?;
                card["why"] = json!(blocker_reason(app, t, &o)?);
                out.push(card);
            }
        }
    }
    Ok(out)
}

pub fn waiting_on(app: &App, t: &Row) -> Result<Vec<Row>> {
    app.db.q(
        "SELECT * FROM tasks WHERE status != 'done' AND id != ? AND (pr_after = ? OR (waits_for IS NOT NULL \
         AND EXISTS (SELECT 1 FROM json_each(tasks.waits_for) WHERE value = ?))) ORDER BY id",
        p![t.id(), t.id(), t.id()],
    )
}

pub fn branch_of(t: &Row) -> Option<String> {
    board::task_context(t)
        .get("where")
        .and_then(|w| w.get("branch"))
        .and_then(|b| b.as_str())
        .filter(|b| real_branch(b))
        .map(|b| b.to_string())
}

/// A branch name, not a detached checkout's `HEAD`.
fn real_branch(b: &str) -> bool {
    !b.is_empty() && b != "HEAD"
}

/// What to replay onto when `other` moves: a stacked PR's own base (the branch it merges into), else
/// `other`'s branch.
fn moved_onto(t: &Row, other: &Row) -> Option<String> {
    let stacked_base = || {
        (crate::stack::parent_id(t) == Some(other.id()))
            .then(|| jloads_obj(t.s("pr_flow")).get("rec").and_then(|r| r["base"].as_str()).map(|b| b.to_string()))
            .flatten()
    };
    stacked_base().filter(|b| real_branch(b)).or_else(|| crate::stack::branch(other).filter(|b| real_branch(b)))
}

fn git_head(path: Option<&str>, branch: Option<&str>) -> Option<String> {
    let (path, branch) = (path?, branch?);
    if !std::path::Path::new(path).is_dir() {
        return None;
    }
    let git = crate::proc::which("git")?;
    let args: Vec<String> = vec!["-C".into(), path.into(), "rev-parse".into(), "--verify".into(), "-q".into(), format!("refs/heads/{branch}^{{commit}}")];
    let out = crate::proc::run(&git, &args, None, 5.0).ok()?;
    if out.code != Some(0) {
        return None;
    }
    Some(out.stdout.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn head_of(t: &Row) -> Option<String> {
    let flow = jloads_obj(t.s("pr_flow"));
    if let Some(h) = flow.get("head").and_then(|v| v.as_str()) {
        if t.s("pr_state").unwrap_or("OPEN").eq_ignore_ascii_case("OPEN") {
            return Some(h.to_string());
        }
    }
    let ctx = board::task_context(t);
    let worktree = ctx.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str()).map(|s| s.to_string());
    git_head(worktree.as_deref().or(t.s("repo_path")), branch_of(t).as_deref())
        .or_else(|| flow.get("head").and_then(|v| v.as_str()).map(|s| s.to_string()))
}

fn where_it_is(app: &App, other: &Row) -> Result<String> {
    let branch = branch_of(other);
    let mut bits = vec![format!("{} “{}”", label(app, other)?, short(&other.st("title"), 60))];
    let merged = on_main(app, other)?;
    if let Some(c) = landed(other) {
        bits.push(format!("landed on the main branch in {}, outside its PR", &c[..c.len().min(12)]));
    } else if merged {
        bits.push(format!("is merged (PR #{}), so its work is on the main branch", other.i0("pr_num")));
    } else if pr_merged(other) {
        let into = crate::stack::parent(app, other)?;
        bits.push(format!(
            "is merged (PR #{}) into {}'s branch {}, not the main branch yet",
            other.i0("pr_num"),
            into.as_ref().map(|p| rf("task", p.id())).unwrap_or_else(|| "its parent".into()),
            into.as_ref().and_then(crate::stack::branch).unwrap_or_else(|| "its parent's branch".into())
        ));
    } else if other.i("pr_num").is_some() {
        bits.push(format!(
            "is in PR #{} ({}) on branch {}",
            other.i0("pr_num"),
            other.st("pr_url"),
            branch.as_deref().unwrap_or("its branch")
        ));
    } else if let Some(b) = &branch {
        bits.push(format!("is on branch {b}"));
    } else {
        bits.push("is done; its branch isn't known, so look for it with git branch -a or its log".into());
    }
    if let Some(h) = head_of(other) {
        if !merged {
            bits.push(format!("at {}", &h[..h.len().min(12)]));
        }
    }
    Ok(bits.join(" "))
}

/// Where another project's task's work is, for a task that waits for it.
fn elsewhere(app: &App, other: &Row) -> Result<String> {
    let mut line = format!("{} “{}” in {}", label(app, other)?, short(&other.st("title"), 60), other.st("project"));
    if let Some(c) = landed(other) {
        line += &format!(" landed on the main branch in {}, outside its PR", &c[..c.len().min(12)]);
    } else if on_main(app, other)? {
        line += &format!(" is merged (PR #{} {})", other.i0("pr_num"), other.st("pr_url"));
    } else if other.i("pr_num").is_some() {
        line += &format!(" is in PR #{} ({}), not merged yet", other.i0("pr_num"), other.st("pr_url"));
    } else {
        line += " is done";
    }
    if let Some(s) = other.s("summary").filter(|s| !s.is_empty()) {
        line += &format!(": {}", short(s, 400));
    }
    Ok(line)
}

pub fn bring_in_text(app: &App, t: &Row) -> Result<String> {
    let (mut lines, mut away, mut owner) = (vec![], vec![], vec![]);
    let mut owner_all_done = true;
    let shared = crate::samepr::shared_ids(app, t)?;
    for n in deps(t) {
        if shared.contains(&n) {
            // `samepr::handoff_line` says where the shared branch is and how this task's work reaches it.
            continue;
        }
        let other = board::find_task(app, Some(n))?;
        if ready_for(app, t, n, other.as_ref())? {
            let o = other.as_ref().unwrap();
            if crate::stack::parent_id(t) == Some(n) && !crate::stack::merged(o) {
                // `stack::handoff_line` says where its branch is and where this task's PR goes.
                continue;
            }
            if crate::human::is(o) {
                owner.push(crate::human::done_line(app, o));
                owner_all_done &= !crate::human::answered(o);
            } else if o.s("project") == t.s("project") {
                lines.push(format!("- {}", where_it_is(app, o)?));
            } else {
                away.push(format!("- {}", elsewhere(app, o)?));
            }
        }
    }
    let mut out = vec![];
    if !owner.is_empty() {
        out.push(if owner_all_done {
            format!("This task waited for {} to do something only they can, and it's done:", app.cfg.owner)
        } else {
            format!("This task waited for {} to do something only they can. Where that stands:", app.cfg.owner)
        });
        out.extend(owner);
    }
    if !away.is_empty() {
        out.push("This task needs work from another project, and it's ready:".to_string());
        out.extend(away);
        out.push(
            "It lives in that project's repo, not this one, so there's nothing to rebase: pick it up the way this \
             project uses that one (update the dependency, rebuild or reinstall it), then build on it."
                .to_string(),
        );
    }
    if lines.is_empty() {
        return Ok(out.join("\n"));
    }
    out.push("This task needs work from another task, and it's ready:".to_string());
    out.extend(lines);
    let pr_first = if crate::projects::task_ships_pr(app, t)? { " Say in the PR description that it needs that PR first." } else { "" };
    out.push(format!(
        "Bring it in with git, not by asking {}: git fetch origin, then rebase this task's branch so it sits on top \
         of that work. When it isn't merged yet, rebase onto its branch (git rebase origin/<its branch>), or with a \
         base of your own already, replay just your commits on top: git rebase --onto origin/<its branch> <your base> \
         <your branch>. Rebase, never merge.{pr_first}",
        app.cfg.owner
    ));
    Ok(out.join("\n"))
}

pub fn park(app: &App, t: &Row, wanted: &str, who: &str, why: &str) -> Result<()> {
    let mut ctx = board::task_context(t);
    ctx.insert("parked".into(), json!({"at": now_iso(), "session": t.v("session_id"), "why": why}));
    app.db.x(
        "UPDATE jobs SET state = 'expired', result = ?, updated_at = ? WHERE task_id = ? AND kind = 'agent' \
         AND purpose = 'start' AND state IN ('pending', 'running')",
        p![jdumps(&json!({"cancelled": "waits"})), now_iso(), t.id()],
    )?;
    board::save_context(app, t.id(), &ctx, false)?;
    let names = jloads_arr(Some(wanted)).iter().filter_map(|v| v.as_i64()).map(|n| rf("task", n)).collect::<Vec<_>>().join(", ");
    let pickup = if matches!(t.s("pickup"), Some("manual") | Some("attach")) { "new".to_string() } else { t.st("pickup") };
    board::update_task(
        app,
        t.id(),
        fields!["waits_for" => wanted, "status" => "queued", "needs_reason" => null, "question" => null,
                "session_id" => null, "start_job" => null, "lost" => 0, "pickup_session" => null, "pickup" => pickup,
                "latest" => format!("Waiting for {names}; it starts again by itself.")],
    )?;
    board::log_event(
        app,
        t.id(),
        who,
        "status",
        &format!(
            "Waits for {names}{}. Back in the queue; it carries on in this conversation once that's done",
            if why.is_empty() { String::new() } else { format!(": {why}") }
        ),
    )?;
    Ok(())
}

pub fn parked(t: &Row) -> Option<Row> {
    board::task_context(t).get("parked").and_then(|v| v.as_object()).cloned()
}

/// What a task that can go on now is told about the work it builds on: the PR it stacks on, then the
/// rest it waited for.
pub fn ready_text(app: &App, t: &Row) -> Result<String> {
    let stack = crate::stack::handoff_line(app, t, &app.cfg.pr_body.remote)?;
    let shared = crate::samepr::handoff_line(app, t, &app.cfg.pr_body.remote)?.filter(|_| crate::samepr::anchor_id(t).is_some());
    let text = bring_in_text(app, t)?;
    Ok(stack.into_iter().chain(shared).chain(Some(text)).filter(|x| !x.is_empty()).collect::<Vec<_>>().join("\n"))
}

pub fn resume_prompt(app: &App, t: &Row) -> Result<String> {
    let tb = board::tb_cmd(app);
    let r = rf("task", t.id());
    let text = ready_text(app, t)?;
    Ok([
        format!("[task-board:{r}] What this task was waiting for is ready, so it carries on here."),
        text,
        format!("Then carry on with the task from where you stopped, and finish with {tb} submit as usual."),
    ]
    .into_iter()
    .filter(|x| !x.is_empty())
    .collect::<Vec<_>>()
    .join("\n"))
}

pub fn started(app: &App, t: &Row) -> Result<()> {
    stack_on_open(app, t)?;
    let mut ctx = board::task_context(t);
    if ctx.remove("parked").is_some() {
        board::save_context(app, t.id(), &ctx, false)?;
    }
    Ok(())
}

/// Parked tasks whose old terminal is still open and idle, to close.
pub fn to_close(app: &App) -> Result<Vec<(Row, Row, String)>> {
    let mut out = vec![];
    for t in app.db.q(
        "SELECT * FROM tasks WHERE status = 'queued' AND session_id IS NULL AND line_session IS NULL \
         AND json_extract(context, '$.parked.session') IS NOT NULL",
        p![],
    )? {
        let Some(p) = parked(&t) else { continue };
        let Some(s) = board::get_session(app, p.s("session"))? else { continue };
        if s.s("status") == Some("gone") || crate::lines::busy(app, &s.st("id"))? {
            continue;
        }
        let at = p.st("at");
        out.push((t, s, at));
    }
    Ok(out)
}

/// Tells a live task (or wakes a done task's PR) when work it builds on moves or merges.
pub fn follow_ups(app: &App) -> Result<i64> {
    let mut sent = 0;
    for t in app.db.q("SELECT * FROM tasks WHERE waits_for IS NOT NULL OR pr_after IS NOT NULL", p![])? {
        let live = matches!(t.s("status"), Some("working") | Some("needs")) && has(t.s("session_id"));
        let pr_open = t.s("status") == Some("done") && board::pr_still_open(&t);
        if !(live || pr_open) {
            continue;
        }
        let flow = jloads_obj(t.s("pr_flow"));
        let seen = flow.get(FOLLOW).and_then(|v| v.as_object()).cloned().unwrap_or_default();
        let mut changes = Row::new();
        let mut notes = vec![];
        let mut held = None;
        for n in deps(&t) {
            let other = board::find_task(app, Some(n))?;
            if !ready(other.as_ref()) {
                continue;
            }
            let other = other.unwrap();
            let now = json!({"head": head_of(&other), "merged": landed(&other).is_some() || (other.st("pr_state").eq_ignore_ascii_case("MERGED") && stack_landed(app, &other)?)});
            let key = n.to_string();
            let Some(before) = seen.get(&key) else {
                changes.insert(key, now);
                continue;
            };
            if *before == now || (now["head"].is_null() && now["merged"] == false) {
                continue;
            }
            if *held.get_or_insert(stack_base_held(app, &t)?) {
                continue;
            }
            if let Some(line) = follow_line(app, &t, &other, before, &now)? {
                notes.push(line);
            }
            changes.insert(key, now);
        }
        if changes.is_empty() {
            continue;
        }
        if !notes.is_empty() {
            let mut lines = vec![format!("[task-board:{}] Work this task builds on has moved:", rf("task", t.id()))];
            lines.extend(notes.iter().cloned());
            lines.push(format!(
                "Rebase, run the tests covering your change (not the whole suite), and push with git push --force-with-lease if the branch was pushed. Don't ask {}; this is routine.{}",
                app.cfg.owner,
                prflow::KEEP_BASE
            ));
            let text = lines.join("\n");
            if live {
                deliver::add(app, "follow", &text, t.id(), None, None)?;
            } else if prflow::waking(app, &t)? || !hours::goal_open(app, board::find_goal(app, t.i("goal_id"))?.as_ref()) || prflow::goal_paused(app, &t)? {
                continue;
            } else if prflow::follow_wake(app, &t, &text)?.is_none() {
                // No slot or a visit under way: left unrecorded, so the next poll wakes it.
                continue;
            }
            board::log_event(
                app,
                t.id(),
                board::BOARD,
                "handoff",
                &format!("Told it to rebase: {}", notes.iter().map(|x| short(x, 120)).collect::<Vec<_>>().join("; ")),
            )?;
            sent += 1;
        }
        let mut merged = seen.clone();
        for (k, v) in changes {
            merged.insert(k, v);
        }
        prflow::merge_flow(app, t.id(), vec![(FOLLOW, Value::Object(merged))])?;
    }
    Ok(sent)
}

/// A head-merge child still stacks on the task: rebasing its branch now would break that child's PR,
/// so a move it builds on is left unrecorded until the stack has merged into it.
fn stack_base_held(app: &App, t: &Row) -> Result<bool> {
    Ok(crate::stack::children(app, t.id())?.iter().any(crate::stack::head))
}

/// Whether git, in the task's checkout, finds `sha` on the task's branch (locally or at origin). None when
/// there's no checkout or branch to ask.
fn on_branch(t: &Row, sha: &str) -> Option<bool> {
    let ctx = board::task_context(t);
    let worktree = ctx.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str());
    let dir = [worktree, t.s("repo_path")].into_iter().flatten().find(|d| std::path::Path::new(d).is_dir())?;
    let branch = crate::stack::branch(t).filter(|b| real_branch(b))?;
    let git = crate::proc::which("git")?;
    let run = |args: &[&str]| {
        let mut all: Vec<String> = vec!["-C".into(), dir.into()];
        all.extend(args.iter().map(|a| a.to_string()));
        crate::proc::run(&git, &all, None, 5.0).ok().and_then(|o| o.code)
    };
    let refs: Vec<String> = [format!("refs/heads/{branch}"), format!("refs/remotes/origin/{branch}")]
        .into_iter()
        .filter(|r| run(&["rev-parse", "--verify", "-q", &format!("{r}^{{commit}}")]) == Some(0))
        .collect();
    if refs.is_empty() {
        return None;
    }
    Some(refs.iter().any(|r| run(&["merge-base", "--is-ancestor", sha, r]) == Some(0)))
}

/// What to tell the task about a dependency's change, if anything: nothing when its head didn't move
/// and it didn't newly merge, or when its old head isn't on the task's branch, so there's nothing of it
/// to replay or drop.
fn follow_line(app: &App, t: &Row, other: &Row, before: &Value, now: &Value) -> Result<Option<String>> {
    let newly_merged = now["merged"] == true && before["merged"] != true;
    let (was, is) = (before["head"].as_str().unwrap_or(""), now["head"].as_str().unwrap_or(""));
    if !newly_merged && (is.is_empty() || was == is) {
        return Ok(None);
    }
    if !was.is_empty() && on_branch(t, was) == Some(false) {
        let what = if newly_merged { "merged" } else { "moved" };
        board::log_event(
            app,
            t.id(),
            board::BOARD,
            "note",
            &format!("Didn't ask it to rebase when {} {what}: its old last commit {} isn't on this task's branch", rf("task", other.id()), short_sha(was)),
        )?;
        return Ok(None);
    }
    let branch = moved_onto(t, other).unwrap_or_else(|| "<its branch>".into());
    let old = short_sha(was);
    let old = if old.is_empty() { "<its old last commit>".to_string() } else { old };
    let lbl = label(app, other)?;
    if newly_merged {
        return Ok(Some(format!(
            "- {lbl} was merged. Drop its commits from your branch and keep only yours: git fetch origin && \
             git rebase --onto origin/<your base, usually main> {old} <your branch>."
        )));
    }
    let new = short_sha(is);
    Ok(Some(format!(
        "- {lbl}'s branch {branch} moved from {old} to {new}. Replay your commits on the new tip: \
         git fetch origin && git rebase --onto origin/{branch} {old} <your branch>."
    )))
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(12).collect()
}
