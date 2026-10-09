//! CI providers: a failed check's failed steps and failing tests, for `tb pr status`.
//!
//! [`provider_for`] picks one per check: the project's own `failures_cmd` when it has one
//! (`[pr.projects.<name>]`), else by the check's link: GitHub Actions (`gh run view --json jobs`),
//! Bitbucket Pipelines (the run's steps and their test reports), or Azure Pipelines (the build's
//! timeline and failed test results, with the CI token stored with `tb ci-token set`, else the env var
//! `pr.azure_token_env`). A check none of them can read gets no detail; `tb pr status` says so.

use std::path::Path;

use serde_json::Value;

use super::bitbucket::{self, BasicHttp, BitbucketHost, Http};
use super::github::{self, GhCli, GithubHost};
use super::{Check, HostResult, PrRef};
use crate::app::App;
use crate::config::PrProject;

/// What a failed check's CI says went wrong.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Failures {
    pub steps: Vec<String>,
    pub tests: Vec<String>,
}

/// One CI system the board can ask about a failed check.
pub trait CiProvider {
    /// For people: "GitHub Actions", "Azure Pipelines", "the project's failures_cmd".
    fn name(&self) -> &'static str;
    fn failures(&self, pr: &PrRef, head: &str, check: &Check) -> HostResult<Failures>;
}

/// Which provider reads a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Command,
    GithubActions,
    BitbucketPipelines,
    Azure,
}

/// The provider for a check, by the project's rules and the check's link.
pub fn pick(project: &PrProject, check: &Check) -> Option<Kind> {
    if project.failures_cmd.as_deref().is_some_and(|c| !c.trim().is_empty()) {
        return Some(Kind::Command);
    }
    let url = check.url.as_deref().unwrap_or("");
    if github::run_id(url).is_some() {
        return Some(Kind::GithubActions);
    }
    if bitbucket::pipeline_build(url).is_some() {
        return Some(Kind::BitbucketPipelines);
    }
    if azure_build(url).is_some() {
        return Some(Kind::Azure);
    }
    None
}

pub fn provider_for(app: &App, project: &PrProject, check: &Check) -> HostResult<Option<Box<dyn CiProvider>>> {
    Ok(match pick(project, check) {
        None => None,
        Some(Kind::Command) => Some(Box::new(CommandCi { cmd: project.failures_cmd.clone().unwrap_or_default() })),
        Some(Kind::GithubActions) => {
            let gh = crate::proc::which(&app.cfg.pr.gh).ok_or_else(|| "the gh command isn't installed".to_string())?;
            Some(Box::new(GithubActions { host: GithubHost::new(Box::new(GhCli { bin: gh, env: crate::accounts::gh_env(&app.cfg) })) }))
        }
        Some(Kind::BitbucketPipelines) => {
            let (email, token) = crate::accounts::board_credentials(app, crate::accounts::Provider::Bitbucket)
                .ok_or_else(|| "Bitbucket isn't connected".to_string())?;
            Some(Box::new(BitbucketPipelines { host: BitbucketHost::new(Box::new(BasicHttp::new(&email, &token)), &app.cfg.pr.bitbucket_api) }))
        }
        Some(Kind::Azure) => {
            let (_, token) = crate::accounts::ci_token(app)
                .ok_or_else(|| format!("no Azure DevOps token: store one with tb ci-token set (or set ${})", app.cfg.pr.azure_token_env))?;
            Some(Box::new(AzurePipelines { http: Box::new(BasicHttp::for_service("Azure DevOps", "", &token)) }))
        }
    })
}

pub struct GithubActions {
    pub host: GithubHost,
}

impl CiProvider for GithubActions {
    fn name(&self) -> &'static str {
        "GitHub Actions"
    }
    fn failures(&self, pr: &PrRef, _head: &str, check: &Check) -> HostResult<Failures> {
        let run = github::run_id(check.url.as_deref().unwrap_or("")).ok_or_else(|| "its link isn't a GitHub Actions run".to_string())?;
        Ok(Failures { steps: self.host.run_failures(&pr.repo, &run)?, tests: vec![] })
    }
}

pub struct BitbucketPipelines {
    pub host: BitbucketHost,
}

