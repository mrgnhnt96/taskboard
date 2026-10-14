//! A finished goal's retrospective: a headless Claude reads what the goal planned against what
//! happened (tasks added late, failed and retried tasks, review findings, the owner's questions,
//! QA comments, issues its tasks found, PR fix cycles) and suggests lessons with the refs they came
//! from. A lesson already on the board is confirmed instead of written twice; the owner keeps or
//! retires each suggestion. Runs once on its own when a goal finishes, and again with
//! `tb lesson retro G<n>`.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, lessons, p, triage};

/// At most this many new lessons from one goal: a retrospective that finds more is listing, not learning.
const NEW_MAX: usize = 5;

fn setting_key(gid: i64) -> String {
    format!("retro:{}", rf("goal", gid))
}

fn save_status(app: &App, gid: i64, v: Value) {
    let _ = app.db.set_setting(&setting_key(gid), Some(&jdumps(&v)));
}

fn ran(app: &App, gid: i64) -> Result<Option<Value>> {
    Ok(app.db.get_setting(&setting_key(gid))?.and_then(|s| serde_json::from_str(&s).ok()))
}

/// The goal just finished: look back over it once, when Claude is there to ask.
pub fn finished(app: &App, goal: &Row) -> Result<()> {
    if !triage::ai_on(app) || ran(app, goal.id())?.is_some() {
        return Ok(());
    }
    queue(app, goal.id());
    Ok(())
}

/// `POST /lessons/retro/G<n>`: run it now (again, if it ran before).
pub fn start(app: &App, g: &str) -> Result<Value> {
    let goal = board::get_goal(app, need_ref(&json!(g), "goal")?)?;
    if !triage::ai_on(app) {
        return err(409, "Claude isn't available to the board (backlog AI is off or claude isn't installed), so there's no retrospective to run.");
    }
    if ran(app, goal.id())?.is_some_and(|s| s["state"] == "running") {
        return status(app, g);
    }
    queue(app, goal.id());
    status(app, g)
}

/// `GET /lessons/retro/G<n>`: whether it ran and what it found.
pub fn status(app: &App, g: &str) -> Result<Value> {
    let goal = board::get_goal(app, need_ref(&json!(g), "goal")?)?;
    let mut v = ran(app, goal.id())?.unwrap_or_else(|| json!({"state": "none"}));
    v["goal"] = json!(rf("goal", goal.id()));
    Ok(v)
}

fn queue(app: &App, gid: i64) {
    save_status(app, gid, json!({"state": "running", "at": now_iso()}));
    app.queue_ai(Box::new(move |a: &App| {
        let r = run(a, gid);
        let v = match r {
            Ok(v) => v,
            Err(e) => {
                a.info(format!("retro: {} failed: {e}", rf("goal", gid)));
                json!({"state": "failed", "at": now_iso(), "error": e})
            }
        };
        save_status(a, gid, v);
    }));
}

fn run(app: &App, gid: i64) -> std::result::Result<Value, String> {
    let prompt = prompt(app, gid).map_err(|e| e.message)?;
    let answer = triage::ask(app, prompt, schema())?;
    let mut v = app.db.tx(|| apply(app, gid, &answer)).map_err(|e| e.message)?;
    v["state"] = json!("done");
    v["at"] = json!(now_iso());
    Ok(v)
}

fn schema() -> Value {
    json!({"type": "object", "required": ["lessons"], "properties": {"lessons": {"type": "array", "items": {
        "type": "object", "required": ["action", "kind", "scope", "text", "evidence"],
        "properties": {"action": {"enum": ["new", "confirm"]}, "lesson": {"type": "integer"},
                       "kind": {"enum": lessons::KINDS}, "scope": {"enum": ["project", "global"]},
                       "text": {"type": "string"}, "evidence": {"type": "array", "items": {"type": "string"}}}}}}})
}

