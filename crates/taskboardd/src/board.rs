//! Tasks, goals, issues, sessions and jobs: the shared helpers every part of the board uses.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{fields, hooks, p, prflow, waitsfor};

pub const BOARD: &str = "Task board";
pub const OWNER: &str = "You";
pub const MIDNA: &str = "Midna";

pub const STATUSES: &[&str] = &["planned", "queued", "working", "needs", "done"];

pub fn get_task(app: &App, id: i64) -> Result<Row> {
    find_task(app, Some(id))?.ok_or_else(|| ApiError::new(404, format!("There's no task {}.", rf("task", id))))
}

pub fn find_task(app: &App, id: Option<i64>) -> Result<Option<Row>> {
    match id {
        Some(id) => app.db.q1("SELECT * FROM tasks WHERE id = ?", p![id]),
        None => Ok(None),
    }
}

pub fn update_task(app: &App, id: i64, mut f: Vec<(&str, Value)>) -> Result<()> {
    let before = if f.iter().any(|(k, _)| hooks::watched(k)) { find_task(app, Some(id))? } else { None };
    let goal = before.as_ref().and_then(|t| t.i("goal_id"));
    let goal_was_finished = match goal {
        Some(g) => goal_finished(app, g)?,
        None => true,
    };
    f.push(("updated_at", json!(now_iso())));
    app.db.update("tasks", &json!(id), f)?;
    app.schedule_md(id);
    if let Some(before) = before {
        let after = get_task(app, id)?;
        for event in hooks::events_between(&before, &after) {
            hooks::fire(app, event, &after, Some(&before));
        }
        if let (Some(g), false) = (goal, goal_was_finished) {
            if goal_finished(app, g)? {
                hooks::fire_goal(app, "goal.finished", &get_goal(app, g)?, Some(&after));
            }
        }
    }
    Ok(())
}

/// Every task in the goal is done and no PR of theirs is still open (`goal_counts`' finished_at, cheaply).
pub fn goal_finished(app: &App, goal_id: i64) -> Result<bool> {
    let rows = goal_tasks(app, goal_id)?;
    Ok(!rows.is_empty()
        && rows.iter().all(|r| r.s("status") == Some("done"))
        && !rows.iter().any(|r| !r.b("failed") && pr_still_open(r)))
}

/// Changes a goal and announces it being paused, resumed or archived.
pub fn update_goal(app: &App, id: i64, f: Vec<(&str, Value)>) -> Result<()> {
    let before = find_goal(app, Some(id))?;
    app.db.update("goals", &json!(id), f)?;
    let (Some(before), Some(after)) = (before, find_goal(app, Some(id))?) else { return Ok(()) };
    if !before.b("archived") && after.b("archived") {
        hooks::fire_goal(app, "goal.archived", &after, None);
    }
    if before.b("paused") != after.b("paused") {
        hooks::fire_goal(app, if after.b("paused") { "goal.paused" } else { "goal.resumed" }, &after, None);
    }
    Ok(())
}

pub fn bump_ctx(app: &App, id: i64) -> Result<()> {
    app.db.x("UPDATE tasks SET ctx_version = COALESCE(ctx_version, 1) + 1 WHERE id = ?", p![id])?;
    Ok(())
}

pub fn task_context(t: &Row) -> Row {
    jloads_obj(t.s("context"))
}

pub fn save_context(app: &App, id: i64, ctx: &Row, bump: bool) -> Result<()> {
    app.db.x(
        "UPDATE tasks SET context = ?, updated_at = ? WHERE id = ?",
        p![jdumps(&Value::Object(ctx.clone())), now_iso(), id],
    )?;
    if bump {
        bump_ctx(app, id)?;
    }
    app.schedule_md(id);
    Ok(())
}

pub fn log_event(app: &App, task_id: i64, who: &str, kind: &str, text: &str) -> Result<i64> {
    log_event_full(app, task_id, who, kind, text, None, None)
}

pub fn log_event_full(
    app: &App,
    task_id: i64,
    who: &str,
    kind: &str,
    text: &str,
    data: Option<Value>,
    at: Option<&str>,
) -> Result<i64> {
    let at = at.map(|s| s.to_string()).unwrap_or_else(now_iso);
    let who = if who.is_empty() { BOARD } else { who };
    let id = app.db.insert(
        "events",
        fields!["task_id" => task_id, "at" => at, "who" => who, "kind" => kind, "text" => text,
                "data" => data.map(|d| jdumps(&d))],
    )?;
    app.db.x("UPDATE tasks SET updated_at = ? WHERE id = ?", p![now_iso(), task_id])?;
    app.schedule_md(task_id);
    Ok(id)
}

pub fn next_position(app: &App, goal_id: Option<i64>) -> Result<Value> {
    let Some(g) = goal_id else { return Ok(Value::Null) };
    let v = app.db.val("SELECT MAX(position) FROM tasks WHERE goal_id = ?", p![g])?;
    Ok(json!(v.as_f64().unwrap_or(0.0).floor() as i64 + 1))
}

pub fn goal_tasks(app: &App, goal_id: i64) -> Result<Vec<Row>> {
    app.db.q("SELECT * FROM tasks WHERE goal_id = ? ORDER BY position, id", p![goal_id])
}

pub fn renumber_goal(app: &App, goal_id: i64) -> Result<()> {
    let rows = app.db.q("SELECT id, position FROM tasks WHERE goal_id = ? ORDER BY position, id", p![goal_id])?;
    for (i, t) in rows.iter().enumerate() {
        let want = (i + 1) as f64;
        if t.f("position") != Some(want) {
            app.db.x("UPDATE tasks SET position = ? WHERE id = ?", p![i as i64 + 1, t.id()])?;
        }
    }
    Ok(())
}

