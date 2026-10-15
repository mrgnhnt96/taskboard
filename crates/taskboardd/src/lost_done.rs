//! A lost task whose work shipped anyway: its terminal closed before it handed in, and the work
//! finished from another terminal. Each PR poll looks in every lost task's project for signs of that
//! since the terminal closed: for a release task, its version's tag, release commit and published
//! release; for any task, commits that name it and merged PRs from its branch. What it finds goes on
//! the card as `looks_finished`, so the card says "Looks finished" with the evidence instead of only
//! offering a restart.

use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, p};

/// `{"lost_at", "checked_at", "found": [{"what", "at", "url"?}]}`, set when the task is lost
/// (`board::mark_lost`) and filled in by `sweep`.
pub const ADDED: &[(&str, &str, &str)] = &[("tasks", "looks_finished", "TEXT")];

/// How often a lost task's project is looked at again.
const EVERY_SECS: f64 = 600.0;
/// How many of the repository's newest commits are read for ones that name the task.
const SCAN: usize = 400;

/// What a task gets when its terminal is lost: when, and nothing found yet.
pub fn lost_now() -> String {
    jdumps(&json!({"lost_at": now_iso()}))
}

/// The card's `looks_finished`: what was found, while the task is still lost; null when nothing was.
pub fn card(t: &Row) -> Value {
    if !t.b("lost") {
        return Value::Null;
    }
    let found = jloads_obj(t.s("looks_finished")).get("found").cloned().unwrap_or(Value::Null);
    if found.as_array().is_some_and(|a| !a.is_empty()) {
        found
    } else {
        Value::Null
    }
}

/// Looks again at each lost task that's due. Outside a transaction: git and gh may take a while.
pub fn sweep(app: &App) -> Result<()> {
    for t in app.db.q("SELECT * FROM tasks WHERE lost = 1 AND status = 'needs'", p![])? {
        let mut lf = jloads_obj(t.s("looks_finished"));
        if age_secs(lf.get("checked_at").and_then(|v| v.as_str())).is_some_and(|a| a < EVERY_SECS) {
            continue;
        }
        let lost_at = match lf.get("lost_at").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => lost_at(app, &t)?,
        };
        let found = look(app, &t, &lost_at);
        let had = lf.get("found").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
        lf.insert("lost_at".into(), json!(lost_at));
        lf.insert("checked_at".into(), json!(now_iso()));
        lf.insert("found".into(), Value::Array(found.clone()));
        app.db.tx(|| {
            // Not `update_task`: the card's "Terminal closed" time is the task's `updated_at`.
            app.db.update("tasks", &json!(t.id()), fields!["looks_finished" => jdumps(&Value::Object(lf.clone()))])?;
            if had == 0 && !found.is_empty() {
                let what: Vec<&str> = found.iter().filter_map(|f| f["what"].as_str()).collect();
                board::log_event(app, t.id(), board::BOARD, "status", &format!("Looks finished since its terminal closed: {}", what.join("; ")))?;
            }
            Ok(())
        })?;
    }
    Ok(())
}

/// When a task lost before `looks_finished` was kept lost its terminal: its last "Terminal ended" line.
fn lost_at(app: &App, t: &Row) -> Result<String> {
    let at = app.db.q(
        "SELECT at FROM events WHERE task_id = ? AND kind = 'status' AND text LIKE 'Terminal ended%' ORDER BY at DESC LIMIT 1",
        p![t.id()],
    )?;
    Ok(at.first().and_then(|r| r.s("at")).or(t.s("updated_at")).unwrap_or_default().to_string())
}

/// Signs in the task's project that its work finished after `since`, newest last.
fn look(app: &App, t: &Row, since: &str) -> Vec<Value> {
    let repo = t.st("repo_path");
    let Some(since_ts) = parse_iso(since) else { return vec![] };
    if repo.is_empty() || !Path::new(&repo).is_dir() {
        return vec![];
    }
    let mut found = vec![];
    let version = release_version(&t.st("title"));
    if let Some(v) = &version {
        found.extend(tag(&repo, v, since_ts));
        found.extend(gh_release(app, &repo, v, since_ts));
    }
    // A task that worked on the main branch has no branch of its own to follow.
    let branch = crate::waitsfor::branch_of(t).filter(|b| !matches!(b.as_str(), "main" | "master" | "develop" | "HEAD"));
    found.extend(commits(&repo, t.id(), branch.as_deref(), version.as_deref(), since_ts));
    if let Some(b) = &branch {
        found.extend(gh_merged(app, &repo, b, since_ts));
    }
    found.sort_by(|a, b| a["at"].as_str().cmp(&b["at"].as_str()));
    found
}

static RELEASE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\brelease\b.*?\bv?(\d+\.\d+\.\d+(?:-[0-9A-Za-z][0-9A-Za-z.-]*)?)").unwrap());

