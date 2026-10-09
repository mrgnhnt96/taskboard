//! Builds in the board's terminals: a Rust repo's terminals, and the worktree subagents they start,
//! share the main checkout's `target/` and a cap on rustc jobs, so a goal running many worktree agents
//! doesn't cold-build the same project once per worktree with every core at once.
//!
//! Both go in the `env` of the `--settings` the board passes to Claude, which reaches its Bash tools
//! and every subagent. `[builds]` in config.toml turns them off.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::app::App;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BuildsConfig {
    /// Point CARGO_TARGET_DIR at the main checkout's `target/`, shared by all its worktrees.
    pub shared_target: bool,
    /// CARGO_BUILD_JOBS for a board terminal's cargo builds; 0 leaves cargo's default (every core).
    pub jobs: i64,
}

impl Default for BuildsConfig {
    fn default() -> Self {
        BuildsConfig { shared_target: true, jobs: 4 }
    }
}

/// The main checkout of the git repo `cwd` is in (a worktree's too), and whether that checkout of it
/// is a Cargo project.
fn cargo_repo(cwd: &str) -> Option<PathBuf> {
    let git = crate::proc::which("git")?;
    let args: Vec<String> =
        ["-C", cwd, "rev-parse", "--path-format=absolute", "--show-toplevel", "--git-common-dir"].iter().map(|s| s.to_string()).collect();
    let out = crate::proc::run(&git, &args, None, 10.0).ok().filter(|o| o.code == Some(0))?;
    let mut lines = out.stdout.lines().map(str::trim);
    let top = PathBuf::from(lines.next()?);
    let common = PathBuf::from(lines.next()?);
    main_checkout(&common).filter(|_| top.join("Cargo.toml").is_file())
}

/// The main checkout from git's common dir: the folder its `.git` is in. A bare repo has none.
fn main_checkout(common: &Path) -> Option<PathBuf> {
    (common.file_name()? == ".git").then(|| common.parent().map(Path::to_path_buf)).flatten()
}

/// The build env for a terminal opened in `cwd`: empty unless it's a Cargo project.
pub fn env_for(cfg: &BuildsConfig, cwd: &str) -> Map<String, Value> {
    let mut env = Map::new();
    if !cfg.shared_target && cfg.jobs <= 0 {
        return env;
    }
    let Some(main) = cargo_repo(cwd) else { return env };
    if cfg.shared_target {
        env.insert("CARGO_TARGET_DIR".into(), json!(main.join("target").to_string_lossy()));
    }
    if cfg.jobs > 0 {
        env.insert("CARGO_BUILD_JOBS".into(), json!(cfg.jobs.to_string()));
    }
    env
}

/// The `--settings` JSON for a board terminal in `cwd`: the context limit and the build env.
pub fn settings_arg(app: &App, cwd: &str) -> Option<String> {
    let mut s = crate::limits::settings(app);
    let env = env_for(&app.cfg.builds, cwd);
    if !env.is_empty() {
        s.insert("env".into(), Value::Object(env));
    }
    (!s.is_empty()).then(|| Value::Object(s).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_main_checkout_is_where_git_s_common_dir_is() {
        assert_eq!(main_checkout(Path::new("/r/app/.git")), Some(PathBuf::from("/r/app")));
        assert_eq!(main_checkout(Path::new("/r/app.git")), None, "a bare repo has no checkout of its own");
    }

    #[test]
    fn a_cargo_worktree_shares_the_main_checkout_s_target() {
        let Some(git) = crate::proc::which("git") else { return };
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("app");
        let run = |cwd: &Path, args: &[&str]| {
            let a: Vec<String> = ["-C", cwd.to_str().unwrap()].iter().chain(args).map(|s| s.to_string()).collect();
            assert_eq!(crate::proc::run(&git, &a, None, 20.0).ok().and_then(|o| o.code), Some(0), "git {args:?}");
        };
        std::fs::create_dir_all(&repo).unwrap();
        run(&repo, &["init", "-q"]);
        std::fs::write(repo.join("Cargo.toml"), "[workspace]\n").unwrap();
        run(&repo, &["add", "."]);
        run(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"]);
        let wt = repo.join(".claude/worktrees/T4");
        run(&repo, &["worktree", "add", "-q", wt.to_str().unwrap()]);
        let main = repo.canonicalize().unwrap();

        let env = env_for(&BuildsConfig::default(), wt.to_str().unwrap());
        assert_eq!(env["CARGO_TARGET_DIR"], json!(main.join("target").to_string_lossy()));
        assert_eq!(env["CARGO_BUILD_JOBS"], json!("4"));
        assert_eq!(env_for(&BuildsConfig::default(), repo.to_str().unwrap())["CARGO_TARGET_DIR"], env["CARGO_TARGET_DIR"]);

        let off = BuildsConfig { shared_target: false, jobs: 0 };
        assert!(env_for(&off, wt.to_str().unwrap()).is_empty());
        let jobs_only = BuildsConfig { shared_target: false, jobs: 2 };
        assert_eq!(Value::Object(env_for(&jobs_only, wt.to_str().unwrap())), json!({"CARGO_BUILD_JOBS": "2"}));

        std::fs::remove_file(repo.join("Cargo.toml")).unwrap();
        assert!(env_for(&BuildsConfig::default(), repo.to_str().unwrap()).is_empty(), "not a Cargo project");
        assert!(env_for(&BuildsConfig::default(), dir.path().to_str().unwrap()).is_empty(), "not a git repo");
    }
}
