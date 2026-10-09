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

/// The `cloudId` the connector's tools take: `[jira] cloud_id`, else the site's host name.
pub fn cloud_id(app: &App) -> String {
    let j = &app.cfg.jira;
    if j.cloud_id.trim().is_empty() {
        format!("the cloud id of {} (the site's host name works as one)", j.site.trim())
    } else {
        j.cloud_id.trim().to_string()
    }
}

/// How to read one ticket's comments: `getJiraIssue` doesn't return them.
fn comments_read(app: &App, key: &str) -> String {
    format!("executeRead: operation listJiraIssueComments, cloudId {}, inputs {{\"issueIdOrKey\": \"{key}\"}}", cloud_id(app))
}

fn intro(app: &App) -> String {
    let j = &app.cfg.jira;
    format!(
        "You work Jira for a task board through the Atlassian connector's tools (site {}, project {}; pass cloudId {} to \
         the tools that take one). Do exactly the one thing below, then answer in the JSON asked for. Never create, edit \
         or delete anything else. Never ask anyone anything. If it can't be done, answer ok false with the reason in \
         error, in one plain sentence.",
        j.site.trim(),
        j.project.trim(),
        cloud_id(app)
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
                "Move {key} to the status “{status}”. List its transitions with executeRead: operation listJiraIssueTransitions, \
                 cloudId {cloud}, inputs {{\"issueIdOrKey\": \"{key}\"}}. Pick \
                 the transition whose target status (its `to`) is “{status}”, not one that's only named like it, and pass that \
                 transition's id to transitionJiraIssue. If it's already there, leave it. If no transition gets there, answer ok \
                 false and say which statuses it can go to.",
                status = s("status"),
                cloud = cloud_id(app)
            );
            if !s("comment").is_empty() {
                t += &format!(" Then add this comment to it:\n{}\n", s("comment"));
            }
            t + " Answer status: its status's name once you're done."
        }
        "comment" => format!("Add this comment to {key}, as it is:\n{}", s("comment")),
        "create" => format!("{}\n\nAnswer key: the ticket's key, status: its status's name, found: true when it was already there, product: the product you picked (if you picked one).", s("brief")),
        "comments" => format!(
            "Make exactly one JQL search, with this query as it is: {}\nThen, for each ticket it finds, read its comments with {} \
             (getJiraIssue doesn't return them), and keep the ones created at or after {} (it's {} UTC now; none is fine). Don't \
             read any other ticket. Answer comments: one each, with key, id (the comment's id), created (its ISO time), author \
             (display name), author_account_id, author_email (when shown) and text (the comment as plain text).",
            s("jql"),
            comments_read(app, "<its key>"),
            s("after"),
            s("now")
        ),
        "comment_get" => format!(
            "Read comment {id} on {key}: list its comments with {} (getJiraIssue doesn't return them) and take the one whose id is \
             {id}. Answer author (display name), author_account_id, author_email (when shown) and text (the comment as plain text).",
            comments_read(app, &key),
            id = s("id")
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

const SERVER: &str = "mcp__claude_ai_Atlassian_MCP";

/// The connector's tool `name`, by its full name.
pub fn tool(name: &str) -> String {
    format!("{SERVER}__{name}")
}

/// The reads the desk gets beyond a new ticket's tools: it works on its own, so it may look up the
/// site's cloud id and an operation's inputs without stopping on a prompt.
pub const DESK_READS: [&str; 2] = ["getAccessibleAtlassianResources", "discover"];

/// The connector's tools one op needs, by exact name: reads, plus only the writes the op makes, so a
/// status read or a comment job can't create, edit or delete. The Jira desk gets `create`'s.
pub fn default_tools(op: &str, input: &Value) -> Vec<String> {
    let mut t = vec!["getJiraIssue", "executeRead"];
    match op {
        "transition" => {
            t.push("transitionJiraIssue");
            if input["comment"].as_str().is_some_and(|c| !c.trim().is_empty()) {
                t.push("addOrEditJiraIssueComment");
            }
        }
        "comment" => t.push("addOrEditJiraIssueComment"),
        "comments" => t.push("searchJiraIssuesUsingJql"),
        "create" => t.extend(["searchJiraIssuesUsingJql", "createJiraIssue"]),
        _ => {}
    }
    t.into_iter().map(tool).collect()
}

/// The tools one op may use: the owner's `[jira] claude_tools` when set, else `default_tools`.
pub fn tools(app: &App, op: &str, input: &Value) -> Vec<String> {
    let own: Vec<String> = app.cfg.jira.claude_tools.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
    if own.is_empty() {
        default_tools(op, input)
    } else {
        own
    }
}

/// The flags that keep a Jira call to the connector's tools.
pub fn tool_flags(app: &App, op: &str, input: &Value) -> Vec<String> {
    vec!["--tools".into(), "".into(), "--allowedTools".into(), tools(app, op, input).join(",")]
}

/// Runs one op through headless Claude.
pub fn run(app: &App, op: &str, input: &Value) -> std::result::Result<Value, String> {
    let claude = proc::which(&app.cfg.claude).ok_or("claude isn't installed")?;
    let j = &app.cfg.jira;
    // No user or project settings: their allow rules would widen the op's tools.
    let mut args: Vec<String> = vec![
        "-p".into(),
        prompt(app, op, input),
        "--model".into(),
        j.claude_model.clone(),
        "--setting-sources".into(),
        "".into(),
        "--no-session-persistence".into(),
    ];
    args.extend(tool_flags(app, op, input));
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

    fn names(op: &str, input: Value) -> Vec<String> {
        default_tools(op, &input).iter().map(|t| t.strip_prefix("mcp__claude_ai_Atlassian_MCP__").unwrap().to_string()).collect()
    }

    #[test]
    fn each_op_gets_only_the_tools_it_needs() {
        assert_eq!(names("status", json!({})), ["getJiraIssue", "executeRead"]);
        assert_eq!(names("comment_get", json!({})), ["getJiraIssue", "executeRead"]);
        assert_eq!(names("transition", json!({"status": "Done"})), ["getJiraIssue", "executeRead", "transitionJiraIssue"]);
        assert_eq!(
            names("transition", json!({"status": "Done", "comment": "shipped"})),
            ["getJiraIssue", "executeRead", "transitionJiraIssue", "addOrEditJiraIssueComment"]
        );
        assert_eq!(names("comment", json!({})), ["getJiraIssue", "executeRead", "addOrEditJiraIssueComment"]);
        assert_eq!(names("comments", json!({})), ["getJiraIssue", "executeRead", "searchJiraIssuesUsingJql"]);
        assert_eq!(names("create", json!({})), ["getJiraIssue", "executeRead", "searchJiraIssuesUsingJql", "createJiraIssue"]);
        for op in ["status", "transition", "comment", "comments", "comment_get", "create"] {
            let t = names(op, json!({"comment": "x"}));
            assert!(!t.iter().any(|n| ["executeWrite", "executeDestructive", "editJiraIssue"].contains(&n.as_str())), "{op}: {t:?}");
        }
    }

    #[test]
    fn the_transition_prompt_names_the_v2_operation() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = crate::config::Config::for_tests(dir.path());
        cfg.jira.site = "acme.atlassian.net".into();
        cfg.jira.project = "PROJ".into();
        let app = App::for_tests(cfg);
        let p = prompt(&app, "transition", &json!({"key": "PROJ-4", "status": "In Review"}));
        assert!(p.contains("executeRead: operation listJiraIssueTransitions"), "{p}");
        assert!(p.contains("cloudId the cloud id of acme.atlassian.net"), "{p}");
        assert!(p.contains(r#"inputs {"issueIdOrKey": "PROJ-4"}"#), "{p}");
        assert!(p.contains("target status (its `to`) is “In Review”"), "{p}");
        assert!(p.contains("id to transitionJiraIssue"), "{p}");
        assert!(p.contains("Never create, edit or delete anything else."), "{p}");
    }

    #[test]
    fn comment_reads_name_the_comments_operation_and_the_cloud_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = crate::config::Config::for_tests(dir.path());
        cfg.jira.site = "acme.atlassian.net".into();
        cfg.jira.project = "PROJ".into();
        let app = App::for_tests(cfg.clone());
        let p = prompt(&app, "comment_get", &json!({"key": "PROJ-4", "id": "10021"}));
        assert!(p.contains("executeRead: operation listJiraIssueComments"), "{p}");
        assert!(p.contains(r#"inputs {"issueIdOrKey": "PROJ-4"}"#), "{p}");
        assert!(p.contains("the one whose id is 10021"), "{p}");
        let p = prompt(&app, "comments", &json!({"jql": "project = PROJ", "after": "2026-10-01T00:00:00Z", "now": "x"}));
        assert!(p.contains("read its comments with executeRead: operation listJiraIssueComments"), "{p}");
        assert!(p.contains("pass cloudId the cloud id of acme.atlassian.net (the site's host name works as one)"), "{p}");

        // A set cloud id goes to every prompt as it is.
        cfg.jira.cloud_id = "1a2b-3c".into();
        let app = App::for_tests(cfg);
        for op in ["status", "transition", "comment_get"] {
            let p = prompt(&app, op, &json!({"key": "PROJ-4", "status": "Done", "id": "1"}));
            assert!(p.contains("cloudId 1a2b-3c"), "{op}: {p}");
            assert!(!p.contains("host name works"), "{op}: {p}");
        }
    }
}
