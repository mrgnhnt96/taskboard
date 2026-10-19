//! Ideas: things the owner may want built one day, kept per project until they're ready to plan.
//! An idea (I4) has a title and detail, notes added over time, and report rounds: each one a dig
//! into the idea against the project's code, asked with a focus and run at a commit. An idea ends
//! as the goal it became, or shelved for later, or dropped.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, p};

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS ideas(
  id INTEGER PRIMARY KEY, project TEXT NOT NULL, title TEXT NOT NULL, detail TEXT,
  state TEXT NOT NULL DEFAULT 'new', goal_id INT, source TEXT, created_at TEXT, updated_at TEXT);
CREATE INDEX IF NOT EXISTS ideas_project ON ideas(project, state);
CREATE TABLE IF NOT EXISTS idea_notes(
  id INTEGER PRIMARY KEY, idea_id INT NOT NULL, text TEXT NOT NULL, source TEXT, at TEXT);
CREATE INDEX IF NOT EXISTS idea_notes_idea ON idea_notes(idea_id);
CREATE TABLE IF NOT EXISTS idea_rounds(
  id INTEGER PRIMARY KEY, idea_id INT NOT NULL, focus TEXT, report TEXT, sha TEXT, web INT DEFAULT 0,
  started_at TEXT, finished_at TEXT, error TEXT, source TEXT);
CREATE INDEX IF NOT EXISTS idea_rounds_idea ON idea_rounds(idea_id);
"#;

pub const STATES: [&str; 6] = ["new", "digging", "reported", "planned", "shelved", "dropped"];
/// States an idea is still being thought about in; `list` shows these unless asked for others.
pub const OPEN: [&str; 3] = ["new", "digging", "reported"];

const TITLE_MAX: usize = 200;
const DETAIL_MAX: usize = 20_000;
const NOTE_MAX: usize = 4_000;

fn clean_title(s: &str) -> Result<String> {
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return err(400, "Say what the idea is: a short title.");
    }
    if t.chars().count() > TITLE_MAX {
        return err(400, format!("An idea's title is {TITLE_MAX} characters at most. Put the rest in its detail."));
    }
    Ok(t)
}

fn clean_long(s: &str, max: usize, what: &str) -> Result<String> {
    let t = s.trim().to_string();
    if t.chars().count() > max {
        return err(400, format!("{what} is {max} characters at most."));
    }
    Ok(t)
}

pub fn get(app: &App, r: &str) -> Result<Row> {
    let id = need_ref(&json!(r), "idea")?;
    app.db.q1("SELECT * FROM ideas WHERE id = ?", p![id])?
        .ok_or_else(|| ApiError::new(404, format!("There's no idea {}.", rf("idea", id))))
}

/// The idea as `list` shows it: its fields, the goal it became, and how many notes and rounds it has.
pub fn dict(app: &App, i: &Row) -> Result<Value> {
    let notes = app.db.count("SELECT COUNT(*) FROM idea_notes WHERE idea_id = ?", p![i.id()])?;
    let rounds = app.db.count("SELECT COUNT(*) FROM idea_rounds WHERE idea_id = ?", p![i.id()])?;
    Ok(json!({
        "id": i.id(), "ref": rf("idea", i.id()), "project": i.v("project"), "title": i.v("title"),
        "detail": i.st("detail"), "state": i.v("state"), "goal": rf_opt("goal", i.i("goal_id")),
        "source": i.v("source"), "created_at": i.v("created_at"), "updated_at": i.v("updated_at"),
        "notes": notes, "rounds": rounds, "planning": app.shared.lock().idea_planning.contains(&i.id()),
    }))
}

