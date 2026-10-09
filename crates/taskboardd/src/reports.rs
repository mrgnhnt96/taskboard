//! `POST /report`: what the hooks and `tb` tell the board.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, deliver, fields, handoff, hooks, lines, midna, ops, p, prflow, projects, screen, steps, transcript, waitsfor};

static MARKER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[task-board:[Tt](\d+)\]").unwrap());
static COMMIT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^\[[^\]]+\]\s+(.+)$").unwrap());
/// An API error that means the Mac lost its connection, not that the API refused.
static NETWORK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)connection error|unable to connect|network|offline|timed? ?out|fetch failed|socket hang up|ECONN|ENOTFOUND|ETIMEDOUT|EAI_AGAIN|ENETUNREACH|EHOSTUNREACH").unwrap()
});
static LEADING_MARKER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*\[task-board:[TJ]\d+\]\s*").unwrap());

pub struct Report<'a> {
    pub app: &'a App,
    pub body: Value,
    pub event: String,
    pub sid: Option<String>,
    pub claude: Option<String>,
    pub cwd: Option<String>,
    pub git: Row,
    pub spooled: bool,
    pub at: String,
    pub waiting_on_background: bool,
    pub stalled: bool,
    pub screened: Option<Value>,
    /// Background commands still running in the transcript, read before the transaction.
    pub background: transcript::Background,
    /// The files the turn edited, read before the transaction for a stop on a terminal with no task.
    pub turn_files: Vec<String>,
    /// When that turn started.
    pub turn_at: Option<String>,
    /// The checkout the turn committed in, when its HEAD moved.
    pub turn_commit: Option<String>,
}

fn s_of(v: &Value, k: &str) -> Option<String> {
    match v.get(k) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

impl<'a> Report<'a> {
    pub fn new(app: &'a App, body: Value, spooled: bool) -> Self {
        let at = if spooled { s_of(&body, "at").unwrap_or_else(now_iso) } else { now_iso() };
        Report {
            app,
            event: s_of(&body, "event").unwrap_or_default(),
            sid: s_of(&body, "session"),
            claude: s_of(&body, "claude_session"),
            cwd: s_of(&body, "cwd"),
            git: body.get("git").and_then(|g| g.as_object()).cloned().unwrap_or_default(),
            body,
            spooled,
            at,
            waiting_on_background: false,
            stalled: false,
            screened: None,
            background: transcript::Background::default(),
            turn_files: vec![],
            turn_at: None,
            turn_commit: None,
        }
    }

    fn sid(&self) -> Option<&str> {
        self.sid.as_deref()
    }

    /// The terminal's name, as a session's own report gives it. A `tb` report's `name` is what it's about
    /// (a step, a goal), never the terminal's.
    fn terminal_name(&self) -> Option<&str> {
        if self.event.starts_with("tb.") {
            return None;
        }
        self.body.get("name").and_then(|v| v.as_str())
    }

    pub fn name(&self) -> String {
        board::session_name(self.app, self.sid(), self.terminal_name())
    }

    fn b(&self, k: &str) -> String {
        body_str(&self.body, k)
    }

    fn log(&self, task_id: i64, kind: &str, text: &str, data: Option<Value>) -> Result<i64> {
        board::log_event_full(self.app, task_id, &self.name(), kind, text, data, Some(&self.at))
    }

    pub fn task(&self) -> Result<Option<Row>> {
        let explicit = self.body.get("task").cloned().unwrap_or(Value::Null);
        if !explicit.is_null() && explicit != "" {
            let id = need_ref(&explicit, "task")?;
            return board::get_task(self.app, id).map(Some);
        }
        board::task_for_session(self.app, self.sid())
    }

    fn touch_session(&self, alive: bool) -> Result<()> {
        let Some(sid) = self.sid() else { return Ok(()) };
        let app = self.app;
        let cur = board::get_session(app, Some(sid))?;
        let mut f = fields!["seen_at" => now_iso(), "last_activity" => self.at];
        if let Some(b) = self.git.s("branch").filter(|b| !b.is_empty()) {
            f.push(("branch", json!(b)));
        }
        if let Some(u) = self.git.i("uncommitted") {
            f.push(("dirty", json!(u)));
        }
        if let Some(c) = &self.claude {
            f.push(("claude_session_id", json!(c)));
        }
        if let Some(n) = self.terminal_name().filter(|n| !n.is_empty()) {
            f.push(("name", json!(n)));
            board::note_rename(app, cur.as_ref(), Some(n))?;
        }
        if cur.as_ref().map(|c| c.s("source") != Some("midna")).unwrap_or(true) {
            if let Some(cwd) = &self.cwd {
                if !cur.as_ref().map(|c| has(c.s("project"))).unwrap_or(false) {
                    let project = projects::project_for_path(app, Some(cwd))?;
                    let path = projects::project_path(app, project.as_deref())?.unwrap_or_else(|| cwd.clone());
                    f.push(("project", json!(project)));
                    f.push(("project_path", json!(path)));
                }
            }
        }
        if alive {
            f.push(("missed", json!(0)));
            if cur.as_ref().map(|c| c.s("status") == Some("gone")).unwrap_or(false) {
                f.push(("status", json!("idle")));
                f.push(("gone_at", Value::Null));
            }
        }
        if cur.is_some() {
            app.db.update("sessions", &json!(sid), f)?;
        } else {
            f.extend(fields!["source" => "hook", "agent" => "claude"]);
            board::upsert_session(app, sid, f)?;
        }
        Ok(())
    }

    fn set_session_status(&self, status: &str) -> Result<()> {
        if let Some(sid) = self.sid() {
            self.app.db.x("UPDATE sessions SET status = ? WHERE id = ? AND status != 'gone'", p![status, sid])?;
        }
        Ok(())
    }

    /// Saves how many background commands and agents the turn that just ended left running, so the
    /// terminal shows it's waiting on them rather than idle.
    fn note_background(&self, running: transcript::Background) -> Result<()> {
        if let Some(sid) = self.sid() {
            self.app.db.x(
                "UPDATE sessions SET background = ?, background_agents = ?, background_at = ? WHERE id = ?",
                p![running.total() as i64, running.agents as i64, now_iso(), sid],
            )?;
        }
        Ok(())
    }

    fn elsewhere(&self, t: &Row) -> Result<bool> {
        let (Some(cwd), Some(project)) = (&self.cwd, t.s("project").filter(|p| !p.is_empty())) else { return Ok(false) };
        Ok(projects::project_for_path(self.app, Some(cwd))?.as_deref() != Some(project))
    }

    /// A hook from inside another open task's worktree: not this task's place.
    fn in_other_task(&self, t: &Row) -> Result<bool> {
        match &self.cwd {
            Some(cwd) => Ok(crate::worktrees::owner(self.app, cwd, t)?.is_some()),
            None => Ok(false),
        }
    }

    fn away(&self, t: &Row) -> Result<bool> {
        Ok(self.elsewhere(t)? || self.in_other_task(t)?)
    }

    fn update_where(&self, t: &Row, turn: bool) -> Result<Row> {
        let mut ctx = board::task_context(t);
        let mut w = ctx.get("where").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        let away = self.away(t)?;
        if !away {
            if let Some(b) = self.git.s("branch").filter(|s| !s.is_empty()) {
                w.insert("branch".into(), json!(b));
            }
            if let Some(c) = self.git.s("commit").filter(|s| !s.is_empty()) {
                w.insert("last_commit".into(), json!(c));
            }
            if let Some(c) = self.git.s("sha").filter(|s| !s.is_empty()) {
                w.insert("sha".into(), json!(c));
            }
            if let Some(u) = self.git.i("uncommitted") {
                w.insert("uncommitted".into(), json!(u));
            }
            if let Some(cwd) = &self.cwd {
                w.insert("worktree".into(), json!(cwd));
            }
        }
        if let Some(c) = &self.claude {
            w.insert("conversation".into(), json!(c));
        }
        ctx.insert("where".into(), Value::Object(w));
        if turn {
            let n = ctx.i("turns").unwrap_or(0) + 1;
            ctx.insert("turns".into(), json!(n));
        }
        ctx.insert("saved_at".into(), json!(now_iso()));
        board::save_context(self.app, t.id(), &ctx, false)?;
        Ok(ctx)
    }

    fn sent(&self, t: &Row) -> Result<()> {
        if let Some(sid) = self.sid() {
            self.app.db.x("UPDATE sessions SET ctx_task = ?, ctx_version = ? WHERE id = ?", p![t.id(), t.v("ctx_version"), sid])?;
        }
        Ok(())
    }

    fn needs_context(&self, t: &Row) -> Result<bool> {
        let Some(s) = board::get_session(self.app, self.sid())? else { return Ok(true) };
        Ok(s.i("ctx_task") != Some(t.id()) || s.i0("ctx_version") < t.i("ctx_version").unwrap_or(1))
    }

    fn handoff_for(&self, t: &Row) -> Result<String> {
        let t = board::get_task(self.app, t.id())?;
        self.sent(&t)?;
        handoff::build(self.app, t.id())
    }
}

pub fn ok(task: Option<&Row>, context: Option<String>) -> Value {
    let mut out = json!({"ok": true, "task": task.map(|t| json!(rf("task", t.id()))).unwrap_or(Value::Null)});
    if let Some(c) = context.filter(|c| !c.is_empty()) {
        out["context"] = json!(c);
    }
    out
}

fn with(mut v: Value, extra: Value) -> Value {
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, x) in e {
            o.insert(k.clone(), x.clone());
        }
    }
    v
}

fn store_plugin(r: &Report) -> Result<()> {
    let tb = r.b("tb_path");
    if !tb.is_empty() {
        r.app.db.set_setting("tb_path", Some(&tb))?;
    }
    let v = r.b("plugin_version");
    if !v.is_empty() {
        r.app.db.set_setting("plugin_version", Some(&v))?;
    }
    Ok(())
}

/// SessionStart sources that mean a new Claude process: the old one's background work died with it.
const NEW_PROCESS_SOURCES: &[&str] = &["startup", "resume"];

fn on_session_start(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    store_plugin(r)?;
    if NEW_PROCESS_SOURCES.contains(&r.b("source").as_str()) {
        r.note_background(transcript::Background::default())?;
    }
    let Some(t) = r.task()? else {
        if crate::jira_desk::is_desk(app, r.sid())? {
            return Ok(ok(None, Some(crate::jira_desk::intro(app))));
        }
        let s = board::get_session(app, r.sid())?;
        let mut c = handoff::no_task_line(app, s.as_ref().and_then(|s| s.s("project")))?;
        let waiting = r.sid().map(|sid| lines::summary(app, sid)).transpose()?.unwrap_or_default();
        if !waiting.is_empty() {
            c = format!("{c}\n{waiting} tb switch T<n> takes one.");
        }
        return Ok(ok(None, Some(c)));
    };
    if let (Some(c), true) = (&r.claude, t.s("session_id") == r.sid()) {
        board::update_task(app, t.id(), fields!["claude_session_id" => c])?;
    }
    if t.b("lost") && t.s("session_id") == r.sid() {
        let src = r.b("source");
        board::set_working(app, &t, &r.name(), Some(&format!("Terminal came back ({})", if src.is_empty() { "restart" } else { &src })))?;
    }
    r.set_session_status(if t.s("status") == Some("working") { "working" } else { "idle" })?;
    let mut ctx = r.handoff_for(&t)?;
    if let Some(sid) = r.sid().filter(|s| t.s("session_id") == Some(*s)) {
        let waiting = lines::summary(app, sid)?;
        if !waiting.is_empty() {
            ctx = format!("{ctx}\n\n{waiting}");
        }
    }
    Ok(ok(Some(&t), Some(ctx)))
}

fn back_to_work(r: &Report, t: &Row, prompt: Option<&str>) -> Result<()> {
    if t.s("status") != Some("needs") || t.b("lost") {
        return Ok(());
    }
    let reason = t.st("needs_reason");
    if matches!(reason.as_str(), "attention" | "offline" | "api_error") || (reason == "question" && (prompt.map(|p| !p.is_empty()).unwrap_or(false) || has(t.s("answered_at")))) {
        let text = if has(t.s("answered_at")) { "Back to work after your answer" } else { "Back to work" };
        board::set_working(r.app, t, &r.name(), Some(text))?;
    }
    Ok(())
}

