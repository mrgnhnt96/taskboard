//! Jira (optional): make tickets, move them, comment, read their status. Through the REST API with
//! a token, or (`[jira] via = "claude"`) a headless `claude -p` with the Atlassian connector's tools.
//! Jobs queue in the database and run one at a time outside any transaction: REST ones on the runner
//! thread, Claude ones on the AI thread, and new tickets on the Jira desk when it's on (`jira_desk.rs`).

use std::collections::BTreeMap;

use base64_lite::encode as b64;
use serde_json::{json, Value};

use crate::config::JiraProduct;

use crate::app::App;
use crate::util::*;
use crate::{board, fields, p};

const TIMEOUT_SECS: u64 = 20;
const RUNNING_EXPIRY_SECS: f64 = 600.0;

pub fn request_job(app: &App, args: Value, target: Value) -> Result<i64> {
    let args: Row = args.as_object().cloned().unwrap_or_default().into_iter().filter(|(_, v)| !v.is_null() && v != "").collect();
    let task = target.get("task").and_then(|v| v.as_i64());
    board::create_job(app, "jira", Value::Object(args), task, "jira", Some(target))
}

pub fn request(app: &App, op: &str, task_id: i64, key: &str, status: Option<&str>, comment: Option<&str>) -> Result<i64> {
    request_job(app, json!({"op": op, "key": key, "status": status, "comment": comment}), json!({"task": task_id}))
}

/// A move or comment the board makes by itself to keep the ticket in step with its task. Its result is
/// logged as the board's, so it never counts as the agent moving on.
pub fn request_by_board(app: &App, op: &str, task_id: i64, key: &str, status: Option<&str>, comment: Option<&str>) -> Result<i64> {
    request_job(app, json!({"op": op, "key": key, "status": status, "comment": comment, "by": "board"}), json!({"task": task_id}))
}

pub fn clean_product(app: &App, v: &str) -> Result<Option<String>> {
    let v = v.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let products = &app.cfg.jira.products;
    if products.is_empty() {
        return Ok(Some(v.to_string()));
    }
    match products.keys().find(|k| k.eq_ignore_ascii_case(v)) {
        Some(k) => Ok(Some(k.clone())),
        None => err(400, format!("The product is one of {}.", products.keys().cloned().collect::<Vec<_>>().join(", "))),
    }
}

fn epic_asked(app: &App, gid: i64) -> Result<bool> {
    Ok(app
        .db
        .q1(
            "SELECT id FROM jobs WHERE kind = 'jira' AND state IN ('pending', 'running') \
             AND json_extract(target, '$.goal') = ? AND json_extract(args, '$.op') = 'create'",
            p![gid],
        )?
        .is_some())
}

pub fn request_epic(app: &App, g: &Row) -> Result<i64> {
    request_job(
        app,
        json!({"op": "create", "summary": g.v("name"), "description": g.v("outcome"),
               "issue_type": app.cfg.jira.epic_type, "product": g.v("product")}),
        json!({"goal": g.id()}),
    )
}

pub fn ensure_epic(app: &App, g: Option<&Row>) -> Result<()> {
    let Some(g) = g else { return Ok(()) };
    if has(g.s("epic_key")) || epic_asked(app, g.id())? || !crate::projects::ships_prs(app, &g.st("project"))? {
        return Ok(());
    }
    request_epic(app, g)?;
    app.info(format!("jira: asked for an epic for {} before its first ticket", rf("goal", g.id())));
    Ok(())
}

/// The project no longer ships PRs: its tickets and epics that haven't been asked for yet aren't wanted.
pub fn drop_pending_creates(app: &App, project: &str) -> Result<()> {
    app.db.x(
        "UPDATE jobs SET state = 'cancelled', updated_at = ? WHERE kind = 'jira' AND state = 'pending' \
         AND json_extract(args, '$.op') = 'create' AND ( \
           json_extract(target, '$.task') IN (SELECT id FROM tasks WHERE project = ?) \
           OR json_extract(target, '$.goal') IN (SELECT id FROM goals WHERE project = ?))",
        p![now_iso(), project, project],
    )?;
    Ok(())
}

pub fn request_create_for_task(app: &App, t: &Row) -> Result<i64> {
    create_for_task(app, t, None)
}

/// A ticket the board asks for by itself (`auto_ticket`): its result is logged as the board's, so it never
/// counts as the agent moving on.
pub fn request_create_by_board(app: &App, t: &Row) -> Result<i64> {
    create_for_task(app, t, Some("board"))
}

fn create_for_task(app: &App, t: &Row, by: Option<&str>) -> Result<i64> {
    let g = board::find_goal(app, t.i("goal_id"))?;
    ensure_epic(app, g.as_ref())?;
    let summary = t.s("ticket_summary").filter(|s| !s.trim().is_empty()).unwrap_or(&t.st("title")).to_string();
    let text = t.s("ticket_text").filter(|s| !s.trim().is_empty()).unwrap_or(&t.st("detail")).to_string();
    let vars = [("summary", summary.clone()), ("title", t.st("title")), ("detail", t.st("detail")), ("task", rf("task", t.id()))];
    request_job(
        app,
        json!({"op": "create", "summary": summary, "description": described(app, &text, &vars, g.as_ref(), &t.st("project")),
               "issue_type": app.cfg.jira.task_type, "by": by}),
        json!({"task": t.id()}),
    )
}

/// The text of a `[jira]` file setting (`ticket_template`, `ticket_guide`): None when it's not set,
/// Err when it can't be read.
pub fn config_file(path: &str) -> Option<std::result::Result<String, String>> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    Some(std::fs::read_to_string(expand_home(path)).map_err(|e| format!("can't read {path}: {e}")))
}

/// A new ticket an agent asks for when the board has a ticket guide needs its text written to it:
/// the task's own title and detail aren't.
pub fn refuse_unguided(app: &App, who: &str, ticket_text: Option<&str>) -> Result<()> {
    if who == board::OWNER || ticket_text.is_some_and(|t| !t.trim().is_empty()) || guide(app).is_none() {
        return Ok(());
    }
    err(
        400,
        "This board writes its tickets to a guide: read it in `tb jira`, then give the ticket its text with --ticket-text \
         (and --ticket-summary) alongside --jira new.",
    )
}

/// `[jira] ticket_guide`'s text, when it's set and there.
pub fn guide(app: &App) -> Option<String> {
    config_file(&app.cfg.jira.ticket_guide).and_then(|r| r.ok()).filter(|g| !g.trim().is_empty())
}

/// Fills a ticket template's `{name}`s; a name it doesn't know stays as it is.
pub fn render_template(template: &str, vars: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v.trim());
    }
    out.trim().to_string()
}

