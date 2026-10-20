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
    pub pr_body: PrBodyConfig,
    pub jira: JiraConfig,
    pub handoff: HandoffConfig,
    pub alerts: AlertsConfig,
    pub limits: LimitsConfig,
    pub attachments: AttachmentsConfig,
    pub terminals: TerminalsConfig,
    pub models: ModelsConfig,
    pub builds: crate::builds::BuildsConfig,
    pub worktrees: crate::slots::WorktreesConfig,
    pub comments: CommentsConfig,
    pub devices: crate::devices::DevicesConfig,
    pub app_origin: crate::apporigin::AppOriginConfig,
    pub bits: crate::bits::BitsConfig,
    pub releases: crate::releases::ReleasesConfig,
    pub planning: crate::planning::PlanningConfig,
    pub checkin: crate::checkin::CheckinConfig,
    pub spans: crate::spans::SpansConfig,
    pub review: crate::review::ReviewConfig,
    pub lens: crate::lens::SubmitConfig,
    pub reviewers: crate::reviewers::ReviewersConfig,
    pub feed: crate::feed::FeedConfig,
    /// `[hosts.<host>]`: each PR host's call budget (`prhost/limit.rs`).
    pub hosts: std::collections::BTreeMap<String, crate::prhost::limit::HostConfig>,
    pub stream: crate::stream::StreamConfig,
    pub pr_builds: crate::prbuilds::PrBuildsConfig,
    pub master: crate::breaks::MasterConfig,
    /// The owner's commit emails: their own pushes (`prbuilds.rs`) and commits on a red default branch.
    pub owner_emails: Vec<String>,
}

/// Alerts' desktop notifications.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AlertsConfig {
    /// The snooze buttons on each alert's notification, in minutes (at most four; empty for none).
    pub snooze_mins: Vec<i64>,
    /// Minutes new alerts gather before they go out together as one notification (`digest.rs`); urgent
    /// alerts still notify on their own. 0 notifies each alert on its own.
    pub digest_mins: i64,
    /// Asks Haiku (the check-in's model and budget) for a short title and fixes for alerts raised as
    /// free text (`suggest.rs`). Off, those alerts show their text with Dismiss.
    pub suggest: bool,
}

