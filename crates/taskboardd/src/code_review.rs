//! The pre-PR code review (docs/REVIEW-PROTOCOL.md): with a reviewer command set, a task's `tb submit`
//! waits in code review while the board runs the command in the task's worktree, then carries on.
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, p, projects, review_triage, steps};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CodeReviewConfig {
    /// The reviewer, run with `sh -c` in the task's worktree. Unset: no code review.
    pub command: Option<String>,
    pub timeout_mins: u64,
    pub max_rounds: u32,
}

impl Default for CodeReviewConfig {
    fn default() -> Self {
        CodeReviewConfig { command: None, timeout_mins: 30, max_rounds: 5 }
    }
}

/// `tasks.code_review`: the stage a task is in while its review runs, as JSON (`state`, `round`,
/// `tries`, the held `tb submit` report as `body`, `since`), and while its agent fixes a round's
/// findings (`fixing`). Null outside it.
pub const ADDED: &[(&str, &str, &str)] = &[("tasks", "code_review", "TEXT")];

/// Projects' own reviewer commands: a command, or "" for none.
const SETTING: &str = "code_review_commands";
const COMMAND_MAX: usize = 1000;
/// A reviewer that fails is run once more before the task carries on without it.
const TRIES: u64 = 2;
const TICK_SECS: f64 = 5.0;
const OUTPUT_MAX: usize = 4000;
pub const EVENT: &str = "code_review";
/// The stage's state while the task's own agent fixes a triaged round's `fix` findings.
pub const FIXING: &str = "fixing";

fn overrides(app: &App) -> Result<Row> {
    Ok(jloads_obj(app.db.get_setting(SETTING)?.as_deref()))
}

/// The project's own reviewer setting: `None` follows `[code_review] command`, `Some("")` is none.
pub fn setting(app: &App, project: &str) -> Result<Option<String>> {
    Ok(overrides(app)?.get(project).and_then(|v| v.as_str()).map(str::to_string))
}

/// The reviewer command for the project's tasks, if it has one.
pub fn command(app: &App, project: &str) -> Result<Option<String>> {
    let cmd = match setting(app, project)? {
        Some(own) => Some(own),
        None => app.cfg.code_review.command.clone(),
    };
    Ok(cmd.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()))
}

/// Sets a project's reviewer from a body's `code_review`: a command, "none" (no review for it) or
/// "default"/null (config.toml's). False when the body doesn't name it.
pub fn set(app: &App, project: &str, body: &Value) -> Result<bool> {
    let Some(v) = body.get("code_review") else { return Ok(false) };
    let to = match v {
        Value::Null => None,
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() || s.eq_ignore_ascii_case("default") {
                None
            } else if s.eq_ignore_ascii_case("none") {
                Some(String::new())
            } else if s.chars().count() > COMMAND_MAX || s.contains(['\n', '\r']) {
                return err(400, format!("code_review is one line of {COMMAND_MAX} characters at most, none or default."));
            } else {
                Some(s.to_string())
            }
        }
        _ => return err(400, "code_review is a reviewer command, none or default."),
    };
    let mut all = overrides(app)?;
    match to {
        Some(s) => all.insert(project.into(), json!(s)),
        None => all.remove(project),
    };
    app.db.set_setting(SETTING, Some(&jdumps(&Value::Object(all))))?;
    Ok(true)
}

/// The task's code review stage, while it's in one.
pub fn stage(t: &Row) -> Option<Value> {
    t.s("code_review").and_then(|s| serde_json::from_str::<Value>(s).ok()).filter(|v| v.is_object())
}

/// The task waits on its review: a round queued, running or in triage (not while its agent fixes one).
pub fn in_review(t: &Row) -> bool {
    stage(t).is_some_and(|s| s["state"] != FIXING)
}

/// The task's agent is fixing a round's findings: that stage.
pub fn fixing(t: &Row) -> Option<Value> {
    stage(t).filter(|s| s["state"] == FIXING)
}

/// The rounds the task's review has run (a round whose reviewer failed counts).
fn rounds_run(done: &[Value]) -> u64 {
    done.iter().filter_map(|r| r["round"].as_u64()).max().unwrap_or(0)
}

pub(crate) fn save_stage(app: &App, id: i64, stage: Option<&Value>) -> Result<()> {
    board::update_task(app, id, fields!["code_review" => stage.map(jdumps)])
}

