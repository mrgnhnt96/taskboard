//! Updates: checks GitHub for a newer release, downloads its DMG, checks it and swaps it in, like
//! Midna's updater.
//!
//! - **Feed**: the repo's releases (`RELEASES`), the same list the website's Download button reads.
//!   A beta build follows every release, prereleases included; a public one only public releases.
//!   The newest one newer than this build, with a DMG attached, is the update.
//! - **Checked**: the DMG's sha256 against the digest GitHub keeps for the asset, then the app inside:
//!   same bundle id, the version the release says, a valid code signature, notarized, and signed by
//!   the same Developer ID team as the app running now. Anything else is refused.
//! - **Applied**: the staged copy is exchanged with the installed bundle by `renamex_np(RENAME_SWAP)`,
//!   so `Taskboard.app` is always the whole old or the whole new app. The daemon is restarted on the
//!   new binary and the app relaunches.
//!
//! Only an installed, Developer ID-signed Taskboard.app updates itself: Taskboard Dev (ad-hoc), a
//! `cargo run` and the sample board never do.
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES: &str = "https://api.github.com/repos/mrgnhnt96/taskboard/releases?per_page=30";
pub const RELEASES_PAGE: &str = "https://github.com/mrgnhnt96/taskboard/releases";
const BUNDLE_ID: &str = "com.mrgnhnt.taskboard";
const FIRST_CHECK: Duration = Duration::from_secs(20);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// This build doesn't update itself, and why.
    Off(String),
    Idle,
    Checking,
    UpToDate { at: String },
    Downloading { version: String },
    /// Downloaded, checked and staged; applied on restart.
    Ready { version: String, page: String },
    Applying { version: String },
    Failed { error: String, at: String },
}

static STATE: Mutex<State> = Mutex::new(State::Idle);
static WAKE: OnceLock<Mutex<Sender<()>>> = OnceLock::new();
/// The staged app and the installed bundle it replaces.
static STAGED: Mutex<Option<(PathBuf, PathBuf)>> = Mutex::new(None);

pub fn state() -> State {
    STATE.lock().map(|s| s.clone()).unwrap_or(State::Idle)
}

#[cfg(test)]
pub fn set_for_test(s: State) {
    set(s)
}

fn set(s: State) {
    if let Ok(mut g) = STATE.lock() {
        *g = s;
    }
}

/// "3:05 PM", the time a check finished.
fn now() -> String {
    chrono::Local::now().format("%-I:%M %p").to_string()
}

/// A release that updates this build.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub dmg: String,
    pub size: u64,
    /// The asset's sha256 as GitHub reports it, when it does.
    pub sha256: Option<String>,
}

/// `1.2.0` or `1.2.0-beta.3` as a sortable key; a beta sorts before its release
/// (`scripts/newest-version.py` orders them the same way).
pub fn version_key(v: &str) -> Option<(u64, u64, u64, bool, u64)> {
    let v = v.strip_prefix('v').unwrap_or(v);
    let (core, beta) = match v.split_once("-beta.") {
        Some((c, b)) => (c, Some(b.parse().ok()?)),
        None => (v, None),
    };
    let mut it = core.split('.').map(|p| p.parse::<u64>());
    let (a, b, c) = (it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?);
    if it.next().is_some() {
        return None;
    }
    Some((a, b, c, beta.is_none(), beta.unwrap_or(0)))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((version_key(candidate), version_key(current)), (Some(a), Some(b)) if a > b)
}

/// The newest release in GitHub's list that updates `current`: a beta build takes prereleases too.
pub fn pick(releases: &Value, current: &str) -> Option<Release> {
    let beta = current.contains("-beta.");
    releases
        .as_array()?
        .iter()
        .filter(|r| r["draft"] != true && (beta || r["prerelease"] != true))
        .filter_map(|r| {
            let version = r["tag_name"].as_str()?.trim_start_matches('v').to_string();
            let asset = r["assets"].as_array()?.iter().find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(".dmg")))?;
            Some(Release {
                page: r["html_url"].as_str().unwrap_or(RELEASES_PAGE).to_string(),
                dmg: asset["browser_download_url"].as_str()?.to_string(),
                size: asset["size"].as_u64().unwrap_or(0),
                sha256: asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_ascii_lowercase),
                version,
            })
        })
        .filter(|r| is_newer(&r.version, current))
        .max_by_key(|r| version_key(&r.version))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(60))
        .user_agent(&format!("Taskboard/{VERSION}"))
        .build()
}

