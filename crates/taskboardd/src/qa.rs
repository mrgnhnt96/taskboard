//! QA comments (optional, off until the owner switches it on in Settings ▸ QA): Jira comments that
//! testers leave on board tickets. The board reads new ones from Jira on its own (`poll`), or takes
//! them from anything that forwards them (`POST /jira/comment`), and asks Claude (no tools) what each
//! one wants:
//! - `task`: a clear direction for work on the ticket or its goal → a high-priority follow-up task on
//!   the same ticket and goal (a later comment adds to it while it's open);
//! - `flag`: a question or a decision with no obvious task → an alert and the goal's QA tab, until the
//!   owner says `tb qa task Q3` or `tb qa ignore Q3` (through Claude);
//! - `none`: nothing to do (a pass, a status note, thanks).

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, dispatch, fields, jira, ops, p, proc};

const ON_KEY: &str = "qa_on";
const SINCE_KEY: &str = "qa_since";
const CHECKED_KEY: &str = "qa_checked_at";
/// How often the board asks Jira for new comments.
const POLL_SECS: f64 = 5.0 * 60.0;
/// Tickets of tasks finished within this long are watched.
const WATCH_DAYS: f64 = 30.0;
const RETRY_SECS: f64 = 5.0 * 60.0;
const TRIES: i64 = 3;
const VERDICTS: [&str; 3] = ["task", "flag", "none"];

/// QA comments are switched on and Jira is set up.
pub fn on(app: &App) -> bool {
    app.cfg.jira_on() && app.db.get_setting(ON_KEY).ok().flatten().as_deref() == Some("1")
}

/// `GET /qa`: the Settings ▸ QA state.
pub fn settings(app: &App) -> Result<Value> {
    Ok(json!({
        "on": app.db.get_setting(ON_KEY)?.as_deref() == Some("1"),
        "jira": app.cfg.jira_on(),
        "since": app.db.get_setting(SINCE_KEY)?,
        "checked_at": app.db.get_setting(CHECKED_KEY)?,
        "comments": app.db.count("SELECT COUNT(*) FROM qa_comments", p![])?,
        "waiting": app.db.count("SELECT COUNT(*) FROM qa_comments WHERE verdict = 'flag' AND handled_at IS NULL", p![])?,
    }))
}

/// `POST /qa {on}`. Switching it on starts from now: comments left before then are never read.
pub fn set_settings(app: &App, body: &Value) -> Result<Value> {
    if let Some(v) = body.get("on").filter(|v| !v.is_null()) {
        let want = as_bool(Some(v), false);
        if want && !app.cfg.jira_on() {
            return err(409, "Set up Jira in the board's config first: QA comments are read from Jira.");
        }
        let was = app.db.get_setting(ON_KEY)?.as_deref() == Some("1");
        if want && !was {
            app.db.set_setting(SINCE_KEY, Some(&now_iso()))?;
            app.db.set_setting(CHECKED_KEY, None)?;
        }
        app.db.set_setting(ON_KEY, Some(if want { "1" } else { "0" }))?;
        if !want {
            for a in dispatch::alerts(app) {
                if a["key"].as_str().map(|k| k.starts_with("qa:")).unwrap_or(false) {
                    dispatch::clear_alert_key(app, a["key"].as_str().unwrap_or(""))?;
                }
            }
        }
    }
    settings(app)
}

pub fn get(app: &App, id: i64) -> Result<Row> {
    app.db.q1("SELECT * FROM qa_comments WHERE id = ?", p![id])?.ok_or_else(|| ApiError::new(404, format!("There's no QA comment {}.", rf("qa", id))))
}

pub fn comment_url(app: &App, key: &str, comment_id: &str) -> String {
    format!("https://{}/browse/{key}?focusedCommentId={comment_id}", app.cfg.jira.site.trim())
}

fn alert_key(id: i64) -> String {
    format!("qa:{id}")
}

/// The board task whose ticket this is: the done one with a PR first, never a QA follow-up.
fn source_task(app: &App, key: &str) -> Result<Option<Row>> {
    app.db.q1(
        "SELECT * FROM tasks WHERE jira_key = ? AND id NOT IN (SELECT task_id FROM qa_comments WHERE task_id IS NOT NULL) \
         ORDER BY status = 'done' DESC, pr_num IS NOT NULL DESC, id DESC",
        p![key],
    )
}

