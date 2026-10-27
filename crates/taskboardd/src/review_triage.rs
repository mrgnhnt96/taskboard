//! Triage of a code review round (docs/REVIEW-PROTOCOL.md): after a round with findings, the board opens
//! a fresh agent in its own terminal (never the task's) with the task's intent, the diff and the
//! findings. It judges only whether this task's change caused each finding, and records each one's
//! outcome with `tb finding T<n> F1 fix|backlog|dismiss "<reason>"`; `backlog` files a backlog item in the
//! task's project and goal. It never edits code. Once every finding has an outcome, the board closes its
//! terminal and the round is triaged. With `fix` findings, the task's own agent gets the fix list, marks
//! each with `tb finding T<n> F1 fixed "<what it changed>"` and submits again for the next round.

use serde_json::{json, Value};

use crate::app::App;
use crate::code_review::{self, EVENT};
use crate::util::*;
use crate::{board, fields, p};

/// The triage agent's job.
pub const PURPOSE: &str = "code_review_triage";
/// What a triage terminal is for, wherever the board shows it.
pub const ROLE: &str = "Triages a code review's findings";
/// The stage's state while the triage agent works.
pub const STATE: &str = "triaging";
pub const OUTCOMES: &[&str] = &["fix", "backlog", "dismiss"];
/// What the task's own agent marks a `fix` finding once it has fixed it.
pub const FIXED: &str = "fixed";
const REASON_MAX: usize = 300;
/// A triage that hasn't finished by then is closed, and the task carries on without it.
const MAX_SECS: i64 = 3600;
const DIFF_INLINE_MAX: usize = 20_000;

/// Whether a round's result needs triage: a `fail` verdict with findings.
pub fn wanted(result: &Value) -> bool {
    result["verdict"] == "fail" && result["findings"].as_array().is_some_and(|f| !f.is_empty())
}

/// This terminal is a triage agent's.
pub fn is_triage(app: &App, sid: Option<&str>) -> Result<bool> {
    let Some(sid) = sid.filter(|s| !s.is_empty()) else { return Ok(false) };
    Ok(!app
        .db
        .val("SELECT 1 FROM jobs WHERE kind = 'agent' AND purpose = ? AND json_extract(target, '$.session') = ? LIMIT 1", p![PURPOSE, sid])?
        .is_null())
}

/// The task a triage terminal works for, if it is one.
pub fn task_of(app: &App, sid: Option<&str>) -> Result<Option<Row>> {
    let Some(sid) = sid.filter(|s| !s.is_empty()) else { return Ok(None) };
    let j = app.db.q1(
        "SELECT * FROM jobs WHERE kind = 'agent' AND purpose = ? AND json_extract(target, '$.session') = ? ORDER BY id DESC LIMIT 1",
        p![PURPOSE, sid],
    )?;
    board::find_task(app, j.and_then(|j| board::job_target(&j).i("task")))
}

/// What a triage terminal is told when its session starts.
pub fn intro(app: &App, sid: Option<&str>) -> Result<String> {
    let t = task_of(app, sid)?;
    let me = t.as_ref().map(|t| rf("task", t.id())).unwrap_or_else(|| "the task".into());
    let tb = board::tb_cmd(app);
    Ok(format!(
        "You are a triage agent for {me}'s code review, not a task's agent: don't take board tasks and never change code. \
         {tb} finding {me} lists the findings and what's left; record each with {tb} finding {me} <id> fix|backlog|dismiss \"<reason>\"."
    ))
}

/// The round's recorded event: its id and data.
fn round_event(app: &App, task_id: i64, n: u64) -> Result<Option<(i64, Value)>> {
    let rows = app.db.q("SELECT id, data FROM events WHERE task_id = ? AND kind = ? ORDER BY id DESC", p![task_id, EVENT])?;
    Ok(rows.iter().find_map(|r| {
        let d = serde_json::from_str::<Value>(r.s("data")?).ok()?;
        (d["ok"] == true && d["round"].as_u64() == Some(n)).then(|| (r.id(), d))
    }))
}

fn findings(d: &Value) -> Vec<Value> {
    d["result"]["findings"].as_array().cloned().unwrap_or_default()
}

