//! The handoff: everything a new terminal needs to pick a task up.

use serde_json::Value;

use crate::app::App;
use crate::util::*;
use crate::{board, p, projects, shared, steps, waitsfor};

const LIMIT: usize = 6000;
const NOTES_LIMIT: usize = 1500;
pub const CLOSING: &str = "Check the branch and uncommitted files before changing anything and report a checkpoint, \
then carry on with the work in the same turn. Don't stop to wait for a go-ahead.";

pub fn marker(task_id: i64) -> String {
    format!("[task-board:{}]", rf("task", task_id))
}

fn joined(items: &[Value], limit: usize) -> String {
    let parts: Vec<String> = items
        .iter()
        .filter_map(|i| i.as_str())
        .map(|i| i.trim().trim_end_matches('.').to_string())
        .filter(|i| !i.is_empty())
        .collect();
    clip(&parts.join("; "), limit)
}

fn note_label(kind: &str) -> &'static str {
    match kind {
        "finding" => "Finding",
        "decision" => "Decision",
        "reference" => "Reference",
        _ => "Note",
    }
}

pub fn notes_block(notes: &[Row], limit: usize) -> String {
    let mut lines = vec![];
    let mut used = 0;
    for n in notes {
        let src = n.s("source").filter(|s| !s.is_empty()).map(|s| format!(" ({s})")).unwrap_or_default();
        let line = format!("- {}: {}{src}", note_label(&n.st("kind")), n.st("text").trim());
        if used + line.chars().count() + 1 > limit {
            break;
        }
        used += line.chars().count() + 1;
        lines.push(line);
    }
    if lines.len() < notes.len() {
        lines.push(format!("- {} more on the goal page.", notes.len() - lines.len()));
    }
    lines.join("\n")
}

