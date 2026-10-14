//! Areas: the separate things inside one project's repo (Quill, Art, Remote), listed in a
//! `.taskboard.toml` at the repo's root, one level only:
//!
//! ```toml
//! [[areas]]
//! name = "Quill"
//! paths = ["apps/quill", "packages/quill_*"]
//! ```
//!
//! Tasks, goals and backlog issues carry an optional `area`, one of these names. The file is read
//! again whenever it changes. Everything else (PR rules, reviewers, terminal limits, Jira) stays per project.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, p};

/// The file at a project's repo root.
pub const FILE: &str = ".taskboard.toml";

#[derive(Debug, Clone, PartialEq)]
pub struct Area {
    pub name: String,
    /// Subdirectories or globs, relative to the repo root (`apps/quill`, `packages/quill_*`).
    pub paths: Vec<String>,
}

/// What a repo's `.taskboard.toml` says: its areas, and why any part of it was left out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Areas {
    pub list: Vec<Area>,
    pub problems: Vec<String>,
}

impl Areas {
    /// The area named `name`, any case.
    pub fn find(&self, name: &str) -> Option<&Area> {
        self.list.iter().find(|a| a.name.eq_ignore_ascii_case(name.trim()))
    }

    pub fn names(&self) -> Vec<&str> {
        self.list.iter().map(|a| a.name.as_str()).collect()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "areas": self.list.iter().map(|a| json!({"name": a.name, "paths": a.paths})).collect::<Vec<_>>(),
            "problems": self.problems,
        })
    }
}

/// Reads the areas out of `.taskboard.toml`'s text.
pub fn parse(text: &str) -> Areas {
    let mut out = Areas::default();
    let doc: toml::Value = match toml::from_str(text) {
        Ok(d) => d,
        Err(e) => {
            out.problems.push(format!("{FILE} isn't valid TOML: {}", one_line(&e.to_string(), 200)));
            return out;
        }
    };
    let Some(items) = doc.get("areas") else { return out };
    let Some(items) = items.as_array() else {
        out.problems.push(format!("{FILE}: areas should be a list ([[areas]])."));
        return out;
    };
    for (i, it) in items.iter().enumerate() {
        let name = it.get("name").and_then(|v| v.as_str()).map(str::trim).unwrap_or("");
        if name.is_empty() {
            out.problems.push(format!("{FILE}: area {} has no name.", i + 1));
            continue;
        }
        if name.eq_ignore_ascii_case("none") || name.eq_ignore_ascii_case("all") {
            out.problems.push(format!("{FILE}: “{name}” can't be an area's name."));
            continue;
        }
        if out.find(name).is_some() {
            out.problems.push(format!("{FILE}: the area “{name}” is listed twice; the first one counts."));
            continue;
        }
        let paths: Vec<String> = match it.get("paths") {
            None => vec![],
            Some(toml::Value::String(s)) => vec![s.clone()],
            Some(toml::Value::Array(a)) => a.iter().filter_map(|v| v.as_str()).map(str::to_string).collect(),
            Some(_) => {
                out.problems.push(format!("{FILE}: the area “{name}”'s paths should be a list of strings."));
                vec![]
            }
        };
        let paths: Vec<String> = paths.iter().map(|p| p.trim().trim_start_matches("./").trim_end_matches('/').to_string()).filter(|p| !p.is_empty()).collect();
        if paths.is_empty() {
            out.problems.push(format!("{FILE}: the area “{name}” has no paths."));
        }
        out.list.push(Area { name: name.to_string(), paths });
    }
    out
}

/// What each repo's file said, by when it last changed.
static CACHE: Lazy<Mutex<HashMap<PathBuf, (Option<(SystemTime, u64)>, Areas)>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// The areas in the repo at `repo`: none when it has no `.taskboard.toml`. Read again when the file changes.
pub fn of_repo(repo: &Path) -> Areas {
    let file = repo.join(FILE);
    let stamp = std::fs::metadata(&file).ok().map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()));
    let mut cache = CACHE.lock();
    if let Some((s, a)) = cache.get(&file) {
        if *s == stamp {
            return a.clone();
        }
    }
    let areas = match stamp {
        None => Areas::default(),
        Some(_) => match std::fs::read_to_string(&file) {
            Ok(text) => parse(&text),
            Err(e) => Areas { list: vec![], problems: vec![format!("Couldn't read {}: {e}", file.display())] },
        },
    };
    cache.insert(file, (stamp, areas.clone()));
    areas
}

