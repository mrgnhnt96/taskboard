//! Reviewing a goal's plan against its outcome before it runs. Once `tb goal new` or `tb propose` adds
//! tasks, a headless `claude -p` reads the goal (its outcome, notes and tasks with their waves, waits and
//! stacks), the project's checkout, its planning rules and kept planning lessons, and the project's other
//! open goals, and says where the plan misses: outcome clauses no task proves, things only the owner can
//! do with no human task for them, tasks planned past an undecided decision, overlap with other goals,
//! and lessons the plan goes against. It never sees the planner's conversation. When the outcome points
//! at a source the plan must match (a review, a doc, a design, a list of issues), it reads that source
//! (the goal's attachments, with local text files inlined, the backlog issues named, links through
//! WebFetch) and flags each of its items no task covers. Before it reports, it checks in the checkout
//! that what each task names or relies on (files, functions, commands, docs) is there and does what the
//! plan says, within a cap on its turns and spending.
//!
//! The review is advice: nothing waits for it. Each run is a row of `plan_reviews`, kept with the commit
//! it read; the latest one is the goal's review. A run that can't start (the review is off, Claude isn't
//! there, the usage is out) is kept as skipped with why. One goal is reviewed once at a time: a plan
//! changed while it runs is reviewed again once it's done. A run the board lost (it stopped while Claude
//! read) is closed as failed the next time the goal's review is looked at.

use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, dig, fields, lessons, p, planning, triage};

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS plan_reviews(
  id INTEGER PRIMARY KEY, goal_id INT NOT NULL, reason TEXT, source TEXT, model TEXT, sha TEXT,
  started_at TEXT, finished_at TEXT, findings TEXT, error TEXT, skipped TEXT);
CREATE INDEX IF NOT EXISTS plan_reviews_goal ON plan_reviews(goal_id, id);
"#;

/// A review reads code like a dig does, so it gets as long.
const TIMEOUT_SECS: u64 = 900;
/// Turns to check what the tasks assume in the checkout and answer: a turn is one round of tool calls,
/// and a plan's checks run several reads at once.
const MAX_TURNS: u32 = 30;
/// The spending cap of one review: `[backlog] budget_usd` when it's more.
const BUDGET_USD: f64 = 2.0;
pub const KINDS: [&str; 5] = ["outcome_unproven", "owner_prereq", "past_decision", "overlap", "lesson_ignored"];
const FINDINGS_MAX: usize = 20;
const REFS_MAX: usize = 12;
const SHORT_MAX: usize = 90;
const DETAIL_MAX: usize = 1_200;
const TASK_DETAIL_MAX: usize = 1_500;
const OTHER_GOALS_MAX: usize = 15;
const OTHER_TASKS_MAX: usize = 12;
const SOURCE_ATTACH_MAX: usize = 12;
const SOURCE_ISSUES_MAX: usize = 30;
/// A local text file attached as a source is read when it's at most this big, and inlined clipped.
const SOURCE_FILE_BYTES: u64 = 512 * 1024;
const SOURCE_TEXT_MAX: usize = 12_000;
const SOURCES_TEXT_MAX: usize = 30_000;
/// Opens a source's link; given only when the goal points at one.
const WEB_TOOLS: [&str; 1] = ["WebFetch"];
pub const LOST: &str = "the board stopped before Claude answered";

/// Why a review can't run now; None when it can.
pub(crate) fn held(app: &App) -> Option<&'static str> {
    if !app.cfg.planning.review {
        return Some("plan review is off ([planning] review in config.toml)");
    }
    if !triage::ai_on(app) {
        return Some("Claude isn't available to the board (backlog AI is off or claude isn't installed)");
    }
    if crate::usage::out_until(app).is_some() {
        return Some("the usage is used up");
    }
    if crate::usage_daily::limited_until(app).is_some() {
        return Some("today's share of the usage is used");
    }
    None
}

/// `[planning] review_model`: `small` or `large` through `[models]`, a model name as it is, and
/// `[backlog] model` when it's empty or `[models]` is off.
pub fn model(app: &App) -> String {
    let want = app.cfg.planning.review_model.trim();
    let fallback = || app.cfg.backlog.model.clone();
    match want {
        "" => fallback(),
        "small" | "large" => app.cfg.models.resolve(Some(want)).unwrap_or_else(fallback),
        m => m.to_string(),
    }
}

