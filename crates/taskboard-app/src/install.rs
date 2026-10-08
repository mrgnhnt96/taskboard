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

/// Register the bundled LaunchAgent (idempotent). Returns the resulting state.
pub fn register(bundle: &Path) -> LoginItem {
    let Some(plist) = bundled_agent_plist(bundle) else {
        return LoginItem::Failed("The launch agent isn't in the app bundle.".into());
    };
    let svc = agent_service(&plist);
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
