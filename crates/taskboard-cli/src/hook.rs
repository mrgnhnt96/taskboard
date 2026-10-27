//! `tb hook <Event>`: the Claude Code hooks. They report to the board and never get in Claude's way:
//! outside a Midna terminal they do nothing, they always exit 0, and they print only valid hook JSON.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::client::{self, CallError};
use taskboardd::comments;
use taskboardd::config::Config;
use taskboardd::hunks::{self, Hunk, Marks};
use taskboardd::steps::{from_listing, Before, Step};

const CONTEXT_EVENTS: &[&str] = &["SessionStart", "UserPromptSubmit"];
const WAITING_NOTIFICATIONS: &[&str] = &["permission_prompt", "idle_prompt"];
const CONTINUING_END_REASONS: &[&str] = &["clear", "resume"];
const GIT_TIMEOUT: f64 = 0.3;
const SLACK: f64 = 0.35;
const MAX_CONTEXT: usize = 9900;
/// A prompt is sent whole up to what the board keeps of one; a longer one keeps its start and its end.
#[cfg(test)]
const MAX_PROMPT: usize = taskboardd::board::PROMPT_FULL_KEEP;
const MAX_LAST_MESSAGE: usize = 2000;
const MAX_OUTPUT: usize = 1000;

/// A command that opens a pull request: `gh pr create`, `glab mr create`, or a POST to GitHub's
/// `…/pulls` or Bitbucket's `…/pullrequests` (through `gh api`, `tb api` or curl).
static PR_CLI_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b(?:gh\s+pr\s+create|glab\s+mr\s+create)\b").unwrap());
static PR_API_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"/(?:pulls|pullrequests)/?["']?(?:\s|$)"#).unwrap());
static POST_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:^|\s)(?:-d|--data\S*|-f|-F|--field|--raw-field|--input|-X\s*POST|--method\s+POST)(?:\s|=|$)").unwrap());
/// GitLab's: the board can't open a merge request there, so the agent still does.
static GITLAB_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bglab\s+mr\s+create\b").unwrap());
/// An MCP tool that opens a pull request (GitHub's, Bitbucket's, …).
static PR_TOOL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^mcp__.*(?:create_?pull_?request|create_pr$|pull_?request_?create)").unwrap());

fn opens_pr(tool: &str, input: &Value) -> bool {
    if tool != "Bash" {
        return PR_TOOL_RE.is_match(tool);
    }
    let cmd = input["command"].as_str().unwrap_or("");
    PR_CLI_RE.is_match(cmd) || (PR_API_RE.is_match(cmd) && POST_RE.is_match(cmd))
}

static PUSH_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bgit\b(?:\s+(?:-[Cc]\s+\S+|--?[\w-]+(?:=\S+)?))*\s+push\b").unwrap());

/// A git command that changes a repository: a commit, a push, a new worktree, or moving its checkout
/// to another branch or commit.
static GIT_WORK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\bgit\b(?:\s+(?:-[Cc]\s+\S+|--?[\w-]+(?:=\S+)?))*\s+(?:commit|push|worktree\s+add|checkout|switch|merge|rebase|reset|cherry-pick|revert)\b").unwrap()
});
static GIT_DIR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s-C\s+(\S+)").unwrap());
static CD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:^|&&|;|\|\||\()\s*(?:cd|pushd)\s+([^\s;&|)]+)").unwrap());

/// Where a tool call changes a repository: the file an edit names; for a git command that changes
/// one (`GIT_WORK_RE`) or a PR opening, the folder it runs in (`git -C <dir>`, else the last `cd <dir>`
/// or `pushd <dir>` before it, else the cwd). None for any other call.
fn work_place(tool: &str, input: &Value, cwd: &str) -> Option<PathBuf> {
    if EDIT_TOOLS.contains(&tool) {
        return input["file_path"].as_str().or(input["notebook_path"].as_str()).filter(|f| !f.is_empty()).map(|f| path_at(f, cwd));
    }
    if tool != "Bash" {
        return None;
    }
    let cmd = input["command"].as_str().unwrap_or("");
    let masked = unquoted(cmd);
    let git = GIT_WORK_RE.find(&masked);
    if git.is_none() && !opens_pr(tool, input) {
        return None;
    }
    Some(runs_in(cmd, git.or_else(|| PR_CLI_RE.find(&masked)), cwd))
}

/// A path as a command or tool names it (quoted, `~/…`, relative to `cwd`), made absolute.
fn path_at(p: &str, cwd: &str) -> PathBuf {
    let p = p.trim_matches(['"', '\'']);
    let p = match (p.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => p.to_string(),
    };
    let p = Path::new(&p);
    if p.is_absolute() { p.to_path_buf() } else { Path::new(cwd).join(p) }
}

/// The folder the part of `cmd` at `part` runs in: its `git -C <dir>`, else the last `cd <dir>` or
/// `pushd <dir>` before it, else the cwd. `part` is a match in `cmd` or in `unquoted(cmd)`.
fn runs_in(cmd: &str, part: Option<regex::Match>, cwd: &str) -> PathBuf {
    let masked = unquoted(cmd);
    let span = |r: std::ops::Range<usize>| cmd.get(r.clone()).unwrap_or(&masked[r]).to_string();
    if let Some(m) = part {
        if let Some(dir) = masked.get(m.range()).and_then(|t| GIT_DIR_RE.captures(t)).and_then(|c| c.get(1)) {
            return path_at(&span(m.start() + dir.start()..m.start() + dir.end()), cwd);
        }
    }
    let before = &masked[..part.map_or(masked.len(), |m| m.start())];
    CD_RE.captures_iter(before).last().and_then(|c| c.get(1)).map_or_else(|| PathBuf::from(cwd), |d| path_at(&span(d.range()), cwd))
}

/// The start of a quoted string a shell runs as a command: `bash -c "…"`, `sh -ec '…'`, `eval "…"`.
static RUNS_TEXT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:\b(?:sh|bash|zsh|dash|ksh)\s+(?:-\w+\s+)*-\w*c\s*|\beval\s+)$").unwrap());
static WORD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\S+").unwrap());

/// `cmd` with what's inside its quotes blanked to `_`, byte for byte, so a pattern matched on it finds
/// only the command's own words, not words in an argument (`tb question "… git commit …"`). A string
/// a shell runs (`bash -c "…"`) is left as it is. Every place in it is a place in `cmd`.
fn unquoted(cmd: &str) -> String {
    let b = cmd.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            q @ (b'\'' | b'"') => {
                let runs = cmd.get(..i).is_some_and(|before| RUNS_TEXT_RE.is_match(before));
                let mut j = i + 1;
                while j < b.len() && b[j] != q {
                    j += if q == b'"' && b[j] == b'\\' { 2 } else { 1 };
                }
                if !runs {
                    for x in out.iter_mut().take(j.min(b.len())).skip(i + 1) {
                        *x = b'_';
                    }
                }
                i = j + 1;
            }
            _ => i += 1,
        }
    }
    // Only bytes of quoted text were replaced, each with `_`, and a quote mark is a whole character.
    String::from_utf8(out).unwrap_or_else(|_| "_".repeat(cmd.len()))
}

/// The words of `text`, a part of `unquoted(cmd)` starting at `at`, each as `cmd` has it (quotes trimmed).
fn words_of<'a>(cmd: &'a str, text: &str, at: usize) -> Vec<&'a str> {
    WORD_RE.find_iter(text).map(|w| cmd.get(at + w.start()..at + w.end()).unwrap_or("").trim_matches(['"', '\''])).collect()
}

/// A folder's repository: its shared git folder (one for all of the repository's worktrees) and its
/// top folder. None outside a repository.
fn repo_of(place: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut dir = place;
    while !dir.is_dir() {
        dir = dir.parent()?;
    }
    let o = std::process::Command::new("git").arg("-C").arg(dir).args(["rev-parse", "--path-format=absolute", "--git-common-dir", "--show-toplevel"]).output().ok().filter(|o| o.status.success())?;
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    let mut lines = out.lines().map(|l| PathBuf::from(l.trim())).map(|p| p.canonicalize().unwrap_or(p));
    Some((lines.next()?, lines.next()?))
}

/// Why the board refuses work at `place` for this task, whose repository is `home`: the work is in
/// another repository, so the board would show the task as tracked while it happens beside it. A
/// repository that holds the task's (a home-folder dotfiles one) isn't another project's.
fn elsewhere(place: &Path, home: &Path, task: &str, project: &str) -> Option<(String, PathBuf)> {
    let (common, top) = repo_of(place)?;
    let (home_common, home_top) = repo_of(home)?;
    if common == home_common || home_top.starts_with(&top) {
        return None;
    }
    let shown = top.display();
    Some((
        format!(
            "{task} is {project}'s work, but this changes {shown}, another repository. The board doesn't track {task}'s work there, so it \
             refuses it: the task would show as in progress while its work happens beside the board. Keep {task}'s work in {project}. \
             Work in {shown} needs its own task in that repository's project: tb found it, or tb task new \"<title>\" --project <its project> \
             (no project for it yet? midna project add {shown}). If {task} itself was filed under the wrong project, move it and carry \
             on here: tb task set {task} --project <its project>. If {task} can't be done without it, tb fail \"<why>\"."
        ),
        top,
    ))
}

/// A `/steps` answer's task, the repository it works in, and its project's name.
fn home_of(v: &Value) -> Option<(&str, &str, &str)> {
    let task = v["task"].as_str()?;
    let vars = &v["vars"];
    let home = ["repo", "worktree"].iter().filter_map(|k| vars[*k].as_str()).find(|p| !p.is_empty())?;
    Some((task, home, vars["project"].as_str().unwrap_or("its project")))
}

/// `PreToolUse`: refuses an edit, a git commit, push or new worktree, or a PR opening in a repository
/// this terminal's task doesn't own, and tells the owner. No task, no board, or no repository: carry on.
fn elsewhere_refused(payload: &Value, session: &str, cfg: &Config, cwd: &str) -> bool {
    let tool = payload["tool_name"].as_str().unwrap_or("");
    let Some(place) = work_place(tool, &payload["tool_input"], cwd) else { return false };
    if repo_of(&place).is_none() {
        return false;
    }
    let refusal = judged(cfg, session, "", |v| {
        let (task, home, project) = home_of(v)?;
        let (why, top) = elsewhere(&place, Path::new(home), task, project)?;
        Some((why, top, task.to_string(), project.to_string()))
    });
    let Some((why, top, task, project)) = refusal else {
        if EDIT_TOOLS.contains(&tool) {
            if let Some((v, _)) = steps(cfg, session, "") {
                if let Some((task, _, _)) = home_of(&v) {
                    note_edit(&cfg.data, task, session, &place);
                }
            }
        }
        return false;
    };
    deny(&why);
    let text = format!("{task} tried to work in {}, a repository it doesn't own ({task} is {project}'s). The board refused it.", top.display());
    let _ = board_call(cfg, "POST", "/alerts", Some(&json!({"text": text, "key": format!("elsewhere:{task}"), "task": task})), hook_timeout());
    true
}

/// A git command that commits, or that moves or rewrites a checkout under everyone working in it:
/// `switch`, `checkout`, `rebase`, `stash` (its `list` and `show` only read) and `reset`.
static SHARED_GIT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\bgit\b(?:\s+(?:-[Cc]\s+\S+|--?[\w-]+(?:=\S+)?))*\s+(commit|switch|checkout|rebase|stash|reset)\b(\s+(?:list|show)\b)?").unwrap()
});
static GIT_ADD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bgit\b(?:\s+(?:-[Cc]\s+\S+|--?[\w-]+(?:=\S+)?))*\s+add\b([^;&|]*)").unwrap());
static COMMIT_ALL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(?:-[A-Za-z]*a[A-Za-z]*|--all)$").unwrap());

/// Where the hook keeps each task's edits: `edits/<task>`, one `<terminal>\t<file>` line per file it
/// edited. The board hears of them only at the turn's end; a commit in the same turn needs them now.
fn ledger_dir(data: &Path) -> PathBuf {
    data.join("edits")
}

fn note_edit(data: &Path, task: &str, session: &str, file: &Path) {
    if task.is_empty() || task.contains(['/', '.']) {
        return;
    }
    let line = format!("{session}\t{}", file.display());
    let path = ledger_dir(data).join(task);
    if std::fs::read_to_string(&path).is_ok_and(|t| t.lines().any(|l| l == line)) {
        return;
    }
    append_line(data, &path, &line);
}

/// `tb hook EditDone` (PostToolUse on an edit): appends the patch Claude made the file with to the
/// task's `hunks/<task>`, one `{"file", "patch"}` line per edit, so `tb slot claim --bring` can tell
/// its hunks from another task's in a file both edited. The patch is Claude's own `structuredPatch`
/// (only the changed lines and their context, however large the file); a new file and a notebook
/// have none, and leave nothing.
fn note_patch(data: &Path, task: &str, file: &Path, response: &Value) {
    let Some(patch) = response["structuredPatch"].as_array().filter(|p| !p.is_empty()) else { return };
    if task.is_empty() || task.contains(['/', '.']) {
        return;
    }
    let line = json!({"file": file.display().to_string(), "patch": patch}).to_string();
    append_line(data, &data.join("hunks").join(task), &line);
}

/// Appends `line` to a ledger file, making its folder first. A write that fails goes to `hook.log`.
fn append_line(data: &Path, path: &Path, line: &str) {
    let start = Instant::now();
    let wrote = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| {
        std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(format!("{line}\n").as_bytes())
    });
    if let Err(e) = wrote {
        log_failure(data, &format!("write {}", path.display()), &e.to_string(), start.elapsed());
    }
}

/// `EditDone`: records the edit's patch under this terminal's task. No task or no board: nothing.
fn edit_done(payload: &Value, session: &str) {
    let tool = payload["tool_name"].as_str().unwrap_or("");
    let cwd = payload["cwd"].as_str().unwrap_or("");
    let Some(file) = EDIT_TOOLS.contains(&tool).then(|| work_place(tool, &payload["tool_input"], cwd)).flatten() else { return };
    let cfg = client::config();
    let Some((v, _)) = steps(&cfg, session, "") else { return };
    if let Some(task) = v["task"].as_str() {
        note_patch(&cfg.data, task, &file, &payload["tool_response"]);
    }
}

/// One edit from the ledger.
#[derive(Debug, Clone, PartialEq)]
struct Edit {
    task: String,
    session: String,
    file: PathBuf,
}

fn ledger(data: &Path) -> Vec<Edit> {
    let Ok(dir) = std::fs::read_dir(ledger_dir(data)) else { return vec![] };
    let mut out = vec![];
    for e in dir.flatten() {
        let task = e.file_name().to_string_lossy().to_string();
        for l in std::fs::read_to_string(e.path()).unwrap_or_default().lines() {
            if let Some((session, file)) = l.split_once('\t') {
                out.push(Edit { task: task.clone(), session: session.to_string(), file: PathBuf::from(file) });
            }
        }
    }
    out
}

/// A file's path from the checkout's top folder, through the real path of the folder it's in.
fn rel_to(file: &Path, top: &Path) -> Option<String> {
    let real = file.parent()?.canonicalize().ok()?.join(file.file_name()?);
    real.strip_prefix(top).ok().map(|r| r.to_string_lossy().to_string()).filter(|r| !r.is_empty())
}

/// One uncommitted file in a checkout: whether it's staged, and whether git tracks it.
#[derive(Debug, Clone, PartialEq)]
struct Change {
    path: String,
    staged: bool,
    tracked: bool,
}

fn changes(top: &Path) -> Option<Vec<Change>> {
    let o = std::process::Command::new("git").arg("-C").arg(top).args(["status", "--porcelain=v1", "-z", "--untracked-files=all"]).output().ok().filter(|o| o.status.success())?;
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    let mut parts = out.split('\0');
    let mut list = vec![];
    while let Some(e) = parts.next() {
        if e.len() < 4 {
            continue;
        }
        let (x, y) = (e.as_bytes()[0], e.as_bytes()[1]);
        if matches!(x, b'R' | b'C') {
            parts.next();
        }
        list.push(Change { path: e[3..].to_string(), staged: !matches!(x, b' ' | b'?'), tracked: x != b'?' && y != b'?' });
    }
    Some(list)
}

