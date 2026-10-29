//! Bitbucket Cloud through its REST API (2.0), with Taskboard's Bitbucket account (email + API token,
//! basic auth; see `accounts.rs`).
//!
//! A PR's threads are its top-level comments with their replies (resolved when the top comment has a
//! `resolution`), and its PR tasks. Reviewers are the PR's `reviewers` plus every participant who
//! approved or asked for changes.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value};

use super::{gist, whole, Cancelled, Check, Comment, HostResult, Look, MergeOpts, OpenPr, PrHost, PrRef, Record, Reply, Reviewer, Thread, REPLY_MAX};

/// One HTTP call: method, full URL and JSON body; answers the JSON reply (Null when it's empty). An
/// error starts with "Bitbucket answered <code>" when Bitbucket answered.
pub trait Http: Send + Sync {
    fn call(&self, method: &str, url: &str, body: Option<&Value>) -> HostResult<Value>;
    /// The account `call` signs in as, as the rate limits know it (`limit::Gate::key`).
    fn limit_key(&self) -> Option<String> {
        None
    }
}

/// JSON over HTTPS with basic auth (Bitbucket, and Azure DevOps in `ci.rs`).
pub struct BasicHttp {
    auth: String,
    agent: ureq::Agent,
    service: &'static str,
    /// The account's rate limit and budget (`prhost::limit`), shared with every other reader on it.
    gate: super::limit::Gate,
}

impl BasicHttp {
    pub fn new(email: &str, token: &str) -> BasicHttp {
        BasicHttp::for_service("Bitbucket", email, token)
    }

    pub fn for_service(service: &'static str, user: &str, token: &str) -> BasicHttp {
        let auth = format!("Basic {}", crate::jira::base64_lite::encode(format!("{user}:{token}").as_bytes()));
        let host = service.to_lowercase();
        let gate = super::limit::Gate::new(&host, super::account_key(&host, &[user, token]));
        BasicHttp { auth, agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(20)).build(), service, gate }
    }
}

/// What a response's headers say about the rate limit: a `Retry-After` (seconds), or
/// `X-RateLimit-Remaining: 0` with its `X-RateLimit-Reset` (epoch seconds).
fn limit_headers(r: &ureq::Response, limited: bool, why: &str) -> Option<super::limit::Hit> {
    let num = |h: &str| r.header(h).and_then(|v| v.trim().parse::<f64>().ok());
    let spent = num("X-RateLimit-Remaining").is_some_and(|n| n <= 0.0);
    if !limited && !spent {
        return None;
    }
    let headers = r
        .headers_names()
        .into_iter()
        .filter(|h| h.eq_ignore_ascii_case("Retry-After") || h.to_ascii_lowercase().starts_with("x-ratelimit-"))
        .filter_map(|h| r.header(&h).map(|v| (h.clone(), v.trim().to_string())))
        .collect();
    Some(super::limit::Hit { retry_after: num("Retry-After"), reset_at: num("X-RateLimit-Reset"), why: why.to_string(), headers })
}

/// What a response's headers say about the hour's calls left, when it sends them: its
/// `X-RateLimit-Limit`, and whether it's near it (`X-RateLimit-NearLimit: true`, or
/// `X-RateLimit-Remaining` under a fifth of the limit).
fn headroom(r: &ureq::Response) -> Option<super::limit::Headroom> {
    let num = |h: &str| r.header(h).and_then(|v| v.trim().parse::<f64>().ok());
    let (cap, left) = (num("X-RateLimit-Limit"), num("X-RateLimit-Remaining"));
    let flagged = r.header("X-RateLimit-NearLimit").map(|v| v.trim().eq_ignore_ascii_case("true"));
    if cap.is_none() && left.is_none() && flagged.is_none() {
        return None;
    }
    let low = matches!((left, cap), (Some(l), Some(c)) if c > 0.0 && l < c * super::limit::NEAR_SHARE);
    Some(super::limit::Headroom { cap: cap.filter(|c| *c > 0.0).map(|c| c as u32), near: flagged == Some(true) || low, reset_at: num("X-RateLimit-Reset") })
}

/// Bitbucket's own count of the account's calls (`X-Request-Count`), on any answer.
fn request_count(r: &ureq::Response) -> Option<u64> {
    r.header("X-Request-Count").and_then(|v| v.trim().parse().ok())
}

impl Http for BasicHttp {
    fn limit_key(&self) -> Option<String> {
        Some(self.gate.key.clone())
    }
    /// A GET is shared on the account with the same read in flight or kept lately; anything else is a
    /// write, which then drops the reads it may change (`limit` "Sharing reads").
    fn call(&self, method: &str, url: &str, body: Option<&Value>) -> HostResult<Value> {
        self.gate.request(method, url, || self.send(method, url, body))
    }
}

impl BasicHttp {
    /// One `tb api` call (`super::rest`) through the account's gate: `{"status", "body"}` with the body
    /// as the host sent it, whatever the status, except a rate limit, which the gate backs off on. A
    /// read isn't shared with the board's own (their answers differ); a write still drops the reads it
    /// may change.
    pub fn raw(&self, method: &str, url: &str, body: Option<&str>) -> HostResult<Value> {
        let go = || {
            let req = self.agent.request(method, url).set("Authorization", &self.auth).set("Accept", "application/json");
            let res = match body {
                Some(b) => req.set("Content-Type", "application/json; charset=utf-8").send_string(b),
                None => req.call(),
            };
            let (code, r) = match res {
                Ok(r) => (r.status(), r),
                Err(ureq::Error::Status(code, r)) => (code, r),
                Err(_) => return Err((format!("Can't reach {}", self.service), None)),
            };
            if let Some(n) = request_count(&r) {
                self.gate.request_count(n);
            }
            let hit = limit_headers(&r, code == 429, &format!("{} answered {code}", self.service));
            if let Some(h) = headroom(&r).filter(|_| hit.is_none()) {
                self.gate.headroom(&h);
            }
            if code == 429 {
                return Err((format!("{} answered 429", self.service), hit));
            }
            Ok((json!({"status": code, "body": r.into_string().unwrap_or_default()}), hit))
        };
        match method {
            "GET" => self.gate.call(false, go),
            _ => self.gate.request(method, url, go),
        }
    }

    /// One call to the host, as the gate takes it: the answer, and the rate limit its headers report.
    fn send(&self, method: &str, url: &str, body: Option<&Value>) -> std::result::Result<(Value, Option<super::limit::Hit>), (String, Option<super::limit::Hit>)> {
        let svc = self.service;
        let req = self.agent.request(method, url).set("Authorization", &self.auth).set("Accept", "application/json");
        let res = match body {
            Some(b) => req.send_json(b.clone()),
            None => req.call(),
        };
        match res {
            Ok(r) => {
                if let Some(n) = request_count(&r) {
                    self.gate.request_count(n);
                }
                let hit = limit_headers(&r, false, &format!("{svc} said the account has no calls left this hour"));
                if let Some(h) = headroom(&r).filter(|_| hit.is_none()) {
                    self.gate.headroom(&h);
                }
                let text = r.into_string().unwrap_or_default();
                if text.trim().is_empty() {
                    return Ok((Value::Null, hit));
                }
                Ok((serde_json::from_str(&text).unwrap_or(Value::Null), hit))
            }
            Err(ureq::Error::Status(code, r)) => {
                if let Some(n) = request_count(&r) {
                    self.gate.request_count(n);
                }
                let hit = limit_headers(&r, code == 429, &format!("{svc} answered {code}"));
                let v: Value = r.into_json().unwrap_or(Value::Null);
                let why = v["error"]["message"].as_str().unwrap_or("").to_string();
                Err((
                    match code {
                        401 if svc == "Bitbucket" => "Bitbucket answered 401: it didn't accept Taskboard's Bitbucket token; sign in again in Settings ▸ Accounts".to_string(),
                        _ if why.is_empty() => format!("{svc} answered {code}"),
                        _ => format!("{svc} answered {code}: {why}"),
                    },
                    hit,
                ))
            }
            Err(_) => Err((format!("Can't reach {svc}"), None)),
        }
    }
}

pub struct BitbucketHost {
    http: Box<dyn Http>,
    api: String,
    /// Which account `http` signs in as, for caching who it is (`prhost::account_key`); empty: ask each read.
    account: String,
}

/// A Bitbucket id in a URL path (`{uuid}` braces escaped).
fn seg(s: &str) -> String {
    s.replace('{', "%7B").replace('}', "%7D")
}

/// A user for Bitbucket's reviewer list: a `{uuid}` or an `account_id`.
fn user_ref(u: &str) -> Value {
    if u.starts_with('{') {
        json!({"uuid": u})
    } else {
        json!({"account_id": u})
    }
}

fn uid(u: &Value) -> String {
    u["uuid"].as_str().or(u["account_id"].as_str()).unwrap_or("").to_string()
}

/// Both of a user's ids, when Bitbucket gives both: the roster may name them by either.
fn ids(u: &Value) -> Vec<String> {
    let all: Vec<String> = [&u["uuid"], &u["account_id"]].iter().filter_map(|v| v.as_str()).filter(|v| !v.is_empty()).map(|v| v.to_string()).collect();
    if all.len() > 1 { all } else { vec![] }
}

