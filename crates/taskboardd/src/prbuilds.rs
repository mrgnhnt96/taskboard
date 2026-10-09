//! PR builds stopped: a board-wide switch, set on the owner's word (`tb pr-builds stop|resume`,
//! recording who), for when CI time is scarce. While it's on:
//!
//! - every build on one of the owner's PRs (a PR a board task made) or the owner's own pushes (an
//!   event's `author` in `owner_emails`, or its branch one of those PRs') is cancelled as soon as the
//!   board hears of it: a build event on the feed (`POST /prs/event`, `kind: build`), or a running
//!   check on a poll;
//! - a cancel runs the CI's command from `[pr_builds.cancel]` (by provider: `github`, `bitbucket`,
//!   `azure`, or any name an event gives), else the PR host's own (`gh run cancel`, Pipelines'
//!   `stopPipeline`); a failed one is tried again after each of `retry_secs` (0, 10, 30, 60 and 120
//!   seconds by default), then raises an alert;
//! - after a cancel the same push is swept again after each of `follow_up_secs` (10, 30, 60 and 120
//!   seconds by default), for builds queued just after it;
//! - the PRs' checks count as passed ("Builds stopped"), so their PRs move on to review.
//!
//! The status bar shows a "PR builds stopped" pill. `stop` and `resume` take down the give-up alerts;
//! `resume` drops the cancels still waiting.
//!
//! Build events also fire the owner's `pr.checks` (a build started) and `pr.fix` (a build failed)
//! hooks for PRs on hosts the board doesn't watch (GitLab, …), once per push; a skip counts that
//! push's checks as passed, as it does for watched PRs.
//!
//! # State
//!
//! The setting `pr_builds` is `{stopped, by, at, reason}` (and `resumed_by`, `resumed_at`); the
//! setting `pr_build_cancels` is the queue: `[{key, task_id, repo, host, num, url, head, branch,
//! provider, build_url, build_id, tries, next_at, error, round, first_at}]` (`round`: 0 for the
//! first cancel, then each follow-up); `pr_build_recent` the last cancels, newest first. A task's `pr_flow.builds_cancelled` maps
//! a head to when its builds were cancelled, and `pr_flow.hooked` the build hooks already asked.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::prhost::{self, Cancelled, PrRef};
use crate::util::*;
use crate::{board, dispatch, hooks, p, prflow};

/// `[pr_builds]` in config.toml.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PrBuildsConfig {
    /// Seconds before each try to cancel a build: the first, then each retry. After the last, an alert.
    pub retry_secs: Vec<f64>,
    /// The command that cancels builds, by CI provider. Run with `/bin/sh -c`, with TB_PROVIDER,
    /// TB_PR_URL, TB_PR_REPO, TB_PR_NUM, TB_HEAD, TB_BRANCH, TB_BUILD_URL and TB_BUILD_ID set; exit 0
    /// when it cancelled them. Unset for github or bitbucket: the PR host's own.
    pub cancel: BTreeMap<String, String>,
    /// How long a cancel command may run.
    pub timeout_secs: f64,
    /// Seconds after a push's first cancel at which it's swept again, for builds queued after it.
    pub follow_up_secs: Vec<f64>,
}

impl Default for PrBuildsConfig {
    fn default() -> Self {
        PrBuildsConfig { retry_secs: vec![0.0, 10.0, 30.0, 60.0, 120.0], cancel: BTreeMap::new(), timeout_secs: 30.0, follow_up_secs: vec![10.0, 30.0, 60.0, 120.0] }
    }
}

const KEY: &str = "pr_builds";
const QUEUE: &str = "pr_build_cancels";
const RECENT: &str = "pr_build_recent";
/// How many cancels `tb pr-builds` lists.
const RECENT_MAX: usize = 10;

fn setting(app: &App, key: &str) -> Value {
    app.db.get_setting(key).ok().flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)
}

fn queue(app: &App) -> Vec<Value> {
    setting(app, QUEUE).as_array().cloned().unwrap_or_default()
}