/// A recorded round: its number, whether the reviewer ran, the head and tree it read, and its result.
pub fn rounds(app: &App, task_id: i64) -> Result<Vec<Value>> {
    let rows = app.db.q("SELECT at, data FROM events WHERE task_id = ? AND kind = ? ORDER BY id", p![task_id, EVENT])?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let mut d = serde_json::from_str::<Value>(r.s("data")?).ok()?;
            d["at"] = json!(r.st("at"));
            Some(d)
        })
        .collect())
}

/// What the task card shows: the stage while it's in review, and its last round. Null when it has neither.
pub fn card(app: &App, t: &Row) -> Result<Value> {
    let last = app.db.q("SELECT at, data FROM events WHERE task_id = ? AND kind = ? ORDER BY id DESC LIMIT 1", p![t.id(), EVENT])?;
    let last = last.first().and_then(|r| {
        let d = serde_json::from_str::<Value>(r.s("data")?).ok()?;
        Some(json!({
            "round": d["round"], "ok": d["ok"], "verdict": d["result"]["verdict"], "headline": d["result"]["headline"],
            "findings": d["result"]["findings"].as_array().map_or(0, |f| f.len()), "error": d["error"], "at": r.st("at"),
        }))
    });
    let now = stage(t);
    if now.is_none() && last.is_none() {
        return Ok(Value::Null);
    }
    let s = now.unwrap_or(Value::Null);
    let mut card = json!({"state": s["state"], "round": s["round"], "tries": s["tries"], "since": s["since"], "last": last});
    if s["state"] == review_triage::STATE {
        card["triage"] = review_triage::progress(app, t.id(), s["round"].as_u64().unwrap_or(1))?;
    }
    if s["state"] == FIXING {
        card["fix"] = review_triage::fix_progress(app, t.id(), s["round"].as_u64().unwrap_or(1))?;
    }
    Ok(card)
}

/// Each round the task's code review ran, for its panel: the try that counted, its reviewer and result,
/// and each finding with its triage outcome, the backlog item it filed and what fixed it. `capped` on
/// a round that reached the cap with `fix` findings left, so the task went on to its PR with them.
pub fn shown_rounds(app: &App, t: &Row) -> Result<Value> {
    let max = app.cfg.code_review.max_rounds as u64;
    let mut out: Vec<Value> = vec![];
    for d in rounds(app, t.id())? {
        let findings: Vec<Value> = d["result"]["findings"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| {
                let tr = &d["triage"][f["id"].as_str().unwrap_or("")];
                json!({
                    "id": f["id"], "title": f["title"], "severity": f["severity"], "file": f["file"], "line": f["line"], "url": f["url"],
                    "outcome": tr["outcome"], "reason": tr["reason"], "issue": tr["issue"].as_i64().map(|i| rf("issue", i)), "fixed": tr["fixed"],
                })
            })
            .collect();
        let open = findings.iter().filter(|f| f["outcome"] == "fix" && f["fixed"].is_null()).count();
        let row = json!({
            "round": d["round"], "try": d["try"], "ok": d["ok"], "reviewer": d["reviewer"].as_str().or(d["result"]["reviewer"].as_str()),
            "verdict": d["result"]["verdict"], "headline": d["result"]["headline"], "error": d["error"], "cost": d["cost"]["usd"],
            "head": d["head"], "at": d["at"], "findings": findings,
            "capped": d["ok"] == true && d["round"].as_u64().is_some_and(|n| n >= max) && open > 0 && stage(t).is_none(),
        });
        match out.iter_mut().find(|r| r["round"] == row["round"]) {
            Some(r) => *r = row,
            None => out.push(row),
        }
    }
    Ok(json!(out))
}

/// Before `tb submit` (outside its transaction): when the task's project has a reviewer and its code
/// hasn't been reviewed as it stands, holds the report and puts the task in code review. The reply for
/// the agent then, else `None` to carry on with the submit. A report sent on later (`spooled`) skips the
/// checks the agent would have heard: the held submit meets them again when it carries on. A submit
/// while the agent fixes a round's findings runs the next round when the code changed, else ends the
/// fixing and carries on.
pub fn gate(app: &App, body: &Value, t: &Row, spooled: bool) -> Result<Option<Value>> {
    if body["code_review_replay"] == true {
        return Ok(None);
    }
    let was = fixing(t);
    let held = hold(app, body, t, spooled)?;
    if let (None, Some(s)) = (&held, was) {
        review_triage::end_fixing(app, t, &s)?;
    }
    Ok(held)
}

