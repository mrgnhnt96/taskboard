//! The device pool: named devices (emulators, simulators, phones, boards) with tags, shared by every
//! project. A task, or a goal for its tasks, asks for N devices by tag (`android:2`, `ios`); the runner
//! starts it only once that many are free, lends them to it while it runs, and names them in its
//! handoff. A device is lent to one task at a time and comes back when the task stops being active
//! (done, or a failed start). A device can have a focus command that raises its window (the
//! simulator, the device hub), run from `tb device focus` or the app.
//!
//! A device may also say what it is (`kind`, a label like phone or simulator, never matched against
//! needs; `target`, the emulator's serial or the simulator's UDID, which its commands use as
//! `{target}`), shown with its name ("dev-a (Android emulator, emulator-5554)"), and how to boot it and shut it down (`start_cmd`, `stop_cmd`): the handoff tells the
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

/// Kinds the board names in words ("android" → "Android emulator"); any other kind shows as it was
/// given. The first four are the Python board's kinds, named as it named them: `android`, `ios`,
/// `device` (a real phone or tablet) and `other`.
const KINDS: &[(&[&str], &str)] = &[
    (&["android", "android_emulator", "android-emulator", "android emulator", "emulator", "android_emu", "avd"], "Android emulator"),
    (&["ios", "ios_simulator", "ios-simulator", "ios simulator", "simulator", "ios_sim", "sim"], "iOS simulator"),
    (&["device"], "Phone or tablet"),
    (&["other"], "Device"),
    (&["android_phone", "android-phone", "android phone"], "Android phone"),
    (&["android_tablet", "android-tablet", "android tablet"], "Android tablet"),
    (&["iphone", "ios_phone", "ios-phone"], "iPhone"),
    (&["ipad", "ios_tablet", "ios-tablet"], "iPad"),
    (&["phone"], "Phone"),
    (&["tablet"], "Tablet"),
    (&["watch"], "Watch"),
    (&["tv"], "TV"),
    (&["desktop", "mac", "macos"], "Mac"),
    (&["browser", "web"], "Browser"),
];

/// A device's kind in words: one the board knows by name, else as given ("Device" when it has none).
pub fn kind_label(kind: &str) -> String {
    let k = kind.trim();
    if k.is_empty() {
        return "Device".into();
    }
    let low = k.to_lowercase();
    KINDS.iter().find(|(keys, _)| keys.contains(&low.as_str())).map(|(_, l)| l.to_string()).unwrap_or_else(|| k.to_string())
}

/// "Android emulator, emulator-5554": a device's kind and target, empty when it has neither (a
/// device with no kind isn't called "Device" here: that says nothing next to its name).
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