pub fn when_of(t: &Row) -> (Value, &'static str) {
    let st = t.st("status");
    match st.as_str() {
        "done" => (
            opt_str(t.s("finished_at").or(t.s("updated_at"))),
            if t.b("failed") { "stopped" } else { "finished" },
        ),
        "needs" => (
            t.v("updated_at"),
            if t.b("lost") {
                "lost"
            } else if has(t.s("question")) {
                "asked"
            } else {
                "updated"
            },
        ),
        "working" => (t.v("updated_at"), "updated"),
        _ => (t.v("created_at"), "added"),
    }
}

pub fn pr_card(t: &Row) -> Value {
    if !has(t.s("pr_repo")) || t.i("pr_num").is_none() {
        return Value::Null;
    }
    let rec = jloads_obj(t.s("pr_flow")).get("rec").cloned();
    let checks = rec.as_ref().map(|r| {
        if r["failed"].as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            "fail"
        } else if r["running"].as_i64().unwrap_or(0) > 0 {
            "pending"
        } else if r["checks"].as_array().map(|a| a.is_empty()).unwrap_or(true) {
            "none"
        } else {
            "pass"
        }
    });
    let review = rec.as_ref().map(|r| match r["review_decision"].as_str().unwrap_or("") {
        "APPROVED" => "approved",
        "CHANGES_REQUESTED" => "changes",
        "REVIEW_REQUIRED" => "pending",
        _ if r["approvals"].as_i64().unwrap_or(0) > 0 => "approved",
        _ => "none",
    });
    let state = match t.s("pr_state").unwrap_or("OPEN").to_uppercase().as_str() {
        "MERGED" => "MERGED",
        "OPEN" => "OPEN",
        _ => "DECLINED",
    };
    let repo = t.st("pr_repo");
    let short_repo = repo.rsplit('/').next().unwrap_or(&repo).to_string();
    json!({
        "host": t.st("pr_host"), "repo": short_repo, "full_repo": repo, "num": t.i0("pr_num"), "url": t.st("pr_url"),
        "state": state, "title": t.v("pr_title"), "checks": checks, "review": review,
        "stage": prflow::card(t),
    })
}

pub fn jira_url(app: &App, key: &str) -> Value {
    if app.cfg.jira.site.trim().is_empty() || key.is_empty() {
        return Value::Null;
    }
    json!(format!("https://{}/browse/{}", app.cfg.jira.site.trim(), key))
}

pub fn jira_card(app: &App, t: &Row) -> Result<Value> {
    if let Some(k) = t.s("jira_key").filter(|k| !k.is_empty()) {
        return Ok(json!({"key": k, "status": t.v("jira_status"), "url": jira_url(app, k)}));
    }
    let asked = app.db.q1(
        "SELECT id FROM jobs WHERE kind = 'jira' AND task_id = ? AND state IN ('pending', 'running') \
         AND json_extract(args, '$.op') = 'create' LIMIT 1",
        p![t.id()],
    )?;
    Ok(if asked.is_some() { json!({"key": null, "status": "Ticket asked for", "url": null}) } else { Value::Null })
}

pub fn goal_ref(g: Option<&Row>) -> Value {
    match g {
        Some(g) => json!({"id": g.id(), "ref": rf("goal", g.id()), "name": g.v("name")}),
        None => Value::Null,
    }
}

pub fn task_card(app: &App, t: &Row) -> Result<Value> {
    let g = find_goal(app, t.i("goal_id"))?;
    let (when, when_kind) = when_of(t);
    let status = t.st("status");
    let position = match t.f("position") {
        Some(p) if p.fract() == 0.0 => json!(p as i64),
        Some(p) => json!(p),
        None => Value::Null,
    };
    let waits: Vec<Value> = waitsfor::ids(t).into_iter().map(|n| json!(rf("task", n))).collect();
    let waiting = if status == "queued" { waitsfor::waiting_line(app, t)? } else { Value::Null };
    Ok(json!({
        "id": t.id(), "ref": rf("task", t.id()), "title": t.v("title"), "project": t.v("project"),
        "status": status, "priority": t.s("priority").unwrap_or("normal"),
        "failed": t.b("failed"), "lost": t.b("lost"), "needs_reason": t.v("needs_reason"),
        "question": t.v("question"), "latest": t.v("latest"), "summary": t.v("summary"),
        "when": when, "when_kind": when_kind, "started_at": t.v("started_at"),
        "updated_at": t.v("updated_at"), "finished_at": t.v("finished_at"),
        "who": t.v("session_name"), "session_id": t.v("session_id"),
        "goal": goal_ref(g.as_ref()),
        "jira": jira_card(app, t)?,
        "pr": pr_card(t),
        "position": position,
        "starting": t.i("start_job").is_some() && status == "queued",
        "waits_for": waits,
        "waiting": waiting,
        "blocked": is_blocked(app, t)?,
    }))
}

pub fn task_for_session(app: &App, sid: Option<&str>) -> Result<Option<Row>> {
    match sid.filter(|s| !s.is_empty()) {
        Some(sid) => app.db.q1(
            "SELECT * FROM tasks WHERE session_id = ? AND status != 'done' ORDER BY updated_at DESC, id DESC LIMIT 1",
            p![sid],
        ),
        None => Ok(None),
    }
}

