//! The work event stream, protocol v1 (`docs/STREAM.md`): one endpoint, `POST /stream/events`
//! (`tb stream send`), that any work relay pushes PR, review, comment, push, build, Jira and heartbeat
//! events into. The board defines the format and the relay adapts to it, so each event carries every
//! field the board needs without calling the host.
//!
//! A body is an envelope, `{v: 1, events: [...]}`, or one event on its own (`{v: 1, id, kind, ...}`).
//! Each event is checked on its own and answered on its own: `accepted`, `duplicate` (its `id` came
//! before) or `rejected` with why. An accepted event is kept in `stream_events` (its whole payload, so
//! later work can apply it) and routed: pr and build to the PR feed ([`feed::intake_from`]), review and
//! comment as hints to read their PR again when it's on the board, heartbeat
//! to each source it names, jira.issue to the ticket's task, epic or issue ([`crate::jira_status::apply`]),
//! jira.comment to QA comments ([`qa::intake`]); every kind counts for its
//! source's health. An event whose route refuses it is rejected and not kept, so the relay can send it
//! again.
//!
//! Each source (prs, builds, jira, qa) has a mode: board (the default, today's behaviour: the board
//! polls and a pr event is a hint to read that PR again) or stream (events are the source of truth and
//! the source's own poll rests while its stream is healthy). config.toml's `[stream.sources.<name>]`
//! sets it; `tb stream mode` overrides it on the board.
//!
//! A pr event without every v1 field is still accepted, kept with `complete = 0`: the board reads that
//! PR from its host instead of trusting the event, unless the event carries the PR's state, title,
//! branch and base and prs streams. Then the PR is updated from it, the rest kept from the last read,
//! and only the checks of a head it moved are read from the host.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{feed, fields, ops, p, qa};

pub const VERSION: i64 = 1;
pub const KINDS: &[&str] = &["pr", "review", "comment", "push", "build", "jira.issue", "jira.comment", "heartbeat"];
pub const SOURCES: &[&str] = &["prs", "builds", "jira", "qa"];
const PR_STATES: &[&str] = &["open", "merged", "declined"];
const CHECK_STATES: &[&str] = &["passed", "failed", "running", "stopped"];
const REVIEWER_STATES: &[&str] = &["approved", "changes", "commented", "pending"];
const REVIEW_STATES: &[&str] = &["approved", "changes", "commented", "dismissed"];
const BUILD_STATES: &[&str] = &["queued", "started", "running", "passed", "failed", "stopped"];
const STATUS_CATEGORIES: &[&str] = &["new", "indeterminate", "done"];
/// Every field of a pr event in v1: one without them all is incomplete.
const PR_FIELDS: &[&str] =
    &["provider", "repo", "num", "url", "title", "author", "state", "draft", "branch", "base", "head", "mergeable", "checks", "reviewers", "threads"];
const MAX_EVENTS: usize = 500;
const MAX_ID: usize = 200;
const KEEP_SECS: f64 = 30.0 * 86400.0;

type Check = std::result::Result<(), String>;

/// `POST /stream/events`: an envelope or one event; answers each event in order.
pub fn ingest(app: &App, body: &Value) -> Result<Value> {
    match body.get("v").and_then(|v| v.as_i64()) {
        Some(VERSION) => {}
        Some(n) => return err(400, format!("This board speaks stream protocol v{VERSION}, not v{n}.")),
        None => return err(400, format!("Say the protocol version: {{\"v\": {VERSION}, \"events\": [...]}}.")),
    }
    let events: Vec<&Value> = match body.get("events") {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(_) => return err(400, "events is a list of events."),
        None if body.get("kind").is_some() => vec![body],
        None => return err(400, "Send events: [...], or one event with its id and kind."),
    };
    if events.is_empty() {
        return err(400, "events is empty.");
    }
    if events.len() > MAX_EVENTS {
        return err(400, format!("Send at most {MAX_EVENTS} events at a time."));
    }
    prune(app)?;
    let results = events.into_iter().map(|ev| one(app, ev)).collect::<Result<Vec<_>>>()?;
    let count = |s: &str| results.iter().filter(|r| r["status"] == s).count();
    Ok(json!({"v": VERSION, "accepted": count("accepted"), "duplicate": count("duplicate"), "rejected": count("rejected"), "results": results}))
}