/// Reviews the goal's plan on its own, or keeps why it can't. A review already running is run again
/// once it's done, so it sees the plan as it is then. Never fails the caller: the review is advice.
pub fn queue(app: &App, gid: i64, reason: &str, source: &str) {
    if let Err(e) = try_queue(app, gid, reason, source) {
        app.info(format!("plan review: {} couldn't start: {}", rf("goal", gid), e.message));
    }
}

fn try_queue(app: &App, gid: i64, reason: &str, source: &str) -> Result<()> {
    {
        let mut s = app.shared.lock();
        if s.plan_reviewing.contains(&gid) {
            s.plan_review_again.insert(gid);
            return Ok(());
        }
    }
    let upto = crate::plan_review_again::upto(app, gid)?;
    if let Some(why) = held(app) {
        let now = now_iso();
        app.db.insert(
            "plan_reviews",
            fields!["goal_id" => gid, "reason" => reason, "source" => source, "started_at" => now, "finished_at" => now, "skipped" => why, "upto" => upto],
        )?;
        return Ok(());
    }
    if !app.shared.lock().plan_reviewing.insert(gid) {
        app.shared.lock().plan_review_again.insert(gid);
        return Ok(());
    }
    let model = model(app);
    let round = app.db.insert(
        "plan_reviews",
        fields!["goal_id" => gid, "reason" => reason, "source" => source, "model" => model, "started_at" => now_iso(), "upto" => upto],
    );
    let round = match round {
        Ok(r) => r,
        Err(e) => {
            app.shared.lock().plan_reviewing.remove(&gid);
            return Err(e);
        }
    };
    let source = source.to_string();
    app.queue_slow(Box::new(move |a: &App| {
        let answer = run(a, gid, round, &model);
        if let Err(e) = finish(a, gid, round, answer) {
            a.info(format!("plan review: recording {}'s review failed: {}", rf("goal", gid), e.message));
        }
        let again = {
            let mut s = a.shared.lock();
            s.plan_reviewing.remove(&gid);
            s.plan_review_again.remove(&gid)
        };
        if again {
            queue(a, gid, "the plan changed while it was reviewed", &source);
        }
    }));
    Ok(())
}

fn run(app: &App, gid: i64, round: i64, model: &str) -> std::result::Result<Value, String> {
    let g = board::get_goal(app, gid).map_err(|e| e.message)?;
    let repo = dig::checkout(app, &g.st("project")).map_err(|e| e.message)?;
    let sha = repo.as_deref().and_then(|r| dig::git(r, &["rev-parse", "HEAD"]));
    if let Some(s) = &sha {
        app.db.update("plan_reviews", &json!(round), fields!["sha" => s.as_str()]).map_err(|e| e.message)?;
    }
    let src = sources(app, &g).map_err(|e| e.message)?;
    let (prompt, known) = prompt(app, &g, sha.as_deref(), repo.is_some(), &src).map_err(|e| e.message)?;
    let extra: &[&str] = if src.web { &WEB_TOOLS } else { &[] };
    triage::ask_model_limited(app, model, prompt, schema(repo.is_some()), repo.as_deref(), extra, TIMEOUT_SECS, &budget(app), Some(MAX_TURNS))
        .and_then(|v| checked(&v, &known))
}

/// One review's spending cap: [`BUDGET_USD`], or `[backlog] budget_usd` when that's more.
fn budget(app: &App) -> String {
    let backlog = app.cfg.backlog.budget_usd.trim().parse::<f64>().unwrap_or(0.0);
    format!("{:.2}", backlog.max(BUDGET_USD))
}

fn finish(app: &App, gid: i64, round: i64, answer: std::result::Result<Value, String>) -> Result<()> {
    let now = now_iso();
    match &answer {
        Ok(v) => app.db.update("plan_reviews", &json!(round), fields!["findings" => jdumps(&v["findings"]), "finished_at" => now]),
        Err(e) => {
            app.info(format!("plan review: reviewing {} failed: {e}", rf("goal", gid)));
            app.db.update("plan_reviews", &json!(round), fields!["error" => e.as_str(), "finished_at" => now])
        }
    }
}

