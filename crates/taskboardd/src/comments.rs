//! The comment guard (`[comments]` in config.toml): with it on, agents may not add code comments.
//! A pragma the tools read (`# noqa`, `// eslint-disable-next-line`, …) is allowed. The `PreToolUse`
//! hook refuses an Edit, Write, MultiEdit or NotebookEdit that adds one, and the board refuses the
//! PR, a review round and `tb submit` while the branch adds any.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::Value;

use crate::config::{CommentsConfig, Config};
use crate::util::RowExt;

const GIT_SECS: f64 = 10.0;
const FETCH_SECS: f64 = 60.0;
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
    /// Python: a triple-quoted string that starts a statement is a docstring, a comment.
    docstrings: bool,
    /// JavaScript: `/…/` regex literals, and `${…}` code inside a `` ` `` template.
    scripted: bool,
}

const C_BLOCK: &[(&str, &str)] = &[("/*", "*/")];
const SLASHES: &[&str] = &["//"];
const HASH: &[&str] = &["#"];
const DASHES: &[&str] = &["--"];

fn syntax(lang: &str) -> Option<Syntax> {
    let s = |line, block, quotes, triple, char_lit| Syntax { line, block, quotes, triple, char_lit, raw: false, word_start: false, docstrings: false, scripted: false };
    Some(match lang {
        "rust" => Syntax { raw: true, ..s(SLASHES, C_BLOCK, &['"'], false, true) },
        "dart" => s(SLASHES, C_BLOCK, &['"', '\''], true, false),
        "typescript" | "javascript" => Syntax { scripted: true, ..s(SLASHES, C_BLOCK, &['"', '\'', '`'], false, false) },
        "swift" => s(SLASHES, C_BLOCK, &['"'], true, false),
        "kotlin" | "java" | "scala" => s(SLASHES, C_BLOCK, &['"'], true, true),
        "go" => s(SLASHES, C_BLOCK, &['"', '`'], false, true),
        "c" | "cpp" | "csharp" | "objc" => s(SLASHES, C_BLOCK, &['"'], false, true),
        "php" => s(&["//", "#"], C_BLOCK, &['"', '\''], false, false),
        "scss" | "less" => s(SLASHES, C_BLOCK, &['"', '\''], false, false),
        "css" => s(&[], C_BLOCK, &['"', '\''], false, false),
        "python" => Syntax { docstrings: true, ..s(HASH, &[], &['"', '\''], true, false) },
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
    // In a docstring: its quotes, where it started, its text so far with its opening quotes.
    let mut doc: Option<(&str, usize, String)> = None;
    // Each open `${` in a template: how many `{` deep its code is.
    let mut templates: Vec<usize> = vec![];
    // The last code character and the word it ends, which tell a regex's `/` from a division.
    let mut prev = ';';
    let mut word = String::new();
    for (n, line) in text.lines().enumerate() {
        let c: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < c.len() {
            if let Some((close, from, buf)) = doc.as_mut() {
                if c[i] == '\\' {
                    buf.extend(c[i..(i + 2).min(c.len())].iter());
                    i += 2;
                } else if at(&c, i, close) {
                    i += 3;
                    out.push((*from, docstring_text(buf, close)));
                    doc = None;
                } else {
                    buf.push(c[i]);
                    i += 1;
                }
                continue;
            }
            if let Some((close, from, buf)) = block.as_mut() {
                if at(&c, i, close) {
                    i += close.chars().count();
                    out.push((*from, block_text(buf)));
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
                } else if sx.scripted && close == "`" && at(&c, i, "${") {
                    templates.push(0);
                    string = None;
                    (prev, i) = ('{', i + 2);
                    word.clear();
                } else if at(&c, i, close) {
                    i += close.chars().count();
                    string = None;
                    prev = '"';
                    word.clear();
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
                    let rest: String = c[i + tok.chars().count()..].iter().collect();
                    let rest = if *tok == "//" { rest.trim_start_matches('/').strip_prefix('!').unwrap_or(rest.trim_start_matches('/')).to_string() } else { rest };
                    out.push((n + 1, rest.trim().to_string()));
                    break;
                }
                i += 1;
                continue;
            }
            if sx.scripted && c[i] == '/' && regex_may_start(prev, &word) {
                if let Some(end) = regex_end(&c, i) {
                    i = end;
                    prev = '/';
                    word.clear();
                    continue;
                }
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
                    if let Some(prefix) = statement_prefix(&c, i).filter(|_| sx.docstrings) {
                        doc = Some((*q, n + 1, format!("{prefix}{q}")));
                        i += 3;
                        continue;
                    }
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
            if let Some(depth) = templates.last_mut() {
                match (c[i], *depth) {
                    ('{', _) => *depth += 1,
                    ('}', 0) => {
                        templates.pop();
                        string = Some(("`".to_string(), true));
                    }
                    ('}', _) => *depth -= 1,
                    _ => {}
                }
            }
            if is_word_char(c[i]) {
                if !(is_word_char(prev) && i > 0 && c[i - 1] == prev) {
                    word.clear();
                }
                word.push(c[i]);
                prev = c[i];
            } else if !c[i].is_whitespace() {
                word.clear();
                prev = c[i];
            }
            i += 1;
        }
        if let Some((_, _, buf)) = block.as_mut().or(doc.as_mut()) {
            buf.push('\n');
        }
    }
    if let Some((_, from, buf)) = block {
        out.push((from, block_text(&buf)));
    }
    if let Some((close, from, buf)) = doc {
        out.push((from, docstring_text(&buf, close)));
    }
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// A `/` after `prev` (the last code character, ending `word`) starts a regex, not a division: it
/// follows an operator, an opening bracket or a keyword, where a value is due.
fn regex_may_start(prev: char, word: &str) -> bool {
    const BEFORE_VALUE: &[&str] = &["return", "typeof", "instanceof", "in", "of", "new", "delete", "void", "throw", "case", "do", "else", "yield", "await"];
    if is_word_char(prev) {
        return BEFORE_VALUE.contains(&word);
    }
    "(,=:[!&|?{};+-*%<>~^".contains(prev)
}

/// Where the regex literal opening at `i` ends, flags and all: its closing `/` outside a `[…]` class
/// on the same line. None when the line has none, so the `/` was a division after all.
fn regex_end(c: &[char], i: usize) -> Option<usize> {
    let mut j = i + 1;
    let mut class = false;
    while j < c.len() {
        match c[j] {
            '\\' => j += 1,
            '[' => class = true,
            ']' => class = false,
            '/' if !class => return Some(j + 1 + c[j + 1..].iter().take_while(|x| x.is_ascii_alphabetic()).count()),
            _ => {}
        }
        j += 1;
    }
    None
}

/// The string prefix (`r`, `b`, `f`, `u`, or two of them) before the quotes at `i`, when only it and
/// whitespace come before them on the line: the string starts a statement.
fn statement_prefix(c: &[char], i: usize) -> Option<String> {
    let start = (i.saturating_sub(2)..i).find(|&j| c[j..i].iter().all(|x| "rRbBfFuU".contains(*x))).unwrap_or(i);
    c[..start].iter().all(|x| x.is_whitespace()).then(|| c[start..i].iter().collect())
}

/// A docstring as it reads in the source, quotes and all, each line's indent dropped.
fn docstring_text(buf: &str, close: &str) -> String {
    format!("{buf}{close}").lines().map(|l| l.trim()).collect::<Vec<_>>().join("\n")
}

/// A block comment's text: a doc marker (`/**`, `/*!`) and each line's leading `*` dropped.
fn block_text(buf: &str) -> String {
    let buf = buf.strip_prefix('*').or(buf.strip_prefix('!')).unwrap_or(buf);
    let lines: Vec<&str> = buf.lines().map(|l| l.trim()).map(|l| l.strip_prefix('*').map(|r| r.trim_start()).unwrap_or(l)).collect();
    lines.join("\n").trim().to_string()
}

pub fn is_pragma(text: &str, pragmas: &[String]) -> bool {
    let t = text.trim();
    pragmas.iter().map(|p| p.trim()).filter(|p| !p.is_empty()).any(|p| t.starts_with(p))
}

/// The comments `new` has that `old` didn't (by what they say), pragmas and empty ones aside.
pub fn added(old: &str, new: &str, lang: &str, pragmas: &[String]) -> Vec<(usize, String)> {
    added_over(&[old], new, lang, pragmas)
}

/// The comments `new` has that none of `olds` did: each comment may appear as many times as the
/// old text that had it most.
fn added_over(olds: &[&str], new: &str, lang: &str, pragmas: &[String]) -> Vec<(usize, String)> {
    let mut had: HashMap<String, usize> = HashMap::new();
    for old in olds {
        let mut here: HashMap<String, usize> = HashMap::new();
        for (_, t) in comments_in(old, lang) {
            *here.entry(t).or_default() += 1;
        }
        for (t, k) in here {
            let e = had.entry(t).or_default();
            *e = (*e).max(k);
        }
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
    (c.guard && !is_build_output(path) && c.languages.iter().any(|l| l.trim().eq_ignore_ascii_case(lang))).then_some(lang)
}

/// A bundler wrote this file, so nobody writes its comments: a minified file (`app.min.js`), or one
/// in a build folder whose name carries a content hash (`dist/assets/api.md.Bx3kR9aQ.js`,
/// `build/static/js/main-3f2a1b9c.js`).
pub fn is_build_output(path: &str) -> bool {
    const BUILD_DIRS: &[&str] = &["dist", "build", "out", ".output", ".next", "_next", ".nuxt", ".svelte-kit", "_app"];
    let mut parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    let Some(name) = parts.pop() else { return false };
    let pieces: Vec<&str> = name.split('.').collect();
    if pieces.len() < 2 {
        return false;
    }
    let stem = &pieces[..pieces.len() - 1];
    if stem.len() > 1 && stem.last().is_some_and(|p| p.eq_ignore_ascii_case("min")) {
        return true;
    }
    let hashed = stem.iter().skip(1).any(|p| is_content_hash(p))
        || stem.iter().any(|p| p.char_indices().filter(|(_, ch)| *ch == '-').any(|(k, _)| is_content_hash(&p[k + 1..])));
    hashed && parts.iter().any(|d| BUILD_DIRS.contains(&d.to_ascii_lowercase().as_str()))
}

/// A bundler's content hash, which a word in a file name doesn't look like: 8 base64url characters
/// (Vite, Rollup, esbuild) or 8 or more hex digits (webpack, Next), mixing digits and letters, or both
/// cases.
fn is_content_hash(s: &str) -> bool {
    let has = |f: fn(&char) -> bool| s.chars().any(|ch| f(&ch));
    let mixed = (has(char::is_ascii_digit) && has(char::is_ascii_alphabetic)) || (has(char::is_ascii_uppercase) && has(char::is_ascii_lowercase));
    let base64 = s.len() == 8 && s.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');
    let hex = s.len() >= 8 && s.chars().all(|ch| ch.is_ascii_hexdigit());
    mixed && (base64 || hex)
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
            "No code comments or docstrings: {} wants none added (in {}). Make the code say it with names and small functions, and put the why in the commit \
             or PR. Pragmas the tools read ({}…) are fine. The board refuses an edit that adds a comment, and the PR, a review round and tb submit \
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
            let edits: Vec<(String, String, bool)> = if tool == "Edit" {
                vec![(s(&input["old_string"]), s(&input["new_string"]), input["replace_all"] == true)]
            } else {
                input["edits"].as_array().into_iter().flatten().map(|e| (s(&e["old_string"]), s(&e["new_string"]), e["replace_all"] == true)).collect()
            };
            let before = std::fs::read_to_string(&path).ok();
            match before.as_deref().and_then(|b| apply_edits(b, &edits)) {
                Some(after) => {
                    let before = before.unwrap_or_default();
                    let head = at_head(&path).unwrap_or_default();
                    return found_in(&path, added_over(&[&before, &head], &after, lang, &c.pragmas));
                }
                None => pairs.extend(edits.into_iter().map(|(o, n, _)| (o, n, lang))),
            }
        }
        "Write" => {
            let Some(lang) = watched(c, &path) else { return vec![] };
            let before = std::fs::read_to_string(&path).unwrap_or_default();
            let head = at_head(&path).unwrap_or_default();
            return found_in(&path, added_over(&[&before, &head], &s(&input["content"]), lang, &c.pragmas));
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

fn found_in(path: &str, adds: Vec<(usize, String)>) -> Vec<Found> {
    adds.into_iter().map(|(line, text)| Found { file: path.to_string(), line, text }).collect()
}

/// A file's text after an Edit or MultiEdit, the way Claude applies them: in order, each `old`
/// replaced once (or everywhere with `replace_all`). None when an `old` isn't there.
pub fn apply_edits(text: &str, edits: &[(String, String, bool)]) -> Option<String> {
    let mut out = text.to_string();
    for (old, new, all) in edits {
        if old.is_empty() {
            if !out.is_empty() {
                return None;
            }
            out = new.clone();
        } else if !out.contains(old.as_str()) {
            return None;
        } else if *all {
            out = out.replace(old.as_str(), new);
        } else {
            out = out.replacen(old.as_str(), new, 1);
        }
    }
    Some(out)
}

/// The file as it is at HEAD in its repo, if it's tracked there.
fn at_head(path: &str) -> Option<String> {
    let p = Path::new(path);
    let dir = p.parent()?.to_str()?;
    let name = p.file_name()?.to_str()?;
    Git(crate::proc::which("git")).run(dir, &["show", &format!("HEAD:./{name}")])
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

/// Why a task's `what` can't go ahead: its branch (in `cwd`, else its worktree, else the project folder
/// when the task ships a PR) adds comments. None with the guard off.
pub fn task_refusal(app: &crate::app::App, t: &crate::util::Row, cwd: Option<&str>, what: &str) -> crate::util::Result<Option<String>> {
    if !app.cfg.comments.guard {
        return Ok(None);
    }
    let ctx = crate::board::task_context(t);
    let wt = ctx.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str()).filter(|w| Path::new(w).is_dir()).map(|s| s.to_string());
    let dir = match cwd.filter(|c| !c.is_empty()).map(|c| c.to_string()).or(wt) {
        Some(d) => Some(d),
        None if crate::projects::task_ships_pr(app, t)? => crate::runner::task_cwd(app, t)?,
        None => None,
    };
    let rec = crate::util::jloads_obj(t.s("pr_flow")).get("rec").cloned().filter(|r| !pr_side(r, "base").is_empty() && !pr_side(r, "branch").is_empty());
    let Some(rec) = rec else {
        let bases = guard_bases(app, t)?;
        return Ok(dir.and_then(|d| refusal(&app.cfg, adds_over(&app.cfg.comments, &d, &bases), "this branch", what)));
    };
    let repo = crate::projects::project_path(app, t.s("project"))?.or_else(|| t.s("repo_path").map(|s| s.to_string()));
    let dirs: Vec<String> = dir.into_iter().chain(repo).collect();
    let known = known_tips(app, t, &pr_side(&rec, "base"))?;
    Ok(refusal(&app.cfg, pr_adds(&app.cfg.comments, &dirs, &rec, &known), "this PR", what))
}

/// The branches a task's branch may have forked from. A PR base (the unmerged parent's branch, with the
/// last head the board saw on it, else the one the task or its goal is set to) is the only one; without
/// one, the base its slot started at, its goal's worktree base and the repo's default branch, nearest
/// first when judged. `fork_point` judges the default branch's fork alongside them either way.
fn guard_bases(app: &crate::app::App, t: &crate::util::Row) -> crate::util::Result<Vec<String>> {
    if let Some(b) = crate::stack::base_branch(app, t)? {
        let known = known_tips(app, t, &b)?;
        return Ok([b].into_iter().chain(known).collect());
    }
    if let Some(b) = crate::steps::chosen_base(app, t)? {
        return Ok(vec![b]);
    }
    let slot = crate::board::task_context(t).s("worktree_base").filter(|b| *b != "HEAD").map(|b| b.to_string());
    let goal = crate::board::find_goal(app, t.i("goal_id"))?.and_then(|g| g.s("worktree_base").map(|b| b.to_string()));
    let default = crate::steps::real_base(app, t)?;
    let mut out: Vec<String> = vec![];
    for b in slot.into_iter().chain(goal).chain([default]).filter(|b| !b.is_empty()) {
        if !out.contains(&b) {
            out.push(b);
        }
    }
    Ok(out)
}

/// The heads the board last saw on `branch` from the tasks this one builds on (its stack parent and the
/// tasks it waits for): what it told the task to rebase onto when that branch moved. The host's
/// `base_head` on this task's own PR stays at the old tip until the PR is read again.
fn known_tips(app: &crate::app::App, t: &crate::util::Row, branch: &str) -> crate::util::Result<Vec<String>> {
    let mut out = vec![];
    if branch.is_empty() {
        return Ok(out);
    }
    for n in crate::waitsfor::deps(t) {
        let Some(o) = crate::board::find_task(app, Some(n))? else { continue };
        if crate::stack::branch(&o).as_deref() != Some(branch) {
            continue;
        }
        if let Some(h) = crate::waitsfor::head_of(&o).filter(|h| !h.is_empty() && !out.contains(h)) {
            out.push(h);
        }
    }
    Ok(out)
}

struct Git(Option<std::path::PathBuf>);

impl Git {
    fn run(&self, dir: &str, args: &[&str]) -> Option<String> {
        self.run_for(dir, args, GIT_SECS)
    }

    fn run_for(&self, dir: &str, args: &[&str], secs: f64) -> Option<String> {
        let bin = self.0.as_ref()?;
        let mut all: Vec<String> = vec!["-C".into(), dir.into()];
        all.extend(args.iter().map(|a| a.to_string()));
        crate::proc::run(bin, &all, None, secs).ok().filter(|o| o.code == Some(0)).map(|o| o.stdout)
    }

    /// The first of `names` that is a commit in `dir`.
    fn commit(&self, dir: &str, names: &[String]) -> Option<String> {
        names.iter().filter(|n| !n.is_empty()).find_map(|n| {
            self.run(dir, &["rev-parse", "--verify", "--quiet", &format!("{n}^{{commit}}")]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
        })
    }
}

fn pr_side(rec: &Value, key: &str) -> String {
    rec[key].as_str().unwrap_or("").trim().to_string()
}

/// The comments the PR in `rec` (`pr_flow.rec`) adds: `merge-base(base, head)..head`, the diff the PR
/// shows, or from the default branch's fork when that is nearer the head (see `fork_point`), read in the first of `dirs` that has its commits. `known` holds tips the board saw on the base
/// since the host last read the PR. A checkout on the PR's branch counts its uncommitted work too.
pub fn pr_adds(c: &CommentsConfig, dirs: &[String], rec: &Value, known: &[String]) -> Vec<Found> {
    if !c.guard {
        return vec![];
    }
    let git = Git(crate::proc::which("git"));
    let Some((dir, head, diff)) = dirs.iter().filter(|d| Path::new(d.as_str()).is_dir()).find_map(|d| pr_diff(&git, d, rec, known).map(|(h, diff)| (d, h, diff))) else {
        return vec![];
    };
    diff_adds_in(c, &diff, |f| match &head {
        Some(h) => git.run(dir, &["show", &format!("{h}:{f}")]),
        None => std::fs::read_to_string(Path::new(dir).join(f)).ok(),
    })
}

/// The PR's diff, and the commit its new side is (None: the checkout's working tree).
fn pr_diff(git: &Git, dir: &str, rec: &Value, known: &[String]) -> Option<(Option<String>, String)> {
    let (base, branch) = (pr_side(rec, "base"), pr_side(rec, "branch"));
    let here = git.run(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).is_some_and(|b| b.trim() == branch);
    let h = if here { "HEAD".to_string() } else { tip(git, dir, &branch, &pr_side(rec, "head"))? };
    let tips: Vec<String> = known.iter().cloned().chain([pr_side(rec, "base_head")]).collect();
    let on_base = nearest_fork(git, dir, &base, &tips, &h);
    let on_default = default_branch(git, dir).filter(|d| bare(d) != bare(&base)).and_then(|d| nearest_fork(git, dir, &d, &[], &h));
    let fork = nearest(git, dir, on_base.into_iter().chain(on_default), &h)?;
    let mut args = vec!["diff", "--no-color", "--no-ext-diff", "-U0", "--diff-filter=AMR", fork.as_str()];
    if !here {
        args.push(&h);
    }
    git.run(dir, &args).map(|diff| ((!here).then_some(h.clone()), diff))
}

/// Where `branch` is now: origin's tip once fetched, since the host's `recorded` SHA goes stale when
/// the branch is pushed again; else that SHA, else what git in `dir` already has.
fn tip(git: &Git, dir: &str, branch: &str, recorded: &str) -> Option<String> {
    let fetched = !branch.is_empty() && git.run_for(dir, &["fetch", "-q", "--no-tags", "origin", branch], FETCH_SECS).is_some();
    let (remote, local, recorded) = (format!("origin/{branch}"), branch.to_string(), recorded.to_string());
    let order = if fetched { [remote, recorded, local] } else { [recorded, remote, local] };
    git.commit(dir, &order)
}

/// Where the branch at `dir` forked from the nearest of `bases` (the branches its PR may go into) and
/// its default branch, each fetched first. A stacked base far behind the default branch, with the
/// branch rebased onto the default branch, forks before every commit the default branch gained since;
/// those were reviewed where they landed, so the default branch's fork wins when it is nearer.
fn fork_point(git: &Git, dir: &str, bases: &[String]) -> Option<String> {
    let default = default_branch(git, dir);
    let mut names: Vec<&str> = vec![];
    for b in bases.iter().map(String::as_str).chain(default.as_deref()).filter(|b| !b.is_empty()) {
        if !names.iter().any(|n| bare(n) == bare(b)) {
            names.push(b);
        }
    }
    let forks: Vec<String> = names.iter().filter_map(|b| nearest_fork(git, dir, b, &[], "HEAD")).collect();
    nearest(git, dir, forks, "HEAD")
}

/// The repo's default branch as git in `dir` knows it: origin's HEAD, else `main` or `master`.
fn default_branch(git: &Git, dir: &str) -> Option<String> {
    let head = git.run(dir, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]).map(|h| h.trim().to_string());
    let names = head.into_iter().chain(["origin/main", "origin/master", "main", "master"].map(String::from));
    names.filter(|n| !n.is_empty()).find(|n| git.commit(dir, std::slice::from_ref(n)).is_some())
}

fn bare(branch: &str) -> &str {
    branch.strip_prefix("origin/").unwrap_or(branch)
}

/// The one of `forks` that `head` has the fewest commits past.
fn nearest(git: &Git, dir: &str, forks: impl IntoIterator<Item = String>, head: &str) -> Option<String> {
    forks.into_iter().min_by_key(|f| commits_past(git, dir, f, head))
}

/// Where `head` forked from `base` (`release/x`, `origin/release/x` or a commit), judged against every
/// tip git has for it: origin's (fetched first, else the local branch) and the `known` SHAs. The fork
/// nearest `head` wins. After the base moves and the branch follows with `rebase --onto`, a tip from
/// before the move forks far back, and every commit the new base brought in would count as the
/// branch's own; a fetch that fails or times out must not leave such a tip to decide.
fn nearest_fork(git: &Git, dir: &str, base: &str, known: &[String], head: &str) -> Option<String> {
    let name = bare(base);
    if !name.is_empty() && !is_sha(name) {
        git.run_for(dir, &["fetch", "-q", "--no-tags", "origin", name], FETCH_SECS);
    }
    let branch = (!name.is_empty()).then(|| git.commit(dir, &[format!("origin/{name}"), name.to_string()])).flatten();
    let known = known.iter().filter_map(|k| git.commit(dir, std::slice::from_ref(k)));
    let mut forks: Vec<String> = vec![];
    for tip in branch.into_iter().chain(known) {
        let Some(f) = git.run(dir, &["merge-base", &tip, head]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else { continue };
        if !forks.contains(&f) {
            forks.push(f);
        }
    }
    nearest(git, dir, forks, head)
}

/// How many commits `head` has past `fork`; the most there can be when git can't count them.
fn commits_past(git: &Git, dir: &str, fork: &str, head: &str) -> u64 {
    git.run(dir, &["rev-list", "--count", &format!("{fork}..{head}")]).and_then(|n| n.trim().parse().ok()).unwrap_or(u64::MAX)
}

fn is_sha(name: &str) -> bool {
    (7..=40).contains(&name.len()) && name.chars().all(|c| c.is_ascii_hexdigit())
}

/// The comments a unified diff (`-U0`) adds, by file.
pub fn diff_adds(c: &CommentsConfig, diff: &str) -> Vec<Found> {
    diff_adds_in(c, diff, |_| None)
}

/// The comments a unified diff (`-U0`) adds, each kept only when `read` (a file's whole new text, by
/// its path) has a comment on that line too: a hunk read alone can start inside a string that an
/// earlier line opened, a template spanning lines in a bundle, and take its `//` for a comment.
fn diff_adds_in(c: &CommentsConfig, diff: &str, read: impl Fn(&str) -> Option<String>) -> Vec<Found> {
    let found = hunk_adds(c, diff);
    let mut lines: HashMap<String, Option<HashSet<usize>>> = HashMap::new();
    found
        .into_iter()
        .filter(|f| {
            let starts = lines.entry(f.file.clone()).or_insert_with(|| {
                let lang = language_of(&f.file)?;
                read(&f.file).map(|text| comments_in(&text, lang).into_iter().map(|(n, _)| n).collect())
            });
            starts.as_ref().is_none_or(|s| s.contains(&f.line))
        })
        .collect()
}

fn hunk_adds(c: &CommentsConfig, diff: &str) -> Vec<Found> {
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

/// The comments the branch at `dir` adds over `base` (the branch its PR goes into), else its default
/// branch, committed or not.
pub fn branch_adds(c: &CommentsConfig, dir: &str, base: Option<&str>) -> Vec<Found> {
    adds_over(c, dir, &base.map(|b| b.to_string()).into_iter().collect::<Vec<_>>())
}

/// The comments the branch at `dir` adds over the nearest of `bases`, else its default branch.
fn adds_over(c: &CommentsConfig, dir: &str, bases: &[String]) -> Vec<Found> {
    if !c.guard || dir.is_empty() || !Path::new(dir).is_dir() {
        return vec![];
    }
    let git = Git(crate::proc::which("git"));
    let Some(base) = fork_point(&git, dir, bases) else { return vec![] };
    let diff = git.run(dir, &["diff", "--no-color", "--no-ext-diff", "-U0", "--diff-filter=AMR", &base]).unwrap_or_default();
    diff_adds_in(c, &diff, |f| std::fs::read_to_string(Path::new(dir).join(f)).ok())
}

/// Why `what` ("opening the PR", "finishing") can't go ahead: the branch adds comments over `base`.
pub fn branch_refusal(cfg: &Config, dir: &str, base: Option<&str>, what: &str) -> Option<String> {
    refusal(cfg, branch_adds(&cfg.comments, dir, base), "this branch", what)
}

fn refusal(cfg: &Config, found: Vec<Found>, whose: &str, what: &str) -> Option<String> {
    if found.is_empty() {
        return None;
    }
    let owner = cfg.owner.clone();
    Some(format!(
        "Task board: not {what} yet: {whose} adds {} ({}), and {owner} wants no comments in code. Take them out and make the code say it \
         (names, small functions); the why goes in the commit or PR. Then commit and try again. Pragmas the tools read are fine.",
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
        assert_eq!(texts(comments_in(rs, "rust")), vec!["yes", "after a lifetime", "block\nspans", "doc"]);
        assert_eq!(texts(comments_in("/// A doc\n//! inner\n/** one */\n/**\n * two\n * lines\n */", "dart")), vec!["A doc", "inner", "one", "two\nlines"]);
        let py = "x = '#no'  # yes\ns = \"\"\"\n# in a docstring\n\"\"\"\n#!/usr/bin/env python";
        assert_eq!(texts(comments_in(py, "python")), vec!["yes", "!/usr/bin/env python"]);
        assert_eq!(texts(comments_in("echo $# ${#x} # real", "shell")), vec!["real"]);
        assert_eq!(texts(comments_in("const u = `http://x` // ok", "typescript")), vec!["ok"]);
        assert_eq!(texts(comments_in("<p>hi</p> <!-- note -->", "html")), vec!["note"]);
    }

    #[test]
    fn a_python_docstring_is_a_comment_but_a_triple_quoted_value_isn_t() {
        let c = on();
        let adds = |new: &str| texts(added("", new, "python", &c.pragmas));
        assert_eq!(adds("def f():\n    \"\"\"Return one.\"\"\""), vec!["\"\"\"Return one.\"\"\""]);
        assert_eq!(adds("\"\"\"The module.\"\"\"\nimport os"), vec!["\"\"\"The module.\"\"\""]);
        assert_eq!(adds("class A:\n    '''An A.'''"), vec!["'''An A.'''"]);
        assert_eq!(adds("def f():\n    \"\"\"\n    Return one.\n    \"\"\"\n    return 1"), vec!["\"\"\"\nReturn one.\n\"\"\""]);
        assert_eq!(adds("def f():\n    r\"\"\"Return \\d.\"\"\""), vec!["r\"\"\"Return \\d.\"\"\""]);
        assert_eq!(adds("def f():\n    Rb'''Bytes \\''' still.'''"), vec!["Rb'''Bytes \\''' still.'''"]);
        assert!(adds("x = \"\"\"data\"\"\"\nprint(\"\"\"data\"\"\")\nreturn f(\n    1, '''a # b''')").is_empty(), "values stay strings");
        assert_eq!(comments_in("x = 1\ndef f():\n    \"\"\"Return one.\"\"\"", "python")[0].0, 3, "the docstring's line");
        let stub = serde_json::json!({"file_path": "/nowhere/m.pyi", "content": "def f() -> int:\n    \"\"\"Return one.\"\"\"\n    ..."});
        assert_eq!(edit_adds(&c, "Write", &stub), vec![Found { file: "/nowhere/m.pyi".into(), line: 2, text: "\"\"\"Return one.\"\"\"".into() }]);
        let diff = "diff --git a/a.py b/a.py\n--- a/a.py\n+++ b/a.py\n@@ -1,0 +2,4 @@\n+def f():\n+    \"\"\"\n+    Return one.\n+    \"\"\"\n\
                    diff --git a/b.py b/b.py\n--- a/b.py\n+++ b/b.py\n@@ -1,0 +1 @@\n+SQL = \"\"\"select 1\"\"\"\n";
        assert_eq!(diff_adds(&c, diff), vec![Found { file: "a.py".into(), line: 3, text: "\"\"\"\nReturn one.\n\"\"\"".into() }], "the branch check too");
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
        let edit = serde_json::json!({"file_path": "/nowhere/a.rs", "old_string": "let a = 1;", "new_string": "// set a\nlet a = 2;"});
        assert_eq!(edit_adds(&c, "Edit", &edit), vec![Found { file: "/nowhere/a.rs".into(), line: 1, text: "set a".into() }], "an unreadable file falls back to the snippet");
        assert!(edit_adds(&CommentsConfig::default(), "Edit", &edit).is_empty(), "off by default");
        let md = serde_json::json!({"file_path": "/x/README.md", "old_string": "", "new_string": "<!-- hi -->"});
        assert!(edit_adds(&c, "Edit", &md).is_empty(), "not a watched language");
        let multi = serde_json::json!({"file_path": "/x/a.dart", "edits": [{"old_string": "a", "new_string": "b // why"}, {"old_string": "c", "new_string": "/* and */ d"}]});
        assert_eq!(edit_adds(&c, "MultiEdit", &multi).len(), 2);
    }

    #[test]
    fn applies_edits_like_claude() {
        let e = |o: &str, n: &str, all| (o.to_string(), n.to_string(), all);
        assert_eq!(apply_edits("a b a", &[e("a", "x", false)]).as_deref(), Some("x b a"));
        assert_eq!(apply_edits("a b a", &[e("a", "x", true), e("b", "y", false)]).as_deref(), Some("x y x"));
        assert_eq!(apply_edits("a", &[e("z", "x", false)]), None);
        assert_eq!(apply_edits("", &[e("", "new", false)]).as_deref(), Some("new"));
    }

    #[test]
    fn edits_are_judged_against_the_whole_file() {
        let c = on();
        let dir = std::env::temp_dir().join(format!("tb-comments-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("m.rs");
        let path = f.to_str().unwrap().to_string();
        std::fs::write(&f, "fn a() {}\n// keep me\nfn b() {}\nfn c() {}\n").unwrap();
        let moved = serde_json::json!({"file_path": path, "edits": [
            {"old_string": "// keep me\n", "new_string": ""},
            {"old_string": "fn c() {}", "new_string": "    // keep me\nfn c() {}"}
        ]});
        assert!(edit_adds(&c, "MultiEdit", &moved).is_empty(), "a comment already in the file may move or re-indent");
        let new = serde_json::json!({"file_path": path, "old_string": "fn b() {}\nfn c() {}", "new_string": "fn b() {}\n// new\nfn c() {}"});
        assert_eq!(edit_adds(&c, "Edit", &new), vec![Found { file: path.clone(), line: 4, text: "new".into() }], "the file's line, not the snippet's");
        let doubled = serde_json::json!({"file_path": path, "edits": [{"old_string": "fn a() {}", "new_string": "// keep me\nfn a() {}"}]});
        assert_eq!(edit_adds(&c, "MultiEdit", &doubled).len(), 1, "a second copy is new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_comment_at_head_may_come_back() {
        let Some(git) = crate::proc::which("git") else { return };
        let dir = std::env::temp_dir().join(format!("tb-comments-head-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_str().unwrap().to_string();
        let g = |args: &[&str]| {
            let mut all: Vec<String> = vec!["-C".into(), d.clone(), "-c".into(), "user.name=t".into(), "-c".into(), "user.email=t@t".into()];
            all.extend(args.iter().map(|a| a.to_string()));
            assert!(crate::proc::run(&git, &all, None, 10.0).is_ok_and(|o| o.code == Some(0)))
        };
        g(&["init", "-q"]);
        let f = dir.join("m.rs");
        std::fs::write(&f, "// moved\nfn a() {}\nfn b() {}\n").unwrap();
        g(&["add", "m.rs"]);
        g(&["commit", "-q", "-m", "x"]);
        std::fs::write(&f, "fn a() {}\nfn b() {}\n").unwrap();
        let path = f.to_str().unwrap().to_string();
        let back = serde_json::json!({"file_path": path, "old_string": "fn b() {}", "new_string": "// moved\nfn b() {}"});
        assert!(edit_adds(&on(), "Edit", &back).is_empty(), "the second half of a move");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn slashes_in_js_strings_regexes_and_templates_are_not_comments() {
        let js = |t: &str| texts(comments_in(t, "javascript"));
        assert!(js("const a = \"// jump: top, bottom\", b = '// x';").is_empty(), "string literals");
        assert!(js("const html = `<pre>\n// jump: top\n</pre>`;").is_empty(), "a template");
        assert!(js("const t = `<code>\n// one line\n// and another\n</code>`\nf(t)").is_empty(), "a template spanning lines");
        assert_eq!(js("const q = /[\"'`]/g; const u = \"//x\"; // real"), vec!["real"], "a regex with quotes in a class");
        assert_eq!(js("if (/\\/\\//.test(p)) return /\\/*/; // after"), vec!["after"], "escaped slashes and a /* in a regex");
        assert_eq!(js("const r = a / b; // halves\nconst s = x/2/y // too"), vec!["halves", "too"], "divisions aren't regexes");
        assert!(js("return /\"/.test(s) && t === \"//\"").is_empty(), "a regex after a keyword");
        assert!(js("const s = `${a ? `in ${b} // nested` : '//'} // still text`;").is_empty(), "templates inside a template's code");
        assert_eq!(js("const s = `${ {a: 1}.a }`; // out"), vec!["out"], "braces inside a template's code");
        assert!(texts(comments_in("const s: string = `${f({ a: \"//\" })}//`;", "typescript")).is_empty());
    }

    #[test]
    fn hashed_build_output_is_not_watched() {
        let c = on();
        for p in [
            "docs/.vitepress/dist/assets/api.md.Bx3kR9aQ.js",
            "dist/assets/index-Bx-z12ab.js",
            "build/static/js/main.3f2a1b9c.js",
            "/repo/.next/static/chunks/page-3f2a1b9c4d5e6f70.js",
            "dist/assets/style.DcRmPxab.css",
            "vendor/jquery.min.js",
        ] {
            assert!(is_build_output(p), "{p}");
            assert_eq!(watched(&c, p), None, "{p}");
        }
        for p in ["src/api.md.ts", "dist/api-v2-client.js", "dist/index.js", "src/assets/index-Bx-z12ab.js", "build/scripts/release.js", "lib/handler.service.js"] {
            assert!(!is_build_output(p), "{p}");
        }
        let edit = serde_json::json!({"file_path": "/x/dist/assets/api.md.Bx3kR9aQ.js", "old_string": "", "new_string": "a(); // b"});
        assert!(edit_adds(&c, "Edit", &edit).is_empty(), "edits to a bundle pass too");
    }

    #[test]
    fn a_bundle_diff_adds_nothing() {
        let diff = "diff --git a/docs/.vitepress/dist/assets/api.md.Bx3kR9aQ.js b/docs/.vitepress/dist/assets/api.md.Bx3kR9aQ.js\n\
                    --- /dev/null\n+++ b/docs/.vitepress/dist/assets/api.md.Bx3kR9aQ.js\n@@ -0,0 +1,2 @@\n+const p=JSON.parse('{}');\n+// jump: top, bottom\n\
                    diff --git a/src/a.js b/src/a.js\n--- a/src/a.js\n+++ b/src/a.js\n@@ -0,0 +1 @@\n+f(); // mine\n";
        assert_eq!(diff_adds(&on(), diff), vec![Found { file: "src/a.js".into(), line: 1, text: "mine".into() }]);
    }

    #[test]
    fn a_hunk_inside_a_template_is_judged_with_the_whole_file() {
        let file = "export const page = `<pre>\n<code>\n// jump: top, bottom\n</code>\n</pre>`;\nf(); // mine\n";
        let diff = "diff --git a/site/page.js b/site/page.js\n--- a/site/page.js\n+++ b/site/page.js\n@@ -3 +3 @@\n-// jump: top\n+// jump: top, bottom\n\
                    @@ -6 +6 @@\n-f();\n+f(); // mine\n";
        let read = |p: &str| (p == "site/page.js").then(|| file.to_string());
        assert_eq!(diff_adds(&on(), diff).len(), 2, "alone, the first hunk reads like a comment");
        assert_eq!(diff_adds_in(&on(), diff, read), vec![Found { file: "site/page.js".into(), line: 6, text: "mine".into() }]);
    }

    #[test]
    fn reads_a_branch_diff() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -3,1 +3,2 @@\n-let a = 1; // old\n+let a = 2; // old\n+// new one\n\
                    diff --git a/notes.md b/notes.md\n--- a/notes.md\n+++ b/notes.md\n@@ -0,0 +1 @@\n+# heading\n";
        assert_eq!(diff_adds(&on(), diff), vec![Found { file: "src/a.rs".into(), line: 4, text: "new one".into() }]);
    }
}
