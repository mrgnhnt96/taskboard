//! The owner's own steps (`[[steps]]` in config.toml): work on a project's tasks before the PR opens,
//! or before the task finishes. A step is one of:
//! - **agent work** (`prompt`): the agent does it and records it with `tb step done`;
//! - **a script** (`run`): the agent runs it with `tb step run`, and it counts once it exits 0;
//! - **the owner's** (`owner = true`, often with `open`): the agent asks with `tb step ask`, the task waits
//!   in Needs you, and the owner's one-click Done (or `tb step done --task T<n>` elsewhere) carries it on.
//!
//! Agent work and scripts may have a `check`: a script `tb` runs before it records the step, which must
//! exit 0. Prompts, scripts and links take placeholders (`{branch}`, `{base}`, `{pr_url}`, …), and
//! scripts get the same values as `TASKBOARD_*` variables.
//!
//! The handoff tells the agent the steps, and the board holds the PR (the Claude Code `PreToolUse` hook on
//! `gh pr create` and the like) and `tb done` until they're recorded. The owner's Done in the app isn't
//! held. The file is read fresh each time, like hooks.json, so edits apply without restarting the board.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, hooks, p, waitsfor};

/// How long a `run` or `check` may take before `tb` stops it, unless the step says.
pub const DEFAULT_TIMEOUT_SECS: u64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Before {
    /// Before the agent opens the PR.
    #[default]
    Pr,
    /// Before the agent runs `tb done`.
    Done,
}

impl Before {
    pub fn as_str(self) -> &'static str {
        match self {
            Before::Pr => "pr",
            Before::Done => "done",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub name: String,
    /// A regex on the whole project name, as a hook's matcher; empty or `*` for every project.
    #[serde(default)]
    pub projects: String,
    #[serde(default)]
    pub before: Before,
    /// What the agent does for the step (or, for the owner's, what to ask for).
    #[serde(default)]
    pub prompt: String,
    /// A script that is the step: it counts once it exits 0.
    #[serde(default)]
    pub run: String,
    /// A script that must exit 0 before the step counts.
    #[serde(default)]
    pub check: String,
    /// Seconds `run` and `check` may each take.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// The owner does this step; the task waits for them.
    #[serde(default)]
    pub owner: bool,
    /// Where the step happens: a URL, an app's URL scheme or a file path.
    #[serde(default)]
    pub open: String,
}

impl Step {
    pub fn kind(&self) -> &'static str {
        if self.owner {
            "owner"
        } else if !self.run.is_empty() {
            "script"
        } else {
            "agent"
        }
    }

    pub fn timeout_secs(&self) -> u64 {
        self.timeout.filter(|t| *t > 0).unwrap_or(DEFAULT_TIMEOUT_SECS)
    }

    /// The step with its placeholders filled.
    pub fn filled(&self, vars: &BTreeMap<String, String>) -> Step {
        Step {
            prompt: fill(&self.prompt, vars),
            run: fill(&self.run, vars),
            check: fill(&self.check, vars),
            open: fill(&self.open, vars),
            ..self.clone()
        }
    }

    /// One line on what the step is, for the handoff and refusals.
    pub fn what(&self) -> String {
        let mut parts = vec![];
        if !self.prompt.trim().is_empty() {
            parts.push(self.prompt.trim().trim_end_matches('.').to_string());
        }
        if !self.run.is_empty() {
            parts.push(format!("the script `{}`", self.run.trim()));
        }
        if !self.open.is_empty() {
            parts.push(format!("at {}", self.open.trim()));
        }
        if !self.check.is_empty() {
            parts.push(format!("passes when `{}` exits 0", self.check.trim()));
        }
        parts.join("; ")
    }

