//! Keeping a task's worktree slot fit to work in (`slots.rs` is the pool). When a task starts again in
//! the slot it holds, files it never edited that match its base (a stale copy of the base branch left
//! behind) are saved to a backup ref and cleared, and a branch that isn't pushed yet is rebased onto
//! the base when that goes cleanly. A task holding more than one slot gives back the extras that are
//! clean and pushed; for the rest the board opens a terminal to sort them out, and tells the owner
//! when that terminal ends with them still held. `tb slot discard` saves a slot's changes to a backup
//! ref and clears it.

use std::path::Path;

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::worktrees::{git, last_line, real, Git, GIT_SECS};
use crate::{board, p, slots};

/// Where saved changes go: `refs/taskboard/backup/slot-<n>-<when>`.
const BACKUPS: &str = "refs/taskboard/backup";
const FETCH_SECS: f64 = 300.0;
/// The job that opens a terminal to sort out a task's extra slots.
pub const PURPOSE: &str = "slots";
/// The task's context key for the extra slots it was asked to sort out.
const ASKED: &str = "extra_slots";

fn extra_key(t: &Row) -> String {
    format!("slots-extra:{}", rf("task", t.id()))
}

/// The slot's changed files, untracked ones one by one: (file, untracked). None when git fails.
fn changed(path: &str) -> Option<Vec<(String, bool)>> {
    let r = git(path, &["status", "--porcelain=v1", "-z", "--untracked-files=all", "--no-renames"], GIT_SECS);
    if r.code != 0 {
        return None;
    }
    Some(r.out.split('\0').filter(|e| e.len() > 3).map(|e| (e[3..].to_string(), e.starts_with("??"))).collect())
}

fn git_env(path: &str, args: &[&str], env: &[(String, String)]) -> Git {
    let Some(bin) = crate::proc::which("git") else { return Git { code: 1, out: String::new(), err: "git isn't installed".into() } };
    let mut all: Vec<String> = vec!["-C".into(), path.into()];
    all.extend(args.iter().map(|a| a.to_string()));
    match crate::proc::run_with(&bin, &all, None, GIT_SECS, env, None) {
        Ok(o) => Git { code: o.code.unwrap_or(1), out: o.stdout, err: o.stderr },
        Err(_) => Git { code: 1, out: String::new(), err: "git didn't run".into() },
    }
}

