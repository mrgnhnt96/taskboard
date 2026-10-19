//! Digging into an idea. Only when asked (`tb idea dig I4 ["focus"]`, the app's Dig button), a
//! headless `claude -p` reads the project's checkout with Claude's read-only tools (and the web, when
//! asked) and reports whether the idea fits the code: what it builds on, how it could grow, its risks
//! and pitfalls, and what's still open. It validates the idea; it doesn't plan it.
//!
//! Each dig is a report round on the idea, kept with the commit it read. A dig that fails keeps its
//! error on the round. One idea is dug into once at a time; a dig the board lost (it stopped while
//! Claude read) is closed as failed the next time the idea is looked at.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, ideas, lessons, p, projects, relevance, triage};

/// A dig reads code (and maybe the web), so it gets as long as a backlog tidy.
const TIMEOUT_SECS: u64 = 900;
const FOCUS_MAX: usize = 1_000;
/// A round read at a commit this far behind the checkout's is stale.
pub const STALE_COMMITS: i64 = 50;
/// The most of each list kept from Claude's report, and the longest item.
const LIST_MAX: usize = 12;
const ITEM_MAX: usize = 600;
const RISK_KINDS: [&str; 4] = ["security", "privacy", "performance", "maintenance"];
const WEB_TOOLS: [&str; 2] = ["WebSearch", "WebFetch"];

/// `git -C repo <args>`'s trimmed output, when it succeeds.
fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let git = crate::proc::which("git")?;
    let mut all: Vec<String> = vec!["-C".into(), repo.to_string_lossy().to_string()];
    all.extend(args.iter().map(|a| a.to_string()));
    let out = crate::proc::run(&git, &all, None, 5.0).ok().filter(|o| o.code == Some(0))?;
    Some(out.stdout.trim().to_string()).filter(|s| !s.is_empty())
}

fn checkout(app: &App, project: &str) -> Result<Option<PathBuf>> {
    Ok(projects::project_path(app, Some(project))?.map(PathBuf::from).filter(|p| p.is_dir()))
}

/// How many commits the checkout's HEAD is past `sha`; None when `sha` isn't in its history.
fn behind(repo: &Path, sha: &str) -> Option<i64> {
    git(repo, &["rev-list", "--count", &format!("{sha}..HEAD")])?.parse().ok()
}

/// A round is stale when the checkout moved `STALE_COMMITS` past it, or its commit is gone (rewritten).
pub fn stale(behind: Option<i64>) -> bool {
    behind.is_none_or(|b| b >= STALE_COMMITS)
}

/// Adds `behind` (commits since the round's) and `stale` to each round of a project's idea.
pub fn mark_drift(app: &App, project: &str, rounds: &mut [Value]) -> Result<()> {
    let repo = checkout(app, project)?.filter(|repo| !rounds.is_empty() && git(repo, &["rev-parse", "HEAD"]).is_some());
    for r in rounds.iter_mut() {
        let (b, s) = match (&repo, r["sha"].as_str().filter(|s| !s.is_empty())) {
            (Some(repo), Some(sha)) => {
                let b = behind(repo, sha);
                (b, stale(b))
            }
            _ => (None, false),
        };
        r["behind"] = json!(b);
        r["stale"] = json!(s);
    }
    Ok(())
}

/// The state an idea goes back to once a dig is over: reported when a round has a report, else new.
fn settled_state(app: &App, id: i64) -> Result<&'static str> {
    let reported = app.db.count("SELECT COUNT(*) FROM idea_rounds WHERE idea_id = ? AND report IS NOT NULL", p![id])? > 0;
    Ok(if reported { "reported" } else { "new" })
}

/// Closes the digs the board lost: an idea left digging with no dig running (the board stopped).
pub fn settle_lost(app: &App) -> Result<()> {
    let running = app.shared.lock().digging.clone();
    for i in app.db.q("SELECT id FROM ideas WHERE state = 'digging'", p![])? {
        if running.contains(&i.id()) {
            continue;
        }
        app.db.tx(|| {
            app.db.x(
                "UPDATE idea_rounds SET error = ?, finished_at = ? WHERE idea_id = ? AND finished_at IS NULL",
                p!["the board stopped before Claude answered", now_iso(), i.id()],
            )?;
            app.db.update("ideas", &json!(i.id()), fields!["state" => settled_state(app, i.id())?, "updated_at" => now_iso()])
        })?;
    }
    Ok(())
}

