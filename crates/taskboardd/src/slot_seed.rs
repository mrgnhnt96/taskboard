//! Seeding a worktree slot's build output. A slot with no `target/` (new, or cleared by slot ageing)
//! builds every dependency from nothing; sccache covers plain library crates but not proc-macros,
//! build scripts or linking. So when a task takes such a slot, the `target/` of the free slot used
//! last (else the main checkout's) is cloned into it. An APFS clone shares the disk blocks until either
//! side writes, so it takes seconds and next to no disk, and Cargo's fingerprints keep the dependencies
//! fresh.
//!
//! The units of the repo's own packages (those Cargo.lock lists with no `source`) are left out, with
//! the incremental state and the copied-up binaries: Cargo judges a local package by file times, and the
//! source slot built it from other sources, so it must build again from this slot's. The clone is made
//! beside the slot and moved in only while the slot still has no `target/`, so a build that started
//! first is never touched. A source with a build running is passed over. `[builds] seed_slots` turns
//! it off.

use std::collections::HashSet;
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::app::App;
use crate::build_sweep::{profiles, remove, Lock};
use crate::slots;
use crate::util::*;

/// Seeds `slot`'s `target/` when it has none and the repo is a Cargo project with a lock file: on its
/// own thread, inline in tests.
pub fn seed(app: &App, repo: &str, slot: &str) {
    let at = Path::new(slot);
    if !app.cfg.builds.seed_slots || at.join("target").exists() || !at.join("Cargo.lock").is_file() {
        return;
    }
    let from = match sources(app, repo, slot) {
        Ok(f) if !f.is_empty() => f,
        _ => return,
    };
    let at = at.to_path_buf();
    if app.inline_deferred {
        if let Some(line) = seed_from(&at, &from) {
            app.info(line);
        }
        return;
    }
    std::thread::spawn(move || {
        if let Some(line) = seed_from(&at, &from) {
            crate::slowlog::line(line);
        }
    });
}

/// The build outputs to seed from, warmest first: the repo's other free slots by last use, then its
/// main checkout's.
fn sources(app: &App, repo: &str, slot: &str) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    for s in slots::rows(app, repo)? {
        let path = s.st("path");
        if path != slot && slots::holder(app, &s)?.is_none() {
            out.push(Path::new(&path).join("target"));
        }
    }
    out.push(Path::new(repo).join("target"));
    Ok(out.into_iter().filter(|t| t.is_dir()).collect())
}

/// Clones the first source no build holds into `slot`'s `target/`, less the repo's own units. The line
/// for the log, or None when there was nothing to do.
fn seed_from(slot: &Path, from: &[PathBuf]) -> Option<String> {
    let own = own_packages(&slot.join("Cargo.lock"))?;
    let target = slot.join("target");
    let tmp = slot.parent()?.join(format!(".seed-{}", slot.file_name()?.to_string_lossy()));
    remove(&tmp);
    for src in from {
        if target.exists() {
            return None;
        }
        let Some(locks) = locks(src) else { continue };
        let cloned = clone(src, &tmp);
        drop(locks);
        if let Err(e) = cloned {
            remove(&tmp);
            return Some(format!("worktrees: couldn't seed the build output of {} from {}: {e}", slot.display(), src.display()));
        }
        prune(&tmp, &own);
        if rename_new(&tmp, &target) {
            slots::resized(&slot.to_string_lossy());
            return Some(format!("worktrees: seeded the build output of {} from {}", slot.display(), src.display()));
        }
        remove(&tmp);
        return None;
    }
    None
}

/// Every profile's `.cargo-lock` under `target`, or None when a build holds one.
fn locks(target: &Path) -> Option<Vec<Lock>> {
    let mut profs = vec![];
    profiles(target, 3, &mut profs);
    profs.iter().map(|p| Lock::take(p)).collect()
}

/// The packages Cargo.lock lists with no `source`: the workspace's members and its path dependencies.
fn own_packages(lock: &Path) -> Option<HashSet<String>> {
    let v: toml::Value = toml::from_str(&fs::read_to_string(lock).ok()?).ok()?;
    let pkgs = v.get("package")?.as_array()?;
    Some(pkgs.iter().filter(|p| p.get("source").is_none()).filter_map(|p| Some(p.get("name")?.as_str()?.to_string())).collect())
}

/// Removes from a cloned target what this slot must build itself: the own packages' fingerprints and
/// build script dirs (so Cargo builds them again), every incremental dir, and the files copied up to
/// each profile's top (the built binaries), keeping its lock file.
fn prune(target: &Path, own: &HashSet<String>) {
    let mut profs = vec![];
    profiles(target, 3, &mut profs);
    for prof in profs {
        remove(&prof.join("incremental"));
        for sub in [".fingerprint", "build"] {
            for e in fs::read_dir(prof.join(sub)).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.rsplit_once('-').is_some_and(|(pkg, h)| h.len() == 16 && own.contains(pkg)) {
                    remove(&e.path());
                }
            }
        }
        for e in fs::read_dir(&prof).into_iter().flatten().flatten() {
            if e.file_type().is_ok_and(|t| !t.is_dir()) && e.file_name() != ".cargo-lock" {
                remove(&e.path());
            }
        }
    }
}

fn c_path(p: &Path) -> std::io::Result<CString> {
    CString::new(p.as_os_str().as_bytes()).map_err(std::io::Error::other)
}