fn on_prompt(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let prompt = r.body.get("prompt").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let m = MARKER_RE.captures(&prompt).and_then(|c| c[1].parse::<i64>().ok());
    if let Some(sid) = r.sid() {
        app.db.x("UPDATE sessions SET board_prompt = ? WHERE id = ?", p![m.is_some() as i64, sid])?;
        if !r.spooled {
            let tree = r.body.get("tree").filter(|t| t.is_object()).map(jdumps);
            app.db.x("UPDATE sessions SET turn_tree = ? WHERE id = ?", p![tree, sid])?;
        }
    }
    if let Some(n) = m {
        let t = board::find_task(app, Some(n))?;
        let mine = board::task_for_session(app, r.sid())?;
        let handed_over = prompt.contains("You are picking up") || prompt.trim_end().ends_with("] continue");
        let free = mine.as_ref().map(|m| Some(m.id()) == t.as_ref().map(|t| t.id())).unwrap_or(true) || handed_over;
        if let Some(t) = &t {
            if t.s("status") == Some("done") && prflow::waking(app, t)? {
                prflow::picked_up(app, t, r.sid().unwrap_or(""), &r.name())?;
                prflow::resumed(app, &board::get_task(app, t.id())?)?;
                r.set_session_status("working")?;
                return Ok(ok(Some(t), None));
            }
            let already = t.s("session_id") == r.sid() && matches!(t.s("status"), Some("working") | Some("needs")) && !t.b("lost");
            if free && !already {
                if let Some(sid) = r.sid.clone() {
                    if let Err(why) = board::claim(app, t, &sid, r.claude.as_deref(), None, "Picked up", false)? {
                        return Ok(ok(None, Some(format!("Task board: {why} Ask {} before working on it.", app.cfg.owner))));
                    }
                    let t = board::get_task(app, t.id())?;
                    r.set_session_status("working")?;
                    if handoff::is_full_handoff(&prompt, t.id()) {
                        r.sent(&t)?;
                        return Ok(ok(Some(&t), None));
                    }
                    let c = r.handoff_for(&t)?;
                    return Ok(ok(Some(&t), Some(c)));
                }
            }
        }
    }
    let Some(t) = r.task()? else {
        if let Some(v) = prflow::visited_by(app, r.sid().unwrap_or(""))? {
            prflow::resumed(app, &v)?;
        }
        return Ok(ok(None, None));
    };
    r.set_session_status("working")?;
    if t.s("session_id") == r.sid() && t.b("lost") {
        board::set_working(app, &t, &r.name(), Some("Terminal came back"))?;
    }
    if t.s("status") == Some("done") {
        return Ok(ok(Some(&t), None));
    }
    back_to_work(r, &board::get_task(app, t.id())?, Some(&prompt))?;
    let t = board::get_task(app, t.id())?;
    let mut ctx = if r.needs_context(&t)? { vec![r.handoff_for(&t)?] } else { vec![] };
    if m.is_none() && owners_ask(&prompt) {
        if let Some(sid) = r.sid() {
            ctx.push(lines::split_check(app, &t, sid)?);
        }
    }
    Ok(ok(Some(&t), Some(ctx.join("\n\n"))))
}

/// Whether a prompt is an ask the owner typed (not a slash command, the board's words, or a tool's tags).
fn owners_ask(prompt: &str) -> bool {
    let p = LEADING_MARKER_RE.replace(prompt, "").trim().to_string();
    !p.is_empty() && !p.starts_with('/') && !p.starts_with('<') && !p.starts_with("[task-board")
}

fn ledger_files(app: &App, sid: &str, task_id: i64, event_id: i64) -> Result<()> {
    let Some(s) = board::get_session(app, Some(sid))? else { return Ok(()) };
    let Some(turn) = transcript::last_turn(app, &s) else { return Ok(()) };
    if turn.files.is_empty() {
        return Ok(());
    }
    app.db.tx(|| {
        let Some(t) = board::find_task(app, Some(task_id))? else { return Ok(()) };
        let mut ctx = board::task_context(&t);
        let mut files: Vec<String> = ctx
            .get("files")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        for f in &turn.files {
            files.retain(|x| x != f);
            files.push(f.clone());
        }
        let n = files.len();
        if n > 100 {
            files.drain(..n - 100);
        }
        ctx.insert("files".into(), json!(files));
        board::save_context(app, task_id, &ctx, false)?;
        app.db.x(
            "UPDATE events SET data = ? WHERE id = ?",
            p![jdumps(&json!({"files": turn.files, "turn": ctx.get("turns")})), event_id],
        )?;
        Ok(())
    })
}

fn on_stop(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    r.set_session_status("idle")?;
    r.note_background(r.background)?;
    let Some(t) = r.task()? else {
        if let Some(ask) = changed_with_no_task(r)? {
            r.set_session_status("working")?;
            return Ok(with(ok(None, None), json!({"block": ask})));
        }
        return pr_visit_paused(r, &one_line(&r.b("last_message"), 500));
    };
    let msg = clip(&r.b("last_message"), 2000);
    let mut sentence = first_sentence(&msg, 200);
    if sentence.is_empty() {
        sentence = "Turn finished".into();
    }
    let ctx = r.update_where(&t, true)?;
    let eid = r.log(t.id(), "turn", &sentence, Some(json!({"turn": ctx.get("turns")})))?;
    back_to_work(r, &board::get_task(app, t.id())?, None)?;
    let t = board::get_task(app, t.id())?;
    if matches!(t.s("status"), Some("working") | Some("queued")) && sentence != "Turn finished" {
        board::update_task(app, t.id(), fields!["latest" => one_line(&sentence, 200)])?;
    }
    if let Some(pr) = find_pr(&msg) {
        prflow::link_pr(app, &t, &pr, &r.name())?;
    }
    r.stalled = stalled(r, &t)?;
    if let Some(sid) = r.sid.clone() {
        let tid = t.id();
        app.defer(Box::new(move |a: &App| {
            if let Err(e) = ledger_files(a, &sid, tid, eid) {
                a.info(format!("ledger: {e}"));
            }
        }));
    }
    app.wake_runner();
    Ok(ok(Some(&t), None))
}

/// What changed in a checkout between two of the hook's tree stamps: the files whose stamp is new,
/// different or gone (committed or reverted, still on disk), and the checkout's root when HEAD moved.
fn tree_changes(before: &Value, after: &Value) -> (Vec<String>, Option<String>) {
    let root = after["root"].as_str().unwrap_or("");
    if root.is_empty() || before["root"].as_str() != Some(root) {
        return (vec![], None);
    }
    let (was, now) = (&before["files"], &after["files"]);
    let mut files: Vec<String> =
        now.as_object().map(|n| n.iter().filter(|(f, stamp)| was.get(f.as_str()) != Some(*stamp)).map(|(f, _)| f.clone()).collect()).unwrap_or_default();
    if let Some(w) = was.as_object() {
        files.extend(w.keys().filter(|f| now.get(f.as_str()).is_none() && std::path::Path::new(f).exists()).cloned());
    }
    let moved = after["head"].as_str().filter(|h| !h.is_empty()).is_some_and(|h| before["head"].as_str() != Some(h));
    (files, moved.then(|| root.to_string()))
}

const UNTRACKED_FILES_SHOWN: usize = 3;

/// The turn's edits that fall in a known project or the terminal's own folder.
fn project_files(app: &App, s: &Row, files: &[String]) -> Result<Vec<String>> {
    let here = s.s("project_path").filter(|p| !p.is_empty()).or(s.s("cwd")).unwrap_or("").trim_end_matches('/').to_string();
    let mut roots: Vec<String> = crate::projects::list_projects(app)?
        .iter()
        .filter_map(|p| p["path"].as_str().map(|x| x.trim_end_matches('/').to_string()))
        .filter(|p| !p.is_empty())
        .collect();
    if !here.is_empty() {
        roots.push(here.clone());
    }
    let mut out: Vec<String> = vec![];
    for f in files {
        let full = if f.starts_with('/') { f.clone() } else { format!("{}/{f}", if here.is_empty() { "" } else { &here }) };
        let full = std::path::Path::new(&full).components().collect::<std::path::PathBuf>().to_string_lossy().to_string();
        if !out.contains(f) && roots.iter().any(|root| full == *root || full.starts_with(&format!("{root}/"))) {
            out.push(f.clone());
        }
    }
    Ok(out)
}

fn shown_files(files: &[String]) -> String {
    let names: Vec<String> = files
        .iter()
        .take(UNTRACKED_FILES_SHOWN)
        .map(|f| std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.clone()))
        .collect();
    let more = files.len() - names.len();
    names.join(", ") + &if more > 0 { format!(" and {more} more") } else { String::new() }
}

/// A done task this terminal finished (no PR, not failed) that a follow-up turn changed code for.
fn reopens_on_change(t: &Row, sid: &str) -> bool {
    t.s("status") == Some("done") && !t.b("failed") && t.i("pr_num").is_none() && t.s("session_id").map(|s| s == sid).unwrap_or(true)
}

fn reopen(r: &Report, t: &Row, sid: &str) -> Result<()> {
    let app = r.app;
    let name = r.name();
    board::update_task(app, t.id(), fields!["session_id" => sid, "session_name" => name, "finished_at" => null, "lost" => 0])?;
    board::set_working(app, &board::get_task(app, t.id())?, &name, Some(&format!("Reopened: {} asked for more changes in {name}", app.cfg.owner)))?;
    board::update_task(app, t.id(), fields!["latest" => format!("Reopened for more changes in {name}.")])?;
    board::upsert_session(app, sid, fields!["last_task" => t.id()])?;
    Ok(())
}

/// A turn on a terminal with no task that changed code: what to tell the agent so the work gets
/// tracked. A follow-up on the done task this terminal last finished reopens it instead.
fn changed_with_no_task(r: &Report) -> Result<Option<String>> {
    let Some(sid) = r.sid.clone() else { return Ok(None) };
    if (r.turn_files.is_empty() && r.turn_commit.is_none()) || prflow::visited_by(r.app, &sid)?.is_some() {
        return Ok(None);
    }
    let Some(s) = board::get_session(r.app, Some(&sid))? else { return Ok(None) };
    let files = project_files(r.app, &s, &r.turn_files)?;
    let committed = !project_files(r.app, &s, r.turn_commit.as_slice())?.is_empty();
    if files.is_empty() && !committed {
        return Ok(None);
    }
    let tb = board::tb_cmd(r.app);
    let shown = if files.is_empty() { format!("a commit: {}", r.git.st("commit")) } else { shown_files(&files) };
    let last = board::find_task(r.app, s.i("last_task"))?;
    if let Some(last) = last {
        if last.s("status") == Some("done") && last.st("finished_at") >= r.turn_at.clone().unwrap_or_default() {
            return Ok(None);
        }
        if reopens_on_change(&last, &sid) {
            reopen(r, &last, &sid)?;
            return Ok(Some(format!(
                "[task-board] This turn changed code ({shown}) after {} was marked done, so the board reopened it. \
                 Finish it again now: {tb} done \"<one paragraph covering all the work on it, old and new>\". \
                 Then end your turn without repeating your answer.",
                rf("task", last.id())
            )));
        }
    }
    let owner = &r.app.cfg.owner;
    let waiting = lines::summary(r.app, &sid)?;
    let switch = if waiting.is_empty() { String::new() } else { format!(" {waiting} If the work is one of those, run {tb} switch T<n> instead.") };
    Ok(Some(format!(
        "[task-board] This turn changed code ({shown}), but this terminal has no task, so the board isn't tracking the work. \
         Put it on the board now as a standalone task on this terminal: {tb} task new \"<short imperative title>\" \
         --detail \"<what {owner} asked for and what you changed>\" --here.{switch} Then end your turn without repeating your answer. \
         If {owner} said not to track it, just end your turn."
    )))
}

fn carry_on_text(app: &App, task: &str) -> String {
    let owner = &app.cfg.owner;
    format!(
        "[task-board] You ended your turn, but {task} isn't finished and nothing is waiting on {owner}: the board \
         started this turn and you didn't ask a question. Carry on with the next step now. If you really need {owner}, \
         run tb question; if you need another task's work, run tb wait-for, with --merged when you need its PR merged first."
    )
}

fn nudge(r: &Report) -> Result<Option<Value>> {
    let Some(t) = r.task()? else { return Ok(None) };
    r.app.db.x("UPDATE sessions SET board_prompt = 0 WHERE id = ?", p![r.sid()])?;
    deliver::add(r.app, "carry_on", &carry_on_text(r.app, &rf("task", t.id())), t.id(), None, None)?;
    deliver::take(r.app, r.sid(), &r.event)
}

fn stalled(r: &Report, t: &Row) -> Result<bool> {
    if t.s("status") != Some("working") || t.b("lost") || as_bool(r.body.get("stop_hook_active"), false) {
        return Ok(false);
    }
    let Some(s) = board::get_session(r.app, r.sid())? else { return Ok(false) };
    if !s.b("board_prompt") || t.s("session_id") != r.sid() {
        return Ok(false);
    }
    Ok(r.background.total() == 0)
}

fn on_pre_compact(r: &mut Report) -> Result<Value> {
    r.touch_session(true)?;
    if let Some(sid) = r.sid() {
        r.app.db.x("UPDATE sessions SET compacting_at = ? WHERE id = ?", p![r.at, sid])?;
    }
    let Some(t) = r.task()? else { return Ok(ok(None, None)) };
    let ctx = r.update_where(&t, false)?;
    r.log(t.id(), "checkpoint", "Saved before compacting", Some(json!({"context": ctx})))?;
    Ok(ok(Some(&t), None))
}

fn commit_subject(r: &Report, out: &str) -> String {
    COMMIT_RE
        .captures(out)
        .map(|c| c[1].trim().to_string())
        .or_else(|| r.git.s("commit").filter(|c| !c.is_empty()).map(|c| c.to_string()))
        .unwrap_or_else(|| "a commit".into())
}