fn same_user(u: &Value, id: &str) -> bool {
    u["uuid"].as_str() == Some(id) || u["account_id"].as_str() == Some(id)
}

impl BitbucketHost {
    pub fn new(http: Box<dyn Http>, api: &str) -> BitbucketHost {
        BitbucketHost { http, api: api.trim_end_matches('/').to_string(), account: String::new() }
    }

    /// Names the account `http` signs in as, so who it is is asked once.
    pub fn with_account(mut self, key: &str) -> BitbucketHost {
        self.account = key.to_string();
        self
    }

    /// The board's own Bitbucket account (`GET /user`): its `{uuid}`, as comments name their authors.
    fn viewer(&self) -> String {
        super::viewer_cached(&self.account, || Ok(uid(&self.get(&format!("{}/user", self.api))?)))
    }

    fn repo_url(&self, repo: &str) -> String {
        format!("{}/repositories/{repo}", self.api)
    }

    fn pr_url(&self, pr: &PrRef) -> String {
        format!("{}/pullrequests/{}", self.repo_url(&pr.repo), pr.num)
    }

    fn get(&self, url: &str) -> HostResult<Value> {
        self.http.call("GET", url, None)
    }

    /// Every page's `values` (at most `pages` pages).
    fn get_all(&self, url: &str, pages: usize) -> HostResult<Vec<Value>> {
        let mut out = vec![];
        let mut next = Some(url.to_string());
        for _ in 0..pages {
            let Some(u) = next.take() else { break };
            let v = self.get(&u)?;
            out.extend(v["values"].as_array().cloned().unwrap_or_default());
            next = v["next"].as_str().map(|s| s.to_string());
        }
        Ok(out)
    }

    /// Whether the PR merges cleanly into its base, as a bool: Bitbucket has no such flag, but its diffstat
    /// marks each conflicting file. Null when the diffstat can't be read whole. A rate limit fails the read
    /// instead: an unknown doesn't hold a merge, so a PR read while limited went on to merge as if it
    /// had no conflicts.
    fn mergeable(&self, pr: &PrRef) -> HostResult<Value> {
        let mut next = Some(format!("{}/diffstat?pagelen=100&fields=next,values.status", self.pr_url(pr)));
        for _ in 0..DIFFSTAT_PAGES {
            let Some(u) = next.take() else { break };
            let v = match self.get(&u) {
                Ok(v) => v,
                Err(e) if self.held_until().is_some() => return Err(e),
                Err(_) => return Ok(Value::Null),
            };
            if v["values"].as_array().is_some_and(|a| a.iter().any(|d| conflicted(&d["status"]))) {
                return Ok(Value::Bool(false));
            }
            next = v["next"].as_str().map(|s| s.to_string());
        }
        Ok(if next.is_some() { Value::Null } else { Value::Bool(true) })
    }

    /// Sets the PR's reviewers to exactly these, last-wins: a newer list replaces this one while it waits.
    fn put_reviewers(&self, pr: &PrRef, p: &Value, reviewers: Vec<Value>) -> HostResult<()> {
        let url = self.pr_url(pr);
        super::limit::last_wins(format!("{url} reviewers"), || self.send_reviewers(&url, p, reviewers))
    }

    /// Sets the PR's reviewers to exactly these, going out even when a newer list follows.
    fn send_reviewers(&self, url: &str, p: &Value, reviewers: Vec<Value>) -> HostResult<()> {
        self.http.call("PUT", url, Some(&json!({"title": p["title"], "reviewers": reviewers}))).map(|_| ())
    }

    fn current_reviewers(p: &Value) -> Vec<Value> {
        p["reviewers"].as_array().cloned().unwrap_or_default().iter().map(|r| user_ref(&uid(r))).filter(|r| r != &user_ref("")).collect()
    }

    fn statuses(&self, repo: &str, commit: &str) -> HostResult<Vec<Value>> {
        self.get_all(&format!("{}/commit/{commit}/statuses?pagelen=100&fields={STATUS_FIELDS}", self.repo_url(repo)), 3)
    }

    /// A Pipelines run by its build number (the number in its link).
    fn pipeline(&self, repo: &str, build: i64) -> HostResult<Value> {
        let runs = self.get_all(&format!("{}/pipelines/?sort=-created_on&pagelen=50", self.repo_url(repo)), 2)?;
        runs.into_iter().find(|r| r["build_number"].as_i64() == Some(build)).ok_or_else(|| format!("Bitbucket Pipelines has no run #{build} lately"))
    }

    /// A Pipelines run's failed steps and the failing tests their reports name.
    pub fn pipeline_failures(&self, repo: &str, build: i64) -> HostResult<(Vec<String>, Vec<String>)> {
        let run = self.pipeline(repo, build)?;
        let uuid = run["uuid"].as_str().unwrap_or("").to_string();
        let steps = self.get_all(&format!("{}/pipelines/{}/steps/?pagelen=100", self.repo_url(repo), seg(&uuid)), 2)?;
        let mut failed = vec![];
        let mut tests = vec![];
        for s in steps {
            if !matches!(s["state"]["result"]["name"].as_str(), Some("FAILED" | "ERROR")) {
                continue;
            }
            failed.push(s["name"].as_str().unwrap_or("step").to_string());
            let su = s["uuid"].as_str().unwrap_or("");
            // Test reports are there only when the step published them; a missing one isn't an error.
            if let Ok(cases) = self.get_all(
                &format!("{}/pipelines/{}/steps/{}/test_reports/test_cases?status=FAILED&pagelen=50", self.repo_url(repo), seg(&uuid), seg(su)),
                1,
            ) {
                tests.extend(cases.iter().filter_map(|c| c["fully_qualified_name"].as_str().or(c["name"].as_str()).map(|x| x.to_string())));
            }
        }
        Ok((failed, tests))
    }
}

/// PRs per list call: one page of them.
const STAMP_BATCH: usize = 50;
/// What a stamp reads of each PR in the list: whatever a full read would see change, short of its
/// checks (a build doesn't move `updated_on`).
const STAMP_FIELDS: &str = "next,values.id,values.state,values.title,values.updated_on,values.source.commit.hash,values.destination.branch.name,values.destination.commit.hash,values.comment_count,values.task_count,values.reviewers.uuid,values.participants.user.uuid,values.participants.approved,values.participants.state,values.participants.participated_on";

/// What a full read uses of the PR (`summarize`): Bitbucket answers only these (#743).
const PR_FIELDS: &str = "state,title,author.uuid,author.account_id,source.branch.name,source.commit.hash,destination.branch.name,destination.commit.hash,reviewers.uuid,reviewers.account_id,reviewers.display_name,participants.user.uuid,participants.user.account_id,participants.user.display_name,participants.state,participants.approved,participants.participated_on";
/// What a full read uses of each comment, and of each PR task.
const COMMENT_FIELDS: &str = "next,values.id,values.parent.id,values.deleted,values.pending,values.inline.path,values.inline.from,values.inline.to,values.resolution,values.content.raw,values.user.uuid,values.user.account_id,values.user.display_name,values.created_on,values.links.html.href";
const TASK_FIELDS: &str = "next,values.id,values.state,values.content.raw,values.creator.uuid,values.creator.account_id,values.creator.display_name,values.comment.id,values.created_on";
/// What a check uses of each build status (`checks_of`).
const STATUS_FIELDS: &str = "next,values.key,values.name,values.state,values.url,values.created_on,values.updated_on";

