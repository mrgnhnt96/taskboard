//! A kept review's suggested fixes: a headless Claude reads the review (where the time went, the slow
//! and repeated commands, timeouts, reruns, time waiting on the owner) and writes at most five concrete
//! fixes, each what to change, the evidence from the review, and the time it would save. Each is filed
//! as an untriaged backlog issue tagged with the review (`issues.review_id`); a fix already open in the
//! backlog is linked instead of written twice. It changes no code and starts nothing: the owner plans
//! a goal from them on the Backlog page. Runs once after each kept review, before the owner is told,
//! and again with `POST /reviews/<id>/fixes`.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, ops, p, projects, review, triage};

/// At most this many fixes from one review, new and linked together.
pub const MAX: usize = 5;
const KINDS: &[&str] = &["bug", "gap", "follow", "clean"];
/// How many open backlog issues the prompt lists for Claude to link to.
const OPEN_LIST: i64 = 80;

/// Review `id` was just kept: file its fixes when Claude is there to ask and `[review] suggest` is on,
/// then tell the owner; otherwise tell the owner now.
pub fn after_save(app: &App, id: i64) -> Result<()> {
    if !app.cfg.review.suggest || !triage::ai_on(app) {
        return review::announce(app, id);
    }
    queue(app, id, true);
    Ok(())
}

/// `POST /reviews/<id>/fixes`: run it now (again, if it ran before).
pub fn start(app: &App, id: i64) -> Result<Value> {
    review::get(app, id)?;
    if !triage::ai_on(app) {
        return err(409, "Claude isn't available to the board (backlog AI is off or claude isn't installed), so there are no fixes to suggest.");
    }
    if status(app, id)?["state"] != "running" {
        queue(app, id, false);
    }
    review::get(app, id)
}

/// Whether the fixes ran for review `id`, and what they filed.
pub fn status(app: &App, id: i64) -> Result<Value> {
    let r = app.db.q1("SELECT fixes FROM reviews WHERE id = ?", p![id])?;
    Ok(r.and_then(|r| r.s("fixes").and_then(|s| serde_json::from_str(s).ok())).unwrap_or_else(|| json!({"state": "none"})))
}

fn save_status(app: &App, id: i64, v: &Value) {
    let _ = app.db.x("UPDATE reviews SET fixes = ? WHERE id = ?", p![jdumps(v), id]);
}

fn queue(app: &App, id: i64, then_announce: bool) {
    save_status(app, id, &json!({"state": "running", "at": now_iso()}));
    app.queue_ai(Box::new(move |a: &App| {
        let v = run(a, id).unwrap_or_else(|e| {
            a.info(format!("review fixes: review {id} failed: {e}"));
            json!({"state": "failed", "at": now_iso(), "error": e})
        });
        save_status(a, id, &v);
        if then_announce {
            if let Err(e) = review::announce(a, id) {
                a.info(format!("review fixes: announcing review {id}: {}", e.message));
            }
        }
    }));
}

fn run(app: &App, id: i64) -> std::result::Result<Value, String> {
    let prompt = prompt(app, id).map_err(|e| e.message)?;
    let answer = triage::ask(app, prompt, schema())?;
    let mut v = app.db.tx(|| apply(app, id, &answer)).map_err(|e| e.message)?;
    v["state"] = json!("done");
    v["at"] = json!(now_iso());
    Ok(v)
}

fn schema() -> Value {
    json!({"type": "object", "required": ["fixes"], "properties": {"fixes": {"type": "array", "items": {
        "type": "object", "required": ["action", "project", "kind", "title", "change", "evidence", "saves_minutes"],
        "properties": {"action": {"enum": ["new", "link"]}, "issue": {"type": "integer"},
                       "project": {"type": "string"}, "kind": {"enum": KINDS}, "title": {"type": "string"},
                       "change": {"type": "string"}, "evidence": {"type": "string"},
                       "saves_minutes": {"type": "integer", "minimum": 0}}}}}})
}

fn open_issues(app: &App) -> Result<Vec<Row>> {
    app.db.q("SELECT * FROM issues WHERE state = 'open' ORDER BY review_id IS NULL, id DESC LIMIT ?", p![OPEN_LIST])
}

fn project_names(app: &App) -> Result<Vec<String>> {
    Ok(projects::list_projects(app)?.iter().filter_map(|p| p["name"].as_str().map(str::to_string)).collect())
}

/// The review as the prompt shows it: its numbers, with the long lists cut short.
fn report_text(data: &Value) -> String {
    let mut d = data.clone();
    if let Some(r) = d["reruns"].as_array_mut() {
        r.truncate(15);
    }
    serde_json::to_string_pretty(&d).unwrap_or_default()
}