fn goal_of(app: &App, t: &Row) -> Result<Option<Row>> {
    Ok(board::find_goal(app, t.i("goal_id"))?.filter(|g| !g.b("archived")))
}

/// A Jira comment's author is the owner (the board posts as them too): never read.
fn is_owner(app: &App, author: &Value) -> bool {
    let j = &app.cfg.jira;
    let id = author["accountId"].as_str().unwrap_or("");
    let email = author["emailAddress"].as_str().unwrap_or("");
    (!id.is_empty() && id == j.assignee_account_id) || (!email.is_empty() && email.eq_ignore_ascii_case(&j.email))
}

/// `POST /jira/comment {key, comment_id, author?, text?}`, and each new comment `poll` finds.
pub fn intake(app: &App, ev: &Value) -> Result<Value> {
    if !on(app) {
        return Ok(json!({"ignored": "QA comments are off"}));
    }
    let key = body_str(ev, "key").trim().to_uppercase();
    let comment_id = body_str(ev, "comment_id").trim().to_string();
    if ops::jira_key(&key).is_err() {
        return err(400, "Give the Jira key, like PROJ-123.");
    }
    if comment_id.is_empty() || !comment_id.chars().all(|c| c.is_ascii_digit()) {
        return err(400, "Give the Jira comment's id.");
    }
    let made = app.db.tx(|| {
        if let Some(have) = app.db.q1("SELECT id FROM qa_comments WHERE comment_id = ?", p![comment_id])? {
            return Ok(Err(json!({"comment": rf("qa", have.id()), "new": false})));
        }
        let Some(t) = source_task(app, &key)? else { return Ok(Err(json!({"ignored": format!("{key} isn't on the board")}))) };
        let text = clip(&body_str(ev, "text"), 3000);
        let id = app.db.insert(
            "qa_comments",
            fields!["jira_key" => key, "comment_id" => comment_id, "source_task_id" => t.id(), "tries" => 0,
                    "author" => one_line(&body_str(ev, "author"), 120), "text" => if text.is_empty() { None } else { Some(text) },
                    "created_at" => now_iso(), "updated_at" => now_iso()],
        )?;
        Ok(Ok(id))
    })?;
    let id = match made {
        Ok(id) => id,
        Err(v) => return Ok(v),
    };
    app.info(format!("qa: comment {comment_id} on {key}, reading it"));
    read_later(app, id);
    Ok(json!({"comment": rf("qa", id), "new": true}))
}

fn read_later(app: &App, id: i64) {
    {
        let mut sh = app.shared.lock();
        if !sh.qa_reading.insert(id) {
            return;
        }
    }
    app.queue_ai(Box::new(move |a: &App| {
        let r = read(a, id);
        a.shared.lock().qa_reading.remove(&id);
        if let Err(e) = r {
            a.info(format!("qa: reading {} failed: {e}", rf("qa", id)));
        }
        a.wake_runner();
    }));
}

/// Read one comment (from Jira when its text didn't come with it), ask Claude, act on the answer.
fn read(app: &App, id: i64) -> Result<()> {
    let mut c = get(app, id)?;
    if c.s("verdict").is_some() {
        return Ok(());
    }
    if c.s("text").map(|t| t.trim().is_empty()).unwrap_or(true) {
        match jira::read_comment(app, &c.st("jira_key"), &c.st("comment_id")) {
            Ok(d) => {
                let text = clip(d["text"].as_str().unwrap_or(""), 3000);
                let author = one_line(d["author"]["displayName"].as_str().unwrap_or(""), 120);
                if is_owner(app, &d["author"]) {
                    return app.db.tx(|| decide(app, &c, &json!({"verdict": "none", "mine": true})));
                }
                app.db.x("UPDATE qa_comments SET text = ?, author = COALESCE(NULLIF(author, ''), ?) WHERE id = ?", p![text, author, id])?;
                c = get(app, id)?;
            }
            Err(e) => return app.db.tx(|| failed(app, &c, &e)),
        }
    }
    let t = board::get_task(app, c.i0("source_task_id"))?;
    let g = goal_of(app, &t)?;
    match ask(app, &prompt_for(app, &c, &t, g.as_ref())) {
        Ok(out) => app.db.tx(|| decide(app, &get(app, id)?, &out)),
        Err(e) => app.db.tx(|| failed(app, &get(app, id)?, &e)),
    }
}