fn on_commit(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let Some(t) = r.task()? else { return Ok(ok(None, None)) };
    let cmd = r.b("command");
    let out = clip(&r.b("output"), 1000);
    r.update_where(&t, false)?;
    if r.away(&t)? {
        return Ok(ok(Some(&t), None));
    }
    if cmd.contains("git commit") {
        let subject = commit_subject(r, &out);
        r.log(t.id(), "commit", &format!("Committed: {}", one_line(&subject, 200)), None)?;
        let mut ctx = board::task_context(&board::get_task(app, t.id())?);
        let mut w = ctx.get("where").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        w.insert("last_commit".into(), json!(subject));
        ctx.insert("where".into(), Value::Object(w));
        board::save_context(app, t.id(), &ctx, false)?;
    }
    if cmd.contains("git push") {
        let branch = r.git.s("branch").filter(|b| !b.is_empty()).unwrap_or("the branch").to_string();
        r.log(t.id(), "commit", &format!("Pushed {branch}"), None)?;
        if let Some(pr) = find_pr(&out) {
            prflow::link_pr(app, &board::get_task(app, t.id())?, &pr, &r.name())?;
        }
    }
    Ok(ok(Some(&t), None))
}

fn on_session_end(r: &mut Report) -> Result<Value> {
    let reason = { let x = r.b("reason"); if x.is_empty() { "other".to_string() } else { x } };
    let t = r.task()?;
    if reason == "clear" {
        r.touch_session(true)?;
        return Ok(ok(t.as_ref(), None));
    }
    r.touch_session(false)?;
    if let Some(sid) = r.sid.clone() {
        board::session_gone(r.app, &sid, &reason)?;
    }
    Ok(ok(t.as_ref(), None))
}

fn pr_visit_paused(r: &Report, message: &str) -> Result<Value> {
    let t = prflow::visited_by(r.app, r.sid().unwrap_or(""))?;
    if let Some(t) = &t {
        if prflow::stopped(r.app, t, &r.name(), message, false, None)? {
            r.set_session_status("needs")?;
        }
    }
    Ok(ok(t.as_ref(), None))
}

/// Events that show the terminal's Claude is trying the API again, which ends "offline"; and the ones
/// that show a turn got through (or it started over), which also end the run of failed tries.
const API_RETRY_EVENTS: &[&str] = &["hook.session_start", "hook.prompt", "hook.stop"];
const API_BACK_EVENTS: &[&str] = &["hook.session_start", "hook.stop"];

/// A turn that ended on an API error: `("network", ..)` when the connection went, else `("api", ..)`,
/// with the sentence the board shows for it.
fn api_error_of(r: &Report) -> (&'static str, String) {
    let error = r.b("error");
    let detail = one_line(&{ let d = r.b("error_details"); if d.is_empty() { r.b("last_message") } else { d } }, 300);
    let (kind, what) = if NETWORK_RE.is_match(&format!("{error} {detail}")) {
        ("network", "Lost its network connection")
    } else {
        ("api", match error.as_str() {
            "rate_limit" => "Hit a usage limit",
            "overloaded" => "The Claude API is overloaded",
            "authentication_failed" | "oauth_org_not_allowed" | "cloud_credential_error" => "Claude Code isn't logged in",
            "billing_error" | "account_on_hold" | "verification_required" => "The Claude account needs attention",
            "server_error" => "The Claude API failed",
            _ => "The Claude API returned an error",
        })
    };
    (kind, if detail.is_empty() { what.to_string() } else { format!("{what} ({detail})") })
}

/// A turn that died on an API error. A lost connection is Midna's to fix: it sends `continue` once
/// the network is back (`agents.resume_after_network`), so the task stays Working until Midna is off
/// or has given up. Any other error needs the owner.
fn on_api_error(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let (kind, text) = api_error_of(r);
    let network = kind == "network";
    let prev = board::get_session(app, r.sid())?;
    let streak = prev.as_ref().filter(|s| network && s.i0("api_error_tries") > 0);
    let tries = streak.map(|s| s.i0("api_error_tries")).unwrap_or(0) + network as i64;
    let since = streak.and_then(|s| s.s("api_error_at").map(str::to_string)).unwrap_or_else(now_iso);
    app.db.x(
        "UPDATE sessions SET api_error = ?, api_error_kind = ?, api_error_at = ?, api_error_tries = ? WHERE id = ?",
        p![text, kind, since, tries, r.sid()],
    )?;
    r.set_session_status("idle")?;
    let Some(t) = r.task()? else { return Ok(ok(None, None)) };
    if t.s("status") != Some("working") || t.b("lost") || t.s("session_id") != r.sid() {
        return Ok(ok(Some(&t), None));
    }
    if network && midna::resumes_after_network(app) && tries <= midna::RESUME_TRIES {
        let note = format!("{text}. Midna will tell it to carry on once the network is back");
        board::update_task(app, t.id(), fields!["latest" => one_line(&note, 200)])?;
        r.log(t.id(), "status", &note, None)?;
        return Ok(ok(Some(&t), None));
    }
    let ask = if network && tries > midna::RESUME_TRIES {
        format!("{text}. Midna told it to carry on {} and it still couldn't connect; type anything in the terminal to try again", plural(midna::RESUME_TRIES, "time"))
    } else {
        format!("{text}. Its turn stopped; type anything in the terminal to carry on")
    };
    board::update_task(app, t.id(), fields!["status" => "needs", "needs_reason" => if network { "offline" } else { "api_error" }, "question" => ask, "answered_at" => null])?;
    r.log(t.id(), "question", &ask, None)?;
    Ok(ok(Some(&t), None))
}

fn on_attention(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let kind = r.b("notification_type");
    if kind == "idle_prompt" && r.background.total() > 0 {
        r.note_background(r.background)?;
        r.waiting_on_background = true;
        return Ok(ok(r.task()?.as_ref(), None));
    }
    r.set_session_status("needs")?;
    let Some(t) = r.task()? else {
        let m = one_line(&{ let m = r.b("message"); if m.is_empty() { r.b("title") } else { m } }, 500);
        return pr_visit_paused(r, &m);
    };
    if t.s("status") == Some("done") {
        return Ok(ok(Some(&t), None));
    }
    if !(t.s("status") == Some("working") || (t.s("status") == Some("needs") && t.s("needs_reason") == Some("attention"))) {
        return Ok(ok(Some(&t), None));
    }
    let default = if kind == "idle_prompt" { "Waiting for your input in the terminal" } else { "Waiting for you in the terminal" };
    let raw = { let m = r.b("message"); if m.is_empty() { r.b("title") } else { m } };
    let message = one_line(if raw.is_empty() { default } else { &raw }, 500);
    if t.s("status") == Some("needs") && t.s("question") == Some(message.as_str()) {
        return Ok(ok(Some(&t), None));
    }
    board::update_task(app, t.id(), fields!["status" => "needs", "needs_reason" => "attention", "question" => message, "answered_at" => null])?;
    r.log(t.id(), "question", &format!("Waiting for you: {message}"), None)?;
    Ok(ok(Some(&t), None))
}

fn on_note(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let text = one_line(&r.b("text"), 2000);
    if text.is_empty() {
        return err(400, "A note needs some text.");
    }
    let t = r.task()?;
    let kind = { let k = r.b("note_kind"); if k.is_empty() { let k2 = r.b("kind"); if k2.is_empty() { "finding".to_string() } else { k2 } } else { k } };
    let goal = r.body.get("goal").cloned().unwrap_or(Value::Null);
    let goal_id = match &goal {
        Value::String(s) if Regex::new(r"^\s*[Gg]?\d+\s*$").unwrap().is_match(s) => parse_ref_str(s.trim(), "goal")?,
        Value::Number(n) => n.as_i64(),
        v if as_bool(Some(v), false) => t.as_ref().and_then(|t| t.i("goal_id")),
        _ => None,
    };
    if let Some(gid) = goal_id {
        board::get_goal(app, gid)?;
        let source = t.as_ref().map(|t| rf("task", t.id())).unwrap_or_else(|| r.name());
        board::add_goal_note(app, gid, &kind, &text, Some(&source), false, t.as_ref().map(|t| t.id()))?;
        if let Some(t) = &t {
            r.log(t.id(), "note", &format!("Goal note ({kind}): {text}"), None)?;
        }
        return Ok(with(ok(t.as_ref(), None), json!({"goal": rf("goal", gid)})));
    }
    let Some(t) = t else { return Ok(ok(None, None)) };
    r.log(t.id(), "note", &text, None)?;
    board::update_task(app, t.id(), fields!["latest" => one_line(&text, 200)])?;
    Ok(ok(Some(&t), None))
}

fn on_checkpoint(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let Some(t) = r.task()? else { return Ok(ok(None, None)) };
    let mut ctx = r.update_where(&t, false)?;
    for key in ["done", "next", "decisions", "files", "answers"] {
        if let Some(v) = r.body.get(key).filter(|v| !v.is_null()) {
            ctx.insert(key.into(), json!(str_list(Some(v))));
        }
    }
    let n = ctx.i("checkpoints").unwrap_or(0) + 1;
    ctx.insert("checkpoints".into(), json!(n));
    ctx.insert("saved_at".into(), json!(now_iso()));
    board::save_context(app, t.id(), &ctx, false)?;
    let list = |k: &str| str_list(ctx.get(k));
    let (done, next, dec) = (list("done"), list("next"), list("decisions"));
    r.log(
        t.id(),
        "checkpoint",
        &format!("Checkpoint: {}", done.last().cloned().unwrap_or_else(|| "progress saved".into())),
        Some(json!({"done": done, "next": next, "decisions": dec})),
    )?;
    let mut latest = vec![];
    if let Some(d) = done.last() {
        latest.push(format!("{}.", d.trim_end_matches('.')));
    }
    if let Some(n) = next.first() {
        latest.push(format!("Next: {}.", n.trim_end_matches('.')));
    }
    if !latest.is_empty() && t.s("status") != Some("needs") {
        board::update_task(app, t.id(), fields!["latest" => one_line(&latest.join(" "), 240)])?;
    }
    Ok(ok(Some(&t), None))
}

fn snapshot(r: &Report, t: Option<&Row>, output: &str) -> Result<Value> {
    let mut snap = Row::new();
    let w = t.map(|t| board::task_context(t).get("where").and_then(|v| v.as_object()).cloned().unwrap_or_default()).unwrap_or_default();
    if let Some(t) = t {
        let ctx = board::task_context(t);
        snap.insert(
            "task".into(),
            json!(format!("{} · {}{}", rf("task", t.id()), t.st("title"), if t.i("goal_id").is_some() { "" } else { " (no goal)" })),
        );
        let step = str_list(ctx.get("next")).first().cloned().or_else(|| t.s("latest").map(|s| s.to_string()));
        if let Some(s) = step.filter(|s| !s.is_empty()) {
            snap.insert("step".into(), json!(one_line(&s, 300)));
        }
    }
    snap.insert("terminal".into(), json!(r.name()));
    if let Some(b) = r.git.s("branch").or(w.s("branch")).filter(|s| !s.is_empty()) {
        snap.insert("branch".into(), json!(b));
    }
    if let Some(c) = r.git.s("commit").or(w.s("last_commit")).filter(|s| !s.is_empty()) {
        snap.insert("last_commit".into(), json!(c));
    }
    if let Some(u) = r.git.i("uncommitted").or(w.i("uncommitted")) {
        snap.insert("uncommitted".into(), json!(u));
    }
    if let Some(t) = t {
        if let Some(last) = r.app.db.q1("SELECT text FROM events WHERE task_id = ? AND kind = 'turn' ORDER BY at DESC, id DESC LIMIT 1", p![t.id()])? {
            snap.insert("last_turn".into(), last.v("text"));
        }
    }
    if !output.trim().is_empty() {
        snap.insert("output".into(), json!(clip(output.trim(), 1500)));
    }
    Ok(Value::Object(snap))
}