pub fn get_session(app: &App, sid: Option<&str>) -> Result<Option<Row>> {
    match sid.filter(|s| !s.is_empty()) {
        Some(sid) => app.db.q1("SELECT * FROM sessions WHERE id = ?", p![sid]),
        None => Ok(None),
    }
}

pub fn runs_claude(s: &Row) -> bool {
    has(s.s("claude_session_id")) || s.st("agent").to_lowercase().contains("claude")
}

pub fn upsert_session(app: &App, sid: &str, f: Vec<(&str, Value)>) -> Result<Option<Row>> {
    let f: Vec<(&str, Value)> = f.into_iter().filter(|(_, v)| !v.is_null()).collect();
    if get_session(app, Some(sid))?.is_some() {
        app.db.update("sessions", &json!(sid), f)?;
    } else {
        let mut row = fields!["id" => sid, "status" => "idle", "source" => "hook", "missed" => 0];
        for (k, v) in f {
            row.retain(|(rk, _)| *rk != k);
            row.push((k, v));
        }
        app.db.insert("sessions", row)?;
    }
    get_session(app, Some(sid))
}

pub fn note_rename(app: &App, cur: Option<&Row>, name: Option<&str>) -> Result<()> {
    if let (Some(cur), Some(name)) = (cur, name) {
        let old = cur.st("name");
        if !old.is_empty() && !name.is_empty() && old != name {
            session_event(app, &cur.st("id"), "rename", &format!("Renamed from “{old}” to “{name}”"), None)?;
        }
    }
    Ok(())
}

const SESSION_EVENTS_KEEP: i64 = 300;

pub fn session_event(app: &App, sid: &str, kind: &str, text: &str, at: Option<&str>) -> Result<()> {
    if sid.is_empty() || kind.is_empty() {
        return Ok(());
    }
    let text = if kind == "reply" { clip(text.trim(), 4000) } else { one_line(text, 600) };
    app.db.insert(
        "session_events",
        fields!["session_id" => sid, "at" => at.map(|s| s.to_string()).unwrap_or_else(now_iso), "kind" => kind, "text" => text],
    )?;
    if let Some(cut) = app.db.q1(
        "SELECT id FROM session_events WHERE session_id = ? ORDER BY id DESC LIMIT 1 OFFSET ?",
        p![sid, SESSION_EVENTS_KEEP],
    )? {
        app.db.x("DELETE FROM session_events WHERE session_id = ? AND id <= ?", p![sid, cut.id()])?;
    }
    Ok(())
}

/// How the board may close a terminal: "close" when idle, "force" when busy, None when it's gone.
/// The terminal's last turn died on a lost connection, and nothing has reached the API since.
pub fn offline(s: &Row) -> bool {
    has(s.s("api_error")) && s.s("api_error_kind") == Some("network")
}

/// A session's status for the page. A terminal whose last turn ended on an API error and that hasn't
/// started another shows it: "offline" when the network went, else "needs".
pub fn shown_status(s: &Row) -> &str {
    let st = s.s("status").unwrap_or("idle");
    if matches!(st, "working" | "gone") || !has(s.s("api_error")) {
        return st;
    }
    if s.s("api_error_kind") == Some("network") {
        "offline"
    } else {
        "needs"
    }
}

pub fn close_rule(s: Option<&Row>) -> Option<&'static str> {
    let s = s?;
    if s.s("status") == Some("gone") {
        return None;
    }
    Some(if s.s("status").unwrap_or("idle") == "idle" { "close" } else { "force" })
}

pub fn session_name(app: &App, sid: Option<&str>, fallback: Option<&str>) -> String {
    if let Ok(Some(s)) = get_session(app, sid) {
        if let Some(n) = s.s("name").filter(|n| !n.is_empty()) {
            return n.to_string();
        }
    }
    if let Some(f) = fallback.filter(|f| !f.is_empty()) {
        return f.to_string();
    }
    match sid {
        Some(s) if !s.is_empty() => format!("Terminal {}", s.chars().take(8).collect::<String>()),
        _ => "A terminal".into(),
    }
}

pub fn add_terminal(app: &App, task_id: i64, sid: &str, why: &str) -> Result<()> {
    if sid.is_empty() {
        return Ok(());
    }
    app.db.x(
        "INSERT INTO task_terminals(task_id, session_id, why, at) VALUES(?, ?, ?, ?) \
         ON CONFLICT(task_id, session_id) DO NOTHING",
        p![task_id, sid, why, now_iso()],
    )?;
    Ok(())
}

pub fn terminals(app: &App, t: &Row) -> Result<Vec<Value>> {
    let rows = app.db.q(
        "SELECT tt.session_id, tt.why, tt.at, s.name, s.status FROM task_terminals tt \
         LEFT JOIN sessions s ON s.id = tt.session_id WHERE tt.task_id = ? ORDER BY tt.rowid DESC",
        p![t.id()],
    )?;
    let mut out: Vec<Value> = rows
        .iter()
        .map(|r| {
            let sid = r.st("session_id");
            json!({"id": sid, "name": r.s("name").map(|s| s.to_string()).unwrap_or_else(|| session_name(app, Some(&sid), t.s("session_name"))),
                   "status": r.s("status").unwrap_or("gone"), "why": r.v("why"), "at": r.v("at")})
        })
        .collect();
    if let Some(sid) = t.s("session_id").filter(|s| !s.is_empty()) {
        if !out.iter().any(|x| x["id"] == sid) {
            let s = get_session(app, Some(sid))?;
            out.push(json!({"id": sid, "name": session_name(app, Some(sid), t.s("session_name")),
                            "status": s.as_ref().and_then(|s| s.s("status")).unwrap_or("gone"),
                            "why": "Worked on the task", "at": t.v("started_at")}));
        }
    }
    out.sort_by(|a, b| b["at"].as_str().unwrap_or("").cmp(a["at"].as_str().unwrap_or("")));
    Ok(out)
}