fn one(app: &App, ev: &Value) -> Result<Value> {
    let id = ev.get("id").and_then(|v| v.as_str()).map(str::trim).unwrap_or("").to_string();
    let kind = body_str(ev, "kind");
    let answer = |status: &str| json!({"id": if id.is_empty() { Value::Null } else { json!(id) }, "kind": kind, "status": status});
    let rejected = |why: String| {
        let mut a = answer("rejected");
        a["why"] = json!(why);
        a
    };
    let complete = match validate(ev) {
        Ok(c) => c,
        Err(why) => return Ok(rejected(why)),
    };
    let kept = app.db.tx(|| {
        if app.db.count("SELECT COUNT(*) FROM stream_events WHERE event_id = ?", p![id])? > 0 {
            return Ok(None);
        }
        let row = fields!["event_id" => id, "kind" => kind, "source" => body_str(ev, "source"), "at" => iso(parse_iso(&body_str(ev, "at")).unwrap_or(0.0)),
                          "received_at" => now_iso(), "complete" => complete as i64, "data" => jdumps(ev)];
        app.db.insert("stream_events", row).map(Some)
    })?;
    let Some(row) = kept else { return Ok(answer("duplicate")) };
    match route(app, &kind, ev, complete) {
        Ok(result) => {
            app.db.update("stream_events", &json!(row), fields!["result" => jdumps(&result)])?;
            let counts_for = if kind == "heartbeat" { heartbeat_sources(ev) } else { source_of(&kind).into_iter().collect() };
            feed::note_relay(app, &body_str(ev, "source"), &kind, ev, &counts_for)?;
            let mut a = answer("accepted");
            if kind == "pr" {
                a["complete"] = json!(complete);
            }
            if !result.is_null() {
                a["result"] = result;
            }
            Ok(a)
        }
        Err(e) if e.status < 500 => {
            app.db.x("DELETE FROM stream_events WHERE id = ?", p![row])?;
            Ok(rejected(e.message))
        }
        Err(e) => {
            app.db.x("DELETE FROM stream_events WHERE id = ?", p![row])?;
            Err(e)
        }
    }
}

fn prune(app: &App) -> Result<()> {
    app.db.x("DELETE FROM stream_events WHERE received_at < ?", p![iso(now_ts() - KEEP_SECS)])?;
    Ok(())
}

/// The event's checks: the envelope fields, then its kind's. `Ok(complete)`: whether a pr event
/// carries every v1 field (always true for the other kinds).
pub fn validate(ev: &Value) -> std::result::Result<bool, String> {
    if !ev.is_object() {
        return Err("An event is an object.".into());
    }
    match ev.get("id") {
        Some(Value::String(s)) if !s.trim().is_empty() && s.trim().len() <= MAX_ID => {}
        Some(Value::String(s)) if s.trim().len() > MAX_ID => return Err(format!("id is at most {MAX_ID} characters.")),
        _ => return Err("Every event needs an id: a string unique to it, so a resend is a duplicate.".into()),
    }
    let kind = body_str(ev, "kind");
    if !KINDS.contains(&kind.as_str()) {
        return Err(format!("kind is one of {}.", KINDS.join(", ")));
    }
    need_time(ev, "at")?;
    need(ev, "source")?;
    match kind.as_str() {
        "pr" => return pr(ev),
        "review" => review(ev)?,
        "comment" => comment(ev)?,
        "push" => push(ev)?,
        "build" => build(ev)?,
        "jira.issue" => jira_issue(ev)?,
        "jira.comment" => jira_comment(ev)?,
        _ => heartbeat(ev)?,
    }
    Ok(true)
}

