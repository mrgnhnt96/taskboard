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
/// `tb done --pr-body`: the board fetches, checks the branch and opens the PR before it answers.
const PR_OPEN_TIMEOUT: f64 = 300.0;
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
        #[arg(long, conflicts_with_all = ["pr_body", "no_pr"])]
        pr: Option<String>,
        /// Let the board open the PR: a file with its description (- for stdin), checked first
        #[arg(long = "pr-body", value_name = "FILE", conflicts_with = "no_pr")]
        pr_body: Option<String>,
        /// The PR's title with --pr-body (default: the task's title, after its ticket key)
        #[arg(long = "title", requires = "pr_body")]
        pr_title: Option<String>,
        /// It finishes without the PR it was meant to open, and why
        #[arg(long = "no-pr", value_name = "WHY")]
        no_pr: Option<String>,
        /// It finishes without evidence (screenshots, results), and why
        #[arg(long = "no-evidence", value_name = "WHY")]
        no_evidence: Option<String>,
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
        /// A planned task: "title::detail", "title::detail::<wave>", "title::detail::<wave or nothing>::<T14 #1, the tasks it waits for>"
        /// or with "::<src/a.rs, src/b.rs>" after the waits, the files the wave plans for it (its wave mates are told)
        #[arg(long = "task", value_name = "TITLE::DETAIL[::WAVE][::WAITS][::FILES]")]
        tasks: Vec<String>,
    },
    /// Named locks, who holds each, and tasks that run alone
    Locks,
    /// The device pool: each device, its tags, who has it, and the tasks waiting for one
    Devices,
    /// Add, change, remove or focus a device in the pool
    Device {
        #[command(subcommand)]
        action: DeviceCmd,
    },
    /// Bits (feature flags): each one, local or backend, whether it's made, and what uses it
    Bits {
        #[arg(long)]
        goal: Option<String>,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Add, change or remove a bit, or record it made in the flag tool
    Bit {
        #[command(subcommand)]
        action: BitCmd,
    },
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
    /// Show or change the context limits: the compact window, when a cold conversation is compacted,
    /// how big and how fresh a conversation must be to resume, and the generated-file globs
    Limits {
        /// Tokens at which Claude compacts a board terminal's conversation (0 = off)
        #[arg(long = "compact-window")]
        compact_window: Option<i64>,
        /// Minutes idle after which a conversation is compacted before it resumes (0 = off)
        #[arg(long = "cold-idle-mins")]
        cold_idle_mins: Option<i64>,
        /// A task or PR conversation resumes only under this many tokens, else starts fresh (0 = off)
        #[arg(long = "warm-tokens")]
        warm_tokens: Option<i64>,
        /// …and only when idle under this many minutes (0 = off)
        #[arg(long = "warm-idle-mins")]
        warm_idle_mins: Option<i64>,
        /// Generated-file globs kept as `-diff` in .git/info/attributes, comma-separated (none clears them)
        #[arg(long)]
        generated: Option<String>,
        /// With --generated: that project's own globs, on top of everyone's
        #[arg(long)]
        project: Option<String>,
        /// Forget every change and go back to config.toml's [limits]
        #[arg(long)]
        reset: bool,
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
enum DeviceCmd {
    /// Add a device to the pool
    Add {
        name: String,
        /// What it is, for tasks to ask by: android, ios, tablet… Repeat for more
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// A shell command that raises its window ({name} is the device's name)
        #[arg(long)]
        focus: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },
    /// Change a device
    Set {
        name: String,
        #[arg(long = "name")]
        rename: Option<String>,
        /// Its tags (replaces them); none for none
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Its focus command, or none
        #[arg(long)]
        focus: Option<String>,
        #[arg(long)]
        note: Option<String>,
        /// Switch it off: no task is lent it
        #[arg(long, conflicts_with = "on")]
        off: bool,
        /// Switch it back on
        #[arg(long)]
        on: bool,
    },
    /// Take a device out of the pool
    Remove { name: String },
    /// Raise the device's window (runs its focus command)
    Focus { name: String },
}

#[derive(Subcommand)]
enum BitCmd {
    /// Add a bit: --backend (it has to be made in the flag tool) or --local (in the code only)
    Add {
        name: String,
        #[arg(long, conflicts_with = "local")]
        backend: bool,
        #[arg(long)]
        local: bool,
        /// A task whose work sits behind it; repeat for more
        #[arg(long = "task")]
        tasks: Vec<String>,
        /// A goal it's for; repeat for more
        #[arg(long = "goal")]
        goals: Vec<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },
    /// The owner made the bit in the flag tool (--undo: it isn't after all). Only on the owner's word
    Made {
        name: String,
        #[arg(long)]
        undo: bool,
    },
    /// Change a bit
    Set {
        name: String,
        #[arg(long = "name")]
        rename: Option<String>,
        #[arg(long, conflicts_with = "local")]
        backend: bool,
        #[arg(long)]
        local: bool,
        #[arg(long = "task")]
        tasks: Vec<String>,
        #[arg(long = "not-task")]
        not_tasks: Vec<String>,
        #[arg(long = "goal")]
        goals: Vec<String>,
        #[arg(long = "not-goal")]
        not_goals: Vec<String>,
        #[arg(long)]
        note: Option<String>,
    },
    /// Remove a bit
    Remove { name: String },
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
    /// Run another round of a step's check or script, on this commit or a new one
    Again {
        name: String,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Answer one finding of a step's last round: tb step triage "Review" F2 --state fixed --commit abc123
    Triage {
        name: String,
        /// The finding's id (tb steps lists them)
        finding: String,
        #[arg(long, value_parser = ["open", "fixed", "answered", "dismissed"])]
        state: String,
        /// Why, or what you did
        #[arg(long)]
        note: Option<String>,
        /// The commit that deals with it (points the next round at it)
        #[arg(long)]
        commit: Option<String>,
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
        /// Devices each of its tasks asks for (unless the task asks for its own): android, android:2. Repeat for more, none for none
        #[arg(long = "device", value_name = "TAG[:N]|none")]
        devices: Vec<String>,
    },
    #[command(about = setup_about())]
    Setup { goal: String, text: String },
    /// Name a wave, or hold it: none of its tasks start, nor any later wave, until it's continued.
    /// (A review stop after a wave is the owner's own checkbox in the app.)
    Wave {
        goal: String,
        wave: i64,
        #[arg(long)]
        name: Option<String>,
        /// Hold the wave (off: let it start)
        #[arg(long, num_args = 0..=1, default_missing_value = "on", value_parser = ["on", "off"])]
        hold: Option<String>,
    },
    /// Let the goal go on past a wave it stopped at (a review stop or a failed task), when the owner says so; or let a held wave start
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
        /// A file the wave plans for it, so its wave mates leave it alone (needs --goal); repeat for more
        #[arg(long = "file", value_name = "PATH")]
        files: Vec<String>,
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
        /// Devices from the pool it needs while it runs, by tag or name: android, android:2. Repeat for more,
        /// none for none (even when its goal asks for some)
        #[arg(long = "device", value_name = "TAG[:N]|none")]
        devices: Vec<String>,
        /// A bit (feature flag) its work sits behind; it waits until a backend bit is made. Repeat for more
        #[arg(long = "bit", value_name = "NAME")]
        bits: Vec<String>,
        /// Its PR builds on this task's PR (any goal): it starts once that's done, from its branch
        #[arg(long = "stack-on", value_name = "T12")]
        stack_on: Option<String>,
        /// It ends in a PR, whatever its project does
        #[arg(long, conflicts_with = "no_pr")]
        pr: bool,
        /// It ends without a PR
        #[arg(long = "no-pr")]
        no_pr: bool,
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
        /// Devices from the pool it needs while it runs: android, android:2. Repeat for more, none for none
        /// (even when its goal asks for some), goal for its goal's
        #[arg(long = "device", value_name = "TAG[:N]|none|goal")]
        devices: Vec<String>,
        /// A bit (feature flag) its work sits behind. Repeat for more, none for none
        #[arg(long = "bit", value_name = "NAME|none")]
        bits: Vec<String>,
        /// A bit it no longer uses
        #[arg(long = "not-bit")]
        not_bits: Vec<String>,
        /// Its PR builds on this task's PR (any goal), or none
        #[arg(long = "stack-on", value_name = "T12|none")]
        stack_on: Option<String>,
        /// Whether it ends in a PR: yes, no, or auto (its project's default)
        #[arg(long, value_parser = ["yes", "no", "auto"])]
        pr: Option<String>,
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
    /// Make backlog issues into tasks: planned in their goal, or with --board queued on the board.
    /// Several issues (tb backlog task B4 B5) change together or not at all.
    Task {
        #[arg(required = true)]
        issues: Vec<String>,
        #[arg(long)]
        board: bool,
    },
    /// Ask Jira for a ticket for each backlog issue, all or none
    Ticket {
        #[arg(required = true)]
        issues: Vec<String>,
    },
    /// Close backlog issues as won't do, all or none (only when the owner says so)
    Drop {
        #[arg(required = true)]
        issues: Vec<String>,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Open closed backlog issues again, all or none
    Reopen {
        #[arg(required = true)]
        issues: Vec<String>,
    },
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
    /// Check a PR description against the rules tb done --pr-body uses ([pr_body] in config.toml)
    BodyCheck {
        /// The file (- for stdin)
        file: String,
    },
    /// Count the PR's checks as passed (e.g. a hook cancelled the builds), so it moves on to review
    SkipChecks {
        task: Option<String>,
        #[arg(long)]
        reason: Option<String>,
        /// Every later push too, not just the current one
        #[arg(long)]
        all: bool,
    },
    /// Reply to a review thread: [T<n>] <thread> "<what you did>" (ids from tb pr status)
    Reply {
        /// [task] thread text
        #[arg(num_args = 2..=3, required = true)]
        args: Vec<String>,
        /// Resolve the thread too
        #[arg(long)]
        resolve: bool,
    },
    /// Resolve a thread that asks for nothing, without replying: [T<n>] <thread>
    Ack {
        /// [task] thread
        #[arg(num_args = 1..=2, required = true)]
        args: Vec<String>,
    },
    /// Every thread is answered: ask the reviewers who wanted changes to look again
    Addressed { task: Option<String> },
    /// Merge it once it's approved, green and every thread is answered (the board checks first)
    Merge { task: Option<String> },
    /// A failed check isn't this PR's fault: clear it for this push, with proof
    NotOurs {
        task: Option<String>,
        /// Why it isn't this PR (20-300 characters)
        #[arg(long)]
        reason: String,
        /// What fails, in a few words (80 characters at most)
        #[arg(long)]
        title: String,
        /// A link showing the same failure without this PR (repeat for more)
        #[arg(long, required = true)]
        proof: Vec<String>,
        /// The failed check to clear (repeat for more; all of this push's failures when left out)
        #[arg(long)]
        check: Vec<String>,
    },
}

/// Splits `[T<n>] rest…` into the task (if named) and the rest.
fn task_and(args: Vec<String>, rest: usize) -> (Option<String>, Vec<String>) {
    if args.len() > rest {
        let mut it = args.into_iter();
        let t = it.next();
        (t, it.collect())
    } else {
        (None, args)
    }
}

fn mark(state: &str) -> &'static str {
    match state {
        "failed" => "✗",
        "running" => "…",
        "stopped" => "■",
        _ => "✓",
    }
}

fn print_pr_status(t: &str, v: &Value) {
    let pr = &v["pr"];
    let live = &v["live"];
    out(&format!("{t} · PR #{} · {}", pr["num"], pr["url"].as_str().unwrap_or("")));
    if let Some(stage) = pr["stage"]["label"].as_str() {
        out(&format!("Stage: {stage}"));
    }
    out(&format!("State: {} · checks: {} · review: {}", pr["state"].as_str().unwrap_or("?"), pr["checks"].as_str().unwrap_or("unknown"), pr["review"].as_str().unwrap_or("unknown")));
    if let Some(e) = live["read_error"].as_str() {
        out(&format!("Couldn't read it just now ({e}); this is the last read."));
    }
    let rec = &v["record"];
    if let (Some(b), Some(base)) = (rec["branch"].as_str(), rec["base"].as_str()) {
        let moved = if live["base_moved"] == true { format!(" · {base} has moved since this push: rebase onto it") } else { String::new() };
        out(&format!("Branch {b} into {base}{moved}"));
    }
    if let Some(l) = pr["bar"]["stacks_on"]["line"].as_str() {
        out(l);
    }
    if let Some(n) = live["builds_note"].as_str() {
        out(n);
    }
    let failures = live["failures"].as_array().cloned().unwrap_or_default();
    if let Some(checks) = rec["checks"].as_array() {
        if !checks.is_empty() {
            out("Checks:");
        }
        for ch in checks {
            let name = ch["name"].as_str().unwrap_or("");
            let f = failures.iter().find(|f| f["check"].as_str() == Some(name));
            let mut line = format!("  {} {name}", mark(ch["state"].as_str().unwrap_or("")));
            if let Some(f) = f {
                if f["base_fails"] == true {
                    line += " [base fails this too]";
                }
                if f["cleared"] == true {
                    line += " [not this PR's]";
                }
            }
            if let Some(u) = ch["url"].as_str() {
                line += &format!(" · {u}");
            }
            out(&line);
            if let Some(f) = f {
                for s in f["steps"].as_array().cloned().unwrap_or_default() {
                    out(&format!("      step: {}", s.as_str().unwrap_or("")));
                }
                for s in f["tests"].as_array().cloned().unwrap_or_default() {
                    out(&format!("      test: {}", s.as_str().unwrap_or("")));
                }
                if let Some(e) = f["error"].as_str() {
                    out(&format!("      couldn't read its steps: {e}"));
                }
            }
        }
    }
    if let Some(e) = live["base_error"].as_str() {
        out(&format!("Couldn't compare with the base branch: {e}"));
    }
    if let Some(m) = live["expected_missing"].as_array().filter(|m| !m.is_empty()) {
        out(&format!("Expected checks not posted yet: {}", m.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")));
    }
    if live["not_ours"].is_object() {
        let n = &live["not_ours"];
        out(&format!("Not this PR's: {} — {}", n["title"].as_str().unwrap_or(""), n["reason"].as_str().unwrap_or("")));
    }
    let reviewers = live["reviewers"].as_array().cloned().unwrap_or_default();
    if !reviewers.is_empty() {
        let need = live["approvals"]["need"].as_i64().map(|n| format!(" · {} of {n} approvals", live["approvals"]["have"])).unwrap_or_default();
        out(&format!("Reviewers{need}:"));
        for r in reviewers {
            let name = r["name"].as_str().filter(|n| !n.is_empty()).or(r["user"].as_str()).unwrap_or("");
            let state = match r["state"].as_str().unwrap_or("") {
                "approved" => "approved",
                "changes" if r["requested"] == true => "asked for changes, asked to look again",
                "changes" => "asked for changes",
                "commented" => "commented",
                _ => "hasn't reviewed yet",
            };
            out(&format!("  {name}: {state}{}", if r["swapped_off"] == true { " (swapped off)" } else { "" }));
        }
    }
    let open = live["open_threads"].as_array().cloned().unwrap_or_default();
    if !open.is_empty() {
        out(&format!("Open threads ({}):", open.len()));
        for th in open {
            let who = th["author_name"].as_str().filter(|n| !n.is_empty()).or(th["author"].as_str()).unwrap_or("");
            let at = th["path"].as_str().map(|p| format!(" · {p}{}", th["line"].as_i64().map(|l| format!(":{l}")).unwrap_or_default())).unwrap_or_default();
            let kind = if th["kind"] == "task" { " · task" } else { "" };
            out(&format!("  {} · {who}{kind}{at} · {}", th["id"].as_str().unwrap_or(""), th["text"].as_str().unwrap_or("")));
        }
    } else if rec["comments"].as_i64().unwrap_or(0) > 0 && !rec["threads"].is_array() {
        out(&format!("Review comments from others: {}", rec["comments"]));
    }
    if v["watched"] != true {
        out("The board doesn't watch this PR's host; read it with the host's own tools.");
    }
    if let Some(b) = live["blockers"].as_array().filter(|b| !b.is_empty()) {
        out(&format!("Before it can merge: {}", b.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("; ")));
    }
    out(&format!("Merging: {}", if v["agents_merge"] == true { "agents may merge once it's approved and green (tb pr merge)" } else { "the owner merges it" }));
}

fn plural_threads(v: &Value) -> String {
    match v["open_threads"].as_i64().unwrap_or(0) {
        0 => "No threads are".to_string(),
        1 => "1 thread is still".to_string(),
        n => format!("{n} threads are still"),
    }
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

/// `tb goal setup`'s help. Clap turns every `{n}` in help into a line break, so each placeholder's name is set
/// in bold, which keeps the braces apart from it until the styling is drawn (or stripped).
fn setup_about() -> String {
    let b = clap::builder::styling::Style::new().bold();
    let names: Vec<String> = ["task", "n", "wave", "goal"].iter().map(|n| format!("{{{b}{n}{b:#}}}")).collect();
    format!("What every task in the goal does first (its handoff shows it): {} are filled in; none clears it", names.join(" "))
}

/// Backlog refs in the order given, each once.
fn issue_refs(v: &[String]) -> Result<Vec<String>, String> {
    let mut refs: Vec<String> = vec![];
    for x in v {
        let r = issue_ref(x)?;
        if !refs.contains(&r) {
            refs.push(r);
        }
    }
    Ok(refs)
}

/// One all-or-nothing backlog action over several issues, through `/backlog/bulk`, credited to this terminal.
fn backlog_bulk(c: &Ctx, action: &str, issues: &[String], extra: Value) -> Result<Value, String> {
    let mut b = json!({"action": action, "ids": issue_refs(issues)?, "who": c.who()});
    for (k, v) in extra.as_object().into_iter().flatten() {
        b[k] = v.clone();
    }
    c.call("POST", "/backlog/bulk", Some(b))
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

    /// Who a PR command is from, for the task's log.
    fn pr_who(&self) -> &'static str {
        if self.session.is_empty() { "tb" } else { "The agent" }
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

/// A task's `--device`: needs, `none` for its own "needs none", or `goal` for its goal's.
fn device_arg(values: &[String]) -> Value {
    match values {
        [one] if one.trim().eq_ignore_ascii_case("goal") => json!("goal"),
        [one] if one.trim().eq_ignore_ascii_case("none") => json!("none"),
        _ => json!(lock_arg(values)),
    }
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

/// "pixel-7 · android, phone · lent to T4" for `tb devices`.
fn device_line(d: &Value) -> String {
    let tags: Vec<&str> = d["tags"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    let mut line = d["name"].as_str().unwrap_or("").to_string();
    if !tags.is_empty() {
        line += &format!(" · {}", tags.join(", "));
    }
    line += &match d["held_by"]["ref"].as_str() {
        Some(r) => format!(" · lent to {r} {}", d["held_by"]["title"].as_str().unwrap_or("")),
        None if d["off"] == true => " · off".to_string(),
        None => " · free".to_string(),
    };
    if let Some(n) = d["note"].as_str().filter(|n| !n.is_empty()) {
        line += &format!(" · {n}");
    }
    if d["can_focus"] == true {
        line += " · can focus";
    }
    line
}

/// "newCheckout · backend · not made in Flagsmith · T4, G2" for `tb bits`.
fn bit_line(b: &Value, tool: &str) -> String {
    let kind = b["kind"].as_str().unwrap_or("");
    let state = match (kind, b["made"] == true) {
        ("local", _) => format!("in the code only, not in {tool}"),
        (_, true) => format!("made in {tool}"),
        _ => format!("not made in {tool} yet"),
    };
    let uses: Vec<&str> = ["tasks", "goals"].iter().flat_map(|k| b[*k].as_array().into_iter().flatten().filter_map(|x| x.as_str())).collect();
    let mut line = format!("{} · {kind} · {state}", b["name"].as_str().unwrap_or(""));
    if !uses.is_empty() {
        line += &format!(" · {}", uses.join(", "));
    }
    if let Some(n) = b["note"].as_str().filter(|n| !n.is_empty()) {
        line += &format!(" · {n}");
    }
    line
}

fn device_cmd(c: &Ctx, action: DeviceCmd) -> Result<i32, String> {
    match action {
        DeviceCmd::Add { name, tags, focus, note } => {
            let v = c.call("POST", "/devices", Some(json!({"name": name, "tags": lock_arg(&tags), "focus": focus, "note": note})))?;
            out(&format!("Added {}.", device_line(&v)));
        }
        DeviceCmd::Set { name, rename, tags, focus, note, off, on } => {
            let mut b = json!({});
            if let Some(x) = rename {
                b["name"] = json!(x);
            }
            if !tags.is_empty() {
                b["tags"] = json!(lock_arg(&tags));
            }
            if let Some(x) = focus {
                b["focus"] = json!(x);
            }
            if let Some(x) = note {
                b["note"] = json!(x);
            }
            if off || on {
                b["off"] = json!(off);
            }
            if b.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                return Err("say what to change, for example: tb device set pixel-7 --tag android --tag phone".into());
            }
            let v = c.call("POST", &format!("/devices/{name}"), Some(b))?;
            out(&format!("Changed {}.", device_line(&v)));
        }
        DeviceCmd::Remove { name } => {
            c.call("POST", &format!("/devices/{name}/remove"), Some(json!({})))?;
            out(&format!("Took {name} out of the pool."));
        }
        DeviceCmd::Focus { name } => {
            c.call("POST", &format!("/devices/{name}/focus"), Some(json!({})))?;
            out(&format!("Raising {name}'s window."));
        }
    }
    Ok(0)
}

fn bit_cmd(c: &Ctx, action: BitCmd) -> Result<i32, String> {
    let refs = |xs: &[String], f: fn(&str) -> Result<String, String>| xs.iter().map(|x| f(x)).collect::<Result<Vec<_>, _>>();
    let v = match action {
        BitCmd::Add { name, backend, local, tasks, goals, project, note } => {
            if !backend && !local {
                return Err("say which kind: --backend (it has to be made in the flag tool) or --local (in the code only)".into());
            }
            let v = c.call(
                "POST",
                "/bits",
                Some(json!({"name": name, "kind": if local { "local" } else { "backend" }, "tasks": refs(&tasks, task_ref)?,
                            "goals": refs(&goals, goal_ref)?, "project": project, "note": note, "who": c.who()})),
            )?;
            out(&format!("Added the bit {}.", v["name"].as_str().unwrap_or("")));
            v
        }
        BitCmd::Made { name, undo } => c.call("POST", &format!("/bits/{name}/made"), Some(json!({"undo": undo, "who": c.who()})))?,
        BitCmd::Set { name, rename, backend, local, tasks, not_tasks, goals, not_goals, note } => {
            let mut b = json!({"who": c.who()});
            if let Some(x) = rename {
                b["name"] = json!(x);
            }
            if backend || local {
                b["kind"] = json!(if local { "local" } else { "backend" });
            }
            for (k, xs, f) in [("tasks", &tasks, task_ref as fn(&str) -> Result<String, String>), ("not_tasks", &not_tasks, task_ref), ("goals", &goals, goal_ref), ("not_goals", &not_goals, goal_ref)] {
                if !xs.is_empty() {
                    b[k] = json!(refs(xs, f)?);
                }
            }
            if let Some(x) = note {
                b["note"] = json!(x);
            }
            if b.as_object().map(|o| o.len() <= 1).unwrap_or(true) {
                return Err("say what to change, for example: tb bit set newCheckout --task T12".into());
            }
            c.call("POST", &format!("/bits/{name}"), Some(b))?
        }
        BitCmd::Remove { name } => {
            c.call("POST", &format!("/bits/{name}/remove"), Some(json!({"who": c.who()})))?;
            out(&format!("Removed the bit {name}."));
            return Ok(0);
        }
    };
    let tool = c.call("GET", "/bits", None).ok().and_then(|l| l["tool"].as_str().map(|s| s.to_string())).unwrap_or_else(|| "the flag tool".into());
    out(&bit_line(&v, &tool));
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
            if let Some(d) = t["devices"]["lent"].as_array().filter(|a| !a.is_empty()) {
                line += &format!(" · has {}", d.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "));
            } else if let Some(n) = t["devices"]["needs_text"].as_str().filter(|n| !n.is_empty()) {
                line += &format!(" · needs {n}");
            }
            if let Some(bs) = t["bits"].as_array().filter(|a| !a.is_empty()) {
                line += &format!(" · bits {}", bs.iter().filter_map(|x| x["name"].as_str()).collect::<Vec<_>>().join(", "));
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
            if w["held"] == true {
                line += " · held";
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
    let head = local_head(&c.cwd).map(|h| format!("&head={h}")).unwrap_or_default();
    Ok(match task {
        Some(x) => format!("/steps?task={}{head}", task_ref(&x)?),
        None => format!("/steps?session={}{head}", c.session),
    })
}

/// This checkout's head commit, so per-head steps are judged on what's here.
pub fn local_head(cwd: &str) -> Option<String> {
    if cwd.is_empty() || !std::path::Path::new(cwd).is_dir() {
        return None;
    }
    let o = std::process::Command::new("git").args(["-C", cwd, "rev-parse", "HEAD"]).output().ok().filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|h| !h.is_empty())
}

/// A PR description from a file, or stdin for `-`.
fn read_body(file: &str) -> Result<String, String> {
    if file == "-" {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).map_err(|e| format!("couldn't read the description from stdin: {e}"))?;
        return Ok(s);
    }
    std::fs::read_to_string(file).map_err(|e| format!("couldn't read {file}: {e}"))
}

/// Refuses a round that comes too soon after the last (the step's `min_gap_mins`).
fn round_gap(v: &Value, name: &str) -> Result<(), String> {
    let at = v["steps"].as_array().into_iter().flatten().find(|s| s["name"].as_str().map(steps::key) == Some(steps::key(name))).and_then(|s| s["next_round_at"].as_str());
    match at {
        Some(at) => Err(format!(
            "“{name}” ran a round too recently; the next can start at {}. Work on what it found until then.",
            taskboardd::util::local_clock(Some(at))
        )),
        None => Ok(()),
    }
}

/// Runs a step's check or script with `$TASKBOARD_RESULT` set, and reads what it wrote there.
fn run_with_result(c: &Ctx, label: &str, script: &str, vars: &BTreeMap<String, String>, timeout: u64) -> (bool, String, Option<Value>) {
    let path = std::env::temp_dir().join(format!("tb-result-{}-{}.json", std::process::id(), taskboardd::util::now_ts() as u64));
    let _ = std::fs::remove_file(&path);
    let mut vars = vars.clone();
    vars.insert("result".into(), path.to_string_lossy().to_string());
    let (passed, output) = run_script(c, label, &steps::fill(script, &vars), &vars, timeout);
    let result = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).filter(|v| v.is_object());
    let _ = std::fs::remove_file(&path);
    let passed = match result.as_ref().and_then(|r| r["verdict"].as_str()) {
        Some("pass") | Some("skip") => true,
        Some("fail") => false,
        _ => passed,
    };
    (passed, output, result)
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
            let mut f = json!({"name": step.name, "note": note, "via": "done", "head": local_head(&c.cwd)});
            if !step.check.is_empty() && !step.owner {
                round_gap(&v, &step.name)?;
                let vars = step_vars(c, &v, &step.name);
                let (passed, output, result) = run_with_result(c, "Check", &step.check, &vars, step.timeout_secs());
                f["ok"] = json!(passed);
                f["output"] = json!(output);
                f["result"] = json!(result);
            }
            step_report(c, f, t.task.or_else(|| v["task"].as_str().map(|s| s.to_string())))
        }
        StepCmd::Run { name, t } => run_step(c, &name, t.task),
        StepCmd::Again { name, t } => {
            let (_, step) = find_step(c, t.task.clone(), &name)?;
            if step.owner || (step.run.is_empty() && step.check.is_empty()) {
                return Err(format!("“{}” has no check or script to run again: {}.", step.name, step.how("tb")));
            }
            if step.run.is_empty() {
                return step_cmd(c, StepCmd::Done { name, note: Some("Another round".into()), skip: false, t });
            }
            run_step(c, &name, t.task)
        }
        StepCmd::Triage { name, finding, state, note, commit, t } => {
            let (v, step) = find_step(c, t.task.clone(), &name)?;
            let task = t.task.or_else(|| v["task"].as_str().map(|s| s.to_string()));
            c.run_report("tb.step_triage", json!({"name": step.name, "finding": finding, "state": state, "note": note, "commit": commit}), task, true, |v| {
                let open = v["open"].as_i64().unwrap_or(0);
                format!(
                    "{} is {} on {}. {}",
                    v["finding"].as_str().unwrap_or("The finding"),
                    v["state"].as_str().unwrap_or(""),
                    v["step"].as_str().unwrap_or("the step"),
                    if open == 0 { "No open findings left.".to_string() } else { format!("{} still open.", if open == 1 { "1 finding is".to_string() } else { format!("{open} findings are") }) }
                )
            })
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

/// `tb step run`: a script step's script, then its check.
fn run_step(c: &Ctx, name: &str, task: Option<String>) -> Result<i32, String> {
    let (v, step) = find_step(c, task.clone(), name)?;
    if step.run.is_empty() {
        return Err(format!("“{}” has no script to run: {}.", step.name, step.how("tb")));
    }
    round_gap(&v, &step.name)?;
    let vars = step_vars(c, &v, &step.name);
    let (mut passed, mut output, mut result) = run_with_result(c, "Script", &step.run, &vars, step.timeout_secs());
    if passed && !step.check.is_empty() {
        let (p, o, r) = run_with_result(c, "Check", &step.check, &vars, step.timeout_secs());
        passed = p;
        output = format!("{output}\n{o}");
        result = r.or(result);
    }
    let f = json!({"name": step.name, "via": "run", "ok": passed, "output": output, "result": result, "head": local_head(&c.cwd)});
    step_report(c, f, task.or_else(|| v["task"].as_str().map(|s| s.to_string())))
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
            // The latest round of each step that ran: its headline and findings, to triage.
            if let Ok(d) = c.call("GET", &format!("/tasks/{task}"), None) {
                for r in d["step_results"].as_array().into_iter().flatten() {
                    let moved = if r["stale"] == true { " (the branch has moved since)" } else { "" };
                    out(&format!("{}: {}{moved}", r["name"].as_str().unwrap_or(""), r["headline"].as_str().unwrap_or("")));
                    for f in r["findings"].as_array().into_iter().flatten() {
                        let at = match (f["file"].as_str(), f["line"].as_i64()) {
                            (Some(file), Some(l)) => format!(" {file}:{l}"),
                            (Some(file), None) => format!(" {file}"),
                            _ => String::new(),
                        };
                        out(&format!(
                            "  {} [{}{}] {}{at}",
                            f["id"].as_str().unwrap_or("?"),
                            f["state"].as_str().filter(|s| !s.is_empty()).unwrap_or("open"),
                            f["severity"].as_str().map(|s| format!(", {s}")).unwrap_or_default(),
                            f["title"].as_str().unwrap_or("")
                        ));
                    }
                }
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
        Cmd::Done { summary, pr, pr_body, pr_title, no_pr, no_evidence, human, t } => {
            let mut f = json!({"summary": summary, "pr": pr, "human": human, "no_pr": no_pr, "no_evidence": no_evidence});
            let Some(file) = pr_body else {
                return c.run_report("tb.done", f, t.task, true, |v| format!("{} is done.", v["task"].as_str().unwrap_or("")));
            };
            let text = read_body(&file)?;
            let problems = taskboardd::propen::body_problems(&c.cfg.pr_body, &text);
            if !problems.is_empty() {
                return Err(format!("the PR description needs work first:\n- {}", problems.join("\n- ")));
            }
            f["pr_body"] = json!(text);
            f["pr_title"] = json!(pr_title);
            // Not spooled: the PR opens now or not at all.
            let task = t.task.map(|x| task_ref(&x)).transpose()?;
            let body = c.body("tb.done", f, task.as_deref());
            let v = match client::request(&c.cfg, "POST", "/report", Some(&body), PR_OPEN_TIMEOUT) {
                Ok(v) => v,
                Err(CallError::Refused(m)) => return Err(m),
                Err(CallError::Unreachable(e)) => return Err(format!("the board isn't answering ({e}), so it can't open the PR. Try again once it's back.")),
            };
            if !v["task"].is_string() {
                out(NO_TASK);
                return Ok(0);
            }
            let pr = v["pr_url"].as_str().map(|u| format!(" Its PR: {u}")).unwrap_or_default();
            out(&format!("{} is done.{pr}", v["task"].as_str().unwrap_or("")));
            Ok(0)
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
        Cmd::Devices => {
            let v = c.call("GET", "/devices", None)?;
            let devices = v["devices"].as_array().cloned().unwrap_or_default();
            if devices.is_empty() {
                out("The device pool is empty. Add one with tb device add NAME --tag android.");
            }
            for d in devices {
                out(&device_line(&d));
            }
            for w in v["waiting"].as_array().cloned().unwrap_or_default() {
                out(&format!("{} {} · needs {} — {}", w["ref"].as_str().unwrap_or(""), w["title"].as_str().unwrap_or(""), w["needs"].as_str().unwrap_or(""), w["why"].as_str().unwrap_or("")));
            }
            Ok(0)
        }
        Cmd::Device { action } => device_cmd(c, action),
        Cmd::Bits { goal, t } => {
            let mut path = "/bits".to_string();
            if let Some(g) = goal {
                path += &format!("?goal={}", goal_ref(&g)?);
            } else if let Some(x) = t.task {
                path += &format!("?task={}", task_ref(&x)?);
            }
            let v = c.call("GET", &path, None)?;
            let bits = v["bits"].as_array().cloned().unwrap_or_default();
            if bits.is_empty() {
                out("No bits yet. Add one with tb bit add NAME --backend (or --local) --task T12.");
            }
            for b in bits {
                out(&bit_line(&b, v["tool"].as_str().unwrap_or("the flag tool")));
            }
            Ok(0)
        }
        Cmd::Bit { action } => bit_cmd(c, action),
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
            GoalCmd::Set { goal, name, outcome, tldr, paused, in_order, max_terminals, epic, product, worktrees, run, deprioritize, prioritize, devices } => {
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
                if !devices.is_empty() {
                    b["devices"] = json!(lock_arg(&devices));
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
            GoalCmd::Wave { goal, wave, name, hold } => {
                let g = goal_ref(&goal)?;
                if name.is_none() && hold.is_none() {
                    return Err("say what to change: tb goal wave G3 2 --name \"API\" or --hold".into());
                }
                if let Some(n) = name {
                    c.call("POST", &format!("/goals/{g}/waves/{wave}"), Some(json!({"name": n})))?;
                    out(&format!("Named wave {wave} of {g}."));
                }
                if let Some(h) = hold {
                    c.call("POST", &format!("/goals/{g}/waves/{wave}/hold"), Some(json!({"on": h == "on", "who": c.who()})))?;
                    out(&if h == "on" {
                        format!("Holding wave {wave} of {g}: none of its tasks start until tb goal continue {g} {wave}.")
                    } else {
                        format!("Wave {wave} of {g} may start.")
                    });
                }
                Ok(0)
            }
            GoalCmd::Continue { goal, wave } => {
                let g = goal_ref(&goal)?;
                let v = c.call("POST", &format!("/goals/{g}/waves/{wave}/continue"), Some(json!({"who": c.who()})))?;
                out(&if v["let_start"] == true { format!("Wave {wave} of {g} can start now.") } else { format!("{g} goes on past wave {wave}.") });
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
            TaskCmd::New { title, detail, goal, also, wave, files, project, planned, here, waits_for, lock, alone, jira, devices, bits, stack_on, pr, no_pr } => {
                if wave.is_some() && goal.is_none() {
                    return Err("--wave needs --goal: waves are a goal's".into());
                }
                if !files.is_empty() && goal.is_none() {
                    return Err("--file needs --goal: it tells the task's wave mates which files are its".into());
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
                if !files.is_empty() {
                    body["files"] = json!(files);
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
                if let Some(x) = stack_on {
                    body["stack_on"] = json!(task_ref(&x)?);
                }
                if pr || no_pr {
                    body["ships_pr"] = json!(pr);
                }
                match c.report("tb.new_task", body, None, TB_TIMEOUT)? {
                    None => out(SAVED),
                    Some(v) if here => {
                        let r = v["created"][0].as_str().unwrap_or("").to_string();
                        out(v["context"].as_str().filter(|s| !s.trim().is_empty()).map(|s| s.to_string()).unwrap_or(format!("Added {r} and you're on it.")).as_str());
                    }
                    Some(v) => {
                        let r = v["created"][0].as_str().unwrap_or("").to_string();
                        // Devices and bits go on with a change, once the task is there.
                        if !devices.is_empty() || !bits.is_empty() {
                            let mut more = json!({"who": c.who()});
                            if !devices.is_empty() {
                                more["devices"] = device_arg(&devices);
                            }
                            if !bits.is_empty() {
                                more["bits"] = json!(lock_arg(&bits));
                            }
                            c.call("POST", &format!("/tasks/{r}"), Some(more))?;
                        }
                        let where_ = v["goal"].as_str().map(|g| format!(" in {g} as planned")).unwrap_or_else(|| " on the board; it waits for the owner to press Start".into());
                        out(&format!("Added {r}{where_}. {}#/?task={r}", c.cfg.page_url));
                        print_warnings(&v);
                    }
                }
                Ok(0)
            }
            TaskCmd::Set { task, title, detail, goal, also, not_also, wave, priority, waits_for, lock, alone, jira, devices, bits, not_bits, stack_on, pr } => {
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
                if !devices.is_empty() {
                    b["devices"] = device_arg(&devices);
                }
                if !bits.is_empty() {
                    b["bits"] = if bits.len() == 1 && bits[0].eq_ignore_ascii_case("none") { json!("none") } else { json!(lock_arg(&bits)) };
                }
                if !not_bits.is_empty() {
                    b["not_bits"] = json!(lock_arg(&not_bits));
                }
                if let Some(x) = stack_on {
                    b["stack_on"] = if x.eq_ignore_ascii_case("none") { json!("none") } else { json!(task_ref(&x)?) };
                }
                if let Some(x) = pr {
                    b["ships_pr"] = json!(x);
                }
                if b.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                    return Err("say what to change, for example: tb task set T12 --priority high".into());
                }
                if !devices.is_empty() || !bits.is_empty() || !not_bits.is_empty() {
                    b["who"] = json!(c.who());
                }
                let v = c.call("POST", &format!("/tasks/{t}"), Some(b))?;
                let mut bits = String::new();
                for x in lock_bits(&v) {
                    bits += &format!(" · {x}");
                }
                if let Some(l) = v["stack_on"]["line"].as_str() {
                    bits += &format!(" · {l}");
                }
                if v["ships_pr_set"].is_boolean() {
                    bits += if v["ships_pr"] == true { " · ends in a PR" } else { " · no PR" };
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
        Cmd::Backlog { action: BacklogCmd::Task { issues, board } } => {
            let v = backlog_bulk(c, "task", &issues, json!({"where": if board { "board" } else { "goal" }}))?;
            let refs = issue_refs(&issues)?;
            for (r, t) in refs.iter().zip(v["tasks"].as_array().cloned().unwrap_or_default()) {
                let where_ = match (t["status"].as_str(), t["goal"]["ref"].as_str()) {
                    (Some("planned"), Some(g)) => format!("a planned task in {g}"),
                    _ => "a queued task".to_string(),
                };
                out(&format!("Made {r} into {}, {where_}.", t["ref"].as_str().unwrap_or("a task")));
            }
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::Ticket { issues } } => {
            backlog_bulk(c, "ticket", &issues, json!({}))?;
            out(&format!("Asked Jira for a ticket for {}.", issue_refs(&issues)?.join(", ")));
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::Drop { issues, reason } } => {
            backlog_bulk(c, "drop", &issues, json!({"reason": reason.unwrap_or_default()}))?;
            out(&format!("Closed {} as won't do.", issue_refs(&issues)?.join(", ")));
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::Reopen { issues } } => {
            backlog_bulk(c, "reopen", &issues, json!({}))?;
            out(&format!("Opened {} again.", issue_refs(&issues)?.join(", ")));
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
        Cmd::Limits { compact_window, cold_idle_mins, warm_tokens, warm_idle_mins, generated, project, reset } => {
            let mut b = json!({});
            for (k, v) in [("compact_window", compact_window), ("cold_idle_mins", cold_idle_mins), ("warm_tokens", warm_tokens), ("warm_idle_mins", warm_idle_mins)] {
                if let Some(n) = v {
                    b[k] = json!(n);
                }
            }
            if let Some(g) = generated {
                b["generated"] = json!(if g.trim().eq_ignore_ascii_case("none") { String::new() } else { g });
                if let Some(p) = project {
                    b["project"] = json!(p);
                }
            } else if project.is_some() {
                return Err("--project goes with --generated".to_string());
            }
            if reset {
                b["reset"] = json!(true);
            }
            let v = if b.as_object().map(|o| o.is_empty()).unwrap_or(true) { c.call("GET", "/limits", None)? } else { c.call("POST", "/limits", Some(b))? };
            out(v["line"].as_str().unwrap_or(""));
            Ok(0)
        }
        Cmd::Pr { action } => match action {
            PrCmd::Status { task } => {
                let t = c.pr_task(task)?;
                let v = c.call("GET", &format!("/tasks/{t}/pr?full=1"), None)?;
                print_pr_status(&t, &v);
                Ok(0)
            }
            PrCmd::Reply { args, resolve } => {
                let (task, rest) = task_and(args, 2);
                let t = c.pr_task(task)?;
                let v = c.call("POST", &format!("/tasks/{t}/pr/reply"), Some(json!({"thread": rest[0], "text": rest[1], "resolve": resolve, "who": c.pr_who()})))?;
                out(&format!("Replied on thread {}{}. {} open.", rest[0], if v["resolved"] == true { " and resolved it" } else { "" }, plural_threads(&v)));
                Ok(0)
            }
            PrCmd::Ack { args } => {
                let (task, rest) = task_and(args, 1);
                let t = c.pr_task(task)?;
                let v = c.call("POST", &format!("/tasks/{t}/pr/ack"), Some(json!({"thread": rest[0], "who": c.pr_who()})))?;
                out(&format!("Thread {} is answered without a reply{}. {} open.", rest[0], if v["resolved"] == true { " and resolved" } else { "" }, plural_threads(&v)));
                Ok(0)
            }
            PrCmd::Addressed { task } => {
                let t = c.pr_task(task)?;
                let v = c.call("POST", &format!("/tasks/{t}/pr/addressed"), Some(json!({"who": c.pr_who()})))?;
                let asked: Vec<&str> = v["asked"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
                if asked.is_empty() {
                    out(&format!("Recorded {t}'s review as addressed. The board watches the PR and brings this conversation back when it needs you. You can stop here."));
                } else {
                    out(&format!("Asked {} to review {t}'s PR again. The board watches it and brings this conversation back when it needs you. You can stop here.", asked.join(", ")));
                }
                Ok(0)
            }
            PrCmd::Merge { task } => {
                let t = c.pr_task(task)?;
                c.call("POST", &format!("/tasks/{t}/pr/merge"), Some(json!({"who": c.pr_who(), "agent": !c.session.is_empty()})))?;
                out(&format!("Merged {t}'s PR and deleted its branch. You can stop here."));
                Ok(0)
            }
            PrCmd::NotOurs { task, reason, title, proof, check } => {
                let t = c.pr_task(task)?;
                let v = c.call("POST", &format!("/tasks/{t}/pr/not-ours"), Some(json!({"reason": reason, "title": title, "proof": proof, "checks": check, "who": c.pr_who()})))?;
                let cleared: Vec<&str> = v["cleared"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
                out(&format!("Cleared {} for this push of {t}'s PR: they failed, but not because of it.", cleared.join(", ")));
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
            PrCmd::BodyCheck { file } => {
                let problems = taskboardd::propen::body_problems(&c.cfg.pr_body, &read_body(&file)?);
                if problems.is_empty() {
                    out("The description is fine.");
                    return Ok(0);
                }
                out(&format!("The description needs work:\n- {}", problems.join("\n- ")));
                Ok(1)
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
        assert!(Cli::try_parse_from(["tb", "goal", "wave", "G1", "2", "--name", "API", "--stop", "on"]).is_err(), "the review stop is the owner's, in the app");
        assert!(Cli::try_parse_from(["tb", "goal", "wave", "G1", "2", "--name", "API", "--hold"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "wave", "G1", "2", "--hold", "off"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "devices"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "device", "add", "pixel-7", "--tag", "android", "--focus", "open -a Simulator"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "device", "set", "pixel-7", "--off", "off"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "device", "focus", "pixel-7"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "new", "x", "--device", "android:2", "--bit", "newCheckout"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--device", "none", "--not-bit", "a"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "set", "G1", "--device", "ios"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "bits", "--goal", "G1"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "bit", "add", "newCheckout", "--backend", "--task", "T1"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "bit", "add", "x", "--backend", "--local"]).is_err());
        assert!(Cli::try_parse_from(["tb", "bit", "made", "newCheckout", "--undo"]).is_ok());
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
        assert!(Cli::try_parse_from(["tb", "backlog", "task", "B4", "B5", "--board"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "backlog", "drop", "B4", "B5", "--reason", "dupe"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "backlog", "reopen", "B4", "B5"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "backlog", "task"]).is_err());
        assert_eq!(issue_refs(&["b4".into(), "B5".into(), "B4".into()]).unwrap(), vec!["B4", "B5"]);
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
        assert!(Cli::try_parse_from(["tb", "task", "new", "x", "--stack-on", "T3", "--no-pr"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "new", "x", "--pr", "--no-pr"]).is_err());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--stack-on", "none", "--pr", "auto"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--pr", "maybe"]).is_err());
        assert!(Cli::try_parse_from(["tb", "done", "x", "--no-pr", "covered by T3", "--no-evidence", "no UI"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "done", "x", "--pr-body", "body.md", "--title", "Add x"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "done", "x", "--pr-body", "body.md", "--no-pr", "why"]).is_err());
        assert!(Cli::try_parse_from(["tb", "done", "x", "--title", "Add x"]).is_err());
        assert!(Cli::try_parse_from(["tb", "step", "triage", "Review", "F2", "--state", "fixed", "--commit", "abc"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "step", "triage", "Review", "F2", "--state", "gone"]).is_err());
        assert!(Cli::try_parse_from(["tb", "step", "again", "Review"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "pr", "body-check", "-"]).is_ok());
    }

    #[test]
    fn lock_args() {
        assert_eq!(lock_arg(&["local-core,emulator-5554".into()]), vec!["local-core", "emulator-5554"]);
        assert_eq!(lock_arg(&["a".into(), "b c".into()]), vec!["a", "b", "c"]);
        assert!(lock_arg(&["none".into()]).is_empty());
    }

    #[test]
    fn goal_setup_help_shows_its_placeholders() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let setup = cmd.find_subcommand_mut("goal").unwrap().find_subcommand_mut("setup").unwrap();
        let help = setup.render_help().to_string();
        assert!(help.contains("{task} {n} {wave} {goal}"), "{help}");
        assert!(Cli::try_parse_from(["tb", "task", "new", "Form", "--goal", "G3", "--wave", "1", "--file", "src/a.rs", "--file", "src/b.rs"]).is_ok());
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
    fn device_switches_and_needs() {
        assert!(Cli::try_parse_from(["tb", "device", "set", "pixel-7", "--off"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "device", "set", "pixel-7", "--on"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "device", "set", "pixel-7", "--on", "--off"]).is_err());
        assert_eq!(device_arg(&["none".into()]), json!("none"));
        assert_eq!(device_arg(&["Goal".into()]), json!("goal"));
        assert_eq!(device_arg(&["android:2, ios".into()]), json!(["android:2", "ios"]));
    }

    #[test]
    fn basic_auth() {
        assert_eq!(basic("a@b.co", "tok"), "YUBiLmNvOnRvaw==");
        assert_eq!(basic("ab", "c"), "YWI6Yw==");
    }
}
