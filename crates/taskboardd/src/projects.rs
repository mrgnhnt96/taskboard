//! Projects: Midna's project list plus every project seen on tasks, goals and sessions.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::app::App;
use crate::p;
use crate::util::*;

const REMOTE_TTL: Duration = Duration::from_secs(600);
const PR_FLOW_SETTING: &str = "project_pr_flow";
pub const PR_FLOWS: &[&str] = &["auto", "on", "off"];

pub fn list_projects(app: &App) -> Result<Vec<Value>> {
    let mut seen: Vec<(String, Option<String>)> = vec![];
    let mut add = |name: Option<&str>, path: Option<&str>| {
        let Some(name) = name.filter(|n| !n.is_empty()) else { return };
        let path = path.filter(|p| !p.is_empty()).map(|p| p.to_string());
        match seen.iter_mut().find(|(n, _)| n == name) {
            Some((_, p)) if p.is_none() && path.is_some() => *p = path,
            Some(_) => {}
            None => seen.push((name.to_string(), path)),
        }
    };
    for p in jloads_arr(app.db.get_setting("midna_projects")?.as_deref()) {
        add(p.get("name").and_then(|v| v.as_str()), p.get("path").and_then(|v| v.as_str()));
    }
    for (sql, pc) in [
        ("SELECT DISTINCT project, repo_path AS path FROM tasks WHERE project IS NOT NULL AND project != ''", "path"),
        ("SELECT DISTINCT project, repo_path AS path FROM goals WHERE project IS NOT NULL AND project != ''", "path"),
        ("SELECT DISTINCT project, project_path AS path FROM sessions WHERE project IS NOT NULL AND project != ''", "path"),
    ] {
        for r in app.db.q(sql, p![])? {
            add(r.s("project"), r.s(pc));
        }
    }
    seen.sort_by_key(|(n, _)| n.to_lowercase());
    Ok(seen.into_iter().map(|(n, p)| json!({"name": n, "path": p})).collect())
}

pub fn project_path(app: &App, name: Option<&str>) -> Result<Option<String>> {
    let Some(name) = name.filter(|n| !n.is_empty()) else { return Ok(None) };
    Ok(list_projects(app)?
        .into_iter()
        .find(|p| p["name"] == name && p["path"].is_string())
        .and_then(|p| p["path"].as_str().map(|s| s.to_string())))
}

fn git_has_remote(path: &str) -> Option<bool> {
    let git = crate::proc::which("git")?;
    let out = crate::proc::run(&git, &["-C".into(), path.into(), "remote".into()], None, 5.0).ok()?;
    if out.code != Some(0) {
        return None;
    }
    Some(!out.stdout.trim().is_empty())
}

pub fn has_remote(app: &App, name: &str) -> Result<Option<bool>> {
    let Some(path) = project_path(app, Some(name))? else { return Ok(None) };
    if !std::path::Path::new(&path).is_dir() {
        return Ok(None);
    }
    if let Some((at, v)) = app.shared.lock().remotes.get(&path) {
        if at.elapsed() < REMOTE_TTL {
            return Ok(*v);
        }
    }
    let found = git_has_remote(&path);
    app.shared.lock().remotes.insert(path, (Instant::now(), found));
    Ok(found)
}

pub fn pr_flow_overrides(app: &App) -> Result<Row> {
    Ok(jloads_obj(app.db.get_setting(PR_FLOW_SETTING)?.as_deref()))
}

pub fn pr_flow(app: &App, name: &str) -> Result<String> {
    Ok(pr_flow_overrides(app)?.get(name).and_then(|v| v.as_str()).unwrap_or("auto").to_string())
}

pub fn set_pr_flow(app: &App, name: &str, flow: &str) -> Result<()> {
    let mut flows = pr_flow_overrides(app)?;
    if flow == "auto" {
        flows.remove(name);
    } else {
        flows.insert(name.into(), json!(flow));
    }
    app.db.set_setting(PR_FLOW_SETTING, Some(&jdumps(&Value::Object(flows))))
}