fn prompt_for(app: &App, c: &Row, t: &Row, g: Option<&Row>) -> String {
    let key = c.st("jira_key");
    let scope = if g.is_some() { "this ticket or its goal" } else { "this ticket" };
    let lines = vec![
        format!("A tester left a comment on the Jira ticket {key}. Decide what the developer should do about it."),
        format!(
            "The ticket's work was done in the board task “{}”{}{}",
            t.st("title"),
            if t.i("pr_num").is_some() { format!(" and its PR #{}.", t.i0("pr_num")) } else { ".".into() },
            g.map(|g| format!(" It belongs to the goal “{}”: {}.", g.st("name"), g.st("outcome").trim_end_matches('.'))).unwrap_or_default()
        ),
        format!("Comments written by {} are the developer's own: answer verdict none.", app.cfg.owner),
        format!(
            "verdict task: the comment gives a clear direction for work on {scope}, such as a code change, a new or changed test, \
             or a bug in this ticket's work, and it is plain what to do. Set pr=true when that work changes code or tests."
        ),
        format!("verdict flag: it asks the developer something (a question, a decision, a request) but there is no obvious task, or what it asks for is outside {scope}."),
        "verdict none: it needs no response, such as a pass, a status note, thanks, or things left untested for another reason.".into(),
        "title: a few plain words on what to do (empty for none). ask: the direction or question in one or two plain sentences (empty for none).".into(),
        String::new(),
        format!("The comment, by {}:", c.s("author").filter(|a| !a.is_empty()).unwrap_or("a tester")),
        c.st("text"),
    ];
    lines.join("\n")
}

fn schema() -> Value {
    json!({"type": "object", "required": ["verdict", "title", "ask"], "properties": {
        "verdict": {"type": "string", "enum": VERDICTS}, "pr": {"type": "boolean"},
        "title": {"type": "string"}, "ask": {"type": "string"}}})
}

fn ask(app: &App, prompt: &str) -> std::result::Result<Value, String> {
    let claude = proc::which(&app.cfg.claude).ok_or("claude isn't installed")?;
    let q = &app.cfg.questions;
    let args: Vec<String> = vec![
        "-p".into(),
        prompt.to_string(),
        "--model".into(),
        q.model.clone(),
        "--setting-sources".into(),
        "".into(),
        "--no-session-persistence".into(),
        "--tools".into(),
        "".into(),
        "--strict-mcp-config".into(),
        "--max-budget-usd".into(),
        q.budget_usd.clone(),
        "--json-schema".into(),
        schema().to_string(),
        "--output-format".into(),
        "json".into(),
    ];
    let out = proc::run(&claude, &args, Some(&app.cfg.data), q.timeout_secs as f64).map_err(|_| "claude didn't answer in time".to_string())?;
    let d: Value = serde_json::from_str(out.stdout.trim()).map_err(|_| "claude's answer wasn't JSON".to_string())?;
    let v = d
        .get("structured_output")
        .cloned()
        .filter(Value::is_object)
        .or_else(|| d.get("result").and_then(|r| r.as_str()).and_then(|r| serde_json::from_str(r).ok()))
        .ok_or("claude's answer had no result")?;
    if !VERDICTS.contains(&v["verdict"].as_str().unwrap_or("")) {
        return Err("claude's answer had no verdict".into());
    }
    Ok(v)
}

fn failed(app: &App, c: &Row, why: &str) -> Result<()> {
    let tries = c.i0("tries") + 1;
    app.db.x("UPDATE qa_comments SET tries = ?, error = ?, updated_at = ? WHERE id = ?", p![tries, clip(why, 300), now_iso(), c.id()])?;
    if tries >= TRIES {
        app.db.x(
            "UPDATE qa_comments SET verdict = 'flag', ask = ?, decided_at = ? WHERE id = ?",
            p![format!("Couldn't read it after {TRIES} tries ({why})"), now_iso(), c.id()],
        )?;
        flag(app, &get(app, c.id())?)?;
    }
    Ok(())
}

/// Store Claude's answer and act on it.
pub fn decide(app: &App, c: &Row, out: &Value) -> Result<()> {
    let verdict = if out["mine"] == true { "none" } else { out["verdict"].as_str().unwrap_or("none") };
    app.db.x(
        "UPDATE qa_comments SET verdict = ?, pr = ?, title = ?, ask = ?, error = NULL, decided_at = ?, updated_at = ? WHERE id = ?",
        p![verdict, (out["pr"] == true) as i64, one_line(out["title"].as_str().unwrap_or(""), 120), clip(out["ask"].as_str().unwrap_or(""), 1000), now_iso(), now_iso(), c.id()],
    )?;
    let c = get(app, c.id())?;
    match verdict {
        "task" => {
            act(app, &c, &board::get_task(app, c.i0("source_task_id"))?, board::BOARD, None)?;
        }
        "flag" => flag(app, &c)?,
        _ => {}
    }
    app.info(format!("qa: comment {} on {} is {verdict}", c.st("comment_id"), c.st("jira_key")));
    Ok(())
}

