//! Bitbucket Cloud through its REST API (2.0), with Taskboard's Bitbucket account (email + API token,
//! basic auth; see `accounts.rs`).
//!
//! A PR's threads are its top-level comments with their replies (resolved when the top comment has a
//! `resolution`), and its PR tasks. Reviewers are the PR's `reviewers` plus every participant who
//! approved or asked for changes.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value};

use super::{gist, Cancelled, Check, HostResult, MergeOpts, PrHost, PrRef, Record, Reply, Reviewer, Thread, REPLY_MAX};

/// One HTTP call: method, full URL and JSON body; answers the JSON reply (Null when it's empty). An
/// error starts with "Bitbucket answered <code>" when Bitbucket answered.
pub trait Http: Send + Sync {
    fn call(&self, method: &str, url: &str, body: Option<&Value>) -> HostResult<Value>;
}

/// JSON over HTTPS with basic auth (Bitbucket, and Azure DevOps in `ci.rs`).
pub struct BasicHttp {
    auth: String,
    agent: ureq::Agent,
    service: &'static str,
}

impl BasicHttp {
    pub fn new(email: &str, token: &str) -> BasicHttp {
        BasicHttp::for_service("Bitbucket", email, token)
    }

    pub fn for_service(service: &'static str, user: &str, token: &str) -> BasicHttp {
        let auth = format!("Basic {}", crate::jira::base64_lite::encode(format!("{user}:{token}").as_bytes()));
        BasicHttp { auth, agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(20)).build(), service }
    }
}

