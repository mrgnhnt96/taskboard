//! The JSON API under `/tasks/api`.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::app::App;
use crate::board::OWNER;
use crate::ops::{goal_detail, issue_detail, new_goal, new_task, opt_goal, task_detail};
use crate::util::*;
use crate::{accounts, board, days, deliver, dispatch as alerts, fields, handoff, hooks, hours, jira, keep_awake, midna, ops, p, prflow, projects, qa, reports, runner, shared, steps, triage, usage};

pub type Query = HashMap<String, String>;

fn q<'a>(query: &'a Query, k: &str, default: &'a str) -> &'a str {
    query.get(k).map(|s| s.as_str()).filter(|s| !s.is_empty()).unwrap_or(default)
}

fn project_match(project: Option<&str>, want: &str) -> bool {
    want.is_empty() || want == "all" || project == Some(want)
}

fn goal_match(goal_id: Option<i64>, want: &str) -> Result<bool> {
    Ok(match want {
        "" | "all" => true,
        "none" => goal_id.is_none(),
        w => goal_id.is_some() && goal_id == parse_ref_str(w, "goal")?,
    })
}

/// A task matches a goal filter through its home goal or a goal it also finishes.
fn task_goal_match(app: &App, t: &Row, want: &str) -> Result<bool> {
    if goal_match(t.i("goal_id"), want)? {
        return Ok(true);
    }
    if matches!(want, "" | "all" | "none") {
        return Ok(false);
    }
    Ok(parse_ref_str(want, "goal")?.map(|g| shared::goal_ids(app, t.id()).map(|ids| ids.contains(&g))).transpose()?.unwrap_or(false))
}

/// The request came from Taskboard.app (the owner's own click), not from `tb` or an agent. The
/// server sets `_from` from the app's `X-Task-Board-From: app` header; the in-process sample board
/// passes it in the query.
pub const FROM: &str = "_from";

fn from_app(query: &Query) -> bool {
    query.get(FROM).map(|s| s.as_str()) == Some("app")
}

fn wave_n(s: &str) -> Result<i64> {
    s.parse::<i64>().ok().filter(|n| *n >= 0).ok_or_else(|| ApiError::new(404, "There's no such wave."))
}

fn tid(s: &str) -> Result<i64> {
    parse_ref_str(s, "task")?.ok_or_else(|| ApiError::new(404, "There's nothing at that address."))
}
fn gid(s: &str) -> Result<i64> {
    parse_ref_str(s, "goal")?.ok_or_else(|| ApiError::new(404, "There's nothing at that address."))
}
fn iid(s: &str) -> Result<i64> {
    parse_ref_str(s, "issue")?.ok_or_else(|| ApiError::new(404, "There's nothing at that address."))
}

fn required(body: &Value, key: &str, limit: usize, label: &str) -> Result<String> {
    let v = clip(&body_str(body, key), limit);
    if v.is_empty() {
        return err(400, format!("{label} can't be empty."));
    }
    Ok(v)
}

/// Counts the PR's checks as passed for its current push (or every push, with `all`), and moves it on.
fn pr_skip_checks(app: &App, id: i64, body: &Value) -> Result<Value> {
    let reason = one_line(&body_str(body, "reason"), 500);
    if reason.is_empty() {
        return err(400, "Say why the checks don't need to pass: --reason \"<why>\" (a hook cancelled the builds, say).");
    }
    let who = { let w = body_str(body, "who"); if w.is_empty() { OWNER.to_string() } else { w } };
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        if t.i("pr_num").is_none() && !has(t.s("pr_url")) {
            return err(409, "This task has no PR.");
        }
        prflow::skip_checks(app, &t, &reason, as_bool(body.get("all"), false), &who)
    })?;
    let t = board::get_task(app, id)?;
    Ok(json!({"ok": true, "task": rf("task", id), "pr": board::pr_card(&t)}))
}

/// What a hook would read for an event on a task (`tb hooks test`): the task given, else the newest one.
fn hook_payload(app: &App, query: &Query) -> Result<Value> {
    let event = q(query, "event", "");
    if !hooks::known(event) {
        return err(400, format!("\"{event}\" isn't an event. Run `tb hooks` for the list."));
    }
    let t = match query.get("task").filter(|s| !s.is_empty()) {
        Some(r) => board::get_task(app, tid(r)?)?,
        None => match app.db.q1("SELECT * FROM tasks ORDER BY id DESC LIMIT 1", p![])? {
            Some(t) => t,
            None => return err(404, "There are no tasks to test with yet."),
        },
    };
    let mut v = hooks::payload(app, event, &t, None);
    v["test"] = json!(true);
    Ok(v)
}

