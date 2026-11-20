//! Expected checks read from Azure DevOps as well as the PR host.
//!
//! A PR host (Bitbucket) shows an Azure Pipelines build only once it starts, not while it waits in
//! Azure's queue, which can take an hour. Waiting only on the host, a push whose build is still
//! queued looks like one whose check never comes: after `expected_wait_mins` it stops holding the
//! merge, and an approved PR could merge before its build starts.
//!
//! A project that names its Azure DevOps project and pipelines (`[pr.projects.<name>] azure_project`
//! and `azure_pipelines`) has the board ask Azure's Build API, for each of those checks the host
//! hasn't shown on the PR's head, for that pipeline's build of the head ([`note`]): once a minute
//! ([`ASK_EVERY`]) while one is still to come, never again once it finished. [`fold`] adds what Azure
//! said to the PR's record as a check of that name (`via: "azure"`), so the stage, `tb pr merge` and
//! `tb pr status` treat a queued or running build as a running check (it holds the merge until it
//! finishes, past `expected_wait_mins`) and a finished one as passed, failed or stopped. Once the host
//! shows the check itself, the host's is the one that counts. With no build in Azure for the head, the
//! expected wait runs out as before; a stacked PR (into a branch other than the repo's default) may
//! wait less, `azure_stacked_wait_mins` ([`stacked_wait`]).
//!
//! The calls go through the Azure DevOps account's rate-limit gate (`prhost::limit`), as the CI reads
//! do, so `tb pr limits` counts them; the reader is named "Azure expected checks". The token is the
//! board's CI token (`accounts::ci_token`).
//!
//! # State
//!
//! `pr_flow.azure_runs` is `{<head>: {at, stacked, error, runs: {<check>: {id, state, status, url,
//! at} | null}}}`, for the PR's current head only; `null` means Azure had no build for it when asked.

use std::sync::Arc;

use serde_json::{json, Map, Value};

use crate::app::App;
use crate::prhost::bitbucket::{BasicHttp, Http};
use crate::prhost::ci;
use crate::prflow::{flow, merge_flow};
use crate::util::*;

/// Seconds between asks for one head while a check is still to come.
pub const ASK_EVERY: f64 = 60.0;

/// The Azure DevOps reader used in place of the real one (tests).
pub fn install(app: &App, http: Arc<dyn Http>) {
    app.shared.lock().azure_http = Some(http);
}

fn http_for(app: &App) -> Option<Arc<dyn Http>> {
    if let Some(h) = app.shared.lock().azure_http.clone() {
        return Some(h);
    }
    let (_, token) = crate::accounts::ci_token(app)?;
    Some(Arc::new(BasicHttp::for_service("Azure DevOps", "", &token)))
}

/// The project's Azure API base and its pipelines by check name, when it names both.
fn setup(app: &App, t: &Row) -> Option<(String, Vec<(String, i64)>)> {
    let rules = crate::projects::pr_rules(app, t.s("project"));
    let base = ci::azure_project_base(rules.azure_project.as_deref()?)?;
    let pipes: Vec<(String, i64)> = rules.azure_pipelines?.into_iter().filter(|(n, _)| !n.trim().is_empty()).collect();
    (!pipes.is_empty()).then_some((base, pipes))
}

