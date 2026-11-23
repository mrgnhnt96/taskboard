//! What the board took off the owner's Needs you list by itself, for the app's Handled for you list:
//! each is a task event of kind [`KIND`], logged by the board, whose data says what did it (`by`),
//! what the item was (`item`), the verdict and what Pull back needs to put it back.
//!
//! `GET /handled` lists them newest first, a page at a time; `POST /handled/:id/pull-back` puts one back on
//! Needs you and stamps the record `pulled_at`, which takes it off the list.

use serde_json::{json, Value};

use crate::api::{Paging, Query};
use crate::app::App;
use crate::util::*;
use crate::{blocks, board, deliver, fields, handoff, human, misses, p};

pub const KIND: &str = "handled";

/// Records that the board took task `task_id`'s item off Needs you. `by` names the sweep that did it
/// (`recheck`, `retry`, `misses`); `data` holds the rest, such as `item` and `verdict`.
pub fn record(app: &App, task_id: i64, by: &str, text: &str, data: Value) -> Result<i64> {
    let mut d = json!({"by": by});
    if let Value::Object(more) = data {
        for (k, v) in more {
            d[k] = v;
        }
    }
    board::log_event_full(app, task_id, board::BOARD, KIND, text, Some(d), None)
}

fn data(e: &Row) -> Value {
    Value::Object(jloads_obj(e.s("data")))
}

/// The issue a misses record is about.
fn issue_of(app: &App, d: &Value) -> Result<Option<Row>> {
    match d["issue"].as_i64() {
        Some(id) => Ok(app.db.q1("SELECT * FROM issues WHERE id = ?", p![id])?),
        None => Ok(None),
    }
}

/// Whether Pull back can still put the item back: it hasn't moved on past what the board did.
fn can_pull(app: &App, t: &Row, d: &Value) -> Result<bool> {
    Ok(match (d["by"].as_str(), d["item"].as_str()) {
        (Some("misses"), _) => t.s("status") != Some("done") && issue_of(app, d)?.is_some_and(|i| matches!(i.s("state"), Some("open" | "task"))),
        (_, Some("question")) => t.s("status") != Some("done") && !asks_now(t),
        (_, Some("human")) => t.s("status") == Some("done") && matches!(t.s("owner_answer"), Some(human::BOARD_AGENT | human::BOARD_ANSWERED)),
        _ => false,
    })
}

/// The task waits on a question of the owner's that nobody answered yet.
fn asks_now(t: &Row) -> bool {
    t.s("status") == Some("needs") && t.s("needs_reason") == Some("question") && t.s("answered_at").is_none()
}

