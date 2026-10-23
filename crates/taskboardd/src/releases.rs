//! Releases: what shipped together. Cutting a release (`tb release cut v1.4`) takes every task in
//! the project that's done and not in a release yet (its PR merged, or done with no PR); the rest
//! of the done work waits in Unreleased for the next cut. Releases are a per-project setting, off
//! unless the project (`tb project set --releases on`) or config.toml's `[releases] on` turns them on.
//! A release whose build or publish failed is marked failed (`tb release fail R3`): its tasks go back
//! in Unreleased and its name is free, so the next cut can take the same name.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, p};

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS releases(
  id INTEGER PRIMARY KEY, project TEXT NOT NULL, name TEXT NOT NULL, notes TEXT,
  cut_at TEXT, cut_by TEXT, updated_at TEXT);
CREATE INDEX IF NOT EXISTS releases_project ON releases(project, id);
"#;

pub const ADDED: &[(&str, &str, &str)] = &[
    ("tasks", "release_id", "INT"),
    ("releases", "failed_at", "TEXT"),
    ("releases", "failed_by", "TEXT"),
    ("releases", "failed_reason", "TEXT"),
];

pub const INDEXES: &str = "CREATE INDEX IF NOT EXISTS tasks_release ON tasks(release_id);";

/// `[releases]` in config.toml.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ReleasesConfig {
    /// Releases for every project that doesn't say.
    pub on: bool,
}

/// `tb project set --releases`: name → whether the project has releases, over config.toml's.
const SETTING: &str = "project_releases";
/// `tb project set --release-script`: name → the command `tb release cut` runs first.
const SCRIPT_SETTING: &str = "project_release_script";
const SCRIPT_MAX: usize = 500;

/// A done task that goes in the next cut: not failed, in no release, its PR merged or with no PR.
const RELEASABLE: &str = "status = 'done' AND COALESCE(failed, 0) = 0 AND release_id IS NULL
  AND ((pr_num IS NULL AND COALESCE(pr_url, '') = '') OR pr_phase = 'merged' OR pr_state = 'merged')";

const NAME_MAX: usize = 80;
const NOTES_MAX: usize = 2000;
const REASON_MAX: usize = 500;

fn overrides(app: &App) -> Result<Row> {
    Ok(jloads_obj(app.db.get_setting(SETTING)?.as_deref()))
}

/// What `tb project set --releases` put on a project, if anything.
pub fn setting(app: &App, project: &str) -> Result<Option<bool>> {
    Ok(overrides(app)?.get(project).and_then(|v| v.as_bool()))
}

/// Whether a project has releases: its own word, else config.toml's `[releases] on`.
pub fn on(app: &App, project: &str) -> bool {
    setting(app, project).ok().flatten().unwrap_or(app.cfg.releases.on)
}

/// Sets a project's releases from a body's `releases` (on/off, true/false; null or "default": back
/// to config.toml's). False when the body doesn't name it.
pub fn set_setting(app: &App, project: &str, body: &Value) -> Result<bool> {
    let Some(v) = body.get("releases") else { return Ok(false) };
    let to = match v {
        Value::Null => None,
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim().to_lowercase().as_str() {
            "on" | "true" => Some(true),
            "off" | "false" => Some(false),
            "default" => None,
            _ => return err(400, "releases is on, off or default."),
        },
        _ => return err(400, "releases is on, off or default."),
    };
    let mut all = overrides(app)?;
    match to {
        Some(b) => all.insert(project.into(), json!(b)),
        None => all.remove(project),
    };
    app.db.set_setting(SETTING, Some(&jdumps(&Value::Object(all))))?;
    Ok(true)
}

fn need_on(app: &App, project: &str) -> Result<()> {
    if !on(app, project) {
        return err(409, format!("Releases are off for {project}. Turn them on with: tb project set {project} --releases on"));
    }
    Ok(())
}