fn left(d: &Value) -> Vec<String> {
    findings(d).iter().filter_map(|f| f["id"].as_str()).filter(|id| d["triage"][*id]["outcome"].is_null()).map(str::to_string).collect()
}

fn with_outcome(d: &Value, outcome: &str) -> Vec<Value> {
    findings(d).into_iter().filter(|f| d["triage"][f["id"].as_str().unwrap_or("")]["outcome"] == outcome).collect()
}

/// The round's `fix` findings its agent hasn't marked fixed.
fn unfixed(d: &Value) -> Vec<String> {
    with_outcome(d, "fix").iter().filter_map(|f| f["id"].as_str()).filter(|id| d["triage"][*id]["fixed"].is_null()).map(str::to_string).collect()
}

fn finding_line(f: &Value) -> String {
    let at = match (f["file"].as_str(), f["line"].as_i64()) {
        (Some(file), Some(l)) => format!(" ({file}:{l})"),
        (Some(file), None) => format!(" ({file})"),
        _ => String::new(),
    };
    let sev = f["severity"].as_str().map(|s| format!("[{s}] ")).unwrap_or_default();
    format!("{}: {sev}{}{at}", f["id"].as_str().unwrap_or(""), f["title"].as_str().unwrap_or("(untitled)"))
}

/// The change under review, from where the branch left its base to the worktree as it stands.
fn diff(dir: &str, base: &str) -> String {
    let from = (!base.is_empty()).then(|| code_review::git_out(dir, &["merge-base", base, "HEAD"], &[])).flatten().unwrap_or_else(|| "HEAD".into());
    code_review::git_out(dir, &["diff", &from], &[]).unwrap_or_default()
}

fn prompt(app: &App, t: &Row, n: u64, result: &Value, intent: &str, diff: &str, files: &std::path::Path) -> String {
    let me = rf("task", t.id());
    let tb = board::tb_cmd(app);
    let list: Vec<String> = result["findings"].as_array().into_iter().flatten().map(finding_line).collect();
    let change = if diff.trim().is_empty() {
        "The diff is empty: the change may be in files git doesn't track yet (git status shows them).".to_string()
    } else if diff.len() <= DIFF_INLINE_MAX {
        format!("```diff\n{}\n```", diff.trim_end())
    } else {
        format!("It's too long to show here: read {}.", files.join("diff.patch").display())
    };
    format!(
        "You are the triage agent for {me}'s code review, round {n}: a reviewer read the task's change and reported findings. \
         For each finding, judge only whether this task's change caused it, and record its outcome:\n\
         - fix: this task's change caused it and it's real; the task's agent fixes it.\n\
         - backlog: real, but this task's change didn't cause it; the board files it in the task's backlog.\n\
         - dismiss: not real, already handled, or not worth acting on.\n\
         A finding that asks to undo what the task or its goal sets out to do is dismiss, saying so.\n\n\
         Record each one, with a one-line reason, as soon as you've judged it: {tb} finding {me} <id> fix|backlog|dismiss \"<reason>\"\n\
         {tb} finding {me} lists them with what's left. Never change code, commit or run the project's tools: read the diff and the files \
         (git diff, git log and git show are fine) and judge. When every finding has an outcome, the board closes this terminal.\n\n\
         # The task\n\n{intent}\n# The change\n\n{change}\n\n# The findings\n\n{}\n\nEach finding in full (detail, url): {}",
        list.join("\n"),
        files.join("findings.json").display()
    )
}

fn flags() -> String {
    let allowed = ["Bash(tb finding:*)", "Bash(git diff:*)", "Bash(git log:*)", "Bash(git show:*)", "Bash(git status:*)", "Read", "Grep", "Glob"];
    let denied = ["Edit", "Write", "NotebookEdit", "Agent", "Task", "Skill"];
    format!("--allowedTools '{}' --disallowedTools '{}' {}", allowed.join(","), denied.join(","), crate::midna::NO_SLASH_COMMANDS)
}

