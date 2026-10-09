//! Operations shared by the HTTP API and `tb` reports: making tasks and goals, and the detail views.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, handoff, hooks, jira, jobs, p, projects, shared, transcript, waitsfor};

fn merge(mut v: Value, extra: Value) -> Value {
    if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
        for (k, x) in e {
            o.insert(k, x);
        }
    }
    v
}

pub fn pickup(body: &Value) -> Result<(String, Option<String>)> {
    let p = match body.get("pickup") {
        Some(Value::String(s)) => json!({"mode": s}),
        Some(Value::Object(o)) => Value::Object(o.clone()),
        _ => json!({}),
    };
    let mode = p["mode"].as_str().filter(|s| !s.is_empty()).unwrap_or("queue").to_string();
    if !["queue", "attach", "new", "manual"].contains(&mode.as_str()) {
        return err(400, "Pickup must be queue, attach, new or manual.");
    }
    let sid = p["session_id"].as_str().map(|s| s.to_string());
    if mode == "attach" && sid.is_none() {
        return err(400, "Pick a Midna terminal to attach.");
    }
    Ok((mode, sid))
}

static JIRA_KEY: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Z][A-Z0-9]+-\d+$").unwrap());

pub fn jira_key(v: &str) -> Result<String> {
    let k = v.trim().to_uppercase();
    if !JIRA_KEY.is_match(&k) {
        return err(400, "Give a Jira key like PROJ-123.");
    }
    Ok(k)
}

pub fn opt_goal(body: &Value, key: &str) -> Result<Option<i64>> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() || s.trim() == "none" => Ok(None),
        Some(v) => parse_ref(v, "goal"),
    }
}

/// Where a task came from, as the task panel's **From** row shows it: `{from, by, url?}`.
pub fn origin(o: Option<&Value>, who: &str) -> Value {
    let o = o.filter(|o| o.is_object());
    let get = |k: &str| o.and_then(|o| o[k].as_str()).filter(|s| !s.trim().is_empty()).map(|s| s.to_string());
    let from = get("from").unwrap_or_else(|| if who == board::OWNER { "Added on the board".into() } else { "Added by an agent".into() });
    let mut out = json!({"from": one_line(&from, 200), "by": one_line(&get("by").unwrap_or_else(|| who.to_string()), 120)});
    if let Some(url) = get("url").filter(|u| u.starts_with("http://") || u.starts_with("https://")) {
        out["url"] = json!(clip(&url, 500));
    }
    out
}

/// A task's wave: a number from 1, or none.
pub fn wave_of(v: &Value) -> Result<Option<i64>> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) if s.trim().is_empty() || s.trim().eq_ignore_ascii_case("none") => Ok(None),
        _ => v
            .as_i64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .filter(|n| *n >= 1)
            .map(Some)
            .ok_or_else(|| ApiError::new(400, "A wave is a number from 1, or none.")),
    }
}

