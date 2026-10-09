//! The device pool: named devices (emulators, simulators, phones, boards) with tags, shared by every
//! project. A task, or a goal for its tasks, asks for N devices by tag (`android:2`, `ios`); the runner
//! starts it only once that many are free, lends them to it while it runs, and names them in its
//! handoff. A device is lent to one task at a time and comes back when the task stops being active
//! (done, or a failed start). A device can have a focus command that raises its window (the
//! simulator, the device hub), run from `tb device focus` or the app.

use std::collections::HashMap;

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, p};

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS devices(
  id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL, tags TEXT DEFAULT '[]', focus TEXT, note TEXT,
  off INT DEFAULT 0, created_at TEXT, updated_at TEXT);
-- What a task (T12) or a goal (G3) asks for: [{"tag": "android", "n": 2}]. A task without its own asks for its goal's;
-- a task's [] needs none, whatever its goal asks for.
CREATE TABLE IF NOT EXISTS device_needs(owner TEXT PRIMARY KEY, needs TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS device_loans(
  id INTEGER PRIMARY KEY, device TEXT NOT NULL, task_id INT NOT NULL, at TEXT, released_at TEXT);
CREATE INDEX IF NOT EXISTS device_loans_task ON device_loans(task_id);
"#;

/// `[devices]` in config.toml.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DevicesConfig {
    /// The focus command for a device that has none of its own: run with `sh -c`, `{name}` is the
    /// device's name (also in `$TASKBOARD_DEVICE`). Empty: only devices with their own command focus.
    pub focus: String,
    /// Seconds a focus command may take.
    pub focus_timeout_secs: Option<f64>,
}

static NAME: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[a-z0-9][a-z0-9._:-]{0,63}$").unwrap());
static TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[a-z0-9][a-z0-9._-]{0,63}$").unwrap());

/// A task holds its devices while it's active: working or needing the owner (not a failed start),
/// or queued with a start job. The same rule as locks.
const ACTIVE_SQL: &str = "((t.status IN ('working', 'needs') AND COALESCE(t.needs_reason, '') != 'start_failed') \
                          OR (t.status = 'queued' AND t.start_job IS NOT NULL))";

#[derive(Debug, Clone, PartialEq)]
pub struct Need {
    pub tag: String,
    pub n: i64,
}

fn tags_of(d: &Row) -> Vec<String> {
    jloads_arr(d.s("tags")).into_iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
}

/// A device answers a need when its name or one of its tags is the need's tag.
fn answers(d: &Row, tag: &str) -> bool {
    d.s("name") == Some(tag) || tags_of(d).iter().any(|t| t == tag)
}

fn clean_name(s: &str) -> Result<String> {
    let v = s.trim().to_lowercase();
    if !NAME.is_match(&v) {
        return err(400, format!("“{v}” can't be a device name. Use lowercase letters, numbers, dots, dashes, colons or underscores, like pixel-7."));
    }
    Ok(v)
}

/// Tags from a body (a list, or words split by commas or spaces), stored as JSON.
fn clean_tags(value: Option<&Value>) -> Result<String> {
    let mut out: Vec<String> = vec![];
    for v in str_list(value) {
        for w in v.replace(',', " ").split_whitespace() {
            let w = w.trim().to_lowercase();
            if w.is_empty() || w == "none" {
                continue;
            }
            if !TAG.is_match(&w) {
                return err(400, format!("“{w}” can't be a tag. Use lowercase letters, numbers, dots, dashes or underscores, like android."));
            }
            if !out.contains(&w) {
                out.push(w);
            }
        }
    }
    Ok(jdumps(&json!(out)))
}

