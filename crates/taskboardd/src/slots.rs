//! The worktree slot pool. A goal with a `worktree_base` starts each task in a slot: a reusable
//! worktree at `<repo>/.claude/worktrees/slot-<n>` that keeps its build output between tasks. A task
//! takes the free slot used last (the warmest build), else the pool grows by one up to `max_slots`;
//! at the max it waits, the way a lock wait shows. A task holds its slot (the lock `wt:slot-<n>`) until
//! it submits, fails or parks (or, failing that, is done with its terminal gone); giving it back needs a
//! clean, pushed checkout. Review fixes later claim a slot again, its own if free, on the pushed branch.
//! `[worktrees]` in config.toml sizes the pool and ages idle slots, and `tb project set` changes that for
//! one project.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::worktrees::{git, last_line, GIT_SECS};
use crate::{board, p};

pub const WAIT: &str = "Waits for a worktree slot";
/// One claim at a time: the runner and API requests claim on their own threads, and a slot picked as
/// free must still be free when it's taken.
static CLAIMS: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
/// `tb project set --min-slots` and the rest: name → the `[worktrees]` keys it sets.
const SETTING: &str = "project_worktree_slots";
pub const KEYS: [&str; 5] = ["min_slots", "max_slots", "idle_trim_days", "idle_remove_days", "low_disk_gb"];

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct WorktreesConfig {
    /// The pool never shrinks below this many slots.
    pub min_slots: i64,
    /// The most slots a repo's pool grows to; past that a task waits for one to free up.
    pub max_slots: i64,
    /// Days a slot sits idle before its build output is cleared.
    pub idle_trim_days: i64,
    /// Days a slot sits idle before it's removed (never below min_slots).
    pub idle_remove_days: i64,
    /// Free disk in GB below which idle slots are cleared sooner.
    pub low_disk_gb: i64,
}

impl Default for WorktreesConfig {
    fn default() -> Self {
        WorktreesConfig { min_slots: 2, max_slots: 10, idle_trim_days: 2, idle_remove_days: 7, low_disk_gb: 50 }
    }
}

fn overrides(app: &App) -> Result<Row> {
    Ok(jloads_obj(app.db.get_setting(SETTING)?.as_deref()))
}

/// What `tb project set` put on a project, key by key.
pub fn set_on(app: &App, name: &str) -> Result<Row> {
    Ok(overrides(app)?.get(name).and_then(|v| v.as_object()).cloned().unwrap_or_default())
}

/// A project's pool settings: its own, else `[worktrees]`.
pub fn config(app: &App, project: Option<&str>) -> WorktreesConfig {
    let mut c = serde_json::to_value(&app.cfg.worktrees).unwrap_or(Value::Null);
    if let Some(set) = project.filter(|n| !n.is_empty()).and_then(|n| set_on(app, n).ok()) {
        for (k, v) in set {
            c[k.as_str()] = v;
        }
    }
    serde_json::from_value(c).unwrap_or_else(|_| app.cfg.worktrees.clone())
}

fn range(key: &str) -> (i64, i64, &'static str) {
    match key {
        "min_slots" => (0, 50, "a count from 0 to 50"),
        "max_slots" => (1, 50, "a count from 1 to 50"),
        "low_disk_gb" => (0, 100_000, "a number of GB"),
        _ => (1, 365, "a number of days from 1 to 365"),
    }
}

/// Sets a project's pool settings from a body's `KEYS` (null or "default": back to config.toml's).
/// False when the body names none.
pub fn set(app: &App, name: &str, body: &Value) -> Result<bool> {
    if !KEYS.iter().any(|k| body.get(*k).is_some()) {
        return Ok(false);
    }
    let mut mine = set_on(app, name)?;
    for k in KEYS {
        let Some(v) = body.get(k) else { continue };
        if v.is_null() || v.as_str().is_some_and(|s| s.trim().eq_ignore_ascii_case("default")) {
            mine.remove(k);
            continue;
        }
        let (lo, hi, what) = range(k);
        match v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())) {
            Some(n) if (lo..=hi).contains(&n) => mine.insert(k.into(), json!(n)),
            _ => return err(400, format!("{k} is {what}, or default.")),
        };
    }
    let mut all = overrides(app)?;
    if mine.is_empty() {
        all.remove(name);
    } else {
        all.insert(name.into(), Value::Object(mine));
    }
    app.db.set_setting(SETTING, Some(&jdumps(&Value::Object(all))))?;
    let c = config(app, Some(name));
    if c.min_slots > c.max_slots {
        return err(400, format!("min_slots ({}) can't be more than max_slots ({}).", c.min_slots, c.max_slots));
    }
    if c.idle_trim_days > c.idle_remove_days {
        return err(400, format!("idle_trim_days ({}) can't be more than idle_remove_days ({}).", c.idle_trim_days, c.idle_remove_days));
    }
    Ok(true)
}

