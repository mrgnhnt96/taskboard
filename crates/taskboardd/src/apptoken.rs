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

/// Names a glob for the token's name could just as well be aimed at: a glob that matches one of these
/// too (`*token*`, or a letter and a star) is someone's search of their own files.
const DECOYS: &[&str] = &["token", "app"];

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

/// `$X` and `${X}` filled in from this environment (`$TASKBOARD_DATA` is always `dir`, `$HOME` the home
/// folder); a variable that isn't a folder here stays as it is.
fn fill_vars(text: &str, dir: &Path, home: &str) -> String {
    VAR_RE
        .replace_all(text, |c: &regex::Captures| match &c[1] {
            "TASKBOARD_DATA" => dir.to_string_lossy().to_string(),
            "HOME" if !home.is_empty() => home.to_string(),
            name => std::env::var(name).ok().filter(|v| v.starts_with('/')).unwrap_or_else(|| c[0].to_string()),
        })
        .into_owned()
}

/// The call's text as a shell would read it: quotes and backslashes gone (`ta''sk` is `task`),
/// variables filled in, `~` as the home folder.
fn as_shell_reads(text: &str, dir: &Path) -> String {
    let unquoted: String = text.chars().filter(|c| !matches!(c, '\'' | '"' | '\\' | '`')).collect();
    let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_default();
    let vars = fill_vars(&unquoted, dir, &home);
    if home.is_empty() {
        return vars;
    }
    // Twice: a match eats the separator the next one starts with.
    let once = TILDE_RE.replace_all(&vars, |c: &regex::Captures| format!("{}{home}{}", &c[1], &c[2])).into_owned();
    TILDE_RE.replace_all(&once, |c: &regex::Captures| format!("{}{home}{}", &c[1], &c[2])).into_owned()
}