pub fn new_task(app: &App, body: &Value, who: &str, log_text: Option<&str>) -> Result<Value> {
    let title = clip(&body_str(body, "title"), 300);
    if title.is_empty() {
        return err(400, "The title can't be empty.");
    }
    let goal_id = opt_goal(body, "goal_id")?;
    let g = match goal_id {
        Some(id) => Some(board::get_goal(app, id)?),
        None => None,
    };
    let mut project = body_str(body, "project");
    if project.is_empty() {
        project = g.as_ref().map(|g| g.st("project")).unwrap_or_default();
    }
    if project.is_empty() {
        return err(400, "Pick a project.");
    }
    let priority = if body["priority"] == "high" { "high" } else { "normal" };
    let (mode, sid) = pickup(body)?;
    let status = if body["status"] == "planned" { "planned" } else { "queued" };
    let auto_close = as_bool(body.get("auto_close"), g.as_ref().map(|g| g.b("auto_close")).unwrap_or(true));
    let jopt = match body.get("jira") {
        Some(Value::String(s)) => json!({"mode": s}),
        Some(Value::Object(o)) => Value::Object(o.clone()),
        _ => json!({}),
    };
    let mut jmode = jopt["mode"].as_str().unwrap_or("none").to_string();
    if !app.cfg.jira_on() {
        jmode = "none".into();
    }
    let jkey = if jmode == "link" { Some(jira_key(jopt["key"].as_str().unwrap_or(""))?) } else { None };
    let now = now_iso();
    let repo = g
        .as_ref()
        .filter(|g| g.s("project") == Some(project.as_str()))
        .and_then(|g| g.s("repo_path").filter(|r| !r.is_empty()).map(|r| r.to_string()))
        .or(projects::project_path(app, Some(&project))?);
    let latest = one_line(&body_str(body, "latest"), 240);
    let lock_names = crate::locks::clean(body.get("locks"))?;
    let lock_warning = crate::locks::unseen_warning(app, lock_names.as_deref(), None)?;
    let alone = crate::locks::clean_alone(body.get("alone"))?;
    let tid = app.db.insert(
        "tasks",
        fields!["title" => title, "detail" => body_str(body, "detail"), "project" => project, "repo_path" => repo,
                "priority" => priority, "status" => status, "goal_id" => goal_id, "position" => board::next_position(app, goal_id)?,
                "pickup" => mode, "pickup_session" => if mode == "attach" { sid } else { None },
                "auto_close" => auto_close as i64, "jira_key" => jkey, "jira_sync" => as_bool(body.get("jira_sync"), true) as i64,
                "jira_none" => (jopt["mode"] == "none") as i64,
                "from_issue_id" => body.get("from_issue_id"), "context" => "{}",
                "meta" => jdumps(&body.get("meta").cloned().filter(|m| m.is_array()).unwrap_or(json!([]))),
                "ctx_version" => 1, "created_at" => now, "updated_at" => now,
                "origin" => jdumps(&origin(body.get("origin"), who)), "locks" => lock_names, "alone" => alone,
                "latest" => if latest.is_empty() { None } else { Some(latest) }],
    )?;
    if let Some(w) = body.get("wave").filter(|w| !w.is_null()) {
        let w = wave_of(w)?;
        if goal_id.is_none() && w.is_some() {
            return err(400, "Only a task in a goal has a wave.");
        }
        board::update_task(app, tid, fields!["wave" => w])?;
    }
    if body_has(body, "waits_for") {
        let t = board::get_task(app, tid)?;
        let wf = waitsfor::clean(app, &body["waits_for"], Some(&t))?;
        board::update_task(app, tid, fields!["waits_for" => wf])?;
    }
    let mut text = log_text.map(|s| s.to_string()).unwrap_or_else(|| if status == "planned" { "Added as planned".into() } else { "Added to the queue".into() });
    if let Some(k) = &jkey {
        text += &format!(", linked {k}");
    }
    board::log_event(app, tid, who, "status", &text)?;
    let t = board::get_task(app, tid)?;
    let also = shared::clean(app, body.get("also"), Some(&t))?;
    shared::add(app, &t, &also, who)?;
    hooks::fire(app, "task.created", &t, None);
    if jmode == "create" {
        jira::request_create_for_task(app, &t)?;
    } else if let Some(k) = &jkey {
        jira::request(app, "status", tid, k, None, None)?;
    }
    app.wake_runner();
    let mut card = board::task_card(app, &t)?;
    if let Some(w) = lock_warning {
        card["warnings"] = json!([w]);
    }
    Ok(card)
}

