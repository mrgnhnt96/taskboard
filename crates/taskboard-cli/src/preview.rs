//! `tb preview <url>`: the card Midna shows when a taskboard:// link is hovered.
//!
//! Midna runs the command the app's Info.plist names (`MidnaLinkPreview`) with the URL and reads one
//! JSON object: {title, subtitle?, status?: {label, tone: ok|warn|err|busy|idle}, fields?: [[label,
//! value]] (≤6), lines?: [str] (≤4), progress?: {done, total}, open_label?}. A non-zero exit (unknown
//! ref, board down) makes Midna fall back to "Opens in Taskboard". It only reads.

use chrono::{DateTime, TimeZone};
use serde_json::{json, Value};

use crate::client::{self, CallError};

/// Midna gives the whole command 1.5s.
const BOARD_TIMEOUT: f64 = 1.0;
const MAX_LINES: usize = 4;
const LINE_MAX: usize = 160;

#[derive(Debug, PartialEq)]
pub enum Target {
    Task(i64),
    Goal(i64),
    Issue(i64),
}

/// The task, goal or backlog item a taskboard:// link names: taskboard://task/T6, taskboard://goal/G3,
/// taskboard://issue/B4, taskboard://#/?task=T6, taskboard://tasks#/goals/G5 (a bare number works
/// too). Any taskboard
/// scheme: Taskboard Dev's is taskboard-dev://.
pub fn parse_url(url: &str) -> Option<Target> {
    let (scheme, rest) = url.trim().split_once(':')?;
    if !scheme.to_ascii_lowercase().starts_with("taskboard") {
        return None;
    }
    let words: Vec<&str> = rest.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    words.windows(2).rev().find_map(|w| {
        let letter = match w[0].to_ascii_lowercase().as_str() {
            "task" | "tasks" => 't',
            "goal" | "goals" => 'g',
            "issue" | "issues" => 'b',
            _ => return None,
        };
        let digits = match w[1].strip_prefix(|c: char| c.is_ascii_alphabetic()) {
            Some(d) if w[1].starts_with([letter, letter.to_ascii_uppercase()]) => d,
            Some(_) => return None,
            None => w[1],
        };
        let n = digits.parse::<i64>().ok().filter(|n| *n > 0)?;
        Some(match letter {
            'g' => Target::Goal(n),
            'b' => Target::Issue(n),
            _ => Target::Task(n),
        })
    })
}

