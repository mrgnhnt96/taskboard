//! Planning from the backlog (the app's Backlog page). An open issue that isn't in a goal yet is
//! untriaged; it's triaged once it's in a goal, deferred or dropped.
//!
//! A headless `claude -p` groups new issues (area, impact, priority, and which issues are the same
//! kind of work), and splits the issues picked for a new goal into waves: everything in a wave runs
//! side by side, and a wave starts once the one before it is done. Without Claude (`[backlog] ai =
//! false`, or no `claude`), issues are grouped by kind and waves follow priority.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::app::App;
use crate::ops::{goal_detail, new_goal};
use crate::util::*;
use crate::{board, fields, p, proc};

pub const IMPACTS: &[&str] = &["high", "med", "low"];
pub const PRIORITIES: &[&str] = &["p1", "p2", "p3"];
/// How long grouping waits after Claude failed before it tries again.
const GROUP_RETRY_SECS: u64 = 600;

pub fn ai_on(app: &App) -> bool {
    app.cfg.backlog.ai && proc::which(&app.cfg.claude).is_some()
}

fn guess_group(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "bug" => ("Bugs", "Things that are broken"),
        "gap" => ("Test gaps", "Behaviour no test covers yet"),
        "follow" => ("Follow-ups", "Work left over from earlier tasks"),
        _ => ("Clean-ups", "Low-risk tidying"),
    }
}

fn pick<'a>(v: Option<&'a str>, allowed: &[&str]) -> Option<&'a str> {
    v.filter(|x| allowed.contains(x))
}

/// The issue's area, impact, priority and group: what Claude (or you) set, else a guess from its kind.
pub fn fields(b: &Row) -> Value {
    let kind = b.st("kind");
    let (g, about) = guess_group(&kind);
    let area = b.s("area").filter(|a| !a.is_empty()).map(str::to_string).unwrap_or_else(|| b.st("project"));
    let impact = pick(b.s("impact"), IMPACTS).unwrap_or(if kind == "bug" { "med" } else { "low" });
    let priority = pick(b.s("priority"), PRIORITIES).unwrap_or(if kind == "bug" { "p2" } else { "p3" });
    let group = b.s("grp").filter(|x| !x.is_empty()).unwrap_or(g);
    let group_about = if b.s("grp").filter(|x| !x.is_empty()).is_some() { b.st("grp_about") } else { about.to_string() };
    json!({"area": area, "impact": impact, "priority": priority, "group": group, "group_about": group_about,
           "grouped": b.s("grouped_at").is_some()})
}

fn untriaged(app: &App, project: &str) -> Result<Vec<Row>> {
    let rows = app.db.q("SELECT * FROM issues WHERE state = 'open' AND goal_id IS NULL ORDER BY created_at DESC, id DESC", p![])?;
    Ok(rows.into_iter().filter(|b| project == "all" || b.s("project") == Some(project)).collect())
}

/// `GET /backlog/triage`: the untriaged issues with their grouping, and how much of the backlog is triaged.
pub fn list(app: &App, project: &str) -> Result<Value> {
    ensure_grouped(app);
    let rows = untriaged(app, project)?;
    let mut out = vec![];
    for b in &rows {
        let mut d = board::issue_card(app, b)?;
        let o = d.as_object_mut().unwrap();
        for (k, v) in fields(b).as_object().unwrap() {
            o.insert(k.clone(), v.clone());
        }
        o.insert("detail".into(), b.v("detail"));
        o.insert("said".into(), b.v("said"));
        out.push(d);
    }
    let total = if project == "all" {
        app.db.count("SELECT COUNT(*) FROM issues", p![])?
    } else {
        app.db.count("SELECT COUNT(*) FROM issues WHERE project = ?", p![project])?
    };
    let mut projects: Vec<Value> = vec![];
    for r in app.db.q("SELECT project, COUNT(*) AS n FROM issues WHERE state = 'open' AND goal_id IS NULL GROUP BY project ORDER BY project", p![])? {
        projects.push(json!({"name": r.v("project"), "untriaged": r.v("n")}));
    }
    let sh = app.shared.lock();
    Ok(json!({
        "issues": out, "untriaged": rows.len(), "triaged": total - rows.len() as i64, "total": total,
        "projects": projects, "grouping": sh.grouping, "ai": ai_on(app),
    }))
}

// ------------------------------------------------------------------ asking Claude