/// A new ticket's description: its text, in `[jira] ticket_template` when there is one.
fn described(app: &App, text: &str, vars: &[(&str, String)], g: Option<&Row>, project: &str) -> String {
    let template = match config_file(&app.cfg.jira.ticket_template) {
        None => return text.to_string(),
        Some(Err(e)) => {
            app.info(format!("jira: ticket_template: {e}; the ticket gets its text as it is"));
            return text.to_string();
        }
        Some(Ok(t)) => t,
    };
    let mut all = vec![("description", text.to_string()), ("project", project.to_string())];
    all.extend(vars.iter().cloned());
    all.push(("goal", g.map(|g| g.st("name")).unwrap_or_default()));
    all.push(("outcome", g.map(|g| g.st("outcome")).unwrap_or_default()));
    all.push(("epic", g.map(|g| g.st("epic_key")).unwrap_or_default()));
    for k in ["summary", "title", "detail", "task"] {
        if !all.iter().any(|(n, _)| *n == k) {
            all.push((k, String::new()));
        }
    }
    render_template(&template, &all)
}

pub fn request_create_for_issue(app: &App, b: &Row) -> Result<i64> {
    let g = board::find_goal(app, b.i("goal_id"))?;
    ensure_epic(app, g.as_ref())?;
    let mut desc = b.st("detail");
    let snap = crate::handoff::snapshot_line(&jloads_obj(b.s("snapshot")));
    if !snap.is_empty() {
        desc += &format!("\n\nWhat was happening when it was found: {snap}");
    }
    let vars = [("summary", b.st("title")), ("title", b.st("title")), ("detail", b.st("detail"))];
    request_job(
        app,
        json!({"op": "create", "summary": b.v("title"), "description": described(app, &desc, &vars, g.as_ref(), &b.st("project")),
               "issue_type": app.cfg.jira.bug_type}),
        json!({"issue": b.id()}),
    )
}

pub(crate) fn job_goal(app: &App, target: &Row) -> Result<Option<Row>> {
    let gid = if let Some(t) = target.i("task") {
        board::find_task(app, Some(t))?.and_then(|t| t.i("goal_id"))
    } else if let Some(b) = target.i("issue") {
        board::find_issue(app, Some(b))?.and_then(|b| b.i("goal_id"))
    } else {
        target.i("goal")
    };
    board::find_goal(app, gid)
}

fn credentials(app: &App) -> Option<(String, String)> {
    {
        let s = app.shared.lock();
        if s.jira_token_off {
            return None;
        }
        if let Some(c) = &s.jira_creds {
            return Some(c.clone());
        }
    }
    let j = &app.cfg.jira;
    let mut email = j.email.clone();
    let mut token = j.token.clone();
    if token.is_empty() && !j.keychain_item.is_empty() {
        if let Ok(o) = std::process::Command::new("security").args(["find-generic-password", "-s", &j.keychain_item, "-w"]).output() {
            if o.status.success() {
                token = String::from_utf8_lossy(&o.stdout).trim().to_string();
            }
        }
        if email.is_empty() {
            if let Ok(o) = std::process::Command::new("security").args(["find-generic-password", "-s", &j.keychain_item]).output() {
                let text = String::from_utf8_lossy(&o.stdout);
                if let Some(c) = regex::Regex::new(r#""acct"<blob>="([^"]*)""#).unwrap().captures(&text) {
                    email = c[1].to_string();
                }
            }
        }
    }
    if email.is_empty() || token.is_empty() {
        return None;
    }
    app.shared.lock().jira_creds = Some((email.clone(), token.clone()));
    Some((email, token))
}

struct JiraError {
    message: String,
    auth: bool,
}

fn call(app: &App, creds: &(String, String), method: &str, path: &str, body: Option<Value>) -> std::result::Result<Value, JiraError> {
    let url = format!("https://{}/rest/api/3/{path}", app.cfg.jira.site.trim());
    let auth = format!("Basic {}", b64(format!("{}:{}", creds.0, creds.1).as_bytes()));
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(TIMEOUT_SECS)).build();
    let req = agent.request(method, &url).set("Authorization", &auth).set("Accept", "application/json");
    let resp = match body {
        Some(b) => req.send_json(b),
        None => req.call(),
    };
    match resp {
        Ok(r) => {
            let text = r.into_string().unwrap_or_default();
            Ok(if text.trim().is_empty() { json!({}) } else { serde_json::from_str(&text).unwrap_or(json!({})) })
        }
        Err(ureq::Error::Status(code, r)) => {
            let text = r.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&text)
                .ok()
                .map(|d| {
                    let mut parts: Vec<String> =
                        d["errorMessages"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
                    if let Some(e) = d["errors"].as_object() {
                        parts.extend(e.iter().map(|(k, v)| format!("{k}: {}", v.as_str().unwrap_or(""))));
                    }
                    parts.join("; ")
                })
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| clip(text.trim(), 200));
            Err(JiraError { message: format!("Jira said {code}: {msg}"), auth: code == 401 || code == 403 })
        }
        Err(e) => Err(JiraError { message: format!("couldn't reach Jira: {e}"), auth: false }),
    }
}

/// A GET or POST on Jira's REST API v3 for code outside the job queue (QA comments). Err when Jira
/// is off, has no credentials, or answers with an error.
pub fn api(app: &App, method: &str, path: &str, body: Option<Value>) -> std::result::Result<Value, String> {
    if !app.cfg.jira_on() {
        return Err("Jira isn't set up".into());
    }
    let creds = credentials(app).ok_or("Jira has no API token")?;
    call(app, &creds, method, path, body).map_err(|e| e.message)
}

/// Jira's rich text (ADF) as plain text: paragraphs and list items on their own lines.
pub fn adf_text(v: &Value) -> String {
    fn walk(v: &Value, out: &mut String) {
        match v["type"].as_str() {
            Some("text") => out.push_str(v["text"].as_str().unwrap_or("")),
            Some("hardBreak") => out.push('\n'),
            Some("mention") => out.push_str(v["attrs"]["text"].as_str().unwrap_or("")),
            Some("inlineCard") | Some("blockCard") => out.push_str(v["attrs"]["url"].as_str().unwrap_or("")),
            _ => {}
        }
        for c in v["content"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            walk(c, out);
        }
        if matches!(v["type"].as_str(), Some("paragraph") | Some("heading") | Some("listItem") | Some("codeBlock")) && !out.ends_with('\n') {
            out.push('\n');
        }
    }
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    let mut out = String::new();
    walk(v, &mut out);
    out.trim().to_string()
}

fn adf(text: &str) -> Value {
    let paras: Vec<Value> = text
        .split('\n')
        .filter(|p| !p.trim().is_empty())
        .map(|p| json!({"type": "paragraph", "content": [{"type": "text", "text": p}]}))
        .collect();
    json!({"type": "doc", "version": 1, "content": paras})
}

/// How a job reaches Jira: the REST API with the token, or headless Claude with the connector.
pub(crate) enum Conn {
    Rest((String, String)),
    Claude,
}

impl JiraError {
    fn new(message: impl Into<String>) -> JiraError {
        JiraError { message: message.into(), auth: false }
    }
}

fn claude(app: &App, op: &str, input: Value) -> std::result::Result<Value, JiraError> {
    crate::jira_claude::run(app, op, &input).map_err(JiraError::new)
}

fn status_of(app: &App, c: &Conn, key: &str) -> std::result::Result<String, JiraError> {
    match c {
        Conn::Rest(cr) => {
            let d = call(app, cr, "GET", &format!("issue/{key}?fields=status"), None)?;
            Ok(d["fields"]["status"]["name"].as_str().unwrap_or("").to_string())
        }
        Conn::Claude => Ok(claude(app, "status", json!({"key": key}))?["status"].as_str().unwrap_or("").to_string()),
    }
}

