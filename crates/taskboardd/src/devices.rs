//! The device pool: named devices (emulators, simulators, phones, boards) with tags, shared by every
//! project. A task, or a goal for its tasks, asks for N devices by tag (`android:2`, `ios`); the runner
//! starts it only once that many are free, lends them to it while it runs, and names them in its
//! handoff. A device is lent to one task at a time and comes back when the task stops being active
//! (done, or a failed start). A device can have a focus command that raises its window (the
//! simulator, the device hub), run from `tb device focus` or the app.
//!
//! A device may also say what it is (`kind`, a label like phone or simulator, never matched against
//! needs; `target`, like Android 14 or an iOS 17 runtime), shown with its name ("dev-a (Android phone,
//! Android 14)"), and how to boot it and shut it down (`start_cmd`, `stop_cmd`): the handoff tells the
//! task that's lent it "Start it: …" and "Stop it when you're done: …", filled like step commands.
//!
//! A goal can hold devices of its own (`tb goal devices`): its tasks are lent only those (or those
//! first, with `[devices] goal_pool_only = false`), a device it reserves is never lent to other
//! goals' tasks (nor to tasks in no goal; several goals may reserve one and share it), and the
//! purposes it gives a device ("measure", or "measure,demo") count as that device's tags for its own
//! tasks. An archived goal holds nothing back.

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
-- A goal's own devices: its tasks get them before the rest of the pool, a reserved one goes to no
-- other goal's tasks, and its purpose counts as a tag on it for the goal's tasks.
CREATE TABLE IF NOT EXISTS goal_devices(
  goal_id INT NOT NULL, device TEXT NOT NULL, purpose TEXT, reserved INT DEFAULT 0, at TEXT,
  PRIMARY KEY(goal_id, device));
"#;

/// Device columns added since the first schema: what it is (a label, not a tag), and how to boot it
/// and shut it down.
pub const ADDED: &[(&str, &str, &str)] = &[("devices", "kind", "TEXT"), ("devices", "target", "TEXT"), ("devices", "start_cmd", "TEXT"), ("devices", "stop_cmd", "TEXT")];

/// Kinds the board names in words ("android" → "Android phone"); any other kind shows as it was given.
const KINDS: &[(&[&str], &str)] = &[
    (&["android", "android_phone", "android-phone", "android phone"], "Android phone"),
    (&["android_tablet", "android-tablet", "android tablet"], "Android tablet"),
    (&["android_emulator", "android-emulator", "android emulator", "emulator", "android_emu", "avd"], "Android emulator"),
    (&["ios", "iphone", "ios_phone", "ios-phone"], "iPhone"),
    (&["ipad", "ios_tablet", "ios-tablet"], "iPad"),
    (&["ios_simulator", "ios-simulator", "ios simulator", "simulator", "ios_sim", "sim"], "iOS simulator"),
    (&["phone"], "Phone"),
    (&["tablet"], "Tablet"),
    (&["watch"], "Watch"),
    (&["tv"], "TV"),
    (&["desktop", "mac", "macos"], "Mac"),
    (&["browser", "web"], "Browser"),
];

/// A device's kind in words: one the board knows by name, else as given.
pub fn kind_label(kind: &str) -> String {
    let k = kind.trim();
    let low = k.to_lowercase();
    KINDS.iter().find(|(keys, _)| keys.contains(&low.as_str())).map(|(_, l)| l.to_string()).unwrap_or_else(|| k.to_string())
}

/// "Android phone, Android 14": a device's kind and target, empty when it has neither.
pub fn what(d: &Row) -> String {
    let mut parts: Vec<String> = vec![];
    if let Some(k) = d.s("kind").filter(|k| !k.trim().is_empty()) {
        parts.push(kind_label(k));
    }
    if let Some(t) = d.s("target").filter(|t| !t.trim().is_empty()) {
        parts.push(t.trim().to_string());
    }
    parts.join(", ")
}

/// "dev-a (Android phone, Android 14)": the name, with its kind and target when it has them.
pub fn label(d: &Row) -> String {
    match what(d) {
        w if w.is_empty() => d.st("name"),
        w => format!("{} ({w})", d.st("name")),
    }
}