/// `POST /ideas/:ref/dig {focus?, web?}`: start a dig. It runs on its own; the round is on the idea now.
pub fn start(app: &App, r: &str, body: &Value) -> Result<Value> {
    let i = ideas::get(app, r)?;
    let me = rf("idea", i.id());
    match i.st("state").as_str() {
        "planned" => return err(409, format!("{me} became a goal already; work on the goal instead.")),
        "shelved" | "dropped" => return err(409, format!("{me} is {}; reopen it first (tb idea reopen {me}).", i.st("state"))),
        _ => {}
    }
    let focus = body_str(body, "focus").trim().to_string();
    if focus.chars().count() > FOCUS_MAX {
        return err(400, format!("A dig's focus is {FOCUS_MAX} characters at most."));
    }
    let web = as_bool(body.get("web"), false);
    if crate::proc::which(&app.cfg.claude).is_none() {
        return err(409, "claude isn't installed, so there's nothing to dig with.");
    }
    let project = i.st("project");
    let Some(repo) = checkout(app, &project)? else {
        return err(409, format!("{me}'s project {project} has no checkout on this Mac to read."));
    };
    let sha = git(&repo, &["rev-parse", "HEAD"]);
    let (prompt, known) = prompt(app, &i, &focus, sha.as_deref(), web)?;
    if !app.shared.lock().digging.insert(i.id()) {
        return err(409, format!("{me} is being dug into already; wait for that report."));
    }
    let round = app.db.tx(|| {
        let now = now_iso();
        let round = app.db.insert(
            "idea_rounds",
            fields!["idea_id" => i.id(), "focus" => focus, "sha" => sha, "web" => web as i64, "started_at" => now,
                    "source" => board::caller(app, body, board::OWNER)],
        )?;
        app.db.update("ideas", &json!(i.id()), fields!["state" => "digging", "updated_at" => now])?;
        Ok(round)
    });
    let round = match round {
        Ok(r) => r,
        Err(e) => {
            app.shared.lock().digging.remove(&i.id());
            return Err(e);
        }
    };
    let id = i.id();
    app.queue_lens(Box::new(move |a: &App| {
        let extra: &[&str] = if web { &WEB_TOOLS } else { &[] };
        let answer = triage::ask_in_with(a, prompt, schema(), Some(&repo), extra, TIMEOUT_SECS).and_then(|v| checked(&v, &known));
        if let Err(e) = finish(a, id, round, answer) {
            a.info(format!("ideas: recording {}'s dig failed: {}", rf("idea", id), e.message));
        }
        a.shared.lock().digging.remove(&id);
    }));
    ideas::show(app, &ideas::get(app, r)?)
}

fn finish(app: &App, id: i64, round: i64, answer: std::result::Result<Value, String>) -> Result<()> {
    app.db.tx(|| {
        let now = now_iso();
        match &answer {
            Ok(report) => app.db.update("idea_rounds", &json!(round), fields!["report" => jdumps(report), "finished_at" => now])?,
            Err(e) => {
                app.info(format!("ideas: digging into {} failed: {e}", rf("idea", id)));
                app.db.update("idea_rounds", &json!(round), fields!["error" => e.as_str(), "finished_at" => now])?
            }
        }
        // Planned or put aside while Claude read: leave it.
        if app.db.q1("SELECT state FROM ideas WHERE id = ?", p![id])?.is_some_and(|i| i.s("state") == Some("digging")) {
            app.db.update("ideas", &json!(id), fields!["state" => settled_state(app, id)?, "updated_at" => now])?;
        }
        Ok(())
    })
}

// ------------------------------------------------------------------ asking Claude

fn schema() -> Value {
    let strings = json!({"type": "array", "items": {"type": "string"}});
    json!({"type": "object",
        "required": ["verdict", "why", "builds_on", "ways_to_extend", "risks", "pitfalls", "open_questions", "related"],
        "properties": {
            "verdict": {"enum": ["fits", "partly", "doesnt_fit"]},
            "why": {"type": "string"},
            "builds_on": {"type": "array", "items": {"type": "object", "required": ["what", "files"],
                "properties": {"what": {"type": "string"}, "files": strings}}},
            "ways_to_extend": strings,
            "risks": {"type": "array", "items": {"type": "object", "required": ["kind", "risk"],
                "properties": {"kind": {"enum": RISK_KINDS}, "risk": {"type": "string"}}}},
            "pitfalls": strings,
            "open_questions": strings,
            "related": strings}})
}