/// A project's pool settings for `GET /projects`, and the ones it sets itself.
pub fn describe(app: &App, name: &str) -> Result<Value> {
    let mut d = serde_json::to_value(config(app, Some(name))).unwrap_or(Value::Null);
    d["set"] = Value::Object(set_on(app, name)?);
    Ok(d)
}

/// Is this path in a pool slot (`…/.claude/worktrees/slot-<n>`)?
pub fn is_slot(path: &str) -> bool {
    crate::worktrees::root_of(Some(path))
        .and_then(|r| r.rsplit('/').next().and_then(|n| n.strip_prefix("slot-")).map(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())))
        .unwrap_or(false)
}

pub fn lock_name(n: i64) -> String {
    format!("wt:slot-{n}")
}

fn slot_path(repo: &str, n: i64) -> String {
    format!("{}/.claude/worktrees/slot-{n}", repo.trim_end_matches('/'))
}

/// Is `path` a worktree git knows, with its own `.git`? A folder without one (its worktree removed
/// under it, then made again by a tool writing there) or one git has pruned walks up to the shared
/// checkout, so every git command run in it would hit that instead.
pub(crate) fn live(path: &str) -> bool {
    if !Path::new(path).join(".git").exists() {
        return false;
    }
    let r = git(path, &["rev-parse", "--show-toplevel"], GIT_SECS);
    r.code == 0 && crate::worktrees::real(r.out.trim()) == crate::worktrees::real(path)
}

/// Leftovers a tool writes into a slot it runs in, safe to drop when the worktree under them is gone.
const LEFTOVERS: [&str; 1] = [".review"];

/// A slot folder holding nothing but `LEFTOVERS`.
fn only_leftovers(path: &str) -> bool {
    std::fs::read_dir(path).is_ok_and(|d| d.flatten().all(|e| LEFTOVERS.contains(&e.file_name().to_string_lossy().as_ref())))
}

/// Adds the worktree at `path`, detached at `start`, and checks git sees it as one.
fn add(repo: &str, path: &str, start: &str) -> Result<()> {
    let r = crate::progress::waiting("git worktree add", crate::progress::WAITING_EVERY, || git(repo, &["worktree", "add", "--detach", path, start], GIT_SECS));
    if r.code != 0 {
        return err(409, format!("Couldn't make its worktree from {start}: {}", last_line(&r.err)));
    }
    if !live(path) {
        return err(409, format!("Made its worktree at {path}, but git doesn't see it as a worktree of {repo}; run git worktree list there."));
    }
    Ok(())
}

/// A slot whose folder isn't a live worktree: made again in place when it's gone or holds only
/// leftovers, else dropped from the pool with its folder left for a look. True when it's live again.
fn repair(app: &App, t: &Row, s: &Row, repo: &str, start: &str) -> Result<bool> {
    let (path, n) = (s.st("path"), s.i("n").unwrap_or(0));
    git(repo, &["worktree", "prune"], GIT_SECS);
    let gone = !Path::new(&path).exists();
    if !gone && !only_leftovers(&path) {
        app.db.x("DELETE FROM worktree_slots WHERE id = ?", p![s.id()])?;
        let why = format!("Dropped worktree slot-{n} from the pool: {path} isn't a git worktree any more and holds more than leftovers, so it's left as it is");
        app.info(format!("worktrees: {why}"));
        board::log_event(app, t.id(), board::BOARD, "midna", &why)?;
        return Ok(false);
    }
    if !gone {
        std::fs::remove_dir_all(&path).map_err(|e| ApiError::new(409, format!("Couldn't clear the broken worktree slot-{n} at {path}: {e}")))?;
    }
    add(repo, &path, start)?;
    crate::progress::stage(&format!("Made slot-{n} again from {start}"));
    board::log_event(app, t.id(), board::BOARD, "midna", &format!("Made worktree slot-{n} again at {path}: it wasn't a git worktree any more"))?;
    Ok(true)
}