fn hold(app: &App, body: &Value, t: &Row, spooled: bool) -> Result<Option<Value>> {
    if board::handed_in(t) {
        return Ok(None);
    }
    let Some(_) = command(app, &t.st("project"))? else { return Ok(None) };
    let given = |k: &str| body[k].as_str().is_some_and(|s| !s.trim().is_empty());
    if given("no_pr") || given("pr") || has(t.s("pr_url")) || crate::design::is(t) || !projects::task_ships_pr(app, t)? {
        return Ok(None);
    }
    let me = rf("task", t.id());
    if in_review(t) {
        return err(409, format!("{me} is already in code review. End your turn: the board carries on once the review is in."));
    }
    let done = rounds(app, t.id())?;
    let run = rounds_run(&done);
    if run >= app.cfg.code_review.max_rounds as u64 {
        return Ok(None);
    }
    let dir = worktree(app, t);
    let (head, tree) = (head_of(&dir), tree_of(&dir));
    if tree.is_some() && done.iter().rev().find(|r| r["ok"] == true).is_some_and(|r| r["tree"].as_str() == tree.as_deref()) {
        // Reviewed as it stands: a submit that was turned back after its review needn't wait again.
        return Ok(None);
    }
    let pr_body = body["pr_body"].as_str().unwrap_or("");
    if !pr_body.trim().is_empty() && !spooled {
        let problems = crate::propen::body_problems(&app.cfg.pr_body, pr_body);
        if !problems.is_empty() {
            return err(400, format!("The PR description needs work before the board opens the PR:\n- {}\nFix the file and run tb submit again.", problems.join("\n- ")));
        }
    }
    let before: &[steps::Before] = if pr_body.trim().is_empty() { &[steps::Before::Done] } else { &[steps::Before::Pr, steps::Before::Done] };
    let left = steps::missing_at(app, t, before, head.as_deref())?;
    if !left.is_empty() && !spooled {
        return err(409, steps::refusal_at(app, t, "the code review", &left, head.as_deref()));
    }
    let mut held = body.clone();
    if let Some(o) = held.as_object_mut() {
        o.remove("at");
        o.insert("task".into(), json!(me));
    }
    let round = run + 1;
    let stage = json!({"state": "queued", "round": round, "tries": 0, "since": now_iso(), "body": held});
    app.db.tx(|| {
        save_stage(app, t.id(), Some(&stage))?;
        board::update_task(app, t.id(), fields!["latest" => "In code review"])?;
        board::log_event(app, t.id(), board::BOARD, "status", &format!("In code review (round {round})"))
    })?;
    let then = if pr_body.trim().is_empty() { "finishes the task" } else { "opens the PR and finishes the task" };
    Ok(Some(json!({
        "ok": true, "task": me, "code_review": true,
        "message": format!("{me} is in code review. The board runs the reviewer on its worktree, then {then}. End your turn now; leave the worktree as it is until then."),
    })))
}

/// Runs the code review rounds that are due, one thread per task, until the board stops.
pub fn run(app: Arc<App>) {
    let busy: Arc<Mutex<HashSet<i64>>> = Default::default();
    while !app.stopping() {
        match waiting(&app) {
            Ok(ids) => {
                for id in ids {
                    if !busy.lock().unwrap().insert(id) {
                        continue;
                    }
                    let (a, b) = (app.clone(), busy.clone());
                    std::thread::spawn(move || {
                        take(&a, id);
                        b.lock().unwrap().remove(&id);
                    });
                }
            }
            Err(e) => app.info(format!("code review: {}", e.message)),
        }
        tidy(&app);
        app.sleep(TICK_SECS);
    }
}

/// Every task in code review, run in turn here (tests; the board runs them on their own threads).
pub fn sweep(app: &App) -> Result<()> {
    for id in waiting(app)? {
        take(app, id);
    }
    tidy(app);
    Ok(())
}

fn waiting(app: &App) -> Result<Vec<i64>> {
    Ok(app.db.q("SELECT id FROM tasks WHERE code_review IS NOT NULL", p![])?.iter().filter_map(|r| r.i("id")).collect())
}

/// Runs the task's reviewer until it gives a result or has failed twice, then lets the task carry on.
fn take(app: &App, id: i64) {
    loop {
        match round(app, id) {
            Ok(true) => continue,
            Ok(false) => return,
            Err(e) => {
                app.info(format!("code review: {}: {}", rf("task", id), e.message));
                return;
            }
        }
    }
}

