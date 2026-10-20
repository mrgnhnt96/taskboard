//! Master breaks: an optional watch on each project's default branch. When its CI goes red the board
//! opens a break, `M<n>`, works out whether it's the owner's to fix, and closes it once the branch is
//! green again.
//!
//! # The flow
//!
//! - **Intake.** Every `[master] every_mins` (5) the board reads each project in `[master.projects]`:
//!   the branch's head commit, its checks and its last `commits` commits ([`BranchCi`]: GitHub
//!   through `gh api`, Bitbucket through REST 2.0). A posted build (`POST /master-builds`, a stream
//!   `build`) reads the branch at once and counts too, whichever commit it built. Builds are kept per
//!   pipeline (a check's name, a posted build's `pipeline`): a red one stays until that same pipeline
//!   passes at or after its commit, so another pipeline's green, or a head still running, doesn't
//!   clear it; a red older than its pipeline's last green is stale. Any pipeline red opens a break
//!   (one per project at a time) or keeps it open; it closes once none is.
//! - **Investigate.** The suspects are the commits from the newest red one back to the last green head
//!   the board saw: the whole range from the host (git's `green..red`) when it's further back than the commits read, every
//!   commit read when the board never saw it green. Each is the owner's when its author's email is in
//!   `owner_emails`.
//!   The evidence is the failing checks (with their failed steps and tests where the CI can be read)
//!   and the suspects.
//! - **Decide.** No suspect of the owner's: `not_ours`. Otherwise a headless `claude -p` reads the
//!   evidence and answers `ours`, `not_ours` or `unsure` with a reason, a title and proof links
//!   (`[master] fault_check`). Its `not_ours` needs an http(s) proof link, else it's `unsure`. When the
//!   check can't give a verdict (claude missing, a timeout, a crash, an answer it can't read) the break
//!   is `unsure` with the error kept (`check_error`), so no fix starts. With the check off, every
//!   suspect being the owner's makes it `ours`, else `unsure`.
//! - **Act.** Only `ours` gets a fix task (high priority, in the project, with a new Jira ticket when
//!   Jira is on, started at once: `[master] start_fix`) and an urgent alert (`master:M<n>`, with the
//!   task). `unsure` isn't the owner's to fix.
//! - **Follow.** The urgent alert follows the fix task (`fix_state`): it needs the owner (its
//!   question), it stopped, it hasn't started after 5 minutes, it finished without a fix PR, its PR's
//!   agent didn't wake, its PR's checks fail, its PR waits for the owner's review, or the branch is
//!   still red 2 hours on. Each new state raises the alert again, so it notifies; the fix task's own
//!   alerts wait behind the urgent one, so this is how its question reaches the owner.
//! - **Override.** `tb master M<n> ours|not-ours|unsure` is the owner's word: it acts the same way and
//!   is never re-decided. `not-ours` takes at least one proof link (`--proof`: a build, an issue).
//! - **Re-check.** While a break is open, a newer red commit brings new suspects and decides again (unless a
//!   person set the verdict); an `unsure` one is decided again after `recheck_mins`; an `ours` one
//!   whose fix task has finished while the branch is still red raises the urgent alert again.
//! - **Cover.** Before a break of the owner's gets a new fix task, the board looks for a fix already
//!   in flight on the same project and branch (the break's own task first, then the other breaks'):
//!   a fix task covers the break when it isn't done, or its PR is still open, or its PR merged after
//!   the failing build was queued (that build couldn't hold the fix). The build's time is when its
//!   failed checks were queued, else when the board first saw that head red. Only when nothing
//!   covers it does the break get a new fix task. A fix that finished without a PR, or whose PR was
//!   declined, hands its break to a covering task (logged there) and raises no alert again.
//! - **Close.** Green again: the break closes, the alert clears and the fix task's log says so.
//!
//! The branch is `[master.projects.<name>] branch`, else the one the import carried over, else the
//! repo's default branch (from the host, else the clone's `origin/HEAD`), else `main`.
//!
//! The app shows a "Master is red" banner line for each open break that's the owner's (`state.master`).
//!
//! # The `breaks` table
//!
//! | Column | |
//! |---|---|
//! | `id` | the break's number: `M<id>` |
//! | `project`, `host`, `repo`, `branch` | what went red: the board's project, `github` or `bitbucket`, `owner/name`, the branch |
//! | `state` | `open` or `closed` |
//! | `head` | the first red commit seen; `last_head` the newest red commit (of any pipeline still red) |
//! | `green_head` | the last green head before it broke (NULL when the board never saw one); `fixed_head` the green head that closed it |
//! | `checks` | JSON list of the pipelines still red |
//! | `evidence` | JSON `{checks: [{name, url, sha?, steps, tests}], read_at}`: the head's failed checks, then each other red pipeline with its commit |
//! | `suspects` | JSON `[{sha, name, email, message, ours}]`, newest first |
//! | `verdict` | `ours`, `not_ours`, `unsure`, or NULL before it's decided |
//! | `verdict_by` | `commits` (no suspect is the owner's), `claude`, `fallback`, or the person who overrode it |
//! | `verdict_why`, `verdict_at` | the reason, and when |
//! | `verdict_title` | the cause in a few plain words (80 characters at most), from the fault check or the person; NULL when nobody gave one |
//! | `task_id` | the fix task (`ours` only) |
//! | `proof` | JSON list of the proof links: a person's with `not-ours`, or the fault check's |
//! | `check_error` | why the fault check couldn't give a verdict (the break is then `unsure`) |
//! | `alert_state` | the fix's state the urgent alert last said (`fix_state`) |
//! | `escalated_at` | when the urgent alert was raised again after the fix task finished |
//! | `queued_at` | when the failing build on `last_head` was queued (NULL when the host doesn't say); `seen_at` when the board first saw `last_head` red |
//! | `opened_at`, `closed_at`, `checked_at` | ISO times |
//!
//! The setting `master_watch` keeps, per project, `{checked_at, green_head, branch?, error?, build?,
//! pipelines}`, `pipelines` being `{<name>: {red?: {sha, url, build_id?, queued_at}, green?: sha}}`.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::prhost::bitbucket::Http;
use crate::prhost::github::Gh;
use crate::prhost::{Check, HostResult, PrRef};
use crate::util::*;
use crate::prcmds::{REASON_MAX, REASON_MIN};
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

/// Columns added to `breaks` since it was made: (table, column, type).
pub const ADDED: &[(&str, &str, &str)] =
    &[("breaks", "proof", "TEXT"), ("breaks", "queued_at", "TEXT"), ("breaks", "seen_at", "TEXT"), ("breaks", "verdict_title", "TEXT"),
      ("breaks", "check_error", "TEXT"), ("breaks", "alert_state", "TEXT")];

/// A break's cause title is at most this long, as `tb pr not-ours --title` is.
pub const TITLE_MAX: usize = crate::prcmds::TITLE_MAX;

/// At most this many suspects are kept for one break.
const SUSPECTS_MAX: usize = 100;

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
    /// Start the fix task at once (in a new terminal) rather than leaving it queued.
    pub start_fix: bool,
    /// Ask for a new Jira ticket for the fix task (when Jira is on).
    pub fix_ticket: bool,
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
            start_fix: true,
            fix_ticket: true,
            projects: BTreeMap::new(),
        }
    }
}

/// One watched project. Every key is optional.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MasterProject {
    /// The branch (default: the repo's default branch).
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

/// One read of a branch: its head, the head's checks, its latest commits (newest first), and when
/// the head's failed checks were queued (the earliest, ISO; None when the host doesn't say).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BranchRead {
    pub head: String,
    pub checks: Vec<Check>,
    pub commits: Vec<Commit>,
    pub queued_at: Option<String>,
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
    /// The commits after `base` up to `head`, newest first (git's `base..head`).
    fn since(&self, _repo: &str, _base: &str, _head: &str) -> HostResult<Vec<Commit>> {
        Err("this host can't list a range of commits".into())
    }
    /// The repo's default branch.
    fn default_branch(&self, _repo: &str) -> HostResult<String> {
        Err("this host can't say the default branch".into())
    }
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

fn gh_commit(c: &Value) -> Commit {
    Commit {
        sha: c["sha"].as_str().unwrap_or("").to_string(),
        name: c["commit"]["author"]["name"].as_str().unwrap_or("").to_string(),
        email: c["commit"]["author"]["email"].as_str().unwrap_or("").to_string(),
        message: one_line(c["commit"]["message"].as_str().unwrap_or(""), 200),
    }
}