/// A path from the checkout's top folder with `.` and `..` resolved by name.
fn tidy(p: &Path) -> String {
    let mut out: Vec<String> = vec![];
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Normal(s) => out.push(s.to_string_lossy().to_string()),
            _ => {}
        }
    }
    out.join("/")
}

/// The files the commit at `at` in `cmd` (run in `dir`) takes: what's staged, what its own `git add`s
/// before it add, and with `-a` every tracked change. Each says whether its working copy goes in whole
/// (added by the command) or only what's staged now.
fn committed(cmd: &str, at: usize, dir: &Path, top: &Path, list: &[Change]) -> Vec<(String, bool)> {
    let masked = unquoted(cmd);
    let rest = &masked[at..];
    let args = &rest[..rest.find([';', '&', '|']).unwrap_or(rest.len())];
    let all_tracked = args.split_whitespace().any(|a| COMMIT_ALL_RE.is_match(a));
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let mut covers: Vec<String> = vec![];
    let mut everything = false;
    let mut tracked = all_tracked;
    for c in GIT_ADD_RE.captures_iter(&masked[..at]) {
        let Some(args) = c.get(1) else { continue };
        for a in words_of(cmd, args.as_str(), args.start()) {
            match a {
                "-A" | "--all" => everything = true,
                "-u" | "--update" => tracked = true,
                "--" => {}
                a if a.starts_with('-') => {}
                a => covers.push(from_top(a, &dir, top)),
            }
        }
    }
    list.iter()
        .filter_map(|c| {
            let whole = everything || (tracked && c.tracked) || covered(&covers, &c.path);
            (whole || c.staged).then(|| (c.path.clone(), whole))
        })
        .collect()
}

/// A path a git command names, from the checkout's top folder.
fn from_top(a: &str, dir: &Path, top: &Path) -> String {
    dir.join(a).strip_prefix(top).map(tidy).unwrap_or_else(|_| tidy(Path::new(a)))
}

/// Whether `path` is one of `covers` or in one of its folders (`""` is the whole checkout).
fn covered(covers: &[String], path: &str) -> bool {
    covers.iter().any(|c| c.is_empty() || path == c || path.starts_with(&format!("{c}/")))
}

/// The files a switch, checkout, rebase, stash or reset at `at` in `cmd` carries off: only the paths it
/// names after `--` when it names some, every uncommitted file otherwise.
fn carried(cmd: &str, at: usize, dir: &Path, top: &Path, list: &[Change]) -> Vec<String> {
    let masked = unquoted(cmd);
    let rest = &masked[at..];
    let args = words_of(cmd, &rest[..rest.find([';', '&', '|']).unwrap_or(rest.len())], at);
    let all = list.iter().map(|c| c.path.clone());
    let Some(i) = args.iter().position(|a| *a == "--") else { return all.collect() };
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let covers: Vec<String> = args[i + 1..].iter().map(|a| from_top(a, &dir, top)).collect();
    if covers.is_empty() {
        return all.collect();
    }
    all.filter(|p| covered(&covers, p)).collect()
}

/// `git` in `top`, its output as text; None when it fails.
fn git_text(top: &Path, args: &[&str]) -> Option<String> {
    let o = std::process::Command::new("git").arg("-C").arg(top).args(args).output().ok().filter(|o| o.status.success())?;
    String::from_utf8(o.stdout).ok()
}

/// `git` in `top` with `input` on its stdin, its output as text; None when it fails.
fn git_in(top: &Path, args: &[&str], input: &[u8]) -> Option<String> {
    use std::process::Stdio;
    let mut child = std::process::Command::new("git").arg("-C").arg(top).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    child.stdin.take()?.write_all(input).ok()?;
    let o = child.wait_with_output().ok().filter(|o| o.status.success())?;
    String::from_utf8(o.stdout).ok()
}

/// A file's uncommitted hunks against HEAD in the checkout at `top`: HEAD's text, the working copy's
/// and the hunks. None for a file HEAD doesn't have, a binary one, or one that's gone.
struct Split {
    head: String,
    work: String,
    hunks: Vec<Hunk>,
}

fn split_of(top: &Path, rel: &str) -> Option<Split> {
    let head = git_text(top, &["show", &format!("HEAD:{rel}")])?;
    let diff = git_text(top, &["diff", "--no-ext-diff", "--no-color", "--no-textconv", "-U0", "HEAD", "--", rel])?;
    if diff.lines().any(|l| l.starts_with("Binary files")) {
        return None;
    }
    let work = std::fs::read_to_string(top.join(rel)).ok()?;
    let hunks = hunks::parse(rel, &hunks::lines(&head), &hunks::lines(&work), &diff);
    Some(Split { head, work, hunks })
}

/// Where `task`'s recorded edits (`hunks/<task>`) sit in `rel`'s working copy `work`; None when its
/// log has none for the file.
fn marks_of(data: &Path, task: &str, top: &Path, rel: &str, work: &[&str]) -> Option<Marks> {
    let log = std::fs::read_to_string(data.join("hunks").join(task)).unwrap_or_default();
    Marks::of(&log, &top.join(rel).to_string_lossy(), work)
}

/// Whether two line ranges of HEAD, as (first line's index, count), touch: a range of none (an
/// insertion) touches the lines on either side of it.
fn overlaps(a: (usize, usize), b: (usize, usize)) -> bool {
    let (a_end, b_end) = (a.0 + a.1, b.0 + b.1);
    if a.1 == 0 || b.1 == 0 {
        a.0 <= b_end && b.0 <= a_end
    } else {
        a.0 < b_end && b.0 < a_end
    }
}

/// Whether a commit of `rel`, a file `other` edited too, takes hunks `other` made: of the working
/// copy when the command stages it `whole`, else of what's staged now. A file whose hunks can't be
/// told apart (HEAD hasn't it, it's binary, `other`'s edits weren't logged by patch) takes them.
fn takes_hunks_of(data: &Path, top: &Path, rel: &str, other: &str, whole: bool) -> bool {
    let Some(s) = split_of(top, rel) else { return true };
    let work = hunks::lines(&s.work);
    let Some(marks) = marks_of(data, other, top, rel, &work) else { return true };
    let theirs: Vec<&Hunk> = s.hunks.iter().filter(|h| marks.made(h)).collect();
    if theirs.is_empty() {
        return false;
    }
    if whole {
        return true;
    }
    // A staged deletion takes everything.
    let Some(index) = git_text(top, &["show", &format!(":{rel}")]) else { return true };
    let Some(diff) = git_text(top, &["diff", "--cached", "--no-ext-diff", "--no-color", "--no-textconv", "-U0", "--", rel]) else { return true };
    let staged = hunks::parse(rel, &hunks::lines(&s.head), &hunks::lines(&index), &diff);
    staged.iter().any(|x| theirs.iter().any(|t| t.id == x.id || overlaps(t.old, x.old)))
}

/// The checkouts git keeps inside the one at `top` (the board's worktree slots in the main checkout),
/// as paths from `top`.
fn nested(top: &Path) -> Vec<String> {
    let list = git_text(top, &["worktree", "list", "--porcelain"]).unwrap_or_default();
    list.lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(|p| PathBuf::from(p).canonicalize().unwrap_or_else(|_| PathBuf::from(p)))
        .filter_map(|p| p.strip_prefix(top).ok().map(|r| r.to_string_lossy().to_string()))
        .filter(|r| !r.is_empty())
        .collect()
}

/// A recorded file's path from `top`, when it's in that checkout and not in one `nested` in it.
fn here_rel(file: &Path, top: &Path, nested: &[String]) -> Option<String> {
    let rel = rel_to(file, top).or_else(|| file.strip_prefix(top).ok().map(|r| r.to_string_lossy().to_string()).filter(|r| !r.is_empty()))?;
    (!covered(nested, &rel)).then_some(rel)
}

/// Rewrites a ledger file with only the lines `keep` passes, and removes it once none are left.
fn rewrite(path: &Path, mut keep: impl FnMut(&str) -> bool) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let kept: Vec<&str> = text.lines().filter(|l| keep(l)).collect();
    if kept.len() == text.lines().count() {
        return;
    }
    if kept.is_empty() {
        let _ = std::fs::remove_file(path);
    } else {
        let _ = std::fs::write(path, kept.iter().map(|l| format!("{l}\n")).collect::<String>());
    }
}

/// Drops `task`'s records (`edits/<task>` and `hunks/<task>`) of the files in the checkout at `top`
/// that `gone` passes (by path from `top`; none in a checkout `nested` in it). The files it dropped.
fn forget(data: &Path, task: &str, top: &Path, nested: &[String], gone: &dyn Fn(&str) -> bool) -> Vec<String> {
    if task.is_empty() || task.contains(['/', '.']) {
        return vec![];
    }
    let mut dropped = vec![];
    let mut keep = |file: &str| match here_rel(Path::new(file), top, nested).filter(|r| gone(r)) {
        Some(rel) => {
            dropped.push(rel);
            false
        }
        None => true,
    };
    rewrite(&ledger_dir(data).join(task), |l| l.split_once('\t').map_or(true, |(_, f)| keep(f)));
    rewrite(&data.join("hunks").join(task), |l| serde_json::from_str::<Value>(l).ok().and_then(|v| v["file"].as_str().map(str::to_string)).map_or(true, |f| keep(&f)));
    dropped.sort();
    dropped.dedup();
    dropped
}

/// Every task's records of files in the checkout at `top` that have nothing uncommitted there (not in
/// `list`): their edits are committed or undone, so they no longer stand in anyone's way.
fn forget_committed(data: &Path, top: &Path, list: &[Change]) {
    let mut tasks: Vec<String> = [ledger_dir(data), data.join("hunks")]
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect::<Vec<_>>())
        .collect();
    if tasks.is_empty() {
        return;
    }
    tasks.sort();
    tasks.dedup();
    let nested = nested(top);
    for t in &tasks {
        forget(data, t, top, &nested, &|rel| !list.iter().any(|c| c.path == rel));
    }
}

/// Whether `other`'s recorded edits of `rel`, uncommitted in the checkout at `top`, are done with
/// there: none of its uncommitted hunks is one `other` made (only whitespace or a lone `}` is no
/// one's), or every hunk `other` made is already in HEAD or in one of the refs `pushed` names for it
/// (its pushed branch, the default branch). A file whose hunks can't be told apart isn't.
fn settled(data: &Path, top: &Path, rel: &str, other: &str, pushed: &dyn Fn(&str, &Path) -> Vec<String>) -> bool {
    let Some(s) = split_of(top, rel) else { return false };
    let work = hunks::lines(&s.work);
    let Some(marks) = marks_of(data, other, top, rel, &work) else { return false };
    if !s.hunks.iter().any(|h| marks.made(h)) {
        return true;
    }
    let log = std::fs::read_to_string(data.join("hunks").join(other)).unwrap_or_default();
    let file = top.join(rel).to_string_lossy().to_string();
    hunks::all_in(&log, &file, &s.head) || pushed(other, top).iter().any(|r| git_text(top, &["show", &format!("{r}:{rel}")]).is_some_and(|t| hunks::all_in(&log, &file, &t)))
}

/// What a git command in a checkout would do to other tasks' uncommitted edits there: its verb, the
/// checkout's top folder and, by the task that made them (and its terminal), the files it would take
/// (a commit) or carry off (a switch, checkout, rebase, stash or reset). None for any other command, or
/// when it touches no one else's edits. A file `own` (this terminal's task) edited too it may commit
/// while none of the other task's hunks of it go in. Records that no longer stand for uncommitted work
/// there (`forget_committed`, `settled`) are dropped on the way.
#[cfg(test)]
fn shared_conflicts(cmd: &str, cwd: &str, data: &Path, own: Option<&str>) -> Option<(String, PathBuf, Vec<(String, String, Vec<String>)>)> {
    shared_conflicts_with(cmd, cwd, data, own, &|_, _| vec![])
}

/// `shared_conflicts`, with `pushed` naming the refs a task's work counts as saved in.
fn shared_conflicts_with(cmd: &str, cwd: &str, data: &Path, own: Option<&str>, pushed: &dyn Fn(&str, &Path) -> Vec<String>) -> Option<(String, PathBuf, Vec<(String, String, Vec<String>)>)> {
    let masked = unquoted(cmd);
    let c = SHARED_GIT_RE.captures(&masked)?;
    if c.get(2).is_some() {
        return None;
    }
    let (m, verb) = (c.get(0)?, c[1].to_string());
    let dir = runs_in(cmd, Some(m), cwd);
    let (_, top) = repo_of(&dir)?;
    if ledger(data).is_empty() {
        return None;
    }
    let list = changes(&top)?;
    forget_committed(data, &top, &list);
    let edits = ledger(data);
    let mine: Vec<String> = edits.iter().filter(|e| Some(e.task.as_str()) == own).filter_map(|e| rel_to(&e.file, &top)).collect();
    let taken = if verb == "commit" { committed(cmd, m.end(), &dir, &top, &list) } else { carried(cmd, m.end(), &dir, &top, &list).into_iter().map(|p| (p, true)).collect() };
    let mut by: Vec<(String, String, Vec<String>)> = vec![];
    let mut done: Vec<(String, String)> = vec![];
    for e in edits.iter().filter(|e| Some(e.task.as_str()) != own) {
        let Some(rel) = rel_to(&e.file, &top) else { continue };
        let Some(&(_, whole)) = taken.iter().find(|(p, _)| *p == rel) else { continue };
        if by.iter().any(|(t, _, files)| *t == e.task && files.contains(&rel)) || done.iter().any(|(t, f)| *t == e.task && *f == rel) {
            continue;
        }
        if settled(data, &top, &rel, &e.task, pushed) {
            done.push((e.task.clone(), rel));
            continue;
        }
        if verb == "commit" && mine.contains(&rel) && !takes_hunks_of(data, &top, &rel, &e.task, whole) {
            continue;
        }
        match by.iter_mut().find(|(t, _, _)| *t == e.task) {
            Some((_, _, files)) => files.push(rel),
            None => by.push((e.task.clone(), e.session.clone(), vec![rel])),
        }
    }
    if !done.is_empty() {
        let nested = nested(&top);
        for (task, rel) in &done {
            forget(data, task, &top, &nested, &|r| r == rel);
        }
    }
    by.sort();
    (!by.is_empty()).then_some((verb, top, by))
}

/// The refusal for a git command that would take or carry off other tasks' uncommitted edits.
fn shared_refusal(own: Option<&str>, verb: &str, top: &Path, theirs: &[(String, String, Vec<String>)]) -> String {
    let who = own.unwrap_or("This terminal");
    let named = theirs.iter().map(|(t, _, files)| format!("{t} ({})", files.join(", "))).collect::<Vec<_>>().join("; ");
    let tasks = theirs.iter().map(|(t, _, _)| t.as_str()).collect::<Vec<_>>().join(" and ");
    if verb == "commit" {
        return format!(
            "{who} can't commit files another task edited in {} and hasn't committed: {named}. They're {tasks}'s work, not {who}'s. \
             Unstage them (git restore --staged <file>) and commit only the files {who} edited, by name (git add <file> … && git commit). \
             In a file {who} edited too, only {who}'s own hunks can go in: tb stage <file> stages just those, from {who}'s recorded edits; \
             then git commit, without -a or a git add of that file. Or move {who}'s own edits into a worktree slot and work there: tb slot claim --bring <its files>. \
             A record left by a task with no terminal here and nothing unpushed, in a slot {who} holds: tb edits clear <task>.",
            top.display()
        );
    }
    format!(
        "{who} can't git {verb} in {}: {tasks} works in this checkout too and hasn't committed its edits there yet: {named}. A git {verb} \
         would carry them to another branch or throw them away. Leave the checkout's branch alone: commit {who}'s own files by name \
         (git add <file> … && git commit), or move {who}'s own edits into a worktree slot and work there: tb slot claim --bring <its files>. \
         A record left by a task with no terminal here and nothing unpushed, in a slot {who} holds: tb edits clear <task>.",
        top.display()
    )
}

/// The top folder of the checkout `dir` is in. None outside a repository.
pub(crate) fn checkout_top(dir: &Path) -> Option<PathBuf> {
    repo_of(dir).map(|(_, top)| top)
}

