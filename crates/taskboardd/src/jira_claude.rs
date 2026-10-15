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
        "added": {"type": "array", "items": s},
        "author": s, "author_account_id": s, "author_email": s, "text": s,
        "comments": {"type": "array", "items": {"type": "object", "properties": {
            "key": s, "id": s, "created": s, "author": s, "author_account_id": s, "author_email": s, "text": s}}},
        "statuses": {"type": "array", "items": {"type": "object", "properties": {"key": s, "status": s, "status_category": s}}}}})
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
        "create" => format!("{}\n\nAnswer key: the ticket's key, status: its status's name, found: true when it was already there, added: on a ticket that was already there, what you gave it (e.g. \"labels web\", \"parent PROJ-1\"; empty when it lacked nothing), product: the product you picked (if you picked one).", s("brief")),
        "comments" => {
            // QA's check: the comments and, with `statuses` (the jira source in board mode), each ticket's status.
            let mut t = format!("Make exactly one JQL search, with this query as it is: {}\n", s("jql"));
            if input["comments"] != false {
                t += &format!(
                    "Then, for each ticket it finds, read its comments with {} (getJiraIssue doesn't return them), and keep the \
                     ones created at or after {} (it's {} UTC now; none is fine). Don't read any other ticket. Answer comments: \
                     one each, with key, id (the comment's id), created (its ISO time), author (display name), \
                     author_account_id, author_email (when shown) and text (the comment as plain text).",
                    comments_read(app, "<its key>"),
                    s("after"),
                    s("now")
                );
            } else {
                t += "Don't read any ticket's comments, or any other ticket.";
            }
            if input["statuses"] == true {
                t += " Answer statuses: one for each ticket the search finds, with key, status (its status's name, from the \
                      search) and status_category (its status category's key: new, indeterminate or done).";
            }
            t
        }
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

/// Built-in tools a Jira call never needs. Denied by name rather than with `--tools ""`, which also
/// takes away the connector's tools (they come through ToolSearch, which stays).
pub const BUILT_INS: [&str; 14] =
    ["Bash", "Edit", "Write", "MultiEdit", "NotebookEdit", "Read", "Glob", "Grep", "LS", "WebFetch", "WebSearch", "Agent", "Task", "Skill"];

/// The older Atlassian connector. With both connected, ToolSearch can offer its tools first; denying
/// the server by name (it doesn't match `SERVER`) keeps the call on the v2 one.
const OLD_SERVER: &str = "mcp__claude_ai_Atlassian";

/// The flags that keep a Jira call to the connector's tools: built-ins, the older connector and the
/// owner's other MCP servers (`servers`) denied, no prompt that approves itself (`-p` denies whatever
/// `--allowedTools` leaves out, whatever the owner's `defaultMode`), and every write the op doesn't
/// need denied outright.
pub fn tool_flags(allowed: &[String], servers: &[String]) -> Vec<String> {
    let mut denied: Vec<String> = WRITES.iter().map(|n| tool(n)).filter(|t| !allowed.contains(t)).collect();
    denied.push(OLD_SERVER.into());
    denied.extend(unused_servers(servers, allowed).into_iter().filter(|s| s != OLD_SERVER));
    denied.extend(BUILT_INS.iter().map(|b| b.to_string()));
    vec![
        "--permission-mode".into(),
        "default".into(),
        "--allowedTools".into(),
        allowed.join(","),
        "--disallowedTools".into(),
        denied.join(","),
    ]
}

/// How long the owner's MCP server list stands before `claude mcp list` runs again: it checks each
/// server's health, a few seconds a run.
const SERVERS_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

/// The last list per `claude` and folder (the folder's project servers count too).
#[allow(clippy::type_complexity)]
static SERVERS: std::sync::Mutex<Vec<(std::path::PathBuf, std::path::PathBuf, std::time::Instant, Vec<String>)>> =
    std::sync::Mutex::new(Vec::new());

/// The MCP servers the owner's Claude loads in `cwd`, by the names their tools go by, from `claude mcp
/// list`. A list it can't get (a timeout, output it can't read) is no servers, so nothing more is denied.
pub fn mcp_servers(claude: &std::path::Path, cwd: &std::path::Path) -> Vec<String> {
    let Ok(mut cache) = SERVERS.lock() else { return vec![] };
    if let Some((.., at, names)) = cache.iter().find(|(c, d, ..)| c == claude && d == cwd) {
        if at.elapsed() < SERVERS_TTL {
            return names.clone();
        }
    }
    let names = match proc::run(claude, &["mcp".into(), "list".into()], Some(cwd), 60.0) {
        Ok(o) if o.code == Some(0) => o.stdout.lines().filter_map(server_of).collect(),
        _ => vec![],
    };
    cache.retain(|(c, d, ..)| !(c == claude && d == cwd));
    cache.push((claude.to_path_buf(), cwd.to_path_buf(), std::time::Instant::now(), names.clone()));
    names
}