pub fn dispatch(app: &App, method: &str, path: &str, query: &Query, body: &Value) -> Result<Value> {
    let segs: Vec<&str> = path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let r = match (method, segs.as_slice()) {
        ("GET", ["state"]) => get_state(app, query),
        ("GET", ["accounts"]) => accounts::status(app, q(query, "fresh", "") == "1"),
        ("GET", ["ci-token"]) => Ok(accounts::ci_token_status(app)),
        ("POST", ["ci-token", "clear"]) => accounts::clear_ci_token(app),
        ("POST", ["ci-token"]) => accounts::set_ci_token(app, body),
        ("POST", ["accounts", "github", "login"]) => accounts::github_login(app),
        ("POST", ["accounts", "github", "cancel"]) => accounts::github_cancel(app),
        ("POST", ["accounts", "github", "import"]) => accounts::github_import(app),
        ("POST", ["accounts", id, "check"]) => accounts::check(app, id),
        ("POST", ["accounts", id, "disconnect"]) => accounts::disconnect(app, id),
        ("POST", ["accounts", id]) => accounts::connect(app, id, body),
        ("GET", ["summary"]) => get_summary(app),
        ("GET", ["locks"]) => crate::locks::overview(app),
        ("GET", ["hooks", "payload"]) => hook_payload(app, query),
        ("GET", ["projects"]) => Ok(json!({"projects": projects::list_projects(app)?.iter().map(|p| projects::describe(app, p)).collect::<Result<Vec<_>>>()?})),
        ("GET", ["projects", "agents-merge"]) => Ok(projects::describe_agents_merge(app)),
        ("POST", ["projects", "agents-merge"]) => set_agents_merge(app, body),
        ("POST", ["projects", name]) => patch_project(app, name, body),
        ("GET", ["sessions"]) => Ok(json!({"sessions": ops::session_list(app, q(query, "project", "all"))?})),
        ("GET", ["sessions", "closed"]) => {
            let (rows, total) = ops::closed_session_list(app, q(query, "project", "all"), q(query, "q", ""), 200)?;
            Ok(json!({"sessions": rows, "total": total}))
        }
        ("GET", ["sessions", id]) => ops::session_detail(app, id),
        ("POST", ["sessions", "close"]) => close_sessions(app, body),
        ("POST", ["sessions", id, "close"]) => close_session(app, id, body),
        ("POST", ["sessions", id, "focus"]) => focus_session(app, id),
        ("POST", ["sessions", id, "rename"]) => rename_session(app, id, body),
        ("POST", ["sessions", id, "reopen"]) => reopen_session(app, id),
        ("GET", ["whoami"]) => {
            let sid = query.get("session").map(|s| s.as_str()).unwrap_or("");
            let t = board::task_for_session(app, Some(sid))?;
            let v = if t.is_none() { prflow::visited_by(app, sid)? } else { None };
            Ok(json!({"session": sid,
                      "task": match t { Some(t) => board::task_card(app, &t)?, None => Value::Null },
                      "line": crate::lines::entries(app, sid)?,
                      "visiting": match v { Some(v) => board::task_card(app, &v)?, None => Value::Null }}))
        }
        ("GET", ["steps"]) => {
            let t = match query.get("task").filter(|s| !s.is_empty()) {
                Some(r) => Some(board::get_task(app, tid(r)?)?),
                None => {
                    let sid = query.get("session").map(|s| s.as_str());
                    match board::task_for_session(app, sid)? {
                        Some(t) => Some(t),
                        // A terminal working on a done task's PR: its pushes are that task's.
                        None => prflow::visited_by(app, sid.unwrap_or(""))?,
                    }
                }
            };
            match t {
                Some(t) => steps::list(app, &t, query.get("head").map(|s| s.as_str()).filter(|h| !h.is_empty())),
                None => Ok(json!({"task": null, "steps": []})),
            }
        }
        ("GET", ["jobs"]) => {
            let st = q(query, "state", "");
            let rows = if st.is_empty() {
                app.db.q("SELECT * FROM jobs ORDER BY id DESC LIMIT 100", p![])?
            } else {
                app.db.q("SELECT * FROM jobs WHERE state = ? ORDER BY id DESC LIMIT 100", p![st])?
            };
            Ok(json!({"jobs": rows.iter().map(board::job_dict).collect::<Vec<_>>()}))
        }
        ("GET", ["tasks"]) => list_tasks(app, query),
        ("POST", ["tasks"]) => {
            let c = app.db.tx(|| new_task(app, body, OWNER, None))?;
            task_detail(app, c["id"].as_i64().unwrap_or(0))
        }
        ("GET", ["tasks", id]) => task_detail(app, tid(id)?),
        ("GET", ["tasks", id, "handoff"]) => {
            let t = tid(id)?;
            board::get_task(app, t)?;
            Ok(json!({"text": handoff::build(app, t)?}))
        }
        ("GET", ["tasks", id, "log"]) => get_log(app, tid(id)?, q(query, "kind", "all")),
        ("GET", ["tasks", id, "pr"]) => {
            let t = tid(id)?;
            let mut v = get_pr(app, t)?;
            if matches!(q(query, "full", ""), "1" | "true") {
                v["live"] = crate::prcmds::status(app, t)?;
                // The read just now stepped the PR: answer with what it found.
                let fresh = get_pr(app, t)?;
                for k in ["pr", "record", "checked_at"] {
                    v[k] = fresh[k].clone();
                }
            }
            Ok(v)
        }
        ("POST", ["tasks", id]) => patch_task(app, tid(id)?, body),
        ("POST", ["tasks", id, "queue"]) => patch_task(app, tid(id)?, &json!({"status": "queued"})),
        ("POST", ["tasks", id, "start"]) => start(app, tid(id)?, query, body),
        ("POST", ["tasks", id, "answer"]) => answer(app, tid(id)?, body),
        ("POST", ["tasks", id, "step"]) => owner_step(app, tid(id)?, body),
        ("POST", ["tasks", id, "detach"]) => detach(app, tid(id)?),
        ("POST", ["tasks", id, "close-terminal"]) => close_terminal(app, tid(id)?, body),
        ("POST", ["tasks", id, "focus"]) => focus_task(app, tid(id)?),
        ("POST", ["tasks", id, "checkpoint-request"]) => checkpoint_request(app, tid(id)?),
        ("POST", ["tasks", id, "resume"]) => resume(app, tid(id)?, body),
        ("POST", ["tasks", id, "requeue"]) => requeue(app, tid(id)?),
        ("POST", ["tasks", id, "done"]) => mark_done(app, tid(id)?, body, false),
        ("POST", ["tasks", id, "fail"]) => mark_done(app, tid(id)?, body, true),
        ("POST", ["tasks", id, "jira"]) => task_jira(app, tid(id)?, body),
        ("POST", ["tasks", id, "meta"]) => {
            let t = tid(id)?;
            app.db.tx(|| {
                board::get_task(app, t)?;
                board::update_task(app, t, fields!["meta" => jdumps(&meta(body.get("meta"))?)])?;
                board::bump_ctx(app, t)
            })?;
            task_detail(app, t)
        }
        ("POST", ["tasks", id, "attachments"]) => {
            let t = tid(id)?;
            app.db.tx(|| {
                board::get_task(app, t)?;
                board::add_attachment(app, OWNER, &body_str(body, "url"), &body_str(body, "title"), &body_str(body, "kind"), Some(t), None)
            })
        }
        ("POST", ["tasks", id, "delete"]) => delete_task(app, tid(id)?),
        ("POST", ["tasks", id, "pr", "wait"]) => pr_wait(app, tid(id)?),
        ("POST", ["tasks", id, "pr", "skip-checks"]) => pr_skip_checks(app, tid(id)?, body),
        ("POST", ["tasks", id, "pr", "merged"]) => pr_merged(app, tid(id)?, body),
        ("POST", ["tasks", id, "pr", "reply"]) => crate::prcmds::reply(app, tid(id)?, body),
        ("POST", ["tasks", id, "pr", "ack"]) => crate::prcmds::ack(app, tid(id)?, body),
        ("POST", ["tasks", id, "pr", "addressed"]) => crate::prcmds::addressed(app, tid(id)?, body),
        ("POST", ["tasks", id, "pr", "merge"]) => crate::prcmds::merge(app, tid(id)?, body),
        ("POST", ["tasks", id, "pr", "not-ours"]) => crate::prcmds::not_ours(app, tid(id)?, body),
        ("POST", ["tasks", id, "pr", "reviewers"]) => crate::asks::pr_reviewers(app, tid(id)?, body),
        ("POST", ["tasks", id, "pr", "reviewed"]) => {
            let t = tid(id)?;
            app.db.tx(|| {
                board::get_task(app, t)?;
                prflow::merge_flow(app, t, fields!["reviewed" => now_iso()])?;
                alerts::clear_alerts(app, Some(t), None)?;
                alerts::prune_alerts(app)
            })?;
            crate::asks::after_reviewed(app, t)?;
            task_detail(app, t)
        }
        ("POST", ["done", "close-terminals"]) => close_done_terminals(app, body, query),
        ("GET", ["goals"]) => {
            let project = q(query, "project", "all");
            let archived = as_bool(query.get("archived").map(|s| json!(s)).as_ref(), false);
            let mut out = vec![];
            for g in app.db.q("SELECT * FROM goals ORDER BY id", p![])? {
                if project_match(g.s("project"), project) && (archived || !g.b("archived")) {
                    out.push(board::goal_dict(app, &g)?);
                }
            }
            Ok(json!({"goals": out}))
        }
        ("POST", ["goals"]) => {
            let g = app.db.tx(|| new_goal(app, body))?;
            goal_detail(app, g["id"].as_i64().unwrap_or(0))
        }
        ("GET", ["goals", id]) => goal_detail(app, gid(id)?),
        ("POST", ["goals", id]) => patch_goal(app, gid(id)?, body),
        (_, ["goals", id, "devices", rest @ ..]) => crate::devices::goal_route(app, method, gid(id)?, rest, body),
        ("POST", ["goals", id, "attachments"]) => {
            let g = gid(id)?;
            app.db.tx(|| {
                board::get_goal(app, g)?;
                board::add_attachment(app, OWNER, &body_str(body, "url"), &body_str(body, "title"), &body_str(body, "kind"), None, Some(g))
            })
        }
        ("POST", ["goals", id, "delete"]) => delete_goal(app, gid(id)?, body),
        ("POST", ["goals", id, "order"]) => goal_order(app, gid(id)?, body),
        ("POST", ["goals", id, "notes"]) => goal_note(app, gid(id)?, body),
        ("POST", ["goals", id, "notes", nid]) => goal_note_edit(app, gid(id)?, nid, body),
        ("POST", ["goals", id, "plan"]) => goal_plan(app, gid(id)?, body),
        ("POST", ["goals", id, "run"]) => run_goal(app, gid(id)?, body),
        ("POST", ["goals", id, "waves", n]) => {
            let (g, n) = (gid(id)?, wave_n(n)?);
            app.db.tx(|| {
                board::get_goal(app, g)?;
                let mut f = vec![];
                if body.get("name").is_some() {
                    let name = one_line(&body_str(body, "name"), 80);
                    f.push(("name", if name.is_empty() { Value::Null } else { json!(name) }));
                }
                if body.get("stop_after").is_some() {
                    // A review stop is the owner's word: only the app's checkbox sets it.
                    if !from_app(query) {
                        return err(403, "Only the owner sets a review stop, from the goal page in the app. Hold a wave with tb goal wave --hold instead.");
                    }
                    f.push(("stop_after", json!(as_bool(body.get("stop_after"), false) as i64)));
                }
                crate::waves::set_wave(app, g, n, f)
            })?;
            goal_detail(app, g)
        }
        ("POST", ["goals", id, "waves", n, "hold"]) => {
            let (g, n) = (gid(id)?, wave_n(n)?);
            let who = { let w = body_str(body, "who"); if w.is_empty() { OWNER.to_string() } else { w } };
            app.db.tx(|| crate::waves::hold(app, g, n, as_bool(body.get("on"), true), &who))?;
            goal_detail(app, g)
        }
        ("POST", ["goals", id, "waves", n, "continue"]) => {
            let (g, n) = (gid(id)?, wave_n(n)?);
            let who = { let w = body_str(body, "who"); if w.is_empty() { OWNER.to_string() } else { w } };
            let started = app.db.tx(|| crate::waves::release(app, g, n, &who))?;
            let mut d = goal_detail(app, g)?;
            d["let_start"] = json!(started);
            Ok(d)
        }
        (_, ["devices", rest @ ..]) => crate::devices::route(app, method, rest, body),
        (_, ["bits", rest @ ..]) => crate::bits::route(app, method, rest, query, body),
        (_, ["reviewers", rest @ ..]) => crate::reviewers::route(app, method, rest, query, body),
        ("POST", ["attachments", id]) => edit_attachment(app, id, body),
        ("POST", ["attachments", id, "remove"]) => remove_attachment(app, id),
        ("GET", ["backlog"]) => list_backlog(app, query),
        ("POST", ["backlog"]) => post_issue(app, body),
        ("POST", ["backlog", "bulk"]) => backlog_bulk(app, body),
        ("GET", ["backlog", "triage"]) => triage::list(app, q(query, "project", "all")),
        ("GET", ["backlog", "plan"]) => Ok(triage::plan_get(app)),
        ("POST", ["backlog", "plan"]) => triage::plan_start(app, body),
        ("POST", ["backlog", "goal"]) => triage::create_goal(app, body),
        ("GET", ["backlog", id]) => issue_detail(app, iid(id)?),
        ("POST", ["backlog", id]) => app.db.tx(|| patch_issue(app, iid(id)?, body)),
        ("POST", ["backlog", id, "promote"]) => app.db.tx(|| promote(app, iid(id)?, body)),
        ("POST", ["backlog", id, "ticket"]) => app.db.tx(|| ticket(app, iid(id)?, body)),
        ("POST", ["backlog", id, "drop"]) => app.db.tx(|| drop_issue(app, iid(id)?, body)),
        ("POST", ["backlog", id, "reopen"]) => app.db.tx(|| reopen_issue(app, iid(id)?, body)),
        ("POST", ["backlog", id, "move"]) => app.db.tx(|| move_issue(app, iid(id)?, body)),
        ("POST", ["backlog", id, "note"]) => {
            let i = iid(id)?;
            let text = required(body, "text", 2000, "The note")?;
            app.db.tx(|| {
                board::get_issue(app, i)?;
                board::add_issue_event(app, i, OWNER, "note", &format!("Added a note: “{text}”"), None)
            })?;
            issue_detail(app, i)
        }
        ("POST", ["report"]) => reports::handle(app, body.clone(), false),
        ("GET", ["days"]) => days::page(app, query.get("date").map(|s| s.as_str()), query.get("hide").map(|s| s.as_str())),
        ("GET", ["history"]) => days::history(app),
        ("POST", ["history"]) => days::set_history(app, body),
        ("POST", ["history", "cleanup"]) => days::cleanup(app),
        ("GET", ["qa"]) => qa::settings(app),
        ("POST", ["qa"]) => app.db.tx(|| qa::set_settings(app, body)),
        ("POST", ["jira", "comment"]) => qa::intake(app, body),
        ("GET", ["jira"]) => crate::jira_desk::state(app),
        ("GET", ["jira", "jobs", id]) => {
            let n = need_ref(&json!(id), "job")?;
            match app.db.q1("SELECT * FROM jobs WHERE id = ? AND kind = 'jira'", p![n])? {
                Some(j) => Ok(board::job_dict(&j)),
                None => err(404, format!("There's no Jira job {}.", rf("job", n))),
            }
        }
        ("POST", ["jira", "jobs", id]) => crate::jira_desk::report(app, need_ref(&json!(id), "job")?, body),
        ("GET", ["qa-comments"]) => {
            let limit = q(query, "limit", "50").parse::<i64>().unwrap_or(50).clamp(1, 200);
            Ok(json!({"on": qa::on(app), "comments": qa::listing(app, limit, as_bool(query.get("waiting").map(|s| json!(s)).as_ref(), false))?}))
        }
        ("GET", ["qa-comments", id]) => Ok(qa::card(app, &qa::get(app, need_ref(&json!(id), "qa")?)?)),
        ("POST", ["qa-comments", id]) => {
            let cid = need_ref(&json!(id), "qa")?;
            let who = { let w = body_str(body, "who"); if w.is_empty() { OWNER.to_string() } else { w } };
            let note = clip(&body_str(body, "note"), 2000);
            let pr = body.get("pr").filter(|v| !v.is_null()).map(|v| as_bool(Some(v), true));
            let t = app.db.tx(|| qa::resolve(app, &qa::get(app, cid)?, &body_str(body, "action"), &who, if note.is_empty() { None } else { Some(&note) }, pr))?;
            let mut out = qa::card(app, &qa::get(app, cid)?);
            out["started"] = rf_opt("task", t.map(|t| t.id()));
            Ok(out)
        }
        ("GET", ["hours"]) => Ok(hours::state(app)),
        ("POST", ["hours"]) => set_hours(app, body),
        ("GET", ["keep-awake"]) => keep_awake::call(app, None),
        ("POST", ["keep-awake"]) => keep_awake::call(app, Some(body)),
        ("GET", ["usage"]) => Ok(usage::state(app)),
        ("GET", ["limits"]) => Ok(crate::limits::state(app)),
        ("POST", ["limits"]) => {
            crate::limits::set(app, body)?;
            app.defer(Box::new(|a: &App| {
                if let Err(e) = crate::gitattrs::sync(a) {
                    a.info(format!("attributes: {e}"));
                }
            }));
            Ok(crate::limits::state(app))
        }
        ("GET", ["prs", "feed"]) => Ok(crate::feed::health(app)),
        ("POST", ["prs", "event"]) => crate::feed::intake(app, body),
        ("POST", ["prs", "heartbeat"]) => app.db.tx(|| crate::feed::heartbeat(app, body)),
        (_, ["master", rest @ ..]) => crate::breaks::route(app, method, rest, body),
        ("GET", ["pr-builds"]) => Ok(crate::prbuilds::status(app)),
        ("POST", ["pr-builds"]) => app.db.tx(|| crate::prbuilds::set(app, body)),
        ("POST", ["prs", "refresh"]) => {
            let changed = prflow::refresh(app)?;
            crate::runner::reviews(app)?;
            Ok(json!({"ok": true, "changed": changed, "checked_at": app.shared.lock().prs_checked_at}))
        }
        ("POST", ["alerts"]) => {
            let text = required(body, "text", 300, "The alert")?;
            let task = match body.get("task").filter(|v| !v.is_null()) {
                Some(v) => Some(board::get_task(app, need_ref(v, "task")?)?.id()),
                None => None,
            };
            let goal = match body.get("goal").filter(|v| !v.is_null()) {
                Some(v) => Some(board::get_goal(app, need_ref(v, "goal")?)?.id()),
                None => None,
            };
            let key = one_line(&body_str(body, "key"), 120);
            let a = app.db.tx(|| alerts::raise(app, &text, task, goal, (!key.is_empty()).then_some(key.as_str()), as_bool(body.get("urgent"), false)))?;
            Ok(json!({"alert": a}))
        }
        ("POST", ["alerts", id, "clear"]) => {
            let found = alerts::alerts(app).into_iter().find(|a| a["id"] == *id || a["key"] == *id);
            let Some(a) = found else { return err(404, "There's no alert with that id or key.") };
            app.db.tx(|| alerts::clear_alerts(app, None, a["id"].as_str()))?;
            Ok(json!({"alerts": alerts::listing(app)}))
        }
        ("POST", ["alerts", id, "dismiss"]) => {
            if let Some(a) = alerts::alerts(app).into_iter().find(|a| a["id"] == *id && alerts::stays(a)) {
                return err(409, if a["urgent"] == true { "An urgent alert stays until it clears." } else { "A PR waiting for your review stays until you review it." });
            }
            app.db.tx(|| alerts::clear_alerts(app, None, Some(id)))?;
            Ok(json!({"alerts": alerts::listing(app)}))
        }
        ("POST", ["alerts", id, "snooze"]) => {
            let mins = body["mins"].as_i64().unwrap_or(0);
            let allowed: Vec<i64> = app.cfg.alerts.snooze_mins.iter().copied().filter(|m| *m > 0).collect();
            if !allowed.contains(&mins) && ![15, 30, 60].contains(&mins) {
                return err(400, format!("Snooze for {} minutes.", allowed.iter().map(|m| m.to_string()).collect::<Vec<_>>().join(", ")));
            }
            let a = app.db.tx(|| alerts::snooze_alert(app, id, mins))?;
            Ok(json!({"alert": a}))
        }
        _ => {
            let known = ["state", "summary", "projects", "sessions", "whoami", "steps", "jobs", "tasks", "done", "goals", "attachments", "backlog", "report", "hours", "keep-awake", "limits", "usage", "prs", "alerts"];
            if segs.first().map(|s| known.contains(s)).unwrap_or(false) && (method == "GET" || method == "POST") {
                return err(404, "There's nothing at that address.");
            }
            err(404, "There's nothing at that address.")
        }
    };
    if method == "POST" {
        app.wake_runner();
    }
    r
}

const DONE_SHOWN_MAX: usize = 200;