pub fn new_goal(app: &App, body: &Value) -> Result<Value> {
    let name = clip(&body_str(body, "name"), 200);
    if name.is_empty() {
        return err(400, "The name can't be empty.");
    }
    let project = body_str(body, "project");
    if project.is_empty() {
        return err(400, "The project can't be empty.");
    }
    let epic = match body.get("epic") {
        Some(Value::String(s)) => json!({"mode": s}),
        Some(Value::Object(o)) => Value::Object(o.clone()),
        _ => json!({}),
    };
    let mut emode = epic["mode"].as_str().unwrap_or("none").to_string();
    if !app.cfg.jira_on() {
        emode = "none".into();
    }
    let ekey = if emode == "link" { Some(jira_key(epic["key"].as_str().unwrap_or(""))?) } else { None };
    let product = jira::clean_product(app, &body_str(body, "product"))?;
    let max_t = match body.get("max_terminals") {
        None | Some(Value::Null) => 2,
        Some(v) => v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).ok_or_else(|| ApiError::new(400, "At most how many terminals? Give a number."))?.max(1),
    };
    let now = now_iso();
    let tldr = body_str(body, "tldr");
    let gid = app.db.insert(
        "goals",
        fields!["name" => name, "outcome" => body_str(body, "outcome"), "tldr" => if tldr.is_empty() { None } else { Some(tldr) },
                "project" => project, "repo_path" => projects::project_path(app, Some(&project))?, "epic_key" => ekey,
                "run_in_order" => as_bool(body.get("run_in_order"), true) as i64, "max_terminals" => max_t,
                "auto_close" => as_bool(body.get("auto_close"), true) as i64, "product" => product,
                "worktree_base" => crate::worktrees::clean_base(body.get("worktree_base"))?,
                "setup" => clean_setup(body),
                "created_at" => now, "updated_at" => now, "archived" => 0],
    )?;
    let g = board::get_goal(app, gid)?;
    hooks::fire_goal(app, "goal.created", &g, None);
    if emode == "create" {
        jira::request_epic(app, &g)?;
    } else if let Some(k) = &ekey {
        jira::request_job(app, json!({"op": "status", "key": k}), json!({"goal": gid}))?;
    }
    board::goal_dict(app, &g)
}

/// A goal's setup text (`tb goal setup`): every task in the goal does it first. Empty or "none" clears it.
pub fn clean_setup(body: &Value) -> Value {
    let s = clip(body_str(body, "setup").trim(), 3000);
    if s.is_empty() || s.eq_ignore_ascii_case("none") {
        Value::Null
    } else {
        json!(s)
    }
}