/// "dev-a (Android emulator, emulator-5554)": the name, with its kind and target when it has them.
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
        // The handoff tells agents `tag:<tag>`, and the old board took `device:<name>`: both name the word after.
        let tag = tag.trim().to_lowercase();
        let tag = tag.strip_prefix("tag:").or_else(|| tag.strip_prefix("device:")).unwrap_or(&tag).to_string();
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
        // A task that's already running gets what it now asks for, as it would have at its start.
        let t = board::get_task(app, id)?;
        if is_active(app, id)? {
            relend(app, &t, true)?;
        }
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
        if let Some(by) = held(app)?.get(name).filter(|x| **x != t.id()) {
            return Ok(Some(format!("Waiting for a free {name} ({name} is with {})", rf("task", *by))));
        }
    } else if !everyone.iter().any(|d| tags_of(d).contains(&first.tag)) && !pools.iter().any(|r| purposes(r).contains(&first.tag)) {
        return Ok(Some(format!("No {} yet (tb device add)", first.tag)));
    }
    let tag = &first.tag;
    let one = format!("{} {tag} device", a_or_an(tag));
    // The devices that would answer it but are switched off (in its goal's own pool, when it's lent
    // only from that), named with a way to switch one on.
    let only_ours = app.cfg.devices.goal_pool_only && !ours.is_empty();
    let mut off: Vec<String> = all.iter().filter(|d| d.b("off") && answers(d, tag) && (!only_ours || ours.contains(&d.st("name")))).map(|d| d.st("name")).collect();
    off.sort();
    let switch_on = off.first().map(|n| format!("; switch one on with tb device set {n} --on")).unwrap_or_default();
    let off_why: Vec<String> = off.iter().map(|n| format!("{n} is off")).collect();
    if have < want && only_ours {
        // Lent only from its goal's own devices.
        let g = rf("goal", t.i0("goal_id"));
        let why = if off.is_empty() { String::new() } else { format!(" ({}{switch_on})", off_why.join("; ")) };
        return Ok(Some(if have == 0 && off.is_empty() {
            format!("Needs {one}, and {g}'s own devices have none (tb goal devices {g} --add)")
        } else if have == 0 {
            format!("Needs {one}, and {g}'s own devices have none on{why}")
        } else {
            format!("Needs {want} {tag} devices, and {g}'s own devices have {have}{why}")
        }));
    }
    if have < want {
        // Devices that would answer it but are reserved for other goals, or switched off.
        let res = reserved(&pools);
        let mut kept: Vec<String> = everyone
            .iter()
            .filter(|d| !d.b("off") && answers(d, tag))
            .filter_map(|d| {
                res.get(&d.st("name"))
                    .filter(|gs| !gs.iter().any(|g| Some(*g) == t.i("goal_id")))
                    .map(|gs| format!("{} is reserved for {}", d.st("name"), goals_text(gs)))
            })
            .collect();
        kept.sort();
        kept.extend(off_why);
        let why = if kept.is_empty() { String::new() } else { format!(" ({}{switch_on})", kept.join("; ")) };
        return Ok(Some(if have == 0 && kept.is_empty() {
            format!("Needs {one}, and the pool has none (tb device add)")
        } else if have == 0 {
            format!("Needs {one}, and the pool has none for it{why}")
        } else {
            format!("Needs {want} {tag} devices, and the pool has {have}{why}")
        }));
    }
    // Enough are on, but others have them: say which device each task has.
    let held = held(app)?;
    let mut with: Vec<(String, String)> = all
        .iter()
        .filter(|d| answers(d, tag))
        .filter_map(|d| held.get(&d.st("name")).filter(|x| **x != t.id()).map(|x| (rf("task", *x), d.st("name"))))
        .collect();
    with.sort();
    let mut by: Vec<String> = vec![];
    for (task, _) in &with {
        if by.last() != Some(task) {
            by.push(task.clone());
        }
    }
    let why: Vec<String> = by
        .iter()
        .map(|task| {
            let names: Vec<String> = with.iter().filter(|(t, _)| t == task).map(|(_, n)| n.clone()).collect();
            format!("{} {} with {task}", and_list(&names), if names.len() == 1 { "is" } else { "are" })
        })
        .collect();
    let what = if want > 1 { format!("{want} {tag} devices") } else { one };
    Ok(Some(if why.is_empty() { format!("Waits for {what}") } else { format!("Waits for {what} ({})", why.join("; ")) }))
}

/// "a phone", "an android".
fn a_or_an(word: &str) -> &'static str {
    if word.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    }
}

/// "pixel-7", "pixel-7 and pixel-8", "pixel-6, pixel-7 and pixel-8".
fn and_list(names: &[String]) -> String {
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => names.join(""),
    }
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

fn is_active(app: &App, task_id: i64) -> Result<bool> {
    Ok(app.db.count(&format!("SELECT COUNT(*) FROM tasks t WHERE t.id = ? AND {ACTIVE_SQL}"), p![task_id])? > 0)
}

/// Lends a running task what it asks for and lacks from the free devices. `fresh` (its needs just
/// changed): gives back those it no longer asks for, as a start would. Otherwise it keeps all it has,
/// switched off or not, and only gains. The names newly lent.
fn relend(app: &App, t: &Row, fresh: bool) -> Result<Vec<String>> {
    let needs = needs(app, t)?;
    let had = lent(app, t.id())?;
    let (seen, ours) = view(app, t.i("goal_id"))?;
    let mut free = free_in(app, &seen, t.id())?;
    if !fresh {
        free.extend(seen.iter().filter(|d| d.b("off") && had.contains(&d.st("name"))).cloned());
    }
    let (picked, _) = fill(&needs, &free, &had, &ours);
    let now = now_iso();
    if fresh {
        for d in had.iter().filter(|d| !picked.contains(d)) {
            app.db.x("UPDATE device_loans SET released_at = ? WHERE task_id = ? AND device = ? AND released_at IS NULL", p![now, t.id(), d])?;
        }
    }
    let new: Vec<String> = picked.into_iter().filter(|d| !had.contains(d)).collect();
    for d in &new {
        app.db.insert("device_loans", crate::fields!["device" => d, "task_id" => t.id(), "at" => now])?;
    }
    if !new.is_empty() {
        board::log_event(app, t.id(), board::BOARD, "note", &format!("Lent it {}", new.join(", ")))?;
        board::bump_ctx(app, t.id())?;
    }
    Ok(new)
}