fn save_queue(app: &App, q: &[Value]) -> Result<()> {
    app.db.set_setting(QUEUE, if q.is_empty() { None } else { Some(jdumps(&Value::Array(q.to_vec()))) }.as_deref())
}

/// The board's PR builds are stopped (`tb pr-builds stop`).
pub fn stopped(app: &App) -> bool {
    setting(app, KEY)["stopped"] == true
}

/// `GET /pr-builds` (`tb pr-builds`), and `state.pr_builds`.
pub fn status(app: &App) -> Value {
    let s = setting(app, KEY);
    let q = queue(app);
    let cancelling = q.iter().filter(|x| x["round"].as_i64().unwrap_or(0) == 0).count();
    json!({"stopped": s["stopped"] == true, "by": s.get("by"), "at": s.get("at"), "reason": s.get("reason"),
           "resumed_by": s.get("resumed_by"), "resumed_at": s.get("resumed_at"), "cancelling": cancelling,
           "recent": setting(app, RECENT).as_array().cloned().unwrap_or_default()})
}

/// `POST /pr-builds {stopped, who, reason?}`: the owner's word.
pub fn set(app: &App, body: &Value) -> Result<Value> {
    if body.get("stopped").and_then(|v| v.as_bool()).is_none() {
        return err(400, "Say whether PR builds are stopped: {\"stopped\": true} or false.");
    }
    let stop = as_bool(body.get("stopped"), false);
    let who = { let w = one_line(&body_str(body, "who"), 80); if w.is_empty() { app.cfg.owner.clone() } else { w } };
    let reason = one_line(&body_str(body, "reason"), 200);
    let was = setting(app, KEY);
    if (was["stopped"] == true) == stop {
        return Ok(status(app));
    }
    let now = now_iso();
    let v = if stop {
        json!({"stopped": true, "by": who, "at": now, "reason": if reason.is_empty() { Value::Null } else { json!(reason) }})
    } else {
        json!({"stopped": false, "by": was.get("by"), "at": was.get("at"), "reason": was.get("reason"), "resumed_by": who, "resumed_at": now})
    };
    app.db.set_setting(KEY, Some(&jdumps(&v)))?;
    dispatch::clear_alert_prefix(app, "pr-builds:")?;
    if stop {
        app.info(format!("pr-builds: {who} stopped PR builds{}", if reason.is_empty() { String::new() } else { format!(": {reason}") }));
        sweep(app)?;
    } else {
        app.info(format!("pr-builds: {who} resumed PR builds"));
        save_queue(app, &[])?;
    }
    Ok(status(app))
}

/// A build's CI from its link: github (Actions), bitbucket (Pipelines), azure, or none.
pub fn provider_of(url: &str) -> Option<&'static str> {
    if prhost::github::run_id(url).is_some() {
        Some("github")
    } else if prhost::bitbucket::pipeline_build(url).is_some() {
        Some("bitbucket")
    } else if prhost::ci::azure_build(url).is_some() {
        Some("azure")
    } else {
        None
    }
}

fn is_running(state: &str) -> bool {
    matches!(state, "started" | "running" | "queued" | "pending" | "in_progress" | "inprogress" | "created")
}

fn flow(t: &Row) -> Row {
    jloads_obj(t.s("pr_flow"))
}

/// Queues a cancel unless one for the same key is waiting.
fn enqueue(app: &App, item: Value) -> Result<bool> {
    let mut q = queue(app);
    if q.iter().any(|x| x["key"] == item["key"]) {
        return Ok(false);
    }
    q.push(item);
    save_queue(app, &q)?;
    app.wake_runner();
    Ok(true)
}

