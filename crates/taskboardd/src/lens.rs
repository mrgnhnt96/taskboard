//! lens at `tb done`: when the lens plugin is installed, a task finishes only on a fresh local review
//! (`/lens:review`) of what's in its checkout. Verified findings become task notes; a blocker holds
//! `tb done` while lens's fix loop runs, and only a blocker the loop escalates is asked of the owner.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

use crate::app::App;
use crate::board;
use crate::util::*;

/// What `tb done` does after looking at the review.
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

/// Is lens installed for the repo at `root`, per Claude Code's `installed_plugins.json`?
pub fn installed(claude_dir: &Path, root: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(claude_dir.join("plugins/installed_plugins.json")) else { return false };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return false };
    let Some(plugins) = v["plugins"].as_object() else { return false };
    plugins.iter().filter(|(name, _)| name.split('@').next() == Some("lens")).any(|(_, installs)| {
        installs.as_array().is_some_and(|a| {
            a.iter().any(|i| match i["projectPath"].as_str().filter(|p| !p.is_empty()) {
                Some(p) if i["scope"].as_str() != Some("user") => root.starts_with(p),
                _ => true,
            })
        })
    })
}

/// The Claude Code folder (`~/.claude`) that holds the projects folder the board reads.
pub fn claude_dir(app: &App) -> PathBuf {
    app.cfg.claude_projects.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// Looks at the lens review of the checkout at `cwd` for task `t`. `None` when lens isn't installed
/// or `cwd` isn't a git checkout: `tb done` goes on as it would without lens.
pub fn review(app: &App, t: &Row, cwd: &str) -> Result<Option<Review>> {
    let Some(root) = Git::new(cwd).out(&["rev-parse", "--show-toplevel"]).map(PathBuf::from) else { return Ok(None) };
    if !installed(&claude_dir(app), &root) {
        return Ok(None);
    }
    let git = Git::new(&root.to_string_lossy());
    let intent = write_intent(app, t, &root)?;
    let run = format!("Run /lens:review --intent {} in {}, then tb done again.", intent.display(), root.display());
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
    Ok(Some(judge(t, &report, &report_path)))
}

/// What the fresh report means for `tb done`.
fn judge(t: &Row, report: &Value, report_path: &Path) -> Review {
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
        let first = open[0]["id"].as_str().unwrap_or("F1");
        let why = format!(
            "lens found {} blocker{} not through its fix loop yet ({ids}). Run the loop as /lens:review step 7 says, \
             starting with `lens fix start --report {} --finding {first}`, then tb done again.",
            open.len(),
            if open.len() == 1 { "" } else { "s" },
            report_path.display(),
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

fn read_ledger(f: &Value) -> Value {
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

/// Writes the task's intent (title, detail, goal) for `/lens:review --intent` under `.review/`,
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
