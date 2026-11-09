//! CI providers: a failed check's failed steps and failing tests, for `tb pr status`.
//!
//! [`provider_for`] picks one per check: the project's own `failures_cmd` when it has one
//! (`[pr.projects.<name>]`), else by the check's link: GitHub Actions (`gh run view --json jobs`),
//! Bitbucket Pipelines (the run's steps and their test reports), or Azure Pipelines (the build's
//! timeline and failed test results, with the Azure DevOps account's token (Settings ▸ Accounts), else the env var
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
    /// Why its failed tests couldn't be read, when its steps could.
    pub tests_error: Option<String>,
    /// Each stage and job the build ran, with its result: read instead when its tests can't be.
    pub stages: Vec<String>,
}

/// One CI system the board can ask about a failed check.
pub trait CiProvider {
    /// For people: "GitHub Actions", "Azure Pipelines", "the project's failures_cmd".
    fn name(&self) -> &'static str;
    fn failures(&self, pr: &PrRef, head: &str, check: &Check) -> HostResult<Failures>;
    /// Each stage and job the check's build ran, with its result, for a project's required stages
    /// (#928). None: this CI can't list them.
    fn stages(&self, _pr: &PrRef, _head: &str, _check: &Check) -> HostResult<Option<Vec<Stage>>> {
        Ok(None)
    }
}

/// One stage or job of a build, with its result ("succeeded", "failed", "canceled", "skipped", or its
/// state while it hasn't finished).
#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    pub job: bool,
    pub name: String,
    /// The stage a job ran in, when the pipeline has stages.
    pub stage: Option<String>,
    pub result: String,
}

impl Stage {
    pub fn passed(&self) -> bool {
        matches!(self.result.as_str(), "succeeded" | "succeededWithIssues")
    }

    /// "stage Build: succeeded", "job Build › Unit tests: failed", "job Build: failed".
    pub fn line(&self) -> String {
        match (self.job, &self.stage) {
            (false, _) => format!("stage {}: {}", self.name, self.result),
            (true, Some(s)) => format!("job {s} › {}: {}", self.name, self.result),
            (true, None) => format!("job {}: {}", self.name, self.result),
        }
    }

    pub fn json(&self) -> Value {
        serde_json::json!({"job": self.job, "name": self.name, "stage": self.stage, "result": self.result})
    }

    pub fn from_json(v: &Value) -> Option<Stage> {
        Some(Stage {
            job: v["job"].as_bool().unwrap_or(false),
            name: v["name"].as_str()?.to_string(),
            stage: v["stage"].as_str().map(str::to_string),
            result: v["result"].as_str().unwrap_or("unknown").to_string(),
        })
    }
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
                // An agent can't use a token from the owner's shell (the daemon, started by launchd, doesn't see it).
                .ok_or_else(|| "no Azure DevOps token: the owner connects Azure DevOps in Taskboard ▸ Settings ▸ Accounts".to_string())?;
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
        Ok(Failures { steps: self.host.run_failures(&pr.repo, &run)?, ..Default::default() })
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
        Ok(Failures { steps, tests, ..Default::default() })
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
        let (base, id, records) = self.timeline(check)?;
        let mut steps = vec![];
        for r in records.iter() {
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
        // Test results need their own scope: a token without it still shows the steps, and each stage's
        // and job's result says whether the tests or a later stage failed.
        match self.failed_tests(&base, &id) {
            Ok(tests) => Ok(Failures { steps, tests, ..Default::default() }),
            Err(e) => Ok(Failures { steps, tests_error: Some(tests_error(e)), stages: stage_results(&records), ..Default::default() }),
        }
    }
    fn stages(&self, _pr: &PrRef, _head: &str, check: &Check) -> HostResult<Option<Vec<Stage>>> {
        Ok(Some(stage_list(&self.timeline(check)?.2)))
    }
}

