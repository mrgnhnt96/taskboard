//! `tb slot claim --bring`: moves a task's uncommitted edits from the shared checkout into the slot it
//! claims. The files are the ones named, else the task's own in the hooks' edit log (`edits/<task>`)
//! that are still uncommitted; a file another working task also has uncommitted edits in is refused
//! by name. The edits go over as a binary patch against the shared checkout's HEAD (kept in
//! `brought/` in the data folder), applied in the slot with `git apply --3way`; only a clean apply
//! restores those files in the shared checkout, so a clash leaves both sides as they were.

use std::path::Path;

use serde_json::Value;

use crate::app::App;
use crate::util::*;
use crate::worktrees::{git, last_line, real, GIT_SECS};
use crate::{board, p};

/// What to bring and what stays behind, worked out before any slot is taken.
pub struct Bring {
    pub top: String,
    pub files: Vec<String>,
    /// The task's other uncommitted files in the shared checkout, left there.
    pub left: Vec<String>,
}

/// One uncommitted path in a checkout; `tracked` when git's index or HEAD has it.
struct Change {
    path: String,
    tracked: bool,
}

/// The files a body's `bring` names: none when it's absent, the defaults when it's an empty list.
pub fn asked(body: &Value) -> Result<Option<Vec<String>>> {
    match body.get("bring") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => Ok(Some(a.iter().filter_map(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())),
        Some(Value::String(s)) => Ok(Some(s.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())),
        Some(_) => err(400, "bring takes a list of files"),
    }
}

/// The shared checkout `t` works in.
fn shared_top(app: &App, t: &Row) -> Result<String> {
    match crate::worktrees::repo_of(app, t)?.filter(|r| Path::new(r).is_dir()) {
        Some(r) => Ok(real(&r)),
        None => err(409, format!("The board doesn't know the folder for the project “{}”, so it can't bring edits from it.", t.st("project"))),
    }
}

/// Works out what `asked` brings for `t`, refusing a file with nothing to bring or one another
/// working task also has uncommitted edits in. Nothing changes here.
pub fn plan(app: &App, t: &Row, asked: &[String]) -> Result<Bring> {
    let me = rf("task", t.id());
    let top = shared_top(app, t)?;
    let changed = changes(&top)?;
    let is_changed = |f: &str| changed.iter().any(|c| c.path == f);
    let mine: Vec<String> = edits_of(app, &me, &top).into_iter().filter(|f| is_changed(f)).collect();
    let files: Vec<String> = if asked.is_empty() {
        mine.clone()
    } else {
        let mut files = vec![];
        let mut idle = vec![];
        for a in asked {
            let rel = rel_in(&top, a);
            let hits: Vec<&Change> = changed.iter().filter(|c| c.path == rel || c.path.starts_with(&format!("{}/", rel.trim_end_matches('/')))).collect();
            if hits.is_empty() {
                idle.push(rel);
            }
            for c in hits {
                if !files.contains(&c.path) {
                    files.push(c.path.clone());
                }
            }
        }
        if !idle.is_empty() {
            return err(409, format!("{me} can't bring {}: nothing in the shared checkout ({top}) is uncommitted there. Nothing changed.", idle.join(", ")));
        }
        files
    };
    let mut clash: Vec<String> = vec![];
    for (task, file) in others(app, &me, &top)? {
        if files.contains(&file) && !clash.iter().any(|c| c.starts_with(&format!("{file} "))) {
            clash.push(format!("{file} ({task} has uncommitted edits in it too)"));
        }
    }
    if !clash.is_empty() {
        let them = if clash.len() == 1 { "it" } else { "them" };
        return err(409, format!("{me} can't bring {}. Leave {them} out and name your own files: tb slot claim --bring <files>. Nothing changed.", clash.join(", ")));
    }
    let left = mine.into_iter().filter(|f| !files.contains(f)).collect();
    Ok(Bring { top, files, left })
}

