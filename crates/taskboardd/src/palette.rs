//! The app's search palette (⌘K): which tasks and backlog issues match what's typed, scored the way
//! the palette ranks them, so `GET /search?palette=1` sends the best few and a count instead of
//! every task and issue. The app scores the same way (it uses these functions) to mark the matches.

use std::collections::HashMap;
use std::ops::Range;

use serde_json::{json, Value};

use crate::app::App;
use crate::board;
use crate::p;
use crate::util::*;

/// Where `needle` (already lower case) first sits in `hay`, ignoring case, as a byte range of `hay`.
pub fn find_ci(hay: &str, needle: &str) -> Option<Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    let want: Vec<char> = needle.chars().collect();
    let chars: Vec<(usize, char)> = hay.char_indices().collect();
    let low = |c: char| c.to_lowercase().next().unwrap_or(c);
    (0..chars.len()).find_map(|start| {
        let ok = want.len() <= chars.len() - start && want.iter().enumerate().all(|(k, w)| low(chars[start + k].1) == *w);
        ok.then(|| {
            let end = chars.get(start + want.len()).map(|(b, _)| *b).unwrap_or(hay.len());
            chars[start].0..end
        })
    })
}

/// The query as lower-case words.
pub fn words(q: &str) -> Vec<String> {
    q.split_whitespace().map(str::to_lowercase).collect()
}

/// How well an object matches: `None` when a word is found nowhere, else lower is better —
/// its ref, then its title's start, a word in its title, anywhere in its title, then its other text.
/// The highlights are each word's first place in the title.
pub fn score(r: &str, title: &str, other: &[&str], words: &[String]) -> Option<(u8, Vec<Range<usize>>)> {
    let r = r.to_lowercase();
    let mut best = u8::MAX;
    let mut hl = Vec::new();
    for w in words {
        let in_title = find_ci(title, w);
        let rank = if !r.is_empty() && r == *w {
            0
        } else if !r.is_empty() && r.starts_with(w.as_str()) {
            1
        } else if let Some(at) = &in_title {
            if at.start == 0 {
                2
            } else if title[..at.start].ends_with(|c: char| !c.is_alphanumeric()) {
                3
            } else {
                4
            }
        } else if other.iter().any(|o| find_ci(o, w).is_some()) {
            5
        } else {
            return None;
        };
        best = best.min(rank);
        hl.extend(in_title);
    }
    hl.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for h in hl {
        match merged.last_mut() {
            Some(last) if h.start <= last.end => last.end = last.end.max(h.end),
            _ => merged.push(h),
        }
    }
    Some((best, merged))
}

/// A task's place among equal matches, close to the app's status order (needs you, failed, working,
/// queued, planned, done); the app sorts what it's sent by the exact one.
fn task_order(t: &Row) -> u8 {
    let done = t.s("status") == Some("done");
    let st = match t.s("status") {
        _ if t.b("failed") && !done => 1,
        Some("needs") => 0,
        Some("working") => 2,
        Some("queued") => 4,
        Some("planned") => 5,
        _ => 6,
    };
    done as u8 * 10 + st
}

/// An issue that's no longer open: made a task or a ticket, dropped or deferred.
fn issue_closed(b: &Row) -> bool {
    matches!(b.s("state"), Some("task" | "ticket" | "drop" | "defer"))
}

/// Best first: the score, then each kind's own order; the first `limit` and how many matched.
fn best<T>(mut found: Vec<((u8, u8, i64), T)>, limit: usize) -> (Vec<T>, usize) {
    found.sort_by_key(|(k, _)| *k);
    let total = found.len();
    (found.into_iter().take(limit).map(|(_, t)| t).collect(), total)
}