impl Default for AlertsConfig {
    fn default() -> Self {
        AlertsConfig { snooze_mins: vec![15, 30, 60], digest_mins: 5, suggest: true }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Intervals {
    pub runner: f64,
    pub spool: f64,
    pub prs: f64,
    /// The review sweep (`asks::sweep`: swaps, stand-ins, the board's own asks and their retries, bot
    /// runs), whatever the PR feed's state.
    pub reviews: f64,
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
            reviews: 60.0,
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
            screen: true,
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
    /// Wave planning, tidies, review fixes and retros.
    pub model: String,
    /// Grouping new issues, a classification job. Empty: `model`.
    pub group_model: String,
    pub budget_usd: String,
    pub timeout_secs: u64,
    /// How many terminals a goal made from the backlog runs at once.
    pub max_terminals: i64,
    /// Tidy the backlog on a timer: Claude closes open issues that repeat another or aren't real
    /// (`tidy.rs`). `tb backlog tidy` changes both on the board.
    pub tidy: bool,
    /// Hours between tidies (1 for hourly, 24 for daily).
    pub tidy_every_hours: f64,
}

impl BacklogAi {
    /// The model that groups new issues.
    pub fn group_model(&self) -> &str {
        if self.group_model.trim().is_empty() { &self.model } else { &self.group_model }
    }
}

impl Default for BacklogAi {
    fn default() -> Self {
        BacklogAi { ai: true, model: "sonnet".into(), group_model: "haiku".into(), budget_usd: "1.00".into(), timeout_secs: 240, max_terminals: 2, tidy: true, tidy_every_hours: 24.0 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PrConfig {
    /// Watch open GitHub PRs with the `gh` CLI.
    pub watch: bool,
    /// Bring a done task's conversation back when its PR needs work.
    pub wake: bool,
    /// Agents may merge a PR once it's approved and green. Unset: off, and `taskboardd import` turns
    /// the board-wide switch on; an explicit false is left alone.
    pub agents_merge: Option<bool>,
    /// A green PR waits for the owner's own look ("Waiting on your review") before it moves on. Off:
    /// the board marks it reviewed for them. Unset: on.
    pub owner_review: Option<bool>,
    /// When `tb pr status` and a failed check's wake tell the agent to rebase onto the base.
    pub rebase: RebasePolicy,
    pub gh: String,
    /// Minutes after the head was pushed with no checks before the checks count as passed.
    pub no_checks_after_mins: f64,
    /// Seconds after a stream review or comment hint before its PR is read once more: a review's
    /// changes request often follows its comments as its own event, or none. 0: no second read.
    pub comment_settle_secs: f64,
    /// Bitbucket Cloud's REST API (2.0).
    pub bitbucket_api: String,
    /// The environment variable holding an Azure DevOps personal access token, for reading the failed
    /// steps and tests of an Azure Pipelines check.
    pub azure_token_env: String,
    /// Approvals a PR needs before it's ready to merge, when its project doesn't say. 0: the host's own
    /// decision, or any approval.
    pub approvals: i64,
    /// Per-project PR rules, by the project's name (`[pr.projects.webapp]`).
    pub projects: BTreeMap<String, PrProject>,
    /// The lens review stage on new PRs (`[pr.lens]`, `lens_pr.rs`).
    pub lens: LensConfig,
}

impl Default for PrConfig {
    fn default() -> Self {
        PrConfig {
            watch: true,
            wake: true,
            agents_merge: None,
            owner_review: None,
            rebase: RebasePolicy::default(),
            gh: "gh".into(),
            no_checks_after_mins: 15.0,
            comment_settle_secs: 90.0,
            bitbucket_api: "https://api.bitbucket.org/2.0".into(),
            azure_token_env: "AZURE_DEVOPS_EXT_PAT".into(),
            approvals: DEFAULT_APPROVALS,
            projects: BTreeMap::new(),
            lens: LensConfig::default(),
        }
    }
}

impl PrConfig {
    /// The PR rules of a task's project (the defaults when it has none).
    pub fn project(&self, name: Option<&str>) -> PrProject {
        name.and_then(|n| self.projects.get(n)).cloned().unwrap_or_default()
    }

    /// `agents_merge`, off when unset.
    pub fn agents_merge_on(&self) -> bool {
        self.agents_merge.unwrap_or(false)
    }

    /// `owner_review`, on when unset.
    pub fn owner_review_on(&self) -> bool {
        self.owner_review.unwrap_or(true)
    }

    /// The rebase policy for a project: its own `rebase`, else `pr.rebase`.
    pub fn rebase_for(&self, name: Option<&str>) -> RebasePolicy {
        self.project(name).rebase.unwrap_or(self.rebase)
    }
}

/// When a PR's branch should be rebased onto its base. A rebase is a push, and a push starts the
/// builds again, so the default only asks for one when the PR can't merge as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RebasePolicy {
    /// Whenever the base has moved since the push.
    Moved,
    /// Only when the PR conflicts with its base (as the host reports it).
    #[default]
    Conflicts,
    /// Never.
    Never,
}

/// One project's PR rules. Every key is optional; unset keys keep the board-wide behaviour. `tb project
/// set` changes `approvals`, `expected` and `expected_wait_mins` on the board, over these
/// (`projects::pr_rules`).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct PrProject {
    /// Approvals a PR needs before it's ready to merge. Unset: `pr.approvals`; 0: the host's own
    /// decision, or any approval.
    pub approvals: Option<i64>,
    /// Check names that must post on every push before the checks count as finished. Unset: the
    /// `no_checks_after_mins` grace; an empty list: no wait at all.
    pub expected: Option<Vec<String>>,
    /// How long to wait for the expected checks to post, in minutes.
    pub expected_wait_mins: Option<f64>,
    /// A command that says, per push, which checks that push starts (`prflow::note_expected`): run with
    /// `/bin/sh -c` once per new head with TB_PR_URL, TB_PR_REPO, TB_PR_NUM, TB_HEAD, TB_BASE, TB_BRANCH
    /// and TB_WORKTREE (when the task has one). A JSON list takes the place of `expected` for that push
    /// (`[]`: none); `null`, nothing or a non-zero exit: can't tell (as if `expected` were unset).
    pub expected_cmd: Option<String>,
    /// A command that prints a failed check's steps and tests (for CI the board can't read itself).
    /// It gets TB_PR_URL, TB_PR_REPO, TB_PR_NUM, TB_HEAD, TB_CHECK and TB_CHECK_URL, and prints JSON
    /// (`{"steps": [...], "tests": [...]}`) or one step per line (`test: <name>` for a test).
    pub failures_cmd: Option<String>,
    /// How `tb pr merge` merges: merge, squash or rebase (Bitbucket: merge_commit, squash,
    /// fast_forward). Unset: the repository's default.
    pub merge_strategy: Option<String>,
    /// Swap reviewers who take too long. Unset: `[reviewers] swap`.
    pub swap: Option<bool>,
    /// The `ask` stage after the owner's review. Unset: `[reviewers] ask_stage`.
    pub ask_stage: Option<bool>,
    /// The Review step: off skips it, so a PR goes to merge without reviewers. Unset: on.
    pub review: Option<bool>,
    /// Agents may merge this project's PRs once they're approved and green. Unset: `pr.agents_merge`.
    pub agents_merge: Option<bool>,
    /// A green PR waits for the owner's review. Off: the board marks it reviewed. Unset: `pr.owner_review`.
    pub owner_review: Option<bool>,
    /// The lens review stage on this project's new PRs. Unset: `pr.lens.on`.
    pub lens: Option<bool>,
    /// When to rebase its PRs onto their base. Unset: `pr.rebase`.
    pub rebase: Option<RebasePolicy>,
}

/// `[pr.lens]`: on a new PR, run lens's PR pass (`lens review --pr <n>`, the binary `[lens] bin` names)
/// and let it post its verified findings on the PR. It shows as the `lens` stage and never holds a merge.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LensConfig {
    /// Run it on every project's new PRs (a project's `lens` says otherwise). Off by default.
    pub on: bool,
    /// How long one run may take, in minutes. A run past it is stopped, and the stage moves on.
    pub timeout_mins: f64,
}

impl Default for LensConfig {
    fn default() -> Self {
        LensConfig { on: false, timeout_mins: 30.0 }
    }
}

/// Approvals a PR needs when neither the project nor `pr.approvals` says (the old board's 2).
pub const DEFAULT_APPROVALS: i64 = 2;

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

/// `tb submit --pr-body FILE`: what the PR description must look like, and how the board opens the PR.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PrBodyConfig {
    /// `## <name>` sections the description needs, in this order.
    pub sections: Vec<String>,
    /// Sections whose content is a bullet list.
    pub bullets: Vec<String>,
    /// Longest paragraph, in characters (0 for no limit).
    pub max_paragraph: usize,
    /// Refuse the board's own words: task, goal and backlog refs (T12, G3, B7) and "task board".
    pub no_board_refs: bool,
    /// More things the description mustn't say, as regexes (case-insensitive): a review tool's name, …
    pub forbid: Vec<String>,
    /// The branch must sit on top of the remote base (rebased), with no merge commits.
    pub rebased: bool,
    /// Refuse a PR (opened by the board or linked with `tb submit --pr`) for a task with no Jira ticket. Unset:
    /// on while Jira is set up (`Config::ticket_required`).
    pub require_ticket: Option<bool>,
    /// Start the PR title with the ticket key ("ABC-12 Add login").
    pub title_prefix: bool,
    /// Add a `## Context` section: the ticket, the PR it stacks on, the task's evidence.
    pub context: bool,
    /// The remote the branch is pushed to and the base is read from.
    pub remote: String,
}

impl Default for PrBodyConfig {
    fn default() -> Self {
        PrBodyConfig {
            sections: vec!["Summary".into(), "Changes".into(), "Testing".into()],
            bullets: vec!["Changes".into(), "Testing".into()],
            max_paragraph: 400,
            no_board_refs: true,
            forbid: vec![],
            rebased: true,
            require_ticket: None,
            title_prefix: true,
            context: true,
            remote: "origin".into(),
        }
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
    /// The site's Atlassian cloud id, for the connector's tools (`cloudId`). Empty: the site's host
    /// name, which the tools take as one.
    pub cloud_id: String,
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
    /// The status a ticket moves to when its task finishes without a PR (`tb submit --no-pr`), with the
    /// reason as a comment. Empty: it goes to `done` like any task without a PR.
    pub canceled: String,
    pub products: BTreeMap<String, JiraProduct>,
    /// How the board talks to Jira: "rest" (the API token) or "claude" (a headless `claude -p` with
    /// the Atlassian connector's tools, for a board with no token).
    pub via: String,
    /// The tools `claude -p` may use for Jira ("claude" via and the desk). Empty: each op gets only the
    /// claude.ai Atlassian connector's tools it needs, by exact name (`jira_claude::default_tools`).
    /// Set, every op gets it as it is; a server's name (`mcp__claude_ai_Atlassian_MCP`) allows all its tools.
    pub claude_tools: Vec<String>,
    /// The model for Jira through Claude: the ops are mechanical, so a small one.
    pub claude_model: String,
    /// Minutes between QA's checks of Jira for new comments. 0: every 5 minutes through the REST API,
    /// every 30 through Claude (each check is one `claude -p`).
    pub qa_poll_mins: u64,
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
    /// A markdown file every new task and bug ticket's description is made from: `{description}`,
    /// `{summary}`, `{title}`, `{detail}`, `{task}`, `{goal}`, `{outcome}`, `{epic}` and `{project}` are
    /// filled in (`jira::render_template`). Empty: the description is the ticket text as it is.
    pub ticket_template: String,
    /// A markdown file of how this board's tickets are written. Agents read it in `tb jira` and write
    /// each new ticket's summary and text to it (`tb task new --jira new --ticket-text …`).
    pub ticket_guide: String,
    /// A task that ships a PR on a ticket finishes with a note for testers (`tb submit --merged-note`); at
    /// merge the board posts it on the ticket before it moves the ticket to `merged`.
    pub merged_note: bool,
    /// A markdown file of the parts that note has. Agents read it in `tb jira`.
    pub merged_note_guide: String,
}

impl Default for JiraConfig {
    fn default() -> Self {
        JiraConfig {
            site: String::new(),
            cloud_id: String::new(),
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
            canceled: String::new(),
            products: BTreeMap::new(),
            via: "rest".into(),
            claude_tools: vec![],
            claude_model: "haiku".into(),
            qa_poll_mins: 0,
            claude_budget_usd: "0.50".into(),
            claude_timeout_secs: 180,
            auto_ticket: false,
            desk: false,
            desk_dir: String::new(),
            default_product: String::new(),
            ticket_template: String::new(),
            ticket_guide: String::new(),
            merged_note: false,
            merged_note_guide: String::new(),
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
    /// A task terminal that has compacted this many times carries on in a fresh terminal from its handoff.
    pub max_compactions: i64,
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
            max_compactions: 3,
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
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TerminalsConfig {
    /// Job purposes (start, pr, plan, reopen, jira_desk) whose terminals open in Midna's Background group.
    pub background: Vec<String>,
    /// A queued task opens its terminal only while fewer than this many of its project's terminals are busy
    /// (`tb project set <name> --max-terminals` changes it for one project).
    pub project_max: i64,
    /// A task opens its terminal only while fewer than this many task terminals run across every project
    /// (`tb terminals --max` changes it on the board).
    pub global_max: i64,
}

pub const PROJECT_MAX_TERMINALS: i64 = 5;
pub const GLOBAL_MAX_TERMINALS: i64 = 8;

impl Default for TerminalsConfig {
    fn default() -> Self {
        TerminalsConfig { background: vec![crate::jira_desk::PURPOSE.into()], project_max: PROJECT_MAX_TERMINALS, global_max: GLOBAL_MAX_TERMINALS }
    }
}

/// The model each task's terminal runs (`--model`): a task picks `small` or `large` at planning
/// (`tb task new --model`), or names a model outright, and tasks without one get `default`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ModelsConfig {
    /// Off, the board passes no `--model` and Claude uses its own default.
    pub enabled: bool,
    pub small: String,
    pub large: String,
    /// `small`, `large`, or a model name.
    pub default: String,
    /// The model of PR visits that don't change code (merging, asking for reviews): `small`, `large` or a
    /// model name. Empty runs them on the task's own model.
    pub pr_visit: String,
    /// Those visits start a fresh conversation with a short prompt instead of resuming the task's.
    pub fresh_pr_visits: bool,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        ModelsConfig {
            enabled: true,
            small: "sonnet".into(),
            large: "opus".into(),
            default: "large".into(),
            pr_visit: "sonnet".into(),
            fresh_pr_visits: true,
        }
    }
}

impl ModelsConfig {
    /// The model to start a task's terminal with, from the task's own `model` (null: the default).
    pub fn resolve(&self, task: Option<&str>) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let pick = task.map(str::trim).filter(|m| !m.is_empty()).unwrap_or(self.default.trim());
        let model = match pick {
            "small" => self.small.trim(),
            "large" => self.large.trim(),
            other => other,
        };
        (!model.is_empty()).then(|| model.to_string())
    }