/// Clones `src` to `dst` (which mustn't exist), a whole folder in one call, sharing its blocks.
#[cfg(target_os = "macos")]
fn clone(src: &Path, dst: &Path) -> std::io::Result<()> {
    let (s, d) = (c_path(src)?, c_path(dst)?);
    match unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), 0) } {
        0 => Ok(()),
        _ => Err(std::io::Error::last_os_error()),
    }
}

#[cfg(not(target_os = "macos"))]
fn clone(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "cloning needs APFS"))
}

/// Moves `from` to `to` only if nothing is at `to` yet. False when something was.
#[cfg(target_os = "macos")]
fn rename_new(from: &Path, to: &Path) -> bool {
    let (Ok(f), Ok(t)) = (c_path(from), c_path(to)) else { return false };
    unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) == 0 }
}

#[cfg(not(target_os = "macos"))]
fn rename_new(_from: &Path, _to: &Path) -> bool {
    false
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::os::unix::io::AsRawFd;

    const LOCK: &str = "[[package]]\nname = \"app\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"app-macros\"\nversion = \"0.1.0\"\n\n\
        [[package]]\nname = \"serde\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n";

    fn put(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "x").unwrap();
    }

    /// A built target: serde (a registry crate) and the repo's own app and app-macros.
    fn built(target: &Path) {
        let d = target.join("debug");
        for f in [
            ".cargo-lock",
            ".fingerprint/serde-0123456789abcdef/lib-serde",
            ".fingerprint/serde-fedcba9876543210/run-build-script-build-script-build",
            "build/serde-fedcba9876543210/output",
            "deps/libserde-0123456789abcdef.rlib",
            ".fingerprint/app-1111111111111111/lib-app",
            ".fingerprint/app-macros-2222222222222222/lib-app_macros",
            "build/app-3333333333333333/output",
            "incremental/app-1abc/s-x/dep-graph.bin",
            "app",
            "app.d",
        ] {
            put(&d.join(f));
        }
        put(&target.join("release/.fingerprint/serde-4444444444444444/lib-serde"));
        put(&target.join("release/.fingerprint/app-5555555555555555/lib-app"));
    }

    fn slot(dir: &Path) -> PathBuf {
        let slot = dir.join("worktrees/slot-2");
        fs::create_dir_all(&slot).unwrap();
        fs::write(slot.join("Cargo.lock"), LOCK).unwrap();
        slot
    }

    #[test]
    fn a_slot_is_seeded_with_the_dependencies_and_builds_its_own_packages_again() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("worktrees/slot-1/target");
        built(&src);
        let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        fs::File::options().write(true).open(src.join("debug/deps/libserde-0123456789abcdef.rlib")).unwrap().set_modified(old).unwrap();
        let slot = slot(dir.path());

        let line = seed_from(&slot, std::slice::from_ref(&src)).expect("seeded");
        assert!(line.contains("seeded the build output"), "{line}");
        let d = slot.join("target/debug");
        for kept in [".cargo-lock", ".fingerprint/serde-0123456789abcdef", "build/serde-fedcba9876543210/output", "deps/libserde-0123456789abcdef.rlib"] {
            assert!(d.join(kept).exists(), "{kept} is kept");
        }
        assert!(slot.join("target/release/.fingerprint/serde-4444444444444444").exists());
        for gone in [".fingerprint/app-1111111111111111", ".fingerprint/app-macros-2222222222222222", "build/app-3333333333333333", "incremental", "app", "app.d"] {
            assert!(!d.join(gone).exists(), "{gone} is left out");
        }
        assert!(!slot.join("target/release/.fingerprint/app-5555555555555555").exists());
        let m = fs::metadata(d.join("deps/libserde-0123456789abcdef.rlib")).unwrap();
        assert_eq!(m.modified().unwrap(), old, "file times kept, so Cargo finds it fresh");
        assert!(src.join("debug/.fingerprint/app-1111111111111111").exists() && src.join("debug/app").exists(), "the source is untouched");
        assert!(!dir.path().join("worktrees/.seed-slot-2").exists(), "nothing left beside the slot");
    }

    #[test]
    fn a_build_already_in_the_slot_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src/target");
        built(&src);
        let slot = slot(dir.path());
        put(&slot.join("target/debug/.cargo-lock"));
        assert_eq!(seed_from(&slot, &[src]), None);
        assert!(!slot.join("target/debug/.fingerprint").exists());

        let stale = dir.path().join("worktrees/.seed-slot-2");
        fs::create_dir_all(stale.join("debug")).unwrap();
        assert!(!rename_new(&stale, &slot.join("target")), "never moved over a target that's there");
    }

    #[test]
    fn a_source_with_a_build_running_is_passed_over() {
        let dir = tempfile::tempdir().unwrap();
        let (busy, idle) = (dir.path().join("busy/target"), dir.path().join("idle/target"));
        built(&busy);
        built(&idle);
        put(&idle.join("debug/deps/only-idle-0123456789abcdef"));
        let held = fs::OpenOptions::new().read(true).write(true).open(busy.join("debug/.cargo-lock")).unwrap();
        assert_eq!(unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
        let slot = slot(dir.path());
        let line = seed_from(&slot, &[busy.clone(), idle.clone()]).expect("seeded");
        assert!(line.ends_with(&idle.display().to_string()), "{line}");
        assert!(slot.join("target/debug/deps/only-idle-0123456789abcdef").exists());
    }

    #[test]
    fn a_repo_without_a_lock_file_is_not_seeded() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src/target");
        built(&src);
        let slot = slot(dir.path());
        fs::remove_file(slot.join("Cargo.lock")).unwrap();
        assert_eq!(seed_from(&slot, &[src]), None);
        assert!(!slot.join("target").exists());
    }
}
