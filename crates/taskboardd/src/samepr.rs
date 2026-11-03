//! Tasks sharing one branch and PR (`tb task new|set --same-pr T<n>`). The task named is the group's
//! anchor and owns the PR; each member waits for the group's task before it, works on the anchor's
//! branch from its tip, gets its own review round on its own commits, pushes once at submit and adds its
//! section to the PR's description. The first to submit (the anchor) opens the PR, which holds until
//! every member has submitted.
//!
//! Members link the PR but never own it (no `pr_num`), so the PR flow follows the anchor alone and a
//! trigger copies its phase and state onto them: they count as merged when it merges, and go back with
//! it when it's declined.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, p, stack, waitsfor};

/// Copies the anchor's PR phase and state onto the members that linked its PR.
pub const SCHEMA: &str = r#"
CREATE TRIGGER IF NOT EXISTS same_pr_mirror AFTER UPDATE OF pr_phase, pr_state ON tasks WHEN NEW.pr_num IS NOT NULL
BEGIN
    UPDATE tasks SET pr_phase = NEW.pr_phase, pr_state = NEW.pr_state
    WHERE same_pr = NEW.id AND pr_url IS NOT NULL AND pr_num IS NULL;
END;
"#;

/// More tasks than this in one PR is a lot for one reader.
const MAX_TASKS: usize = 4;

/// More planned files than this across a group's tasks is a lot for one reader.
const MAX_FILES: usize = 25;

/// The task whose PR this one shares.
pub fn anchor_id(t: &Row) -> Option<i64> {
    t.i("same_pr").filter(|n| *n > 0)
}

pub fn anchor(app: &App, t: &Row) -> Result<Option<Row>> {
    board::find_task(app, anchor_id(t))
}

/// The tasks sharing this one's PR, oldest first.
pub fn members(app: &App, id: i64) -> Result<Vec<Row>> {
    app.db.q("SELECT * FROM tasks WHERE same_pr = ? AND id != ? ORDER BY id", p![id, id])
}

/// The anchor and its members.
fn group(app: &App, anchor: &Row) -> Result<Vec<Row>> {
    let mut all = vec![anchor.clone()];
    all.extend(members(app, anchor.id())?);
    Ok(all)
}

/// The shared PR is merged or declined: nothing more goes on its branch.
fn closed(a: &Row) -> bool {
    stack::merged(a) || waitsfor::pr_declined(a)
}

/// The group task a member comes after: the last one it waits for, else the anchor.
fn predecessor(app: &App, t: &Row) -> Result<Option<Row>> {
    let Some(a) = anchor(app, t)? else { return Ok(None) };
    let ids: Vec<i64> = group(app, &a)?.iter().map(|g| g.id()).filter(|n| *n != t.id()).collect();
    match waitsfor::ids(t).into_iter().rev().find(|n| ids.contains(n)) {
        Some(n) => board::find_task(app, Some(n)),
        None => Ok(Some(a)),
    }
}

/// The group's last task, leaving `except` out: the one no other comes after.
fn tail(app: &App, a: &Row, except: i64) -> Result<Row> {
    let tasks: Vec<Row> = group(app, a)?.into_iter().filter(|g| g.id() != except).collect();
    let mut before: Vec<i64> = vec![];
    for g in tasks.iter().filter(|g| anchor_id(g).is_some()) {
        if let Some(p) = predecessor(app, g)? {
            before.push(p.id());
        }
    }
    Ok(tasks.iter().filter(|g| !before.contains(&g.id())).max_by_key(|g| g.id()).cloned().unwrap_or_else(|| a.clone()))
}

