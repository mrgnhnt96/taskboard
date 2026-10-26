//! Ageing of the worktree slot pool (`slots.rs`), on the board's clean-up timer. A free slot idle past
//! `idle_trim_days` has its `target/` deleted (the checkout stays); one idle past `idle_remove_days` is
//! removed with `git worktree remove`, never taking its repo's pool below `min_slots`. When free disk
//! drops under `low_disk_gb`, the longest-idle free slots are trimmed first, before their timers. A
//! claimed slot, or one with uncommitted changes, is never touched.

use std::path::Path;

use crate::app::App;
use crate::slots::{self, WorktreesConfig};
use crate::util::*;
use crate::worktrees::{git, last_line, GIT_SECS};
use crate::{board, p};

const DAY: f64 = 86_400.0;
const GB: f64 = 1_073_741_824.0;

/// Free disk in GB where `path` is.
fn free_gb(path: &str) -> Option<f64> {
    let c = std::ffi::CString::new(path).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some(st.f_bavail as f64 * st.f_frsize as f64 / GB)
}

/// Is no task holding this slot: never taken, its task gone, or done with its terminal closed?
fn is_free(app: &App, s: &Row) -> Result<bool> {
    let Some(t) = board::find_task(app, s.i("task_id"))? else { return Ok(true) };
    Ok(t.s("status") == Some("done") && !crate::worktrees::terminal_open(app, &t)?)
}

/// When a free slot went idle: the later of its last take and its last holder's last change.
fn idle_since(app: &App, s: &Row) -> f64 {
    let held = s.s("used_at").and_then(parse_iso).unwrap_or(0.0);
    let released = board::find_task(app, s.i("task_id")).ok().flatten().and_then(|t| t.s("updated_at").and_then(parse_iso)).unwrap_or(0.0);
    let made = s.s("made_at").and_then(parse_iso).unwrap_or(0.0);
    held.max(released).max(made)
}

fn clean(path: &str) -> bool {
    let r = git(path, &["status", "--porcelain", "--", ".", ":(exclude)target"], GIT_SECS);
    r.code == 0 && r.out.trim().is_empty()
}

fn trim(app: &App, path: &str, why: &str) {
    let target = Path::new(path).join("target");
    if !target.is_dir() {
        return;
    }
    match std::fs::remove_dir_all(&target) {
        Ok(()) => {
            slots::resized(path);
            app.info(format!("worktrees: cleared the build output of {path}: {why}"))
        }
        Err(e) => app.info(format!("worktrees: couldn't clear the build output of {path}: {e}")),
    }
}

/// Ages every repo's free slots. Returns the paths of the slots removed.
pub fn sweep(app: &App) -> Result<Vec<String>> {
    sweep_at(app, now_ts(), &free_gb)
}

pub fn sweep_at(app: &App, now: f64, disk: &dyn Fn(&str) -> Option<f64>) -> Result<Vec<String>> {
    let mut removed = vec![];
    for repo in app.db.q("SELECT DISTINCT repo FROM worktree_slots", p![])? {
        let repo = repo.st("repo");
        let mut total = app.db.q("SELECT COUNT(*) AS n FROM worktree_slots WHERE repo = ?", p![repo])?.first().and_then(|r| r.i("n")).unwrap_or(0);
        let mut free: Vec<(f64, Row)> = vec![];
        for s in app.db.q("SELECT * FROM worktree_slots WHERE repo = ?", p![repo])? {
            if is_free(app, &s)? {
                free.push((idle_since(app, &s), s));
            }
        }
        free.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut kept = vec![];
        for (since, s) in free {
            let cfg: WorktreesConfig = slots::config(app, s.s("project"));
            let path = s.st("path");
            let idle = (now - since) / DAY;
            if !Path::new(&path).is_dir() {
                app.db.x("DELETE FROM worktree_slots WHERE id = ?", p![s.id()])?;
                total -= 1;
                continue;
            }
            if idle >= cfg.idle_remove_days as f64 && total > cfg.min_slots && clean(&path) {
                trim(app, &path, &format!("idle {idle:.0} days"));
                let r = git(&repo, &["worktree", "remove", &path], GIT_SECS);
                if r.code == 0 {
                    app.db.x("DELETE FROM worktree_slots WHERE id = ?", p![s.id()])?;
                    app.info(format!("worktrees: removed {path}: idle {idle:.0} days"));
                    total -= 1;
                    removed.push(path);
                    continue;
                }
                app.info(format!("worktrees: couldn't remove {path}: {}", last_line(&r.err)));
            }
            if idle >= cfg.idle_trim_days as f64 && clean(&path) {
                trim(app, &path, &format!("idle {idle:.0} days"));
            }
            kept.push((s, cfg));
        }
        for (s, cfg) in kept {
            let path = s.st("path");
            if disk(&repo).is_some_and(|gb| gb < cfg.low_disk_gb as f64) && Path::new(&path).join("target").is_dir() && clean(&path) {
                trim(app, &path, "free disk is low");
            }
        }
    }
    Ok(removed)
}
