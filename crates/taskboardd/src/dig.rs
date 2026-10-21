//! Digging into an idea. Only when asked (`tb idea dig I4 ["focus"]`, the app's Dig button), a
//! headless `claude -p` reads the project's checkout with Claude's read-only tools (and the web, when
//! asked) and reports whether the idea fits the code: what it builds on, how it could grow, its risks
//! and pitfalls, and what's still open. It validates the idea; it doesn't plan it.
//!
//! A report is an ordered list of typed blocks (a gist, a scorecard, titled points, questions, ...)
//! that readers show in order, skipping types they don't know. Each point has a short line to glance
//! at and the detail behind it. Reports from before blocks are lists by heading; `report_sections`
//! reads both.
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
/// The most blocks kept from Claude's report, the most items in each list, and how long each part runs.
const BLOCKS_MAX: usize = 20;
const LIST_MAX: usize = 12;
const GIST_MAX: usize = 140;
const SHORT_MAX: usize = 90;
const DETAIL_MAX: usize = 1_200;
const NAME_MAX: usize = 60;
const FITS: [&str; 3] = ["fits", "partly", "doesnt_fit"];
const RISK_KINDS: [&str; 4] = ["security", "privacy", "performance", "maintenance"];
const WEB_TOOLS: [&str; 2] = ["WebSearch", "WebFetch"];

/// `git -C repo <args>`'s trimmed output, when it succeeds.
pub(crate) fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let git = crate::proc::which("git")?;
    let mut all: Vec<String> = vec!["-C".into(), repo.to_string_lossy().to_string()];
    all.extend(args.iter().map(|a| a.to_string()));
    let out = crate::proc::run(&git, &all, None, 5.0).ok().filter(|o| o.code == Some(0))?;
    Some(out.stdout.trim().to_string()).filter(|s| !s.is_empty())
}