fn get_state(app: &App, query: &Query) -> Result<Value> {
    let project = q(query, "project", "all");
    let goal = q(query, "goal", "all");
    let keep: Vec<i64> = q(query, "keep", "").split(',').filter(|k| !k.trim().is_empty()).filter_map(|k| parse_ref_str(k, "issue").ok().flatten()).collect();
    let mut cols: HashMap<&str, Vec<(Value, String)>> = HashMap::new();
    for k in ["queued", "working", "needs", "done"] {
        cols.insert(k, vec![]);
    }
    for t in app.db.q("SELECT * FROM tasks WHERE status != 'planned'", p![])? {
        if !project_match(t.s("project"), project) || !task_goal_match(app, &t, goal)? {
            continue;
        }
        let st = t.st("status");
        if let Some(col) = cols.get_mut(st.as_str()) {
            let c = board::task_card(app, &t)?;
            col.push((c, t.st("created_at")));
        }
    }
    let mut out_cols = serde_json::Map::new();
    let window = match q(query, "done", "24h") {
        "7d" => Some(7.0 * 86400.0),
        "all" => None,
        _ => Some(86400.0),
    };
    let mut done_hidden = 0;
    let mut counts = serde_json::Map::new();
    for (k, mut v) in cols {
        if k == "queued" {
            v.sort_by(|a, b| {
                (a.0["priority"] != "high", &a.1, a.0["id"].as_i64()).cmp(&(b.0["priority"] != "high", &b.1, b.0["id"].as_i64()))
            });
        } else {
            v.sort_by(|a, b| b.0["when"].as_str().unwrap_or("").cmp(a.0["when"].as_str().unwrap_or("")));
        }
        let mut cards: Vec<Value> = v.into_iter().map(|(c, _)| c).collect();
        if k == "done" {
            let all = cards.len();
            cards.retain(|c| window.map(|w| wall_age_secs(c["when"].as_str()).unwrap_or(0.0) <= w).unwrap_or(true));
            cards.truncate(DONE_SHOWN_MAX);
            done_hidden = all - cards.len();
        } else {
            counts.insert(k.into(), json!(cards.len()));
        }
        out_cols.insert(k.into(), Value::Array(cards));
    }
    let mut open_issues = vec![];
    for b in app.db.q("SELECT * FROM issues WHERE state = 'open' ORDER BY created_at DESC, id DESC", p![])? {
        if project_match(b.s("project"), project) && goal_match(b.i("goal_id"), goal)? {
            open_issues.push(b);
        }
    }
    let mut shown: Vec<Row> = open_issues.iter().take(4).cloned().collect();
    for k in keep {
        if shown.iter().any(|b| b.id() == k) {
            continue;
        }
        if let Some(b) = board::find_issue(app, Some(k))? {
            shown.push(b);
        }
    }
    out_cols.insert("backlog".into(), Value::Array(shown.iter().map(|b| board::issue_card(app, b)).collect::<Result<Vec<_>>>()?));
    out_cols.insert("backlog_open".into(), json!(open_issues.len()));
    let mut goals = vec![];
    for g in app.db.q("SELECT * FROM goals WHERE archived = 0 ORDER BY id", p![])? {
        if project_match(g.s("project"), project) {
            goals.push(board::goal_dict(app, &g)?);
        }
    }
    let sessions = ops::session_list(app, project)?;
    let mut sp: Vec<String> = ops::session_list(app, "all")?.iter().filter_map(|s| s["project"].as_str().map(|x| x.to_string())).collect();
    sp.sort();
    sp.dedup();
    counts.insert("done_hidden".into(), json!(done_hidden));
    counts.insert("open_issues".into(), json!(open_issues.len()));
    counts.insert("untriaged".into(), json!(app.db.count("SELECT COUNT(*) FROM issues WHERE state = 'open' AND goal_id IS NULL", p![])?));
    let mut planned = vec![];
    for t in app.db.q("SELECT * FROM tasks WHERE status = 'planned' ORDER BY goal_id, position, id", p![])? {
        if project_match(t.s("project"), project) && task_goal_match(app, &t, goal)? {
            planned.push(board::task_card(app, &t)?);
        }
    }
    let mut out = json!({
        "now": now_iso(), "midna": midna::status(app), "jira": {"enabled": app.cfg.jira_on(), "site": app.cfg.jira.site},
        "owner": app.cfg.owner,
        "alerts": alerts::listing(app), "work_hours": hours::state(app), "usage": usage::state(app),
        "keep_awake": app.shared.lock().midna_keep_awake.clone(),
        "accounts": accounts::attention(&app.cfg),
        "prs_checked_at": app.shared.lock().prs_checked_at,
        "pr_feed": crate::feed::health(app),
        "pr_builds": crate::prbuilds::status(app),
        "master": crate::breaks::banner(app)?,
        "projects": projects::list_projects(app)?, "sessions": sessions, "session_projects": sp, "goals": goals,
        "columns": Value::Object(out_cols), "counts": Value::Object(counts), "planned": planned,
    });
    if let Some(sid) = query.get("session") {
        out["mine"] = match board::task_for_session(app, Some(sid))? {
            Some(t) => board::task_card(app, &t)?,
            None => Value::Null,
        };
    }
    Ok(out)
}

fn get_summary(app: &App) -> Result<Value> {
    let n = |s: &str| app.db.count("SELECT COUNT(*) FROM tasks WHERE status = ?", p![s]);
    let needs = n("needs")?;
    let mut goals = vec![];
    for g in app.db.q("SELECT * FROM goals WHERE archived = 0 ORDER BY id", p![])? {
        let c = board::goal_counts(app, g.id())?;
        let (done, total) = (c["done"].as_i64().unwrap_or(0), c["total"].as_i64().unwrap_or(0));
        if total > 0 && done < total {
            goals.push(json!({"id": g.id(), "ref": rf("goal", g.id()), "name": g.v("name"), "done": done, "total": total}));
        }
    }
    Ok(json!({"now": now_iso(), "queued": n("queued")?, "working": n("working")?, "needs": needs,
              "open_issues": app.db.count("SELECT COUNT(*) FROM issues WHERE state = 'open'", p![])?,
              "attention": needs > 0, "midna_up": midna::up(app), "goals": goals}))
}

/// `tb project agents-merge on|off|default`: whether agents merge on projects that don't say.
fn set_agents_merge(app: &App, body: &Value) -> Result<Value> {
    let on = match body.get("agents_merge") {
        Some(Value::Bool(b)) => Some(*b),
        Some(Value::String(s)) if s == "on" => Some(true),
        Some(Value::String(s)) if s == "off" => Some(false),
        Some(Value::Null) => None,
        _ => return err(400, "agents_merge is true (on), false (off) or null (config.toml's pr.agents_merge)."),
    };
    app.db.tx(|| projects::set_agents_merge(app, on))?;
    Ok(projects::describe_agents_merge(app))
}

fn patch_project(app: &App, name: &str, body: &Value) -> Result<Value> {
    let list = projects::list_projects(app)?;
    let Some(p) = list.iter().find(|p| p["name"] == name) else { return err(404, format!("There's no project called {name}.")) };
    let rules = projects::PR_RULE_KEYS.iter().any(|k| body.get(*k).is_some());
    let flow = body.get("pr_flow").filter(|v| !v.is_null()).map(|_| body_str(body, "pr_flow"));
    if flow.is_none() && !rules {
        return err(400, format!("Say what to change: pr_flow, or the PR rules {}.", projects::PR_RULE_KEYS.join(", ")));
    }
    if flow.as_ref().is_some_and(|f| !projects::PR_FLOWS.contains(&f.as_str())) {
        return err(400, "pr_flow is auto (by its git remote), on or off.");
    }
    app.db.tx(|| {
        if let Some(f) = &flow {
            projects::set_pr_flow(app, name, f)?;
        }
        projects::set_pr_rules(app, name, body).map(|_| ())
    })?;
    projects::describe(app, p)
}

fn close_session_inner(app: &App, s: &Row, force: bool, why: &str) -> Result<Option<i64>> {
    if crate::jira_desk::is_desk(app, s.s("id"))? && board::task_for_session(app, s.s("id"))?.is_none() {
        return err(409, "The Jira desk stays open: the board sends it every Jira job.");
    }
    match board::close_rule(Some(s)) {
        None => return err(404, "That terminal isn't open any more."),
        Some("force") if !force => return err(409, "It's busy. Use Force close to stop what it's doing."),
        _ => {}
    }
    let sid = s.st("id");
    if crate::jobs::closing(app, &sid)? {
        return Ok(None);
    }
    let t = board::task_for_session(app, Some(&sid))?;
    let jid = board::create_job(app, "close", json!({"session": sid, "force": force, "reason": format!("Task board: {why}")}), t.map(|t| t.id()), "close", None)?;
    board::session_event(app, &sid, "close", if force { "You asked to force close it" } else { "You asked to close it" }, None)?;
    Ok(Some(jid))
}

fn close_session(app: &App, id: &str, body: &Value) -> Result<Value> {
    app.db.tx(|| {
        let Some(s) = board::get_session(app, Some(id))? else { return err(404, "That terminal isn't open any more.") };
        close_session_inner(app, &s, as_bool(body.get("force"), false), "you pressed Close on the Sessions page")?;
        Ok(json!({"ok": true, "session": id}))
    })
}

fn close_sessions(app: &App, body: &Value) -> Result<Value> {
    let ids = body["ids"].as_array().cloned().unwrap_or_default();
    if ids.is_empty() {
        return err(400, "Pick at least one terminal.");
    }
    app.db.tx(|| {
        let (mut closing, mut skipped) = (vec![], vec![]);
        for v in ids.iter().take(100) {
            let sid = v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string());
            let s = board::get_session(app, Some(&sid))?;
            match s {
                Some(s) if board::close_rule(Some(&s)) == Some("close") && !crate::lines::busy(app, &sid)? => {
                    if close_session_inner(app, &s, false, "close the idle terminals you picked")?.is_some() {
                        closing.push(sid);
                    }
                }
                _ => skipped.push(sid),
            }
        }
        Ok(json!({"ok": true, "closing": closing, "skipped": skipped}))
    })
}

fn focus_session(app: &App, id: &str) -> Result<Value> {
    app.db.tx(|| {
        let s = board::get_session(app, Some(id))?.filter(|s| s.s("status") != Some("gone"));
        if s.is_none() {
            return err(404, "That terminal isn't open any more.");
        }
        board::create_job(app, "focus", json!({"session": id}), None, "focus", None)?;
        Ok(json!({"ok": true}))
    })
}

fn rename_session(app: &App, id: &str, body: &Value) -> Result<Value> {
    let name = one_line(&required(body, "name", 400, "The name")?, 400);
    if name.chars().count() > 80 {
        return err(400, "Keep the name to 80 characters or fewer.");
    }
    app.db.tx(|| {
        let Some(s) = board::get_session(app, Some(id))?.filter(|s| s.s("status") != Some("gone")) else {
            return err(404, "That terminal isn't open any more.");
        };
        app.db.x(
            "UPDATE jobs SET state = 'cancelled', updated_at = ? WHERE kind = 'rename' AND state = 'pending' AND json_extract(args, '$.session') = ?",
            p![now_iso(), id],
        )?;
        if name != s.st("name") {
            board::create_job(app, "rename", json!({"session": id, "name": name}), None, "rename", None)?;
        }
        Ok(json!({"ok": true, "session": id, "name": name}))
    })
}

fn reopen_session(app: &App, id: &str) -> Result<Value> {
    app.db.tx(|| {
        let s = board::get_session(app, Some(id))?.filter(|s| s.s("status") == Some("gone") && board::runs_claude(s));
        let Some(s) = s else { return err(404, "That isn't a closed Claude terminal.") };
        if !has(s.s("claude_session_id")) {
            return err(409, "The board never learned this terminal's Claude conversation, so it can't reopen it.");
        }
        if !s.s("project_path").map(|p| Path::new(p).is_dir()).unwrap_or(false) {
            return err(409, "The board doesn't know this terminal's folder any more.");
        }
        board::create_job(
            app,
            "agent",
            json!({"cwd": s.v("project_path"), "title": short(s.s("name").unwrap_or("Reopened"), 40),
                   "prompt": format!("This terminal was closed and has just been reopened from the task board. In one or two lines, say where you'd got to, then wait for {}.", app.cfg.owner),
                   "flags": format!("--resume {}", s.st("claude_session_id"))}),
            None,
            "reopen",
            None,
        )?;
        Ok(json!({"ok": true, "session": id}))
    })
}

fn list_tasks(app: &App, query: &Query) -> Result<Value> {
    let (status, project, goal) = (q(query, "status", "all"), q(query, "project", "all"), q(query, "goal", "all"));
    let mut out = vec![];
    for t in app.db.q("SELECT * FROM tasks ORDER BY id DESC", p![])? {
        if (status == "all" || t.s("status") == Some(status)) && project_match(t.s("project"), project) && task_goal_match(app, &t, goal)? {
            out.push(board::task_card(app, &t)?);
        }
    }
    Ok(json!({"tasks": out}))
}

fn get_log(app: &App, id: i64, kind: &str) -> Result<Value> {
    board::get_task(app, id)?;
    let log: Vec<Value> = app
        .db
        .q("SELECT * FROM events WHERE task_id = ? ORDER BY at DESC, id DESC", p![id])?
        .iter()
        .filter(|e| kind == "all" || e.s("kind") == Some(kind))
        .map(|e| json!({"at": e.v("at"), "who": e.v("who"), "kind": e.v("kind"), "text": e.v("text"),
                        "data": if has(e.s("data")) { serde_json::from_str(&e.st("data")).unwrap_or(Value::Null) } else { Value::Null }}))
        .collect();
    Ok(json!({"log": log}))
}

fn meta(v: Option<&Value>) -> Result<Value> {
    let Some(Value::Array(a)) = v else { return err(400, "Details must be a list of [name, value] pairs.") };
    let mut out = vec![];
    for p in a {
        let (k, val) = match p {
            Value::Array(x) if x.len() == 2 => (x[0].clone(), x[1].clone()),
            Value::Object(o) => (o.get("k").or(o.get("name")).cloned().unwrap_or(Value::Null), o.get("v").or(o.get("value")).cloned().unwrap_or(Value::Null)),
            _ => return err(400, "Details must be a list of [name, value] pairs."),
        };
        let s = |v: Value| match v {
            Value::String(s) => s.trim().to_string(),
            Value::Null => String::new(),
            o => o.to_string(),
        };
        let (k, val) = (s(k), s(val));
        if !k.is_empty() {
            out.push(json!([clip(&k, 80), clip(&val, 500)]));
        }
    }
    Ok(Value::Array(out))
}

