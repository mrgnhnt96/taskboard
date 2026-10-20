//! The worktree slot pool. A goal with a `worktree_base` starts each task in a slot: a reusable
//! worktree at `<repo>/.claude/worktrees/slot-<n>` that keeps its build output between tasks. A task
//! takes the free slot used last (the warmest build), else the pool grows by one up to `max_slots`;
//! at the max it waits, the way a lock wait shows. A task holds its slot (the lock `wt:slot-<n>`) until
//! it's done and its terminal is gone. `[worktrees]` in config.toml sizes the pool and ages idle slots,
//! and `tb project set` changes that for one project.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::worktrees::{git, last_line, GIT_SECS};
use crate::{board, p};

pub const WAIT: &str = "Waits for a worktree slot";
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

/// A repo's slots, the last taken first (`seq` counts takes; `used_at` is only to the second).
fn rows(app: &App, repo: &str) -> Result<Vec<Row>> {
    app.db.q("SELECT * FROM worktree_slots WHERE repo = ? ORDER BY seq DESC, n", p![repo])
}

/// The task holding a slot, or None when it's free: never taken, its task gone, or done with its
/// terminal closed.
fn holder(app: &App, s: &Row) -> Result<Option<Row>> {
    let Some(t) = board::find_task(app, s.i("task_id"))? else { return Ok(None) };
    if t.s("status") == Some("done") && !crate::worktrees::terminal_open(app, &t)? {
        return Ok(None);
    }
    Ok(Some(t))
}

fn slot_at(app: &App, path: &str) -> Result<Option<Row>> {
    let Some(root) = crate::worktrees::root_of(Some(path)) else { return Ok(None) };
    let root = crate::worktrees::real(&root);
    Ok(app.db.q("SELECT * FROM worktree_slots", p![])?.into_iter().find(|s| crate::worktrees::real(s.st("path").as_str()) == root))
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

/// Gives `t` a slot of `repo`, detached at `start`: the last used free one that's clean, else a new
/// one while the pool is under its max. Its folder.
pub fn claim(app: &App, t: &Row, repo: &str, start: &str) -> Result<String> {
    for s in rows(app, repo)? {
        if s.i("task_id") != Some(t.id()) && holder(app, &s)?.is_some() {
            continue;
        }
        let path = s.st("path");
        if !Path::new(&path).is_dir() {
            app.db.x("DELETE FROM worktree_slots WHERE id = ?", p![s.id()])?;
            continue;
        }
        let status = git(&path, &["status", "--porcelain"], GIT_SECS);
        if status.code != 0 || !status.out.trim().is_empty() {
            app.info(format!("worktrees: passed over {path}: it has uncommitted changes"));
            continue;
        }
        let r = crate::progress::waiting("git switch", crate::progress::WAITING_EVERY, || git(&path, &["switch", "--detach", start], GIT_SECS));
        if r.code != 0 {
            return err(409, format!("Couldn't switch its worktree {path} to {start}: {}", last_line(&r.err)));
        }
        let n = s.i("n").unwrap_or(0);
        crate::progress::stage(&format!("Switched slot-{n} to {start}"));
        take(app, t, s.id(), &path, &format!("Took worktree slot-{n} at {path}, detached at {start}"), start)?;
        return Ok(path);
    }
    let have = rows(app, repo)?;
    if have.len() as i64 >= config(app, t.s("project")).max_slots {
        return err(409, WAIT);
    }
    let taken: Vec<i64> = have.iter().filter_map(|s| s.i("n")).collect();
    let n = (1..).find(|n| !taken.contains(n) && !Path::new(&slot_path(repo, *n)).exists()).unwrap_or(1);
    let path = slot_path(repo, n);
    let r = crate::progress::waiting("git worktree add", crate::progress::WAITING_EVERY, || git(repo, &["worktree", "add", "--detach", &path, start], GIT_SECS));
    if r.code != 0 {
        return err(409, format!("Couldn't make its worktree from {start}: {}", last_line(&r.err)));
    }
    crate::progress::stage(&format!("Made slot-{n} from {start}"));
    let now = now_iso();
    let id = app.db.insert(
        "worktree_slots",
        vec![("repo", json!(repo)), ("n", json!(n)), ("path", json!(path)), ("project", t.v("project")), ("made_at", json!(now))],
    )?;
    take(app, t, id, &path, &format!("Made worktree slot-{n} from {start} at {path}"), start)?;
    Ok(path)
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