/// A repo's slots, the last taken first (`seq` counts takes; `used_at` is only to the second).
fn rows(app: &App, repo: &str) -> Result<Vec<Row>> {
    app.db.q("SELECT * FROM worktree_slots WHERE repo = ? ORDER BY seq DESC, n", p![repo])
}

/// The task holding a slot, or None when it's free: never taken, given back, its task gone, or done
/// with its terminal closed and no PR visit running.
fn holder(app: &App, s: &Row) -> Result<Option<Row>> {
    let Some(t) = board::find_task(app, s.i("task_id"))? else { return Ok(None) };
    if t.s("status") == Some("done") && !crate::worktrees::terminal_open(app, &t)? && !crate::prflow::visit_live(app, &t)? {
        return Ok(None);
    }
    Ok(Some(t))
}

fn slot_at(app: &App, path: &str) -> Result<Option<Row>> {
    let Some(root) = crate::worktrees::root_of(Some(path)) else { return Ok(None) };
    let root = crate::worktrees::real(&root);
    Ok(app.db.q("SELECT * FROM worktree_slots", p![])?.into_iter().find(|s| crate::worktrees::real(s.st("path").as_str()) == root))
}

/// The live slot `t` holds, its latest take first. A restart goes back to it whatever folder the task
/// last reported from (a subfolder of the slot has no `.git`), uncommitted work and all.
pub fn held(app: &App, t: &Row) -> Result<Option<Row>> {
    let mine = app.db.q("SELECT * FROM worktree_slots WHERE task_id = ? ORDER BY seq DESC, n", p![t.id()])?;
    Ok(mine.into_iter().find(|s| live(s.st("path").as_str())))
}

/// Does `t` hold the slot `path` is in?
pub fn holds(app: &App, t: &Row, path: &str) -> Result<bool> {
    Ok(slot_at(app, path)?.is_some_and(|s| s.i("task_id") == Some(t.id())))
}

/// Another task holding the slot `path` is in.
pub fn held_by_other(app: &App, path: &str, t: &Row) -> Result<Option<Row>> {
    let Some(s) = slot_at(app, path)? else { return Ok(None) };
    Ok(holder(app, &s)?.filter(|h| h.id() != t.id()))
}

/// Why a task that needs a slot can't start yet: the pool is at its max and none is free.
pub fn blocker(app: &App, t: &Row) -> Result<Option<String>> {
    if t.s("pickup") == Some("attach") || crate::worktrees::made(app, t)?.is_some() {
        return Ok(None);
    }
    let Some(repo) = crate::worktrees::slot_repo(app, t)? else { return Ok(None) };
    let slots = rows(app, &repo)?;
    // Its own slot, even one to make again, is no wait.
    if slots.iter().any(|s| s.i("task_id") == Some(t.id())) {
        return Ok(None);
    }
    if (slots.len() as i64) < config(app, t.s("project")).max_slots {
        return Ok(None);
    }
    for s in &slots {
        if holder(app, s)?.is_none() {
            return Ok(None);
        }
    }
    Ok(Some(WAIT.into()))
}

/// The held slots as locks, for `GET /locks`.
pub fn held_locks(app: &App) -> Result<Vec<Value>> {
    let mut out = vec![];
    for s in app.db.q("SELECT * FROM worktree_slots WHERE task_id IS NOT NULL ORDER BY project, n", p![])? {
        if let Some(t) = holder(app, &s)? {
            out.push(json!({"name": lock_name(s.i("n").unwrap_or(0)), "project": s.v("project"), "path": s.v("path"),
                            "held_by": rf("task", t.id()), "tasks": []}));
        }
    }
    Ok(out)
}

/// Gives `t` a slot of `repo`: its own from before when that's free, else the last used free one
/// that's clean, else a new one while the pool is under its max. Detached at `start`, or on the branch
/// it gave its slot back with. Its folder.
pub fn claim(app: &App, t: &Row, repo: &str, start: &str) -> Result<String> {
    let ctx = board::task_context(t);
    claim_on(app, t, repo, start, ctx.s("worktree_branch"))
}