fn add_comment(app: &App, c: &Conn, key: &str, text: &str) -> std::result::Result<(), JiraError> {
    match c {
        Conn::Rest(cr) => call(app, cr, "POST", &format!("issue/{key}/comment"), Some(json!({"body": adf(text)}))).map(|_| ()),
        Conn::Claude => claude(app, "comment", json!({"key": key, "comment": text})).map(|_| ()),
    }
}

fn transition(app: &App, c: &Conn, key: &str, target: &str, comment: Option<&str>) -> std::result::Result<String, JiraError> {
    let cr = match c {
        Conn::Rest(cr) => cr,
        Conn::Claude => {
            let d = claude(app, "transition", json!({"key": key, "status": target, "comment": comment}))?;
            return Ok(d["status"].as_str().filter(|s| !s.is_empty()).unwrap_or(target).to_string());
        }
    };
    let now = status_of(app, c, key)?;
    let moved = if now.eq_ignore_ascii_case(target) {
        now
    } else {
        let ts = call(app, cr, "GET", &format!("issue/{key}/transitions"), None)?;
        let list = ts["transitions"].as_array().cloned().unwrap_or_default();
        let hit = list
            .iter()
            .find(|t| t["to"]["name"].as_str().map(|n| n.eq_ignore_ascii_case(target)).unwrap_or(false))
            .or_else(|| list.iter().find(|t| t["name"].as_str().map(|n| n.eq_ignore_ascii_case(target)).unwrap_or(false)));
        let Some(t) = hit else {
            let mut can: Vec<String> = list.iter().filter_map(|t| t["to"]["name"].as_str().map(|s| s.to_string())).collect();
            can.sort();
            can.dedup();
            let can = if can.is_empty() { "nothing".to_string() } else { can.join(", ") };
            return Err(JiraError::new(format!("{key} can't go from {now} to {target} (it can go to {can})")));
        };
        call(app, cr, "POST", &format!("issue/{key}/transitions"), Some(json!({"transition": {"id": t["id"]}})))?;
        t["to"]["name"].as_str().unwrap_or(target).to_string()
    };
    if let Some(text) = comment.filter(|t| !t.trim().is_empty()) {
        add_comment(app, c, key, text)?;
    }
    Ok(moved)
}

/// Words that say nothing about what the work is, left out when work is matched by its words.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "into", "onto", "that", "this", "these", "those", "its", "it's", "are", "was", "were",
    "been", "being", "has", "have", "had", "not", "but", "all", "any", "can", "our", "out", "you", "your", "their", "them",
    "they", "there", "then", "than", "when", "what", "which", "who", "whom", "why", "how", "where", "will", "would", "should",
    "could", "may", "might", "must", "also", "just", "only", "more", "most", "some", "such", "very", "too", "via", "per",
    "about", "after", "before", "over", "under", "again", "each", "every", "both", "one", "use", "using", "used", "make",
    "new", "add", "get", "set", "app", "does", "doesn't", "nothing", "something", "now", "off", "own",
];

fn words(text: &str) -> std::collections::BTreeSet<String> {
    words_in_order(text).into_iter().collect()
}

/// The text's meaningful words in the order it says them, each once.
fn words_in_order(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for w in text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .map(|w| w.trim_matches('\''))
        .filter(|w| w.chars().count() >= 3 && !STOPWORDS.contains(w))
        .map(|w| w.replace('\'', ""))
    {
        if !out.contains(&w) {
            out.push(w);
        }
    }
    out
}

/// The JQL that finds open tickets covering `summary`: for an epic any of its first 8 words (in the
/// title's order, so the ones it leads with), else all of them.
pub fn summary_jql(summary: &str, epic: bool) -> Option<String> {
    let text = words_in_order(summary);
    if text.is_empty() {
        return None;
    }
    Some(if epic {
        format!("({})", text.iter().take(8).map(|w| format!("summary ~ \"{w}\"")).collect::<Vec<_>>().join(" OR "))
    } else {
        format!("summary ~ \"{}\"", text.join(" "))
    })
}

/// The product whose name and `what` share the most words with the work, when one clearly does.
pub fn pick_product(products: &BTreeMap<String, JiraProduct>, text: &str) -> Option<String> {
    if products.len() == 1 {
        return products.keys().next().cloned();
    }
    let have = words(text);
    let mut scored: Vec<(usize, &String)> =
        products.iter().map(|(name, p)| (words(&format!("{name} {}", p.what)).intersection(&have).count(), name)).collect();
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    match scored.as_slice() {
        [(n, name), rest @ ..] if *n > 0 && rest.first().map(|r| r.0 < *n).unwrap_or(true) => Some((*name).clone()),
        _ => None,
    }
}

/// The ticket's product: the goal's, the job's, the one the work's words pick, or `default_product`.
fn product_for(app: &App, a: &Row, g: Option<&Row>) -> Option<String> {
    let j = &app.cfg.jira;
    g.and_then(|g| g.s("product"))
        .or(a.s("product"))
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .or_else(|| pick_product(&j.products, &format!("{} {}", a.st("summary"), a.st("description"))))
        .or_else(|| Some(j.default_product.trim().to_string()).filter(|d| !d.is_empty()))
}

fn product_fields(p: &JiraProduct, f: &mut Value, labels: &mut Vec<String>) {
    labels.extend(p.labels.iter().cloned());
    if !p.components.is_empty() {
        f["components"] = json!(p.components.iter().map(|c| json!({"name": c})).collect::<Vec<_>>());
    }
    for (k, v) in &p.fields {
        f[k] = serde_json::to_value(v).unwrap_or(Value::Null);
    }
}

/// A new ticket's fields as Jira's REST API takes them, and its product (None when products are set
/// up but nothing picked one: Claude or the desk picks it then).
pub fn ticket_fields(app: &App, a: &Row, g: Option<&Row>) -> (Value, Option<String>) {
    let j = &app.cfg.jira;
    let issue_type = a.s("issue_type").filter(|s| !s.is_empty()).unwrap_or(&j.task_type).to_string();
    let mut f = json!({
        "project": {"key": j.project}, "summary": clip(&one_line(&a.st("summary"), 250), 250),
        "issuetype": {"name": issue_type},
    });
    let desc = a.st("description");
    if !desc.trim().is_empty() {
        f["description"] = crate::adf::from_markdown(&desc);
    }
    if !j.assignee_account_id.is_empty() {
        f["assignee"] = json!({"accountId": j.assignee_account_id});
    }
    let mut labels = j.labels.clone();
    let product = product_for(app, a, g);
    if let Some(p) = product.as_deref().and_then(|p| j.products.get(p)) {
        product_fields(p, &mut f, &mut labels);
    }
    if !labels.is_empty() {
        labels.dedup();
        f["labels"] = json!(labels);
    }
    if !issue_type.eq_ignore_ascii_case(&j.epic_type) {
        if let Some(epic) = g.and_then(|g| g.s("epic_key")).filter(|e| !e.is_empty()) {
            f["parent"] = json!({"key": epic});
        }
    }
    (f, product)
}

