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
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;

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
}