/// A same-pr value from a body (`T12`, `12`, `none`): the anchor's id. A member named stands for its
/// anchor.
pub fn clean(app: &App, value: &Value, t: &Row) -> Result<Option<i64>> {
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
    let named = board::get_task(app, n)?;
    let a = match anchor_id(&named) {
        Some(id) => board::get_task(app, id)?,
        None => named,
    };
    let (me, ra) = (rf("task", t.id()), rf("task", a.id()));
    if a.id() == t.id() {
        return err(400, "A task can't share its own PR.");
    }
    if !members(app, t.id())?.is_empty() {
        return err(409, format!("Other tasks share {me}'s PR, so it can't share {ra}'s too. Add them to {ra}'s instead."));
    }
    if a.s("project") != t.s("project") {
        return err(409, format!("{ra} is in {}, not {}, so this task can't add to its PR.", a.st("project"), t.st("project")));
    }
    for (x, r) in [(&a, &ra), (t, &me)] {
        if crate::design::is(x) || crate::human::is(x) || !crate::projects::task_ships_pr(app, x)? {
            return err(409, format!("{r} doesn't end in a PR, so there's none to share."));
        }
    }
    if has(t.s("pr_url")) {
        return err(409, format!("{me} has its own PR already."));
    }
    if let Some(p) = stack::parent_id(t) {
        return err(409, format!("{me} stacks on {}; a task shares a PR or stacks on one, not both. Take it off first (--stack-on none).", rf("task", p)));
    }
    if closed(&a) {
        return err(409, format!("{ra}'s PR is closed, so there's nothing left to add to."));
    }
    let after = tail(app, &a, t.id())?;
    if waitsfor::reaches(app, after.id(), t.id(), &mut vec![])? {
        return err(409, format!("{} already waits for {me}, so this would wait forever.", rf("task", after.id())));
    }
    if let Some(why) = wave_clash(t, &after, &group(app, &a)?) {
        return err(409, why);
    }
    Ok(Some(a.id()))
}

/// Group tasks in one goal run one after another: the task before has to be in an earlier wave, and no
/// other group task shares this one's wave.
fn wave_clash(t: &Row, pred: &Row, group: &[Row]) -> Option<String> {
    let (gid, w) = (t.i("goal_id")?, t.i("wave")?);
    let same = |o: &Row| o.id() != t.id() && o.i("goal_id") == Some(gid);
    if same(pred) && pred.i("wave").is_some_and(|p| p >= w) {
        return Some(format!(
            "{} has to be in an earlier wave, so its work is on the shared branch before this one starts.",
            rf("task", pred.id())
        ));
    }
    let o = group.iter().find(|o| same(o) && o.i("wave") == Some(w))?;
    Some(format!(
        "{} shares this PR and is in wave {w} too: tasks sharing a PR run one after another, so put them in different waves.",
        rf("task", o.id())
    ))
}

/// After a task's wave or goal changes: each task in its group still comes after the one before it, and
/// no two share a wave.
pub fn check_waves(app: &App, t: &Row) -> Result<()> {
    let a = match anchor(app, t)? {
        Some(a) => a,
        None if !members(app, t.id())?.is_empty() => t.clone(),
        None => return Ok(()),
    };
    let all = group(app, &a)?;
    for m in all.iter().filter(|m| anchor_id(m).is_some()) {
        let Some(pred) = predecessor(app, m)? else { continue };
        if let Some(why) = wave_clash(m, &pred, &all) {
            return err(409, format!("{}: {why}", rf("task", m.id())));
        }
    }
    Ok(())
}

/// Sets the PR a task shares from a body's `same_pr`, and has it wait for the group's last task; `none`
/// takes it out and drops that wait. Logs the change. False when it didn't change.
pub fn set(app: &App, t: &Row, value: &Value, who: &str) -> Result<bool> {
    let n = clean(app, value, t)?;
    if n == anchor_id(t) {
        return Ok(false);
    }
    let old = predecessor(app, t)?.map(|p| p.id());
    let mut waits: Vec<i64> = waitsfor::ids(t).into_iter().filter(|w| Some(*w) != old).collect();
    let text = match n {
        Some(a) => {
            let after = tail(app, &board::get_task(app, a)?, t.id())?.id();
            if !waits.contains(&after) {
                waits.push(after);
            }
            format!("Shares {}'s branch and PR: starts once {} is done", rf("task", a), rf("task", after))
        }
        None => "Shares no other task's PR".to_string(),
    };
    let waits = if waits.is_empty() { None } else { Some(jdumps(&json!(waits))) };
    board::update_task(app, t.id(), fields!["same_pr" => n, "waits_for" => waits])?;
    board::log_event(app, t.id(), who, "note", &text)?;
    Ok(true)
}

/// The branch a member's worktree starts from: the anchor's, while its PR is open.
pub fn start_branch(app: &App, t: &Row) -> Result<Option<String>> {
    Ok(anchor(app, t)?.filter(|a| !closed(a)).and_then(|a| stack::branch(&a)))
}