fn on_found(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let title = one_line(&{ let t = r.b("title"); if t.is_empty() { r.b("text") } else { t } }, 300);
    if title.is_empty() {
        return err(400, "A found report needs a title.");
    }
    let kind = { let k = r.b("kind"); if ISSUE_KINDS.contains(&k.as_str()) { k } else { "bug".into() } };
    let detail = r.b("detail");
    let t = r.task()?;
    let snap = snapshot(r, t.as_ref(), &r.b("output"))?;
    let s = board::get_session(app, r.sid())?;
    let mut goal_id = t.as_ref().and_then(|t| t.i("goal_id"));
    let mut project = t
        .as_ref()
        .and_then(|t| t.s("project").map(|s| s.to_string()))
        .or_else(|| s.as_ref().and_then(|s| s.s("project").map(|x| x.to_string())))
        .or_else(|| r.cwd.as_deref().and_then(base_name));
    if body_has(&r.body, "goal") {
        let g = board::get_goal(app, need_ref(&r.body["goal"], "goal")?)?;
        goal_id = Some(g.id());
        project = g.s("project").map(|s| s.to_string());
    } else if body_has(&r.body, "project") {
        goal_id = None;
        project = Some(r.b("project"));
    }
    let key = norm_title(&title);
    let cands = match goal_id {
        Some(g) => app.db.q("SELECT * FROM issues WHERE state = 'open' AND goal_id = ?", p![g])?,
        None => app.db.q("SELECT * FROM issues WHERE state = 'open' AND goal_id IS NULL AND project IS ?", p![project])?,
    };
    let name = r.name();
    let while_on = t.as_ref().map(|t| format!(" while working on {}", rf("task", t.id()))).unwrap_or_default();
    if let Some(dup) = cands.iter().find(|c| norm_title(&c.st("title")) == key) {
        let mut text = format!("Seen again by {name}{while_on}. Merged here with its own snapshot instead of adding a second issue.");
        if !detail.is_empty() {
            text += &format!(" They said: “{}”", one_line(&detail, 400));
        }
        board::add_issue_event(app, dup.id(), &name, "seen", &text, Some(&snap))?;
        if let Some(t) = &t {
            r.log(
                t.id(),
                "found",
                &format!("Found an issue: {title}. It was already in the backlog as {}, so this sighting was added to it.", rf("issue", dup.id())),
                Some(json!({"issue": dup.id()})),
            )?;
        }
        return Ok(with(ok(t.as_ref(), None), json!({"issue": rf("issue", dup.id()), "seen": true})));
    }
    let now = now_iso();
    let how = match &t {
        Some(t) => format!("The {name} terminal reported it while working on {}.", rf("task", t.id())),
        None => format!("The {name} terminal reported it while not on a task."),
    };
    let said = format!("“{}”", one_line(if detail.is_empty() { &title } else { &detail }, 1000));
    let iid = app.db.insert(
        "issues",
        fields!["goal_id" => goal_id, "project" => project, "kind" => kind, "title" => title,
                "detail" => if detail.is_empty() { None } else { Some(detail.clone()) }, "said" => said, "how" => how,
                "source" => "terminal", "found_by_task" => t.as_ref().map(|t| t.id()), "found_by_session" => r.sid(),
                "found_by_name" => name, "state" => "open", "snapshot" => jdumps(&snap), "created_at" => now, "updated_at" => now],
    )?;
    board::add_issue_event(app, iid, &name, "report", &format!("Reported{while_on}"), Some(&snap))?;
    if let Some(g) = goal_id {
        app.db.x(
            "UPDATE tasks SET ctx_version = COALESCE(ctx_version, 1) + 1 WHERE goal_id = ? AND status != 'done' AND id IS NOT ?",
            p![g, t.as_ref().map(|t| t.id())],
        )?;
    }
    if let Some(t) = &t {
        let place = if goal_id.is_some() { "the goal’s backlog" } else { "the backlog" };
        r.log(
            t.id(),
            "found",
            &format!("Found an issue: {title}. Added to {place} with a snapshot of what the task was doing."),
            Some(json!({"issue": iid})),
        )?;
    }
    Ok(with(ok(t.as_ref(), None), json!({"issue": rf("issue", iid), "seen": false})))
}

fn rewrite(screened: &Value) -> Value {
    match screened.get("question").and_then(|q| q.as_str()) {
        Some(q) => json!({"question": q}),
        None => json!({}),
    }
}

fn answered(r: &Report, t: &Row, text: &str, screened: &Value) -> Result<Value> {
    r.log(t.id(), "question", &format!("Asked: {text}"), None)?;
    let src = screened["source"].as_str().unwrap_or("the rules");
    let ans = screened["answer"].as_str().unwrap_or("");
    board::log_event(r.app, t.id(), board::BOARD, "answer", &format!("Answered from {src} without asking you: {ans}"))?;
    Ok(with(ok(Some(t), None), json!({"answer": ans, "source": src})))
}

fn on_question(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let text = one_line(&r.b("text"), 2000);
    if text.is_empty() {
        return err(400, "A question needs some text.");
    }
    let t = r.task()?;
    let visiting = prflow::visited_by(app, r.sid().unwrap_or(""))?;
    let screened = r.screened.clone().unwrap_or(json!({}));
    let is_answered = screened["verdict"] == "answered";
    if let Some(v) = &visiting {
        if t.as_ref().map(|t| t.id() == v.id()).unwrap_or(true) {
            if is_answered {
                return answered(r, v, &text, &screened);
            }
            let asked = screened["question"].as_str().map(|s| s.to_string()).unwrap_or_else(|| text.clone());
            let wrote = if screened["question"].is_string() { Some(text.as_str()) } else { None };
            if !prflow::stopped(app, v, &r.name(), &asked, true, wrote)? {
                return err(
                    409,
                    format!("{} is done and its PR doesn't need this terminal. Ask {} in the terminal instead.", rf("task", v.id()), app.cfg.owner),
                );
            }
            r.set_session_status("needs")?;
            return Ok(with(ok(Some(v), None), rewrite(&screened)));
        }
    }
    if let Some(t) = &t {
        if t.s("status") != Some("done") && is_answered {
            return answered(r, t, &text, &screened);
        }
    }
    r.set_session_status("needs")?;
    let Some(t) = t else { return Ok(ok(None, None)) };
    if t.s("status") == Some("done") {
        return err(
            409,
            format!("{} is done, so the board can't flag a question on it. Ask {} in the terminal instead.", rf("task", t.id()), app.cfg.owner),
        );
    }
    let asked = screened["question"].as_str().map(|s| s.to_string()).unwrap_or_else(|| text.clone());
    let mut ctx = board::task_context(&t);
    if ctx.remove("step_waiting").is_some() {
        board::save_context(app, t.id(), &ctx, false)?;
    }
    board::update_task(app, t.id(), fields!["status" => "needs", "needs_reason" => "question", "question" => asked, "answered_at" => null, "lost" => 0])?;
    let mut line = format!("Asked: {asked}");
    if asked != text {
        line += &format!(" (The agent wrote: “{text}”)");
    }
    r.log(t.id(), "question", &line, None)?;
    Ok(with(ok(Some(&t), None), rewrite(&screened)))
}

fn on_wait_for(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let Some(t) = r.task()? else { return Ok(ok(None, None)) };
    if t.s("status") == Some("done") {
        return err(409, format!("{} is done, so it has nothing left to wait for.", rf("task", t.id())));
    }
    let tasks = r.body.get("tasks").cloned().unwrap_or(Value::Null);
    if tasks.as_str().map(|s| s.trim().eq_ignore_ascii_case("none")).unwrap_or(false) {
        board::update_task(app, t.id(), fields!["waits_for" => null])?;
        waitsfor::clear_merged(app, &board::get_task(app, t.id())?)?;
        r.log(t.id(), "note", "Waits for nothing any more", None)?;
        return Ok(with(ok(Some(&t), Some("It waits for nothing now.".into())), json!({"parked": false})));
    }
    let Some(wanted) = waitsfor::clean(app, &tasks, Some(&t))? else {
        return err(400, "Say which task this one needs, for example: tb wait-for T14.");
    };
    let mut all: Vec<Value> = waitsfor::ids(&t).into_iter().map(|n| json!(n)).collect();
    all.extend(jloads_arr(Some(&wanted)));
    let merged = waitsfor::clean(app, &Value::Array(all.iter().map(|v| json!(v.to_string())).collect()), Some(&t))?;
    let why = one_line(&r.b("why"), 500);
    board::update_task(app, t.id(), fields!["waits_for" => merged])?;
    let wanted_ids: Vec<i64> = jloads_arr(Some(&wanted)).iter().filter_map(|v| v.as_i64()).collect();
    let until_merged = as_bool(r.body.get("merged"), false);
    if until_merged {
        waitsfor::set_merged(app, &board::get_task(app, t.id())?, &wanted_ids)?;
    }
    let t = board::get_task(app, t.id())?;
    let names = wanted_ids.iter().map(|n| rf("task", *n)).collect::<Vec<_>>().join(", ");
    if waitsfor::blocker(app, &t)?.is_none() {
        let what = if until_merged { "which is merged" } else { "which is ready" };
        r.log(t.id(), "note", &format!("Needs {names}, {what}{}", if why.is_empty() { String::new() } else { format!(": {why}") }), None)?;
        let mut text = waitsfor::bring_in_text(app, &t)?;
        let merged_ids = waitsfor::merged_ids(&t);
        let mut open = vec![];
        for n in &wanted_ids {
            if !merged_ids.contains(n) && !waitsfor::merged(app, board::find_task(app, Some(*n))?.as_ref())? {
                open.push(rf("task", *n));
            }
        }
        if !open.is_empty() {
            text.push_str(&format!(
                "\nIf you can't build on it until its PR merges, run {} wait-for {} --merged instead.",
                board::tb_cmd(app),
                open.join(" ")
            ));
        }
        return Ok(with(ok(Some(&t), Some(text)), json!({"parked": false})));
    }
    let held = t.s("session_id").filter(|s| !s.is_empty()).map(|s| s.to_string());
    waitsfor::park(app, &t, merged.as_deref().unwrap_or("[]"), &r.name(), &why)?;
    let t = board::get_task(app, t.id())?;
    r.set_session_status("idle")?;
    let b = waitsfor::blocker(app, &t)?;
    if let Some(sid) = held.filter(|s| !lines::of(app, s).map(|l| l.is_empty()).unwrap_or(true)) {
        // Its terminal has a line: it waits at the back of it, so it carries on here, not in a new terminal.
        lines::enqueue(app, &t, &sid, lines::Place::Back)?;
        let t = board::get_task(app, t.id())?;
        let next = next_in_line(r, Some(&sid), &format!("{} waits for {names}", rf("task", t.id())))?;
        let moved = next.is_some();
        let text = [b, next].into_iter().flatten().collect::<Vec<_>>().join("\n\n");
        return Ok(with(ok(Some(&t), Some(text)), json!({"parked": true, "moved_on": moved})));
    }
    app.wake_runner();
    Ok(with(ok(Some(&t), b), json!({"parked": true})))
}

pub fn finish_task(app: &App, t: &Row, who: &str, summary: &str, failed: bool, at: Option<&str>) -> Result<Row> {
    let mut text = one_line(summary, 2000);
    if text.is_empty() {
        text = if failed { "No reason given".into() } else { "Done".into() };
    }
    board::update_task(
        app,
        t.id(),
        fields!["status" => "done", "finished_at" => now_iso(), "summary" => text, "failed" => failed as i64, "lost" => 0,
                "needs_reason" => null, "question" => null, "answered_at" => null, "start_job" => null,
                "line_session" => null, "line_pos" => null,
                "latest" => if failed { format!("Stopped: {text}") } else { text.clone() }],
    )?;
    board::log_event_full(app, t.id(), who, "status", &if failed { format!("Failed: {text}") } else { format!("Marked done: {text}") }, None, at)?;
    let t = board::get_task(app, t.id())?;
    deliver::drop(app, t.id(), "the task is done")?;
    if let (Some(gid), false) = (t.i("goal_id"), failed) {
        let have: Vec<String> = board::goal_notes(app, gid)?.iter().map(|n| n.st("text").trim().to_lowercase()).collect();
        for d in str_list(board::task_context(&t).get("decisions")) {
            if !have.contains(&d.trim().to_lowercase()) {
                board::add_goal_note(app, gid, "decision", d.trim(), Some(&rf("task", t.id())), false, None)?;
            }
        }
    }
    if !failed {
        let in_review = prflow::IN_REVIEW.contains(&t.s("pr_phase").unwrap_or("")) && board::pr_still_open(&t);
        let canceled = t.s("no_pr").filter(|w| !w.is_empty() && !app.cfg.jira.canceled.is_empty());
        if let Some(why) = canceled {
            // `tb done --no-pr`: the ticket is withdrawn, with the reason as its comment.
            board::jira_keep_in_step(app, &t, Some(&app.cfg.jira.canceled.clone()), Some(&format!("PR canceled: {why}")))?;
            app.wake_runner();
            return Ok(t);
        }
        let target = if in_review {
            Some(app.cfg.jira.in_review.clone())
        } else if t.i("pr_num").is_none() && !app.cfg.jira.done.is_empty() {
            Some(app.cfg.jira.done.clone())
        } else {
            None
        };
        board::jira_keep_in_step(app, &t, target.as_deref(), Some(&text))?;
    }
    app.wake_runner();
    Ok(t)
}

fn on_done(r: &mut Report) -> Result<Value> {
    r.touch_session(true)?;
    let Some(t) = r.task()? else { return Ok(ok(None, None)) };
    if t.s("status") == Some("done") {
        return Ok(ok(Some(&t), None));
    }
    let summary = { let s = r.b("summary"); if s.is_empty() { r.b("text") } else { s } };
    let pr_arg = r.b("pr");
    let pr = find_pr(&pr_arg).or_else(|| find_pr(&summary));
    if !pr_arg.is_empty() && find_pr(&pr_arg).is_none() {
        return err(400, "That isn't a pull request link (GitHub, GitLab or Bitbucket).");
    }
    let human = r.b("human");
    let human_min = match human.trim() {
        "" => None,
        h => Some(parse_minutes(h).ok_or_else(|| ApiError::new(400, "Give the human estimate as a length of time, like 3h, 90m or 1h30m."))?),
    };
    if let Some(m) = human_min {
        board::update_task(r.app, t.id(), fields!["human_min" => m])?;
    }
    let no_pr = one_line(&r.b("no_pr"), NO_PR_MAX);
    if !no_pr.is_empty() {
        if !pr_arg.is_empty() || has(t.s("pr_url")) {
            return err(400, format!("{} has a PR, so it can't finish without one.", rf("task", t.id())));
        }
        if !projects::task_ships_pr(r.app, &t)? {
            return err(409, format!("{} doesn't end in a PR, so there's none to cancel: finish with tb done \"<summary>\".", rf("task", t.id())));
        }
        crate::stack::refuse_no_pr(r.app, &t)?;
        board::update_task(r.app, t.id(), fields!["no_pr" => no_pr])?;
        r.log(t.id(), "status", &format!("PR canceled: {no_pr}"), None)?;
    }
    let no_evidence = one_line(&r.b("no_evidence"), 1000);
    if !no_evidence.is_empty() {
        board::update_task(r.app, t.id(), fields!["no_evidence" => no_evidence])?;
        r.log(t.id(), "status", &format!("No evidence: {no_evidence}"), None)?;
    }
    let pr = if no_pr.is_empty() { pr } else { None };
    if let Some(pr) = pr {
        prflow::link_pr(r.app, &t, &pr, &r.name())?;
    }
    let t = board::get_task(r.app, t.id())?;
    let at = r.at.clone();
    let held = t.s("session_id").map(|s| s.to_string());
    let t = finish_task(r.app, &t, &r.name(), &summary, false, Some(&at))?;
    let next = next_in_line(r, held.as_deref(), &format!("{} is done", rf("task", t.id())))?;
    Ok(with(ok(Some(&t), next), json!({"pr_url": t.v("pr_url")})))
}

