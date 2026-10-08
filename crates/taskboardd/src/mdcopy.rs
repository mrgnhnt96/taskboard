//! A plain-text copy of each task at `<data>/tasks/T<n>.md`, readable without the server.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::app::App;
use crate::util::*;
use crate::{board, p};

fn status_label(s: &str) -> &str {
    match s {
        "planned" => "Planned",
        "queued" => "Queued",
        "working" => "Working",
        "needs" => "Needs you",
        "done" => "Done",
        other => other,
    }
}

fn list(ctx: &Row, k: &str) -> Vec<String> {
    ctx.get(k)
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default()
}

pub fn render(app: &App, t: &Row) -> Result<String> {
    let ctx = board::task_context(t);
    let w = ctx.get("where").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    let g = board::find_goal(app, t.i("goal_id"))?;
    let mut status = if t.b("failed") { "Failed".to_string() } else { status_label(&t.st("status")).to_string() };
    if t.b("lost") {
        status += " (terminal lost)";
    }
    let mut lines = vec![format!("# {} · {}", rf("task", t.id()), t.st("title")), String::new()];
    let rows: Vec<(&str, Option<String>)> = vec![
        ("Status", Some(status)),
        ("Project", t.s("project").map(|s| s.to_string())),
        ("Folder", t.s("repo_path").map(|s| s.to_string())),
        ("Goal", g.as_ref().map(|g| format!("{} · {}", rf("goal", g.id()), g.st("name")))),
        ("Terminal", t.s("session_name").map(|s| s.to_string())),
        ("Terminal id", t.s("session_id").map(|s| s.to_string())),
        ("Claude conversation", t.s("claude_session_id").map(|s| s.to_string())),
        ("Jira", t.s("jira_key").filter(|k| !k.is_empty()).map(|k| format!("{k} · {}", t.s("jira_status").unwrap_or("status unknown")))),
        ("PR", t.s("pr_url").filter(|u| !u.is_empty()).map(|u| format!("#{} · {u}", t.i0("pr_num")))),
    ];
    for (k, v) in rows {
        if let Some(v) = v.filter(|v| !v.is_empty()) {
            lines.push(format!("- **{k}:** {v}"));
        }
    }
    if let Some(d) = t.s("detail").filter(|d| !d.trim().is_empty()) {
        lines.extend(["".into(), "## What to do".into(), "".into(), d.trim().to_string()]);
    }
    let mut cl = vec![];
    for (label, key) in [("Done", "done"), ("Next", "next"), ("Decisions", "decisions"), ("Answers", "answers"), ("Files", "files")] {
        let items = list(&ctx, key);
        if !items.is_empty() {
            cl.push(format!("**{label}:**"));
            cl.extend(items.iter().map(|i| format!("- {i}")));
        }
    }
    let wl: Vec<String> = ["branch", "worktree", "last_commit", "uncommitted"]
        .iter()
        .filter_map(|k| w.get(*k).filter(|v| !v.is_null()).map(|v| format!("{k}: {}", v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string()))))
        .collect();
    if !wl.is_empty() {
        cl.push(format!("**Where:** {}", wl.join(" · ")));
    }
    if !cl.is_empty() {
        lines.extend(["".into(), "## Context".into(), "".into()]);
        lines.extend(cl);
    }
    let meta = jloads_arr(t.s("meta"));
    if !meta.is_empty() {
        lines.extend(["".into(), "## Details".into(), "".into()]);
        for m in meta {
            if let Some(a) = m.as_array().filter(|a| a.len() == 2) {
                lines.push(format!("- {}: {}", a[0].as_str().unwrap_or(""), a[1].as_str().map(|s| s.to_string()).unwrap_or_else(|| a[1].to_string())));
            }
        }
    }
    lines.extend(["".into(), "## Log".into(), "".into()]);
    for e in app.db.q("SELECT * FROM events WHERE task_id = ? ORDER BY at, id", p![t.id()])? {
        lines.push(format!("- {} · {} · {}", e.st("at"), e.st("who"), e.st("text").replace('\n', " ")));
    }
    Ok(lines.join("\n") + "\n")
}

pub fn write(app: &App, task_id: i64) -> Result<()> {
    let dir = app.cfg.md_dir();
    let path = dir.join(format!("{}.md", rf("task", task_id)));
    let Some(t) = board::find_task(app, Some(task_id))? else {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    };
    let text = render(app, &t)?;
    std::fs::create_dir_all(&dir).ok();
    let tmp = dir.join(format!(".{}.md.tmp", rf("task", task_id)));
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
    Ok(())
}

pub fn writer_loop(app: Arc<App>) {
    while !app.stopping() {
        let ids = app.take_md(Duration::from_secs(5));
        if ids.is_empty() {
            continue;
        }
        app.sleep(app.cfg.intervals.md_debounce);
        let mut all: Vec<i64> = ids;
        all.extend(app.take_md(Duration::from_millis(1)));
        all.sort();
        all.dedup();
        for id in all {
            if let Err(e) = write(&app, id) {
                app.info(format!("md: couldn't write {}: {e}", rf("task", id)));
            }
        }
    }
}

pub fn _unused(_: Value) {}