fn patch_task(app: &App, id: i64, body: &Value) -> Result<Value> {
    let mut warnings: Vec<String> = vec![];
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        let who = { let w = body_str(body, "who"); if w.is_empty() { OWNER.to_string() } else { w } };
        crate::devices::take_needs(app, "task", id, body, &who)?;
        crate::bits::take_task_bits(app, id, body, &who)?;
        let mut f: Vec<(&str, Value)> = vec![];
        let mut bump = false;
        let has_key = |k: &str| body.get(k).is_some();
        if has_key("title") {
            f.push(("title", json!(required(body, "title", 300, "The title")?)));
            bump = true;
        }
        if has_key("detail") {
            f.push(("detail", json!(body_str(body, "detail"))));
            bump = true;
        }
        if has_key("priority") {
            let pr = body_str(body, "priority");
            if pr != "normal" && pr != "high" {
                return err(400, "Priority must be normal or high.");
            }
            f.push(("priority", json!(pr)));
        }
        let mut new_goal_id: Option<Option<i64>> = None;
        if has_key("goal_id") {
            let mut g = opt_goal(body, "goal_id")?;
            if g.is_none() {
                // Out of its home goal: it runs in the newest other goal it finishes, if any.
                g = shared::next_home(app, id, t.i("goal_id"))?.or(g);
            }
            if g != t.i("goal_id") {
                if let Some(g) = g {
                    board::get_goal(app, g)?;
                }
                f.push(("goal_id", json!(g)));
                f.push(("position", board::next_position(app, g)?));
                new_goal_id = Some(g);
                bump = true;
            }
        }
        let also = shared::clean(app, body.get("also"), Some(&t))?;
        let not_also = shared::clean(app, body.get("not_also"), Some(&t))?;
        if let Some(g) = new_goal_id {
            shared::moved_home(app, id, g)?;
        }
        if has_key("meta") {
            f.push(("meta", json!(jdumps(&meta(body.get("meta"))?))));
            bump = true;
        }
        let mut wave_changed = None;
        if has_key("wave") {
            let w = ops::wave_of(&body["wave"])?;
            if w.is_some() && t.i("goal_id").is_none() && new_goal_id.flatten().is_none() {
                return err(400, "Only a task in a goal has a wave.");
            }
            if w != t.i("wave") {
                f.push(("wave", json!(w)));
                wave_changed = Some(w);
                bump = true;
            }
        }
        if has_key("auto_close") {
            f.push(("auto_close", json!(as_bool(body.get("auto_close"), true) as i64)));
        }
        if has_key("jira_sync") {
            f.push(("jira_sync", json!(as_bool(body.get("jira_sync"), true) as i64)));
        }
        if let Some(p) = body.get("position").and_then(|v| v.as_f64()) {
            f.push(("position", json!(p)));
        }
        let mut new_status = None;
        if let Some(want) = body.get("status").and_then(|v| v.as_str()) {
            if Some(want) != t.s("status") {
                let pair = [t.st("status"), want.to_string()];
                if !(pair.contains(&"planned".to_string()) && pair.contains(&"queued".to_string())) {
                    return err(409, "Only planned and queued can be switched here.");
                }
                f.push(("status", json!(want)));
                new_status = Some(want.to_string());
            }
        }
        if has_key("pickup") {
            let (mode, sid) = ops::pickup(body)?;
            f.push(("pickup_session", json!(if mode == "attach" { sid } else { None })));
            f.push(("pickup", json!(mode)));
        }
        let mut waits_changed = None;
        if has_key("waits_for") {
            let wf = crate::waitsfor::clean(app, &body["waits_for"], Some(&t))?;
            if wf.as_deref() != t.s("waits_for") {
                waits_changed = Some(wf.clone());
                f.push(("waits_for", json!(wf)));
                bump = true;
            }
        }
        let mut locks_changed = None;
        if has_key("locks") {
            let lk = crate::locks::clean(body.get("locks"))?;
            if lk.as_deref() != t.s("locks") {
                warnings.extend(crate::locks::unseen_warning(app, lk.as_deref(), Some(id))?);
                locks_changed = Some(lk.clone());
                f.push(("locks", json!(lk)));
                bump = true;
            }
        }
        let mut alone_changed = None;
        if has_key("alone") {
            let al = crate::locks::clean_alone(body.get("alone"))?;
            if al.as_deref() != t.s("alone") {
                alone_changed = Some(al.clone());
                f.push(("alone", json!(al)));
                bump = true;
            }
        }
        if has_key("jira_key") && app.cfg.jira_on() {
            let k = body_str(body, "jira_key");
            if k.is_empty() || k.eq_ignore_ascii_case("none") {
                f.push(("jira_key", Value::Null));
                f.push(("jira_status", Value::Null));
                f.push(("jira_none", json!(1)));
            } else if k.eq_ignore_ascii_case("new") {
                f.push(("jira_none", json!(0)));
                if !has(t.s("jira_key")) && !jira::ticket_asked(app, id)? {
                    jira::request_create_for_task(app, &t)?;
                    board::log_event(app, id, OWNER, "jira", "Asked Jira for a ticket")?;
                }
            } else {
                let key = ops::jira_key(&k)?;
                f.push(("jira_none", json!(0)));
                if Some(key.as_str()) != t.s("jira_key") {
                    f.push(("jira_key", json!(key)));
                    f.push(("jira_status", Value::Null));
                    jira::request(app, "status", id, &key, None, None)?;
                    board::log_event(app, id, OWNER, "jira", &format!("Linked {key}"))?;
                    bump = true;
                }
            }
        }
        if !also.is_empty() || !not_also.is_empty() {
            let who = { let w = body_str(body, "who"); if w.is_empty() { OWNER.to_string() } else { w } };
            let mut nt = t.clone();
            if let Some(g) = new_goal_id {
                nt.insert("goal_id".into(), json!(g));
            }
            let dropped = shared::drop(app, &nt, &not_also, &who)?;
            let added = shared::add(app, &nt, &also, &who)?;
            if !dropped.is_empty() || !added.is_empty() {
                if f.is_empty() {
                    return board::bump_ctx(app, id);
                }
                bump = true;
            }
        }
        // Stacking (`--stack-on`) and the PR plan (`--pr yes|no`) log their own change.
        let mut planned = false;
        if has_key("stack_on") {
            planned |= crate::stack::set(app, &t, &body["stack_on"], OWNER)?;
        }
        if has_key("ships_pr") {
            planned |= projects::set_task_ships_pr(app, &board::get_task(app, id)?, &body["ships_pr"], OWNER)?;
        }
        if planned && f.is_empty() {
            return board::bump_ctx(app, id);
        }
        if f.is_empty() {
            return Ok(());
        }
        if planned {
            bump = true;
        }
        let edited = f.iter().any(|(k, _)| matches!(*k, "title" | "detail" | "meta"));
        board::update_task(app, id, f)?;
        if bump {
            board::bump_ctx(app, id)?;
        }
        if let Some(st) = new_status {
            board::log_event(app, id, OWNER, "status", if st == "queued" { "Queued" } else { "Moved back to planned" })?;
        } else if let Some(g) = new_goal_id {
            let name = match g {
                Some(g) => format!("Moved to the goal “{}”", board::get_goal(app, g)?.st("name")),
                None => "Taken out of its goal".into(),
            };
            board::log_event(app, id, OWNER, "status", &name)?;
        } else if edited {
            board::log_event(app, id, OWNER, "note", "Edited the task")?;
        }
        if let Some(w) = wave_changed {
            board::log_event(app, id, OWNER, "note", &match w { Some(n) => format!("In wave {n}"), None => "In no wave".into() })?;
        }
        if let Some(wf) = waits_changed {
            let names: Vec<String> = jloads_arr(wf.as_deref()).iter().filter_map(|v| v.as_i64()).map(|n| rf("task", n)).collect();
            board::log_event(app, id, OWNER, "note", &if names.is_empty() { "Waits for no other task".to_string() } else { format!("Waits for {}", names.join(", ")) })?;
        }
        if let Some(lk) = locks_changed {
            let names: Vec<String> = jloads_arr(lk.as_deref()).iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
            board::log_event(app, id, OWNER, "note", &if names.is_empty() {
                "Holds no lock".to_string()
            } else {
                format!("Holds the lock{} {} while it runs", if names.len() > 1 { "s" } else { "" }, names.join(", "))
            })?;
        }
        if let Some(al) = alone_changed {
            board::log_event(app, id, OWNER, "note", crate::locks::alone_text(al.as_deref().unwrap_or("")))?;
        }
        if let Some(g) = t.i("goal_id") {
            board::renumber_goal(app, g)?;
        }
        if let Some(Some(g)) = new_goal_id {
            board::renumber_goal(app, g)?;
        }
        Ok(())
    })?;
    let mut d = task_detail(app, id)?;
    if !warnings.is_empty() {
        d["warnings"] = json!(warnings);
    }
    Ok(d)
}

fn live_session(app: &App, t: &Row) -> Result<Option<Row>> {
    if let Some(s) = board::get_session(app, t.s("session_id"))? {
        if s.s("status") != Some("gone") {
            return Ok(Some(s));
        }
    }
    let v = prflow::visitor(t);
    Ok(board::get_session(app, v.as_deref())?.filter(|s| s.s("status") != Some("gone")))
}

fn need_live(app: &App, t: &Row) -> Result<Row> {
    live_session(app, t)?.ok_or_else(|| ApiError::new(409, "This task has no open Midna terminal."))
}

/// Runs a step's hooks for the owner's action (outside its transaction). A stop is refused with its
/// reason; a skip of `task.starting` closes the task without running it (true).
fn gate(app: &App, id: i64, event: &str, stopped: &str) -> Result<bool> {
    let t = board::get_task(app, id)?;
    if t.s("status") == Some("done") {
        return Ok(false);
    }
    match hooks::gate(app, event, &t, json!({"by": OWNER})) {
        hooks::Decision::Go => Ok(false),
        d @ hooks::Decision::Skip { .. } => {
            app.db.tx(|| runner::skip_task(app, &t, &d))?;
            Ok(true)
        }
        d => {
            let line = d.said(stopped);
            app.db.tx(|| hooks::note(app, id, &line))?;
            err(409, format!("{line}."))
        }
    }
}

fn start(app: &App, id: i64, query: &Query, body: &Value) -> Result<Value> {
    let mode = { let m = body_str(body, "mode"); if m.is_empty() { "new".to_string() } else { m } };
    if !["new", "queue", "attach"].contains(&mode.as_str()) {
        return err(400, "Start mode must be new, queue or attach.");
    }
    // From `tb start` in a terminal: only on a human's word there. From the board's Start: the owner's,
    // and only the app's own request is that; any other caller with no terminal is no one's word.
    let t = board::get_task(app, id)?;
    let via = body_str(body, "via_session");
    let started = if via.is_empty() {
        if !from_app(query) {
            return err(
                403,
                format!(
                    "Only a human can start {}: {} can press Start on the board, or tell an agent in a terminal to start it (tb start).",
                    rf("task", id),
                    app.cfg.owner
                ),
            );
        }
        "Started in the UI".to_string()
    } else {
        if crate::startword::owners_word(app, &via, id)?.is_none() {
            // An ask for the goal ("start the goal") is no word for each of its tasks; the goal runs them.
            let goal = board::find_goal(app, t.i("goal_id"))?
                .map(|g| format!(" If they asked you to start its goal, run tb start {}.", rf("goal", g.id())))
                .unwrap_or_default();
            return err(
                403,
                format!(
                    "Only a human can start {}: nobody asked for it in this terminal's conversation. {} can press Start on the board, or tell you to start it.{goal}",
                    rf("task", id),
                    app.cfg.owner
                ),
            );
        }
        format!("Started by {} via {}", app.cfg.owner, board::session_name(app, Some(&via), None))
    };
    // Starting ahead of its wave or its turn is fine; ahead of the work it needs is not.
    if let Some(b) = crate::waitsfor::blocker(app, &t)? {
        return err(409, format!("{} can't start yet. {b}.", rf("task", id)));
    }
    if gate(app, id, "task.starting", "Stopped from starting")? {
        return task_detail(app, id);
    }
    if mode != "attach" {
        crate::worktrees::ensure(app, &board::get_task(app, id)?)?;
    }
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        if t.s("status") == Some("done") {
            return err(409, "This task is done. Queue it again first.");
        }
        if matches!(t.s("status"), Some("working") | Some("needs")) && live_session(app, &t)?.is_some() && !t.b("lost") {
            return err(409, format!("It's already on {}.", t.s("session_name").unwrap_or("a terminal")));
        }
        if board::live_start_job(app, &t)?.map(|j| matches!(j.s("state"), Some("pending") | Some("running"))).unwrap_or(false) {
            return err(409, "It's already starting in Midna.");
        }
        // The owner's Start takes it out of a terminal's line it waits in and runs it as asked.
        if let (Some(line), Some("queued")) = (t.s("line_session"), t.s("status")) {
            let name = board::session_name(app, Some(line), None);
            crate::lines::leave(app, &t)?;
            board::log_event(app, id, board::OWNER, "status", &format!("Taken out of {name}'s line"))?;
        }
        let sid = body["session_id"].as_str().map(|s| s.to_string());
        board::update_task(
            app,
            id,
            fields!["pickup" => mode, "pickup_session" => if mode == "attach" { sid.clone() } else { None }, "lost" => 0,
                    "session_id" => if t.b("lost") { None } else { t.s("session_id").map(|s| s.to_string()) }],
        )?;
        board::log_event(app, id, board::OWNER, "status", &started)?;
        runner::start_task(app, &board::get_task(app, id)?, &mode, sid.as_deref(), None, None, None)?;
        Ok(())
    })?;
    task_detail(app, id)
}

