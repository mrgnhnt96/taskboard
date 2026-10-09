//! The app's token: what makes a request the owner's own click in Taskboard.app. The daemon makes a
//! fresh random token at each launch and writes it to `app-token` in its data folder, readable by this
//! user only; the app reads it and sends it with each request (`X-Task-Board-Token`), and the server
//! takes `X-Task-Board-From: app` only with the matching token. A request that just says it's the app
//! (curl with the header) is refused.
//!
//! Only a daemon with no team signature (`cargo run`, `scripts/dev-app.sh`, tests) writes or takes the
//! token; a signed one checks the app's code signature on the app socket instead (`apporigin`).
//! Agents run as the same user, so they could read the file; `tb hook PreToolUse` refuses tool calls
//! that reach it, however they spell it, but a guard on what a call says can't be complete.

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

/// Shell variables (`$X`, `${X}`) and a word-leading `~`.
static VAR_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?").unwrap());
static TILDE_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"(^|[\s=:;|&(<>])~(/|$|[\s;|&)])").unwrap());
/// Commands that walk a folder's tree.
static RECURSIVE_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
    regex::Regex::new(r"(?i)\b(find|rg|ag|ack|fd|tar|zip|rsync|du|tree|ditto)\b|\b(grep|egrep|ls|cp|chmod|chown)\b[^\n;|&]*\s-[a-zA-Z]*[rRa]").unwrap()
});

/// The call's text as a shell would read it: quotes and backslashes gone (`app-to''ken` is
/// `app-token`), variables filled in from this environment (`$TASKBOARD_DATA` is always `dir`), `~` as
/// the home folder.
fn as_shell_reads(text: &str, dir: &Path) -> String {
    let unquoted: String = text.chars().filter(|c| !matches!(c, '\'' | '"' | '\\' | '`')).collect();
    let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_default();
    let vars = VAR_RE.replace_all(&unquoted, |c: &regex::Captures| match &c[1] {
        "TASKBOARD_DATA" => dir.to_string_lossy().to_string(),
        "HOME" if !home.is_empty() => home.clone(),
        name => std::env::var(name).ok().filter(|v| v.starts_with('/')).unwrap_or_else(|| c[0].to_string()),
    });
    if home.is_empty() {
        return vars.into_owned();
    }
    // Twice: a match eats the separator the next one starts with.
    let once = TILDE_RE.replace_all(&vars, |c: &regex::Captures| format!("{}{home}{}", &c[1], &c[2])).into_owned();
    TILDE_RE.replace_all(&once, |c: &regex::Captures| format!("{}{home}{}", &c[1], &c[2])).into_owned()
}