fn pr(ev: &Value) -> std::result::Result<bool, String> {
    pr_ref(ev, "a pr event")?;
    one_of(ev, "state", PR_STATES)?;
    for k in ["draft", "mergeable"] {
        bool_or_null(ev, k)?;
    }
    list(ev, "checks", |c| {
        need(c, "name")?;
        need_one_of(c, "state", CHECK_STATES)?;
        opt_time(c, "started_at")?;
        opt_time(c, "finished_at")
    })?;
    list(ev, "reviewers", |r| {
        need(r, "who")?;
        need_one_of(r, "state", REVIEWER_STATES)?;
        opt_time(r, "at")
    })?;
    list(ev, "threads", |t| {
        need(t, "id")?;
        opt_line(t)?;
        bool_or_null(t, "resolved")?;
        list(t, "comments", |c| {
            for k in ["id", "author", "body"] {
                need(c, k)?;
            }
            need_time(c, "at")
        })
    })?;
    Ok(PR_FIELDS.iter().all(|k| ev.get(k).is_some()))
}

fn review(ev: &Value) -> Check {
    pr_ref(&ev["pr"], "pr")?;
    need(ev, "who")?;
    need_one_of(ev, "state", REVIEW_STATES)
}

fn comment(ev: &Value) -> Check {
    pr_ref(&ev["pr"], "pr")?;
    for k in ["comment_id", "author", "body"] {
        need(ev, k)?;
    }
    opt_line(ev)
}

fn push(ev: &Value) -> Check {
    for k in ["repo", "branch", "after"] {
        need(ev, k)?;
    }
    bool_or_null(ev, "forced")?;
    bool_or_null(ev, "mine")?;
    list(ev, "commits", |c| need(c, "sha").map(|_| ()))
}

fn build(ev: &Value) -> Check {
    for k in ["repo", "head"] {
        need(ev, k)?;
    }
    need_one_of(ev, "state", BUILD_STATES)?;
    bool_or_null(ev, "mine")?;
    if ev.get("pr").is_some_and(|v| !v.is_null()) {
        pr_ref(&ev["pr"], "pr")?;
    }
    Ok(())
}

fn jira_issue(ev: &Value) -> Check {
    jira_key(ev)?;
    need(ev, "status")?;
    one_of(ev, "status_category", STATUS_CATEGORIES)
}

fn jira_comment(ev: &Value) -> Check {
    jira_key(ev)?;
    let id = need(ev, "comment_id")?;
    if !id.chars().all(|c| c.is_ascii_digit()) {
        return Err("comment_id is Jira's comment id: digits.".into());
    }
    for k in ["author", "body"] {
        need(ev, k)?;
    }
    Ok(())
}

fn heartbeat(ev: &Value) -> Check {
    if let Some(s) = ev.get("sources").filter(|v| !v.is_null()) {
        let ok = s.as_array().is_some_and(|a| a.iter().all(|x| x.as_str().is_some_and(|x| SOURCES.contains(&x))));
        if !ok {
            return Err(format!("sources is a list of {}.", SOURCES.join(", ")));
        }
    }
    bool_or_null(ev, "active")?;
    opt_time(ev, "idle_until")?;
    opt_time(ev, "connected_at")?;
    list(ev, "covers", |c| {
        need(c, "provider")?;
        match c.get("repo") {
            None | Some(Value::Null) | Some(Value::String(_)) => Ok(()),
            _ => Err("repo is a string.".into()),
        }
    })
}

fn need(v: &Value, k: &str) -> std::result::Result<String, String> {
    let s = body_str(v, k);
    if s.is_empty() || v[k].is_object() || v[k].is_array() {
        return Err(format!("{k} is missing."));
    }
    Ok(s)
}

fn need_time(v: &Value, k: &str) -> Check {
    need(v, k)?;
    opt_time(v, k)
}

