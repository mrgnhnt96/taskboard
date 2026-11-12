//! Human tasks (`tb task new --human`, shown as H<n>): work only the owner can do, such as making an
//! account or signing an agreement. Its detail holds the steps and links. No terminal ever starts one:
//! once nothing holds it back, the runner puts it in Needs you and raises an alert, and it's done when
//! the owner presses Done in the app (with an optional note), or tells an agent it's done (`tb needs done`). Tasks that wait for it, by `waits_for` or
//! by their goal's order, then go on, and a parked one is told what the owner did.
//!
//! The owner can answer with something other than Done (`answer`): Can't do this hands it back, Not
//! needed cancels it, Don't wait for me frees what waits while it stays listed, and Waiting on someone
//! puts it off until a date.

use serde_json::{json, Value};

use crate::app::App;
use crate::board::{self, OWNER};
use crate::util::*;
use crate::dispatch::Later;
use crate::{dig, dispatch, fields, home, hours, midna, p, reports, waitsfor};

pub const KIND: &str = "human";

/// The `needs_reason` of a human task that's ready for the owner.
pub const NEEDS: &str = "human";

/// How the owner answered other than with Done (`tasks.owner_answer`). The first two finish the task.
pub const CANT_DO: &str = "cant_do";
pub const NOT_NEEDED: &str = "not_needed";
pub const DONT_WAIT: &str = "dont_wait";

/// The `answer` of `POST /tasks/:id/owner-answer` that keeps what waits waiting and records nothing on the task.
const WAITING: &str = "waiting";

pub fn is(t: &Row) -> bool {
    t.s("kind") == Some(KIND)
}

/// The owner answered it other than with Done.
pub fn answered(t: &Row) -> bool {
    is(t) && t.s("owner_answer").is_some()
}

/// Still open, but the owner said not to wait for it: it holds nothing back, neither the tasks that
/// wait for it nor its goal.
pub fn released(t: &Row) -> bool {
    is(t) && t.s("owner_answer") == Some(DONT_WAIT) && t.s("status") != Some("done")
}

/// The task's ref as people see it: H<n> for a human task, else T<n>. T<n> still finds either.
pub fn task_ref(t: &Row) -> String {
    if is(t) {
        format!("H{}", t.id())
    } else {
        rf("task", t.id())
    }
}

/// What a new human task must have: the steps to follow, since nobody else can do them.
pub fn refuse_bare(body: &Value) -> Result<()> {
    if body_str(body, "detail").trim().is_empty() {
        return err(400, "A human task needs its steps in --detail: what to do, in order, with the links to open.");
    }
    if is_release_cut(&body_str(body, "title"), &body_str(body, "detail")) {
        return err(
            400,
            "A release cut is a command an agent runs (tb release cut v<version>), so it's never a human task: \
             make it an agent task (tb task new \"Cut v<version>\" --no-pr --detail \"…\") and have the task that needs it run tb wait-for T<n> on it.",
        );
    }
    for k in ["stack_on", "same_pr", "locks", "alone"] {
        if body_has(body, k) && !body[k].is_null() {
            return err(400, format!("A human task has no terminal, so it takes no {}.", k.replace('_', "-")));
        }
    }
    Ok(())
}

/// The words of `s`, lowercased, with apostrophes kept (curly ones made straight) so "don't" stays one word.
fn words(s: &str) -> Vec<String> {
    s.to_lowercase().replace('’', "'").split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).map(|w| w.to_string()).collect()
}

fn is_release_word(w: &str) -> bool {
    w == "release" || w == "releases"
}

/// A task that is a release cut: its detail runs `tb release cut`, or its title names a cut and a release
/// ("Cut v1.4", "Cut the release"). An agent runs that command, so it's an agent task, never a human one.
pub fn is_release_cut(title: &str, detail: &str) -> bool {
    let version = title.split_whitespace().any(|t| {
        let t = t.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        t.strip_prefix('v').is_some_and(|n| n.contains('.') && n.chars().all(|c| c.is_ascii_digit() || c == '.'))
    });
    let title = words(title);
    let cut = title.iter().any(|w| w == "cut");
    (cut && (version || title.iter().any(|w| is_release_word(w)))) || words(detail).windows(3).any(|w| w[0] == "tb" && w[1] == "release" && w[2] == "cut")
}