pub fn task_detail(app: &App, id: i64) -> Result<Value> {
    let t = board::get_task(app, id)?;
    let d = board::task_card(app, &t)?;
    let s = board::get_session(app, t.s("session_id"))?;
    let g = board::find_goal(app, t.i("goal_id"))?;
    let gd = match &g {
        Some(g) => {
            let rows = board::goal_tasks(app, g.id())?;
            let idx = rows.iter().position(|x| x.id() == t.id());
            let next = idx.and_then(|i| rows.get(i + 1)).map(|x| x.v("title")).unwrap_or(Value::Null);
            json!({"id": g.id(), "ref": rf("goal", g.id()), "name": g.v("name"), "position": idx.map(|i| i + 1),
                   "total": rows.len(), "next_title": next,
                   "open_issues": app.db.count("SELECT COUNT(*) FROM issues WHERE goal_id = ? AND state = 'open'", p![g.id()])?,
                   "run_in_order": g.b("run_in_order"), "epic_key": g.v("epic_key")})
        }
        None => Value::Null,
    };
    let session = match (&s, t.s("session_id").filter(|x| !x.is_empty())) {
        (Some(s), _) => json!({"id": s.v("id"), "name": s.v("name"), "status": s.v("status")}),
        (None, Some(sid)) => json!({"id": sid, "name": t.v("session_name"), "status": "gone"}),
        _ => Value::Null,
    };
    let log: Vec<Value> = app
        .db
        .q("SELECT * FROM events WHERE task_id = ? ORDER BY at DESC, id DESC", p![id])?
        .iter()
        .map(|e| json!({"at": e.v("at"), "who": e.v("who"), "kind": e.v("kind"), "text": e.v("text")}))
        .collect();
    let found: Vec<Value> = app
        .db
        .q("SELECT id, title FROM issues WHERE found_by_task = ? ORDER BY created_at", p![id])?
        .iter()
        .map(|b| json!({"id": b.id(), "ref": rf("issue", b.id()), "title": b.v("title")}))
        .collect();
    let fi = board::find_issue(app, t.i("from_issue_id"))?;
    let attachable: Vec<Value> = app
        .db
        .q("SELECT * FROM sessions WHERE status != 'gone' ORDER BY name COLLATE NOCASE", p![])?
        .into_iter()
        .filter(|s| s.s("id") != t.s("session_id") && board::runs_claude(s) && s.s("project") == t.s("project"))
        .map(|s| json!({"id": s.v("id"), "name": s.s("name").map(|n| n.to_string()).unwrap_or_else(|| s.st("id").chars().take(8).collect()), "status": s.s("status").unwrap_or("idle")}))
        .collect();
    let jobs_now: Vec<Value> = app
        .db
        .q("SELECT * FROM jobs WHERE task_id = ? AND state IN ('pending','running') ORDER BY id", p![id])?
        .iter()
        .map(board::job_dict)
        .collect();
    Ok(merge(
        d,
        json!({
            "detail": t.v("detail"), "repo_path": t.v("repo_path"), "pickup": t.v("pickup"), "pickup_session": t.v("pickup_session"),
            "session": session, "terminals": board::terminals(app, &t)?, "claude_session_id": t.v("claude_session_id"),
            "auto_close": t.b("auto_close"), "jira_sync": t.b("jira_sync"),
            "meta": Value::Array(jloads_arr(t.s("meta"))), "context": Value::Object(board::task_context(&t)),
            "ctx_version": t.v("ctx_version"), "created_at": t.v("created_at"), "updated_at": t.v("updated_at"),
            "started_at": t.v("started_at"), "finished_at": t.v("finished_at"),
            "origin": Some(jloads_obj(t.s("origin"))).filter(|o| !o.is_empty()).map(Value::Object).unwrap_or(Value::Null),
            "log": log, "handoff": handoff::build(app, id)?, "goal": gd,
            "blocked_by": waitsfor::blocked_by(app, &t)?, "attachable": attachable, "found": found,
            "from_issue": fi.map(|b| json!({"id": b.id(), "ref": rf("issue", b.id()), "title": b.v("title")})).unwrap_or(Value::Null),
            "jobs": jobs_now,
            "attachments": board::attachments(app, Some(id), None)?,
            "goal_attachments": match &g { Some(g) => Value::Array(board::attachments(app, None, Some(g.id()))?), None => json!([]) },
        }),
    ))
}

pub fn goal_detail(app: &App, id: i64) -> Result<Value> {
    let g = board::get_goal(app, id)?;
    let d = board::goal_dict(app, &g)?;
    let mut tasks = vec![];
    for (i, t) in board::goal_tasks(app, id)?.iter().enumerate() {
        let mut c = board::task_card(app, t)?;
        c["n"] = json!(i + 1);
        c["detail"] = t.v("detail");
        tasks.push(c);
    }
    let mut backlog = vec![];
    for b in app.db.q("SELECT * FROM issues WHERE goal_id = ? AND state != 'drop' ORDER BY created_at DESC, id DESC", p![id])? {
        backlog.push(board::issue_dict(app, &b)?);
    }
    let rows = board::goal_tasks(app, id)?;
    let waves = if crate::waves::uses_waves(&rows) { crate::waves::waves(app, &g, &rows)? } else { vec![] };
    let mut shared_cards = vec![];
    for t in shared::tasks_for(app, id)? {
        shared_cards.push(board::task_card(app, &t)?);
    }
    Ok(merge(
        d,
        json!({
            "tasks": tasks,
            "shared": shared_cards,
            "waves": waves,
            "qa": crate::qa::for_goal(app, id)?,
            "notes": board::goal_notes(app, id)?.iter().map(board::goal_note_dict).collect::<Vec<_>>(),
            "backlog": backlog,
            "attachments": board::attachments(app, None, Some(id))?,
        }),
    ))
}