/// Once terminal `held`'s task is done, fails or parks, moves it on to the next task in its line: the
/// text that brings this terminal's agent onto it, or, for another terminal or a report sent late, a
/// message delivered there.
fn next_in_line(r: &Report, held: Option<&str>, why: &str) -> Result<Option<String>> {
    let Some(held) = held.filter(|h| !h.is_empty()) else { return Ok(None) };
    if Some(held) != r.sid() || r.spooled {
        lines::deliver_next(r.app, held, why)?;
        return Ok(None);
    }
    let Some((_, text)) = lines::advance(r.app, held, r.claude.as_deref(), why)? else { return Ok(None) };
    r.set_session_status("working")?;
    Ok(Some(text))
}

/// How long `tb done --no-pr`'s reason may be: one short line.
pub const NO_PR_MAX: usize = 100;

/// What `tb done` refuses before anything opens or finishes: a blank or long `--no-pr`; a task that ends
/// in a PR and has none, finishing without `--pr-body` or `--no-pr` (unless its repo has no remote at all; a
/// remote the board can't read still counts); and a task with a design attached finishing with no evidence
/// and no `--no-evidence`.
fn done_refusals(r: &Report, t: Option<&Row>) -> Result<()> {
    let app = r.app;
    let Some(t) = t.filter(|t| t.s("status") != Some("done")) else { return Ok(()) };
    let me = rf("task", t.id());
    let tb = board::tb_cmd(app);
    if let Some(raw) = r.body.get("no_pr").filter(|v| !v.is_null()) {
        let why = one_line(raw.as_str().unwrap_or(""), 100_000);
        if why.is_empty() {
            return err(400, format!("Say why {me} finishes without its PR: {tb} done \"<summary>\" --no-pr \"<why>\"."));
        }
        if why.chars().count() > NO_PR_MAX {
            return err(400, format!("Keep the --no-pr reason to one short line ({NO_PR_MAX} characters or fewer); put the rest in the summary."));
        }
    }
    let no_pr = !r.b("no_pr").is_empty();
    let summary = { let s = r.b("summary"); if s.is_empty() { r.b("text") } else { s } };
    let has_pr = find_pr(&r.b("pr")).or_else(|| find_pr(&summary)).is_some() || has(t.s("pr_url")) || !r.b("pr_body").is_empty();
    if !no_pr && !has_pr && projects::task_ships_pr(app, t)? && projects::has_remote(app, &t.st("project"))? != Some(false) {
        return err(
            409,
            format!(
                "{me} ends in a PR, and it has none yet. Finish with {tb} done \"<summary>\" --pr-body <file> (the board opens the PR), \
                 --pr <link> if you opened it, or --no-pr \"<why>\" if it won't have one."
            ),
        );
    }
    let atts = board::attachments(app, Some(t.id()), None)?;
    let design = atts.iter().find(|a| a["kind"] == "design");
    let evidence = atts.iter().any(|a| a["kind"] == "evidence" || a["kind"] == "results");
    if let Some(d) = design {
        if !evidence && r.b("no_evidence").is_empty() && !has(t.s("no_evidence")) {
            return err(
                409,
                format!(
                    "{me} has a design attached ({}), so it finishes with evidence that matches it: {tb} attach <screenshot or link> --kind evidence, \
                     or {tb} done \"<summary>\" --no-evidence \"<why>\".",
                    d["title"].as_str().unwrap_or("design")
                ),
            );
        }
    }
    Ok(())
}

/// `task.finishing`'s hooks, before `tb done` (outside the transaction: a hook may call `tb`). A stop is
/// refused back to the agent with the reason, and noted on the task.
fn finishing(r: &Report) -> Result<()> {
    let app = r.app;
    let Some(t) = r.task()? else { return Ok(()) };
    if t.s("status") == Some("done") {
        return Ok(());
    }
    let summary = { let s = r.b("summary"); if s.is_empty() { r.b("text") } else { s } };
    let with_pr = (find_pr(&r.b("pr")).or_else(|| find_pr(&summary)).is_some() || board::pr_still_open(&t)) && r.b("no_pr").trim().is_empty();
    let before: &[steps::Before] = if with_pr { &[steps::Before::Pr, steps::Before::Done] } else { &[steps::Before::Done] };
    let head = r.git.s("sha").filter(|h| !h.is_empty()).map(|h| h.to_string());
    let left = steps::missing_at(app, &t, before, head.as_deref())?;
    if !left.is_empty() {
        if r.spooled {
            let line = format!("Not finished: {} waits for {}", rf("task", t.id()), steps::names(&left));
            app.db.tx(|| crate::dispatch::add_alert(app, &line, Some(t.id()), t.i("goal_id"), None, None).map(|_| ()))?;
        }
        return err(409, steps::refusal_at(app, &t, "finishing", &left, head.as_deref()));
    }
    let here = if r.away(&t)? { None } else { r.cwd.as_deref() };
    if let Some(why) = crate::comments::task_refusal(app, &t, here, "finishing")? {
        return err(409, why);
    }
    let done = json!({"summary": summary, "pr": r.b("pr")});
    let d = hooks::gate(app, "task.finishing", &t, json!({"by": r.name(), "done": done}));
    if d == hooks::Decision::Go {
        return Ok(());
    }
    let line = d.said("Stopped from finishing");
    app.db.tx(|| {
        hooks::note(app, t.id(), &line)?;
        if r.spooled {
            crate::dispatch::add_alert(app, &format!("{}: {line}", rf("task", t.id())), Some(t.id()), t.i("goal_id"), None, None)?;
        }
        Ok(())
    })?;
    err(409, format!("{line}. The task is still open: deal with that, then run tb done again."))
}

/// Is this report from the terminal working on the task (rather than the owner's, elsewhere)?
fn from_agent(r: &Report, t: &Row) -> bool {
    r.sid().is_some() && r.sid() == t.s("session_id")
}

/// `tb step done` / `tb step run`: a step passed, or its check or script failed (`ok: false`).
fn on_step(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let Some(t) = r.task()? else { return Ok(ok(None, None)) };
    let step = steps::find(app, &t, &r.b("name"))?;
    drop_stale_pin(r, &t)?;
    let t = board::get_task(app, t.id())?;
    let tb = board::tb_cmd(app);
    if step.owner && from_agent(r, &t) {
        return err(409, format!("“{}” is {} step: {}.", step.name, app.cfg.owners(), step.how(&tb)));
    }
    if !step.run.is_empty() && r.b("via") != "run" {
        return err(400, format!("“{}” is a script: {}.", step.name, step.how(&tb)));
    }
    let passed = as_bool(r.body.get("ok"), true);
    let note = one_line(&r.b("note"), 2000);
    let output = clip(r.b("output").trim(), 4000);
    let text = match (passed, note.is_empty()) {
        (true, true) => format!("Step done: {}", step.name),
        (true, false) => format!("Step done: {}: {note}", step.name),
        (false, _) => format!("Step didn't pass: {}{}", step.name, if note.is_empty() { String::new() } else { format!(": {note}") }),
    };
    let mut data = json!({"name": step.name, "before": step.before.as_str(), "ok": passed, "note": note, "output": output});
    let head = { let h = r.b("head"); if h.is_empty() { r.git.st("sha") } else { h } };
    if !head.is_empty() {
        data["head"] = json!(head);
    }
    if let Some(res) = steps::clean_result(r.body.get("result")) {
        data["result"] = res;
    }
    r.log(t.id(), "step", &text, Some(data))?;
    board::update_task(app, t.id(), fields!["latest" => one_line(&text, 200)])?;
    let left: Vec<String> = steps::missing(app, &t, &[steps::Before::Pr, steps::Before::Done])?.into_iter().map(|s| s.name).collect();
    Ok(with(ok(Some(&t), None), json!({"step": step.name, "passed": passed, "left": left})))
}

/// `tb step triage`: an answer to one finding of a step's latest round (fixed, answered, dismissed),
/// optionally pointing at the commit that deals with it.
fn on_step_triage(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let Some(t) = r.task()? else { return Ok(ok(None, None)) };
    let step = steps::find(app, &t, &r.b("name"))?;
    let finding = one_line(&r.b("finding"), 80);
    let state = r.b("state").to_lowercase();
    if !steps::TRIAGE_STATES.contains(&state.as_str()) {
        return err(400, format!("A finding is {}.", steps::TRIAGE_STATES.join(", ")));
    }
    let latest = steps::results(app, &t)?.into_iter().find(|x| x["name"] == json!(step.name));
    let known = latest.as_ref().and_then(|l| l["findings"].as_array().cloned()).unwrap_or_default();
    if !known.iter().any(|f| f["id"].as_str() == Some(finding.as_str())) {
        let ids: Vec<String> = known.iter().filter_map(|f| f["id"].as_str().map(|s| s.to_string())).collect();
        return err(
            400,
            if ids.is_empty() {
                format!("“{}”'s last round has no findings to triage.", step.name)
            } else {
                format!("“{finding}” isn't one of “{}”'s findings: {}.", step.name, ids.join(", "))
            },
        );
    }
    let note = one_line(&r.b("note"), 1000);
    let commit = one_line(&r.b("commit"), 64);
    let mut text = format!("{}: {finding} is {state}", step.name);
    if !commit.is_empty() {
        text += &format!(" in {}", commit.chars().take(12).collect::<String>());
    }
    if !note.is_empty() {
        text += &format!(": {note}");
    }
    r.log(t.id(), "step_triage", &text, Some(json!({"name": step.name, "finding": finding, "state": state, "note": note, "commit": commit})))?;
    let t = board::get_task(app, t.id())?;
    let open = steps::results(app, &t)?.into_iter().find(|x| x["name"] == json!(step.name)).map(|x| x["open"].clone()).unwrap_or(json!(0));
    Ok(with(ok(Some(&t), None), json!({"step": step.name, "finding": finding, "state": state, "open": open})))
}

/// `tb step aim`: saves where the task's rounds look (a worktree, a branch, a pinned commit), so later
/// rounds and the gates follow it; `clear` drops it.
fn on_step_aim(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let Some(t) = r.task()? else { return Ok(ok(None, None)) };
    let mut ctx = board::task_context(&t);
    let mut aim: serde_json::Map<String, Value> = ["worktree", "branch", "sha", "tip"]
        .iter()
        .filter_map(|k| Some((k.to_string(), json!(one_line(r.body.get(*k)?.as_str()?, 400)))))
        .filter(|(_, v)| v.as_str().is_some_and(|s| !s.is_empty()))
        .collect();
    // A pin keeps its branch's tip, so it's dropped once the branch moves on (`steps::aim_now`).
    match (aim.contains_key("sha"), aim.get("branch").and_then(|b| b.as_str()).map(|b| b.to_string())) {
        (false, _) => {
            aim.remove("tip");
        }
        (true, Some(b)) if !aim.contains_key("tip") => {
            if let Some(tip) = steps::branch_tip(&t, &Value::Object(aim.clone()), &b) {
                aim.insert("tip".into(), json!(tip));
            }
        }
        _ => {}
    }
    let text = if as_bool(r.body.get("clear"), false) || aim.is_empty() {
        if ctx.remove("step_aim").is_none() {
            return Ok(with(ok(Some(&t), None), json!({"aim": null})));
        }
        "Rounds look at the agent's checkout again".to_string()
    } else {
        let mut bits = vec![];
        if let Some(b) = aim.get("branch").and_then(|v| v.as_str()) {
            bits.push(b.to_string());
        }
        if let Some(s) = aim.get("sha").and_then(|v| v.as_str()) {
            bits.push(format!("at {}", s.chars().take(12).collect::<String>()));
        }
        if let Some(w) = aim.get("worktree").and_then(|v| v.as_str()) {
            bits.push(format!("in {w}"));
        }
        ctx.insert("step_aim".into(), Value::Object(aim.clone()));
        format!("Rounds aimed at {}", bits.join(" "))
    };
    board::save_context(app, t.id(), &ctx, false)?;
    r.log(t.id(), "status", &text, None)?;
    let t = board::get_task(app, t.id())?;
    Ok(with(ok(Some(&t), None), json!({"aim": steps::aim_now(&t), "head": steps::aim_head(&t)})))
}

