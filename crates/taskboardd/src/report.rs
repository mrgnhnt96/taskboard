//! "Report an issue" (the app's Help menu, `tb report`): a new GitHub issue on the board's repo with
//! this Mac's details filled in. The repo is public, so the body carries versions and counts only: no
//! paths, project or task names, prompts, terminal output or logs. The redacted debug export (`tb
//! export`) is where the reporter can share more, when they choose.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};

use crate::config::Config;

pub const REPO: &str = "mrgnhnt96/taskboard";

/// What the issue body reports. [`gather`] runs processes and reads the board, so it runs off the UI thread.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    /// The versions of Taskboard.app, `tb` and the Claude plugin, each empty when it isn't found.
    pub app: String,
    pub tb: String,
    pub plugin: String,
    /// The daemon's version, or `None` when it didn't answer.
    pub daemon: Option<String>,
    pub daemon_uptime: Option<u64>,
    pub channel: String,
    /// "/Applications", "dev build (cargo)", ...
    pub install: String,
    /// "15.1 (24B83)".
    pub macos: String,
    /// "Mac15,6 · Apple M3 Pro · 36 GB".
    pub hardware: String,
    /// "arm64", "x86_64 (Rosetta)".
    pub arch: String,
    pub shell: String,
    /// `None` when the board couldn't be read.
    pub board: Option<Counts>,
    /// Running agents, deduplicated: "Claude 2.1.3 (opus)".
    pub agents: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    pub goals: usize,
    /// Task state → how many, "failed" for failed tasks whatever their state.
    pub tasks: BTreeMap<String, usize>,
    pub terminals: usize,
    pub projects: usize,
}

/// This Mac's details. `made_by` is "app" or "tb": that binary's own version stands in when the other
/// copy can't be read.
pub fn gather(cfg: &Config, made_by: &str) -> Facts {
    let home = std::env::var("HOME").unwrap_or_default();
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let own = env!("CARGO_PKG_VERSION").to_string();
    let sysctl = |k: &str| output("/usr/sbin/sysctl", &["-n", k]);
    let version = output("/usr/bin/sw_vers", &["-productVersion"]);
    let build = output("/usr/bin/sw_vers", &["-buildVersion"]);
    let memory = sysctl("hw.memsize").parse::<u64>().map(|b| format!("{} GB", b >> 30)).unwrap_or_default();
    let hardware = [sysctl("hw.model"), sysctl("machdep.cpu.brand_string"), memory].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    let rosetta = sysctl("sysctl.proc_translated") == "1";
    let bundle = app_bundle(&exe);
    let app = output("/usr/bin/plutil", &["-extract", "CFBundleShortVersionString", "raw", "-o", "-", &bundle.join("Contents/Info.plist").to_string_lossy()]);
    let tb = output(&Path::new(&home).join(".local/bin/tb").to_string_lossy(), &["--version"]);
    let (daemon, daemon_uptime) = daemon(cfg);
    Facts {
        app: if app.is_empty() && made_by == "app" { own.clone() } else { app },
        tb: match tb.strip_prefix("tb ") {
            Some(v) => v.to_string(),
            None if made_by == "tb" => own.clone(),
            None => String::new(),
        },
        plugin: plugin_version(&claude_dir(&home)),
        daemon,
        daemon_uptime,
        channel: if own.contains("beta") { "beta" } else { "stable" }.into(),
        install: install_kind(&exe, &home),
        macos: if build.is_empty() { version } else { format!("{version} ({build})") },
        hardware,
        arch: format!("{}{}", if std::env::consts::ARCH == "aarch64" { "arm64" } else { std::env::consts::ARCH }, if rosetta { " (Rosetta)" } else { "" }),
        shell: std::env::var("SHELL").ok().and_then(|s| s.rsplit('/').next().map(str::to_string)).unwrap_or_default(),
        board: counts(&cfg.db_path()),
        agents: agents(&crate::midna::call_exe(&cfg.midna, "session.list", &json!({}), 5.0).unwrap_or(Value::Null)),
    }
}

fn output(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// The Taskboard.app this binary runs from, else the installed one.
fn app_bundle(exe: &str) -> PathBuf {
    match exe.find(".app/") {
        Some(i) => PathBuf::from(&exe[..i + 4]),
        None => PathBuf::from("/Applications/Taskboard.app"),
    }
}

fn claude_dir(home: &str) -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => Path::new(home).join(".claude"),
    }
}