/// Whether a project's work ends in pull requests (and so gets Jira tickets): on when it has a git remote.
pub fn ships_prs(app: &App, name: &str) -> Result<bool> {
    let flow = pr_flow(app, name)?;
    if flow != "auto" {
        return Ok(flow == "on");
    }
    Ok(has_remote(app, name)? != Some(false))
}

/// Whether a task ends in a PR: its own setting (`tb task set --pr yes|no`), else its project's.
pub fn task_ships_pr(app: &App, t: &Row) -> Result<bool> {
    match t.i("ships_pr") {
        Some(v) => Ok(v != 0),
        None => ships_prs(app, &t.st("project")),
    }
}

/// A ships-PR setting from a body: yes/no (or true/false), or auto/none for the project's default.
pub fn clean_ships_pr(v: &Value) -> Result<Option<i64>> {
    match v {
        Value::Null => Ok(None),
        Value::Bool(b) => Ok(Some(*b as i64)),
        Value::Number(n) => Ok(Some((n.as_i64().unwrap_or(0) != 0) as i64)),
        Value::String(s) => match s.trim().to_lowercase().as_str() {
            "yes" | "on" | "true" | "1" | "pr" => Ok(Some(1)),
            "no" | "off" | "false" | "0" | "no-pr" => Ok(Some(0)),
            "" | "auto" | "none" | "default" => Ok(None),
            other => err(400, format!("“{other}” isn't a PR setting: give yes, no or auto.")),
        },
        _ => err(400, "Give the PR setting as yes, no or auto."),
    }
}

/// Sets a task's ships-PR flag from a body's `ships_pr`, logging the change. False when it didn't change.
pub fn set_task_ships_pr(app: &App, t: &Row, v: &Value, who: &str) -> Result<bool> {
    let want = clean_ships_pr(v)?;
    if want == t.i("ships_pr") {
        return Ok(false);
    }
    let ends_in_pr = match want {
        Some(v) => v != 0,
        None => ships_prs(app, &t.st("project"))?,
    };
    if !ends_in_pr {
        crate::stack::refuse_no_pr(app, t)?;
    }
    crate::board::update_task(app, t.id(), crate::fields!["ships_pr" => want])?;
    let text = match want {
        Some(1) => "Ends in a PR".to_string(),
        Some(_) => "Ends without a PR".to_string(),
        None => format!("Ends in a PR as its project does ({})", if ships_prs(app, &t.st("project"))? { "yes" } else { "no" }),
    };
    crate::board::log_event(app, t.id(), who, "note", &text)?;
    Ok(true)
}

pub fn describe(app: &App, p: &Value) -> Result<Value> {
    let name = p["name"].as_str().unwrap_or("");
    let mut d = p.clone();
    let o = d.as_object_mut().unwrap();
    o.insert("remote".into(), json!(has_remote(app, name)?));
    o.insert("pr_flow".into(), json!(pr_flow(app, name)?));
    o.insert("ships_prs".into(), json!(ships_prs(app, name)?));
    Ok(d)
}

pub fn project_for_path(app: &App, path: Option<&str>) -> Result<Option<String>> {
    let Some(path) = path.filter(|p| !p.is_empty()) else { return Ok(None) };
    let path = path.trim_end_matches('/');
    let mut best: Option<(String, usize)> = None;
    for p in list_projects(app)? {
        let pp = p["path"].as_str().unwrap_or("").trim_end_matches('/');
        if !pp.is_empty() && (path == pp || path.starts_with(&format!("{pp}/"))) {
            if best.as_ref().map(|(_, l)| pp.len() > *l).unwrap_or(true) {
                best = Some((p["name"].as_str().unwrap_or("").to_string(), pp.len()));
            }
        }
    }
    Ok(best.map(|(n, _)| n).or_else(|| base_name(path)))
}