/// The project a request is about: the body's or query's, else the caller's task's, else the project
/// of the folder `tb` ran in.
fn project_for(app: &App, given: &str, session: Option<&str>, cwd: Option<&str>) -> Result<String> {
    if !given.is_empty() {
        if !crate::projects::list_projects(app)?.iter().any(|p| p["name"] == given) {
            return err(404, format!("There's no project called {given}."));
        }
        return Ok(given.to_string());
    }
    if let Some(p) = board::task_for_session(app, session)?.and_then(|t| t.s("project").filter(|p| !p.is_empty()).map(|p| p.to_string())) {
        return Ok(p);
    }
    crate::projects::project_for_path(app, cwd)?.ok_or_else(|| ApiError::new(400, "Say which project (--project)."))
}

fn body_project(app: &App, body: &Value) -> Result<String> {
    project_for(app, &body_str(body, "project"), body["by_session"].as_str(), body["cwd"].as_str())
}

fn query_project(app: &App, query: &crate::api::Query) -> Result<String> {
    let q = |k: &str| query.get(k).map(|s| s.as_str()).filter(|s| !s.is_empty());
    project_for(app, q("project").unwrap_or(""), q("by_session"), q("cwd"))
}

pub fn get(app: &App, r: &str) -> Result<Row> {
    let id = need_ref(&json!(r), "release")?;
    app.db.q1("SELECT * FROM releases WHERE id = ?", p![id])?
        .ok_or_else(|| ApiError::new(404, format!("There's no release {}.", rf("release", id))))
}

pub(crate) fn clean_name(s: &str) -> Result<String> {
    let n = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if n.is_empty() {
        return err(400, "Say what the release is called (v1.4, 2026.10).");
    }
    if n.chars().count() > NAME_MAX {
        return err(400, format!("A release's name is {NAME_MAX} characters at most."));
    }
    if n.eq_ignore_ascii_case("unreleased") {
        return err(400, "“Unreleased” is the list of done work not in a release yet; call the release something else.");
    }
    Ok(n)
}

fn clean_notes(body: &Value) -> Result<Option<String>> {
    let n = body_str(body, "notes").trim().to_string();
    if n.chars().count() > NOTES_MAX {
        return err(400, format!("A release's notes are {NOTES_MAX} characters at most."));
    }
    Ok(if n.is_empty() { None } else { Some(n) })
}

/// Whether the project has a release with this name (any case) besides release `but`. A failed release
/// doesn't hold its name.
pub(crate) fn named(app: &App, project: &str, name: &str, but: i64) -> Result<bool> {
    Ok(app.db.count("SELECT COUNT(*) FROM releases WHERE project = ? AND name = ? COLLATE NOCASE AND id != ? AND failed_at IS NULL", p![project, name, but])? > 0)
}

/// Whether the project has a failed release with this name (any case): a cut by that name replaces it.
pub(crate) fn failed_named(app: &App, project: &str, name: &str) -> Result<bool> {
    Ok(app.db.count("SELECT COUNT(*) FROM releases WHERE project = ? AND name = ? COLLATE NOCASE AND failed_at IS NOT NULL", p![project, name])? > 0)
}

fn name_taken(app: &App, project: &str, name: &str, but: i64) -> Result<()> {
    if named(app, project, name, but)? {
        return err(409, format!("{project} already has a release called {name}."));
    }
    if crate::pending_cut::holds_name(app, project, name)? {
        return err(409, format!("{name} is held by {project}'s pending cut. Cancel it first with: tb release cancel"));
    }
    Ok(())
}

fn task_count(app: &App, id: i64) -> Result<i64> {
    app.db.count("SELECT COUNT(*) FROM tasks WHERE release_id = ?", p![id])
}

pub(crate) fn unreleased_rows(app: &App, project: &str) -> Result<Vec<Row>> {
    app.db.q(&format!("SELECT * FROM tasks WHERE project = ? AND {RELEASABLE} ORDER BY finished_at, id"), p![project])
}