/// A sentence of a task's detail that tells its agent not to cut a release (or not to release) itself,
/// like "Don't cut a release yourself": work an agent can do belongs in a task of its own.
pub fn forbids_release(detail: &str) -> bool {
    detail.split(['.', '!', '?', ';', '\n']).any(|sentence| {
        let w = words(sentence);
        let Some(neg) = w.iter().position(|w| matches!(w.as_str(), "don't" | "dont" | "never" | "not" | "shouldn't" | "mustn't" | "won't")) else {
            return false;
        };
        let after = &w[neg..];
        after.iter().any(|w| is_release_word(w) || w == "releasing") && after.iter().any(|w| matches!(w.as_str(), "cut" | "cutting" | "yourself" | "itself"))
    })
}

/// The refusal for anything that would start a terminal on a human task.
pub fn refuse_terminal(app: &App, t: &Row) -> Result<()> {
    if is(t) {
        return err(409, format!("{} is a human task: {} does it and presses Done in the app; no terminal starts on it.", task_ref(t), app.cfg.owner));
    }
    Ok(())
}

/// The refusal for an agent's `tb submit` or `tb fail` (or the API's done and fail) on a human task.
pub fn refuse_agent_finish(app: &App, t: &Row) -> Result<()> {
    if is(t) {
        return err(409, format!("{} is a human task: only {} marks it done, with Done in the app.", task_ref(t), app.cfg.owner));
    }
    Ok(())
}

/// The runner's turn on a queued human task nothing holds back: it waits for the owner now.
pub fn ready(app: &App, t: &Row) -> Result<()> {
    let r = task_ref(t);
    let ask = format!("Your task: {}. Follow its steps, then press Done.", t.st("title"));
    board::update_task(app, t.id(), fields!["status" => "needs", "needs_reason" => NEEDS, "question" => ask.clone(), "answered_at" => null, "latest" => ask, "owner_answer" => null])?;
    board::log_event(app, t.id(), "Board", "status", "Ready for you")?;
    dispatch::add_alert_keyed(app, &format!("{r}: {}", t.st("title")), Some(t.id()), t.i("goal_id"), &alert_key(t.id()))
}

fn alert_key(id: i64) -> String {
    format!("human-{id}")
}

/// The owner pressed Done (only from the app): the task is finished with their note, if any.
pub fn done(app: &App, id: i64, body: &Value) -> Result<()> {
    app.db.tx(|| {
        let t = open_human(app, id)?;
        mark_done(app, &t, &owner_note(body), "Done by you")
    })
}

/// A human task that isn't done yet, or the refusal.
fn open_human(app: &App, id: i64) -> Result<Row> {
    let t = board::get_task(app, id)?;
    if !is(&t) {
        return err(409, format!("{} isn't a human task.", rf("task", id)));
    }
    if t.s("status") == Some("done") {
        return err(409, format!("{} is already done.", task_ref(&t)));
    }
    Ok(t)
}

fn mark_done(app: &App, t: &Row, note: &str, by: &str) -> Result<()> {
    keep_note(app, t, note)?;
    board::update_task(app, t.id(), fields!["owner_answer" => null])?;
    finish(app, t, &if note.is_empty() { by.to_string() } else { format!("{by}: {note}") })
}

/// `POST /humans/done` (`tb needs done H3 H4`): the owner's Done on human tasks `tasks`, told to the agent
/// in terminal `via_session`, with an optional `note` for each. `word` is the owner's word there that a task
/// is done (`startword::owners_done_word`). Every task must be an open human task with that word, or none
/// is marked done; each that is goes as Done in the app does.
pub fn done_on_word(app: &App, body: &Value, word: impl Fn(&str, i64) -> Result<Option<String>>) -> Result<Vec<String>> {
    let via = body_str(body, "via_session");
    if via.is_empty() {
        return err(403, format!("Only {} marks a human task done: from the app, or by telling an agent in a terminal.", app.cfg.owner));
    }
    let ids = waitsfor::ref_ids(&body["tasks"])?;
    if ids.is_empty() {
        return err(400, "Name the human tasks that are done: tb needs done H3 H4.");
    }
    let note = owner_note(body);
    let by = format!("Done by you via {}", board::session_name(app, Some(&via), None));
    app.db.tx(|| {
        let tasks = ids.iter().map(|&id| open_human(app, id)).collect::<Result<Vec<_>>>()?;
        let unsaid: Vec<String> = tasks.iter().filter_map(|t| word(&via, t.id()).map(|w| w.is_none().then(|| task_ref(t))).transpose()).collect::<Result<_>>()?;
        if !unsaid.is_empty() {
            let list = unsaid.join(", ");
            return err(
                403,
                format!(
                    "Only {owner} marks a human task done: nobody in this terminal's conversation said {list} {is} done, so none was marked. \
                     {owner} can press Done in the app, or say so here (\"mark {list} done\"); tell them that once.",
                    owner = app.cfg.owner,
                    is = if unsaid.len() == 1 { "is" } else { "are" },
                ),
            );
        }
        for t in &tasks {
            mark_done(app, t, &note, &by)?;
        }
        Ok(tasks.iter().map(task_ref).collect())
    })
}