impl Http for BasicHttp {
    fn call(&self, method: &str, url: &str, body: Option<&Value>) -> HostResult<Value> {
        let req = self.agent.request(method, url).set("Authorization", &self.auth).set("Accept", "application/json");
        let res = match body {
            Some(b) => req.send_json(b.clone()),
            None => req.call(),
        };
        match res {
            Ok(r) => {
                let text = r.into_string().unwrap_or_default();
                if text.trim().is_empty() {
                    return Ok(Value::Null);
                }
                Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
            }
            Err(ureq::Error::Status(code, r)) => {
                let v: Value = r.into_json().unwrap_or(Value::Null);
                let why = v["error"]["message"].as_str().unwrap_or("").to_string();
                let svc = self.service;
                Err(match code {
                    401 if svc == "Bitbucket" => "Bitbucket answered 401: it didn't accept Taskboard's Bitbucket token; sign in again in Settings ▸ Accounts".to_string(),
                    _ if why.is_empty() => format!("{svc} answered {code}"),
                    _ => format!("{svc} answered {code}: {why}"),
                })
            }
            Err(_) => Err(format!("Can't reach {}", self.service)),
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

    /// Sets the PR's reviewers to exactly these.
    fn put_reviewers(&self, pr: &PrRef, p: &Value, reviewers: Vec<Value>) -> HostResult<()> {
        self.http.call("PUT", &self.pr_url(pr), Some(&json!({"title": p["title"], "reviewers": reviewers}))).map(|_| ())
    }

    fn current_reviewers(p: &Value) -> Vec<Value> {
        p["reviewers"].as_array().cloned().unwrap_or_default().iter().map(|r| user_ref(&uid(r))).filter(|r| r != &user_ref("")).collect()
    }

    fn statuses(&self, repo: &str, commit: &str) -> HostResult<Vec<Value>> {
        self.get_all(&format!("{}/commit/{commit}/statuses?pagelen=100", self.repo_url(repo)), 3)
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

impl PrHost for BitbucketHost {
    fn id(&self) -> &'static str {
        "bitbucket"
    }

    fn read(&self, pr: &PrRef) -> HostResult<Record> {
        let p = self.get(&self.pr_url(pr))?;
        let comments = self.get_all(&format!("{}/comments?pagelen=100", self.pr_url(pr)), 5)?;
        // PR tasks need a newer token scope on some workspaces. Tasks that can't be read are unknown, not
        // none: the merge waits on them (`prcmds::merge_blockers`).
        let tasks = self.get_all(&format!("{}/tasks?pagelen=100", self.pr_url(pr)), 3);
        let head = p["source"]["commit"]["hash"].as_str().unwrap_or("");
        let statuses = if head.is_empty() { vec![] } else { self.statuses(&pr.repo, head)? };
        let mut r = summarize(&p, &comments, tasks.as_deref().unwrap_or(&[]), &statuses);
        r.tasks_error = tasks.err();
        r.viewer = self.viewer();
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
        // their old approval or request for changes, and notifies them.
        let p = self.get(&self.pr_url(pr))?;
        let list = Self::current_reviewers(&p);
        let without: Vec<Value> = list.iter().filter(|r| !users.iter().any(|u| same_user(r, u))).cloned().collect();
        self.put_reviewers(pr, &p, without.clone())?;
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
        let p = self.get(&self.pr_url(pr))?;
        self.http.call("PUT", &self.pr_url(pr), Some(&json!({"title": p["title"], "destination": {"branch": {"name": base}}}))).map(|_| ())
    }

    fn description(&self, pr: &PrRef) -> HostResult<String> {
        Ok(self.get(&self.pr_url(pr))?["description"].as_str().unwrap_or("").to_string())
    }

    fn set_description(&self, pr: &PrRef, body: &str) -> HostResult<()> {
        let p = self.get(&self.pr_url(pr))?;
        self.http.call("PUT", &self.pr_url(pr), Some(&json!({"title": p["title"], "description": body}))).map(|_| ())
    }

    fn cancel_builds(&self, pr: &PrRef, head: &str) -> HostResult<Cancelled> {
        let runs = match self.get_all(&format!("{}/pipelines/?sort=-created_on&pagelen=50", self.repo_url(&pr.repo)), 1) {
            Ok(r) => r,
            Err(e) if e.contains("answered 404") || e.contains("answered 403") => {
                return Ok(Cancelled::Unsupported("this repository's builds don't run on Bitbucket Pipelines; stop them in its CI".into()))
            }
            Err(e) => return Err(e),
        };
        let mut n = 0;
        for r in runs {
            let hash = r["target"]["commit"]["hash"].as_str().unwrap_or("");
            let same = !hash.is_empty() && !head.is_empty() && (hash.starts_with(head) || head.starts_with(hash));
            if !same || !matches!(r["state"]["name"].as_str(), Some("PENDING" | "IN_PROGRESS")) {
                continue;
            }
            let uuid = r["uuid"].as_str().unwrap_or("");
            self.http.call("POST", &format!("{}/pipelines/{}/stopPipeline", self.repo_url(&pr.repo), seg(uuid)), None)?;
            n += 1;
        }
        Ok(Cancelled::Stopped(n))
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
                    out.push(Check { name, state: "failed".into(), url });
                }
            }
        }
        Ok(out)
    }
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
pub fn summarize(p: &Value, comments: &[Value], tasks: &[Value], statuses: &[Value]) -> Record {
    let author = uid(&p["author"]);
    let checks: Vec<Check> = statuses
        .iter()
        .map(|s| Check {
            name: s["name"].as_str().filter(|n| !n.is_empty()).or(s["key"].as_str()).unwrap_or("build").to_string(),
            state: check_state(s["state"].as_str().unwrap_or("")).to_string(),
            url: s["url"].as_str().filter(|u| !u.is_empty()).map(|u| u.to_string()),
        })
        .collect();
    let mut reviewers: Vec<Reviewer> = vec![];
    for r in p["reviewers"].as_array().cloned().unwrap_or_default() {
        let user = uid(&r);
        if user.is_empty() || user == author {
            continue;
        }
        reviewers.push(Reviewer { user, name: r["display_name"].as_str().unwrap_or("").to_string(), state: "pending".into(), requested: true });
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
        if state == "changes" {
            let at = part["participated_on"].as_str().unwrap_or("").to_string();
            if changes_at.as_deref().is_none_or(|c| at.as_str() > c) {
                changes_at = Some(at);
            }
        }
        match reviewers.iter_mut().find(|x| x.user == user) {
            Some(x) => {
                if state != "commented" {
                    x.state = state.into();
                }
            }
            None if state != "commented" => reviewers.push(Reviewer {
                user,
                name: part["user"]["display_name"].as_str().unwrap_or("").to_string(),
                state: state.into(),
                requested: false,
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

    /// Every call: method, URL and body.
    pub type Calls = Arc<Mutex<Vec<(String, String, Option<Value>)>>>;

    /// Answers each call with the first canned answer whose "METHOD url" contains the key; logs the calls.
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
                None => Ok(v),
            }
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
                    {"key": "test", "name": "Tests", "state": "FAILED", "url": "https://bitbucket.org/ws/repo/pipelines/results/41"},
                    {"key": "e2e", "name": "", "state": "INPROGRESS"}
                ]})),
                ("GET https://api/repositories/ws/repo/pullrequests/7", pr_json()),
                ("PUT https://api/repositories/ws/repo/pullrequests/7", json!({})),
                ("POST https://api/repositories/ws/repo/pullrequests/7/", json!({})),
                ("GET https://api/repositories/ws/repo/pipelines/", json!({"values": [
                    {"uuid": "{p1}", "build_number": 41, "state": {"name": "IN_PROGRESS"}, "target": {"commit": {"hash": "abc123def"}}},
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
        assert_eq!(r.review_decision, "CHANGES_REQUESTED");
        assert_eq!(r.changes_at.as_deref(), Some("2026-10-02T09:00:00Z"));
        assert_eq!(r.approvals, 1);
        let rv: Vec<(&str, &str, bool)> = r.reviewers.iter().map(|x| (x.user.as_str(), x.state.as_str(), x.requested)).collect();
        assert_eq!(rv, vec![(REV, "changes", true), ("{new}", "pending", true), ("{ok}", "approved", false)]);
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
        assert_eq!(h.cancel_builds(&pr(), "abc123").unwrap(), Cancelled::Stopped(1));
        assert_eq!(h.base_failures(&pr(), "main", 5).unwrap(), vec!["Tests".to_string()]);
        assert_eq!(
            h.base_failed_checks(&pr(), "main", 5).unwrap(),
            vec![Check { name: "Tests".into(), state: "failed".into(), url: Some("https://bitbucket.org/ws/repo/pipelines/results/38".into()) }]
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
    fn finds_a_pipelines_build_number() {
        assert_eq!(pipeline_build("https://bitbucket.org/ws/repo/pipelines/results/41"), Some(41));
        assert_eq!(pipeline_build("https://ci.example.com/41"), None);
    }
}