/// Drops a saved pin once its branch has moved on (`steps::aim_now`), with a line on the task saying so.
fn drop_stale_pin(r: &mut Report, t: &Row) -> Result<()> {
    let now = steps::aim_now(t);
    let Some(sha) = now["dropped"].as_str() else { return Ok(()) };
    let mut ctx = board::task_context(t);
    let mut kept = now.clone();
    if let Some(o) = kept.as_object_mut() {
        o.remove("dropped");
    }
    let branch = now["branch"].as_str().unwrap_or("its branch").to_string();
    if kept.as_object().is_some_and(|o| o.is_empty()) {
        ctx.remove("step_aim");
    } else {
        ctx.insert("step_aim".into(), kept);
    }
    board::save_context(r.app, t.id(), &ctx, false)?;
    let short: String = sha.chars().take(12).collect();
    r.log(t.id(), "status", &format!("Rounds no longer pinned at {short}: {branch} has moved on since"), None)?;
    Ok(())
}

/// `tb step publish`: a step's publish script ran for the head it passed on (after a rebase's push, say).
fn on_step_publish(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let Some(t) = r.task()? else { return Ok(ok(None, None)) };
    let step = steps::find(app, &t, &r.b("name"))?;
    if step.publish.trim().is_empty() {
        return err(400, format!("“{}” has nothing to publish.", step.name));
    }
    let passed = as_bool(r.body.get("ok"), true);
    let head = r.b("head");
    let short: String = head.chars().take(12).collect();
    let at = if short.is_empty() { String::new() } else { format!(" for {short}") };
    let text = if passed { format!("Published {}{at}", step.name) } else { format!("Couldn't publish {}{at}", step.name) };
    let output = clip(r.b("output").trim(), 4000);
    r.log(t.id(), "status", &text, Some(json!({"name": step.name, "publish": true, "ok": passed, "head": head, "output": output})))?;
    Ok(with(ok(Some(&t), None), json!({"step": step.name, "passed": passed, "head": head})))
}

/// Puts the task in Needs you for a step, as a question the owner answers or acknowledges.
fn wait_on_owner(r: &Report, t: &Row, step: &steps::Step, question: &str, failed: bool) -> Result<()> {
    let app = r.app;
    let mut ctx = board::task_context(t);
    ctx.insert("step_waiting".into(), json!(step.name));
    ctx.insert("step_failed".into(), json!(failed));
    board::save_context(app, t.id(), &ctx, false)?;
    board::update_task(app, t.id(), fields!["status" => "needs", "needs_reason" => "question", "question" => question, "answered_at" => null, "lost" => 0])?;
    r.log(t.id(), "question", &format!("Asked: {question}"), None)?;
    r.set_session_status("needs")?;
    crate::dispatch::add_alert(app, &format!("{}: {question}", rf("task", t.id())), Some(t.id()), t.i("goal_id"), None, None)
}

/// `tb step ask`: the agent reached one of the owner's steps; the task waits for them.
fn on_step_ask(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let Some(t) = r.task()? else { return Ok(ok(None, None)) };
    let step = steps::find(app, &t, &r.b("name"))?;
    if !step.owner {
        return err(400, format!("“{}” is yours to do: {}.", step.name, step.how(&board::tb_cmd(app))));
    }
    if steps::recorded(app, t.id())?.contains(&steps::key(&step.name)) {
        return Ok(with(ok(Some(&t), None), json!({"step": step.name, "already": true})));
    }
    // The head, branch and worktree tb resolved (as for tb step done): kept for the take handoff too.
    let given: serde_json::Map<String, Value> = ["head", "branch", "worktree"]
        .iter()
        .map(|k| (k.to_string(), r.b(k).trim().to_string()))
        .filter(|(_, x)| !x.is_empty())
        .map(|(k, x)| (k, json!(x)))
        .collect();
    let t = if given.is_empty() {
        t
    } else {
        let mut ctx = board::task_context(&t);
        ctx.insert("step_asked".into(), Value::Object(given.clone()));
        board::save_context(app, t.id(), &ctx, false)?;
        board::get_task(app, t.id())?
    };
    let mut vars = steps::vars_at(app, &t, given.get("head").and_then(|h| h.as_str()));
    for (k, x) in &given {
        vars.insert(k.clone(), x.as_str().unwrap_or_default().to_string());
    }
    let s = step.filled(&vars);
    let mut q = format!("Step “{}”", s.name);
    if !s.prompt.trim().is_empty() {
        q += &format!(": {}", s.prompt.trim().trim_end_matches('.'));
    }
    if !s.open.is_empty() {
        q += &format!(" (at {})", s.open);
    }
    wait_on_owner(r, &t, &step, &format!("{q}. Press Done once it's done."), false)?;
    Ok(with(ok(Some(&t), None), json!({"step": step.name, "open": s.open})))
}

/// `tb step fail`: a step can't pass; the task waits for the owner's answer (or their skip).
fn on_step_fail(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let Some(t) = r.task()? else { return Ok(ok(None, None)) };
    let step = steps::find(app, &t, &r.b("name"))?;
    let why = one_line(&r.b("why"), 2000);
    if why.is_empty() {
        return err(400, "Say why the step can't pass (--why).");
    }
    wait_on_owner(r, &t, &step, &format!("Step “{}” can't pass: {why}. Answer, or skip the step.", step.name), true)?;
    Ok(with(ok(Some(&t), None), json!({"step": step.name})))
}

fn on_fail(r: &mut Report) -> Result<Value> {
    r.touch_session(true)?;
    let Some(t) = r.task()? else { return Ok(ok(None, None)) };
    if t.s("status") == Some("done") {
        return Ok(ok(Some(&t), None));
    }
    let reason = { let s = r.b("reason"); if s.is_empty() { r.b("text") } else { s } };
    let at = r.at.clone();
    let held = t.s("session_id").map(|s| s.to_string());
    let t = finish_task(r.app, &t, &r.name(), &reason, true, Some(&at))?;
    let next = next_in_line(r, held.as_deref(), &format!("{} stopped", rf("task", t.id())))?;
    Ok(ok(Some(&t), next))
}

fn on_take(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    if !body_has(&r.body, "task") {
        return err(400, "Say which task to take, for example: tb take T12.");
    }
    let t = board::get_task(app, need_ref(&r.body["task"], "task")?)?;
    let Some(sid) = r.sid.clone() else { return err(400, "tb take only works inside a Midna terminal.") };
    let reopening = t.s("status") == Some("done") && board::pr_still_open(&t);
    let already = t.s("session_id") == Some(sid.as_str()) && matches!(t.s("status"), Some("working") | Some("needs")) && !t.b("lost");
    if !already {
        let how = if reopening { format!("Taken back for PR #{}", t.i0("pr_num")) } else { "Taken".into() };
        if let Err(why) = board::claim(app, &t, &sid, r.claude.as_deref(), None, &how, true)? {
            return err(409, why);
        }
    }
    if reopening {
        crate::dispatch::clear_alerts(app, Some(t.id()), None)?;
    }
    let t = board::get_task(app, t.id())?;
    // Lent before the handoff is built, so it names them, the same as a runner start.
    crate::devices::lend(app, &t)?;
    let c = r.handoff_for(&t)?;
    Ok(ok(Some(&t), Some(c)))
}

/// Why the terminal can't leave `t` for another task now: it's waiting on the owner.
fn cant_switch(t: &Row) -> Option<String> {
    let tr = rf("task", t.id());
    (t.s("status") == Some("needs") && t.s("needs_reason") == Some("question"))
        .then(|| format!("{tr} waits here for the owner's answer (a question or a step); switch once that's in."))
}

/// `tb switch T<n>`: this terminal takes a task from its line; the one it's on waits here to resume.
fn on_switch(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let Some(sid) = r.sid.clone() else { return err(400, "tb switch only works inside a Midna terminal.") };
    if !body_has(&r.body, "to") {
        return err(400, "Say which task to switch to, for example: tb switch T12.");
    }
    let t = board::get_task(app, need_ref(&r.body["to"], "task")?)?;
    let tr = rf("task", t.id());
    if t.s("line_session") != Some(sid.as_str()) || t.s("status") != Some("queued") {
        return err(409, format!("{tr} isn't in this terminal's line. tb line shows what is; tb take {tr} takes any other task."));
    }
    let cur = board::task_for_session(app, Some(&sid))?;
    if let Some(cur) = &cur {
        if let Some(why) = cant_switch(cur) {
            return err(409, why);
        }
        r.update_where(cur, false)?;
    }
    if let Err(why) = board::claim(app, &t, &sid, r.claude.as_deref(), None, "Switched to", false)? {
        return err(409, why);
    }
    let t = board::get_task(app, t.id())?;
    crate::devices::lend(app, &t)?;
    waitsfor::started(app, &t)?;
    let mut c = r.handoff_for(&board::get_task(app, t.id())?)?;
    if let Some(cur) = &cur {
        c = format!("{} waits here to resume once {tr} is done.\n\n{c}", rf("task", cur.id()));
    }
    Ok(ok(Some(&t), Some(c)))
}

/// `tb line`: this terminal's line; with `drop`, a task out of it and back to the board.
fn on_line(r: &mut Report) -> Result<Value> {
    let app = r.app;
    r.touch_session(true)?;
    let Some(sid) = r.sid.clone() else { return err(400, "tb line only works inside a Midna terminal.") };
    if body_has(&r.body, "drop") {
        let t = board::get_task(app, need_ref(&r.body["drop"], "task")?)?;
        let tr = rf("task", t.id());
        if t.s("line_session") != Some(sid.as_str()) {
            return err(409, format!("{tr} isn't in this terminal's line."));
        }
        lines::drop(app, &t, &r.name(), &format!("Taken out of {}'s line; waits for Start", r.name()))?;
        return Ok(with(ok(None, Some(format!("{tr} is back on the board; it waits for Start."))), json!({"line": lines::entries(app, &sid)?})));
    }
    let mut parts = vec![];
    if let Some(t) = board::task_for_session(app, Some(&sid))? {
        parts.push(format!("On {} “{}”.", rf("task", t.id()), t.st("title")));
    }
    let s = lines::summary(app, &sid)?;
    parts.push(if s.is_empty() { "Nothing waits in this terminal's line.".into() } else { s });
    Ok(with(ok(None, Some(parts.join(" "))), json!({"line": lines::entries(app, &sid)?})))
}

fn project_for(r: &Report, given: &str) -> Result<Option<String>> {
    if !given.trim().is_empty() {
        return Ok(Some(given.trim().to_string()));
    }
    let s = board::get_session(r.app, r.sid())?;
    Ok(s.and_then(|s| s.s("project").map(|x| x.to_string())).or_else(|| r.cwd.as_deref().and_then(base_name)))
}

static PLANNED_WITH_FILES: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)^(.*)::\s*(\d*)\s*::\s*([#Tt0-9,\s]*)::([^:]*)$").unwrap());
static PLANNED_WITH_REFS: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)^(.*)::\s*(\d*)\s*::\s*([#Tt0-9,\s]*)$").unwrap());
static PLANNED_WITH_WAVE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)^(.*)::\s*(\d+)\s*$").unwrap());
static EARLIER_REF: Lazy<Regex> = Lazy::new(|| Regex::new(r"^#(\d+)$").unwrap());

/// "title::detail", "title::detail::2" (its wave) or "title::detail::<wave or nothing>::T14 #1" (what it
/// waits for: a task, or the k-th task in the same command).
/// A `--task` item's files part ("::src/a.rs, src/b.rs" after the waits), the files the plan gives it.
fn planned_files(item: &str) -> (String, Value) {
    let (title, detail) = item.split_once("::").unwrap_or((item, ""));
    match PLANNED_WITH_FILES.captures(detail) {
        Some(c) => {
            let files: Vec<&str> = c[4].split(',').map(str::trim).filter(|f| !f.is_empty()).collect();
            (format!("{title}::{}::{}::{}", &c[1], &c[2], &c[3]), if files.is_empty() { Value::Null } else { json!(files) })
        }
        None => (item.to_string(), Value::Null),
    }
}

/// The files a wave plans for a task (`files` on a planned item: a list, or a comma-separated string), kept in its
/// context as `plan_files` for its wave mates' handoffs.
fn clean_files(v: Option<&Value>) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let items: Vec<String> = match v {
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        Some(Value::String(s)) => s.split(',').map(str::to_string).collect(),
        _ => vec![],
    };
    for f in items {
        let f = one_line(f.trim(), 200);
        if !f.is_empty() && !out.contains(&f) && out.len() < 30 {
            out.push(f);
        }
    }
    out
}

fn split_planned(item: &str) -> (String, String, Value, Value) {
    let (title, detail) = item.split_once("::").unwrap_or((item, ""));
    if let Some(c) = PLANNED_WITH_REFS.captures(detail) {
        let wave = c[2].parse::<i64>().map(|n| json!(n)).unwrap_or(Value::Null);
        let waits: Vec<String> = c[3].replace(',', " ").split_whitespace().map(|x| x.to_string()).collect();
        return (title.into(), c[1].into(), wave, if waits.is_empty() { Value::Null } else { json!(waits) });
    }
    if let Some(c) = PLANNED_WITH_WAVE.captures(detail) {
        return (title.into(), c[1].into(), json!(c[2].parse::<i64>().unwrap_or(0)), Value::Null);
    }
    (title.into(), detail.into(), Value::Null, Value::Null)
}

/// `#k` in what a planned task waits for: the k-th task made earlier in the same command.
fn earlier_refs(value: &Value, created: &[String]) -> Result<Value> {
    let items: Vec<String> = match value {
        Value::Null => return Ok(Value::Null),
        Value::Array(a) => a.iter().map(|v| v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string())).collect(),
        Value::String(s) => s.replace(',', " ").split_whitespace().map(|x| x.to_string()).collect(),
        v => vec![v.to_string()],
    };
    let mut out = vec![];
    for v in items {
        match EARLIER_REF.captures(v.trim()) {
            Some(c) => {
                let k: usize = c[1].parse().unwrap_or(0);
                if k < 1 || k > created.len() {
                    return err(400, format!("#{k} has to point at a task listed before it in the same command; #1 is the first."));
                }
                out.push(json!(created[k - 1]));
            }
            None => out.push(json!(v)),
        }
    }
    Ok(json!(out))
}