fn flag(app: &App, c: &Row) -> Result<()> {
    let g = goal_of(app, &board::get_task(app, c.i0("source_task_id"))?)?;
    let what = c.s("ask").filter(|a| !a.is_empty()).or(c.s("title").filter(|a| !a.is_empty())).unwrap_or("see the comment");
    dispatch::add_alert_keyed(
        app,
        &format!("{} · {} on {}: {}", rf("qa", c.id()), c.s("author").filter(|a| !a.is_empty()).unwrap_or("QA"), c.st("jira_key"), one_line(what, 300)),
        None,
        g.map(|g| g.id()),
        &alert_key(c.id()),
    )
}

/// The alert goes once the comment is handled (or QA is switched off).
pub fn alert_resolved(app: &App, a: &Value) -> Result<bool> {
    if !on(app) {
        return Ok(true);
    }
    let id = a["key"].as_str().and_then(|k| k.strip_prefix("qa:")).and_then(|n| n.parse::<i64>().ok());
    let Some(c) = id.map(|id| app.db.q1("SELECT * FROM qa_comments WHERE id = ?", p![id])).transpose()?.flatten() else { return Ok(true) };
    Ok(c.s("handled_at").is_some() || c.s("verdict") != Some("flag"))
}

fn open_follow_up(app: &App, key: &str) -> Result<Option<Row>> {
    for r in app.db.q("SELECT task_id FROM qa_comments WHERE jira_key = ? AND task_id IS NOT NULL ORDER BY id DESC", p![key])? {
        if let Some(t) = board::find_task(app, r.i("task_id"))? {
            if t.s("status") != Some("done") {
                return Ok(Some(t));
            }
        }
    }
    Ok(None)
}

fn author(c: &Row) -> String {
    c.s("author").filter(|a| !a.is_empty()).unwrap_or("QA").to_string()
}

fn what(c: &Row) -> String {
    c.s("ask").filter(|a| !a.is_empty()).or(c.s("title")).unwrap_or("").to_string()
}

/// A follow-up task for the comment, or more for the open one on the same ticket.
pub fn act(app: &App, c: &Row, src: &Row, who: &str, note: Option<&str>) -> Result<Row> {
    let key = c.st("jira_key");
    if let Some(t) = open_follow_up(app, &key)? {
        app.db.x("UPDATE tasks SET detail = ?, updated_at = ? WHERE id = ?", p![format!("{}\n\n{}", t.st("detail"), more_brief(app, c, note)), now_iso(), t.id()])?;
        board::bump_ctx(app, t.id())?;
        handled(app, c, who, Some(t.id()))?;
        board::log_event(app, t.id(), who, "jira", &format!("{} commented on {key} again: {}", author(c), what(c)))?;
        return Ok(t);
    }
    let g = goal_of(app, src)?;
    let url = comment_url(app, &key, &c.st("comment_id"));
    let mut meta = vec![json!(["Ticket", key]), json!(["Comment", url]), json!(["Earlier task", rf("task", src.id())])];
    if let Some(n) = src.i("pr_num") {
        meta.push(json!(["Earlier PR", format!("#{n}")]));
    }
    let title = c.s("title").filter(|t| !t.is_empty()).map(|t| t.to_string()).unwrap_or(src.st("title"));
    let made = ops::new_task(
        app,
        &json!({"title": clip(&format!("QA on {key}: {title}"), 300), "project": src.v("project"), "goal_id": g.as_ref().map(|g| g.id()),
                "priority": "high", "pickup": {"mode": "new"}, "jira": {"mode": "link", "key": key},
                "detail": brief(app, c, src, note), "meta": meta,
                "origin": {"from": format!("Jira comment on {key}"), "by": author(c), "url": url}}),
        who,
        Some(&format!("From {}'s comment on {key}{}", author(c), if who != board::BOARD { format!(", on {}'s word", app.cfg.owner) } else { String::new() })),
    )?;
    let t = board::get_task(app, made["id"].as_i64().unwrap_or(0))?;
    handled(app, c, who, Some(t.id()))?;
    board::log_event(app, src.id(), who, "jira", &format!("{} commented on {key}; {} is doing it", author(c), rf("task", t.id())))?;
    if who == board::BOARD {
        dispatch::notify(app, &format!("{} on {key}: {}. {} is on it.", author(c), what(c), rf("task", t.id())), &dispatch::alert_url(app, Some(t.id()), None, None), None);
    }
    Ok(t)
}

