//! Screening an agent's question: when the owner's own rules already answer it, the agent gets the
//! answer and the owner isn't asked. A headless `claude -p` reads the question with the rules.

use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, handoff, proc};

const SKILL_QUESTIONS: &str = include_str!("../../../plugin/task-board/skills/task-board/questions.md");

fn verdict_schema() -> Value {
    json!({"type": "object", "required": ["verdict"], "properties": {
        "verdict": {"enum": ["answered", "ask"]}, "answer": {"type": "string"},
        "source": {"type": "string"}, "question": {"type": "string"}}})
}

/// How much of the owner's rules the screener reads, all files together, and how many files.
const RULES_BUDGET: usize = 60_000;
const RULES_FILES: usize = 200;

fn has_wildcard(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

/// A glob component (`*`, `?`, `[abc]`) as a regex over one file name.
fn component_re(part: &str) -> Option<Regex> {
    let mut re = String::from("^");
    let mut chars = part.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
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
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    Regex::new(&re).ok()
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    v.sort();
    v
}

/// The files a glob names: `*`, `?` and `[…]` within a name, `**` for any depth of folders.
pub fn glob_files(pattern: &Path) -> Vec<PathBuf> {
    let parts: Vec<String> = pattern.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    let mut found = vec![];
    fn walk(at: PathBuf, rest: &[String], found: &mut Vec<PathBuf>) {
        if found.len() >= RULES_FILES {
            return;
        }
        let Some((first, more)) = rest.split_first() else {
            if at.is_file() && !found.contains(&at) {
                found.push(at);
            }
            return;
        };
        if first == "**" {
            walk(at.clone(), more, found);
            for e in sorted_entries(&at).into_iter().filter(|e| e.is_dir() && !e.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))) {
                walk(e, rest, found);
            }
        } else if has_wildcard(first) {
            let Some(re) = component_re(first) else { return };
            for e in sorted_entries(&at) {
                let name = e.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if re.is_match(&name) && (first.starts_with('.') || !name.starts_with('.')) {
                    walk(e, more, found);
                }
            }
        } else {
            walk(at.join(first), more, found);
        }
    }
    let (start, rest) = match parts.split_first() {
        Some((r, rest)) if r == "/" => (PathBuf::from("/"), rest),
        _ => (PathBuf::from("."), &parts[..]),
    };
    walk(start, rest, &mut found);
    found
}

/// The files one `questions.rules` entry names: a file; a folder (its `MEMORY.md`, then every other
/// `.md` in it, like a Claude memory folder); or a glob.
pub fn rule_files(entry: &str) -> Vec<PathBuf> {
    let p = expand_home(entry.trim());
    if has_wildcard(entry) {
        return glob_files(&p);
    }
    if p.is_dir() {
        let mut md: Vec<PathBuf> = sorted_entries(&p).into_iter().filter(|f| f.is_file() && f.extension().is_some_and(|e| e == "md")).collect();
        md.sort_by_key(|f| f.file_name().map(|n| n != "MEMORY.md").unwrap_or(true));
        return md;
    }
    vec![p]
}

fn rules(app: &App) -> String {
    let mut out = vec![format!("questions.md:\n{SKILL_QUESTIONS}")];
    let mut seen: Vec<PathBuf> = vec![];
    let mut left = RULES_BUDGET;
    for p in app.cfg.questions.rules.iter().flat_map(|r| rule_files(r)) {
        if seen.contains(&p) || left == 0 || seen.len() >= RULES_FILES {
            continue;
        }
        seen.push(p.clone());
        if let Ok(text) = std::fs::read_to_string(&p) {
            let text = clip(&text, 12000.min(left));
            left = left.saturating_sub(text.len());
            out.push(format!("{}:\n{}", p.display(), text));
        }
    }
    out.join("\n\n")
}

