//! lens on the board's projects. With `[lens]` on for a project and the `lens` binary installed, the
//! board sweeps what's pushed to it (`lens_sweep.rs`). The old gate at `tb submit` stays behind
//! `[lens] submit` (off by default): a task finishes only on a fresh local review (`lens review`) of
//! what's in its checkout. Verified findings become task notes; a blocker holds `tb submit` while lens's
//! fix loop runs, and only a blocker the loop escalates is asked of the owner.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::board;
use crate::util::*;

/// What `tb submit` does after looking at the review.
#[derive(Debug, PartialEq)]
pub enum Verdict {
    Pass,
    Refuse(String),
    /// A blocker lens escalated: ask the owner, keyed so the same escalation is asked once.
    Ask { key: String, question: String },
}

pub struct Review {
    /// New findings to note on the task, with their keys.
    pub notes: Vec<(String, String)>,
    pub verdict: Verdict,
}

impl Review {
    fn verdict(verdict: Verdict) -> Self {
        Review { notes: vec![], verdict }
    }
}

const NOTED: &str = "lens_noted";
const ASKED: &str = "lens_asked";

/// `[lens]` in config.toml: which projects lens reviews, how, and the binary the board runs (for `[pr.lens]` too).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SubmitConfig {
    /// lens reviews every project (`projects` says otherwise for one). On by default.
    pub on: bool,
    /// Per project, by name: `false` turns lens off for it, `true` on.
    pub projects: std::collections::BTreeMap<String, bool>,
    /// The lens binary: a name looked up on the PATH, or a path.
    pub bin: String,
    /// The old gate: `tb submit` also needs a fresh `lens review` of the task's checkout. Off by default.
    pub submit: bool,
    /// How often the board asks lens whether a project's sweep is due (lens's `[sweep]` decides). 0: never.
    pub sweep_mins: f64,
    /// A sweep that runs longer than this is stopped.
    pub sweep_timeout_mins: f64,
}

impl Default for SubmitConfig {
    fn default() -> Self {
        SubmitConfig { on: true, projects: Default::default(), bin: "lens".into(), submit: false, sweep_mins: 15.0, sweep_timeout_mins: 120.0 }
    }
}

/// `tb project lens on|off`: the board's own word on `[lens] on`, over config.toml's.
const BOARD_SETTING: &str = "lens_on";
/// `tb project set --lens`: name → the board's word on `[lens.projects]`, over config.toml's.
const PROJECT_SETTING: &str = "project_lens";

/// Whether lens reviews this project: the project's word on the board, else `[lens.projects]`, else
/// the board's (`tb project lens`), else `[lens] on`.
pub fn on(app: &App, project: Option<&str>) -> bool {
    let l = &app.cfg.lens;
    project
        .and_then(|p| project_set(app, p).ok().flatten().or_else(|| l.projects.get(p).copied()))
        .unwrap_or_else(|| board_default(app))
}

/// Whether lens reviews projects that don't say: the board's word, else `[lens] on`.
pub fn board_default(app: &App) -> bool {
    board_set(app).unwrap_or(app.cfg.lens.on)
}

