//! The lens review stage (`[pr.lens]`): when the board first reads a new PR, it runs lens's PR pass
//! (`lens review --pr <n>` from the PR's repo), which posts its verified findings on the PR as inline
//! comments. Runs go one at a time on their own thread (`App::queue_lens`): one takes
//! about 15 minutes.
//!
//! The run is kept in the PR's flow as `lens`: `{state: queued|running|done|failed, head, at,
//! started_at?, finished_at?, error?, log?}`. While it's queued or running the PR shows the `lens`
//! stage ("Lens review") in place of `review` or `ask`; every other stage (a failed check, comments,
//! ready to merge) still wins, so it never holds a merge. A run the board lost (it restarted midway)
//! stops holding once its timeout has passed. Each PR is reviewed once: a later push isn't
//! reviewed again, and a PR first read while lens was off isn't reviewed at all.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, prflow, proc};

/// Whether lens reviews this project's new PRs.
pub fn on(app: &App, project: Option<&str>) -> bool {
    crate::projects::pr_rules(app, project).lens.unwrap_or(app.cfg.pr.lens.on)
}

fn timeout_secs(app: &App) -> f64 {
    app.cfg.pr.lens.timeout_mins.max(1.0) * 60.0
}

/// The run holds the stage: queued or running, and not past its timeout (with a minute's grace).
pub fn holds(app: &App, f: &Row) -> bool {
    let Some(l) = f.get("lens").filter(|l| l.is_object()) else { return false };
    if !matches!(l["state"].as_str(), Some("queued" | "running")) {
        return false;
    }
    let since = l["started_at"].as_str().or(l["at"].as_str()).and_then(parse_iso).unwrap_or(0.0);
    now_ts() - since < timeout_secs(app) + 60.0
}

/// The Lens step of the PR bar: null when lens never ran on the PR; a run the board lost reads failed.
pub fn bar(app: &App, f: &Row) -> Value {
    let Some(l) = f.get("lens").filter(|l| l.is_object()) else { return Value::Null };
    let mut l = l.clone();
    if matches!(l["state"].as_str(), Some("queued" | "running")) && !holds(app, f) {
        l["state"] = json!("failed");
        l["error"] = json!("didn't finish");
    }
    l
}

/// Starts the review of a PR the board is reading for the first time, before that read is stepped.
/// Nothing when lens is off for its project, the PR isn't open or the board has read it before.
pub fn kick(app: &App, id: i64, rec: &Value) -> Result<()> {
    let queued = app.db.tx(|| {
        let t = board::get_task(app, id)?;
        let f = jloads_obj(t.s("pr_flow"));
        let open = rec["state"].as_str().unwrap_or("OPEN").eq_ignore_ascii_case("OPEN");
        if f.contains_key("rec") || f.contains_key("lens") || !open || !on(app, t.s("project")) {
            return Ok(false);
        }
        let head: String = rec["head"].as_str().unwrap_or("").chars().take(12).collect();
        prflow::merge_flow(app, id, fields!["lens" => json!({"state": "queued", "head": head, "at": now_iso()})])?;
        Ok(true)
    })?;
    if queued {
        app.queue_lens(Box::new(move |a| {
            if let Err(e) = run(a, id) {
                a.info(format!("lens: T{id}: {e}"));
            }
        }));
    }
    Ok(())
}

/// lens's PR pass on PR `num`.
pub fn args(num: i64) -> Vec<String> {
    vec!["review".into(), "--pr".into(), num.to_string()]
}

fn log_path(app: &App, id: i64, num: i64) -> PathBuf {
    app.cfg.data.join("lens").join(format!("T{id}-pr{num}.log"))
}

fn update(app: &App, id: i64, changes: Value) -> Result<()> {
    app.db.tx(|| {
        let f = jloads_obj(board::get_task(app, id)?.s("pr_flow"));
        let mut l = f.get("lens").cloned().unwrap_or(json!({}));
        for (k, v) in changes.as_object().cloned().unwrap_or_default() {
            l[k] = v;
        }
        prflow::merge_flow(app, id, fields!["lens" => l])?;
        Ok(())
    })
}

/// One queued run: lens's PR pass from the PR's repo, then the outcome on the board and the PR stepped
/// again, so it moves on from the `lens` stage.
pub fn run(app: &App, id: i64) -> Result<()> {
    let t = board::get_task(app, id)?;
    let num = t.i0("pr_num");
    update(app, id, json!({"state": "running", "started_at": now_iso()}))?;
    let outcome = review(app, &t, num);
    let finished = json!(now_iso());
    match &outcome {
        Ok(log) => {
            update(app, id, json!({"state": "done", "finished_at": finished, "log": log}))?;
            board::log_event(app, id, "lens", "status", &format!("PR #{num}: lens reviewed it and posted what it found"))?;
        }
        Err((why, log)) => {
            update(app, id, json!({"state": "failed", "finished_at": finished, "error": why, "log": log}))?;
            board::log_event(app, id, "lens", "status", &format!("PR #{num}: the lens review failed: {why}"))?;
        }
    }
    let t = board::get_task(app, id)?;
    if let Some(rec) = jloads_obj(t.s("pr_flow")).get("rec").filter(|r| r.is_object()).cloned() {
        app.db.tx(|| prflow::step(app, &t, &rec))?;
    }
    Ok(())
}

/// Runs the pass: the log's path, or why it failed (and the log, when there is one).
fn review(app: &App, t: &Row, num: i64) -> std::result::Result<Value, (String, Value)> {
    let fail = |why: &str| (why.to_string(), Value::Null);
    let lens = crate::lens::bin(app).ok_or_else(|| fail("lens isn't installed"))?;
    let cwd = prflow::pr_cwd(app, t).ok().flatten().ok_or_else(|| fail("the PR's project has no folder on this machine"))?;
    let out = proc::run(&lens, &args(num), Some(Path::new(&cwd)), timeout_secs(app))
        .map_err(|e| fail(&match e {
            proc::RunError::TimedOut => format!("it took longer than {} minutes", app.cfg.pr.lens.timeout_mins),
            proc::RunError::Spawn(e) => format!("lens didn't start: {e}"),
        }))?;
    let path = log_path(app, t.id(), num);
    let saved = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
        .and_then(|_| std::fs::write(&path, format!("{}\n--- stderr ---\n{}", out.stdout, out.stderr)))
        .is_ok();
    let log = if saved { json!(path.to_string_lossy()) } else { Value::Null };
    if out.code != Some(0) {
        let why = out.stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("lens failed");
        let why = why.trim().strip_prefix("Error: ").unwrap_or(why.trim());
        return Err((short(why, 200), log));
    }
    Ok(log)
}

