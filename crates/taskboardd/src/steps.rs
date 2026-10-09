//! The owner's own steps (`[[steps]]` in config.toml): work on a project's tasks before the PR opens,
//! or before the task finishes. A step is one of:
//! - **agent work** (`prompt`): the agent does it and records it with `tb step done`;
//! - **a script** (`run`): the agent runs it with `tb step run`, and it counts once it exits 0;
//! - **the owner's** (`owner = true`, often with `open`): the agent asks with `tb step ask`, the task waits
//!   in Needs you, and the owner's one-click Done (or `tb step done --task T<n>` elsewhere) carries it on.
//!
//! Agent work and scripts may have a `check`: a script `tb` runs before it records the step, which must
//! exit 0. Prompts, scripts and links take placeholders (`{branch}`, `{base}`, `{base_ref}`, `{pr_url}`,
//! …), and scripts get the same values as `TASKBOARD_*` variables.
//!
//! A check or script may also write a result to the file `$TASKBOARD_RESULT` names: a verdict (`pass`,
//! `fail`, or `skip` for a round that couldn't review and mustn't block), a headline and findings. The
//! task shows the latest round's headline and findings, and `tb step triage` answers a finding. A step
//! with `per_head = true` passes once per head commit, so every push is checked again (an author-side
//! review gate); `min_gap_mins` keeps its rounds apart; `bar` names it in the app's PR bar.
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
    /// A pass counts only for the head commit it ran on: each new push needs the step again.
    #[serde(default)]
    pub per_head: bool,
    /// Minutes between two rounds of the step (its check or script), on the same commit or not.
    #[serde(default)]
    pub min_gap_mins: Option<f64>,
    /// A short name for the step in the app's PR bar ("WD"); empty keeps it out of the bar.
    #[serde(default)]
    pub bar: String,
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

    /// Seconds between two rounds, if the step keeps them apart.
    pub fn gap_secs(&self) -> Option<f64> {
        self.min_gap_mins.filter(|m| *m > 0.0).map(|m| m * 60.0)
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
    steps: Vec<toml::Value>,
}

/// A `[[steps]]` entry that can't be done, and is left out: what to call it (its name, else "#3") and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bad {
    pub step: String,
    pub why: String,
}

/// Every step in a config file's text that can be done, and the ones left out with why. Err only when the
/// file isn't TOML at all.
pub fn parse_lenient(text: &str) -> std::result::Result<(Vec<Step>, Vec<Bad>), String> {
    let f: File = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut seen = HashSet::new();
    let (mut good, mut bad) = (vec![], vec![]);
    for (i, raw) in f.steps.into_iter().enumerate() {
        let label = raw.get("name").and_then(|n| n.as_str()).map(str::trim).filter(|n| !n.is_empty()).map(|n| n.to_string()).unwrap_or_else(|| format!("#{}", i + 1));
        let why = match raw.try_into::<Step>() {
            Err(e) => Some(e.to_string().trim().to_string()),
            Ok(s) if s.name.trim().is_empty() => Some("every [[steps]] needs a name".into()),
            Ok(s) if s.prompt.trim().is_empty() && s.run.trim().is_empty() && !s.owner => Some("it needs a prompt, a run script, or owner = true".into()),
            Ok(s) if s.owner && !s.run.trim().is_empty() => Some("it's the owner's, so it can't have a run script".into()),
            Ok(s) if !seen.insert(key(&s.name)) => Some(format!("another step is called “{}”", s.name)),
            Ok(s) => {
                good.push(s);
                None
            }
        };
        if let Some(why) = why {
            bad.push(Bad { step: label, why });
        }
    }
    Ok((good, bad))
}

/// Every step in a config file's text. A step that can't be done is an error, so a typo doesn't turn a
/// gate off without a word.
pub fn parse(text: &str) -> std::result::Result<Vec<Step>, String> {
    let (good, bad) = parse_lenient(text)?;
    match bad.first() {
        Some(b) => Err(format!("step “{}”: {}", b.step, b.why)),
        None => Ok(good),
    }
}

/// The steps in config.toml. One that can't be done is left out, logged, and named in a board alert
/// (key `steps:<name>`) until it's fixed; the others still apply.
pub fn load(app: &App) -> Vec<Step> {
    let path = &app.cfg.config_path;
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    let (good, bad) = match parse_lenient(&text) {
        Ok(x) => x,
        Err(e) => (vec![], vec![Bad { step: "file".into(), why: e.trim().to_string() }]),
    };
    note_bad(app, &bad);
    good
}