impl PrHost for BitbucketHost {
    fn id(&self) -> &'static str {
        "bitbucket"
    }
    fn held_until(&self) -> Option<f64> {
        super::limit::held_until(&self.http.limit_key()?)
    }

    fn checks(&self, pr: &PrRef, head: &str) -> HostResult<Option<Vec<Check>>> {
        Ok(Some(checks_of(&self.statuses(&pr.repo, head)?)))
    }

    fn stamps(&self, looks: &[Look]) -> HostResult<Vec<Option<String>>> {
        let mut out = vec![None; looks.len()];
        let mut repos: Vec<(&str, Vec<usize>)> = vec![];
        for (i, l) in looks.iter().enumerate() {
            match repos.iter_mut().find(|(r, _)| *r == l.pr.repo) {
                Some((_, at)) => at.push(i),
                None => repos.push((&l.pr.repo, vec![i])),
            }
        }
        for (repo, at) in repos {
            for chunk in at.chunks(STAMP_BATCH) {
                let q = chunk.iter().map(|&i| format!("id%3D{}", looks[i].pr.num)).collect::<Vec<_>>().join("%20OR%20");
                let url = format!(
                    "{}/pullrequests?q={q}&state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED&pagelen={STAMP_BATCH}&fields={STAMP_FIELDS}",
                    self.repo_url(repo)
                );
                let list = self.get(&url)?;
                let prs = list["values"].as_array().cloned().unwrap_or_default();
                for &i in chunk {
                    let l = &looks[i];
                    let Some(p) = prs.iter().find(|p| p["id"].as_i64() == Some(l.pr.num)) else { continue };
                    let head = p["source"]["commit"]["hash"].as_str().unwrap_or("");
                    let statuses = if l.checks && !head.is_empty() { Value::Array(self.statuses(repo, head)?) } else { Value::Null };
                    out[i] = Some(json!({"pr": p, "statuses": statuses}).to_string());
                }
            }
        }
        Ok(out)
    }

    fn read(&self, pr: &PrRef) -> HostResult<Record> {
        let p = self.get(&format!("{}?fields={PR_FIELDS}", self.pr_url(pr)))?;
        let comments = self.get_all(&format!("{}/comments?pagelen=100&fields={COMMENT_FIELDS}", self.pr_url(pr)), 5)?;
        // PR tasks need a newer token scope on some workspaces. Tasks that can't be read are unknown, not
        // none: the merge waits on them (`prcmds::merge_blockers`).
        let tasks = self.get_all(&format!("{}/tasks?pagelen=100&fields={TASK_FIELDS}", self.pr_url(pr)), 3);
        let head = p["source"]["commit"]["hash"].as_str().unwrap_or("");
        let statuses = if head.is_empty() { vec![] } else { self.statuses(&pr.repo, head)? };
        let mut r = summarize(&p, &comments, tasks.as_deref().unwrap_or(&[]), &statuses);
        r.tasks_error = tasks.err();
        r.mergeable = self.mergeable(pr)?;
        r.viewer = self.viewer();
        r.leave_out_viewer();
        Ok(r)
    }

    fn reply(&self, pr: &PrRef, thread: &Thread, body: &str) -> HostResult<()> {
        let url = format!("{}/comments", self.pr_url(pr));
        let parent = match thread.kind.as_str() {
            "task" => thread.last_id.strip_prefix("comment-").map(|s| s.to_string()),
            _ => Some(thread.id.clone()),
        };
        let b = match parent.and_then(|p| p.parse::<i64>().ok()) {
            Some(id) => json!({"content": {"raw": body}, "parent": {"id": id}}),
            None => json!({"content": {"raw": format!("> {}\n\n{body}", thread.text)}}),
        };
        self.http.call("POST", &url, Some(&b)).map(|_| ())
    }

    fn resolve(&self, pr: &PrRef, thread: &Thread) -> HostResult<bool> {
        if let Some(task) = thread.id.strip_prefix("task-") {
            self.http.call("PUT", &format!("{}/tasks/{task}", self.pr_url(pr)), Some(&json!({"state": "RESOLVED"})))?;
            return Ok(true);
        }
        self.http.call("POST", &format!("{}/comments/{}/resolve", self.pr_url(pr), thread.id), None)?;
        Ok(true)
    }

    fn request_reviews(&self, pr: &PrRef, users: &[String]) -> HostResult<()> {
        let p = self.get(&self.pr_url(pr))?;
        let mut list = Self::current_reviewers(&p);
        for u in users {
            if !list.iter().any(|r| same_user(r, u)) {
                list.push(user_ref(u));
            }
        }
        self.put_reviewers(pr, &p, list)
    }

    fn re_request_reviews(&self, pr: &PrRef, users: &[String]) -> HostResult<()> {
        // Bitbucket has no "ask again": taking someone off the reviewers and putting them back clears
        // their old approval or request for changes, and notifies them. So the taking off isn't
        // last-wins: a newer list replacing it would leave them on, and the putting back would do nothing.
        let p = self.get(&self.pr_url(pr))?;
        let list = Self::current_reviewers(&p);
        let without: Vec<Value> = list.iter().filter(|r| !users.iter().any(|u| same_user(r, u))).cloned().collect();
        self.send_reviewers(&self.pr_url(pr), &p, without.clone())?;
        let mut with = without;
        with.extend(users.iter().map(|u| user_ref(u)));
        self.put_reviewers(pr, &p, with)
    }

    fn remove_reviewers(&self, pr: &PrRef, users: &[String]) -> HostResult<()> {
        let p = self.get(&self.pr_url(pr))?;
        let list = Self::current_reviewers(&p).into_iter().filter(|r| !users.iter().any(|u| same_user(r, u))).collect();
        self.put_reviewers(pr, &p, list)
    }

    fn merge(&self, pr: &PrRef, opts: &MergeOpts) -> HostResult<()> {
        let mut b = json!({"type": "pullrequest", "close_source_branch": opts.close_source_branch});
        if let Some(s) = &opts.strategy {
            let s = match s.as_str() {
                "merge" => "merge_commit",
                "rebase" => "fast_forward",
                other => other,
            };
            b["merge_strategy"] = json!(s);
        }
        self.http.call("POST", &format!("{}/merge", self.pr_url(pr)), Some(&b)).map(|_| ())
    }

    fn close(&self, pr: &PrRef, comment: Option<&str>) -> HostResult<()> {
        if let Some(c) = comment.filter(|c| !c.trim().is_empty()) {
            self.http.call("POST", &format!("{}/comments", self.pr_url(pr)), Some(&json!({"content": {"raw": c}})))?;
        }
        self.http.call("POST", &format!("{}/decline", self.pr_url(pr)), None).map(|_| ())
    }

    fn open(&self, repo: &str, base: &str, branch: &str, title: &str, body: &str) -> HostResult<PrRef> {
        // Closes the branch once the PR merges, as the Python board did.
        let b = json!({"title": title, "description": body, "source": {"branch": {"name": branch}}, "destination": {"branch": {"name": base}},
                       "close_source_branch": true});
        let v = self.http.call("POST", &format!("{}/pullrequests", self.repo_url(repo)), Some(&b))?;
        let num = v["id"].as_i64().ok_or_else(|| "Bitbucket didn't say which PR it opened".to_string())?;
        let url = v["links"]["html"]["href"].as_str().map(|s| s.to_string()).unwrap_or_else(|| format!("https://bitbucket.org/{repo}/pull-requests/{num}"));
        Ok(PrRef { host: "bitbucket".into(), repo: repo.to_string(), num, url })
    }

    fn retarget(&self, pr: &PrRef, base: &str) -> HostResult<()> {
        let url = self.pr_url(pr);
        let p = self.get(&url)?;
        super::limit::last_wins(format!("{url} base"), || self.http.call("PUT", &url, Some(&json!({"title": p["title"], "destination": {"branch": {"name": base}}})))).map(|_| ())
    }

    fn description(&self, pr: &PrRef) -> HostResult<String> {
        Ok(self.get(&format!("{}?fields=description", self.pr_url(pr)))?["description"].as_str().unwrap_or("").to_string())
    }

    fn set_description(&self, pr: &PrRef, body: &str) -> HostResult<()> {
        let url = self.pr_url(pr);
        let p = self.get(&url)?;
        super::limit::last_wins(format!("{url} description"), || self.http.call("PUT", &url, Some(&json!({"title": p["title"], "description": body})))).map(|_| ())
    }

    fn files(&self, pr: &PrRef) -> HostResult<Vec<String>> {
        let stats = self.get_all(&format!("{}/diffstat?pagelen=100&fields=next,values.new.path,values.old.path", self.pr_url(pr)), 3)?;
        Ok(stats.iter().filter_map(|d| d["new"]["path"].as_str().or_else(|| d["old"]["path"].as_str())).map(str::to_string).collect())
    }

    fn commits(&self, pr: &PrRef) -> HostResult<Vec<String>> {
        let list = self.get_all(&format!("{}/commits?pagelen=100&fields=next,values.hash", self.pr_url(pr)), 3)?;
        Ok(list.iter().filter_map(|c| c["hash"].as_str()).map(str::to_string).collect())
    }

    fn cancel_builds(&self, pr: &PrRef, head: &str) -> HostResult<Cancelled> {
        let runs = match self.get_all(&format!("{}/pipelines/?sort=-created_on&pagelen=50", self.repo_url(&pr.repo)), 1) {
            Ok(r) => r,
            Err(e) if e.contains("answered 404") || e.contains("answered 403") => {
                return Ok(Cancelled::Unsupported("this repository's builds don't run on Bitbucket Pipelines; stop them in its CI".into()))
            }
            Err(e) => return Err(e),
        };
        let mut stopped = vec![];
        for r in runs {
            let target = &r["target"];
            let hash = target["commit"]["hash"].as_str().unwrap_or("");
            let same = !hash.is_empty() && !head.is_empty() && (hash.starts_with(head) || head.starts_with(hash));
            // A pull-request pipeline runs for the PR by its number, whatever commit it built.
            let this_pr = pr.num > 0 && [&target["pullrequest"], &target["pull_request"]].iter().any(|p| p["id"].as_i64() == Some(pr.num));
            if !(same || this_pr) || !matches!(r["state"]["name"].as_str(), Some("PENDING" | "IN_PROGRESS")) {
                continue;
            }
            let uuid = r["uuid"].as_str().unwrap_or("");
            self.http.call("POST", &format!("{}/pipelines/{}/stopPipeline", self.repo_url(&pr.repo), seg(uuid)), None)?;
            let name = target["selector"]["pattern"].as_str().or(target["ref_name"].as_str()).filter(|n| !n.is_empty()).unwrap_or("pipeline");
            stopped.push(format!("{name} #{}", r["build_number"].as_i64().unwrap_or(0)));
        }
        Ok(Cancelled::Stopped(stopped))
    }

    fn my_open_prs(&self, repo: &str) -> HostResult<Vec<OpenPr>> {
        let me = self.viewer();
        if me.is_empty() {
            return Err("Bitbucket didn't say which account the board signs in as".into());
        }
        let list = self.get_all(&format!("{}/pullrequests?state=OPEN&pagelen=50", self.repo_url(repo)), 4)?;
        Ok(list
            .iter()
            .filter(|p| same_user(&p["author"], &me))
            .filter_map(|p| {
                Some(OpenPr {
                    num: p["id"].as_i64()?,
                    branch: p["source"]["branch"]["name"].as_str().unwrap_or("").to_string(),
                    url: p["links"]["html"]["href"].as_str().unwrap_or("").to_string(),
                })
            })
            .collect())
    }

    fn members(&self, repo: &str) -> HostResult<Vec<Reviewer>> {
        let ws = repo.split('/').next().unwrap_or("");
        let list = self.get_all(&format!("{}/workspaces/{}/members?pagelen=100", self.api, seg(ws)), 5)?;
        Ok(list
            .iter()
            .map(|m| &m["user"])
            .filter(|u| !uid(u).is_empty())
            .map(|u| Reviewer { user: uid(u), name: u["display_name"].as_str().unwrap_or("").to_string(), ..Default::default() })
            .collect())
    }

    fn recent_comments(&self, repo: &str, prs: usize) -> HostResult<Vec<Comment>> {
        let list = self.get(&format!(
            "{}/pullrequests?state=OPEN&state=MERGED&state=DECLINED&sort=-updated_on&pagelen={}",
            self.repo_url(repo),
            prs.clamp(1, 50)
        ))?;
        let mut out = vec![];
        for p in list["values"].as_array().cloned().unwrap_or_default().into_iter().take(prs) {
            let Some(num) = p["id"].as_i64() else { continue };
            let comments = self.get(&format!("{}/pullrequests/{num}/comments?pagelen=100&sort=-created_on", self.repo_url(repo)))?;
            out.extend(comments["values"].as_array().cloned().unwrap_or_default().iter().filter_map(|c| comment_of(num, c)));
        }
        Ok(out)
    }

    fn base_failures(&self, pr: &PrRef, base: &str, commits: usize) -> HostResult<Vec<String>> {
        let list = self.get(&format!("{}/commits/{base}?pagelen={commits}", self.repo_url(&pr.repo)))?;
        let mut out: Vec<String> = vec![];
        for c in list["values"].as_array().cloned().unwrap_or_default().into_iter().take(commits) {
            let Some(hash) = c["hash"].as_str() else { continue };
            for s in self.statuses(&pr.repo, hash)? {
                if s["state"] == "FAILED" {
                    out.push(s["name"].as_str().or(s["key"].as_str()).unwrap_or("").to_string());
                }
            }
        }
        out.retain(|n| !n.is_empty());
        out.sort();
        out.dedup();
        Ok(out)
    }

    fn base_failed_checks(&self, pr: &PrRef, base: &str, commits: usize) -> HostResult<Vec<Check>> {
        let list = self.get(&format!("{}/commits/{base}?pagelen={commits}", self.repo_url(&pr.repo)))?;
        let mut out: Vec<Check> = vec![];
        for c in list["values"].as_array().cloned().unwrap_or_default().into_iter().take(commits) {
            let Some(hash) = c["hash"].as_str() else { continue };
            for s in self.statuses(&pr.repo, hash)?.into_iter().filter(|s| s["state"] == "FAILED") {
                let name = s["name"].as_str().filter(|n| !n.is_empty()).or(s["key"].as_str()).unwrap_or("").to_string();
                let url = s["url"].as_str().filter(|u| !u.is_empty()).map(|u| u.to_string());
                if !name.is_empty() && !out.iter().any(|o| o.name == name && o.url == url) {
                    out.push(Check { name, state: "failed".into(), url, at: status_at(&s) });
                }
            }
        }
        Ok(out)
    }
}