impl CiProvider for BitbucketPipelines {
    fn name(&self) -> &'static str {
        "Bitbucket Pipelines"
    }
    fn failures(&self, pr: &PrRef, _head: &str, check: &Check) -> HostResult<Failures> {
        let build = bitbucket::pipeline_build(check.url.as_deref().unwrap_or("")).ok_or_else(|| "its link isn't a Pipelines run".to_string())?;
        let (steps, tests) = self.host.pipeline_failures(&pr.repo, build)?;
        Ok(Failures { steps, tests })
    }
}

/// An Azure Pipelines build from a check's link: (the project's API base, build id).
pub fn azure_build(url: &str) -> Option<(String, String)> {
    let re = regex::Regex::new(r"^https://(dev\.azure\.com/[^/]+|[^/.]+\.visualstudio\.com)/([^/]+)/_build/results\?(?:.*&)?buildId=(\d+)").unwrap();
    let c = re.captures(url)?;
    Some((format!("https://{}/{}", &c[1], &c[2]), c[3].to_string()))
}

pub struct AzurePipelines {
    pub http: Box<dyn Http>,
}

impl CiProvider for AzurePipelines {
    fn name(&self) -> &'static str {
        "Azure Pipelines"
    }
    fn failures(&self, _pr: &PrRef, _head: &str, check: &Check) -> HostResult<Failures> {
        let (base, id) = azure_build(check.url.as_deref().unwrap_or("")).ok_or_else(|| "its link isn't an Azure Pipelines build".to_string())?;
        let tl = self.http.call("GET", &format!("{base}/_apis/build/builds/{id}/timeline?api-version=7.1"), None)?;
        let mut steps = vec![];
        for r in tl["records"].as_array().cloned().unwrap_or_default() {
            if r["type"] != "Task" || r["result"] != "failed" {
                continue;
            }
            let name = r["name"].as_str().unwrap_or("step").to_string();
            let why = r["issues"].as_array().and_then(|a| a.iter().find(|i| i["type"] == "error")).and_then(|i| i["message"].as_str()).map(super::gist);
            steps.push(match why {
                Some(w) => format!("{name}: {w}"),
                None => name,
            });
        }
        let mut tests = vec![];
        let runs = self.http.call("GET", &format!("{base}/_apis/test/runs?buildUri=vstfs:///Build/Build/{id}&api-version=7.1"), None)?;
        for run in runs["value"].as_array().cloned().unwrap_or_default() {
            let Some(rid) = run["id"].as_i64() else { continue };
            let res = self.http.call("GET", &format!("{base}/_apis/test/Runs/{rid}/results?outcomes=Failed&$top=50&api-version=7.1"), None)?;
            tests.extend(
                res["value"].as_array().cloned().unwrap_or_default().iter().filter_map(|t| t["automatedTestName"].as_str().or(t["testCaseTitle"].as_str()).map(|s| s.to_string())),
            );
        }
        Ok(Failures { steps, tests })
    }
}

/// A project's own command (`failures_cmd`).
pub struct CommandCi {
    pub cmd: String,
}

