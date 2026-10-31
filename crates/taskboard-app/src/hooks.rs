//! The Claude Code hooks, which report each session's activity to the board through `tb`, and the
//! `task-board` plugin (`plugin/` in the repo, shipped in the bundle at `Contents/Resources/plugin`),
//! which gives sessions the skill and the `/start` command.
//!
//! The hooks are entries in Claude Code's `settings.json`, as Midna writes its own: each runs this
//! app's `tb` at its fixed path in the bundle, so an update reaches them without a reinstall, and is
//! guarded so a missing binary is skipped instead of failing the hook. They're checked by what they
//! say, never by a version: an update whose hooks are unchanged needs nothing. Claude's other
//! settings and the user's own hooks are left as they are, and a file that doesn't parse is never
//! rewritten.
//!
//! The status bar shows whether they're in step:
//! - **not installed**: neither the hooks nor `task-board@taskboard` are in Claude Code's config;
//! - **needs reinstall**: the hooks are missing, differ from what this app writes or run another
//!   `tb`; or the plugin is switched off, loaded from another folder (a repo checkout, an app that's
//!   gone), still carries hooks of its own (they'd fire twice), or a different version from the one
//!   this app ships;
//! - **current**: the hooks are this app's, and the plugin is on, loaded from this app, at its version.
//!
//! Installing (or reinstalling) points the marketplace at this app and installs the plugin with the
//! `claude` CLI, which keeps Claude Code's own files consistent, then writes the hook entries.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const PLUGIN: &str = "task-board@taskboard";
const MARKETPLACE: &str = "taskboard";
/// The plugin's manifest inside the marketplace folder.
const MANIFEST: &str = "task-board/.claude-plugin/plugin.json";
/// The plugin's PreToolUse script for Bash, inside the marketplace folder.
const WAIT_HINT: &str = "task-board/bin/wait-hint";

/// Tools whose PreToolUse the board sees.
const TOOLS: &str = "Bash|Agent|Task|EnterWorktree|mcp__.*|Edit|Write|MultiEdit|NotebookEdit|Read|Grep|Glob";

/// What a hook entry runs: `tb hook <arg>`, or the plugin's wait-hint script.
#[derive(Clone, Copy)]
enum Run {
    Tb(&'static str),
    WaitHint,
}

/// One hook entry: its event, matcher, `if` condition, what it runs, timeout and whether it's async.
struct Entry {
    event: &'static str,
    matcher: Option<&'static str>,
    when: Option<&'static str>,
    run: Run,
    timeout: Option<u32>,
    background: bool,
}

const fn on(event: &'static str, matcher: Option<&'static str>, run: Run, background: bool) -> Entry {
    Entry { event, matcher, when: None, run, timeout: Some(5), background }
}

/// The hooks, in the order they're written.
const ENTRIES: &[Entry] = &[
    on("SessionStart", None, Run::Tb("SessionStart"), false),
    on("UserPromptSubmit", None, Run::Tb("UserPromptSubmit"), false),
    on("Stop", None, Run::Tb("Stop"), false),
    on("StopFailure", None, Run::Tb("StopFailure"), true),
    on("PreCompact", None, Run::Tb("PreCompact"), false),
    on("PreToolUse", Some(TOOLS), Run::Tb("PreToolUse"), false),
    on("PreToolUse", Some("Bash"), Run::WaitHint, false),
    on("PreToolUse", Some("AskUserQuestion"), Run::Tb("AskUser"), true),
    Entry { when: Some("Bash(git *)"), ..on("PostToolUse", Some("Bash"), Run::Tb("PostToolUse"), true) },
    on("PostToolUse", Some("Bash|Agent|Task"), Run::Tb("ToolEnd"), false),
    on("PostToolUse", Some("Edit|Write|MultiEdit"), Run::Tb("EditDone"), true),
    on("PostToolUse", Some("AskUserQuestion"), Run::Tb("AskUserDone"), true),
    on("PostToolUseFailure", Some("Bash|Agent|Task"), Run::Tb("ToolEnd"), false),
    on("PostToolUseFailure", Some("AskUserQuestion"), Run::Tb("AskUserDone"), true),
    Entry { timeout: None, ..on("SessionEnd", None, Run::Tb("SessionEnd"), false) },
    on("Notification", Some("permission_prompt|idle_prompt"), Run::Tb("Notification"), true),
];

#[derive(Clone, Debug, PartialEq)]
pub enum Hooks {
    /// Not inside an installed app (`cargo run`, the sample board): nothing to check.
    Hidden,
    Checking,
    NotInstalled,
    NeedsReinstall(String),
    Current,
}

/// What the hooks should run: the plugin folder to load, the `tb` the hook entries call, and
/// whether this app may install them. Taskboard Dev only reports on the real app's install (it
/// leaves Claude Code's config to Taskboard.app).
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub plugin: PathBuf,
    pub tb: PathBuf,
    pub can_install: bool,
}