impl BranchCi for GithubBranch {
    fn since(&self, repo: &str, base: &str, head: &str) -> HostResult<Vec<Commit>> {
        // Oldest first, up to 250.
        let v = self.api(&format!("repos/{repo}/compare/{base}...{head}"))?;
        let mut list: Vec<Commit> = v["commits"].as_array().cloned().unwrap_or_default().iter().map(gh_commit).collect();
        list.reverse();
        Ok(list)
    }
    fn default_branch(&self, repo: &str) -> HostResult<String> {
        self.api(&format!("repos/{repo}"))?["default_branch"].as_str().filter(|b| !b.is_empty()).map(|b| b.to_string()).ok_or_else(|| format!("{repo} has no default branch"))
    }
    fn read(&self, repo: &str, branch: &str, commits: usize) -> HostResult<BranchRead> {
        let list = self.api(&format!("repos/{repo}/commits?sha={branch}&per_page={commits}"))?;
        let commits: Vec<Commit> = list.as_array().cloned().unwrap_or_default().iter().map(gh_commit).collect();
        let head = commits.first().map(|c| c.sha.clone()).ok_or_else(|| format!("{repo} has no commits on {branch}"))?;
        let mut checks = vec![];
        let mut queued = vec![];
        for r in self.api(&format!("repos/{repo}/commits/{head}/check-runs?per_page=100"))?["check_runs"].as_array().cloned().unwrap_or_default() {
            let state = match (r["status"].as_str(), r["conclusion"].as_str()) {
                (Some("completed"), Some("success" | "neutral" | "skipped")) => "passed",
                (Some("completed"), Some("cancelled" | "stale")) => "stopped",
                (Some("completed"), _) => "failed",
                _ => "running",
            };
            if state == "failed" {
                queued.extend(r["started_at"].as_str().map(|s| s.to_string()));
            }
            checks.push(Check { name: r["name"].as_str().unwrap_or("").to_string(), state: state.into(), url: r["html_url"].as_str().map(|s| s.to_string()), at: r["completed_at"].as_str().or(r["started_at"].as_str()).map(|s| s.to_string()) });
        }
        for s in self.api(&format!("repos/{repo}/commits/{head}/status"))?["statuses"].as_array().cloned().unwrap_or_default() {
            let state = match s["state"].as_str() {
                Some("success") => "passed",
                Some("failure" | "error") => "failed",
                _ => "running",
            };
            if state == "failed" {
                queued.extend(s["created_at"].as_str().map(|s| s.to_string()));
            }
            checks.push(Check { name: s["context"].as_str().unwrap_or("").to_string(), state: state.into(), url: s["target_url"].as_str().map(|s| s.to_string()), at: s["updated_at"].as_str().or(s["created_at"].as_str()).map(|s| s.to_string()) });
        }
        checks.retain(|c| !c.name.is_empty());
        Ok(BranchRead { head, checks, commits, queued_at: earliest(&queued) })
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

fn bb_commit(c: &Value) -> Commit {
    let (name, email) = split_author(c["author"]["raw"].as_str().unwrap_or(""));
    Commit {
        sha: c["hash"].as_str().unwrap_or("").to_string(),
        name: c["author"]["user"]["display_name"].as_str().map(|s| s.to_string()).unwrap_or(name),
        email,
        message: one_line(c["message"].as_str().unwrap_or(""), 200),
    }
}

impl BranchCi for BitbucketBranch {
    fn since(&self, repo: &str, base: &str, head: &str) -> HostResult<Vec<Commit>> {
        let root = format!("{}/repositories/{repo}", self.api.trim_end_matches('/'));
        let mut url = format!("{root}/commits/{head}?exclude={base}&pagelen=100");
        let mut out = vec![];
        for _ in 0..5 {
            let page = self.http.call("GET", &url, None)?;
            out.extend(page["values"].as_array().cloned().unwrap_or_default().iter().map(bb_commit));
            match page["next"].as_str() {
                Some(n) if out.len() < SUSPECTS_MAX => url = n.to_string(),
                _ => break,
            }
        }
        Ok(out)
    }
    fn default_branch(&self, repo: &str) -> HostResult<String> {
        let v = self.http.call("GET", &format!("{}/repositories/{repo}", self.api.trim_end_matches('/')), None)?;
        v["mainbranch"]["name"].as_str().filter(|b| !b.is_empty()).map(|b| b.to_string()).ok_or_else(|| format!("{repo} has no main branch"))
    }
    fn read(&self, repo: &str, branch: &str, commits: usize) -> HostResult<BranchRead> {
        let base = format!("{}/repositories/{repo}", self.api.trim_end_matches('/'));
        let list = self.http.call("GET", &format!("{base}/commits/{branch}?pagelen={commits}"), None)?;
        let commits: Vec<Commit> = list["values"].as_array().cloned().unwrap_or_default().iter().take(commits).map(bb_commit).collect();
        let head = commits.first().map(|c| c.sha.clone()).ok_or_else(|| format!("{repo} has no commits on {branch}"))?;
        let st = self.http.call("GET", &format!("{base}/commit/{head}/statuses?pagelen=100"), None)?;
        let queued: Vec<String> = st["values"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter(|s| s["state"] == "FAILED")
            .filter_map(|s| s["created_on"].as_str().map(|t| t.to_string()))
            .collect();
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
                at: s["updated_on"].as_str().or(s["created_on"].as_str()).map(|t| t.to_string()),
            })
            .filter(|c| !c.name.is_empty())
            .collect();
        Ok(BranchRead { head, checks, commits, queued_at: earliest(&queued) })
    }
}

/// The earliest of these host times, as the board's ISO time.
fn earliest(times: &[String]) -> Option<String> {
    times.iter().filter_map(|t| parse_iso(t)).min_by(|a, b| a.total_cmp(b)).map(iso)
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

/// Where a watched project's repo lives: (host, repo).
fn target(app: &App, name: &str, p: &MasterProject) -> HostResult<(String, String)> {
    if let (Some(h), Some(r)) = (p.host.clone().filter(|h| !h.is_empty()), p.repo.clone().filter(|r| !r.is_empty())) {
        return Ok((h, r));
    }
    let path = projects::project_path(app, Some(name)).ok().flatten().ok_or_else(|| format!("the board doesn't know where {name} is"))?;
    let git = proc::which("git").ok_or("git isn't installed")?;
    let o = proc::run(&git, &["-C".into(), path, "remote".into(), "get-url".into(), "origin".into()], None, 10.0).map_err(|_| "git didn't answer".to_string())?;
    let (host, repo) = crate::prhost::repo_of_remote(o.stdout.trim()).ok_or_else(|| format!("{name}'s origin isn't on GitHub or Bitbucket; set host and repo in [master.projects.{name}]"))?;
    Ok((p.host.clone().filter(|h| !h.is_empty()).unwrap_or(host), p.repo.clone().filter(|r| !r.is_empty()).unwrap_or(repo)))
}

/// The branch set for a project, or the one the board found (or the import carried over) before.
fn known_branch(app: &App, name: &str, p: &MasterProject) -> Option<String> {
    p.branch.clone().filter(|b| !b.trim().is_empty()).or_else(|| watch_state(app).get(name).and_then(|v| v["branch"].as_str()).filter(|b| !b.is_empty()).map(|b| b.to_string()))
}

/// The clone's `origin/HEAD` (`main` for `origin/main`), when it has one.
fn local_default(app: &App, name: &str) -> Option<String> {
    let path = projects::project_path(app, Some(name)).ok().flatten()?;
    let git = proc::which("git")?;
    let o = proc::run(&git, &["-C".into(), path, "symbolic-ref".into(), "--short".into(), "refs/remotes/origin/HEAD".into()], None, 10.0).ok()?;
    let b = o.stdout.trim().strip_prefix("origin/")?.to_string();
    (o.code == Some(0) && !b.is_empty()).then_some(b)
}

/// The branch to watch: the one set or found before, else the repo's default branch (kept for next
/// time), else `main`.
fn branch_for(app: &App, name: &str, p: &MasterProject, ci: &dyn BranchCi, repo: &str) -> Result<String> {
    if let Some(b) = known_branch(app, name, p) {
        return Ok(b);
    }
    match ci.default_branch(repo).ok().or_else(|| local_default(app, name)) {
        Some(b) => {
            note_watch(app, name, fields!["branch" => b.clone()])?;
            Ok(b)
        }
        None => Ok("main".into()),
    }
}

/// The import's word on a project's branch: the old board's, unless one is set or already found.
pub fn carry_branch(app: &App, project: &str, branch: &str) -> Result<()> {
    if branch.trim().is_empty() || app.cfg.master.projects.get(project).and_then(|p| p.branch.as_ref()).is_some_and(|b| !b.trim().is_empty()) {
        return Ok(());
    }
    if watch_state(app).get(project).and_then(|v| v["branch"].as_str()).is_some() {
        return Ok(());
    }
    note_watch(app, project, fields!["branch" => branch.trim()])
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
    for b in app.db.q("SELECT * FROM breaks WHERE state = 'open' AND verdict = 'ours' ORDER BY id", p![])? {
        let made = app.db.tx(|| escalate(app, &b))?;
        start_fix(app, made)?;
        app.db.tx(|| follow(app, &get(app, b.id())?, false))?;
    }
    Ok(())
}

/// Reads one project's branch now and opens, updates or closes its break.
pub fn check_project(app: &App, name: &str, p: &MasterProject) -> Result<()> {
    check_with(app, name, p, None)
}

/// One sha names the other (a build may post a short one).
fn same_sha(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && (a.starts_with(b) || b.starts_with(a))
}

/// `a` is the same commit as `b` or newer on the branch: by their places in the commits read
/// (newest first; one that isn't among them is older than one that is), else by the host's range.
fn at_or_after(ci: &dyn BranchCi, repo: &str, commits: &[Commit], a: &str, b: &str) -> bool {
    if same_sha(a, b) {
        return true;
    }
    let at = |sha: &str| commits.iter().position(|c| same_sha(&c.sha, sha));
    match (at(a), at(b)) {
        (Some(x), Some(y)) => x <= y,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => ci.since(repo, b, a).is_ok_and(|l| !l.is_empty()),
    }
}

/// A build's state as red (true) or green (false); None while it runs, or for one the board can't read.
fn build_red(state: &str) -> Option<bool> {
    match state.to_lowercase().as_str() {
        "failed" | "failure" | "error" | "broken" => Some(true),
        "passed" | "success" | "successful" => Some(false),
        _ => None,
    }
}

/// Each pipeline's builds on the branch (`master_watch.<project>.pipelines`): `{red?: {sha, url,
/// build_id, queued_at}, green?}`. A red stays until the same pipeline passes at or after its
/// commit; a red older than the pipeline's last green is stale; a newer red takes an older one's place.
struct Pipelines<'a> {
    ci: &'a dyn BranchCi,
    repo: &'a str,
    commits: &'a [Commit],
    all: serde_json::Map<String, Value>,
    /// The newest commit a green cleared a red at, in this read.
    cleared: Option<String>,
}

impl Pipelines<'_> {
    fn note(&mut self, pipeline: &str, sha: &str, red: bool, build: Value) {
        if pipeline.is_empty() || sha.is_empty() {
            return;
        }
        let (ci, repo, commits) = (self.ci, self.repo, self.commits);
        let after = |a: &str, b: &str| at_or_after(ci, repo, commits, a, b);
        let one = self.all.entry(pipeline.to_string()).or_insert_with(|| json!({}));
        let red_sha = one["red"]["sha"].as_str().map(|s| s.to_string());
        let green_sha = one["green"].as_str().map(|s| s.to_string());
        if red {
            let stale = green_sha.as_deref().is_some_and(|g| !same_sha(g, sha) && after(g, sha));
            if !stale && red_sha.as_deref().is_none_or(|r| after(sha, r)) {
                let mut b = build;
                b["sha"] = json!(sha);
                one["red"] = b;
            }
            return;
        }
        if green_sha.as_deref().is_none_or(|g| after(sha, g)) {
            one["green"] = json!(sha);
        }
        if red_sha.as_deref().is_some_and(|r| after(sha, r)) {
            if let Some(o) = one.as_object_mut() {
                o.remove("red");
            }
            if self.cleared.as_deref().is_none_or(|c| after(sha, c)) {
                self.cleared = Some(sha.to_string());
            }
        }
    }
    /// The pipelines still red: (name, red build).
    fn reds(&self) -> Vec<(String, Value)> {
        self.all.iter().filter(|(_, v)| v["red"].is_object()).map(|(k, v)| (k.clone(), v["red"].clone())).collect()
    }
    /// The newest red build.
    fn newest_red(&self) -> Option<Value> {
        let mut out: Option<Value> = None;
        for (_, r) in self.reds() {
            if out.as_ref().is_none_or(|o| at_or_after(self.ci, self.repo, self.commits, r["sha"].as_str().unwrap_or(""), o["sha"].as_str().unwrap_or(""))) {
                out = Some(r);
            }
        }
        out
    }
}