fn fetch_releases() -> Result<Value, String> {
    let url = std::env::var("TASKBOARD_RELEASES_URL").unwrap_or_else(|_| RELEASES.into());
    let resp = agent().get(&url).set("Accept", "application/vnd.github+json").call().map_err(|e| format!("Couldn't reach GitHub: {e}"))?;
    let mut body = String::new();
    resp.into_reader().take(8 << 20).read_to_string(&mut body).map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|e| format!("GitHub's release list didn't parse: {e}"))
}

/// Download the DMG to `dir`, hashing as it goes; refused unless it matches GitHub's digest.
fn download(r: &Release, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let dest = dir.join(format!("Taskboard-{}.dmg", r.version));
    let part = dir.join(format!("Taskboard-{}.dmg.part", r.version));
    let resp = agent().get(&r.dmg).call().map_err(|e| format!("Download failed: {e}"))?;
    let mut reader = resp.into_reader();
    let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 1 << 16];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("Download failed: {e}"))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > 1 << 30 {
            let _ = std::fs::remove_file(&part);
            return Err("The download is larger than 1 GB.".into());
        }
        hash.update(&buf[..n]);
        std::io::Write::write_all(&mut f, &buf[..n]).map_err(|e| e.to_string())?;
    }
    drop(f);
    let got: String = hash.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect();
    if r.sha256.as_ref().is_some_and(|want| *want != got) || (r.size > 0 && r.size != total) {
        let _ = std::fs::remove_file(&part);
        return Err("The download doesn't match the release on GitHub.".into());
    }
    std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// An Info.plist as JSON.
fn read_plist(path: &Path) -> Option<Value> {
    let o = Command::new("/usr/bin/plutil").args(["-convert", "json", "-o", "-"]).arg(path).output().ok()?;
    o.status.success().then(|| serde_json::from_slice(&o.stdout).ok()).flatten()
}

/// The Developer ID team a bundle is signed by (None: ad-hoc or unsigned).
pub fn team_id(app: &Path) -> Option<String> {
    let o = Command::new("/usr/bin/codesign").args(["-dv", "--verbose=2"]).arg(app).output().ok()?;
    String::from_utf8_lossy(&o.stderr).lines().find_map(|l| l.strip_prefix("TeamIdentifier=")).filter(|t| *t != "not set").map(str::to_string)
}

