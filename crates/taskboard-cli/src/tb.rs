//! `tb`: how a Claude session (or a person) reports to the task board.

use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};

use crate::client::{self, CallError};
use crate::hook;
use taskboardd::accounts::{self, Provider};
use taskboardd::config::Config;
use taskboardd::hooks;

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
        #[arg(long = "task")]
        tasks: Vec<String>,
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
    /// Add, change or delete a task
    Task {
        #[command(subcommand)]
        action: TaskCmd,
    },
    /// Add a backlog issue
    Backlog {
        #[command(subcommand)]
        action: BacklogCmd,
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
        #[arg(long = "task")]
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
enum TaskCmd {
    /// Add a task: planned in a goal, or on its own
    New {
        title: String,
        #[arg(long)]
        detail: Option<String>,
        #[arg(long)]
        goal: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        planned: bool,
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
        #[arg(long, value_parser = ["normal", "high"])]
        priority: Option<String>,
        #[arg(long = "waits-for")]
        waits_for: Option<String>,
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
    lines.push(format!(
        "Runs {}, at most {} at a time.",
        if g["run_in_order"] == true { "in order" } else { "in any order" },
        g["max_terminals"]
    ));
    if let Some(tasks) = g["tasks"].as_array() {
        for (i, t) in tasks.iter().enumerate() {
            let status = if t["failed"] == true { "failed".to_string() } else { t["status"].as_str().unwrap_or("").to_string() };
            let mut line = format!("  {}. {} [{status}] {}", i + 1, t["ref"].as_str().unwrap_or(""), t["title"].as_str().unwrap_or(""));
            if let Some(w) = t["waiting"].as_str() {
                line += &format!(" — {w}");
            }
            if let Some(u) = t["pr"]["url"].as_str() {
                line += &format!(" ({u})");
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
        Cmd::Checkpoint { done, next, decisions, files, t } => {
            let mut f = json!({"done": done, "next": next, "decisions": decisions});
            if !files.is_empty() {
                f["files"] = json!(files);
            }
            c.run_report("tb.checkpoint", f, t.task, true, |v| format!("Checkpoint saved on {}.", v["task"].as_str().unwrap_or("")))
        }
        Cmd::Found { title, kind, detail, output, t } => c.run_report(
            "tb.found",
            json!({"title": title, "kind": kind, "detail": detail, "output": output.map(|o| o.chars().take(4000).collect::<String>())}),
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
        Cmd::Done { summary, pr, t } => {
            c.run_report("tb.done", json!({"summary": summary, "pr": pr}), t.task, true, |v| format!("{} is done.", v["task"].as_str().unwrap_or("")))
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
            match c.report("tb.propose", json!({"goal": g, "tasks": tasks}), None, TB_TIMEOUT)? {
                None => out(SAVED),
                Some(v) => out(&format!("Added {} to {g} as planned.", v["created"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default())),
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
            GoalCmd::Set { goal, name, outcome, tldr, paused, in_order, max_terminals, epic, product } => {
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
                if b.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                    return Err("say what to change, for example: tb goal set G3 --paused on".into());
                }
                let v = c.call("POST", &format!("/goals/{g}"), Some(b))?;
                out(&format!("Changed {} “{}”.", g, v["name"].as_str().unwrap_or("")));
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
                    return Ok(2);
                }
                let v = c.call("POST", &format!("/goals/{g}/delete"), Some(json!({"tasks": delete_tasks, "backlog": delete_backlog})))?;
                out(&format!("Deleted {g} with {} tasks and {} backlog issues.", v["tasks"], v["issues"]));
                Ok(0)
            }
        },
        Cmd::Task { action } => match action {
            TaskCmd::New { title, detail, goal, project, planned } => {
                let goal = goal.map(|g| goal_ref(&g)).transpose()?;
                match c.report("tb.new_task", json!({"title": title, "detail": detail, "goal": goal, "project": project, "planned": planned}), None, TB_TIMEOUT)? {
                    None => out(SAVED),
                    Some(v) => {
                        let r = v["created"][0].as_str().unwrap_or("").to_string();
                        let where_ = v["goal"].as_str().map(|g| format!(" in {g} as planned")).unwrap_or_else(|| " on the board; it waits for the owner to press Start".into());
                        out(&format!("Added {r}{where_}. {}#/?task={r}", c.cfg.page_url));
                    }
                }
                Ok(0)
            }
            TaskCmd::Set { task, title, detail, goal, priority, waits_for, jira } => {
                let t = task_ref(&task)?;
                let mut b = json!({});
                if let Some(x) = title {
                    b["title"] = json!(x);
                }
                if let Some(x) = detail {
                    b["detail"] = json!(x);
                }
                if let Some(x) = goal {
                    b["goal_id"] = if x.eq_ignore_ascii_case("none") { Value::Null } else { json!(goal_ref(&x)?) };
                }
                if let Some(x) = priority {
                    b["priority"] = json!(x);
                }
                if let Some(x) = waits_for {
                    b["waits_for"] = json!(x);
                }
                if let Some(x) = jira {
                    b["jira_key"] = json!(x);
                }
                if b.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                    return Err("say what to change, for example: tb task set T12 --priority high".into());
                }
                let v = c.call("POST", &format!("/tasks/{t}"), Some(b))?;
                out(&format!("Changed {t} “{}”.", v["title"].as_str().unwrap_or("")));
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
            let name = if c.session.is_empty() { "tb".to_string() } else { format!("terminal {}", c.session.chars().take(8).collect::<String>()) };
            let v = c.call(
                "POST",
                "/backlog",
                Some(json!({"title": title, "kind": kind, "detail": detail, "goal_id": goal, "project": project, "cwd": c.cwd, "where": "tb", "who": name})),
            )?;
            out(&format!("Added {} to the backlog.", v["ref"].as_str().unwrap_or("")));
            Ok(0)
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
        Cmd::Hours { on, off, start, end, days, today_until } => {
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
            let v = if b.as_object().map(|o| o.is_empty()).unwrap_or(true) { c.call("GET", "/hours", None)? } else { c.call("POST", "/hours", Some(b))? };
            out(v["line"].as_str().unwrap_or(""));
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
        Cmd::Statusline { pass } => Ok(hook::statusline(pass)),
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
        assert!(Cli::try_parse_from(["tb", "goal", "set", "G1", "--paused", "on"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "pr", "status"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "token", "bitbucket", "--user"]).is_ok());
        assert!(Cli::try_parse_from(["tb", "token", "jira"]).is_err());
        assert!(Cli::try_parse_from(["tb", "api", "bitbucket", "user", "-X", "get"]).is_ok());
    }

    #[test]
    fn basic_auth() {
        assert_eq!(basic("a@b.co", "tok"), "YUBiLmNvOnRvaw==");
        assert_eq!(basic("ab", "c"), "YWI6Yw==");
    }
}