/// A build status's newest time: updated, else created. Bitbucket lists them unsorted.
/// Diffstat pages read for conflicts; a bigger PR's are unknown.
const DIFFSTAT_PAGES: usize = 5;

/// A diffstat status for a file that doesn't merge: both sides changed it, or one deleted what the other changed.
fn conflicted(status: &Value) -> bool {
    matches!(status.as_str(), Some("merge conflict" | "local deleted" | "remote deleted"))
}

fn status_at(s: &Value) -> Option<String> {
    ["updated_on", "created_on"].iter().filter_map(|k| s[*k].as_str()).find(|t| !t.is_empty()).map(|t| t.to_string())
}

fn check_state(s: &str) -> &'static str {
    match s {
        "SUCCESSFUL" => "passed",
        "FAILED" => "failed",
        "STOPPED" => "stopped",
        _ => "running",
    }
}

/// A PR, its comments, its tasks and its head's build statuses, as one record.
/// One of a PR's comments, whole (none for a deleted, draft or empty one).
fn comment_of(pr: i64, c: &Value) -> Option<Comment> {
    let text = c["content"]["raw"].as_str().unwrap_or("");
    if c["deleted"] == true || c["pending"] == true || text.trim().is_empty() {
        return None;
    }
    Some(Comment {
        pr,
        id: c["id"].as_i64().map(|i| i.to_string()).unwrap_or_default(),
        author: uid(&c["user"]),
        author_name: c["user"]["display_name"].as_str().unwrap_or("").to_string(),
        at: c["created_on"].as_str().unwrap_or("").to_string(),
        text: text.to_string(),
    })
}

/// A commit's statuses as checks.
fn checks_of(statuses: &[Value]) -> Vec<Check> {
    statuses
        .iter()
        .map(|s| Check {
            name: s["name"].as_str().filter(|n| !n.is_empty()).or(s["key"].as_str()).unwrap_or("build").to_string(),
            state: check_state(s["state"].as_str().unwrap_or("")).to_string(),
            url: s["url"].as_str().filter(|u| !u.is_empty()).map(|u| u.to_string()),
            at: status_at(s),
        })
        .collect()
}