/// The version a release task ships ("Release midna v0.1.0-beta.21" → `0.1.0-beta.21`).
pub fn release_version(title: &str) -> Option<String> {
    RELEASE_RE.captures(title).map(|c| c[1].trim_end_matches('.').to_string())
}

fn git(repo: &str, args: &[&str]) -> Option<String> {
    let git = crate::proc::which("git")?;
    let mut all: Vec<String> = vec!["-C".into(), repo.into()];
    all.extend(args.iter().map(|a| a.to_string()));
    let o = crate::proc::run(&git, &all, None, 15.0).ok().filter(|o| o.code == Some(0))?;
    Some(o.stdout)
}

fn after(at: &str, since: f64) -> bool {
    parse_iso(at).is_some_and(|a| a >= since)
}

/// The version's tag (`v<version>` or `<version>`), made after `since`.
fn tag(repo: &str, version: &str, since: f64) -> Option<Value> {
    for name in [format!("v{version}"), version.to_string()] {
        let at = git(repo, &["for-each-ref", "--format=%(creatordate:iso-strict)", &format!("refs/tags/{name}")]).unwrap_or_default();
        let at = at.trim();
        if !at.is_empty() {
            return after(at, since).then(|| json!({"what": format!("Tag {name}"), "at": iso_of(at)}));
        }
    }
    None
}

/// The commits since `since` that name the task, merge its branch, or (for a release) name its version.
fn commits(repo: &str, task: i64, branch: Option<&str>, version: Option<&str>, since: f64) -> Vec<Value> {
    let since_arg = format!("--since={}", iso(since));
    let log = git(repo, &["log", "--all", &since_arg, "-n", &SCAN.to_string(), "--format=%h%x1f%cI%x1f%s"]).unwrap_or_default();
    let names_task = Regex::new(&format!(r"(?i)\bT{task}\b")).unwrap();
    // Branches the board's tasks make start with the task's number: `feat/361-…`, `fix/t361-…`.
    let task_branch = Regex::new(&format!(r"(?i)\bfrom \S*/t?{task}-\S+")).unwrap();
    let mut out = vec![];
    for line in log.lines() {
        let mut it = line.splitn(3, '\x1f');
        let (Some(sha), Some(at), Some(subject)) = (it.next(), it.next(), it.next()) else { continue };
        if !after(at, since) {
            continue;
        }
        let merges = subject.starts_with("Merge ")
            && (task_branch.is_match(subject) || branch.is_some_and(|b| subject.split_whitespace().any(|w| w == b || w.ends_with(&format!("/{b}")))));
        let releases = version.is_some_and(|v| subject.split(|c: char| c.is_whitespace() || c == ',').any(|w| w.trim_start_matches('v') == v));
        if names_task.is_match(subject) || merges || releases {
            out.push(json!({"what": format!("Commit {sha}: {}", one_line(subject, 120)), "at": iso_of(at)}));
        }
        if out.len() >= 3 {
            break;
        }
    }
    out
}

fn gh(app: &App, repo: &str, args: &[&str]) -> Option<Value> {
    let gh = crate::proc::which(&app.cfg.pr.gh)?;
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let o = crate::proc::run_with(&gh, &args, Some(Path::new(repo)), 20.0, &crate::accounts::gh_env(&app.cfg), None).ok()?;
    if o.code != Some(0) {
        return None;
    }
    serde_json::from_str(&o.stdout).ok()
}

/// The version's GitHub release, published after `since`.
fn gh_release(app: &App, repo: &str, version: &str, since: f64) -> Option<Value> {
    for name in [format!("v{version}"), version.to_string()] {
        let Some(r) = gh(app, repo, &["release", "view", &name, "--json", "publishedAt,url,isDraft"]) else { continue };
        let at = r["publishedAt"].as_str().unwrap_or_default();
        if r["isDraft"] == true || !after(at, since) {
            return None;
        }
        return Some(json!({"what": format!("Release {name} published"), "at": at, "url": r["url"]}));
    }
    None
}

/// PRs from the task's branch merged after `since`.
fn gh_merged(app: &App, repo: &str, branch: &str, since: f64) -> Vec<Value> {
    let Some(prs) = gh(app, repo, &["pr", "list", "--state", "merged", "--head", branch, "--json", "number,title,url,mergedAt"]) else {
        return vec![];
    };
    prs.as_array()
        .into_iter()
        .flatten()
        .filter(|pr| pr["mergedAt"].as_str().is_some_and(|at| after(at, since)))
        .map(|pr| json!({"what": format!("PR #{} merged: {}", pr["number"], one_line(pr["title"].as_str().unwrap_or(""), 120)), "at": pr["mergedAt"], "url": pr["url"]}))
        .collect()
}

