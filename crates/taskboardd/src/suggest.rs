//! Suggested fixes for alerts raised as free text (`tb alert raise`, an agent's question): the board
//! knows no fixes for them, so a headless `claude -p` (Haiku, on the check-in's model and budget) reads
//! the alert and gives back a short title, one detail line and one to three actions, each a button label
//! and the request handed to a Claude terminal (`dispatch::act`) for the owner to read and send. Haiku's
//! actions carry no command: nothing it writes is ever run directly. It runs on its own thread, off the
//! hot path; when Haiku fails, times out or answers out of shape, the alert stays plain text with Dismiss.
//! `[alerts] suggest = false` turns it off.

use serde_json::{json, Value};

use crate::app::App;
use crate::dispatch::{self, AlertAction};
use crate::util::*;
use crate::{board, checkin};

/// How often the loop looks for alerts waiting on suggestions.
const TICK_SECS: f64 = 5.0;
/// At most this many actions per alert.
const MAX_ACTIONS: usize = 3;

pub fn run(app: std::sync::Arc<App>) {
    while !app.stopping() {
        if app.cfg.alerts.suggest {
            tick(&app);
        }
        app.sleep(TICK_SECS);
    }
}

/// Asks about every alert marked `suggest: pending`, one at a time.
pub fn tick(app: &App) {
    let pending: Vec<Value> = dispatch::alerts(app).into_iter().filter(|a| a["suggest"] == "pending").collect();
    for a in pending {
        if app.stopping() {
            break;
        }
        let id = a["id"].as_str().unwrap_or("").to_string();
        let fields = suggest(app, &a);
        if fields.is_none() {
            app.info(format!("suggest: no suggestions for alert {id}"));
        }
        if let Err(e) = app.db.tx(|| dispatch::set_suggestions(app, &id, fields.clone())) {
            app.info(format!("suggest: {e}"));
        }
    }
}

fn schema() -> Value {
    json!({"type": "object", "required": ["title", "detail", "actions"], "properties": {
        "title": {"type": "string"}, "detail": {"type": "string"},
        "actions": {"type": "array", "minItems": 1, "maxItems": MAX_ACTIONS, "items": {"type": "object", "required": ["label", "prompt"],
            "properties": {"label": {"type": "string"}, "prompt": {"type": "string"}}}}}})
}

/// What the alert's card gets from Haiku: `{title, detail, actions, project}`, or None.
fn suggest(app: &App, a: &Value) -> Option<Value> {
    let task = a["task_id"].as_i64().and_then(|t| board::find_task(app, Some(t)).ok().flatten());
    let goal = a["goal"].as_str().and_then(|g| g.trim_start_matches('G').parse::<i64>().ok()).and_then(|g| board::get_goal(app, g).ok());
    let project = task.as_ref().and_then(|t| t.s("project")).or_else(|| goal.as_ref().and_then(|g| g.s("project"))).unwrap_or("").to_string();
    let mut about = vec![];
    if let Some(t) = &task {
        about.push(format!("It is about task {} “{}”.", rf("task", t.id()), t.s("title").unwrap_or("")));
    }
    if let Some(g) = &goal {
        about.push(format!("It is part of goal {} “{}”.", rf("goal", g.id()), g.s("name").unwrap_or("")));
    }
    if !project.is_empty() {
        about.push(format!("Project: {project}."));
    }
    let v = checkin::ask_haiku(app, prompt(&app.cfg.owner, a["text"].as_str().unwrap_or(""), &about.join(" ")), &schema())?;
    let mut fields = parse(&v)?;
    fields["project"] = json!(project);
    Some(fields)
}

fn prompt(owner: &str, text: &str, about: &str) -> String {
    format!(
        "An alert came up on {owner}'s task board, where AI coding agents work on tasks. It shows as a card in the board's app.\n\n\
         Write the card:\n\
         - title: what's wrong or what's asked, in at most 8 words.\n\
         - detail: one plain sentence, at most 120 characters, with what {owner} needs to know.\n\
         - actions: one to three things {owner} would most likely want done about it. Each has a label for its button \
           (2 to 5 words, starting with a verb) and a prompt: the request, written as {owner} in the first person, that is \
           typed into a new Claude Code terminal in the project's folder for {owner} to read and send. Make each prompt \
           stand on its own: name the task and say what to do. That Claude uses the board's own tools to make board changes. \
           For a question, offer the likely answers as actions. Don't write shell commands for {owner} to run.\n\n\
         {about}\n\nThe alert:\n{text}"
    )
}

/// Haiku's answer as the card's fields, the actions in `AlertAction`'s shape with no command; None when
/// it has no title or no usable action.
fn parse(v: &Value) -> Option<Value> {
    let title = one_line(v["title"].as_str()?, 80);
    let detail = one_line(v["detail"].as_str().unwrap_or(""), 200);
    let actions: Vec<Value> = v["actions"]
        .as_array()?
        .iter()
        .filter_map(|x| {
            let (label, prompt) = (one_line(x["label"].as_str()?, 40), clip(x["prompt"].as_str()?.trim(), 1000));
            (!label.is_empty() && !prompt.is_empty()).then(|| AlertAction { label, prompt, command: String::new() }.json())
        })
        .take(MAX_ACTIONS)
        .collect();
    (!title.is_empty() && !actions.is_empty()).then(|| json!({"title": title, "detail": detail, "actions": actions}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_good_answer_becomes_the_cards_fields_with_no_commands() {
        let v = json!({"title": "T12 asks which port", "detail": "The dev server needs a port.", "actions": [
            {"label": "Use port 8080", "prompt": "Answer T12: use port 8080.", "command": "tb answer T12 8080"},
            {"label": "", "prompt": "x"}, {"label": "Use 3000", "prompt": "Answer T12: use 3000."},
            {"label": "a", "prompt": "b"}, {"label": "c", "prompt": "d"}]});
        let f = parse(&v).unwrap();
        assert_eq!(f["title"], "T12 asks which port");
        let actions = f["actions"].as_array().unwrap();
        assert_eq!(actions.len(), 3);
        assert_eq!(actions[0], json!({"label": "Use port 8080", "prompt": "Answer T12: use port 8080.", "command": ""}));
        assert_eq!(actions[1]["label"], "Use 3000");
    }

    #[test]
    fn an_answer_out_of_shape_gives_nothing() {
        assert!(parse(&json!({"title": "x", "actions": []})).is_none());
        assert!(parse(&json!({"title": "", "actions": [{"label": "a", "prompt": "b"}]})).is_none());
        assert!(parse(&json!({"actions": [{"label": "a", "prompt": "b"}]})).is_none());
        assert!(parse(&json!({"title": "x", "actions": [{"label": "a"}]})).is_none());
        assert!(parse(&json!("not an object")).is_none());
    }
}