fn planned(r: &Report, g: &Row, items: &Value, warnings: &mut Vec<String>) -> Result<Vec<String>> {
    let mut created: Vec<String> = vec![];
    for item in items.as_array().cloned().unwrap_or_default() {
        let (item, files) = match &item {
            Value::String(s) => {
                let (rest, files) = planned_files(s);
                (Value::String(rest), files)
            }
            v => (v.clone(), v.get("files").cloned().unwrap_or(Value::Null)),
        };
        let (title, detail, wave, waits) = match &item {
            Value::String(s) => split_planned(s),
            Value::Object(o) => (
                o.s("title").unwrap_or("").to_string(),
                o.s("detail").unwrap_or("").to_string(),
                o.get("wave").cloned().unwrap_or(Value::Null),
                o.get("waits_for").cloned().unwrap_or(Value::Null),
            ),
            _ => continue,
        };
        let title = one_line(&title, 300);
        if title.is_empty() {
            continue;
        }
        let c = ops::new_task(
            r.app,
            &json!({"title": title, "detail": detail.trim(), "project": g.v("project"), "goal_id": g.id(),
                    "status": "planned", "pickup": {"mode": "queue"}, "also": item.get("also"), "wave": wave,
                    "waits_for": earlier_refs(&waits, &created)?, "locks": item.get("locks"), "alone": item.get("alone"),
                    "jira": item.get("jira"),
                    "stack_on": item.get("stack_on"), "ships_pr": item.get("ships_pr"),
                    "devices": item.get("devices"), "bits": item.get("bits"),
                    "origin": {"from": format!("Planned in {}", rf("goal", g.id())), "by": r.name()}}),
            &r.name(),
            Some(&format!("Planned by {}", r.name())),
        )?;
        created.push(c["ref"].as_str().unwrap_or("").to_string());
        let files = clean_files(Some(&files));
        if let (false, Some(tid)) = (files.is_empty(), c["id"].as_i64()) {
            let mut ctx = board::task_context(&board::get_task(r.app, tid)?);
            ctx.insert("plan_files".into(), json!(files));
            board::save_context(r.app, tid, &ctx, false)?;
        }
        for w in c["warnings"].as_array().cloned().unwrap_or_default() {
            if let Some(w) = w.as_str().map(|w| w.to_string()) {
                if !warnings.contains(&w) {
                    warnings.push(w);
                }
            }
        }
    }
    Ok(created)
}

fn on_propose(r: &mut Report) -> Result<Value> {
    r.touch_session(true)?;
    let g = board::get_goal(r.app, need_ref(&r.body["goal"], "goal")?)?;
    let tasks = r.body.get("tasks").cloned().unwrap_or(json!([]));
    let mut warnings = vec![];
    let created = planned(r, &g, &tasks, &mut warnings)?;
    Ok(with(ok(None, None), json!({"created": created, "goal": rf("goal", g.id()), "warnings": warnings})))
}

fn on_goal(r: &mut Report) -> Result<Value> {
    r.touch_session(true)?;
    let project = project_for(r, &r.b("project"))?;
    let g = ops::new_goal(
        r.app,
        &json!({"name": r.body.get("name"), "outcome": r.b("outcome"), "tldr": r.b("tldr"), "project": project, "product": r.body.get("product")}),
    )?;
    let gid = g["id"].as_i64().unwrap_or(0);
    let tasks = r.body.get("tasks").cloned().unwrap_or(json!([]));
    let mut warnings = vec![];
    let created = planned(r, &board::get_goal(r.app, gid)?, &tasks, &mut warnings)?;
    Ok(with(ok(None, None), json!({"goal": g["ref"], "name": g["name"], "project": g["project"], "created": created, "warnings": warnings})))
}

fn on_new_task(r: &mut Report) -> Result<Value> {
    r.touch_session(true)?;
    let title = one_line(&r.b("title"), 300);
    if title.is_empty() {
        return err(400, "A task needs a title.");
    }
    if as_bool(r.body.get("here"), false) {
        return new_task_here(r, &title);
    }
    if body_has(&r.body, "goal") {
        let g = board::get_goal(r.app, need_ref(&r.body["goal"], "goal")?)?;
        let item = json!([{"title": title, "detail": r.b("detail"), "also": r.body.get("also"), "wave": r.body.get("wave"),
                           "waits_for": r.body.get("waits_for"), "locks": r.body.get("locks"), "alone": r.body.get("alone"),
                           "jira": r.body.get("jira"), "files": r.body.get("files"),
                           "stack_on": r.body.get("stack_on"), "ships_pr": r.body.get("ships_pr"),
                           "devices": r.body.get("devices"), "bits": r.body.get("bits")}]);
        let mut warnings = vec![];
        let created = planned(r, &g, &item, &mut warnings)?;
        note_made(r, &created)?;
        return Ok(with(ok(None, None), json!({"created": created, "goal": rf("goal", g.id()), "status": "planned", "warnings": warnings})));
    }
    if r.body.get("also").map(|a| !a.is_null() && a != &json!([])).unwrap_or(false) {
        return err(400, "Only a task in a goal can also finish other goals. Give it its home goal with --goal G<n>.");
    }
    let project = project_for(r, &r.b("project"))?;
    let planned_flag = as_bool(r.body.get("planned"), false);
    let c = ops::new_task(
        r.app,
        &json!({"title": title, "detail": r.b("detail"), "project": project, "pickup": {"mode": "manual"},
                "status": if planned_flag { "planned" } else { "queued" },
                "waits_for": r.body.get("waits_for"), "locks": r.body.get("locks"), "alone": r.body.get("alone"),
                "jira": r.body.get("jira"),
                "stack_on": r.body.get("stack_on"), "ships_pr": r.body.get("ships_pr"),
                "devices": r.body.get("devices"), "bits": r.body.get("bits"),
                "origin": {"from": "Added by an agent", "by": r.name()}}),
        &r.name(),
        Some(&format!("Added by {}; waits for you to press Start", r.name())),
    )?;
    note_made(r, &[c["ref"].as_str().unwrap_or("").to_string()])?;
    Ok(with(ok(None, None), json!({"created": [c["ref"]], "status": c["status"], "project": c["project"],
                                   "warnings": c.get("warnings").cloned().unwrap_or(json!([]))})))
}

/// Notes on this terminal that its conversation made these tasks, for `tb start` on an unnamed "queue it".
fn note_made(r: &Report, refs: &[String]) -> Result<()> {
    let Some(sid) = r.sid() else { return Ok(()) };
    for id in refs.iter().filter_map(|t| t.trim_start_matches('T').parse::<i64>().ok()) {
        crate::startword::note_made(r.app, sid, id)?;
    }
    Ok(())
}

/// `tb task new … --here`: a standalone task for the work this terminal is already doing, claimed at once.
fn new_task_here(r: &mut Report, title: &str) -> Result<Value> {
    let app = r.app;
    let Some(sid) = r.sid.clone() else { return err(400, "--here only works inside a Midna terminal.") };
    if body_has(&r.body, "goal") || r.body.get("also").map(|a| !a.is_null() && a != &json!([])).unwrap_or(false) {
        return err(400, "--here makes a standalone task on this terminal; leave out --goal and --also.");
    }
    let place = r.b("line");
    let current = board::task_for_session(app, Some(&sid))?;
    if let Some(mine) = &current {
        let mr = rf("task", mine.id());
        if place.is_empty() {
            return err(
                409,
                format!(
                    "This terminal is already on {mr} “{}”; its work is tracked there. If this is separate work, add it to \
                     this terminal's line: --here --next (after {mr}), or --here --now (switch to it; {mr} waits here to resume).",
                    mine.st("title")
                ),
            );
        }
        if place == "now" {
            if let Some(why) = cant_switch(mine) {
                return err(409, why);
            }
            r.update_where(mine, false)?;
        }
    }
    let name = r.name();
    let owner = app.cfg.owner.clone();
    let project = project_for(r, &r.b("project"))?;
    let c = ops::new_task(
        app,
        &json!({"title": title, "detail": r.b("detail").trim(), "project": project, "pickup": {"mode": "manual"},
                "jira": r.body.get("jira"),
                "stack_on": r.body.get("stack_on"), "ships_pr": r.body.get("ships_pr"),
                "origin": {"from": format!("Code changed in {name} while {owner} worked there"), "by": board::OWNER}}),
        &name,
        Some(&format!("Added by {name} for the code it changed with {owner}")),
    )?;
    let t = board::get_task(app, c["id"].as_i64().unwrap_or(0))?;
    if let (Some(mine), "next") = (&current, place.as_str()) {
        lines::enqueue(app, &t, &sid, lines::Place::Back)?;
        board::log_event(app, t.id(), &name, "status", &format!("Queued in {name} after {}", rf("task", mine.id())))?;
        let (tr, mr) = (rf("task", t.id()), rf("task", mine.id()));
        let text = format!("{tr} is queued in this terminal after {mr}; it starts here once {mr} is done. Carry on with {mr}.");
        return Ok(with(ok(Some(mine), Some(text)), json!({"created": [tr], "status": "queued", "project": c["project"]})));
    }
    if let Err(why) = board::claim(app, &t, &sid, r.claude.as_deref(), None, "Tracked", false)? {
        return err(409, why);
    }
    let t = board::get_task(app, t.id())?;
    r.sent(&t)?;
    let tr = rf("task", t.id());
    let mut text = format!(
        "Task board: you're on {tr} now, and the board tracks this terminal's work on it. Keep working with {owner}; \
         follow the task-board skill's “When you're on a task”, and finish with tb done when the work is."
    );
    if let Some(mine) = &current {
        text.push_str(&format!(" {} waits here to resume once {tr} is done.", rf("task", mine.id())));
    }
    Ok(with(ok(Some(&t), Some(text)), json!({"created": [tr], "status": "working", "project": c["project"]})))
}

fn on_attach(r: &mut Report) -> Result<Value> {
    r.touch_session(true)?;
    let (url, title, kind) = (r.b("url"), r.b("title"), r.b("kind"));
    if body_has(&r.body, "goal") {
        let g = board::get_goal(r.app, need_ref(&r.body["goal"], "goal")?)?;
        let a = board::add_attachment(r.app, &r.name(), &url, &title, &kind, None, Some(g.id()))?;
        return Ok(with(ok(None, None), json!({"attachment": a, "goal": rf("goal", g.id())})));
    }
    let Some(t) = r.task()? else {
        return err(400, "No task on this terminal. Name one with --task T<n>, or a goal with --goal G<n>.");
    };
    let a = board::add_attachment(r.app, &r.name(), &url, &title, &kind, Some(t.id()), None)?;
    Ok(with(ok(Some(&t), None), json!({"attachment": a})))
}

/// `tb unattach`: remove the task's (or, with `goal`, the goal's) attachment whose link, path or
/// title is `url`; the newest one when several match.
fn on_unattach(r: &mut Report) -> Result<Value> {
    r.touch_session(true)?;
    let what = r.b("url");
    let (col, owner, task, goal) = if body_has(&r.body, "goal") {
        let g = board::get_goal(r.app, need_ref(&r.body["goal"], "goal")?)?;
        ("goal_id", g.id(), None, Some(g))
    } else {
        let Some(t) = r.task()? else {
            return err(400, "No task on this terminal. Name one with --task T<n>, or a goal with --goal G<n>.");
        };
        ("task_id", t.id(), Some(t), None)
    };
    let sql = format!("SELECT * FROM attachments WHERE {col} = ? AND removed_at IS NULL AND (url = ? OR title = ?) ORDER BY id DESC");
    let Some(a) = r.app.db.q1(&sql, p![owner, what, what])? else {
        return err(404, "Nothing attached there has that link, path or title.");
    };
    r.app.db.x("UPDATE attachments SET removed_at = ? WHERE id = ?", p![now_iso(), a.id()])?;
    if let Some(t) = &task {
        board::log_event(r.app, t.id(), &r.name(), "note", &format!("Removed the attachment {}", a.st("title")))?;
    }
    let goal = goal.map(|g| json!(rf("goal", g.id()))).unwrap_or(Value::Null);
    Ok(with(ok(task.as_ref(), None), json!({"removed": a.st("title"), "goal": goal})))
}