/// Needs from a body: `android:2 ios`, `["android:2", "ios"]`, or `[{"tag": "android", "n": 2}]`.
/// None (or `none`) asks for nothing.
pub fn clean_needs(value: Option<&Value>) -> Result<Vec<Need>> {
    let mut words: Vec<(String, i64)> = vec![];
    let items: Vec<Value> = match value {
        None | Some(Value::Null) => return Ok(vec![]),
        Some(Value::Array(a)) => a.clone(),
        Some(v) => vec![v.clone()],
    };
    for it in items {
        match it {
            Value::Object(o) => {
                let tag = o.get("tag").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let n = o.get("n").and_then(|v| v.as_i64()).unwrap_or(1);
                words.push((tag, n));
            }
            Value::String(s) => {
                for w in s.replace(',', " ").split_whitespace() {
                    let (tag, n) = match w.rsplit_once(':') {
                        Some((t, n)) if n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty() => (t.to_string(), n.parse().unwrap_or(1)),
                        _ => (w.to_string(), 1),
                    };
                    words.push((tag, n));
                }
            }
            other => words.push((other.to_string(), 1)),
        }
    }
    let mut out: Vec<Need> = vec![];
    for (tag, n) in words {
        let tag = tag.trim().to_lowercase();
        if tag.is_empty() || tag == "none" {
            continue;
        }
        if !NAME.is_match(&tag) {
            return err(400, format!("“{tag}” isn't a device tag or name. Ask for devices like android, android:2 or pixel-7."));
        }
        if !(1..=20).contains(&n) {
            return err(400, "Ask for 1 to 20 devices of a kind.");
        }
        match out.iter_mut().find(|x| x.tag == tag) {
            Some(x) => x.n += n,
            None => out.push(Need { tag, n }),
        }
    }
    Ok(out)
}

fn needs_json(needs: &[Need]) -> Value {
    json!(needs.iter().map(|x| json!({"tag": x.tag, "n": x.n})).collect::<Vec<_>>())
}

/// "2 android, ios".
pub fn needs_text(needs: &[Need]) -> String {
    needs.iter().map(|x| if x.n > 1 { format!("{} {}", x.n, x.tag) } else { x.tag.clone() }).collect::<Vec<_>>().join(", ")
}

fn read_needs(app: &App, owner: &str) -> Result<Option<Vec<Need>>> {
    let Some(v) = app.db.val("SELECT needs FROM device_needs WHERE owner = ?", p![owner])?.as_str().map(|s| s.to_string()) else {
        return Ok(None);
    };
    Ok(Some(clean_needs(Some(&serde_json::from_str::<Value>(&v).unwrap_or(json!([]))))?))
}

/// What the task asks for: its own needs, else its goal's.
pub fn needs(app: &App, t: &Row) -> Result<Vec<Need>> {
    if let Some(n) = read_needs(app, &rf("task", t.id()))? {
        return Ok(n);
    }
    match t.i("goal_id") {
        Some(g) => Ok(read_needs(app, &rf("goal", g))?.unwrap_or_default()),
        None => Ok(vec![]),
    }
}

pub fn goal_needs(app: &App, goal_id: i64) -> Result<Vec<Need>> {
    Ok(read_needs(app, &rf("goal", goal_id))?.unwrap_or_default())
}

/// Sets a task's or goal's needs from `body.devices` (when the body has it); true when they changed.
/// A task's `none` (or an empty list) is its own "needs none", over its goal's needs; its `goal` drops
/// its own needs so it asks for its goal's again.
pub fn take_needs(app: &App, kind: &str, id: i64, body: &Value, who: &str) -> Result<bool> {
    let Some(given) = body.get("devices").filter(|v| !v.is_null()) else { return Ok(false) };
    let owner = rf(kind, id);
    let inherit = kind == "task" && given.as_str().map(|s| s.trim().eq_ignore_ascii_case("goal")).unwrap_or(false);
    // A goal that asks for nothing has no row; a task's empty row is its "needs none".
    let new = match if inherit { None } else { Some(clean_needs(Some(given))?) } {
        Some(n) if n.is_empty() && kind != "task" => None,
        n => n,
    };
    let old = read_needs(app, &owner)?;
    if new == old {
        return Ok(false);
    }
    match &new {
        None => app.db.x("DELETE FROM device_needs WHERE owner = ?", p![owner])?,
        Some(n) => app.db.x(
            "INSERT INTO device_needs(owner, needs) VALUES(?, ?) ON CONFLICT(owner) DO UPDATE SET needs = excluded.needs",
            p![owner, jdumps(&needs_json(n))],
        )?,
    };
    let text = match &new {
        None if kind == "task" => "Needs its goal's devices".to_string(),
        Some(n) if !n.is_empty() => format!("Needs devices: {}", needs_text(n)),
        _ => "Needs no devices".to_string(),
    };
    if kind == "task" {
        board::log_event(app, id, who, "note", &text)?;
        board::bump_ctx(app, id)?;
    } else {
        for t in board::goal_tasks(app, id)? {
            board::bump_ctx(app, t.id())?;
        }
    }
    Ok(true)
}

fn pool(app: &App) -> Result<Vec<Row>> {
    app.db.q("SELECT * FROM devices ORDER BY name", p![])
}