fn ask(app: &App, prompt: String, schema: Value) -> std::result::Result<Value, String> {
    let claude = proc::which(&app.cfg.claude).ok_or("claude isn't installed")?;
    let c = &app.cfg.backlog;
    let args: Vec<String> = vec![
        "-p".into(),
        prompt,
        "--model".into(),
        c.model.clone(),
        "--setting-sources".into(),
        "".into(),
        "--no-session-persistence".into(),
        "--tools".into(),
        "".into(),
        "--strict-mcp-config".into(),
        "--max-budget-usd".into(),
        c.budget_usd.clone(),
        "--json-schema".into(),
        schema.to_string(),
        "--output-format".into(),
        "json".into(),
    ];
    let out = proc::run(&claude, &args, Some(&app.cfg.data), c.timeout_secs as f64).map_err(|_| "claude didn't answer in time".to_string())?;
    let d: Value = serde_json::from_str(out.stdout.trim()).map_err(|_| "claude's answer wasn't JSON".to_string())?;
    d.get("structured_output")
        .cloned()
        .filter(Value::is_object)
        .or_else(|| d.get("result").and_then(|r| r.as_str()).and_then(|r| serde_json::from_str(r).ok()))
        .ok_or_else(|| "claude's answer had no result".to_string())
}

fn issue_block(b: &Row, with_fields: bool) -> String {
    let mut lines = vec![format!("{} [{}] in {}: {}", rf("issue", b.id()), b.st("kind"), b.st("project"), one_line(&b.st("title"), 200))];
    if let Some(d) = b.s("detail").filter(|d| !d.is_empty()) {
        lines.push(format!("  Detail: {}", one_line(d, 500)));
    }
    if let Some(s) = b.s("said").filter(|s| !s.is_empty()) {
        lines.push(format!("  Reporter said: {}", one_line(s, 300)));
    }
    let snap = jloads_obj(b.s("snapshot"));
    if let Some(f) = snap.s("file") {
        lines.push(format!("  File: {f}"));
    }
    if with_fields {
        let f = fields(b);
        lines.push(format!(
            "  Area: {} · impact {} · priority {} · group “{}”",
            f["area"].as_str().unwrap_or(""),
            f["impact"].as_str().unwrap_or(""),
            f["priority"].as_str().unwrap_or(""),
            f["group"].as_str().unwrap_or("")
        ));
    }
    lines.join("\n")
}

/// Group new issues on the Claude thread, unless it's already doing it or failed a moment ago.
pub fn ensure_grouped(app: &App) {
    if !ai_on(app) {
        return;
    }
    let pending = app.db.count("SELECT COUNT(*) FROM issues WHERE state = 'open' AND goal_id IS NULL AND grouped_at IS NULL", p![]).unwrap_or(0);
    if pending == 0 {
        return;
    }
    {
        let mut sh = app.shared.lock();
        if sh.grouping || sh.group_failed_at.is_some_and(|t| t.elapsed().as_secs() < GROUP_RETRY_SECS) {
            return;
        }
        sh.grouping = true;
    }
    app.queue_ai(Box::new(|a: &App| {
        let r = group_now(a);
        let mut sh = a.shared.lock();
        sh.grouping = false;
        if let Err(e) = r {
            sh.group_failed_at = Some(std::time::Instant::now());
            drop(sh);
            a.info(format!("backlog: grouping failed: {e}"));
        }
    }));
}

fn group_schema() -> Value {
    json!({"type": "object", "required": ["issues"], "properties": {"issues": {"type": "array", "items": {
        "type": "object", "required": ["id", "area", "impact", "priority", "group", "group_about"],
        "properties": {"id": {"type": "integer"}, "area": {"type": "string"}, "impact": {"enum": IMPACTS},
                       "priority": {"enum": PRIORITIES}, "group": {"type": "string"}, "group_about": {"type": "string"}}}}}})
}