/// Saves everything changed in the slot (untracked files too, not ignored ones) as a commit on top of
/// its HEAD, under a ref of its own, without touching the slot. The ref.
fn backup(path: &str, n: i64, why: &str) -> Result<String> {
    let index = std::env::temp_dir().join(format!("tb-slot-backup-{}-{n}", std::process::id()));
    let _ = std::fs::remove_file(&index);
    let env: Vec<(String, String)> = [
        ("GIT_INDEX_FILE", index.to_string_lossy().to_string()),
        ("GIT_AUTHOR_NAME", "Task board".into()),
        ("GIT_AUTHOR_EMAIL", "board@localhost".into()),
        ("GIT_COMMITTER_NAME", "Task board".into()),
        ("GIT_COMMITTER_EMAIL", "board@localhost".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let steps = || -> std::result::Result<String, String> {
        for args in [&["read-tree", "HEAD"][..], &["add", "-A"][..]] {
            let r = git_env(path, args, &env);
            if r.code != 0 {
                return Err(last_line(&r.err));
            }
        }
        let tree = git_env(path, &["write-tree"], &env);
        if tree.code != 0 {
            return Err(last_line(&tree.err));
        }
        let commit = git_env(path, &["commit-tree", tree.out.trim(), "-p", "HEAD", "-m", why], &env);
        if commit.code != 0 {
            return Err(last_line(&commit.err));
        }
        let stamp: String = now_iso().chars().filter(|c| c.is_ascii_digit()).collect();
        let name = format!("{BACKUPS}/slot-{n}-{stamp}");
        let r = git(path, &["update-ref", &name, commit.out.trim()], GIT_SECS);
        if r.code != 0 {
            return Err(last_line(&r.err));
        }
        Ok(name)
    };
    let out = steps();
    let _ = std::fs::remove_file(&index);
    out.map_err(|e| ApiError::new(409, format!("Couldn't save the changes in slot-{n} ({path}) before clearing them: {e}")))
}

/// Puts these files back as HEAD has them; one HEAD doesn't have goes.
fn restore(path: &str, files: &[String]) {
    let (known, new): (Vec<&String>, Vec<&String>) = files.iter().partition(|f| git(path, &["cat-file", "-e", &format!("HEAD:{f}")], GIT_SECS).code == 0);
    for chunk in known.chunks(200) {
        let mut args = vec!["checkout", "HEAD", "--"];
        args.extend(chunk.iter().map(|f| f.as_str()));
        git(path, &args, GIT_SECS);
    }
    for f in new {
        git(path, &["rm", "-q", "--cached", "--ignore-unmatch", "--", f], GIT_SECS);
        let _ = std::fs::remove_file(Path::new(path).join(f));
    }
}

/// Is the file in the slot as `at` has it (gone, when `at` doesn't have it)?
fn as_in(path: &str, at: &str, file: &str) -> bool {
    let want = git(path, &["rev-parse", "--verify", "--quiet", &format!("{at}:{file}")], GIT_SECS);
    if !Path::new(path).join(file).exists() {
        return want.code != 0;
    }
    if want.code != 0 {
        return false;
    }
    let have = git(path, &["hash-object", "--", file], GIT_SECS);
    have.code == 0 && have.out.trim() == want.out.trim()
}

/// A task about to start again in the slot it holds: clears what was left there that isn't its work,
/// then brings the slot up to its base. Never while a terminal of its works there.
pub fn on_resume(app: &App, t: &Row, s: &Row) {
    match crate::worktrees::terminal_open(app, t) {
        Ok(false) => {}
        _ => return,
    }
    if let Err(e) = clear_stale(app, t, s).and_then(|_| catch_up(app, t, s)) {
        app.info(format!("worktrees: {} slot-{}: {}", rf("task", t.id()), s.i0("n"), e.message));
    }
}

/// Changed files the task never edited that match its base (or where it branched from it) are a stale
/// copy, not its work: saved to a backup ref, then put back as HEAD has them.
fn clear_stale(app: &App, t: &Row, s: &Row) -> Result<()> {
    let (path, n) = (s.st("path"), s.i0("n"));
    let Some(files) = changed(&path).filter(|f| !f.is_empty()) else { return Ok(()) };
    let base = board::task_context(t).s("worktree_base").unwrap_or("HEAD").to_string();
    let mut at = vec![base.clone()];
    let fork = git(&path, &["merge-base", "HEAD", &base], GIT_SECS);
    if fork.code == 0 && !fork.out.trim().is_empty() {
        at.push(fork.out.trim().to_string());
    }
    let mine = crate::bring::edits_of(app, &rf("task", t.id()), &real(&path));
    let stale: Vec<String> = files.iter().map(|(f, _)| f.clone()).filter(|f| !mine.contains(f) && at.iter().any(|a| as_in(&path, a, f))).collect();
    if stale.is_empty() {
        return Ok(());
    }
    let me = rf("task", t.id());
    let saved = backup(&path, n, &format!("Left in slot-{n} before {me} started again"))?;
    restore(&path, &stale);
    let kept = files.len() - stale.len();
    let rest = if kept > 0 { format!("; {} other changed {} left as {} may be its work", kept, if kept == 1 { "file is" } else { "files are" }, if kept == 1 { "it" } else { "they" }) } else { String::new() };
    let text = format!(
        "Cleared {} in slot-{n} that {me} never edited and that match {base}, left from an earlier run (saved first to {saved}){rest}",
        plural(stale.len() as i64, "changed file")
    );
    app.info(format!("worktrees: {text}"));
    board::log_event(app, t.id(), board::BOARD, "midna", &text)?;
    Ok(())
}

/// Brings a clean slot up to its base: detached, moved to it; on a branch that isn't pushed, rebased
/// onto it unless that conflicts. A pushed branch or one that conflicts is the task's to rebase; its
/// log says how far behind it is.
fn catch_up(app: &App, t: &Row, s: &Row) -> Result<()> {
    let (path, n) = (s.st("path"), s.i0("n"));
    let Some(base) = board::task_context(t).s("worktree_base").filter(|b| *b != "HEAD").map(str::to_string) else { return Ok(()) };
    if base.starts_with("origin/") {
        git(&path, &["fetch", "-q", "origin"], FETCH_SECS);
    }
    let count = git(&path, &["rev-list", "--count", &format!("HEAD..{base}")], GIT_SECS);
    let behind: i64 = count.out.trim().parse().unwrap_or(0);
    if count.code != 0 || behind == 0 {
        return Ok(());
    }
    let ahead = format!("{} behind {base}", plural(behind, "commit"));
    let dirty = changed(&path).map(|f| f.len()).unwrap_or(1);
    let text = match slots::branch_of(&path) {
        _ if dirty > 0 => format!("slot-{n} is {ahead}; it has uncommitted changes, so the board left it as it is"),
        None if git(&path, &["merge-base", "--is-ancestor", "HEAD", &base], GIT_SECS).code == 0 => {
            if git(&path, &["switch", "-q", "--detach", &base], GIT_SECS).code != 0 {
                return Ok(());
            }
            format!("Moved slot-{n} up to {base}: it was {ahead}")
        }
        None => format!("slot-{n} is {ahead}, detached with commits of its own"),
        Some(b) if git(&path, &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/origin/{b}")], GIT_SECS).code == 0 => {
            format!("{b} in slot-{n} is {ahead}; it's pushed, so rebasing it is the task's")
        }
        Some(b) => {
            let r = git(&path, &["rebase", "-q", &base], GIT_SECS);
            if r.code == 0 {
                format!("Rebased {b} in slot-{n} onto {base}: it was {ahead}")
            } else {
                git(&path, &["rebase", "--abort"], GIT_SECS);
                format!("{b} in slot-{n} is {ahead}; rebasing it onto {base} conflicts, so that's the task's")
            }
        }
    };
    board::log_event(app, t.id(), board::BOARD, "midna", &text)?;
    Ok(())
}

fn clean_and_pushed(path: &str) -> bool {
    changed(path).is_some_and(|f| f.is_empty()) && slots::unpushed(path) == 0
}

/// Tasks holding more than one slot keep the one they last took. Each extra that's clean and pushed
/// (or no worktree any more) is given back; for the rest a terminal is opened to sort them out.
pub fn sweep(app: &App) -> Result<()> {
    let tasks = app.db.q("SELECT task_id FROM worktree_slots WHERE task_id IS NOT NULL GROUP BY task_id HAVING COUNT(*) > 1", p![])?;
    for r in tasks {
        let Some(t) = board::find_task(app, r.i("task_id"))? else { continue };
        let Some(keep) = slots::held(app, &t)? else { continue };
        let mut left = vec![];
        for s in app.db.q("SELECT * FROM worktree_slots WHERE task_id = ? AND id != ? ORDER BY n", p![t.id(), keep.id()])? {
            let path = s.st("path");
            if !slots::live(&path) || clean_and_pushed(&path) {
                give_back_extra(app, &t, &s)?;
            } else {
                left.push(s);
            }
        }
        if left.is_empty() {
            crate::dispatch::clear_alert_key(app, &extra_key(&t))?;
        } else {
            sort_out(app, &t, &keep, &left)?;
        }
    }
    Ok(())
}

fn give_back_extra(app: &App, t: &Row, s: &Row) -> Result<()> {
    let (path, n) = (s.st("path"), s.i0("n"));
    // Off its branch, so the task's own slot (or another) can check it out.
    if slots::live(&path) && slots::branch_of(&path).is_some() {
        git(&path, &["switch", "-q", "--detach"], GIT_SECS);
    }
    app.db.x("UPDATE worktree_slots SET task_id = NULL, used_at = ? WHERE id = ?", p![now_iso(), s.id()])?;
    board::log_event(app, t.id(), board::BOARD, "midna", &format!("Gave back worktree slot-{n}, an extra slot it held besides its own"))?;
    Ok(())
}

/// Is the terminal opened for this job still there to do its work?
fn job_live(app: &App, jid: Option<i64>) -> Result<bool> {
    let Some(j) = app.db.q1("SELECT * FROM jobs WHERE id = ?", p![jid])? else { return Ok(false) };
    if matches!(j.s("state"), Some("pending") | Some("running")) {
        return Ok(true);
    }
    let sid = board::job_target(&j).s("session").map(str::to_string);
    Ok(j.s("state") == Some("done") && board::get_session(app, sid.as_deref())?.is_some_and(|s| s.s("status") != Some("gone")))
}

/// Opens a terminal to sort out the extra slots, once for each set of them; when that terminal ends
/// with the same ones held, the owner hears of it instead.
fn sort_out(app: &App, t: &Row, keep: &Row, left: &[Row]) -> Result<()> {
    let paths: Vec<String> = left.iter().map(|s| s.st("path")).collect();
    let ctx = board::task_context(t);
    let asked = ctx.get(ASKED).cloned().unwrap_or(Value::Null);
    let names = left.iter().map(|s| format!("slot-{}", s.i0("n"))).collect::<Vec<_>>().join(", ");
    let me = rf("task", t.id());
    if asked["paths"] == json!(paths) {
        if job_live(app, asked["job"].as_i64())? {
            return Ok(());
        }
        let text = format!(
            "{me} still holds {names} besides its own slot-{}: they have work that isn't committed or pushed, and the terminal the board opened to sort them out ended without freeing them.",
            keep.i0("n")
        );
        return crate::dispatch::add_alert_keyed(app, &text, Some(t.id()), None, &extra_key(t));
    }
    let args = json!({"cwd": paths[0], "title": short(&format!("Slots: {}", t.st("title")), 40), "prompt": prompt(app, t, keep, left), "queue": false});
    let jid = board::create_job(app, "agent", args, None, PURPOSE, Some(json!({"task": t.id()})))?;
    app.db.tx(|| {
        let mut ctx = board::task_context(&board::get_task(app, t.id())?);
        ctx.insert(ASKED.into(), json!({"paths": paths, "job": jid, "at": now_iso()}));
        board::save_context(app, t.id(), &ctx, false)?;
        board::log_event(app, t.id(), board::BOARD, "midna", &format!("Holds {names} besides its own slot-{}; opened a terminal to sort them out", keep.i0("n")))?;
        Ok(())
    })
}

fn prompt(app: &App, t: &Row, keep: &Row, left: &[Row]) -> String {
    let tb = board::tb_cmd(app);
    let me = rf("task", t.id());
    let own = format!("slot-{} ({})", keep.i0("n"), keep.st("path"));
    let extras = left.iter().map(|s| format!("- slot-{}: {}", s.i0("n"), s.st("path"))).collect::<Vec<_>>().join("\n");
    format!(
        "{me} “{}” holds more than one worktree slot. Its work is in {own}; these extra slots hold uncommitted changes \
         or unpushed commits from earlier runs of it:\n{extras}\n\n\
         A task keeps one slot. Sort out each extra slot so the board can give it back:\n\
         - Compare its changes and commits with {own} and with {me}'s branch. Work that's already there, or a copy of it, can go.\n\
         - Changes only to generated files (lockfiles like Podfile.lock, build output, generated code) can go too, unless {me}'s work needs them committed.\n\
         - Work that isn't anywhere else stays: commit it on that slot's branch and push it, then add a note on the task saying which branch has it: {tb} note --task {me} \"…\".\n\
         - To drop a slot's changes, run {tb} slot discard <path>: the board saves them to a backup ref first, then clears the slot.\n\
         Don't change {own}: {me} works there. The board gives an extra slot back once it's clean and pushed. \
         Don't take a task, make one or open a PR here. When every extra slot is sorted out, say so and stop.",
        t.st("title")
    )
}

/// `tb slot discard`: saves the changes in the slot `path` is in to a backup ref, then clears them.
/// Refused in the slot a task works in while its terminal is open.
pub fn discard(app: &App, path: &str) -> Result<Value> {
    let Some(s) = slots::slot_at(app, path)? else { return err(404, format!("{path} isn't in a worktree slot.")) };
    let (sp, n) = (s.st("path"), s.i0("n"));
    if !slots::live(&sp) {
        return err(409, format!("slot-{n} ({sp}) isn't a git worktree any more."));
    }
    let holder = board::find_task(app, s.i("task_id"))?;
    if let Some(h) = &holder {
        if slots::held(app, h)?.is_some_and(|k| k.id() == s.id()) && crate::worktrees::terminal_open(app, h)? {
            return err(409, format!("slot-{n} is where {} works, and its terminal is open: commit or discard its changes there.", rf("task", h.id())));
        }
    }
    let files = changed(&sp).unwrap_or_default().len();
    if files == 0 {
        return Ok(json!({"ok": true, "slot": format!("slot-{n}"), "path": sp, "files": 0}));
    }
    let saved = backup(&sp, n, &format!("Discarded from slot-{n}"))?;
    let reset = git(&sp, &["reset", "-q", "--hard", "HEAD"], GIT_SECS);
    let clean = git(&sp, &["clean", "-fdq"], GIT_SECS);
    if reset.code != 0 || clean.code != 0 {
        return err(409, format!("Saved the changes to {saved}, but couldn't clear slot-{n}: {}", last_line(if reset.code != 0 { &reset.err } else { &clean.err })));
    }
    if let Some(h) = &holder {
        board::log_event(app, h.id(), board::BOARD, "midna", &format!("Discarded {} in slot-{n} (saved first to {saved})", plural(files as i64, "changed file")))?;
    }
    Ok(json!({"ok": true, "slot": format!("slot-{n}"), "path": sp, "files": files, "backup": saved}))
}
