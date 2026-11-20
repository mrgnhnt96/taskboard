//! Hidden runs for agents (Slate, https://github.com/mrgnhnt96/slate): `slate shoot` and
//! `slate drive` launch the app with `SLATE=1`. The window Slate shoots opens below the desktop,
//! never focused, and the app is an accessory app that is never frontmost, so nothing shows on
//! screen and the app in front keeps the keyboard. `scripts/slate.sh` runs them against this
//! checkout's build.
//!
//! Only debug builds and builds with the `snapshot` feature honour it: a release build ignores
//! `SLATE=1`.
//!
//! Slate shoots the main window, or the Settings window with `TASKBOARD_PAGE=settings`. Any other
//! window a hidden run opens stays off screen (`show: false`), and the app never activates.

use gpui_kit::{App, Application, Window, WindowOptions};

/// Builds that honour `SLATE=1`.
const ALLOWED: bool = cfg!(debug_assertions) || cfg!(feature = "snapshot");

/// Whether this run is hidden.
pub fn enabled() -> bool {
    ALLOWED && slate::enabled()
}

/// The app as an accessory app under Slate.
pub fn application(app: Application) -> Application {
    if ALLOWED { slate::hidden(app) } else { app }
}

/// Whether Slate shoots the Settings window rather than the main one.
fn shoots_settings() -> bool {
    std::env::var("TASKBOARD_PAGE").as_deref() == Ok("settings")
}

/// The main window's options: under Slate, the shot window's (no focus, `SLATE_SIZE`), or kept off
/// screen when Slate shoots Settings.
pub fn main_options(options: WindowOptions, cx: &App) -> WindowOptions {
    if !enabled() {
        options
    } else if shoots_settings() {
        off_screen(options)
    } else {
        slate::window_options(options, cx)
    }
}

/// The Settings window's options: under Slate, the shot window's with `TASKBOARD_PAGE=settings`,
/// or kept off screen when it opens later in a drive.
pub fn settings_options(options: WindowOptions, cx: &App) -> WindowOptions {
    if !enabled() {
        options
    } else if shoots_settings() && !attached() {
        slate::window_options(options, cx)
    } else {
        off_screen(options)
    }
}

fn off_screen(options: WindowOptions) -> WindowOptions {
    WindowOptions { show: false, focus: false, ..options }
}

static ATTACHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn attached() -> bool {
    ATTACHED.load(std::sync::atomic::Ordering::SeqCst)
}

/// Hides the main window and has Slate shoot or drive it. Does nothing when Slate shoots Settings.
pub fn attach_main(window: &mut Window, cx: &mut App) {
    if enabled() && !shoots_settings() && !ATTACHED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        slate::attach(window, cx);
    }
}

/// Hides the first Settings window and has Slate shoot or drive it with `TASKBOARD_PAGE=settings`.
pub fn attach_settings(window: &mut Window, cx: &mut App) {
    if enabled() && shoots_settings() && !ATTACHED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        slate::attach(window, cx);
    }
}

/// Whether the app may bring itself or a window to the front: never under Slate or
/// `TASKBOARD_NO_ACTIVATE`.
pub fn may_activate() -> bool {
    !enabled() && std::env::var("TASKBOARD_NO_ACTIVATE").is_err()
}
