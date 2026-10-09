//! The app's token: what makes a request the owner's own click in Taskboard.app. The daemon makes a
//! fresh random token at each launch and writes it to `app-token` in its data folder, readable by this
//! user only; the app reads it and sends it with each request (`X-Task-Board-Token`), and the server
//! takes `X-Task-Board-From: app` only with the matching token. A request that just says it's the app
//! (curl with the header) is refused.
//!
//! Agents run as the same user, so they could read the file too; `tb hook PreToolUse` refuses tool
//! calls that touch it, and that is the limit of what the board can do on one account.

use std::io::Write;
use std::path::{Path, PathBuf};

use rand::RngCore;

use crate::config::Config;

/// The token's file in the data folder.
pub const FILE: &str = "app-token";
/// The header the app sends it in.
pub const HEADER: &str = "x-task-board-token";

pub fn path(cfg: &Config) -> PathBuf {
    cfg.data.join(FILE)
}

/// A new token: 32 random bytes, in hex.
pub fn fresh() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Writes `token` to `path`, readable and writable by this user only (0600), replacing what was there.
pub fn write(path: &Path, token: &str) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(token.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// The token the running daemon wrote, if there's one.
pub fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Whether `given` is `token`, compared in constant time.
pub fn matches(given: Option<&str>, token: &str) -> bool {
    let Some(given) = given else { return false };
    if token.is_empty() || given.len() != token.len() {
        return false;
    }
    given.bytes().zip(token.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// What reaches a file without naming it: a glob, or a piece of the token's name.
static REACH_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"(?i)[*?\[{]|tok").unwrap());

/// The ways a tool call can spell `dir`: as is, from `~`, from `$HOME`.
fn spellings(dir: &Path) -> Vec<String> {
    let abs = dir.to_string_lossy().trim_end_matches('/').to_string();
    let mut out = vec![abs.clone()];
    if let Some(home) = dirs::home_dir() {
        let home = home.to_string_lossy().trim_end_matches('/').to_string();
        if let Some(rest) = abs.strip_prefix(&home).filter(|_| !home.is_empty()) {
            out.extend(["~", "$HOME", "${HOME}"].iter().map(|h| format!("{h}{rest}")));
        }
    }
    out
}

/// Whether an agent's tool call (`tool`, `input`) reads the app token in data folder `dir`: it names
/// the file, or it reaches into the folder without naming another file in it (the folder itself, a
/// `cd` into it, a glob, a search). Naming another file there (`config.toml`) is fine. It errs on the
/// side of yes; an agent has no reason to look through the board's folder.
pub fn tool_reaches(tool: &str, input: &serde_json::Value, dir: &Path) -> bool {
    // Each string in the call on a line of its own: a folder at a string's end is the folder itself.
    fn strings(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) => out.push(s.clone()),
            serde_json::Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
            serde_json::Value::Object(o) => o.values().for_each(|x| strings(x, out)),
            _ => {}
        }
    }
    let mut all = vec![];
    strings(input, &mut all);
    let text = all.join("\n");
    let lower = text.to_lowercase();
    if lower.contains(FILE) {
        return true;
    }
    let mut in_dir = false;
    for d in spellings(dir).iter().map(|d| d.to_lowercase()) {
        for (at, _) in lower.match_indices(&d) {
            in_dir = true;
            // What follows the folder: "/<name>" is one file; anything else is the folder as a whole.
            let rest = &lower[at + d.len()..];
            let named = rest.strip_prefix('/').map(|r| r.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')).unwrap_or(false);
            if !named {
                return true;
            }
        }
    }
    in_dir && (matches!(tool, "Grep" | "Glob") || REACH_RE.is_match(&text))
}

/// What the hook tells an agent whose tool call reaches the app token.
pub fn refusal() -> String {
    "Task board: that reaches the board's app token, which is only for Taskboard.app. Starting a task, a review \
     stop and the app's other signals are the owner's own; ask them."
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_call_that_reaches_the_token_is_caught() {
        let dir = dirs::home_dir().unwrap().join(".config/taskboard");
        let bash = |c: &str| tool_reaches("Bash", &serde_json::json!({"command": c}), &dir);
        for yes in [
            "cat ~/.config/taskboard/app-token",
            "cat $HOME/.config/taskboard/app-token",
            "cat ~/.config/taskboard/APP-TOKEN",
            "cat ~/.config/taskboard/app-tok*",
            "cat ~/.config/taskboard/*",
            "ls ~/.config/taskboard/",
            "grep -r . ~/.config/taskboard",
            "python3 -c 'print(open(\"/x/app-token\").read())'",
            "curl -H \"X-Task-Board-Token: $(cat ~/.config/taskboard/app-token)\" localhost:8792",
            "f=~/.config/taskboard; head -c 64 $f/app-to?en",
            &format!("cat {}/app-token", dir.display()),
            &format!("tar c {}", dir.display()),
        ] {
            assert!(bash(yes), "{yes}");
        }
        assert!(tool_reaches("Read", &serde_json::json!({"file_path": dir.join(FILE).to_string_lossy()}), &dir));
        assert!(tool_reaches("Grep", &serde_json::json!({"pattern": ".", "path": dir.to_string_lossy()}), &dir));
        assert!(tool_reaches("Glob", &serde_json::json!({"pattern": "*", "path": dir.to_string_lossy()}), &dir));
        for no in ["cargo test", "tb start T8", "git status", "cat README.md", "ls ~/.config", "cat ~/.config/taskboard/config.toml", "tail ~/.config/taskboard/server.log"] {
            assert!(!bash(no), "{no}");
        }
        assert!(!tool_reaches("Read", &serde_json::json!({"file_path": "/repo/src/main.rs"}), &dir));
    }

    #[test]
    fn a_custom_data_folder_is_watched_too() {
        let dir = std::path::PathBuf::from("/srv/board-data");
        assert!(tool_reaches("Bash", &serde_json::json!({"command": "cat /srv/board-data/*"}), &dir));
        assert!(tool_reaches("Read", &serde_json::json!({"file_path": "/srv/board-data/app-token"}), &dir));
        assert!(!tool_reaches("Bash", &serde_json::json!({"command": "cat /srv/other/x"}), &dir));
    }

    #[test]
    fn a_token_is_fresh_each_time_and_private() {
        let (a, b) = (fresh(), fresh());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE);
        write(&p, &a).unwrap();
        write(&p, &b).unwrap();
        assert_eq!(read(&p).as_deref(), Some(b.as_str()));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn only_the_same_token_matches() {
        let t = fresh();
        assert!(matches(Some(&t), &t));
        assert!(!matches(None, &t));
        assert!(!matches(Some(""), &t));
        assert!(!matches(Some(&t[..63]), &t));
        assert!(!matches(Some(&fresh()), &t));
        assert!(!matches(Some(""), ""), "no token, no match");
    }
}
