//! A git worktree per task. A task on the `slot` lane (`worklane.rs`; a goal with a `worktree_base` like
//! `origin/main` is on it) starts in a slot from the pool (`slots.rs`), detached at that base, else at
//! the remote's default branch (a stacked task: at its parent's branch).
//! The slot is claimed before the start, outside any transaction; one that can't be had fails the
//! start with the usual retries. A task started before the pool keeps its `<repo>/.claude/worktrees/T<n>`:
//! once the task is finished (failed, no open PR) and its terminal is gone, the board removes that
//! worktree, or keeps it when it has uncommitted changes. Worktrees the
//! agents made themselves (any other `git worktree list` entry) are matched to their task by path,
//! branch or folder name and removed by the same rules; one no task claims is listed on its project.

use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, p, projects};

static BASE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,199}$").unwrap());
const FETCH_SECS: f64 = 300.0;
pub(crate) const GIT_SECS: f64 = 120.0;
static TASK_IN_BRANCH: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:^|[/_-])t(\d+)(?:[/_-]|$)").unwrap());
static TASK_IN_FOLDER: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:^|[_-])t(\d+)$").unwrap());
const WORKTREES: &str = "/.claude/worktrees/";
/// The other worktrees each pass kept, by path: why, or `no task` (`others`).
const SEEN: &str = "worktrees_seen";

/// A goal's worktree base from a body: a branch or `origin/<branch>`; off or none clears it.
pub fn clean_base(value: Option<&Value>) -> Result<Option<String>> {
    let v = match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => return Ok(None),
        Some(Value::String(s)) => s.trim().to_string(),
        Some(v) => v.to_string(),
    };
    if v.is_empty() || v.eq_ignore_ascii_case("off") || v.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    if !BASE_RE.is_match(&v) || v.contains("..") || v.ends_with(".lock") || v.ends_with('/') {
        return err(400, format!("“{v}” isn't a branch the board can make worktrees from. Give one like origin/main, or off."));
    }
    Ok(Some(v))
}

pub(crate) struct Git {
    pub code: i32,
    pub out: String,
    pub err: String,
}

pub(crate) fn git(path: &str, args: &[&str], timeout: f64) -> Git {
    let Some(bin) = crate::proc::which("git") else { return Git { code: 1, out: String::new(), err: "git isn't installed".into() } };
    let mut all: Vec<String> = vec!["-C".into(), path.into()];
    all.extend(args.iter().map(|a| a.to_string()));
    match crate::proc::run(&bin, &all, None, timeout) {
        Ok(o) => Git { code: o.code.unwrap_or(1), out: o.stdout, err: o.stderr },
        Err(crate::proc::RunError::TimedOut) => Git { code: 1, out: String::new(), err: "git timed out".into() },
        Err(crate::proc::RunError::Spawn(e)) => Git { code: 1, out: String::new(), err: e.to_string() },
    }
}

pub(crate) fn last_line(text: &str) -> String {
    text.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("no output").to_string()
}

pub(crate) fn repo_of(app: &App, t: &Row) -> Result<Option<String>> {
    match t.s("repo_path").filter(|r| !r.is_empty()) {
        Some(r) => Ok(Some(r.to_string())),
        None => projects::project_path(app, t.s("project")),
    }
}

fn folder(repo: &str, t: &Row) -> String {
    format!("{}/.claude/worktrees/{}", repo.trim_end_matches('/'), rf("task", t.id()))
}

fn goal_base(app: &App, t: &Row) -> Result<Option<String>> {
    Ok(board::find_goal(app, t.i("goal_id"))?.and_then(|g| g.s("worktree_base").filter(|b| !b.is_empty()).map(|b| b.to_string())))
}

/// Whether the task works in a slot.
fn in_slot(app: &App, t: &Row) -> Result<bool> {
    Ok(crate::worklane::pick(app, t)? == "slot")
}

