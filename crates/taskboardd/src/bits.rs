//! Bits: the feature flags a goal's work sits behind. A bit is local (defined in the code only) or
//! backend (it has to exist in the flag tool too). Bits are linked to tasks and goals; a task waits
//! to start until its backend bits are made in the tool (`tb bit made`), and a goal whose tasks are
//! all done still waits on its unmade backend bits. The handoff lists a task's bits. `[bits]` in
//! config.toml names the tool and its "new flag" link.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, p};

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS bits(
  id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL, kind TEXT NOT NULL DEFAULT 'backend', project TEXT, note TEXT,
  made_at TEXT, made_by TEXT, created_at TEXT, updated_at TEXT);
CREATE TABLE IF NOT EXISTS bit_links(bit_id INT NOT NULL, task_id INT, goal_id INT, at TEXT);
CREATE INDEX IF NOT EXISTS bit_links_bit ON bit_links(bit_id);
CREATE INDEX IF NOT EXISTS bit_links_task ON bit_links(task_id);
CREATE INDEX IF NOT EXISTS bit_links_goal ON bit_links(goal_id);
"#;

/// `[bits]` in config.toml.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BitsConfig {
    /// The flag tool's name, as the app's "Create in <tool>" and "Local, not in <tool>" say it.
    pub tool: String,
    /// The tool's "new flag" page. `{name}` is the bit's name and `{project}` its project (both URL-encoded).
    /// Empty: no create link.
    pub create_url: String,
}

const KINDS: [&str; 2] = ["backend", "local"];

fn tool(app: &App) -> String {
    let t = app.cfg.bits.tool.trim();
    if t.is_empty() { "the flag tool".into() } else { t.to_string() }
}

fn clean_name(s: &str) -> Result<String> {
    let v = s.trim().to_string();
    let ok = !v.is_empty()
        && v.chars().count() <= 100
        && v.chars().next().map(|c| c.is_ascii_alphanumeric()).unwrap_or(false)
        && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':'));
    if !ok {
        return err(400, format!("“{v}” can't be a bit's name. Use the flag's key: letters, numbers, dots, dashes, colons or underscores."));
    }
    Ok(v)
}

