//! How a goal's work is sliced into tasks: the board's built-in rules, unless the owner wrote their own in
//! the board's folder (`planning.md` for the board, `planning/<project>.md` for one project) or named a
//! file elsewhere under `[planning]`. Planners get it in their prompt and
//! from `tb planning`, which also names the models small and large run while `[models]` is on; the
//! skill's `planning.md` keeps the mechanics (waves, locks, `tb propose`).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::Query;
use crate::app::App;
use crate::jira::config_file;
use crate::util::{Result, RowExt};

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
- A separate task is a separate review, not a separate PR to main: every PR to main runs the full checks, and so does every push to it.
- Small, related tasks share one stack: the smallest of them is the head, in the first wave, and each other one stacks on it (--stack-on) with --stack-merge head, so only the head's PR goes to main. Related means the same screen family, the same subsystem or the same kind of fix, even when the tasks don't need each other's code.
- Fan out, don't chain: stack each sibling on the head, not on the sibling before it, so the siblings still run side by side.
- A change too small to be worth a full check run (a one-line pin, a doc-only edit) isn't its own PR to main: fold it into a related stack.
- The size rule is for the whole stack: when one person can't read the head's combined diff, start a second stack.
- Head merge a stack when its pieces belong together or only make sense together (a migration and the code that needs it, a feature that's half there until the last PR): each PR merges into the one below it, review stays small, and main gets it all in one merge.
- Tail merge a stack (--stack-merge tail) only for a piece that must reach main before the rest, say because another goal waits on it: once its parent merges, each child is retargeted to main and runs the full checks there, so each must build, break nothing and be safe to ship alone.
- Otherwise stack a task on another only when it needs that task's unmerged code; else it branches from main.
- Set the mode with --stack-merge, unless it's the project's default (tb planning names it), and say it in the tasks' detail.
- PRs into a branch other than main often run no checks, so each stacked task runs the tests its change affects locally before it opens its PR. A PR into main is pushed only once its tests pass locally.
- Push once per review round: commit each fix locally, then push them together. Rebase a branch with an open PR only when it conflicts or right before it merges; a rebase is a push.
- Queue a manual check run only on the owner's word, and at most one before-and-after pair per experiment.";

/// Where each task works while its wave mates run (`worklane.rs`): sent with whatever rules are in force,
/// since an owner's planning file says how to slice work, not how the board places it.
pub const LANE_RULES: &str = "\
- shared: works in the project's shared checkout; fits small tasks whose files no wave mate touches. One task of a wave has it at a time, since each task makes its own branch.
- slot: works in a worktree slot from the board's pool, on its own branch; fits tasks whose files overlap a wave mate's.
- alone: runs with nothing else in its goal beside it (--alone board: nothing else on the board).
- auto (the default): a slot when a wave runs it beside others and either side plans no files, their files overlap, or a wave mate has the shared checkout (the first task of the wave gets it); else shared. A goal's lane (tb goal set G<n> --lane, or --worktrees, which is slot) is what its tasks on auto take.
- Give a task's files in its wave (\"title::detail::<wave>::::src/a.rs\") so auto can tell; set a lane with \"::lane=<lane>\" after a --task, tb task new|set --lane, or tb goal set --lane.";

/// The built-in rule for each task's model, added to [`BUILT_IN`] while `[models]` is on.
pub const MODEL_RULE: &str = "\
- Give every task a tier and say why in one line of its detail: --model small (or \"::model=small\" on a --task) runs sonnet at medium effort, for fully specified, few-file, mechanical, copy or test work; --model large runs opus at high effort, for design, cross-crate, concurrency, daemon or GPUI rendering, release or signing work, or a vague spec. If the detail names the files and the behaviour, it's small.
- Set --effort (or \"::effort=high\") only when a task needs more or less than its tier's: --effort xhigh for gnarly debugging, --effort low for trivial copy edits.";

/// The built-in rules, with the model rule while the board picks models (`[models] enabled`).
fn built_in(app: &App) -> String {
    if app.cfg.models.enabled { format!("{BUILT_IN}\n{MODEL_RULE}") } else { BUILT_IN.to_string() }
}

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
    Pick { text: built_in(app), from: "built-in", path: None, skipped }
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
    let lessons = crate::lessons::unwritten_planning(app, project.as_deref(), &p.text)?;
    let m = &app.cfg.models;
    let models = m.enabled.then(|| {
        json!({"small": m.resolve(Some("small")), "large": m.resolve(Some("large")), "default": m.resolve(None),
               "small_effort": m.resolve_effort(Some("small"), None), "large_effort": m.resolve_effort(Some("large"), None),
               "default_effort": m.resolve_effort(None, None)})
    });
    let stack_merge = project.as_deref().map(|n| json!({"default": crate::projects::stack_merge_default(app, n), "set": crate::projects::stack_merge_set(app, n).is_some()}));
    Ok(json!({"project": project, "from": p.from, "path": p.path, "text": p.text, "skipped": p.skipped,
              "lessons": crate::lessons::lines(&lessons), "models": models, "lanes": LANE_RULES, "stack_merge": stack_merge}))
}

/// The rules in force for a project's goals (its own file, else the board's, else the built-in ones)
/// under a heading naming whose they are, then the kept planning lessons that fit what's being
/// planned (`about`), for a planner's prompt.
pub fn block(app: &App, project: Option<&str>, about: &str) -> String {
    let p = pick(app, project);
    let heading = match p.from {
        "project" => "How this project slices its work (the owner's rules, in place of the board's):",
        "board" => "How this board slices its work (the owner's rules, in place of the built-in ones):",
        _ => "How to slice the work:",
    };
    let mut out = format!("{heading}\n{}", p.text);
    if let Some(line) = project.and_then(|n| stack_merge_line(app, n)) {
        out += &format!("\n{line}");
    }
    if let Ok(Some(l)) = crate::lessons::planning_block(app, project, about, &p.text) {
        out += &format!("\n\n{l}");
    }
    out
}

/// The project's own stack-merge default, for a planner: what --stack-on does without --stack-merge.
/// Nothing while the project hasn't set one.
fn stack_merge_line(app: &App, project: &str) -> Option<String> {
    let mode = crate::projects::stack_merge_set(app, project)?;
    let other = if mode == "head" { "tail" } else { "head" };
    Some(format!("- {project} {mode}-merges its stacks by default: --stack-on without --stack-merge is --stack-merge {mode}, and a task that {other}-merges says why in its detail."))
}

/// The heading accepted planning lessons go under in a planning file.
const LESSONS_HEADING: &str = "## Learned from finished goals";

/// The project's planning file with its kept planning lessons written in: the file as it is (or,
/// when the project has none yet, the rules in force now, so the new file keeps them), with each
/// lesson not in it yet added under one heading, its ref kept so it isn't added twice.
fn proposal(app: &App, project: &str) -> Result<Value> {
    let Some((path, _)) = file_for(app, Some(project)) else {
        return crate::util::err(400, format!("“{project}” can't name a planning file."));
    };
    let current = match config_file(&path) {
        Some(Ok(t)) if !t.trim().is_empty() => Some(t),
        _ => None,
    };
    let base = current.clone().unwrap_or_else(|| pick(app, Some(project)).text);
    let add = crate::lessons::unwritten_planning(app, Some(project), &base)?;
    let lines: Vec<String> = add.iter().map(|l| format!("- {} ({})", l.st("text").trim(), crate::util::rf("lesson", l.id()))).collect();
    let mut text = base.trim_end().to_string();
    if !lines.is_empty() {
        if !text.contains(LESSONS_HEADING) {
            text += &format!("\n\n{LESSONS_HEADING}");
        }
        text += &format!("\n{}", lines.join("\n"));
    }
    text.push('\n');
    Ok(json!({"project": project, "path": path, "exists": current.is_some(), "add": lines,
              "lessons": add.iter().map(|l| crate::util::rf("lesson", l.id())).collect::<Vec<_>>(), "text": text}))
}

fn project_arg(app: &App, project: &str, cwd: Option<&str>) -> Result<String> {
    let p = if project.is_empty() { crate::projects::project_for_path(app, cwd)? } else { Some(project.to_string()) };
    p.ok_or_else(|| crate::util::ApiError::new(400, "Say which project's planning file (--project)."))
}

/// `GET /planning/lessons?project=<name>` (or `?cwd=`): the edit to the project's planning file that
/// writes its kept planning lessons in, for the owner to look over.
pub fn suggest(app: &App, query: &Query) -> Result<Value> {
    let arg = |k: &str| query.get(k).map(|s| s.as_str()).unwrap_or("");
    proposal(app, &project_arg(app, arg("project"), Some(arg("cwd")).filter(|s| !s.is_empty()))?)
}

/// `POST /planning/lessons {project | cwd}`: the owner accepted the edit; write the file.
pub fn accept(app: &App, body: &Value) -> Result<Value> {
    let project = project_arg(app, body["project"].as_str().unwrap_or(""), body["cwd"].as_str())?;
    let mut v = proposal(app, &project)?;
    if v["add"].as_array().is_none_or(|a| a.is_empty()) {
        v["written"] = json!(false);
        return Ok(v);
    }
    let path = crate::util::expand_home(v["path"].as_str().unwrap_or(""));
    let path = std::path::Path::new(&path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| crate::util::ApiError::new(500, format!("can't make {}: {e}", dir.display())))?;
    }
    std::fs::write(path, v["text"].as_str().unwrap_or("")).map_err(|e| crate::util::ApiError::new(500, format!("can't write {}: {e}", path.display())))?;
    v["written"] = json!(true);
    Ok(v)
}
