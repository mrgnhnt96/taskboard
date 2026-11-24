//! Builds in the board's terminals: a Rust repo's terminals, and the worktree subagents they start,
//! share the main checkout's `target/`, so a goal running many worktree agents doesn't cold-build the
//! same project once per worktree. A slot from the worktree pool (`slots.rs`) keeps its own `target/`
//! instead: it lives on between tasks, so its build stays warm, and two slots building at once never
//! wait on each other's cargo lock.
//!
//! Midna lets at most `guard.build_slots` builds run at once across every terminal (the rest wait in
//! line), so each build gets the cores divided by that many: cargo's jobs (CARGO_BUILD_JOBS) and
//! Gradle's workers (`org.gradle.workers.max`), which Android and Flutter builds run through.
//!
//! These go in the `env` of the `--settings` the board passes to Claude, which reaches its Bash tools
//! and every subagent. `[builds]` in config.toml turns them off or sets the jobs itself.

use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::app::App;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BuildsConfig {
    /// Point CARGO_TARGET_DIR at the main checkout's `target/`, shared by all its worktrees (a pool
    /// slot's own `target/` for a slot).
    pub shared_target: bool,
    /// Jobs for each build in a board terminal (CARGO_BUILD_JOBS, Gradle's workers): unset for the
    /// cores divided by Midna's `guard.build_slots`, 0 for each tool's default (every core).
    pub jobs: Option<i64>,
    /// Give a slot with no `target/` a clone of a warm one when a task takes it (`slot_seed.rs`).
    pub seed_slots: bool,
}

impl Default for BuildsConfig {
    fn default() -> Self {
        BuildsConfig { shared_target: true, jobs: None, seed_slots: true }
    }
}

/// Midna's `guard.build_slots` default, for when the board hasn't read it yet.
const MIDNA_BUILD_SLOTS: i64 = 2;

/// The jobs each build gets on a Mac with `cores`, while Midna runs `slots` builds at once (`None`:
/// not known yet). `None` when nothing caps them: `jobs = 0`, or Midna's slots set to no limit.
pub fn jobs(cfg: &BuildsConfig, cores: usize, slots: Option<i64>) -> Option<i64> {
    match cfg.jobs {
        Some(n) => (n > 0).then_some(n),
        None => {
            let slots = slots.unwrap_or(MIDNA_BUILD_SLOTS);
            (slots > 0).then(|| (cores as i64 / slots).max(1))
        }
    }
}

fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// GRADLE_OPTS with Gradle's workers capped, after any the daemon was started with.
fn gradle_opts(jobs: i64, own: Option<&str>) -> String {
    let cap = format!("-Dorg.gradle.workers.max={jobs}");
    match own.map(str::trim).filter(|o| !o.is_empty()) {
        Some(o) => format!("{o} {cap}"),
        None => cap,
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

/// The build env for a terminal opened in `cwd` whose builds each get `jobs`: the shared `target/`
/// in a Cargo project, the job caps anywhere.
pub fn env_for(cfg: &BuildsConfig, cwd: &str, jobs: Option<i64>) -> Map<String, Value> {
    let mut env = Map::new();
    if let Some(n) = jobs {
        env.insert("CARGO_BUILD_JOBS".into(), json!(n.to_string()));
        env.insert("GRADLE_OPTS".into(), json!(gradle_opts(n, std::env::var("GRADLE_OPTS").ok().as_deref())));
    }
    if !cfg.shared_target {
        return env;
    }
    let Some(main) = cargo_repo(cwd) else { return env };
    let root = crate::worktrees::root_of(Some(cwd)).filter(|r| crate::slots::is_slot(r)).map(PathBuf::from).unwrap_or(main);
    env.insert("CARGO_TARGET_DIR".into(), json!(root.join("target").to_string_lossy()));
    env
}

static CARGO_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bcargo\b").unwrap());

/// The pool slot's `target/` for a Bash `command` run from `cwd` that builds in a slot. The terminal's
/// env (`env_for`) names the target of the folder it opened in, so one that opened in the shared
/// checkout and then ran `tb slot claim` would build the slot's code into the main checkout's target.
/// Its folder is `cwd` after each `cd` before its first `cargo` (all of them when it names none).
/// None outside a slot, in a slot that isn't a Cargo project, or with `shared_target` off.
pub fn slot_target(cfg: &BuildsConfig, command: &str, cwd: &str) -> Option<String> {
    if !cfg.shared_target || cwd.is_empty() {
        return None;
    }
    let before = CARGO_RE.find(command).map_or(command, |m| &command[..m.start()]);
    let place = crate::reports::cd_place(before, cwd);
    let root = crate::worktrees::root_of(Some(&place)).filter(|r| crate::slots::is_slot(r))?;
    Path::new(&root).join("Cargo.toml").is_file().then(|| format!("{root}/target"))
}

/// The `--settings` JSON for a board terminal in `cwd`: the context limit and the build env.
pub fn settings_arg(app: &App, cwd: &str) -> Option<String> {
    let mut s = crate::limits::settings(app);
    let slots = app.shared.lock().midna_build_slots;
    let env = env_for(&app.cfg.builds, cwd, jobs(&app.cfg.builds, cores(), slots));
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
    fn each_build_gets_the_cores_split_across_midna_s_build_slots() {
        let auto = BuildsConfig::default();
        assert_eq!(jobs(&auto, 12, Some(2)), Some(6));
        assert_eq!(jobs(&auto, 12, Some(3)), Some(4));
        assert_eq!(jobs(&auto, 12, None), Some(6), "Midna's default of 2 until the board has read it");
        assert_eq!(jobs(&auto, 4, Some(16)), Some(1), "at least one job each");
        assert_eq!(jobs(&auto, 12, Some(0)), None, "no cap when Midna lets every build run");
        assert_eq!(jobs(&BuildsConfig { jobs: Some(4), ..auto.clone() }, 12, Some(3)), Some(4), "jobs in config.toml wins");
        assert_eq!(jobs(&BuildsConfig { jobs: Some(0), ..auto }, 12, Some(3)), None, "0 leaves each tool's default");
    }

    #[test]
    fn gradle_s_workers_are_capped_after_the_daemon_s_own_options() {
        assert_eq!(gradle_opts(6, None), "-Dorg.gradle.workers.max=6");
        assert_eq!(gradle_opts(6, Some("  ")), "-Dorg.gradle.workers.max=6");
        assert_eq!(gradle_opts(2, Some("-Xmx2g")), "-Xmx2g -Dorg.gradle.workers.max=2");
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

        let on = BuildsConfig::default();
        let env = env_for(&on, wt.to_str().unwrap(), Some(4));
        assert_eq!(env["CARGO_TARGET_DIR"], json!(main.join("target").to_string_lossy()));
        assert_eq!(env["CARGO_BUILD_JOBS"], json!("4"));
        assert!(env["GRADLE_OPTS"].as_str().unwrap().ends_with("-Dorg.gradle.workers.max=4"), "{env:?}");
        assert_eq!(env_for(&on, repo.to_str().unwrap(), None)["CARGO_TARGET_DIR"], env["CARGO_TARGET_DIR"]);

        let off = BuildsConfig { shared_target: false, jobs: Some(0), ..BuildsConfig::default() };
        assert!(env_for(&off, wt.to_str().unwrap(), None).is_empty());
        let env = env_for(&off, wt.to_str().unwrap(), Some(2));
        assert_eq!(env["CARGO_BUILD_JOBS"], json!("2"));
        assert!(env.get("CARGO_TARGET_DIR").is_none());

        let slot = repo.join(".claude/worktrees/slot-2");
        run(&repo, &["worktree", "add", "-q", "--detach", slot.to_str().unwrap()]);
        std::fs::create_dir_all(slot.join("crates/app")).unwrap();
        let env = env_for(&on, slot.join("crates/app").to_str().unwrap(), None);
        assert_eq!(env["CARGO_TARGET_DIR"], json!(slot.join("target").to_string_lossy()), "a slot builds in its own target");

        // A terminal that opened in the shared checkout, then claimed the slot: its commands there build in the slot's target.
        let (main_dir, slot_dir, sub) = (repo.to_str().unwrap(), slot.to_str().unwrap(), slot.join("crates/app"));
        let want = Some(format!("{slot_dir}/target"));
        assert_eq!(slot_target(&on, "cargo test -p app", sub.to_str().unwrap()), want, "cd'd into the slot");
        assert_eq!(slot_target(&on, &format!("cd {slot_dir} && cargo build"), main_dir), want, "cd's there in the command");
        assert_eq!(slot_target(&on, "cd .claude/worktrees/slot-2/crates/app; cargo build", main_dir), want, "a relative cd");
        assert_eq!(slot_target(&on, &format!("cd {slot_dir} && ./scripts/check.sh"), main_dir), want, "a script that may run cargo");
        assert_eq!(slot_target(&on, "cargo build", main_dir), None, "the shared checkout keeps its env");
        assert_eq!(slot_target(&on, &format!("cargo build && cd {slot_dir}"), main_dir), None, "it built before the cd");
        assert_eq!(slot_target(&on, "cargo build", wt.to_str().unwrap()), None, "a worktree that isn't a slot");
        assert_eq!(slot_target(&on, "cargo build", ""), None);
        assert_eq!(slot_target(&off, "cargo build", slot_dir), None, "shared_target off");

        std::fs::remove_file(repo.join("Cargo.toml")).unwrap();
        assert!(env_for(&on, repo.to_str().unwrap(), None).is_empty(), "not a Cargo project");
        assert!(env_for(&on, dir.path().to_str().unwrap(), None).is_empty(), "not a git repo");
        let env = env_for(&on, dir.path().to_str().unwrap(), Some(3));
        assert_eq!(env["CARGO_BUILD_JOBS"], json!("3"), "the jobs are capped outside a Cargo project too");
    }
}
