//! The Claude Code hooks: the `task-board` plugin (`plugin/` in the repo, shipped in the bundle at
//! `Contents/Resources/plugin`), which reports each session's activity to the board through `tb`.
//!
//! Claude Code runs an installed plugin from its own cache, keyed by the plugin's version, so an
//! app update only reaches the hooks once they're reinstalled. The status bar shows whether
//! they're in step:
//! - **not installed**: no `task-board@taskboard` in Claude Code's config;
//! - **needs reinstall**: installed, but switched off, loaded from another folder (a repo
//!   checkout, an app that's gone), or a different version from the one this app ships;
//! - **current**: installed, on, loaded from this app, and at its version.
//!
//! Installing (or reinstalling) points the marketplace at this app and installs the plugin with
//! the `claude` CLI, which keeps Claude Code's own files consistent.
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const PLUGIN: &str = "task-board@taskboard";
const MARKETPLACE: &str = "taskboard";
/// The plugin's manifest inside the marketplace folder.
const MANIFEST: &str = "task-board/.claude-plugin/plugin.json";

#[derive(Clone, Debug, PartialEq)]
pub enum Hooks {
    /// Not inside an installed app (`cargo run`, the sample board): nothing to check.
    Hidden,
    Checking,
    NotInstalled,
    NeedsReinstall(String),
    Current,
}

/// Where the hooks should load from, and whether this app may install them. Taskboard Dev only
/// reports on the real app's install (it leaves Claude Code's config to Taskboard.app).
pub fn target() -> Option<(PathBuf, bool)> {
    let bundle = crate::install::installed_bundle()?;
    if std::env::var_os("TASKBOARD_NO_TB_LINK").is_some() {
        return Some((PathBuf::from("/Applications/Taskboard.app/Contents/Resources/plugin"), false));
    }
    Some((bundle.join("Contents/Resources/plugin"), true))
}