/// One `claude mcp list` line's server (`claude.ai Claude Docs: https://… - ✔ Connected`) as its
/// tools name it: Claude Code turns anything but letters, digits, `_` and `-` into `_`.
fn server_of(line: &str) -> Option<String> {
    let (name, rest) = line.split_once(": ")?;
    if name.trim().is_empty() || !rest.contains(" - ") {
        return None;
    }
    Some(name.trim().chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect())
}

/// The deny rules for the servers a call doesn't use: every one none of `allowed` comes from.
pub fn unused_servers(servers: &[String], allowed: &[String]) -> Vec<String> {
    servers
        .iter()
        .map(|s| format!("mcp__{s}"))
        .filter(|s| !allowed.iter().any(|a| a == s || a.starts_with(&format!("{s}__"))))
        .collect()
}

/// The arguments for one headless call. The user's settings come in because the claude.ai
/// connectors are theirs: with `--setting-sources ""` the connector's tools aren't there at all.
fn args(app: &App, prompt: String, allowed: &[String], servers: &[String]) -> Vec<String> {
    let j = &app.cfg.jira;
    let mut args: Vec<String> = vec![
        "-p".into(),
        prompt,
        "--model".into(),
        j.claude_model.clone(),
        "--setting-sources".into(),
        "user".into(),
        "--no-session-persistence".into(),
        "--disable-slash-commands".into(),
    ];
    args.extend(tool_flags(allowed, servers));
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
    let servers = mcp_servers(&claude, &app.cfg.data);
    let out = proc::run(&claude, &args(app, prompt, allowed, &servers), Some(&app.cfg.data), app.cfg.jira.claude_timeout_secs as f64)
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
        let servers = ["claude_ai_Atlassian_MCP".to_string(), "Amplitude".into()];
        let a = args(&app, "p".into(), &default_tools("comment", &json!({})), &servers);
        let at = |flag: &str| a[a.iter().position(|x| x == flag).unwrap_or_else(|| panic!("no {flag}: {a:?}")) + 1].clone();
        assert_eq!(at("--setting-sources"), "user", "the claude.ai connectors are the user's");
        assert!(!a.contains(&"--tools".to_string()), "--tools also hides the connector's tools: {a:?}");
        assert_eq!(at("--permission-mode"), "default", "never the owner's bypassPermissions");
        let denied = at("--disallowedTools");
        assert!(denied.contains(&tool("createJiraIssue")) && denied.contains(&tool("executeWrite")), "{denied}");
        assert!(!denied.contains(&tool("addOrEditJiraIssueComment")), "the op's own write stays: {denied}");
        let denied: Vec<&str> = denied.split(',').collect();
        assert!(denied.contains(&"Bash") && denied.contains(&"Read") && denied.contains(&"mcp__claude_ai_Atlassian"), "{denied:?}");
        assert!(!denied.contains(&SERVER), "the v2 connector stays: {denied:?}");
        assert!(denied.contains(&"mcp__Amplitude"), "the owner's other servers go: {denied:?}");
        assert!(a.contains(&"--disable-slash-commands".to_string()), "no skills: {a:?}");
        let status = tool_flags(&default_tools("status", &json!({})), &[]);
        assert_eq!(status[5].split(',').count(), WRITES.len() + 1 + BUILT_INS.len(), "a read denies every write: {status:?}");
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

    #[test]
    fn reads_mcp_list_lines_as_tool_names() {
        let out = "Checking MCP server health…\n\nclaude.ai Claude Docs: https://api.anthropic.com/v1/pages/mcp - ✔ Connected\n\
            plugin:dart-flutter:dart-mcp-server: dart mcp-server - ✘ Failed to connect\n\
            atlassian: https://mcp.atlassian.com/v1/sse (SSE) - ! Needs authentication\nNo MCP servers configured.\n";
        let names: Vec<String> = out.lines().filter_map(server_of).collect();
        assert_eq!(names, ["claude_ai_Claude_Docs", "plugin_dart-flutter_dart-mcp-server", "atlassian"]);
    }

    #[test]
    fn denies_only_the_servers_no_allowed_tool_comes_from() {
        let servers: Vec<String> = ["claude_ai_Atlassian_MCP", "claude_ai_Atlassian", "Amplitude"].iter().map(|s| s.to_string()).collect();
        let unused = unused_servers(&servers, &[tool("getJiraIssue"), "Bash(tb jira:*)".into()]);
        assert_eq!(unused, ["mcp__claude_ai_Atlassian", "mcp__Amplitude"], "a server's prefix isn't another's");
        assert_eq!(unused_servers(&servers, &["mcp__Amplitude".into()]), ["mcp__claude_ai_Atlassian_MCP", "mcp__claude_ai_Atlassian"]);
        let flags = tool_flags(&[tool("getJiraIssue")], &servers);
        assert_eq!(flags[5].matches("mcp__claude_ai_Atlassian,").count(), 1, "the older connector once: {flags:?}");
    }
}
