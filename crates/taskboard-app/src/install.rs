//! First-launch install: how taskboardd gets started, and the `tb` link.
//!
//! - **Installed** (`Taskboard.app/Contents/MacOS/taskboard-app`, no `TASKBOARD_DEV`): register the
//!   LaunchAgent shipped in `Contents/Library/LaunchAgents/` with SMAppService, so launchd runs
//!   the bundle's `taskboardd serve` at login and keeps it alive while the app is closed. Then link
//!   `~/.local/bin/tb` to the bundle's copy (the Claude plugin's `tb` shim finds it
//!   there) unless something else already lives at that path.
//! - **Dev** (`TASKBOARD_DEV=1`, or not inside a bundle, e.g. `cargo run`): nothing is
//!   registered; run `taskboardd serve` yourself (or use `TASKBOARD_BACKEND=fake`).
//!
//! `taskboard-app --uninstall` unregisters the agent and removes the link (used by
//! `scripts/dev-app.sh --uninstall`).
//!
//! Everything here blocks; `start` runs it on its own thread and publishes the result in
//! [`status`] for the banner.
use objc2_foundation::NSString;
use objc2_service_management::{SMAppService, SMAppServiceStatus};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq)]
pub enum LoginItem {
    Dev,
    Checking,
    Enabled,
    /// Registered, but the human must switch it on in System Settings ▸ Login Items.
    RequiresApproval,
    NotRegistered,
    Failed(String),
}

static STATUS: Mutex<LoginItem> = Mutex::new(LoginItem::Checking);
static IN_THE_WAY: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// What this launch found in the way of the board (another board on its port, another `tb` first
/// on the PATH, another task-board plugin), one sentence each, for the banner.
pub fn in_the_way() -> Vec<String> {
    IN_THE_WAY.lock().map(|v| v.clone()).unwrap_or_default()
}

/// The PATH agents' shells are likely to have: the app's own (bare when opened from the Finder)
/// behind the usual tool folders, as `taskboardd serve` widens it.
fn agent_path() -> String {
    let home = taskboardd::util::expand_home("~");
    let mut parts: Vec<String> = [".local/bin", ".cargo/bin"].iter().map(|d| home.join(d).to_string_lossy().to_string()).collect();
    parts.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(String::from));
    parts.push(std::env::var("PATH").unwrap_or_default());
    parts.join(":")
}

/// Looks for another board in the way (`taskboardd::cutover`).
fn check_in_the_way() {
    let Ok(cfg) = taskboardd::config::Config::load() else { return };
    let found = taskboardd::cutover::problems(&cfg.host, cfg.port, &agent_path(), &crate::hooks::claude_dir());
    for p in &found {
        eprintln!("taskboard-app: {p}");
    }
    if let Ok(mut g) = IN_THE_WAY.lock() {
        *g = found;
    }
}

/// The daemon's login item, as last checked.
pub fn status() -> LoginItem {
    STATUS.lock().map(|s| s.clone()).unwrap_or(LoginItem::Checking)
}

fn set_status(s: LoginItem) {
    if let Ok(mut g) = STATUS.lock() {
        *g = s;
    }
}

/// `/x/Taskboard.app/Contents/MacOS/taskboard-app` -> `/x/Taskboard.app`.
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && app.extension()? == "app").then(|| app.to_path_buf())
}

/// The bundle we run from, unless in dev mode.
pub fn installed_bundle() -> Option<PathBuf> {
    if std::env::var("TASKBOARD_DEV").is_ok_and(|v| !v.is_empty() && v != "0") {
        return None;
    }
    bundle_of(&std::env::current_exe().ok()?)
}

/// The LaunchAgent plist shipped in the bundle (the first one in `Contents/Library/LaunchAgents`).
fn bundled_agent_plist(bundle: &Path) -> Option<PathBuf> {
    let dir = bundle.join("Contents/Library/LaunchAgents");
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "plist")).collect();
    v.sort();
    v.into_iter().next()
}

fn agent_service(plist: &Path) -> objc2::rc::Retained<SMAppService> {
    let name = plist.file_name().unwrap_or_default().to_string_lossy();
    unsafe { SMAppService::agentServiceWithPlistName(&NSString::from_str(&name)) }
}