fn owner_note(body: &Value) -> String {
    clip(body_str(body, "note").trim(), 2000)
}

fn keep_note(app: &App, t: &Row, note: &str) -> Result<()> {
    if note.is_empty() {
        return Ok(());
    }
    let mut ctx = board::task_context(t);
    ctx.insert("owner_note".into(), json!(note));
    board::save_context(app, t.id(), &ctx, false)
}

fn finish(app: &App, t: &Row, summary: &str) -> Result<()> {
    reports::finish_task(app, t, OWNER, summary, false, None)?;
    dispatch::clear_alert_key(app, &alert_key(t.id()))?;
    dispatch::clear_alerts(app, Some(t.id()), None)
}

/// `POST /tasks/:id/owner-answer`: the owner's answer on a to-do of theirs other than Done, with an
/// optional `note`: `cant_do`, `not_needed`, `dont_wait`, or `waiting` with `until`, a date. Unlike
/// Done, `tb needs` may send these on the owner's word.
pub fn answer(app: &App, id: i64, body: &Value) -> Result<()> {
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        let r = task_ref(&t);
        if !is(&t) {
            return err(409, format!("{r} isn't a human task."));
        }
        if t.s("status") != Some("needs") {
            return err(409, format!("{r} isn't waiting on {} now.", app.cfg.owner));
        }
        let note = owner_note(body);
        match body_str(body, "answer").as_str() {
            CANT_DO => close(app, &t, CANT_DO, "Handed back", &note),
            NOT_NEEDED => close(app, &t, NOT_NEEDED, "Not needed", &note),
            DONT_WAIT => dont_wait(app, &t, &note),
            WAITING => put_off(app, &t, &body_str(body, "until"), &note),
            _ => err(400, "Answer with cant_do, not_needed, dont_wait or waiting."),
        }
    })?;
    app.wake_runner();
    Ok(())
}

fn close(app: &App, t: &Row, answer: &str, what: &str, note: &str) -> Result<()> {
    keep_note(app, t, note)?;
    board::update_task(app, t.id(), fields!["owner_answer" => answer])?;
    finish(app, t, &if note.is_empty() { what.to_string() } else { format!("{what}: {note}") })
}

fn dont_wait(app: &App, t: &Row, note: &str) -> Result<()> {
    if released(t) {
        return err(409, format!("Nothing waits for {} already.", task_ref(t)));
    }
    let goal = t.i("goal_id");
    let goal_was_finished = goal.map(|g| board::goal_finished(app, g)).transpose()?.unwrap_or(true);
    keep_note(app, t, note)?;
    board::update_task(app, t.id(), fields!["owner_answer" => DONT_WAIT])?;
    board::log_event(app, t.id(), OWNER, "status", "Don't wait for this: it stays on the list and holds nothing back")?;
    if let (Some(g), false) = (goal, goal_was_finished) {
        if board::goal_finished(app, g)? {
            board::on_goal_finished(app, g, None)?;
        }
    }
    Ok(())
}