/// The task-board plugin's versions Claude Code has installed, from its `installed_plugins.json`.
fn plugin_version(claude: &Path) -> String {
    let text = std::fs::read_to_string(claude.join("plugins/installed_plugins.json")).unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let mut versions: Vec<String> = v["plugins"]["task-board@taskboard"].as_array().into_iter().flatten().filter_map(|e| e["version"].as_str()).map(clean).collect();
    versions.dedup();
    versions.join(", ")
}

/// The daemon's version and uptime from its root answer, `(None, None)` when it isn't running.
fn daemon(cfg: &Config) -> (Option<String>, Option<u64>) {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(2)).build();
    let root = format!("{}/tasks", cfg.url().trim_end_matches('/'));
    let answer = agent.get(&root).call().ok().and_then(|r| r.into_string().ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok());
    match answer {
        Some(v) if v["board"] == "taskboardd" => (Some(v["version"].as_str().map(clean).unwrap_or_default()), v["uptime_secs"].as_u64()),
        _ => (None, None),
    }
}

/// Where the binary runs from, without the home folder's name.
fn install_kind(exe: &str, home: &str) -> String {
    if exe.starts_with("/Applications/") {
        "/Applications".into()
    } else if !home.is_empty() && exe.starts_with(&format!("{home}/Applications/")) {
        "~/Applications".into()
    } else if exe.contains("/target/debug/") || exe.contains("/target/release/") {
        "dev build (cargo)".into()
    } else if exe.contains(".app/") {
        "app bundle elsewhere".into()
    } else {
        "other".into()
    }
}

/// The board's counts, read-only: goals not archived, tasks by state, open terminals and projects.
fn counts(db: &Path) -> Option<Counts> {
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
    let one = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).map(|n| n as usize).unwrap_or(0);
    let mut tasks = BTreeMap::new();
    if let Ok(mut st) = conn.prepare("SELECT CASE WHEN failed THEN 'failed' ELSE COALESCE(status, '') END, COUNT(*) FROM tasks GROUP BY 1") {
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)));
        for (state, n) in rows.into_iter().flatten().flatten() {
            // A state is one of the board's own words; anything else is counted, never named.
            let state = if !state.is_empty() && state.len() <= 20 && state.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') { state } else { "other".into() };
            *tasks.entry(state).or_insert(0) += n as usize;
        }
    }
    let midna: Vec<Value> = conn
        .query_row("SELECT value FROM settings WHERE key = 'midna_projects'", [], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut projects: std::collections::BTreeSet<String> = midna.iter().filter_map(|p| p["name"].as_str()).filter(|n| !n.is_empty()).map(str::to_string).collect();
    for sql in ["SELECT DISTINCT project FROM tasks", "SELECT DISTINCT project FROM goals", "SELECT DISTINCT project FROM sessions"] {
        if let Ok(mut st) = conn.prepare(sql) {
            projects.extend(st.query_map([], |r| r.get::<_, Option<String>>(0)).into_iter().flatten().flatten().flatten().filter(|n| !n.is_empty()));
        }
    }
    Some(Counts {
        goals: one("SELECT COUNT(*) FROM goals WHERE NOT COALESCE(archived, 0)"),
        tasks,
        terminals: one("SELECT COUNT(*) FROM sessions WHERE COALESCE(status, '') != 'gone'"),
        projects: projects.len(),
    })
}

/// The agents running in Midna's terminals, from `session.list`: "Claude 2.1.3 (opus)", each once.
pub fn agents(sessions: &Value) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for s in sessions.as_array().into_iter().flatten() {
        let name = match s["agent"].as_str() {
            Some("claude") => "Claude",
            Some("codex") => "Codex",
            Some(a) if !a.is_empty() => "other agent",
            _ => continue,
        };
        if s["status"]["state"] == "exited" {
            continue;
        }
        let info = &s["agent_info"];
        let (version, model) = (info["version"].as_str().map(clean).filter(|v| !v.is_empty()), info["model"].as_str().map(clean).filter(|v| !v.is_empty()));
        let line = match (version, model) {
            (Some(v), Some(m)) => format!("{name} {v} ({m})"),
            (Some(v), None) => format!("{name} {v}"),
            (None, Some(m)) => format!("{name} ({m})"),
            (None, None) => name.to_string(),
        };
        if !out.contains(&line) {
            out.push(line);
        }
    }
    out
}

/// A version or model id as the body shows it: its version-like characters only, at most 40 of them,
/// so a stray path or sentence can't ride along.
fn clean(s: &str) -> String {
    let s = s.trim();
    if s.len() > 40 || !s.chars().all(|c| c.is_ascii_alphanumeric() || ".-_+[] ".contains(c)) {
        return "unknown".into();
    }
    s.to_string()
}

fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    match (d, h) {
        (0, 0) => format!("{m}m"),
        (0, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

/// The issue body: prompts for the report, the environment table, and where to find the debug export.
pub fn body(f: &Facts) -> String {
    let daemon = match (&f.daemon, f.daemon_uptime) {
        (Some(v), Some(up)) => format!("{v} · running, up {}", uptime(up)),
        (Some(v), None) => format!("{v} · running"),
        (None, _) => "not running".into(),
    };
    let mut rows: Vec<(&str, String)> = vec![
        ("Taskboard", if f.app.is_empty() { String::new() } else { format!("{} ({})", f.app, f.channel) }),
        ("taskboardd", daemon),
        ("tb", f.tb.clone()),
        ("Claude plugin", if f.plugin.is_empty() { "not installed".into() } else { f.plugin.clone() }),
        ("Installed in", f.install.clone()),
        ("macOS", f.macos.clone()),
        ("Mac", f.hardware.clone()),
        ("Arch", f.arch.clone()),
        ("Shell", f.shell.clone()),
    ];
    match &f.board {
        Some(c) => {
            let tasks = if c.tasks.is_empty() { "none".into() } else { c.tasks.iter().map(|(s, n)| format!("{n} {s}")).collect::<Vec<_>>().join(", ") };
            rows.push(("Board", format!("{}, {}, {}", plural(c.goals, "goal"), plural(c.terminals, "terminal"), plural(c.projects, "project"))));
            rows.push(("Tasks", tasks));
        }
        None => rows.push(("Board", "couldn't be read".into())),
    }
    rows.push(("Agents", if f.agents.is_empty() { "none running".into() } else { f.agents.join(", ") }));
    let table: String = rows.into_iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("| {k} | {} |\n", v.replace('|', "\\|"))).collect();
    format!(
        "### What happened\n\n\n\n### What you expected\n\n\n\n### Steps to reproduce\n\n1. \n\n\
         ### Environment\n\n| | |\n|---|---|\n{table}\n\
         To share more, attach the redacted debug export: `tb export`, or File ▸ Export for Debugging… in Taskboard. \
         It hides names, paths and text, but look it over before you post it.\n\n\
         <!-- Filled in by Taskboard's \"Report an issue\": versions and counts only, no paths, names, prompts, terminal output or logs. -->\n"
    )
}

/// `https://github.com/<REPO>/issues/new?body=…`.
pub fn issue_url(f: &Facts) -> String {
    format!("https://github.com/{REPO}/issues/new?body={}", encode(&body(f)))
}

/// Percent-encode a query value (everything but RFC 3986 unreserved characters).
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            app: "0.1.0-beta.40".into(),
            tb: "0.1.0-beta.40".into(),
            plugin: "0.1.0-beta.40".into(),
            daemon: Some("0.1.0-beta.40".into()),
            daemon_uptime: Some(3 * 3600 + 120),
            channel: "beta".into(),
            install: "/Applications".into(),
            macos: "15.1 (24B83)".into(),
            hardware: "Mac15,6 · Apple M3 Pro · 36 GB".into(),
            arch: "arm64".into(),
            shell: "zsh".into(),
            board: Some(Counts { goals: 2, tasks: BTreeMap::from([("done".into(), 7), ("working".into(), 1)]), terminals: 3, projects: 1 }),
            agents: vec!["Claude 2.1.3 (opus)".into()],
        }
    }

    #[test]
    fn body_lists_the_environment() {
        let b = body(&facts());
        assert!(b.starts_with("### What happened"));
        for row in [
            "| Taskboard | 0.1.0-beta.40 (beta) |",
            "| taskboardd | 0.1.0-beta.40 · running, up 3h 2m |",
            "| tb | 0.1.0-beta.40 |",
            "| Claude plugin | 0.1.0-beta.40 |",
            "| Installed in | /Applications |",
            "| macOS | 15.1 (24B83) |",
            "| Arch | arm64 |",
            "| Board | 2 goals, 3 terminals, 1 project |",
            "| Tasks | 7 done, 1 working |",
            "| Agents | Claude 2.1.3 (opus) |",
        ] {
            assert!(b.contains(row), "{row} missing from\n{b}");
        }
        assert!(b.contains("`tb export`") && b.contains("Export for Debugging"));
    }

    #[test]
    fn missing_pieces_say_so() {
        let b = body(&Facts { daemon: None, plugin: String::new(), shell: String::new(), board: None, agents: vec![], ..facts() });
        assert!(b.contains("| taskboardd | not running |"));
        assert!(b.contains("| Claude plugin | not installed |"));
        assert!(b.contains("| Board | couldn't be read |"));
        assert!(b.contains("| Agents | none running |"));
        assert!(!b.contains("| Shell |"));
    }

    #[test]
    fn url_is_encoded() {
        let u = issue_url(&facts());
        assert!(u.starts_with("https://github.com/mrgnhnt96/taskboard/issues/new?body=%23%23%23%20What"));
        assert!(!u[u.find('?').unwrap() + 1..].contains([' ', '\n', '|', '#', '&']));
        assert_eq!(encode("a b/✓"), "a%20b%2F%E2%9C%93");
    }

    #[test]
    fn install_kind_hides_the_home_folder() {
        assert_eq!(install_kind("/Applications/Taskboard.app/Contents/MacOS/tb", "/Users/x"), "/Applications");
        assert_eq!(install_kind("/Users/x/Applications/Taskboard.app/Contents/MacOS/tb", "/Users/x"), "~/Applications");
        assert_eq!(install_kind("/Users/x/dev/taskboard/target/debug/tb", "/Users/x"), "dev build (cargo)");
        assert_eq!(install_kind("/Volumes/T/Taskboard.app/Contents/MacOS/tb", "/Users/x"), "app bundle elsewhere");
        assert_eq!(app_bundle("/Volumes/T/Taskboard.app/Contents/MacOS/tb"), PathBuf::from("/Volumes/T/Taskboard.app"));
    }

    #[test]
    fn agents_are_named_with_version_and_model_once() {
        let sessions = json!([
            {"agent": "claude", "status": {"state": "working"}, "cwd": "/Users/x/secret-project", "agent_info": {"version": "2.1.3", "model": "opus", "conversation_id": "abc"}},
            {"agent": "claude", "status": {"state": "idle"}, "agent_info": {"version": "2.1.3", "model": "opus"}},
            {"agent": "codex", "status": {"state": "idle"}, "agent_info": {"version": "/Users/x/bin/codex"}},
            {"agent": "claude", "status": {"state": "exited"}, "agent_info": {"version": "1.0.0"}},
            {"agent": null, "name": "zsh"},
        ]);
        assert_eq!(agents(&sessions), vec!["Claude 2.1.3 (opus)", "Codex unknown"]);
    }

    /// A seeded board's names, paths, prompts and text, and a terminal's cwd and conversation, none of
    /// which may reach the public issue.
    #[test]
    fn nothing_sensitive_reaches_the_issue() {
        let data = std::env::temp_dir().join(format!("tb-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        std::fs::create_dir_all(&data).unwrap();
        let cfg = Config::for_tests(&data);
        let secrets = ["Secret Launch Plan", "acme-private", "/Users/sam/code/acme-private", "rotate the prod key", "hunter2-output", "Hidden Goal Name", "sam-terminal"];
        {
            let db = crate::db::Db::open(&cfg.db_path()).unwrap();
            db.insert("goals", vec![("name", json!(secrets[5])), ("project", json!(secrets[1])), ("repo_path", json!(secrets[2]))]).unwrap();
            db.insert("tasks", vec![("title", json!(secrets[0])), ("detail", json!(secrets[3])), ("project", json!(secrets[1])), ("repo_path", json!(secrets[2])), ("status", json!("working")), ("latest", json!(secrets[4]))]).unwrap();
            db.insert("tasks", vec![("title", json!("x")), ("status", json!("Weird State /Users/sam")), ("failed", json!(0))]).unwrap();
            db.insert("tasks", vec![("title", json!("y")), ("status", json!("done")), ("failed", json!(1))]).unwrap();
            db.insert("sessions", vec![("id", json!("s1")), ("name", json!(secrets[6])), ("project", json!("other-proj")), ("project_path", json!(secrets[2])), ("status", json!("idle"))]).unwrap();
            db.insert("sessions", vec![("id", json!("s2")), ("name", json!("old")), ("status", json!("gone"))]).unwrap();
        }
        let c = counts(&cfg.db_path()).unwrap();
        assert_eq!(c, Counts { goals: 1, tasks: BTreeMap::from([("failed".into(), 1), ("other".into(), 1), ("working".into(), 1)]), terminals: 1, projects: 2 });
        let sessions = json!([{"agent": "claude", "name": secrets[6], "cwd": secrets[2], "title": secrets[0], "agent_info": {"version": "2.1.3", "model": "opus", "conversation_id": secrets[4]}}]);
        let f = Facts { board: Some(c), agents: agents(&sessions), ..facts() };
        let (b, u) = (body(&f), issue_url(&f));
        for s in secrets.iter().chain(&["/Users/", "Weird"]) {
            assert!(!b.contains(s), "{s} leaked into\n{b}");
            assert!(!u.contains(&encode(s)), "{s} leaked into the URL");
        }
        let _ = std::fs::remove_dir_all(&data);
    }
}
