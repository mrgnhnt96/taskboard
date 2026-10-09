//! Generated files out of agents' diffs: the board keeps a block of `<glob> -diff` lines (the globs
//! from `tb limits --generated`) in each active project's and worktree's `.git/info/attributes`, so
//! `git diff` shows them as one line instead of every generated line. Only the block is the board's:
//! the rest of the file is left alone, and with no globs the block goes. A repo that stops being
//! looked after (its project removed, its work done) has its block taken out.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::app::App;
use crate::util::*;
use crate::{board, limits, p, projects};

pub const BEGIN: &str = "# >>> taskboard: generated files (kept by the board; change them with tb limits --generated)";
pub const END: &str = "# <<< taskboard";
/// How often the runner puts the blocks back.
pub const EVERY_SECS: f64 = 300.0;
const GIT_SECS: f64 = 5.0;
/// The attributes files the board last kept a block in.
const MANAGED_KEY: &str = "gitattrs_managed";

/// `text` with the board's block set to `globs` (or taken out, with none).
pub fn apply(text: &str, globs: &[String]) -> String {
    let mut kept: Vec<&str> = vec![];
    let mut inside = false;
    for l in text.lines() {
        if l.trim_end() == BEGIN {
            inside = true;
        } else if inside && l.trim_end() == END {
            inside = false;
        } else if !inside {
            kept.push(l);
        }
    }
    while kept.last().is_some_and(|l| l.trim().is_empty()) {
        kept.pop();
    }
    let mut out = kept.join("\n");
    if !globs.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(BEGIN);
        for g in globs {
            out.push_str(&format!("\n{g} -diff"));
        }
        out.push('\n');
        out.push_str(END);
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Writes the block into one attributes file. True when the file changed.
pub fn write(path: &Path, globs: &[String]) -> std::io::Result<bool> {
    let cur = std::fs::read_to_string(path).unwrap_or_default();
    let next = apply(&cur, globs);
    if next == cur || (!path.exists() && globs.is_empty()) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("taskboard.{}.tmp", std::process::id()));
    std::fs::write(&tmp, next)?;
    std::fs::rename(&tmp, path)?;
    Ok(true)
}

/// The attributes file git reads for the checkout at `dir` (a linked worktree shares its repo's).
pub fn attributes_file(dir: &str) -> Option<PathBuf> {
    let git = crate::proc::which("git")?;
    let args: Vec<String> = ["-C", dir, "rev-parse", "--git-path", "info/attributes"].iter().map(|s| s.to_string()).collect();
    let o = crate::proc::run(&git, &args, None, GIT_SECS).ok().filter(|o| o.code == Some(0))?;
    let rel = o.stdout.trim();
    if rel.is_empty() {
        return None;
    }
    let p = Path::new(rel);
    Some(if p.is_absolute() { p.to_path_buf() } else { Path::new(dir).join(p) })
}

/// The folders to look after, with their project: every known project, plus the worktrees and
/// terminal folders of open work.
pub fn targets(app: &App) -> Result<Vec<(Option<String>, String)>> {
    let mut out: Vec<(Option<String>, String)> = vec![];
    let mut add = |project: Option<&str>, dir: Option<&str>| {
        let Some(dir) = dir.map(|d| d.trim_end_matches('/')).filter(|d| !d.is_empty() && Path::new(d).is_dir()) else { return };
        if !out.iter().any(|(_, d)| d == dir) {
            out.push((project.filter(|p| !p.is_empty()).map(|p| p.to_string()), dir.to_string()));
        }
    };
    for p in projects::list_projects(app)? {
        add(p["name"].as_str(), p["path"].as_str());
    }
    for t in app.db.q("SELECT * FROM tasks WHERE status != 'done' OR pr_num IS NOT NULL", p![])? {
        if t.s("status") == Some("done") && !board::pr_still_open(&t) {
            continue;
        }
        let ctx = board::task_context(&t);
        add(t.s("project"), ctx.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str()));
        add(t.s("project"), ctx.s("worktree_made"));
    }
    for s in app.db.q("SELECT project, project_path FROM sessions WHERE status != 'gone'", p![])? {
        add(s.s("project"), s.s("project_path"));
    }
    Ok(out)
}

/// Puts the block in every active project's attributes, and takes it out of the ones it was in
/// before that aren't looked after now. Returns how many files changed.
pub fn sync(app: &App) -> Result<usize> {
    let l = limits::get(app);
    let mut files: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    for (project, dir) in targets(app)? {
        let Some(f) = attributes_file(&dir) else { continue };
        let globs = files.entry(f).or_default();
        for g in l.globs_for(project.as_deref()) {
            if !globs.contains(&g) {
                globs.push(g);
            }
        }
    }
    let before: Vec<String> = app.db.get_setting(MANAGED_KEY)?.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let mut managed: Vec<String> = vec![];
    for old in before {
        files.entry(PathBuf::from(&old)).or_default();
    }
    let mut changed = 0;
    for (f, globs) in files {
        match write(&f, &globs) {
            Ok(true) => changed += 1,
            Ok(false) => {}
            Err(e) => {
                app.info(format!("attributes: couldn't write {}: {e}", f.display()));
                if f.exists() {
                    managed.push(f.to_string_lossy().into_owned());
                }
                continue;
            }
        }
        if !globs.is_empty() {
            managed.push(f.to_string_lossy().into_owned());
        }
    }
    app.db.set_setting(MANAGED_KEY, Some(&serde_json::to_string(&managed).unwrap_or_default()))?;
    if changed > 0 {
        app.info(format!("attributes: updated the generated files in {}", plural(changed as i64, "repo")));
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_owners_lines_and_replaces_its_block() {
        let globs = vec!["*.g.dart".to_string(), "Cargo.lock".to_string()];
        let once = apply("*.png binary\n", &globs);
        assert_eq!(once, format!("*.png binary\n\n{BEGIN}\n*.g.dart -diff\nCargo.lock -diff\n{END}\n"));
        assert_eq!(apply(&once, &globs), once, "the same globs change nothing");
        let fewer = apply(&once, &globs[..1]);
        assert!(fewer.contains("*.g.dart -diff") && !fewer.contains("Cargo.lock"));
        assert_eq!(apply(&once, &[]), "*.png binary\n");
        assert_eq!(apply("", &[]), "");
    }
}
