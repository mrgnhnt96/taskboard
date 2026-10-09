//! GitHub through the `gh` CLI (signed in with Taskboard's GitHub account when there is one, see
//! `accounts::gh_env`). Review threads come from GraphQL, everything else from `gh pr view` and the
//! REST API through `gh api`.

use std::path::PathBuf;

use serde_json::Value;

use super::{gist, Cancelled, Check, Comment, HostResult, MergeOpts, OpenPr, PrHost, PrRef, Record, Reply, Reviewer, Thread, REPLY_MAX};

/// Runs `gh` with these arguments and answers its stdout.
pub trait Gh: Send + Sync {
    fn run(&self, args: &[String]) -> HostResult<String>;
}

pub struct GhCli {
    pub bin: PathBuf,
    pub env: Vec<(String, String)>,
}

impl Gh for GhCli {
    fn run(&self, args: &[String]) -> HostResult<String> {
        let o = crate::proc::run_with(&self.bin, args, None, 30.0, &self.env, None).map_err(|_| "gh didn't answer in 30 seconds".to_string())?;
        if o.code != Some(0) {
            return Err(o.stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("gh failed").to_string());
        }
        Ok(o.stdout)
    }
}

pub struct GithubHost {
    gh: Box<dyn Gh>,
    /// Which account `gh` signs in as, for caching who it is (`prhost::account_key`); empty: ask each read.
    account: String,
}

const VIEW_FIELDS: &str = "number,state,title,author,headRefOid,headRefName,baseRefName,baseRefOid,reviewDecision,statusCheckRollup,comments,reviews,latestReviews,reviewRequests,mergeable";

const THREADS_QUERY: &str = "query($owner:String!,$name:String!,$num:Int!){repository(owner:$owner,name:$name){pullRequest(number:$num){reviewThreads(first:100){nodes{id isResolved isOutdated path line first:comments(first:1){nodes{id author{login} body createdAt url}} last:comments(last:1){nodes{id author{login} body createdAt url}} replies:comments(first:30){nodes{id author{login} body createdAt}}}}}}}";
const RECENT_COMMENTS_QUERY: &str = "query($owner:String!,$name:String!,$n:Int!){repository(owner:$owner,name:$name){pullRequests(first:$n,orderBy:{field:UPDATED_AT,direction:DESC}){nodes{number comments(last:50){nodes{id author{login} body createdAt}} reviews(last:50){nodes{id author{login} body submittedAt comments(first:30){nodes{id author{login} body createdAt}}}}}}}}";
const REPLY_MUTATION: &str = "mutation($id:ID!,$body:String!){addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$id,body:$body}){comment{id}}}";
const RESOLVE_MUTATION: &str = "mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{isResolved}}}";

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn parse(text: &str) -> HostResult<Value> {
    serde_json::from_str(text).map_err(|e| format!("gh's answer doesn't parse: {e}"))
}

impl GithubHost {
    pub fn new(gh: Box<dyn Gh>) -> GithubHost {
        GithubHost { gh, account: String::new() }
    }

    /// Names the account `gh` signs in as, so who it is is asked once.
    pub fn with_account(mut self, key: &str) -> GithubHost {
        self.account = key.to_string();
        self
    }

    /// The board's own GitHub login (`gh api user`).
    fn viewer(&self) -> String {
        super::viewer_cached(&self.account, || Ok(self.api("GET", "user", &[])?["login"].as_str().unwrap_or("").to_string()))
    }

    fn gh(&self, args: Vec<String>) -> HostResult<String> {
        self.gh.run(&args)
    }

    fn api(&self, method: &str, path: &str, fields: &[(&str, &str)]) -> HostResult<Value> {
        let mut args = s(&["api", "-X", method, path]);
        for (k, v) in fields {
            args.push("-f".into());
            args.push(format!("{k}={v}"));
        }
        let out = self.gh(args)?;
        if out.trim().is_empty() {
            return Ok(Value::Null);
        }
        parse(&out)
    }

    fn graphql(&self, query: &str, fields: &[(&str, String)], typed: &[(&str, String)]) -> HostResult<Value> {
        let mut args = s(&["api", "graphql", "-f"]);
        args.push(format!("query={query}"));
        for (k, v) in fields {
            args.push("-f".into());
            args.push(format!("{k}={v}"));
        }
        for (k, v) in typed {
            args.push("-F".into());
            args.push(format!("{k}={v}"));
        }
        parse(&self.gh(args)?)
    }