/// A build's time (an ISO string or epoch seconds) as the board's ISO time.
fn build_ts(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => n.as_f64().map(iso),
        Value::String(s) => parse_iso(s).map(iso),
        _ => None,
    }
}

/// [`check_project`], with a build that was just posted: its pipeline's red or green counts as
/// the head's checks do.
fn check_with(app: &App, name: &str, p: &MasterProject, posted: Option<&Value>) -> Result<()> {
    let found = target(app, name, p).and_then(|(host, repo)| Ok((ci_for(app, &host)?, host, repo)));
    let read = match found {
        Ok((ci, host, repo)) => {
            let branch = branch_for(app, name, p, ci.as_ref(), &repo)?;
            ci.read(&repo, &branch, app.cfg.master.commits.max(1)).map(|r| (ci, host, repo, branch, r))
        }
        Err(e) => Err(e),
    };
    let (ci, host, repo, branch, r) = match read {
        Ok(x) => x,
        Err(e) => {
            app.info(format!("master: couldn't read {name}: {e}"));
            return note_watch(app, name, fields!["checked_at" => now_iso(), "error" => e]);
        }
    };
    note_watch(app, name, fields!["checked_at" => now_iso(), "error" => null])?;
    let open = open_break(app, name)?;
    let mut pipes = Pipelines {
        ci: ci.as_ref(),
        repo: &repo,
        commits: &r.commits,
        all: watch_state(app).get(name).and_then(|v| v["pipelines"].as_object()).cloned().unwrap_or_default(),
        cleared: None,
    };
    if let Some(b) = posted {
        let pipeline = Some(body_str(b, "pipeline")).filter(|s| !s.is_empty()).unwrap_or_else(|| "build".into());
        if let Some(red) = build_red(&body_str(b, "state")) {
            let build = json!({"url": b["url"], "build_id": b["build_id"], "queued_at": build_ts(&b["ts"])});
            pipes.note(&pipeline, &body_str(b, "sha"), red, build);
        }
    }
    for c in &r.checks {
        if let Some(red) = build_red(&c.state) {
            pipes.note(&c.name, &r.head, red, json!({"url": c.url, "queued_at": r.queued_at}));
        }
    }
    let reds = pipes.reds();
    let newest = pipes.newest_red();
    let cleared = pipes.cleared.clone();
    note_watch(app, name, fields!["pipelines" => Value::Object(pipes.all)])?;
    let Some(newest) = newest else {
        if r.green() {
            note_watch(app, name, fields!["green_head" => r.head.clone()])?;
        }
        // Closes once every pipeline that failed has passed again; a head still running with
        // nothing cleared waits for its checks.
        if let (Some(b), Some(at)) = (open, cleared.or_else(|| r.green().then(|| r.head.clone()))) {
            close(app, &b, &at)?;
        }
        return Ok(());
    };
    let red_head = newest["sha"].as_str().unwrap_or(&r.head).to_string();
    let mut failed: Vec<String> = reds.iter().map(|(n, _)| n.clone()).collect();
    failed.sort();
    let queued_at = match &r.queued_at {
        Some(q) if same_sha(&red_head, &r.head) => Some(q.clone()),
        _ => newest["queued_at"].as_str().map(|s| s.to_string()),
    };
    let green = watch_state(app).get(name).and_then(|v| v["green_head"].as_str()).map(|s| s.to_string());
    let suspects = suspects(app, name, ci.as_ref(), &repo, &r, &red_head, green.as_deref());
    let evidence = evidence(app, name, &host, &repo, &r, &red_head, &reds);
    match open {
        None => {
            let id = app.db.insert(
                "breaks",
                fields!["project" => name, "host" => host, "repo" => repo, "branch" => branch, "state" => "open", "head" => red_head.clone(),
                        "last_head" => red_head.clone(), "green_head" => green, "checks" => jdumps(&json!(failed)), "evidence" => jdumps(&evidence),
                        "suspects" => jdumps(&suspects), "queued_at" => queued_at, "seen_at" => now_iso(),
                        "opened_at" => now_iso(), "checked_at" => now_iso()],
            )?;
            app.info(format!("master: {} opened: {branch} on {name} fails {}", bref(id), failed.join(", ")));
            decide_and_act(app, &get(app, id)?)?;
        }
        Some(b) => {
            let moved = b.s("last_head") != Some(red_head.as_str());
            let before = jloads_arr(b.s("suspects"));
            let mut merged = suspects.as_array().cloned().unwrap_or_default();
            for s in &before {
                if !merged.iter().any(|m| m["sha"] == s["sha"]) {
                    merged.push(s.clone());
                }
            }
            let new_suspects = merged.len() != before.len();
            let mut changes = fields!["last_head" => red_head.clone(), "checks" => jdumps(&json!(failed)), "evidence" => jdumps(&evidence),
                                      "suspects" => jdumps(&Value::Array(merged)), "checked_at" => now_iso()];
            if moved || queued_at.is_some() {
                changes.push(("queued_at", json!(queued_at)));
            }
            if moved {
                changes.push(("seen_at", json!(now_iso())));
            }
            app.db.update("breaks", &json!(b.id()), changes)?;
            let b = get(app, b.id())?;
            let by_person = b.s("verdict_by").is_some_and(|v| !matches!(v, "commits" | "claude" | "fallback"));
            let recheck = app.cfg.master.recheck_mins * 60.0;
            let unsure_due = b.s("verdict") == Some("unsure") && age_secs(b.s("verdict_at")).unwrap_or(f64::MAX) >= recheck;
            if !by_person && ((moved && new_suspects) || unsure_due || b.s("verdict").is_none()) {
                decide_and_act(app, &b)?;
            } else {
                let made = app.db.tx(|| escalate(app, &b))?;
                start_fix(app, made)?;
            }
        }
    }
    Ok(())
}