pub fn issue_detail(app: &App, id: i64) -> Result<Value> {
    let b = board::get_issue(app, id)?;
    let mut d = board::issue_dict(app, &b)?;
    d["history"] = Value::Array(board::issue_history(app, id)?);
    Ok(d)
}

fn session_visible(s: &Row) -> bool {
    s.s("status") != Some("gone") && s.st("agent").to_lowercase().contains("claude")
}

fn project_match(project: Option<&str>, want: &str) -> bool {
    want.is_empty() || want == "all" || project == Some(want)
}

/// How long a terminal has been idle, counting only the time the Mac was awake.
fn idle_secs(s: &Row) -> Value {
    age_secs(s.s("last_activity").or(s.s("seen_at"))).map(|a| json!(a.max(0.0).round() as i64)).unwrap_or(Value::Null)
}

pub fn session_list(app: &App, project: &str) -> Result<Vec<Value>> {
    let mut tasks: std::collections::HashMap<String, Row> = std::collections::HashMap::new();
    for t in app.db.q("SELECT * FROM tasks WHERE session_id IS NOT NULL AND status != 'done' ORDER BY updated_at", p![])? {
        tasks.insert(t.st("session_id"), t);
    }
    let mut out = vec![];
    for s in app.db.q("SELECT * FROM sessions ORDER BY name COLLATE NOCASE, id", p![])? {
        if !session_visible(&s) || !project_match(s.s("project"), project) {
            continue;
        }
        let sid = s.st("id");
        let t = tasks.get(&sid);
        let desk = t.is_none() && crate::jira_desk::is_desk(app, Some(&sid))?;
        let mut row = json!({
            "id": sid, "name": s.s("name").filter(|n| !n.is_empty()).map(|n| n.to_string()).unwrap_or_else(|| format!("Terminal {}", sid.chars().take(8).collect::<String>())),
            "project": s.v("project"), "project_path": s.v("project_path"), "status": board::shown_status(&s),
            "api_error": s.v("api_error"), "idle_secs": idle_secs(&s),
            "task_ref": t.map(|t| json!(rf("task", t.id()))).unwrap_or(Value::Null),
            "task_title": t.map(|t| t.v("title")).unwrap_or(Value::Null),
            "task_id": t.map(|t| json!(t.id())).unwrap_or(Value::Null),
            "last_activity": s.v("last_activity"), "seen_at": s.v("seen_at"),
            "can_take": board::shown_status(&s) == "idle" && t.is_none(),
            "closing": jobs::closing(app, &sid)?, "close": board::close_rule(Some(&s)),
            "branch": s.v("branch"), "dirty": s.v("dirty"), "renaming": null, "rename_error": null,
        });
        if desk {
            // The desk is never closed: the board sends it every Jira job.
            row["role"] = json!(crate::jira_desk::ROLE);
            row["close"] = Value::Null;
            row["can_take"] = json!(false);
        }
        for (k, v) in jobs::rename_state(app, &sid)? {
            row[k] = v;
        }
        out.push(row);
    }
    Ok(out)
}

