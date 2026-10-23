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

/// How long tb waits for the board: for the whole answer to a quick call, and for each stage of a long
/// one on the progress stream (the board says "still waiting on …" while one slow call runs).
const TB_TIMEOUT: f64 = 5.0;
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
        /// It misses the goal's own done-when: it goes to the owner, and the goal stays open until it's settled
        #[arg(long)]
        misses_goal: bool,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Ask the owner a question (the board may answer it from the owner's rules)
    Question {
        text: String,
        /// An answer the owner can pick with one click, as "Label: what it means" (repeat it, up to 8)
        #[arg(long = "option")]
        options: Vec<String>,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Take back a question that got its answer another way, so the task stops waiting on the owner
    #[command(alias = "unask")]
    Withdraw {
        /// How it got answered
        why: String,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Hand the task in (a failed one too, with --task): with its PR it waits in review until the PR merges; with --no-pr it's finished
    #[command(alias = "done")]
    Submit {
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
        /// With --pr-body: open the PR from this pushed branch instead of the checkout's own (a checkout
        /// another task is busy in can stay where it is)
        #[arg(long = "branch", value_name = "BRANCH", requires = "pr_body")]
        pr_branch: Option<String>,
        /// It finishes without the PR it was meant to open, and why
        #[arg(long = "no-pr", value_name = "WHY")]
        no_pr: Option<String>,
        /// It finishes without evidence (screenshots, results), and why
        #[arg(long = "no-evidence", value_name = "WHY")]
        no_evidence: Option<String>,
        /// It finishes without the lens review ([lens] submit on), when the owner said to skip it, and why
        #[arg(long = "no-lens", value_name = "WHY")]
        no_lens: Option<String>,
        /// The note for testers the board posts on the ticket at merge, in markdown (`[jira] merged_note`; see tb jira)
        #[arg(long = "merged-note", value_name = "MARKDOWN", conflicts_with = "no_pr")]
        merged_note: Option<String>,
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
    /// Switch this terminal to a task in its line; the one it's on waits here to resume
    Switch { task: String },
    /// This terminal's line: the tasks waiting their turn here
    Line {
        #[command(subcommand)]
        action: Option<LineCmd>,
    },
    /// Start a task, or run a goal (G<n>), only when the human told you to in this conversation (the board
    /// checks their prompts for a task). A goal runs its tasks in their waves; never start them one by one.
    Start {
        /// T<n>, G<n> for a goal, or B<n> for a backlog item (made into its task first)
        task: String,
        /// Queue the task to start once its project has a free terminal (tb project set --max-terminals), instead of now in a new terminal
        #[arg(long)]
        queue: bool,
        /// Run the goal outside work hours, until they open (the board's Start now). Goals only
        #[arg(long, conflicts_with = "queue")]
        now: bool,
    },
    /// This terminal's task
    Status {
        #[command(flatten)]
        t: TaskArg,
    },
    /// This terminal's task, its line and a PR it's visiting, as JSON. Only reads: safe to poll from a status bar
    Peek {
        /// Print Midna status-bar segments instead (`[]` when there's nothing to show)
        #[arg(long)]
        midna: bool,
    },
    /// Write a redacted zip of the board, its logs and state for debugging on another machine, and print its path
    Export {
        /// The folder to write it in (default: Downloads)
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
    /// Print the preview card Midna shows for a hovered taskboard:// task or goal link, as JSON. Only reads
    Preview {
        url: String,
        /// The board's port, when it isn't the usual one
        #[arg(long)]
        port: Option<u16>,
    },
    /// Bring a task's terminal to the front (a link menu's Go to its terminal)
    Focus {
        task: String,
        /// The board's port, when it isn't the usual one
        #[arg(long)]
        port: Option<u16>,
    },
    /// Wait for another task's work before carrying on (or `none`)
    WaitFor {
        tasks: Vec<String>,
        #[arg(long)]
        why: Option<String>,
        /// Wait until its PR merges, not just until it's done
        #[arg(long)]
        merged: bool,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Propose tasks for a goal: --task "title::what to do". Planned, or queued while the goal is running
    Propose {
        goal: String,
        /// A planned task: "title::detail", "title::detail::<wave>", "title::detail::<wave or nothing>::<T14 #1, the tasks it waits for>"
        /// or with "::<src/a.rs, src/b.rs>" after the waits, the files the wave plans for it (its wave mates are told);
        /// end any of them with "::model=small" (small, large or a model name) for the model its terminal runs, "::effort=high" (low, medium, high, xhigh or max) for its effort and/or "::project=api" to put it in another project than the goal's
        #[arg(long = "task", value_name = "TITLE::DETAIL[::WAVE][::WAITS][::FILES][::model=M][::effort=E][::project=P]")]
        tasks: Vec<String>,
        /// Planned even while the goal is running, so they wait for its next run
        #[arg(long)]
        planned: bool,
        /// It's different work from the open backlog items whose titles look like it (the board refuses it otherwise)
        #[arg(long = "not-backlog")]
        not_backlog: bool,
    },
    /// Named locks, who holds each, and tasks that run alone
    Locks,
    /// Worktree slots: who holds each, who waits for one, when each was last used and its size
    Slots {
        /// Only this project's slots
        #[arg(long)]
        project: Option<String>,
    },
    /// Claim or give back this task's worktree slot
    Slot {
        #[command(subcommand)]
        action: SlotCmd,
    },
    /// Stage only this task's own hunks of the files it edited, from its recorded edits, so a commit in a checkout other tasks share takes none of theirs
    Stage {
        /// Only these files; on its own, every uncommitted file the task edited here
        files: Vec<String>,
        #[arg(long)]
        task: Option<String>,
    },
    /// Close the terminals of finished tasks (on the owner's word): tb close --done
    Close {
        /// The terminals of done tasks; terminals still working are left open
        #[arg(long, required = true)]
        done: bool,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        goal: Option<String>,
    },
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
    /// Lessons learned from finished work: this project's and the board's (kept and suggested)
    Lessons {
        /// The project (this terminal's when left out)
        #[arg(long)]
        project: Option<String>,
        /// Every project's
        #[arg(long)]
        all: bool,
        #[arg(long, value_parser = ["suggested", "kept", "retired"])]
        state: Option<String>,
        #[arg(long, value_parser = ["planning", "code", "process", "tooling"])]
        kind: Option<String>,
    },
    /// Tasks, backlog items, goals, goal notes and ideas that match some words, best first
    Search {
        query: String,
        /// The project (this terminal's when left out)
        #[arg(long, conflicts_with = "all")]
        project: Option<String>,
        /// Every project's
        #[arg(long)]
        all: bool,
        #[arg(long, value_parser = ["task", "backlog", "goal", "note", "idea"])]
        kind: Option<String>,
        /// Only what's still open
        #[arg(long)]
        open: bool,
    },
    /// Suggest, change, keep or retire a lesson that outlasts its goal
    Lesson {
        #[command(subcommand)]
        action: LessonCmd,
    },
    /// Keep ideas that aren't ready to plan yet: add, list, show, change, note, shelve, drop or reopen one
    Idea {
        #[command(subcommand)]
        action: IdeaCmd,
    },
    /// A project's releases, newest first, with their task counts and how many done tasks wait in Unreleased
    Releases {
        /// The project (this terminal's when left out)
        #[arg(long)]
        project: Option<String>,
    },
    /// Cut, show, rename or annotate a release, or move a done task between releases
    Release {
        #[command(subcommand)]
        action: ReleaseCmd,
    },
    /// A project's areas: backfill the area of work already done
    Areas {
        #[command(subcommand)]
        action: AreasCmd,
    },
    /// A project's reviewers: list, add, remove (never ask them), back, pin, bot schedule, automation, aliases
    Reviewers {
        #[command(subcommand)]
        action: Option<ReviewersCmd>,
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
        /// Only the comments on this goal's tickets (G2), waiting ones first
        #[arg(long)]
        goal: Option<String>,
    },
    /// Jira: how it's set up and its jobs, one job (J12), or the Jira desk's report on one:
    /// tb jira J12 ok key=PROJ-1 status="To Do" [found=yes [added="labels web"]] [product=web] / tb jira J12 fail "<why>"
    Jira {
        /// J12
        job: Option<String>,
        /// ok or fail
        #[arg(value_parser = ["ok", "fail"])]
        result: Option<String>,
        /// key=… status=… found=yes added=… product=…, or (with fail) why
        rest: Vec<String>,
    },
    /// How this board slices a goal into tasks: the owner's planning file for the project or the board,
    /// else the built-in rules
    Planning {
        /// The project (this terminal's when left out)
        #[arg(long)]
        project: Option<String>,
        /// Show the edit that writes the project's kept planning lessons into its planning file
        #[arg(long, conflicts_with = "accept")]
        suggest: bool,
        /// Write that edit (the owner accepted it)
        #[arg(long)]
        accept: bool,
    },
    /// How well goals were planned: tasks added late, failed or retried, review rounds, questions and time against the estimate, by month
    Quality {
        /// The project (this terminal's when left out)
        #[arg(long)]
        project: Option<String>,
        /// Every project's
        #[arg(long)]
        all: bool,
        /// Each finished goal's numbers too
        #[arg(long)]
        goals: bool,
    },
    /// Where agent time went (the last 7 days, a week or since a date), a kept review by number, or one task's commands (T12)
    Review {
        /// A task (T12) for its commands, or a kept review's number
        target: Option<String>,
        /// From a date (2026-10-09), or 3d or 2w back
        #[arg(long, conflicts_with = "week")]
        since: Option<String>,
        /// This week, last week, or the week of a date
        #[arg(long)]
        week: Option<String>,
        /// Keep it and tell the owner, as the weekly review does
        #[arg(long)]
        save: bool,
        /// The kept reviews
        #[arg(long, conflicts_with_all = ["target", "since", "week", "save"])]
        list: bool,
    },
    /// Tasks, one line each: this terminal's project's (with --goal, every project's)
    Tasks {
        /// The project, or all (this terminal's when left out)
        #[arg(long)]
        project: Option<String>,
        /// G4, or none for tasks outside a goal
        #[arg(long)]
        goal: Option<String>,
        #[arg(long, value_parser = ["planned", "queued", "working", "needs", "review", "done"])]
        status: Option<String>,
        /// Print the board's answer as JSON
        #[arg(long)]
        json: bool,
    },
    /// Add, change, show or delete a task
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
        /// A line on what it is, shown under its title on the board
        #[arg(long)]
        summary: Option<String>,
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
        /// Keep going until a time, then the usual hours: past midnight if it has passed today (6am, tomorrow 9am, off)
        #[arg(long, visible_alias = "today-until", conflicts_with = "for_")]
        until: Option<String>,
        /// Keep going for a while from now, then the usual hours (8h, 90m, 1h30m, off)
        #[arg(long = "for", value_name = "DURATION")]
        for_: Option<String>,
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
    /// Show or change how many task terminals run at once across the board; tasks over it wait for a free slot
    Terminals {
        /// A count from 1 to 50, or default (config.toml's [terminals] global_max)
        #[arg(long)]
        max: Option<String>,
    },
    /// Show or change what Midna's status bar shows of a terminal's task: its goal, and the titles
    StatusBar {
        /// The task's goal before it
        #[arg(long, value_parser = ["on", "off"])]
        goal: Option<String>,
        /// Titles after the refs (off: refs only; hovering still shows them)
        #[arg(long, value_parser = ["on", "off"])]
        title: Option<String>,
        /// Go back to the defaults (both on)
        #[arg(long)]
        reset: bool,
    },
    /// Show or change the context limits: the compact window, when a cold conversation is compacted,
    /// how big and how fresh a conversation must be to resume, how many compactions a task terminal
    /// takes before it carries on in a fresh one, and the generated-file globs
    Limits {
        /// Tokens at which Claude compacts a board terminal's conversation, e.g. 150000 or 150k (0 = off; at least 50k)
        #[arg(long = "compact-window")]
        compact_window: Option<String>,
        /// Minutes idle after which a conversation is compacted before it resumes (0 = off)
        #[arg(long = "cold-idle-mins")]
        cold_idle_mins: Option<i64>,
        /// A task or PR conversation resumes only under this many tokens, else starts fresh (0 = off)
        #[arg(long = "warm-tokens")]
        warm_tokens: Option<String>,
        /// …and only when idle under this many minutes (0 = off)
        #[arg(long = "warm-idle-mins")]
        warm_idle_mins: Option<i64>,
        /// A task terminal that has compacted this many times carries on in a fresh terminal from its handoff (0 = off)
        #[arg(long = "max-compactions")]
        max_compactions: Option<i64>,
        /// Generated-file globs kept as `-diff` in .git/info/attributes, comma-separated (none clears them)
        #[arg(long)]
        generated: Option<String>,
        /// With --generated: that project's own globs, on top of everyone's
        #[arg(long)]
        project: Option<String>,
        /// Minutes between a branch's review rounds (a step's min_gap_mins), 0 to 240 (0 = no gap)
        #[arg(long = "review-gap")]
        review_gap: Option<i64>,
        /// With --review-gap: the step it's for, when more than one step has a gap
        #[arg(long)]
        step: Option<String>,
        /// Forget every change and go back to config.toml's [limits]
        #[arg(long)]
        reset: bool,
    },
    /// A done task's pull request
    Pr {
        #[command(subcommand)]
        action: PrCmd,
    },
    /// The PR feed: its health (with no action), or post an event or heartbeat for a listener
    Feed {
        #[command(subcommand)]
        action: Option<FeedCmd>,
    },
    /// The work event stream (docs/STREAM.md): each source's mode and health (no action), send events
    Stream {
        #[command(subcommand)]
        action: Option<StreamCmd>,
        /// Print GET /stream as JSON (with no action)
        #[arg(long)]
        json: bool,
    },
    /// Stop or resume the board's PR builds, only on the owner's word (no action: whether they're stopped)
    PrBuilds {
        #[command(subcommand)]
        action: Option<PrBuildsCmd>,
    },
    /// Red default branches: tb master (the breaks), tb master M3 (or show M3), tb master M3 ours|not-ours|unsure (the owner's word), tb master check
    Master {
        /// [M<n>] [show|ours|not-ours|unsure|check], in either order
        #[arg(num_args = 0..=2)]
        args: Vec<String>,
        /// Why (with a verdict)
        #[arg(long)]
        why: Option<String>,
        /// Whose word the verdict is (the owner when left out)
        #[arg(long)]
        who: Option<String>,
        /// A link that shows it isn't yours (a build, an issue); not-ours needs one. Repeat for more.
        #[arg(long)]
        proof: Vec<String>,
        /// The cause in a few plain words, 80 characters at most (with a verdict): shown on the break's card and alert
        #[arg(long)]
        title: Option<String>,
        /// List only the breaks whose branch is still red
        #[arg(long)]
        open: bool,
        /// List at most this many breaks, newest first
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// Which accounts are connected (GitHub, Bitbucket, Slack); connect them in Taskboard ▸ Settings
    Accounts,
    /// The CI token the board reads failed steps and tests with (Azure DevOps): show, set or clear it
    CiToken {
        #[command(subcommand)]
        action: Option<CiTokenCmd>,
    },
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
    /// The start check's words: misses lists the starts it refused, with the prompt it read
    Words {
        #[command(subcommand)]
        action: WordsCmd,
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
enum SlotCmd {
    /// Claim a slot for review fixes, a rebase or a failing check: its own again when free, on its pushed branch
    Claim {
        #[arg(long)]
        task: Option<String>,
        /// Move the task's uncommitted edits in these files from the shared checkout into the slot; on
        /// its own, the files the task edited there. A file another task edits too gives up only this
        /// task's hunks; refused where both changed the same lines
        #[arg(long, num_args = 0.., value_name = "FILE")]
        bring: Option<Vec<String>>,
        /// Also bring this hunk no task's edit log has (shell edits, edits from before the task), by the
        /// id the claim listed; repeat for more, or `none` to leave them all
        #[arg(long = "bring-hunk", value_name = "FILE:ID")]
        bring_hunk: Vec<String>,
    },
    /// Give the slot back; refused while it has uncommitted changes or unpushed commits
    Release {
        #[arg(long)]
        task: Option<String>,
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
        /// What it is, shown with its name: android (an emulator), ios (a simulator), device (a phone or tablet), other… A label, not a tag
        #[arg(long)]
        kind: Option<String>,
        /// The device's serial or UDID (emulator-5554, a simulator's UDID): {target} in its start and stop commands
        #[arg(long)]
        target: Option<String>,
        /// How to boot it, told to the task that's lent it ({device} {task} {branch}… filled in)
        #[arg(long = "start", alias = "start-cmd")]
        start: Option<String>,
        /// How to shut it down when the task is done with it ({device} {task}… filled in)
        #[arg(long = "stop", alias = "stop-cmd")]
        stop: Option<String>,
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
        /// What it is (a label, not a tag), or none
        #[arg(long)]
        kind: Option<String>,
        /// Its serial or UDID, or none
        #[arg(long)]
        target: Option<String>,
        /// Its start command, or none
        #[arg(long = "start", alias = "start-cmd")]
        start: Option<String>,
        /// Its stop command, or none
        #[arg(long = "stop", alias = "stop-cmd")]
        stop: Option<String>,
        /// Switch it off: no task is lent it
        #[arg(long, conflicts_with = "on")]
        off: bool,
        /// Switch it back on
        #[arg(long)]
        on: bool,
    },
    /// Take a device out of the pool
    Remove {
        name: String,
        /// Remove it even though tasks or goals still need it, dropping it from their needs
        #[arg(long)]
        force: bool,
    },
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
enum ReleaseCmd {
    /// Cut a release holding every done task in Unreleased (its PR merged, or done with no PR). A project
    /// with a release script runs it first (from main or a release/* branch), with the name, and records the release only if it succeeds
    Cut {
        name: String,
        /// The project (this terminal's when left out)
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        /// Record the release without running the project's release script (for a release already out)
        #[arg(long = "no-script")]
        no_script: bool,
        /// Leave the cut pending until this task or goal is through (T627: done with its PR merged; G12:
        /// finished), then the board cuts it, release script included. Repeat for more. One per project
        #[arg(long, value_name = "T<n>|G<n>")]
        after: Vec<String>,
    },
    /// Drop the project's pending cut (tb release cut --after)
    Cancel {
        /// The project (this terminal's when left out)
        #[arg(long)]
        project: Option<String>,
    },
    /// A release's tasks (R<n>, or unreleased), grouped by goal
    Show {
        release: String,
        /// The project, for unreleased (this terminal's when left out)
        #[arg(long)]
        project: Option<String>,
        /// A changelog ready to paste
        #[arg(long)]
        markdown: bool,
    },
    /// Rename a release or change its notes (--notes "" clears them)
    Set {
        release: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        notes: Option<String>,
    },
    /// Put a done task in another release, or back in Unreleased
    Move { task: String, release: String },
    /// Mark a release failed (its build or publish didn't go out): its tasks go back in Unreleased and
    /// its name is free, so tb release cut can take it again
    Fail {
        release: String,
        /// What went wrong
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Subcommand)]
enum LessonCmd {
    /// Suggest a lesson for later work: planning (how to slice goals), code, process or tooling. It reaches
    /// handoffs (or planners, for planning) once the owner keeps it
    Add {
        text: String,
        #[arg(long, value_parser = ["planning", "code", "process", "tooling"])]
        kind: String,
        /// For every project on the board, not just this one
        #[arg(long, conflicts_with = "project")]
        global: bool,
        /// The project (this terminal's when left out)
        #[arg(long)]
        project: Option<String>,
        /// The work it was learned from (T12, G3, B7); this terminal's task is added. Repeat for more
        #[arg(long)]
        evidence: Vec<String>,
        /// Kept at once. Only on the owner's word
        #[arg(long)]
        kept: bool,
    },
    /// Change a lesson's text, kind, scope or evidence
    Edit {
        lesson: String,
        #[arg(long)]
        text: Option<String>,
        #[arg(long, value_parser = ["planning", "code", "process", "tooling"])]
        kind: Option<String>,
        #[arg(long, conflicts_with = "project")]
        global: bool,
        #[arg(long)]
        project: Option<String>,
        /// More work it was learned from; repeat for more
        #[arg(long)]
        evidence: Vec<String>,
    },
    /// Show one lesson
    Show { lesson: String },
    /// Keep a lesson, so handoffs (or planners) get it. Only on the owner's word
    Keep { lesson: String },
    /// Retire a lesson that no longer holds. Only on the owner's word
    Retire { lesson: String },
    /// Put a kept or retired lesson back to suggested
    Suggest { lesson: String },
    /// Later work showed the lesson again: count it and add that work as evidence
    Confirm {
        lesson: String,
        #[arg(long)]
        evidence: Vec<String>,
    },
    /// Look back over a finished goal and suggest lessons from it (confirming ones already on the board)
    Retro { goal: String },
    /// The lessons that fit some words about the work, best first, with what made each fit
    Search {
        query: String,
        /// The project (this terminal's when left out)
        #[arg(long, conflicts_with = "all")]
        project: Option<String>,
        /// Every project's
        #[arg(long)]
        all: bool,
        /// Only these (retired ones are left out otherwise)
        #[arg(long, value_parser = ["suggested", "kept", "retired"])]
        state: Option<String>,
    },
}

#[derive(Subcommand)]
enum IdeaCmd {
    /// Keep an idea for later: a short title, and what it is in --detail
    Add {
        title: String,
        #[arg(long)]
        detail: Option<String>,
        /// The project (this terminal's when left out)
        #[arg(long)]
        project: Option<String>,
    },
    /// A project's ideas, newest change first: the open ones (new, digging, reported) unless --state says
    List {
        /// The project (this terminal's when left out)
        #[arg(long, conflicts_with = "all")]
        project: Option<String>,
        /// Every project's
        #[arg(long)]
        all: bool,
        #[arg(long, value_parser = ["open", "all", "new", "digging", "reported", "planned", "shelved", "dropped"])]
        state: Option<String>,
    },
    /// Show an idea with its notes and report rounds
    Show { idea: String },
    /// Change an idea's title, detail or project
    Set {
        idea: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        detail: Option<String>,
        #[arg(long)]
        project: Option<String>,
    },
    /// Add a note to an idea
    Note { idea: String, text: String },
    /// Drop an idea: it won't be built
    Drop {
        idea: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Shelve an idea for later
    Shelve {
        idea: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Bring back a shelved or dropped idea
    Reopen { idea: String },
    /// Have Claude read the project's code and report whether the idea fits; it runs on its own
    Dig {
        idea: String,
        /// What this round should look at most (the whole idea when left out)
        focus: Option<String>,
        /// Let Claude search and read the web too
        #[arg(long)]
        web: bool,
    },
    /// Have Claude plan an idea into a goal and its tasks, from its notes and reports; planned only, not started
    Plan { idea: String },
}

#[derive(Subcommand)]
enum AreasCmd {
    /// Infer the area of tasks with none set by hand from their PR's files and commits, and each goal's from its tasks
    Backfill {
        /// Only this project's (every project with areas by default)
        #[arg(long)]
        project: Option<String>,
    },
}

#[derive(Subcommand)]
enum ReviewersCmd {
    /// The project's reviewers (the default): who's pinned, removed, their bot and pace
    List {
        #[arg(long)]
        project: Option<String>,
        /// Every project's
        #[arg(long)]
        all: bool,
    },
    /// Add a reviewer, or fold more about one into the reviewer they already are
    Add {
        name: String,
        #[arg(long)]
        project: Option<String>,
        /// Their id on the PR host: a GitHub login, a Bitbucket {uuid} or account id
        #[arg(long)]
        user: Option<String>,
        /// A commit email of theirs (repeat for more)
        #[arg(long = "email")]
        emails: Vec<String>,
        /// Another name or account of theirs (repeat for more)
        #[arg(long = "alias")]
        aliases: Vec<String>,
        /// Their Slack user id or email, for the availability check
        #[arg(long)]
        slack: Option<String>,
    },
    /// Never ask this person (until tb reviewers back)
    Remove {
        who: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Ask a removed reviewer again
    Back {
        who: String,
        #[arg(long)]
        project: Option<String>,
    },
    /// Put this reviewer first in line for each PR's main-contributor pick
    Pin {
        who: String,
        #[arg(long)]
        project: Option<String>,
    },
    /// Take this reviewer out of the front of the line
    Unpin {
        who: String,
        #[arg(long)]
        project: Option<String>,
    },
    /// Their review bot runs every --every hours and marks its comments with --mark (--off: no bot)
    Bot {
        who: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, required_unless_present = "off")]
        every: Option<f64>,
        #[arg(long, required_unless_present = "off")]
        mark: Option<String>,
        #[arg(long)]
        off: bool,
    },
    /// How automated their reviewing is: off, low, normal, high or a number (1 is normal)
    Auto {
        who: String,
        level: String,
        #[arg(long)]
        project: Option<String>,
    },
    /// Another name, email or host account of theirs (an @login or {uuid} becomes their account when they have none)
    Alias {
        who: String,
        #[arg(required_unless_present = "user")]
        aliases: Vec<String>,
        /// Their host account (a login or {uuid}); one they had stays as an alias
        #[arg(long)]
        user: Option<String>,
        #[arg(long)]
        project: Option<String>,
    },
    /// Pick up commit authors (and their host accounts) into the roster now
    Sync {
        #[arg(long)]
        project: Option<String>,
    },
    /// Two reviewers are one person: fold the second into the first
    Merge {
        keep: String,
        other: String,
        #[arg(long)]
        project: Option<String>,
    },
}

#[derive(Subcommand)]
enum WordsCmd {
    /// The latest task starts and goal runs refused for want of the owner's word, newest first
    Misses {
        #[arg(long, default_value_t = 20)]
        limit: usize,
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
        aim: Aim,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Save where the task's rounds look, so every later round and the gates follow it: tb step aim --branch feat/x
    #[command(name = "aim")]
    SetAim {
        #[command(flatten)]
        aim: Aim,
        /// Drop the saved aim; rounds look at the checkout they run from again
        #[arg(long, conflicts_with_all = ["branch", "worktree", "commit"])]
        clear: bool,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Run a script step (and its check); it counts once it exits 0
    Run {
        name: String,
        #[command(flatten)]
        aim: Aim,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Ask the owner to do their step; the task waits for them, so end your turn after
    Ask {
        name: String,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Pass a skippable step without doing it, saying why: tb step skip "Verify numbers" --why "no number to prove"
    Skip {
        name: String,
        #[arg(long)]
        why: String,
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
        aim: Aim,
        #[command(flatten)]
        t: TaskArg,
    },
    /// Republish a step's passing round for the PR's head (its publish script), as after a rebase's push
    Publish {
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

/// What a step's round looks at, when it isn't this checkout's head (else the aim `tb step aim` saved).
#[derive(Args, Clone, Default)]
struct Aim {
    /// Look at this branch, in the worktree that has it checked out (with --worktree, the branch that checkout is on, for a detached one)
    #[arg(long)]
    branch: Option<String>,
    /// Look at the checkout in this folder
    #[arg(long)]
    worktree: Option<String>,
    /// Look at this commit (a sha or any ref, like HEAD~1) on that checkout's branch; it's resolved to its
    /// sha, and the round runs on a checkout of it
    #[arg(long)]
    commit: Option<String>,
}

impl Aim {
    fn is_empty(&self) -> bool {
        [&self.branch, &self.worktree, &self.commit].iter().all(|x| x.as_deref().is_none_or(|s| s.trim().is_empty()))
    }

    /// The aim `tb step aim` saved on the task (`aim` in `GET /steps`).
    fn saved(v: &Value) -> Aim {
        let s = |k: &str| v["aim"][k].as_str().filter(|x| !x.trim().is_empty()).map(|x| x.to_string());
        Aim { branch: s("branch"), worktree: s("worktree"), commit: s("sha") }
    }
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
        /// A goal it runs after (on the owner's word): its tasks wait until that goal is finished, then it runs on its own. Repeat for more
        #[arg(long = "after", value_name = "G<n>")]
        after: Vec<String>,
        /// A planned task: "title::detail", "title::detail::<wave>", "title::detail::<wave or nothing>::<T14 #1, the tasks it waits for>"
        /// or with "::<src/a.rs, src/b.rs>" after the waits, the files the wave plans for it (its wave mates are told);
        /// end any of them with "::model=small" (small, large or a model name) for the model its terminal runs, "::effort=high" (low, medium, high, xhigh or max) for its effort and/or "::project=api" to put it in another project than the goal's
        #[arg(long = "task", value_name = "TITLE::DETAIL[::WAVE][::WAITS][::FILES][::model=M][::effort=E][::project=P]")]
        tasks: Vec<String>,
        /// One of its project's areas (`.taskboard.toml` at the repo's root), any case
        #[arg(long, value_name = "NAME")]
        area: Option<String>,
        /// It's different work from the open backlog items whose titles look like it (the board refuses it otherwise)
        #[arg(long = "not-backlog")]
        not_backlog: bool,
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
        /// Run tasks one after another; only in a goal without waves (waves set the start order otherwise)
        #[arg(long, value_parser = ["on", "off"])]
        in_order: Option<String>,
        #[arg(long)]
        max_terminals: Option<i64>,
        #[arg(long)]
        epic: Option<String>,
        #[arg(long)]
        product: Option<String>,
        /// Off: none of its tasks or their PRs asks for a Jira ticket, unless the task opts in with --jira new or a key.
        /// Turning it back on makes no tickets for the tasks it already has
        #[arg(long, value_parser = ["on", "off"])]
        jira: Option<String>,
        /// Start each task in its own git worktree, detached at this branch (like origin/main); off for the shared folder
        #[arg(long, value_name = "BASE|off")]
        worktrees: Option<String>,
        /// Where its tasks on auto work: shared (the shared checkout), slot (a worktree from the pool), alone, or auto (the board picks per task)
        #[arg(long, value_parser = ["shared", "slot", "alone", "auto"])]
        lane: Option<String>,
        /// Queue its planned tasks to run (and unpause it), when the owner says so
        #[arg(long)]
        run: bool,
        /// Hold its queued tasks back until it's prioritized or run again
        #[arg(long, conflicts_with = "prioritize")]
        deprioritize: bool,
        /// Back to normal priority
        #[arg(long)]
        prioritize: bool,
        /// One of its project's areas (`.taskboard.toml` at the repo's root), any case, or none
        #[arg(long, value_name = "NAME|none")]
        area: Option<String>,
        /// Devices each of its tasks asks for (unless the task asks for its own): android, android:2. Repeat for more, none for none
        #[arg(long = "device", value_name = "TAG[:N]|none")]
        devices: Vec<String>,
        /// A goal it runs after (on the owner's word): its tasks wait until that goal is finished (every task done, every PR merged or closed), then the board runs it. Repeat for more, none for none
        #[arg(long = "after", value_name = "G<n>|none")]
        after: Vec<String>,
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
    /// Go on past any wave (a review stop, a failed task, or one that hasn't finished), when the owner says so; or let a held wave start
    Continue { goal: String, wave: i64 },
    /// The goal's own devices: its tasks get only those (`[devices] goal_pool_only`, else those first),
    /// a reserved one goes to no other goal's tasks (goals that reserve one share it), and a device's
    /// purposes count as its tags for this goal's tasks. With no flags, lists them
    Devices {
        goal: String,
        /// A device to put in the goal's pool (or change there)
        #[arg(long, value_name = "DEVICE", conflicts_with = "remove")]
        add: Option<String>,
        /// What the goal uses it for, matched like a tag (measure, or a comma list: measure,demo); none drops it
        #[arg(long, requires = "add")]
        purpose: Option<String>,
        /// Keep it for this goal's tasks only
        #[arg(long, requires = "add", conflicts_with = "unreserve")]
        reserve: bool,
        /// Stop keeping it for this goal
        #[arg(long, requires = "add")]
        unreserve: bool,
        /// A device to take out of the goal's pool
        #[arg(long, value_name = "DEVICE")]
        remove: Option<String>,
    },
    /// Finish a goal whose failed tasks were replaced by other work (only on the owner's word). They stay failed
    /// and keep counting as failed; refused while a task is still to do, a PR is open or a miss is open
    Done {
        goal: String,
        /// Why the goal is finished anyway (e.g. "T188 replaced T36 and T84; accepted on its numbers")
        #[arg(long)]
        why: String,
    },
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
enum LineCmd {
    /// Take a task out of this terminal's line and back to the board, to wait for Start
    Drop { task: String },
}

#[derive(Subcommand)]
enum TaskCmd {
    /// Add a task: in a goal (queued while it's running, else planned), or on its own
    New {
        /// Its title; with --issue, the item's title when left out
        #[arg(required_unless_present = "issue")]
        title: Option<String>,
        /// The open backlog item it's made from (B12): linked to it, with its detail, and the item becomes this task
        #[arg(long, value_name = "B12")]
        issue: Option<String>,
        #[arg(long)]
        detail: Option<String>,
        /// The goal it's in; with --here, this terminal does it and the goal's runner leaves it alone
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
        /// Planned, not queued: on its own it waits for the goal it joins later; with --goal it waits for the
        /// goal's next run even while the goal is running
        #[arg(long)]
        planned: bool,
        /// A task for the work this terminal is already doing with the owner; you're on it at once. Standalone, or in
        /// --goal G<n> (design work: make the goal first, and the build tasks that follow join it)
        #[arg(long)]
        here: bool,
        /// With --here on a terminal that has a task: queue it in this terminal's line, after that task
        #[arg(long, requires = "here", conflicts_with = "now")]
        next: bool,
        /// With --here on a terminal that has a task: switch to it now; that task waits here to resume
        #[arg(long, requires = "here")]
        now: bool,
        /// It starts only once this task (any goal) is done; repeat for more. In a goal it waits for the goal's
        /// open design tasks when left out; none waits for nothing
        #[arg(long = "waits-for", value_name = "T12|none")]
        waits_for: Vec<String>,
        /// A named lock it holds while it runs; tasks sharing a lock never run together. Repeat for more
        #[arg(long, value_name = "NAME")]
        lock: Vec<String>,
        /// Nothing else in its goal runs while it does (board: nothing else on the board)
        #[arg(long, num_args = 0..=1, default_missing_value = "goal", value_parser = ["goal", "board", "none"])]
        alone: Option<String>,
        /// Where it works: shared (the shared checkout), slot (a worktree from the pool), alone, or auto (the board picks; `tb planning` lists the rules)
        #[arg(long, value_parser = ["shared", "slot", "alone", "auto"])]
        lane: Option<String>,
        /// With --here --lane slot: claim its slot now and move the uncommitted edits in these files from the
        /// shared checkout into it (on its own, none)
        #[arg(long, num_args = 0.., value_name = "FILE", requires = "here")]
        bring: Option<Vec<String>>,
        /// A Jira key to link, `new` for a new ticket, or `none` for no ticket
        #[arg(long)]
        jira: Option<String>,
        /// The new ticket's summary, written to the board's ticket guide (`tb jira`); the title when left out
        #[arg(long = "ticket-summary")]
        ticket_summary: Option<String>,
        /// The new ticket's description in markdown, written to the board's ticket guide (`tb jira`); the detail when left out
        #[arg(long = "ticket-text")]
        ticket_text: Option<String>,
        /// Devices from the pool it needs while it runs, by tag or name: android, android:2. Repeat for more,
        /// none for none (even when its goal asks for some)
        #[arg(long = "device", value_name = "TAG[:N]|none")]
        devices: Vec<String>,
        /// A bit (feature flag) its work sits behind; it starts whether or not a backend bit is made. Repeat for more
        #[arg(long = "bit", value_name = "NAME")]
        bits: Vec<String>,
        /// Its PR builds on this task's PR (any goal): it starts once that's done, from its branch
        #[arg(long = "stack-on", value_name = "T12")]
        stack_on: Option<String>,
        /// How its stacked PR merges: tail (the default) moves it to the main branch once its parent's
        /// merges; head merges it into its parent's branch, and the parent's PR takes it to main
        #[arg(long = "stack-merge", value_parser = ["head", "tail"], requires = "stack_on")]
        stack_merge: Option<String>,
        /// It ends in a PR, whatever its project does
        #[arg(long, conflicts_with = "no_pr")]
        pr: bool,
        /// It ends without a PR
        #[arg(long = "no-pr")]
        no_pr: bool,
        /// One of its project's areas (`.taskboard.toml` at the repo's root), any case
        #[arg(long, value_name = "NAME")]
        area: Option<String>,
        /// The model its terminal runs: small or large (`[models]` in config.toml) or a model name; `[models] default` when left out
        #[arg(long, value_name = "small|large|NAME")]
        model: Option<String>,
        /// The effort its terminal runs at: low, medium, high, xhigh or max; its model's tier's (`[models]`) when left out
        #[arg(long, value_name = "low|medium|high|xhigh|max")]
        effort: Option<String>,
        /// It's work only the owner can do (make an account, sign something): shown as H<n>, no terminal
        /// starts on it, --detail holds the steps and links, and the owner presses Done in the app.
        /// Tasks that wait for it (--waits-for H<n>, or later in its goal) go on after that
        #[arg(long, conflicts_with_all = ["here", "pr", "stack_on", "model", "effort", "lock", "alone", "kind"], requires = "detail")]
        human: bool,
        /// design: its work is a design (concepts, a canvas, mockups). It ends without a PR once its design
        /// is attached (tb attach <link> --kind design), and in a goal the other tasks wait for it
        #[arg(long, value_parser = ["design", "none"], conflicts_with = "pr")]
        kind: Option<String>,
        /// It's different work from the open backlog items whose titles look like it (the board refuses it otherwise)
        #[arg(long = "not-backlog", conflicts_with = "issue")]
        not_backlog: bool,
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
        /// Where it works: shared (the shared checkout), slot (a worktree from the pool), alone, or auto (the board picks)
        #[arg(long, value_parser = ["shared", "slot", "alone", "auto"])]
        lane: Option<String>,
        /// A Jira key to link, `new` for a new ticket, or `none` for no ticket
        #[arg(long)]
        jira: Option<String>,
        /// The new ticket's summary, written to the board's ticket guide (`tb jira`); the title when left out
        #[arg(long = "ticket-summary")]
        ticket_summary: Option<String>,
        /// The new ticket's description in markdown, written to the board's ticket guide (`tb jira`); the detail when left out
        #[arg(long = "ticket-text")]
        ticket_text: Option<String>,
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
        /// How its stacked PR merges: tail (the default) moves it to the main branch once its parent's
        /// merges; head merges it into its parent's branch, and the parent's PR takes it to main
        #[arg(long = "stack-merge", value_parser = ["head", "tail"])]
        stack_merge: Option<String>,
        /// Whether it ends in a PR: yes, no, or auto (its project's default)
        #[arg(long, value_parser = ["yes", "no", "auto"])]
        pr: Option<String>,
        /// design (it ends with its design attached, not a PR) or none
        #[arg(long, value_parser = ["design", "none"])]
        kind: Option<String>,
        /// Planned (waits for its goal's run) or queued (picked up on its own); only a task in one of these two
        #[arg(long, value_parser = ["planned", "queued"])]
        status: Option<String>,
        /// The note for testers the board posts on the ticket at merge, in markdown (`[jira] merged_note`); none clears it
        #[arg(long = "merged-note", value_name = "MARKDOWN|none")]
        merged_note: Option<String>,
        /// One of its project's areas (`.taskboard.toml` at the repo's root), any case, or none
        #[arg(long, value_name = "NAME|none")]
        area: Option<String>,
        /// The model its terminal runs: small or large (`[models]` in config.toml) or a model name; default for `[models] default`
        #[arg(long, value_name = "small|large|NAME|default")]
        model: Option<String>,
        /// The effort its terminal runs at: low, medium, high, xhigh or max; default for its model's tier's
        #[arg(long, value_name = "low|medium|high|xhigh|max|default")]
        effort: Option<String>,
        /// Move it to another project, any case, in any state: on a terminal it carries on there, a PR stays with it,
        /// and one that ran with no terminal waits for Start there
        #[arg(long, value_name = "NAME")]
        project: Option<String>,
    },
    /// Delete a task with its log (only when the owner asks)
    Delete { task: String },
    /// A task: its title, status, goal, PR, detail and latest log entries
    Show {
        task: String,
        /// How many log entries, newest last
        #[arg(long, default_value_t = 5)]
        log: usize,
        /// Print the board's answer as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum BacklogCmd {
    /// Backlog issues, one line each: this terminal's project's open ones (with --goal, every project's)
    List {
        /// The project, or all (this terminal's when left out)
        #[arg(long)]
        project: Option<String>,
        /// G4, or none for issues outside a goal
        #[arg(long)]
        goal: Option<String>,
        #[arg(long, value_parser = ["bug", "gap", "follow", "clean"])]
        kind: Option<String>,
        /// open (the default), task, ticket, defer, drop, or all
        #[arg(long)]
        state: Option<String>,
        /// new (the default), old, or kind
        #[arg(long, value_parser = ["new", "old", "kind"])]
        sort: Option<String>,
        /// Print the board's answer as JSON
        #[arg(long)]
        json: bool,
    },
    /// A backlog issue: its title, state, goal, detail and history
    Show {
        issue: String,
        /// Print the board's answer as JSON
        #[arg(long)]
        json: bool,
    },
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
        /// One of its project's areas (`.taskboard.toml` at the repo's root), any case
        #[arg(long, value_name = "NAME")]
        area: Option<String>,
    },
    /// Change a backlog issue's title, detail, area or project
    Set {
        issue: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        detail: Option<String>,
        /// One of its project's areas (`.taskboard.toml` at the repo's root), any case, or none
        #[arg(long, value_name = "NAME|none")]
        area: Option<String>,
        /// Move it to another project, any case
        #[arg(long, value_name = "NAME")]
        project: Option<String>,
    },
    /// Move backlog issues to another goal (G2), or out of their goal (none), all or none:
    /// tb backlog move B4 B5 G2
    #[command(override_usage = "tb backlog move <ISSUE>... <GOAL>")]
    Move {
        /// The issues, then the goal (G2 or none) last
        #[arg(required = true, num_args = 2.., value_name = "ISSUE")]
        args: Vec<String>,
    },
    /// Make backlog issues into tasks: in their goal (queued while it's running, else planned), or with --board queued on the board,
    /// or with --here (one issue) a task this terminal is on at once.
    /// Several issues (tb backlog task B4 B5) change together or not at all.
    Task {
        #[arg(required = true)]
        issues: Vec<String>,
        #[arg(long, conflicts_with = "here")]
        board: bool,
        /// Planned in their goal even while it's running, so they wait for its next run
        #[arg(long, conflicts_with_all = ["board", "here"])]
        planned: bool,
        /// Take the one issue as a task on this terminal, like tb task new --issue B<n> --here
        #[arg(long)]
        here: bool,
        /// With --here on a terminal that has a task: queue it in this terminal's line, after that task
        #[arg(long, requires = "here", conflicts_with = "now")]
        next: bool,
        /// With --here on a terminal that has a task: switch to it now; that task waits here to resume
        #[arg(long, requires = "here")]
        now: bool,
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
    /// Add a note to a backlog issue's log; its detail stays as it is
    Note { issue: String, text: String },
    /// Show or change the backlog tidy: on a timer, Claude closes open issues that repeat another or aren't real
    Tidy {
        /// Turn it on
        #[arg(long, conflicts_with = "off")]
        on: bool,
        /// Turn it off
        #[arg(long)]
        off: bool,
        /// How often: hourly, daily, or a length like 6h or 2d
        #[arg(long)]
        every: Option<String>,
        /// Tidy now, every project, whether or not an issue was added since the last tidy
        #[arg(long)]
        now: bool,
        /// Forget the changes and go back to config.toml's [backlog] tidy and tidy_every_hours
        #[arg(long)]
        reset: bool,
    },
}

/// `tb backlog tidy --every`: hours from hourly, daily, or a length like 6h or 2d.
fn tidy_hours(s: &str) -> Result<f64, String> {
    let h = match s.trim().to_lowercase().as_str() {
        "hourly" => 1.0,
        "daily" => 24.0,
        "weekly" => 168.0,
        // parse_minutes counts a day as a work day; a timer's day is 24 hours.
        x if ["days", "day", "d"].iter().any(|s| x.strip_suffix(s).is_some_and(|n| n.trim().parse::<f64>().is_ok())) => {
            let n = ["days", "day", "d"].iter().find_map(|s| x.strip_suffix(s).and_then(|n| n.trim().parse::<f64>().ok())).unwrap_or(0.0);
            n * 24.0
        }
        x =>taskboardd::util::parse_minutes(x).map(|m| m as f64 / 60.0).ok_or_else(|| format!("--every takes hourly, daily, or a length like 6h or 2d, not “{s}”."))?,
    };
    if !(1.0..=taskboardd::tidy::MAX_EVERY_HOURS).contains(&h) {
        return Err("--every takes 1 hour to 30 days.".to_string());
    }
    Ok(h)
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
enum CiTokenCmd {
    /// Whether the board has one, and where it's from
    Show,
    /// Store an Azure DevOps personal access token (Build and Test management, read); read from stdin when left out
    Set { token: Option<String> },
    /// Forget the stored token (the env variable is used again, if set)
    Clear,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum ProjectCmd {
    /// A project's PR flow and git remote (every project with no name)
    Show { name: Option<String> },
    /// Change a project: --pr-flow auto (by its git remote), on or off; its PR rules (approvals, expected checks, the ask stage, swaps, the Review step)
    Set {
        name: String,
        #[arg(long = "pr-flow", value_parser = ["auto", "on", "off"])]
        pr_flow: Option<String>,
        /// Approvals its PRs need before they're ready to merge (0: the host's own decision; default: config.toml's)
        #[arg(long)]
        approvals: Option<String>,
        /// A check that must post on every push (repeat for more; none: wait for none; default: config.toml's)
        #[arg(long = "expected-check")]
        expected_check: Vec<String>,
        /// Minutes to wait for the expected checks before going on without them (default: config.toml's)
        #[arg(long = "expected-wait")]
        expected_wait: Option<String>,
        /// After your review, its PRs wait in the ask stage until reviewers are asked (default: config.toml's)
        #[arg(long = "ask-stage", value_parser = ["on", "off", "default"])]
        ask_stage: Option<String>,
        /// Swap a reviewer who hasn't reviewed after [reviewers] swap_after_mins work minutes (default: config.toml's)
        #[arg(long, value_parser = ["on", "off", "default"])]
        swap: Option<String>,
        /// The Review step: off, its PRs go to merge without reviewers (default: on)
        #[arg(long, value_parser = ["on", "off", "default"])]
        review: Option<String>,
        /// Agents merge its PRs once they're approved and green (tb pr merge) (default: config.toml's pr.agents_merge)
        #[arg(long = "agents-merge", value_parser = ["on", "off", "default"])]
        agents_merge: Option<String>,
        /// A green PR waits for your review; off: the board marks it reviewed for you (default: config.toml's pr.owner_review)
        #[arg(long = "owner-review", value_parser = ["on", "off", "default"])]
        owner_review: Option<String>,
        /// Busy terminals the project may have before its queued tasks wait for one to free up (default: config.toml's [terminals] project_max)
        #[arg(long = "max-terminals")]
        max_terminals: Option<String>,
        /// Worktree slots its pool keeps however idle (default: config.toml's [worktrees] min_slots)
        #[arg(long = "min-slots")]
        min_slots: Option<String>,
        /// Worktree slots its pool grows to before a task waits for one (default: config.toml's [worktrees] max_slots)
        #[arg(long = "max-slots")]
        max_slots: Option<String>,
        /// Days a slot sits idle before its build output is cleared (default: config.toml's [worktrees] idle_trim_days)
        #[arg(long = "idle-trim-days")]
        idle_trim_days: Option<String>,
        /// Days a slot sits idle before it's removed (default: config.toml's [worktrees] idle_remove_days)
        #[arg(long = "idle-remove-days")]
        idle_remove_days: Option<String>,
        /// Free disk in GB below which idle slots are cleared sooner (default: config.toml's [worktrees] low_disk_gb)
        #[arg(long = "low-disk-gb")]
        low_disk_gb: Option<String>,
        /// Other projects whose tasks this project's tasks may wait for (tb wait-for), comma-separated; one way only (none: only its own)
        #[arg(long = "waits-on")]
        waits_on: Option<String>,
        /// Releases (tb release cut) for this project (default: config.toml's [releases] on)
        #[arg(long, value_parser = ["on", "off", "default"])]
        releases: Option<String>,
        /// lens sweeps what's pushed to it on its schedule (default: config.toml's [lens.projects], else tb project lens)
        #[arg(long, value_parser = ["on", "off", "default"])]
        lens: Option<String>,
        /// A tag pattern (v*): a new matching tag on the default branch cuts a release named after it, holding the work it contains (none: no tags)
        #[arg(long = "release-tags")]
        release_tags: Option<String>,
        /// A command tb release cut runs in the checkout, with the release name, before recording the release (none: no script)
        #[arg(long = "release-script")]
        release_script: Option<String>,
    },
    /// Whether agents merge approved, green PRs on every project that doesn't say (default: config.toml's pr.agents_merge)
    AgentsMerge {
        #[arg(value_parser = ["on", "off", "default"])]
        value: Option<String>,
    },
    /// Whether green PRs wait for your review on every project that doesn't say; off: the board marks them reviewed (default: config.toml's pr.owner_review)
    OwnerReview {
        #[arg(value_parser = ["on", "off", "default"])]
        value: Option<String>,
    },
    /// Whether lens sweeps every project that doesn't say (default: config.toml's [lens] on)
    Lens {
        #[arg(value_parser = ["on", "off", "default"])]
        value: Option<String>,
    },
}

#[derive(Subcommand)]
enum PrCmd {
    /// Read every PR on the board now, instead of waiting for the next poll
    Check,
    /// The PR's stage, checks and review
    Status { task: Option<String> },
    /// Finished this visit to the PR; the board brings you back when it needs you
    Wait { task: Option<String> },
    /// The PR was merged
    Merged { task: Option<String> },
    /// Check a PR description against the rules tb submit --pr-body uses ([pr_body] in config.toml)
    BodyCheck {
        /// The file (- for stdin)
        file: String,
    },
    /// Count checks that were stopped or never ran as passed (e.g. a hook cancelled the builds); not failures (use not-ours)
    SkipChecks {
        task: Option<String>,
        /// Why they don't need to pass
        #[arg(long, required = true)]
        reason: String,
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
    /// One review thread's whole text and replies, read with the board's account: [T<n>] <thread>
    Thread {
        /// [task] thread
        #[arg(num_args = 1..=2, required = true)]
        args: Vec<String>,
    },
    /// Every thread is answered: ask the reviewers who wanted changes to look again
    Addressed { task: Option<String> },
    /// Ask for reviews through the PR host: the board picks, or --ask, --replace X [--with Y], --drop X
    Reviewers {
        task: Option<String>,
        /// Ask this person (repeat for more)
        #[arg(long)]
        ask: Vec<String>,
        /// Take this reviewer off and ask --with (or the board's pick) in their place
        #[arg(long)]
        replace: Option<String>,
        #[arg(long, requires = "replace")]
        with: Option<String>,
        /// Take this reviewer off the PR
        #[arg(long)]
        drop: Option<String>,
        /// How many to pick (else enough for [reviewers] count)
        #[arg(long)]
        count: Option<usize>,
        /// Show who the board would pick, and ask nobody
        #[arg(long)]
        dry_run: bool,
    },
    /// Merge it once it's approved, green and every thread is answered (the board checks first)
    Merge { task: Option<String> },
    /// Close it without merging (Bitbucket: decline), keeping its branch: only on the owner's word
    Close {
        task: Option<String>,
        /// Why, posted on the PR as a comment
        #[arg(long)]
        reason: Option<String>,
    },
    /// Record the owner's review, so reviewers can be asked: only the owner, or an agent on their word
    Reviewed { task: Option<String> },
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
    /// A failed check only the owner can fix (a missing secret or variable, a permissions error): tell them, and finish this visit
    NeedsOwner {
        task: Option<String>,
        /// What the owner has to do (20-300 characters)
        #[arg(long)]
        reason: String,
        /// The failed check it's about (repeat for more; all of this push's failures when left out)
        #[arg(long)]
        check: Vec<String>,
    },
}

#[derive(Subcommand)]
enum StreamCmd {
    /// Send one event or a batch: {"v": 1, "events": [...]}; each is answered accepted, duplicate or rejected
    Send {
        /// The JSON (stdin when left out or -)
        json: Option<String>,
    },
    /// Whether the board polls a source or its stream drives it, overriding config.toml
    Mode {
        /// prs, builds, jira or qa
        source: String,
        /// board, stream, or default (config.toml's)
        mode: String,
    },
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum FeedCmd {
    /// One PR changed (or one of its builds did): the board reads it again
    Event {
        /// The PR's link (or use --repo and --num, or --task)
        url: Option<String>,
        #[arg(long)]
        repo: Option<String>,
        #[arg(long)]
        num: Option<i64>,
        #[arg(long)]
        task: Option<String>,
        /// pr (the default) or build
        #[arg(long, value_parser = ["pr", "build"])]
        kind: Option<String>,
        /// A build's state: started, running, passed, failed or stopped
        #[arg(long)]
        state: Option<String>,
        /// The commit the build is for
        #[arg(long)]
        head: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        /// The build's CI: github, bitbucket, azure, or a name in [pr_builds.cancel]
        #[arg(long)]
        provider: Option<String>,
        /// The build's link
        #[arg(long = "build-url")]
        build_url: Option<String>,
        /// Who pushed (an email): a push build of the owner's is cancelled too while PR builds are stopped.
        /// On a PR event, who opened the PR: one of the owner's opened by hand is noted as theirs
        #[arg(long)]
        author: Option<String>,
        /// The PR is the owner's (a PR event): its builds are cancelled too while PR builds are stopped
        #[arg(long)]
        mine: bool,
        /// Where the event came from (slack, webhook, …)
        #[arg(long)]
        source: Option<String>,
    },
    /// The feed is alive
    Heartbeat {
        /// The listener is inside its own hours
        #[arg(long, conflicts_with = "idle")]
        active: bool,
        /// The listener is outside its own hours (quiet is expected; asks but a PR's first wait)
        #[arg(long)]
        idle: bool,
        /// With --idle: when it's back (an ISO time)
        #[arg(long = "idle-until")]
        idle_until: Option<String>,
        /// When the listener last connected (an ISO time): a later one starts the settle window again
        #[arg(long = "connected-at")]
        connected_at: Option<String>,
    },
}

#[derive(Subcommand)]
enum PrBuildsCmd {
    /// Cancel every build of the owner's PRs and pushes from now on; their checks count as passed
    Stop {
        #[arg(long)]
        reason: Option<String>,
        /// Whose word it is (the owner when left out)
        #[arg(long)]
        who: Option<String>,
    },
    /// Let the builds run again
    Resume {
        #[arg(long)]
        who: Option<String>,
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

/// The State line's review: "approved" only once it has the approvals it needs; short of them,
/// "1 of 2 approved".
/// Refused starts (`tb words misses`): when, what, which terminal, and the prompt the check read.
fn misses_lines<Tz: chrono::TimeZone>(misses: &Value, now: &chrono::DateTime<Tz>) -> Vec<String>
where
    Tz::Offset: std::fmt::Display,
{
    let rows = misses.as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        return vec!["No start has been refused.".into()];
    }
    let mut lines = vec![];
    for m in &rows {
        let at = m["at"].as_str().and_then(|a| crate::preview::clock(a, now)).unwrap_or_default();
        let term = m["session_name"].as_str().or(m["session_id"].as_str()).unwrap_or("?");
        lines.push(format!("{at} · {} · {term}", m["ref"].as_str().unwrap_or("?")));
        match m["prompt"].as_str() {
            Some(p) => lines.extend(p.lines().map(|l| format!("  > {l}"))),
            None => lines.push("  (no prompt typed in this conversation)".into()),
        }
    }
    lines
}

fn review_words(pr: &Value) -> String {
    let review = pr["review"].as_str().unwrap_or("unknown");
    let (have, need) = (pr["approvals"]["have"].as_i64().unwrap_or(0), pr["approvals"]["need"].as_i64());
    match need {
        Some(n) if review == "pending" && have > 0 => format!("{have} of {n} approved"),
        _ => review.to_string(),
    }
}

/// The Reviewers header's count: " · 1 of 2 approvals", capped at what's needed (never "3 of 2").
fn approvals_words(a: &Value) -> String {
    let have = a["have"].as_i64().unwrap_or(0);
    a["need"].as_i64().map(|n| format!(" · {} of {n} approval{}", have.min(n), if n == 1 { "" } else { "s" })).unwrap_or_default()
}

fn print_pr_status(t: &str, v: &Value) {
    let pr = &v["pr"];
    let live = &v["live"];
    out(&format!("{t} · PR #{} · {}", pr["num"], pr["url"].as_str().unwrap_or("")));
    if let Some(stage) = pr["stage"]["label"].as_str() {
        out(&format!("Stage: {stage}"));
    }
    out(&format!("State: {} · checks: {} · review: {}", pr["state"].as_str().unwrap_or("?"), pr["checks"].as_str().unwrap_or("unknown"), review_words(pr)));
    if let Some(e) = live["read_error"].as_str() {
        out(&format!("Couldn't read it just now ({e}); this is the last read."));
    }
    let rec = &v["record"];
    if let (Some(b), Some(base)) = (rec["branch"].as_str(), rec["base"].as_str()) {
        let rebase = live["rebase"].as_array().is_some_and(|a| !a.is_empty());
        let moved = match (live["conflicts"] == true, live["base_moved"] == true) {
            (true, _) if rebase => format!(" · conflicts with {base}: rebase onto it"),
            (true, _) => format!(" · conflicts with {base}"),
            (false, true) if rebase => format!(" · {base} has moved since this push: rebase onto it"),
            (false, true) if live["conflicts"] == false => format!(" · {base} has moved since this push, no conflicts: don't rebase"),
            _ => String::new(),
        };
        out(&format!("Branch {b} into {base}{moved}"));
        for cmd in live["rebase"].as_array().cloned().unwrap_or_default() {
            out(&format!("  {}", cmd.as_str().unwrap_or("")));
        }
    }
    if let Some(l) = pr["bar"]["stacks_on"]["line"].as_str() {
        out(l);
    }
    if let Some(h) = pr["bar"]["waits_on_stack"].as_str() {
        out(&format!("Waits on its stack: {h}"));
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
                match (f["base_fails"] == true, f["base_compared"].as_str()) {
                    (true, Some("check")) => line += " [base fails this check too]",
                    (true, _) => line += " [base fails this too]",
                    _ => {}
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
                let on_base = |key: &str, s: &str| f[key].as_array().is_some_and(|a| a.iter().any(|x| x.as_str() == Some(s)));
                for s in f["steps"].as_array().cloned().unwrap_or_default() {
                    let s = s.as_str().unwrap_or("");
                    out(&format!("      step: {s}{}", if on_base("base_steps", s) { " [base fails this too]" } else { "" }));
                }
                for s in f["tests"].as_array().cloned().unwrap_or_default() {
                    let s = s.as_str().unwrap_or("");
                    out(&format!("      test: {s}{}", if on_base("base_tests", s) { " [base fails this too]" } else { "" }));
                }
                if let Some(e) = f["error"].as_str() {
                    out(&format!("      couldn't read its steps: {e}"));
                }
                if let Some(e) = f["tests_error"].as_str() {
                    out(&format!("      couldn't read its tests: {e}"));
                }
            }
        }
    }
    if let Some(e) = live["base_error"].as_str() {
        out(&format!("Couldn't compare with the base branch: {e}"));
    }
    let e = &live["expected"];
    let from = match e["source"].as_str() {
        Some("command") => " (from expected_cmd)",
        Some("list") => " (from the project's expected list)",
        Some("earlier") => " (posted on an earlier push)",
        _ => "",
    };
    match e["checks"].as_array() {
        Some(c) if c.is_empty() => out(&format!("Expected checks for this push: none{from}")),
        Some(c) => out(&format!("Expected checks for this push: {}{from}", c.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))),
        None if e["source"] == "command" => out(&format!(
            "Expected checks for this push: expected_cmd couldn't tell{}",
            e["error"].as_str().map(|x| format!(" ({x})")).unwrap_or_default()
        )),
        None => {}
    }
    if let Some(m) = live["expected_missing"].as_array().filter(|m| !m.is_empty()) {
        let names = m.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ");
        match live["expected_wait_mins"].as_f64() {
            Some(w) if live["expected_waited_out"] == true => out(&format!("Expected checks never posted: {names} (stopped waiting after {w} min; they don't hold the merge)")),
            Some(w) => out(&format!("Expected checks not posted yet: {names} (waits up to {w} min)")),
            None => out(&format!("Expected checks not posted yet: {names}")),
        }
    }
    if live["not_ours"].is_object() {
        let n = &live["not_ours"];
        out(&format!("Not this PR's: {} — {}", n["title"].as_str().unwrap_or(""), n["reason"].as_str().unwrap_or("")));
    }
    let reviewers = live["reviewers"].as_array().cloned().unwrap_or_default();
    if !reviewers.is_empty() {
        out(&format!("Reviewers{}:", approvals_words(&live["approvals"])));
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
            for r in th["replies"].as_array().cloned().unwrap_or_default() {
                let who = r["author_name"].as_str().filter(|n| !n.is_empty()).or(r["author"].as_str()).unwrap_or("");
                out(&format!("      ↳ {who}: {}", r["text"].as_str().unwrap_or("")));
            }
        }
        out(&format!("  Whole text of one: tb pr thread {t} <id>"));
    } else if rec["comments"].as_i64().unwrap_or(0) > 0 && !rec["threads"].is_array() {
        out(&format!("Review comments from others: {}", rec["comments"]));
    }
    if let Some(e) = live["tasks_error"].as_str() {
        out(&format!("PR tasks: couldn't read tasks ({e})"));
    }
    if v["watched"] != true {
        out("The board doesn't watch this PR's host; read it with the host's own tools.");
    }
    if let Some(b) = live["blockers"].as_array().filter(|b| !b.is_empty()) {
        out(&format!("Before it can merge: {}", b.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("; ")));
    }
    out(&format!("Merging: {}", if v["agents_merge"] == true { "agents may merge once it's approved and green (tb pr merge)" } else { "the owner merges it" }));
}

/// `tb pr thread`: one thread's whole text, then each reply's.
fn print_pr_thread(v: &Value) {
    let th = &v["thread"];
    let name = |x: &Value| x["author_name"].as_str().filter(|n| !n.is_empty()).or(x["author"].as_str()).unwrap_or("").to_string();
    let whole = |x: &Value| x["body"].as_str().filter(|b| !b.is_empty()).or(x["text"].as_str()).unwrap_or("").to_string();
    let at = th["path"].as_str().map(|p| format!(" · {p}{}", th["line"].as_i64().map(|l| format!(":{l}")).unwrap_or_default())).unwrap_or_default();
    let state = if th["resolved"] == true {
        "resolved"
    } else if v["open"] == true {
        "open"
    } else {
        "answered"
    };
    let outdated = if th["outdated"] == true { ", outdated" } else { "" };
    out(&format!("Thread {} · {}{}{at} · {state}{outdated}", th["id"].as_str().unwrap_or(""), name(th), if th["kind"] == "task" { " · task" } else { "" }));
    if let Some(u) = th["url"].as_str() {
        out(u);
    }
    out("");
    out(&whole(th));
    for r in th["replies"].as_array().cloned().unwrap_or_default() {
        out("");
        out(&format!("↳ {}:", name(&r)));
        for l in whole(&r).lines() {
            out(&format!("  {l}"));
        }
    }
}

/// `tb pr check`'s answer to `POST /prs/refresh`.
fn pr_check_line(v: &Value) -> String {
    match v["changed"].as_i64().unwrap_or(0) {
        0 => "Checked the PRs: nothing changed.".to_string(),
        1 => "Checked the PRs: 1 task changed.".to_string(),
        n => format!("Checked the PRs: {n} tasks changed."),
    }
}

/// `tb qa [list|waiting] [--goal G2]`'s read of `GET /qa-comments`.
fn qa_list_url(limit: i64, waiting: bool, goal: Option<&str>) -> String {
    let mut url = format!("/qa-comments?limit={limit}");
    if waiting {
        url.push_str("&waiting=1");
    }
    if let Some(g) = goal.filter(|g| !g.trim().is_empty()) {
        url.push_str(&format!("&goal={}", g.trim()));
    }
    url
}

fn plural_threads(v: &Value) -> String {
    match v["open_threads"].as_i64().unwrap_or(0) {
        0 => "No threads are".to_string(),
        1 => "1 thread is still".to_string(),
        n => format!("{n} threads are still"),
    }
}

/// Set once stdout's reader has gone (`tb take | head`, `tb step again … | true`).
static STDOUT_GONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Prints a line. A write that fails (the reader has gone) is ignored, and later lines aren't tried: tb
/// carries on with the work and its report, cleans up after itself, and exits as it would have, rather
/// than panicking or stopping before the work is done.
/// A failed call as a sentence: the board's refusal, a board that isn't there, or one that `went_quiet`
/// (it took the request and may still be working on it, so a retry could do it twice).
fn unanswered(e: CallError, went_quiet: &str) -> String {
    match e {
        CallError::Refused(m) => m,
        CallError::Unreachable(e) => format!("the board isn't answering ({e}). Is `taskboard serve` running?"),
        CallError::TimedOut(e) => format!(
            "the board {went_quiet} ({e}). It may still be working on it: check before running it again (tb pr status shows where a PR is)."
        ),
    }
}

fn out(line: &str) {
    out_to(&mut std::io::stdout(), &STDOUT_GONE, line);
}

fn out_to(w: &mut impl std::io::Write, gone: &std::sync::atomic::AtomicBool, line: &str) {
    use std::sync::atomic::Ordering;
    if gone.load(Ordering::Relaxed) {
        return;
    }
    if writeln!(w, "{}", line.trim_end_matches('\n')).and_then(|_| w.flush()).is_err() {
        gone.store(true, Ordering::Relaxed);
    }
}

fn task_ref(v: &str) -> Result<String, String> {
    let s = v.trim();
    // H<n> is a human task's ref for the same task.
    let digits = s.trim_start_matches(['T', 't', 'H', 'h']);
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
    let names: Vec<String> = ["task", "n", "wave", "goal", "jira", "device", "target", "device2", "target2"].iter().map(|n| format!("{{{b}{n}{b:#}}}")).collect();
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

/// The task backlog item `issue` became, making it one first (planned in its goal, else waiting on the
/// board for its start) while it's still open.
fn item_task(c: &Ctx, issue: &str) -> Result<String, String> {
    let b = issue_ref(issue)?;
    let item = c.call("GET", &format!("/backlog/{b}"), None)?;
    if let (Some("task"), Some(t)) = (item["state"].as_str(), item["task_ref"].as_str()) {
        return Ok(t.to_string());
    }
    let v = backlog_bulk(c, "task", &[b.clone()], json!({"where": "goal", "pickup": {"mode": "manual"}}))?;
    let t = v["tasks"][0]["ref"].as_str().ok_or_else(|| format!("The board didn't say which task {b} became."))?.to_string();
    out(&format!("Made {b} into {t}."));
    Ok(t)
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
            Err(CallError::Unreachable(_) | CallError::TimedOut(_)) => {
                client::spool_write(&self.cfg, &body).map_err(|e| format!("the board isn't answering and the report couldn't be saved: {e}"))?;
                Ok(None)
            }
        }
    }

    /// Posts a report that runs long on the board (`tb submit`, `tb question`) on the progress stream,
    /// printing each stage. Spooled (None) only when the board can't be reached: one that went quiet has
    /// the report and may still be working on it, so a replay could do it twice.
    fn report_progress(&self, event: &str, fields: Value, task: Option<&str>) -> Result<Option<Value>, String> {
        let body = self.body(event, fields, task);
        match client::request_progress(&self.cfg, "POST", "/report", Some(&body), TB_TIMEOUT, |stage| out(&format!("· {stage}"))) {
            Ok(v) => Ok(Some(v)),
            Err(CallError::Unreachable(_)) => {
                client::spool_write(&self.cfg, &body).map_err(|e| format!("the board isn't answering and the report couldn't be saved: {e}"))?;
                Ok(None)
            }
            Err(e) => Err(unanswered(e, &format!("went quiet for {TB_TIMEOUT}s"))),
        }
    }

    /// Posts a report that only asks: never spooled, since a replay would mark the terminal active for nothing.
    fn ask(&self, event: &str) -> Result<Value, String> {
        match client::request(&self.cfg, "POST", "/report", Some(&self.body(event, json!({}), None)), TB_TIMEOUT) {
            Ok(v) => Ok(v),
            Err(CallError::Refused(m)) => Err(m),
            Err(CallError::Unreachable(_)) => Err("The board isn't reachable.".into()),
            Err(CallError::TimedOut(_)) => Err("The board didn't answer in time; it may still be busy.".into()),
        }
    }

    /// How the board names this terminal in an issue's history.
    fn who(&self) -> String {
        if self.session.is_empty() { "tb".to_string() } else { format!("terminal {}", self.session.chars().take(8).collect::<String>()) }
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        let body = self.credited_here(method, body);
        match client::request(&self.cfg, method, path, body.as_ref(), TB_TIMEOUT) {
            Ok(v) => Ok(v),
            Err(CallError::Refused(m)) => Err(m),
            Err(e) => Err(unanswered(e, &format!("didn't answer within {TB_TIMEOUT}s"))),
        }
    }

    /// A long command on the board's progress stream: prints each stage as the board finishes it, and
    /// only gives up when the board goes quiet for `TB_TIMEOUT` (a stage that hangs).
    fn call_progress(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        let body = self.credited_here(method, body);
        client::request_progress(&self.cfg, method, path, body.as_ref(), TB_TIMEOUT, |stage| out(&format!("· {stage}")))
            .map_err(|e| unanswered(e, &format!("went quiet for {TB_TIMEOUT}s")))
    }

    /// A change names this terminal (`by_session`), so the board credits it here rather than to the owner.
    fn credited_here(&self, method: &str, body: Option<Value>) -> Option<Value> {
        if method == "GET" || self.session.is_empty() {
            return body;
        }
        let mut b = body.unwrap_or_else(|| json!({}));
        if let Value::Object(m) = &mut b {
            m.entry("by_session").or_insert_with(|| json!(self.session));
        }
        Some(b)
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

/// What `tb pr addressed` says. A retaken task (`tb take`, still working) only finishes with `tb submit`,
/// so it isn't told to stop.
fn addressed_line(t: &str, asked: &[&str], status: &str) -> String {
    let did = if asked.is_empty() { format!("Recorded {t}'s review as addressed.") } else { format!("Asked {} to review {t}'s PR again.", asked.join(", ")) };
    if matches!(status, "working" | "needs") {
        format!("{did} {t} is still taken: push your changes, then run tb submit \"<summary>\" to finish it.")
    } else {
        format!("{did} The board watches the PR and brings this conversation back when it needs you. You can stop here.")
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

/// Pins a task or goal tb just made under this Midna terminal's links button (⌘L), so a click opens it in
/// Taskboard. Runs in the background and never fails the command: outside Midna, without midna, or when
/// its daemon is down, nothing happens.
fn midna_pin(r: &str, title: &str) {
    let Some(args) = midna_pin_args(std::env::var("MIDNA_SESSION").ok().as_deref(), r, title) else { return };
    if let Some(m) = client::midna_path() {
        let _ = std::process::Command::new(m)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

/// `midna` arguments that pin `r` (T42 or G3), or None outside a Midna terminal or for another ref.
fn midna_pin_args(session: Option<&str>, r: &str, title: &str) -> Option<Vec<String>> {
    session.filter(|s| !s.is_empty())?;
    let kind = match r.chars().next()? {
        'T' => "task",
        'G' => "goal",
        _ => return None,
    };
    let title = if title.trim().is_empty() { r.to_string() } else { format!("{r} · {}", title.trim()) };
    Some(vec!["links".into(), "add".into(), format!("taskboard://{kind}/{r}"), "--title".into(), title])
}

/// "works in a worktree slot", "holds local-core" and "runs alone in its goal".
fn lock_bits(t: &Value) -> Vec<String> {
    let mut bits = vec![];
    match t["lane"]["set"].as_str() {
        Some("shared") => bits.push("works in the shared checkout".into()),
        Some("slot") => bits.push("works in a worktree slot".into()),
        _ => {}
    }
    if let Some(l) = t["locks"].as_array().filter(|a| !a.is_empty()) {
        bits.push(format!("holds {}", l.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")));
    }
    if let Some(a) = t["alone"].as_str() {
        bits.push(alone_text(a).into());
    }
    bits
}

/// A goal's lane line for `tb goal show`; None on auto, where the board picks per task.
/// The goal's lane line for `tb goal show`; nothing on auto.
fn lane_text(lane: &str) -> Option<&'static str> {
    match lane {
        "slot" => Some("Lane: slot. Its tasks work in worktree slots from the pool."),
        "shared" => Some("Lane: shared. Its tasks work in the shared checkout."),
        "alone" => Some("Lane: alone. Each task runs with nothing else in the goal beside it."),
        _ => None,
    }
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

/// Why `tb start G<n>` or `tb goal set G<n> --run` can't run a goal outside a Midna terminal: only a
/// human's word there runs it, and outside one only Run on the board does.
fn goal_run_needs_terminal(cmd: &str, g: &str) -> String {
    format!("{cmd} only works in a Midna terminal, on a human's word there. Outside one, a human can press Run on {g} on the board.")
}

/// After a goal runs: the parked tasks it resumed, and why the rest of its current wave still waits.
fn out_play_waits(v: &Value) {
    let r = v["resumed_now"].as_i64().unwrap_or(0);
    if r > 0 {
        out(&format!("Resumed {r} parked task{}.", if r == 1 { "" } else { "s" }));
    }
    for line in v["hours_wait"].as_str().into_iter().chain(v["waiting"].as_array().into_iter().flatten().filter_map(|w| w.as_str())) {
        out(&format!("{line}."));
    }
}

/// After `tb submit` or `tb fail`: the next task in this terminal's line, which the agent carries on with.
/// What `tb start T<n>` did, from the task the board sends back: a `task.starting` hook may have
/// skipped it, so it's only "opening" when the board says it's starting.
fn start_said(r: &str, queue: bool, v: &Value) -> String {
    let status = v["status"].as_str().unwrap_or("");
    let why = v["summary"].as_str().or(v["latest"].as_str()).filter(|s| !s.is_empty());
    if status == "done" {
        return match why {
            Some(w) => format!("{r} didn't start: {}.", w.trim_end_matches('.')),
            None => format!("{r} didn't start: it's done."),
        };
    }
    if v["starting"] == true || (status == "queued" && queue) {
        return if queue { format!("{r} starts in a new terminal once its project has a free terminal.") } else { format!("Opening {r} in a new terminal now.") };
    }
    match v["who"].as_str().filter(|s| !s.is_empty()) {
        Some(who) if matches!(status, "working" | "needs") => format!("{r} is on {who}."),
        _ => format!("{r} is {status}."),
    }
}

fn then_next(v: &Value) -> String {
    v["context"].as_str().filter(|c| !c.trim().is_empty()).map(|c| format!("\n\n{c}")).unwrap_or_default()
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

fn planning_cmd(c: &Ctx, project: Option<String>) -> Result<i32, String> {
    let q = match &project {
        Some(p) => format!("project={}", qv(p)),
        None => format!("cwd={}", qv(&c.cwd)),
    };
    let v = c.call("GET", &format!("/planning?{q}"), None)?;
    for s in v["skipped"].as_array().cloned().unwrap_or_default() {
        out(&format!("Not used: the {} planning file {}: {}", s["from"].as_str().unwrap_or(""), s["path"].as_str().unwrap_or(""), s["error"].as_str().unwrap_or("")));
    }
    let whose = match (v["from"].as_str(), v["project"].as_str()) {
        (Some("project"), Some(p)) => format!("{p}'s planning file"),
        (Some("board"), _) => "the board's planning file".to_string(),
        _ => "the built-in rules".to_string(),
    };
    let path = v["path"].as_str().map(|p| format!(" ({p})")).unwrap_or_default();
    out(&format!("Slice goals into tasks by {whose}{path}:\n{}", v["text"].as_str().unwrap_or("").trim()));
    if let Some(m) = models_line(&v["models"]) {
        out(&format!("\n{m}"));
    }
    if let Some(l) = v["lanes"].as_str() {
        out(&format!("\nLanes, where each task works while its wave mates run:\n{l}"));
    }
    let lessons: Vec<&str> = v["lessons"].as_array().into_iter().flatten().filter_map(|l| l.as_str()).collect();
    if !lessons.is_empty() {
        out(&format!("\nWhat earlier goals taught about planning here:\n{}", lessons.join("\n")));
    }
    Ok(0)
}

/// "Models: small runs sonnet at medium effort, large runs opus at high; a task without one runs opus at
/// high." Nothing while `[models]` is off.
fn models_line(m: &Value) -> Option<String> {
    let at = |k: &str| m[k].as_str().map(|e| format!(" at {e}")).unwrap_or_default();
    let (small, large, default) = (m["small"].as_str()?, m["large"].as_str()?, m["default"].as_str()?);
    Some(format!(
        "Models (tb task new --model small|large, --effort low|medium|high|xhigh|max to change its effort): small runs {small}{}, \
         large runs {large}{}; a task without one runs {default}{}.",
        at("small_effort"),
        at("large_effort"),
        at("default_effort")
    ))
}

fn planning_lessons_cmd(c: &Ctx, project: Option<String>, accept: bool) -> Result<i32, String> {
    let v = if accept {
        let body = match &project {
            Some(p) => json!({"project": p}),
            None => json!({"cwd": c.cwd}),
        };
        c.call("POST", "/planning/lessons", Some(body))?
    } else {
        let q = match &project {
            Some(p) => format!("project={}", qv(p)),
            None => format!("cwd={}", qv(&c.cwd)),
        };
        c.call("GET", &format!("/planning/lessons?{q}"), None)?
    };
    let (who, path) = (v["project"].as_str().unwrap_or(""), v["path"].as_str().unwrap_or(""));
    let add: Vec<&str> = v["add"].as_array().into_iter().flatten().filter_map(|l| l.as_str()).collect();
    if add.is_empty() {
        out(&format!("{who}'s planning file ({path}) already has every kept planning lesson."));
        return Ok(0);
    }
    if v["written"] == true {
        out(&format!("Wrote {} lesson{} into {path}:\n{}", add.len(), if add.len() == 1 { "" } else { "s" }, add.join("\n")));
        return Ok(0);
    }
    let verb = if v["exists"] == true { "Add to" } else { "Make" };
    out(&format!("{verb} {path}, {who}'s planning file, with these lines:\n{}", add.join("\n")));
    if v["exists"] != true {
        out("It starts from the planning rules in force now.");
    }
    out(&format!("Write it with: tb planning --project {who} --accept"));
    Ok(0)
}

fn share(v: &Value) -> String {
    v.as_f64().map(|x| format!("{:.0}%", x * 100.0)).unwrap_or_else(|| "–".into())
}

fn per_task(v: &Value) -> String {
    v.as_f64().map(|x| format!("{x:.1}")).unwrap_or_else(|| "–".into())
}

fn times(v: &Value) -> String {
    v.as_f64().map(|x| format!("{x:.1}×")).unwrap_or_else(|| "–".into())
}

/// One line of plan-quality numbers (a month's, a project's or a goal's).
fn quality_line(head: &str, v: &Value) -> String {
    let n = |k: &str| v[k].as_f64().unwrap_or(0.0);
    let tasks = n("tasks");
    let rate = |k: &str| if tasks > 0.0 { json!(n(k) / tasks) } else { Value::Null };
    let est = n("est_min");
    let goals = v["goals"].as_i64().map(|g| format!("{g} goal{}, ", if g == 1 { "" } else { "s" })).unwrap_or_default();
    format!(
        "{head}  {goals}{tasks} task{}  added late {}  failed/retried {}  review rounds {}/task  questions {}/task  time vs estimate {}",
        if tasks == 1.0 { "" } else { "s" },
        share(&rate("added_late")),
        share(&rate("failed")),
        per_task(&rate("review_rounds")),
        per_task(&rate("questions")),
        times(&if est > 0.0 { json!(n("work_min") / est) } else { Value::Null }),
    )
}

fn quality_cmd(c: &Ctx, project: Option<String>, all: bool, goals: bool) -> Result<i32, String> {
    let q = match (all, &project) {
        (true, _) => "project=all".to_string(),
        (_, Some(p)) => format!("project={}", qv(p)),
        _ => format!("cwd={}", qv(&c.cwd)),
    };
    let v = c.call("GET", &format!("/quality?{q}"), None)?;
    let ps = v["projects"].as_array().cloned().unwrap_or_default();
    if ps.is_empty() {
        out("No finished goals yet.");
        return Ok(0);
    }
    for (i, p) in ps.iter().enumerate() {
        if i > 0 {
            out("");
        }
        out(&quality_line(p["project"].as_str().unwrap_or(""), &p["total"]));
        for m in p["months"].as_array().into_iter().flatten() {
            out(&quality_line(&format!("  {}", month_name(m["month"].as_str().unwrap_or(""))), m));
        }
        if goals {
            for g in p["goals"].as_array().into_iter().flatten() {
                let head = format!("  {} {}", g["goal"].as_str().unwrap_or(""), g["name"].as_str().unwrap_or(""));
                out(&quality_line(&head, g));
            }
        }
    }
    Ok(0)
}

fn review_cmd(c: &Ctx, target: Option<String>, since: Option<String>, week: Option<String>, save: bool, list: bool) -> Result<i32, String> {
    if list {
        let v = c.call("GET", "/reviews", None)?;
        let rows = v["reviews"].as_array().cloned().unwrap_or_default();
        if rows.is_empty() {
            out("No reviews kept yet.");
        }
        for r in rows {
            let seen = if r["seen_at"].is_string() { "" } else { "  (not opened)" };
            out(&format!("{:>4}  {}  {}{seen}", r["id"], review_period(&r), r["headline"].as_str().unwrap_or("")));
        }
        return Ok(0);
    }
    if let Some(t) = target.as_deref() {
        if (since.is_some() || week.is_some() || save) && !t.chars().all(|ch| ch.is_ascii_digit()) {
            return Err("--since, --week and --save make a new review; leave out the task.".into());
        }
        if t.chars().all(|ch| ch.is_ascii_digit()) {
            let v = c.call("GET", &format!("/reviews/{t}"), None)?;
            print_review(&v["data"]);
            return Ok(0);
        }
        let v = c.call("GET", &format!("/tasks/{}/spans", task_ref(t)?), None)?;
        print_task_spans(&v);
        return Ok(0);
    }
    let v = if save {
        let r = c.call("POST", "/reviews", Some(json!({"since": since, "week": week})))?;
        out(&format!("Kept as review {}; the owner was told.", r["id"]));
        r
    } else {
        let mut q = vec![];
        if let Some(s) = &since {
            q.push(format!("since={}", qv(s)));
        }
        if let Some(w) = &week {
            q.push(format!("week={}", qv(w)));
        }
        c.call("GET", &format!("/reviews/now?{}", q.join("&")), None)?
    };
    print_review(&v["data"]);
    Ok(0)
}

/// Seconds as "2.5 h", "14 min" or "40 s".
fn took(secs: f64) -> String {
    let secs = secs.abs();
    if secs >= 3600.0 {
        format!("{:.1} h", secs / 3600.0)
    } else if secs >= 60.0 {
        format!("{} min", (secs / 60.0).round() as i64)
    } else {
        format!("{} s", secs.round() as i64)
    }
}

fn signed(secs: f64) -> String {
    format!("{}{}", if secs < 0.0 { "-" } else { "+" }, took(secs))
}

fn local_when(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|d| d.with_timezone(&chrono::Local).format("%b %-d, %-I:%M %p").to_string())
        .unwrap_or_else(|_| at.to_string())
}

fn review_period(r: &Value) -> String {
    format!("{} to {}", local_when(r["period_start"].as_str().unwrap_or("")), local_when(r["period_end"].as_str().unwrap_or("")))
}

fn secs(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

fn print_review(r: &Value) {
    let total = secs(&r["seconds"]);
    out(&format!("{}: {} of agent time", review_period(r), took(total)));
    if total <= 0.0 {
        return;
    }
    let change = &r["change"];
    if change.is_object() {
        out(&format!("Since review {}: {} agent time, {} waiting on you, {:+} timeouts, {:+} reruns", change["review"], signed(secs(&change["seconds"])),
            signed(secs(&change["owner_wait"])), change["timeouts"].as_i64().unwrap_or(0), change["reruns"].as_i64().unwrap_or(0)));
    }
    out("");
    out("By category");
    for x in r["categories"].as_array().into_iter().flatten() {
        out(&format!("  {:<16} {:>9}  {:>3.0}%", x["name"].as_str().unwrap_or(""), took(secs(&x["seconds"])), secs(&x["seconds"]) / total * 100.0));
    }
    out("By project");
    for x in r["projects"].as_array().into_iter().flatten() {
        let top = x["categories"][0]["name"].as_str().unwrap_or("");
        out(&format!("  {:<16} {:>9}  mostly {top}", x["name"].as_str().unwrap_or(""), took(secs(&x["seconds"]))));
    }
    let rows = |title: &str, list: &Value, line: &dyn Fn(&Value) -> String| {
        let items = list.as_array().cloned().unwrap_or_default();
        if !items.is_empty() {
            out(title);
            for x in &items {
                out(&format!("  {}", line(x)));
            }
        }
    };
    rows("Slowest tasks", &r["slowest_tasks"], &|x| {
        format!("{:<6} {:>9}  mostly {:<12} {}", x["task"].as_str().unwrap_or(""), took(secs(&x["seconds"])), x["top_category"].as_str().unwrap_or(""), x["title"].as_str().unwrap_or(""))
    });
    rows("Slowest repeated commands", &r["repeated"], &|x| {
        format!("median {:>7}  p90 {:>7}  x{:<3} {}", took(secs(&x["median"])), took(secs(&x["p90"])), x["runs"], x["command"].as_str().unwrap_or(""))
    });
    rows("Timeouts", &r["timeouts"]["commands"], &|x| format!("x{:<3} {:>9}  {}", x["count"], took(secs(&x["seconds"])), x["command"].as_str().unwrap_or("")));
    rows("Failed, then rerun", &r["reruns"], &|x| {
        let end = if x["passed"] == true { "then passed" } else { "never passed" };
        format!("{:<6} {} runs {:>9}  {end}  {}", x["task"].as_str().unwrap_or(""), x["runs"], took(secs(&x["seconds"])), x["command"].as_str().unwrap_or(""))
    });
    let wait = &r["owner_wait"];
    if secs(&wait["seconds"]) > 0.0 {
        let why: Vec<String> = wait["reasons"].as_array().into_iter().flatten().map(|x| format!("{} {}", x["name"].as_str().unwrap_or(""), took(secs(&x["seconds"])))).collect();
        out(&format!("Waiting on you: {} ({})", took(secs(&wait["seconds"])), why.join(", ")));
    }
    rows("Suggested fixes", &r["suggestions"], &|x| format!("{}: {}", x["title"].as_str().unwrap_or(""), x["detail"].as_str().unwrap_or("")));
}

fn print_task_spans(v: &Value) {
    out(&format!("{} {}: {} in commands", v["task"].as_str().unwrap_or(""), v["title"].as_str().unwrap_or(""), took(secs(&v["seconds"]))));
    let cats: Vec<String> = v["categories"].as_array().into_iter().flatten().map(|x| format!("{} {}", x["name"].as_str().unwrap_or(""), took(secs(&x["seconds"])))).collect();
    if !cats.is_empty() {
        out(&cats.join(", "));
    }
    for s in v["spans"].as_array().into_iter().flatten() {
        let mut marks = vec![];
        if s["timed_out"] == true {
            marks.push("timed out".to_string());
        } else if let Some(code) = s["exit_code"].as_i64().filter(|c| *c != 0) {
            marks.push(format!("exit {code}"));
        }
        for (k, word) in [("background", "background"), ("refused", "refused"), ("subagent", "subagent")] {
            if s[k] == true {
                marks.push(word.to_string());
            }
        }
        let marks = if marks.is_empty() { String::new() } else { format!("  ({})", marks.join(", ")) };
        out(&format!("  {}  {:>9}  {:<15} {}{marks}", local_when(s["started_at"].as_str().unwrap_or("")), took(secs(&s["seconds"])), s["category"].as_str().unwrap_or(""), s["command"].as_str().unwrap_or("")));
    }
}

/// "2026-10" as "Oct 2026".
fn month_name(m: &str) -> String {
    chrono::NaiveDate::parse_from_str(&format!("{m}-01"), "%Y-%m-%d").map(|d| d.format("%b %Y").to_string()).unwrap_or_else(|_| m.to_string())
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
        for (key, what) in [("ticket_template", "Ticket template"), ("ticket_guide", "Ticket guide")] {
            let f = &v[key];
            let Some(path) = f["path"].as_str() else { continue };
            match (f["error"].as_str(), f["text"].as_str()) {
                (Some(e), _) => out(&format!("{what}: {e}")),
                (None, Some(text)) => out(&format!(
                    "{what} ({path}): write each new ticket to it and give it with --ticket-summary and --ticket-text alongside --jira new.\n{}",
                    text.trim()
                )),
                (None, None) => out(&format!("{what}: {path}")),
            }
        }
        if v["merged_note"] == true {
            let to = v["merged_status"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or("its merged status");
            out(&format!(
                "Note for testers: a task that ships a PR on a ticket finishes with tb submit \"<summary>\" --merged-note \"<markdown>\"; at merge the board posts it on the ticket, then moves it to {to}."
            ));
            let f = &v["merged_note_guide"];
            match (f["path"].as_str(), f["error"].as_str(), f["text"].as_str()) {
                (Some(_), Some(e), _) => out(&format!("Note guide: {e}")),
                (Some(path), None, Some(text)) => out(&format!("Note guide ({path}): write the note to it.\n{}", text.trim())),
                _ => {}
            }
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
            Some((k, v)) if ["key", "status", "found", "added", "product"].contains(&k.to_lowercase().as_str()) => {
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

/// "Took pixel-7 out of the pool and out of T4's and G2's needs." for `tb device remove`.
fn removed_line(name: &str, v: &Value) -> String {
    let from: Vec<String> = v["dropped_from"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).map(|r| format!("{r}'s")).collect()).unwrap_or_default();
    match from.split_last() {
        None => format!("Took {name} out of the pool."),
        Some((last, [])) => format!("Took {name} out of the pool and out of {last} needs."),
        Some((last, rest)) => format!("Took {name} out of the pool and out of {} and {last} needs.", rest.join(", ")),
    }
}

/// "dev-a (Android emulator, emulator-5554) · android · lent to T4" for `tb devices`.
fn device_line(d: &Value) -> String {
    let tags: Vec<&str> = d["tags"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    let mut line = d["label"].as_str().or(d["name"].as_str()).unwrap_or("").to_string();
    if !tags.is_empty() {
        line += &format!(" · {}", tags.join(", "));
    }
    line += &match d["held_by"]["ref"].as_str() {
        // Switched off while lent: the task keeps it until it's done, and then it isn't lent again.
        Some(r) if d["off"] == true => format!(" · off, still lent to {r} {}", d["held_by"]["title"].as_str().unwrap_or("")),
        Some(r) => format!(" · lent to {r} {}", d["held_by"]["title"].as_str().unwrap_or("")),
        None if d["off"] == true => " · off".to_string(),
        None => " · free".to_string(),
    };
    if let Some(g) = d["reserved_for"].as_str() {
        line += &format!(" · reserved for {g}");
    }
    if let Some(n) = d["note"].as_str().filter(|n| !n.is_empty()) {
        line += &format!(" · {n}");
    }
    if d["can_focus"] == true {
        line += " · can focus";
    }
    if let Some(x) = d["start_cmd"].as_str() {
        line += &format!(" · start: {x}");
    }
    if let Some(x) = d["stop_cmd"].as_str() {
        line += &format!(" · stop: {x}");
    }
    line
}

/// "pixel-7 · for measure · reserved · android · lent to T4" for `tb goal devices`.
fn goal_device_line(d: &Value) -> String {
    let mut line = d["label"].as_str().or(d["name"].as_str()).unwrap_or("").to_string();
    if let Some(p) = d["purpose"].as_str() {
        line += &format!(" · for {p}");
    }
    if d["reserved"] == true {
        line += " · reserved";
    }
    let rest = device_line(&json!({"name": "", "tags": d["tags"], "held_by": d["held_by"], "off": d["off"], "note": d["note"]}));
    line + &rest
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

fn slot_cmd(c: &Ctx, action: SlotCmd) -> Result<i32, String> {
    match action {
        SlotCmd::Claim { task, bring, bring_hunk } => {
            let t = c.pr_task(task)?;
            slot_claim(c, &t, bring, &bring_hunk)?;
        }
        SlotCmd::Release { task } => {
            let t = c.pr_task(task)?;
            let v = c.call("POST", &format!("/tasks/{t}/slot/release"), Some(json!({})))?;
            out(&if v["released"] == true { format!("{t} gave back its worktree slot.") } else { format!("{t} holds no worktree slot.") });
        }
    }
    Ok(0)
}

/// Claims `t` a slot, bringing `bring`'s files and the `hunks` picked (relative to here) from the
/// shared checkout, and says where it works now and what came along.
fn slot_claim(c: &Ctx, t: &str, bring: Option<Vec<String>>, hunks: &[String]) -> Result<(), String> {
    let mut body = json!({});
    let here = std::env::current_dir().unwrap_or_default();
    if let Some(files) = bring {
        body["bring"] = json!(files.iter().map(|f| here.join(f).to_string_lossy().to_string()).collect::<Vec<_>>());
    }
    if !hunks.is_empty() {
        body["bring_hunk"] = json!(hunks.iter().map(|h| if h == "none" { h.clone() } else { here.join(h).to_string_lossy().to_string() }).collect::<Vec<_>>());
    }
    let v = c.call("POST", &format!("/tasks/{t}/slot/claim"), Some(body))?;
    out(&slot_claimed(t, &v));
    Ok(())
}

fn slot_claimed(t: &str, v: &Value) -> String {
    let path = v["path"].as_str().unwrap_or("");
    let mut lines = vec![format!("{t} works in {} ({path}). Work there from now on: cd {path}", v["slot"].as_str().unwrap_or(""))];
    let list = |k: &str| v[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    if !list("brought").is_empty() {
        lines.push(format!("Brought from the shared checkout: {} (the patch is kept at {}).", list("brought"), v["patch"].as_str().unwrap_or("")));
    }
    if !list("split").is_empty() {
        lines.push(format!("Only this task's hunks came from {}; the other tasks' edits stay in the shared checkout.", list("split")));
    }
    if !list("left_hunks").is_empty() {
        lines.push(format!("Hunks no task's edit log has, left in the shared checkout: {}.", list("left_hunks")));
    }
    if !list("left").is_empty() {
        lines.push(format!("Left behind in the shared checkout, still uncommitted: {}. Bring them too with tb slot claim --bring <files>, or commit them there by name.", list("left")));
    }
    if let (Some(sha), Some(b)) = (v["detached_at"].as_str(), v["branch"].as_str()) {
        lines.push(format!("The slot is detached at the tip of {b} ({}), which the shared checkout has checked out. Make a new branch there: git switch -c <new branch>.", &sha[..sha.len().min(10)]));
    }
    lines.join("\n")
}

fn disk_size(kb: i64) -> String {
    match kb {
        k if k >= 1024 * 1024 => format!("{:.1} GB", k as f64 / (1024.0 * 1024.0)),
        k if k >= 1024 => format!("{} MB", k / 1024),
        k => format!("{k} KB"),
    }
}

fn device_cmd(c: &Ctx, action: DeviceCmd) -> Result<i32, String> {
    match action {
        DeviceCmd::Add { name, tags, focus, note, kind, target, start, stop } => {
            let v = c.call(
                "POST",
                "/devices",
                Some(json!({"name": name, "tags": lock_arg(&tags), "focus": focus, "note": note, "kind": kind, "target": target, "start_cmd": start, "stop_cmd": stop})),
            )?;
            out(&format!("Added {}.", device_line(&v)));
        }
        DeviceCmd::Set { name, rename, tags, focus, note, kind, target, start, stop, off, on } => {
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
            for (k, v) in [("kind", kind), ("target", target), ("start_cmd", start), ("stop_cmd", stop)] {
                if let Some(x) = v {
                    b[k] = json!(x);
                }
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
        DeviceCmd::Remove { name, force } => {
            let v = c.call("POST", &format!("/devices/{name}/remove"), Some(json!({"force": force})))?;
            out(&removed_line(&name, &v));
        }
        DeviceCmd::Focus { name } => {
            c.call_progress("POST", &format!("/devices/{name}/focus"), Some(json!({})))?;
            out(&format!("Brought up {name}."));
        }
    }
    Ok(0)
}

/// The project whose folder holds `cwd` (the deepest one), else the folder's name, as the board decides.
fn project_at(projects: &[Value], cwd: &str) -> Option<String> {
    let cwd = cwd.trim_end_matches('/');
    projects
        .iter()
        .filter_map(|p| Some((p["name"].as_str().filter(|n| !n.is_empty())?, p["path"].as_str()?.trim_end_matches('/'))))
        .filter(|(_, pp)| !pp.is_empty() && (cwd == *pp || cwd.starts_with(&format!("{pp}/"))))
        .max_by_key(|(_, pp)| pp.len())
        .map(|(n, _)| n.to_string())
        .or_else(|| cwd.rsplit('/').next().filter(|n| !n.is_empty()).map(|n| n.to_string()))
}

/// The project and goal filters of `tb tasks` and `tb backlog list`: this terminal's project unless
/// a project (or all) is given, or a goal is, which already says which project.
fn list_query(c: &Ctx, project: Option<String>, goal: Option<String>) -> Result<Vec<String>, String> {
    let goal = goal.map(|g| if g == "none" { Ok(g) } else { goal_ref(&g) }).transpose()?;
    let project = match (project, &goal) {
        (Some(p), _) => p,
        (None, Some(g)) if g != "none" => "all".into(),
        _ => {
            let v = c.call("GET", "/projects", None)?;
            project_at(v["projects"].as_array().map(|a| a.as_slice()).unwrap_or(&[]), &c.cwd).ok_or("this folder isn't in a project; pass --project NAME or --project all")?
        }
    };
    let mut q = vec![format!("project={}", qv(&project))];
    if let Some(g) = goal {
        q.push(format!("goal={}", qv(&g)));
    }
    Ok(q)
}

fn goal_word(v: &Value) -> &str {
    v["goal"]["ref"].as_str().unwrap_or("-")
}

/// "T412  queued   G41   Fix the login", for `tb tasks`.
fn task_list_line(t: &Value) -> String {
    let status = if t["failed"] == true { "failed" } else { t["status"].as_str().unwrap_or("") };
    let pr = t["pr"]["num"].as_i64().map(|n| format!("  PR #{n}")).unwrap_or_default();
    format!("{:<5}  {:<7}  {:<4}  {}{pr}", t["ref"].as_str().unwrap_or(""), status, goal_word(t), t["title"].as_str().unwrap_or(""))
}

/// "B12  open  bug     G4    Crash on save", for `tb backlog list`.
fn issue_list_line(b: &Value) -> String {
    let task = b["task_ref"].as_str().map(|t| format!("  → {t}")).unwrap_or_default();
    format!(
        "{:<4}  {:<6}  {:<6}  {:<4}  {}{task}",
        b["ref"].as_str().unwrap_or(""),
        b["state"].as_str().unwrap_or(""),
        b["kind"].as_str().unwrap_or(""),
        goal_word(b),
        b["title"].as_str().unwrap_or("")
    )
}

/// The head line of a show: "T12 · working · taskboard · G4 Find tasks".
fn show_head(v: &Value, state: &str) -> String {
    let mut head = vec![v["ref"].as_str().unwrap_or("").to_string(), state.to_string()];
    head.extend(v["project"].as_str().map(|p| p.to_string()));
    if let Some(g) = v["goal"]["ref"].as_str() {
        head.push(format!("{g} {}", v["goal"]["name"].as_str().unwrap_or("")).trim_end().to_string());
    }
    head.join(" · ")
}

fn indented(label: &str, text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return vec![];
    }
    std::iter::once(format!("{label}:")).chain(text.lines().map(|l| format!("  {l}"))).collect()
}

/// The last `n` entries of a log, oldest first: "  Oct 15, 3:04 PM · T12 · note: Built it".
fn log_lines(label: &str, entries: &[Value], n: usize) -> Vec<String> {
    let shown: Vec<&Value> = entries.iter().take(n).collect();
    if shown.is_empty() {
        return vec![];
    }
    std::iter::once(format!("{label}:"))
        .chain(shown.into_iter().rev().map(|e| {
            let who = e["who"].as_str().filter(|w| !w.is_empty()).map(|w| format!(" · {w}")).unwrap_or_default();
            format!("  {}{who} · {}: {}", local_when(e["at"].as_str().unwrap_or("")), e["kind"].as_str().unwrap_or(""), e["text"].as_str().unwrap_or("").trim())
        }))
        .collect()
}

/// `tb task show`: GET /tasks/<id> with its latest `n` log entries (the board sends them newest first).
fn task_show_lines(v: &Value, n: usize) -> Vec<String> {
    let status = if v["failed"] == true { "failed" } else { v["status"].as_str().unwrap_or("") };
    let mut lines = vec![show_head(v, status), v["title"].as_str().unwrap_or("").to_string()];
    if let Some(num) = v["pr"]["num"].as_i64() {
        lines.push(format!("PR: #{num} {} {}", v["pr"]["state"].as_str().unwrap_or(""), v["pr"]["url"].as_str().unwrap_or("")).trim_end().to_string());
    }
    if let Some(w) = v["who"].as_str().filter(|w| !w.is_empty()) {
        lines.push(format!("Terminal: {w}"));
    }
    if let Some(m) = model_line(v) {
        lines.push(m);
    }
    if let Some(l) = task_lane_line(v) {
        lines.push(l);
    }
    for (label, key) in [("Detail", "detail"), ("Summary", "summary")] {
        let block = indented(label, v[key].as_str().unwrap_or(""));
        if !block.is_empty() {
            lines.push(String::new());
            lines.extend(block);
        }
    }
    let log = log_lines("Log", v["log"].as_array().map(|a| a.as_slice()).unwrap_or(&[]), n);
    if !log.is_empty() {
        lines.push(String::new());
        lines.extend(log);
    }
    lines
}

/// "Lane: slot (its goal's)": where the task works and why; nothing when it's in the shared checkout by default.
fn task_lane_line(v: &Value) -> Option<String> {
    let lane = v["lane"]["lane"].as_str()?;
    let from = match v["lane"]["from"].as_str() {
        Some("goal") => " (its goal's)",
        Some("auto") if lane == "shared" => return None,
        Some("auto") => " (the board's pick)",
        _ => "",
    };
    Some(format!("Lane: {lane}{from}"))
}

/// "Model: small (sonnet · medium)": the task's model and the model and effort its terminal runs;
/// nothing when neither is known.
fn model_line(v: &Value) -> Option<String> {
    let set = v["model"].as_str().filter(|m| !m.is_empty());
    let runs = v["model_runs"].as_str().filter(|m| !m.is_empty());
    let effort = v["effort_runs"].as_str().filter(|e| !e.is_empty());
    let name = match (set, runs) {
        (Some(s), _) => s,
        (None, Some(_)) => "default",
        (None, None) => return None,
    };
    let runs: Vec<&str> = runs.filter(|r| Some(*r) != set).into_iter().chain(effort).collect();
    Some(match runs.is_empty() {
        true => format!("Model: {name}"),
        false => format!("Model: {name} ({})", runs.join(" · ")),
    })
}

/// `tb backlog show`: GET /backlog/<id> with its history (the board sends it oldest first).
fn issue_show_lines(v: &Value) -> Vec<String> {
    let state = format!("{} · {}", v["state"].as_str().unwrap_or(""), v["kind"].as_str().unwrap_or(""));
    let mut lines = vec![show_head(v, &state), v["title"].as_str().unwrap_or("").to_string()];
    if let Some(t) = v["task_ref"].as_str() {
        lines.push(format!("Task: {t}"));
    }
    if let Some(k) = v["jira_key"].as_str().filter(|k| !k.is_empty()) {
        lines.push(format!("Jira: {k}"));
    }
    if let Some(f) = v["from"].as_str().filter(|f| !f.is_empty()) {
        lines.push(f.to_string());
    }
    for (label, key) in [("Detail", "detail"), ("Said", "said"), ("How", "how")] {
        let block = indented(label, v[key].as_str().unwrap_or(""));
        if !block.is_empty() {
            lines.push(String::new());
            lines.extend(block);
        }
    }
    let mut history: Vec<Value> = v["history"].as_array().cloned().unwrap_or_default();
    history.reverse();
    let history = log_lines("History", &history, 5);
    if !history.is_empty() {
        lines.push(String::new());
        lines.extend(history);
    }
    lines
}

fn lesson_ref(v: &str) -> Result<String, String> {
    let s = v.trim();
    let digits = s.trim_start_matches(['L', 'l']);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("expected L<number>, got {v:?}"));
    }
    Ok(format!("L{digits}"))
}

/// "L4 · kept · code · taskboard · confirmed 2× · T12, G3", then the text, for `tb lessons`.
fn lesson_line(l: &Value) -> String {
    let mut head = vec![
        l["ref"].as_str().unwrap_or("").to_string(),
        l["state"].as_str().unwrap_or("").to_string(),
        l["kind"].as_str().unwrap_or("").to_string(),
        l["project"].as_str().map(|p| p.to_string()).unwrap_or_else(|| "global".into()),
    ];
    if let Some(n) = l["confirmations"].as_i64().filter(|n| *n > 0) {
        head.push(format!("confirmed {n}×"));
    }
    let ev: Vec<&str> = l["evidence"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect();
    if !ev.is_empty() {
        head.push(ev.join(", "));
    }
    let seen: Vec<&str> = l["suggest_global"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect();
    if !seen.is_empty() {
        head.push(format!("make global? (confirmed in {})", seen.join(", ")));
    }
    format!("{}\n  {}", head.join(" · "), l["text"].as_str().unwrap_or(""))
}

fn lessons_cmd(c: &Ctx, project: Option<String>, all: bool, state: Option<String>, kind: Option<String>) -> Result<i32, String> {
    let mut q = vec![match (all, &project) {
        (true, _) => "project=all".to_string(),
        (_, Some(p)) => format!("project={}", qv(p)),
        _ => format!("cwd={}", qv(&c.cwd)),
    }];
    for (k, v) in [("state", state), ("kind", kind)] {
        if let Some(v) = v {
            q.push(format!("{k}={}", qv(&v)));
        }
    }
    let v = c.call("GET", &format!("/lessons?{}", q.join("&")), None)?;
    let ls = v["lessons"].as_array().cloned().unwrap_or_default();
    if ls.is_empty() {
        out("No lessons yet. Suggest one with tb lesson add \"<what to do differently>\" --kind planning|code|process|tooling.");
    }
    for l in ls {
        out(&lesson_line(&l));
    }
    Ok(0)
}

fn lesson_search(c: &Ctx, query: &str, project: Option<String>, all: bool, state: Option<String>) -> Result<i32, String> {
    let mut q = vec![
        format!("q={}", qv(query)),
        match (all, &project) {
            (true, _) => "project=all".to_string(),
            (_, Some(p)) => format!("project={}", qv(p)),
            _ => format!("cwd={}", qv(&c.cwd)),
        },
    ];
    if let Some(s) = state {
        q.push(format!("state={}", qv(&s)));
    }
    let v = c.call("GET", &format!("/lessons/search?{}", q.join("&")), None)?;
    let ls = v["lessons"].as_array().cloned().unwrap_or_default();
    if ls.is_empty() {
        out("No lesson fits those words.");
    }
    for l in ls {
        let why: Vec<&str> = l["why"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect();
        out(&format!("{}\n  fits by {} ({})", lesson_line(&l), why.join("; "), l["score"]));
    }
    Ok(0)
}

fn idea_ref(v: &str) -> Result<String, String> {
    let s = v.trim();
    let digits = s.trim_start_matches(['I', 'i']);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("expected I<number>, got {v:?}"));
    }
    Ok(format!("I{digits}"))
}

/// "I4 · reported · taskboard · 2 notes · 1 round · became G9", then the title, for `tb idea list`.
fn idea_line(i: &Value) -> String {
    let s = |k: &str| i[k].as_str().unwrap_or("").to_string();
    let mut head = vec![s("ref"), s("state"), s("project")];
    for (k, word) in [("notes", "note"), ("rounds", "round")] {
        let n = i[k].as_array().map(|a| a.len() as i64).or_else(|| i[k].as_i64()).unwrap_or(0);
        if n > 0 {
            head.push(format!("{n} {word}{}", if n == 1 { "" } else { "s" }));
        }
    }
    if let Some(g) = i["goal"].as_str() {
        head.push(format!("became {g}"));
    }
    if i["planning"] == true {
        head.push("being planned".into());
    }
    format!("{}\n  {}", head.join(" · "), s("title"))
}

/// The idea's line, then its detail, notes and report rounds, for `tb idea show` and after a change.
fn idea_show(i: &Value) {
    out(&idea_line(i));
    let detail = i["detail"].as_str().unwrap_or("").trim();
    if !detail.is_empty() {
        out("");
        out(detail);
    }
    let notes = i["notes"].as_array().cloned().unwrap_or_default();
    if !notes.is_empty() {
        out("");
        out("Notes:");
        for n in &notes {
            let s = |k: &str| n[k].as_str().unwrap_or("");
            out(&format!("  {} ({}): {}", local_when(s("at")), s("source"), s("text")));
        }
    }
    let rounds = i["rounds"].as_array().cloned().unwrap_or_default();
    if !rounds.is_empty() {
        out("");
        out("Report rounds:");
        let latest = rounds.iter().position(|r| r["report"].is_object());
        for (n, r) in rounds.iter().enumerate() {
            let focus = r["focus"].as_str().filter(|f| !f.is_empty()).unwrap_or("the whole idea");
            let sha: String = r["sha"].as_str().unwrap_or("").chars().take(8).collect();
            let at = |k: &str| r[k].as_str().map(local_when);
            let state = match (r["error"].as_str(), at("finished_at"), at("started_at")) {
                (Some(e), _, _) => format!("failed: {e}"),
                (None, Some(f), _) => format!("reported {f}"),
                (None, None, Some(s)) => format!("started {s}"),
                _ => "not started".into(),
            };
            let sha = if sha.is_empty() { String::new() } else { format!(" at {sha}") };
            let web = if r["web"].as_bool().unwrap_or(false) { ", with the web" } else { "" };
            let verdict = r["report"]["verdict"].as_str().map(|v| format!(" · {}", v.replace('_', " "))).unwrap_or_default();
            let stale = match (r["stale"].as_bool().unwrap_or(false), r["behind"].as_i64()) {
                (false, _) => String::new(),
                (true, Some(b)) => format!(" · stale: {b} commits since"),
                (true, None) => " · stale: its commit is gone".into(),
            };
            out(&format!("  {focus} · {state}{sha}{web}{verdict}{stale}"));
            if Some(n) == latest {
                round_report(&r["report"]);
            }
        }
    }
}

/// A round's report in full, under its line in `tb idea show`.
fn round_report(rep: &Value) {
    let why = rep["why"].as_str().unwrap_or("");
    if !why.is_empty() {
        out(&format!("    {why}"));
    }
    for (head, list) in taskboardd::dig::report_sections(rep) {
        if head == "Related" {
            out(&format!("    Related: {}", list.join(", ")));
            continue;
        }
        out(&format!("    {head}:"));
        for x in list {
            out(&format!("      - {x}"));
        }
    }
}

fn idea_cmd(c: &Ctx, action: IdeaCmd) -> Result<i32, String> {
    let post = |path: String, b: Value| c.call("POST", &format!("/ideas/{path}"), Some(b));
    let (v, did) = match action {
        IdeaCmd::List { project, all, state } => return ideas_cmd(c, project, all, state),
        IdeaCmd::Add { title, detail, project } => {
            let b = json!({"title": title, "detail": detail, "project": project, "cwd": c.cwd});
            (c.call("POST", "/ideas", Some(b))?, "Kept")
        }
        IdeaCmd::Show { idea } => (c.call("GET", &format!("/ideas/{}", idea_ref(&idea)?), None)?, ""),
        IdeaCmd::Set { idea, title, detail, project } => {
            let mut b = json!({});
            for (k, x) in [("title", title), ("detail", detail), ("project", project)] {
                if let Some(x) = x {
                    b[k] = json!(x);
                }
            }
            if b.as_object().is_some_and(|o| o.is_empty()) {
                return Err("say what to change, for example: tb idea set I4 --title \"…\" or --detail \"…\"".into());
            }
            (post(idea_ref(&idea)?, b)?, "Changed")
        }
        IdeaCmd::Note { idea, text } => (post(format!("{}/note", idea_ref(&idea)?), json!({"text": text}))?, "Noted on"),
        IdeaCmd::Drop { idea, reason } => (post(format!("{}/drop", idea_ref(&idea)?), json!({"reason": reason}))?, "Dropped"),
        IdeaCmd::Shelve { idea, reason } => (post(format!("{}/shelve", idea_ref(&idea)?), json!({"reason": reason}))?, "Shelved"),
        IdeaCmd::Reopen { idea } => (post(format!("{}/reopen", idea_ref(&idea)?), json!({}))?, "Reopened"),
        IdeaCmd::Dig { idea, focus, web } => {
            (post(format!("{}/dig", idea_ref(&idea)?), json!({"focus": focus, "web": web}))?, "Digging into")
        }
        IdeaCmd::Plan { idea } => (post(format!("{}/plan", idea_ref(&idea)?), json!({}))?, "Planning"),
    };
    if !did.is_empty() {
        out(&format!("{did} {}.", v["ref"].as_str().unwrap_or("")));
    }
    if did == "Planning" {
        out(&match v["goal"].as_str() {
            Some(g) => format!("It became {g}, planned and not started: tb goal show {g}"),
            None => format!("Claude drafts its goal and tasks on its own; the goal stays planned, not started. See it with tb idea show {}.", v["ref"].as_str().unwrap_or("")),
        });
    }
    idea_show(&v);
    Ok(0)
}

fn ideas_cmd(c: &Ctx, project: Option<String>, all: bool, state: Option<String>) -> Result<i32, String> {
    let mut q = vec![match (all, &project) {
        (true, _) => "project=all".to_string(),
        (_, Some(p)) => format!("project={}", qv(p)),
        _ => format!("cwd={}", qv(&c.cwd)),
    }];
    if let Some(s) = state {
        q.push(format!("state={}", qv(&s)));
    }
    let v = c.call("GET", &format!("/ideas?{}", q.join("&")), None)?;
    let is = v["ideas"].as_array().cloned().unwrap_or_default();
    if is.is_empty() {
        out("No ideas here. Keep one with tb idea add \"<title>\" --detail \"…\".");
    }
    for i in is {
        out(&idea_line(&i));
    }
    Ok(0)
}

fn search_cmd(c: &Ctx, query: &str, project: Option<String>, all: bool, kind: Option<String>, open: bool) -> Result<i32, String> {
    let mut q = vec![
        format!("q={}", qv(query)),
        match (all, &project) {
            (true, _) => "project=all".to_string(),
            (_, Some(p)) => format!("project={}", qv(p)),
            _ => format!("cwd={}", qv(&c.cwd)),
        },
    ];
    if let Some(k) = kind {
        q.push(format!("kind={}", qv(&k)));
    }
    if open {
        q.push("open=1".into());
    }
    let v = c.call("GET", &format!("/search?{}", q.join("&")), None)?;
    let hits = v["hits"].as_array().cloned().unwrap_or_default();
    if hits.is_empty() {
        out("Nothing matches those words.");
    }
    for h in &hits {
        out(&search_line(h));
    }
    Ok(0)
}

/// One search hit: its ref, state, goal and title (a goal note shows as its goal's ref, then "note").
fn search_line(h: &Value) -> String {
    let s = |k: &str| h[k].as_str().unwrap_or("");
    let r = if s("kind") == "note" { format!("{} note", s("ref")) } else { s("ref").to_string() };
    let goal = h["goal"]["ref"].as_str().filter(|_| s("kind") != "note").unwrap_or("-");
    format!("{r:<8} {:<9} {goal:<5} {}", s("state"), s("title"))
}

fn areas_cmd(c: &Ctx, action: AreasCmd) -> Result<i32, String> {
    let AreasCmd::Backfill { project } = action;
    let v = c.call_progress("POST", "/areas/backfill", Some(json!({"project": project})))?;
    let projects: Vec<&str> = v["projects"].as_array().into_iter().flatten().filter_map(|p| p.as_str()).collect();
    if projects.is_empty() {
        out("No project has areas yet: list them in .taskboard.toml at a repo's root.");
        return Ok(0);
    }
    let area = |a: &Value| a.as_str().unwrap_or("none").to_string();
    let tasks = v["tasks"].as_array().cloned().unwrap_or_default();
    let goals = v["goals"].as_array().cloned().unwrap_or_default();
    out(&format!("{}: set {} task area(s) and {} goal area(s).", projects.join(", "), tasks.len(), goals.len()));
    for t in &tasks {
        out(&format!("  {} {} → {} (from {})", t["ref"].as_str().unwrap_or(""), t["title"].as_str().unwrap_or(""), area(&t["area"]), t["from"].as_str().unwrap_or("")));
    }
    for g in &goals {
        out(&format!("  {} {} → {}", g["ref"].as_str().unwrap_or(""), g["name"].as_str().unwrap_or(""), area(&g["area"])));
    }
    if let Some(n) = v["unsure"].as_u64().filter(|n| *n > 0) {
        out(&format!("{n} task(s) left blank: their files tie between areas or sit in none."));
    }
    Ok(0)
}

fn release_ref(v: &str) -> Result<String, String> {
    let s = v.trim();
    if s.eq_ignore_ascii_case("unreleased") {
        return Ok("unreleased".into());
    }
    let digits = s.trim_start_matches(['R', 'r']);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("expected R<number> or unreleased, got {v:?}"));
    }
    Ok(format!("R{digits}"))
}

/// Which project a release read is for: the one named, else this terminal's task's, else the folder's.
fn release_project_query(c: &Ctx, project: Option<String>) -> String {
    let mut q = vec![match project {
        Some(p) => format!("project={}", qv(&p)),
        None => format!("cwd={}", qv(&c.cwd)),
    }];
    if !c.session.is_empty() {
        q.push(format!("by_session={}", qv(&c.session)));
    }
    q.join("&")
}

/// "R3 · v1.4 · 2026-10-15 · 5 tasks", for `tb releases` and `tb release show`; "· failed" after a
/// failed one's date.
fn release_line(r: &Value) -> String {
    let mut head = vec![r["ref"].as_str().unwrap_or("").to_string(), r["name"].as_str().unwrap_or("").to_string()];
    if let Some(d) = r["cut_at"].as_str().and_then(|s| s.get(..10)) {
        head.push(d.to_string());
    }
    if r["failed_at"].is_string() {
        head.push("failed".into());
    }
    let n = r["tasks"].as_i64().unwrap_or(0);
    head.push(format!("{n} task{}", if n == 1 { "" } else { "s" }));
    head.join(" · ")
}

fn releases_cmd(c: &Ctx, project: Option<String>) -> Result<i32, String> {
    let v = c.call("GET", &format!("/releases?{}", release_project_query(c, project)), None)?;
    let name = v["project"].as_str().unwrap_or("");
    if v["on"] != true {
        return Err(format!("releases are off for {name}. Turn them on with: tb project set {name} --releases on"));
    }
    let n = v["unreleased"].as_i64().unwrap_or(0);
    out(&format!("{name}: {n} done task{} in Unreleased", if n == 1 { "" } else { "s" }));
    if v["pending"].is_object() {
        out(&pending_line(&v["pending"]));
    }
    for r in v["releases"].as_array().into_iter().flatten() {
        out(&release_line(r));
    }
    Ok(0)
}

/// A pending cut and where each task or goal it waits for stands: "v1.5 · pending · after T627 (met), G12".
fn pending_line(p: &Value) -> String {
    let after: Vec<String> = p["after"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| {
            let r = a["ref"].as_str().unwrap_or("");
            match a["state"].as_str() {
                Some("met") => format!("{r} (done)"),
                Some("dead") => format!("{r} ({})", a["why"].as_str().unwrap_or("can't finish")),
                _ => r.to_string(),
            }
        })
        .collect();
    let state = if p["running"] == true { "cutting now" } else { "pending" };
    format!("{} · {state} · after {}", p["name"].as_str().unwrap_or(""), after.join(", "))
}

/// A release's (or Unreleased's) tasks under their goals, for `tb release show` and `tb release cut`.
fn release_groups(v: &Value) {
    for g in v["groups"].as_array().into_iter().flatten() {
        out(&match g["goal"].as_str() {
            Some(r) => format!("{r} {}", g["name"].as_str().unwrap_or("")),
            None => "No goal".to_string(),
        });
        for t in g["tasks"].as_array().into_iter().flatten() {
            let pr = t["pr_num"].as_i64().map(|n| format!(" · #{n}")).unwrap_or_default();
            out(&format!("  {} {}{pr}", t["ref"].as_str().unwrap_or(""), t["title"].as_str().unwrap_or("")));
        }
    }
}

enum ReleaseCut {
    Cut(Value),
    /// The release script's exit status: nothing was recorded.
    ScriptFailed(i32),
}

/// `tb release cut`: unless `no_script`, asks the board whether the cut would go through (Unreleased
/// isn't empty, the name is free) and for the project's release script, runs the script with the
/// release name, and records the release only when it exits 0. `call` posts to /releases; `run` runs
/// a script (command, release name, folder, whether it replaces a failed release) and gives its exit status.
fn cut_release(
    call: &mut dyn FnMut(Value) -> Result<Value, String>,
    run: &mut dyn FnMut(&str, &str, Option<&str>, bool) -> Result<i32, String>,
    mut body: Value,
    no_script: bool,
) -> Result<ReleaseCut, String> {
    if !no_script {
        let mut check = body.clone();
        check["check"] = json!(true);
        let v = call(check)?;
        if let Some(script) = v["script"].as_str() {
            let started = std::time::Instant::now();
            let code = run(script, v["name"].as_str().unwrap_or(""), v["dir"].as_str(), v["replaces"] == true)?;
            if code != 0 {
                return Ok(ReleaseCut::ScriptFailed(code));
            }
            // The board looks for the run the script's tag started from then on.
            body["script_secs"] = json!(started.elapsed().as_secs_f64());
        }
    }
    call(body).map(ReleaseCut::Cut)
}

/// The CI run the release started, waiting up to a minute for the board to find it; none when the
/// board isn't looking for one (the project isn't on GitHub) or doesn't find it in time.
fn release_run(c: &Ctx, v: &Value) -> Option<String> {
    let r = v["ref"].as_str()?;
    let mut v = v.clone();
    for _ in 0..30 {
        if let Some(url) = v["run_url"].as_str() {
            return Some(url.to_string());
        }
        if v["run_looking"] != true {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
        v = c.call("GET", &format!("/releases/{r}"), None).ok()?;
    }
    None
}

/// Releases come from main or a release/* branch: a cut from a feature branch would tag work that hasn't merged.
fn need_release_branch(branch: &str) -> Result<(), String> {
    if branch == "main" || branch.starts_with("release/") {
        return Ok(());
    }
    let on = if branch.is_empty() { "a detached HEAD".to_string() } else { branch.to_string() };
    Err(format!("release cut only runs on main or a release/* branch, and this checkout is on {on}. Switch to one and run it again."))
}

/// The checkout a release script runs in: the folder's git top level, else the folder.
fn release_checkout(dir: Option<&str>) -> String {
    let dir = dir.map(str::to_string).unwrap_or_else(|| ".".into());
    std::process::Command::new("git")
        .args(["-C", &dir, "rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or(dir)
}

/// The checkout's branch, empty on a detached HEAD or outside git.
fn checkout_branch(top: &str) -> String {
    std::process::Command::new("git")
        .args(["-C", top, "branch", "--show-current"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Runs a project's release script in its checkout with the release name as its one argument and in
/// TB_RELEASE (so the script can tell tb started it), its output going straight to this terminal. When
/// the name's last release failed, TB_RELEASE_REPLACE=1 tells the script it may replace that one's tag.
fn run_release_script(script: &str, name: &str, dir: Option<&str>, replaces: bool) -> Result<i32, String> {
    let top = release_checkout(dir);
    out(&format!("Running the release script in {top}: {script} {name}"));
    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c").arg(format!("{script} \"$1\"")).arg("sh").arg(name).current_dir(&top).env("TB_RELEASE", name).env_remove("TB_RELEASE_REPLACE");
    if replaces {
        cmd.env("TB_RELEASE_REPLACE", "1");
    }
    let status = cmd
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("couldn't start the release script: {e}"))?;
    // Killed by a signal: no code, so say 1.
    Ok(status.code().unwrap_or(1))
}

/// `tb release cut`'s runner: the release script, once the checkout is on main or a release/* branch.
fn run_release_script_on_a_release_branch(script: &str, name: &str, dir: Option<&str>, replaces: bool) -> Result<i32, String> {
    need_release_branch(&checkout_branch(&release_checkout(dir)))?;
    run_release_script(script, name, dir, replaces)
}

fn release_cmd(c: &Ctx, action: ReleaseCmd) -> Result<i32, String> {
    match action {
        ReleaseCmd::Cut { name, project, notes, no_script, after } if !after.is_empty() => {
            let body = json!({"name": name, "project": project, "notes": notes, "cwd": c.cwd, "after": after, "no_script": no_script});
            let v = c.call("POST", "/releases", Some(body))?;
            out(&pending_line(&v));
            out("The board cuts it once those are through and tells you if it drops it. Drop it with: tb release cancel");
        }
        ReleaseCmd::Cancel { project } => {
            let v = c.call("POST", "/releases/pending/cancel", Some(json!({"project": project, "cwd": c.cwd})))?;
            out(&format!("Cancelled the pending cut {}.", v["name"].as_str().unwrap_or("")));
        }
        ReleaseCmd::Cut { name, project, notes, no_script, .. } => {
            let body = json!({"name": name, "project": project, "notes": notes, "cwd": c.cwd});
            match cut_release(&mut |b| c.call("POST", "/releases", Some(b)), &mut run_release_script_on_a_release_branch, body, no_script)? {
                ReleaseCut::Cut(v) => {
                    out(&format!("Cut {}.", release_line(&v)));
                    release_groups(&v);
                    if let Some(url) = release_run(c, &v) {
                        out(&format!("Release run: {url}"));
                    }
                }
                ReleaseCut::ScriptFailed(code) => {
                    eprintln!("tb: the release script exited {code}, so {name} isn't recorded. Fix it and run tb release cut again.");
                    return Ok(code);
                }
            }
        }
        ReleaseCmd::Show { release, project, markdown } => {
            let r = release_ref(&release)?;
            let v = c.call("GET", &format!("/releases/{r}?{}", release_project_query(c, project)), None)?;
            if markdown {
                print!("{}", v["markdown"].as_str().unwrap_or(""));
                return Ok(0);
            }
            let n = v["tasks"].as_i64().unwrap_or(0);
            out(&if r == "unreleased" {
                format!("Unreleased in {} · {n} task{}", v["project"].as_str().unwrap_or(""), if n == 1 { "" } else { "s" })
            } else {
                release_line(&v)
            });
            if let Some(n) = v["notes"].as_str().filter(|n| !n.is_empty()) {
                out(&format!("  {n}"));
            }
            if let Some(u) = v["run_url"].as_str() {
                out(&format!("  Run: {u}"));
            }
            release_groups(&v);
        }
        ReleaseCmd::Set { release, name, notes } => {
            let r = release_ref(&release)?;
            if r == "unreleased" {
                return Err("Unreleased isn't a release; cut one with tb release cut <name>".into());
            }
            let mut b = json!({});
            if let Some(n) = name {
                b["name"] = json!(n);
            }
            if let Some(n) = notes {
                b["notes"] = json!(n);
            }
            if b.as_object().is_some_and(|o| o.is_empty()) {
                return Err(format!("say what to change, for example: tb release set {r} --name v1.5 or --notes \"…\""));
            }
            let v = c.call("POST", &format!("/releases/{r}"), Some(b))?;
            out(&format!("Changed {}.", release_line(&v)));
        }
        ReleaseCmd::Move { task, release } => {
            let v = c.call("POST", "/releases/move", Some(json!({"task": task_ref(&task)?, "release": release_ref(&release)?})))?;
            out(&format!("{}: {}.", v["task"].as_str().unwrap_or(""), v["text"].as_str().unwrap_or("")));
        }
        ReleaseCmd::Fail { release, reason } => {
            let r = release_ref(&release)?;
            if r == "unreleased" {
                return Err("Unreleased isn't a release; mark one failed with tb release fail R<n>".into());
            }
            let v = c.call("POST", &format!("/releases/{r}/fail"), Some(json!({"reason": reason})))?;
            let name = v["name"].as_str().unwrap_or("");
            out(&format!("Marked {} failed; its tasks are back in Unreleased. Cut it again with: tb release cut {name}", v["ref"].as_str().unwrap_or("")));
        }
    }
    Ok(0)
}

fn lesson_cmd(c: &Ctx, action: LessonCmd) -> Result<i32, String> {
    let (v, did) = match action {
        LessonCmd::Retro { goal } => return lesson_retro(c, &goal_ref(&goal)?),
        LessonCmd::Add { text, kind, global, project, evidence, kept } => {
            let b = json!({"text": text, "kind": kind, "scope": if global { "global" } else { "project" }, "project": project,
                           "evidence": evidence, "kept": kept, "cwd": c.cwd});
            (c.call("POST", "/lessons", Some(b))?, if kept { "Kept" } else { "Suggested" })
        }
        LessonCmd::Edit { lesson, text, kind, global, project, evidence } => {
            let mut b = json!({"cwd": c.cwd});
            if let Some(x) = text {
                b["text"] = json!(x);
            }
            if let Some(x) = kind {
                b["kind"] = json!(x);
            }
            if global {
                b["scope"] = json!("global");
            } else if let Some(p) = project {
                b["scope"] = json!("project");
                b["project"] = json!(p);
            }
            if !evidence.is_empty() {
                b["evidence"] = json!(evidence);
            }
            if b.as_object().is_some_and(|o| o.len() == 1) {
                return Err("say what to change, for example: tb lesson edit L4 --text \"…\" or --evidence T12".into());
            }
            (c.call("POST", &format!("/lessons/{}", lesson_ref(&lesson)?), Some(b))?, "Changed")
        }
        LessonCmd::Show { lesson } => (c.call("GET", &format!("/lessons/{}", lesson_ref(&lesson)?), None)?, ""),
        LessonCmd::Keep { lesson } => (c.call("POST", &format!("/lessons/{}/keep", lesson_ref(&lesson)?), Some(json!({})))?, "Kept"),
        LessonCmd::Retire { lesson } => (c.call("POST", &format!("/lessons/{}/retire", lesson_ref(&lesson)?), Some(json!({})))?, "Retired"),
        LessonCmd::Suggest { lesson } => {
            (c.call("POST", &format!("/lessons/{}/suggest", lesson_ref(&lesson)?), Some(json!({})))?, "Back to suggested:")
        }
        LessonCmd::Confirm { lesson, evidence } => {
            (c.call("POST", &format!("/lessons/{}/confirm", lesson_ref(&lesson)?), Some(json!({"evidence": evidence})))?, "Confirmed")
        }
        LessonCmd::Search { query, project, all, state } => return lesson_search(c, &query, project, all, state),
    };
    if !did.is_empty() {
        out(&format!("{did} {}.", v["ref"].as_str().unwrap_or("")));
    }
    out(&lesson_line(&v));
    Ok(0)
}

/// Runs a goal's retrospective and waits for it (a minute or two), then lists what it suggested.
fn lesson_retro(c: &Ctx, g: &str) -> Result<i32, String> {
    let path = format!("/lessons/retro/{g}");
    c.call("POST", &path, Some(json!({})))?;
    out(&format!("Looking back over {g}…"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    let v = loop {
        let v = c.call("GET", &path, None)?;
        if v["state"] != "running" || std::time::Instant::now() > deadline {
            break v;
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
    };
    match v["state"].as_str().unwrap_or("") {
        "running" => {
            out("Still running; see it later with tb lessons --state suggested.");
            return Ok(1);
        }
        "failed" => return Err(format!("the retrospective failed: {}", v["error"].as_str().unwrap_or("no reason given"))),
        _ => {}
    }
    let refs = |k: &str| v[k].as_array().cloned().unwrap_or_default();
    let (new, confirmed, kept, retired) = (refs("new"), refs("confirmed"), refs("kept"), refs("retired"));
    if [&new, &confirmed, &kept, &retired].iter().all(|l| l.is_empty()) {
        out(&format!("Nothing worth keeping from {g}."));
        return Ok(0);
    }
    let lists = [("Suggested", &new), ("Confirmed", &confirmed), ("Kept, confirmed again", &kept), ("Retired, not confirmed lately", &retired)];
    for (label, list) in lists {
        for r in list.iter().filter_map(|r| r.as_str()) {
            out(&format!("{label}: {}", lesson_line(&c.call("GET", &format!("/lessons/{r}"), None)?)));
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
                            "goals": refs(&goals, goal_ref)?, "project": project, "note": note})),
            )?;
            out(&format!("Added the bit {}.", v["name"].as_str().unwrap_or("")));
            v
        }
        BitCmd::Made { name, undo } => c.call("POST", &format!("/bits/{name}/made"), Some(json!({"undo": undo})))?,
        BitCmd::Set { name, rename, backend, local, tasks, not_tasks, goals, not_goals, note } => {
            let mut b = json!({});
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
            if b.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                return Err("say what to change, for example: tb bit set newCheckout --task T12".into());
            }
            c.call("POST", &format!("/bits/{name}"), Some(b))?
        }
        BitCmd::Remove { name } => {
            c.call("POST", &format!("/bits/{name}/remove"), Some(json!({})))?;
            out(&format!("Removed the bit {name}."));
            return Ok(0);
        }
    };
    let tool = c.call("GET", "/bits", None).ok().and_then(|l| l["tool"].as_str().map(|s| s.to_string())).unwrap_or_else(|| "the flag tool".into());
    out(&bit_line(&v, &tool));
    Ok(0)
}

/// `tb checkpoint`'s report: only the lists given, so one left out never wipes what was saved.
fn checkpoint_body(done: Vec<String>, next: Vec<String>, decisions: Vec<String>, files: Vec<String>) -> Result<Value, String> {
    let mut f = json!({});
    for (k, xs) in [("done", done), ("next", next), ("decisions", decisions), ("files", files)] {
        if !xs.is_empty() {
            f[k] = json!(xs);
        }
    }
    if f.as_object().is_some_and(|o| o.is_empty()) {
        return Err("give at least one --done, --next, --decision or --file".into());
    }
    Ok(f)
}

/// `tb propose` names at least one task, each with a title.
fn propose_tasks(tasks: &[String]) -> Result<(), String> {
    if tasks.is_empty() {
        return Err("name at least one task: --task \"title::what to do\"".into());
    }
    if let Some(t) = tasks.iter().find(|t| t.split_once("::").map_or(t.as_str(), |(title, _)| title).trim().is_empty()) {
        return Err(format!("--task {t:?} has no title: give it as \"title::what to do\""));
    }
    Ok(())
}

/// What `tb attach` sends: a link as it is, a file as its full path, once it's there.
fn attach_target(url: &str, cwd: &str) -> Result<String, String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        return Ok(url.to_string());
    }
    let path = match url.strip_prefix("~/") {
        Some(rest) => std::path::Path::new(&std::env::var("HOME").unwrap_or_default()).join(rest),
        None => std::path::Path::new(cwd).join(url),
    };
    if !path.exists() {
        return Err(format!("no such file: {}", path.display()));
    }
    Ok(std::fs::canonicalize(&path).unwrap_or(path).to_string_lossy().to_string())
}

fn close_done_line(n: i64) -> String {
    match n {
        0 => "No finished task has a terminal to close.".into(),
        1 => "Closing 1 finished task's terminal.".into(),
        n => format!("Closing {n} finished tasks' terminals."),
    }
}

/// A query value, percent-encoded.
fn qv(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn reviewer_line(r: &Value) -> String {
    let mut ids: Vec<String> = vec![];
    if let Some(u) = r["user"].as_str() {
        ids.push(u.to_string());
    }
    for k in ["emails", "aliases"] {
        ids.extend(r[k].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str().map(|s| s.to_string())));
    }
    let name = r["name"].as_str().unwrap_or("");
    let mut bits = vec![if ids.is_empty() { name.to_string() } else { format!("{name} ({})", ids.join(", ")) }];
    if r["user"].is_null() {
        bits.push("no host account yet".into());
    }
    if r["pinned"] == true {
        bits.push("pinned".into());
    }
    if let Some(a) = r["automation"].as_f64().filter(|a| (*a - 1.0).abs() > 1e-9) {
        bits.push(format!("automation {a}"));
    }
    if r["bot"].is_object() {
        bits.push(format!("bot every {}h marked “{}”", r["bot"]["every_h"], r["bot"]["mark"].as_str().unwrap_or("")));
    }
    if let Some(m) = r["median_work_mins"].as_f64() {
        bits.push(format!("reviews in about {} work minutes", m.round() as i64));
    }
    if r["open_asks"].as_i64().unwrap_or(0) > 0 {
        bits.push(format!("{} open", r["open_asks"]));
    }
    if r["removed"] == true {
        bits.push(match r["removed_why"].as_str() {
            Some(w) => format!("removed: {w}"),
            None => "removed".into(),
        });
    }
    bits.join(" · ")
}

fn reviewers_cmd(c: &Ctx, action: ReviewersCmd) -> Result<i32, String> {
    let body = |project: &Option<String>, who: &str, extra: Value| -> Value {
        let mut b = json!({"project": project, "cwd": c.cwd, "reviewer": who, "who": c.who()});
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            b[k] = v;
        }
        b
    };
    let v = match action {
        ReviewersCmd::List { project, all } => {
            let path = match (all, project) {
                (true, _) => "/reviewers?project=all".to_string(),
                (false, Some(p)) => format!("/reviewers?project={}", qv(&p)),
                (false, None) => format!("/reviewers?cwd={}", qv(&c.cwd)),
            };
            let v = c.call("GET", &path, None)?;
            let projects = v["projects"].as_array().cloned().unwrap_or_default();
            if projects.is_empty() {
                out("No reviewers yet. Add one with tb reviewers add NAME --user <their host id> --email <their commit email>.");
            }
            for p in projects {
                out(&format!("{}:", p["name"].as_str().unwrap_or("")));
                for r in p["reviewers"].as_array().cloned().unwrap_or_default() {
                    out(&format!("  {}", reviewer_line(&r)));
                }
            }
            return Ok(0);
        }
        ReviewersCmd::Add { name, project, user, emails, aliases, slack } => {
            c.call("POST", "/reviewers", Some(body(&project, &name, json!({"user": user, "emails": emails, "aliases": aliases, "slack": slack}))))?
        }
        ReviewersCmd::Remove { who, project, reason } => c.call("POST", "/reviewers/remove", Some(body(&project, &who, json!({"reason": reason}))))?,
        ReviewersCmd::Back { who, project } => c.call("POST", "/reviewers/back", Some(body(&project, &who, json!({}))))?,
        ReviewersCmd::Pin { who, project } => c.call("POST", "/reviewers/pin", Some(body(&project, &who, json!({"on": true}))))?,
        ReviewersCmd::Unpin { who, project } => c.call("POST", "/reviewers/pin", Some(body(&project, &who, json!({"on": false}))))?,
        ReviewersCmd::Bot { who, project, every, mark, off } => {
            c.call("POST", "/reviewers/bot", Some(body(&project, &who, json!({"every_h": every, "mark": mark, "off": off}))))?
        }
        ReviewersCmd::Auto { who, level, project } => c.call("POST", "/reviewers/auto", Some(body(&project, &who, json!({"level": level}))))?,
        ReviewersCmd::Alias { who, aliases, user, project } => c.call("POST", "/reviewers/alias", Some(body(&project, &who, json!({"aliases": aliases, "user": user}))))?,
        ReviewersCmd::Sync { project } => {
            let v = c.call_progress("POST", "/reviewers/sync", Some(body(&project, "", json!({}))))?;
            out(&format!("{}: {} joined from the commit history, {} matched to a host account.", v["project"].as_str().unwrap_or(""), v["joined"], v["matched"]));
            for r in v["reviewers"].as_array().cloned().unwrap_or_default() {
                out(&format!("  {}", reviewer_line(&r)));
            }
            return Ok(0);
        }
        ReviewersCmd::Merge { keep, other, project } => c.call("POST", "/reviewers/merge", Some(body(&project, &keep, json!({"other": other}))))?,
    };
    out(&format!("{}: {}", v["project"].as_str().unwrap_or(""), reviewer_line(&v)));
    Ok(0)
}

/// What `tb pr reviewers --dry-run` would do; the board changed nothing on the PR.
fn dry_run_line(v: &Value) -> String {
    let mut parts: Vec<String> = vec![];
    if let Some(name) = v["drop"]["name"].as_str() {
        parts.push(format!("take {name} off"));
    }
    if let (Some(old), Some(new)) = (v["replace"]["old"].as_str(), v["replace"]["new"].as_str()) {
        parts.push(format!("ask {new} in place of {old}"));
    }
    let picks: Vec<String> = v["picks"].as_array().cloned().unwrap_or_default().iter().map(|p| format!("{} ({})", p["name"].as_str().unwrap_or(""), p["why"].as_str().unwrap_or(""))).collect();
    if !picks.is_empty() {
        parts.push(format!("ask {}", picks.join(", ")));
    }
    if parts.is_empty() {
        return "The board would ask nobody. Nothing changed on the PR.".to_string();
    }
    format!("The board would {}. Nothing changed on the PR.", parts.join(", then "))
}

fn pr_reviewers_line(t: &str, v: &Value) -> String {
    let names = |k: &str| -> Vec<String> { v[k].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x["name"].as_str().map(|s| s.to_string())).collect() };
    let mut parts = vec![];
    let asked = names("asked");
    if !asked.is_empty() {
        parts.push(format!("asked {}", asked.join(", ")));
    }
    if let Some(r) = v["replaced"].as_object() {
        parts.push(format!("asked {} in place of {}", r["new"].as_str().unwrap_or(""), r["old"].as_str().unwrap_or("")));
    }
    let dropped = names("dropped");
    if !dropped.is_empty() {
        parts.push(format!("took {} off", dropped.join(", ")));
    }
    if parts.is_empty() {
        return format!("{t}'s PR reviewers are unchanged.");
    }
    let s = parts.join("; ");
    let s = format!("{}{}", s[..1].to_uppercase(), &s[1..]);
    if let Some(short) = v["short"].as_str() {
        return format!("{s} on {t}'s PR. {short}");
    }
    format!("{s} on {t}'s PR. The board watches it and brings this conversation back when it needs you. You can stop here.")
}

/// " · area Quill" for a row with an area, or nothing.
fn area_bit(v: &Value) -> String {
    v["area"].as_str().filter(|a| !a.is_empty()).map(|a| format!(" · area {a}")).unwrap_or_default()
}

/// "14/15 done · 1 in review": a task whose PR is still open isn't done until it merges.
fn done_bit(g: &Value) -> String {
    let mut line = format!("{}/{} done", g["done"], g["total"]);
    if let Some(n) = g["failed_accepted"].as_i64().filter(|n| *n > 0) {
        line = format!("{line} · {n} failed (accepted)");
    }
    match g["review"].as_i64().unwrap_or(0) {
        0 => line,
        n => format!("{line} · {n} in review"),
    }
}

/// A goal's projects: its own, then the others its tasks are in ("taskboard + midna").
fn goal_projects(g: &Value) -> String {
    match g["projects"].as_array().filter(|ps| ps.len() > 1) {
        Some(ps) => ps.iter().filter_map(|p| p.as_str()).collect::<Vec<_>>().join(" + "),
        None => g["project"].as_str().unwrap_or("").to_string(),
    }
}

fn goal_lines(g: &Value) -> Vec<String> {
    let mut lines = vec![format!(
        "{} · {} · {}{} · {}",
        g["ref"].as_str().unwrap_or(""),
        g["name"].as_str().unwrap_or(""),
        goal_projects(g),
        area_bit(g),
        done_bit(g)
    )];
    if let Some(o) = g["outcome"].as_str().filter(|o| !o.is_empty()) {
        lines.push(format!("Done when: {o}"));
    }
    if g["paused"] == true {
        lines.push("Paused: nothing new starts.".into());
    }
    if g["jira_off"] == true {
        lines.push("Jira: off. Its tasks and their PRs need no ticket; don't make one.".into());
    }
    if let Some(a) = g["accepted"].as_object() {
        let tasks: Vec<&str> = a["tasks"].as_array().into_iter().flatten().filter_map(|t| t.as_str()).collect();
        lines.push(format!("Finished by the owner without {}: {}", tasks.join(", "), a["why"].as_str().unwrap_or("")));
    }
    let after: Vec<&str> = g["after"].as_array().into_iter().flatten().filter_map(|a| a.as_str()).collect();
    if !after.is_empty() {
        let wait = if g["after_blocker"].is_string() { ": its tasks wait until that's finished" } else { "" };
        lines.push(format!("Runs after {}{wait}.", after.join(", ")));
    }
    if let Some(s) = g["setup"].as_str().filter(|s| !s.is_empty()) {
        lines.push(format!("Set up: {s}"));
    }
    lines.push(format!(
        "Runs {}, at most {} at a time.",
        if g["tasks"].as_array().is_some_and(|ts| ts.iter().any(|t| t["wave"].is_i64())) {
            "in waves"
        } else if g["run_in_order"] == true {
            "in order"
        } else {
            "in any order"
        },
        g["max_terminals"]
    ));
    if let Some(l) = lane_text(g["lane"].as_str().unwrap_or("auto")) {
        lines.push(l.into());
    }
    let own: Vec<String> = g["devices"]["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["in_pool"] == true)
        .map(|d| {
            let mut x: Vec<String> = d["purpose"].as_str().map(|p| format!("for {p}")).into_iter().collect();
            if d["reserved"] == true {
                x.push("reserved".into());
            }
            let name = d["name"].as_str().unwrap_or("");
            if x.is_empty() { name.to_string() } else { format!("{name} ({})", x.join(", ")) }
        })
        .collect();
    if !own.is_empty() {
        lines.push(format!("Its own devices: {}", own.join(", ")));
    }
    if let Some(tasks) = g["tasks"].as_array() {
        for (i, t) in tasks.iter().enumerate() {
            let status = if t["failed"] == true { "failed".to_string() } else { t["status"].as_str().unwrap_or("").to_string() };
            let mut line = format!("  {}. {} [{status}] {}", i + 1, t["ref"].as_str().unwrap_or(""), t["title"].as_str().unwrap_or(""));
            if let Some(w) = t["wave"].as_i64() {
                line += &format!(" · wave {w}");
            }
            if t["area"] != g["area"] {
                line += &area_bit(t);
            }
            if let Some(also) = t["also"].as_array().filter(|a| !a.is_empty()) {
                line += &format!(" · also for {}", also.iter().filter_map(|x| x["ref"].as_str()).collect::<Vec<_>>().join(", "));
            }
            if let Some(w) = t["waits_for"].as_array().filter(|a| !a.is_empty()) {
                line += &format!(" · waits for {}", w.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "));
            }
            if let Some(l) = t["stack_on"]["line"].as_str() {
                line += &format!(" · {l}");
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
        // Who asks, so the board takes this checkout's head only from the task's own terminal.
        Some(x) => format!("/steps?task={}{head}&from={}", task_ref(&x)?, c.session),
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

/// A ref (`HEAD`, `HEAD~1`, a branch, a short sha) as the full sha of its commit in `dir`'s repo.
pub fn resolve_commit(dir: &str, r: &str) -> Result<String, String> {
    let r = r.trim();
    if r.is_empty() {
        return Err("Give the commit as a sha or a ref.".into());
    }
    let o = std::process::Command::new("git")
        .args(["-C", if dir.is_empty() { "." } else { dir }, "rev-parse", "--verify", "--quiet", &format!("{r}^{{commit}}")])
        .output()
        .map_err(|e| format!("couldn't run git: {e}"))?;
    let sha = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if !o.status.success() || sha.is_empty() {
        return Err(format!("“{r}” isn't a commit in {}.", if dir.is_empty() { "this folder" } else { dir }));
    }
    Ok(sha)
}

/// The worktree that has `branch` checked out, from `git worktree list` in `dir`'s repo.
fn worktree_with(dir: &str, branch: &str) -> Option<String> {
    let o = std::process::Command::new("git").args(["-C", dir, "worktree", "list", "--porcelain"]).output().ok().filter(|o| o.status.success())?;
    let want = format!("branch refs/heads/{}", branch.trim().trim_start_matches("refs/heads/"));
    let mut path = None;
    for line in String::from_utf8_lossy(&o.stdout).lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(p.to_string());
        } else if line == want {
            return path;
        }
    }
    None
}

/// Where a step's round runs and what it looks at.
struct Aimed {
    /// The checkout the round looks at (and runs in, unless it's pinned to another commit).
    dir: Option<String>,
    head: Option<String>,
    branch: Option<String>,
    /// The head isn't the checkout's: the round runs on a throwaway checkout of it (`PinnedCheckout`).
    pinned: bool,
}

/// `git -C dir <args>`'s trimmed output, when it succeeds.
fn git_out(dir: &str, args: &[&str]) -> Option<String> {
    let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().ok().filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// The branch checked out in `dir` (none when its head is detached).
fn branch_in(dir: &str) -> Option<String> {
    git_out(dir, &["symbolic-ref", "--short", "-q", "HEAD"]).filter(|b| !b.is_empty())
}

/// The checkout a round starts from: this folder's, when it's a checkout of the task's repo and this is
/// the task's terminal (or the folder is the task's worktree); else the task's worktree, else the repo. So
/// `tb steps --task T<n>` from another checkout of the repo fills in the task's branch, not this one's.
fn home_checkout(c: &Ctx, v: &Value) -> String {
    let repo = v["vars"]["repo"].as_str().unwrap_or("");
    let common = |d: &str| {
        git_out(d, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).and_then(|p| std::fs::canonicalize(p).ok())
    };
    let canon = |d: &str| std::fs::canonicalize(d).ok();
    let theirs = v["vars"]["worktree"].as_str().filter(|w| !w.is_empty() && std::path::Path::new(w).is_dir()).map(|w| w.to_string());
    let mine = !c.session.is_empty() && v["session"].as_str() == Some(c.session.as_str());
    match git_out(&c.cwd, &["rev-parse", "--show-toplevel"]).filter(|t| !t.is_empty()) {
        Some(top) if theirs.as_deref().is_some_and(|w| canon(w) == canon(&top)) => top,
        Some(top) if (mine || theirs.is_none()) && (repo.is_empty() || common(&top) == common(repo) || common(repo).is_none()) => top,
        _ => theirs.unwrap_or_else(|| repo.to_string()),
    }
}

/// Why a round can't look at `dir`: it's a checkout of another repository than the task's, so its
/// steps would pass on work the board doesn't track. None for one of the task's checkouts, or when
/// the task's repository isn't known.
fn outside_repo(dir: &str, v: &Value) -> Option<String> {
    let repo = v["vars"]["repo"].as_str().filter(|r| !r.is_empty())?;
    let common = |d: &str| git_out(d, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).and_then(|p| std::fs::canonicalize(p).ok());
    let mine = common(repo)?;
    if common(dir).as_ref() == Some(&mine) {
        return None;
    }
    let task = v["task"].as_str().unwrap_or("The task");
    let project = v["vars"]["project"].as_str().filter(|p| !p.is_empty()).unwrap_or("its project");
    Some(format!(
        "{dir} isn't a checkout of {repo}, {project}'s repository, so {task}'s steps can't be judged there. Aim at one of its checkouts \
         (tb step aim --worktree <dir>), or tb step aim --clear."
    ))
}

/// What a round looks at: `--worktree` (that checkout; a detached one with `--branch`, that branch's tip), `--branch` (the worktree that has it checked
/// out), `--commit` (that commit, which must be on the checkout's branch); with none of them, the aim
/// `tb step aim` saved, else this checkout's head.
fn aim(c: &Ctx, v: &Value, a: &Aim) -> Result<Aimed, String> {
    let a = if a.is_empty() { Aim::saved(v) } else { a.clone() };
    let given = |x: &Option<String>| x.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_string());
    let here = home_checkout(c, v);
    let mut detached_tip = None;
    let (dir, branch) = if let Some(w) = given(&a.worktree) {
        let dir = std::fs::canonicalize(&w).map_err(|_| format!("There's no folder {w}."))?.to_string_lossy().to_string();
        resolve_commit(&dir, "HEAD").map_err(|_| format!("{dir} isn't a git checkout."))?;
        let has = branch_in(&dir);
        if let (Some(want), Some(has)) = (given(&a.branch), has.as_deref()) {
            if want.trim_start_matches("refs/heads/") != has {
                return Err(format!("{dir} has {has} checked out, not {want}. Aim again with tb step aim."));
            }
        }
        // A detached checkout looks at the branch named with it: the branch's tip, which must exist and
        // have the checkout's head on it, as the Python board judged `refs/heads/<branch>`.
        if has.is_none() {
            if let Some(want) = given(&a.branch) {
                let b = want.trim_start_matches("refs/heads/").to_string();
                let tip = resolve_commit(&dir, &format!("refs/heads/{b}")).map_err(|_| format!("{dir} has no branch {b}."))?;
                if git_out(&dir, &["merge-base", "--is-ancestor", "HEAD", &tip]).is_none() {
                    return Err(format!("{dir}'s head isn't on {b}. Aim at a checkout of {b}, or name the branch its head is on."));
                }
                detached_tip = Some(tip);
                (dir, Some(b))
            } else {
                return Err(format!("{dir} isn't on a branch. Say which one with --branch."));
            }
        } else {
            (dir, has)
        }
    } else if let Some(b) = given(&a.branch) {
        let b = b.trim_start_matches("refs/heads/").to_string();
        let dir = worktree_with(if here.is_empty() { "." } else { &here }, &b)
            .ok_or_else(|| format!("No worktree has {b} checked out. Say which checkout with --worktree <dir> --branch {b}."))?;
        (dir, Some(b))
    } else if here.is_empty() {
        return Ok(Aimed { dir: None, head: None, branch: None, pinned: false });
    } else {
        // The default worktree too: a round on a detached checkout would run on branch "HEAD".
        let Some(b) = branch_in(&here) else {
            return Err(format!("{here} isn't on a branch. Say which one with --branch."));
        };
        (here, Some(b))
    };
    if let Some(why) = outside_repo(&dir, v) {
        return Err(why);
    }
    let checkout = resolve_commit(&dir, "HEAD").ok();
    let head = match given(&a.commit) {
        None => detached_tip.or_else(|| checkout.clone()),
        Some(r) => {
            let sha = resolve_commit(&dir, &r)?;
            // A commit must be on a branch, so the pin can be dropped once the branch moves on.
            let Some(b) = &branch else {
                return Err(format!("{dir} isn't on a branch. Say which one with --branch."));
            };
            if git_out(&dir, &["merge-base", "--is-ancestor", &sha, &format!("refs/heads/{b}")]).is_none() {
                return Err(format!("{} isn't on {b}.", &sha[..sha.len().min(12)]));
            }
            Some(sha)
        }
    };
    // The head isn't what the checkout has: the round runs on a checkout of it.
    let pinned = matches!((&head, &checkout), (Some(h), Some(t)) if h != t);
    Ok(Aimed { dir: Some(dir), head, branch, pinned })
}

/// A throwaway detached checkout of the commit a round is pinned to, so its check and script run on the
/// commit the round records; removed when the round ends.
struct PinnedCheckout {
    repo: String,
    path: String,
}

impl PinnedCheckout {
    fn for_round(at: &Aimed) -> Result<Option<PinnedCheckout>, String> {
        let (true, Some(repo), Some(sha)) = (at.pinned, at.dir.as_deref(), at.head.as_deref()) else { return Ok(None) };
        let short = &sha[..sha.len().min(12)];
        let path = std::env::temp_dir().join(format!("tb-round-{short}-{}", std::process::id())).to_string_lossy().to_string();
        let o = std::process::Command::new("git")
            .args(["-C", repo, "worktree", "add", "--detach", "--quiet", &path, sha])
            .output()
            .map_err(|e| format!("couldn't run git: {e}"))?;
        if !o.status.success() {
            return Err(format!("Couldn't check out {short} for the round: {}", String::from_utf8_lossy(&o.stderr).trim()));
        }
        out(&format!("Checked out {short} in {path} for this round."));
        Ok(Some(PinnedCheckout { repo: repo.to_string(), path }))
    }
}

impl Drop for PinnedCheckout {
    fn drop(&mut self) {
        let _ = std::process::Command::new("git").args(["-C", &self.repo, "worktree", "remove", "--force", &self.path]).output();
        let _ = std::fs::remove_dir_all(&self.path);
        let _ = std::process::Command::new("git").args(["-C", &self.repo, "worktree", "prune"]).output();
    }
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
    let (passed, output) = run_script(c, label, &steps::fill_shell(script, &vars), &vars, timeout);
    let result = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).filter(|v| v.is_object());
    let _ = std::fs::remove_file(&path);
    let passed = match result.as_ref().and_then(|r| r["verdict"].as_str()) {
        Some("pass") | Some("skip") => true,
        Some("fail") => false,
        _ => passed,
    };
    (passed, output, result)
}

/// The placeholders' values for a script: the board's, with this checkout's branch, or what the round
/// aims at (`{branch}`, `{head}`, and `{worktree}`, the folder it runs in).
fn step_vars(c: &Ctx, v: &Value, name: &str, at: &Aimed) -> BTreeMap<String, String> {
    let mut vars: BTreeMap<String, String> = serde_json::from_value(v["vars"].clone()).unwrap_or_default();
    let branch = at.branch.clone().or_else(|| {
        let dir = at.dir.as_deref().unwrap_or(&c.cwd);
        client::git_info(dir, 0.5)["branch"].as_str().filter(|b| !b.is_empty() && *b != "HEAD").map(|b| b.to_string())
    });
    if let Some(b) = branch {
        vars.insert("branch".into(), b);
    }
    if let Some(h) = &at.head {
        vars.insert("head".into(), h.clone());
    }
    if let Some(d) = &at.dir {
        vars.insert("worktree".into(), d.clone());
    }
    vars.insert("step".into(), name.to_string());
    vars
}

/// `step_vars` for a round, run in its pinned checkout when it has one.
fn round_vars(c: &Ctx, v: &Value, name: &str, at: &Aimed, pin: Option<&PinnedCheckout>) -> BTreeMap<String, String> {
    let mut vars = step_vars(c, v, name, at);
    if let Some(p) = pin {
        vars.insert("worktree".into(), p.path.clone());
    }
    vars
}

/// `tb step aim`: saves what the task's rounds look at (resolved here), or drops it with `--clear`.
fn aim_cmd(c: &Ctx, a: Aim, clear: bool, t: TaskArg) -> Result<i32, String> {
    let v = c.call("GET", &steps_path(c, t.task.clone())?, None)?;
    if !v["task"].is_string() {
        return Err(NO_TASK.into());
    }
    let task = t.task.or_else(|| v["task"].as_str().map(|s| s.to_string()));
    if clear {
        return c.run_report("tb.step_aim", json!({"clear": true}), task, true, |v| {
            format!("{}'s rounds look at the checkout they run from again.", v["task"].as_str().unwrap_or("The task"))
        });
    }
    if a.is_empty() {
        return Err("Say what the rounds look at: --branch <b>, --worktree <folder>, --commit <ref>, or --clear.".into());
    }
    let at = aim(c, &v, &a)?;
    let given = |x: &Option<String>| x.as_deref().is_some_and(|s| !s.trim().is_empty());
    let f = json!({
        "worktree": if given(&a.worktree) { json!(at.dir) } else { Value::Null },
        "branch": if given(&a.branch) || given(&a.commit) { json!(at.branch) } else { Value::Null },
        "sha": if given(&a.commit) { json!(at.head) } else { Value::Null },
        // The branch's tip now: once it moves on, the board drops the pin.
        "tip": match (given(&a.commit), at.dir.as_deref(), at.branch.as_deref()) {
            (true, Some(d), Some(b)) => json!(resolve_commit(d, &format!("refs/heads/{b}")).ok()),
            _ => Value::Null,
        },
    });
    c.run_report("tb.step_aim", f, task, true, |v| {
        let a = &v["aim"];
        let mut s = format!("{}'s rounds and gates now look at", v["task"].as_str().unwrap_or("The task"));
        if let Some(b) = a["branch"].as_str() {
            s += &format!(" {b}");
        }
        match a["sha"].as_str() {
            Some(sha) => s += &format!(" at {}", &sha[..sha.len().min(12)]),
            None => s += " (its latest commit)",
        }
        if let Some(w) = a["worktree"].as_str() {
            s += &format!(" in {w}");
        }
        s + "."
    })
}

/// Runs a step's script in the repo (else here), its output shown as it comes; the exit and the
/// output's tail.
fn run_script(c: &Ctx, label: &str, script: &str, vars: &BTreeMap<String, String>, timeout: u64) -> (bool, String) {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let dir = ["worktree", "repo"]
        .iter()
        .find_map(|k| vars.get(*k).filter(|r| !r.is_empty() && std::path::Path::new(r).is_dir()).cloned())
        .unwrap_or_else(|| c.cwd.clone());
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
            // A reader that went away (`tb step run … | head`) doesn't stop the script.
            out(&line);
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
        StepCmd::Done { name, note, skip, aim, t } => done_step(c, &name, note, skip, t, &aim),
        StepCmd::SetAim { aim, clear, t } => aim_cmd(c, aim, clear, t),
        StepCmd::Run { name, aim, t } => run_step(c, &name, t.task, &aim),
        StepCmd::Publish { name, t } => publish_step(c, &name, t.task),
        StepCmd::Again { name, aim, t } => {
            let (_, step) = find_step(c, t.task.clone(), &name)?;
            if step.owner || (step.run.is_empty() && step.check.is_empty()) {
                return Err(format!("“{}” has no check or script to run again: {}.", step.name, step.how("tb")));
            }
            if step.run.is_empty() {
                return done_step(c, &name, Some("Another round".into()), false, t, &aim);
            }
            run_step(c, &name, t.task, &aim)
        }
        StepCmd::Triage { name, finding, state, note, commit, t } => {
            let (v, step) = find_step(c, t.task.clone(), &name)?;
            let task = t.task.or_else(|| v["task"].as_str().map(|s| s.to_string()));
            // A ref (HEAD, a branch, a short sha) is kept as its sha; one this checkout doesn't have, as given.
            let commit = match commit.as_deref().map(str::trim).filter(|x| !x.is_empty()) {
                None => None,
                Some(r) => match resolve_commit(&c.cwd, r) {
                    Ok(sha) => Some(sha),
                    Err(_) if r.len() >= 7 && r.chars().all(|ch| ch.is_ascii_hexdigit()) => Some(r.to_string()),
                    Err(e) => return Err(e),
                },
            };
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
        StepCmd::Ask { name, t } => c.run_report("tb.step_ask", ask_fields(c, &name, t.task.clone())?, t.task, true, |v| {
            if v["already"] == true {
                return format!("{} is already done; carry on.", v["step"].as_str().unwrap_or("The step"));
            }
            let mut s = format!("Asked {} to do {}; the task waits for them. End your turn now.", c.cfg.owner, v["step"].as_str().unwrap_or("the step"));
            if let Some(o) = v["open"].as_str().filter(|o| !o.is_empty()) {
                s += &format!(" (It happens at {o}.)");
            }
            s
        }),
        StepCmd::Skip { name, why, t } => skip_step(c, &name, &why, t),
        StepCmd::Fail { name, why, t } => c.run_report("tb.step_fail", json!({"name": name, "why": why}), t.task, true, |v| {
            format!("Told {} that {} can't pass; the task waits for their answer. End your turn now.", c.cfg.owner, v["step"].as_str().unwrap_or("the step"))
        }),
    }
}

/// What `tb step ask` sends: the step, and the head, branch and worktree its question and the take handoff
/// fill in, resolved as `tb step done` resolves them (else this checkout's head, as an owner step's done records).
fn ask_fields(c: &Ctx, name: &str, task: Option<String>) -> Result<Value, String> {
    let v = c.call("GET", &steps_path(c, task)?, None)?;
    let at = aim(c, &v, &Aim::default()).unwrap_or_else(|_| {
        let here = home_checkout(c, &v);
        Aimed { head: local_head(&here), dir: Some(here).filter(|d| !d.is_empty()), branch: None, pinned: false }
    });
    Ok(json!({"name": name, "head": at.head, "branch": at.branch, "worktree": at.dir}))
}

/// `tb step done`: records the step, after its check (on what `aim` names) for agent work with one.
fn done_step(c: &Ctx, name: &str, note: Option<String>, skip: bool, t: TaskArg, aim_at: &Aim) -> Result<i32, String> {
    let (v, step) = find_step(c, t.task.clone(), name)?;
    let mine = !c.session.is_empty() && v["session"].as_str() == Some(c.session.as_str());
    if skip || (step.owner && !mine) {
        let task = v["task"].as_str().unwrap_or("").to_string();
        c.call("POST", &format!("/tasks/{task}/step"), Some(json!({"name": step.name, "note": note, "skip": skip, "session": c.session})))?;
        out(&format!("{} {} on {task}.", step.name, if skip { "skipped" } else { "done" }));
        return Ok(0);
    }
    let checked = !step.check.is_empty() && !step.owner;
    let at = match aim(c, &v, aim_at) {
        Ok(at) => at,
        // A step with no check records where it's done from, whatever the saved aim says.
        Err(_) if !checked && aim_at.is_empty() => Aimed { dir: None, head: local_head(&c.cwd), branch: None, pinned: false },
        Err(e) => return Err(e),
    };
    let mut f = json!({"name": step.name, "note": note, "via": "done", "head": at.head});
    if checked {
        round_gap(&v, &step.name)?;
        let pin = PinnedCheckout::for_round(&at)?;
        let vars = round_vars(c, &v, &step.name, &at, pin.as_ref());
        let (passed, output, result) = run_with_result(c, "Check", &step.check, &vars, step.timeout_secs());
        f["ok"] = json!(passed);
        f["output"] = json!(output);
        f["result"] = json!(result);
    }
    step_report(c, f, t.task.or_else(|| v["task"].as_str().map(|s| s.to_string())))
}

/// `tb step skip`: passes a skippable step on the aimed head (else this checkout's) without doing it.
fn skip_step(c: &Ctx, name: &str, why: &str, t: TaskArg) -> Result<i32, String> {
    let (v, step) = find_step(c, t.task.clone(), name)?;
    if !step.skippable {
        return Err(format!("“{}” isn't skippable: {}.", step.name, step.how("tb")));
    }
    if why.trim().is_empty() {
        return Err(format!("Say why “{}” doesn't apply: tb step skip \"{}\" --why \"<reason>\".", step.name, step.name));
    }
    let head = aim(c, &v, &Aim::default()).ok().and_then(|at| at.head).or_else(|| local_head(&c.cwd));
    let f = json!({"name": step.name, "via": "skip", "skip": true, "why": why, "head": head});
    step_report(c, f, t.task.or_else(|| v["task"].as_str().map(|s| s.to_string())))
}

/// `tb step run`: a script step's script, then its check, on this checkout or what `aim` names.
fn run_step(c: &Ctx, name: &str, task: Option<String>, aim_at: &Aim) -> Result<i32, String> {
    let (v, step) = find_step(c, task.clone(), name)?;
    if step.run.is_empty() {
        return Err(format!("“{}” has no script to run: {}.", step.name, step.how("tb")));
    }
    round_gap(&v, &step.name)?;
    let at = aim(c, &v, aim_at)?;
    let pin = PinnedCheckout::for_round(&at)?;
    let vars = round_vars(c, &v, &step.name, &at, pin.as_ref());
    let (mut passed, mut output, mut result) = run_with_result(c, "Script", &step.run, &vars, step.timeout_secs());
    if passed && !step.check.is_empty() {
        let (p, o, r) = run_with_result(c, "Check", &step.check, &vars, step.timeout_secs());
        passed = p;
        output = format!("{output}\n{o}");
        result = r.or(result);
    }
    let f = json!({"name": step.name, "via": "run", "ok": passed, "output": output, "result": result, "head": at.head});
    step_report(c, f, task.or_else(|| v["task"].as_str().map(|s| s.to_string())))
}

/// `tb step publish`: runs a step's `publish` script for the head it passed on (the aim's, else this
/// checkout's), with that round's result as `$TASKBOARD_RESULT`, and records it on the task.
fn publish_step(c: &Ctx, name: &str, task: Option<String>) -> Result<i32, String> {
    let (v, step) = find_step(c, task.clone(), name)?;
    if step.publish.trim().is_empty() {
        return Err(format!("“{}” has no publish script.", step.name));
    }
    // With no PR, a publish has nowhere to go, and its script's `{pr}` would be empty.
    if !has_pr(&v) {
        return Err(format!(
            "{} has no PR yet, so there's nothing to publish “{}” on. Open the PR, then publish it.",
            v["task"].as_str().unwrap_or("The task"),
            step.name
        ));
    }
    let at = aim(c, &v, &Aim::default())?;
    let head = at.head.clone().or_else(|| v["head"].as_str().map(|h| h.to_string()));
    let short = head.as_deref().map(|h| h[..h.len().min(12)].to_string()).unwrap_or_else(|| "this commit".into());
    let done = steps::from_listing(&v).into_iter().any(|(s, done)| done && steps::key(&s.name) == steps::key(&step.name));
    if !done {
        return Err(format!("“{}” hasn't passed on {short} yet: {}, then publish it.", step.name, step.how("tb")));
    }
    let task_id = task.clone().or_else(|| v["task"].as_str().map(|s| s.to_string()));
    // The round being published, for the script to read: the one that passed on this head.
    let round = publish_round(&v, &step, head.as_deref());
    let path = std::env::temp_dir().join(format!("tb-publish-{}-{}.json", std::process::id(), taskboardd::util::now_ts() as u64));
    let mut vars = step_vars(c, &v, &step.name, &at);
    if let Some(r) = &round {
        if std::fs::write(&path, r.to_string()).is_ok() {
            vars.insert("result".into(), path.to_string_lossy().to_string());
        }
    }
    let (passed, output) = run_script(c, "Publish", &steps::fill_shell(&step.publish, &vars), &vars, step.timeout_secs());
    let _ = std::fs::remove_file(&path);
    let f = json!({"name": step.name, "head": head, "ok": passed, "output": output});
    let reported = task_id.map(|t| task_ref(&t)).transpose()?;
    match c.report("tb.step_publish", f, reported.as_deref(), TB_TIMEOUT)? {
        None => out(SAVED),
        Some(v) if !v["task"].is_string() => out(NO_TASK),
        Some(_) if passed => out(&format!("Published {} for {short}.", step.name)),
        Some(_) => out(&format!("{} didn't publish (output above). Fix what it reports and run tb step publish \"{}\" again.", step.name, step.name)),
    }
    Ok(if passed { 0 } else { 1 })
}

/// The task in a `GET /steps` answer has a PR (`{pr}` or `{pr_url}`).
fn has_pr(v: &Value) -> bool {
    ["pr", "pr_url"].iter().any(|k| v["vars"][*k].as_str().is_some_and(|x| !x.trim().is_empty()))
}

/// The step's round that passed on `head` (any passing round, for a step that isn't per head), from
/// `passed_rounds` in `GET /steps`; none when the board doesn't say, or its round is on another commit.
fn publish_round(v: &Value, step: &Step, head: Option<&str>) -> Option<Value> {
    let r = v["passed_rounds"].as_object()?.iter().find(|(k, _)| steps::key(k) == steps::key(&step.name))?.1;
    let on_head = match (head, r["head"].as_str()) {
        _ if !step.counts_per_head() => true,
        (Some(h), Some(rh)) => steps::same_head(h, rh),
        (Some(_), None) => false,
        (None, _) => true,
    };
    (on_head && r["passed"] != false).then(|| r.clone())
}

/// Reports a step's outcome; a failed one exits 1 so the agent sees it.
fn step_report(c: &Ctx, f: Value, task: Option<String>) -> Result<i32, String> {
    let task = task.map(|t| task_ref(&t)).transpose()?;
    let passed = f["ok"] != false;
    let did = if f["skip"] == true { "skipped" } else { "done" };
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
            out(&if left.is_empty() { format!("{step} is {did}. No steps left.") } else { format!("{step} is {did}. Still to do: {}.", left.join(", ")) });
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
            let ignored = v["ignored"].as_array().cloned().unwrap_or_default();
            if all.is_empty() && ignored.is_empty() {
                out(&format!("{task} has no steps."));
            }
            // Placeholders follow the aim (`{branch}` is the aimed branch), else this checkout.
            let at = aim(c, &v, &Aim::default()).unwrap_or(Aimed { dir: None, head: None, branch: None, pinned: false });
            let vars = step_vars(c, &v, "", &at);
            for (st, done) in all {
                let when = match st.before {
                    steps::Before::Done => "before tb submit",
                    steps::Before::Merge => "before the merge",
                    steps::Before::Pr => "before the PR",
                };
                let st = st.filled(&vars);
                let gap = st.gap_text().filter(|_| st.gap_secs().is_some()).map(|g| format!("; {g}")).unwrap_or_default();
                out(&format!("[{}] {} ({when}{gap}): {}. {}", if done { "done" } else { "to do" }, st.name, st.what(), st.how("tb")));
            }
            // Steps config.toml has but can't run: left out, so not asked for, until the owner fixes them.
            for b in &ignored {
                out(&format!("[left out] {}: {}", b["name"].as_str().unwrap_or("?"), b["why"].as_str().unwrap_or("")));
            }
            if v["aim"].is_object() {
                let a = &v["aim"];
                let bits: Vec<String> = [("branch", ""), ("sha", "at "), ("worktree", "in ")]
                    .iter()
                    .filter_map(|(k, pre)| a[*k].as_str().map(|x| format!("{pre}{}", if *k == "sha" { &x[..x.len().min(12)] } else { x })))
                    .collect();
                out(&format!("Rounds look at {} (tb step aim --clear drops it).", bits.join(" ")));
                if let Some(sha) = a["dropped"].as_str() {
                    out(&format!(
                        "The pin at {} was dropped: {} has moved on since.",
                        &sha[..sha.len().min(12)],
                        a["branch"].as_str().unwrap_or("its branch")
                    ));
                }
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
            let f = checkpoint_body(done, next, decisions, files)?;
            c.run_report("tb.checkpoint", f, t.task, true, |v| format!("Checkpoint saved on {}.", v["task"].as_str().unwrap_or("")))
        }
        Cmd::Found { title, kind, detail, output, misses_goal, t } => c.run_report(
            "tb.found",
            json!({"title": short_title(&title)?, "kind": kind, "detail": detail, "output": output.map(|o| o.chars().take(4000).collect::<String>()), "misses_goal": misses_goal}),
            t.task,
            false,
            |v| {
                if v["misses_goal"] == true {
                    format!("Sent {} to the owner as a miss on the goal's done-when; the goal stays open until it's settled.", v["issue"].as_str().unwrap_or(""))
                } else if v["seen"] == true {
                    format!("Already in the backlog as {}; added this sighting. To look for related work: tb search \"<words>\".", v["issue"].as_str().unwrap_or(""))
                } else {
                    format!("Added {} to the backlog.", v["issue"].as_str().unwrap_or(""))
                }
            },
        ),
        Cmd::Question { text, options, t } => {
            let task = t.task.map(|x| task_ref(&x)).transpose()?;
            let mut alert = Some(text.clone());
            // The board may have claude screen it first (`[questions] screen`), which takes a while.
            let r = c.report_progress("tb.question", json!({"text": text, "options": options}), task.as_deref());
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
        Cmd::Withdraw { why, t } => c.run_report("tb.withdraw", json!({"why": why}), t.task, true, |v| {
            format!("Withdrew the question on {}; it's no longer waiting on {}.", v["task"].as_str().unwrap_or("the task"), c.cfg.owner)
        }),
        Cmd::Submit { summary, pr, pr_body, pr_title, pr_branch, no_pr, no_evidence, no_lens, merged_note, human, t } => {
            let mut f = json!({"summary": summary, "pr": pr, "human": human, "no_pr": no_pr, "no_evidence": no_evidence, "no_lens": no_lens, "merged_note": merged_note});
            let Some(file) = pr_body else {
                // The board runs the task.finishing hooks and checks the diff before it answers.
                let task = t.task.map(|x| task_ref(&x)).transpose()?;
                match c.report_progress("tb.done", f, task.as_deref())? {
                    None => out(SAVED),
                    Some(v) if !v["task"].is_string() => out(NO_TASK),
                    Some(v) => out(&format!("{} is submitted.{}", v["task"].as_str().unwrap_or(""), then_next(&v))),
                }
                return Ok(0);
            };
            let text = read_body(&file)?;
            let problems = taskboardd::propen::body_problems(&c.cfg.pr_body, &text);
            if !problems.is_empty() {
                return Err(format!("the PR description needs work first:\n- {}", problems.join("\n- ")));
            }
            f["pr_body"] = json!(text);
            f["pr_title"] = json!(pr_title);
            f["pr_branch"] = json!(pr_branch);
            // Not spooled: the PR opens now or not at all. On the progress stream: the board fetches,
            // checks the branch, opens the PR and runs the finishing hooks, a stage at a time.
            let task = t.task.map(|x| task_ref(&x)).transpose()?;
            let body = c.body("tb.done", f, task.as_deref());
            let v = match client::request_progress(&c.cfg, "POST", "/report", Some(&body), TB_TIMEOUT, |stage| out(&format!("· {stage}"))) {
                Ok(v) => v,
                Err(CallError::Refused(m)) => return Err(m),
                Err(CallError::Unreachable(e)) => return Err(format!("the board isn't answering ({e}), so it can't open the PR. Try again once it's back.")),
                Err(CallError::TimedOut(e)) => {
                    return Err(format!("the board went quiet for {TB_TIMEOUT}s ({e}); it may still be opening the PR. Check with tb pr status before trying again."))
                }
            };
            if !v["task"].is_string() {
                out(NO_TASK);
                return Ok(0);
            }
            let pr = v["pr_url"].as_str().map(|u| format!(" Its PR: {u}")).unwrap_or_default();
            out(&format!("{} is submitted.{pr}{}", v["task"].as_str().unwrap_or(""), then_next(&v)));
            Ok(0)
        }
        Cmd::Fail { reason, t } => {
            c.run_report("tb.fail", json!({"reason": reason}), t.task, true, |v| format!("{} is marked failed.{}", v["task"].as_str().unwrap_or(""), then_next(v)))
        }
        Cmd::Take { task } => {
            let r = task_ref(&task)?;
            match c.report("tb.take", json!({"task": r}), None, TB_TIMEOUT)? {
                None => out(SAVED),
                Some(v) => out(v["context"].as_str().unwrap_or(&format!("Took {r}."))),
            }
            Ok(0)
        }
        Cmd::Switch { task } => {
            let r = task_ref(&task)?;
            match c.report("tb.switch", json!({"to": r}), None, TB_TIMEOUT)? {
                None => out(SAVED),
                Some(v) => out(v["context"].as_str().unwrap_or(&format!("Switched to {r}."))),
            }
            Ok(0)
        }
        Cmd::Line { action } => {
            let body = match action {
                Some(LineCmd::Drop { task }) => json!({"drop": task_ref(&task)?}),
                None => json!({}),
            };
            match c.report("tb.line", body, None, TB_TIMEOUT)? {
                None => out("The board isn't answering."),
                Some(v) => out(v["context"].as_str().unwrap_or("")),
            }
            Ok(0)
        }
        Cmd::Start { task, queue, now } => {
            if task.trim().starts_with(['G', 'g']) {
                let g = goal_ref(&task)?;
                if queue {
                    return Err(format!("--queue doesn't apply to a goal: tb start {g} runs it, and the board starts its tasks in their waves."));
                }
                if c.session.is_empty() {
                    return Err(goal_run_needs_terminal("tb start", &g));
                }
                let v = c.call("POST", &format!("/goals/{g}/run"), Some(json!({"via_session": c.session, "now": now})))?;
                let n = v["queued_now"].as_i64().unwrap_or(0);
                let until = v["hours_until"].as_str().filter(|_| now).map(|at| format!(" It runs outside work hours until they open ({}).", taskboardd::hours::say_when(at))).unwrap_or_default();
                out(&format!("{g} runs: queued {n} planned task{}. The board starts them in their waves.{until}", if n == 1 { "" } else { "s" }));
                out_play_waits(&v);
                return Ok(0);
            }
            if now {
                return Err(format!("--now only applies to a goal (tb start G<n> --now). Start {} without it.", task.trim()));
            }
            let item = task.trim().starts_with(['B', 'b']);
            if item && c.session.is_empty() {
                return Err(format!("tb start only works in a Midna terminal, on a human's word there. A human can make {} a task and press Start on it.", issue_ref(&task)?));
            }
            let r = if item { item_task(c, &task)? } else { task_ref(&task)? };
            if c.session.is_empty() {
                return Err(format!("tb start only works in a Midna terminal, on a human's word there. A human can press Start on {r} on the board."));
            }
            let mode = if queue { "queue" } else { "new" };
            let v = c.call_progress("POST", &format!("/tasks/{r}/start"), Some(json!({"mode": mode, "via_session": c.session})))?;
            out(&start_said(&r, queue, &v));
            Ok(0)
        }
        Cmd::Preview { url, port } => crate::preview::run(&url, port),
        Cmd::Focus { task, port } => crate::preview::focus(&task_ref(&task)?, port),
        Cmd::Export { out: dir } => {
            let path = taskboardd::export::run(&c.cfg, "tb", &dir.unwrap_or_else(taskboardd::export::default_dir))?;
            out(&path.display().to_string());
            Ok(0)
        }
        Cmd::Peek { midna } => {
            // whoami, not a tb.status report: a report marks the terminal active and hands it deliveries.
            if c.session.is_empty() {
                if midna {
                    out("[]");
                    return Ok(0);
                }
                return Err("tb peek only works inside a Midna terminal.".into());
            }
            match c.call("GET", &format!("/whoami?session={}", c.session), None) {
                Ok(v) if midna => {
                    let o = taskboardd::statusbar::Options::from_whoami(&v);
                    out(&taskboardd::statusbar::segments(&v, o).to_string())
                }
                Ok(v) => out(&v.to_string()),
                // A status bar hides its item while the board is down rather than showing an error.
                Err(_) if midna => out("[]"),
                Err(e) => return Err(e),
            }
            Ok(0)
        }
        Cmd::Status { t } => {
            if let Some(x) = t.task {
                let v = c.call("GET", &format!("/tasks/{}", task_ref(&x)?), None)?;
                out(&format!("{} · {} · {}{}", v["ref"].as_str().unwrap_or(""), v["title"].as_str().unwrap_or(""), v["status"].as_str().unwrap_or(""), area_bit(&v)));
                if let Some(l) = v["stack_on"]["line"].as_str() {
                    out(l);
                }
                return Ok(0);
            }
            let v = c.ask("tb.status")?;
            out(v["context"].as_str().unwrap_or("No task on this terminal."));
            Ok(0)
        }
        Cmd::WaitFor { tasks, why, merged, t } => {
            if tasks.is_empty() {
                return Err("say which task this one needs, for example: tb wait-for T14 (or none)".into());
            }
            let list = if tasks.len() == 1 && tasks[0].eq_ignore_ascii_case("none") {
                json!("none")
            } else {
                json!(tasks.iter().map(|x| task_ref(x)).collect::<Result<Vec<_>, _>>()?)
            };
            let mut body = json!({"tasks": list, "why": why});
            if merged {
                body["merged"] = json!(true);
            }
            c.run_report("tb.wait_for", body, t.task, true, |v| {
                if v["moved_on"] == true {
                    format!("{} waits now; this terminal moves on.\n\n{}", v["task"].as_str().unwrap_or(""), v["context"].as_str().unwrap_or(""))
                } else if v["parked"] == true {
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
        Cmd::Propose { goal, tasks, planned, not_backlog } => {
            let g = goal_ref(&goal)?;
            propose_tasks(&tasks)?;
            short_task_titles(&tasks)?;
            match c.report("tb.propose", json!({"goal": g, "tasks": tasks, "planned": planned, "not_backlog": not_backlog}), None, TB_TIMEOUT)? {
                None => out(SAVED),
                Some(v) => {
                    out(&format!("Added {} to {g} ({}).", v["created"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
                                 v["why"].as_str().unwrap_or("planned")));
                    print_warnings(&v);
                    for (r, t) in v["created"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).zip(&tasks) {
                        midna_pin(r, t.split_once("::").map_or(t.as_str(), |(t, _)| t));
                    }
                }
            }
            Ok(0)
        }
        Cmd::Jira { job, result, rest } => jira_cmd(c, job, result, rest),
        Cmd::Planning { project, suggest, accept } => match (suggest, accept) {
            (false, false) => planning_cmd(c, project),
            _ => planning_lessons_cmd(c, project, accept),
        },
        Cmd::Quality { project, all, goals } => quality_cmd(c, project, all, goals),
        Cmd::Review { target, since, week, save, list } => review_cmd(c, target, since, week, save, list),
        Cmd::Lessons { project, all, state, kind } => lessons_cmd(c, project, all, state, kind),
        Cmd::Search { query, project, all, kind, open } => search_cmd(c, &query, project, all, kind, open),
        Cmd::Lesson { action } => lesson_cmd(c, action),
        Cmd::Idea { action } => idea_cmd(c, action),
        Cmd::Releases { project } => releases_cmd(c, project),
        Cmd::Release { action } => release_cmd(c, action),
        Cmd::Areas { action } => areas_cmd(c, action),
        Cmd::Close { done: _, project, goal } => {
            let goal = goal.map(|g| goal_ref(&g)).transpose()?;
            let v = c.call("POST", "/done/close-terminals", Some(json!({"project": project, "goal": goal})))?;
            out(&close_done_line(v["jobs"].as_i64().unwrap_or(0)));
            Ok(0)
        }
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
        Cmd::Slots { project } => {
            let path = match project {
                Some(p) => format!("/slots?project={}", qv(&p)),
                None => "/slots".to_string(),
            };
            let v = c.call("GET", &path, None)?;
            let slots = v["slots"].as_array().cloned().unwrap_or_default();
            if slots.is_empty() {
                out("No worktree slots yet.");
            }
            for s in slots {
                let holder = s["held_by"].as_str().map(|h| format!("held by {h}")).unwrap_or_else(|| "free".into());
                let used = s["used_at"].as_str().map(|u| format!(" · last used {}", local_when(u))).unwrap_or_default();
                let size = s["size_kb"].as_i64().map(|k| format!(" · {}", disk_size(k))).unwrap_or_default();
                let waiting = s["waiting"].as_array().filter(|a| !a.is_empty()).map(|a| format!(" · waiting: {}", a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))).unwrap_or_default();
                out(&format!("{} · {} · {holder}{used}{size}{waiting}", s["name"].as_str().unwrap_or(""), s["project"].as_str().unwrap_or("")));
            }
            Ok(0)
        }
        Cmd::Slot { action } => slot_cmd(c, action),
        Cmd::Stage { files, task } => {
            let t = c.pr_task(task)?;
            out(&hook::stage_own(&c.cfg.data, &t, std::path::Path::new(&c.cwd), &files)?);
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
        Cmd::Feed { action } => feed_cmd(c, action),
        Cmd::Stream { action: Some(StreamCmd::Send { json }), .. } => stream_send(c, json),
        Cmd::Stream { action: Some(StreamCmd::Mode { source, mode }), .. } => {
            let v = c.call("POST", "/stream/mode", Some(json!({"source": source, "mode": mode})))?;
            out(&stream_lines(&v));
            Ok(0)
        }
        Cmd::Stream { action: None, json } => {
            let v = c.call("GET", "/stream", None)?;
            out(&if json { serde_json::to_string_pretty(&v).unwrap_or_default() } else { stream_lines(&v) });
            Ok(0)
        }
        Cmd::PrBuilds { action } => pr_builds_cmd(c, action),
        Cmd::Master { args, why, who, proof, title, open, limit } => master_cmd(c, args, Verdict { why, who, proof, title }, open, limit),
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
        Cmd::Reviewers { action } => reviewers_cmd(c, action.unwrap_or(ReviewersCmd::List { project: None, all: false })),
        Cmd::Goals { project } => {
            let v = c.call("GET", &format!("/goals?project={}", project.unwrap_or_else(|| "all".into())), None)?;
            let goals = v["goals"].as_array().cloned().unwrap_or_default();
            if goals.is_empty() {
                out("No goals.");
            }
            for g in goals {
                out(&format!(
                    "{} · {} · {}{} · {}{}",
                    g["ref"].as_str().unwrap_or(""),
                    g["name"].as_str().unwrap_or(""),
                    goal_projects(&g),
                    area_bit(&g),
                    done_bit(&g),
                    if g["paused"] == true { " · paused" } else { "" }
                ));
            }
            Ok(0)
        }
        Cmd::Goal { action } => match action {
            GoalCmd::New { name, outcome, tldr, project, product, after, tasks, area, not_backlog } => {
                short_task_titles(&tasks)?;
                let after = if after.is_empty() { Value::Null } else { json!(after) };
                let body = json!({"name": name, "outcome": outcome, "tldr": tldr, "project": project, "product": product, "after": after, "tasks": tasks, "area": area, "not_backlog": not_backlog});
                match c.report("tb.goal", body, None, TB_TIMEOUT)? {
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
                        midna_pin(v["goal"].as_str().unwrap_or(""), v["name"].as_str().unwrap_or(""));
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
            GoalCmd::Set { goal, name, outcome, tldr, paused, in_order, max_terminals, epic, product, jira, worktrees, lane, run, deprioritize, prioritize, area, devices, after } => {
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
                if let Some(x) = jira {
                    b["jira_off"] = json!(x == "off");
                }
                if let Some(x) = worktrees {
                    b["worktree_base"] = json!(x);
                }
                if let Some(x) = lane {
                    b["lane"] = json!(x);
                }
                if let Some(x) = area {
                    b["area"] = json!(x);
                }
                if !devices.is_empty() {
                    b["devices"] = json!(lock_arg(&devices));
                }
                if !after.is_empty() {
                    b["after"] = json!(after);
                }
                if deprioritize || prioritize {
                    b["deprioritized"] = json!(deprioritize);
                }
                let empty = b.as_object().map(|o| o.is_empty()).unwrap_or(true);
                if empty && !run {
                    return Err("say what to change, for example: tb goal set G3 --paused on".into());
                }
                if run {
                    // Ask first: a run the board refuses leaves the goal as it was.
                    if c.session.is_empty() {
                        return Err(goal_run_needs_terminal("tb goal set --run", &g));
                    }
                    c.call("POST", &format!("/goals/{g}/run"), Some(json!({"via_session": c.session, "check": true})))?;
                }
                if !empty {
                    let v = c.call("POST", &format!("/goals/{g}"), Some(b))?;
                    out(&format!("Changed {} “{}”.", g, v["name"].as_str().unwrap_or("")));
                }
                if run {
                    let v = c.call("POST", &format!("/goals/{g}/run"), Some(json!({"via_session": c.session})))?;
                    let n = v["queued_now"].as_i64().unwrap_or(0);
                    out(&format!("{g} runs: queued {n} planned task{}.", if n == 1 { "" } else { "s" }));
                    out_play_waits(&v);
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
                    c.call("POST", &format!("/goals/{g}/waves/{wave}/hold"), Some(json!({"on": h == "on"})))?;
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
                let v = c.call("POST", &format!("/goals/{g}/waves/{wave}/continue"), Some(json!({})))?;
                out(&if v["let_start"] == true { format!("Wave {wave} of {g} can start now.") } else { format!("{g} goes on past wave {wave}.") });
                Ok(0)
            }
            GoalCmd::Devices { goal, add, purpose, reserve, unreserve, remove } => {
                let g = goal_ref(&goal)?;
                let v = match (add, remove) {
                    (Some(d), _) => {
                        let mut b = json!({"device": d});
                        if let Some(p) = purpose {
                            b["purpose"] = json!(p);
                        }
                        if reserve || unreserve {
                            b["reserved"] = json!(reserve);
                        }
                        c.call("POST", &format!("/goals/{g}/devices"), Some(b))?
                    }
                    (None, Some(d)) => c.call("POST", &format!("/goals/{g}/devices/{d}/remove"), Some(json!({})))?,
                    (None, None) => c.call("GET", &format!("/goals/{g}/devices"), None)?,
                };
                let devices = v["devices"].as_array().cloned().unwrap_or_default();
                if devices.is_empty() {
                    out(&format!("{g} has no devices of its own; its tasks use the shared pool."));
                }
                for d in devices {
                    out(&goal_device_line(&d));
                }
                Ok(0)
            }
            GoalCmd::Done { goal, why } => {
                let g = goal_ref(&goal)?;
                let v = c.call("POST", &format!("/goals/{g}/accept"), Some(json!({"why": why})))?;
                out(&format!("{g}: {}", v["state"].as_str().unwrap_or("")));
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
        Cmd::Qa { what, r#ref, note, pr, no_pr, limit, goal } => {
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
                let v = c.call("GET", &qa_list_url(limit, what == "waiting", goal.as_deref()), None)?;
                if v["on"] == false {
                    out("QA comments are off. The owner switches them on in Taskboard's Settings ▸ QA.");
                }
                let rows = v["comments"].as_array().cloned().unwrap_or_default();
                if rows.is_empty() {
                    out(if goal.is_some() {
                        "No QA questions on this goal's tickets."
                    } else if what == "waiting" {
                        "No QA comment is waiting on the owner."
                    } else {
                        "No Jira comments on board tickets yet."
                    });
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
            let mut b = json!({"action": what});
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
            TaskCmd::New { title, issue, detail, goal, also, wave, files, project, planned, here, next, now, waits_for, lock, alone, lane, bring, jira, ticket_summary, ticket_text, devices, bits, stack_on, stack_merge, pr, no_pr, area, model, effort, human, kind, not_backlog } => {
                if wave.is_some() && goal.is_none() {
                    return Err("--wave needs --goal: waves are a goal's".into());
                }
                if !files.is_empty() && goal.is_none() {
                    return Err("--file needs --goal: it tells the task's wave mates which files are its".into());
                }
                if here && !also.is_empty() {
                    return Err("--here takes no --also: give the task its one goal with --goal G<n>".into());
                }
                if !also.is_empty() && goal.is_none() {
                    return Err("--also needs --goal: the task's home goal, which runs it".into());
                }
                let goal = goal.map(|g| goal_ref(&g)).transpose()?;
                let also = also.iter().map(|g| goal_ref(g)).collect::<Result<Vec<_>, _>>()?;
                let title = title.as_deref().map(short_title).transpose()?.unwrap_or_default();
                let mut body = json!({"title": title, "detail": detail, "goal": goal, "project": project, "planned": planned, "area": area, "not_backlog": not_backlog});
                if let Some(i) = issue {
                    body["issue"] = json!(issue_ref(&i)?);
                }
                if !also.is_empty() {
                    body["also"] = json!(also);
                }
                if here {
                    body["here"] = json!(true);
                }
                if human {
                    body["kind"] = json!("human");
                }
                if let Some(k) = kind {
                    body["kind"] = json!(k);
                }
                if next || now {
                    body["line"] = json!(if next { "next" } else { "now" });
                }
                if let Some(w) = wave {
                    body["wave"] = json!(w);
                }
                if !files.is_empty() {
                    body["files"] = json!(files);
                }
                if waits_for.iter().any(|x| x.eq_ignore_ascii_case("none")) {
                    if waits_for.len() > 1 {
                        return Err("--waits-for none waits for nothing; leave out the other --waits-for".into());
                    }
                    body["waits_for"] = json!("none");
                } else if !waits_for.is_empty() {
                    body["waits_for"] = json!(waits_for.iter().map(|x| task_ref(x)).collect::<Result<Vec<_>, _>>()?);
                }
                if !lock.is_empty() {
                    body["locks"] = json!(lock_arg(&lock));
                }
                if let Some(a) = alone {
                    body["alone"] = json!(a);
                }
                if bring.is_some() && lane.as_deref() != Some("slot") {
                    return Err("--bring needs --lane slot: it claims the task's worktree slot".into());
                }
                if let Some(l) = &lane {
                    body["lane"] = json!(l);
                }
                if let Some(m) = model {
                    body["model"] = json!(m);
                }
                if let Some(e) = effort {
                    body["effort"] = json!(e);
                }
                if let Some(j) = jira {
                    body["jira"] = jira_arg(&j);
                }
                if let Some(x) = ticket_summary {
                    body["ticket_summary"] = json!(x);
                }
                if let Some(x) = ticket_text {
                    body["ticket_text"] = json!(x);
                }
                if let Some(x) = stack_on {
                    body["stack_on"] = json!(task_ref(&x)?);
                }
                if let Some(x) = stack_merge {
                    body["stack_merge"] = json!(x);
                }
                if pr || no_pr {
                    body["ships_pr"] = json!(pr);
                }
                // In the same request, so an unknown bit or device tag adds no task at all.
                if !devices.is_empty() {
                    body["devices"] = device_arg(&devices);
                }
                if !bits.is_empty() {
                    body["bits"] = json!(lock_arg(&bits));
                }
                match c.report("tb.new_task", body, None, TB_TIMEOUT)? {
                    None => out(SAVED),
                    Some(v) if here => {
                        let r = v["created"][0].as_str().unwrap_or("").to_string();
                        out(v["context"].as_str().filter(|s| !s.trim().is_empty()).map(|s| s.to_string()).unwrap_or(format!("Added {r} and you're on it.")).as_str());
                        midna_pin(&r, v["title"].as_str().unwrap_or(&title));
                        if lane.as_deref() == Some("slot") {
                            slot_claim(c, &r, bring, &[])?;
                        }
                    }
                    Some(v) => {
                        let r = v["created"][0].as_str().unwrap_or("").to_string();
                        let where_ = match v["goal"].as_str() {
                            Some(g) => format!(" in {g} ({})", v["why"].as_str().unwrap_or("planned")),
                            None if human => " on the board; it goes to the owner as soon as nothing holds it back, and anything waiting for it goes on once they press Done".to_string(),
                            None => format!(" on the board; it waits for the owner to press Start (or `tb start {r}`, only if they told you to start it)"),
                        };
                        out(&format!("Added {r}{where_}. {}#/?task={r}", c.cfg.page_url));
                        print_warnings(&v);
                        midna_pin(&r, v["title"].as_str().unwrap_or(&title));
                    }
                }
                Ok(0)
            }
            TaskCmd::Set { task, title, detail, goal, also, not_also, wave, priority, waits_for, lock, alone, lane, jira, ticket_summary, ticket_text, devices, bits, not_bits, stack_on, stack_merge, pr, kind, status, merged_note, area, model, effort, project } => {
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
                if let Some(x) = lane {
                    b["lane"] = json!(x);
                }
                if let Some(x) = model {
                    b["model"] = json!(x);
                }
                if let Some(x) = effort {
                    b["effort"] = json!(x);
                }
                if let Some(x) = jira {
                    b["jira_key"] = json!(x);
                }
                if let Some(x) = ticket_summary {
                    b["ticket_summary"] = json!(x);
                }
                if let Some(x) = ticket_text {
                    b["ticket_text"] = json!(x);
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
                if let Some(x) = stack_merge {
                    b["stack_merge"] = json!(x);
                }
                if let Some(x) = kind {
                    b["kind"] = json!(x);
                }
                if let Some(x) = pr {
                    b["ships_pr"] = json!(x);
                }
                if let Some(x) = status {
                    b["status"] = json!(x);
                }
                if let Some(x) = merged_note {
                    b["merged_note"] = if x.eq_ignore_ascii_case("none") { json!("") } else { json!(x) };
                }
                if let Some(x) = area {
                    b["area"] = json!(x);
                }
                if let Some(x) = project {
                    b["project"] = json!(x);
                }
                if b.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                    return Err("say what to change, for example: tb task set T12 --priority high".into());
                }
                let moving = b.get("project").is_some();
                let v = c.call("POST", &format!("/tasks/{t}"), Some(b))?;
                let mut bits = String::new();
                if moving {
                    bits += &format!(" · in {}", v["project"].as_str().unwrap_or(""));
                }
                for x in lock_bits(&v) {
                    bits += &format!(" · {x}");
                }
                if let Some(l) = v["stack_on"]["line"].as_str() {
                    bits += &format!(" · {l}");
                }
                if v["ships_pr_set"].is_boolean() {
                    bits += if v["ships_pr"] == true { " · ends in a PR" } else { " · no PR" };
                }
                if let Some(why) = v["why"].as_str() {
                    bits += &format!(" · {why}");
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
            TaskCmd::Show { task, log, json } => {
                let t = task_ref(&task)?;
                let mut v = c.call("GET", &format!("/tasks/{t}"), None)?;
                // The detail carries only the newest page of the log.
                let have = v["log"].as_array().map_or(0, |a| a.len());
                if log > have && v["log_total"].as_u64().is_some_and(|n| n as usize > have) {
                    v["log"] = c.call("GET", &format!("/tasks/{t}/log?limit={}", log.min(500)), None)?["log"].take();
                }
                if json {
                    out(&serde_json::to_string_pretty(&v).unwrap_or_default());
                } else {
                    task_show_lines(&v, log).iter().for_each(|l| out(l));
                }
                Ok(0)
            }
        },
        Cmd::Tasks { project, goal, status, json } => {
            let mut q = list_query(c, project, goal)?;
            if let Some(s) = status {
                q.push(format!("status={}", qv(&s)));
            }
            let v = c.call("GET", &format!("/tasks?{}", q.join("&")), None)?;
            if json {
                out(&serde_json::to_string_pretty(&v).unwrap_or_default());
                return Ok(0);
            }
            let tasks = v["tasks"].as_array().cloned().unwrap_or_default();
            if tasks.is_empty() {
                out("No tasks.");
            }
            tasks.iter().for_each(|t| out(&task_list_line(t)));
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::List { project, goal, kind, state, sort, json } } => {
            let mut q = list_query(c, project, goal)?;
            for (k, v) in [("kind", kind), ("state", state), ("sort", sort)] {
                if let Some(v) = v {
                    q.push(format!("{k}={}", qv(&v)));
                }
            }
            let v = c.call("GET", &format!("/backlog?{}", q.join("&")), None)?;
            if json {
                out(&serde_json::to_string_pretty(&v).unwrap_or_default());
                return Ok(0);
            }
            let issues = v["issues"].as_array().cloned().unwrap_or_default();
            if issues.is_empty() {
                out("No backlog issues.");
            }
            issues.iter().for_each(|b| out(&issue_list_line(b)));
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::Show { issue, json } } => {
            let b = issue_ref(&issue)?;
            let mut v = c.call("GET", &format!("/backlog/{b}"), None)?;
            if json {
                out(&serde_json::to_string_pretty(&v).unwrap_or_default());
            } else {
                // The detail carries only the oldest page of the history; the lines show the newest.
                let have = v["history"].as_array().map_or(0, |a| a.len());
                if v["history_total"].as_u64().is_some_and(|n| n as usize > have) {
                    let mut newest = c.call("GET", &format!("/backlog/{b}/history?limit=5"), None)?["history"].take();
                    if let Some(a) = newest.as_array_mut() {
                        a.reverse();
                    }
                    v["history"] = newest;
                }
                issue_show_lines(&v).iter().for_each(|l| out(l));
            }
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::Add { title, kind, detail, goal, project, area } } => {
            let goal = goal.map(|g| goal_ref(&g)).transpose()?;
            let name = c.who();
            let v = c.call(
                "POST",
                "/backlog",
                Some(json!({"title": short_title(&title)?, "kind": kind, "detail": detail, "goal_id": goal, "project": project, "area": area, "cwd": c.cwd, "where": "tb", "who": name})),
            )?;
            out(&format!("Added {} to the backlog.", v["ref"].as_str().unwrap_or("")));
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::Note { issue, text } } => {
            let r = issue_ref(&issue)?;
            c.call("POST", &format!("/backlog/{r}/note"), Some(json!({"text": text})))?;
            out(&format!("Added a note to {r}."));
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::Set { issue, title, detail, area, project } } => {
            let r = issue_ref(&issue)?;
            let mut b = json!({"who": c.who()});
            if let Some(x) = title {
                b["title"] = json!(short_title(&x)?);
            }
            if let Some(x) = detail {
                b["detail"] = json!(x);
            }
            if let Some(x) = area {
                b["area"] = json!(x);
            }
            if let Some(x) = project {
                b["project"] = json!(x);
            }
            if b.as_object().map_or(0, |o| o.len()) < 2 {
                return Err("say what to change, for example: tb backlog set B3 --title \"Login button flickers\"".into());
            }
            let moving = b.get("project").is_some();
            let v = c.call("POST", &format!("/backlog/{r}"), Some(b))?;
            let issue = v.get("issue").filter(|x| x.is_object()).unwrap_or(&v);
            let place = if moving { format!(" · in {}", issue["project"].as_str().unwrap_or("")) } else { String::new() };
            out(&format!("Changed {r} “{}”{place}.", issue["title"].as_str().unwrap_or("")));
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::Move { mut args } } => {
            let goal = args.pop().unwrap_or_default();
            let to = if goal.trim().eq_ignore_ascii_case("none") { Value::Null } else { json!(goal_ref(&goal)?) };
            if let [issue] = args.as_slice() {
                let r = issue_ref(issue)?;
                let v = c.call("POST", &format!("/backlog/{r}/move"), Some(json!({"goal_id": to, "who": c.who()})))?;
                let issue = v.get("issue").filter(|x| x.is_object()).unwrap_or(&v);
                match issue["goal"]["ref"].as_str().or(to.as_str()) {
                    Some(g) => out(&format!("Moved {r} to {g}.")),
                    None => out(&format!("Moved {r} out of its goal.")),
                }
                return Ok(0);
            }
            backlog_bulk(c, "move", &args, json!({"goal_id": to}))?;
            let refs = issue_refs(&args)?.join(", ");
            match to.as_str() {
                Some(g) => out(&format!("Moved {refs} to {g}.")),
                None => out(&format!("Moved {refs} out of their goals.")),
            }
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::Task { issues, here: true, next, now, .. } } => {
            let [issue] = issues.as_slice() else {
                return Err("--here takes one issue: this terminal works on one task at a time".into());
            };
            let mut body = json!({"title": "", "issue": issue_ref(issue)?, "here": true});
            if next || now {
                body["line"] = json!(if next { "next" } else { "now" });
            }
            match c.report("tb.new_task", body, None, TB_TIMEOUT)? {
                None => out(SAVED),
                Some(v) => {
                    let r = v["created"][0].as_str().unwrap_or("").to_string();
                    out(v["context"].as_str().filter(|s| !s.trim().is_empty()).map(|s| s.to_string()).unwrap_or(format!("Made {issue} into {r} and you're on it.")).as_str());
                    midna_pin(&r, v["title"].as_str().unwrap_or(""));
                }
            }
            Ok(0)
        }
        Cmd::Backlog { action: BacklogCmd::Task { issues, board, planned, .. } } => {
            let v = backlog_bulk(c, "task", &issues, json!({"where": if board { "board" } else { "goal" }, "planned": planned}))?;
            let refs = issue_refs(&issues)?;
            for (r, t) in refs.iter().zip(v["tasks"].as_array().cloned().unwrap_or_default()) {
                let where_ = match (t["status"].as_str(), t["goal"]["ref"].as_str()) {
                    (Some(s), Some(g)) => format!("a {s} task in {g} ({})", t["why"].as_str().unwrap_or(s)),
                    _ => "a queued task".to_string(),
                };
                out(&format!("Made {r} into {}, {where_}.", t["ref"].as_str().unwrap_or("a task")));
                midna_pin(t["ref"].as_str().unwrap_or(""), t["title"].as_str().unwrap_or(""));
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
        Cmd::Backlog { action: BacklogCmd::Tidy { on, off, every, now, reset } } => {
            let mut b = json!({});
            if on || off {
                b["on"] = json!(on);
            }
            if let Some(e) = every {
                b["every_hours"] = json!(tidy_hours(&e)?);
            }
            if now {
                b["now"] = json!(true);
            }
            if reset {
                b["reset"] = json!(true);
            }
            let v = if b.as_object().is_some_and(|o| o.is_empty()) { c.call("GET", "/backlog/tidy", None)? } else { c.call("POST", "/backlog/tidy", Some(b))? };
            out(v["line"].as_str().unwrap_or(""));
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
            let v = c.call("POST", "/alerts", Some(json!({"text": text, "urgent": urgent, "key": key, "task": task, "goal": goal, "session": c.session})))?;
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
        Cmd::Attach { url, kind, title, summary, goal, t } => {
            let goal = goal.map(|g| goal_ref(&g)).transpose()?;
            let url = attach_target(&url, &c.cwd)?;
            let needs = goal.is_none();
            c.run_report("tb.attach", json!({"url": url, "kind": kind, "title": title, "summary": summary, "goal": goal}), t.task, needs, |v| {
                format!("Attached {} to {}.", v["attachment"]["title"].as_str().unwrap_or("it"), v["goal"].as_str().or(v["task"].as_str()).unwrap_or("the task"))
            })
        }
        Cmd::Hours { on, off, start, end, days, until, for_, alert_every } => {
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
            if let Some(x) = until {
                b["until"] = json!(x);
            }
            if let Some(x) = for_ {
                b["for"] = json!(x);
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
        Cmd::Terminals { max } => {
            let v = match max {
                Some(m) => c.call("POST", "/terminals", Some(json!({"max_terminals": m})))?,
                None => c.call("GET", "/terminals", None)?,
            };
            out(v["line"].as_str().unwrap_or(""));
            Ok(0)
        }
        Cmd::StatusBar { goal, title, reset } => {
            let mut b = json!({});
            for (k, v) in [("goal", goal), ("title", title)] {
                if let Some(v) = v {
                    b[k] = json!(v == "on");
                }
            }
            if reset {
                b["reset"] = json!(true);
            }
            let v = if b.as_object().is_some_and(|o| o.is_empty()) { c.call("GET", "/status-bar", None)? } else { c.call("POST", "/status-bar", Some(b))? };
            out(v["line"].as_str().unwrap_or(""));
            Ok(0)
        }
        Cmd::Limits { compact_window, cold_idle_mins, warm_tokens, warm_idle_mins, max_compactions, generated, project, review_gap, step, reset } => {
            let mut b = json!({});
            for (k, v) in [("compact_window", compact_window), ("warm_tokens", warm_tokens)] {
                if let Some(n) = v {
                    b[k] = json!(n);
                }
            }
            for (k, v) in [("cold_idle_mins", cold_idle_mins), ("warm_idle_mins", warm_idle_mins), ("max_compactions", max_compactions)] {
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
            if let Some(n) = review_gap {
                if !(0..=taskboardd::limits::MAX_REVIEW_GAP).contains(&n) {
                    return Err(format!("--review-gap takes 0 to {} minutes (0 = no gap).", taskboardd::limits::MAX_REVIEW_GAP));
                }
                b["review_gap"] = json!(n);
                if let Some(s) = step {
                    b["step"] = json!(s);
                }
            } else if step.is_some() {
                return Err("--step goes with --review-gap".to_string());
            }
            if reset {
                b["reset"] = json!(true);
            }
            let v = if b.as_object().map(|o| o.is_empty()).unwrap_or(true) { c.call("GET", "/limits", None)? } else { c.call("POST", "/limits", Some(b))? };
            out(v["line"].as_str().unwrap_or(""));
            Ok(0)
        }
        Cmd::Pr { action } => match action {
            PrCmd::Check => {
                let v = c.call("POST", "/prs/refresh", Some(json!({})))?;
                out(&pr_check_line(&v));
                Ok(0)
            }
            PrCmd::Status { task } => {
                let t = c.pr_task(task)?;
                let v = c.call_progress("GET", &format!("/tasks/{t}/pr?full=1"), None)?;
                print_pr_status(&t, &v);
                Ok(0)
            }
            PrCmd::Reply { args, resolve } => {
                let (task, rest) = task_and(args, 2);
                let t = c.pr_task(task)?;
                let v = c.call_progress("POST", &format!("/tasks/{t}/pr/reply"), Some(json!({"thread": rest[0], "text": rest[1], "resolve": resolve})))?;
                out(&format!("Replied on thread {}{}. {} open.", rest[0], if v["resolved"] == true { " and resolved it" } else { "" }, plural_threads(&v)));
                Ok(0)
            }
            PrCmd::Ack { args } => {
                let (task, rest) = task_and(args, 1);
                let t = c.pr_task(task)?;
                let v = c.call_progress("POST", &format!("/tasks/{t}/pr/ack"), Some(json!({"thread": rest[0]})))?;
                out(&format!("Thread {} is answered without a reply{}. {} open.", rest[0], if v["resolved"] == true { " and resolved" } else { "" }, plural_threads(&v)));
                Ok(0)
            }
            PrCmd::Thread { args } => {
                let (task, rest) = task_and(args, 1);
                let t = c.pr_task(task)?;
                let v = c.call_progress("POST", &format!("/tasks/{t}/pr/thread"), Some(json!({"thread": rest[0]})))?;
                print_pr_thread(&v);
                Ok(0)
            }
            PrCmd::Addressed { task } => {
                let t = c.pr_task(task)?;
                let v = c.call_progress("POST", &format!("/tasks/{t}/pr/addressed"), Some(json!({})))?;
                let asked: Vec<&str> = v["asked"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
                let status = c.call("GET", &format!("/tasks/{t}"), None).ok().and_then(|d| d["status"].as_str().map(str::to_string)).unwrap_or_default();
                out(&addressed_line(&t, &asked, &status));
                Ok(0)
            }
            PrCmd::Reviewers { task, ask, replace, with, drop, count, dry_run } => {
                let t = c.pr_task(task)?;
                let v = c.call_progress("POST", &format!("/tasks/{t}/pr/reviewers"), Some(json!({"ask": ask, "replace": replace, "with": with, "drop": drop, "count": count, "dry_run": dry_run})))?;
                if dry_run {
                    out(&dry_run_line(&v));
                    return Ok(0);
                }
                out(&pr_reviewers_line(&t, &v));
                Ok(0)
            }
            PrCmd::Merge { task } => {
                let t = c.pr_task(task)?;
                c.call_progress("POST", &format!("/tasks/{t}/pr/merge"), Some(json!({"agent": !c.session.is_empty(), "via_session": c.session})))?;
                out(&format!("Merged {t}'s PR and deleted its branch. You can stop here."));
                Ok(0)
            }
            PrCmd::Close { task, reason } => {
                let t = c.pr_task(task)?;
                c.call_progress("POST", &format!("/tasks/{t}/pr/close"), Some(json!({"reason": reason.unwrap_or_default(), "agent": !c.session.is_empty(), "via_session": c.session})))?;
                out(&format!("Closed {t}'s PR without merging it; its branch stays. You can stop here."));
                Ok(0)
            }
            PrCmd::Reviewed { task } => {
                let t = c.pr_task(task)?;
                let v = c.call("POST", &format!("/tasks/{t}/pr/reviewed"), Some(json!({"agent": !c.session.is_empty(), "via_session": c.session})))?;
                let stage = v["pr"]["stage"]["label"].as_str().filter(|l| !l.is_empty());
                out(&match stage {
                    Some(l) => format!("Marked {t}'s PR reviewed. Stage: {l}."),
                    None => format!("Marked {t}'s PR reviewed."),
                });
                Ok(0)
            }
            PrCmd::NotOurs { task, reason, title, proof, check } => {
                let t = c.pr_task(task)?;
                let v = c.call_progress("POST", &format!("/tasks/{t}/pr/not-ours"), Some(json!({"reason": reason, "title": title, "proof": proof, "checks": check})))?;
                let cleared: Vec<&str> = v["cleared"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
                out(&format!("Cleared {} for this push of {t}'s PR: they failed, but not because of it.", cleared.join(", ")));
                Ok(0)
            }
            PrCmd::NeedsOwner { task, reason, check } => {
                let t = c.pr_task(task)?;
                c.call_progress("POST", &format!("/tasks/{t}/pr/needs-owner"), Some(json!({"reason": reason, "checks": check})))?;
                out(&format!("Told the owner {t}'s PR needs them. The board watches it and brings this conversation back when it needs you. You can stop here."));
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
                c.call("POST", &format!("/tasks/{t}/pr/skip-checks"), Some(json!({"reason": reason, "all": all, "who": who})))?;
                out(&format!("{t}'s PR checks that didn't fail count as passed{}.", if all { " on every push" } else { " for this push" }));
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
                c.call_progress("POST", &format!("/tasks/{t}/pr/merged"), Some(json!({})))?;
                out(&format!("Recorded {t}'s PR as merged."));
                Ok(0)
            }
        },
        Cmd::Hooks { action } => hooks_cmd(c, action.unwrap_or(HooksCmd::List)),
        Cmd::Words { action: WordsCmd::Misses { limit } } => {
            let v = c.call("GET", &format!("/start-misses?limit={limit}"), None)?;
            for line in misses_lines(&v["misses"], &chrono::Local::now()) {
                out(&line);
            }
            Ok(0)
        }
        Cmd::Hello => {
            let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "tb".into());
            match c.report("tb.hello", json!({"tb_path": exe, "plugin_version": env!("CARGO_PKG_VERSION")}), None, TB_TIMEOUT)? {
                None => out(SAVED),
                Some(_) => out(&format!("The board knows tb is at {exe}.")),
            }
            Ok(0)
        }
        Cmd::CiToken { action } => {
            let v = match action {
                None | Some(CiTokenCmd::Show) => c.call("GET", "/ci-token", None)?,
                Some(CiTokenCmd::Set { token }) => {
                    let token = match token {
                        Some(t) => t,
                        None => {
                            let mut s = String::new();
                            std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).map_err(|e| format!("couldn't read the token from stdin: {e}"))?;
                            s.trim().to_string()
                        }
                    };
                    c.call("POST", "/ci-token", Some(json!({"token": token})))?
                }
                Some(CiTokenCmd::Clear) => c.call("POST", "/ci-token/clear", None)?,
            };
            out(&match v["source"].as_str() {
                Some("board") => "CI token: stored on the board (Keychain).".to_string(),
                Some(_) => format!("CI token: from ${} in the daemon's environment.", v["env"].as_str().unwrap_or("")),
                None => format!("No CI token: store one with tb ci-token set (or set ${} for the daemon).", v["env"].as_str().unwrap_or("")),
            });
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
    let r = &p["pr_rules"];
    let approvals = match r["approvals"].as_i64() {
        Some(0) => " · approvals: the host's decision".to_string(),
        Some(n) => format!(" · {n} approval{}", if n == 1 { "" } else { "s" }),
        None => String::new(),
    };
    let expected = match r["expected"].as_array() {
        Some(l) if l.is_empty() => " · expects no checks".to_string(),
        Some(l) => format!(
            " · expects {} (waits {} min)",
            l.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "),
            r["expected_wait_mins"].as_f64().map(|m| m.to_string()).unwrap_or_default()
        ),
        None => String::new(),
    };
    let switches: String = [("ask_stage", "ask stage"), ("swap", "swaps"), ("review", "review"), ("agents_merge", "agents merge"), ("owner_review", "owner review")]
        .iter()
        .filter_map(|(k, label)| r[*k].as_bool().map(|on| format!(" · {label} {}", if on { "on" } else { "off" })))
        .collect();
    let terminals = p["max_terminals"].as_i64().map(|n| format!(" · up to {n} busy terminals")).unwrap_or_default();
    let slots = if p["worktrees"]["set"].as_object().is_some_and(|s| !s.is_empty()) {
        let w = &p["worktrees"];
        format!(
            " · {}–{} worktree slots, idle ones cleared after {} days and removed after {} (sooner under {} GB free)",
            w["min_slots"], w["max_slots"], w["idle_trim_days"], w["idle_remove_days"], w["low_disk_gb"]
        )
    } else {
        String::new()
    };
    let tags = p["release_tags"].as_str().map(|t| format!(", cut by tags {t}")).unwrap_or_default()
        + &p["release_script"].as_str().map(|s| format!(", release script {s}")).unwrap_or_default();
    let releases = match p["releases"].as_bool() {
        Some(on) => format!(" · releases {}{}", if on { "on" } else { "off" }, if on { tags.as_str() } else { "" }),
        None => String::new(),
    };
    let lens = p["lens"].as_bool().map(|on| format!(" · lens sweeps {}", if on { "on" } else { "off" })).unwrap_or_default();
    let sweep = &p["lens_sweep"];
    let sweep = match sweep["state"].as_str() {
        Some(state) => {
            let said = sweep["error"].as_str().or(sweep["said"].as_str()).map(|s| format!(": {s}")).unwrap_or_default();
            let links: String = [("PR", "pr"), ("digest", "digest")]
                .iter()
                .filter_map(|(label, k)| sweep[*k].as_str().map(|v| format!("\n    Last sweep {label}: {v}")))
                .collect();
            format!("\n  lens sweep {state}{said}{links}")
        }
        None => String::new(),
    };
    let waits_on: Vec<&str> = p["waits_on"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    let waits_on = if waits_on.is_empty() { String::new() } else { format!(" · its tasks may wait for {}'s", waits_on.join(", ")) };
    let gaps: String = p["setup_gaps"]
        .as_array()
        .map(|a| a.iter().map(|g| format!("\n  Needs setup: {}. To fix it: {}", g["text"].as_str().unwrap_or(""), g["fix"].as_str().unwrap_or(""))).collect())
        .unwrap_or_default();
    let stray: String = p["stray_worktrees"]
        .as_array()
        .map(|a| a.iter().filter_map(|w| w.as_str()).map(|w| format!("\n  Worktree no task claims (left for you): {w}")).collect())
        .unwrap_or_default();
    format!(
        "{} · PR flow {} · {} · {remote}{terminals}{slots}{releases}{lens}{waits_on}{approvals}{expected}{switches}{gaps}{sweep}{stray}",
        p["name"].as_str().unwrap_or(""),
        p["pr_flow"].as_str().unwrap_or("auto"),
        if p["ships_prs"] == true { "work ends in PRs" } else { "no PRs" }
    )
}

/// `tb project set`'s PR-rule flags as the API's body: `default` sends null (back to config.toml's).
fn project_rules(approvals: Option<String>, expected: Vec<String>, wait: Option<String>) -> Result<serde_json::Map<String, Value>, String> {
    let mut b = serde_json::Map::new();
    let is_default = |s: &str| s.trim().eq_ignore_ascii_case("default");
    if let Some(a) = approvals {
        let v = if is_default(&a) { Value::Null } else { json!(a.trim().parse::<i64>().map_err(|_| format!("--approvals takes a count or default, not “{a}”"))?) };
        b.insert("approvals".into(), v);
    }
    if !expected.is_empty() {
        let v = match expected.as_slice() {
            [one] if is_default(one) => Value::Null,
            [one] if one.trim().eq_ignore_ascii_case("none") => json!([]),
            list => json!(list),
        };
        b.insert("expected".into(), v);
    }
    if let Some(w) = wait {
        let v = if is_default(&w) { Value::Null } else { json!(w.trim().parse::<f64>().map_err(|_| format!("--expected-wait takes minutes or default, not “{w}”"))?) };
        b.insert("expected_wait_mins".into(), v);
    }
    Ok(b)
}

/// An on/off PR rule from `tb project set` (`default` sends null: back to config.toml's).
fn project_switch(b: &mut serde_json::Map<String, Value>, key: &str, v: Option<String>) {
    if let Some(v) = v {
        b.insert(key.into(), match v.as_str() {
            "on" => json!(true),
            "off" => json!(false),
            _ => Value::Null,
        });
    }
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
                for a in p["areas"].as_array().into_iter().flatten() {
                    let paths: Vec<&str> = a["paths"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect();
                    out(&format!("  Area {}: {}", a["name"].as_str().unwrap_or(""), paths.join(", ")));
                }
                for x in p["area_problems"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
                    out(&format!("  .taskboard.toml: {x}"));
                }
            }
            Ok(0)
        }
        ProjectCmd::Set { name, pr_flow, approvals, expected_check, expected_wait, ask_stage, swap, review, agents_merge, owner_review, max_terminals, min_slots, max_slots, idle_trim_days, idle_remove_days, low_disk_gb, waits_on, releases, lens, release_tags, release_script } => {
            let mut body = project_rules(approvals, expected_check, expected_wait)?;
            for (key, flag, v) in [
                ("min_slots", "--min-slots", min_slots),
                ("max_slots", "--max-slots", max_slots),
                ("idle_trim_days", "--idle-trim-days", idle_trim_days),
                ("idle_remove_days", "--idle-remove-days", idle_remove_days),
                ("low_disk_gb", "--low-disk-gb", low_disk_gb),
            ] {
                let Some(v) = v else { continue };
                let v = if v.trim().eq_ignore_ascii_case("default") {
                    Value::Null
                } else {
                    json!(v.trim().parse::<i64>().map_err(|_| format!("{flag} takes a number or default, not “{v}”"))?)
                };
                body.insert(key.into(), v);
            }
            if let Some(w) = waits_on {
                body.insert("waits_on".into(), json!(w));
            }
            if let Some(m) = max_terminals {
                let v = if m.trim().eq_ignore_ascii_case("default") {
                    Value::Null
                } else {
                    json!(m.trim().parse::<i64>().map_err(|_| format!("--max-terminals takes a count or default, not “{m}”"))?)
                };
                body.insert("max_terminals".into(), v);
            }
            project_switch(&mut body, "ask_stage", ask_stage);
            project_switch(&mut body, "swap", swap);
            project_switch(&mut body, "review", review);
            project_switch(&mut body, "agents_merge", agents_merge);
            project_switch(&mut body, "owner_review", owner_review);
            project_switch(&mut body, "releases", releases);
            project_switch(&mut body, "lens", lens);
            if let Some(t) = release_tags {
                body.insert("release_tags".into(), json!(t));
            }
            if let Some(s) = release_script {
                body.insert("release_script".into(), json!(s));
            }
            if let Some(flow) = pr_flow {
                body.insert("pr_flow".into(), json!(flow));
            }
            if body.is_empty() {
                return Err(format!("say what to change, for example: tb project set {name} --pr-flow off, --approvals 2 or --ask-stage on"));
            }
            let v = c.call("POST", &format!("/projects/{name}"), Some(Value::Object(body)))?;
            out(&format!("Changed {}.", project_line(&v)));
            Ok(0)
        }
        ProjectCmd::AgentsMerge { value } => {
            let v = match value {
                None => c.call("GET", "/projects/agents-merge", None)?,
                Some(s) => {
                    let mut body = serde_json::Map::new();
                    project_switch(&mut body, "agents_merge", Some(s));
                    c.call("POST", "/projects/agents-merge", Some(Value::Object(body)))?
                }
            };
            let on = if v["agents_merge"] == true { "on" } else { "off" };
            let from = if v["set"].is_boolean() { "set on the board" } else { "config.toml's pr.agents_merge" };
            out(&format!("Agents merge: {on} for projects that don't say ({from})."));
            Ok(0)
        }
        ProjectCmd::OwnerReview { value } => {
            let v = match value {
                None => c.call("GET", "/projects/owner-review", None)?,
                Some(s) => {
                    let mut body = serde_json::Map::new();
                    project_switch(&mut body, "owner_review", Some(s));
                    c.call("POST", "/projects/owner-review", Some(Value::Object(body)))?
                }
            };
            let on = if v["owner_review"] == true { "on" } else { "off: the board marks green PRs reviewed" };
            let from = if v["set"].is_boolean() { "set on the board" } else { "config.toml's pr.owner_review" };
            out(&format!("Owner review: {on} for projects that don't say ({from})."));
            Ok(0)
        }
        ProjectCmd::Lens { value } => {
            let v = match value {
                None => c.call("GET", "/projects/lens", None)?,
                Some(s) => {
                    let mut body = serde_json::Map::new();
                    project_switch(&mut body, "lens", Some(s));
                    c.call("POST", "/projects/lens", Some(Value::Object(body)))?
                }
            };
            let on = if v["lens"] == true { "on" } else { "off: the board doesn't run lens sweep" };
            let from = if v["set"].is_boolean() { "set on the board" } else { "config.toml's [lens] on" };
            let installed = if v["installed"] == true { "" } else { " lens isn't installed, so it doesn't run anywhere yet." };
            let gate = if v["submit"] == true { " tb submit also needs a lens review ([lens] submit)." } else { "" };
            out(&format!("Lens sweeps: {on} for projects that don't say ({from}).{installed}{gate}"));
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

/// "The PR feed is healthy · last event 3:04 PM" for `tb feed`.
fn feed_line(v: &Value) -> String {
    if v["on"] != true {
        return "The PR feed is off ([feed] on in config.toml); PRs are polled.".into();
    }
    let mut line = if v["healthy"] == true {
        "The PR feed is healthy".to_string()
    } else {
        format!("The PR feed is {}: {}", v["problem"].as_str().unwrap_or("unhealthy"), v["why"].as_str().unwrap_or(""))
    };
    for (k, label) in [("last_event_at", "last event"), ("last_heartbeat_at", "last heartbeat")] {
        if let Some(t) = v[k].as_str() {
            line += &format!(" · {label} {}", taskboardd::util::local_clock(Some(t)));
        }
    }
    let restarts = v["restarts"].as_array().map(|a| a.len()).unwrap_or(0);
    if restarts > 0 {
        line += &format!(" · restarted {restarts}×");
    }
    if v["active"] == false && v["off_hours"].is_string() {
        line += &match v["idle_until"].as_str() {
            Some(t) => format!(" · listener idle until {}", taskboardd::util::local_clock(Some(t))),
            None => " · listener idle".to_string(),
        };
    }
    if let Some(h) = v["holding"].as_str() {
        line += &format!("\n{h}");
    }
    line
}

/// `GET /prs/feed`'s `rate_limits`: one line per account the board has called, saying whether the
/// host has it rate-limited and how many calls the board made on it in the last hour.
fn rate_lines(v: &Value) -> Vec<String> {
    let mut lines = vec![];
    for a in v.as_array().into_iter().flatten() {
        let label = a["label"].as_str().unwrap_or("");
        let mut line = a["text"].as_str().map(str::to_string).unwrap_or_else(|| format!("{label} isn't rate-limited"));
        let calls = a["calls_last_hour"].as_u64().unwrap_or(0);
        line += &match a["max_per_hour"].as_u64() {
            Some(b) => format!(" · the board's calls in the last hour: {calls} of {b}"),
            None => format!(" · the board's calls in the last hour: {calls}"),
        };
        if let Some(f) = a["stretch"].as_f64() {
            line += &format!(" · polling {f:.1}× slower to fit");
        }
        if let Some(why) = a["why"].as_str() {
            line += &format!("\n  {why}");
        }
        lines.push(line);
    }
    lines
}

/// One line per source: its mode (and where it's set), its stream's health and whether its poll runs,
/// then a line per relay under it.
fn stream_lines(v: &Value) -> String {
    let mut lines = vec![];
    for s in v["sources"].as_array().into_iter().flatten() {
        let mut line = format!("{:<7} {}", s["source"].as_str().unwrap_or("?"), s["mode"].as_str().unwrap_or("board"));
        match s["mode_from"].as_str() {
            Some("override") => line += " (tb stream mode)",
            Some("config") => line += " (config.toml)",
            _ => {}
        }
        if s["watched"] == true {
            line += &match s["problem"].as_str() {
                Some(p) => format!(" · {p}: {}", s["why"].as_str().unwrap_or("")),
                None => " · healthy".to_string(),
            };
            for (k, label) in [("last_event_at", "last event"), ("last_heartbeat_at", "last heartbeat")] {
                if let Some(t) = s[k].as_str() {
                    line += &format!(" · {label} {}", taskboardd::util::local_clock(Some(t)));
                }
            }
        }
        line += match (s["polling"] == true, s["polling_some"] == true) {
            (true, true) => " · polling some PRs",
            (true, false) => " · polling",
            _ => " · poll resting",
        };
        lines.push(line);
        let relays = s["relays"].as_array().cloned().unwrap_or_default();
        let width = relays.iter().filter_map(|r| r["relay"].as_str()).map(|n| n.chars().count()).max().unwrap_or(0).max(6);
        for r in &relays {
            lines.push(format!("  {:<width$} {}", r["relay"].as_str().unwrap_or("?"), relay_line(r)));
        }
    }
    lines.join("\n")
}

/// One relay's health, idle state, last event and heartbeat, and what it covers.
fn relay_line(r: &Value) -> String {
    let mut parts = vec![match r["problem"].as_str() {
        Some(p) => format!("{p}: {}", r["why"].as_str().unwrap_or("")),
        None => "healthy".to_string(),
    }];
    if r["idle"] == true {
        parts.push(match r["idle_until"].as_str() {
            Some(t) => format!("idle until {}", taskboardd::util::local_clock(Some(t))),
            None => "idle".to_string(),
        });
    }
    for (k, label) in [("last_event_at", "last event"), ("last_heartbeat_at", "last heartbeat")] {
        if let Some(t) = r[k].as_str() {
            parts.push(format!("{label} {}", taskboardd::util::local_clock(Some(t))));
        }
    }
    parts.push(match &r["covers"] {
        Value::String(s) if s == "all" => "covers every PR".to_string(),
        Value::Array(a) if a.is_empty() => "no heartbeats, covers nothing".to_string(),
        Value::Array(a) => {
            let each: Vec<String> = a.iter().map(|c| format!("{} {}", c["provider"].as_str().unwrap_or("?"), c["repo"].as_str().unwrap_or("every repo")).trim().to_string()).collect();
            format!("covers {}", each.join(", "))
        }
        _ => "covers nothing".to_string(),
    });
    parts.join(" · ")
}

fn stream_send(c: &Ctx, json: Option<String>) -> Result<i32, String> {
    let text = match json.as_deref() {
        None | Some("-") => read_body("-")?,
        Some(j) => j.to_string(),
    };
    let body: Value = serde_json::from_str(&text).map_err(|e| format!("That isn't JSON: {e}"))?;
    let v = c.call("POST", "/stream/events", Some(body))?;
    for r in v["results"].as_array().into_iter().flatten() {
        let id = r["id"].as_str().unwrap_or("(no id)");
        match r["why"].as_str() {
            Some(why) => out(&format!("{id}: rejected: {why}")),
            None => out(&format!("{id}: {}", r["status"].as_str().unwrap_or("?"))),
        }
    }
    Ok(if v["rejected"].as_i64().unwrap_or(0) > 0 { 1 } else { 0 })
}

fn feed_cmd(c: &Ctx, action: Option<FeedCmd>) -> Result<i32, String> {
    match action {
        None => {
            let v = c.call("GET", "/prs/feed", None)?;
            out(&feed_line(&v));
            for line in rate_lines(&v["rate_limits"]) {
                out(&line);
            }
        }
        Some(FeedCmd::Heartbeat { active, idle, idle_until, connected_at }) => {
            let mut body = json!({"idle_until": idle_until, "connected_at": connected_at});
            if active || idle {
                body["active"] = json!(active);
            }
            c.call("POST", "/prs/heartbeat", Some(body))?;
        }
        Some(FeedCmd::Event { url, repo, num, task, kind, state, head, branch, provider, build_url, author, mine, source }) => {
            let task = task.map(|t| task_ref(&t)).transpose()?;
            let mut body = json!({"url": url, "repo": repo, "num": num, "task": task, "kind": kind, "state": state, "head": head, "branch": branch,
                                  "provider": provider, "build_url": build_url, "author": author, "source": source});
            if mine {
                body["mine"] = json!(true);
            }
            let v = c.call("POST", "/prs/event", Some(body))?;
            match v["task"].as_str() {
                Some(t) if v["refreshed"] == true => out(&format!("Read {t}'s PR again: {}.", v["phase"].as_str().unwrap_or("no stage"))),
                Some(t) => out(&format!("Noted for {t}.{}", v["read_error"].as_str().map(|e| format!(" Its PR couldn't be read: {e}")).unwrap_or_default())),
                None => out("Noted; no task on the board has that PR."),
            }
        }
    }
    Ok(0)
}

/// "PR builds are stopped (by Sam at 3:04 PM: CI minutes ran out)" for `tb pr-builds`.
fn pr_builds_line(v: &Value) -> String {
    let clock = |k: &str| v[k].as_str().map(|t| format!(" at {}", taskboardd::util::local_clock(Some(t)))).unwrap_or_default();
    if v["stopped"] != true {
        return match v["resumed_by"].as_str() {
            Some(w) => format!("PR builds run (resumed by {w}{}).", clock("resumed_at")),
            None => "PR builds run.".into(),
        };
    }
    let reason = v["reason"].as_str().map(|r| format!(": {r}")).unwrap_or_default();
    let mut line = format!("PR builds are stopped (by {}{}{reason}).", v["by"].as_str().unwrap_or("the owner"), clock("at"));
    if let Some(n) = v["cancelling"].as_i64().filter(|n| *n > 0) {
        line += &format!(" Cancelling the builds of {n} more.");
    }
    line
}

fn pr_builds_cmd(c: &Ctx, action: Option<PrBuildsCmd>) -> Result<i32, String> {
    let v = match action {
        None => c.call("GET", "/pr-builds", None)?,
        Some(PrBuildsCmd::Stop { reason, who }) => c.call("POST", "/pr-builds", Some(json!({"stopped": true, "reason": reason, "who": who})))?,
        Some(PrBuildsCmd::Resume { who }) => c.call("POST", "/pr-builds", Some(json!({"stopped": false, "who": who})))?,
    };
    out(&pr_builds_line(&v));
    let recent = v["recent"].as_array().cloned().unwrap_or_default();
    if !recent.is_empty() {
        out("Recent cancels:");
        for r in &recent {
            out(&pr_builds_recent_line(r));
        }
    }
    Ok(0)
}

/// "  3:04pm  PR #9 (T12) at 1a2b3c4d: stopped 2 builds" for one of `tb pr-builds`' recent cancels.
fn pr_builds_recent_line(r: &Value) -> String {
    let when = taskboardd::util::local_clock(r["at"].as_str());
    let what = match (r["num"].as_i64().filter(|n| *n > 0), r["task"].as_str()) {
        (Some(n), Some(t)) => format!("PR #{n} ({t})"),
        (Some(n), None) => format!("PR #{n}"),
        _ => format!("{} {}", r["repo"].as_str().unwrap_or(""), r["branch"].as_str().unwrap_or("")).trim().to_string(),
    };
    let head: String = r["head"].as_str().unwrap_or("").chars().take(8).collect();
    let at = if head.is_empty() { String::new() } else { format!(" at {head}") };
    let again = if r["follow_up"] == true { " (follow-up)" } else { "" };
    let how = match (r["ok"] == true, r["build"].as_str()) {
        (true, Some(b)) => format!("stopped {b}"),
        (true, None) => r["what"].as_str().unwrap_or("").to_string(),
        (false, _) => format!("gave up: {}", r["what"].as_str().unwrap_or("")),
    };
    format!("  {when}  {what}{at}{again}: {how}")
}

fn break_ref(v: &str) -> Option<String> {
    let s = v.trim();
    let digits = s.strip_prefix(['M', 'm']).unwrap_or(s);
    (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())).then(|| format!("M{digits}"))
}

/// "M3 · webapp main fails build · yours to fix · T12" for `tb master`.
fn break_line(m: &Value) -> String {
    let checks: Vec<&str> = m["checks"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    let mut line = format!(
        "{} · {} {} {} {}",
        m["ref"].as_str().unwrap_or(""),
        m["project"].as_str().unwrap_or(""),
        m["branch"].as_str().unwrap_or(""),
        if m["state"] == "open" { "fails" } else { "failed" },
        checks.join(", ")
    );
    if let Some(cause) = m["verdict_title"].as_str() {
        line += &format!(" · {cause}");
    }
    line += &format!(" · {}", m["verdict_label"].as_str().unwrap_or("not decided yet"));
    if let Some(t) = m["task"]["ref"].as_str() {
        line += &format!(" · {t}");
    }
    if m["state"] == "closed" {
        line += &format!(" · green again {}", taskboardd::util::local_clock(m["closed_at"].as_str()));
    }
    line
}

fn break_detail(m: &Value) -> String {
    let mut lines = vec![break_line(m)];
    if let Some(w) = m["verdict_why"].as_str() {
        lines.push(format!("Why: {w} ({})", m["verdict_by"].as_str().unwrap_or("")));
    }
    for u in m["proof"].as_array().cloned().unwrap_or_default() {
        lines.push(format!("Proof: {}", u.as_str().unwrap_or("")));
    }
    for c in m["evidence"]["checks"].as_array().cloned().unwrap_or_default() {
        let mut l = format!("  ✗ {}", c["name"].as_str().unwrap_or(""));
        if let Some(u) = c["url"].as_str() {
            l += &format!(" {u}");
        }
        lines.push(l);
        for (k, label) in [("steps", "step"), ("tests", "test")] {
            for x in c[k].as_array().cloned().unwrap_or_default() {
                lines.push(format!("      {label}: {}", x.as_str().unwrap_or("")));
            }
        }
    }
    lines.push("Suspects:".into());
    for s in m["suspects"].as_array().cloned().unwrap_or_default() {
        lines.push(format!(
            "  {} {} <{}>{} {}",
            s["sha"].as_str().unwrap_or("").chars().take(10).collect::<String>(),
            s["name"].as_str().unwrap_or(""),
            s["email"].as_str().unwrap_or(""),
            if s["ours"] == true { " (yours)" } else { "" },
            s["message"].as_str().unwrap_or("")
        ));
    }
    lines.join("\n")
}

/// What `tb master M3 ours|not-ours|unsure` sends with the verdict.
struct Verdict {
    why: Option<String>,
    who: Option<String>,
    proof: Vec<String>,
    title: Option<String>,
}

/// A verdict says why in 20 to 300 characters, and a not-ours names its cause in at most 80, as the board
/// checks: refused here, nothing is sent.
fn verdict_check(verdict: &str, v: &Verdict) -> Result<(), String> {
    let why = v.why.as_deref().map(|w| w.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default();
    let n = why.chars().count();
    if !(20..=300).contains(&n) {
        return Err(format!("say why in 20 to 300 characters with --why: what broke it and how you checked (that's {n})"));
    }
    let title = v.title.as_deref().map(str::trim).unwrap_or("");
    if title.chars().count() > TITLE_MAX {
        return Err(format!("the --title is {} characters; name the cause in {TITLE_MAX} or fewer", title.chars().count()));
    }
    if matches!(verdict, "not-ours" | "not_ours") && title.is_empty() {
        return Err(format!("not-ours needs --title: the cause in a few plain words, {TITLE_MAX} characters at most"));
    }
    Ok(())
}

fn master_cmd(c: &Ctx, args: Vec<String>, verdict: Verdict, open_only: bool, limit: i64) -> Result<i32, String> {
    let (refs, words): (Vec<String>, Vec<String>) = args.into_iter().partition(|a| break_ref(a).is_some());
    let r = refs.first().and_then(|r| break_ref(r));
    let word = words.first().map(|w| w.to_lowercase());
    match (r, word.as_deref()) {
        (None, None) => {
            let v = c.call("GET", &format!("/master?limit={limit}{}", if open_only { "&open=1" } else { "" }), None)?;
            let open = v["open"].as_array().cloned().unwrap_or_default();
            if open.is_empty() {
                out("No default branch is red.");
            }
            for m in open.iter().chain(v["closed"].as_array().into_iter().flatten()) {
                out(&break_line(m));
            }
            for w in v["watched"].as_array().cloned().unwrap_or_default() {
                if let Some(e) = w["error"].as_str() {
                    out(&format!("{} {}: couldn't read it: {e}", w["project"].as_str().unwrap_or(""), w["branch"].as_str().unwrap_or("")));
                }
            }
        }
        (None, Some("check")) => {
            let v = c.call("POST", "/master/check", Some(json!({})))?;
            out(&format!("Read every watched branch: {} red.", v["open"].as_array().map(|a| a.len()).unwrap_or(0)));
        }
        (Some(r), None | Some("show")) => out(&break_detail(&c.call("GET", &format!("/master/{r}"), None)?)),
        (Some(r), Some(v @ ("ours" | "not-ours" | "not_ours" | "unsure"))) => {
            verdict_check(v, &verdict)?;
            let Verdict { why, who, proof, title } = verdict;
            let m = c.call("POST", &format!("/master/{r}"), Some(json!({"verdict": v, "why": why, "who": who, "proof": proof, "title": title})))?;
            out(&break_line(&m));
        }
        (None, Some("show" | "ours" | "not-ours" | "not_ours" | "unsure")) => return Err("name the break, for example: tb master M3 ours".into()),
        (_, Some(w)) => return Err(format!("{w} isn't a master command: show, ours, not-ours, unsure or check")),
    }
    Ok(0)
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
    fn a_round_aimed_at_another_repository_is_refused() {
        let root = std::env::temp_dir().join(format!("tb-aim-outside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (mine, other) = (root.join("mine"), root.join("other"));
        for d in [&mine, &other] {
            std::fs::create_dir_all(d).unwrap();
            assert!(std::process::Command::new("git").arg("-C").arg(d).args(["init", "-q"]).status().unwrap().success());
        }
        let v = json!({"task": "T9", "vars": {"repo": mine.to_string_lossy(), "project": "web"}});
        assert_eq!(outside_repo(&mine.join(".").to_string_lossy(), &v), None, "a checkout of the task's repository");
        let why = outside_repo(&other.to_string_lossy(), &v).expect("another repository");
        assert!(why.contains("web's repository") && why.contains("T9's steps") && why.contains("tb step aim --clear"), "{why}");
        assert_eq!(outside_repo(&other.to_string_lossy(), &json!({"vars": {}})), None, "no repository to compare");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tb_words_misses_lists_each_refused_start_with_its_prompt() {
        assert!(matches!(Cli::try_parse_from(["tb", "words", "misses", "--limit", "5"]).unwrap().cmd, Cmd::Words { action: WordsCmd::Misses { limit: 5 } }));
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-12T15:00:00Z").unwrap();
        let misses = json!([
            {"at": "2026-10-12T13:30:00Z", "ref": "T8", "session_id": "s1", "session_name": "Term", "prompt": "make it\nand start it"},
            {"at": "2026-10-11T09:05:00Z", "ref": "G2", "session_id": "s2", "session_name": null, "prompt": null},
        ]);
        assert_eq!(
            misses_lines(&misses, &now),
            ["1:30 PM · T8 · Term", "  > make it", "  > and start it", "Oct 11, 9:05 AM · G2 · s2", "  (no prompt typed in this conversation)"]
        );
        assert_eq!(misses_lines(&json!([]), &now), ["No start has been refused."]);
    }

    #[test]
    fn tb_pr_needs_owner_takes_a_reason_and_optional_checks() {
        let parse = |args: &[&str]| match Cli::try_parse_from(args).unwrap().cmd {
            Cmd::Pr { action: PrCmd::NeedsOwner { task, reason, check } } => (task, reason, check),
            _ => panic!("not needs-owner"),
        };
        assert_eq!(parse(&["tb", "pr", "needs-owner", "--reason", "Add the secret"]), (None, "Add the secret".into(), vec![]));
        assert_eq!(
            parse(&["tb", "pr", "needs-owner", "T5", "--reason", "Add the secret", "--check", "ci", "--check", "lint"]),
            (Some("T5".into()), "Add the secret".into(), vec!["ci".to_string(), "lint".into()])
        );
        assert!(Cli::try_parse_from(["tb", "pr", "needs-owner"]).is_err(), "--reason is required");
    }

    #[test]
    fn model_flags_parse_and_show() {
        let model = |args: &[&str]| match Cli::try_parse_from(args).unwrap().cmd {
            Cmd::Task { action: TaskCmd::New { model, .. } | TaskCmd::Set { model, .. } } => model,
            _ => panic!("{args:?}"),
        };
        assert_eq!(model(&["tb", "task", "new", "Pen", "--model", "small"]).as_deref(), Some("small"));
        assert_eq!(model(&["tb", "task", "set", "T3", "--model", "default"]).as_deref(), Some("default"));
        assert_eq!(model(&["tb", "task", "new", "Pen"]), None);
        assert_eq!(model_line(&json!({"model": "small", "model_runs": "sonnet"})).as_deref(), Some("Model: small (sonnet)"));
        assert_eq!(model_line(&json!({"model": "haiku", "model_runs": "haiku"})).as_deref(), Some("Model: haiku"));
        assert_eq!(model_line(&json!({"model": null, "model_runs": "opus"})).as_deref(), Some("Model: default (opus)"));
        assert_eq!(model_line(&json!({"model": null, "model_runs": null})), None);
        assert_eq!(model_line(&json!({"model": "small", "model_runs": "sonnet", "effort_runs": "medium"})).as_deref(), Some("Model: small (sonnet · medium)"));
        assert_eq!(model_line(&json!({"model": "haiku", "model_runs": "haiku", "effort_runs": "max"})).as_deref(), Some("Model: haiku (max)"));
        assert_eq!(model_line(&json!({"model": null, "model_runs": "opus", "effort_runs": "high"})).as_deref(), Some("Model: default (opus · high)"));
        let effort = |args: &[&str]| match Cli::try_parse_from(args).unwrap().cmd {
            Cmd::Task { action: TaskCmd::New { effort, .. } | TaskCmd::Set { effort, .. } } => effort,
            _ => panic!("not a task command"),
        };
        assert_eq!(effort(&["tb", "task", "new", "Pen", "--effort", "high"]).as_deref(), Some("high"));
        assert_eq!(effort(&["tb", "task", "set", "T3", "--effort", "default"]).as_deref(), Some("default"));
    }

    #[test]
    fn lane_flags_parse_and_show() {
        let lane = |args: &[&str]| match Cli::try_parse_from(args).unwrap().cmd {
            Cmd::Task { action: TaskCmd::New { lane, .. } | TaskCmd::Set { lane, .. } } => lane,
            Cmd::Goal { action: GoalCmd::Set { lane, .. } } => lane,
            _ => panic!("{args:?}"),
        };
        assert_eq!(lane(&["tb", "task", "new", "Pen", "--lane", "slot"]).as_deref(), Some("slot"));
        assert_eq!(lane(&["tb", "task", "set", "T3", "--lane", "auto"]).as_deref(), Some("auto"));
        assert_eq!(lane(&["tb", "goal", "set", "G1", "--lane", "shared"]).as_deref(), Some("shared"));
        assert_eq!(lane(&["tb", "task", "new", "Pen"]), None);
        assert!(Cli::try_parse_from(["tb", "task", "set", "T3", "--lane", "sideways"]).is_err());
        assert_eq!(task_lane_line(&json!({"lane": {"lane": "slot", "from": "goal"}})).as_deref(), Some("Lane: slot (its goal's)"));
        assert_eq!(task_lane_line(&json!({"lane": {"lane": "slot", "from": "auto"}})).as_deref(), Some("Lane: slot (the board's pick)"));
        assert_eq!(task_lane_line(&json!({"lane": {"lane": "shared", "from": "task"}})).as_deref(), Some("Lane: shared"));
        assert_eq!(task_lane_line(&json!({"lane": {"lane": "shared", "from": "auto"}})), None);
        assert_eq!(lane_text("auto"), None);
    }

    #[test]
    fn planning_names_the_models_while_they_are_on() {
        let v = json!({"small": "sonnet", "large": "opus", "default": "opus"});
        assert_eq!(
            models_line(&v).as_deref(),
            Some("Models (tb task new --model small|large, --effort low|medium|high|xhigh|max to change its effort): small runs sonnet, large runs opus; a task without one runs opus.")
        );
        let v = json!({"small": "sonnet", "large": "opus", "default": "opus", "small_effort": "medium", "large_effort": "high", "default_effort": "high"});
        assert!(models_line(&v).unwrap().ends_with("small runs sonnet at medium, large runs opus at high; a task without one runs opus at high."));
        assert_eq!(models_line(&Value::Null), None);
    }

    #[test]
    fn area_flags_parse_and_show() {
        let area = |args: &[&str]| match Cli::try_parse_from(args).unwrap().cmd {
            Cmd::Task { action: TaskCmd::New { area, .. } | TaskCmd::Set { area, .. } } => area,
            Cmd::Goal { action: GoalCmd::New { area, .. } | GoalCmd::Set { area, .. } } => area,
            Cmd::Backlog { action: BacklogCmd::Add { area, .. } | BacklogCmd::Set { area, .. } } => area,
            _ => panic!("{args:?}"),
        };
        assert_eq!(area(&["tb", "task", "new", "Pen", "--area", "quill"]).as_deref(), Some("quill"));
        assert_eq!(area(&["tb", "task", "set", "T3", "--area", "none"]).as_deref(), Some("none"));
        assert_eq!(area(&["tb", "goal", "new", "Brushes", "--area", "Art"]).as_deref(), Some("Art"));
        assert_eq!(area(&["tb", "goal", "set", "G2", "--area", "Art"]).as_deref(), Some("Art"));
        assert_eq!(area(&["tb", "backlog", "add", "Lag", "--area", "Quill"]).as_deref(), Some("Quill"));
        assert_eq!(area(&["tb", "backlog", "set", "B4", "--area", "none"]).as_deref(), Some("none"));
        assert_eq!(area(&["tb", "task", "new", "Pen"]), None);
        let g = json!({"ref": "G2", "name": "Brushes", "project": "rune", "area": "Art", "done": 0, "total": 2,
                       "tasks": [{"ref": "T3", "title": "Pen", "status": "planned", "area": "Quill"}, {"ref": "T4", "title": "Ink", "status": "planned", "area": "Art"}]});
        let lines = goal_lines(&g);
        assert_eq!(lines[0], "G2 · Brushes · rune · area Art · 0/2 done");
        assert_eq!(done_bit(&json!({"done": 14, "total": 15, "review": 1})), "14/15 done · 1 in review");
        assert!(lines.iter().any(|l| l.contains("T3 [planned] Pen · area Quill")), "{lines:?}");
        assert!(lines.iter().any(|l| l.ends_with("T4 [planned] Ink")), "the goal's own area isn't repeated: {lines:?}");
        assert!(!lines.iter().any(|l| l.starts_with("Jira")), "{lines:?}");
        let off = goal_lines(&json!({"ref": "G2", "name": "Brushes", "project": "rune", "done": 0, "total": 0, "tasks": [], "jira_off": true}));
        assert!(off.iter().any(|l| l.starts_with("Jira: off.")), "{off:?}");
        let accepted = json!({"ref": "G3", "name": "Teardown", "project": "rune", "done": 20, "total": 22, "failed_accepted": 2, "tasks": [],
                              "accepted": {"why": "T188 replaced them", "tasks": ["T36", "T84"]}});
        assert_eq!(done_bit(&accepted), "20/22 done · 2 failed (accepted)");
        let lines = goal_lines(&accepted);
        assert!(lines.iter().any(|l| l == "Finished by the owner without T36, T84: T188 replaced them"), "{lines:?}");
    }

    #[test]
    fn tb_status_with_the_board_down_saves_nothing_and_fails() {
        let data = std::env::temp_dir().join(format!("tb-status-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        std::fs::create_dir_all(&data).unwrap();
        let mut cfg = Config::for_tests(&data);
        cfg.port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let c = Ctx { cfg, session: "s1".into(), claude: String::new(), cwd: String::new() };
        let e = run_cmd(&c, Cli::try_parse_from(["tb", "status"]).unwrap().cmd).unwrap_err();
        assert_eq!(e, "The board isn't reachable.");
        let spooled = std::fs::read_dir(c.cfg.spool_dir()).map(|d| d.count()).unwrap_or(0);
        assert_eq!(spooled, 0, "a status check is never spooled");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn tb_pr_check_reads_the_prs_and_says_what_changed() {
        assert!(matches!(Cli::try_parse_from(["tb", "pr", "check"]).unwrap().cmd, Cmd::Pr { action: PrCmd::Check }));
        assert_eq!(pr_check_line(&json!({"ok": true, "changed": 2})), "Checked the PRs: 2 tasks changed.");
        assert_eq!(pr_check_line(&json!({"ok": true, "changed": 1})), "Checked the PRs: 1 task changed.");
        assert_eq!(pr_check_line(&json!({"ok": true, "changed": 0})), "Checked the PRs: nothing changed.");
    }

    fn board_down_ctx(tag: &str) -> (Ctx, std::path::PathBuf) {
        let data = std::env::temp_dir().join(format!("tb-{tag}-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        std::fs::create_dir_all(&data).unwrap();
        let mut cfg = Config::for_tests(&data);
        cfg.port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        (Ctx { cfg, session: "s1".into(), claude: String::new(), cwd: data.to_string_lossy().to_string() }, data)
    }

    #[test]
    fn tb_submit_and_question_with_the_board_down_are_saved_for_later() {
        let (c, data) = board_down_ctx("done-down");
        assert_eq!(run_cmd(&c, Cli::try_parse_from(["tb", "submit", "Shipped it"]).unwrap().cmd), Ok(0));
        assert_eq!(run_cmd(&c, Cli::try_parse_from(["tb", "question", "Which base?"]).unwrap().cmd), Ok(0));
        assert_eq!(std::fs::read_dir(c.cfg.spool_dir()).map(|d| d.count()).unwrap_or(0), 2, "both go to the spool");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn tb_bit_set_takes_one_change_and_refuses_none() {
        let (c, data) = board_down_ctx("bitset");
        let none = run_cmd(&c, Cli::try_parse_from(["tb", "bit", "set", "live-flag"]).unwrap().cmd).unwrap_err();
        assert!(none.starts_with("say what to change"), "{none}");
        for flag in [&["--note", "x one"][..], &["--local"], &["--backend"], &["--name", "other"], &["--task", "T3"], &["--not-task", "T3"]] {
            let mut args = vec!["tb", "bit", "set", "live-flag"];
            args.extend_from_slice(flag);
            let e = run_cmd(&c, Cli::try_parse_from(args).unwrap().cmd).unwrap_err();
            assert!(!e.starts_with("say what to change"), "{flag:?} alone is a change: {e}");
        }
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn tb_checkpoint_propose_and_attach_refuse_what_does_nothing() {
        assert_eq!(checkpoint_body(vec![], vec![], vec![], vec![]).unwrap_err(), "give at least one --done, --next, --decision or --file");
        assert_eq!(checkpoint_body(vec!["a".into()], vec![], vec![], vec![]).unwrap(), json!({"done": ["a"]}), "lists left out aren't sent");
        assert!(propose_tasks(&[]).unwrap_err().starts_with("name at least one task"));
        assert!(propose_tasks(&["::detail".into()]).unwrap_err().contains("has no title"));
        assert!(propose_tasks(&["  ".into()]).unwrap_err().contains("has no title"));
        propose_tasks(&["Fix it::the detail".into()]).unwrap();

        let dir = std::env::temp_dir().join(format!("tb-attach-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shot.png"), b"png").unwrap();
        let cwd = dir.to_string_lossy().to_string();
        let full = std::fs::canonicalize(dir.join("shot.png")).unwrap().to_string_lossy().to_string();
        assert_eq!(attach_target("shot.png", &cwd).unwrap(), full, "a relative path is made full");
        assert_eq!(attach_target(&full, "/").unwrap(), full);
        assert_eq!(attach_target("https://x.test/a", &cwd).unwrap(), "https://x.test/a");
        assert_eq!(attach_target("/no/such/missing.png", &cwd).unwrap_err(), "no such file: /no/such/missing.png");
        assert_eq!(attach_target("missing.png", &cwd).unwrap_err(), format!("no such file: {cwd}/missing.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tb_close_done_and_backlog_note_parse() {
        let Cmd::Close { done, project, goal } = Cli::try_parse_from(["tb", "close", "--done", "--goal", "G2"]).unwrap().cmd else { panic!("not tb close") };
        assert_eq!((done, project, goal.as_deref()), (true, None, Some("G2")));
        assert!(Cli::try_parse_from(["tb", "close"]).is_err(), "tb close needs --done");
        assert_eq!(close_done_line(2), "Closing 2 finished tasks' terminals.");
        assert_eq!(close_done_line(0), "No finished task has a terminal to close.");
        let Cmd::Backlog { action: BacklogCmd::Note { issue, text } } = Cli::try_parse_from(["tb", "backlog", "note", "B2", "seen again on the tablet build"]).unwrap().cmd else {
            panic!("not tb backlog note")
        };
        assert_eq!((issue.as_str(), text.as_str()), ("B2", "seen again on the tablet build"));
    }

    #[test]
    fn tb_tasks_and_backlog_list_parse_and_print_one_line_each() {
        let Cmd::Tasks { project, goal, status, json } = Cli::try_parse_from(["tb", "tasks", "--goal", "G41", "--status", "queued", "--json"]).unwrap().cmd else { panic!("not tb tasks") };
        assert_eq!((project, goal.as_deref(), status.as_deref(), json), (None, Some("G41"), Some("queued"), true));
        assert!(Cli::try_parse_from(["tb", "tasks", "--status", "busy"]).is_err(), "an unknown status");
        let Cmd::Backlog { action: BacklogCmd::List { project, kind, state, sort, .. } } = Cli::try_parse_from(["tb", "backlog", "list", "--project", "all", "--kind", "gap", "--state", "all", "--sort", "old"]).unwrap().cmd else {
            panic!("not tb backlog list")
        };
        assert_eq!((project.as_deref(), kind.as_deref(), state.as_deref(), sort.as_deref()), (Some("all"), Some("gap"), Some("all"), Some("old")));
        let t = json!({"ref": "T412", "status": "queued", "goal": {"ref": "G41", "name": "Find"}, "title": "Fix the login", "pr": null});
        assert_eq!(task_list_line(&t), "T412   queued   G41   Fix the login");
        let t = json!({"ref": "T9", "status": "done", "failed": true, "goal": null, "title": "Ship", "pr": {"num": 435}});
        assert_eq!(task_list_line(&t), "T9     failed   -     Ship  PR #435");
        let b = json!({"ref": "B12", "state": "task", "kind": "bug", "goal": null, "title": "Crash on save", "task_ref": "T88"});
        assert_eq!(issue_list_line(&b), "B12   task    bug     -     Crash on save  → T88");
    }

    #[test]
    fn tb_list_defaults_to_the_project_this_terminal_is_in() {
        let projects = [json!({"name": "web", "path": "/src/web"}), json!({"name": "web-admin", "path": "/src/web/admin/"})];
        assert_eq!(project_at(&projects, "/src/web/lib").as_deref(), Some("web"));
        assert_eq!(project_at(&projects, "/src/web/admin/ui").as_deref(), Some("web-admin"), "the deepest project");
        assert_eq!(project_at(&projects, "/src/webby").as_deref(), Some("webby"), "not a project: the folder's name");
        let (c, data) = board_down_ctx("tb-list");
        assert_eq!(list_query(&c, Some("all".into()), Some("41".into())).unwrap(), ["project=all", "goal=G41"]);
        assert_eq!(list_query(&c, None, Some("G41".into())).unwrap(), ["project=all", "goal=G41"], "a goal says which project");
        assert_eq!(list_query(&c, Some("my app".into()), Some("none".into())).unwrap(), ["project=my%20app", "goal=none"]);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn tb_task_and_backlog_show_read_the_item() {
        let Cmd::Task { action: TaskCmd::Show { task, log, json } } = Cli::try_parse_from(["tb", "task", "show", "T383"]).unwrap().cmd else { panic!("not tb task show") };
        assert_eq!((task.as_str(), log, json), ("T383", 5, false));
        let Cmd::Backlog { action: BacklogCmd::Show { issue, json } } = Cli::try_parse_from(["tb", "backlog", "show", "B7", "--json"]).unwrap().cmd else { panic!("not tb backlog show") };
        assert_eq!((issue.as_str(), json), ("B7", true));
        let log: Vec<Value> = (1..=4).rev().map(|n| json!({"at": "", "who": "T383", "kind": "note", "text": format!("step {n}")})).collect();
        let v = json!({"ref": "T383", "status": "working", "project": "taskboard", "goal": {"ref": "G41", "name": "Find tasks"},
                       "title": "tb tasks", "who": "Terminal 3", "detail": "Add readers.\nCLI only.", "summary": null,
                       "pr": {"num": 435, "state": "OPEN", "url": "https://github.com/o/r/pull/435"}, "log": log});
        let lines = task_show_lines(&v, 2);
        assert_eq!(lines[..4], ["T383 · working · taskboard · G41 Find tasks", "tb tasks", "PR: #435 OPEN https://github.com/o/r/pull/435", "Terminal: Terminal 3"]);
        assert_eq!(lines[4..8], ["", "Detail:", "  Add readers.", "  CLI only."]);
        assert_eq!(lines[8..], ["", "Log:", "   · T383 · note: step 3", "   · T383 · note: step 4"], "the latest two, oldest first");
        assert_eq!(task_show_lines(&v, 0).len(), 8, "--log 0 leaves the log out");
        let b = json!({"ref": "B7", "state": "open", "kind": "gap", "project": "taskboard", "goal": null, "title": "No readers",
                       "task_ref": null, "jira_key": "", "from": "Found by T12", "detail": "tb can't list tasks.",
                       "history": [{"at": "", "who": "T12", "kind": "found", "text": "Filed"}]});
        assert_eq!(issue_show_lines(&b), ["B7 · open · gap · taskboard", "No readers", "Found by T12", "", "Detail:", "  tb can't list tasks.", "", "History:", "   · T12 · found: Filed"]);
    }

    #[test]
    fn tb_export_writes_a_zip_and_prints_its_path() {
        let Cmd::Export { out } = Cli::try_parse_from(["tb", "export", "--out", "/tmp/x"]).unwrap().cmd else { panic!("not tb export") };
        assert_eq!(out, Some(std::path::PathBuf::from("/tmp/x")));
        let (c, data) = board_down_ctx("export");
        let into = data.join("out");
        assert_eq!(run_cmd(&c, Cmd::Export { out: Some(into.clone()) }).unwrap(), 0);
        let zips: Vec<_> = std::fs::read_dir(&into).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert!(zips.len() == 1 && zips[0].starts_with("taskboard-debug-") && zips[0].ends_with(".zip"), "{zips:?}");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn tb_qa_goal_narrows_the_list_to_that_goal() {
        let Cmd::Qa { what, goal, .. } = Cli::try_parse_from(["tb", "qa", "waiting", "--goal", "G2"]).unwrap().cmd else { panic!("not tb qa") };
        assert_eq!((what.as_deref(), goal.as_deref()), (Some("waiting"), Some("G2")));
        assert_eq!(qa_list_url(20, true, Some("G2")), "/qa-comments?limit=20&waiting=1&goal=G2");
        assert_eq!(qa_list_url(20, false, Some("G2")), "/qa-comments?limit=20&goal=G2");
        assert_eq!(qa_list_url(20, false, None), "/qa-comments?limit=20");
    }

    #[test]
    fn tb_stream_lists_each_relay_and_what_it_covers() {
        let v = json!({"sources": [{
            "source": "prs", "mode": "stream", "mode_from": "config", "watched": true, "problem": null, "polling": true, "polling_some": true,
            "relays": [
                {"relay": "fi", "problem": null, "idle": false, "last_heartbeat_at": "2026-10-12T22:41:00Z",
                 "covers": [{"provider": "bitbucket", "repo": "acme/webapp"}, {"provider": "bitbucket", "repo": "acme/api"}]},
                {"relay": "github", "problem": null, "idle": false, "last_event_at": "2026-10-12T22:12:00Z", "covers": [], "event_only": true},
                {"relay": "all", "problem": "stuck", "why": "prs's relay all hasn't sent a heartbeat for 4 minutes", "idle": false, "covers": "all"}
            ]
        }]});
        assert!(Cli::try_parse_from(["tb", "stream", "--json"]).is_ok());
        let text = stream_lines(&v);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("prs     stream (config.toml) · healthy") && lines[0].ends_with(" · polling some PRs"), "{}", lines[0]);
        assert!(lines[1].starts_with("  fi     healthy · last heartbeat "), "{}", lines[1]);
        assert!(lines[1].ends_with(" · covers bitbucket acme/webapp, bitbucket acme/api"), "{}", lines[1]);
        assert!(lines[2].starts_with("  github healthy · last event ") && lines[2].ends_with(" · no heartbeats, covers nothing"), "{}", lines[2]);
        let at = lines[1].split("last heartbeat ").nth(1).unwrap().split(' ').next().unwrap().to_lowercase();
        assert!(at.ends_with("am") || at.ends_with("pm"), "12-hour time: {at}");
        assert_eq!(lines[3], "  all    stuck: prs's relay all hasn't sent a heartbeat for 4 minutes · covers every PR");
    }

    #[test]
    fn tb_feed_says_who_is_rate_limited_and_the_board_s_calls() {
        let v = json!([
            {"label": "Bitbucket", "text": "Bitbucket rate-limited until 3:40pm", "why": "Bitbucket answered 429", "calls_last_hour": 812, "max_per_hour": 900, "stretch": 1.5},
            {"label": "GitHub", "text": null, "why": null, "calls_last_hour": 40, "max_per_hour": null, "stretch": null},
        ]);
        assert_eq!(
            rate_lines(&v),
            vec![
                "Bitbucket rate-limited until 3:40pm · the board's calls in the last hour: 812 of 900 · polling 1.5× slower to fit\n  Bitbucket answered 429".to_string(),
                "GitHub isn't rate-limited · the board's calls in the last hour: 40".to_string(),
            ]
        );
        assert!(rate_lines(&Value::Null).is_empty());
    }

    #[test]
    fn tb_start_says_what_the_board_did() {
        let skipped = json!({"status": "done", "summary": "Skipped by hook `x.sh`: APP-41 is already fixed", "starting": false});
        assert_eq!(start_said("T4", false, &skipped), "T4 didn't start: Skipped by hook `x.sh`: APP-41 is already fixed.");
        assert_eq!(start_said("T4", false, &json!({"status": "queued", "starting": true})), "Opening T4 in a new terminal now.");
        assert_eq!(start_said("T4", true, &json!({"status": "queued", "starting": false})), "T4 starts in a new terminal once its project has a free terminal.");
        assert_eq!(start_said("T4", false, &json!({"status": "working", "who": "Term", "starting": false})), "T4 is on Term.");
    }

    #[test]
    fn pr_addressed_on_a_retaken_task_points_to_submit() {
        let visit = addressed_line("T4", &["Pat"], "done");
        assert_eq!(visit, "Asked Pat to review T4's PR again. The board watches the PR and brings this conversation back when it needs you. You can stop here.");
        let retaken = addressed_line("T4", &[], "working");
        assert_eq!(retaken, "Recorded T4's review as addressed. T4 is still taken: push your changes, then run tb submit \"<summary>\" to finish it.");
        assert!(!retaken.contains("stop here"));
        assert!(addressed_line("T4", &[], "needs").contains("tb submit"));
    }

    #[test]
    fn pr_status_counts_approvals_against_the_ones_needed() {
        let pr = |review: &str, have: i64, need: Value| json!({"review": review, "approvals": {"have": have, "need": need}});
        assert_eq!(review_words(&pr("pending", 1, json!(2))), "1 of 2 approved");
        assert_eq!(review_words(&pr("approved", 2, json!(2))), "approved");
        assert_eq!(review_words(&pr("pending", 0, json!(2))), "pending");
        assert_eq!(review_words(&pr("changes", 1, json!(2))), "changes");
        assert_eq!(review_words(&pr("pending", 1, Value::Null)), "pending", "no count: the host decides");
        assert_eq!(review_words(&json!({"review": null})), "unknown");
    }

    #[test]
    fn the_reviewers_line_never_counts_more_approvals_than_needed() {
        assert_eq!(approvals_words(&json!({"have": 3, "need": 2})), " · 2 of 2 approvals", "#150: never 3 of 2");
        assert_eq!(approvals_words(&json!({"have": 1, "need": 2})), " · 1 of 2 approvals");
        assert_eq!(approvals_words(&json!({"have": 2, "need": 1})), " · 1 of 1 approval");
        assert_eq!(approvals_words(&json!({"have": 1, "need": null})), "", "no count: the host decides");
    }

    #[test]
    fn a_goal_run_outside_a_midna_terminal_says_why() {
        let why = goal_run_needs_terminal("tb goal set --run", "G2");
        assert!(why.starts_with("tb goal set --run only works in a Midna terminal"), "{why}");
        assert!(why.contains("press Run on G2 on the board"), "{why}");
    }

    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    /// A repo on `main` with two commits, a branch `feat` one commit further in a second worktree, and a
    /// `Ctx` in the repo.
    fn repo(name: &str) -> (std::path::PathBuf, Ctx, [String; 3]) {
        let root = std::env::temp_dir().join(format!("tb-aim-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo = std::fs::canonicalize(repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "T"]);
        let mut shas = vec![];
        for (i, f) in ["a", "b"].iter().enumerate() {
            std::fs::write(repo.join(f), "x").unwrap();
            git(&repo, &["add", f]);
            git(&repo, &["commit", "-q", "-m", &format!("c{i}")]);
            shas.push(git(&repo, &["rev-parse", "HEAD"]));
        }
        let wt = root.join("feat");
        git(&repo, &["worktree", "add", "-q", "-b", "feat", &wt.to_string_lossy()]);
        std::fs::write(wt.join("c"), "x").unwrap();
        git(&wt, &["add", "c"]);
        git(&wt, &["commit", "-q", "-m", "c2"]);
        shas.push(git(&wt, &["rev-parse", "HEAD"]));
        let c = Ctx { cfg: Config::for_tests(&root), session: String::new(), claude: String::new(), cwd: repo.to_string_lossy().to_string() };
        (root, c, [shas[0].clone(), shas[1].clone(), shas[2].clone()])
    }

    #[test]
    fn an_aimed_round_runs_where_it_looks() {
        let (root, c, [first, second, feat]) = repo("runs");
        let v = json!({"vars": {"repo": c.cwd}});
        let a = |branch: Option<&str>, worktree: Option<&str>, commit: Option<&str>| Aim {
            branch: branch.map(Into::into),
            worktree: worktree.map(Into::into),
            commit: commit.map(Into::into),
        };
        // Unaimed: this checkout's head, run here.
        let at = aim(&c, &v, &Aim::default()).unwrap();
        assert_eq!((at.head.as_deref(), at.branch.as_deref(), at.pinned), (Some(second.as_str()), Some("main"), false));
        // A branch: the worktree that has it.
        let at = aim(&c, &v, &a(Some("feat"), None, None)).unwrap();
        assert!(at.dir.as_deref().unwrap().ends_with("/feat"));
        assert_eq!((at.head.as_deref(), at.pinned), (Some(feat.as_str()), false));
        // A branch no worktree has is refused, as the Python board did.
        git(std::path::Path::new(&c.cwd), &["branch", "loose", &first]);
        let e = aim(&c, &v, &a(Some("loose"), None, None)).err().unwrap();
        assert!(e.contains("No worktree has loose checked out"), "{e}");
        // A commit must be on the branch; an older one is pinned and runs on a checkout of it.
        let e = aim(&c, &v, &a(None, None, Some(&feat))).err().unwrap();
        assert!(e.contains("isn't on main"), "{e}");
        let at = aim(&c, &v, &a(Some("feat"), None, Some("HEAD~1"))).unwrap();
        assert_eq!((at.head.as_deref(), at.pinned), (Some(second.as_str()), true));
        let at = aim(&c, &v, &a(None, None, Some("HEAD~1"))).unwrap();
        assert_eq!((at.head.as_deref(), at.pinned), (Some(first.as_str()), true));
        let pin = PinnedCheckout::for_round(&at).unwrap().expect("a pinned round gets a checkout");
        assert_eq!(git(std::path::Path::new(&pin.path), &["rev-parse", "HEAD"]), first);
        let vars = round_vars(&c, &v, "Review", &at, Some(&pin));
        let (passed, output) = run_script(&c, "Check", "git rev-parse HEAD", &vars, 30);
        assert!(passed && output.contains(&first), "the script ran on the pinned commit: {output}");
        let path = pin.path.clone();
        drop(pin);
        assert!(!std::path::Path::new(&path).exists(), "the throwaway checkout is removed");
        // The saved aim is followed when no flags are given.
        let saved = json!({"vars": {"repo": c.cwd}, "aim": {"branch": "feat"}});
        assert_eq!(aim(&c, &saved, &Aim::default()).unwrap().head.as_deref(), Some(feat.as_str()));
        let saved = json!({"vars": {"repo": c.cwd}, "aim": {"branch": "main", "sha": first}});
        let at = aim(&c, &saved, &Aim::default()).unwrap();
        assert_eq!((at.head.as_deref(), at.pinned), (Some(first.as_str()), true));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_commit_from_a_detached_checkout_needs_its_branch() {
        let (root, c, [first, _second, feat]) = repo("detached");
        let det = root.join("det");
        git(std::path::Path::new(&c.cwd), &["worktree", "add", "-q", "--detach", &det.to_string_lossy(), &first]);
        let det = std::fs::canonicalize(det).unwrap().to_string_lossy().to_string();
        let c = Ctx { cwd: det.clone(), ..c };
        let v = json!({"vars": {"repo": det}});
        let a = |branch: Option<&str>, worktree: Option<&str>, commit: Option<&str>| Aim {
            branch: branch.map(Into::into),
            worktree: worktree.map(Into::into),
            commit: commit.map(Into::into),
        };
        let e = aim(&c, &v, &a(None, None, Some(&feat))).err().unwrap();
        assert!(e.contains("isn't on a branch. Say which one with --branch."), "{e}");
        let e = aim(&c, &v, &a(None, Some(&det), Some("HEAD"))).err().unwrap();
        assert!(e.contains("isn't on a branch"), "{e}");
        // Named with the checkout, the branch is what the commit must be on.
        let e = aim(&c, &v, &a(Some("main"), Some(&det), Some(&feat))).err().unwrap();
        assert!(e.contains("isn't on main"), "{e}");
        let at = aim(&c, &v, &a(Some("main"), Some(&det), Some(&first))).unwrap();
        assert_eq!((at.head.as_deref(), at.branch.as_deref()), (Some(first.as_str()), Some("main")));
        // Without a commit or a branch, the detached checkout the round runs from is refused too (#107).
        assert_eq!(aim(&c, &v, &Aim::default()).err().unwrap(), format!("{det} isn't on a branch. Say which one with --branch."));
        let at = aim(&c, &v, &a(Some("main"), Some(&det), None)).unwrap();
        assert_eq!(at.branch.as_deref(), Some("main"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_detached_checkout_with_its_branch_looks_at_the_branch_tip() {
        let (root, c, [first, second, feat]) = repo("detached-tip");
        let det = root.join("det");
        git(std::path::Path::new(&c.cwd), &["worktree", "add", "-q", "--detach", &det.to_string_lossy(), &first]);
        let det = std::fs::canonicalize(det).unwrap().to_string_lossy().to_string();
        let v = json!({"vars": {"repo": c.cwd}});
        let a = |branch: &str, worktree: &str| Aim { branch: Some(branch.into()), worktree: Some(worktree.into()), commit: None };
        // main has moved past the checkout: the round judges main's tip, on a checkout of it.
        let at = aim(&c, &v, &a("main", &det)).unwrap();
        assert_eq!((at.head.as_deref(), at.branch.as_deref(), at.pinned), (Some(second.as_str()), Some("main"), true));
        let e = aim(&c, &v, &a("no-such-branch", &det)).err().unwrap();
        assert_eq!(e, format!("{det} has no branch no-such-branch."));
        // Without its branch, a detached checkout is refused, as the Python board did.
        let bare = Aim { worktree: Some(det.clone()), ..Aim::default() };
        assert_eq!(aim(&c, &v, &bare).err().unwrap(), format!("{det} isn't on a branch. Say which one with --branch."));
        // A branch the checkout's head isn't on is refused.
        let det2 = root.join("det2");
        git(std::path::Path::new(&c.cwd), &["worktree", "add", "-q", "--detach", &det2.to_string_lossy(), &feat]);
        let det2 = std::fs::canonicalize(det2).unwrap().to_string_lossy().to_string();
        let e = aim(&c, &v, &a("main", &det2)).err().unwrap();
        assert!(e.contains("head isn't on main"), "{e}");
        let at = aim(&c, &v, &a("feat", &det2)).unwrap();
        assert_eq!((at.head.as_deref(), at.pinned), (Some(feat.as_str()), false), "at the tip, it runs in the checkout");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_gone_reader_doesnt_stop_tb() {
        struct Gone(usize);
        impl std::io::Write for Gone {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                self.0 += 1;
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let gone = std::sync::atomic::AtomicBool::new(false);
        let mut w = Gone(0);
        out_to(&mut w, &gone, "Checked out abc in /tmp/x for this round.");
        // Still here: the round carries on, and later lines aren't tried.
        out_to(&mut w, &gone, "Check: true");
        assert!(gone.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(w.0, 1);
        let mut buf = vec![];
        out_to(&mut buf, &std::sync::atomic::AtomicBool::new(false), "line\n");
        assert_eq!(buf, b"line\n");
    }

    #[test]
    fn publish_needs_a_pr() {
        assert!(!has_pr(&json!({"vars": {"pr": "", "pr_url": ""}})));
        assert!(!has_pr(&json!({"vars": {}})));
        assert!(has_pr(&json!({"vars": {"pr": "12", "pr_url": ""}})));
        assert!(has_pr(&json!({"vars": {"pr": "", "pr_url": "https://example.com/pr/1"}})));
    }

    #[test]
    fn asks_split_into_this_terminals_line() {
        let ok = |a: &[&str]| Cli::try_parse_from(a).is_ok();
        assert!(ok(&["tb", "task", "new", "Footer", "--here", "--next"]));
        assert!(ok(&["tb", "task", "new", "Footer", "--here", "--now"]));
        assert!(!ok(&["tb", "task", "new", "Footer", "--next"]), "--next needs --here");
        assert!(!ok(&["tb", "task", "new", "Footer", "--here", "--next", "--now"]));
        assert!(ok(&["tb", "switch", "T3"]));
        assert!(ok(&["tb", "line"]));
        assert!(ok(&["tb", "line", "drop", "T3"]));
    }

    #[test]
    fn step_commands_take_an_aim() {
        assert!(Cli::try_parse_from(["tb", "step", "publish", "Review", "--task", "T3"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "step", "done", "Review", "--branch", "feat", "--commit", "HEAD~1"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "step", "aim", "--worktree", "/tmp", "--commit", "abc1234"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "step", "aim", "--clear"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "step", "aim", "--clear", "--branch", "x"]).is_err());
        assert!(Cli::try_parse_from(["tb", "step", "run", "Review", "--branch", "x", "--worktree", "/tmp"]).is_ok(), "a detached checkout takes its branch with it");
    }

    #[test]
    fn step_skip_needs_a_reason() {
        assert!(Cli::try_parse_from(["tb", "step", "skip", "Staging", "--why", "docs only"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "step", "skip", "Staging", "--why", "docs only", "--task", "T3"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "step", "skip", "Staging"]).is_err());
    }

    #[test]
    fn refs() {
        assert_eq!(task_ref("t12").unwrap(), "T12");
        assert_eq!(task_ref("12").unwrap(), "T12");
        assert!(task_ref("G3").is_err());
        assert_eq!(goal_ref("3").unwrap(), "G3");
    }

    #[test]
    fn a_master_verdict_needs_a_reason_and_not_ours_a_title() {
        let v = |why: Option<&str>, title: Option<&str>| Verdict { why: why.map(String::from), who: None, proof: vec![], title: title.map(String::from) };
        let why = "Sam's change broke the build; checked the log.";
        assert!(verdict_check("unsure", &v(None, None)).unwrap_err().contains("--why"));
        assert!(verdict_check("ours", &v(Some("too short"), None)).unwrap_err().contains("--why"));
        assert!(verdict_check("ours", &v(Some(why), None)).is_ok());
        assert!(verdict_check("not-ours", &v(Some(why), None)).unwrap_err().contains("--title"));
        assert!(verdict_check("not-ours", &v(Some(why), Some(&"x".repeat(81)))).unwrap_err().contains("80"));
        assert!(verdict_check("not-ours", &v(Some(why), Some("Android NDK download failed"))).is_ok());
    }

    #[test]
    fn a_change_names_the_terminal_it_came_from() {
        let root = std::env::temp_dir().join(format!("tb-from-here-{}", std::process::id()));
        let c = Ctx { cfg: Config::for_tests(&root), session: "s1".into(), claude: String::new(), cwd: String::new() };
        assert_eq!(c.credited_here("POST", Some(json!({"wave": 2}))), Some(json!({"wave": 2, "by_session": "s1"})));
        assert_eq!(c.credited_here("POST", None), Some(json!({"by_session": "s1"})));
        assert_eq!(c.credited_here("GET", None), None, "a read sends nothing");
        let outside = Ctx { session: String::new(), ..c };
        assert_eq!(outside.credited_here("POST", Some(json!({"wave": 2}))), Some(json!({"wave": 2})), "outside Midna it stays the owner's");
    }

    #[test]
    fn joining_a_running_goal_planned_is_asked_for() {
        let Cmd::Propose { planned, .. } = Cli::try_parse_from(["tb", "propose", "G5", "--task", "A::b", "--planned"]).unwrap().cmd else { panic!("not tb propose") };
        assert!(planned);
        let Cmd::Backlog { action: BacklogCmd::Task { planned, .. } } = Cli::try_parse_from(["tb", "backlog", "task", "B3", "--planned"]).unwrap().cmd else { panic!("not tb backlog task") };
        assert!(planned);
        assert!(Cli::try_parse_from(["tb", "backlog", "task", "B3", "--planned", "--board"]).is_err(), "planned is for its goal");
    }

    #[test]
    fn backlog_tidy_takes_plain_lengths() {
        assert!(Cli::try_parse_from(["tb", "backlog", "tidy", "--off", "--every", "daily"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "backlog", "tidy", "--on", "--off"]).is_err());
        assert_eq!(tidy_hours("hourly"), Ok(1.0));
        assert_eq!(tidy_hours("Daily"), Ok(24.0));
        assert_eq!(tidy_hours("6h"), Ok(6.0));
        assert_eq!(tidy_hours("2d"), Ok(48.0));
        assert!(tidy_hours("30m").is_err());
        assert!(tidy_hours("soon").is_err());
    }

    #[test]
    fn parses_commands() {
        assert!(Cli::try_parse_from(["tb", "checkpoint", "--done", "a", "--next", "b", "--decision", "c"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "limits", "--review-gap", "10", "--step", "Author review"]).is_ok());
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
        assert!(Cli::try_parse_from(["tb", "feed"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "pr-builds", "stop", "--reason", "CI is out of minutes", "--who", "Sam"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "pr-builds", "resume"]).is_ok());
        let line = pr_builds_recent_line(&json!({"at": "2026-10-08T15:04:00Z", "ok": true, "what": "stopped 2 builds", "task": "T12", "num": 9,
                                                 "head": "1a2b3c4d5e", "follow_up": true}));
        assert!(line.ends_with("PR #9 (T12) at 1a2b3c4d (follow-up): stopped 2 builds"), "{line}");
        let line = pr_builds_recent_line(&json!({"ok": false, "what": "no command", "repo": "acme/web", "branch": "wip", "num": 0}));
        assert!(line.ends_with("acme/web wip: gave up: no command"), "{line}");
        let line = pr_builds_recent_line(&json!({"ok": true, "what": "stopped 2 builds", "build": "CI #12", "repo": "acme/web", "num": 14}));
        assert!(line.ends_with("PR #14: stopped CI #12"), "one line per build, by its pipeline's name: {line}");
        assert!(Cli::try_parse_from(["tb", "feed", "event", "https://github.com/acme/web/pull/14", "--mine", "--branch", "hand"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "master"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "master", "M3", "not-ours", "--why", "flaky", "--proof", "https://ci/1", "--proof", "https://ci/2"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "master", "show", "M3"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "master", "M3", "not-ours", "--why", "flaky", "--title", "Android NDK download failed", "--proof", "https://ci/1"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "master", "--open", "--limit", "5"]).is_ok());
        let line = break_line(&json!({"ref": "M3", "project": "webapp", "branch": "main", "state": "open", "checks": ["build"],
                                      "verdict_title": "Android NDK download failed", "verdict_label": "not yours"}));
        assert_eq!(line, "M3 · webapp main fails build · Android NDK download failed · not yours");
        assert_eq!(break_ref("m4").as_deref(), Some("M4"));
        assert_eq!(break_ref("ours"), None);
        assert!(Cli::try_parse_from(["tb", "feed", "heartbeat"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "feed", "heartbeat", "--idle", "--idle-until", "2026-10-09T15:00:00Z"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "feed", "heartbeat", "--active", "--idle"]).is_err());
        assert!(Cli::try_parse_from(["tb", "feed", "event", "https://bitbucket.org/a/b/pull-requests/9", "--kind", "build", "--state", "started", "--head", "abc"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "device", "add", "pixel-7", "--tag", "android", "--focus", "open -a Simulator"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "device", "set", "pixel-7", "--off"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "device", "focus", "pixel-7"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "device", "remove", "pixel-7", "--force"]).is_ok());
        assert_eq!(removed_line("pixel-7", &json!({"dropped_from": []})), "Took pixel-7 out of the pool.");
        assert_eq!(removed_line("pixel-7", &json!({"dropped_from": ["G2", "T4"]})), "Took pixel-7 out of the pool and out of G2's and T4's needs.");
        assert!(Cli::try_parse_from(["tb", "task", "new", "x", "--device", "android:2", "--bit", "newCheckout"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--device", "none", "--not-bit", "a"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "set", "G1", "--device", "ios"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "bits", "--goal", "G1"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "bit", "add", "newCheckout", "--backend", "--task", "T1"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "bit", "add", "x", "--backend", "--local"]).is_err());
        assert!(Cli::try_parse_from(["tb", "bit", "made", "newCheckout", "--undo"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "reviewers"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "reviewers", "bot", "ana", "--every", "4", "--mark", "AI review"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "reviewers", "bot", "ana", "--off"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "reviewers", "bot", "ana", "--every", "4"]).is_err());
        assert!(Cli::try_parse_from(["tb", "reviewers", "merge", "Ana", "ana2", "--project", "web"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "pr", "reviewers", "T3", "--replace", "bo", "--with", "cy"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "pr", "reviewers", "--with", "cy"]).is_err());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--wave", "none"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--status", "planned"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--status", "queued"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--status", "done"]).is_err());
        assert!(Cli::try_parse_from(["tb", "backlog", "set", "B3", "--title", "x"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "set", "G1", "--paused", "on"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "setup", "G1", "Run make bootstrap in {task}'s worktree"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "set", "G1", "--run"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "set", "G1", "--deprioritize"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "set", "G1", "--deprioritize", "--prioritize"]).is_err());
        assert!(Cli::try_parse_from(["tb", "goal", "set", "G1", "--jira", "off"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "set", "G1", "--jira", "maybe"]).is_err());
        assert!(Cli::try_parse_from(["tb", "goal", "done", "G1", "--why", "T9 replaced it"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "done", "G1"]).is_err());
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
        assert!(Cli::try_parse_from(["tb", "backlog", "task", "B3", "--here"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "backlog", "task", "B3", "--here", "--next"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "backlog", "task", "B3", "--here", "--now"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "backlog", "task", "B3", "--next"]).is_err(), "--next needs --here");
        assert!(Cli::try_parse_from(["tb", "backlog", "task", "B3", "--here", "--board"]).is_err());
        assert!(Cli::try_parse_from(["tb", "backlog", "task", "B3", "--here", "--next", "--now"]).is_err());
        assert_eq!(issue_refs(&["b4".into(), "B5".into(), "B4".into()]).unwrap(), vec!["B4", "B5"]);
        assert!(Cli::try_parse_from(["tb", "hours", "--alert-every", "10"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "start", "G1", "--now"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "start", "G1", "--now", "--queue"]).is_err());
        assert!(Cli::try_parse_from(["tb", "review", "--week", "last"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "review", "T12"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "review", "--list", "--week", "last"]).is_err());
        assert!(Cli::try_parse_from(["tb", "review", "--since", "3d", "--week", "this"]).is_err());
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
        // --issue: the title can come from the item; it works on this terminal, in a goal and on the board.
        let Cmd::Task { action: TaskCmd::New { title, issue, .. } } = Cli::try_parse_from(["tb", "task", "new", "--issue", "B7", "--here"]).unwrap().cmd else {
            panic!("task new")
        };
        assert_eq!((title, issue.as_deref()), (None, Some("B7")));
        assert!(Cli::try_parse_from(["tb", "task", "new", "--issue", "B7", "--here", "--next"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "new", "--issue", "B7", "--here", "--now"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "new", "Own title", "--issue", "B7", "--goal", "G2"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "new", "--issue", "B7"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "new"]).is_err(), "a title or an item");
        assert!(Cli::try_parse_from(["tb", "api", "bitbucket", "user", "-X", "get"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "new", "x", "--stack-on", "T3", "--no-pr"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "new", "x", "--pr", "--no-pr"]).is_err());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--stack-on", "none", "--pr", "auto"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--pr", "maybe"]).is_err());
        assert!(Cli::try_parse_from(["tb", "task", "new", "x", "--stack-on", "T3", "--stack-merge", "head"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "new", "x", "--stack-merge", "head"]).is_err());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--stack-merge", "tail"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "task", "set", "T1", "--stack-merge", "sideways"]).is_err());
        assert!(Cli::try_parse_from(["tb", "submit", "x", "--no-pr", "covered by T3", "--no-evidence", "no UI"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "submit", "x", "--pr-body", "body.md", "--no-lens", "the owner said to skip it"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "project", "lens", "off"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "project", "set", "webapp", "--lens", "default"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "project", "set", "webapp", "--lens", "maybe"]).is_err());
        assert!(Cli::try_parse_from(["tb", "submit", "x", "--pr-body", "body.md", "--title", "Add x"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "submit", "x", "--pr-body", "body.md", "--no-pr", "why"]).is_err());
        assert!(Cli::try_parse_from(["tb", "submit", "x", "--title", "Add x"]).is_err());
        assert!(Cli::try_parse_from(["tb", "done", "x", "--no-pr", "covered by T3"]).is_ok(), "tb done still works for agents on an older handoff");
        assert!(Cli::try_parse_from(["tb", "step", "triage", "Review", "F2", "--state", "fixed", "--commit", "abc"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "step", "triage", "Review", "F2", "--state", "gone"]).is_err());
        assert!(Cli::try_parse_from(["tb", "step", "again", "Review"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "step", "again", "Review", "--commit", "HEAD~1"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "step", "run", "Tests", "--branch", "feat/x", "--task", "T3"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "step", "run", "Tests", "--worktree", "/tmp/wt"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "step", "run", "Tests", "--branch", "feat/x", "--commit", "abc"]).is_ok(), "a commit on a branch");
        assert!(Cli::try_parse_from(["tb", "pr", "body-check", "-"]).is_ok());
    }

    #[test]
    fn midna_pin_only_inside_midna() {
        let args = midna_pin_args(Some("s1"), "T42", "  Fix the footer ").unwrap();
        assert_eq!(args, ["links", "add", "taskboard://task/T42", "--title", "T42 · Fix the footer"]);
        let args = midna_pin_args(Some("s1"), "G3", "Click refs").unwrap();
        assert_eq!(args, ["links", "add", "taskboard://goal/G3", "--title", "G3 · Click refs"]);
        assert_eq!(midna_pin_args(Some("s1"), "T7", "").unwrap()[4], "T7");
        assert!(midna_pin_args(None, "T42", "x").is_none(), "outside Midna");
        assert!(midna_pin_args(Some(""), "T42", "x").is_none(), "MIDNA_SESSION set but empty");
        assert!(midna_pin_args(Some("s1"), "", "x").is_none(), "no ref came back");
        assert!(midna_pin_args(Some("s1"), "B3", "x").is_none(), "only tasks and goals");
    }

    #[test]
    fn a_rounds_aim_resolves_to_a_sha_and_a_worktree() {
        let dir = std::env::temp_dir().join(format!("tb-aim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |d: &std::path::Path, args: &[&str]| {
            let o = std::process::Command::new("git").arg("-C").arg(d).args(args).output().unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "T"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
        let first = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "two"]);
        let r = repo.to_string_lossy().to_string();
        assert_eq!(resolve_commit(&r, "HEAD~1").unwrap(), first);
        assert_eq!(resolve_commit(&r, &first[..8]).unwrap(), first);
        assert_eq!(resolve_commit(&r, "HEAD").unwrap(), git(&repo, &["rev-parse", "HEAD"]));
        assert!(resolve_commit(&r, "nope").is_err());
        let wt = dir.join("wt");
        git(&repo, &["worktree", "add", "-q", "-b", "feat/x", &wt.to_string_lossy()]);
        let found = worktree_with(&r, "feat/x").unwrap();
        assert_eq!(std::fs::canonicalize(found).unwrap(), std::fs::canonicalize(&wt).unwrap());
        assert!(worktree_with(&r, "feat/y").is_none());
        let _ = std::fs::remove_dir_all(&dir);
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
        assert!(help.contains("{task} {n} {wave} {goal} {jira} {device} {target} {device2} {target2}"), "{help}");
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
        let lent_off = json!({"name": "pixel-7", "tags": ["android"], "off": true, "held_by": {"ref": "T2", "title": "Sim test"}});
        assert_eq!(device_line(&lent_off), "pixel-7 · android · off, still lent to T2 Sim test");
        assert!(Cli::try_parse_from(["tb", "goal", "devices", "G3"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "devices", "G3", "--add", "rig", "--purpose", "measure", "--reserve"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "goal", "devices", "G3", "--reserve"]).is_err(), "reserve says which device");
        assert!(Cli::try_parse_from(["tb", "goal", "devices", "G3", "--add", "rig", "--remove", "rig"]).is_err());
        let own = json!({"name": "rig", "tags": ["bench"], "purpose": "measure", "reserved": true, "held_by": null});
        assert_eq!(goal_device_line(&own), "rig · for measure · reserved · bench · free");
        assert_eq!(device_line(&json!({"name": "rig", "tags": [], "reserved_for": "G3"})), "rig · free · reserved for G3");
    }

    #[test]
    fn basic_auth() {
        assert_eq!(basic("a@b.co", "tok"), "YUBiLmNvOnRvaw==");
        assert_eq!(basic("ab", "c"), "YWI6Yw==");
    }

    /// `cut_release` against a fake board: the bodies it posted, and the scripts it ran.
    fn cut_with(script: Option<&str>, code: i32, no_script: bool) -> (Result<ReleaseCut, String>, Vec<Value>, Vec<(String, String, Option<String>)>) {
        let mut posted = vec![];
        let mut ran = vec![];
        let r = cut_release(
            &mut |b| {
                posted.push(b.clone());
                Ok(if b["check"] == true {
                    json!({"project": "webapp", "name": "v1.4", "script": script, "dir": "/repo"})
                } else {
                    json!({"ref": "R1", "name": "v1.4"})
                })
            },
            &mut |s, n, d, _| {
                ran.push((s.to_string(), n.to_string(), d.map(str::to_string)));
                Ok(code)
            },
            json!({"name": "v1.4", "cwd": "/repo"}),
            no_script,
        );
        (r, posted, ran)
    }

    #[test]
    fn a_pending_cut_line_says_what_it_waits_for() {
        let p = json!({"name": "v1.5", "running": false, "after": [
            {"ref": "T627", "state": "met"}, {"ref": "G12", "state": "waiting"}]});
        assert_eq!(pending_line(&p), "v1.5 · pending · after T627 (done), G12");
        let p = json!({"name": "v1.5", "running": true, "after": [{"ref": "T627", "state": "met"}]});
        assert_eq!(pending_line(&p), "v1.5 · cutting now · after T627 (done)");
    }

    #[test]
    fn release_cut_runs_the_script_then_records_the_release() {
        let (r, posted, ran) = cut_with(Some("scripts/release.sh"), 0, false);
        assert!(matches!(r, Ok(ReleaseCut::Cut(v)) if v["ref"] == "R1"));
        assert_eq!(ran, vec![("scripts/release.sh".into(), "v1.4".into(), Some("/repo".into()))]);
        assert_eq!(posted.len(), 2);
        assert_eq!(posted[0]["check"], true, "it asks before running the script");
        assert_eq!(posted[1].get("check"), None);
    }

    #[test]
    fn release_cut_records_nothing_when_the_script_fails() {
        let (r, posted, ran) = cut_with(Some("scripts/release.sh"), 3, false);
        assert!(matches!(r, Ok(ReleaseCut::ScriptFailed(3))));
        assert_eq!(ran.len(), 1);
        assert_eq!(posted.len(), 1, "only the check, no cut");
    }

    #[test]
    fn release_cut_without_a_script_or_with_no_script_just_records() {
        let (r, posted, ran) = cut_with(None, 0, false);
        assert!(matches!(r, Ok(ReleaseCut::Cut(_))));
        assert!(ran.is_empty());
        assert_eq!(posted.len(), 2);

        let (r, posted, ran) = cut_with(Some("scripts/release.sh"), 1, true);
        assert!(matches!(r, Ok(ReleaseCut::Cut(_))));
        assert!(ran.is_empty(), "--no-script skips it");
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].get("check"), None);
    }

    #[test]
    fn release_cut_tells_the_script_when_it_replaces_a_failed_release() {
        for replaces in [true, false] {
            let mut told = None;
            let check = json!({"name": "v1.4", "script": "scripts/release.sh", "replaces": replaces});
            let r = cut_release(&mut |b| Ok(if b["check"] == true { check.clone() } else { json!({"ref": "R2"}) }), &mut |_, _, _, re| { told = Some(re); Ok(0) }, json!({"name": "v1.4"}), false);
            assert!(r.is_ok());
            assert_eq!(told, Some(replaces));
        }
    }

    #[test]
    fn release_cut_runs_only_on_main_or_a_release_branch() {
        assert!(need_release_branch("main").is_ok());
        assert!(need_release_branch("release/1.4").is_ok());
        assert!(need_release_branch("feat/t1-x").unwrap_err().contains("on feat/t1-x"));
        assert!(need_release_branch("").unwrap_err().contains("detached HEAD"));
    }

    #[test]
    fn release_cut_stops_when_the_board_refuses_the_check() {
        let mut ran = false;
        let r = cut_release(&mut |_| Err("Nothing to release".into()), &mut |_, _, _, _| { ran = true; Ok(0) }, json!({"name": "v1"}), false);
        assert_eq!(r.err().as_deref(), Some("Nothing to release"));
        assert!(!ran, "a doomed cut never runs the script");
    }

    #[test]
    fn release_script_gets_the_name_as_one_argument() {
        let dir = std::env::temp_dir().join(format!("tb-release-script-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().to_string();
        std::fs::write(dir.join("release.sh"), "test \"$#\" = 1 && test \"$1\" = 'v1 rc' && test \"$TB_RELEASE\" = 'v1 rc' && exit 7\nexit 1\n").unwrap();
        assert_eq!(run_release_script("sh release.sh", "v1 rc", Some(&d), false).unwrap(), 7);
        assert_eq!(run_release_script("true", "v1", Some(&d), false).unwrap(), 0);
        assert_eq!(run_release_script("test \"$TB_RELEASE_REPLACE\" = 1 #", "v1", Some(&d), true).unwrap(), 0);
        assert_eq!(run_release_script("test -z \"$TB_RELEASE_REPLACE\" #", "v1", Some(&d), false).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn release_cut_runs_the_script_only_from_main_or_a_release_branch() {
        let dir = std::env::temp_dir().join(format!("tb-release-branch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().to_string();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&d).args(args).output().unwrap().status.success());
        git(&["init", "-q", "-b", "main"]);
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "x"]);
        let ran = dir.join("ran");
        let script = format!("touch '{}' #", ran.display());
        let run = |branch: &str| {
            git(&["checkout", "-q", "-B", branch]);
            let _ = std::fs::remove_file(&ran);
            let r = run_release_script_on_a_release_branch(&script, "v1", Some(&d), false);
            (r, ran.exists())
        };

        let (r, did) = run("feat/t1-x");
        assert!(r.unwrap_err().contains("on feat/t1-x"));
        assert!(!did, "a feature branch doesn't run the script");
        assert!(matches!(run("release/1.4"), (Ok(0), true)));
        assert!(matches!(run("main"), (Ok(0), true)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Runs this repo's scripts/release.sh with `args` and TB_RELEASE (unset when None): its exit
    /// code and stderr. A bad version makes it stop at the usage check, right after the guard.
    fn release_sh(args: &[&str], tb_release: Option<&str>) -> (i32, String) {
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/release.sh");
        let mut cmd = std::process::Command::new("bash");
        cmd.arg(script).args(args).env_remove("TB_RELEASE");
        if let Some(r) = tb_release {
            cmd.env("TB_RELEASE", r);
        }
        let o = cmd.output().unwrap();
        (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stderr).into_owned())
    }

    #[test]
    fn release_sh_refuses_unless_tb_started_it() {
        let (code, err) = release_sh(&["1.2.3"], None);
        assert_eq!(code, 1);
        assert!(err.contains("Run tb release cut v1.2.3 instead"), "{err}");

        let (code, err) = release_sh(&["not-a-version"], Some("not-a-version"));
        assert_eq!(code, 1);
        assert!(err.starts_with("Usage:"), "past the guard with TB_RELEASE: {err}");

        let (code, err) = release_sh(&["--force", "not-a-version"], None);
        assert_eq!(code, 1);
        assert!(err.starts_with("Usage:"), "past the guard with --force: {err}");
    }
}