impl Target {
    fn of(bundle: &Path, can_install: bool) -> Target {
        Target { plugin: bundle.join("Contents/Resources/plugin"), tb: bundle.join("Contents/MacOS/tb"), can_install }
    }
}

pub fn target() -> Option<Target> {
    let bundle = crate::install::installed_bundle()?;
    if std::env::var_os("TASKBOARD_NO_TB_LINK").is_some() {
        return Some(Target::of(Path::new("/Applications/Taskboard.app"), false));
    }
    Some(Target::of(&bundle, true))
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

// ------------------------------------------------------------------ settings.json entries

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The shell command for a hook: run the program if it's there, else do nothing and succeed.
fn command(run: Run, t: &Target) -> String {
    match run {
        Run::Tb(arg) => {
            let q = sh_quote(&t.tb.to_string_lossy());
            format!("[ -x {q} ] && exec {q} hook {arg} || true")
        }
        Run::WaitHint => {
            let q = sh_quote(&t.plugin.join(WAIT_HINT).to_string_lossy());
            format!("[ -x {q} ] && exec {q} || true")
        }
    }
}

fn entry(e: &Entry, t: &Target) -> Value {
    let mut h = json!({ "type": "command" });
    if let Some(w) = e.when {
        h["if"] = json!(w);
    }
    h["command"] = json!(command(e.run, t));
    if let Some(s) = e.timeout {
        h["timeout"] = json!(s);
    }
    if e.background {
        h["async"] = json!(true);
    }
    let mut v = serde_json::Map::new();
    if let Some(m) = e.matcher {
        v.insert("matcher".into(), json!(m));
    }
    v.insert("hooks".into(), json!([h]));
    Value::Object(v)
}

/// A hook command this app wrote, for any copy of `tb` (an older app, one that moved).
fn is_ours(hook: &Value) -> bool {
    hook["command"].as_str().is_some_and(|c| c.starts_with("[ -x '") && (c.contains("/tb' hook ") || c.contains(&format!("/{WAIT_HINT}' "))))
}

/// Our entries per event (an entry counts if any of its hooks is ours).
fn ours(settings: &Value) -> Vec<(String, Value)> {
    let mut out = vec![];
    for (ev, arr) in settings.get("hooks").and_then(Value::as_object).into_iter().flatten() {
        for e in arr.as_array().into_iter().flatten() {
            if e["hooks"].as_array().is_some_and(|hs| hs.iter().any(is_ours)) {
                out.push((ev.clone(), e.clone()));
            }
        }
    }
    out
}

/// `settings` without our hooks that `drop` picks (other hooks in a shared entry are kept; emptied
/// entries, events and the `hooks` object itself are dropped).
fn strip(settings: &mut Value, drop: impl Fn(&Value) -> bool) {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else { return };
    for arr in hooks.values_mut() {
        let Some(entries) = arr.as_array_mut() else { continue };
        for e in entries.iter_mut() {
            if let Some(hs) = e.get_mut("hooks").and_then(Value::as_array_mut) {
                hs.retain(|h| !(is_ours(h) && drop(h)));
            }
        }
        entries.retain(|e| e["hooks"].as_array().is_none_or(|hs| !hs.is_empty()));
    }
    hooks.retain(|_, arr| arr.as_array().is_none_or(|a| !a.is_empty()));
    if hooks.is_empty()
        && let Some(o) = settings.as_object_mut()
    {
        o.remove("hooks");
    }
}

/// `settings` with exactly this app's hooks: any of ours already there are replaced.
fn with(settings: &Value, t: &Target) -> Value {
    let mut v = settings.clone();
    strip(&mut v, |_| true);
    if !v.is_object() {
        v = json!({});
    }
    let hooks = v.as_object_mut().unwrap().entry("hooks").or_insert_with(|| json!({}));
    for e in ENTRIES {
        let arr = hooks.as_object_mut().unwrap().entry(e.event.to_string()).or_insert_with(|| json!([]));
        if let Some(a) = arr.as_array_mut() {
            a.push(entry(e, t));
        }
    }
    v
}

enum State {
    None,
    Current,
    Stale(String),
}

/// Whether `settings` has this app's hooks, judged by what they say.
fn state(settings: &Value, t: &Target) -> State {
    let have = ours(settings);
    if have.is_empty() {
        return State::None;
    }
    let want: Vec<(String, Value)> = ENTRIES.iter().map(|e| (e.event.to_string(), entry(e, t))).collect();
    if have.len() == want.len() && want.iter().all(|w| have.contains(w)) {
        return State::Current;
    }
    let commands: Vec<String> = ENTRIES.iter().map(|e| command(e.run, t)).collect();
    let elsewhere = have.iter().flat_map(|(_, e)| e["hooks"].as_array().cloned().unwrap_or_default()).filter(is_ours).any(|h| !commands.iter().any(|c| h["command"] == json!(c)));
    let mut missing: Vec<&str> = ENTRIES.iter().map(|e| e.event).filter(|ev| !have.iter().any(|(h, _)| h == ev)).collect();
    missing.dedup();
    State::Stale(if elsewhere {
        "Its hooks run another copy of tb, not this app's.".to_string()
    } else if !missing.is_empty() {
        format!("Its hooks miss {} event{}: {}.", missing.len(), if missing.len() == 1 { "" } else { "s" }, missing.join(", "))
    } else {
        "Its hooks differ from what this version of Taskboard installs.".to_string()
    })
}

/// `settings.json` and its text, refusing one that isn't a JSON object (it's never rewritten).
fn read_settings(path: &Path) -> Result<(Option<String>, Value), String> {
    match std::fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok((Some(s), json!({}))),
        Ok(s) => match serde_json::from_str::<Value>(&s) {
            Ok(v) if v.is_object() => Ok((Some(s), v)),
            Ok(_) => Err(format!("{} isn't a JSON object, so Taskboard won't change it.", path.display())),
            Err(e) => Err(format!("{} isn't plain JSON ({e}), so Taskboard won't change it.", path.display())),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((None, json!({}))),
        Err(e) => Err(format!("Couldn't read {}: {e}", path.display())),
    }
}

/// Apply `change` to `settings.json` and write it back when that changed anything.
fn edit_settings(path: &Path, change: impl Fn(&Value) -> Value) -> Result<(), String> {
    let (before, v) = read_settings(path)?;
    let after = change(&v);
    if after == v {
        return Ok(());
    }
    let mut text = serde_json::to_string_pretty(&after).unwrap_or_default();
    if before.as_deref().is_none_or(|b| b.ends_with('\n')) {
        text.push('\n');
    }
    write(path, before.as_deref(), &text).map_err(|e| format!("Writing {} failed: {e}", path.display()))
}

/// Atomic write. The first time Taskboard changes the file, the original is kept next to it as
/// `<name>.before-taskboard`.
fn write(path: &Path, before: Option<&str>, after: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if let Some(b) = before {
        let backup = path.with_file_name(format!("{name}.before-taskboard"));
        if !backup.exists() {
            std::fs::write(&backup, b)?;
        }
    }
    let tmp = path.with_file_name(format!(".{name}.taskboard-tmp"));
    std::fs::write(&tmp, after)?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, path)
}

