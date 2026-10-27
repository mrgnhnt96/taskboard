//! Keeping each project's own checkout of its default branch up to date. Worktrees start from
//! `origin/<default>`, so nothing else ever moves the local `main` (or `master`) and it drifts far
//! behind. On the PR timer, every few minutes, each project's repo is fetched and its local default
//! branch fast-forwarded to origin's: moved in place when no worktree has it checked out, else merged
//! `--ff-only` where it is, but only when that checkout has no changes to tracked files and no merge or
//! rebase under way. A branch with commits of its own is left alone.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use once_cell::sync::Lazy;

use crate::app::App;
use crate::util::*;
use crate::worktrees::{git, last_line, GIT_SECS};

/// How often a repo is looked at.
const EVERY_SECS: f64 = 300.0;
const FETCH_SECS: f64 = 120.0;

/// When each repo was last looked at, by path.
static CHECKED: Lazy<Mutex<HashMap<String, Instant>>> = Lazy::new(Default::default);

/// Looks at each project's repo that's due. Outside a transaction: git may take a while.
pub fn sweep(app: &App) -> Result<()> {
    let mut seen = HashSet::new();
    for p in crate::projects::list_projects(app)? {
        let Some(path) = p["path"].as_str().filter(|p| Path::new(p).is_dir()) else { continue };
        let Some(repo) = top(path) else { continue };
        if !seen.insert(repo.clone()) || !due(&repo) {
            continue;
        }
        if let Some(text) = sync(&repo) {
            app.info(format!("default branch: {}: {text}", p["name"].as_str().unwrap_or("?")));
        }
    }
    Ok(())
}

fn due(repo: &str) -> bool {
    let mut all = CHECKED.lock().unwrap();
    if all.get(repo).is_some_and(|at| at.elapsed().as_secs_f64() < EVERY_SECS) {
        return false;
    }
    all.insert(repo.to_string(), Instant::now());
    true
}

/// The main checkout of the repo `path` is in (a worktree's path gives its repo's).
fn top(path: &str) -> Option<String> {
    let r = git(path, &["rev-parse", "--path-format=absolute", "--git-common-dir"], GIT_SECS);
    if r.code != 0 {
        return None;
    }
    let common = r.out.trim();
    Some(common.strip_suffix("/.git").unwrap_or(common).to_string())
}

/// Fetches origin and fast-forwards the default branch when it's behind and that's safe. What it did,
/// or why it couldn't; None when there was nothing to do.
pub fn sync(repo: &str) -> Option<String> {
    if git(repo, &["remote", "get-url", "origin"], GIT_SECS).code != 0 {
        return None;
    }
    let f = git(repo, &["fetch", "-q", "origin"], FETCH_SECS);
    if f.code != 0 {
        return Some(format!("git fetch failed: {}", last_line(&f.err)));
    }
    let branch = default_branch(repo)?;
    let (local, remote) = (format!("refs/heads/{branch}"), format!("refs/remotes/origin/{branch}"));
    let count = |range: String| git(repo, &["rev-list", "--count", &range], GIT_SECS).out.trim().parse::<i64>().unwrap_or(0);
    let behind = count(format!("{local}..{remote}"));
    if behind == 0 {
        return None;
    }
    let lag = format!("{branch} was {}", plural(behind, "commit"));
    if count(format!("{remote}..{local}")) > 0 {
        return Some(format!("{lag} behind origin/{branch} and has commits of its own, so it's left as it is"));
    }
    match checked_out(repo, &local) {
        None => {
            let old = git(repo, &["rev-parse", &local], GIT_SECS).out.trim().to_string();
            let r = git(repo, &["update-ref", &local, &remote, &old], GIT_SECS);
            if r.code != 0 {
                return Some(format!("{lag} behind; moving it failed: {}", last_line(&r.err)));
            }
        }
        Some(at) => {
            if let Some(why) = busy(&at) {
                return Some(format!("{lag} behind; {at} {why}, so it's left as it is"));
            }
            let r = git(&at, &["merge", "--ff-only", "-q", &remote], GIT_SECS);
            if r.code != 0 {
                return Some(format!("{lag} behind; fast-forwarding it in {at} failed: {}", last_line(&r.err)));
            }
        }
    }
    Some(format!("fast-forwarded {branch} to origin/{branch}: it was {} behind", plural(behind, "commit")))
}