/// Waiting on someone: off the list until the work hours of `date`. What waits for it keeps waiting.
fn put_off(app: &App, t: &Row, date: &str, on: &str) -> Result<()> {
    let Ok(day) = chrono::NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d") else {
        return err(400, "Say when to bring it back, as a date: until 2026-11-12.");
    };
    let until = hours::day_start(app, day);
    if until <= now_ts() {
        return err(400, "Pick a date that's still to come.");
    }
    let later = Later::WaitingOn(until, on.to_string());
    let ids: Vec<String> = dispatch::alerts(app).iter().filter(|a| a["task_id"].as_i64() == Some(t.id())).filter_map(|a| a["id"].as_str().map(str::to_string)).collect();
    if ids.is_empty() {
        home::put_away(app, &format!("{}{}", home::TASK_ID, rf("task", t.id())), Some(later.clone()))?;
    }
    for id in &ids {
        dispatch::snooze_to(app, id, &later)?;
        dispatch::withdraw(app, id);
    }
    let who = if on.is_empty() { "someone".to_string() } else { on.to_string() };
    board::log_event(app, t.id(), OWNER, "status", &format!("Waiting on {who} until {}", day.format("%b %-d")))?;
    Ok(())
}

/// `POST /tasks/:id/talk`: a Claude terminal to ask about a human task in, opened in its project's
/// checkout (the home folder when it has none) with the task's steps in its context. Claude helps the
/// owner through them; it never does them or marks the task done. Refused for a task that isn't a
/// human one, one that's done, or while Midna is down.
pub fn talk(app: &App, id: i64) -> Result<Value> {
    let t = board::get_task(app, id)?;
    let r = task_ref(&t);
    if !is(&t) {
        return err(409, format!("{r} isn't a human task."));
    }
    if t.s("status") == Some("done") {
        return err(409, format!("{r} is done already."));
    }
    if !midna::up(app) {
        return err(409, "Midna isn't running, so the board can't open a terminal.");
    }
    let cwd = match dig::checkout(app, &t.st("project"))? {
        Some(p) => p,
        None => dirs::home_dir().unwrap_or_default(),
    };
    let args = json!({"cwd": cwd, "title": short(&format!("Ask: {}", t.st("title")), 40), "prompt": "", "draft": "",
                      "context": talk_context(app, &t)?, "focus": true, "queue": false});
    let j = app.db.tx(|| board::create_job(app, "agent", args, None, "human_talk", Some(json!({"task": id}))))?;
    Ok(json!({"ok": true, "job": rf("job", j)}))
}

/// `POST /alerts/:id/talk`: a Claude terminal to talk an alert over in, in its project's checkout (the
/// home folder when it has none). Claude explains it and what can be done; nothing here settles the alert.
/// Refused for an alert that's gone, or while Midna is down.
pub fn talk_alert(app: &App, id: &str) -> Result<Value> {
    let Some(a) = dispatch::listing(app).into_iter().find(|a| a["id"] == id) else {
        return err(404, "There's no alert with that id.");
    };
    if !midna::up(app) {
        return err(409, "Midna isn't running, so the board can't open a terminal.");
    }
    let project = alert_project(app, &a)?;
    let cwd = match project.as_deref().map(|p| dig::checkout(app, p)).transpose()?.flatten() {
        Some(p) => p,
        None => dirs::home_dir().unwrap_or_default(),
    };
    let text = a["title"].as_str().filter(|t| !t.is_empty()).or(a["text"].as_str()).unwrap_or("");
    let args = json!({"cwd": cwd, "title": short(&format!("Ask: {text}"), 40), "prompt": "", "draft": "",
                      "context": alert_context(app, &a, project.as_deref()), "focus": true, "queue": false});
    let j = app.db.tx(|| board::create_job(app, "agent", args, None, "alert_talk", Some(json!({"alert": id}))))?;
    Ok(json!({"ok": true, "job": rf("job", j)}))
}

fn alert_project(app: &App, a: &Value) -> Result<Option<String>> {
    if let Some(p) = a["project"].as_str().filter(|p| !p.is_empty()) {
        return Ok(Some(p.to_string()));
    }
    let of_task = a["task"].as_str().and_then(home::number).and_then(|id| board::get_task(app, id).ok()).map(|t| t.st("project"));
    let of_goal = a["goal"].as_str().and_then(home::number).map(|g| app.db.val("SELECT project FROM goals WHERE id = ?", p![g])).transpose()?;
    Ok(of_task.or(of_goal.and_then(|p| p.as_str().map(str::to_string))).filter(|p| !p.is_empty()))
}

