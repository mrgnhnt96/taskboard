// Excerpt of crates/taskboard-app/src/hooks.rs, lines 1-411
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
//!   gone), or still carries hooks of its own (they'd fire twice);
//! - **current**: the hooks are this app's, and the plugin is on and loaded from this app.
//!
//! The plugin's version isn't checked either: Claude Code serves it from the bundle, so an update
//! brings its new skill and command with it. Its `plugin.json` version is a hash of what it carries
//! (`scripts/plugin-version.sh`), so it moves only when they change.
//!
//! Installing (or reinstalling) points the marketplace at this app and installs the plugin with the
//! `claude` CLI, which keeps Claude Code's own files consistent, then writes the hook entries.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const PLUGIN: &str = "task-board@taskboard";
const MARKETPLACE: &str = "taskboard";
/// The plugin's PreToolUse script for Bash, inside the marketplace folder.
const WAIT_HINT: &str = "task-board/bin/wait-hint";

/// Tools whose PreToolUse the board sees.
const TOOLS: &str = "Bash|Agent|Task|Workflow|EnterWorktree|mcp__.*|Edit|Write|MultiEdit|NotebookEdit|Read|Grep|Glob";

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

/// The plugin's state: installed, on, loaded from `want`, without hooks of its own.
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
            Hooks::Current
        }
        Some(p) if !p.is_dir() => Hooks::NeedsReinstall(format!("Claude Code loads it from {}, which is gone.", p.display())),
        Some(p) => Hooks::NeedsReinstall(format!("Claude Code loads it from {}, not from this app.", p.display())),
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

