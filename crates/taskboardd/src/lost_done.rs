//! A lost task whose work shipped anyway: its terminal closed before it handed in, and the work
//! finished from another terminal. Each PR poll looks in every lost task's project for signs of that
//! since the terminal closed: for a release task, its version's tag, release commit and published
//! release; for any task, commits on the default branch that name it or merge its branch, and PRs from
//! its branch merged into the default branch. What it finds goes on the card as `looks_finished`, so
//! the card says "Looks finished" with the evidence instead of only offering a restart. The sweep runs
//! on a thread of its own (`start`), and its `gh` calls go through the rate limits.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::prhost::github::Gh;
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

/// What a lost task's new terminal is told when you mark it done from its card: check that the work
/// shipped and hand it in with `tb submit`, using what `sweep` found when it found something. The app
/// only asks; the terminal makes the change.
pub fn finish_prompt(t: &Row, tb: &str) -> String {
    let found = card(t);
    let found = found.as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut lines = vec!["Morgan marked this task done from its card: its terminal closed before it was handed in, and its work looks finished.".to_string()];
    if found.is_empty() {
        lines.push("The board found no sign of it yet, so look in the repository and its pull requests for the work.".into());
    } else {
        lines.push("Since the terminal closed, the board found:".into());
        for f in found {
            let url = f["url"].as_str().map(|u| format!(" ({u})")).unwrap_or_default();
            lines.push(format!("- {}{url}", f["what"].as_str().unwrap_or("")));
        }
    }
    lines.push(format!(
        "Check that the work shipped, then finish with {tb} submit \"<summary>\": add --pr <link> for its pull request, or \
         --no-pr \"<why>\" when it shipped without one. Don't redo the work. If it didn't ship after all, say what's missing \
         with {tb} note and carry on with the task below."
    ));
    lines.join("\n")
}

/// Whether a sweep `start` began is still going.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Sweeps on a thread of its own, so a slow git or gh doesn't hold up the runner; none while the last
/// one is still going.
pub fn start(app: &Arc<App>) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    struct Done;
    impl Drop for Done {
        fn drop(&mut self) {
            RUNNING.store(false, Ordering::SeqCst);
        }
    }
    let app = Arc::clone(app);
    std::thread::spawn(move || {
        let _done = Done;
        if let Err(e) = sweep(&app) {
            app.info(format!("lost-task sweep failed: {e}"));
        }
    });
}

/// Looks again at each lost task that's due. Outside a transaction: git and gh may take a while.
pub fn sweep(app: &App) -> Result<()> {
    for t in app.db.q("SELECT * FROM tasks WHERE lost = 1 AND status = 'needs'", p![])? {
        if app.stopping() {
            break;
        }
        let mut lf = jloads_obj(t.s("looks_finished"));
        if age_secs(lf.get("checked_at").and_then(|v| v.as_str())).is_some_and(|a| a < EVERY_SECS) {
            continue;
        }
        let lost_at = match lf.get("lost_at").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => lost_at(app, &t)?,
        };
        let kept = lf.get("found").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let had = kept.len();
        // Work that shipped stays shipped: a later look that finds less (gh offline) doesn't take it back.
        let mut found = kept;
        for f in look(app, &t, &lost_at) {
            if !found.iter().any(|k| k["what"] == f["what"]) {
                found.push(f);
            }
        }
        found.sort_by(|a, b| a["at"].as_str().cmp(&b["at"].as_str()));
        lf.insert("lost_at".into(), json!(lost_at));
        lf.insert("checked_at".into(), json!(now_iso()));
        lf.insert("found".into(), Value::Array(found));
        save(app, &t, &lf, had)?;
    }
    Ok(())
}