/// Where a member's review starts: the shared branch's tip when the task before it submitted, so the
/// review sees only this task's commits.
pub fn review_base(app: &App, t: &Row) -> Option<String> {
    anchor_id(t)?;
    let pred = predecessor(app, t).ok()??;
    board::task_context(&pred).get("same_pr_head").and_then(|h| h.as_str()).filter(|h| !h.is_empty()).map(|h| h.to_string())
}

/// Keeps the commit a group task submitted at, for the next task's review to start from.
pub fn note_head(app: &App, id: i64, head: Option<&str>) -> Result<()> {
    let Some(head) = head.filter(|h| !h.is_empty()) else { return Ok(()) };
    let mut ctx = board::task_context(&board::get_task(app, id)?);
    ctx.insert("same_pr_head".into(), json!(head));
    board::save_context(app, id, &ctx, false)
}

/// Why the anchor's PR holds: the first member that hasn't submitted yet.
pub fn hold(app: &App, t: &Row) -> Result<Option<String>> {
    if anchor_id(t).is_some() {
        return Ok(None);
    }
    let open = members(app, t.id())?.into_iter().find(|m| m.s("status") != Some("done") && !m.b("failed"));
    Ok(open.map(|m| format!("{} shares this PR and hasn't added its work yet", rf("task", m.id()))))
}

/// The handoff's lines about the shared PR: for a member, where its work goes and how it reaches the PR;
/// for an anchor, the members that come after it.
pub fn handoff_line(app: &App, t: &Row, remote: &str) -> Result<Option<String>> {
    let me = rf("task", t.id());
    let Some(a) = anchor(app, t)? else {
        let open: Vec<String> = members(app, t.id())?.iter().filter(|m| !board::handed_in(m)).map(|m| rf("task", m.id())).collect();
        if open.is_empty() {
            return Ok(None);
        }
        return Ok(Some(format!(
            "{} {} this task's branch and PR and {} after it: keep the branch name, and open the PR with tb submit --pr-body as usual. \
             It holds until each of them has added its work and its section.",
            open.join(", "),
            if open.len() == 1 { "shares" } else { "share" },
            if open.len() == 1 { "runs" } else { "run" },
        )));
    };
    let ra = rf("task", a.id());
    if stack::merged(&a) {
        return Ok(Some(format!("This task shared {ra}'s PR, which is merged now: build on the main branch as usual.")));
    }
    if waitsfor::pr_declined(&a) {
        return Ok(Some(format!(
            "This task shares {ra}'s PR, which was closed without merging: take it off (tb task set {me} --same-pr none) and build on the main branch."
        )));
    }
    let Some(b) = stack::branch(&a) else {
        return Ok(Some(format!("This task shares {ra}'s branch and PR, once its branch is known (tb status {ra}).")));
    };
    let pr = a.i("pr_num").map(|n| format!(", PR #{n}")).unwrap_or_default();
    let local = format!("{b}-{}", me.to_lowercase());
    Ok(Some(format!(
        "This task shares {ra}'s branch and PR ({b}{pr}): work on a branch made from it (git fetch {remote}, then git switch -c {local} {remote}/{b}) \
         and commit on top, rebasing onto {remote}/{b} if it moves (never merge). Push once, at submit: git push {remote} HEAD:{b}. \
         Then tb submit --pr-body as usual, with only this task's part in the file: the board adds it to {ra}'s PR as its own section \
         instead of opening another."
    )))
}

