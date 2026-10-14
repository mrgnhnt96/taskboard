//! Lessons: what the board has learned from finished work, kept past the goal it came from. A goal
//! note reaches only that goal's tasks; a lesson reaches every later task in its project (or, when
//! global, on the board), and planning lessons reach whoever plans a goal. Agents suggest lessons
//! (`tb lesson add`); only kept ones reach handoffs and planners, and only the owner keeps or
//! retires one. Each lesson keeps the refs of the work it came from (T12, G3) as its evidence, and
//! is confirmed again when later work shows the same thing.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, p};

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS lessons(
  id INTEGER PRIMARY KEY, scope TEXT NOT NULL DEFAULT 'project', project TEXT, kind TEXT NOT NULL,
  text TEXT NOT NULL, evidence TEXT DEFAULT '[]', state TEXT NOT NULL DEFAULT 'suggested',
  confirmations INT DEFAULT 0, last_confirmed TEXT, source TEXT,
  decided_at TEXT, decided_by TEXT, created_at TEXT, updated_at TEXT);
CREATE INDEX IF NOT EXISTS lessons_project ON lessons(project, state);
"#;

pub const KINDS: [&str; 4] = ["planning", "code", "process", "tooling"];
const STATES: [&str; 3] = ["suggested", "kept", "retired"];

/// A lesson is a sentence or two; anything longer belongs in a doc it can point to.
const TEXT_MAX: usize = 400;
/// At most this many lessons, and this many characters of them, go into one prompt.
const PROMPT_COUNT: usize = 8;
const PROMPT_CHARS: usize = 1500;

pub(crate) fn clean_text(s: &str) -> Result<String> {
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return err(400, "Say what the lesson is.");
    }
    if t.chars().count() > TEXT_MAX {
        return err(400, format!("A lesson is a sentence or two, {TEXT_MAX} characters at most. Put the detail in a doc and point to it."));
    }
    Ok(t)
}

pub(crate) fn clean_kind(s: &str) -> Result<String> {
    let k = s.trim().to_lowercase();
    if !KINDS.contains(&k.as_str()) {
        return err(400, format!("A lesson's kind is one of {}.", KINDS.join(", ")));
    }
    Ok(k)
}

/// Task and goal refs (T12, G3), cleaned and in a stable form.
fn clean_evidence(v: Option<&Value>) -> Result<Vec<String>> {
    let mut out: Vec<String> = vec![];
    for s in str_list(v).iter().flat_map(|s| s.replace(',', " ").split_whitespace().map(|x| x.to_string()).collect::<Vec<_>>()) {
        let r = match s.chars().next().map(|c| c.to_ascii_uppercase()) {
            Some('T') => rf("task", need_ref(&json!(s), "task")?),
            Some('G') => rf("goal", need_ref(&json!(s), "goal")?),
            Some('B') => rf("issue", need_ref(&json!(s), "issue")?),
            _ => return err(400, format!("“{s}” isn't a task, goal or backlog ref (T12, G3, B7).")),
        };
        if !out.contains(&r) {
            out.push(r);
        }
    }
    Ok(out)
}

pub(crate) fn evidence(l: &Row) -> Vec<String> {
    jloads_arr(l.s("evidence")).iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
}

pub fn get(app: &App, r: &str) -> Result<Row> {
    let id = need_ref(&json!(r), "lesson")?;
    app.db.q1("SELECT * FROM lessons WHERE id = ?", p![id])?
        .ok_or_else(|| ApiError::new(404, format!("There's no lesson {}.", rf("lesson", id))))
}

pub fn dict(l: &Row) -> Value {
    json!({
        "id": l.id(), "ref": rf("lesson", l.id()), "scope": l.v("scope"), "project": l.v("project"),
        "kind": l.v("kind"), "text": l.v("text"), "evidence": evidence(l), "state": l.v("state"),
        "confirmations": l.i0("confirmations"), "last_confirmed": l.v("last_confirmed"), "source": l.v("source"),
        "decided_at": l.v("decided_at"), "decided_by": l.v("decided_by"),
        "created_at": l.v("created_at"), "updated_at": l.v("updated_at"),
    })
}

/// The lesson's project: the body's, else the caller's task's, else the project of the folder `tb` ran in.
fn project_for(app: &App, body: &Value) -> Result<Option<String>> {
    let p = body_str(body, "project");
    if !p.is_empty() {
        return Ok(Some(p));
    }
    if let Some(t) = board::task_for_session(app, body["by_session"].as_str())? {
        if let Some(p) = t.s("project").filter(|p| !p.is_empty()) {
            return Ok(Some(p.to_string()));
        }
    }
    crate::projects::project_for_path(app, body["cwd"].as_str())
}

