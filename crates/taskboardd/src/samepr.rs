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

/// Why the anchor's PR holds: a backlog item that blocks it and isn't fixed yet, else the first member
/// that hasn't submitted yet.
pub fn hold(app: &App, t: &Row) -> Result<Option<String>> {
    if anchor_id(t).is_some() {
        return Ok(None);
    }
    if let Some(b) = holding(app, t.id())?.first() {
        let why = b.s("blocks_why").filter(|w| !w.is_empty()).unwrap_or(b.st("title").as_str()).to_string();
        return Ok(Some(format!("{} blocks this PR: {why}", rf("issue", b.id()))));
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

/// Why `t` can't become a member of the task that owns a PR.
pub fn cant_join(app: &App, t: &Row) -> Result<Option<String>> {
    let me = rf("task", t.id());
    if let Some(a) = anchor_id(t) {
        return Ok(Some(format!("{me} shares {}'s PR already.", rf("task", a))));
    }
    if let Some(p) = stack::parent_id(t) {
        return Ok(Some(format!("{me} stacks on {}; a task shares a PR or stacks on one, not both. Take it off first (--stack-on none).", rf("task", p))));
    }
    if !members(app, t.id())?.is_empty() {
        return Ok(Some(format!("Other tasks share {me}'s PR, so it can't share another.")));
    }
    Ok(None)
}

/// The other task that owns `pr`.
pub fn owner(app: &App, t: &Row, pr: &PrLink) -> Result<Option<Row>> {
    app.db.q1("SELECT * FROM tasks WHERE pr_repo = ? AND pr_num = ? AND id != ?", p![pr.repo, pr.num, t.id()])
}

/// `t`'s work went through `pr`, which another task owns: it becomes that task's member, so it counts as
/// merged when that PR does. Whether it joined; false when no other task owns the PR or `t` can't join.
pub fn join_owner(app: &App, t: &Row, pr: &PrLink, who: &str) -> Result<bool> {
    if has(t.s("pr_repo")) || crate::prflow::other_repo(app, t, pr).is_some() || cant_join(app, t)?.is_some() {
        return Ok(false);
    }
    let Some(a) = owner(app, t, pr)? else { return Ok(false) };
    board::update_task(
        app,
        t.id(),
        fields!["same_pr" => a.id(), "pr_host" => a.v("pr_host"), "pr_repo" => a.v("pr_repo"), "pr_url" => a.v("pr_url"),
                "pr_state" => a.v("pr_state"), "pr_phase" => a.v("pr_phase")],
    )?;
    board::log_event(app, t.id(), who, "status", &format!("Its work went through {}'s PR #{}", rf("task", a.id()), pr.num))?;
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

/// The `pr` items holding task `a`'s PR: open, or made into a task that isn't done (or failed), oldest first.
pub fn holding(app: &App, a: i64) -> Result<Vec<Row>> {
    app.db.q(
        "SELECT i.* FROM issues i LEFT JOIN tasks t ON t.id = i.task_id WHERE i.holds_pr = ? AND i.blocks = 'pr'
         AND (i.state = 'open' OR (i.state = 'task' AND (t.id IS NULL OR t.status != 'done' OR COALESCE(t.failed, 0) = 1))) ORDER BY i.id",
        p![a],
    )
}

/// The alert a `pr` item raises when its goal already has `[backlog] critical_max` of them made into tasks.
pub fn critical_key(issue: i64) -> String {
    format!("critical:{issue}")
}

fn fix_title(a: i64) -> String {
    format!("Pre-merge fixes for {}", rf("task", a))
}

fn fix_line(b: &Row) -> String {
    let why = b.s("blocks_why").filter(|w| !w.is_empty()).map(|w| format!(": {w}")).unwrap_or_default();
    format!("- {} {}{why}", rf("issue", b.id()), b.st("title"))
}

/// The group's pre-merge fix task that hasn't started yet, which more items join.
fn open_fix(app: &App, a: i64) -> Result<Option<Row>> {
    app.db.q1(
        "SELECT t.* FROM tasks t JOIN issues i ON i.id = t.from_issue_id WHERE t.same_pr = ? AND i.holds_pr = ?
         AND t.status IN ('planned', 'queued') AND t.started_at IS NULL AND COALESCE(t.failed, 0) = 0 ORDER BY t.id DESC LIMIT 1",
        p![a, a],
    )
}

/// Items made into pre-merge fixes in `a`'s goal (or, outside a goal, for `a`'s PR).
fn promoted(app: &App, a: &Row) -> Result<i64> {
    let sql = "SELECT COUNT(*) AS n FROM issues i JOIN tasks a ON a.id = i.holds_pr WHERE i.blocks = 'pr' AND i.state = 'task' AND ";
    let row = match a.i("goal_id") {
        Some(g) => app.db.q1(&format!("{sql}a.goal_id = ?"), p![g])?,
        None => app.db.q1(&format!("{sql}a.id = ?"), p![a.id()])?,
    };
    Ok(row.map(|r| r.i0("n")).unwrap_or(0))
}

/// Whether more work can still go on `a`'s branch before its PR merges.
fn takes_fixes(app: &App, a: &Row) -> Result<bool> {
    Ok(!closed(a) && !crate::design::is(a) && !crate::human::is(a) && crate::projects::task_ships_pr(app, a)?)
}

/// A `pr` item filed from task `filer` holds its group's PR (the filer's anchor, or the filer when it
/// has none) and becomes part of one pre-merge fix task that shares the PR: the first item makes it,
/// later ones join it until it starts, and one after that makes another. Making it a task steps the PR
/// again, so one already at merge goes back to waiting before anything wakes for the merge. Past `[backlog] critical_max`
/// items in the goal, the owner hears of it instead and it holds the PR as it is. When the PR is already
/// merged or closed, it blocks the goal and becomes an ordinary task there. Says what became of it.
pub fn take_critical(app: &App, iid: i64, filer: &Row, who: &str) -> Result<Option<String>> {
    let b = board::get_issue(app, iid)?;
    if b.s("state") != Some("open") {
        return Ok(None);
    }
    let a = anchor(app, filer)?.unwrap_or_else(|| filer.clone());
    let ra = rf("task", a.id());
    if !takes_fixes(app, &a)? {
        return goal_instead(app, &b, &a, who).map(Some);
    }
    app.db.update("issues", &json!(iid), fields!["holds_pr" => a.id(), "updated_at" => now_iso()])?;
    let b = board::get_issue(app, iid)?;
    let max = app.cfg.backlog.critical_max.max(0);
    let text = if promoted(app, &a)? >= max {
        let place = a.i("goal_id").map(|g| rf("goal", g)).unwrap_or_else(|| format!("{ra}'s PR"));
        let text = format!(
            "{} holds {ra}'s PR: {}. {place} already has {max} items made into pre-merge fixes, so this one waits for you: make it a task or close it.",
            rf("issue", iid),
            b.st("title")
        );
        crate::dispatch::add_alert_keyed(app, &text, Some(a.id()), a.i("goal_id"), &critical_key(iid))?;
        board::add_issue_event(app, iid, who, "blocks", &format!("Holds {ra}'s PR. Past the {max} pre-merge fixes a goal makes, so it went to {}.", app.cfg.owner), None)?;
        restep(app, a.id())?;
        format!("It holds {ra}'s PR; past the {max} pre-merge fixes a goal makes, so it went to {} to decide.", app.cfg.owner)
    } else if let Some(fix) = open_fix(app, a.id())? {
        let rfix = rf("task", fix.id());
        let detail = format!("{}\n{}", fix.st("detail").trim_end(), fix_line(&b));
        board::update_task(app, fix.id(), fields!["detail" => detail])?;
        board::log_event(app, fix.id(), who, "note", &format!("Added {}: {}", rf("issue", iid), b.st("title")))?;
        crate::api::issue_tasked(app, iid, fix.id(), who, &format!("Added to {rfix}, the pre-merge fix task for {ra}'s PR."))?;
        if !asks(app) {
            return Ok(Some(format!("It holds {ra}'s PR and was added to {rfix}, its pre-merge fix task.")));
        }
        if fix.s("status") == Some("queued") {
            board::update_task(app, fix.id(), fields!["status" => "planned"])?;
            board::log_event(app, fix.id(), board::BOARD, "status", &format!("Planned: {} approves {} first", app.cfg.owner, rf("issue", iid)))?;
        }
        ask_owner(app, iid, &a, fix.id())?;
        format!("It holds {ra}'s PR and was added to {rfix}, its pre-merge fix task, which waits for {} to approve it.", app.cfg.owner)
    } else {
        let id = make_fix(app, &b, &a, who)?;
        let rfix = rf("task", id);
        if !asks(app) {
            return Ok(Some(format!("It holds {ra}'s PR until {rfix}, its pre-merge fix task, is done.")));
        }
        ask_owner(app, iid, &a, id)?;
        format!("It holds {ra}'s PR. {rfix}, its pre-merge fix task, waits for {} to approve it.", app.cfg.owner)
    };
    Ok(Some(text))
}

/// The alert a `pr` item raises while it waits for the owner to approve or reject it.
pub const ASK_PREFIX: &str = "critical-ask:";

pub fn ask_key(issue: i64) -> String {
    format!("{ASK_PREFIX}{issue}")
}

/// Whether the owner approves each `pr` item before its fix task runs (`[backlog] critical`).
fn asks(app: &App) -> bool {
    !app.cfg.backlog.critical.trim().eq_ignore_ascii_case("auto")
}

/// Whether task `id` fixes an item that still waits for the owner, so nothing but the owner's answer
/// queues it.
pub fn awaits_owner(app: &App, id: i64) -> Result<bool> {
    Ok(app.db.count("SELECT COUNT(*) FROM issues WHERE task_id = ? AND COALESCE(asks_owner, 0) = 1", p![id])? > 0)
}

/// Puts item `iid`, made part of fix task `fix` for `a`'s PR, on the owner's Needs you list to approve
/// or reject. It stays there until they answer.
fn ask_owner(app: &App, iid: i64, a: &Row, fix: i64) -> Result<()> {
    app.db.update("issues", &json!(iid), fields!["asks_owner" => 1, "updated_at" => now_iso()])?;
    let b = board::get_issue(app, iid)?;
    let (rb, ra, rfix) = (rf("issue", iid), rf("task", a.id()), rf("task", fix));
    let pr = a.i("pr_num").map(|n| format!("PR #{n}")).unwrap_or_else(|| "PR".into());
    let why = b.s("blocks_why").filter(|w| !w.trim().is_empty()).unwrap_or(b.st("title").as_str()).trim().to_string();
    let text = format!(
        "{rb} holds {ra}'s {pr}: {}. Why: {why}. Approve it and {rfix} fixes it before the merge; reject it and it goes back to the backlog and stops holding the PR.",
        b.st("title")
    );
    let action = |label: &str, n: u8, what: &str| {
        let command = format!("tb needs answer {rb} --pick {n}");
        crate::dispatch::AlertAction { label: label.into(), prompt: format!("Run `{command}` to {what}."), command }.json()
    };
    let fields = json!({
        "title": format!("{rb} holds {ra}'s {pr}"),
        "detail": why,
        "project": a.v("project"),
        "issue": rb,
        "actions": [
            action("Approve", 1, &format!("approve {rb}, so {rfix} fixes it before {ra}'s {pr} merges")),
            action("Reject", 2, &format!("reject {rb}, so it goes back to the backlog and stops holding {ra}'s {pr}")),
        ],
    });
    crate::dispatch::add_alert_keyed_with(app, &text, None, a.i("goal_id"), &ask_key(iid), fields)
}

/// `POST /backlog/:id/critical {approve}`: the owner's answer on a `pr` item that waits for them.
/// Approving lets its fix task run: queued (in a goal, once the goal runs). Rejecting sends it back to the
/// plain backlog, holding nothing, and takes it out of its fix task: an unstarted task left with nothing
/// to fix goes, a started one stops sharing the PR. Either way the PR steps again. Gives back what it
/// did and the task it deleted, if any.
pub fn answer(app: &App, iid: i64, approve: bool, who: &str) -> Result<(Value, Option<i64>)> {
    let b = board::get_issue(app, iid)?;
    let rb = rf("issue", iid);
    if !b.b("asks_owner") {
        return err(409, format!("{rb} isn't waiting for approval."));
    }
    let fix = board::find_task(app, b.i("task_id"))?;
    let ra = b.i("holds_pr").map(|a| rf("task", a)).unwrap_or_else(|| "its task".into());
    crate::dispatch::clear_alert_key(app, &ask_key(iid))?;
    let (text, gone) = if approve {
        (approved(app, &b, fix.as_ref(), &ra, who)?, None)
    } else {
        rejected(app, &b, fix.as_ref(), &ra, who)?
    };
    if let Some(a) = b.i("holds_pr") {
        restep(app, a)?;
    }
    Ok((json!({"ok": true, "text": text, "issue": rb, "task": fix.map(|t| rf("task", t.id()))}), gone))
}

fn approved(app: &App, b: &Row, fix: Option<&Row>, ra: &str, who: &str) -> Result<String> {
    let rb = rf("issue", b.id());
    app.db.update("issues", &json!(b.id()), fields!["asks_owner" => 0, "updated_at" => now_iso()])?;
    board::add_issue_event(app, b.id(), who, "blocks", &format!("Approved: it holds {ra}'s PR until it's fixed."), None)?;
    let Some(t) = fix else { return Ok(format!("Approved {rb}: it holds {ra}'s PR.")) };
    let rfix = rf("task", t.id());
    if t.s("status") != Some("planned") {
        return Ok(format!("Approved {rb}: {rfix} fixes it before {ra}'s PR merges."));
    }
    if awaits_owner(app, t.id())? {
        return Ok(format!("Approved {rb}. {rfix} still waits for the other items in it."));
    }
    if release(app, t, who)? {
        return Ok(format!("Approved {rb}: {rfix} is queued to fix it before {ra}'s PR merges."));
    }
    Ok(format!("Approved {rb}: {rfix} runs with its goal, before {ra}'s PR merges."))
}

/// Planned fix task `t` no longer waits for the owner: queued outside a goal or in a running one,
/// otherwise it stays planned until the goal runs. Whether it was queued.
fn release(app: &App, t: &Row, who: &str) -> Result<bool> {
    if t.s("status") != Some("planned") || awaits_owner(app, t.id())? {
        return Ok(false);
    }
    let (status, why) = match t.i("goal_id") {
        Some(g) => board::joining_status(app, g, false)?,
        None => ("queued", "queued".to_string()),
    };
    if status != "queued" {
        return Ok(false);
    }
    board::update_task(app, t.id(), fields!["status" => "queued"])?;
    board::log_event(app, t.id(), who, "status", &format!("Approved, so {why}"))?;
    Ok(true)
}

fn rejected(app: &App, b: &Row, fix: Option<&Row>, ra: &str, who: &str) -> Result<(String, Option<i64>)> {
    let rb = rf("issue", b.id());
    app.db.update(
        "issues",
        &json!(b.id()),
        fields!["blocks" => crate::blocks::NONE, "holds_pr" => null, "asks_owner" => 0, "state" => "open", "task_id" => null, "updated_at" => now_iso()],
    )?;
    board::add_issue_event(app, b.id(), who, "blocks", &format!("Rejected: back in the backlog, and it no longer holds {ra}'s PR."), None)?;
    let back = format!("Rejected {rb}: it's back in the backlog and no longer holds {ra}'s PR");
    let Some(t) = fix else { return Ok((format!("{back}."), None)) };
    let rfix = rf("task", t.id());
    let left = app.db.q("SELECT id FROM issues WHERE task_id = ? ORDER BY id", p![t.id()])?;
    if let Some(first) = left.first() {
        let line = format!("- {rb} ");
        let detail = t.st("detail");
        let detail: Vec<&str> = detail.lines().filter(|l| !l.starts_with(&line)).collect();
        let mut f = fields!["detail" => detail.join("\n")];
        if t.i("from_issue_id") == Some(b.id()) {
            f.extend(fields!["from_issue_id" => first.id()]);
        }
        board::update_task(app, t.id(), f)?;
        board::log_event(app, t.id(), who, "note", &format!("Dropped {rb}: {} rejected it", app.cfg.owner))?;
        release(app, &board::get_task(app, t.id())?, who)?;
        return Ok((format!("{back}; {rfix} fixes the rest."), None));
    }
    let unstarted = matches!(t.s("status"), Some("planned") | Some("queued")) && t.s("started_at").is_none();
    if unstarted {
        crate::api::purge_tasks(app, &[t.id()])?;
        if let Some(g) = t.i("goal_id") {
            board::renumber_goal(app, g)?;
        }
        return Ok((format!("{back}; {rfix} had nothing else to fix, so it's gone."), Some(t.id())));
    }
    board::update_task(app, t.id(), fields!["from_issue_id" => null])?;
    set(app, &board::get_task(app, t.id())?, &json!("none"), who)?;
    board::log_event(app, t.id(), who, "note", &format!("{} rejected {rb}, its only item, after it started: it no longer shares {ra}'s PR", app.cfg.owner))?;
    Ok((format!("{back}; {rfix} had started, so it no longer shares the PR."), None))
}

/// Task `id` is going: the items in it that waited for the owner don't any more.
pub fn forget_asks(app: &App, id: i64) -> Result<()> {
    for b in app.db.q("SELECT id FROM issues WHERE task_id = ? AND COALESCE(asks_owner, 0) = 1", p![id])? {
        app.db.update("issues", &json!(b.id()), fields!["asks_owner" => 0])?;
        crate::dispatch::clear_alert_key(app, &ask_key(b.id()))?;
    }
    Ok(())
}

/// The pre-merge fix task for `a`'s PR, made from item `b`: it shares the PR, in `a`'s goal, one wave
/// after the group's last task.
fn make_fix(app: &App, b: &Row, a: &Row, who: &str) -> Result<i64> {
    let ra = rf("task", a.id());
    let detail = format!(
        "These backlog items hold {ra}'s PR: it can't merge until each is fixed on its branch. Read each one's report (tb backlog show) before changing anything.\n{}",
        fix_line(b)
    );
    let mut body = crate::api::issue_task_fields(app, b, who);
    body["title"] = json!(fix_title(a.id()));
    body["detail"] = json!(detail);
    body["project"] = a.v("project");
    body["latest"] = json!(format!("Holds {ra}'s PR: fixes what was found before it merges."));
    body["same_pr"] = json!(ra);
    let mut log = format!("Added to fix what holds {ra}'s PR ({})", rf("issue", b.id()));
    if asks(app) {
        body["status"] = json!("planned");
    }
    if let Some(g) = a.i("goal_id") {
        let (status, why) = match asks(app) {
            true => ("planned", format!("planned: {} approves it first", app.cfg.owner)),
            false => board::joining_status(app, g, false)?,
        };
        body["goal_id"] = json!(g);
        body["status"] = json!(status);
        if let Some(w) = tail(app, a, 0)?.i("wave") {
            body["wave"] = json!(w + 1);
        }
        log += &format!(", {why}");
    }
    let c = crate::ops::new_task(app, &body, who, Some(&log))?;
    let id = c["id"].as_i64().unwrap_or(0);
    let rfix = rf("task", id);
    crate::api::issue_tasked(app, b.id(), id, who, &format!("Made into {rfix}, the pre-merge fix task that shares {ra}'s PR."))?;
    Ok(id)
}

/// A `pr` item whose PR is already merged or closed (or that has no PR to hold) blocks the goal instead
/// and becomes an ordinary task in it; outside a goal it blocks nothing.
fn goal_instead(app: &App, b: &Row, a: &Row, who: &str) -> Result<String> {
    let ra = rf("task", a.id());
    let why = if stack::merged(a) {
        format!("{ra}'s PR is already merged")
    } else if waitsfor::pr_declined(a) {
        format!("{ra}'s PR is already closed")
    } else {
        format!("{ra} has no PR to hold")
    };
    let Some(g) = b.i("goal_id").or(a.i("goal_id")) else {
        app.db.update("issues", &json!(b.id()), fields!["blocks" => crate::blocks::NONE, "holds_pr" => null, "updated_at" => now_iso()])?;
        board::add_issue_event(app, b.id(), who, "blocks", &format!("{why} and this isn't in a goal, so it blocks nothing."), None)?;
        return Ok(format!("{why} and it isn't in a goal, so it blocks nothing."));
    };
    app.db.update(
        "issues",
        &json!(b.id()),
        fields!["blocks" => crate::blocks::GOAL, "misses_goal" => 1, "goal_id" => g, "holds_pr" => null, "updated_at" => now_iso()],
    )?;
    board::add_issue_event(app, b.id(), who, "blocks", &format!("{why}, so it blocks {} instead.", rf("goal", g)), None)?;
    let b = board::get_issue(app, b.id())?;
    let (status, join) = board::joining_status(app, g, false)?;
    let mut body = crate::api::issue_task_fields(app, &b, who);
    body["goal_id"] = json!(g);
    body["status"] = json!(status);
    let c = crate::ops::new_task(app, &body, who, Some(&format!("Added from the backlog ({}), {join}", rf("issue", b.id()))))?;
    let id = c["id"].as_i64().unwrap_or(0);
    let rt = rf("task", id);
    crate::api::issue_tasked(app, b.id(), id, who, &format!("Made into {rt}, a {status} task in its goal: {why}."))?;
    Ok(format!("{why}, so it blocks the goal instead, as {rt}."))
}

/// An item that holds a PR was made a task, ticketed or closed: its alert goes, and the PR steps again so
/// the hold shows at once (it lifts unless a task it was made into is still to do).
pub fn critical_settled(app: &App, b: &Row) -> Result<()> {
    crate::dispatch::clear_alert_key(app, &critical_key(b.id()))?;
    match b.i("holds_pr") {
        Some(a) if b.s("blocks") == Some(crate::blocks::PR) => restep(app, a),
        _ => Ok(()),
    }
}

/// Steps task `a`'s open PR again with its last read, so a hold that came or went since shows now: a
/// PR at merge goes back to waiting before anything wakes for the merge.
fn restep(app: &App, a: i64) -> Result<()> {
    let Some(t) = board::find_task(app, Some(a))? else { return Ok(()) };
    if !board::pr_still_open(&t) {
        return Ok(());
    }
    match crate::prflow::flow(&t).get("rec").filter(|r| r.is_object()) {
        Some(rec) => crate::prflow::step(app, &t, rec).map(|_| ()),
        None => Ok(()),
    }
}