/// Closes the goal's reviews the board lost: started, not finished, and none running.
fn settle_lost(app: &App, gid: i64) -> Result<()> {
    // Read the set after the rows: a review joins it before its row is made.
    let open = app.db.count("SELECT COUNT(*) FROM plan_reviews WHERE goal_id = ? AND finished_at IS NULL", p![gid])?;
    if open == 0 || app.shared.lock().plan_reviewing.contains(&gid) {
        return Ok(());
    }
    app.db.x(
        "UPDATE plan_reviews SET error = ?, finished_at = ? WHERE goal_id = ? AND finished_at IS NULL",
        p![LOST, now_iso(), gid],
    )?;
    Ok(())
}

/// Earlier rounds `GET /goals/:ref/review` lists under the latest one.
const EARLIER_MAX: i64 = 10;

/// The goal's latest review for the API: its round, its state (running, done, failed or skipped), why it
/// ran, when, and what it found. Null when the goal was never reviewed.
pub fn card(app: &App, gid: i64) -> Result<Value> {
    settle_lost(app, gid)?;
    let Some(r) = app.db.q1("SELECT * FROM plan_reviews WHERE goal_id = ? ORDER BY id DESC LIMIT 1", p![gid])? else {
        return Ok(Value::Null);
    };
    let rounds = app.db.count("SELECT COUNT(*) FROM plan_reviews WHERE goal_id = ?", p![gid])?;
    let mut card = round_card(&r, rounds);
    card["again"] = json!(app.shared.lock().plan_review_again.contains(&gid));
    Ok(card)
}

fn round_card(r: &Row, round: i64) -> Value {
    let state = if r.s("finished_at").is_none() {
        "running"
    } else if r.s("skipped").is_some() {
        "skipped"
    } else if r.s("error").is_some() {
        "failed"
    } else {
        "done"
    };
    let findings = r.s("findings").and_then(|f| serde_json::from_str::<Value>(f).ok()).filter(Value::is_array).unwrap_or(json!([]));
    json!({
        "id": r.id(), "goal": rf("goal", r.i0("goal_id")), "round": round, "state": state, "reason": r.v("reason"), "source": r.v("source"),
        "model": r.v("model"), "sha": r.v("sha"), "started_at": r.v("started_at"), "finished_at": r.v("finished_at"),
        "skipped": r.v("skipped"), "error": r.v("error"), "findings": findings, "again": false,
    })
}

/// `GET /goals/:ref/review`: the goal's latest plan review, and the rounds before it newest first, each
/// with what it found then.
pub fn show(app: &App, gid: i64) -> Result<Value> {
    board::get_goal(app, gid)?;
    let review = card(app, gid)?;
    let latest = review["round"].as_i64().unwrap_or(0);
    let rows = app.db.q("SELECT * FROM plan_reviews WHERE goal_id = ? ORDER BY id DESC LIMIT ? OFFSET 1", p![gid, EARLIER_MAX])?;
    let earlier: Vec<Value> = rows.iter().zip(1..).map(|(r, back)| round_card(r, latest - back)).collect();
    Ok(json!({"goal": rf("goal", gid), "review": review, "earlier": earlier}))
}

/// `POST /goals/:ref/review`: review the plan again now. 409 when it can't run; while one runs, that one.
pub fn start(app: &App, gid: i64, body: &Value) -> Result<Value> {
    let me = rf("goal", gid);
    board::get_goal(app, gid)?;
    if board::goal_tasks(app, gid)?.is_empty() {
        return err(409, format!("{me} has no tasks yet, so there's no plan to review."));
    }
    if let Some(why) = held(app) {
        return err(409, format!("{me}'s plan can't be reviewed now: {why}."));
    }
    if !app.shared.lock().plan_reviewing.contains(&gid) {
        try_queue(app, gid, "asked", &board::caller(app, body, board::OWNER))?;
    }
    show(app, gid)
}

// ------------------------------------------------------------------ asking Claude