fn add(app: &App, body: &Value) -> Result<Value> {
    let text = clean_text(&body_str(body, "text"))?;
    let kind = clean_kind(&body_str(body, "kind"))?;
    let global = body_str(body, "scope") == "global";
    let project = if global { None } else { project_for(app, body)? };
    if !global && project.is_none() {
        return err(400, "Say which project the lesson is for (--project), or make it global (--global).");
    }
    let mut ev = clean_evidence(body.get("evidence"))?;
    // Suggested from a task: that task is where it was learned.
    if let Some(t) = board::task_for_session(app, body["by_session"].as_str())? {
        let r = rf("task", t.id());
        if !ev.contains(&r) {
            ev.insert(0, r);
        }
    }
    let who = board::caller(app, body, board::OWNER);
    let kept = as_bool(body.get("kept"), false);
    let now = now_iso();
    let id = app.db.insert(
        "lessons",
        fields!["scope" => if global { "global" } else { "project" }, "project" => project, "kind" => kind,
                "text" => text, "evidence" => jdumps(&json!(ev)), "state" => if kept { "kept" } else { "suggested" },
                "source" => who.clone(), "decided_at" => if kept { Some(now.clone()) } else { None },
                "decided_by" => if kept { Some(who) } else { None }, "created_at" => now, "updated_at" => now],
    )?;
    Ok(dict(&get(app, &id.to_string())?))
}

fn set(app: &App, r: &str, body: &Value) -> Result<Value> {
    let l = get(app, r)?;
    let mut f: Vec<(&str, Value)> = vec![];
    if body_has(body, "text") {
        f.push(("text", json!(clean_text(&body_str(body, "text"))?)));
    }
    if body_has(body, "kind") {
        f.push(("kind", json!(clean_kind(&body_str(body, "kind"))?)));
    }
    match body_str(body, "scope").as_str() {
        "" => {}
        "global" => {
            f.push(("scope", json!("global")));
            f.push(("project", Value::Null));
        }
        "project" => {
            let p = project_for(app, body)?.or_else(|| l.s("project").map(|s| s.to_string()));
            let Some(p) = p else { return err(400, "Say which project the lesson is for (--project).") };
            f.push(("scope", json!("project")));
            f.push(("project", json!(p)));
        }
        s => return err(400, format!("A lesson is for a project or global, not “{s}”.")),
    }
    if body.get("evidence").is_some() {
        let mut ev = evidence(&l);
        for r in clean_evidence(body.get("evidence"))? {
            if !ev.contains(&r) {
                ev.push(r);
            }
        }
        f.push(("evidence", json!(jdumps(&json!(ev)))));
    }
    if f.is_empty() {
        return err(400, "Say what to change: text, kind, scope, project or evidence.");
    }
    f.push(("updated_at", json!(now_iso())));
    app.db.update("lessons", &json!(l.id()), f)?;
    Ok(dict(&get(app, r)?))
}

/// Keep or retire a lesson (on the owner's word), or put it back to suggested.
fn decide(app: &App, r: &str, state: &str, body: &Value) -> Result<Value> {
    let l = get(app, r)?;
    if !STATES.contains(&state) {
        return err(400, "A lesson is suggested, kept or retired.");
    }
    if l.s("state") != Some(state) {
        let now = now_iso();
        let who = board::caller(app, body, board::OWNER);
        app.db.update("lessons", &json!(l.id()), fields!["state" => state, "decided_at" => now, "decided_by" => who, "updated_at" => now])?;
    }
    Ok(dict(&get(app, r)?))
}

/// Later work showed the same thing again: count it and add that work to the evidence.
fn confirm(app: &App, r: &str, body: &Value) -> Result<Value> {
    let l = get(app, r)?;
    confirm_with(app, &l, &clean_evidence(body.get("evidence"))?)?;
    Ok(dict(&get(app, r)?))
}

pub(crate) fn confirm_with(app: &App, l: &Row, more: &[String]) -> Result<()> {
    let mut ev = evidence(l);
    for r in more {
        if !ev.contains(r) {
            ev.push(r.clone());
        }
    }
    let now = now_iso();
    app.db.update(
        "lessons",
        &json!(l.id()),
        fields!["confirmations" => l.i0("confirmations") + 1, "last_confirmed" => now, "evidence" => jdumps(&json!(ev)), "updated_at" => now],
    )?;
    Ok(())
}

/// A lesson the board itself suggests (from a goal's retrospective), for the owner to keep or retire.
pub(crate) fn suggest(app: &App, project: Option<&str>, kind: &str, text: &str, ev: &[String], source: &str) -> Result<i64> {
    let now = now_iso();
    app.db.insert(
        "lessons",
        fields!["scope" => if project.is_some() { "project" } else { "global" }, "project" => project, "kind" => clean_kind(kind)?,
                "text" => clean_text(text)?, "evidence" => jdumps(&json!(ev)), "state" => "suggested", "source" => source,
                "created_at" => now, "updated_at" => now],
    )
}