/// Device name → the active task that has it.
fn held(app: &App) -> Result<HashMap<String, i64>> {
    let mut out = HashMap::new();
    for r in app.db.q(
        &format!("SELECT l.device, l.task_id FROM device_loans l JOIN tasks t ON t.id = l.task_id WHERE l.released_at IS NULL AND {ACTIVE_SQL}"),
        p![],
    )? {
        if let (Some(d), Some(t)) = (r.s("device"), r.i("task_id")) {
            out.insert(d.to_string(), t);
        }
    }
    Ok(out)
}

/// The devices lent to a task now (names, in the order lent).
pub fn lent(app: &App, task_id: i64) -> Result<Vec<String>> {
    Ok(app
        .db
        .q("SELECT device FROM device_loans WHERE task_id = ? AND released_at IS NULL ORDER BY id", p![task_id])?
        .iter()
        .filter_map(|r| r.s("device").map(|s| s.to_string()))
        .collect())
}

/// Picks devices for the needs from the free ones, preferring those listed in `prefer`. Each need
/// takes its count; a device answers one need. The picks, and the needs left short.
fn fill(needs: &[Need], free: &[Row], prefer: &[String]) -> (Vec<String>, Vec<Need>) {
    let mut order: Vec<&Row> = free.iter().collect();
    order.sort_by_key(|d| (!prefer.iter().any(|p| Some(p.as_str()) == d.s("name")), tags_of(d).len(), d.st("name")));
    let mut picked: Vec<String> = vec![];
    let mut short: Vec<Need> = vec![];
    for need in needs {
        let mut got = 0;
        for d in &order {
            if got >= need.n {
                break;
            }
            let name = d.st("name");
            if !picked.contains(&name) && answers(d, &need.tag) {
                picked.push(name);
                got += 1;
            }
        }
        if got < need.n {
            short.push(Need { tag: need.tag.clone(), n: need.n - got });
        }
    }
    (picked, short)
}

fn free_for(app: &App, task_id: i64) -> Result<Vec<Row>> {
    let held = held(app)?;
    Ok(pool(app)?.into_iter().filter(|d| !d.b("off") && held.get(&d.st("name")).map(|t| *t == task_id).unwrap_or(true)).collect())
}

/// Why the device pool holds this queued task back: not enough free devices for what it asks for.
pub fn blocker(app: &App, t: &Row) -> Result<Option<String>> {
    let needs = needs(app, t)?;
    if needs.is_empty() {
        return Ok(None);
    }
    let (_, short) = fill(&needs, &free_for(app, t.id())?, &[]);
    let Some(first) = short.first() else { return Ok(None) };
    let all = pool(app)?;
    let have = all.iter().filter(|d| !d.b("off") && answers(d, &first.tag)).count() as i64;
    let want = needs.iter().find(|x| x.tag == first.tag).map(|x| x.n).unwrap_or(first.n);
    if have < want {
        return Ok(Some(if have == 0 {
            format!("Needs a {} device, and the pool has none (tb device add)", first.tag)
        } else {
            format!("Needs {want} {} devices, and the pool has {have}", first.tag)
        }));
    }
    let held = held(app)?;
    let mut by: Vec<String> = all
        .iter()
        .filter(|d| answers(d, &first.tag))
        .filter_map(|d| held.get(&d.st("name")).filter(|x| **x != t.id()).map(|x| rf("task", *x)))
        .collect();
    by.sort();
    by.dedup();
    let what = if want > 1 { format!("{want} {} devices", first.tag) } else { format!("a {} device", first.tag) };
    Ok(Some(if by.is_empty() { format!("Waits for {what}") } else { format!("Waits for {what} ({} {} them)", by.join(", "), if by.len() == 1 { "has" } else { "have" }) }))
}

/// Lends the task the devices it asks for (as many as are free), in place of any it had. The names.
pub fn lend(app: &App, t: &Row) -> Result<Vec<String>> {
    let needs = needs(app, t)?;
    let before = lent(app, t.id())?;
    let now = now_iso();
    app.db.x("UPDATE device_loans SET released_at = ? WHERE task_id = ? AND released_at IS NULL", p![now, t.id()])?;
    if needs.is_empty() {
        return Ok(vec![]);
    }
    let (picked, _) = fill(&needs, &free_for(app, t.id())?, &before);
    for d in &picked {
        app.db.insert("device_loans", crate::fields!["device" => d, "task_id" => t.id(), "at" => now])?;
    }
    if !picked.is_empty() && picked != before {
        board::log_event(app, t.id(), board::BOARD, "note", &format!("Lent it {}", picked.join(", ")))?;
    }
    Ok(picked)
}

