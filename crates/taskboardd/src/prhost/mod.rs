//! PR hosts: one interface over GitHub and Bitbucket Cloud, so the PR flow (`prflow.rs`), `tb pr …` and
//! the app read and act on a pull request the same way whichever host it lives on.
//!
//! # The interface
//!
//! [`PrHost`] is everything the board does to a PR on its host. Get one with [`host_for`] (by the
//! task's `pr_host`: `github` or `bitbucket`) and pass it the PR as a [`PrRef`] ([`PrRef::of`] a task
//! row). Every call answers `Result<_, String>`; the error is a sentence for a person (the owner or the
//! agent), never a stack of causes.
//!
//! | Call | GitHub (`gh`) | Bitbucket Cloud (REST 2.0) |
//! |---|---|---|
//! | [`read`](PrHost::read): state, head, base, checks, reviewers, threads, tasks | `gh pr view --json …` + GraphQL `reviewThreads` | `pullrequests/{n}`, `/comments`, `/tasks`, `commit/{head}/statuses` |
//! | [`reviewers`](PrHost::reviewers) / [`threads`](PrHost::threads) | from `read` | from `read` |
//! | [`reply`](PrHost::reply) to a thread | GraphQL `addPullRequestReviewThreadReply`; a quoting `gh pr comment` for a plain comment | a reply comment (`parent`) |
//! | [`resolve`](PrHost::resolve) a thread | GraphQL `resolveReviewThread` (plain comments can't be: `Ok(false)`) | `comments/{id}/resolve`; a task is set `RESOLVED` |
//! | [`request_reviews`](PrHost::request_reviews) | `POST pulls/{n}/requested_reviewers` | `PUT` the PR with the reviewer added |
//! | [`re_request_reviews`](PrHost::re_request_reviews): ask someone who already reviewed to look again | the same request (GitHub re-requests) | `PUT` without them, then with them, which clears their old state |
//! | [`remove_reviewers`](PrHost::remove_reviewers) | `DELETE pulls/{n}/requested_reviewers` | `PUT` the PR without them |
//! | [`replace_reviewer`](PrHost::replace_reviewer) | request the new one, then remove the old | the same |
//! | [`merge`](PrHost::merge) (closing the source branch) | `gh pr merge --<strategy> --delete-branch` | `POST …/merge` with `close_source_branch` |
//! | [`retarget`](PrHost::retarget) the base | `gh pr edit --base` | `PUT` the PR's `destination` |
//! | [`open`](PrHost::open) a PR from a pushed branch | `POST repos/{repo}/pulls` | `POST …/pullrequests` (with `close_source_branch`) |
//! | [`description`](PrHost::description) / [`set_description`](PrHost::set_description) | `GET` / `PATCH repos/{repo}/pulls/{n}` | `GET` / `PUT` the PR's `description` |
//! | [`cancel_builds`](PrHost::cancel_builds) for a head | `gh run cancel` on its Actions runs | `stopPipeline` on its Pipelines runs; other CI: [`Cancelled::Unsupported`] |
//! | [`base_failures`](PrHost::base_failures): checks failing on the base's last few commits | check runs and statuses of `commits?sha=<base>` | statuses of `commits/<base>` |
//! | [`members`](PrHost::members): who can review in the repo (the reviewer picker, `picker.rs`) | `repos/{repo}/collaborators` | `workspaces/{ws}/members` |
//!
//! Users are named by the host's own id ([`Reviewer::user`]): a GitHub login, a Bitbucket account's
//! `{uuid}` (an `account_id` works too). [`Reviewer::name`] is for people.
//!
//! # The record
//!
//! [`Record`] is one read of a PR. [`Record::to_value`] turns it into the JSON the PR flow keeps as
//! `pr_flow.rec` and steps on (`prflow::phase_of`): `state`, `head`, `branch`, `base`, `base_head`,
//! `checks` (`name`, `state` = passed / failed / running / stopped, `url`), `failed`, `running`,
//! `comments` (comments from others), `approvals` (not the author's or the board's own account's), `review_decision`, `changes_at`, `reviewers`,
//! `threads`, `tasks_open`, `tasks_error`, `viewer`. A thread ([`Thread`]) is open while it's unresolved
//! and someone other than the board's own account (`viewer`, the account it posts as; the PR's author
//! when that isn't known) had the last word; a PR task (Bitbucket) is open until it's resolved. When the
//! tasks couldn't be read, `tasks_error` says why and `tasks_open` is null: unknown, not none. The flow adds its own per-thread
//! state on top (threads acknowledged with `tb pr ack`), see `prflow::open_threads`.
//!
//! # CI
//!
//! A failed check's steps and failing tests come from a [`ci::CiProvider`]: GitHub Actions, Bitbucket
//! Pipelines, Azure Pipelines, or a project's own `failures_cmd` (`[pr.projects.<name>]`). See `ci.rs`.
//!
//! # Tests
//!
//! [`install`] puts a host in place of the real one for a board (the integration tests use
//! [`FakeHost`], which keeps a record in memory and logs every call). The GitHub and Bitbucket
//! implementations talk through [`github::Gh`] and [`bitbucket::Http`], so their unit tests feed them
//! canned answers instead of the network.