    fn owner_name(pr: &PrRef) -> (String, String) {
        let (o, n) = pr.repo.split_once('/').unwrap_or((&pr.repo, ""));
        (o.to_string(), n.to_string())
    }

    fn default_strategy(&self, pr: &PrRef) -> String {
        self.gh(s(&["repo", "view", &pr.repo, "--json", "viewerDefaultMergeMethod"]))
            .ok()
            .and_then(|o| parse(&o).ok())
            .and_then(|v| v["viewerDefaultMergeMethod"].as_str().map(|m| m.to_lowercase()))
            .filter(|m| matches!(m.as_str(), "merge" | "squash" | "rebase"))
            .unwrap_or_else(|| "merge".into())
    }
}

impl PrHost for GithubHost {
    fn id(&self) -> &'static str {
        "github"
    }

    fn read(&self, pr: &PrRef) -> HostResult<Record> {
        let view = parse(&self.gh(s(&["pr", "view", &pr.num.to_string(), "-R", &pr.repo, "--json", VIEW_FIELDS]))?)?;
        let (owner, name) = Self::owner_name(pr);
        let threads = self.graphql(THREADS_QUERY, &[("owner", owner), ("name", name)], &[("num", pr.num.to_string())])?;
        let mut r = summarize(&view, &threads);
        r.viewer = self.viewer();
        r.leave_out_viewer();
        Ok(r)
    }

    fn reply(&self, pr: &PrRef, thread: &Thread, body: &str) -> HostResult<()> {
        if thread.kind == "review" {
            self.graphql(REPLY_MUTATION, &[("id", thread.id.clone()), ("body", body.to_string())], &[])?;
            return Ok(());
        }
        // A plain comment or a review's summary has no thread on GitHub: answer with a comment that quotes it.
        let quote = format!("> @{}: {}\n\n{body}", thread.author, thread.text);
        self.gh(vec!["pr".into(), "comment".into(), pr.num.to_string(), "-R".into(), pr.repo.clone(), "--body".into(), quote])?;
        Ok(())
    }

    fn resolve(&self, _pr: &PrRef, thread: &Thread) -> HostResult<bool> {
        if thread.kind != "review" {
            return Ok(false);
        }
        self.graphql(RESOLVE_MUTATION, &[("id", thread.id.clone())], &[])?;
        Ok(true)
    }

    fn request_reviews(&self, pr: &PrRef, users: &[String]) -> HostResult<()> {
        if users.is_empty() {
            return Ok(());
        }
        let fields: Vec<(&str, &str)> = users.iter().map(|u| ("reviewers[]", u.as_str())).collect();
        self.api("POST", &format!("repos/{}/pulls/{}/requested_reviewers", pr.repo, pr.num), &fields).map(|_| ())
    }

    fn re_request_reviews(&self, pr: &PrRef, users: &[String]) -> HostResult<()> {
        // Requesting someone who already reviewed asks them again on GitHub.
        self.request_reviews(pr, users)
    }

    fn remove_reviewers(&self, pr: &PrRef, users: &[String]) -> HostResult<()> {
        if users.is_empty() {
            return Ok(());
        }
        let fields: Vec<(&str, &str)> = users.iter().map(|u| ("reviewers[]", u.as_str())).collect();
        self.api("DELETE", &format!("repos/{}/pulls/{}/requested_reviewers", pr.repo, pr.num), &fields).map(|_| ())
    }

    fn merge(&self, pr: &PrRef, opts: &MergeOpts) -> HostResult<()> {
        let strategy = opts.strategy.clone().unwrap_or_else(|| self.default_strategy(pr));
        let flag = match strategy.as_str() {
            "squash" => "--squash",
            "rebase" => "--rebase",
            "merge" | "merge_commit" => "--merge",
            other => return Err(format!("GitHub can't merge with “{other}”: use merge, squash or rebase")),
        };
        let mut args = s(&["pr", "merge", &pr.num.to_string(), "-R", &pr.repo, flag]);
        if opts.close_source_branch {
            args.push("--delete-branch".into());
        }
        self.gh(args).map(|_| ())
    }

    fn open(&self, repo: &str, base: &str, branch: &str, title: &str, body: &str) -> HostResult<PrRef> {
        let v = self.api("POST", &format!("repos/{repo}/pulls"), &[("title", title), ("head", branch), ("base", base), ("body", body)])?;
        let num = v["number"].as_i64().ok_or_else(|| "GitHub didn't say which PR it opened".to_string())?;
        let url = v["html_url"].as_str().map(|s| s.to_string()).unwrap_or_else(|| format!("https://github.com/{repo}/pull/{num}"));
        Ok(PrRef { host: "github".into(), repo: repo.to_string(), num, url })
    }

    fn retarget(&self, pr: &PrRef, base: &str) -> HostResult<()> {
        self.gh(s(&["pr", "edit", &pr.num.to_string(), "-R", &pr.repo, "--base", base])).map(|_| ())
    }

    fn description(&self, pr: &PrRef) -> HostResult<String> {
        let v = self.api("GET", &format!("repos/{}/pulls/{}", pr.repo, pr.num), &[])?;
        Ok(v["body"].as_str().unwrap_or("").to_string())
    }

    fn set_description(&self, pr: &PrRef, body: &str) -> HostResult<()> {
        self.api("PATCH", &format!("repos/{}/pulls/{}", pr.repo, pr.num), &[("body", body)]).map(|_| ())
    }

    fn cancel_builds(&self, pr: &PrRef, head: &str) -> HostResult<Cancelled> {
        if head.is_empty() {
            return Ok(Cancelled::Stopped(vec![]));
        }
        let runs = parse(&self.gh(s(&["run", "list", "-R", &pr.repo, "--commit", head, "--json", "databaseId,status,workflowName,number", "--limit", "50"]))?)?;
        let mut stopped = vec![];
        for r in runs.as_array().cloned().unwrap_or_default() {
            if !matches!(r["status"].as_str(), Some("queued" | "in_progress" | "waiting" | "requested" | "pending")) {
                continue;
            }
            let Some(id) = r["databaseId"].as_i64() else { continue };
            self.gh(s(&["run", "cancel", &id.to_string(), "-R", &pr.repo]))?;
            let name = r["workflowName"].as_str().filter(|n| !n.is_empty()).unwrap_or("run");
            stopped.push(format!("{name} #{}", r["number"].as_i64().unwrap_or(id)));
        }
        Ok(Cancelled::Stopped(stopped))
    }

    fn my_open_prs(&self, repo: &str) -> HostResult<Vec<OpenPr>> {
        let list = parse(&self.gh(s(&["pr", "list", "-R", repo, "--author", "@me", "--state", "open", "--json", "number,headRefName,url", "--limit", "100"]))?)?;
        Ok(list
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|p| {
                Some(OpenPr { num: p["number"].as_i64()?, branch: p["headRefName"].as_str().unwrap_or("").to_string(), url: p["url"].as_str().unwrap_or("").to_string() })
            })
            .collect())
    }

    fn members(&self, repo: &str) -> HostResult<Vec<Reviewer>> {
        let v = self.api("GET", &format!("repos/{repo}/collaborators?per_page=100"), &[])?;
        Ok(v.as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|u| u["login"].as_str().map(|l| Reviewer { user: l.to_string(), name: l.to_string(), ..Default::default() }))
            .collect())
    }

    fn recent_comments(&self, repo: &str, prs: usize) -> HostResult<Vec<Comment>> {
        let (owner, name) = repo.split_once('/').unwrap_or((repo, ""));
        let v = self.graphql(RECENT_COMMENTS_QUERY, &[("owner", owner.to_string()), ("name", name.to_string())], &[("n", prs.min(100).to_string())])?;
        Ok(recent_comments(&v))
    }

    fn base_failures(&self, pr: &PrRef, base: &str, commits: usize) -> HostResult<Vec<String>> {
        let list = self.api("GET", &format!("repos/{}/commits?sha={base}&per_page={commits}", pr.repo), &[])?;
        let mut out: Vec<String> = vec![];
        for c in list.as_array().cloned().unwrap_or_default().into_iter().take(commits) {
            let Some(sha) = c["sha"].as_str() else { continue };
            let runs = self.api("GET", &format!("repos/{}/commits/{sha}/check-runs?per_page=100", pr.repo), &[])?;
            for r in runs["check_runs"].as_array().cloned().unwrap_or_default() {
                if matches!(r["conclusion"].as_str(), Some("failure" | "timed_out")) {
                    out.push(r["name"].as_str().unwrap_or("").to_string());
                }
            }
            let st = self.api("GET", &format!("repos/{}/commits/{sha}/status", pr.repo), &[])?;
            for r in st["statuses"].as_array().cloned().unwrap_or_default() {
                if matches!(r["state"].as_str(), Some("failure" | "error")) {
                    out.push(r["context"].as_str().unwrap_or("").to_string());
                }
            }
        }
        out.retain(|n| !n.is_empty());
        out.sort();
        out.dedup();
        Ok(out)
    }

    fn base_failed_checks(&self, pr: &PrRef, base: &str, commits: usize) -> HostResult<Vec<Check>> {
        let list = self.api("GET", &format!("repos/{}/commits?sha={base}&per_page={commits}", pr.repo), &[])?;
        let mut out: Vec<Check> = vec![];
        let mut add = |name: &str, url: Option<&str>| {
            let url = url.filter(|u| !u.is_empty()).map(|u| u.to_string());
            if !name.is_empty() && !out.iter().any(|o| o.name == name && o.url == url) {
                out.push(Check { name: name.to_string(), state: "failed".into(), url });
            }
        };
        for c in list.as_array().cloned().unwrap_or_default().into_iter().take(commits) {
            let Some(sha) = c["sha"].as_str() else { continue };
            let runs = self.api("GET", &format!("repos/{}/commits/{sha}/check-runs?per_page=100", pr.repo), &[])?;
            for r in runs["check_runs"].as_array().cloned().unwrap_or_default() {
                if matches!(r["conclusion"].as_str(), Some("failure" | "timed_out")) {
                    add(r["name"].as_str().unwrap_or(""), r["details_url"].as_str().or(r["html_url"].as_str()));
                }
            }
            let st = self.api("GET", &format!("repos/{}/commits/{sha}/status", pr.repo), &[])?;
            for r in st["statuses"].as_array().cloned().unwrap_or_default() {
                if matches!(r["state"].as_str(), Some("failure" | "error")) {
                    add(r["context"].as_str().unwrap_or(""), r["target_url"].as_str());
                }
            }
        }
        Ok(out)
    }
}

