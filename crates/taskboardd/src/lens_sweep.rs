//! The lens sweep: review moves out of task sessions. Every `[lens] sweep_mins` the board runs
//! `lens sweep --repo <dir>` headless for each project lens is on for (`lens::on`) that has a folder
//! and a remote. lens decides whether a sweep is due (its `[sweep] every` and `max_lines`, the volume
//! trigger) and otherwise says why it waits; a due sweep reviews what was pushed since the last one,
//! fixes blockers on one branch and opens one PR with the digest.
//!
//! The board keeps each project's latest run in the `lens_sweeps` setting, `{<project>: {state:
//! running|done|failed, at, finished_at?, said, report?, digest?, pr?, error?, filed}}`, which the
//! project shows as `lens_sweep`. A new sweep PR raises an alert with the PR and the digest. A blocker
//! lens's fixer escalated becomes a new task on the project, once (`filed` keeps the keys), never a
//! session brought back.

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, dispatch, ops, proc, projects};

const SETTING: &str = "lens_sweeps";
/// The most escalation keys a project keeps (oldest dropped first).
const FILED_MAX: usize = 500;

pub fn run(app: Arc<App>) {
    while !app.stopping() {
        let mins = app.cfg.lens.sweep_mins;
        if mins > 0.0 {
            if let Err(e) = tick(&app) {
                app.info(format!("lens sweep: {}", e.message));
            }
        }
        app.sleep(if mins > 0.0 { mins * 60.0 } else { 600.0 });
    }
}

/// One round: asks lens to sweep each project it's on for, one at a time.
pub fn tick(app: &App) -> Result<()> {
    let Some(bin) = crate::lens::bin(app) else { return Ok(()) };
    for p in projects::list_projects(app)? {
        let (Some(name), Some(path)) = (p["name"].as_str(), p["path"].as_str()) else { continue };
        if app.stopping() {
            break;
        }
        if !crate::lens::on(app, Some(name)) || !Path::new(path).is_dir() || projects::has_remote(app, name)? != Some(true) {
            continue;
        }
        sweep(app, &bin, name, path)?;
    }
    Ok(())
}

/// What the board knows of each project's latest sweep.
pub fn all(app: &App) -> Result<Row> {
    Ok(jloads_obj(app.db.get_setting(SETTING)?.as_deref()))
}

/// A project's latest sweep, or null before the first.
pub fn of(app: &App, project: &str) -> Result<Value> {
    Ok(all(app)?.get(project).cloned().unwrap_or(Value::Null))
}

fn save(app: &App, project: &str, rec: &Value) -> Result<()> {
    app.db.tx(|| {
        let mut all = all(app)?;
        all.insert(project.into(), rec.clone());
        app.db.set_setting(SETTING, Some(&jdumps(&Value::Object(all))))
    })
}

fn sweep(app: &App, bin: &Path, project: &str, path: &str) -> Result<()> {
    let before = of(app, project)?;
    let filed = before.get("filed").cloned().unwrap_or(json!([]));
    let mut rec = json!({"state": "running", "at": now_iso(), "filed": filed});
    // What the last run found stays on show while lens decides whether this one is due.
    for k in ["report", "digest", "pr", "said"] {
        if let Some(v) = before.get(k).filter(|v| !v.is_null()) {
            rec[k] = v.clone();
        }
    }
    save(app, project, &rec)?;
    let args: Vec<String> = vec!["sweep".into(), "--repo".into(), path.into()];
    let timeout = app.cfg.lens.sweep_timeout_mins.max(1.0) * 60.0;
    let out = proc::run(bin, &args, Some(Path::new(path)), timeout);
    rec["finished_at"] = json!(now_iso());
    let out = match out {
        Ok(o) => o,
        Err(e) => {
            let why = match e {
                proc::RunError::TimedOut => format!("it took longer than {} minutes", app.cfg.lens.sweep_timeout_mins),
                proc::RunError::Spawn(e) => format!("lens didn't start: {e}"),
            };
            rec["state"] = json!("failed");
            rec["error"] = json!(why);
            return save(app, project, &rec);
        }
    };
    if out.code != Some(0) {
        let why = out.stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("lens failed").trim();
        rec["state"] = json!("failed");
        rec["error"] = json!(short(why.strip_prefix("Error: ").unwrap_or(why), 300));
        return save(app, project, &rec);
    }
    rec["state"] = json!("done");
    outcome(app, project, Path::new(path), &out.stdout, &mut rec)?;
    save(app, project, &rec)
}