/// The answer: findings, and with a checkout the checks Claude made in it (asked for so it reads before
/// it answers; not kept).
fn schema(read: bool) -> Value {
    let strings = json!({"type": "array", "items": {"type": "string"}});
    let checked = json!({"type": "array", "items": {"type": "object", "required": ["task", "assumes", "found"], "properties": {
        "task": {"type": "string"}, "assumes": {"type": "string"}, "found": {"type": "string"}}}});
    let required = if read { json!(["checked", "findings"]) } else { json!(["findings"]) };
    json!({"type": "object", "required": required, "properties": {"checked": checked, "findings": {"type": "array", "items": {
        "type": "object", "required": ["kind", "short", "detail"],
        "properties": {"kind": {"enum": KINDS}, "short": {"type": "string"}, "detail": {"type": "string"},
                       "tasks": strings, "refs": strings}}}}})
}

/// The refs a finding may name: the goal's tasks, and the other goals and lessons the prompt showed.
pub struct Known {
    pub tasks: HashSet<String>,
    pub refs: HashSet<String>,
}

fn task_line(t: &Row) -> String {
    let mut about = vec![format!("wave {}", t.i("wave").map(|w| w.to_string()).unwrap_or_else(|| "none".into()))];
    if let Some(k) = t.s("kind").filter(|k| !k.is_empty()) {
        about.push(format!("{k} task"));
    }
    let waits = crate::waitsfor::ids(t);
    if !waits.is_empty() {
        about.push(format!("waits for {}", waits.iter().map(|n| rf("task", *n)).collect::<Vec<_>>().join(", ")));
    }
    if let Some(parent) = crate::stack::parent_id(t) {
        about.push(format!("stacks on {} ({}-merge)", rf("task", parent), crate::stack::mode(t)));
    }
    let held = crate::locks::names(t);
    if !held.is_empty() {
        about.push(format!("holds {}", held.join(", ")));
    }
    let mut line = format!("- {} [{}] {} ({})", rf("task", t.id()), t.st("status"), one_line(&t.st("title"), 200), about.join("; "));
    let detail = t.st("detail");
    if !detail.trim().is_empty() {
        line.push_str(&format!("\n  {}", one_line(&detail, TASK_DETAIL_MAX)));
    }
    line
}

/// The project's other open goals (not archived, a task not done or no tasks yet), with what they plan.
fn other_goals(app: &App, g: &Row) -> Result<(Vec<String>, HashSet<String>)> {
    let rows = app.db.q(
        "SELECT * FROM goals g WHERE project = ? AND id != ? AND COALESCE(archived, 0) = 0
           AND (NOT EXISTS (SELECT 1 FROM tasks t WHERE t.goal_id = g.id)
                OR EXISTS (SELECT 1 FROM tasks t WHERE t.goal_id = g.id AND t.status != 'done'))
         ORDER BY id DESC LIMIT ?",
        p![g.st("project"), g.id(), OTHER_GOALS_MAX as i64],
    )?;
    let mut lines = vec![];
    let mut known = HashSet::new();
    for o in rows {
        let me = rf("goal", o.id());
        let mut line = format!("- {me} “{}”", one_line(&o.st("name"), 200));
        if let Some(out) = o.s("outcome").filter(|s| !s.trim().is_empty()) {
            line.push_str(&format!(", done when: {}", one_line(out, 400)));
        }
        let tasks: Vec<String> = board::goal_tasks(app, o.id())?
            .iter()
            .filter(|t| t.s("status") != Some("done"))
            .take(OTHER_TASKS_MAX)
            .map(|t| format!("{} {}", rf("task", t.id()), one_line(&t.st("title"), 120)))
            .collect();
        if !tasks.is_empty() {
            line.push_str(&format!("\n  Open tasks: {}", tasks.join("; ")));
        }
        lines.push(line);
        known.insert(me);
    }
    Ok((lines, known))
}

/// What the goal points at that the plan must match, beyond its own text: its attachments (and its
/// tasks' designs, proposals and docs), the backlog issues its outcome, TLDR or notes name, and whether
/// any of it is a link Claude may open.
struct Sources {
    block: Option<String>,
    issues: HashSet<String>,
    web: bool,
}

