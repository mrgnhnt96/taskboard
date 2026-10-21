//! Talking an idea over: the app's Talk it over button opens a Claude terminal in the project's
//! checkout with the idea, its notes and its dig reports in its context, for the owner to think it
//! through with. Claude waits for them and keeps what they settle as notes on the idea (`tb idea note`).
//! Asked about one of a report's open questions, the question is typed in for the owner to send.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, dig, ideas, midna, p};

/// The context goes on Claude's command line; a long history of reports is cut short.
const CONTEXT_MAX: usize = 60_000;

/// `POST /ideas/:ref/talk {question?}`: a terminal to talk the idea over in, about one of its reports'
/// open questions when `question` names one. Refused for an idea that became a goal, while Midna is
/// down, or with no checkout of its project.
pub fn start(app: &App, r: &str, body: &Value) -> Result<Value> {
    let i = ideas::get(app, r)?;
    let me = rf("idea", i.id());
    if i.st("state") == "planned" {
        let g = rf_opt("goal", i.i("goal_id"));
        return err(409, format!("{me} became {} already; work on the goal instead.", g.as_str().unwrap_or("a goal")));
    }
    let asked = body_str(body, "question");
    let draft = if asked.trim().is_empty() {
        String::new()
    } else {
        let Some(choices) = question(app, i.id(), asked.trim())? else {
            return err(404, format!("{me}'s reports don't ask “{}”.", one_line(&asked, 80)));
        };
        let choices = if choices.is_empty() { String::new() } else { format!(" ({})", choices.join(" / ")) };
        format!("Help me decide this one: {}{choices}", asked.trim())
    };
    if !midna::up(app) {
        return err(409, "Midna isn't running, so the board can't open a terminal.");
    }
    let project = i.st("project");
    let Some(cwd) = dig::checkout(app, &project)? else {
        return err(409, format!("{me}'s project {project} has no checkout on this Mac to talk it over in."));
    };
    let args = json!({"cwd": cwd, "title": short(&format!("Talk: {}", i.st("title")), 40), "prompt": "", "draft": draft,
                      "context": context(app, &i)?, "focus": true, "queue": false});
    let j = app.db.tx(|| board::create_job(app, "agent", args, None, "idea_talk", Some(json!({"idea": i.id()}))))?;
    Ok(json!({"ok": true, "job": rf("job", j)}))
}

/// The choices of the question `text` in one of the idea's reports; None when none asks it.
fn question(app: &App, id: i64, text: &str) -> Result<Option<Vec<String>>> {
    let strs = |v: &Value| v.as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    for r in app.db.q("SELECT report FROM idea_rounds WHERE idea_id = ? AND report IS NOT NULL", p![id])? {
        let rep: Value = r.s("report").and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
        let blocks = rep["blocks"].as_array().cloned().unwrap_or_default();
        if let Some(q) = blocks.iter().find(|b| b["type"] == "question" && b["text"].as_str().map(str::trim) == Some(text)) {
            return Ok(Some(strs(&q["choices"])));
        }
        // A report from before reports were blocks lists its questions as plain lines.
        if strs(&rep["open_questions"]).iter().any(|q: &String| q.trim() == text) {
            return Ok(Some(vec![]));
        }
    }
    Ok(None)
}

fn context(app: &App, i: &Row) -> Result<String> {
    let owner = &app.cfg.owner;
    let me = rf("idea", i.id());
    let mut parts = vec![format!(
        "{owner} opened this terminal from the idea page on the task board to talk over their idea {me} for {}. You're in \
         the project's checkout; read the code as much as the talk needs, but don't change anything. Wait for {owner}; \
         don't start on anything by yourself. When the talk settles something (a decision, an answer to an open \
         question, a new angle, a reason to drop it), keep it on the idea: tb idea note {me} \"<what was settled>\", one \
         short note each. You aren't on a task: don't take one, and don't plan or dig the idea unless {owner} asks.",
        i.st("project")
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
        let lines: Vec<String> = rounds.iter().map(crate::idea_plan::report_text).collect();
        parts.push(format!(
            "What digging into it against the code found, oldest first (the code may have moved since a round's commit):\n\n{}",
            lines.join("\n\n")
        ));
    }
    parts.push(format!("For all of it as it stands now: tb idea show {me}"));
    Ok(clip(&parts.join("\n\n"), CONTEXT_MAX))
}