/// The value after `<label>: ` on a line of lens's summary.
fn line_of<'a>(stdout: &'a str, label: &str) -> Option<&'a str> {
    stdout.lines().find_map(|l| l.trim().strip_prefix(label)).map(str::trim).filter(|v| !v.is_empty())
}

/// Takes in what a finished `lens sweep` printed: a sweep that ran leaves `Report:` and `Digest:`
/// lines (and `PR:` when it opened one); one that wasn't due or had nothing new says why in a line.
/// Paths lens gives relative are in `repo`.
pub fn outcome(app: &App, project: &str, repo: &Path, stdout: &str, rec: &mut Value) -> Result<()> {
    let within = |p: &str| repo.join(p).to_string_lossy().to_string();
    let said = stdout.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    rec["said"] = json!(short(said, 300));
    let Some(report_path) = line_of(stdout, "Report:") else { return Ok(()) };
    let report_path = within(report_path);
    rec["report"] = json!(report_path);
    rec["digest"] = line_of(stdout, "Digest:").map_or(Value::Null, |d| json!(within(d)));
    let report = std::fs::read_to_string(&report_path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null);
    let pr = report["pr"].as_str().or_else(|| line_of(stdout, "PR:")).filter(|u| u.starts_with("http")).map(String::from);
    rec["pr"] = pr.as_ref().map_or(Value::Null, |u| json!(u));
    if let Some(url) = &pr {
        let digest = rec["digest"].as_str().map(|d| format!(" Digest: {d}")).unwrap_or_default();
        dispatch::add_alert_keyed(app, &format!("lens sweep of {project} opened {url} with its fixes.{digest}"), None, None, &format!("lens-sweep:{url}"))?;
    }
    let mut filed: Vec<String> = rec["filed"].as_array().map(|a| a.iter().filter_map(|k| k.as_str().map(String::from)).collect()).unwrap_or_default();
    for f in report["findings"].as_array().into_iter().flatten().filter(|f| f["fix"]["status"] == "escalated") {
        let key = format!("{}:{}", f["file"].as_str().unwrap_or(""), f["title"].as_str().unwrap_or(""));
        if filed.contains(&key) {
            continue;
        }
        let mut f = f.clone();
        if let Some(l) = f["fix"]["ledger"].as_str() {
            f["fix"]["ledger"] = json!(within(l));
        }
        match file_task(app, project, &f, rec) {
            Ok(id) => app.info(format!("lens sweep: {project}: filed {} for escalated {}", rf("task", id), f["id"].as_str().unwrap_or("?"))),
            Err(e) => app.info(format!("lens sweep: {project}: couldn't file escalated {}: {}", f["id"].as_str().unwrap_or("?"), e.message)),
        }
        filed.push(key);
    }
    let skip = filed.len().saturating_sub(FILED_MAX);
    rec["filed"] = json!(filed[skip..]);
    Ok(())
}

/// A new task for a blocker lens's fixer couldn't fix: what it found, why the fixer stopped, and where
/// its attempts are.
fn file_task(app: &App, project: &str, f: &Value, rec: &Value) -> Result<i64> {
    let ledger = crate::lens::read_ledger(f);
    let e = &ledger["escalation"];
    let at = format!("{}:{}", f["file"].as_str().unwrap_or("?"), f["line"]);
    let mut detail = vec![format!(
        "lens's sweep found a blocker at {at}: {}. {}",
        f["title"].as_str().unwrap_or(""),
        f["failureScenario"].as_str().unwrap_or("")
    )];
    let reason = e["reason"].as_str().filter(|r| !r.is_empty()).unwrap_or("no reason given");
    let needs = e["needs"].as_str().map(|n| format!(" It needs: {n}.")).unwrap_or_default();
    detail.push(format!("Its fixer couldn't fix it and escalated: {reason}.{needs}"));
    if let Some(l) = f["fix"]["ledger"].as_str() {
        detail.push(format!("The fix attempts are in {l} (with their patches beside it)."));
    }
    for (label, k) in [("Digest", "digest"), ("Sweep PR", "pr")] {
        if let Some(v) = rec[k].as_str() {
            detail.push(format!("{label}: {v}"));
        }
    }
    detail.push("Fix it in one small PR. Leave the sweep's other findings alone.".into());
    let title = one_line(&format!("Fix lens finding at {at}: {}", f["title"].as_str().unwrap_or("")), 200);
    let body = json!({
        "title": title,
        "detail": detail.join("\n\n"),
        "project": project,
        "origin": {"from": "lens sweep", "by": board::BOARD, "url": rec["pr"]},
    });
    let made = ops::new_task(app, &body, board::BOARD, Some("Added from a lens sweep's escalated blocker"))?;
    Ok(made["id"].as_i64().unwrap_or(0))
}
