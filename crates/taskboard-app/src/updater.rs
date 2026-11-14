//! Updates: checks GitHub for a newer release, downloads the app, checks it and swaps it in, like
//! Midna's updater.
//!
//! - **Feed**: the repo's releases (`RELEASES`), the same list the website's Download button reads.
//!   A beta build follows every release, prereleases included; a public one only public releases.
//!   The newest one newer than this build, with a `Taskboard-<version>.app.tar.gz` attached, is the
//!   update. That's the notarized app the release's DMG holds, packed so it unpacks in a moment; a DMG
//!   takes seconds to mount and unmount.
//! - **Checked**: the archive's sha256 against the digest GitHub keeps for the asset, then the app inside:
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
    /// Stays until dismissed or the next check. `stage` names the step that failed:
    /// "Checking for updates", "Downloading 1.2.0", "Checking 1.2.0", "Installing 1.2.0".
    Failed { stage: String, error: String, at: String },
}

impl State {
    /// "Couldn’t install 1.2.0", the failure's title in the footer and Settings.
    pub fn failure_title(stage: &str) -> String {
        match stage.split_once(' ') {
            Some(("Downloading", v)) => format!("Couldn’t download {v}"),
            Some(("Checking", v)) if v != "for updates" => format!("Couldn’t check {v}"),
            Some(("Installing", v)) => format!("Couldn’t install {v}"),
            _ => "Couldn’t check for updates".into(),
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State::Idle);
static WAKE: OnceLock<Mutex<Sender<()>>> = OnceLock::new();
/// The staged app and the installed bundle it replaces.
static STAGED: Mutex<Option<(PathBuf, PathBuf)>> = Mutex::new(None);

pub fn state() -> State {
    STATE.lock().map(|s| s.clone()).unwrap_or(State::Idle)
}

fn set(s: State) {
    if let Ok(mut g) = STATE.lock() {
        *g = s;
    }
}

/// Put a failure away (its ✕ in the footer or Settings); the next check runs as usual.
pub fn dismiss() {
    if let Ok(mut g) = STATE.lock()
        && matches!(*g, State::Failed { .. })
    {
        *g = State::Idle;
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
    pub archive: String,
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
            let asset = r["assets"].as_array()?.iter().find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(".app.tar.gz")))?;
            Some(Release {
                page: r["html_url"].as_str().unwrap_or(RELEASES_PAGE).to_string(),
                archive: asset["browser_download_url"].as_str()?.to_string(),
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

/// Download the archive to `dir`, hashing as it goes; refused unless it matches GitHub's digest.
fn download(r: &Release, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let dest = dir.join(format!("Taskboard-{}.app.tar.gz", r.version));
    let part = dir.join(format!("Taskboard-{}.app.tar.gz.part", r.version));
    let resp = agent().get(&r.archive).call().map_err(|e| format!("Download failed: {e}"))?;
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

/// Unpack the app from the archive into `dir` and check it (see the module docs).
fn stage(archive: &Path, dir: &Path, version: &str, installed: &Path) -> Result<PathBuf, String> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let unpacked = Command::new("/usr/bin/tar").arg("-xzf").arg(archive).arg("-C").arg(dir).output().map_err(|e| e.to_string())?;
    if !unpacked.status.success() {
        return Err(format!("Couldn't unpack the update: {}", String::from_utf8_lossy(&unpacked.stderr).trim()));
    }
    let app = dir.join("Taskboard.app");
    if !app.is_dir() {
        return Err("The update has no Taskboard.app in it.".into());
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

/// One check: find a newer release, then download, check and stage it. A ready update stays
/// ready until a newer one is downloaded and checked; a failed check for a newer one only logs.
fn check(bundle: &Path) {
    let before = state();
    let ready = match &before {
        State::Ready { version, .. } => Some(version.clone()),
        _ => None,
    };
    if ready.is_none() {
        set(State::Checking);
    }
    let fail = |stage: String, error: String| {
        if ready.is_some() {
            crate::log::info(format!("{stage}: {error}"));
            set(before.clone());
        } else {
            set(State::Failed { stage, error, at: now() });
        }
    };
    let releases = match fetch_releases() {
        Ok(r) => r,
        Err(e) => return fail("Checking for updates".into(), e),
    };
    let Some(r) = pick(&releases, VERSION) else {
        if ready.is_none() {
            set(State::UpToDate { at: now() });
        }
        return;
    };
    if ready.as_deref().is_some_and(|v| !is_newer(&r.version, v)) {
        return;
    }
    if ready.is_none() {
        set(State::Downloading { version: r.version.clone() });
    }
    let dir = cache_dir();
    let archive = match download(&r, &dir) {
        Ok(a) => a,
        Err(e) => return fail(format!("Downloading {}", r.version), e),
    };
    let to = dir.join(format!("staged-{}", r.version));
    let app = stage(&archive, &to, &r.version, bundle);
    let _ = std::fs::remove_file(&archive);
    match app {
        Ok(app) => {
            if let Ok(mut g) = STAGED.lock() {
                if let Some(old) = g.as_ref().and_then(|(a, _)| a.parent()).filter(|o| *o != to) {
                    let _ = std::fs::remove_dir_all(old);
                }
                *g = Some((app, bundle.to_path_buf()));
            }
            set(State::Ready { version: r.version, page: r.page });
        }
        Err(e) => fail(format!("Checking {}", r.version), e),
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
        if !matches!(state(), State::Checking | State::Downloading { .. } | State::Applying { .. } | State::Ready { .. }) {
            set(State::Checking);
        }
        let _ = tx.send(());
    }
}

/// The note by Restart to update: what keeps running and what doesn't, from the board's
/// `GET /restart` (`code_reviews`: the reviews a restart cuts short and starts again).
pub fn restart_note(code_reviews: Option<usize>) -> String {
    let reviews = match code_reviews {
        Some(0) => String::new(),
        Some(1) => ", and 1 code review starts again".into(),
        Some(n) => format!(", and {n} code reviews start again"),
        None => ", and code reviews running now start again".into(),
    };
    format!("Agents keep running in Midna. The board is offline for about 10 seconds{reviews}.")
}

/// How many code reviews a restart of the board would start again, from its `GET /restart`
/// (None until it's been asked).
static RESTART_REVIEWS: Mutex<Option<usize>> = Mutex::new(None);

/// Keep what `GET /restart` said (`{"count": 2, "code_reviews": [...]}`), for [`restart_note`].
pub fn learn_restart(v: Option<&Value>) {
    let n = v.and_then(|v| v["count"].as_u64().or_else(|| v["code_reviews"].as_array().map(|a| a.len() as u64)));
    if let Ok(mut g) = RESTART_REVIEWS.lock() {
        *g = n.map(|n| n as usize);
    }
}

pub fn restart_reviews() -> Option<usize> {
    RESTART_REVIEWS.lock().ok().and_then(|g| *g)
}

/// Where the version being installed is written, so the relaunched app can say "Updated to X".
fn updated_marker() -> PathBuf {
    cache_dir().join("updated-to")
}

/// The update this launch finished, said once: "1.2.0" and when, if the app relaunched into it.
pub fn take_updated() -> Option<(String, String)> {
    let p = updated_marker();
    let text = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    let (v, at) = text.trim().split_once('\n').unwrap_or((text.trim(), ""));
    (v == VERSION).then(|| (v.to_string(), at.to_string()))
}

/// The update this launch finished and when ("1.2.0", "3:02 PM"), for Settings › System.
static UPDATED: Mutex<Option<(String, String)>> = Mutex::new(None);

pub fn updated() -> Option<(String, String)> {
    UPDATED.lock().ok().and_then(|g| g.clone())
}

pub fn set_updated(v: Option<(String, String)>) {
    if let Ok(mut g) = UPDATED.lock() {
        *g = v;
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
    let _ = std::fs::write(updated_marker(), format!("{version}\n{}", now()));
    let done = swap_in(&staged, &installed).and_then(|_| {
        if let Err(e) = crate::install::restart_daemon(&installed) {
            crate::log::info(format!("restarting the daemon after the update: {e}"));
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
        let _ = std::fs::remove_file(updated_marker());
        set(State::Failed { stage: format!("Installing {version}"), error: e.clone(), at: now() });
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
            return Err(format!("Replacing the app: {} can’t be written to.", parent.display()));
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

    fn release(tag: &str, prerelease: bool, archive: bool) -> Value {
        let dmg = json!({"name": format!("Taskboard-{tag}.dmg"), "browser_download_url": format!("https://x/{tag}.dmg"), "size": 9, "digest": "sha256:CD"});
        let tgz = json!({"name": format!("Taskboard-{tag}.app.tar.gz"), "browser_download_url": format!("https://x/{tag}.app.tar.gz"), "size": 5, "digest": "sha256:AB"});
        let assets = if archive { json!([dmg, tgz]) } else { json!([dmg]) };
        json!({"tag_name": tag, "draft": false, "prerelease": prerelease, "html_url": format!("https://x/{tag}"), "assets": assets})
    }

    #[test]
    fn picks_the_newest_release_for_the_channel() {
        let list = json!([release("v0.2.0-beta.2", true, true), release("v0.1.1", false, true), release("v0.2.0-beta.3", true, false), release("v0.1.0", false, true)]);
        let beta = pick(&list, "0.1.0-beta.44").unwrap();
        assert_eq!((beta.version.as_str(), beta.archive.as_str(), beta.sha256.as_deref()), ("0.2.0-beta.2", "https://x/v0.2.0-beta.2.app.tar.gz", Some("ab")));
        assert_eq!(pick(&list, "0.1.0").unwrap().version, "0.1.1");
        assert_eq!(pick(&list, "0.1.1"), None);
        let draft = json!([{"tag_name": "v9.0.0", "draft": true, "prerelease": false, "assets": [{"name": "a.app.tar.gz", "browser_download_url": "u"}]}]);
        assert_eq!(pick(&draft, "0.1.0"), None);
    }

    #[test]
    fn failures_name_their_stage() {
        assert_eq!(State::failure_title("Checking for updates"), "Couldn’t check for updates");
        assert_eq!(State::failure_title("Downloading 1.2.0"), "Couldn’t download 1.2.0");
        assert_eq!(State::failure_title("Checking 1.2.0"), "Couldn’t check 1.2.0");
        assert_eq!(State::failure_title("Installing 1.2.0"), "Couldn’t install 1.2.0");
    }

    #[test]
    fn the_restart_note_promises_only_what_keeps_running() {
        assert_eq!(restart_note(Some(0)), "Agents keep running in Midna. The board is offline for about 10 seconds.");
        assert_eq!(restart_note(Some(2)), "Agents keep running in Midna. The board is offline for about 10 seconds, and 2 code reviews start again.");
        assert!(restart_note(None).contains("code reviews running now start again"));
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