static ISSUE_REF_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b[Bb](\d+)\b").unwrap());
static LINK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"https?://[^\s<>()\[\]"'`]+"#).unwrap());

/// A link WebFetch can open: not a claude.ai artifact, whose page needs a sign-in.
fn openable(url: &str) -> bool {
    let l = url.to_ascii_lowercase();
    (l.starts_with("http://") || l.starts_with("https://")) && !l.starts_with("https://claude.ai/")
}

/// A local text file's contents, clipped to `room`; None for a link, a folder, a binary or a big file.
fn file_text(url: &str, room: usize) -> Option<String> {
    if room == 0 || url.contains("://") {
        return None;
    }
    let path = expand_home(url);
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > SOURCE_FILE_BYTES {
        return None;
    }
    let text = String::from_utf8(std::fs::read(&path).ok()?).ok()?;
    let text = text.trim();
    (!text.is_empty() && !text.contains('\0')).then(|| clip(text, SOURCE_TEXT_MAX.min(room)))
}

fn attachment_lines(app: &App, g: &Row, tasks: &[Row]) -> Result<(Vec<String>, bool)> {
    let mut atts: Vec<Value> = board::attachments(app, None, Some(g.id()))?
        .into_iter()
        .filter(|a| !matches!(a["kind"].as_str(), Some("evidence" | "results")))
        .collect();
    for t in tasks {
        atts.extend(
            board::attachments(app, Some(t.id()), None)?
                .into_iter()
                .filter(|a| matches!(a["kind"].as_str(), Some("design" | "proposal" | "doc"))),
        );
    }
    let mut lines = vec![];
    let mut web = false;
    let mut room = SOURCES_TEXT_MAX;
    for a in atts.iter().take(SOURCE_ATTACH_MAX) {
        let url = a["url"].as_str().unwrap_or("");
        let on = a["task"].as_str().map(|t| format!(" (on {t})")).unwrap_or_default();
        let mut line = format!("- {}{on}: {} {url}", a["kind"].as_str().unwrap_or("other"), a["title"].as_str().unwrap_or(""));
        if let Some(s) = a["summary"].as_str() {
            line.push_str(&format!(" — {s}"));
        }
        if openable(url) {
            web = true;
        } else if url.contains("://") {
            line.push_str("\n  (a claude.ai artifact the board can't open: go by its title and summary)");
        } else if let Some(text) = file_text(url, room) {
            room = room.saturating_sub(text.chars().count());
            line.push_str(&format!("\n  Its contents:\n{}", text.lines().map(|l| format!("  | {l}")).collect::<Vec<_>>().join("\n")));
        }
        lines.push(line);
    }
    if atts.len() > SOURCE_ATTACH_MAX {
        lines.push(format!("- {} more attached on the goal page.", atts.len() - SOURCE_ATTACH_MAX));
    }
    Ok((lines, web))
}

/// The goal's sources (see [`Sources`]).
fn sources(app: &App, g: &Row) -> Result<Sources> {
    let tasks = board::goal_tasks(app, g.id())?;
    let notes = board::goal_notes(app, g.id())?;
    let mut text = vec![g.st("outcome"), g.st("tldr")];
    text.extend(notes.iter().map(|n| n.st("text")));
    let text = text.join("\n");
    let (mut lines, mut web) = attachment_lines(app, g, &tasks)?;
    web |= LINK_RE.find_iter(&text).any(|m| openable(m.as_str()));
    let mut issues = HashSet::new();
    let mut issue_lines = vec![];
    for c in ISSUE_REF_RE.captures_iter(&text) {
        let Ok(id) = c[1].parse::<i64>() else { continue };
        let r = rf("issue", id);
        if issues.contains(&r) || issues.len() == SOURCE_ISSUES_MAX {
            continue;
        }
        let Some(b) = board::find_issue(app, Some(id))? else { continue };
        let mut line = format!("- {r} [{}] {}: {}", b.st("state"), b.st("kind"), one_line(&b.st("title"), 200));
        if let Some(d) = b.s("detail").filter(|d| !d.trim().is_empty()) {
            line.push_str(&format!("\n  {}", one_line(d, 600)));
        }
        issue_lines.push(line);
        issues.insert(r);
    }
    lines.extend(issue_lines);
    let block = (!lines.is_empty() || web).then(|| {
        let mut b = vec!["Sources the goal points at (attachments and the backlog issues it names):".to_string()];
        b.extend(lines);
        if web {
            b.push("Open a source's link with WebFetch to read it.".into());
        }
        b.join("\n")
    });
    Ok(Sources { block, issues, web })
}