/// `tb edits clear`: drops every record `task` has of files in the checkout at `top` (none in a
/// checkout nested in it), and says which files they were.
pub(crate) fn clear_edits(data: &Path, task: &str, top: &Path) -> String {
    let dropped = forget(data, task, top, &nested(top), &|_| true);
    if dropped.is_empty() {
        return format!("{task} has no recorded edits in {}.", top.display());
    }
    format!("Cleared {task}'s recorded edits in {}: {}.", top.display(), dropped.join(", "))
}

/// `tb stage`: stages only `own`'s hunks of `files` (named from `cwd`; none named, every uncommitted
/// file it edited in this checkout), found from its recorded edits, so a commit in a checkout other
/// tasks share takes none of theirs. A file no other task edited goes in whole. A hunk another task
/// made too, or a shared file whose hunks can't be told apart, is refused with nothing staged. What
/// it staged and what it left out.
pub(crate) fn stage_own(data: &Path, own: &str, cwd: &Path, files: &[String]) -> Result<String, String> {
    let Some((_, top)) = repo_of(cwd) else { return Err(format!("{} isn't in a git checkout.", cwd.display())) };
    let list = changes(&top).ok_or_else(|| format!("git status failed in {}.", top.display()))?;
    let edits = ledger(data);
    let rel_of = |e: &Edit| rel_to(&e.file, &top);
    let targets: Vec<String> = if files.is_empty() {
        let mut mine: Vec<String> = edits.iter().filter(|e| e.task == own).filter_map(rel_of).filter(|r| list.iter().any(|c| c.path == *r)).collect();
        mine.sort();
        mine.dedup();
        mine
    } else {
        let dir = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        let named: Vec<String> = files.iter().map(|f| from_top(f, &dir, &top)).collect();
        let idle: Vec<&String> = named.iter().filter(|r| !list.iter().any(|c| c.path == **r)).collect();
        if !idle.is_empty() {
            return Err(format!("{own} can't stage {}: nothing in {} is uncommitted there. Nothing was staged.", idle.iter().map(|r| r.as_str()).collect::<Vec<_>>().join(", "), top.display()));
        }
        named
    };
    if targets.is_empty() {
        return Err(format!("{own} has no uncommitted edits in {} to stage.", top.display()));
    }
    let (mut whole, mut parts, mut clash, mut left, mut none) = (vec![], vec![], vec![], vec![], vec![]);
    for f in &targets {
        let mut others: Vec<&str> = edits.iter().filter(|e| e.task != own && rel_of(e).as_deref() == Some(f)).map(|e| e.task.as_str()).collect();
        others.sort();
        others.dedup();
        if others.is_empty() {
            whole.push(f.as_str());
            continue;
        }
        let names = others.join(" and ");
        let Some(s) = split_of(&top, f) else {
            clash.push(format!("{f} ({names} edits it too, and only a text file HEAD has can be split by hunk)"));
            continue;
        };
        let work = hunks::lines(&s.work);
        let mine = marks_of(data, own, &top, f, &work).unwrap_or_default();
        let mut theirs = vec![];
        for o in &others {
            match marks_of(data, o, &top, f, &work) {
                Some(m) => theirs.push((*o, m)),
                None => clash.push(format!("{f} ({o} edits it too, and its edits can't be told apart from {own}'s)")),
            }
        }
        let mut take = vec![];
        for h in &s.hunks {
            let by: Vec<&str> = theirs.iter().filter(|(_, m)| m.made(h)).map(|(t, _)| *t).collect();
            match (mine.made(h), by.is_empty()) {
                (true, true) => take.push(h.id.clone()),
                (true, false) => clash.push(format!("{f} ({} changed the same lines as {own}, at line {})", by.join(" and "), h.line())),
                (false, true) => left.push(format!("{f} line {}", h.line())),
                (false, false) => {}
            }
        }
        if take.is_empty() {
            none.push(f.as_str());
            continue;
        }
        let content = hunks::build(&hunks::lines(&s.head), &work, &s.hunks, |h| take.contains(&h.id));
        parts.push((f.as_str(), content, take.len(), s.hunks.len()));
    }
    if !clash.is_empty() {
        return Err(format!("{own} can't stage {}. Nothing was staged.", clash.join(", ")));
    }
    let failed = |f: &str| format!("Staging {f} in {} failed.", top.display());
    for (f, content, _, _) in &parts {
        let mode = git_text(&top, &["ls-tree", "HEAD", "--", f]).and_then(|t| t.split_whitespace().next().map(str::to_string)).unwrap_or_else(|| "100644".into());
        let blob = git_in(&top, &["hash-object", "-w", "--stdin"], content.as_bytes()).ok_or_else(|| failed(f))?;
        git_text(&top, &["update-index", "--add", "--cacheinfo", &format!("{mode},{},{f}", blob.trim())]).ok_or_else(|| failed(f))?;
    }
    if !whole.is_empty() {
        let mut args = vec!["add", "-A", "--"];
        args.extend(whole.iter());
        git_text(&top, &args).ok_or_else(|| failed(&whole.join(", ")))?;
    }
    let mut lines = vec![];
    if !parts.is_empty() {
        let each = parts.iter().map(|(f, _, n, of)| format!("{f} ({n} of {of} hunks)")).collect::<Vec<_>>().join(", ");
        lines.push(format!("Staged only {own}'s hunks of {each}; the other tasks' stay unstaged."));
    }
    if !whole.is_empty() {
        lines.push(format!("Staged whole (no other task edits them): {}.", whole.join(", ")));
    }
    if !none.is_empty() {
        lines.push(format!("Nothing of {own}'s to stage in {}.", none.join(", ")));
    }
    if !left.is_empty() {
        lines.push(format!("Left unstaged, in no task's edit log (shell edits?): {}. Stage one by hand with git add -p if it's {own}'s.", left.join(", ")));
    }
    if !parts.is_empty() || !whole.is_empty() {
        lines.push("Commit with git commit, without -a or a git add of these files.".into());
    }
    Ok(lines.join("\n"))
}

/// The refs a task's work counts as saved in, for `settled`: its branch on the remote and the
/// remote's default branch. Asked of the board; no answer, only the default branch.
fn pushed_refs(cfg: &Config, task: &str, top: &Path) -> Vec<String> {
    let remote = &cfg.pr_body.remote;
    let branch = board_call(cfg, "GET", &format!("/steps?task={task}"), None, hook_timeout()).ok().and_then(|v| v["vars"]["branch"].as_str().map(str::to_string)).filter(|b| !b.is_empty());
    branch.into_iter().chain(default_branch(top, remote)).map(|b| format!("{remote}/{b}")).collect()
}

/// The task holding the board's worktree slot at `top`. None: not a slot, free, or no board.
fn slot_holder(cfg: &Config, top: &Path) -> Option<String> {
    let v = board_call(cfg, "GET", &format!("/slots/at?path={}", crate::tb::qv(&top.to_string_lossy())), None, hook_timeout()).ok()?;
    v["held_by"].as_str().map(str::to_string)
}

/// Whether the `/steps` answer `v` for a terminal has it working in the checkout at `top`.
fn works_in(v: &Value, top: &Path) -> bool {
    v["vars"]["worktree"].as_str().filter(|w| !w.is_empty()).is_some_and(|w| Path::new(w).canonicalize().is_ok_and(|w| w == top))
}

/// `PreToolUse`: in a checkout tasks share, refuses a commit that takes files another task still on its
/// terminal edited (and this one didn't), and a switch, checkout, rebase, stash or reset while such a
/// task has uncommitted edits there. In a worktree slot this task holds, only a task whose terminal
/// works in that slot now counts (a stopped rebase or merge there isn't held up by a record left from
/// before). No other task's edits, no board: carry on.
fn shared_checkout_refused(payload: &Value, session: &str, cfg: &Config, cwd: &str) -> bool {
    if payload["tool_name"] != "Bash" {
        return false;
    }
    let cmd = payload["tool_input"]["command"].as_str().unwrap_or("");
    if !SHARED_GIT_RE.is_match(&unquoted(cmd)) || ledger(&cfg.data).is_empty() {
        return false;
    }
    let Some((v, _)) = steps(cfg, session, "") else { return false };
    let own = v["task"].as_str();
    let pushed = |task: &str, top: &Path| pushed_refs(cfg, task, top);
    let Some((verb, top, mut theirs)) = shared_conflicts_with(cmd, cwd, &cfg.data, own, &pushed) else { return false };
    // Only a task still on its terminal is working there; a finished one's ledger is done with.
    theirs.retain(|(task, terminal, _)| {
        // Asked of the board itself: a terminal it doesn't hear back from loses its ledger.
        let on = steps_live(cfg, terminal, "").is_some_and(|v| v["task"].as_str() == Some(task.as_str()));
        if !on {
            let _ = std::fs::remove_file(ledger_dir(&cfg.data).join(task));
        }
        on
    });
    if !theirs.is_empty() && own.is_some() && slot_holder(cfg, &top).as_deref() == own {
        theirs.retain(|(task, terminal, _)| {
            let here = steps_live(cfg, terminal, "").is_some_and(|v| works_in(&v, &top));
            if !here {
                forget(&cfg.data, task, &top, &nested(&top), &|_| true);
            }
            here
        });
    }
    if theirs.is_empty() {
        return false;
    }
    deny(&shared_refusal(own, &verb, &top, &theirs));
    true
}

/// `git worktree add` and the rest of its command, up to the next `;`, `&` or `|`.
static WORKTREE_ADD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bgit\b(?:\s+(?:-[Cc]\s+\S+|--?[\w-]+(?:=\S+)?))*\s+worktree\s+add\b([^;&|]*)").unwrap());

/// The folder a `git worktree add` makes, as its arguments name it: the first that isn't an option or
/// an option's value.
fn worktree_target(args: &str) -> Option<&str> {
    let mut words = args.split_whitespace();
    while let Some(w) = words.next() {
        match w {
            "-b" | "-B" | "--reason" => {
                words.next();
            }
            _ if w.starts_with('-') => {}
            _ => return Some(w),
        }
    }
    None
}