    /// How the agent gets past the step.
    pub fn how(&self, tb: &str) -> String {
        let n = &self.name;
        match self.kind() {
            "owner" => format!("{tb} step ask \"{n}\", then end your turn; the board brings you back once it's done"),
            "script" => format!("{tb} step run \"{n}\"; fix what it reports and run it again"),
            _ if !self.check.is_empty() => format!("{tb} step done \"{n}\" --note \"…\" (it runs the check first)"),
            _ => format!("{tb} step done \"{n}\" --note \"<what came of it>\""),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct File {
    #[serde(default)]
    steps: Vec<Step>,
}

/// Every step in a config file's text. A step that can't be done is an error, so a typo doesn't turn a
/// gate off without a word.
pub fn parse(text: &str) -> std::result::Result<Vec<Step>, String> {
    let f: File = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut seen = HashSet::new();
    for s in &f.steps {
        if s.name.trim().is_empty() {
            return Err("every [[steps]] needs a name".into());
        }
        if s.prompt.trim().is_empty() && s.run.trim().is_empty() && !s.owner {
            return Err(format!("step “{}” needs a prompt, a run script, or owner = true", s.name));
        }
        if s.owner && !s.run.trim().is_empty() {
            return Err(format!("step “{}” is the owner's, so it can't have a run script", s.name));
        }
        if !seen.insert(key(&s.name)) {
            return Err(format!("two steps are called “{}”", s.name));
        }
    }
    Ok(f.steps)
}

/// The steps in config.toml. One that doesn't parse is logged and leaves no steps.
pub fn load(app: &App) -> Vec<Step> {
    let path = &app.cfg.config_path;
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    match parse(&text) {
        Ok(s) => s,
        Err(e) => {
            app.info(format!("steps: {} ignored: {e}", path.display()));
            vec![]
        }
    }
}

/// The steps a task's project has, in file order.
pub fn for_task(app: &App, t: &Row) -> Vec<Step> {
    let project = t.st("project");
    load(app).into_iter().filter(|s| hooks::matches(&s.projects, &project)).collect()
}

pub fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// `{name}` → its value; a placeholder with no value stays as it is.
pub fn fill(text: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = text.to_string();
    for (k, v) in vars.iter().filter(|(_, v)| !v.is_empty()) {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// The branch the repo's PRs go into: origin's default branch, else `main`.
fn base_branch(repo: &str) -> String {
    if repo.is_empty() {
        return "main".into();
    }
    std::process::Command::new("git")
        .args(["-C", repo, "symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_start_matches("origin/").to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "main".into())
}

/// What placeholders fill in for a task (`{task}`, `{branch}`, …). Scripts get them as `TASKBOARD_<NAME>`.
pub fn vars(t: &Row) -> BTreeMap<String, String> {
    let repo = t.st("repo_path");
    let mut v = BTreeMap::new();
    v.insert("task".into(), rf("task", t.id()));
    v.insert("title".into(), t.st("title"));
    v.insert("project".into(), t.st("project"));
    v.insert("branch".into(), waitsfor::branch_of(t).unwrap_or_default());
    v.insert("base".into(), base_branch(&repo));
    v.insert("repo".into(), repo);
    v.insert("pr_url".into(), t.st("pr_url"));
    v.insert("jira".into(), t.st("jira_key"));
    v
}

/// The names of the steps that passed on a task (lowercased). A failed check or script doesn't count.
pub fn recorded(app: &App, task_id: i64) -> Result<HashSet<String>> {
    let rows = app.db.q("SELECT data FROM events WHERE task_id = ? AND kind = 'step'", p![task_id])?;
    Ok(rows
        .iter()
        .filter_map(|r| r.s("data").and_then(|d| serde_json::from_str::<Value>(d).ok()))
        .filter(|d| d["ok"] != false)
        .filter_map(|d| d["name"].as_str().map(key))
        .collect())
}

/// The task's steps for `before` that haven't passed yet.
pub fn missing(app: &App, t: &Row, before: &[Before]) -> Result<Vec<Step>> {
    let done = recorded(app, t.id())?;
    Ok(for_task(app, t).into_iter().filter(|s| before.contains(&s.before) && !done.contains(&key(&s.name))).collect())
}

/// The task's step by name (any case), or a 400 naming the ones it has.
pub fn find(app: &App, t: &Row, name: &str) -> Result<Step> {
    let all = for_task(app, t);
    if let Some(s) = all.iter().find(|s| key(&s.name) == key(name)) {
        return Ok(s.clone());
    }
    if all.is_empty() {
        return err(400, format!("{} has no steps.", rf("task", t.id())));
    }
    err(400, format!("“{name}” isn't one of {}'s steps: {}.", rf("task", t.id()), names(&all)))
}

pub fn names(steps: &[Step]) -> String {
    steps.iter().map(|s| format!("“{}”", s.name)).collect::<Vec<_>>().join(", ")
}

/// The step the task waits on the owner for (`tb step ask` or `tb step fail`), if it does.
pub fn waiting(t: &Row) -> Option<String> {
    if t.s("status") != Some("needs") || t.s("needs_reason") != Some("question") {
        return None;
    }
    board::task_context(t).s("step_waiting").filter(|s| !s.is_empty()).map(|s| s.to_string())
}

/// What the app shows on a task that waits on the owner for a step: its name and where it happens.
pub fn waiting_card(app: &App, t: &Row) -> Value {
    let Some(name) = waiting(t) else { return Value::Null };
    let open = for_task(app, t).into_iter().find(|s| key(&s.name) == key(&name)).map(|s| fill(&s.open, &vars(t))).unwrap_or_default();
    let failed = board::task_context(t).get("step_failed") == Some(&json!(true));
    json!({"name": name, "open": if open.is_empty() { Value::Null } else { json!(open) }, "failed": failed})
}

/// `GET /steps`: a task's steps (placeholders as written), which have passed, and the placeholders' values.
pub fn list(app: &App, t: &Row) -> Result<Value> {
    let done = recorded(app, t.id())?;
    let steps: Vec<Value> = for_task(app, t)
        .iter()
        .map(|s| {
            let mut v = serde_json::to_value(s).unwrap_or_default();
            v["kind"] = json!(s.kind());
            v["done"] = json!(done.contains(&key(&s.name)));
            v
        })
        .collect();
    Ok(json!({"task": rf("task", t.id()), "session": t.v("session_id"), "steps": steps, "vars": vars(t)}))
}

/// The steps in a `GET /steps` answer, with whether each has passed.
pub fn from_listing(v: &Value) -> Vec<(Step, bool)> {
    v["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| {
            let mut x = x.clone();
            let done = x["done"] == true;
            let o = x.as_object_mut()?;
            o.remove("done");
            o.remove("kind");
            serde_json::from_value::<Step>(x).ok().map(|s| (s, done))
        })
        .collect()
}

/// The refusal an agent reads when it tries to open the PR or finish with steps left.
pub fn refusal(tb: &str, what: &str, left: &[Step]) -> String {
    let mut lines = vec![format!("Not yet: {what} waits for {} first.", if left.len() == 1 { "this step" } else { "these steps" })];
    for s in left {
        lines.push(format!("- {} ({}): {}", s.name, s.what(), s.how(tb)));
    }
    lines.push("Then try again.".into());
    lines.join("\n")
}

/// The handoff's lines about the task's steps, or nothing when it has none.
pub fn handoff_block(app: &App, t: &Row, tb: &str, ships_prs: bool) -> String {
    let v = vars(t);
    let all: Vec<Step> = for_task(app, t).iter().map(|s| s.filled(&v)).collect();
    let mut s = String::new();
    for (before, when) in [(Before::Pr, "Before you open the PR"), (Before::Done, "Before you run tb done")] {
        let these: Vec<&Step> = all.iter().filter(|s| s.before == before && (ships_prs || before == Before::Done)).collect();
        if these.is_empty() {
            continue;
        }
        s += &format!("\n{when}, do {}:", if these.len() == 1 { "this step" } else { "these steps in order" });
        for (i, st) in these.iter().enumerate() {
            s += &format!("\n{}. {}: {}. Then: {}.", i + 1, st.name, st.what(), st.how(tb));
        }
    }
    if s.is_empty() {
        return s;
    }
    format!(
        "{}\nThe board won't let the PR open or the task finish until they pass. A script or check can take a while: \
         give it time. If one can't pass, {tb} step fail \"<name>\" --why \"…\" and end your turn. {tb} steps lists them.",
        s.trim_start()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_steps_and_defaults_to_before_the_pr() {
        let s = parse(
            "port = 1\n[[steps]]\nname = \"Author-side review\"\nprojects = \"work-.*\"\nprompt = \"Review it\"\ncheck = \"true\"\n\
             [[steps]]\nname = \"Tests\"\nbefore = \"done\"\nrun = \"make test\"\ntimeout = 900\n\
             [[steps]]\nname = \"Sign-off\"\nowner = true\nopen = \"https://x/{branch}\"\n",
        )
        .unwrap();
        assert_eq!(s.iter().map(|s| s.kind()).collect::<Vec<_>>(), vec!["agent", "script", "owner"]);
        assert_eq!(s[0].before, Before::Pr);
        assert_eq!(s[1].before, Before::Done);
        assert_eq!(s[1].timeout_secs(), 900);
        assert_eq!(s[0].timeout_secs(), DEFAULT_TIMEOUT_SECS);
        assert!(hooks::matches(&s[0].projects, "work-app"));
        assert!(!hooks::matches(&s[0].projects, "home"));
    }

    #[test]
    fn a_step_must_be_doable() {
        assert!(parse("[[steps]]\nname = \"\"\nprompt = \"x\"\n").is_err());
        assert!(parse("[[steps]]\nname = \"x\"\n").is_err());
        assert!(parse("[[steps]]\nname = \"x\"\nbefore = \"later\"\nprompt = \"x\"\n").is_err());
        assert!(parse("[[steps]]\nname = \"x\"\nowner = true\nrun = \"y\"\n").is_err());
        assert!(parse("[[steps]]\nname = \"x\"\nprompt = \"y\"\nchek = \"z\"\n").is_err(), "a misspelt key");
        assert!(parse("[[steps]]\nname = \"x\"\nprompt = \"y\"\n[[steps]]\nname = \"X\"\nprompt = \"z\"\n").is_err());
        assert!(parse("port = 1\n").unwrap().is_empty());
    }

    #[test]
    fn placeholders_fill_what_is_known() {
        let mut v = BTreeMap::new();
        v.insert("branch".to_string(), "feat/login".to_string());
        v.insert("pr_url".to_string(), String::new());
        assert_eq!(fill("diff {base}...{branch} {pr_url}", &v), "diff {base}...feat/login {pr_url}");
    }

    #[test]
    fn refusal_says_how_to_get_past_each_kind() {
        let s = parse(
            "[[steps]]\nname = \"Review\"\nprompt = \"Run /author-review\"\n[[steps]]\nname = \"Tests\"\nrun = \"make test\"\n\
             [[steps]]\nname = \"Sign-off\"\nowner = true\nopen = \"https://x\"\n",
        )
        .unwrap();
        let r = refusal("tb", "the PR", &s);
        assert!(r.contains("- Review (Run /author-review): tb step done \"Review\""), "{r}");
        assert!(r.contains("- Tests (the script `make test`): tb step run \"Tests\""), "{r}");
        assert!(r.contains("- Sign-off (at https://x): tb step ask \"Sign-off\", then end your turn"), "{r}");
    }
}
