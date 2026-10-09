//! The comment guard (`[comments]` in config.toml): with it on, agents may not add code comments.
//! A pragma the tools read (`# noqa`, `// eslint-disable-next-line`, …) is allowed. The `PreToolUse`
//! hook refuses an Edit, Write, MultiEdit or NotebookEdit that adds one, and the board refuses the
//! PR, a review round and `tb done` while the branch adds any.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::config::{CommentsConfig, Config};

const GIT_SECS: f64 = 10.0;
const SHOWN: usize = 3;

/// How a language writes comments and strings.
struct Syntax {
    line: &'static [&'static str],
    block: &'static [(&'static str, &'static str)],
    quotes: &'static [char],
    /// `"""` and `'''` strings.
    triple: bool,
    /// `'x'` is a character, and a lone `'` (a Rust lifetime) is nothing.
    char_lit: bool,
    /// Rust's `r"…"` and `r#"…"#`.
    raw: bool,
    /// A line comment starts only at the start of a word (shell's `$#` isn't one).
    word_start: bool,
}

const C_BLOCK: &[(&str, &str)] = &[("/*", "*/")];
const SLASHES: &[&str] = &["//"];
const HASH: &[&str] = &["#"];
const DASHES: &[&str] = &["--"];

fn syntax(lang: &str) -> Option<Syntax> {
    let s = |line, block, quotes, triple, char_lit| Syntax { line, block, quotes, triple, char_lit, raw: false, word_start: false };
    Some(match lang {
        "rust" => Syntax { raw: true, ..s(SLASHES, C_BLOCK, &['"'], false, true) },
        "dart" => s(SLASHES, C_BLOCK, &['"', '\''], true, false),
        "typescript" | "javascript" => s(SLASHES, C_BLOCK, &['"', '\'', '`'], false, false),
        "swift" => s(SLASHES, C_BLOCK, &['"'], true, false),
        "kotlin" | "java" | "scala" => s(SLASHES, C_BLOCK, &['"'], true, true),
        "go" => s(SLASHES, C_BLOCK, &['"', '`'], false, true),
        "c" | "cpp" | "csharp" | "objc" => s(SLASHES, C_BLOCK, &['"'], false, true),
        "php" => s(&["//", "#"], C_BLOCK, &['"', '\''], false, false),
        "scss" | "less" => s(SLASHES, C_BLOCK, &['"', '\''], false, false),
        "css" => s(&[], C_BLOCK, &['"', '\''], false, false),
        "python" => s(HASH, &[], &['"', '\''], true, false),
        "ruby" | "yaml" | "toml" | "elixir" | "r" => s(HASH, &[], &['"', '\''], false, false),
        "shell" => Syntax { word_start: true, ..s(HASH, &[], &['"', '\''], false, false) },
        "sql" | "lua" | "haskell" => s(DASHES, if lang == "lua" { &[("--[[", "]]")] } else { C_BLOCK }, &['"', '\''], false, false),
        "html" | "xml" | "vue" | "svelte" => s(&[], &[("<!--", "-->")], &[], false, false),
        _ => return None,
    })
}

/// The language a file is written in, by its extension.
pub fn language_of(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    Some(match ext {
        "rs" => "rust",
        "dart" => "dart",
        "ts" | "tsx" | "mts" | "cts" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "java" => "java",
        "scala" => "scala",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "m" | "mm" => "objc",
        "cs" => "csharp",
        "php" => "php",
        "css" => "css",
        "scss" => "scss",
        "less" => "less",
        "py" | "pyi" => "python",
        "rb" => "ruby",
        "sh" | "bash" | "zsh" => "shell",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "ex" | "exs" => "elixir",
        "r" => "r",
        "sql" => "sql",
        "lua" => "lua",
        "hs" => "haskell",
        "html" | "htm" => "html",
        "xml" => "xml",
        "vue" => "vue",
        "svelte" => "svelte",
        _ => return None,
    })
}

fn at(chars: &[char], i: usize, s: &str) -> bool {
    s.chars().enumerate().all(|(k, c)| chars.get(i + k) == Some(&c))
}