/// `port`: the board to ask when it isn't this machine's usual one (Taskboard Dev's).
pub fn run(url: &str, port: Option<u16>) -> Result<i32, String> {
    let target = parse_url(url).ok_or_else(|| format!("not a task, goal or backlog link: {url}"))?;
    let mut cfg = client::config();
    if let Some(p) = port {
        cfg.port = p;
    }
    let path = match target {
        Target::Task(n) => format!("/tasks/T{n}"),
        Target::Goal(n) => format!("/goals/G{n}"),
        Target::Issue(n) => format!("/backlog/B{n}"),
    };
    let v = client::request(&cfg, "GET", &path, None, BOARD_TIMEOUT).map_err(|e| match e {
        CallError::Refused(m) | CallError::Unreachable(m) | CallError::TimedOut(m) => m,
    })?;
    let card = match target {
        Target::Task(_) => task_preview(&v, &chrono::Local::now()),
        Target::Goal(_) => goal_preview(&v),
        Target::Issue(_) => issue_preview(&v),
    };
    println!("{card}");
    Ok(0)
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

fn status(label: &str, tone: &str) -> Value {
    json!({"label": label, "tone": tone})
}

fn clip(text: &str) -> String {
    let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= LINE_MAX {
        return one;
    }
    format!("{}…", one.chars().take(LINE_MAX - 1).collect::<String>().trim_end())
}

fn task_status(t: &Value) -> Value {
    if t["failed"] == true {
        return status("Failed", "err");
    }
    match s(t, "status") {
        "done" => status("Done", "ok"),
        "needs" => status("Needs you", "warn"),
        "working" => status("Working", "busy"),
        "queued" if t["blocked"] == true => status("Blocked", "warn"),
        "queued" if !t["waiting"].is_null() => status("Waiting", "idle"),
        "queued" => status("Queued", "idle"),
        _ => status("Planned", "idle"),
    }
}

fn pr_field(pr: &Value) -> Option<String> {
    let num = pr["num"].as_i64()?;
    let state = match s(pr, "state") {
        "MERGED" => Some("merged"),
        "DECLINED" => Some("closed"),
        _ => match s(pr, "checks") {
            "pass" => Some("checks pass"),
            "fail" => Some("checks failing"),
            "pending" => Some("checks running"),
            _ => None,
        },
    };
    let repo = s(pr, "repo");
    let name = if repo.is_empty() { format!("#{num}") } else { format!("{repo}#{num}") };
    Some(match state {
        Some(st) => format!("{name} · {st}"),
        None => name,
    })
}

/// "2:05 PM" today, "Oct 3, 2:05 PM" another day.
pub fn clock<Tz: TimeZone>(at: &str, now: &DateTime<Tz>) -> Option<String>
where
    Tz::Offset: std::fmt::Display,
{
    let at = DateTime::parse_from_rfc3339(at).ok()?.with_timezone(&now.timezone());
    let time = at.format("%-I:%M %p").to_string();
    Some(if at.date_naive() == now.date_naive() { time } else { format!("{}, {time}", at.format("%b %-d")) })
}

pub fn task_preview<Tz: TimeZone>(t: &Value, now: &DateTime<Tz>) -> Value
where
    Tz::Offset: std::fmt::Display,
{
    let goal = &t["goal"];
    let subtitle = match goal["ref"].as_str() {
        Some(g) => format!("{g} · {} · {}", s(goal, "name"), s(t, "project")),
        None => s(t, "project").to_string(),
    };
    let mut fields: Vec<Value> = vec![];
    if let Some(name) = t["session"]["name"].as_str() {
        fields.push(json!(["Terminal", name]));
    }
    let branch = s(&t["context"]["where"], "branch");
    if !branch.is_empty() {
        fields.push(json!(["Branch", branch]));
    }
    if let Some(pr) = pr_field(&t["pr"]) {
        fields.push(json!(["PR", pr]));
    }
    if let Some(when) = t["updated_at"].as_str().and_then(|a| clock(a, now)) {
        fields.push(json!(["Updated", when]));
    }
    let ctx = &t["context"];
    let said = |key: &str, word: &str| -> Vec<String> {
        ctx[key].as_array().into_iter().flatten().filter_map(|x| x.as_str()).map(|x| clip(&format!("{word}: {x}"))).collect()
    };
    let mut lines = said("done", "Done");
    lines.truncate(MAX_LINES / 2);
    lines.extend(said("next", "Next"));
    if lines.is_empty() && !s(t, "latest").is_empty() {
        lines.push(clip(s(t, "latest")));
    }
    lines.truncate(MAX_LINES);
    json!({
        "title": format!("{} · {}", s(t, "ref"), s(t, "title")),
        "subtitle": subtitle,
        "status": task_status(t),
        "fields": fields,
        "lines": lines,
        "open_label": "Open task",
    })
}

fn goal_status(g: &Value) -> Value {
    let n = |key: &str| g[key].as_i64().unwrap_or(0);
    let (done, total) = (n("done"), n("total"));
    if n("active") > 0 {
        status(&format!("{} running", n("active")), "busy")
    } else if n("needs") > 0 {
        status("Needs you", "warn")
    } else if n("review") > 0 {
        status("In review", "busy")
    } else if total > 0 && done >= total {
        status("Done", "ok")
    } else if n("blocked") > 0 {
        status("Blocked", "warn")
    } else if g["paused"] == true {
        status("Paused", "idle")
    } else if n("queued") > 0 || n("starting") > 0 {
        status("Queued", "idle")
    } else if n("failed") > 0 {
        // Failed tasks the goal moved past say so only once nothing else is live.
        status("Failed", "err")
    } else {
        status("Idle", "idle")
    }
}

pub fn goal_preview(g: &Value) -> Value {
    let (done, total) = (g["done"].as_i64().unwrap_or(0), g["total"].as_i64().unwrap_or(0));
    let running: Vec<String> = g["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| matches!(s(t, "status"), "working" | "needs") && t["failed"] != true)
        .map(|t| clip(&format!("{} · {}", s(t, "ref"), s(t, "title"))))
        .take(MAX_LINES)
        .collect();
    let mut fields = vec![json!(["Tasks", format!("{done} of {total} done")])];
    if let Some(prs) = g["prs_open"].as_array().filter(|p| !p.is_empty()) {
        fields.push(json!(["Open PRs", prs.len().to_string()]));
    }
    let mut card = json!({
        "title": format!("{} · {}", s(g, "ref"), s(g, "name")),
        "subtitle": s(g, "project"),
        "status": goal_status(g),
        "fields": fields,
        "lines": running,
        "open_label": "Open goal",
    });
    if total > 0 {
        card["progress"] = json!({"done": done.min(total), "total": total});
    }
    card
}

fn issue_status(b: &Value) -> Value {
    match s(b, "state") {
        "task" => status("Now a task", "ok"),
        "ticket" => status(b["jira_key"].as_str().unwrap_or("Ticket asked for"), "busy"),
        "drop" => status("Won’t do", "idle"),
        "defer" => status("Deferred", "idle"),
        "closed" => status("Closed", "ok"),
        _ => status("Open", "warn"),
    }
}

pub fn issue_preview(b: &Value) -> Value {
    let goal = &b["goal"];
    let subtitle = match goal["ref"].as_str() {
        Some(g) => format!("{g} · {} · {}", s(goal, "name"), s(b, "project")),
        None => s(b, "project").to_string(),
    };
    let mut fields: Vec<Value> = vec![];
    if !s(b, "kind_label").is_empty() {
        fields.push(json!(["Kind", s(b, "kind_label")]));
    }
    if let Some(t) = b["task_ref"].as_str() {
        fields.push(json!(["Task", t]));
    }
    if !s(b, "from").is_empty() {
        fields.push(json!(["From", s(b, "from")]));
    }
    let lines: Vec<String> = s(b, "detail").lines().map(str::trim).filter(|l| !l.is_empty()).map(clip).take(MAX_LINES).collect();
    json!({
        "title": format!("{} · {}", s(b, "ref"), s(b, "title")),
        "subtitle": subtitle,
        "status": issue_status(b),
        "fields": fields,
        "lines": lines,
        "open_label": "Open item",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn reads_every_link_shape() {
        assert_eq!(parse_url("taskboard://task/T6"), Some(Target::Task(6)));
        assert_eq!(parse_url("taskboard://goal/G3"), Some(Target::Goal(3)));
        assert_eq!(parse_url("taskboard://#/?task=T6"), Some(Target::Task(6)));
        assert_eq!(parse_url("taskboard://tasks#/goals/G5"), Some(Target::Goal(5)));
        assert_eq!(parse_url("taskboard://task/t12"), Some(Target::Task(12)));
        assert_eq!(parse_url("taskboard://task/12"), Some(Target::Task(12)));
        assert_eq!(parse_url("taskboard://tasks#/?goal=G2&task=T9"), Some(Target::Task(9)));
        assert_eq!(parse_url("taskboard-dev://task/T6"), Some(Target::Task(6)));
        assert_eq!(parse_url("taskboard://issue/B4"), Some(Target::Issue(4)));
        assert_eq!(parse_url("taskboard-dev://issue/b4"), Some(Target::Issue(4)));
    }

    #[test]
    fn refuses_links_that_name_nothing() {
        assert_eq!(parse_url("taskboard://"), None);
        assert_eq!(parse_url("taskboard://tasks"), None);
        assert_eq!(parse_url("taskboard://task/G6"), None);
        assert_eq!(parse_url("taskboard://goal/T6"), None);
        assert_eq!(parse_url("taskboard://issue/T6"), None);
        assert_eq!(parse_url("taskboard://task/T0"), None);
        assert_eq!(parse_url("https://example.com/task/T6"), None);
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 12, 18, 0, 0).unwrap()
    }

    fn task(extra: Value) -> Value {
        let mut t = json!({
            "ref": "T6", "title": "Fix the thing", "project": "midna", "status": "working", "failed": false,
            "blocked": false, "waiting": null, "updated_at": "2026-10-12T17:05:00Z",
            "goal": {"ref": "G2", "name": "Polish"},
            "session": {"name": "T6 Fix the thing"},
            "pr": null,
            "context": {"where": {"branch": "feat/thing"}, "done": ["Wrote the parser"], "next": ["Add tests"]},
        });
        for (k, v) in extra.as_object().unwrap() {
            t[k] = v.clone();
        }
        t
    }

    #[test]
    fn a_working_task_shows_its_terminal_branch_and_checkpoint() {
        assert_eq!(
            task_preview(&task(json!({})), &now()),
            json!({
                "title": "T6 · Fix the thing",
                "subtitle": "G2 · Polish · midna",
                "status": {"label": "Working", "tone": "busy"},
                "fields": [["Terminal", "T6 Fix the thing"], ["Branch", "feat/thing"], ["Updated", "5:05 PM"]],
                "lines": ["Done: Wrote the parser", "Next: Add tests"],
                "open_label": "Open task",
            })
        );
    }

    #[test]
    fn each_task_state_has_its_pill() {
        let pill = |extra: Value| task_preview(&task(extra), &now())["status"].clone();
        assert_eq!(pill(json!({"status": "done"})), json!({"label": "Done", "tone": "ok"}));
        assert_eq!(pill(json!({"status": "done", "failed": true})), json!({"label": "Failed", "tone": "err"}));
        assert_eq!(pill(json!({"status": "needs"})), json!({"label": "Needs you", "tone": "warn"}));
        assert_eq!(pill(json!({"status": "queued", "blocked": true})), json!({"label": "Blocked", "tone": "warn"}));
        assert_eq!(pill(json!({"status": "queued", "waiting": "Waits for T5"})), json!({"label": "Waiting", "tone": "idle"}));
        assert_eq!(pill(json!({"status": "queued"})), json!({"label": "Queued", "tone": "idle"}));
        assert_eq!(pill(json!({"status": "planned"})), json!({"label": "Planned", "tone": "idle"}));
    }

    #[test]
    fn a_finished_task_shows_its_pr_and_checks() {
        let pr = |state: &str, checks: Value| {
            let t = task(json!({"status": "done", "session": null, "pr": {"repo": "midna", "num": 26, "state": state, "checks": checks}}));
            task_preview(&t, &now())["fields"].clone()
        };
        assert_eq!(pr("OPEN", json!("pass"))[1], json!(["PR", "midna#26 · checks pass"]));
        assert_eq!(pr("OPEN", json!("fail"))[1], json!(["PR", "midna#26 · checks failing"]));
        assert_eq!(pr("OPEN", json!("pending"))[1], json!(["PR", "midna#26 · checks running"]));
        assert_eq!(pr("OPEN", Value::Null)[1], json!(["PR", "midna#26"]));
        assert_eq!(pr("MERGED", json!("pass"))[1], json!(["PR", "midna#26 · merged"]));
        assert_eq!(pr("OPEN", json!("pass"))[0], json!(["Branch", "feat/thing"]));
    }

    #[test]
    fn a_task_without_a_goal_or_checkpoint_says_what_it_has() {
        let t = task(json!({"goal": null, "session": null, "context": null, "latest": "Started on it", "updated_at": "2026-10-03T21:14:00Z"}));
        let p = task_preview(&t, &now());
        assert_eq!(p["subtitle"], json!("midna"));
        assert_eq!(p["fields"], json!([["Updated", "Oct 3, 9:14 PM"]]));
        assert_eq!(p["lines"], json!(["Started on it"]));
    }

    #[test]
    fn long_checkpoints_are_cut_to_four_short_lines() {
        let long = "word ".repeat(100);
        let t = task(json!({"context": {"done": [long, "b", "c"], "next": ["d", "e", "f"]}}));
        let lines = task_preview(&t, &now())["lines"].as_array().unwrap().clone();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].as_str().unwrap().ends_with('…'));
        assert!(lines[0].as_str().unwrap().chars().count() <= LINE_MAX);
        assert_eq!(lines[2], json!("Next: d"));
    }

    #[test]
    fn clock_is_twelve_hour() {
        assert_eq!(clock("2026-10-12T00:07:00Z", &now()).as_deref(), Some("12:07 AM"));
        assert_eq!(clock("2026-10-12T13:30:00Z", &now()).as_deref(), Some("1:30 PM"));
        assert_eq!(clock("nope", &now()), None);
    }

    fn goal(extra: Value) -> Value {
        let mut g = json!({
            "ref": "G5", "name": "Stream protocol", "project": "taskboard", "done": 2, "total": 4,
            "active": 1, "needs": 0, "failed": 0, "blocked": 0, "queued": 1, "starting": 0, "paused": false,
            "prs_open": [12],
            "tasks": [
                {"ref": "T1", "title": "Done one", "status": "done", "failed": false},
                {"ref": "T3", "title": "Running one", "status": "working", "failed": false},
                {"ref": "T4", "title": "Queued one", "status": "queued", "failed": false},
            ],
        });
        for (k, v) in extra.as_object().unwrap() {
            g[k] = v.clone();
        }
        g
    }

    #[test]
    fn a_goal_shows_its_progress_and_running_tasks() {
        assert_eq!(
            goal_preview(&goal(json!({}))),
            json!({
                "title": "G5 · Stream protocol",
                "subtitle": "taskboard",
                "status": {"label": "1 running", "tone": "busy"},
                "fields": [["Tasks", "2 of 4 done"], ["Open PRs", "1"]],
                "lines": ["T3 · Running one"],
                "progress": {"done": 2, "total": 4},
                "open_label": "Open goal",
            })
        );
    }

    #[test]
    fn each_goal_state_has_its_pill() {
        let pill = |extra: Value| goal_preview(&goal(extra))["status"].clone();
        let idle = json!({"active": 0, "queued": 0});
        let with = |more: Value| {
            let mut v = idle.clone();
            for (k, x) in more.as_object().unwrap() {
                v[k] = x.clone();
            }
            v
        };
        assert_eq!(pill(with(json!({"needs": 1}))), json!({"label": "Needs you", "tone": "warn"}));
        assert_eq!(pill(with(json!({"failed": 1}))), json!({"label": "Failed", "tone": "err"}));
        assert_eq!(pill(with(json!({"done": 4}))), json!({"label": "Done", "tone": "ok"}));
        assert_eq!(pill(with(json!({"done": 3, "review": 1}))), json!({"label": "In review", "tone": "busy"}));
        assert_eq!(pill(with(json!({"failed": 2, "review": 1}))), json!({"label": "In review", "tone": "busy"}));
        assert_eq!(pill(with(json!({"blocked": 1}))), json!({"label": "Blocked", "tone": "warn"}));
        assert_eq!(pill(with(json!({"paused": true}))), json!({"label": "Paused", "tone": "idle"}));
        assert_eq!(pill(with(json!({"queued": 2}))), json!({"label": "Queued", "tone": "idle"}));
        assert_eq!(pill(idle), json!({"label": "Idle", "tone": "idle"}));
    }

    #[test]
    fn an_empty_goal_has_no_progress_bar() {
        let p = goal_preview(&goal(json!({"done": 0, "total": 0, "tasks": [], "prs_open": []})));
        assert!(p.get("progress").is_none());
        assert_eq!(p["fields"], json!([["Tasks", "0 of 0 done"]]));
    }

    #[test]
    fn a_backlog_item_shows_its_kind_task_and_detail() {
        let b = json!({
            "ref": "B4", "title": "Login drops the session", "project": "midna", "state": "task", "kind_label": "Bug",
            "task_ref": "T9", "from": "Found by T3 · api · 2:05 PM", "goal": {"ref": "G2", "name": "Polish"},
            "detail": "First line\n\nSecond line",
        });
        assert_eq!(
            issue_preview(&b),
            json!({
                "title": "B4 · Login drops the session",
                "subtitle": "G2 · Polish · midna",
                "status": {"label": "Now a task", "tone": "ok"},
                "fields": [["Kind", "Bug"], ["Task", "T9"], ["From", "Found by T3 · api · 2:05 PM"]],
                "lines": ["First line", "Second line"],
                "open_label": "Open item",
            })
        );
        let open = json!({"ref": "B5", "title": "x", "project": "p", "state": "open", "goal": null, "task_ref": null});
        let p = issue_preview(&open);
        assert_eq!(p["status"], json!({"label": "Open", "tone": "warn"}));
        assert_eq!(p["subtitle"], json!("p"));
        assert_eq!(p["fields"], json!([]));
    }
}