pub fn get_goal(app: &App, id: i64) -> Result<Row> {
    find_goal(app, Some(id))?.ok_or_else(|| ApiError::new(404, format!("There's no goal {}.", rf("goal", id))))
}

pub fn find_goal(app: &App, id: Option<i64>) -> Result<Option<Row>> {
    match id {
        Some(id) => app.db.q1("SELECT * FROM goals WHERE id = ?", p![id]),
        None => Ok(None),
    }
}

pub fn pr_still_open(t: &Row) -> bool {
    t.i("pr_num").is_some()
        && t.s("pr_state").unwrap_or("OPEN").to_uppercase() == "OPEN"
        && !matches!(t.s("pr_phase"), Some("merged") | Some("declined"))
}

pub fn is_blocked(app: &App, t: &Row) -> Result<bool> {
    Ok(t.s("status") == Some("queued") && t.i("start_job").is_none() && waitsfor::blocker(app, t)?.is_some())
}

pub fn goal_counts(app: &App, goal_id: i64) -> Result<Value> {
    let rows = goal_tasks(app, goal_id)?;
    let n = |f: &dyn Fn(&Row) -> bool| rows.iter().filter(|r| f(r)).count() as i64;
    let done = n(&|r| r.s("status") == Some("done"));
    let mut blocked = 0;
    for r in &rows {
        if is_blocked(app, r)? {
            blocked += 1;
        }
    }
    let prs_open: Vec<i64> = rows
        .iter()
        .filter(|r| r.s("status") == Some("done") && !r.b("failed") && pr_still_open(r))
        .filter_map(|r| r.i("pr_num"))
        .collect();
    let total = rows.len() as i64;
    let finished_at = if total > 0 && done == total && prs_open.is_empty() {
        rows.iter().filter_map(|r| r.s("finished_at")).max().map(|s| json!(s)).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    Ok(json!({
        "done": done, "total": total,
        "active": n(&|r| matches!(r.s("status"), Some("working") | Some("needs"))),
        "needs": n(&|r| r.s("status") == Some("needs")),
        "queued": n(&|r| r.s("status") == Some("queued")),
        "starting": n(&|r| r.s("status") == Some("queued") && r.i("start_job").is_some()),
        "blocked": blocked,
        "planned": n(&|r| r.s("status") == Some("planned")),
        "failed": n(&|r| r.s("status") == Some("done") && r.b("failed")),
        "prs_open": prs_open,
        "prs": n(&|r| r.i("pr_num").is_some()),
        "finished_at": finished_at,
        "open_issues": app.db.count("SELECT COUNT(*) FROM issues WHERE goal_id = ? AND state = 'open'", p![goal_id])?,
        "closed_count": app.db.count("SELECT COUNT(*) FROM issues WHERE goal_id = ? AND state = 'drop'", p![goal_id])?,
    }))
}

pub fn goal_state_line(c: &Value) -> String {
    let total = c["total"].as_i64().unwrap_or(0);
    let done = c["done"].as_i64().unwrap_or(0);
    if total > 0 && done == total {
        let open = c["prs_open"].as_array().cloned().unwrap_or_default();
        return match open.len() {
            0 => "Done".into(),
            1 => format!("Waits for PR #{} to merge", open[0]),
            n => format!("Waits for {n} PRs to merge"),
        };
    }
    if c["active"] == 0 && c["queued"] == 0 && done == 0 {
        return "Not started".into();
    }
    "In progress".into()
}

pub fn goal_dict(app: &App, g: &Row) -> Result<Value> {
    let c = goal_counts(app, g.id())?;
    let mut d = json!({
        "id": g.id(), "ref": rf("goal", g.id()), "name": g.v("name"), "outcome": g.v("outcome"),
        "tldr": g.s("tldr").unwrap_or(""), "hours_until": g.v("hours_until"),
        "project": g.v("project"), "repo_path": g.v("repo_path"),
        "epic_key": g.v("epic_key"), "epic_status": g.v("epic_status"),
        "epic_url": jira_url(app, g.s("epic_key").unwrap_or("")), "product": g.v("product"),
        "run_in_order": g.b("run_in_order"), "max_terminals": g.v("max_terminals"),
        "auto_close": g.b("auto_close"), "archived": g.b("archived"), "paused": g.b("paused"),
        "deprioritized": g.b("deprioritized"),
        "created_at": g.v("created_at"), "updated_at": g.v("updated_at"),
        "state": goal_state_line(&c),
        "peek": goal_peek(app, g)?,
    });
    if let (Some(o), Some(c)) = (d.as_object_mut(), c.as_object()) {
        for (k, v) in c {
            o.insert(k.clone(), v.clone());
        }
    }
    Ok(d)
}

fn peek_order(s: &str) -> u8 {
    match s {
        "needs" => 0,
        "working" => 1,
        "failed" => 2,
        "starting" => 3,
        "queued" => 4,
        "blocked" => 5,
        _ => 9,
    }
}

pub fn goal_peek(app: &App, g: &Row) -> Result<Vec<Value>> {
    let mut out = vec![];
    for t in goal_tasks(app, g.id())? {
        let mut why = Value::Null;
        let key = match t.s("status") {
            Some("done") if !t.b("failed") => continue,
            Some("done") => "failed".to_string(),
            Some("planned") => continue,
            Some("queued") => {
                if t.i("start_job").is_some() {
                    "starting".into()
                } else if let Some(b) = waitsfor::blocker(app, &t)? {
                    why = json!(b);
                    "blocked".into()
                } else {
                    "queued".into()
                }
            }
            other => other.unwrap_or("").to_string(),
        };
        out.push(json!({"ref": rf("task", t.id()), "title": t.v("title"), "status": key, "why": why}));
    }
    out.sort_by_key(|x| peek_order(x["status"].as_str().unwrap_or("")));
    Ok(out)
}

pub fn goal_note_dict(n: &Row) -> Value {
    json!({"id": n.id(), "kind": n.v("kind"), "text": n.v("text"), "source": n.v("source"),
           "pinned": n.b("pinned"), "at": n.v("at")})
}

pub const ATTACH_KINDS: &[&str] = &["design", "proposal", "doc", "evidence", "results", "other"];

pub fn attachments(app: &App, task_id: Option<i64>, goal_id: Option<i64>) -> Result<Vec<Value>> {
    let (col, val) = if let Some(t) = task_id { ("task_id", t) } else { ("goal_id", goal_id.unwrap_or(-1)) };
    Ok(app
        .db
        .q(&format!("SELECT * FROM attachments WHERE {col} = ? AND removed_at IS NULL ORDER BY id"), p![val])?
        .iter()
        .map(attachment_dict)
        .collect())
}

static ATT_TAG_RE: Lazy<Regex> = Lazy::new(|| {
    let tag = r"(?:\d{4}-\d{2}-\d{2}|[TG]\d+)";
    Regex::new(&format!(r"\s*[\(\[]\s*{tag}(?:\s*[,;·]\s*{tag})*\s*[\)\]]\s*$")).unwrap()
});

pub fn attachment_title(title: &str, url: &str) -> String {
    let t = ATT_TAG_RE.replace(&one_line(title, 200), "").trim().to_string();
    if !t.is_empty() {
        return t;
    }
    url.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(url).to_string()
}

pub fn attachment_dict(a: &Row) -> Value {
    json!({"id": a.id(), "kind": a.v("kind"), "title": attachment_title(&a.st("title"), &a.st("url")),
           "url": a.v("url"), "added_by": a.v("added_by"), "at": a.v("at"),
           "task": rf_opt("task", a.i("task_id")), "goal": rf_opt("goal", a.i("goal_id"))})
}

static URL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^https?://\S+$").unwrap());

pub fn add_attachment(
    app: &App,
    who: &str,
    url: &str,
    title: &str,
    kind: &str,
    task_id: Option<i64>,
    goal_id: Option<i64>,
) -> Result<Value> {
    let url = url.trim();
    if !(URL_RE.is_match(url) || url.starts_with('/') || url.starts_with("~/")) {
        return err(400, "Attach a link that starts with https:// or a full file path.");
    }
    let kind = if ATTACH_KINDS.contains(&kind) { kind } else { "other" };
    let title = attachment_title(title, url);
    let (col, val) = if let Some(t) = task_id { ("task_id", t) } else { ("goal_id", goal_id.unwrap_or(-1)) };
    let cur = app.db.q1(
        &format!("SELECT * FROM attachments WHERE {col} = ? AND url = ? AND removed_at IS NULL"),
        p![val, url],
    )?;
    let aid = if let Some(cur) = cur {
        app.db.x("UPDATE attachments SET title = ?, kind = ? WHERE id = ?", p![title, kind, cur.id()])?;
        cur.id()
    } else {
        app.db.insert(
            "attachments",
            fields!["task_id" => task_id, "goal_id" => goal_id, "kind" => kind, "title" => title,
                    "url" => url, "added_by" => who, "at" => now_iso()],
        )?
    };
    let label = match kind {
        "design" => "a design",
        "proposal" => "a proposal",
        "doc" => "a doc",
        "evidence" => "evidence",
        "results" => "results",
        _ => "a link",
    };
    if let Some(t) = task_id {
        log_event(app, t, who, "note", &format!("Attached {label}: {title}"))?;
        bump_ctx(app, t)?;
    } else {
        app.db.x(
            "UPDATE tasks SET ctx_version = COALESCE(ctx_version, 1) + 1 WHERE goal_id = ? AND status != 'done'",
            p![goal_id],
        )?;
    }
    let a = app.db.q1("SELECT * FROM attachments WHERE id = ?", p![aid])?.unwrap_or_default();
    Ok(attachment_dict(&a))
}

pub fn goal_notes(app: &App, goal_id: i64) -> Result<Vec<Row>> {
    app.db.q("SELECT * FROM goal_notes WHERE goal_id = ? ORDER BY pinned DESC, at DESC, id DESC", p![goal_id])
}

pub fn add_goal_note(
    app: &App,
    goal_id: i64,
    kind: &str,
    text: &str,
    source: Option<&str>,
    pinned: bool,
    except_task: Option<i64>,
) -> Result<i64> {
    let kind = if NOTE_KINDS.contains(&kind) { kind } else { "finding" };
    let id = app.db.insert(
        "goal_notes",
        fields!["goal_id" => goal_id, "kind" => kind, "text" => text, "source" => source,
                "pinned" => pinned as i64, "at" => now_iso()],
    )?;
    app.db.x("UPDATE goals SET updated_at = ? WHERE id = ?", p![now_iso(), goal_id])?;
    app.db.x(
        "UPDATE tasks SET ctx_version = COALESCE(ctx_version, 1) + 1 WHERE goal_id = ? AND status != 'done' AND id IS NOT ?",
        p![goal_id, except_task],
    )?;
    Ok(id)
}

pub fn get_issue(app: &App, id: i64) -> Result<Row> {
    find_issue(app, Some(id))?.ok_or_else(|| ApiError::new(404, format!("There's no backlog issue {}.", rf("issue", id))))
}

pub fn find_issue(app: &App, id: Option<i64>) -> Result<Option<Row>> {
    match id {
        Some(id) => app.db.q1("SELECT * FROM issues WHERE id = ?", p![id]),
        None => Ok(None),
    }
}

pub fn add_issue_event(app: &App, issue_id: i64, who: &str, kind: &str, text: &str, snapshot: Option<&Value>) -> Result<()> {
    app.db.insert(
        "issue_events",
        fields!["issue_id" => issue_id, "at" => now_iso(), "who" => who, "kind" => kind, "text" => text,
                "snapshot" => snapshot.map(jdumps)],
    )?;
    app.db.x("UPDATE issues SET updated_at = ? WHERE id = ?", p![now_iso(), issue_id])?;
    Ok(())
}

pub fn issue_from_line(b: &Row) -> String {
    let t = local_clock(b.s("created_at"));
    match b.s("source") {
        Some("terminal") => {
            let by = b.i("found_by_task").map(|i| rf("task", i)).unwrap_or_else(|| "a terminal".into());
            let who = b.s("found_by_name").filter(|s| !s.is_empty()).unwrap_or("a terminal");
            format!("Found by {by} · {who} · {t}")
        }
        Some("answer") => format!("From an answer · {t}"),
        Some("review") => format!("From a review · {t}"),
        _ => format!("Added by you · {t}"),
    }
}

pub fn issue_card(app: &App, b: &Row) -> Result<Value> {
    let g = find_goal(app, b.i("goal_id"))?;
    Ok(json!({
        "id": b.id(), "ref": rf("issue", b.id()), "kind": b.v("kind"), "kind_label": kind_label(&b.st("kind")),
        "title": b.v("title"), "goal": goal_ref(g.as_ref()), "goal_id": b.v("goal_id"), "project": b.v("project"),
        "found_by_task": b.v("found_by_task"),
        "found_by_name": b.v("found_by_name"), "source": b.v("source"),
        "created_at": b.v("created_at"), "updated_at": b.v("updated_at"), "state": b.v("state"),
        "jira_key": b.v("jira_key"), "jira_url": jira_url(app, b.s("jira_key").unwrap_or("")),
        "task_id": b.v("task_id"), "task_ref": rf_opt("task", b.i("task_id")),
        "from": issue_from_line(b),
    }))
}

pub fn issue_dict(app: &App, b: &Row) -> Result<Value> {
    let mut d = issue_card(app, b)?;
    let ft = find_task(app, b.i("found_by_task"))?;
    let o = d.as_object_mut().unwrap();
    o.insert("detail".into(), b.v("detail"));
    o.insert("said".into(), b.v("said"));
    o.insert("how".into(), b.v("how"));
    o.insert("snapshot".into(), Value::Object(jloads_obj(b.s("snapshot"))));
    o.insert("found_by_session".into(), b.v("found_by_session"));
    o.insert(
        "found_by_task".into(),
        ft.map(|ft| json!({"id": ft.id(), "ref": rf("task", ft.id()), "title": ft.v("title")})).unwrap_or(Value::Null),
    );
    Ok(d)
}

pub fn issue_history(app: &App, issue_id: i64) -> Result<Vec<Value>> {
    Ok(app
        .db
        .q("SELECT * FROM issue_events WHERE issue_id = ? ORDER BY at, id", p![issue_id])?
        .iter()
        .map(|e| {
            json!({"at": e.v("at"), "who": e.v("who"), "kind": e.v("kind"), "text": e.v("text"),
                   "snapshot": if has(e.s("snapshot")) { Value::Object(jloads_obj(e.s("snapshot"))) } else { Value::Null }})
        })
        .collect())
}

pub fn create_job(app: &App, kind: &str, args: Value, task_id: Option<i64>, purpose: &str, target: Option<Value>) -> Result<i64> {
    let now = now_iso();
    let id = app.db.insert(
        "jobs",
        fields!["kind" => kind, "args" => jdumps(&args), "state" => "pending", "task_id" => task_id,
                "created_at" => now, "updated_at" => now, "attempts" => 0,
                "purpose" => if purpose.is_empty() { None } else { Some(purpose) },
                "target" => target.map(|t| jdumps(&t))],
    )?;
    app.notify_jobs();
    Ok(id)
}

pub fn job_args(j: &Row) -> Row {
    jloads_obj(j.s("args"))
}

pub fn job_target(j: &Row) -> Row {
    jloads_obj(j.s("target"))
}

pub fn job_dict(j: &Row) -> Value {
    json!({"id": j.id(), "ref": rf("job", j.id()), "kind": j.v("kind"), "args": Value::Object(job_args(j)),
           "state": j.v("state"), "task_id": j.v("task_id"), "purpose": j.v("purpose"),
           "result": if has(j.s("result")) { Value::Object(jloads_obj(j.s("result"))) } else { Value::Null },
           "created_at": j.v("created_at"), "updated_at": j.v("updated_at"), "attempts": j.v("attempts")})
}

pub fn live_start_job(app: &App, t: &Row) -> Result<Option<Row>> {
    let Some(jid) = t.i("start_job") else { return Ok(None) };
    let j = app.db.q1("SELECT * FROM jobs WHERE id = ?", p![jid])?;
    Ok(j.filter(|j| matches!(j.s("state"), Some("pending") | Some("running") | Some("done"))))
}

pub const USAGE_CLOSE: &str = "usage_close";
pub const HOURS_CLOSE: &str = "hours_close";

fn requeue_reason(purpose: &str) -> (&'static str, &'static str) {
    if purpose == USAGE_CLOSE {
        ("when the 5-hour usage ran out", "when the usage resets")
    } else {
        ("idle after work hours", "when the work hours open")
    }
}

pub fn requeue_close(app: &App, sid: &str) -> Result<Option<String>> {
    for j in app.db.q(
        "SELECT args, purpose FROM jobs WHERE kind = 'close' AND purpose IN (?, ?) ORDER BY id DESC LIMIT 50",
        p![USAGE_CLOSE, HOURS_CLOSE],
    )? {
        if job_args(&j).get("session").and_then(|v| v.as_str()) == Some(sid) {
            return Ok(j.s("purpose").map(|s| s.to_string()));
        }
    }
    Ok(None)
}

pub fn closing_session(app: &App, sid: &str) -> Result<bool> {
    for j in app.db.q("SELECT * FROM jobs WHERE kind = 'close' ORDER BY id DESC LIMIT 50", p![])? {
        if job_args(&j).get("session").and_then(|v| v.as_str()) != Some(sid) {
            continue;
        }
        match j.s("state") {
            Some("pending") | Some("running") => return Ok(true),
            Some("done") if age_secs(j.s("updated_at")).unwrap_or(0.0) < 300.0 => return Ok(true),
            _ => {}
        }
    }
    Ok(false)
}

/// The `tb` command agents are told to run: the path the plugin reported, or a bare `tb`.
pub fn tb_cmd(app: &App) -> String {
    match app.db.get_setting("tb_path") {
        Ok(Some(p)) if !p.is_empty() && !p.contains(' ') => p,
        _ => "tb".into(),
    }
}

pub fn jira_keep_in_step(app: &App, t: &Row, target_status: Option<&str>, comment: Option<&str>) -> Result<Option<i64>> {
    if !app.cfg.jira_on() || !has(t.s("jira_key")) || !t.b("jira_sync") {
        return Ok(None);
    }
    if let Some(target) = target_status.filter(|s| !s.is_empty()) {
        if t.st("jira_status").to_lowercase() == target.to_lowercase() {
            return Ok(None);
        }
        return crate::jira::request(app, "transition", t.id(), &t.st("jira_key"), Some(target), comment).map(Some);
    }
    if let Some(c) = comment {
        return crate::jira::request(app, "comment", t.id(), &t.st("jira_key"), None, Some(c)).map(Some);
    }
    Ok(None)
}

pub fn set_working(app: &App, t: &Row, who: &str, text: Option<&str>) -> Result<()> {
    let was = t.st("status");
    let started = t.s("started_at").map(|s| s.to_string()).unwrap_or_else(now_iso);
    update_task(
        app,
        t.id(),
        fields!["status" => "working", "needs_reason" => null, "question" => null, "answered_at" => null,
                "lost" => 0, "start_tries" => 0, "retry_at" => null, "started_at" => started],
    )?;
    if let Some(text) = text {
        log_event(app, t.id(), who, "status", text)?;
    }
    if was != "working" {
        let t = get_task(app, t.id())?;
        let status = app.cfg.jira.in_progress.clone();
        jira_keep_in_step(app, &t, Some(&status), None)?;
    }
    Ok(())
}

pub fn claim(app: &App, t: &Row, sid: &str, claude: Option<&str>, who: Option<&str>, how: &str, reopen_pr: bool) -> Result<std::result::Result<(), String>> {
    let tref = rf("task", t.id());
    if t.s("status") == Some("done") && !(reopen_pr && pr_still_open(t)) {
        return Ok(Err(format!("{tref} is already done.")));
    }
    if let Some(other_sid) = t.s("session_id").filter(|s| !s.is_empty() && *s != sid) {
        if let Some(other) = get_session(app, Some(other_sid))? {
            if other.s("status") != Some("gone")
                && matches!(t.s("status"), Some("working") | Some("needs"))
                && !t.b("lost")
            {
                let n = other.s("name").filter(|n| !n.is_empty()).unwrap_or("another terminal");
                return Ok(Err(format!("{tref} is already on {n}.")));
            }
        }
    }
    let name = session_name(app, Some(sid), None);
    if let Some(prev) = task_for_session(app, Some(sid))? {
        if prev.id() != t.id() {
            update_task(
                app,
                prev.id(),
                fields!["session_id" => null, "status" => "queued", "pickup" => "manual", "start_job" => null],
            )?;
            log_event(app, prev.id(), &name, "status", &format!("{name} took {tref} instead, so this went back to the queue"))?;
        }
    }
    let already = t.s("session_id") == Some(sid) && t.s("status") == Some("working");
    let mut f = fields!["session_id" => sid, "session_name" => name, "start_job" => null, "lost" => 0];
    if !already {
        f.push(("latest", json!(format!("{how} in {name}."))));
    }
    if let Some(c) = claude.filter(|c| !c.is_empty()) {
        f.push(("claude_session_id", json!(c)));
    }
    update_task(app, t.id(), f)?;
    add_terminal(app, t.id(), sid, "Worked on the task")?;
    if !already {
        if let Some(s) = get_session(app, Some(sid))? {
            if has(s.s("project_path")) && !has(t.s("repo_path")) {
                update_task(app, t.id(), fields!["repo_path" => s.v("project_path")])?;
            }
        }
        let fresh = get_task(app, t.id())?;
        set_working(app, &fresh, who.unwrap_or(&name), Some(&format!("{how} in {name}")))?;
    }
    let gone = get_session(app, Some(sid))?.map(|s| s.s("status") == Some("gone")).unwrap_or(false);
    upsert_session(app, sid, fields!["status" => if gone { None } else { Some("working") }, "last_task" => t.id()])?;
    Ok(Ok(()))
}

const DELIBERATE_EXITS: &[&str] = &["prompt_input_exit", "logout"];
const LOST_RESTARTS_PER_HOUR: i64 = 2;

pub fn restarts_left(app: &App, t: &Row) -> Result<bool> {
    let since = iso(now_ts() - 3600.0);
    let ended = app.db.count(
        "SELECT COUNT(*) FROM events WHERE task_id = ? AND who = ? AND kind = 'status' AND text LIKE 'Terminal ended%' AND at >= ?",
        p![t.id(), MIDNA, since],
    )?;
    Ok(ended < LOST_RESTARTS_PER_HOUR)
}

pub fn mark_lost(app: &App, t: &Row, reason: &str) -> Result<()> {
    if t.s("status") == Some("done") || t.b("lost") {
        return Ok(());
    }
    let closed = format!("The terminal closed at {} before the task was done.", local_clock(None));
    let asked = t.s("status") == Some("needs") && t.s("needs_reason") == Some("question");
    if !DELIBERATE_EXITS.contains(&reason) && !asked && restarts_left(app, t)? {
        let pickup = if matches!(t.s("pickup"), Some("manual") | Some("attach")) { "new".to_string() } else { t.st("pickup") };
        update_task(
            app,
            t.id(),
            fields!["status" => "queued", "lost" => 0, "needs_reason" => null, "session_id" => null, "start_job" => null,
                    "pickup" => pickup, "pickup_session" => null,
                    "latest" => format!("{closed} It starts again from its handoff.")],
        )?;
        log_event(app, t.id(), MIDNA, "status", &format!("Terminal ended ({reason}) before the task was done; queued to start again from its handoff"))?;
        app.info(format!("task {} lost ({reason}): queued to start again", rf("task", t.id())));
        return Ok(());
    }
    let saved = task_context(t).get("saved_at").and_then(|v| v.as_str()).map(|s| s.to_string());
    let saved_line = match saved {
        Some(s) => format!(" Its context was saved at {}.", local_clock(Some(&s))),
        None => " Nothing was saved yet.".into(),
    };
    update_task(
        app,
        t.id(),
        fields!["status" => "needs", "lost" => 1, "needs_reason" => "lost", "start_job" => null,
                "latest" => format!("{closed}{saved_line}")],
    )?;
    log_event(app, t.id(), MIDNA, "status", &format!("Terminal ended ({reason}) before the task was done"))?;
    app.info(format!("task {} lost: {reason}", rf("task", t.id())));
    Ok(())
}

pub fn session_gone(app: &App, sid: &str, reason: &str) -> Result<()> {
    let t = task_for_session(app, Some(sid))?;
    upsert_session(app, sid, fields!["status" => "gone", "gone_at" => now_iso(), "last_task" => t.as_ref().map(|t| t.id())])?;
    if let Some(t) = t {
        if closing_session(app, sid)? {
            detach_closed(app, &t)?;
        } else {
            mark_lost(app, &t, reason)?;
        }
    }
    for b in app.db.q("SELECT id FROM issues WHERE found_by_session = ? AND state = 'open'", p![sid])? {
        add_issue_event(app, b.id(), MIDNA, "lost", "The terminal that reported it closed. The issue and its snapshot are kept.", None)?;
    }
    Ok(())
}

pub fn detach_closed(app: &App, t: &Row) -> Result<()> {
    let name = t.s("session_name").filter(|s| !s.is_empty()).unwrap_or("its terminal").to_string();
    if t.s("status") == Some("done") {
        update_task(app, t.id(), fields!["session_id" => null])?;
        log_event(app, t.id(), MIDNA, "midna", &format!("Closed {name}"))?;
        return Ok(());
    }
    if let Some(purpose) = requeue_close(app, &t.st("session_id"))? {
        let (why, until) = requeue_reason(&purpose);
        let asked = t.s("status") == Some("needs") && t.s("needs_reason") == Some("question");
        let pickup = if matches!(t.s("pickup"), Some("manual") | Some("attach")) { "new".to_string() } else { t.st("pickup") };
        let mut f = fields!["session_id" => null, "start_job" => null, "lost" => 0, "pickup" => pickup,
                            "pickup_session" => null,
                            "latest" => format!("Closed {why}. {}", if asked { "It starts again once you answer.".to_string() } else { format!("It starts again from its handoff {until}.") })];
        if !asked {
            f.extend(fields!["status" => "queued", "needs_reason" => null, "question" => null]);
        }
        update_task(app, t.id(), f)?;
        log_event(app, t.id(), MIDNA, "status", &format!("Closed {name} {why}"))?;
        return Ok(());
    }
    update_task(
        app,
        t.id(),
        fields!["session_id" => null, "status" => "queued", "pickup" => "manual", "start_job" => null,
                "needs_reason" => null, "lost" => 0, "latest" => "Its terminal was closed. Press Start to run it again."],
    )?;
    log_event(app, t.id(), MIDNA, "status", &format!("Closed {name}. Back in the queue; press Start to run it again"))?;
    Ok(())
}