/// After round `n` found something: opens the triage agent and puts the stage in triage. An error
/// means it couldn't, and the caller carries on without it.
pub fn open(app: &App, t: &Row, stage: &mut Value, n: u64, result: &Value) -> Result<()> {
    let dir = code_review::worktree(app, t);
    let here = code_review::task_dir(app, t.id()).join(format!("round-{n}"));
    let vars = crate::steps::vars_at(app, t, None);
    let change = diff(&dir, vars.get("base_ref").map(String::as_str).unwrap_or(""));
    let intent = code_review::intent_text(app, t);
    std::fs::create_dir_all(&here)
        .and_then(|_| std::fs::write(here.join("diff.patch"), &change))
        .and_then(|_| std::fs::write(here.join("findings.json"), jdumps(&result["findings"])))
        .or_else(|e| err(500, format!("couldn't write its files: {e}")))?;
    let text = prompt(app, t, n, result, &intent, &change, &here);
    let me = rf("task", t.id());
    app.db.tx(|| {
        let job = board::create_job(
            app,
            "agent",
            json!({"cwd": dir, "title": format!("Triage {me}"), "prompt": text, "flags": flags(), "background": true}),
            None,
            PURPOSE,
            Some(json!({"task": t.id(), "round": n})),
        )?;
        stage["state"] = json!(STATE);
        stage["job"] = json!(job);
        stage["triage_since"] = json!(now_iso());
        code_review::save_stage(app, t.id(), Some(stage))?;
        let count = result["findings"].as_array().map_or(0, |f| f.len());
        board::log_event(app, t.id(), board::BOARD, "status", &format!("Triaging code review round {n}'s {count} finding{}", if count == 1 { "" } else { "s" }))
            .map(|_| ())
    })
}

/// The triage terminal, once its agent job has opened one.
fn session(app: &App, stage: &Value) -> Result<Option<String>> {
    let Some(jid) = stage["job"].as_i64() else { return Ok(None) };
    let j = app.db.q1("SELECT * FROM jobs WHERE id = ?", p![jid])?;
    Ok(j.and_then(|j| board::job_target(&j).s("session").map(str::to_string)))
}

/// Closes the triage terminal, if it's open.
pub fn close(app: &App, stage: &Value, why: &str) -> Result<()> {
    let Some(sid) = session(app, stage)? else { return Ok(()) };
    let s = board::get_session(app, Some(&sid))?;
    if s.as_ref().is_some_and(|s| s.s("status") == Some("gone")) || crate::jobs::closing(app, &sid)? {
        return Ok(());
    }
    app.db.tx(|| board::create_job(app, "close", json!({"session": sid, "force": true, "reason": why}), None, PURPOSE, None).map(|_| ()))
}

/// While a task's round is in triage (each tick of the review loop): once every finding has an outcome,
/// closes the triage terminal and the round is triaged. A triage whose terminal couldn't open, closed
/// with findings left or ran past an hour ends too, with an alert.
pub fn tick(app: &App, t: &Row, stage: Value) -> Result<()> {
    let id = t.id();
    let me = rf("task", id);
    let n = stage["round"].as_u64().unwrap_or(1);
    let d = round_event(app, id, n)?.map(|(_, d)| d).unwrap_or(Value::Null);
    let rest = left(&d);
    if rest.is_empty() {
        close(app, &stage, &format!("Task board: {me}'s code review round {n} is triaged"))?;
        let counts: Vec<String> = OUTCOMES
            .iter()
            .map(|o| (o, findings(&d).iter().filter(|f| d["triage"][f["id"].as_str().unwrap_or("")]["outcome"] == *o).count()))
            .filter(|(_, c)| *c > 0)
            .map(|(o, c)| format!("{c} {o}"))
            .collect();
        app.db.tx(|| board::log_event(app, id, board::BOARD, "status", &format!("Code review round {n} triaged: {}", counts.join(", "))).map(|_| ()))?;
        let fix = with_outcome(&d, "fix");
        if fix.is_empty() {
            return code_review::carry_on(app, t, stage, None);
        }
        let max = app.cfg.code_review.max_rounds as u64;
        if n >= max {
            let lines: Vec<String> = fix.iter().map(finding_line).collect();
            let alert = format!("{me}'s code review reached its {max}-round cap with findings still to fix, so it went on to its PR: {}", lines.join("; "));
            return code_review::carry_on(app, t, stage, Some(alert));
        }
        return start_fixing(app, t, n, &d, &fix);
    }
    let job = stage["job"].as_i64().map(|j| app.db.q1("SELECT * FROM jobs WHERE id = ?", p![j])).transpose()?.flatten();
    let refused = job.as_ref().map_or(true, |j| matches!(j.s("state"), Some("failed") | Some("cancelled")));
    let gone = match session(app, &stage)? {
        Some(sid) => board::get_session(app, Some(&sid))?.is_some_and(|s| s.s("status") == Some("gone")),
        None => false,
    };
    let late = stage["triage_since"].as_str().and_then(parse_iso).is_some_and(|at| now_ts() - at > MAX_SECS as f64);
    let why = if refused {
        format!("its triage agent couldn't open{}", job.and_then(|j| jloads_obj(j.s("result")).s("stderr").map(|e| format!(" ({})", one_line(e, 200)))).unwrap_or_default())
    } else if gone {
        "its triage terminal closed".into()
    } else if late {
        close(app, &stage, &format!("Task board: {me}'s triage ran past an hour"))?;
        "its triage ran past an hour".into()
    } else {
        return Ok(());
    };
    let alert = format!("{me}'s code review round {n}: {why} with {} of its findings left ({}), so it carried on.", rest.len(), rest.join(", "));
    code_review::carry_on(app, t, stage, Some(alert))
}