fn check_state(c: &Value) -> &'static str {
    match (c["status"].as_str(), c["conclusion"].as_str(), c["state"].as_str()) {
        (_, _, Some(s)) => match s {
            "SUCCESS" => "passed",
            "FAILURE" | "ERROR" => "failed",
            _ => "running",
        },
        (Some("COMPLETED"), Some(con), _) => match con {
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => "passed",
            "CANCELLED" | "STALE" => "stopped",
            _ => "failed",
        },
        _ => "running",
    }
}

fn login(v: &Value) -> String {
    v["author"]["login"].as_str().unwrap_or("").to_string()
}

/// `gh pr view --json …` and the review threads' GraphQL answer, as one record.
/// The comments of `RECENT_COMMENTS_QUERY`'s answer: plain comments, review summaries and review
/// comments, each with its own time.
pub fn recent_comments(v: &Value) -> Vec<Comment> {
    let mut out = vec![];
    let nodes = |x: &Value| x["nodes"].as_array().cloned().unwrap_or_default();
    let mut add = |pr: i64, c: &Value, at: &str| {
        let text = c["body"].as_str().unwrap_or("");
        if text.trim().is_empty() {
            return;
        }
        let who = login(c);
        out.push(Comment { pr, id: c["id"].as_str().unwrap_or("").to_string(), author: who.clone(), author_name: who, at: c[at].as_str().unwrap_or("").to_string(), text: text.to_string() });
    };
    for p in nodes(&v["data"]["repository"]["pullRequests"]) {
        let pr = p["number"].as_i64().unwrap_or(0);
        for c in nodes(&p["comments"]) {
            add(pr, &c, "createdAt");
        }
        for r in nodes(&p["reviews"]) {
            add(pr, &r, "submittedAt");
            for c in nodes(&r["comments"]) {
                add(pr, &c, "createdAt");
            }
        }
    }
    out
}