/// Each project's newest release that didn't fail (`ref`, `name`, `cut_at`) and its count of done tasks waiting in
/// Unreleased, in two queries for every project at once (`GET /state`'s projects).
pub(crate) fn latest_and_unreleased(app: &App) -> Result<(HashMap<String, Value>, HashMap<String, i64>)> {
    let mut latest = HashMap::new();
    for r in app.db.q("SELECT * FROM releases WHERE id IN (SELECT MAX(id) FROM releases WHERE failed_at IS NULL GROUP BY project)", p![])? {
        latest.insert(r.st("project"), json!({"ref": rf("release", r.id()), "name": r.v("name"), "cut_at": r.v("cut_at")}));
    }
    let mut unreleased = HashMap::new();
    for r in app.db.q(&format!("SELECT project, COUNT(*) AS n FROM tasks WHERE project IS NOT NULL AND {RELEASABLE} GROUP BY project"), p![])? {
        unreleased.insert(r.st("project"), r.i("n").unwrap_or(0));
    }
    Ok((latest, unreleased))
}

pub fn dict(app: &App, r: &Row) -> Result<Value> {
    Ok(json!({
        "id": r.id(), "ref": rf("release", r.id()), "project": r.v("project"), "name": r.v("name"),
        "notes": r.v("notes"), "cut_at": r.v("cut_at"), "cut_by": r.v("cut_by"), "updated_at": r.v("updated_at"),
        "failed_at": r.v("failed_at"), "failed_by": r.v("failed_by"), "failed_reason": r.v("failed_reason"),
        "tasks": task_count(app, r.id())?,
        "run_url": r.v("run_url"), "run_looking": crate::release_run::looking(r),
    }))
}

fn task_brief(t: &Row) -> Value {
    json!({
        "id": t.id(), "ref": rf("task", t.id()), "title": t.v("title"), "summary": t.v("summary"),
        "goal": rf_opt("goal", t.i("goal_id")), "pr_url": t.v("pr_url"), "pr_num": t.v("pr_num"),
        "finished_at": t.v("finished_at"),
    })
}

/// The tasks grouped by goal, goals in the order their first task finished, tasks with no goal last.
fn groups(app: &App, tasks: &[Row]) -> Result<Vec<Value>> {
    let mut order: Vec<Option<i64>> = vec![];
    let mut by: HashMap<Option<i64>, Vec<Value>> = HashMap::new();
    for t in tasks {
        let g = t.i("goal_id");
        if !by.contains_key(&g) {
            order.push(g);
        }
        by.entry(g).or_default().push(task_brief(t));
    }
    order.sort_by_key(|g| g.is_none());
    order
        .into_iter()
        .map(|g| {
            let name = match g {
                Some(id) => board::get_goal(app, id).map(|r| r.st("name")).unwrap_or_default(),
                None => String::new(),
            };
            Ok(json!({"goal": rf_opt("goal", g), "name": if g.is_some() { json!(name) } else { Value::Null }, "tasks": by.remove(&g).unwrap_or_default()}))
        })
        .collect()
}

fn scripts(app: &App) -> Result<Row> {
    Ok(jloads_obj(app.db.get_setting(SCRIPT_SETTING)?.as_deref()))
}

/// The command `tb release cut` runs before recording a release, if the project has one.
pub fn script(app: &App, project: &str) -> Result<Option<String>> {
    Ok(scripts(app)?.get(project).and_then(|v| v.as_str()).map(str::to_string))
}

/// Sets a project's release script from a body's `release_script` (null, "", "none" or "default":
/// none). False when the body doesn't name it.
pub fn set_script(app: &App, project: &str, body: &Value) -> Result<bool> {
    let Some(v) = body.get("release_script") else { return Ok(false) };
    let to = match v {
        Value::Null => None,
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() || s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("default") {
                None
            } else if s.chars().count() > SCRIPT_MAX || s.contains(['\n', '\r']) {
                return err(400, format!("release_script is one line of {SCRIPT_MAX} characters at most, or none."));
            } else {
                Some(s.to_string())
            }
        }
        _ => return err(400, "release_script is a command like scripts/release.sh, or none."),
    };
    let mut all = scripts(app)?;
    match to {
        Some(s) => all.insert(project.into(), json!(s)),
        None => all.remove(project),
    };
    app.db.set_setting(SCRIPT_SETTING, Some(&jdumps(&Value::Object(all))))?;
    Ok(true)
}