/// The prompt, and the refs it shows (the only ones a finding may name).
fn prompt(app: &App, g: &Row, sha: Option<&str>, read: bool, src: &Sources) -> Result<(String, Known)> {
    let owner = &app.cfg.owner;
    let me = rf("goal", g.id());
    let project = g.st("project");
    let at = sha.map(|s| format!(" at commit {}", &s[..s.len().min(12)])).unwrap_or_default();
    let reading = if read {
        format!(
            "You're in the project's checkout{at}, read-only (Read, Grep and Glob). Work in two steps.\n\
             1. Check the plan against the checkout before you answer. For each task, list what it names or relies \
             on (files, functions, types, commands, config keys, docs, behaviour it extends), find each with Grep and \
             Glob and Read it, and see whether it's there and does what the plan says. Put each check in checked: \
             the task, what it assumes, and what you found with its file and line. Read what settles each \
             assumption, not the whole checkout: you have {MAX_TURNS} turns, so run several searches and reads at \
             once. An answer with nothing in checked hasn't done this step.\n\
             2. Report the gaps in findings. A task that rests on something the checkout doesn't have, or that works \
             otherwise than the plan says, so it can't deliver its part as planned, is an outcome_unproven finding: \
             quote the clause it leaves unbuilt, and give the file and what's there."
        )
    } else {
        "The project has no checkout on this Mac, so judge from what's below.".to_string()
    };
    let mut parts = vec![format!(
        "{owner}'s planner just planned goal {me} in {project}. Before its tasks run, review the plan against the \
         goal's outcome. {reading} You don't see the planner's conversation: judge the plan as written below.\n\n\
         Report only real gaps, each as one finding of one kind:\n\
         - outcome_unproven: a clause of the outcome (\"Done when\") that no task builds or proves. Quote the clause. \
         A clause each task proves for itself, by delivering it or by attaching its own results (a report, a \
         measurement, screenshots) the way the clause asks, is proven: don't ask for a separate task to prove it.\n\
         - owner_prereq: something only {owner} can do (a sign-in, an account, a key, a purchase, an approval, a \
         device) that a task needs, with no human task (kind human) planned for it ahead of that task. A step a task \
         or the goal notes record as already done (the key is made, the account exists, {owner} already signed in) \
         isn't one.\n\
         - past_decision: a task planned past a decision, spec, design or baseline that isn't settled yet (an \
         earlier task settles it, or nothing does), so it would likely be redone once it is.\n\
         - overlap: a task that does what one of the other open goals below does or plans to.\n\
         - lesson_ignored: a planning rule or kept lesson below that the plan goes against.\n\
         When the outcome, TLDR or notes point at a source the plan must match (a review, a doc, a design, a spec, \
         a list of issues), read that source in full: it's attached or linked under Sources below, named there as \
         backlog issues, written in the goal notes, or a file in the checkout. List every item it asks for (each \
         decision, fix, screen or issue), not only the ones the tasks name, and check each against the tasks. An \
         item no task covers, and the goal notes don't settle or set aside, is an outcome_unproven finding: name \
         the item and where it is in the source. A source you can't read (not attached, or a link or artifact that \
         won't open) is one outcome_unproven finding naming it and asking for its items where the review can read \
         them (attached, or in the goal notes); don't guess its items.\n\
         A finding's short is a glance, {SHORT_MAX} characters at most; its detail gives the evidence and what to \
         change in the plan. tasks lists this goal's task refs it's about; refs lists the other goals, backlog issues and \
         lessons it's about (only refs shown below). Don't flag what the goal notes already settle, don't restate the plan, and \
         don't suggest wording or style. A plan with nothing wrong gets an empty list."
    )];
    let mut goal = vec![format!("Goal {me}: {}", g.st("name"))];
    if let Some(t) = g.s("tldr").filter(|t| !t.trim().is_empty()) {
        goal.push(format!("TLDR: {}", t.trim()));
    }
    goal.push(match g.s("outcome").filter(|t| !t.trim().is_empty()) {
        Some(o) => format!("Done when: {}", o.trim()),
        None => "Done when: (the goal names no outcome)".into(),
    });
    let notes = board::goal_notes(app, g.id())?;
    if !notes.is_empty() {
        goal.push(format!("Goal notes:\n{}", crate::handoff::notes_block(&notes, 2500)));
    }
    let tasks = board::goal_tasks(app, g.id())?;
    goal.push(format!("Tasks, in order:\n{}", tasks.iter().map(task_line).collect::<Vec<_>>().join("\n")));
    parts.push(goal.join("\n"));
    let about = format!("{}\n{}\n{}", g.st("name"), g.st("outcome"), tasks.iter().map(|t| t.st("title")).collect::<Vec<_>>().join("\n"));
    let rules = planning::block(app, Some(&project), &about);
    parts.push(rules.clone());
    if let Some(b) = &src.block {
        parts.push(b.clone());
    }
    let (others, mut refs) = other_goals(app, g)?;
    refs.extend(src.issues.iter().cloned());
    if !others.is_empty() {
        parts.push(format!("The project's other open goals:\n{}", others.join("\n")));
    }
    for l in lessons::kept(app, Some(&project), &["planning"])? {
        let r = rf("lesson", l.id());
        if rules.contains(&format!("({r})")) {
            refs.insert(r);
        }
    }
    let known = Known { tasks: tasks.iter().map(|t| rf("task", t.id())).collect(), refs };
    Ok((parts.join("\n\n"), known))
}

