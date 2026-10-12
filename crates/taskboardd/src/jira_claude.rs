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

/// The connector's tools that create, change or delete something. An op is denied every one it
/// doesn't need, so the owner's own allow rules (loaded with the user's settings) can't widen it.
pub const WRITES: [&str; 14] = [
    "createJiraIssue",
    "editJiraIssue",
    "transitionJiraIssue",
    "addCommentToJiraIssue",
    "addOrEditJiraIssueComment",
    "addWorklogToJiraIssue",
    "createIssueLink",
    "executeWrite",
    "executeDestructive",
    "createConfluencePage",
    "updateConfluencePage",
    "createConfluenceFooterComment",
    "createConfluenceInlineComment",
    "deleteConfluencePage",
];

/// The flags that keep a Jira call to the connector's tools: no built-in tools, no prompt that
/// approves itself (`-p` denies whatever `--allowedTools` leaves out, whatever the owner's
/// `defaultMode`), and every write the op doesn't need denied outright.
pub fn tool_flags(allowed: &[String]) -> Vec<String> {
    let denied: Vec<String> = WRITES.iter().map(|n| tool(n)).filter(|t| !allowed.contains(t)).collect();
    vec![
        "--tools".into(),
        "".into(),
        "--permission-mode".into(),
        "default".into(),
        "--allowedTools".into(),
        allowed.join(","),
        "--disallowedTools".into(),
        denied.join(","),
    ]
}

/// The arguments for one headless call. The user's settings come in because the claude.ai
/// connectors are theirs: with `--setting-sources ""` the connector's tools aren't there at all.
fn args(app: &App, prompt: String, allowed: &[String]) -> Vec<String> {
    let j = &app.cfg.jira;
    let mut args: Vec<String> = vec![
        "-p".into(),
        prompt,
        "--model".into(),
        j.claude_model.clone(),
        "--setting-sources".into(),
        "user".into(),
        "--no-session-persistence".into(),
    ];
    args.extend(tool_flags(allowed));
    args.extend([
        "--max-budget-usd".into(),
        j.claude_budget_usd.clone(),
        "--json-schema".into(),
        schema().to_string(),
        "--output-format".into(),
        "json".into(),
    ]);
    args
}

fn call(app: &App, prompt: String, allowed: &[String]) -> std::result::Result<Value, String> {
    let claude = proc::which(&app.cfg.claude).ok_or("claude isn't installed")?;
    let out = proc::run(&claude, &args(app, prompt, allowed), Some(&app.cfg.data), app.cfg.jira.claude_timeout_secs as f64)
        .map_err(|_| "claude didn't answer in time".to_string())?;
    parse(&out.stdout)
}

pub const UNREACHABLE: &str = "Jira through Claude can't reach the connector";

/// The boards (by data folder) whose calls have reached the connector: a failing op only checks it until then.
static REACHED: std::sync::Mutex<Vec<std::path::PathBuf>> = std::sync::Mutex::new(Vec::new());

fn reached(app: &App) -> bool {
    REACHED.lock().map(|r| r.contains(&app.cfg.data)).unwrap_or(false)
}

fn mark_reached(app: &App) {
    if let Ok(mut r) = REACHED.lock() {
        if !r.contains(&app.cfg.data) {
            r.push(app.cfg.data.clone());
        }
    }
}

/// One read-only call that only lists the connector's sites: an error says the connector's tools
/// aren't there (a Claude Code change, a signed-out connector), not that one op went wrong.
pub fn check(app: &App) -> std::result::Result<(), String> {
    let prompt = format!(
        "{}\n\nCall getAccessibleAtlassianResources once. Answer ok true if it listed at least one site; else ok false, \
         and if the tool isn't there at all, say so.",
        intro(app)
    );
    match call(app, prompt, &[tool("getAccessibleAtlassianResources")]) {
        Ok(_) => {
            mark_reached(app);
            Ok(())
        }
        Err(e) => Err(format!("{UNREACHABLE}: {}", e.trim_start_matches("Jira through Claude: "))),
    }
}

/// Runs one op through headless Claude. Until a call has reached the connector, a failing op
/// checks it, so a missing connector reads as that instead of as each op's own error.
pub fn run(app: &App, op: &str, input: &Value) -> std::result::Result<Value, String> {
    match call(app, prompt(app, op, input), &tools(app, op, input)) {
        Ok(v) => {
            mark_reached(app);
            Ok(v)
        }
        Err(e) if !reached(app) && e != "claude isn't installed" => check(app).and(Err(e)),
        Err(e) => Err(e),
    }
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
    fn ops_load_the_users_connectors_and_deny_the_writes_they_dont_need() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::for_tests(crate::config::Config::for_tests(dir.path()));
        let a = args(&app, "p".into(), &default_tools("comment", &json!({})));
        let at = |flag: &str| a[a.iter().position(|x| x == flag).unwrap_or_else(|| panic!("no {flag}: {a:?}")) + 1].clone();
        assert_eq!(at("--setting-sources"), "user", "the claude.ai connectors are the user's");
        assert_eq!(at("--tools"), "");
        assert_eq!(at("--permission-mode"), "default", "never the owner's bypassPermissions");
        let denied = at("--disallowedTools");
        assert!(denied.contains(&tool("createJiraIssue")) && denied.contains(&tool("executeWrite")), "{denied}");
        assert!(!denied.contains(&tool("addOrEditJiraIssueComment")), "the op's own write stays: {denied}");
        let status = tool_flags(&default_tools("status", &json!({})));
        assert_eq!(status[7].split(',').count(), WRITES.len(), "a read denies every write: {status:?}");
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
