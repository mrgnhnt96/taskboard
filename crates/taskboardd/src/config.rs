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
            model: "claude-haiku-4-5".into(),
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
}

impl Default for PrConfig {
    fn default() -> Self {
        PrConfig { watch: true, wake: true, agents_merge: false, gh: "gh".into(), no_checks_after_mins: 15.0 }
    }
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
    /// The owner's name with a possessive, for agent-facing text ("Sam's answers").
    pub fn owners(&self) -> String {
        if self.owner == "the owner" {
            "the owner's".into()
        } else {
            format!("{}'s", self.owner)
        }
    }
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
    }
}