/// The commits from the newest red one back to the last green head, each marked ours or not: from
/// the commits read when both are among them, else the whole range from the host; every commit read
/// from the red one on when there's no green head (or the host can't give the range).
fn suspect_commits(ci: &dyn BranchCi, repo: &str, r: &BranchRead, red: &str, green: Option<&str>) -> Vec<Commit> {
    let read: Vec<Commit> = match r.commits.iter().position(|c| same_sha(&c.sha, red)) {
        Some(j) => r.commits[j..].to_vec(),
        None => vec![],
    };
    let mut list = match green {
        Some(g) => match read.iter().position(|c| same_sha(&c.sha, g)) {
            Some(i) => read[..i.max(1)].to_vec(),
            None => ci.since(repo, g, red).ok().filter(|l| !l.is_empty()).unwrap_or_else(|| read.clone()),
        },
        None => read.clone(),
    };
    if list.is_empty() {
        list = vec![Commit { sha: red.to_string(), ..Default::default() }];
    }
    list.truncate(SUSPECTS_MAX);
    list
}

fn suspects(app: &App, project: &str, ci: &dyn BranchCi, repo: &str, r: &BranchRead, red: &str, green: Option<&str>) -> Value {
    json!(suspect_commits(ci, repo, r, red, green)
        .iter()
        .map(|c| json!({"sha": c.sha, "name": c.name, "email": c.email, "message": c.message, "ours": is_owners(app, project, &c.email)}))
        .collect::<Vec<_>>())
}

/// A commit's email is the owner's by the same rule as the owner's builds (`prbuilds::owner_author`:
/// `owner_emails`, `[reviewers] me` and their roster aliases). With none of those set, the project's
/// own `git config user.email` stands in.
pub fn is_owners(app: &App, project: &str, email: &str) -> bool {
    let e = email.trim();
    if e.is_empty() {
        return false;
    }
    let set = app.cfg.owner_emails.iter().chain(app.cfg.reviewers.me.iter()).any(|s| !s.trim().is_empty());
    if set {
        return crate::prbuilds::owner_author(app, e).unwrap_or(false);
    }
    git_email(app, project).is_some_and(|g| g.eq_ignore_ascii_case(e))
}

fn git_email(app: &App, project: &str) -> Option<String> {
    let path = crate::projects::project_path(app, Some(project)).ok().flatten()?;
    let git = crate::proc::which("git")?;
    let out = crate::proc::run(&git, &["-C".into(), path, "config".into(), "user.email".into()], None, 5.0).ok()?;
    let email = out.stdout.trim().to_string();
    (out.code == Some(0) && !email.is_empty()).then_some(email)
}

/// The failing checks, with their failed steps and tests where the CI can be read: the head's, then
/// each other pipeline still red on an older commit (with that commit and its build's link).
fn evidence(app: &App, project: &str, host: &str, repo: &str, r: &BranchRead, red: &str, reds: &[(String, Value)]) -> Value {
    let rules = app.cfg.pr.project(Some(project));
    let pr = PrRef { host: host.to_string(), repo: repo.to_string(), num: 0, url: String::new() };
    let mut checks: Vec<Value> = r
        .checks
        .iter()
        .filter(|c| c.state == "failed")
        .map(|c| {
            let f = crate::prhost::ci::provider_for(app, &rules, c).ok().flatten().and_then(|p| p.failures(&pr, &r.head, c).ok());
            json!({"name": c.name, "url": c.url, "steps": f.as_ref().map(|f| f.steps.clone()).unwrap_or_default(),
                   "tests": f.as_ref().map(|f| f.tests.clone()).unwrap_or_default()})
        })
        .collect();
    for (name, b) in reds {
        if !checks.iter().any(|c| c["name"] == json!(name)) {
            checks.push(json!({"name": name, "url": b["url"], "sha": b["sha"], "steps": [], "tests": []}));
        }
    }
    let mut out = json!({"checks": checks, "read_at": now_iso()});
    if let Some(b) = posted_build(app, project, red) {
        if let Some(url) = b["url"].as_str() {
            for c in out["checks"].as_array_mut().into_iter().flatten().filter(|c| c["url"].as_str().is_none_or(|u| u.is_empty())) {
                c["url"] = json!(url);
            }
        }
        out["build"] = b;
    }
    out
}

/// The last build posted for this project (`POST /master-builds` or a stream `build`), when it's of
/// this commit (or doesn't say which commit it built).
fn posted_build(app: &App, project: &str, sha: &str) -> Option<Value> {
    let b = watch_state(app).get(project)?.get("build")?.clone();
    let built = b["sha"].as_str().unwrap_or("");
    (built.is_empty() || sha.is_empty() || same_sha(built, sha)).then_some(b)
}

/// The watched project a posted build is of: `project` by name, else the repo in its `url` (or
/// `repo`) matching a project's repo or name, else `pipeline` as a project's name, else the only
/// watched project.
fn project_of_build(app: &App, body: &Value) -> Option<(String, MasterProject)> {
    let projects = &app.cfg.master.projects;
    let by_name = |n: &str| projects.iter().find(|(k, _)| k.eq_ignore_ascii_case(n.trim())).map(|(k, p)| (k.clone(), p.clone()));
    if let Some(found) = Some(body_str(body, "project")).filter(|n| !n.is_empty()).and_then(|n| by_name(&n)) {
        return Some(found);
    }
    let re = regex::Regex::new(r"(?:github\.com|bitbucket\.org)/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)").unwrap();
    let repo = re.captures(&body_str(body, "url")).map(|c| c[1].to_string()).unwrap_or_else(|| body_str(body, "repo"));
    if !repo.is_empty() {
        let short = repo.rsplit('/').next().unwrap_or(&repo).to_string();
        let found = projects.iter().find(|(name, p)| {
            let theirs = p.repo.clone().filter(|r| !r.is_empty()).or_else(|| target(app, name, p).ok().map(|(_, r)| r));
            theirs.is_some_and(|r| r.eq_ignore_ascii_case(&repo)) || name.eq_ignore_ascii_case(&short)
        });
        if let Some((k, p)) = found {
            return Some((k.clone(), p.clone()));
        }
    }
    if let Some(found) = Some(body_str(body, "pipeline")).filter(|n| !n.is_empty()).and_then(|n| by_name(&n)) {
        return Some(found);
    }
    (projects.len() == 1).then(|| projects.iter().next().map(|(k, p)| (k.clone(), p.clone()))).flatten()
}