/// Lends running tasks that are short of what they ask for the devices that have come free.
pub fn top_up(app: &App) -> Result<usize> {
    let mut n = 0;
    for t in app.db.q(&format!("SELECT t.* FROM tasks t WHERE {ACTIVE_SQL} ORDER BY t.id"), p![])? {
        if !needs(app, &t)?.is_empty() {
            n += relend(app, &t, false)?.len();
        }
    }
    Ok(n)
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
    let focus = d.s("focus").filter(|f| !f.trim().is_empty()).is_some()
        || !app.cfg.devices.focus.trim().is_empty()
        || crate::devfocus::builtin_for(&d.st("kind"), &d.st("target")).is_some();
    let pools: Vec<Row> = goal_pools(app)?.into_iter().filter(|r| r.s("device") == d.s("name")).collect();
    let by: Vec<i64> = pools.iter().filter(|r| r.b("reserved")).map(|r| r.i0("goal_id")).collect();
    let reserved_for = (!by.is_empty()).then(|| goals_text(&by));
    let goals: Vec<Value> = pools.iter().map(|r| json!({"goal": rf("goal", r.i0("goal_id")), "purpose": r.v("purpose"), "reserved": r.b("reserved")})).collect();
    let kind = kind_label(d.s("kind").unwrap_or(""));
    Ok(json!({"id": d.id(), "name": d.v("name"), "label": label(d), "kind": d.v("kind"), "kind_label": kind, "target": d.v("target"),
              "tags": tags_of(d), "note": d.v("note"), "off": d.b("off"), "focus": d.v("focus"), "can_focus": focus,
              "start_cmd": d.v("start_cmd"), "stop_cmd": d.v("stop_cmd"), "held_by": holder, "goals": goals, "reserved_for": reserved_for}))
}