/// One try of the task's round. True when the reviewer failed and gets another go.
fn round(app: &App, id: i64) -> Result<bool> {
    let t = board::get_task(app, id)?;
    let Some(mut stage) = stage(&t) else { return Ok(false) };
    let triaging = stage["state"] == review_triage::STATE;
    if board::handed_in(&t) || t.b("failed") {
        if triaging {
            review_triage::close(app, &stage, &format!("Task board: {} has ended", rf("task", id)))?;
        }
        save_stage(app, id, None)?;
        return Ok(false);
    }
    if triaging {
        return review_triage::tick(app, &t, stage).map(|_| false);
    }
    if stage["state"] == FIXING {
        return Ok(false);
    }
    let Some(cmd) = command(app, &t.st("project"))? else {
        return carry_on(app, &t, stage, None).map(|_| false);
    };
    let n = stage["round"].as_u64().unwrap_or(1);
    let tries = stage["tries"].as_u64().unwrap_or(0) + 1;
    stage["state"] = json!("running");
    stage["tries"] = json!(tries);
    save_stage(app, id, Some(&stage))?;
    let ran = review(app, &t, &cmd, n);
    let me = rf("task", id);
    let line = match &ran.error {
        None => format!(
            "Code review round {n}: {}{}",
            ran.result["verdict"].as_str().unwrap_or("pass"),
            ran.result["headline"].as_str().map(|h| format!(": {h}")).unwrap_or_default()
        ),
        Some(e) => format!("Code review round {n} failed (try {tries}): {e}"),
    };
    let data = json!({
        "name": "Code review", "round": n, "try": tries, "ok": ran.error.is_none(), "head": ran.head, "tree": ran.tree,
        "result": ran.result, "reviewer": ran.reviewer, "cost": ran.cost, "error": ran.error, "output": ran.output,
        "secs": ran.secs,
    });
    app.db.tx(|| board::log_event_full(app, id, board::BOARD, EVENT, &line, Some(data.clone()), None).map(|_| ()))?;
    if ran.error.is_some() && tries < TRIES && !app.stopping() {
        stage["state"] = json!("queued");
        save_stage(app, id, Some(&stage))?;
        return Ok(true);
    }
    if app.stopping() && ran.error.is_some() {
        // Cut short by the board stopping: the round runs again when it's back.
        stage["state"] = json!("queued");
        stage["tries"] = json!(tries - 1);
        save_stage(app, id, Some(&stage))?;
        return Ok(false);
    }
    if ran.error.is_none() && review_triage::wanted(&ran.result) {
        match review_triage::open(app, &t, &mut stage, n, &ran.result) {
            Ok(()) => return Ok(false),
            Err(e) => {
                let alert = format!("{me}'s code review round {n} found something, but its triage couldn't start ({}), so it carried on.", e.message);
                return carry_on(app, &t, stage, Some(alert)).map(|_| false);
            }
        }
    }
    let alert = ran.error.as_ref().map(|e| format!("{me}'s code review failed twice ({e}), so it carried on without one."));
    carry_on(app, &t, stage, alert).map(|_| false)
}

/// Ends the stage and sends the held `tb submit` on, as if the agent had just run it. When that's
/// refused, the agent hears why in its terminal and the owner gets an alert.
pub(crate) fn carry_on(app: &App, t: &Row, stage: Value, alert: Option<String>) -> Result<()> {
    let id = t.id();
    let me = rf("task", id);
    app.db.tx(|| {
        save_stage(app, id, None)?;
        if let Some(a) = &alert {
            crate::dispatch::add_alert(app, a, Some(id), t.i("goal_id"), None, None)?;
        }
        Ok(())
    })?;
    let mut body = stage["body"].clone();
    if !body.is_object() {
        return Ok(());
    }
    body["code_review_replay"] = json!(true);
    if let Err(e) = crate::reports::handle(app, body, true) {
        let text = format!(
            "[task-board] {me}'s code review is in, but its tb submit was turned back: {}\nDeal with that, then run tb submit again.",
            e.message
        );
        app.db.tx(|| {
            crate::dispatch::add_alert(app, &format!("{me}: tb submit was turned back after its code review: {}", one_line(&e.message, 300)), Some(id), t.i("goal_id"), None, None)?;
            crate::deliver::add(app, "code_review", &text, id, None, None).map(|_| ())
        })?;
    }
    Ok(())
}