fn opt_time(v: &Value, k: &str) -> Check {
    let s = body_str(v, k);
    if !s.is_empty() && parse_iso(&s).is_none() {
        return Err(format!("{k} is a time like 2026-10-12T15:04:00Z."));
    }
    Ok(())
}

fn one_of(v: &Value, k: &str, allowed: &[&str]) -> Check {
    let s = body_str(v, k);
    if !s.is_empty() && !allowed.contains(&s.as_str()) {
        return Err(format!("{k} is one of {}.", allowed.join(", ")));
    }
    Ok(())
}

fn need_one_of(v: &Value, k: &str, allowed: &[&str]) -> Check {
    need(v, k).map_err(|_| format!("{k} is missing: one of {}.", allowed.join(", ")))?;
    one_of(v, k, allowed)
}

fn bool_or_null(v: &Value, k: &str) -> Check {
    match v.get(k) {
        None | Some(Value::Null) | Some(Value::Bool(_)) => Ok(()),
        _ => Err(format!("{k} is true or false.")),
    }
}

fn opt_line(v: &Value) -> Check {
    match v.get("line") {
        None | Some(Value::Null) => Ok(()),
        Some(l) if l.as_i64().is_some_and(|n| n > 0) => Ok(()),
        _ => Err("line is a line number.".into()),
    }
}

fn list(v: &Value, k: &str, each: impl Fn(&Value) -> Check) -> Check {
    match v.get(k) {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Array(a)) => a.iter().enumerate().try_for_each(|(i, x)| {
            if !x.is_object() {
                return Err(format!("{k}[{i}] is an object."));
            }
            each(x).map_err(|e| format!("{k}[{i}]: {e}"))
        }),
        _ => Err(format!("{k} is a list.")),
    }
}

/// Which PR: its `url`, or its `repo` and `num`.
fn pr_ref(v: &Value, what: &str) -> Check {
    if !v.is_object() {
        return Err(format!("{what} is missing: {{repo, num}} or {{url}}."));
    }
    if find_pr(&body_str(v, "url")).is_some() {
        return Ok(());
    }
    if !body_str(v, "repo").is_empty() && num_of(v) > 0 {
        return Ok(());
    }
    Err(format!("{what} needs the PR's url, or its repo and num."))
}

fn num_of(v: &Value) -> i64 {
    v["num"].as_i64().unwrap_or(0)
}

fn jira_key(ev: &Value) -> Check {
    ops::jira_key(&body_str(ev, "key")).map(|_| ()).map_err(|e| e.message)
}

/// The sources a heartbeat names, else every source.
fn heartbeat_sources(ev: &Value) -> Vec<&str> {
    let named: Vec<&str> = ev["sources"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    if named.is_empty() { SOURCES.to_vec() } else { named }
}

/// The source an event kind counts for: its health hears from it.
fn source_of(kind: &str) -> Option<&'static str> {
    match kind {
        "pr" | "review" | "comment" => Some("prs"),
        "push" | "build" => Some("builds"),
        "jira.issue" => Some("jira"),
        "jira.comment" => Some("qa"),
        _ => None,
    }
}

/// Whether an incomplete pr event still says what the PR is (its state, title, branch and base): in
/// stream mode the PR is updated from it, keeping the rest of the last read and reading from the host
/// only what it moved without carrying (`prflow::apply_event`).
fn carries_state(ev: &Value) -> bool {
    ["state", "title", "branch", "base"].iter().all(|k| ev.get(*k).is_some_and(|v| !v.is_null()))
}