/// Whether glob `pat` (`*`, `?`, `[…]`, `{a,b}`) matches `name`, ignoring case.
fn glob_matches(pat: &str, name: &str) -> bool {
    let mut re = String::from("(?i)^");
    let mut chars = pat.chars().peekable();
    let mut braces = 0;
    while let Some(c) = chars.next() {
        match c {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            '[' => {
                let mut class = String::new();
                for d in chars.by_ref() {
                    if d == ']' {
                        break;
                    }
                    class.push(d);
                }
                let class = class.strip_prefix('!').map(|c| format!("^{c}")).unwrap_or(class);
                re.push_str(&format!("[{}]", class.replace('\\', "\\\\")));
            }
            '{' => {
                braces += 1;
                re.push_str("(?:");
            }
            ',' if braces > 0 => re.push('|'),
            '}' if braces > 0 => {
                braces -= 1;
                re.push(')');
            }
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push_str(&")".repeat(braces));
    re.push('$');
    regex::Regex::new(&re).map(|r| r.is_match(name)).unwrap_or(true)
}

fn has_glob(s: &str) -> bool {
    s.contains(['*', '?', '[', '{'])
}

/// `p` made absolute (from `cwd`), `.` and `..` worked out, and symlinks resolved as far as the path
/// exists (`/tmp` is `/private/tmp`). Lowercase: macOS folders ignore case.
fn resolve(p: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let p = Path::new(p);
    let abs = if p.is_absolute() { p.to_path_buf() } else { cwd?.join(p) };
    let mut clean = PathBuf::from("/");
    for c in abs.components() {
        match c {
            std::path::Component::ParentDir => {
                clean.pop();
            }
            std::path::Component::Normal(n) => clean.push(n),
            _ => {}
        }
    }
    let mut rest = vec![];
    let mut head = clean.clone();
    let real = loop {
        if let Ok(r) = std::fs::canonicalize(&head) {
            break r;
        }
        match (head.file_name().map(|n| n.to_os_string()), head.parent().map(Path::to_path_buf)) {
            (Some(n), Some(up)) => {
                rest.push(n);
                head = up;
            }
            _ => break head,
        }
    };
    let full = rest.into_iter().rev().fold(real, |acc, n| acc.join(n));
    Some(PathBuf::from(full.to_string_lossy().to_lowercase()))
}

/// Whether an agent's tool call (`tool`, `input`, run in `cwd`) reads the app token in data folder
/// `dir`: it names the file (quoted in pieces or not, or by a glob such as `app-*`), it reaches into
/// the folder without naming another file in it (the folder itself, a `cd` into it, a glob, a search;
/// spelled any way that resolves to it: `/tmp` for `/private/tmp`, `./`, `..`, a symlink, `$HOME`,
/// `$TASKBOARD_DATA`), or it walks a folder above it (`find ~`, Grep or Glob from `~`). Naming another
/// file there (`config.toml`) is fine. It errs on the side of yes; an agent has no reason to look
/// through the board's folder.
///
/// A guard on what a call says can't be complete: a script can build the path at run time. It only
/// matters for a daemon without a team signature, which is the only one that writes the token
/// (`apporigin`).
pub fn tool_reaches(tool: &str, input: &serde_json::Value, dir: &Path, cwd: Option<&Path>) -> bool {
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
    let raw = all.join("\n");
    let text = as_shell_reads(&raw, dir);
    let lower = text.to_lowercase();
    if lower.contains(FILE) || raw.to_lowercase().contains(FILE) || text.contains("TASKBOARD_DATA") {
        return true;
    }
    let real_dir = resolve(&dir.to_string_lossy(), None).unwrap_or_else(|| dir.to_path_buf());
    // Paths are only read where they're paths: a command, or a read or search tool's arguments (a
    // file being written can hold `/*` or `find /` as text).
    let paths = matches!(tool, "Bash" | "Read" | "Grep" | "Glob" | "LS");
    let recursive = matches!(tool, "Grep" | "Glob") || (tool == "Bash" && RECURSIVE_RE.is_match(&text));
    let words = text.split(|c: char| c.is_whitespace() || ";|&()<>=,:".contains(c)).filter(|w| !w.is_empty());
    for w in words {
        // A glob for the token's name, anywhere (`cat $D/app-*`): one that matches it and not every
        // token (`*token*` is someone's search of their own code). `*` alone is judged by its folder.
        let segs: Vec<&str> = w.split('/').collect();
        if segs.iter().any(|s| has_glob(s) && glob_matches(s, FILE) && !glob_matches(s, "token")) {
            return true;
        }
        if !paths || !(w.starts_with('/') || w.starts_with('.') || w.contains('/')) {
            continue;
        }
        // The folder part before any glob, resolved.
        let plain: Vec<&str> = segs.iter().take_while(|s| !has_glob(s)).copied().collect();
        let globbed = plain.len() < segs.len();
        let base = match plain.as_slice() {
            [] => ".".to_string(),
            [""] => "/".to_string(),
            _ => plain.join("/"),
        };
        let Some(p) = resolve(&base, cwd) else { continue };
        if p == real_dir {
            return true;
        }
        if let Ok(inside) = p.strip_prefix(&real_dir) {
            if inside.components().count() == 1 && globbed {
                return true;
            }
            continue;
        }
        if real_dir.starts_with(&p) && (recursive || globbed) {
            return true;
        }
    }
    // Grep and Glob with no path search the working folder.
    if matches!(tool, "Grep" | "Glob") && input.get("path").and_then(|v| v.as_str()).is_none_or(str::is_empty) {
        if let Some(c) = cwd.and_then(|c| resolve(&c.to_string_lossy(), None)) {
            if real_dir.starts_with(&c) {
                return true;
            }
        }
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
        let bash = |c: &str| tool_reaches("Bash", &serde_json::json!({"command": c}), &dir, None);
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
        assert!(tool_reaches("Read", &serde_json::json!({"file_path": dir.join(FILE).to_string_lossy()}), &dir, None));
        assert!(tool_reaches("Grep", &serde_json::json!({"pattern": ".", "path": dir.to_string_lossy()}), &dir, None));
        assert!(tool_reaches("Glob", &serde_json::json!({"pattern": "*", "path": dir.to_string_lossy()}), &dir, None));
        for no in ["cargo test", "tb start T8", "git status", "cat README.md", "ls ~/.config", "cat ~/.config/taskboard/config.toml", "tail ~/.config/taskboard/server.log"] {
            assert!(!bash(no), "{no}");
        }
        assert!(!tool_reaches("Read", &serde_json::json!({"file_path": "/repo/src/main.rs"}), &dir, None));
    }

    #[test]
    fn a_custom_data_folder_is_watched_too() {
        let dir = std::path::PathBuf::from("/srv/board-data");
        assert!(tool_reaches("Bash", &serde_json::json!({"command": "cat /srv/board-data/*"}), &dir, None));
        assert!(tool_reaches("Read", &serde_json::json!({"file_path": "/srv/board-data/app-token"}), &dir, None));
        assert!(!tool_reaches("Bash", &serde_json::json!({"command": "cat /srv/other/x"}), &dir, None));
    }

    /// The spellings that got through in beta.14 (#116).
    #[test]
    fn quoting_globs_and_other_spellings_are_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        // A data folder under /tmp, as `/private/tmp` (what it resolves to) or `/tmp`.
        let real = std::fs::canonicalize(tmp.path()).unwrap();
        let dir = real.join("board");
        std::fs::create_dir_all(&dir).unwrap();
        let repo = real.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let link = real.join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        let d = dir.display().to_string();
        let parent = real.display().to_string();
        let tmp_spelling = d.strip_prefix("/private").map(str::to_string);
        let bash = |c: &str| tool_reaches("Bash", &serde_json::json!({"command": c}), &dir, Some(&repo));
        let mut yes = vec![
            "cat $DATA/./app-to''ken".to_string(),
            "cat \"$TASKBOARD_DATA\"/app-to\"ken\"".into(),
            "cat $TASKBOARD_DATA/*".into(),
            "cat ${TASKBOARD_DATA}".into(),
            "python3 -c 'import os; print(open(os.environ[\"TASKBOARD_DATA\"] + \"/x\").read())'".into(),
            format!("find {parent} -name 'app-*' -exec cat {{}} +"),
            format!("find {parent} -type f -exec cat {{}} +"),
            format!("grep -r . {parent}"),
            format!("rg . {parent}"),
            format!("cat {parent}/*/*"),
            format!("cat {d}/./x/../app-tok\\en"),
            format!("cat {d}/../board/*"),
            format!("cat {d}/a[p]p-t?ken"),
            format!("cat {d}/app-{{tok,x}}en"),
            format!("cat {}/*", link.display()),
            "cat ../board/*".into(),
            "cd .. && cat board/app-*".into(),
            "cat x/app-*".into(),
            "find / -name 'app*' -print".into(),
        ];
        if let Some(t) = &tmp_spelling {
            yes.push(format!("cat {t}/*"));
            yes.push(format!("ls {t}"));
        }
        for c in &yes {
            assert!(bash(c), "{c}");
        }
        for (tool, input) in [
            ("Grep", serde_json::json!({"pattern": ".", "path": parent, "glob": "app-*"})),
            ("Grep", serde_json::json!({"pattern": "[0-9a-f]{64}", "path": parent})),
            ("Glob", serde_json::json!({"pattern": "**/app-*", "path": parent})),
            ("Glob", serde_json::json!({"pattern": "*/*", "path": parent})),
            ("Read", serde_json::json!({"file_path": format!("{}/app-token", link.display())})),
            ("Write", serde_json::json!({"file_path": "/repo/x.sh", "content": format!("#!/bin/sh\ncat {d}/app-to''ken\n")})),
            ("Write", serde_json::json!({"file_path": "/repo/x.py", "content": "print(open(os.environ['TASKBOARD_DATA'] + '/app-tok' + 'en').read())"})),
        ] {
            assert!(tool_reaches(tool, &input, &dir, Some(&repo)), "{tool} {input}");
        }
        // Grep or Glob with no path searches the working folder: a folder above the board's is out.
        assert!(tool_reaches("Grep", &serde_json::json!({"pattern": "x"}), &dir, Some(&real)));

        // Everyday work in the repo is fine.
        for c in [
            "find . -name '*.rs' -path '*/src/*'",
            "grep -rn token src",
            "rg -n 'app_token' crates",
            "find . -name '*token*'",
            "ls *",
            "cat src/*.rs",
            "cargo test -p taskboardd",
            "sed -i '' 's/a/b/' src/main.rs",
            "curl -s https://example.com/x",
        ] {
            assert!(!bash(c), "{c}");
        }
        for (tool, input) in [
            ("Grep", serde_json::json!({"pattern": "fn main", "path": repo.display().to_string()})),
            ("Grep", serde_json::json!({"pattern": "fn main"})),
            ("Glob", serde_json::json!({"pattern": "**/*.rs"})),
            ("Glob", serde_json::json!({"pattern": "**/*token*.rs"})),
            ("Write", serde_json::json!({"file_path": "/repo/src/x.rs", "content": "/* find / -type f */\nfn main() {}"})),
            ("Read", serde_json::json!({"file_path": format!("{d}/config.toml")})),
        ] {
            assert!(!tool_reaches(tool, &input, &dir, Some(&repo)), "{tool} {input}");
        }
    }

    #[test]
    fn globs_match_like_the_shell() {
        for (pat, name) in [("app-*", "app-token"), ("a?p-t[o]ken", "app-token"), ("app-{x,tok}en", "app-token"), ("[!b]pp-*", "app-token"), ("*", "x")] {
            assert!(glob_matches(pat, name), "{pat} {name}");
        }
        for (pat, name) in [("app-*", "token"), ("*.rs", "app-token"), ("[b]pp-*", "app-token"), ("app-{x,y}", "app-token")] {
            assert!(!glob_matches(pat, name), "{pat} {name}");
        }
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