fn pr_item(app: &App, t: &Row, head: &str, provider: &str, body: &Value) -> Value {
    json!({"key": format!("T{}:{head}", t.id()), "task_id": t.id(), "repo": t.v("pr_repo"), "host": t.v("pr_host"), "num": t.v("pr_num"),
           "url": t.v("pr_url"), "head": head, "branch": body.get("branch").cloned().unwrap_or(Value::Null), "provider": provider,
           "build_url": body.get("build_url").cloned().unwrap_or(Value::Null), "build_id": body.get("build_id").cloned().unwrap_or(Value::Null),
           "tries": 0, "round": 0, "next_at": iso(now_ts() + app.cfg.pr_builds.retry_secs.first().copied().unwrap_or(0.0))})
}

/// A build event on the feed for one of the board's PRs: cancels it while PR builds are stopped, and
/// asks the owner's build hooks for a PR on a host the board doesn't watch.
pub fn on_build_event(app: &App, t: &Row, body: &Value) -> Result<Value> {
    let state = body_str(body, "state").to_lowercase();
    let head = { let h = body_str(body, "head"); if h.is_empty() { flow(t).get("rec").and_then(|r| r["head"].as_str()).unwrap_or("").to_string() } else { h } };
    let mut out = json!({"state": state});
    if is_running(&state) && stopped(app) {
        let provider = provider_for(t, body);
        out["cancel"] = json!(if enqueue(app, pr_item(app, t, &head, &provider, body))? { "queued" } else { "waiting" });
    }
    if !prhost::watched(t.s("pr_host")) {
        if let Some(d) = build_hooks(app, t, &state, &head)? {
            out["hook"] = json!(d);
        }
    }
    Ok(out)
}

fn provider_for(t: &Row, body: &Value) -> String {
    let p = body_str(body, "provider").to_lowercase();
    if !p.is_empty() {
        return p;
    }
    if let Some(p) = provider_of(&body_str(body, "build_url")) {
        return p.into();
    }
    t.st("pr_host")
}

/// The board's open PR in `repo` (any repo, when it's not given) from `branch`: one of the owner's PRs.
fn owner_pr_on(app: &App, repo: &str, branch: &str) -> Result<Option<Row>> {
    if branch.is_empty() {
        return Ok(None);
    }
    let tasks = app.db.q(
        "SELECT * FROM tasks WHERE pr_num IS NOT NULL AND pr_repo IS NOT NULL AND (pr_phase IS NULL OR pr_phase NOT IN ('merged', 'declined'))",
        p![],
    )?;
    Ok(tasks.into_iter().find(|t| {
        (repo.is_empty() || t.s("pr_repo").is_some_and(|r| r.eq_ignore_ascii_case(repo)))
            && flow(t).get("rec").and_then(|r| r["branch"].as_str()) == Some(branch)
    }))
}

/// A build event for no PR on the board: one of the owner's pushes (by `author`, or on the branch of
/// one of the owner's open PRs, whoever wrote the commit) is cancelled too.
pub fn on_push_build(app: &App, body: &Value) -> Result<Value> {
    let state = body_str(body, "state").to_lowercase();
    let author = body_str(body, "author").to_lowercase();
    let head = body_str(body, "head");
    let url = body_str(body, "url");
    let (host, repo) = match find_pr(&url) {
        Some(l) => (l.host, l.repo),
        None => (body_str(body, "host"), body_str(body, "repo")),
    };
    let by_owner = !author.is_empty() && app.cfg.owner_emails.iter().any(|e| e.trim().eq_ignore_ascii_case(&author));
    let pr = if by_owner { None } else { owner_pr_on(app, &repo, &body_str(body, "branch"))? };
    let mine = by_owner || pr.is_some();
    if !is_running(&state) || !stopped(app) || !mine {
        return Ok(json!({"state": state, "owners": mine}));
    }
    if let Some(t) = pr {
        let provider = provider_for(&t, body);
        let queued = enqueue(app, pr_item(app, &t, &head, &provider, body))?;
        return Ok(json!({"state": state, "owners": true, "task": rf("task", t.id()), "cancel": if queued { "queued" } else { "waiting" }}));
    }
    let provider = { let p = provider_for(&Row::new(), body); if p.is_empty() { host.clone() } else { p } };
    let item = json!({"key": format!("{repo}:{}:{head}", body_str(body, "branch")), "task_id": null, "repo": repo, "host": host, "num": 0,
                      "url": url, "head": head, "branch": body.get("branch"), "provider": provider, "build_url": body.get("build_url"),
                      "build_id": body.get("build_id"), "tries": 0, "round": 0, "next_at": now_iso()});
    let queued = enqueue(app, item)?;
    Ok(json!({"state": state, "owners": true, "cancel": if queued { "queued" } else { "waiting" }}))
}

