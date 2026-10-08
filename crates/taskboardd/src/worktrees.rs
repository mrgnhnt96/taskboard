//! A git worktree per task. A goal with a `worktree_base` (like `origin/main`) starts each task in
//! `<repo>/.claude/worktrees/T<n>`, detached at that base.
//! The worktree is made before the start, outside any transaction; one that can't be made fails the
//! start with the usual retries. Once the task is finished (failed, no open PR) and its terminal is
//! gone, the board removes the worktree, or keeps it when it has uncommitted changes.

use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, p, projects};

static BASE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,199}$").unwrap());
const FETCH_SECS: f64 = 300.0;
const GIT_SECS: f64 = 120.0;
const WORKTREES: &str = "/.claude/worktrees/";

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

struct Git {
    code: i32,
    out: String,
    err: String,
}

fn git(path: &str, args: &[&str], timeout: f64) -> Git {
    let Some(bin) = crate::proc::which("git") else { return Git { code: 1, out: String::new(), err: "git isn't installed".into() } };
    let mut all: Vec<String> = vec!["-C".into(), path.into()];
    all.extend(args.iter().map(|a| a.to_string()));
    match crate::proc::run(&bin, &all, None, timeout) {
        Ok(o) => Git { code: o.code.unwrap_or(1), out: o.stdout, err: o.stderr },
        Err(crate::proc::RunError::TimedOut) => Git { code: 1, out: String::new(), err: "git timed out".into() },
        Err(crate::proc::RunError::Spawn(e)) => Git { code: 1, out: String::new(), err: e.to_string() },
    }
}

fn last_line(text: &str) -> String {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("no output").to_string()
}

fn repo_of(app: &App, t: &Row) -> Result<Option<String>> {
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

fn existing(t: &Row) -> Option<String> {
    let ctx = board::task_context(t);
    let wt = ctx.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str()).map(|s| s.to_string());
    let made = ctx.s("worktree_made").map(|s| s.to_string());
    [wt, made].into_iter().flatten().find(|p| Path::new(p).is_dir())
}

/// The task's worktree when its goal makes them and it's already there (no git).
pub fn made(app: &App, t: &Row) -> Result<Option<String>> {
    Ok(if goal_base(app, t)?.is_some() { existing(t) } else { None })
}

/// Makes the task's worktree if its goal asks for one; its folder, or None for the shared folder.
pub fn ensure(app: &App, t: &Row) -> Result<Option<String>> {
    let Some(base) = goal_base(app, t)? else { return Ok(None) };
    if let Some(have) = existing(t) {
        return Ok(Some(have));
    }
    let Some(repo) = repo_of(app, t)?.filter(|r| Path::new(r).is_dir()) else {
        return err(409, format!("The board doesn't know the folder for the project “{}”, so it can't make a worktree.", t.st("project")));
    };
    let path = folder(&repo, t);
    if Path::new(&path).is_dir() {
        record(app, t, &path, None)?;
        return Ok(Some(path));
    }
    if git(&repo, &["remote", "get-url", "origin"], GIT_SECS).code == 0 {
        let r = git(&repo, &["fetch", "origin"], FETCH_SECS);
        if r.code != 0 {
            return err(409, format!("git fetch failed before making its worktree: {}", last_line(&r.err)));
        }
    }
    let start = base;
    let r = git(&repo, &["worktree", "add", "--detach", &path, &start], GIT_SECS);
    if r.code != 0 {
        return err(409, format!("Couldn't make its worktree from {start}: {}", last_line(&r.err)));
    }
    record(app, t, &path, Some(&start))?;
    Ok(Some(path))
}

fn record(app: &App, t: &Row, path: &str, start: Option<&str>) -> Result<()> {
    app.db.tx(|| {
        let mut ctx = board::task_context(&board::get_task(app, t.id())?);
        let mut w = ctx.get("where").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        w.insert("worktree".into(), json!(path));
        ctx.insert("where".into(), Value::Object(w));
        ctx.insert("worktree_made".into(), json!(path));
        if let Some(s) = start {
            ctx.insert("worktree_base".into(), json!(s));
        }
        board::save_context(app, t.id(), &ctx, false)?;
        let text = match start {
            Some(s) => format!("Made its worktree from {s} at {path}"),
            None => format!("Starts in its worktree at {path}"),
        };
        board::log_event(app, t.id(), board::BOARD, "midna", &text)?;
        Ok(())
    })
}

fn finished(t: &Row) -> bool {
    t.b("failed") || !board::pr_still_open(t)
}

fn terminal_open(app: &App, t: &Row) -> Result<bool> {
    Ok(board::get_session(app, t.s("session_id"))?.map(|s| s.s("status") != Some("gone")).unwrap_or(false))
}

/// Removes the worktrees of finished tasks whose terminals are gone; keeps one with uncommitted
/// changes. Each is looked at once. Returns the tasks whose worktree was removed.
pub fn clean_up(app: &App) -> Result<Vec<i64>> {
    let mut removed = vec![];
    for t in app.db.q("SELECT * FROM tasks WHERE status = 'done' AND context LIKE '%worktree_made%'", p![])? {
        let ctx = board::task_context(&t);
        let Some(path) = ctx.s("worktree_made").filter(|p| !p.is_empty()).map(|s| s.to_string()) else { continue };
        if ctx.get("worktree_removed").is_some() || ctx.get("worktree_kept").is_some() || !finished(&t) || terminal_open(app, &t)? {
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
    Ok(removed)
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
    let mine = board::task_context(t);
    if root_of(mine.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str())).as_deref() == Some(root.as_str()) {
        return Ok(None);
    }
    for o in app.db.q("SELECT * FROM tasks WHERE id != ? AND context LIKE '%worktree%'", p![t.id()])? {
        if o.s("status") == Some("done") && !board::pr_still_open(&o) {
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