/// Keeps an alert up for each step left out, and clears the ones fixed (logging when that changes).
fn note_bad(app: &App, bad: &[Bad]) {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static LAST: Mutex<Option<HashMap<std::path::PathBuf, Vec<Bad>>>> = Mutex::new(None);
    let path = app.cfg.config_path.clone();
    let changed = {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let m = last.get_or_insert_with(HashMap::new);
        m.insert(path.clone(), bad.to_vec()).as_deref() != Some(bad)
    };
    let wanted: Vec<(String, String)> = bad
        .iter()
        .map(|b| {
            let text = if b.step == "file" {
                format!("Steps are off: {} isn't valid TOML: {}", path.display(), one_line(&b.why, 300))
            } else {
                format!("Step “{}” in {} is ignored: {}", b.step, path.display(), one_line(&b.why, 300))
            };
            (format!("steps:{}", key(&b.step)), text)
        })
        .collect();
    if changed {
        for (_, text) in &wanted {
            app.info(format!("steps: {text}"));
        }
    }
    let up: Vec<(String, String)> = crate::dispatch::alerts(app)
        .iter()
        .filter_map(|a| Some((a["key"].as_str().filter(|k| k.starts_with("steps:"))?.to_string(), a["text"].as_str().unwrap_or("").to_string())))
        .collect();
    if up.len() == wanted.len() && wanted.iter().all(|w| up.contains(w)) {
        return;
    }
    let res = app.db.tx(|| {
        for a in crate::dispatch::alerts(app) {
            let Some(k) = a["key"].as_str().filter(|k| k.starts_with("steps:")) else { continue };
            if !wanted.iter().any(|(w, t)| w == k && a["text"].as_str() == Some(t.as_str())) {
                crate::dispatch::clear_alert_key(app, k)?;
            }
        }
        for (k, text) in &wanted {
            crate::dispatch::add_alert_keyed(app, text, None, None, k)?;
        }
        Ok(())
    });
    if let Err(e) = res {
        app.info(format!("steps: couldn't raise the alert: {}", e.message));
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

/// origin's default branch, if git knows it.
pub fn default_base(repo: &str) -> Option<String> {
    if repo.is_empty() {
        return None;
    }
    std::process::Command::new("git")
        .args(["-C", repo, "symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_start_matches("origin/").to_string())
        .filter(|b| !b.is_empty())
}

/// The branch the repo's PRs go into: origin's default branch, else `main`.
fn base_branch(repo: &str) -> String {
    default_base(repo).unwrap_or_else(|| "main".into())
}

/// The branch this task's PR goes into: the branch of the PR it stacks on while that's unmerged, else
/// the repo's default.
pub fn real_base(app: &App, t: &Row) -> Result<String> {
    Ok(match crate::stack::base_branch(app, t)? {
        Some(b) => b,
        None => base_branch(&t.st("repo_path")),
    })
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

/// `vars` with the real base: a stacked task's parent branch, and `{base_ref}` with the remote on it.
pub fn vars_for(app: &App, t: &Row) -> BTreeMap<String, String> {
    let mut v = vars(t);
    let base = real_base(app, t).unwrap_or_else(|_| v.get("base").cloned().unwrap_or_default());
    v.insert("base_ref".into(), format!("{}/{base}", app.cfg.pr_body.remote));
    v.insert("base".into(), base);
    v
}

/// One recorded round of a step (`tb step done` / `tb step run`), oldest first.
#[derive(Debug, Clone)]
pub struct Round {
    pub key: String,
    pub passed: bool,
    pub head: Option<String>,
    pub at: String,
    pub data: Value,
}

pub fn rounds(app: &App, task_id: i64) -> Result<Vec<Round>> {
    let rows = app.db.q("SELECT at, data FROM events WHERE task_id = ? AND kind = 'step' ORDER BY id", p![task_id])?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let d = serde_json::from_str::<Value>(r.s("data")?).ok()?;
            Some(Round {
                key: key(d["name"].as_str()?),
                passed: d["ok"] != false,
                head: d["head"].as_str().filter(|h| !h.is_empty()).map(|h| h.to_string()),
                at: r.st("at"),
                data: d,
            })
        })
        .collect())
}

/// Two commits are the same when one names the other (a short sha and its full one).
pub fn same_head(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    a.len() >= 7 && b.len() >= 7 && (a.starts_with(b) || b.starts_with(a))
}

/// Has the step passed (on `head`, for a per-head step)? A head the board doesn't know takes any pass.
pub fn passed_on(step: &Step, rounds: &[Round], head: Option<&str>) -> bool {
    let k = key(&step.name);
    rounds.iter().filter(|r| r.key == k && r.passed).any(|r| {
        if !step.per_head {
            return true;
        }
        match (head, r.head.as_deref()) {
            (Some(h), Some(rh)) => same_head(h, rh),
            (Some(_), None) => false,
            (None, _) => true,
        }
    })
}

/// A round that couldn't review (`skip`) or didn't finish (stopped at its timeout): it holds nothing
/// back, so the next round may start straight away, on the same commit or not.
pub fn unreviewed(r: &Round) -> bool {
    r.data["result"]["verdict"] == "skip" || r.data["output"].as_str().map(|o| o.contains("stopped after")).unwrap_or(false)
}

/// Where the task's rounds look (`tb step aim`): `{worktree, branch, sha}`, any of them; null when unaimed.
pub fn saved_aim(t: &Row) -> Value {
    match board::task_context(t).get("step_aim") {
        Some(a) if a.as_object().is_some_and(|o| o.values().any(|v| v.as_str().is_some_and(|s| !s.trim().is_empty()))) => a.clone(),
        _ => Value::Null,
    }
}

/// The commit the saved aim points at now: its pinned sha, else its worktree's head, else its branch's
/// tip in the task's repo.
pub fn aim_head(t: &Row) -> Option<String> {
    let a = saved_aim(t);
    let s = |k: &str| a[k].as_str().filter(|x| !x.trim().is_empty()).map(|x| x.trim().to_string());
    if let Some(sha) = s("sha") {
        return Some(sha);
    }
    let (dir, rev) = match (s("worktree"), s("branch")) {
        (Some(w), _) => (w, "HEAD".to_string()),
        (None, Some(b)) => (t.st("repo_path"), format!("refs/heads/{b}")),
        _ => return None,
    };
    if dir.is_empty() || !std::path::Path::new(&dir).is_dir() {
        return None;
    }
    let git = crate::proc::which("git")?;
    let args: Vec<String> = vec!["-C".into(), dir, "rev-parse".into(), "--verify".into(), "-q".into(), format!("{rev}^{{commit}}")];
    let out = crate::proc::run(&git, &args, None, 5.0).ok().filter(|o| o.code == Some(0))?;
    Some(out.stdout.trim().to_string()).filter(|h| !h.is_empty())
}

/// The head a gate judges per-head steps on: the saved aim's, else the one the caller names, else the
/// task's as the board knows it.
pub fn judged_head(t: &Row, head: Option<&str>) -> Option<String> {
    aim_head(t).or_else(|| head.filter(|h| !h.is_empty()).map(|h| h.to_string())).or_else(|| waitsfor::head_of(t))
}

/// When the step may run its next round, if it keeps rounds apart and the last real round was too recent.
pub fn next_round_at(step: &Step, rounds: &[Round]) -> Option<String> {
    let gap = step.gap_secs()?;
    let k = key(&step.name);
    let last = rounds.iter().filter(|r| r.key == k && !unreviewed(r)).filter_map(|r| parse_iso(&r.at)).reduce(f64::max)?;
    let next = last + gap;
    if next > now_ts() {
        Some(iso(next))
    } else {
        None
    }
}

/// The names of the steps that passed on a task (lowercased). A failed check or script doesn't count.
/// A per-head step's pass on an older commit still counts here: `missing` and `list` judge the head.
pub fn recorded(app: &App, task_id: i64) -> Result<HashSet<String>> {
    let rows = app.db.q("SELECT data FROM events WHERE task_id = ? AND kind = 'step'", p![task_id])?;
    Ok(rows
        .iter()
        .filter_map(|r| r.s("data").and_then(|d| serde_json::from_str::<Value>(d).ok()))
        .filter(|d| d["ok"] != false)
        .filter_map(|d| d["name"].as_str().map(key))
        .collect())
}

/// The task's steps for `before` that haven't passed yet (on its current head, for a per-head step).
pub fn missing(app: &App, t: &Row, before: &[Before]) -> Result<Vec<Step>> {
    missing_at(app, t, before, None)
}

/// `missing`, judging per-head steps on `head` (else the task's head as the board knows it).
pub fn missing_at(app: &App, t: &Row, before: &[Before], head: Option<&str>) -> Result<Vec<Step>> {
    let rounds = rounds(app, t.id())?;
    let known = judged_head(t, head);
    Ok(for_task(app, t).into_iter().filter(|s| before.contains(&s.before) && !passed_on(s, &rounds, known.as_deref())).collect())
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
    let open = for_task(app, t).into_iter().find(|s| key(&s.name) == key(&name)).map(|s| fill(&s.open, &vars_for(app, t))).unwrap_or_default();
    let failed = board::task_context(t).get("step_failed") == Some(&json!(true));
    json!({"name": name, "open": if open.is_empty() { Value::Null } else { json!(open) }, "failed": failed})
}

/// `GET /steps`: a task's steps (placeholders as written), which have passed (on `head`, for a
/// per-head step), when each may run its next round, and the placeholders' values.
pub fn list(app: &App, t: &Row, head: Option<&str>) -> Result<Value> {
    let rounds = rounds(app, t.id())?;
    let known = judged_head(t, head);
    let steps: Vec<Value> = for_task(app, t)
        .iter()
        .map(|s| {
            let mut v = serde_json::to_value(s).unwrap_or_default();
            v["kind"] = json!(s.kind());
            v["done"] = json!(passed_on(s, &rounds, known.as_deref()));
            v["next_round_at"] = json!(next_round_at(s, &rounds));
            v
        })
        .collect();
    Ok(json!({"task": rf("task", t.id()), "session": t.v("session_id"), "steps": steps, "vars": vars_for(app, t),
              "head": known, "aim": saved_aim(t), "pr_open": t.s("status") == Some("done") && board::pr_still_open(t)}))
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
            o.remove("next_round_at");
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
    let v = vars_for(app, t);
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

/// What `tb step triage` may say of a finding.
pub const TRIAGE_STATES: &[&str] = &["open", "fixed", "answered", "dismissed"];

/// A round's result as `tb` read it from `$TASKBOARD_RESULT`: the verdict, headline and findings kept,
/// clipped so a chatty tool can't fill the log.
pub fn clean_result(v: Option<&Value>) -> Option<Value> {
    let v = v.filter(|v| v.is_object())?;
    let verdict = v["verdict"].as_str().map(|s| s.to_lowercase()).filter(|s| matches!(s.as_str(), "pass" | "fail" | "skip"));
    let headline = v["headline"].as_str().map(|h| one_line(h, 200)).filter(|h| !h.is_empty());
    let text = |f: &Value, k: &str, n: usize| f[k].as_str().map(|s| clip(s.trim(), n)).filter(|s| !s.is_empty()).map(Value::String).unwrap_or(Value::Null);
    let findings: Vec<Value> = v["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .take(200)
        .enumerate()
        .map(|(i, f)| {
            if let Some(s) = f.as_str() {
                return json!({"id": format!("F{}", i + 1), "title": one_line(s, 300)});
            }
            let id = match &f["id"] {
                Value::String(s) if !s.trim().is_empty() => one_line(s, 80),
                Value::Number(n) => n.to_string(),
                _ => format!("F{}", i + 1),
            };
            let mut o = json!({"id": id, "title": text(f, "title", 300), "severity": text(f, "severity", 20), "state": text(f, "state", 20),
                               "file": text(f, "file", 300), "detail": text(f, "detail", 1500), "url": text(f, "url", 500)});
            if let Some(l) = f["line"].as_i64() {
                o["line"] = json!(l);
            }
            o
        })
        .collect();
    if verdict.is_none() && headline.is_none() && findings.is_empty() {
        return None;
    }
    Some(json!({"verdict": verdict, "headline": headline, "findings": findings}))
}

/// How a finding's state sorts: the ones still to deal with first.
fn state_rank(state: &str) -> u8 {
    match state {
        "" | "open" | "new" => 0,
        "answered" | "replied" => 1,
        "fixed" | "resolved" => 2,
        "dismissed" | "wontfix" | "false-positive" | "ignored" => 3,
        _ => 4,
    }
}

fn severity_rank(sev: &str) -> u8 {
    match sev {
        "critical" | "blocker" => 0,
        "high" | "major" | "error" => 1,
        "medium" | "warning" => 2,
        "low" | "minor" => 3,
        "info" | "nit" => 4,
        _ => 5,
    }
}

/// A finding counts as open until it's answered, fixed or dismissed.
pub fn finding_open(f: &Value) -> bool {
    state_rank(&f["state"].as_str().unwrap_or("").to_lowercase()) == 0
}

/// A round's findings with later triage (`tb step triage`) on them, sorted by state, then severity.
pub fn findings(round: &Round, triage: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = round.data["result"]["findings"].as_array().cloned().unwrap_or_default();
    for (i, f) in out.iter_mut().enumerate() {
        if !f.is_object() {
            *f = json!({"title": f.as_str().unwrap_or("")});
        }
        if f["id"].is_null() {
            f["id"] = json!(format!("F{}", i + 1));
        }
        let id = f["id"].as_str().map(|s| s.to_string()).unwrap_or_else(|| f["id"].to_string());
        for tr in triage.iter().filter(|t| t["finding"].as_str() == Some(id.as_str()) && t["at"].as_str().unwrap_or("") >= round.at.as_str()) {
            f["state"] = tr["state"].clone();
            if tr["note"].as_str().map(|n| !n.is_empty()).unwrap_or(false) {
                f["note"] = tr["note"].clone();
            }
            if tr["commit"].as_str().map(|c| !c.is_empty()).unwrap_or(false) {
                f["commit"] = tr["commit"].clone();
            }
        }
    }
    out.sort_by_key(|f| (state_rank(&f["state"].as_str().unwrap_or("").to_lowercase()), severity_rank(&f["severity"].as_str().unwrap_or("").to_lowercase())));
    out
}

/// The headline for a round: what its result says, else what came of it.
pub fn headline(round: &Round, findings: &[Value]) -> String {
    if let Some(h) = round.data["result"]["headline"].as_str().map(str::trim).filter(|h| !h.is_empty()) {
        return one_line(h, 200);
    }
    let open = findings.iter().filter(|f| finding_open(f)).count() as i64;
    if unreviewed(round) {
        return if round.data["result"]["verdict"] == "skip" { "Couldn't review this round".into() } else { "Didn't finish".into() };
    }
    if open > 0 {
        return plural(open, "open finding");
    }
    if !round.passed && !findings.is_empty() {
        return "Answered, not approved".into();
    }
    if !round.passed {
        return "Didn't pass".into();
    }
    if findings.is_empty() {
        "No findings".into()
    } else {
        format!("{}, all answered", plural(findings.len() as i64, "finding"))
    }
}

/// What the app shows for each of the task's steps that has run: its latest round's headline and
/// findings, and whether the head has moved since.
pub fn results(app: &App, t: &Row) -> Result<Vec<Value>> {
    let all = rounds(app, t.id())?;
    if all.is_empty() {
        return Ok(vec![]);
    }
    let triage: Vec<Value> = app
        .db
        .q("SELECT at, data FROM events WHERE task_id = ? AND kind = 'step_triage' ORDER BY id", p![t.id()])?
        .iter()
        .filter_map(|r| {
            let mut d = serde_json::from_str::<Value>(r.s("data")?).ok()?;
            d["at"] = json!(r.st("at"));
            Some(d)
        })
        .collect();
    let head = judged_head(t, None);
    let mut out = vec![];
    for st in for_task(app, t) {
        let k = key(&st.name);
        let Some(last) = all.iter().rev().find(|r| r.key == k) else { continue };
        let mine: Vec<Value> = triage.iter().filter(|x| x["name"].as_str().map(key).as_deref() == Some(k.as_str())).cloned().collect();
        let fs = findings(last, &mine);
        let stale = st.per_head && matches!((&head, &last.head), (Some(h), Some(rh)) if !same_head(h, rh));
        out.push(json!({
            "name": st.name, "bar": if st.bar.is_empty() { Value::Null } else { json!(st.bar) },
            "at": last.at, "head": last.head, "passed": last.passed, "stale": stale,
            "verdict": last.data["result"]["verdict"], "headline": headline(last, &fs),
            "open": fs.iter().filter(|f| finding_open(f)).count(), "findings": fs,
            "rounds": all.iter().filter(|r| r.key == k).count(),
        }));
    }
    Ok(out)
}

/// The latest round of the step the PR bar names (`bar = "WD"`), as in `results`; null when the task's
/// project has no such step or it hasn't run.
pub fn bar_result(app: &App, t: &Row) -> Result<Value> {
    if !for_task(app, t).iter().any(|s| !s.bar.is_empty()) {
        return Ok(Value::Null);
    }
    Ok(results(app, t)?.into_iter().find(|r| r["bar"].is_string()).unwrap_or(Value::Null))
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
