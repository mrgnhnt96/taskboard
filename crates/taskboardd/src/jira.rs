//! Jira through its REST API (optional): make tickets, move them, comment, read their status.
//! Jobs queue in the database and run one at a time on the runner thread, outside any transaction.

use base64_lite::encode as b64;
use serde_json::{json, Value};

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

/// A task whose ticket is still being made waits for it, so its branch and commits can carry the key.
pub fn ticket_blocker(app: &App, t: &Row) -> Result<bool> {
    if has(t.s("jira_key")) {
        return Ok(false);
    }
    Ok(app
        .db
        .q1(
            "SELECT id FROM jobs WHERE kind = 'jira' AND task_id = ? AND state IN ('pending', 'running') \
             AND json_extract(args, '$.op') = 'create'",
            p![t.id()],
        )?
        .is_some())
}

fn job_goal(app: &App, target: &Row) -> Result<Option<Row>> {
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

fn status_of(app: &App, c: &(String, String), key: &str) -> std::result::Result<String, JiraError> {
    let d = call(app, c, "GET", &format!("issue/{key}?fields=status"), None)?;
    Ok(d["fields"]["status"]["name"].as_str().unwrap_or("").to_string())
}

fn transition(app: &App, c: &(String, String), key: &str, target: &str) -> std::result::Result<String, JiraError> {
    let now = status_of(app, c, key)?;
    if now.eq_ignore_ascii_case(target) {
        return Ok(now);
    }
    let ts = call(app, c, "GET", &format!("issue/{key}/transitions"), None)?;
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
        return Err(JiraError { message: format!("{key} can't go from {now} to {target} (it can go to {can})"), auth: false });
    };
    call(app, c, "POST", &format!("issue/{key}/transitions"), Some(json!({"transition": {"id": t["id"]}})))?;
    Ok(t["to"]["name"].as_str().unwrap_or(target).to_string())
}