fn from_status(s: SMAppServiceStatus) -> LoginItem {
    match s {
        SMAppServiceStatus::Enabled => LoginItem::Enabled,
        SMAppServiceStatus::RequiresApproval => LoginItem::RequiresApproval,
        SMAppServiceStatus::NotRegistered => LoginItem::NotRegistered,
        SMAppServiceStatus::NotFound => LoginItem::Failed("The launch agent isn't in the app bundle.".into()),
        _ => LoginItem::Failed("macOS reported an unknown login item status.".into()),
    }
}

/// The agent's launchd label (`Label` in its plist).
fn agent_label(plist: &Path) -> Option<String> {
    let out = std::process::Command::new("/usr/bin/plutil").args(["-extract", "Label", "raw", "-o", "-"]).arg(plist).output().ok()?;
    let label = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !label.is_empty()).then_some(label)
}

/// `launchctl print` for a job launchd won't start under its registration: the binary's signature
/// no longer matches the one recorded when it was registered (each ad-hoc dev build has a new one),
/// so every spawn fails with EX_CONFIG (78).
fn stale_registration(launchctl_print: &str) -> bool {
    launchctl_print.lines().map(str::trim).any(|l| l == "job state = spawn failed" || l.starts_with("last exit code = 78"))
}

fn launchd_job(label: &str) -> Option<String> {
    let uid = String::from_utf8_lossy(&std::process::Command::new("/usr/bin/id").arg("-u").output().ok()?.stdout).trim().to_string();
    let out = std::process::Command::new("/bin/launchctl").arg("print").arg(format!("gui/{uid}/{label}")).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Register the bundled LaunchAgent (idempotent). Returns the resulting state. A registration
/// launchd can't start (see [`stale_registration`]) is dropped and made again for this bundle.
pub fn register(bundle: &Path) -> LoginItem {
    let Some(plist) = bundled_agent_plist(bundle) else {
        return LoginItem::Failed("The launch agent isn't in the app bundle.".into());
    };
    let svc = agent_service(&plist);
    let stale = matches!(unsafe { svc.status() }, SMAppServiceStatus::Enabled)
        && agent_label(&plist).and_then(|l| launchd_job(&l)).is_some_and(|p| stale_registration(&p));
    if stale {
        eprintln!("taskboard-app: launchd can't start the daemon under its old registration; registering it again");
        let _ = unsafe { svc.unregisterAndReturnError() };
        // macOS drops the registration in the background; registering before it's gone is a no-op.
        for _ in 0..50 {
            if !matches!(unsafe { svc.status() }, SMAppServiceStatus::Enabled) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    match unsafe { svc.status() } {
        SMAppServiceStatus::Enabled | SMAppServiceStatus::RequiresApproval => {}
        _ => {
            if let Err(e) = unsafe { svc.registerAndReturnError() } {
                let state = from_status(unsafe { svc.status() });
                // "Operation not permitted" = the human must approve it; the status says so.
                if state != LoginItem::RequiresApproval {
                    return LoginItem::Failed(e.localizedDescription().to_string());
                }
            }
        }
    }
    from_status(unsafe { svc.status() })
}

pub fn unregister(bundle: &Path) -> Result<(), String> {
    let plist = bundled_agent_plist(bundle).ok_or("The launch agent isn't in the app bundle.")?;
    unsafe { agent_service(&plist).unregisterAndReturnError() }.map_err(|e| e.localizedDescription().to_string())
}

/// Open System Settings ▸ General ▸ Login Items (where a RequiresApproval agent is switched on).
pub fn open_login_items() {
    unsafe { SMAppService::openSystemSettingsLoginItems() };
}

// ------------------------------------------------------------------ tb link

fn tb_link() -> PathBuf {
    taskboardd::util::expand_home("~/.local/bin/tb")
}

/// A symlink to some Taskboard bundle's `tb` (this one, an older copy, Taskboard Dev): ours to
/// replace. Anything else called `tb` is someone else's.
fn is_our_link(link: &Path) -> bool {
    std::fs::read_link(link).is_ok_and(|t| t.ends_with("Contents/MacOS/tb") && bundle_of(&t).is_some_and(|b| b.to_string_lossy().contains("Taskboard")))
}

/// Link `~/.local/bin/tb` to the bundle's copy, leaving any file we didn't make alone
/// (e.g. one `install.sh` copied there).
pub fn link_tb(bundle: &Path) -> Result<(), String> {
    let link = tb_link();
    let target = bundle.join("Contents/MacOS/tb");
    if std::fs::symlink_metadata(&link).is_ok() {
        if !is_our_link(&link) {
            return Ok(());
        }
        if std::fs::read_link(&link).ok().as_deref() == Some(target.as_path()) {
            return Ok(());
        }
    }
    let dir = link.parent().ok_or("no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(".tb.{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(&target, &tmp).and_then(|_| std::fs::rename(&tmp, &link)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

// ------------------------------------------------------------------ entry points

/// `taskboard-app --<verb>` commands that run without a window. None: start the app.
pub fn headless(args: &[String]) -> Option<i32> {
    match args.get(1).map(String::as_str) {
        Some("--uninstall") => {
            let Some(bundle) = bundle_of(&std::env::current_exe().ok()?) else {
                eprintln!("taskboard-app: --uninstall only works from inside Taskboard.app");
                return Some(2);
            };
            let mut code = 0;
            if let Err(e) = unregister(&bundle) {
                eprintln!("taskboard-app: unregistering the daemon failed: {e}");
                code = 1;
            }
            let link = tb_link();
            if std::fs::read_link(&link).is_ok_and(|t| t.starts_with(&bundle)) {
                let _ = std::fs::remove_file(&link);
            }
            Some(code)
        }
        Some("--version") => {
            println!("taskboard-app {}", env!("CARGO_PKG_VERSION"));
            Some(0)
        }
        _ => None,
    }
}

/// Start the install / login-item work for this launch (installed apps only).
pub fn start(backend: &str) {
    let bundle = match installed_bundle() {
        Some(b) if backend != "fake" => b,
        _ => {
            set_status(LoginItem::Dev);
            return;
        }
    };
    std::thread::Builder::new()
        .name("install".into())
        .spawn(move || {
            check_in_the_way();
            set_status(register(&bundle));
            // Taskboard Dev leaves the link to the real app (TASKBOARD_NO_TB_LINK in its LSEnvironment).
            if std::env::var_os("TASKBOARD_NO_TB_LINK").is_some() {
                return;
            }
            if let Err(e) = link_tb(&bundle) {
                eprintln!("taskboard-app: couldn't link tb: {e}");
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_daemon_launchd_cant_start_needs_registering_again() {
        let failed = "\tlast exit code = 78: EX_CONFIG\n\tjob state = spawn failed\n";
        let retrying = "\tstate = spawn scheduled\n\tlast exit code = 78: EX_CONFIG\n";
        let running = "\tlast exit code = (never exited)\n\tjob state = running\n";
        let exited = "\tlast exit code = 1\n\tjob state = exited\n";
        assert!(stale_registration(failed));
        assert!(stale_registration(retrying));
        assert!(!stale_registration(running));
        assert!(!stale_registration(exited), "an ordinary crash isn't a registration problem");
    }

    #[test]
    fn bundle_detection() {
        assert_eq!(bundle_of(Path::new("/A/Taskboard.app/Contents/MacOS/taskboard-app")), Some(PathBuf::from("/A/Taskboard.app")));
        assert_eq!(bundle_of(Path::new("/x/target/debug/taskboard-app")), None);
    }

    #[::core::prelude::v1::test]
    fn only_replaces_taskboard_links() {
        let dir = std::env::temp_dir().join(format!("tb-link-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ours = dir.join("ours");
        let theirs = dir.join("theirs");
        std::os::unix::fs::symlink("/Applications/Taskboard.app/Contents/MacOS/tb", &ours).unwrap();
        std::os::unix::fs::symlink("/Applications/Other.app/Contents/MacOS/tb", &theirs).unwrap();
        assert!(is_our_link(&ours));
        assert!(!is_our_link(&theirs));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