fn claim_on(app: &App, t: &Row, repo: &str, start: &str, branch: Option<&str>) -> Result<String> {
    let _one = CLAIMS.lock();
    // One slot per task: the one it holds already, as it left it.
    if let Some(s) = held(app, t)?.filter(|s| s.s("repo") == Some(repo)) {
        let path = s.st("path");
        crate::dispatch::clear_alert_key(app, &dirty_key(&path))?;
        take(app, t, s.id(), &path, &format!("Back in its worktree slot-{} at {path}", s.i0("n")), &ctx_base(t, start))?;
        return Ok(path);
    }
    let mut slots = rows(app, repo)?;
    // A conversation resumes only in the folder it ran in.
    if let Some(before) = board::task_context(t).s("worktree_slot") {
        slots.sort_by_key(|s| s.s("path") != Some(before));
    }
    for s in slots {
        if s.i("task_id") != Some(t.id()) && holder(app, &s)?.is_some() {
            continue;
        }
        let path = s.st("path");
        if !live(&path) && !repair(app, t, &s, repo, start)? {
            continue;
        }
        let status = git(&path, &["status", "--porcelain"], GIT_SECS);
        if status.code != 0 || !status.out.trim().is_empty() {
            app.info(format!("worktrees: passed over {path}: it has uncommitted changes"));
            dirty_alert(app, &s)?;
            continue;
        }
        crate::dispatch::clear_alert_key(app, &dirty_key(&path))?;
        let n = s.i("n").unwrap_or(0);
        let at = check_out(&path, start, branch)?;
        crate::progress::stage(&format!("Switched slot-{n} to {at}"));
        take(app, t, s.id(), &path, &format!("Took worktree slot-{n} at {path}, {}", at_text(&at, start)), start)?;
        return Ok(path);
    }
    let have = rows(app, repo)?;
    if have.len() as i64 >= config(app, t.s("project")).max_slots {
        return err(409, WAIT);
    }
    let taken: Vec<i64> = have.iter().filter_map(|s| s.i("n")).collect();
    let n = (1..).find(|n| !taken.contains(n) && !Path::new(&slot_path(repo, *n)).exists()).unwrap_or(1);
    let path = slot_path(repo, n);
    add(repo, &path, start)?;
    crate::progress::stage(&format!("Made slot-{n} from {start}"));
    let now = now_iso();
    let id = app.db.insert(
        "worktree_slots",
        vec![("repo", json!(repo)), ("n", json!(n)), ("path", json!(path)), ("project", t.v("project")), ("made_at", json!(now))],
    )?;
    let at = match branch {
        Some(_) => check_out(&path, start, branch)?,
        None => start.to_string(),
    };
    let text = if at == start { format!("Made worktree slot-{n} from {start} at {path}") } else { format!("Made worktree slot-{n} at {path}, on its branch {at}") };
    take(app, t, id, &path, &text, start)?;
    Ok(path)
}

/// The base a slot it's back in was taken at, so `take` doesn't move it to `start`.
fn ctx_base(t: &Row, start: &str) -> String {
    board::task_context(t).s("worktree_base").unwrap_or(start).to_string()
}

fn dirty_key(path: &str) -> String {
    format!("slot-dirty:{path}")
}

/// A free slot left with uncommitted changes never frees up by itself, so it's the owner's to clear:
/// one alert for the slot, not a failed start for each task waiting on the pool.
fn dirty_alert(app: &App, s: &Row) -> Result<()> {
    let path = s.st("path");
    let text = format!("Worktree slot-{} is free but has uncommitted changes, so no task can take it: {path}", s.i0("n"));
    crate::dispatch::add_alert_keyed(app, &text, None, None, &dirty_key(&path))
}

fn at_text(at: &str, start: &str) -> String {
    if at == start {
        format!("detached at {start}")
    } else {
        format!("on its branch {at}")
    }
}

/// Puts a slot on `branch` (a task's pushed branch, which every worktree of the repo shares), else
/// detached at `start`. Where it is.
fn check_out(path: &str, start: &str, branch: Option<&str>) -> Result<String> {
    if let Some(b) = branch.filter(|b| !b.is_empty()) {
        let local = git(path, &["switch", b], GIT_SECS);
        if local.code == 0 || git(path, &["switch", "-c", b, "--track", &format!("origin/{b}")], GIT_SECS).code == 0 {
            return Ok(b.to_string());
        }
    }
    let r = crate::progress::waiting("git switch", crate::progress::WAITING_EVERY, || git(path, &["switch", "--detach", start], GIT_SECS));
    if r.code != 0 {
        return err(409, format!("Couldn't switch its worktree {path} to {start}: {}", last_line(&r.err)));
    }
    Ok(start.to_string())
}

