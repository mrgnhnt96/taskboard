//! Backlog issues a task fixed on the way (`tb submit --fixes B<n>`). The first `tb submit` of a task shows
//! the project's open issues about the area or files it changed, once; the issues it names are kept on the
//! task (`fixes` in its context) and close as fixed when its PR merges, or as it finishes when it has no PR.

use std::path::Path;

use serde_json::{json, Value};

use crate::app::App;
use crate::board;
use crate::util::*;

/// The most related issues a submit lists.
const SHOWN: usize = 12;

/// File names too common to say which file an issue means.
const GENERIC: &[&str] = &[
    "mod.rs", "lib.rs", "main.rs", "build.rs", "Cargo.toml", "Cargo.lock", "README.md", "package.json", "index.js", "index.ts",
    "index.tsx", "__init__.py", "pubspec.yaml", "main.dart",
];

/// The issue ids a submit names, as the CLI sends them (`["B12", 14]`).
pub fn named(body: &Value) -> Result<Vec<i64>> {
    let mut out: Vec<i64> = vec![];
    for v in body.get("fixes").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        if let Some(id) = parse_ref(&v, "issue")? {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    Ok(out)
}

/// The issues a task says it fixed, kept on it.
pub fn of_task(t: &Row) -> Vec<i64> {
    board::task_context(t).get("fixes").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_i64()).collect()).unwrap_or_default()
}

/// Refuses an issue the task can't fix: one that isn't in its project's backlog or isn't open.
pub fn check(app: &App, t: &Row, ids: &[i64]) -> Result<()> {
    let me = rf("task", t.id());
    for &id in ids {
        let b = rf("issue", id);
        let Some(issue) = board::find_issue(app, Some(id))? else {
            return err(404, format!("There's no {b} on the board."));
        };
        if issue.s("project") != t.s("project") {
            return err(400, format!("{b} is in {}'s backlog, not {}'s, so {me} can't close it.", issue.st("project"), t.st("project")));
        }
        match (issue.s("state"), issue.i("task_id")) {
            (Some("open" | "defer"), _) => {}
            (Some("task"), Some(other)) => return err(409, format!("{b} is already {}: it closes with that task, not {me}.", rf("task", other))),
            _ => return err(409, format!("{b} isn't open any more, so there's nothing for {me} to close.")),
        }
    }
    Ok(())
}

/// Whether `text` names file `f` (repo-relative): by its name when that's specific enough, or by a path
/// that ends its own (`src/tb.rs` for `crates/cli/src/tb.rs`), never inside a longer name or another path.
fn names_file(text: &str, f: &str) -> bool {
    let name = f.rsplit('/').next().unwrap_or(f);
    if name.is_empty() {
        return false;
    }
    let word = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/');
    let mut from = 0;
    while let Some(at) = text[from..].find(name) {
        let end = from + at + name.len();
        from = end;
        if text[end..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            continue;
        }
        let start = text[..end - name.len()].char_indices().rev().take_while(|(_, c)| word(*c)).last().map(|(i, _)| i).unwrap_or(end - name.len());
        let token = text[start..end].trim_start_matches("./");
        let ok = if token == name {
            name.contains('.') && !GENERIC.contains(&name)
        } else {
            token.ends_with(&format!("/{name}")) && (f == token || f.ends_with(&format!("/{token}")))
        };
        if ok {
            return true;
        }
    }
    false
}