/// `POST /master-builds {pipeline, build_id, url, sha, state, ts, last_green}` (the old board's
/// listener): a default-branch build started or finished. It's a feed event, and the project's
/// branch is read now (as `POST /master/check` reads it), so a red build opens its break at once;
/// the build's `state`, `build_id` and `url` go into the break's evidence (`build`; a failing check
/// without a link gets the build's).
pub fn posted(app: &App, body: &Value) -> Result<Value> {
    crate::feed::note_event(app, "master-build", body, None)?;
    let Some((name, p)) = project_of_build(app, body) else {
        return Ok(json!({"ok": true, "project": null, "checked": false, "why": "no project in [master.projects] matches this build"}));
    };
    record_build(app, &name, &p, body)
}

/// A stream `build` event (protocol v1) whose repo and branch are a watched project's: the same as
/// `POST /master-builds` with its `pipeline`, `url`, `sha` (its `head`), `state` and `ts` (its `at`).
/// `build_id` is the event's, else the last number in its `url`; the watch works out the last green
/// head itself. None when no project in `[master.projects]` watches that branch.
pub fn stream_build(app: &App, ev: &Value) -> Result<Option<Value>> {
    let (repo, branch) = (body_str(ev, "repo"), body_str(ev, "branch"));
    if repo.is_empty() || branch.is_empty() {
        return Ok(None);
    }
    let watched = app.cfg.master.projects.iter().find(|(name, p)| {
        let Ok((host, theirs)) = target(app, name, p) else { return false };
        theirs.eq_ignore_ascii_case(&repo) && ci_for(app, &host).ok().and_then(|ci| branch_for(app, name, p, ci.as_ref(), &theirs).ok()).is_some_and(|b| b == branch)
    });
    let Some((name, p)) = watched.map(|(k, p)| (k.clone(), p.clone())) else { return Ok(None) };
    let build_id = ev.get("build_id").filter(|v| !v.is_null() && v.as_str() != Some("")).cloned().or_else(|| id_in_url(&body_str(ev, "url")).map(|n| json!(n)));
    let body = json!({"pipeline": ev["pipeline"], "build_id": build_id, "url": ev["url"], "sha": ev["head"], "state": ev["state"], "ts": ev["at"]});
    record_build(app, &name, &p, &body).map(Some)
}

/// The last all-digit segment of a build link's path (`.../pipelines/results/38`, `.../actions/runs/41`).
fn id_in_url(url: &str) -> Option<u64> {
    let path = url.split(['?', '#']).next().unwrap_or("");
    path.rsplit('/').find_map(|seg| seg.parse().ok())
}

/// Keeps a default-branch build as the project's latest and reads its branch now.
fn record_build(app: &App, name: &str, p: &MasterProject, body: &Value) -> Result<Value> {
    let name = name.to_string();
    let mut build = serde_json::Map::new();
    for k in ["pipeline", "build_id", "url", "sha", "state", "ts", "last_green"] {
        if let Some(v) = body.get(k).filter(|v| !v.is_null() && v.as_str() != Some("")) {
            build.insert(k.into(), v.clone());
        }
    }
    build.insert("at".into(), json!(now_iso()));
    note_watch(app, &name, fields!["build" => Value::Object(build)])?;
    check_with(app, &name, p, Some(body))?;
    let open = match open_break(app, &name)? {
        Some(b) => dict(app, &b)?,
        None => Value::Null,
    };
    Ok(json!({"ok": true, "project": name, "checked": true, "break": open}))
}

/// `GET /master-breaks?limit=n` (the old board's): `{breaks}`, the open breaks then the closed ones,
/// each newest first, at most `limit` (20) in all.
pub fn old_list(app: &App, limit: i64) -> Result<Value> {
    let mut out = vec![];
    for b in app.db.q("SELECT * FROM breaks ORDER BY state = 'open' DESC, id DESC LIMIT ?", p![limit.clamp(1, 500)])? {
        out.push(dict(app, &b)?);
    }
    Ok(json!({"breaks": out}))
}

/// The old board's routes under `/master-breaks`: the list, one break, and `POST :ref/verdict
/// {verdict: yours|not_yours|unsure, why, proof?}` (`yours` is `ours`, `not_yours` `not-ours`).
pub fn old_route(app: &App, method: &str, rest: &[&str], limit: i64, body: &Value) -> Result<Value> {
    match (method, rest) {
        ("GET", []) => old_list(app, limit),
        ("GET", [id]) => dict(app, &get(app, need_ref(&json!(id), "break")?)?),
        ("POST", [id, "verdict"]) => {
            let v = body_str(body, "verdict").to_lowercase().replace('-', "_");
            let mut b = Value::Object(body.as_object().cloned().unwrap_or_default());
            b["verdict"] = json!(match v.as_str() {
                "yours" => "ours",
                "not_yours" => "not_ours",
                other => other,
            });
            override_verdict(app, need_ref(&json!(id), "break")?, &b)
        }
        _ => err(404, "There's nothing at that address."),
    }
}

/// A verdict as it's decided: who decided it, why, and the cause in a few words when they gave one.
struct Verdict {
    verdict: String,
    by: String,
    why: String,
    title: Option<String>,
    /// The fault check's proof links (its `not_ours` needs one).
    proof: Vec<String>,
    /// Why the fault check couldn't give a verdict.
    error: Option<String>,
}

impl Verdict {
    fn new(verdict: &str, by: &str, why: String) -> Verdict {
        Verdict { verdict: verdict.into(), by: by.into(), why, title: None, proof: vec![], error: None }
    }
}

fn decide(app: &App, b: &Row) -> Verdict {
    let suspects = jloads_arr(b.s("suspects"));
    let ours: Vec<&Value> = suspects.iter().filter(|s| s["ours"] == true).collect();
    let made = Verdict::new;
    if ours.is_empty() {
        return made("not_ours", "commits", format!("None of the suspect commits ({}) are {}.", shas(&suspects), app.cfg.owners()));
    }
    if app.cfg.master.fault_check {
        // Only a verdict the check reached can start a fix; one it couldn't reach is unsure.
        return ask_claude(app, b).unwrap_or_else(|e| Verdict {
            error: Some(e.clone()),
            ..made("unsure", "claude", format!("The fault check couldn't decide: {e}."))
        });
    }
    if ours.len() == suspects.len() {
        made("ours", "fallback", format!("Every suspect commit ({}) is {}.", shas(&suspects), app.cfg.owners()))
    } else {
        made("unsure", "fallback", format!("{} of the suspect commits ({}) are {}, and the others aren't.", ours.len(), shas(&suspects), app.cfg.owners()))
    }
}

/// A cause title as it's kept: one line, NULL when empty.
fn title_of(raw: &str) -> Option<String> {
    Some(one_line(raw, TITLE_MAX)).filter(|t| !t.is_empty())
}

fn shas(list: &[Value]) -> String {
    list.iter().map(|s| s["sha"].as_str().unwrap_or("").chars().take(8).collect::<String>()).collect::<Vec<_>>().join(", ")
}

fn verdict_schema() -> Value {
    json!({"type": "object", "required": ["verdict", "why", "title", "proof"], "properties": {
        "verdict": {"enum": ["ours", "not_ours", "unsure"]}, "why": {"type": "string"}, "title": {"type": "string", "maxLength": TITLE_MAX},
        "proof": {"type": "array", "items": {"type": "string"}}}})
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
         tell. Don't guess.\nGive the reason in one or two plain sentences, and a title: the cause in a few plain words \
         (80 characters at most, for example \"Android NDK download failed\"). For not_ours, give proof: links (http or https) \
         to what shows it isn't {owner}'s, such as the same failure on an earlier build or an issue about it; without one it \
         counts as unsure. Otherwise proof is an empty list.\n\nFailing checks:\n{}\n\nSuspect commits, newest \
         first:\n{}",
        failing.join("\n"),
        suspects.join("\n"),
        branch = b.st("branch"),
        repo = b.st("repo")
    )
}