/// `[devices]` in config.toml.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DevicesConfig {
    /// The focus command for a device that has none of its own: run with `sh -c`, `{name}` is the
    /// device's name (also in `$TASKBOARD_DEVICE`). Empty: only devices with their own command focus.
    pub focus: String,
    /// Seconds a focus command may take.
    pub focus_timeout_secs: Option<f64>,
    /// A goal with devices of its own lends its tasks only those (the Python board's rule); false
    /// lends from its own first, then the rest of the pool.
    pub goal_pool_only: bool,
}

impl Default for DevicesConfig {
    fn default() -> Self {
        DevicesConfig { focus: String::new(), focus_timeout_secs: None, goal_pool_only: true }
    }
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

/// A device answers a need when its name or one of its tags is the need's tag. One outside its goal's
/// own pool (`name_only`, see `view`) answers only by its name.
fn answers(d: &Row, tag: &str) -> bool {
    d.s("name") == Some(tag) || (!d.b("name_only") && tags_of(d).iter().any(|t| t == tag))
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

/// Every live goal's own devices (rows of goal_devices whose goal and device still exist, the goal
/// not archived: an archived goal holds nothing back).
fn goal_pools(app: &App) -> Result<Vec<Row>> {
    app.db.q(
        "SELECT gd.* FROM goal_devices gd JOIN goals g ON g.id = gd.goal_id JOIN devices d ON d.name = gd.device \
         WHERE COALESCE(g.archived, 0) = 0 ORDER BY gd.goal_id, gd.device",
        p![],
    )
}

/// One goal's own devices, archived or not, for its page.
fn own_pool(app: &App, goal_id: i64) -> Result<Vec<Row>> {
    app.db.q("SELECT gd.* FROM goal_devices gd JOIN devices d ON d.name = gd.device WHERE gd.goal_id = ? ORDER BY gd.device", p![goal_id])
}

/// The goals that reserve each device (more than one may: their tasks share it).
fn reserved(pools: &[Row]) -> HashMap<String, Vec<i64>> {
    let mut out: HashMap<String, Vec<i64>> = HashMap::new();
    for r in pools.iter().filter(|r| r.b("reserved")) {
        if let (Some(d), Some(g)) = (r.s("device"), r.i("goal_id")) {
            out.entry(d.to_string()).or_default().push(g);
        }
    }
    out
}

/// "G3", "G3 and G4": the goals that reserve a device.
fn goals_text(goals: &[i64]) -> String {
    let refs: Vec<String> = goals.iter().map(|g| rf("goal", *g)).collect();
    match refs.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => refs.join(""),
    }
}

/// A pool row's purposes: one tag, or a comma list of them (`measure,demo`).
fn purposes(r: &Row) -> Vec<String> {
    r.s("purpose").unwrap_or("").split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// The pool as a task sees it: devices reserved for other goals left out, and in its own goal's
/// pool, each device's purposes added to its tags. With the names in its goal's pool, lent first.
/// With `[devices] goal_pool_only` (the Python board's rule), a goal with devices of its own lends
/// its tasks only those for a tag; a device asked for by name is lent whatever the pool (as Python
/// did), so the rest stay in the view marked `name_only`.
fn view(app: &App, goal: Option<i64>) -> Result<(Vec<Row>, Vec<String>)> {
    let pools = goal_pools(app)?;
    let res = reserved(&pools);
    let ours: Vec<&Row> = pools.iter().filter(|r| goal.is_some() && r.i("goal_id") == goal).collect();
    let only_ours = app.cfg.devices.goal_pool_only && !ours.is_empty();
    let mut out = vec![];
    for mut d in pool(app)? {
        let name = d.st("name");
        if res.get(&name).is_some_and(|gs| !gs.iter().any(|g| Some(*g) == goal)) {
            continue;
        }
        let own = ours.iter().find(|r| r.s("device") == Some(name.as_str()));
        if only_ours && own.is_none() {
            d.insert("name_only".into(), json!(1));
        }
        if let Some(own) = own {
            let mut tags = tags_of(&d);
            for p in purposes(own) {
                if !tags.contains(&p) {
                    tags.push(p);
                }
            }
            d.insert("tags".into(), json!(jdumps(&json!(tags))));
        }
        out.push(d);
    }
    Ok((out, ours.iter().filter_map(|r| r.s("device").map(str::to_string)).collect()))
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

/// Picks devices for the needs from the free ones, preferring those listed in `prefer` (the ones it
/// had), then those in `first` (its goal's own). Each need takes its count; a device answers one
/// need. The picks, and the needs left short.
fn fill(needs: &[Need], free: &[Row], prefer: &[String], first: &[String]) -> (Vec<String>, Vec<Need>) {
    let mut order: Vec<&Row> = free.iter().collect();
    let has = |l: &[String], d: &Row| l.iter().any(|p| Some(p.as_str()) == d.s("name"));
    order.sort_by_key(|d| (!has(prefer, d), !has(first, d), tags_of(d).len(), d.st("name")));
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

/// The devices in the task's view of the pool that are on and not lent to another task.
fn free_in(app: &App, seen: &[Row], task_id: i64) -> Result<Vec<Row>> {
    let held = held(app)?;
    Ok(seen.iter().filter(|d| !d.b("off") && held.get(&d.st("name")).map(|t| *t == task_id).unwrap_or(true)).cloned().collect())
}

/// Why the device pool holds this queued task back: not enough free devices for what it asks for.
pub fn blocker(app: &App, t: &Row) -> Result<Option<String>> {
    let needs = needs(app, t)?;
    if needs.is_empty() {
        return Ok(None);
    }
    let (all, ours) = view(app, t.i("goal_id"))?;
    let (_, short) = fill(&needs, &free_in(app, &all, t.id())?, &[], &ours);
    let Some(first) = short.first() else { return Ok(None) };
    let have = all.iter().filter(|d| !d.b("off") && answers(d, &first.tag)).count() as i64;
    let want = needs.iter().find(|x| x.tag == first.tag).map(|x| x.n).unwrap_or(first.n);
    // A device asked for by name, or a name the pool has no device or tag for, before the goal's own
    // pool: a named device is lent whatever the pool (as on the Python board).
    let pools = goal_pools(app)?;
    let everyone = pool(app)?;
    if let Some(d) = everyone.iter().find(|d| d.s("name") == Some(first.tag.as_str())) {
        let name = &first.tag;
        if let Some(gs) = reserved(&pools).get(name).filter(|gs| !gs.iter().any(|g| Some(*g) == t.i("goal_id"))) {
            return Ok(Some(format!("Waiting for a free {name} ({name} is reserved for {})", goals_text(gs))));
        }
        if d.b("off") {
            return Ok(Some(format!("Waiting for {name} (it's off)")));
        }
    } else if !everyone.iter().any(|d| tags_of(d).contains(&first.tag)) && !pools.iter().any(|r| purposes(r).contains(&first.tag)) {
        return Ok(Some(format!("No {} yet (tb device add)", first.tag)));
    }
    if have < want && app.cfg.devices.goal_pool_only && !ours.is_empty() {
        // Lent only from its goal's own devices.
        let g = rf("goal", t.i0("goal_id"));
        return Ok(Some(if have == 0 {
            format!("Needs a {} device, and {g}'s own devices have none (tb goal devices {g} --add)", first.tag)
        } else {
            format!("Needs {want} {} devices, and {g}'s own devices have {have}", first.tag)
        }));
    }
    if have < want {
        // Devices that would answer it but are reserved for other goals.
        let res = reserved(&pools);
        let mut kept: Vec<String> = everyone
            .iter()
            .filter(|d| !d.b("off") && answers(d, &first.tag))
            .filter_map(|d| {
                res.get(&d.st("name"))
                    .filter(|gs| !gs.iter().any(|g| Some(*g) == t.i("goal_id")))
                    .map(|gs| format!("{} is reserved for {}", d.st("name"), goals_text(gs)))
            })
            .collect();
        kept.sort();
        let kept = if kept.is_empty() { String::new() } else { format!(" ({})", kept.join("; ")) };
        return Ok(Some(if have == 0 && kept.is_empty() {
            format!("Needs a {} device, and the pool has none (tb device add)", first.tag)
        } else if have == 0 {
            format!("Needs a {} device, and the pool has none for it{kept}", first.tag)
        } else {
            format!("Needs {want} {} devices, and the pool has {have}{kept}", first.tag)
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
    let (seen, ours) = view(app, t.i("goal_id"))?;
    let (picked, short) = fill(&needs, &free_in(app, &seen, t.id())?, &before, &ours);
    for d in &picked {
        app.db.insert("device_loans", crate::fields!["device" => d, "task_id" => t.id(), "at" => now])?;
    }
    if !picked.is_empty() && picked != before {
        board::log_event(app, t.id(), board::BOARD, "note", &format!("Lent it {}", picked.join(", ")))?;
    }
    // Started (by hand, or a start that didn't wait) without all it asks for: say why on the task.
    if !short.is_empty() {
        let why = blocker(app, t)?.unwrap_or_else(|| format!("No {} device was free", short[0].tag));
        board::log_event(app, t.id(), board::BOARD, "note", &format!("{why}; it started without"))?;
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
    // Each lent device's name with its kind and target, for showing; `lent` stays the names.
    let mut labels = vec![];
    for n in &has {
        labels.push(match app.db.q1("SELECT * FROM devices WHERE name = ?", p![n])? {
            Some(d) => label(&d),
            None => n.clone(),
        });
    }
    Ok(json!({"needs": needs_json(&needs), "needs_text": needs_text(&needs), "lent": has, "lent_labels": labels}))
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
    let pools: Vec<Row> = goal_pools(app)?.into_iter().filter(|r| r.s("device") == d.s("name")).collect();
    let by: Vec<i64> = pools.iter().filter(|r| r.b("reserved")).map(|r| r.i0("goal_id")).collect();
    let reserved_for = (!by.is_empty()).then(|| goals_text(&by));
    let goals: Vec<Value> = pools.iter().map(|r| json!({"goal": rf("goal", r.i0("goal_id")), "purpose": r.v("purpose"), "reserved": r.b("reserved")})).collect();
    let kind = d.s("kind").map(kind_label);
    Ok(json!({"id": d.id(), "name": d.v("name"), "label": label(d), "kind": d.v("kind"), "kind_label": kind, "target": d.v("target"),
              "tags": tags_of(d), "note": d.v("note"), "off": d.b("off"), "focus": d.v("focus"), "can_focus": focus,
              "start_cmd": d.v("start_cmd"), "stop_cmd": d.v("stop_cmd"), "held_by": holder, "goals": goals, "reserved_for": reserved_for}))
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
    // The goal's own devices first, each with its purpose and whether it's reserved here.
    let mine: Vec<Row> = own_pool(app, goal_id)?;
    let mut devices = vec![];
    for d in &all {
        let mut v = device_dict(d, &held, app)?;
        let own = mine.iter().find(|r| r.s("device") == d.s("name"));
        v["in_pool"] = json!(own.is_some());
        v["purpose"] = own.map(|r| r.v("purpose")).unwrap_or(Value::Null);
        v["reserved"] = json!(own.is_some_and(|r| r.b("reserved")));
        devices.push(v);
    }
    devices.sort_by_key(|v| v["in_pool"] != true);
    let ours: Vec<i64> = tasks.iter().map(|t| t.id()).collect();
    let lent = held.iter().filter(|(_, t)| ours.contains(t)).count();
    Ok(json!({"needs": needs_json(&needs), "needs_text": needs_text(&needs), "devices": devices, "lent_here": lent, "pool": mine.len()}))
}

/// A goal's own devices, for `GET /goals/:id/devices`.
fn goal_pool(app: &App, goal_id: i64) -> Result<Value> {
    let held = held(app)?;
    let mut out = vec![];
    for r in own_pool(app, goal_id)? {
        let d = get(app, &r.st("device"))?;
        let mut v = device_dict(&d, &held, app)?;
        v["purpose"] = r.v("purpose");
        v["reserved"] = json!(r.b("reserved"));
        out.push(v);
    }
    Ok(json!({"goal": rf("goal", goal_id), "devices": out}))
}

/// Puts a device in a goal's own pool, or changes its purpose (`none` drops it) or `reserved`.
fn goal_pool_set(app: &App, goal_id: i64, body: &Value) -> Result<Value> {
    board::get_goal(app, goal_id)?;
    let d = get(app, &body_str(body, "device"))?;
    let name = d.st("name");
    let had = app.db.q1("SELECT * FROM goal_devices WHERE goal_id = ? AND device = ?", p![goal_id, name])?;
    // One tag, or a comma list of them (`measure,demo`); each counts as a tag.
    let purpose = if body.get("purpose").is_some() {
        let v = body_str(body, "purpose").trim().to_lowercase();
        let mut list: Vec<String> = vec![];
        for p in v.split(',').map(str::trim).filter(|p| !p.is_empty() && *p != "none") {
            if !TAG.is_match(p) {
                return err(400, format!("“{p}” can't be a purpose: it counts as a tag, so use lowercase letters, numbers, dots, dashes or underscores, like measure (or a comma list, like measure,demo)."));
            }
            if !list.iter().any(|x| x == p) {
                list.push(p.to_string());
            }
        }
        (!list.is_empty()).then(|| list.join(","))
    } else {
        had.as_ref().and_then(|r| r.s("purpose").map(str::to_string))
    };
    // More than one goal may reserve a device, as on the Python board: their tasks share it.
    let reserve = if body.get("reserved").is_some() { as_bool(body.get("reserved"), false) } else { had.as_ref().is_some_and(|r| r.b("reserved")) };
    app.db.x(
        "INSERT INTO goal_devices(goal_id, device, purpose, reserved, at) VALUES(?, ?, ?, ?, ?) \
         ON CONFLICT(goal_id, device) DO UPDATE SET purpose = excluded.purpose, reserved = excluded.reserved",
        p![goal_id, name, purpose, reserve as i64, now_iso()],
    )?;
    for t in board::goal_tasks(app, goal_id)? {
        board::bump_ctx(app, t.id())?;
    }
    goal_pool(app, goal_id)
}

fn goal_pool_remove(app: &App, goal_id: i64, name: &str) -> Result<Value> {
    let n = name.trim().to_lowercase();
    if app.db.q1("SELECT 1 FROM goal_devices WHERE goal_id = ? AND device = ?", p![goal_id, n])?.is_none() {
        return err(404, format!("{n} isn't one of {}'s devices.", rf("goal", goal_id)));
    }
    app.db.x("DELETE FROM goal_devices WHERE goal_id = ? AND device = ?", p![goal_id, n])?;
    for t in board::goal_tasks(app, goal_id)? {
        board::bump_ctx(app, t.id())?;
    }
    goal_pool(app, goal_id)
}

/// `/goals/:id/devices…` routes: the goal's own devices.
pub fn goal_route(app: &App, method: &str, goal_id: i64, rest: &[&str], body: &Value) -> Result<Value> {
    match (method, rest) {
        ("GET", []) => goal_pool(app, goal_id),
        ("POST", []) => app.db.tx(|| goal_pool_set(app, goal_id, body)),
        ("POST", [name, "remove"]) => app.db.tx(|| goal_pool_remove(app, goal_id, name)),
        _ => err(404, "There's nothing at that address."),
    }
}

fn get(app: &App, name: &str) -> Result<Row> {
    let n = name.trim().to_lowercase();
    match app.db.q1("SELECT * FROM devices WHERE name = ?", p![n])? {
        Some(d) => Ok(d),
        None => err(404, format!("There's no device called {n}. tb devices lists the pool.")),
    }
}

/// A kind or target from a body: one line, or None for empty (or `none`).
fn clean_label(body: &Value, key: &str) -> Option<String> {
    let v = one_line(&body_str(body, key), 80);
    (!v.is_empty() && !v.eq_ignore_ascii_case("none")).then_some(v)
}

/// A start or stop command from a body, as given; None for empty (or `none`).
fn clean_cmd(body: &Value, key: &str) -> Option<String> {
    let v = body_str(body, key).trim().to_string();
    (!v.is_empty() && !v.eq_ignore_ascii_case("none")).then_some(v)
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
                       "note" => if note.is_empty() { None } else { Some(note) }, "kind" => clean_label(body, "kind"), "target" => clean_label(body, "target"),
                       "start_cmd" => clean_cmd(body, "start_cmd"), "stop_cmd" => clean_cmd(body, "stop_cmd"),
                       "off" => 0, "created_at" => now, "updated_at" => now],
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
            app.db.x("UPDATE goal_devices SET device = ? WHERE device = ?", p![n, new_name])?;
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
    for k in ["kind", "target"] {
        if body.get(k).is_some() {
            f.push((k, json!(clean_label(body, k))));
        }
    }
    for k in ["start_cmd", "stop_cmd"] {
        if body.get(k).is_some() {
            f.push((k, json!(clean_cmd(body, k))));
        }
    }
    if f.is_empty() {
        return err(400, "Say what to change: name, tags, kind, target, start_cmd, stop_cmd, focus, note or off.");
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
    app.db.x("DELETE FROM goal_devices WHERE device = ?", p![d.st("name")])?;
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

/// What fills a device's start and stop commands: the task's step placeholders (`{task}`, `{branch}`,
/// …), and the device's `{device}` (also `{name}`), `{kind}` and `{target}`.
pub fn cmd_vars(app: &App, t: &Row, d: &Row) -> std::collections::BTreeMap<String, String> {
    let mut v = crate::steps::vars_for(app, t);
    v.insert("device".into(), d.st("name"));
    v.insert("name".into(), d.st("name"));
    v.insert("kind".into(), d.st("kind"));
    v.insert("target".into(), d.st("target"));
    v
}

/// "Start it: …" and "Stop it when you're done: …" for a lent device, filled for the task.
fn cmd_lines(app: &App, t: &Row, d: &Row) -> Vec<String> {
    let vars = cmd_vars(app, t, d);
    let mut out = vec![];
    if let Some(c) = d.s("start_cmd").filter(|c| !c.trim().is_empty()) {
        out.push(format!("Start it: {}", crate::steps::fill(c.trim(), &vars)));
    }
    if let Some(c) = d.s("stop_cmd").filter(|c| !c.trim().is_empty()) {
        out.push(format!("Stop it when you're done: {}", crate::steps::fill(c.trim(), &vars)));
    }
    out
}

/// The handoff's line about the devices lent to the task, and how to start and stop each.
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
    let (all, _) = view(app, t.i("goal_id"))?;
    let rows: Vec<Option<&Row>> = names.iter().map(|n| all.iter().find(|d| d.s("name") == Some(n.as_str()))).collect();
    let described: Vec<String> = names
        .iter()
        .zip(&rows)
        .map(|(n, d)| match d {
            Some(d) => {
                // What it is (kind, target), else its tags; then its note.
                let mut parts: Vec<String> = match what(d) {
                    w if w.is_empty() => tags_of(d),
                    w => vec![w],
                };
                parts.extend(d.s("note").filter(|x| !x.is_empty()).map(str::to_string));
                if parts.is_empty() { n.clone() } else { format!("{n} ({})", parts.join(", ")) }
            }
            None => n.clone(),
        })
        .collect();
    let mut text = format!(
        "The board lent this task {} {}: use only {}, since other tasks have the rest of the pool. They go back when the task is done.",
        if names.len() == 1 { "the device" } else { "the devices" },
        described.join(", "),
        if names.len() == 1 { "that one" } else { "these" }
    );
    for (n, d) in names.iter().zip(&rows) {
        let Some(d) = d else { continue };
        let cmds = cmd_lines(app, t, d);
        if cmds.is_empty() {
            continue;
        }
        if names.len() == 1 {
            text += &format!("\n{}", cmds.join("\n"));
        } else {
            text += &format!("\n{n}:\n{}", cmds.iter().map(|c| format!("  {c}")).collect::<Vec<_>>().join("\n"));
        }
    }
    Ok(vec![text])
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
        assert_eq!(fill(&needs, &free, &[], &[]), (vec!["pixel-7".to_string(), "iphone".to_string()], vec![]));
        assert_eq!(fill(&needs, &free, &["pixel-8".to_string()], &[]).0, vec!["pixel-8".to_string(), "iphone".to_string()]);
        let (picked, short) = fill(&[Need { tag: "android".into(), n: 3 }], &free, &[], &[]);
        assert_eq!(picked.len(), 2);
        assert_eq!(short, vec![Need { tag: "android".into(), n: 1 }]);
        assert_eq!(fill(&[Need { tag: "iphone".into(), n: 1 }], &free, &[], &[]).0, vec!["iphone".to_string()], "a name answers too");
        assert_eq!(fill(&needs, &free, &[], &["pixel-8".to_string()]).0[0], "pixel-8", "the goal's own devices go first");
        assert_eq!(fill(&needs, &free, &["pixel-7".to_string()], &["pixel-8".to_string()]).0[0], "pixel-7", "after the ones it had");
    }
}