/// Keeps what a look at `t` found, logging the first find. The look took a while: a task picked up (and
/// maybe lost again) since keeps what it has now.
fn save(app: &App, t: &Row, lf: &Row, had: usize) -> Result<()> {
    app.db.tx(|| {
        let Some(now) = app.db.q("SELECT lost, status, looks_finished FROM tasks WHERE id = ?", p![t.id()])?.into_iter().next() else { return Ok(()) };
        if !now.b("lost") || now.st("status") != "needs" || now.s("looks_finished") != t.s("looks_finished") {
            return Ok(());
        }
        // Not `update_task`: the card's "Terminal closed" time is the task's `updated_at`.
        app.db.update("tasks", &json!(t.id()), fields!["looks_finished" => jdumps(&Value::Object(lf.clone()))])?;
        let found = lf.get("found").and_then(|v| v.as_array()).map(Vec::as_slice).unwrap_or(&[]);
        if had == 0 && !found.is_empty() {
            let what: Vec<&str> = found.iter().filter_map(|f| f["what"].as_str()).collect();
            board::log_event(app, t.id(), board::BOARD, "status", &format!("Looks finished since its terminal closed: {}", what.join("; ")))?;
        }
        Ok(())
    })
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
    let slug = github_repo(&repo);
    if let Some(v) = &version {
        found.extend(tag(&repo, v, since_ts));
        found.extend(slug.as_deref().and_then(|r| gh_release(app, r, v, since_ts)));
    }
    let mains = Mains::of(&repo);
    // A task that worked on the main branch has no branch of its own to follow.
    let branch = crate::waitsfor::branch_of(t).filter(|b| b != "HEAD" && !mains.is_main(b));
    found.extend(commits(&repo, &mains, t.id(), branch.as_deref(), version.as_deref(), since_ts));
    if let (Some(b), Some(r)) = (&branch, &slug) {
        found.extend(gh_merged(app, r, &mains, b, since_ts));
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

/// The repo's default branch and the names that count as one (`main`, `master`, `develop`, `trunk`
/// and whatever origin's HEAD points at), with the refs of them the clone has.
struct Mains {
    default: Option<String>,
    refs: Vec<String>,
}

const MAIN_NAMES: [&str; 4] = ["main", "master", "develop", "trunk"];

impl Mains {
    fn of(repo: &str) -> Mains {
        let mut want: Vec<String> = vec!["refs/remotes/origin/HEAD".into()];
        for n in MAIN_NAMES {
            want.push(format!("refs/heads/{n}"));
            want.push(format!("refs/remotes/origin/{n}"));
        }
        let mut args = vec!["for-each-ref", "--format=%(refname)%09%(symref)"];
        args.extend(want.iter().map(String::as_str));
        let listed = git(repo, &args).unwrap_or_default();
        let mut refs = vec![];
        let mut default = None;
        for line in listed.lines() {
            let (name, symref) = line.split_once('\t').unwrap_or((line, ""));
            if !want.iter().any(|w| w == name) {
                continue;
            }
            if name == "refs/remotes/origin/HEAD" {
                default = symref.strip_prefix("refs/remotes/origin/").filter(|d| !d.is_empty()).map(str::to_string);
            }
            refs.push(name.to_string());
        }
        Mains { default, refs }
    }

    /// Whether a branch ("main", "origin/main") is a default branch.
    fn is_main(&self, branch: &str) -> bool {
        let b = branch.strip_prefix("origin/").unwrap_or(branch);
        MAIN_NAMES.contains(&b) || self.default.as_deref() == Some(b)
    }
}

/// A commit subject that names the task the way a person tags work, not in passing: "T361: …",
/// "[T361] …", "… (T361)". "read task 45 as T45" only cites it.
fn tags_task(task: i64) -> Regex {
    Regex::new(&format!(r"(?i)^\[?T{task}\]?(?:[:\s]|$)|[(\[]T{task}[)\]]")).unwrap()
}

/// A branch the board made for the task: `t361-…`, `feat/t361-…`. A bare `361-…` is some other
/// tracker's number unless it's the task's own branch.
fn task_branch(task: i64) -> Regex {
    Regex::new(&format!(r"(?i)(?:^|/)t{task}-[a-z0-9][\w.-]*$")).unwrap()
}

/// The commits on the default branch since `since` that tag the task, merge its branch into the default
/// branch, or (for a release) release its version. Stashes and work on other branches aren't there.
fn commits(repo: &str, mains: &Mains, task: i64, branch: Option<&str>, version: Option<&str>, since: f64) -> Vec<Value> {
    if mains.refs.is_empty() {
        return vec![];
    }
    let since_arg = format!("--since={}", iso(since));
    let n = SCAN.to_string();
    let mut args = vec!["log", &since_arg, "-n", &n, "--format=%h%x1f%cI%x1f%s"];
    args.extend(mains.refs.iter().map(String::as_str));
    args.push("--");
    let log = git(repo, &args).unwrap_or_default();
    let names_task = tags_task(task);
    let task_branch = task_branch(task);
    let release_word = Regex::new(r"(?i)\brelease").unwrap();
    let mut out = vec![];
    for line in log.lines() {
        let mut it = line.splitn(3, '\x1f');
        let (Some(sha), Some(at), Some(subject)) = (it.next(), it.next(), it.next()) else { continue };
        if !after(at, since) {
            continue;
        }
        // Undoing the work isn't finishing it.
        if subject.starts_with("Revert ") {
            continue;
        }
        let ours = |m: &str| task_branch.is_match(m) || branch.is_some_and(|b| m == b || m.ends_with(&format!("/{b}")));
        // Only a merge into a default branch ships it: a stacked branch taking the work in ("Merge branch
        // 'feat/t361-x' into feat/t362-y") or the branch pulled into itself carries on unfinished work.
        let merges = merged_branch(subject).is_some_and(|m| ours(m) && merge_target(subject).is_none_or(|to| mains.is_main(to)));
        // A version alone is a dependency bump or a changelog line; a release commit says so.
        let releases = version.is_some_and(|v| {
            release_word.is_match(subject) && subject.split(|c: char| c.is_whitespace() || c == ',').any(|w| w.trim_start_matches('v') == v)
        });
        // A merge subject names its two branches: "Merge branch 'main' into feat/t361-x" names the task but brings in main.
        let names = merged_branch(subject).is_none() && names_task.is_match(subject);
        if names || merges || releases {
            out.push(json!({"what": format!("Commit {sha}: {}", one_line(subject, 120)), "at": iso_of(at)}));
        }
        if out.len() >= 3 {
            break;
        }
    }
    out
}

/// The branch a merge commit brings in, never the one it lands on: "Merge pull request #10 from me/feat/361-x",
/// "Merge branch 'feat/361-x'", "Merge remote-tracking branch 'origin/feat/361-x'". "Merge branch 'main' into feat/361-x"
/// brings in `main`.
fn merged_branch(subject: &str) -> Option<&str> {
    if let Some(rest) = subject.strip_prefix("Merge pull request #") {
        return rest.split_once(" from ").and_then(|(_, r)| r.split_whitespace().next());
    }
    let rest = subject.strip_prefix("Merge branch '").or_else(|| subject.strip_prefix("Merge remote-tracking branch '"))?;
    rest.split_once('\'').map(|(b, _)| b)
}

/// The branch a local merge lands on, when its subject names one: "Merge branch 'main' into feat/361-x" lands on `feat/361-x`.
fn merge_target(subject: &str) -> Option<&str> {
    merged_branch(subject)?;
    subject.rsplit_once(" into ").map(|(_, b)| b.trim())
}

/// The `owner/name` of the clone's origin, when it's on GitHub.
fn github_repo(repo: &str) -> Option<String> {
    let url = git(repo, &["remote", "get-url", "origin"])?;
    crate::prhost::repo_of_remote(url.trim()).filter(|(host, _)| host == "github").map(|(_, r)| r)
}

/// A `gh` read on `repo` (`owner/name`), counted and held by the rate limits like the board's other reads.
fn gh(app: &App, repo: &str, args: &[&str]) -> Option<Value> {
    let bin = crate::proc::which(&app.cfg.pr.gh)?;
    let cli = crate::prhost::github::GhCli::for_board(app, bin);
    let mut args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    args.extend(["-R".to_string(), repo.to_string()]);
    let out = crate::prhost::limit::reader("lost tasks", || cli.run(&args)).ok()?;
    serde_json::from_str(&out).ok()
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

/// PRs from the task's branch merged into a default branch after `since`; one into a stacked branch
/// hasn't shipped yet.
fn gh_merged(app: &App, repo: &str, mains: &Mains, branch: &str, since: f64) -> Vec<Value> {
    let Some(prs) = gh(app, repo, &["pr", "list", "--state", "merged", "--head", branch, "--json", "number,title,url,mergedAt,baseRefName"]) else {
        return vec![];
    };
    prs.as_array()
        .into_iter()
        .flatten()
        .filter(|pr| pr["mergedAt"].as_str().is_some_and(|at| after(at, since)))
        .filter(|pr| pr["baseRefName"].as_str().is_some_and(|b| mains.is_main(b)))
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
        let other = format!("Merge pull request #9 from me/feat/t{}0-other", id);
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &format!("T{id}: wip")], "2026-10-15T20:30:00Z");
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &other], "2026-10-15T21:10:00Z");
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &format!("Merge pull request #10 from me/feat/t{id}-lost-cards")], "2026-10-15T21:20:00Z");
        sweep(&app).unwrap();
        let found = card(&board::get_task(&app, id).unwrap());
        let what: Vec<&str> = found.as_array().unwrap().iter().map(|f| f["what"].as_str().unwrap()).collect();
        assert_eq!(what.len(), 1, "{found}");
        assert!(what[0].ends_with(&format!("from me/feat/t{id}-lost-cards")), "{found}");
    }

    #[test]
    fn merging_main_into_the_branch_is_not_the_branch_merging() {
        assert_eq!(merged_branch("Merge branch 'main' into feat/361-lost-cards"), Some("main"));
        assert_eq!(merged_branch("Merge branch 'feat/361-lost-cards'"), Some("feat/361-lost-cards"));
        assert_eq!(merged_branch("Merge pull request #10 from me/feat/361-lost-cards"), Some("me/feat/361-lost-cards"));
        assert_eq!(merged_branch("Merge remote-tracking branch 'origin/feat/361-x' into main"), Some("origin/feat/361-x"));
        assert_eq!(merged_branch("Merges are hard"), None);

        let r = repo();
        let data = tempfile::tempdir().unwrap();
        let app = app(data.path());
        let id = lost_task(&app, "Lost cards", r.path(), "2026-10-15T21:00:00Z", json!({"where": {"branch": "feat/lost-cards"}}));
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", "Merge branch 'main' into feat/lost-cards"], "2026-10-15T21:10:00Z");
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &format!("Merge branch 'main' into feat/t{id}-lost-cards")], "2026-10-15T21:11:00Z");
        // Another terminal picking the work up and pulling it: the branch merging into itself.
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", "Merge branch 'feat/lost-cards' of github.com:me/repo into feat/lost-cards"], "2026-10-15T21:12:00Z");
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &format!("Merge remote-tracking branch 'origin/feat/t{id}-lost-cards' into feat/t{id}-lost-cards")], "2026-10-15T21:13:00Z");
        sweep(&app).unwrap();
        assert!(card(&board::get_task(&app, id).unwrap()).is_null());
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", "Merge branch 'feat/lost-cards'"], "2026-10-15T21:20:00Z");
        app.db.x("UPDATE tasks SET looks_finished = json_remove(looks_finished, '$.checked_at') WHERE id = ?", p![id]).unwrap();
        sweep(&app).unwrap();
        assert_eq!(card(&board::get_task(&app, id).unwrap()).as_array().map(|a| a.len()), Some(1));
    }

    #[test]
    fn losing_the_terminal_again_starts_over_without_the_old_evidence() {
        let r = repo();
        let data = tempfile::tempdir().unwrap();
        let app = app(data.path());
        let id = lost_task(&app, "Lost cards", r.path(), "2026-10-15T21:00:00Z", json!({}));
        let seen = json!({"lost_at": "2026-10-15T21:00:00Z", "found": [{"what": "PR #12 merged: Lost cards", "at": "2026-10-15T21:30:00Z"}]});
        app.db.x("UPDATE tasks SET looks_finished = ?, lost = 0, status = 'working' WHERE id = ?", p![jdumps(&seen), id]).unwrap();
        board::mark_lost(&app, &board::get_task(&app, id).unwrap(), "logout").unwrap();
        let t = board::get_task(&app, id).unwrap();
        assert!(t.b("lost") && card(&t).is_null());
        let lf: Value = serde_json::from_str(&t.st("looks_finished")).unwrap();
        assert!(lf["found"].is_null() && lf["lost_at"].as_str().is_some_and(|a| a != "2026-10-15T21:00:00Z"), "{lf}");
    }

    #[test]
    fn evidence_already_found_stays_when_a_later_look_finds_less() {
        let r = repo();
        let data = tempfile::tempdir().unwrap();
        let app = app(data.path());
        let id = lost_task(&app, "Lost cards", r.path(), "2026-10-15T21:00:00Z", json!({}));
        let seen = json!({"lost_at": "2026-10-15T21:00:00Z", "found": [{"what": "PR #12 merged: Lost cards", "at": "2026-10-15T21:30:00Z"}]});
        app.db.x("UPDATE tasks SET looks_finished = ? WHERE id = ?", p![jdumps(&seen), id]).unwrap();
        sweep(&app).unwrap();
        let t = board::get_task(&app, id).unwrap();
        assert_eq!(card(&t)[0]["what"], "PR #12 merged: Lost cards");
        let log = app.db.q("SELECT text FROM events WHERE task_id = ?", p![id]).unwrap();
        assert!(!log.iter().any(|e| e.st("text").starts_with("Looks finished")));
    }

    #[test]
    fn github_counts_published_releases_and_branch_prs_merged_after_the_close() {
        let r = repo();
        let data = tempfile::tempdir().unwrap();
        let gh = data.path().join("gh");
        std::fs::write(
            &gh,
            r#"#!/bin/sh
case "$*" in
  "release view v1.2.3 "*) echo '{"publishedAt":"2026-10-15T22:26:00Z","url":"https://github.com/a/b/releases/tag/v1.2.3","isDraft":false}';;
  "release view v2.0.0 "*) echo '{"publishedAt":"2026-10-15T22:26:00Z","url":"https://github.com/a/b/releases/tag/v2.0.0","isDraft":true}';;
  "release view v3.0.0 "*) exit 1;;
  "release view 3.0.0 "*) echo '{"publishedAt":"2026-10-15T22:40:00Z","url":"https://github.com/a/b/releases/tag/3.0.0","isDraft":false}';;
  "release view v4.0.0 "*) echo '{"publishedAt":"2026-10-15T21:00:00Z","url":"https://github.com/a/b/releases/tag/v4.0.0","isDraft":false}';;
  "pr list "*" -R a/b") echo '[{"number":12,"title":"Lost cards","url":"https://github.com/a/b/pull/12","mergedAt":"2026-10-15T22:30:00Z","baseRefName":"main"},{"number":11,"title":"Earlier try","url":"https://github.com/a/b/pull/11","mergedAt":"2026-10-15T20:00:00Z","baseRefName":"main"},{"number":13,"title":"Stacked","url":"https://github.com/a/b/pull/13","mergedAt":"2026-10-15T22:35:00Z","baseRefName":"feat/t9-below"}]';;
  *) exit 1;;