fn on_host(rec: &Value, name: &str) -> bool {
    rec["checks"].as_array().into_iter().flatten().any(|c| c["via"] != "azure" && c["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

fn entry<'a>(f: &'a Row, rec: &Value) -> Option<&'a Value> {
    f.get("azure_runs")?.get(rec["head"].as_str().filter(|h| !h.is_empty())?)
}

fn finished(run: &Value) -> bool {
    run.is_object() && run["state"] != "running"
}

/// Asks Azure for each of the project's pipelines the host hasn't shown on this head, at most every
/// [`ASK_EVERY`] seconds, and keeps the answers in the flow. Outside any transaction: it goes to the
/// network.
pub fn note(app: &App, t: &Row, rec: &Value) -> Result<()> {
    let Some((base, pipes)) = setup(app, t) else { return Ok(()) };
    let head = rec["head"].as_str().unwrap_or("");
    if head.is_empty() {
        return Ok(());
    }
    let f = flow(t);
    let prior = entry(&f, rec).cloned().unwrap_or_else(|| json!({}));
    let wanted: Vec<&(String, i64)> = pipes.iter().filter(|(n, _)| !on_host(rec, n) && !finished(&prior["runs"][n.as_str()])).collect();
    if wanted.is_empty() {
        return Ok(());
    }
    if prior["at"].as_str().and_then(parse_iso).is_some_and(|at| now_ts() - at < ASK_EVERY) {
        return Ok(());
    }
    let Some(http) = http_for(app) else { return Ok(()) };
    let branch = rec["branch"].as_str().unwrap_or("");
    let num = t.i0("pr_num");
    let mut runs = prior["runs"].as_object().cloned().unwrap_or_default();
    let mut error = None;
    crate::prhost::limit::reader("Azure expected checks", || {
        for (name, def) in wanted {
            match ci::azure_run(http.as_ref(), &base, *def, branch, num, head) {
                Ok(run) => {
                    runs.insert(name.clone(), run.map_or(Value::Null, |r| r.json()));
                }
                Err(e) => {
                    app.info(format!("prs: couldn't ask Azure DevOps about {name} for {}: {e}", t.st("pr_url")));
                    error = Some(e);
                }
            }
        }
    });
    let stacked = stacked(app, t, rec);
    let mut all = Map::new();
    all.insert(head.to_string(), json!({"at": now_iso(), "runs": runs, "error": error, "stacked": stacked}));
    merge_flow(app, t.id(), vec![("azure_runs", Value::Object(all))]).map(|_| ())
}

/// The PR goes into a branch other than the repo's default (asked only when the project sets
/// `azure_stacked_wait_mins`).
fn stacked(app: &App, t: &Row, rec: &Value) -> bool {
    if crate::projects::pr_rules(app, t.s("project")).azure_stacked_wait_mins.is_none() {
        return false;
    }
    let base = rec["base"].as_str().unwrap_or("");
    let default = crate::worktrees::repo_of(app, t).ok().flatten().and_then(|r| crate::steps::default_base(&r));
    matches!(default, Some(d) if !base.is_empty() && d != base)
}

/// The record with what Azure said about the checks the host hasn't shown on its head: each as a
/// check of its name (`via: "azure"`), with `running` and `failed` counted again. An earlier fold's
/// checks are dropped first, so a record kept in the flow can be folded again.
pub fn fold(t: &Row, rec: &Value) -> Value {
    let mut out = rec.clone();
    let Some(list) = out["checks"].as_array_mut() else { return out };
    list.retain(|c| c["via"] != "azure");
    let f = flow(t);
    if let Some(runs) = entry(&f, rec).and_then(|e| e["runs"].as_object()) {
        for (name, run) in runs.iter().filter(|(_, r)| r.is_object()) {
            let shown = list.iter().any(|c| c["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)));
            if !shown {
                list.push(json!({"name": name, "state": run["state"], "url": run["url"], "at": run["at"], "via": "azure", "azure_status": run["status"]}));
            }
        }
    }
    let count = |s: &str| list.iter().filter(|c| c["state"] == s).count();
    let running = count("running");
    let failed: Vec<Value> = list.iter().filter(|c| c["state"] == "failed").map(|c| c["name"].clone()).collect();
    out["running"] = json!(running);
    out["failed"] = Value::Array(failed);
    out
}

/// How long a stacked PR waits for `missing` expected checks: `azure_stacked_wait_mins`, when the PR
/// goes into a branch other than the repo's default and Azure, asked without an error, has no build
/// for any of them on this head.
pub fn stacked_wait(app: &App, t: &Row, rec: &Value, missing: &[String]) -> Option<f64> {
    let mins = crate::projects::pr_rules(app, t.s("project")).azure_stacked_wait_mins?;
    let (_, pipes) = setup(app, t)?;
    let f = flow(t);
    let e = entry(&f, rec)?;
    if e["stacked"] != true || !e["error"].is_null() || missing.is_empty() {
        return None;
    }
    let runs = e["runs"].as_object()?;
    let no_build = |m: &String| pipes.iter().any(|(n, _)| n.eq_ignore_ascii_case(m)) && runs.iter().any(|(n, r)| n.eq_ignore_ascii_case(m) && r.is_null());
    missing.iter().all(no_build).then_some(mins)
}
