//! How a goal's work is sliced into tasks: the board's built-in rules, unless the owner wrote their own in
//! the board's folder (`planning.md` for the board, `planning/<project>.md` for one project) or named a
//! file elsewhere under `[planning]`. Planners get it in their prompt and
//! from `tb planning`; the skill's `planning.md` keeps the mechanics (waves, locks, `tb propose`).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::Query;
use crate::app::App;
use crate::jira::config_file;
use crate::util::Result;

/// `[planning]` in config.toml.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct PlanningConfig {
    /// A markdown file of how this board slices goals into tasks, in place of the built-in rules.
    /// Empty: `planning.md` in the board's folder, when it's there with something in it.
    pub guide: String,
    /// `[planning.projects]`: project name → its own file, in place of the board's. Not named:
    /// `planning/<project>.md` in the board's folder, when it's there with something in it.
    pub projects: BTreeMap<String, String>,
}

/// The rules a planner follows when no file of the owner's says otherwise.
pub const BUILT_IN: &str = "\
- Each task that changes code ends in one PR. It holds finished code that builds, breaks nothing that works today, and can be merged on its own. It can be several commits.
- Each task is small enough for one terminal.
- Aim for PRs that are easy to review and have a small impact, not for as many PRs as possible.
- List the tasks in the order they should run; a task that needs an earlier one's code goes after it.
- Stack a task on another only when it needs that task's unmerged code; otherwise it branches from main.
- Tail merge a stack (the default) when each PR builds, breaks nothing and is safe to ship alone: main gets reviewed pieces one at a time.
- Head merge a stack when the pieces only make sense together (a migration and the code that needs it, a feature that's half there until the last PR) or main would be in a state you wouldn't ship between them: each PR merges into the one below it, review stays small, and main gets it all in one merge.
- Set the mode with --stack-merge and say it in the tasks' detail.";

/// Where a guide came from, and why a file set for it wasn't used.
struct Pick {
    text: String,
    from: &'static str,
    path: Option<String>,
    skipped: Vec<Value>,
}

/// A file the owner named in config.toml, else the one in its usual place in the board's folder
/// (`planning.md`, `planning/<project>.md`), which is used only when it's there with something in it.
fn file_for(app: &App, project: Option<&str>) -> Option<(String, bool)> {
    let p = &app.cfg.planning;
    let (named, usual) = match project {
        Some(n) => (p.projects.get(n), Some(n).filter(|n| !n.contains(['/', '\\']) && !n.starts_with('.')).map(|n| app.cfg.data.join("planning").join(format!("{n}.md")))),
        None => (Some(&p.guide), Some(app.cfg.data.join("planning.md"))),
    };
    match named.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(path) => Some((path.to_string(), true)),
        None => usual.map(|u| (u.to_string_lossy().to_string(), false)),
    }
}

fn pick(app: &App, project: Option<&str>) -> Pick {
    let project = project.filter(|n| !n.is_empty());
    let mut skipped = vec![];
    let candidates = [project.and_then(|n| file_for(app, Some(n))).map(|f| ("project", f)), file_for(app, None).map(|f| ("board", f))];
    for (from, (path, named)) in candidates.into_iter().flatten() {
        match config_file(&path) {
            None => {}
            Some(Ok(text)) if !text.trim().is_empty() => return Pick { text: text.trim().to_string(), from, path: Some(path), skipped },
            Some(Ok(_)) if named => skipped.push(json!({"from": from, "path": path, "error": "it's empty"})),
            Some(Err(e)) if named => skipped.push(json!({"from": from, "path": path, "error": e})),
            Some(_) => {}
        }
    }
    Pick { text: BUILT_IN.to_string(), from: "built-in", path: None, skipped }
}

/// `GET /planning?project=<name>` (or `?cwd=<folder>` for the project it's in): the rules in force,
/// where they came from, and any file set for them but not used.
pub fn get(app: &App, query: &Query) -> Result<Value> {
    let arg = |k: &str| query.get(k).map(|s| s.as_str()).filter(|s| !s.is_empty());
    let project = match arg("project") {
        Some(p) => Some(p.to_string()),
        None => crate::projects::project_for_path(app, arg("cwd"))?,
    };
    let p = pick(app, project.as_deref());
    let lessons = crate::lessons::kept(app, project.as_deref(), &["planning"])?;
    Ok(json!({"project": project, "from": p.from, "path": p.path, "text": p.text, "skipped": p.skipped,
              "lessons": crate::lessons::lines(&lessons)}))
}

/// The rules in force for a project's goals (its own file, else the board's, else the built-in ones)
/// under a heading naming whose they are, then the kept planning lessons, for a planner's prompt.
pub fn block(app: &App, project: Option<&str>) -> String {
    let p = pick(app, project);
    let heading = match p.from {
        "project" => "How this project slices its work (the owner's rules, in place of the board's):",
        "board" => "How this board slices its work (the owner's rules, in place of the built-in ones):",
        _ => "How to slice the work:",
    };
    let mut out = format!("{heading}\n{}", p.text);
    if let Ok(Some(l)) = crate::lessons::planning_block(app, project) {
        out += &format!("\n\n{l}");
    }
    out
}
