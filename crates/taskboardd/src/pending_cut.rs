//! A release cut that waits for work to finish (`tb release cut v1.5 --after T627 --after G12`). The board
//! keeps it pending, one per project, and cuts it on its own once every named task is done with its PR
//! merged (or with no PR) and every named goal is finished, running the project's release script first
//! just as `tb release cut` does. A named task that fails or whose PR closes unmerged, or a goal that can
//! no longer finish, drops the pending cut and tells the owner, as does a script that fails. Its name is
//! held while it waits, so no other cut, rename or tag takes it.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, p};

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS pending_cuts(
  id INTEGER PRIMARY KEY, project TEXT NOT NULL UNIQUE, name TEXT NOT NULL, notes TEXT, after TEXT NOT NULL,
  dir TEXT, no_script INT DEFAULT 0, made_at TEXT, made_by TEXT, running_at TEXT);
"#;

const AFTER_MAX: usize = 20;
/// How long the release script may run.
const SCRIPT_SECS: f64 = 1800.0;
/// A cut still marked running this long after it started was cut short (the board stopped mid-script).
const STALE_SECS: f64 = SCRIPT_SECS + 600.0;
const OUTPUT_TAIL: usize = 400;

/// Where one named task or goal stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Standing {
    Waiting,
    Met,
    /// It can't be met any more, and why.
    Dead(String),
}

pub fn for_project(app: &App, project: &str) -> Result<Option<Row>> {
    app.db.q1("SELECT * FROM pending_cuts WHERE project = ?", p![project])
}

/// Whether the project's pending cut holds this name (any case).
pub fn holds_name(app: &App, project: &str, name: &str) -> Result<bool> {
    Ok(app.db.count("SELECT COUNT(*) FROM pending_cuts WHERE project = ? AND name = ? COLLATE NOCASE", p![project, name])? > 0)
}

fn after(r: &Row) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(&r.st("after")).unwrap_or_default()
}

fn pr_merged(t: &Row) -> bool {
    t.s("pr_phase") == Some("merged") || t.st("pr_state").eq_ignore_ascii_case("merged")
}

fn has_pr(t: &Row) -> bool {
    t.i("pr_num").is_some() || !t.st("pr_url").is_empty()
}

fn pr_closed(t: &Row) -> bool {
    t.s("pr_phase") == Some("declined") || matches!(t.st("pr_state").to_uppercase().as_str(), "DECLINED" | "CLOSED" | "SUPERSEDED")
}

fn task_standing(t: Option<Row>, r: &str) -> Standing {
    let Some(t) = t else { return Standing::Dead(format!("{r} was deleted")) };
    if t.b("failed") {
        return Standing::Dead(format!("{r} failed"));
    }
    if has_pr(&t) && pr_closed(&t) && !pr_merged(&t) {
        return Standing::Dead(format!("{r}'s PR was closed without merging"));
    }
    if t.s("status") == Some("done") && (!has_pr(&t) || pr_merged(&t)) {
        return Standing::Met;
    }
    Standing::Waiting
}

fn goal_standing(app: &App, g: Option<Row>, r: &str) -> Result<Standing> {
    let Some(g) = g else { return Ok(Standing::Dead(format!("{r} was deleted"))) };
    if board::goal_finished(app, g.id())? {
        return Ok(Standing::Met);
    }
    if g.b("archived") {
        return Ok(Standing::Dead(format!("{r} was archived unfinished")));
    }
    if let Some(t) = crate::shared::counted_tasks(app, g.id())?.into_iter().find(|t| t.b("failed")) {
        return Ok(Standing::Dead(format!("{r} can't finish: {} failed", rf("task", t.id()))));
    }
    Ok(Standing::Waiting)
}

fn is_goal(s: &str) -> bool {
    s.trim().starts_with(['G', 'g'])
}

/// Where a named ref (T<n> or G<n>) stands.
pub fn standing(app: &App, r: &str) -> Result<Standing> {
    if is_goal(r) {
        let id = parse_ref_str(r, "goal")?.unwrap_or(0);
        return goal_standing(app, board::find_goal(app, Some(id))?, r);
    }
    let id = parse_ref_str(r, "task")?.unwrap_or(0);
    Ok(task_standing(board::find_task(app, Some(id))?, r))
}

