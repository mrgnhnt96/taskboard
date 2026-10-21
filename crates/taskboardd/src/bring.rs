//! `tb slot claim --bring`: moves a task's uncommitted edits from the shared checkout into the slot it
//! claims. The files are the ones named, else the task's own in the hooks' edit log (`edits/<task>`)
//! that are still uncommitted. A file only this task edits goes whole. One another working task also
//! edits is split by hunk (see `hunks`): the slot gets HEAD with this task's hunks, the shared
//! checkout keeps HEAD with everyone else's. A hunk two tasks made, or a file another task's edits
//! can't be told apart in, is refused by name; hunks no log has are listed with ids to pick from
//! (`bring_hunk`, `<file>:<id>`). The edits go over as a binary patch against the shared checkout's
//! HEAD (kept in `brought/` in the data folder), applied in the slot with `git apply --3way`; only a
//! clean apply changes the shared checkout, so a clash leaves both sides as they were.

use std::collections::HashSet;
use std::path::Path;

use serde_json::Value;

use crate::app::App;
use crate::hunks::{self, Marks};
use crate::util::*;
use crate::worktrees::{git, last_line, real, Git, GIT_SECS};
use crate::{board, p};

/// What to bring and what stays behind, worked out before any slot is taken.
pub struct Bring {
    pub top: String,
    pub files: Vec<String>,
    /// The task's other uncommitted files in the shared checkout, left there.
    pub left: Vec<String>,
    /// The files in `files` that are split by hunk.
    pub parts: Vec<Part>,
    /// Hunks no task's log has that stay in the shared checkout, as `<file>:<id> (line …)`.
    pub left_hunks: Vec<String>,
}

/// A file split by hunk: its working copy when planned, and what the slot and the shared checkout
/// each get.
pub struct Part {
    pub file: String,
    work: String,
    slot: String,
    shared: String,
    mode: String,
}

/// A body's list of strings, or comma-separated string, under `key`.
fn list(body: &Value, key: &str) -> Result<Option<Vec<String>>> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => Ok(Some(a.iter().filter_map(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())),
        Some(Value::String(s)) => Ok(Some(s.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())),
        Some(_) => err(400, format!("{key} takes a list")),
    }
}

/// The files a body's `bring` names: none when it's absent (and no hunk is picked), the defaults
/// when it's an empty list.
pub fn asked(body: &Value) -> Result<Option<Vec<String>>> {
    let files = list(body, "bring")?;
    Ok(if files.is_none() && !picks(body)?.is_empty() { Some(vec![]) } else { files })
}

/// The hunks a body's `bring_hunk` picks, as `<file>:<id>`, or `none`.
pub fn picks(body: &Value) -> Result<Vec<String>> {
    Ok(list(body, "bring_hunk")?.unwrap_or_default())
}

/// The shared checkout `t` works in.
fn shared_top(app: &App, t: &Row) -> Result<String> {
    match crate::worktrees::repo_of(app, t)?.filter(|r| Path::new(r).is_dir()) {
        Some(r) => Ok(real(&r)),
        None => err(409, format!("The board doesn't know the folder for the project “{}”, so it can't bring edits from it.", t.st("project"))),
    }
}