fn prompt(app: &App, gid: i64) -> Result<String> {
    Ok(format!(
        "{owner}'s coding agents just finished a goal. Look back over it and say what later work should learn from it: \
         where the plan missed (tasks added after planning, tasks that failed or had to be retried), what reviews kept \
         finding, what {owner} had to be asked, what testers sent back, and what slowed the PRs down.\n\
         A lesson is one or two plain sentences someone can act on next time (\"Split UI and daemon changes into \
         separate tasks: …\"), not a summary of what happened. kind is planning (how goals and tasks are cut and \
         ordered), code (how this code works or breaks), process (how the work moves: reviews, PRs, questions) or \
         tooling (builds, tests, scripts). scope is project unless it holds for any project.\n\
         Give each lesson's evidence: the refs below it came from (T12, B7, G3).\n\
         When an existing lesson below already says it, answer action \"confirm\" with its number (L4 → 4) instead of \
         writing it again (one from another project too, when this goal shows the same thing); never confirm a retired \
         one, and don't suggest it again either. Give at most {NEW_MAX} new \
         lessons, only ones the evidence below shows. Nothing worth keeping? Answer an empty list.\n\n{}",
        dossier(app, gid)?,
        owner = app.cfg.owner
    ))
}

/// Everything the goal left behind, as text for the prompt.
pub fn dossier(app: &App, gid: i64) -> Result<String> {
    let goal = board::get_goal(app, gid)?;
    let tasks = crate::shared::counted_tasks(app, gid)?;
    let mut out = vec![format!("GOAL {} in {}: {}", rf("goal", gid), goal.st("project"), one_line(&goal.st("name"), 200))];
    for (label, k) in [("Outcome", "outcome"), ("In short", "tldr")] {
        if let Some(s) = goal.s(k).filter(|s| !s.trim().is_empty()) {
            out.push(format!("{label}: {}", one_line(s, 600)));
        }
    }
    let notes = board::goal_notes(app, gid)?;
    if !notes.is_empty() {
        out.push("Goal notes:".into());
        for n in notes.iter().take(20) {
            out.push(format!("- {}: {}", n.st("kind"), one_line(&n.st("text"), 300)));
        }
    }
    let planned_by = tasks.iter().filter_map(|t| t.s("started_at")).min().map(|s| s.to_string());
    out.push(String::new());
    out.push("TASKS".into());
    for t in &tasks {
        out.push(task_block(app, t, planned_by.as_deref())?);
    }
    let ids: Vec<i64> = tasks.iter().map(|t| t.id()).collect();
    let found = found_issues(app, &ids)?;
    if !found.is_empty() {
        out.push(String::new());
        out.push("ISSUES ITS TASKS FOUND (backlog)".into());
        for b in &found {
            out.push(format!(
                "{} [{}] found by {} ({}): {}",
                rf("issue", b.id()), b.st("kind"), rf("task", b.i0("found_by_task")), b.st("state"), one_line(&b.st("title"), 200)
            ));
        }
    }
    let qa = qa_comments(app, &ids)?;
    if !qa.is_empty() {
        out.push(String::new());
        out.push("TESTERS' COMMENTS".into());
        for c in &qa {
            let what = c.s("ask").or(c.s("title")).or(c.s("text")).unwrap_or("");
            out.push(format!("On {} ({}): {} — {}", rf("task", c.i0("source_task_id")), c.st("jira_key"), c.st("verdict"), one_line(what, 300)));
        }
    }
    out.push(String::new());
    out.push("EXISTING LESSONS".into());
    let existing = existing(app, goal.s("project"))?;
    if existing.is_empty() {
        out.push("(none yet)".into());
    }
    for l in &existing {
        let scope = match l.s("project") {
            Some(p) if l.s("scope") == Some("project") && Some(p) != goal.s("project") => format!("project {p}"),
            _ => l.st("scope"),
        };
        out.push(format!(
            "{} [{} · {} · {scope}, confirmed {}]: {}",
            rf("lesson", l.id()), l.st("state"), l.st("kind"), l.i0("confirmations"), one_line(&l.st("text"), 400)
        ));
    }
    Ok(out.join("\n"))
}

