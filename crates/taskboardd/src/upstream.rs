//! Keeps a task's branch tracking a branch it can sync from. A tool that pulls a branch from its
//! upstream (a session-start hook running `git pull --rebase`) rebases it onto whatever it tracks. A
//! branch made with `git switch -c <branch> origin/main` tracks main. If the task's PR goes into another
//! branch (a stack's head branch, or a base set on the task), such a pull rebases the work off that
//! branch and puts its commits back under the task's as new ones. The board points the branch at the
//! one its PR goes into instead. A branch with no upstream, or one tracking its own copy, is left alone.

use crate::app::App;
use crate::util::*;
use crate::worktrees::git;

/// Git calls here run inside a hook's report, so they're short.
const SECS: f64 = 5.0;

/// Points the branch checked out in `dir` at `<remote>/<base>` when it tracks a branch that's neither
/// its own copy nor `base`. Returns what it did, for the task's log. It does nothing on a detached HEAD,
/// on `base` itself, with no upstream or a local one, or when `<remote>/<base>` isn't fetched.
pub fn retarget(dir: &str, base: &str) -> Option<String> {
    let out = |args: &[&str]| Some(git(dir, args, SECS)).filter(|g| g.code == 0).map(|g| g.out.trim().to_string()).filter(|s| !s.is_empty());
    let branch = out(&["symbolic-ref", "--short", "-q", "HEAD"])?;
    if branch == base {
        return None;
    }
    let remote = out(&["config", &format!("branch.{branch}.remote")]).filter(|r| r != ".")?;
    let merge = out(&["config", &format!("branch.{branch}.merge")])?;
    let tracked = merge.strip_prefix("refs/heads/").unwrap_or(&merge).to_string();
    if tracked == branch || tracked == base {
        return None;
    }
    out(&["rev-parse", "--verify", "-q", &format!("refs/remotes/{remote}/{base}")])?;
    if git(dir, &["branch", "-q", &format!("--set-upstream-to={remote}/{base}"), &branch], SECS).code != 0 {
        return None;
    }
    Some(format!(
        "Pointed {branch}'s upstream at {remote}/{base}, the branch its PR goes into: it tracked {remote}/{tracked}, so a pull would rebase it off {base}"
    ))
}

/// `retarget` for the task's branch in `dir`, onto the branch its PR goes into.
pub fn mend(app: &App, t: &Row, dir: &str) -> Result<Option<String>> {
    let base = crate::steps::real_base(app, t)?;
    Ok(retarget(dir, &base))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &std::path::Path, args: &[&str]) -> String {
        let g = git(dir.to_str().unwrap(), args, 30.0);
        assert_eq!(g.code, 0, "git {args:?}: {}", g.err);
        g.out.trim().to_string()
    }

    /// A clone of a remote with `main` and a head branch `feat/head` one commit ahead of it.
    fn clone() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let (origin, work) = (tmp.path().join("origin.git"), tmp.path().join("work"));
        run(tmp.path(), &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        run(tmp.path(), &["clone", "-q", origin.to_str().unwrap(), work.to_str().unwrap()]);
        let commit = |m: &str| run(&work, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", m]);
        run(&work, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        commit("one");
        run(&work, &["push", "-q", "origin", "main"]);
        run(&work, &["switch", "-q", "-c", "feat/head"]);
        commit("head");
        run(&work, &["push", "-q", "origin", "feat/head"]);
        (tmp, work)
    }

    fn upstream(dir: &std::path::Path) -> String {
        run(dir, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
    }

    #[test]
    fn a_stacked_branch_tracking_main_is_pointed_at_its_head_branch() {
        let (_tmp, work) = clone();
        // Made from main as agents do, then moved onto the head branch: it still tracks main.
        run(&work, &["switch", "-q", "-c", "feat/child", "origin/main"]);
        run(&work, &["reset", "-q", "--hard", "origin/feat/head"]);
        assert_eq!(upstream(&work), "origin/main");
        let said = retarget(work.to_str().unwrap(), "feat/head").expect("it tracked main");
        assert!(said.contains("Pointed feat/child's upstream at origin/feat/head") && said.contains("tracked origin/main"), "{said}");
        assert_eq!(upstream(&work), "origin/feat/head");
        // A pull now keeps it on its head branch.
        run(&work, &["-c", "user.name=t", "-c", "user.email=t@t", "pull", "-q", "--rebase"]);
        assert_eq!(git(work.to_str().unwrap(), &["merge-base", "--is-ancestor", "origin/feat/head", "HEAD"], 30.0).code, 0);
        assert_eq!(retarget(work.to_str().unwrap(), "feat/head"), None, "already on its base: nothing to do");
    }

    #[test]
    fn a_branch_tracking_its_own_copy_or_nothing_or_on_its_base_is_left_alone() {
        let (_tmp, work) = clone();
        let dir = work.to_str().unwrap();
        run(&work, &["switch", "-q", "-c", "feat/child", "origin/feat/head"]);
        run(&work, &["push", "-q", "-u", "origin", "feat/child"]);
        assert_eq!(retarget(dir, "feat/head"), None, "its own copy");
        assert_eq!(upstream(&work), "origin/feat/child");
        run(&work, &["switch", "-q", "--no-track", "-c", "feat/loose", "origin/main"]);
        assert_eq!(retarget(dir, "feat/head"), None, "no upstream");
        run(&work, &["switch", "-q", "main"]);
        assert_eq!(retarget(dir, "main"), None, "the base itself");
        run(&work, &["switch", "-q", "--detach"]);
        assert_eq!(retarget(dir, "feat/head"), None, "detached");
    }

    #[test]
    fn a_base_origin_lacks_is_never_tracked() {
        let (_tmp, work) = clone();
        run(&work, &["switch", "-q", "-c", "feat/child", "origin/main"]);
        assert_eq!(retarget(work.to_str().unwrap(), "feat/gone"), None);
        assert_eq!(upstream(&work), "origin/main");
    }
}