/// Works out what `asked` and `picks` bring for `t`, refusing a file with nothing to bring, a hunk
/// another task made too, and (until some are picked) hunks no task's log has. Nothing changes here.
pub fn plan(app: &App, t: &Row, asked: &[String], picks: &[String]) -> Result<Bring> {
    let me = rf("task", t.id());
    let top = shared_top(app, t)?;
    let changed = changes(&top)?;
    let is_changed = |f: &str| changed.iter().any(|c| c == f);
    let mine: Vec<String> = edits_of(app, &me, &top).into_iter().filter(|f| is_changed(f)).collect();
    let mut files: Vec<String> = if asked.is_empty() {
        mine.clone()
    } else {
        let mut files = vec![];
        let mut idle = vec![];
        for a in asked {
            let rel = rel_in(&top, a);
            let hits: Vec<&String> = changed.iter().filter(|c| **c == rel || c.starts_with(&format!("{}/", rel.trim_end_matches('/')))).collect();
            if hits.is_empty() {
                idle.push(rel);
            }
            for c in hits {
                if !files.contains(c) {
                    files.push(c.clone());
                }
            }
        }
        if !idle.is_empty() {
            return err(409, format!("{me} can't bring {}: nothing in the shared checkout ({top}) is uncommitted there. Nothing changed.", idle.join(", ")));
        }
        files
    };
    let mut picked: Vec<(String, String)> = vec![];
    for p in picks.iter().filter(|p| *p != "none") {
        let Some((f, id)) = p.rsplit_once(':') else { return err(400, format!("--bring-hunk takes <file>:<id>, or none, not “{p}”.")) };
        let rel = rel_in(&top, f);
        if !is_changed(&rel) {
            return err(409, format!("{me} can't bring {p}: {rel} has nothing uncommitted in the shared checkout ({top}). Nothing changed."));
        }
        if !files.contains(&rel) {
            files.push(rel.clone());
        }
        picked.push((rel, id.to_string()));
    }
    let others = others(app, &me, &top)?;
    let my_log = hunk_log(app, &me);
    let (mut parts, mut clash, mut unpicked, mut left_hunks) = (vec![], vec![], vec![], vec![]);
    for f in &files {
        let theirs: Vec<(String, String)> = others.iter().filter(|(_, g)| g == f).map(|(task, _)| (task.clone(), hunk_log(app, task))).collect();
        let here: Vec<&str> = picked.iter().filter(|(g, _)| g == f).map(|(_, id)| id.as_str()).collect();
        if theirs.is_empty() && here.is_empty() {
            continue;
        }
        match split(&top, f, &me, &my_log, &theirs, &here) {
            Err(why) => clash.push(why),
            Ok((part, listed)) => {
                (if picks.is_empty() { &mut unpicked } else { &mut left_hunks }).extend(listed);
                parts.extend(part);
            }
        }
    }
    if !clash.is_empty() {
        let them = if clash.len() == 1 { "it" } else { "them" };
        return err(409, format!("{me} can't bring {}. Leave {them} out and name your own files: tb slot claim --bring <files>. Nothing changed.", clash.join(", ")));
    }
    if !unpicked.is_empty() {
        return err(
            409,
            format!(
                "{me} shares files with other tasks, and these hunks in them aren't in any task's edit log (shell edits, or edits from before the task):\n  {}\nPick yours with --bring-hunk <file>:<id> (once for each), or leave them all with --bring-hunk none. Nothing changed.",
                unpicked.join("\n  ")
            ),
        );
    }
    // A file split down to nothing of this task's stays behind.
    files.retain(|f| (!others.iter().any(|(_, g)| g == f) && !picked.iter().any(|(g, _)| g == f)) || parts.iter().any(|p: &Part| &p.file == f));
    let left = mine.into_iter().filter(|f| !files.contains(f)).collect();
    Ok(Bring { top, files, left, parts, left_hunks })
}