/// The default branch's name: origin's HEAD, else main or master on origin, when the repo has it locally.
fn default_branch(repo: &str) -> Option<String> {
    let head = git(repo, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"], GIT_SECS);
    let from_head = head.out.trim().strip_prefix("origin/").map(str::to_string).filter(|_| head.code == 0);
    let has = |r: &str| git(repo, &["rev-parse", "--verify", "--quiet", r], GIT_SECS).code == 0;
    from_head
        .into_iter()
        .chain(["main".to_string(), "master".to_string()])
        .find(|b| has(&format!("refs/remotes/origin/{b}")) && has(&format!("refs/heads/{b}")))
}

/// The worktree that has the branch checked out, if one does.
fn checked_out(repo: &str, local: &str) -> Option<String> {
    let list = git(repo, &["worktree", "list", "--porcelain"], GIT_SECS);
    worktree_on(&list.out, local)
}

fn worktree_on(porcelain: &str, local: &str) -> Option<String> {
    porcelain.split("\n\n").find_map(|block| {
        let at = block.lines().find_map(|l| l.strip_prefix("worktree "))?;
        block.lines().any(|l| l.strip_prefix("branch ") == Some(local)).then(|| at.to_string())
    })
}

/// Why the checkout can't be moved now: changes to tracked files, or a merge, rebase, cherry-pick or
/// revert under way.
fn busy(at: &str) -> Option<&'static str> {
    let status = git(at, &["status", "--porcelain=v1", "--untracked-files=no"], GIT_SECS);
    if status.code != 0 {
        return Some("couldn't be read");
    }
    if !status.out.trim().is_empty() {
        return Some("has uncommitted changes");
    }
    let gitdir = git(at, &["rev-parse", "--path-format=absolute", "--git-dir"], GIT_SECS).out.trim().to_string();
    let mid = ["rebase-merge", "rebase-apply", "MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"].iter().any(|f| Path::new(&gitdir).join(f).exists());
    mid.then_some("is mid-merge or mid-rebase")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, args: &[&str]) {
        let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    }

    fn commit(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), file).unwrap();
        sh(dir, &["add", file]);
        sh(dir, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", file]);
    }

    /// An origin with a commit, a clone of it, then two more commits on origin.
    fn setup() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let (origin, clone) = (tmp.path().join("origin"), tmp.path().join("clone"));
        std::fs::create_dir(&origin).unwrap();
        sh(&origin, &["init", "-q", "-b", "main"]);
        commit(&origin, "a");
        sh(tmp.path(), &["clone", "-q", origin.to_str().unwrap(), clone.to_str().unwrap()]);
        commit(&origin, "b");
        commit(&origin, "c");
        (tmp, origin, clone)
    }

    fn head(dir: &Path, r: &str) -> String {
        git(dir.to_str().unwrap(), &["rev-parse", r], GIT_SECS).out.trim().to_string()
    }

    #[test]
    fn checked_out_and_clean_is_fast_forwarded() {
        let (_tmp, origin, clone) = setup();
        let text = sync(clone.to_str().unwrap()).unwrap();
        assert!(text.starts_with("fast-forwarded main"), "{text}");
        assert_eq!(head(&clone, "HEAD"), head(&origin, "HEAD"));
        assert_eq!(sync(clone.to_str().unwrap()), None);
    }

    #[test]
    fn not_checked_out_is_moved_in_place() {
        let (_tmp, origin, clone) = setup();
        sh(&clone, &["switch", "-q", "-c", "feature"]);
        sync(clone.to_str().unwrap()).unwrap();
        assert_eq!(head(&clone, "refs/heads/main"), head(&origin, "HEAD"));
        assert_ne!(head(&clone, "HEAD"), head(&origin, "HEAD"));
    }

    #[test]
    fn uncommitted_changes_or_own_commits_are_left_alone() {
        let (_tmp, _origin, clone) = setup();
        let before = head(&clone, "HEAD");
        std::fs::write(clone.join("a"), "edited").unwrap();
        assert!(sync(clone.to_str().unwrap()).unwrap().contains("uncommitted changes"));
        assert_eq!(head(&clone, "HEAD"), before);
        sh(&clone, &["checkout", "-q", "a"]);
        commit(&clone, "mine");
        assert!(sync(clone.to_str().unwrap()).unwrap().contains("commits of its own"));
    }

    #[test]
    fn untracked_files_dont_block() {
        let (_tmp, origin, clone) = setup();
        std::fs::write(clone.join("scratch"), "x").unwrap();
        sync(clone.to_str().unwrap()).unwrap();
        assert_eq!(head(&clone, "HEAD"), head(&origin, "HEAD"));
    }

    #[test]
    fn finds_the_worktree_on_a_branch() {
        let list = "worktree /r\nHEAD 1\nbranch refs/heads/feat\n\nworktree /r/w\nHEAD 2\nbranch refs/heads/main\n\nworktree /r/d\nHEAD 3\ndetached\n";
        assert_eq!(worktree_on(list, "refs/heads/main").as_deref(), Some("/r/w"));
        assert_eq!(worktree_on(list, "refs/heads/master"), None);
    }
}