esac
"#,
        )
        .unwrap();
        std::process::Command::new("chmod").arg("+x").arg(&gh).status().unwrap();
        sh(r.path(), &["remote", "add", "origin", "git@github.com:a/b.git"], "2026-10-15T20:00:00Z");
        let mut cfg = Config::for_tests(data.path());
        cfg.pr.gh = gh.to_string_lossy().to_string();
        let app = App::for_tests(cfg);
        let since = "2026-10-15T22:00:00Z";
        let found = |title: &str, context: Value| {
            let id = lost_task(&app, title, r.path(), since, context);
            sweep(&app).unwrap();
            card(&board::get_task(&app, id).unwrap())
        };

        let published = found("Release x v1.2.3", json!({}));
        assert_eq!(published[0]["what"], "Release v1.2.3 published", "{published}");
        assert_eq!(published[0]["url"], "https://github.com/a/b/releases/tag/v1.2.3");
        assert!(found("Release x v2.0.0", json!({})).is_null(), "a draft isn't published");
        assert_eq!(found("Release x v3.0.0", json!({}))[0]["what"], "Release 3.0.0 published");
        assert!(found("Release x v4.0.0", json!({})).is_null(), "published before the close");

        let merged = found("Lost cards", json!({"where": {"branch": "feat/lost-cards"}}));
        let what: Vec<&str> = merged.as_array().unwrap().iter().map(|f| f["what"].as_str().unwrap()).collect();
        assert_eq!(what, ["PR #12 merged: Lost cards"], "{merged}");
        assert_eq!(merged[0]["url"], "https://github.com/a/b/pull/12");

        // A clone whose origin isn't on GitHub asks gh nothing.
        sh(r.path(), &["remote", "set-url", "origin", "git@gitlab.com:a/b.git"], "2026-10-15T20:00:00Z");
        assert!(found("Release x v1.2.3", json!({})).is_null());
    }

    fn whats(app: &App, id: i64) -> Vec<String> {
        let found = card(&board::get_task(app, id).unwrap());
        found.as_array().into_iter().flatten().map(|f| f["what"].as_str().unwrap().to_string()).collect()
    }

    fn again(app: &App, id: i64) {
        app.db.x("UPDATE tasks SET looks_finished = json_remove(looks_finished, '$.checked_at') WHERE id = ?", p![id]).unwrap();
        sweep(app).unwrap();
    }

    #[test]
    fn only_merges_into_the_default_branch_count() {
        let r = repo();
        let data = tempfile::tempdir().unwrap();
        let app = app(data.path());
        let id = lost_task(&app, "Lost cards", r.path(), "2026-10-15T21:00:00Z", json!({}));
        // A stacked branch taking the work in hasn't shipped it.
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &format!("Merge branch 'feat/t{id}-x' into feat/t{id}9-y")], "2026-10-15T21:10:00Z");
        sweep(&app).unwrap();
        assert!(whats(&app, id).is_empty(), "{:?}", whats(&app, id));
        // A merge or a tagged commit made on another branch isn't on the default branch yet.
        sh(r.path(), &["switch", "-q", "-c", "side"], "2026-10-15T21:11:00Z");
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &format!("Merge branch 'feat/t{id}-x'")], "2026-10-15T21:12:00Z");
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &format!("T{id}: finish the cards")], "2026-10-15T21:13:00Z");
        sh(r.path(), &["switch", "-q", "main"], "2026-10-15T21:14:00Z");
        again(&app, id);
        assert!(whats(&app, id).is_empty(), "{:?}", whats(&app, id));
        // Into the default branch, named or not, it has.
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &format!("Merge branch 'feat/t{id}-x' into develop")], "2026-10-15T21:20:00Z");
        again(&app, id);
        assert_eq!(whats(&app, id).len(), 1, "{:?}", whats(&app, id));
    }

    #[test]
    fn stashes_reverts_and_cited_ids_are_not_the_task_finishing() {
        let r = repo();
        let data = tempfile::tempdir().unwrap();
        let app = app(data.path());
        let id = lost_task(&app, "Lost cards", r.path(), "2026-10-15T21:00:00Z", json!({}));
        std::fs::write(r.path().join("f"), "x").unwrap();
        sh(r.path(), &["add", "f"], "2026-10-15T21:05:00Z");
        sh(r.path(), &["commit", "-q", "-m", "add f"], "2026-10-15T20:00:00Z");
        std::fs::write(r.path().join("f"), "y").unwrap();
        sh(r.path(), &["switch", "-q", "-c", &format!("fix/t{id}-x")], "2026-10-15T21:06:00Z");
        // "WIP on fix/t<id>-x: …", kept under refs/stash.
        sh(r.path(), &["stash", "push", "-q"], "2026-10-15T21:07:00Z");
        sh(r.path(), &["switch", "-q", "main"], "2026-10-15T21:08:00Z");
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &format!("Revert \"Merge pull request #10 from me/feat/t{id}-x\"")], "2026-10-15T21:10:00Z");
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &format!("docs: read task {id} as T{id}")], "2026-10-15T21:11:00Z");
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &format!("Merge pull request #11 from me/{id}-fix-login")], "2026-10-15T21:12:00Z");
        sweep(&app).unwrap();
        assert!(whats(&app, id).is_empty(), "{:?}", whats(&app, id));
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &format!("Finish the lost cards (T{id})")], "2026-10-15T21:20:00Z");
        again(&app, id);
        assert_eq!(whats(&app, id).len(), 1, "{:?}", whats(&app, id));
    }

    #[test]
    fn a_bare_numbered_branch_counts_only_when_it_is_the_tasks_own() {
        assert!(task_branch(361).is_match("me/feat/t361-lost-cards"));
        assert!(task_branch(361).is_match("t361-x"));
        assert!(!task_branch(361).is_match("361-fix-login"));
        assert!(!task_branch(361).is_match("feat/t3610-x"));
        let r = repo();
        let data = tempfile::tempdir().unwrap();
        let app = app(data.path());
        let id = lost_task(&app, "Lost cards", r.path(), "2026-10-15T21:00:00Z", json!({"where": {"branch": "feat/361-lost-cards"}}));
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", "Merge pull request #11 from me/361-fix-login"], "2026-10-15T21:10:00Z");
        sweep(&app).unwrap();
        assert!(whats(&app, id).is_empty(), "{:?}", whats(&app, id));
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", "Merge pull request #12 from me/feat/361-lost-cards"], "2026-10-15T21:20:00Z");
        again(&app, id);
        assert_eq!(whats(&app, id).len(), 1, "{:?}", whats(&app, id));
    }

    #[test]
    fn a_version_alone_is_not_a_release() {
        let r = repo();
        let data = tempfile::tempdir().unwrap();
        let app = app(data.path());
        let id = lost_task(&app, "Release midna v0.1.0-beta.21", r.path(), "2026-10-15T21:00:00Z", json!({}));
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", "Bump bar from 0.1.0-beta.20 to 0.1.0-beta.21"], "2026-10-15T21:10:00Z");
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", "docs: changelog for v0.1.0-beta.21"], "2026-10-15T21:11:00Z");
        sweep(&app).unwrap();
        assert!(whats(&app, id).is_empty(), "{:?}", whats(&app, id));
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", "chore(release): 0.1.0-beta.21"], "2026-10-15T21:20:00Z");
        again(&app, id);
        assert_eq!(whats(&app, id).len(), 1, "{:?}", whats(&app, id));
    }

    #[test]
    fn a_task_lost_before_the_column_counts_from_its_terminal_ending() {
        let r = repo();
        let data = tempfile::tempdir().unwrap();
        let app = app(data.path());
        let id = lost_task(&app, "Lost cards", r.path(), "x", json!({}));
        app.db.x("UPDATE tasks SET looks_finished = NULL, updated_at = '2026-10-15T23:00:00Z' WHERE id = ?", p![id]).unwrap();
        app.db.x("INSERT INTO events (task_id, at, who, kind, text) VALUES (?, '2026-10-15T21:00:00Z', 'Midna', 'status', 'Terminal ended')", p![id]).unwrap();
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &format!("T{id}: before the end")], "2026-10-15T20:50:00Z");
        sh(r.path(), &["commit", "-q", "--allow-empty", "-m", &format!("T{id}: after the end")], "2026-10-15T21:10:00Z");
        sweep(&app).unwrap();
        let t = board::get_task(&app, id).unwrap();
        let lf: Value = serde_json::from_str(&t.st("looks_finished")).unwrap();
        assert_eq!(lf["lost_at"], "2026-10-15T21:00:00Z", "{lf}");
        assert_eq!(whats(&app, id).len(), 1, "{lf}");
        assert!(whats(&app, id)[0].ends_with("after the end"));

        // With no such line, the card's own time.
        let other = lost_task(&app, "Lost cards", r.path(), "x", json!({}));
        app.db.x("UPDATE tasks SET looks_finished = NULL, updated_at = '2026-10-15T23:00:00Z' WHERE id = ?", p![other]).unwrap();
        sweep(&app).unwrap();
        let lf: Value = serde_json::from_str(&board::get_task(&app, other).unwrap().st("looks_finished")).unwrap();
        assert_eq!(lf["lost_at"], "2026-10-15T23:00:00Z", "{lf}");
    }

    #[test]
    fn a_task_picked_up_while_it_was_looked_at_keeps_what_it_has_now() {
        let r = repo();
        let data = tempfile::tempdir().unwrap();
        let app = app(data.path());
        let id = lost_task(&app, "Lost cards", r.path(), "2026-10-15T21:00:00Z", json!({}));
        let seen = board::get_task(&app, id).unwrap();
        let lf = jloads_obj(Some(&jdumps(&json!({"lost_at": "2026-10-15T21:00:00Z", "found": [{"what": "PR #12 merged: Lost cards", "at": "2026-10-15T21:30:00Z"}]}))));
        // Lost again since: a fresh lost_at.
        app.db.x("UPDATE tasks SET looks_finished = ? WHERE id = ?", p![jdumps(&json!({"lost_at": "2026-10-15T22:00:00Z"})), id]).unwrap();
        save(&app, &seen, &lf, 0).unwrap();
        let now: Value = serde_json::from_str(&board::get_task(&app, id).unwrap().st("looks_finished")).unwrap();
        assert_eq!(now, json!({"lost_at": "2026-10-15T22:00:00Z"}));
        // Picked up and still working.
        app.db.x("UPDATE tasks SET looks_finished = ?, lost = 0, status = 'working' WHERE id = ?", p![seen.st("looks_finished"), id]).unwrap();
        save(&app, &seen, &lf, 0).unwrap();
        assert!(board::get_task(&app, id).unwrap().st("looks_finished").find("PR #12").is_none());
        // Still lost as it was: kept.
        app.db.x("UPDATE tasks SET lost = 1, status = 'needs' WHERE id = ?", p![id]).unwrap();
        save(&app, &seen, &lf, 0).unwrap();
        assert_eq!(card(&board::get_task(&app, id).unwrap())[0]["what"], "PR #12 merged: Lost cards");
        assert!(app.db.q("SELECT text FROM events WHERE task_id = ?", p![id]).unwrap().iter().any(|e| e.st("text").starts_with("Looks finished")));
    }

    #[test]
    fn a_task_back_from_lost_shows_nothing() {
        let t: Row = jloads_obj(Some(&jdumps(&json!({"lost": 0, "looks_finished": jdumps(&json!({"found": [{"what": "x"}]}))}))));
        assert!(card(&t).is_null());
    }

    #[test]
    fn marking_a_lost_task_done_asks_a_new_terminal_to_submit_it_with_what_was_found() {
        let r = repo();
        let data = tempfile::tempdir().unwrap();
        let app = app(data.path());
        let id = lost_task(&app, "Release midna v0.1.0-beta.21", r.path(), "2026-10-15T21:25:00Z", json!({}));
        let seen = json!({"lost_at": "2026-10-15T21:25:00Z", "found": [{"what": "PR #12 merged: Lost cards", "at": "2026-10-15T22:00:00Z", "url": "https://x/pull/12"}]});
        app.db.x("UPDATE tasks SET looks_finished = ? WHERE id = ?", p![jdumps(&seen), id]).unwrap();

        let path = format!("tasks/T{id}/resume");
        crate::api::dispatch(&app, "POST", &path, &crate::api::Query::new(), &json!({"mode": "finish"})).unwrap();
        let jobs = app.db.q("SELECT args FROM jobs WHERE task_id = ?", p![id]).unwrap();
        let args: Value = serde_json::from_str(jobs.last().unwrap().s("args").unwrap()).unwrap();
        let prompt = args["prompt"].as_str().unwrap();
        assert!(prompt.starts_with(&format!("[task-board:T{id}] Morgan marked this task done")), "{prompt}");
        assert!(prompt.contains("- PR #12 merged: Lost cards (https://x/pull/12)"), "{prompt}");
        assert!(prompt.contains("tb submit"), "{prompt}");
        assert!(!board::get_task(&app, id).unwrap().b("lost"));

        // Only a lost task: one with a live terminal is handed in from there.
        let e = crate::api::dispatch(&app, "POST", &path, &crate::api::Query::new(), &json!({"mode": "finish"})).unwrap_err();
        assert_eq!(e.status, 409);
    }

    #[test]
    fn with_nothing_found_the_terminal_is_told_to_look() {
        let t: Row = jloads_obj(Some(&jdumps(&json!({"lost": 1, "looks_finished": jdumps(&json!({"lost_at": "x"}))}))));
        let p = finish_prompt(&t, "tb");
        assert!(p.contains("found no sign of it yet"), "{p}");
        assert!(p.contains("tb submit \"<summary>\""), "{p}");
    }
}
