//! Master breaks: an optional watch on each project's default branch. When its CI goes red the board
//! opens a break, `M<n>`, works out whether it's the owner's to fix, and closes it once the branch is
//! green again.
//!
//! # The flow
//!
//! - **Intake.** Every `[master] every_mins` (5) the board reads each project in `[master.projects]`:
//!   the branch's head commit, its checks and its last `commits` commits ([`BranchCi`]: GitHub
//!   through `gh api`, Bitbucket through REST 2.0). A failed check on the head opens a break (one per
//!   project at a time); a head whose checks all passed closes it.
//! - **Investigate.** The suspects are the commits since the last green head the board saw (the head
//!   alone when it hasn't seen one). Each is the owner's when its author's email is in `owner_emails`.
//!   The evidence is the failing checks (with their failed steps and tests where the CI can be read)
//!   and the suspects.
//! - **Decide.** No suspect of the owner's: `not_ours`. Otherwise a headless `claude -p` reads the
//!   evidence and answers `ours`, `not_ours` or `unsure` with a reason (`[master] fault_check`); when
//!   it can't be asked, every suspect being the owner's makes it `ours`, else `unsure`.
//! - **Act.** Only `ours` gets a fix task (queued, high priority, in the project) and an urgent alert
//!   (`master:M<n>`, with the task). `unsure` isn't the owner's to fix.
//! - **Override.** `tb master M<n> ours|not-ours|unsure` is the owner's word: it acts the same way and
//!   is never re-decided.
//! - **Re-check.** While a break is open, a new head brings new suspects and decides again (unless a
//!   person set the verdict); an `unsure` one is decided again after `recheck_mins`; an `ours` one
//!   whose fix task has finished while the branch is still red raises the urgent alert again.
//! - **Close.** Green again: the break closes, the alert clears and the fix task's log says so.
//!
//! The app shows a "Master is red" banner for each open break (`state.master`).
//!
//! # The `breaks` table
//!
//! | Column | |
//! |---|---|
//! | `id` | the break's number: `M<id>` |
//! | `project`, `host`, `repo`, `branch` | what went red: the board's project, `github` or `bitbucket`, `owner/name`, the branch |
//! | `state` | `open` or `closed` |
//! | `head` | the first red head seen; `last_head` the latest one read while it's red |
//! | `green_head` | the last green head before it broke (NULL when the board never saw one); `fixed_head` the green head that closed it |
//! | `checks` | JSON list of the failing checks' names (on `last_head`) |
//! | `evidence` | JSON `{checks: [{name, url, steps, tests}], read_at}` |
//! | `suspects` | JSON `[{sha, name, email, message, ours}]`, newest first |
//! | `verdict` | `ours`, `not_ours`, `unsure`, or NULL before it's decided |
//! | `verdict_by` | `commits` (no suspect is the owner's), `claude`, `fallback`, or the person who overrode it |
//! | `verdict_why`, `verdict_at` | the reason, and when |
//! | `task_id` | the fix task (`ours` only) |
//! | `escalated_at` | when the urgent alert was raised again after the fix task finished |
//! | `opened_at`, `closed_at`, `checked_at` | ISO times |
//!
//! The setting `master_watch` keeps, per project, `{checked_at, green_head, error?}`.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::prhost::bitbucket::Http;
use crate::prhost::github::Gh;
use crate::prhost::{Check, HostResult, PrRef};
use crate::util::*;
use crate::{board, dispatch, fields, p, proc, projects};

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS breaks(
  id INTEGER PRIMARY KEY, project TEXT NOT NULL, host TEXT, repo TEXT, branch TEXT,
  state TEXT NOT NULL DEFAULT 'open', head TEXT, last_head TEXT, green_head TEXT, fixed_head TEXT,
  checks TEXT, evidence TEXT, suspects TEXT,
  verdict TEXT, verdict_by TEXT, verdict_why TEXT, verdict_at TEXT,
  task_id INT, escalated_at TEXT, opened_at TEXT, closed_at TEXT, checked_at TEXT);
CREATE INDEX IF NOT EXISTS breaks_project ON breaks(project, state);
"#;

/// `[master]` in config.toml.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MasterConfig {
    /// How often each watched branch is read, in minutes.
    pub every_mins: f64,
    /// How many of the branch's latest commits are read (the suspects come from them).
    pub commits: usize,
    /// Ask a headless `claude -p` whether a break is the owner's when one of their commits is a suspect.
    pub fault_check: bool,
    pub model: String,
    pub budget_usd: String,
    pub timeout_secs: u64,
    /// An `unsure` break is decided again after this many minutes; an `ours` one whose fix task
    /// finished while it's still red raises its alert again after it.
    pub recheck_mins: f64,
    /// The projects whose default branch is watched, by the board's project name.
    pub projects: BTreeMap<String, MasterProject>,
}