fn create(app: &App, c: &(String, String), a: &Row, g: Option<&Row>) -> std::result::Result<(String, String), JiraError> {
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
    let product = g.and_then(|g| g.s("product")).map(|s| s.to_string()).or_else(|| a.s("product").map(|s| s.to_string()));
    if let Some(p) = product.as_deref().and_then(|p| j.products.get(p)) {
        labels.extend(p.labels.iter().cloned());
        if !p.components.is_empty() {
            f["components"] = json!(p.components.iter().map(|c| json!({"name": c})).collect::<Vec<_>>());
        }
        for (k, v) in &p.fields {
            f[k] = serde_json::to_value(v).unwrap_or(Value::Null);
        }
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
    let d = call(app, c, "POST", "issue", Some(json!({"fields": f})))?;
    let key = d["key"].as_str().unwrap_or("").to_string();
    if key.is_empty() {
        return Err(JiraError { message: "Jira made no ticket".into(), auth: false });
    }
    let status = status_of(app, c, &key).unwrap_or_default();
    Ok((key, status))
}

fn run_one(app: &App, job: &Row, c: &(String, String)) -> std::result::Result<(Option<String>, Option<String>), JiraError> {
    let a = board::job_args(job);
    let target = board::job_target(job);
    let key = a.st("key");
    match a.st("op").as_str() {
        "create" => {
            let g = job_goal(app, &target).map_err(|e| JiraError { message: e.message, auth: false })?;
            let (k, s) = create(app, c, &a, g.as_ref())?;
            Ok((Some(k), Some(s)))
        }
        _ if key.is_empty() => Err(JiraError { message: "the job has no ticket key".into(), auth: false }),
        "transition" => {
            let st = transition(app, c, &key, &a.st("status"))?;
            if has(a.s("comment")) {
                call(app, c, "POST", &format!("issue/{key}/comment"), Some(json!({"body": adf(&a.st("comment"))})))?;
            }
            Ok((Some(key), Some(st)))
        }
        "status" => {
            let st = status_of(app, c, &key)?;
            Ok((Some(key), Some(st)))
        }
        "comment" => {
            call(app, c, "POST", &format!("issue/{key}/comment"), Some(json!({"body": adf(&a.st("comment"))})))?;
            Ok((Some(key), None))
        }
        op => Err(JiraError { message: format!("the board doesn't know the Jira job {op}"), auth: false }),
    }
}

/// Runs the pending Jira jobs, oldest first.
pub fn run_pending(app: &App) -> Result<()> {
    let jobs = app.db.q("SELECT * FROM jobs WHERE kind = 'jira' AND state = 'pending' ORDER BY id", p![])?;
    if jobs.is_empty() {
        return Ok(());
    }
    if !app.cfg.jira_on() {
        app.db.tx(|| {
            for j in &jobs {
                finish(app, j.id(), false, None, None, Some("Jira isn't set up in config.toml"))?;
            }
            Ok(())
        })?;
        return Ok(());
    }
    let Some(creds) = credentials(app) else {
        app.db.tx(|| {
            for j in &jobs {
                finish(app, j.id(), false, None, None, Some("there's no Jira API token (config.toml, TASKBOARD_JIRA_TOKEN or the Keychain)"))?;
            }
            Ok(())
        })?;
        return Ok(());
    };
    for j in jobs {
        app.db.x("UPDATE jobs SET state = 'running', attempts = attempts + 1, updated_at = ? WHERE id = ?", p![now_iso(), j.id()])?;
        let out = run_one(app, &j, &creds);
        match out {
            Ok((key, status)) => app.db.tx(|| finish(app, j.id(), true, key.as_deref(), status.as_deref(), None).map(|_| ()))?,
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

fn describe(op: &str, key: Option<&str>, status: Option<&str>) -> String {
    let key = key.unwrap_or("the ticket");
    match op {
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
    let Some(job) = app.db.q1("SELECT * FROM jobs WHERE id = ? AND kind = 'jira'", p![job_id])? else { return Ok(None) };
    if matches!(job.s("state"), Some("done") | Some("failed") | Some("expired")) && !ok {
        return Ok(Some(job));
    }
    let a = board::job_args(&job);
    let target = board::job_target(&job);
    let op = a.st("op");
    let key = key.map(|s| s.to_string()).or_else(|| a.s("key").map(|s| s.to_string())).filter(|k| !k.is_empty());
    let result = json!({"ok": ok, "key": key, "status": status, "message": message});
    app.db.x(
        "UPDATE jobs SET state = ?, result = ?, updated_at = ? WHERE id = ?",
        p![if ok { "done" } else { "failed" }, jdumps(&result), now_iso(), job_id],
    )?;
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
                board::log_event(app, t.id(), "Jira", "jira", &describe(&op, key.as_deref(), status.as_deref()))?;
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
                board::add_issue_event(app, bid, "Jira", "ticket", &describe(&op, key.as_deref(), None), None)?;
            } else {
                app.db.update("issues", &json!(bid), fields!["state" => "open", "updated_at" => now_iso()])?;
                board::add_issue_event(app, bid, board::BOARD, "ticket", &format!("Couldn't make the ticket: {why}"), None)?;
            }
        }
    }
    app.info(format!("jira {} {}", rf("job", job_id), if ok { "ok".to_string() } else { format!("failed: {why}") }));
    Ok(Some(job))
}

pub fn expire(app: &App) -> Result<()> {
    for j in app.db.q("SELECT * FROM jobs WHERE kind = 'jira' AND state = 'running'", p![])? {
        if age_secs(j.s("updated_at")).unwrap_or(0.0) > RUNNING_EXPIRY_SECS {
            finish(app, j.id(), false, None, None, Some("Jira didn't answer in time"))?;
            app.db.x("UPDATE jobs SET state = 'expired' WHERE id = ?", p![j.id()])?;
        }
    }
    Ok(())
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
