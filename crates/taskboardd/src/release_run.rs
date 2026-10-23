//! The CI run a release started. Once the board records a release of a GitHub project, whether `tb
//! release cut`, a pending cut or a release tag cut it, it looks for the run its tag started for a few
//! minutes and keeps the run's job page on the release: `tb release cut` prints it and `tb release show`
//! lists it. A project off GitHub, or a release whose tag starts no run, has none.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{fields, p};

pub const ADDED: &[(&str, &str, &str)] = &[
    ("releases", "run_url", "TEXT"),
    ("releases", "run_dir", "TEXT"),
    ("releases", "run_since", "TEXT"),
];

/// How long after the cut the board keeps looking for the run.
pub const LOOK_SECS: f64 = 300.0;
/// A run made a little before the board's `since` still counts: the clocks may be apart.
const SLACK_SECS: f64 = 10.0;

static LOOKING: AtomicBool = AtomicBool::new(false);

/// Whether the checkout's origin is on GitHub, the one host whose runs the board can look up.
fn on_github(dir: &str) -> bool {
    let Some(git) = crate::proc::which("git") else { return false };
    let args: Vec<String> = ["-C", dir, "remote", "get-url", "origin"].iter().map(|a| a.to_string()).collect();
    crate::proc::run(&git, &args, None, 5.0).ok().filter(|o| o.code == Some(0)).is_some_and(|o| o.stdout.contains("github.com"))
}

/// Has the board look for the release's run in `dir`: one the tag started since `since`, or with no
/// `since` (the tag was pushed before the board heard of it), the tag's newest.
pub fn track(app: &App, id: i64, dir: Option<&str>, since: Option<f64>) -> Result<()> {
    let Some(dir) = dir.filter(|d| Path::new(d).is_dir() && on_github(d)) else { return Ok(()) };
    app.db.update("releases", &json!(id), fields!["run_dir" => dir, "run_since" => since.map(iso)])?;
    Ok(())
}

/// Whether the board is still looking for the release's run.
pub fn looking(r: &crate::util::Row) -> bool {
    r.s("run_url").is_none()
        && r.s("run_dir").is_some()
        && r.s("failed_at").is_none()
        && r.s("cut_at").and_then(parse_iso).is_some_and(|at| now_ts() - at < LOOK_SECS)
}

fn gh(app: &App, dir: &str, args: &[&str]) -> Option<Value> {
    let gh = crate::proc::which(&app.cfg.pr.gh)?;
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let o = crate::proc::run_with(&gh, &args, Some(Path::new(dir)), 20.0, &crate::accounts::gh_env(&app.cfg), None).ok()?;
    if o.code != Some(0) {
        return None;
    }
    serde_json::from_str(&o.stdout).ok()
}

/// Of `gh run list`'s runs, newest first, the newest the tag started since `since`: its id and page.
fn pick(runs: &Value, tag: &str, since: Option<f64>) -> Option<(i64, String)> {
    runs.as_array()?
        .iter()
        .filter(|r| r["headBranch"] == tag)
        .filter(|r| since.is_none_or(|s| r["createdAt"].as_str().and_then(parse_iso).is_some_and(|at| at >= s - SLACK_SECS)))
        .find_map(|r| Some((r["databaseId"].as_i64()?, r["url"].as_str().unwrap_or_default().to_string())))
}

/// The release's run: its first job's page, which shows when the build is done, else the run's.
fn find(app: &App, r: &crate::util::Row) -> Option<String> {
    let (dir, tag) = (r.s("run_dir")?, r.st("name"));
    let runs = gh(app, dir, &["run", "list", "--branch", &tag, "--limit", "20", "--json", "databaseId,createdAt,headBranch,url"])?;
    let (id, url) = pick(&runs, &tag, r.s("run_since").and_then(parse_iso))?;
    let job = gh(app, dir, &["run", "view", &id.to_string(), "--json", "jobs"])
        .and_then(|v| v["jobs"][0]["url"].as_str().map(str::to_string))
        .filter(|u| !u.is_empty());
    job.or(Some(url).filter(|u| !u.is_empty()))
}

/// The runner's step: looks for the runs of the releases the board is still looking for, on one thread
/// of its own so gh doesn't hold up the board.
pub fn sweep(app: &Arc<App>) -> Result<()> {
    let since = iso(now_ts() - LOOK_SECS);
    let rows = app.db.q("SELECT * FROM releases WHERE run_url IS NULL AND run_dir IS NOT NULL AND failed_at IS NULL AND cut_at >= ?", p![since])?;
    let rows: Vec<_> = rows.into_iter().filter(looking).collect();
    if rows.is_empty() || LOOKING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let app = Arc::clone(app);
    std::thread::spawn(move || {
        for r in rows {
            if let Some(url) = find(&app, &r) {
                app.info(format!("release run: {} {}: {url}", r.st("project"), r.st("name")));
                if let Err(e) = app.db.update("releases", &json!(r.id()), fields!["run_url" => url]) {
                    app.info(format!("release run: {e}"));
                }
            }
        }
        LOOKING.store(false, Ordering::SeqCst);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_run_the_tag_started_since_the_push_is_picked() {
        let runs = json!([
            {"databaseId": 4, "headBranch": "main", "createdAt": "2026-10-20T10:05:00Z", "url": "u4"},
            {"databaseId": 3, "headBranch": "v2", "createdAt": "2026-10-20T10:04:00Z", "url": "u3"},
            {"databaseId": 2, "headBranch": "v2", "createdAt": "2026-10-20T10:03:00Z", "url": "u2"},
            {"databaseId": 1, "headBranch": "v2", "createdAt": "2026-10-19T10:00:00Z", "url": "u1"},
        ]);
        let since = parse_iso("2026-10-20T10:00:00Z");
        assert_eq!(pick(&runs, "v2", since), Some((3, "u3".into())));
        assert_eq!(pick(&runs, "v2", None), Some((3, "u3".into())));
        assert_eq!(pick(&runs, "v3", None), None, "another tag's runs aren't the release's");
        assert_eq!(pick(&runs, "v2", parse_iso("2026-10-21T00:00:00Z")), None, "a run from before the push is an older release's");
    }
}