/// `tb submit --pr-body` for a member: its work is on the shared branch, its steps passed, its section
/// goes into the anchor's description and the task links the anchor's PR. False when the task isn't a
/// member of an open group, so the submit opens a PR as usual.
pub fn member_submit(app: &App, body: &mut Value, t: &Row, text: &str) -> Result<bool> {
    let Some(a) = anchor(app, t)?.filter(|a| !closed(a)) else { return Ok(false) };
    if has(t.s("pr_url")) {
        body["pr"] = json!(t.st("pr_url"));
        return Ok(true);
    }
    let (me, ra) = (rf("task", t.id()), rf("task", a.id()));
    if !body_str(body, "pr").is_empty() {
        return err(400, format!("{me} shares {ra}'s PR, so it adds to that one: drop --pr."));
    }
    let Some(pr) = crate::prhost::PrRef::of(&a) else {
        return err(409, format!("{ra} hasn't opened the PR this task shares yet. Wait for it: tb wait-for {ra}."));
    };
    let problems = crate::propen::body_problems_for(app, t, text)?;
    if !problems.is_empty() {
        return err(400, format!("This task's part of the PR description needs work:\n- {}\nFix the file and run tb submit again.", problems.join("\n- ")));
    }
    let Some(b) = stack::branch(&a) else { return err(409, format!("The board doesn't know {ra}'s branch, so it can't check this task's work is on it.")) };
    let Some(dir) = crate::propen::dir_for(t, &body_str(body, "cwd")) else { return err(409, "The board can't find this task's checkout.") };
    let remote = &app.cfg.pr_body.remote;
    let git = |args: &[&str]| crate::propen::git(&dir, args, 120.0).map_err(|e| ApiError::new(409, e));
    git(&["fetch", remote, &b])?;
    let (_, head, _) = git(&["rev-parse", "HEAD"])?;
    let (_, tip, _) = git(&["rev-parse", "--verify", "-q", &format!("refs/remotes/{remote}/{b}")])?;
    if head.is_empty() || head != tip {
        return err(409, format!("This task's work isn't on {b} as it stands: push it there (git push {remote} HEAD:{b}), rebasing onto {remote}/{b} first if it moved, then run tb submit again."));
    }
    let left = crate::steps::missing_at(app, t, &[crate::steps::Before::Pr, crate::steps::Before::Done], Some(&head))?;
    if !left.is_empty() {
        return err(409, crate::steps::refusal_at(app, t, "adding to the PR", &left, Some(&head)));
    }
    let section = crate::propen::member_section(&t.st("title"), text);
    let host = crate::prhost::host_for(app, &pr.host).map_err(|e| ApiError::new(502, e))?;
    let current = host.description(&pr).map_err(|e| ApiError::new(502, format!("Couldn't read {ra}'s PR #{}: {e}", pr.num)))?;
    if !current.contains(section.lines().next().unwrap_or_default()) {
        let next = crate::propen::with_section(&current, &section);
        host.set_description(&pr, &next).map_err(|e| ApiError::new(502, format!("Couldn't add this task's section to {ra}'s PR #{}: {e}", pr.num)))?;
    }
    app.db.tx(|| {
        board::update_task(
            app,
            t.id(),
            fields!["pr_host" => a.v("pr_host"), "pr_repo" => a.v("pr_repo"), "pr_url" => a.v("pr_url"),
                    "pr_state" => a.v("pr_state"), "pr_phase" => a.v("pr_phase")],
        )?;
        note_head(app, t.id(), Some(&head))?;
        board::log_event(app, t.id(), board::BOARD, "status", &format!("Added its section to {ra}'s PR #{}", pr.num)).map(|_| ())
    })?;
    body["pr"] = json!(a.st("pr_url"));
    Ok(true)
}

/// Plan-check warnings on a goal's open tasks: a shared PR with more tasks, or more planned files across
/// them, than one reader can follow.
pub fn plan_warnings(app: &App, tasks: &[Row]) -> Result<Vec<String>> {
    let anchors: BTreeSet<i64> = tasks.iter().filter_map(anchor_id).collect();
    let mut out = vec![];
    for id in anchors {
        let Some(a) = board::find_task(app, Some(id))? else { continue };
        let all = group(app, &a)?;
        let files: BTreeSet<String> = all.iter().flat_map(|g| str_list(board::task_context(g).get("plan_files"))).collect();
        let ra = rf("task", id);
        let last = rf("task", all.last().map(|g| g.id()).unwrap_or(id));
        if all.len() > MAX_TASKS {
            out.push(format!(
                "{} tasks share {ra}'s PR: more than {MAX_TASKS} is a lot for one reader. Give some their own PR (tb task set {last} --same-pr none).",
                all.len()
            ));
        } else if files.len() > MAX_FILES {
            out.push(format!(
                "The tasks sharing {ra}'s PR plan to change {} files: more than {MAX_FILES} is a lot for one reader. Give some their own PR (tb task set {last} --same-pr none).",
                files.len()
            ));
        }
    }
    Ok(out)
}

/// The group tasks a member works on top of, while the shared PR is open: their work is on its branch
/// already, so there's nothing to bring in.
pub fn shared_ids(app: &App, t: &Row) -> Result<Vec<i64>> {
    match anchor(app, t)?.filter(|a| !closed(a)) {
        Some(a) => Ok(group(app, &a)?.iter().map(|g| g.id()).filter(|n| *n != t.id()).collect()),
        None => Ok(vec![]),
    }
}
