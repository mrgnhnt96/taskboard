//! `tb`: how a Claude session (or a person) reports to the task board.

use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};

use crate::client::{self, CallError};
use crate::hook;
use taskboardd::accounts::{self, Provider};
use taskboardd::config::Config;
use taskboardd::hooks;
use taskboardd::steps::{self, Step};
use std::collections::BTreeMap;

const TB_TIMEOUT: f64 = 5.0;
const QUESTION_TIMEOUT: f64 = 100.0;
const SAVED: &str = "Saved; the board will pick it up when it's back.";
const NO_TASK: &str = "No task on this terminal, so nothing was logged. Run tb take T<n> first, or pass --task T<n>.";

#[derive(Parser)]
#[command(name = "tb", about = "Report to the local task board", version)]
pub struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone, Default)]
struct TaskArg {
    /// The task (T12, t12 or 12); defaults to this terminal's task.
    #[arg(long)]
    task: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Log a note on the task (or, with --goal, on its goal)
    Note {
        text: String,
        #[arg(long)]
        goal: bool,
        #[arg(long, default_value = "finding", value_parser = ["finding", "decision", "reference"])]
        kind: String,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Save a checkpoint a new terminal could carry on from
    Checkpoint {
        #[arg(long)]
        done: Vec<String>,
        #[arg(long)]
        next: Vec<String>,
        #[arg(long = "decision")]
        decisions: Vec<String>,
        #[arg(long = "file")]
        files: Vec<String>,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Report a problem outside the task to the backlog
    Found {
        title: String,
        #[arg(long, default_value = "bug", value_parser = ["bug", "gap", "follow", "clean"])]
        kind: String,
        #[arg(long)]
        detail: Option<String>,
        #[arg(long)]
        output: Option<String>,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Ask the owner a question (the board may answer it from the owner's rules)
    Question {
        text: String,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Finish the task
    Done {
        summary: String,
        /// The PR this task opened (or put its link in the summary)
        #[arg(long)]
        pr: Option<String>,
        /// How long the work would have taken a developer by hand: 3h, 90m, 1h30m, 2d
        #[arg(long)]
        human: Option<String>,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Record one of the owner's steps as done (from config.toml's [[steps]])
    Step {
        #[command(subcommand)]
        action: StepCmd,
    },
    /// The task's steps (config.toml's [[steps]]) and which are done
    Steps {
        #[command(flatten)]
        t: TaskArg,
    },
    /// Give up on the task
    Fail {
        reason: String,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Take a task in this terminal (prints its handoff)
    Take { task: String },
    /// This terminal's task
    Status {
        #[command(flatten)]
        t: TaskArg,
    },
    /// Wait for another task's work before carrying on (or `none`)
    WaitFor {
        tasks: Vec<String>,
        #[arg(long)]
        why: Option<String>,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Propose planned tasks for a goal: --task "title::what to do"
    Propose {
        goal: String,
        /// A planned task: "title::detail", "title::detail::<wave>" or "title::detail::<wave or nothing>::<T14 #1, the tasks it waits for>"
        #[arg(long = "task", value_name = "TITLE::DETAIL[::WAVE][::WAITS]")]
        tasks: Vec<String>,
    },
    /// Named locks, who holds each, and tasks that run alone
    Locks,
    /// List goals
    Goals {
        #[arg(long)]
        project: Option<String>,
    },
    /// Make, show, change or delete a goal
    Goal {
        #[command(subcommand)]
        action: GoalCmd,
    },
    /// QA testers' Jira comments on board tickets: list, waiting, show Q3, or on the owner's word task Q3 / ignore Q3
    Qa {
        /// list (default), waiting, Q3, task or ignore
        what: Option<String>,
        /// Q3 (with task or ignore)
        r#ref: Option<String>,
        /// What the owner said to do, in their words; it goes in the task's brief
        #[arg(long)]
        note: Option<String>,
        /// The work changes code (the default)
        #[arg(long, conflicts_with = "no_pr")]
        pr: bool,
        /// The work needs no PR
        #[arg(long = "no-pr")]
        no_pr: bool,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// Jira: how it's set up and its jobs, one job (J12), or the Jira desk's report on one:
    /// tb jira J12 ok key=PROJ-1 status="To Do" [found=yes] [product=web] / tb jira J12 fail "<why>"
    Jira {
        /// J12
        job: Option<String>,
        /// ok or fail
        #[arg(value_parser = ["ok", "fail"])]
        result: Option<String>,
        /// key=… status=… found=yes product=…, or (with fail) why
        rest: Vec<String>,
    },
    /// Add, change or delete a task
    Task {
        #[command(subcommand)]
        action: TaskCmd,
    },
    /// Add, change, promote or close backlog issues
    Backlog {
        #[command(subcommand)]
        action: BacklogCmd,
    },
    /// Raise an alert for the owner (the banner and a notification), or clear one you raised
    Alert {
        #[command(subcommand)]
        action: AlertCmd,
    },
    /// Show a project or change its PR flow
    Project {
        #[command(subcommand)]
        action: ProjectCmd,
    },
    /// Attach a link or file path (design, proposal, doc, evidence, results) to a task or goal
    Attach {
        url: String,
        #[arg(long, default_value = "other", value_parser = ["design", "proposal", "doc", "evidence", "results", "other"])]
        kind: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        goal: Option<String>,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Remove an attachment from the task (or, with --goal, the goal), by its link, path or title
    Unattach {
        url: String,
        #[arg(long)]
        goal: Option<String>,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Show or change the work hours
    Hours {
        #[arg(long)]
        on: bool,
        #[arg(long)]
        off: bool,
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        end: Option<String>,
        #[arg(long)]
        days: Option<String>,
        #[arg(long = "today-until")]
        today_until: Option<String>,
        /// How often unanswered alerts repeat, in minutes (0 for never)
        #[arg(long = "alert-every", value_name = "MIN")]
        alert_every: Option<i64>,
    },
    /// Show or change how Midna keeps the Mac awake for agents in the work hours (`tb hours` sets when)
    KeepAwake {
        #[arg(long)]
        on: bool,
        #[arg(long)]
        off: bool,
        /// with-work (only while agents have work) or always (the whole window)
        #[arg(long, value_parser = ["with-work", "always"])]
        mode: Option<String>,
        /// One day's own keep-awake hours inside the work hours: fri=9am-3pm, sat=off, fri=default (repeatable)
        #[arg(long = "day")]
        day: Vec<String>,
        /// Let the Mac sleep on battery below this percent (0 = no limit)
        #[arg(long = "min-battery")]
        min_battery: Option<i64>,
        /// Minutes to stay awake after the work runs out
        #[arg(long)]
        linger: Option<i64>,
        /// off: let the Mac sleep for the rest of today; clear: back to the work hours
        #[arg(long, value_parser = ["off", "clear"])]
        today: Option<String>,
    },
    /// A done task's pull request
    Pr {
        #[command(subcommand)]
        action: PrCmd,
    },
    /// Which accounts are connected (GitHub, Bitbucket, Slack); connect them in Taskboard ▸ Settings
    Accounts,
    /// Print an account's token for a script: github, bitbucket or slack
    Token {
        #[arg(value_parser = ["github", "bitbucket", "slack"])]
        provider: String,
        /// Print who the token belongs to instead (Bitbucket: the email for basic auth)
        #[arg(long)]
        user: bool,
    },
    /// Call an account's REST API with its token: `tb api bitbucket repositories/acme/web/pullrequests/9`
    Api {
        #[arg(value_parser = ["github", "bitbucket", "slack"])]
        provider: String,
        /// The path under https://api.github.com/, https://api.bitbucket.org/2.0/ or https://slack.com/api/ (or a full URL)
        path: String,
        /// GET, POST, PUT or DELETE (POST when there's --data)
        #[arg(short = 'X', long)]
        method: Option<String>,
        /// A JSON body
        #[arg(short, long)]
        data: Option<String>,
    },
    /// The owner's hooks: commands run at each step of a task's flow (hooks.json)
    Hooks {
        #[command(subcommand)]
        action: Option<HooksCmd>,
    },
    /// Tell the board where this tb is
    Hello,
    /// Claude Code hook (used by the plugin)
    Hook { event: Option<String> },
    /// git credential helper for the board's sessions (set up by the plugin's SessionStart hook)
    #[command(hide = true)]
    GitCredential {
        #[arg(value_parser = ["get", "store", "erase"])]
        op: String,
    },
    /// Claude Code status line: saves the rate limits for the board and prints a short line
    Statusline {
        /// Print the input unchanged (to chain into your own status line command)
        #[arg(long)]
        pass: bool,
    },
}

#[derive(Subcommand)]
enum HooksCmd {
    /// The flow's events and the hooks on each (the default)
    List,
    /// Run an event's hooks now against a task, with `"test": true` in their input
    Test {
        event: String,
        /// The task (defaults to the newest one)
        #[arg(long)]
        task: Option<String>,
    },
    /// The latest hook runs
    Log {
        #[arg(short, default_value_t = 20)]
        n: usize,
    },
}

#[derive(Subcommand)]
enum StepCmd {
    /// The step is done (runs its check first): tb step done "Author-side review" --note "fixed two findings"
    Done {
        name: String,
        /// What came of it
        #[arg(long)]
        note: Option<String>,
        /// The owner skips the step (not from the task's own terminal)
        #[arg(long)]
        skip: bool,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Run a script step (and its check); it counts once it exits 0
    Run {
        name: String,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Ask the owner to do their step; the task waits for them, so end your turn after
    Ask {
        name: String,
        #[command(flatten)]
        t: TaskArg,
    },
    /// A step can't pass: the task waits for the owner's answer, so end your turn after
    Fail {
        name: String,
        #[arg(long)]
        why: String,
        #[command(flatten)]
        t: TaskArg,
    },
}

#[derive(Subcommand)]
enum GoalCmd {
    /// Make a goal, optionally with planned tasks
    New {
        name: String,
        #[arg(long)]
        outcome: Option<String>,
        #[arg(long)]
        tldr: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        product: Option<String>,
        /// A planned task: "title::detail", "title::detail::<wave>" or "title::detail::<wave or nothing>::<T14 #1, the tasks it waits for>"
        #[arg(long = "task", value_name = "TITLE::DETAIL[::WAVE][::WAITS]")]
        tasks: Vec<String>,
    },
    /// The goal, its tasks and why each queued one waits
    Show { goal: String },
    /// Change a goal
    Set {
        goal: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        outcome: Option<String>,
        #[arg(long)]
        tldr: Option<String>,
        #[arg(long, value_parser = ["on", "off"])]
        paused: Option<String>,
        #[arg(long, value_parser = ["on", "off"])]
        in_order: Option<String>,
        #[arg(long)]
        max_terminals: Option<i64>,
        #[arg(long)]
        epic: Option<String>,
        #[arg(long)]
        product: Option<String>,
        /// Start each task in its own git worktree, detached at this branch (like origin/main); off for the shared folder
        #[arg(long, value_name = "BASE|off")]
        worktrees: Option<String>,
        /// Queue its planned tasks to run (and unpause it), when the owner says so
        #[arg(long)]
        run: bool,
        /// Hold its queued tasks back until it's prioritized or run again
        #[arg(long, conflicts_with = "prioritize")]
        deprioritize: bool,
        /// Back to normal priority
        #[arg(long)]
        prioritize: bool,
    },
    /// What every task in the goal does first (its handoff shows it): {task} {n} {wave} {goal} are filled in; none clears it
    Setup { goal: String, text: String },
    /// Name a wave, or stop the goal after it for the owner's review
    Wave {
        goal: String,
        wave: i64,
        #[arg(long)]
        name: Option<String>,
        /// Stop the goal after this wave for the owner's review
        #[arg(long, value_parser = ["on", "off"])]
        stop: Option<String>,
    },
    /// Let the goal go on past a wave it stopped at (a review stop or a failed task), when the owner says so
    Continue { goal: String, wave: i64 },
    /// Delete a goal (only when the owner asks)
    Delete {
        goal: String,
        #[arg(long)]
        keep_tasks: bool,
        #[arg(long)]
        delete_tasks: bool,
        #[arg(long)]
        delete_backlog: bool,
    },
}

#[derive(Subcommand)]
enum TaskCmd {
    /// Add a task: planned in a goal, or on its own
    New {
        title: String,
        #[arg(long)]
        detail: Option<String>,
        #[arg(long)]
        goal: Option<String>,
        /// Its work also finishes this goal (counted there, run by --goal); repeat for more
        #[arg(long)]
        also: Vec<String>,
        /// The goal's wave it runs in, side by side with the rest of that wave
        #[arg(long)]
        wave: Option<i64>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        planned: bool,
        /// A standalone task for the work this terminal is already doing with the owner; you're on it at once
        #[arg(long)]
        here: bool,
        /// It starts only once this task (any goal) is done; repeat for more
        #[arg(long = "waits-for", value_name = "T12")]
        waits_for: Vec<String>,
        /// A named lock it holds while it runs; tasks sharing a lock never run together. Repeat for more
        #[arg(long, value_name = "NAME")]
        lock: Vec<String>,
        /// Nothing else in its goal runs while it does (board: nothing else on the board)
        #[arg(long, num_args = 0..=1, default_missing_value = "goal", value_parser = ["goal", "board", "none"])]
        alone: Option<String>,
        /// A Jira key to link, `new` for a new ticket, or `none` for no ticket
        #[arg(long)]
        jira: Option<String>,
    },
    /// Change a task
    Set {
        task: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        detail: Option<String>,
        #[arg(long)]
        goal: Option<String>,
        /// Its work also finishes this goal: one task, counted in both; its own goal still runs it. Repeat for more
        #[arg(long)]
        also: Vec<String>,
        /// It no longer finishes this other goal
        #[arg(long = "not-also")]
        not_also: Vec<String>,
        /// Its wave in its goal (a number), or none
        #[arg(long)]
        wave: Option<String>,
        #[arg(long, value_parser = ["normal", "high"])]
        priority: Option<String>,
        #[arg(long = "waits-for")]
        waits_for: Option<String>,
        /// A named lock it holds while it runs; tasks sharing a lock never run together. Repeat for more, none for none
        #[arg(long, value_name = "NAME|none")]
        lock: Vec<String>,
        /// Nothing else in its goal runs while it does (board: nothing else on the board); none to run alongside others
        #[arg(long, num_args = 0..=1, default_missing_value = "goal", value_parser = ["goal", "board", "none"])]
        alone: Option<String>,
        /// A Jira key to link, `new` for a new ticket, or `none`
        #[arg(long)]
        jira: Option<String>,
    },
    /// Delete a task with its log (only when the owner asks)
    Delete { task: String },
}

#[derive(Subcommand)]
enum BacklogCmd {
    /// Add an issue to the backlog
    Add {
        title: String,
        #[arg(long, default_value = "bug", value_parser = ["bug", "gap", "follow", "clean"])]
        kind: String,
        #[arg(long)]
        detail: Option<String>,
        #[arg(long)]
        goal: Option<String>,
        #[arg(long)]
        project: Option<String>,
    },
    /// Change a backlog issue's title or detail
    Set {
        issue: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        detail: Option<String>,
    },
    /// Move a backlog issue to another goal (G2), or out of its goal (none)
    Move { issue: String, goal: String },
    /// Make a backlog issue into a task: planned in its goal, or with --board queued on the board
    Task {
        issue: String,
        #[arg(long)]
        board: bool,
    },
    /// Ask Jira for a ticket for a backlog issue
    Ticket { issue: String },
    /// Close a backlog issue as won't do (only when the owner says so)
    Drop {
        issue: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Open a closed backlog issue again
    Reopen { issue: String },
}

#[derive(Subcommand)]
enum AlertCmd {
    /// Raise an alert: tb alert raise "main is red on web" --urgent --key main:web
    Raise {
        text: String,
        /// It repeats outside the work hours too, shows first and can't be dismissed until it's cleared
        #[arg(long)]
        urgent: bool,
        /// Name it so raising it again changes nothing and `tb alert clear <key>` takes it away
        #[arg(long)]
        key: Option<String>,
        /// The task it's about (its banner gets an Open button; it clears once the task moves on)
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        goal: Option<String>,
    },
    /// Clear an alert by its id or key
    Clear { id: String },
}

#[derive(Subcommand)]
enum ProjectCmd {
    /// A project's PR flow and git remote (every project with no name)
    Show { name: Option<String> },
    /// Change a project: --pr-flow auto (by its git remote), on or off
    Set {
        name: String,
        #[arg(long = "pr-flow", value_parser = ["auto", "on", "off"])]
        pr_flow: Option<String>,
    },
}

#[derive(Subcommand)]
enum PrCmd {
    /// The PR's stage, checks and review
    Status { task: Option<String> },
    /// Finished this visit to the PR; the board brings you back when it needs you
    Wait { task: Option<String> },
    /// The PR was merged
    Merged { task: Option<String> },
    /// Count the PR's checks as passed (e.g. a hook cancelled the builds), so it moves on to review
    SkipChecks {
        task: Option<String>,
        #[arg(long)]
        reason: Option<String>,
        /// Every later push too, not just the current one
        #[arg(long)]
        all: bool,
    },
}

fn out(line: &str) {
    println!("{}", line.trim_end_matches('\n'));
}

fn task_ref(v: &str) -> Result<String, String> {
    let s = v.trim();
    let digits = s.trim_start_matches(['T', 't']);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("expected T<number>, got {v:?}"));
    }
    Ok(format!("T{digits}"))
}

fn goal_ref(v: &str) -> Result<String, String> {
    let s = v.trim();
    let digits = s.trim_start_matches(['G', 'g']);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("expected G<number>, got {v:?}"));
    }
    Ok(format!("G{digits}"))
}

fn issue_ref(v: &str) -> Result<String, String> {
    let s = v.trim();
    let digits = s.trim_start_matches(['B', 'b']);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("expected B<number>, got {v:?}"));
    }
    Ok(format!("B{digits}"))
}

struct Ctx {
    cfg: Config,
    session: String,
    claude: String,
    cwd: String,
}

impl Ctx {
    fn new() -> Ctx {
        Ctx {
            cfg: client::config(),
            session: std::env::var("MIDNA_SESSION").unwrap_or_default(),
            claude: std::env::var("CLAUDE_CODE_SESSION_ID").unwrap_or_default(),
            cwd: std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
        }
    }

    fn body(&self, event: &str, fields: Value, task: Option<&str>) -> Value {
        let mut b = client::base_body(event, &self.session, &self.claude, &self.cwd, client::git_info(&self.cwd, 0.5));
        if let Some(t) = task {
            b["task"] = json!(t);
        }
        if let Value::Object(f) = fields {
            for (k, v) in f {
                b[k] = v;
            }
        }
        b
    }

    /// Posts a report; spools it when the board can't be reached. None = spooled.
    fn report(&self, event: &str, fields: Value, task: Option<&str>, timeout: f64) -> Result<Option<Value>, String> {
        let body = self.body(event, fields, task);
        match client::request(&self.cfg, "POST", "/report", Some(&body), timeout) {
            Ok(v) => Ok(Some(v)),
            Err(CallError::Refused(m)) => Err(m),
            Err(CallError::Unreachable(_)) => {
                client::spool_write(&self.cfg, &body).map_err(|e| format!("the board isn't answering and the report couldn't be saved: {e}"))?;
                Ok(None)
            }
        }
    }

    /// How the board names this terminal in an issue's history.
    fn who(&self) -> String {
        if self.session.is_empty() { "tb".to_string() } else { format!("terminal {}", self.session.chars().take(8).collect::<String>()) }
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        match client::request(&self.cfg, method, path, body.as_ref(), TB_TIMEOUT) {
            Ok(v) => Ok(v),
            Err(CallError::Refused(m)) => Err(m),
            Err(CallError::Unreachable(e)) => Err(format!("the board isn't answering ({e}). Is `taskboard serve` running?")),
        }
    }

    fn run_report(&self, event: &str, fields: Value, task: Option<String>, needs_task: bool, done: impl Fn(&Value) -> String) -> Result<i32, String> {
        let task = task.map(|t| task_ref(&t)).transpose()?;
        match self.report(event, fields, task.as_deref(), TB_TIMEOUT)? {
            None => out(SAVED),
            Some(v) => {
                if needs_task && !v["task"].is_string() {
                    out(NO_TASK);
                } else {
                    out(&done(&v));
                }
            }
        }
        Ok(0)
    }

    /// The task a PR command is about: the one named, or this terminal's (including a PR it's visiting).
    fn pr_task(&self, task: Option<String>) -> Result<String, String> {
        if let Some(t) = task {
            return task_ref(&t);
        }
        if self.session.is_empty() {
            return Err("name the task, for example: tb pr status T12".into());
        }
        let v = self.call("GET", &format!("/whoami?session={}", self.session), None)?;
        v["task"]["ref"].as_str().map(|s| s.to_string()).or_else(|| v["visiting"]["ref"].as_str().map(|s| s.to_string())).ok_or_else(|| "this terminal has no task; name one, for example: tb pr status T12".into())
    }
}

/// Longest title a terminal may give a task or backlog issue: a card shows it whole.
const TITLE_MAX: usize = 80;

/// A title as a short summary, or why it isn't one.
fn short_title(title: &str) -> Result<String, String> {
    let t = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let n = t.chars().count();
    if n > TITLE_MAX {
        return Err(format!(
            "that title is {n} characters; keep it to {TITLE_MAX} or fewer. Write a short summary a person can read at a glance (for example \"midnad tests fail on clean main\") and put names, errors and specifics in --detail"
        ));
    }
    Ok(t)
}

/// Checks each `--task "title::detail"`'s title.
fn short_task_titles(tasks: &[String]) -> Result<(), String> {
    tasks.iter().try_for_each(|x| short_title(x.split_once("::").map_or(x.as_str(), |(t, _)| t)).map(|_| ()))
}

fn midna_attention(text: &str) {
    if std::env::var("MIDNA_SESSION").map(|s| s.is_empty()).unwrap_or(true) {
        return;
    }
    if let Some(m) = client::midna_path() {
        let _ = std::process::Command::new(m)
            .args(["attention", text])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// "holds local-core" and "runs alone in its goal".
fn lock_bits(t: &Value) -> Vec<String> {
    let mut bits = vec![];
    if let Some(l) = t["locks"].as_array().filter(|a| !a.is_empty()) {
        bits.push(format!("holds {}", l.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")));
    }
    if let Some(a) = t["alone"].as_str() {
        bits.push(alone_text(a).into());
    }
    bits
}

fn alone_text(scope: &str) -> &'static str {
    match scope {
        "board" => "runs alone on the board",
        "goal" => "runs alone in its goal",
        _ => "runs alone",
    }
}

/// `--lock a,b --lock c` → [a, b, c]; `--lock none` → [].
fn lock_arg(values: &[String]) -> Vec<String> {
    if values.len() == 1 && values[0].trim().eq_ignore_ascii_case("none") {
        return vec![];
    }
    values.iter().flat_map(|v| v.replace(',', " ").split_whitespace().map(|x| x.to_string()).collect::<Vec<_>>()).collect()
}

fn print_warnings(v: &Value) {
    for w in v["warnings"].as_array().into_iter().flatten().filter_map(|w| w.as_str()) {
        out(&format!("Note: {w}"));
    }
}

/// `--jira` on `tb task new`: a key links it, `new` asks for one, `none` means no ticket.
fn jira_arg(v: &str) -> Value {
    match v.trim().to_lowercase().as_str() {
        "new" => json!({"mode": "create"}),
        "none" => json!({"mode": "none"}),
        _ => json!({"mode": "link", "key": v.trim()}),
    }
}

fn jira_job_line(j: &Value) -> String {
    let a = &j["args"];
    let r = &j["result"];
    let what = match a["op"].as_str().unwrap_or("") {
        "create" => format!("find or make “{}”", a["summary"].as_str().unwrap_or("")),
        "transition" => format!("move {} to {}", a["key"].as_str().unwrap_or(""), a["status"].as_str().unwrap_or("")),
        "status" => format!("read {}'s status", a["key"].as_str().unwrap_or("")),
        "comment" => format!("comment on {}", a["key"].as_str().unwrap_or("")),
        op => op.to_string(),
    };
    let how = match (j["state"].as_str().unwrap_or(""), r["key"].as_str(), r["message"].as_str()) {
        ("done", Some(k), _) => format!("done: {k}{}", r["status"].as_str().map(|s| format!(" ({s})")).unwrap_or_default()),
        ("failed" | "expired", _, Some(m)) => format!("failed: {m}"),
        (st, _, _) => st.to_string(),
    };
    format!("{}  {what} · {how}{}", j["ref"].as_str().unwrap_or(""), j["task_id"].as_i64().map(|t| format!(" · T{t}")).unwrap_or_default())
}

fn jira_cmd(c: &Ctx, job: Option<String>, result: Option<String>, rest: Vec<String>) -> Result<i32, String> {
    let Some(job) = job else {
        let v = c.call("GET", "/jira", None)?;
        if v["on"] != true {
            out("Jira is off: the board's config.toml has no [jira] site and project.");
            return Ok(0);
        }
        out(&format!(
            "Jira {} {} through {}{}{}.",
            v["site"].as_str().unwrap_or(""),
            v["project"].as_str().unwrap_or(""),
            if v["via"] == "claude" { "headless Claude and the Atlassian connector" } else { "the REST API" },
            if v["auto_ticket"] == true { "; every PR task gets a ticket" } else { "" },
            if v["desk"] == true { "; the Jira desk finds or makes tickets" } else { "" }
        ));
        for p in v["products"].as_array().cloned().unwrap_or_default() {
            out(&format!("Product {}: {}", p["name"].as_str().unwrap_or(""), p["what"].as_str().unwrap_or("")));
        }
        for j in v["jobs"].as_array().cloned().unwrap_or_default() {
            out(&jira_job_line(&j));
        }
        return Ok(0);
    };
    let n = job.trim().trim_start_matches(['J', 'j']).to_string();
    if n.is_empty() || !n.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(format!("“{job}” isn't a Jira job; they look like J12"));
    }
    let Some(result) = result else {
        let v = c.call("GET", &format!("/jira/jobs/J{n}"), None)?;
        out(&jira_job_line(&v));
        return Ok(0);
    };
    let mut b = json!({"ok": result == "ok"});
    let mut why = vec![];
    for r in rest {
        match r.split_once('=') {
            Some((k, v)) if ["key", "status", "found", "product"].contains(&k.to_lowercase().as_str()) => {
                let k = k.to_lowercase();
                b[&k] = if k == "found" { json!(matches!(v.to_lowercase().as_str(), "yes" | "true" | "1")) } else { json!(v) };
            }
            _ => why.push(r),
        }
    }
    if !why.is_empty() {
        b["message"] = json!(why.join(" "));
    }
    let v = c.call("POST", &format!("/jira/jobs/J{n}"), Some(b))?;
    out(&jira_job_line(&v));
    Ok(0)
}

fn goal_lines(g: &Value) -> Vec<String> {
    let mut lines = vec![format!(
        "{} · {} · {} · {}/{} done",
        g["ref"].as_str().unwrap_or(""),
        g["name"].as_str().unwrap_or(""),
        g["project"].as_str().unwrap_or(""),
        g["done"],
        g["total"]
    )];
    if let Some(o) = g["outcome"].as_str().filter(|o| !o.is_empty()) {
        lines.push(format!("Done when: {o}"));
    }
    if g["paused"] == true {
        lines.push("Paused: nothing new starts.".into());
    }
    if let Some(s) = g["setup"].as_str().filter(|s| !s.is_empty()) {
        lines.push(format!("Set up: {s}"));
    }
    lines.push(format!(
        "Runs {}, at most {} at a time.",
        if g["run_in_order"] == true { "in order" } else { "in any order" },
        g["max_terminals"]
    ));
    if let Some(tasks) = g["tasks"].as_array() {
        for (i, t) in tasks.iter().enumerate() {
            let status = if t["failed"] == true { "failed".to_string() } else { t["status"].as_str().unwrap_or("").to_string() };
            let mut line = format!("  {}. {} [{status}] {}", i + 1, t["ref"].as_str().unwrap_or(""), t["title"].as_str().unwrap_or(""));
            if let Some(w) = t["wave"].as_i64() {
                line += &format!(" · wave {w}");
            }
            if let Some(also) = t["also"].as_array().filter(|a| !a.is_empty()) {
                line += &format!(" · also for {}", also.iter().filter_map(|x| x["ref"].as_str()).collect::<Vec<_>>().join(", "));
            }
            if let Some(w) = t["waits_for"].as_array().filter(|a| !a.is_empty()) {
                line += &format!(" · waits for {}", w.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "));
            }
            for b in lock_bits(t) {
                line += &format!(" · {b}");
            }
            if let Some(w) = t["waiting"].as_str() {
                line += &format!(" — {w}");
            }
            if let Some(u) = t["pr"]["url"].as_str() {
                line += &format!(" ({u})");
            }
            lines.push(line);
        }
    }
    if let Some(waves) = g["waves"].as_array().filter(|a| !a.is_empty()) {
        lines.push("Waves:".into());
        for w in waves {
            let name = w["name"].as_str().filter(|n| !n.is_empty()).map(|n| format!(" · {n}")).unwrap_or_default();
            let mut line = format!("  Wave {}{name} · {} · {}/{} done", w["wave"], w["state"].as_str().unwrap_or(""), w["done"], w["total"]);
            if w["stop_after"] == true {
                line += if w["released_at"].is_string() { " · stop point, let go on" } else { " · stop point" };
            }
            if let Some(h) = w["hold"].as_str() {
                line += &format!(" · {h}");
            }
            lines.push(line);
        }
    }
    if let Some(shared) = g["shared"].as_array().filter(|a| !a.is_empty()) {
        lines.push("From other goals (their home goal runs them):".into());
        for t in shared {
            let status = if t["failed"] == true { "failed".to_string() } else { t["status"].as_str().unwrap_or("").to_string() };
            let mut line = format!("  {} [{status}] {} · from {}", t["ref"].as_str().unwrap_or(""), t["title"].as_str().unwrap_or(""), t["goal"]["ref"].as_str().unwrap_or("?"));
            if let Some(n) = t["pr"]["num"].as_i64() {
                line += &format!(" · PR #{n}");
            }
            lines.push(line);
        }
    }
    if let Some(issues) = g["backlog"].as_array().filter(|a| !a.is_empty()) {
        lines.push("Backlog:".into());
        for b in issues.iter().filter(|b| b["state"] == "open") {
            lines.push(format!("  {} {}", b["ref"].as_str().unwrap_or(""), b["title"].as_str().unwrap_or("")));
        }
    }
    lines
}

/// The task's steps (`GET /steps`) and the step called `name`.
fn find_step(c: &Ctx, task: Option<String>, name: &str) -> Result<(Value, Step), String> {
    let v = c.call("GET", &steps_path(c, task)?, None)?;
    if !v["task"].is_string() {
        return Err(NO_TASK.into());
    }
    let all = steps::from_listing(&v);
    match all.iter().find(|(s, _)| steps::key(&s.name) == steps::key(name)) {
        Some((s, _)) => Ok((v.clone(), s.clone())),
        None if all.is_empty() => Err(format!("{} has no steps.", v["task"].as_str().unwrap_or("The task"))),
        None => Err(format!("“{name}” isn't one of its steps: {}.", steps::names(&all.into_iter().map(|(s, _)| s).collect::<Vec<_>>()))),
    }
}

fn steps_path(c: &Ctx, task: Option<String>) -> Result<String, String> {
    Ok(match task {
        Some(x) => format!("/steps?task={}", task_ref(&x)?),
        None => format!("/steps?session={}", c.session),
    })
}

/// The placeholders' values for a script: the board's, with this checkout's branch.
fn step_vars(c: &Ctx, v: &Value, name: &str) -> BTreeMap<String, String> {
    let mut vars: BTreeMap<String, String> = serde_json::from_value(v["vars"].clone()).unwrap_or_default();
    if let Some(b) = client::git_info(&c.cwd, 0.5)["branch"].as_str().filter(|b| !b.is_empty()) {
        vars.insert("branch".into(), b.to_string());
    }
    vars.insert("step".into(), name.to_string());
    vars
}

/// Runs a step's script in the repo (else here), its output shown as it comes; the exit and the
/// output's tail.
fn run_script(c: &Ctx, label: &str, script: &str, vars: &BTreeMap<String, String>, timeout: u64) -> (bool, String) {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let dir = vars.get("repo").filter(|r| !r.is_empty() && std::path::Path::new(r).is_dir()).cloned().unwrap_or_else(|| c.cwd.clone());
    out(&format!("{label}: {script}"));
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(format!("exec 2>&1\n{script}")).current_dir(&dir).stdin(Stdio::null()).stdout(Stdio::piped());
    // Its own process group, so a timeout stops everything the script started.
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    for (k, v) in vars {
        cmd.env(format!("TASKBOARD_{}", k.to_uppercase()), v);
    }
    let mut child = match cmd.spawn() {
        Ok(ch) => ch,
        Err(e) => return (false, format!("couldn't start it: {e}")),
    };
    let stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut tail: std::collections::VecDeque<String> = Default::default();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            println!("{line}");
            tail.push_back(line);
            if tail.len() > 200 {
                tail.pop_front();
            }
        }
        tail.into_iter().collect::<Vec<_>>().join("\n")
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() > Duration::from_secs(timeout) => {
                let _ = Command::new("/bin/kill").args(["-KILL", "--", &format!("-{}", child.id())]).status();
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => break None,
        }
    };
    let mut output = reader.join().unwrap_or_default();
    let passed = match status {
        Some(s) if s.success() => true,
        Some(s) => {
            let line = format!("({label} exited {})", s.code().map(|c| c.to_string()).unwrap_or_else(|| "on a signal".into()));
            out(&line);
            output += &format!("\n{line}");
            false
        }
        None => {
            let line = format!("({label} stopped after {timeout}s)");
            out(&line);
            output += &format!("\n{line}");
            false
        }
    };
    (passed, output)
}

fn step_cmd(c: &Ctx, action: StepCmd) -> Result<i32, String> {
    match action {
        StepCmd::Done { name, note, skip, t } => {
            let (v, step) = find_step(c, t.task.clone(), &name)?;
            let mine = !c.session.is_empty() && v["session"].as_str() == Some(c.session.as_str());
            if skip || (step.owner && !mine) {
                let task = v["task"].as_str().unwrap_or("").to_string();
                c.call("POST", &format!("/tasks/{task}/step"), Some(json!({"name": step.name, "note": note, "skip": skip, "session": c.session})))?;
                out(&format!("{} {} on {task}.", step.name, if skip { "skipped" } else { "done" }));
                return Ok(0);
            }
            let mut f = json!({"name": step.name, "note": note, "via": "done"});
            if !step.check.is_empty() && !step.owner {
                let vars = step_vars(c, &v, &step.name);
                let (passed, output) = run_script(c, "Check", &steps::fill(&step.check, &vars), &vars, step.timeout_secs());
                f["ok"] = json!(passed);
                f["output"] = json!(output);
            }
            step_report(c, f, t.task)
        }
        StepCmd::Run { name, t } => {
            let (v, step) = find_step(c, t.task.clone(), &name)?;
            if step.run.is_empty() {
                return Err(format!("“{}” has no script to run: {}.", step.name, step.how("tb")));
            }
            let vars = step_vars(c, &v, &step.name);
            let (mut passed, mut output) = run_script(c, "Script", &steps::fill(&step.run, &vars), &vars, step.timeout_secs());
            if passed && !step.check.is_empty() {
                let (p, o) = run_script(c, "Check", &steps::fill(&step.check, &vars), &vars, step.timeout_secs());
                passed = p;
                output = format!("{output}\n{o}");
            }
            step_report(c, json!({"name": step.name, "via": "run", "ok": passed, "output": output}), t.task)
        }
        StepCmd::Ask { name, t } => c.run_report("tb.step_ask", json!({"name": name}), t.task, true, |v| {
            if v["already"] == true {
                return format!("{} is already done; carry on.", v["step"].as_str().unwrap_or("The step"));
            }
            let mut s = format!("Asked {} to do {}; the task waits for them. End your turn now.", c.cfg.owner, v["step"].as_str().unwrap_or("the step"));
            if let Some(o) = v["open"].as_str().filter(|o| !o.is_empty()) {
                s += &format!(" (It happens at {o}.)");
            }
            s
        }),
        StepCmd::Fail { name, why, t } => c.run_report("tb.step_fail", json!({"name": name, "why": why}), t.task, true, |v| {
            format!("Told {} that {} can't pass; the task waits for their answer. End your turn now.", c.cfg.owner, v["step"].as_str().unwrap_or("the step"))
        }),
    }
}

/// Reports a step's outcome; a failed one exits 1 so the agent sees it.
fn step_report(c: &Ctx, f: Value, task: Option<String>) -> Result<i32, String> {
    let task = task.map(|t| task_ref(&t)).transpose()?;
    let passed = f["ok"] != false;
    match c.report("tb.step", f, task.as_deref(), TB_TIMEOUT)? {
        None => out(SAVED),
        Some(v) if !v["task"].is_string() => out(NO_TASK),
        Some(v) => {
            let step = v["step"].as_str().unwrap_or("The step");
            let left: Vec<&str> = v["left"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
            if !passed {
                out(&format!("{step} didn't pass (output above). Fix what it reports and try again, or tb step fail \"{step}\" --why \"…\" if it can't pass."));
                return Ok(1);
            }
            out(&if left.is_empty() { format!("{step} is done. No steps left.") } else { format!("{step} is done. Still to do: {}.", left.join(", ")) });
        }
    }
    Ok(if passed { 0 } else { 1 })
}

fn run_cmd(c: &Ctx, cmd: Cmd) -> Result<i32, String> {
    match cmd {
        Cmd::Note { text, goal, kind, t } => {
            let mut f = json!({"text": text});
            if goal {
                f["goal"] = json!(true);
                f["note_kind"] = json!(kind);
            }
            c.run_report("tb.note", f, t.task, !goal, |v| match v["goal"].as_str() {
                Some(g) => format!("Noted on {g}."),
                None => format!("Noted on {}.", v["task"].as_str().unwrap_or("the task")),
            })
        }
        Cmd::Step { action } => step_cmd(c, action),
        Cmd::Steps { t } => {
            let v = c.call("GET", &steps_path(c, t.task)?, None)?;
            let Some(task) = v["task"].as_str() else {
                out(NO_TASK);
                return Ok(0);
            };
            let all = steps::from_listing(&v);
            if all.is_empty() {
                out(&format!("{task} has no steps."));
            }
            let vars = step_vars(c, &v, "");
            for (st, done) in all {
                let when = if st.before == steps::Before::Done { "before tb done" } else { "before the PR" };
                let st = st.filled(&vars);
                out(&format!("[{}] {} ({when}): {}. {}", if done { "done" } else { "to do" }, st.name, st.what(), st.how("tb")));
            }
            Ok(0)
        }
        Cmd::Checkpoint { done, next, decisions, files, t } => {
            let mut f = json!({"done": done, "next": next, "decisions": decisions});
            if !files.is_empty() {
                f["files"] = json!(files);
            }
            c.run_report("tb.checkpoint", f, t.task, true, |v| format!("Checkpoint saved on {}.", v["task"].as_str().unwrap_or("")))
        }
        Cmd::Found { title, kind, detail, output, t } => c.run_report(
            "tb.found",
            json!({"title": short_title(&title)?, "kind": kind, "detail": detail, "output": output.map(|o| o.chars().take(4000).collect::<String>())}),
            t.task,
            false,
            |v| {
                if v["seen"] == true {
                    format!("Already in the backlog as {}; added this sighting.", v["issue"].as_str().unwrap_or(""))
                } else {
                    format!("Added {} to the backlog.", v["issue"].as_str().unwrap_or(""))
                }
            },
        ),
        Cmd::Question { text, t } => {
            let task = t.task.map(|x| task_ref(&x)).transpose()?;
            let mut alert = Some(text.clone());
            let r = c.report("tb.question", json!({"text": text}), task.as_deref(), QUESTION_TIMEOUT);
            let owner = c.cfg.owner.clone();
            let res = match r {
                Ok(None) => {
                    out(SAVED);
                    Ok(0)
                }
                Ok(Some(v)) if v["answer"].is_string() => {
                    alert = None;
                    out(&format!(
                        "Answered without asking {owner}, from {}:\n{}\n\nCarry on with the work. If this answer really doesn't fit, ask again with tb question and say why.",
                        v["source"].as_str().unwrap_or("the rules"),
                        v["answer"].as_str().unwrap_or("")
                    ));
                    Ok(0)
                }
                Ok(Some(v)) => {
                    if let Some(q) = v["question"].as_str() {
                        alert = Some(q.to_string());
                    }
                    match v["task"].as_str() {
                        Some(t) => out(&format!("Asked {owner} on {t}; the task now waits for the answer.")),
                        None => out(NO_TASK),
                    }
                    Ok(0)
                }
                Err(e) => Err(e),
            };
            if let Some(a) = alert {
                midna_attention(&a);
            }
            res
        }
        Cmd::Done { summary, pr, human, t } => {
            c.run_report("tb.done", json!({"summary": summary, "pr": pr, "human": human}), t.task, true, |v| format!("{} is done.", v["task"].as_str().unwrap_or("")))
        }
        Cmd::Fail { reason, t } => {
            c.run_report("tb.fail", json!({"reason": reason}), t.task, true, |v| format!("{} is marked failed.", v["task"].as_str().unwrap_or("")))
        }
        Cmd::Take { task } => {
            let r = task_ref(&task)?;
            match c.report("tb.take", json!({"task": r}), None, TB_TIMEOUT)? {
                None => out(SAVED),
                Some(v) => out(v["context"].as_str().unwrap_or(&format!("Took {r}."))),
            }
            Ok(0)
        }
        Cmd::Status { t } => {
            if let Some(x) = t.task {
                let v = c.call("GET", &format!("/tasks/{}", task_ref(&x)?), None)?;
                out(&format!("{} · {} · {}", v["ref"].as_str().unwrap_or(""), v["title"].as_str().unwrap_or(""), v["status"].as_str().unwrap_or("")));
                return Ok(0);
            }
            match c.report("tb.status", json!({}), None, TB_TIMEOUT)? {
                None => out("The board isn't answering."),
                Some(v) => out(v["context"].as_str().unwrap_or("No task on this terminal.")),
            }
            Ok(0)
        }
        Cmd::WaitFor { tasks, why, t } => {
            if tasks.is_empty() {
                return Err("say which task this one needs, for example: tb wait-for T14 (or none)".into());
            }
            let list = if tasks.len() == 1 && tasks[0].eq_ignore_ascii_case("none") {
                json!("none")
            } else {
                json!(tasks.iter().map(|x| task_ref(x)).collect::<Result<Vec<_>, _>>()?)
            };
            c.run_report("tb.wait_for", json!({"tasks": list, "why": why}), t.task, true, |v| {
                if v["parked"] == true {
                    format!(
                        "{} waits now: {}. End your turn; the board carries this conversation on once it's ready.",
                        v["task"].as_str().unwrap_or(""),
                        v["context"].as_str().unwrap_or("")
                    )
                } else {
                    v["context"].as_str().unwrap_or("Noted.").to_string()
                }
            })
        }
        Cmd::Propose { goal, tasks } => {
            let g = goal_ref(&goal)?;
            short_task_titles(&tasks)?;
            match c.report("tb.propose", json!({"goal": g, "tasks": tasks}), None, TB_TIMEOUT)? {
                None => out(SAVED),
                Some(v) => {
                    out(&format!("Added {} to {g} as planned.", v["created"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default()));
                    print_warnings(&v);
                }
            }
            Ok(0)
        }
        Cmd::Jira { job, result, rest } => jira_cmd(c, job, result, rest),
        Cmd::Locks => {
            let v = c.call("GET", "/locks", None)?;
            let locks = v["locks"].as_array().cloned().unwrap_or_default();
            let alone = v["alone"].as_array().cloned().unwrap_or_default();
            if locks.is_empty() && alone.is_empty() {
                out("No task holds a lock or runs alone.");
            }
            for e in locks {
                let holder = e["held_by"].as_str().map(|h| format!("held by {h}")).unwrap_or_else(|| "free".into());
                let rest = e["tasks"].as_array().filter(|a| !a.is_empty()).map(|a| format!(" · waiting or planned: {}", a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))).unwrap_or_default();
                out(&format!("{} · {holder}{rest}", e["name"].as_str().unwrap_or("")));
            }
            for x in alone {
                let scope = x["scope"].as_str().unwrap_or("");
                out(&format!(
                    "{} {} · {}{}{}",
                    x["ref"].as_str().unwrap_or(""),
                    x["title"].as_str().unwrap_or(""),
                    alone_text(scope),
                    x["goal"].as_str().filter(|_| scope == "goal").map(|g| format!(" ({g})")).unwrap_or_default(),
                    if x["running"] == true { " · running" } else { "" }
                ));
            }
            Ok(0)
        }
        Cmd::Goals { project } => {
            let v = c.call("GET", &format!("/goals?project={}", project.unwrap_or_else(|| "all".into())), None)?;
            let goals = v["goals"].as_array().cloned().unwrap_or_default();
            if goals.is_empty() {
                out("No goals.");
            }
            for g in goals {
                out(&format!(
                    "{} · {} · {} · {}/{} done{}",
                    g["ref"].as_str().unwrap_or(""),
                    g["name"].as_str().unwrap_or(""),
                    g["project"].as_str().unwrap_or(""),
                    g["done"],
                    g["total"],
                    if g["paused"] == true { " · paused" } else { "" }
                ));
            }
            Ok(0)
        }
        Cmd::Goal { action } => match action {
            GoalCmd::New { name, outcome, tldr, project, product, tasks } => {
                short_task_titles(&tasks)?;
                match c.report("tb.goal", json!({"name": name, "outcome": outcome, "tldr": tldr, "project": project, "product": product, "tasks": tasks}), None, TB_TIMEOUT)? {
                    None => out(SAVED),
                    Some(v) => {
                        let created = v["created"].as_array().map(|a| a.len()).unwrap_or(0);
                        out(&format!(
                            "Made {} “{}” in {}{}. {}tasks{}",
                            v["goal"].as_str().unwrap_or(""),
                            v["name"].as_str().unwrap_or(""),
                            v["project"].as_str().unwrap_or(""),
                            if created > 0 { format!(" with {created} planned") } else { String::new() },
                            c.cfg.page_url,
                            format!("#/goals/{}", v["goal"].as_str().unwrap_or(""))
                        ));
                        print_warnings(&v);
                    }
                }
                Ok(0)
            }
            GoalCmd::Show { goal } => {
                let v = c.call("GET", &format!("/goals/{}", goal_ref(&goal)?), None)?;
                for l in goal_lines(&v) {
                    out(&l);
                }
                Ok(0)
            }
            GoalCmd::Set { goal, name, outcome, tldr, paused, in_order, max_terminals, epic, product, worktrees, run, deprioritize, prioritize } => {
                let g = goal_ref(&goal)?;
                let mut b = json!({});
                if let Some(x) = name {
                    b["name"] = json!(x);
                }
                if let Some(x) = outcome {
                    b["outcome"] = json!(x);
                }
                if let Some(x) = tldr {
                    b["tldr"] = json!(x);
                }
                if let Some(x) = paused {
                    b["paused"] = json!(x == "on");
                }
                if let Some(x) = in_order {
                    b["run_in_order"] = json!(x == "on");
                }
                if let Some(x) = max_terminals {
                    b["max_terminals"] = json!(x);
                }
                if let Some(x) = epic {
                    b["epic_key"] = json!(if x.eq_ignore_ascii_case("none") { String::new() } else { x });
                }
                if let Some(x) = product {
                    b["product"] = json!(x);
                }
                if let Some(x) = worktrees {
                    b["worktree_base"] = json!(x);
                }
                if deprioritize || prioritize {
                    b["deprioritized"] = json!(deprioritize);
                }
                let empty = b.as_object().map(|o| o.is_empty()).unwrap_or(true);
                if empty && !run {
                    return Err("say what to change, for example: tb goal set G3 --paused on".into());
                }
                if !empty {
                    let v = c.call("POST", &format!("/goals/{g}"), Some(b))?;
                    out(&format!("Changed {} “{}”.", g, v["name"].as_str().unwrap_or("")));
                }
                if run {
                    let v = c.call("POST", &format!("/goals/{g}/run"), Some(json!({})))?;
                    let n = v["queued_now"].as_i64().unwrap_or(0);
                    out(&format!("{g} runs: queued {n} planned task{}.", if n == 1 { "" } else { "s" }));
                }
                Ok(0)
            }
            GoalCmd::Setup { goal, text } => {
                let g = goal_ref(&goal)?;
                let v = c.call("POST", &format!("/goals/{g}"), Some(json!({"setup": text})))?;
                if v["setup"].is_string() {
                    out(&format!("Set up for {g}: every task's handoff now starts with it."));
                } else {
                    out(&format!("{g} has no setup now."));
                }
                Ok(0)
            }
            GoalCmd::Wave { goal, wave, name, stop } => {
                let g = goal_ref(&goal)?;
                let mut b = json!({});
                if let Some(n) = name {
                    b["name"] = json!(n);
                }
                if let Some(s) = stop {
                    b["stop_after"] = json!(s == "on");
                }
                if b.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                    return Err("say what to change: tb goal wave G3 2 --name \"API\" or --stop on".into());
                }
                c.call("POST", &format!("/goals/{g}/waves/{wave}"), Some(b))?;
                out(&format!("Changed wave {wave} of {g}."));
                Ok(0)
            }
            GoalCmd::Continue { goal, wave } => {
                let g = goal_ref(&goal)?;
                c.call("POST", &format!("/goals/{g}/waves/{wave}/continue"), Some(json!({"who": c.who()})))?;
                out(&format!("{g} goes on past wave {wave}."));
                Ok(0)
            }
            GoalCmd::Delete { goal, keep_tasks, delete_tasks, delete_backlog } => {
                let g = goal_ref(&goal)?;
                if keep_tasks == delete_tasks {
                    let v = c.call("GET", &format!("/goals/{g}"), None)?;
                    out(&format!(
                        "{g} “{}” has {} tasks and {} backlog issues. Ask the owner what goes with it, then run one of:\n  tb goal delete {g} --keep-tasks\n  tb goal delete {g} --delete-tasks\n(add --delete-backlog to delete its backlog too)",
                        v["name"].as_str().unwrap_or(""),
                        v["total"],
                        v["backlog"].as_array().map(|a| a.len()).unwrap_or(0)
                    ));
                    for t in v["tasks"].as_array().cloned().unwrap_or_default() {
                        let also: Vec<&str> = t["also"].as_array().map(|a| a.iter().filter_map(|x| x["ref"].as_str()).collect()).unwrap_or_default();
                        if let Some(last) = also.last() {
                            out(&format!("{} also finishes {}, so it moves to {last} whatever the owner chooses.", t["ref"].as_str().unwrap_or(""), also.join(", ")));
                        }
                    }
                    if let Some(n) = v["shared"].as_array().map(|a| a.len()).filter(|n| *n > 0) {
                        out(&format!("{n} task(s) from other goals stop counting toward it; their home goals keep them."));
                    }
                    return Ok(2);
                }
                let v = c.call("POST", &format!("/goals/{g}/delete"), Some(json!({"tasks": delete_tasks, "backlog": delete_backlog})))?;
                out(&format!("Deleted {g} with {} tasks and {} backlog issues.", v["tasks"], v["issues"]));
                if let Some(moved) = v["moved"].as_array().filter(|a| !a.is_empty()) {
                    out(&format!("Moved to the next goal they finish: {}.", moved.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")));
                }
                Ok(0)
            }
        },
        Cmd::Qa { what, r#ref, note, pr, no_pr, limit } => {
            let what = what.unwrap_or_default();
            let line = |c: &Value| {
                let state = if c["waiting"] == true {
                    "waiting on the owner".to_string()
                } else {
                    match c["verdict"].as_str() {
                        Some("task") => "has work".into(),
                        Some("flag") => "asked the owner".into(),
                        Some("none") => "needs nothing".into(),
                        None => "being read".into(),
                        Some(v) => v.to_string(),
                    }
                };
                let what = c["ask"].as_str().filter(|s| !s.is_empty()).or(c["title"].as_str().filter(|s| !s.is_empty()));
                format!(
                    "{}  {} {} · {}{}{}",
                    c["ref"].as_str().unwrap_or(""),
                    c["jira_key"].as_str().unwrap_or(""),
                    c["author"].as_str().filter(|s| !s.is_empty()).unwrap_or("comment"),
                    state,
                    what.map(|w| format!(": {w}")).unwrap_or_default(),
                    c["task"].as_str().map(|t| format!(" · {t}")).unwrap_or_default()
                )
            };
            if what.is_empty() || what == "list" || what == "waiting" {
                let v = c.call("GET", &format!("/qa-comments?limit={limit}{}", if what == "waiting" { "&waiting=1" } else { "" }), None)?;
                if v["on"] == false {
                    out("QA comments are off. The owner switches them on in Taskboard's Settings ▸ QA.");
                }
                let rows = v["comments"].as_array().cloned().unwrap_or_default();
                if rows.is_empty() {
                    out(if what == "waiting" { "No QA comment is waiting on the owner." } else { "No Jira comments on board tickets yet." });
                }
                for row in rows {
                    out(&line(&row));
                }
                return Ok(0);
            }
            if r#ref.is_none() && what.trim_start_matches(['Q', 'q']).chars().all(|ch| ch.is_ascii_digit()) {
                let v = c.call("GET", &format!("/qa-comments/{what}"), None)?;
                out(&line(&v));
                out(&format!("Comment: {}", v["url"].as_str().unwrap_or("")));
                if let Some(t) = v["text"].as_str().filter(|t| !t.is_empty()) {
                    out(t);
                }
                return Ok(0);
            }
            let Some(r) = r#ref.filter(|_| what == "task" || what == "ignore") else {
                return Err("say tb qa, tb qa waiting, tb qa Q3, tb qa task Q3 [--note TEXT] or tb qa ignore Q3".into());
            };
            let mut b = json!({"action": what, "who": c.who()});
            if let Some(n) = note {
                b["note"] = json!(n);
            }
            if pr || no_pr {
                b["pr"] = json!(pr);
            }
            let v = c.call("POST", &format!("/qa-comments/{r}"), Some(b))?;
            if what == "task" {
                out(&format!("{} is now {}.", v["ref"].as_str().unwrap_or(""), v["started"].as_str().or(v["task"].as_str()).unwrap_or("a task")));
            } else {
                out(&format!("{} is left as it is.", v["ref"].as_str().unwrap_or("")));
            }
            Ok(0)
        }
        Cmd::Task { action } => match action {
            TaskCmd::New { title, detail, goal, also, wave, project, planned, here, waits_for, lock, alone, jira } => {
                if wave.is_some() && goal.is_none() {
                    return Err("--wave needs --goal: waves are a goal's".into());
                }
                if here && (goal.is_some() || !also.is_empty()) {
                    return Err("--here makes a standalone task on this terminal; leave out --goal and --also".into());
                }
                if !also.is_empty() && goal.is_none() {
                    return Err("--also needs --goal: the task's home goal, which runs it".into());
                }
                let goal = goal.map(|g| goal_ref(&g)).transpose()?;
                let also = also.iter().map(|g| goal_ref(g)).collect::<Result<Vec<_>, _>>()?;
                let mut body = json!({"title": short_title(&title)?, "detail": detail, "goal": goal, "project": project, "planned": planned});
                if !also.is_empty() {
                    body["also"] = json!(also);
                }
                if here {
                    body["here"] = json!(true);
                }
                if let Some(w) = wave {
                    body["wave"] = json!(w);
                }
                if !waits_for.is_empty() {
                    body["waits_for"] = json!(waits_for.iter().map(|x| task_ref(x)).collect::<Result<Vec<_>, _>>()?);
                }
                if !lock.is_empty() {
                    body["locks"] = json!(lock_arg(&lock));
                }
                if let Some(a) = alone {
                    body["alone"] = json!(a);
                }
                if let Some(j) = jira {
                    body["jira"] = jira_arg(&j);
                }
                match c.report("tb.new_task", body, None, TB_TIMEOUT)? {
                    None => out(SAVED),
                    Some(v) if here => {
                        let r = v["created"][0].as_str().unwrap_or("").to_string();
                        out(v["context"].as_str().filter(|s| !s.trim().is_empty()).map(|s| s.to_string()).unwrap_or(format!("Added {r} and you're on it.")).as_str());
                    }
                    Some(v) => {
                        let r = v["created"][0].as_str().unwrap_or("").to_string();
                        let where_ = v["goal"].as_str().map(|g| format!(" in {g} as planned")).unwrap_or_else(|| " on the board; it waits for the owner to press Start".into());
                        out(&format!("Added {r}{where_}. {}#/?task={r}", c.cfg.page_url));
                        print_warnings(&v);
                    }
                }
                Ok(0)
            }
            TaskCmd::Set { task, title, detail, goal, also, not_also, wave, priority, waits_for, lock, alone, jira } => {
                let t = task_ref(&task)?;
                let mut b = json!({});
                if let Some(x) = title {
                    b["title"] = json!(short_title(&x)?);
                }
                if let Some(x) = detail {
                    b["detail"] = json!(x);
                }
                if let Some(x) = goal {
                    b["goal_id"] = if x.eq_ignore_ascii_case("none") { Value::Null } else { json!(goal_ref(&x)?) };
                }
                if !also.is_empty() {
                    b["also"] = json!(also.iter().map(|g| goal_ref(g)).collect::<Result<Vec<_>, _>>()?);
                }
                if !not_also.is_empty() {
                    b["not_also"] = json!(not_also.iter().map(|g| goal_ref(g)).collect::<Result<Vec<_>, _>>()?);
                }
                if !also.is_empty() || !not_also.is_empty() {
                    b["who"] = json!(c.who());
                }
                if let Some(x) = priority {
                    b["priority"] = json!(x);
                }
                if let Some(x) = wave {
                    b["wave"] = if x.eq_ignore_ascii_case("none") { Value::Null } else { json!(x) };
                }
                if let Some(x) = waits_for {
                    b["waits_for"] = json!(x);
                }
                if !lock.is_empty() {
                    b["locks"] = json!(lock_arg(&lock));
                }
                if let Some(x) = alone {
                    b["alone"] = json!(x);
                }
                if let Some(x) = jira {
                    b["jira_key"] = json!(x);
                }
                if b.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                    return Err("say what to change, for example: tb task set T12 --priority high".into());
                }
                let v = c.call("POST", &format!("/tasks/{t}"), Some(b))?;
                let mut bits = String::new();
                for x in lock_bits(&v) {
                    bits += &format!(" · {x}");
                }
                out(&format!("Changed {t} “{}”{bits}.", v["title"].as_str().unwrap_or("")));
                print_warnings(&v);
                Ok(0)
            }
            TaskCmd::Delete { task } => {
                let t = task_ref(&task)?;
                c.call("POST", &format!("/tasks/{t}/delete"), Some(json!({})))?;
                out(&format!("Deleted {t} and its log."));
                Ok(0)
            }
        },
        Cmd::Backlog { action: BacklogCmd::Add { title, kind, detail, goal, project } } => {
            let goal = goal.map(|g| goal_ref(&g)).transpose()?;
            let name = c.who();
            let v = c.call(
                "POST",
                "/backlog",
                Some(json!({"title": short_title(&title)?, "kind": kind, "detail": detail, "goal_id": goal, "project": project, "cwd": c.cwd, "where": "tb", "who": name})),
            )?;
            out(&format!("Added {} to the backlog.", v["ref"].as_str().unwrap_or("")));
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::Set { issue, title, detail } } => {
            let r = issue_ref(&issue)?;
            let mut b = json!({"who": c.who()});
            if let Some(x) = title {
                b["title"] = json!(short_title(&x)?);
            }
            if let Some(x) = detail {
                b["detail"] = json!(x);
            }
            if b.as_object().map_or(0, |o| o.len()) < 2 {
                return Err("say what to change, for example: tb backlog set B3 --title \"Login button flickers\"".into());
            }
            let v = c.call("POST", &format!("/backlog/{r}"), Some(b))?;
            out(&format!("Changed {r} “{}”.", v["title"].as_str().unwrap_or("")));
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::Move { issue, goal } } => {
            let r = issue_ref(&issue)?;
            let to = if goal.trim().eq_ignore_ascii_case("none") { Value::Null } else { json!(goal_ref(&goal)?) };
            let v = c.call("POST", &format!("/backlog/{r}/move"), Some(json!({"goal_id": to, "who": c.who()})))?;
            let issue = v.get("issue").filter(|x| x.is_object()).unwrap_or(&v);
            match issue["goal"]["ref"].as_str().or(to.as_str()) {
                Some(g) => out(&format!("Moved {r} to {g}.")),
                None => out(&format!("Moved {r} out of its goal.")),
            }
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::Task { issue, board } } => {
            let r = issue_ref(&issue)?;
            let v = c.call("POST", &format!("/backlog/{r}/promote"), Some(json!({"where": if board { "board" } else { "goal" }})))?;
            let t = &v["task"];
            let where_ = match (t["status"].as_str(), t["goal"]["ref"].as_str()) {
                (Some("planned"), Some(g)) => format!("a planned task in {g}"),
                _ => "a queued task".to_string(),
            };
            out(&format!("Made {r} into {}, {where_}.", t["ref"].as_str().unwrap_or("a task")));
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::Ticket { issue } } => {
            let r = issue_ref(&issue)?;
            c.call("POST", &format!("/backlog/{r}/ticket"), Some(json!({})))?;
            out(&format!("Asked Jira for a ticket for {r}."));
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::Drop { issue, reason } } => {
            let r = issue_ref(&issue)?;
            c.call("POST", &format!("/backlog/{r}/drop"), Some(json!({"reason": reason.unwrap_or_default()})))?;
            out(&format!("Closed {r} as won't do."));
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::Reopen { issue } } => {
            let r = issue_ref(&issue)?;
            c.call("POST", &format!("/backlog/{r}/reopen"), Some(json!({})))?;
            out(&format!("Opened {r} again."));
            Ok(0)
        }
        Cmd::Project { action } => project_cmd(c, action),
        Cmd::Alert { action: AlertCmd::Raise { text, urgent, key, task, goal } } => {
            let task = task.map(|t| task_ref(&t)).transpose()?;
            let goal = goal.map(|g| goal_ref(&g)).transpose()?;
            let v = c.call("POST", "/alerts", Some(json!({"text": text, "urgent": urgent, "key": key, "task": task, "goal": goal})))?;
            let a = &v["alert"];
            out(&format!(
                "Raised {}alert {}{}.",
                if a["urgent"] == true { "an urgent " } else { "an " },
                a["id"].as_str().unwrap_or(""),
                a["key"].as_str().map(|k| format!(" ({k})")).unwrap_or_default()
            ));
            Ok(0)
        }
        Cmd::Alert { action: AlertCmd::Clear { id } } => {
            c.call("POST", &format!("/alerts/{id}/clear"), Some(json!({})))?;
            out(&format!("Cleared {id}."));
            Ok(0)
        }
        Cmd::Unattach { url, goal, t } => {
            let goal = goal.map(|g| goal_ref(&g)).transpose()?;
            let needs = goal.is_none();
            c.run_report("tb.unattach", json!({"url": url, "goal": goal}), t.task, needs, |v| {
                format!("Removed {} from {}.", v["removed"].as_str().unwrap_or("it"), v["goal"].as_str().or(v["task"].as_str()).unwrap_or("the task"))
            })
        }
        Cmd::Attach { url, kind, title, goal, t } => {
            let goal = goal.map(|g| goal_ref(&g)).transpose()?;
            let url = if url.starts_with("http://") || url.starts_with("https://") || url.starts_with('/') || url.starts_with("~/") {
                url
            } else {
                std::fs::canonicalize(&url).map(|p| p.to_string_lossy().to_string()).unwrap_or(url)
            };
            let needs = goal.is_none();
            c.run_report("tb.attach", json!({"url": url, "kind": kind, "title": title, "goal": goal}), t.task, needs, |v| {
                format!("Attached {} to {}.", v["attachment"]["title"].as_str().unwrap_or("it"), v["goal"].as_str().or(v["task"].as_str()).unwrap_or("the task"))
            })
        }
        Cmd::Hours { on, off, start, end, days, today_until, alert_every } => {
            let mut b = json!({});
            if on {
                b["on"] = json!(true);
            }
            if off {
                b["on"] = json!(false);
            }
            if let Some(x) = start {
                b["start"] = json!(x);
            }
            if let Some(x) = end {
                b["end"] = json!(x);
            }
            if let Some(x) = days {
                b["days"] = json!(x);
            }
            if let Some(x) = today_until {
                b["today_until"] = json!(x);
            }
            if let Some(x) = alert_every {
                b["alert_every_mins"] = json!(x);
            }
            let v = if b.as_object().map(|o| o.is_empty()).unwrap_or(true) { c.call("GET", "/hours", None)? } else { c.call("POST", "/hours", Some(b))? };
            out(v["line"].as_str().unwrap_or(""));
            if alert_every.is_some() {
                out(v["alerts_line"].as_str().unwrap_or(""));
            }
            Ok(0)
        }
        Cmd::KeepAwake { on, off, mode, day, min_battery, linger, today } => {
            let mut b = json!({});
            if on || off {
                b["enabled"] = json!(on);
            }
            if let Some(x) = mode {
                b["mode"] = json!(x.replace('-', "_"));
            }
            if let Some(x) = today {
                b["today"] = json!(x);
            }
            if !day.is_empty() {
                let mut hours = serde_json::Map::new();
                for d in &day {
                    let Some((k, v)) = d.split_once('=') else {
                        return Err(format!("--day {d}: give a day and its hours, like fri=9am-3pm or sat=off").into());
                    };
                    let v = v.trim();
                    hours.insert(k.trim().to_lowercase(), if v.eq_ignore_ascii_case("default") { Value::Null } else { json!(v) });
                }
                b["hours"] = Value::Object(hours);
            }
            if let Some(x) = min_battery {
                b["min_battery"] = json!(x);
            }
            if let Some(x) = linger {
                b["linger_mins"] = json!(x);
            }
            let v = if b.as_object().map(|o| o.is_empty()).unwrap_or(true) { c.call("GET", "/keep-awake", None)? } else { c.call("POST", "/keep-awake", Some(b))? };
            out(v["line"].as_str().unwrap_or(""));
            let st = &v["settings"];
            if st["enabled"] == true {
                out(&format!("Hours: {}", v["schedule"].as_str().unwrap_or("")));
                if let Some(t) = v["today"]["line"].as_str() {
                    out(&format!("Today: {t}"));
                }
                let floor = match st["min_battery"].as_i64().unwrap_or(0) {
                    0 => "even on a low battery".to_string(),
                    n => format!("lets it sleep on battery below {n}%"),
                };
                out(&format!("{} · {floor}", if st["mode"] == "always" { "Always during the hours" } else { "Only while agents have work" }));
            }
            Ok(0)
        }
        Cmd::Pr { action } => match action {
            PrCmd::Status { task } => {
                let t = c.pr_task(task)?;
                let v = c.call("GET", &format!("/tasks/{t}/pr"), None)?;
                let pr = &v["pr"];
                out(&format!("{t} · PR #{} · {}", pr["num"], pr["url"].as_str().unwrap_or("")));
                if let Some(stage) = pr["stage"]["label"].as_str() {
                    out(&format!("Stage: {stage}"));
                }
                out(&format!("State: {} · checks: {} · review: {}", pr["state"].as_str().unwrap_or("?"), pr["checks"].as_str().unwrap_or("unknown"), pr["review"].as_str().unwrap_or("unknown")));
                let rec = &v["record"];
                if let Some(checks) = rec["checks"].as_array() {
                    for ch in checks {
                        out(&format!("  {} {}", if ch["state"] == "failed" { "✗" } else if ch["state"] == "running" { "…" } else { "✓" }, ch["name"].as_str().unwrap_or("")));
                    }
                }
                if rec["comments"].as_i64().unwrap_or(0) > 0 {
                    out(&format!("Review comments from others: {}", rec["comments"]));
                }
                if let (Some(b), Some(base)) = (rec["branch"].as_str(), rec["base"].as_str()) {
                    out(&format!("Branch {b} into {base}"));
                }
                if v["watched"] != true {
                    out("The board doesn't watch this PR's host; read it with the host's own tools.");
                }
                out(&format!("Merging: {}", if v["agents_merge"] == true { "agents may merge once it's approved and green" } else { "the owner merges it" }));
                Ok(0)
            }
            PrCmd::Wait { task } => {
                let t = c.pr_task(task)?;
                c.call("POST", &format!("/tasks/{t}/pr/wait"), Some(json!({})))?;
                out(&format!("The board watches {t}'s PR again and brings this conversation back when it needs you. You can stop here."));
                Ok(0)
            }
            PrCmd::SkipChecks { task, reason, all } => {
                let t = c.pr_task(task)?;
                let who = if c.session.is_empty() { "tb" } else { "The agent" };
                c.call("POST", &format!("/tasks/{t}/pr/skip-checks"), Some(json!({"reason": reason.unwrap_or_default(), "all": all, "who": who})))?;
                out(&format!("{t}'s PR checks count as passed{}.", if all { " on every push" } else { " for this push" }));
                Ok(0)
            }
            PrCmd::Merged { task } => {
                let t = c.pr_task(task)?;
                c.call("POST", &format!("/tasks/{t}/pr/merged"), Some(json!({"who": if c.session.is_empty() { "tb" } else { "The agent" }})))?;
                out(&format!("Recorded {t}'s PR as merged."));
                Ok(0)
            }
        },
        Cmd::Hooks { action } => hooks_cmd(c, action.unwrap_or(HooksCmd::List)),
        Cmd::Hello => {
            let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "tb".into());
            match c.report("tb.hello", json!({"tb_path": exe, "plugin_version": env!("CARGO_PKG_VERSION")}), None, TB_TIMEOUT)? {
                None => out(SAVED),
                Some(_) => out(&format!("The board knows tb is at {exe}.")),
            }
            Ok(0)
        }
        Cmd::Accounts => {
            let v = c.call("GET", "/accounts", None)?;
            for a in v["accounts"].as_array().cloned().unwrap_or_default() {
                let label = a["label"].as_str().unwrap_or("");
                if a["connected"] == true {
                    let scopes = a["scopes"].as_array().map(|s| s.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                    out(&format!("{label}: {}{}", a["detail"].as_str().unwrap_or("connected"), if scopes.is_empty() { String::new() } else { format!(" ({scopes})") }));
                } else {
                    out(&format!("{label}: not connected"));
                }
                if let Some(e) = a["error"].as_str() {
                    out(&format!("  ! {e}"));
                }
            }
            out("Connect or change them in Taskboard ▸ Settings ▸ Accounts (the owner does this).");
            Ok(0)
        }
        Cmd::Token { provider, user } => {
            let p = Provider::parse(&provider).ok_or("unknown account")?;
            let (who, secret) = accounts::credentials(&c.cfg, p).ok_or_else(|| format!("{} isn't connected. Ask {} to connect it in Taskboard ▸ Settings.", p.label(), c.cfg.owner))?;
            out(if user { &who } else { &secret });
            Ok(0)
        }
        Cmd::Api { provider, path, method, data } => api_call(&c.cfg, &provider, &path, method, data),
        Cmd::Hook { event } => Ok(hook::run(event.as_deref())),
        Cmd::GitCredential { op } => Ok(crate::gitcred::run(&op)),
        Cmd::Statusline { pass } => Ok(hook::statusline(pass)),
    }
}

fn project_line(p: &Value) -> String {
    let remote = match p["remote"].as_bool() {
        Some(true) => "git remote",
        Some(false) => "no git remote",
        None => "no folder",
    };
    format!(
        "{} · PR flow {} · {} · {remote}",
        p["name"].as_str().unwrap_or(""),
        p["pr_flow"].as_str().unwrap_or("auto"),
        if p["ships_prs"] == true { "work ends in PRs" } else { "no PRs" }
    )
}

fn project_cmd(c: &Ctx, action: ProjectCmd) -> Result<i32, String> {
    match action {
        ProjectCmd::Show { name } => {
            let v = c.call("GET", "/projects", None)?;
            let list = v["projects"].as_array().cloned().unwrap_or_default();
            let shown: Vec<&Value> = list.iter().filter(|p| name.as_deref().is_none_or(|n| p["name"].as_str() == Some(n))).collect();
            if shown.is_empty() {
                return Err(match name {
                    Some(n) => format!("There's no project called {n}."),
                    None => "There are no projects yet.".into(),
                });
            }
            for p in shown {
                out(&project_line(p));
            }
            Ok(0)
        }
        ProjectCmd::Set { name, pr_flow } => {
            let Some(flow) = pr_flow else {
                return Err(format!("say what to change, for example: tb project set {name} --pr-flow off"));
            };
            let v = c.call("POST", &format!("/projects/{name}"), Some(json!({"pr_flow": flow})))?;
            out(&format!("Changed {}.", project_line(&v)));
            Ok(0)
        }
    }
}

fn hooks_cmd(c: &Ctx, action: HooksCmd) -> Result<i32, String> {
    let file = hooks::load(&c.cfg)?;
    match action {
        HooksCmd::List => {
            out(&format!("Hooks file: {}", hooks::path(&c.cfg).display()));
            out("A step's hooks run before it and may stop or skip it, unless it says otherwise. [after] events are announced once they happened.\n");
            for e in hooks::EVENTS {
                out(&format!("{:<15} {}{}", e.name, e.when, if e.before { "" } else { " [after]" }));
                if e.before && !e.no_stop.is_empty() {
                    out(&format!("                  can't stop: {}", e.no_stop));
                }
                if e.before && !e.no_skip.is_empty() {
                    out(&format!("                  can't skip: {}", e.no_skip));
                }
                for g in file.hooks.get(e.name).into_iter().flatten() {
                    let on = if g.matcher.trim().is_empty() || g.matcher.trim() == "*" { "every project".to_string() } else { format!("projects {}", g.matcher) };
                    for h in &g.hooks {
                        out(&format!("  → {} ({on})", h.command));
                    }
                }
            }
            Ok(0)
        }
        HooksCmd::Test { event, task } => {
            let task = task.map(|t| task_ref(&t)).transpose()?;
            let mut path = format!("/hooks/payload?event={event}");
            if let Some(t) = &task {
                path += &format!("&task={t}");
            }
            let input = c.call("GET", &path, None)?;
            let t = &input["task"];
            let list = hooks::matching(&file, &event, t["project"].as_str().unwrap_or(""));
            if list.is_empty() {
                out(&format!("No hooks on {event} for {} ({}).", t["ref"].as_str().unwrap_or(""), t["project"].as_str().unwrap_or("")));
                return Ok(0);
            }
            let cwd = t["repo_path"].as_str().map(std::path::PathBuf::from).filter(|p| p.is_dir());
            let mut failed = false;
            for h in list {
                let r = hooks::run(&h, &event, &input, cwd.as_deref(), &hooks::log_path(&c.cfg));
                out(&format!("{}: {}", h.command, r.line()));
                if r.decision().is_some() && !input["can"]["block"].as_bool().unwrap_or(false) && !input["can"]["skip"].as_bool().unwrap_or(false) {
                    out(&format!("  ({event} can't be stopped or skipped, so the board would ignore that)"));
                }
                for l in r.stdout.lines().chain(r.stderr.lines()) {
                    out(&format!("  {l}"));
                }
                failed |= !r.ok();
            }
            Ok(if failed { 1 } else { 0 })
        }
        HooksCmd::Log { n } => {
            let text = std::fs::read_to_string(hooks::log_path(&c.cfg)).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            if lines.is_empty() {
                out("No hook has run yet.");
            }
            for l in &lines[lines.len().saturating_sub(n)..] {
                let v: Value = serde_json::from_str(l).unwrap_or_default();
                let res = match (v["error"].as_str(), v["code"].as_i64()) {
                    _ if v["decision"].is_object() => format!("asked to {}: {}", v["decision"]["decision"].as_str().unwrap_or(""), v["decision"]["reason"].as_str().unwrap_or("")),
                    (Some(e), _) => e.to_string(),
                    (None, Some(0)) => "ok".into(),
                    (None, code) => format!("exited {}", code.map(|c| c.to_string()).unwrap_or_else(|| "on a signal".into())),
                };
                out(&format!("{} {} {} {}: {res}", v["at"].as_str().unwrap_or(""), v["event"].as_str().unwrap_or(""), v["task"].as_str().unwrap_or(""), v["command"].as_str().unwrap_or("")));
            }
            Ok(0)
        }
    }
}

/// `tb api`: one authenticated request; prints the answer's body.
fn api_call(cfg: &Config, provider: &str, path: &str, method: Option<String>, data: Option<String>) -> Result<i32, String> {
    let p = Provider::parse(provider).ok_or("unknown account")?;
    let (who, secret) = accounts::credentials(cfg, p).ok_or_else(|| format!("{} isn't connected. Ask {} to connect it in Taskboard ▸ Settings.", p.label(), cfg.owner))?;
    let base = match p {
        Provider::Github => "https://api.github.com/",
        Provider::Bitbucket => "https://api.bitbucket.org/2.0/",
        Provider::Slack => "https://slack.com/api/",
    };
    let url = if path.starts_with("https://") { path.to_string() } else { format!("{base}{}", path.trim_start_matches('/')) };
    if !url.starts_with(base) {
        return Err(format!("{} requests go to {base}", p.label()));
    }
    let method = method.map(|m| m.to_uppercase()).unwrap_or_else(|| if data.is_some() { "POST".into() } else { "GET".into() });
    let auth = match p {
        Provider::Bitbucket => format!("Basic {}", basic(&who, &secret)),
        _ => format!("Bearer {secret}"),
    };
    let req = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(30)).build().request(&method, &url).set("Authorization", &auth).set("Accept", "application/json");
    let resp = match data {
        Some(d) => {
            serde_json::from_str::<Value>(&d).map_err(|e| format!("--data isn't JSON: {e}"))?;
            req.set("Content-Type", "application/json; charset=utf-8").send_string(&d)
        }
        None => req.call(),
    };
    match resp {
        Ok(r) => {
            out(&r.into_string().unwrap_or_default());
            Ok(0)
        }
        Err(ureq::Error::Status(code, r)) => {
            eprintln!("tb: {} answered {code}", p.label());
            out(&r.into_string().unwrap_or_default());
            Ok(1)
        }
        Err(e) => Err(format!("couldn't reach {}: {e}", p.label())),
    }
}

fn basic(user: &str, secret: &str) -> String {
    taskboardd::jira::base64_lite::encode(format!("{user}:{secret}").as_bytes())
}

pub fn main_with(args: Vec<String>) -> i32 {
    if args.get(1).map(|a| a == "hook").unwrap_or(false) {
        return hook::run(args.get(2).map(|s| s.as_str()));
    }
    if args.get(1).map(|a| a == "statusline").unwrap_or(false) {
        return hook::statusline(args.iter().any(|a| a == "--pass"));
    }
    let cli = match Cli::try_parse_from(&args) {
        Ok(c) => c,
        Err(e) => {
            let code = if e.use_stderr() { 2 } else { 0 };
            let _ = e.print();
            return code;
        }
    };
    let ctx = Ctx::new();
    match run_cmd(&ctx, cli.cmd) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("tb: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs() {
        assert_eq!(task_ref("t12").unwrap(), "T12");
        assert_eq!(task_ref("12").unwrap(), "T12");
        assert!(task_ref("G3").is_err());
        assert_eq!(goal_ref("3").unwrap(), "G3");
    }

    #[test]
    fn parses_commands() {
        assert!(Cli::try_parse_from(["tb", "checkpoint", "--done", "a", "--next", "b", "--decision", "c"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "found", "x", "--kind", "nope"]).is_err());
        assert!(Cli::try_parse_from(["tb", "task", "new", "x", "--goal", "G1", "--also", "G2", "--also", "G3"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--also", "G2", "--not-also", "G3"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "new", "x", "--detail", "y", "--here"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--lock", "local-core,emulator", "--alone"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--alone", "board"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--alone", "nope"]).is_err());
        assert!(Cli::try_parse_from(["tb", "task", "new", "x", "--goal", "G1", "--waits-for", "T2", "--lock", "a", "--alone"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "set", "G1", "--worktrees", "origin/main"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "locks"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "qa", "task", "Q3", "--note", "do it", "--no-pr"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "qa", "waiting"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "wave", "G1", "2", "--name", "API", "--stop", "on"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--wave", "none"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "backlog", "set", "B3", "--title", "x"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "set", "G1", "--paused", "on"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "setup", "G1", "Run make bootstrap in {task}'s worktree"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "set", "G1", "--run"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "set", "G1", "--deprioritize"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "set", "G1", "--deprioritize", "--prioritize"]).is_err());
        assert!(Cli::try_parse_from(["tb", "project", "show"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "project", "set", "web", "--pr-flow", "off"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "project", "set", "web", "--pr-flow", "maybe"]).is_err());
        assert!(Cli::try_parse_from(["tb", "backlog", "task", "B3", "--board"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "backlog", "ticket", "B3"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "backlog", "drop", "B3", "--reason", "dupe"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "backlog", "reopen", "B3"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "hours", "--alert-every", "10"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "alert", "raise", "main is red", "--urgent", "--key", "main:web"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "alert", "clear", "main:web"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "pr", "status"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "token", "bitbucket", "--user"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "token", "jira"]).is_err());
        assert!(Cli::try_parse_from(["tb", "jira"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "jira", "J12", "ok", "key=PROJ-1", "status=To Do", "found=yes"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "jira", "J12", "fail", "no", "such", "project"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "jira", "J12", "maybe"]).is_err());
        assert!(Cli::try_parse_from(["tb", "task", "new", "Fix it", "--jira", "none"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "api", "bitbucket", "user", "-X", "get"]).is_ok());
    }

    #[test]
    fn lock_args() {
        assert_eq!(lock_arg(&["local-core,emulator-5554".into()]), vec!["local-core", "emulator-5554"]);
        assert_eq!(lock_arg(&["a".into(), "b c".into()]), vec!["a", "b", "c"]);
        assert!(lock_arg(&["none".into()]).is_empty());
    }

    #[test]
    fn titles_stay_short() {
        assert_eq!(short_title("  Fix the\n login  flicker ").unwrap(), "Fix the login flicker");
        assert!(short_title(&"x".repeat(TITLE_MAX)).is_ok());
        let e = short_title(&"x".repeat(TITLE_MAX + 1)).unwrap_err();
        assert!(e.contains("--detail"), "{e}");
        assert!(short_task_titles(&["Short::".to_string() + &"long detail ".repeat(20)]).is_ok());
        assert!(short_task_titles(&["y".repeat(TITLE_MAX + 1)]).is_err());
    }

    #[test]
    fn basic_auth() {
        assert_eq!(basic("a@b.co", "tok"), "YUBiLmNvOnRvaw==");
        assert_eq!(basic("ab", "c"), "YWI6Yw==");
    }
}