fn list(app: &App, query: &crate::api::Query) -> Result<Value> {
    let q = |k: &str| query.get(k).map(|s| s.as_str()).filter(|s| !s.is_empty() && *s != "all");
    let project = match q("project") {
        Some(p) => Some(p.to_string()),
        None => crate::projects::project_for_path(app, q("cwd"))?,
    };
    let rows = app.db.q("SELECT * FROM lessons ORDER BY state = 'retired', confirmations DESC, id DESC", p![])?;
    let every = query.get("project").map(|s| s == "all").unwrap_or(false);
    let rows: Vec<Value> = rows
        .iter()
        .filter(|l| every || project.is_none() || l.s("scope") == Some("global") || l.s("project") == project.as_deref())
        .filter(|l| match q("state") {
            Some(s) => l.s("state") == Some(s),
            None => l.s("state") != Some("retired"),
        })
        .filter(|l| q("kind").map(|k| l.s("kind") == Some(k)).unwrap_or(true))
        .map(dict)
        .collect();
    Ok(json!({"project": project, "lessons": rows}))
}

/// `/lessons…` routes.
pub fn route(app: &App, method: &str, rest: &[&str], query: &crate::api::Query, body: &Value) -> Result<Value> {
    match (method, rest) {
        ("GET", []) => list(app, query),
        ("POST", []) => app.db.tx(|| add(app, body)),
        ("GET", [r]) => Ok(dict(&get(app, r)?)),
        ("POST", [r]) => app.db.tx(|| set(app, r, body)),
        ("POST", [r, "keep"]) => app.db.tx(|| decide(app, r, "kept", body)),
        ("POST", [r, "retire"]) => app.db.tx(|| decide(app, r, "retired", body)),
        ("POST", [r, "suggest"]) => app.db.tx(|| decide(app, r, "suggested", body)),
        ("POST", [r, "confirm"]) => app.db.tx(|| confirm(app, r, body)),
        ("POST", ["retro", g]) => crate::retro::start(app, g),
        ("GET", ["retro", g]) => crate::retro::status(app, g),
        _ => err(404, "There's nothing at that address."),
    }
}

/// The kept lessons for a project's work (its own and the board's global ones), most confirmed first.
pub fn kept(app: &App, project: Option<&str>, kinds: &[&str]) -> Result<Vec<Row>> {
    let rows = app.db.q(
        "SELECT * FROM lessons WHERE state = 'kept' AND (scope = 'global' OR project IS ?)
         ORDER BY confirmations DESC, COALESCE(last_confirmed, decided_at, created_at) DESC, id DESC",
        p![project],
    )?;
    Ok(rows.into_iter().filter(|l| kinds.contains(&l.s("kind").unwrap_or(""))).collect())
}

/// Lessons as prompt lines, capped in count and length so they never crowd out the task.
pub fn lines(rows: &[Row]) -> Vec<String> {
    let mut out = vec![];
    let mut used = 0;
    for l in rows.iter().take(PROMPT_COUNT) {
        let line = format!("- {} ({})", l.st("text").trim(), rf("lesson", l.id()));
        if used + line.chars().count() + 1 > PROMPT_CHARS {
            break;
        }
        used += line.chars().count() + 1;
        out.push(line);
    }
    out
}

/// The handoff's lessons: what earlier work in this project (and on the board) taught about the
/// code, the process and the tools. Planning lessons go to planners instead.
pub fn handoff_block(app: &App, t: &Row) -> Result<Vec<String>> {
    let ls = lines(&kept(app, t.s("project"), &["code", "process", "tooling"])?);
    if ls.is_empty() {
        return Ok(vec![]);
    }
    Ok(vec![format!("Lessons from earlier work here:\n{}", ls.join("\n"))])
}

/// The planning lessons for a planner's prompt, or nothing.
pub fn planning_block(app: &App, project: Option<&str>) -> Result<Option<String>> {
    let ls = lines(&kept(app, project, &["planning"])?);
    if ls.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("What earlier goals taught about planning here:\n{}", ls.join("\n"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_input() {
        assert_eq!(clean_text("  two   words ").unwrap(), "two words");
        assert!(clean_text("").is_err());
        assert!(clean_text(&"x".repeat(TEXT_MAX + 1)).is_err());
        assert_eq!(clean_kind("Code").unwrap(), "code");
        assert!(clean_kind("vibes").is_err());
        assert_eq!(clean_evidence(Some(&json!(["t12, G3", "T12", "b7"]))).unwrap(), vec!["T12", "G3", "B7"]);
        assert!(clean_evidence(Some(&json!(["X1"]))).is_err());
    }
}