fn prompt(app: &App, id: i64) -> Result<String> {
    let r = review::get(app, id)?;
    let mut open = vec![];
    for b in open_issues(app)? {
        let mut line = format!("{} [{}] in {}: {}", rf("issue", b.id()), b.st("kind"), b.st("project"), one_line(&b.st("title"), 200));
        if let Some(d) = b.s("detail").filter(|d| !d.is_empty()) {
            line.push_str(&format!("\n  {}", one_line(d, 300)));
        }
        open.push(line);
    }
    if open.is_empty() {
        open.push("(none)".into());
    }
    Ok(format!(
        "This is a review of where {owner}'s coding agents spent their time from {start} to {end}: time by category \
         and project (seconds), the slowest tasks, slow repeated commands (median and p90 seconds), timeouts, failed \
         commands that were rerun, time tasks waited on {owner}, the change since the last review, and fixes a few \
         rules already pointed at (suggestions).\n\
         Suggest the fixes that would save the most agent time. Each fix is one concrete change to the tooling, the \
         build or test setup, or how agents work (\"Run the daemon's integration tests as one binary\"), not a \
         restatement of a number. Give it:\n\
         - title: the change, in a few words;\n\
         - change: what to change, in one to three plain sentences;\n\
         - evidence: the numbers from this review that show it (commands, tasks, minutes);\n\
         - saves_minutes: about how many minutes of agent time a week it would save, from those numbers;\n\
         - project: the project the change belongs in, one of: {projects};\n\
         - kind: bug (something broken), gap (something missing), follow (a follow-up to make) or clean (tidying).\n\
         When an open backlog issue below already covers a fix, answer action \"link\" with its number (B12 → 12) \
         instead of writing it again. Give at most {MAX} fixes, biggest saving first, only ones the review shows. \
         Nothing worth fixing? Answer an empty list.\n\n\
         REVIEW\n{report}\n\nOPEN BACKLOG ISSUES\n{open}",
        owner = app.cfg.owner,
        start = r["period_start"].as_str().unwrap_or(""),
        end = r["period_end"].as_str().unwrap_or(""),
        projects = project_names(app)?.join(", "),
        report = report_text(&r["data"]),
        open = open.join("\n"),
    ))
}

fn detail(change: &str, evidence: &str, minutes: i64) -> String {
    let mut out = vec![change.trim().to_string()];
    if !evidence.trim().is_empty() {
        out.push(format!("Evidence: {}", evidence.trim()));
    }
    if minutes > 0 {
        out.push(format!("Estimated time saved: about {} a week.", review::hours_text(minutes as f64 * 60.0)));
    }
    out.join("\n\n")
}

/// Writes Claude's answer into the backlog: new fixes as open issues tagged with review `id`, ones the
/// backlog already has as a note on that issue. A rerun links what the last run filed rather than
/// filing it again.
pub fn apply(app: &App, id: i64, answer: &Value) -> Result<Value> {
    let r = review::get(app, id)?;
    let review_ref = format!("review {id}");
    let open = open_issues(app)?;
    let names = project_names(app)?;
    let period = format!("{} to {}", r["period_start"].as_str().unwrap_or("").get(..10).unwrap_or(""), r["period_end"].as_str().unwrap_or("").get(..10).unwrap_or(""));
    let (mut filed, mut linked, mut skipped): (Vec<String>, Vec<String>, usize) = (vec![], vec![], 0);
    for item in answer["fixes"].as_array().cloned().unwrap_or_default() {
        let title = one_line(item["title"].as_str().unwrap_or(""), 200);
        let change = item["change"].as_str().unwrap_or("");
        let evidence = item["evidence"].as_str().unwrap_or("");
        let minutes = item["saves_minutes"].as_i64().unwrap_or(0).max(0);
        if filed.len() + linked.len() >= MAX || title.is_empty() || change.trim().is_empty() {
            skipped += 1;
            continue;
        }
        let target = match item["action"].as_str() {
            Some("link") => item["issue"].as_i64().and_then(|n| open.iter().find(|b| b.id() == n)),
            _ => open.iter().find(|b| ops::titles_match(&b.st("title"), &title)),
        };
        if let Some(b) = target {
            let r = rf("issue", b.id());
            if !filed.contains(&r) && !linked.contains(&r) {
                if b.i("review_id") != Some(id) {
                    board::add_issue_event(app, b.id(), "Review", "note", &format!("Suggested again by {review_ref}: {}", one_line(&detail(change, evidence, minutes), 600)), None)?;
                }
                linked.push(r);
            } else {
                skipped += 1;
            }
            continue;
        }
        let project = item["project"].as_str().unwrap_or("").trim();
        let kind = item["kind"].as_str().unwrap_or("");
        if item["action"] == "link" || !names.iter().any(|n| n == project) || !KINDS.contains(&kind) {
            skipped += 1;
            continue;
        }
        let now = now_iso();
        let bid = app.db.insert(
            "issues",
            fields!["project" => project, "kind" => kind, "title" => title, "detail" => detail(change, evidence, minutes),
                    "how" => format!("Suggested by the time review of {period}."), "source" => "review", "review_id" => id,
                    "found_by_name" => "Review", "state" => "open", "snapshot" => "{}", "created_at" => now, "updated_at" => now],
        )?;
        board::add_issue_event(app, bid, "Review", "note", &format!("Suggested by {review_ref}"), None)?;
        filed.push(rf("issue", bid));
    }
    Ok(json!({"filed": filed, "linked": linked, "skipped": skipped}))
}

/// The issues review `id`'s fixes filed or linked, as the backlog shows them.
pub fn issues(app: &App, id: i64) -> Result<Vec<Value>> {
    let st = status(app, id)?;
    let mut out = vec![];
    for (key, is_linked) in [("filed", false), ("linked", true)] {
        for r in str_list(st.get(key)) {
            let Some(n) = r.trim_start_matches('B').parse::<i64>().ok() else { continue };
            if let Some(b) = app.db.q1("SELECT * FROM issues WHERE id = ?", p![n])? {
                let mut c = board::issue_card(app, &b)?;
                c["linked"] = json!(is_linked);
                out.push(c);
            }
        }
    }
    Ok(out)
}