// ------------------------------------------------------------------ the plugin

/// The hooks' state in the Claude config at `claude`, against what this app installs.
pub fn check(claude: &Path, t: &Target) -> Hooks {
    let (settings, known, installed) = match (
        read_json(&claude.join("settings.json")),
        read_json(&claude.join("plugins/known_marketplaces.json")),
        read_json(&claude.join("plugins/installed_plugins.json")),
    ) {
        (Ok(s), Ok(k), Ok(i)) => (s, k, i),
        (Err(e), ..) | (_, Err(e), _) | (.., Err(e)) => return Hooks::NeedsReinstall(e),
    };
    match (state(&settings, t), plugin_check(&settings, &known, &installed, &t.plugin)) {
        (State::None, Hooks::NotInstalled) => Hooks::NotInstalled,
        (State::None, _) => Hooks::NeedsReinstall("Its hooks aren't in Claude Code's settings.".into()),
        (State::Stale(why), _) => Hooks::NeedsReinstall(why),
        (State::Current, Hooks::NotInstalled) => Hooks::NeedsReinstall("The task-board plugin, with its skill and /start command, isn't installed.".into()),
        (State::Current, plugin) => plugin,
    }
}

/// The plugin's state: installed, on, loaded from `want`, without hooks of its own, at its version.
fn plugin_check(settings: &Value, known: &Value, installed: &Value, want: &Path) -> Hooks {
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
        Some(p) if same_dir(&p, want) => {
            if entries.iter().filter_map(|e| e["installPath"].as_str()).any(|p| Path::new(p).join("hooks/hooks.json").is_file()) {
                return Hooks::NeedsReinstall("Claude Code's copy of the plugin still has hooks of its own, so each would run twice.".into());
            }
            version_check(&entries, want)
        }
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

/// Point the `taskboard` marketplace at this app, install the plugin from it (its copy has no hooks,
/// which replaces one that had), then write this app's hooks into `settings.json`. Blocking.
pub fn install(t: &Target) -> Result<(), String> {
    if !t.plugin.join(".claude-plugin/marketplace.json").is_file() {
        return Err(format!("This app has no plugin at {}.", t.plugin.display()));
    }
    let settings = claude_dir().join("settings.json");
    // Refuse a settings file that can't be changed before the plugin is touched.
    read_settings(&settings)?;
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
    run(&["plugin", "marketplace", "add", &t.plugin.to_string_lossy()])?;
    run(&["plugin", "install", PLUGIN, "--scope", "user"])?;
    // Read again: the claude CLI just wrote the file too.
    edit_settings(&settings, |v| with(v, t))
}

/// Take out the hooks that run a `tb` inside `bundle` (`taskboard-app --uninstall`); hooks for
/// another copy of the app stay.
pub fn uninstall(claude: &Path, bundle: &Path) -> Result<(), String> {
    let inside = sh_quote(&bundle.to_string_lossy());
    let inside = inside.trim_end_matches('\'').to_string() + "/";
    edit_settings(&claude.join("settings.json"), |v| {
        let mut v = v.clone();
        strip(&mut v, |h| h["command"].as_str().is_some_and(|c| c.contains(&inside)));
        v
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup(name: &str) -> (Tmp, PathBuf, Target) {
        let root = std::env::temp_dir().join(format!("taskboard-hooks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let claude = root.join("claude");
        let t = Target::of(&root.join("Taskboard.app"), true);
        std::fs::create_dir_all(claude.join("plugins")).unwrap();
        std::fs::create_dir_all(&t.plugin).unwrap();
        (Tmp(root), claude, t)
    }

    fn write(p: PathBuf, v: Value) {
        std::fs::write(p, v.to_string()).unwrap();
    }

    /// The plugin installed from `from`, and the hooks `t` writes (none for `None`).
    fn installed(claude: &Path, enabled: bool, from: &Path, t: Option<&Target>) {
        let settings = json!({"enabledPlugins": {PLUGIN: enabled},
            "extraKnownMarketplaces": {"taskboard": {"source": {"source": "directory", "path": from}}}});
        write(claude.join("settings.json"), t.map_or(settings.clone(), |t| with(&settings, t)));
        write(claude.join("plugins/installed_plugins.json"), json!({"version": 2, "plugins": {PLUGIN: [{"scope": "user", "version": "1.0.0"}]}}));
    }

    const CLI: &str = "/Applications/Taskboard.app/Contents/MacOS/tb";

    fn app(at: &str) -> Target {
        Target::of(Path::new(at), true)
    }

    #[::core::prelude::v1::test]
    fn writes_the_events_and_timeouts_the_plugin_had() {
        let t = app("/Applications/Taskboard.app");
        let v = with(&json!({}), &t);
        let events: Vec<&str> = v["hooks"].as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            events,
            ["SessionStart", "UserPromptSubmit", "Stop", "StopFailure", "PreCompact", "PreToolUse", "PostToolUse", "PostToolUseFailure", "SessionEnd", "Notification"]
        );
        assert_eq!(v["hooks"]["Stop"][0], json!({"hooks": [{"type": "command", "command": format!("[ -x '{CLI}' ] && exec '{CLI}' hook Stop || true"), "timeout": 5}]}));
        let git = &v["hooks"]["PostToolUse"][0];
        assert_eq!(git["matcher"], "Bash");
        assert_eq!(git["hooks"][0]["if"], "Bash(git *)");
        assert_eq!(git["hooks"][0]["async"], true);
        assert_eq!(v["hooks"]["SessionEnd"][0]["hooks"][0].get("timeout"), None);
        assert_eq!(
            v["hooks"]["PreToolUse"][1]["hooks"][0]["command"],
            "[ -x '/Applications/Taskboard.app/Contents/Resources/plugin/task-board/bin/wait-hint' ] && exec '/Applications/Taskboard.app/Contents/Resources/plugin/task-board/bin/wait-hint' || true"
        );
        // A path with a quote in it stays one word.
        assert!(command(Run::Tb("Stop"), &app("/Users/o'neil/Taskboard.app")).starts_with(r"[ -x '/Users/o'\''neil/Taskboard.app/Contents/MacOS/tb' ]"));
    }

    #[::core::prelude::v1::test]
    fn install_keeps_the_user_s_settings_and_is_current() {
        let t = app("/Applications/Taskboard.app");
        let user = json!({"model": "opus", "statusLine": {"type": "command", "command": "mine"},
            "hooks": {"Stop": [{"matcher": "", "hooks": [{"type": "command", "command": "/u/stop"}]}]}});
        assert!(matches!(state(&user, &t), State::None));
        let v = with(&user, &t);
        assert!(matches!(state(&v, &t), State::Current));
        assert_eq!(v["statusLine"]["command"], "mine", "the user's status line stays");
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "/u/stop");
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(v.as_object().unwrap().keys().next().unwrap(), "model", "key order is kept");
        assert_eq!(with(&v, &t), v, "reinstalling doesn't add a second copy");
        let mut u = v.clone();
        strip(&mut u, |_| true);
        assert_eq!(u, user, "stripping gives back the original");
    }

    #[::core::prelude::v1::test]
    fn stale_when_the_app_moves_or_an_event_is_missing() {
        let t = app("/Applications/Taskboard.app");
        let v = with(&json!({}), &app("/Volumes/Old/Taskboard.app"));
        assert!(matches!(state(&v, &t), State::Stale(w) if w.contains("another copy of tb")));
        assert_eq!(with(&v, &t), with(&json!({}), &t), "installing replaces the old app's hooks");

        let mut v = with(&json!({}), &t);
        v["hooks"].as_object_mut().unwrap().remove("Notification");
        assert!(matches!(state(&v, &t), State::Stale(w) if w == "Its hooks miss 1 event: Notification."));

        let mut v = with(&json!({}), &t);
        v["hooks"]["Stop"][0]["hooks"][0]["timeout"] = json!(1);
        assert!(matches!(state(&v, &t), State::Stale(w) if w.contains("differ")));
    }

    #[::core::prelude::v1::test]
    fn a_command_does_nothing_when_tb_is_missing() {
        for run in [Run::Tb("Stop"), Run::WaitHint] {
            let o = Command::new("/bin/sh").arg("-c").arg(command(run, &app("/nonexistent/Taskboard.app"))).output().unwrap();
            assert!(o.status.success() && o.stdout.is_empty() && o.stderr.is_empty(), "{o:?}");
        }
    }

    #[::core::prelude::v1::test]
    fn a_command_runs_tb_with_its_event() {
        let (t, _, target) = setup("run");
        std::fs::create_dir_all(target.tb.parent().unwrap()).unwrap();
        std::fs::write(&target.tb, "#!/bin/sh\necho \"$@\"\n").unwrap();
        std::fs::set_permissions(&target.tb, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let o = Command::new("/bin/sh").arg("-c").arg(command(Run::Tb("ToolEnd"), &target)).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout), "hook ToolEnd\n");
        drop(t);
    }

    #[::core::prelude::v1::test]
    fn settings_writes_are_safe() {
        let (_t, claude, target) = setup("write");
        let path = claude.join("settings.json");
        edit_settings(&path, |v| with(v, &target)).unwrap();
        assert!(!path.with_file_name("settings.json.before-taskboard").exists(), "nothing to back up");
        assert!(std::fs::read_to_string(&path).unwrap().ends_with('\n'));

        std::fs::write(&path, r#"{"model": "opus"}"#).unwrap();
        edit_settings(&path, |v| with(v, &target)).unwrap();
        assert_eq!(std::fs::read_to_string(path.with_file_name("settings.json.before-taskboard")).unwrap(), r#"{"model": "opus"}"#);
        assert!(!std::fs::read_to_string(&path).unwrap().ends_with('\n'), "keeps the file's own ending");

        for bad in ["{ not json", "[1]"] {
            std::fs::write(&path, bad).unwrap();
            assert!(edit_settings(&path, |v| with(v, &target)).unwrap_err().contains("won't change it"));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), bad);
        }
    }

    #[::core::prelude::v1::test]
    fn uninstall_takes_out_only_this_app_s_hooks() {
        let (t, claude, target) = setup("uninstall");
        let bundle = t.0.join("Taskboard.app");
        let user = json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "/u/stop"}]}]}});
        write(claude.join("settings.json"), with(&user, &target));
        uninstall(&claude, &t.0.join("Other.app")).unwrap();
        assert!(matches!(state(&read_json(&claude.join("settings.json")).unwrap(), &target), State::Current));
        uninstall(&claude, &bundle).unwrap();
        assert_eq!(read_json(&claude.join("settings.json")).unwrap(), user);
    }

    #[::core::prelude::v1::test]
    fn nothing_installed() {
        let (_t, claude, target) = setup("none");
        assert_eq!(check(&claude, &target), Hooks::NotInstalled);
        write(claude.join("settings.json"), json!({"enabledPlugins": {"other@x": true}}));
        assert_eq!(check(&claude, &target), Hooks::NotInstalled);
    }

    #[::core::prelude::v1::test]
    fn current_with_the_hooks_and_the_plugin_from_this_app() {
        let (_t, claude, target) = setup("current");
        installed(&claude, true, &target.plugin, Some(&target));
        assert_eq!(check(&claude, &target), Hooks::Current);
    }

    #[::core::prelude::v1::test]
    fn an_update_with_the_same_hooks_stays_current() {
        let (t, claude, target) = setup("update");
        installed(&claude, true, &target.plugin, Some(&target));
        // The new release replaces the bundle in place: same paths, same entries.
        std::fs::remove_dir_all(t.0.join("Taskboard.app")).unwrap();
        std::fs::create_dir_all(&target.plugin).unwrap();
        assert_eq!(check(&claude, &target), Hooks::Current);
    }

    #[::core::prelude::v1::test]
    fn reinstall_when_the_hooks_are_missing_or_another_app_s() {
        let (t, claude, target) = setup("hooks");
        // Installed before the hooks moved into settings.json.
        installed(&claude, true, &target.plugin, None);
        assert_eq!(check(&claude, &target), Hooks::NeedsReinstall("Its hooks aren't in Claude Code's settings.".into()));

        installed(&claude, true, &target.plugin, Some(&Target::of(&t.0.join("Old.app"), true)));
        assert!(matches!(check(&claude, &target), Hooks::NeedsReinstall(w) if w.contains("another copy of tb")));

        write(claude.join("settings.json"), with(&json!({}), &target));
        std::fs::remove_file(claude.join("plugins/installed_plugins.json")).unwrap();
        assert!(matches!(check(&claude, &target), Hooks::NeedsReinstall(w) if w.contains("/start command, isn't installed")));
    }

    #[::core::prelude::v1::test]
    fn reinstall_when_the_plugin_still_has_hooks() {
        let (t, claude, target) = setup("leftover");
        let cache = t.0.join("cache/task-board/1.0.0");
        std::fs::create_dir_all(cache.join("hooks")).unwrap();
        std::fs::write(cache.join("hooks/hooks.json"), "{}").unwrap();
        installed(&claude, true, &target.plugin, Some(&target));
        write(claude.join("plugins/installed_plugins.json"), json!({"plugins": {PLUGIN: [{"version": "1.0.0", "installPath": cache}]}}));
        assert!(matches!(check(&claude, &target), Hooks::NeedsReinstall(w) if w.ends_with("each would run twice.")));
        std::fs::remove_file(cache.join("hooks/hooks.json")).unwrap();
        assert_eq!(check(&claude, &target), Hooks::Current);
    }

    #[::core::prelude::v1::test]
    fn reinstall_when_switched_off_moved_or_gone() {
        let (t, claude, target) = setup("stale");
        installed(&claude, false, &target.plugin, Some(&target));
        assert_eq!(check(&claude, &target), Hooks::NeedsReinstall("It's switched off in Claude Code.".into()));

        let checkout = t.0.join("checkout/plugin");
        std::fs::create_dir_all(&checkout).unwrap();
        installed(&claude, true, &checkout, Some(&target));
        assert!(matches!(check(&claude, &target), Hooks::NeedsReinstall(w) if w.ends_with("not from this app.")));

        installed(&claude, true, &t.0.join("Old.app/Contents/Resources/plugin"), Some(&target));
        assert!(matches!(check(&claude, &target), Hooks::NeedsReinstall(w) if w.ends_with("which is gone.")));

        // Listed in known_marketplaces.json only (an older Claude Code).
        write(claude.join("settings.json"), with(&json!({"enabledPlugins": {PLUGIN: true}}), &target));
        write(claude.join("plugins/known_marketplaces.json"), json!({"taskboard": {"source": {"source": "directory", "path": target.plugin}}}));
        assert_eq!(check(&claude, &target), Hooks::Current);

        std::fs::write(claude.join("settings.json"), "{ not json").unwrap();
        assert!(matches!(check(&claude, &target), Hooks::NeedsReinstall(w) if w.ends_with("doesn't parse")));
    }

    #[::core::prelude::v1::test]
    fn reinstall_when_the_cached_version_is_old() {
        let (_t, claude, target) = setup("version");
        installed(&claude, true, &target.plugin, Some(&target));
        std::fs::create_dir_all(target.plugin.join("task-board/.claude-plugin")).unwrap();
        write(target.plugin.join(MANIFEST), json!({"name": "task-board", "version": "1.0.0"}));
        assert_eq!(check(&claude, &target), Hooks::Current);

        write(target.plugin.join(MANIFEST), json!({"name": "task-board", "version": "0.1.0-beta.16"}));
        assert_eq!(check(&claude, &target), Hooks::NeedsReinstall("Claude Code runs version 1.0.0; this app ships 0.1.0-beta.16.".into()));
    }

    /// Runs the real `claude` CLI on a throwaway config (`CLAUDE_CONFIG_DIR`, which `claude`
    /// honours): `cargo test -p taskboard-app hooks -- --ignored`.
    #[::core::prelude::v1::test]
    #[ignore]
    fn install_and_reinstall_with_claude() {
        let (t, claude, _) = setup("claude");
        let (old, new) = (Target::of(&t.0.join("Old.app"), true), Target::of(&t.0.join("New.app"), true));
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin");
        for d in [&old.plugin, &new.plugin] {
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