fn prompt(app: &App, t: Option<&Row>, question: &str) -> Result<String> {
    let owner = &app.cfg.owner;
    let mut parts = vec![format!(
        "An agent working on {owner}'s task board wants to ask {owner} a question. Decide whether the rules and \
         context below already settle it.\n- answered: they clearly do. Give the answer in plain words and name its \
         source (a file name or 'the task').\n- ask: they don't, or you're unsure. Rewrite the question for {owner} in \
         plain words: say what it's about, explain every reference and term, and keep it short.\nNever invent a rule."
    )];
    parts.push(format!("The question:\n{question}"));
    if let Some(t) = t {
        parts.push(format!("The task's handoff:\n{}", clip(&handoff::build(app, t.id())?, 5000)));
        let earlier: Vec<String> = app
            .db
            .q("SELECT text FROM events WHERE task_id = ? AND kind IN ('question','answer') ORDER BY id DESC LIMIT 12", crate::p![t.id()])?
            .iter()
            .map(|e| format!("- {}", one_line(&e.st("text"), 400)))
            .collect();
        if !earlier.is_empty() {
            parts.push(format!("Earlier questions and answers:\n{}", earlier.join("\n")));
        }
        if let Some(g) = board::find_goal(app, t.i("goal_id"))? {
            let notes = board::goal_notes(app, g.id())?;
            if !notes.is_empty() {
                parts.push(format!("The goal's notes:\n{}", handoff::notes_block(&notes, 2500)));
            }
        }
    }
    parts.push(format!("Rules:\n{}", rules(app)));
    Ok(parts.join("\n\n"))
}

/// Returns `{verdict, answer?, source?, question?}`, or None when screening is off or fails.
pub fn question(app: &App, t: Option<&Row>, text: &str) -> Option<Value> {
    if !app.cfg.questions.screen {
        return None;
    }
    let claude = proc::which(&app.cfg.claude)?;
    let p = prompt(app, t, text).ok()?;
    let q = &app.cfg.questions;
    let args: Vec<String> = vec![
        "-p".into(),
        p,
        "--model".into(),
        q.model.clone(),
        "--setting-sources".into(),
        "".into(),
        "--no-session-persistence".into(),
        "--tools".into(),
        "".into(),
        "--strict-mcp-config".into(),
        "--max-budget-usd".into(),
        q.budget_usd.clone(),
        "--json-schema".into(),
        verdict_schema().to_string(),
        "--output-format".into(),
        "json".into(),
    ];
    let out = match proc::run(&claude, &args, Some(&app.cfg.data), q.timeout_secs as f64) {
        Ok(o) => o,
        Err(_) => {
            app.info("screen: claude didn't answer in time; the question goes to the owner as written");
            return None;
        }
    };
    let d: Value = serde_json::from_str(out.stdout.trim()).ok()?;
    let v = d.get("structured_output").cloned().or_else(|| d.get("result").and_then(|r| r.as_str()).and_then(|r| serde_json::from_str(r).ok()))?;
    match v["verdict"].as_str() {
        Some("answered") if v["answer"].as_str().map(|a| !a.trim().is_empty()).unwrap_or(false) => Some(json!({
            "verdict": "answered", "answer": v["answer"], "source": v["source"].as_str().filter(|s| !s.is_empty()).unwrap_or("the rules")})),
        Some("ask") => {
            let q = v["question"].as_str().filter(|q| !q.trim().is_empty()).map(|q| q.to_string());
            Some(json!({"verdict": "ask", "question": q}))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_take_folders_and_globs() {
        let dir = tempfile_dir();
        let mem = dir.join("proj-a").join("memory");
        std::fs::create_dir_all(&mem).unwrap();
        std::fs::create_dir_all(dir.join("proj-b").join("memory").join("deep")).unwrap();
        for (f, t) in [("a-note.md", "a"), ("MEMORY.md", "index"), ("skip.txt", "no")] {
            std::fs::write(mem.join(f), t).unwrap();
        }
        std::fs::write(dir.join("proj-b").join("memory").join("b.md"), "b").unwrap();
        std::fs::write(dir.join("proj-b").join("memory").join("deep").join("c.md"), "c").unwrap();
        let names = |v: Vec<PathBuf>| v.iter().map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().to_string()).collect::<Vec<_>>();
        assert_eq!(names(rule_files(&mem.to_string_lossy())), vec!["proj-a/memory/MEMORY.md", "proj-a/memory/a-note.md"]);
        assert_eq!(
            names(rule_files(&format!("{}/proj-*/memory/*.md", dir.display()))),
            vec!["proj-a/memory/MEMORY.md", "proj-a/memory/a-note.md", "proj-b/memory/b.md"]
        );
        assert_eq!(names(rule_files(&format!("{}/**/c.md", dir.display()))), vec!["proj-b/memory/deep/c.md"]);
        assert_eq!(names(rule_files(&format!("{}/proj-[b]/memory/b.md", dir.display()))), vec!["proj-b/memory/b.md"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tempfile_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("tb-rules-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::canonicalize(d).unwrap()
    }
}