pub fn summarize(p: &Value, comments: &[Value], tasks: &[Value], statuses: &[Value]) -> Record {
    let author = uid(&p["author"]);
    let checks = checks_of(statuses);
    let mut reviewers: Vec<Reviewer> = vec![];
    for r in p["reviewers"].as_array().cloned().unwrap_or_default() {
        let user = uid(&r);
        if user.is_empty() || user == author {
            continue;
        }
        reviewers.push(Reviewer { user, name: r["display_name"].as_str().unwrap_or("").to_string(), state: "pending".into(), requested: true, changes_at: None, ids: ids(&r) });
    }
    let mut changes_at: Option<String> = None;
    for part in p["participants"].as_array().cloned().unwrap_or_default() {
        let user = uid(&part["user"]);
        if user.is_empty() || user == author {
            continue;
        }
        let state = match (part["state"].as_str(), part["approved"].as_bool()) {
            (Some("approved"), _) | (_, Some(true)) => "approved",
            (Some("changes_requested"), _) => "changes",
            _ => "commented",
        };
        let mine = (state == "changes").then(|| part["participated_on"].as_str().unwrap_or("").to_string());
        if let Some(at) = &mine {
            if changes_at.as_deref().is_none_or(|c| at.as_str() > c) {
                changes_at = Some(at.clone());
            }
        }
        match reviewers.iter_mut().find(|x| x.user == user) {
            Some(x) => {
                if state != "commented" {
                    x.state = state.into();
                    x.changes_at = mine;
                }
            }
            None if state != "commented" => reviewers.push(Reviewer {
                user,
                name: part["user"]["display_name"].as_str().unwrap_or("").to_string(),
                state: state.into(),
                requested: false,
                changes_at: mine,
                ids: ids(&part["user"]),
            }),
            None => {}
        }
    }
    let approvals = reviewers.iter().filter(|r| r.state == "approved").count() as i64;
    let changes = reviewers.iter().any(|r| r.state == "changes");

    let live: Vec<&Value> = comments.iter().filter(|c| c["pending"] != true).collect();
    let by_id: HashMap<i64, &Value> = live.iter().filter_map(|c| c["id"].as_i64().map(|i| (i, *c))).collect();
    let root_of = |c: &Value| -> i64 {
        let mut cur = c;
        for _ in 0..50 {
            match cur["parent"]["id"].as_i64().and_then(|p| by_id.get(&p)) {
                Some(up) => cur = up,
                None => break,
            }
        }
        cur["id"].as_i64().unwrap_or(0)
    };
    let mut groups: HashMap<i64, Vec<&Value>> = HashMap::new();
    for c in &live {
        groups.entry(root_of(c)).or_default().push(c);
    }
    let mut threads: Vec<Thread> = vec![];
    for c in live.iter().filter(|c| c["parent"]["id"].as_i64().is_none()) {
        let id = c["id"].as_i64().unwrap_or(0);
        let group = groups.get(&id).cloned().unwrap_or_default();
        let spoken: Vec<&&Value> = group.iter().filter(|x| x["deleted"] != true).collect();
        if spoken.is_empty() {
            continue;
        }
        let last = spoken.iter().max_by_key(|x| x["created_on"].as_str().unwrap_or("").to_string()).unwrap();
        let mut later: Vec<&&Value> = spoken.iter().copied().filter(|x| x["id"].as_i64() != Some(id)).collect();
        later.sort_by_key(|x| x["created_on"].as_str().unwrap_or("").to_string());
        let replies = later
            .iter()
            .map(|x| Reply {
                author: uid(&x["user"]),
                author_name: x["user"]["display_name"].as_str().unwrap_or("").to_string(),
                text: crate::util::one_line(x["content"]["raw"].as_str().unwrap_or(""), REPLY_MAX),
                body: whole(x["content"]["raw"].as_str().unwrap_or("")),
                at: x["created_on"].as_str().unwrap_or("").to_string(),
            })
            .collect();
        threads.push(Thread {
            id: id.to_string(),
            kind: if c["inline"].is_object() { "review" } else { "comment" }.into(),
            resolvable: true,
            resolved: c["resolution"].is_object(),
            author: uid(&c["user"]),
            author_name: c["user"]["display_name"].as_str().unwrap_or("").to_string(),
            last_author: uid(&last["user"]),
            last_id: last["id"].as_i64().unwrap_or(0).to_string(),
            last_at: last["created_on"].as_str().unwrap_or("").to_string(),
            path: c["inline"]["path"].as_str().map(|s| s.to_string()),
            line: c["inline"]["to"].as_i64().or(c["inline"]["from"].as_i64()),
            text: gist(c["content"]["raw"].as_str().unwrap_or("")),
            body: whole(c["content"]["raw"].as_str().unwrap_or("")),
            url: c["links"]["html"]["href"].as_str().map(|s| s.to_string()),
            outdated: false,
            replies,
        });
    }
    let mut tasks_open = 0;
    for t in tasks {
        let open = t["state"].as_str() != Some("RESOLVED");
        if open {
            tasks_open += 1;
        }
        threads.push(Thread {
            id: format!("task-{}", t["id"].as_i64().unwrap_or(0)),
            kind: "task".into(),
            resolvable: true,
            resolved: !open,
            author: uid(&t["creator"]),
            author_name: t["creator"]["display_name"].as_str().unwrap_or("").to_string(),
            last_author: uid(&t["creator"]),
            // A task on a comment is answered on that comment.
            last_id: t["comment"]["id"].as_i64().map(|c| format!("comment-{c}")).unwrap_or_default(),
            last_at: t["created_on"].as_str().unwrap_or("").to_string(),
            path: None,
            line: None,
            text: gist(t["content"]["raw"].as_str().unwrap_or("")),
            body: whole(t["content"]["raw"].as_str().unwrap_or("")),
            url: None,
            outdated: false,
            replies: vec![],
        });
    }
    let comments_n = live.iter().filter(|c| c["deleted"] != true && uid(&c["user"]) != author).count() as i64;
    let state = p["state"].as_str().unwrap_or("OPEN").to_string();
    Record {
        state,
        title: p["title"].as_str().unwrap_or("").to_string(),
        author,
        head: p["source"]["commit"]["hash"].as_str().unwrap_or("").to_string(),
        branch: p["source"]["branch"]["name"].as_str().unwrap_or("").to_string(),
        base: p["destination"]["branch"]["name"].as_str().unwrap_or("").to_string(),
        base_head: p["destination"]["commit"]["hash"].as_str().unwrap_or("").to_string(),
        checks,
        review_decision: if changes { "CHANGES_REQUESTED".into() } else { String::new() },
        approvals,
        changes_at: if changes { changes_at } else { None },
        comments: comments_n,
        reviewers,
        threads,
        tasks_open,
        tasks_error: None,
        viewer: String::new(),
        mergeable: Value::Null,
    }
}