/// "- G3 (open): Ideas: record, dig and plan", for the board's related work.
fn card_line(c: &Value) -> String {
    let s = |k: &str| c[k].as_str().unwrap_or("").to_string();
    format!("- {} ({}): {}", s("ref"), s("state"), one_line(&s("title"), 200))
}

/// The prompt, and the board refs it shows (the only ones a report may call related).
fn prompt(app: &App, i: &Row, focus: &str, sha: Option<&str>, web: bool) -> Result<(String, HashSet<String>)> {
    let owner = &app.cfg.owner;
    let project = i.st("project");
    let me = rf("idea", i.id());
    let tools = if web { "Read, Grep, Glob, WebSearch and WebFetch" } else { "Read, Grep and Glob" };
    let at = sha.map(|s| format!(" at commit {}", &s[..s.len().min(12)])).unwrap_or_default();
    let mut parts = vec![format!(
        "{owner} has an idea for {project} that isn't ready to plan, and wants to know whether it holds up against \
         the code. You're in the project's checkout{at} and may read it ({tools}); you can't change anything.\n\n\
         Validate the idea; don't plan it. No tasks, waves, steps, estimates or schedules. Answer with:\n\
         - verdict: fits (it sits well with how the project works now), partly (it fits with changes to it or to \
         the code), or doesnt_fit; and why, in a few sentences.\n\
         - builds_on: what in the code it would build on, each with the files it's in (path, or path:line).\n\
         - ways_to_extend: how it could grow beyond what it asks for.\n\
         - risks: each with its kind: security, privacy, performance or maintenance.\n\
         - pitfalls: what's likely to go wrong building it.\n\
         - open_questions: what {owner} should settle before planning it.\n\
         - related: the refs of the board's work listed below that bear on it (only those).\n\
         Read the code before saying what the idea builds on, and name only files you read. Empty lists are fine."
    )];
    let mut idea = format!("The idea, {me}: {}", i.st("title"));
    let detail = i.st("detail");
    if !detail.trim().is_empty() {
        idea.push_str(&format!("\n{}", detail.trim()));
    }
    let notes = app.db.q("SELECT text FROM idea_notes WHERE idea_id = ? ORDER BY id", p![i.id()])?;
    if !notes.is_empty() {
        idea.push_str(&format!("\nNotes:\n{}", notes.iter().map(|n| format!("- {}", n.st("text").trim())).collect::<Vec<_>>().join("\n")));
    }
    parts.push(idea);
    parts.push(if focus.is_empty() {
        "This round looks at the whole idea.".to_string()
    } else {
        format!("This round's focus (dig into this most): {focus}")
    });
    let earlier = app.db.q("SELECT * FROM idea_rounds WHERE idea_id = ? AND report IS NOT NULL ORDER BY id", p![i.id()])?;
    if !earlier.is_empty() {
        let lines: Vec<String> = earlier.iter().map(round_line).collect();
        parts.push(format!("Earlier rounds (build on them; don't repeat them):\n{}", lines.join("\n")));
    }
    let mut known = HashSet::new();
    let text = format!("{}\n{}\n{focus}", i.st("title"), detail);
    let find = |kind: &str, open: bool, limit: usize| {
        relevance::find(app, &relevance::Find { text: &text, project: Some(&project), kind: Some(kind), open, limit })
    };
    let done: Vec<Value> = find("task", false, 40)?.into_iter().filter(|c| c["state"] == "done").take(8).collect();
    let mut related = vec![];
    for (head, cards) in [("Goals", find("goal", false, 5)?), ("Open backlog issues", find("issue", true, 8)?), ("Done tasks", done)] {
        if cards.is_empty() {
            continue;
        }
        known.extend(cards.iter().filter_map(|c| c["ref"].as_str().map(str::to_string)));
        related.push(format!("{head}:\n{}", cards.iter().map(card_line).collect::<Vec<_>>().join("\n")));
    }
    let w = relevance::Work { project: Some(&project), text: text.clone(), task: None };
    let picked = relevance::pick(app, &w, lessons::kept(app, Some(&project), &lessons::KINDS)?, 5, 0)?;
    if !picked.is_empty() {
        known.extend(picked.iter().map(|l| rf("lesson", l.id())));
        related.push(format!("Lessons:\n{}", lessons::lines(&picked).join("\n")));
    }
    if !related.is_empty() {
        parts.push(format!("The board's work that may bear on it:\n\n{}", related.join("\n\n")));
    }
    Ok((parts.join("\n\n"), known))
}