/// The comments in `text`: (line, from 1; what it says, without its markers).
pub fn comments_in(text: &str, lang: &str) -> Vec<(usize, String)> {
    let Some(sx) = syntax(lang) else { return vec![] };
    let mut out = vec![];
    // In a string: what closes it, and whether `\` escapes.
    let mut string: Option<(String, bool)> = None;
    // In a block comment: what closes it, where it started, what it says so far.
    let mut block: Option<(&str, usize, String)> = None;
    for (n, line) in text.lines().enumerate() {
        let c: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < c.len() {
            if let Some((close, from, buf)) = block.as_mut() {
                if at(&c, i, close) {
                    i += close.chars().count();
                    out.push((*from, buf.trim().to_string()));
                    block = None;
                } else {
                    buf.push(c[i]);
                    i += 1;
                }
                continue;
            }
            if let Some((close, escapes)) = &string {
                if *escapes && c[i] == '\\' {
                    i += 2;
                } else if at(&c, i, close) {
                    i += close.chars().count();
                    string = None;
                } else {
                    i += 1;
                }
                continue;
            }
            if let Some((open, close)) = sx.block.iter().find(|(o, _)| at(&c, i, o)) {
                block = Some((close, n + 1, String::new()));
                i += open.chars().count();
                continue;
            }
            if let Some(tok) = sx.line.iter().find(|t| at(&c, i, t)) {
                if !sx.word_start || i == 0 || c[i - 1].is_whitespace() || matches!(c[i - 1], ';' | '&' | '|' | '(' | ')') {
                    out.push((n + 1, c[i + tok.chars().count()..].iter().collect::<String>().trim().to_string()));
                    break;
                }
                i += 1;
                continue;
            }
            if sx.raw && c[i] == 'r' && (i == 0 || !(c[i - 1].is_alphanumeric() || c[i - 1] == '_')) {
                let hashes = c[i + 1..].iter().take_while(|x| **x == '#').count();
                if c.get(i + 1 + hashes) == Some(&'"') {
                    string = Some((format!("\"{}", "#".repeat(hashes)), false));
                    i += 2 + hashes;
                    continue;
                }
            }
            if sx.triple {
                if let Some(q) = ["\"\"\"", "'''"].iter().find(|q| at(&c, i, q) && sx.quotes.contains(&q.chars().next().unwrap())) {
                    string = Some((q.to_string(), true));
                    i += 3;
                    continue;
                }
            }
            if sx.quotes.contains(&c[i]) {
                string = Some((c[i].to_string(), c[i] != '`' || lang != "go"));
                i += 1;
                continue;
            }
            if sx.char_lit && c[i] == '\'' {
                if c.get(i + 1) == Some(&'\\') {
                    let end = c[i + 2..].iter().position(|x| *x == '\'').map(|p| i + 3 + p);
                    i = end.unwrap_or(i + 1);
                } else if c.get(i + 2) == Some(&'\'') {
                    i += 3;
                } else {
                    i += 1;
                }
                continue;
            }
            i += 1;
        }
        if let Some((_, _, buf)) = block.as_mut() {
            buf.push('\n');
        }
    }
    if let Some((_, from, buf)) = block {
        out.push((from, buf.trim().to_string()));
    }
    out
}

pub fn is_pragma(text: &str, pragmas: &[String]) -> bool {
    let t = text.trim();
    pragmas.iter().map(|p| p.trim()).filter(|p| !p.is_empty()).any(|p| t.starts_with(p))
}

/// The comments `new` has that `old` didn't (by what they say), pragmas and empty ones aside.
pub fn added(old: &str, new: &str, lang: &str, pragmas: &[String]) -> Vec<(usize, String)> {
    let mut had: HashMap<String, usize> = HashMap::new();
    for (_, t) in comments_in(old, lang) {
        *had.entry(t).or_default() += 1;
    }
    let mut out = vec![];
    for (n, t) in comments_in(new, lang) {
        if t.is_empty() || is_pragma(&t, pragmas) {
            continue;
        }
        match had.get_mut(&t) {
            Some(k) if *k > 0 => *k -= 1,
            _ => out.push((n, t)),
        }
    }
    out
}

/// The guard watches this file: it's on, and the file's language is one of its languages.
pub fn watched(c: &CommentsConfig, path: &str) -> Option<&'static str> {
    let lang = language_of(path)?;
    (c.guard && c.languages.iter().any(|l| l.trim().eq_ignore_ascii_case(lang))).then_some(lang)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub file: String,
    pub line: usize,
    pub text: String,
}