/// A path with its links resolved, or for one that doesn't exist yet its `.` and `..` worked out.
fn resolved(p: &Path) -> PathBuf {
    if let Ok(real) = p.canonicalize() {
        return real;
    }
    let mut out = PathBuf::new();
    for part in p.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// What a tool call does that makes or enters a worktree outside the board's slot pool: a `git worktree
/// add`, an `EnterWorktree`, or a subagent with `isolation: worktree`. A pool slot
/// (`…/.claude/worktrees/slot-<n>`) and the task's own worktree (`own`, one from before the pool) are fine.
/// None for any other call.
fn stray_worktree(tool: &str, input: &Value, cwd: &str, own: Option<&str>) -> Option<String> {
    let allowed = |p: &Path| {
        let p = resolved(p);
        taskboardd::slots::is_slot(&p.to_string_lossy()) || own.filter(|o| !o.is_empty()).is_some_and(|o| p.starts_with(resolved(Path::new(o))))
    };
    match tool {
        "Bash" => {
            let cmd = input["command"].as_str().unwrap_or("");
            WORKTREE_ADD_RE.captures_iter(cmd).find_map(|c| {
                let m = c.get(0)?;
                let Some(target) = worktree_target(&c[1]) else { return Some("git worktree add".to_string()) };
                let at = path_at(target, &runs_in(cmd, Some(m), cwd).to_string_lossy());
                (!allowed(&at)).then(|| format!("git worktree add {}", resolved(&at).display()))
            })
        }
        "EnterWorktree" => match input["path"].as_str().filter(|p| !p.is_empty()) {
            Some(p) => (!allowed(&path_at(p, cwd))).then(|| format!("EnterWorktree into {p}")),
            None => Some("EnterWorktree making a new worktree".to_string()),
        },
        _ if SUBAGENT_TOOLS.contains(&tool) && input["isolation"] == "worktree" => Some("a subagent with isolation: worktree".to_string()),
        _ => None,
    }
}

/// The refusal for `task` making or entering a worktree outside the slot pool (`what`).
fn stray_refusal(task: &str, what: &str, tb: &str) -> String {
    format!(
        "{task} can't make or enter a worktree of its own ({what}): every worktree a task uses comes from the board's slot pool, \
         so they're reused and cleaned up instead of piling up on the disk. Claim one with {tb} slot claim --bring <files>: it gives \
         {task} a free slot (its own again when it had one), moves {task}'s uncommitted edits in those files there from this \
         checkout (on its own, --bring takes the files {task} edited) and prints its path; work there (cd <path>, or EnterWorktree \
         with that path). Subagents run without isolation: worktree; give each its own files instead."
    )
}

/// `PreToolUse`: refuses a board task's `git worktree add`, `EnterWorktree` or worktree subagent when
/// the worktree isn't a pool slot (or the task's own), and points to `tb slot claim`. No task, no board:
/// carry on.
fn stray_worktree_refused(payload: &Value, session: &str, cfg: &Config, cwd: &str) -> bool {
    let tool = payload["tool_name"].as_str().unwrap_or("");
    let input = &payload["tool_input"];
    if stray_worktree(tool, input, cwd, None).is_none() {
        return false;
    }
    let Some((task, what)) = judged(cfg, session, "", |v| Some((v["task"].as_str()?.to_string(), stray_worktree(tool, input, cwd, v["vars"]["worktree"].as_str())?))) else { return false };
    deny(&stray_refusal(&task, &what, &tb_path()));
    true
}

/// `git commit` or `git push` and the rest of its command, up to the next `;`, `&` or `|`.
static COMMIT_PUSH_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bgit\b(?:\s+(?:-[Cc]\s+\S+|--?[\w-]+(?:=\S+)?))*\s+(commit|push)\b([^;&|]*)").unwrap());
static SWITCH_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bgit\b(?:\s+(?:-[Cc]\s+\S+|--?[\w-]+(?:=\S+)?))*\s+(?:switch|checkout)\b").unwrap());

fn git_out(dir: &Path, args: &[&str]) -> Option<String> {
    let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().ok().filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

/// The default branch of the repository at `dir`, by its `remote`: the remote's HEAD, else main or
/// master, whichever the repository has. None without that remote: nothing is pushed anywhere.
fn default_branch(dir: &Path, remote: &str) -> Option<String> {
    git_out(dir, &["remote", "get-url", remote])?;
    if let Some(head) = git_out(dir, &["symbolic-ref", "--short", "-q", &format!("refs/remotes/{remote}/HEAD")]) {
        return Some(head.strip_prefix(&format!("{remote}/")).unwrap_or(&head).to_string());
    }
    ["main", "master"].into_iter().find(|b| git_out(dir, &["rev-parse", "--verify", "-q", &format!("refs/heads/{b}")]).is_some()).map(String::from)
}

/// Whether `git push <args>`, from a checkout on `current`, lands on `default`: a refspec whose
/// destination is it (`main`, `HEAD:main`, `+fix:refs/heads/main`, `:main`), `--all` or `--mirror`, or
/// no refspec while the checkout is on it. `--tags` alone pushes only tags.
fn push_lands_on(args: &str, current: Option<&str>, default: &str) -> bool {
    let (mut named, mut tags) = (vec![], false);
    let mut words = args.split_whitespace();
    while let Some(w) = words.next() {
        match w {
            "--all" | "--branches" | "--mirror" => return true,
            "--tags" => tags = true,
            "-o" | "--push-option" | "--repo" | "--receive-pack" | "--exec" => {
                words.next();
            }
            _ if w.starts_with('-') => {}
            _ => named.push(w.trim_matches(['"', '\''])),
        }
    }
    let refspecs = named.get(1..).unwrap_or_default();
    if refspecs.is_empty() {
        return !tags && current == Some(default);
    }
    refspecs.iter().any(|r| {
        let r = r.trim_start_matches('+');
        let to = r.rsplit_once(':').map_or(r, |(_, to)| to);
        let to = to.strip_prefix("refs/heads/").unwrap_or(to);
        to == default || (to == "HEAD" && current == Some(default))
    })
}

/// What a command does to its repository's default branch (`remote`'s), with the repository's top
/// folder and that branch: "commit on" while the checkout is on it, "push to" when a push lands on it.
/// After a switch or checkout earlier in the command, the checkout's branch is unknown.
fn onto_default(cmd: &str, cwd: &str, remote: &str) -> Option<(&'static str, PathBuf, String)> {
    let masked = unquoted(cmd);
    COMMIT_PUSH_RE.captures_iter(&masked).find_map(|c| {
        let m = c.get(0)?;
        let dir = runs_in(cmd, Some(m), cwd);
        let (_, top) = repo_of(&dir)?;
        let default = default_branch(&top, remote)?;
        let moved = SWITCH_RE.is_match(&masked[..m.start()]);
        let current = git_out(&dir, &["symbolic-ref", "--short", "-q", "HEAD"]).filter(|_| !moved);
        let args = c.get(2).map_or("", |a| cmd.get(a.range()).unwrap_or(a.as_str()));
        match &c[1] {
            "commit" => (current.as_deref() == Some(default.as_str())).then_some("commit on"),
            _ => push_lands_on(args, current.as_deref(), &default).then_some("push to"),
        }
        .map(|what| (what, top, default))
    })
}

/// `PreToolUse`: with `[default_branch] guard` on (the default), refuses a commit on a repository's
/// default branch and a push that lands on it, so work gets there through PRs. Read locally, no board needed.
fn default_branch_refused(payload: &Value, cfg: &Config, cwd: &str) -> bool {
    if !cfg.default_branch.guard || payload["tool_name"] != "Bash" {
        return false;
    }
    let cmd = payload["tool_input"]["command"].as_str().unwrap_or("");
    let Some((what, top, branch)) = onto_default(cmd, cwd, &cfg.pr_body.remote) else { return false };
    deny(&format!(
        "Task board: agents don't {what} {branch}, {}'s default branch: work lands there through PRs. Commit on a branch of its own \
         (git switch -c <branch>; uncommitted edits come along), push that, and open the PR (on a task: tb submit --pr-body <file>). \
         This is {}'s setting ([default_branch] in the board's config.toml); leave it to them.",
        top.display(),
        cfg.owner
    ));
    true
}

/// The head of the task's own checkout (`{worktree}` and `{branch}` in a `GET /steps` answer): its
/// branch's tip there, else the checkout's head.
fn own_head(vars: &Value) -> Option<String> {
    let dir = vars["worktree"].as_str().filter(|d| !d.is_empty() && Path::new(d).is_dir())?;
    let rev = |r: &str| {
        let o = std::process::Command::new("git").args(["-C", dir, "rev-parse", "--verify", "-q", &format!("{r}^{{commit}}")]).output().ok().filter(|o| o.status.success())?;
        Some(String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|h| !h.is_empty())
    };
    vars["branch"].as_str().filter(|b| !b.is_empty()).and_then(|b| rev(&format!("refs/heads/{b}"))).or_else(|| rev("HEAD"))
}

/// The refusal for opening a PR from the terminal of a task (its `GET /steps` listing) that ends in one:
/// the board opens that PR itself. None when the task ends without a PR.
fn board_opens_pr(steps: &Value, tb: &str) -> Option<String> {
    if steps["ships_pr"] != true {
        return None;
    }
    let task = steps["task"].as_str().unwrap_or("This task");
    Some(format!(
        "{task} ends in a PR, and the board opens it: write the description to a file and finish with \
         {tb} submit \"<summary>\" --pr-body <file>. It checks the description and that the branch is pushed and \
         rebased, then opens the PR. ({tb} submit --pr <link> is only for a PR that was already open before the task.)"
    ))
}

/// `PreToolUse`: refuses a command that opens a PR on a task that ends in one (the board opens it, at
/// `tb submit --pr-body`; a GitLab merge request is still the agent's to open), holds one on any other
/// task while its steps for before the PR
/// (config.toml's `[[steps]]`) aren't recorded, and a push to an open PR while a per-head step hasn't
/// passed on the commit being pushed, and work in a repository the task doesn't own (`elsewhere_refused`).
/// Anything else, no task, or no board: carry on. True when it refused the call.
fn pre_tool_use(payload: &Value, session: &str) -> bool {
    let tool = payload["tool_name"].as_str().unwrap_or("");
    let cfg = client::config();
    let cwd = payload["cwd"].as_str().filter(|c| !c.is_empty()).map(|c| c.to_string()).or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string()));
    if EDIT_TOOLS.contains(&tool) {
        // The comment guard (config.toml's [comments]): read locally, no board needed.
        if let Some(reason) = comments::edit_refusal(&cfg, tool, &payload["tool_input"]) {
            deny(&reason);
            return true;
        }
        return elsewhere_refused(payload, session, &cfg, cwd.as_deref().unwrap_or(""));
    }
    let here = cwd.as_deref().unwrap_or("");
    if default_branch_refused(payload, &cfg, here) {
        return true;
    }
    if elsewhere_refused(payload, session, &cfg, here) || stray_worktree_refused(payload, session, &cfg, here) || shared_checkout_refused(payload, session, &cfg, here) {
        return true;
    }
    let opening = opens_pr(tool, &payload["tool_input"]);
    let pushing = tool == "Bash" && PUSH_RE.is_match(payload["tool_input"]["command"].as_str().unwrap_or(""));
    if !opening && !pushing {
        return false;
    }
    if opening {
        if let Some(reason) = cwd.as_deref().and_then(|d| comments::branch_refusal(&cfg, d, "opening the PR")) {
            deny(&reason);
            return true;
        }
    }
    let what = if opening { "opening the PR" } else { "pushing to the open PR" };
    // Whether the steps are done is the board's to say now, not a kept answer's.
    let Some(mut v) = steps_live(&cfg, session, "") else { return false };
    if opening && !GITLAB_RE.is_match(payload["tool_input"]["command"].as_str().unwrap_or("")) {
        if let Some(reason) = board_opens_pr(&v, &tb_path()) {
            deny(&reason);
            return true;
        }
    }
    if let Some(why) = v["aim_outside"].as_str() {
        deny(&format!("Not yet: {what} waits for the task's steps, and they can't be judged. {why}"));
        return true;
    }
    // Per-head steps are judged on the task's own checkout, not on whichever folder this call runs in.
    if let Some(head) = own_head(&v["vars"]).filter(|h| v["head"].as_str() != Some(h.as_str())) {
        match steps_live(&cfg, session, &format!("&head={head}")) {
            Some(at) => v = at,
            None => return false,
        }
    }
    let vars = serde_json::from_value(v["vars"].clone()).unwrap_or_default();
    let left: Vec<Step> = from_listing(&v)
        .into_iter()
        .filter(|(s, done)| s.before == Before::Pr && !done && (opening || (s.per_head && v["pr_open"] == true)))
        .map(|(s, _)| s.filled(&vars))
        .collect();
    if left.is_empty() {
        return false;
    }
    deny(&taskboardd::steps::refusal(&tb_path(), what, &left));
    true
}

/// `PreToolUse`: refuses a tool call that reads the daemon's app token (`taskboardd::apptoken`), which
/// is what makes a request the owner's click in the app.
fn token_refused(payload: &Value) -> bool {
    let tool = payload["tool_name"].as_str().unwrap_or("");
    let cwd = payload["cwd"].as_str().filter(|c| !c.is_empty()).map(std::path::Path::new);
    if !taskboardd::apptoken::tool_reaches(tool, &payload["tool_input"], &client::config().data, cwd) {
        return false;
    }
    deny(&taskboardd::apptoken::refusal());
    true
}

const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];
const SUBAGENT_TOOLS: &[&str] = &["Agent", "Task"];
/// Tool calls that can change files without naming them: the board stamps the checkout around each
/// one, so a no-task Stop counts only what changed while the agent's own calls ran.
const WINDOW_TOOLS: &[&str] = &["Bash", "Agent", "Task"];

/// The id Claude gives a tool call, which its PreToolUse and PostToolUse both carry; failing that, one
/// made from the call itself, which the board can match to the transcript's copy of the call.
fn tool_call_id(payload: &Value) -> String {
    if let Some(id) = payload["tool_use_id"].as_str().filter(|id| !id.is_empty()) {
        return id.to_string();
    }
    taskboardd::transcript::call_key(payload["tool_name"].as_str().unwrap_or(""), &payload["tool_input"])
}

/// Whether the hooks.json Claude loaded for this plugin sends `ToolEnd`, so each window this hook
/// opens is closed: the plugin's folder is its installed version's, the one Claude read.
fn hooks_send_tool_end(root: Option<&std::path::Path>) -> bool {
    let Some(text) = root.and_then(|r| std::fs::read_to_string(r.join("hooks").join("hooks.json")).ok()) else { return false };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return false };
    v["hooks"]["PostToolUse"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|m| m["hooks"].as_array().into_iter().flatten())
        .any(|h| h["args"].as_array().is_some_and(|a| a.iter().any(|x| x == "ToolEnd")) || h["command"].as_str().is_some_and(|c| c.contains("ToolEnd")))
}

/// The report that opens (`hook.tool_start`) or closes (`hook.tool_end`) a tool call's window on the
/// board, with the checkout's stamp at that moment. None for a call that can't change files unnamed.
fn tool_window_body(payload: &Value, session: &str, event: &str) -> Option<Value> {
    let tool = payload["tool_name"].as_str().unwrap_or("");
    if !WINDOW_TOOLS.contains(&tool) {
        return None;
    }
    let cwd = payload["cwd"].as_str().filter(|c| !c.is_empty()).map(|c| c.to_string()).or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string())).unwrap_or_default();
    let mut body = client::base_body(event, session, payload["session_id"].as_str().unwrap_or(""), &cwd, Value::Null);
    body["tool"] = json!(tool);
    body["tool_use_id"] = json!(tool_call_id(payload));
    body["background"] = json!(payload["tool_input"]["run_in_background"] == true);
    if event == "hook.tool_start" {
        body["ends"] = json!(hooks_send_tool_end(plugin_root().as_deref()));
    }
    body["tree"] = client::tree_stamp(&cwd, GIT_TIMEOUT);
    Some(body)
}

/// Sends a tool call's window in the background. One the board doesn't hear about only leaves the
/// Stop to its older guess, so it isn't kept for later.
fn tool_window(payload: &Value, session: &str, event: &str) {
    let Some(body) = tool_window_body(payload, session, event) else { return };
    send_later(&client::config(), &body, false);
}

fn deny(reason: &str) {
    let o = json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": reason}});
    let mut so = std::io::stdout();
    let _ = so.write_all(o.to_string().as_bytes());
    let _ = so.flush();
}

/// `PreToolUse` on Bash: runs the command with its input from /dev/null, so a stray `cat > file`, a
/// `read` or an interactive prompt meets the end of its input and fails at once instead of waiting
/// for typing that never comes, which holds the turn (and its task) open for good. Heredocs and pipes
/// in the command still feed what they name. Only the input changes: Claude Code still asks for the
/// call as it would have.
fn close_stdin(payload: &Value) {
    if payload["tool_name"] != "Bash" {
        return;
    }
    let Some(command) = payload["tool_input"]["command"].as_str().and_then(stdin_closed) else { return };
    let mut input = payload["tool_input"].clone();
    input["command"] = json!(command);
    let o = json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "updatedInput": input}});
    let mut so = std::io::stdout();
    let _ = so.write_all(o.to_string().as_bytes());
    let _ = so.flush();
}

const STDIN_CLOSED_END: &str = "\n} < /dev/null";

/// `command` in a group reading from /dev/null, or None when it's empty or grouped already. The
/// closing brace goes on its own line, after a heredoc's end word or a trailing comment.
fn stdin_closed(command: &str) -> Option<String> {
    let command = command.trim_end();
    if command.trim().is_empty() || (command.starts_with("{ ") && command.ends_with(STDIN_CLOSED_END)) {
        return None;
    }
    Some(format!("{{ {command}{STDIN_CLOSED_END}"))
}

static GIT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bgit\b(?:\s+(?:-[Cc]\s+\S+|--?[\w-]+(?:=\S+)?))*\s+(commit|push)\b").unwrap());

fn board_event(hook: &str) -> Option<&'static str> {
    Some(match hook {
        "SessionStart" => "hook.session_start",
        "UserPromptSubmit" => "hook.prompt",
        "Stop" => "hook.stop",
        "StopFailure" => "hook.api_error",
        "PreCompact" => "hook.pre_compact",
        "PostToolUse" => "hook.commit",
        "SessionEnd" => "hook.session_end",
        "Notification" => "hook.attention",
        // Claude's question picker (`AskUserQuestion`) opens, and closes.
        "AskUser" => "hook.ask",
        "AskUserDone" => "hook.ask_done",
        _ => return None,
    })
}

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// The last `n` characters of `s`.
fn clip_end(s: &str, n: usize) -> String {
    let c: Vec<char> = s.chars().collect();
    c[c.len().saturating_sub(n)..].iter().collect()
}

fn clip_middle(s: &str, n: usize) -> String {
    let c: Vec<char> = s.chars().collect();
    if c.len() <= n {
        return s.to_string();
    }
    let head = n / 2;
    let tail = n - head - 3;
    format!("{}\n…\n{}", c[..head].iter().collect::<String>(), c[c.len() - tail..].iter().collect::<String>())
}

fn tool_output(resp: &Value) -> String {
    match resp {
        Value::Object(o) => [o.get("stdout"), o.get("stderr")]
            .iter()
            .filter_map(|x| x.and_then(|v| v.as_str()))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

/// `hook.log` in the board's data folder: a line for each board call or ledger write a hook couldn't
/// make. The hooks carry on past those so they never get in Claude's way; this says what was dropped.
const HOOK_LOG: &str = "hook.log";
/// Past this many bytes the log moves to `hook.log.1` (dropping the one before) and starts over.
const HOOK_LOG_CAP: u64 = 1024 * 1024;

/// The terminal and hook event this run is for, set once by `run`, for `hook.log`'s lines.
static RUN: OnceLock<(String, String)> = OnceLock::new();

/// `client::request`, with a failure written to `hook.log`.
fn board_call(cfg: &Config, method: &str, path: &str, body: Option<&Value>, timeout: f64) -> Result<Value, CallError> {
    let start = Instant::now();
    let r = client::request(cfg, method, path, body, timeout);
    if let Err(e) = &r {
        let why = match e {
            CallError::Refused(m) => format!("refused: {m}"),
            CallError::Unreachable(m) => format!("unreachable: {m}"),
            CallError::TimedOut(m) => format!("timed out: {m}"),
        };
        // The query holds the terminal, which the line names already.
        log_failure(&cfg.data, &format!("{method} {}", path.split('?').next().unwrap_or(path)), &why, start.elapsed());
    }
    r
}

/// Appends a failure to `hook.log`. Logging is never why a hook fails: a line that can't be written
/// is dropped.
fn log_failure(data: &Path, endpoint: &str, error: &str, elapsed: Duration) {
    let (session, event) = RUN.get().map_or(("", ""), |(s, e)| (s.as_str(), e.as_str()));
    let at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    append_capped(&data.join(HOOK_LOG), &failure_line(&at, session, event, endpoint, error, elapsed), HOOK_LOG_CAP);
}

/// One `hook.log` line: when, the terminal, the hook event, what failed, how long it took, and why,
/// on one line however the error reads.
fn failure_line(at: &str, session: &str, event: &str, endpoint: &str, error: &str, elapsed: Duration) -> String {
    let or_dash = |s: &str| if s.is_empty() { "-".to_string() } else { s.to_string() };
    let error = error.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{at} session={} event={} {endpoint} failed after {}ms: {error}\n", or_dash(session), or_dash(event), elapsed.as_millis())
}

/// Appends `line` to `path`, first moving a file past `cap` bytes to `<path>.1`.
fn append_capped(path: &Path, line: &str, cap: u64) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() >= cap) {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        let _ = std::fs::rename(path, old);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

pub fn hook_timeout() -> f64 {
    std::env::var("TASKBOARD_HOOK_TIMEOUT").ok().and_then(|v| v.parse::<f64>().ok()).filter(|v| *v > 0.0).unwrap_or(1.0)
}

fn plugin_root() -> Option<std::path::PathBuf> {
    std::env::var("CLAUDE_PLUGIN_ROOT").ok().map(std::path::PathBuf::from).filter(|p| p.is_dir())
}

fn tb_path() -> String {
    if let Some(r) = plugin_root() {
        return r.join("bin").join("tb").to_string_lossy().to_string();
    }
    std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "tb".into())
}

/// When Taskboard has a GitHub or Bitbucket account, has this session's git ask `tb git-credential`
/// first (through Claude Code's `CLAUDE_ENV_FILE`), so an agent on a task pushes with that account.
/// Only this session's environment changes; `~/.gitconfig` doesn't.
fn git_env(cfg: &Config, tb: &str) {
    let Some(file) = std::env::var_os("CLAUDE_ENV_FILE").filter(|f| !f.is_empty()) else { return };
    if !taskboardd::accounts::has_git_account(cfg) {
        return;
    }
    let Some(env) = crate::gitcred::session_env(tb) else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = f.write_all(env.as_bytes());
    }
}