fn task_block(app: &App, t: &Row, planned_by: Option<&str>) -> Result<String> {
    let id = t.id();
    let outcome = if t.b("failed") { "FAILED" } else { t.s("status").unwrap_or("?") };
    let mut head = format!("{} wave {} [{}]: {}", rf("task", id), t.i0("wave"), outcome, one_line(&t.st("title"), 200));
    if planned_by.is_some_and(|p| t.s("created_at").is_some_and(|c| c > p)) {
        head.push_str(" — added after the goal started");
    }
    let mut lines = vec![head];
    let mut facts = vec![];
    if t.i0("start_tries") > 1 {
        facts.push(format!("started {} times", t.i0("start_tries")));
    }
    let states = app.db.q("SELECT status, needs_reason, failed FROM task_states WHERE task_id = ? ORDER BY at", p![id])?;
    let failed_runs = states.iter().filter(|s| s.b("failed")).count();
    if failed_runs > 0 {
        facts.push(format!("failed {}", plural(failed_runs as i64, "time")));
    }
    let needs: Vec<String> = states.iter().filter_map(|s| s.s("needs_reason").filter(|r| !r.is_empty()).map(|r| r.to_string())).collect();
    if !needs.is_empty() {
        let mut uniq: Vec<String> = vec![];
        for n in needs {
            if !uniq.contains(&n) {
                uniq.push(n);
            }
        }
        facts.push(format!("needed {}: {}", app.cfg.owner, uniq.join(", ")));
    }
    if let Some(m) = t.i("human_min").filter(|m| *m > 0) {
        facts.push(format!("a developer would take ~{m} min"));
    }
    if let Some(n) = t.i("pr_num") {
        facts.push(format!("PR #{n} {}", t.st("pr_state")));
    }
    if !facts.is_empty() {
        lines.push(format!("  {}", facts.join("; ")));
    }
    if let Some(s) = t.s("summary").filter(|s| !s.trim().is_empty()) {
        lines.push(format!("  Summary: {}", one_line(s, 400)));
    }
    let events = app.db.q("SELECT kind, text FROM events WHERE task_id = ? AND kind IN ('question', 'answer', 'handoff') ORDER BY id", p![id])?;
    let mut fixes = 0;
    for e in &events {
        match e.s("kind") {
            Some("question") => lines.push(format!("  Asked: {}", one_line(&e.st("text"), 300))),
            Some("answer") => lines.push(format!("  {}", one_line(&e.st("text"), 300))),
            Some("handoff") if e.st("text").contains("for PR #") => fixes += 1,
            _ => {}
        }
    }
    if fixes > 0 {
        lines.push(format!("  PR fix cycles: {fixes}"));
    }
    for r in crate::steps::rounds(app, id)?.iter().filter(|r| !r.passed) {
        let name = r.data["name"].as_str().unwrap_or(&r.key);
        let found: Vec<String> = r.data["result"]["findings"]
            .as_array()
            .map(|fs| {
                fs.iter()
                    .take(8)
                    .map(|f| match f.as_str() {
                        Some(s) => one_line(s, 160),
                        None => format!("{}{}", one_line(f["title"].as_str().unwrap_or(""), 160), f["severity"].as_str().map(|s| format!(" ({s})")).unwrap_or_default()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if found.is_empty() {
            lines.push(format!("  Step “{name}” didn't pass"));
        } else {
            lines.push(format!("  Step “{name}” found: {}", found.join("; ")));
        }
    }
    Ok(lines.join("\n"))
}

fn in_list(ids: &[i64]) -> String {
    ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
}

fn found_issues(app: &App, ids: &[i64]) -> Result<Vec<Row>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    app.db.q(&format!("SELECT * FROM issues WHERE found_by_task IN ({}) ORDER BY id", in_list(ids)), p![])
}

fn qa_comments(app: &App, ids: &[i64]) -> Result<Vec<Row>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    app.db.q(&format!("SELECT * FROM qa_comments WHERE source_task_id IN ({}) ORDER BY id", in_list(ids)), p![])
}

/// The lessons a goal in `project` could confirm: its project's and the board's, retired ones too
/// so they aren't suggested again, and other projects' kept ones (one confirmed here as well may
/// hold for every project).
fn existing(app: &App, project: Option<&str>) -> Result<Vec<Row>> {
    app.db.q(
        "SELECT * FROM lessons WHERE scope = 'global' OR project IS ? OR state = 'kept'
         ORDER BY project IS NOT ? AND scope = 'project', state = 'retired', confirmations DESC, id DESC LIMIT 60",
        p![project, project],
    )
}

fn same_text(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    norm(a) == norm(b)
}

/// Writes Claude's answer into the lessons: new ones as suggested, repeats as confirmations. Evidence
/// is kept to this goal's own refs, and a lesson is confirmed by a goal only once, so a rerun counts
/// nothing twice.
pub fn apply(app: &App, gid: i64, answer: &Value) -> Result<Value> {
    let goal = board::get_goal(app, gid)?;
    let project = goal.s("project").filter(|p| !p.is_empty());
    let goal_ref = rf("goal", gid);
    let tasks = crate::shared::counted_tasks(app, gid)?;
    let ids: Vec<i64> = tasks.iter().map(|t| t.id()).collect();
    let mut allowed: Vec<String> = vec![goal_ref.clone()];
    allowed.extend(ids.iter().map(|i| rf("task", *i)));
    allowed.extend(found_issues(app, &ids)?.iter().map(|b| rf("issue", b.id())));
    let existing = existing(app, project)?;
    let source = format!("retro {goal_ref}");
    let (mut new, mut confirmed, mut skipped) = (vec![], vec![], 0);
    let mut new_texts: Vec<String> = vec![];
    for item in answer["lessons"].as_array().cloned().unwrap_or_default() {
        let mut ev = vec![goal_ref.clone()];
        for r in str_list(item.get("evidence")) {
            let r = r.trim().to_uppercase();
            if allowed.contains(&r) && !ev.contains(&r) {
                ev.push(r);
            }
        }
        let text = item["text"].as_str().unwrap_or("");
        let target = match item["action"].as_str() {
            Some("confirm") => item["lesson"].as_i64().and_then(|n| existing.iter().find(|l| l.id() == n)),
            _ => existing.iter().find(|l| same_text(&l.st("text"), text)),
        };
        if let Some(l) = target {
            if l.s("state") == Some("retired") || lessons::evidence(l).contains(&goal_ref) {
                skipped += 1;
                continue;
            }
            lessons::confirm_with(app, l, &ev)?;
            confirmed.push(rf("lesson", l.id()));
            continue;
        }
        if item["action"] == "confirm" || new.len() >= NEW_MAX || new_texts.iter().any(|t| same_text(t, text)) {
            skipped += 1;
            continue;
        }
        let scope_project = if item["scope"] == "global" { None } else { project };
        let kind = item["kind"].as_str().unwrap_or("");
        if lessons::clean_kind(kind).is_err() || lessons::clean_text(text).is_err() || (scope_project.is_none() && item["scope"] != "global") {
            skipped += 1;
            continue;
        }
        let id = lessons::suggest(app, scope_project, kind, text, &ev, &source)?;
        new.push(rf("lesson", id));
        new_texts.push(text.to_string());
    }
    let up = lessons::upkeep(app, Some((&goal_ref, project)))?;
    Ok(json!({"goal": goal_ref, "new": new, "confirmed": confirmed, "skipped": skipped,
              "kept": up["kept"], "retired": up["retired"], "suggest_global": up["suggest_global"]}))
}
