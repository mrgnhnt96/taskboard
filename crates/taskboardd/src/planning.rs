//! How a goal's work is sliced into tasks: the board's built-in rules, unless `[planning]` names a
//! file of the owner's own for the whole board or for one project. Planners get it in their prompt and
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
    /// Empty, missing or blank: the built-in rules.
    pub guide: String,
    /// `[planning.projects]`: project name → its own file, in place of the board's.
    pub projects: BTreeMap<String, String>,
}

/// The rules a planner follows when no file of the owner's says otherwise.
pub const BUILT_IN: &str = "\
- Each task that changes code ends in one PR. It holds finished code that builds, breaks nothing that works today, and can be merged on its own. It can be several commits.
- Each task is small enough for one terminal.
- Aim for PRs that are easy to review and have a small impact, not for as many PRs as possible.
- List the tasks in the order they should run; a task that needs an earlier one's code goes after it.";

/// Where a guide came from, and why a file set for it wasn't used.
struct Pick {
    text: String,
    from: &'static str,
    path: Option<String>,
    skipped: Vec<Value>,
}

fn pick(app: &App, project: Option<&str>) -> Pick {
    let p = &app.cfg.planning;
    let own = project.filter(|n| !n.is_empty()).and_then(|n| p.projects.get(n));
    let mut skipped = vec![];
    for (from, path) in [("project", own.map(|s| s.as_str()).unwrap_or("")), ("board", p.guide.as_str())] {
        match config_file(path) {
            None => {}
            Some(Ok(text)) if !text.trim().is_empty() => {
                return Pick { text: text.trim().to_string(), from, path: Some(path.trim().to_string()), skipped };
            }
            Some(Ok(_)) => skipped.push(json!({"from": from, "path": path.trim(), "error": "it's empty"})),
            Some(Err(e)) => skipped.push(json!({"from": from, "path": path.trim(), "error": e})),
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
    Ok(json!({"project": project, "from": p.from, "path": p.path, "text": p.text, "skipped": p.skipped}))
}

/// The rules in force for a project's goals (its own file, else the board's, else the built-in ones)
/// under a heading naming whose they are, for a planner's prompt.
pub fn block(app: &App, project: Option<&str>) -> String {
    let p = pick(app, project);
    let heading = match p.from {
        "project" => "How this project slices its work (the owner's rules, in place of the board's):",
        "board" => "How this board slices its work (the owner's rules, in place of the built-in ones):",
        _ => "How to slice the work:",
    };
    format!("{heading}\n{}", p.text)
}