fn plugin_version() -> String {
    plugin_root()
        .and_then(|r| std::fs::read_to_string(r.join(".claude-plugin").join("plugin.json")).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["version"].as_str().map(|s| s.to_string()))
        .unwrap_or_default()
}

fn hello_marker(cfg: &Config) -> std::path::PathBuf {
    cfg.data.join("plugin-hello")
}

fn say_hello(cfg: &Config, base: &Value, version: &str, path: &str, left: f64) {
    let want = format!("{version}\n{path}");
    if left <= 0.05 || std::fs::read_to_string(hello_marker(cfg)).map(|t| t == want).unwrap_or(false) {
        return;
    }
    let mut hello = base.clone();
    hello["event"] = json!("tb.hello");
    hello["tb_path"] = json!(path);
    hello["plugin_version"] = json!(version);
    if board_call(cfg, "POST", "/report", Some(&hello), left).is_ok() {
        let _ = std::fs::create_dir_all(&cfg.data);
        let tmp = hello_marker(cfg).with_extension(format!("{}.tmp", std::process::id()));
        if std::fs::write(&tmp, want).is_ok() {
            let _ = std::fs::rename(&tmp, hello_marker(cfg));
        }
    }
}

/// A `claude -p` (or Agent SDK) run: Claude Code marks it with an `sdk-*` entrypoint. One an agent
/// starts in its terminal inherits `MIDNA_SESSION`, but it isn't that terminal's conversation.
fn headless(entrypoint: Option<&str>) -> bool {
    entrypoint.is_some_and(|e| e.starts_with("sdk"))
}

pub fn run(event_arg: Option<&str>) -> i32 {
    let start = Instant::now();
    // A headless run reports nothing for the terminal it ran in; it only meets the token guard.
    let session = if headless(std::env::var("CLAUDE_CODE_ENTRYPOINT").ok().as_deref()) { String::new() } else { std::env::var("MIDNA_SESSION").unwrap_or_default() };
    // The app token guard runs in every terminal, Midna's or not; everything else only in Midna's.
    if session.is_empty() && event_arg != Some("PreToolUse") {
        return 0;
    }
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let payload: Value = serde_json::from_str(&raw).ok().filter(|v: &Value| v.is_object()).unwrap_or(json!({}));
    let hook = event_arg.map(|s| s.to_string()).filter(|s| !s.is_empty()).or_else(|| payload["hook_event_name"].as_str().map(|s| s.to_string())).unwrap_or_default();
    let _ = RUN.set((session.clone(), hook.clone()));
    if hook == "PreToolUse" {
        let tool = payload["tool_name"].as_str().unwrap_or("");
        // A subagent's prompt isn't a read: its own tool calls meet the guard.
        if !SUBAGENT_TOOLS.contains(&tool) && token_refused(&payload) {
            return 0;
        }
        if session.is_empty() {
            close_stdin(&payload);
            return 0;
        }
        if !pre_tool_use(&payload, &session) {
            close_stdin(&payload);
            tool_window(&payload, &session, "hook.tool_start");
        }
        return 0;
    }
    if session.is_empty() {
        return 0;
    }
    if hook == "ToolEnd" {
        // `tb hook ToolEnd`, from PostToolUse and PostToolUseFailure: the call's window closes.
        tool_window(&payload, &session, "hook.tool_end");
        return 0;
    }
    if hook == "EditDone" {
        edit_done(&payload, &session);
        return 0;
    }
    let Some(event) = board_event(&hook) else { return 0 };
    let s = |k: &str| payload[k].as_str().unwrap_or("").to_string();
    let mut extra = serde_json::Map::new();
    match hook.as_str() {
        "PostToolUse" => {
            if payload["tool_name"] != "Bash" {
                return 0;
            }
            let command = payload["tool_input"]["command"].as_str().unwrap_or("");
            if !GIT_RE.is_match(command) {
                return 0;
            }
            extra.insert("command".into(), json!(clip(command, 2000)));
            extra.insert("output".into(), json!(clip_middle(&tool_output(&payload["tool_response"]), MAX_OUTPUT)));
        }
        "Notification" => {
            let kind = s("notification_type");
            if !WAITING_NOTIFICATIONS.contains(&kind.as_str()) {
                return 0;
            }
            extra.insert("message".into(), json!(s("message")));
            extra.insert("notification_type".into(), json!(kind));
            extra.insert("transcript_path".into(), json!(s("transcript_path")));
            if !s("title").is_empty() {
                extra.insert("title".into(), json!(s("title")));
            }
        }
        "AskUser" => {
            extra.insert("tool_input".into(), payload["tool_input"].clone());
        }
        "SessionEnd" => {
            let reason = if s("reason").is_empty() { "other".to_string() } else { s("reason") };
            if CONTINUING_END_REASONS.contains(&reason.as_str()) {
                return 0;
            }
            extra.insert("reason".into(), json!(reason));
        }
        "UserPromptSubmit" => {
            // The prompt, and that its only cut is the board's own: a "…" at its end is the owner's.
            if let Value::Object(m) = taskboardd::startword::hook_prompt(&s("prompt")) {
                extra.extend(m);
            }
        }
        "Stop" => {
            extra.insert("last_message".into(), json!(clip(&s("last_assistant_message"), MAX_LAST_MESSAGE)));
            // Its end too, where the agent asks whether to go on ("Want me to run G4?").
            extra.insert("last_message_end".into(), json!(clip_end(&s("last_assistant_message"), taskboardd::startword::REPLY_END_KEEP)));
            extra.insert("transcript_path".into(), json!(s("transcript_path")));
            extra.insert("stop_hook_active".into(), json!(payload["stop_hook_active"] == true));
        }
        "StopFailure" => {
            extra.insert("error".into(), json!(s("error")));
            extra.insert("error_details".into(), json!(clip(&s("error_details"), MAX_OUTPUT)));
            extra.insert("last_message".into(), json!(clip(&s("last_assistant_message"), MAX_OUTPUT)));
        }
        "PreCompact" => {
            extra.insert("trigger".into(), json!(s("trigger")));
        }
        _ => {}
    }
    let cfg = client::config();
    if CONTEXT_EVENTS.contains(&hook.as_str()) || hook == "Stop" {
        // A new turn, or its end: what the board said of this terminal's steps may have moved on.
        forget_steps(&cfg.data, &session);
    }
    let timeout = hook_timeout();
    let deadline = start + Duration::from_secs_f64(timeout + SLACK);
    let left = || deadline.saturating_duration_since(Instant::now()).as_secs_f64().min(timeout);
    let cwd = if s("cwd").is_empty() { std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default() } else { s("cwd") };
    let (version, path) = (plugin_version(), tb_path());
    if hook == "SessionStart" {
        extra.insert("source".into(), json!(s("source")));
        extra.insert("tb_path".into(), json!(path));
        extra.insert("plugin_version".into(), json!(version));
    }
    if hook == "UserPromptSubmit" || hook == "Stop" {
        extra.insert("tree".into(), client::tree_stamp(&cwd, GIT_TIMEOUT));
    }
    if hook == "UserPromptSubmit" {
        // This hook stamps each Bash and subagent call (`tool_window`), so the board needn't guess.
        extra.insert("tool_windows".into(), json!(true));
    }
    if let Some(dir) = extra.get("command").and_then(|c| c.as_str()).and_then(|c| taskboardd::reports::git_place(c, &cwd)) {
        extra.insert("work_git".into(), client::git_info(&dir, GIT_TIMEOUT));
    }
    let git = client::git_info(&cwd, GIT_TIMEOUT);
    let base = client::base_body(event, &session, &s("session_id"), &cwd, git);
    let mut body = base.clone();
    for (k, v) in extra {
        body[k] = v;
    }
    if !CONTEXT_EVENTS.contains(&hook.as_str()) && hook != "Stop" {
        // Nothing comes back to Claude from this one: the board takes it when it can.
        send_later(&cfg, &body, true);
        return 0;
    }
    let post_timeout = left();
    let resp = if post_timeout > 0.05 {
        match board_call(&cfg, "POST", "/report", Some(&body), post_timeout) {
            Ok(v) => Some(v),
            Err(CallError::Refused(..)) => Some(json!({})),
            Err(CallError::Unreachable(_) | CallError::TimedOut(_)) => None,
        }
    } else {
        None
    };
    let Some(resp) = resp else {
        spool(&cfg, &body);
        return 0;
    };
    if hook == "SessionStart" {
        say_hello(&cfg, &base, &version, &path, left());
        git_env(&cfg, &path);
    }
    let context = resp["context"].as_str().filter(|c| !c.trim().is_empty()).unwrap_or("").to_string();
    let deliver = &resp["deliver"];
    let ids: Vec<Value> = if CONTEXT_EVENTS.contains(&hook.as_str()) || hook == "Stop" {
        deliver["ids"].as_array().cloned().unwrap_or_default()
    } else {
        vec![]
    };
    let mut message = if ids.is_empty() { String::new() } else { deliver["text"].as_str().unwrap_or("").to_string() };
    if hook == "Stop" && message.is_empty() {
        // The board's own reason to keep the turn going (code changed with no task to track it).
        message = resp["block"].as_str().map(|b| b.trim().to_string()).unwrap_or_default();
    }
    let out = if hook == "Stop" && !message.is_empty() {
        Some(json!({"decision": "block", "reason": clip(&message, MAX_CONTEXT)}))
    } else if CONTEXT_EVENTS.contains(&hook.as_str()) && (!context.is_empty() || !message.is_empty()) {
        let both = match (message.is_empty(), context.is_empty()) {
            (false, false) => format!("{message}\n\n{context}"),
            (false, true) => message.clone(),
            _ => context.clone(),
        };
        Some(json!({"hookSpecificOutput": {"hookEventName": hook, "additionalContext": clip(&both, MAX_CONTEXT)}}))
    } else {
        None
    };
    if let Some(o) = out {
        let mut so = std::io::stdout();
        let _ = so.write_all(o.to_string().as_bytes());
        let _ = so.flush();
    }
    if !ids.is_empty() {
        let mut ack = base.clone();
        ack["event"] = json!("hook.delivered");
        ack["ids"] = Value::Array(ids);
        send_later(&cfg, &ack, true);
    }
    0
}

/// Keeps a report the board didn't get for it to read later. One that can't be kept is lost, and
/// `hook.log` says so.
fn spool(cfg: &Config, body: &Value) {
    let start = Instant::now();
    if let Err(e) = client::spool_write(cfg, body) {
        log_failure(&cfg.data, "spool", &e.to_string(), start.elapsed());
    }
}

/// How long a report sent in the background waits for the board: nothing waits on it.
const SEND_TIMEOUT: f64 = 10.0;

/// Sends a report that needs no answer without waiting for it: spooled first, marked as being sent,
/// then posted by a detached `tb hook Send`. With `keep`, one the board can't take is left in the
/// spool for it to read later; without (a tool call's window, which the board ignores once late),
/// it's dropped.
fn send_later(cfg: &Config, body: &Value, keep: bool) {
    use std::os::unix::process::CommandExt;
    let start = Instant::now();
    let path = match client::spool_claimed(cfg, body) {
        Ok(p) => p,
        Err(e) => return log_failure(&cfg.data, "spool", &e.to_string(), start.elapsed()),
    };
    let spawned = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .args(["hook", "Send"])
            .arg(&path)
            .args((!keep).then_some("--drop"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // Its own group: Claude ending the hook's doesn't end the send.
            .process_group(0)
            .spawn()
    });
    if let Err(e) = spawned {
        log_failure(&cfg.data, "send", &e.to_string(), start.elapsed());
        unclaim(&path, keep);
    }
}

/// Hands a report being sent back to the spool reader (`keep`), or drops it.
fn unclaim(path: &Path, keep: bool) {
    if !keep || std::fs::rename(path, path.with_extension("")).is_err() {
        let _ = std::fs::remove_file(path);
    }
}

/// `tb hook Send <file> [--drop]`: posts a report `send_later` spooled, for the board to queue.
/// Taken or refused, it's done with; the board unreachable or slow, it goes back to the spool
/// (`--drop`: it's dropped). Only a file in the spool marked as being sent is touched.
pub fn send(file: Option<&str>, drop: bool) -> i32 {
    if let Some(path) = file {
        send_file(&client::config(), Path::new(path), drop);
    }
    0
}

fn send_file(cfg: &Config, path: &Path, drop: bool) {
    let ours = path.parent() == Some(cfg.spool_dir().as_path()) && path.to_string_lossy().ends_with(taskboardd::reports::SENDING);
    if !ours {
        return;
    }
    let Some(body) = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else {
        let _ = std::fs::remove_file(path);
        return;
    };
    let _ = RUN.set((body["session"].as_str().unwrap_or("").to_string(), format!("Send {}", body["event"].as_str().unwrap_or(""))));
    match board_call(cfg, "POST", "/report?queue=1", Some(&body), SEND_TIMEOUT) {
        Ok(_) | Err(CallError::Refused(_)) => {
            let _ = std::fs::remove_file(path);
        }
        Err(CallError::Unreachable(_) | CallError::TimedOut(_)) => unclaim(path, !drop),
    }
}

/// How long a `/steps` answer this terminal had stands in for asking again. PreToolUse asks for
/// nearly every call, and a board busy elsewhere shouldn't hold each one; a refusal is never made on
/// a kept answer alone (`judged`), and `tb` commands and new turns drop it (`forget_steps`).
const STEPS_FRESH: Duration = Duration::from_secs(15);
/// How old a kept answer can be and still stand in when the board can't answer at all.
const STEPS_STALE: Duration = Duration::from_secs(120);

/// The answers this hook has had, and whether each was kept rather than the board's: one call's
/// checks ask once.
static STEPS_HAD: Lazy<std::sync::Mutex<std::collections::HashMap<String, (Value, bool)>>> = Lazy::new(Default::default);

fn steps_dir(data: &Path) -> PathBuf {
    data.join("hook-cache")
}

/// A name that doesn't change between builds (FNV-1a), unlike std's hasher.
fn stable_hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// `more` is the rest of the query after the terminal (`&head=…`).
fn steps_file(data: &Path, session: &str, more: &str) -> PathBuf {
    steps_dir(data).join(format!("{:016x}-{:016x}.json", stable_hash(session), stable_hash(more)))
}

fn kept_steps(data: &Path, session: &str, more: &str, max_age: Duration) -> Option<Value> {
    let p = steps_file(data, session, more);
    let age = std::fs::metadata(&p).ok()?.modified().ok()?.elapsed().unwrap_or_default();
    if age > max_age {
        return None;
    }
    serde_json::from_str(&std::fs::read_to_string(&p).ok()?).ok()
}

/// Keeps the board's answer for the next hook, and clears out answers too old to stand in.
fn keep_steps(data: &Path, session: &str, more: &str, v: &Value) {
    let dir = steps_dir(data);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let p = steps_file(data, session, more);
    let tmp = p.with_extension(format!("{}.tmp", std::process::id()));
    if std::fs::write(&tmp, v.to_string()).is_err() || std::fs::rename(&tmp, &p).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        if e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > STEPS_STALE * 5) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Drops the `/steps` answers kept for this terminal, so the next hook asks the board: after a `tb`