impl Default for MasterConfig {
    fn default() -> Self {
        MasterConfig {
            every_mins: 5.0,
            commits: 10,
            fault_check: true,
            model: "sonnet".into(),
            budget_usd: "0.50".into(),
            timeout_secs: 180,
            recheck_mins: 30.0,
            projects: BTreeMap::new(),
        }
    }
}

/// One watched project. Every key is optional.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MasterProject {
    /// The branch (default `main`).
    pub branch: Option<String>,
    /// `owner/name` or `workspace/repo`. Unset: from the project's `origin` remote.
    pub repo: Option<String>,
    /// `github` or `bitbucket`. Unset: from the remote.
    pub host: Option<String>,
}

/// One commit on the branch.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Commit {
    pub sha: String,
    pub name: String,
    pub email: String,
    pub message: String,
}

/// One read of a branch: its head, the head's checks, and its latest commits (newest first).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BranchRead {
    pub head: String,
    pub checks: Vec<Check>,
    pub commits: Vec<Commit>,
}

impl BranchRead {
    pub fn failed(&self) -> Vec<String> {
        self.checks.iter().filter(|c| c.state == "failed").map(|c| c.name.clone()).collect()
    }
    /// Every check on the head passed (and there is at least one).
    pub fn green(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|c| c.state == "passed")
    }
}

/// Reads a branch's CI on its host.
pub trait BranchCi: Send + Sync {
    fn read(&self, repo: &str, branch: &str, commits: usize) -> HostResult<BranchRead>;
}

/// GitHub, through `gh api`: `commits?sha=<branch>`, then the head's `check-runs` and `status`.
pub struct GithubBranch {
    pub gh: Box<dyn Gh>,
}

fn parse(text: &str) -> HostResult<Value> {
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(text).map_err(|e| format!("the answer doesn't parse: {e}"))
}

impl GithubBranch {
    fn api(&self, path: &str) -> HostResult<Value> {
        parse(&self.gh.run(&["api".to_string(), path.to_string()])?)
    }
}

impl BranchCi for GithubBranch {
    fn read(&self, repo: &str, branch: &str, commits: usize) -> HostResult<BranchRead> {
        let list = self.api(&format!("repos/{repo}/commits?sha={branch}&per_page={commits}"))?;
        let commits: Vec<Commit> = list
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|c| Commit {
                sha: c["sha"].as_str().unwrap_or("").to_string(),
                name: c["commit"]["author"]["name"].as_str().unwrap_or("").to_string(),
                email: c["commit"]["author"]["email"].as_str().unwrap_or("").to_string(),
                message: one_line(c["commit"]["message"].as_str().unwrap_or(""), 200),
            })
            .collect();
        let head = commits.first().map(|c| c.sha.clone()).ok_or_else(|| format!("{repo} has no commits on {branch}"))?;
        let mut checks = vec![];
        for r in self.api(&format!("repos/{repo}/commits/{head}/check-runs?per_page=100"))?["check_runs"].as_array().cloned().unwrap_or_default() {
            let state = match (r["status"].as_str(), r["conclusion"].as_str()) {
                (Some("completed"), Some("success" | "neutral" | "skipped")) => "passed",
                (Some("completed"), Some("cancelled" | "stale")) => "stopped",
                (Some("completed"), _) => "failed",
                _ => "running",
            };
            checks.push(Check { name: r["name"].as_str().unwrap_or("").to_string(), state: state.into(), url: r["html_url"].as_str().map(|s| s.to_string()) });
        }
        for s in self.api(&format!("repos/{repo}/commits/{head}/status"))?["statuses"].as_array().cloned().unwrap_or_default() {
            let state = match s["state"].as_str() {
                Some("success") => "passed",
                Some("failure" | "error") => "failed",
                _ => "running",
            };
            checks.push(Check { name: s["context"].as_str().unwrap_or("").to_string(), state: state.into(), url: s["target_url"].as_str().map(|s| s.to_string()) });
        }
        checks.retain(|c| !c.name.is_empty());
        Ok(BranchRead { head, checks, commits })
    }
}

/// Bitbucket Cloud: `commits/<branch>`, then the head's `statuses`.
pub struct BitbucketBranch {
    pub http: Box<dyn Http>,
    pub api: String,
}

/// "Ann Lee <ann@acme.dev>" → (name, email).
fn split_author(raw: &str) -> (String, String) {
    match raw.rsplit_once('<') {
        Some((n, e)) => (n.trim().to_string(), e.trim_end_matches('>').trim().to_string()),
        None => (raw.trim().to_string(), String::new()),
    }
}