pub fn summarize(d: &Value, threads: &Value) -> Record {
    let author = d["author"]["login"].as_str().unwrap_or("").to_string();
    let checks: Vec<Check> = d["statusCheckRollup"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|c| Check {
            name: c["name"].as_str().or(c["context"].as_str()).unwrap_or("check").to_string(),
            state: check_state(c).to_string(),
            url: c["detailsUrl"].as_str().or(c["targetUrl"].as_str()).filter(|u| !u.is_empty()).map(|u| u.to_string()),
        })
        .collect();
    let reviews = d["reviews"].as_array().cloned().unwrap_or_default();
    let others = |a: &Value| login(a) != author;
    let comments_v = d["comments"].as_array().cloned().unwrap_or_default();
    let comments = comments_v.iter().filter(|c| others(c)).count()
        + reviews
            .iter()
            .filter(|r| others(r))
            .filter(|r| r["state"] != "APPROVED")
            .filter(|r| r["state"] == "CHANGES_REQUESTED" || r["body"].as_str().map(|b| !b.trim().is_empty()).unwrap_or(false))
            .count();
    let changes_at = reviews
        .iter()
        .filter(|r| r["state"] == "CHANGES_REQUESTED" && others(r))
        .filter_map(|r| r["submittedAt"].as_str())
        .max()
        .map(|s| s.to_string());
    let requested: Vec<String> = d["reviewRequests"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r["login"].as_str().or(r["slug"].as_str()).map(|s| s.to_string()))
        .collect();
    // Each reviewer's latest review (older gh: every review, the last one per person wins).
    let latest = d["latestReviews"].as_array().cloned().unwrap_or_else(|| reviews.clone());
    let mut reviewers: Vec<Reviewer> = vec![];
    for r in latest.iter().filter(|r| others(r)) {
        let user = login(r);
        let state = match r["state"].as_str().unwrap_or("") {
            "APPROVED" => "approved",
            "CHANGES_REQUESTED" => "changes",
            "COMMENTED" => "commented",
            _ => "pending",
        };
        // When they asked for changes: this review's time, else their latest request among every review.
        let mine = (state == "changes").then(|| {
            r["submittedAt"].as_str().map(|s| s.to_string()).or_else(|| {
                reviews.iter().filter(|x| x["state"] == "CHANGES_REQUESTED" && login(x) == user).filter_map(|x| x["submittedAt"].as_str()).max().map(|s| s.to_string())
            })
        });
        let mine = mine.flatten();
        match reviewers.iter_mut().find(|x| x.user == user) {
            Some(x) => {
                x.state = state.into();
                x.changes_at = mine;
            }
            None => reviewers.push(Reviewer { name: user.clone(), user, state: state.into(), requested: false, changes_at: mine }),
        }
    }
    for u in &requested {
        match reviewers.iter_mut().find(|x| &x.user == u) {
            Some(x) => x.requested = true,
            None => reviewers.push(Reviewer { user: u.clone(), name: u.clone(), state: "pending".into(), requested: true, changes_at: None }),
        }
    }
    let approvals = reviewers.iter().filter(|r| r.state == "approved").count() as i64;
    let mut list: Vec<Thread> = vec![];
    let nodes = &threads["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"];
    for n in nodes.as_array().cloned().unwrap_or_default() {
        let first = &n["first"]["nodes"][0];
        let last = &n["last"]["nodes"][0];
        let last = if last.is_null() { first } else { last };
        let replies = n["replies"]["nodes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .skip(1)
            .map(|c| Reply {
                author: login(c),
                author_name: login(c),
                text: crate::util::one_line(c["body"].as_str().unwrap_or(""), REPLY_MAX),
                at: c["createdAt"].as_str().unwrap_or("").to_string(),
            })
            .collect();
        list.push(Thread {
            id: n["id"].as_str().unwrap_or("").to_string(),
            kind: "review".into(),
            resolvable: true,
            resolved: n["isResolved"].as_bool().unwrap_or(false),
            author: login(first),
            author_name: login(first),
            last_author: login(last),
            last_id: last["id"].as_str().unwrap_or("").to_string(),
            last_at: last["createdAt"].as_str().unwrap_or("").to_string(),
            path: n["path"].as_str().map(|s| s.to_string()),
            line: n["line"].as_i64(),
            text: gist(first["body"].as_str().unwrap_or("")),
            url: first["url"].as_str().map(|s| s.to_string()),
            outdated: n["isOutdated"].as_bool().unwrap_or(false),
            replies,
        });
    }
    let plain = |id: &Value, who: String, body: &str, at: &Value, url: &Value, kind: &str| Thread {
        id: id.as_str().unwrap_or("").to_string(),
        kind: kind.into(),
        resolvable: false,
        resolved: false,
        author_name: who.clone(),
        last_author: who.clone(),
        author: who,
        last_id: id.as_str().unwrap_or("").to_string(),
        last_at: at.as_str().unwrap_or("").to_string(),
        path: None,
        line: None,
        text: gist(body),
        url: url.as_str().map(|s| s.to_string()),
        outdated: false,
        replies: vec![],
    };
    for c in comments_v.iter().filter(|c| others(c)) {
        list.push(plain(&c["id"], login(c), c["body"].as_str().unwrap_or(""), &c["createdAt"], &c["url"], "comment"));
    }
    for r in reviews.iter().filter(|r| others(r) && r["body"].as_str().is_some_and(|b| !b.trim().is_empty())) {
        list.push(plain(&r["id"], login(r), r["body"].as_str().unwrap_or(""), &r["submittedAt"], &r["url"], "summary"));
    }
    list.retain(|t| !t.id.is_empty());
    Record {
        state: d["state"].as_str().unwrap_or("OPEN").to_string(),
        title: d["title"].as_str().unwrap_or("").to_string(),
        author,
        head: d["headRefOid"].as_str().unwrap_or("").to_string(),
        branch: d["headRefName"].as_str().unwrap_or("").to_string(),
        base: d["baseRefName"].as_str().unwrap_or("").to_string(),
        base_head: d["baseRefOid"].as_str().unwrap_or("").to_string(),
        checks,
        review_decision: d["reviewDecision"].as_str().unwrap_or("").to_string(),
        approvals,
        changes_at,
        comments: comments as i64,
        reviewers,
        threads: list,
        tasks_open: 0,
        tasks_error: None,
        viewer: String::new(),
        mergeable: d["mergeable"].clone(),
    }
}

/// A GitHub run id from a check's link (`…/actions/runs/<id>/job/<job>`).
pub fn run_id(url: &str) -> Option<String> {
    let re = regex::Regex::new(r"/actions/runs/(\d+)").unwrap();
    re.captures(url).map(|c| c[1].to_string())
}

/// Failed steps of a GitHub Actions run (`gh run view <id> --json jobs`), as "job › step".
pub fn failed_steps(jobs: &Value) -> Vec<String> {
    let mut out = vec![];
    for j in jobs["jobs"].as_array().cloned().unwrap_or_default() {
        if !matches!(j["conclusion"].as_str(), Some("failure" | "timed_out")) {
            continue;
        }
        let job = j["name"].as_str().unwrap_or("job");
        let steps: Vec<String> = j["steps"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter(|s| matches!(s["conclusion"].as_str(), Some("failure" | "timed_out")))
            .map(|s| format!("{job} › {}", s["name"].as_str().unwrap_or("step")))
            .collect();
        if steps.is_empty() {
            out.push(job.to_string());
        } else {
            out.extend(steps);
        }
    }
    out
}

impl GithubHost {
    /// `gh run view` for a check's run: its failed steps.
    pub fn run_failures(&self, repo: &str, run: &str) -> HostResult<Vec<String>> {
        let v = parse(&self.gh(s(&["run", "view", run, "-R", repo, "--json", "jobs"]))?)?;
        Ok(failed_steps(&v))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use parking_lot::Mutex;
    use serde_json::json;
    use std::sync::Arc;

    /// Answers each `gh` call with the first canned answer whose key the joined arguments contain.
    pub struct FakeGh {
        pub answers: Vec<(&'static str, String)>,
        pub calls: Arc<Mutex<Vec<Vec<String>>>>,
    }

    impl Gh for FakeGh {
        fn run(&self, args: &[String]) -> HostResult<String> {
            self.calls.lock().push(args.to_vec());
            let joined = args.join(" ");
            self.answers.iter().find(|(k, _)| joined.contains(k)).map(|(_, v)| v.clone()).ok_or_else(|| format!("no answer for {joined}"))
        }
    }

    fn pr() -> PrRef {
        PrRef { host: "github".into(), repo: "acme/webapp".into(), num: 9, url: "https://github.com/acme/webapp/pull/9".into() }
    }

    fn view() -> Value {
        json!({
            "state": "OPEN", "title": "Add x", "author": {"login": "me"}, "headRefOid": "abc", "headRefName": "feat",
            "baseRefName": "main", "baseRefOid": "b1", "reviewDecision": "CHANGES_REQUESTED",
            "statusCheckRollup": [
                {"name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"},
                {"name": "test", "status": "COMPLETED", "conclusion": "FAILURE", "detailsUrl": "https://github.com/acme/webapp/actions/runs/77/job/1"},
                {"context": "ci/legacy", "state": "PENDING"}
            ],
            "comments": [{"id": "IC_1", "author": {"login": "me"}, "body": "mine"}, {"id": "IC_2", "author": {"login": "rev"}, "body": "Why?\nmore"}],
            "reviews": [{"id": "PRR_1", "author": {"login": "rev"}, "state": "CHANGES_REQUESTED", "body": "", "submittedAt": "2026-10-01T10:00:00Z"}],
            "latestReviews": [{"author": {"login": "rev"}, "state": "CHANGES_REQUESTED"}, {"author": {"login": "ok"}, "state": "APPROVED"}],
            "reviewRequests": [{"login": "new"}]
        })
    }

    fn threads() -> Value {
        json!({"data": {"repository": {"pullRequest": {"reviewThreads": {"nodes": [
            {"id": "PRRT_1", "isResolved": false, "isOutdated": false, "path": "src/a.rs", "line": 3,
             "first": {"nodes": [{"id": "c1", "author": {"login": "rev"}, "body": "Rename this", "createdAt": "t1"}]},
             "last": {"nodes": [{"id": "c2", "author": {"login": "me"}, "body": "Done", "createdAt": "t2"}]},
             "replies": {"nodes": [{"id": "c1", "author": {"login": "rev"}, "body": "Rename this", "createdAt": "t1"},
                                   {"id": "c2", "author": {"login": "me"}, "body": "Done\nin a.rs", "createdAt": "t2"}]}},
            {"id": "PRRT_2", "isResolved": false, "path": "src/b.rs",
             "first": {"nodes": [{"id": "c3", "author": {"login": "rev"}, "body": "And this", "createdAt": "t3"}]},
             "last": {"nodes": [{"id": "c3", "author": {"login": "rev"}, "body": "And this", "createdAt": "t3"}]}}
        ]}}}}})
    }

    #[test]
    fn summarizes_gh_output() {
        let r = summarize(&view(), &threads());
        assert_eq!(r.failed(), vec!["test".to_string()]);
        let v = r.to_value();
        assert_eq!(v["running"], 1);
        assert_eq!(v["comments"], 2);
        assert_eq!(r.approvals, 1);
        assert_eq!(r.base_head, "b1");
        assert_eq!(r.checks[1].url.as_deref(), Some("https://github.com/acme/webapp/actions/runs/77/job/1"));
        let states: Vec<(String, String, bool)> = r.reviewers.iter().map(|x| (x.user.clone(), x.state.clone(), x.requested)).collect();
        let at: Vec<Option<&str>> = r.reviewers.iter().map(|x| x.changes_at.as_deref()).collect();
        assert_eq!(at, vec![Some("2026-10-01T10:00:00Z"), None, None], "each reviewer's own request for changes");
        assert_eq!(
            states,
            vec![("rev".into(), "changes".into(), false), ("ok".into(), "approved".into(), false), ("new".into(), "pending".into(), true)]
        );
        let open: Vec<&str> = r.threads.iter().filter(|t| t.waiting_on("me")).map(|t| t.id.as_str()).collect();
        assert_eq!(open, vec!["PRRT_2", "IC_2"], "the first thread had my last word; my own comment isn't a thread");
        assert_eq!(r.threads.iter().find(|t| t.id == "IC_2").unwrap().text, "Why?");
        let t1 = r.threads.iter().find(|t| t.id == "PRRT_1").unwrap();
        assert_eq!(t1.replies, vec![Reply { author: "me".into(), author_name: "me".into(), text: "Done in a.rs".into(), at: "t2".into() }]);
    }

    #[test]
    fn talks_to_gh_for_each_call() {
        let calls = Arc::new(Mutex::new(vec![]));
        let gh = FakeGh {
            answers: vec![
                ("pr view", view().to_string()),
                ("reviewThreads", threads().to_string()),
                ("addPullRequestReviewThreadReply", "{}".into()),
                ("resolveReviewThread", "{}".into()),
                ("repo view", json!({"viewerDefaultMergeMethod": "SQUASH"}).to_string()),
                ("pr merge", String::new()),
                ("pr comment", String::new()),
                ("requested_reviewers", "{}".into()),
                ("run list", json!([{"databaseId": 5, "status": "in_progress", "workflowName": "CI", "number": 12}, {"databaseId": 6, "status": "completed"}]).to_string()),
                ("pr list", json!([{"number": 9, "headRefName": "feat", "url": "https://github.com/acme/webapp/pull/9"}]).to_string()),
                ("run cancel", String::new()),
                ("pr edit", String::new()),
                ("repos/acme/webapp/pulls -f", json!({"number": 30, "html_url": "https://github.com/acme/webapp/pull/30"}).to_string()),
            ],
            calls: calls.clone(),
        };
        let h = GithubHost::new(Box::new(gh));
        let rec = h.read(&pr()).unwrap();
        let t = rec.threads.iter().find(|t| t.id == "PRRT_2").unwrap();
        h.reply(&pr(), t, "Renamed").unwrap();
        assert!(h.resolve(&pr(), t).unwrap());
        let c = rec.threads.iter().find(|t| t.id == "IC_2").unwrap();
        assert!(!h.resolve(&pr(), c).unwrap(), "a plain comment can't be resolved on GitHub");
        h.reply(&pr(), c, "Because.").unwrap();
        h.merge(&pr(), &MergeOpts { strategy: None, close_source_branch: true }).unwrap();
        h.re_request_reviews(&pr(), &["rev".into()]).unwrap();
        assert_eq!(h.cancel_builds(&pr(), "abc").unwrap(), Cancelled::Stopped(vec!["CI #12".into()]));
        assert_eq!(
            h.my_open_prs("acme/webapp").unwrap(),
            vec![OpenPr { num: 9, branch: "feat".into(), url: "https://github.com/acme/webapp/pull/9".into() }]
        );
        h.retarget(&pr(), "develop").unwrap();
        let opened = h.open("acme/webapp", "main", "feat", "Add x", "## Summary").unwrap();
        assert_eq!((opened.num, opened.url.as_str()), (30, "https://github.com/acme/webapp/pull/30"));
        let calls = calls.lock().clone();
        let has = |want: &[&str]| calls.iter().any(|c| want.iter().all(|w| c.iter().any(|a| a.contains(w))));
        assert!(has(&["pr", "merge", "--squash", "--delete-branch"]), "merges with the repo's default and closes the branch");
        assert!(has(&["comment", "> @rev: Why?\n\nBecause."]));
        assert!(has(&["POST", "repos/acme/webapp/pulls/9/requested_reviewers", "reviewers[]=rev"]));
        assert!(has(&["run", "cancel", "5"]));
        assert!(!has(&["run", "cancel", "6"]));
        assert!(has(&["--base", "develop"]));
        assert!(has(&["POST", "repos/acme/webapp/pulls", "head=feat", "base=main", "title=Add x"]));
    }

    #[test]
    fn lists_a_repo_s_recent_comments_whole_with_their_own_times() {
        let answer = json!({"data": {"repository": {"pullRequests": {"nodes": [
            {"number": 12, "comments": {"nodes": [{"id": "IC_1", "author": {"login": "ana"}, "body": "Looks fine\n\n<sub>AI review</sub>", "createdAt": "2026-10-08T09:00:00Z"}]},
             "reviews": {"nodes": [{"id": "PRR_1", "author": {"login": "ana"}, "body": "", "submittedAt": "2026-10-08T09:01:00Z",
                                    "comments": {"nodes": [{"id": "RC_1", "author": {"login": "ana"}, "body": "Rename", "createdAt": "2026-10-08T09:02:00Z"}]}}]}}
        ]}}}});
        let calls = Arc::new(Mutex::new(vec![]));
        let h = GithubHost::new(Box::new(FakeGh { answers: vec![("pullRequests(first:$n", answer.to_string())], calls: calls.clone() }));
        let got = h.recent_comments("acme/webapp", 20).unwrap();
        assert_eq!(got.iter().map(|c| (c.pr, c.id.as_str(), c.at.as_str())).collect::<Vec<_>>(),
                   vec![(12, "IC_1", "2026-10-08T09:00:00Z"), (12, "RC_1", "2026-10-08T09:02:00Z")], "an empty review summary isn't a comment");
        assert!(got[0].text.ends_with("<sub>AI review</sub>"), "the whole comment, footer too");
        assert!(calls.lock()[0].iter().any(|a| a == "n=20"));
    }

    #[test]
    fn reads_failed_steps_of_a_run() {
        assert_eq!(run_id("https://github.com/a/b/actions/runs/123/job/9").as_deref(), Some("123"));
        let jobs = json!({"jobs": [
            {"name": "test", "conclusion": "failure", "steps": [{"name": "checkout", "conclusion": "success"}, {"name": "cargo test", "conclusion": "failure"}]},
            {"name": "lint", "conclusion": "success", "steps": []}
        ]});
        assert_eq!(failed_steps(&jobs), vec!["test › cargo test".to_string()]);
    }
}
