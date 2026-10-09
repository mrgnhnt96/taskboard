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
//! | [`cancel_builds`](PrHost::cancel_builds) for a head | `gh run cancel` on its Actions runs | `stopPipeline` on its Pipelines runs; other CI: [`Cancelled::Unsupported`] |
//! | [`base_failures`](PrHost::base_failures): checks failing on the base's last few commits | check runs and statuses of `commits?sha=<base>` | statuses of `commits/<base>` |
//!
//! Users are named by the host's own id ([`Reviewer::user`]): a GitHub login, a Bitbucket account's
//! `{uuid}` (an `account_id` works too). [`Reviewer::name`] is for people.
//!
//! # The record
//!
//! [`Record`] is one read of a PR. [`Record::to_value`] turns it into the JSON the PR flow keeps as
//! `pr_flow.rec` and steps on (`prflow::phase_of`): `state`, `head`, `branch`, `base`, `base_head`,
//! `checks` (`name`, `state` = passed / failed / running / stopped, `url`), `failed`, `running`,
//! `comments` (comments from others), `approvals`, `review_decision`, `changes_at`, `reviewers`,
//! `threads`, `tasks_open`. A thread ([`Thread`]) is open while it's unresolved and someone else had
//! the last word; a PR task (Bitbucket) is open until it's resolved. The flow adds its own per-thread
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
}

impl Thread {
    /// Waiting on the PR's author: unresolved, and someone else spoke last. A task waits until resolved.
    pub fn waiting_on(&self, pr_author: &str) -> bool {
        if self.resolved {
            return false;
        }
        self.kind == "task" || self.last_author != pr_author
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
    pub mergeable: Value,
}

impl Record {
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
            "tasks_open": self.tasks_open,
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
    fn cancel_builds(&self, pr: &PrRef, head: &str) -> HostResult<Cancelled>;
    /// Names of the checks that failed on any of the base branch's last `commits` commits.
    fn base_failures(&self, pr: &PrRef, base: &str, commits: usize) -> HostResult<Vec<String>>;
}

/// The host a task's PR lives on: one [`install`]ed for this board, else the real one.
pub fn host_for(app: &App, host: &str) -> HostResult<Arc<dyn PrHost>> {
    if let Some(h) = app.shared.lock().pr_hosts.get(host) {
        return Ok(h.clone());
    }
    match host {
        "github" => {
            let gh = crate::proc::which(&app.cfg.pr.gh).ok_or_else(|| "the gh command isn't installed".to_string())?;
            Ok(Arc::new(github::GithubHost::new(Box::new(github::GhCli { bin: gh, env: crate::accounts::gh_env(&app.cfg) }))))
        }
        "bitbucket" => {
            let (email, token) = crate::accounts::board_credentials(app, crate::accounts::Provider::Bitbucket)
                .ok_or_else(|| "Bitbucket isn't connected: connect it in Taskboard ▸ Settings ▸ Accounts".to_string())?;
            Ok(Arc::new(bitbucket::BitbucketHost::new(Box::new(bitbucket::BasicHttp::new(&email, &token)), &app.cfg.pr.bitbucket_api)))
        }
        "" => Err("this PR's host isn't known".into()),
        other => Err(format!("the board doesn't work with {other} PRs")),
    }
}

/// Uses `host` for every PR on its host id on this board (tests).
pub fn install(app: &App, host: Arc<dyn PrHost>) {
    app.shared.lock().pr_hosts.insert(host.id().to_string(), host);
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
}

impl FakeHost {
    pub fn new(host: &'static str, rec: Record) -> Arc<FakeHost> {
        Arc::new(FakeHost { host, rec: Mutex::new(rec), calls: Mutex::new(vec![]), base_failing: Mutex::new(vec![]) })
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
        let author = r.author.clone();
        if let Some(t) = r.threads.iter_mut().find(|t| t.id == thread.id) {
            t.last_author = author;
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