/// The idea with its notes (oldest first) and report rounds (newest first), each round with how far
/// the checkout has moved since (`behind`, `stale`).
pub fn show(app: &App, i: &Row) -> Result<Value> {
    let mut v = dict(app, i)?;
    let notes: Vec<Value> = app
        .db
        .q("SELECT * FROM idea_notes WHERE idea_id = ? ORDER BY id", p![i.id()])?
        .iter()
        .map(|n| json!({"id": n.id(), "text": n.v("text"), "source": n.v("source"), "at": n.v("at")}))
        .collect();
    let mut rounds: Vec<Value> =
        app.db.q("SELECT * FROM idea_rounds WHERE idea_id = ? ORDER BY id DESC", p![i.id()])?.iter().map(round_dict).collect();
    crate::dig::mark_drift(app, &i.st("project"), &mut rounds)?;
    v["notes"] = json!(notes);
    v["rounds"] = json!(rounds);
    Ok(v)
}

fn round_dict(r: &Row) -> Value {
    let report = r.s("report").and_then(|s| serde_json::from_str::<Value>(s).ok()).unwrap_or(Value::Null);
    json!({
        "id": r.id(), "focus": r.st("focus"), "report": report, "sha": r.v("sha"), "web": r.b("web"),
        "started_at": r.v("started_at"), "finished_at": r.v("finished_at"), "error": r.v("error"), "source": r.v("source"),
    })
}

/// The idea's project: the body's, else the caller's task's, else the project of the folder `tb` ran in.
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
    let title = clean_title(&body_str(body, "title"))?;
    let detail = clean_long(&body_str(body, "detail"), DETAIL_MAX, "An idea's detail")?;
    let Some(project) = project_for(app, body)? else {
        return err(400, "Say which project the idea is for (--project).");
    };
    let now = now_iso();
    let id = app.db.insert(
        "ideas",
        fields!["project" => project, "title" => title, "detail" => detail, "state" => "new",
                "source" => board::caller(app, body, board::OWNER), "created_at" => now, "updated_at" => now],
    )?;
    show(app, &get(app, &id.to_string())?)
}

fn set(app: &App, r: &str, body: &Value) -> Result<Value> {
    let i = get(app, r)?;
    let mut f: Vec<(&str, Value)> = vec![];
    if body_has(body, "title") {
        f.push(("title", json!(clean_title(&body_str(body, "title"))?)));
    }
    if body_has(body, "detail") {
        f.push(("detail", json!(clean_long(&body_str(body, "detail"), DETAIL_MAX, "An idea's detail")?)));
    }
    if body_has(body, "project") {
        let p = body_str(body, "project");
        if p.is_empty() {
            return err(400, "An idea is always in a project: say which.");
        }
        f.push(("project", json!(p)));
    }
    if f.is_empty() {
        return err(400, "Say what to change: title, detail or project.");
    }
    f.push(("updated_at", json!(now_iso())));
    app.db.update("ideas", &json!(i.id()), f)?;
    show(app, &get(app, r)?)
}

fn add_note(app: &App, i: &Row, text: &str, body: &Value) -> Result<()> {
    let now = now_iso();
    app.db.insert("idea_notes", fields!["idea_id" => i.id(), "text" => text, "source" => board::caller(app, body, board::OWNER), "at" => now])?;
    app.db.update("ideas", &json!(i.id()), fields!["updated_at" => now])
}

fn note(app: &App, r: &str, body: &Value) -> Result<Value> {
    let i = get(app, r)?;
    let text = clean_long(&body_str(body, "text"), NOTE_MAX, "A note")?;
    if text.is_empty() {
        return err(400, "Say what the note is.");
    }
    add_note(app, &i, &text, body)?;
    show(app, &get(app, r)?)
}