fn route(app: &App, kind: &str, ev: &Value, complete: bool) -> Result<Value> {
    match kind {
        "pr" => {
            let apply = ((complete || carries_state(ev)) && streams(app, "prs")).then_some(ev);
            feed::intake_from(app, &as_feed_pr(ev), &["prs"], apply)
        }
        "build" => {
            let out = feed::intake_from(app, &as_feed_build(ev), &["builds"], None)?;
            Ok(match crate::breaks::stream_build(app, ev)? {
                Some(watch) => json!({"feed": out, "master": watch}),
                None => out,
            })
        }
        "review" | "comment" => pr_hint(app, kind, ev),
        "push" => {
            feed::note_event_for(app, "builds", kind, ev, None)?;
            Ok(json!({"builds": crate::prbuilds::on_push_build(app, &as_push_build(ev))?}))
        }
        "heartbeat" => {
            let sources = heartbeat_sources(ev);
            let hb = json!({"active": ev["active"], "idle_until": ev["idle_until"], "connected_at": ev["connected_at"]});
            app.db.tx(|| feed::heartbeat_for(app, &hb, &sources))?;
            Ok(json!({"sources": sources}))
        }
        "jira.issue" => {
            feed::note_event_for(app, "jira", kind, ev, None)?;
            crate::jira_status::apply(app, ev)
        }
        "jira.comment" => {
            feed::note_event_for(app, "qa", kind, ev, None)?;
            qa::intake(app, &json!({"key": ev["key"], "comment_id": body_str(ev, "comment_id"), "author": ev["author"], "text": ev["body"]}))
        }
        _ => {
            if let Some(src) = source_of(kind) {
                feed::note_event_for(app, src, kind, ev, None)?;
            }
            Ok(Value::Null)
        }
    }
}

const MODES_SETTING: &str = "stream_modes";
pub const MODES: &[&str] = &["board", "stream"];

/// `[stream]` in config.toml: each source's mode and, for builds, jira and qa, its stream's health
/// settings (prs keeps `[feed]`'s).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StreamConfig {
    pub sources: BTreeMap<String, SourceConfig>,
}

/// `[stream.sources.<name>]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SourceConfig {
    /// board (the board's own poll; events are hints) or stream (events are the source of truth).
    pub mode: String,
    pub restart_cmd: String,
    pub stuck_secs: f64,
    pub silent_secs: f64,
    pub restart_mins: Vec<f64>,
    pub settle_secs: f64,
}

impl Default for SourceConfig {
    fn default() -> Self {
        let f = feed::FeedConfig::default();
        SourceConfig {
            mode: "board".into(),
            restart_cmd: String::new(),
            stuck_secs: f.stuck_secs,
            silent_secs: f.silent_secs,
            restart_mins: f.restart_mins,
            settle_secs: f.settle_secs,
        }
    }
}

pub fn source_config(app: &App, name: &str) -> SourceConfig {
    app.cfg.stream.sources.get(name).cloned().unwrap_or_default()
}

pub fn label(name: &str) -> &'static str {
    match name {
        "prs" => "PR",
        "builds" => "builds",
        "jira" => "Jira",
        _ => "QA",
    }
}

fn overrides(app: &App) -> Row {
    jloads_obj(app.db.get_setting(MODES_SETTING).ok().flatten().as_deref())
}

/// A source's mode and where it comes from: `tb stream mode` (override), config.toml (config), or
/// neither (default, board).
pub fn mode(app: &App, name: &str) -> (&'static str, &'static str) {
    let pick = |m: &str| MODES.iter().copied().find(|x| x.eq_ignore_ascii_case(m.trim()));
    if let Some(m) = overrides(app).s(name).and_then(pick) {
        return (m, "override");
    }
    match app.cfg.stream.sources.get(name).and_then(|c| pick(&c.mode)) {
        Some(m) => (m, "config"),
        None => ("board", "default"),
    }
}

/// The stream drives this source: its events are applied as they are.
pub fn streams(app: &App, name: &str) -> bool {
    mode(app, name).0 == "stream"
}