/// Takes back the devices of tasks that aren't active any more.
pub fn release_idle(app: &App) -> Result<usize> {
    let rows = app.db.q(
        &format!("SELECT l.id FROM device_loans l JOIN tasks t ON t.id = l.task_id WHERE l.released_at IS NULL AND NOT {ACTIVE_SQL}"),
        p![],
    )?;
    let now = now_iso();
    for r in &rows {
        app.db.x("UPDATE device_loans SET released_at = ? WHERE id = ?", p![now, r.id()])?;
    }
    app.db.x("UPDATE device_loans SET released_at = ? WHERE released_at IS NULL AND task_id NOT IN (SELECT id FROM tasks)", p![now])?;
    Ok(rows.len())
}

/// A task card's devices: what it asks for and what it has now.
pub fn card(app: &App, t: &Row) -> Result<Value> {
    let needs = needs(app, t)?;
    let held = held(app)?;
    let has: Vec<String> = lent(app, t.id())?.into_iter().filter(|d| held.get(d) == Some(&t.id())).collect();
    if needs.is_empty() && has.is_empty() {
        return Ok(Value::Null);
    }
    Ok(json!({"needs": needs_json(&needs), "needs_text": needs_text(&needs), "lent": has}))
}

fn device_dict(d: &Row, held: &HashMap<String, i64>, app: &App) -> Result<Value> {
    let holder = match held.get(&d.st("name")) {
        Some(tid) => match board::find_task(app, Some(*tid))? {
            Some(t) => json!({"ref": rf("task", t.id()), "title": t.v("title"), "goal": rf_opt("goal", t.i("goal_id"))}),
            None => Value::Null,
        },
        None => Value::Null,
    };
    let focus = d.s("focus").filter(|f| !f.trim().is_empty()).is_some() || !app.cfg.devices.focus.trim().is_empty();
    Ok(json!({"id": d.id(), "name": d.v("name"), "tags": tags_of(d), "note": d.v("note"), "off": d.b("off"),
              "focus": d.v("focus"), "can_focus": focus, "held_by": holder}))
}

/// `GET /devices`: the pool, who has each device, and the queued tasks waiting for one.
pub fn overview(app: &App) -> Result<Value> {
    let held = held(app)?;
    let devices = pool(app)?.iter().map(|d| device_dict(d, &held, app)).collect::<Result<Vec<_>>>()?;
    let mut waiting = vec![];
    for t in app.db.q("SELECT * FROM tasks WHERE status = 'queued' AND start_job IS NULL ORDER BY id", p![])? {
        if let Some(why) = blocker(app, &t)? {
            waiting.push(json!({"ref": rf("task", t.id()), "title": t.v("title"), "goal": rf_opt("goal", t.i("goal_id")),
                                "needs": needs_text(&needs(app, &t)?), "why": why}));
        }
    }
    Ok(json!({"devices": devices, "waiting": waiting}))
}

/// A goal page's devices aside: what the goal asks for and the pool, with who has each device.
/// Empty pools and goals that ask for nothing give null.
pub fn goal_card(app: &App, goal_id: i64) -> Result<Value> {
    let needs = goal_needs(app, goal_id)?;
    let all = pool(app)?;
    let tasks: Vec<Row> = board::goal_tasks(app, goal_id)?;
    let mut asks = !needs.is_empty();
    for t in &tasks {
        asks = asks || !self::needs(app, t)?.is_empty();
    }
    if all.is_empty() && !asks {
        return Ok(Value::Null);
    }
    let held = held(app)?;
    let devices = all.iter().map(|d| device_dict(d, &held, app)).collect::<Result<Vec<_>>>()?;
    let ours: Vec<i64> = tasks.iter().map(|t| t.id()).collect();
    let lent = held.iter().filter(|(_, t)| ours.contains(t)).count();
    Ok(json!({"needs": needs_json(&needs), "needs_text": needs_text(&needs), "devices": devices, "lent_here": lent}))
}

fn get(app: &App, name: &str) -> Result<Row> {
    let n = name.trim().to_lowercase();
    match app.db.q1("SELECT * FROM devices WHERE name = ?", p![n])? {
        Some(d) => Ok(d),
        None => err(404, format!("There's no device called {n}. tb devices lists the pool.")),
    }
}