/// command that may change them, and at each new turn.
pub fn forget_steps(data: &Path, session: &str) {
    if let Ok(mut had) = STEPS_HAD.lock() {
        had.retain(|k, _| k.split('\n').next() != Some(session));
    }
    let start = format!("{:016x}-", stable_hash(session));
    for e in std::fs::read_dir(steps_dir(data)).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with(&start) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// This terminal's `/steps` answer, and whether it's a kept one: one this hook had, one kept in the
/// last `STEPS_FRESH`, the board's, or, when the board can't answer, one kept in the last
/// `STEPS_STALE`. None: no answer at all.
fn steps(cfg: &Config, session: &str, more: &str) -> Option<(Value, bool)> {
    let key = format!("{session}\n{more}");
    if let Some(had) = STEPS_HAD.lock().ok().and_then(|h| h.get(&key).cloned()) {
        return Some(had);
    }
    if let Some(v) = kept_steps(&cfg.data, session, more, STEPS_FRESH) {
        if let Ok(mut had) = STEPS_HAD.lock() {
            had.insert(key, (v.clone(), true));
        }
        return Some((v, true));
    }
    if let Some(v) = steps_live(cfg, session, more) {
        return Some((v, false));
    }
    kept_steps(&cfg.data, session, more, STEPS_STALE).map(|v| (v, true))
}

/// The board's own `/steps` answer for this terminal (kept for the next hook). None: no answer.
fn steps_live(cfg: &Config, session: &str, more: &str) -> Option<Value> {
    let key = format!("{session}\n{more}");
    if let Some((v, false)) = STEPS_HAD.lock().ok().and_then(|h| h.get(&key).cloned()) {
        return Some(v);
    }
    let v = board_call(cfg, "GET", &format!("/steps?session={session}{more}"), None, hook_timeout()).ok()?;
    keep_steps(&cfg.data, session, more, &v);
    if let Ok(mut had) = STEPS_HAD.lock() {
        had.insert(key, (v.clone(), false));
    }
    Some(v)
}

/// What `judge` makes of this terminal's `/steps` answer: Some is a refusal. One made on a kept answer
/// is made again on the board's own, so keeping answers only ever lets a call through sooner. No
/// answer: None.
fn judged<T>(cfg: &Config, session: &str, more: &str, judge: impl Fn(&Value) -> Option<T>) -> Option<T> {
    let (v, kept) = steps(cfg, session, more)?;
    let verdict = judge(&v);
    if verdict.is_none() || !kept {
        return verdict;
    }
    match steps_live(cfg, session, more) {
        Some(v) => judge(&v),
        None => verdict,
    }
}

fn status_text(v: &Value) -> String {
    let mut parts = vec![];
    if let Some(m) = v["model"]["display_name"].as_str() {
        parts.push(m.to_string());
    }
    if let Some(d) = v["workspace"]["current_dir"].as_str().or(v["cwd"].as_str()) {
        parts.push(d.trim_end_matches('/').rsplit('/').next().unwrap_or(d).to_string());
    }
    if let Some(p) = v["rate_limits"]["five_hour"]["used_percentage"].as_f64() {
        parts.push(format!("5h {}%", p.round() as i64));
    }
    parts.join(" · ")
}

/// `tb statusline`: saves Claude Code's status-line input (with its rate limits) for the board and prints
/// a short status line; with `--pass` it prints the input unchanged, to chain into another command.
pub fn statusline(pass: bool) -> i32 {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let parsed = serde_json::from_str::<Value>(&raw);
    let mut so = std::io::stdout();
    let shown = if pass { raw.clone() } else { parsed.as_ref().map(status_text).unwrap_or_default() };
    let _ = so.write_all(shown.as_bytes());
    let _ = so.flush();
    let Ok(v) = parsed else { return 0 };
    let sid = v["session_id"].as_str().unwrap_or("").to_string();
    let name = std::env::var("MIDNA_SESSION").ok().filter(|s| !s.is_empty()).unwrap_or(sid);
    if name.is_empty() || name.contains('/') {
        return 0;
    }
    let cfg = client::config();
    let _ = save_private(&cfg.statusline_dir, &name, &raw);
    0
}

/// Writes one session's status JSON to `<dir>/<name>.json`. It holds the session's folder, cost and rate
/// limits, so the folder is kept 0700 and the file 0600.
fn save_private(dir: &std::path::Path, name: &str, raw: &str) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let path = dir.join(format!("{name}.json"));
    let tmp = dir.join(format!("{name}.json.{}.tmp", std::process::id()));
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(raw.as_bytes())?;
    drop(f);
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_commands_read_from_dev_null() {
        // stdin is a pipe held open, as Claude Code's is: without the group, the bare `cat` would wait on it forever.
        let sh = |c: &str| {
            use std::process::{Command, Stdio};
            let mut child = Command::new("sh").arg("-c").arg(stdin_closed(c).unwrap()).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
            let _held = child.stdin.take();
            let mut out = String::new();
            child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
            child.wait().unwrap();
            out
        };
        assert_eq!(sh("cat > /dev/null; echo after # a comment"), "after\n");
        assert_eq!(sh("cat <<'EOF'\nheredoc\nEOF"), "heredoc\n");
        assert_eq!(sh("printf 'piped' | cat"), "piped");
        let once = stdin_closed("echo hi\n").unwrap();
        assert_eq!(once, "{ echo hi\n} < /dev/null");
        assert_eq!(stdin_closed(&once), None, "grouped already");
        assert_eq!(stdin_closed("  "), None);
    }

    #[test]
    fn pr_openings_are_recognised() {
        let bash = |c: &str| opens_pr("Bash", &json!({"command": c}));
        assert!(bash("gh pr create --fill"));
        assert!(bash("cd x && gh pr create -t T -b B"));
        assert!(bash("glab mr create"));
        assert!(bash("tb api bitbucket repositories/acme/web/pullrequests -d '{\"title\":\"x\"}'"));
        assert!(bash("gh api repos/acme/web/pulls -f title=x -f head=b -f base=main"));
        assert!(!bash("gh pr view 9"));
        assert!(!bash("gh api repos/acme/web/pulls"));
        assert!(!bash("tb api bitbucket repositories/acme/web/pullrequests/9/comments -d '{}'"));
        assert!(opens_pr("mcp__github__create_pull_request", &json!({})));
        assert!(!opens_pr("mcp__github__get_pull_request", &json!({})));
    }

    #[test]
    fn the_place_a_call_works_is_its_file_or_the_folder_its_git_runs_in() {
        let bash = |c: &str| work_place("Bash", &json!({"command": c}), "/w/a");
        assert_eq!(work_place("Edit", &json!({"file_path": "/w/b/src/x.rs"}), "/w/a"), Some(PathBuf::from("/w/b/src/x.rs")));
        assert_eq!(work_place("Write", &json!({"file_path": "notes.md"}), "/w/a"), Some(PathBuf::from("/w/a/notes.md")));
        assert_eq!(bash("git commit -m x"), Some(PathBuf::from("/w/a")));
        assert_eq!(bash("git -C /w/b commit -m x"), Some(PathBuf::from("/w/b")));
        assert_eq!(bash("cd /w/b && git worktree add ../b-fix -b fix"), Some(PathBuf::from("/w/b")));
        assert_eq!(bash("cd ../b && git add . && git commit -m x"), Some(PathBuf::from("/w/a/../b")));
        assert_eq!(bash("cd \"/w/b\" && gh pr create --fill"), Some(PathBuf::from("/w/b")));
        assert_eq!(bash("git status && git log"), None, "reading changes nothing");
        assert_eq!(bash("git worktree list"), None);
        assert_eq!(bash("git -C /w/b checkout -b fix"), Some(PathBuf::from("/w/b")));
        assert_eq!(bash("pushd /w/b && git switch main"), Some(PathBuf::from("/w/b")));
        assert_eq!(bash("cd /w/b; git rebase origin/main"), Some(PathBuf::from("/w/b")));
        assert_eq!(bash("git branch --show-current && git diff"), None);
        assert_eq!(work_place("Read", &json!({"file_path": "/w/b/x"}), "/w/a"), None);
    }

    fn git_repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        let ok = std::process::Command::new("git").arg("-C").arg(dir).args(["init", "-q"]).status().unwrap().success();
        assert!(ok);
    }

    #[test]
    fn the_gate_judges_the_tip_of_the_tasks_branch_in_its_worktree() {
        let root = std::env::temp_dir().join(format!("tb-own-head-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        git_repo(&root);
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git").arg("-C").arg(&root).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        git(&["commit", "-q", "--allow-empty", "-m", "one"]);
        let one = git(&["rev-parse", "HEAD"]);
        git(&["branch", "fix"]);
        git(&["commit", "-q", "--allow-empty", "-m", "two"]);
        let two = git(&["rev-parse", "HEAD"]);
        let dir = root.to_string_lossy();
        assert_eq!(own_head(&json!({"worktree": dir, "branch": "fix"})), Some(one), "the task's branch, not the checkout's HEAD");
        assert_eq!(own_head(&json!({"worktree": dir, "branch": ""})), Some(two.clone()));
        assert_eq!(own_head(&json!({"worktree": dir, "branch": "gone"})), Some(two));
        assert_eq!(own_head(&json!({"worktree": root.join("missing").to_string_lossy(), "branch": "fix"})), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn work_in_a_repository_the_task_does_not_own_is_refused() {
        let root = std::env::temp_dir().join(format!("tb-elsewhere-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (a, b) = (root.join("a"), root.join("b"));
        git_repo(&a);
        git_repo(&b);
        let (why, top) = elsewhere(&b.join("src/x.rs"), &a, "T115", "a").expect("another repository");
        assert_eq!(top, b.canonicalize().unwrap());
        assert!(why.contains("T115 is a's work") && why.contains("midna project add"), "{why}");
    assert!(why.contains("tb task set T115 --project <its project>"), "a task filed under the wrong project can move: {why}");
        assert!(elsewhere(&a.join("src/x.rs"), &a, "T115", "a").is_none(), "the task's own repository");
        assert!(elsewhere(&root.join("loose.txt"), &a, "T115", "a").is_none(), "outside any repository");

        // The task's own worktree shares its repository.
        let commit = |d: &Path, args: &[&str]| std::process::Command::new("git").arg("-C").arg(d).args(args).output().unwrap();
        commit(&a, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "x"]);
        commit(&a, &["worktree", "add", "-q", root.join("a-fix").to_str().unwrap()]);
        assert!(elsewhere(&root.join("a-fix/src/x.rs"), &a, "T115", "a").is_none());

        // A repository that holds the task's (a dotfiles one in the home folder) isn't another project's.
        git_repo(&root);
        assert!(elsewhere(&root.join(".zshrc"), &a, "T115", "a").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pushes_that_land_on_the_default_branch_are_known() {
        let on_main = |args: &str| push_lands_on(args, Some("main"), "main");
        let on_fix = |args: &str| push_lands_on(args, Some("fix"), "main");
        assert!(on_main("") && on_main(" origin") && on_main(" -u origin HEAD"));
        assert!(on_fix(" origin main") && on_fix(" origin fix:main") && on_fix(" origin +HEAD:refs/heads/main") && on_fix(" origin :main"));
        assert!(on_fix(" --all origin") && on_fix(" --mirror"));
        assert!(!on_fix("") && !on_fix(" origin") && !on_fix(" -u origin fix") && !on_fix(" origin HEAD"));
        assert!(!on_main(" origin v1.2.0") && !on_main(" --tags") && !on_main(" origin :refs/tags/v1.2.0") && !on_main(" origin main:fix"));
        assert!(!on_fix(" -o ci.skip origin mainline"));
    }

    #[test]
    fn commits_on_and_pushes_to_the_default_branch_are_caught() {
        let root = std::env::temp_dir().join(format!("tb-default-branch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        git_repo(&root);
        let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&root).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap().status.success());
        git(&["checkout", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "one"]);
        let dir = root.to_string_lossy().to_string();
        let what = |cmd: &str| onto_default(cmd, &dir, "origin").map(|(what, _, branch)| format!("{what} {branch}"));
        assert_eq!(what("git commit -m x"), None, "no remote: nothing is pushed anywhere");

        git(&["remote", "add", "origin", "https://example.com/r.git"]);
        assert_eq!(what("git add a && git commit -m x").as_deref(), Some("commit on main"));
        assert_eq!(what("git push").as_deref(), Some("push to main"));
        assert_eq!(what("git switch -c fix && git commit -m x"), None, "the switch moves it off main first");
        assert_eq!(what("git checkout -b x && git push origin x:main").as_deref(), Some("push to main"), "a named destination still counts");
        git(&["switch", "-q", "-c", "fix"]);
        assert_eq!(what("git commit -m x && git push -u origin fix"), None);
        assert_eq!(what("git push origin fix:main").as_deref(), Some("push to main"));
        assert_eq!(what(&format!("cd / && git -C {dir} push origin HEAD:main")).as_deref(), Some("push to main"));

        // The remote's HEAD names the default branch when it's set.
        git(&["update-ref", "refs/remotes/origin/trunk", "HEAD"]);
        git(&["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/trunk"]);
        assert_eq!(what("git push origin fix:main"), None);
        assert_eq!(what("git push origin trunk").as_deref(), Some("push to trunk"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A shared checkout with one commit, and a board data folder beside it.
    fn shared_checkout(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("tb-shared-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (repo, data) = (root.join("repo"), root.join("data"));
        git_repo(&repo);
        for f in ["a.rs", "b.rs", "src/c.rs"] {
            std::fs::create_dir_all(repo.join(f).parent().unwrap()).unwrap();
            std::fs::write(repo.join(f), "one\n").unwrap();
        }
        let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(&repo).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "one"]);
        (root, repo, data)
    }

    #[test]
    fn the_hook_remembers_each_tasks_edits_once() {
        let (root, repo, data) = shared_checkout("ledger");
        note_edit(&data, "T1", "s1", &repo.join("a.rs"));
        note_edit(&data, "T1", "s1", &repo.join("a.rs"));
        note_edit(&data, "T2", "s2", &repo.join("b.rs"));
        note_edit(&data, "../x", "s3", &repo.join("b.rs"));
        let mut edits = ledger(&data);
        edits.sort_by(|a, b| a.task.cmp(&b.task));
        assert_eq!(edits.len(), 2, "{edits:?}");
        assert_eq!(edits[0], Edit { task: "T1".into(), session: "s1".into(), file: repo.join("a.rs") });
        assert_eq!(edits[1].task, "T2");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_hook_keeps_each_edits_patch_under_its_task() {
        let (root, repo, data) = shared_checkout("patches");
        let patch = json!({"structuredPatch": [{"oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1, "lines": ["-one", "+two"]}]});
        note_patch(&data, "T1", &repo.join("a.rs"), &patch);
        note_patch(&data, "T1", &repo.join("b.rs"), &patch);
        note_patch(&data, "T1", &repo.join("a.rs"), &json!({"structuredPatch": [], "type": "create"}));
        note_patch(&data, "../x", &repo.join("a.rs"), &patch);
        let log = std::fs::read_to_string(data.join("hunks").join("T1")).unwrap();
        let recs: Vec<Value> = log.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(recs.len(), 2, "an empty patch isn't kept: {log}");
        assert_eq!(recs[0], json!({"file": repo.join("a.rs").display().to_string(), "patch": patch["structuredPatch"]}));
        assert!(!data.join("x").exists() && std::fs::read_dir(data.join("hunks")).unwrap().count() == 1, "a bad task name writes nothing");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_checkouts_changes_say_whats_staged_and_tracked() {
        let (root, repo, _) = shared_checkout("changes");
        std::fs::write(repo.join("a.rs"), "two\n").unwrap();
        std::fs::write(repo.join("b.rs"), "two\n").unwrap();
        std::fs::write(repo.join("new.rs"), "new\n").unwrap();
        std::process::Command::new("git").arg("-C").arg(&repo).args(["add", "b.rs"]).output().unwrap();
        let mut list = changes(&repo).unwrap();
        list.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(
            list,
            vec![
                Change { path: "a.rs".into(), staged: false, tracked: true },
                Change { path: "b.rs".into(), staged: true, tracked: true },
                Change { path: "new.rs".into(), staged: false, tracked: false },
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_commit_takes_whats_staged_its_own_adds_and_with_a_every_tracked_change() {
        let top = Path::new("/r");
        let list = vec![
            Change { path: "a.rs".into(), staged: true, tracked: true },
            Change { path: "b.rs".into(), staged: false, tracked: true },
            Change { path: "src/c.rs".into(), staged: false, tracked: true },
            Change { path: "new.rs".into(), staged: false, tracked: false },
        ];
        let take = |cmd: &str, dir: &str| {
            let at = SHARED_GIT_RE.find(cmd).unwrap().end();
            let mut t: Vec<String> = committed(cmd, at, Path::new(dir), top, &list).into_iter().map(|(p, _)| p).collect();
            t.sort();
            t
        };
        assert_eq!(take("git commit -m x", "/r"), vec!["a.rs"]);
        assert_eq!(take("git add b.rs && git commit -m x", "/r"), vec!["a.rs", "b.rs"]);
        assert_eq!(take("git add c.rs && git commit -m x", "/r/src"), vec!["a.rs", "src/c.rs"], "an add from a subfolder");
        assert_eq!(take("git add src && git commit -m x", "/r"), vec!["a.rs", "src/c.rs"], "a folder adds what's in it");
        assert_eq!(take("git commit -am x", "/r"), vec!["a.rs", "b.rs", "src/c.rs"]);
        assert_eq!(take("git add -u && git commit -m x", "/r"), vec!["a.rs", "b.rs", "src/c.rs"]);
        assert_eq!(take("git add -A && git commit -m x", "/r"), vec!["a.rs", "b.rs", "new.rs", "src/c.rs"]);
        assert_eq!(take("git add . && git commit -m x", "/r"), vec!["a.rs", "b.rs", "new.rs", "src/c.rs"]);
        assert_eq!(take("git commit -m x && git add b.rs", "/r"), vec!["a.rs"], "an add after the commit isn't in it");
        let whole = |cmd: &str| committed(cmd, SHARED_GIT_RE.find(cmd).unwrap().end(), top, top, &list).into_iter().filter(|(_, w)| *w).map(|(p, _)| p).collect::<Vec<_>>();
        assert!(whole("git commit -m x").is_empty(), "only what's staged");
        assert_eq!(whole("git add a.rs && git commit -m x"), vec!["a.rs"], "an add stages its working copy");
    }

    #[test]
    fn a_commit_of_another_tasks_edits_in_a_shared_checkout_is_refused() {
        let (root, repo, data) = shared_checkout("commit");
        let cwd = repo.to_string_lossy().to_string();
        std::fs::write(repo.join("a.rs"), "mine\n").unwrap();
        std::fs::write(repo.join("b.rs"), "theirs\n").unwrap();
        note_edit(&data, "T1", "s1", &repo.join("a.rs"));
        note_edit(&data, "T2", "s2", &repo.join("b.rs"));

        assert!(shared_conflicts("git add a.rs && git commit -m x", &cwd, &data, Some("T1")).is_none(), "its own files");
        assert!(shared_conflicts("git status && git diff b.rs && git log", &cwd, &data, Some("T1")).is_none(), "read-only git");
        let (verb, top, theirs) = shared_conflicts("git add -A && git commit -m x", &cwd, &data, Some("T1")).expect("T2's file");
        assert_eq!((verb.as_str(), top.clone()), ("commit", repo.canonicalize().unwrap()));
        assert_eq!(theirs, vec![("T2".to_string(), "s2".to_string(), vec!["b.rs".to_string()])]);
        let why = shared_refusal(Some("T1"), &verb, &top, &theirs);
        assert!(why.contains("T2 (b.rs)") && why.contains("git restore --staged") && why.contains("tb slot claim --bring <its files>"), "{why}");

        // A file both tasks edited whose hunks can't be told apart (no patch log) is still T2's; a file
        // no task edited isn't the hook's call.
        note_edit(&data, "T1", "s1", &repo.join("b.rs"));
        std::fs::write(repo.join("src/c.rs"), "sed\n").unwrap();
        assert!(shared_conflicts("git commit -am x", &cwd, &data, Some("T1")).is_some());
        std::process::Command::new("git").arg("-C").arg(&repo).args(["checkout", "-q", "--", "b.rs"]).output().unwrap();
        assert!(shared_conflicts("git commit -am x", &cwd, &data, Some("T1")).is_none());
        // Committed edits are no one's any more.
        std::fs::remove_dir_all(ledger_dir(&data)).unwrap();
        note_edit(&data, "T2", "s2", &repo.join("b.rs"));
        std::process::Command::new("git").arg("-C").arg(&repo).args(["checkout", "-q", "--", "b.rs"]).output().unwrap();
        assert!(shared_conflicts("git commit -am x", &cwd, &data, Some("T1")).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `b.rs` in a shared checkout with T1's edit at line 2 and T2's at line 7, each with its patch
    /// logged; T1 also edited `a.rs` alone.
    fn two_tasks_in_one_file(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let (root, repo, data) = shared_checkout(name);
        std::fs::write(repo.join("b.rs"), "1\n2\n3\n4\n5\n6\n7\n8\n").unwrap();
        let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(&repo).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap();
        git(&["commit", "-qam", "eight"]);
        std::fs::write(repo.join("b.rs"), "1\ntwo\n3\n4\n5\n6\nseven\n8\n").unwrap();
        std::fs::write(repo.join("a.rs"), "mine\n").unwrap();
        note_edit(&data, "T1", "s1", &repo.join("a.rs"));
        note_edit(&data, "T1", "s1", &repo.join("b.rs"));
        note_edit(&data, "T2", "s2", &repo.join("b.rs"));
        let patch = |start: u64, lines: &[&str]| json!({"structuredPatch": [{"newStart": start, "lines": lines}]});
        note_patch(&data, "T1", &repo.join("b.rs"), &patch(1, &[" 1", "-2", "+two", " 3", " 4"]));
        note_patch(&data, "T2", &repo.join("b.rs"), &patch(5, &[" 5", " 6", "-7", "+seven", " 8"]));
        (root, repo, data)
    }

    #[test]
    fn a_commit_in_a_shared_file_may_take_only_this_tasks_hunks() {
        let (root, repo, data) = two_tasks_in_one_file("hunks");
        let cwd = repo.to_string_lossy().to_string();
        let t2_b = vec![("T2".to_string(), "s2".to_string(), vec!["b.rs".to_string()])];
        for cmd in ["git add b.rs && git commit -m x", "git commit -am x", "git add -A && git commit -m x"] {
            let (verb, top, theirs) = shared_conflicts(cmd, &cwd, &data, Some("T1")).unwrap_or_else(|| panic!("{cmd}"));
            assert_eq!(theirs, t2_b, "{cmd}");
            let why = shared_refusal(Some("T1"), &verb, &top, &theirs);
            assert!(why.contains("T2 (b.rs)") && why.contains("tb stage <file>"), "{why}");
        }
        assert!(shared_conflicts("git add a.rs && git commit -m x", &cwd, &data, Some("T1")).is_none(), "a file only T1 edited");

        let said = stage_own(&data, "T1", &repo, &[]).unwrap();
        assert!(said.contains("b.rs (1 of 2 hunks)") && said.contains("Staged whole (no other task edits them): a.rs"), "{said}");
        let index = git_text(&repo, &["show", ":b.rs"]).unwrap();
        assert_eq!(index, "1\ntwo\n3\n4\n5\n6\n7\n8\n", "T2's line stays unstaged");
        assert_eq!(std::fs::read_to_string(repo.join("b.rs")).unwrap(), "1\ntwo\n3\n4\n5\n6\nseven\n8\n", "the working copy is untouched");
        assert!(shared_conflicts("git commit -m x", &cwd, &data, Some("T1")).is_none(), "only T1's hunks are staged");
        assert_eq!(shared_conflicts("git commit -am x", &cwd, &data, Some("T1")).map(|(_, _, t)| t), Some(t2_b.clone()), "-a stages T2's too");

        // A hand-staged hunk of T2's is caught even when it isn't one of the working copy's hunks.
        let both = "1\ntwo\n3\n4\n5\n6\nSEVEN\n8\n";
        let blob = git_in(&repo, &["hash-object", "-w", "--stdin"], both.as_bytes()).unwrap();
        git_text(&repo, &["update-index", "--cacheinfo", &format!("100644,{},b.rs", blob.trim())]).unwrap();
        assert_eq!(shared_conflicts("git commit -m x", &cwd, &data, Some("T1")).map(|(_, _, t)| t), Some(t2_b));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn staging_own_hunks_refuses_a_hunk_both_tasks_changed() {
        let (root, repo, data) = two_tasks_in_one_file("stage-clash");
        let patch = json!({"structuredPatch": [{"newStart": 1, "lines": [" 1", " two", " 3", " 4", " 5", " 6", "-7", "+seven", " 8"]}]});
        note_patch(&data, "T1", &repo.join("b.rs"), &patch);
        let why = stage_own(&data, "T1", &repo, &["b.rs".to_string()]).unwrap_err();
        assert!(why.contains("b.rs (T2 changed the same lines as T1, at line 7)") && why.contains("Nothing was staged"), "{why}");
        assert_eq!(git_text(&repo, &["diff", "--cached", "--name-only"]).unwrap(), "", "nothing staged");
        let why = stage_own(&data, "T1", &repo, &["src/c.rs".to_string()]).unwrap_err();
        assert!(why.contains("nothing in") && why.contains("src/c.rs"), "{why}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn moving_a_shared_checkout_under_another_tasks_edits_is_refused() {
        let (root, repo, data) = shared_checkout("move");
        let cwd = repo.to_string_lossy().to_string();
        std::fs::write(repo.join("b.rs"), "theirs\n").unwrap();
        note_edit(&data, "T2", "s2", &repo.join("b.rs"));
        for cmd in ["git switch -c fix/x", "git checkout main", "git rebase origin/main", "git stash", "git reset --hard", &format!("cd /tmp && git -C {cwd} switch main")] {
            let (verb, _, theirs) = shared_conflicts(cmd, "/tmp", &data, Some("T1")).or_else(|| shared_conflicts(cmd, &cwd, &data, Some("T1"))).unwrap_or_else(|| panic!("{cmd}"));
            assert_eq!(theirs[0].0, "T2", "{cmd}");
            let why = shared_refusal(Some("T1"), &verb, &repo, &theirs);
            assert!(why.contains(&format!("can't git {verb}")) && why.contains("T2 (b.rs)") && why.contains("by name") && why.contains("tb slot claim --bring <its files>"), "{why}");
        }
        for cmd in ["git stash list", "git stash show -p", "git log --oneline", "git branch -a"] {
            assert!(shared_conflicts(cmd, &cwd, &data, Some("T1")).is_none(), "{cmd}");
        }
        assert!(shared_conflicts("git switch main", &cwd, &data, Some("T2")).is_none(), "its own edits");
        std::fs::write(repo.join("a.rs"), "mine\n").unwrap();
        for cmd in ["git checkout -- a.rs", "git stash push -- a.rs", "git reset -q -- a.rs"] {
            assert!(shared_conflicts(cmd, &cwd, &data, Some("T1")).is_none(), "only the files it names: {cmd}");
        }
        assert!(shared_conflicts("git checkout -- b.rs", &cwd, &data, Some("T1")).is_some(), "another task's file");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn git_words_inside_quoted_arguments_are_not_git_commands() {
        let said = unquoted(r#"tb question "may I git commit -am x?" && echo 'git stash'"#);
        assert_eq!(said.len(), r#"tb question "may I git commit -am x?" && echo 'git stash'"#.len());
        assert!(!SHARED_GIT_RE.is_match(&said), "{said}");
        assert!(SHARED_GIT_RE.is_match(&unquoted(r#"git commit -m "fix: x""#)));
        assert!(SHARED_GIT_RE.is_match(&unquoted(r#"bash -c "git commit -am x""#)), "a string a shell runs is a command");

        let (root, repo, data) = shared_checkout("quoted");
        let cwd = repo.to_string_lossy().to_string();
        std::fs::write(repo.join("b.rs"), "theirs\n").unwrap();
        note_edit(&data, "T2", "s2", &repo.join("b.rs"));
        for cmd in [r#"tb question "can T1 git add -A && git commit -am x?""#, r#"git commit -m "fix; git add -A""#, r#"git commit -m "not -a""#] {
            assert!(shared_conflicts(cmd, &cwd, &data, Some("T1")).is_none(), "{cmd}");
        }
        assert!(shared_conflicts(r#"bash -c "git commit -am x""#, &cwd, &data, Some("T1")).is_some());
        assert!(shared_conflicts(r#"git commit -am "x""#, &cwd, &data, Some("T1")).is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_hunk_of_only_braces_is_no_tasks_and_its_record_goes() {
        let (root, repo, data) = shared_checkout("braces");
        let cwd = repo.to_string_lossy().to_string();
        let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(&repo).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap();
        std::fs::write(repo.join("b.rs"), "1\n2\n3\n").unwrap();
        git(&["commit", "-qam", "three"]);
        std::fs::write(repo.join("b.rs"), "1\n2\n}\n3\n").unwrap();
        note_edit(&data, "T2", "s2", &repo.join("b.rs"));
        note_patch(&data, "T2", &repo.join("b.rs"), &json!({"structuredPatch": [{"newStart": 2, "lines": [" 2", "+}", " 3"]}]}));
        for cmd in ["git commit -am x", "git rebase --continue"] {
            assert!(shared_conflicts(cmd, &cwd, &data, Some("T1")).is_none(), "{cmd}");
        }
        assert!(ledger(&data).is_empty(), "the record that stood for nothing is dropped");
        assert!(!data.join("hunks").join("T2").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_record_whose_hunks_are_in_the_tasks_pushed_branch_is_dropped() {
        let (root, repo, data) = shared_checkout("pushed");
        let cwd = repo.to_string_lossy().to_string();
        let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(&repo).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap();
        std::fs::write(repo.join("b.rs"), "1\n4\n").unwrap();
        git(&["commit", "-qam", "two"]);
        git(&["switch", "-qc", "theirs"]);
        std::fs::write(repo.join("b.rs"), "1\nadded\n4\n").unwrap();
        git(&["commit", "-qam", "theirs"]);
        git(&["switch", "-q", "-"]);
        std::fs::write(repo.join("b.rs"), "1\nadded\n4\n").unwrap();
        note_edit(&data, "T2", "s2", &repo.join("b.rs"));
        note_patch(&data, "T2", &repo.join("b.rs"), &json!({"structuredPatch": [{"newStart": 1, "lines": [" 1", "+added", " 4"]}]}));
        assert!(shared_conflicts("git rebase --continue", &cwd, &data, Some("T1")).is_some(), "not pushed: still T2's");
        assert_eq!(ledger(&data).len(), 1);
        let pushed = |_: &str, _: &Path| vec!["theirs".to_string()];
        assert!(shared_conflicts_with("git rebase --continue", &cwd, &data, Some("T1"), &pushed).is_none());
        assert!(ledger(&data).is_empty(), "its hunks are saved in its branch");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn records_of_committed_files_are_dropped_and_tb_edits_clear_drops_the_rest() {
        let (root, repo, data) = shared_checkout("forget");
        let cwd = repo.to_string_lossy().to_string();
        let top = repo.canonicalize().unwrap();
        std::fs::write(repo.join("b.rs"), "theirs\n").unwrap();
        for f in ["a.rs", "b.rs", "src/c.rs"] {
            note_edit(&data, "T2", "s2", &repo.join(f));
        }
        note_patch(&data, "T2", &repo.join("a.rs"), &json!({"structuredPatch": [{"newStart": 1, "lines": ["-one", "+two"]}]}));
        let (_, _, theirs) = shared_conflicts("git switch main", &cwd, &data, Some("T1")).unwrap();
        assert_eq!(theirs[0].2, vec!["b.rs".to_string()]);
        let left: Vec<PathBuf> = ledger(&data).into_iter().map(|e| e.file).collect();
        assert_eq!(left, vec![repo.join("b.rs")], "a.rs and src/c.rs have nothing uncommitted");
        assert!(!data.join("hunks").join("T2").exists());

        let said = clear_edits(&data, "T2", &top);
        assert!(said.starts_with("Cleared T2's recorded edits in") && said.ends_with(": b.rs."), "{said}");
        assert!(ledger(&data).is_empty());
        assert!(shared_conflicts("git switch main", &cwd, &data, Some("T1")).is_none());
        assert!(clear_edits(&data, "T2", &top).starts_with("T2 has no recorded edits in"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn opening_a_pr_on_a_task_that_ends_in_one_points_to_pr_body() {
        let why = board_opens_pr(&json!({"task": "T7", "ships_pr": true}), "tb").unwrap();
        assert!(why.starts_with("T7 ends in a PR"), "{why}");
        assert!(why.contains("tb submit \"<summary>\" --pr-body <file>"), "{why}");
        assert!(board_opens_pr(&json!({"task": "T7", "ships_pr": false}), "tb").is_none());
        assert!(board_opens_pr(&json!({"task": "T7"}), "tb").is_none(), "an older board's listing has no flag");
        assert!(GITLAB_RE.is_match("glab mr create --fill") && !GITLAB_RE.is_match("gh pr create"), "the board can't open a GitLab MR");
    }

    #[test]
    fn bash_and_subagent_calls_open_a_window_by_their_id() {
        let call = |tool: &str, input: Value, id: Option<&str>| {
            let mut p = json!({"tool_name": tool, "tool_input": input, "cwd": "/nowhere/at/all", "session_id": "c1"});
            if let Some(id) = id {
                p["tool_use_id"] = json!(id);
            }
            tool_window_body(&p, "s1", "hook.tool_start")
        };
        let b = call("Bash", json!({"command": "ls"}), Some("toolu_1")).expect("a Bash call");
        assert_eq!((b["event"].as_str(), b["tool_use_id"].as_str(), b["background"].as_bool()), (Some("hook.tool_start"), Some("toolu_1"), Some(false)));
        assert_eq!(b["tree"], Value::Null, "no checkout, no stamp");
        let bg = call("Agent", json!({"prompt": "x", "run_in_background": true}), None).expect("a subagent");
        assert_eq!(bg["background"], json!(true));
        assert_eq!(bg["tool_use_id"], call("Agent", json!({"prompt": "x", "run_in_background": true}), None).unwrap()["tool_use_id"], "the same call, the same id");
        assert!(call("Read", json!({"file_path": "/a"}), Some("toolu_2")).is_none());
        assert_eq!(bg["tool_use_id"].as_str(), Some(taskboardd::transcript::call_key("Agent", &json!({"prompt": "x", "run_in_background": true})).as_str()), "the board can find it in the transcript");
    }

    #[test]
    fn worktrees_outside_the_slot_pool_are_caught() {
        let bash = |c: &str| stray_worktree("Bash", &json!({"command": c}), "/w/b", None);
        assert_eq!(bash("git worktree add ../b-fix -b fix").as_deref(), Some("git worktree add /w/b-fix"));
        assert_eq!(bash("cd /w/a && git worktree add -b fix --lock --reason why /w/a-fix main").as_deref(), Some("git worktree add /w/a-fix"));
        assert!(bash("git -C /w/b worktree add .claude/worktrees/t9").is_some());
        assert!(bash("git worktree add").is_some(), "no folder named: still a new worktree");
        assert_eq!(bash("git worktree add .claude/worktrees/slot-3 --detach"), None, "a pool slot");
        assert_eq!(bash("git -C /w/b worktree add /w/b/.claude/worktrees/slot-12"), None);
        assert_eq!(bash("git worktree list && git worktree remove ../b-fix"), None);
        assert_eq!(bash("git status; echo worktree add"), None);
        assert!(stray_worktree("Bash", &json!({"command": "git worktree add ../b-t9"}), "/w/b", Some("/w/b-t9")).is_none(), "the task's own worktree");

        let enter = |i: Value| stray_worktree("EnterWorktree", &i, "/w/b", Some("/w/b/.claude/worktrees/t9"));
        assert!(enter(json!({})).is_some(), "a new worktree");
        assert!(enter(json!({"name": "fix"})).is_some());
        assert!(enter(json!({"name": "slot-2"})).is_some(), "a new one named like a slot isn't a slot");
        assert_eq!(enter(json!({"path": ".claude/worktrees/slot-2"})), None);
        assert_eq!(enter(json!({"path": "/w/b/.claude/worktrees/t9"})), None);
        assert!(enter(json!({"path": "/w/b/.claude/worktrees/t10"})).is_some());

        for tool in SUBAGENT_TOOLS {
            assert!(stray_worktree(tool, &json!({"prompt": "x", "isolation": "worktree"}), "/w/b", None).is_some());
            assert_eq!(stray_worktree(tool, &json!({"prompt": "x"}), "/w/b", None), None);
        }
        assert_eq!(stray_worktree("Read", &json!({"file_path": "/w/b/.claude/worktrees/x"}), "/w/b", None), None);

        let why = stray_refusal("T5", "a subagent with isolation: worktree", "tb");
        assert!(why.contains("T5") && why.contains("tb slot claim --bring <files>") && why.contains("isolation: worktree"), "{why}");
        assert!(!why.contains("doesn't work in a worktree"), "any task can claim a slot now: {why}");
    }

    #[test]
    fn knows_whether_the_loaded_hooks_send_tool_end() {
        let plugin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin/task-board");
        assert!(hooks_send_tool_end(Some(&plugin)), "this plugin's hooks.json");
        let old = std::env::temp_dir().join(format!("tb-old-hooks-{}", std::process::id()));
        std::fs::create_dir_all(old.join("hooks")).unwrap();
        let hooks = json!({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "tb", "args": ["hook", "PreToolUse"]}]}], "PostToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "tb", "args": ["hook", "PostToolUse"]}]}]}});
        std::fs::write(old.join("hooks").join("hooks.json"), hooks.to_string()).unwrap();
        assert!(!hooks_send_tool_end(Some(&old)), "a hooks.json from before ToolEnd");
        let _ = std::fs::remove_dir_all(&old);
        assert!(!hooks_send_tool_end(None), "no plugin folder");
    }

    #[test]
    fn git_commands_are_recognised() {
        assert!(GIT_RE.is_match("git commit -m x"));
        assert!(GIT_RE.is_match("git -C /a push origin x"));
        assert!(!GIT_RE.is_match("git status"));
        assert!(PUSH_RE.is_match("git push --force-with-lease"));
        assert!(!PUSH_RE.is_match("git commit -m push"));
        assert_eq!(clip_middle("abcdefghij", 7), "abc\n…\nj");
    }

    #[test]
    fn a_long_prompt_keeps_its_start_and_its_end() {
        let typed = format!("start T8. here's the log:\n{}\njk", "INFO all good\n".repeat(3000));
        let report = taskboardd::startword::hook_prompt(&typed);
        assert_eq!(report["prompt_cut"], taskboardd::startword::KEPT_ENDS, "it says how it cut");
        let sent = report["prompt"].as_str().unwrap();
        assert!(sent.chars().count() <= MAX_PROMPT);
        assert!(sent.starts_with("start T8.") && sent.ends_with("\njk") && sent.contains(taskboardd::startword::CUT_MARK));
        let short = format!("start T8. {}…", "word ".repeat(1000));
        assert_eq!(taskboardd::startword::keep_ends(&short, MAX_PROMPT), short, "sent whole up to the board's cap");
    }

    #[test]
    fn a_headless_run_isnt_the_terminals_conversation() {
        assert!(headless(Some("sdk-cli")), "claude -p");
        assert!(headless(Some("sdk-ts")), "the Agent SDK");
        assert!(!headless(Some("cli")), "the terminal's own claude");
        assert!(!headless(None));
    }

    #[test]
    fn a_hook_log_line_says_who_what_why_and_how_long() {
        let line = failure_line("2026-10-26 14:03:11", "7", "PreToolUse", "GET /steps", "timed out:\n  read  timed out", Duration::from_millis(1003));
        assert_eq!(line, "2026-10-26 14:03:11 session=7 event=PreToolUse GET /steps failed after 1003ms: timed out: read timed out\n");
        assert!(failure_line("t", "", "", "spool", "x", Duration::ZERO).starts_with("t session=- event=- spool"), "{}", failure_line("t", "", "", "spool", "x", Duration::ZERO));
    }

    #[test]
    fn the_hook_log_starts_over_past_its_cap_keeping_the_one_before() {
        let root = std::env::temp_dir().join(format!("tb-hook-log-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let log = root.join(HOOK_LOG);
        append_capped(&log, "one\n", 8);
        append_capped(&log, "two\n", 8);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "one\ntwo\n");
        append_capped(&log, "three\n", 8);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "three\n");
        assert_eq!(std::fs::read_to_string(root.join("hook.log.1")).unwrap(), "one\ntwo\n");
        append_capped(&root.join("gone").join(HOOK_LOG), "x\n", 8);
        assert!(!root.join("gone").exists(), "a log that can't be written is dropped, not made");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_board_call_or_ledger_write_that_fails_goes_to_the_hook_log() {
        let root = std::env::temp_dir().join(format!("tb-hook-log-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut cfg = Config::for_tests(&root);
        // A port nothing listens on: the board is down.
        cfg.port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(board_call(&cfg, "GET", "/steps?session=7", None, 0.5).is_err());
        // `edits` is a file, so the ledger's folder can't be made.
        std::fs::write(root.join("edits"), "").unwrap();
        note_edit(&root, "T1", "7", Path::new("/w/a.rs"));
        let log = std::fs::read_to_string(root.join(HOOK_LOG)).unwrap();
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines.len(), 2, "{log}");
        assert!(lines[0].contains(" GET /steps failed after ") && lines[0].contains("ms: unreachable: "), "{}", lines[0]);
        assert!(!lines[0].contains("session=7&"), "the query isn't repeated: {}", lines[0]);
        assert!(lines[1].contains(&format!(" write {} failed after ", root.join("edits").join("T1").display())), "{}", lines[1]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A board that answers `answers` in turn, one request each, then stops listening; and what it
    /// was asked.
    fn board(cfg: &mut Config, answers: Vec<Value>) -> std::sync::mpsc::Receiver<String> {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        cfg.port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for answer in answers {
                let Ok((stream, _)) = listener.accept() else { return };
                let mut r = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                let _ = r.read_line(&mut first);
                let mut len = 0;
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                        break;
                    }
                    if let Some(n) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = n.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; len];
                let _ = std::io::Read::read_exact(&mut r, &mut body);
                let _ = tx.send(first.trim().to_string());
                let text = answer.to_string();
                let mut w = stream;
                let _ = write!(w, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
            }
        });
        rx
    }

    fn down(cfg: &mut Config) {
        cfg.port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    }

    fn age(p: &Path, by: Duration) {
        let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
        f.set_modified(std::time::SystemTime::now() - by).unwrap();
    }

    fn test_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("tb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn a_kept_steps_answer_stands_in_while_fresh_and_while_the_board_is_down() {
        let root = test_root("steps-kept");
        let mut cfg = Config::for_tests(&root);
        down(&mut cfg);
        assert!(steps(&cfg, "fresh-1", "").is_none(), "nothing kept, board down: no answer");

        keep_steps(&root, "fresh-1", "", &json!({"task": "T1"}));
        let rx = board(&mut cfg, vec![json!({"task": "T2"})]);
        assert_eq!(steps(&cfg, "fresh-1", ""), Some((json!({"task": "T1"}), true)), "a fresh answer is used without asking");
        assert!(rx.try_recv().is_err());

        // Too old to stand in when the board answers, still good when it doesn't.
        keep_steps(&root, "aged-1", "", &json!({"task": "T1"}));
        age(&steps_file(&root, "aged-1", ""), STEPS_FRESH + Duration::from_secs(5));
        down(&mut cfg);
        assert_eq!(steps(&cfg, "aged-1", ""), Some((json!({"task": "T1"}), true)));
        board(&mut cfg, vec![json!({"task": "T3"})]);
        assert_eq!(steps(&cfg, "aged-1", ""), Some((json!({"task": "T3"}), false)), "the board's answer when it has one");
        assert_eq!(kept_steps(&root, "aged-1", "", STEPS_FRESH), Some(json!({"task": "T3"})), "and it's kept for the next hook");

        keep_steps(&root, "stale-1", "", &json!({"task": "T1"}));
        age(&steps_file(&root, "stale-1", ""), STEPS_STALE + Duration::from_secs(5));
        down(&mut cfg);
        assert!(steps(&cfg, "stale-1", "").is_none(), "too old even with the board down");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn forgetting_steps_drops_only_that_terminals_answers() {
        let root = test_root("steps-forget");
        let mut cfg = Config::for_tests(&root);
        keep_steps(&root, "forget-a", "", &json!({"task": "T1"}));
        keep_steps(&root, "forget-a", "&head=x", &json!({"task": "T1"}));
        keep_steps(&root, "forget-b", "", &json!({"task": "T2"}));
        down(&mut cfg);
        assert!(steps(&cfg, "forget-a", "").is_some());
        forget_steps(&root, "forget-a");
        assert!(steps(&cfg, "forget-a", "").is_none(), "neither the memo nor the file is left");
        assert!(kept_steps(&root, "forget-a", "&head=x", STEPS_STALE).is_none());
        assert_eq!(kept_steps(&root, "forget-b", "", STEPS_FRESH), Some(json!({"task": "T2"})));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_refusal_on_a_kept_answer_is_checked_with_the_board() {
        let root = test_root("steps-judged");
        let mut cfg = Config::for_tests(&root);
        let refuse = |v: &Value| v["home"].as_str().filter(|h| *h == "/elsewhere").map(str::to_string);

        keep_steps(&root, "judge-1", "", &json!({"home": "/elsewhere"}));
        let rx = board(&mut cfg, vec![json!({"home": "/here"})]);
        assert_eq!(judged(&cfg, "judge-1", "", refuse), None, "the board moved on: let through");
        let asked = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(asked.starts_with("GET ") && asked.contains("/steps?session=judge-1 "), "{asked}");

        keep_steps(&root, "judge-2", "", &json!({"home": "/elsewhere"}));
        down(&mut cfg);
        assert_eq!(judged(&cfg, "judge-2", "", refuse).as_deref(), Some("/elsewhere"), "no board: the kept answer stands");

        keep_steps(&root, "judge-3", "", &json!({"home": "/here"}));
        let rx = board(&mut cfg, vec![json!({"home": "/elsewhere"})]);
        assert_eq!(judged(&cfg, "judge-3", "", refuse), None, "a kept answer that lets through isn't rechecked");
        assert!(rx.try_recv().is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_background_send_clears_its_report_or_hands_it_back() {
        let root = test_root("send");
        let mut cfg = Config::for_tests(&root);
        let body = json!({"event": "hook.tool_end", "session": "7"});

        let sent = client::spool_claimed(&cfg, &body).unwrap();
        let rx = board(&mut cfg, vec![json!({"ok": true, "queued": true})]);
        send_file(&cfg, &sent, false);
        let asked = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(asked.starts_with("POST ") && asked.ends_with("/report?queue=1 HTTP/1.1"), "{asked}");
        assert!(!sent.exists());

        down(&mut cfg);
        let kept = client::spool_claimed(&cfg, &body).unwrap();
        send_file(&cfg, &kept, false);
        assert!(!kept.exists());
        let back = kept.with_extension("");
        assert!(back.to_string_lossy().ends_with(".json"), "{}", back.display());
        assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(&back).unwrap()).unwrap(), body, "back in the spool for the board");

        let dropped = client::spool_claimed(&cfg, &body).unwrap();
        send_file(&cfg, &dropped, true);
        assert!(!dropped.exists() && !dropped.with_extension("").exists(), "--drop: gone");

        // Only a report in the spool marked as being sent is touched.
        let stray = root.join("x.json.sending");
        std::fs::write(&stray, body.to_string()).unwrap();
        send_file(&cfg, &stray, true);
        assert!(stray.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod private_status_tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_status_folder_and_files_are_private_even_when_the_folder_was_open() {
        let root = std::env::temp_dir().join(format!("tb-statusline-test-{}", std::process::id()));
        let dir = root.join("statusline");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        super::save_private(&dir, "s1", "{}").unwrap();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("s1.json")), 0o600);
        assert_eq!(std::fs::read_to_string(dir.join("s1.json")).unwrap(), "{}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