fn clean_kind(s: &str) -> Result<String> {
    let k = s.trim().to_lowercase();
    if !KINDS.contains(&k.as_str()) {
        return err(400, "A bit is local (in the code only) or backend (in the flag tool too).");
    }
    Ok(k)
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The tool's "new flag" link for a backend bit, from `[bits] create_url`.
pub fn create_url(app: &App, b: &Row) -> Value {
    let u = app.cfg.bits.create_url.trim();
    if u.is_empty() || b.s("kind") != Some("backend") {
        return Value::Null;
    }
    json!(u.replace("{name}", &enc(&b.st("name"))).replace("{project}", &enc(&b.st("project"))))
}

pub fn get(app: &App, name: &str) -> Result<Row> {
    match app.db.q1("SELECT * FROM bits WHERE name = ?", p![name.trim()])? {
        Some(b) => Ok(b),
        None => err(404, format!("There's no bit called {}. Add it with tb bit add {} --backend (or --local).", name.trim(), name.trim())),
    }
}

fn waits(b: &Row) -> bool {
    b.s("kind") == Some("backend") && b.s("made_at").is_none()
}

fn links(app: &App, bit_id: i64) -> Result<(Vec<Value>, Vec<Value>)> {
    let rows = app.db.q("SELECT task_id, goal_id FROM bit_links WHERE bit_id = ? ORDER BY rowid", p![bit_id])?;
    let tasks = rows.iter().filter_map(|r| r.i("task_id")).map(|n| json!(rf("task", n))).collect();
    let goals = rows.iter().filter_map(|r| r.i("goal_id")).map(|n| json!(rf("goal", n))).collect();
    Ok((tasks, goals))
}

pub fn dict(app: &App, b: &Row) -> Result<Value> {
    let (tasks, goals) = links(app, b.id())?;
    Ok(json!({
        "id": b.id(), "name": b.v("name"), "kind": b.v("kind"), "project": b.v("project"), "note": b.v("note"),
        "made": b.s("made_at").is_some(), "made_at": b.v("made_at"), "made_by": b.v("made_by"),
        "waiting": waits(b), "create_url": create_url(app, b), "tasks": tasks, "goals": goals,
        "created_at": b.v("created_at"),
    }))
}

/// The bits linked to a task.
pub fn of_task(app: &App, task_id: i64) -> Result<Vec<Row>> {
    app.db.q("SELECT b.* FROM bits b WHERE b.id IN (SELECT bit_id FROM bit_links WHERE task_id = ?) ORDER BY b.name", p![task_id])
}

/// The bits of a goal: linked to it or to a task that counts toward it.
pub fn of_goal(app: &App, goal_id: i64) -> Result<Vec<Row>> {
    let ids: Vec<i64> = crate::shared::counted_tasks(app, goal_id)?.iter().map(|t| t.id()).collect();
    let mut out = app.db.q("SELECT b.* FROM bits b WHERE b.id IN (SELECT bit_id FROM bit_links WHERE goal_id = ?) ORDER BY b.name", p![goal_id])?;
    for tid in ids {
        for b in of_task(app, tid)? {
            if !out.iter().any(|x| x.id() == b.id()) {
                out.push(b);
            }
        }
    }
    out.sort_by_key(|b| b.st("name"));
    Ok(out)
}

/// The goal's unmade backend bits.
pub fn goal_waiting(app: &App, goal_id: i64) -> Result<i64> {
    Ok(of_goal(app, goal_id)?.iter().filter(|b| waits(b)).count() as i64)
}

/// A goal's bits, for its page: the list and "N of M created".
pub fn goal_card(app: &App, goal_id: i64) -> Result<Value> {
    let all = of_goal(app, goal_id)?;
    let backend: Vec<&Row> = all.iter().filter(|b| b.s("kind") == Some("backend")).collect();
    let made = backend.iter().filter(|b| b.s("made_at").is_some()).count();
    Ok(json!({
        "list": all.iter().map(|b| dict(app, b)).collect::<Result<Vec<_>>>()?,
        "backend": backend.len(), "made": made, "waiting": backend.len() - made, "tool": tool(app),
    }))
}

/// A task card's bits: name, kind and whether it's made.
pub fn task_card(app: &App, task_id: i64) -> Result<Vec<Value>> {
    Ok(of_task(app, task_id)?
        .iter()
        .map(|b| json!({"name": b.v("name"), "kind": b.v("kind"), "made": b.s("made_at").is_some(), "waiting": waits(b)}))
        .collect())
}

/// Why its bits hold this queued task back: a backend bit not made in the flag tool yet.
pub fn blocker(app: &App, t: &Row) -> Result<Option<String>> {
    let open: Vec<String> = of_task(app, t.id())?.iter().filter(|b| waits(b)).map(|b| b.st("name")).collect();
    Ok(match open.len() {
        0 => None,
        1 => Some(format!("Waits for the bit {} to be made in {}", open[0], tool(app))),
        n => Some(format!("Waits for {n} bits to be made in {}: {}", tool(app), open.join(", "))),
    })
}

fn names_from(v: Option<&Value>) -> Vec<String> {
    str_list(v).iter().flat_map(|s| s.replace(',', " ").split_whitespace().map(|x| x.to_string()).collect::<Vec<_>>()).collect()
}

fn link(app: &App, b: &Row, task: Option<i64>, goal: Option<i64>) -> Result<bool> {
    let have = app.db.count(
        "SELECT COUNT(*) FROM bit_links WHERE bit_id = ? AND task_id IS ? AND goal_id IS ?",
        p![b.id(), task, goal],
    )?;
    if have > 0 {
        return Ok(false);
    }
    if let Some(t) = task {
        board::get_task(app, t)?;
    }
    if let Some(g) = goal {
        board::get_goal(app, g)?;
    }
    app.db.insert("bit_links", fields!["bit_id" => b.id(), "task_id" => task, "goal_id" => goal, "at" => now_iso()])?;
    Ok(true)
}

fn unlink(app: &App, b: &Row, task: Option<i64>, goal: Option<i64>) -> Result<bool> {
    let n = app.db.count("SELECT COUNT(*) FROM bit_links WHERE bit_id = ? AND task_id IS ? AND goal_id IS ?", p![b.id(), task, goal])?;
    app.db.x("DELETE FROM bit_links WHERE bit_id = ? AND task_id IS ? AND goal_id IS ?", p![b.id(), task, goal])?;
    Ok(n > 0)
}

fn refs(v: Option<&Value>, kind: &str) -> Result<Vec<i64>> {
    names_from(v).iter().filter(|s| !s.eq_ignore_ascii_case("none")).map(|s| need_ref(&json!(s), kind)).collect()
}

/// Links a task to bits from `body.bits` (names; `none` unlinks all) and unlinks `body.not_bits`.
/// True when its bits changed.
pub fn take_task_bits(app: &App, task_id: i64, body: &Value, who: &str) -> Result<bool> {
    if body.get("bits").is_none() && body.get("not_bits").is_none() {
        return Ok(false);
    }
    let mut changed: Vec<String> = vec![];
    let add = names_from(body.get("bits"));
    if add.len() == 1 && add[0].eq_ignore_ascii_case("none") {
        for b in of_task(app, task_id)? {
            unlink(app, &b, Some(task_id), None)?;
            changed.push(format!("no longer uses the bit {}", b.st("name")));
        }
    } else {
        for n in add {
            let b = get(app, &n)?;
            if link(app, &b, Some(task_id), None)? {
                changed.push(format!("uses the bit {}", b.st("name")));
            }
        }
    }
    for n in names_from(body.get("not_bits")) {
        let b = get(app, &n)?;
        if unlink(app, &b, Some(task_id), None)? {
            changed.push(format!("no longer uses the bit {}", b.st("name")));
        }
    }
    if changed.is_empty() {
        return Ok(false);
    }
    let text = changed.join("; ");
    board::log_event(app, task_id, who, "note", &(text[..1].to_uppercase() + &text[1..]))?;
    board::bump_ctx(app, task_id)?;
    Ok(true)
}

fn log_on_tasks(app: &App, b: &Row, who: &str, text: &str) -> Result<()> {
    for r in app.db.q("SELECT task_id FROM bit_links WHERE bit_id = ? AND task_id IS NOT NULL", p![b.id()])? {
        if let Some(t) = r.i("task_id") {
            if board::find_task(app, Some(t))?.is_some() {
                board::log_event(app, t, who, "note", text)?;
                board::bump_ctx(app, t)?;
            }
        }
    }
    Ok(())
}

fn project_for(app: &App, body: &Value, tasks: &[i64], goals: &[i64]) -> Result<Option<String>> {
    let p = body_str(body, "project");
    if !p.is_empty() {
        return Ok(Some(p));
    }
    if let Some(t) = tasks.first() {
        return Ok(board::get_task(app, *t)?.s("project").map(|s| s.to_string()));
    }
    if let Some(g) = goals.first() {
        return Ok(board::get_goal(app, *g)?.s("project").map(|s| s.to_string()));
    }
    Ok(None)
}

fn who_of(body: &Value) -> String {
    let w = body_str(body, "who");
    if w.is_empty() { board::OWNER.to_string() } else { w }
}

fn add(app: &App, body: &Value) -> Result<Value> {
    let name = clean_name(&body_str(body, "name"))?;
    if app.db.q1("SELECT id FROM bits WHERE name = ?", p![name])?.is_some() {
        return err(409, format!("There's already a bit called {name}. Change it with tb bit set {name}."));
    }
    let kind = clean_kind(&{ let k = body_str(body, "kind"); if k.is_empty() { "backend".to_string() } else { k } })?;
    let tasks = refs(body.get("tasks"), "task")?;
    let goals = refs(body.get("goals"), "goal")?;
    let now = now_iso();
    let note = one_line(&body_str(body, "note"), 300);
    app.db.insert(
        "bits",
        fields!["name" => name, "kind" => kind, "project" => project_for(app, body, &tasks, &goals)?,
                "note" => if note.is_empty() { None } else { Some(note) }, "created_at" => now, "updated_at" => now],
    )?;
    let b = get(app, &name)?;
    for t in &tasks {
        link(app, &b, Some(*t), None)?;
    }
    for g in &goals {
        link(app, &b, None, Some(*g))?;
    }
    let who = who_of(body);
    log_on_tasks(app, &b, &who, &format!("Uses the {kind} bit {name}"))?;
    dict(app, &b)
}

fn set(app: &App, name: &str, body: &Value) -> Result<Value> {
    let b = get(app, name)?;
    let who = who_of(body);
    let mut f: Vec<(&str, Value)> = vec![];
    let mut name = b.st("name");
    if body_has(body, "name") {
        let n = clean_name(&body_str(body, "name"))?;
        if n != name {
            if app.db.q1("SELECT id FROM bits WHERE name = ?", p![n])?.is_some() {
                return err(409, format!("There's already a bit called {n}."));
            }
            f.push(("name", json!(n)));
            name = n;
        }
    }
    if body_has(body, "kind") {
        let k = clean_kind(&body_str(body, "kind"))?;
        if Some(k.as_str()) != b.s("kind") {
            f.push(("kind", json!(k)));
        }
    }
    if body.get("note").is_some() {
        let v = one_line(&body_str(body, "note"), 300);
        f.push(("note", if v.is_empty() { Value::Null } else { json!(v) }));
    }
    if body_has(body, "project") {
        f.push(("project", json!(body_str(body, "project"))));
    }
    let mut linked = false;
    for t in refs(body.get("tasks"), "task")? {
        linked |= link(app, &b, Some(t), None)?;
    }
    for t in refs(body.get("not_tasks"), "task")? {
        linked |= unlink(app, &b, Some(t), None)?;
    }
    for g in refs(body.get("goals"), "goal")? {
        linked |= link(app, &b, None, Some(g))?;
    }
    for g in refs(body.get("not_goals"), "goal")? {
        linked |= unlink(app, &b, None, Some(g))?;
    }
    if f.is_empty() && !linked {
        return err(400, "Say what to change: name, kind, note, project, tasks or goals.");
    }
    if !f.is_empty() {
        f.push(("updated_at", json!(now_iso())));
        app.db.update("bits", &json!(b.id()), f)?;
    }
    let nb = get(app, &name)?;
    log_on_tasks(app, &nb, &who, &format!("The bit {name} changed"))?;
    dict(app, &nb)
}

/// The bit is made in the flag tool (or, with `undo`, not after all).
fn made(app: &App, name: &str, body: &Value) -> Result<Value> {
    let b = get(app, name)?;
    let who = who_of(body);
    let undo = as_bool(body.get("undo"), false);
    if undo {
        if b.s("made_at").is_none() {
            return dict(app, &b);
        }
        app.db.update("bits", &json!(b.id()), fields!["made_at" => null, "made_by" => null, "updated_at" => now_iso()])?;
        log_on_tasks(app, &b, &who, &format!("The bit {} isn't made in {} after all", b.st("name"), tool(app)))?;
    } else {
        if b.s("made_at").is_some() {
            return dict(app, &b);
        }
        app.db.update("bits", &json!(b.id()), fields!["made_at" => now_iso(), "made_by" => who.clone(), "updated_at" => now_iso()])?;
        log_on_tasks(app, &b, &who, &format!("The bit {} is made in {}", b.st("name"), tool(app)))?;
    }
    dict(app, &get(app, name)?)
}

fn remove(app: &App, name: &str, body: &Value) -> Result<Value> {
    let b = get(app, name)?;
    log_on_tasks(app, &b, &who_of(body), &format!("No longer uses the bit {}", b.st("name")))?;
    app.db.x("DELETE FROM bit_links WHERE bit_id = ?", p![b.id()])?;
    app.db.x("DELETE FROM bits WHERE id = ?", p![b.id()])?;
    Ok(json!({"ok": true, "removed": b.v("name")}))
}

fn list(app: &App, query: &crate::api::Query) -> Result<Value> {
    let q = |k: &str| query.get(k).map(|s| s.as_str()).filter(|s| !s.is_empty() && *s != "all");
    let rows = if let Some(g) = q("goal") {
        of_goal(app, need_ref(&json!(g), "goal")?)?
    } else if let Some(t) = q("task") {
        of_task(app, need_ref(&json!(t), "task")?)?
    } else {
        app.db.q("SELECT * FROM bits ORDER BY name", p![])?
    };
    let rows: Vec<&Row> = rows.iter().filter(|b| q("project").map(|p| b.s("project") == Some(p)).unwrap_or(true)).collect();
    Ok(json!({"bits": rows.iter().map(|b| dict(app, b)).collect::<Result<Vec<_>>>()?, "tool": tool(app),
              "create_url": app.cfg.bits.create_url.trim()}))
}

/// `/bits…` routes.
pub fn route(app: &App, method: &str, rest: &[&str], query: &crate::api::Query, body: &Value) -> Result<Value> {
    match (method, rest) {
        ("GET", []) => list(app, query),
        ("POST", []) => app.db.tx(|| add(app, body)),
        ("GET", [name]) => dict(app, &get(app, name)?),
        ("POST", [name]) => app.db.tx(|| set(app, name, body)),
        ("POST", [name, "made"]) => app.db.tx(|| made(app, name, body)),
        ("POST", [name, "remove"]) => app.db.tx(|| remove(app, name, body)),
        _ => err(404, "There's nothing at that address."),
    }
}

/// The handoff's line about the task's bits.
pub fn handoff_lines(app: &App, t: &Row) -> Result<Vec<String>> {
    let bits = of_task(app, t.id())?;
    if bits.is_empty() {
        return Ok(vec![]);
    }
    let tool = tool(app);
    let each: Vec<String> = bits
        .iter()
        .map(|b| {
            let state = match (b.s("kind"), b.s("made_at").is_some()) {
                (Some("local"), _) => format!("local: in the code only, not in {tool}"),
                (_, true) => format!("backend: made in {tool}"),
                _ => format!("backend: not made in {tool} yet"),
            };
            format!("{} ({state})", b.st("name"))
        })
        .collect();
    Ok(vec![format!(
        "This task's work sits behind {} {}. Gate the new code on {}. A new flag goes on the board with {} bit add NAME --backend|--local --task {}.",
        if bits.len() == 1 { "the bit" } else { "the bits" },
        each.join(", "),
        if bits.len() == 1 { "it" } else { "them" },
        board::tb_cmd(app),
        rf("task", t.id())
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_links() {
        assert!(clean_name("newCheckout").is_ok());
        assert!(clean_name("web.new-checkout_v2").is_ok());
        assert!(clean_name("-x").is_err());
        assert!(clean_name("has space").is_err());
        assert_eq!(enc("a b/c"), "a%20b%2Fc");
        assert_eq!(names_from(Some(&json!(["a, b", "c"]))), vec!["a", "b", "c"]);
    }
}
