//! The app's token: what makes a request the owner's own click in Taskboard.app. The daemon makes a
//! fresh random token at each launch and writes it to [`FILE`] in a folder of its own ([`DIR`]) in the
//! data folder, readable by this user only; the app reads it and sends it with each request
//! (`X-Task-Board-Token`), and the server takes `X-Task-Board-From: app` only with the matching token.
//! A request that just says it's the app (curl with the header) is refused.
//!
//! Only a daemon with no team signature (`cargo run`, `scripts/dev-app.sh`, tests) writes or takes the
//! token; a signed one checks the app's code signature on the app socket instead (`apporigin`).
//! Agents run as the same user, so they could read the file; `tb hook PreToolUse` refuses tool calls
//! that reach into its folder, however they spell it, but a guard on what a call says can't be complete.

use std::io::Write;
use std::path::{Path, PathBuf};

use rand::RngCore;

use crate::config::Config;

/// The token's folder in the data folder: the token's alone, so the rest of the data folder is anyone's
/// to read and search. Hidden, so a shell's `*` and `rg` pass it by.
pub const DIR: &str = ".app-only";
/// The token's file in [`DIR`].
pub const FILE: &str = "app-token";
/// The header the app sends it in.
pub const HEADER: &str = "x-task-board-token";

pub fn path(cfg: &Config) -> PathBuf {
    cfg.data.join(DIR).join(FILE)
}

/// Where the token was before it had a folder of its own, removed at launch.
pub fn old_path(cfg: &Config) -> PathBuf {
    cfg.data.join(FILE)
}

/// A new token: 32 random bytes, in hex.
pub fn fresh() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Writes `token` to `path`, readable and writable by this user only (0600, its folder 0700), replacing
/// what was there.
pub fn write(path: &Path, token: &str) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
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