fn answer(app: &App, id: i64, body: &Value) -> Result<Value> {
    let text = required(body, "text", 4000, "The answer")?;
    let when = { let w = body_str(body, "when"); if w.is_empty() { "now".to_string() } else { w } };
    if when != "now" && when != "morning" {
        return err(400, "Send the answer now or in the morning.");
    }
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        let mut ctx = board::task_context(&t);
        let mut entry = format!("“{text}” ({})", local_clock(None));
        if let Some(q) = t.s("question").filter(|q| !q.is_empty()) {
            entry = format!("{} → {entry}", clip(q, 160));
        }
        let mut answers = str_list(ctx.get("answers"));
        answers.push(entry);
        ctx.insert("answers".into(), json!(answers));
        let s = live_session(app, &t)?;
        board::save_context(app, id, &ctx, s.is_none())?;
        let later = if when == "morning" { hours::until_open(app) } else { None };
        if later.is_none() {
            board::update_task(app, id, fields!["answered_at" => now_iso()])?;
        }
        let message = format!("[task-board:{}] {} answer: {text}", rf("task", id), capitalize(&app.cfg.owners()));
        let visiting = t.s("status") == Some("done") && (prflow::visitor(&t).is_some() || prflow::waking(app, &t)?);
        let asked_or_lost = t.s("status") == Some("needs") && matches!(t.s("needs_reason"), Some("question") | Some("lost"));
        let pickup = if matches!(t.s("pickup"), Some("manual") | Some("attach")) { "new".to_string() } else { t.st("pickup") };
        if let Some(later) = later {
            let at = hours::say_when(&later);
            board::log_event(app, id, OWNER, "answer", &format!("Answered: {text} (sends {at})"))?;
            if s.is_some() || visiting {
                deliver::add(app, "answer", &message, id, None, Some(&later))?;
            } else if asked_or_lost {
                board::update_task(
                    app,
                    id,
                    fields!["status" => "queued", "needs_reason" => null, "question" => null, "lost" => 0, "session_id" => null,
                            "pickup_session" => null, "pickup" => pickup, "latest" => format!("Answered; it starts {at} with your answer in its handoff.")],
                )?;
            }
        } else if s.is_some() {
            deliver::add(app, "answer", &message, id, None, None)?;
            board::log_event(app, id, OWNER, "answer", &format!("Answered: {text}"))?;
        } else {
            board::log_event(app, id, OWNER, "answer", &format!("Answered: {text}"))?;
            if asked_or_lost {
                board::update_task(
                    app,
                    id,
                    fields!["status" => "queued", "needs_reason" => null, "question" => null, "lost" => 0, "session_id" => null,
                            "latest" => "Answered; it starts again from its handoff."],
                )?;
            }
            if t.s("status") != Some("done") || visiting {
                deliver::answer_in_new_tab(app, &board::get_task(app, id)?, &message)?;
            }
        }
        Ok(())
    })?;
    task_detail(app, id)
}

/// The owner did (or skips) a step: the app's Done on a task waiting for one, or `tb step done --task T<n>`
/// from another terminal. A task waiting on that step carries on, as after an answer.
fn owner_step(app: &App, id: i64, body: &Value) -> Result<Value> {
    let t = board::get_task(app, id)?;
    let named = body_str(body, "name");
    let name = if named.is_empty() { steps::waiting(&t).unwrap_or_default() } else { named };
    if name.is_empty() {
        return err(400, format!("{} isn't waiting on a step; name one.", rf("task", id)));
    }
    let step = steps::find(app, &t, &name)?;
    let session = body_str(body, "session");
    if !session.is_empty() && t.s("session_id") == Some(session.as_str()) {
        return err(409, format!("This is the agent's terminal for {}. Record {} steps from the app or another terminal.", rf("task", id), app.cfg.owners()));
    }
    let skip = as_bool(body.get("skip"), false);
    let note = one_line(&body_str(body, "note"), 2000);
    let mut text = format!("Step {}: {}", if skip { "skipped" } else { "done" }, step.name);
    if !note.is_empty() {
        text += &format!(": {note}");
    }
    let resumes = steps::waiting(&t).map(|w| steps::key(&w) == steps::key(&step.name)).unwrap_or(false);
    app.db.tx(|| {
        board::log_event_full(app, id, OWNER, "step", &text, Some(json!({"name": step.name, "before": step.before.as_str(), "ok": true, "note": note, "skipped": skip, "head": crate::waitsfor::head_of(&t)})), None)?;
        let mut ctx = board::task_context(&t);
        if resumes && ctx.remove("step_waiting").is_some() {
            board::save_context(app, id, &ctx, false)?;
        }
        alerts::clear_alerts(app, Some(id), None)
    })?;
    if resumes {
        return answer(app, id, &json!({"text": format!("{text}. Carry on.")}));
    }
    task_detail(app, id)
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn cancel_pending(app: &App, id: i64) -> Result<()> {
    app.db.x(
        "UPDATE jobs SET state = 'expired', result = ?, updated_at = ? WHERE task_id = ? AND state = 'pending' \
         AND purpose IN ('start','offer') AND kind IN ('agent','message','deliver')",
        p![jdumps(&json!({"cancelled": true})), now_iso(), id],
    )?;
    deliver::drop(app, id, "the task was detached")
}

fn detach(app: &App, id: i64) -> Result<Value> {
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        if t.s("status") == Some("done") {
            return err(409, "This task is done.");
        }
        cancel_pending(app, id)?;
        crate::lines::leave(app, &t)?;
        let pickup = if t.s("pickup") == Some("attach") { "manual".to_string() } else { t.st("pickup") };
        board::update_task(
            app,
            id,
            fields!["session_id" => null, "status" => "queued", "needs_reason" => null, "question" => null, "lost" => 0,
                    "start_job" => null, "pickup" => pickup, "pickup_session" => null, "latest" => "Detached; back in the queue."],
        )?;
        let text = match t.s("session_name").filter(|n| !n.is_empty()) {
            Some(n) => format!("Detached from {n}; back in the queue"),
            None => "Back in the queue".into(),
        };
        board::log_event(app, id, OWNER, "status", &text)?;
        Ok(())
    })?;
    task_detail(app, id)
}

fn close_terminal(app: &App, id: i64, body: &Value) -> Result<Value> {
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        let s = need_live(app, &t)?;
        board::create_job(app, "close", json!({"session": s.v("id"), "force": as_bool(body.get("force"), false)}), Some(id), "close", None)?;
        Ok(())
    })?;
    task_detail(app, id)
}

fn focus_task(app: &App, id: i64) -> Result<Value> {
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        let s = need_live(app, &t)?;
        board::create_job(app, "focus", json!({"session": s.v("id")}), Some(id), "focus", None)?;
        Ok(json!({"ok": true}))
    })
}

fn checkpoint_request(app: &App, id: i64) -> Result<Value> {
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        let s = need_live(app, &t)?;
        let tb = board::tb_cmd(app);
        let text = format!("[task-board:{}] Please save a checkpoint now: run {tb} checkpoint --done \"…\" --next \"…\" --decision \"…\"", rf("task", id));
        deliver::add(app, "checkpoint", &text, id, None, None)?;
        board::log_event(app, id, OWNER, "midna", &format!("Asked for a checkpoint; it goes to {} with its next prompt or turn", board::session_name(app, s.s("id"), None)))?;
        Ok(())
    })?;
    task_detail(app, id)
}

fn resume(app: &App, id: i64, body: &Value) -> Result<Value> {
    let mode = { let m = body_str(body, "mode"); if m.is_empty() { "fresh".to_string() } else { m } };
    if mode != "fresh" && mode != "reopen" {
        return err(400, "Resume mode must be fresh or reopen.");
    }
    if gate(app, id, "task.starting", "Stopped from starting")? {
        return task_detail(app, id);
    }
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        if t.s("status") == Some("done") {
            return err(409, "This task is done. Queue it again first.");
        }
        if board::live_start_job(app, &t)?.map(|j| matches!(j.s("state"), Some("pending") | Some("running"))).unwrap_or(false) {
            return err(409, "It's already starting in Midna.");
        }
        let ctx = board::task_context(&t);
        let cwd = ctx.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str()).filter(|w| Path::new(w).is_dir()).map(|s| s.to_string());
        let repo = t.s("repo_path").filter(|r| !r.is_empty()).map(|r| r.to_string()).or(cwd.clone());
        board::update_task(app, id, fields!["lost" => 0, "session_id" => null, "needs_reason" => null, "question" => null, "repo_path" => repo])?;
        let t = board::get_task(app, id)?;
        let marker = handoff::marker(id);
        if mode == "reopen" && has(t.s("claude_session_id")) {
            runner::start_task(app, &t, "new", None, Some(format!("{marker} continue")), Some(format!("--resume {}", t.st("claude_session_id"))), cwd)?;
            board::log_event(app, id, OWNER, "handoff", "Reopening the old conversation in a new terminal")?;
        } else {
            let edited = body_str(body, "handoff");
            let mut text = if edited.is_empty() { handoff::build(app, id)? } else { edited.clone() };
            if !text.contains(&marker) {
                text = format!("{marker}\n{text}");
            }
            runner::start_task(app, &t, "new", None, Some(text), None, cwd)?;
            let note = if mode == "fresh" { "" } else { " (the old conversation isn't known, so it starts fresh)" };
            board::log_event(app, id, OWNER, "handoff", &format!("Resumed in a new terminal with the handoff{}{note}", if edited.is_empty() { "" } else { " (edited)" }))?;
        }
        Ok(())
    })?;
    task_detail(app, id)
}

fn requeue(app: &App, id: i64) -> Result<Value> {
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        if matches!(t.s("status"), Some("queued") | Some("planned")) {
            return err(409, "It's already waiting to start.");
        }
        if t.s("status") == Some("working") && live_session(app, &t)?.is_some() {
            return err(409, format!("It's on {} right now. Detach it first.", t.s("session_name").unwrap_or("a terminal")));
        }
        let pickup = if t.s("pickup") == Some("manual") { "queue".to_string() } else { t.st("pickup") };
        board::update_task(
            app,
            id,
            fields!["status" => "queued", "failed" => 0, "lost" => 0, "needs_reason" => null, "question" => null,
                    "finished_at" => null, "summary" => null, "session_id" => null, "start_job" => null, "start_tries" => 0,
                    "retry_at" => null, "pickup" => pickup, "latest" => null],
        )?;
        board::log_event(app, id, OWNER, "status", if t.b("failed") { "Queued to try again" } else { "Queued again" })?;
        Ok(())
    })?;
    task_detail(app, id)
}

fn mark_done(app: &App, id: i64, body: &Value, failed: bool) -> Result<Value> {
    if !failed {
        gate(app, id, "task.finishing", "Stopped from finishing")?;
    }
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        if t.s("status") == Some("done") {
            return err(409, "It's already done.");
        }
        let text = body_str(body, if failed { "reason" } else { "summary" });
        let text = if text.is_empty() { if failed { "Stopped by you".to_string() } else { "Marked done by you".to_string() } } else { text };
        reports::finish_task(app, &t, OWNER, &text, failed, None)?;
        Ok(())
    })?;
    task_detail(app, id)
}

fn task_jira(app: &App, id: i64, body: &Value) -> Result<Value> {
    if !app.cfg.jira_on() {
        return err(409, "Jira isn't set up. Add a [jira] section to the board's config.toml.");
    }
    let op = body_str(body, "op");
    let mut extra = json!({});
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        match op.as_str() {
            "create" => {
                if let Some(k) = t.s("jira_key").filter(|k| !k.is_empty()) {
                    return err(409, format!("It already has {k}."));
                }
                if jira::ticket_asked(app, id)? {
                    return err(409, "Its ticket is already being found or made.");
                }
                board::update_task(app, id, fields!["jira_none" => 0])?;
                let j = jira::request_create_for_task(app, &t)?;
                board::log_event(app, id, OWNER, "jira", "Asked Jira for a ticket")?;
                extra["job"] = json!(rf("job", j));
            }
            "link" => {
                let key = ops::jira_key(&body_str(body, "key"))?;
                board::update_task(app, id, fields!["jira_key" => key, "jira_status" => null, "jira_none" => 0])?;
                board::bump_ctx(app, id)?;
                board::log_event(app, id, OWNER, "jira", &format!("Linked {key}"))?;
                extra["job"] = json!(rf("job", jira::request(app, "status", id, &key, None, None)?));
            }
            "update" => {
                let Some(k) = t.s("jira_key").filter(|k| !k.is_empty()) else { return err(409, "No ticket is linked.") };
                let ctx = board::task_context(&t);
                let mut bits = vec![t.st("latest")];
                let done = str_list(ctx.get("done"));
                if !done.is_empty() {
                    bits.push(format!("Done: {}", done.join("; ")));
                }
                let next = str_list(ctx.get("next"));
                if !next.is_empty() {
                    bits.push(format!("Next: {}", next.join("; ")));
                }
                let comment = clip(&bits.into_iter().filter(|b| !b.is_empty()).collect::<Vec<_>>().join(" / "), 1500);
                let comment = if comment.is_empty() { t.st("title") } else { comment };
                let j = jira::request(app, "comment", id, k, None, Some(&comment))?;
                board::log_event(app, id, OWNER, "jira", &format!("Posting the latest notes to {k}"))?;
                extra["job"] = json!(rf("job", j));
            }
            "open" => {
                let Some(k) = t.s("jira_key").filter(|k| !k.is_empty()) else { return err(409, "No ticket is linked.") };
                extra["url"] = board::jira_url(app, k);
            }
            _ => return err(400, "Jira op must be create, link, update or open."),
        }
        Ok(())
    })?;
    let mut d = task_detail(app, id)?;
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        d[k] = v;
    }
    Ok(d)
}