pub mod bitbucket;
pub mod ci;
pub mod github;

use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;

pub type HostResult<T> = std::result::Result<T, String>;

/// The hosts the board watches.
pub const WATCHED: &[&str] = &["github", "bitbucket"];

/// A pull request on its host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRef {
    /// `github` or `bitbucket`.
    pub host: String,
    /// `owner/name` (GitHub) or `workspace/repo` (Bitbucket).
    pub repo: String,
    pub num: i64,
    pub url: String,
}

impl PrRef {
    /// The PR a task row links to.
    pub fn of(t: &Row) -> Option<PrRef> {
        let num = t.i("pr_num")?;
        let repo = t.s("pr_repo").filter(|r| !r.is_empty())?;
        Some(PrRef { host: t.st("pr_host"), repo: repo.to_string(), num, url: t.st("pr_url") })
    }
}

/// One check (a CI build or status) on the PR's head.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    /// passed, failed, running or stopped.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Someone asked to review the PR, or who reviewed it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Reviewer {
    /// The host's id for them: what [`PrHost::request_reviews`] and friends take.
    pub user: String,
    pub name: String,
    /// approved, changes (asked for changes), commented, or pending (asked and hasn't answered yet).
    /// Someone asked again keeps their last state (GitHub; Bitbucket clears it) with `requested` set.
    pub state: String,
    /// On the PR's reviewer list right now (not only someone who left a review).
    pub requested: bool,
}

/// A conversation on the PR: a review thread, a plain comment, a review's summary, or a PR task.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    /// What `tb pr reply` / `tb pr ack` take.
    pub id: String,
    /// review (a code thread), comment, summary (a review's body) or task.
    pub kind: String,
    /// The host can mark it resolved (otherwise `tb pr ack` only notes it on the board).
    pub resolvable: bool,
    pub resolved: bool,
    /// Who started it.
    pub author: String,
    pub author_name: String,
    /// Who spoke last, and that comment's id and time: an ack holds until someone speaks again.
    pub last_author: String,
    pub last_id: String,
    pub last_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    /// The first comment's text, clipped.
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The code it's on changed since (GitHub).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outdated: bool,
    /// The comments after the first, oldest first (what `tb pr status` shows under it).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<Reply>,
}

/// A reply on a thread.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub author: String,
    pub author_name: String,
    /// The reply's text on one line, clipped.
    pub text: String,
    pub at: String,
}

/// How much of a reply's text a record keeps.
pub(crate) const REPLY_MAX: usize = 400;

impl Thread {
    /// Waiting on us (the board's account, see [`Record::us`]): unresolved, and someone else spoke last.
    /// A task waits until resolved.
    pub fn waiting_on(&self, us: &str) -> bool {
        if self.resolved {
            return false;
        }
        self.kind == "task" || self.last_author != us
    }
}

/// One read of a PR.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Record {
    /// OPEN, MERGED, CLOSED, DECLINED or SUPERSEDED.
    pub state: String,
    pub title: String,
    pub author: String,
    pub head: String,
    pub branch: String,
    pub base: String,
    pub base_head: String,
    pub checks: Vec<Check>,
    /// The host's own verdict (GitHub's `reviewDecision`): APPROVED, CHANGES_REQUESTED,
    /// REVIEW_REQUIRED or empty.
    pub review_decision: String,
    /// Approvals from people other than the author.
    pub approvals: i64,
    /// When the latest standing request for changes was made.
    pub changes_at: Option<String>,
    /// Comments from people other than the author.
    pub comments: i64,
    pub reviewers: Vec<Reviewer>,
    pub threads: Vec<Thread>,
    pub tasks_open: i64,
    /// Why the PR's tasks couldn't be read (Bitbucket): then `tasks_open` is unknown.
    pub tasks_error: Option<String>,
    /// The account the board reads and posts as, in the host's ids like [`Thread::last_author`]; empty
    /// when the host couldn't say.
    pub viewer: String,
    pub mergeable: Value,
}