/// `POST /stream/mode {source, mode}` (`tb stream mode`): board or stream overrides config.toml's mode;
/// default drops the override.
pub fn set_mode(app: &App, body: &Value) -> Result<Value> {
    let name = body_str(body, "source").to_lowercase();
    if !SOURCES.contains(&name.as_str()) {
        return err(400, format!("source is one of {}.", SOURCES.join(", ")));
    }
    let m = body_str(body, "mode").to_lowercase();
    let mut o = overrides(app);
    match m.as_str() {
        "board" | "stream" => o.insert(name.clone(), json!(m)),
        "default" => o.remove(&name),
        _ => return err(400, "mode is board, stream or default."),
    };
    app.db.set_setting(MODES_SETTING, Some(&jdumps(&Value::Object(o))))?;
    Ok(overview(app))
}

/// `GET /stream` (`tb stream`): each source's mode and its stream's health.
pub fn overview(app: &App) -> Value {
    let sources: Vec<Value> = SOURCES
        .iter()
        .map(|name| {
            let (m, from) = mode(app, name);
            let mut h = feed::health_of(app, &feed::source(app, name));
            h["source"] = json!(name);
            h["mode"] = json!(m);
            h["mode_from"] = json!(from);
            h
        })
        .collect();
    json!({"v": VERSION, "sources": sources})
}

/// A review or comment on a PR is a hint, like an incomplete pr event: a PR on the board is read again
/// from its host, and the result names its task. Any other PR only counts for the prs source's health.
fn pr_hint(app: &App, kind: &str, ev: &Value) -> Result<Value> {
    let pr = &ev["pr"];
    let t = feed::task_of_event(app, &json!({"url": pr["url"], "host": pr["provider"], "repo": pr["repo"], "num": pr["num"]}))?;
    feed::note_event_for(app, "prs", kind, ev, t.as_ref().map(|t| t.id()))?;
    let Some(t) = t else { return Ok(Value::Null) };
    let mut out = json!({"ok": true, "kind": kind, "task": rf("task", t.id())});
    feed::read_pr(app, &t, None, &mut out)?;
    app.db.tx(|| crate::prflow::settle_after_hint(app, &t))?;
    Ok(out)
}

/// A pr event as the PR feed's own event (`POST /prs/event`).
fn as_feed_pr(ev: &Value) -> Value {
    json!({"kind": "pr", "url": ev["url"], "host": ev["provider"], "repo": ev["repo"], "num": ev["num"], "state": ev["state"],
           "author": ev["author"], "branch": ev["branch"], "head": ev["head"], "created_at": ev["created_at"], "mine": ev["mine"], "source": ev["source"]})
}

/// A build event as the PR feed's own build event: the PR it's on (if any) names the task.
fn as_feed_build(ev: &Value) -> Value {
    let pr = &ev["pr"];
    let repo = if body_str(pr, "repo").is_empty() { ev["repo"].clone() } else { pr["repo"].clone() };
    json!({"kind": "build", "url": pr["url"], "host": pr["provider"], "repo": repo, "num": pr["num"], "state": ev["state"], "head": ev["head"],
           "branch": ev["branch"], "provider": ev["provider"], "pipeline": ev["pipeline"], "build_url": ev["url"], "author": ev["author"],
           "mine": ev["mine"], "source": ev["source"]})
}