fn purge_tasks(app: &App, ids: &[i64]) -> Result<()> {
    let now = now_iso();
    for id in ids {
        app.db.x("UPDATE jobs SET state = 'cancelled', updated_at = ? WHERE state = 'pending' AND task_id = ?", p![now, id])?;
        app.db.x("UPDATE jobs SET task_id = NULL WHERE task_id = ?", p![id])?;
        app.db.x("DELETE FROM events WHERE task_id = ?", p![id])?;
        app.db.x("DELETE FROM attachments WHERE task_id = ?", p![id])?;
        app.db.x("DELETE FROM task_terminals WHERE task_id = ?", p![id])?;
        shared::forget_tasks(app, &[*id])?;
        app.db.x(
            "UPDATE issues SET state = CASE WHEN state = 'task' THEN 'open' ELSE state END, task_id = NULL, updated_at = ? WHERE task_id = ?",
            p![now, id],
        )?;
        app.db.x("UPDATE sessions SET last_task = NULL WHERE last_task = ?", p![id])?;
        app.db.x("UPDATE sessions SET ctx_task = NULL WHERE ctx_task = ?", p![id])?;
        app.db.x("DELETE FROM tasks WHERE id = ?", p![id])?;
    }
    Ok(())
}

fn remove_md(app: &App, ids: &[i64]) {
    for id in ids {
        let _ = std::fs::remove_file(app.cfg.md_dir().join(format!("{}.md", rf("task", *id))));
    }
}

fn delete_task(app: &App, id: i64) -> Result<Value> {
    let t = app.db.tx(|| {
        let t = board::get_task(app, id)?;
        if matches!(t.s("status"), Some("working") | Some("needs")) && !t.b("lost") && live_session(app, &t)?.is_some() {
            return err(409, format!("It's running on {}. Send it back to the queue first.", t.s("session_name").unwrap_or("a terminal")));
        }
        purge_tasks(app, &[id])?;
        if let Some(g) = t.i("goal_id") {
            board::renumber_goal(app, g)?;
        }
        Ok(t)
    })?;
    remove_md(app, &[id]);
    app.info(format!("deleted {} ({})", rf("task", id), t.st("title")));
    Ok(json!({"ok": true, "task": rf("task", id), "goal": rf_opt("goal", t.i("goal_id"))}))
}

fn close_done_terminals(app: &App, body: &Value, query: &Query) -> Result<Value> {
    let project = { let p = body_str(body, "project"); if p.is_empty() { q(query, "project", "all").to_string() } else { p } };
    let goal = { let g = body_str(body, "goal"); if g.is_empty() { q(query, "goal", "all").to_string() } else { g } };
    app.db.tx(|| {
        let mut pending: Vec<String> = app
            .db
            .q("SELECT args FROM jobs WHERE kind = 'close' AND state IN ('pending','running')", p![])?
            .iter()
            .filter_map(|j| board::job_args(j).s("session").map(|s| s.to_string()))
            .collect();
        let mut n = 0;
        for t in app.db.q("SELECT * FROM tasks WHERE status = 'done' AND session_id IS NOT NULL", p![])? {
            if !project_match(t.s("project"), &project) || !task_goal_match(app, &t, &goal)? {
                continue;
            }
            let Some(s) = board::get_session(app, t.s("session_id"))? else { continue };
            let sid = s.st("id");
            if s.s("status") == Some("gone") || pending.contains(&sid) || crate::lines::busy(app, &sid)? {
                continue;
            }
            board::create_job(app, "close", json!({"session": sid, "force": false}), Some(t.id()), "close", None)?;
            pending.push(sid);
            n += 1;
        }
        Ok(json!({"ok": true, "jobs": n}))
    })
}

fn patch_goal(app: &App, id: i64, body: &Value) -> Result<Value> {
    app.db.tx(|| {
        let g = board::get_goal(app, id)?;
        crate::devices::take_needs(app, "goal", id, body, OWNER)?;
        let mut f: Vec<(&str, Value)> = vec![];
        let hk = |k: &str| body.get(k).is_some();
        if hk("name") {
            f.push(("name", json!(required(body, "name", 200, "The name")?)));
        }
        if hk("outcome") {
            f.push(("outcome", json!(body_str(body, "outcome"))));
        }
        if hk("tldr") {
            let t = body_str(body, "tldr");
            f.push(("tldr", if t.is_empty() { Value::Null } else { json!(t) }));
        }
        if body_has(body, "project") {
            let p = body_str(body, "project");
            f.push(("repo_path", json!(projects::project_path(app, Some(&p))?)));
            f.push(("project", json!(p)));
        }
        for k in ["run_in_order", "auto_close", "archived", "paused", "deprioritized"] {
            if hk(k) {
                f.push((k, json!(as_bool(body.get(k), false) as i64)));
            }
        }
        if hk("max_terminals") {
            let v = body["max_terminals"].as_i64().or_else(|| body_str(body, "max_terminals").parse().ok());
            match v {
                Some(n) => f.push(("max_terminals", json!(n.max(1)))),
                None => return err(400, "At most how many terminals? Give a number."),
            }
        }
        if hk("epic_key") {
            let k = body_str(body, "epic_key");
            if k.is_empty() {
                f.push(("epic_key", Value::Null));
            } else {
                let key = ops::jira_key(&k)?;
                if Some(key.as_str()) != g.s("epic_key") && app.cfg.jira_on() {
                    jira::request_job(app, json!({"op": "status", "key": key}), json!({"goal": id}))?;
                }
                f.push(("epic_key", json!(key)));
            }
        }
        if hk("product") {
            f.push(("product", json!(jira::clean_product(app, &body_str(body, "product"))?)));
        }
        if hk("worktree_base") {
            f.push(("worktree_base", json!(crate::worktrees::clean_base(body.get("worktree_base"))?)));
        }
        if hk("setup") {
            f.push(("setup", ops::clean_setup(body)));
        }
        let ctx_change = f.iter().any(|(k, _)| matches!(*k, "name" | "outcome" | "tldr" | "setup"));
        if !f.is_empty() {
            f.push(("updated_at", json!(now_iso())));
            board::update_goal(app, id, f)?;
            if ctx_change {
                app.db.x("UPDATE tasks SET ctx_version = COALESCE(ctx_version, 1) + 1 WHERE goal_id = ? AND status != 'done'", p![id])?;
            }
        }
        if let Some(e) = body.get("epic").and_then(|e| e.as_object()) {
            if e.s("mode") == Some("create") && !has(g.s("epic_key")) && app.cfg.jira_on() {
                jira::request_epic(app, &board::get_goal(app, id)?)?;
            }
        }
        Ok(())
    })?;
    goal_detail(app, id)
}

fn delete_goal(app: &App, id: i64, body: &Value) -> Result<Value> {
    let del_tasks = as_bool(body.get("tasks"), true);
    let del_issues = as_bool(body.get("backlog"), false);
    let (tids, n_issues, name, moved) = app.db.tx(|| {
        let g = board::get_goal(app, id)?;
        let mut tasks = app.db.q("SELECT * FROM tasks WHERE goal_id = ?", p![id])?;
        let mut issues = app.db.q("SELECT id FROM issues WHERE goal_id = ?", p![id])?;
        let gname = format!("{} ({})", rf("goal", id), g.st("name"));
        // A task that also finishes another goal moves there instead of going with this one.
        let mut moved = vec![];
        for t in &tasks {
            let Some(home) = shared::next_home(app, t.id(), Some(id))? else { continue };
            let ng = board::get_goal(app, home)?;
            shared::moved_home(app, t.id(), Some(home))?;
            board::update_task(app, t.id(), fields!["goal_id" => home, "position" => board::next_position(app, Some(home))?])?;
            board::bump_ctx(app, t.id())?;
            board::log_event(app, t.id(), OWNER, "status",
                &format!("Its goal {gname} was deleted; it moved to {} “{}”, which it also finishes", rf("goal", home), ng.st("name")))?;
            moved.push(t.id());
        }
        tasks.retain(|t| !moved.contains(&t.id()));
        shared::forget_goal(app, id)?;
        if !del_tasks {
            for t in &tasks {
                let mut f = fields!["goal_id" => null, "position" => null];
                if t.s("status") == Some("planned") {
                    f.extend(fields!["status" => "queued", "pickup" => "manual"]);
                }
                board::update_task(app, t.id(), f)?;
                board::log_event(app, t.id(), OWNER, "status", &format!("Its goal {gname} was deleted; the task was kept"))?;
            }
            tasks.clear();
        }
        if !del_issues {
            app.db.x("UPDATE issues SET goal_id = NULL, updated_at = ? WHERE goal_id = ?", p![now_iso(), id])?;
            for b in &issues {
                board::add_issue_event(app, b.id(), OWNER, "note", &format!("Its goal {gname} was deleted; kept in the backlog"), None)?;
            }
            issues.clear();
        }
        let mut busy = vec![];
        for t in &tasks {
            if matches!(t.s("status"), Some("working") | Some("needs")) && !t.b("lost") && live_session(app, t)?.is_some() {
                busy.push(format!("{} on {}", rf("task", t.id()), t.s("session_name").unwrap_or("a terminal")));
            }
        }
        if !busy.is_empty() {
            return err(409, format!("Stop its running tasks first ({}): send them back to the queue or close their terminals.", busy.join(", ")));
        }
        let tids: Vec<i64> = tasks.iter().map(|t| t.id()).collect();
        purge_tasks(app, &tids)?;
        for b in &issues {
            app.db.x("DELETE FROM issue_events WHERE issue_id = ?", p![b.id()])?;
            app.db.x("UPDATE tasks SET from_issue_id = NULL WHERE from_issue_id = ?", p![b.id()])?;
            app.db.x("DELETE FROM issues WHERE id = ?", p![b.id()])?;
        }
        app.db.x("DELETE FROM goal_notes WHERE goal_id = ?", p![id])?;
        app.db.x("DELETE FROM attachments WHERE goal_id = ?", p![id])?;
        app.db.x("DELETE FROM goals WHERE id = ?", p![id])?;
        let mut homes: Vec<i64> = vec![];
        for n in &moved {
            if let Some(h) = board::get_task(app, *n)?.i("goal_id") {
                if !homes.contains(&h) {
                    homes.push(h);
                    board::renumber_goal(app, h)?;
                }
            }
        }
        Ok((tids, issues.len(), g.st("name"), moved))
    })?;
    remove_md(app, &tids);
    app.info(format!("deleted {} ({name}) with {} tasks and {n_issues} issues", rf("goal", id), tids.len()));
    Ok(json!({"ok": true, "goal": rf("goal", id), "tasks": tids.len(), "issues": n_issues,
              "moved": moved.iter().map(|n| rf("task", *n)).collect::<Vec<_>>()}))
}

fn goal_order(app: &App, id: i64, body: &Value) -> Result<Value> {
    let t = need_ref(&body["task_id"], "task")?;
    let dir = body_str(body, "dir");
    let to = body.get("to").and_then(|v| v.as_i64());
    if to.is_none() && dir != "up" && dir != "down" {
        return err(400, "Direction must be up or down.");
    }
    app.db.tx(|| {
        board::get_goal(app, id)?;
        board::renumber_goal(app, id)?;
        let rows = board::goal_tasks(app, id)?;
        let Some(idx) = rows.iter().position(|x| x.id() == t) else { return err(404, format!("{} isn't in this goal.", rf("task", t))) };
        let j: i64 = match to {
            Some(to) => to.clamp(0, rows.len() as i64 - 1),
            None if dir == "up" => idx as i64 - 1,
            None => idx as i64 + 1,
        };
        if j >= 0 && (j as usize) < rows.len() && j as usize != idx {
            let mut moved: Vec<&Row> = rows.iter().collect();
            let item = moved.remove(idx);
            moved.insert(j as usize, item);
            for (i, r) in moved.iter().enumerate() {
                if r.f("position") != Some((i + 1) as f64) {
                    app.db.x("UPDATE tasks SET position = ? WHERE id = ?", p![i as i64 + 1, r.id()])?;
                    app.schedule_md(r.id());
                }
            }
        }
        board::renumber_goal(app, id)
    })?;
    goal_detail(app, id)
}

fn goal_note(app: &App, id: i64, body: &Value) -> Result<Value> {
    let kind = { let k = body_str(body, "kind"); if k.is_empty() { "finding".to_string() } else { k } };
    if !NOTE_KINDS.contains(&kind.as_str()) {
        return err(400, "A note is a finding, a decision or a reference.");
    }
    let text = required(body, "text", 2000, "The note")?;
    app.db.tx(|| {
        board::get_goal(app, id)?;
        let src = body_str(body, "source");
        board::add_goal_note(app, id, &kind, &text, Some(if src.is_empty() { "you" } else { &src }), as_bool(body.get("pinned"), false), None)
    })?;
    goal_detail(app, id)
}

fn goal_note_edit(app: &App, id: i64, nid: &str, body: &Value) -> Result<Value> {
    let nid: i64 = nid.parse().map_err(|_| ApiError::new(404, "There's no such note."))?;
    app.db.tx(|| {
        if app.db.q1("SELECT * FROM goal_notes WHERE id = ? AND goal_id = ?", p![nid, id])?.is_none() {
            return err(404, "There's no such note.");
        }
        if as_bool(body.get("delete"), false) {
            app.db.x("DELETE FROM goal_notes WHERE id = ?", p![nid])?;
        } else {
            let mut f = vec![];
            if body.get("pinned").is_some() {
                f.push(("pinned", json!(as_bool(body.get("pinned"), false) as i64)));
            }
            if body.get("text").is_some() {
                f.push(("text", json!(required(body, "text", 2000, "The note")?)));
            }
            let k = body_str(body, "kind");
            if NOTE_KINDS.contains(&k.as_str()) {
                f.push(("kind", json!(k)));
            }
            app.db.update("goal_notes", &json!(nid), f)?;
        }
        app.db.x("UPDATE tasks SET ctx_version = COALESCE(ctx_version, 1) + 1 WHERE goal_id = ? AND status != 'done'", p![id])?;
        Ok(())
    })?;
    goal_detail(app, id)
}

