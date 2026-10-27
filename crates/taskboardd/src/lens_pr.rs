//! The lens review stage (`[pr.lens]`): when the board first reads a new PR, it runs lens's PR pass
//! (`lens review --pr <n>` from the PR's repo), which posts its verified findings on the PR as inline
//! comments. Runs go one at a time on their own thread (`App::queue_lens`): one takes
//! about 15 minutes.
//!
//! The run is kept in the PR's flow as `lens`: `{state: queued|running|done|failed|skipped|stopped,
//! head, at, tries, started_at?, finished_at?, error?, log?, reason?, by?}`. Until it's done the review
//! holds the merge: the PR shows the `lens` stage ("Lens review") ahead of review and merge (only a
//! failed check, a rebase or comments come first), and `tb pr merge` refuses. Its findings then hold
//! the merge as open threads until each is resolved (`Thread::lens_finding`). A run that fails, or one
//! the board lost (it restarted midway, so it's past its timeout), is tried again once; a second failure
//! asks the owner, who runs it again (`tb pr lens --retry`) or lets the PR go without it (`--skip`).
//! Each PR is reviewed once: a later push isn't reviewed again, and a PR first read while lens was off
//! isn't reviewed at all. A PR that merges or closes first needs no review: a queued run is `skipped`
//! and a running one killed and `stopped`.

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

/// Times a run is tried before the owner is asked.
const TRIES: i64 = 2;

/// The review holds the merge: it hasn't finished (done, or skipped or stopped), failed runs included.
pub fn holds(_app: &App, f: &Row) -> bool {
    let Some(l) = f.get("lens").filter(|l| l.is_object()) else { return false };
    !matches!(l["state"].as_str(), Some("done" | "skipped" | "stopped"))
}

/// Why the review holds the merge, for `tb pr merge`'s refusal: None when it doesn't.
pub fn blocker(app: &App, f: &Row) -> Option<String> {
    if !holds(app, f) {
        return None;
    }
    let l = &f["lens"];
    Some(match l["state"].as_str() {
        Some("failed") => format!(
            "the lens review failed ({}): {} runs it again with tb pr lens --retry, or lets it go with --skip \"<why>\"",
            l["error"].as_str().unwrap_or("it didn't finish"),
            app.cfg.owner
        ),
        _ => "lens is still reviewing it".into(),
    })
}

/// queued or running, and past its timeout (with a minute's grace): the board lost it (it restarted).
fn lost(app: &App, l: &Value) -> bool {
    if !matches!(l["state"].as_str(), Some("queued" | "running")) {
        return false;
    }
    let since = l["started_at"].as_str().or(l["at"].as_str()).and_then(parse_iso).unwrap_or(0.0);
    now_ts() - since >= timeout_secs(app) + 60.0
}

/// The Lens step of the PR bar: null when lens never ran on the PR; a run the board lost reads failed.
pub fn bar(app: &App, f: &Row) -> Value {
    let Some(l) = f.get("lens").filter(|l| l.is_object()) else { return Value::Null };
    let mut l = l.clone();
    if lost(app, &l) {
        l["state"] = json!("failed");
        l["error"] = json!("didn't finish");
    }
    l
}

fn queue(app: &App, id: i64) {
    app.queue_lens(Box::new(move |a| {
        if let Err(e) = run(a, id) {
            a.info(format!("lens: T{id}: {e}"));
        }
    }));
}

/// Starts the review of a PR the board is reading for the first time, before that read is stepped.
/// Nothing when lens is off for its project, the PR isn't open or the board has read it before. A run
/// the board lost is counted as failed: tried again, or the owner asked.
pub fn kick(app: &App, id: i64, rec: &Value) -> Result<()> {
    let open = rec["state"].as_str().unwrap_or("OPEN").eq_ignore_ascii_case("OPEN");
    let t = board::get_task(app, id)?;
    if let Some(l) = jloads_obj(t.s("pr_flow")).get("lens").filter(|l| open && lost(app, l)) {
        let tries = l["tries"].as_i64().unwrap_or(1);
        return failed(app, &t, tries, "it didn't finish (the board restarted)", Value::Null);
    }
    let queued = app.db.tx(|| {
        let t = board::get_task(app, id)?;
        let f = jloads_obj(t.s("pr_flow"));
        if f.contains_key("rec") || f.contains_key("lens") || !open || !on(app, t.s("project")) {
            return Ok(false);
        }
        let head: String = rec["head"].as_str().unwrap_or("").chars().take(12).collect();
        prflow::merge_flow(app, id, fields!["lens" => json!({"state": "queued", "head": head, "at": now_iso(), "tries": 1})])?;
        Ok(true)
    })?;
    if queued {
        queue(app, id);
    }
    Ok(())
}