fn group_now(app: &App) -> std::result::Result<(), String> {
    let rows = untriaged(app, "all").map_err(|e| e.message)?;
    let new: Vec<&Row> = rows.iter().filter(|b| b.s("grouped_at").is_none()).collect();
    if new.is_empty() {
        return Ok(());
    }
    let done: Vec<&Row> = rows.iter().filter(|b| b.s("grouped_at").is_some()).collect();
    let mut parts = vec![format!(
        "You sort {owner}'s backlog: issues coding agents reported while working, waiting to be planned into goals. \
         For each NEW issue give:\n\
         - area: the part of the product or code it touches, 1–3 words (\"Runner\", \"Backlog UI\"). Reuse an area already in use when it fits.\n\
         - impact: how much fixing it helps — high (agents waste turns, or {owner} misses problems, often), med, or low.\n\
         - priority: p1 do first (it blocks or spoils other work), p2 normal, p3 when there's time.\n\
         - group: issues that touch the same code or the same problem share a group, so one goal can hold work that fits \
         together. 2–5 plain words (\"Runner reliability\"). Reuse an existing group's exact name when the issue fits it. \
         A group never spans two projects.\n\
         - group_about: one short line saying what the group's issues have in common.\n\
         Answer for every NEW issue by its number (B12 → 12).",
        owner = app.cfg.owner
    )];
    parts.push(format!("NEW issues:\n{}", new.iter().map(|b| issue_block(b, false)).collect::<Vec<_>>().join("\n")));
    if !done.is_empty() {
        parts.push(format!("Already sorted (for context; don't answer for these):\n{}", done.iter().map(|b| issue_block(b, true)).collect::<Vec<_>>().join("\n")));
    }
    let v = ask(app, parts.join("\n\n"), group_schema())?;
    let mut answers: HashMap<i64, Value> = HashMap::new();
    for x in v["issues"].as_array().cloned().unwrap_or_default() {
        if let Some(id) = x["id"].as_i64() {
            answers.insert(id, x);
        }
    }
    let now = now_iso();
    app.db
        .tx(|| {
            for b in &new {
                // Still untriaged and still unsorted: it may have changed while Claude thought.
                let Some(cur) = board::find_issue(app, Some(b.id()))? else { continue };
                if cur.s("grouped_at").is_some() || cur.s("state") != Some("open") {
                    continue;
                }
                let Some(a) = answers.get(&b.id()) else { continue };
                let s = |k: &str, n: usize| one_line(a[k].as_str().unwrap_or(""), n);
                let mut f = fields!["area" => s("area", 40), "grp" => s("group", 60), "grp_about" => s("group_about", 160), "grouped_at" => now];
                if let Some(i) = pick(a["impact"].as_str(), IMPACTS) {
                    f.push(("impact", json!(i)));
                }
                if cur.s("priority").is_none() {
                    if let Some(pr) = pick(a["priority"].as_str(), PRIORITIES) {
                        f.push(("priority", json!(pr)));
                    }
                }
                app.db.update("issues", &json!(b.id()), f)?;
            }
            Ok(())
        })
        .map_err(|e| e.message)?;
    app.info(format!("backlog: Claude grouped {} new issue{}", new.len(), if new.len() == 1 { "" } else { "s" }));
    Ok(())
}

// ------------------------------------------------------------------ wave plans

fn wave_why(p: &str) -> &'static str {
    match p {
        "p1" => "Highest priority first; later work builds on these.",
        "p2" => "Independent of each other, so they run side by side.",
        _ => "Low-risk follow-ups and clean-ups last.",
    }
}

fn suggest_name(rows: &[Row]) -> String {
    let mut n: Vec<(String, usize)> = vec![];
    for b in rows {
        let g = fields(b)["group"].as_str().unwrap_or("").to_string();
        match n.iter_mut().find(|(k, _)| *k == g) {
            Some(x) => x.1 += 1,
            None => n.push((g, 1)),
        }
    }
    n.sort_by(|a, b| b.1.cmp(&a.1));
    n.first().map(|x| x.0.clone()).unwrap_or_default()
}

/// Waves by priority; issues in the same area wait for the first one there.
pub fn rules_plan(rows: &[Row]) -> Value {
    let mut waves = vec![];
    for pr in PRIORITIES {
        let mut first_in_area: HashMap<String, String> = HashMap::new();
        let mut items = vec![];
        for b in rows.iter().filter(|b| fields(b)["priority"] == *pr) {
            let area = fields(b)["area"].as_str().unwrap_or("").to_string();
            let r = rf("issue", b.id());
            let after: Vec<String> = first_in_area.get(&area).cloned().into_iter().collect();
            first_in_area.entry(area).or_insert_with(|| r.clone());
            items.push(json!({"ref": r, "after": after}));
        }
        if items.is_empty() {
            continue;
        }
        let same_area = items.iter().any(|i| !i["after"].as_array().map(|a| a.is_empty()).unwrap_or(true));
        let why = format!("{}{}", wave_why(pr), if same_area { " Issues in the same area wait for each other so terminals don’t edit the same files." } else { "" });
        waves.push(json!({"why": why, "items": items}));
    }
    json!({"waves": waves, "name": suggest_name(rows), "by": "rules"})
}