fn on_hello(r: &mut Report) -> Result<Value> {
    r.touch_session(true)?;
    store_plugin(r)?;
    Ok(ok(None, None))
}

fn on_delivered(r: &mut Report) -> Result<Value> {
    r.touch_session(true)?;
    let ids = r.body.get("ids").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    deliver::delivered(r.app, r.sid(), &ids)?;
    Ok(ok(None, None))
}

fn on_status(r: &mut Report) -> Result<Value> {
    r.touch_session(true)?;
    let Some(t) = r.task()? else { return Ok(ok(None, Some("No task on this terminal.".into()))) };
    let label = if t.b("failed") {
        "Failed".to_string()
    } else {
        match t.s("status") {
            Some("planned") => "Planned".into(),
            Some("queued") => "Queued".into(),
            Some("working") => "Working".into(),
            Some("needs") => "Needs you".into(),
            Some("done") => "Done".into(),
            other => other.unwrap_or("").to_string(),
        }
    };
    let g = board::find_goal(r.app, t.i("goal_id"))?;
    let mut line = format!("{} · {} · {label}", rf("task", t.id()), t.st("title"));
    if let Some(sid) = r.sid().filter(|s| t.s("session_id") == Some(*s)) {
        let s = lines::summary(r.app, sid)?;
        if !s.is_empty() {
            line = format!("{line}\n{s}");
        }
    }
    Ok(with(
        ok(Some(&t), Some(line)),
        json!({"title": t.v("title"), "status": t.v("status"), "jira_key": t.v("jira_key"), "pr_url": t.v("pr_url"),
               "goal": g.map(|g| g.v("name")).unwrap_or(Value::Null)}),
    ))
}

const START_TEXT: &[(&str, &str)] =
    &[("startup", "Started a Claude session"), ("resume", "Resumed an earlier conversation"), ("clear", "Cleared its conversation and started again")];

fn session_history(r: &Report, out: &Value) -> Option<(&'static str, String)> {
    let b = &r.body;
    match r.event.as_str() {
        "hook.session_start" => {
            let src = r.b("source");
            START_TEXT.iter().find(|(k, _)| *k == src).map(|(_, v)| ("start", v.to_string()))
        }
        "hook.prompt" => {
            let text = LEADING_MARKER_RE.replace(b["prompt"].as_str().unwrap_or(""), "").trim().to_string();
            if text.is_empty() || text.starts_with('<') {
                None
            } else {
                Some(("prompt", text))
            }
        }
        "hook.stop" => {
            let t = r.b("last_message");
            if t.is_empty() {
                None
            } else {
                Some(("reply", t))
            }
        }
        "hook.pre_compact" => Some((
            "compact",
            format!("Compacted its conversation{}", if r.b("trigger") == "manual" { " (you asked)" } else { "" }),
        )),
        "hook.commit" => {
            let cmd = r.b("command");
            if cmd.contains("git push") {
                Some(("commit", format!("Pushed {}", r.git.s("branch").filter(|s| !s.is_empty()).unwrap_or("the branch"))))
            } else {
                Some(("commit", format!("Committed: {}", commit_subject(r, &r.b("output")))))
            }
        }
        "hook.api_error" => Some(("wait", api_error_of(r).1)),
        "hook.attention" => {
            if r.waiting_on_background {
                return None;
            }
            let what = match r.b("notification_type").as_str() {
                "permission_prompt" => "Waiting for permission",
                "idle_prompt" => "Waiting for your reply",
                _ => "Waiting for you",
            };
            let msg = r.b("message");
            Some(("wait", if msg.is_empty() { what.to_string() } else { format!("{what}: {msg}") }))
        }
        "hook.session_end" => {
            let reason = r.b("reason");
            Some((
                "end",
                format!("The Claude session ended{}", if reason.is_empty() || reason == "other" { String::new() } else { format!(" ({reason})") }),
            ))
        }
        "tb.found" => Some(("found", format!("Reported an issue: {}", r.b("title")))),
        "tb.checkpoint" => {
            let done = str_list(b.get("done"));
            Some((
                "checkpoint",
                format!("Checkpoint saved{}", if done.is_empty() { String::new() } else { format!(": {}", done.iter().take(2).cloned().collect::<Vec<_>>().join("; ")) }),
            ))
        }
        "tb.question" => {
            if let Some(a) = out.get("source").and_then(|s| s.as_str()) {
                Some(("ask", format!("Asked, and the board answered it from {a}: {}", r.b("text"))))
            } else {
                Some(("ask", format!("Asked you: {}", out.get("question").and_then(|q| q.as_str()).map(|s| s.to_string()).unwrap_or_else(|| r.b("text")))))
            }
        }
        "tb.done" => Some(("done", format!("Finished {}: {}", out["task"].as_str().unwrap_or("its task"), r.b("summary")))),
        "tb.step" if out["passed"] == false => out["step"].as_str().map(|s| ("step", format!("The step {s} didn't pass"))),
        "tb.step" => out["step"].as_str().map(|s| ("step", format!("Did the step {s}"))),
        "tb.step_ask" => out["step"].as_str().map(|s| ("ask", format!("Asked you to do the step {s}"))),
        "tb.step_fail" => out["step"].as_str().map(|s| ("ask", format!("The step {s} can't pass: {}", r.b("why")))),
        "tb.fail" => Some(("fail", format!("Gave up on {}: {}", out["task"].as_str().unwrap_or("its task"), r.b("reason")))),
        "tb.take" => out["task"].as_str().map(|t| ("take", format!("Picked up {t}"))),
        "tb.switch" => out["task"].as_str().map(|t| ("take", format!("Switched to {t}"))),
        _ => None,
    }
}

type Handler = fn(&mut Report) -> Result<Value>;

fn handler(event: &str) -> Option<Handler> {
    Some(match event {
        "hook.session_start" => on_session_start,
        "hook.prompt" => on_prompt,
        "hook.stop" => on_stop,
        "hook.pre_compact" => on_pre_compact,
        "hook.commit" => on_commit,
        "hook.session_end" => on_session_end,
        "hook.attention" => on_attention,
        "hook.api_error" => on_api_error,
        "hook.delivered" => on_delivered,
        "tb.note" => on_note,
        "tb.checkpoint" => on_checkpoint,
        "tb.found" => on_found,
        "tb.question" => on_question,
        "tb.wait_for" => on_wait_for,
        "tb.done" => on_done,
        "tb.fail" => on_fail,
        "tb.step" => on_step,
        "tb.step_ask" => on_step_ask,
        "tb.step_fail" => on_step_fail,
        "tb.step_triage" => on_step_triage,
        "tb.step_aim" => on_step_aim,
        "tb.step_publish" => on_step_publish,
        "tb.take" => on_take,
        "tb.propose" => on_propose,
        "tb.goal" => on_goal,
        "tb.attach" => on_attach,
        "tb.unattach" => on_unattach,
        "tb.new_task" => on_new_task,
        "tb.hello" => on_hello,
        "tb.status" => on_status,
        "tb.switch" => on_switch,
        "tb.line" => on_line,
        _ => return None,
    })
}

const DEDUPE_FIELDS: &[&str] = &["text", "title", "summary", "reason", "prompt", "last_message", "message", "command", "task", "source", "done", "next", "ids"];
const DEDUPE_KEEP: usize = 4000;

fn dedupe_key(body: &Value) -> Option<String> {
    body.get("at").and_then(|v| v.as_str()).filter(|s| !s.is_empty())?;
    let mut parts = vec![body.get("event").cloned(), body.get("session").cloned(), body.get("at").cloned()];
    parts.extend(DEDUPE_FIELDS.iter().map(|f| body.get(*f).cloned()));
    let mut h = DefaultHasher::new();
    serde_json::to_string(&parts).unwrap_or_default().hash(&mut h);
    Some(format!("{:016x}", h.finish()))
}

fn seen_before(app: &App, body: &Value, spooled: bool) -> bool {
    let Some(key) = dedupe_key(body) else { return false };
    let mut s = app.shared.lock();
    if s.recent_set.contains(&key) {
        return spooled;
    }
    s.recent_set.insert(key.clone());
    s.recent_reports.push_back(key);
    while s.recent_reports.len() > DEDUPE_KEEP {
        if let Some(old) = s.recent_reports.pop_front() {
            s.recent_set.remove(&old);
        }
    }
    false
}

pub fn handle(app: &App, body: Value, spooled: bool) -> Result<Value> {
    if !body.is_object() {
        return err(400, "A report must be a JSON object.");
    }
    if seen_before(app, &body, spooled) {
        app.info(format!("spool: skipped a report already received ({})", body["event"]));
        return Ok(json!({"ok": true, "task": null, "duplicate": true}));
    }
    let mut r = Report::new(app, body, spooled);
    let Some(f) = handler(&r.event) else {
        app.info(format!("report ignored: unknown event {:?}", r.event));
        return Ok(json!({"ok": true, "task": null, "ignored": true}));
    };
    if r.event == "hook.stop" || r.event == "hook.attention" {
        let path = r.body.get("transcript_path").and_then(|v| v.as_str()).map(|s| s.to_string());
        r.background = transcript::background_running(&app.cfg.claude_projects, path.as_deref()).unwrap_or_default();
    }
    if r.event == "hook.stop" && !spooled && !as_bool(r.body.get("stop_hook_active"), false) && r.task().ok().flatten().is_none() {
        if let Some(s) = board::get_session(app, r.sid()).ok().flatten() {
            if let Some(turn) = transcript::last_turn(app, &s) {
                r.turn_files = turn.files;
                r.turn_at = turn.at.as_str().map(|s| s.to_string());
            }
            let before = s.s("turn_tree").and_then(|t| serde_json::from_str::<Value>(t).ok());
            if let (Some(before), Some(after)) = (before, r.body.get("tree")) {
                let (files, commit) = tree_changes(&before, after);
                for f in files {
                    if !r.turn_files.contains(&f) {
                        r.turn_files.push(f);
                    }
                }
                r.turn_commit = commit;
            }
        }
    }
    if r.event == "tb.question" && !spooled {
        let t = r.task().ok().flatten();
        r.screened = screen::question(app, t.as_ref(), &one_line(&r.b("text"), 2000));
    }
    if r.event == "tb.done" {
        let t = r.task().ok().flatten();
        done_refusals(&r, t.as_ref())?;
        crate::propen::before_done(app, &mut r.body, t.as_ref())?;
        finishing(&r)?;
    }
    app.db.tx(|| {
        if API_RETRY_EVENTS.contains(&r.event.as_str()) {
            app.db.x("UPDATE sessions SET api_error = NULL, api_error_kind = NULL WHERE id = ?", p![r.sid()])?;
        }
        if API_BACK_EVENTS.contains(&r.event.as_str()) {
            app.db.x("UPDATE sessions SET api_error_at = NULL, api_error_tries = 0 WHERE id = ?", p![r.sid()])?;
        }
        if r.event.starts_with("hook.") && r.event != "hook.pre_compact" && r.event != "hook.delivered" {
            // Any other word from the terminal means its compaction is over; SessionStart "compact" ends it.
            app.db.x("UPDATE sessions SET compacting_at = NULL WHERE id = ? AND compacting_at IS NOT NULL", p![r.sid()])?;
        }
        let mut out = f(&mut r)?;
        if let Some(sid) = r.sid.clone() {
            if let Some((kind, text)) = session_history(&r, &out) {
                // Every board marker (a task's, a goal's planner, the ticket desk's) says the board sent it.
                let sent = r.event == "hook.prompt" && crate::startword::MARKER_RE.is_match(r.body["prompt"].as_str().unwrap_or(""));
                board::session_event_with(app, &sid, kind, &text, Some(&r.at), sent.then_some(crate::startword::BOARD_PROMPT))?;
            }
        }
        if !spooled {
            let mut handed = deliver::take(app, r.sid(), &r.event)?;
            if handed.is_none() && r.stalled {
                handed = nudge(&r)?;
            }
            if let Some(h) = handed {
                out["deliver"] = h;
                if r.event == "hook.stop" {
                    r.set_session_status("working")?;
                }
            }
        }
        Ok(out)
    })
}

pub fn ingest_spool(app: &App) -> Result<usize> {
    let dir = app.cfg.spool_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else { return Ok(0) };
    let mut items = vec![];
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if !name.ends_with(".json") || name.starts_with('.') {
            continue;
        }
        match std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            Some(body) => items.push((body["at"].as_str().unwrap_or("").to_string(), name, p, body)),
            None => {
                app.info(format!("spool {name} unreadable; moved aside"));
                let _ = std::fs::rename(&p, p.with_extension("json.bad"));
            }
        }
    }
    items.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    let mut done = 0;
    for (_, name, p, body) in items {
        match handle(app, body, true) {
            Ok(_) => done += 1,
            Err(e) => app.info(format!("spool {name} refused: {}", e.message)),
        }
        let _ = std::fs::remove_file(&p);
    }
    if done > 0 {
        app.info(format!("spool: handled {}", plural(done as i64, "report")));
    }
    Ok(done)
}