/// `GET /devices`: the pool, who has each device, and the tasks waiting for one: queued ones, and
/// running ones short of what they ask for.
pub fn overview(app: &App) -> Result<Value> {
    let held = held(app)?;
    let devices = pool(app)?.iter().map(|d| device_dict(d, &held, app)).collect::<Result<Vec<_>>>()?;
    let mut waiting = vec![];
    let sql = format!("SELECT t.* FROM tasks t WHERE (t.status = 'queued' AND t.start_job IS NULL) OR {ACTIVE_SQL} ORDER BY t.id");
    for t in app.db.q(&sql, p![])? {
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
    let uses = uses_in_goal(app, goal_id, &tasks, &held)?;
    for v in devices.iter_mut() {
        let name = v["name"].as_str().unwrap_or("").to_string();
        v["uses"] = uses.get(&name).cloned().unwrap_or_else(|| json!({"has": [], "can": [], "had": []}));
    }
    devices.sort_by_key(|v| v["in_pool"] != true);
    let ours: Vec<i64> = tasks.iter().map(|t| t.id()).collect();
    let lent = held.iter().filter(|(_, t)| ours.contains(t)).count();
    let empty_why = mine.is_empty().then_some(if all.is_empty() {
        "The device pool is empty. Terminals add emulators and simulators to it. Each task gets its own while it runs."
    } else {
        "None picked yet, so tasks here can use any free device in the pool. Each task gets its own while it runs."
    });
    Ok(json!({"needs": needs_json(&needs), "needs_text": needs_text(&needs), "devices": devices, "lent_here": lent, "pool": mine.len(), "empty_why": empty_why}))
}

/// Per device, the goal's tasks that have it now, can get it when they start, or had it before, each
/// task once per device (has, then had, then can) as `{ref, n, title}` with `n` its place in the goal.
/// A task that isn't done can get every device in its goal's view that meets one of its needs.
fn uses_in_goal(app: &App, goal_id: i64, tasks: &[Row], held: &HashMap<String, i64>) -> Result<HashMap<String, Value>> {
    let (seen, _) = view(app, Some(goal_id))?;
    let mut out: HashMap<String, [Vec<Value>; 3]> = HashMap::new();
    for (ix, t) in tasks.iter().enumerate() {
        let entry = json!({"ref": rf("task", t.id()), "n": ix + 1, "title": t.v("title")});
        let had: Vec<String> = app
            .db
            .q("SELECT DISTINCT device FROM device_loans WHERE task_id = ?", p![t.id()])?
            .iter()
            .filter_map(|r| r.s("device").map(str::to_string))
            .collect();
        let open = !matches!(t.s("status"), Some("done")) && !t.b("failed");
        let needs = if open { needs(app, t)? } else { vec![] };
        for d in &seen {
            let name = d.st("name");
            let slot = if held.get(&name) == Some(&t.id()) {
                0
            } else if had.contains(&name) {
                2
            } else if !d.b("off") && needs.iter().any(|n| answers(d, &n.tag) && (!d.b("name_only") || n.tag == name)) {
                1
            } else {
                continue;
            };
            out.entry(name).or_default()[slot].push(entry.clone());
        }
    }
    Ok(out.into_iter().map(|(k, [has, can, had])| (k, json!({"has": has, "can": can, "had": had}))).collect())
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
    let n = from_path(name).trim().to_lowercase();
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

/// A name from a URL path, %-escapes decoded ("pixel%20live" is "pixel live").
fn from_path(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%' && i + 2 < bytes.len()).then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())).flatten();
        match hex {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| name.to_string())
}

fn get(app: &App, name: &str) -> Result<Row> {
    let n = from_path(name).trim().to_lowercase();
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

/// A need only this device meets: it names the device, and no other device in the pool answers it.
fn only_this(need: &Need, d: &Row, others: &[Row]) -> bool {
    need.tag == d.st("name") && !others.iter().any(|o| answers(o, &need.tag))
}

/// The device_needs rows with a need only this device meets, as (owner ref, its needs).
fn needing(app: &App, d: &Row) -> Result<Vec<(String, Vec<Need>)>> {
    let others: Vec<Row> = pool(app)?.into_iter().filter(|o| o.id() != d.id()).collect();
    let mut out = vec![];
    for r in app.db.q("SELECT owner FROM device_needs ORDER BY owner", p![])? {
        let owner = r.st("owner");
        let needs = read_needs(app, &owner)?.unwrap_or_default();
        if needs.iter().any(|n| only_this(n, d, &others)) {
            out.push((owner, needs));
        }
    }
    Ok(out)
}

/// A device_needs owner ("T4", "G2") as a task or goal id.
enum Owner {
    Task(i64),
    Goal(i64),
    Other,
}

fn owner_of(owner: &str) -> Owner {
    let id = owner.get(1..).and_then(|n| n.parse().ok());
    match (owner.chars().next(), id) {
        (Some('T'), Some(id)) => Owner::Task(id),
        (Some('G'), Some(id)) => Owner::Goal(id),
        _ => Owner::Other,
    }
}

/// The tasks not finished and the goals not archived among those owners: the ones still waiting on it.
fn still_needing(app: &App, owners: &[(String, Vec<Need>)]) -> Result<Vec<String>> {
    let mut out = vec![];
    for (owner, _) in owners {
        let live = match owner_of(owner) {
            Owner::Task(t) => board::find_task(app, Some(t))?.is_some_and(|t| !matches!(t.s("status"), Some("done")) && !t.b("failed")),
            Owner::Goal(g) => board::find_goal(app, Some(g))?.is_some_and(|g| !g.b("archived")),
            Owner::Other => false,
        };
        if live {
            out.push(owner.clone());
        }
    }
    Ok(out)
}

/// Removes a device from the pool. Refused while a task has it, or while tasks or goals still need it
/// (by name, with no other device to meet it); `force` removes it anyway and drops it from their needs.
fn remove(app: &App, name: &str, body: &Value) -> Result<Value> {
    let d = get(app, name)?;
    let dev = d.st("name");
    if let Some(t) = held(app)?.get(&dev) {
        return err(409, format!("{} has {dev} now. Remove it once that task is done, or switch it off with tb device set {dev} --off.", rf("task", *t)));
    }
    let owners = needing(app, &d)?;
    let live = still_needing(app, &owners)?;
    let force = as_bool(body.get("force"), false);
    if !live.is_empty() && !force {
        return err(
            409,
            format!("{} still {} {dev}. Change what they need first, or remove it anyway with tb device remove {dev} --force, which drops it from their needs.", and_list(&live), if live.len() == 1 { "needs" } else { "need" }),
        );
    }
    let others: Vec<Row> = pool(app)?.into_iter().filter(|o| o.id() != d.id()).collect();
    for (owner, needs) in &owners {
        let left: Vec<Need> = needs.iter().filter(|n| !only_this(n, &d, &others)).cloned().collect();
        // A task's empty row is its own "needs none"; a goal that needs nothing has no row.
        if left.is_empty() && !matches!(owner_of(owner), Owner::Task(_)) {
            app.db.x("DELETE FROM device_needs WHERE owner = ?", p![owner])?;
        } else {
            app.db.x("UPDATE device_needs SET needs = ? WHERE owner = ?", p![jdumps(&needs_json(&left)), owner])?;
        }
        if !live.contains(owner) {
            continue;
        }
        let line = format!("{dev} was removed from the device pool, so it no longer needs it");
        match owner_of(owner) {
            Owner::Task(t) => {
                board::log_event(app, t, &board::caller(app, body, &app.cfg.owner), "note", &line)?;
                board::bump_ctx(app, t)?;
            }
            Owner::Goal(g) => {
                board::add_goal_note(app, g, "finding", &line, Some("board"), false, None)?;
                for t in board::goal_tasks(app, g)? {
                    board::bump_ctx(app, t.id())?;
                }
            }
            Owner::Other => {}
        }
    }
    app.db.x("DELETE FROM devices WHERE id = ?", p![d.id()])?;
    app.db.x("DELETE FROM goal_devices WHERE device = ?", p![dev])?;
    Ok(json!({"ok": true, "removed": d.v("name"), "dropped_from": live}))
}

/// The command that raises a device's window: its own, else `[devices] focus`, with `{name}`, `{target}`
/// and `{kind}` filled in. None: the board's own focus for a simulator or emulator (`devfocus`), if any.
pub fn focus_command(app: &App, d: &Row) -> Option<String> {
    let cmd = d.s("focus").filter(|f| !f.trim().is_empty()).map(|s| s.to_string()).or_else(|| Some(app.cfg.devices.focus.clone()).filter(|f| !f.trim().is_empty()))?;
    Some(cmd.replace("{name}", &d.st("name")).replace("{target}", &d.st("target")).replace("{kind}", &d.st("kind")))
}

fn focus(app: &App, name: &str) -> Result<Value> {
    let d = get(app, name)?;
    let dev = d.st("name");
    let timeout = app.cfg.devices.focus_timeout_secs.unwrap_or(15.0);
    let Some(cmd) = focus_command(app, &d) else {
        if crate::devfocus::builtin_for(&d.st("kind"), &d.st("target")).is_some() {
            let raised = crate::progress::waiting(&format!("{dev} to come up"), crate::progress::WAITING_EVERY, || crate::devfocus::focus(&dev, &d.st("kind"), &d.st("target"), timeout));
            return match raised {
                Ok(()) => Ok(json!({"ok": true, "device": d.v("name")})),
                Err(why) => err(409, why),
            };
        }
        return err(
            409,
            format!("{dev} has no focus command. Give it --kind ios or android and its --target (the board raises those itself), or a command with tb device set {dev} --focus \"…\", or set [devices] focus in config.toml."),
        );
    };
    let env = vec![
        ("TASKBOARD_DEVICE".to_string(), dev.clone()),
        ("TASKBOARD_DEVICE_TARGET".to_string(), d.st("target")),
        ("TASKBOARD_DEVICE_KIND".to_string(), d.st("kind")),
    ];
    // Waited for, so the person or agent who asked sees it when nothing came up.
    let run = || crate::proc::run_with(std::path::Path::new("/bin/sh"), &["-c".to_string(), cmd], None, timeout, &env, None);
    let failed = match crate::progress::waiting(&format!("{dev}'s focus command"), crate::progress::WAITING_EVERY, run) {
        Ok(o) if o.code == Some(0) => None,
        Ok(o) => {
            let said = one_line(if o.stderr.trim().is_empty() { &o.stdout } else { &o.stderr }, 300);
            Some(if said.is_empty() { format!("its focus command exited {}", o.code.map(|c| c.to_string()).unwrap_or_else(|| "on a signal".into())) } else { said })
        }
        Err(crate::proc::RunError::TimedOut) => Some(format!("its focus command took over {timeout} seconds")),
        Err(crate::proc::RunError::Spawn(e)) => Some(format!("its focus command couldn't start: {e}")),
    };
    if let Some(why) = failed {
        return err(409, format!("Couldn't bring up {dev}: {why}"));
    }
    Ok(json!({"ok": true, "device": d.v("name")}))
}

/// `/devices…` routes.
pub fn route(app: &App, method: &str, rest: &[&str], body: &Value) -> Result<Value> {
    match (method, rest) {
        ("GET", []) => overview(app),
        ("POST", []) => app.db.tx(|| add(app, body)),
        ("GET", [name]) => device_dict(&get(app, name)?, &held(app)?, app),
        ("POST", [name]) => app.db.tx(|| set(app, name, body)),
        ("POST", [name, "remove"]) => app.db.tx(|| remove(app, name, body)),
        ("POST", [name, "focus"]) => focus(app, name),
        _ => err(404, "There's nothing at that address."),
    }
}

/// The devices lent to a task as goal setup placeholders: `{device}` and `{target}` for the first,
/// `{device2}` and `{target2}` for the second, and so on. Empty when it has none, so a setup that names
/// them still reads.
pub fn setup_vars(app: &App, t: &Row) -> Result<std::collections::BTreeMap<String, String>> {
    let mut v = std::collections::BTreeMap::new();
    v.insert("device".to_string(), String::new());
    v.insert("target".to_string(), String::new());
    v.insert("device2".to_string(), String::new());
    v.insert("target2".to_string(), String::new());
    let names = lent(app, t.id())?;
    for (i, n) in names.iter().enumerate() {
        let sfx = if i == 0 { String::new() } else { (i + 1).to_string() };
        let target = app.db.q1("SELECT target FROM devices WHERE name = ?", p![n])?.and_then(|r| r.s("target").map(str::to_string)).unwrap_or_default();
        v.insert(format!("device{sfx}"), n.clone());
        v.insert(format!("target{sfx}"), target);
    }
    Ok(v)
}

/// What fills a device's start and stop commands: the task's step placeholders (`{task}`, `{branch}`,
/// …, with `{jira}` falling back to the task's ref when it has no ticket), its `{n}`, `{wave}` and
/// `{goal}`, and the device's `{device}` (also `{name}`), `{kind}` and `{target}`.
pub fn cmd_vars(app: &App, t: &Row, d: &Row) -> std::collections::BTreeMap<String, String> {
    let mut v = crate::steps::vars_for(app, t);
    if v.get("jira").is_none_or(|j| j.trim().is_empty()) {
        v.insert("jira".into(), rf("task", t.id()));
    }
    v.insert("n".into(), t.id().to_string());
    v.insert("wave".into(), t.i("wave").map(|w| w.to_string()).unwrap_or_default());
    v.insert("goal".into(), t.i("goal_id").map(|g| rf("goal", g)).unwrap_or_default());
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

/// The handoff's line about the devices lent to the task, how to start and stop each, and which
/// devices other tasks have. Lent devices are described from their own rows (as the Python board
/// did from the task's loans), so one reserved for another goal since it was lent keeps its details.
/// A task that needs devices but has none still hears which ones to leave alone. `starting`: the
/// handoff it starts with (it has started, though `started_at` isn't set yet).
/// A task that asks for no devices, while the pool has some: how to get one rather than grabbing one.
fn pick_none_line(app: &App, t: &Row) -> Result<Vec<String>> {
    if pool(app)?.is_empty() {
        return Ok(vec![]);
    }
    let tb = board::tb_cmd(app);
    Ok(vec![format!(
        "If this task needs an emulator, simulator or phone, don't pick one yourself: run {tb} task set {} --device tag:<tag> \
         ({tb} devices lists them) and the board lends you a free one that no other terminal is using.",
        rf("task", t.id())
    )])
}

pub fn handoff_lines(app: &App, t: &Row, starting: bool) -> Result<Vec<String>> {
    let names = lent(app, t.id())?;
    if names.is_empty() {
        let needs = needs(app, t)?;
        if needs.is_empty() {
            return pick_none_line(app, t);
        }
        let mut text = if starting || has(t.s("started_at")) {
            format!(
                "This task asks for devices ({}), but none were free when it started. Run {} devices to see who has them.",
                needs_text(&needs),
                board::tb_cmd(app)
            )
        } else {
            format!(
                "This task asks for devices ({}). The board lends it to this task when it starts, and this handoff then says which.",
                needs_text(&needs)
            )
        };
        let others = others_text(app, t, &[])?;
        if !others.is_empty() {
            text += &format!("\nOther devices in use, don't touch them: {others}.");
        }
        return Ok(vec![text]);
    }
    let (seen, _) = view(app, t.i("goal_id"))?;
    let all = pool(app)?;
    // The task's view first (its goal's purposes added to the tags), else the device's own row.
    let rows: Vec<Option<&Row>> =
        names.iter().map(|n| seen.iter().chain(&all).find(|d| d.s("name") == Some(n.as_str()))).collect();
    let described: Vec<String> = names
        .iter()
        .zip(&rows)
        .map(|(n, d)| match d {
            Some(d) => {
                // What it is (kind, target), else its tags; then its note, clipped to 300 as the
                // Python board did (imported notes can run past the 200 a new one may have).
                let mut parts: Vec<String> = match what(d) {
                    w if w.is_empty() => tags_of(d),
                    w => vec![w],
                };
                parts.extend(d.s("note").filter(|x| !x.is_empty()).map(|x| clip(x, 300)));
                if parts.is_empty() { n.clone() } else { format!("{n} ({})", parts.join(", ")) }
            }
            None => n.clone(),
        })
        .collect();
    let one = names.len() == 1;
    let mut text = format!(
        "The board lent this task {} {}: use only {}, since other tasks have the rest of the pool. {} when the task is done.",
        if one { "the device" } else { "the devices" },
        described.join(", "),
        if one { "that one" } else { "these" },
        if one { "It goes back" } else { "They go back" }
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
    let others = others_text(app, t, &names)?;
    if !others.is_empty() {
        text += &format!("\nOther devices in use, don't touch them: {others}.");
    }
    Ok(vec![text])
}

/// Every device this task must leave alone, as the Python board listed them: each with its target
/// and why (`with T5`, `reserved for G3`, `kept for …`), joined with "; " and clipped to 500.
fn others_text(app: &App, t: &Row, mine: &[String]) -> Result<String> {
    let holders = held(app)?;
    let res = reserved(&goal_pools(app)?);
    let mut list = vec![];
    for d in pool(app)? {
        let name = d.st("name");
        if mine.contains(&name) {
            continue;
        }
        let mut why = vec![];
        if let Some(tid) = holders.get(&name).filter(|tid| **tid != t.id()) {
            why.push(format!("with {}", rf("task", *tid)));
        }
        if let Some(gs) = res.get(&name).filter(|gs| !gs.iter().any(|g| Some(*g) == t.i("goal_id"))) {
            why.push(format!("reserved for {}", goals_text(gs)));
        }
        if d.b("off") {
            why.push(kept_for(&d));
        }
        if why.is_empty() {
            continue;
        }
        let target = d.s("target").map(str::trim).filter(|x| !x.is_empty()).map(|x| format!(" {x}")).unwrap_or_default();
        list.push(format!("{name}{target} ({})", why.join(", ")));
    }
    Ok(clip(&list.join("; "), 500))
}

/// Why a device is kept back: its note's "Kept for …" (as imported from the Python board), else just that.
fn kept_for(d: &Row) -> String {
    static KEPT: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bkept for ([^;]+)").unwrap());
    match d.s("note").and_then(|n| KEPT.captures(n)) {
        Some(c) => format!("kept for {}", short(c[1].trim(), 80)),
        None => "kept back".into(),
    }
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
        assert_eq!(
            clean_needs(Some(&json!("tag:phone, tag:android:2, device:pixel-7"))).unwrap(),
            vec![Need { tag: "phone".into(), n: 1 }, Need { tag: "android".into(), n: 2 }, Need { tag: "pixel-7".into(), n: 1 }]
        );
        assert_eq!(clean_needs(Some(&json!([{"tag": "tag:ios"}]))).unwrap(), vec![Need { tag: "ios".into(), n: 1 }]);
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