pub fn closed_session_list(app: &App, project: &str, q: &str, limit: usize) -> Result<(Vec<Value>, usize)> {
    let q = q.trim().to_lowercase();
    let mut by_session: std::collections::HashMap<String, Row> = std::collections::HashMap::new();
    for t in app.db.q("SELECT * FROM tasks WHERE session_id IS NOT NULL ORDER BY updated_at", p![])? {
        by_session.insert(t.st("session_id"), t);
    }
    let mut rows = vec![];
    for s in app.db.q("SELECT * FROM sessions WHERE status = 'gone' ORDER BY gone_at DESC, seen_at DESC, id", p![])? {
        if !s.st("agent").to_lowercase().contains("claude") || !project_match(s.s("project"), project) {
            continue;
        }
        let t = match s.i("last_task") {
            Some(id) => board::find_task(app, Some(id))?,
            None => by_session.get(&s.st("id")).cloned(),
        };
        let sid = s.st("id");
        let name = s.s("name").filter(|n| !n.is_empty()).map(|n| n.to_string()).unwrap_or_else(|| format!("Terminal {}", sid.chars().take(8).collect::<String>()));
        if !q.is_empty() {
            let hay = [Some(name.clone()), s.s("project").map(|x| x.to_string()), t.as_ref().map(|t| t.st("title")), t.as_ref().map(|t| rf("task", t.id()))];
            if !hay.iter().flatten().any(|h| h.to_lowercase().contains(&q)) {
                continue;
            }
        }
        rows.push(json!({
            "id": sid, "name": name, "project": s.v("project"), "project_path": s.v("project_path"),
            "closed_at": s.s("gone_at").or(s.s("seen_at")), "last_activity": s.v("last_activity"),
            "claude_session_id": s.v("claude_session_id"),
            "task_ref": t.as_ref().map(|t| json!(rf("task", t.id()))).unwrap_or(Value::Null),
            "task_title": t.as_ref().map(|t| t.v("title")).unwrap_or(Value::Null),
            "task_status": t.as_ref().map(|t| t.v("status")).unwrap_or(Value::Null),
            "can_reopen": has(s.s("claude_session_id")) && s.s("project_path").map(|p| std::path::Path::new(p).is_dir()).unwrap_or(false),
        }));
    }
    let total = rows.len();
    rows.truncate(limit);
    Ok((rows, total))
}

static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"</?[a-zA-Z][\w-]*(?:\s[^<>]*)?>").unwrap());

fn strip_tags(text: &str) -> String {
    one_line(&TAG_RE.replace_all(text, " "), 600)
}

const GIT_EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

fn git(path: &str, args: &[&str]) -> Option<String> {
    let mut all = vec!["-C", path];
    all.extend_from_slice(args);
    let o = std::process::Command::new("git").args(&all).output().ok()?;
    if o.status.success() {
        Some(String::from_utf8_lossy(&o.stdout).into_owned())
    } else {
        None
    }
}

