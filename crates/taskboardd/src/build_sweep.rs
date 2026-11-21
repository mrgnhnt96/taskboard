//! Clearing stale build output, once a night. Slot ageing (`slot_ageing.rs`) only clears a slot that
//! sits free and idle, so a busy slot's `target/`, and the main checkout's, keep every crate they ever
//! built. This pass goes through every slot (held or free) and each repo's main checkout, and removes
//! the Cargo build units no build read or wrote in `[worktrees] stale_build_days` (0 never does).
//!
//! A unit is every file sharing a metadata hash under a profile's `.fingerprint/`, `build/`, `deps/`
//! and `examples/`, removed together so Cargo sees it missing and builds it again; each
//! `incremental/<crate>-<hash>` is a unit of its own. Cargo reads a unit's fingerprint on every build
//! that uses it, so a unit any recent build needed is kept. The profile's `.cargo-lock` is held while
//! it's swept, and a profile a build holds is skipped.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use crate::app::App;
use crate::util::*;
use crate::{p, slots};

const LAST_KEY: &str = "build_sweep_last";
const DAY: f64 = 86_400.0;
const GB: f64 = 1_000_000_000.0;
const UNIT_DIRS: [&str; 4] = [".fingerprint", "build", "deps", "examples"];

/// The metadata hash in a build file's name: the first `-<16 hex>` followed by the end or a `.`.
fn unit_hash(name: &str) -> Option<&str> {
    let b = name.as_bytes();
    (0..b.len()).filter(|&i| b[i] == b'-').find_map(|i| {
        let h = name.get(i + 1..i + 17)?;
        let ends = b.len() == i + 17 || b[i + 17] == b'.';
        (ends && h.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))).then_some(h)
    })
}

/// The newest read or write of a file at or under `p`. A dir's own times move when it's only listed
/// (this sweep lists them), so only files count. Unknown counts as now, so it's kept.
fn last_used(p: &Path) -> f64 {
    let Ok(m) = fs::symlink_metadata(p) else { return f64::MAX };
    if !m.is_dir() {
        return (m.atime() as f64).max(m.mtime() as f64);
    }
    fs::read_dir(p).into_iter().flatten().flatten().map(|e| last_used(&e.path())).fold(0.0, f64::max)
}

/// Bytes on disk at or under `p`, a hard-linked file's split between its links.
fn size(p: &Path) -> u64 {
    let Ok(m) = fs::symlink_metadata(p) else { return 0 };
    if m.is_dir() {
        return fs::read_dir(p).into_iter().flatten().flatten().map(|e| size(&e.path())).sum();
    }
    m.blocks() * 512 / m.nlink().max(1)
}

fn remove(p: &Path) {
    let _ = if p.is_dir() && !p.is_symlink() { fs::remove_dir_all(p) } else { fs::remove_file(p) };
}

/// The profile dirs under a target dir: those holding a `.fingerprint/`.
fn profiles(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if dir.join(".fingerprint").is_dir() {
        out.push(dir.to_path_buf());
        return;
    }
    if depth == 0 {
        return;
    }
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            profiles(&e.path(), depth - 1, out);
        }
    }
}

/// Holds a profile's `.cargo-lock` while it lives; None when a build holds it.
struct Lock(Option<fs::File>);