impl Record {
    /// Who "us" is for a thread: the board's own account, else (not known) the PR's author.
    pub fn us(&self) -> &str {
        if self.viewer.is_empty() {
            &self.author
        } else {
            &self.viewer
        }
    }

    /// Recounts `approvals` without the board's own account (`viewer`): like the PR's author, its
    /// approval doesn't count toward the ones a PR needs.
    pub fn leave_out_viewer(&mut self) {
        if !self.viewer.is_empty() {
            self.approvals = self.reviewers.iter().filter(|r| r.state == "approved" && r.user != self.viewer).count() as i64;
        }
    }

    pub fn failed(&self) -> Vec<String> {
        self.checks.iter().filter(|c| c.state == "failed").map(|c| c.name.clone()).collect()
    }

    /// The record as the PR flow keeps it.
    pub fn to_value(&self) -> Value {
        json!({
            "state": self.state, "title": self.title, "author": self.author, "head": self.head,
            "branch": self.branch, "base": self.base, "base_head": self.base_head,
            "review_decision": self.review_decision, "checks": self.checks, "failed": self.failed(),
            "running": self.checks.iter().filter(|c| c.state == "running").count(),
            "comments": self.comments, "approvals": self.approvals, "mergeable": self.mergeable,
            "changes_at": self.changes_at, "reviewers": self.reviewers, "threads": self.threads,
            "tasks_open": if self.tasks_error.is_some() { Value::Null } else { json!(self.tasks_open) },
            "tasks_error": self.tasks_error, "viewer": self.viewer,
            // Reviewers asked who haven't reviewed yet (the PR bar's "x of N").
            "requested": self.reviewers.iter().filter(|r| r.requested && r.state == "pending").count(),
        })
    }
}

/// How to merge.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MergeOpts {
    /// The host's strategy name; None for the repository's default.
    pub strategy: Option<String>,
    /// Delete the PR's branch once it's merged.
    pub close_source_branch: bool,
}

/// What [`PrHost::cancel_builds`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum Cancelled {
    /// This many builds were stopped (0: none were running).
    Stopped(usize),
    /// The host can't stop this PR's builds, and why.
    Unsupported(String),
}

/// Everything the board does to a PR on its host. See the module docs for what each call is on GitHub
/// and Bitbucket.
pub trait PrHost: Send + Sync {
    /// `github` or `bitbucket`.
    fn id(&self) -> &'static str;
    fn read(&self, pr: &PrRef) -> HostResult<Record>;
    fn reviewers(&self, pr: &PrRef) -> HostResult<Vec<Reviewer>> {
        Ok(self.read(pr)?.reviewers)
    }
    fn threads(&self, pr: &PrRef) -> HostResult<Vec<Thread>> {
        Ok(self.read(pr)?.threads)
    }
    fn reply(&self, pr: &PrRef, thread: &Thread, body: &str) -> HostResult<()>;
    /// Marks the thread resolved on the host; `Ok(false)` when the host can't resolve this kind.
    fn resolve(&self, pr: &PrRef, thread: &Thread) -> HostResult<bool>;
    fn request_reviews(&self, pr: &PrRef, users: &[String]) -> HostResult<()>;
    /// Asks people who already reviewed to look again.
    fn re_request_reviews(&self, pr: &PrRef, users: &[String]) -> HostResult<()>;
    fn remove_reviewers(&self, pr: &PrRef, users: &[String]) -> HostResult<()>;
    fn replace_reviewer(&self, pr: &PrRef, old: &str, new: &str) -> HostResult<()> {
        self.request_reviews(pr, &[new.to_string()])?;
        self.remove_reviewers(pr, &[old.to_string()])
    }
    fn merge(&self, pr: &PrRef, opts: &MergeOpts) -> HostResult<()>;
    fn retarget(&self, pr: &PrRef, base: &str) -> HostResult<()>;
    /// Opens a PR in `repo` from the pushed `branch` into `base`; the new PR.
    fn open(&self, repo: &str, base: &str, branch: &str, title: &str, body: &str) -> HostResult<PrRef>;
    /// The PR's description, as it is on the host.
    fn description(&self, _pr: &PrRef) -> HostResult<String> {
        Err(format!("the board can't read a {} PR's description", self.id()))
    }
    /// Replaces the PR's description.
    fn set_description(&self, _pr: &PrRef, _body: &str) -> HostResult<()> {
        Err(format!("the board can't change a {} PR's description", self.id()))
    }
    fn cancel_builds(&self, pr: &PrRef, head: &str) -> HostResult<Cancelled>;
    /// Names of the checks that failed on any of the base branch's last `commits` commits.
    fn base_failures(&self, pr: &PrRef, base: &str, commits: usize) -> HostResult<Vec<String>>;
    /// People who can review in `repo` (`user` and `name`; `state` is empty): the reviewer picker
    /// matches them to commit authors. None by default.
    fn members(&self, _repo: &str) -> HostResult<Vec<Reviewer>> {
        Ok(vec![])
    }