fn add(app: &App, body: &Value) -> Result<Value> {
    let name = clean_name(&body_str(body, "name"))?;
    if app.db.q1("SELECT id FROM devices WHERE name = ?", p![name])?.is_some() {
        return err(409, format!("There's already a device called {name}. Change it with tb device set."));
    }
    let now = now_iso();
    let focus = body_str(body, "focus");
    let note = one_line(&body_str(body, "note"), 200);
    app.db.insert(
        "devices",
        crate::fields!["name" => name, "tags" => clean_tags(body.get("tags"))?, "focus" => if focus.trim().is_empty() { None } else { Some(focus) },
                       "note" => if note.is_empty() { None } else { Some(note) }, "off" => 0, "created_at" => now, "updated_at" => now],
    )?;
    device_dict(&get(app, &name)?, &held(app)?, app)
}

fn set(app: &App, name: &str, body: &Value) -> Result<Value> {
    let d = get(app, name)?;
    let mut f: Vec<(&str, Value)> = vec![];
    let mut new_name = d.st("name");
    if body_has(body, "name") {
        let n = clean_name(&body_str(body, "name"))?;
        if n != new_name {
            if app.db.q1("SELECT id FROM devices WHERE name = ?", p![n])?.is_some() {
                return err(409, format!("There's already a device called {n}."));
            }
            app.db.x("UPDATE device_loans SET device = ? WHERE device = ?", p![n, new_name])?;
            f.push(("name", json!(n)));
            new_name = n;
        }
    }
    if body.get("tags").is_some() {
        f.push(("tags", json!(clean_tags(body.get("tags"))?)));
    }
    if body.get("focus").is_some() {
        let v = body_str(body, "focus");
        f.push(("focus", if v.trim().is_empty() || v.trim() == "none" { Value::Null } else { json!(v) }));
    }
    if body.get("note").is_some() {
        let v = one_line(&body_str(body, "note"), 200);
        f.push(("note", if v.is_empty() { Value::Null } else { json!(v) }));
    }
    if body.get("off").is_some() {
        f.push(("off", json!(as_bool(body.get("off"), false) as i64)));
    }
    if f.is_empty() {
        return err(400, "Say what to change: name, tags, focus, note or off.");
    }
    f.push(("updated_at", json!(now_iso())));
    app.db.update("devices", &json!(d.id()), f)?;
    device_dict(&get(app, &new_name)?, &held(app)?, app)
}

fn remove(app: &App, name: &str) -> Result<Value> {
    let d = get(app, name)?;
    if let Some(t) = held(app)?.get(&d.st("name")) {
        return err(409, format!("{} has {} now. Remove it once that task is done, or switch it off with tb device set {} --off.", rf("task", *t), d.st("name"), d.st("name")));
    }
    app.db.x("DELETE FROM devices WHERE id = ?", p![d.id()])?;
    Ok(json!({"ok": true, "removed": d.v("name")}))
}

/// The command that raises a device's window: its own, else `[devices] focus`, with `{name}` filled in.
pub fn focus_command(app: &App, d: &Row) -> Option<String> {
    let cmd = d.s("focus").filter(|f| !f.trim().is_empty()).map(|s| s.to_string()).or_else(|| Some(app.cfg.devices.focus.clone()).filter(|f| !f.trim().is_empty()))?;
    Some(cmd.replace("{name}", &d.st("name")))
}

fn focus(app: &App, name: &str) -> Result<Value> {
    let d = get(app, name)?;
    let Some(cmd) = focus_command(app, &d) else {
        return err(409, format!("{} has no focus command. Give it one with tb device set {} --focus \"…\", or set [devices] focus in config.toml.", d.st("name"), d.st("name")));
    };
    let dev = d.st("name");
    let timeout = app.cfg.devices.focus_timeout_secs.unwrap_or(15.0);
    app.defer(Box::new(move |a: &App| {
        let env = vec![("TASKBOARD_DEVICE".to_string(), dev.clone())];
        match crate::proc::run_with(std::path::Path::new("/bin/sh"), &["-c".to_string(), cmd], None, timeout, &env, None) {
            Ok(o) if o.code == Some(0) => {}
            Ok(o) => a.info(format!("devices: focusing {dev} exited {:?}: {}", o.code, one_line(&o.stderr, 300))),
            Err(crate::proc::RunError::TimedOut) => a.info(format!("devices: focusing {dev} took over {timeout}s")),
            Err(crate::proc::RunError::Spawn(e)) => a.info(format!("devices: couldn't focus {dev}: {e}")),
        }
    }));
    Ok(json!({"ok": true, "device": d.v("name")}))
}