/// An earlier round for the prompt: its focus, verdict and why, and what it left open.
fn round_line(r: &Row) -> String {
    let rep: Value = r.s("report").and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
    let focus = r.s("focus").filter(|f| !f.is_empty()).unwrap_or("the whole idea");
    let mut line = format!("- {focus}: {} ({})", rep["verdict"].as_str().unwrap_or("?"), one_line(rep["why"].as_str().unwrap_or(""), 400));
    let open: Vec<&str> = rep["open_questions"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if !open.is_empty() {
        line.push_str(&format!(" Left open: {}", open.join("; ")));
    }
    line
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(Value::as_str).map(|s| one_line(s, ITEM_MAX)).filter(|s| !s.is_empty()).take(LIST_MAX).collect())
        .unwrap_or_default()
}

/// Claude's report made safe to keep: a known verdict, lists capped, risks of a known kind, and only
/// the related refs the prompt showed.
pub fn checked(v: &Value, known: &HashSet<String>) -> std::result::Result<Value, String> {
    let Some(verdict) = v["verdict"].as_str().filter(|x| ["fits", "partly", "doesnt_fit"].contains(x)) else {
        return Err("claude's report had no verdict".into());
    };
    let items = |k: &str| v[k].as_array().cloned().unwrap_or_default();
    let builds_on: Vec<Value> = items("builds_on")
        .iter()
        .filter_map(|b| {
            let what = one_line(b["what"].as_str().unwrap_or(""), ITEM_MAX);
            (!what.is_empty()).then(|| json!({"what": what, "files": strings(&b["files"])}))
        })
        .take(LIST_MAX)
        .collect();
    let risks: Vec<Value> = items("risks")
        .iter()
        .filter_map(|r| {
            let kind = r["kind"].as_str().filter(|k| RISK_KINDS.contains(k))?;
            let risk = one_line(r["risk"].as_str().unwrap_or(""), ITEM_MAX);
            (!risk.is_empty()).then(|| json!({"kind": kind, "risk": risk}))
        })
        .take(LIST_MAX)
        .collect();
    let mut related: Vec<String> = vec![];
    for r in strings(&v["related"]) {
        let r = r.trim().to_uppercase();
        if known.contains(&r) && !related.contains(&r) {
            related.push(r);
        }
    }
    Ok(json!({
        "verdict": verdict, "why": one_line(v["why"].as_str().unwrap_or(""), 2_000), "builds_on": builds_on,
        "ways_to_extend": strings(&v["ways_to_extend"]), "risks": risks, "pitfalls": strings(&v["pitfalls"]),
        "open_questions": strings(&v["open_questions"]), "related": related,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_are_checked_before_they_are_kept() {
        let known: HashSet<String> = ["G3", "B7", "L2"].iter().map(|s| s.to_string()).collect();
        let v = json!({"verdict": "partly", "why": "  It fits\nmostly. ",
            "builds_on": [{"what": "The tidy's headless call", "files": ["src/tidy.rs:240"]}, {"what": " ", "files": []}],
            "ways_to_extend": ["Dig on a schedule", ""],
            "risks": [{"kind": "security", "risk": "Reads secrets in the checkout"}, {"kind": "cost", "risk": "Pricey"}],
            "pitfalls": ["Long runs"], "open_questions": ["Web on by default?"],
            "related": ["g3", "G3", "T99", "B7"]});
        let r = checked(&v, &known).unwrap();
        assert_eq!(r["why"], "It fits mostly.");
        assert_eq!(r["builds_on"], json!([{"what": "The tidy's headless call", "files": ["src/tidy.rs:240"]}]));
        assert_eq!(r["ways_to_extend"], json!(["Dig on a schedule"]));
        assert_eq!(r["risks"], json!([{"kind": "security", "risk": "Reads secrets in the checkout"}]), "a risk of an unknown kind is left out");
        assert_eq!(r["related"], json!(["G3", "B7"]), "only refs the prompt showed, once each");
        assert!(checked(&json!({"verdict": "maybe"}), &known).is_err());
    }

    #[test]
    fn a_round_goes_stale_when_the_checkout_moves_far_or_its_commit_is_gone() {
        assert!(!stale(Some(0)));
        assert!(!stale(Some(STALE_COMMITS - 1)));
        assert!(stale(Some(STALE_COMMITS)));
        assert!(stale(None));
    }
}