fn snap_label(k: &str) -> String {
    match k {
        "task" => "Task".into(),
        "step" => "Step".into(),
        "terminal" => "Terminal".into(),
        "branch" => "Branch".into(),
        "worktree" => "Worktree".into(),
        "last_commit" | "commit" => "Last commit".into(),
        "uncommitted" => "Not committed".into(),
        "last_turn" => "Last turn".into(),
        "output" => "Output".into(),
        other => {
            let s = other.replace('_', " ");
            let mut c = s.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

pub fn snapshot_line(snap: &Row) -> String {
    snap.iter()
        .filter(|(_, v)| !v.is_null() && v.as_str() != Some(""))
        .map(|(k, v)| {
            let val = if k == "uncommitted" && v.is_i64() {
                let n = v.as_i64().unwrap();
                if n == 0 {
                    "nothing".to_string()
                } else {
                    plural(n, "file")
                }
            } else {
                clip(v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string()).as_str(), 300)
            };
            format!("{}: {val}", snap_label(k))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

pub fn report_block(app: &App, tb: &str) -> String {
    let owner = &app.cfg.owner;
    [
        "How to report (run these in this terminal; the board knows which task you're on):".to_string(),
        format!("- Progress: {tb} note \"<what happened>\" (add --goal to put it in the goal notes)"),
        format!("- At each milestone and before a long step: {tb} checkpoint --done \"…\" --next \"…\" --decision \"…\""),
        format!("- A problem outside this task: {tb} found \"<problem>\" --kind bug|gap|follow|clean --output \"<excerpt>\""),
        format!(
            "- The goal's own done-when isn't met: {tb} found \"<what misses>\" --misses-goal. It goes to {owner}, not the \
             backlog, and the goal stays open until it's settled"
        ),
        format!(
            "- Needs another task's work that isn't there yet: {tb} wait-for T<n> --why \"<what you need>\". Don't ask \
             {owner}. The board starts that task and carries this conversation on once it's done (add --merged when \
             you need its PR merged first)"
        ),
        format!(
            "- A question for {owner}: {tb} question \"<question>\", then wait for the answer (if it prints the board's \
             answer instead, carry on)"
        ),
        format!("- Finished: {tb} submit \"<summary>\" (--pr <link> for a PR that was open before the task)"),
        format!("- Can't be done: {tb} fail \"<why>\""),
        "If you find a problem outside this task, report it with found instead of fixing it.".to_string(),
    ]
    .join("\n")
}

fn pr_block(app: &App, t: &Row, tb: &str) -> Result<String> {
    let ships = projects::task_ships_pr(app, t)?;
    let own = steps::handoff_block(app, t, tb, ships);
    let mut s = pr_lines(app, t, ships)?;
    if let Some(gaps) = crate::setup::handoff_line(app, &t.st("project"))? {
        s = format!("{s}\n{gaps}");
    }
    Ok(if own.is_empty() { s } else { format!("{s}\n{own}") })
}

fn pr_lines(app: &App, t: &Row, ships: bool) -> Result<String> {
    if !ships {
        let why = if t.i("ships_pr") == Some(0) { "This task ends without a PR" } else { "This project has no git remote, so the task ends without a PR" };
        return Ok(format!("{why}, so it has no Jira ticket. Don't make or link one; a branch it needs is named <type>/<short-slug>."));
    }
    let mut s = "If this task changes code, it ends in one pull request: finished code that builds, breaks nothing \
                 that works today, and can be merged on its own. Keep it small and easy to review; split anything \
                 bigger off with tb found. The board opens the PR: push the rebased branch, write the description to a \
                 file and finish with tb submit \"<summary>\" --pr-body <file> (it checks the description and that the \
                 branch is pushed and rebased, then opens the PR). Opening it yourself from this terminal is refused."
        .to_string();
    let branch = branch_name(app, t)?;
    let key = t.s("jira_key").filter(|k| !k.is_empty());
    match (key, &branch) {
        (Some(k), Some(_)) => s += &format!(" Its Jira ticket is {k}: put the key in the commit messages and the PR title."),
        (Some(k), None) => s += &format!(" Its Jira ticket is {k}: put the key in the branch name, the commit messages and the PR title."),
        _ => {}
    }
    if let Some(b) = &branch {
        let setup = board::find_goal(app, t.i("goal_id"))?.is_some_and(|g| g.s("setup").is_some_and(|x| !x.trim().is_empty()));
        s += &format!(" Name its branch {b}{}.", if setup { " unless the goal's setup says otherwise" } else { "" });
    }
    s += " A PR that was already open before the task is linked with tb submit \"<summary>\" --pr <link>. If it turns \
          out no PR is needed after all, finish with tb submit \"<summary>\" --no-pr \"<why>\".";
    if app.cfg.pr.watch && app.cfg.pr.wake {
        s += " Once it's done, the board watches the PR and brings this conversation back when a check fails or a \
              reviewer comments.";
    }
    Ok(s)
}

fn first_line(t: &Row) -> String {
    let mut first = format!(
        "{} You are picking up “{}” in {}",
        marker(t.id()),
        t.st("title"),
        t.s("project").filter(|p| !p.is_empty()).unwrap_or("this project")
    );
    // A task that ends without a PR has no ticket, whatever key it kept from before.
    if let Some(k) = t.s("jira_key").filter(|k| !k.is_empty() && t.i("ships_pr") != Some(0)) {
        first += &format!(" ({k})");
    }
    first + "."
}

fn goal_and_notes(app: &App, g: Option<&Row>) -> Result<Vec<String>> {
    let Some(g) = g else { return Ok(vec![]) };
    let mut line = format!("It is part of the goal “{}”", g.st("name"));
    if let Some(o) = g.s("outcome").filter(|o| !o.trim().is_empty()) {
        line += &format!(", done when {}", o.trim().trim_end_matches('.'));
    }
    line += ".";
    if let Some(tldr) = g.s("tldr").filter(|t| !t.trim().is_empty()) {
        line += &format!(" In short: {}", clip(tldr.trim(), 500));
    }
    let notes = board::goal_notes(app, g.id())?;
    if !notes.is_empty() {
        line += &format!(" Read the goal notes first.\nGoal notes:\n{}", notes_block(&notes, NOTES_LIMIT));
    }
    Ok(vec![line])
}

/// The other goals a shared task also finishes, so its one change (and PR) serves them too.
fn also_goals(app: &App, t: &Row) -> Result<Vec<String>> {
    let mut gs = vec![];
    for n in shared::goal_ids(app, t.id())? {
        if let Some(g) = board::find_goal(app, Some(n))? {
            gs.push(g);
        }
    }
    if gs.is_empty() {
        return Ok(vec![]);
    }
    let mut lines = vec![format!(
        "This task's work also finishes {}, so make the one change (and PR) serve them too:",
        if gs.len() == 1 { "another goal".to_string() } else { format!("{} other goals", gs.len()) }
    )];
    for g in &gs {
        let mut line = format!("- {} “{}”", rf("goal", g.id()), g.st("name"));
        if let Some(o) = g.s("outcome").filter(|o| !o.trim().is_empty()) {
            line += &format!(", done when {}", o.trim().trim_end_matches('.'));
        }
        lines.push(line + ".");
        let notes = board::goal_notes(app, g.id())?;
        if !notes.is_empty() {
            lines.push(notes_block(&notes, NOTES_LIMIT / 2));
        }
    }
    Ok(vec![lines.join("\n")])
}

fn branch_and_last_commit(w: &Row) -> Vec<String> {
    let mut wl = vec![];
    if let Some(b) = w.s("branch").filter(|s| !s.is_empty()) {
        wl.push(format!("Branch: {b}"));
    }
    if let Some(c) = w.s("last_commit").filter(|s| !s.is_empty()) {
        wl.push(format!("Last commit: {c}"));
    }
    if let Some(n) = w.i("uncommitted").filter(|n| *n > 0) {
        wl.push(format!("Not committed: {}", plural(n, "file")));
    }
    if wl.is_empty() {
        vec![]
    } else {
        vec![wl.join("\n")]
    }
}

fn arr(ctx: &Row, k: &str) -> Vec<Value> {
    ctx.get(k).and_then(|v| v.as_array()).cloned().unwrap_or_default()
}

fn checkpoint_and_answers(app: &App, t: &Row, ctx: &Row) -> Vec<String> {
    let mut parts = vec![];
    let (done, next, dec, ans) = (arr(ctx, "done"), arr(ctx, "next"), arr(ctx, "decisions"), arr(ctx, "answers"));
    if !done.is_empty() {
        parts.push(format!("Done: {}.", joined(&done, 900)));
    }
    if !next.is_empty() {
        parts.push(format!("Next: {}.", joined(&next, 900)));
    }
    if !dec.is_empty() {
        parts.push(format!("Keep: {}.", joined(&dec, 700)));
    }
    if !ans.is_empty() {
        parts.push(format!("{} answers: {}.", capitalize_first(&app.cfg.owners()), joined(&ans, 700)));
    }
    let meta = jloads_arr(t.s("meta"));
    let pairs: Vec<String> = meta
        .iter()
        .filter_map(|m| m.as_array())
        .filter(|m| m.len() == 2 && m[0].as_str().map(|k| !k.is_empty()).unwrap_or(false))
        .map(|m| format!("{}: {}", m[0].as_str().unwrap(), m[1].as_str().map(|s| s.to_string()).unwrap_or_else(|| m[1].to_string())))
        .collect();
    if !pairs.is_empty() {
        parts.push(format!("Details: {}.", clip(&pairs.join("; "), 400)));
    }
    parts
}

fn capitalize_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn attachments_block(app: &App, t: &Row, g: Option<&Row>) -> Result<Vec<String>> {
    let mut atts = board::attachments(app, Some(t.id()), None)?;
    if let Some(g) = g {
        atts.extend(board::attachments(app, None, Some(g.id()))?);
    }
    if atts.is_empty() {
        return Ok(vec![]);
    }
    let label = |k: &str| match k {
        "design" => "Design",
        "proposal" => "Proposal",
        "doc" => "Doc",
        "evidence" => "Evidence",
        "results" => "Results",
        _ => "Link",
    };
    let lines: Vec<String> = atts
        .iter()
        .take(12)
        .map(|a| {
            format!(
                "- {}{}: {} {}{}",
                label(a["kind"].as_str().unwrap_or("")),
                if a["goal"].is_string() { " (goal)" } else { "" },
                a["title"].as_str().unwrap_or(""),
                a["url"].as_str().unwrap_or(""),
                a["summary"].as_str().map(|s| format!(" — {s}")).unwrap_or_default()
            )
        })
        .collect();
    let mut parts = vec![format!("Attached (read these before you start):\n{}", lines.join("\n"))];
    if atts.iter().any(|a| a["kind"] == "design") {
        parts.push(design_check(app, t));
    }
    Ok(parts)
}

/// The folder a task's screenshots go in: under the board's data dir, so they outlive the task's worktree
/// and never end up in a commit.
pub fn evidence_dir(app: &App, task_id: i64) -> std::path::PathBuf {
    app.cfg.data.join("evidence").join(rf("task", task_id))
}

fn design_check(app: &App, t: &Row) -> String {
    let tb = board::tb_cmd(app);
    let dir = evidence_dir(app, t.id());
    let _ = std::fs::create_dir_all(&dir);
    format!(
        "Design check: the attached designs are the reference for anything on screen. Open them before you change any UI \
         and build to them, not from memory. {tb} submit needs evidence for a task with a design: a screenshot of each \
         changed screen, or --no-evidence \"<why>\" when no screen changed.\n\
         - Run the app on this task's emulator or simulator (the device the board lent it) and take a screenshot of each \
         changed screen.\n\
         - Compare each one with its design side by side until they match; say why when one can't.\n\
         - Save each screenshot in {}/, outside the worktree, so it outlives the worktree and never ends up in a commit.\n\
         - Attach each one: {tb} attach <file> --kind evidence --title \"<screen>: matches <design>\".\n\
         - In {tb} submit, list the screens you checked and anything that still differs. The evidence goes into the PR's \
         description.",
        dir.display()
    )
}

fn backlog_origin(app: &App, t: &Row) -> Result<Vec<String>> {
    let Some(b) = board::find_issue(app, t.i("from_issue_id"))? else { return Ok(vec![]) };
    let mut s = format!("This task came from the backlog ({}).", rf("issue", b.id()));
    if let Some(h) = b.s("how").filter(|h| !h.trim().is_empty()) {
        s += &format!(" {}", h.trim());
    }
    if let Some(said) = b.s("said").filter(|h| !h.trim().is_empty()) {
        s += &format!(" They said: {}", clip(said.trim(), 500));
    }
    let snap = snapshot_line(&jloads_obj(b.s("snapshot")));
    if !snap.is_empty() {
        s += &format!("\nWhat was happening then: {}.", clip(&snap, 900));
    }
    Ok(vec![s])
}

fn goal_open_issues(app: &App, t: &Row, g: Option<&Row>) -> Result<Vec<String>> {
    let Some(g) = g else { return Ok(vec![]) };
    let issues = app.db.q(
        "SELECT id, title FROM issues WHERE goal_id = ? AND state = 'open' AND id IS NOT ? ORDER BY created_at DESC, id DESC",
        p![g.id(), t.i("from_issue_id")],
    )?;
    if issues.is_empty() {
        return Ok(vec![]);
    }
    let mut shown: Vec<String> =
        issues.iter().take(6).map(|b| format!("{} {}", rf("issue", b.id()), b.st("title").trim().trim_end_matches('.'))).collect();
    if issues.len() > 6 {
        shown.push(format!("{} more on the goal page", issues.len() - 6));
    }
    Ok(vec![format!(
        "Known issues in the goal’s backlog, not part of this task: {}. Leave them unless they block you.",
        clip(&shown.join("; "), 700)
    )])
}

fn other_tasks(app: &App, t: &Row) -> Result<Vec<String>> {
    let mut out = vec![];
    if let Some(line) = crate::stack::handoff_line(app, t, &app.cfg.pr_body.remote)? {
        out.push(line);
    }
    let text = waitsfor::bring_in_text(app, t)?;
    if !text.is_empty() {
        out.push(text);
    }
    if let Some(text) = crate::human::goal_text(app, t)? {
        out.push(text);
    }
    let waiting = waitsfor::waiting_on(app, t)?;
    if !waiting.is_empty() {
        let names: Vec<String> =
            waiting.iter().take(6).map(|w| format!("{} {}", rf("task", w.id()), short(&w.st("title"), 50))).collect();
        out.push(format!(
            "Waiting for this task's work: {}. They start once this task is done, so finish it and push its branch.",
            names.join(", ")
        ));
    }
    Ok(out)
}

/// The task's lane, when it or its goal names one or the board picked a slot.
fn lane_line(app: &App, t: &Row) -> Result<Option<String>> {
    let (lane, from) = crate::worklane::picked(app, t)?;
    let by = match from {
        "task" => "it's set on the task",
        "goal" => "its goal's lane",
        _ => "the board picked it: a wave mate plans files that overlap its own, one of them plans none, or a wave mate has the shared checkout",
    };
    Ok(match lane.as_str() {
        "slot" => Some(format!("Lane: slot ({by}). The task works in a worktree slot from the board's pool, apart from the shared checkout.")),
        "shared" if from != "auto" => Some(format!("Lane: shared ({by}). The task works in the project's shared checkout; don't move to a worktree.")),
        "alone" => Some(format!("Lane: alone ({by}).")),
        _ => None,
    })
}

/// The worktree slot or worktree the board gave the task, its lane, the locks it holds, and whether it
/// runs alone.
fn worktree_and_lock_lines(app: &App, t: &Row, ctx: &Row) -> Result<Vec<String>> {
    let mut lines = vec![];
    lines.extend(lane_line(app, t)?);
    if let Some(slot) = ctx.s("worktree_slot").filter(|s| !s.is_empty()) {
        let name = slot.rsplit('/').next().unwrap_or(slot);
        let base = ctx.s("worktree_base").map(|b| format!(", detached at {b}")).unwrap_or_default();
        lines.push(format!(
            "The board gave the task the worktree {name} from its pool: {slot}{base}. Work there and make the task's branch \
             in it with git switch -c <branch>; don't make another worktree. Its builds go in {slot}/target, kept for \
             the next task that takes it."
        ));
    }
    if let Some(made) = ctx.s("worktree_made").filter(|m| !m.is_empty()) {
        if ctx.get("worktree_removed").is_none() {
            let base = ctx.s("worktree_base").map(|b| format!(", detached at {b}")).unwrap_or_default();
            lines.push(format!(
                "The board made this worktree for the task: {made}{base}. Work there and make the task's branch in it with \
                 git switch -c <branch>; don't make another worktree. The board removes it once the task is done and its \
                 PR is merged or closed, unless it has uncommitted changes."
            ));
        }
    }
    let held = crate::locks::names(t);
    if !held.is_empty() {
        lines.push(format!("This task holds {}: no other task that names the same lock runs while it does.", held.join(", ")));
    }
    match crate::worklane::alone_scope(app, t)?.as_deref() {
        Some("board") => lines.push("This task runs alone on the board: no other task starts until it's done.".into()),
        Some(_) => lines.push("This task runs alone in its goal: no other task in the goal starts until it's done.".into()),
        None => {}
    }
    Ok(lines)
}

/// A task title as a branch slug: lowercase words joined by dashes, at most 40 characters.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for w in title.to_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()) {
        if !out.is_empty() && out.len() + 1 + w.len() > 40 {
            break;
        }
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(&w.chars().take(40).collect::<String>());
    }
    out
}

/// The branch the task's PR goes on, from `[handoff] branch` ({type} {key} {slug} {task} {n}).
/// None when no template is set.
pub fn branch_name(app: &App, t: &Row) -> Result<Option<String>> {
    let tpl = app.cfg.handoff.branch.trim();
    if tpl.is_empty() {
        return Ok(None);
    }
    let bug = board::find_issue(app, t.i("from_issue_id"))?.is_some_and(|b| b.s("kind") == Some("bug"));
    Ok(Some(fill_branch(tpl, if bug { "fix" } else { "feat" }, t.s("jira_key").unwrap_or(""), &t.st("title"), t.id())))
}

/// Fills a branch template. With no ticket, {key} goes along with the separator after it.
pub fn fill_branch(tpl: &str, kind: &str, key: &str, title: &str, id: i64) -> String {
    let mut b = tpl.to_string();
    if key.is_empty() {
        b = regex::Regex::new(r"\{key\}[-_.]?").unwrap().replace_all(&b, "").to_string();
    }
    let b = b
        .replace("{type}", kind)
        .replace("{key}", key)
        .replace("{slug}", &slug(title))
        .replace("{task}", &rf("task", id))
        .replace("{n}", &id.to_string());
    b.trim_matches(|c| c == '-' || c == '_' || c == '/').to_string()
}

/// The goal's setup text (`tb goal setup`) with {task} {n} {wave} {goal} filled for this task, {jira}
/// with its ticket key (its ref when it has none, as device commands fill it), and the devices lent to
/// it: {device} {target} for the first, {device2} {target2} for the second…
pub fn fill_setup(app: &App, text: &str, t: &Row) -> Result<String> {
    let mut out = text
        .replace("{task}", &rf("task", t.id()))
        .replace("{n}", &t.id().to_string())
        .replace("{wave}", &t.i("wave").map(|w| w.to_string()).unwrap_or_default())
        .replace("{goal}", &t.i("goal_id").map(|g| rf("goal", g)).unwrap_or_default())
        .replace("{jira}", &t.s("jira_key").map(str::trim).filter(|k| !k.is_empty()).map(str::to_string).unwrap_or_else(|| rf("task", t.id())));
    for (k, v) in crate::devices::setup_vars(app, t)? {
        out = out.replace(&format!("{{{k}}}"), &v);
    }
    Ok(out)
}

fn setup_block(app: &App, t: &Row, g: Option<&Row>) -> Result<Vec<String>> {
    let Some(text) = g.and_then(|g| g.s("setup")).filter(|s| !s.trim().is_empty()) else { return Ok(vec![]) };
    Ok(vec![format!("Set up (every task in this goal does this):\n{}", clip(fill_setup(app, text, t)?.trim(), 1500))])
}

/// The task's wave: where it sits, who still runs beside it (and the files each owns or has touched), whether the
/// goal stops after it, and what comes next.
fn wave_section(app: &App, t: &Row, g: Option<&Row>) -> Result<Vec<String>> {
    let (Some(g), Some(n)) = (g, t.i("wave")) else { return Ok(vec![]) };
    let tasks = board::goal_tasks(app, g.id())?;
    let wl = crate::waves::waves(app, g, &tasks)?;
    let Some(pos) = wl.iter().position(|w| w["wave"].as_i64() == Some(n)) else { return Ok(vec![]) };
    let w = &wl[pos];
    let name = w["name"].as_str().filter(|s| !s.is_empty()).map(|s| format!(" ({s})")).unwrap_or_default();
    let mut lines = vec![format!("This task is in wave {} of {}{name}.", pos + 1, wl.len())];
    let own = str_list(board::task_context(t).get("plan_files"));
    if !own.is_empty() {
        lines.push(format!("The wave plans these files for this task: {}.", own.iter().take(10).cloned().collect::<Vec<_>>().join(", ")));
    }
    // Every mate that isn't done (planned ones too: before the goal runs, they all are); a done one
    // has nothing left to collide with.
    let mates: Vec<&Row> = tasks.iter().filter(|m| m.i("wave") == Some(n) && m.id() != t.id() && m.s("status") != Some("done")).collect();
    if mates.is_empty() {
        lines.push("Nothing else runs in this wave.".into());
    } else {
        lines.push("These run side by side with it, so keep to this task's files and leave theirs alone:".into());
        for m in mates.iter().take(8) {
            // The files the wave planned for the mate, else the ones it has touched so far.
            let ctx = board::task_context(m);
            let plan: Vec<String> = str_list(ctx.get("plan_files")).into_iter().take(5).collect();
            let owns = if !plan.is_empty() {
                format!(", owns {}", plan.join(", "))
            } else {
                let files: Vec<String> = str_list(ctx.get("files")).into_iter().rev().take(5).collect();
                if files.is_empty() { String::new() } else { format!(", has touched {}", files.join(", ")) }
            };
            lines.push(format!("- {} “{}” ({}{owns})", rf("task", m.id()), short(&m.st("title"), 60), m.st("status")));
        }
    }
    if w["stop_after"] == true {
        lines.push(format!("The goal stops after this wave for {}'s review.", app.cfg.owner));
    }
    match wl.get(pos + 1) {
        Some(next) => {
            let refs: Vec<String> = next["tasks"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
            lines.push(format!("Next is {}: {}.", crate::waves::wave_label(next), refs.join(", ")));
        }
        None => lines.push("This is the goal's last wave.".into()),
    }
    Ok(vec![lines.join("\n")])
}

/// `[handoff] footer` and `footer_file`, joined.
fn footer(app: &App) -> Option<String> {
    let h = &app.cfg.handoff;
    let mut parts = vec![];
    if !h.footer.trim().is_empty() {
        parts.push(h.footer.trim().to_string());
    }
    if !h.footer_file.trim().is_empty() {
        if let Ok(text) = std::fs::read_to_string(expand_home(h.footer_file.trim())) {
            if !text.trim().is_empty() {
                parts.push(text.trim().to_string());
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(clip(&parts.join("\n\n"), 1500))
    }
}

/// Joins the parts and the tail, cutting parts from the last one back (each down to 80 characters) until
/// it fits. The first part and the `kept` ones (by index: the device lines, whose start and stop commands
/// are no use cut short) are never cut; when even that doesn't fit, they're all that's left.
fn fit_to_limit(mut parts: Vec<String>, tail: Vec<String>, kept: &[usize]) -> String {
    let all = |p: &[String]| p.iter().chain(tail.iter()).cloned().collect::<Vec<_>>().join("\n\n");
    let text = all(&parts);
    let len = text.chars().count();
    if len <= LIMIT {
        return text;
    }
    let mut over = len as i64 - LIMIT as i64;
    for i in (1..parts.len()).rev() {
        if over <= 0 {
            break;
        }
        if kept.contains(&i) {
            continue;
        }
        let n = parts[i].chars().count() as i64;
        let cut = (n - over - 1).max(80) as usize;
        if (cut as i64) < n {
            parts[i] = clip(&parts[i], cut);
            over -= n - parts[i].chars().count() as i64;
        }
    }
    let text = all(&parts);
    if text.chars().count() > LIMIT {
        let first: Vec<String> = parts.iter().enumerate().filter(|(i, _)| *i == 0 || kept.contains(i)).map(|(_, p)| p.clone()).collect();
        return all(&first);
    }
    text
}

pub fn build(app: &App, task_id: i64) -> Result<String> {
    build_with(app, task_id, false)
}

/// The handoff a task starts with: it has started, though `started_at` is set only once a
/// terminal takes it, so the devices line says none were free rather than that they come later.
pub fn build_starting(app: &App, task_id: i64) -> Result<String> {
    build_with(app, task_id, true)
}

fn build_with(app: &App, task_id: i64, starting: bool) -> Result<String> {
    let t = board::get_task(app, task_id)?;
    let ctx = board::task_context(&t);
    let g = board::find_goal(app, t.i("goal_id"))?;
    let tb = board::tb_cmd(app);
    let w = ctx.get("where").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    let mut parts = vec![first_line(&t)];
    parts.extend(goal_and_notes(app, g.as_ref())?);
    parts.extend(crate::lessons::handoff_block(app, &t)?);
    parts.extend(also_goals(app, &t)?);
    parts.extend(wave_section(app, &t, g.as_ref())?);
    parts.extend(setup_block(app, &t, g.as_ref())?);
    // The devices lent to it, early and never cut, so a long handoff keeps how to start and stop them.
    let mut kept = vec![];
    for line in crate::devices::handoff_lines(app, &t, starting)? {
        kept.push(parts.len());
        parts.push(line);
    }
    parts.extend(branch_and_last_commit(&w));
    parts.extend(checkpoint_and_answers(app, &t, &ctx));
    parts.extend(attachments_block(app, &t, g.as_ref())?);
    parts.extend(backlog_origin(app, &t)?);
    parts.extend(goal_open_issues(app, &t, g.as_ref())?);
    if let Some(d) = t.s("detail").filter(|d| !d.trim().is_empty()) {
        parts.push(format!("What to do:\n{}", d.trim()));
    }
    parts.extend(other_tasks(app, &t)?);
    parts.extend(worktree_and_lock_lines(app, &t, &ctx)?);
    parts.extend(crate::comments::rule_line(&app.cfg));
    parts.extend(crate::bits::handoff_lines(app, &t)?);
    let mut tail = vec![pr_block(app, &t, &tb)?];
    tail.extend(crate::jira::handoff_block(app, &t)?);
    tail.push(report_block(app, &tb));
    tail.extend(footer(app));
    tail.push(CLOSING.to_string());
    Ok(fit_to_limit(parts, tail, &kept))
}

/// What a fresh conversation about a done task's PR needs to know.
pub fn pr_context(app: &App, t: &Row) -> String {
    let ctx = board::task_context(t);
    let w = ctx.get("where").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    let mut first = format!(
        "This is a fresh conversation for “{}” in {}",
        t.st("title"),
        t.s("project").filter(|p| !p.is_empty()).unwrap_or("this project")
    );
    if let Some(k) = t.s("jira_key").filter(|k| !k.is_empty()) {
        first += &format!(" ({k})");
    }
    first += ". The task is done and its PR is open; here is where it stood.";
    if let Some(wt) = w.s("worktree").filter(|s| !s.is_empty()) {
        first += &format!(" Worktree: {wt}.");
    }
    let mut parts = vec![first];
    parts.extend(branch_and_last_commit(&w));
    parts.extend(checkpoint_and_answers(app, t, &ctx));
    parts.join("\n\n")
}

pub fn no_task_line(app: &App, project: Option<&str>) -> Result<String> {
    let tb = board::tb_cmd(app);
    let project = project.filter(|p| !p.is_empty());
    let q = match project {
        Some(p) => app.db.q1(
            "SELECT id FROM tasks WHERE status = 'queued' AND project = ? AND session_id IS NULL AND line_session IS NULL \
             ORDER BY priority = 'high' DESC, created_at, id LIMIT 1",
            p![p],
        )?,
        None => None,
    };
    let n = q.map(|q| rf("task", q.id())).unwrap_or_else(|| "T<n>".into());
    let owner = &app.cfg.owner;
    let line = format!(
        "Task board: no task on this terminal. To take one, run {tb} take {n}, or ask {owner}. \
         If you change code for {owner} here, put it on the board as you start: {tb} task new \"<title>\" --detail \"…\" --here."
    );
    // A task whose terminal closed mid-work comes first, so follow-up work here attaches to it.
    let lost = match project {
        Some(p) => app.db.q(
            "SELECT id, title FROM tasks WHERE status = 'needs' AND lost = 1 AND project = ? ORDER BY updated_at DESC, id DESC",
            p![p],
        )?,
        None => vec![],
    };
    let lost: Vec<String> = lost
        .iter()
        .map(|t| {
            let r = rf("task", t.id());
            format!("{r} “{}” lost its terminal — {tb} take {r} if you are carrying it on.", t.st("title"))
        })
        .collect();
    if lost.is_empty() {
        return Ok(line);
    }
    Ok(format!("Task board: {}\n{line}", lost.join("\n")))
}

pub fn is_full_handoff(prompt: &str, task_id: i64) -> bool {
    prompt.contains(&marker(task_id)) && prompt.contains(CLOSING)
}