/// The areas of the project named `project`, from its repo.
pub fn of_project(app: &App, project: &str) -> Result<Areas> {
    Ok(match crate::projects::project_path(app, Some(project))? {
        Some(p) => of_repo(Path::new(&p)),
        None => Areas::default(),
    })
}

/// An `area` from a request body for the project: the area's own name (any case matches), or null
/// for `none` or empty. Refused when the project has no such area.
pub fn clean(app: &App, project: &str, v: &Value) -> Result<Value> {
    let want = match v {
        Value::Null => return Ok(Value::Null),
        Value::String(s) => s.trim().to_string(),
        other => other.to_string(),
    };
    if want.is_empty() || want.eq_ignore_ascii_case("none") {
        return Ok(Value::Null);
    }
    let areas = of_project(app, project)?;
    match areas.find(&want) {
        Some(a) => Ok(json!(a.name)),
        None if areas.list.is_empty() => err(400, format!("{project} has no areas: list them in {FILE} at its repo's root ([[areas]] with a name and paths).")),
        None => err(400, format!("{project} has no area “{want}”. Its areas: {}.", areas.names().join(", "))),
    }
}

/// An area path or glob as a regex over repo-relative files: the path itself or anything under it.
/// `*` and `?` stay inside one folder; `**` crosses folders.
fn glob_re(pat: &str) -> Regex {
    let cs: Vec<char> = pat.chars().collect();
    let mut re = String::from("^");
    let mut i = 0;
    while i < cs.len() {
        match cs[i] {
            '*' if cs.get(i + 1) == Some(&'*') => {
                i += 1;
                if cs.get(i + 1) == Some(&'/') {
                    i += 1;
                    re.push_str("(?:.*/)?");
                } else {
                    re.push_str(".*");
                }
            }
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    re.push_str("(?:/.*)?$");
    Regex::new(&re).expect("every other character is escaped")
}

/// How specific a path is: its characters that aren't wildcards. A nested area's path beats the one around it.
fn specificity(pat: &str) -> usize {
    pat.chars().filter(|c| !matches!(c, '*' | '?')).count()
}

/// The area most of `files` (repo-relative) are in. Each file counts for the area with the most specific
/// path that covers it (for neither when two areas cover it equally); the area with the most files wins.
/// A tie, or no file in any area, is none: never a guess.
pub fn area_for(areas: &Areas, files: &[String]) -> Option<String> {
    let pats: Vec<(usize, Regex, usize)> =
        areas.list.iter().enumerate().flat_map(|(i, a)| a.paths.iter().map(move |p| (i, glob_re(p), specificity(p)))).collect();
    let mut counts = vec![0usize; areas.list.len()];
    for f in files {
        let f = f.trim().trim_start_matches("./");
        let (mut best, mut tied): (Option<(usize, usize)>, bool) = (None, false);
        for (i, re, n) in &pats {
            if !re.is_match(f) {
                continue;
            }
            match best {
                Some((bn, _)) if *n < bn => {}
                Some((bn, bi)) if *n == bn => tied |= *i != bi,
                _ => (best, tied) = (Some((*n, *i)), false),
            }
        }
        if let (Some((_, i)), false) = (best, tied) {
            counts[i] += 1;
        }
    }
    let top = *counts.iter().max()?;
    if top == 0 || counts.iter().filter(|c| **c == top).count() > 1 {
        return None;
    }
    counts.iter().position(|c| *c == top).map(|i| areas.list[i].name.clone())
}

/// The one name most of `names` share, or none on a tie.
fn most_shared(names: &[String]) -> Option<String> {
    let mut counts: Vec<(&str, usize)> = vec![];
    for n in names {
        match counts.iter_mut().find(|(x, _)| x == n) {
            Some((_, c)) => *c += 1,
            None => counts.push((n, 1)),
        }
    }
    let top = counts.iter().map(|(_, c)| *c).max()?;
    let mut winners = counts.iter().filter(|(_, c)| *c == top);
    let first = winners.next()?;
    winners.next().is_none().then(|| first.0.to_string())
}

/// Whether inference may write this row's area: never once it was set by hand (even to none).
fn may_infer(r: &Row) -> bool {
    match r.s("area_source") {
        Some(s) => s == "inferred",
        None => r.s("area").is_none(),
    }
}

/// Runs git in `dir`: its stdout, or none when it fails.
fn git(dir: &str, args: &[&str]) -> Option<String> {
    let bin = crate::proc::which("git")?;
    let mut all: Vec<String> = vec!["-C".into(), dir.into()];
    all.extend(args.iter().map(|a| a.to_string()));
    crate::proc::run(&bin, &all, None, 20.0).ok().filter(|o| o.code == Some(0)).map(|o| o.stdout)
}

fn lines(out: Option<String>) -> Vec<String> {
    out.unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// The files the checkout at `dir` changes, repo-relative: its commits since it left `base` (the remote's,
/// else the local branch) and what isn't committed yet. With none, the folder it works in, when that's
/// below the repo's root.
pub fn checkout_files(dir: &str, base: &str) -> Vec<String> {
    let since = |b: String| git(dir, &["diff", "--name-only", &format!("{b}...HEAD")]);
    let mut files = lines(since(format!("origin/{base}")).or_else(|| since(base.to_string())));
    files.extend(lines(git(dir, &["diff", "--name-only", "HEAD"])));
    files.extend(lines(git(dir, &["ls-files", "--others", "--exclude-standard"])));
    files.sort();
    files.dedup();
    if files.is_empty() {
        files.extend(lines(git(dir, &["rev-parse", "--show-prefix"])).into_iter().map(|p| p.trim_end_matches('/').to_string()));
    }
    files
}

/// Writes an inferred area on a task (none clears one inferred before), noting the change. Whether it changed.
fn set_task(app: &App, t: &Row, area: Option<&str>, from: &str) -> Result<bool> {
    if t.s("area") == area {
        return Ok(false);
    }
    app.db.update("tasks", &json!(t.id()), fields!["area" => area, "area_source" => area.map(|_| "inferred"), "updated_at" => now_iso()])?;
    let text = match area {
        Some(a) => format!("Area: {a}, from {from}"),
        None => format!("Cleared its area: {from} no longer point at one"),
    };
    board::log_event(app, t.id(), board::BOARD, "status", &text)?;
    Ok(true)
}

/// A goal's area, unless it was set by hand: the one most of its tasks share. The area it set, if it changed.
pub fn infer_goal(app: &App, goal_id: i64) -> Result<Option<Option<String>>> {
    let Some(g) = app.db.q("SELECT * FROM goals WHERE id = ?", p![goal_id])?.into_iter().next() else { return Ok(None) };
    if !may_infer(&g) {
        return Ok(None);
    }
    let names: Vec<String> = app.db.q("SELECT area FROM tasks WHERE goal_id = ? AND area IS NOT NULL", p![goal_id])?.iter().map(|r| r.st("area")).collect();
    let area = most_shared(&names);
    if g.s("area") == area.as_deref() {
        return Ok(None);
    }
    app.db.update("goals", &json!(goal_id), fields!["area" => area.clone(), "area_source" => area.as_ref().map(|_| "inferred"), "updated_at" => now_iso()])?;
    Ok(Some(area))
}

/// Re-infers an unfinished task's area as its commits land, from the checkout it works in (`cwd`, else
/// its worktree): the area most of the files it changes are in. An area set by hand always stays;
/// with nothing changed yet it waits.
pub fn infer_task(app: &App, task_id: i64, cwd: Option<&str>) -> Result<()> {
    let t = board::get_task(app, task_id)?;
    if !may_infer(&t) || t.s("status") == Some("done") {
        return Ok(());
    }
    let areas = of_project(app, &t.st("project"))?;
    if areas.list.is_empty() {
        return Ok(());
    }
    let wt = board::task_context(&t).get("where").and_then(|w| w["worktree"].as_str()).map(str::to_string);
    let Some(dir) = [cwd.map(str::to_string), wt].into_iter().flatten().find(|d| !d.is_empty() && Path::new(d).is_dir()) else {
        return Ok(());
    };
    let files = checkout_files(&dir, &crate::steps::real_base(app, &t)?);
    if files.is_empty() {
        return Ok(());
    }
    if set_task(app, &t, area_for(&areas, &files).as_deref(), "the files it changes")? {
        if let Some(g) = t.i("goal_id") {
            infer_goal(app, g)?;
        }
    }
    Ok(())
}

/// The files a task changed, from what the board recorded: its PR's files on the host, else its PR's
/// commits in the repo, else its worktree while that's still on the task's branch, else the folder its
/// repo path is in below the project's root. And where they came from.
fn recorded_files(app: &App, t: &Row, root: &str) -> (Vec<String>, &'static str) {
    if let Some(pr) = crate::prhost::PrRef::of(t) {
        let got = crate::prhost::host_for(app, &pr.host)
            .and_then(|h| crate::progress::waiting(&format!("PR #{}'s files", pr.num), crate::progress::WAITING_EVERY, || h.files(&pr)));
        match got {
            Ok(f) if !f.is_empty() => return (f, "its PR's files"),
            Ok(_) => {}
            Err(e) => app.info(format!("areas: couldn't list the files of {}'s PR #{}: {e}", rf("task", t.id()), pr.num)),
        }
    }
    let rec = jloads_obj(t.s("pr_flow")).get("rec").cloned().unwrap_or(Value::Null);
    let repo = t.s("repo_path").filter(|r| Path::new(r).is_dir()).unwrap_or(root);
    if rec.is_object() && !repo.is_empty() {
        let f = crate::picker::changed_files(repo, &rec);
        if !f.is_empty() {
            return (f, "its PR's commits");
        }
    }
    let w = board::task_context(t).get("where").cloned().unwrap_or(Value::Null);
    if let (Some(dir), Some(branch)) = (w["worktree"].as_str().filter(|d| Path::new(d).is_dir()), w["branch"].as_str()) {
        if lines(git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])).first().map(String::as_str) == Some(branch) {
            let f = checkout_files(dir, &crate::steps::real_base(app, t).unwrap_or_else(|_| "main".into()));
            if !f.is_empty() {
                return (f, "its worktree's changes");
            }
        }
    }
    let (repo, root) = (t.st("repo_path"), root.trim_end_matches('/'));
    let rel = repo.trim_end_matches('/').strip_prefix(root).and_then(|r| r.strip_prefix('/')).filter(|r| !r.is_empty() && !root.is_empty());
    match rel {
        Some(rel) => (vec![rel.to_string()], "its repo path"),
        None => (vec![], ""),
    }
}

/// `tb areas backfill`: infers the area of each task in the project (every project with areas when none
/// is named) whose area wasn't set by hand, from its recorded commits and PR files, then each goal's from
/// its tasks. What it set.
pub fn backfill(app: &App, body: &Value) -> Result<Value> {
    let named = body_str(body, "project");
    let projects: Vec<String> = if named.is_empty() {
        crate::projects::list_projects(app)?.iter().filter_map(|p| p["name"].as_str().map(str::to_string)).collect()
    } else {
        vec![named.clone()]
    };
    let (mut tasks, mut goals, mut unsure, mut looked) = (vec![], vec![], 0usize, vec![]);
    for project in &projects {
        let areas = of_project(app, project)?;
        if areas.list.is_empty() {
            if !named.is_empty() {
                return err(400, format!("{project} has no areas: list them in {FILE} at its repo's root ([[areas]] with a name and paths)."));
            }
            continue;
        }
        looked.push(project.clone());
        let root = crate::projects::project_path(app, Some(project))?.unwrap_or_default();
        for t in app.db.q("SELECT * FROM tasks WHERE project = ? ORDER BY id", p![project])? {
            if !may_infer(&t) {
                continue;
            }
            let (files, from) = recorded_files(app, &t, &root);
            if files.is_empty() {
                continue;
            }
            let area = area_for(&areas, &files);
            if area.is_none() {
                unsure += 1;
            }
            if set_task(app, &t, area.as_deref(), from)? {
                crate::progress::stage(&format!("{} → {}", rf("task", t.id()), area.as_deref().unwrap_or("none")));
                tasks.push(json!({"ref": rf("task", t.id()), "title": t.v("title"), "area": area, "from": from}));
            }
        }
        for g in app.db.q("SELECT id, name FROM goals WHERE project = ? ORDER BY id", p![project])? {
            if let Some(area) = infer_goal(app, g.id())? {
                goals.push(json!({"ref": rf("goal", g.id()), "name": g.v("name"), "area": area}));
            }
        }
    }
    Ok(json!({"projects": looked, "tasks": tasks, "goals": goals, "unsure": unsure}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_areas_with_paths_and_globs() {
        let a = parse(
            r#"
[[areas]]
name = "Quill"
paths = ["apps/quill/", "./packages/quill_*"]

[[areas]]
name = "Art"
paths = "apps/art"
"#,
        );
        assert_eq!(a.problems, Vec::<String>::new());
        assert_eq!(a.list, vec![
            Area { name: "Quill".into(), paths: vec!["apps/quill".into(), "packages/quill_*".into()] },
            Area { name: "Art".into(), paths: vec!["apps/art".into()] },
        ]);
        assert_eq!(a.find("quill").map(|x| x.name.as_str()), Some("Quill"));
        assert!(a.find("remote").is_none());
    }

    #[test]
    fn skips_bad_areas_and_says_why() {
        let a = parse("[[areas]]\npaths = [\"x\"]\n[[areas]]\nname = \"None\"\n[[areas]]\nname = \"A\"\npaths=[\"a\"]\n[[areas]]\nname = \"a\"\n");
        assert_eq!(a.names(), vec!["A"]);
        assert_eq!(a.problems.len(), 3);
        let bad = parse("[[areas]\n");
        assert!(bad.list.is_empty() && bad.problems[0].contains("isn't valid TOML"));
        assert_eq!(parse(""), Areas::default());
    }

    #[test]
    fn reads_the_file_again_when_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        assert!(of_repo(dir.path()).list.is_empty());
        std::fs::write(dir.path().join(FILE), "[[areas]]\nname = \"Quill\"\npaths = [\"apps/quill\"]\n").unwrap();
        assert_eq!(of_repo(dir.path()).names(), vec!["Quill"]);
        std::fs::write(dir.path().join(FILE), "[[areas]]\nname = \"Quill\"\npaths = [\"apps/quill\"]\n[[areas]]\nname = \"Remote\"\npaths = [\"apps/remote\"]\n").unwrap();
        assert_eq!(of_repo(dir.path()).names(), vec!["Quill", "Remote"]);
        std::fs::remove_file(dir.path().join(FILE)).unwrap();
        assert!(of_repo(dir.path()).list.is_empty());
    }

    fn areas(list: &[(&str, &[&str])]) -> Areas {
        let list = list.iter().map(|(n, ps)| Area { name: n.to_string(), paths: ps.iter().map(|p| p.to_string()).collect() }).collect();
        Areas { list, problems: vec![] }
    }

    fn strs(fs: &[&str]) -> Vec<String> {
        fs.iter().map(|f| f.to_string()).collect()
    }

    #[test]
    fn a_path_covers_itself_and_what_is_under_it() {
        let re = glob_re("apps/quill");
        assert!(re.is_match("apps/quill"));
        assert!(re.is_match("apps/quill/lib/main.dart"));
        assert!(!re.is_match("apps/quill_web/x"));
        assert!(!re.is_match("apps/quil"));
        assert!(!re.is_match("x/apps/quill/y"));
    }

    #[test]
    fn globs_stay_in_one_folder_unless_double_starred() {
        let one = glob_re("packages/quill_*");
        assert!(one.is_match("packages/quill_core/lib/a.dart"));
        assert!(!one.is_match("packages/art/quill_x"));
        let deep = glob_re("**/remote");
        assert!(deep.is_match("remote/a.rs") && deep.is_match("apps/x/remote/a.rs"));
        let any = glob_re("tools/**/*.sh");
        assert!(any.is_match("tools/a.sh") && any.is_match("tools/x/y/a.sh") && !any.is_match("tools/a.rs"));
        assert!(glob_re("apps/a?t").is_match("apps/art/b"));
        assert!(!glob_re("apps/a.t").is_match("apps/art/b"));
    }

    #[test]
    fn the_area_with_the_most_files_wins() {
        let a = areas(&[("Quill", &["apps/quill", "packages/quill_*"]), ("Art", &["apps/art"])]);
        assert_eq!(area_for(&a, &strs(&["apps/quill/a", "packages/quill_core/b", "apps/art/c", "README.md"])), Some("Quill".into()));
        assert_eq!(area_for(&a, &strs(&["./apps/art/c"])), Some("Art".into()));
    }

    #[test]
    fn a_nested_area_beats_the_one_around_it() {
        let a = areas(&[("Apps", &["apps"]), ("Quill", &["apps/quill"])]);
        assert_eq!(area_for(&a, &strs(&["apps/quill/a", "apps/quill/b", "apps/art/c"])), Some("Quill".into()));
        assert_eq!(area_for(&a, &strs(&["apps/art/c", "apps/remote/d", "apps/quill/a"])), Some("Apps".into()));
    }

    #[test]
    fn a_tie_or_no_match_is_blank() {
        let a = areas(&[("Quill", &["apps/quill"]), ("Art", &["apps/art"])]);
        assert_eq!(area_for(&a, &strs(&["apps/quill/a", "apps/art/b"])), None);
        assert_eq!(area_for(&a, &strs(&["docs/x.md", "Cargo.toml"])), None);
        assert_eq!(area_for(&a, &[]), None);
        // A file two areas cover equally counts for neither.
        let both = areas(&[("Quill", &["shared"]), ("Art", &["shared"])]);
        assert_eq!(area_for(&both, &strs(&["shared/a"])), None);
    }

    #[test]
    fn a_goal_takes_the_area_most_of_its_tasks_share() {
        assert_eq!(most_shared(&strs(&["Quill", "Art", "Quill"])), Some("Quill".into()));
        assert_eq!(most_shared(&strs(&["Quill", "Art"])), None);
        assert_eq!(most_shared(&[]), None);
    }

    #[test]
    fn inference_never_overrides_an_area_set_by_hand() {
        let row = |area: Option<&str>, source: Option<&str>| -> Row { json!({"area": area, "area_source": source}).as_object().unwrap().clone() };
        assert!(may_infer(&row(None, None)));
        assert!(may_infer(&row(Some("Quill"), Some("inferred"))));
        assert!(!may_infer(&row(Some("Quill"), Some("set"))));
        assert!(!may_infer(&row(None, Some("set"))));
        assert!(!may_infer(&row(Some("Quill"), None)));
    }

    #[test]
    fn a_checkout_changes_its_commits_and_uncommitted_files() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_str().unwrap();
        let run = |args: &[&str]| assert!(git(d, args).is_some(), "git {args:?}");
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::create_dir_all(dir.path().join("apps/quill")).unwrap();
        std::fs::write(dir.path().join("README.md"), "x").unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "base"]);
        run(&["checkout", "-qb", "work"]);
        std::fs::write(dir.path().join("apps/quill/a.dart"), "x").unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "work"]);
        std::fs::write(dir.path().join("apps/quill/b.dart"), "x").unwrap();
        assert_eq!(checkout_files(d, "main"), strs(&["apps/quill/a.dart", "apps/quill/b.dart"]));
    }
}