/// Asks a headless `claude -p` for the verdict, or why it couldn't give one. A `not_ours` without an
/// http(s) proof link comes back `unsure`.
fn ask_claude(app: &App, b: &Row) -> std::result::Result<Verdict, String> {
    let claude = proc::which(&app.cfg.claude).ok_or_else(|| format!("{} isn't installed", app.cfg.claude))?;
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
    let out = match proc::run(&claude, &args, Some(&app.cfg.data), m.timeout_secs as f64) {
        Ok(o) => o,
        Err(proc::RunError::TimedOut) => return Err(format!("it took longer than {} seconds", m.timeout_secs)),
        Err(proc::RunError::Spawn(e)) => return Err(format!("it couldn't start ({e})")),
    };
    let unreadable = || {
        let said = if out.stderr.trim().is_empty() { out.stdout.trim() } else { out.stderr.trim() };
        format!("its answer couldn't be read (exit {}: {})", out.code.map(|c| c.to_string()).unwrap_or_else(|| "none".into()), one_line(said, 200))
    };
    let d: Value = serde_json::from_str(out.stdout.trim()).map_err(|_| unreadable())?;
    let v = d
        .get("structured_output")
        .cloned()
        .or_else(|| d.get("result").and_then(|r| r.as_str()).and_then(|r| serde_json::from_str(r).ok()))
        .ok_or_else(unreadable)?;
    let verdict = v["verdict"].as_str().filter(|x| matches!(*x, "ours" | "not_ours" | "unsure")).ok_or_else(|| format!("it gave no verdict ({})", one_line(&v.to_string(), 200)))?;
    let why = one_line(v["why"].as_str().unwrap_or(""), 500);
    let proof: Vec<String> = v["proof"]
        .as_array()
        .map(|a| a.iter().filter_map(|p| p.as_str()).map(str::trim).filter(|p| p.starts_with("http://") || p.starts_with("https://")).map(str::to_string).collect())
        .unwrap_or_default();
    let mut out = Verdict { title: title_of(v["title"].as_str().unwrap_or("")), proof, ..Verdict::new(verdict, "claude", why) };
    if out.verdict == "not_ours" && out.proof.is_empty() {
        out.verdict = "unsure".into();
        out.why = format!("It looked like someone else's, but nothing proved it: {}", out.why);
    }
    Ok(out)
}

fn decide_and_act(app: &App, b: &Row) -> Result<()> {
    let v = decide(app, b);
    let made = app.db.tx(|| set_verdict(app, b, &v, None))?;
    start_fix(app, made)
}

/// Starts a new fix task at once (`[master] start_fix`), outside the verdict's transaction. When it
/// can't start now it stays queued, high priority, for the runner.
fn start_fix(app: &App, made: Option<i64>) -> Result<()> {
    let Some(tid) = made.filter(|_| app.cfg.master.start_fix) else { return Ok(()) };
    let t = board::get_task(app, tid)?;
    if !matches!(crate::hooks::gate(app, "task.starting", &t, json!({})), crate::hooks::Decision::Go) {
        return Ok(());
    }
    if let Err(e) = crate::worktrees::ensure(app, &t) {
        return board::log_event(app, tid, board::BOARD, "status", &format!("Couldn't start it at once: {}. It waits in the queue.", e.message)).map(|_| ());
    }
    let r = app.db.tx(|| {
        let t = board::get_task(app, tid)?;
        if t.s("status") != Some("queued") || board::live_start_job(app, &t)?.is_some() {
            return Ok(());
        }
        crate::runner::start_task(app, &t, "new", None, None, None, None).map(|_| ())
    });
    match r {
        Err(e) if e.status < 500 => board::log_event(app, tid, board::BOARD, "status", &format!("Couldn't start it at once: {}. It waits in the queue.", e.message)).map(|_| ()),
        other => other,
    }
}

/// Records a verdict and acts on it: `ours` gets the fix task and the urgent alert; anything else
/// takes the alert down. The fix task, when it made one.
fn set_verdict(app: &App, b: &Row, v: &Verdict, proof: Option<&[String]>) -> Result<Option<i64>> {
    let (verdict, by, why) = (v.verdict.as_str(), v.by.as_str(), v.why.as_str());
    let proof = proof.map(|p| p.to_vec()).or_else(|| Some(v.proof.clone()).filter(|p| !p.is_empty()));
    app.db.update("breaks", &json!(b.id()), fields!["verdict" => verdict, "verdict_by" => by, "verdict_why" => why, "verdict_at" => now_iso(),
                                                   "verdict_title" => v.title, "proof" => proof.map(|p| jdumps(&json!(p))), "check_error" => v.error])?;
    let b = get(app, b.id())?;
    if let Some(tid) = b.i("task_id") {
        board::log_event(app, tid, board::BOARD, "status", &format!("{}: {} ({by}): {why}", bref(b.id()), verdict_label(verdict)))?;
    }
    if verdict != "ours" {
        dispatch::clear_alert_key(app, &alert_key(b.id()))?;
        app.db.update("breaks", &json!(b.id()), fields!["alert_state" => null])?;
        return Ok(None);
    }
    let made = match fix_for(app, &b)? {
        Fix::Own | Fix::Joined | Fix::Unfixed(_) => None,
        Fix::New => new_fix(app, &b)?.1,
    };
    follow(app, &get(app, b.id())?, false)?;
    Ok(made)
}

/// A new fix task for the break: (its id, and the same again as the one to start).
fn new_fix(app: &App, b: &Row) -> Result<(i64, Option<i64>)> {
    let card = crate::ops::new_task(app, &fix_task(app, b), board::BOARD, Some(&format!("Added to fix {}", bref(b.id()))))?;
    let tid = card["id"].as_i64().unwrap_or(0);
    app.db.update("breaks", &json!(b.id()), fields!["task_id" => tid])?;
    Ok((tid, Some(tid)))
}

/// Who fixes an `ours` break.
enum Fix {
    /// Its own task still covers it.
    Own,
    /// Another break's fix covers it: the break now points at that task (logged there).
    Joined,
    /// Its own task finished without a fix PR (or its PR was declined) and nothing else covers it.
    Unfixed(Row),
    /// Nothing covers it: it needs a new fix task.
    New,
}

/// When the failing build on the break's head was queued: the host's word, else when the board
/// first saw that head red, else when the break opened.
fn build_time(b: &Row) -> Option<f64> {
    b.s("queued_at").or(b.s("seen_at")).or(b.s("opened_at")).and_then(parse_iso)
}

/// When the task's PR merged: its "PR #n was merged" (or "Merged PR #n") event.
fn merged_at(app: &App, t: &Row) -> Result<Option<f64>> {
    let n = t.i0("pr_num");
    let e = app.db.q1(
        "SELECT at FROM events WHERE task_id = ? AND (text = ? OR text = ?) ORDER BY id DESC LIMIT 1",
        p![t.id(), format!("PR #{n} was merged"), format!("Merged PR #{n}")],
    )?;
    Ok(e.and_then(|e| e.s("at").and_then(parse_iso)))
}

fn pr_merged(t: &Row) -> bool {
    t.st("pr_state").eq_ignore_ascii_case("MERGED") || t.s("pr_phase") == Some("merged")
}

fn pr_declined(t: &Row) -> bool {
    matches!(t.st("pr_state").to_uppercase().as_str(), "DECLINED" | "CLOSED" | "SUPERSEDED") || t.s("pr_phase") == Some("declined")
}

/// A fix that finished without a PR, or whose PR was declined.
fn no_fix_pr(t: &Row) -> bool {
    t.s("status") == Some("done") && (t.i("pr_num").is_none() || pr_declined(t))
}

/// The task covers the break: it isn't done, or its PR is open, or its PR merged after the failing
/// build was queued (that build couldn't hold the fix).
fn covers(app: &App, t: &Row, b: &Row) -> Result<bool> {
    if t.s("status") != Some("done") {
        return Ok(true);
    }
    if no_fix_pr(t) {
        return Ok(false);
    }
    if !pr_merged(t) {
        return Ok(true);
    }
    Ok(match (merged_at(app, t)?, build_time(b)) {
        (Some(m), Some(q)) => m > q,
        _ => false,
    })
}