    /// The checks that failed on the base branch's last `commits` commits, one per run (with its link,
    /// so their failed steps and tests can be read and compared one by one).
    fn base_failed_checks(&self, pr: &PrRef, base: &str, commits: usize) -> HostResult<Vec<Check>> {
        Ok(self.base_failures(pr, base, commits)?.into_iter().map(|name| Check { name, state: "failed".into(), url: None }).collect())
    }
}

/// The host a task's PR lives on: one [`install`]ed for this board, else the real one.
pub fn host_for(app: &App, host: &str) -> HostResult<Arc<dyn PrHost>> {
    if let Some(h) = app.shared.lock().pr_hosts.get(host) {
        return Ok(h.clone());
    }
    match host {
        "github" => {
            let gh = crate::proc::which(&app.cfg.pr.gh).ok_or_else(|| "the gh command isn't installed".to_string())?;
            let env = crate::accounts::gh_env(&app.cfg);
            let key = account_key("github", &env.iter().map(|(_, v)| v.as_str()).collect::<Vec<_>>());
            Ok(Arc::new(github::GithubHost::new(Box::new(github::GhCli { bin: gh, env })).with_account(&key)))
        }
        "bitbucket" => {
            let (email, token) = crate::accounts::board_credentials(app, crate::accounts::Provider::Bitbucket)
                .ok_or_else(|| "Bitbucket isn't connected: connect it in Taskboard ▸ Settings ▸ Accounts".to_string())?;
            let key = account_key("bitbucket", &[&app.cfg.pr.bitbucket_api, &email, &token]);
            Ok(Arc::new(bitbucket::BitbucketHost::new(Box::new(bitbucket::BasicHttp::new(&email, &token)), &app.cfg.pr.bitbucket_api).with_account(&key)))
        }
        "" => Err("this PR's host isn't known".into()),
        other => Err(format!("the board doesn't work with {other} PRs")),
    }
}

/// Uses `host` for every PR on its host id on this board (tests).
pub fn install(app: &App, host: Arc<dyn PrHost>) {
    app.shared.lock().pr_hosts.insert(host.id().to_string(), host);
}

/// The host and repo a git remote's URL points at (`git@github.com:o/r.git`, `https://bitbucket.org/w/r`).
pub fn repo_of_remote(url: &str) -> Option<(String, String)> {
    let re = regex::Regex::new(r"(github\.com|bitbucket\.org)[:/]([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+?)(?:\.git)?/?$").unwrap();
    let c = re.captures(url.trim())?;
    let host = if &c[1] == "github.com" { "github" } else { "bitbucket" };
    Some((host.to_string(), c[2].to_string()))
}

pub fn watched(host: Option<&str>) -> bool {
    host.is_some_and(|h| WATCHED.contains(&h))
}

/// A host for tests: answers `read` with the record it's given and logs every call
/// (`"reply 12 thanks"`, `"merge close=true"`, …). Writes change the record the way the host would.
pub struct FakeHost {
    pub host: &'static str,
    pub rec: Mutex<Record>,
    pub calls: Mutex<Vec<String>>,
    /// Base-branch failures to report.
    pub base_failing: Mutex<Vec<String>>,
    /// What `members` answers.
    pub members: Mutex<Vec<Reviewer>>,
    /// Base-branch failed runs to report (with links); empty: `base_failing` without links.
    pub base_checks: Mutex<Vec<Check>>,
}