/// A git date as the board's UTC ISO time.
fn iso_of(at: &str) -> String {
    parse_iso(at).map(iso).unwrap_or_else(|| at.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn release_titles_name_their_version() {
        assert_eq!(release_version("Release midna v0.1.0-beta.21").as_deref(), Some("0.1.0-beta.21"));
        assert_eq!(release_version("release 2.3.4.").as_deref(), Some("2.3.4"));
        assert_eq!(release_version("Cut the release notes"), None);
        assert_eq!(release_version("Bump to v1.2.3"), None);
    }

    fn sh(dir: &Path, args: &[&str], date: &str) {
        let o = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }

    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        sh(d.path(), &["init", "-q", "-b", "main"], "2026-10-15T20:00:00Z");
        sh(d.path(), &["commit", "-q", "--allow-empty", "-m", "chore: release v0.1.0-beta.20"], "2026-10-15T20:00:00Z");
        sh(d.path(), &["tag", "-a", "v0.1.0-beta.20", "-m", "v0.1.0-beta.20"], "2026-10-15T20:00:00Z");
        d
    }

    fn app(dir: &Path) -> std::sync::Arc<App> {
        let mut cfg = Config::for_tests(dir);
        cfg.pr.gh = "/nonexistent/gh".into();
        App::for_tests(cfg)
    }

    fn lost_task(app: &App, title: &str, repo: &Path, lost_at: &str, context: Value) -> i64 {
        app.db
            .insert(
                "tasks",
                fields!["title" => title, "repo_path" => repo.to_string_lossy(), "status" => "needs", "lost" => 1,
                        "needs_reason" => "lost", "context" => jdumps(&context),
                        "looks_finished" => jdumps(&json!({"lost_at": lost_at}))],
            )
            .unwrap()
    }

    #[test]
    fn a_release_shipped_after_its_terminal_closed_looks_finished() {
        let r = repo();
        let data = tempfile::tempdir().unwrap();
        let app = app(data.path());
        let id = lost_task(&app, "Release midna v0.1.0-beta.21", r.path(), "2026-10-15T21:25:00Z", json!({}));
        sweep(&app).unwrap();
        assert!(card(&board::get_task(&app, id).unwrap()).is_null());

        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", "chore: release v0.1.0-beta.21"], "2026-10-15T22:21:00Z");
        sh(r.path(), &["tag", "-a", "v0.1.0-beta.21", "-m", "v0.1.0-beta.21"], "2026-10-15T22:21:30Z");
        app.db.x("UPDATE tasks SET looks_finished = json_remove(looks_finished, '$.checked_at') WHERE id = ?", p![id]).unwrap();
        sweep(&app).unwrap();
        let t = board::get_task(&app, id).unwrap();
        let found = card(&t);
        let what: Vec<&str> = found.as_array().unwrap().iter().map(|f| f["what"].as_str().unwrap()).collect();
        assert_eq!(what.len(), 2, "{found}");
        assert!(what[0].starts_with("Commit ") && what[0].ends_with("chore: release v0.1.0-beta.21"), "{found}");
        assert_eq!(what[1], "Tag v0.1.0-beta.21");
        assert_eq!(found[0]["at"], "2026-10-15T22:21:00Z");
        let log = app.db.q("SELECT text FROM events WHERE task_id = ?", p![id]).unwrap();
        assert!(log.iter().any(|e| e.st("text").starts_with("Looks finished since its terminal closed: Commit ")));
    }

    #[test]
    fn commits_that_name_the_task_or_merge_its_branch_count_only_after_the_close() {
        let r = repo();
        let data = tempfile::tempdir().unwrap();
        let app = app(data.path());
        let id = lost_task(&app, "Lost cards notice finished work", r.path(), "2026-10-15T21:00:00Z", json!({}));
        let other = format!("Merge pull request #9 from me/feat/{}0-other", id);
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &format!("wip T{id}")], "2026-10-15T20:30:00Z");
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &other], "2026-10-15T21:10:00Z");
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &format!("Merge pull request #10 from me/feat/{id}-lost-cards")], "2026-10-15T21:20:00Z");
        sweep(&app).unwrap();
        let found = card(&board::get_task(&app, id).unwrap());
        let what: Vec<&str> = found.as_array().unwrap().iter().map(|f| f["what"].as_str().unwrap()).collect();
        assert_eq!(what.len(), 1, "{found}");
        assert!(what[0].ends_with(&format!("from me/feat/{id}-lost-cards")), "{found}");
    }

    #[test]
    fn a_task_back_from_lost_shows_nothing() {
        let t: Row = jloads_obj(Some(&jdumps(&json!({"lost": 0, "looks_finished": jdumps(&json!({"found": [{"what": "x"}]}))}))));
        assert!(card(&t).is_null());
    }
}
