//! The app's side of the board's Claude Code hooks and `task-board` plugin: which copy of `tb` and
//! the plugin they should run (this bundle's), and the last install that failed. Checking and
//! installing them is [`taskboardd::claude_hooks`], which `tb install-hooks` uses too.
use std::path::Path;
use std::sync::Mutex;

pub use taskboardd::claude_hooks::{Hooks, Target, check, claude_dir, uninstall};

pub fn target() -> Option<Target> {
    let bundle = crate::install::installed_bundle()?;
    if std::env::var_os("TASKBOARD_NO_TB_LINK").is_some() {
        return Some(Target::of(Path::new("/Applications/Taskboard.app"), false));
    }
    Some(Target::of(&bundle, true))
}

/// The last install that failed and when ("3:05 PM"). It stays, in the footer and on the hooks
/// row, until it's dismissed or an install works.
#[derive(Clone, Debug, PartialEq)]
pub struct Failed {
    /// "Writing ~/.claude/settings.json: permission denied", the stage first.
    pub text: String,
    pub at: String,
}

static FAILED: Mutex<Option<Failed>> = Mutex::new(None);

pub fn failed() -> Option<Failed> {
    FAILED.lock().ok().and_then(|g| g.clone())
}

/// Put the failure away (its ✕).
pub fn dismiss() {
    set_failed(None);
}

fn set_failed(f: Option<Failed>) {
    if let Ok(mut g) = FAILED.lock() {
        *g = f;
    }
}

/// Install the plugin and hooks for this app. Blocking. A failure is kept (see [`failed`]).
pub fn install(t: &Target) -> Result<(), String> {
    let r = taskboardd::claude_hooks::install(t);
    set_failed(r.as_ref().err().map(|e| Failed { text: e.clone(), at: chrono::Local::now().format("%-I:%M %p").to_string() }));
    r
}