/// Shelve or drop an idea (with an optional reason, kept as a note), or reopen one that was. A
/// reopened idea is reported again when it has a finished report, else new.
fn move_to(app: &App, r: &str, to: &str, body: &Value) -> Result<Value> {
    let i = get(app, r)?;
    let at = i.st("state");
    let me = rf("idea", i.id());
    if at == "planned" {
        let g = rf_opt("goal", i.i("goal_id"));
        return err(409, format!("{me} became {} already; work on the goal instead.", g.as_str().unwrap_or("a goal")));
    }
    if at == "digging" {
        return err(409, format!("{me} is being dug into; wait for the report first."));
    }
    if app.shared.lock().idea_planning.contains(&i.id()) {
        return err(409, format!("{me} is being planned into a goal; wait for it."));
    }
    let state = if to == "reopen" {
        if OPEN.contains(&at.as_str()) {
            return show(app, &i);
        }
        let reported = app.db.count("SELECT COUNT(*) FROM idea_rounds WHERE idea_id = ? AND report IS NOT NULL", p![i.id()])? > 0;
        if reported { "reported" } else { "new" }
    } else {
        to
    };
    if state == at {
        return show(app, &i);
    }
    let reason = one_line(&body_str(body, "reason"), NOTE_MAX);
    if !reason.is_empty() {
        let word = match state {
            "shelved" => "Shelved",
            "dropped" => "Dropped",
            _ => "Reopened",
        };
        add_note(app, &i, &format!("{word}: {reason}"), body)?;
    }
    app.db.update("ideas", &json!(i.id()), fields!["state" => state, "updated_at" => now_iso()])?;
    show(app, &get(app, r)?)
}

/// `GET /ideas`: a project's ideas (`project=all` for every project; else the folder `tb` ran in),
/// the open ones unless `state` asks for others (`all`, or one state), newest change first.
fn list(app: &App, query: &crate::api::Query) -> Result<Value> {
    let q = |k: &str| query.get(k).map(|s| s.as_str()).filter(|s| !s.is_empty());
    let every = q("project") == Some("all");
    let project = match q("project").filter(|p| *p != "all") {
        Some(p) => Some(p.to_string()),
        None if every => None,
        None => crate::projects::project_for_path(app, q("cwd"))?,
    };
    let state = q("state").unwrap_or("open");
    if state != "open" && state != "all" && !STATES.contains(&state) {
        return err(400, format!("An idea's state is open, all or one of {}.", STATES.join(", ")));
    }
    let rows = app.db.q("SELECT * FROM ideas WHERE ? IS NULL OR project = ? ORDER BY updated_at DESC, id DESC", p![project, project])?;
    let mut out = vec![];
    for i in rows.iter().filter(|i| {
        let s = i.s("state").unwrap_or("");
        match state {
            "all" => true,
            "open" => OPEN.contains(&s),
            st => s == st,
        }
    }) {
        out.push(dict(app, i)?);
    }
    Ok(json!({"project": project, "ideas": out}))
}

/// `/ideas…` routes.
pub fn route(app: &App, method: &str, rest: &[&str], query: &crate::api::Query, body: &Value) -> Result<Value> {
    crate::dig::settle_lost(app)?;
    match (method, rest) {
        ("GET", []) => list(app, query),
        ("POST", []) => app.db.tx(|| add(app, body)),
        ("GET", [r]) => show(app, &get(app, r)?),
        ("POST", [r]) => app.db.tx(|| set(app, r, body)),
        ("POST", [r, "note"]) => app.db.tx(|| note(app, r, body)),
        ("POST", [r, "shelve"]) => app.db.tx(|| move_to(app, r, "shelved", body)),
        ("POST", [r, "drop"]) => app.db.tx(|| move_to(app, r, "dropped", body)),
        ("POST", [r, "reopen"]) => app.db.tx(|| move_to(app, r, "reopen", body)),
        ("POST", [r, "dig"]) => crate::dig::start(app, r, body),
        ("POST", [r, "plan"]) => crate::idea_plan::start(app, r, body),
        _ => err(404, "There's nothing at that address."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_input() {
        assert_eq!(clean_title("  a   better  inbox ").unwrap(), "a better inbox");
        assert!(clean_title(" ").is_err());
        assert!(clean_title(&"x".repeat(TITLE_MAX + 1)).is_err());
        assert_eq!(clean_long("  keep\n\nlines  ", 100, "Detail").unwrap(), "keep\n\nlines");
        assert!(clean_long(&"x".repeat(101), 100, "Detail").is_err());
    }
}