struct Ran {
    head: Option<String>,
    tree: Option<String>,
    result: Value,
    reviewer: Option<String>,
    cost: Value,
    error: Option<String>,
    output: String,
    secs: f64,
}

/// The board's own files for a task's review: `state` (kept across rounds) and one folder per round.
pub fn task_dir(app: &App, id: i64) -> PathBuf {
    app.cfg.data.join("code-review").join(rf("task", id))
}

pub(crate) fn worktree(app: &App, t: &Row) -> String {
    steps::vars_at(app, t, None).get("worktree").cloned().filter(|w| !w.is_empty()).unwrap_or_else(|| t.st("repo_path"))
}

fn review(app: &App, t: &Row, cmd: &str, n: u64) -> Ran {
    let started = Instant::now();
    let dir = worktree(app, t);
    let head = head_of(&dir);
    let tree = tree_of(&dir);
    let mut ran = Ran { head: head.clone(), tree, result: Value::Null, reviewer: None, cost: Value::Null, error: None, output: String::new(), secs: 0.0 };
    let base = task_dir(app, t.id());
    let state = base.join("state");
    let here = base.join(format!("round-{n}"));
    let result = here.join("result.json");
    let intent = here.join("intent.md");
    let prepared = std::fs::create_dir_all(&state)
        .and_then(|_| std::fs::create_dir_all(&here))
        .and_then(|_| match std::fs::remove_file(&result) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        })
        .and_then(|_| std::fs::write(&intent, intent_text(app, t)));
    if let Err(e) = prepared {
        ran.error = Some(format!("couldn't set up its files: {e}"));
        return ran;
    }
    let mut env: Vec<(String, String)> = steps::vars_at(app, t, head.as_deref()).into_iter().map(|(k, v)| (format!("TASKBOARD_{}", k.to_uppercase()), v)).collect();
    let var = |k: &str| env.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone()).filter(|v| !v.is_empty());
    let base_ref = var("TASKBOARD_BASE_REF").unwrap_or_default();
    let branch = var("TASKBOARD_BRANCH").or_else(|| git_out(&dir, &["symbolic-ref", "--short", "-q", "HEAD"], &[])).unwrap_or_default();
    let path = |p: &Path| p.to_string_lossy().to_string();
    let mut add = |k: &str, v: String| {
        env.retain(|(x, _)| x != k);
        env.push((k.to_string(), v));
    };
    add("TASKBOARD_REVIEW_PROTOCOL", "1".into());
    add("TASKBOARD_REPO", dir.clone());
    add("TASKBOARD_BASE", base_ref);
    add("TASKBOARD_HEAD", head.clone().unwrap_or_default());
    add("TASKBOARD_BRANCH", branch);
    add("TASKBOARD_ROUND", n.to_string());
    add("TASKBOARD_INTENT", path(&intent));
    add("TASKBOARD_STATE_DIR", path(&state));
    add("TASKBOARD_RESULT", path(&result));
    if let Some(prev) = previous(app, t.id(), n, &here) {
        add("TASKBOARD_PREVIOUS", path(&prev));
    }
    let timeout = Duration::from_secs(app.cfg.code_review.timeout_mins.max(1) * 60);
    let me = rf("task", t.id());
    let ended = run_reviewer(cmd, &dir, &env, timeout, &|| app.stopping(), &|line| app.info(format!("code review {me}: {line}")));
    ran.secs = started.elapsed().as_secs_f64();
    match ended {
        Err(e) => ran.error = Some(e),
        Ok((code, output)) => {
            ran.output = output;
            ran.error = if code != Some(0) {
                Some(match code {
                    Some(c) => format!("the reviewer exited with {c}"),
                    None => "the reviewer was killed".into(),
                })
            } else {
                match read_result(&result) {
                    Ok(v) => {
                        ran.reviewer = v["reviewer"].as_str().map(|s| one_line(s, 80)).filter(|s| !s.is_empty());
                        ran.cost = v["cost"].clone();
                        ran.result = steps::clean_result(Some(&v)).unwrap_or(Value::Null);
                        None
                    }
                    Err(e) => Some(e),
                }
            };
        }
    }
    ran
}

