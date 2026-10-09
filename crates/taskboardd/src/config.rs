//! Configuration: `config.toml` (every key optional) with environment overrides.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::util::expand_home;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct FileConfig {
    pub owner: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub data: Option<String>,
    pub page_url: Option<String>,
    pub midna: Option<String>,
    pub midna_bundle: Option<String>,
    pub open_midna: Option<bool>,
    pub notify: Option<bool>,
    pub first_weekday: Option<String>,
    pub claude: Option<String>,
    pub claude_projects: Option<String>,
    pub statusline_dir: Option<String>,
    pub runner: Option<bool>,
    pub intervals: Intervals,
    pub work_hours: WorkHoursDefault,
    pub questions: Questions,
    pub backlog: BacklogAi,
    pub pr: PrConfig,
    pub jira: JiraConfig,
    pub handoff: HandoffConfig,
    pub alerts: AlertsConfig,
    pub limits: LimitsConfig,
    pub attachments: AttachmentsConfig,
    pub terminals: TerminalsConfig,
    pub comments: CommentsConfig,
    pub devices: crate::devices::DevicesConfig,
    pub bits: crate::bits::BitsConfig,
}

/// Alerts' desktop notifications.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AlertsConfig {
    /// The snooze buttons on each alert's notification, in minutes (at most four; empty for none).
    pub snooze_mins: Vec<i64>,
}