/// Splits `f` by hunk between `me` (its own log, `mine`, and the hunks it `picks`) and the other
/// tasks with edits in it (each with its log): the part to bring, if any of it is `me`'s, and the
/// hunks no log has that stay. The reason it can't be split, to refuse with.
fn split(top: &str, f: &str, me: &str, mine: &str, theirs: &[(String, String)], picks: &[&str]) -> std::result::Result<(Option<Part>, Vec<String>), String> {
    let names = theirs.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join(" and ");
    let whole = || if names.is_empty() { format!("{f} (only a file HEAD has can be split by hunk; bring it whole with --bring {f})") } else { format!("{f} ({names} has uncommitted edits in it too)") };
    let head = git(top, &["show", &format!("HEAD:{f}")], GIT_SECS);
    let diff = git(top, &["diff", "--no-ext-diff", "--no-color", "--no-textconv", "-U0", "HEAD", "--", f], GIT_SECS);
    let Ok(work) = std::fs::read_to_string(Path::new(top).join(f)) else { return Err(whole()) };
    if head.code != 0 || diff.code != 0 || head.out.contains('\u{FFFD}') || diff.out.lines().any(|l| l.starts_with("Binary files")) {
        return Err(whole());
    }
    let (h, w) = (hunks::lines(&head.out), hunks::lines(&work));
    let all = hunks::parse(f, &h, &w, &diff.out);
    let abs = format!("{top}/{f}");
    let my_marks = Marks::of(mine, &abs, &w).unwrap_or_default();
    let mut their_marks = vec![];
    for (task, log) in theirs {
        // A task whose edits were logged by file only can't be told apart from this one.
        let Some(m) = Marks::of(log, &abs, &w) else { return Err(format!("{f} ({task} has uncommitted edits in it too)")) };
        their_marks.push((task.as_str(), m));
    }
    let ids = || all.iter().map(|x| format!("{} (line {})", x.id, x.line())).collect::<Vec<_>>().join(", ");
    if let Some(p) = picks.iter().find(|p| !all.iter().any(|x| x.id == **p)) {
        return Err(format!("{f}:{p} (no such hunk; its hunks are {})", ids()));
    }
    let mut take = HashSet::new();
    let mut listed = vec![];
    for x in &all {
        let by: Vec<&str> = their_marks.iter().filter(|(_, m)| m.made(x)).map(|(t, _)| *t).collect();
        let picked = picks.contains(&x.id.as_str());
        if !by.is_empty() && (picked || my_marks.made(x)) {
            return Err(format!("{f} ({} changed the same lines as {me}, at line {})", by.join(" and "), x.line()));
        }
        if picked || my_marks.made(x) {
            take.insert(x.id.clone());
        } else if by.is_empty() {
            listed.push(format!("{f}:{} (line {}: {})", x.id, x.line(), hunks::preview(x, &h, &w)));
        }
    }
    if take.is_empty() {
        return Ok((None, listed));
    }
    let tree = git(top, &["ls-tree", "HEAD", "--", f], GIT_SECS);
    let mode = tree.out.split_whitespace().next().unwrap_or("100644").to_string();
    let slot = hunks::build(&h, &w, &all, |x| take.contains(&x.id));
    let shared = hunks::build(&h, &w, &all, |x| !take.contains(&x.id));
    Ok((Some(Part { file: f.to_string(), work, slot, shared, mode }), listed))
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

/// Moves `b`'s files into the slot at `slot`: saves the patch, applies it there, then takes the
/// edits out of the shared checkout (a split file gets the other tasks' hunks only, a whole one goes
/// back to HEAD). The saved patch. On a clash, or a split file that can't be written, the slot is put
/// back as it was and the shared checkout is left alone; the 409 names the files and the patch.
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
        give_back(slot);
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
    if let Err(why) = take_out(b) {
        give_back(slot);
        return err(409, format!("{me} couldn't take its hunks out of {why}. Nothing changed: the edits are still in the shared checkout ({}) and the slot was given back. The patch is saved at {patch}.", b.top));
    }
    let whole: Vec<String> = b.files.iter().filter(|f| !b.parts.iter().any(|p| &p.file == *f)).cloned().collect();
    restore(&b.top, &whole)?;
    Ok(patch)
}

/// Puts a slot back as it was before the patch.
fn give_back(slot: &str) {
    git(slot, &["reset", "-q", "--hard"], GIT_SECS);
    git(slot, &["clean", "-fdq"], GIT_SECS);
}

/// Writes each split file's shared copy, its other tasks' hunks only, and unstages it. A file that
/// changed since the plan, or can't be written, puts back the ones already written; the reason.
fn take_out(b: &Bring) -> std::result::Result<(), String> {
    let mut done: Vec<&Part> = vec![];
    for p in &b.parts {
        let path = Path::new(&b.top).join(&p.file);
        let wrote = match std::fs::read_to_string(&path) {
            Ok(now) if now == p.work => std::fs::write(&path, &p.shared).map_err(|e| format!("{} ({e})", p.file)),
            _ => Err(format!("{} (it changed while the slot was being claimed)", p.file)),
        };
        if let Err(why) = wrote {
            for d in done {
                let _ = std::fs::write(Path::new(&b.top).join(&d.file), &d.work);
            }
            return Err(why);
        }
        done.push(p);
    }
    if !b.parts.is_empty() {
        let mut args = vec!["restore", "--source=HEAD", "--staged", "--"];
        args.extend(b.parts.iter().map(|p| p.file.as_str()));
        git(&b.top, &args, GIT_SECS);
    }
    Ok(())
}

