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
    if has(g.s("epic_key")) || epic_asked(app, g.id())? {
        return Ok(());
    }
    request_epic(app, g)?;
    app.info(format!("jira: asked for an epic for {} before its first ticket", rf("goal", g.id())));
    Ok(())
}

pub fn request_create_for_task(app: &App, t: &Row) -> Result<i64> {
    let g = board::find_goal(app, t.i("goal_id"))?;
    ensure_epic(app, g.as_ref())?;
    request_job(
        app,
        json!({"op": "create", "summary": t.v("title"), "description": t.v("detail"), "issue_type": app.cfg.jira.task_type}),
        json!({"task": t.id()}),
    )
}

pub fn request_create_for_issue(app: &App, b: &Row) -> Result<i64> {
    let g = board::find_goal(app, b.i("goal_id"))?;
    ensure_epic(app, g.as_ref())?;
    let mut desc = b.st("detail");
    let snap = crate::handoff::snapshot_line(&jloads_obj(b.s("snapshot")));
    if !snap.is_empty() {
        desc += &format!("\n\nWhat was happening when it was found: {snap}");
    }
    request_job(
        app,
        json!({"op": "create", "summary": b.v("title"), "description": desc, "issue_type": app.cfg.jira.bug_type}),
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

fn words(text: &str) -> std::collections::BTreeSet<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 3).map(|w| w.to_string()).collect()
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
        f["description"] = adf(&desc);
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
    let mut lines = vec![
        format!("Find or make the Jira {} for this work in project {}.", if epic { "epic" } else { "ticket" }, j.project.trim()),
        format!(
            "First search {} for an open {} (not done) that already covers it, by its summary and description. If one does, \
             use it: don't make another and don't change it.",
            j.project.trim(),
            if epic { "epic" } else { "ticket" }
        ),
        format!(
            "Only if none does, make a {issue_type} with this summary and description{}",
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

/// An open ticket of the type whose summary matches, when there is one (REST).
fn find_open(app: &App, cr: &(String, String), f: &Value) -> std::result::Result<Option<String>, JiraError> {
    let summary = f["summary"].as_str().unwrap_or("");
    let text: Vec<String> = words(summary).into_iter().collect();
    if text.is_empty() {
        return Ok(None);
    }
    let jql = format!(
        "project = \"{}\" AND issuetype = \"{}\" AND statusCategory != Done AND summary ~ \"{}\" ORDER BY created DESC",
        app.cfg.jira.project.trim().replace('"', ""),
        f["issuetype"]["name"].as_str().unwrap_or("").replace('"', ""),
        text.join(" ")
    );
    let d = call(app, cr, "POST", "search/jql", Some(json!({"jql": jql, "fields": ["summary"], "maxResults": 20})))?;
    let norm = |s: &str| s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    Ok(d["issues"]
        .as_array()
        .and_then(|a| a.iter().find(|i| norm(i["fields"]["summary"].as_str().unwrap_or("")) == norm(summary)))
        .and_then(|i| i["key"].as_str().map(|k| k.to_string())))
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
            return Ok((key, d["status"].as_str().unwrap_or("").to_string(), json!({"found": d["found"] == true, "product": product})));
        }
    };
    let (f, product) = ticket_fields(app, a, g);
    let product = json!(product);
    // A failed search only means the board makes the ticket.
    if let Ok(Some(key)) = find_open(app, cr, &f) {
        let status = status_of(app, c, &key).unwrap_or_default();
        return Ok((key, status, json!({"found": true, "product": product})));
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
        op => Err(JiraError::new(format!("the board doesn't know the Jira job {op}"))),
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

/// QA: the comments on these tickets made in the last `minutes`, each as {key, id, created, author:
/// {displayName, accountId, emailAddress}, text}.
pub fn recent_comments(app: &App, keys: &[String], minutes: i64) -> std::result::Result<Vec<Value>, String> {
    if !app.cfg.jira_on() {
        return Err("Jira isn't set up".into());
    }
    if app.cfg.jira_via_claude() {
        let d = crate::jira_claude::run(app, "comments", &json!({"keys": keys.join(", "), "minutes": minutes}))?;
        return Ok(d["comments"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|c| {
                let author = json!({"displayName": c["author"], "accountId": c["author_account_id"], "emailAddress": c["author_email"]});
                comment_row(c["key"].as_str().unwrap_or(""), &c["id"], &c["created"], author, c["text"].as_str().unwrap_or("").to_string())
            })
            .collect());
    }
    let jql = format!("key in ({}) AND updated >= -{minutes}m", keys.join(","));
    let found = api(app, "POST", "search/jql", Some(json!({"jql": jql, "fields": ["summary"], "maxResults": 100})))?;
    let mut out = vec![];
    for issue in found["issues"].as_array().cloned().unwrap_or_default() {
        let key = issue["key"].as_str().unwrap_or("").to_string();
        let d = api(app, "GET", &format!("issue/{key}/comment?orderBy=-created&maxResults=50"), None)?;
        for cm in d["comments"].as_array().cloned().unwrap_or_default() {
            out.push(comment_row(&key, &cm["id"], &cm["created"], cm["author"].clone(), adf_text(&cm["body"])));
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
        _ => format!("Jira: {op} {key}"),
    }
}

fn op_line(op: &str) -> &str {
    match op {
        "create" => "make the ticket",
        "transition" => "move the ticket",
        "status" => "read the ticket's status",
        "comment" => "comment on the ticket",
        other => other,
    }
}

pub fn finish(app: &App, job_id: i64, ok: bool, key: Option<&str>, status: Option<&str>, message: Option<&str>) -> Result<Option<Row>> {
    finish_with(app, job_id, ok, key, status, message, &Value::Null)
}

/// A Jira job's result. `extra` carries what a new ticket's job learned: `found` (it was already
/// there) and `product` (the one picked for it, which a goal with none takes).
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
    let result = json!({"ok": ok, "key": key, "status": status, "message": message, "found": found, "product": extra["product"]});
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
                }
                let new_key = key.is_some() && key.as_deref() != t.s("jira_key");
                if !f.is_empty() {
                    board::update_task(app, t.id(), f)?;
                    if new_key {
                        board::bump_ctx(app, t.id())?;
                    }
                }
                board::log_event(app, t.id(), "Jira", "jira", &describe(&op, key.as_deref(), status.as_deref(), found))?;
                if op == "create" && matches!(t.s("status"), Some("working") | Some("needs")) {
                    let st = app.cfg.jira.in_progress.clone();
                    board::jira_keep_in_step(app, &board::get_task(app, t.id())?, Some(&st), None)?;
                }
            } else {
                board::log_event(app, t.id(), board::BOARD, "jira", &format!("Couldn't {}: {why}", op_line(&op)))?;
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
            }
            app.db.update("goals", &json!(gid), f)?;
        }
    }
    if let Some(bid) = target.i("issue") {
        if board::find_issue(app, Some(bid))?.is_some() {
            if ok {
                app.db.update("issues", &json!(bid), fields!["jira_key" => key, "state" => "ticket", "updated_at" => now_iso()])?;
                board::add_issue_event(app, bid, "Jira", "ticket", &describe(&op, key.as_deref(), None, found), None)?;
            } else {
                app.db.update("issues", &json!(bid), fields!["state" => "open", "updated_at" => now_iso()])?;
                board::add_issue_event(app, bid, board::BOARD, "ticket", &format!("Couldn't make the ticket: {why}"), None)?;
            }
        }
    }
    app.info(format!("jira {} {}", rf("job", job_id), if ok { "ok".to_string() } else { format!("failed: {why}") }));
    app.wake_runner();
    Ok(Some(job))
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

/// With `auto_ticket` on, a queued or working task in a project that ships PRs gets a ticket.
pub fn wants_ticket(app: &App, t: &Row) -> Result<bool> {
    if !app.cfg.jira_on() || !app.cfg.jira.auto_ticket || has(t.s("jira_key")) || t.b("jira_none") {
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
        request_create_for_task(app, &t)?;
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
        Some(why) if !asked(last.as_ref()) => format!("Waits for its Jira ticket. Couldn't make it: {why}"),
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
    if !app.cfg.jira_on() || !crate::projects::ships_prs(app, &t.st("project"))? {
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
    Ok(if lines.is_empty() { lines } else { vec![lines.join(" ")] })
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
