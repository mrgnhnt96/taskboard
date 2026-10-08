//! `tb git-credential`: git's credential helper inside the board's Claude sessions, so an agent on a
//! task pushes with Taskboard's GitHub or Bitbucket account without touching the Mac's git setup.
//!
//! `SessionStart` puts it first in the session's helper list through `GIT_CONFIG_*` (see
//! [`session_env`]); nothing is written to `~/.gitconfig`. A terminal without a task, a host Taskboard
//! has no account for, or a board that isn't answering goes to the helpers git would have used anyway
//! (`git credential fill|approve|reject` without these variables), so it behaves exactly as before.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

use crate::client;
use taskboardd::accounts;
use taskboardd::config::Config;

/// How long the board may take to say whether this terminal has a task.
const WHOAMI_TIMEOUT: f64 = 1.0;

/// The variables `SessionStart` exports (into `CLAUDE_ENV_FILE`) so git asks `tb` first: an empty
/// `credential.helper` clears the helpers configured so far, then `tb git-credential` is the only one.
/// None when the session already sets `GIT_CONFIG_COUNT` (it isn't ours to override).
pub fn session_env(tb: &str) -> Option<String> {
    if std::env::var_os("GIT_CONFIG_COUNT").is_some() {
        return None;
    }
    let helper = format!("!{} git-credential", sh_quote(tb));
    let lines = [
        ("GIT_CONFIG_COUNT", "2".to_string()),
        ("GIT_CONFIG_KEY_0", "credential.helper".into()),
        ("GIT_CONFIG_VALUE_0", String::new()),
        ("GIT_CONFIG_KEY_1", "credential.helper".into()),
        ("GIT_CONFIG_VALUE_1", helper),
    ];
    Some(lines.iter().map(|(k, v)| format!("export {k}={}\n", sh_quote(v))).collect())
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The `key=value` lines git sends a helper.
fn parse(input: &str) -> Vec<(String, String)> {
    input.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn field<'a>(f: &'a [(String, String)], k: &str) -> Option<&'a str> {
    f.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str())
}

/// Whether this terminal is working on a task (or visiting one's PR).
fn on_task(cfg: &Config) -> bool {
    let Ok(session) = std::env::var("MIDNA_SESSION") else { return false };
    if session.is_empty() {
        return false;
    }
    match client::request(cfg, "GET", &format!("/whoami?session={session}"), None, WHOAMI_TIMEOUT) {
        Ok(v) => v["task"]["ref"].is_string() || v["visiting"]["ref"].is_string(),
        Err(_) => false,
    }
}

/// git's own helpers, as if `tb` weren't there: `git credential <action>` without the session's
/// `GIT_CONFIG_*`, and without prompting (the git that called us prompts if it has to).
fn pass_through(action: &str, input: &str) -> String {
    let mut cmd = Command::new("git");
    cmd.args(["credential", action]).env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("GIT_CONFIG_") {
            cmd.env_remove(&k);
        }
    }
    let Ok(mut child) = cmd.spawn() else { return String::new() };
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(input.as_bytes());
    }
    match child.wait_with_output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => String::new(),
    }
}

/// `tb git-credential get|store|erase`.
pub fn run(op: &str) -> i32 {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let f = parse(&input);
    let cfg = client::config();
    let ours = (field(&f, "protocol") == Some("https"))
        .then(|| field(&f, "host"))
        .flatten()
        .filter(|_| on_task(&cfg))
        .and_then(|host| accounts::git_credentials(&cfg, host));
    let mut out = std::io::stdout();
    match op {
        "get" => {
            let answer = match ours {
                Some((user, token)) => format!("username={user}\npassword={token}\n"),
                None => pass_through("fill", &input),
            };
            let _ = out.write_all(answer.as_bytes());
        }
        // Taskboard's own token is never handed to (or taken from) the Mac's credential store.
        "store" | "erase" if ours.as_ref().is_none_or(|(_, token)| field(&f, "password") != Some(token.as_str())) => {
            pass_through(if op == "store" { "approve" } else { "reject" }, &input);
        }
        _ => {}
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_git_sends() {
        let f = parse("protocol=https\nhost=github.com\nusername=x-access-token\npassword=a=b\n");
        assert_eq!(field(&f, "host"), Some("github.com"));
        assert_eq!(field(&f, "password"), Some("a=b"), "only the first = splits");
        assert_eq!(field(&f, "path"), None);
    }

    #[test]
    fn session_env_clears_the_helpers_then_asks_tb() {
        let env = session_env("/Applications/Task Board.app/tb").unwrap();
        assert!(env.contains("export GIT_CONFIG_COUNT='2'\n"));
        assert!(env.contains("export GIT_CONFIG_VALUE_0=''\n"), "an empty helper resets the list");
        assert!(env.contains(r"export GIT_CONFIG_VALUE_1='!'\''/Applications/Task Board.app/tb'\'' git-credential'"));
    }
}
