//! Planning an idea into a goal. Only when asked (`tb idea plan I4`, the app's Plan button), a headless
//! `claude -p` reads the project's checkout with Claude's read-only tools, with the idea, its notes and
//! its dig reports in the prompt, and drafts one goal (name, TLDR, outcome) and its tasks by the board's
//! planning rules (`tb planning`). The board makes them: the goal and its tasks are planned, never
//! started or queued. The idea is then planned, linked to its goal, and the goal keeps a pinned note
//! of the idea it came from.
//!
//! One idea is planned once at a time, never while it's being dug into. Nothing about a plan running is
//! kept in the database, so a plan the board lost (it stopped while Claude read) leaves the idea as it was.

use std::path::Path;

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, dig, fields, ideas, ops, p, planning, triage};

/// Planning reads code like a dig does, so it gets as long.
const TIMEOUT_SECS: u64 = 900;
const NAME_MAX: usize = 80;
const TASKS_MAX: usize = 12;
const TITLE_MAX: usize = 300;
const DETAIL_MAX: usize = 4_000;
const WHO: &str = "Claude";

/// `POST /ideas/:ref/plan`: start planning the idea. It runs on its own; the idea shows `planning` now.
pub fn start(app: &App, r: &str, body: &Value) -> Result<Value> {
    let i = ideas::get(app, r)?;
    let me = rf("idea", i.id());
    plannable(&i)?;
    if crate::proc::which(&app.cfg.claude).is_none() {
        return err(409, "claude isn't installed, so there's nothing to plan with.");
    }
    let project = i.st("project");
    let Some(repo) = dig::checkout(app, &project)? else {
        return err(409, format!("{me}'s project {project} has no checkout on this Mac to read."));
    };
    let prompt = prompt(app, &i, &repo)?;
    {
        let mut s = app.shared.lock();
        if s.digging.contains(&i.id()) {
            return err(409, format!("{me} is being dug into; wait for its report before planning it."));
        }
        if !s.idea_planning.insert(i.id()) {
            return err(409, format!("{me} is being planned already."));
        }
    }
    // Dug into, shelved or planned while the prompt was put together: leave it.
    if let Err(e) = ideas::get(app, r).and_then(|i| plannable(&i)) {
        app.shared.lock().idea_planning.remove(&i.id());
        return Err(e);
    }
    let id = i.id();
    let by = board::caller(app, body, board::OWNER);
    app.queue_slow(Box::new(move |a: &App| {
        let answer = triage::ask_in_with(a, prompt, schema(a.cfg.models.enabled), Some(&repo), &[], TIMEOUT_SECS)
            .and_then(|v| checked(&v, a.cfg.models.enabled));
        if let Err(e) = finish(a, id, &project, &by, answer) {
            a.info(format!("ideas: recording {}'s plan failed: {}", rf("idea", id), e.message));
        }
        a.shared.lock().idea_planning.remove(&id);
    }));
    ideas::show(app, &ideas::get(app, r)?)
}

/// 409 unless the idea is open and not being dug into.
fn plannable(i: &Row) -> Result<()> {
    let me = rf("idea", i.id());
    match i.st("state").as_str() {
        "planned" => {
            let g = rf_opt("goal", i.i("goal_id"));
            err(409, format!("{me} became {} already; work on the goal instead.", g.as_str().unwrap_or("a goal")))
        }
        "shelved" | "dropped" => err(409, format!("{me} is {}; reopen it first (tb idea reopen {me}).", i.st("state"))),
        "digging" => err(409, format!("{me} is being dug into; wait for its report before planning it.")),
        _ => Ok(()),
    }
}