/// Copy the app out of the DMG into `dir` and check it (see the module docs).
fn stage(dmg: &Path, dir: &Path, version: &str, installed: &Path) -> Result<PathBuf, String> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mnt = dir.join("mnt");
    let attached = Command::new("/usr/bin/hdiutil").args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint"]).arg(&mnt).arg(dmg).output().map_err(|e| e.to_string())?;
    if !attached.status.success() {
        return Err(format!("Couldn't open the DMG: {}", String::from_utf8_lossy(&attached.stderr).trim()));
    }
    let found = std::fs::read_dir(&mnt).ok().into_iter().flatten().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "app"));
    let app = dir.join("Taskboard.app");
    let copied = found.as_ref().map(|src| Command::new("/usr/bin/ditto").arg(src).arg(&app).status().is_ok_and(|s| s.success()));
    let _ = Command::new("/usr/bin/hdiutil").args(["detach", "-force"]).arg(&mnt).output();
    match copied {
        None => return Err("The DMG has no app in it.".into()),
        Some(false) => return Err("Couldn't copy the app out of the DMG.".into()),
        Some(true) => {}
    }
    let info = read_plist(&app.join("Contents/Info.plist")).ok_or("The new app has no Info.plist.")?;
    let id = info["CFBundleIdentifier"].as_str().unwrap_or("");
    if id != BUNDLE_ID {
        return Err(format!("The new app is {id}, not {BUNDLE_ID}."));
    }
    let v = info["CFBundleShortVersionString"].as_str().unwrap_or("");
    if v != version {
        return Err(format!("The new app says it's {v}; the release is {version}."));
    }
    let o = Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(&app).output().map_err(|e| e.to_string())?;
    if !o.status.success() {
        return Err(format!("The new app's code signature is invalid: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    let o = Command::new("/usr/sbin/spctl").args(["--assess", "--type", "execute"]).arg(&app).output().map_err(|e| e.to_string())?;
    if !o.status.success() {
        return Err(format!("macOS doesn't accept the new app: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    let (want, got) = (team_id(installed), team_id(&app));
    if want.is_none() || want != got {
        return Err(format!("The new app is signed by {}, this one by {}.", got.as_deref().unwrap_or("nobody"), want.as_deref().unwrap_or("nobody")));
    }
    Ok(app)
}

/// Why this build doesn't update itself, if it doesn't.
fn off_reason(backend: &str) -> Option<String> {
    if backend == "fake" {
        return Some("The sample board doesn't update.".into());
    }
    let Some(bundle) = crate::install::installed_bundle() else {
        return Some("A dev build: it isn't running from Taskboard.app.".into());
    };
    if read_plist(&bundle.join("Contents/Info.plist")).is_none_or(|i| i["CFBundleIdentifier"] != BUNDLE_ID) {
        return Some("Taskboard Dev updates with scripts/dev-app.sh.".into());
    }
    if team_id(&bundle).is_none() {
        return Some("This copy isn't a signed release build.".into());
    }
    None
}

fn cache_dir() -> PathBuf {
    taskboardd::util::expand_home("~/Library/Caches/com.mrgnhnt.taskboard/updates")
}

/// One check: find a newer release, then download, check and stage it.
fn check(bundle: &Path) {
    let ready = match state() {
        State::Ready { version, .. } => Some(version),
        _ => None,
    };
    if ready.is_none() {
        set(State::Checking);
    }
    let fail = |error: String| set(State::Failed { error, at: now() });
    let releases = match fetch_releases() {
        Ok(r) => r,
        Err(e) if ready.is_none() => return fail(e),
        Err(_) => return,
    };
    let Some(r) = pick(&releases, VERSION) else {
        return set(State::UpToDate { at: now() });
    };
    if ready.as_deref().is_some_and(|v| !is_newer(&r.version, v)) {
        return;
    }
    set(State::Downloading { version: r.version.clone() });
    let dir = cache_dir();
    let staged = download(&r, &dir).and_then(|dmg| {
        let app = stage(&dmg, &dir.join("staged"), &r.version, bundle);
        let _ = std::fs::remove_file(&dmg);
        app
    });
    match staged {
        Ok(app) => {
            if let Ok(mut g) = STAGED.lock() {
                *g = Some((app, bundle.to_path_buf()));
            }
            set(State::Ready { version: r.version, page: r.page });
        }
        Err(e) => fail(e),
    }
}

/// Start checking: soon after launch, then every few hours, and whenever [`check_now`] asks.
pub fn start(backend: &str) {
    if let Some(why) = off_reason(backend) {
        return set(State::Off(why));
    }
    let Some(bundle) = crate::install::installed_bundle() else { return };
    let (tx, rx): (Sender<()>, Receiver<()>) = channel();
    let _ = WAKE.set(Mutex::new(tx));
    let _ = std::thread::Builder::new().name("updater".into()).spawn(move || {
        let mut wait = FIRST_CHECK;
        loop {
            match rx.recv_timeout(wait) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return,
            }
            if !matches!(state(), State::Applying { .. }) {
                check(&bundle);
            }
            wait = CHECK_EVERY;
        }
    });
}

/// Check now (Settings › System › Check now).
pub fn check_now() {
    if let Some(tx) = WAKE.get().and_then(|m| m.lock().ok()) {
        if !matches!(state(), State::Checking | State::Downloading { .. } | State::Applying { .. }) {
            set(State::Checking);
        }
        let _ = tx.send(());
    }
}

/// Swap the staged app in and restart the daemon on it. Blocking; the caller quits the app after,
/// and the relauncher started here opens the new one once this process is gone.
pub fn apply() -> Result<(), String> {
    let (staged, installed) = STAGED.lock().ok().and_then(|g| g.clone()).ok_or("No update is ready.")?;
    let version = match state() {
        State::Ready { version, .. } => version,
        _ => return Err("No update is ready.".into()),
    };
    set(State::Applying { version: version.clone() });
    let done = swap_in(&staged, &installed).and_then(|_| {
        if let Err(e) = crate::install::restart_daemon(&installed) {
            eprintln!("taskboard-app: restarting the daemon after the update: {e}");
        }
        Command::new("/bin/sh")
            .args(["-c", "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; /usr/bin/open \"$2\"", "relaunch"])
            .arg(std::process::id().to_string())
            .arg(&installed)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Couldn't relaunch: {e}"))
    });
    if let Err(e) = &done {
        set(State::Failed { error: e.clone(), at: now() });
    }
    done
}

/// Move `staged` next to `installed` (same volume), exchange the two atomically and delete the old one.
fn swap_in(staged: &Path, installed: &Path) -> Result<(), String> {
    let parent = installed.parent().ok_or("The app has no parent folder.")?;
    let side = parent.join(format!(".Taskboard.update-{}.app", std::process::id()));
    let _ = std::fs::remove_dir_all(&side);
    if std::fs::rename(staged, &side).is_err() {
        // another volume: copy, keeping signatures and extended attributes
        if !Command::new("/usr/bin/ditto").arg(staged).arg(&side).status().is_ok_and(|s| s.success()) {
            let _ = std::fs::remove_dir_all(&side);
            return Err(format!("Can't write next to {} (is the folder writable?)", installed.display()));
        }
    }
    swap(&side, installed).inspect_err(|_| {
        let _ = std::fs::remove_dir_all(&side);
    })?;
    // `side` now holds the old app.
    let _ = std::fs::remove_dir_all(&side);
    Ok(())
}

fn swap(a: &Path, b: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    let ca = std::ffi::CString::new(a.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let cb = std::ffi::CString::new(b.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    if unsafe { libc::renamex_np(ca.as_ptr(), cb.as_ptr(), libc::RENAME_SWAP) } == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    // A volume without RENAME_SWAP: two renames, rolled back on failure.
    if matches!(err.raw_os_error(), Some(libc::ENOTSUP) | Some(libc::EINVAL)) {
        let old = b.with_extension("app.old");
        let _ = std::fs::remove_dir_all(&old);
        std::fs::rename(b, &old).map_err(|e| e.to_string())?;
        if let Err(e) = std::fs::rename(a, b) {
            let _ = std::fs::rename(&old, b);
            return Err(e.to_string());
        }
        return std::fs::rename(&old, a).map_err(|e| e.to_string());
    }
    Err(format!("Swapping in the new app failed: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn versions_order_like_newest_version_py() {
        assert!(is_newer("0.1.0", "0.1.0-beta.44"));
        assert!(is_newer("0.1.0-beta.45", "0.1.0-beta.44"));
        assert!(is_newer("v0.2.0-beta.1", "0.1.9"));
        assert!(!is_newer("0.1.0-beta.9", "0.1.0-beta.44"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    fn release(tag: &str, prerelease: bool, dmg: bool) -> Value {
        let assets = if dmg { json!([{"name": format!("Taskboard-{tag}.dmg"), "browser_download_url": format!("https://x/{tag}.dmg"), "size": 5, "digest": "sha256:AB"}]) } else { json!([]) };
        json!({"tag_name": tag, "draft": false, "prerelease": prerelease, "html_url": format!("https://x/{tag}"), "assets": assets})
    }

    #[test]
    fn picks_the_newest_release_for_the_channel() {
        let list = json!([release("v0.2.0-beta.2", true, true), release("v0.1.1", false, true), release("v0.2.0-beta.3", true, false), release("v0.1.0", false, true)]);
        let beta = pick(&list, "0.1.0-beta.44").unwrap();
        assert_eq!((beta.version.as_str(), beta.sha256.as_deref()), ("0.2.0-beta.2", Some("ab")));
        assert_eq!(pick(&list, "0.1.0").unwrap().version, "0.1.1");
        assert_eq!(pick(&list, "0.1.1"), None);
        let draft = json!([{"tag_name": "v9.0.0", "draft": true, "prerelease": false, "assets": [{"name": "a.dmg", "browser_download_url": "u"}]}]);
        assert_eq!(pick(&draft, "0.1.0"), None);
    }

    #[test]
    fn swap_exchanges_the_bundles() {
        let tmp = std::env::temp_dir().join(format!("taskboard-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (a, b) = (tmp.join("staged/Taskboard.app"), tmp.join("Apps/Taskboard.app"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("v"), "new").unwrap();
        std::fs::write(b.join("v"), "old").unwrap();
        swap_in(&a, &b).unwrap();
        assert_eq!(std::fs::read_to_string(b.join("v")).unwrap(), "new");
        assert!(!a.exists());
        assert_eq!(std::fs::read_dir(tmp.join("Apps")).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