/// After round `n` is triaged with `fix` findings: drops the held `tb submit` and sends the task's own
/// agent the fix list. Its next `tb submit` runs the next round, or carries on when the code is unchanged.
fn start_fixing(app: &App, t: &Row, n: u64, d: &Value, fix: &[Value]) -> Result<()> {
    let id = t.id();
    let me = rf("task", id);
    let tb = board::tb_cmd(app);
    let max = app.cfg.code_review.max_rounds as u64;
    let list: Vec<String> = fix
        .iter()
        .map(|f| format!("- {}\n  Triage: {}", finding_line(f), d["triage"][f["id"].as_str().unwrap_or("")]["reason"].as_str().unwrap_or("")))
        .collect();
    let files = code_review::task_dir(app, id).join(format!("round-{n}")).join("findings.json");
    let k = fix.len();
    let text = format!(
        "[task-board] {me}'s code review, round {n}, found {k} thing{} your change caused, to fix before its PR:\n{}\n\
         Each in full (detail, url): {}\n\
         Fix only these: the round's other findings went to the backlog or were dismissed. Mark each once it's fixed: \
         {tb} finding {me} <id> fixed \"<what you changed>\"\n\
         Then commit, push and run tb submit again as before, with the same summary and --pr-body: the board reviews the change \
         as round {} of at most {max}. With the code unchanged, tb submit goes straight on to the PR, and the owner hears of any \
         finding not marked fixed.",
        if k == 1 { "" } else { "s" },
        list.join("\n"),
        files.display(),
        n + 1
    );
    let stage = json!({"state": code_review::FIXING, "round": n, "since": now_iso()});
    app.db.tx(|| {
        code_review::save_stage(app, id, Some(&stage))?;
        board::update_task(app, id, fields!["latest" => "Fixing code review findings"])?;
        board::log_event(app, id, board::BOARD, "status", &format!("Fixing code review round {n}'s {k} finding{}", if k == 1 { "" } else { "s" }))?;
        crate::deliver::add(app, "code_review_fix", &text, id, None, None).map(|_| ())
    })
}

/// A `tb submit` while the agent fixes round `n`'s findings that carries on without another round (the
/// code is unchanged, or the rounds ran out): ends the fixing, and tells the owner of any finding left
/// unfixed.
pub fn end_fixing(app: &App, t: &Row, stage: &Value) -> Result<()> {
    let id = t.id();
    let me = rf("task", id);
    let n = stage["round"].as_u64().unwrap_or(1);
    let d = round_event(app, id, n)?.map(|(_, d)| d).unwrap_or(Value::Null);
    let rest = unfixed(&d);
    let open: Vec<String> = with_outcome(&d, "fix").iter().filter(|f| f["id"].as_str().is_some_and(|i| rest.iter().any(|r| r == i))).map(finding_line).collect();
    app.db.tx(|| {
        code_review::save_stage(app, id, None)?;
        if !open.is_empty() {
            let alert = format!("{me} went on to its PR without another code review round, with round {n}'s findings not marked fixed: {}", open.join("; "));
            crate::dispatch::add_alert(app, &alert, Some(id), t.i("goal_id"), None, None)?;
        }
        Ok(())
    })
}