/// Where a slot for `t` starts when the shared checkout is on its branch with its commits: detached
/// at that branch's tip (a sha, since the branch can't be checked out twice), and the branch.
pub fn start_at(app: &App, t: &Row) -> Result<Option<(String, String)>> {
    let Ok(top) = shared_top(app, t) else { return Ok(None) };
    let ctx = board::task_context(t);
    let Some(own) = ctx.get("where").and_then(|w| w.get("branch")).and_then(|b| b.as_str()).filter(|b| !b.is_empty()) else { return Ok(None) };
    let on = git(&top, &["symbolic-ref", "--quiet", "--short", "HEAD"], GIT_SECS);
    if on.code != 0 || on.out.trim() != own || ["main", "master"].contains(&own) {
        return Ok(None);
    }
    let base = ["origin/main", "origin/master", "main", "master"].into_iter().find(|b| *b != own && git(&top, &["rev-parse", "--verify", "--quiet", b], GIT_SECS).code == 0);
    if let Some(b) = base {
        let ahead = git(&top, &["rev-list", "--count", &format!("{b}..HEAD")], GIT_SECS);
        if ahead.out.trim() == "0" {
            return Ok(None);
        }
    }
    let sha = git(&top, &["rev-parse", "HEAD"], GIT_SECS);
    Ok((sha.code == 0).then(|| (sha.out.trim().to_string(), own.to_string())))
}

/// Moves `b`'s files into the slot at `slot`: saves the patch, applies it there, then restores the
/// files in the shared checkout. The saved patch. On a clash the slot is put back as it was and the
/// shared checkout is left alone; the 409 names the files and the patch.
pub fn carry(app: &App, t: &Row, b: &Bring, slot: &str) -> Result<String> {
    let me = rf("task", t.id());
    let name = slot.rsplit('/').next().unwrap_or(slot);
    let dirty = git(slot, &["status", "--porcelain"], GIT_SECS);
    if dirty.code != 0 || !dirty.out.trim().is_empty() {
        return err(409, format!("{me} can't bring {} into {name} ({slot}): the slot has uncommitted changes of its own. Commit or discard them there first. Nothing changed.", b.files.join(", ")));
    }
    let patch = save_patch(app, t, b)?;
    let applied = git(slot, &["apply", "--3way", "--binary", &patch], GIT_SECS);
    if applied.code != 0 {
        git(slot, &["reset", "-q", "--hard"], GIT_SECS);
        git(slot, &["clean", "-fdq"], GIT_SECS);
        return err(
            409,
            format!(
                "{me} couldn't bring {} into {name}: they clash with what's there ({}). Nothing changed: the edits are still in the shared checkout ({}) and the slot was given back. The patch is saved at {patch}.",
                b.files.join(", "),
                last_line(&applied.err),
                b.top
            ),
        );
    }
    git(slot, &["reset", "-q"], GIT_SECS);
    restore(&b.top, &b.files)?;
    Ok(patch)
}

/// The patch of `b`'s files against the shared checkout's HEAD, new files included, saved in the
/// data folder.
fn save_patch(app: &App, t: &Row, b: &Bring) -> Result<String> {
    let dir = app.cfg.data.join("brought");
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::new(500, format!("Couldn't make {}: {e}", dir.display())))?;
    let stamp = now_iso().replace([':', '.'], "-");
    let path = dir.join(format!("{}-{stamp}.patch", rf("task", t.id())));
    let part = dir.join(format!("{}-{stamp}.part", rf("task", t.id())));
    let (part_s, path_s) = (part.to_string_lossy().to_string(), path.to_string_lossy().to_string());
    let changed = changes(&b.top)?;
    let tracked: Vec<&str> = b.files.iter().filter(|f| changed.iter().any(|c| &c.path == *f && c.tracked)).map(|f| f.as_str()).collect();
    let mut all: Vec<u8> = vec![];
    if !tracked.is_empty() {
        let mut args = vec!["diff", "--binary", "--output", part_s.as_str(), "HEAD", "--"];
        args.extend(tracked.iter());
        let r = git(&b.top, &args, GIT_SECS);
        if r.code != 0 {
            return err(409, format!("Couldn't take the patch of {}: {}. Nothing changed.", tracked.join(", "), last_line(&r.err)));
        }
        all.extend(std::fs::read(&part).unwrap_or_default());
    }
    for f in b.files.iter().filter(|f| !tracked.contains(&f.as_str())) {
        // A diff with --no-index exits 1 when the files differ, which they always do here.
        let r = git(&b.top, &["diff", "--binary", "--no-index", "--output", &part_s, "/dev/null", f], GIT_SECS);
        if r.code > 1 {
            return err(409, format!("Couldn't take the patch of {f}: {}. Nothing changed.", last_line(&r.err)));
        }
        all.extend(std::fs::read(&part).unwrap_or_default());
    }
    let _ = std::fs::remove_file(&part);
    std::fs::write(&path, &all).map_err(|e| ApiError::new(500, format!("Couldn't save the patch at {path_s}: {e}")))?;
    Ok(path_s)
}

