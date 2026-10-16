//! Releases cut by a tag: a project with releases on can name a tag pattern (`tb project set <name>
//! --release-tags 'v*'`; none by default). When a new tag matching it lands on the project's default
//! branch, the board cuts a release named after the tag holding the done, unreleased tasks whose merged
//! commits the tag contains; work merged after the tag stays in Unreleased. Tags there when the pattern
//! was set (or releases turned on) are only noted, never cut.

use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::board;

/// `tb project set --release-tags`: name → the tag pattern.
const SETTING: &str = "project_release_tags";
/// What the board has seen of each project's tags: name → `{pattern, seen: [tag], checked_at}`.
const STATE: &str = "release_tags_seen";
/// How often a project's tags are looked at.
const EVERY_SECS: f64 = 300.0;
const PATTERN_MAX: usize = 100;

/// A merged PR's number in a commit subject: "Merge pull request #12 from …", "Title (#12)", Bitbucket's
/// "Merged in feat/x (pull request #12)".
static PR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:pull request #(\d+)|\(#(\d+)\))").unwrap());

fn overrides(app: &App) -> Result<Row> {
    Ok(jloads_obj(app.db.get_setting(SETTING)?.as_deref()))
}

/// The project's tag pattern, if it has one.
pub fn pattern(app: &App, project: &str) -> Result<Option<String>> {
    Ok(overrides(app)?.get(project).and_then(|v| v.as_str()).map(str::to_string))
}

fn clean_pattern(s: &str) -> Result<Option<String>> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("default") {
        return Ok(None);
    }
    if s.chars().count() > PATTERN_MAX || s.chars().any(char::is_whitespace) || s.starts_with('-') || s.contains("..") {
        return err(400, "release_tags is a tag pattern like v* (no spaces), or none.");
    }
    Ok(Some(s.trim_start_matches("refs/tags/").to_string()))
}

/// Sets a project's tag pattern from a body's `release_tags` (null, "", "none" or "default": none).
/// False when the body doesn't name it.
pub fn set_setting(app: &App, project: &str, body: &Value) -> Result<bool> {
    let Some(v) = body.get("release_tags") else { return Ok(false) };
    let to = match v {
        Value::Null => None,
        Value::String(s) => clean_pattern(s)?,
        _ => return err(400, "release_tags is a tag pattern like v*, or none."),
    };
    let mut all = overrides(app)?;
    match to {
        Some(p) => all.insert(project.into(), json!(p)),
        None => all.remove(project),
    };
    app.db.set_setting(SETTING, Some(&jdumps(&Value::Object(all))))?;
    Ok(true)
}

fn state(app: &App) -> Result<Row> {
    Ok(jloads_obj(app.db.get_setting(STATE)?.as_deref()))
}

fn save_state(app: &App, project: &str, v: Option<Value>) -> Result<()> {
    let mut all = state(app)?;
    match v {
        Some(v) => all.insert(project.into(), v),
        None => all.remove(project),
    };
    app.db.set_setting(STATE, Some(&jdumps(&Value::Object(all))))
}

fn git(repo: &str, args: &[&str], timeout: f64) -> Option<String> {
    let git = crate::proc::which("git")?;
    let mut all: Vec<String> = vec!["-C".into(), repo.into()];
    all.extend(args.iter().map(|a| a.to_string()));
    let o = crate::proc::run(&git, &all, None, timeout).ok().filter(|o| o.code == Some(0))?;
    Some(o.stdout.trim().to_string())
}

fn has_origin(repo: &str) -> bool {
    git(repo, &["remote"], 5.0).is_some_and(|r| r.lines().any(|l| l.trim() == "origin"))
}

/// The default branch's ref: origin's HEAD, else origin's or the local main or master.
fn default_ref(repo: &str, origin: bool) -> Option<String> {
    if origin {
        if let Some(h) = git(repo, &["rev-parse", "--abbrev-ref", "origin/HEAD"], 5.0).filter(|h| h.starts_with("origin/")) {
            return Some(h);
        }
    }
    let names: &[&str] = if origin { &["origin/main", "origin/master", "main", "master"] } else { &["main", "master"] };
    names.iter().find(|n| git(repo, &["rev-parse", "--verify", "--quiet", &format!("{n}^{{commit}}")], 5.0).is_some()).map(|n| n.to_string())
}