/// What `tb project lens` set on the board, if anything.
pub fn board_set(app: &App) -> Option<bool> {
    match app.db.get_setting(BOARD_SETTING).ok().flatten()?.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Sets the board's word on `[lens] on`; None goes back to config.toml's.
pub fn set_board(app: &App, on: Option<bool>) -> Result<()> {
    app.db.set_setting(BOARD_SETTING, on.map(|b| if b { "true" } else { "false" }))
}

/// `GET /projects/lens`: the board-wide switch, what the board set, config.toml's, whether lens is
/// installed and whether `tb submit` gates on it (`[lens] submit`).
pub fn describe_board(app: &App) -> Value {
    json!({"lens": board_default(app), "set": board_set(app), "config": app.cfg.lens.on, "installed": bin(app).is_some(), "submit": app.cfg.lens.submit})
}

fn project_overrides(app: &App) -> Result<Row> {
    Ok(jloads_obj(app.db.get_setting(PROJECT_SETTING)?.as_deref()))
}

/// What `tb project set --lens` put on a project, if anything.
pub fn project_set(app: &App, project: &str) -> Result<Option<bool>> {
    Ok(project_overrides(app)?.get(project).and_then(|v| v.as_bool()))
}

/// Sets a project's lens review from a body's `lens` (on/off, true/false; null or "default": back to
/// config.toml's). False when the body doesn't name it.
pub fn set_project(app: &App, project: &str, body: &Value) -> Result<bool> {
    let Some(v) = body.get("lens") else { return Ok(false) };
    let to = match v {
        Value::Null => None,
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim().to_lowercase().as_str() {
            "on" | "true" => Some(true),
            "off" | "false" => Some(false),
            "default" => None,
            _ => return err(400, "lens is on, off or default."),
        },
        _ => return err(400, "lens is on, off or default."),
    };
    let mut all = project_overrides(app)?;
    match to {
        Some(b) => all.insert(project.into(), json!(b)),
        None => all.remove(project),
    };
    app.db.set_setting(PROJECT_SETTING, Some(&jdumps(&Value::Object(all))))?;
    Ok(true)
}

/// Whether `tb submit` would look for a lens review of this task: the gate is on (`[lens] submit`),
/// lens is on for its project and installed.
pub fn applies(app: &App, project: Option<&str>) -> bool {
    app.cfg.lens.submit && on(app, project) && bin(app).is_some()
}

/// The lens binary, when it's installed.
pub fn bin(app: &App) -> Option<PathBuf> {
    crate::proc::which(&app.cfg.lens.bin)
}

/// Looks at the lens review of the checkout at `cwd` for task `t`. `None` when the gate is off, lens is
/// off for its project or isn't installed, or `cwd` isn't a git checkout: `tb submit` goes on as it would without lens.
pub fn review(app: &App, t: &Row, cwd: &str) -> Result<Option<Review>> {
    if !applies(app, t.s("project")) {
        return Ok(None);
    }
    let Some(root) = Git::new(cwd).out(&["rev-parse", "--show-toplevel"]).map(PathBuf::from) else { return Ok(None) };
    let git = Git::new(&root.to_string_lossy());
    let intent = write_intent(app, t, &root)?;
    let run = format!("Run `lens review --intent {}` in {}, then tb submit again.", intent.display(), root.display());
    let Some(now) = git.tree_now() else { return Ok(Some(Review::verdict(Verdict::Refuse(format!("lens couldn't read the checkout. {run}"))))) };
    let report_path = root.join(".review").join(format!("{}.json", git.branch()));
    let report = std::fs::read_to_string(&report_path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
    let Some(report) = report else {
        let from = default_base(&git).and_then(|base| git.out(&["merge-base", &base, "HEAD"]));
        if from.is_some_and(|from| git.patch_id(&from, &now).as_deref() == Some("")) {
            return Ok(None);
        }
        return Ok(Some(Review::verdict(Verdict::Refuse(format!("This change has no lens review yet. {run}")))));
    };
    if !fresh(&git, &root, &report, &now) {
        return Ok(Some(Review::verdict(Verdict::Refuse(format!("The lens review at {} is older than the change. {run}", report_path.display())))));
    }
    Ok(Some(judge(t, &report, &root)))
}

/// What the fresh report means for `tb submit`.
fn judge(t: &Row, report: &Value, root: &Path) -> Review {
    let ctx = board::task_context(t);
    let noted = str_list(ctx.get(NOTED));
    let asked = str_list(ctx.get(ASKED));
    let findings = report["findings"].as_array().cloned().unwrap_or_default();
    let notes = findings
        .iter()
        .map(|f| (finding_key(f), note(f)))
        .filter(|(k, _)| !noted.contains(k))
        .collect();
    let blockers: Vec<&Value> = findings.iter().filter(|f| f["severity"] == "blocker").collect();
    let open: Vec<&&Value> = blockers.iter().filter(|f| f["fix"]["status"] != "resolved" && f["fix"]["status"] != "escalated").collect();
    if !open.is_empty() {
        let ids = open.iter().map(|f| f["id"].as_str().unwrap_or("?")).collect::<Vec<_>>().join(", ");
        let why = format!(
            "lens found {} blocker{} not through its fix loop yet ({ids}). Run `lens review` again in {} \
             (it picks the fix loop up where it stopped), then tb submit again.",
            open.len(),
            if open.len() == 1 { "" } else { "s" },
            root.display(),
        );
        return Review { notes, verdict: Verdict::Refuse(why) };
    }
    let waiting = t.s("status") == Some("needs") && t.s("needs_reason") == Some("question") && t.s("answered_at").is_none_or(str::is_empty);
    for f in blockers.iter().filter(|f| f["fix"]["status"] == "escalated") {
        let ledger = read_ledger(f);
        let key = format!("{}@{}", finding_key(f), last_tree(&ledger).unwrap_or_default());
        if !asked.contains(&key) {
            return Review { notes, verdict: Verdict::Ask { key, question: escalation(f, &ledger) } };
        }
        if waiting {
            let why = "lens escalated a blocker and the owner hasn't answered yet. End your turn; the board brings you back with the answer.";
            return Review { notes, verdict: Verdict::Refuse(why.into()) };
        }
    }
    Review { notes, verdict: Verdict::Pass }
}

/// Saves keys under a context list (`lens_noted`, `lens_asked`) so each is acted on once.
pub fn remember(app: &App, task_id: i64, asked: bool, keys: &[String]) -> Result<()> {
    if keys.is_empty() {
        return Ok(());
    }
    let field = if asked { ASKED } else { NOTED };
    let t = board::get_task(app, task_id)?;
    let mut ctx = board::task_context(&t);
    let mut list = str_list(ctx.get(field));
    list.extend(keys.iter().filter(|k| !list.contains(k)).cloned().collect::<Vec<_>>());
    ctx.insert(field.into(), json!(list));
    board::save_context(app, task_id, &ctx, false)
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn finding_key(f: &Value) -> String {
    format!("{}:{}:{}", f["file"].as_str().unwrap_or(""), f["line"], f["title"].as_str().unwrap_or(""))
}

fn note(f: &Value) -> String {
    let profiles = f["profiles"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let fix = match f["fix"]["status"].as_str() {
        Some("resolved") => " (fixed by lens's fix loop)",
        Some("escalated") => " (lens's fix loop couldn't fix it)",
        _ => "",
    };
    one_line(
        &format!(
            "lens {} {} ({profiles}): {}:{} {}{fix}. {}",
            f["id"].as_str().unwrap_or("?"),
            f["severity"].as_str().unwrap_or("finding"),
            f["file"].as_str().unwrap_or("?"),
            f["line"],
            f["title"].as_str().unwrap_or(""),
            f["failureScenario"].as_str().unwrap_or(""),
        ),
        2000,
    )
}

pub(crate) fn read_ledger(f: &Value) -> Value {
    f["fix"]["ledger"]
        .as_str()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

fn last_tree(ledger: &Value) -> Option<String> {
    ledger["attempts"].as_array()?.last()?["after"].as_str().map(String::from)
}

fn escalation(f: &Value, ledger: &Value) -> String {
    let attempts = ledger["attempts"].as_array().map(Vec::len).or_else(|| f["fix"]["attempts"].as_u64().map(|n| n as usize)).unwrap_or(0);
    let e = &ledger["escalation"];
    let reason = e["reason"].as_str().filter(|r| !r.is_empty()).unwrap_or("no reason given");
    let needs = e["needs"].as_str().map(|n| format!(" (needs: {n})")).unwrap_or_default();
    let see = ledger["baseTree"].as_str().map(|b| format!(" The attempts are in the working tree: git diff {b}.")).unwrap_or_default();
    one_line(
        &format!(
            "lens couldn't fix blocker {} at {}:{} ({}) after {attempts} attempt{}: {reason}{needs}. {}{see} \
             Ship it as it is, or how should it be fixed?",
            f["id"].as_str().unwrap_or("?"),
            f["file"].as_str().unwrap_or("?"),
            f["line"],
            f["title"].as_str().unwrap_or(""),
            if attempts == 1 { "" } else { "s" },
            f["failureScenario"].as_str().unwrap_or(""),
        ),
        2000,
    )
}

/// Writes the task's intent (title, detail, goal) for `lens review --intent` under `.review/`,
/// which ignores itself in git.
fn write_intent(app: &App, t: &Row, root: &Path) -> Result<PathBuf> {
    let dir = root.join(".review");
    let failed = |e: std::io::Error| ApiError::new(500, format!("couldn't write the lens intent in {}: {e}", dir.display()));
    std::fs::create_dir_all(&dir).map_err(failed)?;
    if !dir.join(".gitignore").exists() {
        std::fs::write(dir.join(".gitignore"), "*\n").map_err(failed)?;
    }
    let mut text = format!("# {}: {}\n", rf("task", t.id()), t.st("title"));
    if let Some(d) = t.s("detail").filter(|d| !d.trim().is_empty()) {
        text.push_str(&format!("\n{}\n", d.trim()));
    }
    if let Some(g) = t.i("goal_id").and_then(|id| board::get_goal(app, id).ok()) {
        text.push_str(&format!("\n## Goal {}: {}\n", rf("goal", g.id()), g.st("name")));
        for (label, k) in [("In short", "tldr"), ("Done when", "outcome")] {
            if let Some(v) = g.s(k).filter(|v| !v.trim().is_empty()) {
                text.push_str(&format!("\n{label}: {}\n", v.trim()));
            }
        }
    }
    let path = dir.join(format!("{}-intent.md", rf("task", t.id())));
    std::fs::write(&path, text).map_err(failed)?;
    Ok(path)
}

/// Is the report about the change the checkout holds now? Compares the reviewed change (after any
/// fix attempts) with the current one by patch id, so a clean rebase or a commit keeps it fresh.
fn fresh(git: &Git, root: &Path, report: &Value, now: &str) -> bool {
    let (Some(base), Some(merge_base)) = (report["base"].as_str(), report["mergeBase"].as_str()) else { return false };
    let Some(reviewed) = reviewed_tree(git, root, report) else { return false };
    let then = git.patch_id(merge_base, &reviewed);
    let from = git.out(&["merge-base", base, "HEAD"]);
    then.is_some() && from.is_some_and(|from| then == git.patch_id(&from, now))
}

fn reviewed_tree(git: &Git, root: &Path, report: &Value) -> Option<String> {
    let fixed = report["findings"].as_array().and_then(|fs| fs.iter().rev().find_map(|f| last_tree(&read_ledger(f))));
    if fixed.is_some() {
        return fixed;
    }
    let head = report["head"].as_str()?;
    if report["dirty"] != true {
        return git.out(&["rev-parse", &format!("{head}^{{tree}}")]);
    }
    let patch = root.join(".review").join(report["branch"].as_str()?).join("diff.patch");
    let index = TempIndex::new();
    let env = index.env();
    git.run_env(&["read-tree", report["mergeBase"].as_str()?], &env, None)?;
    git.run_env(&["apply", "--cached", "--allow-empty", &patch.to_string_lossy()], &env, None)?;
    git.run_env(&["write-tree"], &env, None)
}

fn default_base(git: &Git) -> Option<String> {
    git.out(&["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"])
        .or_else(|| ["main", "master"].into_iter().find(|b| git.out(&["rev-parse", "--verify", "--quiet", &format!("{b}^{{commit}}")]).is_some()).map(String::from))
}

struct Git {
    dir: String,
}

impl Git {
    fn new(dir: &str) -> Self {
        Git { dir: dir.to_string() }
    }

    fn run_env(&self, args: &[&str], env: &[(String, String)], input: Option<&[u8]>) -> Option<String> {
        let bin = crate::proc::which("git")?;
        let mut all: Vec<String> = vec!["-C".into(), self.dir.clone()];
        all.extend(args.iter().map(|a| a.to_string()));
        let o = crate::proc::run_with(&bin, &all, None, 30.0, env, input).ok()?;
        (o.code == Some(0)).then(|| o.stdout.trim().to_string())
    }

    fn out(&self, args: &[&str]) -> Option<String> {
        self.run_env(args, &[], None).filter(|s| !s.is_empty())
    }

    /// The branch name lens files its report under (`detached-<short sha>` off a branch).
    fn branch(&self) -> String {
        self.out(&["symbolic-ref", "--quiet", "--short", "HEAD"])
            .or_else(|| self.out(&["rev-parse", "--short", "HEAD"]).map(|s| format!("detached-{s}")))
            .unwrap_or_default()
    }

    /// The working tree as a tree object, untracked files included and ignored ones left out.
    fn tree_now(&self) -> Option<String> {
        let index = TempIndex::new();
        let env = index.env();
        if self.run_env(&["rev-parse", "--verify", "--quiet", "HEAD"], &[], None).is_some() {
            self.run_env(&["read-tree", "HEAD"], &env, None)?;
        }
        self.run_env(&["add", "-A", "."], &env, None)?;
        self.run_env(&["write-tree"], &env, None)
    }

    /// The patch id of the change from commit `from` to `tree`; empty for no change.
    fn patch_id(&self, from: &str, tree: &str) -> Option<String> {
        let diff = self.run_env(&["diff", "--binary", from, tree], &[], None)?;
        if diff.is_empty() {
            return Some(String::new());
        }
        let id = self.run_env(&["patch-id", "--stable"], &[], Some(format!("{diff}\n").as_bytes()))?;
        id.split_whitespace().next().map(String::from)
    }
}

/// A throwaway git index file, removed when dropped.
struct TempIndex {
    path: PathBuf,
}

impl TempIndex {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        TempIndex { path: std::env::temp_dir().join(format!("taskboard-lens-{}-{n}.index", std::process::id())) }
    }

    fn env(&self) -> Vec<(String, String)> {
        vec![("GIT_INDEX_FILE".into(), self.path.to_string_lossy().into_owned())]
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