/// The patch of `b`'s files against the shared checkout's HEAD, saved in the data folder: whole files
/// as they are, split ones as the slot gets them. Built in an index of its own, so the patch has the
/// blob ids `--3way` falls back on and the shared checkout's index isn't touched.
fn save_patch(app: &App, t: &Row, b: &Bring) -> Result<String> {
    let dir = app.cfg.data.join("brought");
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::new(500, format!("Couldn't make {}: {e}", dir.display())))?;
    let stamp = now_iso().replace([':', '.'], "-");
    let path = dir.join(format!("{}-{stamp}.patch", rf("task", t.id())));
    let index = dir.join(format!("{}-{stamp}.index", rf("task", t.id())));
    let path_s = path.to_string_lossy().to_string();
    let env = [("GIT_INDEX_FILE".to_string(), index.to_string_lossy().to_string())];
    let run = |args: &[&str], input: Option<&[u8]>| -> Result<String> {
        let r = git_with(&b.top, args, &env, input);
        if r.code != 0 {
            return err(409, format!("Couldn't take the patch of {}: {}. Nothing changed.", b.files.join(", "), last_line(&r.err)));
        }
        Ok(r.out)
    };
    let made = (|| {
        run(&["read-tree", "HEAD"], None)?;
        let whole: Vec<&str> = b.files.iter().filter(|f| !b.parts.iter().any(|p| &p.file == *f)).map(|f| f.as_str()).collect();
        if !whole.is_empty() {
            let mut args = vec!["update-index", "--add", "--remove", "--"];
            args.extend(whole.iter());
            run(&args, None)?;
        }
        for p in &b.parts {
            let blob = run(&["hash-object", "-w", "--stdin"], Some(p.slot.as_bytes()))?;
            run(&["update-index", "--add", "--cacheinfo", &format!("{},{},{}", p.mode, blob.trim(), p.file)], None)?;
        }
        let mut args = vec!["diff", "--cached", "--binary", "--no-ext-diff", "--no-color", "--output", path_s.as_str(), "HEAD", "--"];
        args.extend(b.files.iter().map(|f| f.as_str()));
        run(&args, None)
    })();
    let _ = std::fs::remove_file(&index);
    made?;
    Ok(path_s)
}

/// `git` with extra environment variables and bytes for its stdin.
fn git_with(path: &str, args: &[&str], env: &[(String, String)], input: Option<&[u8]>) -> Git {
    let Some(bin) = crate::proc::which("git") else { return Git { code: 1, out: String::new(), err: "git isn't installed".into() } };
    let mut all: Vec<String> = vec!["-C".into(), path.into()];
    all.extend(args.iter().map(|a| a.to_string()));
    match crate::proc::run_with(&bin, &all, None, GIT_SECS, env, input) {
        Ok(o) => Git { code: o.code.unwrap_or(1), out: o.stdout, err: o.stderr },
        Err(crate::proc::RunError::TimedOut) => Git { code: 1, out: String::new(), err: "git timed out".into() },
        Err(crate::proc::RunError::Spawn(e)) => Git { code: 1, out: String::new(), err: e.to_string() },
    }
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
fn changes(top: &str) -> Result<Vec<String>> {
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
        let x = e.as_bytes()[0];
        if matches!(x, b'R' | b'C') {
            if let Some(from) = parts.next().filter(|_| x == b'R') {
                list.push(from.to_string());
            }
        }
        list.push(e[3..].to_string());
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

/// `task`'s log of its edits' patches (`hunks/<task>`), empty when it has none.
fn hunk_log(app: &App, task: &str) -> String {
    std::fs::read_to_string(app.cfg.data.join("hunks").join(task)).unwrap_or_default()
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