/// `GET /search?palette=1&q=…&limit=…`: the best `limit` tasks and backlog issues for the palette,
/// as `GET /tasks` and `GET /backlog` send them, with how many of each match. With nothing typed:
/// the tasks still to do and no issues.
pub fn find(app: &App, q: &str, limit: usize) -> Result<Value> {
    let w = words(q);
    let goals: HashMap<i64, String> = app.db.q("SELECT id, name FROM goals", p![])?.iter().map(|g| (g.id(), g.st("name"))).collect();
    let goal = |id: Option<i64>| id.and_then(|g| goals.get(&g)).map(String::as_str).unwrap_or("");

    let mut tasks = vec![];
    for t in app.db.q("SELECT * FROM tasks", p![])? {
        let open = t.s("status") != Some("done");
        let sc = if w.is_empty() {
            if !open {
                continue;
            }
            0
        } else {
            let other = [t.s("project").unwrap_or(""), goal(t.i("goal_id"))];
            match score(&rf("task", t.id()), t.s("title").unwrap_or(""), &other, &w) {
                Some((sc, _)) => sc,
                None => continue,
            }
        };
        tasks.push(((sc, task_order(&t), -t.id()), t));
    }
    let (tasks, tasks_total) = best(tasks, limit);

    let mut issues = vec![];
    if !w.is_empty() {
        for b in app.db.q("SELECT * FROM issues", p![])? {
            let other = [b.s("project").unwrap_or(""), goal(b.i("goal_id")), b.s("area").unwrap_or(""), b.s("detail").unwrap_or(""), b.s("said").unwrap_or("")];
            if let Some((sc, _)) = score(&rf("issue", b.id()), b.s("title").unwrap_or(""), &other, &w) {
                issues.push(((sc, issue_closed(&b) as u8, -b.id()), b));
            }
        }
    }
    let (issues, issues_total) = best(issues, limit);

    let tasks: Result<Vec<Value>> = tasks.iter().map(|t| board::task_card(app, t)).collect();
    let mut cards = vec![];
    for b in &issues {
        let mut d = board::issue_card(app, b)?;
        d["said"] = b.v("said");
        d["detail"] = b.v("detail");
        cards.push(d);
    }
    Ok(json!({"tasks": tasks?, "tasks_total": tasks_total, "issues": cards, "issues_total": issues_total}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> (tempfile::TempDir, std::sync::Arc<App>) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::for_tests(crate::config::Config::for_tests(dir.path()));
        (dir, app)
    }

    fn search(app: &App, q: &str, limit: &str) -> Value {
        let q: crate::api::Query = [("palette", "1"), ("q", q), ("limit", limit)].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        crate::api::dispatch(app, "GET", "/search", &q, &json!({})).unwrap()
    }

    fn refs(v: &Value, k: &str) -> Vec<String> {
        v[k].as_array().unwrap().iter().map(|c| c["ref"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn sends_the_best_few_and_counts_every_match() {
        let (_d, app) = board();
        for n in 0..300 {
            let status = if n % 3 == 0 { "done" } else { "queued" };
            app.db.insert("tasks", crate::fields!["title" => format!("Passkey step {n}"), "project" => "webapp", "status" => status]).unwrap();
        }
        let needs = app.db.insert("tasks", crate::fields!["title" => "Fix the passkey prompt", "project" => "webapp", "status" => "needs"]).unwrap();
        let g = app.db.insert("goals", crate::fields!["name" => "Passkeys", "project" => "webapp"]).unwrap();
        let other = app.db.insert("tasks", crate::fields!["title" => "Settings page", "project" => "webapp", "status" => "queued", "goal_id" => g]).unwrap();
        let b = app.db.insert("issues", crate::fields!["project" => "webapp", "kind" => "bug", "title" => "Prompt flickers", "detail" => "On a passkey login", "state" => "open"]).unwrap();
        app.db.insert("issues", crate::fields!["project" => "webapp", "kind" => "bug", "title" => "Dark mode", "state" => "open"]).unwrap();

        let v = search(&app, "passkey", "6");
        assert_eq!(v["tasks_total"], 302, "every match counts, the goal's too");
        assert_eq!(v["tasks"].as_array().unwrap().len(), 6);
        // A word starting the title beats one inside it; among those, still to do and newest first.
        assert_eq!(refs(&v, "tasks")[0], "T300");
        assert!(!refs(&v, "tasks").contains(&format!("T{needs}")));
        assert_eq!(refs(&v, "issues"), [format!("B{b}")]);
        assert_eq!(v["issues"][0]["detail"], "On a passkey login");
        assert_eq!(v["issues_total"], 1);

        assert_eq!(refs(&search(&app, &format!("t{other}"), "6"), "tasks")[0], format!("T{other}"), "a ref first");
        let none = search(&app, "", "500");
        assert_eq!(none["tasks_total"], 202, "nothing typed: the open tasks");
        assert_eq!(refs(&none, "tasks")[0], format!("T{needs}"), "the one that needs you first");
        assert_eq!(none["issues_total"], 0);
    }
}