/// A run failed on its `tries`th try: queued again while it has tries left, else failed, holding the
/// merge, with the owner asked.
fn failed(app: &App, t: &Row, tries: i64, why: &str, log: Value) -> Result<()> {
    let (id, num) = (t.id(), t.i0("pr_num"));
    if tries < TRIES {
        update(app, id, json!({"state": "queued", "at": now_iso(), "tries": tries + 1, "started_at": null, "error": why, "log": log}))?;
        board::log_event(app, id, "lens", "status", &format!("PR #{num}: the lens review failed ({why}), so it runs again"))?;
        queue(app, id);
        return Ok(());
    }
    update(app, id, json!({"state": "failed", "finished_at": now_iso(), "error": why, "log": log}))?;
    app.db.tx(|| {
        board::log_event(app, id, "lens", "status", &format!("PR #{num}: the lens review failed: {why}"))?;
        crate::dispatch::add_alert(
            app,
            &format!(
                "PR #{num} for {} waits on its lens review, which failed twice ({why}): tb pr lens {} --retry, or --skip \"<why>\"",
                rf("task", id),
                rf("task", id)
            ),
            Some(id),
            t.i("goal_id"),
            None,
            Some("pr"),
        )
    })?;
    Ok(())
}

/// `tb pr lens --retry`: runs a failed (or lost) review again. `--skip "<why>"`: the PR goes on without
/// it.
pub fn act(app: &App, id: i64, body: &Value) -> Result<Value> {
    let t = board::get_task(app, id)?;
    let f = jloads_obj(t.s("pr_flow"));
    let Some(l) = f.get("lens").filter(|l| l.is_object()) else {
        return err(409, format!("lens hasn't reviewed {}'s PR.", rf("task", id)));
    };
    let who = board::caller(app, body, &app.cfg.owner);
    let num = t.i0("pr_num");
    let skip = one_line(&body_str(body, "skip"), 500);
    if !skip.is_empty() {
        if as_bool(body.get("agent"), false) {
            return err(403, format!("Only {} lets a PR go without its lens review. Run tb pr wait and leave it to them.", app.cfg.owner));
        }
        if !holds(app, &f) {
            return err(409, format!("PR #{num}'s lens review isn't holding it."));
        }
        update(app, id, json!({"state": "skipped", "finished_at": now_iso(), "reason": skip, "by": who}))?;
        board::log_event(app, id, &who, "status", &format!("PR #{num}: goes on without its lens review: {skip}"))?;
    } else if as_bool(body.get("retry"), false) {
        if !(l["state"] == "failed" || lost(app, l)) {
            return err(409, format!("PR #{num}'s lens review hasn't failed (it's {}).", l["state"].as_str().unwrap_or("unknown")));
        }
        update(app, id, json!({"state": "queued", "at": now_iso(), "tries": 1, "started_at": null, "finished_at": null}))?;
        board::log_event(app, id, &who, "status", &format!("PR #{num}: the lens review runs again"))?;
        queue(app, id);
    } else {
        return err(400, "Say what to do: --retry, or --skip \"<why>\".");
    }
    step_last(app, id)?;
    let t = board::get_task(app, id)?;
    Ok(json!({"ok": true, "task": rf("task", id), "lens": jloads_obj(t.s("pr_flow")).get("lens").cloned(), "pr": board::pr_card(app, &t)}))
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
/// again (read fresh once lens has posted), so it moves on from the `lens` stage.
pub fn run(app: &App, id: i64) -> Result<()> {
    let t = board::get_task(app, id)?;
    let num = t.i0("pr_num");
    if let Some(why) = finished(&t) {
        update(app, id, json!({"state": "skipped", "finished_at": now_iso(), "reason": why}))?;
        board::log_event(app, id, "lens", "status", &format!("PR #{num}: no lens review, {why}"))?;
        return Ok(());
    }
    update(app, id, json!({"state": "running", "started_at": now_iso()}))?;
    let outcome = review(app, &t, num);
    let finished_at = json!(now_iso());
    match &outcome {
        Ok(None) => {
            let why = board::get_task(app, id).ok().as_ref().and_then(finished).unwrap_or("the PR finished");
            update(app, id, json!({"state": "stopped", "finished_at": finished_at, "reason": why}))?;
            board::log_event(app, id, "lens", "status", &format!("PR #{num}: stopped the lens review, {why}"))?;
            return Ok(());
        }
        Ok(Some(log)) => {
            update(app, id, json!({"state": "done", "finished_at": finished_at, "log": log}))?;
            board::log_event(app, id, "lens", "status", &format!("PR #{num}: lens reviewed it and posted what it found"))?;
            return reread(app, id, num);
        }
        Err((why, log)) => {
            let tries = jloads_obj(t.s("pr_flow")).get("lens").and_then(|l| l["tries"].as_i64()).unwrap_or(1);
            failed(app, &t, tries, why, log.clone())?;
        }
    }
    step_last(app, id)
}

/// Steps the PR with a fresh read once lens has posted: the record read before it ran has none of its
/// findings, so it would move a green PR on to its merge. When the host can't be read now, the next
/// late read (`prflow::late_reads`) steps it instead; until then lens's stage stays.
fn reread(app: &App, id: i64, num: i64) -> Result<()> {
    let t = board::get_task(app, id)?;
    if !prflow::flow(&t).contains_key("rec") {
        // The PR's first read, which started the review, steps it.
        return Ok(());
    }
    if !crate::prhost::watched(t.s("pr_host")) {
        return step_last(app, id);
    }
    if let Err(e) = prflow::refresh_task(app, id)? {
        app.info(format!("lens: reading PR #{num} after its review: {e}"));
        prflow::merge_flow(app, id, fields!["reread_at" => now_iso()])?;
    }
    Ok(())
}

/// Steps the PR again with its last read.
fn step_last(app: &App, id: i64) -> Result<()> {
    let t = board::get_task(app, id)?;
    if let Some(rec) = jloads_obj(t.s("pr_flow")).get("rec").filter(|r| r.is_object()).cloned() {
        app.db.tx(|| prflow::step(app, &t, &rec))?;
    }
    Ok(())
}

/// Why the PR needs no review any more: it merged or closed.
fn finished(t: &Row) -> Option<&'static str> {
    match t.s("pr_phase") {
        Some("merged") => Some("the PR merged"),
        Some("declined") => Some("the PR was closed"),
        _ => None,
    }
}

/// Runs the pass: the log's path, `None` when the PR merged or closed midway and the run was killed,
/// or why it failed (and the log, when there is one).
fn review(app: &App, t: &Row, num: i64) -> std::result::Result<Option<Value>, (String, Value)> {
    let fail = |why: &str| (why.to_string(), Value::Null);
    let lens = crate::lens::bin(app).ok_or_else(|| fail("lens isn't installed"))?;
    let cwd = prflow::pr_cwd(app, t).ok().flatten().ok_or_else(|| fail("the PR's project has no folder on this machine"))?;
    let id = t.id();
    let done = || board::get_task(app, id).ok().as_ref().and_then(finished).is_some();
    let out = proc::run_until(&lens, &args(num), Some(Path::new(&cwd)), timeout_secs(app), &[], None, &done)
        .map_err(|e| fail(&match e {
            proc::RunError::TimedOut => format!("it took longer than {} minutes", app.cfg.pr.lens.timeout_mins),
            proc::RunError::Spawn(e) => format!("lens didn't start: {e}"),
        }))?;
    let Some(out) = out else { return Ok(None) };
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
    Ok(Some(log))
}