/// While the task's agent fixes round `n`'s findings, for the task card: how many to fix and how many are fixed.
pub fn fix_progress(app: &App, task_id: i64, n: u64) -> Result<Value> {
    let d = round_event(app, task_id, n)?.map(|(_, d)| d).unwrap_or(Value::Null);
    let all = with_outcome(&d, "fix").len();
    Ok(json!({"findings": all, "fixed": all - unfixed(&d).len()}))
}

/// How far round `n`'s triage has got, for the task card: its findings and how many have an outcome.
pub fn progress(app: &App, task_id: i64, n: u64) -> Result<Value> {
    let d = round_event(app, task_id, n)?.map(|(_, d)| d).unwrap_or(Value::Null);
    let all = findings(&d).len();
    Ok(json!({"findings": all, "triaged": all - left(&d).len()}))
}

/// The round in triage and each of its findings with its outcome so far, for `tb finding T<n>`.
pub fn state(app: &App, task_id: i64) -> Result<Value> {
    let t = board::get_task(app, task_id)?;
    let me = rf("task", task_id);
    if let Some(stage) = code_review::fixing(&t) {
        let n = stage["round"].as_u64().unwrap_or(1);
        let d = round_event(app, task_id, n)?.map(|(_, d)| d).unwrap_or(Value::Null);
        let list: Vec<Value> = with_outcome(&d, "fix")
            .iter()
            .map(|f| {
                let tr = &d["triage"][f["id"].as_str().unwrap_or("")];
                json!({"id": f["id"], "line": finding_line(f), "outcome": tr["outcome"], "reason": tr["reason"], "fixed": tr["fixed"]})
            })
            .collect();
        return Ok(json!({"task": me, "triaging": false, "fixing": true, "round": n, "findings": list, "left": unfixed(&d)}));
    }
    let Some(stage) = code_review::stage(&t).filter(|s| s["state"] == STATE) else {
        return Ok(json!({"task": me, "triaging": false}));
    };
    let n = stage["round"].as_u64().unwrap_or(1);
    let d = round_event(app, task_id, n)?.map(|(_, d)| d).unwrap_or(Value::Null);
    let list: Vec<Value> = findings(&d)
        .iter()
        .map(|f| {
            let tr = &d["triage"][f["id"].as_str().unwrap_or("")];
            json!({"id": f["id"], "line": finding_line(f), "outcome": tr["outcome"], "reason": tr["reason"], "issue": tr["issue"].as_i64().map(|i| rf("issue", i))})
        })
        .collect();
    Ok(json!({"task": me, "triaging": true, "round": n, "findings": list, "left": left(&d)}))
}