/// Review fixes, a rebase or a failing check on a task that gave its slot back: claims a slot again
/// (its own when free) on its pushed branch. The slot it holds already; None when it never worked in
/// a slot; a 409 `WAIT` while the pool is full.
pub fn reclaim(app: &App, t: &Row) -> Result<Option<String>> {
    let ctx = board::task_context(t);
    let Some(before) = ctx.s("worktree_slot").filter(|p| is_slot(p)) else { return Ok(None) };
    if let Some(s) = held(app, t)? {
        return Ok(Some(s.st("path")));
    }
    let Some((repo, _)) = before.rsplit_once("/.claude/worktrees/") else { return Ok(None) };
    if !Path::new(repo).is_dir() {
        return Ok(None);
    }
    let start = ctx.s("worktree_base").unwrap_or("HEAD").to_string();
    let branch = ctx.s("worktree_branch").or_else(|| ctx.get("where").and_then(|w| w.get("branch")).and_then(|b| b.as_str())).map(|b| b.to_string());
    claim_on(app, t, repo, &start, branch.as_deref()).map(Some)
}

/// Why `t` can't give its slot back yet: uncommitted changes, or commits that aren't pushed. None
/// when it can, or holds none.
pub fn release_refusal(app: &App, t: &Row) -> Result<Option<String>> {
    let Some(s) = app.db.q1("SELECT * FROM worktree_slots WHERE task_id = ?", p![t.id()])? else { return Ok(None) };
    let path = s.st("path");
    if !live(&path) {
        return Ok(None);
    }
    let me = rf("task", t.id());
    let n = s.i("n").unwrap_or(0);
    let status = git(&path, &["status", "--porcelain"], GIT_SECS);
    if status.code != 0 || !status.out.trim().is_empty() {
        return Ok(Some(format!(
            "{me} can't give back its worktree slot-{n} ({path}): it has uncommitted changes. Commit and push them, or discard them, then run this again."
        )));
    }
    let k = unpushed(&path);
    if k > 0 {
        let on = branch_of(&path).map(|b| format!(" on {b}")).unwrap_or_default();
        return Ok(Some(format!(
            "{me} can't give back its worktree slot-{n} ({path}): {k} commit{}{on} {} pushed. Push {} (git push), then run this again.",
            if k == 1 { "" } else { "s" },
            if k == 1 { "isn't" } else { "aren't" },
            if k == 1 { "it" } else { "them" }
        )));
    }
    Ok(None)
}

fn branch_of(path: &str) -> Option<String> {
    let r = git(path, &["symbolic-ref", "--quiet", "--short", "HEAD"], GIT_SECS);
    (r.code == 0).then(|| r.out.trim().to_string()).filter(|b| !b.is_empty())
}

/// Commits in a slot that only it has. Without a remote, a branch keeps its commits after the slot
/// lets go of it; only a detached HEAD's would be lost.
fn unpushed(path: &str) -> i64 {
    let remote = !git(path, &["remote"], GIT_SECS).out.trim().is_empty();
    if !remote && branch_of(path).is_some() {
        return 0;
    }
    let r = git(path, &["rev-list", "--count", "HEAD", "--not", if remote { "--remotes" } else { "--branches" }], GIT_SECS);
    r.out.trim().parse().unwrap_or(0)
}

/// Gives back the slot `t` holds: detached at its base again and free for the next task, its branch
/// kept for review fixes. Refuses (409) while it has uncommitted changes or unpushed commits. False
/// when it held none.
pub fn release(app: &App, t: &Row) -> Result<bool> {
    if let Some(why) = release_refusal(app, t)? {
        return err(409, why);
    }
    give_back(app, t)
}

/// `release` that keeps the slot, and says why in the task's log, when it can't be given back.
pub fn release_if_clean(app: &App, t: &Row) -> Result<bool> {
    if let Some(why) = release_refusal(app, t)? {
        board::log_event(app, t.id(), board::BOARD, "midna", &format!("Kept its worktree slot: {}", why.split(": ").nth(1).unwrap_or(&why)))?;
        return Ok(false);
    }
    give_back(app, t)
}