/// The fields a new ticket gets that route it (team, labels, parent epic, components, a product's
/// own fields): what a reused ticket is given when it lacks them. Not its summary, description,
/// type, project or assignee.
fn routing_keys(f: &Value) -> Vec<String> {
    f.as_object()
        .map(|o| o.keys().filter(|k| !["project", "summary", "issuetype", "description", "assignee"].contains(&k.as_str())).cloned().collect())
        .unwrap_or_default()
}

/// What a found ticket (`have`, its current fields) lacks of a new ticket's routing fields (`f`), as
/// a REST update, and what was added in words. Labels are merged with its own (Jira sets them, so
/// the whole list goes); the parent epic only when it has none; any other field only when it's empty.
pub fn fill_in(f: &Value, have: &Value) -> (Value, Vec<String>) {
    let empty = |v: &Value| v.is_null() || v.as_str() == Some("") || v.as_array().is_some_and(|a| a.is_empty()) || v.as_object().is_some_and(|o| o.is_empty());
    let mut update = json!({});
    let mut added = vec![];
    for k in routing_keys(f) {
        match k.as_str() {
            "labels" => {
                let mut labels: Vec<String> = have["labels"].as_array().map(|a| a.iter().filter_map(|l| l.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
                let missing: Vec<String> = f["labels"].as_array().map(|a| a.iter().filter_map(|l| l.as_str()).filter(|l| !labels.iter().any(|x| x == l)).map(|l| l.to_string()).collect()).unwrap_or_default();
                if !missing.is_empty() {
                    added.push(format!("labels {}", missing.join(", ")));
                    labels.extend(missing);
                    update["labels"] = json!(labels);
                }
            }
            "parent" if empty(&have["parent"]) => {
                added.push(format!("parent {}", f["parent"]["key"].as_str().unwrap_or("epic")));
                update["parent"] = f["parent"].clone();
            }
            "parent" => {}
            _ if empty(&have[&k]) => {
                added.push(k.clone());
                update[&k] = f[&k].clone();
            }
            _ => {}
        }
    }
    (update, added)
}

/// Gives a found ticket what it lacks of the job's routing fields (REST). What was added; nothing when
/// it lacked nothing or Jira wouldn't say (the link still stands).
fn fill_in_found(app: &App, cr: &(String, String), key: &str, f: &Value) -> Vec<String> {
    let keys = routing_keys(f);
    if keys.is_empty() {
        return vec![];
    }
    let Ok(d) = call(app, cr, "GET", &format!("issue/{key}?fields={}", keys.join(",")), None) else { return vec![] };
    let (update, added) = fill_in(f, &d["fields"]);
    if added.is_empty() || call(app, cr, "PUT", &format!("issue/{key}"), Some(json!({"fields": update}))).is_err() {
        return vec![];
    }
    added
}

/// What Claude (headless or the desk) is told to find or make: search first, then the fields.
pub fn ticket_brief(app: &App, a: &Row, g: Option<&Row>) -> String {
    let j = &app.cfg.jira;
    let (f, product) = ticket_fields(app, a, g);
    let issue_type = f["issuetype"]["name"].as_str().unwrap_or("Task").to_string();
    let epic = issue_type.eq_ignore_ascii_case(&j.epic_type);
    let mut extra = f.clone();
    for k in ["project", "summary", "issuetype", "description"] {
        extra.as_object_mut().map(|o| o.remove(k));
    }
    let first = if epic {
        format!(
            "First pick an open epic (not done) in {} that already covers this work, even when it's worded differently: search \
             the open epics by the words of the summary and description below. If one covers it, use it: don't make another.",
            j.project.trim()
        )
    } else {
        format!(
            "First search {} for an open ticket (not done) that already covers it, by its summary and description. If one does, \
             use it: don't make another.",
            j.project.trim()
        )
    };
    let reuse = if routing_keys(&extra).is_empty() {
        "Don't change a ticket you use.".to_string()
    } else {
        "On a ticket you use, set only what it lacks of the fields below: add the labels it doesn't have to its own (send \
         the whole merged list), give it the parent epic only when it has none, and set any other field only when it's empty. \
         Change nothing else (summary, description, status, assignee), never move it from a different parent, and say what you \
         added."
            .to_string()
    };
    let mut lines = vec![
        format!("Find or make the Jira {} for this work in project {}.", if epic { "epic" } else { "ticket" }, j.project.trim()),
        first,
        reuse,
        format!(
            "Only if none {}, make a {issue_type} with this summary and description{}",
            if epic { "fits" } else { "does" },
            if extra.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                ".".to_string()
            } else {
                format!(", and these fields as Jira's REST API names them: {extra}")
            }
        ),
    ];
    if product.is_none() && !j.products.is_empty() {
        lines.push("Pick its product from what each is for, and give the ticket that product's fields too:".into());
        for (name, p) in &j.products {
            let (mut pf, mut labels) = (json!({}), vec![]);
            product_fields(p, &mut pf, &mut labels);
            if !labels.is_empty() {
                pf["labels"] = json!(labels);
            }
            lines.push(format!("- {name}: {}. Fields: {pf}", if p.what.trim().is_empty() { "no description" } else { p.what.trim() }));
        }
        lines.push("If none fits, pick the closest.".into());
    }
    lines.push(format!("Summary: {}", f["summary"].as_str().unwrap_or("")));
    let desc = a.st("description");
    if !desc.trim().is_empty() {
        lines.push(format!("Description:\n{}", clip(desc.trim(), 3000)));
    }
    lines.join("\n")
}

/// Of these open epics (key, summary), newest first, the one that already covers a goal named
/// `summary`: the same summary, or else the one sharing the most of its words, when they're at least
/// half of either's.
pub fn covering_epic(summary: &str, epics: &[(String, String)]) -> Option<String> {
    let norm = |s: &str| s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    if let Some((k, _)) = epics.iter().find(|(_, s)| norm(s) == norm(summary)) {
        return Some(k.clone());
    }
    let want = words(summary);
    let mut best: Option<(usize, &String)> = None;
    for (k, s) in epics {
        let have = words(s);
        let n = want.intersection(&have).count();
        if n > 0 && n * 2 >= want.len().max(have.len()) && best.map(|b| n > b.0).unwrap_or(true) {
            best = Some((n, k));
        }
    }
    best.map(|b| b.1.clone())
}

/// An open ticket of the type that already covers the work, when there is one (REST): one with the
/// same summary, or for an epic, the open epic whose summary best shares its words.
fn find_open(app: &App, cr: &(String, String), f: &Value) -> std::result::Result<Option<String>, JiraError> {
    let summary = f["summary"].as_str().unwrap_or("");
    let issue_type = f["issuetype"]["name"].as_str().unwrap_or("");
    let epic = issue_type.eq_ignore_ascii_case(&app.cfg.jira.epic_type);
    let Some(matching) = summary_jql(summary, epic) else { return Ok(None) };
    let jql = format!(
        "project = \"{}\" AND issuetype = \"{}\" AND statusCategory != Done AND {matching} ORDER BY created DESC",
        app.cfg.jira.project.trim().replace('"', ""),
        issue_type.replace('"', ""),
    );
    let d = call(app, cr, "POST", "search/jql", Some(json!({"jql": jql, "fields": ["summary"], "maxResults": 20})))?;
    let found: Vec<(String, String)> = d["issues"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| Some((i["key"].as_str()?.to_string(), i["fields"]["summary"].as_str().unwrap_or("").to_string())))
                .collect()
        })
        .unwrap_or_default();
    if epic {
        return Ok(covering_epic(summary, &found));
    }
    let norm = |s: &str| s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    Ok(found.into_iter().find(|(_, s)| norm(s) == norm(summary)).map(|(k, _)| k))
}