fn plan_schema() -> Value {
    json!({"type": "object", "required": ["name", "waves"], "properties": {
        "name": {"type": "string"},
        "waves": {"type": "array", "items": {"type": "object", "required": ["why", "items"], "properties": {
            "why": {"type": "string"},
            "items": {"type": "array", "items": {"type": "object", "required": ["id"], "properties": {
                "id": {"type": "integer"}, "after": {"type": "array", "items": {"type": "integer"}}}}}}}}}})
}

/// Claude's waves, checked: every picked issue exactly once (missing ones join the last wave), and
/// `after` only names an issue earlier in the same wave.
fn ai_plan(app: &App, rows: &[Row]) -> std::result::Result<Value, String> {
    let owner = &app.cfg.owner;
    let prompt = format!(
        "{owner} picked these backlog issues for one new goal. Each becomes a task a coding agent works on in its own \
         terminal. Split them into waves: everything in a wave runs side by side (up to {n} terminals at once), and a \
         wave starts only once the wave before it is done.\n\
         - Put first what other work builds on, and higher priority earlier.\n\
         - Keep issues that would edit the same files apart, or in the same wave with the later one listing the \
         earlier in `after` (it then starts from that branch). `after` only names issues earlier in the same wave.\n\
         - Use as few waves as the work allows. Every issue appears exactly once, by its number (B12 → 12).\n\
         - why: one plain sentence per wave saying why these go together and why at this point.\n\
         - name: a name for the goal, 2–5 plain words.\n\nIssues:\n{}",
        rows.iter().map(|b| issue_block(b, true)).collect::<Vec<_>>().join("\n"),
        n = app.cfg.backlog.max_terminals.max(1)
    );
    let v = ask(app, prompt, plan_schema())?;
    checked_plan(&v, rows)
}