/// The item's ref and what it was, as the owner saw it on Needs you.
fn shown_as(t: &Row, d: &Value) -> (String, String) {
    let was = d["was"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    match (d["by"].as_str(), d["issue"].as_i64()) {
        (Some("misses"), Some(i)) => (rf("issue", i), was.unwrap_or_else(|| t.st("title"))),
        _ => (human::task_ref(t), was.unwrap_or_else(|| t.st("title"))),
    }
}

/// The why behind the verdict: the screener's reason, or where the answer came from.
fn why_of(d: &Value) -> Option<String> {
    let s = |k: &str| d[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    s("why").or_else(|| s("source").map(|src| format!("From {src}")))
}

fn row(app: &App, e: &Row) -> Result<Option<Value>> {
    let Some(t) = app.db.q1("SELECT * FROM tasks WHERE id = ?", p![e.i("task_id")])? else { return Ok(None) };
    let d = data(e);
    let (r, title) = shown_as(&t, &d);
    Ok(Some(json!({
        "id": e.id(), "at": e.v("at"), "by": d["by"], "item": d["item"], "verdict": d["verdict"],
        "text": e.v("text"), "why": why_of(&d), "ref": r, "title": title, "task": human::task_ref(&t),
        "project": t.v("project"), "goal": rf_opt("goal", t.i("goal_id")), "can_pull": can_pull(app, &t, &d)?,
    })))
}

/// `GET /handled?project=&limit=&before=&ids=`: what the board took off Needs you and hasn't been pulled
/// back, newest first. `before` is the last page's last id; `ids` asks for those records again.
pub fn list(app: &App, query: &Query) -> Result<Value> {
    let project = query.get("project").map(String::as_str).filter(|p| !p.is_empty() && *p != "all");
    let page = Paging::of(query);
    let mut sql = "SELECT e.* FROM events e JOIN tasks t ON t.id = e.task_id WHERE e.kind = ? \
                   AND json_extract(COALESCE(e.data, '{}'), '$.pulled_at') IS NULL"
        .to_string();
    let mut args = p![KIND];
    if let Some(p) = project {
        sql += " AND t.project = ?";
        args.push(json!(p));
    }
    let total = app.db.count(&sql.replacen("e.*", "COUNT(*)", 1), args.clone())? as usize;
    if let Some(ids) = query.get("ids") {
        let ids: Vec<String> = ids.split(',').filter_map(|x| x.trim().parse::<i64>().ok()).map(|i| i.to_string()).collect();
        sql += &format!(" AND e.id IN ({})", if ids.is_empty() { "0".to_string() } else { ids.join(",") });
    }
    if let Some(b) = page.before.as_deref().and_then(|b| b.parse::<i64>().ok()) {
        sql += " AND e.id < ?";
        args.push(json!(b));
    }
    sql += " ORDER BY e.id DESC";
    if let Some(l) = page.sql_limit() {
        sql += &format!(" LIMIT {l}");
    }
    let mut rows = app.db.q(&sql, args)?;
    let more = page.cut(&mut rows);
    let next = rows.last().filter(|_| more).map(|e| e.id().to_string());
    let mut items = vec![];
    for e in &rows {
        items.extend(row(app, e)?);
    }
    Ok(json!({"items": items, "total": total, "next": next}))
}

/// `POST /handled/:id/pull-back` (only from the app): puts the item back on Needs you as it was before
/// the board took it off, and takes the record off the Handled for you list.
pub fn pull_back(app: &App, id: i64) -> Result<Value> {
    let said = app.db.tx(|| {
        let Some(e) = app.db.q1("SELECT * FROM events WHERE id = ? AND kind = ?", p![id, KIND])? else {
            return err(404, "The board has no record of handling that.");
        };
        let d = data(&e);
        if d.get("pulled_at").is_some_and(|v| !v.is_null()) {
            return err(409, "It's already back on Needs you.");
        }
        let t = board::get_task(app, e.i("task_id").unwrap_or_default())?;
        let said = match (d["by"].as_str(), d["item"].as_str()) {
            (Some("misses"), _) => pull_miss(app, &t, &d)?,
            (_, Some("question")) => pull_question(app, &t, &d)?,
            (_, Some("human")) => pull_human(app, &t, &d)?,
            _ => return err(409, "The board can't put this one back on Needs you."),
        };
        // A miss's record goes with the task the board made for it; any other is stamped.
        app.db.x("UPDATE events SET data = json_set(COALESCE(data, '{}'), '$.pulled_at', ?) WHERE id = ?", p![now_iso(), id])?;
        Ok(said)
    })?;
    app.wake_runner();
    Ok(json!({"ok": true, "said": said}))
}

/// Drops a task the board made and nobody started yet, as if it was never made.
fn drop_unstarted(app: &App, t: &Row) -> Result<bool> {
    if !matches!(t.s("status"), Some("queued" | "planned")) || t.s("session_id").is_some() || t.s("started_at").is_some() {
        return Ok(false);
    }
    crate::api::purge_tasks(app, &[t.id()])?;
    if let Some(g) = t.i("goal_id") {
        board::renumber_goal(app, g)?;
    }
    crate::api::remove_md(app, &[t.id()]);
    Ok(true)
}

/// A question the board answered from the rules: asked again, so the owner's answer is the one the task goes on.
fn pull_question(app: &App, t: &Row, d: &Value) -> Result<String> {
    let r = human::task_ref(t);
    if t.s("status") == Some("done") {
        return err(409, format!("{r} finished since, so its question can't come back."));
    }
    if asks_now(t) {
        return err(409, format!("{r} is asking you something else now: answer that on Needs you."));
    }
    let asked = d["was"].as_str().filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| t.st("question"));
    let live = board::live_session(app, t)?.is_some();
    let mut ctx = board::task_context(t);
    if let (Some(a), false) = (d["answer"].as_str(), live) {
        // It hasn't read the board's answer yet: it starts from the owner's alone.
        let kept: Vec<String> = str_list(ctx.get("answers")).into_iter().filter(|x| !x.contains(&format!("“{a}”"))).collect();
        ctx.insert("answers".into(), json!(kept));
    }
    super::recheck::pin(&mut ctx);
    board::save_context(app, t.id(), &ctx, !live)?;
    board::update_task(app, t.id(), fields!["status" => "needs", "needs_reason" => "question", "question" => asked.clone(), "answered_at" => null, "lost" => 0])?;
    board::log_event(app, t.id(), board::OWNER, "question", &format!("Pulled back from Handled for you: {asked}"))?;
    if live {
        let owner = &app.cfg.owner;
        let text = format!(
            "{} {owner} pulled back the board's answer to your question (“{}”): it wasn't theirs. Hold off on what that answer said; {owner}'s own answer comes next.",
            handoff::marker(t.id()),
            short(&asked, 200)
        );
        deliver::add(app, "answer", &text, t.id(), None, None)?;
    }
    Ok(format!("{r}'s question is back on Needs you"))
}

/// A human task the board settled or gave to an agent: open again and waiting on the owner. A task the
/// board made to do it goes too when nobody started it.
fn pull_human(app: &App, t: &Row, d: &Value) -> Result<String> {
    let r = human::task_ref(t);
    if !(t.s("status") == Some("done") && matches!(t.s("owner_answer"), Some(human::BOARD_AGENT | human::BOARD_ANSWERED))) {
        return err(409, format!("{r} moved on since the board took it off, so there's nothing to pull back."));
    }
    let mut ctx = board::task_context(t);
    super::recheck::pin(&mut ctx);
    board::save_context(app, t.id(), &ctx, false)?;
    human::reopen(app, t.id())?;
    let mut said = format!("{r} is back on Needs you");
    if let Some(m) = d["made"].as_i64().map(|m| app.db.q1("SELECT * FROM tasks WHERE id = ?", p![m])).transpose()?.flatten() {
        if drop_unstarted(app, &m)? {
            said += &format!("; {} is gone", rf("task", m.id()));
        } else if m.s("status") != Some("done") {
            said += &format!("; {} already started on it", rf("task", m.id()));
        }
    }
    Ok(said)
}

/// A miss the board gave to an agent: the owner's call again. The task it made goes when nobody started it;
/// one already working is left to stop by hand first.
fn pull_miss(app: &App, t: &Row, d: &Value) -> Result<String> {
    let Some(issue) = issue_of(app, d)? else { return err(409, "That miss isn't on the board any more.") };
    let (b, r) = (rf("issue", issue.id()), rf("task", t.id()));
    if t.s("status") == Some("done") {
        return err(409, format!("{r} already finished {b}."));
    }
    if !drop_unstarted(app, t)? {
        return err(409, format!("{r} is already working on {b}. Send it back to the queue first, then pull it back."));
    }
    let issue = board::get_issue(app, issue.id())?;
    if issue.s("state") != Some("open") {
        return err(409, format!("{b} isn't open any more."));
    }
    misses::mark_owner(app, issue.id())?;
    board::add_issue_event(app, issue.id(), board::OWNER, "note", "Pulled back from Handled for you: it's yours to call", None)?;
    if let Some(g) = issue.i("goal_id") {
        blocks::miss_alert(app, issue.id(), g, &issue.st("title"))?;
    }
    Ok(format!("{b} is back on Needs you"))
}