/// Claude Code's config folder (`CLAUDE_CONFIG_DIR`, else `~/.claude`).
pub fn claude_dir() -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => dirs_home().join(".claude"),
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn read_json(p: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(p) {
        Ok(text) => serde_json::from_str(&text).map_err(|_| format!("{} doesn't parse", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(e) => Err(format!("couldn't read {}: {e}", p.display())),
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The hooks' state in the Claude config at `claude`, against the plugin folder `want`.
pub fn check(claude: &Path, want: &Path) -> Hooks {
    let (settings, known, installed) = match (
        read_json(&claude.join("settings.json")),
        read_json(&claude.join("plugins/known_marketplaces.json")),
        read_json(&claude.join("plugins/installed_plugins.json")),
    ) {
        (Ok(s), Ok(k), Ok(i)) => (s, k, i),
        (Err(e), ..) | (_, Err(e), _) | (.., Err(e)) => return Hooks::NeedsReinstall(e),
    };
    let enabled = settings["enabledPlugins"][PLUGIN].as_bool();
    let entries = installed["plugins"][PLUGIN].as_array().cloned().unwrap_or_default();
    let is_installed = !entries.is_empty();
    if !is_installed && enabled.is_none() {
        return Hooks::NotInstalled;
    }
    if enabled == Some(false) {
        return Hooks::NeedsReinstall("It's switched off in Claude Code.".into());
    }
    if !is_installed {
        return Hooks::NeedsReinstall("Claude Code lists it but it isn't installed.".into());
    }
    let from = settings["extraKnownMarketplaces"][MARKETPLACE]["source"]["path"]
        .as_str()
        .or(known[MARKETPLACE]["source"]["path"].as_str())
        .map(PathBuf::from);
    match from {
        None => Hooks::NeedsReinstall("Its marketplace is missing from Claude Code.".into()),
        Some(p) if same_dir(&p, want) => version_check(&entries, want),
        Some(p) if !p.is_dir() => Hooks::NeedsReinstall(format!("Claude Code loads it from {}, which is gone.", p.display())),
        Some(p) => Hooks::NeedsReinstall(format!("Claude Code loads it from {}, not from this app.", p.display())),
    }
}

/// Current when Claude Code's cached copy is the version this app ships. A shipped manifest that's
/// missing or has no version gives nothing to compare, so that counts as current.
fn version_check(entries: &[Value], want: &Path) -> Hooks {
    let Ok(manifest) = read_json(&want.join(MANIFEST)) else { return Hooks::Current };
    let Some(ships) = manifest["version"].as_str() else { return Hooks::Current };
    if entries.iter().any(|e| e["version"].as_str() == Some(ships)) {
        return Hooks::Current;
    }
    match entries.iter().find_map(|e| e["version"].as_str()) {
        Some(runs) => Hooks::NeedsReinstall(format!("Claude Code runs version {runs}; this app ships {ships}.")),
        None => Hooks::NeedsReinstall(format!("Claude Code doesn't say which version it runs; this app ships {ships}.")),
    }
}

/// The `claude` CLI. An app opened from Finder has a bare PATH, so look where installers put it.
fn claude_bin() -> Option<PathBuf> {
    let home = dirs_home();
    let mut dirs = vec![home.join(".local/bin"), home.join(".claude/local"), PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")];
    dirs.extend(std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default());
    dirs.into_iter().map(|d| d.join("claude")).find(|p| p.is_file())
}

/// Point the `taskboard` marketplace at `want` and install the plugin from it. Blocking.
pub fn install(want: &Path) -> Result<(), String> {
    if !want.join(".claude-plugin/marketplace.json").is_file() {
        return Err(format!("This app has no plugin at {}.", want.display()));
    }
    let claude = claude_bin().ok_or("Couldn't find the claude command.")?;
    let run = |args: &[&str]| -> Result<(), String> {
        let out = Command::new(&claude).args(args).output().map_err(|e| format!("Couldn't run claude: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
        let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().trim_start_matches('✘').trim();
        Err(format!("claude {} failed: {last}", args[..2].join(" ")))
    };
    // Removing the marketplace also uninstalls the plugin; it's fine for it to be missing.
    let _ = run(&["plugin", "marketplace", "remove", MARKETPLACE]);
    run(&["plugin", "marketplace", "add", &want.to_string_lossy()])?;
    run(&["plugin", "install", PLUGIN, "--scope", "user"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup(name: &str) -> (Tmp, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("taskboard-hooks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (claude, plugin) = (root.join("claude"), root.join("Taskboard.app/Contents/Resources/plugin"));
        std::fs::create_dir_all(claude.join("plugins")).unwrap();
        std::fs::create_dir_all(&plugin).unwrap();
        (Tmp(root), claude, plugin)
    }

    fn write(p: PathBuf, v: Value) {
        std::fs::write(p, v.to_string()).unwrap();
    }

    fn installed(claude: &Path, enabled: bool, from: &Path) {
        write(claude.join("settings.json"), json!({"enabledPlugins": {PLUGIN: enabled},
            "extraKnownMarketplaces": {"taskboard": {"source": {"source": "directory", "path": from}}}}));
        write(claude.join("plugins/installed_plugins.json"), json!({"version": 2, "plugins": {PLUGIN: [{"scope": "user", "version": "1.0.0"}]}}));
    }

    #[::core::prelude::v1::test]
    fn nothing_installed() {
        let (_t, claude, plugin) = setup("none");
        assert_eq!(check(&claude, &plugin), Hooks::NotInstalled);
        write(claude.join("settings.json"), json!({"enabledPlugins": {"other@x": true}}));
        assert_eq!(check(&claude, &plugin), Hooks::NotInstalled);
    }

    #[::core::prelude::v1::test]
    fn current_when_loaded_from_this_app() {
        let (_t, claude, plugin) = setup("current");
        installed(&claude, true, &plugin);
        assert_eq!(check(&claude, &plugin), Hooks::Current);
    }

    #[::core::prelude::v1::test]
    fn reinstall_when_switched_off_moved_or_gone() {
        let (t, claude, plugin) = setup("stale");
        installed(&claude, false, &plugin);
        assert_eq!(check(&claude, &plugin), Hooks::NeedsReinstall("It's switched off in Claude Code.".into()));

        let checkout = t.0.join("checkout/plugin");
        std::fs::create_dir_all(&checkout).unwrap();
        installed(&claude, true, &checkout);
        assert!(matches!(check(&claude, &plugin), Hooks::NeedsReinstall(w) if w.ends_with("not from this app.")));

        installed(&claude, true, &t.0.join("Old.app/Contents/Resources/plugin"));
        assert!(matches!(check(&claude, &plugin), Hooks::NeedsReinstall(w) if w.ends_with("which is gone.")));

        // Listed in known_marketplaces.json only (an older Claude Code).
        write(claude.join("settings.json"), json!({"enabledPlugins": {PLUGIN: true}}));
        write(claude.join("plugins/known_marketplaces.json"), json!({"taskboard": {"source": {"source": "directory", "path": plugin}}}));
        assert_eq!(check(&claude, &plugin), Hooks::Current);

        std::fs::write(claude.join("settings.json"), "{ not json").unwrap();
        assert!(matches!(check(&claude, &plugin), Hooks::NeedsReinstall(w) if w.ends_with("doesn't parse")));
    }

    #[::core::prelude::v1::test]
    fn reinstall_when_the_cached_version_is_old() {
        let (_t, claude, plugin) = setup("version");
        installed(&claude, true, &plugin);
        std::fs::create_dir_all(plugin.join("task-board/.claude-plugin")).unwrap();
        write(plugin.join(MANIFEST), json!({"name": "task-board", "version": "1.0.0"}));
        assert_eq!(check(&claude, &plugin), Hooks::Current);

        write(plugin.join(MANIFEST), json!({"name": "task-board", "version": "0.1.0-beta.16"}));
        assert_eq!(
            check(&claude, &plugin),
            Hooks::NeedsReinstall("Claude Code runs version 1.0.0; this app ships 0.1.0-beta.16.".into())
        );
    }

    /// Runs the real `claude` CLI on a throwaway config (`CLAUDE_CONFIG_DIR`, which `claude`
    /// honours): `cargo test -p taskboard-app hooks -- --ignored`.
    #[::core::prelude::v1::test]
    #[ignore]
    fn install_and_reinstall_with_claude() {
        let (t, claude, _) = setup("claude");
        let (old, new) = (t.0.join("Old.app/plugin"), t.0.join("New.app/plugin"));
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin");
        for d in [&old, &new] {
            assert!(Command::new("ditto").arg(&repo).arg(d).status().unwrap().success());
        }
        // SAFETY: the only test that touches CLAUDE_CONFIG_DIR, and it's run on its own.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &claude) };
        assert_eq!(check(&claude_dir(), &old), Hooks::NotInstalled);
        install(&old).unwrap();
        assert_eq!(check(&claude, &old), Hooks::Current);
        assert!(matches!(check(&claude, &new), Hooks::NeedsReinstall(_)));
        install(&new).unwrap();
        assert_eq!(check(&claude, &new), Hooks::Current);
    }
}