pub(crate) fn checkout(app: &App, project: &str) -> Result<Option<PathBuf>> {
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
    // Read the set after the ideas: a dig joins it before its idea turns digging.
    for i in app.db.q("SELECT id FROM ideas WHERE state = 'digging'", p![])? {
        if app.shared.lock().digging.contains(&i.id()) {
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
    diggable(&i)?;
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
    {
        let mut s = app.shared.lock();
        if s.idea_planning.contains(&i.id()) {
            return err(409, format!("{me} is being planned into a goal; there's nothing left to dig into."));
        }
        if !s.digging.insert(i.id()) {
            return err(409, format!("{me} is being dug into already; wait for that report."));
        }
    }
    let round = app.db.tx(|| {
        // Shelved, dropped or planned while the prompt was put together: leave it.
        diggable(&ideas::get(app, r)?)?;
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

/// 409 unless the idea is open (new, digging or reported).
fn diggable(i: &Row) -> Result<()> {
    let me = rf("idea", i.id());
    match i.st("state").as_str() {
        "planned" => err(409, format!("{me} became a goal already; work on the goal instead.")),
        "shelved" | "dropped" => err(409, format!("{me} is {}; reopen it first (tb idea reopen {me}).", i.st("state"))),
        _ => Ok(()),
    }
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
    let block = |kind: &str, required: &[&str], props: Value| {
        let mut props = props;
        props["type"] = json!({"enum": [kind]});
        let mut required: Vec<&str> = required.to_vec();
        required.insert(0, "type");
        json!({"type": "object", "required": required, "properties": props})
    };
    let blocks = [
        block("gist", &["text"], json!({"text": {"type": "string"}})),
        block("scorecard", &["fit", "size", "risk"], json!({
            "fit": {"enum": FITS}, "size": {"enum": ["s", "m", "l"]}, "risk": {"enum": ["low", "medium", "high"]}})),
        block("points", &["title", "items"], json!({"title": {"type": "string"},
            "items": {"type": "array", "items": {"type": "object", "required": ["short", "detail"],
                "properties": {"short": {"type": "string"}, "detail": {"type": "string"}, "tag": {"enum": RISK_KINDS}}}}})),
        block("question", &["id", "text", "choices"], json!({"id": {"type": "string"}, "text": {"type": "string"}, "choices": strings})),
        block("compare", &["options"], json!({"lean": {"type": "string"},
            "options": {"type": "array", "items": {"type": "object", "required": ["name", "pros", "cons"],
                "properties": {"name": {"type": "string"}, "pros": strings, "cons": strings}}}})),
        block("flow", &["steps"], json!({"steps": {"type": "array", "items": {"type": "object", "required": ["name"],
            "properties": {"name": {"type": "string"}, "new": {"type": "boolean"}}}}})),
        block("code", &["items"], json!({"items": {"type": "array", "items": {"type": "object", "required": ["what", "file"],
            "properties": {"what": {"type": "string"}, "file": {"type": "string"}}}}})),
        block("related", &["refs"], json!({"refs": strings})),
    ];
    json!({"type": "object", "required": ["blocks"], "properties": {"blocks": {"type": "array", "items": {"anyOf": blocks}}}})
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
         Validate the idea; don't plan it. No tasks, waves, steps, estimates or schedules. Answer with blocks, in \
         the order {owner} should read them:\n\
         - gist, first: the verdict in one line of {GIST_MAX} characters at most.\n\
         - scorecard, next: fit is fits (it sits well with how the project works now), partly (it fits with changes \
         to it or to the code) or doesnt_fit; size is s, m or l; risk is low, medium or high.\n\
         - code: what in the code it would build on, each with the file it's in (path, or path:line).\n\
         - points, as many as help: a titled list, such as \"Watch out\" for its risks and pitfalls (tag a risk with \
         its kind: security, privacy, performance or maintenance) and \"Could grow into\" for how it could grow \
         beyond what it asks for.\n\
         - question, one each: what {owner} should settle before planning it, with a short slug id and two to four \
         choices to pick from.\n\
         - compare, when it could go two to four ways: each option's pros and cons, and the one you lean to.\n\
         - flow, when it moves something along a path: the steps in order, marking the new ones.\n\
         - related, last: the refs of the board's work listed below that bear on it (only those).\n\
         A point's short is a glance, {SHORT_MAX} characters at most; its detail keeps the reasoning, evidence and \
         files. Read the code before saying what the idea builds on, and name only files you read. Leave out \
         blocks with nothing to say."
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

/// An earlier round for the prompt: its focus, verdict and why, then the rest of its report.
fn round_line(r: &Row) -> String {
    let rep: Value = r.s("report").and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
    let focus = r.s("focus").filter(|f| !f.is_empty()).unwrap_or("the whole idea");
    let mut line = format!("- {focus}: {} ({})", rep["verdict"].as_str().unwrap_or("?"), one_line(rep["why"].as_str().unwrap_or(""), 400));
    for (head, items) in report_sections(&rep) {
        line.push_str(&format!("\n  {head}:\n{}", items.iter().map(|s| format!("    - {s}")).collect::<Vec<_>>().join("\n")));
    }
    line
}

fn texts(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

/// Adds lines under a heading, after any already there.
fn add(out: &mut Vec<(String, Vec<String>)>, head: &str, lines: Vec<String>) {
    if lines.is_empty() {
        return;
    }
    match out.iter_mut().find(|(h, _)| h == head) {
        Some((_, had)) => had.extend(lines),
        None => out.push((head.to_string(), lines)),
    }
}

/// A report as headings over lines, detail kept, for the planner, later rounds and `tb idea show`. It
/// reads block reports and the list-shaped ones from before them; the verdict and why aren't in it.
pub fn report_sections(rep: &Value) -> Vec<(String, Vec<String>)> {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let mut out = vec![];
    let Some(blocks) = rep["blocks"].as_array() else {
        let list = |k: &str| rep[k].as_array().cloned().unwrap_or_default();
        let builds = list("builds_on")
            .iter()
            .map(|b| {
                let files = texts(&b["files"]);
                if files.is_empty() { s(&b["what"]) } else { format!("{} ({})", s(&b["what"]), files.join(", ")) }
            })
            .collect();
        add(&mut out, "Builds on", builds);
        add(&mut out, "Ways to extend", texts(&rep["ways_to_extend"]));
        add(&mut out, "Risks", list("risks").iter().map(|r| format!("{}: {}", s(&r["kind"]), s(&r["risk"]))).collect());
        add(&mut out, "Pitfalls", texts(&rep["pitfalls"]));
        add(&mut out, "Open questions", texts(&rep["open_questions"]));
        add(&mut out, "Related", texts(&rep["related"]));
        return out;
    };
    let list = |b: &Value, k: &str| b[k].as_array().cloned().unwrap_or_default();
    for b in blocks {
        match b["type"].as_str().unwrap_or("") {
            "scorecard" => add(&mut out, "Scorecard", vec![format!("fit: {} · size: {} · risk: {}", s(&b["fit"]), s(&b["size"]), s(&b["risk"]))]),
            "code" => add(&mut out, "Builds on", list(b, "items").iter().map(|c| {
                if s(&c["file"]).is_empty() { s(&c["what"]) } else { format!("{} ({})", s(&c["what"]), s(&c["file"])) }
            }).collect()),
            "points" => {
                let title = s(&b["title"]);
                let lines = list(b, "items").iter().map(|p| {
                    let tag = p["tag"].as_str().map(|t| format!("{t}: ")).unwrap_or_default();
                    let detail = p["detail"].as_str().filter(|d| !d.is_empty()).map(|d| format!(" — {d}")).unwrap_or_default();
                    format!("{tag}{}{detail}", s(&p["short"]))
                }).collect();
                add(&mut out, if title.is_empty() { "Points" } else { &title }, lines);
            }
            "question" => {
                let choices = texts(&b["choices"]);
                let choices = if choices.is_empty() { String::new() } else { format!(" (choices: {})", choices.join(" / ")) };
                add(&mut out, "Open questions", vec![format!("{}{choices}", s(&b["text"]))]);
            }
            "compare" => add(&mut out, "Options", list(b, "options").iter().map(|o| {
                let lean = if o["name"] == b["lean"] { " (leaning)" } else { "" };
                let side = |k: &str, head: &str| {
                    let xs = texts(&o[k]);
                    if xs.is_empty() { String::new() } else { format!(" — {head}: {}", xs.join("; ")) }
                };
                format!("{}{lean}{}{}", s(&o["name"]), side("pros", "pros"), side("cons", "cons"))
            }).collect()),
            "flow" => add(&mut out, "Flow", vec![list(b, "steps").iter().map(|st| {
                if st["new"] == true { format!("{} (new)", s(&st["name"])) } else { s(&st["name"]) }
            }).collect::<Vec<_>>().join(" → ")]),
            "related" => add(&mut out, "Related", texts(&b["refs"])),
            _ => {}
        }
    }
    out
}

/// Strings off a list, each one line clipped to `max`, empty ones dropped, `LIST_MAX` at most.
fn strings(v: &Value, max: usize) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(Value::as_str).map(|s| one_line(s, max)).filter(|s| !s.is_empty()).take(LIST_MAX).collect())
        .unwrap_or_default()
}

/// A question's id as a slug: lowercase letters, digits and dashes.
fn slug(id: &str) -> String {
    let s: String = id.trim().to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-").chars().take(40).collect::<String>().trim_end_matches('-').to_string()
}

/// One of Claude's blocks made safe to keep; None for a type this board doesn't know or a block with
/// nothing in it. `ids` holds the question ids taken so far, so each is unique.
fn block(b: &Value, known: &HashSet<String>, ids: &mut HashSet<String>) -> Option<Value> {
    let line = |v: &Value, max: usize| one_line(v.as_str().unwrap_or(""), max);
    let list = |k: &str| b[k].as_array().cloned().unwrap_or_default();
    let kind = b["type"].as_str()?;
    let kept = match kind {
        "gist" => {
            let text = line(&b["text"], GIST_MAX);
            (!text.is_empty()).then(|| json!({"text": text}))?
        }
        "scorecard" => {
            let pick = |k: &str, of: &[&str]| b[k].as_str().filter(|x| of.contains(x)).map(str::to_string);
            let fit = pick("fit", &FITS)?;
            json!({"fit": fit, "size": pick("size", &["s", "m", "l"]), "risk": pick("risk", &["low", "medium", "high"])})
        }
        "points" => {
            let items: Vec<Value> = list("items")
                .iter()
                .filter_map(|p| {
                    let detail = clip(p["detail"].as_str().unwrap_or("").trim(), DETAIL_MAX);
                    let short = Some(line(&p["short"], SHORT_MAX)).filter(|s| !s.is_empty()).unwrap_or_else(|| one_line(&detail, SHORT_MAX));
                    if short.is_empty() {
                        return None;
                    }
                    let mut item = json!({"short": short, "detail": detail});
                    if let Some(tag) = p["tag"].as_str().filter(|t| RISK_KINDS.contains(t)) {
                        item["tag"] = json!(tag);
                    }
                    Some(item)
                })
                .take(LIST_MAX)
                .collect();
            (!items.is_empty()).then(|| json!({"title": line(&b["title"], NAME_MAX), "items": items}))?
        }
        "question" => {
            let text = line(&b["text"], 200);
            if text.is_empty() {
                return None;
            }
            let mut id = slug(b["id"].as_str().unwrap_or(""));
            if id.is_empty() || ids.contains(&id) {
                id = (1..).map(|n| format!("q{n}")).find(|q| !ids.contains(q))?;
            }
            ids.insert(id.clone());
            let mut choices = strings(&b["choices"], NAME_MAX);
            choices.dedup();
            choices.truncate(4);
            json!({"id": id, "text": text, "choices": choices})
        }
        "compare" => {
            let options: Vec<Value> = list("options")
                .iter()
                .filter_map(|o| {
                    let name = line(&o["name"], NAME_MAX);
                    (!name.is_empty()).then(|| json!({"name": name, "pros": strings(&o["pros"], SHORT_MAX), "cons": strings(&o["cons"], SHORT_MAX)}))
                })
                .take(4)
                .collect();
            if options.len() < 2 {
                return None;
            }
            let lean = line(&b["lean"], NAME_MAX);
            let lean = options.iter().any(|o| o["name"] == lean.as_str()).then_some(lean);
            json!({"lean": lean, "options": options})
        }
        "flow" => {
            let steps: Vec<Value> = list("steps")
                .iter()
                .filter_map(|st| {
                    let name = line(&st["name"], 40);
                    (!name.is_empty()).then(|| json!({"name": name, "new": st["new"].as_bool().unwrap_or(false)}))
                })
                .take(8)
                .collect();
            (steps.len() >= 2).then(|| json!({"steps": steps}))?
        }
        "code" => {
            let items: Vec<Value> = list("items")
                .iter()
                .filter_map(|c| {
                    let what = line(&c["what"], SHORT_MAX);
                    (!what.is_empty()).then(|| json!({"what": what, "file": line(&c["file"], 200)}))
                })
                .take(LIST_MAX)
                .collect();
            (!items.is_empty()).then(|| json!({"items": items}))?
        }
        "related" => {
            let mut refs: Vec<String> = vec![];
            for r in strings(&b["refs"], 20) {
                let r = r.to_uppercase();
                if known.contains(&r) && !refs.contains(&r) {
                    refs.push(r);
                }
            }
            (!refs.is_empty()).then(|| json!({"refs": refs}))?
        }
        _ => return None,
    };
    let mut kept = kept;
    kept["type"] = json!(kind);
    Some(kept)
}

/// Claude's report made safe to keep: its known blocks in order, each capped, with one gist, one
/// scorecard (it must have one with a known fit) and one related, which names only refs the prompt
/// showed. The verdict and why repeat the scorecard's fit and the gist, for readers of the headline.
pub fn checked(v: &Value, known: &HashSet<String>) -> std::result::Result<Value, String> {
    let mut blocks: Vec<Value> = vec![];
    let mut ids = HashSet::new();
    for b in v["blocks"].as_array().into_iter().flatten() {
        let once = ["gist", "scorecard", "related"].contains(&b["type"].as_str().unwrap_or(""));
        if once && blocks.iter().any(|k| k["type"] == b["type"]) {
            continue;
        }
        blocks.extend(block(b, known, &mut ids));
        if blocks.len() == BLOCKS_MAX {
            break;
        }
    }
    let first = |kind: &str, k: &str| blocks.iter().find(|b| b["type"] == kind).map(|b| b[k].clone());
    let Some(verdict) = first("scorecard", "fit") else {
        return Err("claude's report had no verdict".into());
    };
    let why = first("gist", "text").unwrap_or(json!(""));
    Ok(json!({"verdict": verdict, "why": why, "blocks": blocks}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_are_checked_before_they_are_kept() {
        let known: HashSet<String> = ["G3", "B7", "L2"].iter().map(|s| s.to_string()).collect();
        let long = "x".repeat(200);
        let v = json!({"blocks": [
            {"type": "gist", "text": format!("  It fits\nmostly. {long}")},
            {"type": "scorecard", "fit": "partly", "size": "xl", "risk": "medium"},
            {"type": "gist", "text": "A second gist"},
            {"type": "chart", "data": [1, 2]},
            {"type": "points", "title": "Watch out", "items": [
                {"short": "Reads secrets", "detail": "The checkout may hold .env files.\n\nClaude reads them.", "tag": "security"},
                {"short": " ", "detail": "Long runs tie up a lens slot", "tag": "cost"},
                {"short": "", "detail": ""}]},
            {"type": "points", "title": "Empty", "items": []},
            {"type": "question", "id": "Web by default?", "text": "Web on by default?", "choices": ["Yes", "No"]},
            {"type": "question", "id": "web-by-default", "text": "Which web tools?", "choices": []},
            {"type": "compare", "lean": "Nope", "options": [{"name": "A", "pros": ["fast"], "cons": []}, {"name": "B", "pros": [], "cons": []}]},
            {"type": "compare", "options": [{"name": "Alone", "pros": [], "cons": []}]},
            {"type": "flow", "steps": [{"name": "Phone"}, {"name": "Relay", "new": true}]},
            {"type": "code", "items": [{"what": "The tidy's headless call", "file": "src/tidy.rs:240"}, {"what": " ", "file": "x"}]},
            {"type": "related", "refs": ["g3", "G3", "T99", "B7"]}]});
        let r = checked(&v, &known).unwrap();
        assert_eq!(r["verdict"], "partly");
        let why = r["why"].as_str().unwrap();
        assert!(why.starts_with("It fits mostly. xxx") && why.chars().count() == GIST_MAX, "the first gist, one line and clipped: {why}");
        let blocks = r["blocks"].as_array().unwrap();
        let kinds: Vec<&str> = blocks.iter().map(|b| b["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["gist", "scorecard", "points", "question", "question", "compare", "flow", "code", "related"],
            "unknown, repeated and empty blocks are dropped; the order is kept");
        assert_eq!(blocks[1], json!({"type": "scorecard", "fit": "partly", "size": null, "risk": "medium"}));
        assert_eq!(blocks[2]["items"], json!([
            {"short": "Reads secrets", "detail": "The checkout may hold .env files.\n\nClaude reads them.", "tag": "security"},
            {"short": "Long runs tie up a lens slot", "detail": "Long runs tie up a lens slot"}]),
            "detail keeps its paragraphs; a missing short comes from the detail; an unknown tag is left off");
        assert_eq!((blocks[3]["id"].clone(), blocks[4]["id"].clone()), (json!("web-by-default"), json!("q1")), "ids are unique slugs");
        assert_eq!(blocks[5]["lean"], Value::Null, "a lean that isn't an option is left out");
        assert_eq!(blocks[6]["steps"][1], json!({"name": "Relay", "new": true}));
        assert_eq!(blocks[7]["items"], json!([{"what": "The tidy's headless call", "file": "src/tidy.rs:240"}]));
        assert_eq!(blocks[8]["refs"], json!(["G3", "B7"]), "only refs the prompt showed, once each");
        assert!(checked(&json!({"blocks": [{"type": "gist", "text": "No card"}]}), &known).is_err());
        assert!(checked(&json!({"blocks": [{"type": "scorecard", "fit": "maybe"}]}), &known).is_err());
    }

    #[test]
    fn report_sections_read_blocks_and_older_reports() {
        let blocks = json!({"verdict": "partly", "why": "Mostly", "blocks": [
            {"type": "gist", "text": "Mostly"},
            {"type": "scorecard", "fit": "partly", "size": "m", "risk": "low"},
            {"type": "code", "items": [{"what": "Event stream", "file": "src/events.rs:40"}]},
            {"type": "points", "title": "Watch out", "items": [{"short": "Leaks", "detail": "Why it leaks", "tag": "privacy"}]},
            {"type": "question", "id": "relay", "text": "Which relay?", "choices": ["Host", "Push"]},
            {"type": "compare", "lean": "Host", "options": [{"name": "Host", "pros": ["ours"], "cons": []}, {"name": "Push", "pros": [], "cons": ["cost"]}]},
            {"type": "flow", "steps": [{"name": "Phone", "new": false}, {"name": "Relay", "new": true}]},
            {"type": "someday", "x": 1},
            {"type": "related", "refs": ["G5"]}]});
        let lines = |rep: &Value| report_sections(rep).into_iter().map(|(h, l)| format!("{h}: {}", l.join(" | "))).collect::<Vec<_>>();
        assert_eq!(lines(&blocks), [
            "Scorecard: fit: partly · size: m · risk: low",
            "Builds on: Event stream (src/events.rs:40)",
            "Watch out: privacy: Leaks — Why it leaks",
            "Open questions: Which relay? (choices: Host / Push)",
            "Options: Host (leaning) — pros: ours | Push — cons: cost",
            "Flow: Phone → Relay (new)",
            "Related: G5",
        ]);
        let old = json!({"verdict": "fits", "why": "Yes", "builds_on": [{"what": "Store", "files": ["a.rs", "b.rs"]}],
            "ways_to_extend": [], "risks": [{"kind": "security", "risk": "Secrets"}], "pitfalls": ["Slow"],
            "open_questions": ["Web?"], "related": ["G3"]});
        assert_eq!(lines(&old), ["Builds on: Store (a.rs, b.rs)", "Risks: security: Secrets", "Pitfalls: Slow", "Open questions: Web?", "Related: G3"]);
    }

    #[test]
    fn a_round_goes_stale_when_the_checkout_moves_far_or_its_commit_is_gone() {
        assert!(!stale(Some(0)));
        assert!(!stale(Some(STALE_COMMITS - 1)));
        assert!(stale(Some(STALE_COMMITS)));
        assert!(stale(None));
    }
}