impl FakeHost {
    pub fn new(host: &'static str, rec: Record) -> Arc<FakeHost> {
        Arc::new(FakeHost { host, rec: Mutex::new(rec), calls: Mutex::new(vec![]), base_failing: Mutex::new(vec![]), members: Mutex::new(vec![]), base_checks: Mutex::new(vec![]) })
    }
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().clone()
    }
    fn log(&self, s: String) {
        self.calls.lock().push(s);
    }
}

impl PrHost for FakeHost {
    fn id(&self) -> &'static str {
        self.host
    }
    fn read(&self, _pr: &PrRef) -> HostResult<Record> {
        Ok(self.rec.lock().clone())
    }
    fn reply(&self, _pr: &PrRef, thread: &Thread, body: &str) -> HostResult<()> {
        self.log(format!("reply {} {body}", thread.id));
        let mut r = self.rec.lock();
        let us = r.us().to_string();
        if let Some(t) = r.threads.iter_mut().find(|t| t.id == thread.id) {
            t.last_author = us;
            t.last_id = format!("{}-reply", t.last_id);
        }
        Ok(())
    }
    fn resolve(&self, _pr: &PrRef, thread: &Thread) -> HostResult<bool> {
        self.log(format!("resolve {}", thread.id));
        if !thread.resolvable {
            return Ok(false);
        }
        if let Some(t) = self.rec.lock().threads.iter_mut().find(|t| t.id == thread.id) {
            t.resolved = true;
        }
        Ok(true)
    }
    fn request_reviews(&self, _pr: &PrRef, users: &[String]) -> HostResult<()> {
        self.log(format!("request {}", users.join(",")));
        let mut r = self.rec.lock();
        for u in users {
            match r.reviewers.iter_mut().find(|x| &x.user == u) {
                Some(x) => x.requested = true,
                None => r.reviewers.push(Reviewer { user: u.clone(), name: u.clone(), state: "pending".into(), requested: true }),
            }
        }
        Ok(())
    }
    fn re_request_reviews(&self, _pr: &PrRef, users: &[String]) -> HostResult<()> {
        self.log(format!("re-request {}", users.join(",")));
        for r in self.rec.lock().reviewers.iter_mut().filter(|r| users.contains(&r.user)) {
            r.requested = true;
        }
        Ok(())
    }
    fn remove_reviewers(&self, _pr: &PrRef, users: &[String]) -> HostResult<()> {
        self.log(format!("remove {}", users.join(",")));
        self.rec.lock().reviewers.retain(|r| !users.contains(&r.user));
        Ok(())
    }
    fn merge(&self, _pr: &PrRef, opts: &MergeOpts) -> HostResult<()> {
        self.log(format!("merge close={} strategy={}", opts.close_source_branch, opts.strategy.as_deref().unwrap_or("default")));
        self.rec.lock().state = "MERGED".into();
        Ok(())
    }
    fn open(&self, repo: &str, base: &str, branch: &str, title: &str, _body: &str) -> HostResult<PrRef> {
        self.log(format!("open {repo} {branch} into {base}: {title}"));
        let mut r = self.rec.lock();
        r.base = base.to_string();
        r.branch = branch.to_string();
        r.title = title.to_string();
        let url = match self.host {
            "bitbucket" => format!("https://bitbucket.org/{repo}/pull-requests/21"),
            _ => format!("https://github.com/{repo}/pull/21"),
        };
        Ok(PrRef { host: self.host.to_string(), repo: repo.to_string(), num: 21, url })
    }
    fn retarget(&self, _pr: &PrRef, base: &str) -> HostResult<()> {
        self.log(format!("retarget {base}"));
        self.rec.lock().base = base.to_string();
        Ok(())
    }
    fn cancel_builds(&self, _pr: &PrRef, head: &str) -> HostResult<Cancelled> {
        self.log(format!("cancel {head}"));
        Ok(Cancelled::Stopped(0))
    }
    fn base_failures(&self, _pr: &PrRef, base: &str, commits: usize) -> HostResult<Vec<String>> {
        self.log(format!("base {base} {commits}"));
        Ok(self.base_failing.lock().clone())
    }
    fn members(&self, _repo: &str) -> HostResult<Vec<Reviewer>> {
        Ok(self.members.lock().clone())
    }

    fn base_failed_checks(&self, pr: &PrRef, base: &str, commits: usize) -> HostResult<Vec<Check>> {
        let checks = self.base_checks.lock().clone();
        if checks.is_empty() {
            return Ok(self.base_failures(pr, base, commits)?.into_iter().map(|name| Check { name, state: "failed".into(), url: None }).collect());
        }
        self.log(format!("base {base} {commits}"));
        Ok(checks)
    }
}