/// The body's `after` (a ref or a list of them) as T<n>/G<n> refs, each checked against the project.
fn clean_after(app: &App, project: &str, body: &Value) -> Result<Vec<String>> {
    let given: Vec<String> = match &body["after"] {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).collect(),
        _ => return err(400, "after is a list of tasks and goals (T627, G12)."),
    };
    let mut out: Vec<String> = vec![];
    for s in given.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let r = if is_goal(s) {
            let Some(id) = parse_ref_str(s, "goal")? else { continue };
            let g = board::get_goal(app, id)?;
            if g.s("project").is_some_and(|p| !p.is_empty() && p != project) {
                return err(409, format!("{} is {}'s goal, not {project}'s.", rf("goal", id), g.st("project")));
            }
            rf("goal", id)
        } else {
            let Some(id) = parse_ref_str(s, "task").map_err(|_| ApiError::new(400, format!("“{s}” isn't a task or goal (T627, G12).")))? else { continue };
            let t = board::get_task(app, id)?;
            if t.s("project") != Some(project) {
                return err(409, format!("{} is in {}, not {project}.", rf("task", id), t.st("project")));
            }
            rf("task", id)
        };
        if let Standing::Dead(why) = standing(app, &r)? {
            return err(409, format!("{why}, so it can't hold a cut."));
        }
        if !out.contains(&r) {
            out.push(r);
        }
    }
    if out.is_empty() {
        return err(400, "Say which tasks or goals the cut waits for (--after T627 --after G12).");
    }
    if out.len() > AFTER_MAX {
        return err(400, format!("A cut waits for {AFTER_MAX} tasks and goals at most."));
    }
    Ok(out)
}

/// What the app shows of a named ref beside its standing: a task's title, status and PR stage, or a
/// goal's name and how many of its tasks are done.
fn brief(app: &App, r: &str) -> Result<Value> {
    if is_goal(r) {
        let Some(g) = parse_ref_str(r, "goal")?.map(|id| board::find_goal(app, Some(id))).transpose()?.flatten() else { return Ok(json!({})) };
        let tasks = crate::shared::counted_tasks(app, g.id())?;
        let done = tasks.iter().filter(|t| t.s("status") == Some("done") && !t.b("failed")).count();
        return Ok(json!({"title": g.v("name"), "done": done, "total": tasks.len()}));
    }
    let Some(t) = parse_ref_str(r, "task")?.map(|id| board::find_task(app, Some(id))).transpose()?.flatten() else { return Ok(json!({})) };
    Ok(json!({"title": t.v("title"), "status": t.v("status"), "pr_phase": t.v("pr_phase")}))
}