pub(crate) fn give_back(app: &App, t: &Row) -> Result<bool> {
    let Some(s) = app.db.q1("SELECT * FROM worktree_slots WHERE task_id = ?", p![t.id()])? else { return Ok(false) };
    let (path, n) = (s.st("path"), s.i("n").unwrap_or(0));
    let ctx = board::task_context(&board::get_task(app, t.id())?);
    let mut branch = None;
    // A dead slot's git commands would switch the shared checkout instead.
    if live(&path) {
        branch = branch_of(&path);
        let base = ctx.s("worktree_base").unwrap_or("HEAD");
        // Off its branch either way, so a reclaimed slot elsewhere can check it out.
        if git(&path, &["switch", "--detach", base], GIT_SECS).code != 0 {
            git(&path, &["switch", "--detach"], GIT_SECS);
        }
    }
    app.db.tx(|| {
        app.db.x("UPDATE worktree_slots SET task_id = NULL, used_at = ? WHERE id = ?", p![now_iso(), s.id()])?;
        let mut ctx = board::task_context(&board::get_task(app, t.id())?);
        if let Some(b) = &branch {
            ctx.insert("worktree_branch".into(), json!(b));
        }
        board::save_context(app, t.id(), &ctx, false)?;
        let kept = branch.as_ref().map(|b| format!("; its branch {b} is kept for review fixes")).unwrap_or_default();
        board::log_event(app, t.id(), board::BOARD, "midna", &format!("Gave back worktree slot-{n}{kept}"))?;
        Ok(true)
    })
}

/// Every slot for `GET /slots`: who holds it, who waits for one, when it was last used and its size.
pub fn overview(app: &App, project: Option<&str>) -> Result<Value> {
    let project = project.filter(|p| !p.is_empty());
    let all = app.db.q("SELECT * FROM worktree_slots ORDER BY project, repo, n", p![])?;
    let mut waiting: Vec<(String, String)> = vec![];
    for t in app.db.q("SELECT * FROM tasks WHERE status = 'queued' ORDER BY id", p![])? {
        if blocker(app, &t)?.is_some() {
            if let Some(repo) = crate::worktrees::slot_repo(app, &t)? {
                waiting.push((repo, rf("task", t.id())));
            }
        }
    }
    let mut out = vec![];
    for s in all.iter().filter(|s| project.is_none() || s.s("project") == project) {
        let repo = s.st("repo");
        let held = holder(app, s)?;
        out.push(json!({
            "name": format!("slot-{}", s.i0("n")), "n": s.v("n"), "project": s.v("project"), "repo": repo, "path": s.v("path"),
            "held_by": held.as_ref().map(|t| rf("task", t.id())), "title": held.as_ref().map(|t| t.v("title")),
            "used_at": s.v("used_at"), "made_at": s.v("made_at"), "size_kb": size_kb(&s.st("path")),
            "waiting": waiting.iter().filter(|(r, _)| *r == repo).map(|(_, t)| t.clone()).collect::<Vec<_>>(),
        }));
    }
    Ok(json!({"slots": out}))
}

fn size_kb(path: &str) -> Option<i64> {
    let o = crate::proc::run(Path::new("/usr/bin/du"), &["-sk".to_string(), path.to_string()], None, 20.0).ok()?;
    o.stdout.split_whitespace().next()?.parse().ok()
}

fn take(app: &App, t: &Row, id: i64, path: &str, text: &str, start: &str) -> Result<()> {
    app.db.tx(|| {
        app.db.x(
            "UPDATE worktree_slots SET task_id = ?, used_at = ?, seq = (SELECT COALESCE(MAX(seq), 0) + 1 FROM worktree_slots) WHERE id = ?",
            p![t.id(), now_iso(), id],
        )?;
        let mut ctx = board::task_context(&board::get_task(app, t.id())?);
        let mut w = ctx.get("where").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        w.insert("worktree".into(), json!(path));
        ctx.insert("where".into(), Value::Object(w));
        ctx.insert("worktree_slot".into(), json!(path));
        ctx.insert("worktree_base".into(), json!(start));
        board::save_context(app, t.id(), &ctx, false)?;
        board::log_event(app, t.id(), board::BOARD, "midna", text)?;
        Ok(())
    })
}