/// After a poll: a watched PR with checks running while PR builds are stopped gets its builds cancelled
/// (once per push).
pub fn sweep(app: &App) -> Result<()> {
    if !stopped(app) {
        return Ok(());
    }
    let tasks = app.db.q(
        "SELECT * FROM tasks WHERE pr_num IS NOT NULL AND pr_repo IS NOT NULL AND (pr_phase IS NULL OR pr_phase NOT IN ('merged', 'declined'))",
        p![],
    )?;
    for t in tasks {
        let f = flow(&t);
        let Some(rec) = f.get("rec").filter(|r| r.is_object()) else { continue };
        let head = rec["head"].as_str().unwrap_or("");
        if head.is_empty() || rec["running"].as_i64().unwrap_or(0) == 0 || f.get("builds_cancelled").and_then(|m| m.get(head)).is_some() {
            continue;
        }
        let running_url = rec["checks"].as_array().and_then(|a| a.iter().find(|c| c["state"] == "running")).and_then(|c| c["url"].as_str()).unwrap_or("");
        let provider = provider_of(running_url).map(|p| p.to_string()).unwrap_or_else(|| t.st("pr_host"));
        enqueue(app, pr_item(app, &t, head, &provider, &json!({"build_url": running_url})))?;
    }
    Ok(())
}

/// Every runner tick: tries the cancels that are due.
pub fn tick(app: &App) -> Result<()> {
    let due: Vec<Value> = queue(app).into_iter().filter(|x| x["next_at"].as_str().map(|n| n <= now_iso().as_str()).unwrap_or(true)).collect();
    for item in due {
        if !stopped(app) {
            return save_queue(app, &[]);
        }
        let r = attempt(app, &item);
        let mut q = queue(app);
        let Some(pos) = q.iter().position(|x| x["key"] == item["key"]) else { continue };
        match r {
            Ok((what, stopped_n)) => {
                q.remove(pos);
                // Swept again on the follow-up schedule, from the first cancel, for builds queued after it.
                let round = item["round"].as_i64().unwrap_or(0) as usize;
                let first_at = if round == 0 { now_ts() } else { item["first_at"].as_f64().unwrap_or_else(now_ts) };
                if let Some(wait) = app.cfg.pr_builds.follow_up_secs.get(round) {
                    let mut next = item.clone();
                    next["round"] = json!(round + 1);
                    next["first_at"] = json!(first_at);
                    next["tries"] = json!(0);
                    next["error"] = Value::Null;
                    next["next_at"] = json!(iso(first_at + wait));
                    q.push(next);
                }
                save_queue(app, &q)?;
                if round == 0 || stopped_n.is_none_or(|n| n > 0) {
                    cancelled(app, &item, &what, round)?;
                }
            }
            Err((why, retry)) => {
                let tries = item["tries"].as_i64().unwrap_or(0) as usize + 1;
                let waits = &app.cfg.pr_builds.retry_secs;
                if retry && tries < waits.len() {
                    q[pos]["tries"] = json!(tries);
                    q[pos]["error"] = json!(why);
                    q[pos]["next_at"] = json!(iso(now_ts() + waits[tries]));
                    save_queue(app, &q)?;
                } else {
                    q.remove(pos);
                    save_queue(app, &q)?;
                    gave_up(app, &item, tries, &why)?;
                }
            }
        }
    }
    Ok(())
}