fn refs_in(v: &Value, known: &HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for r in v.as_array().into_iter().flatten().filter_map(Value::as_str) {
        let r = r.trim().to_uppercase();
        if known.contains(&r) && !out.contains(&r) && out.len() < REFS_MAX {
            out.push(r);
        }
    }
    out
}

/// Claude's findings, kept to known kinds and refs, each part clipped. Err when the answer isn't one.
pub fn checked(v: &Value, known: &Known) -> std::result::Result<Value, String> {
    let Some(items) = v["findings"].as_array() else {
        return Err("claude's answer had no findings".into());
    };
    let mut findings = vec![];
    for f in items {
        let Some(kind) = f["kind"].as_str().filter(|k| KINDS.contains(k)) else { continue };
        let short = one_line(f["short"].as_str().unwrap_or(""), SHORT_MAX);
        if short.is_empty() {
            continue;
        }
        findings.push(json!({
            "kind": kind, "short": short, "detail": clip(f["detail"].as_str().unwrap_or("").trim(), DETAIL_MAX),
            "tasks": refs_in(&f["tasks"], &known.tasks), "refs": refs_in(&f["refs"], &known.refs),
        }));
        if findings.len() == FINDINGS_MAX {
            break;
        }
    }
    Ok(json!({"findings": findings}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn findings_keep_known_kinds_and_refs_only() {
        let known = Known {
            tasks: ["T1".to_string(), "T2".to_string()].into_iter().collect(),
            refs: ["G7".to_string(), "L3".to_string()].into_iter().collect(),
        };
        let v = json!({"findings": [
            {"kind": "overlap", "short": "  Same   screen as G7 ", "detail": "x", "tasks": ["t1", "T9", "T1"], "refs": ["G7", "G8", "L3"]},
            {"kind": "style", "short": "Rename it", "detail": ""},
            {"kind": "owner_prereq", "short": "", "detail": "no short"},
        ]});
        let out = checked(&v, &known).unwrap();
        assert_eq!(out["findings"].as_array().unwrap().len(), 1);
        let f = &out["findings"][0];
        assert_eq!(f["short"], "Same screen as G7");
        assert_eq!(f["tasks"], json!(["T1"]));
        assert_eq!(f["refs"], json!(["G7", "L3"]));
        assert!(checked(&json!({"nope": 1}), &known).is_err());
    }
}