fn handled(app: &App, c: &Row, who: &str, task_id: Option<i64>) -> Result<()> {
    app.db.x(
        "UPDATE qa_comments SET task_id = COALESCE(?, task_id), handled_at = ?, handled_by = ?, updated_at = ? WHERE id = ?",
        p![task_id, now_iso(), who, now_iso(), c.id()],
    )?;
    dispatch::clear_alert_key(app, &alert_key(c.id()))
}

fn brief(app: &App, c: &Row, src: &Row, note: Option<&str>) -> String {
    let (key, who) = (c.st("jira_key"), author(c));
    let mut lines = vec![format!("{who} commented on {key} while testing it."), String::new(), format!("What they want: {}", what(c))];
    if let Some(n) = note {
        lines.extend([String::new(), format!("{} says: {n}", app.cfg.owner)]);
    }
    lines.extend([
        String::new(),
        "Their comment:".into(),
        c.s("text").filter(|t| !t.is_empty()).unwrap_or("(not read yet: read it on the ticket)").to_string(),
        String::new(),
        format!("Comment: {}", comment_url(app, &key, &c.st("comment_id"))),
        format!("Earlier work: {} “{}”{}", rf("task", src.id()), src.st("title"), src.i("pr_num").map(|n| format!(", PR #{n}")).unwrap_or_default()),
        String::new(),
    ]);
    if c.b("pr") {
        lines.push(format!(
            "Make the change and open its PR on {key}. Your done summary goes on {key} as a comment, so write it to {who}: what changed and how to check it. \
             If you find it needs nothing after all, finish with no PR and say why."
        ));
    } else {
        lines.push(format!("Do it and report back. Your done summary goes on {key} as a comment, so write it to {who}. If it turns out to need a code change, say what in the summary."));
    }
    lines.join("\n")
}

fn more_brief(app: &App, c: &Row, note: Option<&str>) -> String {
    let mut lines = vec![format!("Another comment from {} on {}: {}", author(c), c.st("jira_key"), what(c))];
    if let Some(n) = note {
        lines.push(format!("{} says: {n}", app.cfg.owner));
    }
    lines.push(format!("Comment: {}", comment_url(app, &c.st("jira_key"), &c.st("comment_id"))));
    lines.push(String::new());
    lines.push(c.s("text").filter(|t| !t.is_empty()).unwrap_or("(not read yet)").to_string());
    lines.join("\n")
}

/// `tb qa task Q3 [--note]` / `tb qa ignore Q3`: the owner's word on a flagged comment.
pub fn resolve(app: &App, c: &Row, action: &str, who: &str, note: Option<&str>, pr: Option<bool>) -> Result<Option<Row>> {
    if c.s("handled_at").is_some() {
        return err(409, format!("{} was already handled by {}.", rf("qa", c.id()), c.s("handled_by").unwrap_or("someone")));
    }
    match action {
        "ignore" => {
            handled(app, c, who, None)?;
            board::log_event(app, c.i0("source_task_id"), who, "jira", &format!("{}'s comment on {} was left as it is", author(c), c.st("jira_key")))?;
            Ok(None)
        }
        "task" => {
            if pr.is_some() || c.get("pr").map(Value::is_null).unwrap_or(true) {
                app.db.x("UPDATE qa_comments SET pr = ? WHERE id = ?", p![(pr != Some(false)) as i64, c.id()])?;
            }
            let c = get(app, c.id())?;
            Ok(Some(act(app, &c, &board::get_task(app, c.i0("source_task_id"))?, who, note)?))
        }
        _ => err(400, "Say task or ignore."),
    }
}