/// One word of a command as the shell hands it over: variables filled in, a leading `~` as the home
/// folder.
fn shell_word(w: &str, dir: &Path) -> String {
    let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_default();
    let w = fill_vars(w, dir, &home);
    match w.strip_prefix('~') {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("{home}{rest}"),
        _ => w,
    }
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
    // A pattern the shell can't read as a glob (a lone `[`) is taken as it's written: no match.
    regex::Regex::new(&re).map(|r| r.is_match(name)).unwrap_or(false)
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

/// A Bash command as the shell runs it: its simple commands, each a list of words with the quotes
/// gone (a quoted sentence is one word), and its heredoc bodies, which are data for the command they
/// feed, not paths. Command substitutions (`$(…)`, backquotes) and `sh -c '…'` are commands too.
#[derive(Default)]
struct Script {
    commands: Vec<Vec<String>>,
    bodies: Vec<String>,
}

static HEREDOC_RE: once_cell::sync::Lazy<regex::Regex> =
    once_cell::sync::Lazy::new(|| regex::Regex::new(r#"<<(-?)\s*(?:'([^']+)'|"([^"]+)"|\\?([A-Za-z_][A-Za-z0-9_]*))"#).unwrap());
static ASSIGN_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=").unwrap());

impl Script {
    fn parse(src: &str) -> Script {
        let mut s = Script::default();
        s.read(src);
        s
    }

    fn read(&mut self, src: &str) {
        let code = self.take_heredocs(src);
        let chars: Vec<char> = code.chars().collect();
        let (mut cmd, mut word) = (vec![], String::new());
        let (mut single, mut double) = (false, false);
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            i += 1;
            if single {
                if c == '\'' {
                    single = false;
                } else {
                    word.push(c);
                }
                continue;
            }
            match c {
                '\\' => {
                    if let Some(&n) = chars.get(i) {
                        if double && !matches!(n, '$' | '`' | '"' | '\\' | '\n') {
                            word.push('\\');
                        }
                        if n != '\n' {
                            word.push(n);
                        }
                        i += 1;
                    }
                }
                '\'' if !double => single = true,
                '"' => double = !double,
                '`' => {
                    let end = (i..chars.len()).find(|&j| chars[j] == '`' && chars[j - 1] != '\\').unwrap_or(chars.len());
                    self.read(&chars[i..end].iter().collect::<String>());
                    word.push_str("$__");
                    i = end + 1;
                }
                '$' if chars.get(i) == Some(&'(') => {
                    let (mut depth, mut quoted, mut j) = (1, false, i + 1);
                    while j < chars.len() {
                        match chars[j] {
                            '\'' => quoted = !quoted,
                            '(' if !quoted => depth += 1,
                            ')' if !quoted => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        j += 1;
                    }
                    self.read(&chars[i + 1..j.min(chars.len())].iter().collect::<String>());
                    word.push_str("$__");
                    i = j + 1;
                }
                c if !double && (c.is_whitespace() || ";|&()<>".contains(c)) => {
                    if !word.is_empty() {
                        cmd.push(std::mem::take(&mut word));
                    }
                    if (c == '\n' || ";|&()".contains(c)) && !cmd.is_empty() {
                        self.push(std::mem::take(&mut cmd));
                    }
                }
                c => word.push(c),
            }
        }
        if !word.is_empty() {
            cmd.push(word);
        }
        if !cmd.is_empty() {
            self.push(cmd);
        }
    }

    /// Keeps a command, and reads the scripts a shell is handed (`sh -c '…'`, `eval '…'`).
    fn push(&mut self, cmd: Vec<String>) {
        if let Some(("sh" | "bash" | "zsh" | "dash" | "ksh" | "eval", args)) = command_word(&cmd) {
            for a in args.iter().filter(|a| !a.starts_with('-')) {
                self.read(a);
            }
        }
        self.commands.push(cmd);
    }

    /// `src` without its heredoc bodies (the lines after `<<EOF` up to `EOF`), which go to `bodies`.
    fn take_heredocs(&mut self, src: &str) -> String {
        let mut code = String::new();
        let mut lines = src.split('\n');
        while let Some(line) = lines.next() {
            code.push_str(line);
            code.push('\n');
            // Not a here-string (`<<<`).
            let ends: Vec<(bool, String)> = HEREDOC_RE
                .captures_iter(line)
                .filter(|c| {
                    let at = c.get(0).unwrap();
                    !line[..at.start()].ends_with('<') && !line[at.start() + 2..].starts_with('<')
                })
                .map(|c| (&c[1] == "-", c.get(2).or(c.get(3)).or(c.get(4)).unwrap().as_str().to_string()))
                .collect();
            for (tabs, end) in ends {
                let mut body = String::new();
                for l in lines.by_ref() {
                    if (if tabs { l.trim_start_matches('\t') } else { l }) == end {
                        break;
                    }
                    body.push_str(l);
                    body.push('\n');
                }
                self.bodies.push(body);
            }
        }
        code
    }
}

/// A simple command's command word (its file name) and what follows it, past `X=1`, keywords (`if`,
/// `then`, `!`, …) and wrappers with their flags (`sudo`, `xargs -0`, `env`, …).
fn command_word(cmd: &[String]) -> Option<(&str, &[String])> {
    const KEYWORDS: &[&str] = &["if", "then", "do", "else", "elif", "while", "until", "!", "{", "}"];
    const WRAPPERS: &[&str] = &["sudo", "xargs", "exec", "command", "builtin", "env", "time", "nice", "nohup"];
    let mut wrapped = false;
    for (i, w) in cmd.iter().enumerate() {
        if KEYWORDS.contains(&w.as_str()) || ASSIGN_RE.is_match(w) || (wrapped && w.starts_with('-')) {
            continue;
        }
        let name = w.rsplit('/').next().unwrap_or(w);
        if WRAPPERS.contains(&name) {
            wrapped = true;
            continue;
        }
        return Some((name, &cmd[i + 1..]));
    }
    None
}

/// Whether a simple command walks the folders it's given: `find`, `rg`, `tar`, `grep -r`, `ls -R`, ….
fn walks(cmd: &[String]) -> bool {
    let Some((name, args)) = command_word(cmd) else { return false };
    let flag = |set: &str| {
        args.iter().any(|a| a == "--recursive" || a.strip_prefix('-').is_some_and(|f| !f.starts_with('-') && f.contains(|c| set.contains(c))))
    };
    match name {
        "find" | "rg" | "ag" | "ack" | "fd" | "tar" | "zip" | "rsync" | "du" | "tree" | "ditto" => true,
        "grep" | "egrep" | "fgrep" => flag("rR"),
        "ls" | "chmod" | "chown" => flag("R"),
        "cp" => flag("rRa"),
        _ => false,
    }
}

/// Whether a glob's segments past a folder (`pattern`) reach the token, whose path below that folder
/// is `target` (`[…, "taskboard", "<file>"]`): they match the file, the folder it's in (unless the
/// command only lists names), or, for a command that walks, any folder above it. `**` matches any
/// number of folders; a segment with a variable the hook can't fill in matches anything.
fn glob_reaches(pattern: &[&str], target: &[String], walks: bool, lists: bool) -> bool {
    match (pattern.first(), target.first()) {
        (None, _) => target.is_empty() || (target.len() == 1 && !lists) || walks,
        (Some(&"**"), _) => glob_reaches(&pattern[1..], target, walks, lists) || (!target.is_empty() && glob_reaches(pattern, &target[1..], walks, lists)),
        (Some(_), None) => false,
        (Some(p), Some(t)) => (p.contains('$') || glob_matches(p, t)) && glob_reaches(&pattern[1..], &target[1..], walks, lists),
    }
}

/// Whether a simple command's `words` reach the token in `real_dir` (resolved): a word that is the
/// folder itself (fine for a plain `ls`), a glob or variable that can match the token or its folder,
/// or, when the command walks, a folder above it.
fn words_reach(words: &[String], walks: bool, lists: bool, dir: &Path, real_dir: &Path, cwd: Option<&Path>) -> bool {
    let token = real_dir.join(FILE);
    for word in words {
        // `--path=/x` and `X=/x` hand over the part after `=`.
        let after = word.split_once('=').map(|(_, v)| v).filter(|_| word.starts_with('-') || ASSIGN_RE.is_match(word));
        for w in std::iter::once(word.as_str()).chain(after).map(|w| shell_word(w, dir)) {
            if !(w.starts_with('/') || w.starts_with('.') || w.contains('/')) {
                continue;
            }
            let segs: Vec<&str> = w.split('/').collect();
            let plain: Vec<&str> = segs.iter().take_while(|s| !has_glob(s) && !s.contains('$')).copied().collect();
            let pattern = &segs[plain.len()..];
            let base = match plain.as_slice() {
                [] => ".".to_string(),
                [""] => "/".to_string(),
                _ => plain.join("/"),
            };
            let Some(p) = resolve(&base, cwd) else { continue };
            if pattern.iter().all(|s| s.is_empty()) {
                if (p == real_dir && !lists) || (walks && real_dir.starts_with(&p)) {
                    return true;
                }
                continue;
            }
            if let Ok(below) = token.strip_prefix(&p) {
                let target: Vec<String> = below.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                let pattern: Vec<&str> = pattern.iter().copied().filter(|s| !s.is_empty()).collect();
                if glob_reaches(&pattern, &target, walks, lists) {
                    return true;
                }
            }
        }
    }
    false
}

/// Whether an agent's tool call (`tool`, `input`, run in `cwd`) reads the app token in data folder
/// `dir`. It does when it names the file (quoted in pieces or not, or by a glob aimed at its name),
/// or when a command's path arguments reach it: the folder itself (except to `ls` it), a glob or
/// variable that can match the token (`$D/*`), or a folder above it handed to a command that walks
/// folders (`find ~`, `grep -r . /`, Grep or Glob from `~`). Paths are spelled any way that resolves:
/// `/tmp` for `/private/tmp`, `./`, `..`, a symlink, `$HOME`, `$TASKBOARD_DATA`. Naming another file there
/// (`config.toml`) is fine.
///
/// Commands are read as a shell reads them, so text that isn't a path argument doesn't count: a
/// quoted sentence, a sed or regex pattern, a URL, a heredoc's body (unless it spells the folder),
/// a `grep -r` in another command on the same line.
///
/// A guard on what a call says can't be complete: a script can build the path at run time. It only
/// matters for a daemon without a team signature, which is the only one that writes the token
/// (`apporigin`).
pub fn tool_reaches(tool: &str, input: &serde_json::Value, dir: &Path, cwd: Option<&Path>) -> bool {
    fn strings(v: &serde_json::Value, skip: Option<&str>, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) => out.push(s.clone()),
            serde_json::Value::Array(a) => a.iter().for_each(|x| strings(x, None, out)),
            serde_json::Value::Object(o) => o.iter().filter(|(k, _)| Some(k.as_str()) != skip).for_each(|(_, x)| strings(x, None, out)),
            _ => {}
        }
    }
    let mut all = vec![];
    strings(input, None, &mut all);
    let raw = all.join("\n");
    let text = as_shell_reads(&raw, dir);
    if text.to_lowercase().contains(FILE) || raw.to_lowercase().contains(FILE) || text.contains("TASKBOARD_DATA") {
        return true;
    }
    // A glob for the token's name, anywhere (the name's first letters and a `*`): one that matches it
    // and none of the decoys. `*` alone is judged by its folder.
    let named = |s: &str| has_glob(s) && glob_matches(s, FILE) && !DECOYS.iter().any(|d| glob_matches(s, d));
    if text.split(|c: char| c.is_whitespace() || ";|&()<>=,:".contains(c)).any(|w| w.split('/').any(named)) {
        return true;
    }
    let real_dir = resolve(&dir.to_string_lossy(), None).unwrap_or_else(|| dir.to_path_buf());
    let script = match tool {
        "Bash" => Script::parse(input["command"].as_str().unwrap_or("")),
        // Grep's pattern is a regex, not a path.
        "Read" | "Grep" | "Glob" | "LS" => {
            let mut words = vec![];
            strings(input, (tool == "Grep").then_some("pattern"), &mut words);
            Script { commands: vec![words], bodies: vec![] }
        }
        _ => return false,
    };
    let mut at = cwd.map(Path::to_path_buf);
    for cmd in &script.commands {
        let (walks, lists) = match tool {
            "Bash" => (walks(cmd), command_word(cmd).is_some_and(|(n, _)| n == "ls") && !walks(cmd)),
            _ => (matches!(tool, "Grep" | "Glob"), tool == "LS"),
        };
        if words_reach(cmd, walks, lists, dir, &real_dir, at.as_deref()) {
            return true;
        }
        // A `cd` moves where the next commands' relative paths start (`cd x && grep -r . ../..`).
        // One the hook can't follow (`cd $X`, `cd -`) leaves it where it was.
        if let Some(("cd", args)) = command_word(cmd) {
            let to = match args.iter().find(|a| !a.starts_with('-') || a.as_str() == "-") {
                Some(a) => Some(shell_word(a, dir)).filter(|a| !a.contains('$') && !has_glob(a) && a != "-"),
                None => dirs::home_dir().map(|h| h.to_string_lossy().to_string()),
            };
            if let Some(p) = to.and_then(|t| resolve(&t, at.as_deref())) {
                at = Some(p);
            }
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
    // A heredoc feeding a script (`python3 - <<EOF`) that spells the folder as a whole, not one file
    // in it.
    for body in script.bodies.iter().map(|b| as_shell_reads(b, dir).to_lowercase()) {
        for d in spellings(dir).iter().map(|d| d.to_lowercase()) {
            for (at, _) in body.match_indices(&d) {
                let rest = &body[at + d.len()..];
                let one_file = rest.strip_prefix('/').is_some_and(|r| r.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'));
                if !one_file {
                    return true;
                }
            }
        }
    }
    false
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
        for no in ["cargo test", "tb start T8", "git status", "cat README.md", "ls ~/.config", "ls ~/.config/taskboard/", "cat ~/.config/taskboard/config.toml", "tail ~/.config/taskboard/server.log"] {
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
            yes.push(format!("grep -r . {t}"));
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

    /// A lone `/` in a tb command's prose read as the root folder, and `zip` or `tree` in the same
    /// prose read as a command walking it.
    #[test]
    fn a_tree_walking_word_in_prose_is_not_a_command() {
        let dir = std::path::PathBuf::from("/srv/board-data");
        let bash = |c: &str| tool_reaches("Bash", &serde_json::json!({"command": c}), &dir, None);
        for no in [
            "tb task new x --detail 'what happened / what was expected; attach the export zip (T113)'",
            "tb task new \"tree-walking words\" --detail \"a lone slash / between words\"",
            "git commit -m 'find / and grep -r / in prose'",
        ] {
            assert!(!bash(no), "{no}");
        }
        for yes in [
            "find / -name x",
            "/usr/bin/find / -type f",
            "sudo find / -type f",
            "echo x | xargs -0 grep -r tok /",
            "sh -c 'tar c /'",
            "X=1 rg . /",
            "echo $(find / -type f)",
            "echo `find / -type f`",
            "cd /tmp && ls -R /",
            "if find / -type f; then :; fi",
        ] {
            assert!(bash(yes), "{yes}");
        }
    }

    /// Commands agents were refused for although they reach nowhere near the token: a `/` or a glob
    /// in a sed pattern, a URL or a heredoc read as a path from the root, a `grep -r` elsewhere on the
    /// line, another file in the folder named next to the word "token".
    #[test]
    fn commands_are_read_as_a_shell_reads_them() {
        let home = dirs::home_dir().unwrap();
        let dir = home.join(".config/taskboard");
        let d = dir.display().to_string();
        let bash = |c: &str| tool_reaches("Bash", &serde_json::json!({"command": c}), &dir, Some(&home));
        for no in [
            "T=$(sed -n '/^{/,$p' tok.json | jq -r .token); curl -s -X POST localhost:8090/db -H \"Authorization: Bearer $T\" -d '{\"table\":\"households\"}'".to_string(),
            "l=$(curl -sI http://x.com | grep -i '^location:'); case \"$l\" in *https://x.com*) echo live;; esac".into(),
            "grep -rn 'a\\|b' --include='*.rs' crates; sed -i '' 's/screen: false,/screen: true,/' src/config.rs".into(),
            "grep -rn x src; python3 - <<'EOF'\ns = s.replace('\"/tasks/{t}/pr\"', '\"/\"')\nEOF".into(),
            "grep -rln x --include='*.rs' crates/*app*".into(),
            "cd /tmp/a/b && grep -rn x . ../..".into(),
            format!("sqlite3 -readonly {d}/tasks.db \"select token_count from usage\""),
            format!("grep -n '^\\[questions\\]' {d}/config.toml"),
            format!("ls -la {d}/"),
            "git commit -m 'find / and grep -r ~ in prose'".into(),
        ] {
            assert!(!bash(&no), "{no}");
        }
        for yes in [
            "cd ~ && grep -r . .config".to_string(),
            format!("D={d}; cat $D/*"),
            "echo \"$(find / -type f)\"".into(),
            "cat ~/.config/*/*".into(),
            format!("cd {d} && cat x"),
            format!("ls -R {d}"),
            format!("python3 - <<'EOF'\nimport os; print(os.listdir('{d}'))\nEOF"),
        ] {
            assert!(bash(&yes), "{yes}");
        }
    }
}