impl Lock {
    fn take(profile: &Path) -> Option<Lock> {
        let path = profile.join(".cargo-lock");
        if !path.exists() {
            return Some(Lock(None));
        }
        let f = fs::OpenOptions::new().read(true).write(true).open(&path).ok()?;
        (unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(Lock(Some(f)))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        if let Some(f) = &self.0 {
            unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

/// What a sweep of one target dir did.
#[derive(Debug, Default, PartialEq)]
pub struct Swept {
    pub bytes: u64,
    pub units: usize,
    pub busy: usize,
}

/// Removes the units under `target` nothing used since `cutoff` (a timestamp).
pub fn sweep_target(target: &Path, cutoff: f64) -> Swept {
    let mut out = Swept::default();
    let mut profs = vec![];
    profiles(target, 3, &mut profs);
    for prof in profs {
        let Some(_lock) = Lock::take(&prof) else {
            out.busy += 1;
            continue;
        };
        let mut units: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for sub in UNIT_DIRS {
            for e in fs::read_dir(prof.join(sub)).into_iter().flatten().flatten() {
                if let Some(h) = unit_hash(&e.file_name().to_string_lossy()) {
                    units.entry(h.to_string()).or_default().push(e.path());
                }
            }
        }
        for e in fs::read_dir(prof.join("incremental")).into_iter().flatten().flatten() {
            units.insert(format!("incremental/{}", e.file_name().to_string_lossy()), vec![e.path()]);
        }
        for paths in units.values() {
            if paths.iter().map(|p| last_used(p)).fold(0.0, f64::max) >= cutoff {
                continue;
            }
            out.bytes += paths.iter().map(|p| size(p)).sum::<u64>();
            out.units += 1;
            paths.iter().for_each(|p| remove(p));
        }
    }
    out
}

/// Every slot's checkout and each repo's main checkout: (checkout, is a slot).
fn checkouts(app: &App) -> Result<Vec<(String, bool)>> {
    let mut out: Vec<(String, bool)> = vec![];
    for r in app.db.q("SELECT DISTINCT repo FROM worktree_slots ORDER BY repo", p![])? {
        out.push((r.st("repo"), false));
    }
    for s in app.db.q("SELECT path FROM worktree_slots ORDER BY path", p![])? {
        out.push((s.st("path"), true));
    }
    Ok(out)
}

/// Sweeps every checkout's `target/`. Returns the bytes removed.
pub fn sweep(app: &App) -> Result<u64> {
    let days = app.cfg.worktrees.stale_build_days;
    if days <= 0 {
        return Ok(0);
    }
    let cutoff = now_ts() - days as f64 * DAY;
    let mut total = 0;
    for (path, slot) in checkouts(app)? {
        let target = Path::new(&path).join("target");
        if !target.is_dir() {
            continue;
        }
        let s = sweep_target(&target, cutoff);
        if s.units > 0 {
            app.info(format!("build sweep: removed {} build units unused {days} days ({:.1} GB) from {}", s.units, s.bytes as f64 / GB, target.display()));
            if slot {
                slots::resized(&path);
            }
        }
        if s.busy > 0 {
            app.info(format!("build sweep: skipped {} profiles of {} a build holds", s.busy, target.display()));
        }
        total += s.bytes;
    }
    app.db.set_setting(LAST_KEY, Some(&now_iso()))?;
    Ok(total)
}

/// Once a day, after 3 AM local time, when builds are fewest.
pub fn sweep_loop(app: std::sync::Arc<App>) {
    use chrono::Timelike;
    while !app.stopping() {
        let now = local_now();
        let last = app.db.get_setting(LAST_KEY).ok().flatten().and_then(|s| parse_iso(&s)).map(|t| local_dt(t).date_naive());
        if now.hour() >= 3 && last != Some(now.date_naive()) {
            if let Err(e) = sweep(&app) {
                app.info(format!("build sweep failed: {}", e.message));
            }
        }
        app.sleep(30.0 * 60.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn put(p: &Path, age_days: u64) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![0u8; 8192]).unwrap();
        let t = SystemTime::now() - Duration::from_secs(age_days * 86_400);
        fs::File::options().write(true).open(p).unwrap().set_times(fs::FileTimes::new().set_accessed(t).set_modified(t)).unwrap();
    }

    #[test]
    fn reads_the_metadata_hash_from_build_file_names() {
        assert_eq!(unit_hash("libserde-0123456789abcdef.rlib"), Some("0123456789abcdef"));
        assert_eq!(unit_hash("midna_app-0123456789abcdef"), Some("0123456789abcdef"));
        assert_eq!(unit_hash("foo-bar-0123456789abcdef.foo.1a2b-cgu.0.rcgu.o"), Some("0123456789abcdef"));
        assert_eq!(unit_hash("midna-app"), None);
        assert_eq!(unit_hash("lib-0123456789ABCDEF.rlib"), None);
    }

    #[test]
    fn removes_whole_units_nothing_used_lately_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let debug = dir.path().join("target/debug");
        let (old, new) = ("aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb");
        put(&debug.join(format!(".fingerprint/serde-{old}/lib-serde")), 10);
        put(&debug.join(format!("deps/libserde-{old}.rlib")), 10);
        put(&debug.join(format!("build/serde-{old}/output")), 10);
        put(&debug.join(format!(".fingerprint/tokio-{new}/lib-tokio")), 1);
        put(&debug.join(format!("deps/libtokio-{new}.rlib")), 10);
        put(&debug.join("incremental/serde-1x2y/s-old/dep-graph.bin"), 10);
        put(&debug.join("incremental/tokio-3z4w/s-new/dep-graph.bin"), 0);
        put(&debug.join("midna-app"), 10);

        let cutoff = now_ts() - 3.0 * DAY;
        let s = sweep_target(&dir.path().join("target"), cutoff);

        assert_eq!(s.units, 2, "serde's unit and its incremental dir");
        assert!(s.bytes > 0);
        assert!(!debug.join(format!(".fingerprint/serde-{old}")).exists());
        assert!(!debug.join(format!("deps/libserde-{old}.rlib")).exists());
        assert!(!debug.join(format!("build/serde-{old}")).exists());
        assert!(!debug.join("incremental/serde-1x2y").exists());
        assert!(debug.join(format!("deps/libtokio-{new}.rlib")).exists(), "an old rlib whose fingerprint a build read stays");
        assert!(debug.join("incremental/tokio-3z4w").exists());
        assert!(debug.join("midna-app").exists(), "files without a hash aren't touched");
    }

    #[test]
    fn skips_a_profile_a_build_holds() {
        let dir = tempfile::tempdir().unwrap();
        let debug = dir.path().join("target/debug");
        put(&debug.join(".fingerprint/serde-aaaaaaaaaaaaaaaa/lib-serde"), 10);
        put(&debug.join(".cargo-lock"), 0);
        let held = Lock::take(&debug).unwrap();

        let s = sweep_target(&dir.path().join("target"), now_ts() - 3.0 * DAY);

        assert_eq!(s, Swept { bytes: 0, units: 0, busy: 1 });
        assert!(debug.join(".fingerprint/serde-aaaaaaaaaaaaaaaa").exists());
        drop(held);
    }
}