/// The fix in flight that covers the break: its own task first, then the other `ours` breaks' of
/// the same project and branch, newest first. Joining another break's fix points the break at it
/// and logs that on both tasks.
fn fix_for(app: &App, b: &Row) -> Result<Fix> {
    let own = board::find_task(app, b.i("task_id"))?;
    if let Some(t) = &own {
        if covers(app, t, b)? {
            return Ok(Fix::Own);
        }
    }
    let others = app.db.q(
        "SELECT task_id FROM breaks WHERE project = ? AND branch IS ? AND id != ? AND verdict = 'ours' AND task_id IS NOT NULL ORDER BY id DESC",
        p![b.st("project"), b.v("branch"), b.id()],
    )?;
    for o in others {
        if o.i("task_id") == b.i("task_id") {
            continue;
        }
        let Some(t) = board::find_task(app, o.i("task_id"))? else { continue };
        if !covers(app, &t, b)? {
            continue;
        }
        app.db.update("breaks", &json!(b.id()), fields!["task_id" => t.id()])?;
        let text = match &own {
            Some(old) => format!("{} handed over to this fix from {}", bref(b.id()), rf("task", old.id())),
            None => format!("{} joins this fix: {} fails again at {}", bref(b.id()), b.st("branch"), b.st("last_head").chars().take(10).collect::<String>()),
        };
        board::log_event(app, t.id(), board::BOARD, "status", &text)?;
        if let Some(old) = &own {
            board::log_event(app, old.id(), board::BOARD, "status", &format!("{} handed over to {}, which covers it", bref(b.id()), rf("task", t.id())))?;
        }
        return Ok(Fix::Joined);
    }
    Ok(match own {
        Some(t) if no_fix_pr(&t) => Fix::Unfixed(t),
        _ => Fix::New,
    })
}

fn alert_key(id: i64) -> String {
    format!("master:{}", bref(id))
}

/// A `master:M<n>` alert holds while its break is open and the owner's: `follow` swaps its line as the
/// fix moves, and it goes when the break closes or is ruled someone else's, never on task events.
pub fn alert_resolved(app: &App, a: &Value) -> Result<bool> {
    let key = a["key"].as_str().unwrap_or("");
    let Ok(Some(id)) = parse_ref_str(key.trim_start_matches("master:"), "break") else { return Ok(true) };
    let b = app.db.q1("SELECT state, verdict FROM breaks WHERE id = ?", p![id])?;
    Ok(b.map(|b| b.s("state") != Some("open") || b.s("verdict") != Some("ours")).unwrap_or(true))
}

/// A fix task still queued this long after it was made hasn't started on time.
const START_LATE_SECS: f64 = 300.0;
/// A break of the owner's still red this long after it opened says so.
const STILL_RED_SECS: f64 = 7200.0;

/// Where an `ours` break's fix stands, as (state, the master alert's text): `no_fix`, `stopped`,
/// `needs`, `late`, `no_pr`, `stuck`, `checks`, `review`, `still_red`, or `on` while it's under way.
fn fix_state(app: &App, b: &Row) -> Result<(&'static str, String)> {
    let p = b.st("project");
    let Some(t) = board::find_task(app, b.i("task_id"))? else {
        return Ok(("no_fix", format!("Master {p} broke on your change and no fix is running.")));
    };
    let n = rf("task", t.id());
    let fix = format!("{n}, the fix for master {p},");
    let pr = t.i0("pr_num");
    let pr_open = t.i("pr_num").is_some() && !pr_merged(&t) && !pr_declined(&t);
    let status = t.st("status");
    if status == "done" && t.b("failed") {
        return Ok(("stopped", format!("{fix} has stopped: {}", one_line(t.s("summary").filter(|s| !s.is_empty()).unwrap_or("it ended without finishing"), 200))));
    }
    if status == "needs" {
        if t.b("lost") {
            return Ok(("stopped", format!("{fix} has stopped: its terminal was lost.")));
        }
        let asked = t.s("question").or(t.s("needs_reason")).filter(|q| !q.is_empty()).unwrap_or("it's waiting on you");
        return Ok(("needs", format!("{fix} needs you: {}", one_line(asked, 300))));
    }
    if status == "queued" && age_secs(t.s("created_at")).unwrap_or(0.0) >= START_LATE_SECS {
        let last = app.db.q1("SELECT text FROM events WHERE task_id = ? ORDER BY id DESC LIMIT 1", p![t.id()])?;
        let why = last.and_then(|e| e.s("text").map(|s| one_line(s, 200))).unwrap_or_else(|| "it's still queued".into());
        return Ok(("late", format!("{n} hasn't started after 5 minutes: {why}")));
    }
    if no_fix_pr(&t) {
        return Ok(("no_pr", format!("{fix} finished without a fix PR, and {} is still red.", b.st("branch"))));
    }
    if status == "done" && pr_open {
        if crate::prflow::wake_stuck(&t) {
            return Ok(("stuck", format!("The fix PR #{pr} for master {p} needs its agent, but it didn't wake.")));
        }
        if t.s("pr_phase") == Some("fix") {
            return Ok(("checks", format!("The fix PR #{pr}'s checks are failing; {n} is fixing them.")));
        }
        if crate::prflow::awaiting_owner(&t) {
            return Ok(("review", format!("The fix PR #{pr} for master {p} is waiting for your review.")));
        }
    }
    let red_for = age_secs(b.s("opened_at")).unwrap_or(0.0);
    if red_for >= STILL_RED_SECS {
        let hours = (red_for / 3600.0).floor() as i64;
        return Ok(("still_red", format!("Master {p} is still red {} after your change broke it; {n} is on it.", plural(hours, "hour"))));
    }
    Ok((
        "on",
        format!(
            "{} is red on {p}: {} fails{}. It's yours to fix: {n} is on it.",
            b.st("branch"),
            jloads_arr(b.s("checks")).iter().filter_map(|c| c.as_str()).collect::<Vec<_>>().join(", "),
            b.s("verdict_title").map(|t| format!(" ({t})")).unwrap_or_default(),
        ),
    ))
}

/// Keeps an `ours` break's urgent alert (`master:M<n>`) in step with its fix: a new state takes the
/// alert down and raises it again with its own line, so it notifies anew; `again` does that even when
/// the state is the same. The fix's own alerts wait behind an urgent one, so its question reaches the
/// owner here.
fn follow(app: &App, b: &Row, again: bool) -> Result<()> {
    if b.s("verdict") != Some("ours") || b.s("state") != Some("open") {
        return Ok(());
    }
    let (state, text) = fix_state(app, b)?;
    if !again && b.s("alert_state") == Some(state) {
        return Ok(());
    }
    dispatch::clear_alert_key(app, &alert_key(b.id()))?;
    dispatch::raise(app, &text, b.i("task_id"), None, Some(&alert_key(b.id())), true)?;
    app.db.update("breaks", &json!(b.id()), fields!["alert_state" => state])?;
    if let (Some(tid), Some(_)) = (b.i("task_id"), b.s("alert_state").filter(|s| *s != state)) {
        board::log_event(app, tid, board::BOARD, "status", &format!("{}'s alert: {text}", bref(b.id())))?;
    }
    Ok(())
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
    let tb = board::tb_cmd(app);
    let m = bref(b.id());
    detail.push(format!(
        "How to finish:\n\
         - Branch from origin/{branch}, make the smallest change that turns it green, and open the PR.\n\
         - If it goes green on its own, check {tb} master {m} before doing more, then finish with {tb} submit \"<summary>\" --no-pr \"<why>\".\n\
         - If you find it isn't {owners} change after all, run {tb} master {m} not-ours --title \"<cause>\" --why \"<how you checked>\" --proof <build or artifact link> \
         (or {tb} master {m} unsure --why \"<what you checked>\" when you can't tell), then finish with no PR.",
        branch = b.st("branch"),
        owners = app.cfg.owners(),
    ));
    let title = match b.s("verdict_title") {
        Some(cause) => {
            let pipeline = ev["build"]["pipeline"].as_str().filter(|p| !p.is_empty()).map(|p| p.to_string()).unwrap_or_else(|| checks.join(", "));
            format!("Fix master {pipeline}: {cause}")
        }
        None => format!("Fix red {} on {}: {}", b.st("branch"), b.st("project"), checks.join(", ")),
    };
    json!({"title": short(&title, 80), "detail": detail.join("\n\n"),
           "project": b.st("project"), "priority": "high", "pickup": if app.cfg.master.start_fix { "new" } else { "queue" },
           "jira": {"mode": if app.cfg.master.fix_ticket { "create" } else { "none" }},
           "origin": {"from": format!("{} ({} is red)", bref(b.id()), b.st("branch")), "by": board::BOARD}, "not_backlog": true})
}