/// Makes Claude's plan, or keeps why there's none as a note on the idea.
fn finish(app: &App, id: i64, project: &str, by: &str, answer: std::result::Result<Value, String>) -> Result<()> {
    let me = rf("idea", id);
    let made = answer.map_err(|e| ApiError::new(500, e)).and_then(|plan| app.db.tx(|| make(app, id, project, by, &plan)));
    if let Err(e) = made {
        app.info(format!("ideas: planning {me} failed: {}", e.message));
        app.db.tx(|| {
            let now = now_iso();
            let text = format!("Claude couldn't plan this: {}", one_line(&e.message, 1_000));
            app.db.insert("idea_notes", fields!["idea_id" => id, "text" => text, "source" => board::BOARD, "at" => now])?;
            app.db.update("ideas", &json!(id), fields!["updated_at" => now])
        })?;
    }
    Ok(())
}

/// The goal and its planned tasks, the goal's note of the idea, and the idea planned with its goal.
fn make(app: &App, id: i64, project: &str, by: &str, plan: &Value) -> Result<i64> {
    let me = rf("idea", id);
    let i = ideas::get(app, &me)?;
    plannable(&i)?;
    if i.st("project") != project {
        return err(409, format!("{me} moved to {} while Claude planned it in {project}.", i.st("project")));
    }
    let g = ops::new_goal(app, &json!({"name": plan["name"], "tldr": plan["tldr"], "outcome": plan["outcome"], "project": project}))?;
    let gid = g["id"].as_i64().unwrap_or(0);
    let log = format!("Planned from {me} by {WHO}");
    let mut made: Vec<String> = vec![];
    for t in plan["tasks"].as_array().cloned().unwrap_or_default() {
        let nth = |n: &Value| n.as_u64().and_then(|n| made.get((n as usize).checked_sub(1)?)).cloned();
        let waits: Vec<String> = t["after"].as_array().map(|a| a.iter().filter_map(nth).collect()).unwrap_or_default();
        let stack_on = nth(&t["stack_on"]);
        let c = ops::new_task(
            app,
            &json!({"title": t["title"], "detail": t["detail"], "project": project, "goal_id": gid,
                    "status": "planned", "pickup": {"mode": "queue"}, "waits_for": waits,
                    "stack_on": stack_on, "stack_merge": if stack_on.is_some() { t["stack_merge"].clone() } else { Value::Null },
                    "model": t["model"], "not_backlog": true, "origin": {"from": format!("Planned from {me}"), "by": WHO}}),
            WHO,
            Some(&log),
        )?;
        made.push(c["ref"].as_str().unwrap_or("").to_string());
    }
    let reports = app.db.count("SELECT COUNT(*) FROM idea_rounds WHERE idea_id = ? AND report IS NOT NULL", p![id])?;
    let mut note = format!("Planned from idea {me} “{}”, asked by {by}", i.st("title"));
    note += &match reports {
        0 => ", with no dig report.".to_string(),
        1 => ", with its dig report.".to_string(),
        n => format!(", with its {n} dig reports."),
    };
    let detail = i.st("detail");
    if !detail.trim().is_empty() {
        note += &format!("\n{}", clip(detail.trim(), 1_000));
    }
    note += &format!("\nThe idea, its notes and reports: tb idea show {me}");
    board::add_goal_note(app, gid, "decision", &note, Some(WHO), true, None)?;
    app.db.update("ideas", &json!(id), fields!["state" => "planned", "goal_id" => gid, "updated_at" => now_iso()])?;
    Ok(gid)
}

// ------------------------------------------------------------------ asking Claude

fn schema(models: bool) -> Value {
    let mut task = json!({"type": "object", "required": ["title", "detail", "after", "stack_on", "stack_merge"],
        "properties": {
            "title": {"type": "string"},
            "detail": {"type": "string"},
            "after": {"type": "array", "items": {"type": "integer"}},
            "stack_on": {"type": "integer"},
            "stack_merge": {"enum": ["tail", "head"]}}});
    if models {
        task["required"].as_array_mut().unwrap().push(json!("model"));
        task["properties"]["model"] = json!({"enum": ["small", "large"]});
    }
    json!({"type": "object", "required": ["name", "tldr", "outcome", "tasks"],
        "properties": {
            "name": {"type": "string"},
            "tldr": {"type": "string"},
            "outcome": {"type": "string"},
            "tasks": {"type": "array", "items": task}}})
}

