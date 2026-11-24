//! Telling the owner a dev build has fallen behind main. `scripts/dev-app.sh` stamps the commit it
//! built from and the checkout it came from into the daemon's environment (`TASKBOARD_DEV_COMMIT`,
//! `TASKBOARD_DEV_CHECKOUT`); a release build has neither, so nothing here runs for it. When the
//! daemon starts and once a day after, it fetches `origin/main` in that checkout and, while main has
//! commits touching the daemon that the build lacks, keeps a quiet Needs you item saying so. A rebuild
//! from main restarts the daemon on a current commit, and the item goes.

use serde_json::json;

use crate::app::App;
use crate::dispatch;
use crate::util::*;
use crate::worktrees::git;

pub const COMMIT_ENV: &str = "TASKBOARD_DEV_COMMIT";
pub const CHECKOUT_ENV: &str = "TASKBOARD_DEV_CHECKOUT";
const KEY: &str = "dev-behind";
const DAEMON_PATH: &str = "crates/taskboardd";
const FETCH_SECS: f64 = 60.0;
const GIT_SECS: f64 = 15.0;

/// How far a build sits behind main: every commit it lacks, and those that touch the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Behind {
    pub total: u64,
    pub daemon: u64,
}

/// The commit and checkout a dev build was made from, or None for a release build.
fn stamp() -> Option<(String, String)> {
    let var = |k| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    Some((var(COMMIT_ENV)?, var(CHECKOUT_ENV)?))
}

/// Fetches main and raises or clears the notice. Does nothing on a release build.
pub fn check(app: &App) {
    let Some((commit, checkout)) = stamp() else { return };
    let f = git(&checkout, &["fetch", "--quiet", "origin", "main"], FETCH_SECS);
    if f.code != 0 {
        app.info(format!("dev build check: can't fetch main in {checkout}: {}", crate::worktrees::last_line(&f.err)));
    }
    let Some(b) = behind(&checkout, &commit) else { return };
    if let Err(e) = apply(app, b) {
        app.info(format!("dev build check: {}", e.message));
    }
}

/// How far `commit` is behind `origin/main` in `checkout`, or None when git can't tell (the commit or
/// main isn't there).
pub fn behind(checkout: &str, commit: &str) -> Option<Behind> {
    let count = |extra: &[&str]| {
        let range = format!("{commit}..origin/main");
        let mut args = vec!["rev-list", "--count", range.as_str()];
        args.extend_from_slice(extra);
        let g = git(checkout, &args, GIT_SECS);
        if g.code != 0 {
            return None;
        }
        g.out.trim().parse::<u64>().ok()
    };
    Some(Behind { total: count(&[])?, daemon: count(&["--", DAEMON_PATH])? })
}

pub fn text(b: Behind) -> String {
    let commits = if b.total == 1 { "commit" } else { "commits" };
    format!("Taskboard Dev is {} {commits} behind main ({} in the daemon): run scripts/dev-app.sh", b.total, b.daemon)
}

/// Keeps the notice in line with `b`: up (with its current count) while main has daemon commits the
/// build lacks, gone otherwise.
pub fn apply(app: &App, b: Behind) -> Result<()> {
    let up = dispatch::alerts(app).into_iter().find(|a| a["key"] == KEY);
    if b.daemon == 0 {
        return if up.is_some() { dispatch::clear_alert_key(app, KEY) } else { Ok(()) };
    }
    let want = text(b);
    if up.as_ref().is_some_and(|a| a["text"] == want.as_str()) {
        return Ok(());
    }
    if up.is_some() {
        dispatch::clear_alert_key(app, KEY)?;
    }
    dispatch::add_quiet_alert(app, &want, KEY, json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    fn run(dir: &Path, args: &[&str]) -> String {
        let o = Command::new("git").current_dir(dir).args(args).output().unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    fn commit(dir: &Path, file: &str) -> String {
        let p = dir.join(file);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, file).unwrap();
        run(dir, &["add", "-A"]);
        run(dir, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", file]);
        run(dir, &["rev-parse", "HEAD"])
    }

    #[test]
    fn counts_the_commits_main_has_that_the_build_lacks() {
        let d = tempfile::tempdir().unwrap();
        run(d.path(), &["init", "-q", "-b", "main"]);
        let built = commit(d.path(), "README.md");
        commit(d.path(), "crates/taskboard-app/src/a.rs");
        commit(d.path(), "crates/taskboardd/src/b.rs");
        commit(d.path(), "crates/taskboardd/src/c.rs");
        let head = run(d.path(), &["rev-parse", "HEAD"]);
        run(d.path(), &["update-ref", "refs/remotes/origin/main", &head]);
        let repo = d.path().to_str().unwrap();
        assert_eq!(behind(repo, &built), Some(Behind { total: 3, daemon: 2 }));
        assert_eq!(behind(repo, &head), Some(Behind { total: 0, daemon: 0 }));
        assert_eq!(behind(repo, "0123456789abcdef0123456789abcdef01234567"), None, "a commit git doesn't have");
    }

    #[test]
    fn the_notice_follows_the_count_and_goes_once_caught_up() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::for_tests(crate::config::Config::for_tests(dir.path()));
        let ours = |app: &App| dispatch::alerts(app).into_iter().filter(|a| a["key"] == KEY).collect::<Vec<_>>();

        apply(&app, Behind { total: 4, daemon: 0 }).unwrap();
        assert!(ours(&app).is_empty(), "no daemon commits behind: nothing to rebuild for");

        apply(&app, Behind { total: 4, daemon: 2 }).unwrap();
        let up = ours(&app);
        assert_eq!(up.len(), 1);
        assert_eq!(up[0]["text"], "Taskboard Dev is 4 commits behind main (2 in the daemon): run scripts/dev-app.sh");
        assert_eq!(up[0]["quiet"], true, "it sits in Needs you without notifying");

        apply(&app, Behind { total: 4, daemon: 2 }).unwrap();
        assert_eq!(ours(&app)[0]["id"], up[0]["id"], "the same count keeps the same item");

        apply(&app, Behind { total: 6, daemon: 3 }).unwrap();
        let up = ours(&app);
        assert_eq!(up.len(), 1);
        assert!(up[0]["text"].as_str().unwrap().starts_with("Taskboard Dev is 6 commits"));

        apply(&app, Behind { total: 0, daemon: 0 }).unwrap();
        assert!(ours(&app).is_empty(), "rebuilt from main: the item goes");
    }

    #[test]
    fn one_commit_reads_singular() {
        assert_eq!(text(Behind { total: 1, daemon: 1 }), "Taskboard Dev is 1 commit behind main (1 in the daemon): run scripts/dev-app.sh");
    }
}