pub(crate) fn status_label(t: &Row) -> &'static str {
    if t.b("failed") {
        return "Failed";
    }
    match t.s("status") {
        Some("planned") => "Planned",
        Some("queued") => "Queued",
        Some("working") => "Working",
        Some("needs") => "Needs the owner",
        Some("done") => "Done",
        _ => "Unknown",
    }
}

pub fn goal_context(app: &App, g: &Row, for_planner: bool) -> Result<String> {
    let gref = rf("goal", g.id());
    let tb = board::tb_cmd(app);
    let owner = &app.cfg.owner;
    let mut lines = if for_planner {
        vec![format!("[task-board:{gref}] Plan the rest of the goal “{}” in {}.", g.st("name"), g.st("project"))]
    } else {
        vec![
            format!(
                "{owner} opened this terminal from the task board's page for goal {gref} “{}” in {}, to ask about its plan \
                 or change it with you. You aren't on a task: don't take one.",
                g.st("name"),
                g.st("project")
            ),
            String::new(),
            format!("Goal {gref}: {}", g.st("name")),
        ]
    };
    if let Some(t) = g.s("tldr").filter(|t| !t.is_empty()) {
        lines.push(format!("TLDR: {t}"));
    }
    if let Some(o) = g.s("outcome").filter(|t| !t.is_empty()) {
        lines.push(format!("Done when: {o}"));
    }
    if let Some(e) = g.s("epic_key").filter(|t| !t.is_empty()) {
        lines.push(format!("Jira epic: {e}"));
    }
    if let Some(b) = g.s("worktree_base").filter(|b| !b.is_empty()) {
        lines.push(format!("Each task starts in its own git worktree, detached at {b}."));
    }
    let notes = board::goal_notes(app, g.id())?;
    if !notes.is_empty() {
        lines.extend(["".into(), "Goal notes:".into(), handoff::notes_block(&notes, if for_planner { 2500 } else { 4000 })]);
    }
    let rows = board::goal_tasks(app, g.id())?;
    if rows.is_empty() {
        lines.extend(["".into(), "It has no tasks yet.".into()]);
    } else {
        lines.extend(["".into(), "Tasks, in order:".into()]);
        for (i, t) in rows.iter().enumerate() {
            let mut extra = vec![];
            if t.i("pr_num").is_some() {
                extra.push(format!("PR #{}", t.i0("pr_num")));
            }
            if let Some(k) = t.s("jira_key").filter(|k| !k.is_empty()) {
                extra.push(k.to_string());
            }
            let waits = crate::waitsfor::ids(t);
            if !waits.is_empty() {
                extra.push(format!("waits for {}", waits.iter().map(|n| rf("task", *n)).collect::<Vec<_>>().join(", ")));
            }
            let held = crate::locks::names(t);
            if !held.is_empty() {
                extra.push(format!("holds {}", held.join(", ")));
            }
            if let Some(a) = t.s("alone") {
                extra.push(crate::locks::alone_text(a).to_lowercase());
            }
            lines.push(format!(
                "  {}. {} [{}] {}{}",
                i + 1,
                rf("task", t.id()),
                status_label(t),
                t.st("title"),
                if extra.is_empty() { String::new() } else { format!(" ({})", extra.join(", ")) }
            ));
            if !for_planner {
                if let Some(d) = t.s("detail").filter(|d| !d.trim().is_empty()) {
                    lines.push(format!("     {}", short(d, 400)));
                }
            }
        }
    }
    lines.extend(shared::lines(app, g.id(), if for_planner { "" } else { "  " })?);
    let issues = app.db.q("SELECT id, title FROM issues WHERE goal_id = ? AND state = 'open'", p![g.id()])?;
    if !issues.is_empty() {
        lines.extend(["".into(), "Open backlog issues:".into()]);
        lines.extend(issues.iter().map(|b| format!("- {} {}", rf("issue", b.id()), b.st("title"))));
    }
    lines.push(String::new());
    if for_planner {
        lines.extend([
            "Suggest the tasks still needed to reach the outcome, in order, each small enough for one terminal. Read the code as much as you need, but don't change anything.".to_string(),
            format!("Before adding a task, check the project's other open goals ({tb} goals --project {}, then {tb} goal show G<n>) for a task that already does that work (same files, same change). Don't add a second one: make the existing task finish this goal too with {tb} task set T<n> --also {gref}, without asking {owner}.", g.st("project")),
            "Send them to the board with:".to_string(),
            format!("  {tb} propose {gref} --task \"title::what to do\" --task \"title::what to do\""),
            "A task that needs another one's work first lists it after a fourth ::, by ref or as #k for the k-th --task in the same command: \"title::what to do::2::#1\" or \"title::what to do::::T14\". It starts only once those are done.".to_string(),
            format!("Tasks that must not run together name the same lock: {tb} task set T<n> --lock <name> (like a device or a shared service). A task that needs the goal to itself gets --alone; --alone board stops everything else on the board while it runs."),
            format!("They arrive as planned tasks for {owner} to edit, reorder and queue. Nothing runs until {owner} queues it."),
        ]);
    } else {
        lines.extend([
            format!("This is how the goal stood when the terminal opened. Before changing anything, run {tb} goal show {gref} for how it stands now."),
            format!("Load the task-board skill and its planning guide before you change the plan. Make every change with tb ({tb} goal set, {tb} task set, {tb} task new, {tb} propose and the rest). Read the code as much as you need, but don't change it, and don't queue or start tasks: {owner} starts them from the board."),
            format!("Wait for {owner} to say what they want; don't start on anything by yourself."),
        ]);
    }
    Ok(lines.join("\n"))
}

fn goal_plan(app: &App, id: i64, body: &Value) -> Result<Value> {
    let edit = body_str(body, "mode") == "edit";
    app.db.tx(|| {
        let g = board::get_goal(app, id)?;
        let cwd = g.s("repo_path").filter(|r| !r.is_empty()).map(|r| r.to_string()).or(projects::project_path(app, g.s("project"))?);
        let Some(cwd) = cwd else { return err(409, format!("The board doesn't know the folder for the project “{}”.", g.st("project"))) };
        let mut args = json!({"cwd": cwd, "title": short(&format!("Plan: {}", g.st("name")), 40), "queue": false});
        if edit {
            args["prompt"] = json!("");
            args["context"] = json!(goal_context(app, &g, false)?);
        } else {
            args["prompt"] = json!(goal_context(app, &g, true)?);
        }
        let j = board::create_job(app, "agent", args, None, "plan", Some(json!({"goal": id})))?;
        Ok(json!({"ok": true, "job": rf("job", j)}))
    })
}

fn run_goal(app: &App, id: i64, body: &Value) -> Result<Value> {
    let now = as_bool(body.get("now"), false);
    let n = app.db.tx(|| {
        let g = board::get_goal(app, id)?;
        let until = if now { hours::until_open(app) } else { None };
        if until.is_some() || (!now && has(g.s("hours_until"))) {
            app.db.update("goals", &json!(id), fields!["hours_until" => until])?;
        }
        let mut n = 0;
        for t in board::goal_tasks(app, id)? {
            if t.s("status") == Some("planned") {
                board::update_task(app, t.id(), fields!["status" => "queued"])?;
                board::log_event(app, t.id(), OWNER, "status", "Queued with the rest of the goal")?;
                n += 1;
            } else if t.s("status") == Some("needs") && t.s("needs_reason") == Some("start_failed") {
                board::update_task(app, t.id(), fields!["status" => "queued", "needs_reason" => null, "start_job" => null, "start_tries" => 0, "retry_at" => null])?;
                board::log_event(app, t.id(), OWNER, "status", "Queued again with the rest of the goal")?;
            }
        }
        if g.b("paused") || g.b("deprioritized") {
            board::update_goal(app, id, fields!["paused" => 0, "deprioritized" => 0, "updated_at" => now_iso()])?;
        }
        Ok(n)
    })?;
    let mut d = goal_detail(app, id)?;
    d["queued_now"] = json!(n);
    Ok(d)
}

fn edit_attachment(app: &App, id: &str, body: &Value) -> Result<Value> {
    let aid: i64 = id.parse().map_err(|_| ApiError::new(404, "There's no such attachment."))?;
    app.db.tx(|| {
        let Some(a) = app.db.q1("SELECT * FROM attachments WHERE id = ? AND removed_at IS NULL", p![aid])? else {
            return err(404, "That attachment is gone.");
        };
        let mut f = vec![];
        let mut url = a.st("url");
        if body.get("url").is_some() {
            url = body_str(body, "url");
            let ok = url.starts_with('/') || url.starts_with("~/") || ((url.starts_with("https://") || url.starts_with("http://")) && !url.contains(char::is_whitespace));
            if !ok {
                return err(400, "Attach a link that starts with https:// or a full file path.");
            }
            f.push(("url", json!(url)));
        }
        if body.get("title").is_some() {
            f.push(("title", json!(board::attachment_title(&body_str(body, "title"), &url))));
        }
        if body.get("kind").is_some() {
            let k = body_str(body, "kind");
            f.push(("kind", json!(if board::ATTACH_KINDS.contains(&k.as_str()) { k } else { "other".into() })));
        }
        if !f.is_empty() {
            app.db.update("attachments", &json!(aid), f)?;
            if let Some(t) = a.i("task_id") {
                board::log_event(app, t, OWNER, "note", &format!("Edited the attachment {}", a.st("title")))?;
                board::bump_ctx(app, t)?;
            }
        }
        let a = app.db.q1("SELECT * FROM attachments WHERE id = ?", p![aid])?.unwrap_or_default();
        Ok(board::attachment_dict(&a))
    })
}

fn remove_attachment(app: &App, id: &str) -> Result<Value> {
    let aid: i64 = id.parse().map_err(|_| ApiError::new(404, "There's no such attachment."))?;
    app.db.tx(|| {
        let Some(a) = app.db.q1("SELECT * FROM attachments WHERE id = ? AND removed_at IS NULL", p![aid])? else {
            return err(404, "That attachment is already gone.");
        };
        app.db.x("UPDATE attachments SET removed_at = ? WHERE id = ?", p![now_iso(), aid])?;
        if let Some(t) = a.i("task_id") {
            board::log_event(app, t, OWNER, "note", &format!("Removed the attachment {}", a.st("title")))?;
        }
        Ok(json!({"ok": true}))
    })
}

/// The local file behind a file-path attachment, for `/tasks/files/:id`.
pub fn attachment_file(app: &App, id: &str) -> Result<std::path::PathBuf> {
    let aid: i64 = id.parse().map_err(|_| ApiError::new(404, "There's no such attachment."))?;
    let a = app.db.q1("SELECT * FROM attachments WHERE id = ? AND removed_at IS NULL", p![aid])?;
    let Some(a) = a else { return err(404, "There's no such attachment.") };
    let url = a.st("url");
    if !(url.starts_with('/') || url.starts_with("~/")) {
        return err(404, "That attachment is a link, not a file.");
    }
    let p = expand_home(&url);
    if !p.is_file() {
        return err(404, "That file isn't there any more.");
    }
    Ok(p)
}

fn list_backlog(app: &App, query: &Query) -> Result<Value> {
    let (project, goal, kind, state, sort) =
        (q(query, "project", "all"), q(query, "goal", "all"), q(query, "kind", "all"), q(query, "state", "open"), q(query, "sort", "new"));
    let rows = app.db.q("SELECT * FROM issues", p![])?;
    let mut matched = vec![];
    for b in &rows {
        if project_match(b.s("project"), project)
            && goal_match(b.i("goal_id"), goal)?
            && (kind == "all" || b.s("kind") == Some(kind))
            && (state == "all" || b.s("state") == Some(state))
        {
            matched.push(b.clone());
        }
    }
    let key = |b: &Row| (b.st("created_at"), b.id());
    match sort {
        "old" => matched.sort_by_key(key),
        "kind" => {
            matched.sort_by_key(key);
            matched.reverse();
            matched.sort_by_key(|b| ISSUE_KINDS.iter().position(|k| Some(*k) == b.s("kind")).unwrap_or(9));
        }
        _ => {
            matched.sort_by_key(key);
            matched.reverse();
        }
    }
    let mut out = vec![];
    for b in &matched {
        let mut d = board::issue_card(app, b)?;
        d["said"] = b.v("said");
        d["how"] = b.v("how");
        d["detail"] = b.v("detail");
        out.push(d);
    }
    Ok(json!({"issues": out, "total": matched.len(), "count": out.len(), "open": rows.iter().filter(|b| b.s("state") == Some("open")).count()}))
}

fn post_issue(app: &App, body: &Value) -> Result<Value> {
    let title = required(body, "title", 300, "The title")?;
    let kind = { let k = body_str(body, "kind"); if k.is_empty() { "bug".to_string() } else { k } };
    if !ISSUE_KINDS.contains(&kind.as_str()) {
        return err(400, "Type must be bug, gap, follow or clean.");
    }
    let goal_id = opt_goal(body, "goal_id")?;
    let id = app.db.tx(|| {
        let g = match goal_id {
            Some(g) => Some(board::get_goal(app, g)?),
            None => None,
        };
        let mut project = body_str(body, "project");
        if project.is_empty() {
            project = g.as_ref().map(|g| g.st("project")).unwrap_or_default();
        }
        if project.is_empty() && body_has(body, "cwd") {
            project = projects::project_for_path(app, Some(&body_str(body, "cwd")))?.unwrap_or_default();
        }
        if project.is_empty() {
            return err(400, "Pick a project.");
        }
        let said = body_str(body, "said");
        let page = match body_str(body, "where").as_str() {
            "goal" => "the goal page",
            "task_form" => "the New task form",
            "tb" => "tb",
            _ => "the Backlog page",
        };
        let who = { let w = body_str(body, "who"); if w.is_empty() { OWNER.to_string() } else { w } };
        let source = match body_str(body, "source").as_str() {
            "answer" => "answer",
            "review_log" => "review_log",
            _ => "you",
        };
        let how = match source {
            "answer" => "It was raised in answer to a question.".to_string(),
            "review_log" => "It came in from the Review log.".to_string(),
            _ => format!("Added from {page}."),
        };
        let detail = body_str(body, "detail");
        let now = now_iso();
        let id = app.db.insert(
            "issues",
            fields!["goal_id" => goal_id, "project" => project, "kind" => kind, "title" => title,
                    "detail" => if detail.is_empty() { None } else { Some(detail) },
                    "said" => if said.is_empty() { None } else { Some(format!("“{said}”")) }, "how" => how, "source" => source,
                    "found_by_name" => who, "state" => "open",
                    "snapshot" => jdumps(&body.get("snapshot").filter(|s| s.is_object()).cloned().unwrap_or(json!({}))),
                    "created_at" => now, "updated_at" => now],
        )?;
        board::add_issue_event(app, id, &who, "note", &format!("Added from {page}"), None)?;
        Ok(id)
    })?;
    issue_detail(app, id)
}