/// `/devices…` routes.
pub fn route(app: &App, method: &str, rest: &[&str], body: &Value) -> Result<Value> {
    match (method, rest) {
        ("GET", []) => overview(app),
        ("POST", []) => app.db.tx(|| add(app, body)),
        ("GET", [name]) => device_dict(&get(app, name)?, &held(app)?, app),
        ("POST", [name]) => app.db.tx(|| set(app, name, body)),
        ("POST", [name, "remove"]) => app.db.tx(|| remove(app, name)),
        ("POST", [name, "focus"]) => focus(app, name),
        _ => err(404, "There's nothing at that address."),
    }
}

/// The handoff's line about the devices lent to the task.
pub fn handoff_lines(app: &App, t: &Row) -> Result<Vec<String>> {
    let names = lent(app, t.id())?;
    if names.is_empty() {
        let needs = needs(app, t)?;
        if needs.is_empty() {
            return Ok(vec![]);
        }
        return Ok(vec![format!(
            "This task asks for devices ({}), but none were free when it started. Run {} devices to see who has them.",
            needs_text(&needs),
            board::tb_cmd(app)
        )]);
    }
    let all = pool(app)?;
    let described: Vec<String> = names
        .iter()
        .map(|n| match all.iter().find(|d| d.s("name") == Some(n.as_str())) {
            Some(d) => {
                let tags = tags_of(d);
                let note = d.s("note").map(|x| format!(", {x}")).unwrap_or_default();
                if tags.is_empty() && note.is_empty() { n.clone() } else { format!("{n} ({}{note})", tags.join(", ")) }
            }
            None => n.clone(),
        })
        .collect();
    Ok(vec![format!(
        "The board lent this task {} {}: use only {}, since other tasks have the rest of the pool. They go back when the task is done.",
        if names.len() == 1 { "the device" } else { "the devices" },
        described.join(", "),
        if names.len() == 1 { "that one" } else { "these" }
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, tags: &[&str]) -> Row {
        let mut r = Row::new();
        r.insert("name".into(), json!(name));
        r.insert("tags".into(), json!(jdumps(&json!(tags))));
        r
    }

    #[test]
    fn needs_parse_from_words_and_lists() {
        assert_eq!(clean_needs(Some(&json!("android:2, ios"))).unwrap(), vec![Need { tag: "android".into(), n: 2 }, Need { tag: "ios".into(), n: 1 }]);
        assert_eq!(clean_needs(Some(&json!(["Android", "android"]))).unwrap(), vec![Need { tag: "android".into(), n: 2 }]);
        assert_eq!(clean_needs(Some(&json!([{"tag": "ios", "n": 3}]))).unwrap(), vec![Need { tag: "ios".into(), n: 3 }]);
        assert!(clean_needs(Some(&json!("none"))).unwrap().is_empty());
        assert!(clean_needs(Some(&json!("android:0"))).is_err());
        assert!(clean_needs(Some(&json!("Bad Tag!"))).is_err());
        assert_eq!(needs_text(&clean_needs(Some(&json!("android:2 ios"))).unwrap()), "2 android, ios");
    }

    #[test]
    fn fill_prefers_the_devices_it_had_and_narrow_ones() {
        let free = vec![dev("pixel-7", &["android"]), dev("pixel-8", &["android", "tablet"]), dev("iphone", &["ios"])];
        let needs = vec![Need { tag: "android".into(), n: 1 }, Need { tag: "ios".into(), n: 1 }];
        assert_eq!(fill(&needs, &free, &[]), (vec!["pixel-7".to_string(), "iphone".to_string()], vec![]));
        assert_eq!(fill(&needs, &free, &["pixel-8".to_string()]).0, vec!["pixel-8".to_string(), "iphone".to_string()]);
        let (picked, short) = fill(&[Need { tag: "android".into(), n: 3 }], &free, &[]);
        assert_eq!(picked.len(), 2);
        assert_eq!(short, vec![Need { tag: "android".into(), n: 1 }]);
        assert_eq!(fill(&[Need { tag: "iphone".into(), n: 1 }], &free, &[]).0, vec!["iphone".to_string()], "a name answers too");
    }
}