/// `tb finding T<n> <id> <outcome> <reason>`: records one finding's outcome in the round in triage, or
/// (`fixed`) that the task's agent has fixed one of the round it's fixing.
pub fn report(app: &App, task_id: i64, body: &Value) -> Result<Value> {
    let t = board::get_task(app, task_id)?;
    let me = rf("task", task_id);
    let sid = body["session"].as_str().filter(|s| !s.is_empty());
    if body["outcome"].as_str().is_some_and(|o| o.trim().eq_ignore_ascii_case(FIXED)) {
        return mark_fixed(app, &t, body, sid);
    }
    if sid.is_some() && t.s("session_id") == sid {
        return err(403, format!("{me}'s own agent can't triage its findings: a fresh triage agent does."));
    }
    let Some(stage) = code_review::stage(&t).filter(|s| s["state"] == STATE) else {
        return err(409, format!("{me} has no code review round in triage."));
    };
    let n = stage["round"].as_u64().unwrap_or(1);
    let fid = body["finding"].as_str().map(str::trim).unwrap_or("");
    let outcome = body["outcome"].as_str().map(|s| s.trim().to_lowercase()).unwrap_or_default();
    let reason = body["reason"].as_str().map(str::trim).unwrap_or("");
    if !OUTCOMES.contains(&outcome.as_str()) {
        return err(400, "The outcome is fix, backlog or dismiss (or fixed, from the task's own agent).");
    }
    if reason.is_empty() {
        return err(400, format!("Say why: tb finding {me} {} {outcome} \"<reason>\"", if fid.is_empty() { "<id>" } else { fid }));
    }
    if reason.chars().count() > REASON_MAX || reason.contains(['\n', '\r']) {
        return err(400, format!("The reason is one line of {REASON_MAX} characters at most."));
    }
    let Some((eid, mut d)) = round_event(app, task_id, n)? else {
        return err(409, format!("{me}'s code review round {n} has no result to triage."));
    };
    let all = findings(&d);
    let Some(f) = all.iter().find(|f| f["id"].as_str().is_some_and(|id| id.eq_ignore_ascii_case(fid))) else {
        let ids: Vec<&str> = all.iter().filter_map(|f| f["id"].as_str()).collect();
        return err(404, format!("Round {n} has no finding “{fid}”; its findings are {}.", ids.join(", ")));
    };
    let fid = f["id"].as_str().unwrap_or(fid).to_string();
    if let Some(was) = d["triage"][&fid]["outcome"].as_str() {
        return err(409, format!("{fid} is already marked {was}."));
    }
    let name = board::session_name(app, sid, None);
    app.db.tx(|| {
        let issue = if outcome == "backlog" { Some(file_issue(app, &t, n, f, reason, sid, &name)?) } else { None };
        if !d["triage"].is_object() {
            d["triage"] = json!({});
        }
        d["triage"][&fid] = json!({"outcome": outcome, "reason": reason, "issue": issue, "at": now_iso()});
        app.db.x("UPDATE events SET data = ? WHERE id = ?", p![jdumps(&d), eid])?;
        let filed = issue.map(|i| format!(" (filed as {})", rf("issue", i))).unwrap_or_default();
        board::log_event_full(
            app,
            task_id,
            &name,
            "code_review_triage",
            &format!("Triage, round {n}: {fid} {outcome}{filed}: {reason}"),
            Some(json!({"round": n, "finding": fid, "outcome": outcome, "reason": reason, "issue": issue})),
            None,
        )
        .map(|_| ())
    })?;
    let rest = left(&d);
    let message = if rest.is_empty() {
        format!("Marked {fid} {outcome}. That was the last one: round {n} is triaged and the board closes this terminal.")
    } else {
        format!("Marked {fid} {outcome}. Left: {}.", rest.join(", "))
    };
    Ok(json!({"ok": true, "task": me, "round": n, "finding": fid, "outcome": outcome, "issue": d["triage"][&fid]["issue"].as_i64().map(|i| rf("issue", i)), "left": rest, "message": message}))
}

