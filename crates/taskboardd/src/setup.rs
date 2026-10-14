//! A project's setup gaps: rules that keep its work from finishing, such as a Review step with nobody
//! the board can ask, or a PR flow turned on for a folder with no git remote. `tb project show`, the
//! handoff and the app list them, and `sweep` alerts the owner once while a project with gaps has
//! work on it (again when another of its PRs gets stuck on them).

use serde_json::{json, Value};

use crate::app::App;
use crate::p;
use crate::util::*;

/// Project name → what was last alerted for it (its gaps and stuck PRs), so a dismissed alert stays
/// dismissed until something changes.
const ALERTED_SETTING: &str = "setup_alerted";

pub struct Gap {
    pub key: &'static str,
    pub text: String,
    pub fix: String,
}

impl Gap {
    pub fn json(&self) -> Value {
        json!({"key": self.key, "text": self.text, "fix": self.fix})
    }
}

pub fn gaps(app: &App, project: &str) -> Result<Vec<Gap>> {
    let mut out = vec![];
    let ships = crate::projects::ships_prs(app, project)?;
    if ships && review_unstaffed(app, project)? {
        out.push(Gap {
            key: "no_reviewers",
            text: format!("its PRs need {} to merge, but it has no reviewers the board can ask", approvals_words(app, project)),
            fix: format!("tb reviewers add <name> --user <host id> --project {project}, or tb project set {project} --review off"),
        });
    }
    if crate::projects::pr_flow(app, project)? == "on" && crate::projects::has_remote(app, project)? == Some(false) {
        out.push(Gap {
            key: "no_remote",
            text: "its PR flow is on, but its folder has no git remote to open PRs on".into(),
            fix: format!("add a git remote, or tb project set {project} --pr-flow auto"),
        });
    }
    Ok(out)
}

pub fn gaps_json(app: &App, project: &str) -> Result<Value> {
    Ok(Value::Array(gaps(app, project)?.iter().map(Gap::json).collect()))
}

/// The project's PRs have a Review step, and the board has nobody it could ask to review them.
pub fn review_unstaffed(app: &App, project: &str) -> Result<bool> {
    Ok(crate::reviewers::review_on(app, Some(project)) && !crate::reviewers::any_askable(app, project)?)
}

/// A PR held at review that nobody is on and nobody can be asked to join: it waits forever.
pub fn pr_stuck(app: &App, t: &Row) -> Result<bool> {
    if t.s("pr_phase") != Some("review") {
        return Ok(false);
    }
    let rec = jloads_obj(t.s("pr_flow")).get("rec").cloned().unwrap_or(Value::Null);
    let on_pr = rec["reviewers"].as_array().is_some_and(|a| !a.is_empty()) || rec["requested"].as_i64().unwrap_or(0) > 0 || rec["approvals"].as_i64().unwrap_or(0) > 0;
    Ok(!on_pr && review_unstaffed(app, &t.st("project"))?)
}

fn approvals_words(app: &App, project: &str) -> String {
    match crate::projects::pr_rules(app, Some(project)).approvals.unwrap_or(app.cfg.pr.approvals) {
        n if n > 0 => plural(n, "approval"),
        _ => "an approval".into(),
    }
}

/// One line for an agent's handoff: what in its project's setup will stop its work.
pub fn handoff_line(app: &App, project: &str) -> Result<Option<String>> {
    let gaps = gaps(app, project)?;
    if gaps.is_empty() {
        return Ok(None);
    }
    let texts: Vec<&str> = gaps.iter().map(|g| g.text.as_str()).collect();
    Ok(Some(format!(
        "Project setup: {project} isn't set up to finish its work: {}. It's the owner's to fix; don't change it, say so when your PR is up.",
        texts.join("; ")
    )))
}

/// Alerts the owner about each project whose gaps stand in the way of work on it: queued or running
/// tasks, or PRs stuck on them. Once per project until its gaps or stuck PRs change; cleared once the
/// gaps are fixed or its work is gone.
pub fn sweep(app: &App) -> Result<()> {
    let mut alerted = jloads_obj(app.db.get_setting(ALERTED_SETTING)?.as_deref());
    let before = alerted.clone();
    for p in crate::projects::list_projects(app)? {
        let Some(project) = p["name"].as_str() else { continue };
        let key = format!("setup:{project}");
        let gaps = gaps(app, project)?;
        let busy = !gaps.is_empty() && app.db.q1("SELECT id FROM tasks WHERE project = ? AND status IN ('queued', 'working') LIMIT 1", p![project])?.is_some();
        let stuck: Vec<i64> = if gaps.is_empty() {
            vec![]
        } else {
            let open = app.db.q("SELECT * FROM tasks WHERE project = ? AND status = 'done' AND pr_phase = 'review' ORDER BY id", p![project])?;
            open.into_iter().filter(|t| pr_stuck(app, t).unwrap_or(false)).map(|t| t.id()).collect()
        };
        if !busy && stuck.is_empty() {
            if alerted.remove(project).is_some() {
                crate::dispatch::clear_alert_key(app, &key)?;
            }
            continue;
        }
        let said = json!({"gaps": gaps.iter().map(|g| g.key).collect::<Vec<_>>(), "stuck": stuck});
        if alerted.get(project) == Some(&said) {
            continue;
        }
        crate::dispatch::clear_alert_key(app, &key)?;
        crate::dispatch::add_alert_keyed(app, &alert_text(project, &gaps, &stuck), None, None, &key)?;
        alerted.insert(project.to_string(), said);
    }
    if alerted != before {
        app.db.set_setting(ALERTED_SETTING, Some(&jdumps(&Value::Object(alerted))))?;
    }
    Ok(())
}

fn alert_text(project: &str, gaps: &[Gap], stuck: &[i64]) -> String {
    let texts: Vec<&str> = gaps.iter().map(|g| g.text.as_str()).collect();
    let fixes: Vec<&str> = gaps.iter().map(|g| g.fix.as_str()).collect();
    let held = match stuck {
        [] => String::new(),
        [one] => format!(" {}'s PR is stuck at review.", rf("task", *one)),
        many => format!(" {} have PRs stuck at review.", many.iter().map(|t| rf("task", *t)).collect::<Vec<_>>().join(", ")),
    };
    format!("{project} isn't set up to finish its work: {}.{held} To fix it: {}.", texts.join("; "), fixes.join("; "))
}