    /// The model of a PR visit that doesn't change code; the task's own when `pr_visit` is empty.
    pub fn resolve_pr_visit(&self, task: Option<&str>) -> Option<String> {
        match self.pr_visit.trim() {
            "" => self.resolve(task),
            m => self.resolve(Some(m)),
        }
    }
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
    pub pr_body: PrBodyConfig,
    pub jira: JiraConfig,
    pub handoff: HandoffConfig,
    pub alerts: AlertsConfig,
    pub limits: LimitsConfig,
    pub attachments: AttachmentsConfig,
    pub terminals: TerminalsConfig,
    pub models: ModelsConfig,
    pub builds: crate::builds::BuildsConfig,
    pub worktrees: crate::slots::WorktreesConfig,
    pub comments: CommentsConfig,
    pub devices: crate::devices::DevicesConfig,
    pub app_origin: crate::apporigin::AppOriginConfig,
    pub bits: crate::bits::BitsConfig,
    pub releases: crate::releases::ReleasesConfig,
    pub planning: crate::planning::PlanningConfig,
    pub checkin: crate::checkin::CheckinConfig,
    pub spans: crate::spans::SpansConfig,
    pub review: crate::review::ReviewConfig,
    pub lens: crate::lens::SubmitConfig,
    pub reviewers: crate::reviewers::ReviewersConfig,
    pub feed: crate::feed::FeedConfig,
    pub hosts: std::collections::BTreeMap<String, crate::prhost::limit::HostConfig>,
    pub stream: crate::stream::StreamConfig,
    pub pr_builds: crate::prbuilds::PrBuildsConfig,
    pub master: crate::breaks::MasterConfig,
    pub owner_emails: Vec<String>,
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

/// The `[terminals]` line `init` wrote before the Jira desk moved to the Background group by default:
/// a live empty list that would keep it out. Exactly this line (the owner's own `background = []`
/// wouldn't carry init's example) becomes `INIT_BACKGROUND`.
const OLD_INIT_BACKGROUND: &str = r#"background = []                      # example: ["plan"]"#;
const INIT_BACKGROUND: &str = r#"background = ["jira_desk"]           # example: ["jira_desk", "plan"]"#;

/// The `[jira] claude_tools` values `init` wrote before each Jira op got its own exact tools: whole
/// servers, which let a status read or a comment job create, edit or delete. Only the line `init`
/// wrote (its comment starts with `OLD_INIT_TOOLS_NOTE`) becomes `INIT_TOOLS`; the owner's own list stays.
const OLD_INIT_TOOLS: [&str; 2] = [r#"["mcp__claude_ai_Atlassian_MCP", "mcp__atlassian"]"#, r#"["mcp__claude_ai_Atlassian", "mcp__atlassian"]"#];
const OLD_INIT_TOOLS_NOTE: &str = r#"# the connector's tools, for via = "claude" and the desk"#;
/// The comment line `init` wrote under the old `claude_tools`; it goes with it.
const OLD_INIT_TOOLS_MORE: &str = "# the claude.ai Atlassian connector, and a local Atlassian MCP server";
const INIT_TOOLS: &str = "claude_tools = []                    # empty: each Jira op gets only the connector's tools it needs";

fn old_init_tools(line: &str) -> bool {
    let Some(rest) = line.trim().strip_prefix("claude_tools") else { return false };
    let Some(rest) = rest.trim_start().strip_prefix('=') else { return false };
    let Some((value, note)) = rest.split_once('#') else { return false };
    OLD_INIT_TOOLS.contains(&value.trim()) && format!("#{note}").starts_with(OLD_INIT_TOOLS_NOTE)
}

/// An old `init`'s config.toml brought up to date, or None when there's nothing to change.
pub fn migrate_text(text: &str) -> Option<String> {
    if !text.lines().any(|l| l.trim_end() == OLD_INIT_BACKGROUND || old_init_tools(l)) {
        return None;
    }
    let mut lines: Vec<&str> = Vec::new();
    let mut after_tools = false;
    for l in text.lines() {
        if after_tools && l.trim() == OLD_INIT_TOOLS_MORE {
            after_tools = false;
            continue;
        }
        after_tools = old_init_tools(l);
        lines.push(if l.trim_end() == OLD_INIT_BACKGROUND {
            INIT_BACKGROUND
        } else if after_tools {
            INIT_TOOLS
        } else {
            l
        });
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

impl Config {
    pub fn load() -> anyhow_like::Result<Config> {
        let path = default_config_path();
        let file = if path.is_file() {
            let mut text = std::fs::read_to_string(&path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
            if let Some(new) = migrate_text(&text) {
                // Best effort: the file keeps working the old way if it can't be written.
                let tmp = path.with_extension("toml.taskboard.tmp");
                if std::fs::write(&tmp, &new).and_then(|_| std::fs::rename(&tmp, &path)).is_err() {
                    let _ = std::fs::remove_file(&tmp);
                }
                text = new;
            }
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
            pr_body: f.pr_body,
            jira,
            handoff: f.handoff,
            alerts: f.alerts,
            limits: f.limits,
            attachments: f.attachments,
            terminals: f.terminals,
            models: f.models,
            builds: f.builds,
            worktrees: f.worktrees,
            comments: f.comments,
            devices: f.devices,
            app_origin: f.app_origin,
            bits: f.bits,
            releases: f.releases,
            planning: f.planning,
            checkin: f.checkin,
            spans: f.spans,
            review: f.review,
            lens: f.lens,
            reviewers: f.reviewers,
            feed: f.feed,
            hosts: f.hosts,
            stream: f.stream,
            pr_builds: f.pr_builds,
            master: f.master,
            owner_emails: f.owner_emails,
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
        c.lens.bin = data.join("no-lens").to_string_lossy().to_string();
        c.runner = false;
        // No `--model` in started terminals' flags unless a test turns it on.
        c.models.enabled = false;
        c.models.fresh_pr_visits = false;
        c.open_midna = false;
        c.notify = false;
        c.midna = data.join("no-midna");
        c.questions.screen = false;
        c.backlog.ai = false;
        c.pr.watch = false;
        // No host is read for the owner's open PRs unless a test asks.
        c.pr_builds.owner_prs_secs = 0.0;
        // The host's verdict (or any approval) is enough, unless a test asks for a count.
        c.pr.approvals = 0;
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
    /// Where the JSON API answers.
    pub fn api_url(&self) -> String {
        format!("{}/tasks/api", self.url().trim_end_matches('/'))
    }
    pub fn jira_on(&self) -> bool {
        !self.jira.site.trim().is_empty() && !self.jira.project.trim().is_empty()
    }
    /// Every PR needs a Jira ticket: `[pr_body] require_ticket`, else whether Jira is set up.
    pub fn ticket_required(&self) -> bool {
        self.pr_body.require_ticket.unwrap_or_else(|| self.jira_on())
    }
    /// Jira goes through headless Claude and the Atlassian connector instead of the REST API.
    pub fn jira_via_claude(&self) -> bool {
        self.jira.via.trim().eq_ignore_ascii_case("claude")
    }
    /// How often QA checks Jira for new comments, in seconds (`[jira] qa_poll_mins`).
    pub fn qa_poll_secs(&self) -> f64 {
        let mins = match self.jira.qa_poll_mins {
            0 if self.jira_via_claude() => 30,
            0 => 5,
            m => m,
        };
        mins as f64 * 60.0
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
        assert_eq!(c.pr.approvals, DEFAULT_APPROVALS);
        assert_eq!(Config::from_file(FileConfig::default(), PathBuf::from("/tmp/x.toml")).pr.approvals, 2, "a PR needs 2 approvals unless told otherwise");
    }

    #[test]
    fn task_models_resolve_through_the_tiers() {
        let m = ModelsConfig::default();
        assert_eq!(m.resolve(None).as_deref(), Some("opus"), "no model: the default, large");
        assert_eq!(m.resolve(Some("small")).as_deref(), Some("sonnet"));
        assert_eq!(m.resolve(Some("large")).as_deref(), Some("opus"));
        assert_eq!(m.resolve(Some("claude-haiku-5-5")).as_deref(), Some("claude-haiku-5-5"));
        assert_eq!(m.resolve(Some(" ")).as_deref(), Some("opus"));
        let small = ModelsConfig { default: "small".into(), ..ModelsConfig::default() };
        assert_eq!(small.resolve(None).as_deref(), Some("sonnet"));
        let off = ModelsConfig { enabled: false, ..ModelsConfig::default() };
        assert_eq!(off.resolve(Some("small")), None, "off: Claude's own default");
        let c = Config::from_file(toml::from_str("[models]\nsmall = \"haiku\"").unwrap(), PathBuf::from("/tmp/x.toml"));
        assert_eq!((c.models.small.as_str(), c.models.large.as_str(), c.models.enabled), ("haiku", "opus", true));
        assert_eq!(m.resolve_pr_visit(Some("large")).as_deref(), Some("sonnet"), "PR visits: sonnet by default");
        let own = ModelsConfig { pr_visit: "".into(), ..ModelsConfig::default() };
        assert_eq!(own.resolve_pr_visit(Some("small")).as_deref(), Some("sonnet"), "empty: the task's own");
        let tier = ModelsConfig { pr_visit: "large".into(), ..ModelsConfig::default() };
        assert_eq!(tier.resolve_pr_visit(None).as_deref(), Some("opus"));
        assert_eq!(off.resolve_pr_visit(None), None);
    }

    #[test]
    fn api_url_has_its_slash() {
        let c = Config::for_tests(Path::new("/tmp/x"));
        assert_eq!(c.api_url(), format!("http://127.0.0.1:{}/tasks/api", c.port));
    }

    #[test]
    fn the_jira_desk_opens_in_the_background_by_default() {
        let f: FileConfig = toml::from_str("").unwrap();
        assert_eq!(Config::from_file(f, PathBuf::from("/tmp/x.toml")).terminals.background, vec!["jira_desk"]);
        let f: FileConfig = toml::from_str("[terminals]\nbackground = []").unwrap();
        assert!(Config::from_file(f, PathBuf::from("/tmp/x.toml")).terminals.background.is_empty());
    }

    #[test]
    fn an_old_init_s_empty_background_gets_the_jira_desk() {
        let old = "[terminals]\nbackground = []                      # example: [\"plan\"]\n\n[limits]\n";
        let new = migrate_text(old).unwrap();
        assert_eq!(new, format!("[terminals]\n{INIT_BACKGROUND}\n\n[limits]\n"));
        let f: FileConfig = toml::from_str(&new).unwrap();
        assert_eq!(Config::from_file(f, PathBuf::from("/tmp/x.toml")).terminals.background, vec!["jira_desk"]);
        assert_eq!(migrate_text("[terminals]\nbackground = []\n"), None, "the owner's own empty list stays");
        assert_eq!(migrate_text(EXAMPLE), None, "today's init has nothing to change");
    }

    #[test]
    fn an_old_init_s_whole_connector_gives_way_to_each_op_s_tools() {
        let beta16 = concat!(
            "[jira]\nvia = \"claude\"\n",
            "claude_tools = [\"mcp__claude_ai_Atlassian_MCP\", \"mcp__atlassian\"]   # the connector's tools, for via = \"claude\" and the desk:\n",
            "                                     # the claude.ai Atlassian connector, and a local Atlassian MCP server\n",
            "claude_model = \"haiku\"\n"
        );
        let new = migrate_text(beta16).unwrap();
        assert_eq!(new, format!("[jira]\nvia = \"claude\"\n{INIT_TOOLS}\nclaude_model = \"haiku\"\n"));
        let f: FileConfig = toml::from_str(&new).unwrap();
        assert!(Config::from_file(f, PathBuf::from("/tmp/x.toml")).jira.claude_tools.is_empty());

        let older = "[jira]\nclaude_tools = [\"mcp__claude_ai_Atlassian\", \"mcp__atlassian\"]   # the connector's tools, for via = \"claude\" and the desk\nclaude_model = \"sonnet\"\n";
        assert_eq!(migrate_text(older).unwrap(), format!("[jira]\n{INIT_TOOLS}\nclaude_model = \"sonnet\"\n"));

        for own in [
            "[jira]\nclaude_tools = [\"mcp__claude_ai_Atlassian_MCP\", \"mcp__atlassian\"]\n",
            "[jira]\nclaude_tools = [\"mcp__atlassian\"]   # the connector's tools, for via = \"claude\" and the desk\n",
            "[jira]\nclaude_tools = [\"mcp__claude_ai_Atlassian_MCP\", \"mcp__atlassian\"]   # mine\n",
        ] {
            assert_eq!(migrate_text(own), None, "the owner's own list stays: {own}");
        }
    }

    #[test]
    fn first_weekday_parses() {
        let f: FileConfig = toml::from_str("first_weekday = \"Monday\"").unwrap();
        assert_eq!(Config::from_file(f, PathBuf::from("/tmp/x.toml")).first_weekday, chrono::Weekday::Mon);
        let f: FileConfig = toml::from_str("first_weekday = \"someday\"").unwrap();
        assert_eq!(Config::from_file(f, PathBuf::from("/tmp/x.toml")).first_weekday, chrono::Weekday::Sun);
    }
}