pub fn dict(app: &App, r: &Row) -> Result<Value> {
    let after = after(r)
        .into_iter()
        .map(|a| {
            let (state, why) = match standing(app, &a)? {
                Standing::Waiting => ("waiting", Value::Null),
                Standing::Met => ("met", Value::Null),
                Standing::Dead(w) => ("dead", json!(w)),
            };
            let mut v = brief(app, &a)?;
            v["ref"] = json!(a);
            v["state"] = json!(state);
            v["why"] = why;
            Ok(v)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({
        "project": r.v("project"), "name": r.v("name"), "notes": r.v("notes"), "after": after,
        "no_script": r.b("no_script"), "made_at": r.v("made_at"), "made_by": r.v("made_by"), "running": r.s("running_at").is_some(),
    }))
}

/// `POST /releases` with `after`: sets up the project's pending cut. `dir` is the checkout its release
/// script will run in.
pub fn make(app: &App, project: &str, name: &str, notes: Option<String>, body: &Value, dir: Option<String>) -> Result<Value> {
    if let Some(p) = for_project(app, project)? {
        return err(409, format!("{project} already has a cut pending ({}). Cancel it first with: tb release cancel", p.st("name")));
    }
    let after = clean_after(app, project, body)?;
    let who = board::caller(app, body, board::OWNER);
    let now = now_iso();
    app.db.insert(
        "pending_cuts",
        fields!["project" => project, "name" => name, "notes" => notes, "after" => jdumps(&json!(after)), "dir" => dir,
                "no_script" => body["no_script"] == true, "made_at" => now, "made_by" => who],
    )?;
    app.info(format!("pending cut: {project}: {name} waits for {}", after.join(", ")));
    dict(app, &for_project(app, project)?.ok_or_else(|| ApiError::new(500, "The pending cut wasn't kept."))?)
}

/// `POST /releases/pending/cancel`: drops the project's pending cut.
pub fn cancel(app: &App, project: &str) -> Result<Value> {
    let Some(r) = for_project(app, project)? else { return err(404, format!("{project} has no cut pending.")) };
    if r.s("running_at").is_some() {
        return err(409, format!("{} is being cut right now; it can't be cancelled.", r.st("name")));
    }
    app.db.x("DELETE FROM pending_cuts WHERE id = ?", p![r.id()])?;
    app.info(format!("pending cut: {project}: {} cancelled", r.st("name")));
    Ok(json!({"project": project, "name": r.v("name"), "cancelled": true}))
}

fn drop_cut(app: &App, r: &Row, why: &str) -> Result<()> {
    let (project, name) = (r.st("project"), r.st("name"));
    app.db.x("DELETE FROM pending_cuts WHERE id = ?", p![r.id()])?;
    app.info(format!("pending cut: {project}: dropped {name}: {why}"));
    let text = format!("The pending release {name} for {project} was dropped: {why}. Cut it by hand with tb release cut {name} once it's ready.");
    if let Err(e) = crate::dispatch::raise(app, &text, None, None, Some(&format!("pending-cut:{project}:{name}")), false) {
        app.info(format!("pending cut: {project}: couldn't tell the owner {name} was dropped: {e}"));
    }
    Ok(())
}

/// Looks at every pending cut: drops the ones that can't happen any more and marks the ones whose work is
/// all through as running. The ones to cut now.
pub fn check(app: &App) -> Result<Vec<Row>> {
    app.db.tx(|| {
        let mut ready = vec![];
        for r in app.db.q("SELECT * FROM pending_cuts ORDER BY id", p![])? {
            if let Some(at) = r.s("running_at") {
                if age_secs(Some(at)).is_some_and(|a| a >= STALE_SECS) {
                    drop_cut(app, &r, "the board stopped while its release script ran, so whether it went out isn't known")?;
                }
                continue;
            }
            if !crate::releases::on(app, &r.st("project")) {
                continue;
            }
            let mut waiting = false;
            let mut dead = None;
            for a in after(&r) {
                match standing(app, &a)? {
                    Standing::Waiting => waiting = true,
                    Standing::Met => {}
                    Standing::Dead(why) => {
                        dead = Some(why);
                        break;
                    }
                }
            }
            if let Some(why) = dead {
                drop_cut(app, &r, &why)?;
            } else if !waiting {
                app.db.update("pending_cuts", &json!(r.id()), fields!["running_at" => now_iso()])?;
                ready.push(app.db.q1("SELECT * FROM pending_cuts WHERE id = ?", p![r.id()])?.unwrap_or(r));
            }
        }
        Ok(ready)
    })
}

/// What stops a cut before its script runs: nothing waits in Unreleased, or the name is taken.
fn refusal(app: &App, r: &Row) -> Result<Option<String>> {
    let (project, name) = (r.st("project"), r.st("name"));
    if crate::releases::unreleased_rows(app, &project)?.is_empty() {
        return Ok(Some("no done task waits in Unreleased".into()));
    }
    if crate::releases::named(app, &project, &name, 0)? {
        return Ok(Some(format!("{project} already has a release called {name}")));
    }
    Ok(None)
}

fn tail(s: &str) -> String {
    let s = s.trim();
    let n = s.chars().count();
    if n <= OUTPUT_TAIL { s.to_string() } else { format!("…{}", s.chars().skip(n - OUTPUT_TAIL).collect::<String>()) }
}

fn git(dir: &str, args: &[&str]) -> Option<String> {
    let git = crate::proc::which("git")?;
    let mut all: Vec<String> = vec!["-C".into(), dir.into()];
    all.extend(args.iter().map(|a| a.to_string()));
    let o = crate::proc::run(&git, &all, None, 10.0).ok().filter(|o| o.code == Some(0))?;
    Some(o.stdout.trim().to_string())
}

/// Runs the release script in the cut's checkout, which must be on main or a release/* branch, as `tb
/// release cut` does. Why it didn't go out, if it didn't.
fn run_script(app: &App, r: &Row, script: &str) -> Option<String> {
    let name = r.st("name");
    let dir = r.s("dir").map(str::to_string).or_else(|| crate::projects::project_path(app, Some(r.s("project")?)).ok().flatten());
    let Some(dir) = dir.filter(|d| std::path::Path::new(d).is_dir()) else { return Some("its checkout is gone".into()) };
    let top = git(&dir, &["rev-parse", "--show-toplevel"]).filter(|t| !t.is_empty()).unwrap_or(dir);
    let branch = git(&top, &["branch", "--show-current"]).unwrap_or_default();
    if branch != "main" && !branch.starts_with("release/") {
        let on = if branch.is_empty() { "a detached HEAD".to_string() } else { branch };
        return Some(format!("{top} is on {on}, and the release script only runs on main or a release/* branch"));
    }
    let replaces = crate::releases::failed_named(app, &r.st("project"), &name).unwrap_or(false);
    let mut env = vec![("TB_RELEASE".to_string(), name.clone())];
    if replaces {
        env.push(("TB_RELEASE_REPLACE".into(), "1".into()));
    }
    app.info(format!("pending cut: running the release script in {top}: {script} {name}"));
    let args = vec!["-c".to_string(), format!("{script} \"$1\""), "sh".to_string(), name];
    match crate::proc::run_with(std::path::Path::new("/bin/sh"), &args, Some(std::path::Path::new(&top)), SCRIPT_SECS, &env, None) {
        Err(crate::proc::RunError::Spawn(e)) => Some(format!("the release script couldn't start: {e}")),
        Err(crate::proc::RunError::TimedOut) => Some(format!("the release script ran past {} minutes and was stopped", SCRIPT_SECS as i64 / 60)),
        Ok(o) if o.code == Some(0) => None,
        Ok(o) => {
            let out = if o.stderr.trim().is_empty() { o.stdout } else { o.stderr };
            Some(format!("the release script exited {}: {}", o.code.unwrap_or(1), tail(&out)))
        }
    }
}

/// Cuts a ready pending cut: runs the release script (unless it was set up without one), then records the
/// release holding everything in Unreleased. A failure drops it and tells the owner. The release's ref.
pub fn fire(app: &App, r: &Row) -> Result<Option<String>> {
    let project = r.st("project");
    if let Some(why) = app.db.tx(|| refusal(app, r))? {
        app.db.tx(|| drop_cut(app, r, &why))?;
        return Ok(None);
    }
    let script = if r.b("no_script") { None } else { crate::releases::script(app, &project)? };
    if let Some(why) = script.and_then(|s| run_script(app, r, &s)) {
        app.db.tx(|| drop_cut(app, r, &why))?;
        return Ok(None);
    }
    app.db.tx(|| {
        if let Some(why) = refusal(app, r)? {
            drop_cut(app, r, &format!("its release script ran, but {why}, so the board didn't record it"))?;
            return Ok(None);
        }
        app.db.x("DELETE FROM pending_cuts WHERE id = ?", p![r.id()])?;
        let tasks = crate::releases::unreleased_rows(app, &project)?;
        let who = r.s("made_by").unwrap_or(board::OWNER).to_string();
        let id = crate::releases::put_in_release(app, &project, &r.st("name"), r.s("notes").map(str::to_string), &tasks, &who)?;
        let rel = rf("release", id);
        app.info(format!("pending cut: {project}: cut {rel} ({}) with {} tasks", r.st("name"), tasks.len()));
        Ok(Some(rel))
    })
}

/// The runner's step: cuts each pending cut whose work is through, each on its own thread so a release
/// script doesn't hold up the board.
pub fn sweep(app: &Arc<App>) -> Result<()> {
    if app.db.count("SELECT COUNT(*) FROM pending_cuts", p![])? == 0 {
        return Ok(());
    }
    for r in check(app)? {
        let app = Arc::clone(app);
        std::thread::spawn(move || {
            if let Err(e) = fire(&app, &r) {
                app.info(format!("pending cut: {}: {e}", r.st("project")));
            }
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(f: Value) -> Row {
        f.as_object().unwrap().clone()
    }

    #[test]
    fn a_task_stands_by_its_status_and_pr() {
        let s = |f: Value| task_standing(Some(task(f)), "T1");
        assert_eq!(s(json!({"status": "working"})), Standing::Waiting);
        assert_eq!(s(json!({"status": "done"})), Standing::Met);
        assert_eq!(s(json!({"status": "done", "pr_num": 4, "pr_phase": "review"})), Standing::Waiting);
        assert_eq!(s(json!({"status": "done", "pr_num": 4, "pr_state": "MERGED"})), Standing::Met);
        assert_eq!(s(json!({"status": "done", "pr_num": 4, "pr_phase": "declined"})), Standing::Dead("T1's PR was closed without merging".into()));
        assert_eq!(s(json!({"status": "done", "failed": 1})), Standing::Dead("T1 failed".into()));
        assert_eq!(task_standing(None, "T1"), Standing::Dead("T1 was deleted".into()));
    }

    #[test]
    fn long_output_keeps_its_end() {
        let s = "x".repeat(OUTPUT_TAIL) + "end";
        assert!(tail(&s).ends_with("end"));
        assert_eq!(tail(&s).chars().count(), OUTPUT_TAIL + 1);
    }
}