/// A changelog ready to paste: the release's name and date, its notes, then its tasks under their goals.
fn markdown(title: &str, notes: Option<&str>, groups: &[Value]) -> String {
    let mut out = format!("## {title}\n");
    if let Some(n) = notes.filter(|n| !n.trim().is_empty()) {
        out.push_str(&format!("\n{}\n", n.trim()));
    }
    if groups.is_empty() {
        out.push_str("\nNothing yet.\n");
    }
    for g in groups {
        let heading = g["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(if g["goal"].is_null() { "Other changes" } else { "Untitled goal" });
        out.push_str(&format!("\n### {heading}\n\n"));
        for t in g["tasks"].as_array().into_iter().flatten() {
            let pr = match (t["pr_num"].as_i64(), t["pr_url"].as_str().filter(|u| !u.is_empty())) {
                (Some(n), Some(u)) => format!(" ([#{n}]({u}))"),
                (None, Some(u)) => format!(" ([PR]({u}))"),
                _ => String::new(),
            };
            out.push_str(&format!("- {}{pr}\n", t["title"].as_str().unwrap_or("").trim()));
        }
    }
    out
}

fn date(at: Option<&str>) -> String {
    at.and_then(|s| s.get(..10)).unwrap_or("").to_string()
}

/// One release, or Unreleased, with its tasks grouped by goal and the changelog.
fn show(app: &App, r: &str, query: &crate::api::Query) -> Result<Value> {
    if r.eq_ignore_ascii_case("unreleased") {
        let project = query_project(app, query)?;
        need_on(app, &project)?;
        let tasks = unreleased_rows(app, &project)?;
        let gs = groups(app, &tasks)?;
        let md = markdown("Unreleased", None, &gs);
        return Ok(json!({"ref": "unreleased", "project": project, "name": "Unreleased", "tasks": tasks.len(), "groups": gs, "markdown": md}));
    }
    let rel = get(app, r)?;
    need_on(app, rel.s("project").unwrap_or(""))?;
    let tasks = app.db.q("SELECT * FROM tasks WHERE release_id = ? ORDER BY finished_at, id", p![rel.id()])?;
    let gs = groups(app, &tasks)?;
    let day = date(rel.s("cut_at"));
    let title = if day.is_empty() { rel.st("name") } else { format!("{} ({day})", rel.st("name")) };
    let mut v = dict(app, &rel)?;
    v["markdown"] = json!(markdown(&title, rel.s("notes"), &gs));
    v["groups"] = json!(gs);
    Ok(v)
}

/// How many releases one page of `GET /releases` holds at most.
const PAGE_MAX: i64 = 500;

/// `GET /releases`: the project's releases, newest first, and how many done tasks wait in Unreleased.
/// `limit` caps the page and `before` (a release id) starts after that release, with `more` saying
/// whether older ones are left. `counts` is the whole project's: every release, those cut in the last
/// 30 days, and those marked failed.
fn list(app: &App, query: &crate::api::Query) -> Result<Value> {
    let project = query_project(app, query)?;
    if !on(app, &project) {
        return Ok(json!({"project": project, "on": false, "unreleased": 0, "releases": [], "total": 0, "pending": null, "more": false}));
    }
    let num = |k: &str| query.get(k).and_then(|s| s.parse::<i64>().ok());
    let limit = num("limit").map(|n| n.clamp(1, PAGE_MAX));
    let before = query.get("before").and_then(|s| s.trim().trim_start_matches(['R', 'r']).parse::<i64>().ok()).unwrap_or(i64::MAX);
    let mut rows = app.db.q("SELECT * FROM releases WHERE project = ? AND id < ? ORDER BY id DESC LIMIT ?", p![project, before, limit.map_or(-1, |n| n + 1)])?;
    let more = limit.is_some_and(|n| rows.len() as i64 > n);
    if let Some(n) = limit {
        rows.truncate(n as usize);
    }
    let releases = rows.iter().map(|r| dict(app, r)).collect::<Result<Vec<_>>>()?;
    let pending = match crate::pending_cut::for_project(app, &project)? {
        Some(r) => crate::pending_cut::dict(app, &r)?,
        None => Value::Null,
    };
    let month_ago = iso(now_ts() - 30.0 * 86400.0);
    let count = |sql: &str, extra: Option<&str>| match extra {
        Some(x) => app.db.count(&format!("SELECT COUNT(*) FROM releases WHERE project = ? {sql}"), p![project, x]),
        None => app.db.count(&format!("SELECT COUNT(*) FROM releases WHERE project = ? {sql}"), p![project]),
    };
    let counts = json!({
        "total": count("", None)?,
        "recent": count("AND cut_at >= ?", Some(&month_ago))?,
        "failed": count("AND failed_at IS NOT NULL", None)?,
    });
    Ok(json!({
        "project": project, "on": true, "unreleased": unreleased_rows(app, &project)?.len(), "releases": releases,
        "total": counts["total"], "pending": pending, "more": more, "counts": counts,
    }))
}

/// `POST /releases`: cut a release holding every task in Unreleased. With `check`, only says whether
/// the cut would go through, and the project's release script and where to run it, without cutting:
/// `tb release cut` asks first so a doomed cut doesn't run the script. With `after` (tasks and goals),
/// sets up the project's pending cut instead, which the board cuts once they're through.
fn cut(app: &App, body: &Value) -> Result<Value> {
    let project = body_project(app, body)?;
    need_on(app, &project)?;
    let name = clean_name(&body_str(body, "name"))?;
    let notes = clean_notes(body)?;
    name_taken(app, &project, &name, 0)?;
    if !body["after"].is_null() {
        return crate::pending_cut::make(app, &project, &name, notes, body, script_dir(app, body, &project)?);
    }
    let tasks = unreleased_rows(app, &project)?;
    if tasks.is_empty() {
        return err(409, format!("Nothing to release in {project}: no done task waits in Unreleased."));
    }
    if body["check"] == true {
        let dir = script_dir(app, body, &project)?;
        // `replaces`: the script may move the failed release's tag instead of refusing it.
        let replaces = failed_named(app, &project, &name)?;
        return Ok(json!({"project": project, "name": name, "unreleased": tasks.len(), "script": script(app, &project)?, "dir": dir, "replaces": replaces}));
    }
    let who = board::caller(app, body, board::OWNER);
    let id = put_in_release(app, &project, &name, notes, &tasks, &who)?;
    // `script_secs`: how long `tb release cut` ran the release script, so its tag's run is one from since then.
    let since = body["script_secs"].as_f64().map(|s| now_ts() - s);
    crate::release_run::track(app, id, script_dir(app, body, &project)?.as_deref(), since)?;
    show(app, &rf("release", id), &crate::api::Query::new())
}

/// Where the release script runs: the terminal's checkout, unless --project named another project.
fn script_dir(app: &App, body: &Value, project: &str) -> Result<Option<String>> {
    let cwd = body["cwd"].as_str().filter(|c| !c.is_empty());
    let here = body_str(body, "project").is_empty() || crate::projects::project_for_path(app, cwd)?.as_deref() == Some(project);
    let path = crate::projects::list_projects(app)?.into_iter().find(|p| p["name"] == project).and_then(|p| p["path"].as_str().map(str::to_string));
    Ok(if here { cwd.map(str::to_string) } else { path })
}

/// Makes the release and puts the tasks in it, each with a "release" log line. The caller has checked the name.
pub(crate) fn put_in_release(app: &App, project: &str, name: &str, notes: Option<String>, tasks: &[Row], who: &str) -> Result<i64> {
    let now = now_iso();
    let id = app.db.insert("releases", fields!["project" => project, "name" => name, "notes" => notes, "cut_at" => now, "cut_by" => who, "updated_at" => now])?;
    let r = rf("release", id);
    for t in tasks {
        board::update_task(app, t.id(), fields!["release_id" => id])?;
        board::log_event(app, t.id(), who, "release", &format!("In release {name} ({r})"))?;
    }
    Ok(id)
}

/// `POST /releases/R<n>`: rename it or change its notes.
fn set(app: &App, r: &str, body: &Value) -> Result<Value> {
    let rel = get(app, r)?;
    let project = rel.st("project");
    need_on(app, &project)?;
    let mut f = fields![];
    if body.get("name").is_some_and(|v| !v.is_null()) {
        let name = clean_name(&body_str(body, "name"))?;
        name_taken(app, &project, &name, rel.id())?;
        f.push(("name", json!(name)));
    }
    if body.get("notes").is_some() {
        f.push(("notes", json!(clean_notes(body)?)));
    }
    if f.is_empty() {
        return err(400, "Say what to change: name or notes.");
    }
    f.push(("updated_at", json!(now_iso())));
    app.db.update("releases", &json!(rel.id()), f)?;
    dict(app, &get(app, r)?)
}

/// `POST /releases/R<n>/fail`: its build or publish failed. Its tasks go back in Unreleased, each with
/// a "release" log line, and its name is free for the next cut.
fn fail(app: &App, r: &str, body: &Value) -> Result<Value> {
    let rel = get(app, r)?;
    let project = rel.st("project");
    need_on(app, &project)?;
    let r = rf("release", rel.id());
    if rel.s("failed_at").is_some() {
        return err(409, format!("{r} is already marked failed."));
    }
    let reason = body_str(body, "reason").trim().to_string();
    if reason.chars().count() > REASON_MAX {
        return err(400, format!("A failed release's reason is {REASON_MAX} characters at most."));
    }
    let reason = if reason.is_empty() { None } else { Some(reason) };
    let who = board::caller(app, body, board::OWNER);
    let now = now_iso();
    let name = rel.st("name");
    for t in app.db.q("SELECT * FROM tasks WHERE release_id = ?", p![rel.id()])? {
        board::update_task(app, t.id(), fields!["release_id" => Value::Null])?;
        board::log_event(app, t.id(), &who, "release", &format!("Back in Unreleased: release {name} ({r}) failed"))?;
    }
    app.db.update("releases", &json!(rel.id()), fields!["failed_at" => now, "failed_by" => who, "failed_reason" => reason, "updated_at" => now])?;
    dict(app, &get(app, &r)?)
}

/// `POST /releases/move`: put a done task in another release, or back in Unreleased.
fn move_task(app: &App, body: &Value) -> Result<Value> {
    let tid = need_ref(&body["task"], "task")?;
    let t = board::get_task(app, tid)?;
    let project = t.st("project");
    need_on(app, &project)?;
    if t.s("status") != Some("done") {
        return err(409, format!("{} isn't done; only done work goes in a release.", rf("task", tid)));
    }
    let to = body_str(body, "release");
    let who = board::caller(app, body, board::OWNER);
    let (release, text) = if to.trim().eq_ignore_ascii_case("unreleased") || to.trim().is_empty() {
        (None, "Back in Unreleased".to_string())
    } else {
        let rel = get(app, &to)?;
        if rel.s("failed_at").is_some() {
            return err(409, format!("{} failed; put the task back in Unreleased, or in a release that went out.", rf("release", rel.id())));
        }
        if rel.s("project") != Some(project.as_str()) {
            return err(409, format!("{} is {}'s release; {} is in {project}.", rf("release", rel.id()), rel.st("project"), rf("task", tid)));
        }
        (Some(rel.id()), format!("In release {} ({})", rel.st("name"), rf("release", rel.id())))
    };
    if t.i("release_id") == release {
        return err(409, format!("{} is already there.", rf("task", tid)));
    }
    board::update_task(app, tid, fields!["release_id" => release])?;
    board::log_event(app, tid, &who, "release", &text)?;
    Ok(json!({"task": rf("task", tid), "release": rf_opt("release", release), "text": text}))
}

pub fn route(app: &App, method: &str, rest: &[&str], query: &crate::api::Query, body: &Value) -> Result<Value> {
    match (method, rest) {
        ("GET", []) => list(app, query),
        ("POST", []) => app.db.tx(|| cut(app, body)),
        ("POST", ["move"]) => app.db.tx(|| move_task(app, body)),
        ("POST", ["pending", "cancel"]) => app.db.tx(|| crate::pending_cut::cancel(app, &body_project(app, body)?)),
        ("POST", [r, "fail"]) => app.db.tx(|| fail(app, r, body)),
        ("GET", [r]) => show(app, r, query),
        ("POST", [r]) => app.db.tx(|| set(app, r, body)),
        _ => err(404, "There's nothing at that address."),
    }
}