/// Whether glob `pat` (`*`, `?`, `[…]`, `{a,b}`) matches `name`, ignoring case. As in the shell, a
/// leading `*`, `?` or `[…]` doesn't match a dot file's dot.
fn glob_matches(pat: &str, name: &str) -> bool {
    if name.starts_with('.') && pat.starts_with(['*', '?', '[']) {
        return false;
    }
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
/// gone (a quoted sentence is one word). Heredoc bodies are left out: they're data for the command
/// they feed, not paths. Command substitutions (`$(…)`, backquotes) and `sh -c '…'` are commands too.
#[derive(Default)]
struct Script {
    commands: Vec<Vec<String>>,
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

    /// `src` without its heredoc bodies (the lines after `<<EOF` up to `EOF`).
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
                for l in lines.by_ref() {
                    if (if tabs { l.trim_start_matches('\t') } else { l }) == end {
                        break;
                    }
                }
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

/// Whether glob `pattern`'s segments past a folder reach the token, whose path below that folder is
/// `target` (`[…, DIR, FILE]`): a segment matches the token's folder, or they end at the token. `**`
/// matches any number of folders; a segment with a variable the hook can't fill in matches anything.
fn glob_reaches(pattern: &[&str], target: &[String]) -> bool {
    match (pattern.first(), target.first()) {
        (None, _) => target.len() <= 1,
        (Some(&"**"), _) => glob_reaches(&pattern[1..], target) || (!target.is_empty() && glob_reaches(pattern, &target[1..])),
        (Some(_), None) => false,
        (Some(p), Some(t)) => (p.contains('$') || glob_matches(p, t)) && (target.len() == 2 || glob_reaches(&pattern[1..], &target[1..])),
    }
}

/// Whether a simple command's `words` reach the token's folder `private` (resolved): a path in it
/// (through a symlink too), or a glob or variable that can match it or the token.
fn words_reach(words: &[String], dir: &Path, private: &Path, cwd: Option<&Path>) -> bool {
    let token = private.join(FILE);
    for word in words {
        // `--path=/x` and `X=/x` hand over the part after `=`.
        let after = word.split_once('=').map(|(_, v)| v).filter(|_| word.starts_with('-') || ASSIGN_RE.is_match(word));
        for w in std::iter::once(word.as_str()).chain(after).map(|w| shell_word(w, dir)) {
            let segs: Vec<&str> = w.split('/').collect();
            let plain: Vec<&str> = segs.iter().take_while(|s| !has_glob(s) && !s.contains('$')).copied().collect();
            let base = match plain.as_slice() {
                [] => ".".to_string(),
                [""] => "/".to_string(),
                _ => plain.join("/"),
            };
            let Some(p) = resolve(&base, cwd) else { continue };
            if p.starts_with(private) {
                return true;
            }
            let pattern: Vec<&str> = segs[plain.len()..].iter().copied().filter(|s| !s.is_empty()).collect();
            if let Ok(below) = token.strip_prefix(&p) {
                let target: Vec<String> = below.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                if !pattern.is_empty() && glob_reaches(&pattern, &target) {
                    return true;
                }
            }
        }
    }
    false
}

/// Whether `text` names the token's folder: [`DIR`] as a whole path segment, however it's quoted.
fn names_dir(text: &str, dir: &Path) -> bool {
    let text = as_shell_reads(text, dir).to_lowercase();
    let part = |c: char| c.is_alphanumeric() || "_-.".contains(c);
    text.match_indices(DIR).any(|(at, _)| !text[..at].chars().next_back().is_some_and(part) && !text[at + DIR.len()..].chars().next().is_some_and(part))
}

/// Whether an agent's tool call (`tool`, `input`, run in `cwd`) reaches the token's folder ([`DIR`] in
/// data folder `dir`). It does when it names the folder (quoted in pieces or not), or when a path
/// argument lands in it: the folder or a file in it however it's spelled (`/tmp` for `/private/tmp`,
/// `./`, `..`, a symlink, `$HOME`, the data folder's variable), a glob that can match the folder or the
/// token (`.*/*`), or any path from a working folder inside it.
///
/// The rest of the data folder is the agent's to read, and so is a search of it: `config.toml`,
/// `find ~ -name config.toml`, `grep -r x` over the data folder. A shell's `*` doesn't match the
/// folder's leading dot, and `rg` and Grep skip it unless asked for hidden files; `grep -r` and `find`
/// do walk into it.
///
/// Commands are read as a shell reads them, so text that isn't a path argument doesn't count: a
/// quoted sentence, a sed or regex pattern, a URL, a heredoc's body (unless it names the folder).
///
/// A guard on what a call says can't be complete: a script can build the path at run time. It only
/// matters for a daemon without a team signature, which is the only one that writes the token
/// (`apporigin`).
pub fn tool_reaches(tool: &str, input: &serde_json::Value, dir: &Path, cwd: Option<&Path>) -> bool {
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
    if all.iter().any(|s| names_dir(s, dir)) {
        return true;
    }
    let commands = match tool {
        "Bash" => Script::parse(input["command"].as_str().unwrap_or("")).commands,
        "Read" | "Grep" | "Glob" | "LS" => {
            // The path read or searched (Glob's pattern below it); Grep and Glob with none search the
            // working folder.
            let path = ["file_path", "path"].iter().find_map(|k| input[*k].as_str()).unwrap_or("");
            let word = match input["pattern"].as_str().filter(|_| tool == "Glob") {
                Some(p) if p.starts_with('/') || path.is_empty() => p.to_string(),
                Some(p) => format!("{path}/{p}"),
                None if path.is_empty() => ".".to_string(),
                None => path.to_string(),
            };
            vec![vec![word]]
        }
        _ => return false,
    };
    let private = resolve(&dir.join(DIR).to_string_lossy(), None).unwrap_or_else(|| dir.join(DIR));
    // Resolved like `private` (lowercase, symlinks followed), so the two compare.
    let mut at = cwd.and_then(|c| resolve(&c.to_string_lossy(), None));
    for cmd in &commands {
        if words_reach(cmd, dir, &private, at.as_deref()) {
            return true;
        }
        // A `cd` moves where the next commands' relative paths start (`cd x && cat ../y`). One the hook
        // can't follow (`cd $X`, `cd -`) leaves it where it was.
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
    use serde_json::json;

    /// The data folder's variable, spelled so this file doesn't name it.
    fn data_var() -> String {
        ["$TASKBOARD", "DATA"].join("_")
    }

    #[test]
    fn a_tool_call_into_the_tokens_folder_is_caught() {
        let home = dirs::home_dir().unwrap();
        let dir = home.join(".config/taskboard");
        let d = dir.display().to_string();
        let var = data_var();
        let (head, tail) = DIR.split_at(3);
        let bash = |c: &str| tool_reaches("Bash", &json!({"command": c}), &dir, Some(&home));
        for yes in [
            format!("cat ~/.config/taskboard/{DIR}/{FILE}"),
            format!("cat $HOME/.config/taskboard/{DIR}/{FILE}"),
            format!("cat {var}/{DIR}/{FILE}"),
            format!("cat {var}/.*/*"),
            format!("cat ~/.config/taskboard/{}/*", DIR.to_uppercase()),
            format!("cat ~/.config/taskboard/'{head}'\"{tail}\"/*"),
            format!("cat ~/.config/taskboard/{head}\\{tail}/*"),
            format!("ls ~/.config/taskboard/{DIR}"),
            format!("grep -r . ~/.config/taskboard/{DIR}"),
            format!("cd ~/.config/taskboard && cat {DIR}/*"),
            format!("cat ~/.config/taskboard/{head}*/*"),
            "cat ~/.config/taskboard/.*/*".into(),
            "cat ~/.config/taskboard/**/*".into(),
            "cd ~/.config/taskboard && cat .*/*".into(),
            format!("python3 -c 'print(open(\"{d}/{DIR}/x\").read())'"),
            format!("python3 - <<'EOF'\nimport os; print(os.listdir('{d}/{DIR}'))\nEOF"),
        ] {
            assert!(bash(&yes), "{yes}");
        }
        let private = dir.join(DIR).display().to_string();
        for (tool, input) in [
            ("Read", json!({"file_path": format!("{private}/{FILE}")})),
            ("Glob", json!({"pattern": "*", "path": private})),
            ("Glob", json!({"pattern": "**/*", "path": d})),
            ("Grep", json!({"pattern": ".", "path": private})),
            ("Write", json!({"file_path": "/repo/x.sh", "content": format!("#!/bin/sh\ncat {d}/{DIR}/*\n")})),
        ] {
            assert!(tool_reaches(tool, &input, &dir, Some(&home)), "{tool} {input}");
        }

        // Everything else in the data folder, and searches for it.
        for no in [
            "cat ~/.config/taskboard/config.toml".to_string(),
            format!("cat {var}/config.toml"),
            "ls ~/.config/taskboard".into(),
            "ls -la ~/.config/taskboard/".into(),
            "find ~ -name config.toml -path '*taskboard*'".into(),
            "find ~/.config -name config.toml".into(),
            "find ~/.config/taskboard -name '*.toml'".into(),
            "grep -r lens ~/.config/taskboard".into(),
            "grep -rn lens ~/.config".into(),
            "cat ~/.config/taskboard/*".into(),
            "cat ~/.config/taskboard/*/*".into(),
            format!("sqlite3 -readonly {d}/tasks.db 'select 1'"),
            format!("cat {FILE}"),
            "git commit -m 'keep the app token out of reach'".into(),
            "tb task new x --detail 'GET tasks/{id}, and a lone / in prose'".into(),
            "T=$(sed -n '/^{/,$p' tok.json | jq -r .token); echo $T".into(),
            "grep -rn x src; python3 - <<'EOF'\ns = s.replace('\"/tasks/{t}/pr\"', '\"/\"')\nEOF".into(),
            "cargo test -p taskboardd".into(),
        ] {
            assert!(!bash(&no), "{no}");
        }
        for (tool, input) in [
            ("Read", json!({"file_path": format!("{d}/config.toml")})),
            ("Glob", json!({"pattern": "**/config.toml", "path": home.join(".config").display().to_string()})),
            ("Glob", json!({"pattern": "*.toml", "path": d})),
            ("Grep", json!({"pattern": "lens", "path": d})),
            ("Grep", json!({"pattern": "fn main"})),
            ("Edit", json!({"file_path": "/repo/SKILL.md", "old_string": format!("never read {FILE}"), "new_string": "x"})),
        ] {
            assert!(!tool_reaches(tool, &input, &dir, Some(&home)), "{tool} {input}");
        }
    }

    #[test]
    fn symlinks_tmp_and_a_working_folder_inside_are_followed() {
        let tmp = tempfile::tempdir().unwrap();
        // Under /tmp, as `/private/tmp` (what it resolves to) or `/tmp`.
        let real = std::fs::canonicalize(tmp.path()).unwrap();
        let dir = real.join("board");
        let private = dir.join(DIR);
        std::fs::create_dir_all(&private).unwrap();
        let link = real.join("link");
        std::os::unix::fs::symlink(&private, &link).unwrap();
        let (d, l, r) = (dir.display(), link.display(), real.display());
        let bash = |c: &str, cwd: &Path| tool_reaches("Bash", &json!({"command": c}), &dir, Some(cwd));
        for yes in [format!("cat {l}/*"), format!("cat {l}/{FILE}"), "cd link && cat x".into(), format!("cat {d}/.[a]{}/x", &DIR[2..]), format!("cat board/.?{}/*", &DIR[2..])] {
            assert!(bash(&yes, &real), "{yes}");
        }
        if let Some(t) = d.to_string().strip_prefix("/private") {
            assert!(bash(&format!("cat {t}/.*/*"), &real));
        }
        assert!(tool_reaches("Read", &json!({"file_path": format!("{l}/{FILE}")}), &dir, None));
        // From a working folder inside it, any path is in it.
        assert!(bash("cat x", &private));
        assert!(bash("grep -r x", &private));
        assert!(tool_reaches("Grep", &json!({"pattern": "x"}), &dir, Some(&private)));

        for no in [format!("cat {d}/*"), format!("grep -r . {r}"), format!("find {r} -type f"), "cat board/config.toml".into(), "cat x".into()] {
            assert!(!bash(&no, &real), "{no}");
        }
    }

    #[test]
    fn a_custom_data_folder_is_watched_too() {
        let dir = std::path::PathBuf::from("/srv/board-data");
        let bash = |c: &str| tool_reaches("Bash", &json!({"command": c}), &dir, None);
        assert!(bash("cat /srv/board-data/.*/*"));
        assert!(bash(&format!("cat /srv/board-data/{DIR}/{FILE}")));
        assert!(!bash("cat /srv/board-data/*"));
        assert!(!bash("cat /srv/other/x"));
    }

    #[test]
    fn globs_match_like_the_shell() {
        for (pat, name) in [("ab-*", "ab-cd"), ("a?-c[d]", "ab-cd"), ("ab-{x,cd}", "ab-cd"), ("[!x]b-*", "ab-cd"), ("*", "x"), (".*", ".x"), (".[a]b", ".ab")] {
            assert!(glob_matches(pat, name), "{pat} {name}");
        }
        // A leading `*`, `?` or `[…]` doesn't match a dot file.
        for (pat, name) in [("ab-*", "cd"), ("*.rs", "ab-cd"), ("[x]b-*", "ab-cd"), ("ab-{x,y}", "ab-cd"), ("*", ".x"), ("?x", ".x"), ("[.]x", ".x")] {
            assert!(!glob_matches(pat, name), "{pat} {name}");
        }
    }

    #[test]
    fn a_token_is_fresh_each_time_and_private() {
        let (a, b) = (fresh(), fresh());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(DIR).join(FILE);
        write(&p, &a).unwrap();
        write(&p, &b).unwrap();
        assert_eq!(read(&p).as_deref(), Some(b.as_str()));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(p.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
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