/// Cancels one queued item's builds: what it did (and how many builds it stopped, when it knows), or
/// why not and whether trying again could help.
fn attempt(app: &App, item: &Value) -> std::result::Result<(String, Option<usize>), (String, bool)> {
    let s = |k: &str| item[k].as_str().unwrap_or("").to_string();
    let provider = s("provider");
    if let Some(cmd) = app.cfg.pr_builds.cancel.get(&provider).map(|c| c.trim()).filter(|c| !c.is_empty()) {
        let env: Vec<(String, String)> = [
            ("TB_PROVIDER", provider.clone()),
            ("TB_PR_URL", s("url")),
            ("TB_PR_REPO", s("repo")),
            ("TB_PR_NUM", item["num"].as_i64().filter(|n| *n > 0).map(|n| n.to_string()).unwrap_or_default()),
            ("TB_HEAD", s("head")),
            ("TB_BRANCH", s("branch")),
            ("TB_BUILD_URL", s("build_url")),
            ("TB_BUILD_ID", item["build_id"].as_str().map(|x| x.to_string()).or_else(|| item["build_id"].as_i64().map(|n| n.to_string())).unwrap_or_default()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let timeout = app.cfg.pr_builds.timeout_secs;
        let o = crate::proc::run_with(Path::new("/bin/sh"), &["-c".into(), cmd.to_string()], None, timeout, &env, None)
            .map_err(|_| (format!("the {provider} cancel command didn't finish in {timeout} seconds"), true))?;
        if o.code != Some(0) {
            let why = o.stderr.lines().chain(o.stdout.lines()).find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
            return Err((format!("the {provider} cancel command exited {}{}", o.code.unwrap_or(-1), if why.is_empty() { String::new() } else { format!(": {why}") }), true));
        }
        return Ok((format!("the {provider} cancel command ran"), None));
    }
    let host = s("host");
    if !prhost::watched(Some(&host)) || !(provider == host || provider.is_empty()) {
        let name = if provider.is_empty() { "this CI".to_string() } else { provider.clone() };
        return Err((format!("there's no cancel command for {name} in [pr_builds.cancel]"), false));
    }
    let h = prhost::host_for(app, &host).map_err(|e| (e, true))?;
    let pr = PrRef { host: host.clone(), repo: s("repo"), num: item["num"].as_i64().unwrap_or(0), url: s("url") };
    match h.cancel_builds(&pr, &s("head")) {
        Ok(Cancelled::Stopped(n)) => Ok((format!("stopped {}", plural(n as i64, "build")), Some(n))),
        Ok(Cancelled::Unsupported(why)) => Err((why, false)),
        Err(e) => Err((e, true)),
    }
}

fn short_head(item: &Value) -> String {
    item["head"].as_str().unwrap_or("").chars().take(8).collect()
}

/// Keeps a cancel (or a give-up, `ok: false`) in the recent list `tb pr-builds` shows.
fn remember(app: &App, item: &Value, what: &str, ok: bool) -> Result<()> {
    let mut recent = setting(app, RECENT).as_array().cloned().unwrap_or_default();
    recent.insert(0, json!({"at": now_iso(), "ok": ok, "what": what, "task": item["task_id"].as_i64().map(|t| rf("task", t)),
                            "repo": item["repo"], "num": item["num"], "branch": item["branch"], "head": item["head"],
                            "follow_up": item["round"].as_i64().unwrap_or(0) > 0}));
    recent.truncate(RECENT_MAX);
    app.db.set_setting(RECENT, Some(&jdumps(&Value::Array(recent))))
}

fn cancelled(app: &App, item: &Value, what: &str, round: usize) -> Result<()> {
    remember(app, item, what, true)?;
    let again = if round > 0 { " again" } else { "" };
    let Some(tid) = item["task_id"].as_i64() else {
        app.info(format!("pr-builds: cancelled the builds of {} {}{again}: {what}", item["repo"].as_str().unwrap_or(""), short_head(item)));
        return Ok(());
    };
    let t = board::get_task(app, tid)?;
    if round == 0 {
        let mut done = flow(&t).get("builds_cancelled").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        done.insert(item["head"].as_str().unwrap_or("").to_string(), json!(now_iso()));
        prflow::merge_flow(app, tid, vec![("builds_cancelled", Value::Object(done))])?;
    }
    board::log_event(app, tid, board::BOARD, "status", &format!("PR builds are stopped: cancelled the builds of PR #{} at {}{again} ({what})", t.i0("pr_num"), short_head(item)))?;
    Ok(())
}

fn gave_up(app: &App, item: &Value, tries: usize, why: &str) -> Result<()> {
    let task = item["task_id"].as_i64();
    let what = match task {
        Some(_) => format!("PR #{}", item["num"].as_i64().unwrap_or(0)),
        None => format!("{} {}", item["repo"].as_str().unwrap_or(""), item["branch"].as_str().unwrap_or("")),
    };
    let text = format!(
        "PR builds are stopped, but the board couldn't cancel the builds of {what} at {} after {}: {why}. Cancel them in the CI.",
        short_head(item),
        if tries == 1 { "1 try".to_string() } else { format!("{tries} tries") }
    );
    remember(app, item, why, false)?;
    if let Some(tid) = task {
        board::log_event(app, tid, board::BOARD, "status", &text)?;
    }
    let goal = match task {
        Some(t) => board::get_task(app, t)?.i("goal_id"),
        None => None,
    };
    dispatch::raise(app, &text, task, goal, Some(&format!("pr-builds:{}", item["key"].as_str().unwrap_or(""))), false)?;
    Ok(())
}

/// The owner's `pr.checks` / `pr.fix` hooks for a build on a PR the board doesn't read (GitLab, …),
/// once per push and event. A skip counts the push's checks as passed.
fn build_hooks(app: &App, t: &Row, state: &str, head: &str) -> Result<Option<String>> {
    let event = if is_running(state) {
        "pr.checks"
    } else if matches!(state, "failed" | "failure" | "error") {
        "pr.fix"
    } else {
        return Ok(None);
    };
    let key = format!("{event}:{head}");
    let f = flow(t);
    let mut hooked = f.get("hooked").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    if hooked.contains_key(&key) || prflow::checks_skipped(&f, &json!({"head": head})).is_some() {
        return Ok(None);
    }
    let d = hooks::gate(app, event, t, json!({"head": head, "build_state": state}));
    hooked.insert(key, json!(now_iso()));
    let pr = format!("PR #{} for {}", t.i0("pr_num"), rf("task", t.id()));
    let mut changes = vec![("hooked", Value::Object(hooked))];
    let said = match &d {
        hooks::Decision::Go => "go",
        hooks::Decision::Skip { reason, .. } => {
            let mut skips = f.get("skip_checks").and_then(|v| v.as_object()).cloned().unwrap_or_default();
            skips.insert(if head.is_empty() { "*".into() } else { head.to_string() }, json!(reason));
            changes.push(("skip_checks", Value::Object(skips)));
            hooks::note(app, t.id(), &d.said(&format!("{pr}: its checks were skipped")))?;
            "skip"
        }
        hooks::Decision::Block { .. } => {
            let line = d.said(&format!("{pr} stopped at its failed build"));
            hooks::note(app, t.id(), &line)?;
            dispatch::add_alert(app, &line, Some(t.id()), t.i("goal_id"), None, Some("pr"))?;
            "block"
        }
    };
    prflow::merge_flow(app, t.id(), changes)?;
    Ok(Some(said.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_provider_from_a_build_link() {
        assert_eq!(provider_of("https://github.com/acme/web/actions/runs/123/job/4"), Some("github"));
        assert_eq!(provider_of("https://bitbucket.org/ws/repo/pipelines/results/77"), Some("bitbucket"));
        assert_eq!(provider_of("https://jenkins.example.com/job/x/1"), None);
        assert!(is_running("started") && !is_running("passed"));
    }
}