impl BranchCi for BitbucketBranch {
    fn read(&self, repo: &str, branch: &str, commits: usize) -> HostResult<BranchRead> {
        let base = format!("{}/repositories/{repo}", self.api.trim_end_matches('/'));
        let list = self.http.call("GET", &format!("{base}/commits/{branch}?pagelen={commits}"), None)?;
        let commits: Vec<Commit> = list["values"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .take(commits)
            .map(|c| {
                let (name, email) = split_author(c["author"]["raw"].as_str().unwrap_or(""));
                Commit {
                    sha: c["hash"].as_str().unwrap_or("").to_string(),
                    name: c["author"]["user"]["display_name"].as_str().map(|s| s.to_string()).unwrap_or(name),
                    email,
                    message: one_line(c["message"].as_str().unwrap_or(""), 200),
                }
            })
            .collect();
        let head = commits.first().map(|c| c.sha.clone()).ok_or_else(|| format!("{repo} has no commits on {branch}"))?;
        let st = self.http.call("GET", &format!("{base}/commit/{head}/statuses?pagelen=100"), None)?;
        let checks = st["values"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|s| Check {
                name: s["name"].as_str().or(s["key"].as_str()).unwrap_or("").to_string(),
                state: match s["state"].as_str() {
                    Some("SUCCESSFUL") => "passed",
                    Some("FAILED") => "failed",
                    Some("STOPPED") => "stopped",
                    _ => "running",
                }
                .into(),
                url: s["url"].as_str().map(|u| u.to_string()),
            })
            .filter(|c| !c.name.is_empty())
            .collect();
        Ok(BranchRead { head, checks, commits })
    }
}

/// Uses `ci` for every branch on its host on this board (tests).
pub fn install(app: &App, host: &str, ci: Arc<dyn BranchCi>) {
    app.shared.lock().branch_ci.insert(host.to_string(), ci);
}

fn ci_for(app: &App, host: &str) -> HostResult<Arc<dyn BranchCi>> {
    if let Some(c) = app.shared.lock().branch_ci.get(host) {
        return Ok(c.clone());
    }
    match host {
        "github" => {
            let gh = proc::which(&app.cfg.pr.gh).ok_or_else(|| "the gh command isn't installed".to_string())?;
            Ok(Arc::new(GithubBranch { gh: Box::new(crate::prhost::github::GhCli { bin: gh, env: crate::accounts::gh_env(&app.cfg) }) }))
        }
        "bitbucket" => {
            let (email, token) = crate::accounts::board_credentials(app, crate::accounts::Provider::Bitbucket)
                .ok_or_else(|| "Bitbucket isn't connected: connect it in Taskboard ▸ Settings ▸ Accounts".to_string())?;
            Ok(Arc::new(BitbucketBranch { http: Box::new(crate::prhost::bitbucket::BasicHttp::new(&email, &token)), api: app.cfg.pr.bitbucket_api.clone() }))
        }
        other => Err(format!("the board can't watch a branch on {other}")),
    }
}

/// Where a watched project's branch lives: (host, repo, branch).
fn target(app: &App, name: &str, p: &MasterProject) -> HostResult<(String, String, String)> {
    let branch = p.branch.clone().filter(|b| !b.trim().is_empty()).unwrap_or_else(|| "main".into());
    if let (Some(h), Some(r)) = (p.host.clone().filter(|h| !h.is_empty()), p.repo.clone().filter(|r| !r.is_empty())) {
        return Ok((h, r, branch));
    }
    let path = projects::project_path(app, Some(name)).ok().flatten().ok_or_else(|| format!("the board doesn't know where {name} is"))?;
    let git = proc::which("git").ok_or("git isn't installed")?;
    let o = proc::run(&git, &["-C".into(), path, "remote".into(), "get-url".into(), "origin".into()], None, 10.0).map_err(|_| "git didn't answer".to_string())?;
    let (host, repo) = crate::prhost::repo_of_remote(o.stdout.trim()).ok_or_else(|| format!("{name}'s origin isn't on GitHub or Bitbucket; set host and repo in [master.projects.{name}]"))?;
    Ok((p.host.clone().filter(|h| !h.is_empty()).unwrap_or(host), p.repo.clone().filter(|r| !r.is_empty()).unwrap_or(repo), branch))
}

fn watch_state(app: &App) -> Row {
    jloads_obj(app.db.get_setting("master_watch").ok().flatten().as_deref())
}

fn note_watch(app: &App, project: &str, changes: Vec<(&str, Value)>) -> Result<()> {
    let mut all = watch_state(app);
    let mut one = all.get(project).and_then(|v| v.as_object()).cloned().unwrap_or_default();
    for (k, v) in changes {
        if v.is_null() {
            one.remove(k);
        } else {
            one.insert(k.to_string(), v);
        }
    }
    all.insert(project.to_string(), Value::Object(one));
    app.db.set_setting("master_watch", Some(&jdumps(&Value::Object(all))))
}

pub fn bref(id: i64) -> String {
    rf("break", id)
}

fn open_break(app: &App, project: &str) -> Result<Option<Row>> {
    app.db.q1("SELECT * FROM breaks WHERE project = ? AND state = 'open' ORDER BY id DESC LIMIT 1", p![project])
}

pub fn get(app: &App, id: i64) -> Result<Row> {
    match app.db.q1("SELECT * FROM breaks WHERE id = ?", p![id])? {
        Some(b) => Ok(b),
        None => err(404, format!("There's no break {}.", bref(id))),
    }
}

/// Runs on every runner tick: reads each watched branch that's due.
pub fn tick(app: &App) -> Result<()> {
    let every = app.cfg.master.every_mins * 60.0;
    for (name, p) in app.cfg.master.projects.clone() {
        let w = watch_state(app);
        let last = w.get(&name).and_then(|v| v["checked_at"].as_str()).and_then(parse_iso);
        if last.is_some_and(|l| now_ts() - l < every) {
            continue;
        }
        check_project(app, &name, &p)?;
    }
    Ok(())
}

/// Reads one project's branch now and opens, updates or closes its break.
pub fn check_project(app: &App, name: &str, p: &MasterProject) -> Result<()> {
    let read = target(app, name, p).and_then(|(host, repo, branch)| {
        let ci = ci_for(app, &host)?;
        ci.read(&repo, &branch, app.cfg.master.commits.max(1)).map(|r| (host, repo, branch, r))
    });
    let (host, repo, branch, r) = match read {
        Ok(x) => x,
        Err(e) => {
            app.info(format!("master: couldn't read {name}: {e}"));
            return note_watch(app, name, fields!["checked_at" => now_iso(), "error" => e]);
        }
    };
    note_watch(app, name, fields!["checked_at" => now_iso(), "error" => null])?;
    let open = open_break(app, name)?;
    let failed = r.failed();
    if r.green() {
        note_watch(app, name, fields!["green_head" => r.head.clone()])?;
        if let Some(b) = open {
            close(app, &b, &r.head)?;
        }
        return Ok(());
    }
    if failed.is_empty() {
        // Still running (or stopped): wait for the verdict of its checks.
        return Ok(());
    }
    let green = watch_state(app).get(name).and_then(|v| v["green_head"].as_str()).map(|s| s.to_string());
    let suspects = suspects(app, &r, green.as_deref());
    let evidence = evidence(app, name, &host, &repo, &r);
    match open {
        None => {
            let id = app.db.insert(
                "breaks",
                fields!["project" => name, "host" => host, "repo" => repo, "branch" => branch, "state" => "open", "head" => r.head.clone(),
                        "last_head" => r.head.clone(), "green_head" => green, "checks" => jdumps(&json!(failed)), "evidence" => jdumps(&evidence),
                        "suspects" => jdumps(&suspects), "opened_at" => now_iso(), "checked_at" => now_iso()],
            )?;
            app.info(format!("master: {} opened: {branch} on {name} fails {}", bref(id), failed.join(", ")));
            decide_and_act(app, &get(app, id)?)?;
        }
        Some(b) => {
            let moved = b.s("last_head") != Some(r.head.as_str());
            let before = jloads_arr(b.s("suspects"));
            let mut merged = suspects.as_array().cloned().unwrap_or_default();
            for s in &before {
                if !merged.iter().any(|m| m["sha"] == s["sha"]) {
                    merged.push(s.clone());
                }
            }
            let new_suspects = merged.len() != before.len();
            app.db.update(
                "breaks",
                &json!(b.id()),
                fields!["last_head" => r.head.clone(), "checks" => jdumps(&json!(failed)), "evidence" => jdumps(&evidence),
                        "suspects" => jdumps(&Value::Array(merged)), "checked_at" => now_iso()],
            )?;
            let b = get(app, b.id())?;
            let by_person = b.s("verdict_by").is_some_and(|v| !matches!(v, "commits" | "claude" | "fallback"));
            let recheck = app.cfg.master.recheck_mins * 60.0;
            let unsure_due = b.s("verdict") == Some("unsure") && age_secs(b.s("verdict_at")).unwrap_or(f64::MAX) >= recheck;
            if !by_person && ((moved && new_suspects) || unsure_due || b.s("verdict").is_none()) {
                decide_and_act(app, &b)?;
            } else {
                escalate(app, &b)?;
            }
        }
    }
    Ok(())
}

/// The commits since the last green head (the head alone without one), each marked ours or not.
fn suspects(app: &App, r: &BranchRead, green: Option<&str>) -> Value {
    let list: Vec<&Commit> = match green.and_then(|g| r.commits.iter().position(|c| c.sha == g)) {
        Some(i) if i > 0 => r.commits[..i].iter().collect(),
        _ => r.commits.iter().take(1).collect(),
    };
    json!(list
        .iter()
        .map(|c| json!({"sha": c.sha, "name": c.name, "email": c.email, "message": c.message, "ours": is_owners(app, &c.email)}))
        .collect::<Vec<_>>())
}

pub fn is_owners(app: &App, email: &str) -> bool {
    let e = email.trim();
    !e.is_empty() && app.cfg.owner_emails.iter().any(|o| o.trim().eq_ignore_ascii_case(e))
}

/// The failing checks, with their failed steps and tests where the CI can be read.
fn evidence(app: &App, project: &str, host: &str, repo: &str, r: &BranchRead) -> Value {
    let rules = app.cfg.pr.project(Some(project));
    let pr = PrRef { host: host.to_string(), repo: repo.to_string(), num: 0, url: String::new() };
    let checks: Vec<Value> = r
        .checks
        .iter()
        .filter(|c| c.state == "failed")
        .map(|c| {
            let f = crate::prhost::ci::provider_for(app, &rules, c).ok().flatten().and_then(|p| p.failures(&pr, &r.head, c).ok());
            json!({"name": c.name, "url": c.url, "steps": f.as_ref().map(|f| f.steps.clone()).unwrap_or_default(),
                   "tests": f.as_ref().map(|f| f.tests.clone()).unwrap_or_default()})
        })
        .collect();
    json!({"checks": checks, "read_at": now_iso()})
}

/// (verdict, by, why).
fn decide(app: &App, b: &Row) -> (String, String, String) {
    let suspects = jloads_arr(b.s("suspects"));
    let ours: Vec<&Value> = suspects.iter().filter(|s| s["ours"] == true).collect();
    if ours.is_empty() {
        return ("not_ours".into(), "commits".into(), format!("None of the suspect commits ({}) are {}.", shas(&suspects), app.cfg.owners()));
    }
    if app.cfg.master.fault_check {
        if let Some((v, why)) = ask_claude(app, b) {
            return (v, "claude".into(), why);
        }
    }
    if ours.len() == suspects.len() {
        ("ours".into(), "fallback".into(), format!("Every suspect commit ({}) is {}.", shas(&suspects), app.cfg.owners()))
    } else {
        ("unsure".into(), "fallback".into(), format!("{} of the suspect commits ({}) are {}, and the others aren't.", ours.len(), shas(&suspects), app.cfg.owners()))
    }
}

fn shas(list: &[Value]) -> String {
    list.iter().map(|s| s["sha"].as_str().unwrap_or("").chars().take(8).collect::<String>()).collect::<Vec<_>>().join(", ")
}

fn verdict_schema() -> Value {
    json!({"type": "object", "required": ["verdict", "why"], "properties": {
        "verdict": {"enum": ["ours", "not_ours", "unsure"]}, "why": {"type": "string"}}})
}

fn fault_prompt(app: &App, b: &Row) -> String {
    let owner = &app.cfg.owner;
    let ev: Value = serde_json::from_str(b.s("evidence").unwrap_or("{}")).unwrap_or(json!({}));
    let failing: Vec<String> = ev["checks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|c| {
            let mut line = format!("- {}", c["name"].as_str().unwrap_or(""));
            for (k, label) in [("steps", "failed steps"), ("tests", "failing tests")] {
                let items: Vec<&str> = c[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
                if !items.is_empty() {
                    line += &format!("; {label}: {}", items.join(", "));
                }
            }
            line
        })
        .collect();
    let suspects: Vec<String> = jloads_arr(b.s("suspects"))
        .iter()
        .map(|s| {
            format!(
                "- {} by {} <{}>{}: {}",
                s["sha"].as_str().unwrap_or("").chars().take(10).collect::<String>(),
                s["name"].as_str().unwrap_or(""),
                s["email"].as_str().unwrap_or(""),
                if s["ours"] == true { format!(" ({owner}'s)") } else { String::new() },
                s["message"].as_str().unwrap_or("")
            )
        })
        .collect();
    format!(
        "The {branch} branch of {repo} went red. Decide whether it's {owner}'s to fix: whether one of {owner}'s commits \
         among the suspects broke it.\n- ours: the failure clearly comes from a change in one of {owner}'s commits.\n- not_ours: \
         it clearly comes from someone else's commit, or from the CI itself (a flaky test, an outage).\n- unsure: you can't \
         tell. Don't guess.\nGive the reason in one or two plain sentences.\n\nFailing checks:\n{}\n\nSuspect commits, newest \
         first:\n{}",
        failing.join("\n"),
        suspects.join("\n"),
        branch = b.st("branch"),
        repo = b.st("repo")
    )
}

/// Asks a headless `claude -p` for the verdict: (verdict, why), or None when it can't be asked.
fn ask_claude(app: &App, b: &Row) -> Option<(String, String)> {
    let claude = proc::which(&app.cfg.claude)?;
    let m = &app.cfg.master;
    let args: Vec<String> = vec![
        "-p".into(),
        fault_prompt(app, b),
        "--model".into(),
        m.model.clone(),
        "--setting-sources".into(),
        "".into(),
        "--no-session-persistence".into(),
        "--tools".into(),
        "".into(),
        "--strict-mcp-config".into(),
        "--max-budget-usd".into(),
        m.budget_usd.clone(),
        "--json-schema".into(),
        verdict_schema().to_string(),
        "--output-format".into(),
        "json".into(),
    ];
    let out = proc::run(&claude, &args, Some(&app.cfg.data), m.timeout_secs as f64).ok()?;
    let d: Value = serde_json::from_str(out.stdout.trim()).ok()?;
    let v = d.get("structured_output").cloned().or_else(|| d.get("result").and_then(|r| r.as_str()).and_then(|r| serde_json::from_str(r).ok()))?;
    let verdict = v["verdict"].as_str().filter(|x| matches!(*x, "ours" | "not_ours" | "unsure"))?.to_string();
    Some((verdict, one_line(v["why"].as_str().unwrap_or(""), 500)))
}

fn decide_and_act(app: &App, b: &Row) -> Result<()> {
    let (v, by, why) = decide(app, b);
    app.db.tx(|| set_verdict(app, b, &v, &by, &why))
}

/// Records a verdict and acts on it: `ours` gets the fix task and the urgent alert; anything else
/// takes the alert down.
fn set_verdict(app: &App, b: &Row, verdict: &str, by: &str, why: &str) -> Result<()> {
    app.db.update("breaks", &json!(b.id()), fields!["verdict" => verdict, "verdict_by" => by, "verdict_why" => why, "verdict_at" => now_iso()])?;
    let b = get(app, b.id())?;
    if let Some(tid) = b.i("task_id") {
        board::log_event(app, tid, board::BOARD, "status", &format!("{}: {} ({by}): {why}", bref(b.id()), verdict_label(verdict)))?;
    }
    if verdict != "ours" {
        return dispatch::clear_alert_key(app, &alert_key(b.id()));
    }
    let tid = match b.i("task_id").and_then(|t| board::find_task(app, Some(t)).ok().flatten()) {
        Some(t) => t.id(),
        None => {
            let card = crate::ops::new_task(app, &fix_task(app, &b), board::BOARD, Some(&format!("Added to fix {}", bref(b.id()))))?;
            let tid = card["id"].as_i64().unwrap_or(0);
            app.db.update("breaks", &json!(b.id()), fields!["task_id" => tid])?;
            tid
        }
    };
    alert(app, &get(app, b.id())?, tid, false)
}

fn alert_key(id: i64) -> String {
    format!("master:{}", bref(id))
}

fn alert(app: &App, b: &Row, tid: i64, again: bool) -> Result<()> {
    let text = format!(
        "{} is red on {}{}: {} fails. It's yours to fix: {} is on it.",
        b.st("branch"),
        b.st("project"),
        if again { ", still" } else { "" },
        jloads_arr(b.s("checks")).iter().filter_map(|c| c.as_str()).collect::<Vec<_>>().join(", "),
        rf("task", tid)
    );
    if again {
        dispatch::clear_alert_key(app, &alert_key(b.id()))?;
    }
    dispatch::raise(app, &text, Some(tid), None, Some(&alert_key(b.id())), true).map(|_| ())
}

fn fix_task(app: &App, b: &Row) -> Value {
    let checks: Vec<String> = jloads_arr(b.s("checks")).iter().filter_map(|c| c.as_str().map(|s| s.to_string())).collect();
    let ev: Value = serde_json::from_str(b.s("evidence").unwrap_or("{}")).unwrap_or(json!({}));
    let mut detail = vec![format!(
        "{} on {} ({}) went red at {} and {} {} the suspect commits. Fix it on {} so its checks pass again.",
        b.st("branch"),
        b.st("project"),
        b.st("repo"),
        b.st("last_head").chars().take(10).collect::<String>(),
        app.cfg.owners(),
        if b.st("verdict_by") == "claude" { "change is among" } else { "commits are" },
        b.st("branch")
    )];
    detail.push(format!("Why it's {}: {}", app.cfg.owners(), b.st("verdict_why")));
    for c in ev["checks"].as_array().cloned().unwrap_or_default() {
        let mut line = format!("- {} fails", c["name"].as_str().unwrap_or(""));
        if let Some(u) = c["url"].as_str() {
            line += &format!(" ({u})");
        }
        for (k, label) in [("steps", "failed steps"), ("tests", "failing tests")] {
            let items: Vec<&str> = c[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
            if !items.is_empty() {
                line += &format!("; {label}: {}", items.join(", "));
            }
        }
        detail.push(line);
    }
    let suspects: Vec<String> = jloads_arr(b.s("suspects"))
        .iter()
        .map(|s| format!("- {} {} ({})", s["sha"].as_str().unwrap_or("").chars().take(10).collect::<String>(), s["message"].as_str().unwrap_or(""), s["name"].as_str().unwrap_or("")))
        .collect();
    detail.push(format!("Suspect commits:\n{}", suspects.join("\n")));
    json!({"title": short(&format!("Fix red {} on {}: {}", b.st("branch"), b.st("project"), checks.join(", ")), 80), "detail": detail.join("\n\n"),
           "project": b.st("project"), "priority": "high", "origin": {"from": format!("{} ({} is red)", bref(b.id()), b.st("branch")), "by": board::BOARD}})
}

/// An `ours` break whose fix task finished while the branch is still red: raise the alert again,
/// once per `recheck_mins`.
fn escalate(app: &App, b: &Row) -> Result<()> {
    if b.s("verdict") != Some("ours") {
        return Ok(());
    }
    let Some(t) = board::find_task(app, b.i("task_id"))? else { return Ok(()) };
    if t.s("status") != Some("done") {
        return Ok(());
    }
    let recheck = app.cfg.master.recheck_mins * 60.0;
    let since = b.s("escalated_at").or(t.s("finished_at"));
    if age_secs(since).unwrap_or(f64::MAX) < recheck {
        return Ok(());
    }
    app.db.update("breaks", &json!(b.id()), fields!["escalated_at" => now_iso()])?;
    board::log_event(app, t.id(), board::BOARD, "status", &format!("{} is still red after this task finished", b.st("branch")))?;
    alert(app, b, t.id(), true)
}

fn close(app: &App, b: &Row, head: &str) -> Result<()> {
    app.db.update("breaks", &json!(b.id()), fields!["state" => "closed", "closed_at" => now_iso(), "fixed_head" => head])?;
    dispatch::clear_alert_key(app, &alert_key(b.id()))?;
    if let Some(tid) = b.i("task_id") {
        board::log_event(app, tid, board::BOARD, "status", &format!("{} is green again at {}: {} is closed", b.st("branch"), head.chars().take(10).collect::<String>(), bref(b.id())))?;
    }
    app.info(format!("master: {} closed: {} on {} is green", bref(b.id()), b.st("branch"), b.st("project")));
    Ok(())
}

pub fn verdict_label(v: &str) -> &'static str {
    match v {
        "ours" => "yours to fix",
        "not_ours" => "not yours",
        "unsure" => "unsure, so not yours to fix",
        _ => "not decided yet",
    }
}

/// A break as the API answers it.
pub fn dict(app: &App, b: &Row) -> Result<Value> {
    let task = match board::find_task(app, b.i("task_id"))? {
        Some(t) => json!({"ref": rf("task", t.id()), "title": t.v("title"), "status": t.v("status")}),
        None => Value::Null,
    };
    Ok(json!({
        "id": b.id(), "ref": bref(b.id()), "project": b.v("project"), "host": b.v("host"), "repo": b.v("repo"), "branch": b.v("branch"),
        "state": b.v("state"), "head": b.v("head"), "last_head": b.v("last_head"), "green_head": b.v("green_head"), "fixed_head": b.v("fixed_head"),
        "checks": jloads_arr(b.s("checks")), "evidence": serde_json::from_str::<Value>(b.s("evidence").unwrap_or("null")).unwrap_or(Value::Null),
        "suspects": jloads_arr(b.s("suspects")), "verdict": b.v("verdict"), "verdict_label": b.s("verdict").map(verdict_label),
        "verdict_by": b.v("verdict_by"), "verdict_why": b.v("verdict_why"), "verdict_at": b.v("verdict_at"), "task": task,
        "opened_at": b.v("opened_at"), "closed_at": b.v("closed_at"), "checked_at": b.v("checked_at"),
    }))
}

/// `state.master`: the open breaks, for the app's "Master is red" banner.
pub fn banner(app: &App) -> Result<Vec<Value>> {
    let mut out = vec![];
    for b in app.db.q("SELECT * FROM breaks WHERE state = 'open' ORDER BY id", p![])? {
        out.push(dict(app, &b)?);
    }
    Ok(out)
}

/// `GET /master` (`tb master`): the open breaks, then the last ten closed ones, and what's watched.
pub fn list(app: &App) -> Result<Value> {
    let mut open = vec![];
    for b in app.db.q("SELECT * FROM breaks WHERE state = 'open' ORDER BY id", p![])? {
        open.push(dict(app, &b)?);
    }
    let mut closed = vec![];
    for b in app.db.q("SELECT * FROM breaks WHERE state = 'closed' ORDER BY id DESC LIMIT 10", p![])? {
        closed.push(dict(app, &b)?);
    }
    let w = watch_state(app);
    let watched: Vec<Value> = app
        .cfg
        .master
        .projects
        .iter()
        .map(|(name, p)| {
            let s = w.get(name).cloned().unwrap_or(json!({}));
            json!({"project": name, "branch": p.branch.clone().unwrap_or_else(|| "main".into()), "checked_at": s.get("checked_at"),
                   "green_head": s.get("green_head"), "error": s.get("error")})
        })
        .collect();
    Ok(json!({"open": open, "closed": closed, "watched": watched}))
}

/// `POST /master/:ref {verdict, who}` (`tb master M3 ours|not-ours|unsure`): the owner's word.
pub fn override_verdict(app: &App, id: i64, body: &Value) -> Result<Value> {
    let b = get(app, id)?;
    let v = body_str(body, "verdict").to_lowercase().replace('-', "_");
    if !matches!(v.as_str(), "ours" | "not_ours" | "unsure") {
        return err(400, "A verdict is ours, not-ours or unsure.");
    }
    if b.s("state") != Some("open") {
        return err(409, format!("{} is closed: the branch is green again.", bref(id)));
    }
    let who = { let w = one_line(&body_str(body, "who"), 80); if w.is_empty() { app.cfg.owner.clone() } else { w } };
    let why = { let w = one_line(&body_str(body, "why"), 300); if w.is_empty() { format!("{who} said so.") } else { w } };
    set_verdict(app, &b, &v, &who, &why)?;
    dict(app, &get(app, id)?)
}

pub fn route(app: &App, method: &str, rest: &[&str], body: &Value) -> Result<Value> {
    match (method, rest) {
        ("GET", []) => list(app),
        ("POST", ["check"]) => {
            for (name, p) in app.cfg.master.projects.clone() {
                app.db.tx(|| check_project(app, &name, &p))?;
            }
            list(app)
        }
        ("GET", [id]) => dict(app, &get(app, need_ref(&json!(id), "break")?)?),
        ("POST", [id]) => app.db.tx(|| override_verdict(app, need_ref(&json!(id), "break")?, body)),
        _ => err(404, "There's nothing at that address."),
    }
}

/// A branch for tests: answers each read with what it's given.
pub struct FakeBranch {
    pub read: parking_lot::Mutex<BranchRead>,
}

impl BranchCi for FakeBranch {
    fn read(&self, _repo: &str, _branch: &str, _commits: usize) -> HostResult<BranchRead> {
        Ok(self.read.lock().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    struct CannedGh(Mutex<Vec<(String, String)>>);
    impl Gh for CannedGh {
        fn run(&self, args: &[String]) -> HostResult<String> {
            let path = args.get(1).cloned().unwrap_or_default();
            self.0.lock().iter().find(|(p, _)| path.starts_with(p.as_str())).map(|(_, a)| a.clone()).ok_or_else(|| format!("no answer for {path}"))
        }
    }

    #[test]
    fn reads_a_github_branch() {
        let gh = CannedGh(Mutex::new(vec![
            ("repos/a/b/commits?".into(), json!([{"sha": "c2", "commit": {"author": {"name": "Ann", "email": "ann@x.dev"}, "message": "Break it\n\nbody"}},
                                                  {"sha": "c1", "commit": {"author": {"name": "Bo", "email": "bo@x.dev"}, "message": "Fine"}}]).to_string()),
            ("repos/a/b/commits/c2/check-runs".into(), json!({"check_runs": [{"name": "build", "status": "completed", "conclusion": "failure", "html_url": "u"},
                                                                              {"name": "lint", "status": "in_progress"}]}).to_string()),
            ("repos/a/b/commits/c2/status".into(), json!({"statuses": [{"context": "ci/x", "state": "success"}]}).to_string()),
        ]));
        let r = GithubBranch { gh: Box::new(gh) }.read("a/b", "main", 10).unwrap();
        assert_eq!(r.head, "c2");
        assert_eq!(r.commits[0].email, "ann@x.dev");
        assert_eq!(r.commits[0].message, "Break it body");
        assert_eq!(r.failed(), vec!["build"]);
        assert!(!r.green());
        assert_eq!(r.checks.iter().map(|c| c.state.as_str()).collect::<Vec<_>>(), vec!["failed", "running", "passed"]);
    }

    struct CannedHttp(Vec<(String, Value)>);
    impl Http for CannedHttp {
        fn call(&self, _m: &str, url: &str, _b: Option<&Value>) -> HostResult<Value> {
            self.0.iter().find(|(p, _)| url.contains(p.as_str())).map(|(_, v)| v.clone()).ok_or_else(|| format!("no answer for {url}"))
        }
    }

    #[test]
    fn reads_a_bitbucket_branch() {
        let http = CannedHttp(vec![
            ("/commits/main".into(), json!({"values": [{"hash": "h2", "author": {"raw": "Ann Lee <ann@x.dev>"}, "message": "Oops"}]})),
            ("/commit/h2/statuses".into(), json!({"values": [{"name": "Pipeline", "state": "FAILED", "url": "u"}]})),
        ]);
        let r = BitbucketBranch { http: Box::new(http), api: "https://api/2.0".into() }.read("ws/r", "main", 5).unwrap();
        assert_eq!((r.head.as_str(), r.commits[0].name.as_str(), r.commits[0].email.as_str()), ("h2", "Ann Lee", "ann@x.dev"));
        assert_eq!(r.failed(), vec!["Pipeline"]);
    }
}