/// A push as the build it starts on its new head, for the owner's PR builds (`prbuilds::on_push_build`,
/// as `POST /pr-builds/event` takes one): a push to one of the owner's PRs' branches, or by the owner.
fn as_push_build(ev: &Value) -> Value {
    json!({"kind": "build", "repo": ev["repo"], "branch": ev["branch"], "head": ev["after"], "state": "started", "author": ev["author"],
           "mine": ev["mine"], "provider": ev["provider"], "source": ev["source"]})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &str, extra: Value) -> Value {
        let mut e = json!({"id": "e1", "kind": kind, "at": "2026-10-12T15:04:00Z", "source": "relay"});
        for (k, v) in extra.as_object().unwrap() {
            e[k] = v.clone();
        }
        e
    }

    #[test]
    fn every_event_needs_its_envelope_fields() {
        assert!(validate(&json!({"kind": "push"})).unwrap_err().contains("id"));
        assert!(validate(&json!({"id": "x", "kind": "nope"})).unwrap_err().starts_with("kind"));
        assert!(validate(&json!({"id": "x", "kind": "heartbeat", "source": "r"})).unwrap_err().starts_with("at"));
        assert!(validate(&json!({"id": "x", "kind": "heartbeat", "at": "soon", "source": "r"})).unwrap_err().starts_with("at"));
        assert!(validate(&json!({"id": "x", "kind": "heartbeat", "at": "2026-10-12T15:04:00Z"})).unwrap_err().starts_with("source"));
        assert_eq!(validate(&ev("heartbeat", json!({}))), Ok(true));
    }

    #[test]
    fn a_pr_event_is_complete_only_with_every_field() {
        assert_eq!(validate(&ev("pr", json!({"repo": "acme/webapp", "num": 9}))), Ok(false));
        assert!(validate(&ev("pr", json!({"repo": "acme/webapp"}))).unwrap_err().contains("url"));
        assert!(validate(&ev("pr", json!({"repo": "acme/webapp", "num": 9, "state": "closed"}))).unwrap_err().starts_with("state"));
        let bad = validate(&ev("pr", json!({"repo": "acme/webapp", "num": 9, "threads": [{"id": "t1", "comments": [{"id": "c1", "author": "sam"}]}]})));
        assert_eq!(bad.unwrap_err(), "threads[0]: comments[0]: body is missing.");
        let full = ev("pr", json!({"provider": "bitbucket", "repo": "acme/webapp", "num": 9, "url": "https://bitbucket.org/acme/webapp/pull-requests/9",
            "title": "Login", "author": "sam", "state": "open", "draft": false, "branch": "login", "base": "main", "head": "abc", "mergeable": null,
            "checks": [{"name": "build", "state": "running", "url": null, "started_at": "2026-10-12T15:00:00Z", "finished_at": null}],
            "reviewers": [{"who": "pat", "state": "pending", "at": null}],
            "threads": [{"id": "t1", "path": "a.rs", "line": 3, "resolved": false, "comments": [{"id": "c1", "author": "pat", "body": "Why?", "at": "2026-10-12T15:01:00Z"}]}]}));
        assert_eq!(validate(&full), Ok(true));
    }

    #[test]
    fn each_kind_checks_its_own_fields() {
        let pr = json!({"repo": "acme/webapp", "num": 9});
        assert!(validate(&ev("review", json!({"pr": pr, "who": "pat"}))).unwrap_err().starts_with("state"));
        assert!(validate(&ev("review", json!({"pr": pr, "who": "pat", "state": "approved"}))).is_ok());
        assert!(validate(&ev("comment", json!({"who": "pat"}))).unwrap_err().starts_with("pr"));
        assert!(validate(&ev("comment", json!({"pr": pr, "comment_id": "c1", "author": "pat", "body": "Hm", "line": 0}))).unwrap_err().starts_with("line"));
        assert!(validate(&ev("push", json!({"repo": "acme/webapp", "branch": "main", "after": "abc", "commits": [{"message": "x"}]}))).unwrap_err().contains("sha"));
        assert!(validate(&ev("build", json!({"repo": "acme/webapp", "head": "abc", "state": "done"}))).unwrap_err().starts_with("state"));
        assert!(validate(&ev("jira.issue", json!({"key": "nope", "status": "Done"}))).unwrap_err().contains("Jira key"));
        assert!(validate(&ev("jira.issue", json!({"key": "PROJ-1", "status": "Done", "status_category": "finished"}))).unwrap_err().starts_with("status_category"));
        assert!(validate(&ev("jira.comment", json!({"key": "PROJ-1", "comment_id": "c1", "author": "a", "body": "b"}))).unwrap_err().contains("digits"));
        assert!(validate(&ev("heartbeat", json!({"sources": ["prs", "mail"]}))).unwrap_err().starts_with("sources"));
    }
}