/// The reviewer's result: a JSON object with a pass, fail or skip verdict.
fn read_result(path: &Path) -> std::result::Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|_| "the reviewer wrote no result".to_string())?;
    let v = serde_json::from_str::<Value>(&text).map_err(|e| format!("its result isn't JSON ({e})"))?;
    match v["verdict"].as_str().map(str::to_lowercase).as_deref() {
        Some("pass" | "fail" | "skip") => Ok(v),
        _ => Err("its result has no pass, fail or skip verdict".into()),
    }
}

/// The task as the reviewer reads it: its title and detail, and its goal's done-when.
pub(crate) fn intent_text(app: &App, t: &Row) -> String {
    let mut s = format!("# {}: {}\n", rf("task", t.id()), t.st("title"));
    let detail = t.st("detail");
    if !detail.trim().is_empty() {
        s += &format!("\n{}\n", detail.trim());
    }
    if let Some(g) = t.i("goal_id").and_then(|g| board::get_goal(app, g).ok()) {
        s += &format!("\n## Goal: {}\n", g.st("name"));
        let outcome = g.st("outcome");
        if !outcome.trim().is_empty() {
            s += &format!("\nDone when: {}\n", outcome.trim());
        }
    }
    s
}

/// The last round's result before round `n`, each finding with the outcome its triage gave it (and the
/// task agent's note when it marked it fixed), written for the reviewer to read.
fn previous(app: &App, id: i64, n: u64, here: &Path) -> Option<PathBuf> {
    let last = rounds(app, id).ok()?.into_iter().rev().find(|r| r["ok"] == true && r["round"].as_u64().is_some_and(|m| m < n))?;
    let mut result = last["result"].clone();
    if !result.is_object() {
        return None;
    }
    let triage = &last["triage"];
    for f in result["findings"].as_array_mut().into_iter().flatten() {
        let t = &triage[f["id"].as_str().unwrap_or("")];
        f["outcome"] = t["outcome"].clone();
        f["reason"] = t["reason"].clone();
        f["fixed"] = t["fixed"].clone();
    }
    result["protocol"] = json!(1);
    let path = here.join("previous.json");
    std::fs::write(&path, jdumps(&result)).ok()?;
    Some(path)
}

/// The PR description's account of the task's code review: how many rounds it took and what came of
/// their findings. None when it had no round.
pub fn summary(app: &App, task_id: i64) -> Result<Option<String>> {
    let done = rounds(app, task_id)?;
    let run = rounds_run(&done);
    if run == 0 {
        return Ok(None);
    }
    let (mut fixed, mut open, mut backlog, mut dismissed) = (0, 0, 0, 0);
    for r in done.iter().filter(|r| r["ok"] == true) {
        for tr in r["triage"].as_object().into_iter().flat_map(|o| o.values()) {
            match tr["outcome"].as_str() {
                Some("fix") if tr["fixed"].is_string() => fixed += 1,
                Some("fix") => open += 1,
                Some("backlog") => backlog += 1,
                Some("dismiss") => dismissed += 1,
                _ => {}
            }
        }
    }
    let took = if run == 1 { "1 round".to_string() } else { format!("{run} rounds") };
    let line = if fixed + open + backlog + dismissed == 0 {
        format!("- {took}, nothing to fix")
    } else {
        let mut l = format!("- {took}: {fixed} fixed, {backlog} backlogged, {dismissed} dismissed");
        if open > 0 {
            l += &format!(", {open} left open");
        }
        l
    };
    Ok(Some(format!("## Code review\n\n{line}")))
}

/// Clears the folders of tasks that are no longer in code review and have been handed in.
fn tidy(app: &App) {
    let Ok(dirs) = std::fs::read_dir(app.cfg.data.join("code-review")) else { return };
    for d in dirs.flatten() {
        let name = d.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_prefix('T').and_then(|n| n.parse::<i64>().ok()) else { continue };
        let ended = match board::find_task(app, Some(id)) {
            Ok(Some(t)) => board::handed_in(&t) && !in_review(&t),
            Ok(None) => true,
            Err(_) => false,
        };
        if ended {
            let _ = std::fs::remove_dir_all(d.path());
        }
    }
}

pub(crate) fn git_out(dir: &str, args: &[&str], env: &[(String, String)]) -> Option<String> {
    let bin = crate::proc::which("git")?;
    let mut all: Vec<String> = vec!["-C".into(), dir.into()];
    all.extend(args.iter().map(|a| a.to_string()));
    let o = crate::proc::run_with(&bin, &all, None, 60.0, env, None).ok()?;
    (o.code == Some(0)).then(|| o.stdout.trim().to_string()).filter(|s| !s.is_empty())
}