fn diffstat(path: Option<&str>) -> Value {
    let Some(path) = path.filter(|p| std::path::Path::new(p).is_dir()) else { return Value::Null };
    if git(path, &["rev-parse", "--is-inside-work-tree"]).is_none() {
        return Value::Null;
    }
    let base = if git(path, &["rev-parse", "--verify", "-q", "HEAD"]).is_some() { "HEAD" } else { GIT_EMPTY_TREE };
    let num = git(path, &["diff", base, "--numstat", "--no-renames"]).unwrap_or_default();
    let untracked: Vec<String> = git(path, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(|l| l.to_string()).collect();
    let (mut files, mut added, mut removed) = (0i64, 0i64, 0i64);
    for line in num.lines() {
        let mut it = line.split('\t');
        files += 1;
        added += it.next().and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
        removed += it.next().and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
    }
    for f in untracked.iter().take(500) {
        let fp = std::path::Path::new(path).join(f);
        if fp.metadata().map(|m| m.len() > 2_000_000).unwrap_or(true) {
            continue;
        }
        if let Ok(data) = std::fs::read(&fp) {
            if !data.iter().take(8000).any(|b| *b == 0) {
                added += data.iter().filter(|b| **b == b'\n').count() as i64 + if !data.is_empty() && !data.ends_with(b"\n") { 1 } else { 0 };
            }
        }
    }
    json!({"files": files + untracked.len() as i64, "added": added, "removed": removed, "new": untracked.len()})
}

pub fn session_detail(app: &App, sid: &str) -> Result<Value> {
    let Some(s) = board::get_session(app, Some(sid))? else { return err(404, "The board doesn't know that terminal.") };
    let t = board::task_for_session(app, Some(sid))?;
    let last = match &t {
        Some(_) => None,
        None => board::find_task(app, s.i("last_task"))?,
    };
    let turns = transcript::turns(app, &s, 40);
    let events = app.db.q("SELECT * FROM session_events WHERE session_id = ? ORDER BY id DESC LIMIT 80", p![sid])?;
    let latest = |kind: &str| events.iter().find(|e| e.s("kind") == Some(kind)).map(|e| json!({"text": e.v("text"), "at": e.v("at")}));
    let mut prompt = latest("prompt");
    if let Some(lt) = turns["turns"].as_array().and_then(|a| a.iter().find(|x| x["prompt"].is_string())) {
        let newer = prompt.as_ref().map(|p| lt["at"].as_str().unwrap_or("") > p["at"].as_str().unwrap_or("")).unwrap_or(true);
        if newer {
            prompt = Some(json!({"text": lt["prompt"], "at": lt["at"]}));
        }
    }
    if let Some(p) = prompt.as_mut() {
        p["text"] = json!(strip_tags(p["text"].as_str().unwrap_or("")));
    }
    let mut timeline: Vec<Value> = events
        .iter()
        .map(|e| json!({"at": e.v("at"), "kind": e.v("kind"), "text": if e.s("kind") == Some("prompt") { json!(strip_tags(&e.st("text"))) } else { e.v("text") }}))
        .collect();
    if timeline.is_empty() {
        timeline = turns["turns"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|x| {
                let p = x["prompt"].as_str().map(strip_tags).unwrap_or_else(|| "A turn that started from a notice, not a prompt".into());
                json!({"at": x["at"], "kind": "turn", "text": clip(&p, 300), "files": x["files"].as_array().map(|a| a.len()).unwrap_or(0)})
            })
            .collect();
    }
    let n_prompts = events.iter().filter(|e| e.s("kind") == Some("prompt")).count() as i64;
    let gone = s.s("status") == Some("gone");
    let mut out = json!({
        "id": s.v("id"), "name": s.s("name").filter(|n| !n.is_empty()).map(|n| n.to_string()).unwrap_or_else(|| format!("Terminal {}", sid.chars().take(8).collect::<String>())),
        "project": s.v("project"), "project_path": s.v("project_path"), "agent": s.v("agent"),
        "last_activity": s.v("last_activity"), "seen_at": s.v("seen_at"), "branch": s.v("branch"), "dirty": s.v("dirty"),
        "gone_at": s.v("gone_at"), "claude_session_id": s.v("claude_session_id"), "status_at": s.v("status_at"),
        "status": board::shown_status(&s), "api_error": s.v("api_error"), "idle_secs": idle_secs(&s),
        "close": board::close_rule(Some(&s)), "closing": jobs::closing(app, sid)?, "renaming": null, "rename_error": null,
        "task": match &t { Some(t) => board::task_card(app, t)?, None => Value::Null },
        "last_task": last.map(|l| json!({"ref": rf("task", l.id()), "title": l.v("title"), "status": l.v("status")})).unwrap_or(Value::Null),
        "prompt": prompt, "reply": latest("reply"),
        "waiting": if matches!(board::shown_status(&s), "needs" | "offline") { latest("wait") } else { None },
        "stats": {"turns": turns["count"].as_i64().unwrap_or(0).max(n_prompts),
                  "commits": events.iter().filter(|e| e.s("kind") == Some("commit")).count(),
                  "files": turns["files"]},
        "diff": if gone { Value::Null } else { diffstat(s.s("project_path")) },
        "timeline": timeline,
    });
    for (k, v) in jobs::rename_state(app, sid)? {
        out[k] = v;
    }
    if t.is_none() && crate::jira_desk::is_desk(app, Some(sid))? {
        out["role"] = json!(crate::jira_desk::ROLE);
        out["close"] = Value::Null;
    }
    Ok(out)
}