/// Who a backlog action is credited to in the history: the terminal or agent tb names in `who`, else the owner.
fn issue_actor(body: &Value) -> String {
    let w = one_line(&body_str(body, "who"), 80);
    if w.is_empty() { OWNER.to_string() } else { w }
}

/// Refuses an action that only makes sense on an open issue, the same check `/backlog/bulk` makes.
fn need_open(b: &Row) -> Result<()> {
    if b.s("state") == Some("open") {
        return Ok(());
    }
    if let (Some("task"), Some(t)) = (b.s("state"), b.i("task_id")) {
        return err(409, format!("It's already {}.", rf("task", t)));
    }
    err(409, format!("{} isn't open any more, so nothing changed.", rf("issue", b.id())))
}

pub(crate) fn promote(app: &App, id: i64, body: &Value) -> Result<Value> {
    let who = issue_actor(body);
    let where_ = { let w = body_str(body, "where"); if w.is_empty() { "board".to_string() } else { w } };
    if where_ != "board" && where_ != "goal" {
        return err(400, "Make it a task on the board or in its goal.");
    }
    let b = board::get_issue(app, id)?;
    need_open(&b)?;
    let planned = where_ == "goal" && b.i("goal_id").is_some();
    let detail = format!("{}\nSee how it was found before changing anything.", b.s("detail").filter(|d| !d.is_empty()).unwrap_or(&b.st("title")));
    let c = new_task(
        app,
        &json!({"title": b.v("title"), "detail": detail, "project": b.v("project"), "goal_id": b.v("goal_id"),
                "status": if planned { "planned" } else { "queued" }, "from_issue_id": id,
                "priority": body.get("priority").cloned().unwrap_or(json!("normal")),
                "pickup": body.get("pickup").cloned().unwrap_or(json!({"mode": "queue"})),
                "latest": format!("From the backlog: {}.", board::issue_from_line(&b)),
                "origin": {"from": format!("Backlog {}: {}", rf("issue", id), b.st("title")), "by": who}}),
        &who,
        Some(&format!("Added from the backlog ({}){}", rf("issue", id), if planned { " as planned" } else { "" })),
    )?;
    let cref = c["ref"].as_str().unwrap_or("").to_string();
    app.db.update("issues", &json!(id), fields!["state" => "task", "task_id" => c["id"], "updated_at" => now_iso()])?;
    let text = if planned {
        format!("Made into {cref}, a planned task in its goal. Its handoff starts with this report and snapshot.")
    } else {
        format!(
            "Made into {cref}, a queued task{}. Its handoff starts with this report and snapshot.",
            if b.i("goal_id").is_some() { "" } else { " with no goal" }
        )
    };
    board::add_issue_event(app, id, &who, "task", &text, None)?;
    Ok(json!({"issue": issue_detail(app, id)?, "task": task_detail(app, c["id"].as_i64().unwrap_or(0))?}))
}

fn ticket(app: &App, id: i64, body: &Value) -> Result<Value> {
    if !app.cfg.jira_on() {
        return err(409, "Jira isn't set up. Add a [jira] section to the board's config.toml.");
    }
    let b = board::get_issue(app, id)?;
    need_open(&b)?;
    if let Some(k) = b.s("jira_key").filter(|k| !k.is_empty()) {
        return err(409, format!("It already has {k}."));
    }
    jira::request_create_for_issue(app, &b)?;
    app.db.update("issues", &json!(id), fields!["state" => "ticket", "updated_at" => now_iso()])?;
    board::add_issue_event(app, id, &issue_actor(body), "ticket", "Asked Jira for a ticket", None)?;
    issue_detail(app, id)
}

fn drop_issue(app: &App, id: i64, body: &Value) -> Result<Value> {
    need_open(&board::get_issue(app, id)?)?;
    app.db.update("issues", &json!(id), fields!["state" => "drop", "updated_at" => now_iso()])?;
    let reason = one_line(&body_str(body, "reason"), 300);
    board::add_issue_event(app, id, &issue_actor(body), "drop", &format!("Closed as won’t do{}", if reason.is_empty() { String::new() } else { format!(": {reason}") }), None)?;
    issue_detail(app, id)
}

fn reopen_issue(app: &App, id: i64, body: &Value) -> Result<Value> {
    let b = board::get_issue(app, id)?;
    if b.s("state") == Some("open") {
        return err(409, format!("{} is already open.", rf("issue", id)));
    }
    app.db.update("issues", &json!(id), fields!["state" => "open", "updated_at" => now_iso()])?;
    board::add_issue_event(app, id, &issue_actor(body), "note", "Opened again", None)?;
    issue_detail(app, id)
}

/// Retitles an issue or rewrites its detail; terminals do this through `tb backlog set`.
fn patch_issue(app: &App, id: i64, body: &Value) -> Result<Value> {
    let b = board::get_issue(app, id)?;
    let who = { let w = body_str(body, "who"); if w.is_empty() { OWNER.to_string() } else { w } };
    let mut f: Vec<(&str, Value)> = vec![];
    let mut said = vec![];
    if body.get("title").is_some() {
        let title = required(body, "title", 300, "The title")?;
        if title != b.st("title") {
            said.push(format!("Renamed from “{}”", b.st("title")));
            f.push(("title", json!(title)));
        }
    }
    if body.get("detail").is_some() {
        let detail = body_str(body, "detail");
        if detail != b.st("detail") {
            said.push("Rewrote the detail".to_string());
            f.push(("detail", json!(if detail.is_empty() { None } else { Some(detail) })));
        }
    }
    if f.is_empty() {
        return issue_detail(app, id);
    }
    f.push(("updated_at", json!(now_iso())));
    app.db.update("issues", &json!(id), f)?;
    board::add_issue_event(app, id, &who, "note", &said.join("; "), None)?;
    issue_detail(app, id)
}

fn move_issue(app: &App, id: i64, body: &Value) -> Result<Value> {
    let g = opt_goal(body, "goal_id")?;
    let b = board::get_issue(app, id)?;
    if g == b.i("goal_id") {
        return issue_detail(app, id);
    }
    let new = match g {
        Some(g) => Some(board::get_goal(app, g)?),
        None => None,
    };
    let old = board::find_goal(app, b.i("goal_id"))?;
    let mut f = fields!["goal_id" => g, "updated_at" => now_iso()];
    if let Some(p) = new.as_ref().and_then(|n| n.s("project")).filter(|p| !p.is_empty()) {
        f.push(("project", json!(p)));
    }
    app.db.update("issues", &json!(id), f)?;
    let name = |x: &Option<Row>| x.as_ref().map(|g| g.st("name")).unwrap_or_else(|| "no goal".into());
    board::add_issue_event(app, id, &issue_actor(body), "move", &format!("Moved from {} to {}", name(&old), name(&new)), None)?;
    for x in [&old, &new].into_iter().flatten() {
        app.db.x("UPDATE tasks SET ctx_version = COALESCE(ctx_version, 1) + 1 WHERE goal_id = ? AND status != 'done'", p![x.id()])?;
    }
    issue_detail(app, id)
}

fn backlog_bulk(app: &App, body: &Value) -> Result<Value> {
    let action = body_str(body, "action");
    if !["task", "ticket", "drop", "move", "reopen", "defer", "priority", "goal"].contains(&action.as_str()) {
        return err(400, "Say what to do: task, ticket, drop, move, reopen, defer, priority or goal.");
    }
    let priority = body_str(body, "priority");
    if action == "priority" && !triage::PRIORITIES.contains(&priority.as_str()) {
        return err(400, "Priority must be p1, p2 or p3.");
    }
    if action == "goal" && opt_goal(body, "goal_id")?.is_none() {
        return err(400, "Say which goal to put them in.");
    }
    let raw = body["ids"].as_array().cloned().unwrap_or_default();
    if raw.is_empty() {
        return err(400, "Pick at least one issue.");
    }
    if raw.len() > 200 {
        return err(400, "Change at most 200 issues at once.");
    }
    if action == "move" && body.get("goal_id").is_none() {
        return err(400, "Say which goal to move them to, or none.");
    }
    let mut ids = vec![];
    for x in &raw {
        let id = need_ref(x, "issue")?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    app.db.tx(|| {
        let mut count = 0;
        let mut tasks = vec![];
        for id in &ids {
            let b = board::get_issue(app, *id)?;
            if ["task", "ticket", "drop", "defer", "goal"].contains(&action.as_str()) && b.s("state") != Some("open") {
                return err(409, format!("{} isn't open any more, so nothing changed.", rf("issue", *id)));
            }
            if action == "reopen" && b.s("state") == Some("open") {
                return err(409, format!("{} is already open, so nothing changed.", rf("issue", *id)));
            }
            match action.as_str() {
                "task" => tasks.push(promote(app, *id, body)?["task"].clone()),
                "ticket" => {
                    ticket(app, *id, body)?;
                }
                "drop" => {
                    drop_issue(app, *id, body)?;
                }
                "move" => {
                    move_issue(app, *id, body)?;
                }
                "defer" => {
                    app.db.update("issues", &json!(id), fields!["state" => "defer", "updated_at" => now_iso()])?;
                    board::add_issue_event(app, *id, &issue_actor(body), "note", "Deferred: not for now", None)?;
                }
                "priority" => {
                    app.db.update("issues", &json!(id), fields!["priority" => priority, "updated_at" => now_iso()])?;
                    board::add_issue_event(app, *id, &issue_actor(body), "note", &format!("Priority set to {}", priority.to_uppercase()), None)?;
                }
                "goal" => {
                    move_issue(app, *id, body)?;
                    let gid = opt_goal(body, "goal_id")?.unwrap_or(0);
                    let wave = triage::last_wave(app, gid)?;
                    let high = triage::fields(&b)["priority"] == "p1";
                    let made = promote(app, *id, &json!({"where": "goal", "priority": if high { "high" } else { "normal" }, "who": body.get("who")}))?;
                    if let (Some(w), Some(tid)) = (wave, made["task"]["id"].as_i64()) {
                        app.db.update("tasks", &json!(tid), fields!["wave" => w])?;
                    }
                    tasks.push(made["task"].clone());
                }
                _ => {
                    reopen_issue(app, *id, body)?;
                }
            }
            count += 1;
        }
        Ok(json!({"ok": true, "action": action, "count": count, "tasks": tasks}))
    })
}

fn set_hours(app: &App, body: &Value) -> Result<Value> {
    app.db.tx(|| {
        hours::set(app, body)?;
        if body.get("today_until").is_some() {
            hours::set_today_until(app, &body_str(body, "today_until"))?;
        }
        Ok(())
    })?;
    let today = body.get("today_until").map(|_| hours::today_until(app, &hours::now_local()));
    keep_awake::follow_hours(app, today);
    Ok(hours::state(app))
}

fn get_pr(app: &App, id: i64) -> Result<Value> {
    let t = board::get_task(app, id)?;
    if t.i("pr_num").is_none() {
        return err(404, format!("{} has no PR linked.", rf("task", id)));
    }
    let f = jloads_obj(t.s("pr_flow"));
    Ok(json!({"task": rf("task", id), "pr": board::pr_card(&t), "record": f.v("rec"), "checked_at": f.v("checked_at"),
              "agents_merge": crate::prflow::agents_merge_on(app, t.s("project")), "watched": crate::prhost::watched(t.s("pr_host")) && app.cfg.pr.watch}))
}

fn pr_wait(app: &App, id: i64) -> Result<Value> {
    let t = board::get_task(app, id)?;
    if t.i("pr_num").is_some() {
        if let Some(why) = crate::comments::task_refusal(app, &t, None, "handing the PR back for review")? {
            return err(409, why);
        }
    }
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        if t.i("pr_num").is_none() {
            return err(404, format!("{} has no PR linked.", rf("task", id)));
        }
        prflow::waited(app, &t)?;
        board::log_event(app, id, board::BOARD, "status", &format!("The agent finished its visit to PR #{}; the board watches it again", t.i0("pr_num")))?;
        Ok(())
    })?;
    get_pr(app, id)
}

fn pr_merged(app: &App, id: i64, body: &Value) -> Result<Value> {
    app.db.tx(|| {
        let t = board::get_task(app, id)?;
        if t.i("pr_num").is_none() {
            return err(404, format!("{} has no PR linked.", rf("task", id)));
        }
        let who = { let w = body_str(body, "who"); if w.is_empty() { board::BOARD.to_string() } else { w } };
        prflow::mark_merged(app, &t, &who)
    })?;
    // PRs stacked on this one go into its base now.
    crate::stack::retarget(app)?;
    get_pr(app, id)
}