/// The board's account on a host, looked up once per account (`key`) for the daemon's life. A failed
/// lookup isn't kept, so the next read asks again; it answers "" meanwhile.
pub(crate) fn viewer_cached(key: &str, look_up: impl FnOnce() -> HostResult<String>) -> String {
    static SEEN: std::sync::OnceLock<Mutex<std::collections::HashMap<String, String>>> = std::sync::OnceLock::new();
    let seen = SEEN.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if !key.is_empty() {
        if let Some(v) = seen.lock().get(key) {
            return v.clone();
        }
    }
    match look_up() {
        Ok(v) if !v.is_empty() => {
            if !key.is_empty() {
                seen.lock().insert(key.to_string(), v.clone());
            }
            v
        }
        _ => String::new(),
    }
}

/// A cache key for an account that doesn't keep its secret: a hash of it.
pub(crate) fn account_key(host: &str, parts: &[&str]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    format!("{host}:{:x}", h.finish())
}

/// The first line of a comment, clipped for a list.
pub(crate) fn gist(text: &str) -> String {
    one_line(text.lines().find(|l| !l.trim().is_empty()).unwrap_or(""), 140)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_waits_on_the_author_until_resolved_or_answered() {
        let mut t = Thread { kind: "review".into(), last_author: "rev".into(), ..Default::default() };
        assert!(t.waiting_on("me"));
        t.last_author = "me".into();
        assert!(!t.waiting_on("me"), "the author had the last word");
        t.kind = "task".into();
        assert!(t.waiting_on("me"), "a task waits until it's resolved");
        t.resolved = true;
        assert!(!t.waiting_on("me"));
    }

    #[test]
    fn us_is_the_board_s_account_else_the_pr_s_author() {
        let mut r = Record { author: "me".into(), ..Default::default() };
        assert_eq!(r.us(), "me");
        r.viewer = "bot".into();
        assert_eq!(r.us(), "bot");
        let t = Thread { kind: "review".into(), last_author: "bot".into(), ..Default::default() };
        assert!(!t.waiting_on(r.us()), "the board's account answered last, though it isn't the PR's author");
    }

    #[test]
    fn the_board_s_own_approval_doesn_t_count() {
        let rv = |user: &str| Reviewer { user: user.into(), state: "approved".into(), ..Default::default() };
        let mut r = Record { author: "me".into(), approvals: 2, reviewers: vec![rv("bot"), rv("rev")], ..Default::default() };
        r.leave_out_viewer();
        assert_eq!(r.approvals, 2, "the board's account isn't known yet");
        r.viewer = "bot".into();
        r.leave_out_viewer();
        assert_eq!(r.approvals, 1);
    }

    #[test]
    fn tasks_that_couldn_t_be_read_are_unknown() {
        let r = Record { tasks_open: 0, tasks_error: Some("Bitbucket answered 403".into()), ..Default::default() };
        let v = r.to_value();
        assert!(v["tasks_open"].is_null());
        assert_eq!(v["tasks_error"], "Bitbucket answered 403");
    }

    #[test]
    fn reads_the_host_and_repo_from_a_remote() {
        let r = |u: &str| repo_of_remote(u).map(|(h, r)| format!("{h} {r}"));
        assert_eq!(r("git@github.com:acme/webapp.git").as_deref(), Some("github acme/webapp"));
        assert_eq!(r("https://github.com/acme/web.app").as_deref(), Some("github acme/web.app"));
        assert_eq!(r("https://me@bitbucket.org/ws/repo.git").as_deref(), Some("bitbucket ws/repo"));
        assert_eq!(r("https://gitlab.com/a/b"), None);
    }

    #[test]
    fn the_record_counts_failed_and_running_checks() {
        let r = Record {
            checks: vec![
                Check { name: "a".into(), state: "failed".into(), url: None },
                Check { name: "b".into(), state: "running".into(), url: None },
                Check { name: "c".into(), state: "stopped".into(), url: None },
            ],
            ..Default::default()
        };
        let v = r.to_value();
        assert_eq!(v["failed"], json!(["a"]));
        assert_eq!(v["running"], 1);
    }
}