impl AzurePipelines {
    fn timeline(&self, check: &Check) -> HostResult<(String, String, Vec<Value>)> {
        let (base, id) = azure_build(check.url.as_deref().unwrap_or("")).ok_or_else(|| "its link isn't an Azure Pipelines build".to_string())?;
        let tl = self.http.call("GET", &format!("{base}/_apis/build/builds/{id}/timeline?api-version=7.1"), None).map_err(|e| azure_error(e, "the build's steps", "Build (read)"))?;
        let records = tl["records"].as_array().cloned().unwrap_or_default();
        Ok((base, id, records))
    }

    fn failed_tests(&self, base: &str, id: &str) -> HostResult<Vec<String>> {
        let mut tests = vec![];
        let runs = self.http.call("GET", &format!("{base}/_apis/test/runs?buildUri=vstfs:///Build/Build/{id}&api-version=7.1"), None)?;
        for run in runs["value"].as_array().cloned().unwrap_or_default() {
            let Some(rid) = run["id"].as_i64() else { continue };
            let res = self.http.call("GET", &format!("{base}/_apis/test/Runs/{rid}/results?outcomes=Failed&$top=50&api-version=7.1"), None)?;
            tests.extend(
                res["value"].as_array().cloned().unwrap_or_default().iter().filter_map(|t| t["automatedTestName"].as_str().or(t["testCaseTitle"].as_str()).map(|s| s.to_string())),
            );
        }
        Ok(tests)
    }
}

/// An Azure DevOps refusal, saying what was being read; a 401 or 403 also names the scope the Azure
/// DevOps token needs for it, as Azure answers 401 to a token without that scope.
fn azure_error(e: String, what: &str, scope: &str) -> String {
    if e.contains("answered 401") || e.contains("answered 403") {
        return format!("{e} reading {what}: the Azure DevOps token needs {scope} (a new one goes in Taskboard ▸ Settings ▸ Accounts)");
    }
    format!("{e} reading {what}")
}

/// Why a build's test results couldn't be read; a 401 or 403 means the token lacks the Test Management scope.
fn tests_error(e: String) -> String {
    if e.contains("answered 401") || e.contains("answered 403") {
        return format!("the Azure DevOps token lacks Test Management (read): {e} (a new token goes in Taskboard ▸ Settings ▸ Accounts)");
    }
    format!("{e} reading the build's test results")
}

/// A build timeline's stages in order, each followed by its jobs: "stage Build: succeeded",
/// "job Build › Unit tests: failed". A pipeline without stages lists its jobs alone.
fn stage_results(records: &[Value]) -> Vec<String> {
    stage_list(records).iter().map(Stage::line).collect()
}

/// [`stage_results`], as stages and jobs.
fn stage_list(records: &[Value]) -> Vec<Stage> {
    let by_id: std::collections::HashMap<&str, &Value> = records.iter().filter_map(|r| Some((r["id"].as_str()?, r))).collect();
    let stage_of = |r: &Value| -> Option<String> {
        let mut at = by_id.get(r["parentId"].as_str()?).copied();
        for _ in 0..8 {
            let p = at?;
            if p["type"] == "Stage" {
                return p["id"].as_str().map(str::to_string);
            }
            at = by_id.get(p["parentId"].as_str()?).copied();
        }
        None
    };
    let outcome = |r: &Value| r["result"].as_str().or(r["state"].as_str()).unwrap_or("unknown").to_string();
    let named = |r: &Value| r["name"].as_str().unwrap_or("?").to_string();
    let ordered = |kind: &str| {
        let mut v: Vec<&Value> = records.iter().filter(|r| r["type"] == kind).collect();
        v.sort_by_key(|r| r["order"].as_i64().unwrap_or(i64::MAX));
        v
    };
    let jobs: Vec<(Option<String>, &Value)> = ordered("Job").into_iter().map(|j| (stage_of(j), j)).collect();
    let job = |j: &Value, stage: Option<String>| Stage { job: true, name: named(j), stage, result: outcome(j) };
    let mut out = vec![];
    for s in ordered("Stage") {
        let sid = s["id"].as_str().map(str::to_string);
        let shown = s["name"] != "__default";
        if shown {
            out.push(Stage { job: false, name: named(s), stage: None, result: outcome(s) });
        }
        for (_, j) in jobs.iter().filter(|(st, _)| *st == sid && sid.is_some()) {
            out.push(job(j, shown.then(|| named(s))));
        }
    }
    out.extend(jobs.iter().filter(|(st, _)| st.is_none()).map(|(_, j)| job(j, None)));
    out
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
        Ok(parse_command_output(&self.run(pr, head, check)?))
    }
    fn stages(&self, pr: &PrRef, head: &str, check: &Check) -> HostResult<Option<Vec<Stage>>> {
        Ok(command_stages(&self.run(pr, head, check)?))
    }
}