fn head_of(dir: &str) -> Option<String> {
    git_out(dir, &["rev-parse", "HEAD"], &[])
}

/// The worktree's tree as it stands, uncommitted changes and new files included, written through a
/// throwaway index so the checkout's own index is left alone.
fn tree_of(dir: &str) -> Option<String> {
    let index = std::env::temp_dir().join(format!("taskboard-review-index-{}-{}", std::process::id(), rand::random::<u64>()));
    let env = [("GIT_INDEX_FILE".to_string(), index.to_string_lossy().to_string())];
    let tree = git_out(dir, &["read-tree", "HEAD"], &env)
        .or(Some(String::new()))
        .and_then(|_| git_out(dir, &["add", "-A"], &env).or(Some(String::new())))
        .and_then(|_| git_out(dir, &["write-tree"], &env));
    let _ = std::fs::remove_file(&index);
    tree
}

/// Runs the reviewer with `sh -c` in its own process group, its stderr passed to `progress` line by
/// line. Its exit code (`None` when killed) and the end of what it said, or why it couldn't run.
fn run_reviewer(
    cmd: &str,
    dir: &str,
    env: &[(String, String)],
    timeout: Duration,
    stop: &dyn Fn() -> bool,
    progress: &dyn Fn(&str),
) -> std::result::Result<(Option<i32>, String), String> {
    if !Path::new(dir).is_dir() {
        return Err(format!("its worktree {dir} isn't there"));
    }
    let mut child = Command::new("/bin/sh")
        .args(["-c", cmd])
        .current_dir(dir)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("couldn't start the reviewer: {e}"))?;
    let pgid = child.id() as i32;
    let mut out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let drain = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = out.read_to_end(&mut s);
    });
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(err).lines().map_while(|l| l.ok()) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut said = String::new();
    let mut heard = |line: String| {
        if !line.trim().is_empty() {
            progress(&one_line(&line, 300));
        }
        said.push_str(&line);
        said.push('\n');
        if said.len() > OUTPUT_MAX * 2 {
            said = said.split_off(said.len() - OUTPUT_MAX);
        }
    };
    let end = Instant::now() + timeout;
    let kill = |child: &mut std::process::Child| {
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
        let _ = child.wait();
    };
    let status = loop {
        while let Ok(line) = rx.try_recv() {
            heard(line);
        }
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() >= end => {
                kill(&mut child);
                return Err(format!("the reviewer ran past {} minutes", timeout.as_secs() / 60));
            }
            Ok(None) if stop() => {
                kill(&mut child);
                return Err("the board stopped".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("lost the reviewer: {e}")),
        }
    };
    // Anything it left running in its group goes with it.
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
    while let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
        heard(line);
    }
    drop(drain);
    let tail = if said.len() > OUTPUT_MAX { said[said.len() - OUTPUT_MAX..].to_string() } else { said };
    Ok((status.code(), tail.trim_end().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(cmd: &str, secs: u64) -> std::result::Result<(Option<i32>, String), String> {
        let dir = tempfile::tempdir().unwrap();
        run_reviewer(cmd, &dir.path().to_string_lossy(), &[("X".into(), "y".into())], Duration::from_secs(secs), &|| false, &|_| {})
    }

    #[test]
    fn reviewer_exit_and_stderr() {
        let (code, out) = run("echo working >&2; echo \"$X\" >&2; exit 3", 10).unwrap();
        assert_eq!(code, Some(3));
        assert_eq!(out, "working\ny");
    }

    #[test]
    fn reviewer_killed_at_timeout_with_its_group() {
        let started = Instant::now();
        let e = run("sleep 30 & sleep 30", 1).unwrap_err();
        assert!(e.contains("ran past"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn result_needs_a_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.json");
        assert!(read_result(&p).unwrap_err().contains("no result"));
        std::fs::write(&p, "{").unwrap();
        assert!(read_result(&p).unwrap_err().contains("isn't JSON"));
        std::fs::write(&p, r#"{"verdict":"maybe"}"#).unwrap();
        assert!(read_result(&p).is_err());
        std::fs::write(&p, r#"{"protocol":1,"verdict":"FAIL","findings":[]}"#).unwrap();
        assert!(read_result(&p).is_ok());
    }
}
