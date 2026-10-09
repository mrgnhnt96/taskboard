//! Jira through a headless `claude -p` and the Atlassian connector's tools (`[jira] via = "claude"`),
//! for a board with no REST token. Each op is one call: a plain prompt, and an answer in a fixed
//! JSON shape (`ok`, `error`, and the op's own fields).

use serde_json::{json, Value};

use crate::app::App;
use crate::proc;
use crate::util::*;

fn schema() -> Value {
    let s = json!({"type": "string"});
    json!({"type": "object", "required": ["ok"], "properties": {
        "ok": {"type": "boolean"}, "error": s, "key": s, "status": s, "found": {"type": "boolean"}, "product": s,
        "author": s, "author_account_id": s, "author_email": s, "text": s,
        "comments": {"type": "array", "items": {"type": "object", "properties": {
            "key": s, "id": s, "created": s, "author": s, "author_account_id": s, "author_email": s, "text": s}}}}})
}

fn intro(app: &App) -> String {
    let j = &app.cfg.jira;
    format!(
        "You work Jira for a task board through the Atlassian connector's tools (site {}, project {}). Do exactly the one \
         thing below, then answer in the JSON asked for. Never ask anyone anything. If it can't be done, answer ok false \
         with the reason in error, in one plain sentence.",
        j.site.trim(),
        j.project.trim()
    )
}

/// The prompt for one op. `input` holds the op's own fields (`key`, `status`, `comment`, `brief`…).
pub fn prompt(app: &App, op: &str, input: &Value) -> String {
    let s = |k: &str| input[k].as_str().unwrap_or("").to_string();
    let key = s("key");
    let ask = match op {
        "status" => format!("Read the status of {key}. Answer status: its status's name."),
        "transition" => {
            let mut t = format!(
                "Move {key} to the status “{}”: pick the transition that ends there. If it's already there, leave it. If no \
                 transition gets there, answer ok false and say which statuses it can go to.",
                s("status")
            );
            if !s("comment").is_empty() {
                t += &format!(" Then add this comment to it:\n{}\n", s("comment"));
            }
            t + " Answer status: its status's name once you're done."
        }
        "comment" => format!("Add this comment to {key}, as it is:\n{}", s("comment")),
        "create" => format!("{}\n\nAnswer key: the ticket's key, status: its status's name, found: true when it was already there, product: the product you picked (if you picked one).", s("brief")),
        "comments" => format!(
            "For each of these tickets: {}, list its comments made in the last {} minutes (none is fine). Answer comments: one \
             each, with key, id (the comment's id), created (its ISO time), author (display name), author_account_id, \
             author_email (when shown) and text (the comment as plain text).",
            s("keys"),
            input["minutes"].as_i64().unwrap_or(15)
        ),
        "comment_get" => format!(
            "Read comment {} on {key}. Answer author (display name), author_account_id, author_email (when shown) and text \
             (the comment as plain text).",
            s("id")
        ),
        other => format!("Do the Jira op {other} on {key}."),
    };
    format!("{}\n\n{ask}", intro(app))
}

/// The answer's object, from `claude -p --output-format json`.
pub fn parse(stdout: &str) -> std::result::Result<Value, String> {
    let d: Value = serde_json::from_str(stdout.trim()).map_err(|_| "claude's answer wasn't JSON".to_string())?;
    let v = d
        .get("structured_output")
        .cloned()
        .filter(Value::is_object)
        .or_else(|| d.get("result").and_then(|r| r.as_str()).and_then(|r| serde_json::from_str(r).ok()))
        .ok_or("claude's answer had no result")?;
    if v["ok"] != true {
        let why = v["error"].as_str().filter(|e| !e.trim().is_empty()).unwrap_or("claude couldn't do it");
        return Err(format!("Jira through Claude: {}", one_line(why, 300)));
    }
    Ok(v)
}

/// The flags that keep a Jira call to the connector's tools.
pub fn tool_flags(app: &App) -> Vec<String> {
    vec!["--tools".into(), "".into(), "--allowedTools".into(), app.cfg.jira.claude_tools.join(",")]
}

/// Runs one op through headless Claude.
pub fn run(app: &App, op: &str, input: &Value) -> std::result::Result<Value, String> {
    let claude = proc::which(&app.cfg.claude).ok_or("claude isn't installed")?;
    let j = &app.cfg.jira;
    let mut args: Vec<String> = vec!["-p".into(), prompt(app, op, input), "--model".into(), j.claude_model.clone(), "--no-session-persistence".into()];
    args.extend(tool_flags(app));
    args.extend([
        "--max-budget-usd".into(),
        j.claude_budget_usd.clone(),
        "--json-schema".into(),
        schema().to_string(),
        "--output-format".into(),
        "json".into(),
    ]);
    let out = proc::run(&claude, &args, Some(&app.cfg.data), j.claude_timeout_secs as f64).map_err(|_| "claude didn't answer in time".to_string())?;
    parse(&out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_answers() {
        let ok = parse(r#"{"structured_output": {"ok": true, "status": "In Progress"}}"#).unwrap();
        assert_eq!(ok["status"], "In Progress");
        let from_result = parse(r#"{"result": "{\"ok\": true, \"key\": \"PROJ-4\"}"}"#).unwrap();
        assert_eq!(from_result["key"], "PROJ-4");
        let e = parse(r#"{"structured_output": {"ok": false, "error": "PROJ-4 can't go to Done"}}"#).unwrap_err();
        assert!(e.contains("can't go to Done"), "{e}");
        assert!(parse("not json").is_err());
    }
}