fn shown(found: &[Found]) -> String {
    let mut s: Vec<String> = found.iter().take(SHOWN).map(|f| format!("{}:{} “{}”", f.file, f.line, crate::util::one_line(&f.text, 80))).collect();
    if found.len() > SHOWN {
        s.push(format!("and {} more", found.len() - SHOWN));
    }
    s.join("; ")
}

fn pragma_examples(c: &CommentsConfig) -> String {
    c.pragmas.iter().map(|p| p.trim()).filter(|p| p.len() > 2).take(4).collect::<Vec<_>>().join(", ")
}

/// The rule, for the handoff and the skill.
pub fn rule_line(cfg: &Config) -> Option<String> {
    let c = &cfg.comments;
    c.guard.then(|| {
        format!(
            "No code comments: {} wants none added (in {}). Make the code say it with names and small functions, and put the why in the commit \
             or PR. Pragmas the tools read ({}…) are fine. The board refuses an edit that adds a comment, and the PR, a review round and tb done \
             while the branch adds any.",
            cfg.owner.clone(),
            c.languages.join(", "),
            pragma_examples(c)
        )
    })
}

fn notebook_cell(path: &str, cell: Option<&str>) -> (String, &'static str) {
    let nb: Value = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    let lang = nb["metadata"]["language_info"]["name"].as_str().or(nb["metadata"]["kernelspec"]["language"].as_str()).unwrap_or("python");
    let lang = language_of(&format!("x.{}", match lang { "python" => "py", "javascript" => "js", "typescript" => "ts", "r" | "R" => "r", other => other })).unwrap_or("python");
    let old = cell
        .and_then(|id| nb["cells"].as_array()?.iter().find(|c| c["id"].as_str() == Some(id)).cloned())
        .map(|c| match &c["source"] {
            Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect::<String>(),
            Value::String(s) => s.clone(),
            _ => String::new(),
        })
        .unwrap_or_default();
    (old, lang)
}

/// The comments an edit tool's call would add, with the files' current text read from disk.
pub fn edit_adds(c: &CommentsConfig, tool: &str, input: &Value) -> Vec<Found> {
    if !c.guard {
        return vec![];
    }
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let path = s(&input["file_path"]);
    let mut pairs: Vec<(String, String, &'static str)> = vec![];
    match tool {
        "Edit" | "MultiEdit" => {
            let Some(lang) = watched(c, &path) else { return vec![] };
            if tool == "Edit" {
                pairs.push((s(&input["old_string"]), s(&input["new_string"]), lang));
            } else {
                for e in input["edits"].as_array().into_iter().flatten() {
                    pairs.push((s(&e["old_string"]), s(&e["new_string"]), lang));
                }
            }
        }
        "Write" => {
            let Some(lang) = watched(c, &path) else { return vec![] };
            pairs.push((std::fs::read_to_string(&path).unwrap_or_default(), s(&input["content"]), lang));
        }
        "NotebookEdit" => {
            let nb = s(&input["notebook_path"]);
            if !c.guard || input["cell_type"] == "markdown" || input["edit_mode"] == "delete" {
                return vec![];
            }
            let (old, lang) = notebook_cell(&nb, if input["edit_mode"] == "insert" { None } else { input["cell_id"].as_str() });
            if !c.languages.iter().any(|l| l.trim().eq_ignore_ascii_case(lang)) {
                return vec![];
            }
            pairs.push((old, s(&input["new_source"]), lang));
            return pairs.iter().flat_map(|(o, n, l)| added(o, n, l, &c.pragmas)).map(|(line, text)| Found { file: nb.clone(), line, text }).collect();
        }
        _ => return vec![],
    }
    pairs.iter().flat_map(|(o, n, l)| added(o, n, l, &c.pragmas)).map(|(line, text)| Found { file: path.clone(), line, text }).collect()
}

/// What the `PreToolUse` hook says when an edit adds a comment.
pub fn edit_refusal(cfg: &Config, tool: &str, input: &Value) -> Option<String> {
    let found = edit_adds(&cfg.comments, tool, input);
    if found.is_empty() {
        return None;
    }
    let owner = cfg.owner.clone();
    Some(format!(
        "Task board: this edit adds a code comment ({}), and {owner} wants no comments in code. Take it out and make the code say it \
         (names, small functions); the why goes in the commit or PR. Pragmas the tools read ({}…) are fine.",
        shown(&found),
        pragma_examples(&cfg.comments)
    ))
}

/// Why a task's `what` can't go ahead: its branch (in `cwd`, else its worktree or project folder)
/// adds comments. None with the guard off.
pub fn task_refusal(app: &crate::app::App, t: &crate::util::Row, cwd: Option<&str>, what: &str) -> crate::util::Result<Option<String>> {
    if !app.cfg.comments.guard {
        return Ok(None);
    }
    let ctx = crate::board::task_context(t);
    let wt = ctx.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str()).filter(|w| Path::new(w).is_dir()).map(|s| s.to_string());
    let dir = match cwd.filter(|c| !c.is_empty()).map(|c| c.to_string()).or(wt) {
        Some(d) => Some(d),
        None => crate::runner::task_cwd(app, t)?,
    };
    Ok(dir.and_then(|d| branch_refusal(&app.cfg, &d, what)))
}