impl CiProvider for CommandCi {
    fn name(&self) -> &'static str {
        "the project's failures_cmd"
    }
    fn failures(&self, pr: &PrRef, head: &str, check: &Check) -> HostResult<Failures> {
        let cmd = crate::util::expand_home(self.cmd.trim()).to_string_lossy().to_string();
        let env: Vec<(String, String)> = [
            ("TB_PR_URL", pr.url.clone()),
            ("TB_PR_REPO", pr.repo.clone()),
            ("TB_PR_NUM", pr.num.to_string()),
            ("TB_HEAD", head.to_string()),
            ("TB_CHECK", check.name.clone()),
            ("TB_CHECK_URL", check.url.clone().unwrap_or_default()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let o = crate::proc::run_with(Path::new("/bin/sh"), &["-c".into(), cmd], None, 60.0, &env, None).map_err(|_| "failures_cmd didn't finish in 60 seconds".to_string())?;
        if o.code != Some(0) {
            return Err(format!("failures_cmd failed: {}", o.stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("no output")));
        }
        Ok(parse_command_output(&o.stdout))
    }
}

/// `{"steps": [...], "tests": [...]}`, or one step per line with `test: <name>` lines for tests.
pub fn parse_command_output(out: &str) -> Failures {
    if let Ok(v) = serde_json::from_str::<Value>(out.trim()) {
        if v.is_object() {
            let list = |k: &str| v[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
            return Failures { steps: list("steps"), tests: list("tests") };
        }
    }
    let mut f = Failures::default();
    for line in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match line.strip_prefix("test:") {
            Some(t) => f.tests.push(t.trim().to_string()),
            None => f.steps.push(line.to_string()),
        }
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use serde_json::json;
    use std::sync::Arc;

    fn check(url: &str) -> Check {
        Check { name: "ci".into(), state: "failed".into(), url: Some(url.into()), at: None }
    }

    #[test]
    fn picks_a_provider_by_the_project_then_the_link() {
        let none = PrProject::default();
        assert_eq!(pick(&none, &check("https://github.com/a/b/actions/runs/1/job/2")), Some(Kind::GithubActions));
        assert_eq!(pick(&none, &check("https://bitbucket.org/w/r/pipelines/results/3")), Some(Kind::BitbucketPipelines));
        assert_eq!(pick(&none, &check("https://dev.azure.com/org/Proj/_build/results?buildId=55&view=logs")), Some(Kind::Azure));
        assert_eq!(pick(&none, &check("https://jenkins.example.com/job/1")), None);
        let cmd = PrProject { failures_cmd: Some("ci-why".into()), ..Default::default() };
        assert_eq!(pick(&cmd, &check("https://github.com/a/b/actions/runs/1")), Some(Kind::Command), "the project's command wins");
        assert_eq!(azure_build("https://acme.visualstudio.com/Proj/_build/results?view=x&buildId=9"), Some(("https://acme.visualstudio.com/Proj".into(), "9".into())));
    }

    #[test]
    fn reads_a_command_s_json_or_lines() {
        assert_eq!(parse_command_output(r#"{"steps": ["build"], "tests": ["a::b"]}"#), Failures { steps: vec!["build".into()], tests: vec!["a::b".into()] });
        assert_eq!(parse_command_output("Run tests\ntest: login works\n\n"), Failures { steps: vec!["Run tests".into()], tests: vec!["login works".into()] });
    }

    #[test]
    fn a_command_gets_the_check_in_its_environment() {
        let pr = PrRef { host: "bitbucket".into(), repo: "w/r".into(), num: 4, url: "u".into() };
        let f = CommandCi { cmd: "echo \"$TB_CHECK on $TB_HEAD\"; echo test: $TB_PR_NUM".into() }.failures(&pr, "abc", &check("x")).unwrap();
        assert_eq!(f, Failures { steps: vec!["ci on abc".into()], tests: vec!["4".into()] });
        assert!(CommandCi { cmd: "echo nope >&2; exit 3".into() }.failures(&pr, "abc", &check("x")).unwrap_err().contains("nope"));
    }

    struct Canned(Vec<(&'static str, Value)>, Arc<Mutex<Vec<String>>>);
    impl Http for Canned {
        fn call(&self, _m: &str, url: &str, _b: Option<&Value>) -> HostResult<Value> {
            self.1.lock().push(url.to_string());
            self.0.iter().find(|(k, _)| url.contains(k)).map(|(_, v)| v.clone()).ok_or_else(|| format!("no answer for {url}"))
        }
    }

    #[test]
    fn reads_an_azure_build_s_failed_tasks_and_tests() {
        let calls = Arc::new(Mutex::new(vec![]));
        let http = Canned(
            vec![
                ("/timeline", json!({"records": [
                    {"type": "Task", "name": "Run tests", "result": "failed", "issues": [{"type": "warning", "message": "w"}, {"type": "error", "message": "exit 1\nmore"}]},
                    {"type": "Job", "name": "Build", "result": "failed"},
                    {"type": "Task", "name": "Lint", "result": "succeeded"}
                ]})),
                ("/test/runs?", json!({"value": [{"id": 8}]})),
                ("/test/Runs/8/results", json!({"value": [{"automatedTestName": "Suite.login"}, {"testCaseTitle": "logout"}]})),
            ],
            calls.clone(),
        );
        let az = AzurePipelines { http: Box::new(http) };
        let pr = PrRef { host: "github".into(), repo: "a/b".into(), num: 1, url: String::new() };
        let f = az.failures(&pr, "h", &check("https://dev.azure.com/org/Proj/_build/results?buildId=55")).unwrap();
        assert_eq!(f.steps, vec!["Run tests: exit 1".to_string()]);
        assert_eq!(f.tests, vec!["Suite.login".to_string(), "logout".to_string()]);
        assert!(calls.lock()[0].starts_with("https://dev.azure.com/org/Proj/_apis/build/builds/55/timeline"));
    }
}