/// Each runner tick: ask Jira for new comments every few minutes, and retry comments that couldn't be read.
pub fn tick(app: &App) -> Result<()> {
    if !on(app) {
        return Ok(());
    }
    for c in app.db.q("SELECT * FROM qa_comments WHERE verdict IS NULL AND tries < ?", p![TRIES])? {
        let wait = if c.i0("tries") > 0 { age_secs(c.s("updated_at")).unwrap_or(0.0) < RETRY_SECS } else { age_secs(c.s("created_at")).unwrap_or(0.0) < 60.0 };
        if !wait && !app.shared.lock().qa_reading.contains(&c.id()) {
            read_later(app, c.id());
        }
    }
    let checked = app.db.get_setting(CHECKED_KEY)?;
    if checked.as_deref().map(|c| age_secs(Some(c)).unwrap_or(f64::MAX) < POLL_SECS).unwrap_or(false) {
        return Ok(());
    }
    app.db.set_setting(CHECKED_KEY, Some(&now_iso()))?;
    app.queue_ai(Box::new(move |a: &App| {
        if let Err(e) = poll(a) {
            a.info(format!("qa: couldn't check Jira for comments: {e}"));
        }
    }));
    Ok(())
}

/// Board tickets updated since the last check, and their comments left since QA was switched on.
pub fn poll(app: &App) -> std::result::Result<usize, String> {
    let since = app.db.get_setting(SINCE_KEY).map_err(|e| e.message)?.unwrap_or_else(now_iso);
    let cutoff = iso(now_ts() - WATCH_DAYS * 86400.0);
    let keys: Vec<String> = app
        .db
        .q(
            "SELECT DISTINCT jira_key FROM tasks WHERE jira_key IS NOT NULL AND jira_key != '' \
             AND (status != 'done' OR COALESCE(finished_at, updated_at) >= ?) ORDER BY id DESC LIMIT 100",
            p![cutoff],
        )
        .map_err(|e| e.message)?
        .iter()
        .map(|r| r.st("jira_key"))
        .collect();
    if keys.is_empty() {
        return Ok(0);
    }
    // Jira's JQL takes minutes, not seconds: look back a little past the last check.
    let comments = jira::recent_comments(app, &keys, (POLL_SECS / 60.0) as i64 * 2 + 5)?;
    let since = parse_iso(&since).unwrap_or(0.0);
    let mut new = 0;
    for cm in comments {
        let older = cm["created"].as_str().and_then(jira::comment_time).map(|t| t < since).unwrap_or(false);
        if older || is_owner(app, &cm["author"]) {
            continue;
        }
        let ev = json!({"key": cm["key"], "comment_id": cm["id"], "author": cm["author"]["displayName"], "text": cm["text"]});
        if intake(app, &ev).map(|r| r["new"] == true).unwrap_or(false) {
            new += 1;
        }
    }
    Ok(new)
}

pub fn card(app: &App, c: &Row) -> Value {
    let mut v = Value::Object(c.clone());
    v["ref"] = json!(rf("qa", c.id()));
    v["url"] = json!(comment_url(app, &c.st("jira_key"), &c.st("comment_id")));
    v["task"] = rf_opt("task", c.i("task_id"));
    v["source_task"] = rf_opt("task", c.i("source_task_id"));
    v["waiting"] = json!(c.s("verdict") == Some("flag") && c.s("handled_at").is_none());
    v
}

/// `GET /qa-comments`: newest first; `waiting` keeps the flags still waiting on the owner.
pub fn listing(app: &App, limit: i64, waiting: bool) -> Result<Vec<Value>> {
    let w = if waiting { "WHERE verdict = 'flag' AND handled_at IS NULL " } else { "" };
    Ok(app.db.q(&format!("SELECT * FROM qa_comments {w}ORDER BY id DESC LIMIT ?"), p![limit])?.iter().map(|c| card(app, c)).collect())
}

/// The goal page's QA tab: comments on its tickets that need or needed something, waiting ones first.
pub fn for_goal(app: &App, goal_id: i64) -> Result<Vec<Value>> {
    if !on(app) {
        return Ok(vec![]);
    }
    let rows = app.db.q(
        "SELECT q.*, f.title AS task_title, f.status AS task_status FROM qa_comments q \
         LEFT JOIN tasks s ON s.id = q.source_task_id LEFT JOIN tasks f ON f.id = q.task_id \
         WHERE (s.goal_id = ? OR f.goal_id = ?) AND COALESCE(q.verdict, '') != 'none' \
         ORDER BY q.verdict = 'flag' AND q.handled_at IS NULL DESC, q.id DESC",
        p![goal_id, goal_id],
    )?;
    Ok(rows.iter().map(|c| card(app, c)).collect())
}