/// The build number in a Bitbucket Pipelines link (`…/pipelines/results/<n>`).
pub fn pipeline_build(url: &str) -> Option<i64> {
    let re = regex::Regex::new(r"bitbucket\.org/.+/pipelines/results/(\d+)").unwrap();
    re.captures(url).and_then(|c| c[1].parse().ok())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::Arc;

    #[test]
    fn rate_limit_headers_say_until_when() {
        let r = |raw: &str| raw.parse::<ureq::Response>().unwrap();
        let limited = r("HTTP/1.1 429 Too Many Requests\r\nRetry-After: 120\r\n\r\n");
        assert_eq!(limit_headers(&limited, true, "Bitbucket answered 429").map(|h| h.retry_after), Some(Some(120.0)));
        let spent = r("HTTP/1.1 200 OK\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: 1800000900\r\n\r\n{}");
        assert_eq!(limit_headers(&spent, false, "x").map(|h| h.reset_at), Some(Some(1_800_000_900.0)));
        assert_eq!(limit_headers(&r("HTTP/1.1 200 OK\r\nX-RateLimit-Remaining: 12\r\n\r\n{}"), false, "x"), None);
    }

    #[test]
    fn rate_limit_headers_say_when_the_account_is_near_its_limit() {
        let r = |raw: &str| raw.parse::<ureq::Response>().unwrap();
        assert_eq!(headroom(&r("HTTP/1.1 200 OK\r\n\r\n{}")), None, "no headers, nothing to go on");
        let plenty = headroom(&r("HTTP/1.1 200 OK\r\nX-RateLimit-Limit: 1000\r\nX-RateLimit-NearLimit: false\r\n\r\n{}")).unwrap();
        assert_eq!((plenty.cap, plenty.near), (Some(1000), false));
        assert!(headroom(&r("HTTP/1.1 200 OK\r\nX-RateLimit-Limit: 1000\r\nX-RateLimit-NearLimit: true\r\n\r\n{}")).unwrap().near);
        assert!(headroom(&r("HTTP/1.1 200 OK\r\nX-RateLimit-Limit: 1000\r\nX-RateLimit-Remaining: 150\r\n\r\n{}")).unwrap().near);
        assert!(!headroom(&r("HTTP/1.1 200 OK\r\nX-RateLimit-Limit: 1000\r\nX-RateLimit-Remaining: 250\r\n\r\n{}")).unwrap().near);
    }

    #[test]
    fn the_request_count_header_is_read_from_any_answer() {
        let r = |raw: &str| raw.parse::<ureq::Response>().unwrap();
        assert_eq!(request_count(&r("HTTP/1.1 200 OK\r\nX-Request-Count: 2189\r\n\r\n{}")), Some(2189));
        assert_eq!(request_count(&r("HTTP/1.1 429 Too Many Requests\r\nx-request-count: 3001\r\n\r\n")), Some(3001));
        assert_eq!(request_count(&r("HTTP/1.1 200 OK\r\n\r\n{}")), None);
        assert_eq!(request_count(&r("HTTP/1.1 200 OK\r\nX-Request-Count: lots\r\n\r\n{}")), None);
    }

    /// Every call: method, URL and body.
    pub type Calls = Arc<Mutex<Vec<(String, String, Option<Value>)>>>;

    /// Answers each call with the first canned answer whose "METHOD url" contains the key, cut to the
    /// URL's `fields=` as Bitbucket cuts it, so a field a read uses but doesn't ask for is missing here
    /// too; logs the calls.
    pub struct FakeHttp {
        pub answers: Vec<(&'static str, Value)>,
        pub calls: Calls,
    }

    impl Http for FakeHttp {
        fn call(&self, method: &str, url: &str, body: Option<&Value>) -> HostResult<Value> {
            self.calls.lock().push((method.into(), url.into(), body.cloned()));
            let key = format!("{method} {url}");
            let v = self.answers.iter().find(|(k, _)| key.contains(k)).map(|(_, v)| v.clone()).ok_or_else(|| format!("Bitbucket answered 404: no answer for {key}"))?;
            // A canned `{"error": "…"}` is a failed call.
            match v["error"].as_str() {
                Some(e) => Err(e.to_string()),
                None => Ok(match url.split(['?', '&']).find_map(|q| q.strip_prefix("fields=")) {
                    Some(f) => only(&v, &f.split(',').collect::<Vec<_>>()),
                    None => v,
                }),
            }
        }
    }

    /// The parts of `v` the dotted paths name (`values.user.uuid`); a list keeps the paths for each item.
    fn only(v: &Value, paths: &[&str]) -> Value {
        match v {
            Value::Array(items) => Value::Array(items.iter().map(|x| only(x, paths)).collect()),
            Value::Object(o) => {
                let mut out = serde_json::Map::new();
                for (k, x) in o {
                    let rest: Vec<&str> = paths.iter().filter_map(|p| if p == k { Some("") } else { p.strip_prefix(k.as_str())?.strip_prefix('.') }).collect();
                    if rest.contains(&"") {
                        out.insert(k.clone(), x.clone());
                    } else if !rest.is_empty() {
                        out.insert(k.clone(), only(x, &rest));
                    }
                }
                Value::Object(out)
            }
            _ => v.clone(),
        }
    }

    const ME: &str = "{me}";
    const REV: &str = "{rev}";

    fn pr_json() -> Value {
        json!({
            "id": 7, "title": "Add x", "state": "OPEN", "author": {"uuid": ME, "display_name": "Me"},
            "source": {"branch": {"name": "feat"}, "commit": {"hash": "abc123"}},
            "destination": {"branch": {"name": "main"}, "commit": {"hash": "base1"}},
            "reviewers": [{"uuid": REV, "display_name": "Rev"}, {"uuid": "{new}", "display_name": "New"}],
            "participants": [
                {"user": {"uuid": REV, "display_name": "Rev"}, "role": "REVIEWER", "approved": false, "state": "changes_requested", "participated_on": "2026-10-02T09:00:00Z"},
                {"user": {"uuid": "{ok}", "display_name": "Ok"}, "role": "PARTICIPANT", "approved": true, "state": "approved"},
                {"user": {"uuid": ME, "display_name": "Me"}, "role": "PARTICIPANT", "approved": false, "state": null}
            ]
        })
    }

    fn comments() -> Value {
        json!({"values": [
            {"id": 1, "user": {"uuid": REV, "display_name": "Rev"}, "content": {"raw": "Rename this"}, "inline": {"path": "a.rs", "to": 4}, "created_on": "t1"},
            {"id": 2, "parent": {"id": 1}, "user": {"uuid": ME}, "content": {"raw": "Done"}, "created_on": "t2"},
            {"id": 3, "user": {"uuid": REV, "display_name": "Rev"}, "content": {"raw": "Why?"}, "created_on": "t3"},
            {"id": 4, "parent": {"id": 3}, "user": {"uuid": ME}, "content": {"raw": "Because"}, "created_on": "t4"},
            {"id": 5, "parent": {"id": 4}, "user": {"uuid": REV}, "content": {"raw": "Not convinced"}, "created_on": "t5"},
            {"id": 6, "user": {"uuid": REV}, "content": {"raw": "Old"}, "created_on": "t6", "resolution": {"type": "resolved"}},
            {"id": 8, "user": {"uuid": REV}, "content": {"raw": "draft"}, "created_on": "t8", "pending": true}
        ], "next": "https://api/x/comments?page=2"})
    }

    fn host(calls: Calls) -> BitbucketHost {
        let http = FakeHttp {
            answers: vec![
                ("comments?page=2", json!({"values": [{"id": 9, "user": {"uuid": "{ok}"}, "content": {"raw": "Nice"}, "created_on": "t9"}]})),
                ("GET https://api/repositories/ws/repo/pullrequests/7/comments", comments()),
                ("GET https://api/repositories/ws/repo/pullrequests/7/tasks", json!({"values": [
                    {"id": 11, "state": "UNRESOLVED", "content": {"raw": "Add a test"}, "creator": {"uuid": REV}, "comment": {"id": 3}},
                    {"id": 12, "state": "RESOLVED", "content": {"raw": "Done one"}, "creator": {"uuid": REV}}
                ]})),
                ("GET https://api/repositories/ws/repo/commit/abc123/statuses", json!({"values": [
                    {"key": "build", "name": "Build", "state": "SUCCESSFUL"},
                    {"key": "test", "name": "Tests", "state": "FAILED", "url": "https://bitbucket.org/ws/repo/pipelines/results/41", "created_on": "2026-10-02T08:00:00.000000+00:00", "updated_on": "2026-10-02T08:10:00.000000+00:00"},
                    {"key": "e2e", "name": "", "state": "INPROGRESS", "created_on": "2026-10-02T08:20:00.000000+00:00"}
                ]})),
                ("GET https://api/repositories/ws/repo/pullrequests/7/diffstat", json!({"values": [
                    {"status": "modified", "new": {"path": "a.rs"}},
                    {"status": "merge conflict", "new": {"path": "b.rs"}, "old": {"path": "b.rs"}}
                ]})),
                ("GET https://api/repositories/ws/repo/pullrequests/7", pr_json()),
                ("PUT https://api/repositories/ws/repo/pullrequests/7", json!({})),
                ("POST https://api/repositories/ws/repo/pullrequests/7/", json!({})),
                ("GET https://api/repositories/ws/repo/pipelines/", json!({"values": [
                    {"uuid": "{p1}", "build_number": 41, "state": {"name": "IN_PROGRESS"}, "target": {"commit": {"hash": "abc123def"}, "ref_name": "feat"}},
                    {"uuid": "{p2}", "build_number": 42, "state": {"name": "PENDING"},
                     "target": {"type": "pipeline_pullrequest_target", "commit": {"hash": "merge9"}, "selector": {"pattern": "**"}, "pullrequest": {"id": 7}}},
                    {"uuid": "{p0}", "build_number": 40, "state": {"name": "COMPLETED"}, "target": {"commit": {"hash": "abc123def"}}}
                ]})),
                ("POST https://api/repositories/ws/repo/pipelines/", Value::Null),
                ("POST https://api/repositories/ws/repo/pullrequests", json!({"id": 31, "links": {"html": {"href": "https://bitbucket.org/ws/repo/pull-requests/31"}}})),
                ("GET https://api/repositories/ws/repo/commits/main", json!({"values": [{"hash": "m1"}, {"hash": "m2"}]})),
                ("GET https://api/repositories/ws/repo/commit/m1/statuses", json!({"values": [{"key": "test", "name": "Tests", "state": "FAILED", "url": "https://bitbucket.org/ws/repo/pipelines/results/38"}]})),
                ("GET https://api/repositories/ws/repo/commit/m2/statuses", json!({"values": [{"key": "build", "name": "Build", "state": "SUCCESSFUL"}]})),
            ],
            calls,
        };
        BitbucketHost::new(Box::new(http), "https://api/")
    }

    #[test]
    fn a_thread_the_board_s_own_account_answered_isn_t_waiting() {
        // The board posts as a bot account, not as the PR's author.
        let calls: Calls = Arc::new(Mutex::new(vec![]));
        let mut c = comments();
        c["values"] = json!([
            {"id": 1, "user": {"uuid": REV}, "content": {"raw": "Rename this"}, "created_on": "t1"},
            {"id": 2, "parent": {"id": 1}, "user": {"uuid": "{bot}"}, "content": {"raw": "Done"}, "created_on": "t2"},
            {"id": 3, "user": {"uuid": REV}, "content": {"raw": "Why?"}, "created_on": "t3"}
        ]);
        c["next"] = Value::Null;
        let http = FakeHttp {
            answers: vec![
                ("GET https://api/user", json!({"uuid": "{bot}", "account_id": "557058:bot"})),
                ("/pullrequests/7/tasks", json!({"error": "Bitbucket answered 403"})),
                ("GET https://api/repositories/ws/repo/pullrequests/7/comments", c),
                ("GET https://api/repositories/ws/repo/commit/abc123/statuses", json!({"values": []})),
                ("GET https://api/repositories/ws/repo/pullrequests/7", pr_json()),
            ],
            calls: calls.clone(),
        };
        let h = BitbucketHost::new(Box::new(http), "https://api/").with_account("bitbucket:test-bot");
        let r = h.read(&pr()).unwrap();
        assert_eq!(r.viewer, "{bot}");
        let open: Vec<&str> = r.threads.iter().filter(|t| t.waiting_on(r.us())).map(|t| t.id.as_str()).collect();
        assert_eq!(open, vec!["3"], "the bot had the last word on 1");
        assert_eq!(r.tasks_error.as_deref(), Some("Bitbucket answered 403"), "tasks that can't be read are unknown");
        assert!(r.to_value()["tasks_open"].is_null());
        h.read(&pr()).unwrap();
        assert_eq!(calls.lock().iter().filter(|(_, u, _)| u == "https://api/user").count(), 1, "who the account is is asked once");
    }

    #[test]
    fn stamps_are_one_list_call_per_repo_and_statuses_only_for_prs_waiting_on_checks() {
        let calls: Calls = Arc::new(Mutex::new(vec![]));
        let other = json!({"id": 8, "state": "OPEN", "updated_on": "2026-10-02T10:00:00Z", "source": {"commit": {"hash": "def456"}}});
        let http = FakeHttp {
            answers: vec![
                ("GET https://api/repositories/ws/repo/pullrequests?q=", json!({"values": [pr_json(), other]})),
                ("GET https://api/repositories/ws/lib/pullrequests?q=", json!({"values": [{"id": 3, "updated_on": "2026-10-01T10:00:00Z"}]})),
                ("GET https://api/repositories/ws/repo/commit/abc123/statuses", json!({"values": [{"key": "build", "state": "INPROGRESS"}]})),
            ],
            calls: calls.clone(),
        };
        let h = BitbucketHost::new(Box::new(http), "https://api/");
        let look = |repo: &str, num: i64, checks: bool| Look { pr: PrRef { repo: repo.into(), num, ..pr() }, checks };
        let looks = [look("ws/repo", 7, true), look("ws/lib", 3, false), look("ws/repo", 8, false), look("ws/repo", 9, false)];
        let stamps = h.stamps(&looks).unwrap();
        let urls: Vec<String> = calls.lock().iter().map(|(_, u, _)| u.clone()).collect();
        assert_eq!(urls.len(), 3, "a list call per repo and one PR's statuses: {urls:?}");
        assert!(urls[0].starts_with("https://api/repositories/ws/repo/pullrequests?q=id%3D7%20OR%20id%3D8%20OR%20id%3D9&state=OPEN&"), "{}", urls[0]);
        assert!(urls[0].contains("values.updated_on") && urls[0].contains("values.source.commit.hash"), "{}", urls[0]);
        assert!(urls[1].starts_with("https://api/repositories/ws/repo/commit/abc123/statuses?pagelen=100&fields="), "only #7 waits on checks: {}", urls[1]);
        assert!(urls[2].starts_with("https://api/repositories/ws/lib/pullrequests?q=id%3D3&"), "{}", urls[2]);
        assert!(stamps[0].as_deref().unwrap().contains("INPROGRESS"), "#7's stamp moves with its checks");
        assert!(stamps[1].is_some() && stamps[2].is_some());
        assert_ne!(stamps[0], stamps[2]);
        assert_eq!(stamps[3], None, "#9 wasn't in the list: read it in full");
    }

    fn pr() -> PrRef {
        PrRef { host: "bitbucket".into(), repo: "ws/repo".into(), num: 7, url: "https://bitbucket.org/ws/repo/pull-requests/7".into() }
    }

    #[test]
    fn reads_a_pr_with_its_threads_tasks_and_builds() {
        let calls = Arc::new(Mutex::new(vec![]));
        let r = host(calls.clone()).read(&pr()).unwrap();
        assert_eq!((r.state.as_str(), r.head.as_str(), r.branch.as_str(), r.base.as_str(), r.base_head.as_str()), ("OPEN", "abc123", "feat", "main", "base1"));
        assert_eq!(r.failed(), vec!["Tests".to_string()]);
        assert_eq!(r.checks[2].name, "e2e", "a status without a name goes by its key");
        let at: Vec<Option<&str>> = r.checks.iter().map(|c| c.at.as_deref()).collect();
        assert_eq!(at, vec![None, Some("2026-10-02T08:10:00.000000+00:00"), Some("2026-10-02T08:20:00.000000+00:00")], "updated, else created");
        assert_eq!(r.review_decision, "CHANGES_REQUESTED");
        assert_eq!(r.changes_at.as_deref(), Some("2026-10-02T09:00:00Z"));
        assert_eq!(r.approvals, 1);
        let rv: Vec<(&str, &str, bool)> = r.reviewers.iter().map(|x| (x.user.as_str(), x.state.as_str(), x.requested)).collect();
        assert_eq!(rv, vec![(REV, "changes", true), ("{new}", "pending", true), ("{ok}", "approved", false)]);
        assert_eq!(r.reviewers[0].changes_at.as_deref(), Some("2026-10-02T09:00:00Z"), "each reviewer's own request for changes");
        assert_eq!(r.reviewers[2].changes_at, None);
        let open: Vec<&str> = r.threads.iter().filter(|t| t.waiting_on(ME)).map(|t| t.id.as_str()).collect();
        assert_eq!(open, vec!["3", "9", "task-11"], "1 has my last word, 6 is resolved, 8 is a draft, a reply chain counts under its root");
        let t3 = r.threads.iter().find(|t| t.id == "3").unwrap();
        assert_eq!((t3.last_author.as_str(), t3.last_id.as_str()), (REV, "5"));
        let replies: Vec<(&str, &str)> = t3.replies.iter().map(|x| (x.author.as_str(), x.text.as_str())).collect();
        assert_eq!(replies, vec![(ME, "Because"), (REV, "Not convinced")], "the replies under it, oldest first");
        let t1 = r.threads.iter().find(|t| t.id == "1").unwrap();
        assert_eq!((t1.kind.as_str(), t1.path.as_deref(), t1.line), ("review", Some("a.rs"), Some(4)));
        assert_eq!(r.tasks_open, 1);
        assert_eq!(r.comments, 5, "every live comment from someone else, drafts aside");
        assert!(calls.lock().iter().any(|(_, u, _)| u.contains("comments?page=2")), "follows the next page");
        assert_eq!(r.mergeable, json!(false), "a file in merge conflict");
    }

    #[test]
    fn a_read_asks_only_for_the_fields_it_uses() {
        // #743: a full read is six calls; each answers only what the record is made from. A next page's
        // link is Bitbucket's own, and keeps the first page's fields.
        let calls = Arc::new(Mutex::new(vec![]));
        host(calls.clone()).read(&pr()).unwrap();
        let urls: Vec<String> = calls.lock().iter().map(|(_, u, _)| u.clone()).filter(|u| !u.ends_with("/user") && !u.contains("page=2")).collect();
        assert!(urls.len() >= 5, "{urls:?}");
        for u in &urls {
            assert!(u.contains("fields="), "{u}");
        }
        let pr_get = urls.iter().find(|u| u.contains("/pullrequests/7?")).unwrap();
        assert!(pr_get.contains("participants.approved") && !pr_get.contains("summary"), "{pr_get}");
        // The fake answers only the fields asked for, so the other read tests catch a list that drops one.
        assert_eq!(host(calls).files(&pr()).unwrap(), vec!["a.rs", "b.rs"]);
    }

    #[test]
    fn reads_whether_a_pr_conflicts_from_its_diffstat() {
        let mergeable = |diffstat: Vec<(&'static str, Value)>| {
            let mut answers = diffstat;
            answers.extend([
                ("GET https://api/repositories/ws/repo/pullrequests/7/comments", json!({"values": []})),
                ("GET https://api/repositories/ws/repo/pullrequests/7/tasks", json!({"values": []})),
                ("GET https://api/repositories/ws/repo/commit/abc123/statuses", json!({"values": []})),
                ("GET https://api/repositories/ws/repo/pullrequests/7", pr_json()),
            ]);
            let h = BitbucketHost::new(Box::new(FakeHttp { answers, calls: Arc::new(Mutex::new(vec![])) }), "https://api/");
            h.read(&pr()).unwrap().mergeable
        };
        let page = |status: &str, next: Option<&str>| json!({"values": [{"status": status, "new": {"path": "a.rs"}}], "next": next});
        assert_eq!(mergeable(vec![("/diffstat", page("modified", None))]), json!(true), "every file merges");
        for status in ["local deleted", "remote deleted"] {
            assert_eq!(mergeable(vec![("/diffstat", page(status, None))]), json!(false), "{status}");
        }
        assert_eq!(
            mergeable(vec![("diffstat?page=2", page("merge conflict", None)), ("/diffstat", page("added", Some("https://api/repositories/ws/repo/pullrequests/7/diffstat?page=2")))]),
            json!(false),
            "a conflict on a later page"
        );
        assert_eq!(mergeable(vec![("/diffstat", page("modified", Some("https://api/repositories/ws/repo/pullrequests/7/diffstat")))]), Value::Null, "too many pages to read whole");
        assert_eq!(mergeable(vec![("/diffstat", json!({"error": "Bitbucket answered 500"}))]), Value::Null, "an unreadable diffstat");
    }

    /// A FakeHttp on its own account, which a diffstat call rate-limits.
    struct LimitedOnDiffstat(FakeHttp);

    impl Http for LimitedOnDiffstat {
        fn call(&self, method: &str, url: &str, body: Option<&Value>) -> HostResult<Value> {
            if url.contains("/diffstat") {
                let hit = crate::prhost::limit::Hit { retry_after: Some(600.0), why: "Bitbucket answered 429".into(), ..Default::default() };
                crate::prhost::limit::limits().hit("bb:diffstat-limited", "bitbucket", &hit, crate::util::now_ts());
                return Err("Bitbucket answered 429".into());
            }
            self.0.call(method, url, body)
        }
        fn limit_key(&self) -> Option<String> {
            Some("bb:diffstat-limited".into())
        }
    }

    #[test]
    fn a_rate_limited_diffstat_fails_the_read_not_the_conflict_check() {
        // Read as unknown, the PR's conflicts held nothing, and its merge wake said nothing held it.
        let answers = vec![
            ("GET https://api/repositories/ws/repo/pullrequests/7/comments", json!({"values": []})),
            ("GET https://api/repositories/ws/repo/pullrequests/7/tasks", json!({"values": []})),
            ("GET https://api/repositories/ws/repo/commit/abc123/statuses", json!({"values": []})),
            ("GET https://api/repositories/ws/repo/pullrequests/7", pr_json()),
        ];
        let h = BitbucketHost::new(Box::new(LimitedOnDiffstat(FakeHttp { answers, calls: Arc::new(Mutex::new(vec![])) })), "https://api/");
        assert_eq!(h.read(&pr()).err().as_deref(), Some("Bitbucket answered 429"));
    }

    /// Each call, as "METHOD url", with the last-wins key it was marked with (`limit::last_wins`).
    struct Marked(FakeHttp, Arc<Mutex<Vec<(String, Option<String>)>>>);

    impl Http for Marked {
        fn call(&self, method: &str, url: &str, body: Option<&Value>) -> HostResult<Value> {
            self.1.lock().push((format!("{method} {url}"), crate::prhost::limit::last_wins_key()));
            self.0.call(method, url, body)
        }
    }

    #[test]
    fn writes_that_only_set_a_pr_s_state_are_last_wins_by_field() {
        let marks = Arc::new(Mutex::new(vec![]));
        let http = FakeHttp { answers: vec![("GET", pr_json()), ("PUT", json!({})), ("POST", json!({}))], calls: Arc::new(Mutex::new(vec![])) };
        let h = BitbucketHost::new(Box::new(Marked(http, marks.clone())), "https://api/");
        h.request_reviews(&pr(), &["557058:abc".to_string()]).unwrap();
        h.retarget(&pr(), "develop").unwrap();
        h.set_description(&pr(), "## Summary").unwrap();
        h.merge(&pr(), &MergeOpts { strategy: Some("squash".into()), close_source_branch: true }).unwrap();
        h.re_request_reviews(&pr(), &[REV.to_string()]).unwrap();
        let pr7 = "https://api/repositories/ws/repo/pullrequests/7";
        let marks = marks.lock().clone();
        let puts: Vec<_> = marks.iter().filter(|(c, _)| c.starts_with("PUT")).map(|(_, k)| k.clone()).collect();
        let reviewers = Some(format!("{pr7} reviewers"));
        assert_eq!(
            puts,
            vec![reviewers.clone(), Some(format!("{pr7} base")), Some(format!("{pr7} description")), None, reviewers],
            "asking again takes them off with a write that always goes out, then puts them back last-wins"
        );
        assert!(marks.iter().any(|(c, _)| c == &format!("POST {pr7}/merge")));
        assert!(marks.iter().filter(|(c, _)| !c.starts_with("PUT")).all(|(_, k)| k.is_none()), "reads and merges aren't: {marks:?}");
    }

    #[test]
    fn writes_go_to_the_documented_endpoints() {
        let calls = Arc::new(Mutex::new(vec![]));
        let h = host(calls.clone());
        let r = h.read(&pr()).unwrap();
        let t3 = r.threads.iter().find(|t| t.id == "3").unwrap().clone();
        let task = r.threads.iter().find(|t| t.id == "task-11").unwrap().clone();
        h.reply(&pr(), &t3, "Fixed").unwrap();
        h.resolve(&pr(), &t3).unwrap();
        h.reply(&pr(), &task, "Added").unwrap();
        h.resolve(&pr(), &task).unwrap();
        h.re_request_reviews(&pr(), &[REV.to_string()]).unwrap();
        h.request_reviews(&pr(), &["557058:abc".to_string()]).unwrap();
        h.merge(&pr(), &MergeOpts { strategy: Some("squash".into()), close_source_branch: true }).unwrap();
        h.retarget(&pr(), "develop").unwrap();
        assert_eq!(h.cancel_builds(&pr(), "abc123").unwrap(), Cancelled::Stopped(vec!["feat #41".into(), "** #42".into()]), "the head's builds and the PR's own");
        assert_eq!(h.base_failures(&pr(), "main", 5).unwrap(), vec!["Tests".to_string()]);
        assert_eq!(
            h.base_failed_checks(&pr(), "main", 5).unwrap(),
            vec![Check { name: "Tests".into(), state: "failed".into(), url: Some("https://bitbucket.org/ws/repo/pipelines/results/38".into()), at: None }]
        );
        let opened = h.open("ws/repo", "main", "feat", "Add x", "## Summary").unwrap();
        assert_eq!((opened.num, opened.url.as_str()), (31, "https://bitbucket.org/ws/repo/pull-requests/31"));
        let calls = calls.lock().clone();
        let find = |m: &str, u: &str| calls.iter().filter(|(cm, cu, _)| cm == m && cu.ends_with(u)).map(|(_, _, b)| b.clone().unwrap_or(Value::Null)).collect::<Vec<_>>();
        let replies = find("POST", "/pullrequests/7/comments");
        assert_eq!(replies[0], json!({"content": {"raw": "Fixed"}, "parent": {"id": 3}}));
        assert_eq!(replies[1], json!({"content": {"raw": "Added"}, "parent": {"id": 3}}), "a task on a comment is answered there");
        assert_eq!(find("POST", "/comments/3/resolve").len(), 1);
        assert_eq!(find("PUT", "/tasks/11"), vec![json!({"state": "RESOLVED"})]);
        let puts = find("PUT", "/pullrequests/7");
        assert_eq!(puts[0]["reviewers"], json!([{"uuid": "{new}"}]), "off the list first…");
        assert_eq!(puts[1]["reviewers"], json!([{"uuid": "{new}"}, {"uuid": REV}]), "…then back on");
        assert_eq!(puts[2]["reviewers"], json!([{"uuid": REV}, {"uuid": "{new}"}, {"account_id": "557058:abc"}]));
        assert_eq!(puts[3]["destination"], json!({"branch": {"name": "develop"}}));
        assert!(puts.iter().all(|p| p["title"] == "Add x"), "Bitbucket wants the title with every update");
        assert_eq!(find("POST", "/pullrequests/7/merge"), vec![json!({"type": "pullrequest", "close_source_branch": true, "merge_strategy": "squash"})]);
        assert_eq!(find("POST", "/pipelines/%7Bp1%7D/stopPipeline").len(), 1);
        assert_eq!(
            find("POST", "/repo/pullrequests"),
            vec![json!({"title": "Add x", "description": "## Summary", "source": {"branch": {"name": "feat"}}, "destination": {"branch": {"name": "main"}}, "close_source_branch": true})]
        );
        assert!(find("POST", "/pipelines/%7Bp0%7D/stopPipeline").is_empty(), "a finished run isn't stopped");
    }

    #[test]
    fn lists_the_account_s_open_prs_whoever_opened_them() {
        let calls: Calls = Arc::new(Mutex::new(vec![]));
        let http = FakeHttp {
            answers: vec![
                ("GET https://api/user", json!({"uuid": ME})),
                ("GET https://api/repositories/ws/repo/pullrequests?state=OPEN", json!({"values": [
                    {"id": 7, "author": {"uuid": ME}, "source": {"branch": {"name": "by-hand"}}, "links": {"html": {"href": "https://bitbucket.org/ws/repo/pull-requests/7"}}},
                    {"id": 8, "author": {"uuid": REV}, "source": {"branch": {"name": "theirs"}}}
                ]})),
            ],
            calls,
        };
        let got = BitbucketHost::new(Box::new(http), "https://api/").my_open_prs("ws/repo").unwrap();
        assert_eq!(got, vec![OpenPr { num: 7, branch: "by-hand".into(), url: "https://bitbucket.org/ws/repo/pull-requests/7".into() }]);
    }

    #[test]
    fn finds_a_pipelines_build_number() {
        assert_eq!(pipeline_build("https://bitbucket.org/ws/repo/pipelines/results/41"), Some(41));
        assert_eq!(pipeline_build("https://ci.example.com/41"), None);
    }

    #[test]
    fn lists_a_repo_s_recent_comments_whole() {
        let calls: Calls = Arc::new(Mutex::new(vec![]));
        let http = FakeHttp {
            answers: vec![
                ("GET https://api/repositories/ws/repo/pullrequests?", json!({"values": [{"id": 30}, {"id": 28}]})),
                ("GET https://api/repositories/ws/repo/pullrequests/30/comments", json!({"values": [
                    {"id": 1, "user": {"uuid": REV, "display_name": "Rev"}, "content": {"raw": "Two findings\n\n_AI review_"}, "created_on": "2026-10-08T09:00:00Z"},
                    {"id": 2, "user": {"uuid": REV}, "content": {"raw": "gone"}, "created_on": "t", "deleted": true}
                ]})),
                ("GET https://api/repositories/ws/repo/pullrequests/28/comments", json!({"values": [
                    {"id": 7, "user": {"uuid": ME}, "content": {"raw": "Thanks"}, "created_on": "2026-10-08T08:00:00Z"}
                ]})),
            ],
            calls: calls.clone(),
        };
        let h = BitbucketHost::new(Box::new(http), "https://api/");
        let got = h.recent_comments("ws/repo", 20).unwrap();
        assert_eq!(got.iter().map(|c| (c.pr, c.id.as_str(), c.author.as_str())).collect::<Vec<_>>(), vec![(30, "1", REV), (28, "7", ME)]);
        assert_eq!((got[0].text.as_str(), got[0].at.as_str()), ("Two findings\n\n_AI review_", "2026-10-08T09:00:00Z"));
        assert!(calls.lock()[0].1.contains("sort=-updated_on&pagelen=20"), "{:?}", calls.lock()[0]);
    }
}