/// Claude's answer made safe to show and to build a goal from.
fn checked_plan(v: &Value, rows: &[Row]) -> std::result::Result<Value, String> {
    let ids: Vec<i64> = rows.iter().map(|b| b.id()).collect();
    let mut seen: Vec<i64> = vec![];
    let mut waves = vec![];
    for w in v["waves"].as_array().cloned().unwrap_or_default() {
        let mut items = vec![];
        let mut in_wave: Vec<i64> = vec![];
        for it in w["items"].as_array().cloned().unwrap_or_default() {
            let Some(id) = it["id"].as_i64().filter(|i| ids.contains(i) && !seen.contains(i)) else { continue };
            let after: Vec<String> = it["after"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(Value::as_i64)
                .filter(|a| in_wave.contains(a))
                .map(|a| rf("issue", a))
                .collect();
            seen.push(id);
            in_wave.push(id);
            items.push(json!({"ref": rf("issue", id), "after": after}));
        }
        if !items.is_empty() {
            waves.push(json!({"why": one_line(w["why"].as_str().unwrap_or(""), 300), "items": items}));
        }
    }
    let missing: Vec<Value> = ids.iter().filter(|i| !seen.contains(i)).map(|i| json!({"ref": rf("issue", *i), "after": []})).collect();
    if waves.is_empty() {
        return Err("claude's plan had no waves".into());
    }
    if !missing.is_empty() {
        if let Some(last) = waves.last_mut() {
            last["items"].as_array_mut().unwrap().extend(missing);
        }
    }
    let name = one_line(v["name"].as_str().unwrap_or(""), 80);
    Ok(json!({"waves": waves, "name": if name.is_empty() { suggest_name(rows) } else { name }, "by": "claude"}))
}

fn picked(app: &App, body: &Value) -> Result<Vec<Row>> {
    let raw = body["ids"].as_array().cloned().unwrap_or_default();
    if raw.is_empty() {
        return err(400, "Pick at least one issue.");
    }
    if raw.len() > 60 {
        return err(400, "Plan at most 60 issues at once.");
    }
    let mut rows: Vec<Row> = vec![];
    for x in &raw {
        let id = need_ref(x, "issue")?;
        if rows.iter().any(|b| b.id() == id) {
            continue;
        }
        let b = board::get_issue(app, id)?;
        if b.s("state") != Some("open") {
            return err(409, format!("{} isn't open any more.", rf("issue", id)));
        }
        rows.push(b);
    }
    Ok(rows)
}

/// `POST /backlog/plan {ids}`: plan waves for these issues. Claude plans on its own thread; the
/// answer says `planning` until `GET /backlog/plan` has the waves.
pub fn plan_start(app: &App, body: &Value) -> Result<Value> {
    let rows = picked(app, body)?;
    let refs: Vec<String> = rows.iter().map(|b| rf("issue", b.id())).collect();
    let seq = {
        let mut sh = app.shared.lock();
        sh.plan_seq += 1;
        sh.plan_seq
    };
    let base = json!({"id": seq, "ids": refs, "at": now_iso()});
    let with = |extra: Value| {
        let mut out = base.clone();
        for (k, v) in extra.as_object().unwrap() {
            out[k] = v.clone();
        }
        out
    };
    if !ai_on(app) {
        app.shared.lock().plan = Some(merge(with(rules_plan(&rows)), json!({"state": "ready"})));
        return Ok(plan_get(app));
    }
    app.shared.lock().plan = Some(with(json!({"state": "planning"})));
    let planning = app.shared.lock().plan.clone().unwrap();
    app.queue_ai(Box::new(move |a: &App| {
        let done = match ai_plan(a, &rows) {
            Ok(p) => merge(planning.clone(), merge(p, json!({"state": "ready"}))),
            Err(e) => {
                a.info(format!("backlog: planning failed: {e}"));
                merge(
                    planning.clone(),
                    merge(rules_plan(&rows), json!({"state": "ready", "note": format!("Claude couldn’t plan this ({e}), so these waves follow priority.")})),
                )
            }
        };
        let mut sh = a.shared.lock();
        if sh.plan.as_ref().and_then(|p| p["id"].as_i64()) == Some(seq) {
            sh.plan = Some(done);
        }
    }));
    Ok(plan_get(app))
}

fn merge(mut a: Value, b: Value) -> Value {
    for (k, v) in b.as_object().cloned().unwrap_or_default() {
        a[k] = v;
    }
    a
}

/// `GET /backlog/plan`: the latest plan (`state`: none, planning or ready).
pub fn plan_get(app: &App) -> Value {
    app.shared.lock().plan.clone().unwrap_or_else(|| json!({"state": "none"}))
}

/// `POST /backlog/goal {name, waves: [{why, items: [{ref, after}]}]}`: a new goal from the plan.
/// Each issue becomes a planned task in it, in its wave; `after` becomes the task's wait-for.
pub fn create_goal(app: &App, body: &Value) -> Result<Value> {
    let name = one_line(&body_str(body, "name"), 200);
    if name.is_empty() {
        return err(400, "Give the goal a name.");
    }
    let waves = body["waves"].as_array().cloned().unwrap_or_default();
    let mut plan: Vec<(String, Vec<(i64, Vec<i64>)>)> = vec![];
    let mut all: Vec<i64> = vec![];
    for w in &waves {
        let mut items = vec![];
        for it in w["items"].as_array().cloned().unwrap_or_default() {
            let id = need_ref(&it["ref"], "issue")?;
            if all.contains(&id) {
                return err(400, format!("{} is in the plan twice.", rf("issue", id)));
            }
            let after: Vec<i64> = it["after"].as_array().cloned().unwrap_or_default().iter().filter_map(|a| need_ref(a, "issue").ok()).collect();
            all.push(id);
            items.push((id, after));
        }
        if !items.is_empty() {
            plan.push((one_line(w["why"].as_str().unwrap_or(""), 300), items));
        }
    }
    if all.is_empty() {
        return err(400, "The plan has no issues.");
    }
    let gid = app.db.tx(|| {
        let mut project = String::new();
        for id in &all {
            let b = board::get_issue(app, *id)?;
            if b.s("state") != Some("open") {
                return err(409, format!("{} isn't open any more, so nothing changed.", rf("issue", *id)));
            }
            if b.i("goal_id").is_some() {
                return err(409, format!("{} is already in a goal, so nothing changed.", rf("issue", *id)));
            }
            if project.is_empty() {
                project = b.st("project");
            } else if b.s("project") != Some(project.as_str()) {
                return err(409, "A goal holds one project's work. Pick issues from one project.");
            }
        }
        let g = new_goal(
            app,
            &json!({"name": name, "project": project, "run_in_order": false, "max_terminals": app.cfg.backlog.max_terminals.max(1),
                    "outcome": format!("Clear {} from the backlog.", plural(all.len() as i64, "issue"))}),
        )?;
        let gid = g["id"].as_i64().unwrap_or(0);
        let mut task_of: HashMap<i64, i64> = HashMap::new();
        let mut note = vec!["Planned from the backlog in waves. A wave starts once the one before it is done.".to_string()];
        for (wn, (why, items)) in plan.iter().enumerate() {
            let wave = wn as i64 + 1;
            let mut line = vec![];
            for (id, after) in items {
                app.db.update("issues", &json!(id), fields!["goal_id" => gid, "updated_at" => now_iso()])?;
                board::add_issue_event(app, *id, board::OWNER, "move", &format!("Put in {} from the Backlog page, wave {wave}", rf("goal", gid)), None)?;
                let b = board::get_issue(app, *id)?;
                let high = fields(&b)["priority"] == "p1";
                let r = crate::api::promote(app, *id, &json!({"where": "goal", "priority": if high { "high" } else { "normal" }}))?;
                let tid = r["task"]["id"].as_i64().unwrap_or(0);
                task_of.insert(*id, tid);
                let waits: Vec<i64> = after.iter().filter_map(|a| task_of.get(a).copied()).collect();
                let mut f = fields!["wave" => wave];
                if !waits.is_empty() {
                    f.push(("waits_for", json!(jdumps(&json!(waits)))));
                }
                app.db.update("tasks", &json!(tid), f)?;
                line.push(format!(
                    "{}{}",
                    rf("task", tid),
                    if waits.is_empty() { String::new() } else { format!(" (after {})", waits.iter().map(|w| rf("task", *w)).collect::<Vec<_>>().join(", ")) }
                ));
            }
            note.push(format!("Wave {wave}: {}{}", line.join(", "), if why.is_empty() { String::new() } else { format!(". {why}") }));
        }
        board::add_goal_note(app, gid, "decision", &note.join("\n"), Some("Backlog page"), true, None)?;
        Ok(gid)
    })?;
    {
        let mut sh = app.shared.lock();
        let ours = sh.plan.as_ref().map(|p| p["ids"].as_array().map(|a| a.iter().any(|r| all.iter().any(|i| r.as_str() == Some(rf("issue", *i).as_str())))).unwrap_or(false)).unwrap_or(false);
        if ours {
            sh.plan = None;
        }
    }
    app.info(format!("backlog: made {} ({name}) from {} in {} wave{}", rf("goal", gid), plural(all.len() as i64, "issue"), plan.len(), if plan.len() == 1 { "" } else { "s" }));
    goal_detail(app, gid)
}

/// The wave a task added to a goal later joins: the goal's last one (None when it has no waves).
pub fn last_wave(app: &App, goal_id: i64) -> Result<Option<i64>> {
    Ok(app.db.val("SELECT MAX(wave) FROM tasks WHERE goal_id = ?", p![goal_id])?.as_i64())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, kind: &str) -> Row {
        let mut r = Row::new();
        r.insert("id".into(), json!(id));
        r.insert("kind".into(), json!(kind));
        r.insert("project".into(), json!("webapp"));
        r
    }

    #[test]
    fn claudes_plan_is_checked_before_use() {
        let rows = vec![row(1, "bug"), row(2, "gap"), row(3, "clean")];
        // 9 isn't picked, 1 twice, 3 left out, and `after` names a later or unknown issue.
        let v = json!({"name": "", "waves": [
            {"why": "First", "items": [{"id": 2, "after": [1]}, {"id": 9}, {"id": 1, "after": [2, 7]}]},
            {"why": "Again", "items": [{"id": 1}]}]});
        let p = checked_plan(&v, &rows).unwrap();
        let waves = p["waves"].as_array().unwrap();
        assert_eq!(waves.len(), 1, "a wave left empty is dropped");
        let items: Vec<(&str, Value)> = waves[0]["items"].as_array().unwrap().iter().map(|i| (i["ref"].as_str().unwrap(), i["after"].clone())).collect();
        assert_eq!(items, vec![("B2", json!([])), ("B1", json!(["B2"])), ("B3", json!([]))]);
        assert_eq!(p["name"], "Bugs", "no name: the most common group");
        assert!(checked_plan(&json!({"waves": []}), &rows).is_err());
    }
}