impl CommandCi {
    fn run(&self, pr: &PrRef, head: &str, check: &Check) -> HostResult<String> {
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
        Ok(o.stdout)
    }
}

/// The stages and jobs in a command's JSON (`"stages": [{"name": "Lint", "result": "succeeded", "job":
/// false, "stage": null}]`); None when it printed none.
pub fn command_stages(out: &str) -> Option<Vec<Stage>> {
    let v: Value = serde_json::from_str(out.trim()).ok()?;
    Some(v["stages"].as_array()?.iter().filter_map(Stage::from_json).collect())
}

/// `{"steps": [...], "tests": [...], "stages": [...]}` (stages: [`command_stages`]), or one step per line with `test: <name>` lines for tests.
pub fn parse_command_output(out: &str) -> Failures {
    if let Ok(v) = serde_json::from_str::<Value>(out.trim()) {
        if v.is_object() {
            let list = |k: &str| v[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
            return Failures { steps: list("steps"), tests: list("tests"), ..Default::default() };
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
        assert_eq!(parse_command_output(r#"{"steps": ["build"], "tests": ["a::b"]}"#), Failures { steps: vec!["build".into()], tests: vec!["a::b".into()], ..Default::default() });
        assert_eq!(parse_command_output("Run tests\ntest: login works\n\n"), Failures { steps: vec!["Run tests".into()], tests: vec!["login works".into()], ..Default::default() });
    }

    #[test]
    fn a_command_gets_the_check_in_its_environment() {
        let pr = PrRef { host: "bitbucket".into(), repo: "w/r".into(), num: 4, url: "u".into() };
        let f = CommandCi { cmd: "echo \"$TB_CHECK on $TB_HEAD\"; echo test: $TB_PR_NUM".into() }.failures(&pr, "abc", &check("x")).unwrap();
        assert_eq!(f, Failures { steps: vec!["ci on abc".into()], tests: vec!["4".into()], ..Default::default() });
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
        assert!(f.tests_error.is_none() && f.stages.is_empty(), "readable tests need no stage list");
    }

    struct NoTestScope(&'static str);
    impl Http for NoTestScope {
        fn call(&self, _m: &str, url: &str, _b: Option<&Value>) -> HostResult<Value> {
            if url.contains("/_apis/test/") {
                return Err(format!("Azure DevOps answered {}", self.0));
            }
            Ok(json!({"records": [
                {"id": "t1", "parentId": "j2", "type": "Task", "name": "Push image", "result": "failed", "order": 1},
                {"id": "s2", "type": "Stage", "name": "Deploy", "result": "failed", "order": 2},
                {"id": "s1", "type": "Stage", "name": "Build", "result": "succeeded", "order": 1},
                {"id": "p1", "parentId": "s1", "type": "Phase", "name": "Build", "order": 1},
                {"id": "p2", "parentId": "s2", "type": "Phase", "name": "Deploy", "order": 1},
                {"id": "j1b", "parentId": "p1", "type": "Job", "name": "Lint", "result": "succeeded", "order": 2},
                {"id": "j1a", "parentId": "p1", "type": "Job", "name": "Unit tests", "result": "succeeded", "order": 1},
                {"id": "j2", "parentId": "p2", "type": "Job", "name": "Publish", "result": "failed", "order": 1},
                {"id": "s3", "type": "Stage", "name": "Notify", "result": null, "state": "pending", "order": 3}
            ]}))
        }
    }

    #[test]
    fn unreadable_azure_test_results_say_so_and_list_each_stage_s_result() {
        let pr = PrRef { host: "github".into(), repo: "a/b".into(), num: 1, url: String::new() };
        for code in ["401", "403"] {
            let f = AzurePipelines { http: Box::new(NoTestScope(code)) }.failures(&pr, "h", &check("https://dev.azure.com/org/Proj/_build/results?buildId=55")).unwrap();
            assert_eq!(f.steps, vec!["Push image".to_string()]);
            assert!(f.tests.is_empty());
            let e = f.tests_error.unwrap();
            assert!(e.starts_with("the Azure DevOps token lacks Test Management (read)") && e.contains(&format!("answered {code}")), "{e}");
            assert_eq!(
                f.stages,
                vec!["stage Build: succeeded", "job Build › Unit tests: succeeded", "job Build › Lint: succeeded", "stage Deploy: failed", "job Deploy › Publish: failed", "stage Notify: pending"]
            );
        }
        let f = AzurePipelines { http: Box::new(NoTestScope("500")) }.failures(&pr, "h", &check("https://dev.azure.com/org/Proj/_build/results?buildId=55")).unwrap();
        assert_eq!(f.tests_error.as_deref(), Some("Azure DevOps answered 500 reading the build's test results"));
        assert_eq!(f.stages.len(), 6, "any unreadable test results fall back to the stages");
    }

    #[test]
    fn an_azure_build_lists_its_stages_and_jobs_with_their_results() {
        let pr = PrRef { host: "github".into(), repo: "a/b".into(), num: 1, url: String::new() };
        let az = AzurePipelines { http: Box::new(NoTestScope("401")) };
        let stages = az.stages(&pr, "h", &check("https://dev.azure.com/org/Proj/_build/results?buildId=55")).unwrap().unwrap();
        let tests = stages.iter().find(|s| s.name == "Unit tests").unwrap();
        assert!(tests.job && tests.passed() && tests.stage.as_deref() == Some("Build"));
        let publish = stages.iter().find(|s| s.name == "Publish").unwrap();
        assert!(!publish.passed());
        assert!(!stages.iter().find(|s| s.name == "Notify").unwrap().passed(), "a stage that hasn't finished hasn't passed");
        assert_eq!(Stage::from_json(&publish.json()).as_ref(), Some(publish));
    }

    #[test]
    fn a_command_can_list_stages_as_json() {
        let out = r#"{"steps": ["Upload"], "stages": [{"name": "Lint", "result": "succeeded"}, {"name": "Upload", "result": "canceled", "job": true, "stage": "Ship"}]}"#;
        let stages = command_stages(out).unwrap();
        assert_eq!(stages.len(), 2);
        assert!(stages[0].passed() && !stages[0].job);
        assert_eq!(stages[1].line(), "job Ship › Upload: canceled");
        assert_eq!(command_stages("Run tests\ntest: a"), None);
        assert_eq!(command_stages(r#"{"steps": []}"#), None);
        let pr = PrRef { host: "bitbucket".into(), repo: "w/r".into(), num: 4, url: "u".into() };
        let cmd = CommandCi { cmd: format!("echo '{out}'") };
        assert_eq!(cmd.stages(&pr, "abc", &check("x")).unwrap(), Some(stages));
        assert_eq!(cmd.failures(&pr, "abc", &check("x")).unwrap().steps, vec!["Upload".to_string()]);
    }

    #[test]
    fn a_pipeline_without_stages_lists_its_jobs() {
        let records = vec![
            json!({"id": "s", "type": "Stage", "name": "__default", "result": "failed", "order": 1}),
            json!({"id": "p", "parentId": "s", "type": "Phase", "name": "Job", "order": 1}),
            json!({"id": "j", "parentId": "p", "type": "Job", "name": "Build", "result": "failed", "order": 1}),
            json!({"id": "k", "type": "Job", "name": "Orphan", "state": "inProgress", "order": 2}),
        ];
        assert_eq!(stage_results(&records), vec!["job Build: failed", "job Orphan: inProgress"]);
    }
}