fn alert_context(app: &App, a: &Value, project: Option<&str>) -> String {
    let owner = &app.cfg.owner;
    let mut parts = vec![format!(
        "{owner} opened this terminal from the task board to talk over an alert. Explain what it means and what can be done \
         about it, looking in the code or on the board as you need. Don't change code. Wait for {owner}; don't start on \
         anything by yourself. You aren't on a task: don't take one."
    )];
    let mut alert = format!("The alert: {}", a["text"].as_str().unwrap_or("").trim());
    if let Some(d) = a["detail"].as_str().filter(|d| !d.is_empty()) {
        alert.push_str(&format!("\n{d}"));
    }
    if let Some(p) = project {
        alert.push_str(&format!("\nProject: {p}"));
    }
    parts.push(alert);
    if let Some(t) = a["task"].as_str() {
        parts.push(format!("For its task as it stands now: tb task show {t}"));
    }
    if let Some(g) = a["goal"].as_str() {
        parts.push(format!("For its goal: tb goal show {g}"));
    }
    parts.join("\n\n")
}

fn talk_context(app: &App, t: &Row) -> Result<String> {
    let owner = &app.cfg.owner;
    let r = task_ref(t);
    let mut parts = vec![format!(
        "{owner} opened this terminal from the task board to ask about {r}, a human task: something only they can do, \
         following its steps. Help them with it: explain a step, find what a step needs (in the code, the board or on \
         the web), or draft text they'll paste. Don't do the steps for them and don't change code. Wait for {owner}; \
         don't start on anything by yourself. You aren't on a task: don't take one. You can't mark {r} done; {owner} \
         presses Done in the app when it is."
    )];
    let mut task = format!("{r}: {}", t.st("title"));
    if !t.st("project").is_empty() {
        task.push_str(&format!(" (project {})", t.st("project")));
    }
    task.push_str(&format!("\nIts steps:\n{}", t.st("detail").trim()));
    parts.push(task);
    let waiting: Vec<String> =
        waitsfor::waiting_on(app, t)?.iter().map(|o| format!("- {} {}", task_ref(o), short(&o.st("title"), 80))).collect();
    if !waiting.is_empty() {
        parts.push(format!("Waiting for it to be done:\n{}", waiting.join("\n")));
    }
    parts.push(format!("For all of it as it stands now: tb task show {r}"));
    Ok(parts.join("\n\n"))
}

/// What the owner said when they pressed Done.
pub fn note(t: &Row) -> Option<String> {
    board::task_context(t).get("owner_note").and_then(|v| v.as_str()).map(|s| s.to_string()).filter(|s| !s.is_empty())
}

/// The resume prompt's line for a human task this one waited for.
pub fn done_line(app: &App, t: &Row) -> String {
    let (owner, name) = (&app.cfg.owner, format!("{} “{}”", task_ref(t), short(&t.st("title"), 80)));
    let mut line = match t.s("owner_answer") {
        Some(CANT_DO) => format!("- {name} wasn't done: {owner} can't do it, so find another way, or say what's missing with a question"),
        Some(NOT_NEEDED) => format!("- {name} was cancelled: {owner} says it isn't needed"),
        Some(DONT_WAIT) if released(t) => format!("- {name} isn't done yet: {owner} said not to wait for it, so carry on without it"),
        _ => format!("- {name} is done: {owner} did it"),
    };
    match note(t) {
        Some(n) => line += &format!(" and noted: {n}"),
        None => line += ".",
    }
    line
}

/// The handoff's lines for the human tasks the owner finished in this task's goal, besides the ones
/// it waits for (`waitsfor::bring_in_text` names those).
pub fn goal_text(app: &App, t: &Row) -> Result<Option<String>> {
    let Some(gid) = t.i("goal_id") else { return Ok(None) };
    let deps = crate::waitsfor::deps(t);
    let lines: Vec<(String, bool)> = board::goal_tasks(app, gid)?
        .iter()
        .filter(|o| is(o) && waitsfor::ready(Some(o)) && !deps.contains(&o.id()))
        .map(|o| (done_line(app, o), answered(o)))
        .collect();
    if lines.is_empty() {
        return Ok(None);
    }
    let owner = &app.cfg.owner;
    let head = if lines.iter().any(|(_, a)| *a) { format!("What {owner} did or said of their part of this goal:") } else { format!("Done by {owner} in this goal, which this task may build on:") };
    Ok(Some(format!("{head}\n{}", lines.into_iter().map(|(l, _)| l).collect::<Vec<_>>().join("\n"))))
}