/// The tags matching the pattern, oldest first.
fn tags(repo: &str, pattern: &str) -> Vec<String> {
    let out = git(repo, &["for-each-ref", "--sort=creatordate", "--format=%(refname:strip=2)", &format!("refs/tags/{pattern}")], 15.0).unwrap_or_default();
    out.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// `owner/repo` from a PR link or a remote's URL, lowercased.
fn repo_slug(url: &str) -> Option<String> {
    let path = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let path = path.split(['?', '#']).next()?;
    let path = path.rsplit_once(':').filter(|(h, _)| !h.contains('/')).map(|(_, p)| p).unwrap_or(path);
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    // A PR link ends `owner/repo/pull/12` (GitHub) or `owner/repo/pull-requests/12` (Bitbucket).
    let at = parts.iter().position(|s| *s == "pull" || *s == "pull-requests").unwrap_or(parts.len());
    (at >= 2).then(|| format!("{}/{}", parts[at - 2], parts[at - 1]).to_lowercase())
}

fn pr_number(t: &Row) -> Option<i64> {
    t.i("pr_num").or_else(|| t.s("pr_url")?.trim_end_matches('/').rsplit('/').next()?.parse().ok())
}

/// The ids of the tasks whose work the tag's commit holds: a PR one when a commit it reaches merged that PR
/// (in this repository), one with no PR when it finished by the tag's commit.
fn held(repo: &str, sha: &str, tasks: &[Row]) -> HashSet<i64> {
    let tag_at = git(repo, &["log", "-1", "--format=%cI", sha], 5.0).and_then(|s| parse_iso(&s)).unwrap_or(f64::MAX);
    let origin = git(repo, &["remote", "get-url", "origin"], 5.0).and_then(|u| repo_slug(&u));
    let with_pr: Vec<&Row> = tasks.iter().filter(|t| pr_number(t).is_some()).collect();
    let mut merged: HashSet<i64> = HashSet::new();
    if !with_pr.is_empty() {
        // A PR merges after its task was made, so the log only needs the commits since the oldest one.
        let since = with_pr.iter().filter_map(|t| t.s("created_at").and_then(parse_iso)).fold(f64::MAX, f64::min);
        let mut args = vec!["log".to_string(), sha.to_string(), "--format=%s".to_string()];
        if since < f64::MAX {
            args.push(format!("--since={}", iso(since - 86_400.0)));
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        for line in git(repo, &args, 30.0).unwrap_or_default().lines() {
            for c in PR_RE.captures_iter(line) {
                if let Some(n) = c.get(1).or(c.get(2)).and_then(|m| m.as_str().parse().ok()) {
                    merged.insert(n);
                }
            }
        }
    }
    let finished_by_tag = |t: &Row| t.s("finished_at").and_then(parse_iso).is_some_and(|at| at <= tag_at);
    tasks
        .iter()
        .filter(|t| match pr_number(t) {
            Some(n) => {
                let elsewhere = match (&origin, t.s("pr_url").and_then(repo_slug)) {
                    (Some(o), Some(r)) => *o != r,
                    _ => false,
                };
                !elsewhere && merged.contains(&n)
            }
            None => finished_by_tag(t),
        })
        .map(|t| t.id())
        .collect()
}

/// Looks at each project with a tag pattern that's due. Outside a transaction: git may take a while.
pub fn sweep(app: &App) -> Result<()> {
    let all = state(app)?;
    for project in overrides(app)?.keys() {
        let due = all.get(project).and_then(|s| s["checked_at"].as_str()).and_then(|at| age_secs(Some(at))).is_none_or(|a| a >= EVERY_SECS);
        if due {
            if let Err(e) = look(app, project) {
                app.info(format!("release tags: {project}: {e}"));
            }
        }
    }
    Ok(())
}

/// Looks at one project's tags now: notes them the first time, then cuts a release for each new one on its
/// default branch. The releases it cut.
pub fn look(app: &App, project: &str) -> Result<Vec<String>> {
    let Some(pattern) = pattern(app, project)? else { return Ok(vec![]) };
    if !crate::releases::on(app, project) {
        // Turning releases on again starts afresh, so tags made while they were off aren't cut.
        return save_state(app, project, None).map(|_| vec![]);
    }
    let Some(repo) = crate::projects::project_path(app, Some(project))?.filter(|p| std::path::Path::new(p).is_dir()) else { return Ok(vec![]) };
    let origin = has_origin(&repo);
    if origin {
        let _ = git(&repo, &["fetch", "--quiet", "--tags", "origin"], 60.0);
    }
    let prev = state(app)?.get(project).cloned().unwrap_or(Value::Null);
    let mut seen: Vec<String> = prev["seen"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
    let all = tags(&repo, &pattern);
    let mut cut = vec![];
    if prev["pattern"].as_str() == Some(pattern.as_str()) {
        let branch = default_ref(&repo, origin);
        let new: Vec<&String> = all.iter().filter(|t| !seen.contains(t)).collect();
        for tag in new {
            let Some(sha) = git(&repo, &["rev-parse", "--verify", "--quiet", &format!("refs/tags/{tag}^{{commit}}")], 5.0) else { continue };
            // A tag off the default branch may land on it later; it's looked at again until then.
            let on_branch = branch.as_deref().is_some_and(|b| git(&repo, &["merge-base", "--is-ancestor", &sha, b], 5.0).is_some());
            if !on_branch {
                continue;
            }
            if let Some(r) = cut_for(app, project, &repo, tag, &sha)? {
                cut.push(r);
            }
            seen.push(tag.clone());
        }
    } else {
        seen = all;
    }
    save_state(app, project, Some(json!({"pattern": pattern, "seen": seen, "checked_at": now_iso()})))?;
    Ok(cut)
}

/// Cuts the tag's release, unless the project has one by that name or the tag holds no unreleased work.
fn cut_for(app: &App, project: &str, repo: &str, tag: &str, sha: &str) -> Result<Option<String>> {
    let Ok(name) = crate::releases::clean_name(tag) else { return Ok(None) };
    let ids = held(repo, sha, &crate::releases::unreleased_rows(app, project)?);
    app.db.tx(|| {
        // A release script run by `tb release cut` pushes the tag after the board recorded the release.
        if crate::releases::named(app, project, &name, 0)? {
            app.info(format!("release tags: {project}: tag {tag} already has its release, so no second one"));
            return Ok(None);
        }
        let tasks: Vec<Row> = crate::releases::unreleased_rows(app, project)?.into_iter().filter(|t| ids.contains(&t.id())).collect();
        if tasks.is_empty() {
            app.info(format!("release tags: {project}: tag {tag} holds no unreleased work, so no release"));
            return Ok(None);
        }
        let id = crate::releases::put_in_release(app, project, &name, None, &tasks, board::BOARD)?;
        let r = rf("release", id);
        app.info(format!("release tags: {project}: cut {r} for tag {tag} with {} tasks", tasks.len()));
        Ok(Some(r))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_slugs_match_links_and_remotes() {
        assert_eq!(repo_slug("https://github.com/Me/App/pull/12").as_deref(), Some("me/app"));
        assert_eq!(repo_slug("https://bitbucket.org/acme/web/pull-requests/9").as_deref(), Some("acme/web"));
        assert_eq!(repo_slug("git@github.com:me/app.git").as_deref(), Some("me/app"));
        assert_eq!(repo_slug("https://github.com/me/app.git").as_deref(), Some("me/app"));
    }

    #[test]
    fn subjects_name_merged_prs() {
        let n = |s: &str| PR_RE.captures(s).and_then(|c| c.get(1).or(c.get(2))).map(|m| m.as_str().to_string());
        assert_eq!(n("Merge pull request #12 from me/feat/x").as_deref(), Some("12"));
        assert_eq!(n("feat: thing (#34)").as_deref(), Some("34"));
        assert_eq!(n("Merged in feat/x (pull request #5)").as_deref(), Some("5"));
        assert_eq!(n("fix #7 in the parser"), None);
    }

    #[test]
    fn patterns_are_checked() {
        assert_eq!(clean_pattern(" v* ").unwrap().as_deref(), Some("v*"));
        assert_eq!(clean_pattern("refs/tags/release-*").unwrap().as_deref(), Some("release-*"));
        assert_eq!(clean_pattern("none").unwrap(), None);
        assert!(clean_pattern("v 1").is_err());
        assert!(clean_pattern("--all").is_err());
    }
}