/// An `ours` break whose fix task finished while the branch is still red. A fix in flight covers it
/// (its own, or another break's it's handed to); a fix whose PR merged before the failing build was
/// queued gets a new fix task (the one to start); one that finished without a fix PR, with nothing
/// covering it, raises the alert again once per `recheck_mins`.
fn escalate(app: &App, b: &Row) -> Result<Option<i64>> {
    if b.s("verdict") != Some("ours") || b.i("task_id").is_none() {
        return Ok(None);
    }
    let t = match fix_for(app, b)? {
        Fix::Own | Fix::Joined => return Ok(None),
        Fix::New => {
            let (_, made) = new_fix(app, b)?;
            follow(app, &get(app, b.id())?, true)?;
            return Ok(made);
        }
        Fix::Unfixed(t) => t,
    };
    let recheck = app.cfg.master.recheck_mins * 60.0;
    let since = b.s("escalated_at").or(t.s("finished_at"));
    if age_secs(since).unwrap_or(f64::MAX) < recheck {
        return Ok(None);
    }
    app.db.update("breaks", &json!(b.id()), fields!["escalated_at" => now_iso()])?;
    board::log_event(app, t.id(), board::BOARD, "status", &format!("{} is still red after this task finished", b.st("branch")))?;
    follow(app, b, true)?;
    Ok(None)
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
        "verdict_by": b.v("verdict_by"), "verdict_why": b.v("verdict_why"), "verdict_title": b.v("verdict_title"), "verdict_at": b.v("verdict_at"),
        "proof": jloads_arr(b.s("proof")), "check_error": b.v("check_error"), "task": task,
        "opened_at": b.v("opened_at"), "closed_at": b.v("closed_at"), "checked_at": b.v("checked_at"),
    }))
}

/// `state.master`: the open breaks that are the owner's, for the app's "Master is red" banner.
pub fn banner(app: &App) -> Result<Vec<Value>> {
    let mut out = vec![];
    for b in app.db.q("SELECT * FROM breaks WHERE state = 'open' AND verdict = 'ours' ORDER BY id", p![])? {
        out.push(dict(app, &b)?);
    }
    Ok(out)
}

/// How many breaks `GET /master` lists unless it's told.
pub const LIST_LIMIT: i64 = 20;

/// `GET /master?open=1&limit=20` (`tb master [--open] [--limit N]`): the open breaks, then the closed
/// ones (at most ten, none with `open`), each newest first, at most `limit` in all; and what's watched.
pub fn list(app: &App, open_only: bool, limit: i64) -> Result<Value> {
    let limit = limit.clamp(1, 500);
    let mut open = vec![];
    for b in app.db.q("SELECT * FROM breaks WHERE state = 'open' ORDER BY id DESC LIMIT ?", p![limit])? {
        open.push(dict(app, &b)?);
    }
    let room = if open_only { 0 } else { (limit - open.len() as i64).min(10) };
    let mut closed = vec![];
    for b in app.db.q("SELECT * FROM breaks WHERE state = 'closed' ORDER BY id DESC LIMIT ?", p![room])? {
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
            json!({"project": name, "branch": known_branch(app, name, p), "checked_at": s.get("checked_at"),
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
    let proof: Vec<String> = match &body["proof"] {
        Value::String(s) => vec![s.trim().to_string()],
        Value::Array(a) => a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        _ => vec![],
    };
    if v == "not_ours" && proof.is_empty() {
        return err(400, "Add at least one --proof link for not-ours: the build that shows it, or an issue about it.");
    }
    if let Some(bad) = proof.iter().find(|p| !crate::prcmds::is_link(p)) {
        return err(400, format!("{bad} isn't a link: proof is http(s) links."));
    }
    let who = board::caller(app, body, &app.cfg.owner);
    if who != app.cfg.owner && b.s("verdict_by") == Some(app.cfg.owner.as_str()) {
        return err(
            409,
            format!("{} already decided {} is {}; their word stands. Ask them if you think otherwise.", app.cfg.owner, bref(id), b.st("verdict").replace('_', "-")),
        );
    }
    let why = one_line(&body_str(body, "why"), 1000);
    let n = why.chars().count();
    if !(REASON_MIN..=REASON_MAX).contains(&n) {
        return err(
            400,
            format!("The reason is {n} characters: say in plain words what broke it and how you checked, in {REASON_MIN} to {REASON_MAX} (--why)."),
        );
    }
    let raw_title = one_line(&body_str(body, "title"), 1000);
    if raw_title.chars().count() > TITLE_MAX {
        return err(400, format!("The title is {} characters: name the cause in {TITLE_MAX} at most.", raw_title.chars().count()));
    }
    if v == "not_ours" && raw_title.is_empty() {
        return err(400, format!("Name the cause for not-ours in a few plain words, {TITLE_MAX} characters at most (--title)."));
    }
    let verdict = Verdict { title: title_of(&raw_title), ..Verdict::new(&v, &who, why) };
    let made = app.db.tx(|| set_verdict(app, &b, &verdict, (!proof.is_empty()).then_some(proof.as_slice())))?;
    start_fix(app, made)?;
    dict(app, &get(app, id)?)
}

pub fn route(app: &App, method: &str, rest: &[&str], body: &Value, open_only: bool, limit: i64) -> Result<Value> {
    match (method, rest) {
        ("GET", []) => list(app, open_only, limit),
        ("POST", ["check"]) => {
            for (name, p) in app.cfg.master.projects.clone() {
                check_project(app, &name, &p)?;
            }
            list(app, open_only, limit)
        }
        ("GET", [id]) => dict(app, &get(app, need_ref(&json!(id), "break")?)?),
        ("POST", [id]) => override_verdict(app, need_ref(&json!(id), "break")?, body),
        _ => err(404, "There's nothing at that address."),
    }
}

/// A branch for tests: answers each read with what it's given, a range from `history` (every
/// commit, newest first), and `default` as the default branch.
#[derive(Default)]
pub struct FakeBranch {
    pub read: parking_lot::Mutex<BranchRead>,
    pub history: parking_lot::Mutex<Vec<Commit>>,
    pub default: parking_lot::Mutex<Option<String>>,
    /// The branches read, in order.
    pub branches: parking_lot::Mutex<Vec<String>>,
}

impl BranchCi for FakeBranch {
    fn read(&self, _repo: &str, branch: &str, _commits: usize) -> HostResult<BranchRead> {
        self.branches.lock().push(branch.to_string());
        Ok(self.read.lock().clone())
    }
    fn since(&self, _repo: &str, base: &str, head: &str) -> HostResult<Vec<Commit>> {
        let h = self.history.lock();
        let from = h.iter().position(|c| c.sha == head).ok_or("no such head")?;
        let to = h.iter().position(|c| c.sha == base).ok_or("no such base")?;
        Ok(h[from..to.max(from)].to_vec())
    }
    fn default_branch(&self, _repo: &str) -> HostResult<String> {
        self.default.lock().clone().ok_or_else(|| "no default branch".into())
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

    #[test]
    fn reads_a_github_range_and_default_branch() {
        let gh = CannedGh(Mutex::new(vec![
            ("repos/a/b/compare/c0...c3".into(), json!({"commits": [{"sha": "c1", "commit": {"author": {"email": "a@x"}, "message": "one"}},
                                                                     {"sha": "c2", "commit": {"author": {"email": "b@x"}, "message": "two"}},
                                                                     {"sha": "c3", "commit": {"author": {"email": "c@x"}, "message": "three"}}]}).to_string()),
            ("repos/a/b".into(), json!({"default_branch": "master"}).to_string()),
        ]));
        let ci = GithubBranch { gh: Box::new(gh) };
        let shas: Vec<String> = ci.since("a/b", "c0", "c3").unwrap().into_iter().map(|c| c.sha).collect();
        assert_eq!(shas, vec!["c3", "c2", "c1"], "newest first");
        assert_eq!(ci.default_branch("a/b").unwrap(), "master");
    }

    #[test]
    fn reads_a_bitbucket_range_and_main_branch() {
        let http = CannedHttp(vec![
            ("/commits/h3?exclude=h0".into(), json!({"values": [{"hash": "h3", "author": {"raw": "Ann <ann@x.dev>"}, "message": "c"},
                                                                 {"hash": "h2", "author": {"raw": "Bo <bo@x.dev>"}, "message": "b"}]})),
            ("/repositories/ws/r".into(), json!({"mainbranch": {"name": "master"}})),
        ]);
        let ci = BitbucketBranch { http: Box::new(http), api: "https://api/2.0".into() };
        assert_eq!(ci.since("ws/r", "h0", "h3").unwrap().len(), 2);
        assert_eq!(ci.default_branch("ws/r").unwrap(), "master");
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