/// Finds an open ticket that already covers the work, or makes one. Ok((key, status, {found, product})).
fn create(app: &App, c: &Conn, a: &Row, g: Option<&Row>) -> std::result::Result<(String, String, Value), JiraError> {
    let cr = match c {
        Conn::Rest(cr) => cr,
        Conn::Claude => {
            let d = claude(app, "create", json!({"brief": ticket_brief(app, a, g)}))?;
            let key = d["key"].as_str().unwrap_or("").trim().to_uppercase();
            if crate::ops::jira_key(&key).is_err() {
                return Err(JiraError::new("Claude gave no ticket key"));
            }
            let product = d["product"].as_str().filter(|p| !p.is_empty()).map(|p| json!(p)).unwrap_or(Value::Null);
            let added = if d["found"] == true { d["added"].clone() } else { Value::Null };
            return Ok((key, d["status"].as_str().unwrap_or("").to_string(), json!({"found": d["found"] == true, "product": product, "added": added})));
        }
    };
    let (f, product) = ticket_fields(app, a, g);
    let product = json!(product);
    // A failed search only means the board makes the ticket.
    if let Ok(Some(key)) = find_open(app, cr, &f) {
        let added = fill_in_found(app, cr, &key, &f);
        let status = status_of(app, c, &key).unwrap_or_default();
        return Ok((key, status, json!({"found": true, "product": product, "added": added})));
    }
    let d = call(app, cr, "POST", "issue", Some(json!({"fields": f})))?;
    let key = d["key"].as_str().unwrap_or("").to_string();
    if key.is_empty() {
        return Err(JiraError::new("Jira made no ticket"));
    }
    let status = status_of(app, c, &key).unwrap_or_default();
    Ok((key, status, json!({"found": false, "product": product})))
}

type Done = (Option<String>, Option<String>, Value);

fn run_one(app: &App, job: &Row, c: &Conn) -> std::result::Result<Done, JiraError> {
    let a = board::job_args(job);
    let target = board::job_target(job);
    let key = a.st("key");
    match a.st("op").as_str() {
        "create" => {
            let g = job_goal(app, &target).map_err(|e| JiraError::new(e.message))?;
            let (k, s, extra) = create(app, c, &a, g.as_ref())?;
            Ok((Some(k), Some(s), extra))
        }
        _ if key.is_empty() => Err(JiraError::new("the job has no ticket key")),
        "transition" => {
            let st = transition(app, c, &key, &a.st("status"), a.s("comment"))?;
            Ok((Some(key), Some(st), Value::Null))
        }
        "status" => {
            let st = status_of(app, c, &key)?;
            Ok((Some(key), Some(st), Value::Null))
        }
        "comment" => {
            add_comment(app, c, &key, &a.st("comment"))?;
            Ok((Some(key), None, Value::Null))
        }
        "merged_note" => merged_note(app, c, &key, &a, target.i("task")),
        op => Err(JiraError::new(format!("the board doesn't know the Jira job {op}"))),
    }
}

/// The note for testers, then the "PR #n was merged" line, then the move: each comment once, however
/// many tries the move takes.
fn merged_note(app: &App, c: &Conn, key: &str, a: &Row, task: Option<i64>) -> std::result::Result<Done, JiraError> {
    let posted = task.map(|t| crate::merged_note::posted(app, t)).unwrap_or_default();
    if posted.is_empty() {
        add_markdown_comment(app, c, key, &a.st("note"))?;
        task.inspect(|t| crate::merged_note::set_posted(app, *t, "note"));
    }
    if posted != "both" {
        add_comment(app, c, key, &a.st("comment"))?;
        task.inspect(|t| crate::merged_note::set_posted(app, *t, "both"));
    }
    let target = a.st("status");
    if target.trim().is_empty() {
        return Ok((Some(key.to_string()), None, Value::Null));
    }
    let st = transition(app, c, key, &target, None)?;
    Ok((Some(key.to_string()), Some(st), Value::Null))
}

fn add_markdown_comment(app: &App, c: &Conn, key: &str, text: &str) -> std::result::Result<(), JiraError> {
    match c {
        Conn::Rest(cr) => call(app, cr, "POST", &format!("issue/{key}/comment"), Some(json!({"body": crate::adf::from_markdown(text)}))).map(|_| ()),
        Conn::Claude => claude(app, "comment", json!({"key": key, "comment": text})).map(|_| ()),
    }
}

fn fail_all(app: &App, jobs: &[Row], why: &str) -> Result<()> {
    app.db.tx(|| {
        for j in jobs {
            finish(app, j.id(), false, None, None, Some(why))?;
        }
        Ok(())
    })
}

/// The job goes to the Jira desk (`[jira] desk`): finding or making a ticket.
pub fn for_desk(app: &App, j: &Row) -> bool {
    crate::jira_desk::on(app) && board::job_args(j).s("op") == Some("create")
}