/// Puts `files` back to HEAD in the shared checkout; a file HEAD doesn't have is deleted.
fn restore(top: &str, files: &[String]) -> Result<()> {
    let (old, new): (Vec<&String>, Vec<&String>) = files.iter().partition(|f| git(top, &["cat-file", "-e", &format!("HEAD:{f}")], GIT_SECS).code == 0);
    if !old.is_empty() {
        let mut args = vec!["restore", "--source=HEAD", "--staged", "--worktree", "--"];
        args.extend(old.iter().map(|f| f.as_str()));
        let r = git(top, &args, GIT_SECS);
        if r.code != 0 {
            return err(409, format!("The edits are in the slot, but restoring {} in the shared checkout failed: {}", old.iter().map(|f| f.as_str()).collect::<Vec<_>>().join(", "), last_line(&r.err)));
        }
    }
    for f in new {
        git(top, &["rm", "--cached", "-q", "--ignore-unmatch", "--", f], GIT_SECS);
        let _ = std::fs::remove_file(Path::new(top).join(f));
    }
    Ok(())
}

/// The uncommitted paths in a checkout (a rename gives both its paths).
fn changes(top: &str) -> Result<Vec<Change>> {
    let r = git(top, &["status", "--porcelain=v1", "-z", "--untracked-files=all"], GIT_SECS);
    if r.code != 0 {
        return err(409, format!("git status failed in {top}: {}", last_line(&r.err)));
    }
    let mut parts = r.out.split('\0');
    let mut list = vec![];
    while let Some(e) = parts.next() {
        if e.len() < 4 {
            continue;
        }
        let (x, y) = (e.as_bytes()[0], e.as_bytes()[1]);
        if matches!(x, b'R' | b'C') {
            if let Some(from) = parts.next().filter(|_| x == b'R') {
                list.push(Change { path: from.to_string(), tracked: true });
            }
        }
        list.push(Change { path: e[3..].to_string(), tracked: x != b'?' && y != b'?' });
    }
    Ok(list)
}

/// `file` from the checkout's top folder; an absolute path through the real path of its folder.
fn rel_in(top: &str, file: &str) -> String {
    let p = Path::new(file);
    if !p.is_absolute() {
        return file.trim_start_matches("./").to_string();
    }
    let real_parent = p.parent().map(|d| real(&d.to_string_lossy())).unwrap_or_default();
    let full = format!("{real_parent}/{}", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
    full.strip_prefix(&format!("{top}/")).map(str::to_string).unwrap_or_else(|| file.to_string())
}

/// The files in `task`'s edit log under `top`, from the checkout's top folder.
fn edits_of(app: &App, task: &str, top: &str) -> Vec<String> {
    let text = std::fs::read_to_string(app.cfg.data.join("edits").join(task)).unwrap_or_default();
    let mut out = vec![];
    for l in text.lines() {
        let Some((_, file)) = l.split_once('\t') else { continue };
        let rel = rel_in(top, file);
        if !Path::new(&rel).is_absolute() && !out.contains(&rel) {
            out.push(rel);
        }
    }
    out
}

/// Every (task, file) other working tasks have in their edit logs under `top`.
fn others(app: &App, me: &str, top: &str) -> Result<Vec<(String, String)>> {
    let Ok(dir) = std::fs::read_dir(app.cfg.data.join("edits")) else { return Ok(vec![]) };
    let mut out = vec![];
    for e in dir.flatten() {
        let task = e.file_name().to_string_lossy().to_string();
        let Some(id) = task.strip_prefix('T').and_then(|n| n.parse::<i64>().ok()).filter(|_| task != me) else { continue };
        if app.db.q1("SELECT id FROM tasks WHERE id = ? AND status IN ('working', 'needs', 'idle')", p![id])?.is_none() {
            continue;
        }
        out.extend(edits_of(app, &task, top).into_iter().map(|f| (task.clone(), f)));
    }
    out.sort();
    Ok(out)
}