fn prompt(app: &App, i: &Row, repo: &Path) -> Result<String> {
    let owner = &app.cfg.owner;
    let project = i.st("project");
    let me = rf("idea", i.id());
    let head = dig::git(repo, &["rev-parse", "HEAD"]);
    let at = head.as_deref().map(|s| format!(" at commit {}", short(s))).unwrap_or_default();
    let model = if app.cfg.models.enabled { "\n- model: small or large, by the rules' model rule." } else { "" };
    let mut parts = vec![format!(
        "{owner} has an idea for {project} that's ready to plan. Plan it into one goal and its tasks. You're in the \
         project's checkout{at} and may read it (Read, Grep and Glob); you can't change anything. Read the code the \
         work touches before slicing it.\n\n\
         Answer with:\n\
         - name: the goal's name, 2 to 6 plain words.\n\
         - tldr: what the goal gets {owner}, in two or three plain sentences.\n\
         - outcome: when the goal is done, as something {owner} can check.\n\
         - tasks: in the order they should run. Each has a title (one line), and a detail saying what to build, \
         where in the code, and what review will check (migrations, API shape, how it behaves when it's switched \
         off, edge cases), so the first PR passes.\n\
         - after: the numbers of earlier tasks (1 is the first) whose work it needs; empty when none.\n\
         - stack_on: the number of an earlier task whose unmerged code it builds on, or 0 for none.\n\
         - stack_merge: tail or head, for a stacked task (tail when it isn't stacked).{model}\n\
         Where the rules below name a tb flag (--stack-merge, --model), answer with the matching field. \
         At most {TASKS_MAX} tasks."
    )];
    let mut idea = format!("The idea, {me}: {}", i.st("title"));
    let detail = i.st("detail");
    if !detail.trim().is_empty() {
        idea.push_str(&format!("\n{}", detail.trim()));
    }
    let notes = app.db.q("SELECT text FROM idea_notes WHERE idea_id = ? ORDER BY id", p![i.id()])?;
    if !notes.is_empty() {
        idea.push_str(&format!("\nNotes:\n{}", notes.iter().map(|n| format!("- {}", n.st("text").trim())).collect::<Vec<_>>().join("\n")));
    }
    parts.push(idea);
    let rounds = app.db.q("SELECT * FROM idea_rounds WHERE idea_id = ? AND report IS NOT NULL ORDER BY id", p![i.id()])?;
    if !rounds.is_empty() {
        let lines: Vec<String> = rounds.iter().map(report_text).collect();
        parts.push(format!(
            "What digging into it against the code found, oldest first (the code may have moved since a round's commit):\n\n{}",
            lines.join("\n\n")
        ));
    }
    let about = format!("{}\n{}", i.st("title"), detail);
    parts.push(planning::block(app, Some(&project), &about));
    Ok(parts.join("\n\n"))
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// A dig round's whole report, for the prompt.
pub(crate) fn report_text(r: &Row) -> String {
    let rep: Value = r.s("report").and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
    let focus = r.s("focus").filter(|f| !f.is_empty()).unwrap_or("the whole idea");
    let at = r.s("sha").map(|s| format!(", at commit {}", short(s))).unwrap_or_default();
    let mut out = vec![format!(
        "Round on {focus}{at}: {} ({})",
        rep["verdict"].as_str().unwrap_or("?"),
        rep["why"].as_str().unwrap_or("").trim()
    )];
    for (head, items) in crate::dig::report_sections(&rep) {
        out.push(format!("{head}:\n{}", items.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n")));
    }
    out.join("\n")
}

/// Claude's plan made safe to make: a name (the first line, clipped), an outcome, one to `TASKS_MAX`
/// tasks with titles, and `after`/`stack_on` naming only earlier tasks.
pub fn checked(v: &Value, models: bool) -> std::result::Result<Value, String> {
    let name = clip(&one_line(v["name"].as_str().unwrap_or(""), NAME_MAX), NAME_MAX);
    if name.is_empty() {
        return Err("claude's plan had no goal name".into());
    }
    let outcome = clip(v["outcome"].as_str().unwrap_or("").trim(), 2_000);
    if outcome.is_empty() {
        return Err("claude's plan had no outcome".into());
    }
    let mut tasks: Vec<Value> = vec![];
    for t in v["tasks"].as_array().cloned().unwrap_or_default() {
        if tasks.len() == TASKS_MAX {
            break;
        }
        let title = one_line(t["title"].as_str().unwrap_or(""), TITLE_MAX);
        if title.is_empty() {
            continue;
        }
        // Numbered from 1, as the prompt says; this task would be `n`.
        let n = tasks.len() as u64 + 1;
        let earlier = |x: &Value| x.as_u64().filter(|k| (1..n).contains(k));
        let mut after: Vec<u64> = vec![];
        for k in t["after"].as_array().cloned().unwrap_or_default().iter().filter_map(earlier) {
            if !after.contains(&k) {
                after.push(k);
            }
        }
        let stack_on = earlier(&t["stack_on"]);
        let merge = stack_on.map(|_| if t["stack_merge"] == "head" { "head" } else { "tail" });
        let model = t["model"].as_str().filter(|m| models && ["small", "large"].contains(m));
        tasks.push(json!({"title": title, "detail": clip(t["detail"].as_str().unwrap_or("").trim(), DETAIL_MAX),
                          "after": after, "stack_on": stack_on, "stack_merge": merge, "model": model}));
    }
    if tasks.is_empty() {
        return Err("claude's plan had no tasks".into());
    }
    Ok(json!({"name": name, "tldr": clip(v["tldr"].as_str().unwrap_or("").trim(), 2_000), "outcome": outcome, "tasks": tasks}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_are_checked_before_they_are_made() {
        let v = json!({"name": " Better\ninbox ", "tldr": " Fewer pings. ", "outcome": "Done when the inbox groups pings.",
            "tasks": [
                {"title": "Schema", "detail": "Add a table.", "after": [1, 2], "stack_on": 0, "stack_merge": "head", "model": "small"},
                {"title": " ", "detail": "", "after": [], "stack_on": 0, "stack_merge": "tail"},
                {"title": "Routes", "detail": "", "after": [1, 1, 3, -1], "stack_on": 1, "stack_merge": "head", "model": "huge"},
                {"title": "App", "detail": "", "after": [], "stack_on": 3, "stack_merge": "tail"}]});
        let p = checked(&v, true).unwrap();
        assert_eq!(p["name"], "Better inbox");
        assert_eq!(p["tldr"], "Fewer pings.");
        let t = p["tasks"].as_array().unwrap();
        assert_eq!(t.len(), 3, "a task with no title is left out");
        assert_eq!(t[0], json!({"title": "Schema", "detail": "Add a table.", "after": [], "stack_on": null, "stack_merge": null, "model": "small"}),
                   "the first task can't wait for or stack on anything; a merge goes with a stack");
        assert_eq!(t[1]["after"], json!([1]), "only earlier tasks, once each");
        assert_eq!((&t[1]["stack_on"], &t[1]["stack_merge"], &t[1]["model"]), (&json!(1), &json!("head"), &Value::Null));
        assert_eq!((&t[2]["stack_on"], &t[2]["stack_merge"]), (&Value::Null, &Value::Null), "it can't stack on a later task");
        assert_eq!(checked(&v, false).unwrap()["tasks"][0]["model"], Value::Null, "no model while the board doesn't pick them");
        assert!(checked(&json!({"name": "x", "outcome": "y", "tasks": []}), false).is_err());
        assert!(checked(&json!({"name": "x", "outcome": " ", "tasks": [{"title": "a"}]}), false).is_err());
        assert!(checked(&json!({"name": "", "outcome": "y", "tasks": [{"title": "a"}]}), false).is_err());
        let many: Vec<Value> = (0..20).map(|k| json!({"title": format!("t{k}")})).collect();
        assert_eq!(checked(&json!({"name": "x", "outcome": "y", "tasks": many}), false).unwrap()["tasks"].as_array().unwrap().len(), TASKS_MAX);
    }
}