struct Git(Option<std::path::PathBuf>);

impl Git {
    fn run(&self, dir: &str, args: &[&str]) -> Option<String> {
        let bin = self.0.as_ref()?;
        let mut all: Vec<String> = vec!["-C".into(), dir.into()];
        all.extend(args.iter().map(|a| a.to_string()));
        crate::proc::run(bin, &all, None, GIT_SECS).ok().filter(|o| o.code == Some(0)).map(|o| o.stdout)
    }
}

/// Where the branch at `dir` forked from its default branch.
fn fork_point(git: &Git, dir: &str) -> Option<String> {
    let mut bases = vec![];
    if let Some(h) = git.run(dir, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]) {
        bases.push(h.trim().to_string());
    }
    bases.extend(["origin/main", "origin/master", "main", "master"].iter().map(|s| s.to_string()));
    bases.into_iter().filter(|b| !b.is_empty()).find_map(|b| git.run(dir, &["merge-base", "HEAD", &b]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
}

/// The comments a unified diff (`-U0`) adds, by file.
pub fn diff_adds(c: &CommentsConfig, diff: &str) -> Vec<Found> {
    struct Hunk {
        file: String,
        lang: &'static str,
        removed: String,
        added: String,
        lines: Vec<usize>,
    }
    let mut hunks: Vec<Hunk> = vec![];
    let mut file: Option<(String, &'static str)> = None;
    let mut next = 0usize;
    for l in diff.lines() {
        if let Some(p) = l.strip_prefix("+++ ") {
            let p = p.strip_prefix("b/").unwrap_or(p);
            file = if p == "/dev/null" { None } else { watched(c, p).map(|lang| (p.to_string(), lang)) };
        } else if l.starts_with("--- ") || l.starts_with("diff --git ") {
            if l.starts_with("diff --git ") {
                file = None;
            }
        } else if let Some(h) = l.strip_prefix("@@ ") {
            let plus = h.split_whitespace().find(|x| x.starts_with('+')).unwrap_or("+0");
            next = plus.trim_start_matches('+').split(',').next().and_then(|n| n.parse().ok()).unwrap_or(0);
            if let Some((f, lang)) = &file {
                hunks.push(Hunk { file: f.clone(), lang, removed: String::new(), added: String::new(), lines: vec![] });
            }
        } else if file.is_some() {
            let Some(h) = hunks.last_mut() else { continue };
            if let Some(a) = l.strip_prefix('+') {
                h.added.push_str(a);
                h.added.push('\n');
                h.lines.push(next);
                next += 1;
            } else if let Some(r) = l.strip_prefix('-') {
                h.removed.push_str(r);
                h.removed.push('\n');
            }
        }
    }
    let mut had: HashMap<(String, String), usize> = HashMap::new();
    for h in &hunks {
        for (_, t) in comments_in(&h.removed, h.lang) {
            *had.entry((h.file.clone(), t)).or_default() += 1;
        }
    }
    let mut out = vec![];
    for h in &hunks {
        for (n, t) in comments_in(&h.added, h.lang) {
            if t.is_empty() || is_pragma(&t, &c.pragmas) {
                continue;
            }
            match had.get_mut(&(h.file.clone(), t.clone())) {
                Some(k) if *k > 0 => *k -= 1,
                _ => out.push(Found { file: h.file.clone(), line: h.lines.get(n - 1).copied().unwrap_or(0), text: t }),
            }
        }
    }
    out
}

/// The comments the branch at `dir` adds over its default branch, committed or not.
pub fn branch_adds(c: &CommentsConfig, dir: &str) -> Vec<Found> {
    if !c.guard || dir.is_empty() || !Path::new(dir).is_dir() {
        return vec![];
    }
    let git = Git(crate::proc::which("git"));
    let Some(base) = fork_point(&git, dir) else { return vec![] };
    let diff = git.run(dir, &["diff", "--no-color", "--no-ext-diff", "-U0", "--diff-filter=AMR", &base]).unwrap_or_default();
    diff_adds(c, &diff)
}

/// Why `what` ("opening the PR", "finishing") can't go ahead: the branch adds comments.
pub fn branch_refusal(cfg: &Config, dir: &str, what: &str) -> Option<String> {
    let found = branch_adds(&cfg.comments, dir);
    if found.is_empty() {
        return None;
    }
    let owner = cfg.owner.clone();
    Some(format!(
        "Task board: not {what} yet: this branch adds {} ({}), and {owner} wants no comments in code. Take them out, commit, and try again. \
         Pragmas the tools read are fine.",
        if found.len() == 1 { "a code comment".to_string() } else { format!("{} code comments", found.len()) },
        shown(&found)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> CommentsConfig {
        CommentsConfig { guard: true, ..CommentsConfig::default() }
    }

    fn texts(v: Vec<(usize, String)>) -> Vec<String> {
        v.into_iter().map(|(_, t)| t).collect()
    }

    #[test]
    fn finds_comments_outside_strings() {
        let rs = "let a = \"// not\"; // yes\nfn f<'a>(x: &'a str) -> char { '/' } // after a lifetime\nlet r = r#\"/* raw */\"#;\n/* block\n spans */ let b = 1;\n/// doc";
        assert_eq!(texts(comments_in(rs, "rust")), vec!["yes", "after a lifetime", "block\n spans", "/ doc"]);
        let py = "x = '#no'  # yes\ns = \"\"\"\n# in a docstring\n\"\"\"\n#!/usr/bin/env python";
        assert_eq!(texts(comments_in(py, "python")), vec!["yes", "!/usr/bin/env python"]);
        assert_eq!(texts(comments_in("echo $# ${#x} # real", "shell")), vec!["real"]);
        assert_eq!(texts(comments_in("const u = `http://x` // ok", "typescript")), vec!["ok"]);
        assert_eq!(texts(comments_in("<p>hi</p> <!-- note -->", "html")), vec!["note"]);
    }

    #[test]
    fn only_new_comments_count_and_pragmas_pass() {
        let c = on();
        assert!(added("let a = 1; // keep", "let a = 2; // keep", "rust", &c.pragmas).is_empty(), "an existing comment carried over");
        assert_eq!(texts(added("", "x = 1  # noqa: E501\ny = 2  # why", "python", &c.pragmas)), vec!["why"]);
        assert!(added("", "// eslint-disable-next-line no-console\n// @ts-ignore", "typescript", &c.pragmas).is_empty());
    }

    #[test]
    fn edits_are_checked_by_language_when_on() {
        let c = on();
        let edit = serde_json::json!({"file_path": "/x/a.rs", "old_string": "let a = 1;", "new_string": "// set a\nlet a = 2;"});
        assert_eq!(edit_adds(&c, "Edit", &edit), vec![Found { file: "/x/a.rs".into(), line: 1, text: "set a".into() }]);
        assert!(edit_adds(&CommentsConfig::default(), "Edit", &edit).is_empty(), "off by default");
        let md = serde_json::json!({"file_path": "/x/README.md", "old_string": "", "new_string": "<!-- hi -->"});
        assert!(edit_adds(&c, "Edit", &md).is_empty(), "not a watched language");
        let multi = serde_json::json!({"file_path": "/x/a.dart", "edits": [{"old_string": "a", "new_string": "b // why"}, {"old_string": "c", "new_string": "/* and */ d"}]});
        assert_eq!(edit_adds(&c, "MultiEdit", &multi).len(), 2);
    }

    #[test]
    fn reads_a_branch_diff() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -3,1 +3,2 @@\n-let a = 1; // old\n+let a = 2; // old\n+// new one\n\
                    diff --git a/notes.md b/notes.md\n--- a/notes.md\n+++ b/notes.md\n@@ -0,0 +1 @@\n+# heading\n";
        assert_eq!(diff_adds(&on(), diff), vec![Found { file: "src/a.rs".into(), line: 4, text: "new one".into() }]);
    }
}