/// Which of `files` an issue's words name, as file names.
pub fn named_files(text: &str, files: &[String]) -> Vec<String> {
    let mut out = vec![];
    for f in files {
        let name = f.rsplit('/').next().unwrap_or(f).to_string();
        if names_file(text, f) && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

fn issue_text(b: &Row) -> String {
    let snap = jloads_obj(b.s("snapshot"));
    [b.st("title"), b.st("detail"), snap.st("output")].join("\n")
}

/// The files the task's checkout changes: its worktree's, else the terminal's checkout's.
fn changed_files(app: &App, t: &Row, cwd: Option<&str>) -> Result<Vec<String>> {
    let wt = board::task_context(t).get("where").and_then(|w| w["worktree"].as_str()).map(str::to_string);
    let Some(dir) = [wt, cwd.map(str::to_string)].into_iter().flatten().find(|d| !d.is_empty() && Path::new(d).is_dir()) else {
        return Ok(vec![]);
    };
    Ok(crate::areas::checkout_files(&dir, &crate::steps::real_base(app, t)?))
}

/// The project's open backlog issues about the task's area or the files it changed, newest first, each
/// with why it's related. Issues the task found itself are left out: they were outside it.
pub fn related(app: &App, t: &Row, cwd: Option<&str>) -> Result<Vec<(Row, String)>> {
    let issues = app.db.q(
        "SELECT * FROM issues WHERE project = ? AND state IN ('open', 'defer') AND COALESCE(found_by_task, 0) != ? ORDER BY id DESC",
        crate::p![t.st("project"), t.id()],
    )?;
    if issues.is_empty() {
        return Ok(vec![]);
    }
    let files = changed_files(app, t, cwd)?;
    let area = t.s("area").filter(|a| !a.is_empty());
    let mut out = vec![];
    for b in issues {
        let names = named_files(&issue_text(&b), &files);
        let why = if !names.is_empty() {
            names.join(", ")
        } else if area.is_some() && b.s("area") == area {
            format!("area {}", area.unwrap_or(""))
        } else {
            continue;
        };
        out.push((b, why));
    }
    Ok(out)
}

/// The first `tb submit` of a task that names no fixes, with open issues about what it changed: refused once
/// with the list, so the agent can say which it fixed. The same submit again goes through.
pub fn ask_once(app: &App, t: &Row, cwd: Option<&str>) -> Result<()> {
    let mut ctx = board::task_context(t);
    if ctx.get("fixes_asked").is_some_and(|v| v == &json!(true)) {
        return Ok(());
    }
    let related = related(app, t, cwd)?;
    ctx.insert("fixes_asked".into(), json!(true));
    app.db.tx(|| board::save_context(app, t.id(), &ctx, false))?;
    if related.is_empty() {
        return Ok(());
    }
    let me = rf("task", t.id());
    let mut lines: Vec<String> =
        related.iter().take(SHOWN).map(|(b, why)| format!("- {} · {} ({why})", rf("issue", b.id()), one_line(&b.st("title"), 120))).collect();
    if related.len() > SHOWN {
        lines.push(format!("- and {} more", related.len() - SHOWN));
    }
    err(
        409,
        format!(
            "Before {me} finishes: these open backlog issues are about the area or files it changed.\n{}\n\
             If this work fixed any of them, run the same tb submit with --fixes B<n> for each: they close as fixed when its PR \
             merges (as it finishes, with no PR). If it fixed none, run the same tb submit again.",
            lines.join("\n")
        ),
    )
}

/// Keeps the issues a submit named on the task, with a line in its log.
pub fn record(app: &App, t: &Row, ids: &[i64], who: &str) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut all = of_task(t);
    for id in ids {
        if !all.contains(id) {
            all.push(*id);
        }
    }
    let mut ctx = board::task_context(t);
    ctx.insert("fixes".into(), json!(all));
    board::save_context(app, t.id(), &ctx, false)?;
    let refs: Vec<String> = ids.iter().map(|i| rf("issue", *i)).collect();
    board::log_event(app, t.id(), who, "status", &format!("Fixes {}: closes as fixed once it ships", refs.join(", ")))?;
    Ok(())
}

/// Closes the issues a task fixed once its work ships: its PR merged, or it finished with no PR. Called on
/// every watched change to a task (`board::update_task`).
pub fn on_change(app: &App, before: &Row, after: &Row) -> Result<()> {
    if after.s("status") != Some("done") || after.b("failed") {
        return Ok(());
    }
    let merged = after.s("pr_phase") == Some("merged") && before.s("pr_phase") != Some("merged");
    let no_pr = before.s("status") != Some("done") && after.i("pr_num").is_none() && !has(after.s("pr_url"));
    if !merged && !no_pr {
        return Ok(());
    }
    let ids = of_task(after);
    if ids.is_empty() {
        return Ok(());
    }
    let me = rf("task", after.id());
    let by = if merged { format!("{me} (PR #{})", after.i0("pr_num")) } else { format!("{me}, which finished without a PR") };
    let mut closed = vec![];
    for id in ids {
        let Some(b) = board::find_issue(app, Some(id))? else { continue };
        if !matches!(b.s("state"), Some("open" | "defer")) {
            continue;
        }
        app.db.update("issues", &json!(id), crate::fields!["state" => "drop", "updated_at" => now_iso()])?;
        board::add_issue_event(app, id, board::BOARD, "drop", &format!("Closed as fixed by {by}"), None)?;
        board::miss_settled(app, &b)?;
        closed.push(rf("issue", id));
    }
    if !closed.is_empty() {
        board::log_event(app, after.id(), board::BOARD, "status", &format!("Closed {} as fixed", closed.join(", ")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_issue_names_a_changed_file_by_path_or_specific_name() {
        let changed = files(&["crates/taskboard-cli/src/tb.rs", "crates/taskboardd/src/lib.rs", "crates/taskboardd/src/tidy.rs"]);
        assert_eq!(named_files("tb submit in crates/taskboard-cli/src/tb.rs:120 drops it", &changed), vec!["tb.rs"]);
        assert_eq!(named_files("tidy.rs never re-reads old issues", &changed), vec!["tidy.rs"]);
        assert!(named_files("lib.rs exports too much", &changed).is_empty(), "a generic name says nothing");
        assert!(named_files("xtb.rs and tb.rsx and src/tb.rs", &files(&["other/tb.rs"])).is_empty(), "only whole names, not another path's");
        assert_eq!(named_files("see src/tb.rs", &changed), vec!["tb.rs"], "a path that ends the file's own");
        assert_eq!(named_files("crates/taskboardd/src/lib.rs exports too much", &changed), vec!["lib.rs"], "a full path names even a generic file");
        assert!(named_files("nothing here", &changed).is_empty());
    }

    #[test]
    fn named_fixes_read_each_ref_once() {
        assert_eq!(named(&json!({"fixes": ["B3", "b4", 3]})).unwrap(), vec![3, 4]);
        assert!(named(&json!({"fixes": ["T3"]})).is_err());
        assert!(named(&json!({})).unwrap().is_empty());
    }
}