/// What a slot starts from when its goal names no base: the remote's default branch, else main or
/// master, else the checkout's head.
fn default_base(repo: &str, remote: bool) -> String {
    let has = |r: &str| git(repo, &["rev-parse", "--verify", "--quiet", r], GIT_SECS).code == 0;
    if remote {
        let head = git(repo, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"], GIT_SECS);
        if head.code == 0 && !head.out.trim().is_empty() {
            return head.out.trim().to_string();
        }
    }
    let names: &[&str] = if remote { &["origin/main", "origin/master", "main", "master"] } else { &["main", "master"] };
    names.iter().find(|b| has(b)).copied().unwrap_or("HEAD").to_string()
}

/// The task's recorded worktree that's there; a slot only while the task still holds it.
fn existing(app: &App, t: &Row) -> Result<Option<String>> {
    let ctx = board::task_context(t);
    let wt = ctx.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str()).map(|s| s.to_string());
    let made = ctx.s("worktree_made").map(|s| s.to_string());
    for p in [wt, made].into_iter().flatten().filter(|p| Path::new(p).is_dir()) {
        if !crate::slots::is_slot(&p) || crate::slots::holds(app, t, &p)? {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// The task's worktree when it works in a slot and it's already there (no git).
pub fn made(app: &App, t: &Row) -> Result<Option<String>> {
    if !in_slot(app, t)? {
        return Ok(None);
    }
    existing(app, t)
}

/// The repo a task would take a slot of: when it works in a slot and the folder is known.
pub fn slot_repo(app: &App, t: &Row) -> Result<Option<String>> {
    if !in_slot(app, t)? {
        return Ok(None);
    }
    Ok(repo_of(app, t)?.filter(|r| Path::new(r).is_dir()))
}

/// Makes the task's worktree if it works in a slot; its folder, or None for the shared folder. Auto's
/// pick is kept as the task starts.
pub fn ensure(app: &App, t: &Row) -> Result<Option<String>> {
    let (lane, from) = crate::worklane::picked(app, t)?;
    if from == "auto" && t.i("goal_id").is_some() {
        crate::worklane::pin(app, t, &lane)?;
    }
    if lane != "slot" {
        return Ok(None);
    }
    if let Some(have) = existing(app, t)? {
        return Ok(Some(have));
    }
    let Some(repo) = repo_of(app, t)?.filter(|r| Path::new(r).is_dir()) else {
        return err(409, format!("The board doesn't know the folder for the project “{}”, so it can't make a worktree.", t.st("project")));
    };
    // A task started before the slot pool keeps its own worktree.
    let path = folder(&repo, t);
    if Path::new(&path).is_dir() {
        record(app, t, &path)?;
        return Ok(Some(path));
    }
    let remote = git(&repo, &["remote", "get-url", "origin"], GIT_SECS).code == 0;
    if remote {
        let r = crate::progress::waiting("git fetch origin", crate::progress::WAITING_EVERY, || git(&repo, &["fetch", "origin"], FETCH_SECS));
        if r.code != 0 {
            return err(409, format!("git fetch failed before making its worktree: {}", last_line(&r.err)));
        }
        crate::progress::stage("Fetched origin");
    }
    let base = goal_base(app, t)?.unwrap_or_else(|| default_base(&repo, remote));
    // A stacked task starts from its parent's branch while the parent's PR is unmerged.
    let start = match crate::stack::base_branch(app, t)? {
        Some(b) => stack_start(&repo, &b, remote).unwrap_or(base),
        None => base,
    };
    crate::slots::claim(app, t, &repo, &start).map(Some)
}

/// Where a stacked task starts: its parent's branch on the remote, else the local one; None when
/// neither exists yet (not pushed, or the child ran ahead of its parent), so it starts from the base.
fn stack_start(repo: &str, branch: &str, remote: bool) -> Option<String> {
    let has = |r: &str| git(repo, &["rev-parse", "--verify", "--quiet", r], GIT_SECS).code == 0;
    if remote && has(&format!("refs/remotes/origin/{branch}")) {
        return Some(format!("origin/{branch}"));
    }
    has(&format!("refs/heads/{branch}")).then(|| branch.to_string())
}

fn record(app: &App, t: &Row, path: &str) -> Result<()> {
    app.db.tx(|| {
        let mut ctx = board::task_context(&board::get_task(app, t.id())?);
        let mut w = ctx.get("where").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        w.insert("worktree".into(), json!(path));
        ctx.insert("where".into(), Value::Object(w));
        ctx.insert("worktree_made".into(), json!(path));
        board::save_context(app, t.id(), &ctx, false)?;
        board::log_event(app, t.id(), board::BOARD, "midna", &format!("Starts in its worktree at {path}"))?;
        Ok(())
    })
}

/// Done with its worktree: failed, its PR merged or closed, or it ends without a PR. A task that ships
/// a PR keeps it while the PR isn't linked yet.
fn finished(app: &App, t: &Row) -> Result<bool> {
    if t.b("failed") {
        return Ok(true);
    }
    if t.i("pr_num").is_some() {
        return Ok(!board::pr_still_open(t));
    }
    Ok(!t.st("no_pr").trim().is_empty() || !crate::projects::task_ships_pr(app, t)?)
}

pub(crate) fn terminal_open(app: &App, t: &Row) -> Result<bool> {
    Ok(board::get_session(app, t.s("session_id"))?.map(|s| s.s("status") != Some("gone")).unwrap_or(false))
}

/// Removes the worktrees of finished tasks whose terminals are gone; keeps one with uncommitted
/// changes. Each is looked at once. Returns the tasks whose worktree was removed.
pub fn clean_up(app: &App) -> Result<Vec<i64>> {
    let mut removed = vec![];
    for t in app.db.q("SELECT * FROM tasks WHERE status = 'done' AND context LIKE '%worktree_made%'", p![])? {
        let ctx = board::task_context(&t);
        let Some(path) = ctx.s("worktree_made").filter(|p| !p.is_empty()).map(|s| s.to_string()) else { continue };
        if ctx.get("worktree_removed").is_some() || ctx.get("worktree_kept").is_some() || !finished(app, &t)? || terminal_open(app, &t)? {
            continue;
        }
        if !Path::new(&path).is_dir() {
            note(app, &t, "worktree_removed", None)?;
            continue;
        }
        let status = git(&path, &["status", "--porcelain"], GIT_SECS);
        if status.code != 0 || !status.out.trim().is_empty() {
            note(app, &t, "worktree_kept", Some(&format!("Left its worktree at {path}: it has uncommitted changes")))?;
            continue;
        }
        let repo = repo_of(app, &t)?.unwrap_or_else(|| path.clone());
        let r = git(&repo, &["worktree", "remove", &path], GIT_SECS);
        if r.code != 0 {
            note(app, &t, "worktree_kept", Some(&format!("Couldn't remove its worktree at {path}: {}", last_line(&r.err))))?;
            continue;
        }
        note(app, &t, "worktree_removed", Some(&format!("Removed its worktree at {path}")))?;
        removed.push(t.id());
    }
    removed.extend(others(app)?);
    Ok(removed)
}

/// One `git worktree list --porcelain` entry.
struct Entry {
    path: String,
    branch: Option<String>,
    locked: bool,
    gone: bool,
}

fn entries(repo: &str) -> Option<Vec<Entry>> {
    let r = git(repo, &["worktree", "list", "--porcelain"], GIT_SECS);
    if r.code != 0 {
        return None;
    }
    let mut all: Vec<Entry> = vec![];
    for line in r.out.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            all.push(Entry { path: path.to_string(), branch: None, locked: false, gone: false });
        } else if let Some(e) = all.last_mut() {
            match line.split_once(' ').map(|(k, _)| k).unwrap_or(line) {
                "branch" => e.branch = line.strip_prefix("branch ").map(|b| b.trim_start_matches("refs/heads/").to_string()),
                "locked" => e.locked = true,
                "prunable" => e.gone = true,
                _ => {}
            }
        }
    }
    Some(all)
}

/// A path with its links resolved (git lists `/private/var/…` for `/var/…`), through its deepest
/// folder that's there.
pub(crate) fn real(path: &str) -> String {
    let path = Path::new(path);
    for up in path.ancestors() {
        if let Ok(r) = std::fs::canonicalize(up) {
            let rest = path.strip_prefix(up).unwrap_or(Path::new(""));
            return r.join(rest).to_string_lossy().trim_end_matches('/').to_string();
        }
    }
    path.to_string_lossy().trim_end_matches('/').to_string()
}

fn inside(path: &str, dir: &str) -> bool {
    if path.is_empty() || dir.is_empty() {
        return false;
    }
    let (path, dir) = (real(path), real(dir));
    path == dir || path.starts_with(&format!("{dir}/"))
}

/// The task an agent's worktree belongs to: one whose recorded worktree is in it or whose branch it
/// has, else the `t<n>` in its branch or folder name when T<n> is this project's.
fn task_of(app: &App, project: &str, repo: &str, e: &Entry, tasks: &[Row]) -> Result<Option<Row>> {
    let here = tasks.iter().find(|t| {
        let ctx = board::task_context(t);
        let w = ctx.get("where");
        let wt = w.and_then(|w| w.get("worktree")).and_then(|v| v.as_str()).unwrap_or("");
        let branch = w.and_then(|w| w.get("branch")).and_then(|v| v.as_str()).unwrap_or("");
        inside(wt, &e.path) || (!branch.is_empty() && e.branch.as_deref() == Some(branch))
    });
    if let Some(t) = here {
        return Ok(Some(t.clone()));
    }
    let name = Path::new(&e.path).file_name().and_then(|n| n.to_str()).unwrap_or("");
    let num = e.branch.as_deref().and_then(|b| TASK_IN_BRANCH.captures(b)).or_else(|| TASK_IN_FOLDER.captures(name)).and_then(|c| c[1].parse::<i64>().ok());
    let Some(t) = board::find_task(app, num)? else { return Ok(None) };
    let ours = t.s("project") == Some(project) || t.s("repo_path").map(|r| real(r) == real(repo)).unwrap_or(false);
    Ok(ours.then_some(t))
}

/// Why the agent's worktree at `path` stays, or None when it can go.
fn keep_why(app: &App, t: &Row, e: &Entry) -> Result<Option<String>> {
    if t.s("status") != Some("done") {
        return Ok(Some("its task is still active".into()));
    }
    if !finished(app, t)? {
        return Ok(Some(if board::pr_still_open(t) { "its PR is still open" } else { "its PR isn't linked yet" }.into()));
    }
    let open = app.db.q("SELECT project_path FROM sessions WHERE status IS NOT 'gone' AND project_path IS NOT NULL", p![])?;
    if terminal_open(app, t)? || open.iter().any(|s| inside(s.st("project_path").as_str(), &e.path)) {
        return Ok(Some("a terminal is still open in it".into()));
    }
    if e.locked {
        return Ok(Some("it's locked".into()));
    }
    let status = git(&e.path, &["status", "--porcelain"], GIT_SECS);
    if status.code != 0 || !status.out.trim().is_empty() {
        return Ok(Some("it has uncommitted changes".into()));
    }
    Ok(None)
}

/// The worktrees agents made in each project's repo: removes those of finished tasks the way
/// `clean_up` does, and remembers why each other one stays (saying so once, on its task's timeline
/// and in the daemon log) and which match no task (`stray`). Never the main checkout.
fn others(app: &App) -> Result<Vec<i64>> {
    let mut removed = vec![];
    let before = jloads_obj(app.db.get_setting(SEEN)?.as_deref());
    let mut seen = serde_json::Map::new();
    for p in projects::list_projects(app)? {
        let (Some(name), Some(repo)) = (p["name"].as_str(), p["path"].as_str()) else { continue };
        if !Path::new(repo).is_dir() {
            continue;
        }
        let Some(list) = entries(repo) else { continue };
        let Some(main) = list.first().map(|e| e.path.clone()) else { continue };
        let tasks = app.db.q("SELECT * FROM tasks WHERE project = ? OR repo_path = ?", p![name, repo])?;
        let made: Vec<String> = tasks.iter().filter_map(|t| board::task_context(t).s("worktree_made").map(real)).collect();
        let mut pruned = false;
        for e in list.iter().skip(1).filter(|e| !e.gone && real(&e.path) != real(&main) && real(&e.path) != real(repo)) {
            // The board's own are the pass above's; the pool's slots stay.
            if made.contains(&real(&e.path)) || crate::slots::is_slot(&e.path) {
                continue;
            }
            let Some(t) = task_of(app, name, repo, e, &tasks)? else {
                if before.get(&e.path).is_none() {
                    app.info(format!("worktrees: {} in {name} matches no task; left for the owner", e.path));
                }
                seen.insert(e.path.clone(), json!({"project": name, "why": "no task"}));
                continue;
            };
            let why = match keep_why(app, &t, e)? {
                Some(why) => why,
                None => {
                    let r = git(repo, &["worktree", "remove", &e.path], GIT_SECS);
                    if r.code == 0 {
                        board::log_event(app, t.id(), board::BOARD, "midna", &format!("Removed its worktree at {}", e.path))?;
                        app.info(format!("worktrees: removed {} ({})", e.path, rf("task", t.id())));
                        removed.push(t.id());
                        pruned = true;
                        continue;
                    }
                    format!("git couldn't remove it: {}", last_line(&r.err))
                }
            };
            if before.get(&e.path).and_then(|v| v["why"].as_str()) != Some(why.as_str()) {
                app.info(format!("worktrees: kept {} ({}): {why}", e.path, rf("task", t.id())));
                if t.s("status") == Some("done") {
                    board::log_event(app, t.id(), board::BOARD, "midna", &format!("Left its worktree at {}: {why}", e.path))?;
                }
            }
            seen.insert(e.path.clone(), json!({"project": name, "task": t.id(), "why": why}));
        }
        if pruned {
            git(repo, &["worktree", "prune"], GIT_SECS);
        }
    }
    if Value::Object(seen.clone()) != Value::Object(before) {
        app.db.set_setting(SEEN, Some(&Value::Object(seen).to_string()))?;
    }
    Ok(removed)
}

/// The project's worktrees the last pass matched to no task, for the owner to look at.
pub fn stray(app: &App, project: &str) -> Result<Vec<String>> {
    let seen = jloads_obj(app.db.get_setting(SEEN)?.as_deref());
    Ok(seen.iter().filter(|(_, v)| v["project"] == project && v["why"] == "no task").map(|(k, _)| k.clone()).collect())
}

fn note(app: &App, t: &Row, key: &str, text: Option<&str>) -> Result<()> {
    app.db.tx(|| {
        let mut ctx = board::task_context(&board::get_task(app, t.id())?);
        ctx.insert(key.into(), json!(now_iso()));
        board::save_context(app, t.id(), &ctx, false)?;
        if let Some(text) = text {
            board::log_event(app, t.id(), board::BOARD, "midna", text)?;
        }
        Ok(())
    })
}

/// `…/.claude/worktrees/<name>` for a path inside a worktree, else None.
pub fn root_of(path: Option<&str>) -> Option<String> {
    let p = path?;
    let i = p.find(WORKTREES)?;
    let rest = &p[i + WORKTREES.len()..];
    let name = rest.split('/').next().filter(|n| !n.is_empty())?;
    Some(format!("{}{name}", &p[..i + WORKTREES.len()]))
}

/// Another open task whose worktree this path is in (its PR still open, if it's done).
pub fn owner(app: &App, path: &str, t: &Row) -> Result<Option<Row>> {
    let Some(root) = root_of(Some(path)) else { return Ok(None) };
    // A slot is its holder's, whoever had it before.
    if crate::slots::is_slot(&root) {
        return crate::slots::held_by_other(app, &root, t);
    }
    let mine = board::task_context(t);
    if root_of(mine.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str())).as_deref() == Some(root.as_str()) {
        return Ok(None);
    }
    for o in app.db.q("SELECT * FROM tasks WHERE id != ? AND context LIKE '%worktree%'", p![t.id()])? {
        if o.s("status") == Some("done") && !board::pr_still_open(&o) {
            continue;
        }
        // A task waiting in this terminal's line shares its checkout; it doesn't own it.
        if o.s("line_session").is_some() && o.s("line_session") == t.s("session_id") {
            continue;
        }
        let ctx = board::task_context(&o);
        let theirs = [
            root_of(ctx.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str())),
            root_of(ctx.s("worktree_made")),
        ];
        if theirs.iter().any(|r| r.as_deref() == Some(root.as_str())) {
            return Ok(Some(o));
        }
    }
    Ok(None)
}