/// `tb finding T<n> <id> fixed "<what changed>"`: the task's agent marks one of the round's `fix` findings fixed.
fn mark_fixed(app: &App, t: &Row, body: &Value, sid: Option<&str>) -> Result<Value> {
    let task_id = t.id();
    let me = rf("task", task_id);
    if is_triage(app, sid)? {
        return err(403, format!("A triage agent marks fix, backlog or dismiss; {me}'s own agent marks a finding fixed."));
    }
    let Some(stage) = code_review::fixing(t) else {
        return err(409, format!("{me} has no code review findings to fix."));
    };
    let n = stage["round"].as_u64().unwrap_or(1);
    let fid = body["finding"].as_str().map(str::trim).unwrap_or("");
    let note = body["reason"].as_str().map(str::trim).unwrap_or("");
    if note.is_empty() {
        return err(400, format!("Say what you changed: tb finding {me} {} fixed \"<what you changed>\"", if fid.is_empty() { "<id>" } else { fid }));
    }
    if note.chars().count() > REASON_MAX || note.contains(['\n', '\r']) {
        return err(400, format!("What you changed is one line of {REASON_MAX} characters at most."));
    }
    let Some((eid, mut d)) = round_event(app, task_id, n)? else {
        return err(409, format!("{me}'s code review round {n} has no result."));
    };
    let fix = with_outcome(&d, "fix");
    let Some(f) = fix.iter().find(|f| f["id"].as_str().is_some_and(|id| id.eq_ignore_ascii_case(fid))) else {
        let ids: Vec<&str> = fix.iter().filter_map(|f| f["id"].as_str()).collect();
        return err(404, format!("Round {n} has no finding “{fid}” to fix; the ones to fix are {}.", ids.join(", ")));
    };
    let fid = f["id"].as_str().unwrap_or(fid).to_string();
    if let Some(was) = d["triage"][&fid]["fixed"].as_str() {
        return err(409, format!("{fid} is already marked fixed: {was}"));
    }
    let name = board::session_name(app, sid, None);
    app.db.tx(|| {
        d["triage"][&fid]["fixed"] = json!(note);
        d["triage"][&fid]["fixed_at"] = json!(now_iso());
        app.db.x("UPDATE events SET data = ? WHERE id = ?", p![jdumps(&d), eid])?;
        board::log_event_full(
            app,
            task_id,
            &name,
            "code_review_triage",
            &format!("Fixed, round {n}: {fid}: {note}"),
            Some(json!({"round": n, "finding": fid, "outcome": FIXED, "reason": note})),
            None,
        )
        .map(|_| ())
    })?;
    let rest = unfixed(&d);
    let message = if rest.is_empty() {
        format!("Marked {fid} fixed. That was the last one: commit, push and run tb submit again for the next round.")
    } else {
        format!("Marked {fid} fixed. Left to fix: {}.", rest.join(", "))
    };
    Ok(json!({"ok": true, "task": me, "round": n, "finding": fid, "outcome": FIXED, "left": rest, "message": message}))
}

/// Files a `backlog` finding in the task's project and goal, or adds it to the open item that already has its title.
fn file_issue(app: &App, t: &Row, n: u64, f: &Value, reason: &str, sid: Option<&str>, name: &str) -> Result<i64> {
    let me = rf("task", t.id());
    let title = one_line(f["title"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or(f["id"].as_str().unwrap_or("A code review finding")), 300);
    let goal_id = t.i("goal_id");
    let project = t.s("project");
    let key = norm_title(&title);
    let cands = match goal_id {
        Some(g) => app.db.q("SELECT * FROM issues WHERE state = 'open' AND goal_id = ?", p![g])?,
        None => app.db.q("SELECT * FROM issues WHERE state = 'open' AND goal_id IS NULL AND project IS ?", p![project])?,
    };
    if let Some(dup) = cands.iter().find(|c| norm_title(&c.st("title")) == key) {
        board::add_issue_event(app, dup.id(), name, "seen", &format!("Seen again in {me}'s code review round {n}, and left to the backlog: {reason}"), None)?;
        return Ok(dup.id());
    }
    let mut detail = f["detail"].as_str().unwrap_or("").trim().to_string();
    let at = finding_line(f);
    detail = [format!("From {me}'s code review, round {n}: {at}"), detail, f["url"].as_str().map(|u| u.to_string()).unwrap_or_default(), format!("Triage: {reason}")]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let now = now_iso();
    let snap = json!({"task": format!("{me} · {}", t.st("title")), "terminal": name});
    let iid = app.db.insert(
        "issues",
        fields!["goal_id" => goal_id, "project" => project, "kind" => "follow", "title" => title, "detail" => detail,
                "said" => format!("“{}”", one_line(reason, 1000)), "how" => format!("{me}'s code review triage left it to the backlog."),
                "source" => "terminal", "found_by_task" => t.id(), "found_by_session" => sid, "found_by_name" => name,
                "state" => "open", "snapshot" => jdumps(&snap), "created_at" => now, "updated_at" => now, "misses_goal" => 0],
    )?;
    board::add_issue_event(app, iid, name, "report", &format!("Left to the backlog by {me}'s code review triage"), Some(&snap))?;
    if let Some(g) = goal_id {
        app.db.x("UPDATE tasks SET ctx_version = COALESCE(ctx_version, 1) + 1 WHERE goal_id = ? AND status != 'done' AND id IS NOT ?", p![g, t.id()])?;
    }
    Ok(iid)
}
