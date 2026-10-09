//! Projects: Midna's project list plus every project seen on tasks, goals and sessions.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::app::App;
use crate::p;
use crate::util::*;

const REMOTE_TTL: Duration = Duration::from_secs(600);
const PR_FLOW_SETTING: &str = "project_pr_flow";
/// `tb project set --approvals / --expected-check / --expected-wait / --ask-stage / --swap / --review / --agents-merge`: name → the rules set on the board.
const PR_RULES_SETTING: &str = "project_pr_rules";
/// The PR rules `tb project set` can change, over `[pr.projects.<name>]`.
pub const PR_RULE_KEYS: &[&str] = &["approvals", "expected", "expected_wait_mins", "ask_stage", "swap", "review", "agents_merge"];
pub const PR_FLOWS: &[&str] = &["auto", "on", "off"];
/// `tb project agents-merge on|off`: the board's own word on `pr.agents_merge`, over config.toml's
/// (`taskboardd import` turns it on: the old board's agents merged on every project).
const AGENTS_MERGE_SETTING: &str = "pr_agents_merge";

/// Whether agents merge PRs on a project that doesn't say: the board's word (`tb project agents-merge`),
/// else config.toml's `pr.agents_merge`.
pub fn agents_merge_default(app: &App) -> bool {
    agents_merge_set(app).unwrap_or(app.cfg.pr.agents_merge)
}

/// What `tb project agents-merge` (or an import) set on the board, if anything.
pub fn agents_merge_set(app: &App) -> Option<bool> {
    match app.db.get_setting(AGENTS_MERGE_SETTING).ok().flatten()?.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Sets the board's word on `pr.agents_merge`; None goes back to config.toml's.
pub fn set_agents_merge(app: &App, on: Option<bool>) -> Result<()> {
    app.db.set_setting(AGENTS_MERGE_SETTING, on.map(|b| if b { "true" } else { "false" }))
}

/// `GET /projects/agents-merge`: the board-wide switch, what the board set, and config.toml's.
pub fn describe_agents_merge(app: &App) -> Value {
    json!({"agents_merge": agents_merge_default(app), "set": agents_merge_set(app), "config": app.cfg.pr.agents_merge})
}

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

fn pr_rule_overrides(app: &App) -> Result<Row> {
    Ok(jloads_obj(app.db.get_setting(PR_RULES_SETTING)?.as_deref()))
}

/// The rules `tb project set` put on a project (only the keys it set).
pub fn pr_rules_set(app: &App, name: &str) -> Result<Row> {
    Ok(pr_rule_overrides(app)?.get(name).and_then(|v| v.as_object()).cloned().unwrap_or_default())
}

/// A project's PR rules: `[pr.projects.<name>]` with what `tb project set` changed on top.
pub fn pr_rules(app: &App, name: Option<&str>) -> crate::config::PrProject {
    let mut r = app.cfg.pr.project(name);
    let Some(name) = name.filter(|n| !n.is_empty()) else { return r };
    let set = pr_rules_set(app, name).unwrap_or_default();
    if let Some(n) = set.get("approvals").and_then(|v| v.as_i64()) {
        r.approvals = Some(n);
    }
    if let Some(list) = set.get("expected").and_then(|v| v.as_array()) {
        r.expected = Some(list.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect());
    }
    if let Some(m) = set.get("expected_wait_mins").and_then(|v| v.as_f64()) {
        r.expected_wait_mins = Some(m);
    }
    if let Some(b) = set.get("swap").and_then(|v| v.as_bool()) {
        r.swap = Some(b);
    }
    if let Some(b) = set.get("ask_stage").and_then(|v| v.as_bool()) {
        r.ask_stage = Some(b);
    }
    if let Some(b) = set.get("review").and_then(|v| v.as_bool()) {
        r.review = Some(b);
    }
    if let Some(b) = set.get("agents_merge").and_then(|v| v.as_bool()) {
        r.agents_merge = Some(b);
    }
    r
}

/// Changes a project's PR rules from a body: each of [`PR_RULE_KEYS`] given is set, or with null goes
/// back to config.toml's. True when the body named any.
pub fn set_pr_rules(app: &App, name: &str, body: &Value) -> Result<bool> {
    let mut set = pr_rules_set(app, name)?;
    let mut any = false;
    for key in PR_RULE_KEYS {
        let Some(v) = body.get(*key) else { continue };
        any = true;
        if v.is_null() {
            set.remove(*key);
            continue;
        }
        let clean = match *key {
            "approvals" => match v.as_i64() {
                Some(n) if (0..=20).contains(&n) => json!(n),
                _ => return err(400, "approvals is a count from 0 (the host's own decision) to 20."),
            },
            "expected" => {
                let Some(list) = v.as_array() else { return err(400, "expected is a list of check names ([] for none).") };
                let mut names: Vec<String> = vec![];
                for x in list {
                    let n = x.as_str().map(|s| one_line(s, 200)).unwrap_or_default();
                    if n.is_empty() {
                        return err(400, "An expected check needs a name.");
                    }
                    if !names.iter().any(|m| m.eq_ignore_ascii_case(&n)) {
                        names.push(n);
                    }
                }
                json!(names)
            }
            "ask_stage" | "swap" | "review" | "agents_merge" => match v.as_bool().or_else(|| match v.as_str() {
                Some("on") => Some(true),
                Some("off") => Some(false),
                _ => None,
            }) {
                Some(b) => json!(b),
                None => return err(400, format!("{key} is on or off (true or false).")),
            },
            _ => match v.as_f64() {
                Some(m) if m > 0.0 && m <= 24.0 * 60.0 => json!(m),
                _ => return err(400, "expected_wait_mins is minutes, more than 0 and at most a day (1440)."),
            },
        };
        set.insert(key.to_string(), clean);
    }
    if !any {
        return Ok(false);
    }
    let mut all = pr_rule_overrides(app)?;
    if set.is_empty() {
        all.remove(name);
    } else {
        all.insert(name.into(), Value::Object(set));
    }
    app.db.set_setting(PR_RULES_SETTING, Some(&jdumps(&Value::Object(all))))?;
    Ok(true)
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
    let r = pr_rules(app, Some(name));
    let approvals = r.approvals.unwrap_or(app.cfg.pr.approvals);
    o.insert(
        "pr_rules".into(),
        json!({"approvals": approvals, "expected": r.expected,
               "expected_wait_mins": r.expected_wait_mins.unwrap_or(crate::config::EXPECTED_WAIT_MINS),
               "ask_stage": crate::reviewers::ask_stage_on(app, Some(name)),
               "swap": crate::reviewers::swap_on(app, Some(name)),
               "review": crate::reviewers::review_on(app, Some(name)),
               "agents_merge": crate::prflow::agents_merge_on(app, Some(name)),
               "set": pr_rules_set(app, name)?}),
    );
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