/// Runs the pending Jira jobs, oldest first: through the REST API one after another, through headless
/// Claude one at a time on the AI thread, and new tickets through the desk when it's on.
pub fn run_pending(app: &App) -> Result<()> {
    let jobs = app.db.q("SELECT * FROM jobs WHERE kind = 'jira' AND state = 'pending' ORDER BY id", p![])?;
    if jobs.is_empty() {
        return Ok(());
    }
    if !app.cfg.jira_on() {
        return fail_all(app, &jobs, "Jira isn't set up in config.toml");
    }
    let (desk, jobs): (Vec<Row>, Vec<Row>) = jobs.into_iter().partition(|j| for_desk(app, j));
    if !desk.is_empty() {
        crate::jira_desk::tick(app)?;
    }
    if jobs.is_empty() {
        return Ok(());
    }
    if app.cfg.jira_via_claude() {
        return run_through_claude(app, &jobs);
    }
    let Some(creds) = credentials(app) else {
        return fail_all(app, &jobs, "there's no Jira API token (config.toml, TASKBOARD_JIRA_TOKEN or the Keychain), or set [jira] via = \"claude\"");
    };
    let conn = Conn::Rest(creds);
    for j in jobs {
        app.db.x("UPDATE jobs SET state = 'running', attempts = attempts + 1, updated_at = ? WHERE id = ?", p![now_iso(), j.id()])?;
        match run_one(app, &j, &conn) {
            Ok((key, status, extra)) => app.db.tx(|| finish_with(app, j.id(), true, key.as_deref(), status.as_deref(), None, &extra).map(|_| ()))?,
            Err(e) => {
                if e.auth {
                    app.info(format!("jira: {}; not using the API token until a restart", e.message));
                    let mut s = app.shared.lock();
                    s.jira_token_off = true;
                    s.jira_creds = None;
                }
                app.db.tx(|| finish(app, j.id(), false, None, None, Some(&e.message)).map(|_| ()))?;
                if e.auth {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn claude_busy(app: &App) -> Result<bool> {
    Ok(app.db.q1("SELECT id FROM jobs WHERE kind = 'jira' AND state = 'running' AND json_extract(target, '$.desk') IS NULL", p![])?.is_some())
}

/// The oldest job goes to headless Claude on the AI thread; the next waits until it's back.
fn run_through_claude(app: &App, jobs: &[Row]) -> Result<()> {
    if claude_busy(app)? {
        return Ok(());
    }
    if crate::proc::which(&app.cfg.claude).is_none() {
        return fail_all(app, jobs, "claude isn't installed, and Jira goes through it ([jira] via = \"claude\")");
    }
    let j = jobs[0].clone();
    app.db.x("UPDATE jobs SET state = 'running', attempts = attempts + 1, updated_at = ? WHERE id = ?", p![now_iso(), j.id()])?;
    app.queue_ai(Box::new(move |a: &App| {
        let r = match run_one(a, &j, &Conn::Claude) {
            Ok((key, status, extra)) => a.db.tx(|| finish_with(a, j.id(), true, key.as_deref(), status.as_deref(), None, &extra).map(|_| ())),
            Err(e) => a.db.tx(|| finish(a, j.id(), false, None, None, Some(&e.message)).map(|_| ())),
        };
        if let Err(e) = r {
            a.info(format!("jira {}: recording its result failed: {e}", rf("job", j.id())));
        }
        a.wake_runner();
    }));
    Ok(())
}

/// Jira's comment times ("2026-10-08T14:03:11.000+0000") as seconds.
pub fn comment_time(s: &str) -> Option<f64> {
    let s = s.trim();
    let fixed = match s.len() {
        n if n > 5 && (s.as_bytes()[n - 5] == b'+' || s.as_bytes()[n - 5] == b'-') && s[n - 4..].chars().all(|c| c.is_ascii_digit()) => {
            format!("{}:{}", &s[..n - 2], &s[n - 2..])
        }
        _ => s.to_string(),
    };
    parse_iso(&fixed)
}

fn comment_row(key: &str, id: &Value, created: &Value, author: Value, text: String) -> Value {
    json!({"key": key, "id": id.as_str().map(|s| s.to_string()).unwrap_or_else(|| id.to_string()), "created": created, "author": author, "text": text})
}

/// The one search QA makes per check: the board's tickets updated in the last `minutes`.
pub fn recent_jql(keys: &[String], minutes: i64) -> String {
    format!("key in ({}) AND updated >= -{minutes}m", keys.join(","))
}

/// What QA's one search found: the tickets' new comments and, when asked, their statuses.
#[derive(Debug, Default)]
pub struct Recent {
    /// Each as {key, id, created, author: {displayName, accountId, emailAddress}, text}.
    pub comments: Vec<Value>,
    /// Each as {key, status, status_category} (`jira_status::apply`'s input).
    pub statuses: Vec<Value>,
}

/// QA: the comments on these tickets made in the last `minutes` (when `comments`) and the statuses of
/// the ones updated then (when `statuses`): one search, then the comments of the tickets it finds;
/// through Claude, all of it in one call.
pub fn recent(app: &App, keys: &[String], minutes: i64, comments: bool, statuses: bool) -> std::result::Result<Recent, String> {
    if !app.cfg.jira_on() {
        return Err("Jira isn't set up".into());
    }
    let jql = recent_jql(keys, minutes);
    let on_board = |k: &Value| k.as_str().is_some_and(|k| keys.iter().any(|x| x.eq_ignore_ascii_case(k)));
    let mut out = Recent::default();
    if app.cfg.jira_via_claude() {
        let now = now_ts();
        let input = json!({"jql": jql, "now": iso(now), "after": iso(now - minutes as f64 * 60.0), "comments": comments, "statuses": statuses});
        let d = crate::jira_claude::run(app, "comments", &input)?;
        let after = now - minutes as f64 * 60.0;
        let asked = |c: &&Value| on_board(&c["key"]) && !c["created"].as_str().and_then(comment_time).is_some_and(|t| t < after);
        if comments {
            out.comments = d["comments"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter(asked)
                .map(|c| {
                    let author = json!({"displayName": c["author"], "accountId": c["author_account_id"], "emailAddress": c["author_email"]});
                    comment_row(c["key"].as_str().unwrap_or(""), &c["id"], &c["created"], author, c["text"].as_str().unwrap_or("").to_string())
                })
                .collect();
        }
        if statuses {
            out.statuses = d["statuses"].as_array().cloned().unwrap_or_default().into_iter().filter(|s| on_board(&s["key"])).collect();
        }
        return Ok(out);
    }
    let found = api(app, "POST", "search/jql", Some(json!({"jql": jql, "fields": ["summary", "status"], "maxResults": 100})))?;
    for issue in found["issues"].as_array().cloned().unwrap_or_default() {
        let key = issue["key"].as_str().unwrap_or("").to_string();
        if statuses {
            let st = &issue["fields"]["status"];
            out.statuses.push(json!({"key": key, "status": st["name"], "status_category": st["statusCategory"]["key"]}));
        }
        if !comments {
            continue;
        }
        let d = api(app, "GET", &format!("issue/{key}/comment?orderBy=-created&maxResults=50"), None)?;
        for cm in d["comments"].as_array().cloned().unwrap_or_default() {
            out.comments.push(comment_row(&key, &cm["id"], &cm["created"], cm["author"].clone(), adf_text(&cm["body"])));
        }
    }
    Ok(out)
}

/// QA: one comment, as {author: {displayName, accountId, emailAddress}, text}.
pub fn read_comment(app: &App, key: &str, id: &str) -> std::result::Result<Value, String> {
    if app.cfg.jira_on() && app.cfg.jira_via_claude() {
        let d = crate::jira_claude::run(app, "comment_get", &json!({"key": key, "id": id}))?;
        return Ok(json!({"author": {"displayName": d["author"], "accountId": d["author_account_id"], "emailAddress": d["author_email"]},
                         "text": d["text"].as_str().unwrap_or("")}));
    }
    let d = api(app, "GET", &format!("issue/{key}/comment/{id}"), None)?;
    Ok(json!({"author": d["author"], "text": adf_text(&d["body"])}))
}

fn describe(op: &str, key: Option<&str>, status: Option<&str>, found: bool) -> String {
    let key = key.unwrap_or("the ticket");
    match op {
        "create" if found => format!("Linked {key}{}, which already covers it", status.map(|s| format!(" ({s})")).unwrap_or_default()),
        "create" => format!("Made {key}{}", status.map(|s| format!(" ({s})")).unwrap_or_default()),
        "transition" => format!("Moved {key} to {}", status.unwrap_or("its new status")),
        "status" => format!("{key} is {}", status.unwrap_or("in an unknown status")),
        "comment" => format!("Commented on {key}"),
        "merged_note" => match status {
            Some(st) => format!("Posted the note for testers on {key} and moved it to {st}"),
            None => format!("Posted the note for testers on {key}"),
        },
        _ => format!("Jira: {op} {key}"),
    }
}

fn op_line(op: &str) -> &str {
    match op {
        "create" => "make the ticket",
        "transition" => "move the ticket",
        "status" => "read the ticket's status",
        "comment" => "comment on the ticket",
        "merged_note" => "post the note for testers and move the ticket",
        other => other,
    }
}

pub fn finish(app: &App, job_id: i64, ok: bool, key: Option<&str>, status: Option<&str>, message: Option<&str>) -> Result<Option<Row>> {
    finish_with(app, job_id, ok, key, status, message, &Value::Null)
}

/// A Jira job's result. `extra` carries what a new ticket's job learned: `found` (it was already
/// there), `product` (the one picked for it, which a goal with none takes) and, on a found ticket,
/// `added` (what it was given of the job's routing fields: a list, or a comma-separated string).
pub fn finish_with(app: &App, job_id: i64, ok: bool, key: Option<&str>, status: Option<&str>, message: Option<&str>, extra: &Value) -> Result<Option<Row>> {
    let Some(job) = app.db.q1("SELECT * FROM jobs WHERE id = ? AND kind = 'jira'", p![job_id])? else { return Ok(None) };
    if matches!(job.s("state"), Some("done") | Some("failed") | Some("expired")) && !ok {
        return Ok(Some(job));
    }
    let a = board::job_args(&job);
    let target = board::job_target(&job);
    let op = a.st("op");
    let found = extra["found"] == true;
    let key = key.map(|s| s.to_string()).or_else(|| a.s("key").map(|s| s.to_string())).filter(|k| !k.is_empty());
    let added: Vec<String> = match &extra["added"] {
        Value::Array(a) => a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Value::String(s) => s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
        _ => vec![],
    };
    let added = if found { added } else { vec![] };
    let said = |line: String| if added.is_empty() { line } else { format!("{line}, and gave it {}", added.join("; ")) };
    let result = json!({"ok": ok, "key": key, "status": status, "message": message, "found": found, "product": extra["product"], "added": added});
    app.db.x(
        "UPDATE jobs SET state = ?, result = ?, updated_at = ? WHERE id = ?",
        p![if ok { "done" } else { "failed" }, jdumps(&result), now_iso(), job_id],
    )?;
    if ok && op == "create" {
        if let Some(p) = extra["product"].as_str().filter(|p| !p.is_empty()) {
            if let Some(g) = job_goal(app, &target)?.filter(|g| !has(g.s("product"))) {
                if app.cfg.jira.products.is_empty() || app.cfg.jira.products.contains_key(p) {
                    app.db.update("goals", &json!(g.id()), fields!["product" => p, "updated_at" => now_iso()])?;
                }
            }
        }
    }
    let status = status.filter(|s| !s.is_empty()).map(|s| s.to_string());
    let why = message.unwrap_or("no reason given");
    if let Some(tid) = target.i("task") {
        if let Some(t) = board::find_task(app, Some(tid))? {
            if ok {
                let mut f = vec![];
                if let Some(k) = &key {
                    f.push(("jira_key", json!(k)));
                }
                if let Some(s) = &status {
                    f.push(("jira_status", json!(s)));
                    f.push(("jira_status_at", json!(now_iso())));
                }
                let new_key = key.is_some() && key.as_deref() != t.s("jira_key");
                if !f.is_empty() {
                    board::update_task(app, t.id(), f)?;
                    if new_key {
                        board::bump_ctx(app, t.id())?;
                    }
                }
                let who = if a.s("by") == Some("board") { board::BOARD } else { "Jira" };
                board::log_event(app, t.id(), who, "jira", &said(describe(&op, key.as_deref(), status.as_deref(), found)))?;
                if op == "create" && matches!(t.s("status"), Some("working") | Some("needs")) {
                    let st = app.cfg.jira.in_progress.clone();
                    board::jira_keep_in_step(app, &board::get_task(app, t.id())?, Some(&st), None)?;
                }
            } else {
                board::log_event(app, t.id(), board::BOARD, "jira", &format!("Couldn't {}: {why}", op_line(&op)))?;
            }
            if op == "merged_note" {
                crate::merged_note::finished(app, &board::get_task(app, t.id())?, ok, why)?;
            }
        }
    }
    if let Some(gid) = target.i("goal") {
        if ok && board::find_goal(app, Some(gid))?.is_some() {
            let mut f = fields!["updated_at" => now_iso()];
            if let Some(k) = &key {
                f.push(("epic_key", json!(k)));
            }
            if let Some(s) = &status {
                f.push(("epic_status", json!(s)));
                f.push(("jira_status_at", json!(now_iso())));
            }
            app.db.update("goals", &json!(gid), f)?;
        }
    }
    if let Some(bid) = target.i("issue") {
        if board::find_issue(app, Some(bid))?.is_some() {
            if ok {
                app.db.update("issues", &json!(bid), fields!["jira_key" => key, "state" => "ticket", "updated_at" => now_iso()])?;
                board::add_issue_event(app, bid, "Jira", "ticket", &said(describe(&op, key.as_deref(), None, found)), None)?;
            } else {
                app.db.update("issues", &json!(bid), fields!["state" => "open", "updated_at" => now_iso()])?;
                board::add_issue_event(app, bid, board::BOARD, "ticket", &format!("Couldn't make the ticket: {why}"), None)?;
            }
        }
    }
    if !ok && op == "create" {
        ticket_failed_alert(app, &target, why)?;
    }
    app.info(format!("jira {} {}", rf("job", job_id), if ok { "ok".to_string() } else { format!("failed: {why}") }));
    app.wake_runner();
    Ok(Some(job))
}

/// How to go on after a task's ticket couldn't be found or made.
pub fn retry_hint(tb: &str, task: &str) -> String {
    format!("Try again with {tb} task set {task} --jira new, or link one with {tb} task set {task} --jira KEY.")
}

/// A ticket, epic or bug ticket that couldn't be found or made raises an alert, with what to do next.
fn ticket_failed_alert(app: &App, target: &Row, why: &str) -> Result<()> {
    let why = why.trim_end_matches('.');
    if let Some(t) = board::find_task(app, target.i("task"))? {
        let n = rf("task", t.id());
        let text = format!("Couldn't make the Jira ticket for {n} “{}”: {why}. {}", t.st("title"), retry_hint("tb", &n));
        return crate::dispatch::add_alert(app, &text, Some(t.id()), t.i("goal_id"), None, None);
    }
    if let Some(b) = board::find_issue(app, target.i("issue"))? {
        let n = rf("issue", b.id());
        let text = format!("Couldn't make the Jira ticket for {n} “{}”: {why}. Try again with tb backlog ticket {n}.", b.st("title"));
        return crate::dispatch::add_alert(app, &text, None, b.i("goal_id"), None, None);
    }
    if let Some(g) = board::find_goal(app, target.i("goal"))? {
        let text = format!(
            "Couldn't find or make the Jira epic for {} “{}”: {why}. Its tickets are made without one; set it with tb goal set {} --epic KEY.",
            rf("goal", g.id()),
            g.st("name"),
            rf("goal", g.id())
        );
        return crate::dispatch::add_alert(app, &text, None, Some(g.id()), None, None);
    }
    Ok(())
}

pub fn expire(app: &App) -> Result<()> {
    for j in app.db.q("SELECT * FROM jobs WHERE kind = 'jira' AND state = 'running'", p![])? {
        let desk = board::job_target(&j).get("desk").is_some();
        let limit = if desk { crate::jira_desk::EXPIRY_SECS } else { RUNNING_EXPIRY_SECS };
        if age_secs(j.s("updated_at")).unwrap_or(0.0) > limit {
            finish(app, j.id(), false, None, None, Some(if desk { "The Jira desk didn't answer in time" } else { "Jira didn't answer in time" }))?;
            app.db.x("UPDATE jobs SET state = 'expired' WHERE id = ?", p![j.id()])?;
        }
    }
    Ok(())
}

// --- Tickets for PR work (`[jira] auto_ticket`) ---

/// The task's latest job for a new ticket.
fn last_create(app: &App, task_id: i64) -> Result<Option<Row>> {
    app.db.q1(
        "SELECT * FROM jobs WHERE kind = 'jira' AND task_id = ? AND json_extract(args, '$.op') = 'create' ORDER BY id DESC LIMIT 1",
        p![task_id],
    )
}

/// A new ticket for the task is being found or made.
pub fn ticket_asked(app: &App, task_id: i64) -> Result<bool> {
    Ok(asked(last_create(app, task_id)?.as_ref()))
}

fn asked(j: Option<&Row>) -> bool {
    matches!(j.and_then(|j| j.s("state")), Some("pending") | Some("running"))
}

fn failure(j: Option<&Row>) -> Option<String> {
    let j = j.filter(|j| matches!(j.s("state"), Some("failed") | Some("expired")))?;
    Some(jloads_obj(j.s("result")).s("message").filter(|m| !m.is_empty()).unwrap_or("no reason given").to_string())
}

/// With `auto_ticket` on, a queued or working task in a project that ships PRs gets a ticket. A human
/// task never has a PR, so it gets one only when asked for with `--jira new` or a key.
pub fn wants_ticket(app: &App, t: &Row) -> Result<bool> {
    if !app.cfg.jira_on() || !app.cfg.jira.auto_ticket || has(t.s("jira_key")) || t.b("jira_none") || crate::human::is(t) {
        return Ok(false);
    }
    if !matches!(t.s("status"), Some("queued") | Some("working") | Some("needs")) {
        return Ok(false);
    }
    crate::projects::ships_prs(app, &t.st("project"))
}

/// Asks for a ticket for each task that wants one and hasn't asked yet. One that failed waits for
/// `tb task set T<n> --jira new` (try again) or `--jira KEY` (link one).
pub fn ensure_tickets(app: &App) -> Result<()> {
    if !app.cfg.jira_on() || !app.cfg.jira.auto_ticket {
        return Ok(());
    }
    let rows = app.db.q(
        "SELECT * FROM tasks WHERE status IN ('queued', 'working', 'needs') AND COALESCE(jira_key, '') = '' AND COALESCE(jira_none, 0) = 0",
        p![],
    )?;
    for t in rows {
        if !wants_ticket(app, &t)? || last_create(app, t.id())?.is_some() {
            continue;
        }
        request_create_by_board(app, &t)?;
        board::log_event(app, t.id(), board::BOARD, "jira", "Asked for a Jira ticket for its PR")?;
    }
    Ok(())
}

/// A task waits for its ticket, so its branch and commits can carry the key: while one is being
/// found or made, and (with `auto_ticket`) until it has one.
pub fn ticket_blocker(app: &App, t: &Row) -> Result<bool> {
    if has(t.s("jira_key")) || t.b("jira_none") {
        return Ok(false);
    }
    Ok(asked(last_create(app, t.id())?.as_ref()) || wants_ticket(app, t)?)
}

/// Why a queued task waits for its ticket, for the board and `tb goal show`.
pub fn ticket_wait(app: &App, t: &Row) -> Result<Option<String>> {
    if !ticket_blocker(app, t)? {
        return Ok(None);
    }
    let last = last_create(app, t.id())?;
    Ok(Some(match failure(last.as_ref()) {
        Some(why) if !asked(last.as_ref()) => {
            format!("Waits for its Jira ticket. Couldn't make it: {}. {}", why.trim_end_matches('.'), retry_hint("tb", &rf("task", t.id())))
        }
        _ if crate::jira_desk::on(app) => "Waits for its Jira ticket from the Jira desk".to_string(),
        _ => "Waits for its Jira ticket".to_string(),
    }))
}

/// A task's Jira row: its ticket, or where its ticket is.
pub fn card(app: &App, t: &Row) -> Result<Value> {
    if let Some(k) = t.s("jira_key").filter(|k| !k.is_empty()) {
        return Ok(json!({"key": k, "status": t.v("jira_status"), "url": board::jira_url(app, k)}));
    }
    let last = last_create(app, t.id())?;
    let desk = crate::jira_desk::on(app);
    if asked(last.as_ref()) {
        let status = if desk { "Ticket asked for · Jira desk" } else { "Ticket asked for" };
        return Ok(json!({"key": null, "status": status, "url": null, "desk": desk}));
    }
    let failed = failure(last.as_ref()).filter(|_| t.s("status") != Some("done") && !t.b("jira_none"));
    if let Some(why) = failed {
        return Ok(json!({"key": null, "status": format!("Couldn't make the ticket: {why}"), "url": null, "desk": desk, "failed": true}));
    }
    if wants_ticket(app, t)? {
        let status = if desk { "No ticket yet. The Jira desk finds or makes one…" } else { "No ticket yet" };
        return Ok(json!({"key": null, "status": status, "url": null, "desk": desk}));
    }
    Ok(Value::Null)
}

/// The handoff's Jira lines, while Jira is on and the project ships PRs.
pub fn handoff_block(app: &App, t: &Row) -> Result<Vec<String>> {
    if !app.cfg.jira_on() || !crate::projects::task_ships_pr(app, t)? {
        return Ok(vec![]);
    }
    let tb = board::tb_cmd(app);
    let n = rf("task", t.id());
    let mut lines = vec![];
    if !has(t.s("jira_key")) {
        if app.cfg.jira.auto_ticket && !t.b("jira_none") {
            lines.push(format!("Jira: this task's PR needs a ticket, and the board is getting one{}; it's in your context once it's linked.", if crate::jira_desk::on(app) { " from the Jira desk" } else { "" }));
        } else {
            lines.push("Jira: this task has no ticket.".into());
        }
        lines.push(format!(
            "If you know its ticket, link it: {tb} task set {n} --jira KEY. For a new one: {tb} task set {n} --jira new. Find or make it \
             yourself; never ask {} which ticket. Settle the ticket and the branch before you commit.",
            app.cfg.owner
        ));
    }
    let mut out = if lines.is_empty() { lines } else { vec![lines.join(" ")] };
    out.extend(crate::merged_note::handoff_line(app, t)?);
    Ok(out)
}

pub mod base64_lite {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(input: &[u8]) -> String {
        let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
        for chunk in input.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(T[(n >> 18) as usize & 63] as char);
            out.push(T[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
            out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
        }
        out
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn encodes() {
            assert_eq!(super::encode(b"a@b.c:tok"), "YUBiLmM6dG9r");
            assert_eq!(super::encode(b"ab"), "YWI=");
        }
    }
}