impl Default for AlertsConfig {
    fn default() -> Self {
        AlertsConfig { snooze_mins: vec![15, 30, 60] }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Intervals {
    pub runner: f64,
    pub spool: f64,
    pub prs: f64,
    pub midna_sync: f64,
    pub midna_up: f64,
    pub running_job_expiry: f64,
    pub pending_job_expiry: f64,
    pub gone_after_missed: i64,
    pub md_debounce: f64,
}

impl Default for Intervals {
    fn default() -> Self {
        Intervals {
            runner: 5.0,
            spool: 10.0,
            prs: 60.0,
            midna_sync: 3.0,
            midna_up: 20.0,
            running_job_expiry: 360.0,
            pending_job_expiry: 3600.0,
            gone_after_missed: 2,
            md_debounce: 1.0,
        }
    }
}

/// The work hours the board starts with, before anyone changes them from the page or `tb hours`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WorkHoursDefault {
    pub on: bool,
    pub start: String,
    pub end: String,
    pub days: Vec<String>,
    pub alert_every_mins: i64,
}

impl Default for WorkHoursDefault {
    fn default() -> Self {
        WorkHoursDefault {
            on: false,
            start: "09:00".into(),
            end: "17:00".into(),
            days: ["mon", "tue", "wed", "thu", "fri"].iter().map(|s| s.to_string()).collect(),
            alert_every_mins: 10,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Questions {
    /// Screen agents' questions with a headless `claude -p` before flagging them for the owner.
    pub screen: bool,
    pub model: String,
    pub budget_usd: String,
    pub timeout_secs: u64,
    /// Extra rule files (markdown) the screener reads, e.g. the owner's memory notes.
    pub rules: Vec<String>,
}

impl Default for Questions {
    fn default() -> Self {
        Questions {
            screen: false,
            model: "claude-haiku-5-5".into(),
            budget_usd: "0.30".into(),
            timeout_secs: 75,
            rules: vec![],
        }
    }
}

/// Planning from the backlog: a headless `claude -p` groups new issues (area, impact, priority,
/// similar work) and splits the issues picked for a goal into waves.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BacklogAi {
    /// Ask Claude. Off (or no `claude`), the board groups by kind and plans waves by priority.
    pub ai: bool,
    pub model: String,
    pub budget_usd: String,
    pub timeout_secs: u64,
    /// How many terminals a goal made from the backlog runs at once.
    pub max_terminals: i64,
}

impl Default for BacklogAi {
    fn default() -> Self {
        BacklogAi { ai: true, model: "sonnet".into(), budget_usd: "1.00".into(), timeout_secs: 240, max_terminals: 2 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PrConfig {
    /// Watch open GitHub PRs with the `gh` CLI.
    pub watch: bool,
    /// Bring a done task's conversation back when its PR needs work.
    pub wake: bool,
    /// Agents may merge a PR once it's approved and green.
    pub agents_merge: bool,
    pub gh: String,
    /// Minutes after the head was pushed with no checks before the checks count as passed.
    pub no_checks_after_mins: f64,
    /// Bitbucket Cloud's REST API (2.0).
    pub bitbucket_api: String,
    /// The environment variable holding an Azure DevOps personal access token, for reading the failed
    /// steps and tests of an Azure Pipelines check.
    pub azure_token_env: String,
    /// Per-project PR rules, by the project's name (`[pr.projects.webapp]`).
    pub projects: BTreeMap<String, PrProject>,
}

impl Default for PrConfig {
    fn default() -> Self {
        PrConfig {
            watch: true,
            wake: true,
            agents_merge: false,
            gh: "gh".into(),
            no_checks_after_mins: 15.0,
            bitbucket_api: "https://api.bitbucket.org/2.0".into(),
            azure_token_env: "AZURE_DEVOPS_EXT_PAT".into(),
            projects: BTreeMap::new(),
        }
    }
}

impl PrConfig {
    /// The PR rules of a task's project (the defaults when it has none).
    pub fn project(&self, name: Option<&str>) -> PrProject {
        name.and_then(|n| self.projects.get(n)).cloned().unwrap_or_default()
    }
}

/// One project's PR rules. Every key is optional; unset keys keep the board-wide behaviour.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct PrProject {
    /// Approvals a PR needs before it's ready to merge. Unset: the host's own decision, or any approval.
    pub approvals: Option<i64>,
    /// Check names that must post on every push before the checks count as finished. Unset: the
    /// `no_checks_after_mins` grace; an empty list: no wait at all.
    pub expected: Option<Vec<String>>,
    /// How long to wait for the expected checks to post, in minutes.
    pub expected_wait_mins: Option<f64>,
    /// A command that prints a failed check's steps and tests (for CI the board can't read itself).
    /// It gets TB_PR_URL, TB_PR_REPO, TB_PR_NUM, TB_HEAD, TB_CHECK and TB_CHECK_URL, and prints JSON
    /// (`{"steps": [...], "tests": [...]}`) or one step per line (`test: <name>` for a test).
    pub failures_cmd: Option<String>,
    /// How `tb pr merge` merges: merge, squash or rebase (Bitbucket: merge_commit, squash,
    /// fast_forward). Unset: the repository's default.
    pub merge_strategy: Option<String>,
}

/// How long expected checks may take to post when a project doesn't say.
pub const EXPECTED_WAIT_MINS: f64 = 90.0;

/// What every handoff adds: the branch name to use and a footer (the owner's code style, say).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct HandoffConfig {
    /// Text added to the end of every handoff.
    pub footer: String,
    /// A file whose text is added to the end of every handoff (read each time), after `footer`.
    pub footer_file: String,
    /// The branch name a task's PR goes on: {type} {key} {slug} {task} {n}. Empty: the repo's convention.
    pub branch: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct JiraProduct {
    pub labels: Vec<String>,
    pub components: Vec<String>,
    /// Extra fields set on every ticket made for this product, as Jira's REST API takes them.
    pub fields: BTreeMap<String, toml::Value>,
    pub what: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct JiraConfig {
    /// e.g. "acme.atlassian.net". Jira is off while this is empty.
    pub site: String,
    pub project: String,
    pub email: String,
    pub token: String,
    /// macOS Keychain item holding the API token (account = email), used when `token` is empty.
    pub keychain_item: String,
    pub assignee_account_id: String,
    pub task_type: String,
    pub bug_type: String,
    pub epic_type: String,
    pub labels: Vec<String>,
    pub in_progress: String,
    pub in_review: String,
    pub done: String,
    pub merged: String,
    pub products: BTreeMap<String, JiraProduct>,
    /// How the board talks to Jira: "rest" (the API token) or "claude" (a headless `claude -p` with
    /// the Atlassian connector's tools, for a board with no token).
    pub via: String,
    /// The tools `claude -p` may use for Jira ("claude" via and the desk): the Atlassian connector's.
    pub claude_tools: Vec<String>,
    pub claude_model: String,
    pub claude_budget_usd: String,
    pub claude_timeout_secs: u64,
    /// Ask for a ticket for every queued or working task in a project that ships PRs; the task
    /// waits until it has one.
    pub auto_ticket: bool,
    /// Tickets are found or made by the Jira desk: one Claude terminal in Midna's Background group
    /// that searches Jira for an open ticket covering the work before it makes one.
    pub desk: bool,
    /// The folder the desk's terminal opens in (the board's data folder when empty).
    pub desk_dir: String,
    /// The product a ticket gets when its goal has none and nothing in the work picks one.
    pub default_product: String,
}

impl Default for JiraConfig {
    fn default() -> Self {
        JiraConfig {
            site: String::new(),
            project: String::new(),
            email: String::new(),
            token: String::new(),
            keychain_item: "taskboard-jira".into(),
            assignee_account_id: String::new(),
            task_type: "Task".into(),
            bug_type: "Bug".into(),
            epic_type: "Epic".into(),
            labels: vec![],
            in_progress: "In Progress".into(),
            in_review: "In Review".into(),
            done: String::new(),
            merged: String::new(),
            products: BTreeMap::new(),
            via: "rest".into(),
            claude_tools: vec!["mcp__claude_ai_Atlassian".into(), "mcp__atlassian".into()],
            claude_model: "sonnet".into(),
            claude_budget_usd: "0.50".into(),
            claude_timeout_secs: 180,
            auto_ticket: false,
            desk: false,
            desk_dir: String::new(),
            default_product: String::new(),
        }
    }
}

/// How big an agent's conversation may get, and when one is too old or too big to resume. `tb limits`
/// changes these on the board; these are where it starts. 0 turns a limit off.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LimitsConfig {
    /// Tokens at which Claude compacts a board agent's conversation (`--settings` autoCompactWindow).
    pub compact_window: i64,
    /// Minutes idle after which a conversation is compacted before it's resumed.
    pub cold_idle_mins: i64,
    /// A task or PR conversation resumes only under this many tokens; a bigger one starts fresh from the handoff.
    pub warm_tokens: i64,
    /// …and only when it's been idle under this many minutes.
    pub warm_idle_mins: i64,
    /// Generated-file globs marked `-diff` in every active project's `.git/info/attributes`.
    pub generated: Vec<String>,
    /// More globs for one project, by its name.
    pub project_generated: BTreeMap<String, Vec<String>>,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        LimitsConfig {
            compact_window: 150_000,
            cold_idle_mins: 60,
            warm_tokens: 60_000,
            warm_idle_mins: 60,
            generated: vec![],
            project_generated: BTreeMap::new(),
        }
    }
}

/// Files `tb attach` refuses, by extension: writing goes up as a brief artifact, not a loose file.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AttachmentsConfig {
    pub refuse: Vec<String>,
    /// What the agent is told; `{ext}` is the refused extension.
    pub refuse_message: String,
}

impl Default for AttachmentsConfig {
    fn default() -> Self {
        AttachmentsConfig {
            refuse: [".md", ".txt", ".rst", ".html"].iter().map(|s| s.to_string()).collect(),
            refuse_message: "The board doesn't take {ext} files as attachments: publish it as a brief artifact and attach that link.".into(),
        }
    }
}

/// How the board's terminals open in Midna.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct TerminalsConfig {
    /// Job purposes (start, pr, plan, reopen) whose terminals open in Midna's Background group.
    pub background: Vec<String>,
}

/// The comment guard: agents may not add code comments, pragmas aside.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CommentsConfig {
    pub guard: bool,
    /// The languages it watches (rust, dart, python, typescript, javascript, swift, kotlin, go, …).
    pub languages: Vec<String>,
    /// A comment whose text starts with one of these is a pragma, and allowed.
    pub pragmas: Vec<String>,
}

impl Default for CommentsConfig {
    fn default() -> Self {
        let langs = ["rust", "dart", "python", "typescript", "javascript", "swift", "kotlin", "go", "java", "c", "cpp", "csharp", "ruby", "shell"];
        let pragmas = [
            "!/", "noqa", "type:", "pragma", "pylint:", "mypy:", "fmt:", "isort:", "-*-", "eslint-", "@ts-", "prettier-ignore", "istanbul ", "c8 ",
            "biome-ignore", "ignore:", "ignore_for_file:", "coverage:", "nolint", "NOLINT", "go:", "+build", "swiftlint:", "swift-format-ignore",
            "rubocop:", "frozen_string_literal:", "shellcheck ", "clang-format ", "@formatter:", "region", "endregion",
        ];
        CommentsConfig {
            guard: false,
            languages: langs.iter().map(|s| s.to_string()).collect(),
            pragmas: pragmas.iter().map(|s| s.to_string()).collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub owner: String,
    pub host: String,
    pub port: u16,
    pub data: PathBuf,
    pub page_url: String,
    pub midna: PathBuf,
    pub midna_bundle: String,
    pub open_midna: bool,
    pub notify: bool,
    /// The day weeks start on (the Days page, its `week` array and the hours menu): Sunday unless set.
    pub first_weekday: chrono::Weekday,
    pub claude: String,
    pub claude_projects: PathBuf,
    pub statusline_dir: PathBuf,
    pub runner: bool,
    pub intervals: Intervals,
    pub work_hours: WorkHoursDefault,
    pub questions: Questions,
    pub backlog: BacklogAi,
    pub pr: PrConfig,
    pub jira: JiraConfig,
    pub handoff: HandoffConfig,
    pub alerts: AlertsConfig,
    pub limits: LimitsConfig,
    pub attachments: AttachmentsConfig,
    pub terminals: TerminalsConfig,
    pub comments: CommentsConfig,
    pub devices: crate::devices::DevicesConfig,
    pub bits: crate::bits::BitsConfig,
    /// Accounts in memory instead of the Keychain, `gh` and git (tests, the sample board).
    pub accounts_sandbox: bool,
    pub config_path: PathBuf,
}

pub fn default_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("TASKBOARD_CONFIG") {
        return expand_home(&p);
    }
    expand_home("~/.config/taskboard/config.toml")
}

pub fn default_data_dir() -> PathBuf {
    expand_home("~/.config/taskboard")
}

fn git_first_name() -> Option<String> {
    let out = std::process::Command::new("git").args(["config", "--global", "user.name"]).output().ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    name.split_whitespace().next().map(|s| s.to_string())
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

impl Config {
    pub fn load() -> anyhow_like::Result<Config> {
        let path = default_config_path();
        let file = if path.is_file() {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
            toml::from_str::<FileConfig>(&text).map_err(|e| format!("{} doesn't parse: {e}", path.display()))?
        } else {
            FileConfig::default()
        };
        Ok(Config::from_file(file, path))
    }

    pub fn from_file(f: FileConfig, config_path: PathBuf) -> Config {
        let port = env("TASKBOARD_PORT").and_then(|p| p.parse().ok()).or(f.port).unwrap_or(8792);
        let host = f.host.clone().unwrap_or_else(|| "127.0.0.1".into());
        let data = env("TASKBOARD_DATA").map(|d| expand_home(&d)).or(f.data.as_deref().map(expand_home)).unwrap_or_else(default_data_dir);
        let is_default_data = data == default_data_dir();
        let runner = match env("TASKBOARD_RUNNER") {
            Some(v) => matches!(v.as_str(), "1" | "true" | "on" | "yes"),
            None => f.runner.unwrap_or(is_default_data),
        };
        let mut jira = f.jira.clone();
        if let Some(e) = env("TASKBOARD_JIRA_EMAIL") {
            jira.email = e;
        }
        if let Some(t) = env("TASKBOARD_JIRA_TOKEN") {
            jira.token = t;
        }
        let owner = f.owner.clone().filter(|o| !o.trim().is_empty()).or_else(git_first_name).unwrap_or_else(|| "the owner".into());
        Config {
            owner,
            page_url: env("TASKBOARD_PAGE_URL")
                .or(f.page_url.clone())
                .unwrap_or_else(|| "taskboard://".into()),
            host,
            port,
            statusline_dir: env("TASKBOARD_CHAT_INFO_DIR")
                .map(|d| expand_home(&d))
                .or(f.statusline_dir.as_deref().map(expand_home))
                .unwrap_or_else(|| data.join("statusline")),
            data,
            midna: expand_home(&env("TASKBOARD_MIDNA").or(f.midna.clone()).unwrap_or_else(|| "~/.local/bin/midna".into())),
            midna_bundle: f.midna_bundle.clone().unwrap_or_else(|| "com.mrgnhnt.midna".into()),
            open_midna: f.open_midna.unwrap_or(true),
            notify: f.notify.unwrap_or(true),
            first_weekday: f.first_weekday.as_deref().and_then(parse_weekday).unwrap_or(chrono::Weekday::Sun),
            claude: env("TASKBOARD_CLAUDE").or(f.claude.clone()).unwrap_or_else(|| "claude".into()),
            claude_projects: expand_home(
                &env("TASKBOARD_CLAUDE_PROJECTS").or(f.claude_projects.clone()).unwrap_or_else(|| "~/.claude/projects".into()),
            ),
            runner,
            intervals: f.intervals,
            work_hours: f.work_hours,
            questions: f.questions,
            backlog: f.backlog,
            pr: f.pr,
            jira,
            handoff: f.handoff,
            alerts: f.alerts,
            limits: f.limits,
            attachments: f.attachments,
            terminals: f.terminals,
            comments: f.comments,
            devices: f.devices,
            bits: f.bits,
            accounts_sandbox: env("TASKBOARD_ACCOUNTS").as_deref() == Some("sandbox"),
            config_path,
        }
    }

    /// A config for tests: everything under `data`, no runner, no Midna, no notifications.
    pub fn for_tests(data: &Path) -> Config {
        let mut c = Config::from_file(FileConfig::default(), data.join("config.toml"));
        c.owner = "Sam".into();
        c.data = data.to_path_buf();
        c.statusline_dir = data.join("statusline");
        c.claude_projects = data.join("claude-projects");
        // Tests never run the real claude (QA comments, screening and wave plans would ask it).
        c.claude = data.join("no-claude").to_string_lossy().to_string();
        c.runner = false;
        c.open_midna = false;
        c.notify = false;
        c.midna = data.join("no-midna");
        c.questions.screen = false;
        c.backlog.ai = false;
        c.pr.watch = false;
        c.accounts_sandbox = true;
        c.page_url = "taskboard://".into();
        c
    }

    pub fn db_path(&self) -> PathBuf {
        self.data.join("tasks.db")
    }
    pub fn md_dir(&self) -> PathBuf {
        self.data.join("tasks")
    }
    pub fn spool_dir(&self) -> PathBuf {
        self.data.join("spool")
    }
    pub fn log_path(&self) -> PathBuf {
        self.data.join("server.log")
    }
    pub fn url(&self) -> String {
        env("TASKBOARD_URL").unwrap_or_else(|| format!("http://127.0.0.1:{}", self.port))
    }
    pub fn jira_on(&self) -> bool {
        !self.jira.site.trim().is_empty() && !self.jira.project.trim().is_empty()
    }
    /// Jira goes through headless Claude and the Atlassian connector instead of the REST API.
    pub fn jira_via_claude(&self) -> bool {
        self.jira.via.trim().eq_ignore_ascii_case("claude")
    }
    /// The owner's name with a possessive, for agent-facing text ("Sam's answers").
    pub fn owners(&self) -> String {
        if self.owner == "the owner" {
            "the owner's".into()
        } else {
            format!("{}'s", self.owner)
        }
    }
}

/// `sun`, `Monday`, `sat` … as a weekday (the first three letters count).
pub fn parse_weekday(s: &str) -> Option<chrono::Weekday> {
    use chrono::Weekday::*;
    let s: String = s.trim().to_lowercase().chars().take(3).collect();
    Some(match s.as_str() {
        "mon" => Mon,
        "tue" => Tue,
        "wed" => Wed,
        "thu" => Thu,
        "fri" => Fri,
        "sat" => Sat,
        "sun" => Sun,
        _ => return None,
    })
}

pub mod anyhow_like {
    pub type Result<T> = std::result::Result<T, String>;
}

pub const EXAMPLE: &str = include_str!("../../../config.example.toml");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_parses() {
        let f: FileConfig = toml::from_str(EXAMPLE).expect("config.example.toml parses");
        let c = Config::from_file(f, PathBuf::from("/tmp/x.toml"));
        assert!(!c.owner.is_empty());
        assert_eq!(c.intervals.runner, 5.0);
        assert_eq!(c.first_weekday, chrono::Weekday::Sun);
    }

    #[test]
    fn first_weekday_parses() {
        let f: FileConfig = toml::from_str("first_weekday = \"Monday\"").unwrap();
        assert_eq!(Config::from_file(f, PathBuf::from("/tmp/x.toml")).first_weekday, chrono::Weekday::Mon);
        let f: FileConfig = toml::from_str("first_weekday = \"someday\"").unwrap();
        assert_eq!(Config::from_file(f, PathBuf::from("/tmp/x.toml")).first_weekday, chrono::Weekday::Sun);
    }
}
