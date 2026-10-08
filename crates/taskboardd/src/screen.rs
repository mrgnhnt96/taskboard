//! Screening an agent's question: when the owner's own rules already answer it, the agent gets the
//! answer and the owner isn't asked. A headless `claude -p` reads the question with the rules.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, handoff, proc};

const SKILL_QUESTIONS: &str = include_str!("../../../plugin/task-board/skills/task-board/questions.md");

fn verdict_schema() -> Value {
    json!({"type": "object", "required": ["verdict"], "properties": {
        "verdict": {"enum": ["answered", "ask"]}, "answer": {"type": "string"},
        "source": {"type": "string"}, "question": {"type": "string"}}})
}

fn rules(app: &App) -> String {
    let mut out = vec![format!("questions.md:\n{SKILL_QUESTIONS}")];
    for r in &app.cfg.questions.rules {
        let p = expand_home(r);
        if let Ok(text) = std::fs::read_to_string(&p) {
            out.push(format!("{}:\n{}", p.display(), clip(&text, 12000)));
        }
    }
    out.join("\n\n")
}

fn prompt(app: &App, t: Option<&Row>, question: &str) -> Result<String> {
    let owner = &app.cfg.owner;
    let mut parts = vec![format!(
        "An agent working on {owner}'s task board wants to ask {owner} a question. Decide whether the rules and \
         context below already settle it.\n- answered: they clearly do. Give the answer in plain words and name its \
         source (a file name or 'the task').\n- ask: they don't, or you're unsure. Rewrite the question for {owner} in \
         plain words: say what it's about, explain every reference and term, and keep it short.\nNever invent a rule."
    )];
    parts.push(format!("The question:\n{question}"));
    if let Some(t) = t {
        parts.push(format!("The task's handoff:\n{}", clip(&handoff::build(app, t.id())?, 5000)));
        let earlier: Vec<String> = app
            .db
            .q("SELECT text FROM events WHERE task_id = ? AND kind IN ('question','answer') ORDER BY id DESC LIMIT 12", crate::p![t.id()])?
            .iter()
            .map(|e| format!("- {}", one_line(&e.st("text"), 400)))
            .collect();
        if !earlier.is_empty() {
            parts.push(format!("Earlier questions and answers:\n{}", earlier.join("\n")));
        }
        if let Some(g) = board::find_goal(app, t.i("goal_id"))? {
            let notes = board::goal_notes(app, g.id())?;
            if !notes.is_empty() {
                parts.push(format!("The goal's notes:\n{}", handoff::notes_block(&notes, 2500)));
            }
        }
    }
    parts.push(format!("Rules:\n{}", rules(app)));
    Ok(parts.join("\n\n"))
}

/// Returns `{verdict, answer?, source?, question?}`, or None when screening is off or fails.
pub fn question(app: &App, t: Option<&Row>, text: &str) -> Option<Value> {
    if !app.cfg.questions.screen {
        return None;
    }
    let claude = proc::which(&app.cfg.claude)?;
    let p = prompt(app, t, text).ok()?;
    let q = &app.cfg.questions;
    let args: Vec<String> = vec![
        "-p".into(),
        p,
        "--model".into(),
        q.model.clone(),
        "--setting-sources".into(),
        "".into(),
        "--no-session-persistence".into(),
        "--tools".into(),
        "".into(),
        "--strict-mcp-config".into(),
        "--max-budget-usd".into(),
        q.budget_usd.clone(),
        "--json-schema".into(),
        verdict_schema().to_string(),
        "--output-format".into(),
        "json".into(),
    ];
    let out = match proc::run(&claude, &args, Some(&app.cfg.data), q.timeout_secs as f64) {
        Ok(o) => o,
        Err(_) => {
            app.info("screen: claude didn't answer in time; the question goes to the owner as written");
            return None;
        }
    };
    let d: Value = serde_json::from_str(out.stdout.trim()).ok()?;
    let v = d.get("structured_output").cloned().or_else(|| d.get("result").and_then(|r| r.as_str()).and_then(|r| serde_json::from_str(r).ok()))?;
    match v["verdict"].as_str() {
        Some("answered") if v["answer"].as_str().map(|a| !a.trim().is_empty()).unwrap_or(false) => Some(json!({
            "verdict": "answered", "answer": v["answer"], "source": v["source"].as_str().filter(|s| !s.is_empty()).unwrap_or("the rules")})),
        Some("ask") => {
            let q = v["question"].as_str().filter(|q| !q.trim().is_empty()).map(|q| q.to_string());
            Some(json!({"verdict": "ask", "question": q}))
        }
        _ => None,
    }
}
